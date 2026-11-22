// A dialog over the window (showModal) with the on-screen keyboard up, as
// on an iPad with no keyboard attached (Sorbet): centred in what the
// keyboard leaves, its fields scrolling when that's short, its buttons in
// sight.
import 'package:factory_app/agent_view.dart' show AgentView;
import 'package:factory_app/controls.dart';
import 'package:factory_app/forms.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_test.dart' show workspace;

Future<void> _show(WidgetTester tester, {int fields = 1}) async {
  final harness = Harness(workspace());
  // An iPad in landscape.
  await harness.show(tester, size: const Size(1180, 820));
  await pumpFrames(tester);
  showModal(
    tester.element(find.byType(AgentView)),
    (context, close) => FormFrame(
      title: 'Confirm this iPad is yours',
      body: 'We send a code to your email.',
      fields: [
        for (var i = 0; i < fields; i++)
          Field(
            label: 'Email $i',
            child: TextField(controller: TextEditingController()),
          ),
      ],
      error: null,
      submitLabel: 'Send code',
      onSubmit: () {},
      onCancel: close,
    ),
  );
  await pumpFrames(tester);
}

void main() {
  testWidgets('the keyboard up: the dialog sits above it, Send code in '
      'sight', (tester) async {
    await _show(tester);
    final before = tester.getRect(find.text('Send code'));
    tester.view.viewInsets = const FakeViewPadding(bottom: 400);
    addTearDown(tester.view.resetViewInsets);
    await pumpFrames(tester);
    final send = tester.getRect(find.text('Send code'));
    expect(send.bottom, lessThan(820 - 400));
    expect(send.top, lessThan(before.top));
    // Centred in what's left.
    final title = tester.getRect(find.text('Confirm this iPad is yours'));
    expect(title.top, greaterThan(0));
  });

  testWidgets('a long form with the keyboard up scrolls its fields; its '
      'title and buttons stay in sight', (tester) async {
    await _show(tester, fields: 12);
    tester.view.viewInsets = const FakeViewPadding(bottom: 430);
    addTearDown(tester.view.resetViewInsets);
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    expect(tester.getRect(find.text('Send code')).bottom, lessThan(820 - 430));
    expect(
      tester.getRect(find.text('Confirm this iPad is yours')).top,
      greaterThanOrEqualTo(0),
    );
    await tester.dragUntilVisible(
      find.text('Email 11'),
      find.text('Email 0'),
      const Offset(0, -40),
    );
    expect(find.text('Email 11'), findsOneWidget);
  });

  testWidgets('no keyboard (a Mac): where it always was', (tester) async {
    await _show(tester);
    final rect = tester.getRect(find.text('Confirm this iPad is yours'));
    final dialog = tester.getRect(find.text('Send code'));
    // Centred in the whole window.
    expect((rect.top + dialog.bottom) / 2, closeTo(820 / 2, 60));
  });

  // Something under it had the keys (the iPad's paired and full tiers, the
  // message box): the dialog takes them all the same, and Esc closes it
  // (Sorbet: the sign-in dialog stayed, and Esc found no action).
  testWidgets('Esc closes a dialog with no field, whatever had the keys', (
    tester,
  ) async {
    final harness = Harness(workspace());
    await harness.show(tester, size: const Size(1180, 820));
    await pumpFrames(tester);
    await tester.tap(find.byType(EditableText).first);
    await pumpFrames(tester);
    expect(FocusManager.instance.primaryFocus, isNot(isNull));
    // A dialog of words and buttons, as the sign-in one is.
    showModal(
      tester.element(find.byType(AgentView)),
      (context, close) => Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          const Text('Sign in on Studio?'),
          Button(label: 'Not now', onPressed: close),
        ],
      ),
    );
    await pumpFrames(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    expect(find.text('Sign in on Studio?'), findsNothing);
  });
}
