// The Windows app reaching its own Claktory (windows.md section 3, B): the
// app-token goes in Hello only after the server end of the app's own
// connection is confirmed as this same Windows account (Fugo 48393), and
// never to anyone else.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/tunnel_client.dart';
import 'package:factory_app/windows_local.dart';
import 'package:flutter_test/flutter_test.dart';

final _token = 'ab' * 32;

class _NoTunnels implements Tunnels {
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

/// A loopback server that keeps every frame it gets, and the client port
/// of each connection it accepts.
class _Server {
  _Server._(this.server);

  final HttpServer server;
  final frames = <ClientMessage>[];
  final clientPorts = <int>[];

  static Future<_Server> start() async {
    final server = _Server._(
      await HttpServer.bind(InternetAddress.loopbackIPv4, 0),
    );
    server.server.listen((request) async {
      server.clientPorts.add(request.connectionInfo!.remotePort);
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        server.frames.add(ClientMessage.fromBuffer(frame as List<int>));
      });
    });
    return server;
  }
}

Backend _backend(_Server server, WindowsLocal local) => Backend(
  workspace: WorkspaceStore(),
  transcript: TranscriptStore(),
  explorer: ExplorerStore(),
  extensionView: ExtensionViewStore(),
  tunnels: _NoTunnels(),
  localPairing: () async => null,
  localEndpoint: 'ws://127.0.0.1:${server.server.port}',
  windowsLocal: local,
);

Future<void> _until(bool Function() done) async {
  final end = DateTime.now().add(const Duration(seconds: 10));
  while (!done() && DateTime.now().isBefore(end)) {
    await Future<void>.delayed(const Duration(milliseconds: 20));
  }
}

void main() {
  test('the token goes in Hello once the server end of its own connection '
      'is this account', () async {
    final server = await _Server.start();
    final asked = <(int, int)>[];
    final devs = <bool>[];
    final backend = _backend(
      server,
      WindowsLocal(
        readToken: ({required dev}) async {
          devs.add(dev);
          return _token;
        },
        sameUser: (serverPort, clientPort) async {
          asked.add((serverPort, clientPort));
          return true;
        },
      ),
    );
    unawaited(backend.run());
    await _until(() => server.frames.any((frame) => frame.hasHello()));
    final hello = server.frames.firstWhere((frame) => frame.hasHello()).hello;
    expect(hello.token, _token);
    // Asked about this very connection: the server's port, and the client
    // port the server saw it from.
    expect(asked.first, (server.server.port, server.clientPorts.first));
    // A test talks to the dev Claktory (backend.dart's localPort), so it
    // reads the dev one's token.
    expect(devs.first, isTrue);
    await backend.close();
    await server.server.close(force: true);
  });

  test('a port held by another account gets nothing, not even a Hello, and '
      'the app keeps trying', () async {
    final server = await _Server.start();
    var checks = 0;
    final backend = _backend(
      server,
      WindowsLocal(
        readToken: ({required dev}) async => _token,
        sameUser: (serverPort, clientPort) async {
          checks++;
          return false;
        },
      ),
    );
    unawaited(backend.run());
    await _until(() => checks >= 2);
    expect(checks, greaterThanOrEqualTo(2));
    expect(server.frames, isEmpty);
    expect(backend.connected.value, isFalse);
    await backend.close();
    await server.server.close(force: true);
  });

  test("a check that can't tell counts as another account", () async {
    final server = await _Server.start();
    var checks = 0;
    final backend = _backend(
      server,
      WindowsLocal(
        readToken: ({required dev}) async => _token,
        sameUser: (serverPort, clientPort) async {
          checks++;
          throw StateError('only on Windows');
        },
      ),
    );
    unawaited(backend.run());
    await _until(() => checks >= 1);
    await Future<void>.delayed(const Duration(milliseconds: 200));
    expect(server.frames, isEmpty);
    await backend.close();
    await server.server.close(force: true);
  });

  test('with no app-token yet it never connects, and reads it again on the '
      'next try', () async {
    final server = await _Server.start();
    var reads = 0;
    final backend = _backend(
      server,
      WindowsLocal(
        readToken: ({required dev}) async {
          reads++;
          return reads < 3 ? null : _token;
        },
        sameUser: (serverPort, clientPort) async => true,
      ),
    );
    unawaited(backend.run());
    await _until(() => server.frames.any((frame) => frame.hasHello()));
    expect(reads, greaterThanOrEqualTo(3));
    expect(server.clientPorts, hasLength(1));
    expect(
      server.frames.firstWhere((frame) => frame.hasHello()).hello.token,
      _token,
    );
    await backend.close();
    await server.server.close(force: true);
  });

  test("the default check, off Windows, says no (the Rust's Err)", () async {
    expect(await loopbackSameUser(7420, 50000), isFalse);
  });

  test("the app-token's path follows the backend's home", () {
    expect(
      appTokenPath({
        'LOCALAPPDATA': r'C:\Users\fred\AppData\Local',
      }, dev: false),
      r'C:\Users\fred\AppData\Local\Claktory\factory\app-token',
    );
    expect(
      appTokenPath({'LOCALAPPDATA': r'C:\Users\fred\AppData\Local'}, dev: true),
      r'C:\Users\fred\AppData\Local\Claktory\factory-dev\app-token',
    );
    expect(
      appTokenPath({
        'LOCALAPPDATA': r'C:\x',
        'FACTORY_HOME': r'D:\claktory',
      }, dev: true),
      r'D:\claktory\app-token',
    );
  });
}
