# Android behavior contract — B.1–2 and Dart accessibility

These are behavior adapters behind the shared phone policy, **not an Android-ready app**.
No SDK, emulator, hardware, native codec, Keystore or system gesture acceptance
is claimed here. There is no second PhoneShell, product state store or Material
restyle. Geist, Claktory's colors, custom controls and backend authority stay put.

## B.2 implementation status

The independently testable Back host and identity helpers landed in `d032ca3c`
and `344eec30`. `back_host_test.dart` sends real StandardMethodCodec
`flutter/backgesture` and JSONMethodCodec `flutter/navigation` messages through
Flutter's binding. It proves both-edge cancellation restores a real route's
animation and state, a commit pops once, a late overlay prevents a stale pop,
IME ownership survives a zero-inset race, three-button ordering/root delegation,
ahead-of-time eligibility, reduced motion and same-key Page content updates.
These are **framework-channel widget tests, not Android OS/device tests**.

`AndroidBackHost` is the sole event dispatcher. Flutter 3.50 broadcasts predictive
Back to *all* registered observers, so the Android page deliberately uses
MaterialRouteTransitionMixin with FadeForwards transitions, not the additional
observer in the default PredictiveBackPageTransitionsBuilder. The host drives
the captured page's controller for preview and restores it before registry
commit. It never calls the route's unchecked handleCommitBackGesture pop.
NavigationNotification publishes the shared registry's aggregate eligibility;
inner Navigator notifications cannot override sibling sheets/drawer state.

