# Android main parity audit

Owner: Doppio. The product-surface audit below is **not run yet**. Preliminary
native camera permission checks are recorded separately; they are not an
all-surfaces pass. Checks must not interrupt a Doze interval. Historical
friend-build results do not certify newer surfaces.

## Scope and evidence

Build the current main Flutter app and pin the isolated QA backend to the same
source revision with `qa.sh --rev`. Use scripted models and synthetic accounts
only. Never use the real Claktory, real providers or personal conversations.
Keep private artifact hashes, home paths, machine names, hardware/device ids,
keys, codes, capability URLs and raw UI/log output out of this public report.
Sanitized provenance and reached-surface backing belong in private QA evidence.

For each surface:

- Reach the actual named surface, exercise its action and independently check
  the resulting backend state. A screenshot or a widget-only test is not
  native acceptance.
- Test font scales **1.0 and 2.0**, including scrolling, selection, IME,
  unclipped actions, semantics and Android 48 dp touch targets.
- Complete one actual **TalkBack** pass. First check whether the approved test
  image includes it; do not install system software or silently substitute a
  semantics test.
- Exercise actual predictive-gesture and three-button Back. Cancellation must
  preserve state. One committed Back dismisses one layer: selection/IME,
  nested sheet, containing sheet, route. Never keyboard and page together.
- Restore only this test's font scale, navigation and accessibility changes.
  Send defects to each file's owner with a reproducible path, expected/actual
  behavior, source revision and sanitized evidence; send the list to Prosciutto.

## Matrix

Executable rows are **PENDING**, not passes or confirmed defects. The two
explicit exclusions below are **NOT REACHED**, not passes.

