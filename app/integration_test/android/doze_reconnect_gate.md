# Android reconnect after Doze — #214

Owner: Doppio. **D5 direct and forced-local-relay 5-minute cases PASS** on
one paired debug APK. Both main-build hour rows also pass (direct with a
timing note). #214 remains **open**: the direct simulated-next-day row
recovered on main c414bba7 but is **NOT PASS** because it opened three tunnels
after resume; relay is not run. Physical/OEM/actual-overnight acceptance is owed.
Earlier candidate failures remain recorded below, not relabeled as passes.
The signed friend APK gate is separate (`friend_apk_gate.md`).

## Candidate and ownership

Test Risotto's Rust endpoint rebuild and Pesci's Dart reopen **together**.
Record both source commits, installed APK checksum/package/version/Flutter
stamp, backend source/runtime, Android API/page size, device model, and whether
this is an emulator or physical phone. Keep private artifact/backing checksums
in private QA evidence, not this public-source report. Risotto owns the shared
emulator until he explicitly hands it over. Do not install or drive it during his slot.
Use an isolated qa.sh sandbox with a scripted QA agent; never port 7420, real
Claktory's data, a live model, or a new approval request to Matt.

For an emulator, screen-off plus `dumpsys deviceidle force-idle` is **forced
Doze**, not proof of natural OEM Doze or physical power management. Verify the
actual deep-idle state before starting the elapsed interval and before waking.
Do not count merely backgrounding the activity as Doze. An elapsed interval
interrupted by a host/emulator restart, app force-stop/reinstall, changing
credentials or an early foreground visit must be retried, not shortened.

## Matrix

| Away interval | Direct | Forced local relay |
|---|---|---|
| At least 5 minutes | PASS e85dc6e9, 329.659 s idle, connected in 2.920 s | PASS same APK, 323.527 s idle, connected in 1.722 s |
| At least 1 hour | PASS with note on main f765aae5; onShow → connected 3.820 s, wake → connected 5.146 s | PASS on the same APK; onShow → connected 1.146 s, wake → connected 1.650 s |
| Simulated next day (90 min deep idle + clock/network/standby changes) | NOT PASS on main c414bba7: recovered, but 3 post-resume opens / 2 native binds | NOT RUN; main build |

Each cell needs its own sleeping/waking record and an actual post-resume
message/reply. Cases may share a paired device, but not an interval disturbed
by another case. Run the short cases first. Do not leave a long test holding
the team's only emulator without agreeing the slot. For a long interval,
record its due time and use a monitor to resume the check, not a sleeping
shell. Keep sandbox and relay processes alive for that interval and state if
the host slept or restarted.

**Latest lead landing gate:** only the two 5-minute cells gate landing.
Prosciutto accepted both; Risotto's Rust and then Pesci's Dart/release logging
guard landed, present in main **f765aae5**. The hour and simulated-next-day
cells run on a **main build after landing**; they decide whether #214 closes,
not whether this strictly better fix lands. Do not run them on the old
candidate and relabel them as main evidence. Team landings require Prosciutto's
review and an approved gap.

**Final lead/PM ruling:** no 24-hour emulator infrastructure. For the simulated
next-day cell, hold deep idle for 90 minutes, exercise idle/maintenance windows
with `deviceidle step`, apply rare/restricted standby buckets to the exact app,
then before unlocking advance the AVD clock by at least 20 hours and toggle
Wi-Fi off/on. Record each actual state and the before/after clock/network,
including whether Ethernet remained active (Wi-Fi toggling alone may not change
the active AVD network). Restore only this test's clock/automatic-time settings,
standby bucket and network changes afterward. A device-only clock jump tests
clock skew, not true time passing at the backend or actual VPS token expiry;
state that limitation. Record the cell as **simulated next day**, never an
actual overnight PASS. A real overnight check awaits a physical phone or the
friend's actual use.

The direct row requires backend device path `DEVICE_PATH_DIRECT`, both before
sleep and after reconnection. The relay row requires `DEVICE_PATH_RELAYED`, a
code without `a=`, a pinned owned local iroh-relay, and the exact app UID's
IPv4 **and** IPv6 UDP blocked. Removing direct hints alone is not proof. Follow
the scoped firewall/reverse setup and cleanup in `friend_apk_gate.md`; never
flush global rules, remove another agent's reverse, or enable a public relay
on a QA sandbox. Physical public-network/VPS relay is a separate owed row.