Dispatcher invariants (addressing Nora's review):
- Preserve the captured attempt across `inactive`/resume (a notification shade
  or permission blip). Cancel preview without navigation on hidden, paused or
  detached; a late terminal callback after actual backgrounding does nothing.
- A button Back received during an active swipe is consumed without committing
  or replacing that swipe. Its own commit/cancel is authoritative; a separate
  button after completion works normally.
- Restore a driven route only while its Navigator still exists and is mounted;
  teardown is not permission to touch a disposed animation controller.
- The newest engine-local host generation owns native Back eligibility and
  dispatch. Old queued publications and captured gestures cannot override or
  navigate under its replacement. Disposing the current host releases native
  eligibility to false without looking up ancestors from a deactivated context;
  disposing an older host cannot reset its successor. Three handoff regressions
  plus the existing active-preview teardown case pin these invariants.
- Android phone pages must not install an independent predictive observer.
  Their `popGestureEnabled` is deliberately false even under the SDK's default
  predictive theme; the host, not this flag, advertises eligibility and drives
  preview. A regression tripwire checks this plus actual host-driven progress.
  Any new nested Navigator/dialog must join the same registry/dispatch contract,
  not add another independent predictive handler.

The Dart phone binding is in place: Mei's registry/policy and inset hooks in
`5c46eaa4`, Android adapter in `7cf84e16`, Back delegation in `34d486bc`, core
controls/inbox targets via Priya in `9932f51f`, and composer targets via Tomás in
`af967686`. Device naming dispatch landed via Zoe in `ed158845`; pre-binding
lookup safety and unknown-device fallback have dedicated plain tests.

`foreground_core_test.dart` reaches the actual shared PhoneShell surfaces with
Android policy explicitly enabled: Menu/drawer, Settings, Extensions, dirty
Agent settings and Keep editing, editable and read-only transcript selection,
composition/Copy/Paste/explicit Send, 48dp core actions and approval, cutout/IME
ownership across portrait/short-landscape resize, Needs you, ChatView, and
fake-camera Paste/manual Connect. Root localization delegates mirror main.dart
so selection's root-overlay toolbar is tested, not masked by a field-only locale
override. These reached-surface tests are not screenshots, a native scanner, a real
backend connection, or OS gesture/IME evidence. The suite also separates
capability facts from layout width, proves an Android `pad` still uses the
shared wide Shell rather than the iOS workbench, and compares 48dp wide Android
control primitives with unchanged 28px Mac glyphs/8px resize handles (19 tests).

Mei's shared `touchFirst` hook (`d0feb006`), Priya's wide controls/resize targets
(`b32019e3`, `947f585`), and the wrapping sidebar footer (`379b7130`) keep core
control density independent of phone composition. Native capabilities are facts:
Android tablets report `pad`, but presentation remains platform/width gated.
`loadDeviceCapabilities` still queries native channels/keyboard events only on
iOS; Mei owns its eventual Android gate extension when Raj's host is ready.

Still owed: native storage/scanner, all live OS/device evidence, full supported
locales/accessibility, and complete tablet/foldable/cross-tier route restoration.
Phone portrait/short-landscape resize and wide core primitives are covered;
this package does not claim full tablet/foldable foreground acceptance.

Reached-surface tests found two shared defects; owners fixed them rather than
Android code crossing merge boundaries: page/sheet nested MediaQuery removal
reintroduced the original IME inset (240dp observed where children should see
zero), and a Needs you approval action row overflowed a 393dp sheet by 23px.
Mei fixed the combined inset transformation in `5c46eaa4` and ActionLine layout
in `be7c5d8d`.

Scope: Research `android.md`, Part II.B.1–2 and ongoing Android accessibility
coverage (the research file lives outside this repository); shared phone
foundations are in `../mobile/`.
Source inventory baseline: `682c7e2e`.

## Dart accessibility and large-text evidence

`accessibility_test.dart` adds 25 reached-surface widget tests with Android
policy enabled and bundled Geist/GeistMono fonts. `support/android_harness.dart`
shares the main-like root delegate/overlay placement with the foreground suite;
it does not introduce a product screen or navigation state store.

The assertions cover:
- Simulated accessibility traversal: Menu, the full agent name/role, Extensions,
  Attach files, the named editable field and Send, as separate nodes in order.
  Conversation/drawer targets meet Flutter's Android 48dp and labeled-target
  guidelines. A long visual header may ellipsize but keeps its full spoken label.
- Semantic activation reaches the drawer, Settings, Agent settings, Keep editing,
  approval and Connect. Search retains its input label while semantic editing
  filters the drawer; Clear search independently restores the list. Underlying
  conversation semantics disappear behind the drawer, pushed Settings and
  Extensions sheet, and return after dismissal.
  The Extensions heading names its sheet route; composer focus is isolated while
  the sheet is open and restored afterwards without losing the draft.
- The composer keeps one meaningful input label even with a custom rotating
  placeholder. Semantic activation focuses it, and semantic text editing keeps
  its value, selection/edit actions and draft without sending, including an
  initially valid selection with no keyboard-inset change. Editable and
  read-only selection both expose working Copy. Approval and Send produce exactly
  one request to the intended backend destination; disabled Connect announces its
  disabled state and has no tap action.
- Linear text scales 2x and 3x at 360×640 and 640×360 reach Settings/dirty forms,
  Chat, Needs you approval and manual Connect without overflow. Chat input and
  Send stay above synthetic 240/120dp keyboard insets. These are linear widget
  stress profiles, **not** evidence of Android's nonlinear font scaling or actual
  display-size settings.

Shared fixes remain with their named owners: Priya's Button roles/disabled state,
optional TextField label, field-box activation and focus-driven semantics
publication (`e1e6883`, `818e70d`, `dcc9100`, `106fe70`); Tomás's persistent message
labels, Extensions route name and full-box composer activation (`0eac1348`,
`e307310c`, `66277d6c`, `6495ae87`); and Mei's drawer targets/search and sheet
focus/actions (`a746124b`, `c801f547`). Katya owns only this Android test coverage
and adapter documentation. Test-only changes add no release claim.

**Still owed:** live TalkBack traversal, announcements and activation; accessibility
focus (distinct from Flutter input focus); Switch Access; supported languages/RTL;
real IME/display scaling and physical-device geometry. Simulated semantics order,
widget focus and fake channel success do not close those native acceptance gates.

## Recent shared-surface parity audit

`parity_audit_test.dart` adds 28 Android widget tests for Move, secret request
approval/collection, chats and the Mac-only drop boundary. It reuses the real
Shell and Android Back host; an optional root drop overlay mirrors main.dart.
No duplicate screens or fake successful desktop channels are introduced.

The freeze lifetime gate is structural: Mei's `f37b2934` puts the whole window
Overlay beneath MoveGate and completes torn-down surfaces with null. Both the
Move form and an unrelated Extensions sheet disappear with their window; a
fresh overlay is created after a canceled move. The Android host's generation
and disposal regressions independently pin native Back eligibility handoff.

Twenty cases cover 360×640 and 640×360 at linear 2x/3x text with bundled fonts:
- Settings → General → Move opens the actual MoveSettingRow and MoveForm. The
  code, digits confirmation and Move now send exactly one corresponding request.
  With synthetic 240/120dp IME insets, at least 48dp of the input's actual target
  intersects its scroll viewport; the field and Connect remain reachable.
- A frozen move replaces PhoneShell with MovingScreen. Framework Back falls
  through to SystemNavigator, never CancelMove; only explicit Cancel move sends
  that request. Done removes the cancellation action. Backend phases are injected
  status/snapshot updates and requests are recorded, **not** a live move/transport
  handshake.
- Real transcript secret cards approve only the need (no secret entries), or
  collect an explicitly supplied synthetic value for the correct agent/request.
  The value keeps its variable label and obscured semantics; IME Back preserves
  it without deciding. Only explicit Add and give sends it.
- Chat rename preserves edited text on predictive cancellation, consumes IME Back
  without closing the form, and saves to the right chat. Back dismisses Delete's
  confirmation without deleting; explicit Delete sends one request. Chats expose
  neither attachment nor voice actions.

The other eight cases prove Move's canceled gesture and IME/sheet Back separation;
freezing from either an open Move form or an unrelated Extensions sheet takes
all transient surfaces with the old window and releases native Back eligibility,
even during a captured gesture; a blocked chat's honest no-model explanation at
3x in both orientations; no Mac drop overlay/native accept on compact or wide
Android, even with a stale drag signal; and a missing Android picker reporting
that attachment isn't available
rather than invoking the Mac picker. Desktop-channel handlers record calls and
throw MissingPluginException: they never simulate successful Mac APIs.

Shared fixes stay with their owners: Kenji's Move input/layout (`df8ff209`),
modal heading/route name (`b0f70819`) and Mac-drop gate (`e5eb0b21`); Tomás's
picker dispatch/unavailable explanation (`7351c789`), transcript Latest target
(`38424084`) and responsive blocked composer (`bd0e533b`); and Mei's form-bar
targets and persistent secret input labels (`9a1dc2fa`). Incidental General-page
residuals have a named owner and are closed by Zoe's
`6b109aaa`: ColorPicker/avatars take the Android 48dp floor (iOS 44/Mac 26 stay
unchanged), and the name input carries its own “Claktory name” label.

Still owed: Android's native Photo Picker/SAF binding (Raj, deliberately SDK
gated), real move transport/reconnection, OS Back/IME and physical-device tests,
TalkBack announcements/accessibility focus and nonlinear font/display scaling.
The Android app is not declared ready by this Dart-only parity audit.

