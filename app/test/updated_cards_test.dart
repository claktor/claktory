// Cards their agents changed while they waited (#428): redrawn in place
// with "Updated · <time>" and the agent's reason; what the person is typing
// stays; every answer says which version it's for, and the refusal of an
// answer to an older one shows as it is; History says how often.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'rivet_test.dart' show snapshot;

const _reason = 'The build server is a different one now.';

AskPerson _ask({
  int revision = 1,
  String title = 'Restart the build server?',
}) => AskPerson(
  id: 'ask7',
  title: title,
  body: 'It looks stuck.',
  wantsText: true,
  state: AskPersonState.ASK_PERSON_STATE_PENDING,
  revision: revision,
  updatedMs: Int64(revision > 1 ? 1000 : 0),
  updateReason: revision > 1 ? _reason : '',
);

void _load(Harness harness, AskPerson ask) => harness.backend.transcript.load(
  Transcript(
    agentId: 'a1',
    entries: [Entry(seq: Int64(7), timestampMs: Int64(500), askPerson: ask)],
  ),
);

Harness _asking(AskPerson ask) {
  final workspace = snapshot(selected: 'a1');
  workspace.agents.last.waitingSeqs.add(Int64(7));
  final harness = Harness(workspace);
  _load(harness, ask);
  return harness;
}

Finder _inCard(Finder finder) =>
    find.descendant(of: find.byType(AskPersonCard), matching: finder);

void main() {
  testWidgets('a changed card says so, with its reason, and the answer is '
      'for that version', (tester) async {
    final harness = _asking(_ask(revision: 3));
    await harness.show(tester);
    await settle(tester);
    expect(_inCard(find.textContaining('Updated · ')), findsOneWidget);
    expect(_inCard(find.text(_reason)), findsOneWidget);
    await tester.enterText(_inCard(find.byType(EditableText)), 'Yes');
    await tester.pump();
    await tester.tap(_inCard(find.text('Answer')));
    await settle(tester);
    expect(harness.last.answerAsk.revision, 3);
  });

  testWidgets('a card never changed says nothing of it', (tester) async {
    final harness = _asking(_ask());
    await harness.show(tester);
    await settle(tester);
    expect(_inCard(find.textContaining('Updated')), findsNothing);
  });

  testWidgets('changed while the person types: their words stay, under the '
      'new version', (tester) async {
    final harness = _asking(_ask());
    await harness.show(tester);
    await settle(tester);
    await tester.enterText(_inCard(find.byType(EditableText)), 'Yes, do');
    await tester.pump();
    _load(harness, _ask(revision: 2, title: 'Restart the new build server?'));
    await settle(tester);
    expect(_inCard(find.text('Restart the new build server?')), findsOneWidget);
    expect(_inCard(find.textContaining('Updated · ')), findsOneWidget);
    expect(
      tester
          .widget<EditableText>(_inCard(find.byType(EditableText)))
          .controller
          .text,
      'Yes, do',
    );
  });

  testWidgets("an answer to an older version: the refusal shows, the words "
      'stay', (tester) async {
    final harness = _asking(_ask());
    await harness.show(tester);
    await settle(tester);
    harness.backend.refuse = 'This card changed. Look at it again.';
    await tester.enterText(_inCard(find.byType(EditableText)), 'Yes, do');
    await tester.pump();
    await tester.tap(_inCard(find.text('Answer')));
    await settle(tester);
    expect(
      _inCard(find.text('This card changed. Look at it again.')),
      findsOneWidget,
    );
    expect(
      tester
          .widget<EditableText>(_inCard(find.byType(EditableText)))
          .controller
          .text,
      'Yes, do',
    );
  });

  testWidgets("a procedure's step answer says the version too", (tester) async {
    final harness = _asking(
      _ask(revision: 2)
        ..wantsText = false
        ..steps.addAll([
          AskStep(text: 'Open the console'),
          AskStep(text: 'Press restart'),
        ]),
    );
    await harness.show(tester);
    await settle(tester);
    await tester.tap(_inCard(find.text('Done')));
    await settle(tester);
    expect(harness.last.answerAsk.step, 0);
    expect(harness.last.answerAsk.revision, 2);
  });

  testWidgets('a changed proposal says so, and its decision is for that '
      'version', (tester) async {
    final harness = Harness(snapshot(selected: 'a1'));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(4),
            proposal: Proposal(
              id: 'p7',
              text: 'Add a fourth backend agent.',
              state: ProposalState.PROPOSAL_STATE_PENDING,
              revision: 2,
              updatedMs: Int64(1000),
              updateReason: _reason,
            ),
          ),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    expect(find.textContaining('Updated · '), findsOneWidget);
    expect(find.text(_reason), findsOneWidget);
    await tester.tap(find.text('Approve'));
    await settle(tester);
    expect(harness.last.decideProposal.revision, 2);
  });

  testWidgets("History says how often a card was changed", (tester) async {
    final now = DateTime.now().millisecondsSinceEpoch;
    final harness = Harness(snapshot())
      ..backend.needsYouHistory = [
        NeedsYouRecord(
          kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_QUESTION,
          agentId: 'a1',
          agentName: 'Builder',
          seq: Int64(7),
          title: 'Restart the new build server?',
          outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_ANSWERED,
          updates: 2,
          askedMs: Int64(now - 60000),
          decidedMs: Int64(now),
        ),
      ];
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.tap(find.text('History'));
    await settle(tester);
    await settle(tester);
    expect(find.text('Answered · Updated twice'), findsOneWidget);
  });

  group('a proposal decided while the connection is down (row 1052)', () {
    Proposal proposal({int revision = 1}) => Proposal(
      id: 'p7',
      text: revision > 1
          ? 'Add a fifth backend agent.'
          : 'Add a fourth backend agent.',
      state: ProposalState.PROPOSAL_STATE_PENDING,
      revision: revision,
      updatedMs: Int64(revision > 1 ? 1000 : 0),
    );

    void load(Harness harness, Proposal proposal) =>
        harness.backend.transcript.load(
          Transcript(
            agentId: 'a1',
            entries: [Entry(seq: Int64(4), proposal: proposal)],
          ),
        );

    Future<Harness> approvedOffline(WidgetTester tester) async {
      final harness = Harness(snapshot(selected: 'a1'));
      load(harness, proposal());
      await harness.show(tester);
      await settle(tester);
      harness.backend.refuse = 'Connection lost.';
      await tester.tap(find.text('Approve'));
      await settle(tester);
      expect(find.text(askWaiting), findsOneWidget);
      return harness;
    }

    testWidgets('goes by itself once the connection is back, for the version '
        'pressed', (tester) async {
      final harness = await approvedOffline(tester);
      harness.backend.refuse = null;
      harness.backend.connected.value = false;
      await tester.pump();
      harness.backend.connected.value = true;
      await settle(tester);
      expect(harness.last.decideProposal.revision, 1);
      expect(find.text(askWaiting), findsNothing);
    });

    testWidgets('changed meanwhile: the refusal shows in place of "Not sent '
        'yet", and nothing is decided', (tester) async {
      final harness = await approvedOffline(tester);
      load(harness, proposal(revision: 2));
      harness.backend.refuse = 'This card changed. Look at it again.';
      harness.backend.connected.value = false;
      await tester.pump();
      harness.backend.connected.value = true;
      await settle(tester);
      expect(harness.last.decideProposal.revision, 1);
      expect(find.text(askWaiting), findsNothing);
      expect(find.text('This card changed. Look at it again.'), findsOneWidget);
      expect(find.text('Add a fifth backend agent.'), findsOneWidget);
    });
  });
}
