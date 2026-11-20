import 'dart:convert';
import 'dart:ui' as ui;

import 'package:factory_app/controls.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/registry.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, column, leaf, row, showPanes, split, text;

/// A [width]×[height] PNG, half opaque.
Future<List<int>> png(WidgetTester tester, int width, int height) async {
  final bytes = await tester.runAsync(() async {
    final recorder = ui.PictureRecorder();
    ui.Canvas(recorder).drawRect(
      ui.Rect.fromLTWH(0, 0, width / 2, height.toDouble()),
      ui.Paint()..color = const ui.Color(0xFF2F6FEB),
    );
    final image = await recorder.endRecording().toImage(width, height);
    final data = await image.toByteData(format: ui.ImageByteFormat.png);
    return data!.buffer.asUint8List();
  });
  return bytes!;
}

FileContent image(String path, List<int> data) => FileContent(
  path: path,
  mime: 'image/png',
  size: Int64(data.length),
  data: data,
);

/// Shows [file] beside the conversation and waits for it to decode.
Future<Harness> showImage(WidgetTester tester, FileContent file) async {
  final harness = await showPanes(
    tester,
    beside('image', file.path),
    files: {file.path: file},
  );
  await tester.runAsync(
    () => Future<void>.delayed(const Duration(milliseconds: 300)),
  );
  await settle(tester);
  // The header's zoom follows after the frame that fitted it.
  await tester.pump();
  return harness;
}

/// The zoom the header shows, as a percentage: the leftmost "N%" in it.
Future<int> zoomShown(WidgetTester tester) async {
  // The header's zoom follows a frame after the zoom itself.
  await tester.pump();
  final percents = find.byWidgetPredicate(
    (w) => w is Text && RegExp(r'^\d+%$').hasMatch(w.data ?? ''),
  );
  final texts = percents.evaluate().toList()
    ..sort(
      (a, b) => tester
          .getTopLeft(find.byWidget(a.widget))
          .dx
          .compareTo(tester.getTopLeft(find.byWidget(b.widget)).dx),
    );
  final shown = (texts.first.widget as Text).data!;
  return int.parse(shown.substring(0, shown.length - 1));
}

Finder get picture => find.byType(InteractiveViewer);

