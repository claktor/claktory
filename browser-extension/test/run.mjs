// The Chrome extension's tests: Chrome for Testing, headless, in a throwaway
// profile, with the Dev build loaded and a stand-in helper (host.mjs) that
// hands every native-messaging frame to this script, which plays Claktory's
// helper and Mac app. Pages come from a local HTTPS server on a.test and b.test.
//
//   node browser-extension/test/run.mjs          (CHROME=… for a given Chrome)
//
// Nothing here touches a real Chrome profile, Claktory or the network.
import {execFileSync} from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import https from 'node:https';
import crypto from 'node:crypto';
import net from 'node:net';
import path from 'node:path';
import {Chrome} from './cdp.mjs';

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.dirname(here);
const DEV_ID = 'jpdkjilehjhinhohikojjngehpbakcmo';
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const only = process.argv[2] ? new RegExp(process.argv[2]) : null;

let failures = 0;
function check(name, ok, detail) {
  if (!ok) failures++;
  console.log(`${ok ? 'PASS' : 'FAIL'} ${name}${!ok && detail !== undefined ? ': ' + JSON.stringify(detail).slice(0, 600) : ''}`);
}

async function until(what, fn, ms = 5000) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    last = await fn();
    if (last) return last;
    await sleep(50);
  }
  throw new Error(`timed out waiting for ${what}: ${JSON.stringify(last)}`);
}

// ---- The run's folder: the Dev build, a certificate, the helper's manifest ----

const run = fs.mkdtempSync('/tmp/claktory-ext-');
const extension = path.join(run, 'ext');
execFileSync(path.join(root, 'build.sh'), ['--dev', extension], {stdio: 'ignore'});
execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', path.join(run, 'key.pem'), '-out', path.join(run, 'cert.pem'),
  '-days', '2', '-subj', '/CN=a.test', '-addext', 'subjectAltName=DNS:a.test,DNS:b.test'], {stdio: 'ignore'});

function serve(req, res) {
  const url = new URL(req.url, 'https://x');
  if (url.pathname === '/big.html') {
    // Far more than a result holds: 300 KB of text and 600 links.
    const words = 'lorem ipsum dolor sit amet, ünïcödé ✓ 漢字 '.repeat(20);
    const body = Array.from({length: 600}, (_, i) => `<p>${i} ${words}</p><a href="/p${i}">Link number ${i}</a>`).join('');
    res.writeHead(200, {'content-type': 'text/html; charset=utf-8'});
    return res.end(`<!doctype html><title>Big</title><body>${body}</body>`);
  }
  // Abbacchio's B1 pages (coordination/qa/b1-pages), when B1_PAGES names them.
  // Her pages load /qa.js from the root.
  if (process.env.B1_PAGES && url.pathname === '/qa.js') url.pathname = '/qa/qa.js';
  const qa = process.env.B1_PAGES && url.pathname.startsWith('/qa/');
  const folder = qa ? process.env.B1_PAGES : path.join(here, 'pages');
  const file = path.join(folder, path.normalize(qa ? url.pathname.slice(3) : url.pathname));
  if (!file.startsWith(folder) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) { res.writeHead(404); return res.end('no'); }
  res.writeHead(200, {'content-type': file.endsWith('.js') ? 'text/javascript' : 'text/html; charset=utf-8'});
  fs.createReadStream(file).pipe(res);
}
const secure = https.createServer({key: fs.readFileSync(path.join(run, 'key.pem')), cert: fs.readFileSync(path.join(run, 'cert.pem'))}, serve);
await new Promise((resolve) => secure.listen(0, '127.0.0.1', resolve));
const plain = http.createServer(serve);
await new Promise((resolve) => plain.listen(0, '127.0.0.1', resolve));
const A = `https://a.test:${secure.address().port}`;
const B = `https://b.test:${secure.address().port}`;
const PLAIN = `http://a.test:${plain.address().port}`;
const NUMBERED = `https://127.0.0.1:${secure.address().port}`;

// ---- The stand-in helper: each launch is one connection here ----

class Helper {
  constructor(socket) {
    this.socket = socket;
    this.frames = [];
    this.waiters = [];
    this.closed = new Promise((resolve) => socket.on('close', resolve));
    let lines = '';
    socket.setEncoding('utf8');
    socket.on('data', (chunk) => {
      lines += chunk;
      let i;
      while ((i = lines.indexOf('\n')) >= 0) {
        const frame = JSON.parse(lines.slice(0, i));
        lines = lines.slice(i + 1);
        this.frames.push(frame);
        for (const waiter of [...this.waiters]) waiter();
      }
    });
  }

  /** The next frame `test` likes, taken off the list. */
  next(test = () => true, ms = 5000, what = String(test)) {
    return new Promise((resolve, reject) => {
      const look = () => {
        const index = this.frames.findIndex(test);
        if (index < 0) return false;
        const [frame] = this.frames.splice(index, 1);
        this.waiters = this.waiters.filter((w) => w !== look);
        clearTimeout(timer);
        resolve(frame);
        return true;
      };
      const timer = setTimeout(() => { this.waiters = this.waiters.filter((w) => w !== look); reject(new Error(`no frame ${what} in ${ms} ms; have ${JSON.stringify(this.frames)}`)); }, ms);
      if (!look()) this.waiters.push(look);
    });
  }

  send(frame) { this.socket.write((typeof frame === 'string' ? frame : JSON.stringify(frame)) + '\n'); }

  async isClosed(ms = 3000) { return Promise.race([this.closed.then(() => true), sleep(ms).then(() => false)]); }
}

const socketPath = path.join(run, 's.sock');
const launched = [];
let onLaunch = null;
const helperServer = net.createServer((socket) => {
  const helper = new Helper(socket);
  launched.push(helper);
  if (onLaunch) onLaunch();
});
await new Promise((resolve) => helperServer.listen(socketPath, resolve));

/** The next helper Chrome starts. */
function nextHelper(ms = 5000) {
  return until('a helper', () => launched.shift(), ms).catch(() => null);
}

