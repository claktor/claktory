// The iPad's drawing tools (ipad/ink_tools.dart): choosing a tool or a
// color sends it, Undo and Redo follow the surface, a Pencil's double tap
// is followed without being sent back, and the bar fits the narrowest
// pane at every text size.
import 'package:factory_app/ipad/ink_bridge.dart' show InkTool, InkToolKind;
import 'package:factory_app/ipad/ink_tools.dart';

import 'dart:ui' show Tristate;

import 'package:factory_app/protocol/factory/v1/factory.pb.dart' show Workspace;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'as_ipad.dart';
import 'window_fixtures.dart' show TextFactor;

class _Sent {
  final choices = <InkTool>[];
  var undos = 0;
  var redos = 0;
  final fingers = <bool>[];
}

Future<(InkToolsController, _Sent)> _show(
  WidgetTester tester, {
  double width = 600,
  double textScale = 1,
}) async {
  asIPad();
  final controller = InkToolsController();
  addTearDown(controller.dispose);
  final sent = _Sent();
  await Harness(Workspace()).show(
    tester,
    size: const Size(1024, 768),
    home: MediaQuery.withClampedTextScaling(
      minScaleFactor: textScale,
      maxScaleFactor: textScale,
      child: Align(
        alignment: Alignment.topLeft,
        child: SizedBox(
          width: width,
          child: InkTools(
            controller: controller,
            onChoice: sent.choices.add,
            onUndo: () => sent.undos++,
            onRedo: () => sent.redos++,
            onFingerDraws: sent.fingers.add,
          ),
        ),
      ),
    ),
  );
  return (controller, sent);
}

Finder _key(String key) => find.byKey(ValueKey(key));

