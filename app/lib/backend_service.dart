// Friend bug #24: a Mac's backend started some other way (factoryd in
// Terminal) while its background service wasn't running it doesn't start
// again by itself and can't move to another computer. The backend says so
// (Workspace.backend_service, BY_HAND) with a sentence for the person;
// Restore (RestoreBackendService) puts the service back in charge: the
// backend stops and the service starts it again. Shown in Needs you and in
// Settings → General beside Move, on every device that shows that Mac's
// Claktory.
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'outbox.dart' show connectionLost;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'tell_rivet.dart' show ErrorLine;

/// Whether [workspace]'s backend runs without its service: it has
/// something to say, and Needs you an item.
bool backendByHand(Workspace workspace) =>
    workspace.hasBackendService() &&
    workspace.backendService.state ==
        BackendServiceState.BACKEND_SERVICE_STATE_BY_HAND;

/// Needs you's title for it.
const backendServiceTitle = "Claktory isn't running in the background";

/// The line under the title, folded: Restore where it can, else what to
/// do.
String backendServiceSummary(BackendService service) => service.restoring
    ? backendRestarting
    : service.canRestore
    ? 'Restore it so it starts again by itself.'
    : 'Quit the app and open it again to repair it.';

/// While the service takes it back: until the app reconnects.
const backendRestarting = 'Restarting…';

/// The backend's sentence, with Restore when it can; "Restarting…" once
/// asked, until the app reconnects (the service starts it again, and the
/// new one says it runs as installed). Nothing while it runs as installed.
class BackendServiceRestore extends StatefulWidget {
  const BackendServiceRestore({super.key, this.top = 0});

  /// Room above it, when it shows.
  final double top;

  @override
  State<BackendServiceRestore> createState() => _BackendServiceRestoreState();
}

class _BackendServiceRestoreState extends State<BackendServiceRestore> {
  /// Asked, or acked: restarting until the app has reconnected.
  var _restarting = false;

  /// The connection dropped since Restore was acked: the next one is the
  /// service's backend.
  var _wentDown = false;
  String? _failure;
  Backend? _backend;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (identical(backend, _backend)) return;
    _backend?.connected.removeListener(_connectedChanged);
    _backend = backend..connected.addListener(_connectedChanged);
  }

  @override
  void dispose() {
    _backend?.connected.removeListener(_connectedChanged);
    super.dispose();
  }

  void _connectedChanged() {
    if (!_restarting) return;
    if (!(_backend?.connected.value ?? false)) {
      _wentDown = true;
    } else if (_wentDown) {
      // Back: what it says now is the new backend's.
      setState(() => _restarting = _wentDown = false);
    }
  }

  Future<void> _restore(Backend backend) async {
    setState(() {
      _restarting = true;
      _wentDown = false;
      _failure = null;
    });
    try {
      await backend.restoreBackendService();
    } on RequestFailure catch (failure) {
      // The connection closing as the backend stops is how it goes.
      if (connectionLost(failure) || !mounted) return;
      setState(() {
        _restarting = false;
        _failure = failure.message;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: Listenable.merge([backend.workspace, backend.connected]),
      builder: (context, _) {
        final workspace = backend.workspace.workspace;
        if (!backendByHand(workspace)) return const SizedBox.shrink();
        final service = workspace.backendService;
        final palette = Palette.of(context);
        final type = TypeScale.of(context);
        final restarting = service.restoring || _restarting;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            if (widget.top > 0) SizedBox(height: widget.top),
            if (service.reason.isNotEmpty)
              Text(
                service.reason,
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            if (restarting) ...[
              const SizedBox(height: Space.s),
              Semantics(
                liveRegion: true,
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const LiveDot(),
                    const SizedBox(width: Space.s),
                    Text(
                      backendRestarting,
                      style: type.caption.copyWith(color: palette.textMuted),
                    ),
                  ],
                ),
              ),
            ] else if (service.canRestore) ...[
              const SizedBox(height: Space.s),
              Button(
                key: const ValueKey('restore-backend-service'),
                label: 'Restore',
                primary: true,
                onPressed: backend.connected.value
                    ? () => _restore(backend)
                    : null,
              ),
            ],
            if (_failure case final failure? when !restarting) ...[
              const SizedBox(height: Space.s),
              ErrorLine(
                "Couldn't restart Claktory in the background: $failure",
                about: 'Restarting Claktory in the background:',
              ),
            ],
          ],
        );
      },
    );
  }
}
