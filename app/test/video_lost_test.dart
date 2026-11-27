// A video coming when its Claktory goes away (iPhone):
// it waited for the connection with no end, never giving up after the
// minute a read waits, and as the Claktory came back it said "Connection
// lost." with the file's path for the seconds the read took to get going
// again. A video comes a megabyte at a time, each one read in a piece on a
// quick link, and a read's first piece failed with its request. Now the
// viewer waits for the connection as a read in pieces does: it says so
// once the minute is up, and back, it carries on loading, never saying the
// connection was lost.
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/paced.dart' show lostWhileReading;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/video.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, snapshot;
import 'video_test.dart' show FakePlayback;

const _path = '/w/big5.mp4';
const _size = 30 * 1024 * 1024;
const _mb = 1024 * 1024;

Future<void> _loading(WidgetTester tester) async {
  for (var i = 0; i < 8; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 100)),
    );
    await tester.pump();
  }
  await settle(tester);
}

/// A harness showing [_path], whose Claktory goes away as its eleventh
/// megabyte is asked for; [dropped] says once it has.
({Harness harness, bool Function() dropped}) _dropping() {
  final harness = Harness(snapshot(layout: beside('video', _path)));
  var dropped = false;
  harness.backend
    ..rangedReads = true
    ..files[_path] = FileContent(
      path: _path,
      mime: 'video/mp4',
      size: Int64(_size),
      data: List<int>.filled(_size, 1),
      sha256: 'v1',
    )
    // As the backend fails a request in flight when the connection goes.
    ..holdRead = (request) async {
      if (!harness.backend.connected.value) {
        throw const RequestFailure("Can't reach your Claktory right now.");
      }
      if (!dropped && request.offset >= 10 * _mb) {
        dropped = true;
        harness.backend.connected.value = false;
        throw const RequestFailure('Connection lost.');
      }
    };
  return (harness: harness, dropped: () => dropped);
}

Future<void> _untilDropped(WidgetTester tester, bool Function() dropped) async {
  for (var i = 0; i < 30 && !dropped(); i++) {
    await _loading(tester);
  }
  await _loading(tester);
  expect(dropped(), isTrue);
}

/// What the reads asked for since [before], past one-byte checks.
List<ReadFile> _piecesSince(Harness harness, int before) => [
  for (final m in harness.backend.sent.skip(before))
    if (m.hasReadFile() && m.readFile.maxBytes > 1) m.readFile,
];

void main() {
  setUp(() {
    FakePlayback.made.clear();
    newVideoPlayback = FakePlayback.new;
  });

  testWidgets('a video whose Claktory stays away past the minute says so, '
      "with no path, and carries on from where it got once it's back", (
    tester,
  ) async {
    final (:harness, :dropped) = _dropping();
    await harness.show(tester);
    tester.view.physicalSize = const Size(2000, 900);
    await _untilDropped(tester, dropped);

    // Waiting for it, not failed.
    expect(find.textContaining('Waiting for the connection'), findsOneWidget);
    expect(find.text('Connection lost.'), findsNothing);
    expect(find.text(lostWhileReading), findsNothing);

    // Still waiting just before the minute is up.
    await tester.pump(const Duration(seconds: 55));
    expect(find.text(lostWhileReading), findsNothing);
    // Past it: it says so, with Try again and no path.
    await tester.pump(const Duration(seconds: 6));
    expect(find.text(lostWhileReading), findsOneWidget);
    expect(find.text('Try again'), findsOneWidget);
    expect(find.text(_path), findsNothing);
    expect(find.text('Connection lost.'), findsNothing);
    final before = harness.backend.sent.length;

    // Back: loading again at once, never "Connection lost.", and on from
    // the eleventh megabyte.
    harness.backend.connected.value = true;
    await tester.pump();
    expect(find.text(lostWhileReading), findsNothing);
    expect(find.text('Connection lost.'), findsNothing);
    expect(find.text(_path), findsNothing);
    expect(find.textContaining('Loading video'), findsOneWidget);
    for (var i = 0; i < 30 && FakePlayback.made.isEmpty; i++) {
      await _loading(tester);
      expect(find.text('Connection lost.'), findsNothing);
    }
    final pieces = _piecesSince(harness, before);
    expect(pieces, isNotEmpty);
    expect(pieces.first.offset.toInt(), 10 * _mb);
    expect(FakePlayback.made, hasLength(1));
  });

  testWidgets('back within the minute, a video carries on loading without '
      'a word of the connection lost', (tester) async {
    final (:harness, :dropped) = _dropping();
    await harness.show(tester);
    tester.view.physicalSize = const Size(2000, 900);
    await _untilDropped(tester, dropped);
    await tester.pump(const Duration(seconds: 30));
    expect(find.textContaining('Waiting for the connection'), findsOneWidget);
    final before = harness.backend.sent.length;

    harness.backend.connected.value = true;
    await tester.pump();
    expect(find.text('Connection lost.'), findsNothing);
    expect(find.text(_path), findsNothing);
    for (var i = 0; i < 30 && FakePlayback.made.isEmpty; i++) {
      await _loading(tester);
      expect(find.text('Connection lost.'), findsNothing);
    }
    expect(_piecesSince(harness, before).first.offset.toInt(), 10 * _mb);
    expect(FakePlayback.made, hasLength(1));
    // The minute from the drop goes by with nothing said.
    await tester.pump(const Duration(seconds: 40));
    expect(find.text(lostWhileReading), findsNothing);
  });

  testWidgets('Try again on a video given up on waits for the connection, '
      "then carries on from where it got", (tester) async {
    final (:harness, :dropped) = _dropping();
    await harness.show(tester);
    tester.view.physicalSize = const Size(2000, 900);
    await _untilDropped(tester, dropped);
    await tester.pump(const Duration(seconds: 61));
    expect(find.text(lostWhileReading), findsOneWidget);

    // Still away: waiting again, not failed.
    await tester.tap(find.text('Try again'));
    await _loading(tester);
    expect(find.text(lostWhileReading), findsNothing);
    expect(find.textContaining('Waiting for the connection'), findsOneWidget);
    expect(find.text("Can't reach your Claktory right now."), findsNothing);
    final before = harness.backend.sent.length;

    harness.backend.connected.value = true;
    for (var i = 0; i < 30 && FakePlayback.made.isEmpty; i++) {
      await _loading(tester);
    }
    expect(_piecesSince(harness, before).first.offset.toInt(), 10 * _mb);
    expect(FakePlayback.made, hasLength(1));
  });
}
