// Claktory's reader: a content script in Chrome's isolated world, in the top
// frame only, put there by the service worker once the person confirmed a
// session. Fixed operations (read and scroll; and describe, watch and act for
// a click or fill the person reviews), taken only from this extension's own
// worker; never from the page, its messages, events or attributes.
(() => {
  if (globalThis.__claktoryReader) return;
  globalThis.__claktoryReader = true;

  const ELEMENTS_LIMIT = 200;
  const NAME_LIMIT = 200;
  const PATH_LIMIT = 300;
  const TIME_LIMIT_MS = 1500; // a page too big to read in this is read in part
  let revision = 0; // counts the document's changes
  let refs = new Map(); // opaque ref -> {el: WeakRef, fp, dirty}: kept until the next read
  new MutationObserver(() => { revision++; }).observe(document, {subtree: true, childList: true, attributes: true, characterData: true});

  const INTERACTIVE = 'a[href],button,input,select,textarea,summary,label,[role=button],[role=link],[role=checkbox],[role=switch],[role=radio],[role=tab],[role=menuitem],[role=option],[role=combobox],[role=textbox],[contenteditable=""],[contenteditable=true]';

  // Characters that change how text reads without showing: bidi controls and
  // isolates, zero-width characters. Returned escaped (⟦U+202E⟧), so nothing
  // downstream can render them.
  const BIDI = /[\u061C\u200E\u200F\u202A-\u202E\u2066-\u2069]/g;
  // Zero-width and invisible characters (U+2061–2064 are invisible operators, U+2065 is held for more).
  const ZERO_WIDTH = /[\u200B-\u200D\u2060-\u2065\uFEFF]/g;
  const hex = (c) => '⟦U+' + c.codePointAt(0).toString(16).toUpperCase().padStart(4, '0') + '⟧';
  // A word mixing Latin with Cyrillic or Greek letters (clаktory with a Cyrillic а).
  const MIXED = /(?=\S*\p{Script=Latin})(?=\S*[\p{Script=Cyrillic}\p{Script=Greek}])\S+/u;
  function marks(s) {
    const found = [];
    if (BIDI.test(s)) found.push('bidi_control');
    if (ZERO_WIDTH.test(s)) found.push('zero_width');
    BIDI.lastIndex = 0; ZERO_WIDTH.lastIndex = 0;
    if (MIXED.test(s)) found.push('mixed_script');
    if (/[\r\n\u2028\u2029]/.test(s)) found.push('line_break');
    return found;
  }
  // Control characters other than line breaks and tabs: escaped the same way.
  const CONTROL = /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F]/g;
  // A lone surrogate can't travel as UTF-8: it becomes U+FFFD.
  const LONE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g;
  const shown = (s) => s.replace(LONE, '\uFFFD').replace(BIDI, hex).replace(ZERO_WIDTH, hex).replace(CONTROL, hex);
  // One line of at most `limit` characters, the marker where it was cut
  // ("…[cut 12 characters]") included.
  function oneLine(s, limit) {
    s = shown(s.trim().replace(/\r\n|\r|\n|\u2028|\u2029/g, '⏎').replace(/[ \t\f\v]+/g, ' '));
    if (s.length <= limit) return s;
    let keep = limit;
    let marker = '';
    for (let i = 0; i < 3; i++) {
      marker = '…[cut ' + (s.length - keep) + ' characters]';
      keep = Math.max(0, limit - marker.length);
    }
    if (keep > 0 && /[\uD800-\uDBFF]/.test(s[keep - 1])) keep--;
    return s.slice(0, keep) + '…[cut ' + (s.length - keep) + ' characters]';
  }

  const encoder = new TextEncoder();
  const utf8 = (s) => encoder.encode(s).length;
  /** Bytes `s` takes inside a JSON string. */
  const jsonBytes = (s) => utf8(JSON.stringify(s)) - 2;

  function inViewport(r) { return r.bottom > 0 && r.right > 0 && r.top < innerHeight && r.left < innerWidth; }

  function visible(el) {
    if (!(el instanceof Element)) return false;
    if (el.closest('[aria-hidden=true],[hidden],[inert]')) return false;
    if (el.checkVisibility && !el.checkVisibility({checkOpacity: true, checkVisibilityCSS: true})) return false;
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) return false;
    // Pushed off the page (left: -9999px and the like): not visible.
    const left = r.right + scrollX, top = r.bottom + scrollY;
    return left > 0 && top > 0 && r.left + scrollX < document.documentElement.scrollWidth;
  }

  function fileLabel(el) {
    return el.tagName === 'LABEL' && el.control instanceof HTMLInputElement && el.control.type === 'file';
  }

  function role(el) {
    const explicit = el.getAttribute('role');
    if (explicit && explicit.trim()) return oneLine(explicit, 40);
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'button' || tag === 'summary') return 'button';
    if (tag === 'select') return 'combobox';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'label') return fileLabel(el) ? 'file' : 'label';
    if (tag === 'input') {
      const type = (el.getAttribute('type') || 'text').toLowerCase();
      return {checkbox: 'checkbox', radio: 'radio', submit: 'button', button: 'button', reset: 'button', password: 'password', file: 'file', hidden: 'hidden', number: 'spinbutton', date: 'date', email: 'textbox'}[type] || 'textbox';
    }
    if (el.isContentEditable) return 'textbox';
    return oneLine(tag, 40);
  }

  const textOf = (el) => (el.innerText || el.textContent || '');
  /** A field or an editable region: what's in it is a value. */
  const editable = (el) => ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName) || el.isContentEditable;

  // A name from labels and visible text, never from values or arbitrary
  // attributes: aria-labelledby, aria-label, <label>, the element's own text,
  // title, placeholder.
  function rawName(el) {
    const by = el.getAttribute('aria-labelledby');
    if (by) {
      const parts = by.split(/\s+/).map((id) => document.getElementById(id)).filter((e) => e && !withheldWhy(e)).map(textOf);
      if (parts.join('').trim()) return parts.join(' ');
    }
    const aria = el.getAttribute('aria-label');
    if (aria && aria.trim()) return aria;
    if (el.labels && el.labels.length) return Array.from(el.labels).filter((e) => !withheldWhy(e)).map(textOf).join(' ');
    // An editable region's text is what someone typed: a value, never a name.
    if (!editable(el) && textOf(el).trim()) return textOf(el);
    if (el.getAttribute('title')) return el.getAttribute('title');
    if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') return el.getAttribute('placeholder') || '';
    return '';
  }
  const name = (el) => oneLine(rawName(el), NAME_LIMIT);

  // What the element shows, when its accessible name says something else
  // (aria-label "Cancel" on a button that reads "Delete forever").
  function shownText(el) {
    if (editable(el)) return null;
    const t = oneLine(textOf(el), NAME_LIMIT);
    return t && t !== name(el) ? t : null;
  }

  function fingerprint(el) { return role(el) + '|' + name(el) + '|' + (el.getAttribute('type') || ''); }

  function ref() {
    const bytes = crypto.getRandomValues(new Uint8Array(12));
    return 'e' + Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
  }

  // Off-limits targets (browser-control.md §8), marked so they're never
  // offered: a heuristic over labels, which catches the common ones, never all.
  const SENSITIVE_AUTOCOMPLETE = /cc-|one-time-code|current-password|new-password/;
  const SENSITIVE_FIELD = /password|passcode|passkey|one-time|verification code|security code|\bcvc\b|\bcvv\b|card number|\biban\b|account number|routing|\bpin\b/i;
  const SENSITIVE_ACTION = /\bpay\b|payment|purchase|\bbuy\b|checkout|place order|confirm order|delete account|close account|transfer|send money/i;
  function sensitive(el) {
    const kept = withheldWhy(el);
    if (kept) return kept;
    if (fileLabel(el)) return 'file picker';
    // A download link is the person's, except in an errand (#435 R9),
    // whose reader sets __claktoryDownloads in its own isolated world: a
    // page's script can't reach that world's globalThis, so can't set it.
    if (el.tagName === 'A' && el.hasAttribute('download') && !globalThis.__claktoryDownloads) return 'download';
    if (el instanceof HTMLInputElement) {
      const type = (el.type || 'text').toLowerCase();
      if (['password', 'hidden', 'file'].includes(type)) return type;
      if (SENSITIVE_AUTOCOMPLETE.test(el.autocomplete || '')) return oneLine(el.autocomplete, 40);
      const security = getComputedStyle(el).webkitTextSecurity;
      if (security && security !== 'none') return 'masked like a password';
      const group = el.closest('[role=group],fieldset');
      if (el.maxLength === 1 && group && /code|digit|pin/i.test(rawName(group) + (group.getAttribute('aria-label') || ''))) return 'one digit of a code';
    }
    const words = rawName(el) + ' ' + (shownText(el) || '') + ' ' + (el.getAttribute('name') || '');
    if (['INPUT', 'TEXTAREA'].includes(el.tagName) || el.isContentEditable) {
      if (SENSITIVE_FIELD.test(words)) return 'looks like a credential or payment field';
    } else if (SENSITIVE_ACTION.test(words)) return 'looks like a payment or account change';
    return null;
  }

  // Recovery codes a site shows once (#435 W8b, Fugo 40691): a block whose
  // words are mostly code-shaped (4 or more), on a page about recovery or
  // backup codes, is withheld whole: no text of it is read, and it's offered
  // only as a ref, for errand_handoff kind recovery_codes, which Claktory
  // reads itself. An authenticator's setup key (Fugo 42966) is withheld the
  // same way, and never offered: an otpauth:// address anywhere, or a
  // base32 run of 16 characters or more on a page about a setup key. Both
  // are heuristics that err toward withholding. Once withheld, a block stays
  // withheld in every later read of the page, with anything inside it
  // off-limits and not offered.
  const CODE_WORD = /^(?=\S*\d)[A-Za-z0-9]{3,12}(?:-[A-Za-z0-9]{3,12})?$/;
  const CODES_ABOUT = /recovery|backup|codes\b/i;
  const CODES_WHY = "recovery codes: the person's (errand_handoff, kind recovery_codes)";
  // A base32 key: 16 characters or more, one case, in groups (spaces or
  // hyphens between them allowed), standing alone. Anywhere, with at least
  // two of the digits 2-7, as no word has. On a page about a setup key
  // (`about`), with none needed (Secco 43242: random base32 has fewer than
  // two in 16 characters one time in six) when it's one run with no
  // separators, as no word is, or when one of its groups has no vowel, as
  // no word has; else one digit (so a sentence of four-letter words isn't
  // taken for a key).
  const KEY_RUN = /(?<![A-Za-z0-9])(?:[A-Za-z2-7]{4}[ -]?){4,}[A-Za-z2-7]*(?![A-Za-z0-9])/g;
  const hasKey = (text, about) => /otpauth:\/\//i.test(text) || (text.replace(/\s+/g, ' ').match(KEY_RUN) || []).some((run) => {
    const groups = run.trim().split(/[ -]/).filter(Boolean);
    const bare = groups.join('');
    if (bare.length < 16 || (bare !== bare.toUpperCase() && bare !== bare.toLowerCase())) return false;
    const digits = (bare.match(/[2-7]/g) || []).length;
    if (digits >= 2) return true;
    if (!about) return false;
    return groups.length === 1 || digits >= 1 || groups.some((g) => !/[aeiouy]/i.test(g));
  });
  const KEY_ABOUT = /setup key|secret key|can.t scan|enter this key|authenticator|two-step|2-step|two-factor|2fa/i;
  const KEY_WHY = "an authenticator's setup key: never the agent's (choose codes by text message or email instead)";
  let withheld = []; // {el: WeakRef, why} of the blocks withheld so far on this page
  const words = (text) => text.split(/\s+/).map((w) => w.replace(/^[^A-Za-z0-9]+|[^A-Za-z0-9]+$/g, '')).filter(Boolean);
  const codeWords = (text) => words(text).filter((w) => CODE_WORD.test(w)).length;
  const OTPAUTH_ATTRIBUTES = ['href', 'src', 'alt', 'title', 'value', 'data', 'content', 'aria-label'].map((a) => `[${a}*="otpauth:" i]`).join(',');
  const otpauth = (el) => /otpauth:/i.test(Array.from(el.attributes || [], (a) => a.value).join(' '));
  // Elements the page scan looks at, at most; past them, on a page about
  // codes or a key, nothing of the rest is read or offered (`cutoff`).
  const SCAN_LIMIT = 5000;
  let cutoff = null; // the first node the scan didn't reach, on such a page
  const pastCutoff = (node) => !!cutoff && cutoff.isConnected &&
    (node === cutoff || cutoff.contains(node) || !!(cutoff.compareDocumentPosition(node) & Node.DOCUMENT_POSITION_FOLLOWING));
  function codeBlocks() {
    const kept = withheld.map((w) => ({el: w.el.deref(), why: w.why})).filter((w) => w.el && w.el.isConnected);
    const covered = (el) => kept.some((k) => k.el === el || k.el.contains(el));
    const root = document.body || document.documentElement;
    const all = root ? root.textContent || '' : '';
    cutoff = null;
    pageWords = all + ' ' + document.title;
    shadowsLeftOut = new Map();
    if (root && CODES_ABOUT.test(all + ' ' + document.title)) {
      // How many words, and code words, each element holds, counted from
      // its text nodes up (its textContent runs list items together).
      const counts = new Map();
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
      let seen = 0;
      let node = walker.nextNode();
      for (; node && seen < SCAN_LIMIT; node = walker.nextNode(), seen++) {
        const n = codeWords(node.nodeValue), w = words(node.nodeValue).length;
        if (!w) continue;
        for (let e = node.parentElement; e && e !== document.documentElement; e = e.parentElement) {
          const c = counts.get(e) || {codes: 0, words: 0};
          c.codes += n; c.words += w;
          counts.set(e, c);
        }
      }
      if (node) cutoff = node;
      const qualifies = (el) => { const c = counts.get(el); return !!c && c.codes >= 4 && c.codes * 2 >= c.words; };
      for (const el of counts.keys()) {
        // The widest block that qualifies: its parent doesn't.
        if (qualifies(el) && !(el.parentElement && qualifies(el.parentElement)) && !covered(el)) kept.push({el, why: CODES_WHY});
      }
    }
    const about = KEY_ABOUT.test(all + ' ' + document.title);
    if (root && (about || /otpauth:/i.test(all) || document.querySelector(OTPAUTH_ATTRIBUTES))) {
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT);
      let seen = 0;
      let node = walker.nextNode();
      for (; node && seen < SCAN_LIMIT; node = walker.nextNode(), seen++) {
        const el = node.nodeType === Node.TEXT_NODE ? node.parentElement : node;
        if (!el || covered(el)) continue;
        let key;
        if (node.nodeType === Node.TEXT_NODE) {
          key = /otpauth:\/\//i.test(node.nodeValue) || (about && hasKey(node.nodeValue, true));
        } else {
          key = otpauth(el) || (about && (
            // A key drawn a group per element ("JBSW" "Y3DP" …): the
            // smallest element whose whole text holds it.
            (smallKey(el) && !Array.from(el.children).some(smallKey)) ||
            // In a field's value (a read-only box showing the key).
            ((el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) && hasKey(el.value || '', true)) ||
            // A picture that may be its QR code (Fugo 42966 4(a)).
            squarePicture(el)));
        }
        if (key) kept.push({el, why: KEY_WHY});
      }
      if (node && (!cutoff || (node.compareDocumentPosition(cutoff) & Node.DOCUMENT_POSITION_FOLLOWING))) cutoff = node;
    }
    withheld = kept.map((w) => ({el: new WeakRef(w.el), why: w.why}));
    return kept;
  }
  const smallKey = (el) => { const t = el.textContent || ''; return t.length <= 200 && hasKey(t, true); };
  const squarePicture = (el) => {
    if (!['IMG', 'svg', 'CANVAS', 'PICTURE', 'OBJECT'].includes(el.tagName)) return false;
    const r = el.getBoundingClientRect();
    return r.width >= 100 && r.height >= 100 && Math.max(r.width, r.height) <= 1.25 * Math.min(r.width, r.height);
  };
  const withheldWhy = (el) => {
    for (const w of withheld) { const block = w.el.deref(); if (block && block.contains(el)) return w.why; }
    return null;
  };
  // An errand also reads open shadow roots (overlays, components), which
  // the scan for recovery codes and setup keys above doesn't see: one that
  // may hold either is left out whole, its text and its elements.
  let pageWords = ''; // the page's own text and title, as codeBlocks saw them
  let shadowsLeftOut = new Map(); // ShadowRoot -> left out, for this read
  const secretive = (root) => {
    // Its words text node by text node: its textContent runs them together.
    const texts = [];
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node && texts.length < SCAN_LIMIT; node = walker.nextNode()) texts.push(node.nodeValue);
    const own = texts.join(' ');
    const about = pageWords + ' ' + own;
    // Past the scan's limit nothing more of it is checked: on a page about
    // codes or a key it's left out, as the page past its cutoff is.
    if (texts.length >= SCAN_LIMIT && (CODES_ABOUT.test(about) || KEY_ABOUT.test(about))) return true;
    if (/otpauth:/i.test(own) || root.querySelector(OTPAUTH_ATTRIBUTES)) return true;
    if (CODES_ABOUT.test(about) && codeWords(own) >= 4) return true;
    return KEY_ABOUT.test(about) && (texts.some((text) => hasKey(text, true)) || hasKey(own, true) ||
      Array.from(root.querySelectorAll('img,svg,canvas,picture,object')).some(squarePicture) ||
      Array.from(root.querySelectorAll('input,textarea')).some((field) => hasKey(field.value || '', true)));
  };
  // Whether `node` is in, or is, a shadow root left out (or one inside one).
  function unreadShadow(node) {
    for (let root = node.getRootNode(); root instanceof ShadowRoot; root = root.host.getRootNode()) {
      if (!shadowsLeftOut.has(root)) shadowsLeftOut.set(root, secretive(root));
      if (shadowsLeftOut.get(root)) return true;
    }
    return false;
  }

  // Where a click goes when it leaves the page: another origin, a new tab.
  function leaves(el) {
    const a = el.closest('a[href]');
    if (a) {
      if (a.target && a.target !== '_self') return 'new_tab';
      try { if (new URL(a.href).origin !== location.origin) return 'other_origin'; } catch {}
    }
    const form = el.form || (el.closest && el.closest('form'));
    if (form && (el.type === 'submit' || el.tagName === 'BUTTON')) {
      try { if (new URL(form.action, location.href).origin !== location.origin) return 'other_origin'; } catch {}
      if (form.target && form.target !== '_self') return 'new_tab';
    }
    return null;
  }

  // Controls whose handler may ask Chrome for a permission (notifications,
  // location, camera, microphone), read or write the clipboard, or open a
  // window (Abbacchio's B1 audit). A page's handlers can't be seen from here,
  // so these are known by their words, or by an inline handler's code when
  // there is one: a heuristic, with the person's review of each click as the
  // real gate.
  const PERMISSION_WORDS = /notification|\balerts?\b|\blocation\b|\blocate\b|near me|\bgps\b|camera|webcam|\bscan|\bqr\b|barcode|microphone|\bmic\b|\brecord(ing)?\b|screen ?shar|share (your |my )?screen|clipboard|\bpaste\b|\bcopy\b|pop-?up|\bwindow\b|full ?screen|bluetooth|\busb\b|\binstall\b/i;
  const PERMISSION_CODE = /Notification|requestPermission|geolocation|getUserMedia|getDisplayMedia|mediaDevices|clipboard|execCommand\(\s*['"](copy|cut|paste)|\bopen\(|requestFullscreen|show(Open|Save|Directory)\w*Picker|bluetooth|\.usb\b|\.serial\b|\.hid\b|navigator\.share/;
  function asksPermission(el) {
    const words = rawName(el) + ' ' + (shownText(el) || '') + ' ' + (el.getAttribute('title') || '');
    if (PERMISSION_WORDS.test(words)) return 'it may ask for a permission, use the clipboard or open a window: the person does this';
    for (let e = el; e && e !== document.documentElement; e = e.parentElement) {
      const code = ['onclick', 'onmousedown', 'onmouseup', 'onpointerdown', 'onpointerup'].map((a) => e.getAttribute(a) || '').join(' ');
      if (PERMISSION_CODE.test(code)) return 'its handler asks for a permission, the clipboard or a window: the person does this';
    }
    return null;
  }

  // The colour a text node is drawn on, for text in its background's colour.
  function background(el) {
    for (let e = el; e; e = e.parentElement) {
      const c = getComputedStyle(e).backgroundColor;
      if (c && c !== 'transparent' && !/rgba\(.*, 0\)$/.test(c)) return c;
    }
    return 'rgb(255, 255, 255)';
  }

  /** A path for the answer: no ? or #, bounded, starting with / (after the origin, if any). */
  function webPath(origin, pathname) {
    const p = oneLine(pathname.replace(/[?#]/g, (c) => encodeURIComponent(c)), PATH_LIMIT - origin.length);
    return origin + (p.startsWith('/') ? p : '/' + p);
  }

  /** The longest start of `s` that fits in `bytes` (UTF-8) and `json` (escaped). */
  function cut(s, bytes, json) {
    let n = Math.min(s.length, bytes, json);
    while (n > 0 && (utf8(s.slice(0, n)) > bytes || jsonBytes(s.slice(0, n)) > json)) n = Math.floor(n * 0.9);
    // Never split a surrogate pair.
    if (n > 0 && /[\uD800-\uDBFF]/.test(s[n - 1])) n--;
    return s.slice(0, n);
  }

  // ---- Errands: past what covers the page (Fugo 40527) ----
  // Claktory's errand reader sets __claktoryErrand in its own isolated
  // world: there the reader reaches into open shadow roots, lists what
  // covers the page first and can clear it. The extension never does.
  const errand = () => globalThis.__claktoryErrand === true;

  // The element at (x, y), through open shadow roots.
  function deepFromPoint(x, y) {
    let e = document.elementFromPoint(x, y);
    while (e && e.shadowRoot) {
      const inner = e.shadowRoot.elementFromPoint(x, y);
      if (!inner || inner === e) break;
      e = inner;
    }
    return e;
  }
  const fromPoint = (x, y) => errand() ? deepFromPoint(x, y) : document.elementFromPoint(x, y);

  // Whether `inner` is `outer` or inside it: across open shadow roots for
  // an errand.
  function inside(outer, inner) {
    if (!errand()) return outer.contains(inner);
    for (let e = inner; e; e = e.parentNode || (e instanceof ShadowRoot ? e.host : null)) if (e === outer) return true;
    return false;
  }

  // Every element matching `selector`, in open shadow roots too.
  function deepQuery(selector, root = document, found = []) {
    for (const el of root.querySelectorAll(selector)) found.push(el);
    for (const host of root.querySelectorAll('*')) if (host.shadowRoot) deepQuery(selector, host.shadowRoot, found);
    return found;
  }

  const parentOf = (e) => e.parentElement || (e.getRootNode() instanceof ShadowRoot ? e.getRootNode().host : null);
  const modal = (e) => e.getAttribute('aria-modal') === 'true' || /^(alert)?dialog$/.test(e.getAttribute('role') || '') || (e.tagName === 'DIALOG' && e.open);
  // The outermost fixed, sticky or modal element around `el`: what floats
  // over the page.
  function floating(el) {
    let root = null;
    for (let e = el; e && e !== document.body && e !== document.documentElement; e = parentOf(e)) {
      const position = getComputedStyle(e).position;
      if (position === 'fixed' || position === 'sticky' || modal(e)) root = e;
    }
    return root;
  }
  const scrollBlocked = () => [document.documentElement, document.body].some((e) => e && ['hidden', 'clip'].includes(getComputedStyle(e).overflowY));
  const COOKIE_WORDS = /cookie|consent|gdpr|your privacy|privacy (choices|preferences|settings|center|centre)/i;
  const PRESSABLE = 'button,[role=button],input[type=button],input[type=submit],input[type=reset],a[href]';
  function pressables(root) {
    return deepQuery(PRESSABLE, root.shadowRoot || root).concat(root.matches(PRESSABLE) ? [root] : []).filter(visible);
  }
  // What covers the page now, the most of it first: cookie notices, and
  // dialogs or pop-ups over a third of it, modal, or keeping it from
  // scrolling. A fixed header or a chat button isn't one.
  function overlays() {
    const seen = new Map();
    for (let i = 1; i < 8; i++) {
      for (let j = 1; j < 8; j++) {
        const top = deepFromPoint(innerWidth * i / 8, innerHeight * j / 8);
        const root = top && floating(top);
        if (root) seen.set(root, (seen.get(root) || 0) + 1);
      }
    }
    for (const el of deepQuery('[role=dialog],[role=alertdialog],[aria-modal=true],dialog[open]')) {
      const root = floating(el) || el;
      if (!seen.has(root)) seen.set(root, 0);
    }
    const blocked = scrollBlocked();
    const found = [];
    for (const [root, count] of [...seen].sort((a, b) => b[1] - a[1])) {
      // A frame that covers the page is Claktory's to see to, from outside.
      if (!visible(root) || root.tagName === 'IFRAME' || root.tagName === 'FRAME') continue;
      const r = root.getBoundingClientRect();
      const area = Math.max(0, Math.min(r.right, innerWidth) - Math.max(r.left, 0)) * Math.max(0, Math.min(r.bottom, innerHeight) - Math.max(r.top, 0));
      const covers = area / Math.max(1, innerWidth * innerHeight);
      let kind = null;
      if (COOKIE_WORDS.test(textOf(root).slice(0, 4000)) && pressables(root).length) kind = 'cookie notice';
      else if (covers >= 0.3 || (modal(root) && covers > 0.02) || (blocked && covers > 0.1)) kind = 'pop-up';
      if (kind) found.push({root, kind, covers, shown: count});
    }
    return found;
  }
  // The overlay `el` is in, if any, among `covering`.
  const overlayOf = (el, covering) => covering.find((o) => inside(o.root, el));

  // What covers a target, in Claktory's words: never the page's.
  function coveredBy(top) {
    if (!errand() || !top) return 'covered';
    const over = overlayOf(top, overlays());
    if (over) return 'covered by a ' + over.kind + ': clear it with errand_act op dismiss, then read the page again';
    if (top.tagName === 'IFRAME' || top.tagName === 'FRAME') return 'covered by a frame: read the page again to see it';
    return 'covered by something else on the page: read it again';
  }

  // A button's choice in a cookie notice or a pop-up, by its name: the
  // most private first. Never page code, only its words.
  const PRIVATE = /^(reject|decline|refuse|deny|disagree)( all| cookies| all cookies| optional cookies| non-essential cookies| additional cookies)?$|^(use |allow |accept )?(only )?(strictly )?(necessary|essential|required)( cookies)?( only)?$|^continue without (accepting|agreeing|cookies)$/i;
  const CLOSE = /^(close|dismiss|×|✕|✖|x|no,? thanks?|not now|maybe later|not interested|skip|continue without.*|close (dialog|banner|this|pop-?up|message|window))$/i;
  const ACCEPT = /^(accept|agree|allow|ok|okay|got it|i understand|i agree|understood)( all| cookies| all cookies| and continue| and close| & close)?$/i;
  // A notice's settings, where a more private choice may be; and saving
  // them as they're set (nothing optional, by the law it answers to).
  const SETTINGS = /^(cookie |privacy )?(settings|preferences|options)$|^(manage|customi[sz]e|change|adjust|set|show)( my| your)?( cookie| privacy)? (settings|preferences|choices|options|cookies|purposes|details)$|^(manage|customi[sz]e|more options)$/i;
  const SAVE = /^(confirm|save)( my| your)? (choices|selection|settings|preferences)( and close| & close)?$|^(allow|accept) (selection|selected)$/i;
  function choice(el) {
    const words = oneLine(rawName(el), NAME_LIMIT).replace(/[.!]+$/, '').trim();
    if (PRIVATE.test(words)) return 'private';
    if (CLOSE.test(words)) return 'close';
    if (SAVE.test(words)) return 'save';
    if (SETTINGS.test(words)) return 'settings';
    if (ACCEPT.test(words)) return 'accept';
    return null;
  }
  // Fields that make an overlay the person's: sign-in, payment, a code.
  const personal = (root) => deepQuery('input,textarea,[contenteditable]', root.shadowRoot || root).some((field) => sensitive(field));

  // The visible page, bounded: `limit` bytes of text at most, and the whole
  // answer within `budget` bytes of JSON. Interactive elements first (up to
  // 200, in half the budget), then the text in reading order, by walking the
  // text of visible elements; shadow roots and frames are counted, not entered.
  function read(limit, budget) {
    const started = performance.now();
    let late = false;
    const overdue = () => (late = late || performance.now() - started > TIME_LIMIT_MS);
    const notes = {open_shadow_roots: 0, closed_shadow_roots: 0, frames: 0, canvases: 0};

    refs = new Map();
    const elements = [];
    let notListed = 0;
    let elementBytes = 2;
    const elementBudget = Math.floor(budget / 2);
    const found = codeBlocks();
    const blocks = found.map((w) => w.el);
    for (const {el, why} of found) {
      // A setup key is never offered, even as a ref.
      if (why !== CODES_WHY) continue;
      const id = ref();
      const item = {ref: id, role: 'region', name: 'recovery codes (withheld)', sensitive: CODES_WHY};
      if (!inViewport(el.getBoundingClientRect())) item.offscreen = true;
      elementBytes += utf8(JSON.stringify(item)) + 1;
      refs.set(id, {el: new WeakRef(el), fp: fingerprint(el)});
      elements.push(item);
    }
    // An errand lists what covers the page first, then what's on screen,
    // then the rest: a cookie notice at a long page's end is never cut.
    const covering = errand() ? overlays() : [];
    let candidates = document.querySelectorAll(INTERACTIVE);
    if (errand()) {
      const rank = (el) => {
        const over = covering.findIndex((o) => inside(o.root, el));
        if (over >= 0) return over;
        return inViewport(el.getBoundingClientRect()) ? 100 : 200;
      };
      candidates = deepQuery(INTERACTIVE).map((el, at) => [rank(el), at, el]).sort((a, b) => a[0] - b[0] || a[1] - b[1]).map((entry) => entry[2]);
    }
    for (const el of candidates) {
      if (elements.length && elements.length % 50 === 0 && overdue()) break;
      if (!visible(el)) continue;
      // Inside recovery codes: not offered at all, so no name of it is read.
      if (withheldWhy(el) || otpauth(el) || pastCutoff(el) || (errand() && unreadShadow(el))) continue;
      const r = role(el);
      if (r === 'hidden' || (r === 'label' && !fileLabel(el))) continue;
      if (elements.length >= ELEMENTS_LIMIT) { notListed++; continue; }
      const id = ref();
      const item = {ref: id, role: r, name: name(el)};
      const differs = shownText(el);
      if (differs) item.shows = differs;
      const m = marks(rawName(el) + (editable(el) ? '' : ' ' + textOf(el) + ' ' + el.textContent));
      if (m.length) item.marks = m;
      const s = sensitive(el);
      if (s) item.sensitive = s;
      const out = leaves(el);
      if (out) item.leaves = out;
      if (!inViewport(el.getBoundingClientRect())) item.offscreen = true;
      const over = errand() && overlayOf(el, covering);
      if (over) item.overlay = over.kind;
      if (el.disabled || el.getAttribute('aria-disabled') === 'true') item.disabled = true;
      const checked = el instanceof HTMLInputElement && ['checkbox', 'radio'].includes(el.type)
        ? (el.indeterminate ? 'mixed' : String(el.checked))
        : (el.getAttribute('aria-checked') || '').trim().toLowerCase();
      if (['true', 'false', 'mixed'].includes(checked)) item.checked = checked;
      const expanded = el.tagName === 'SUMMARY' && el.closest('details') ? String(el.closest('details').open) : (el.getAttribute('aria-expanded') || '').trim().toLowerCase();
      if (['true', 'false'].includes(expanded)) item.expanded = expanded;
      // Links on the web: the destination's path (never its query, fragment
      // or a name and password), with the origin when it's another.
      if (r === 'link' && el.href) {
        try {
          const u = new URL(el.href);
          if (u.protocol === 'https:' || u.protocol === 'http:') item.path = webPath(u.origin === location.origin ? '' : u.origin, u.pathname);
        } catch {}
      }
      const size = utf8(JSON.stringify(item)) + 1;
      if (elementBytes + size > elementBudget) { notListed++; continue; }
      elementBytes += size;
      refs.set(id, {el: new WeakRef(el), fp: fingerprint(el)});
      elements.push(item);
    }

    const answer = {
      revision, title: oneLine(document.title, 300), title_marks: marks(document.title), path: webPath('', location.pathname),
      text: '', text_marks: [], truncated: false, elements, elements_not_listed: notListed, unsupported: [], unsupported_page: null,
    };
    if (errand() && covering.length) answer.overlays = [...new Set(covering.map((o) => o.kind))];
    // What's left of the budget, with room for the notes at the end.
    let textBytes = Math.min(limit, budget);
    let textJson = budget - utf8(JSON.stringify(answer)) - 512;
    const pieces = [];
    const textMarks = new Set();
    let truncated = notListed > 0;
    const shadows = [];
    const skipped = new Set(covering.map((o) => o.root));
    const walkerOf = (root, skipping) => document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
      acceptNode(node) {
        if (skipping && skipped.has(node)) return NodeFilter.FILTER_REJECT;
        if (errand() && node.nodeType === Node.ELEMENT_NODE && node.shadowRoot && !shadows.includes(node.shadowRoot)) shadows.push(node.shadowRoot);
        if (node.nodeType === Node.ELEMENT_NODE) {
          const tag = node.tagName;
          if (tag === 'SCRIPT' || tag === 'STYLE' || tag === 'NOSCRIPT' || tag === 'TEMPLATE') return NodeFilter.FILTER_REJECT;
          if (tag === 'IFRAME' || tag === 'FRAME' || tag === 'OBJECT' || tag === 'EMBED') { notes.frames++; return NodeFilter.FILTER_REJECT; }
          if (tag === 'CANVAS') { notes.canvases++; return NodeFilter.FILTER_REJECT; }
          if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return NodeFilter.FILTER_REJECT;
          // Recovery codes: never read.
          if (blocks.includes(node) || pastCutoff(node)) return NodeFilter.FILTER_REJECT;
          if (node.shadowRoot && !errand()) notes.open_shadow_roots++;
          else if (node instanceof HTMLElement && chrome.dom && chrome.dom.openOrClosedShadowRoot(node)) notes.closed_shadow_roots++;
          if (!visible(node)) return NodeFilter.FILTER_REJECT;
          // What's typed into an editable region is a value: never read.
          if (node.isContentEditable && node.getAttribute('contenteditable') !== null) return NodeFilter.FILTER_REJECT;
          return NodeFilter.FILTER_SKIP;
        }
        return NodeFilter.FILTER_ACCEPT;
      },
    });
    let seen = 0;
    // A cut ends every walk: the text is whole up to it.
    let full = false;
    const said = (words) => { if (!full) { pieces.push(words); textBytes -= utf8(words); textJson -= jsonBytes(words); } };
    const walk = (walker) => {
    for (let node = walker.nextNode(); node && !full; node = walker.nextNode()) {
      if (++seen % 100 === 0 && overdue()) { full = true; break; }
      if (pastCutoff(node)) { truncated = true; full = true; break; }
      let piece = node.nodeValue.replace(/\s+/g, ' ');
      if (!piece.trim()) continue;
      for (const m of marks(node.nodeValue)) if (m !== 'line_break') textMarks.add(m);
      const parent = node.parentElement;
      if (parent && getComputedStyle(parent).color === background(parent)) { piece = ' ⟦same colour as its background:⟧ ' + piece + ' ⟦end⟧ '; }
      piece = shown(piece);
      const bytes = utf8(piece), json = jsonBytes(piece);
      if (bytes > textBytes || json > textJson) {
        const part = cut(piece, textBytes, textJson);
        if (part) pieces.push(part);
        truncated = true;
        full = true;
        break;
      }
      pieces.push(piece);
      textBytes -= bytes;
      textJson -= json;
    }
    };
    for (const over of covering) {
      if (unreadShadow(over.root)) continue;
      said(' ⟦' + over.kind + ':⟧ ');
      walk(walkerOf(over.root, false));
      said(' ⟦end of the ' + over.kind + '⟧ ');
    }
    walk(walkerOf(document.body || document.documentElement, true));
    for (let i = 0; i < shadows.length && i < 50; i++) if (!unreadShadow(shadows[i])) walk(walkerOf(shadows[i], true));
    if (cutoff) answer.unsupported.push('the page is too long to check for recovery codes or a setup key past its start: the rest isn\'t read');
    if ([...shadowsLeftOut.values()].some(Boolean)) answer.unsupported.push('a part of the page that may hold recovery codes or a setup key, inside a component, isn\'t read');
    if (late) {
      truncated = true;
      answer.unsupported.push('the page is too big to read whole in time: only its start was read');
    }
    answer.text = pieces.join('');
    answer.text_marks = [...textMarks];
    answer.truncated = truncated;
    // An errand's frames are Claktory's to read, beside the page.
    if (notes.frames && !errand()) answer.unsupported.push(notes.frames + ' frames not read');
    if (notes.closed_shadow_roots) answer.unsupported.push(notes.closed_shadow_roots + ' closed shadow roots not readable');
    if (notes.open_shadow_roots) answer.unsupported.push(notes.open_shadow_roots + ' open shadow roots not read');
    if (notes.canvases) answer.unsupported.push(notes.canvases + ' canvases not readable');
    if (!answer.text.replace(/\s/g, '') && !elements.length) {
      answer.unsupported_page = 'nothing readable on this page' + (notes.canvases ? ': it draws on a canvas' : '');
    }
    // Never past the budget, whatever the estimates said.
    while (utf8(JSON.stringify(answer)) > budget && answer.text) {
      answer.text = answer.text.slice(0, Math.floor(answer.text.length * 0.9));
      answer.truncated = true;
    }
    return answer;
  }

  // ---- Actions the person reviews ----

  // A target still the one that was read: connected, same fingerprint,
  // visible, enabled, on screen and not covered; never scrolled to.
  function check(id) {
    const found = known(id);
    if (found.error) return found;
    return placed(found.el, found.kept);
  }

  /** The element a ref was read as, if it's still there and still the same. */
  function known(id) {
    const kept = refs.get(id);
    if (!kept) return {error: 'stale_target', why: 'unknown: read the page again'};
    const el = kept.el.deref();
    if (!el || !el.isConnected) return {error: 'stale_target', why: 'removed'};
    if (fingerprint(el) !== kept.fp) return {error: 'stale_target', why: 'changed'};
    return {el, kept};
  }

  /** Visible, enabled, on screen and not covered: where the person sees it. */
  function placed(el, kept) {
    if (!visible(el)) return {error: 'stale_target', why: 'hidden'};
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') return {error: 'stale_target', why: 'disabled'};
    const r = el.getBoundingClientRect();
    if (!inViewport(r)) return {error: 'stale_target', why: 'offscreen: scroll, read, then act'};
    const x = Math.min(Math.max(r.left + r.width / 2, 0), innerWidth - 1), y = Math.min(Math.max(r.top + r.height / 2, 0), innerHeight - 1);
    const top = fromPoint(x, y);
    if (!top || (top !== el && !inside(el, top) && !(el.tagName === 'LABEL' && el.control === top))) return {error: 'stale_target', why: coveredBy(top)};
    return {el, kept};
  }

  const TEXT_TYPES = ['text', 'search', 'email', 'url', 'tel', 'number', 'date', 'time', 'datetime-local', 'month', 'week'];
  const BUTTON_TYPES = ['checkbox', 'radio', 'submit', 'button', 'reset', 'image'];
  const fillable = (el) => (el instanceof HTMLInputElement && TEXT_TYPES.includes((el.type || 'text').toLowerCase())) || el instanceof HTMLTextAreaElement || el.isContentEditable;
  const submits = (el) => !!(el.form || el.closest('form')) && ((el instanceof HTMLButtonElement && (el.getAttribute('type') || 'submit').toLowerCase() === 'submit') || (el instanceof HTMLInputElement && ['submit', 'image'].includes(el.type)));

  // What a click or fill would do, for the person to review, or why it's
  // refused: the checks a target passes now and again just before it's done.
  function describe(id, kind, text) {
    const t = known(id);
    if (t.error) return t;
    const el = t.el;
    // What's refused is refused wherever it is on the page.
    const s = sensitive(el);
    if (s) return {error: 'sensitive_action', why: s};
    if (kind === 'click') {
      if (el instanceof HTMLSelectElement || el.tagName === 'OPTION') return {error: 'unsupported_target', why: 'choosing from a list isn\'t supported: the person picks it'};
      if (fillable(el) && !(el instanceof HTMLInputElement && BUTTON_TYPES.includes(el.type))) return {error: 'unsupported_target', why: 'a text field is filled, not clicked'};
      if (leaves(el) === 'new_tab') return {error: 'sensitive_action', why: 'it opens a new tab or window: the person takes over there'};
      const a = el.closest('a[href]');
      if (a && !/^https?:/i.test(a.href) && !(a.getAttribute('href') || '').startsWith('#')) return {error: 'sensitive_action', why: 'it opens another app or runs the page\'s own link code: the person does this'};
      const p = asksPermission(el);
      if (p) return {error: 'sensitive_action', why: p};
      if (errand() && choice(el) === 'accept') {
        const over = overlayOf(el, overlays());
        if (over && over.kind === 'cookie notice' && pressables(over.root).some((b) => ['private', 'settings'].includes(choice(b)))) {
          return {error: 'sensitive_action', why: 'a cookie notice with a more private choice: clear it with errand_act op dismiss, which picks it'};
        }
      }
    } else {
      if (el instanceof HTMLSelectElement) return {error: 'unsupported_target', why: 'choosing from a list isn\'t supported: the person picks it'};
      if (!fillable(el)) return {error: 'unsupported_target', why: 'it isn\'t a text field'};
      if (/[\r\n]/.test(text) && el instanceof HTMLInputElement) return {error: 'unsupported_target', why: 'a one-line field can\'t take line breaks'};
    }
    const where = placed(el, t.kept);
    if (where.error) return where;
    const action = {path: webPath('', location.pathname), role: role(el), name: name(el)};
    const differs = shownText(el);
    if (differs) action.shows = differs;
    const m = marks(rawName(el) + (editable(el) ? '' : ' ' + textOf(el)));
    if (m.length) action.marks = m;
    if (kind === 'click') {
      const a = el.closest('a[href]');
      if (a) {
        try { const u = new URL(a.href); action.destination = webPath(u.origin === location.origin ? '' : u.origin, u.pathname); } catch {}
      } else if (submits(el)) {
        const form = el.form || el.closest('form');
        try { const u = new URL(form.action, location.href); action.destination = webPath(u.origin === location.origin ? '' : u.origin, u.pathname); } catch {}
        action.submits_form = true;
      }
      const out = leaves(el);
      if (out) action.leaves = out;
    }
    return {action};
  }

  // Any change to the target, or to the form it's in, while the person
  // reviews it makes the review stale.
  let watching = null;
  function watch(id) {
    if (watching) watching.disconnect();
    const kept = refs.get(id);
    const el = kept && kept.el.deref();
    if (!el) return {error: 'stale_target', why: 'removed'};
    kept.dirty = false;
    watching = new MutationObserver(() => { kept.dirty = true; });
    watching.observe(el, {subtree: true, childList: true, attributes: true, characterData: true});
    const form = el.form || el.closest('form');
    if (form && form !== el) watching.observe(form, {attributes: true});
    return {};
  }

  const valueOf = (el) => (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) ? el.value : el.isContentEditable ? el.textContent : null;

  // Fill, then read the value back after the page had a moment to react
  // (React re-rendering, an editor undoing a change it didn't make).
  async function fill(el, text) {
    const before = valueOf(el);
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
      const proto = el instanceof HTMLInputElement ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, text);
      el.dispatchEvent(new Event('input', {bubbles: true}));
      el.dispatchEvent(new Event('change', {bubbles: true}));
    } else {
      // An editable region: its contents replaced the way editors listen to.
      el.focus();
      getSelection().selectAllChildren(el);
      document.execCommand('insertText', false, text);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
    return filled(el, text, before);
  }

  // Whether `el` holds `text` now, and if not why and what it holds.
  function filled(el, text, before) {
    const after = valueOf(el) || '';
    const took = el.isContentEditable ? after.replace(/\s+/g, ' ').trim() === text.replace(/\s+/g, ' ').trim() : after === text;
    const answer = {done: true, took};
    if (!took) {
      answer.why = after === before ? 'the page refused it: nothing changed' : 'the page changed what was typed';
      answer.now = oneLine(after, NAME_LIMIT);
    }
    if (el instanceof HTMLInputElement && el.maxLength > 0 && after.length > el.maxLength) answer.over_max_length = el.maxLength;
    return answer;
  }

  // Does it, once, if nothing changed since the person's review.
  async function act(id, kind, text) {
    if (watching) { watching.disconnect(); watching = null; }
    const kept = refs.get(id);
    if (kept && kept.dirty) return {error: 'stale_target', why: 'changed while it was reviewed'};
    const t = check(id);
    if (t.error) return t;
    if (kind === 'click') {
      t.el.click();
      return {done: true};
    }
    return fill(t.el, text);
  }

  function scroll(direction, pages) {
    window.scrollBy({top: (direction === 'up' ? -1 : 1) * innerHeight * pages, behavior: 'instant'});
    return {scroll_y: Math.round(scrollY)};
  }

  // ---- Errands only (agent-browser-r11.md) ----
  // Claktory's errand reader, in its own isolated world, sets
  // __claktoryErrand: the browser's own input clicks, types and scrolls
  // there, paced as a person's, and these only check and measure. The
  // extension never has them.

  // A target still the one read, where it's acted on: its box and the
  // viewport's; for a text field, which kind and whether it holds text
  // (never what), keeping what it held to tell what typing changed.
  function aim(id) {
    const t = check(id);
    if (t.error) return t;
    const r = t.el.getBoundingClientRect();
    const answer = {left: r.left, top: r.top, width: r.width, height: r.height, view_width: innerWidth, view_height: innerHeight};
    if (fillable(t.el)) {
      t.kept.before = valueOf(t.el) || '';
      answer.field = t.el instanceof HTMLInputElement ? 'input' : t.el instanceof HTMLTextAreaElement ? 'textarea' : 'editable';
      answer.holds_text = t.kept.before !== '';
      answer.focused = focusedOn(t.el);
    }
    return answer;
  }

  // The target is what's at (x, y), now: checked right before the press.
  function hits(id, x, y) {
    const t = check(id);
    if (t.error) return t;
    if (!(x >= 0 && y >= 0 && x < innerWidth && y < innerHeight)) return {error: 'stale_target', why: 'offscreen: scroll, read, then act'};
    const top = fromPoint(x, y);
    const el = t.el;
    if (!top || (top !== el && !inside(el, top) && !(el.tagName === 'LABEL' && el.control === top))) return {error: 'stale_target', why: coveredBy(top)};
    return {};
  }

  // Whether the keyboard's focus is on `el` itself, through open shadow
  // roots; for a part of an editable region, its caret is in it.
  function focusedOn(el) {
    let active = document.activeElement;
    while (active && active.shadowRoot && active.shadowRoot.activeElement) active = active.shadowRoot.activeElement;
    if (active === el) return true;
    const selection = getSelection();
    return !!(active && el.isContentEditable && active.isContentEditable && active.contains(el) && selection && el.contains(selection.anchorNode));
  }

  // Checked before every key: still the field read, still focused.
  // In a frame, the frame must have the keyboard too.
  function focused(id, frame) {
    const found = known(id);
    if (found.error) return found;
    return {focused: focusedOn(found.el) && (!frame || document.hasFocus())};
  }

  // After typing: what the field holds against what was meant.
  function typed(id, text) {
    const found = known(id);
    if (found.error) return found;
    return filled(found.el, text, found.kept.before === undefined ? null : found.kept.before);
  }

  // Elements a wheel must not turn: it would change their value or scroll
  // something the reader doesn't read.
  const UNWHEELED = 'input,textarea,select,option,iframe,frame,object,embed,canvas,video,audio,[contenteditable]:not([contenteditable=false]),[role=slider],[role=spinbutton],[role=listbox],[role=combobox]';
  // Scrolled by the page's own scrollbar, not a box inside it.
  function pageScrolls(el) {
    for (let e = el; e && e !== document.body && e !== document.documentElement; e = e.parentElement) {
      const s = getComputedStyle(e);
      if (/(auto|scroll|overlay)/.test(s.overflowY) && e.scrollHeight > e.clientHeight + 1) return false;
    }
    return true;
  }
  function wheelable(x, y) {
    if (!(x >= 0 && y >= 0 && x < innerWidth && y < innerHeight)) return false;
    const top = deepFromPoint(x, y);
    const active = document.activeElement;
    const onFocused = !!active && active !== document.body && active !== document.documentElement && (active === top || active.contains(top));
    return !!top && !top.closest(UNWHEELED) && !onFocused && pageScrolls(top);
  }

  // Where the wheel turns: (x, y) if it still may, or another such place;
  // and how far the page is scrolled and can be.
  function wheelSpot(x, y) {
    const where = {scroll_y: Math.round(scrollY), scroll_max: Math.max(0, Math.round(document.documentElement.scrollHeight - innerHeight)), view_width: innerWidth, view_height: innerHeight};
    if (Number.isFinite(x) && Number.isFinite(y) && wheelable(x, y)) return Object.assign(where, {x, y});
    for (const fy of [0.5, 0.35, 0.65, 0.2, 0.8]) {
      for (const fx of [0.5, 0.3, 0.7, 0.15, 0.85]) {
        const px = Math.round(innerWidth * fx), py = Math.round(innerHeight * fy);
        if (wheelable(px, py)) return Object.assign(where, {x: px, y: py});
      }
    }
    return {error: 'unsupported_target', why: 'nowhere on the page turns its own scroll without touching a field, a list or a frame: the person scrolls it'};
  }

  // What covers the page, for a dismiss: each overlay, whether it's the
  // person's (sign-in, payment, a code), and its buttons with their names
  // and choices; a link or a button that sends a form is marked, never
  // pressed to dismiss.
  function covers() {
    const found = overlays().slice(0, 5).map((o) => ({kind: o.kind, covers: Math.round(o.covers * 100), personal: personal(o.root), buttons: buttonsIn(o.root), switches: switchesIn(o.root)}));
    return {overlays: found, scroll_blocked: scrollBlocked()};
  }
  // The purposes a consent panel in `root` may switch on (Fugo 41149):
  // every checkbox and switch that can change (a disabled one is what the
  // site needs, always on), whether it's on, and a ref to press it (itself,
  // or its label when it's drawn hidden), or none when neither shows.
  function switchesIn(root) {
    const switches = [];
    const toggles = deepQuery('input[type=checkbox],[role=switch],[role=checkbox]', root.shadowRoot || root);
    for (const el of toggles.slice(0, 40)) {
      if (el.disabled || el.getAttribute('aria-disabled') === 'true') continue;
      const on = el instanceof HTMLInputElement ? el.checked : el.getAttribute('aria-checked') === 'true';
      const target = visible(el) ? el : Array.from(el.labels || []).find(visible);
      const entry = {on};
      if (target) {
        const id = ref();
        refs.set(id, {el: new WeakRef(target), fp: fingerprint(target)});
        entry.ref = id;
      }
      switches.push(entry);
    }
    return switches;
  }
  // The buttons in `root`, each with a ref, its name and its choice.
  function buttonsIn(root) {
    const buttons = [];
    for (const b of pressables(root).slice(0, 40)) {
      const id = ref();
      refs.set(id, {el: new WeakRef(b), fp: fingerprint(b)});
      const button = {ref: id, name: oneLine(rawName(b), NAME_LIMIT), choice: choice(b)};
      // A link that goes anywhere (another page, or runs the page's own
      // link code) is never pressed to dismiss: only one to this page.
      const a = b.closest('a[href]');
      if (a) {
        try {
          const to = new URL(a.href, location.href), here = new URL(location.href);
          to.hash = ''; here.hash = '';
          if (to.href !== here.href) button.link = true;
        } catch (_) { button.link = true; }
      }
      if (submits(b) && !((b.form || b.closest('form')).getAttribute('method') || '').match(/^dialog$/i)) button.submits = true;
      const s = sensitive(b);
      if (s) button.sensitive = s;
      buttons.push(button);
    }
    return buttons;
  }
  // Another site's frame that covers the page, for a dismiss: only its
  // buttons, whether it's about cookies and whether it asks for what's the
  // person's. Never its text.
  function frameButtons() {
    refs = new Map();
    const root = document.body || document.documentElement;
    if (!root) return {cookies: false, personal: false, buttons: [], switches: []};
    const cookies = COOKIE_WORDS.test(document.title + ' ' + textOf(root).slice(0, 4000));
    return {cookies, personal: personal(root), buttons: buttonsIn(root), switches: switchesIn(root)};
  }

  function errandOp(cmd) {
    if (cmd.op === 'covers') return covers();
    if (cmd.op === 'frame_buttons') return frameButtons();
    if (cmd.op === 'aim' && typeof cmd.target === 'string') return aim(cmd.target);
    if (cmd.op === 'hits' && typeof cmd.target === 'string' && Number.isFinite(cmd.x) && Number.isFinite(cmd.y)) return hits(cmd.target, cmd.x, cmd.y);
    if (cmd.op === 'focused' && typeof cmd.target === 'string') return focused(cmd.target, cmd.frame === true);
    if (cmd.op === 'typed' && typeof cmd.target === 'string' && typeof cmd.text === 'string') return typed(cmd.target, cmd.text);
    if (cmd.op === 'wheel_spot') return wheelSpot(cmd.x, cmd.y);
    return null;
  }

  function run(cmd) {
    if (!cmd || typeof cmd !== 'object') return {error: 'bad_command'};
    if (globalThis.__claktoryErrand) {
      const answer = errandOp(cmd);
      if (answer) return answer;
    }
    if (cmd.op === 'read' && Number.isInteger(cmd.limit) && Number.isInteger(cmd.budget)) return read(Math.min(Math.max(cmd.limit, 1), 65536), Math.min(Math.max(cmd.budget, 1024), 65536));
    if (cmd.op === 'scroll' && ['up', 'down'].includes(cmd.direction) && [1, 2, 3].includes(cmd.pages)) return scroll(cmd.direction, cmd.pages);
    if (cmd.op === 'forget') { refs = new Map(); if (watching) { watching.disconnect(); watching = null; } return {}; }
    const kind = cmd.kind === 'click' || cmd.kind === 'fill' ? cmd.kind : null;
    const text = kind === 'fill' && typeof cmd.text === 'string' ? cmd.text : '';
    if (cmd.op === 'describe' && kind && typeof cmd.target === 'string') return describe(cmd.target, kind, text);
    if (cmd.op === 'watch' && typeof cmd.target === 'string') return watch(cmd.target);
    if (cmd.op === 'act' && kind && typeof cmd.target === 'string') return act(cmd.target, kind, text);
    // An errand's fill from the vault (#435 W6): the target, still the one
    // read and where the person would see it, held for Claktory to fill by
    // node. Only in an errand's own isolated world, which sets
    // __claktoryFills; the page can't reach that world's globalThis.
    if (cmd.op === 'element' && globalThis.__claktoryFills && typeof cmd.target === 'string') {
      const t = check(cmd.target);
      if (t.error) return t;
      globalThis.__claktoryElement = t.el;
      return {};
    }
    return {error: 'bad_command'};
  }

  chrome.runtime.onMessage.addListener((cmd, sender, reply) => {
    // Only this extension's worker: never another frame, page or extension.
    if (sender.id !== chrome.runtime.id || sender.tab) return false;
    Promise.resolve()
      .then(() => run(cmd))
      .then(reply, () => reply({error: 'unsupported_page', why: 'the page couldn\'t be read'}));
    return true;
  });
})();
