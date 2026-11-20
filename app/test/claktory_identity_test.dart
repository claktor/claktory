import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace snapshot() => Workspace(
  agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
  selectedAgentId: 'rivet',
  settings: Settings(userName: 'Matt', name: 'Studio', color: 'teal'),
  remote: RemoteAccess(machine: 'Mac Studio'),
);

void main() {
  testWidgets("Settings → General changes the Claktory's name and color", (
    tester,
  ) async {
    final harness = Harness(snapshot());
    harness.page.value = AppPage.settings;
    await harness.show(tester);
    await pumpFrames(tester);
    expect(find.text('Claktory name'), findsOneWidget);
    expect(
      find.text('Shown on every device connected to this Claktory.'),
      findsOneWidget,
    );
    final field = find
        .byWidgetPredicate(
          (widget) =>
              widget is EditableText && widget.controller.text == 'Studio',
        )
        .first;
    final controller = tester.widget<EditableText>(field).controller;
    await tester.enterText(field, ' Office ');
    await tester.pump();
    expect(
      find.text('Shown on every device connected to this Claktory.'),
      findsNothing,
    );
    await tester.tap(find.text('Save').first);
    await pumpFrames(tester);
    expect(harness.last.updateSettings, UpdateSettings(name: 'Office'));

    // Emptied, it goes back to the computer's name, shown as the hint.
    controller.text = '';
    await tester.pump();
    expect(find.text('Mac Studio'), findsOneWidget);

    final teal = tester.widget<Semantics>(
      find.byWidgetPredicate(
        (widget) => widget is Semantics && widget.properties.label == 'teal',
      ),
    );
    expect(teal.properties.selected, isTrue, reason: 'its color is chosen');
    await tester.tap(find.bySemanticsLabel('amber'));
    await pumpFrames(tester);
    expect(harness.last.updateSettings, UpdateSettings(color: 'amber'));
  });
}
