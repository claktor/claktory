import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:factory_app/loopback_forward.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/connection.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/tunnel_client.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
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
  final requestPath = '/${'5e' * 32}';

  @override
  TunnelStatus status = tunnelStatus();

  var closed = false;
  var reconnects = 0;

  @override
  Future<void> reconnect() async => reconnects++;

  /// The ports asked for; [forwardPort] answers what [forwarded] makes.
  final forwards = <int>[];
  Future<PortForward> Function(int port) forwarded = (_) async =>
      throw const ForwardFailure(ForwardProblem.unsupported);

  @override
  Future<PortForward> forwardPort(int port) {
    forwards.add(port);
    return forwarded(port);
  }

  @override
  Future<void> close() async => closed = true;
}

/// A tunnel whose status is a new object each time it's read, as the Rust
/// side's is.
class _FreshStatusTunnel extends FakeTunnel {
  _FreshStatusTunnel(super.port);

  TunnelPath path = TunnelPath.direct;

  @override
  TunnelStatus get status => tunnelStatus(path: path);
}

/// Tunnels that never open.
class _FailingTunnels implements Tunnels {
  @override
  Future<Tunnel> open({
    required String deviceKey,
    required String endpointId,
    required List<String> relays,
    List<String> addresses = const [],
  }) async => throw const TunnelOpenFailure('no network');

  @override
  Future<({String device, String backend})> pairingIds({
    required String deviceKey,
    required String endpointId,
  }) async => (device: 'b' * 64, backend: 'a' * 64);
}

class FakeTunnels implements Tunnels {
  FakeTunnels(this.tunnel);

  final FakeTunnel tunnel;
  final opened = <(String, String, List<String>)>[];
  final addresses = <List<String>>[];

  @override
  Future<Tunnel> open({
    required String deviceKey,
    required String endpointId,
    required List<String> relays,
    List<String> addresses = const [],
  }) async {
    opened.add((deviceKey, endpointId, relays));
    this.addresses.add(addresses);
    return tunnel;
  }

  /// Endpoint ids of 64 "b"s for this device and 64 "a"s for the backend,
  /// as in the pairing number's example.
  @override
  Future<({String device, String backend})> pairingIds({
    required String deviceKey,
    required String endpointId,
  }) async => (device: 'b' * 64, backend: 'a' * 64);
}

Backend backendWith(Tunnels tunnels, {RemoteFactory? local}) => Backend(
  workspace: WorkspaceStore(),
  transcript: TranscriptStore(),
  explorer: ExplorerStore(),
  extensionView: ExtensionViewStore(),
  tunnels: tunnels,
  localPairing: () async => local,
);

