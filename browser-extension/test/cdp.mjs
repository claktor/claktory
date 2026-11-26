// A small Chrome DevTools Protocol client over --remote-debugging-pipe, with
// no dependencies: Chrome for Testing in a throwaway profile, for the
// extension's tests only.
import {spawn} from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

/// Chrome for Testing: CHROME, or the newest one @puppeteer/browsers put in
/// ~/.cache/chrome-for-testing.
export function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  const root = path.join(os.homedir(), '.cache', 'chrome-for-testing', 'chrome');
  const found = fs.existsSync(root) ? fs.readdirSync(root).filter((d) => d.startsWith('mac_arm-') || d.startsWith('mac-') || d.startsWith('linux')).sort() : [];
  for (const dir of found.reverse()) {
    for (const bin of ['chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing', 'chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing', 'chrome-linux64/chrome']) {
      const file = path.join(root, dir, bin);
      if (fs.existsSync(file)) return file;
    }
  }
  throw new Error('No Chrome for Testing: set CHROME, or run: npx @puppeteer/browsers install chrome@stable --path ~/.cache/chrome-for-testing');
}

export class Chrome {
  /// Starts Chrome headless in `profile` (a fresh folder, never the
  /// person's), with `args` added.
  static async launch({profile, args = [], env = process.env}) {
    const chrome = new Chrome();
    chrome.process = spawn(findChrome(), [
      '--headless=new', '--remote-debugging-pipe', '--enable-unsafe-extension-debugging', '--no-first-run', '--no-default-browser-check',
      '--disable-background-networking', '--disable-component-update', '--disable-sync',
      // Never macOS's Keychain prompt for "Chrome Safe Storage" (Bruno 41667).
      '--use-mock-keychain', '--password-store=basic', `--user-data-dir=${profile}`, ...args, 'about:blank',
    ], {stdio: ['ignore', 'ignore', process.env.CHROME_LOG ? 'inherit' : 'ignore', 'pipe', 'pipe'], env});
    chrome.write = chrome.process.stdio[3];
    chrome.next = 1;
    chrome.waiting = new Map();
    chrome.listeners = [];
    let buffer = Buffer.alloc(0);
    chrome.process.stdio[4].on('data', (chunk) => {
      buffer = Buffer.concat([buffer, chunk]);
      let end;
      while ((end = buffer.indexOf(0)) >= 0) {
        const message = JSON.parse(buffer.subarray(0, end).toString('utf8'));
        buffer = buffer.subarray(end + 1);
        chrome.dispatch(message);
      }
    });
    chrome.exited = new Promise((resolve) => chrome.process.on('exit', resolve));
    await chrome.send('Target.setDiscoverTargets', {discover: true, filter: [{}]});
    return chrome;
  }

  dispatch(message) {
    if (message.id && this.waiting.has(message.id)) {
      const {resolve, reject, method} = this.waiting.get(message.id);
      this.waiting.delete(message.id);
      if (message.error) reject(new Error(`${method}: ${message.error.message}`));
      else resolve(message.result);
      return;
    }
    for (const listener of [...this.listeners]) listener(message);
  }

  send(method, params = {}, sessionId) {
    const id = this.next++;
    const message = {id, method, params};
    if (sessionId) message.sessionId = sessionId;
    this.write.write(JSON.stringify(message) + '\0');
    return new Promise((resolve, reject) => this.waiting.set(id, {resolve, reject, method}));
  }

  /// The first event `method` (for `sessionId`, when given) that `test` likes.
  event(method, test = () => true, ms = 10000) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.listeners = this.listeners.filter((l) => l !== listener); reject(new Error(`no ${method} in ${ms} ms`)); }, ms);
      const listener = (message) => {
        if (message.method === method && test(message.params, message.sessionId)) {
          clearTimeout(timer);
          this.listeners = this.listeners.filter((l) => l !== listener);
          resolve(message.params);
        }
      };
      this.listeners.push(listener);
    });
  }

  /// A target whose info `test` likes, now or once it appears.
  async target(test, ms = 10000) {
    const appeared = this.event('Target.targetCreated', (p) => test(p.targetInfo), ms).then((p) => p.targetInfo);
    const {targetInfos} = await this.send('Target.getTargets');
    const now = targetInfos.find(test);
    if (now) { appeared.catch(() => {}); return now; }
    return appeared;
  }

  async attach(targetId) {
    const {sessionId} = await this.send('Target.attachToTarget', {targetId, flatten: true});
    return sessionId;
  }

  /// Evaluates `fn(...args)` in a session (a page, a worker), awaiting it,
  /// and returns its value as JSON.
  async evaluate(sessionId, fn, ...args) {
    const {result, exceptionDetails} = await this.send('Runtime.evaluate', {
      expression: `(${fn})(...${JSON.stringify(args)})`, awaitPromise: true, returnByValue: true,
    }, sessionId);
    if (exceptionDetails) throw new Error(exceptionDetails.exception?.description || exceptionDetails.text);
    return result.value;
  }

  async close() {
    try { await Promise.race([this.send('Browser.close'), new Promise((r) => setTimeout(r, 2000))]); } catch {}
    this.process.kill();
    await this.exited;
  }
}
