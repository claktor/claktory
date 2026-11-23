// Search and contents in a PDF on an iPad (ipad/document_find.dart): the
// pane's Search and Contents open a panel at the page's start; native
// finds and highlights, the panel lists each match with its page and its
// sentence, and tapping one, or a contents entry, shows that place.
import 'dart:async';
import 'dart:ui' show Tristate;

import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/ipad/document_find.dart';
import 'package:factory_app/ipad/ink_surface.dart' show InkSurface;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'as_ipad.dart';
import 'document_native.dart';

/// Wide enough that the paper's pane keeps the panel beside the page.
const _wide = Size(2000, 1024);

Finder _inPanel(Finder finder) => find.descendant(
  of: find.byKey(const ValueKey('find-panel')),
  matching: finder,
);

List<Map<String, Object?>> _sent(DocumentNative native, String method) => [
  for (final call in native.calls)
    if (call.method == method) (call.arguments as Map).cast<String, Object?>(),
];

Future<void> _type(WidgetTester tester, String text) async {
  await tester.enterText(_inPanel(find.byType(EditableText)), text);
  await tester.pump(const Duration(milliseconds: 300));
  await settle(tester);
}

void main() {
  testWidgets('Search finds in the PDF; each match shows its page and its '
      'sentence, and tapping it goes there', (tester) async {
    asIPad();
    final semantics = tester.ensureSemantics();
    final native = DocumentNative();
    await showDocument(tester, asking: false, size: _wide);
    await native.ready();
    await settle(tester);
    // No contents in this PDF: no Contents.
    expect(find.byKey(const ValueKey('document-contents')), findsNothing);

    await tester.tap(find.byKey(const ValueKey('document-search')));
    await settle(tester);
    expect(find.text('Search this PDF'), findsOneWidget);

    await tester.enterText(_inPanel(find.byType(EditableText)), 'riv');
    await tester.pump(const Duration(milliseconds: 100));
    await _type(tester, 'river');
    // Typing paused once: one search, for what was typed by then.
    expect(_sent(native, 'search'), [
      {'query': 'river', 'max_results': 200},
    ]);
    expect(_inPanel(find.text('2 matches')), findsOneWidget);
    expect(_inPanel(find.text('Page 2')), findsOneWidget);
    expect(_inPanel(find.text('Page 4')), findsOneWidget);
    final excerpt = tester.widget<RichText>(
      _inPanel(
        find.byWidgetPredicate(
          (w) => w is RichText && w.text.toPlainText().contains('page 4'),
        ),
      ),
    );
    final bold = <String>[];
    excerpt.text.visitChildren((span) {
      if (span is TextSpan &&
          span.text != null &&
          span.style?.fontWeight == FontWeight.w600) {
        bold.add(span.text!);
      }
      return true;
    });
    expect(bold, ['river']);

    await tester.tap(_inPanel(find.text('Page 4')));
    await settle(tester);
    final go = _sent(native, 'goToPage').single;
    expect(go['page_1based'], 4);
    expect(go['region'], {'x': 0.1, 'y': 0.8, 'w': 0.2, 'h': 0.02});
    // The match marked apart from the others, high in the view.
    expect(go['match'], isTrue);
    // Beside the page, it stays for the next match, and says which one
    // the page shows.
    expect(find.byKey(const ValueKey('find-panel')), findsOneWidget);
    bool selected(String text) =>
        tester
            .getSemantics(_inPanel(find.text(text)))
            .flagsCollection
            .isSelected ==
        Tristate.isTrue;
    expect(selected('Page 4'), isTrue);
    expect(selected('Page 2'), isFalse);
    semantics.dispose();
    final panel = tester.getRect(find.byKey(const ValueKey('find-panel')));
    expect(panel.width, findPanelWidth);
    // The page beside it, all of it in view.
    expect(tester.getRect(find.byType(InkSurface)).left, panel.right);
  });

  testWidgets('an answer to an older search is dropped', (tester) async {
    asIPad();
    final native = DocumentNative();
    await showDocument(tester, asking: false, size: _wide);
    await native.ready();
    await settle(tester);
    final late = Completer<Map<String, Object?>>();
    final first = native.searchFor;
    native.searchFor = (arguments) =>
        arguments['query'] == 'cur' ? late.future : first(arguments);
    await tester.tap(find.byKey(const ValueKey('document-search')));
    await settle(tester);
    await _type(tester, 'cur');
    expect(_inPanel(find.text('Searching…')), findsOneWidget);
    await _type(tester, 'curlew');
    expect(_inPanel(find.text('2 matches')), findsOneWidget);
    late.complete({'query': 'cur', 'truncated': false, 'matches': <Object?>[]});
    await settle(tester);
    expect(_inPanel(find.text('2 matches')), findsOneWidget);
    expect(_inPanel(find.text('No matches')), findsNothing);
  });

  testWidgets('no matches, and more than are listed', (tester) async {
    asIPad();
    final native = DocumentNative();
    await showDocument(tester, asking: false, size: _wide);
    await native.ready();
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('document-search')));
    await settle(tester);
    await _type(tester, 'nothing');
    expect(_inPanel(find.text('No matches')), findsOneWidget);

    native.searchFor = (arguments) => {
      'query': arguments['query'],
      'truncated': true,
      'matches': [
        for (var i = 0; i < 200; i++) {'page_1based': 1, 'text': 'the the the'},
      ],
    };
    await _type(tester, 'the');
    expect(
      _inPanel(find.text('Over 200 matches · showing the first 200')),
      findsOneWidget,
    );
  });

  testWidgets('on a page PDFKit reads as one block, each row shows the '
      'words around its own match', (tester) async {
    asIPad();
    final native = DocumentNative();
    await showDocument(tester, asking: false, size: _wide);
    await native.ready();
    await settle(tester);
    // Line extension gives the whole page, for each of its matches: the
    // word on its first line and on its fiftieth.
    final page = [
      for (var i = 1; i <= 60; i++)
        'Line $i${i == 1 || i == 50 ? ' curlew' : ''}',
    ].join('\n');
    native.searchFor = (arguments) => {
      'query': arguments['query'],
      'truncated': false,
      'matches': [
        for (final y in [0.1, 0.8])
          {
            'page_1based': 3,
            'region': {'x': 0.1, 'y': y, 'w': 0.2, 'h': 0.02},
            'text': page,
          },
      ],
    };
    await tester.tap(find.byKey(const ValueKey('document-search')));
    await settle(tester);
    await _type(tester, 'curlew');
    final rows = [
      for (final element in _inPanel(
        find.byWidgetPredicate(
          (w) => w is RichText && w.text.toPlainText().contains('curlew'),
        ),
      ).evaluate())
        (element.widget as RichText).text.toPlainText(),
    ];
    expect(rows, hasLength(2));
    expect(rows[0], startsWith('Line 1 curlew Line 2'));
    expect(rows[0], endsWith('…'));
    expect(rows[1], startsWith('…'));
    expect(rows[1], contains('Line 50 curlew'));
    expect(rows[1].indexOf('curlew'), lessThan(50));
    expect(rows[1], isNot(contains('\n')));
  });

  test('an excerpt is cut to whole words around its match', () {
    final page = [for (var i = 1; i <= 60; i++) 'Line $i of page 3'].join('\n');
    final far = excerptAround(page, 'Line 44 of page 3', 0);
    expect(far, startsWith('…'));
    expect(far, endsWith('…'));
    expect(far, contains('Line 44 of page 3'));
    expect(far.indexOf('Line 44'), lessThan(50));
    // No word cut in half at either end.
    final flat = page.replaceAll('\n', ' ');
    expect(' $flat '.contains(' ${far.substring(1, far.length - 1)} '), isTrue);
    // A short line stays whole; a match ignoring accents shows its start.
    expect(
      excerptAround('Curlews nest near the river.', 'river', 0),
      'Curlews nest near the river.',
    );
    expect(excerptAround('Le café au bord', 'cafe', 0), 'Le café au bord');
    // The second of a line's two matches.
    final twice = '${'word ' * 30}river ${'word ' * 30}river ${'word ' * 30}';
    final second = excerptAround(twice, 'river', 1);
    expect('river'.allMatches(second), hasLength(1));
    expect(second.indexOf('river'), greaterThan(30));
  });

  testWidgets('Close and Esc close it, and the highlights go', (tester) async {
    asIPad();
    final native = DocumentNative();
    await showDocument(tester, asking: false, size: _wide);
    await native.ready();
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('document-search')));
    await settle(tester);
    await _type(tester, 'river');
    await tester.tap(find.byKey(const ValueKey('find-close')));
    await settle(tester);
    expect(find.byKey(const ValueKey('find-panel')), findsNothing);
    expect(_sent(native, 'search').last['query'], '');

    // Opened again: empty, and Esc closes it from the field.
    await tester.tap(find.byKey(const ValueKey('document-search')));
    await settle(tester);
    expect(_inPanel(find.text('2 matches')), findsNothing);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(find.byKey(const ValueKey('find-panel')), findsNothing);
  });

  testWidgets('Contents lists the PDF\'s own, indented, and goes to each '
      'one\'s page', (tester) async {
    asIPad();
    final native = DocumentNative();
    await showDocument(tester, asking: false, size: _wide);
    await native.ready(
      outline: [
        {'title': 'Spring survey', 'page_1based': 1, 'depth': 0},
        {'title': 'Curlews', 'page_1based': 3, 'depth': 1},
        {'title': 'What to do next', 'page_1based': 5, 'depth': 0},
      ],
    );
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('document-search')));
    await settle(tester);
    await _type(tester, 'river');
    // From a search to the contents: the search ends, highlights and all.
    await tester.tap(find.byKey(const ValueKey('document-contents')));
    await settle(tester);
    expect(_sent(native, 'search').last['query'], '');
    expect(_inPanel(find.text('Contents')), findsOneWidget);
    final top = tester.getTopLeft(_inPanel(find.text('Spring survey')));
    final nested = tester.getTopLeft(_inPanel(find.text('Curlews')));
    expect(nested.dx, greaterThan(top.dx));

    await tester.tap(_inPanel(find.text('Curlews')));
    await settle(tester);
    final go = _sent(native, 'goToPage').single;
    expect(go['page_1based'], 3);
    expect(go.containsKey('region'), isFalse);
  });

  testWidgets('in a narrow pane it covers the page, and going to a match '
      'closes it to show the place', (tester) async {
    asIPad();
    final native = DocumentNative();
    await showDocument(tester, asking: false);
    await native.ready();
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('document-search')));
    await settle(tester);
    final panel = tester.getRect(find.byKey(const ValueKey('find-panel')));
    expect(panel.width, lessThan(findPanelCovers));
    expect(panel.width, greaterThan(findPanelWidth));
    await _type(tester, 'river');
    await tester.tap(_inPanel(find.text('Page 2')));
    await settle(tester);
    final go = _sent(native, 'goToPage').single;
    expect(go['page_1based'], 2);
    expect(go['match'], isTrue);
    expect(find.byKey(const ValueKey('find-panel')), findsNothing);
    // The matches stay marked on the page (Zucchero's row 114), and Search
    // shows lit: it opens on the same search again.
    final searches = _sent(native, 'search').length;
    expect(_sent(native, 'search').last['query'], 'river');
    bool lit() => tester
        .widget<GlyphButton>(find.byKey(const ValueKey('document-search')))
        .active;
    expect(lit(), isTrue);
    await tester.tap(find.byKey(const ValueKey('document-search')));
    await settle(tester);
    expect(_inPanel(find.text('Page 2')), findsOneWidget);
    expect(_sent(native, 'search'), hasLength(searches));
    // Closed there, the search ends, marks and all.
    await tester.tap(find.byKey(const ValueKey('find-close')));
    await settle(tester);
    expect(_sent(native, 'search').last['query'], '');
    expect(lit(), isFalse);
  });

  testWidgets('every row is a finger\'s height, at the largest text too', (
    tester,
  ) async {
    asIPad();
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final native = DocumentNative();
    await showDocument(tester, asking: false, size: _wide);
    await native.ready();
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('document-search')));
    await settle(tester);
    await _type(tester, 'river');
    expect(tester.takeException(), isNull);
    final row = find
        .ancestor(
          of: _inPanel(find.text('Page 2')),
          matching: find.byType(Semantics),
        )
        .first;
    expect(tester.getSize(row).height, greaterThanOrEqualTo(44));
  });

  testWidgets('on the phone\'s routes, Search and Contents are in More', (
    tester,
  ) async {
    asIPad();
    final native = DocumentNative();
    await showDocument(
      tester,
      asking: false,
      size: const Size(375, 1024),
      phoneRoutes: true,
    );
    await native.ready(
      outline: [
        {'title': 'Curlews', 'page_1based': 3, 'depth': 0},
      ],
    );
    await settle(tester);
    await tester.tap(find.bySemanticsLabel('More'));
    await settle(tester);
    expect(find.text('Contents'), findsOneWidget);
    await tester.tap(find.text('Search'));
    await settle(tester);
    final panel = tester.getRect(find.byKey(const ValueKey('find-panel')));
    expect(panel.width, 375);
    await _type(tester, 'river');
    expect(_inPanel(find.text('2 matches')), findsOneWidget);
  });

  testWidgets('with the keyboard up, the last match scrolls fully above it, '
      'at the largest text', (tester) async {
    asIPad();
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final native = DocumentNative();
    native.searchFor = (arguments) => {
      'query': arguments['query'],
      'truncated': true,
      'matches': [
        for (var i = 1; i <= 200; i++)
          {'page_1based': i, 'text': 'Line $i near the river.'},
      ],
    };
    await showDocument(tester, asking: false, size: _wide);
    await native.ready();
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('document-search')));
    await settle(tester);
    tester.view.viewInsets = const FakeViewPadding(bottom: 400);
    addTearDown(tester.view.resetViewInsets);
    await _type(tester, 'river');
    final last = _inPanel(find.text('Page 200'));
    final list = find.descendant(
      of: find.byKey(const ValueKey('find-results')),
      matching: find.byType(Scrollable),
    );
    expect(list, findsOneWidget);
    // To the end of the list.
    for (var i = 0; i < 5; i++) {
      final position = tester.state<ScrollableState>(list).position;
      position.jumpTo(position.maxScrollExtent);
      await settle(tester);
    }
    expect(tester.takeException(), isNull);
    final row = find.ancestor(of: last, matching: find.byType(Semantics)).first;
    expect(tester.getRect(row).bottom, lessThanOrEqualTo(1024 - 400));
  });
}