## Shared seam agreement

Mei and Katya agreed by team message during B.1; Mei alone lands the shared
files and bindings. These are the agreed shapes, **not declarations added by
this package**:

- `mobile/platform_policy.dart`: `PhonePlatformPolicy.of(context)` selects by
  platform. `touchTarget`, `drawerEdgeSwipe`, `page<T>({required LocalKey key,
  required Widget child, bool fullscreenDialog = false})`,
  `selection(context, extra: ...)` and `localizations` preserve iOS defaults.
  Selection returns the existing controls/contextMenuBuilder/magnifier record,
  retaining `ExtraEditItems<EditableTextState>` for Comment/Suggest. Mei also
  agreed `selectableMenu({extra})` for read-only `SelectableRegion` menus,
  preserving the existing iOS helper unchanged.
- `mobile/back.dart`: `PhoneBack` is a `ChangeNotifier`, above PhoneDrawerHost.
  `register(BackToken)` returns a `BackHandle` with idempotent `release()` and
  `changed()` for dismissibility updates.
  `BackToken` exposes `bool get canClose` and `void back()`.
  `target` returns the newest registered token, then the open drawer, then the
  top pushed page, else null (Android owns root Back). A chat, like an agent
  conversation, is a root, not an extra Back target.
- `BackTarget.canClose` is exported, as is `TokenTarget.token`. Token occurrences,
  drawer open-generations, and page route-id/push-serial pairs are identities.
  `commit(capturedTarget)` does nothing if that occurrence has gone; it must not
  act on its replacement. It must not pop a page under a newly appeared modal.
  Changes to target/dismissibility notify listeners before the next gesture.
