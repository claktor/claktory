// No double approval (Matt via Rivet): a proposal to Rivet that names
// grants shows them under the agent's text, apart, as Claktory's words,
// with their warnings; once approved, whether Rivet used each and when.
// History says a change made with no card of its own was approved in that
// agent's proposal, and opens it.
import 'package:factory_app/decision.dart' show DecisionComment;
import 'package:factory_app/identity.dart' show Palette;
import 'package:factory_app/needs_you_history.dart' show HistoryRow;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/transcript.dart' show ProposalGrants;
import 'package:factory_app/visible_codes.dart' show visibleCodes;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'rivet_test.dart' show snapshot;

const _text = 'Give Carne Charts so it can build the users dashboard.';
const _summary = 'Carne gets Charts: Edit dashboards';
const _reads = 'Charts reads account\u200B-aggregates.csv in Ops';
const _warning =
    'Charts can reach api.example.com on the internet, so '
    'account-aggregates.csv could leave this computer.';

Proposal _proposal({
  ProposalState state = ProposalState.PROPOSAL_STATE_PENDING,
  int decided = 0,
  int used = 0,
  int usedReads = 0,
  bool replied = false,
}) => Proposal(
  id: 'p7',
  text: _text,
  state: state,
  decidedAtMs: Int64(decided),
  replied: replied,
  grants: [
    ProposalGrant(summary: _summary, usedMs: Int64(used)),
    ProposalGrant(
      summary: _reads,
      warnings: [_warning],
      usedMs: Int64(usedReads),
    ),
  ],
);

Color? _colorOf(WidgetTester tester, String text) =>
    tester.widget<Text>(_inGrants(find.text(text))).style?.color;

Harness _proposing(Proposal proposal) {
  final harness = Harness(snapshot(selected: 'a1'));
  harness.backend.transcript.load(
    Transcript(
      agentId: 'a1',
      entries: [Entry(seq: Int64(4), proposal: proposal)],
    ),
  );
  return harness;
}

Finder _inGrants(Finder finder) =>
    find.descendant(of: find.byType(ProposalGrants), matching: finder);