/// A backend's end of the tunnel: the first Hello it gets.
Future<(HttpServer, Future<ClientMessage>)> helloServer() async {
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
  return (server, hello.future);
}

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

    test('carry direct addresses', () {
      final target = parsePairingCode(
        'factory://?t=$_token&e=$_endpoint&a=127.0.0.1%3A4919'
        '&a=%5B%3A%3A1%5D%3A4919&a=nowhere',
      )!;
      expect(target.addresses, ['127.0.0.1:4919', '[::1]:4919']);
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
      'plain ws to the tunnel\'s port and path, the token in Hello', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final hello = Completer<ClientMessage>();
    final asked = <String>[];
    server.listen((request) async {
      asked.add(request.uri.path);
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
    // At the tunnel's own path: only that is forwarded.
    expect(asked, [tunnels.tunnel.requestPath]);
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

  test('an open tunnel tells listeners only when its status changes', () async {
    final (server, hello) = await helloServer();
    final tunnel = _FreshStatusTunnel(server.port);
    final backend = backendWith(FakeTunnels(tunnel))
      ..deviceKey = (_) => 'k' * 64;
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await hello.timeout(const Duration(seconds: 5));
    var told = 0;
    backend.tunnel.addListener(() => told++);

    // Read every second, a new but equal status each time: nothing to tell.
    await Future<void>.delayed(const Duration(milliseconds: 2500));
    expect(told, 0);
    tunnel.path = TunnelPath.relayed;
    await Future<void>.delayed(const Duration(milliseconds: 1200));
    expect(told, 1);
    expect(backend.tunnel.value?.path, TunnelPath.relayed);
    await backend.close();
    await server.close(force: true);
  });

  test("this Mac's Factory is reached through the tunnel with its own "
      'code: its direct address, its token, a key of its own', () async {
    final (server, hello) = await helloServer();
    final tunnels = FakeTunnels(FakeTunnel(server.port));
    final local = parsePairingCode(
      'factory://?t=$_token&name=this-mac&e=$_endpoint&a=127.0.0.1%3A4919',
    )!;
    final backend = backendWith(tunnels, local: local)
      ..deviceKey = (_) => throw StateError('not a paired device');
    unawaited(backend.run());

    final message = await hello.timeout(const Duration(seconds: 5));
    expect(message.hello.token, _token);
    final (key, endpoint, relays) = tunnels.opened.single;
    expect(key, matches(RegExp(r'^[0-9a-f]{64}$')));
    expect(endpoint, _endpoint);
    expect(relays, isEmpty);
    expect(tunnels.addresses.single, ['127.0.0.1:4919']);
    await backend.close();
    await server.close(force: true);
  });

  test("this Mac's Factory, its tunnel failing, is reached on its open "
      "port with its code's token", () async {
    final (server, hello) = await helloServer();
    final local = parsePairingCode(
      'factory://?t=$_token&name=this-mac&e=$_endpoint',
    )!;
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      tunnels: _FailingTunnels(),
      localPairing: () async => local,
      localEndpoint: 'ws://127.0.0.1:${server.port}',
    );
    unawaited(backend.run());
    final message = await hello.timeout(const Duration(seconds: 10));
    expect(message.hello.token, _token);
    await backend.close();
    await server.close(force: true);
  });

  test("refused by this Mac's own Factory (its code not written yet), it "
      'tries again rather than giving up', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    var hellos = 0;
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (!message.hasHello()) return;
        hellos++;
        socket.add(
          ServerMessage(
            replyTo: message.id,
            error: Error(
              code: ErrorCode.ERROR_CODE_UNAUTHORIZED,
              message: 'This Claktory needs its token.',
            ),
          ).writeToBuffer(),
        );
      });
    });
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      tunnels: _FailingTunnels(),
      localPairing: () async => null,
      localEndpoint: 'ws://127.0.0.1:${server.port}',
    );
    unawaited(backend.run());
    final end = DateTime.now().add(const Duration(seconds: 10));
    while (hellos < 2 && DateTime.now().isBefore(end)) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(hellos, greaterThanOrEqualTo(2));
    expect(backend.refused.value, isNull);
    await backend.close();
    await server.close(force: true);
  });

  test('back from the background, it reconnects at once and quietly', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    var hellos = 0;
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (message.hasHello()) {
          hellos++;
          socket.add(
            ServerMessage(
              replyTo: message.id,
              welcome: Welcome(serverVersion: 'test'),
            ).writeToBuffer(),
          );
        }
      });
    });
    final tunnels = FakeTunnels(FakeTunnel(server.port));
    final backend = backendWith(tunnels)..deviceKey = (_) => 'k' * 64;
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    Future<void> until(bool Function() done) async {
      for (var i = 0; i < 100 && !done(); i++) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      expect(done(), isTrue);
    }

    await until(() => backend.connected.value);
    expect(hellos, 1);

    // A moment away changes nothing.
    backend.hidden();
    backend.resumed();
    expect(backend.reconnecting.value, isFalse);
    expect(tunnels.tunnel.reconnects, 0);

    // Longer, and it dials afresh at once, saying nothing meanwhile.
    backend.hidden();
    backend.resumed(now: DateTime.now().add(const Duration(minutes: 1)));
    expect(backend.reconnecting.value, isTrue);
    expect(tunnels.tunnel.reconnects, 1);
    await until(() => hellos == 2 && backend.connected.value);
    expect(backend.reconnecting.value, isFalse);
    await backend.close();
    await server.close(force: true);
  });

  test('a check the backend answers changes nothing; one it doesn\'t '
      'answer reconnects afresh', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    var hellos = 0;
    var silent = false;
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (message.hasHello()) {
          hellos++;
          silent = false;
          socket.add(
            ServerMessage(
              replyTo: message.id,
              welcome: Welcome(serverVersion: 'test'),
            ).writeToBuffer(),
          );
        } else if (!silent) {
          // As factoryd answers a message it doesn't know.
          socket.add(
            ServerMessage(
              replyTo: message.id,
              error: Error(
                code: ErrorCode.ERROR_CODE_UNKNOWN_MESSAGE,
                message: 'unknown message',
              ),
            ).writeToBuffer(),
          );
        }
      });
    });
    final tunnels = FakeTunnels(FakeTunnel(server.port));
    final backend = backendWith(tunnels)
      ..answerWithin = const Duration(seconds: 1)
      ..deviceKey = (_) => 'k' * 64;
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    Future<void> until(bool Function() done) async {
      for (var i = 0; i < 250 && !done(); i++) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      expect(
        done(),
        isTrue,
        reason:
            'hellos $hellos reconnects ${tunnels.tunnel.reconnects} connected ${backend.connected.value}',
      );
    }

    await until(() => backend.connected.value);
    backend.check();
    // Past the time to answer, so a reconnect would have begun.
    await Future<void>.delayed(const Duration(milliseconds: 1400));
    expect(hellos, 1, reason: 'answered: the connection stays');
    expect(tunnels.tunnel.reconnects, 0);

    // The other end has gone quiet, as after the Mac slept.
    silent = true;
    backend.check();
    await until(() => tunnels.tunnel.reconnects == 1);
    expect(backend.reconnecting.value, isTrue);
    await until(() => hellos == 2 && backend.connected.value);
    expect(tunnels.tunnel.reconnects, 1, reason: 'the tunnel redials once');
    await backend.close();
    await server.close(force: true);
  });

  test('earlier entries are asked for and put before the ones held', () async {
    Entry numbered(int seq) => Entry(
      seq: Int64(seq),
      user: UserText(text: 'message $seq'),
    );
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final asked = <LoadEntries>[];
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (message.hasHello()) {
          socket
            ..add(
              ServerMessage(
                replyTo: message.id,
                welcome: Welcome(serverVersion: 'test'),
              ).writeToBuffer(),
            )
            ..add(
              ServerMessage(
                transcript: Transcript(
                  agentId: 'a',
                  entries: [numbered(10), numbered(11)],
                  hasEarlier: true,
                ),
              ).writeToBuffer(),
            );
        } else if (message.hasLoadEntries()) {
          asked.add(message.loadEntries);
          socket.add(
            ServerMessage(
              replyTo: message.id,
              entryPage: EntryPage(
                agentId: 'a',
                entries: [numbered(8), numbered(9)],
                hasEarlier: false,
              ),
            ).writeToBuffer(),
          );
        }
      });
    });
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = (_) => 'k' * 64;
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    for (var i = 0; i < 250 && backend.transcript.length < 2; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(backend.transcript.hasEarlier, isTrue);

    await backend.loadEarlier('a');
    expect(asked.single.agentId, 'a');
    expect(asked.single.beforeSeq.toInt(), 10, reason: 'the first one held');
    expect(
      [for (var i = 0; i < 4; i++) backend.transcript[i].seq],
      [8, 9, 10, 11],
    );
    expect(backend.transcript.hasEarlier, isFalse);
    expect(backend.transcript.earlier, 2);
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
    await tester.tap(find.text("Can't connect to mac-mini"));
    await settle(tester);
    expect(find.byType(ConnectionDialog), findsOneWidget);
    expect(find.text('Nothing answered within 20 seconds.'), findsOneWidget);
    expect(
      find.textContaining('Make sure mac-mini is on, awake and online'),
      findsOneWidget,
    );
    await tester.tap(find.text('Reconnect'));
    await settle(tester);
    expect(find.byType(ConnectionDialog), findsNothing);
  });

  testWidgets('reconnecting after a resume or a drop, the footer says '
      'nothing for a few seconds', (tester) async {
    // What showed before stays: the agent's conversation.
    final snapshot = Workspace(
      agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
      selectedAgentId: 'rivet',
    );
    final harness = Harness(snapshot);
    harness.backend.connected.value = false;
    harness.backend.connectTo(parsePairingCode(_code));
    harness.backend.workspace.apply(snapshot);
    harness.backend.reconnecting.value = true;
    harness.backend.tunnel.value = tunnelStatus(
      phase: TunnelPhase.failed,
      path: TunnelPath.none,
      failure: TunnelFailure.unreachable,
      error: 'The connection to the machine timed out.',
    );
    await harness.show(tester);
    await settle(tester);
    expect(find.textContaining("Can't connect"), findsNothing);
    expect(find.textContaining('Connecting'), findsNothing);
    harness.backend.reconnecting.value = false;
    await settle(tester);
    expect(find.text("Can't connect to mac-mini"), findsOneWidget);
  });

  testWidgets('Settings → Devices says this device is connected', (
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
    final line = find.descendant(
      of: find.byType(SettingsView),
      matching: find.text('Connected to mac-mini'),
    );
    // Settings builds its sections as they scroll into view.
    await tester.scrollUntilVisible(
      line,
      200,
      scrollable: find
          .descendant(
            of: find.byType(SettingsView),
            matching: find.byType(Scrollable),
          )
          .first,
    );
    expect(line, findsOneWidget);
    expect(find.text('Private and encrypted.'), findsOneWidget);
    expect(find.textContaining('relay'), findsNothing);
  });
}
