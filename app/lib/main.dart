import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'browser/browser_view.dart' show BrowserHost, BrowserScope;
import 'connections.dart';
import 'drafts.dart';
import 'drop.dart';
import 'identity.dart';
import 'ipad/workbench.dart' show WorkbenchFocus, WorkbenchScope;
import 'licenses.dart';
import 'links.dart' show appLinks;
import 'mobile/device.dart' show loadDeviceCapabilities;
import 'mobile/platform_policy.dart';
import 'mobile/push.dart' show HttpPushRelay, IosPushDevice, PushRegistrar;
import 'notifications.dart';
import 'rail.dart';
import 'scope.dart';
import 'setup.dart';
import 'shell.dart';
import 'state.dart';
import 'unreadable.dart';
import 'update.dart';
import 'zoom.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // Third-party licences the app ships, before anything can list them.
  registerLicenses();
  // What this device is (an iPad, a phone), before the first frame: so
  // nothing lays out as another device first.
  await loadDeviceCapabilities();
  final drafts = await DeviceDrafts.load(DeviceDraftStorage());
  final storage = DeviceConnectionStorage();
  final localBackend = await runsClaktoryHere();
  void start(Connections connections) => runApp(
    FactoryApp(
      drafts: drafts,
      connections: connections,
      localBackend: localBackend,
    ),
  );
  final connections = await Connections.load(storage);
  // What this device keeps can't be read: it says so, rather than start
  // looking unpaired and then save that over what it keeps.
  if (connections.readFailure case final failure?) {
    runApp(UnreadableApp(storage: storage, failure: failure, onRead: start));
  } else {
    start(connections);
  }
  // Links that opened the app, such as claktory://add?code=… (links.dart).
  unawaited(appLinks.listen());
}

class FactoryApp extends StatefulWidget {
  const FactoryApp({
    super.key,
    required this.drafts,
    required this.connections,
    this.open,
    this.localBackend,
    this.setupChannel,
  });

  /// The drafts this device keeps, for every Claktory.
  final DeviceDrafts drafts;

  /// Every Claktory this app connects to (this Mac's, and others on other
  /// machines), and which one the window shows.
  final Connections connections;

  /// Connects to a Claktory; tests give one that doesn't.
  final Backend Function(RemoteFactory? target)? open;

  /// Whether this device runs a Claktory of its own; by default, whether
  /// it's a Mac ([hasLocalBackend]).
  final bool? localBackend;

  /// Sets this Mac up from the app ([setsUpThisMac]); tests give one to
  /// set up with anywhere.
  final SetupChannel? setupChannel;

  @override
  State<FactoryApp> createState() => _FactoryAppState();
}

class _FactoryAppState extends State<FactoryApp> {
  late final _claktories = _made = Claktories(
    connections: widget.connections,
    localBackend: widget.localBackend ?? hasLocalBackend,
    open: widget.open ?? _open,
    drafts: widget.drafts,
    // A Mac keeps whether it runs a Claktory of its own.
    keepLocal: _canChooseLocal ? _keepLocal : null,
  );
  Claktories? _made;
  final _appBuild = AppBuild.current();

  /// Whether this Mac runs a Claktory of its own now: until the
  /// Claktories are made, as the app started.
  bool get _localNow =>
      _made?.localBackend ?? widget.localBackend ?? hasLocalBackend;

  /// The channel that sets this Mac up and keeps its choice, where there
  /// is one.
  SetupChannel? get _channel =>
      widget.setupChannel ?? (setsUpThisMac ? const SetupChannel() : null);

  bool get _canChooseLocal =>
      widget.setupChannel != null ||
      defaultTargetPlatform == TargetPlatform.macOS;

  /// Setting this Mac up, on the first launch of the app downloaded for
  /// it, or once the person chooses this Mac after all.
  MacSetup? _setup;

