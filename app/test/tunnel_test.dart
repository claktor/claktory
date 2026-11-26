import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:factory_app/loopback_forward.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/background_grace.dart';
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
  int relayUpMs = 0,
  TunnelRelayTrouble trouble = TunnelRelayTrouble.none,
  int received = 0,
  int linkReceived = 0,
  int dialTimeoutMs = 0,
  TunnelReach reach = TunnelReach.none,
}) => TunnelStatus(
  phase: phase,
  path: path,
  error: error,
  failure: failure,
  relayConnected: relay,
  relayUpMs: relayUpMs,
  relayTrouble: trouble,
  received: received,
  linkReceived: linkReceived,
  dialTimeoutMs: dialTimeoutMs,
  reach: reach,
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

  /// What [setRelays] was told, in order.
  final relaysTold = <List<String>>[];

  @override
  void setRelays(List<String> relays) => relaysTold.add(relays);

  /// The ports asked for; [forwardPort] answers what [forwarded] makes.
  final forwards = <int>[];
  Future<PortForward> Function(int port) forwarded = (_) async =>
      throw const ForwardFailure(ForwardProblem.unsupported);

  @override
  Future<PortForward> forwardPort(int port, {int? target}) {
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

/// Tunnels that open each of [tunnels] in turn, the last again after.
class _TunnelsInTurn extends FakeTunnels {
  _TunnelsInTurn(this.tunnels) : super(tunnels.last);

  final List<FakeTunnel> tunnels;

  @override
  Future<Tunnel> open({
    required String deviceKey,
    required String endpointId,
    required List<String> relays,
    List<String> addresses = const [],
  }) async {
    await super.open(
      deviceKey: deviceKey,
      endpointId: endpointId,
      relays: relays,
      addresses: addresses,
    );
    return tunnels[(opened.length - 1).clamp(0, tunnels.length - 1)];
  }
}

/// Tunnels whose open finishes only when [opens] completes.
class _SlowTunnels extends FakeTunnels {
  _SlowTunnels(super.tunnel, this.opens);

  final Future<void> opens;

  @override
  Future<Tunnel> open({
    required String deviceKey,
    required String endpointId,
    required List<String> relays,
    List<String> addresses = const [],
  }) async {
    final tunnel = await super.open(
      deviceKey: deviceKey,
      endpointId: endpointId,
      relays: relays,
      addresses: addresses,
    );
    await opens;
    return tunnel;
  }
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

  test('a tunnel that opens after its connection closed is closed too, '
      'never left on the relay as this device', () async {
    final opens = Completer<void>();
    final tunnel = FakeTunnel(1);
    final tunnels = _SlowTunnels(tunnel, opens.future);
    final backend = backendWith(tunnels)..deviceKey = (_) => 'k' * 64;
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await pumpEventQueue();
    expect(tunnels.opened, hasLength(1));
    await backend.close();
    expect(tunnel.closed, isFalse);
    opens.complete();
    await pumpEventQueue();
    expect(tunnel.closed, isTrue);
    expect(tunnels.opened, hasLength(1));
    // Quietly: a connection being closed shows no failure.
    expect(backend.tunnel.value, isNull);
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
    // The system it runs on, for the check-ins' counts (tests run as
    // Android).
    expect(message.hello.platform, 'android');
    // Its release version; an unstamped build says so.
    expect(message.hello.clientVersion, 'dev');
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

  test('back within the grace the device kept it running, it only checks '
      'the connection (#413); after it, or with none, it reconnects', () async {
    addTearDown(() => BackgroundGrace.debugKind = null);
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    var hellos = 0;
    var checks = 0;
    var answer = true;
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
        } else if (message == ClientMessage(id: message.id)) {
          checks++;
          if (answer) {
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
        }
      });
    });
    final tunnels = FakeTunnels(FakeTunnel(server.port));
    final backend = backendWith(tunnels)..deviceKey = (_) => 'k' * 64;
    backend.answerWithin = const Duration(milliseconds: 300);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    Future<void> until(bool Function() done) async {
      for (var i = 0; i < 150 && !done(); i++) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      expect(done(), isTrue);
    }

    await until(() => backend.connected.value);
    final later = DateTime.now().add(const Duration(seconds: 10));

    // Android keeps the app running: 10 s away is only checked, and the
    // answer leaves the connection as it was.
    BackgroundGrace.debugKind = GraceKind.timed;
    backend.hidden();
    backend.resumed(now: later);
    expect(backend.reconnecting.value, isFalse);
    expect(tunnels.tunnel.reconnects, 0);
    await until(() => checks == 1);
    await Future<void>.delayed(const Duration(milliseconds: 500));
    expect(hellos, 1);
    expect(backend.connected.value, isTrue);

    // A check that goes unanswered reconnects, as before the grace.
    answer = false;
    backend.hidden();
    backend.resumed(now: later);
    await until(() => checks == 2);
    await until(() => hellos == 2 && backend.connected.value);
    expect(tunnels.tunnel.reconnects, 1);
    answer = true;

    // Past the grace, it reconnects at once.
    backend.hidden();
    backend.resumed(now: DateTime.now().add(const Duration(seconds: 30)));
    expect(tunnels.tunnel.reconnects, 2);
    await until(() => hellos == 3 && backend.connected.value);

    // On iOS, only when iOS gave the time: here it never said it did.
    BackgroundGrace.debugKind = GraceKind.native;
    backend.hidden();
    backend.resumed(now: later);
    expect(tunnels.tunnel.reconnects, 3);
    await until(() => hellos == 4 && backend.connected.value);
    expect(checks, 2);
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
    backend.resumed(now: DateTime.now().add(const Duration(seconds: 10)));
    expect(backend.reconnecting.value, isTrue);
    expect(tunnels.tunnel.reconnects, 1);
    await until(() => hellos == 2 && backend.connected.value);
    expect(backend.reconnecting.value, isFalse);
    expect(tunnels.opened, hasLength(1));

    // However long, the tunnel only reconnects (it makes its endpoint
    // afresh itself when that's needed): the same tunnel, told once.
    backend.hidden();
    backend.resumed(now: DateTime.now().add(const Duration(hours: 1)));
    expect(backend.reconnecting.value, isTrue);
    expect(tunnels.tunnel.reconnects, 2);
    // The phone's clock jumped too, so a check comes at once: the old
    // connection is closing, and that's no error; nor does it tell the
    // tunnel again while it's bringing the connection back.
    backend.check();
    expect(tunnels.tunnel.reconnects, 2);
    await until(() => hellos == 3 && backend.connected.value);
    expect(tunnels.tunnel.closed, isFalse);
    expect(tunnels.opened, hasLength(1));
    expect(backend.reconnecting.value, isFalse);
    await backend.close();
    await server.close(force: true);
  });

  test(
    'back from the background, a tunnel that does not bring the '
    'connection back in time is told once, then opened afresh, once',
    () async {
      // The first tunnel's backend stops welcoming (its connection dead);
      // the second's welcomes.
      var welcoming = true;
      Future<HttpServer> serve(bool Function() welcomes) async {
        final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
        server.listen((request) async {
          final socket = await WebSocketTransformer.upgrade(request);
          socket.listen((frame) {
            final message = ClientMessage.fromBuffer(frame as List<int>);
            if (message.hasHello() && welcomes()) {
              socket.add(
                ServerMessage(
                  replyTo: message.id,
                  welcome: Welcome(serverVersion: 'test'),
                ).writeToBuffer(),
              );
            }
          });
        });
        return server;
      }

      final stuck = await serve(() => welcoming);
      final fine = await serve(() => true);
      final first = FakeTunnel(stuck.port);
      final tunnels = _TunnelsInTurn([first, FakeTunnel(fine.port)]);
      final backend = backendWith(tunnels)
        ..deviceKey = ((_) => 'k' * 64)
        ..connectWithin = const Duration(milliseconds: 150)
        ..tunnelBackWithin = const Duration(milliseconds: 800);
      backend.connectTo(parsePairingCode(_code));
      unawaited(backend.run());
      Future<void> until(bool Function() done) async {
        for (var i = 0; i < 150 && !done(); i++) {
          await Future<void>.delayed(const Duration(milliseconds: 20));
        }
        expect(done(), isTrue);
      }

      await until(() => backend.connected.value);
      welcoming = false;
      backend.hidden();
      backend.resumed(now: DateTime.now().add(const Duration(minutes: 5)));
      expect(first.reconnects, 1);
      // Tries that fail meanwhile (no welcome in time) don't tell it again.
      await Future<void>.delayed(const Duration(milliseconds: 600));
      expect(backend.connected.value, isFalse);
      expect(first.reconnects, 1);
      expect(tunnels.opened, hasLength(1));
      // Past its time, it's closed, and a new one opened once it has.
      await until(() => backend.connected.value);
      expect(first.closed, isTrue);
      expect(first.reconnects, 1);
      expect(tunnels.opened, hasLength(2));
      await backend.close();
      await stuck.close(force: true);
      await fine.close(force: true);
    },
  );

  test('a try that runs out of time gives the next longer, up to three '
      'times as long; welcomed, the first again (#413)', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    var welcoming = false;
    // How long each connection the app gave up on stayed open.
    final held = <Duration>[];
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      final opened = DateTime.now();
      socket.listen(
        (frame) {
          final message = ClientMessage.fromBuffer(frame as List<int>);
          if (message.hasHello() && welcoming) {
            socket.add(
              ServerMessage(
                replyTo: message.id,
                welcome: Welcome(serverVersion: 'test'),
              ).writeToBuffer(),
            );
          }
        },
        onDone: () {
          if (!welcoming) held.add(DateTime.now().difference(opened));
        },
      );
    });
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..connectWithin = const Duration(milliseconds: 300);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    Future<void> until(bool Function() done) async {
      for (var i = 0; i < 400 && !done(); i++) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      expect(done(), isTrue);
    }

    await until(() => held.length >= 4);
    final ms = [for (final each in held.take(4)) each.inMilliseconds];
    expect(ms[0], inInclusiveRange(250, 500), reason: '$ms');
    expect(ms[1], inInclusiveRange(550, 800), reason: '$ms');
    expect(ms[2], inInclusiveRange(850, 1100), reason: '$ms');
    expect(ms[3], inInclusiveRange(850, 1100), reason: '$ms');
    welcoming = true;
    await until(() => backend.connected.value);
    // Welcomed: the next that runs out of time had the first window.
    welcoming = false;
    held.clear();
    backend.networkMoved();
    await until(() => held.length >= 2);
    expect(held[1].inMilliseconds, inInclusiveRange(250, 500), reason: '$held');
    await backend.close();
    await server.close(force: true);
  });

  test('a Welcome slower than its window is waited for while the '
      "tunnel's link still answers, and one try does (#413)", () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    var hellos = 0;
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) async {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (!message.hasHello()) return;
        hellos++;
        // Behind everything else on a weak link.
        await Future<void>.delayed(const Duration(milliseconds: 1500));
        socket.add(
          ServerMessage(
            replyTo: message.id,
            welcome: Welcome(serverVersion: 'test'),
          ).writeToBuffer(),
        );
      });
    });
    final backend = backendWith(FakeTunnels(_AnsweringLink(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..connectWithin = const Duration(milliseconds: 300);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    for (var i = 0; i < 150 && !backend.connected.value; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(backend.connected.value, isTrue);
    expect(hellos, 1);
    await backend.close();
    await server.close(force: true);
  });

  test("through a tunnel, a try waits at least as long as the tunnel's "
      'dial has, and a little more (#413)', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    // The connection comes slowly: the tunnel still dialing.
    server.listen((request) async {
      await Future<void>.delayed(const Duration(milliseconds: 700));
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (message.hasHello()) {
          socket.add(
            ServerMessage(
              replyTo: message.id,
              welcome: Welcome(serverVersion: 'test'),
            ).writeToBuffer(),
          );
        }
      });
    });
    final tunnel = FakeTunnel(server.port)
      ..status = tunnelStatus(dialTimeoutMs: 500);
    final backend = backendWith(FakeTunnels(tunnel))
      ..deviceKey = ((_) => 'k' * 64)
      // Shorter than the slow connection: it'd never connect on its own.
      ..connectWithin = const Duration(milliseconds: 200);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    final began = DateTime.now();
    for (var i = 0; i < 100 && !backend.connected.value; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(backend.connected.value, isTrue);
    expect(
      DateTime.now().difference(began),
      lessThan(const Duration(milliseconds: 1500)),
    );
    await backend.close();
    await server.close(force: true);
  });

  test('in the background, a dropped connection waits for the app to be '
      'back: no reconnect, no new tunnel, no tries meanwhile', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    var welcoming = true;
    var attempts = 0;
    final sockets = <WebSocket>[];
    server.listen((request) async {
      attempts++;
      final socket = await WebSocketTransformer.upgrade(request);
      if (!welcoming) {
        await socket.close();
        return;
      }
      sockets.add(socket);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (message.hasHello()) {
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
    final backend = backendWith(tunnels)
      ..deviceKey = ((_) => 'k' * 64)
      ..connectWithin = const Duration(milliseconds: 150)
      ..tunnelBackWithin = const Duration(milliseconds: 300);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    Future<void> until(bool Function() done) async {
      for (var i = 0; i < 150 && !done(); i++) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      expect(done(), isTrue);
    }

    await until(() => backend.connected.value);
    // Hidden (Android keeps the app running), then its connection drops,
    // and the backend turns tries away for a while.
    backend.hidden();
    welcoming = false;
    final before = attempts;
    for (final socket in sockets) {
      await socket.close();
    }
    await until(() => !backend.connected.value);
    backend.check();
    await Future<void>.delayed(const Duration(seconds: 1));
    expect(tunnels.tunnel.reconnects, 0);
    expect(tunnels.tunnel.closed, isFalse);
    expect(tunnels.opened, hasLength(1));
    expect(attempts - before, lessThanOrEqualTo(1));

    // Back: told once, and connected.
    welcoming = true;
    backend.resumed(now: DateTime.now().add(const Duration(minutes: 5)));
    await until(() => backend.connected.value);
    expect(tunnels.tunnel.reconnects, 1);
    expect(tunnels.opened, hasLength(1));
    await backend.close();
    await server.close(force: true);
  });

  test("its tunnel's port no longer answering (taken back while the app "
      'was away), it opens the tunnel afresh at once', () async {
    final firstSockets = <WebSocket>[];
    Future<HttpServer> serve(
      void Function() hello, [
      List<WebSocket>? kept,
    ]) async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      server.listen((request) async {
        final socket = await WebSocketTransformer.upgrade(request);
        kept?.add(socket);
        socket.listen((frame) {
          final message = ClientMessage.fromBuffer(frame as List<int>);
          if (message.hasHello()) {
            hello();
            socket.add(
              ServerMessage(
                replyTo: message.id,
                welcome: Welcome(serverVersion: 'test'),
              ).writeToBuffer(),
            );
          }
        });
      });
      return server;
    }

    var firstHellos = 0;
    var hellos = 0;
    final first = await serve(() => firstHellos++, firstSockets);
    final server = await serve(() => hellos++);
    final taken = FakeTunnel(first.port);
    final tunnels = _TunnelsInTurn([taken, FakeTunnel(server.port)]);
    final backend = backendWith(tunnels)..deviceKey = (_) => 'k' * 64;
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    for (var i = 0; i < 100 && !backend.connected.value; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(backend.connected.value, isTrue);
    expect(firstHellos, 1);
    // The system takes its port back: the connection drops, and nothing
    // listens there any more.
    final started = DateTime.now();
    await first.close(force: true);
    for (final socket in firstSockets) {
      await socket.close();
    }
    for (var i = 0; i < 50 && backend.connected.value; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    for (var i = 0; i < 100 && !backend.connected.value; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(backend.connected.value, isTrue);
    // Well within the time the tunnel is given to come back on its own.
    expect(
      DateTime.now().difference(started),
      lessThan(const Duration(seconds: 2)),
    );
    expect(taken.closed, isTrue);
    expect(tunnels.opened, hasLength(2));
    expect(hellos, 1);
    await backend.close();
    await server.close(force: true);
  });

  test("a tunnel whose port never answered isn't opened afresh over and "
      'over: it stays, so why it can\'t connect shows', () async {
    // A port nothing listens on.
    final gone = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final dead = FakeTunnel(gone.port)
      ..status = tunnelStatus(
        phase: TunnelPhase.failed,
        path: TunnelPath.none,
        failure: TunnelFailure.unreachable,
        error: "Can't reach the Mac",
      );
    await gone.close();
    final tunnels = FakeTunnels(dead);
    final backend = backendWith(tunnels)..deviceKey = (_) => 'k' * 64;
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    for (var i = 0; i < 100 && backend.tunnel.value == null; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    await Future<void>.delayed(const Duration(milliseconds: 500));
    expect(backend.connected.value, isFalse);
    expect(backend.tunnel.value?.failure, TunnelFailure.unreachable);
    expect(dead.closed, isFalse);
    expect(tunnels.opened, hasLength(1));
    await backend.close();
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

  test('the tunnel follows the relays its Claktory says it uses, told '
      'only when they change (#450)', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    WebSocket? connected;
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      connected = socket;
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (message.hasHello()) {
          socket.add(
            ServerMessage(
              replyTo: message.id,
              welcome: Welcome(serverVersion: 'test'),
            ).writeToBuffer(),
          );
        }
      });
    });
    const n0s = ['https://use1-1.relay.n0.iroh.link./'];
    const ours = ['https://relay.claktory.com/', ...n0s];
    void send(Workspace workspace, String revision) => connected!.add(
      ServerMessage(
        workspace: workspace,
        workspaceRevision: revision,
      ).writeToBuffer(),
    );
    final tunnels = FakeTunnels(FakeTunnel(server.port));
    final backend = backendWith(tunnels)..deviceKey = (_) => 'k' * 64;
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    Future<void> until(bool Function() done) async {
      for (var i = 0; i < 250 && !done(); i++) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      expect(done(), isTrue);
    }

    await until(() => backend.connected.value);
    // Not signed in: n0's.
    send(Workspace(remote: RemoteAccess(on: true, relays: n0s)), '1');
    await until(() => tunnels.tunnel.relaysTold.isNotEmpty);
    expect(tunnels.tunnel.relaysTold, [n0s]);
    // The same again tells it nothing; none at all neither.
    send(Workspace(remote: RemoteAccess(on: true, relays: n0s)), '2');
    send(Workspace(remote: RemoteAccess(on: true)), '3');
    send(Workspace(), '4');
    // Signed in: Claktory's.
    send(Workspace(remote: RemoteAccess(on: true, relays: ours)), '5');
    await until(() => tunnels.tunnel.relaysTold.length == 2);
    expect(tunnels.tunnel.relaysTold, [n0s, ours]);
    // Made from scratch (as while it can't connect), the new tunnel homes
    // on the relays it last said, not the pairing code's (Abbacchio, #37).
    expect(tunnels.opened, hasLength(1));
    backend.reconnectNow();
    await until(() => tunnels.opened.length == 2);
    expect(tunnels.opened.last.$3, ours);
    await until(() => backend.connected.value);
    send(Workspace(remote: RemoteAccess(on: true, relays: ours)), '6');
    await Future<void>.delayed(const Duration(milliseconds: 100));
    expect(tunnels.tunnel.relaysTold, [n0s, ours], reason: 'told already');
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
    await tester.tap(find.text("Can't connect to mac-mini").first);
    await settle(tester);
    // Not connected: the click reconnects at once too (#214).
    expect(harness.backend.reconnectsAsked, 1);
    expect(find.byType(ConnectionDialog), findsOneWidget);
    expect(find.text('Nothing answered within 20 seconds.'), findsOneWidget);
    expect(
      find.textContaining('Make sure mac-mini is on, awake and online'),
      findsOneWidget,
    );
    // The dialog's (the main pane behind it has one too, row 480).
    await tester.tap(
      find.descendant(
        of: find.byType(ConnectionDialog),
        matching: find.text('Reconnect'),
      ),
    );
    await settle(tester);
    expect(find.byType(ConnectionDialog), findsNothing);
    expect(harness.backend.reconnectsAsked, 2);
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
    // The footer, and the line above the conversation's message box.
    expect(find.text("Can't connect to mac-mini"), findsNWidgets(2));
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

/// A tunnel whose link keeps answering: its own traffic arrives at every
/// read, as QUIC's acknowledgements and heartbeats do.
class _AnsweringLink extends FakeTunnel {
  _AnsweringLink(super.port);

  var _link = 0;

  @override
  TunnelStatus get status => tunnelStatus(linkReceived: _link += 100);
}
