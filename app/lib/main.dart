import 'dart:async';

import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'connections.dart';
import 'drafts.dart';
import 'history.dart';
import 'identity.dart';
import 'login.dart';
import 'scope.dart';
import 'shell.dart';
import 'state.dart';
import 'update.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final drafts = await Drafts.load(DeviceDraftStorage());
  final connections = await Connections.load(DeviceConnectionStorage());
  runApp(FactoryApp(drafts: drafts, connections: connections));
}

class FactoryApp extends StatefulWidget {
  const FactoryApp({
    super.key,
    required this.drafts,
    required this.connections,
  });

  final Drafts drafts;

  /// Which Factory to show: this Mac's, or one on another machine.
  final Connections connections;

  @override
  State<FactoryApp> createState() => _FactoryAppState();
}

class _FactoryAppState extends State<FactoryApp> {
  final _backend = Backend(
    workspace: WorkspaceStore(),
    transcript: TranscriptStore(),
    explorer: ExplorerStore(),
    extensionView: ExtensionViewStore(),
  );
  late final _login = LoginLauncher(
    workspace: _backend.workspace,
    send: _backend.startLogin,
  );
  late final _history = AgentHistory(
    workspace: _backend.workspace,
    select: (id) => fireAndLog(_backend.selectAgent(id)),
  );
  final _page = ValueNotifier(AppPage.agent);
  final _appBuild = AppBuild.current();

  @override
  void initState() {
    super.initState();
    widget.drafts.watch(_backend.workspace);
    // The device's key for each Factory, kept with its pairing.
    _backend.deviceKey = widget.connections.deviceKey;
    _backend.connectTo(widget.connections.current);
    unawaited(_backend.run());
  }

  @override
  void dispose() {
    _login.dispose();
    _page.dispose();
    _history.dispose();
    unawaited(_backend.close());
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return WidgetsApp(
      title: 'Claktory',
      color: signal,
      debugShowCheckedModeBanner: false,
      builder: (context, _) => AppScope(
        backend: _backend,
        login: _login,
        page: _page,
        appBuild: _appBuild,
        drafts: widget.drafts,
        history: _history,
        connections: widget.connections,
        child: ScrollConfiguration(
          behavior: const _ScrollBehavior(),
          child: const DevMark(child: Shell()),
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
