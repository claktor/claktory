# Friend APK: exact-artifact gate

Owner: Doppio. This is the gate for **the unmodified, signed APK**, not a
Flutter integration-test build. `foreground_test.dart` is a separate developer
suite whose complete native green run is still owed; it cannot certify these
APK bytes. **101502 forced-local-relay
and retention gate PASS, 2026-11-22.** This supersedes the known relay-only
first-pairing defect in 101501. Earlier 500/501 results remain separately
qualified below. Physical phone, actual VPS/public-relay network and
Doze/next-day behavior are not closed by this emulator gate.
A PASS requires the named UI states and backend evidence below, not screenshots
alone. Pending, failed and unexercised rows remain explicit.

## Identity and boundaries

- Production application ID `com.claktory.app`, versionName `0.1.15`,
  **versionCode `101502`**, Flutter stamp **v0.1.15**. No debug signing or debuggable application.
- Certificate SHA-256:
  `a4cf34938b823d9017eb01fe8b2e0978f481ec931dfd4fbfa1d705ddc34dd41e`.
  One signer, APK Signature Scheme v2 verified. Risotto supplies the final APK
  SHA-256 and source main commit after his landing. Do not reuse a trial APK.
- Risotto's native artifact verifier is a separate required report (both ABIs,
  ELF/JNI/16 KiB/ZIP alignment). This UI gate does not replace it.
- Only the agreed serial android36 slot. No other emulator/device is touched.
  ADB is a **test driver**, never the installation route. Do not run `adb install`,
  `flutter test`, `flutter run`, rebuild, re-sign or patch the friend artifact.
  Keep developer instrumentation runs separate: this Flutter version removed
  installed Claktory packages during its dev test install/cleanup. Finish and
  record the exact-artifact gate first; re-download through the browser if an
  installed friend APK is needed after a developer run.
- QA: `scripts/qa.sh --rev c36914a5 …`, the **v0.1.15 release source**. Scripted
  QA model only, fresh synthetic agent. Public relays are prohibited there.
- **Current relay ruling:** qa.sh sandbox, pinned local iroh-relay, code without
  `a=`, and the exact app UID's IPv4/IPv6 UDP dropped. Use the sandbox's normal
  first-device Allow after comparing its number. The executed 101502 recipe
  and results are below. No standalone home or public backend relay.
- Do not put Matt in the Allow loop or bypass device policy. The older dev 7421
  proposal in historical step 6 was superseded by the local-relay ruling.
  Never real Claktory, port 7420, real app/data/supervisor.
- No code, credentials, account seed, tunnel URL or whole UI hierarchy in logs,
  screenshots, git or reports. The helper keeps XML in memory; its Android
  shell-private temporary dump is removed in a trap. Private code files live
  only in this checkout's ignored build directory, mode 0600, then are deleted.

The default QA cache at c36914a5 reports runtime **dev**, which is not the
version-compatibility proof. Current `scripts/qa.sh` deliberately unsets
FACTORY_VERSION. For a stamped run, first create an **owned** cache with
`CLAKTORY_QA_ROOT="$PWD/app/build/android-friend/qa-v015" scripts/qa.sh --rev c36914a5 up android-friend-v015 --model --no-account`.
Then export c36914a5 using `git archive | tar -xm` (fresh mtimes) into this
checkout's ignored build directory, build factoryd there with
`FACTORY_VERSION=v0.1.15 --message-format=json` and that owned
cache's target directory, and atomically replace **only the owned cache's**
bin/c36914a5c8/factoryd. Restart only android-friend-v015 through qa.sh.
If reusing build outputs, require every exported workspace compiler-artifact
to have **fresh:false** in Cargo JSON (including factory-agent, factory-protocol
and factoryd), or use an empty target directory. Cleaning only factory-protocol
is insufficient: archived commit-time mtimes can silently reuse another
commit's workspace rlib. Never infer source provenance from a runtime stamp.
Keep at least 15 GB free, never modify the shared QA cache, and remove the
owned build outputs after the run. Require `workspace … version` = **v0.1.15**.

## Helper and staging

Run from the checkout root, with the approved existing SDK/JDK (no installs):

