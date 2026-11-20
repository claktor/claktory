import 'dart:async';

import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'connections.dart';
import 'drafts.dart';
import 'identity.dart';
import 'rail.dart';
import 'scope.dart';
import 'shell.dart';
import 'state.dart';
import 'update.dart';
import 'zoom.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final drafts = await DeviceDrafts.load(DeviceDraftStorage());
  final connections = await Connections.load(DeviceConnectionStorage());
  runApp(FactoryApp(drafts: drafts, connections: connections));
}

class FactoryApp extends StatefulWidget {
  const FactoryApp({
    super.key,
    required this.drafts,
    required this.connections,
    this.open,
    this.localBackend,
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

  @override
  State<FactoryApp> createState() => _FactoryAppState();
}

class _FactoryAppState extends State<FactoryApp> {
  late final _claktories = Claktories(
    connections: widget.connections,
    localBackend: widget.localBackend ?? hasLocalBackend,
    open: widget.open ?? _open,
    drafts: widget.drafts,
  );
  final _appBuild = AppBuild.current();

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
      localBackend: widget.localBackend ?? hasLocalBackend,
    );
    // The device's key for each Claktory, kept with its pairing.
    backend.deviceKey = widget.connections.deviceKey;
    backend.connectTo(target);
    unawaited(backend.run());
    return backend;
  }

  @override
  void dispose() {
    _window.remove();
    _window.dispose();
    _claktories.dispose();
    super.dispose();
  }

  Widget _buildWindow(BuildContext context) {
    final claktories = ClaktoriesScope.of(context)!;
    final shown = claktories.selected;
    // Each Claktory gets a window of its own: nothing of one carries over
    // to the next, and its stores are already live, so it shows at once.
    // The rail sits inside it, so its dialogs cover the whole window.
    if (isPhoneLayout(context)) {
      return PhoneClaktories(shell: Shell(key: ValueKey(shown.key)));
    }
    return Shell(key: ValueKey(shown.key), rail: const ClaktoryRail());
  }

  @override
  Widget build(BuildContext context) {
    return WidgetsApp(
      title: 'Claktory',
      color: signal,
      debugShowCheckedModeBanner: false,
      builder: (context, _) => ClaktoriesScope(
        claktories: _claktories,
        child: Builder(
          builder: (context) {
            final shown = ClaktoriesScope.of(context)!.selected;
            return AppScope(
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
                  child: AppZoom(child: Overlay(initialEntries: [_window])),
                ),
              ),
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
