import 'package:factory_app/name_step.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_test.dart' show phone;

Workspace snapshot({String name = ''}) => Workspace(
  agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
  selectedAgentId: 'rivet',
  settings: Settings(userName: name),
);

void main() {
  testWidgets('the first launch asks the name, over everything, and saves it', (
    tester,
  ) async {
    final harness = Harness(snapshot(), named: false);
    await harness.show(tester);
    await pumpFrames(tester);
    expect(find.byType(NameStep), findsOneWidget);
    expect(find.text("What's your name?"), findsOneWidget);
    // Esc doesn't get past it.
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester);
    expect(find.byType(NameStep), findsOneWidget);

    await tester.enterText(
      find.descendant(
        of: find.byType(NameStep),
        matching: find.byType(EditableText),
      ),
      '  Matt ',
    );
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await pumpFrames(tester);
    expect(harness.last.updateSettings.userName, 'Matt');

    // It goes once the backend has the name.
    await harness.apply(tester, snapshot(name: 'Matt'));
    await pumpFrames(tester);
    expect(find.byType(NameStep), findsNothing);
  });

  testWidgets('nothing is asked before the backend answers, or with a name', (
    tester,
  ) async {
    final harness = Harness(snapshot(name: 'Matt'), named: false);
    await harness.show(tester);
    await pumpFrames(tester);
    expect(find.byType(NameStep), findsNothing);
  });

  testWidgets('a phone not yet connected is asked to connect, not its name', (
    tester,
  ) async {
    final harness = Harness(Workspace(), named: false, localBackend: false);
    harness.backend.connected.value = false;
    harness.backend.connectTo(null);
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    expect(find.byType(NameStep), findsNothing);
    expect(find.text('Connect to your Claktory'), findsOneWidget);
  });

  testWidgets('a refusal shows on the step', (tester) async {
    final harness = Harness(snapshot(), named: false);
    harness.backend.refuse = 'The settings file has an error.';
    await harness.show(tester);
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, 'Matt');
    await tester.pump();
    await tester.tap(find.text('Continue'));
    await pumpFrames(tester);
    expect(find.text('The settings file has an error.'), findsOneWidget);
    expect(find.byType(NameStep), findsOneWidget);
  });

  testWidgets('on a phone too', (tester) async {
    final harness = Harness(snapshot(), named: false);
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    expect(find.text("What's your name?"), findsOneWidget);
  });

  testWidgets('Settings → General changes it', (tester) async {
    final harness = Harness(snapshot(name: 'Matt'));
    harness.page.value = AppPage.settings;
    await harness.show(tester);
    await pumpFrames(tester);
    expect(find.text('Your name'), findsOneWidget);
    final saves = find.text('Save').evaluate().length;
    final field = find
        .byWidgetPredicate(
          (widget) =>
              widget is EditableText && widget.controller.text == 'Matt',
        )
        .first;
    await tester.enterText(field, 'Mateusz');
    await tester.pump();
    expect(find.text('Save').evaluate().length, saves + 1);
    await tester.tap(find.text('Save').first);
    await pumpFrames(tester);
    expect(harness.last.updateSettings.userName, 'Mateusz');
  });
}
