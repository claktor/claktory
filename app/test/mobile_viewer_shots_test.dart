// How much of a phone's screen each viewer gives its content (Bruno's
// v0.1.22 task, Matt: a landscape video showed less video than portrait):
// a video, an image, a PDF and a text file, pushed over the conversation
// on an iPhone 16 in portrait and in landscape, measured, and with SHOTS
// set, drawn to PNGs there:
//   SHOTS=/tmp/viewers flutter test test/mobile_viewer_shots_test.dart
// Each prints "<kind> <orientation>: content <n>% of the screen".
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/mobile/viewer_chrome.dart';
import 'package:factory_app/pane_chrome.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/video.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, snapshot;
import 'phone_sizes_test.dart' show loadGeist;
import 'video_test.dart' show FakePlayback;
import 'viewers_test.dart' show png;

const videoFrame = Key('frame');

/// A player whose frame shows where the video is.
class CardPlayback extends FakePlayback {
  @override
  Widget view() => const DecoratedBox(
    key: videoFrame,
    decoration: BoxDecoration(
      gradient: LinearGradient(
        colors: [Color(0xFF1E5AA8), Color(0xFF3FA34D), Color(0xFFE0A030)],
      ),
    ),
  );
}

enum ViewerKind { video, image, pdf, text }

const phonePortrait = (
  size: Size(393, 852),
  padding: FakeViewPadding(top: 59, bottom: 34),
);
const phoneLandscape = (
  size: Size(852, 393),
  padding: FakeViewPadding(left: 59, right: 59, bottom: 21),
);

String _path(ViewerKind kind) => switch (kind) {
  ViewerKind.video => '/w/river.mp4',
  ViewerKind.image => '/w/shot.png',
  ViewerKind.pdf => '/w/paper.pdf',
  ViewerKind.text => '/w/notes.md',
};

Future<FileContent> _file(WidgetTester tester, ViewerKind kind) async {
  final path = _path(kind);
  switch (kind) {
    case ViewerKind.video:
      return FileContent(
        path: path,
        mime: 'video/mp4',
        size: Int64(400000),
        data: List<int>.filled(400000, 1),
        sha256: 'v1',
      );
    case ViewerKind.image:
      final data = await png(tester, 1600, 900);
      return FileContent(
        path: path,
        mime: 'image/png',
        size: Int64(data.length),
        data: data,
        sha256: 'v1',
      );
    case ViewerKind.pdf:
      final data = '%PDF-1.7\na paper about lapwings'.codeUnits;
      return FileContent(
        path: path,
        mime: 'application/pdf',
        size: Int64(data.length),
        data: data,
        sha256: 'v1',
      );
    case ViewerKind.text:
      final text = [
        '# Release notes',
        '',
        for (var i = 1; i <= 60; i++)
          '- Line $i: what changed, and what it means for you.',
      ].join('\n');
      return FileContent(
        path: path,
        mime: 'text/markdown',
        size: Int64(text.length),
        data: text.codeUnits,
        sha256: 'v1',
      );
  }
}

String _viewKind(ViewerKind kind) => switch (kind) {
  ViewerKind.video => 'video',
  ViewerKind.image => 'image',
  ViewerKind.pdf => 'pdf',
  ViewerKind.text => 'text',
};

Future<Harness> openOnPhone(
  WidgetTester tester,
  ViewerKind kind,
  ({Size size, FakeViewPadding padding}) screen,
) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final path = _path(kind);
  final harness = Harness(snapshot(layout: beside(_viewKind(kind), path)));
  harness.backend.files[path] = await _file(tester, kind);
  tester.view.padding = screen.padding;
  addTearDown(tester.view.resetPadding);
  await harness.show(tester, size: screen.size);
  await pumpFrames(tester);
  await tester.tap(find.bySemanticsLabel(RegExp('^More options for ')));
  await pumpFrames(tester);
  await tester.tap(find.text(path.split('/').last).last);
  for (var i = 0; i < 8; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 60)),
    );
    await tester.pump();
  }
  await pumpFrames(tester);
  return harness;
}

/// The content's rect: the video's frame, the picture, else the viewer's
/// body.
Rect contentRect(WidgetTester tester, ViewerKind kind) {
  switch (kind) {
    case ViewerKind.video:
      return tester.getRect(find.byKey(videoFrame));
    case ViewerKind.image:
      return tester.getRect(find.byType(RawImage).last);
    case ViewerKind.pdf:
    case ViewerKind.text:
      final body = tester.widget<PaneScaffold>(find.byType(PaneScaffold)).body;
      return tester.getRect(find.byWidget(body));
  }
}

/// What of [content] shows on screen and no chrome covers: a picture or
/// video's bars over it, while they show, a document's header above it.
double uncoveredArea(WidgetTester tester, Rect content, Size screen) {
  var shown = content.intersect(Offset.zero & screen);
  if (shown.isEmpty) return 0;
  bool visible(Finder bar) {
    if (bar.evaluate().isEmpty) return false;
    final opacity = tester.widgetList<AnimatedOpacity>(
      find.ancestor(of: bar, matching: find.byType(AnimatedOpacity)),
    );
    return opacity.every((o) => o.opacity > 0);
  }

  if (visible(find.byKey(viewerTopBar))) {
    final top = tester.getRect(find.byKey(viewerTopBar));
    shown = Rect.fromLTRB(
      shown.left,
      top.bottom.clamp(shown.top, shown.bottom),
      shown.right,
      shown.bottom,
    );
  }
  if (visible(find.byKey(viewerFootBar))) {
    final foot = tester.getRect(find.byKey(viewerFootBar));
    shown = Rect.fromLTRB(
      shown.left,
      shown.top,
      shown.right,
      foot.top.clamp(shown.top, shown.bottom),
    );
  }
  return shown.width * shown.height;
}

Future<void> _shot(WidgetTester tester, String name) async {
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

void main() {
  setUpAll(loadGeist);
  setUp(() {
    FakePlayback.made.clear();
    newVideoPlayback = CardPlayback.new;
  });

  for (final kind in ViewerKind.values) {
    for (final (name, screen) in [
      ('portrait', phonePortrait),
      ('landscape', phoneLandscape),
    ]) {
      testWidgets('${kind.name} in $name: what of the screen its content has', (
        tester,
      ) async {
        await openOnPhone(tester, kind, screen);
        final content = contentRect(tester, kind);
        final share =
            uncoveredArea(tester, content, screen.size) /
            (screen.size.width * screen.size.height);
        // ignore: avoid_print
        print(
          '${kind.name} $name: content ${(share * 100).round()}% of the '
          'screen (${content.size.width.round()}×${content.size.height.round()})',
        );
        await _shot(tester, '${kind.name}-$name');
        expect(share, greaterThan(0));
        if (kind == ViewerKind.video || kind == ViewerKind.image) {
          // A tap on it hides the chrome: all of it is the content's.
          await tester.tapAt(content.center + const Offset(0, 60));
          await tester.pump(const Duration(milliseconds: 400));
          await pumpFrames(tester);
          final hidden =
              uncoveredArea(tester, contentRect(tester, kind), screen.size) /
              (screen.size.width * screen.size.height);
          // ignore: avoid_print
          print(
            '${kind.name} $name, chrome hidden: content '
            '${(hidden * 100).round()}% of the screen',
          );
          await _shot(tester, '${kind.name}-$name-hidden');
          expect(hidden, greaterThanOrEqualTo(share));
        }
      });
    }
  }
}
