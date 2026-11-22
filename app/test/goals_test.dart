// Goals on a checklist (#270): the person's outcomes above the items, each
// with how far its items have come, added, edited, finished, dropped and
// removed in the app; an item says which goal it serves. And an extension
// option that's one of a few (the checklist's check-in), in agent settings.
import 'package:factory_app/controls.dart' show Select;
import 'package:factory_app/extensions.dart' show goalLine, goalProgress;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'extensions_test.dart'
    show
        at,
        checklistExtension,
        checklistView,
        glyphButton,
        now,
        showWith,
        workspace;
import 'app_test.dart' as app show builder, workspace;
import 'harness.dart';

const _android = 'Get the Android app to a link I can share';

ExtensionView _withGoals() {
  final view = checklistView();
  view.checklist.goals.addAll([
    Goal(
      id: 1,
      text: _android,
      status: ChecklistStatus.CHECKLIST_STATUS_OPEN,
      created: at(now),
      log: [ChecklistUpdate(time: at(now), text: 'Added')],
    ),
    Goal(
      id: 2,
      text: 'Ship the site',
      status: ChecklistStatus.CHECKLIST_STATUS_DONE,
      created: at(now),
    ),
  ]);
  // #2 (in progress) and #3 (open) serve G1; #1 (done) too.
  for (final item in view.checklist.items) {
    item.goal = 1;
  }
  return view;
}

Future<Harness> _panel(WidgetTester tester, ExtensionView view) async {
  final harness = await showWith(tester, workspace(openPanel: 'checklist'));
  harness.backend.extensionView.apply(view);
  await settle(tester);
  return harness;
}

