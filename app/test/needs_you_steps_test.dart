// Friend bug #11: a procedure (ask_person's steps) answered in Needs you,
// its agent's conversation not open, moves on to its next step there, as it
// does in the conversation: on the Mac's page, in the chip's drop-down and
// on a phone, without closing and opening it again.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _steps = [
  'Open System Settings',
  'Choose General',
  'Choose Login Items',
  'Find Claktory',
  'Turn it on',
  'Enter your password',
  'Close System Settings',
];

/// Kai's procedure, its first [done] steps done.
Entry _procedure(int done) => Entry(
  seq: Int64(9),
  timestampMs: Int64(1000),
  askPerson: AskPerson(
    id: 'ask11',
    title: 'Allow the helper',
    body: 'Seven steps on this Mac.',
    state: done == _steps.length
        ? AskPersonState.ASK_PERSON_STATE_ANSWERED
        : AskPersonState.ASK_PERSON_STATE_PENDING,
    steps: [
      for (final (i, text) in _steps.indexed)
        AskStep(
          text: text,
          state: i < done
              ? AskStepState.ASK_STEP_STATE_DONE
              : AskStepState.ASK_STEP_STATE_PENDING,
          decidedMs: i < done ? Int64(2000) : Int64(0),
        ),
    ],
  ),
);

/// June is open (or nothing is), Kai's procedure waits; [working], Kai has
/// been told of a step (the Workspace comes again).
Workspace _workspace({
  String selected = '',
  bool waiting = true,
  bool working = false,
}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      projectId: 'p1',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
    ),
    Agent(
      id: 'kai',
      name: 'Kai',
      projectId: 'p1',
      modelAvailable: true,
      status: working
          ? AgentStatus.AGENT_STATUS_WORKING
          : AgentStatus.AGENT_STATUS_IDLE,
      waitingSeqs: [if (waiting) Int64(9)],
    ),
  ],
  selectedAgentId: selected,
);

/// Done on step [at] (1-based), as the backend then has it: the entry at
/// its next step, and the Workspace sent again with Kai changed (told of it,
/// it starts or stops working).
Future<void> _done(WidgetTester tester, Harness harness, int at) async {
  await tester.ensureVisible(find.text('Step $at of 7'));
  // The card's own Done, its primary button (not the page's).
  await tester.tap(
    find
        .descendant(of: find.byType(AskPersonCard), matching: find.text('Done'))
        .last,
  );
  await tester.pump();
  final answer = harness.last.answerAsk;
  expect((answer.agentId, answer.seq.toInt(), answer.step), ('kai', 9, at - 1));
  harness.backend.entriesById['kai/9'] = _procedure(at);
  final kai = harness.backend.workspace.workspace.agents.firstWhere(
    (agent) => agent.id == 'kai',
  );
  await harness.apply(
    tester,
    _workspace(
      selected: harness.backend.workspace.workspace.selectedAgentId,
      waiting: at < _steps.length,
      working: kai.status != AgentStatus.AGENT_STATUS_WORKING,
    ),
  );
  await settle(tester);
}

/// How many times Kai's procedure was fetched so far.
int _fetches(Harness harness) => harness.backend.sent
    .where(
      (message) =>
          message.hasLoadEntries() &&
          message.loadEntries.agentId == 'kai' &&
          message.loadEntries.fromSeq.toInt() == 9,
    )
    .length;

void main() {
  // Polpo's needs-you-refetch.md: every waiting procedure was fetched again
  // on every change to the Workspace, several a second on a busy Claktory.
  testWidgets('a procedure is fetched again only once its agent changes', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    harness.backend.entriesById['kai/9'] = _procedure(5);
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await settle(tester);
    await tester.tap(find.text('Allow the helper · Step 6 of 7').first);
    await settle(tester);
    expect(find.text('Step 6 of 7'), findsOneWidget);
    final fetched = _fetches(harness);

    // June works, again and again: nothing of Kai's to fetch.
    for (var i = 0; i < 5; i++) {
      final workspace = _workspace();
      workspace.agents.first
        ..status = i.isEven
            ? AgentStatus.AGENT_STATUS_WORKING
            : AgentStatus.AGENT_STATUS_IDLE
        ..contextTokens = Int64(1000 * i);
      await harness.apply(tester, workspace);
      await settle(tester);
    }
    expect(_fetches(harness), fetched);

    // Kai told of a step answered on the phone: fetched once, its next step.
    harness.backend.entriesById['kai/9'] = _procedure(6);
    await harness.apply(tester, _workspace(working: true));
    await settle(tester);
    expect(_fetches(harness), fetched + 1);
    expect(find.text('Step 7 of 7'), findsOneWidget);

    // The same Workspace again: nothing more.
    await harness.apply(tester, _workspace(working: true));
    await settle(tester);
    expect(_fetches(harness), fetched + 1);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets("on the Mac's Needs you page, Done shows the next step", (
    tester,
  ) async {
    final harness = Harness(_workspace());
    harness.backend.entriesById['kai/9'] = _procedure(5);
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await settle(tester);
    await tester.tap(find.text('Allow the helper · Step 6 of 7').first);
    await settle(tester);
    await tester.pump(const Duration(seconds: 1));
    expect(find.text('Step 6 of 7'), findsOneWidget);

    await _done(tester, harness, 6);
    expect(find.text('Step 6 of 7'), findsNothing);
    expect(find.text('Step 7 of 7'), findsOneWidget);
    expect(find.text('Close System Settings'), findsOneWidget);
    // Folded, its line says the step it's at too.
    expect(find.text('Allow the helper · Step 7 of 7'), findsWidgets);

    // The last one: how it ended, kept until the person leaves.
    await _done(tester, harness, 7);
    expect(find.textContaining('All 7 steps done'), findsOneWidget);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets("in the chip's drop-down over another conversation, Done shows "
      'the next step', (tester) async {
    final harness = Harness(_workspace(selected: 'june'));
    harness.backend.transcript.load(Transcript(agentId: 'june'));
    harness.backend.entriesById['kai/9'] = _procedure(5);
    await harness.show(tester);
    await settle(tester);
    await tester.tap(find.text('1 needs you'));
    await settle(tester);
    await settle(tester);
    await tester.tap(find.text('Allow the helper · Step 6 of 7').first);
    await settle(tester);
    await tester.pump(const Duration(seconds: 1));
    expect(
      find.descendant(
        of: find.byType(NeedsYouChip),
        matching: find.byType(AskPersonCard),
      ),
      findsOneWidget,
    );
    expect(find.text('Step 6 of 7'), findsOneWidget);

    await _done(tester, harness, 6);
    expect(find.text('Step 6 of 7'), findsNothing);
    expect(find.text('Step 7 of 7'), findsOneWidget);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets("on a phone's Needs you, Done shows the next step", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace());
    harness.backend.entriesById['kai/9'] = _procedure(5);
    await harness.show(tester, size: const Size(393, 852));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await settle(tester);
    // An iPhone's Needs you shows the card itself, one a screen (#514).
    expect(find.text('Step 6 of 7'), findsOneWidget);

    await _done(tester, harness, 6);
    expect(find.text('Step 6 of 7'), findsNothing);
    expect(find.text('Step 7 of 7'), findsOneWidget);
  });
}
