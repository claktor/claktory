import 'package:factory_app/markdown.dart';
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
}