void main() {
  test('every tool and color has a name and a wire name', () {
    expect(DrawingTool.values.map((t) => t.wire), [
      'pen',
      'pencil',
      'marker',
      'eraser',
      'lasso',
    ]);
    for (final tool in DrawingTool.values) {
      expect(DrawingTool.of(tool.kind), tool);
      expect(tool.kind.name, tool.wire);
      expect(colorsOf(tool).isNotEmpty, tool.inks);
    }
    // Opaque, fixed colors: the same ink on every device and theme.
    for (final color in [...penColors, ...markerColors]) {
      expect(color.argb >> 24, 0xFF);
    }
  });

  testWidgets('a tool chosen is sent with its own last color and width', (
    tester,
  ) async {
    final (controller, sent) = await _show(tester);
    expect(
      controller.choice,
      const InkTool(kind: InkToolKind.pen, color: 0xFF1A1A1A, width: 3),
    );

    await tester.tap(_key('ink-tool-marker'));
    await tester.pump();
    expect(
      sent.choices.last,
      const InkTool(kind: InkToolKind.marker, color: 0xFFFFD60A, width: 12),
    );

    await tester.tap(_key('ink-tool-eraser'));
    await tester.pump();
    expect(
      sent.choices.last,
      const InkTool(kind: InkToolKind.eraser, color: null, width: 12),
    );
    // An eraser has no color to choose.
    expect(_key('ink-color'), findsNothing);

    await tester.tap(_key('ink-tool-pen'));
    await tester.pump();
    expect(
      sent.choices.last,
      const InkTool(kind: InkToolKind.pen, color: 0xFF1A1A1A, width: 3),
    );
  });

  testWidgets('the color popover changes the tool in hand only', (
    tester,
  ) async {
    final (controller, sent) = await _show(tester);
    expect(tester.getSemantics(_key('ink-color')).label, 'Color, Black');
    await tester.tap(_key('ink-color'));
    await tester.pumpAndSettle();
    await tester.tap(_key('ink-color-Red'));
    await tester.pumpAndSettle();
    expect(
      sent.choices.last,
      const InkTool(kind: InkToolKind.pen, color: 0xFFD63A2F, width: 3),
    );
    expect(tester.getSemantics(_key('ink-color')).label, 'Color, Red');

    // The pencil keeps its own.
    await tester.tap(_key('ink-tool-pencil'));
    await tester.pump();
    expect(sent.choices.last.color, 0xFF1A1A1A);
    await tester.tap(_key('ink-tool-pen'));
    await tester.pump();
    expect(sent.choices.last.color, 0xFFD63A2F);
    expect(controller.tool, DrawingTool.pen);
  });

  testWidgets('Undo and Redo act only when the surface says they can', (
    tester,
  ) async {
    final (controller, sent) = await _show(tester);
    bool enabled(String key) =>
        tester
            .getSemantics(_key(key))
            .getSemanticsData()
            .flagsCollection
            .isEnabled ==
        Tristate.isTrue;
    expect(enabled('ink-undo'), isFalse);
    await tester.tap(_key('ink-undo'));
    expect(sent.undos, 0);

    controller.undoState(canUndo: true, canRedo: false);
    await tester.pump();
    expect(enabled('ink-undo'), isTrue);
    expect(enabled('ink-redo'), isFalse);
    await tester.tap(_key('ink-undo'));
    await tester.tap(_key('ink-redo'));
    expect(sent.undos, 1);
    expect(sent.redos, 0);
  });

  testWidgets('a Pencil\'s double tap is followed, not sent back', (
    tester,
  ) async {
    final (controller, sent) = await _show(tester);
    // As ev_tool_changed.json: the eraser, 12 wide.
    controller.follow(const InkTool(kind: InkToolKind.eraser, width: 12));
    await tester.pump();
    expect(sent.choices, isEmpty);
    expect(
      tester
          .getSemantics(_key('ink-tool-eraser'))
          .getSemanticsData()
          .flagsCollection
          .isSelected,
      Tristate.isTrue,
    );
  });

  testWidgets('Draw with finger is in the bar, lit while on', (tester) async {
    final semantics = tester.ensureSemantics();
    final (controller, sent) = await _show(tester);
    bool lit() =>
        tester
            .getSemantics(find.bySemanticsLabel('Draw with finger'))
            .flagsCollection
            .isSelected ==
        Tristate.isTrue;
    expect(lit(), isFalse);
    await tester.tap(_key('ink-finger'));
    await tester.pumpAndSettle();
    expect(controller.fingerDraws, isTrue);
    expect(lit(), isTrue);
    await tester.tap(_key('ink-finger'));
    await tester.pumpAndSettle();
    expect(controller.fingerDraws, isFalse);
    expect(sent.fingers, [true, false]);
    semantics.dispose();
  });

  // Every control a whole finger's target inside the bar, none touching
  // another: at the narrowest pane, an iPad's narrowest window (375), just
  // short of the full bar and at it.
  const narrowKeys = [
    'ink-tool-ink',
    'ink-tool-eraser',
    'ink-tool-lasso',
    'ink-undo',
    'ink-redo',
    'ink-finger',
  ];
  const fullKeys = [
    'ink-tool-pen',
    'ink-tool-pencil',
    'ink-tool-marker',
    'ink-tool-eraser',
    'ink-tool-lasso',
    'ink-color',
    'ink-undo',
    'ink-redo',
    'ink-finger',
  ];
  for (final scale in [TextFactor.standard, TextFactor.ax5]) {
    for (final width in [
      320.0,
      375.0,
      inkToolsFullWidth - 1,
      inkToolsFullWidth,
    ]) {
      testWidgets('at $width pt and text size $scale every control is whole, '
          'a gap apart', (tester) async {
        await _show(tester, width: width, textScale: scale);
        expect(tester.takeException(), isNull);
        final narrow = width < inkToolsFullWidth;
        final keys = narrow ? narrowKeys : fullKeys;
        expect(_key('ink-tool-pen'), narrow ? findsNothing : findsOneWidget);
        final rects = [for (final key in keys) tester.getRect(_key(key))];
        for (final (i, rect) in rects.indexed) {
          expect(rect.left, greaterThanOrEqualTo(0), reason: keys[i]);
          expect(rect.right, lessThanOrEqualTo(width), reason: keys[i]);
          expect(rect.width, greaterThanOrEqualTo(44), reason: keys[i]);
          expect(rect.height, greaterThanOrEqualTo(44), reason: keys[i]);
          if (i > 0) {
            expect(
              rect.left - rects[i - 1].right,
              greaterThanOrEqualTo(4),
              reason: '${keys[i - 1]} to ${keys[i]}',
            );
          }
        }
      });
    }
  }

  testWidgets('in a narrow pane one button holds pen, pencil, highlighter '
      'and their colors', (tester) async {
    final (controller, sent) = await _show(tester, width: 375);
    expect(tester.getSemantics(_key('ink-tool-ink')).label, 'Pen, Black');

    // In hand: a tap offers the others and the colors.
    await tester.tap(_key('ink-tool-ink'));
    await tester.pumpAndSettle();
    await tester.tap(_key('ink-menu-pencil'));
    await tester.pumpAndSettle();
    expect(sent.choices.last.kind, InkToolKind.pencil);
    expect(tester.getSemantics(_key('ink-tool-ink')).label, 'Pencil, Black');

    await tester.tap(_key('ink-tool-ink'));
    await tester.pumpAndSettle();
    await tester.tap(_key('ink-color-Red'));
    await tester.pumpAndSettle();
    expect(
      sent.choices.last,
      const InkTool(kind: InkToolKind.pencil, color: 0xFFD63A2F, width: 2),
    );

    // Not in hand: a tap takes the last one back, with no menu.
    await tester.tap(_key('ink-tool-eraser'));
    await tester.pump();
    expect(controller.tool, DrawingTool.eraser);
    await tester.tap(_key('ink-tool-ink'));
    await tester.pumpAndSettle();
    expect(controller.tool, DrawingTool.pencil);
    expect(_key('ink-menu-pen'), findsNothing);
    expect(sent.choices.last.color, 0xFFD63A2F);
  });
}
