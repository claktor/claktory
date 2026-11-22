# Claktory's browser extension ↔ helper protocol (version 1)

Reading (B2a) and actions the person reviews (B2b).

This is the wire between the extension's service worker (`src/sw.js`) and Claktory's browser helper (`crates/claktory-browser-host`), agreed between the extension's owner (Cioccolata) and the helper's (Secco). The helper passes the worker's frames to the Mac app and back. The app and backend side is in Giorno's B3a contract: the proto messages and the read result's schema (`crates/factoryd/src/browser/schema.rs`).

## Transport

- Chrome's native messaging, to host `com.claktory.browser`, or `com.claktory.browser.dev` for Claktory Dev, whose extension id is `jpdkjilehjhinhohikojjngehpbakcmo`. The release's id is the Chrome Web Store's.
- Each frame is one JSON object of at most 256 KiB, checked before it's read. A `result` is at most 64 KiB.
- Both ends refuse unknown fields and unknown types:
  - the helper closes with `bad_frame` or `bad_command`;
  - the worker ends the session with `helper_refused`.
- One helper process is one session. Chrome starts it when the person chooses **Connect** in the popup, and it ends with the session.

## Worker → helper

| Frame | When |
|---|---|
| `{"type":"hello","v":1,"tab":T,"origin":"https://host[:port]","title":S}` | First and only once. `T` is Chrome's tab id; the helper pins `tab` and `origin` for its life, and every later frame carries the same `tab`. `origin` is always https, never an IP address, `localhost`, `*.localhost` or `*.local` (the worker refuses those pages itself). `title` is one line of at most 300 characters, with control, bidi and zero-width characters escaped as `⟦U+202E⟧`. |
| `{"type":"result","tab":T,"id":N,"result":R}` | The answer to command `N`, the only one outstanding. `R` is a read result, a scroll result or a failure (below). |
| `{"type":"paused","tab":T,"reason":W}` | The session paused. `W` is `tab_inactive` (another tab became active in its window), `window_blurred` (another window or app came to the front, or the tab moved to another window), `popup_opened` (the page opened a tab or window) or `dialog_open` (the page stopped answering, a dialog may be open). Commands get `paused` until the person resumes it. |
| `{"type":"resumed","tab":T}` | The person chose **Resume** in the popup, with the tab in front. |
| `{"type":"review","tab":T,"id":N,"hash":H,"expires_ms":E,"action":A}` | A click, fill or navigation the extension checked and would do, for the person to allow or deny on their Mac (below). Once per command; the command stays in flight until the person decides. |
| `{"type":"ended","tab":T,"reason":W}` | The worker ended the session. `W` is `tab_closed`, `origin_changed` (the tab started loading another origin, or Chrome took its grant away), `stopped` (**Stop** or **Cancel** in the popup), `expired` (15 minutes, 5 idle, or 5 waiting for the person to confirm), `limit_exceeded` (100 commands or 2 MiB returned) or `helper_refused`. The helper relays it and exits; the worker closes its side a second later if the helper hasn't. |

A disabled or removed extension sends nothing: Chrome just closes the port.

## Helper → worker

