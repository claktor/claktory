// A document on a phone (text, Markdown, a PDF), portrait and landscape,
// handled as a picture or video is: the page runs to the screen's edges
// with its words inside the safe areas, a tap hides or shows the header
// (and the status bar with it), and turning the phone keeps the place
// being read.
import 'package:factory_app/pane_chrome.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/markdown_media.dart' show MarkdownPicture;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'mobile_viewer_shots_test.dart';
import 'viewers_test.dart' show png;

/// How far the header is open: 1 shown, 0 hidden.
double _header(WidgetTester tester) => tester
    .widget<AnimatedAlign>(
      find.ancestor(
        of: find.byType(PhonePaneHeader),
        matching: find.byType(AnimatedAlign),
      ),
    )
    .heightFactor!;

/// A lone tap, then long enough for no double tap to follow and the
/// header to move.
Future<void> _tap(WidgetTester tester, Offset at) async {
  await tester.tapAt(at);
  await tester.pump(kDoubleTapTimeout + const Duration(milliseconds: 20));
  await pumpFrames(tester);
}

/// The top of what the page shows: the viewer's body.
double _bodyTop(WidgetTester tester) => tester
    .getTopLeft(
      find.byWidget(
        tester.widget<PaneScaffold>(find.byType(PaneScaffold)).body,
      ),
    )
    .dy;

