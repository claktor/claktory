// A video's preview coming on a slow link when the connection drops
// (Android, 512:2048): back, it carried on from 0, a minute lost on that
// link, since the preview came in one read of up to 8 MB and nothing of a
// read given up is kept. It comes a megabyte at a time now, each written as
// it comes, and back it carries on from the last one.
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

const _path = '/w/clip-b.mp4';
const _size = 30 * 1024 * 1024;
const _previewSize = 4400000;
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

/// Until the preview plays, with Show original beside it: the disk takes
/// its time on a loaded machine.
Future<void> _previewPlaying(WidgetTester tester) async {
  for (var i = 0; i < 30; i++) {
    await _loading(tester);
    if (find.textContaining('Show original').evaluate().isNotEmpty) return;
  }
}

void main() {
  setUp(() {
    FakePlayback.made.clear();
    newVideoPlayback = FakePlayback.new;
  });

  testWidgets('a preview cut off by a dropped connection carries on from '
      'where it got once it is back', (tester) async {
    final harness = Harness(snapshot(layout: beside('video', _path)));
    var dropped = false;
    harness.backend
      ..rangedReads = true
      // A slow link: the preview, not the original.
      ..linkTime = ((bytes) => Duration(microseconds: bytes * 1000000 ~/ 64000))
      ..files[_path] = FileContent(
        path: _path,
        mime: 'video/mp4',
        size: Int64(_size),
        data: List<int>.filled(_size, 1),
        sha256: 'v1',
      )
      ..previews[_path] = FileContent(
        path: _path,
        mime: 'video/mp4',
        size: Int64(_previewSize),
        data: List<int>.filled(_previewSize, 2),
        sha256: 'v1',
        preview: FilePreview.FILE_PREVIEW_MADE,
        originalSize: Int64(_size),
      )
      // The connection goes as the preview's third megabyte comes.
      ..holdRead = (request) async {
        if (!dropped && request.previewWidth > 0 && request.offset >= 2 * _mb) {
          dropped = true;
          harness.backend.connected.value = false;
          throw const RequestFailure('The connection was lost.');
        }
      };
    await harness.show(tester);
    tester.view.physicalSize = const Size(2000, 900);
    await _loading(tester);
    expect(dropped, isTrue);
    final before = harness.backend.sent.length;

    // Back.
    harness.backend.connected.value = true;
    await _loading(tester);
    final after = [
      for (final m in harness.backend.sent.skip(before))
        if (m.hasReadFile()) m.readFile,
    ];
    // From where it got, the preview's: never its start again, nor the
    // original's.
    final pieces = [
      for (final r in after)
        if (r.maxBytes > 1) r,
    ];
    expect(pieces.every((r) => r.previewWidth > 0), isTrue);
    expect(pieces.first.offset.toInt(), 2 * _mb);
    expect(FakePlayback.made.last.opened, contains('-preview-'));
  });

  testWidgets('the original, given up on after a long drop, carries on '
      "from where it got once it's back, as Share does (Bruno 29790)", (
    tester,
  ) async {
    final harness = Harness(snapshot(layout: beside('video', _path)));
    var dropped = false;
    harness.backend
      ..rangedReads = true
      ..linkTime = ((bytes) => Duration(microseconds: bytes * 1000000 ~/ 64000))
      ..files[_path] = FileContent(
        path: _path,
        mime: 'video/mp4',
        size: Int64(_size),
        data: List<int>.filled(_size, 1),
        sha256: 'v1',
      )
      ..previews[_path] = FileContent(
        path: _path,
        mime: 'video/mp4',
        size: Int64(_previewSize),
        data: List<int>.filled(_previewSize, 2),
        sha256: 'v1',
        preview: FilePreview.FILE_PREVIEW_MADE,
        originalSize: Int64(_size),
      );
    await harness.show(tester);
    tester.view.physicalSize = const Size(2000, 900);
    await _previewPlaying(tester);
    expect(FakePlayback.made.last.opened, contains('-preview-'));

    // The original, till the connection goes at its eleventh megabyte and
    // doesn't come back in time.
    harness.backend.holdRead = (request) async {
      if (!dropped && request.previewWidth == 0 && request.offset >= 10 * _mb) {
        dropped = true;
        harness.backend.connected.value = false;
        throw const RequestFailure(lostWhileReading);
      }
    };
    await tester.tap(find.textContaining('Show original'));
    for (var i = 0; i < 30 && !dropped; i++) {
      await _loading(tester);
    }
    await _loading(tester);
    expect(dropped, isTrue);
    expect(find.text("Couldn't load the original."), findsOneWidget);
    final before = harness.backend.sent.length;

    // Back: on by itself, from where it got, with no Try again.
    harness.backend.connected.value = true;
    // (The disk takes its time on a loaded machine.)
    for (var i = 0; i < 30 && FakePlayback.made.length < 2; i++) {
      await _loading(tester);
    }
    final pieces = [
      for (final m in harness.backend.sent.skip(before))
        if (m.hasReadFile() && m.readFile.maxBytes > 1) m.readFile,
    ];
    expect(pieces, isNotEmpty);
    expect(pieces.every((r) => r.previewWidth == 0), isTrue);
    expect(pieces.first.offset.toInt(), 10 * _mb);
    expect(FakePlayback.made.last.opened, isNot(contains('-preview-')));
    expect(find.text("Couldn't load the original."), findsNothing);
  });

  testWidgets("an original that failed for another reason waits for Try "
      'again', (tester) async {
    final harness = Harness(snapshot(layout: beside('video', _path)));
    harness.backend
      ..rangedReads = true
      ..linkTime = ((bytes) => Duration(microseconds: bytes * 1000000 ~/ 64000))
      ..files[_path] = FileContent(
        path: _path,
        mime: 'video/mp4',
        size: Int64(_size),
        data: List<int>.filled(_size, 1),
        sha256: 'v1',
      )
      ..previews[_path] = FileContent(
        path: _path,
        mime: 'video/mp4',
        size: Int64(_previewSize),
        data: List<int>.filled(_previewSize, 2),
        sha256: 'v1',
        preview: FilePreview.FILE_PREVIEW_MADE,
        originalSize: Int64(_size),
      );
    await harness.show(tester);
    tester.view.physicalSize = const Size(2000, 900);
    await _previewPlaying(tester);
    harness.backend.holdRead = (request) async {
      if (request.previewWidth == 0 && request.offset >= 10 * _mb) {
        throw const RequestFailure('The disk is unreadable.');
      }
    };
    await tester.tap(find.textContaining('Show original'));
    await _loading(tester);
    expect(find.text("Couldn't load the original."), findsOneWidget);
    final before = harness.backend.sent.length;
    harness.backend.connected.value = false;
    await _loading(tester);
    harness.backend.connected.value = true;
    await _loading(tester);
    expect(
      harness.backend.sent.skip(before).where((m) => m.hasReadFile()),
      isEmpty,
    );
    expect(find.text("Couldn't load the original."), findsOneWidget);
  });

  testWidgets('the original takes over from its preview where it was', (
    tester,
  ) async {
    final harness = Harness(snapshot(layout: beside('video', _path)));
    harness.backend
      ..rangedReads = true
      ..linkTime = ((bytes) => Duration(microseconds: bytes * 1000000 ~/ 64000))
      ..files[_path] = FileContent(
        path: _path,
        mime: 'video/mp4',
        size: Int64(_size),
        data: List<int>.filled(_size, 1),
        sha256: 'v1',
      )
      ..previews[_path] = FileContent(
        path: _path,
        mime: 'video/mp4',
        size: Int64(_previewSize),
        data: List<int>.filled(_previewSize, 2),
        sha256: 'v1',
        preview: FilePreview.FILE_PREVIEW_MADE,
        originalSize: Int64(_size),
      );
    await harness.show(tester);
    tester.view.physicalSize = const Size(2000, 900);
    await _previewPlaying(tester);
    final preview = FakePlayback.made.single;
    await preview.seek(12);
    await tester.tap(find.textContaining('Show original'));
    for (var i = 0; i < 30 && FakePlayback.made.length < 2; i++) {
      await _loading(tester);
    }
    expect(FakePlayback.made.last.opened, isNot(contains('-preview-')));
    expect(FakePlayback.made.last.seeks, [12]);
  });
}
