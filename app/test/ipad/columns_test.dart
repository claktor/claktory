import 'dart:ui' show Size;

import 'package:factory_app/ipad/columns.dart';
import 'package:factory_app/layout.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter_test/flutter_test.dart' hide find;

import 'window_fixtures.dart';

ViewNode leaf(String id, String kind) => ViewNode(
  id: id,
  leaf: ViewLeaf(kind: kind, path: kind == chatKind ? '' : '/f/$id'),
);

ViewNode split(
  String id,
  ViewAxis axis,
  List<ViewNode> children, [
  List<double> weights = const [],
]) => ViewNode(
  id: id,
  split: ViewSplit(axis: axis, children: children, weights: weights),
);

const row = ViewAxis.VIEW_AXIS_ROW;
const col = ViewAxis.VIEW_AXIS_COLUMN;

/// The tree as text: leaves by id, splits as row(…) or col(…).
String shape(ViewNode node) => node.hasSplit()
    ? '${node.split.axis == row ? 'row' : 'col'}'
          '(${node.split.children.map(shape).join(' ')})'
    : node.id;

/// Each shown leaf's width when [node] is laid out [width] wide, as
/// PaneLayout lays out shares: one hairline between each two.
Map<String, double> widths(ViewNode node, double width) {
  if (!node.hasSplit()) return {node.id: width};
  final shares = sharesOf(node.split);
  final along = node.split.axis == row;
  final room = width - (node.split.children.length - 1);
  return {
    for (var i = 0; i < node.split.children.length; i++)
      ...widths(node.split.children[i], along ? room * shares[i] : width),
  };
}

/// A paper beside the conversation, its notes under it: the layout the
/// workbench is for.
final paper = split(
  'root',
  row,
  [
    leaf('chat', chatKind),
    split('docs', col, [leaf('pdf', 'pdf'), leaf('notes', 'text')]),
  ],
  const [0.4, 0.6],
);

