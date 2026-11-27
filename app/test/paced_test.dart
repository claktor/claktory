// On a slow link (two bars of 4G), a big file comes in pieces sized to the link,
// answered as one read would be, with progress as each comes, and it
// carries on from where it got when the connection drops and comes
// back. Uploads go in pieces the same way.
import 'dart:math' as math;

import 'package:crypto/crypto.dart' show sha256;
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/paced.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

/// A file on the machine, read as factoryd's ReadFile reads it.
class _Disk {
  _Disk(int size, this.mime) : bytes = _bytes(size, 1);

  Uint8List bytes;
  final String mime;
  var modified = 1;

  /// Every request: its offset and limit.
  final asked = <(int, int)>[];

  /// Called before each read answers (to drop the connection, say).
  void Function(int read)? before;

  static Uint8List _bytes(int size, int seed) => Uint8List.fromList([
    for (var i = 0; i < size; i++) (i * 31 + seed) & 255,
  ]);

  String get sha => sha256.convert(bytes).toString();

  void change() {
    bytes = _bytes(bytes.length, modified + 7);
    modified++;
  }

  Future<FileContent> read(int offset, int maxBytes, String expected) async {
    before?.call(asked.length);
    asked.add((offset, maxBytes));
    return answer(offset, maxBytes, expected);
  }

  /// What one ReadFile answers.
  FileContent answer(int offset, int maxBytes, String expected) {
    final ranged =
        offset > 0 || mime == 'video/mp4' || mime == 'application/pdf';
    var limit = ranged
        ? 8 * 1024 * 1024
        : mime.startsWith('image/')
        ? 20000000
        : 2000000;
    if (maxBytes > 0) limit = math.min(limit, maxBytes);
    if (expected.isNotEmpty && expected != sha) {
      return FileContent(path: '/f', error: 'changed');
    }
    final start = math.min(offset, bytes.length);
    final data = bytes.sublist(start, math.min(bytes.length, start + limit));
    final truncated = offset > 0 || data.length < bytes.length;
    return FileContent(
      path: '/f',
      mime: mime,
      size: Int64(bytes.length),
      data: data,
      truncated: truncated,
      modifiedMs: Int64(modified),
      offset: Int64(offset),
      sha256: !truncated || ranged ? sha : '',
    );
  }
}

void _same(FileContent paced, FileContent one) {
  expect(paced.error, one.error);
  expect(paced.size, one.size);
  expect(paced.mime, one.mime);
  expect(paced.offset, one.offset);
  expect(paced.truncated, one.truncated);
  expect(paced.sha256, one.sha256);
  expect(paced.data.length, one.data.length);
  expect(listEquals(paced.data, one.data), isTrue);
}

Pace _small() => Pace(least: 1000, most: 50000, first: 1000);