## Measurements and assertions per cell

1. Begin at the actual synthetic agent conversation, connected. Record the
   approved device and its path, selected agent, process PID and backend/relay
   log offsets. Finish one nonce roundtrip to prove the starting connection.
2. Send the app to the background using the OS, switch the screen off and
   enter actual deep Doze. Record deep-idle confirmation and the time. Do not
   force-stop the app: that tests process death, not the lifecycle fix.
3. After the full interval, verify deep Doze again, release only this test's
   forced-idle/battery simulation, wake the device and resume the actual
   activity. Record a host monotonic start immediately before the resume
   command. No synthetic Flutter lifecycle callbacks or fake network status.
4. Scope logs to this device's public endpoint and this resume window. Record
   the actual sandbox `tunnel connection opened` and `device connected` lines,
   with only synthetic public ids; redact private client ids/addresses. The
   number of new tunnel-open lines is **exactly one**. Record the preceding
   old-connection drop when emitted; do not assume it must arrive after wake.
5. Record both device-wake → backend `device connected` and app onShow →
   connected, plus the reached connected UI upper bound. Latest lead ruling:
   the **5-second** target starts at onShow (the app learning the person came
   back), not the earlier Android wake command; retain the wake clock beside
   it. Exactly one tunnel opens, with no bind after the first post-resume
   endpoint. Confirm connected UI separately; do not infer it from
   a stale transcript. UI polling gives an upper bound; label it
   as such, not precise transport latency. A stale transcript alone is not a
   connected surface. Open connection details and assert its real connection
   status/path, then return to the same conversation.
6. Send a new ASCII nonce from the named composer and assert its exact QA
   assistant reply, correct backend `replied_to`, same approved device and no
   pending device. Verify direct/relayed path again. Do not reuse a historical
   reply or accept an echoed user bubble as the assistant response.
7. Attach adb logcat **before sleeping**. Build debug/profile with
   `--dart-define=FACTORY_LOG_STDOUT=true` for the safe `factory.backend`
   lifecycle mirror (ignored in release mode), and enable Risotto's debug
   native logging before launching. Positively observe `to the background`
   and `back after Ns`. Between them require **zero binds and zero tunnel
   opens**; after resume require exactly one open/bind. Correlate close and
   fallback categories. Save selected sanitized fragments, not raw logcat/UI
   XML. A disconnected VM observer or absent logging cannot prove zero events.
8. Relay client registrations are matched by the endpoint's first 5 bytes
   (10 lower-hex digits), using the whole word `registering`, not
   `unregistering`. Record duplicate/replaced-client count. Normal PASS has
   zero. A close-timeout line in a normal 5-minute case is itself a finding,
   not a PASS. Final lead exception: a separately classified exceptional slow
   close may be **PASS-with-note** only if a duplicate follows the explicit
   3-second-timeout line, the newer client is served, and a positive completion
   log proves the old endpoint finished closing within 10 seconds. Missing
   completion evidence or any uncorrelated duplicate fails. Iroh 1.5 cannot
   abort a started close, so R1 lets it finish in a tracked background task;
   R2 joins concurrent reconnect calls. Rust owns endpoint rebuilding; Dart
   reconnects once, with one fresh-open fallback after 10 seconds without
   Welcome (or an unambiguous dead local port). Retry ticks must not start
   competing rebuilds.

App log category: `factory.backend`. Current messages include
`to the background`, `back after Ns`, `in the background: the tunnel waits`,
`in the background: trying again once the app is back`,
`not back within 10s: opening the tunnel afresh`,
`the tunnel's port didn't answer: opening it afresh`, and
`the old tunnel didn't close within 10s`.
Rust separately reports its 3-second close timeout and positive background
completion. The old Dart 3-second-close string belongs to the failed candidate,
not the current contract. Do not treat absent logging as zero events.

## First paired direct 5-minute case — FAIL, 2026-11-22

- Test source a337ee66 = Risotto Rust 82627336 + Pesci Dart ce166597, debug
  arm64 dev APK, 105,226,736 bytes (checksum in private QA evidence).
  This is not the signed friend APK and contains no C1 code.
- Fresh qa.sh sandbox `doze214-direct5`, proven release-source c36914a5c8
  backend runtime v0.1.15. Actual paste, matching number/first-device Allow,
  drawer agent-row selection, exact pre-sleep QA reply and
  `DEVICE_PATH_DIRECT` were reached. Exactly one initial backend tunnel.