```sh
export JAVA_HOME="$HOME/Library/Java/jdk-21.0.13.1+2/Contents/Home"
export ANDROID_HOME="$HOME/Library/Android/sdk"
G=app/integration_test/android/friend_apk_gate.py
A=$ANDROID_HOME/platform-tools/adb
python3 "$G" self-test
python3 "$G" --stage artifact artifact \
  --url "$STAGING_URL" --apk app/build/android-friend/friend.apk \
  --sha256 "$APK_SHA256" --source-commit "$SOURCE_MAIN_COMMIT" \
  --apksigner "$ANDROID_HOME/build-tools/36.0.0/apksigner" \
  --aapt2 "$ANDROID_HOME/build-tools/36.0.0/aapt2"
```

Pericolo/Risotto supply a staging download URL for these same signed bytes.
Before the public link is published this may be a temporary local HTTP server
in this checkout serving **only** the APK (browser URL `http://10.0.2.2:<port>/…`,
host download URL `http://127.0.0.1:<port>/…`). No codes or evidence in that
server's document root. Stop it afterward. A staging transfer is not evidence
that the eventual public HTTPS link works; repeat download/hash after publish.
The helper only downloads/verifies; it never installs.

### 1. Browser → download → unknown-app consent → installer

Use a clean install of **prod**, not the dev package. If prod already exists,
remove it only after Risotto hands over the slot and confirms its test data is
disposable. Record browser/package-installer versions and whether the source's
unknown-app permission was initially off. Do not reset global device settings.

```sh
python3 "$G" --adb "$A" --stage browser open-download --url "$DEVICE_STAGING_URL"
```

Drive the browser's download and open the downloaded file **in the browser or
Files**, not via shell `pm install`. The real package-installer must block the
source until **Allow from this source** is explicitly enabled. Assert the
actual Settings activity and switch, return to the same download, then Install
and Open. Use `ui --expect … --tap …` for the image's real labels, or manually
perform those consent steps and record the package/activity/visible control.
Do not pretend an ADB permission grant exercised consent. Browser first-run,
Play Protect or vendor prompts are real UI to record, not to bypass.

```sh
python3 "$G" --adb "$A" --stage installed installed --sha256 "$APK_SHA256"
```

This hashes the installed base APK and requires equality to the downloaded
signed file. An installed package/version alone would not prove the artifact.
Record that no USB, ADB or developer mode was required by **the install flow**;
the AVD's attached test driver itself is not proof of a friend's USB-free phone.

### 2. Fresh paste-first pairing and refusals

```sh
python3 "$G" --adb "$A" --stage fresh ui --app-foreground \
  --expect 'Paste code' --absent 'Scan code' --absent 'Scan code instead'
python3 "$G" --adb "$A" --stage instructions ui --contains --expect \
  "Paste the code for this phone. Find it in Settings → Devices → Add a device on a device that's already connected, or where you set up your Claktory."
python3 "$G" --adb "$A" --stage wrong-input type --value not-a-code
python3 "$G" --adb "$A" --stage wrong-refusal ui --contains --expect \
  "That isn't a Claktory code. Copy it again, all of it, and paste it here."
python3 "$G" --adb "$A" --stage old-input type --old-code
python3 "$G" --adb "$A" --stage old-refusal ui --contains --expect \
  'This code is from an older Claktory. Update it where it runs, then add this phone again.'
```

Record Connect disabled for both refusals (Android semantic `enabled=false`).
No connection is submitted for either synthetic input. No scanner, dead Scan
control, camera request or iPhone/Mac-specific pairing instruction is allowed.

### 3. Direct v0.1.15 backend: real pairing and first conversation

```sh
export CLAKTORY_QA_ROOT="$PWD/app/build/android-friend/qa-v015"
Q='scripts/qa.sh --rev c36914a5'
S=android-friend-v015
# Prepare/stamp the sandbox as described above before continuing.
$Q send "$S" UpdateSettings '{"user_name":"QA","name":"Friend QA"}'
$Q send "$S" CreateAgent '{"name":"Friend QA agent"}'
$Q workspace "$S" version
$Q workspace "$S" remote.endpoint_id
# Save cli pair --json's code PRIVATELY, never echo it:
# $Q cli "$S" pair 'Android friend gate' --no-wait --json
python3 "$G" --adb "$A" --stage qa-code type --code-file "$PRIVATE_CODE_FILE" \
  --endpoint "$QA_ENDPOINT_ID" --backend-port "$QA_PORT"
```