void main() {
  final systemModes = <String>[];
  setUp(() {
    systemModes.clear();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
          if (call.method == 'SystemChrome.setEnabledSystemUIMode') {
            systemModes.add('${call.arguments}');
          }
          return null;
        });
  });
  tearDown(() {
    debugDefaultTargetPlatformOverride = null;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, null);
  });

  testWidgets('in landscape, a tap gives a text file the whole screen and '
      'another brings the header back', (tester) async {
    await openOnPhone(tester, ViewerKind.text, phoneLandscape);
    expect(_header(tester), 1);
    final withHeader = _bodyTop(tester);
    expect(withHeader, greaterThan(40));

    await _tap(tester, const Offset(426, 250));
    expect(_header(tester), 0);
    expect(_bodyTop(tester), 0);
    expect(systemModes.last, contains('immersive'));

    await _tap(tester, const Offset(426, 250));
    expect(_header(tester), 1);
    expect(_bodyTop(tester), withHeader);
    expect(systemModes.last, contains('edgeToEdge'));
  });

  testWidgets('on a turned phone the header goes a moment after the last '
      'touch, as a video\'s controls do; upright it stays', (tester) async {
    await openOnPhone(tester, ViewerKind.text, phoneLandscape);
    await tester.pump(const Duration(seconds: 4));
    await pumpFrames(tester);
    expect(_header(tester), 0);

    tester.view.physicalSize =
        phonePortrait.size * tester.view.devicePixelRatio;
    tester.view.padding = phonePortrait.padding;
    await pumpFrames(tester);
    await _tap(tester, const Offset(200, 500));
    expect(_header(tester), 1);
    await tester.pump(const Duration(seconds: 4));
    await pumpFrames(tester);
    expect(_header(tester), 1);
  });

  testWidgets('the header\'s fill runs under the status bar and to the '
      'screen\'s sides; its buttons and the words stay inside the safe '
      'areas', (tester) async {
    await openOnPhone(tester, ViewerKind.text, phoneLandscape);
    final fill = tester.getRect(
      find
          .ancestor(
            of: find.byType(PhonePaneHeader),
            matching: find.byType(ColoredBox),
          )
          .first,
    );
    expect(fill.left, 0);
    expect(fill.right, 852);
    expect(fill.top, 0);
    expect(
      tester.getRect(find.bySemanticsLabel('Back').last).left,
      greaterThanOrEqualTo(59),
    );
    final line = tester.getRect(find.textContaining('Line 1:').first);
    expect(line.left, greaterThanOrEqualTo(59));
    expect(line.right, lessThanOrEqualTo(852 - 59));

    tester.view.physicalSize =
        phonePortrait.size * tester.view.devicePixelRatio;
    tester.view.padding = phonePortrait.padding;
    await pumpFrames(tester);
    final portrait = tester.getRect(
      find
          .ancestor(
            of: find.byType(PhonePaneHeader),
            matching: find.byType(ColoredBox),
          )
          .first,
    );
    expect(portrait.top, 0);
    // The status bar's height is the header's fill, its row under it.
    expect(
      tester.getRect(find.bySemanticsLabel('Back').last).top,
      greaterThanOrEqualTo(59),
    );
  });

  testWidgets('a double tap leaves the header as it is', (tester) async {
    await openOnPhone(tester, ViewerKind.text, phonePortrait);
    await tester.tapAt(const Offset(200, 500));
    await tester.pump(const Duration(milliseconds: 80));
    await tester.tapAt(const Offset(200, 500));
    await tester.pump(kDoubleTapTimeout * 2);
    await pumpFrames(tester);
    expect(_header(tester), 1);
  });

  testWidgets('turned, formatted text keeps the line being read at the top', (
    tester,
  ) async {
    await openOnPhone(tester, ViewerKind.text, phonePortrait);
    final view = find.byType(Scrollable).first;
    await tester.drag(view, const Offset(0, -900));
    await pumpFrames(tester);
    final top = _bodyTop(tester);
    // The first line whose top is in view, read from the top.
    String firstLine() {
      final lines = find.byWidgetPredicate(
        (widget) =>
            widget is RichText &&
            RegExp(r'^Line \d+:').hasMatch(widget.text.toPlainText()),
      );
      String? best;
      var bestTop = double.infinity;
      for (final element in lines.evaluate()) {
        final box = element.renderObject! as RenderBox;
        final y = box.localToGlobal(Offset.zero).dy;
        if (y >= top - 1 && y < bestTop) {
          bestTop = y;
          best = (element.widget as RichText).text.toPlainText();
        }
      }
      return best!.split(':').first;
    }

    final before = firstLine();
    expect(before, isNot('Line 1'));
    tester.view.physicalSize =
        phoneLandscape.size * tester.view.devicePixelRatio;
    tester.view.padding = phoneLandscape.padding;
    await pumpFrames(tester);
    final turned = firstLine();
    final number = int.parse(before.split(' ').last);
    expect(
      int.parse(turned.split(' ').last),
      inInclusiveRange(number - 1, number + 1),
    );
  });

  testWidgets('on a turned phone a picture in Markdown is never taller than '
      'the screen', (tester) async {
    final data = await png(tester, 600, 1800);
    await openOnPhone(
      tester,
      ViewerKind.text,
      phoneLandscape,
      text: '# Shot\n\n![A tall one](tall.png)\n',
      files: {
        '/w/tall.png': FileContent(
          path: '/w/tall.png',
          mime: 'image/png',
          size: Int64(data.length),
          data: data,
          sha256: 'p1',
        ),
      },
    );
    for (var i = 0; i < 10; i++) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 60)),
      );
      await tester.pump();
    }
    await pumpFrames(tester);
    final picture = tester.getRect(
      find.descendant(
        of: find.byType(MarkdownPicture),
        matching: find.byType(RawImage),
      ),
    );
    expect(picture.height, lessThan(393));
    expect(picture.width / picture.height, closeTo(600 / 1800, 0.01));
  });

  testWidgets('the status bar follows the page in front: a picture opened '
      'from a document whose header is hidden shows it; back, it goes', (
    tester,
  ) async {
    final data = await png(tester, 600, 900);
    await openOnPhone(
      tester,
      ViewerKind.text,
      phonePortrait,
      text: '# Shot\n\nSome words to read first.\n\n![A picture](tall.png)\n',
      files: {
        '/w/tall.png': FileContent(
          path: '/w/tall.png',
          mime: 'image/png',
          size: Int64(data.length),
          data: data,
          sha256: 'p1',
        ),
      },
    );
    for (var i = 0; i < 10; i++) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 60)),
      );
      await tester.pump();
    }
    await pumpFrames(tester);
    await _tap(
      tester,
      tester.getCenter(find.text('Some words to read first.')),
    );
    expect(_header(tester), 0);
    expect(systemModes.last, contains('immersive'));

    await tester.tap(find.byType(MarkdownPicture));
    for (var i = 0; i < 10; i++) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 60)),
      );
      await tester.pump();
    }
    await pumpFrames(tester);
    expect(systemModes.last, contains('edgeToEdge'));

    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(systemModes.last, contains('immersive'));
    expect(_header(tester), 0);
  });
}
