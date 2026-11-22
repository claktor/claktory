// #274: what the person types in a Needs you card, and where they type it,
// survive the list changing around it: a new question coming in above it,
// one answered elsewhere going, the list reordering.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent _agent(String id, String name, {List<int> waiting = const []}) => Agent(
  id: id,
  name: name,
  projectId: 'p1',
  modelAvailable: true,
  status: AgentStatus.AGENT_STATUS_IDLE,
  waitingSeqs: [for (final seq in waiting) Int64(seq)],
);

Workspace _workspace({List<int> june = const [], List<int> omar = const []}) =>
    Workspace(
      projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
      agents: [
        _agent('june', 'June', waiting: june),
        _agent('omar', 'Omar', waiting: omar),
      ],
    );

/// A question asked at [time]: the newest shows first.
Entry _ask(int seq, String title, {required int time}) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(time),
  askPerson: AskPerson(id: 'ask$seq', title: title, wantsText: true),
);

const _region = 'Which region should the bucket live in?';

void main() {
  for (final (device, size) in [
    ('Mac', const Size(1200, 900)),
    ('iPad', const Size(1180, 820)),
    ('iPhone', const Size(390, 844)),
  ]) {
    testWidgets('$device: a new question, one answered elsewhere and the '
        'list reordering keep what is typed in a card, and the focus', (
      tester,
    ) async {
      final harness = Harness(_workspace(june: [7], omar: [2]));
      harness.backend.entriesById['june/7'] = _ask(7, _region, time: 2000);
      harness.backend.entriesById['omar/2'] = _ask(
        2,
        'Ship it today?',
        time: 3000,
      );
      await harness.show(tester, size: size);
      await settle(tester);
      harness.page.value = AppPage.inbox;
      await settle(tester);
      await settle(tester);
      expect(find.byType(InboxView), findsOneWidget);

      // June's question, below Omar's newer one, opened and half answered.
      await tester.tap(find.text(_region));
      await settle(tester);
      final field = find.descendant(
        of: find.byType(AskPersonCard),
        matching: find.byType(EditableText),
      );
      await tester.showKeyboard(field);
      await tester.enterText(field, 'eu-west');
      await tester.pump();

      void typedAndFocused() {
        final editable = tester.widget<EditableText>(field);
        expect(editable.controller.text, 'eu-west');
        expect(editable.focusNode.hasFocus, isTrue);
      }

      typedAndFocused();

      // A new question comes in, on top.
      harness.backend.entriesById['june/9'] = _ask(
        9,
        'Which domain?',
        time: 4000,
      );
      await harness.apply(tester, _workspace(june: [7, 9], omar: [2]));
      await settle(tester);
      await settle(tester);
      expect(find.text('Which domain?'), findsOneWidget);
      typedAndFocused();

      // Omar's is answered elsewhere: it goes from above.
      await harness.apply(tester, _workspace(june: [7, 9]));
      await settle(tester);
      typedAndFocused();

      // A newer one again, from another agent, and the new one goes.
      harness.backend.entriesById['omar/4'] = _ask(
        4,
        'Rename the repo?',
        time: 5000,
      );
      await harness.apply(tester, _workspace(june: [7], omar: [4]));
      await settle(tester);
      await settle(tester);
      typedAndFocused();

      // Typing goes on where it was.
      await tester.enterText(field, 'eu-west-1');
      await tester.pump();
      expect(tester.widget<EditableText>(field).controller.text, 'eu-west-1');
      expect(tester.takeException(), isNull);
    });
  }
}
