// Claktory's browser extension: its service worker. It holds the only channel
// to Claktory (native messaging, to Claktory's browser helper) and at most one
// session: one tab, one origin, one document at a time. The helper's frames
// are checked here (PROTOCOL.md), nothing reads the page until the person has
// confirmed the session on their Mac, and nothing is clicked, typed or
// navigated until they allow that one action there. Everything is in memory:
// a session never outlives this worker, and nothing is remembered for a site.
import {HOST} from './config.js';

const LIMITS = {
  hardMs: 15 * 60_000, // a session's hard limit, whatever the Mac says
  idleMs: 5 * 60_000, // with no command
  pairingMs: 5 * 60_000, // waiting for the person to confirm on the Mac
  commands: 100,
  bytes: 2 * 1024 * 1024, // returned, over the session
  resultBytes: 64 * 1024, // one result
  minRead: 1024, // less room than this left: the session is at its limit
  answerMs: 3000, // the page answers within this, or it doesn't count
  reviewMs: 120_000, // the person decides on an action within this
  returnMs: 10_000, // after Allow, the person is back in the tab within this
  loadMs: 10_000, // a page a navigation opens loads within this
  actionBytes: 64 * 1024, // what a review shows, as JSON
  fillText: 8192, // characters
};

// What the helper itself ends a session with (PROTOCOL.md).
const HELPER_ENDINGS = ['no_app', 'app_refused', 'app_closed', 'wrong_tab', 'bad_frame', 'bad_command'];
const POPUP = chrome.runtime.getURL('popup.html');
const TIMEOUT = Symbol('timeout');

/** The session, while there is one. */
let session = null;
/** How the last one ended, for the popup: {reason, tabId, origin, agent, bound}. */
let last = null;

const PAUSED_WHY = {
  tab_inactive: 'another tab became active: the person resumes it in the popup',
  window_blurred: 'Chrome isn\'t the window in front: the person resumes it in the popup',
  popup_opened: 'the page opened a new tab or window: the person resumes it in the popup',
  dialog_open: 'the page isn\'t answering, a dialog may be open in it: the person resumes it in the popup',
};

// ---- Addresses ----

function originOf(url) {
  try { return new URL(url).origin; } catch { return null; }
}

/** Why Claktory won't read a page at `url`, or null when it may. */
function refusal(url) {
  let u;
  try { u = new URL(url); } catch { return 'scheme'; }
  if (u.protocol !== 'https:') return 'scheme';
  if (u.username || u.password) return 'credentials';
  const host = u.hostname.toLowerCase();
  // The URL parser writes every IPv4 form (0x7f.1, 2130706433) as a dotted
  // quad, and IPv6 in brackets.
  if (host.startsWith('[') || /^\d{1,3}(\.\d{1,3}){3}$/.test(host)) return 'address';
  if (host === 'localhost' || host.endsWith('.localhost') || host === 'local' || host.endsWith('.local')) return 'local';
  return null;
}

// Characters that change how text reads without showing: escaped, as the
// page's text is (content.js).
const BIDI = /[\u061C\u200E\u200F\u202A-\u202E\u2066-\u2069]/g;
// Zero-width and invisible characters (U+2061–2064 are invisible operators, U+2065 is held for more).
const ZERO_WIDTH = /[\u200B-\u200D\u2060-\u2065\uFEFF]/g;
const CONTROL = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F]/g;
const hex = (c) => '⟦U+' + c.codePointAt(0).toString(16).toUpperCase().padStart(4, '0') + '⟧';
const LONE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g;
function oneLine(s, limit) {
  s = s.replace(LONE, '\uFFFD').trim().replace(/\r\n|\r|\n|\u2028|\u2029/g, '⏎').replace(/[ \t\f\v]+/g, ' ').replace(BIDI, hex).replace(ZERO_WIDTH, hex).replace(CONTROL, hex);
  if (s.length <= limit) return s;
  let keep = limit - 1;
  if (/[\uD800-\uDBFF]/.test(s[keep - 1])) keep--;
  return s.slice(0, keep) + '…';
}

/**
 * Text to be typed, as the person reviews it: exactly, with nothing that
 * doesn't show. A line break shows as ⏎; tabs, other controls, bidi and
 * zero-width characters as ⟦U+0009⟧; a ⏎ or ⟦ the text itself has, the same
 * way, so the escapes can't be faked.
 */
