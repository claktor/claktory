// #375 (Matt, two bars of 4G): a video read, and an upload, over a slow
// link. The app's pings wait behind whatever frame is on its way, so one
// big frame used to hold the pong past its time and the app closed the
// connection, failing the read with "Connection lost." Here a real Backend
// talks to a fake Claktory through a link throttled both ways: the file
// comes whole, with progress, on the one connection it started on.
import 'dart:async';
import 'dart:io';
import 'dart:math' as math;

import 'package:factory_app/backend.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

/// Bytes a second each way: two bars of 4G, about.
var _rate = 200 * 1024;

/// A link that passes [_rate] bytes a second each way, to [port].
Future<ServerSocket> _slowLink(int port) async {
  final link = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
  link.listen((client) async {
    final server = await Socket.connect(InternetAddress.loopbackIPv4, port);
    void pace(Stream<Uint8List> from, Socket to) {
      final queue = <int>[];
      var ended = false;
      // A connection that closes loses what was still on its way, as a
      // dropped tunnel does: a frame cut off never arrives.
      from.listen(
        queue.addAll,
        onDone: () {
          queue.clear();
          ended = true;
        },
        onError: (_) {},
      );
      Timer.periodic(const Duration(milliseconds: 50), (timer) {
        final n = math.min(_rate ~/ 20, queue.length);
        if (n > 0) {
          to.add(queue.sublist(0, n));
          queue.removeRange(0, n);
        }
        if (ended && queue.isEmpty) {
          timer.cancel();
          unawaited(to.close().catchError((_) {}));
        }
      });
    }

    pace(client, server);
    pace(server, client);
  });
  return link;
}

/// What the fake Claktory has, and what it heard.
class _Claktory {
  final video = Uint8List.fromList([
    for (var i = 0; i < 2600 * 1024; i++) (i * 2654435761) >> 11 & 255,
  ]);
  var welcomes = 0;
  final uploaded = <int>[];
}

Future<HttpServer> _serve(_Claktory claktory) async {
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  server.listen((request) async {
    final socket = await WebSocketTransformer.upgrade(
      request,
      // As factoryd: nothing compressed (a video wouldn't be anyway).
      compression: CompressionOptions.compressionOff,
    );
    socket.listen((frame) {
      final message = ClientMessage.fromBuffer(frame as List<int>);
      void reply(ServerMessage answer) =>
          socket.add((answer..replyTo = message.id).writeToBuffer());
      if (message.hasHello()) {
        claktory.welcomes++;
        return reply(ServerMessage(welcome: Welcome(serverVersion: 'test')));
      }
      if (message.hasReadFile()) {
        final read = message.readFile;
        final bytes = claktory.video;
        final offset = read.offset.toInt();
        var limit = 8 * 1024 * 1024;
        if (read.maxBytes > 0) limit = math.min(limit, read.maxBytes.toInt());
        final end = math.min(bytes.length, offset + limit);
        return reply(
          ServerMessage(
            fileContent: FileContent(
              path: read.path,
              mime: 'video/mp4',
              size: Int64(bytes.length),
              data: bytes.sublist(offset, end),
              truncated: offset > 0 || end < bytes.length,
              offset: read.offset,
              modifiedMs: Int64(1),
              sha256: 'abc',
            ),
          ),
        );
      }
      if (message.hasStartUpload()) {
        return reply(
          ServerMessage(
            uploadProgress: UploadProgress(
              uploadId: message.startUpload.uploadId,
              // What it has of it, from a try that was cut off.
              received: Int64(claktory.uploaded.length),
            ),
          ),
        );
      }
      if (message.hasUploadChunk()) {
        // Where it says, as factoryd keeps it: a piece that came after
        // all from a try that was given up on is written over.
        final chunk = message.uploadChunk;
        if (chunk.offset.toInt() <= claktory.uploaded.length) {
          claktory.uploaded
            ..length = chunk.offset.toInt()
            ..addAll(chunk.data);
        }
        return reply(
          ServerMessage(
            uploadProgress: UploadProgress(
              uploadId: message.uploadChunk.uploadId,
              received: Int64(claktory.uploaded.length),
            ),
          ),
        );
      }
      reply(ServerMessage(ack: Ack()));
    });
  });
  return server;
}