- Deep idle verified before the interval and again before waking; elapsed
  **335.154 seconds**. Wake clock **02:07:59.152430Z**. First new tunnel open
  **02:08:18.697305Z**, **19.545 seconds** later. First new `device connected`
  **02:09:02.374350Z**, **63.222 seconds** later. By 02:09:10, there were four
  new tunnel opens and two new device-connected events, not one.
- UI hierarchy collection timed out after 30 seconds. The helper records
  `MEASUREMENT_FAILED`; it is not a connected-UI timing pass. Backend timing
  and extra connections independently fail the case.
- Positive native endpoint-bound logging was present, with repeated binds
  after resume. Risotto independently read the same PID log: the endpoint was
  ready at 02:08:08.5, Dart dialled about 9 seconds later, the direct dial
  connected at 02:08:19.10 then was LocallyClosed 12 ms later, followed by another
  open_tunnel. His interpretation and Pesci's source review identified two
  competing endpoint owners; this is not excused as a semantics-tool problem.
- Device idle/battery simulation was restored. No 1-hour/90-minute case was
  started. Failed fixture was left available for owners' investigation; no
  real Claktory, public relay or live provider was involved.

Durable sanitized backing in Doppio's agent folder:
`evidence/doze214-20261122/direct5-failed.json` (checksum kept privately).
Read-back equals the source record (normalized final newline). Selected actual
backend lines retain only a synthetic public endpoint; private client id is
redacted. No pairing code, token, capability URL or UI hierarchy is archived.

## Revised candidate: wake target met, hidden-state gap found

Pre-D5 paired source **1158a943** combines Rust **fe22f05a**, Dart **ecbd7d3a**
and **90cec9ce** (a test-only follow-up; the production fix is already inherited).
Debug arm64 dev APK: 135,680,158 bytes (checksum kept privately).
Installed base APK was independently streamed/hashed and matched these bytes.
The artifact contains no C1 messaging-service type; notifications remain parked.

Fresh fixture `d214-ready` reached actual paste/code, matching number and normal
first-device Allow, the drawer's agent row, an exact QA assistant nonce reply,
`DEVICE_PATH_DIRECT` and exactly one initial backend tunnel. This is setup
proof, **not a Doze PASS**. Cold bulk adb input had produced partial/mismatched
field content in earlier setup attempts. The corrected driver waits for the
real IME, sends bounded real-key chunks and asserts the entire actual field
matches the canonical code before Connect. Its first native dial then had
nonempty direct hints. No app parser, credential handler or native callback
was replaced. Those failed setups never reached a measured Doze interval.

Host load later reached **394 on 24 logical CPUs**; even streaming the installed
APK took 77 seconds. Timing acceptance was paused rather than blaming emulator
scheduling on phone networking. The helper now records host load at sleep and
wake. The original failure's absolute timings may include host starvation;
its repeated closures/reopens and the competing endpoint owners were also
independently confirmed from code/native logs.

The emulator and only owned QA `d214-ready` were paused outside a measured
interval, then restarted after the host-load monitor fired. The QA model was
restarted through `qa.sh serve-model` at its preserved sandbox port; a fresh
exact assistant reply proved the fixture before sleeping. No real provider,
public relay or new approval bypass was used.

Actual revised-pair direct case:

- Deep idle began **2026-11-22 03:18:42.385851 UTC** and was verified again
  before waking after **308.055 seconds**.
- Wake began **03:23:50.489796 UTC**. One backend tunnel opened at
  **03:23:52.425259**, and the same stored device connected at
  **03:23:52.585405**: **2.096 seconds** after wake. Connected UI was reached
  within **5.893 seconds**, including one bounded UI-observation error.
- The same app process and approved device stayed in use, with path
  `DEVICE_PATH_DIRECT` and no pending device. The actual post-resume message
  `final-direct5-after` reached exact assistant text `OK: final-direct5-after`;
  backend assistant sequence 9 has `replied_to=8`, the nonce's user sequence.
- Native endpoint binds occurred **12.598, 30.355 and 53.362 seconds into
  idle**, then once at **1.992 seconds after wake**. These idle binds prevent
  certification despite the good wake timing. The first idle rebuild logged
  the 3-second close timeout; completion followed 0.073 seconds later and
  reported **3.1 seconds total since close began**, not 3.1 after the warning.
