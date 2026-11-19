import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/devices.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/tunnel_client.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _token = 'a' * 64;
final _endpoint = 'c' * 64;
final _code =
    'factory://100.64.0.2:7443?t=$_token&fp=${'b' * 64}&name=mac-mini'
    '&e=$_endpoint&r=https://relay-eu.example.com&r=https://relay-us.example.com';

TunnelStatus tunnelStatus({
  TunnelPhase phase = TunnelPhase.connected,
  TunnelPath path = TunnelPath.direct,
  TunnelFailure failure = TunnelFailure.none,
  String error = '',
  bool relay = true,
}) => TunnelStatus(
  phase: phase,
  path: path,
  error: error,
  failure: failure,
  relayConnected: relay,
);

/// A tunnel that's a local port: whatever listens there stands for the
/// backend at the other end.
class FakeTunnel implements Tunnel {
  FakeTunnel(this.port);

  @override
  final int port;

  @override
  TunnelStatus status = tunnelStatus();

  var closed = false;

  @override
  Future<void> close() async => closed = true;
}

class FakeTunnels implements Tunnels {
  FakeTunnels(this.tunnel);

  final FakeTunnel tunnel;
  final opened = <(String, String, List<String>)>[];

  @override
  Future<Tunnel> open({
    required String deviceKey,
    required String endpointId,
    required List<String> relays,
  }) async {
    opened.add((deviceKey, endpointId, relays));
    return tunnel;
  }
}

Backend backendWith(Tunnels tunnels) => Backend(
  workspace: WorkspaceStore(),
  transcript: TranscriptStore(),
  explorer: ExplorerStore(),
  extensionView: ExtensionViewStore(),
  tunnels: tunnels,
);