void main() {
  test('the pace starts small, grows on a quick link, and keeps each '
      'piece to about its target on a slow one', () {
    final pace = Pace();
    expect(pace.next, 64 * 1024);
    // A quick link: 64 KB in 5 ms. It grows, twice till the link's speed
    // can be told, then at most four times a step, up to its most.
    pace.took(64 * 1024, const Duration(milliseconds: 5));
    expect(pace.next, 128 * 1024);
    for (var i = 0; i < 6; i++) {
      pace.took(pace.next, const Duration(milliseconds: 20));
    }
    expect(pace.next, 4 * 1024 * 1024);
    // Two bars of 4G, about 60 KB a second: pieces of about 1.5 s.
    final slow = Pace();
    for (var i = 0; i < 8; i++) {
      slow.took(slow.next, Duration(microseconds: slow.next * 1e6 ~/ 60000));
    }
    expect(slow.next, inInclusiveRange(80000, 100000));
    // Edge, 15 KB a second: never under its least.
    final edge = Pace();
    for (var i = 0; i < 8; i++) {
      edge.took(edge.next, Duration(microseconds: edge.next * 1e6 ~/ 15000));
    }
    expect(edge.next, 32 * 1024);
  });

  // A pace learned on a quick link kept whole-file pieces
  // on a slow one.
  test('a quick link gone slow: the next piece is the slow one\'s at once; '
      'a piece cut off shrinks it; a new connection starts it again', () {
    final pace = Pace();
    for (var i = 0; i < 8; i++) {
      pace.took(pace.next, const Duration(milliseconds: 20));
    }
    expect(pace.next, 4 * 1024 * 1024);
    // 687 KB at 64 KB a second: 10.7 s, far past its 1.5 s.
    pace.took(687 * 1024, const Duration(milliseconds: 10700));
    expect(pace.next, inInclusiveRange(90000, 100000));
    // Cut off: a quarter of it next, never under its least.
    pace.failed(400 * 1024);
    expect(pace.next, 32 * 1024);
    final quick = Pace();
    for (var i = 0; i < 8; i++) {
      quick.took(quick.next, const Duration(milliseconds: 20));
    }
    quick.failed(687 * 1024);
    expect(quick.next, 687 * 1024 ~/ 4);
    expect(quick.estimate(1000000), isNull);
    // Another connection, maybe another link: from its first size.
    quick
      ..took(512, const Duration(milliseconds: 40))
      ..relink();
    expect(quick.next, 64 * 1024);
    expect(quick.estimate(1000000), isNull);
  });

  test('a tiny read is the round trip: it leaves the pieces as they are, '
      'and how long a file takes counts it once (#379)', () {
    final pace = Pace();
    expect(pace.estimate(1000000), isNull, reason: 'nothing measured yet');
    // One byte in 100 ms: all latency.
    pace.took(1, const Duration(milliseconds: 100));
    expect(pace.next, 64 * 1024);
    expect(pace.estimate(1000000), isNull);
    // 64 KB in 100 ms more than the round trip: about 655 KB a second.
    pace.took(64 * 1024, const Duration(milliseconds: 200));
    final takes = pace.estimate(6400000)!;
    expect(takes.inMilliseconds, inInclusiveRange(9800, 9950));
    // On Wi-Fi a first piece is mostly round trip, and a big video still
    // isn't "slow".
    final wifi = Pace()..took(1, const Duration(milliseconds: 60));
    wifi.took(64 * 1024, const Duration(milliseconds: 70));
    expect(wifi.estimate(9400000)!, lessThan(const Duration(seconds: 5)));
  });

  for (final (what, mime, size, maxBytes, offset) in [
    ('a range of a video', 'video/mp4', 300000, 8 * 1024 * 1024, 0),
    ('a video from an offset', 'video/mp4', 300000, 8 * 1024 * 1024, 70000),
    ('an image whole', 'image/png', 120000, 0, 0),
    ('a PDF whole', 'application/pdf', 90000, 0, 0),
    ('a small text file', 'text/plain', 800, 0, 0),
    ('an image with a limit', 'image/jpeg', 120000, 50000, 0),
  ]) {
    test('$what comes in pieces, answered as one read', () async {
      final disk = _Disk(size, mime);
      final progress = <int>[];
      final paced = await pacedRead(
        range: disk.read,
        connected: ValueNotifier(true),
        pace: _small(),
        maxBytes: maxBytes,
        offset: offset,
        onProgress: (through, whole) {
          expect(whole, size);
          progress.add(through);
        },
      );
      _same(paced, disk.answer(offset, maxBytes, ''));
      if (size > 1000) expect(disk.asked.length, greaterThan(1));
      for (final (_, asked) in disk.asked) {
        expect(asked, lessThanOrEqualTo(50000));
      }
      // Each piece moves it on, to the end of what it reads.
      expect(progress, orderedEquals([...progress]..sort()));
      expect(progress.last, offset + paced.data.length);
    });
  }

  test('told to stop, a read answers what came so far (#379)', () async {
    final disk = _Disk(20000, 'video/mp4');
    final seen = <int>[];
    final paced = await pacedRead(
      range: disk.read,
      connected: ValueNotifier(true),
      pace: _small(),
      maxBytes: 8 * 1024 * 1024,
      carryOn: (through, size) {
        seen.add(through);
        return through < 5000;
      },
    );
    expect(paced.data.length, seen.last);
    expect(paced.data.length, lessThan(20000));
    expect(paced.truncated, isTrue);
    expect(paced.offset.toInt(), 0);
    expect(listEquals(paced.data, disk.bytes.sublist(0, seen.last)), isTrue);
    // Kept: what else the first piece said.
    expect(paced.sha256, disk.sha);
  });

  test('told not to go on past the first piece, a read answers it alone '
      '(#379)', () async {
    final disk = _Disk(20000, 'application/pdf');
    final paced = await pacedRead(
      range: disk.read,
      connected: ValueNotifier(true),
      pace: _small(),
      onFirst: (first) => false,
    );
    expect(disk.asked, hasLength(1));
    expect(paced.data.length, lessThan(20000));
    expect(paced.truncated, isTrue);
    // Going on, it reads the rest.
    final whole = await pacedRead(
      range: disk.read,
      connected: ValueNotifier(true),
      pace: _small(),
      onFirst: (first) => true,
    );
    expect(whole.data.length, 20000);
  });

  test('two pieces of different sizes tell the round trip from the speed, '
      'with no tiny read (#379)', () {
    // 100 ms away, 1 MB a second: 64 KB takes 165 ms, 256 KB 362 ms.
    final pace = Pace();
    pace.took(65536, const Duration(microseconds: 165536));
    expect(pace.estimate(1000000), isNull);
    pace.took(262144, const Duration(microseconds: 362144));
    final takes = pace.estimate(1000000)!;
    expect(takes.inMilliseconds, inInclusiveRange(1050, 1150));
  });

  test('on a slow link whose pieces stay one size, with nothing tiny, a '
      'piece that took half the target or more says how long the rest '
      'takes (Zucchero\'s row 341)', () {
    // 500 kbit/s, 150 ms away: 64 KB takes about 1.2 s, and the pieces
    // after stay well under twice that, so the round trip never shows.
    final pace = Pace();
    pace.took(65536, const Duration(milliseconds: 1199));
    final takes = pace.estimate(34000000)!;
    expect(takes.inSeconds, inInclusiveRange(500, 700));
    pace.took(pace.next, const Duration(milliseconds: 1600));
    expect(pace.estimate(34000000), isNotNull);
    // After a new connection, the same from its first piece.
    pace.relink();
    expect(pace.estimate(34000000), isNull);
    pace.took(65536, const Duration(milliseconds: 1199));
    expect(pace.estimate(34000000), isNotNull);
  });

  test('a short first piece on a quick link far away says nothing yet: '
      'mostly round trip, it would make the link look slow', () {
    // 300 ms away, 10 MB a second: 64 KB in 306 ms.
    final pace = Pace();
    pace.took(65536, const Duration(milliseconds: 306));
    expect(pace.estimate(34000000), isNull);
    // The next, bigger piece tells the round trip from the speed.
    expect(pace.next, 2 * 65536);
    pace.took(pace.next, const Duration(milliseconds: 313));
    expect(pace.estimate(34000000), isNotNull);
  });

  test('a long text file stops where one read would: its first 2 MB, '
      'without a hash', () async {
    final disk = _Disk(2100000, 'text/plain');
    final paced = await pacedRead(
      range: disk.read,
      connected: ValueNotifier(true),
      pace: Pace(first: 64 * 1024),
    );
    _same(paced, disk.answer(0, 0, ''));
    expect(paced.data.length, 2000000);
    expect(paced.sha256, isEmpty);
  });

  test('the connection drops part way: it waits, and carries on from where '
      'it got once it is back', () async {
    final disk = _Disk(20000, 'video/mp4');
    final connected = ValueNotifier(true);
    var dropped = false;
    disk.before = (read) {
      if (read == 3 && !dropped) {
        dropped = true;
        connected.value = false;
        Future<void>.delayed(
          const Duration(milliseconds: 50),
          () => connected.value = true,
        );
        throw const RequestFailure('Connection lost.');
      }
    };
    final paced = await pacedRead(
      range: disk.read,
      connected: connected,
      pace: _small(),
      maxBytes: 8 * 1024 * 1024,
    );
    _same(paced, disk.answer(0, 8 * 1024 * 1024, ''));
    // Nothing read twice: the piece that was lost, from its offset.
    final offsets = [for (final (offset, _) in disk.asked) offset];
    expect(offsets.toSet().length, offsets.length);
  });

  test('lost while the connection still says it is up (its socket closed a '
      'moment before the app hears so): it waits, and carries on once it '
      'is back (Sale\'s row 87)', () async {
    final disk = _Disk(20000, 'video/mp4');
    final connected = ValueNotifier(true);
    var lost = 0;
    disk.before = (read) {
      if (read == 3 && lost == 0) {
        lost++;
        // Says so a moment later, then back.
        Future<void>.delayed(const Duration(milliseconds: 30), () {
          connected.value = false;
          Future<void>.delayed(
            const Duration(milliseconds: 30),
            () => connected.value = true,
          );
        });
        throw const RequestFailure('Connection lost.');
      }
    };
    final paced = await pacedRead(
      range: disk.read,
      connected: connected,
      pace: _small(),
      maxBytes: 8 * 1024 * 1024,
    );
    _same(paced, disk.answer(0, 8 * 1024 * 1024, ''));
    expect(lost, 1);
  });

  test('lost again and again while it says it is up: it stops at its '
      'time, saying so', () async {
    final disk = _Disk(20000, 'video/mp4');
    disk.before = (read) {
      if (read >= 2) throw const RequestFailure('Connection lost.');
    };
    await expectLater(
      pacedRead(
        range: disk.read,
        connected: ValueNotifier(true),
        pace: _small(),
        maxBytes: 8 * 1024 * 1024,
        reconnectWithin: const Duration(milliseconds: 300),
      ),
      throwsA(
        isA<RequestFailure>().having(
          (failure) => failure.message,
          'message',
          lostWhileReading,
        ),
      ),
    );
  });

  test('till the link\'s speed can be told, a piece grows at most twice: '
      'on a slow link that let the first through quickly, the next is no '
      'long wait', () {
    // The first 64 KB in a burst, 200 ms; a 4x piece would take 4 s on
    // the link behind it.
    final pace = Pace();
    pace.took(65536, const Duration(milliseconds: 200));
    expect(pace.estimate(1000000), isNull);
    expect(pace.next, 2 * 65536);
    // Twice the size: the round trip shows, and it grows as before.
    pace.took(pace.next, const Duration(milliseconds: 2200));
    expect(pace.estimate(1000000), isNotNull);
  });

  test(
    "it doesn't come back in time: it says so, not just Connection lost",
    () async {
      final disk = _Disk(20000, 'video/mp4');
      final connected = ValueNotifier(true);
      disk.before = (read) {
        if (read == 2) {
          connected.value = false;
          throw const RequestFailure('Connection lost.');
        }
      };
      await expectLater(
        pacedRead(
          range: disk.read,
          connected: connected,
          pace: _small(),
          maxBytes: 8 * 1024 * 1024,
          reconnectWithin: const Duration(milliseconds: 50),
        ),
        throwsA(
          isA<RequestFailure>().having(
            (failure) => failure.message,
            'message',
            lostWhileReading,
          ),
        ),
      );
    },
  );

  test('a refusal while connected fails at once, as before', () async {
    final disk = _Disk(20000, 'video/mp4');
    disk.before = (read) {
      if (read == 2) throw const RequestFailure('Not allowed.');
    };
    await expectLater(
      pacedRead(
        range: disk.read,
        connected: ValueNotifier(true),
        pace: _small(),
      ),
      throwsA(isA<RequestFailure>()),
    );
  });

  test('the file changes while it comes: read again, as it is now', () async {
    final disk = _Disk(20000, 'image/png');
    disk.before = (read) {
      if (read == 3) disk.change();
    };
    final paced = await pacedRead(
      range: disk.read,
      connected: ValueNotifier(true),
      pace: _small(),
    );
    _same(paced, disk.answer(0, 0, ''));
  });

  test('a version named by the caller that changed answers "changed", as '
      'one read would', () async {
    final disk = _Disk(20000, 'video/mp4');
    final sha = disk.sha;
    disk.before = (read) {
      if (read == 2) disk.change();
    };
    final paced = await pacedRead(
      range: disk.read,
      connected: ValueNotifier(true),
      pace: _small(),
      maxBytes: 8 * 1024 * 1024,
      expectedSha256: sha,
    );
    expect(paced.error, 'changed');
  });

  test(
    'a backend from before reads from an offset: one read, as it was',
    () async {
      final disk = _Disk(20000, 'image/png');
      Future<FileContent> old(int offset, int maxBytes, String expected) =>
          disk.read(0, maxBytes, '');
      final paced = await pacedRead(
        range: old,
        connected: ValueNotifier(true),
        pace: _small(),
      );
      _same(paced, disk.answer(0, 0, ''));
    },
  );

  test('an upload goes in pieces of the pace, and stops where the backend '
      'says it has got to', () async {
    final pieces = <(int, int)>[];
    final data = List<int>.generate(10000, (i) => i & 255);
    final received = await pacedUpload(
      send: (offset, piece) async {
        pieces.add((offset, piece.length));
        return offset + piece.length;
      },
      pace: _small(),
      offset: 500,
      data: data,
    );
    expect(received, 10500);
    expect(pieces.first, (500, 1000));
    var at = 500;
    for (final (offset, length) in pieces) {
      expect(offset, at);
      at += length;
    }
    // The backend had more than was sent (an earlier try): it stops there.
    final early = await pacedUpload(
      send: (offset, piece) async => 9000,
      pace: _small(),
      offset: 0,
      data: data,
    );
    expect(early, 9000);
  });
}