const profile = path.join(run, 'profile');
fs.mkdirSync(path.join(profile, 'NativeMessagingHosts'), {recursive: true});
const wrapper = path.join(run, 'host.sh');
fs.writeFileSync(wrapper, `#!/bin/sh\nexec "${process.execPath}" "${path.join(here, 'host.mjs')}" "$@"\n`, {mode: 0o755});
fs.writeFileSync(path.join(profile, 'NativeMessagingHosts', 'com.claktory.browser.dev.json'), JSON.stringify({
  name: 'com.claktory.browser.dev', description: 'Claktory browser helper (test stand-in)', path: wrapper, type: 'stdio',
  allowed_origins: [`chrome-extension://${DEV_ID}/`],
}));

// ---- Chrome ----

const chrome = await Chrome.launch({
  profile,
  args: [`--host-resolver-rules=MAP a.test 127.0.0.1, MAP b.test 127.0.0.1`, '--ignore-certificate-errors', '--window-size=1280,900'],
  env: {...process.env, CLAKTORY_TEST_SOCKET: socketPath},
});
const {id: extId} = await chrome.send('Extensions.loadUnpacked', {path: extension});
console.log('Chrome', (await chrome.send('Browser.getVersion')).product);

/** A page in a new tab: its page session and its tab target. */
async function open(url) {
  const {targetId} = await chrome.send('Target.createTarget', {url});
  const session = await chrome.attach(targetId);
  const page = {targetId, session, contexts: []};
  chrome.listeners.push((message) => {
    if (message.sessionId !== session) return;
    if (message.method === 'Runtime.executionContextCreated') page.contexts.push(message.params.context);
    if (message.method === 'Page.javascriptDialogOpening') page.dialog = message.params;
  });
  await chrome.send('Page.enable', {}, session);
  await chrome.send('Runtime.enable', {}, session);
  await until('the page to load', () => chrome.evaluate(session, () => document.readyState === 'complete').catch(() => false));
  return page;
}

async function tabOf(page) {
  const {targetInfos} = await chrome.send('Target.getTargets', {filter: [{}]});
  const info = targetInfos.find((t) => t.targetId === page.targetId);
  const tab = targetInfos.find((t) => t.type === 'tab' && t.url === info.url && !targetInfos.some((o) => o.type === 'tab' && o !== t && o.url === t.url));
  if (!tab) throw new Error('no single tab for ' + info.url);
  return tab.targetId;
}

async function navigate(page, url) {
  await chrome.send('Page.navigate', {url}, page.session);
  await until('the page to load', () => chrome.evaluate(page.session, (u) => location.href === u && document.readyState === 'complete', url).catch(() => false));
}

/** Readers this extension put into the page. */
const readers = (page) => page.contexts.filter((c) => c.auxData && c.auxData.type === 'isolated' && (c.origin.includes(extId) || c.name === 'Claktory Dev'));

let popupTarget = null;
/** The toolbar button clicked on `page`'s tab: the popup, once it shows. */
async function popup(page) {
  if (popupTarget) await chrome.send('Target.closeTarget', {targetId: popupTarget}).catch(() => {});
  // The button acts on the window's active tab: make it this one.
  await chrome.send('Target.activateTarget', {targetId: page.targetId});
  await sleep(100);
  const before = new Set((await chrome.send('Target.getTargets')).targetInfos.map((t) => t.targetId));
  // Chrome sometimes misses the first click after it starts: click again.
  let info = null;
  for (let attempt = 0; !info && attempt < 3; attempt++) {
    await chrome.send('Extensions.triggerAction', {id: extId, targetId: await tabOf(page)});
    info = await chrome.target((t) => t.url === `chrome-extension://${extId}/popup.html` && !before.has(t.targetId), 3000).catch(() => null);
  }
  if (!info) throw new Error('no popup');
  popupTarget = info.targetId;
  const session = await chrome.attach(info.targetId);
  await until('the popup', () => chrome.evaluate(session, () => document.getElementById('heading') && document.getElementById('heading').textContent).catch(() => false));
  return session;
}
const popupText = (p) => chrome.evaluate(p, () => document.getElementById('main').innerText);
const buttons = (p) => chrome.evaluate(p, () => [...document.querySelectorAll('button')].map((b) => b.dataset.action + (b.disabled ? ':off' : '')));
const press = (p, action) => chrome.evaluate(p, (a) => { const b = document.querySelector(`button[data-action="${a}"]`); if (!b || b.disabled) return false; b.click(); return true; }, action);
async function shows(p, text, ms) {
  let seen = '';
  try {
    return await until(`the popup to show "${text}"`, async () => (seen = await popupText(p)).includes(text), ms);
  } catch {
    throw new Error(`the popup didn't show "${text}"; it shows: ${seen}`);
  }
}

// B3a's schema (Giorno's crates/factoryd/src/browser/schema.rs): what the
// backend accepts, every rule of it; anything else is a failure here too.
const ERRORS = ['extension_disabled', 'no_mac', 'no_helper', 'setup_required', 'approval_pending', 'denied', 'expired', 'paused', 'wrong_device',
  'wrong_principal', 'stale_target', 'stale_document', 'origin_changed', 'unsupported_page', 'unsupported_target', 'sensitive_action', 'limit_exceeded',
  'interrupted', 'outcome_unknown'];
