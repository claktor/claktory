# Claktory on Android: the native host

The Android app is the same Flutter app the iPhone runs, in a thin Kotlin
host, talking to the person's Claktory through the same Rust tunnel. This
file is the host's design: the build tuple, the host's shape, how the
tunnel library gets built and packaged, the JNI bootstrap iroh needs, and
the gate every release artifact passes. The work and its reasons are in
Iris's spec (`workspace/factory-research/android.md`, Part I §3–4, §8;
Part II A); this is how the Android Native side carries it out.

Status: **A.1 boot proof done on the emulator** (§7, "Results"); device
acceptance still owed. Later packages are still design.

Owner: Raj (Android Native). Shared files named below stay their owners';
the changes they need are handed over as reviewed diffs.

## 1. The build tuple

`toolchain.properties` pins every tool, and is the one place they're
named: Gradle reads it for SDK levels, ABIs and the NDK; the scripts
(`scripts/android/`, Arjun's) check the machine against it before building
and record it beside every artifact; the verifier reads its ABIs, page
size and minSdk.

| Pin | Value | Why |
|---|---|---|
| Flutter / Dart | 3.50.0 (ab2912035e) / 3.14.0 | What `app/.metadata` and the iPhone and Mac builds use. |
| Rust | 1.97.1, targets `aarch64-linux-android`, `x86_64-linux-android` | `rust-toolchain.toml`'s channel; the targets are a handoff (§3). |
| JDK | Temurin 21 LTS, explicit path | AGP 9.3 needs ≥17; Gradle 9.5 runs on 17–26. One named JDK, never an IDE's bundled one or whatever `JAVA_HOME` holds. |
| Gradle | 9.5.0, distribution SHA-256 pinned | Flutter 3.50's template; `distributionSha256Sum` in the wrapper so a swapped download fails. |
| AGP | **9.3.3** (template says 9.3.1) | Same minor as Flutter's template, patch fixes only; inside Flutter 3.50's known-supported range (≤9.4). 9.4.x is a later, deliberate upgrade. |
| Kotlin | 2.4.20 | Flutter's template and its max known KGP. AGP 9 compiles Kotlin itself. |
| minSdk / compile / target | 24 / 36 / 36 | 24 is Flutter's floor and the NDK API the Rust and C code link for (Flutter hands minSdk to the native-assets hook, `targetNdkApi`). 36 is Play's requirement for new apps and updates. |
| Build tools | 36.0.0 | AGP 9.3's pairing. |
| NDK | 28.2.13676358 (r28c) | Flutter's template and AGP 9.3's default. r28+ links 16 KiB-aligned by default, and `native_toolchain_rust` needs ≥ r27 (it looks for the API-35 clang wrappers). |
| ABIs | `arm64-v8a`, `x86_64` | Phones are arm64; x86_64 is for emulators and CI hosts that aren't Apple silicon. No 32-bit ARM: untested support is worse than none (spec §3). |
| Page size | 16 KiB | Required of apps targeting 35+; built that way from day one. |
| bundletool | 1.18.3 | Current release (Google Maven), for AAB config dumps and APK sets. |

**The build never installs anything itself.** `gradle.properties` sets
`android.builder.sdkDownload=false` (AGP otherwise installs a missing NDK or
platform on its own, licences and all) and turns off Gradle's JDK
auto-provisioning (`org.gradle.java.installations.auto-download=false`).
A missing piece fails the build and goes to Arjun. Gradle's and AGP's Maven
dependencies do download into `~/.gradle` on the first build, like Cargo's
crates: that cache is part of the install budget Matt approves.

**Ambient overrides are refused, not tolerated.** The doctor script fails
when any of these are set: `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`,
`CARGO_BUILD_RUSTFLAGS`, `CARGO_TARGET_*_LINUX_ANDROID_RUSTFLAGS`,
`ANDROID_NDK_HOME`/`ANDROID_NDK_ROOT` pointing elsewhere than the pinned
NDK, or a `JAVA_HOME` that isn't the pinned JDK. Each of these silently
changes what gets built: the first three, for instance, switch off the
16 KiB link flag (§3).

## 2. The host

Generated once with `flutter create --platforms=android --org com.claktory`
in my checkout, then every generated file reviewed: anything it writes
outside `app/android/` is reverted, except `app/.metadata`'s new android
entry, which goes to its owner.

```
app/android/
  toolchain.properties  DESIGN.md  tools/          (this design, the verifier)
  settings.gradle.kts  build.gradle.kts  gradle.properties  .gitignore
  gradle/wrapper/gradle-wrapper.properties
  app/build.gradle.kts  app/proguard-rules.pro
  app/src/main/AndroidManifest.xml
  app/src/main/res/{mipmap-*,drawable*,values*,xml/{backup_rules,data_extraction_rules}.xml}
  app/src/main/kotlin/com/claktory/app/
    ClaktoryApplication.kt   the JNI bootstrap, first thing in the process (§4)
    TunnelBootstrap.kt       its one native method
    MainActivity.kt          thin: registers the adapters, nothing else
    Adapters.kt              which adapter answers which channel
    NetworkPath.kt           factory/network (boot proof, see below)
    …                        later: DeviceStore (§5), Drafts, Scanner, Pickers,
                             WebAuth, Links, Recorder, AudioPlayer, VideoPlayers
  app/src/test/…             JVM unit tests (Robolectric where Android is needed)
  app/src/androidTest/…      instrumentation tests, on an emulator or a phone
```

- **Ids.** `com.claktory.app`, and `com.claktory.app.dev` for Claktory Dev,
  as on the iPhone; `connectScheme` in `connectors.dart` already expects
  those. Product flavors `prod` and `dev`; the build script passes
  `--flavor dev` and `--dart-define=FACTORY_DEV=true` together, never one
  without the other. Kotlin package and namespace `com.claktory.app` for
  both.
- **MainActivity** is a `FlutterActivity` whose `configureFlutterEngine`
  calls `Adapters.register(engine, this)`; each adapter is its own class
  owning one channel, so a feature is a file, testable without the
  activity.
- **Manifest.** `INTERNET` and `ACCESS_NETWORK_STATE` (iroh's DNS reads the
  active network through `ConnectivityManager`; without the permission
  that throws). `allowBackup="false"`, `fullBackupContent` (≤ API 30) and
  `dataExtractionRules` (31+) excluding everything from cloud backup and
  device transfer. Camera as an optional feature, microphone asked on use,
  both later. Launcher activity `singleTop`, `adjustResize`, resizable, no
  orientation lock. `android:enableOnBackInvokedCallback`: the shared Dart
  Back is bound on main (Katya, 34d486bc), so the dev build can turn it on
  to run her OS Back matrix; that's a test run, not device acceptance, and
  three-button Back stays in the foreground-core gate either way.
- **Release builds** shrink with R8; `proguard-rules.pro` keeps
  `TunnelBootstrap`'s native method. No debug-signing fallback: a release
  without its signing config fails.
- **factory/network belongs in the boot proof, not later parity.** iroh's
  network monitor on Android is a no-op (`netwatch` 0.20.2,
  `netmon/android.rs`: "Android doesn't allow us to do this"), so iroh
  never notices Wi-Fi ↔ cellular on its own. `network.dart` already turns
  the channel's events into `Tunnel.reconnect`, which calls
  `endpoint.network_change()`. On Android the channel is a
  `ConnectivityManager.NetworkCallback` giving the same event as
  `NetworkPath.swift`: `{satisfied, interfaces: ["wifi"|"cellular"|"wired"|
  "loopback"|"other"…], expensive, constrained}`; `expensive` is
  "not unmetered", `constrained` is Data Saver.

## 3. How the tunnel library is built and packaged

Through the existing native-assets hook, unchanged in shape; no second
build, no hand-copied `jniLibs`. Read in the pinned sources (Flutter 3.50's
`flutter_tools`, `native_toolchain_rust` 1.1.1, FRB 2.13.0):

1. Gradle runs the variant's `flutter assemble` with the variant's minSdk.
2. Flutter runs `hook/build.dart` per ABI. `native_toolchain_rust` maps
   arm64 → `aarch64-linux-android`, x64 → `x86_64-linux-android`, points
   Cargo's linker and `CC`/`CXX`/`AR` (ring and zstd compile C) at the NDK's
   `<triple><minSdk>-clang` wrappers, and runs `cargo build --release
   --target …` with the pinned Rust (rustup installs the toolchain's listed
   targets on first use).
3. The `.so` becomes a code asset; Flutter copies it to
   `build/…/jniLibs/lib/<abi>/libclaktory_tunnel.so` and its Gradle plugin
   adds that folder to the variant's generated `jniLibs`, so AGP strips,
   aligns and packages it like any native library.
4. At run time FRB opens it with `dlopen("libclaktory_tunnel.so")`.

**Handoff to Tomás** (his files; small diffs, with tests, for review):

- `rust/rust-toolchain.toml`: add the two Android targets. Note: rustup
  then fetches their standard libraries (~100–200 MB, once, into the shared
  `~/.rustup`) the next time anyone builds the app; it belongs in the same
  approval as the SDK.
- `rust/Cargo.toml`: `[target.'cfg(target_os = "android")'.dependencies]
  jni-sys = "0.4"`: already in `Cargo.lock` at 0.4.1 through iroh's
  resolver, so no new crate; two raw calls don't need the `jni` wrapper.
- `rust/src/lib.rs`: `#[cfg(any(target_os = "android", test))] mod android;`
  (its tests run on the Mac).
- `rust/src/api/tunnel.rs`: in `open_tunnel`, before `device_endpoint`,
  `#[cfg(target_os = "android")] crate::android::ready()?;` so a tunnel
  can't start with iroh's DNS uninitialised (§4).
- `hook/build.dart`: add `androidTriples` (`aarch64-linux-android`,
  `x86_64-linux-android`) to the triples whose
  `CARGO_TARGET_<triple>_RUSTFLAGS` the hook sets, and in `rustFlags()`
  give them `-C link-arg=-Wl,-z,max-page-size=16384` beside the home-folder
  remap every target already gets (edf589a). **The explicit 16 KiB flag is
  required, not optional:** `native_toolchain_rust` adds the same flag
  only when it finds no rustflags anywhere, and setting the Android
  triples' variable for the remap is exactly what switches its helper off.
  Without the flag the build would lean on NDK r28's default alone; the
  TestFlight lesson is not to rely on that: say it, and verify it (§6). It
  doesn't touch the iOS triples' `-C strip=none`. A test,
  `app/test/android_build_test.dart` (Tomás's, with the hook), keeps the
  triples, the flag and `rust-toolchain.toml`'s Android targets in step.

(Before edf589a the helper would have applied on this Mac: no
`.cargo/config*` exists in the repository's ancestors or in `~/.cargo`.
Once the hook sets the Android triples' flags, it never does.)

## 4. The JNI bootstrap

iroh 1.5's default DNS resolver (`n0-dns-resolver` 0.1.1) reads the active
network's DNS servers through JNI, from a JavaVM and a Context published to
`ndk_context` (`iroh::dns::install_android_jni_context`). Nothing publishes
them for us: FFI's `dlopen` doesn't call `JNI_OnLoad`, and nothing in
Flutter knows about `ndk_context`. Without them the lookup fails (it's
wrapped in `catch_unwind`) and iroh quietly uses public fallback
nameservers, ignoring the phone's own and its VPN's: not acceptable as
working DNS.

**Design.** `app/rust/src/android.rs` (mine), Android-only:

```rust
// Called once, by ClaktoryApplication.onCreate, before any Dart runs.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_claktory_app_TunnelBootstrap_nativeInstall<'a>(
    mut env: EnvUnowned<'a>, _class: JClass<'a>, application: JObject<'a>,
) -> jboolean
```

- The JavaVM comes from the calling `JNIEnv` (`GetJavaVM`), so no
  `JNI_OnLoad` export is needed.
- The context is a **global** reference to the `Application`, made here and
  never deleted: `ndk_context` requires both pointers to live until the
  process exits, and a local reference dies when this call returns. Never
  an Activity (recreated on rotation) and never `JNI_OnLoad`'s reserved
  argument, as iroh's doc example passes.
- A `OnceLock` makes it idempotent: a second call does nothing and returns
  true; a failure (no VM, no global ref) returns false and leaves the state
  unset.
- `ready()` returns an error ("the private connection isn't set up on this
  device") when nothing was installed; `open_tunnel` calls it first (the
  handoff above), so a missing bootstrap fails loudly instead of resolving
  through fallbacks.

Kotlin: `ClaktoryApplication.onCreate` calls
`System.loadLibrary("claktory_tunnel")` then
`TunnelBootstrap.install(this)`. Loading through `System.loadLibrary` is
what lets the JVM bind the native method; FRB's later
`dlopen("libclaktory_tunnel.so")` from Dart gets the same loaded instance
(same soname, same namespace). **That's an assumption until proved**, and
`open_tunnel`'s `ready()` proves it without any new API: if Dart had a
second copy, its bootstrap would be empty and the tunnel would refuse to
open.

Things to prove on the phone, not just the emulator: the DNS servers iroh
uses are the phone's (log the count, never the addresses), with Wi-Fi,
cellular and a VPN; starting with no network at all doesn't crash
(`getActiveNetwork` returns null then); the R8 release build still binds
the native method.

**Private DNS, for later.** iroh sends plain UDP queries to the servers
`LinkProperties.getDnsServers()` lists. With Android's Private DNS on, the
phone's own lookups go encrypted to the chosen provider, but iroh's (relay
names, n0's address lookup) don't. Not a boot-proof blocker; the fix is a
custom iroh resolver over Android's `DnsResolver` (API 29+) or upstream.
Raised with Mike as a product decision.

## 5. The device's credentials (package A.2)

What `factory/device`'s `connections` / `setConnections` keep: the map
`{current, saved, keys, account}` exactly as `DeviceConnectionStorage`
sends it, where `keys` holds this device's iroh key per pairing code and
`account` is `DeviceAccount.toKept()`, `{key, service, email}`, `key`
being the raw account seed. Those seeds are what the person's Claktories
and the account service know this phone by; losing or cloning them is
losing or cloning the device. The iPhone keeps the same map in the
Keychain, this device only, after first unlock.

**Not EncryptedSharedPreferences** (androidx security-crypto is
deprecated) and never plain SharedPreferences: a Keystore key protecting a
small file of our own, because the app already owns the channel and the
schema is one map (spec §4).

### The key

- AES-256-GCM, generated in Android Keystore under the alias
  `claktory.device.v1`, purposes encrypt and decrypt, no padding,
  randomized encryption required, so Keystore makes a fresh 12-byte nonce
  for every write. Never exportable; the app never sees it.
- No user authentication and no biometric prompt: the app must reach its
  Claktories whenever it's open.
- StrongBox when the phone has it (`FEATURE_STRONGBOX_KEYSTORE`), else the
  TEE. Never a fallback to anything that isn't Keystore. Where it lives
  (`KeyInfo`: StrongBox, TEE or software, as on an emulator) is recorded
  for diagnostics, never hidden.
- What it protects is the file at rest. The seeds are still decrypted into
  the app's memory for each signing and tunnel call (the Rust APIs take
  raw seeds); nothing here claims hardware-only signing.

### The file

`noBackupFilesDir/connections.v1`: never in cloud backup or device
transfer by location, and also excluded by the backup and extraction rules
(§2), so a restore can't carry it.

```
"CLKD"  format u8 = 1  key version u8 = 1  nonce length u8 = 12
nonce (12)  ciphertext + GCM tag (16)
```

- The header and the application id (`com.claktory.app` or `.dev`) are
  the GCM additional data: a changed header, or Claktory Dev's file read
  by Claktory, fails authentication.
- The plaintext is UTF-8 JSON, `{"schema": 1, "current", "saved", "keys",
  "account"}`, every value of the map kept, none invented.
- Writes are atomic: encrypt, write a temporary file beside it, fsync,
  rename over the old one (`AtomicFile`). The last good file stays until
  the new one is complete. Decrypted bytes are cleared after use where the
  JVM lets us (byte arrays, not Strings).
- A new key version (rotation, a new format) reads the old file, writes
  the new one, and deletes the old key only after that write succeeds.

### Reading, and what can go wrong

Every read ends in exactly one state, and each failure is its own error
code, never an empty map:

| State | Cause | `connections` answers | The file |
|---|---|---|---|
| Fresh | No file (first run) | `{}` | none |
| Read | Key there, file authenticates, schema known | the map | kept |
| `credentials_key_missing` | File there, key gone: restored or transferred onto another phone, Keystore cleared | error | kept |
| `credentials_key_invalidated` | `KeyPermanentlyInvalidatedException` | error | kept |
| `credentials_unreadable` | Bad magic, truncated, tag fails, JSON broken | error | kept |
| `credentials_newer_version` | Written by a newer app (a downgrade) | error | kept, untouched, for the newer app |
| `credentials_unavailable` | Keystore failed for another reason (busy, provider error) | error | kept; read again next launch |

- **After any failed read, `setConnections` refuses**
  (`credentials_read_failed`) and never touches the file, until a read
  works again (the app retries `credentials_unavailable` by itself) or the
  person starts over. A failed read can't turn into an empty map that the
  next write saves over the person's real identity. A write before any
  read reads first, and is refused if that read fails. (As on the Mac and
  iPhone, `ConnectionStore.swift`.)
- **Starting over is explicit.** A new method, `resetConnections`, called
  only when the person chooses to, moves the unreadable file aside
  (`connections.v1.unreadable`, one copy kept, replaced by the next) and
  deletes the key; the store is then Fresh. Clearing this phone's copy is
  not revoking it: the person forgets the device on their Claktory and
  signs out of the account the way they do today.
- **Never a new identity silently.** The store never makes, re-keys or
  copies a seed; only the existing Dart flows (pairing, signing in) make
  one, after the person has chosen to.

**The shared Dart side doesn't show any of this yet.** `Connections.load`
(`connections.dart`) catches a failed read and carries on with nothing,
and `_changed` logs a failed write and carries on. On Android, with the
rules above, the identity is safe, but the person would see an unpaired
app, and pairing again would live only in memory until the next launch.
It needs a state the app shows ("Claktory can't read this phone's
sign-in and connections", with Start over). The iPhone has the same
defect today (an unreadable Keychain item looks unpaired). Owners, named
by Mike: Kenji the shared Dart (surfacing the codes, the state, Start
over), Tomás the Apple side of `resetConnections`, Hana the words, Katya
the Android binding; I supply the Android store's tests against Kenji's
interface at A.2. Until it lands, A.2 can't count as passing "explicit
recovery failures".

### As built

`CredentialStore.kt` (the envelope and the states above, plain JVM),
`KeystoreSealer.kt` (the key), `DraftStore.kt`, and `DeviceChannel.kt`
(`factory/device`: the stores on one background thread, answers on the
main one; `capabilities`; `cameraAccess` answers `unavailable`, so the
pair screen opens on Paste code; `qrCode` answers null; the picker and web
sign-in aren't implemented yet). The stores are one per process, not per
engine. Proofs: `CredentialStoreTest` (JVM, every state, the envelope's
bytes, nothing kept in logs or errors) and `KeystoreStoreTest`
(instrumentation, the real Keystore under its own alias: round trip, key
not exportable, a fresh Keystore nonce per write, deleted key, flipped
byte, the other flavor's file, start over, nothing kept in logcat).

### What's never written anywhere

Seeds, the account key, the tunnel's capability path and pairing codes
never appear in logs (logcat or Dart), in exception messages or
`PlatformException` details (the Dart side logs those), in test output or
in build artifacts. Errors carry only the codes above and a category. The
Kotlin map type has no `toString` that prints values. Tests check it:
instrumentation reads the app's own logcat and every error message after
the failure cases and fails on any seed or capability substring.

### Drafts

Unsent drafts (`drafts` / `setDrafts`) are a separate store: a private
file, written atomically, excluded from backup like everything else, not
encrypted with the credentials' key and never reset with them.
Recordings and attachment copies are temporary files in the cache folder,
cleaned up after sending or cancelling.

### Proof owed in A.2

Instrumentation tests on an emulator and a real phone: round trip of the
complete map (account included); every failure state above produced on
purpose (delete the key, flip a byte, write a newer version, a file from
the other flavor) and answered with its code, the file kept; a write
refused after a failed read; reset only when called; no seed or capability
in logcat or errors; the file absent from an `adb backup`/`bmgr` backup
and from a device-to-device transfer test; identity unchanged across
relaunch and process death; reinstall starts Fresh, never with an old
identity.

### The tunnel's local listener (A.2)

Any app with `INTERNET` reaches 127.0.0.1, so the tunnel's capability
path (81cc2b4, 0866ac8) is the whole gate between another app and this
device's identity. Reused, not forked. Proofs owed, from Viktor's review:

- A second app (another UID) on the same phone, against a discovered-device
  pairing on a disposable Claktory: no path, a wrong path of the right
  length, the right prefix with a wrong tail, slow and oversized requests.
  Each is closed, and nothing is dialled (the backend sees no session).
- The capability URL lives only in the process, used by `dart:io`: never
  in an Intent, the clipboard, a Custom Tab, a WebView or any URL handed
  to the system, and never in logs, errors or artifacts (above).
- If any Kotlin or Java code ever makes HTTP calls, `network_security_config`
  allows cleartext only to 127.0.0.1, never globally. (`dart:io` sockets
  aren't governed by it.)
- Backend sign-out refuses a discovered device's reconnection while code
  pairing still works (9174b359).

### A.2 results (2026-11-21, emulator only)

On the android36 emulator (API 36, arm64, 4 KiB pages), against a QA
sandbox (`scripts/qa.sh`), never the dev or real Claktory:

- `KeystoreStoreTest`: 6/6 with the real Keystore. `CredentialStoreTest`
  and `DraftStoreTest`: 22/22 on the JVM.
- Claktory Dev, debug, and Claktory, release (R8, signed with the release
  key): paired by pasting a code, allowed, connected; force-stopped and
  opened again: still connected, no pairing. The backend logs
  `kind=Phone`, and the pending device says `DEVICE_KIND_PHONE`.
- A byte of `connections.v1` flipped: the app shows "Claktory can't read
  this phone's sign-in and connections" with Try again and Start over;
  Start over moves the file to `connections.v1.unreadable`, deletes the
  key, and the app opens on pairing. The old file put back, its key gone,
  as a restore onto another phone would be: `credentials_key_missing`,
  the same screen. Logcat shows only the codes.
- The key: made on the first write, "in software" on the emulator (no TEE
  there). A real phone's TEE or StrongBox is still to be seen.
- The loopback listener, probed as another UID (uid 2000, through
  `adb forward`, whose far end adbd opens on the phone) on both of the
  app's tunnel ports, while the app was connected: no path, nothing sent,
  a wrong path of the right length, one longer, one shorter, a 2 KB
  request line, a byte a second, a TLS hello, a POST. Every one closed
  with no byte answered: at once for a full wrong line, after the 10 s
  limit for silent, slow or non-HTTP ones. The sandbox's log shows no
  connection from any of them, and the app stayed connected. The right
  prefix with a wrong tail can't be tried from outside the process without
  the secret; the tunnel's Rust tests cover it (81cc2b4, 0866ac8). The
  pairing was by code; the gate runs before anything is dialled, the same
  for a discovered device, which is still to be tried once a sandbox can
  sign the app in.
- No pairing code, token or capability path in the app's logcat.

Still owed: a real phone (the friend's first session is the device
acceptance), backup and transfer exclusion on a device with a backup
transport, process death under memory pressure, and a discovered-device
pairing.

### Release signing

One key signs every Claktory APK that leaves this Mac, so an update
installs over the last one: RSA 4096, PKCS12, 30 years, `CN=Claktory,
O=Claktory`, certificate SHA-256 `a4cf3493…c34dd41e`. It was made on
2026-11-21 and lives outside the repository, with its passwords in a 0600
`key.properties` beside it, until the vault can hold it (#252); its loss
would mean no update could install over the friend's app, so the vault
copy is owed. Fugo's ruling: any agent running as Matt's user can read it
there (medium-low: a forged update still has to reach the phone); once
#252 lands it moves into the vault, granted only to whoever builds
releases, without ask-each-time (Matt: nothing that puts him in the loop
for each use), and the file here is deleted. Before
that, Matt keeps an offline copy: losing it is the irreversible risk. The build reads it with `-PclaktoryKeyProperties=FILE`
(or `ORG_GRADLE_PROJECT_claktoryKeyProperties`). For Google Play later,
the same key goes to Play App Signing as the app's own, so installs from
the link and from Play stay one app.

`versionCode` has to rise with every APK, or Android refuses the update:
Claktory's version, major × 10,000,000 + minor × 100,000 + patch × 100 +
a rebuild count (v0.1.15 → 101500), with `--build-name` the version.
`tools/release_apk.sh vX.Y.Z [REBUILD] --key FILE --out DIR` does all of
it, and also stamps `FACTORY_VERSION` into the Dart: without it the app
calls itself "dev", which turns off its version checks against the
backend and shows "dev" in its footer (the first friend APK, 101500, was
built so; 101501 replaced it).

### Links and the clipboard

- Any app can fire `claktory://add?code=…` at us. A link never connects
  to, or switches to, a Claktory without asking, as on the Mac; the
  question names the Claktory and says it's a new one. Katya binds it;
  the host only delivers the intent. Only verified App Links if https
  links are ever used.
- Anything sensitive the app copies (a pairing code) is marked
  `ClipDescription.EXTRA_IS_SENSITIVE` (API 33+), so it isn't previewed
  or synced.

Viktor reviewed §4 and §5 on paper (no changes to §5) and keeps them in
the threat model as "design reviewed, proofs owed in A.2". Private DNS is
not a security boundary: relays are untrusted and the backend's key is
pinned, so spoofed DNS costs availability and leaks metadata only.

## 6. The artifact gate

`tools/check_artifact.py` is the Android cousin of `scripts/check-macho.py`:
it refuses an artifact that would fail on a phone, from what's in the file,
before it goes anywhere. Arjun's `scripts/android/verify.sh` runs it on
every release candidate; the rules are mine.

For every `.so` (alone, in an APK, a split-APK set or a bundle):

- 64-bit little-endian shared object; machine matching its `lib/<abi>/`;
  only the ABIs the app ships.
- Every LOAD segment aligned ≥ 16 KiB, offset and address congruent.
- RELRO: the range the loader makes read-only, rounded out to 16 KiB, may
  not cover writable data outside RELRO. (Not "RELRO must end on 16 KiB":
  Flutter's own engine ends mid-page with nothing writable after it, and
  loads fine.)
- NDK-linked libraries carry `.note.android.ident` with an API ≤ minSdk.
- Dependencies are Android's own libraries or shipped beside it.
- The tunnel exports what Dart and Java look up by name: FRB's core
  symbols, every `frbgen_*` in the generated Dart binding (read from it, so
  regenerating can't drift), and the bootstrap's JNI method.
- No `.debug_*` sections shipped (AGP strips them; if it can't find the
  NDK's strip it warns and ships them unstripped, and this catches it).
- No home-folder paths in the binary (`/Users/<name>/`, `/home/<name>/`):
  the hook remaps them to `~`.

For every APK: libraries stored uncompressed at 16 KiB offsets (what
`zipalign -c -P 16` checks; `extractNativeLibs` stays false), every shipped
ABI folder holding the tunnel, Flutter and (release) the Dart code, and
none without them. For a bundle: the same per ABI, plus bundletool's
`dump config` must show `PAGE_ALIGNMENT_16K`; without bundletool it fails
closed.

**Evidence so far:** 29 tests (`python3 -m unittest discover -s
app/android/tools`), synthetic fixtures for each failure (4 KiB LOAD,
RELRO overlapping writable data, wrong machine, API too new, missing note,
unshipped dependency, missing FRB/bootstrap export, debug info, executable,
compressed or misaligned APK entries, a stray 32-bit ABI, an ABI without
the tunnel, no libraries, an unchecked bundle, a bad split). LLVM's
`objdump` reads the fixtures as the verifier does, and the verifier runs on
Flutter 3.50's real release `libflutter.so` for arm64 (from the engine
cache): API note 24, 64 KiB alignment, dependencies all Android's, RELRO
fine; its only problem is the debug info AGP strips. The macOS build of the
tunnel exports exactly the FRB set the verifier requires.

**Still owed before it's trusted on our own artifact:** the first real
Android build run through it and through the NDK's tools (`llvm-readelf
-lW`, `zipalign -c -P 16 -v 4`, `bundletool dump config`, Google's
alignment check script), and differences resolved in the verifier, not
hand-waved. Then the release install on a 4 KiB and a 16 KiB system
(`adb shell getconf PAGE_SIZE`), not page-size compatibility mode.

## 7. The boot proof, step by step

Once Arjun has installed the tuple (Matt's approval, through Mike):

1. `doctor`: the machine matches `toolchain.properties`, no ambient
   overrides.
2. Tomás lands the §3 handoff; I land `android.rs` with its host tests.
3. Generate and review the host (§2), dev flavor, debug then release.
4. Verifier on the debug APK, then on the release AAB and its split APKs
   from bundletool, the bad fixtures still refused.
5. Release APK on an arm64 emulator (and a 16 KiB image if this Mac runs
   one): process starts, bootstrap installed (asked through FRB), the
   tunnel opens to an isolated dev or QA Claktory, the app connects
   direct and through a relay, and reconnects after a network change.
6. Write the manifest: source hash, the tuple as found, every artifact's
   SHA-256, verifier output, kept beside the build with matching symbols
   and the R8 mapping.

### Results (2026-11-21, emulator only)

Toolchain as pinned (Temurin 21.0.13.1, SDK 36, build-tools 36.0.0, NDK
r28c, emulator 37.3.10, one Android 36 arm64 google_apis image, 4 KiB
pages). Against the dev Claktory:

- The tunnel cross-compiles for `aarch64-linux-android` at API 24; the
  verifier and `llvm-readelf` agree on it (16 KiB LOADs, API note 24,
  libc/libm/libdl only, FRB and bootstrap exports, no home paths).
- Debug and release (R8) APKs, arm64-v8a + x86_64, pass the verifier and
  `zipalign -c -P 16`. The release AAB passes too (bundletool 1.18.3:
  `uncompressNativeLibraries` with `PAGE_ALIGNMENT_16K`), and so do the
  split APKs bundletool makes from it.
- 16 KiB: those split APKs, installed with bundletool on the Android 36
  `google_apis_ps16k` image (`getconf PAGE_SIZE` 16384), run natively
  (`pageSizeCompat=0`): the DNS count, pairing, Allow, the conversation.
- Release APK on the emulator: the bootstrap installs before Dart, and
  iroh's resolver reads the phone's DNS (`The private connection reads 1
  DNS servers from this phone.` in logcat, the count of
  `nativeSystemDnsServers`); FFI loads the same library (`open_tunnel`'s
  `ready()` passes); pairing by pasted code, Allow, the workspace, and a
  message sent and shown back ("Sent from Android emulator release").
- Direct ("Connected directly…") with the code's addresses; relayed
  ("…by an indirect route…") with a code stripped of them.
- factory/network: Wi-Fi off, the tunnel was back over cellular in ~1.5 s;
  Wi-Fi on, back in ~5 s. NetworkPath's JVM tests 5/5.

Findings:

- `flutter build` builds armv7 too unless told `--target-platform
  android-arm64,android-x64` (Gradle's `abiFilters` doesn't reach Flutter's
  assemble), and the hook then fails for want of an armv7 Rust target. The
  build scripts must always pass it.
- Relay-only, the first connection after Allow took ~2 minutes of 8-second
  dial timeouts before it came back (direct: at once). Not diagnosed: the
  app has no Rust logs on Android. Watch for it on a phone.
- Flutter warns that a launch intent not sent by the app itself "will be
  IGNORED in release builds" (stricter entrypoint verification): matters
  for links (§5), not for the launcher.
- Shared, not native: a pasted code with a broken %-escape throws inside
  the paste form's build (`parsePairingCode` → `Uri.queryParameters`), and
  the form disappears; `phone_test.dart` taps the first "Builder", which
  on Android is the header (agent settings), so it never finds Send.
- The connection isn't kept across launches yet (no `factory/device`
  store until A.2), and the backend sees `kind=Unspecified`.

Emulator success ends at "the build works". **Native acceptance stays open
until the same release build does this on a real phone** (Matt's, or one
he provides), on 4 KiB and 16 KiB systems.

## 8. Contract notes for the capability adapters (from Katya's B.1)

- `NativeRecorder.access()` (`recorder.dart`) maps only `granted`,
  `undetermined` and `restricted`; any other string, `unavailable`
  included, becomes denied. A phone without a microphone must surface as
  unavailable through a `PlatformException`, or the Dart mapping changes
  first (Tomás's file); don't answer `unavailable` and assume it's honored.
- `DeviceCamera` maps every `CameraAccess` name, but its denied screen
  assumes Settings is the only way back. Android can ask again after a
  first denial (until "don't ask again"), so the native side has to tell
  "can still ask" from "Settings only" and the shared UI has to map it,
  deliberately, with Katya and the file's owner.

- `factory/device` `capabilities` and the `factory/device/keyboard` event
  channel (`mobile/device.dart`, d0feb006) are one schema for every
  platform; Android answers the same map (fixtures in
  `app/test/ipad/fixtures/bridge/capabilities_*.json`), no Android-only
  one: `schema_version` 1, `idiom`, `ios_app_on_mac` false,
  `os_version`, `hardware_keyboard` (a physical keyboard attached and not
  hidden, from `Configuration`), and keyboard events
  `{hardware_keyboard: bool}` on change. `idiom` is the truth: `pad` when
  `smallestScreenWidthDp` ≥ 600, else `phone` (Katya's recommendation,
  pending Mike). Layout doesn't follow it on Android: phone or not is
  still the window's width, and any iPad workbench needs its own platform
  gate. Today `loadDeviceCapabilities` asks the channel only on iOS;
  Android reads the fallback until an owner-reviewed gate change lets it
  ask.

## 9. Open points for Mike

- **Home-folder paths in shipped binaries:** fixed for the Mac and iPhone
  by Tomás (edf589a, `--remap-path-prefix=$HOME=~` per target); Android's
  triples get it in the same function. The verifier now refuses a library
  naming a home folder.
- **Private DNS bypass** (§4): a product call, later.
- **Nothing in `app/android/` lands on main until the host exists.**
  Flutter treats an `app/android/` folder as the Android platform being
  there: from then on `flutter pub get` writes Android plugin files into
  it in every checkout. These files stay on `factory/android-native` until
  the generated host, with its `.gitignore`, lands with them.
