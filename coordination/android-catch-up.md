# Android catch-up — audit and acceptance

Owner: Doppio (recording); Prosciutto leads the delivery chain. Native/build:
Risotto. Publishing/release integration: Pericolo. Updated 2026-11-22.

## Decision needed from the audit

At audit start, the friend page served **0.1.15 (101502)**, not current Android main.
Do not treat a source feature as present in that APK. The immediate gate is
**connects to v0.1.17 and main, scans a real camera QR, and says Claktory**;
other parity work is a separate slice. No production push registrations.

## Connection — audit result: code works; old QR/link entry is missing

- Published APK: release package `com.claktory.app`, 0.1.15 (101502),
  70,392,303 bytes. Copied as an individual artifact and matched to the public
  page's checksum. Risotto identifies it as release rebuild 2, built near
  c267bf3f; that is approximate source provenance, not a reconstructed exact
  source stamp. The release signer matches the original friend-build signer.
- v0.1.17 backend: **3fc0e6f9**, confirmed by Pericolo as the source of the
  shipped backend, Mac release and TestFlight build. Current-main audit pin:
  **ab691fc0**. The final signed-APK gate refreshed main to **74310c56**.
  Separate guarded qa.sh sandboxes, scripted model, no real account.
- **No version/protocol/tunnel/relay incompatibility was reproduced.** The
  unmodified published APK actually paired by code, matched the real approval
  number and exchanged exact visible nonce/replies with backend `replied_to`,
  direct and forced local relay, against both source pins. The first fixtures
  reported `dev`; after Mista added guarded `qa.sh up --version v0.1.17`
  (d212b48a), both old-APK routes were repeated against the exact release
  source reporting **v0.1.17**. No response was patched or guard bypassed.
- **The missing entry path is reproduced:** a real ACTION_VIEW of a valid
  `factory://` QA code could not resolve any activity in 101502. That APK also
  cannot scan a camera QR. Typed code works; tapping/scanning cannot. This is
  a concrete explanation for a failed link/QR attempt, not proof of what
  happened on Matt's particular phone or public-network server.
- A harness initially expected today's fingerprint dialog in the old APK,
  which predates it; the actual old UI had already reached number approval.
  That was a harness mismatch, not a connection failure. Each disposable case
  used normal matching-number first-device approval; earlier case devices
  were normally unpaired before reusing a sandbox.

| Installed APK | Backend | Code + direct reply | Forced-local-relay reply |
|---|---|---|---|
| Published 101502 | v0.1.17 / 3fc0e6f9, runtime stamped | PASS | PASS |
| Published 101502 | main / ab691fc0, runtime dev | PASS | PASS |
| Replacement 101701 | v0.1.15, v0.1.17 and gate-main | PASS, fresh QR and code | PASS, fresh QR and code; install-over-old also kept its approved pairing on v0.1.17 and audit-main |

## What still says “factory”

| Surface | Published / main finding | Fix owner / action |
|---|---|---|
| Download page and link | Title is Claktory; URL is `claktory.com/android-kyckk2srtq`; file is `claktory-0.1.15-101502.apk`. These already use the brand. At audit start it advertised the old build. | Pericolo: replace APK, version, size and checksum on the same link after acceptance. |
| Raw pairing code | The backend still writes legacy `factory://` codes; this is the identified candidate for Matt's “factory” link, not the download URL. The outer `claktory://add?code=…` link already exists. | Prosciutto's compatibility plan: Polnareff's additive reader d6669fbb accepts both schemes on scan/paste/link paths; it rides the next release, **not this APK's critical path**. Backend emission waits for all released readers to accept it. Keep saved pairings and old codes; do not add a duplicate native `claktory` filter (release already registers it). |
| Onboarding / Add a Claktory / waiting for approval | Shared Dart includes literal `factoryd pair` / `factoryd allow` command guidance (`connect_step.dart`, `devices.dart`, `pairing.dart`). The web page also suggests `factoryd status`. | Shared owner / Pericolo: prefer app instructions in user-facing guidance; retain the correct internal command names wherever a command is actually shown. |
| Android-policy UI | No user-facing Factory name found in `app/lib/android`; matches there are internal channels/types or documentation. Shared app title is Claktory. | Doppio: no Android-policy rename needed on current inspection. |
| Launcher, native links, notification labels, share sheet | Risotto found no other native Factory branding: label, APK name and share sheet say Claktory. Android has no push notification UI. Published 101502 has no LinkActivity; actual ACTION_VIEW confirmed **no handler for its pairing link**. | Risotto: proposed native fix 45498a2e accepts legacy `factory://` camera links through the confirmation path; keep existing ids/channels. |

## User-visible parity: main is not the published APK

