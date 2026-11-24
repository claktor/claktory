import 'package:factory_app/decision.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'rivet_test.dart' show snapshot;

const _text = 'Add a fourth backend agent: the three are saturated.';

Entry proposal({
  ProposalState state = ProposalState.PROPOSAL_STATE_PENDING,
  String comment = '',
  int decided = 0,
}) => Entry(
  seq: Int64(4),
  proposal: Proposal(
    id: 'p7',
    text: _text,
    state: state,
    comment: comment,
    decidedAtMs: Int64(decided),
  ),
);

Harness proposing() {
  final harness = Harness(snapshot(selected: 'a1'));
  harness.backend.transcript.load(
    Transcript(agentId: 'a1', entries: [proposal()]),
  );
  return harness;
}

void main() {
  testWidgets('a typed comment is an answer: Approve and Dismiss give way '
      'to Submit, and Submit or Return sends it as a reply, never an '
      'approval (#370)', (tester) async {
    final harness = proposing();
    await harness.show(tester);
    expect(find.text('Proposal to Rivet'), findsOneWidget);
    expect(find.text(_text), findsOneWidget);
    // The comment is one tap away, under the buttons.
    expect(find.text('A comment for Rivet, if you like'), findsNothing);
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    expect(find.text('Rivet gets it with your answer.'), findsOneWidget);
    await tester.enterText(_commentField, 'Just one, on Sonnet ');
    await settle(tester);
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Dismiss'), findsNothing);
    expect(
      find.text(
        'Rivet gets the proposal with your comment and does what you say.',
      ),
      findsOneWidget,
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    final decide = harness.last.decideProposal;
    expect(decide.agentId, 'a1');
    expect(decide.proposalId, 'p7');
    expect(decide.reply, isTrue);
    // A Claktory from before Submit reads it as no approval.
    expect(decide.approve, isFalse);
    expect(decide.comment, 'Just one, on Sonnet');
  });

  testWidgets('Submit sends the same; cleared, Approve is back and '
      'approves', (tester) async {
    final harness = proposing();
    await harness.show(tester);
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    await tester.enterText(_commentField, 'Not now, next week');
    await settle(tester);
    await tester.tap(find.text('Submit'));
    await settle(tester);
    expect(harness.last.decideProposal.reply, isTrue);

    await tester.enterText(_commentField, '');
    await settle(tester);
    await tester.tap(find.text('Approve'));
    await settle(tester);
    final decide = harness.last.decideProposal;
    expect(decide.approve, isTrue);
    expect(decide.reply, isFalse);
  });

  testWidgets('answered, it says so with the comment', (tester) async {
    final harness = proposing();
    await harness.show(tester);
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          proposal()
            ..proposal.state = ProposalState.PROPOSAL_STATE_APPROVED
            ..proposal.replied = true
            ..proposal.comment = 'Just one, on Sonnet',
        ],
      ),
    );
    await settle(tester);
    expect(
      find.text('Answered · sent to Rivet', findRichText: true),
      findsOneWidget,
    );
    expect(find.text('Just one, on Sonnet'), findsOneWidget);
    expect(find.text('Submit'), findsNothing);
  });

  testWidgets('Dismiss with nothing typed sends no comment', (tester) async {
    final harness = proposing();
    await harness.show(tester);
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    final decide = harness.last.decideProposal;
    expect(decide.approve, isFalse);
    expect(decide.comment, '');
  });

  testWidgets('a refusal shows, and the proposal stays to decide', (
    tester,
  ) async {
    final harness = proposing();
    harness.backend.refuse = 'Rivet has no model available';
    await harness.show(tester);
    await tester.tap(find.text('Approve'));
    await settle(tester);
    expect(find.text('Rivet has no model available'), findsOneWidget);
    expect(find.text('Approve'), findsOneWidget);
  });

  testWidgets('decided, it shows how, when and the comment', (tester) async {
    final harness = proposing();
    await harness.show(tester);
    final at = DateTime.now().copyWith(hour: 14, minute: 2);
    harness.backend.transcript.update(
      EntryUpdate(
        agentId: 'a1',
        entry: proposal(
          state: ProposalState.PROPOSAL_STATE_APPROVED,
          comment: 'Yes, do it',
          decided: at.millisecondsSinceEpoch,
        ),
      ),
    );
    await settle(tester);
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Dismiss'), findsNothing);
    expect(find.textContaining('Approved, sent to Rivet'), findsOneWidget);
    expect(find.textContaining('14:02'), findsOneWidget);
    expect(find.text('Yes, do it'), findsOneWidget);

    harness.backend.transcript.update(
      EntryUpdate(
        agentId: 'a1',
        entry: proposal(state: ProposalState.PROPOSAL_STATE_DISMISSED),
      ),
    );
    await settle(tester);
    expect(find.textContaining('Dismissed'), findsOneWidget);
  });
}

/// The comment's field, once Add a comment is tapped.
final _commentField = find.descendant(
  of: find.byType(DecisionComment),
  matching: find.byType(EditableText),
);