void main() {
  test('a goal on its items, and how far they have come', () {
    expect(goalLine('Ship it'), 'Goal: Ship it');
    expect(goalLine(_android), 'Goal: Get the Android app to a…');
    final goal = Goal(id: 1);
    expect(goalProgress(goal, []), 'No items yet');
    expect(
      goalProgress(goal, [
        ChecklistItem(goal: 1, status: ChecklistStatus.CHECKLIST_STATUS_DONE),
        ChecklistItem(goal: 1),
        ChecklistItem(
          goal: 1,
          status: ChecklistStatus.CHECKLIST_STATUS_DROPPED,
        ),
        ChecklistItem(goal: 2),
      ]),
      '1 of 2 items done',
    );
  });

  testWidgets('the open goals above the items, the finished folded, and '
      'each item says its goal', (tester) async {
    await _panel(tester, _withGoals());
    expect(find.text('Goals'), findsOneWidget);
    expect(find.text(_android), findsOneWidget);
    expect(find.text('1 of 3 items done'), findsOneWidget);
    expect(find.text('Ship the site'), findsNothing);
    expect(
      tester.getTopLeft(find.text(_android)).dy,
      lessThan(tester.getTopLeft(find.text('Usage circles')).dy),
    );
    expect(find.text('Goal: Get the Android app to a…'), findsNWidgets(3));

    await tester.tap(find.text('1 finished'));
    await settle(tester);
    expect(find.text('Ship the site'), findsOneWidget);
    expect(find.text('No items yet'), findsOneWidget);
  });

  testWidgets('Add goal… adds one in your words', (tester) async {
    final harness = await _panel(tester, checklistView());
    expect(find.text('Goals'), findsNothing, reason: 'none yet');
    await tester.tap(find.text('Add goal…'));
    await settle(tester);
    expect(find.text('Add a goal'), findsOneWidget);
    await tester.enterText(
      find
          .descendant(
            of: find.byType(Overlay),
            matching: find.byType(EditableText),
          )
          .last,
      '  $_android ',
    );
    await tester.tap(find.text('Add'));
    await settle(tester);
    final added = harness.last.addGoal;
    expect(added.agentId, 'a1');
    expect(added.text, _android);
  });

  testWidgets('an empty checklist still offers Add goal…', (tester) async {
    await _panel(
      tester,
      ExtensionView(
        extensionId: 'checklist',
        agentId: 'a1',
        title: 'Checklist',
        checklist: Checklist(),
      ),
    );
    expect(find.text('Nothing on the checklist yet.'), findsOneWidget);
    expect(find.text('Add goal…'), findsOneWidget);
  });

  testWidgets("a goal's ⋯ finishes it, and Remove asks first", (tester) async {
    final harness = await _panel(tester, _withGoals());
    await tester.tap(glyphButton('More for $_android'));
    await settle(tester);
    await tester.tap(find.text('Mark done'));
    await settle(tester);
    var update = harness.last.updateGoal;
    expect(update.goalId, 1);
    expect(update.status, ChecklistStatus.CHECKLIST_STATUS_DONE);
    expect(update.hasText(), isFalse);
    expect(update.remove, isFalse);

    await tester.tap(glyphButton('More for $_android'));
    await settle(tester);
    await tester.tap(find.text('Remove…'));
    await settle(tester);
    expect(find.text('Remove this goal?'), findsOneWidget);
    expect(find.text('Its items stay on the checklist.'), findsOneWidget);
    await tester.tap(find.text('Remove'));
    await settle(tester);
    update = harness.last.updateGoal;
    expect(update.remove, isTrue);
    expect(update.status, ChecklistStatus.CHECKLIST_STATUS_UNSPECIFIED);
  });

  testWidgets('Edit… changes its words; Reopen a finished one', (tester) async {
    final harness = await _panel(tester, _withGoals());
    await tester.tap(glyphButton('More for $_android'));
    await settle(tester);
    await tester.tap(find.text('Edit…'));
    await settle(tester);
    final field = find
        .descendant(
          of: find.byType(Overlay),
          matching: find.byType(EditableText),
        )
        .last;
    expect(tester.widget<EditableText>(field).controller.text, _android);
    await tester.enterText(field, 'Get the Android app on the Play Store');
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(
      harness.last.updateGoal.text,
      'Get the Android app on the Play Store',
    );

    await tester.tap(find.text('1 finished'));
    await settle(tester);
    await tester.tap(glyphButton('More for Ship the site'));
    await settle(tester);
    expect(find.text('Mark done'), findsNothing);
    await tester.tap(find.text('Reopen'));
    await settle(tester);
    expect(harness.last.updateGoal.goalId, 2);
    expect(
      harness.last.updateGoal.status,
      ChecklistStatus.CHECKLIST_STATUS_OPEN,
    );
  });

  testWidgets("a choice option in an agent's settings: the default until "
      'one is chosen, sent as the choice', (tester) async {
    final snapshot = workspace();
    snapshot.extensions[1] = checklistExtension.deepCopy()
      ..options.add(
        ExtensionOption(
          id: 'nudge_every',
          label: 'Check in on its checklist',
          kind: ExtensionOptionKind.EXTENSION_OPTION_KIND_CHOICE,
          choices: [
            OptionChoice(value: 'off', label: 'Off'),
            OptionChoice(value: '30m', label: 'Every 30 minutes'),
            OptionChoice(value: '1d', label: 'Once a day'),
          ],
          defaultChoice: 'off',
        ),
      );
    final harness = await showWith(tester, snapshot);
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    final select = find.descendant(
      of: find
          .ancestor(
            of: find.text('Check in on its checklist'),
            matching: find.byType(Row),
          )
          .first,
      matching: find.byType(Select<String>),
    );
    await tester.ensureVisible(select);
    await pumpFrames(tester);
    final widget = tester.widget<Select<String>>(select);
    expect(widget.value, 'off');
    expect(
      [for (final option in widget.options) option.label],
      ['Off', 'Every 30 minutes', 'Once a day'],
    );
    widget.onChanged('30m');
    await pumpFrames(tester);
    await tester.tap(find.text('Save'));
    await settle(tester);
    final change = harness.last.updateAgent.extension_8;
    expect(change.id, 'checklist');
    expect(change.optionId, 'nudge_every');
    expect(change.value.choice, '30m');
  });

  testWidgets('on a phone at the largest text, goals fit 320 points', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final snapshot = app.workspace(
      agents: [
        app.builder(working: false)
          ..openPanel = 'checklist'
          ..extensions.add(AgentExtension(id: 'checklist', enabled: true)),
      ],
    );
    snapshot.extensions.add(checklistExtension);
    snapshot.openPanel = 'checklist';
    final harness = Harness(snapshot);
    harness.backend.extensionView.apply(_withGoals());
    await harness.show(tester, size: const Size(320, 568));
    await pumpFrames(tester);
    // Above the items, at the top of the list, as the progress is.
    await tester.scrollUntilVisible(
      find.text(_android),
      -200,
      scrollable: find.byType(Scrollable).last,
    );
    await pumpFrames(tester);
    expect(find.text(_android), findsOneWidget);
    expect(tester.getRect(find.text(_android)).right, lessThanOrEqualTo(320));
    expect(tester.takeException(), isNull);
  });
}
