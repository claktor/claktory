// Pictures and videos in a Markdown file (#540, Matt via Bruno 41298): a
// picture beside the file, or at an absolute path, shows in place, read as
// the image viewer reads it, never wider than the text; a click opens it in
// its own pane, a right click offers Open and Share…; a video is a card
// with Play that opens the video viewer. Never fetched (Fugo, 41320): a
// picture on the web is a link; a path that isn't a picture or video isn't
// read; an SVG isn't read; a file named as a picture that isn't one is
// never kept on the device. One that can't show says so under its words.
import 'dart:convert';
import 'dart:io';

import 'package:factory_app/markdown.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/file_cache.dart';
import 'package:factory_app/viewers/image.dart' show ImageViewer;
import 'package:factory_app/viewers/markdown_media.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, snapshot, text;
import 'viewers_test.dart' show png;

const _doc = '/w/docs/notes.md';

/// Lets the decoder work, then the frames.
Future<void> _working(WidgetTester tester) async {
  for (var i = 0; i < 20; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 20)),
    );
    await tester.pump();
  }
}

FileContent _png(String path, List<int> bytes, {String sha = 'v1'}) =>
    FileContent(
      path: path,
      mime: 'image/png',
      size: Int64(bytes.length),
      data: bytes,
      sha256: sha,
    );

List<String> _readPaths(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasReadFile()) message.readFile.path,
];

Future<Harness> _show(
  WidgetTester tester,
  String markdown, {
  Map<String, FileContent> files = const {},
  Size size = const Size(1400, 900),
}) async {
  final harness = Harness(snapshot(layout: beside('text', _doc)));
  harness.backend.files[_doc] = text(_doc, markdown, mime: 'text/markdown');
  harness.backend.files.addAll(files);
  await harness.show(tester, size: size);
  await _working(tester);
  return harness;
}