function exactly(text) {
  return text.replace(LONE, '\uFFFD').replace(/[⟦⏎]/g, hex).replace(/\r\n|\n/g, '⏎')
    .replace(/[\t\r\u2028\u2029]/g, hex).replace(BIDI, hex).replace(ZERO_WIDTH, hex).replace(CONTROL, hex);
}

/** JSON with keys sorted at every level and no spaces: what the hash is over (PROTOCOL.md). */
function canonical(value) {
  if (Array.isArray(value)) return '[' + value.map(canonical).join(',') + ']';
  if (value && typeof value === 'object') return '{' + Object.keys(value).sort().map((k) => JSON.stringify(k) + ':' + canonical(value[k])).join(',') + '}';
  if (typeof value === 'number' && !Number.isSafeInteger(value)) throw new Error('only whole numbers are hashed');
  return JSON.stringify(value);
}

async function sha256(text) {
  const digest = await crypto.subtle.digest('SHA-256', encoder.encode(text));
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, '0')).join('');
}

const encoder = new TextEncoder();
const utf8 = (s) => encoder.encode(s).length;

function bounded(promise, ms) {
  let timer;
  return Promise.race([
    promise.then((value) => value, (error) => (error instanceof Error ? error : new Error(String(error)))),
    new Promise((resolve) => { timer = setTimeout(() => resolve(TIMEOUT), ms); }),
  ]).finally(() => clearTimeout(timer));
}

function token() {
  const bytes = crypto.getRandomValues(new Uint8Array(12));
  return 'd' + Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

// ---- The session ----

/** Starts a session on the tab the person opened the popup on. */
async function start(tabId) {
  if (session) return {error: session.tabId === tabId ? 'busy_here' : 'busy'};
  const tab = await chrome.tabs.get(tabId).catch(() => null);
  // Without the person's gesture on this tab, Chrome hides its address.
  if (!tab || !tab.url) return {error: 'no_access'};
  if (tab.incognito) return {error: 'private'};
  const why = refusal(tab.url);
  if (why) return {error: why};
  if (session) return {error: 'busy'};
  const s = {
    tabId, windowId: tab.windowId, origin: originOf(tab.url), title: oneLine(tab.title || '', 300),
    state: 'pairing', binding: null, code: null, agent: null, purpose: null, deadline: 0,
    documentId: null, document: null, lastId: 0, commandSession: null, lastSequence: -1, commands: 0, bytes: 0, busy: false, paused: null,
    review: null, returning: false, timers: {}, port: null,
  };
  session = s;
  last = null;
  try {
    s.port = chrome.runtime.connectNative(HOST);
  } catch {
    session = null;
    last = {reason: 'no_helper', tabId, origin: s.origin, agent: null, bound: false};
    return {error: 'no_helper'};
  }
  s.port.onMessage.addListener((frame) => { onHelper(s, frame); });
  s.port.onDisconnect.addListener(() => {
    const why = (chrome.runtime.lastError && chrome.runtime.lastError.message) || '';
    end(s, /not found|forbidden/i.test(why) ? 'no_helper' : 'helper_gone', false);
  });
  send(s, {type: 'hello', v: 1, tab: tabId, origin: s.origin, title: s.title});
  s.timers.pairing = setTimeout(() => { if (s.state !== 'active') end(s, 'expired', true); }, LIMITS.pairingMs);
  return {ok: true};
}

function send(s, frame) {
  try { s.port.postMessage(frame); } catch { end(s, 'helper_gone', false); }
}

/** Ends the session, once: `tell` says the helper hears why. */
function end(s, reason, tell) {
  if (session !== s) return;
  session = null;
  for (const timer of Object.values(s.timers)) clearTimeout(timer);
  // An action waiting for the person is never done.
  if (s.review && !s.review.decided) { s.review.decided = true; s.review.settle('ended'); }
  if (tell && s.port) {
    // The helper exits once it has passed this on; letting go at once could
    // drop the frame on its way, so the port closes from the helper's side,
    // or here a second later.
    send(s, {type: 'ended', tab: s.tabId, reason});
    setTimeout(() => { try { s.port.disconnect(); } catch {} }, 1000);
  } else {
    try { if (s.port) s.port.disconnect(); } catch {}
  }
  chrome.action.setBadgeText({tabId: s.tabId, text: ''}).catch(() => {});
  // The page keeps no element references once the session is over.
  if (s.documentId) chrome.tabs.sendMessage(s.tabId, {op: 'forget'}, {documentId: s.documentId}).catch(() => {});
  last = {reason, tabId: s.tabId, origin: s.origin, agent: s.agent, bound: s.state === 'active'};
}

/** The helper broke the protocol: the session ends, and it's told why. */
function refuse(s) {
  end(s, 'helper_refused', true);
}

/** Exactly these keys: `required`, and any of `optional`. */
function exact(frame, required, optional = []) {
  if (!frame || typeof frame !== 'object' || Array.isArray(frame)) return false;
  const keys = Object.keys(frame);
  return required.every((k) => keys.includes(k)) && keys.every((k) => required.includes(k) || optional.includes(k));
}

const isId = (v) => typeof v === 'string' && v.length >= 1 && v.length <= 128;

/** A command's op, checked: read, scroll, click, fill or navigate, nothing else. */
function op(cmd) {
  if (cmd && (cmd.op === 'click' || cmd.op === 'fill') && exact(cmd, cmd.op === 'fill' ? ['op', 'document', 'revision', 'target', 'text'] : ['op', 'document', 'revision', 'target'])) {
    if (!isId(cmd.document) || !Number.isSafeInteger(cmd.revision) || cmd.revision < 0 || !isId(cmd.target) || cmd.target.length > 64) return null;
    if (cmd.op === 'click') return {op: 'click', document: cmd.document, target: cmd.target};
    // Text to type: no controls but line breaks and tabs.
    if (typeof cmd.text !== 'string' || cmd.text.length > LIMITS.fillText || /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F]/.test(cmd.text)) return null;
    return {op: 'fill', document: cmd.document, target: cmd.target, text: cmd.text};
  }
  if (cmd && cmd.op === 'navigate' && exact(cmd, ['op', 'url'])) {
    if (typeof cmd.url !== 'string' || cmd.url.length > 2048) return null;
    return {op: 'navigate', url: cmd.url};
  }
  if (cmd && cmd.op === 'read' && exact(cmd, ['op'], ['limit'])) {
    if (cmd.limit === undefined) return {op: 'read', limit: LIMITS.resultBytes};
    if (Number.isInteger(cmd.limit) && cmd.limit >= 1 && cmd.limit <= 65536) return {op: 'read', limit: cmd.limit};
    return null;
  }
  if (cmd && cmd.op === 'scroll' && exact(cmd, ['op', 'direction', 'pages'])) {
    if (['up', 'down'].includes(cmd.direction) && Number.isInteger(cmd.pages) && cmd.pages >= 1 && cmd.pages <= 3) return {op: 'scroll', direction: cmd.direction, pages: cmd.pages};
  }
  return null;
}

