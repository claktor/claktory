// The emailed sign-in code as six digit boxes: one field underneath,
// so typing fills the boxes from the left, Delete empties them from the
// right, pasting the code fills all six and sends it, a paste with more or
// other than six digits keeps the first six digits, a screen reader finds
// one field, and on a 320-point phone at the largest text the six still fit
// on one line, each a finger tall.
import 'package:factory_app/account.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/phone.dart' show PairScreen;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/your_claktories.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'account_test.dart' show awaiting, showAccount;
import 'device_confirm_test.dart' show FakeAccountApi;
import 'harness.dart';
import 'pair_start.dart';

final _mac = TargetPlatformVariant.only(TargetPlatform.macOS);

Finder _box(int i) => find.byKey(ValueKey('code-box-$i'));

final _editable = find.descendant(
  of: find.byType(CodeBoxes),
  matching: find.byType(EditableText),
);

/// The digit each box shows, '' for an empty one.
List<String> _shown(WidgetTester tester) => [
  for (var i = 0; i < codeLength; i++)
    tester
            .widgetList<Text>(
              find.descendant(of: _box(i), matching: find.byType(Text)),
            )
            .firstOrNull
            ?.data ??
        '',
];

/// The box with the signal border, the one the next digit goes in.
int? _next(WidgetTester tester) {
  final signal = Palette.of(tester.element(_box(0))).signal;
  for (var i = 0; i < codeLength; i++) {
    final box = tester.widget<Container>(_box(i));
    final border = (box.decoration! as BoxDecoration).border! as Border;
    if (border.top.color == signal) return i;
  }
  return null;
}

/// Types [digits] one at a time, as a keyboard does.
Future<void> _type(WidgetTester tester, String digits) async {
  for (final digit in digits.split('')) {
    final value = tester.state<EditableTextState>(_editable).textEditingValue;
    final text = value.text + digit;
    tester.testTextInput.updateEditingValue(
      TextEditingValue(
        text: text,
        selection: TextSelection.collapsed(offset: text.length),
      ),
    );
    await tester.pump();
  }
}

/// The clipboard holds [text].
void _clipboard(WidgetTester tester, String text) {
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    SystemChannels.platform,
    (call) async => switch (call.method) {
      'Clipboard.getData' => {'text': text},
      'Clipboard.hasStrings' => {'value': true},
      _ => null,
    },
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      null,
    ),
  );
}

/// ⌘V.
Future<void> _paste(WidgetTester tester) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
  await tester.sendKeyEvent(LogicalKeyboardKey.keyV);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
  await pumpFrames(tester);
}