| Frame | When |
|---|---|
| `{"type":"pairing","code":"123456"}` | The code the Mac app shows, exactly 6 digits, for the popup to show beside the site and title ("Confirm on your Mac"). It comes when the app sends it, normally milliseconds after `hello`; a second one replaces the first. Only before `bound`. |
| `{"type":"bound","agent":S,"until_ms":U,"purpose"?:P}` | The person confirmed the session on their Mac. `agent` is 1–100 characters, `until_ms` the session's end (ms since 1970), `purpose` the agent's words (at most 500 characters), shown as plain text. Only once, and only after `pairing`. The worker then puts its reader into the tab's top frame, and ends the session itself at the earlier of `until_ms` and 15 minutes from now. |
| `{"type":"command","id":N,"session":S,"sequence":Q,"cmd":C}` | One command. `S` is the backend's session id (1–128 characters), the same for the helper's whole life; ids and sequences strictly increase; one in flight at a time. `C` is exactly one of:<br>`{"op":"read","limit"?:1..65536}`<br>`{"op":"scroll","direction":"up"\|"down","pages":1..3}`<br>`{"op":"click","document":D,"revision":R,"target":REF}`<br>`{"op":"fill","document":D,"revision":R,"target":REF,"text":S}` (at most 8192 characters; no control characters but line breaks and tabs)<br>`{"op":"navigate","url":U}` (the session's origin, then a path: no query, fragment, backslash or name and password).<br>`D` and `REF` come from the last read. Before `bound` the answer is `approval_pending`. |
| `{"type":"decision","id":N,"hash":H,"allow":true\|false}` | The person's **Allow once** or **Deny** on the review of command `N`, with its hash. Only for the review waiting, once. It only ever comes from the person's click in the Mac app's own review; the backend has no way to send one. |
| `{"type":"stop"}` | Claktory stopped the session (Stop on the Mac, or the backend ended it). The worker ends it without answering, then the helper closes. |
| `{"type":"ended","reason":W}` | The helper ended it: `no_app` (Claktory isn't open on this Mac; the helper never launches it), `app_refused`, `app_closed`, `wrong_tab`, `bad_frame` or `bad_command`. Then it closes. |

## Results

As in B3a's schema, which the backend enforces exactly:

- **Read:** `{"document","revision","title","title_marks","path","text","text_marks","truncated","elements","elements_not_listed","unsupported","unsupported_page"}`.
  - `document` is an opaque id that changes with every document the reader is put into; a read after a same-origin navigation starts on the new one.
  - `text` is the page's visible text in reading order, at most `limit` bytes (UTF-8), and the whole result fits in 64 KiB: elements take at most half of it, then text the rest.
  - `elements` lists at most 200 visible controls: `{ref, role, name, shows?, marks?, sensitive?, leaves?, offscreen?, disabled?, checked?, expanded?, path?}`.
  - Every string is one line, with control, bidi and zero-width characters escaped. Names are at most 200 characters, marker included.
  - Never included: field values, editable text, hidden or off-page text, query strings, fragments, or a link's name and password.
- **Scroll:** `{"scroll_y":n}`. It scrolls the document only, never to a target.
- **Failure:** `{"error":E,"why"?:S}`, where `E` is one of:
  - `approval_pending`, `paused`, `limit_exceeded`;
  - for actions: `denied`, `expired`, `stale_target`, `sensitive_action`, `unsupported_target`, `outcome_unknown`;
  - `stale_document`: the tab has another document, or the page didn't answer in 3 s. Read again;
  - `unsupported_page`;
  - `origin_changed`, sent with `ended`.

## Actions: reviewed, then done once

1. **Checked first.** The target is the element the last read listed: still connected, with the same role, name and type. `document` must be the current one (`stale_document` otherwise; `revision` is kept but not compared, since busy pages change every second).
   - Refused at once, with no review (`sensitive_action`): passwords, hidden and file fields, file pickers, downloads, one-time codes, card and payment fields, payment and account-closing buttons, links and forms that open a new tab, links to other apps (mailto:, javascript: and the like).
   - Also refused: controls that may ask Chrome for a permission, use the clipboard or open a window: notifications, location, camera, scanning and QR codes, microphone, screen sharing, copy and paste, pop-ups and windows. These are known by their words or an inline handler's code, since a page's handlers can't be seen; the person's review is the real gate.
   - `unsupported_target`: lists, non-text fields, clicking a text field, a line break for a one-line field, and a review too long to show.
   - Then the target must be visible, enabled, on screen and not covered (`stale_target`); nothing is ever scrolled to.
2. **Review.** `A` is plain data for the Mac app to word in its own fixed text:
   - `{op, origin, path, role, name, shows?, marks?, text?, destination?, leaves?, warnings}`. Every string is escaped as in a read.
   - `text` is the fill text exactly: a line break shows as ⏎; tabs, other controls, bidi and zero-width characters as ⟦U+0009⟧; and a ⏎ or ⟦ in the text itself the same way.
   - `destination` is where a link, a form's button or a navigation goes: a path, or another origin and path.
   - `warnings` ⊆ `may_send_as_typed` (fill), `may_ask_permission` (every click), `submits_form`, `ends_session` (it leaves the origin).
   - `A` is at most 64 KiB, and every number in it is whole.
   - `H` is SHA-256 (lowercase hex) over the canonical JSON of `{"action":A,"command":N,"document":D,"op":C,"sequence":Q,"session":S,"target":REF}`. `C` is the op exactly as received; `D` and `REF` are null for a navigation. Canonical: keys sorted by UTF-16 code units at every level, no whitespace, strings as `JSON.stringify` writes them. The helper computes it too and refuses a review whose hash differs (test vector: `crates/claktory-browser-host/src/canonical.rs`).
   - `E` is when the review lapses: 2 minutes, on this Mac's clock (ms since 1970).
3. **While the person reviews it:**
   - Chrome going to the background doesn't pause the session: that's Claktory's own review.
   - Another tab, a window the page opens, or a dialog pauses the session, and the answer is `paused`, with nothing done.
   - Stop or a closed port ends it, with nothing done.
   - Any change to the target, or to the form it's in, makes it stale.
4. **Deny** → `{"error":"denied"}`. No decision in 2 minutes → `{"error":"expired"}`. Either way, Chrome not in front afterwards pauses the session.
5. **Allow:** the person goes back to the tab. The extension never brings Chrome or the tab forward itself; Claktory's toolbar button shows GO meanwhile.
   - Once that tab is in front in the focused window, within 10 seconds of the decision, everything is checked again: the tab, the origin, the document and the target. The review is built afresh, and its hash must equal `H`.
   - Then the action is done, once, and never tried again.
   - Not back in 10 s → `expired`, and the session pauses. Anything changed → `stale_target` (or `stale_document`), and nothing is done.

**Results of an action** (a result without `error` only ever follows an allowed review; the helper checks):
- click: `{"done":true}`. The click is `element.click()`, which the page sees as untrusted.
- fill: `{"done":true,"took":bool,"why"?:S,"now"?:S,"over_max_length"?:n}`.
  - Text fields get the native value setter plus `input` and `change`. Editable regions get `insertText`, replacing what's there.
  - The value is read back after 100 ms: `took`, or why not, and what it is now (one line, at most 200 characters).
  - Enter or submit is never added.
- navigate: `{"done":true,"path":P,"document":D}`, once the page has loaded (10 s at most), with the reader in it. Another origin on the way ends the session (`origin_changed`).
- `{"error":"outcome_unknown"}`: it was sent, and the page didn't answer in 3 s (a dialog its handler opened, say). It may or may not have happened, and it's never retried.

## What the worker checks before every command

1. The session is bound and not paused, and within 100 commands and 2 MiB.
2. The tab still exists and is on the session's origin; otherwise it ends with `tab_closed` or `origin_changed`.
3. Which document is in the tab, asked of Chrome within 3 s. No answer pauses the session (`dialog_open`).
4. A different document: a read puts the reader into it afresh, and anything else gets `stale_document`.
5. The page's answer, within 3 s.

Nothing found after the session ended goes out.
