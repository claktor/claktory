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
import 'package:factory_app/voice_keep.dart';
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

  /// Answers this many TranscribeAudio with no_recording first, as a
  /// Claktory that restarted since does.
  var forgotten = 0;

  /// The TranscribeAudio request ids, in order.
  final requests = <String>[];

  /// StartUpload's upload ids, in order.
  final starts = <String>[];
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
        claktory.starts.add(start.uploadId);
        // Picks up what it has of the same upload.
        final has = claktory.received[start.uploadId] ??= [];
        return reply(
          ServerMessage(
            uploadProgress: UploadProgress(
              uploadId: start.uploadId,
              received: Int64(has.length),
            ),
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
        claktory.requests.add(asked.requestId);
        final error = claktory.forgotten > 0 ? 'no_recording' : claktory.error;
        if (claktory.forgotten > 0) claktory.forgotten--;
        socket.add(
          ServerMessage(
            audioTranscribed: AudioTranscribed(
              requestId: asked.requestId,
              text: error.isEmpty ? 'Yes, the second one.' : '',
              error: error,
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

  group('kept on the device (#401)', () {
    Future<KeptVoice> kept(int size) async {
      final said = await recording(size);
      return KeptVoice(
        path: said.path,
        duration: said.duration,
        claktory: 'claktory',
        kind: KeptKind.dictation,
        kept: false,
      );
    }

    test('it goes with its own ids, and picks up the upload where it '
        'stopped', () async {
      final voice = await kept(1024 * 1024 + 300);
      // Some of it got there before the connection dropped.
      claktory.received[voice.uploadId] = List.of(
        (await File(voice.path).readAsBytes()).sublist(0, 1000),
      );
      final words = await transcribeOnClaktory(backend, voice);
      expect(words, 'Yes, the second one.');
      expect(claktory.starts, [voice.uploadId]);
      expect(
        claktory.received[voice.uploadId],
        await File(voice.path).readAsBytes(),
      );
      expect(claktory.requests, [voice.requestId]);
      expect(voice.uploaded, isTrue);
    });

    test('once uploaded, asked again it only asks for the words, with the '
        'same request', () async {
      final voice = await kept(10);
      await transcribeOnClaktory(backend, voice);
      await transcribeOnClaktory(backend, voice);
      expect(claktory.starts, [voice.uploadId], reason: 'uploaded once');
      expect(claktory.requests, [voice.requestId, voice.requestId]);
    });

    test(
      'a Claktory that no longer has it gets it again, as a new upload',
      () async {
        final voice = await kept(10);
        final first = voice.uploadId;
        voice.uploaded = true;
        claktory.forgotten = 1;
        final words = await transcribeOnClaktory(backend, voice);
        expect(words, 'Yes, the second one.');
        expect(voice.uploadId, isNot(first));
        expect(claktory.starts, [voice.uploadId]);
        expect(claktory.requests, hasLength(2));
      },
    );

    test('a cut-off upload is left for its next try, not cancelled', () async {
      final voice = await kept(10);
      await backend.close();
      await expectLater(
        transcribeOnClaktory(backend, voice),
        throwsA(isA<RequestFailure>()),
      );
      expect(claktory.cancelled, isEmpty);
      expect(voice.uploaded, isFalse);
    });
  });
}