function onHelper(s, frame) {
  if (session !== s) return;
  const type = frame && typeof frame === 'object' ? frame.type : undefined;
  if (type === 'pairing') {
    if (!exact(frame, ['type', 'code']) || typeof frame.code !== 'string' || !/^[0-9]{6}$/.test(frame.code) || s.state !== 'pairing') return refuse(s);
    s.code = frame.code;
    return;
  }
  if (type === 'bound') {
    const fine = exact(frame, ['type', 'agent', 'until_ms'], ['purpose'])
      && typeof frame.agent === 'string' && frame.agent.trim() && frame.agent.length <= 100
      && Number.isSafeInteger(frame.until_ms)
      && (frame.purpose === undefined || (typeof frame.purpose === 'string' && frame.purpose.length <= 500));
    if (!fine || s.state !== 'pairing' || !s.code) return refuse(s);
    s.binding = bind(s, frame);
    return;
  }
  if (type === 'command') {
    // Ids and sequences only rise, within one session the helper names the
    // same way every time; one command at a time.
    if (!exact(frame, ['type', 'id', 'session', 'sequence', 'cmd']) || !Number.isSafeInteger(frame.id) || frame.id <= s.lastId
      || !isId(frame.session) || (s.commandSession !== null && frame.session !== s.commandSession)
      || !Number.isSafeInteger(frame.sequence) || frame.sequence <= s.lastSequence || s.busy) return refuse(s);
    const cmd = op(frame.cmd);
    if (!cmd) return refuse(s);
    s.lastId = frame.id;
    s.commandSession = frame.session;
    s.lastSequence = frame.sequence;
    answer(s, frame, cmd);
    return;
  }
  if (type === 'decision') {
    // The person's Allow or Deny on their Mac, for the review waiting, once.
    const review = s.review;
    if (!exact(frame, ['type', 'id', 'hash', 'allow']) || typeof frame.allow !== 'boolean' || !review || review.decided
      || frame.id !== review.id || frame.hash !== review.hash) return refuse(s);
    review.decided = true;
    review.settle(frame.allow ? 'allow' : 'deny');
    return;
  }
  if (type === 'stop') {
    if (!exact(frame, ['type'])) return refuse(s);
    return end(s, 'stopped_in_claktory', false);
  }
  if (type === 'ended') {
    if (!exact(frame, ['type', 'reason']) || !HELPER_ENDINGS.includes(frame.reason)) return refuse(s);
    return end(s, frame.reason, false);
  }
  refuse(s);
}

