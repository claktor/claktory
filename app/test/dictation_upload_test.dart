// Answering by voice (dictation.dart): a recording goes to the Claktory as
// an upload for no agent, in chunks, and comes back as the words said; the
// Claktory's errors in the person's words, and an older Claktory told to
// update.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/dictation.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/state.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

/// What the fake Claktory got, and how it answers.
class _Claktory {
  final received = <String, List<int>>{};
  final purposes = <UploadPurpose>[];
  final finished = <String>[];
  final cancelled = <String>[];
  final heard = <String>[];

  /// Answers every TranscribeAudio with this error, if set.
  String error = '';

  /// Refuses StartUpload as a Claktory from before dictation does.
  bool older = false;
}

Future<HttpServer> _serve(_Claktory claktory) async {
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  server.listen((request) async {
    final socket = await WebSocketTransformer.upgrade(request);
    socket.listen((frame) {
      final message = ClientMessage.fromBuffer(frame as List<int>);
      void reply(ServerMessage answer) =>
          socket.add((answer..replyTo = message.id).writeToBuffer());
      if (message.hasHello()) {
        return reply(ServerMessage(welcome: Welcome(serverVersion: 'test')));
      }
      if (message.hasStartUpload()) {
        final start = message.startUpload;
        if (claktory.older) {
          return reply(
            ServerMessage(error: Error(message: 'no agent with id ""')),
          );
        }
        claktory.purposes.add(start.purpose);
        claktory.received[start.uploadId] = [];
        return reply(
          ServerMessage(
            uploadProgress: UploadProgress(uploadId: start.uploadId),
          ),
        );
      }
      if (message.hasUploadChunk()) {
        final chunk = message.uploadChunk;
        final bytes = claktory.received[chunk.uploadId]!..addAll(chunk.data);
        return reply(
          ServerMessage(
            uploadProgress: UploadProgress(
              uploadId: chunk.uploadId,
              received: Int64(bytes.length),
            ),
          ),
        );
      }
      if (message.hasFinishUpload()) {
        claktory.finished.add(message.finishUpload.uploadId);
        return reply(
          ServerMessage(
            uploadedFile: UploadedFile(uploadId: message.finishUpload.uploadId),
          ),
        );
      }
      if (message.hasCancelUpload()) {
        claktory.cancelled.add(message.cancelUpload.uploadId);
      }
      reply(ServerMessage(ack: Ack()));
      if (message.hasTranscribeAudio()) {
        final asked = message.transcribeAudio;
        claktory.heard.add(asked.uploadId);
        socket.add(
          ServerMessage(
            audioTranscribed: AudioTranscribed(
              requestId: asked.requestId,
              text: claktory.error.isEmpty ? 'Yes, the second one.' : '',
              error: claktory.error,
            ),
          ).writeToBuffer(),
        );
      }
    });
  });
  return server;
}

void main() {
  late _Claktory claktory;
  late HttpServer server;
  late Backend backend;
  late Directory folder;

  setUp(() async {
    claktory = _Claktory();
    server = await _serve(claktory);
    backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      localPairing: () async => null,
      localEndpoint: 'ws://127.0.0.1:${server.port}',
    );
    unawaited(backend.run());
    for (var i = 0; i < 100 && !backend.connected.value; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(backend.connected.value, isTrue);
    folder = await Directory.systemTemp.createTemp('dictation');
  });

  tearDown(() async {
    await backend.close();
    await server.close(force: true);
    await folder.delete(recursive: true);
  });

  Future<Recording> recording(int size) async {
    final file = File('${folder.path}/said.m4a');
    await file.writeAsBytes(List.generate(size, (i) => i % 251));
    return Recording(path: file.path, duration: const Duration(seconds: 3));
  }

  test('a recording goes up in chunks, for no agent, and comes back as the '
      'words said', () async {
    // Over a chunk, so it takes two.
    final said = await recording(1024 * 1024 + 300);
    final words = await transcribeOnClaktory(backend, said);
    expect(words, 'Yes, the second one.');
    expect(claktory.purposes, [UploadPurpose.UPLOAD_PURPOSE_DICTATION]);
    final id = claktory.received.keys.single;
    expect(claktory.received[id], await File(said.path).readAsBytes());
    expect(claktory.finished, [id]);
    expect(claktory.heard, [id]);
  });

  test("the Claktory's errors, in the person's words", () async {
    claktory.error = 'too_long';
    await expectLater(
      transcribeOnClaktory(backend, await recording(10)),
      throwsA(
        isA<DictationFailure>().having(
          (failure) => failure.message,
          'message',
          'That was too long. Try a shorter answer.',
        ),
      ),
    );
  });

  test('silence: no words caught', () async {
    claktory.error = 'no_speech';
    await expectLater(
      transcribeOnClaktory(backend, await recording(10)),
      throwsA(
        isA<DictationFailure>().having(
          (failure) => failure.message,
          'message',
          "Didn't catch any words. Try again.",
        ),
      ),
    );
  });

  test('a Claktory from before dictation: update it', () async {
    claktory.older = true;
    await expectLater(
      transcribeOnClaktory(backend, await recording(10)),
      throwsA(
        isA<DictationFailure>().having(
          (failure) => failure.message,
          'message',
          contains('Update it to answer by voice'),
        ),
      ),
    );
    expect(claktory.heard, isEmpty);
  });
}