void main() {
  testWidgets('typing fills the boxes from the left, the next one marked, '
      'and the sixth digit sends the code', (tester) async {
    final harness = await showAccount(tester, awaiting);
    expect(find.byType(CodeBoxes), findsOneWidget);
    expect(_editable, findsOneWidget, reason: 'one field, not six');
    expect(_shown(tester), ['', '', '', '', '', '']);
    expect(_next(tester), 0, reason: 'focused at once');

    await _type(tester, '12');
    expect(_shown(tester), ['1', '2', '', '', '', '']);
    expect(_next(tester), 2);
    expect(harness.backend.sent, isEmpty);

    await _type(tester, '3456');
    expect(_shown(tester), ['1', '2', '3', '4', '5', '6']);
    expect(_next(tester), 5, reason: 'full: the last box');
    expect(harness.last.accountVerify.code, '123456');
  }, variant: _mac);

  testWidgets('Delete empties the boxes from the right', (tester) async {
    await showAccount(tester, awaiting);
    await _type(tester, '1234');
    await tester.sendKeyEvent(LogicalKeyboardKey.backspace);
    await tester.pump();
    expect(_shown(tester), ['1', '2', '3', '', '', '']);
    expect(_next(tester), 3);
    await tester.sendKeyEvent(LogicalKeyboardKey.backspace);
    await tester.sendKeyEvent(LogicalKeyboardKey.backspace);
    await tester.pump();
    expect(_shown(tester), ['1', '', '', '', '', '']);
    expect(_next(tester), 1);
  }, variant: _mac);

  testWidgets('a click on any box focuses the field', (tester) async {
    await showAccount(tester, awaiting);
    final node = tester.widget<EditableText>(_editable).focusNode;
    node.unfocus();
    await pumpFrames(tester);
    expect(_next(tester), isNull, reason: 'no box marked unfocused');
    await tester.tap(_box(4));
    await pumpFrames(tester);
    expect(node.hasFocus, isTrue);
    expect(_next(tester), 0);
  }, variant: _mac);

  testWidgets('pasting the code fills all six and sends it', (tester) async {
    final harness = await showAccount(tester, awaiting);
    _clipboard(tester, '482913');
    await _paste(tester);
    expect(_shown(tester), ['4', '8', '2', '9', '1', '3']);
    expect(harness.last.accountVerify.code, '482913');
  }, variant: _mac);

  testWidgets('a paste keeps only its digits, the first six', (tester) async {
    final harness = await showAccount(tester, awaiting);
    // Too few: the boxes it fills, nothing sent.
    _clipboard(tester, '48 29');
    await _paste(tester);
    expect(_shown(tester), ['4', '8', '2', '9', '', '']);
    expect(_next(tester), 4);
    expect(harness.backend.sent, isEmpty);
    tester.state<EditableTextState>(_editable).widget.controller.clear();
    await tester.pump();

    // No digits at all: nothing.
    _clipboard(tester, 'code');
    await _paste(tester);
    expect(_shown(tester), ['', '', '', '', '', '']);
    expect(harness.backend.sent, isEmpty);

    // Words, spaces and a seventh digit: the first six digits, sent.
    _clipboard(tester, 'Your code: 482 913 7');
    await _paste(tester);
    expect(_shown(tester), ['4', '8', '2', '9', '1', '3']);
    expect(harness.last.accountVerify.code, '482913');
  }, variant: _mac);

  testWidgets('a screen reader finds one field, "Code from your email", with '
      'the digits typed', (tester) async {
    final semantics = tester.ensureSemantics();
    await showAccount(tester, awaiting);
    await _type(tester, '123');
    expect(
      tester.getSemantics(_editable),
      isSemantics(
        label: 'Code from your email',
        value: '123',
        isTextField: true,
        isFocused: true,
      ),
    );
    expect(find.bySemanticsLabel('1'), findsNothing, reason: 'no box speaks');
    semantics.dispose();
  }, variant: _mac);

  testWidgets('on a 320-point phone at the largest text the six boxes fit '
      'one line, each a finger tall, and take the code', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final api = FakeAccountApi();
    final account = DeviceAccount(key: 'k' * 64, open: (_, _) => api);
    final harness = Harness(Workspace(), localBackend: false, account: account);
    await harness.show(tester, size: const Size(320, 568));
    await pumpFrames(tester);
    expect(find.byType(PairScreen), findsOneWidget);
    // The first screen's own way in.
    await pastPairStart(tester, email: true);
    await pumpFrames(tester);
    await tester.enterText(
      find.byKey(const ValueKey('device-email')),
      'matt@example.com',
    );
    await pumpFrames(tester);
    await tester.ensureVisible(find.text('Send code'));
    await pumpFrames(tester);
    await tester.tap(find.text('Send code'));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);

    final form = tester.getRect(find.byType(DeviceSignInForm));
    final top = tester.getTopLeft(_box(0)).dy;
    for (var i = 0; i < codeLength; i++) {
      final box = tester.getRect(_box(i));
      expect(box.top, top, reason: 'one line');
      expect(box.height, greaterThanOrEqualTo(44));
      expect(box.left, greaterThanOrEqualTo(form.left));
      expect(box.right, lessThanOrEqualTo(form.right));
    }
    final code = tester.widget<EditableText>(_editable);
    expect(code.keyboardType, TextInputType.number);
    expect(code.autofillHints, contains(AutofillHints.oneTimeCode));
    expect(code.focusNode.hasFocus, isTrue, reason: 'the keyboard comes up');

    // The code the keyboard offers from Mail or Messages, all at once.
    tester.testTextInput.updateEditingValue(
      const TextEditingValue(
        text: '123456',
        selection: TextSelection.collapsed(offset: 6),
      ),
    );
    await pumpFrames(tester);
    expect(account.signedIn, isTrue);
    expect(tester.takeException(), isNull);
  });
}
