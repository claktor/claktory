// Cards their agents took back once they no longer applied (#407): in the
// conversation they say who withdrew them and why, with nothing to answer;
// one being answered just closes; a late answer's refusal shows as it is;
// and Needs you's History says the same.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'rivet_test.dart' show snapshot;

const _reason = 'The build passed on its own, so I no longer need this.';

AskPerson _ask({bool withdrawn = false, bool wantsText = true}) => AskPerson(
  id: 'ask7',
  title: 'Restart the build server?',
  body: 'It looks stuck.',
  wantsText: wantsText,
  state: withdrawn
      ? AskPersonState.ASK_PERSON_STATE_DISMISSED
      : AskPersonState.ASK_PERSON_STATE_PENDING,
  withdrawn: withdrawn,
  withdrawReason: withdrawn ? _reason : '',
  decidedMs: Int64(withdrawn ? 1000 : 0),
);

Harness _asking(AskPerson ask) {
  final workspace = snapshot(selected: 'a1');
  workspace.agents.last.waitingSeqs.add(Int64(7));
  final harness = Harness(workspace);
  harness.backend.transcript.load(
    Transcript(
      agentId: 'a1',
      entries: [Entry(seq: Int64(7), timestampMs: Int64(500), askPerson: ask)],
    ),
  );
  return harness;
}

Finder _inCard(Finder finder) =>
    find.descendant(of: find.byType(AskPersonCard), matching: finder);

