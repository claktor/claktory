// An admin agent's action card (#423 items 14–16, Fugo): what Approve does
// in Claktory's own words, and the agent's reason apart, named as its, so
// its words never read as part of what the card does.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'rivet_test.dart' show snapshot;

const _description =
    'Builder asks to uninstall Charts: its dashboards and data go too.';
const _reason = 'This only pauses it.';

Finder _inCard(Finder finder) =>
    find.descendant(of: find.byType(AskPersonCard), matching: finder);

Harness _approved(String error) {
  final workspace = snapshot(selected: 'a1');
  final harness = Harness(workspace);
  harness.backend.transcript.load(
    Transcript(
      agentId: 'a1',
      entries: [
        Entry(
          seq: Int64(7),
          timestampMs: Int64(500),
          askPerson: AskPerson(
            id: 'ask7',
            title: 'Add Freya to Website',
            actionDescription: _description,
            actionReason: _reason,
            state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
            decidedMs: Int64(900),
            actionError: error,
          ),
        ),
      ],
    ),
  );
  return harness;
}

void main() {
  testWidgets("the action in Claktory's words, the agent's reason apart and "
      'named as its; the combined body not shown', (tester) async {
    final workspace = snapshot(selected: 'a1');
    workspace.agents.last.waitingSeqs.add(Int64(7));
    final harness = Harness(workspace);
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(7),
            timestampMs: Int64(500),
            askPerson: AskPerson(
              id: 'ask7',
              title: 'Uninstall Charts',
              body: '$_description $_reason',
              actionDescription: _description,
              actionReason: _reason,
              state: AskPersonState.ASK_PERSON_STATE_PENDING,
            ),
          ),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    expect(_inCard(find.text(_description)), findsOneWidget);
    expect(_inCard(find.text("Builder's reason")), findsOneWidget);
    expect(
      _inCard(find.textContaining(_reason, findRichText: true)),
      findsOneWidget,
    );
    expect(
      _inCard(find.textContaining('$_description $_reason')),
      findsNothing,
    );
    expect(_inCard(find.text('Approve')), findsOneWidget);
  });

  testWidgets("approved, but the change couldn't be made: the card says so "
      'and why, where it says Approved (Zucchero, #414)', (tester) async {
    const why =
        "Freya is Partners' liaison: it reads that pipe, so it can't use "
        'another.';

    await _approved(why).show(tester);
    await settle(tester);
    final said = _inCard(
      find.textContaining(approvedNotDone, findRichText: true),
    );
    expect(said, findsOneWidget);
    expect(_inCard(find.text(why)), findsOneWidget);
  });

  testWidgets('approved and made: Approved, as before', (tester) async {
    await _approved('').show(tester);
    await settle(tester);
    expect(
      _inCard(find.textContaining(approvedNotDone, findRichText: true)),
      findsNothing,
    );
    expect(
      _inCard(find.textContaining('Approved', findRichText: true)),
      findsOneWidget,
    );
  });
}