void main() {
  group('pairing codes with the tunnel', () {
    test('carry the endpoint id and relays', () {
      final target = parsePairingCode(_code)!;
      expect(target.tunnelled, isTrue);
      expect(target.endpointId, _endpoint);
      expect(target.relays, [
        'https://relay-eu.example.com',
        'https://relay-us.example.com',
      ]);
      expect(target.label, 'mac-mini');
    });

    test('need only the token and the endpoint id', () {
      final target = parsePairingCode(
        'factory://?t=$_token&e=$_endpoint&name=nas',
      )!;
      expect(target.tunnelled, isTrue);
      expect(target.relays, isEmpty);
      // base32, as iroh also writes them.
      expect(
        parsePairingCode('factory://?t=$_token&e=${'a2' * 26}'),
        isNotNull,
      );
    });

    test('refuse a bad endpoint id, and keep old codes as they were', () {
      expect(parsePairingCode('factory://?t=$_token&e=nope'), isNull);
      expect(parsePairingCode('factory://?t=$_token'), isNull);
      final old = parsePairingCode(
        'factory://100.64.0.2:7443?t=$_token&fp=${'b' * 64}',
      )!;
      expect(old.tunnelled, isFalse);
      // A relay that isn't a URL is left out.
      expect(parsePairingCode('$_code&r=not%20a%20url')!.relays, hasLength(2));
    });
  });

  test('device keys: made on the device, one per pairing, kept and '
      'forgotten with it', () async {
    expect(newDeviceKey(), matches(RegExp(r'^[0-9a-f]{64}$')));
    expect(newDeviceKey(Random(1)), newDeviceKey(Random(1)));
    expect(newDeviceKey(), isNot(newDeviceKey()));

    final storage = MemoryConnectionStorage();
    final connections = await Connections.load(storage);
    final target = parsePairingCode(_code)!;
    final other = parsePairingCode(
      'factory://?t=$_token&e=${'d' * 64}&name=nas',
    )!;
    connections.use(target);
    connections.use(other);
    final key = connections.deviceKey(target);
    expect(connections.deviceKey(target), key, reason: 'kept');
    expect(connections.deviceKey(other), isNot(key), reason: 'per pairing');
    await pumpEventQueue();
    expect(storage.kept.keys[target.code], key);

    // After a restart, the same key.
    final again = await Connections.load(storage);
    expect(again.deviceKey(again.saved.first), key);
    again.forget(again.saved.first);
    await pumpEventQueue();
    expect(storage.kept.keys.containsKey(target.code), isFalse);
  });

  test('a Factory with an endpoint id is reached only through its tunnel: '
      'plain ws to the tunnel\'s port, the token in Hello', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final hello = Completer<ClientMessage>();
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        if (!hello.isCompleted) {
          hello.complete(ClientMessage.fromBuffer(frame as List<int>));
        }
      });
    });
    final tunnels = FakeTunnels(FakeTunnel(server.port));
    final backend = backendWith(tunnels)..deviceKey = (_) => 'k' * 64;
    final target = parsePairingCode(_code)!;
    backend.connectTo(target);
    unawaited(backend.run());

    final message = await hello.future.timeout(const Duration(seconds: 5));
    expect(message.hello.token, _token);
    final (key, endpoint, relays) = tunnels.opened.single;
    expect(key, 'k' * 64);
    expect(endpoint, _endpoint);
    expect(relays, [
      'https://relay-eu.example.com',
      'https://relay-us.example.com',
    ]);
    expect(backend.tunnel.value?.path, TunnelPath.direct);

    // Showing this Mac's instead closes the tunnel.
    backend.connectTo(null);
    await pumpEventQueue();
    expect(tunnels.tunnel.closed, isTrue);
    expect(backend.tunnel.value, isNull);
    await backend.close();
    await server.close(force: true);
  });

  test('turned away at the tunnel, it stops trying and says why', () async {
    // Nothing listens: the tunnel closes every socket at once.
    final unused = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final port = unused.port;
    await unused.close();
    final tunnel = FakeTunnel(port)
      ..status = tunnelStatus(
        phase: TunnelPhase.failed,
        path: TunnelPath.none,
        failure: TunnelFailure.refused,
        error: "The machine turned this device away: it isn't paired",
      );
    final backend = backendWith(FakeTunnels(tunnel));
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    for (var i = 0; i < 50 && backend.refused.value == null; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(backend.refused.value, contains("isn't paired"));
    await backend.close();
  });

  testWidgets("the footer says it can't reach the machine, and why on a "
      'click', (tester) async {
    final harness = Harness(Workspace());
    harness.backend.connected.value = false;
    harness.backend.connectTo(parsePairingCode(_code));
    harness.backend.tunnel.value = tunnelStatus(
      phase: TunnelPhase.failed,
      path: TunnelPath.none,
      failure: TunnelFailure.unreachable,
      error: 'Nothing answered within 20 seconds.',
    );
    await harness.show(tester);
    await tester.tap(find.text("Can't reach mac-mini"));
    await settle(tester);
    expect(find.byType(TunnelProblem), findsOneWidget);
    expect(find.text('Nothing answered within 20 seconds.'), findsOneWidget);
    expect(find.textContaining('Check that mac-mini is on'), findsOneWidget);
    await tester.tap(find.text('Try again'));
    await settle(tester);
    expect(find.byType(TunnelProblem), findsNothing);
  });

  testWidgets('Settings → Devices says how this device reaches it', (
    tester,
  ) async {
    final snapshot = Workspace(
      agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
      selectedAgentId: 'rivet',
      remote: RemoteAccess(on: true, port: 7443, machine: 'mac-mini'),
    );
    final harness = Harness(snapshot);
    harness.backend.connectTo(parsePairingCode(_code));
    harness.backend.tunnel.value = tunnelStatus(path: TunnelPath.relayed);
    harness.page.value = AppPage.settings;
    await harness.show(tester);
    await harness.apply(tester, snapshot);
    await pumpFrames(tester);
    final line = find.text(
      'Reaches mac-mini through the private tunnel, through a relay',
    );
    await tester.ensureVisible(line);
    expect(line, findsOneWidget);
  });
}
