// Images as previews made for the screen (#379): a phone asks for one and
// says it's a preview, with Show original beside it; the original comes in
// its place with nothing moving, by itself once zoomed past the preview's
// sharpness; and a Mac on a normal link gets the original as before.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/mobile/metrics.dart' show Touch;
import 'package:factory_app/viewers/image.dart' show decodeWithin;
import 'package:factory_app/viewers/preview_badge.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, snapshot;
import 'viewers_test.dart' show png, showImage;

const _shot = '/w/shot.png';

/// The original's size, as the backend says it.
const _originalSize = 6900000;

Future<void> _decoding(WidgetTester tester) async {
  for (var i = 0; i < 6; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 100)),
    );
    await tester.pump();
  }
  await pumpFrames(tester);
  await tester.pump();
}

/// An iPhone 16 with the screenshot pushed over Builder's conversation:
/// the original 800 × 400, its preview 400 × 200 (as wide as the screen).
Future<Harness> _phone(
  WidgetTester tester, {
  double textScale = 1,
  Size size = const Size(393, 852),
}) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final original = await png(tester, 800, 400);
  final preview = await png(tester, 400, 200);
  final harness = Harness(snapshot(layout: beside('image', _shot)));
  harness.backend.files[_shot] = FileContent(
    path: _shot,
    mime: 'image/png',
    size: Int64(original.length),
    data: original,
    sha256: 'v1',
  );
  harness.backend.previews[_shot] = FileContent(
    path: _shot,
    mime: 'image/webp',
    size: Int64(preview.length),
    data: preview,
    sha256: 'v1',
    preview: FilePreview.FILE_PREVIEW_MADE,
    pixelWidth: 400,
    pixelHeight: 200,
    originalSize: Int64(_originalSize),
    originalPixelWidth: 800,
    originalPixelHeight: 400,
  );
  tester.platformDispatcher.textScaleFactorTestValue = textScale;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  await tester.tap(find.bySemanticsLabel('Extensions'));
  await pumpFrames(tester);
  await tester.tap(find.text('shot.png').last);
  await _decoding(tester);
  return harness;
}

Iterable<ReadFile> _reads(Harness harness) =>
    harness.backend.sent.where((m) => m.hasReadFile()).map((m) => m.readFile);

Rect _picture(WidgetTester tester) => tester.getRect(find.byType(RawImage));

