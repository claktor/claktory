// #214: a reconnect that doesn't bring the connection back is started from
// scratch, as relaunching the app does: the try under way given up on
// whatever it waits for, the tunnel dropped, the tunnels' runtime made
// afresh when it doesn't answer. Reconnect (a tap) does it at once.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/tunnel_client.dart';
import 'package:flutter_test/flutter_test.dart';

import 'tunnel_test.dart' show FakeTunnel;

final _code = 'factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac-mini';

/// Waits up to [seconds] for [done].
Future<void> until(bool Function() done, {int seconds = 5}) async {
  final end = DateTime.now().add(Duration(seconds: seconds));
  while (!done() && DateTime.now().isBefore(end)) {
    await Future<void>.delayed(const Duration(milliseconds: 10));
  }
  expect(done(), isTrue);
}

/// A backend at the other end of the tunnel: it welcomes each Hello while
/// [welcoming], and counts them.
class _Far {
  late final HttpServer server;
  var hellos = 0;
  var welcoming = true;

  Future<void> start() async {
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (!message.hasHello()) return;
        hellos++;
        if (!welcoming) return;
        socket.add(
          ServerMessage(
            replyTo: message.id,
            welcome: Welcome(serverVersion: 'test'),
          ).writeToBuffer(),
        );
      });
    });
  }

  Future<void> close() => server.close(force: true);
}

/// Tunnels whose opens go as [opens] says, in turn: a tunnel, or a
/// future that never completes (stuck).
class _Tunnels implements Tunnels {
  _Tunnels(this.port);

  final int port;
  var opens = 0;
  final made = <FakeTunnel>[];

  /// Which opens (counted from 1) never complete.
  final stuck = <int>{};

  @override
  Future<Tunnel> open({
    required String deviceKey,
    required String endpointId,
    required List<String> relays,
    List<String> addresses = const [],
  }) {
    opens++;
    if (stuck.contains(opens)) return Completer<Tunnel>().future;
    final tunnel = FakeTunnel(port);
    made.add(tunnel);
    return Future.value(tunnel);
  }

  @override
  Future<({String device, String backend})> pairingIds({
    required String deviceKey,
    required String endpointId,
  }) async => (device: 'b' * 64, backend: 'a' * 64);
}

class _Runtime implements TunnelRuntime {
  var answering = true;
  var asked = 0;
  var freshened = 0;

  @override
  Future<bool> answers(Duration within) async {
    asked++;
    return answering;
  }

  @override
  void fresh() => freshened++;
}

Backend _backend(Tunnels tunnels, TunnelRuntime runtime) =>
    Backend(
        workspace: WorkspaceStore(),
        transcript: TranscriptStore(),
        explorer: ExplorerStore(),
        extensionView: ExtensionViewStore(),
        tunnels: tunnels,
        tunnelRuntime: runtime,
        localPairing: () async => null,
      )
      ..deviceKey = ((_) => 'k' * 64)
      // Only a from-scratch start may rescue these: nothing else times out.
      ..tunnelOpenWithin = const Duration(minutes: 5)
      ..connectWithin = const Duration(minutes: 5);

void main() {
  late _Far far;
  setUp(() async {
    far = _Far();
    await far.start();
  });
  tearDown(() => far.close());

  test('Reconnect gives up on a try stuck opening the tunnel, asks the '
      'runtime, and connects from scratch at once', () async {
    final tunnels = _Tunnels(far.server.port)..stuck.add(1);
    final runtime = _Runtime();
    final backend = _backend(tunnels, runtime)
      ..backWithin = const Duration(minutes: 5);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await until(() => tunnels.opens == 1);
    await Future<void>.delayed(const Duration(milliseconds: 100));
    expect(backend.connected.value, isFalse);

    backend.reconnectNow();
    await until(() => backend.connected.value);
    expect(tunnels.opens, 2);
    expect(runtime.asked, 1);
    expect(runtime.freshened, 0, reason: 'it answered');
    await backend.close();
  });

  test('a runtime that no longer answers is made afresh before the next '
      'tunnel opens', () async {
    final tunnels = _Tunnels(far.server.port)..stuck.add(1);
    final runtime = _Runtime()..answering = false;
    final backend = _backend(tunnels, runtime)
      ..backWithin = const Duration(minutes: 5);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await until(() => tunnels.opens == 1);
    backend.reconnectNow();
    await until(() => backend.connected.value);
    expect(runtime.freshened, 1);
    await backend.close();
  });

  test('back from the background, a connection not back in time is '
      'started from scratch by itself: the old tunnel closed, a new one '
      'opened', () async {
    final tunnels = _Tunnels(far.server.port);
    final runtime = _Runtime();
    final backend = _backend(tunnels, runtime)
      ..backWithin = const Duration(milliseconds: 300)
      ..tunnelBackWithin = const Duration(minutes: 5);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await until(() => backend.connected.value);
    expect(tunnels.opens, 1);

    // Away a while; back, the far end has stopped answering (what a
    // relaunch would get past).
    far.welcoming = false;
    backend.hidden();
    backend.resumed(now: DateTime.now().add(const Duration(minutes: 10)));
    await until(() => tunnels.made.first.closed);
    expect(tunnels.opens, greaterThanOrEqualTo(2));
    expect(runtime.asked, greaterThanOrEqualTo(1));

    // It answers again: connected, and the watch ends.
    far.welcoming = true;
    await until(() => backend.connected.value, seconds: 10);
    final opens = tunnels.opens;
    await Future<void>.delayed(const Duration(milliseconds: 900));
    expect(tunnels.opens, opens, reason: 'no more from-scratch starts');
    await backend.close();
  });

  test(
    'the person asking while connected starts it from scratch too',
    () async {
      final tunnels = _Tunnels(far.server.port);
      final backend = _backend(tunnels, _Runtime());
      backend.connectTo(parsePairingCode(_code));
      unawaited(backend.run());
      await until(() => backend.connected.value);
      final hellos = far.hellos;
      backend.reconnectNow();
      await until(() => tunnels.made.first.closed);
      await until(() => backend.connected.value && far.hellos > hellos);
      expect(tunnels.opens, 2);
      await backend.close();
    },
  );
}