void main() {
  group('markdownPath', () {
    test('beside the file, up from it, absolute and from home', () {
      expect(markdownPath(_doc, 'shot.png'), '/w/docs/shot.png');
      expect(markdownPath(_doc, './img/a.png'), '/w/docs/img/a.png');
      expect(markdownPath(_doc, '../pic.gif'), '/w/pic.gif');
      expect(markdownPath(_doc, '/tmp/x.png'), '/tmp/x.png');
      expect(markdownPath(_doc, '~/Pictures/x.png'), '~/Pictures/x.png');
      expect(markdownPath(_doc, 'my%20shot.png'), '/w/docs/my shot.png');
      expect(markdownPath(_doc, 'a.png#top'), '/w/docs/a.png');
      expect(markdownPath(_doc, 'file:///tmp/x.png'), '/tmp/x.png');
    });

    test('not a file here: the web, other schemes, past the root', () {
      expect(markdownPath(_doc, 'https://example.com/x.png'), isNull);
      expect(markdownPath(_doc, 'http://example.com/x.png'), isNull);
      expect(markdownPath(_doc, 'mailto:a@b.c'), isNull);
      expect(markdownPath(_doc, '../../../x.png'), isNull);
      expect(markdownPath(_doc, ''), isNull);
      expect(markdownPath(_doc, '#heading'), isNull);
    });

    test('only pictures and videos show in place, by extension', () {
      expect(markdownMediaKind('/a/b.PNG'), MarkdownMediaKind.picture);
      expect(markdownMediaKind('/a/b.jpeg'), MarkdownMediaKind.picture);
      expect(markdownMediaKind('/a/b.webp'), MarkdownMediaKind.picture);
      expect(markdownMediaKind('/a/b.gif'), MarkdownMediaKind.picture);
      expect(markdownMediaKind('/a/b.mov'), MarkdownMediaKind.video);
      expect(markdownMediaKind('/a/b.mp4'), MarkdownMediaKind.video);
      expect(markdownMediaKind('/a/b.svg'), isNull);
      expect(markdownMediaKind('/a/.ssh/id_ed25519'), isNull);
      expect(markdownMediaKind('/a/b.png.txt'), isNull);
    });
  });

  testWidgets('a picture beside the file shows in place, never wider than '
      'the text', (tester) async {
    final small = await png(tester, 120, 60);
    final wide = await png(tester, 3000, 1000);
    final harness = await _show(
      tester,
      '# Kit\n\n![A small shot](shot.png)\n\nText between.\n\n'
      '![A wide one](../wide.png)\n',
      files: {
        '/w/docs/shot.png': _png('/w/docs/shot.png', small),
        '/w/wide.png': _png('/w/wide.png', wide),
      },
    );
    expect(
      _readPaths(harness),
      containsAll(['/w/docs/shot.png', '/w/wide.png']),
    );
    final images = tester.widgetList<RawImage>(find.byType(RawImage)).toList();
    expect(images, hasLength(2));
    final sizes = [
      for (final element in find.byType(RawImage).evaluate())
        tester.getSize(find.byWidget(element.widget)),
    ];
    // Its own size when there's room.
    expect(sizes.first, const Size(120, 60));
    // The text's width at most, in proportion.
    final column = tester.getSize(find.byType(Markdown));
    expect(sizes.last.width, lessThanOrEqualTo(column.width));
    expect(sizes.last.width / sizes.last.height, closeTo(3, 0.01));
    // The words around it are still there.
    expect(find.text('Text between.'), findsOneWidget);
    expect(find.text('A small shot'), findsNothing);
  });

  testWidgets('Reload, or the file read again, reads its pictures again '
      "(Abbacchio's row 1447)", (tester) async {
    final before = await png(tester, 120, 60);
    final after = await png(tester, 80, 80);
    final harness = await _show(
      tester,
      '![A shot](shot.png)\n',
      files: {'/w/docs/shot.png': _png('/w/docs/shot.png', before)},
    );
    expect(tester.getSize(find.byType(RawImage)), const Size(120, 60));
    // An agent rewrites the picture, not the Markdown file.
    harness.backend.files['/w/docs/shot.png'] = _png(
      '/w/docs/shot.png',
      after,
      sha: 'v2',
    );
    await tester.tap(find.bySemanticsLabel('Reload'));
    await _working(tester);
    expect(tester.getSize(find.byType(RawImage)), const Size(80, 80));
  });

  testWidgets('a click opens the picture in its own pane', (tester) async {
    final bytes = await png(tester, 120, 60);
    final harness = await _show(
      tester,
      '![A shot](shot.png)\n',
      files: {'/w/docs/shot.png': _png('/w/docs/shot.png', bytes)},
    );
    await tester.tap(find.byType(RawImage));
    await tester.pump();
    final layouts = [
      for (final message in harness.backend.sent)
        if (message.hasSetLayout()) message.setLayout,
    ];
    expect(layouts, isNotEmpty);
    expect(
      layouts.last.root.toString(),
      allOf(contains('/w/docs/shot.png'), contains('image')),
    );
  });

  testWidgets('a right click offers Open, Download and Share…', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final bytes = await png(tester, 120, 60);
    await _show(
      tester,
      '![A shot](shot.png)\n',
      files: {'/w/docs/shot.png': _png('/w/docs/shot.png', bytes)},
    );
    await tester.tap(find.byType(RawImage), buttons: kSecondaryButton);
    await tester.pumpAndSettle();
    expect(find.text('Open'), findsOneWidget);
    expect(find.text('Download'), findsOneWidget);
    expect(find.text('Share…'), findsOneWidget);
  });

  testWidgets('a picture on the web is a link, never fetched', (tester) async {
    final harness = await _show(
      tester,
      'Before ![The logo](https://example.com/logo.png) after.\n',
    );
    expect(_readPaths(harness), [_doc]);
    expect(find.byType(RawImage), findsNothing);
    final link = find.byWidgetPredicate((widget) {
      if (widget is! RichText) return false;
      var linked = false;
      widget.text.visitChildren((span) {
        if (span is TextSpan &&
            span.text == 'The logo' &&
            span.recognizer is TapGestureRecognizer) {
          linked = true;
        }
        return true;
      });
      return linked;
    });
    expect(link, findsOneWidget);
  });

  testWidgets('a path to a file that is not a picture or video is a link, '
      'never read', (tester) async {
    final harness = await _show(
      tester,
      '![key](../../.ssh/id_ed25519) ![d](diagram.svg) ![t](notes.txt)\n',
      files: {
        '/w/docs/diagram.svg': text(
          '/w/docs/diagram.svg',
          '<svg xmlns="http://www.w3.org/2000/svg"><image '
              'href="https://example.com/beacon.png"/></svg>',
          mime: 'image/svg+xml',
        ),
      },
    );
    expect(_readPaths(harness), [_doc]);
    expect(find.byType(RawImage), findsNothing);
    expect(find.textContaining('key'), findsOneWidget);
  });

  testWidgets('a picture that is missing says so under its words', (
    tester,
  ) async {
    await _show(tester, '![A missing shot](gone.png)\n');
    expect(find.text('A missing shot'), findsOneWidget);
    expect(find.text("Can't show gone.png"), findsOneWidget);
  });

  testWidgets('a file named as a picture that is not one is not shown, and '
      'never kept on the device', (tester) async {
    final folder = Directory.systemTemp.createTempSync('md-kept');
    keepsFilesOf = (_) => true;
    fileCacheFolder = () async => folder.path;
    addTearDown(() {
      keepsFilesOf = (backend) => backend.target != null;
      FileCache.forgetAllInMemory();
      folder.deleteSync(recursive: true);
    });
    final secret = utf8.encode('-----BEGIN OPENSSH PRIVATE KEY-----\n');
    await _show(
      tester,
      '![x](key.png)\n',
      files: {
        '/w/docs/key.png': FileContent(
          path: '/w/docs/key.png',
          mime: 'image/png',
          size: Int64(secret.length),
          data: secret,
          sha256: 'k1',
        ),
      },
    );
    expect(find.text("Can't show key.png"), findsOneWidget);
    await _working(tester);
    final kept = [
      for (final entry in folder.listSync(recursive: true))
        if (entry is File &&
            !entry.path.endsWith('.json') &&
            !entry.path.endsWith('.part'))
          entry.path,
    ];
    // The Markdown file itself may be kept; the "picture" never.
    expect([
      for (final path in kept)
        if (File(path).readAsBytesSync().length == secret.length) path,
    ], isEmpty);
  });

  testWidgets('a picture written into the file shows; one too big is its '
      'words', (tester) async {
    final bytes = await png(tester, 40, 20);
    final small = 'data:image/png;base64,${base64.encode(bytes)}';
    final big =
        'data:image/png;base64,${'A' * (markdownDataLimit * 4 ~/ 3 + 4096)}';
    final svg =
        'data:image/svg+xml;base64,${base64.encode(utf8.encode('<svg/>'))}';
    await _show(
      tester,
      '![Inline]($small)\n\n![Too big]($big)\n\n![Vector]($svg)\n',
    );
    expect(find.byType(RawImage), findsOneWidget);
    expect(find.textContaining('Too big'), findsOneWidget);
    expect(find.textContaining('Vector'), findsOneWidget);
  });

  testWidgets('a video is a card with Play that opens the video viewer, '
      'with only its first bytes read', (tester) async {
    final movie = [
      0, 0, 0, 0x18, ...'ftypisom'.codeUnits, ...List<int>.filled(200, 0), //
    ];
    final harness = await _show(
      tester,
      '![The demo](demo.mp4)\n',
      files: {
        '/w/docs/demo.mp4': FileContent(
          path: '/w/docs/demo.mp4',
          mime: 'video/mp4',
          size: Int64(movie.length),
          data: movie,
          sha256: 'm1',
        ),
      },
    );
    final reads = [
      for (final message in harness.backend.sent)
        if (message.hasReadFile() && message.readFile.path.endsWith('.mp4'))
          message.readFile,
    ];
    expect(reads.map((r) => r.maxBytes.toInt()), [64]);
    expect(find.text('The demo'), findsOneWidget);
    await tester.tap(find.text('The demo'));
    await tester.pump();
    final layouts = [
      for (final message in harness.backend.sent)
        if (message.hasSetLayout()) message.setLayout,
    ];
    expect(
      layouts.last.root.toString(),
      allOf(contains('/w/docs/demo.mp4'), contains('video')),
    );
  });

  testWidgets('a missing video says so under its words', (tester) async {
    await _show(tester, '![The demo](demo.mov)\n');
    expect(find.text("Can't show demo.mov"), findsOneWidget);
  });

  testWidgets(
    'on an iPhone a picture fits the screen, a touch and hold '
    'offers Open and Share…, Open opens it over the document, and Back comes back to it',
    (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final bytes = await png(tester, 1200, 600);
      final harness = Harness(snapshot(layout: beside('text', _doc)));
      harness.backend.files[_doc] = text(
        _doc,
        '# Kit\n\n![The banner](banner.png)\n',
        mime: 'text/markdown',
      );
      harness.backend.files['/w/docs/banner.png'] = _png(
        '/w/docs/banner.png',
        bytes,
      );
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      await harness.show(tester, size: const Size(393, 852));
      await pumpFrames(tester);
      await tester.tap(find.bySemanticsLabel(RegExp('^More options for ')));
      await pumpFrames(tester);
      await tester.tap(find.text('notes.md').last);
      await pumpFrames(tester);
      await _working(tester);
      final picture = find.byType(RawImage);
      expect(picture, findsOneWidget);
      expect(tester.getSize(picture).width, lessThanOrEqualTo(393));
      expect(tester.getSize(picture).width, greaterThan(300));

      await tester.longPress(picture);
      await pumpFrames(tester);
      expect(find.text('Open'), findsOneWidget);
      expect(find.text('Download'), findsNothing);
      expect(find.text('Share…'), findsOneWidget);
      await tester.tap(find.text('Open'));
      await pumpFrames(tester);
      await _working(tester);
      // A page over the document, not a pane in its place.
      expect(find.byType(ImageViewer), findsOneWidget);
      expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
      // Back comes back to the document (Zucchero's row 601).
      await tester.tap(find.bySemanticsLabel('Back').last);
      await pumpFrames(tester);
      expect(find.byType(ImageViewer), findsNothing);
      expect(find.text('Kit'), findsOneWidget);
      expect(find.byType(RawImage), findsOneWidget);
    },
  );

  testWidgets('in a conversation, with no file to start from, a picture is '
      'its words as before, and nothing is read', (tester) async {
    final harness = Harness(snapshot());
    await harness.show(
      tester,
      home: const Markdown('Look: ![A shot](shot.png) here.'),
    );
    await tester.pump();
    expect(find.textContaining('A shot'), findsOneWidget);
    expect(_readPaths(harness), isEmpty);
  });
}
