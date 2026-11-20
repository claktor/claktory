import 'dart:io';

import 'package:factory_app/attachments.dart';
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/outbox.dart';
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
}
