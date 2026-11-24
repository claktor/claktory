// Videos on a slow link (#379): once the original's first pieces show it
// would take too long, a preview made for this screen comes instead, and
// says it's one; Show original brings the original while the preview
// plays, and on a quick link nothing changes.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/preview_badge.dart';
import 'package:factory_app/viewers/video.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, snapshot;
import 'video_test.dart' show FakePlayback;

const _path = '/w/render.mp4';
const _size = 3300000;
const _previewSize = 870000;

Future<void> _loading(WidgetTester tester) async {
  for (var i = 0; i < 8; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 100)),
    );
    await tester.pump();
  }
  await settle(tester);
}

/// The render beside the conversation, on a link [bytesPerSecond] fast.
Future<Harness> _show(
  WidgetTester tester, {
  required int bytesPerSecond,
  FilePreview made = FilePreview.FILE_PREVIEW_MADE,
  ({int width, int height})? original,
}) async {
  final harness = Harness(snapshot(layout: beside('video', _path)));
  harness.backend
    ..rangedReads = true
    ..linkTime = ((bytes) =>
        Duration(microseconds: bytes * 1000000 ~/ bytesPerSecond))
    ..files[_path] = FileContent(
      path: _path,
      mime: 'video/mp4',
      size: Int64(_size),
      data: List<int>.filled(_size, 1),
      sha256: 'v1',
    )
    ..previews[_path] = made == FilePreview.FILE_PREVIEW_MADE
        ? FileContent(
            path: _path,
            mime: 'video/mp4',
            size: Int64(_previewSize),
            data: List<int>.filled(_previewSize, 2),
            sha256: 'v1',
            preview: made,
            originalSize: Int64(_size),
            originalPixelWidth: original?.width,
            originalPixelHeight: original?.height,
          )
        : FileContent(
            path: _path,
            mime: 'video/mp4',
            size: Int64(_size),
            data: List<int>.filled(_size, 1),
            sha256: 'v1',
            preview: made,
          );
  await harness.show(tester);
  tester.view.physicalSize = const Size(2000, 900);
  await _loading(tester);
  return harness;
}

Iterable<ReadFile> _reads(Harness harness) =>
    harness.backend.sent.where((m) => m.hasReadFile()).map((m) => m.readFile);

void main() {
  setUp(() {
    FakePlayback.made.clear();
    newVideoPlayback = FakePlayback.new;
  });

  testWidgets('on a slow link a preview plays, and says so', (tester) async {
    final harness = await _show(tester, bytesPerSecond: 120000);
    final reads = _reads(harness).toList();
    // The original's start, then the preview from its own start.
    expect(reads.first.previewWidth, 0);
    expect(reads.skip(1).every((r) => r.previewWidth > 0), isTrue);
    expect(reads.skip(1).first.offset.toInt(), 0);
    expect(FakePlayback.made.last.opened, contains('-preview-'));
    expect(find.text('Preview'), findsOneWidget);
    expect(find.text('Show original · 3.3 MB'), findsOneWidget);
    // The header gives the original's size.
    expect(find.textContaining('3.3 MB'), findsWidgets);
  });

  testWidgets('Show original brings it while the preview plays, and '
      'carries on from where it was', (tester) async {
    final harness = await _show(tester, bytesPerSecond: 120000);
    final preview = FakePlayback.made.last;
    await preview.seek(4);
    harness.backend.sent.clear();
    await tester.tap(find.text('Show original · 3.3 MB'));
    await _loading(tester);
    expect(_reads(harness), isNotEmpty);
    expect(_reads(harness).every((r) => r.previewWidth == 0), isTrue);
    final original = FakePlayback.made.last;
    expect(original, isNot(same(preview)));
    expect(original.opened, isNot(contains('-preview-')));
    expect(original.seeks, [4]);
    expect(find.byType(PreviewBadge), findsNothing);
  });

  testWidgets('on a quick link the original comes, as before', (tester) async {
    final harness = await _show(tester, bytesPerSecond: 50000000);
    expect(_reads(harness).every((r) => r.previewWidth == 0), isTrue);
    expect(find.byType(PreviewBadge), findsNothing);
    expect(FakePlayback.made.last.opened, isNot(contains('-preview-')));
  });

  testWidgets("a Claktory that can't make one sends the original, with no "
      'badge', (tester) async {
    await _show(
      tester,
      bytesPerSecond: 120000,
      made: FilePreview.FILE_PREVIEW_UNAVAILABLE,
    );
    expect(find.byType(PreviewBadge), findsNothing);
    expect(find.text('0:00 / 1:00'), findsOneWidget);
  });

  // Zucchero's #379 LOW: the header gave the preview's size beside the
  // original's bytes, as if it were the original's.
  testWidgets("while the preview plays, the header gives the original's "
      'size', (tester) async {
    await _show(
      tester,
      bytesPerSecond: 120000,
      original: (width: 3840, height: 2160),
    );
    expect(find.text('Preview'), findsOneWidget);
    expect(find.textContaining('3840 × 2160'), findsOneWidget);
    // The player's own (the fake's 1280 × 720, the preview's).
    expect(find.textContaining('1280 × 720'), findsNothing);
  });

  testWidgets("a preview from a Claktory that doesn't give the original's "
      'size shows none', (tester) async {
    await _show(tester, bytesPerSecond: 120000);
    expect(find.text('Preview'), findsOneWidget);
    expect(find.textContaining(' × '), findsNothing);
    expect(find.textContaining('3.3 MB'), findsWidgets);
  });

  testWidgets("the original gives the player's size", (tester) async {
    await _show(tester, bytesPerSecond: 50000000);
    expect(find.textContaining('1280 × 720'), findsOneWidget);
  });
}