The code helper checks an independently supplied backend endpoint. It changes
only the display-name hint to ASCII, retains authentication/relay/direct hints,
and quotes shell input safely. It never types percent escapes. This is not
proof of native clipboard paste or Unicode-name input; enter those manually
on the final phone if they matter. Keyboard input into **Paste code** remains
the real app flow, without modifying the APK or faking a channel.

Hide only the keyboard with one OS Back if it covers Connect; then activate
Connect through its named UI control. Assert **Waiting for <sandbox machine>**
(`qa-android-friend-v015` in this run), the four digits, the correct pending
device in the **sandbox** backend, and compare
its number. Use only that sandbox's normal first-device Allow; if policy says
ask the person, stop rather than bypass it. Assert connected status and device
kind **PHONE**, never Mac or Unspecified.

Always open **Menu**, assert drawer route/Search/Close menu, search for the
synthetic agent, and activate the row **inside the drawer**, never the
conversation header (which opens Agent settings). Assert the conversation's
**Message Friend QA agent** field, not just a matching agent name.
Type a unique one-line ASCII nonce such as `android-friend-<run-id>`, activate
**Send**, then assert visible assistant reply **`OK: android-friend-<run-id>`**
and backend transcript evidence: exactly one new user entry with that nonce,
a later assistant entry, its `replied_to` equal to the user's seq. Record those
synthetic entries via `transcript "$S" 'Friend QA agent' --last 10`.
An echoed user bubble or thinking entry is not a reply.

### 4. OS Back / actual IME / unavailable actions

- Focus the named composer, type a synthetic draft, wait for an **actual
  Android IME**. Record keyboard/app bounds; the message box and Send target
  must be visible above the keyboard. No fake insets or screenshot-only pass.
- Send **one** `back` while IME is up: keyboard hides; same conversation and
  draft remain. Never also close the page. Draft is not sent.
- Open drawer with keyboard down; one `back` closes it and leaves prod Claktory
  resumed. Repeat with Extensions sheet (or an actual pushed page). At bare
  conversation root Back belongs to Android and may background it.
- With empty input, activate **Record a voice message**: assert
  **You can't record voice messages on this phone yet.** No recording, silent
  failure or **Download the speech model?** prompt. No native recorder claimed.
- Activate **Attach files**: assert **You can't attach files from this phone
  yet.** No silent desktop fallback or false picker success.
- **No updater:** observe connected conversation and Settings: no desktop
  Update/Install path. An absent banner with no newer release advertised is
  only a smoke check. Record whether a newer `latest_release` was actually
  advertised; mark the adverse-condition row **NOT EXERCISED** if not. The
  shared owner tests of the platform gate remain separate evidence. Never
  install a backend/Mac update just to test this, or hit a real release feed.

### 5. Same-artifact force-stop and relaunch

Record the current synthetic conversation/endpoint and approved device id.
Do **not** clear data, reinstall, run an instrumentation APK, supply a new
code, or re-allow the device:

```sh
python3 "$G" --adb "$A" --stage process-death relaunch
python3 "$G" --adb "$A" --stage stored-connection ui --app-foreground \
  --expect 'Message Friend QA agent' --absent 'Paste code'
python3 "$G" --adb "$A" --stage same-artifact installed --sha256 "$APK_SHA256"
```

Wait for actual connected status, same selected conversation and previous
reply, then send another nonce and verify a second backend reply. Backend
must show the **same** approved device, with no fresh pending device. Run-as
cannot inspect the non-debuggable release's private files; do not weaken it.
This proves real native storage via behavior, not encryption/backup security.

### 6. Historical dev-relay proposal — superseded, not executed

**Do not use this recipe for the current gate.** The final ruling uses an
isolated qa.sh backend and local relay; see the 101502 executed section below.
This older proposal is retained only to explain the earlier owed matrix row.