- Host load was **165.47 at sleep / 121.50 at wake on 24 logical CPUs**.
  Absolute timings remain load-qualified.
- Dart lifecycle events during this interval were **not observed**, not zero.
  The VM observer disconnected during Doze; logcat retained native events but
  no Dart resume line (`developer.log` is not stdout). A later metadata-only
  observer and actual HOME/resume positively reached the background/resume
  canaries. No absent-log claim is used to close a gate.

Prosciutto identified the missing Dart hidden-state guards and assigned Pesci
**D5**: no new endpoint or connection while hidden, no hidden fallback/dead-port
reopen or sleep check, and failed hidden tries wait for the resume's wake.
The existing live background connection may stay open. D5 also adds an opt-in
`FACTORY_LOG_STDOUT=true` debug logging mirror. Reruns were held until D5 was
paired with fe22f05a/ecbd7d3a and rebuilt with that define. The resulting
short-case passes are recorded below; the pre-D5 interval remains uncertified.
The gate requires attached sanitized adb logcat, positive background/resume,
zero binds between those events, then one open/one bind and connected within
5 seconds. Missing observability is not PASS.

Durable sanitized backing in Doppio's agent folder:
`evidence/doze214-20261122/direct5-final-pre-d5.json` (checksum kept privately).
Read-back matched the helper's measured values and independently checked the
synthetic transcript reply relation. Only scoped synthetic public ids and exact
lifecycle/native event fragments are retained, not private logs or credentials.

No full-suite builds run for this gate during the release freeze; all changes
stay on branches. Physical-phone availability remains unknown pending Matt's
card, not a claim that no phone exists. No actual overnight or configured FCM
result is promised.

## D5 direct and forced-local-relay 5-minute cases — PASS, 2026-11-22

Paired source **e85dc6e9** = the pre-D5 pair plus Pesci **5340c10c** and
**023e1530** (release logging guard). Rust is **fe22f05a**. One debug arm64 dev
APK with `FACTORY_DEV=true` and `FACTORY_LOG_STDOUT=true` was used for both
cells: **135,676,091 bytes** (checksum retained in private QA evidence).
Installed APK bytes matched independently. Manifest version is **0.1.0 (1)**;
Hello reports **dev**, as expected for an unstamped debug build. No C1 service
type is present. Local touched tests: **21/21 tunnel tests**, plus helper
self-test/syntax.
This is not a friend release, physical-device or compatibility certification.

| Assertion | Direct | Forced local relay |
|---|---|---|
| Verified deep idle | 329.659 s | 323.527 s |
| Wake UTC | 03:39:06.429376 | 03:49:21.884300 |
| Backend one tunnel open UTC | 03:39:09.269905 | 03:49:23.558478 |
| Backend device connected UTC | 03:39:09.349511 | 03:49:23.606751 |
| Wake → backend connected | **2.920 s** | **1.722 s** |
| Connected UI upper bound | 6.696 s | 5.523 s |
| Hidden opens / binds | **0 / 0** | **0 / 0** |
| Post-resume opens / binds | **1 / 1** | **1 / 1** |
| Duplicates/replaced/Another endpoint warnings | **0** | **0** |
| Slow-close/fresh-fallback events | **0** | **0** |
| Actual post-reply `replied_to` → nonce seq | **12 → 12** | **2 → 2** |
| Host wake load / logical CPUs | 40.27 / 24 | 16.14 / 24 |

Each UI upper bound includes one bounded observation error; neither is claimed
as exact transport latency. Actual named connected UI, exact assistant replies,
matching backend reply relations, same process/approved device and no pending
device were independently reached. Direct stayed `DEVICE_PATH_DIRECT`; relay
stayed `DEVICE_PATH_RELAYED`. No persisted app data was copied between cases.

Attached adb positively captured both hide and resume canaries. Direct showed
`to the background` at 03:33:37.190 and `back after 330s` at 03:39:07.783;
relay showed 03:43:58.701 and `back after 323s` at 03:49:22.475. In each case
the socket dropped while hidden, followed by the D5 tunnel-waits/try-once-back
messages and **no hidden bind**. The sole post-resume native binds were
03:39:08.641 (direct) and 03:49:23.559 (relay).