void main() {
  setUp(() {
    TestWidgetsFlutterBinding.ensureInitialized()
            .platformDispatcher
            .alwaysUse24HourFormatTestValue =
        true;
  });

  testWidgets("a pending proposal's grants show apart from the agent's text, "
      "as Claktory's, with their warnings", (tester) async {
    await _proposing(_proposal()).show(tester);
    await settle(tester);
    expect(
      _inGrants(
        find.text('Approving also lets Rivet do this, once, within a day:'),
      ),
      findsOneWidget,
    );
    expect(_inGrants(find.text('•  $_summary')), findsOneWidget);
    // A hidden character in a name or path shows by its code.
    expect(
      _inGrants(find.textContaining('account\\u{200B}-aggregates.csv')),
      findsOneWidget,
    );
    expect(_inGrants(find.text(_warning)), findsOneWidget);
    // The agent's words are never part of the list.
    expect(find.text(_text), findsOneWidget);
    expect(_inGrants(find.text(_text)), findsNothing);
    // Nothing is used yet, so nothing says so.
    expect(_inGrants(find.textContaining('Not used')), findsNothing);
    expect(find.text('Approve'), findsOneWidget);
  });

  testWidgets('on a phone the grants and their warnings fit', (tester) async {
    await _proposing(_proposal()).show(tester, size: const Size(390, 844));
    await settle(tester);
    expect(_inGrants(find.text('•  $_summary')), findsOneWidget);
    expect(_inGrants(find.text(_warning)), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('a comment covers none of the grants, and says so', (
    tester,
  ) async {
    final harness = _proposing(_proposal());
    await harness.show(tester);
    await settle(tester);
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    await tester.enterText(
      find.descendant(
        of: find.byType(DecisionComment),
        matching: find.byType(EditableText),
      ),
      'Only the reads',
    );
    await settle(tester);
    expect(
      find.text(
        'Rivet gets the proposal with your comment, but nothing above is '
        'allowed without asking you.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('approved, each grant says whether Rivet used it, and when', (
    tester,
  ) async {
    final decided = DateTime.now().subtract(const Duration(hours: 1));
    final used = DateTime(decided.year, decided.month, decided.day, 14, 2);
    await _proposing(
      _proposal(
        state: ProposalState.PROPOSAL_STATE_APPROVED,
        decided: decided.millisecondsSinceEpoch,
        used: used.millisecondsSinceEpoch,
      ),
    ).show(tester);
    await settle(tester);
    expect(
      _inGrants(find.text('Approving let Rivet do this, once, within a day:')),
      findsOneWidget,
    );
    expect(_inGrants(find.textContaining('Done · ')), findsOneWidget);
    expect(_inGrants(find.textContaining('14:02')), findsOneWidget);
    expect(_inGrants(find.text('Not used yet')), findsOneWidget);
  });

  testWidgets("past its day, an approved grant Rivet didn't use says so", (
    tester,
  ) async {
    await _proposing(
      _proposal(
        state: ProposalState.PROPOSAL_STATE_APPROVED,
        decided: DateTime.now()
            .subtract(const Duration(hours: 25))
            .millisecondsSinceEpoch,
      ),
    ).show(tester);
    await settle(tester);
    expect(_inGrants(find.text('Not used')), findsNWidgets(2));
    expect(_inGrants(find.text('Not used yet')), findsNothing);
    // Neither can be used any more: muted, without warnings.
    expect(_inGrants(find.text(_warning)), findsNothing);
  });

  testWidgets('a grant Rivet used can no longer be: muted, its warnings '
      'left out, while the one still unused keeps its text color '
      "(Abbacchio's row 1088)", (tester) async {
    final decided = DateTime.now().subtract(const Duration(hours: 1));
    await _proposing(
      _proposal(
        state: ProposalState.PROPOSAL_STATE_APPROVED,
        decided: decided.millisecondsSinceEpoch,
        usedReads: decided
            .add(const Duration(minutes: 10))
            .millisecondsSinceEpoch,
      ),
    ).show(tester);
    await settle(tester);
    final palette = Palette.of(tester.element(find.byType(ProposalGrants)));
    expect(_inGrants(find.text(_warning)), findsNothing);
    expect(_colorOf(tester, '•  ${visibleCodes(_reads)}'), palette.textMuted);
    expect(_colorOf(tester, '•  $_summary'), palette.text);
    expect(_inGrants(find.text('Not used yet')), findsOneWidget);
  });

  testWidgets('dismissed or answered, the grants are what Rivet still has to '
      'ask for, without warnings', (tester) async {
    for (final proposal in [
      _proposal(state: ProposalState.PROPOSAL_STATE_DISMISSED, decided: 1000),
      // A reply is no approval, whatever the state says.
      _proposal(
        state: ProposalState.PROPOSAL_STATE_DISMISSED,
        decided: 1000,
        replied: true,
      ),
    ]) {
      await _proposing(proposal).show(tester);
      await settle(tester);
      expect(
        _inGrants(find.text('Rivet still needs your OK for this:')),
        findsOneWidget,
      );
      expect(_inGrants(find.text(_warning)), findsNothing);
      expect(_inGrants(find.textContaining('Not used')), findsNothing);
    }
  });

  group('History', () {
    final now = DateTime.now();
    NeedsYouRecord grantRecord({
      NeedsYouRecordKind kind =
          NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PROPOSAL_GRANT,
      String title = 'Rivet turned on Continue automatically',
    }) => NeedsYouRecord(
      kind: kind,
      agentId: 'rivet',
      agentName: 'Rivet',
      seq: Int64(30),
      title: title,
      outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_CHANGED,
      proposalId: 'p7',
      proposalAgentId: 'a1',
      proposalSeq: Int64(4),
      appChangeId: kind == NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_APP_ACCESS
          ? 'ch1'
          : '',
      appId: 'charts',
      askedMs: Int64(
        now.subtract(const Duration(hours: 2)).millisecondsSinceEpoch,
      ),
      decidedMs: Int64(
        now.subtract(const Duration(hours: 1)).millisecondsSinceEpoch,
      ),
    );

    Future<Harness> openHistory(
      WidgetTester tester,
      List<NeedsYouRecord> records,
    ) async {
      final harness = Harness(snapshot())..backend.needsYouHistory = records;
      await harness.show(tester, size: const Size(1200, 900));
      await settle(tester);
      harness.page.value = AppPage.inbox;
      await pumpFrames(tester);
      await tester.tap(find.text('History'));
      await settle(tester);
      await settle(tester);
      return harness;
    }

    testWidgets("a change made on a proposal says whose, and opens it", (
      tester,
    ) async {
      final harness = await openHistory(tester, [grantRecord()]);
      expect(
        find.text('Rivet turned on Continue automatically'),
        findsOneWidget,
      );
      expect(find.text("Approved in Builder's proposal"), findsOneWidget);
      // It has no Undo.
      expect(find.text('Undo'), findsNothing);
      await tester.tap(find.text("Approved in Builder's proposal"));
      await settle(tester);
      expect(harness.page.value, AppPage.agent);
      expect(harness.last.selectAgent.agentId, 'a1');
      expect(harness.backend.reveal.value?.seq, 4);
    });

    testWidgets("the row itself opens Rivet's call that made it", (
      tester,
    ) async {
      final harness = await openHistory(tester, [grantRecord()]);
      await tester.tap(find.text('Rivet turned on Continue automatically'));
      await settle(tester);
      expect(harness.last.selectAgent.agentId, 'rivet');
      expect(harness.backend.reveal.value?.seq, 30);
    });

    testWidgets('an app access change made on a proposal keeps its Undo', (
      tester,
    ) async {
      await openHistory(tester, [
        grantRecord(
          kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_APP_ACCESS,
          title: 'Rivet gave Carne Edit dashboards on Charts',
        ),
      ]);
      expect(find.text("Approved in Builder's proposal"), findsOneWidget);
      expect(find.text('Undo'), findsOneWidget);
    });

    testWidgets("a record of another kind says nothing of proposals", (
      tester,
    ) async {
      await openHistory(tester, [
        NeedsYouRecord(
          kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_QUESTION,
          agentId: 'a1',
          title: 'Deploy?',
          outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_ANSWERED,
          decidedMs: Int64(now.millisecondsSinceEpoch),
        ),
      ]);
      expect(find.byType(HistoryRow), findsOneWidget);
      expect(find.textContaining('proposal'), findsNothing);
    });
  });
}