The earlier proposal split direct v0.1.15 proof from owner-reported dev 7421
relay proof (dev was v0.1.16-dev, not v0.1.15). Additional dev approval would
have required Matt, so that row was carried/owed rather than bypassing policy.
101502 instead proves v0.1.15 over the isolated local relay on the exact APK.
The actual friend's VPS/public-network path remains unexercised.

## Evidence and decision

`friend_apk_gate.py` appends non-sensitive PASS events to
`app/build/android-friend/evidence/events.jsonl`. A failed command is never a
PASS; **the helper is not an overall pass/fail orchestrator**. Assemble the
complete matrix before calling the friend gate green:

| Row | Required evidence | Executed status |
|---|---|---|
| Artifact | downloaded/installed hash equal; cert; version101502 and actual Flutter stamp; owner native verifier/source | PASS 101502 |
| Install | browser→download→unknown-source consent→installer→Open, package/activity/control assertions | PASS 500 consent; PASS 501 real Update |
| Pair UI | Paste/instructions/no Scan; wrong+old refusals, Connect disabled | CARRIED 500 functional |
| v0.1.15 transport | proven backend/source, number, PHONE, correct conversation/roundtrips | PASS 502 fresh forced-local-relay pairing |
| Back/IME | real keyboard bounds, draft/page preserved; drawer+sheet close only | CARRIED 500 functional |
| Honest unavailable | mic text/no download; attachment text | CARRIED 500 functional |
| Updater | no desktop offer; adverse condition must run on numbered app, not dev | PASS 101501 replay; initial 101500 claim INVALID |
| Relaunch | same installed bytes, native stored endpoint/device/conversation, second reply | PASS 502 with UDP still blocked |
| Relay | exactly 1 initial tunnel/registration, no duplicates/warnings, indirect path, seconds after Allow | PASS 502 on local relay; actual VPS/public network OWED |
| Public URL | published HTTPS download hashes to same signed artifact | PASS 502 file download/hash and final page identity; Pericolo independently re-downloaded final deploy twice |

### Not provable from this AVD alone

- The friend's USB-free install on his actual browser/vendor package-installer,
  vendor warnings/Play Protect/network policy, device ABI/min API compatibility.
- Physical phone networking to his actual VPS (especially v0.1.15 over relay),
  cellular/Wi-Fi transitions and next-day resume, OEM process/power restrictions.
- Physical keyboard/IME variants, live TalkBack/Switch Access, nonlinear text
  scaling, predictive gesture cancellation, physical 4/16KiB device acceptance.
- Keystore hardware protection/backup/transfer/security recovery, hostile UID,
  release signing-key custody. Native/Security/QA gates own these.

Report emulator-only results explicitly. No notifications, voice recording,
scanner, OAuth, picker, custom links, tablets, foldables or non-English support
are promised in this friend build. Cleanup only this sandbox with
`$Q down "$S"`, delete private code files, stop staging server, and
hand the serial emulator slot back to Risotto. Never `down --all`.


## Initial 101500 execution — 2026-11-21 (functional results only)

**Functional emulator pilot PASS, not a version-compatibility pass**, for SHA-256
`e5bfc13095312dedf9632f634e1e207fc510055f53e94860b169fdc638800d09`,
Risotto's source main `1ac12fd7`, versionCode 101500, expected full signer above.
android36, API36, 4KiB pages, 1080×1920 at420dpi; Chrome133.0.6943.137 and
Google Package Installer. Owner native artifact verifier passed separately.

- Downloaded through a temporary emulator-host HTTP staging link, not a
  published URL. Chrome's harmful-file warning, unknown-source block, real
  Settings consent (previously off), Installer Install/Open all reached and
  asserted by Android UI nodes. Downloaded and installed APK hashes match;
  installed bytes rechecked after force-stop and at run end.
- Paste instructions/no Scan and wrong/old refusals passed, Connect disabled.
  Android exports the field label as an accessibility **hint**; the helper
  matches that as well as text/content-desc. This was a harness correction,
  not a product defect. End/Delete replaces unreliable injected Ctrl+A for
  clearing text. No credentials or UI dumps persisted in tracked evidence.