| Capability | Current iPhone | Android main | Published 101502 |
|---|---|---|---|
| QR pairing | Native scanner is wired to shared confirmation. | Present and exercised on signed 101701: actual emulator camera image → decoded QR → confirmation, approval and reply, on all three tested backend versions. Physical-phone camera remains owed. | No CAMERA permission: camera scanning cannot work in this APK. |
| People preview | Shared phone People UI, shown when the backend offers the DM preview. | The same shared People UI has no Android exclusion; native DM key-store work is on main. Full current Android preview E2E is not implied by its native store tests. | Absent from the near-build source and old AOT text inventory; not a claim of a reached preview surface. |
| Push / background notifications | iPhone push registration is wired. | `main.dart` still creates the registrar only for iOS; Android C1 is parked and is **not** part of this catch-up fix. | Not available; no notification permission in the artifact. |
| Voice messages | Native recorder/playback. | Native recorder, picker and playback adapters exist; bounded foreground emulator paths were previously exercised. Physical audio remains owed. | No RECORD_AUDIO permission: microphone recording cannot work. |
| Staying current | Release process ships matching Apple app. | Android source has advanced, but the friend link has not followed it. | Still 0.1.15 while the person's backend is v0.1.17. |

## Native findings and immediate fixes

Risotto confirms 101502 predates native QR commit **7e4a13e2**: no CAMERA,
RECORD_AUDIO or LinkActivity in the published manifest. Main has the scanner,
but previous emulator tests only proved camera frames/lifecycle, **not a read
of an actual QR**. Native fix **45498a2e** enables ZXing TRY_HARDER (a tilted
two-pixels-per-module fixture now decodes), accepts legacy camera links, and
runs lint before a release APK build. Owner-reported device checks passed:
WebLinks 4/4, QrScanner 2/2. Follow-up **24e23e58** raises 4:3 analysis to
1280×960 instead of falling to 640×480; 16:9 remains 1280×720. Risotto
built release-signed **0.1.17 (101701)** from that fix; 101700 is superseded
and is not gated. Doppio independently verified the release signer, version,
label, permissions and both ABIs. An actual install over 101502, without
uninstalling or clearing data, preserved its approved pairing and linked relay
reply on both backends. The completed real camera-fed QR gates are below.

The additive branded-code parser **d6669fbb** is not held on this APK's path:
Prosciutto explicitly ruled that the backend must keep emitting `factory://`
until released apps on every platform accept both schemes. The raw code's
legacy prefix remains an acknowledged compatibility gap, not a claim that
this APK has renamed it. Native ids/channel names stay unchanged.

## Signed 101701 gate — PASS on the emulator

Artifact: **Claktory 0.1.17 (101701), source 24e23e58**, release signed with
the same signer as 101502; arm64 and x86_64, not debuggable. Actual drawer text
showed **“Claktory v0.1.15 · app v0.1.17”**, so the Flutter stamp, not only the
manifest, is the release version.

Every cell below is a fresh pairing, real fingerprint confirmation with no
connection before Connect, matching OS-visible number and normal approval,
named composer, exact visible assistant nonce/reply and backend `replied_to`.
QR cells used the actual Android camera API fed a host image, including the
normal OS permission prompt—not a fake scan/channel callback or pasted code.

| Backend source / actual version | Code, direct | Code, local relay | Camera QR, direct | Camera QR, local relay |
|---|---|---|---|---|
| c36914a5 / v0.1.15 | PASS | PASS | PASS | PASS |
| 3fc0e6f9 / v0.1.17 | PASS | PASS | PASS | PASS |
| 74310c56 / main (dev version) | PASS | PASS | PASS | PASS |

Each fresh cell had **one initial tunnel**. Relay cells had no direct hints,
exact test-UID IPv4/IPv6 UDP DROP and one owned TCP3340 reverse, verified
RELAYED path, **one app registration and zero duplicate/replaced clients** in
the completed pairing/reply window. Later foreground retries after deliberately
stopping the sandbox are recorded separately, not miscounted as initial pairing.

Also passed: two actual install-over-101502 checks (v0.1.17 and audit-main),
without uninstall/data clear, preserving the approved device and a new linked
relay reply; cold `factory://` and warm branded `claktory://add` links reaching
real confirmation, with Cancel leaving no pending device. **Oldest backend
tested: v0.1.15**, using Pericolo's confirmed shipped source and guarded stamp.

Camera fixture qualification: an oversized centered image was clipped and did
not decode. Smaller images positioned within the emulator's imagefile crop
passed. Risotto assesses the offset as emulator imagefile placement, not the
APK preview transform; this is an owner assessment, not a controlled same-size
placement A/B. A physical phone aimed at a Mac's QR remains unverified.

Synthetic results, selected redacted connection events and persisted linked
reply pairs were archived and read back in private evidence:
`evidence/android-catch-up-20261122/release-101701-gate.json`. No QR/code material
was archived. Exact UID rules/reverse were removed, original debug/adbd state
restored, test data cleared, all QA/relay processes and the emulator stopped.
This is not physical/OEM/public-network relay or full People/push/media parity.

## Delivery ownership

- Risotto: native fixes and signed versioned build **done**; lint gate included.
- Doppio: audit and signed-APK emulator gate **PASS**. No Android-policy code
  change was needed. Prosciutto owns the shared Android changelog entry.
- Pericolo: next, swap the APK/checksum on the existing link and integrate a
  matching Android artifact into every release; keep signing with Risotto until
  the approved vault-file-import handover.
- Zucchero: then independently download/install over old/pair by QR as a friend.
  Prosciutto reports the completed chain to Bruno. Publication/friend verification
  are not claimed complete by this emulator report.

Public page inspected: https://claktory.com/android-kyckk2srtq

Private artifact hashes, signing-key paths, endpoint ids, pairing material and
raw UI/logs are deliberately excluded from this report.
