// Cards their agents changed while they waited (#428): redrawn in place
// with "Updated · <time>" and the agent's reason; what the person is typing
// stays; every answer says which version it's for, and the refusal of an
// answer to an older one shows as it is; History says how often.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/identity.dart' show Palette;
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

  testWidgets("a review card whose file changed while it waited says so in "
      "Claktory's words, never as the agent's reason, and Approve is for "
      'the file as it reads now (Ghiaccio 46899)', (tester) async {
    const note =
        'This file changed since Builder asked. Approving means it as it '
        'reads now.';
    final harness = _asking(
      AskPerson(
        id: 'ask7',
        title: 'Is the merch page right?',
        reviewPath: '/w/merch.md',
        state: AskPersonState.ASK_PERSON_STATE_PENDING,
        revision: 2,
        updatedMs: Int64(1000),
        updateNote: note,
      ),
    );
    await harness.show(tester);
    await settle(tester);
    expect(_inCard(find.textContaining('Updated · ')), findsOneWidget);
    expect(_inCard(find.text(note)), findsOneWidget);
    expect(_inCard(find.text("Builder's reason")), findsNothing);
    await tester.tap(_inCard(find.text('Approve')));
    await settle(tester);
    expect(harness.last.answerAsk.revision, 2);
  });

  testWidgets('a card never changed says nothing of it', (tester) async {
    final harness = _asking(_ask());
    await harness.show(tester);
    await settle(tester);
    expect(_inCard(find.textContaining('Updated')), findsNothing);
  });

  testWidgets('changed while the person types: their words stay, under the '
      'new version, and the field says so', (tester) async {
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
    ); // Said over the field (N4b, Trish's words), until it's emptied.
    const said =
        'Builder updated this request while you were typing. Your words are '
        'kept.';
    expect(_inCard(find.text(said)), findsOneWidget);
    await tester.enterText(_inCard(find.byType(EditableText)), '');
    await tester.pump();
    expect(_inCard(find.text(said)), findsNothing);
  });

  testWidgets("an answer to an older version is set aside, not lost: the "
      'card says its agent changed it after the answer, and what the person '
      "said; the words stay to send again (#473)", (tester) async {
    final harness = _asking(_ask());
    await harness.show(tester);
    await settle(tester);
    harness.backend.refuse = 'This request changed. Look at it again.';
    await tester.enterText(_inCard(find.byType(EditableText)), 'Yes, do');
    await tester.pump();
    await tester.tap(_inCard(find.text('Answer')));
    await settle(tester);
    expect(
      _inCard(find.text('This request changed. Look at it again.')),
      findsNothing,
    );
    expect(
      _inCard(find.text('Builder changed this after your answer.')),
      findsOneWidget,
    );
    expect(_inCard(find.text('You said: “Yes, do”')), findsOneWidget);
    expect(
      tester
          .widget<EditableText>(_inCard(find.byType(EditableText)))
          .controller
          .text,
      'Yes, do',
    );

    // The new version comes: marked Updated in signal, with its reason, and
    // the answer set aside still shown until the person answers again.
    _load(harness, _ask(revision: 2, title: 'Restart the new build server?'));
    await settle(tester);
    final mark = _inCard(find.textContaining('Updated · '));
    expect(mark, findsOneWidget);
    final span =
        (tester.widget<Text>(mark).textSpan! as TextSpan).children!.first
            as TextSpan;
    expect(span.text, 'Updated');
    expect(span.style!.color, Palette.of(tester.element(mark)).signal);
    expect(_inCard(find.text("Builder's reason")), findsOneWidget);
    expect(_inCard(find.text('You said: “Yes, do”')), findsOneWidget);
    harness.backend.refuse = null;
    await tester.tap(_inCard(find.text('Answer')));
    await settle(tester);
    expect(harness.last.answerAsk.revision, 2);
    expect(_inCard(find.textContaining('You said')), findsNothing);
  });

  testWidgets('Needs you times a changed card from its change, says '
      'Updated, and puts it first (#473)', (tester) async {
    final now = DateTime.now().millisecondsSinceEpoch;
    final workspace = snapshot(selected: 'a1');
    workspace.agents.last.waitingSeqs.addAll([Int64(7), Int64(8)]);
    final harness = Harness(workspace);
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          // Asked an hour ago, changed a minute ago.
          Entry(
            seq: Int64(7),
            timestampMs: Int64(now - 3600000),
            askPerson: _ask(revision: 2)..updatedMs = Int64(now - 60000),
          ),
          // Asked ten minutes ago, never changed.
          Entry(
            seq: Int64(8),
            timestampMs: Int64(now - 600000),
            askPerson: AskPerson(
              id: 'ask8',
              title: 'Deploy the site?',
              state: AskPersonState.ASK_PERSON_STATE_PENDING,
            ),
          ),
        ],
      ),
    );
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await settle(tester);
    expect(
      find.textContaining('Updated 1 min ago', findRichText: true),
      findsWidgets,
    );
    expect(find.textContaining('1 h ago', findRichText: true), findsNothing);
    final changed = tester.getTopLeft(
      find.text('Restart the build server?').first,
    );
    final other = tester.getTopLeft(find.text('Deploy the site?').first);
    expect(changed.dy, lessThan(other.dy));
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
          // Words, so it reads Answered (a bare yes reads Approved).
          reply: 'After lunch',
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
    expect(
      find.text('Answered · Updated twice · “After lunch”', findRichText: true),
      findsOneWidget,
    );
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
      harness.backend.refuse = 'This request changed. Look at it again.';
      harness.backend.connected.value = false;
      await tester.pump();
      harness.backend.connected.value = true;
      await settle(tester);
      expect(harness.last.decideProposal.revision, 1);
      expect(find.text(askWaiting), findsNothing);
      expect(
        find.text('This request changed. Look at it again.'),
        findsOneWidget,
      );
      expect(find.text('Add a fifth backend agent.'), findsOneWidget);
    });
  });
}