  void _startSetup({bool ask = true}) {
    final channel = _channel;
    _setup?.dispose();
    _setup = null;
    if (channel == null || !_claktories.localBackend) return;
    final local = _claktories.all.firstWhere((each) => each.target == null);
    final setup = _setup = MacSetup(
      channel: channel,
      connected: local.backend.connected,
      reconnect: local.backend.reconnectNow,
      ask: ask,
      elsewhere: () => _claktories.setLocalBackend(false),
    );
    unawaited(setup.start());
  }

  /// Keeps the person's choice: this Mac runs a Claktory of its own, or
  /// only shows others. Chosen here after all, it's set up.
  Future<void> _keepLocal(bool local) async {
    await (_channel ?? const SetupChannel()).setAppOnly(!local);
    if (!mounted) return;
    if (local) {
      setState(() => _startSetup(ask: false));
    } else if (_setup?.stage != SetupStage.done) {
      setState(() {
        _setup?.dispose();
        _setup = null;
      });
    }
  }

  /// A Mac (and an iPhone, through the same channel) shows each
  /// Claktory's notifications while the person isn't using it
  /// (notifications.dart).
  ClaktoryNotifications? _notifications;

  @override
  void initState() {
    super.initState();
    _startSetup();
    if (defaultTargetPlatform == TargetPlatform.macOS ||
        defaultTargetPlatform == TargetPlatform.iOS) {
      _notifications = ClaktoryNotifications(
        claktories: _claktories,
        system: MacNotifications(),
      );
    }
    // Chrome's tabs reach an agent only through a Mac's app.
    if (defaultTargetPlatform == TargetPlatform.macOS) {
      _browser = BrowserHost.of(_claktories);
    }
    // An iPhone registers with each Claktory for pushes, once the person
    // allows notifications (mobile/push.dart).
    if (defaultTargetPlatform == TargetPlatform.iOS) {
      _pushes = PushRegistrar.current = PushRegistrar(
        claktories: _claktories,
        device: IosPushDevice(),
        relay: HttpPushRelay(),
      );
    }
  }

  PushRegistrar? _pushes;

  /// This Mac's browser control (browser/browser_view.dart).
  BrowserHost? _browser;

  /// The window, as an overlay entry: the root layer the phone's list of
  /// Claktories and its dialogs float in.
  late final _window = OverlayEntry(builder: _buildWindow);