- Initial backend reported **runtime v0.1.15**, own cache, port 7760, direct
  tunnel. Its reused-target workspace provenance was incomplete (only protocol
  cleaned). This is superseded by the proven fresh-mtime/Cargo-JSON backend
  below. Also, 101500's Flutter stamp was **dev**, despite manifest 0.1.15: its
  version-compatibility claim is INVALID. No shared cache was altered.
- Real number/Allow, one approved device, no fresh pending device after
  force-stop. Backend user entries report DEVICE_KIND_PHONE. Two synthetic
  nonces each produced exactly one user entry and one later exact QA assistant
  reply with the correct replied_to, including the seq 0 presence case.
- Drawer Search reached; its actual agent row opened the conversation, not
  Agent settings. OS Back closed drawer and actual named Extensions sheet
  independently without leaving prod Claktory.
- Actual IME confirmed by input-method/window service, visible IME top1017px;
  editable and Send bottom1006px. Single OS Back hid only IME, preserving the
  draft and foreground conversation. Dumpsys was checked after transition,
  not immediately before Android completed the hide animation.
- Mic and paperclip produced their exact unavailable explanations; no model
  download prompt. Root OS Back backgrounded normally; same process resumed
  at the conversation. Separately, actual am force-stop/am start retained the
  native connection, prior reply and same approved device; second send worked.
- Also exercised a real scripted ask_person question card: title/body reached,
  explicit Approve produced exactly one backend ASK_PERSON_STATE_ANSWERED.
- Initial adverse-updater claim is **INVALID**: while the sandbox advertised
  latest_release v0.1.16/latest_backend v0.1.15, app **dev** outranks releases
  and could mask a broken platform gate. No Update was seen, but the corrected
  numbered 101501 must replay this condition. No installer was invoked.
- Relay: **carried owner-reported Risotto A.1 evidence only**. No Matt request,
  no dev allow bypass, no claim of real relay on this exact APK. Physical-phone,
  friend's actual VPS/next-day, vendor installer and public URL remain owed.

Non-sensitive backing is durably archived outside git/build in **Doppio's
agent folder: `evidence/friend-apk-20261121/README.md` and
`evidence.json.gz.b64`**. It contains 17 files and125 events; decoded JSON
SHA-256 `49444dd2e3ddbe284e1c6fad52d04d1538caf60129c1d2a067b767a6d2512039`.
Read-back equality and credential/path/endpoint redaction were checked. The
archive explicitly qualifies superseded claims. `adverse-101501.json` beside
it records the later corrected updater replay (outside the125-event snapshot). All own sandboxes were ended by name; private
pairing inputs and owned stamped target/source caches were removed. The small
proven QA binary cache remains available for a later developer regression run. No user-visible
product change was made by this test package, so no changelog line is needed.


## Corrected 101501 reduced re-gate — PASS

Bruno approved a reduced gate after correcting the Dart stamp. Candidate
SHA-256 **11b1a1ea9782db500d9e5022eedfd172447d97e8e0a8a895a78d10bec1c1fb8c**,
source main 1ac12fd717 (owner-reported), same certificate and production package,
manifest 0.1.15/versionCode 101501. Helper now defaults to 101502; use explicit
`--version-code 101500` or `--version-code 101501` for historical artifacts.

- Restored 101500 from the actual public HTTPS link via Chrome and Package
  Installer; installed hash matched. Paired to a fresh isolated upgrade QA,
  sent a nonce and received its exact scripted reply. The old footer was
  explicitly observed as **Claktory v0.1.15 · app dev** (not inferred from
  manifest). Saved the approved device id/paired time and transcript.
- Downloaded 101501 from Pericolo's public HTTPS URL; verified hash, full cert,
  v2, package/version. Browser → downloaded file → actual Installer **Update**
  → Open, not adb installation. No data clear, new code or Allow.
- Installed bytes match the public501 hash. Prior conversation and reply
  remained. Actual footer now says **Claktory v0.1.15**, with no app-dev suffix:
  the shared footer collapses the line only when backend and Flutter app
  versions match. This proves the app stamp independently of Android metadata.