Relay used fresh QA `d214-relay-d5`, normal real number/Allow, no direct code
hints, pinned local iroh-relay 1.5.0, exact dev UID **10231** with IPv4 **and**
IPv6 UDP blocked (verified before and after), and only TCP3340 reverse. Its
app-scoped registration was positively observed at **03:49:23.489757 UTC**;
there was exactly one registration and zero duplicates. An ANSI-formatting
parser gap initially reported zero registrations: stripping ANSI and rescanning
the unchanged preserved offset found the positive event. A colored
registration/unregistration/duplicate regression now covers this gap; no new
interval or APK bytes were substituted.

Durable sanitized backing in Doppio's agent folder, read-back verified against
the measured values and synthetic backend reply relations:

- `evidence/doze214-20261122/direct5-d5-pass.json`.
- `evidence/doze214-20261122/relay5-d5-pass.json`.

Artifact and backing checksums are kept privately, not in this public source.

After checking both cells, only the owned exact-UID rules and TCP3340 reverse
were removed; absence was verified, tracing cleared and adbd unrooted. Owned
observers, QA/model, local relay and emulator were stopped at a case boundary.
No short-case forced interval remained after that cleanup. The long cells use
a new **main build** after the approved Rust → Dart landing, not this candidate
APK. Physical Android remains unknown pending the person's card; actual VPS/public
network and true overnight remain owed.

## Main-build direct-hour row — PASS with note

A separate debug arm64 APK was built from main **f765aae5**, 135,791,800 bytes;
its checksum is kept privately and installed bytes matched independently. It
contains no C1 messaging service. The direct-hour fixture reached normal real
pairing/number approval, the actual agent and exact pre-reply, direct transport
and one initial tunnel. Deep idle began **2026-11-22 03:58:05.316072 UTC**;
the full-hour wake was scheduled for **04:58:05.316072 UTC**, with a monitor
at 04:58:20.
Attached adb positively observed background and the hidden-wait messages.

At the 04:38 UTC status check this row was still sleeping. At the first wake
attempt Android had naturally entered `IDLE_MAINTENANCE` at the hour boundary:
at 04:58:41 it reported maintenance had begun 35.847 seconds earlier, with
forced idle still enabled, screen off, unplugged and the same app process.
The helper correctly refused **before any wake/unforce/foreground command**.
No state was forced or stepped; the check waited for natural deep idle to
return. Actual away time was **3921.878 seconds**, including that scheduled
maintenance—not 65 minutes of continuous `IDLE`.

- Wake clock: **05:03:27.248805 UTC**.
- Attached adb first onShow/resume log host receipt: **05:03:28.574548 UTC**.
- Exactly one backend tunnel opened: **05:03:30.805992 UTC**.
- Backend `device connected`: **05:03:32.394325 UTC**.
- **onShow/resume-log receipt → connected: 3.820 seconds**.
- **Device wake → connected: 5.146 seconds**; the helper's conservative
  five-second wake flag remains **false**, not silently rewritten.
- Connected UI upper bound: **9.673 seconds**. Host wake load: **97.36 on
  24 logical CPUs**.
- Zero opens/binds while hidden, then exactly one open/bind. No duplicate,
  replaced-client, Another endpoint, slow-close or fresh-fallback event.
- Same approved direct device/process, no pending device. Actual nonce
  `main-direct1h-after` reached exact assistant reply, whose backend
  `replied_to=2` matches the nonce's user sequence 2.

Prosciutto's explicit acceptance ruling:

> Direct 1 h: PASS with a note, once the nonce and replied_to check finishes.
> The 5 s target is from the person coming back to connected. The app learns
> that at onShow, so onShow → connected (3.82 s) is the measure. Device wake →
> connected (5.146 s) also includes Android waking from deep idle before the
> app shows, and the host was at 97/24 load.

The nonce/reply relation then finished and was independently verified. Both
clocks, load and natural-maintenance qualification remain recorded. Durable
sanitized backing is `evidence/doze214-20261122/main-direct1h-pass-with-note.json`
in the agent's private evidence; its checksum/read-back are kept privately.

Only after this case and cleanup was the emulator handed to Risotto for a
brief native instrumentation window. The fresh relay-hour fixture is recorded
below. Emulator restarts happen only at case boundaries so the two-hour task
lifetime cannot truncate a long row. The parity audit does not interrupt Doze.

## Main-build relay-hour row — PASS

The instrumentation window finished and returned the emulator before this
fresh case. A new emulator process was started at that boundary; the same main
APK was reinstalled and its installed bytes independently verified. Fresh
normal pairing/number approval, the actual agent row, exact pre-reply and
`DEVICE_PATH_RELAYED` were reached, with one initial tunnel. Only the exact dev
app UID's IPv4/IPv6 UDP was blocked, with owned local relay and TCP3340 reverse.