  /// A live connection to [target] (null: this Mac's Claktory).
  Backend _open(RemoteFactory? target) {
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      localBackend: _localNow,
    );
    // The device's key for each Claktory, kept with its pairing.
    backend.deviceKey = widget.connections.deviceKey;
    // Signed in to the person's account, it shows each Claktory it's theirs.
    backend.account = widget.connections.account;
    backend.connectTo(target);
    unawaited(backend.run());
    return backend;
  }

  @override
  void dispose() {
    _window.remove();
    _window.dispose();
    _setup?.dispose();
    _notifications?.dispose();
    _pushes?.dispose();
    _browser?.dispose();
    _claktories.dispose();
    for (final focus in _focuses.values) {
      focus.dispose();
    }
    super.dispose();
  }

  /// What the person reads in each Claktory's window (an iPad's focused
  /// pane, ipad/workbench.dart), for this session: one each, since agent
  /// ids repeat across Claktories.
  final _focuses = <Claktory, WorkbenchFocus>{};

  WorkbenchFocus _focusFor(Claktories claktories, Claktory shown) {
    // Those of Claktories gone go with them.
    final open = claktories.all.toSet();
    final gone = [
      for (final claktory in _focuses.keys)
        if (!open.contains(claktory)) claktory,
    ];
    for (final claktory in gone) {
      final focus = _focuses.remove(claktory)!;
      WidgetsBinding.instance.addPostFrameCallback((_) => focus.dispose());
    }
    return _focuses.putIfAbsent(shown, WorkbenchFocus.new);
  }

  Widget _buildWindow(BuildContext context) {
    final claktories = ClaktoriesScope.of(context)!;
    final shown = claktories.selected;
    // Each Claktory gets a window of its own: nothing of one carries over
    // to the next, and its stores are already live, so it shows at once.
    // The rail sits inside it, so its dialogs cover the whole window. Its
    // focus sits above both, so a window narrowed to the phone's routes
    // and widened again keeps what was being read.
    return WorkbenchScope(
      focus: _focusFor(claktories, shown),
      child: isPhoneLayout(context)
          ? PhoneClaktories(shell: Shell(key: ObjectKey(shown)))
          : Shell(key: ObjectKey(shown), rail: const ClaktoryRail()),
    );
  }

  @override
  Widget build(BuildContext context) {
    return WidgetsApp(
      title: 'Claktory',
      color: signal,
      // The words of the iPhone's text selection menu (Copy, Paste…).
      localizationsDelegates: PhonePlatformPolicy.current.localizations,
      debugShowCheckedModeBanner: false,
      builder: (context, _) => ClaktoriesScope(
        claktories: _claktories,
        child: Builder(
          builder: (context) {
            final shown = ClaktoriesScope.of(context)!.selected;
            final scope = AppScope(
              backend: shown.backend,
              login: shown.login,
              page: shown.page,
              appBuild: _appBuild,
              drafts: shown.drafts,
              history: shown.history,
              connections: widget.connections,
              child: ScrollConfiguration(
                behavior: const _ScrollBehavior(),
                child: DevMark(
                  child: AppZoom(
                    child: Stack(
                      children: [
                        Positioned.fill(
                          child: Overlay(initialEntries: [_window]),
                        ),
                        // Files dragged over the window from the Finder.
                        const Positioned.fill(child: WindowDropOverlay()),
                        // A change to what this device keeps that wasn't
                        // saved says so.
                        Positioned(
                          left: 0,
                          right: 0,
                          bottom: 0,
                          child: Center(
                            child: UnsavedNote(connections: widget.connections),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            );
            return SetupScope(
              setup: _setup,
              child: BrowserScope(host: _browser, child: scope),
            );
          },
        ),
      ),
    );
  }
}

/// Thin, quiet scrollbars on vertical scroll views; no overscroll glow.
class _ScrollBehavior extends ScrollBehavior {
  const _ScrollBehavior();

  @override
  Widget buildScrollbar(
    BuildContext context,
    Widget child,
    ScrollableDetails details,
  ) {
    if (axisDirectionToAxis(details.direction) == Axis.horizontal) {
      return child;
    }
    return RawScrollbar(
      controller: details.controller,
      thumbColor: Palette.of(context).textFaint.withValues(alpha: 0.5),
      thickness: 6,
      radius: const Radius.circular(3),
      crossAxisMargin: 2,
      child: child,
    );
  }

  @override
  Widget buildOverscrollIndicator(
    BuildContext context,
    Widget child,
    ScrollableDetails details,
  ) => child;
}

/// This is Factory Dev, the build agents try changes in, rather than the
/// person's Factory. Set by `scripts/dev.sh` and `scripts/release.sh --dev`.
const isDevBuild = bool.fromEnvironment('FACTORY_DEV');

/// In Factory Dev, a faint "DEV VERSION" across the window, so it's never
/// mistaken for the real Factory. It never takes a click.
class DevMark extends StatelessWidget {
  const DevMark({super.key, required this.child, this.enabled = isDevBuild});

  final Widget child;
  final bool enabled;

  @override
  Widget build(BuildContext context) {
    if (!enabled) return child;
    final palette = Palette.of(context);
    return Stack(
      fit: StackFit.expand,
      children: [
        child,
        IgnorePointer(
          child: Center(
            child: Transform.rotate(
              angle: -0.35,
              child: Text(
                'DEV VERSION',
                textAlign: TextAlign.center,
                style: TextStyles.title.copyWith(
                  fontSize: 120,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 12,
                  color: palette.signal.withValues(alpha: 0.07),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}
