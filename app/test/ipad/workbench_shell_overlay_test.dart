// The sidebar over the window in the real Shell (the simulator pass found
// Esc and a tap beside it leaving it open).
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/shell.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../phone_test.dart' show workspace;
import 'as_ipad.dart';

Widget _window(WorkbenchFocus focus) => WorkbenchScope(
  focus: focus,
  child: const Shell(rail: SizedBox(width: 56)),
);

Future<WorkbenchFocus> _open(WidgetTester tester) async {
  asIPad();
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  final harness = Harness(workspace());
  await harness.show(tester, size: const Size(744, 1133), home: _window(focus));
  await pumpFrames(tester);
  await tester.tap(find.byKey(const ValueKey('workbench-sidebar')));
  await pumpFrames(tester);
  expect(focus.overlayOpen, isTrue);
  return focus;
}

void main() {
  testWidgets('Esc closes it', (tester) async {
    final focus = await _open(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester);
    expect(focus.overlayOpen, isFalse);
  });

  testWidgets('Esc and ⌃⌘S reach the workbench when nothing in it has the '
      'keyboard (the message box let go of it)', (tester) async {
    final focus = await _open(tester);
    FocusManager.instance.primaryFocus?.unfocus();
    await pumpFrames(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester);
    expect(focus.overlayOpen, isFalse);

    // Let go again, as a tap that dismisses the keyboard does.
    FocusManager.instance.primaryFocus?.unfocus();
    await pumpFrames(tester);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.control);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyS);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.control);
    await pumpFrames(tester);
    expect(focus.overlayOpen, isTrue);
  });

  testWidgets('a tap beside it closes it', (tester) async {
    final focus = await _open(tester);
    await tester.tapAt(const Offset(700, 600));
    await pumpFrames(tester);
    expect(focus.overlayOpen, isFalse);
  });
}