- A dirty token remains the target even with `canClose == false`. Committing
  invokes its `back()` to ask explicitly whether to discard; begin and cancel
  do not ask, close, clear the form or mutate drafts.
- **The IME has no registry token.** Android normally consumes Back for its
  keyboard before Flutter. Never hide the IME and then fall through to a page.

`AndroidBackCoordinator<T>` binds with three closures: `readTarget` reads
PhoneBack.target, `canClose` reads BackTarget.canClose, `commitTarget` calls
PhoneBack.commit. It captures one attempt, including IME ownership at begin,
then finishes it exactly once. A zero keyboard inset arriving during completion
cannot turn an IME-owned attempt into page Back. The shared registry, not this
adapter, owns ordering and stale-target validation; the tests supply a fake of
that contract. No duplicate production registry is introduced here.

### Dispatcher integration rules

- Publish native pop eligibility ahead of time with the registry's aggregate
  `NavigationNotification`, forwarded by WidgetsApp to SystemNavigator. This
  app has a builder/Overlay root, not an enclosing Navigator route for PopScope.
  If one is added later, coordinate its PopScope/NavigatorPopHandler rather than
  adding another pop owner. No asynchronous `WillPopScope`.
- `canClose` is **not** automatically the outer Navigator's `canPop`. An overlay
  blocks the outer route pop while its Back callback dismisses only that layer.
  A dirty form blocks dismissal and asks on committed Back. A native page pop
  already reported by `didPop == true` must not also call shared commit.
- Use a gesture-begin snapshot for predictive Back. Three-button Back captures
  and commits once. Progress is presentation-only. Cancellation restores the
  visual position with no action. Never re-target at commit.
- Do not infer IME ownership from keyboard *focus*: a hardware keyboard or an
  unfocused-but-closing IME is different. Bind actual Android/Flutter keyboard
  visibility/lifecycle and prove event ordering live; the boolean fake does
  not validate dispatch from the OS.
- At root allow normal Android Back/home behavior, including predictive home.
  Never stop agents, mutate remote layout, delete comments, clear drafts or
  automatically send messages as a Back side effect.

## Platform behavior

`AndroidBehaviorPolicy` supplies implementation helpers for Mei's seam:

- 48 logical pixels/dp minimum interactive/focusable area, **also in wide touch
  windows**. Visible glyphs stay small; grow the hit region, not the branding.
  Dense drawer rows, header actions, chips, sheet choices, media controls and
  comment pins must all meet this. The value alone does not migrate existing
  `Touch.target`/hardcoded 44/36/28 controls or prove their geometry.
- Disable leading-edge drawer **opening** on Android; Menu remains accessible.
  Left and right edges belong to system Back, including RTL. Do not add broad
  gesture-exclusion rectangles. Map/media/code/selection drags stay local.
- The Android Page uses Flutter's MaterialRouteTransitionMixin and ordinary
  Android FadeForwards transitions, driven predictively by the single Back host.
  This is not a MaterialApp, scaffold, font or product navigation redesign.
  B.2 framework-channel tests cover preview/cancel/commit and reduced motion;
  real OS transition/dispatch acceptance remains owed.
- Android selection handle controls, adaptive toolbar and magnifier preserve
  Copy/Paste/Select all plus supported extra Comment/Suggest actions. A widget
  test reaches an actual EditableText toolbar and executes Comment. Live native
  selection, read-only selection and TalkBack acceptance are still owed.
- Existing `InputKind` setups stay shared: message Return inserts a newline,
  explicit Send sends; composition must not be submitted early. Email/autofill,
  the single code field, secure text, emoji, non-Latin composition, paste,
  hardware Ctrl+Return, dictation and a second IME need live checks. Streaming
  must not steal focus or move the caret.
- `DefaultMaterialLocalizations.delegate` is an **English fallback only** for
  WidgetsApp. Mike explicitly deferred flutter_localizations/global delegates
  and supported-language decisions beyond B.1. Full locale/RTL toolbar support
  is owed. Mei added direct material_ui with the seam; Android implementations
  and their tests now import that supported package, not deprecated SDK exports.
