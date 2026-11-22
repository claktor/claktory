// Abbacchio's #325 LOWs (rows 499–501): a copy from the Checklist panel
// is each item's words, what it waits on and its note, a line each, never
// the panel's labels run together ("router#1OpenUnassignToday"); with no
// goals, the items start under Add goal…, not two fifths down the panel;
// and an ask card in a narrow Mac column keeps its words readable, its
// buttons under them inside the card.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'extensions_test.dart' show at, now, showWith, workspace;
import 'harness.dart';
import 'selection_test.dart' show watchClipboard;

ExtensionView _view() => ExtensionView(
  extensionId: 'checklist',
  agentId: 'a1',
  title: 'Checklist',
  checklist: Checklist(
    items: [
      ChecklistItem(
        id: 1,
        text: 'Install the router',
        status: ChecklistStatus.CHECKLIST_STATUS_BLOCKED,
        created: at(now),
        blockedOn: 'person',
        blockedKind: BlockedKind.BLOCKED_KIND_PERSON_DECISION,
        note: 'Needs the admin password',
      ),
      ChecklistItem(
        id: 2,
        text: 'Choose the hosting plan',
        status: ChecklistStatus.CHECKLIST_STATUS_OPEN,
        created: at(now),
      ),
    ],
  ),
);

void main() {
  testWidgets('a copy from the Checklist panel: the items, a line each, '
      'without their labels', (tester) async {
    final copied = watchClipboard(tester);
    final harness = await showWith(tester, workspace(openPanel: 'checklist'));
    harness.backend.extensionView.apply(_view());
    await settle(tester);
    await pumpFrames(tester);
    final region = tester.state<SelectableRegionState>(
      find.ancestor(
        of: find.text('Install the router'),
        matching: find.byType(SelectableRegion),
      ),
    );
    region.selectAll(SelectionChangedCause.keyboard);
    await tester.pump();
    Actions.invoke(
      tester.element(find.text('Install the router')),
      CopySelectionTextIntent.copy,
    );
    await tester.pump();
    final text = copied.single;
    expect(
      text,
      contains(
        'Install the router\nWaiting for Builder to ask you\n'
        'Needs the admin password\nChoose the hosting plan',
      ),
      reason: text,
    );
    for (final label in ['#1', '#2', 'Open', 'Unassigned', 'Today']) {
      expect(text, isNot(contains(label)), reason: text);
    }
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('with no goals, the items start right under Add goal…', (
    tester,
  ) async {
    final harness = await showWith(tester, workspace(openPanel: 'checklist'));
    harness.backend.extensionView.apply(_view());
    await settle(tester);
    final add = tester.getRect(find.text('Add goal…'));
    final first = tester.getRect(find.text('Install the router'));
    expect(first.top - add.bottom, lessThan(80));
  });

  for (final width in const [185.0, 220.0, 300.0, 340.0]) {
    testWidgets('an ask card $width wide on a Mac: its words a few to a '
        'line, Dismiss and Approve inside it and working', (tester) async {
      final harness = Harness(workspace());
      await harness.show(
        tester,
        home: Align(
          alignment: Alignment.topLeft,
          child: SizedBox(
            width: width,
            child: AskPersonCard(
              AskPerson(id: 'q1', title: 'Budget: 1k or 2k?', body: 'For Q1.'),
              agentId: 'a2',
              seq: 4,
            ),
          ),
        ),
      );
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      final card = tester.getRect(find.byType(AskPersonCard));
      final lead = find.text('Tester carries on once you approve.');
      expect(tester.getRect(lead).right, lessThanOrEqualTo(card.right));
      if (width >= 300) {
        expect(tester.getSize(lead).width, greaterThanOrEqualTo(96));
      }
      for (final label in ['Dismiss', 'Approve']) {
        final rect = tester.getRect(find.text(label));
        expect(rect.left, greaterThan(card.left), reason: label);
        expect(rect.right, lessThan(card.right), reason: label);
      }
      await tester.tap(find.text('Approve'));
      await tester.pump();
      expect(harness.last.answerAsk.seq.toInt(), 4);
      expect(harness.last.answerAsk.dismiss, isFalse);
    });
  }
}