| Surface | Required reached/action assertions | Fixture status |
|---|---|---|
| History | Open History; reach decided and waiting items; open the corresponding card/message | QA history scripts identified; inspect/adapt safely to the owned sandbox |
| People and direct messages | Open People; reach a synthetic contact and DM; send and verify an exact reply | Current user-apps/account-worker recipe needed |
| Review bar | Open Approve and Dismiss sheets from a real review request; check the submitted decision/comment | Real review request plus shared review widget test identified |
| Menu app strip | Backend offers an app; reach its menu entry and open its actual surface | NOT REACHED by lead ruling: Apps are hidden until C6/shipping acceptance |
| Usage subscription details | Open a subscription row; reach limits, reset information and its actions | NOT REACHED: widget tests only, no sandbox recipe; audit the rest of Usage on device |
| Card comments (#289) | Reach comment controls on actual cards; submit and check the backend decision/comment | Existing synthetic question/proposal/secret/pipe cards can supply cases |
| Every encountered sheet | Nested overlays, IME and selection each consume only their own Back; predictive cancellation preserves state | Enumerate from the reached surfaces, not screenshots alone |

## Fixture preparation

QA has recipes for question/proposal/secret History cards, pipe History and
typed blocks. Inspect them before execution: adapt output, sandbox and cache
paths to the owned checkout; do not run a recipe that writes another checkout
or shared coordination files. Keep ordinary real pairing and matching-number
approval; do not substitute an approval bypass.

QA currently has no ready native fixture for subscription limits or peer DMs.
Record any unreached row plainly and obtain a safe recipe from its owner; never
use real sign-ins to turn an untested row green. Peer messaging may require
the isolated user-apps worker and account service, not merely a fake menu model.

Backend/frontend owners confirmed that the menu app strip has no Flutter build
flag: it requires `Workspace.apps_available` and at least one app with a view.
Apps are intentionally not shipped; the normal QA backend does not offer them,
and its driver refuses dev mode. Prosciutto and Bruno ruled this row
**NOT REACHED** until C6 proves Apps before they are shown. Do not use the dev
Claktory, add a QA seam, bypass the guard or change the shipped feature gate.
Subscription-limit details are likewise **widget tests only, no sandbox recipe**;
audit the rest of Usage on device. Do not install a Rust target or any other
system tooling for this audit.

## Preliminary native phase-2 checks — repaired main 2539daa1

Built the app and instrumentation APK from the repaired main revision, with
a guarded QA backend from the same revision. Installed app bytes were checked
against the build privately. This was a debug arm64 API-36 emulator check, not
a physical-phone or release certification.

| Path | Result | Reached evidence / limitation |
|---|---|---|
| First camera refusal | PASS | Actual Android permission dialog denied once; Scan code remained, Open Settings was absent, no backend pairing began |
| Permanent camera refusal | PASS | Actual second Android denial; Open Settings appeared and Scan code was absent |
| Camera Settings recovery | PASS | App's Open Settings reached real App info → Permissions → Camera; Allow while using selected; real Back returned to an Android texture preview and active Camera2 client |
| QR camera → Dart → pairing | NOT REACHED | Fresh emulator boot with the smaller host-validated image still produced no host-decodable QR in the actual preview and no confirmation; Camera2 preview was 1280×960, analysis 640×480 |
| Pasted image | PASS, bounded | Actual OS clipboard image → Paste image → copied attachment → Send; 671-byte PNG matched the backend byte for byte, with the exact reply linked to its message; same-UID fixture, not another app's URI grant |
| Photo Picker | PASS, bounded | Actual system Photo Picker selected that PNG; copied attachment and Send reached identical backend bytes and the linked reply |
| Document picker | PASS for small file; larger sends under investigation | Actual system document picker selected a 65-byte text file from Downloads; copy, attachment and Send matched backend bytes and linked reply. Larger binary/video sends failed as described below |
| Voice capture and audio playback | NOT REACHED yet | Actual mic action reported speech recognition not installed; this QA backend reports UNSUPPORTED and correctly never asks for the microphone. A separate main-build offline speech sandbox now reports READY, but its device path has not run; no READY state was faked |
| Video playback | NOT REACHED | Actual document picker copied video attachments, but sending them failed before the backend received the messages; the viewer has not been reached |
| Native links and Share | PENDING | Actual intent/chooser paths still to run |
| Connector webAuth UI path | NOT REACHED | Lead ruling: guarded QA refuses service connections and has no local fake OAuth catalog; unit/native-core tests are not UI E2E |

No pairing code was pasted and no `scanned` callback was injected in the camera
case. A fresh smaller-image retry also did not reach confirmation. The QR
result is **not a decoder defect report** until the imagefile fixture is
validated through the actual camera. Subsequent picker checks used the real
paste-code fallback, fingerprint confirmation, matching number approval and
an exact nonce/reply; that fallback is not QR success. One second-denial
selector was too narrow (Android uses a separate permanent-denial control),
and one UI dump during preview recreation failed; both were treated as harness
observations, not app success. In the first permission case, private code/image
files and temporary screenshots were deleted, the unused ticket revoked, and
QA/app/emulator stopped before the native owner's separate 16 KiB pairing
slot. The retry's private QR/code and temporary screenshots were also deleted.
No active Doze case was disturbed.

Sanitized detailed backing is kept in the agent's private evidence as
`evidence/android-phase2-20261122/camera-permission-partial.json` and
`native-partial.json`; private artifact checksums and identifiers are not in
this public report.

## Preparatory findings sent to owners

- **Camera first-refusal state:** the shared pairing screen assumed every
  false permission request meant permanent denial. Sent to the shared phone
  owner; fixed in `11f2d0ff` by reading access again. The actual first/second
  refusal and Settings-recovery checks above now pass on repaired main.
- **Microphone guidance:** a first Android refusal could show Mac-specific
  settings instructions, and permanent phone denial used iPhone-only wording.
  Sent to the shared voice/composer owners with UX writing; fixed in
  `d57c3abc`. Actual microphone permission-flow retest is still pending.

- **Attachment transport failure, isolated to Android GSO configuration:** actual video files
  of about 494 KiB and 5.6 KiB, and random binary files of 5.6 KiB and 400 KiB,
  copied into attachment chips but failed to send. A real hold on Not sent
  showed “Connection lost.” The backend received none of those messages;
  logs showed a dropped tunnel and reconnect, with no backend panic. The
  iPhone owner reports the original video and incompressible data passed
  over one tunnel. Random binary uploads of 1, 2, 3 and 4 KiB reached the
  backend byte for byte with linked replies; their visible-reply observers
  timed out with prior failed outbox messages occupying the bottom, so these
  are transport evidence, not extra all-path UI passes. The matched Android
  A/B below isolated the GSO setting. The proper-main fix and native rerun
  are recorded after that historical diagnostic.

## Android upload GSO diagnostic A/B

**Diagnostic result only; not a main-build/native-feature acceptance pass.**
The native owner supplied matching debug APKs from `53638636`: unmodified,
and the same source/toolchain with only the Android device endpoint's
segmentation offload disabled. Installed bytes matched their private artifact
receipts. Both used the same approved device, guarded QA backend `2539daa1`,
and byte-identical random fixtures through the real document picker, copied
attachment and Send. Both emulator interfaces stayed at MTU 1500 throughout.

| Build | Random file bytes | Result |
|---|---:|---|
| Unmodified control | 5,698 | FAIL: no backend message; tunnel drop 5.397 s after the actual Send tap |
| Unmodified control | 409,600 | FAIL: no backend message; tunnel drop 6.626 s after the actual Send tap |
| GSO-disabled diagnostic | 5,698 | PASS: byte-exact backend file, visible exact reply and matching `replied_to`; no send-interval drop |
| GSO-disabled diagnostic | 409,600 | PASS: byte-exact backend file, visible exact reply and matching `replied_to`; no send-interval drop |

A preceding repaired-main `2539daa1` replay also failed both exact sizes,
with drops after 8.477 s and 8.588 s. Earlier “5.6 KiB” video and matched
binary fixtures were **5,743 bytes**, not the exact 5,698-byte fixture newly
generated for this A/B. The distinction is preserved in the private backing.
An 8,192-character repeated text message reached the backend with its linked
scripted reply (the QA script replies with the first 200 characters); an
8,192-character random-letter message did not. Actual full field readbacks
were checked before sending. A prior incomplete key-entry attempt was not
sent or counted.

The lead selected **GSO off on Android only** from these results; the native
owner landed it as `cf8cc718`. No MTU change was needed or made. The proper-main
rerun is below; physical-phone Wi-Fi and cellular checks remain owed. This
does not close #214 or certify a true overnight resume.

Both diagnostic APKs were replaced with the original repaired-main app and
its installed bytes rechecked. Own clipboard/download fixtures and debug
forwards were cleared, the two old QA cases stopped, baseline settings and
non-root adb restored, and the emulator stopped before the native owner's
next hardware slot. The separately prepared offline speech QA remains up,
without an Android connection. Durable sanitized A/B backing is
`evidence/android-phase2-20261122/gso-diagnostic-ab.json`; no pairing material,
raw UI or private checksums are published here.

## Proper-main GSO-off native rerun — cf8cc718

Fresh app and guarded QA builds used the same main revision, with installed
APK bytes checked against the private build receipt. A fresh direct pairing
reached the real fingerprint/off-account confirmation, matching-number
approval and exact visible nonce/reply with the backend's `replied_to` link.
These are API-36 arm64 emulator checks, not release or physical-device gates.

| Path | Result | Reached evidence / boundary |
|---|---|---|
| 5,698-byte and 409,600-byte random uploads | PASS | Real document-picker Search → selection → copy → attachment chip → Send; exact backend bytes, visible exact replies and linked backend entries for both sizes |
| Microphone first refusal | PASS | Actual Android denial → retry guidance, no Open Settings |
| Permanent microphone refusal and recovery | PASS | Actual second denial → Android Settings guidance/Open Settings; real App info → Permissions → Microphone → Allow while using → Back to composer |
| Native recording and Cancel | PASS, bounded | Real mic tap and advancing recording timer; Cancel restored the composer with no new backend entry |
| Native recording and Send | PASS, bounded | Actual 19-second AAC recording, 16 kHz mono; all 94,009 bytes matched between the app's native code cache and backend upload. Silent emulator produced the real no-speech error, not a successful spoken transcription |
| Audio playback with a fixture recording | PASS, bounded | Known spoken PCM, padded with silence to 30 seconds, was uploaded through normal QA requests and transcribed to “Hello from the speech sandbox.” Actual Play/Pause/Resume/end states and byte-exact downloaded cache checked. Voice-message seeking is **not offered**, not a missed UI test; audible hardware output is not certified |
| Native video | PASS, with shared-route workaround | Real uploaded video, 320×240/12 seconds; Extensions → named video row reached the native viewer, then Play with advancing position, Pause, timeline seek, end at 0:12 and Back to composer |
| Cold and warm links | PASS | Actual dev-scheme ACTION_VIEW: cold fingerprint/warning → Cancel with no pending device; warm fingerprint → Connect → matching number approval → named agent and exact linked nonce/reply |
| Share | PASS, bounded | Actual pipe-invite card/Make invite/Share → Android chooser; Back preserved the card. A second chooser selected Messages and reached its real MultiShareActivity, then Back returned. No recipient or Send action, and no external delivery claim |

These new random fixtures are not the deleted A/B payloads; their own hashes
and source receipts are retained privately. A cache clear removed the earlier
ignored scripts/build outputs, but their sanitized durable backing and public
reports survived. The reusable `native_core_gate.py` now lives in the tracked
test tree. Its full prepare/two-upload run, refusal self-test, Python compile
check and `scripts/check-secrets.sh` all passed; these checks are not substitutes
for the actual OS paths above.

**Shared phone defect found:** tapping the sent video's chip added its pane
to the backend layout but did not push the viewer over the conversation.
The shared owner confirmed and fixed it in `bf2679c2`; the video check above
used the real Extensions row instead. An Android chip-tap retest on a new main
build is still pending, not claimed by that workaround. One viewer-header
assertion initially expected the filename alone rather than its combined
filename/details label; the corrected named header and controls were reached.

**Harness observations, not product failures:** the QA guard marker is a
10-character commit abbreviation, resolved back to the full pinned revision;
a first code-key-entry attempt needed canonical ASCII formatting for adb;
and a scripted pipe tool added mid-context needed `extension_call`. The Share
target's immediate activity check was too early, then its actual settled
Messages component was positively verified. No fake platform callbacks or
lifecycle success were used.

QR decode through the actual camera remains **NOT REACHED** from the earlier
fixture limitation. Connector webAuth remains **NOT REACHED** by the guarded
QA ruling. A successful spoken recording on a physical Android, audible
playback, OEM/network behavior, real Wi-Fi/cellular and true overnight resume
remain owed. The unanswered physical-device availability request is still
unknown, not “no device”.

The Share invite was closed/deleted through normal requests; own download
fixtures and disposable dev data were cleared, both QA cases stopped,
non-root adb/empty forwards and original global settings verified, and the
emulator stopped before the native owner's separate API-24 slot. Durable
sanitized backing is `evidence/android-phase2-20261122/postgso-main-native.json`.
Private identifiers, checksums, pairing material and raw UI are not published.

No History/People/review/Usage/card-comment surface has been audited yet.
These preliminary findings are not a clean bill of health for that matrix.