- Exactly one page or sheet owns safe areas and keyboard obstruction. Reuse
  `_Screen`/PhoneSheetHost's existing max(bottom-safe-area, keyboard-inset)
  ownership and removal of child MediaQuery padding/insets. Never stack a
  second SafeArea/keyboard padding inside a composer. Background may extend
  edge-to-edge; content clears status bars, camera cutouts, gesture/button nav
  bars and the animated IME. Test changing all four edges during rotation.
- Window width is available layout space, **not** pointer/touch capability.
  Exercise short landscape, >=600dp tablets, split screen and fold/unfold;
  keep route/draft/scroll/selection without relying on orientation locks.

## Apple-only and desktop-fallback inventory

Audit search: all `Platform.is*`, `TargetPlatform.*`, `UiKitView`,
`CupertinoPage`, `factory/window`, `factory/menu`, `factory/setup` under lib
(excluding generated protocol/Rust bridge). Also inspected unguarded channel
adapters and page/sheet inset ownership. Paths below are relative to `app/lib`.
This is the baseline inventory, not permission for Katya to edit shared files.

| Source | Baseline behavior / Android action; merge owner |
|---|---|
| `phone.dart` | Nested CupertinoPage Navigator, no system Back hook; `_Screen` owns insets; PairScreen has iPhone/Mac wording and a 44-high link. Use policy pages/Back and neutral device wording/48dp. Mei. |
| `mobile/presentation.dart`, `controls.dart::showModal` | Sheets/actions use OverlayEntry and visibility counts, not closable Back targets. Wire occurrence tokens, dirty confirmation and focus/semantics restoration. Mei. |
| `mobile/drawer.dart`, `mobile/metrics.dart`, `mobile/header.dart` | Leading-edge opener, 44dp default, dense 36dp lines and phone-width-based metrics. Policy edge disabling plus 48dp for touch at every width. Mei. |
| `mobile/input.dart`, `selection.dart` | Only iOS gets handles/native menus; Android uses desktop selection. Bind editable and read-only Android helpers, including viewer Comment/Suggest. Mei seam; Tomás call sites. |
| `mobile/haptics.dart` | iOS-only feedback. Deliberate Android choices/voice feedback need capability-specific binding, not per-token feedback. Mei/shared voice owner. |
| `main.dart`, `pubspec.yaml` | WidgetsApp only Cupertino localization, cupertino_ui direct dependency; no Android policy binding. Mei holds dependency/seam additions; full localization explicitly deferred by Mike. |
| `scanner.dart` | iOS-only permission methods and UiKitView. AndroidView/texture preview plus lifecycle/debounce/permission binding required. Shared owner assigned by Mike; Raj native. Current Android gate has a documented baseline test. |
| `message_box.dart::_pick` | Resolved in `7351c789`: touch policies use factory/device pickAttachments, never factory/window pickFiles. Missing native support says “You can't attach files from this phone yet.” Native Android picker remains Raj's deliberate SDK-gated deferral. Tomás callsite; Kenji attachments. |
| `apps.dart::pickAppFile` | Baseline iOS-only device picker dispatch still needs its own audit/binding; the message-box fix is not evidence for Apps. Shared feature owner via Mike. |
| `attachments.dart`, `drop.dart` | Resolved in `e5eb0b21`: native window-drop accept/listen and overlay are explicitly macOS-only, not width-gated; Android gets no phantom Mac overlay or native window calls. Kenji. Device picker still returns filesystem paths; Raj must copy bounded content:// streams into private temporary files first. |
| `connectors.dart`, `web_auth.dart` | connectInSheet is Platform.isIOS; Android falls to desktop receiver/browser path. Use Raj's Custom Tabs/callback contract through shared dispatch, preserve backend OAuth state/PKCE. Shared owner via Mike. |
| `loopback_forward.dart` | iOS-only webAuth/closeSheet; Android uses launchUrl branch. Validate browser return/loopback forwarding, cancellation and process recreation with shared owner; not an automatic gate replacement. |
| `links.dart` | listen returns unless macOS; Android gets neither cold-start queue nor warm opened callbacks. Shared owner via Mike; Raj native. |
| `recorder.dart`, `audio.dart`, `viewers/video.dart` | No iOS gate in native adapters, but Android host is absent. Keep existing wire shape; no duplicate Dart recorder/player. Raj implements permissions/codecs/focus/lifecycle/texture cleanup; Tomás shared entry points. |
| `network.dart` | factory/network EventChannel (not a method), ordered preferred interfaces, movement debounce; FLUTTER_TEST global is empty. Raj supplies Android connectivity mapping; fake tests bind actual channel explicitly. Network availability is not proof tunnel connected. |
| `connections.dart`, `drafts.dart` | Unguarded factory/device reads/writes; connection storage includes account map. Propagate storage errors, don't reset/re-key silently; Raj encrypted envelope and separate drafts. Shared interfaces stay with existing owners. |
| `device_kind.dart`, `devices.dart::thisDeviceKind` | Android Hello kind unspecified/name empty; UI calls it Mac. Shared owner via Mike: phone/tablet kind and useful non-identifying Android name; no hardware identifier permission. |
| `compatibility.dart`, `update.dart`, `app_update.dart` | iOS-only phone words/installer guidance; wide Android can see desktop update banner; self-replacement code stages Mac bundles. Android must never invoke Mac installer/process paths. Separate frontend installer from backend Update behavior; shared owner via Mike. |
| `zoom.dart` | zoomsHere excludes only iOS; Android gets desktop zoom/menu/band registration and Ctrl-key help. Separate touch text/accessibility scaling from desktop window/menu APIs at every width. Shared owner via Mike. |
| `annotations.dart` | iOS-only tap/click wording branch and shared editor/selection hooks. Android must say tap, use supported selection actions and preserve comments/anchors. Tomás/Hana review. |
| `controls.dart::WindowDrag`, `pickFolder`, `TitleSpace` | Desktop window drag/zoom and local folder picker are unguarded or guarded only by phone layout. Audit wide touch layouts; remote FolderBrowser is not a local picker. Mei/shared owner. |
| `identity.dart::isPhoneLayout` | Already includes Android, uses shortest side <600. Keep phone landscape; decouple wide touch targets. Mei. |
| `connections.dart::hasLocalBackend`, `rail.dart` lifecycle, `screens.dart::_visible` | Already distinguish Android correctly as foreground phone/no local backend. Preserve; resume must reconnect truthfully. Existing owners. |
| `your_claktories.dart` platform tag, `viewers/image.dart`/`video.dart` touch predicates | Already include Android. No gratuitous predicate rewrite; verify behavior. Existing owners. |
| `main.dart::_canChooseLocal`, `setup.dart`, `remove_mac.dart` | macOS-only setup/local-backend choices are intentional; keep unavailable on Android, never create dummy desktop channels. Existing owners. |