Deep idle began **2026-11-22 05:12:25.375402 UTC**. At the hour boundary
Android naturally entered maintenance: at 06:12:49 it reported maintenance
24.140 seconds earlier and its next alarm 275.859 seconds later, still forced
idle, screen off, unplugged and with the same process. This was read only;
no wake, unforce, foreground or artificial state step happened. The check
waited for natural deep idle to return. Actual away time was **3921.775 seconds
including scheduled maintenance**, not 65 minutes of continuous `IDLE`.

- Wake clock: **06:17:47.177844 UTC**.
- Attached adb first onShow/resume log host receipt: **06:17:47.681287 UTC**.
- One scoped app relay registration: **06:17:48.790743 UTC**.
- Exactly one backend tunnel opened: **06:17:48.800192 UTC**.
- Backend `device connected`: **06:17:48.827742 UTC**.
- **onShow/resume-log receipt → connected: 1.146 seconds**.
- **Device wake → connected: 1.650 seconds**; both clocks meet the target.
- Connected UI upper bound: **5.514 seconds**. Host wake load: **25.30 on
  24 logical CPUs**.
- Zero opens/binds while hidden, then exactly one open/bind. Zero duplicate,
  replaced-client, Another endpoint, slow-close or fresh-fallback event.
- Same approved relayed device/process, no pending device. Actual nonce
  `main-relay1h-after` reached exact assistant reply; its backend
  `replied_to=2` equals the nonce's user sequence 2.
- The exact dev UID's IPv4 and IPv6 UDP blocks were verified again after the
  reply; the actual path was `DEVICE_PATH_RELAYED`, not merely missing direct
  hints. Relay counts use the preserved scoped offset with ANSI stripped.

Durable sanitized backing is
`evidence/doze214-20261122/main-relay1h-pass.json` in the agent's private
evidence. Read-back matched measured clocks/load and the reply relation;
checksums remain private.

The owned rules/reverse/property were then removed and absence verified, adbd
unrooted, and the owned observer, QA/model and relay stopped. Only then was
the emulator handed over for native tests. No Doze interval is active.
Simulated-next-day rows remain **NOT RUN**; per the latest lead sequence,
phase-2 native end-to-end checks run between the relay hour and next-day rows.
A fresh main build used there gets its own per-row provenance, never the old
artifact relabeled as a newer build.

## Main simulated-next-day direct row — finding, NOT PASS

Fresh app and guarded QA builds use main **c414bba7**, including Android's
GSO-off fix and the shared phone chip-tap fix. Installed bytes and private
source receipts were checked. The actual sent-video chip retest first pushed
the named native viewer directly, without the Extensions workaround, and
Back returned to the composer. That completed before this Doze interval.

Actual forced deep idle began **2026-11-22 09:05:43.566674 UTC**; the minimum
90-minute due time is **10:35:43.566674 UTC**. The attached sanitized observer
has positive native-bind and background canaries. The same approved device
was DIRECT with no pending approval. Actual rare (40) and restricted (45)
standby buckets, and an IDLE → IDLE_MAINTENANCE step, were recorded without
unlocking. This interval includes maintenance, not 90 continuous minutes in
IDLE. No UI audit runs during it.

The host-measured interval before wake was **5,531.974 seconds**, including
manual and natural maintenance. Before any unlock, the device clock advanced
**72,003 seconds**, and actual Wi-Fi settings went off then on. Those commands
ran from **10:37:53.791 to 10:37:55.284 UTC**, before the wake clock at
**10:37:55.627880 UTC**. The host clock stayed stable. The captured default
network remained 101; Ethernet and Wi-Fi transports were still listed even
in the off-setting snapshot, so native connectivity callbacks may have
settled after the commands returned. No actual route change is assumed.

**What passed:** zero hidden native binds and zero hidden backend tunnel
opens; same process/approved DIRECT device, no pending approval; the actual
post-resume nonce `main-direct-sim90-after` reached its exact visible assistant
reply and verified backend `replied_to`. The persisted synthetic user/reply
pair was independently retained. First connection took **2.527 s from wake**
and **1.508 s from the first onShow host receipt**, within the five-second
target on both clocks. The helper kept five bounded UI-observation errors and
no connected-UI upper bound; a later actual drawer reached Connected to Doze QA.