void main() {
  testWidgets('a withdrawn question says who withdrew it and why, in its '
      "agent's words, with nothing to answer", (tester) async {
    final harness = _asking(_ask(withdrawn: true));
    await harness.show(tester);
    await settle(tester);
    expect(
      find.textContaining('Withdrawn by Builder', findRichText: true),
      findsOneWidget,
    );
    expect(find.text(_reason), findsOneWidget);
    expect(find.textContaining('Left to'), findsNothing);
    expect(find.textContaining('Dismissed'), findsNothing);
    expect(_inCard(find.byType(EditableText)), findsNothing);
    expect(_inCard(find.text('Answer')), findsNothing);
    expect(_inCard(find.text('Dismiss')), findsNothing);
  });

  testWidgets('a withdrawn procedure says the same, not how its steps went', (
    tester,
  ) async {
    final ask = _ask(withdrawn: true)
      ..steps.addAll([
        AskStep(text: 'Open the console'),
        AskStep(text: 'Press restart'),
      ]);
    final harness = _asking(ask);
    await harness.show(tester);
    await settle(tester);
    expect(
      find.textContaining('Withdrawn by Builder', findRichText: true),
      findsOneWidget,
    );
    expect(find.textContaining('Left to'), findsNothing);
    expect(find.text("Can't do this…"), findsNothing);
  });

  testWidgets('a question being answered when it is withdrawn just closes', (
    tester,
  ) async {
    final harness = _asking(_ask());
    await harness.show(tester);
    await settle(tester);
    await tester.enterText(_inCard(find.byType(EditableText)), 'Yes, do');
    await tester.pump();
    expect(_inCard(find.text('Answer')), findsOneWidget);

    // The backend drops it from what waits, and sends it withdrawn.
    final workspace = harness.backend.workspace.workspace.deepCopy();
    workspace.agents.last.waitingSeqs.clear();
    harness.backend.workspace.apply(workspace);
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(7),
            timestampMs: Int64(500),
            askPerson: _ask(withdrawn: true),
          ),
        ],
      ),
    );
    await settle(tester);
    expect(_inCard(find.byType(EditableText)), findsNothing);
    expect(_inCard(find.text('Answer')), findsNothing);
    expect(
      find.textContaining('Withdrawn by Builder', findRichText: true),
      findsOneWidget,
    );
  });

  testWidgets('an answer that arrives after it was withdrawn shows the '
      'refusal as it is', (tester) async {
    final harness = _asking(_ask());
    await harness.show(tester);
    await settle(tester);
    harness.backend.refuse = 'This question was withdrawn.';
    await tester.enterText(_inCard(find.byType(EditableText)), 'Yes, do');
    await tester.pump();
    await tester.tap(_inCard(find.text('Answer')));
    await settle(tester);
    expect(_inCard(find.text('This question was withdrawn.')), findsOneWidget);
  });

  testWidgets("a withdrawn proposal says who withdrew it and why, never "
      "Dismissed, with nothing to decide", (tester) async {
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
              state: ProposalState.PROPOSAL_STATE_DISMISSED,
              withdrawn: true,
              withdrawReason: _reason,
              decidedAtMs: Int64(1000),
            ),
          ),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    expect(
      find.textContaining('Withdrawn by Builder', findRichText: true),
      findsOneWidget,
    );
    expect(find.text(_reason), findsOneWidget);
    expect(find.textContaining('Dismissed'), findsNothing);
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Add a comment'), findsNothing);
  });

  testWidgets("Needs you's History says who withdrew a card, and why under "
      'its title', (tester) async {
    final now = DateTime.now();
    final harness = Harness(snapshot())
      ..backend.needsYouHistory = [
        NeedsYouRecord(
          kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_QUESTION,
          agentId: 'a1',
          agentName: 'Builder',
          seq: Int64(7),
          title: 'Restart the build server?',
          outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_WITHDRAWN,
          agentReason: _reason,
          askedMs: Int64(
            now.subtract(const Duration(hours: 3)).millisecondsSinceEpoch,
          ),
          decidedMs: Int64(
            now.subtract(const Duration(hours: 2)).millisecondsSinceEpoch,
          ),
        ),
      ];
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.tap(find.text('History'));
    await settle(tester);
    await settle(tester);
    expect(find.text('Restart the build server?'), findsOneWidget);
    expect(
      find.text('Withdrawn by Builder', findRichText: true),
      findsOneWidget,
    );
    expect(find.text('Builder: “$_reason”'), findsOneWidget);
    expect(find.text('Dismissed', findRichText: true), findsNothing);
  });

  testWidgets("a withdrawn review keeps its file's name but has nothing to "
      'review, and isn\'t headed as needing an answer (Abbacchio row 1032)', (
    tester,
  ) async {
    final harness = _asking(
      _ask(withdrawn: true)..reviewPath = '/Users/me/draft.md',
    );
    await harness.show(tester);
    await settle(tester);
    expect(_inCard(find.text('draft.md')), findsOneWidget);
    expect(_inCard(find.text('Review')), findsNothing);
    expect(_inCard(find.text('Builder needs your answer')), findsNothing);
    expect(_inCard(find.text('Builder asked')), findsOneWidget);
  });

  testWidgets("a reason's code shows as code, not its backticks", (
    tester,
  ) async {
    final harness = _asking(
      _ask(withdrawn: true)..withdrawReason = 'I fixed `style.css` myself.',
    );
    await harness.show(tester);
    await settle(tester);
    expect(find.textContaining('`', findRichText: true), findsNothing);
    expect(
      find.textContaining('style.css', findRichText: true),
      findsOneWidget,
    );
  });

  testWidgets('History, open, shows a card withdrawn meanwhile', (
    tester,
  ) async {
    final now = DateTime.now().millisecondsSinceEpoch;
    final workspace = snapshot();
    workspace.agents.last.waitingSeqs.add(Int64(7));
    final harness = Harness(workspace);
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.tap(find.text('History'));
    await settle(tester);
    await settle(tester);
    expect(find.text('Restart the build server?'), findsNothing);
    // Its agent takes it back: it leaves what waits, and History has it.
    harness.backend.needsYouHistory = [
      NeedsYouRecord(
        kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_QUESTION,
        agentId: 'a1',
        agentName: 'Builder',
        seq: Int64(7),
        title: 'Restart the build server?',
        outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_WITHDRAWN,
        agentReason: _reason,
        askedMs: Int64(now - 60000),
        decidedMs: Int64(now),
      ),
    ];
    final after = harness.backend.workspace.workspace.deepCopy();
    after.agents.last.waitingSeqs.clear();
    harness.backend.workspace.apply(after);
    await settle(tester);
    await settle(tester);
    expect(find.text('Restart the build server?'), findsOneWidget);
  });

  testWidgets('open in Needs you when its agent takes it back, its row says '
      'Withdrawn, not Answered (Zucchero row 405)', (tester) async {
    final workspace = snapshot();
    workspace.agents.last.waitingSeqs.add(Int64(7));
    final harness = Harness(workspace);
    harness.backend.entriesById['a1/7'] = Entry(
      seq: Int64(7),
      timestampMs: Int64(500),
      askPerson: _ask(),
    );
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await settle(tester);
    await tester.tap(find.text('Restart the build server?').first);
    await settle(tester);
    // Its agent takes it back.
    harness.backend.entriesById['a1/7'] = Entry(
      seq: Int64(7),
      timestampMs: Int64(500),
      askPerson: _ask(withdrawn: true),
    );
    final after = harness.backend.workspace.workspace.deepCopy();
    after.agents.last.waitingSeqs.clear();
    await harness.apply(tester, after);
    await settle(tester);
    await settle(tester);
    expect(
      find.textContaining('Withdrawn by Builder', findRichText: true),
      findsOneWidget,
    );
    expect(find.text('Withdrawn'), findsOneWidget);
    expect(find.text('Answered'), findsNothing);
  });
}
