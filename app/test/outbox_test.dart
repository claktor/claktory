import 'dart:io';

import 'package:factory_app/attachments.dart';
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/outbox.dart';
import 'package:factory_app/voice_keep.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

import 'outbox_fake.dart';

void main() {
  late Directory folder;
  late LocalFile big;
  late LocalFile small;

  LocalFile file(String name, int size) {
    final path = '${folder.path}/$name';
    File(path).writeAsBytesSync([for (var i = 0; i < size; i++) i % 251]);
    return LocalFile(path: path, name: name, size: size);
  }

  setUp(() {
    folder = Directory.systemTemp.createTempSync('outbox');
    big = file('big.bin', Outbox.chunk * 2 + 1000);
    small = file('notes.txt', 10);
  });

  tearDown(() => folder.deleteSync(recursive: true));

  /// Lets the outbox get on with it until [done]: it reads real files, which
  /// takes longer on a busy machine, so no fixed wait is enough.
  Future<void> until(bool Function() done) async {
    for (var i = 0; i < 500 && !done(); i++) {
      await Future<void>.delayed(const Duration(milliseconds: 10));
    }
    // Anything it does right after.
    await Future<void>.delayed(const Duration(milliseconds: 10));
  }

  test('files upload a chunk at a time, then go with the message', () async {
    final backend = FakeOutboxBackend();
    final outbox = Outbox(backend);
    final progress = <double>[];
    final message = outbox.send('a', 'here', files: [big, small]);
    outbox.addListener(() {
      if (message.files.first.done == null) {
        progress.add(message.files.first.progress);
      }
    });
    expect(outbox.of('a').single.uploading, isTrue);
    await until(() => backend.sent.isNotEmpty);
    expect(backend.chunks, 4, reason: 'three for the big one, one small');
    expect(progress.last, greaterThan(0.5));
    for (final file in [big, small]) {
      final id = message.files[file == big ? 0 : 1].uploadId;
      expect(backend.uploads[id], File(file.path).readAsBytesSync());
    }
    expect(backend.sent.single, hasLength(2));
    backend.answers.single.complete();
    await until(() => outbox.of('a').isEmpty);
    expect(outbox.of('a'), isEmpty);
  });

  test('a cut-off upload picks up where it stopped on Retry', () async {
    final backend = FakeOutboxBackend()
      ..refuseChunks = const RequestFailure('Not connected.');
    final outbox = Outbox(backend);
    final message = outbox.send('a', '', files: [big]);
    await until(() => message.failure != null);
    expect(message.failure, 'Not connected.');
    expect(backend.answers, isEmpty, reason: 'not sent without its file');

    // Some of it got there before the drop.
    final id = message.files.single.uploadId;
    backend
      ..uploads[id] = List.of(
        File(big.path).readAsBytesSync().sublist(0, Outbox.chunk),
      )
      ..refuseChunks = null;
    outbox.retry(message);
    await until(() => backend.answers.isNotEmpty);
    expect(backend.chunks, 2, reason: 'only what was missing');
    expect(backend.uploads[id], File(big.path).readAsBytesSync());
    expect(backend.answers, hasLength(1));
  });

  test('taking one back drops what the backend has of its files', () async {
    final backend = FakeOutboxBackend()
      ..refuseChunks = const RequestFailure('Not connected.');
    final outbox = Outbox(backend);
    final message = outbox.send('a', 'here', files: [big]);
    await until(() => message.failure != null);
    final id = message.files.single.uploadId;
    backend.uploads[id] = [1, 2, 3];
    message.files.single.sent = 3;
    outbox.takeBack(message);
    await until(() => !backend.uploads.containsKey(id));
    expect(outbox.of('a'), isEmpty);
    expect(backend.uploads, isNot(contains(id)));
  });

  test("a file that can't be read stops the send and says which", () async {
    final backend = FakeOutboxBackend();
    final outbox = Outbox(backend);
    File(small.path).deleteSync();
    final message = outbox.send('a', 'here', files: [small]);
    await until(() => message.failure != null);
    expect(message.failure, contains('notes.txt'));
    expect(backend.answers, isEmpty);
  });

  group('on a weak connection (#401)', () {
    late Directory kept;
    late ValueNotifier<bool> connected;

    setUp(() {
      kept = Directory.systemTemp.createTempSync('kept');
      final was = keptVoiceFolder;
      keptVoiceFolder = () async => kept.path;
      connected = ValueNotifier(true);
      addTearDown(() {
        keptVoiceFolder = was;
        kept.deleteSync(recursive: true);
      });
    });

    Outbox outboxFor(FakeOutboxBackend backend) => Outbox(
      backend,
      connected: connected,
      claktory: () => 'claktory',
      kind: KeptKind.agent,
    );

    /// A recording made in the kept folder, as the mic makes them.
    LocalFile recording(int size) {
      final dir = Directory('${kept.path}/123-abc')..createSync();
      final path = '${dir.path}/Voice.m4a';
      File(path).writeAsBytesSync([for (var i = 0; i < size; i++) i % 251]);
      return LocalFile(path: path, name: 'Voice.m4a', size: size);
    }

    test('a message cut off by the connection waits, then goes by itself '
        'once it is back, picking up its upload', () async {
      final backend = FakeOutboxBackend()
        ..refuseChunks = const RequestFailure('Connection lost.');
      final outbox = outboxFor(backend);
      final message = outbox.send('a', 'here', files: [big]);
      await until(() => message.failed);
      expect(message.waiting, isTrue);
      expect(message.failure, waitingForConnection);

      final id = message.files.single.uploadId;
      backend
        ..uploads[id] = List.of(
          File(big.path).readAsBytesSync().sublist(0, Outbox.chunk),
        )
        ..refuseChunks = null;
      connected.value = false;
      connected.value = true;
      await until(() => backend.answers.isNotEmpty);
      expect(message.waiting, isFalse);
      expect(backend.chunks, 2, reason: 'only what was missing');
      expect(backend.ids.single, message.clientId);
    });

    test('a text sent while not connected waits for the connection', () async {
      final backend = FakeOutboxBackend();
      connected.value = false;
      final outbox = outboxFor(backend);
      final message = outbox.send('a', 'hello');
      await until(() => message.failed);
      expect(message.waiting, isTrue);
      expect(backend.answers, isEmpty);
      connected.value = true;
      await until(() => backend.answers.isNotEmpty);
      expect(backend.ids.single, message.clientId);
    });

    test("a refusal isn't the connection's: it waits for Retry", () async {
      final backend = FakeOutboxBackend()
        ..refuseChunks = const RequestFailure('Too big.');
      final outbox = outboxFor(backend);
      final message = outbox.send('a', '', files: [big]);
      await until(() => message.failed);
      expect(message.waiting, isFalse);
      expect(message.failure, 'Too big.');
      backend.refuseChunks = null;
      connected.value = false;
      connected.value = true;
      await Future<void>.delayed(const Duration(milliseconds: 100));
      expect(backend.answers, isEmpty);
      expect(message.failure, 'Too big.');
    });

    test("a voice message's recording is kept beside what it's for until "
        'the Claktory has the message', () async {
      final backend = FakeOutboxBackend();
      final outbox = outboxFor(backend);
      final file = recording(300);
      final message = outbox.send(
        'a',
        '',
        files: [file],
        voice: const Duration(seconds: 4),
      );
      await until(() => backend.answers.isNotEmpty);
      final saved = KeptVoice.read(File(file.path).parent)!;
      expect(saved.kind, KeptKind.agent);
      expect(saved.target, 'a');
      expect(saved.claktory, 'claktory');
      expect(saved.clientId, message.clientId);
      expect(saved.uploadId, message.files.single.uploadId);
      expect(saved.duration, const Duration(seconds: 4));
      expect(backend.voices.single, isTrue);

      backend.answers.single.complete();
      await until(() => !File(file.path).existsSync());
      expect(File(file.path).parent.existsSync(), isFalse);
    });

    test('a kept voice message comes back and goes with its own ids, so the '
        'Claktory takes it once', () async {
      final backend = FakeOutboxBackend();
      final file = recording(300);
      final voice = KeptVoice(
        path: file.path,
        duration: const Duration(seconds: 4),
        claktory: 'claktory',
        kind: KeptKind.agent,
        target: 'a',
        clientId: 'client-1',
        uploadId: 'upload-1',
      );
      await voice.save();
      // Some of it got there before the app ended.
      backend.uploads['upload-1'] = List.of(
        File(file.path).readAsBytesSync().sublist(0, 100),
      );
      final outbox = outboxFor(backend);
      outbox.restore(voice);
      outbox.restore(voice);
      expect(outbox.of('a'), hasLength(1), reason: 'once, however often');
      await until(() => backend.answers.isNotEmpty);
      expect(backend.ids.single, 'client-1');
      expect(backend.voices.single, isTrue);
      expect(backend.uploads['upload-1'], File(file.path).readAsBytesSync());
      backend.answers.single.complete();
      await until(() => !voice.folder.existsSync());
      expect(voice.folder.existsSync(), isFalse);
    });

    test('a kept voice message waits for the connection', () async {
      final backend = FakeOutboxBackend();
      connected.value = false;
      final file = recording(30);
      final voice = KeptVoice(
        path: file.path,
        duration: const Duration(seconds: 1),
        claktory: 'claktory',
        kind: KeptKind.agent,
        target: 'a',
        clientId: 'client-1',
      );
      final outbox = outboxFor(backend)..restore(voice);
      final message = outbox.of('a').single;
      expect(message.waiting, isTrue);
      expect(backend.answers, isEmpty);
      connected.value = true;
      await until(() => backend.answers.isNotEmpty);
      expect(backend.ids.single, 'client-1');
    });

    test('deleting a voice message not sent deletes its recording and what '
        'the Claktory has of it', () async {
      final backend = FakeOutboxBackend()
        ..refuseChunks = const RequestFailure('Connection lost.');
      final outbox = outboxFor(backend);
      final file = recording(300);
      final message = outbox.send(
        'a',
        '',
        files: [file],
        voice: const Duration(seconds: 4),
      );
      await until(() => message.failed);
      expect(File(file.path).existsSync(), isTrue);
      final id = message.files.single.uploadId;
      outbox.takeBack(message);
      await until(() => !File(file.path).parent.existsSync());
      expect(File(file.path).parent.existsSync(), isFalse);
      expect(backend.uploads, isNot(contains(id)));
      expect(outbox.of('a'), isEmpty);
    });
  });
}