/** The tab, while it's still on the session's origin; otherwise the session ends. */
async function currentTab(s) {
  const tab = await chrome.tabs.get(s.tabId).catch(() => null);
  if (session !== s) return null;
  if (!tab) { end(s, 'tab_closed', true); return null; }
  if (!tab.url || originOf(tab.url) !== s.origin) { end(s, 'origin_changed', true); return null; }
  return tab;
}

/** Puts the reader into the tab's top document: true when it's there. */
async function inject(s) {
  const done = await bounded(chrome.scripting.executeScript({target: {tabId: s.tabId, frameIds: [0]}, files: ['content.js']}), LIMITS.answerMs);
  if (session !== s || done === TIMEOUT || done instanceof Error || !done[0] || !done[0].documentId) return false;
  s.documentId = done[0].documentId;
  s.document = token();
  return true;
}

/** The person confirmed on their Mac: the session starts reading. */
async function bind(s, frame) {
  s.state = 'binding';
  const remaining = Math.min(frame.until_ms - Date.now(), LIMITS.hardMs);
  if (remaining <= 0) return end(s, 'expired', true);
  if (!await currentTab(s)) return;
  if (!await inject(s)) return end(s, 'origin_changed', true);
  if (session !== s) return;
  s.agent = frame.agent;
  s.purpose = frame.purpose === undefined ? null : frame.purpose;
  s.state = 'active';
  s.deadline = performance.now() + remaining;
  clearTimeout(s.timers.pairing);
  s.timers.hard = setTimeout(() => end(s, 'expired', true), remaining);
  idle(s);
  chrome.action.setBadgeBackgroundColor({tabId: s.tabId, color: '#E5480F'}).catch(() => {});
  chrome.action.setBadgeText({tabId: s.tabId, text: 'ON'}).catch(() => {});
}

function idle(s) {
  clearTimeout(s.timers.idle);
  s.timers.idle = setTimeout(() => end(s, 'expired', true), LIMITS.idleMs);
}

function pause(s, reason) {
  if (session !== s || s.state !== 'active' || s.paused) return;
  s.paused = reason;
  // An action waiting for the person's review or return isn't done.
  if (s.review && !s.review.decided) { s.review.decided = true; s.review.settle('paused'); }
  send(s, {type: 'paused', tab: s.tabId, reason});
  chrome.action.setBadgeText({tabId: s.tabId, text: 'II'}).catch(() => {});
}

/** The person resumes, in the popup, on the session's tab in front. */
async function resume() {
  const s = session;
  if (!s || !s.paused) return {error: 'not_paused'};
  const tab = await chrome.tabs.get(s.tabId).catch(() => null);
  const front = await chrome.windows.getLastFocused().catch(() => null);
  if (session !== s || !tab || !tab.active || !front || front.id !== tab.windowId) return {error: 'not_in_front'};
  s.windowId = tab.windowId;
  s.paused = null;
  send(s, {type: 'resumed', tab: s.tabId});
  chrome.action.setBadgeText({tabId: s.tabId, text: 'ON'}).catch(() => {});
  return {ok: true};
}

async function answer(s, frame, cmd) {
  const id = frame.id;
  s.busy = true;
  let result;
  try {
    result = ['click', 'fill', 'navigate'].includes(cmd.op) ? await action(s, frame, cmd) : await run(s, cmd);
  } catch {
    result = {error: 'unsupported_page', why: 'the page couldn\'t be read'};
  }
  s.busy = false;
  // Ended meanwhile: nothing it found goes out.
  if (session !== s || result === null) return;
  let size = utf8(JSON.stringify(result));
  if (size > LIMITS.resultBytes) {
    result = {error: 'unsupported_page', why: 'what the page gave didn\'t fit in a result'};
    size = utf8(JSON.stringify(result));
  }
  s.bytes += size;
  send(s, {type: 'result', tab: s.tabId, id, result});
  if (s.over) end(s, 'limit_exceeded', true);
}