`url_launcher` must work for privacy, Delete account, service authentication and
normal links. Existing account paths are reused, not a second account system.
A code search is not proof every shared control reaches 48dp; geometry and
surface-reachability tests remain the foreground-core exit gate.

## Exact native wire fixtures

`test/android/support/channel_fakes.dart` installs strict StandardMethodCodec
transcripts and event streams. Arguments/results are compared recursively;
unexpected or unconsumed calls fail even when production code catches errors.
Data are synthetic and not usable credentials. Fixtures test the **existing
Dart adapters**, not a parallel test-only transport implementation.

| Channel | Contract pinned by tests / specified next |
|---|---|
| factory/device | connections() → `{current: String, saved: List<String>, keys: Map<String,String>, account: Map<String,String>}`; setConnections(same map) → void; resetConnections() → void. Actual DeviceAccount map keys are `key` (raw seed), `service`, `email`; preserve the complete map, not just endpoint keys. Platform errors become ConnectionStorageFailure with the original code/message; a failed read blocks writes until explicit recovery/reset (Kenji's shared tests). |
| factory/device | drafts() → Map<String,String>; setDrafts(map) → void, separate from credential state. qrCode(String) → `{size: int, modules: Uint8List}` length size²; null/unusable result means unavailable. QR fixture tests the byte contract, not a valid QR image. |
| factory/device | cameraAccess() → granted/undetermined/denied/restricted/unavailable; requestCamera() → bool; openCameraSettings() → void. Permission wire tested via current iOS gate, not an Android preview. Android denial/retry distinctions need native/shared agreement. |
| claktory/scanner | view type; per-view `claktory/scanner/<id>` native callback `scanned(String)`. AndroidView lifecycle/duplicate/camera-stop tests deferred until native preview exists. |
| factory/device | pickAttachments() → List<String> private file paths; empty on cancel. No content URI in this response. Native copy/limits/cleanup not proven by fixture. |
| factory/device | webAuth({url: String, scheme: String}) → callback String or null; cancelWebAuth() → void. PlatformException becomes WebAuthFailure. Backend callback validation/PKCE remains shared, never in this policy. |
| factory/links | listen() → List<String> cold-start URLs; native opened(String) for warm URLs. Android shared listen gate remains to change; callbacks must not auto-approve connecting. |
| factory/network | EventChannel listen/cancel, event `{satisfied: bool, interfaces: List<String>, expensive: bool, constrained: bool}`. Interfaces wifi/cellular/wired/loopback/other in preference order. Initial path is not a move; later primary/reachability change is debounced. |
| factory/recorder | access() → granted/undetermined/restricted are explicitly mapped; other strings, including unavailable, currently map to denied. MissingPluginException/PlatformException map to MicAccess.unavailable. Agree no-microphone mapping before binding. request() → bool; openSettings(); start(String path) → void; stop() → `{path: String, durationMs: int}` or null; cancel() deletes. Start errors preserve denied/busy/failed code. |
| factory/recorder/events | `{level: num, ms: num, ended?: limit/interrupted/error}`. Standard channel errors remain errors. Ten-minute cap, AAC .m4a/16kHz mono require hardware inspection. Preserve hold→release sends, slide cancels, accessible tap alternative; no preview/new speech service. |
| factory/audio + /events | play({path, fromMs}), pause(), seek(int milliseconds), stop(); event `{path, positionMs, durationMs, state: playing/paused/ended/stopped}`. One active foreground player; focus/headphone behavior remains native acceptance. |
| claktory/video | open({path}) → `{texture, width, height, quarterTurns, position, duration, playing}`; play/pause/close({texture}), seek({texture, seconds}); native state(same map). Video times are seconds, audio/recorder times milliseconds. Texture ownership/rotation method fixture tested; live callback/lifecycle/codec acceptance owed. |

Do not add factory/window/menu/setup Android handlers merely to hide missing
plugins. The local tunnel capability contract is already shared Rust/Dart;
never persist, expose as a connection code, or fork it in this folder.

### Deferred link and sensitive-copy gates

Kenji is the named shared `links.dart` owner (Mike's ruling), Raj the native
intent owner. Before Android add-link delivery is enabled, **every** add intent
must ask, including an already-saved destination. Name the destination,
distinguish a new Claktory, and require an explicit Connect tap; an arbitrary
app can send a custom-scheme intent. Do not inherit the Mac's saved-target
auto-switch path. Future https links must be verified Android App Links.

A pairing-code copy must use a native sensitive-clipboard path supporting
ClipDescription.EXTRA_IS_SENSITIVE on API 33+, not ordinary Flutter
Clipboard.setData. Raj's proposed `factory/device:copySensitive` is deliberately
not added/called speculatively: bind it when a real sensitive-copy site is
assigned. The foreground tests exercise ordinary draft text only. Both native
link/clipboard gates remain owed, not evidence supplied by these widget tests.

## Navigation and input acceptance matrix

Every future device test must open the named surface through its real action
and assert route identity, visible title/action/semantics and underlying state.
A screenshot is supplementary, not a reached-state assertion. Record source,
APK hash/build mode/API/vendor, navigation mode, IME and device identifier.
All integration connects to isolated QA/Dev Claktories, never port 7420.

| Case / setup | Required committed result | Cancel/invariant assertion | Evidence now |
|---|---|---|---|
| Root conversation | Return home/back normally | Agent continues; draft, accepted messages and backend layout unchanged | Shared agent/chat roots plus host SystemNavigator fixture; OS/backend-live evidence owed |
| Settings / Files / text viewer / Map nested page | Only top phone route closes; previous scroll/focus restored | Stack, route key, draft, remote layout unchanged | Actual Settings via drawer, both-edge cancel/draft/Back; Files/viewer host fixtures; other actual routes and OS owed |
| Drawer over page | Drawer closes; page remains | Drawer progress restored; page/draft unchanged | Fake ordering; real gesture/buttons owed |
| Selection/menu/dialog/sheet, including nested sheet | Top occurrence only, no page/drawer double-dismiss | No token removed; underlying semantics remain blocked appropriately | Actual Extensions, editable/transcript toolbar, Agent settings/confirmation; other surfaces and OS owed |
| IME + page/sheet/drawer | First Back hides keyboard only; second separate Back acts on app layer | No draft/caret/route loss; zero inset mid-completion never falls through | Actual Settings + fake view insets/TextInput.hide channel; host IME/button race; OS dispatch owed |
| Dirty agent/secret/comment form | Ask Discard/Keep editing; explicit decision on second surface | No ask on canceled gesture, no discard; Keep editing restores field/focus | Actual Agent settings edited/canceled/kept, no UpdateAgent; other forms and OS owed |
| Target disposed/reopened/replaced during gesture | No action on replacement, no pop under a new modal | Same destination/draft after completion | Shared registry tests plus host stale-modal/lifecycle/teardown tests; OS owed |
| Page already popped natively | Exactly one pop; never shared commit again | No route below popped | Independent native route observer disabled by tripwire; host single-commit/duplicate-terminal tests; OS owed |
| Left AND right predictive edge, three-button and physical/keyboard Back | Same ordered semantics for each supported source | Each predictive cancel preserves full state | Actual Settings via both-edge framework messages; generic host button/overlap tests; every OS path owed |
| Drawer-edge conflict / Map/image/code/selection drag | No app drawer opening from OS Back edges; local gestures stay local | No root navigation or data edit | Actual phone edge drags do not open drawer; Menu does; media/Map/OS gesture arena owed |
| Text toolbar / composer / comment editor | Copy/Paste/Select all, handles/magnifier and supported Comment/Suggest; Return newline not Send | IME composing region, emoji, destination-specific draft retained | Actual composer Copy/Paste/composition/Send and transcript toolbar Back; extra Comment helper test; native/comment-editor/IME coverage owed |
| Insets / resize / accessibility | Send/caret/last line/Close reachable above IME; target >=48dp; one inset owner | No duplicates/offstage focus; preserve route/scroll/draft on resize | 48dp core phone actions, drawer and wide primitives; composer resize/inset geometry; Dart semantics and linear 2x/3x text profiles; Mac density preserved; cross-tier/live-AX/device checks owed |
| Connection/account storage binding | Complete connections map, including raw account key, encrypted with a Keystore-backed key in private no-backup storage; exclude Auto Backup and device-to-device transfer | No plaintext prefs copy; corruption/invalidation/restore mismatch never silently replace identity; no raw seed or tunnel capability in channel logs, errors or artifacts | Synthetic wire shape only; Raj native tests and Nora independent recovery/backup/non-disclosure checks owed |

Run OS rows in gesture and three-button navigation, both orientations, cutouts,
button/gesture nav bars, hardware keyboard, Gboard plus another/non-Latin IME,
default/large/largest font and display scale, TalkBack and Switch Access,
light/dark, reduced motion and RTL. Minimum widget sizes: 320×568 stress,
360×640 compact, 412×915 tall, short landscape, and 600/840dp resized windows.
Release acceptance adds representative API 24/36, 4KiB/16KiB, and physical arm64
on Pixel + another vendor family when available. Emulator-only evidence does
not close native acceptance.

## Checks and handoff

From app: `flutter analyze lib/android test/android` and
`flutter test test/android`. Existing shared phone/input/channel suites remain
regression coverage; no Android SDK is required for these host-side tests.

Mei: shared policy/registry/binding landed; wider capability hooks, docs and iOS
regression evidence remain hers.
Raj: consume wire fixtures; prove native channels, permissions/security/lifecycle
on devices. Mike assigns shared entry-point owners above; Katya supplies focused
patches/tests rather than editing their files. Nora independently checks live
Back/IME/recovery paths. Mei holds CHANGELOG/IDENTITY/manual for the Android
binding/release batch; Hana approved folding naming into the eventual Android
line, not advertising an Android release before one exists.