void main() {
  testWidgets('a phone asks for a preview for its screen and says it is one, '
      "in the original's box", (tester) async {
    final harness = await _phone(tester);
    final asked = _reads(harness).where((r) => r.path == _shot).toList();
    expect(asked, isNotEmpty);
    expect(asked.first.previewWidth > 0, isTrue);
    // Portrait: the short side first.
    expect(
      asked.first.previewWidth,
      lessThanOrEqualTo(asked.first.previewHeight),
    );
    expect(find.text('Preview'), findsOneWidget);
    expect(find.text('Show original · 6.9 MB'), findsOneWidget);
    expect(find.bySemanticsLabel('Show original, 6.9 MB'), findsOneWidget);
    // The original's pixels and size, not the preview's.
    expect(find.textContaining('800 × 400'), findsWidgets);
    final shown = _picture(tester);
    expect(shown.width / shown.height, closeTo(2, 0.01));
    // A finger's reach.
    expect(
      tester.getSize(find.bySemanticsLabel('Show original, 6.9 MB')).height,
      greaterThanOrEqualTo(44),
    );
  });

  testWidgets('Show original loads it in place, with how much has come, and '
      'nothing moves', (tester) async {
    final harness = await _phone(tester);
    final before = _picture(tester);
    harness.backend.sent.clear();
    await tester.tap(find.text('Show original · 6.9 MB'));
    await _decoding(tester);
    final asked = _reads(harness).where((r) => r.path == _shot).toList();
    expect(asked, isNotEmpty);
    expect(asked.every((r) => r.previewWidth == 0), isTrue);
    expect(find.byType(PreviewBadge), findsNothing);
    expect(_picture(tester), before);
  });

  testWidgets('the badge shows the original on its way', (tester) async {
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Center(
          child: PreviewBadge(
            originalSize: _originalSize,
            load: const OriginalLoading(1200000, _originalSize),
            onShowOriginal: () {},
          ),
        ),
      ),
    );
    expect(find.text('Loading original… 1.2 MB of 6.9 MB'), findsOneWidget);
  });

  testWidgets('zooming in past the preview loads the original by itself', (
    tester,
  ) async {
    final harness = await _phone(tester);
    harness.backend.sent.clear();
    // A double tap: 100% of the original, two screen pixels to each of the
    // preview's.
    final at = _picture(tester).center;
    await tester.tapAt(at);
    await tester.pump(const Duration(milliseconds: 60));
    await tester.tapAt(at);
    await _decoding(tester);
    expect(
      _reads(harness).where((r) => r.path == _shot && r.previewWidth == 0),
      isNotEmpty,
    );
    expect(find.byType(PreviewBadge), findsNothing);
  });

  testWidgets("an original that can't come leaves the preview, and says so", (
    tester,
  ) async {
    final harness = await _phone(tester);
    harness.backend.files.remove(_shot);
    await tester.tap(find.text('Show original · 6.9 MB'));
    await _decoding(tester);
    expect(find.text("Couldn't load the original."), findsOneWidget);
    expect(find.text('Try again'), findsOneWidget);
    expect(find.byType(RawImage), findsOneWidget);
  });

  testWidgets('an original too big to open whole leaves the preview', (
    tester,
  ) async {
    final harness = await _phone(tester);
    harness.backend.files[_shot] = harness.backend.files[_shot]!.deepCopy()
      ..size = Int64(30000000)
      ..truncated = true;
    await tester.tap(find.text('Show original · 6.9 MB'));
    await _decoding(tester);
    expect(find.text("Couldn't load the original."), findsOneWidget);
    expect(find.byType(RawImage), findsOneWidget);
    expect(find.textContaining('too big'), findsNothing);
  });

  testWidgets('the badge fits a phone at the largest text', (tester) async {
    await _phone(tester, textScale: 3.12);
    expect(tester.takeException(), isNull);
    final badge = tester.getRect(find.byType(PreviewBadge));
    expect(badge.right, lessThanOrEqualTo(393));
  });

  // Zucchero's #379 row: at AX5 on an SE the badge took most of the
  // picture. It grows with the text only so far.
  testWidgets('at the largest text on a small iPhone, the badge grows no '
      'more than at 1.5x and leaves the picture to see', (tester) async {
    const se = Size(320, 568);
    await _phone(tester, textScale: maxPreviewBadgeTextScale, size: se);
    final capped = tester.getSize(find.byType(PreviewBadge));
    await tester.pumpWidget(const SizedBox());
    await _phone(tester, textScale: 3.12, size: se);
    expect(tester.takeException(), isNull);
    final badge = tester.getRect(find.byType(PreviewBadge));
    expect(badge.size, capped);
    expect(badge.right, lessThanOrEqualTo(se.width));
    // Two rows at most, a finger's each: the picture's room stays the
    // picture's.
    expect(badge.height, lessThanOrEqualTo(2 * Touch.target));
  });

  testWidgets('a Mac on a normal link gets the original', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final data = await png(tester, 400, 200);
    final harness = await showImage(
      tester,
      FileContent(
        path: _shot,
        mime: 'image/png',
        size: Int64(data.length),
        data: data,
        sha256: 'v1',
      ),
    );
    expect(_reads(harness).where((r) => r.previewWidth > 0), isEmpty);
    expect(find.byType(PreviewBadge), findsNothing);
  });

  testWidgets('a Mac on a slow link stops the original and shows a preview', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final preview = await png(tester, 400, 200);
    final big = List<int>.filled(900000, 0);
    final harness = Harness(snapshot(layout: beside('image', _shot)));
    harness.backend
      ..linkTime = ((bytes) => Duration(milliseconds: bytes ~/ 120))
      ..files[_shot] = FileContent(
        path: _shot,
        mime: 'image/png',
        size: Int64(big.length),
        data: big,
        sha256: 'v1',
      )
      ..previews[_shot] = FileContent(
        path: _shot,
        mime: 'image/webp',
        size: Int64(preview.length),
        data: preview,
        sha256: 'v1',
        preview: FilePreview.FILE_PREVIEW_MADE,
        pixelWidth: 400,
        pixelHeight: 200,
        originalSize: Int64(big.length),
        originalPixelWidth: 800,
        originalPixelHeight: 400,
      );
    await harness.show(tester);
    await _decoding(tester);
    final reads = _reads(harness).where((r) => r.path == _shot).toList();
    expect(reads.first.previewWidth, 0);
    expect(reads.last.previewWidth, greaterThan(0));
    expect(find.text('Show original · 900 KB'), findsOneWidget);
  });

  testWidgets('an image too big to open whole comes as a preview, even on a '
      'Mac on a quick link, without its first 20 MB first', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final preview = await png(tester, 400, 200);
    final huge = Uint8List(36400000);
    final harness = Harness(snapshot(layout: beside('image', _shot)));
    harness.backend
      ..linkTime = ((bytes) => Duration(microseconds: bytes ~/ 50))
      ..files[_shot] = FileContent(
        path: _shot,
        mime: 'image/png',
        size: Int64(huge.length),
        data: huge,
        sha256: 'v1',
      )
      ..previews[_shot] = FileContent(
        path: _shot,
        mime: 'image/webp',
        size: Int64(preview.length),
        data: preview,
        sha256: 'v1',
        preview: FilePreview.FILE_PREVIEW_MADE,
        pixelWidth: 400,
        pixelHeight: 200,
        originalSize: Int64(huge.length),
        originalPixelWidth: 8000,
        originalPixelHeight: 4000,
      );
    await harness.show(tester);
    await _decoding(tester);
    expect(find.text('Show original · 36.4 MB'), findsOneWidget);
    expect(find.textContaining('too big'), findsNothing);
  });

  test('an image taller than the GPU allows is decoded within it in '
      'proportion, never squashed', () {
    // The iPhone's decoder capped only its height: 2880 × 15308 as
    // 2880 × 8192.
    expect(
      decodeWithin(const Size(2880, 15308), const Size(2880, 8192)),
      const Size(1541, 8192),
    );
    // And a too-wide one.
    expect(
      decodeWithin(const Size(20000, 1000), const Size(8192, 1000)),
      const Size(8192, 410),
    );
    // Whole, or scaled evenly: as it came.
    expect(
      decodeWithin(const Size(2880, 15308), const Size(2880, 15308)),
      isNull,
    );
    expect(
      decodeWithin(const Size(2880, 15308), const Size(1440, 7654)),
      isNull,
    );
    expect(decodeWithin(const Size(1001, 333), const Size(500, 166)), isNull);
  });
}
