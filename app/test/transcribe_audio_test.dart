// transcribeAudio (backend.dart): a dictation's words come back later, as
// AudioTranscribed for the request that asked, not as the request's reply;
// and a connection that drops fails what's still waiting.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

/// A backend that welcomes, acks everything, and answers each
/// TranscribeAudio after another one's answer (so out of order), saying
/// what its upload id is, unless it's "never".
Future<HttpServer> fakeBackend(List<WebSocket> sockets) async {
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  server.listen((request) async {
    final socket = await WebSocketTransformer.upgrade(request);
    sockets.add(socket);
    final waiting = <TranscribeAudio>[];
    socket.listen((frame) {
      final message = ClientMessage.fromBuffer(frame as List<int>);
      if (message.hasHello()) {
        socket.add(
          ServerMessage(
            replyTo: message.id,
            welcome: Welcome(serverVersion: 'test'),
          ).writeToBuffer(),
        );
        return;
      }
      socket.add(
        ServerMessage(replyTo: message.id, ack: Ack()).writeToBuffer(),
      );
      if (!message.hasTranscribeAudio()) return;
      final asked = message.transcribeAudio;
      if (asked.uploadId == 'never') return;
      waiting.add(asked);
      if (waiting.length == 2) {
        for (final done in waiting.reversed) {
          socket.add(
            ServerMessage(
              audioTranscribed: AudioTranscribed(
                requestId: done.requestId,
                text: 'heard ${done.uploadId}',
                durationMs: Int64(1200),
              ),
            ).writeToBuffer(),
          );
        }
        waiting.clear();
      }
    });
  });
  return server;
}

void main() {
  test('each dictation gets its own words, whenever they come', () async {
    final sockets = <WebSocket>[];
    final server = await fakeBackend(sockets);
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      localEndpoint: 'ws://127.0.0.1:${server.port}',
    );
    unawaited(backend.run());
    for (var i = 0; i < 100 && !backend.connected.value; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(backend.connected.value, isTrue);

    final first = backend.transcribeAudio(requestId: 'r1', uploadId: 'a');
    final second = backend.transcribeAudio(requestId: 'r2', uploadId: 'b');
    expect((await first).text, 'heard a');
    expect((await second).text, 'heard b');
    expect((await first).durationMs.toInt(), 1200);

    // Dropped before the answer: it fails, rather than waiting forever.
    final lost = backend.transcribeAudio(requestId: 'r3', uploadId: 'never');
    await Future<void>.delayed(const Duration(milliseconds: 50));
    await sockets.single.close();
    await expectLater(lost, throwsA(isA<RequestFailure>()));

    await backend.close();
    await server.close(force: true);
  });
}
