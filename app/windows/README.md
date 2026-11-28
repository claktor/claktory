# Claktory for Windows: the app's Windows runner

The Flutter app's Windows host (windows.md, Part I section 5; Part II B: Avdol's).
The same `app/lib` runs here as on the Mac. What's Windows-specific is the window and
the channels the Dart side talks to the system through. Each channel keeps the Mac's
names and messages (`app/macos/Runner`), so `app/lib` keeps one code path.

## Building

On Windows 11 x64, with Visual Studio Build Tools (C++ workload, Windows SDK), CMake, Git,
Rust (`x86_64-pc-windows-msvc`, for the app's tunnel library) and the Flutter version
`app/.metadata` names:

Build from a folder Defender skips (on the lab laptop, `C:\build\<name>`), with TEMP there
too: the tunnel's Rust build goes to TEMP, and is several times slower anywhere scanned.

```
cd app
flutter build windows --debug                                  # Claktory Dev (debug)
flutter build windows --release --dart-define=FACTORY_DEV=true  # Claktory Dev for the dev Claktory
```

A Profile or Release build carries the Visual C++ runtime it needs beside `Claktory.exe`
(`msvcp140.dll`, `vcruntime140.dll`, `vcruntime140_1.dll`, app-local as Microsoft allows),
for the app, its plugin and `claktory_tunnel.dll`, so it starts on a Windows without the
VC++ redistributable; the UCRT is part of Windows. A Debug build uses the debug runtime,
which only a machine with Visual Studio has: never give one to someone without it.

The real Claktory is built only by the release scripts (Pericolo's), with
`CLAKTORY_FLAVOR=real` in the environment and `--release`.

## Flavors

`CMakeLists.txt` chooses one; `runner/flavor.h` names what follows from it:

| Flavor | Built by | Name shown | App id (AUMID, window class, mutex) | Folder in `%LOCALAPPDATA%\Claktory` | Icon |
|---|---|---|---|---|---|
| real | `CLAKTORY_FLAVOR=real`, Release | Claktory | `dev.factory.app` | `app` | `app_icon.ico` |
| dev | `--dart-define=FACTORY_DEV=true` | Claktory Dev | `dev.factory.app.dev` | `app-dev` | `app_icon_dev.ico` |
| debug | anything else (`flutter run`, a plain build) | Claktory Dev | `dev.factory.app.debug` | `app-debug` | `app_icon_dev.ico` |

The ids are the Mac's bundle ids, as AGENTS.md keeps internal names. The installer's
Start menu shortcut should carry the same AppUserModelID, so the taskbar groups the
running app with it.

Notifications need Windows to know the id. At start, the app registers it for this account
under `HKCU\Software\Classes\AppUserModelId\<id>`, with DisplayName, IconUri
(`data\notification_icon[_dev].png` beside the exe) and CustomActivator, as the Windows
App SDK does for unpackaged apps. CustomActivator is the flavor's toast activator CLSID
(`flavor.h`): real `{B92913F3-A773-47B2-93D2-0F951B7A1399}`, dev
`{8058C1B5-FD67-439A-882C-5A7F685812FE}`, debug `{B376FC2C-E72D-4024-AA71-68CD59CF51BC}`,
whose `HKCU\Software\Classes\CLSID\<clsid>\LocalServer32` names the exe, so a toast
clicked while the app is closed starts it (`toast_activator.h`). Each start rewrites what
changed, so a moved install fixes itself. It works without a shortcut. The uninstaller
must remove both keys: `AppUserModelId\<id>` and `CLSID\<clsid>`.

`python3 app/windows/tool/make_icons.py` (on a Mac) remakes both icons from the Mac's.

## What the app keeps on this PC

Only the device-local state tenet 7 allows, in `%LOCALAPPDATA%\Claktory\<folder>`. The
folder is owned by this account, with a protected DACL: this account and SYSTEM only, so
other accounts can't list or read it. An administrator can still take ownership. A folder
left by an older build is used only if it's a plain folder this account owns, and its DACL
is made private again.

- `connections.dpapi`: the connections (`factory/device` connections, setConnections,
  resetConnections). The JSON the Dart side hands over, sealed with user DPAPI
  (`CryptProtectData`, never prompting) after a `CLAKTORY-DPAPI-1` header. Its fixed
  entropy only keeps a generic unprotect-any-blob path from opening it; it's no
  protection, since any program running as this account can read it from the binary.
  The rules are the Mac's `ConnectionStore.swift` (`runner/core/connection_store.h`):
  - Before the first read, and after a failed one, nothing is written over it
    (`credentials_read_failed`) until a read works or the person starts over.
  - There is no plain-text fallback.
  - Failures are `credentials_unavailable` (try again), `credentials_unreadable` (another
    account's, damaged, or its key reset), `credentials_write_failed` or
    `credentials_reset_failed`.

  DPAPI keeps this from other accounts and from an offline disk, not from programs
  running as this account (windows.md section 3). Under authority B it holds only
  device credentials from the account path and the account key: never an unspent
  pairing or connect code, and no DM key material (Fugo, windows-constraints section 12).
  The store itself doesn't look at what it keeps, so that rests on the Dart side.
- `drafts.json`: unsent drafts (`factory/device` drafts, setDrafts).
- `window.json`: the window's last frame and whether it was maximized (IDENTITY, Window).

Every file is written whole or not at all: a copy beside it, flushed, then moved over
(`runner/file_store.h`).

## Channels

| Channel | Methods | Here |
|---|---|---|
| `factory/device` | connections, setConnections, resetConnections, drafts, setDrafts | Done (`device_channel.cpp`) |
| `factory/device` | qrCode | Not implemented on purpose: no code is shown or read under authority B |
| `factory/device` | webAuth, cancelWebAuth | Not implemented: desktop sign-in uses the loopback receiver (`lib/web_auth.dart`) |
| `factory/device` | cacheFolder, downloadsFolder, showFiles | Done: the viewers' cache (`Cache` in the app's folder), Save to Downloads and Show in Explorer |
| `factory/device` | shareFiles, defaultBrowser | Not implemented: R1 saves to Downloads; only Safari needs defaultBrowser |
| `factory/window` | drag, zoom, band, devMark, pickFolder, pickFiles, takeFiles; back: filesOver, filesDropped | Done (`window_channel.cpp`, `file_dialogs.cpp`, `drop_target.cpp`) |
| `factory/notifications` | listen, post, permission, ask, openSettings; back: using, clicked | Done (`notifications_channel.cpp`, `toast_activator.cpp`): toasts, and their clicks, the app running or not |
| `factory/sight` | state, seen; back: sight | Done (`sight_channel.cpp`, `presence.cpp`): not shown while minimized, on another desktop, locked or with the display off (Windows tells no covering); reduceMotion is Animation effects off |
| `factory/network` | events: satisfied, interfaces, expensive (metered), constrained (over its limit), network | Done (`network_channel.cpp`) |
| `factory/links` | listen; back: opened | Not in R1: its links (add, add-friend) carry codes, which the app never takes under authority B |
| `factory/setup` | location, install, uninstall, appOnly, setAppOnly, backgroundStatus, openLoginItems | Done (`setup_channel.cpp`): runs this PC's `factoryd.exe install` and `uninstall`, which own the sign-in task; removeApp and moveToApplications aren't Windows' (Installed apps removes Claktory) |
| `factory/engine` | menu; back: menuChosen, quitting | Done (`engine_channel.cpp`): Stop or Start Claktory in the window's system menu, and closing the window asks quitting first |
| `factory/secret-clipboard` | copy, clear, sweep | Later, if an allowed secret is copied |
| `factory/recorder`, `factory/audio` | voice | Phase 5 |
| `claktory/video`, `claktory/ink`, `claktory/replay-writer`, `factory/dm-keys`, `factory/browser`, `factory/setup-link` | | Not in R1 (windows.md section 5) |

A channel or method with no handler answers "not implemented", and the Dart side takes
its `MissingPluginException` path, the one it takes on any platform without the
feature. The Dart side must also gate each surface so it never offers what Windows
can't do (windows.md: truthful unsupported states).

## Title bar

For now, the window keeps Windows' own title bar. It's colored with the app's sidebar,
text and hairline colors, dark or light as Windows is set (Windows 11; Windows 10 keeps
its own colors). The app's 44 px band sits below it and also drags the window (`drag`,
with Snap) and maximizes it on a double-click (`zoom`).

docs/IDENTITY.md wants no empty band above the header bars, with only the window
controls native. That needs the title bar drawn into the client area, with the system's
caption buttons kept at the top right. Two ways to try on the laptop:
- DWM's own buttons in an extended frame, with the Flutter view's region cut around
  them;
- buttons drawn by the runner, with `WM_NCHITTEST` answering `HTMAXBUTTON` for Snap
  Layouts.

Either way, the app's right-hand header bar must leave room for the buttons: a
`trailing` inset on `factory/device/window_controls`, a shared Dart change.

## Log

The runner logs lines starting `Claktory: `. They go to OutputDebugString (DebugView,
Visual Studio) and stderr (`flutter run`). When `CLAKTORY_LOG_FILE` names a file, they're
also appended to it, for runs with no console or debugger. The log never holds a secret.

## Tests

`runner/core` is standard C++17. Its tests (`test/core_test.cpp`) run anywhere:

```
app/windows/test/run_core_tests.sh                      # Mac or Linux
cmake -S app/windows/test -B build/windows-core-test    # Windows
cmake --build build/windows-core-test --config Debug && build\windows-core-test\Debug\core_test.exe
```

No x86 builds run on Claktory's own Mac (Matt's rule, Bruno 48601): the two scripts below
are for a remote build machine (prime) only, never this Mac. Windows builds otherwise run
on the Windows laptop with Visual Studio's build tools.

