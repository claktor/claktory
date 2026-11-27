// A project's row on an iPad: its ⋯ by a finger opens the menu as a
// popover with a finger's rows; a trackpad's or mouse's right click opens it
// where the pointer is, as on a Mac; and a pointer over the row tints it,
// after a finger's touch too.
import 'package:factory_app/identity.dart' show Palette;
import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'as_ipad.dart';

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Site')],
  agents: [
    Agent(id: 'june', name: 'June', projectId: 'p1', modelAvailable: true),
  ],
  selectedAgentId: 'june',
);

Future<Harness> _show(WidgetTester tester) async {
  asIPad();
  final harness = Harness(_workspace());
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  // The window as an iPad's is: the workbench, beside the rail.
  await harness.show(
    tester,
    size: const Size(1194, 834),
    home: WorkbenchScope(
      focus: focus,
      child: const Shell(rail: SizedBox(width: 56)),
    ),
  );
  await pumpFrames(tester);
  return harness;
}

/// The project row's own box, whose color is its hover tint.
BoxDecoration? _rowDecoration(WidgetTester tester) {
  final box = tester
      .widgetList<AnimatedContainer>(
        find.ancestor(
          of: find.text('Site'),
          matching: find.byType(AnimatedContainer),
        ),
      )
      .first;
  return box.decoration as BoxDecoration?;
}

void main() {
  testWidgets("by a finger, ⋯'s menu has a finger's rows", (tester) async {
    await _show(tester);
    await tester.tap(find.bySemanticsLabel('Site options'));
    await pumpFrames(tester);
    // Each row a finger's height: the next one starts that far below.
    final rows = ['Files', 'Add an agent…', 'Connect a pipe…'];
    for (var i = 1; i < rows.length; i++) {
      final above = tester.getCenter(find.text(rows[i - 1]).last).dy;
      final below = tester.getCenter(find.text(rows[i]).last).dy;
      expect(below - above, greaterThanOrEqualTo(44), reason: rows[i]);
    }
  });

  testWidgets('a trackpad\'s right click opens the menu at the pointer', (
    tester,
  ) async {
    await _show(tester);
    final at = tester.getCenter(find.text('Site')) + const Offset(20, 0);
    await tester.tapAt(
      at,
      buttons: kSecondaryButton,
      kind: PointerDeviceKind.mouse,
    );
    await pumpFrames(tester);
    final first = tester.getTopLeft(find.text('Files'));
    expect((first.dy - at.dy).abs(), lessThan(40), reason: 'by the pointer');
    expect(first.dx, greaterThan(at.dx - 1));
  });

  testWidgets('a pointer over the row tints it, as on a Mac', (tester) async {
    await _show(tester);
    final palette = Palette.of(tester.element(find.text('Site')));
    expect(_rowDecoration(tester)?.color, isNot(palette.hover));
    // A finger first, as an iPad is mostly used: the pointer afterwards
    // still shows hover.
    await tester.tapAt(const Offset(900, 400));
    await pumpFrames(tester);
    expect(FocusManager.instance.highlightMode, FocusHighlightMode.touch);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    final row = tester.getCenter(find.text('Site'));
    await mouse.addPointer(location: row - const Offset(0, 4));
    addTearDown(mouse.removePointer);
    await mouse.moveTo(row);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 300));
    expect(_rowDecoration(tester)?.color, palette.hover);
  });
}