**What did not pass:** three post-resume tunnel opens, not one:

- **10:37:57.983369 UTC** — initial open; connected at 10:37:58.155019.
- **10:37:59.501964 UTC** — another open after a network-moved event at
  10:37:58.515189 (host receipt).
- **10:38:11.114973 UTC** — a third open after another network-moved event at
  10:38:10.416808 (host receipt).

There were two post-resume native binds and two 1006 closes, with no observed
slow-close warning or fallback. The attached observer retained two lifecycle
resume log events only 2 ms apart; it did not erase either or infer native
subscription count from them. Native path/listen event logging was not
available in this build. Dart and native network owners have the timeline;
the cause and any required fix/retest are pending. Recovery and a correct
reply do **not** override the extra-open criterion.

The owned device clock, automatic time, Wi-Fi, standby bucket, debug property,
battery/idle state and non-root adb were restored and checked. Forward/reverse
lists were empty; the app's disposable data was cleared, QA stopped and both
observer and emulator stopped before the native owner's next sequential
API-24/API-36 slot. Sanitized backing was archived outside build output and
read back: `evidence/doze214-20261122/main-direct-sim90-finding.json`. Private
checksums, endpoint identifiers, pairing material and raw UI are not published.
This remains a **SIMULATED next day / clock-skew** test, not true overnight or
backend-token passage. Relay and independent QA remain owed.

## Measurement helper

`doze_reconnect_gate.py` is deliberately **not an overall PASS orchestrator**.
It records actual forced deep idle and measures the real resume; transport,
post-resume roundtrip and native close/bind assertions remain independent.
It never installs, pairs, approves or uses fake lifecycle callbacks. It refuses
physical phones, pre-existing forced idle/battery simulation, evidence outside
this checkout's ignored app/build, early resumes and rotated/truncated logs.
All raw UI/log text stays in memory; only scoped sanitized evidence is kept.

```sh
D=app/integration_test/android/doze_reconnect_gate.py
python3 "$D" self-test
# AFTER receiving the emulator slot, establishing the transport and reaching
# a visible named connected status (leave that surface in place):
python3 "$D" --own-emulator-slot --package com.claktory.app.dev begin \
  --record app/build/android-doze/direct-5min.json \
  --backend-log app/build/android-doze/qa/sandboxes/doze/factoryd.log \
  --endpoint "$SYNTHETIC_APP_PUBLIC_ENDPOINT" --transport direct \
  --seconds 300 --connected-label 'Connected to Doze QA'
# Schedule a monitor at or after the recorded due time, then:
python3 "$D" --own-emulator-slot --package com.claktory.app.dev resume \
  --record app/build/android-doze/direct-5min.json
```

Use a distinct record for each cell; valid intervals are 300, 3600 and 5400
seconds (the last is only the simulated next-day baseline). A relay cell also needs `--relay-log` pointing to the owned local
relay log. The helper does **not** configure transport or firewall rules. It
starts the wake clock before screen-on/activity resume (a conservative bound),
uses the sandbox's UTC timestamps for backend connection time, and reports UI
polling as an upper bound. Its five-second flag is only for that conservative
wake clock; correlate the separate onShow log for the lead's acceptance target.
`MEASURED_NOT_CERTIFIED` is not PASS. A bounded-close
exception is never automatically accepted by the helper. Finish all remaining
assertions and preserve their backing before marking a matrix cell green.

If a long interval is interrupted or refused before wake, clean up the owned
forced state explicitly (`dumpsys deviceidle unforce`, `dumpsys battery reset`)
after deciding that case must be retried. A too-early resume deliberately
leaves the interval sleeping rather than shortening it silently.

## Cleanup and evidence

Restore only this test's forced idle/battery state, debug property, exact UID
rules and reverse. Verify their absence before unrooting. Stop only the owned
QA/relay processes once all scheduled checks finish. Give the serial emulator
back explicitly. Archive sanitized backing outside ignored build output and
verify its hash/read-back before trimming caches. Never retain pairing codes,
notification keys, capability URLs, whole UI hierarchies or raw private logs.

Physical-phone availability was requested from Matt by card. Until answered,
it is **unknown**, not evidence that no phone exists. If one is available,
supply a short phone-only check; an emulator PASS cannot close that row.
Report each matrix cell separately to Prosciutto, including elapsed interval,
transport evidence, reconnect timings, one-tunnel log and anything still owed.
