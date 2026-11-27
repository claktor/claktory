// Sending from an iPad's on-screen keyboard: its
// Return arrives as text and adds a line, so with no hardware keyboard the
// message box has Send, as on an iPhone. A hardware keyboard sends with
// Return, as in Messages, and Send steps aside; the draft and the focus
// stay through the keyboard coming and going. At every width: a narrow
// Split View lays the box out as a phone's, and Return still sends from a
// hardware keyboard.
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/mobile/device.dart' show hardwareKeyboard;
import 'package:factory_app/shell.dart';
import 'package:flutter/services.dart' show LogicalKeyboardKey;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show leaf, snapshot;
import 'as_ipad.dart';

/// An iPad Pro 13-inch window, full width, landscape.
const _wide = Size(1376, 1032);

Finder get _send =>
    find.ancestor(of: find.text('Send'), matching: find.byType(Button));

Future<Harness> _show(
  WidgetTester tester, {
  bool iPad = true,
  Size size = _wide,
}) async {
  if (iPad) asIPad();
  addTearDown(() => hardwareKeyboard.value = false);
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  final harness = Harness(snapshot(layout: leaf('chat', 'chat')));
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(
      focus: focus,
      child: const Shell(rail: SizedBox(width: 56)),
    ),
  );
  await pumpFrames(tester);
  return harness;
}

Future<void> _type(WidgetTester tester, String text) async {
  await tester.tap(find.byType(EditableText));
  await tester.pump();
  await tester.enterText(find.byType(EditableText), text);
  await tester.pump();
}

bool _focused(WidgetTester tester) =>
    tester.widget<EditableText>(find.byType(EditableText)).focusNode.hasFocus;

String _draft(WidgetTester tester) =>
    tester.widget<EditableText>(find.byType(EditableText)).controller.text;

void main() {
  testWidgets('with only the on-screen keyboard, an iPad sends with Send', (
    tester,
  ) async {
    final harness = await _show(tester);
    expect(_send, findsOneWidget);
    expect(tester.widget<Button>(_send).onPressed, isNull);
    await _type(tester, 'Still there iPad?');
    expect(tester.widget<Button>(_send).onPressed, isNotNull);
    await tester.tap(_send);
    await pumpFrames(tester);
    expect(harness.last.sendMessage.text, 'Still there iPad?');
    expect(_draft(tester), isEmpty);
  });

  testWidgets('a hardware keyboard coming and going keeps the draft and the '
      'focus; with it Return sends and Send steps aside', (tester) async {
    final harness = await _show(tester);
    await _type(tester, 'Typed on glass');
    expect(_focused(tester), isTrue);

    hardwareKeyboard.value = true;
    await pumpFrames(tester);
    expect(_send, findsNothing);
    expect(_draft(tester), 'Typed on glass');
    expect(_focused(tester), isTrue);

    hardwareKeyboard.value = false;
    await pumpFrames(tester);
    expect(_send, findsOneWidget);
    expect(_draft(tester), 'Typed on glass');
    expect(_focused(tester), isTrue);

    hardwareKeyboard.value = true;
    await pumpFrames(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(harness.last.sendMessage.text, 'Typed on glass');
  });

  testWidgets("a Mac's box has no Send: Return sends", (tester) async {
    await _show(tester, iPad: false);
    expect(_send, findsNothing);
  });

  testWidgets('in a narrow Split View, a hardware keyboard sends with '
      'Return; the on-screen one adds a line', (tester) async {
    // A third of an iPad Pro 13 in landscape: the phone's layout.
    final harness = await _show(tester, size: const Size(507, 1032));
    await _type(tester, 'From the narrow window');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);

    hardwareKeyboard.value = true;
    await pumpFrames(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(harness.last.sendMessage.text, 'From the narrow window');
  });
}