void main() {
  group('the window fixtures', () {
    for (final fixture in windowFixtures) {
      test('$fixture takes ${fixture.tier.name}', () {
        expect(
          tierFor(
            idiom: fixture.idiom,
            window: fixture.size,
            textFactor: fixture.textFactor,
          ),
          fixture.tier,
        );
      });
    }

    test('cover every tier, the phone and larger text', () {
      expect(
        windowFixtures.map((fixture) => fixture.tier).toSet(),
        WorkbenchTier.values.toSet(),
      );
      expect(windowFixtures.any((f) => f.idiom == Idiom.phone), isTrue);
      expect(windowFixtures.any((f) => f.textFactor > 2), isTrue);
      expect(windowFixtures.any((f) => f.keyboard > 0), isTrue);
      expect(
        windowFixtures.map((fixture) => fixture.name).toSet(),
        hasLength(windowFixtures.length),
      );
    });
  });

  group('tierFor', () {
    test('larger text never gives a wider composition', () {
      for (final fixture in windowFixtures) {
        var last = WorkbenchTier.full.index;
        for (final factor in [1.0, 1.2, 1.5, 2.0, 2.5, 3.2]) {
          final tier = tierFor(
            idiom: fixture.idiom,
            window: fixture.size,
            textFactor: factor,
          );
          expect(tier.index, lessThanOrEqualTo(last), reason: '$fixture');
          last = tier.index;
        }
      }
    });

    test('the iPad is never compact for being an iPad, only narrow', () {
      expect(
        tierFor(idiom: Idiom.pad, window: const Size(600, 440)),
        WorkbenchTier.focused,
      );
      expect(
        tierFor(idiom: Idiom.phone, window: const Size(1366, 1024)),
        WorkbenchTier.compact,
      );
    });

    test('the text factor is at least 1', () {
      expect(textFactorOf((size) => size * 0.8), 1);
      expect(textFactorOf((size) => size * 2), 2);
    });
  });

  group('project', () {
    for (final fixture in windowFixtures) {
      if (fixture.tier == WorkbenchTier.compact) continue;
      test('$fixture shows the focused leaf, each at its least width, and '
          'lists every other', () {
        for (final focused in ['chat', 'pdf', 'notes']) {
          final room = Size(
            contentWidth(fixture.tier, fixture.size.width),
            fixture.size.height,
          );
          final projection = project(
            paper,
            tier: fixture.tier,
            room: room,
            focused: focused,
            textFactor: fixture.textFactor,
          );
          final shown = leaves(projection.shown!).map((l) => l.id).toList();
          expect(shown, contains(focused));
          // Nothing lost, nothing twice, the layout's order kept.
          expect({...shown, ...projection.hidden}, {'chat', 'pdf', 'notes'});
          expect(
            shown.toSet().intersection(projection.hidden.toSet()),
            isEmpty,
          );
          final order = leaves(paper).map((l) => l.id).toList();
          expect(shown, [...order.where(shown.contains)]);
          if (fixture.tier == WorkbenchTier.focused) {
            expect(shown, [focused]);
          }
          // When more than one is shown, each has its least width.
          if (shown.length > 1) {
            final laid = widths(projection.shown!, room.width);
            for (final id in shown) {
              final node = find(paper, id)!;
              expect(
                laid[id],
                greaterThanOrEqualTo(
                  leafMinWidth(node, fixture.textFactor) - 0.5,
                ),
                reason: '$id in $fixture',
              );
            }
          }
        }
      });
    }

    test('wide windows show the whole paper layout', () {
      for (final size in const [Size(1366, 1024), Size(840, 1000)]) {
        final tier = tierFor(idiom: Idiom.pad, window: size);
        final projection = project(
          paper,
          tier: tier,
          room: Size(contentWidth(tier, size.width), size.height),
        );
        expect(shape(projection.shown!), 'row(chat col(pdf notes))');
        expect(projection.hidden, isEmpty);
      }
    });

    test('at large text the conversation keeps the focused document', () {
      // Two text columns no longer fit side by side: the notes alone.
      expect(
        shape(
          project(
            paper,
            tier: WorkbenchTier.paired,
            room: Size(contentWidth(WorkbenchTier.paired, 1376), 1032),
            focused: 'notes',
            textFactor: TextFactor.ax3,
          ).shown!,
        ),
        'notes',
      );
      final projection = project(
        paper,
        tier: WorkbenchTier.paired,
        room: Size(contentWidth(WorkbenchTier.paired, 1376), 1032),
        focused: 'pdf',
        textFactor: TextFactor.ax3,
      );
      expect(shape(projection.shown!), 'row(chat pdf)');
      expect(projection.hidden, ['notes']);
      // The root split keeps its id, so a divider drag maps back.
      expect(projection.shown!.id, 'root');
    });

    test('a short window stacks no more than fits', () {
      final projection = project(
        paper,
        tier: WorkbenchTier.full,
        room: const Size(1000, 380),
        focused: 'pdf',
      );
      expect(shape(projection.shown!), 'row(chat pdf)');
      expect(projection.hidden, ['notes']);
    });

    test('shares are raised for the presentation only', () {
      final narrow = split(
        'root',
        row,
        [leaf('chat', chatKind), leaf('pdf', 'pdf')],
        const [0.9, 0.1],
      );
      final projection = project(
        narrow,
        tier: WorkbenchTier.paired,
        room: const Size(783, 1000),
      );
      final laid = widths(projection.shown!, 783);
      expect(laid['pdf'], greaterThanOrEqualTo(400 - 0.5));
      expect(laid['chat'], greaterThanOrEqualTo(320 - 0.5));
      // The layout itself is untouched.
      expect(narrow.split.weights, [0.9, 0.1]);
    });

    test('compact lists everything but the focus as hidden, no tree', () {
      final projection = project(
        paper,
        tier: WorkbenchTier.compact,
        room: const Size(375, 1024),
        focused: 'pdf',
      );
      expect(projection.shown, isNull);
      expect(projection.hidden, ['chat', 'notes']);
    });

    test('a focus that was closed elsewhere falls back to the '
        'conversation', () {
      expect(focusIn(paper, 'gone'), 'chat');
      expect(focusIn(paper, null), 'chat');
      expect(focusIn(paper, 'pdf'), 'pdf');
    });
  });

  group('resizeBack', () {
    test('a hidden child keeps its share; the shown ones share the rest', () {
      final three = split(
        'root',
        row,
        [leaf('chat', chatKind), leaf('pdf', 'pdf'), leaf('notes', 'text')],
        const [0.5, 0.25, 0.25],
      );
      final projection = project(
        three,
        tier: WorkbenchTier.paired,
        room: const Size(783, 1000),
        focused: 'pdf',
      );
      expect(shape(projection.shown!), 'row(chat pdf)');
      final next = resizeBack(three, projection, 'root', const [0.5, 0.5]);
      expect(next.split.weights, [0.375, 0.375, 0.25]);
      expect(shape(next), shape(three));
      // A reset shares the shown children's room equally.
      final reset = resizeBack(three, projection, 'root', const []);
      expect(reset.split.weights, [0.375, 0.375, 0.25]);
    });

    test('a nested split collapsed to one shown leaf maps to its child', () {
      final projection = project(
        paper,
        tier: WorkbenchTier.full,
        room: const Size(1000, 380),
        focused: 'pdf',
      );
      final next = resizeBack(paper, projection, 'root', const [0.3, 0.7]);
      expect(next.split.weights, [0.3, 0.7]);
      // The column inside keeps its own shares.
      expect(find(next, 'docs')!.split.children.map((c) => c.id), [
        'pdf',
        'notes',
      ]);
    });

    test('an unknown split or a mismatched drag changes nothing', () {
      final projection = project(
        paper,
        tier: WorkbenchTier.full,
        room: const Size(1076, 1024),
      );
      expect(
        identical(resizeBack(paper, projection, 'x', const [1]), paper),
        isTrue,
      );
      expect(
        identical(resizeBack(paper, projection, 'root', const [1]), paper),
        isTrue,
      );
    });
  });

  group('compactRoutes', () {
    test('the conversation, with the focused viewer over it', () {
      expect(compactRoutes(paper, 'pdf'), ['chat', 'pdf']);
      expect(compactRoutes(paper, 'chat'), ['chat']);
      expect(compactRoutes(paper, 'gone'), ['chat']);
    });
  });
}
