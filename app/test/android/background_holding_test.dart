import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:factory_app/backend.dart';
import 'package:factory_app/dictation.dart';
import 'package:factory_app/imports/handoff.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';

class _Sending extends RecordingBackend {
  final sendAck = Completer<void>();

  @override
  Future<void> sendMessage(
    String agentId,
    String text, {
    String clientId = '',
    List<Attachment> attachments = const [],
    bool voice = false,
    int checklistItem = 0,
    String checklistAgentId = '',
  }) => sendAck.future;
}

class _Dictation extends RecordingBackend {
  final asked = Completer<void>();
  final heard = Completer<AudioTranscribed>();

  @override
  Future<int> startDictationUpload(
    String uploadId, {
    required int size,
    String name = 'dictation.m4a',
    String mime = 'audio/mp4',
  }) async => 0;

  @override
  Future<void> finishDictationUpload(
    String uploadId, {
    String sha256 = '',
  }) async {}

  @override
  Future<AudioTranscribed> transcribeAudio({
    required String requestId,
    required String uploadId,
  }) {
    asked.complete();
    return heard.future;
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test(
    'a direct import holds through upload and final send, outside the outbox',
    () async {
      final backend = _Sending();
      addTearDown(backend.close);
      final handoff = ImportHandoff(backend, 'agent', [
        ImportArtifact('record.json', Uint8List.fromList([1, 2, 3])),
      ]);
      addTearDown(handoff.dispose);
      final done = handoff.run();
      expect(backend.holds.value, isTrue);
      // It may be reading or uploading, or waiting for the send's answer:
      // none of these asynchronous gaps releases the whole operation.
      await Future<void>.delayed(Duration.zero);
      expect(backend.holds.value, isTrue);
      expect(backend.outbox.sending, isFalse);
      backend.sendAck.complete();
      await done;
      expect(handoff.state, HandoffState.done);
      expect(backend.holds.value, isFalse);
    },
  );

  test(
    'an import refusal releases the hold without changing its failure',
    () async {
      final backend = _Sending();
      addTearDown(backend.close);
      final handoff = ImportHandoff(backend, 'agent', [
        ImportArtifact('record.json', Uint8List.fromList([1, 2, 3])),
      ]);
      addTearDown(handoff.dispose);
      final done = handoff.run();
      await Future<void>.delayed(Duration.zero);
      backend.sendAck.completeError(
        const RequestFailure('The send was refused.'),
      );
      await done;
      expect(handoff.state, HandoffState.failed);
      expect(handoff.failure, 'The send was refused.');
      expect(backend.holds.value, isFalse);
    },
  );

  test(
    'dictation holds from the local file read through the transcribed answer',
    () async {
      final backend = _Dictation();
      addTearDown(backend.close);
      final folder = Directory.systemTemp.createTempSync(
        'background_dictation',
      );
      addTearDown(() => folder.deleteSync(recursive: true));
      final file = File('${folder.path}/voice.m4a')
        ..writeAsBytesSync([1, 2, 3]);
      final done = transcribeOnClaktory(
        backend,
        Recording(path: file.path, duration: const Duration(seconds: 1)),
      );
      expect(backend.holds.value, isTrue);
      await backend.asked.future;
      expect(
        backend.holds.value,
        isTrue,
        reason: 'upload finished; words still pending',
      );
      expect(backend.outbox.sending, isFalse);
      backend.heard.complete(AudioTranscribed(text: 'My answer.'));
      expect(await done, 'My answer.');
      expect(backend.holds.value, isFalse);
    },
  );

  test(
    'a dictation file error releases its hold and preserves the error',
    () async {
      final backend = _Dictation();
      addTearDown(backend.close);
      final folder = Directory.systemTemp.createTempSync(
        'background_dictation',
      );
      addTearDown(() => folder.deleteSync(recursive: true));
      final done = transcribeOnClaktory(
        backend,
        Recording(
          path: '${folder.path}/missing.m4a',
          duration: const Duration(seconds: 1),
        ),
      );
      expect(backend.holds.value, isTrue);
      await expectLater(done, throwsA(isA<FileSystemException>()));
      expect(backend.holds.value, isFalse);
    },
  );
}