const READ_KEYS = ['revision', 'title', 'title_marks', 'path', 'text', 'text_marks', 'truncated', 'elements', 'elements_not_listed', 'unsupported', 'unsupported_page'];

/**
 * What every command checks first: the session is bound, not paused and
 * within its limits; the tab is on its origin; and which document is in it.
 * The command's answer when it can't go on, or null when it can (`s.over`
 * and the rest set), or `null` with the session gone: the caller checks.
 */
async function ready(s, cmd) {
  // Bound, with the reader on its way into the page: the command waits for it.
  if (s.state === 'binding') await s.binding;
  if (session !== s) return {gone: true};
  if (s.state !== 'active') return {answer: {error: 'approval_pending', why: 'the person hasn\'t confirmed this session on their Mac yet'}};
  if (s.paused) return {answer: {error: 'paused', why: PAUSED_WHY[s.paused]}};
  if (s.commands >= LIMITS.commands) { s.over = true; return {answer: {error: 'limit_exceeded', why: 'a session allows 100 commands'}}; }
  const room = LIMITS.bytes - s.bytes;
  if (room < LIMITS.minRead) { s.over = true; return {answer: {error: 'limit_exceeded', why: 'a session returns 2 MiB at most'}}; }
  s.commands++;
  idle(s);
  const tab = await currentTab(s);
  if (!tab) return {gone: true};
  // Which document is in the tab now: a command sent to an earlier one would
  // never answer (the back/forward cache keeps it), and a page held by a
  // dialog runs nothing, so this is bounded too.
  const now = await documentNow(s);
  if (now === null) return {gone: true};
  if (now.error) return {answer: now};
  if (now.id !== s.documentId) {
    // The tab loaded another page of the same origin. A read starts on it
    // afresh; anything else needs that read first.
    if (cmd.op !== 'read') return {answer: {error: 'stale_document', why: 'the page loaded a new document: read it again'}};
    if (!await inject(s)) return session === s ? {answer: {error: 'stale_document', why: 'the new page couldn\'t be read yet: read it again'}} : {gone: true};
  }
  return {tab, room};
}

/** Chrome's id for the document in the tab now: {id}, a failure, or null with the session gone. */
async function documentNow(s) {
  const probe = await bounded(chrome.scripting.executeScript({target: {tabId: s.tabId, frameIds: [0]}, func: () => 0}), LIMITS.answerMs);
  if (session !== s) return null;
  if (probe === TIMEOUT) { pause(s, 'dialog_open'); return {error: 'paused', why: PAUSED_WHY.dialog_open}; }
  if (probe instanceof Error || !probe[0]) {
    if (!await currentTab(s)) return null;
    return {error: 'stale_document', why: 'the page is changing: read it again'};
  }
  return {id: probe[0].documentId};
}

/** A message to the reader in the session's document, bounded. */
function page(s, message) {
  return bounded(chrome.tabs.sendMessage(s.tabId, message, {documentId: s.documentId}), LIMITS.answerMs);
}

/** Runs a read or a scroll: its result, or null when the session ended meanwhile. */
async function run(s, cmd) {
  const first = await ready(s, cmd);
  if (first.gone) return null;
  if (first.answer) return first.answer;
  const {room} = first;
  const message = cmd.op === 'read' ? {op: 'read', limit: cmd.limit, budget: Math.min(LIMITS.resultBytes, room) - 128} : cmd;
  const reply = await page(s, message);
  if (session !== s) return null;
  if (reply === TIMEOUT) return {error: 'stale_document', why: 'the page didn\'t answer in 3 seconds: read it again'};
  if (reply instanceof Error || !reply || typeof reply !== 'object') return {error: 'stale_document', why: 'the page is gone: read it again'};
  if (reply.error) return {error: reply.error === 'unsupported_page' ? 'unsupported_page' : 'stale_document', why: String(reply.why || '')};
  if (cmd.op === 'scroll') return {scroll_y: Number(reply.scroll_y) || 0};
  const result = {document: s.document};
  for (const key of READ_KEYS) result[key] = reply[key];
  return result;
}

