// PaneLayout through a PaneProjection: how an iPad presents an agent's
// layout in a window without room for all of it (docs/ipad-workbench.md).
// The Mac's panes, with none, are panes_test.dart's.
import 'package:factory_app/controls.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/layout.dart' show leaves, resize;
import 'package:factory_app/panes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show leaf, row, snapshot, split, text;

/// The conversation, then main.rs and notes.md beside it.
ViewNode _three() => split('s1', row, [
  leaf('chat', 'chat'),
  leaf('v1', 'text', '/w/main.rs'),
  leaf('v2', 'text', '/w/notes.md'),
]);

/// A window that shows every leaf but [hiding], each at least [least].
class _Window {
  _Window({this.hiding = const {}, this.focused = 'chat', this.least = 300});

  final Set<String> hiding;
  String focused;
  final double least;

  /// What PaneLayout asked to present, and in what room.
  final asked = <({ViewNode full, Size room})>[];

  /// What it sent back through [PaneView.resized].
  final resizes = <({ViewNode full, String split, List<double> shares})>[];

  final focuses = <String>[];

  PaneView project(ViewNode full, Size room) {
    asked.add((full: full, room: room));
    return _View(this, full);
  }
}

class _View implements PaneView {
  _View(this.window, this.full);

  final _Window window;
  final ViewNode full;

  @override
  ViewNode get shown {
    final split = full.split;
    return ViewNode(
      id: full.id,
      split: ViewSplit(
        axis: split.axis,
        children: [
          for (final child in split.children)
            if (!window.hiding.contains(child.id)) child,
        ],
      ),
    );
  }

  @override
  List<String> get hidden => [
    for (final leaf in leaves(full))
      if (window.hiding.contains(leaf.id)) leaf.id,
  ];

  @override
  String get focused => window.focused;

  @override
  ViewNode resized(ViewNode full, String splitId, List<double> shownShares) {
    window.resizes.add((full: full, split: splitId, shares: shownShares));
    return resize(full, splitId, [
      if (shownShares.isNotEmpty) ...[...shownShares, 0.1],
    ]);
  }

  @override
  double minExtent(ViewNode node, bool row) => window.least;
}

Future<Harness> _show(
  WidgetTester tester,
  _Window window, {
  ViewNode? layout,
}) async {
  final harness = Harness(snapshot(layout: layout ?? _three()));
  harness.backend.files.addAll({
    '/w/main.rs': text('/w/main.rs', 'fn main() {}\n'),
    '/w/notes.md': text('/w/notes.md', '# Notes\n'),
  });
  await harness.show(
    tester,
    home: Builder(
      builder: (context) {
        final store = AppScope.of(context).workspace;
        return ListenableBuilder(
          listenable: store,
          builder: (context, _) => PaneProjection(
            project: window.project,
            focus: window.focuses.add,
            child: PaneLayout(
              agent: store.selectedAgent!,
              chat: const ColoredBox(color: Color(0xFF808080)),
            ),
          ),
        );
      },
    ),
  );
  await settle(tester);
  return harness;
}

Finder _pane(String id) => find.byKey(ValueKey('pane-$id'));

List<SetLayout> _sent(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasSetLayout()) message.setLayout,
];

