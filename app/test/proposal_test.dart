import 'package:factory_app/controls.dart' show TextField;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
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
  testWidgets('a proposal is approved with a comment by Enter', (tester) async {
    final harness = proposing();
    await harness.show(tester);
    expect(find.text('Proposal to Rivet'), findsOneWidget);
    expect(find.text(_text), findsOneWidget);
    await tester.tap(
      find.ancestor(
        of: find.text('A comment for Rivet, if you like'),
        matching: find.byType(TextField),
      ),
    );
    await tester.pump();
    tester.testTextInput.enterText('Yes, do it ');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await settle(tester);
    final decide = harness.last.decideProposal;
    expect(decide.agentId, 'a1');
    expect(decide.proposalId, 'p7');
    expect(decide.approve, isTrue);
    expect(decide.comment, 'Yes, do it');
  });

  testWidgets('Dismiss sends no comment', (tester) async {
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
