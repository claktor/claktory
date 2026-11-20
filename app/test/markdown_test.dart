import 'package:factory_app/identity.dart';
import 'package:factory_app/loopback_forward.dart' show linkLauncher;
import 'package:factory_app/markdown.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

const _sample = '''# Title

A paragraph with **bold** and `code`.

- one
- two

> Quoted text.

```
let x = 1;
```

| A | B |
|---|---|
| c | d |
''';

Future<void> _show(WidgetTester tester, Widget child) => tester.pumpWidget(
  Directionality(
    textDirection: TextDirection.ltr,
    child: MediaQuery(
      data: const MediaQueryData(),
      child: SingleChildScrollView(child: child),
    ),
  ),
);

void main() {
  testWidgets('without a text block builder, every block is a plain Text', (
    tester,
  ) async {
    await _show(tester, const Markdown(_sample));
    // The transcript's path: the blocks are Text widgets, each straight
    // under the layout around it, with nothing wrapped around them.
    final texts = tester.widgetList<Text>(find.byType(Text)).toList();
    expect(
      [for (final text in texts) text.data ?? text.textSpan!.toPlainText()],
      [
        'Title',
        'A paragraph with bold and code.',
        '•',
        'one',
        '•',
        'two',
        'Quoted text.',
        'let x = 1;',
        'A',
        'B',
        'c',
        'd',
      ],
    );
    for (final text in find.byType(Text).evaluate()) {
      final parent = text.findAncestorWidgetOfExactType<KeyedSubtree>();
      expect(parent, isNull);
    }
  });

  testWidgets('a text block builder gets every block in reading order', (
    tester,
  ) async {
    final seen = <(int, String)>[];
    await _show(
      tester,
      Markdown(
        _sample,
        textBlock: (index, span, build) {
          seen.add((index, span.toPlainText()));
          return KeyedSubtree(key: ValueKey(index), child: build(span));
        },
      ),
    );
    expect(seen, [
      (0, 'Title'),
      (1, 'A paragraph with bold and code.'),
      (2, 'one'),
      (3, 'two'),
      (4, 'Quoted text.'),
      (5, 'let x = 1;'),
      (6, 'A'),
      (7, 'B'),
      (8, 'c'),
      (9, 'd'),
    ]);
    // What it builds is what shows, styled as before.
    expect(find.text('A paragraph with bold and code.'), findsOneWidget);
    expect(find.byKey(const ValueKey(5)), findsOneWidget);
  });

  group('links', () {
    late List<Uri> opened;
    setUp(() {
      opened = [];
      linkLauncher = (url) async {
        opened.add(url);
        return true;
      };
    });
    tearDown(() => linkLauncher = null);

    Future<void> showIn(
      WidgetTester tester,
      Widget child, {
      Brightness brightness = Brightness.light,
    }) => tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: MediaQuery(
          data: MediaQueryData(platformBrightness: brightness),
          child: Overlay(initialEntries: [OverlayEntry(builder: (_) => child)]),
        ),
      ),
    );

    /// The style [text] is drawn in, with what it inherits.
    TextStyle styleOf(WidgetTester tester, String text) {
      for (final paragraph in tester.renderObjectList<RenderParagraph>(
        find.byType(RichText),
      )) {
        TextStyle? found;
        void visit(InlineSpan span, TextStyle inherited) {
          final style = inherited.merge(span.style);
          if (span is TextSpan) {
            if (span.text == text) found ??= style;
            for (final child in span.children ?? const <InlineSpan>[]) {
              visit(child, style);
            }
          }
        }

        visit(paragraph.text, const TextStyle());
        if (found != null) return found!;
      }
      throw StateError('no span "$text"');
    }

    Offset centerOf(WidgetTester tester, String text) {
      final found = find.textRange.ofSubstring(text).evaluate().first;
      final paragraph = found.renderObject;
      final box = paragraph
          .getBoxesForSelection(
            TextSelection(
              baseOffset: found.textRange.start,
              extentOffset: found.textRange.end,
            ),
          )
          .first
          .toRect();
      return paragraph.localToGlobal(box.center);
    }

    for (final brightness in Brightness.values) {
      testWidgets('a link is in the link color, underlined faintly '
          '(${brightness.name})', (tester) async {
        await showIn(
          tester,
          const Markdown(
            'See [the docs](https://a.test/docs) and '
            'https://b.test/raw for more.',
          ),
          brightness: brightness,
        );
        final link = brightness == Brightness.dark
            ? const Color(0xFF6AA8FF)
            : const Color(0xFF1F5FD1);
        for (final text in ['the docs', 'https://b.test/raw']) {
          final style = styleOf(tester, text);
          expect(style.color, link, reason: text);
          expect(style.decoration, TextDecoration.underline);
          expect(style.decorationColor, link.withValues(alpha: 0.4));
        }
        // The text around them keeps its own color.
        expect(styleOf(tester, 'See ').color, isNot(link));
      });
    }

    testWidgets('hovered, its underline is full; pressed, it dims, and '
        'still opens', (tester) async {
      await showIn(
        tester,
        const Markdown('See [the docs](https://a.test/docs) now.'),
      );
      const link = Color(0xFF1F5FD1);
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: Offset.zero);
      addTearDown(mouse.removePointer);
      await mouse.moveTo(centerOf(tester, 'the docs'));
      await tester.pump();
      expect(styleOf(tester, 'the docs').decorationColor, link);
      // Hovering stays put: the rebuilt text doesn't leave and re-enter.
      await tester.pump();
      await tester.pump();
      expect(styleOf(tester, 'the docs').decorationColor, link);

      await mouse.down(centerOf(tester, 'the docs'));
      await tester.pump();
      expect(styleOf(tester, 'the docs').color, link.withValues(alpha: 0.7));
      await mouse.up();
      await tester.pump();
      expect(opened, [Uri.parse('https://a.test/docs')]);
      expect(styleOf(tester, 'the docs').color, link);

      await mouse.moveTo(centerOf(tester, 'now'));
      await tester.pump();
      expect(
        styleOf(tester, 'the docs').decorationColor,
        link.withValues(alpha: 0.4),
      );
    });

    testWidgets("on a bubble's fill, a link keeps the text's color", (
      tester,
    ) async {
      const white = Color(0xFFFFFFFF);
      await showIn(
        tester,
        const Markdown('See [the docs](https://a.test/docs).', color: white),
      );
      final style = styleOf(tester, 'the docs');
      expect(style.color, white);
      expect(style.decorationColor, white.withValues(alpha: 0.4));
    });

    testWidgets("muted text's links are in the link color too", (tester) async {
      await showIn(
        tester,
        Builder(
          builder: (context) => Markdown(
            'See [the docs](https://a.test/docs).',
            color: Palette.of(context).textMuted,
          ),
        ),
      );
      expect(styleOf(tester, 'the docs').color, const Color(0xFF1F5FD1));
    });

    testWidgets("a command's output shows its addresses in the link color", (
      tester,
    ) async {
      await showIn(
        tester,
        const CodeBlock('Open https://dash.test/x now.', links: true),
        brightness: Brightness.dark,
      );
      final style = styleOf(tester, 'https://dash.test/x');
      expect(style.color, const Color(0xFF6AA8FF));
      expect(style.decoration, TextDecoration.underline);
      await tester.tapOnText(find.textRange.ofSubstring('https://dash.test/x'));
      expect(opened, [Uri.parse('https://dash.test/x')]);
    });

    test('a long bare address shows as its host', () {
      const long = 'https://dash.cloudflare.com/profile/api-tokens';
      expect(shortAddress(long, long), 'dash.cloudflare.com/…');
      expect(
        shortAddress(
          'www.example.com/a/long/path/to/a/page',
          'http://www.example.com/a/long/path/to/a/page',
        ),
        'example.com/…',
      );
      // Short enough to show whole, or a link with words of its own.
      expect(shortAddress('https://a.test/x', 'https://a.test/x'), isNull);
      expect(shortAddress('the token page', long), isNull);
    });

    const long = 'https://dash.cloudflare.com/profile/api-tokens';

    testWidgets('with short links, a long bare address shows as its host, '
        'opens whole, and shows whole in a tip', (tester) async {
      await showIn(
        tester,
        const Markdown(
          'Make one at $long, as [the guide](https://a.test/guide) says.',
          shortLinks: true,
        ),
      );
      await tester.pump();
      expect(find.textContaining(long), findsNothing);
      final short = styleOf(tester, 'dash.cloudflare.com/…');
      expect(short.color, const Color(0xFF1F5FD1));
      expect(short.decoration, TextDecoration.underline);
      // A markdown link shows its words, as ever.
      expect(styleOf(tester, 'the guide').color, const Color(0xFF1F5FD1));

      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: Offset.zero);
      addTearDown(mouse.removePointer);
      await mouse.moveTo(centerOf(tester, 'dash.cloudflare.com/…'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 500));
      expect(find.text(long), findsOneWidget);
      // Below the pointer.
      expect(
        tester.getTopLeft(find.text(long)).dy,
        greaterThan(centerOf(tester, 'dash.cloudflare.com/…').dy),
      );

      // Not on the other link, nor off them.
      await mouse.moveTo(centerOf(tester, 'the guide'));
      await tester.pump();
      expect(find.text(long), findsNothing);
      await mouse.moveTo(Offset.zero);
      await tester.pump();
      expect(find.text(long), findsNothing);

      await tester.tapOnText(
        find.textRange.ofSubstring('dash.cloudflare.com/…'),
      );
      expect(opened, [Uri.parse(long)]);
    });

    testWidgets('without short links, an address shows whole', (tester) async {
      await showIn(tester, const Markdown('Make one at $long.'));
      expect(styleOf(tester, long).color, const Color(0xFF1F5FD1));
      expect(find.textContaining('…'), findsNothing);
    });

    testWidgets('a link that goes while hovered is let go quietly', (
      tester,
    ) async {
      await showIn(
        tester,
        const Markdown('See [the docs](https://a.test/docs).'),
      );
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: centerOf(tester, 'the docs'));
      addTearDown(mouse.removePointer);
      await tester.pump();
      await showIn(tester, const SizedBox());
      await mouse.moveTo(Offset.zero);
      await tester.pump();
      expect(tester.takeException(), isNull);
    });
  });
}