void main() {
  group('registry', () {
    test('knows text and images, and nothing else yet', () {
      expect(viewerFor('text')?.label, 'Text');
      expect(viewerFor('image')?.label, 'Image');
      expect(viewerFor('chat'), isNull, reason: 'not a viewer');
      expect(viewerFor('pdf'), isNull);
    });

    test('a pane is called by its title, else its file name', () {
      expect(viewerTitle(ViewLeaf(kind: 'text', path: '/w/a.rs')), 'a.rs');
      expect(
        viewerTitle(ViewLeaf(kind: 'text', path: '/w/a.rs', title: 'Plan')),
        'Plan',
      );
      expect(viewerTitle(ViewLeaf(kind: 'text', path: '/w/dir/')), 'dir');
    });

    testWidgets('an unknown kind gets a plain file glyph and says so', (
      tester,
    ) async {
      await showPanes(
        tester,
        beside('dashboard', '/w/board.json'),
        files: {'/w/board.json': text('/w/board.json', '{}')},
      );
      expect(
        find.text("Claktory can't show this kind of file here yet."),
        findsOneWidget,
      );
      expect(find.text('/w/board.json'), findsOneWidget);
      expect(find.byType(FileGlyph), findsWidgets);
    });
  });

  group('text', () {
    testWidgets('Markdown shows formatted, and Plain shows its source', (
      tester,
    ) async {
      await showPanes(
        tester,
        beside('text', '/w/notes.md'),
        files: {
          '/w/notes.md': text(
            '/w/notes.md',
            '# Plans\n\nShip it.\n',
            mime: 'text/markdown',
          ),
        },
      );
      expect(find.text('# Plans'), findsNothing);
      expect(find.text('Plans', findRichText: true), findsOneWidget);
      await tester.tap(find.text('Plain'));
      await settle(tester);
      expect(find.text('# Plans'), findsOneWidget);
      await tester.tap(find.text('Formatted'));
      await settle(tester);
      expect(find.text('# Plans'), findsNothing);
    });

    testWidgets('a file cut short says so; one that cannot be read says why', (
      tester,
    ) async {
      await showPanes(
        tester,
        split('s1', row, [
          leaf('chat', 'chat'),
          split('s2', column, [
            leaf('v1', 'text', '/w/big.log'),
            leaf('v2', 'text', '/w/gone.txt'),
          ]),
        ]),
        files: {
          '/w/big.log': FileContent(
            path: '/w/big.log',
            mime: 'text/plain',
            size: Int64(5 * 1024 * 1024),
            data: utf8.encode('line\n'),
            truncated: true,
          ),
        },
      );
      expect(
        find.textContaining('Showing the first 5 bytes of'),
        findsOneWidget,
      );
      expect(find.text('No such file: /w/gone.txt'), findsOneWidget);
    });

    testWidgets('an empty file, and one that is not text, say so', (
      tester,
    ) async {
      await showPanes(
        tester,
        split('s1', row, [
          leaf('chat', 'chat'),
          split('s2', column, [
            leaf('v1', 'text', '/w/empty.txt'),
            leaf('v2', 'text', '/w/app.bin'),
          ]),
        ]),
        files: {
          '/w/empty.txt': text('/w/empty.txt', ''),
          '/w/app.bin': FileContent(
            path: '/w/app.bin',
            mime: 'application/octet-stream',
            size: Int64(4),
            data: [0, 1, 2, 3],
          ),
        },
      );
      expect(find.text('This file is empty.'), findsOneWidget);
      expect(
        find.text("Claktory can't show this kind of file here yet."),
        findsOneWidget,
      );
    });

    testWidgets('its text is selectable for Copy; the line numbers are not', (
      tester,
    ) async {
      await showPanes(
        tester,
        beside(),
        files: {'/w/main.rs': text('/w/main.rs', 'fn main() {\n}\n')},
      );
      expect(
        find.ancestor(
          of: find.text('fn main() {'),
          matching: find.byType(SelectableRegion),
        ),
        findsOneWidget,
      );
      // The gutter's numbers are left out of what's selected.
      expect(
        find.ancestor(
          of: find.text('1'),
          matching: find.byWidgetPredicate(
            (w) => w is SelectionContainer && w.registrar == null,
          ),
        ),
        findsOneWidget,
      );
    });
  });

  group('image', () {
    testWidgets('a small image shows its pixel size at 100%', (tester) async {
      await showImage(tester, image('/w/dot.png', await png(tester, 3, 2)));
      expect(find.textContaining('3 × 2'), findsOneWidget);
      expect(await zoomShown(tester), 100, reason: 'never enlarged to fit');
    });

    testWidgets('a double click goes to 100% and back to fitted', (
      tester,
    ) async {
      await showImage(
        tester,
        image('/w/big.png', await png(tester, 4000, 2000)),
      );
      final fitted = await zoomShown(tester);
      expect(fitted, lessThan(100));
      final at = tester.getCenter(picture);
      await tester.tapAt(at);
      await tester.pump(const Duration(milliseconds: 50));
      await tester.tapAt(at);
      await settle(tester);
      expect(await zoomShown(tester), 100);
      await tester.tapAt(at);
      await tester.pump(const Duration(milliseconds: 50));
      await tester.tapAt(at);
      await settle(tester);
      expect(await zoomShown(tester), fitted);
    });

    testWidgets('⌘+, ⌘− and ⌘0 zoom once it has been clicked', (tester) async {
      await showImage(
        tester,
        image('/w/big.png', await png(tester, 4000, 2000)),
      );
      final fitted = await zoomShown(tester);
      await tester.tapAt(tester.getCenter(picture));
      await settle(tester);
      Future<void> command(LogicalKeyboardKey key) async {
        await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
        await tester.sendKeyEvent(key);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
        await settle(tester);
      }

      await command(LogicalKeyboardKey.equal);
      expect(await zoomShown(tester), closeTo(fitted * 1.25, 1));
      await command(LogicalKeyboardKey.minus);
      expect(await zoomShown(tester), fitted);
      await command(LogicalKeyboardKey.minus);
      await command(LogicalKeyboardKey.minus);
      expect(await zoomShown(tester), 10, reason: 'never under a tenth');
      await command(LogicalKeyboardKey.digit0);
      expect(await zoomShown(tester), fitted);
    });

    testWidgets('a pinch zooms', (tester) async {
      await showImage(
        tester,
        image('/w/big.png', await png(tester, 4000, 2000)),
      );
      final fitted = await zoomShown(tester);
      final pinch = TestPointer(1, PointerDeviceKind.trackpad);
      final at = tester.getCenter(picture);
      await tester.sendEventToBinding(pinch.panZoomStart(at));
      for (var i = 1; i <= 5; i++) {
        await tester.sendEventToBinding(
          pinch.panZoomUpdate(at, scale: 1 + i * 0.2),
        );
        await tester.pump();
      }
      await tester.sendEventToBinding(pinch.panZoomEnd());
      await settle(tester);
      expect(await zoomShown(tester), greaterThan(fitted * 1.5));
    });

    testWidgets('one too big to read whole, or not an image, says so', (
      tester,
    ) async {
      await showPanes(
        tester,
        split('s1', row, [
          leaf('chat', 'chat'),
          split('s2', column, [
            leaf('v1', 'image', '/w/huge.png'),
            leaf('v2', 'image', '/w/fake.png'),
          ]),
        ]),
        files: {
          '/w/huge.png': FileContent(
            path: '/w/huge.png',
            mime: 'image/png',
            size: Int64(34 * 1000 * 1000),
            data: [137, 80, 78, 71],
            truncated: true,
          ),
          '/w/fake.png': image('/w/fake.png', utf8.encode('not a png')),
        },
      );
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 300)),
      );
      await settle(tester);
      expect(
        find.text('This image is too big to open here (34.0 MB).'),
        findsOneWidget,
      );
      expect(find.text("Claktory can't open this image."), findsOneWidget);
    });
  });
}