void main() {
  testWidgets('only the panes shown are laid out, in the room PaneLayout '
      'has; a window resize sends nothing', (tester) async {
    final window = _Window(hiding: {'v2'});
    final harness = await _show(tester, window);
    expect(_pane('chat'), findsOneWidget);
    expect(_pane('v1'), findsOneWidget);
    expect(_pane('v2'), findsNothing);
    expect(window.asked.last.room, const Size(1200, 900));
    expect(window.asked.last.full.split.children, hasLength(3));

    tester.view.physicalSize = const Size(900, 900);
    await settle(tester);
    expect(window.asked.last.room, const Size(900, 900));
    expect(_sent(harness), isEmpty);
  });

  testWidgets('Close changes the whole layout by id, hidden panes and all, '
      'and what shows is projected from it', (tester) async {
    final window = _Window(hiding: {'v2'});
    final harness = await _show(tester, window);
    // The backend's echo, as it comes: the layout as closed.
    harness.backend.appliesLayouts = true;
    window.asked.clear();
    await tester.tap(
      find.descendant(
        of: _pane('v1'),
        matching: find.byWidgetPredicate(
          (widget) => widget is GlyphButton && widget.label == 'Close',
        ),
      ),
    );
    await tester.pump();
    final sent = _sent(harness).single.root;
    expect([for (final leaf in leaves(sent)) leaf.id], ['chat', 'v2']);
    // Projected from the layout as closed, hidden pane and all.
    expect(
      window.asked.map(
        (asked) => [for (final leaf in leaves(asked.full)) leaf.id],
      ),
      anyElement(equals(['chat', 'v2'])),
    );
  });

  testWidgets("a divider drag works on the shown split with the window's "
      'least sizes, and sends the whole layout resized back', (tester) async {
    final window = _Window(hiding: {'v2'}, least: 400);
    final harness = await _show(tester, window);
    final divider = find.byKey(const ValueKey('divider-s1-0'));
    final gesture = await tester.startGesture(tester.getCenter(divider));
    for (var i = 0; i < 10; i++) {
      await gesture.moveBy(const Offset(100, 0));
      await tester.pump();
    }
    // Held at the window's least, not the Mac's 200.
    expect(tester.getRect(_pane('v1')).width, closeTo(400, 1));
    expect(_sent(harness), isEmpty);
    await gesture.up();
    await settle(tester);
    final resize = window.resizes.single;
    expect(resize.split, 's1');
    expect(resize.full.split.children, hasLength(3));
    expect(resize.shares, hasLength(2));
    expect(resize.shares.first, greaterThan(resize.shares.last));
    expect(_sent(harness).single.root.split.weights, hasLength(3));

    // A double click resets it: equal shares among the shown.
    await tester.tap(divider);
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tap(divider);
    await settle(tester);
    expect(window.resizes.last.shares, isEmpty);
    expect(_sent(harness), hasLength(2));
  });

  testWidgets('touching a pane focuses it, without taking the touch', (
    tester,
  ) async {
    final window = _Window();
    await _show(tester, window);
    await tester.tap(find.text('fn main() {}'));
    await tester.pump();
    expect(window.focuses, ['v1']);
  });

  group('on an iPad', () {
    final iPad = TargetPlatformVariant.only(TargetPlatform.iOS);

    testWidgets("a finger's sizes: 44pt headers and dividers; the focused "
        "pane's header is marked", (tester) async {
      final window = _Window(focused: 'v1');
      await _show(tester, window);
      final header = find.descendant(
        of: _pane('v1'),
        matching: find.byType(PaneHeader),
      );
      expect(tester.getSize(header).height, greaterThanOrEqualTo(44));
      expect(
        tester.getSize(find.byKey(const ValueKey('divider-s1-0'))).width,
        44,
      );
      final palette = Palette.of(tester.element(header));
      Color fill(String id) {
        final box = tester
            .widgetList<Container>(
              find.descendant(
                of: find.descendant(
                  of: _pane(id),
                  matching: find.byType(PaneHeader),
                ),
                matching: find.byType(Container),
              ),
            )
            .firstWhere((box) => box.decoration is BoxDecoration);
        return (box.decoration! as BoxDecoration).color!;
      }

      expect(fill('v1'), palette.selected);
      expect(fill('v2'), palette.sidebar);
    }, variant: iPad);

    testWidgets('a pane moves by its header only after a hold, so a swipe '
        'across it does nothing; the pane moved is focused', (tester) async {
      // Two shown, each wide enough to take a pane at its edge.
      final window = _Window(hiding: {'v1'});
      final harness = await _show(tester, window);
      final chat = tester.getRect(_pane('chat'));
      Future<void> dragTo({required bool hold}) async {
        final gesture = await tester.startGesture(
          // On its glyph, clear of the divider's 22pt at its left.
          tester.getTopLeft(
                find.descendant(
                  of: _pane('v2'),
                  matching: find.byType(PaneHeader),
                ),
              ) +
              const Offset(30, 22),
        );
        if (hold) await tester.pump(const Duration(milliseconds: 700));
        await gesture.moveBy(const Offset(-40, 0));
        await tester.pump();
        await gesture.moveTo(Offset(chat.left + 30, chat.center.dy));
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 200));
        await gesture.up();
        await settle(tester);
      }

      await dragTo(hold: false);
      expect(_sent(harness), isEmpty);

      window.focuses.clear();
      await dragTo(hold: true);
      final root = _sent(harness).single.root;
      final ids = [for (final leaf in leaves(root)) leaf.id];
      expect(ids.first, 'v2');
      expect(ids, contains('v1'), reason: 'the hidden pane stays');
      expect(window.focuses.last, 'v2');
    }, variant: iPad);
  });
}
