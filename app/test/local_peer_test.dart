// The Mac app checks who listens at its own
// Claktory's socket before saying a word there. Something else in its
// place hears nothing, not even a Hello, and the app says so and keeps
// trying; once the real backend is back, it connects.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:flutter_test/flutter_test.dart';

import 'network_test.dart' show until;
import 'tunnel_test.dart' show FakeTunnel, FakeTunnels;

void main() {
  test("a backend that isn't Claktory's own hears nothing, and the app says "
      'so until the real one answers', () async {
    final folder = await Directory.systemTemp.createTemp('peer');
    addTearDown(() => folder.delete(recursive: true));
    final path = '${folder.path}/app.sock';
    final server = await HttpServer.bind(
      InternetAddress(path, type: InternetAddressType.unix),
      0,
    );
    addTearDown(() => server.close(force: true));
    final heard = <ClientMessage>[];
    var requests = 0;
    server.listen((request) async {
      requests++;
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        heard.add(message);
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

    var impostor = true;
    var checks = 0;
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      tunnels: FakeTunnels(FakeTunnel(0)),
      localSocket: path,
      localPeerCheck: (_) async {
        checks++;
        return impostor ? "it isn't Claktory's backend (-67050)" : null;
      },
    );
    unawaited(backend.run());
    await until(() => backend.localRefused.value != null);
    expect(
      backend.localRefused.value,
      "Another program on this Mac answered in Claktory's place, so the app "
      "didn't connect to it. If you didn't expect this, check what's running "
      'on this Mac. The app tries again by itself.',
    );
    expect(requests, 0, reason: 'not even the WebSocket upgrade');
    expect(heard, isEmpty);
    expect(backend.connected.value, isFalse);

    // The real one back: it's let in, and the words go.
    impostor = false;
    await until(() => backend.connected.value, seconds: 30);
    expect(checks, greaterThan(1));
    expect(heard.first.hasHello(), isTrue);
    expect(backend.localRefused.value, isNull);
    await backend.close();
  });
}