// ---- Actions: reviewed by the person, one at a time ----

const PAGE_REFUSALS = ['stale_target', 'sensitive_action', 'unsupported_target', 'unsupported_page'];

/** A path for a review: no ? or #, at most 300 characters. */
function reviewPath(pathname) {
  return oneLine(pathname.replace(/[?#]/g, encodeURIComponent), 300);
}

/**
 * What the person reviews for this command, and its hash, or the answer when
 * it's refused: built from the page as it is now, so the same call just
 * before acting tells whether anything changed.
 */
async function review(s, frame, cmd, tab) {
  let action;
  if (cmd.op === 'navigate') {
    // Only another page of the session's own site, as Claktory sent it.
    let u = null;
    try { u = new URL(cmd.url); } catch {}
    if (!u || u.origin !== s.origin || u.username || u.password || /[?#\\]/.test(cmd.url) || !cmd.url.startsWith(s.origin + '/') && cmd.url !== s.origin) return {refused: true};
    action = {op: 'navigate', origin: s.origin, path: reviewPath(new URL(tab.url).pathname), destination: reviewPath(u.pathname), warnings: []};
  } else {
    if (cmd.document !== s.document) return {answer: {error: 'stale_document', why: 'that read was of an earlier page: read it again'}};
    const reply = await page(s, {op: 'describe', kind: cmd.op, target: cmd.target, text: cmd.op === 'fill' ? cmd.text : ''});
    if (session !== s) return {gone: true};
    if (reply === TIMEOUT || reply instanceof Error || !reply || typeof reply !== 'object') return {answer: {error: 'stale_document', why: 'the page didn\'t answer: read it again'}};
    if (reply.error) return {answer: {error: PAGE_REFUSALS.includes(reply.error) ? reply.error : 'stale_document', why: String(reply.why || '')}};
    const found = reply.action;
    const warnings = cmd.op === 'fill' ? ['may_send_as_typed'] : ['may_ask_permission'];
    if (found.submits_form) warnings.push('submits_form');
    if (found.leaves === 'other_origin') warnings.push('ends_session');
    delete found.submits_form;
    action = {op: cmd.op, origin: s.origin, ...found, warnings};
    if (cmd.op === 'fill') action.text = exactly(cmd.text);
  }
  if (utf8(JSON.stringify(action)) > LIMITS.actionBytes) return {answer: {error: 'unsupported_target', why: 'too long to show for review'}};
  const hash = await sha256(canonical({
    action, command: frame.id, document: cmd.op === 'navigate' ? null : cmd.document, op: frame.cmd, sequence: frame.sequence,
    session: frame.session, target: cmd.op === 'navigate' ? null : cmd.target,
  }));
  if (session !== s) return {gone: true};
  return {action, hash};
}

/** True once the session's tab is the active one in the focused window, within `ms`. */
async function front(s, ms) {
  const end = Date.now() + ms;
  while (session === s && !s.paused) {
    const tab = await chrome.tabs.get(s.tabId).catch(() => null);
    const win = tab && await chrome.windows.get(tab.windowId).catch(() => null);
    if (tab && tab.active && win && win.focused) return true;
    if (Date.now() >= end) return false;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return false;
}

/** After a review that ended without acting: Chrome not in front pauses the session. */
async function settleFocus(s) {
  if (!await front(s, 0)) pause(s, 'window_blurred');
}

function badge(s, text) {
  if (session === s) chrome.action.setBadgeText({tabId: s.tabId, text}).catch(() => {});
}

/** A click, fill or navigation: reviewed, then done once, if nothing changed. */
async function action(s, frame, cmd) {
  const first = await ready(s, cmd);
  if (first.gone) return null;
  if (first.answer) return first.answer;
  const shown = await review(s, frame, cmd, first.tab);
  if (shown.gone) return null;
  if (shown.refused) { refuse(s); return null; }
  if (shown.answer) return shown.answer;
  if (cmd.op !== 'navigate') {
    const watched = await page(s, {op: 'watch', target: cmd.target});
    if (session !== s) return null;
    if (!watched || watched === TIMEOUT || watched instanceof Error || watched.error) return {error: 'stale_target', why: 'removed'};
  }

  // The person reviews it on their Mac. Chrome going to the background
  // meanwhile is expected and doesn't pause the session; anything else does,
  // and then nothing is done.
  let settle;
  const decided = new Promise((resolve) => { settle = resolve; });
  s.review = {id: frame.id, hash: shown.hash, decided: false, settle};
  badge(s, '…');
  send(s, {type: 'review', tab: s.tabId, id: frame.id, hash: shown.hash, expires_ms: Date.now() + LIMITS.reviewMs, action: shown.action});
  const timer = setTimeout(() => { if (!s.review.decided) { s.review.decided = true; settle('expired'); } }, LIMITS.reviewMs);
  const decision = await decided;
  clearTimeout(timer);
  if (session !== s) return null;
  if (decision !== 'allow') {
    s.review = null;
    badge(s, s.paused ? 'II' : 'ON');
    if (decision === 'paused') return {error: 'paused', why: PAUSED_WHY[s.paused] + ', so it wasn\'t done'};
    await settleFocus(s);
    if (session !== s) return null;
    return decision === 'deny' ? {error: 'denied', why: 'the person denied it'} : {error: 'expired', why: 'the person didn\'t decide within 2 minutes'};
  }

  // Allowed: done only once the person is back in the tab, within 10 s.
  s.returning = true;
  s.review = null;
  badge(s, 'GO');
  const back = await front(s, LIMITS.returnMs);
  s.returning = false;
  if (session !== s) return null;
  badge(s, s.paused ? 'II' : 'ON');
  if (s.paused) return {error: 'paused', why: PAUSED_WHY[s.paused] + ', so it wasn\'t done'};
  if (!back) { pause(s, 'window_blurred'); return {error: 'expired', why: 'the person didn\'t go back to the tab within 10 seconds, so it wasn\'t done'}; }

  // Everything again, just before: the tab, its document, and what the
  // person saw, to the byte.
  const tab = await currentTab(s);
  if (!tab) return null;
  const now = await documentNow(s);
  if (now === null) return null;
  if (now.error) return now;
  if (now.id !== s.documentId) return {error: 'stale_document', why: 'the page loaded a new document while it was reviewed'};
  const again = await review(s, frame, cmd, tab);
  if (again.gone) return null;
  if (again.answer) return again.answer;
  if (again.refused || again.hash !== shown.hash) return {error: 'stale_target', why: 'it changed while it was reviewed'};

  if (cmd.op === 'navigate') return navigate(s, cmd.url);
  const done = await page(s, {op: 'act', kind: cmd.op, target: cmd.target, text: cmd.op === 'fill' ? cmd.text : ''});
  if (session !== s) return null;
  // Sent, and no answer: the page may have done it (a dialog its handler
  // opened holds it). Never tried again.
  if (done === TIMEOUT || done instanceof Error || !done || typeof done !== 'object') {
    return {error: 'outcome_unknown', why: 'the page didn\'t answer in 3 seconds after it was sent: it may or may not have happened'};
  }
  if (done.error) return {error: PAGE_REFUSALS.includes(done.error) ? done.error : 'stale_document', why: String(done.why || '')};
  if (cmd.op === 'click') return {done: true};
  const result = {done: true, took: done.took === true};
  if (typeof done.why === 'string') result.why = done.why;
  if (typeof done.now === 'string') result.now = done.now;
  if (Number.isSafeInteger(done.over_max_length)) result.over_max_length = done.over_max_length;
  return result;
}

/** Goes to another page of the same site, and puts the reader into it once it loads. */
async function navigate(s, url) {
  const loaded = new Promise((resolve) => {
    const timer = setTimeout(() => { chrome.tabs.onUpdated.removeListener(listen); resolve(false); }, LIMITS.loadMs);
    function listen(tabId, change) {
      if (tabId !== s.tabId || change.status !== 'complete') return;
      clearTimeout(timer);
      chrome.tabs.onUpdated.removeListener(listen);
      resolve(true);
    }
    chrome.tabs.onUpdated.addListener(listen);
  });
  const went = await chrome.tabs.update(s.tabId, {url}).then(() => true, () => false);
  if (!went) return session === s ? {error: 'outcome_unknown', why: 'Chrome didn\'t say whether it went'} : null;
  const ok = await loaded;
  // Another origin on the way ended the session already.
  const tab = session === s && await currentTab(s);
  if (!tab) return null;
  if (!ok) return {error: 'outcome_unknown', why: 'the page didn\'t finish loading in 10 seconds: read it to see where it is'};
  if (!await inject(s)) return session === s ? {error: 'stale_document', why: 'the new page couldn\'t be read yet: read it again'} : null;
  return {done: true, path: reviewPath(new URL(tab.url).pathname), document: s.document};
}

// ---- What the popup sees and does ----

function view(tabId) {
  const s = session;
  if (s) {
    return {
      state: s.state === 'active' ? (s.paused ? 'paused' : 'active') : 'pairing',
      here: s.tabId === tabId, origin: s.origin, title: s.title, code: s.code, agent: s.agent, purpose: s.purpose,
      remaining_ms: s.state === 'active' ? Math.max(0, Math.round(s.deadline - performance.now())) : null,
      paused: s.paused, commands: s.commands,
      waiting: s.returning ? 'return' : s.review ? 'review' : null,
    };
  }
  return {state: 'none', last: last && {reason: last.reason, here: last.tabId === tabId, origin: last.origin, agent: last.agent, bound: last.bound}};
}

async function popup(message) {
  if (!message || typeof message !== 'object') return {error: 'bad_request'};
  const tabId = Number.isInteger(message.tabId) ? message.tabId : null;
  switch (message.kind) {
    case 'view': {
      const page = tabId === null ? null : await chrome.tabs.get(tabId).catch(() => null);
      const why = !page || !page.url ? 'no_access' : page.incognito ? 'private' : refusal(page.url);
      return {...view(tabId), page: page && page.url ? {origin: originOf(page.url), title: oneLine(page.title || '', 300), refusal: why} : {refusal: why}};
    }
    case 'start':
      return tabId === null ? {error: 'no_access'} : start(tabId);
    case 'stop':
      if (session) end(session, 'stopped', true);
      return {ok: true};
    case 'resume':
      return resume();
    case 'close_tab': {
      // Chrome's own grant on the tab lasts until it leaves or closes: the
      // person closes it here, never Claktory by itself.
      const ours = (session && session.tabId === tabId) || (last && last.tabId === tabId && last.bound);
      if (!ours) return {error: 'not_ours'};
      if (session) end(session, 'stopped', true);
      await chrome.tabs.remove(tabId).catch(() => {});
      last = null;
      return {ok: true};
    }
    default:
      return {error: 'bad_request'};
  }
}

// Only this extension's own popup: never a page, a content script or another
// extension (externally_connectable lets none of them in either).
chrome.runtime.onMessage.addListener((message, sender, reply) => {
  if (sender.id !== chrome.runtime.id || sender.tab || sender.url !== POPUP) return false;
  popup(message).then(reply, () => reply({error: 'failed'}));
  return true;
});

// ---- The tab, the window and the page ----

// Another tab became the active one in the session's window.
chrome.tabs.onActivated.addListener(({tabId, windowId}) => {
  if (session && windowId === session.windowId && tabId !== session.tabId) pause(session, 'tab_inactive');
});
// Another window, or another app, came to the front.
// Not while the person reviews an action on their Mac, or is on the way back
// from it: that's Claktory's own review, and the tab is checked again first.
chrome.windows.onFocusChanged.addListener((windowId) => {
  if (session && windowId !== session.windowId && !session.review && !session.returning) pause(session, 'window_blurred');
});
// The page opened a tab or window: it's never read, and the person's
// attention may be there now.
chrome.tabs.onCreated.addListener((tab) => {
  if (session && tab.openerTabId === session.tabId) pause(session, 'popup_opened');
});
// The tab starts loading: if Chrome hides its address now (another origin
// took away the grant) or the origin differs, the session ends, even if a
// redirect would bring it back.
chrome.tabs.onUpdated.addListener((tabId, change) => {
  const s = session;
  if (!s || s.tabId !== tabId || (change.status !== 'loading' && change.url === undefined)) return;
  currentTab(s);
});
chrome.tabs.onRemoved.addListener((tabId) => {
  if (session && session.tabId === tabId) end(session, 'tab_closed', true);
});
// Chrome swapped a prerendered page in: another page, so it ends.
chrome.tabs.onReplaced.addListener((added, removed) => {
  if (session && session.tabId === removed) end(session, 'origin_changed', true);
});
chrome.tabs.onAttached.addListener((tabId, {newWindowId}) => {
  if (session && session.tabId === tabId) { session.windowId = newWindowId; pause(session, 'window_blurred'); }
});
