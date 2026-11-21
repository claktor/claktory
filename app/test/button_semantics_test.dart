import 'dart:ui' show Tristate;

import 'package:factory_app/controls.dart';
import 'package:flutter/rendering.dart' show SemanticsAction, SemanticsNode;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

Widget _app(Widget child) => WidgetsApp(
  color: const Color(0xFF000000),
  builder: (context, _) => Center(child: child),
);

void main() {
  testWidgets('a Button is a button to a screen reader, with its label, '
      'enabled or not', (tester) async {
    final handle = tester.ensureSemantics();
    await tester.pumpWidget(
      _app(
        Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Button(label: 'Approve', onPressed: () {}),
            const Button(label: 'Connect', onPressed: null),
          ],
        ),
      ),
    );
    expect(
      tester.getSemantics(find.text('Approve')),
      matchesSemantics(
        label: 'Approve',
        isButton: true,
        hasEnabledState: true,
        isEnabled: true,
        isFocusable: true,
        hasTapAction: true,
        hasFocusAction: true,
      ),
    );
    expect(
      tester.getSemantics(find.text('Connect')),
      matchesSemantics(
        label: 'Connect',
        isButton: true,
        hasEnabledState: true,
        isEnabled: false,
      ),
    );
    handle.dispose();
  });

  testWidgets("a TextField with a label says what it is, typed in or not, "
      'once, keeping its text actions', (tester) async {
    final handle = tester.ensureSemantics();
    final controller = TextEditingController();
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      _app(
        SizedBox(
          width: 300,
          child: TextField(
            controller: controller,
            placeholder: 'Message Mike',
            semanticsLabel: 'Message Mike',
          ),
        ),
      ),
    );
    final field = find.byType(EditableText);
    SemanticsNode node() => tester.getSemantics(field);
    expect(node().label, 'Message Mike');
    expect(node().flagsCollection.isTextField, isTrue);
    expect(find.bySemanticsLabel('Message Mike'), findsOneWidget);

    // A screen reader's double tap focuses it, with its text actions.
    expect(node().getSemanticsData().hasAction(SemanticsAction.tap), isTrue);
    tester.semantics.tap(find.semantics.byLabel('Message Mike'));
    await tester.pump();
    await tester.pump();
    expect(
      tester.state<EditableTextState>(field).widget.focusNode.hasFocus,
      isTrue,
    );
    expect(
      node().getSemanticsData().hasAction(SemanticsAction.setText),
      isTrue,
    );

    await tester.enterText(field, 'hello');
    await tester.pump();
    expect(node().label, 'Message Mike');
    expect(node().value, 'hello');
    final data = node().getSemanticsData();
    expect(data.hasAction(SemanticsAction.setText), isTrue);
    expect(data.hasAction(SemanticsAction.setSelection), isTrue);
    expect(find.bySemanticsLabel('Message Mike'), findsOneWidget);
    handle.dispose();
  });

  testWidgets('a bordered field is one element as big as its box, and a '
      'tap on its padding focuses it', (tester) async {
    final handle = tester.ensureSemantics();
    final controller = TextEditingController(text: 'eu-west-1');
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      _app(
        SizedBox(
          width: 300,
          child: TextField(
            controller: controller,
            semanticsLabel: 'Region',
            bordered: true,
          ),
        ),
      ),
    );
    final box = tester.getRect(find.byType(TextField));
    final node = tester.getSemantics(find.byType(TextField));
    expect(node.label, 'Region');
    expect(node.value, 'eu-west-1');
    expect(node.flagsCollection.isTextField, isTrue);
    expect(node.rect.size, box.size);
    expect(node.getSemanticsData().hasAction(SemanticsAction.tap), isTrue);

    final field = find.byType(EditableText);
    await tester.tapAt(box.topLeft + const Offset(3, 3));
    await tester.pump();
    expect(
      tester.state<EditableTextState>(field).widget.focusNode.hasFocus,
      isTrue,
    );
    handle.dispose();
  });

  testWidgets("a field without a border is its owner's TextFieldBox: one "
      'element as big as the box, focused by a tap anywhere in it', (
    tester,
  ) async {
    final handle = tester.ensureSemantics();
    final controller = TextEditingController();
    final focus = FocusNode();
    addTearDown(controller.dispose);
    addTearDown(focus.dispose);
    await tester.pumpWidget(
      _app(
        TextFieldBox(
          focusNode: focus,
          child: Container(
            width: 300,
            constraints: const BoxConstraints(minHeight: 48),
            padding: const EdgeInsets.all(12),
            child: TextField(
              controller: controller,
              focusNode: focus,
              bordered: false,
              placeholder: 'Search',
              semanticsLabel: 'Search',
            ),
          ),
        ),
      ),
    );
    final box = tester.getRect(find.byType(TextFieldBox));
    final node = tester.getSemantics(find.byType(TextFieldBox));
    expect(node.label, 'Search');
    expect(node.flagsCollection.isTextField, isTrue);
    expect(node.rect.size, box.size);
    expect(node.getSemanticsData().hasAction(SemanticsAction.tap), isTrue);

    tester.semantics.tap(find.semantics.byLabel('Search'));
    await tester.pump();
    expect(focus.hasFocus, isTrue);
    focus.unfocus();
    await tester.pump();
    await tester.tapAt(box.topLeft + const Offset(3, 3));
    await tester.pump();
    expect(focus.hasFocus, isTrue);
    handle.dispose();
  });

  testWidgets('focused with its caret already placed (a draft at its start), '
      'a field tells a screen reader at once that it can be edited', (
    tester,
  ) async {
    final handle = tester.ensureSemantics();
    final controller = TextEditingController.fromValue(
      const TextEditingValue(selection: TextSelection.collapsed(offset: 0)),
    );
    final focus = FocusNode();
    addTearDown(controller.dispose);
    addTearDown(focus.dispose);
    await tester.pumpWidget(
      _app(
        SizedBox(
          width: 300,
          child: TextField(
            controller: controller,
            focusNode: focus,
            bordered: false,
            semanticsLabel: 'Message Mike',
          ),
        ),
      ),
    );
    focus.requestFocus();
    await tester.pump();
    await tester.pump();
    final data = tester
        .getSemantics(find.byType(EditableText))
        .getSemanticsData();
    expect(data.flagsCollection.isFocused, Tristate.isTrue);
    expect(data.hasAction(SemanticsAction.setText), isTrue);
    handle.dispose();
  });
}