Every runner source compiles for Windows on a Mac too, in all three flavors, without
building anything (clang-cl, Windows' headers from cargo-xwin's cache, Flutter's Windows
headers fetched once): `app/windows/tool/check_compile.sh`. It catches what MSVC would
refuse early; the Windows build still decides.

Until a Windows machine has Visual Studio's build tools, `app/windows/tool/cross_build.sh`
(`--dev` for the dev flavor) builds a debug app on the Mac: the runner and its plugin with
clang-cl and lld-link, Flutter's debug engine for Windows, the Dart code as a debug kernel.
It leaves `app/build/windows-cross/Claktory` and a zip of it. It has no private tunnel
(the Rust build hook wants Visual Studio), so connecting through one fails there.

`windows_smoke.exe` (`test/windows_smoke.cpp`) is built beside the app on its own, after
`flutter build windows --debug`:

```
cmake --build build\windows\x64 --config Debug --target windows_smoke
```

(the cross build makes it with every debug build). On
Windows, with no window, it checks the private folder (a new one, one already there made
private, a junction refused), the store over real DPAPI and the drafts file. Over ssh with a
key DPAPI isn't open (no password signed in: "Access is denied"), so the store's part says
SKIPPED there; run it in the account's desktop session for that part.

What needs Windows itself (DPAPI, the dialogs, drops, the window) is checked on the lab
laptop and in Hermes's acceptance runs.
