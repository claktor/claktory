import 'dart:async';
import 'dart:typed_data';

import 'package:factory_app/attachments.dart';
import 'package:factory_app/outbox.dart';

/// The outbox's backend, answering when the test says: each send waits
/// for its completer in [answers]; uploads are taken whole.
class FakeOutboxBackend implements OutboxBackend {
  /// One per send, in order.
  final answers = <Completer<void>>[];

  /// The client ids sent, in order.
  final ids = <String>[];

  /// What each send carried.
  final sent = <List<SentFile>>[];

  /// The bytes of each upload, by upload id.
  final uploads = <String, List<int>>{};

  /// Refuses chunks while set, as a dropped connection would.
  Object? refuseChunks;

  /// Chunks taken so far.
  var chunks = 0;

  @override
  Future<void> send(
    String agentId,
    String text,
    String clientId,
    List<SentFile> files,
  ) {
    ids.add(clientId);
    sent.add(files);
    final answer = Completer<void>();
    answers.add(answer);
    return answer.future;
  }

  @override
  Future<int> startUpload(
    String uploadId,
    String agentId,
    LocalFile file,
  ) async => (uploads[uploadId] ??= []).length;

  @override
  Future<int> uploadChunk(String uploadId, int offset, Uint8List data) async {
    if (refuseChunks case final refusal?) throw refusal;
    chunks++;
    final bytes = uploads[uploadId]!;
    if (offset != bytes.length) throw StateError('wrong offset');
    bytes.addAll(data);
    return bytes.length;
  }

  @override
  Future<SentFile> finishUpload(String uploadId) async => SentFile(
    path: '/scratch/$uploadId',
    name: 'file',
    size: uploads[uploadId]!.length,
  );

  @override
  Future<void> cancelUpload(String uploadId) async => uploads.remove(uploadId);
}
