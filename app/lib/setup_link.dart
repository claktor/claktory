// Connecting this Mac to a Claktory from a shell (#245): an agent setting
// Claktory up on a server and this Mac at once writes the server's pairing
// code where only a program on this Mac can (connect-request.json, read by
// app/macos/Runner/SetupLink.swift), and follows how it goes in
// connect-status.json: connecting, waiting with the four digits and this
// device's key to check before it runs `factoryd allow` on the server,
// connected, or failed with why. Nobody pastes the code or reads the
// digits off the screen; the digits are still checked, by the agent.
//
// Only a Mac with no Claktory saved yet takes one: one already set up
// never connects without the person (its links ask first, links.dart).
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'connections.dart';
import 'controls.dart' show showModal;
import 'forms.dart' show FormFrame;
import 'scope.dart';
import 'tunnel_client.dart' show RustTunnels;

/// Works out this device's id (its key, in hex) on [target] from the key
/// it reaches it with.
typedef DeviceEndpoint = Future<String> Function(
  String deviceKey,
  RemoteFactory target,
);

Future<String> _deviceEndpoint(String deviceKey, RemoteFactory target) async {
  final ids = await RustTunnels().pairingIds(
    deviceKey: deviceKey,
    endpointId: target.endpointId,
  );
  return ids.device;
}

/// The requests the Mac's app takes (`factory/setup-link`), and where it
/// writes how they go.
class SetupLinks extends ChangeNotifier {
  SetupLinks({this.channel = const MethodChannel('factory/setup-link')});

  final MethodChannel channel;
  final _waiting = <String>[];

  /// Takes requests from the app (macOS): those that came before, then
  /// each as it comes.
  Future<void> listen() async {
    if (defaultTargetPlatform != TargetPlatform.macOS) return;
    channel.setMethodCallHandler((call) async {
      if (call.method == 'request' && call.arguments is String) {
        receive(call.arguments as String);
      }
    });
    try {
      final waiting = await channel.invokeListMethod<String>('listen');
      for (final code in waiting ?? const <String>[]) {
        receive(code);
      }
    } on PlatformException {
      // An app without the channel: none.
    } on MissingPluginException {
      // The same, in tests.
    }
  }

  /// A request to connect with [code].
  void receive(String code) {
    _waiting.add(code);
    notifyListeners();
  }

  /// The oldest request not yet taken, taking it.
  String? take() => _waiting.isEmpty ? null : _waiting.removeAt(0);

  /// Says how the request goes, for the agent that made it.
  Future<void> status(Map<String, Object?> status) async {
    try {
      await channel.invokeMethod<void>('status', status);
    } on PlatformException {
      // Nowhere to say it.
    } on MissingPluginException {
      // The same, in tests.
    }
  }
}

/// The app's requests.
final setupLinks = SetupLinks();

/// Takes what [links] brings, in the window: connects to the Claktory a
/// request names while this Mac has none saved, says how it goes, and once
/// connected tells the person what happened.
class SetupLinkOpener extends StatefulWidget {
  const SetupLinkOpener({
    super.key,
    required this.child,
    this.links,
    this.deviceEndpoint = _deviceEndpoint,
  });

  final Widget child;

  /// Where requests come from: the app's ([setupLinks]) unless a test says.
  final SetupLinks? links;

  final DeviceEndpoint deviceEndpoint;

  @override
  State<SetupLinkOpener> createState() => _SetupLinkOpenerState();
}

class _SetupLinkOpenerState extends State<SetupLinkOpener> {
  SetupLinks get _links => widget.links ?? setupLinks;

  /// The Claktory a request is connecting to, until it's connected or
  /// failed.
  RemoteFactory? _target;
  Backend? _backend;
  Connections? _connections;
  Map<String, Object?>? _said;

  /// When it started connecting: a welcome before then was another
  /// Claktory's (this Mac's own).
  DateTime? _since;

  @override
  void initState() {
    super.initState();
    _links.addListener(_next);
    unawaited(_links.listen());
    WidgetsBinding.instance.addPostFrameCallback((_) => _next());
  }

  @override
  void dispose() {
    _links.removeListener(_next);
    _stopFollowing();
    super.dispose();
  }

  void _next() {
    if (!mounted) return;
    final code = _links.take();
    if (code == null) return;
    final scope = AppScope.scope(context);
    final connections = scope.connections;
    final target = parsePairingCode(code);
    if (target == null) return _fail('bad_code');
    if (connections.readFailure != null) return _fail('connections_unreadable');
    // Set up already: only the person adds another (a link asks first).
    if (connections.saved.isNotEmpty) return _fail('already_connected');
    if (_target != null) return _fail('busy');
    _target = target;
    _since = DateTime.now();
    _say({'state': 'connecting', 'name': target.label});
    connections.use(target);
    _backend = scope.backend;
    _connections = connections;
    for (final changes in _followed) {
      changes.addListener(_progress);
    }
  }

  List<Listenable> get _followed => [
    ?_backend?.pairing,
    ?_backend?.refused,
    ?_backend?.signInRequired,
    ?_backend?.connectedSince,
    ?_connections,
  ];

  void _stopFollowing() {
    for (final changes in _followed) {
      changes.removeListener(_progress);
    }
    _backend = null;
    _connections = null;
  }

  Future<void> _progress() async {
    final target = _target;
    final backend = _backend;
    final connections = _connections;
    if (target == null || backend == null || connections == null) return;
    if (connections.current != target) {
      // The person picked another, or forgot it, meanwhile.
      return _done({'state': 'failed', 'reason': 'cancelled'});
    }
    final refused = backend.refused.value;
    if (refused != null) {
      return _done({
        'state': 'failed',
        'reason': 'refused',
        'message': refused,
      });
    }
    if (backend.signInRequired.value != null) {
      return _done({'state': 'failed', 'reason': 'sign_in_required'});
    }
    final waiting = backend.pairing.value;
    if (waiting != null) {
      String endpoint;
      try {
        endpoint = await widget.deviceEndpoint(
          connections.deviceKey(target),
          target,
        );
      } catch (_) {
        endpoint = '';
      }
      if (!mounted || _target != target) return;
      _say({
        'state': 'waiting',
        'name': target.label,
        'device_id': waiting.pending.deviceId,
        'number': waiting.number,
        'endpoint_id': endpoint,
      });
      return;
    }
    final since = _since;
    final welcomed = backend.connectedSince.value;
    if (since != null && welcomed != null && !welcomed.isBefore(since)) {
      _done({'state': 'connected', 'name': target.label});
      if (mounted) _tell(target.label);
    }
  }

  void _fail(String reason) => _say({'state': 'failed', 'reason': reason});

  void _done(Map<String, Object?> status) {
    _say(status);
    _target = null;
    _stopFollowing();
  }

  void _say(Map<String, Object?> status) {
    if (mapEquals(status, _said)) return;
    _said = status;
    unawaited(_links.status(status));
  }

  /// Tells the person, once, that a setup on this Mac connected it.
  void _tell(String name) => showModal(
    context,
    (dialog, close) => FormFrame(
      title: 'Connected to $name',
      body:
          'This Mac was just connected to $name by a setup run on it. If '
          "you didn't start one, remove this Mac in $name's Settings → "
          'Devices.',
      fields: const [],
      error: null,
      submitLabel: 'OK',
      onSubmit: close,
      onCancel: close,
    ),
  );

  @override
  Widget build(BuildContext context) => widget.child;
}