void main() {
  late _Claktory claktory;
  late HttpServer server;
  late ServerSocket link;
  late Backend backend;

  setUp(() async {
    _rate = 200 * 1024;
    claktory = _Claktory();
    server = await _serve(claktory);
    link = await _slowLink(server.port);
    backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      localPairing: () async => null,
      localEndpoint: 'ws://127.0.0.1:${link.port}',
    );
    unawaited(backend.run());
    for (var i = 0; i < 200 && !backend.connected.value; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    expect(backend.connected.value, isTrue);
  });

  tearDown(() async {
    await backend.close();
    await link.close();
    await server.close(force: true);
  });

  test('a 2.5 MB video over a slow link comes whole, with progress as it '
      'comes, on the connection it started on', () async {
    final progress = <int>[];
    final started = DateTime.now();
    final read = await backend.readFile(
      '/w/clip.mp4',
      maxBytes: 8 * 1024 * 1024,
      onProgress: (through, size) => progress.add(through),
    );
    expect(read.error, isEmpty);
    expect(read.data.length, claktory.video.length);
    expect(listEquals(read.data, claktory.video), isTrue);
    // Never closed under it: one Hello, still connected.
    expect(claktory.welcomes, 1);
    expect(backend.connected.value, isTrue);
    // Progress moved through it, often: within the first 2 seconds, and
    // never more than a few seconds apart.
    expect(progress.length, greaterThan(8));
    expect(progress.first, lessThan(claktory.video.length ~/ 4));
    expect(
      DateTime.now().difference(started),
      greaterThan(const Duration(seconds: 10)),
      reason: 'the link is as slow as it says',
    );
  }, timeout: const Timeout(Duration(seconds: 60)));

  test('a photo going up a slow uplink, a 1 MB chunk as the outbox sends it, '
      'goes on the same connection', () async {
    // Slower up than down: a 1 MB frame takes 13 s.
    _rate = 80 * 1024;
    final data = List<int>.generate(1024 * 1024, (i) => i * 7 & 255);
    await backend.startUpload('u1', 'agent', name: 'a.jpg', size: data.length);
    final received = await backend.uploadChunk('u1', 0, data);
    expect(received, data.length);
    expect(claktory.uploaded.length, data.length);
    expect(claktory.welcomes, 1);
    expect(backend.connected.value, isTrue);
  }, timeout: const Timeout(Duration(seconds: 60)));

  // Sale's row 68 (Matt's weak 4G): a voice message went up in one piece
  // on a quick link; on a slow one the pace learned there sent it whole
  // again, the connection dropped under it (its pings held up), and every
  // try sent the same piece, cut off again: "Sending…" for good. A piece
  // cut off shrinks the next, and a new connection starts small.
  test('after a quick link, a slow one: the upload still goes, in smaller '
      'pieces, as the outbox tries again', () async {
    _rate = 20 * 1024 * 1024;
    final quick = List<int>.generate(3 * 1024 * 1024, (i) => i * 3 & 255);
    await backend.startUpload('u0', 'agent', name: 'a.m4a', size: quick.length);
    expect(await backend.uploadChunk('u0', 0, quick), quick.length);
    claktory.uploaded.clear();

    // Slower than a 10 s frame: the 687 KB of a 2:40 voice message.
    _rate = 48 * 1024;
    final voice = List<int>.generate(687 * 1024, (i) => i * 5 & 255);
    var at = 0;
    var tries = 0;
    final started = DateTime.now();
    // As the outbox does: from where the backend says it got, once
    // connected again.
    while (at < voice.length) {
      tries++;
      expect(tries, lessThan(6), reason: 'the same piece, cut off again');
      for (var i = 0; i < 1000 && !backend.connected.value; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      try {
        at = await backend.startUpload(
          'u1',
          'agent',
          name: 'b.m4a',
          size: voice.length,
        );
        at = await backend.uploadChunk('u1', at, voice.sublist(at));
      } on RequestFailure {
        continue;
      }
    }
    expect(claktory.uploaded.length, voice.length);
    expect(listEquals(claktory.uploaded, voice), isTrue);
    expect(
      DateTime.now().difference(started),
      lessThan(const Duration(seconds: 45)),
    );
  }, timeout: const Timeout(Duration(seconds: 90)));
}
