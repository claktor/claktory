// Polpo's idle-keepalive.md (Matt's call): a phone or tablet left alone,
// with nothing arriving, rests its connection. The app pings every 25 s
// instead of every 5, and the tunnel checks its paths every 25 s; a touch
// ends it at once and the tunnel checks the link then. What arrives
// meanwhile doesn't end it, and a Mac never rests.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/user_input.dart';
import 'package:flutter_test/flutter_test.dart';

import 'reconnect_from_scratch_test.dart' show until;
import 'tunnel_test.dart' show FakeTunnel, FakeTunnels;

final _code =
    'factory://?t=${'a' * 64}&name=mac-mini&e=${'c' * 64}'
    '&r=https://relay-eu.example.com';

/// A Claktory at the other end of the tunnel: it welcomes a Hello and
/// answers everything else, so no request is left waiting.
Future<(HttpServer, List<WebSocket>)> _claktory() async {
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  final sockets = <WebSocket>[];
  server.listen((request) async {
    final socket = await WebSocketTransformer.upgrade(request);
    sockets.add(socket);
    socket.listen((frame) {
      final message = ClientMessage.fromBuffer(frame as List<int>);
      socket.add(
        (message.hasHello()
                ? ServerMessage(
                    replyTo: message.id,
                    welcome: Welcome(serverVersion: 'test'),
                  )
                : ServerMessage(
                    replyTo: message.id,
                    error: Error(
                      code: ErrorCode.ERROR_CODE_UNKNOWN_MESSAGE,
                      message: 'unknown message',
                    ),
                  ))
            .writeToBuffer(),
      );
    });
  });
  return (server, sockets);
}

Future<(Backend, FakeTunnel)> _connected(HttpServer server) async {
  final tunnel = FakeTunnel(server.port);
  final backend = Backend(
    workspace: WorkspaceStore(),
    transcript: TranscriptStore(),
    explorer: ExplorerStore(),
    extensionView: ExtensionViewStore(),
    tunnels: FakeTunnels(tunnel),
    localPairing: () async => null,
  )..deviceKey = ((_) => 'k' * 64);
  backend.connectTo(parsePairingCode(_code));
  unawaited(backend.run());
  await until(() => backend.connected.value);
  return (backend, tunnel);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  void idleAfter(Duration after, {bool device = true}) {
    final (before, onDevice) = (Backend.idleAfter, Backend.idleOnThisDevice);
    Backend.idleAfter = after;
    Backend.idleOnThisDevice = device;
    addTearDown(() {
      Backend.idleAfter = before;
      Backend.idleOnThisDevice = onDevice;
    });
  }

  test('left alone it rests its connection; a touch wakes it at once, and '
      'what arrives meanwhile leaves it resting', () async {
    idleAfter(const Duration(milliseconds: 400));
    final (server, sockets) = await _claktory();
    addTearDown(() => server.close(force: true));
    final (backend, tunnel) = await _connected(server);
    addTearDown(backend.close);
    expect(backend.pingInterval, const Duration(seconds: 5));

    await until(() => tunnel.idleTold.isNotEmpty);
    expect(tunnel.idleTold, [true]);
    expect(backend.pingInterval, const Duration(seconds: 25));

    // The Claktory sends something: the link is plainly alive, and the
    // person still isn't looking.
    for (final socket in sockets) {
      socket.add(ServerMessage(workspace: Workspace()).writeToBuffer());
    }
    await Future<void>.delayed(const Duration(milliseconds: 200));
    expect(tunnel.idleTold, [true], reason: 'still resting');
    expect(backend.pingInterval, const Duration(seconds: 25));

    UserInput.instance.touched();
    expect(tunnel.idleTold, [true, false], reason: 'woken at once');
    expect(backend.pingInterval, const Duration(seconds: 5));

    // Alone again, it rests again.
    await until(() => tunnel.idleTold.length == 3);
    expect(tunnel.idleTold, [true, false, true]);
    expect(backend.pingInterval, const Duration(seconds: 25));

    // Back from the background is a touch too.
    backend.resumed();
    expect(tunnel.idleTold.last, isFalse);
    expect(backend.pingInterval, const Duration(seconds: 5));
  });

  test('a Mac never rests its connection', () async {
    idleAfter(const Duration(milliseconds: 100), device: false);
    final (server, _) = await _claktory();
    addTearDown(() => server.close(force: true));
    final (backend, tunnel) = await _connected(server);
    addTearDown(backend.close);
    await Future<void>.delayed(const Duration(milliseconds: 600));
    expect(tunnel.idleTold, isEmpty);
    expect(backend.pingInterval, const Duration(seconds: 5));
  });
}