- Proven backend: fresh `git archive c36914a5 | tar -xm` extract; Cargo JSON
  checked all **6 workspace artifacts fresh:false**, including factory-agent,
  factory-protocol and factoryd. Binary SHA-256
  **44575d3c9b0f52f34a8e0fac7f175df4864d0fabd15ca5020087dca15c3cc2f7**.
  The proven binary replaced only the owned cache's backend, then only upgrade
  QA restarted. Runtime **v0.1.15**, source c36914a5c8, port 7758.
- Against that proven backend: same-APK force-stop/am start again reached the
  correct conversation, no Paste/version-blocking step. A new nonce produced
  one user entry and one exact later assistant reply with correct replied_to
  and PHONE attribution. Same device id and paired_ms, connected, zero pending
  devices. Numbered app-v0.1.15 ↔ backend-v0.1.15 acceptance therefore passed.
- Pericolo's final page points at101501 and its SHA; public file is served as
  application/vnd.android.package-archive,69,917,167 bytes. He independently
  re-downloaded the final deployment twice; the old101500 URL was removed and
  purged. This gate independently downloaded and hashed501 as well.
- **Adverse updater replay PASS on actual numbered501:** proven backend
  runtime v0.1.15 advertised latest_release v0.1.16/latest_backend v0.1.15 via
  the local non-installable fixture. Actual connected prod conversation and
  Connected to Upgrade QA were reached; Update/Install update/Update now
  controls were absent. This supersedes, rather than carries, the invalid
  app-dev adverse check. The small replay got its own serial emulator slot;
  Risotto's debug property/reverse settings were left untouched.
- Earlier full UI/Back/IME/question/consent rows are **carried 101500 functional
  evidence**, not relabelled as fresh501 runs. This reduced run is the explicit
  approved scope; it does not prove incompatible-version refusal, actual VPS
  relay, physical hardware or next-day behavior.


## 101502 forced-local-relay gate — PASS, 2026-11-22

**Exact APK:** SHA-256
`6650c837a2624dd6933852d6c6758a061793190d1df9820cd93ce424bc660d6b`,
70,392,303 bytes, source main **6c8291b55c** (owner-reported, connection fix plus
logcat routing), same certificate/v2, prod versionCode 101502, actual Flutter
footer **Claktory v0.1.15**. Public HTTPS download and installed base APK hashes
matched before and after the gate. Browser download → Package Installer
**Update** → Open, never adb installation/re-signing. Own disposable app data
was then cleared deliberately to test a **fresh first pairing**, not just an
already-approved connection. No 501→502 data-upgrade claim is made by this run.

### Fixture and enforcement

- Owned qa.sh sandbox `android-relay-502`, free port 7799, account disabled,
  synthetic QA model only. Runtime v0.1.15, proven c36914a5c8 factoryd44575d3c
  and its fresh-mtime/Cargo-JSON provenance above. No standalone home, public
  backend relay, real provider key or Matt approval.
- Reused the already-built pinned iroh-relay 1.5.0 binary (no install/build),
  SHA-256 `1d03a3099ed901581dd3b3dd6351fa1306a9a7d23dc3c6e9b22e37c1276fd2a0`.
  Owned process started with `RUST_LOG=iroh_relay=trace … --dev`, port 3340.
  Backend SetRemoteAccess used on:true, set_relays:true and
  `http://127.0.0.1:3340/`; code retained only that relay, e and t, with **no a=**.
- After verifying this is the agreed emulator and reverse/property baselines
  were empty: adb root, find **exact prod** UID 10225 (not prefix-matched dev),
  insert owner-scoped OUTPUT UDP DROP with iptables **and** ip6tables. Each
  exact rule was checked with -C. Add TCP3340 reverse **after** root; set
  debug.claktory.tunnel=debug before the fresh app process. No whole-chain
  flush, network-wide rule, or fake platform callback/inset.

```sh
# S=android-relay-502, Q points at the owned proven-v015 cache.
$Q send "$S" SetRemoteAccess \
  '{"on":true,"relays":["http://127.0.0.1:3340/"],"set_relays":true}'
# Wait for remote.relays and endpoint id before making the private code.
# Confirm no pre-existing matching rules/reverses, then:
adb -s emulator-5554 root
adb -s emulator-5554 wait-for-device
# Resolve exactly com.claktory.app's UID, not com.claktory.app.dev's.
adb -s emulator-5554 shell iptables -I OUTPUT -m owner --uid-owner "$U" -p udp -j DROP
adb -s emulator-5554 shell ip6tables -I OUTPUT -m owner --uid-owner "$U" -p udp -j DROP
adb -s emulator-5554 reverse tcp:3340 tcp:3340
adb -s emulator-5554 shell setprop debug.claktory.tunnel debug
# type --code-file ... --endpoint ... --backend-port ... --relay-only
```

