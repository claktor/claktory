# Claktory's Chrome extension

Lets an agent read a tab the person chooses, after they confirm it in Claktory on their Mac, and click, type or go to another page of the same site, one action at a time, each after the person allows it there (browser control, B2a and B2b; browser-control.md §§5–7 in Research). Browser control stays off for every agent until its security proofs (B6a, B6b) pass.

## How a session goes

1. An agent asks for a session (`browser_session` in factoryd); Claktory on the Mac shows it as pending.
2. The person opens the tab and clicks Claktory's toolbar button. The popup shows the site and **Connect**: Chrome grants access to this tab only (activeTab), and only because of that click.
3. **Connect** starts Claktory's helper (native messaging), which tells the Mac app. The app shows a 6-digit code, and the popup shows the same one with "Confirm on your Mac".
4. The person confirms on the Mac: the helper sends `bound`, and only then does the worker put its reader into the page.
5. Commands come through the helper, one at a time, each checked here (PROTOCOL.md). A click, fill or navigation is shown to the person on their Mac first. After **Allow once**, it's done when they're back in the tab, if nothing changed meanwhile. Some controls are refused outright: passwords, codes, payments, files, downloads, permissions, the clipboard, new windows.
6. The session ends at the first of these:
   - **Stop** in the popup, or Stop in Claktory;
   - the tab closing or starting to load another site;
   - 15 minutes, 5 idle, 100 commands or 2 MiB.
   Another tab or window in front, a window the page opens, or a dialog pauses it until **Resume**. Afterwards, **Close this tab** ends Chrome's own grant, which otherwise lasts until the tab leaves the site.

## Files

- `src/`: the extension. It has these permissions and nothing else: `activeTab`, `scripting`, `nativeMessaging`.
  - It has no host permissions and no content scripts in the manifest.
  - `externally_connectable` is `{"ids": []}`, so no other extension or page can connect.
  - It never runs in private windows.
  - `sw.js` is the worker. `content.js` is the reader, put into the top frame only after `bound`. `popup.*` is the popup.
- `config.js`: the native host's name, rewritten for Dev by `build.sh`.
- `build.sh [--dev] OUT`: the release, or Claktory Dev (its fixed id from `dev-public-key.b64`, host `com.claktory.browser.dev`).
- `PROTOCOL.md`: the frames between the worker and the helper.
- `test/`: Chrome for Testing, headless, in a throwaway profile, with a stand-in helper (`host.mjs`) whose frames the test plays as Claktory, and pages on a local HTTPS server.

## Tests

```sh
npx @puppeteer/browsers install chrome@stable --path ~/.cache/chrome-for-testing   # once
node browser-extension/test/run.mjs            # or CHROME=/path/to/Chrome-for-Testing; a name filter as the argument
```

They need Node 22 or later, and no packages. `SLOW=1` adds a 2-minute test of a review nobody answers. They check every read result against B3a's schema, as the backend does. They cover:
- the manifest;
- refused pages;
- pairing and reading;
- actions: review, Allow, Deny, the hash (against the helper's test vector), refusals, a target that changes during its review, a dialog, a navigation, and reviews ended by another tab or Stop;
- same-origin navigation, and ending on another origin;
- pausing and Resume;
- Stop, and Close this tab;
- strict frames;
- limits;
- big pages, hostile pages and edge cases;
- that other extensions get no answer.

Use Chrome for Testing 157 (157.0.8103.80 is the one measured). Headless 158 opens an extension's popup as a tab of its own, and that click doesn't let the extension into the page, so every test that connects fails there. Real Chrome shows the popup as usual.

Branded Chrome ignores `--load-extension`, so a manual pass on Chrome itself comes before a release. So does a pass with a visible window, for focus changes and permission prompts, which headless Chrome can't show.