const MARKS = ['bidi_control', 'zero_width', 'mixed_script', 'line_break'];
function schemaProblems(result, op, limit = 65536) {
  const problems = [];
  const fail = (what) => problems.push(what);
  const only = (o, required, optional, where) => {
    if (!o || typeof o !== 'object' || Array.isArray(o)) return fail(`${where} isn't an object`);
    for (const k of required) if (!(k in o)) fail(`${where} lacks ${k}`);
    for (const k of Object.keys(o)) if (!required.includes(k) && !optional.includes(k)) fail(`${where} has ${k}`);
  };
  const control = (s) => /[\u0000-\u001F\u007F-\u009F]/.test(s);
  const text = (s, max, where) => { if (typeof s !== 'string' || s.length > max || control(s)) fail(`${where}: ${JSON.stringify(s).slice(0, 80)}`); };
  const id = (s, where) => { if (typeof s !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(s)) fail(`${where}: ${s}`); };
  const path = (s, where) => {
    if (typeof s !== 'string' || /[?#]/.test(s) || control(s)) return fail(`${where}: ${s}`);
    if (s.startsWith('/')) return;
    const m = /^https?:\/\/([^/@]+)(\/.*)$/.exec(s);
    if (!m) fail(`${where}: ${s}`);
  };
  if ('error' in result) {
    only(result, ['error'], ['why'], 'failure');
    if (!ERRORS.includes(result.error)) fail(`error ${result.error}`);
    return problems;
  }
  if (op === 'scroll') {
    only(result, ['scroll_y'], [], 'scroll');
    if (typeof result.scroll_y !== 'number') fail('scroll_y');
    return problems;
  }
  only(result, ['document', 'revision', 'title', 'path', 'text', 'truncated', 'elements'], ['title_marks', 'text_marks', 'elements_not_listed', 'unsupported', 'unsupported_page'], 'read');
  id(result.document, 'document');
  if (!Number.isInteger(result.revision) || result.revision < 0) fail('revision');
  text(result.title, 300, 'title');
  path(result.path, 'path');
  if (!result.path || !result.path.startsWith('/')) fail('path must start with /');
  if (typeof result.text !== 'string' || new TextEncoder().encode(result.text).length > limit || /[\u0000-\u0008\u000B-\u001F\u007F-\u009F]/.test(result.text)) fail('text');
  for (const key of ['title_marks', 'text_marks']) if (key in result && (!Array.isArray(result[key]) || result[key].some((m) => !MARKS.includes(m)))) fail(key);
  if (typeof result.truncated !== 'boolean') fail('truncated');
  if ('elements_not_listed' in result && !(Number.isInteger(result.elements_not_listed) && result.elements_not_listed >= 0)) fail('elements_not_listed');
  if ('unsupported' in result && (!Array.isArray(result.unsupported) || result.unsupported.length > 20 || result.unsupported.some((u) => typeof u !== 'string' || u.length > 200))) fail('unsupported');
  if ('unsupported_page' in result && result.unsupported_page !== null && typeof result.unsupported_page !== 'string') fail('unsupported_page');
  if (!Array.isArray(result.elements) || result.elements.length > 200) return [...problems, 'elements'];
  result.elements.forEach((e, i) => {
    const where = `element ${i}`;
    only(e, ['ref', 'role', 'name'], ['shows', 'marks', 'sensitive', 'leaves', 'offscreen', 'disabled', 'checked', 'expanded', 'path'], where);
    id(e.ref, `${where} ref`);
    if (typeof e.role !== 'string' || e.role.length < 1 || e.role.length > 40 || control(e.role)) fail(`${where} role ${e.role}`);
    for (const key of ['name', 'shows', 'sensitive', 'expanded']) if (key === 'name' || key in e) text(e[key], 200, `${where} ${key}`);
    if ('marks' in e && (!Array.isArray(e.marks) || e.marks.some((m) => !MARKS.includes(m)))) fail(`${where} marks`);
    if ('leaves' in e && !['other_origin', 'new_tab'].includes(e.leaves)) fail(`${where} leaves`);
    if ('checked' in e && !['true', 'false', 'mixed'].includes(e.checked)) fail(`${where} checked ${e.checked}`);
    if ('expanded' in e && !['true', 'false'].includes(e.expanded)) fail(`${where} expanded ${e.expanded}`);
    for (const key of ['offscreen', 'disabled']) if (key in e && e[key] !== true) fail(`${where} ${key}`);
    if ('path' in e) path(e.path, `${where} path`);
  });
  return problems;
}

let nextId = 1;
let nextSequence = 1;
const SESSION = 'session-1';
/** A command frame, as the helper passes it on. */
const frame = (cmd) => ({type: 'command', id: nextId++, session: SESSION, sequence: nextSequence++, cmd});

/** JSON with sorted keys and no spaces, as the helper hashes it (PROTOCOL.md). */
function canonical(value) {
  if (Array.isArray(value)) return '[' + value.map(canonical).join(',') + ']';
  if (value && typeof value === 'object') return '{' + Object.keys(value).sort().map((k) => JSON.stringify(k) + ':' + canonical(value[k])).join(',') + '}';
  return JSON.stringify(value);
}
const sha256 = (text) => crypto.createHash('sha256').update(text, 'utf8').digest('hex');
/** The hash a review of `sent` must carry. */
function reviewHash(sent, action) {
  const nav = sent.cmd.op === 'navigate';
  return sha256(canonical({action, command: sent.id, document: nav ? null : sent.cmd.document, op: sent.cmd, sequence: sent.sequence, session: sent.session, target: nav ? null : sent.cmd.target}));
}

/** An action sent: its review, once it comes (or its result, when refused at once). */
async function ask(helper, cmd) {
  const sent = frame(cmd);
  helper.send(sent);
  const answer = await helper.next((f) => (f.type === 'review' || f.type === 'result') && f.id === sent.id, 8000);
  return {sent, review: answer.type === 'review' ? answer : null, result: answer.type === 'result' ? answer.result : null};
}

/** The person's decision on a review, and the action's result. */
async function decide(helper, asked, allow) {
  helper.send({type: 'decision', id: asked.sent.id, hash: asked.review.hash, allow});
  return (await helper.next((f) => f.type === 'result' && f.id === asked.sent.id, 20000)).result;
}

/** A command through the helper, and its result. */
async function command(helper, cmd) {
  const sent = frame(cmd);
  const id = sent.id;
  helper.send(sent);
  const {result} = await helper.next((f) => f.type === 'result' && f.id === id, 8000);
  const problems = schemaProblems(result, cmd.op, cmd.limit);
  if (problems.length) check(`${cmd.op} keeps to B3a's schema`, false, problems);
  return result;
}

/** Connect from the popup, pair and bind: the helper, with the session reading. */
async function session(page, {agent = 'Ana', ms = 10 * 60_000, purpose} = {}) {
  const p = await popup(page);
  await press(p, 'start');
  const helper = await nextHelper();
  if (!helper) throw new Error('no helper started');
  await helper.next((f) => f.type === '_launched');
  await helper.next((f) => f.type === 'hello');
  helper.send({type: 'pairing', code: '482913'});
  const until_ms = Date.now() + ms;
  helper.send(purpose === undefined ? {type: 'bound', agent, until_ms} : {type: 'bound', agent, until_ms, purpose});
  await until('the reader', () => readers(page).length > 0);
  return {helper, popup: p};
}

async function test(name, fn) {
  if (only && !only.test(name)) return;
  try { await fn(); } catch (e) { check(name, false, String(e && e.stack || e)); }
}

// ---- The tests ----

await test('manifest', async () => {
  for (const file of [path.join(root, 'src', 'manifest.json'), path.join(extension, 'manifest.json')]) {
    const m = JSON.parse(fs.readFileSync(file, 'utf8'));
    const where = path.relative(root, file);
    check(`${where}: only activeTab, scripting and nativeMessaging`, JSON.stringify(m.permissions) === '["activeTab","scripting","nativeMessaging"]', m.permissions);
    check(`${where}: no host permissions, content scripts or web resources`, !m.host_permissions && !m.optional_permissions && !m.optional_host_permissions && !m.content_scripts && !m.web_accessible_resources, m);
    check(`${where}: no other extension or page may connect`, JSON.stringify(m.externally_connectable) === '{"ids":[]}', m.externally_connectable);
    check(`${where}: never in private windows`, m.incognito === 'not_allowed', m.incognito);
  }
  check('the Dev build has the Dev id', extId === DEV_ID, extId);
  const sources = fs.readdirSync(path.join(root, 'src')).filter((f) => f.endsWith('.js')).map((f) => fs.readFileSync(path.join(root, 'src', f), 'utf8')).join('\n');
  check('no eval, remote code or page-world scripts', !/\beval\(|new Function|world:\s*['"]MAIN|importScripts|https?:\/\/(?!a\.test)/.test(sources.replace(/\/\/.*$/gm, '')));
  check('no onMessageExternal or onConnectExternal', !/External/.test(sources));
});

await test('refusals', async () => {
  for (const [url, words] of [[`${PLAIN}/article.html`, 'starts with https'], [`${NUMBERED}/article.html`, 'addressed by number']]) {
    const page = await open(url);
    const p = await popup(page);
    await shows(p, words);
    check(`refused: ${url.split(':')[0]} ${words}`, !(await buttons(p)).includes('start'), await buttons(p));
    await chrome.send('Target.closeTarget', {targetId: page.targetId});
  }
  check('no helper started for a refused page', (await nextHelper(500)) === null);
});

await test('pairing and reading', async () => {
  const page = await open(`${A}/article.html`);
  const p = await popup(page);
  await shows(p, 'Let an agent read this tab');
  check('the popup names the site before connecting', (await popupText(p)).includes(A));
  check('nothing is read before Connect', readers(page).length === 0 && (await nextHelper(300)) === null);
  await press(p, 'start');
  const helper = await nextHelper();
  const launch = await helper.next((f) => f.type === '_launched');
  check('Chrome names the extension to the helper', launch.args.includes(`chrome-extension://${DEV_ID}/`), launch.args);
  const hello = await helper.next((f) => f.type === 'hello');
  const tab = hello.tab;
  check('hello', JSON.stringify(Object.keys(hello)) === '["type","v","tab","origin","title"]' && hello.v === 1 && Number.isInteger(tab) && hello.origin === A && hello.title === 'The harbour report', hello);
  await shows(p, 'Waiting for Claktory');
  const early = await command(helper, {op: 'read'});
  check('a command before the person confirms waits for them', early.error === 'approval_pending', early);
  check('nothing is in the page before the person confirms', readers(page).length === 0);
  helper.send({type: 'pairing', code: '482913'});
  await shows(p, '482 913');
  helper.send({type: 'bound', agent: 'Ana', until_ms: Date.now() + 10 * 60_000, purpose: 'Summarise the <b>harbour</b> report'});
  await until('the reader', () => readers(page).length > 0);
  await shows(p, 'Ana is reading this tab');
  const text = await popupText(p);
  check('the popup shows the agent, its purpose as text, and the time left', /Ana says\s+Summarise the <b>harbour<\/b> report/.test(text) && /Ends in\s+(9|10):\d\d/.test(text), text);
  const read = await command(helper, {op: 'read'});
  const keys = ['document', 'revision', 'title', 'title_marks', 'path', 'text', 'text_marks', 'truncated', 'elements', 'elements_not_listed', 'unsupported', 'unsupported_page'];
  check('a read has exactly the contract\'s fields', JSON.stringify(Object.keys(read)) === JSON.stringify(keys), Object.keys(read));
  check('a read has the page\'s visible text', read.text.includes('Ships came in early on Tuesday.') && read.text.includes('The end of the report.') && read.title === 'The harbour report' && read.path === '/article.html', read.text);
  const json = JSON.stringify(read);
  const leaked = ['VALUE-CANARY', 'PASSWORD-CANARY', 'HIDDEN-CANARY', 'TEXTAREA-CANARY', 'HIDDEN-TEXT-CANARY', 'OFFSCREEN-TEXT-CANARY', 'EDITABLE-CANARY', 'SECRET-QUERY-CANARY'].filter((c) => json.includes(c));
  check('no values, hidden text, editable text or query strings', leaked.length === 0, leaked);
  const by = (name) => read.elements.find((e) => e.name === name);
  check('links say their path, and when they leave the site', by('next page') && by('next page').path === '/next.html' && by('other site') && by('other site').leaves === 'other_origin', read.elements);
  check('a password field is marked sensitive', read.elements.some((e) => e.role === 'password' && e.sensitive === 'password'), read.elements);
  check('a name that differs from what shows says both', by('Cancel') && by('Cancel').shows === 'Delete forever', by('Cancel'));
  check('refs are opaque', read.elements.every((e) => /^e[0-9a-f]{24}$/.test(e.ref)) && /^d[0-9a-f]{24}$/.test(read.document), read.document);
  const scrolled = await command(helper, {op: 'scroll', direction: 'down', pages: 1});
  check('scroll says where it is', JSON.stringify(Object.keys(scrolled)) === '["scroll_y"]' && scrolled.scroll_y > 0, scrolled);
  const small = await command(helper, {op: 'read', limit: 20});
  check('a read keeps to its limit', new TextEncoder().encode(small.text).length <= 20 && small.truncated === true, small.text);

  // Another page of the same site: a new document, read afresh.
  await navigate(page, `${A}/next.html`);
  const stale = await command(helper, {op: 'scroll', direction: 'down', pages: 1});
  check('after the page changes, only a read goes on', stale.error === 'stale_document', stale);
  const again = await command(helper, {op: 'read'});
  check('a read on the new page has a new document', again.title === 'Next page' && again.document !== read.document, again);
  check('the same site keeps the session', !(await helper.isClosed(200)));

  // Another site: the session ends at once, without waiting for a command.
  await navigate(page, `${B}/other.html`);
  const ended = await helper.next((f) => f.type === 'ended');
  check('going to another site ends it', JSON.stringify(ended) === JSON.stringify({type: 'ended', tab, reason: 'origin_changed'}), ended);
  check('and the helper is let go', await helper.isClosed());
  await navigate(page, `${A}/article.html`);
  const back = await popup(page);
  await shows(back, 'The tab went to another site');
  check('coming back needs Connect again', (await buttons(back)).includes('start') && (await nextHelper(300)) === null, await buttons(back));
  await chrome.send('Target.closeTarget', {targetId: page.targetId});
});

await test('pausing', async () => {
  // The other tab first: a tab opened from the session's one would pause it
  // as a window the page opened.
  const other = await open(`${A}/next.html?pause`);
  const page = await open(`${A}/article.html?pause`);
  await chrome.send('Target.activateTarget', {targetId: page.targetId});
  const {helper, popup: p} = await session(page);
  await chrome.send('Target.activateTarget', {targetId: other.targetId});
  const paused = await helper.next((f) => f.type === 'paused');
  check('another tab in front pauses it', paused.reason === 'tab_inactive', paused);
  const refused = await command(helper, {op: 'read'});
  check('a paused session reads nothing', refused.error === 'paused', refused);
  await chrome.send('Target.activateTarget', {targetId: page.targetId});
  const q = await popup(page);
  await shows(q, 'Paused');
  check('the popup offers Resume', (await buttons(q)).includes('resume'), await buttons(q));
  await press(q, 'resume');
  await helper.next((f) => f.type === 'resumed');
  const read = await command(helper, {op: 'read'});
  check('after Resume it reads again', read.title === 'The harbour report', read);

  // A page that opens a window pauses it too.
  await chrome.send('Runtime.evaluate', {expression: 'window.open("/next.html?opened")', userGesture: true}, page.session);
  const opened = await helper.next((f) => f.type === 'paused');
  check('a window the page opens pauses it', opened.reason === 'popup_opened', opened);
  await chrome.send('Target.activateTarget', {targetId: page.targetId});
  const r = await popup(page);
  await press(r, 'resume');
  await helper.next((f) => f.type === 'resumed');

  // A dialog holds the page: the command is bounded, and the session pauses.
  chrome.send('Runtime.evaluate', {expression: 'setTimeout(() => alert("hi"), 0)'}, page.session).catch(() => {});
  await until('the dialog', () => page.dialog);
  const held = await command(helper, {op: 'read'});
  check('a dialog pauses it', held.error === 'paused', held);
  const dialog = await helper.next((f) => f.type === 'paused');
  check('and says why', dialog.reason === 'dialog_open', dialog);
  await chrome.send('Page.handleJavaScriptDialog', {accept: false}, page.session);
  void p;
  await chrome.send('Target.closeTarget', {targetId: other.targetId});
  await chrome.send('Target.closeTarget', {targetId: page.targetId});
  const closed = await helper.next((f) => f.type === 'ended');
  check('closing the tab ends it', closed.reason === 'tab_closed', closed);
});

await test('stopping', async () => {
  const page = await open(`${A}/article.html?stop`);
  const {helper, popup: p} = await session(page);
  await press(p, 'stop');
  const ended = await helper.next((f) => f.type === 'ended');
  check('Stop in the popup ends it and tells Claktory', ended.reason === 'stopped', ended);
  check('and lets the helper go', await helper.isClosed());
  await shows(p, 'You stopped it.');
  check('then offers to close the tab', (await buttons(p)).includes('close_tab'), await buttons(p));
  await press(p, 'close_tab');
  await until('the tab to close', async () => !(await chrome.send('Target.getTargets')).targetInfos.some((t) => t.targetId === page.targetId));
  check('Close this tab closes it', true);

  // Claktory stops it: the worker ends it without answering.
  const second = await open(`${A}/article.html?stop2`);
  const s2 = await session(second);
  s2.helper.send({type: 'stop'});
  const q = await popup(second);
  await shows(q, 'It was stopped in Claktory.');
  check('a stop from Claktory gets no reply', !s2.helper.frames.some((f) => f.type === 'ended'), s2.helper.frames);
  await chrome.send('Target.closeTarget', {targetId: second.targetId});
});

await test('strict frames', async () => {
  const cases = [
    ['an unknown field', (h) => h.send({...frame({op: 'read'}), extra: 1})],
    ['an id that doesn\'t increase', async (h) => { await command(h, {op: 'scroll', direction: 'up', pages: 1}); h.send({...frame({op: 'read'}), id: 1}); }],
    ['a sequence that doesn\'t increase', async (h) => { await command(h, {op: 'scroll', direction: 'up', pages: 1}); h.send({...frame({op: 'read'}), sequence: 1}); }],
    ['another session', async (h) => { await command(h, {op: 'scroll', direction: 'up', pages: 1}); h.send({...frame({op: 'read'}), session: 'session-2'}); }],
    ['no session', (h) => { const f = frame({op: 'read'}); delete f.session; h.send(f); }],
    ['an op it doesn\'t have', (h) => h.send(frame({op: 'type', target: 'x'}))],
    ['a click without its document', (h) => h.send(frame({op: 'click', target: 'x'}))],
    ['a fill with a control character', (h) => h.send(frame({op: 'fill', document: 'd', revision: 0, target: 'x', text: 'a\u0007b'}))],
    ['a navigation to another site', (h) => h.send(frame({op: 'navigate', url: `${B}/other.html`}))],
    ['a navigation with a query', (h) => h.send(frame({op: 'navigate', url: `${A}/next.html?x=1`}))],
    ['a decision with no review', (h) => h.send({type: 'decision', id: nextId - 1, hash: '0'.repeat(64), allow: true})],
    ['a limit too big', (h) => h.send(frame({op: 'read', limit: 70000}))],
    ['a second bound', (h) => h.send({type: 'bound', agent: 'Bo', until_ms: Date.now() + 60_000})],
    ['an unknown type', (h) => h.send({type: 'grant'})],
    ['not an object', (h) => h.send('[1,2]')],
  ];
  for (const [what, act] of cases) {
    const page = await open(`${A}/article.html?strict`);
    const {helper} = await session(page);
    await act(helper);
    const ended = await helper.next((f) => f.type === 'ended');
    check(`${what} ends it as helper_refused`, ended.reason === 'helper_refused', ended);
    await chrome.send('Target.closeTarget', {targetId: page.targetId});
  }
  // A bad pairing code, before anything is bound.
  const page = await open(`${A}/article.html?code`);
  const p = await popup(page);
  await press(p, 'start');
  const helper = await nextHelper();
  await helper.next((f) => f.type === 'hello');
  helper.send({type: 'pairing', code: '12345'});
  const ended = await helper.next((f) => f.type === 'ended');
  check('a code that isn\'t 6 digits ends it', ended.reason === 'helper_refused', ended);
  // The helper's own ending: the popup says what to do.
  const q = await popup(page);
  await press(q, 'start');
  const h2 = await nextHelper();
  await h2.next((f) => f.type === 'hello');
  h2.send({type: 'ended', reason: 'no_app'});
  await shows(q, 'Open Claktory on this Mac');
  await chrome.send('Target.closeTarget', {targetId: page.targetId});
});

await test('limits', async () => {
  const page = await open(`${A}/next.html?limits`);
  const {helper} = await session(page, {ms: 1500});
  const ended = await helper.next((f) => f.type === 'ended', 5000);
  check('the session ends when its time is up', ended.reason === 'expired', ended);
  const {helper: h2} = await session(page);
  let last;
  for (let i = 0; i < 100; i++) last = await command(h2, {op: 'scroll', direction: 'down', pages: 1});
  check('100 commands run', last.scroll_y !== undefined, last);
  const over = await command(h2, {op: 'scroll', direction: 'up', pages: 1});
  check('the 101st is refused', over.error === 'limit_exceeded', over);
  const done = await h2.next((f) => f.type === 'ended');
  check('and the session ends', done.reason === 'limit_exceeded', done);
  await chrome.send('Target.closeTarget', {targetId: page.targetId});
});

await test('big and hostile pages', async () => {
  const big = await open(`${A}/big.html`);
  const {helper} = await session(big);
  const read = await command(helper, {op: 'read'});
  const size = new TextEncoder().encode(JSON.stringify(read)).length;
  check('a big page fits in 64 KiB', size <= 65536 && read.truncated === true && read.elements.length <= 200 && read.elements_not_listed > 0, {size, elements: read.elements.length, not: read.elements_not_listed});
  check('and keeps text as well as elements', read.text.length > 10000, read.text.length);
  await chrome.send('Target.closeTarget', {targetId: big.targetId});
  await helper.next((f) => f.type === 'ended');

  const hostile = await open(`${A}/hostile.html`);
  const s = await session(hostile);
  await sleep(500);
  const heard = s.helper.frames.filter((f) => f.type !== '_launched' && f.type !== 'hello');
  check('nothing the page posts reaches the helper', heard.length === 0, heard);
  const page = await command(s.helper, {op: 'read'});
  check('bidi and zero-width characters come escaped and marked', page.title.includes('⟦U+202E⟧') && page.title_marks.includes('bidi_control') && page.text.includes('⟦U+200B⟧') && page.text_marks.includes('zero_width') && page.text_marks.includes('mixed_script'), {title: page.title, marks: page.text_marks});
  const approve = page.elements.find((e) => e.role === 'button');
  check('a hostile label is escaped and marked', approve && approve.name.includes('⟦U+202E⟧') && approve.marks.includes('bidi_control'), approve);
  check('white-on-white text is fenced', page.text.includes('⟦same colour as its background:⟧'), page.text);
  await navigate(hostile, `${A}/edge.html`);
  const edge = await command(s.helper, {op: 'read'});
  check('control characters come escaped', edge.text.includes('bell⟦U+0007⟧here') && edge.text.includes('C1⟦U+0085⟧there'), edge.text.slice(0, 120));
  const named = (n) => edge.elements.find((e) => e.name.startsWith(n));
  const long = named('Go Go');
  check('a long name is cut to 200 with its marker', long && long.name.length <= 200 && /…\[cut \d+ characters\]$/.test(long.name), long && long.name.length);
  check('only web links have a path', named('Write to us') && !('path' in named('Write to us')) && named('Do nothing') && !('path' in named('Do nothing')), edge.elements);
  check('a link\'s path leaves out its name and password', named('Signed-in link') && named('Signed-in link').path === 'https://b.test/in', named('Signed-in link'));
  check('odd states are left out, and a half-checked box is mixed', !('checked' in named('Odd checkbox')) && !('expanded' in named('Odd expander')) && named('Some of them').checked === 'mixed', edge.elements);
  check('typed text in an editable region stays out', !JSON.stringify(edge).includes('Typed by someone') && !JSON.stringify(edge).includes('SECRET-QUERY-CANARY'), edge.elements);
  // Another extension can't reach this one.
  const probe = path.join(run, 'probe');
  fs.mkdirSync(probe);
  fs.writeFileSync(path.join(probe, 'manifest.json'), JSON.stringify({manifest_version: 3, name: 'probe', version: '1', background: {service_worker: 'sw.js'}}));
  fs.writeFileSync(path.join(probe, 'sw.js'), '');
  const {id} = await chrome.send('Extensions.loadUnpacked', {path: probe});
  const worker = await chrome.target((t) => t.type === 'service_worker' && t.url.startsWith(`chrome-extension://${id}/`));
  const ws = await chrome.attach(worker.targetId);
  const answer = await chrome.evaluate(ws, async (target) => {
    const out = [];
    for (const m of [{kind: 'stop'}, {kind: 'view', tabId: 1}, {type: 'command', id: 1000, cmd: {op: 'read'}}]) {
      try { out.push(await chrome.runtime.sendMessage(target, m)); } catch (e) { out.push(String(e.message)); }
    }
    return out;
  }, extId);
  check('another extension gets no answer', answer.every((a) => typeof a === 'string' && /Receiving end does not exist|Could not establish connection/.test(a)), answer);
  const still = await command(s.helper, {op: 'scroll', direction: 'down', pages: 1});
  check('and the session goes on', still.scroll_y !== undefined, still);
  await chrome.send('Target.closeTarget', {targetId: hostile.targetId});
});

await test('actions', async () => {
  // Secco's vector (crates/claktory-browser-host/src/canonical.rs): this
  // test's hash is the helper's.
  const vector = canonical({
    action: {op: 'fill', origin: 'https://example.com', path: '/compose', role: 'textbox', name: 'Message', text: 'Hello⏎there', warnings: ['may_send_as_typed']},
    command: 7, document: 'D1', op: {op: 'fill', document: 'D1', revision: 3, target: 'r-9f2c', text: 'Hello\nthere'}, sequence: 12, session: 's-1', target: 'r-9f2c',
  });
  check('the hash matches the helper\'s test vector', sha256(vector) === 'd86d019b1243ac926d61234d61c8ce8ebaaffbfbe9286c6a6566c38ab32fc098', sha256(vector));

  const page = await open(`${A}/actions.html`);
  const effects = () => chrome.evaluate(page.session, () => window.effects);
  const {helper} = await session(page);
  let read = await command(helper, {op: 'read'});
  const el = (name) => read.elements.find((e) => e.name === name) || read.elements.find((e) => e.name.startsWith(name));
  const target = (name, extra = {}) => ({op: 'click', document: read.document, revision: read.revision, target: el(name).ref, ...extra});

  // Allowed: reviewed, then done once.
  const save = await ask(helper, target('Save'));
  check('a click waits for the person\'s review', save.review && !save.result && (await effects()).length === 0, save);
  const a = save.review.action;
  check('the review says what it is, in data only', JSON.stringify(Object.keys(save.review)) === '["type","tab","id","hash","expires_ms","action"]'
    && a.op === 'click' && a.origin === A && a.path === '/actions.html' && a.role === 'button' && a.name === 'Save' && JSON.stringify(a.warnings) === '["may_ask_permission"]'
    && Math.abs(save.review.expires_ms - Date.now() - 120_000) < 5000, save.review);
  check('the review\'s hash binds the action to the command', save.review.hash === reviewHash(save.sent, a), save.review.hash);
  const p = await popup(page);
  await shows(p, 'Choose Allow once or Don\'t allow in Claktory on your Mac');
  const done = await decide(helper, save, true);
  check('allowed, it\'s done once', JSON.stringify(done) === '{"done":true}' && JSON.stringify(await effects()) === '["saved"]', {done, effects: await effects()});

  // Denied: nothing happens.
  const again = await ask(helper, target('Save'));
  const denied = await decide(helper, again, false);
  check('denied, nothing happens', denied.error === 'denied' && (await effects()).length === 1, denied);

  // A fill: the exact text shown, typed, read back.
  const fill = await ask(helper, {...target('Name'), op: 'fill', text: 'Ada\tL⟦x⟧\u202E'});
  check('a fill shows the exact text, escaped', fill.review && fill.review.action.text === 'Ada⟦U+0009⟧L⟦U+27E6⟧x⟧⟦U+202E⟧' && fill.review.action.warnings.includes('may_send_as_typed'), fill.review && fill.review.action);
  const filled = await decide(helper, fill, true);
  check('a fill reads back what it typed', filled.done === true && filled.took === true, filled);
  const notes = await ask(helper, {...target('Notes'), op: 'fill', text: 'one\ntwo'});
  check('a text area takes line breaks', notes.review && notes.review.action.text === 'one⏎two', notes);
  check('and it\'s typed as it was shown', (await decide(helper, notes, true)).took === true && await chrome.evaluate(page.session, () => document.getElementById('notes').value) === 'one\ntwo');

  // Refused at once, with no review.
  for (const [what, cmd, error] of [
    ['a password field', {...target('Password'), op: 'fill', text: 'x'}, 'sensitive_action'],
    ['a line break in a one-line field', {...target('Name'), op: 'fill', text: 'a\nb'}, 'unsupported_target'],
    ['a list', {...target('Colour'), op: 'fill', text: 'Blue'}, 'unsupported_target'],
    ['clicking a text field', target('Name'), 'unsupported_target'],
    ['a link to another app', target('Write to us'), 'sensitive_action'],
    ['a window from an inline handler', target('More'), 'sensitive_action'],
    ['Turn on notifications', target('Turn on notifications'), 'sensitive_action'],
    ['Use my location', target('Use my location'), 'sensitive_action'],
    ['Scan a QR code', target('Scan a QR code'), 'sensitive_action'],
    ['Paste from clipboard', target('Paste from clipboard'), 'sensitive_action'],
    ['Open help window', target('Open help window'), 'sensitive_action'],
    ['an unknown target', {...target('Save'), target: 'e000000000000000000000000'}, 'stale_target'],
    ['an earlier document', {...target('Save'), document: 'd000000000000000000000000'}, 'stale_document'],
  ]) {
    const refused = await ask(helper, cmd);
    check(`refused at once: ${what}`, !refused.review && refused.result && refused.result.error === error, refused.result);
  }
  check('none of them reached the page', JSON.stringify(await effects()) === JSON.stringify(['saved', 'typed:Ada\tL⟦x⟧\u202E']), await effects());

  // The target changes while the person reviews it: nothing is done.
  const changing = await ask(helper, target('Keep project'));
  await chrome.evaluate(page.session, () => { document.getElementById('changing').textContent = 'Delete project'; });
  const stale = await decide(helper, changing, true);
  check('a target that changed during the review isn\'t clicked', stale.error === 'stale_target' && !(await effects()).includes('kept'), stale);
  read = await command(helper, {op: 'read'});

  // The page holds on a dialog: sent, never retried.
  const confirmer = await ask(helper, target('Ask first'));
  const unknown = await decide(helper, confirmer, true);
  check('a click the page doesn\'t answer is outcome_unknown', unknown.error === 'outcome_unknown', unknown);
  await until('the dialog', () => page.dialog);
  await chrome.send('Page.handleJavaScriptDialog', {accept: false}, page.session);
  page.dialog = null;
  const resumed = await popup(page);
  if ((await buttons(resumed)).includes('resume')) { await press(resumed, 'resume'); await helper.next((f) => f.type === 'resumed'); }
  read = await command(helper, {op: 'read'});

  // A wrong hash in a decision ends the session.
  const wrong = await ask(helper, target('Save'));
  helper.send({type: 'decision', id: wrong.sent.id, hash: '0'.repeat(64), allow: true});
  const ended = await helper.next((f) => f.type === 'ended');
  check('a decision with another hash ends it, and nothing is done', ended.reason === 'helper_refused' && (await effects()).filter((e) => e === 'saved').length === 1, ended);
  await chrome.send('Target.closeTarget', {targetId: page.targetId});
});

await test('reviews that end', async () => {
  // The other tab first: one opened from the session's tab would pause it
  // as a window the page opened.
  const other = await open(`${A}/next.html?end`);
  const page = await open(`${A}/actions.html?end`);
  const effects = () => chrome.evaluate(page.session, () => window.effects);
  let {helper} = await session(page);
  let read = await command(helper, {op: 'read'});
  const save = () => ({op: 'click', document: read.document, revision: read.revision, target: read.elements.find((e) => e.name === 'Save').ref});

  // Another tab during the review pauses it, and nothing is done.
  const one = await ask(helper, save());
  await chrome.send('Target.activateTarget', {targetId: other.targetId});
  const paused = await helper.next((f) => f.type === 'result' && f.id === one.sent.id);
  check('another tab during a review: paused, not done', paused.result.error === 'paused' && (await effects()).length === 0, paused.result);
  await chrome.send('Target.closeTarget', {targetId: other.targetId});
  await chrome.send('Target.activateTarget', {targetId: page.targetId});
  const p = await popup(page);
  await press(p, 'resume');
  await helper.next((f) => f.type === 'resumed');

  // Stop during a review: it ends, and nothing is done.
  const two = await ask(helper, save());
  const q = await popup(page);
  await press(q, 'stop');
  const ended = await helper.next((f) => f.type === 'ended');
  check('Stop during a review ends it, not done', ended.reason === 'stopped' && (await effects()).length === 0 && !helper.frames.some((f) => f.type === 'result' && f.id === two.sent.id), ended);

  // A navigation, reviewed, to another page of the site.
  ({helper} = await session(page));
  read = await command(helper, {op: 'read'});
  const go = await ask(helper, {op: 'navigate', url: `${A}/next.html`});
  check('a navigation is reviewed with where it goes', go.review && go.review.action.op === 'navigate' && go.review.action.destination === '/next.html' && go.review.hash === reviewHash(go.sent, go.review.action), go.review);
  const went = await decide(helper, go, true);
  check('allowed, it goes and says where', went.done === true && went.path === '/next.html' && /^d[0-9a-f]{24}$/.test(went.document) && went.document !== read.document, went);
  const after = await command(helper, {op: 'read'});
  check('and the new page reads', after.title === 'Next page' && after.document === went.document, after.title);
  await chrome.send('Target.closeTarget', {targetId: page.targetId});
  await helper.next((f) => f.type === 'ended');
});

// Two minutes long, so only with SLOW=1.
await test('a review nobody answers', async () => {
  if (!process.env.SLOW) return;
  const page = await open(`${A}/actions.html?slow`);
  const {helper} = await session(page);
  const read = await command(helper, {op: 'read'});
  const asked = await ask(helper, {op: 'click', document: read.document, revision: read.revision, target: read.elements.find((e) => e.name === 'Save').ref});
  const lapsed = await helper.next((f) => f.type === 'result' && f.id === asked.sent.id, 130_000);
  check('a review with no decision in 2 minutes expires, not done', lapsed.result.error === 'expired' && (await chrome.evaluate(page.session, () => window.effects)).length === 0, lapsed.result);
  helper.send({type: 'decision', id: asked.sent.id, hash: asked.review.hash, allow: true});
  const late = await helper.next((f) => f.type === 'ended');
  check('and a decision after it is refused', late.reason === 'helper_refused', late);
  await chrome.send('Target.closeTarget', {targetId: page.targetId});
});

// Every one of Abbacchio's B1 pages read once: within the schema, and none
// of what they hide in values, closed shadow roots or frames.
await test('b1 pages', async () => {
  if (!process.env.B1_PAGES) return;
  const hidden = ['CANARY-PASSWORD', 'CANARY-PW2', 'SECRET-QUERY-CANARY', 'CLOSED-SHELL-TEXT', 'CROSS-ORIGIN-FRAME-TEXT', 'SAME-ORIGIN-FRAME-TEXT', 'SRCDOC-TEXT'];
  const pages = fs.readdirSync(process.env.B1_PAGES).filter((f) => f.endsWith('.html') && !f.startsWith('frame-'));
  for (const name of pages) {
    const page = await open(`${A}/qa/${name}`);
    const {helper} = await session(page);
    const read = await command(helper, {op: 'read'});
    const json = JSON.stringify(read);
    const leaked = hidden.filter((c) => json.includes(c));
    check(`${name}: read within the schema, nothing hidden`, !read.error && leaked.length === 0, read.error ? read : leaked);
    if (name === 'frames.html') check('frames.html: the top frame only', read.text.includes('TOP-FRAME-TEXT') && read.unsupported.some((u) => /frames not read/.test(u)), read.unsupported);
    if (name === 'canvas.html') check('canvas.html: a canvas page is unsupported', typeof read.unsupported_page === 'string', read.unsupported_page);
    if (name === 'components.html') check('components.html: shadow roots counted', read.unsupported.some((u) => /closed shadow roots/.test(u)), read.unsupported);
    if (name === 'sensitive.html') {
      // Abbacchio's B1 audit: these reached the page in the spike.
      for (const label of ['Turn on notifications', 'Use my location', 'Scan a QR code', 'Paste from clipboard', 'Open help window', 'Attach a file', 'Upload a photo', 'Download report', 'Pay $49.00', 'Delete account']) {
        const el = read.elements.find((e) => e.name === label || e.name.startsWith(label));
        if (!el) { check(`sensitive.html: ${label} is listed`, false, read.elements.map((e) => e.name)); continue; }
        const tried = await ask(helper, {op: 'click', document: read.document, revision: read.revision, target: el.ref});
        check(`sensitive.html: ${label} is refused before any review`, !tried.review && tried.result && tried.result.error === 'sensitive_action', tried.result || tried.review);
      }
      const effects = await chrome.evaluate(page.session, () => window.__qa.effects());
      check('sensitive.html: nothing reached the page', effects.length === 0, effects);
    }
    await chrome.send('Target.closeTarget', {targetId: page.targetId});
    await helper.next((f) => f.type === 'ended');
  }
});

await chrome.close();
secure.close();
plain.close();
helperServer.close();
fs.rmSync(run, {recursive: true, force: true});
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);