### Observed first-pair result

Displayed number matched the fresh sandbox's pending device, then normal
first-device Allow. Connected was **observed within 4.151 seconds after Allow
ack**, a conservative upper bound including Android UI polling (Allow RPC
0.064s). Correct connected status and actual indirect-route explanation were
reached; backend path **DEVICE_PATH_RELAYED**. New first-session bug did not
recur.

Synthetic public app endpoint
`329e5460fc472f51b8a1c080a61cfa54e15b15247131e19368cde7391cf1d003`
correlates the evidence below. These are public throwaway ids, not secrets.
iroh-base 1.5.0 fmt_short is the first 5 bytes as lower hex, so relay alias is
`329e5460fc` (source verified, not guessed).

- **Exactly 1** backend `tunnel connection opened` before retention.
- **Exactly 1** relay `registering client remote_endpoint=329e5460fc`; zero
  `multiple connections found, deactivating old connection`/duplicate-client
  lines for that id. Match the whole word registering, not unregistering.
- Positive app logcat canary from the actual prod PID:
  `connecting relay_url=Some(RelayUrl("http://127.0.0.1:3340/")) ip_addresses=[]`.
  **Zero Another endpoint warnings**. Empty/unenabled logging would fail,
  rather than count as zero. No global logcat history was cleared.

### Retention and cleanup

Under the same UDP DROP, one exact nonce user/assistant roundtrip worked,
then actual force-stop/am start retained the selected conversation, prior
reply, same approved device/paired_ms and zero pending devices. A second nonce
worked; both replies had correct replied_to (including seq 0), PHONE attribution
and still-RELAYED backend path. Re-launched prod PID had a new positive canary
and zero warnings. **Two total** relay registrations/backend tunnels across
the two processes are expected; zero duplicate clients across both. The
exactly-one requirement refers to the initial pairing, not the entire run.

Only both owned UID rules were deleted; -C returned absent for each while
still root. Only tcp3340 reverse removed, tracing property cleared, prod
force-stopped to reset its logger, adbd unrooted and uid2000 verified. Owned QA
and relay stopped, private code/raw logs removed. Emulator returned to Risotto
for #214; no global reverse flush or other agent's rule/process touched.

Physical-tap helper notes: a tap event means target-found/input-issued, not
callback success. Close menu's full-screen semantics bounds cover the drawer,
so its centre is occluded; the gate used actual **OS Back** and asserted the
resulting conversation rather than pretending a centre tap dismissed it.

Durable outside-build backing: **Doppio agent folder,
`evidence/friend-apk-20261122-relay502/README.md` + `evidence.json.gz.b64`**.
Seven backing files include actual sanitized registration/connect log lines,
counts, latency, pre/post device state, transcript and backend provenance.
Decoded JSON SHA-256
`56c94277827a3fc2dae771841f5ea8857a95e288ae0ad2b59548915c611b0d4b`;
read-back byte equality verified. No code/token/seed/capability URL/home path
archived. The synthetic public endpoint id is deliberately retained for log
correlation; private client ids and addresses are redacted.

Result handed directly to **Pericolo**, the named next agent. He confirmed
publication in Ops commit f01d10f: the friend page
`https://claktory.com/android-kyckk2srtq` points to 101502, and two final-deploy
downloads (including the CDN-cached copy) match the expected SHA. The 101501
file returns 404; its CDN copies and the old page were purged (owner-reported).
This gate independently fetched the final page and asserted its 101502
Download URL, hash and size, with no 101501 reference. The exact public 101502
file had already been independently downloaded/hashed for the installed gate.

This proves forced **local** relay on an emulator, not a physical phone's
actual VPS/public relay or next-day/Doze behavior (#214 remains a separate
investigation).
