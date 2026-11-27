import 'package:factory_app/ask_card.dart';
import 'package:factory_app/controls.dart' show Pressable;
import 'package:factory_app/identity.dart' show Palette;
import 'package:factory_app/inbox.dart';
import 'package:factory_app/needs_you_history.dart' show NeedsYouHistoryList;
import 'package:factory_app/selection.dart' show CopyMenu;
import 'package:factory_app/pipes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/transcript.dart' show ProposalCard;
import 'package:factory_app/update.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'selection_test.dart' show copyItem, rightClick, watchClipboard;

Agent _agent(String id, String name, {List<int> waiting = const []}) => Agent(
  id: id,
  name: name,
  projectId: 'p1',
  modelAvailable: true,
  status: AgentStatus.AGENT_STATUS_IDLE,
  waitingSeqs: [for (final seq in waiting) Int64(seq)],
);

Workspace _workspace({
  List<int> june = const [],
  List<int> omar = const [],
  List<Pipe> pipes = const [],
  List<Chat> chats = const [],
  String selected = '',
}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  agents: [
    _agent('june', 'June', waiting: june),
    _agent('omar', 'Omar', waiting: omar),
  ],
  pipes: pipes,
  chats: chats,
  selectedAgentId: selected,
);

Entry _ask(
  int seq, {
  bool wantsText = true,
  AskPersonState state = AskPersonState.ASK_PERSON_STATE_PENDING,
  String answer = '',
}) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(1000 + seq),
  askPerson: AskPerson(
    id: 'ask$seq',
    title: 'Which region should the bucket live in?',
    body: 'The **launch** needs it today.',
    wantsText: wantsText,
    state: state,
    answer: answer,
  ),
);

Entry _proposal(int seq) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(500),
  proposal: Proposal(id: 'prop$seq', text: 'Add a designer agent'),
);

Finder _labeled(String label) => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == label,
);

void main() {
  Future<Harness> show(WidgetTester tester, Workspace snapshot) async {
    final harness = Harness(snapshot);
    await harness.show(tester);
    await settle(tester);
    return harness;
  }

  testWidgets('Needs you tops the sidebar while anything waits, with how '
      'many everywhere: agents and pipes', (tester) async {
    final harness = await show(
      tester,
      _workspace(
        june: [3],
        omar: [5, 6],
        pipes: [
          Pipe(
            id: 'pipe1',
            name: 'Acme',
            projectId: 'p1',
            state: PipeState.PIPE_STATE_APPROVAL,
          ),
        ],
      ),
    );
    expect(find.text('Needs you'), findsOneWidget);
    expect(_labeled('4 things are waiting for your answer'), findsOneWidget);

    await harness.apply(tester, _workspace());
    await settle(tester);
    expect(find.text('Needs you'), findsNothing);
  });

  testWidgets('the page lists each card, fetched, and opens one to answer '
      "it there: an agent's question", (tester) async {
    final harness = await show(tester, _workspace(june: [7], omar: [2]));
    harness.backend.entriesById['june/7'] = _ask(7);
    harness.backend.entriesById['omar/2'] = _proposal(2);

    await tester.tap(find.text('Needs you'));
    await settle(tester);
    // The cards, fetched.
    await settle(tester);
    expect(find.byType(InboxView), findsOneWidget);
    // Each card is asked for as it is now.
    final asked = [
      for (final message in harness.backend.sent)
        if (message.hasLoadEntries())
          '${message.loadEntries.agentId}/${message.loadEntries.fromSeq}',
    ];
    expect(asked, containsAll(['june/7', 'omar/2']));

    expect(
      find.text('Which region should the bucket live in?'),
      findsOneWidget,
    );
    expect(
      find.text('Proposes to Rivet: Add a designer agent'),
      findsOneWidget,
    );
    // Folded: no card yet.
    expect(find.byType(AskPersonCard), findsNothing);

    await tester.tap(find.text('Which region should the bucket live in?'));
    await settle(tester);
    expect(find.byType(AskPersonCard), findsOneWidget);
    expect(find.text('June needs your answer'), findsOneWidget);

    // Answer waits for words.
    await tester.tap(find.text('Answer'));
    await settle(tester);
    expect(harness.backend.sent.any((m) => m.hasAnswerAsk()), isFalse);

    await tester.enterText(
      find.descendant(
        of: find.byType(AskPersonCard),
        matching: find.byType(EditableText),
      ),
      'eu-west-1',
    );
    await tester.pump();
    await tester.tap(find.text('Answer'));
    await settle(tester);
    final answer = harness.last.answerAsk;
    expect(answer.agentId, 'june');
    expect(answer.seq.toInt(), 7);
    expect(answer.text, 'eu-west-1');
    expect(answer.dismiss, isFalse);

    // Answered: it no longer waits, and stays here, answered, until the
    // person leaves.
    harness.backend.entriesById['june/7'] = _ask(
      7,
      state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
      answer: 'eu-west-1',
    );
    await harness.apply(tester, _workspace(omar: [2]));
    await settle(tester);
    expect(find.text('Answered'), findsWidgets);
    expect(find.text('eu-west-1'), findsOneWidget);
  });

  testWidgets("a proposal answered from Needs you is its agent's, not the "
      'selected one', (tester) async {
    final harness = await show(tester, _workspace(omar: [2]));
    harness.backend.entriesById['omar/2'] = _proposal(2);
    await tester.tap(find.text('Needs you'));
    await settle(tester);
    // The cards, fetched.
    await settle(tester);
    await tester.tap(find.text('Proposes to Rivet: Add a designer agent'));
    await settle(tester);
    await tester.ensureVisible(find.text('Dismiss'));
    await settle(tester);
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    final decided = harness.last.decideProposal;
    expect(decided.agentId, 'omar');
    expect(decided.proposalId, 'prop2');
    expect(decided.approve, isFalse);
  });

  testWidgets("Needs you's words select and copy; its buttons still work", (
    tester,
  ) async {
    final copied = watchClipboard(tester);
    final harness = await show(tester, _workspace(omar: [2]));
    harness.backend.entriesById['omar/2'] = _proposal(2);
    await tester.tap(find.text('Needs you'));
    await settle(tester);
    await settle(tester);
    // Folded, its line; open, the card's own words. (Nested shapes join
    // the selection a frame or two later.)
    await pumpFrames(tester);
    await rightClick(
      tester,
      find.text('Proposes to Rivet: Add a designer agent'),
    );
    expect(find.byType(CopyMenu), findsOneWidget);
    await tester.tap(copyItem);
    await tester.pump();
    expect(copied, hasLength(1));
    expect('Proposes to Rivet: Add a designer agent', contains(copied.single));
    await tester.tap(find.text('Proposes to Rivet: Add a designer agent'));
    await settle(tester);
    expect(
      find.ancestor(
        of: find.text('Dismiss'),
        matching: find.byType(SelectableRegion),
      ),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Dismiss'));
    await settle(tester);
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    expect(harness.last.decideProposal.proposalId, 'prop2');

    // History's too.
    await tester.tap(find.text('History'));
    await settle(tester);
    expect(
      find.ancestor(
        of: find.byType(NeedsYouHistoryList),
        matching: find.byType(SelectableRegion),
      ),
      findsOneWidget,
    );
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('Select All copies each item on its own lines', (tester) async {
    final copied = watchClipboard(tester);
    final harness = await show(tester, _workspace(june: [1], omar: [2]));
    harness.backend.entriesById['june/1'] = _ask(1);
    harness.backend.entriesById['omar/2'] = _proposal(2);
    await tester.tap(find.text('Needs you'));
    await settle(tester);
    await settle(tester);
    // Nested shapes join the selection a frame or two later.
    await pumpFrames(tester);
    final region = tester.state<SelectableRegionState>(
      find.byType(SelectableRegion),
    );
    region.selectAll(SelectionChangedCause.keyboard);
    await tester.pump();
    // As ⌘C does.
    Actions.invoke(
      tester.element(find.textContaining('Proposes to Rivet')),
      CopySelectionTextIntent.copy,
    );
    await tester.pump();
    final text = copied.single;
    final items = text.split('\n\n');
    expect(items, hasLength(2), reason: text);
    for (final item in items) {
      expect(item.split('\n').length, greaterThan(1), reason: item);
    }
    expect(text, contains('Add a designer agent'));
    expect(text, contains('Which region should the bucket live in?'));
  });

  testWidgets("a pipe's approval opens to the approval itself", (tester) async {
    final harness = await show(
      tester,
      _workspace(
        pipes: [
          Pipe(
            id: 'pipe1',
            name: 'Acme',
            projectId: 'p1',
            state: PipeState.PIPE_STATE_APPROVAL,
            digits: '4821',
            peer: PipePeer(ownerName: 'Ana'),
          ),
        ],
      ),
    );
    await tester.tap(find.text('Needs you'));
    await settle(tester);
    // The cards, fetched.
    await settle(tester);
    await tester.tap(find.text('Ana wants to connect').first);
    await settle(tester);
    expect(find.byType(PipeApprovalCard), findsOneWidget);
    // Once it has opened to its full height.
    await settle(tester);
    await tester.ensureVisible(find.text('Approve'));
    await settle(tester);
    await tester.tap(find.text('Approve'));
    await settle(tester);
    expect(harness.last.approvePipe.pipeId, 'pipe1');
  });

  testWidgets("a message no agent answers waits in Needs you, saying to "
      'choose a lead; the pipe counts it', (tester) async {
    final harness = Harness(
      _workspace(
        pipes: [
          Pipe(
            id: 'pipe1',
            name: 'Acme',
            projectId: 'p1',
            state: PipeState.PIPE_STATE_OPEN,
            online: true,
            peer: PipePeer(ownerName: 'Ana', claktoryName: "Ana's Claktory"),
            memberIds: ['june'],
            unansweredSeqs: [Int64(7)],
          ),
        ],
      ),
    );
    harness.backend.pipeMessagesById['pipe1/7'] = PipeMessage(
      seq: Int64(7),
      timestampMs: Int64(5000),
      author: PipeAuthor(
        kind: PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_AGENT,
        name: 'Bo',
      ),
      text: 'Is the launch still Friday?',
      state: PipeMessageState.PIPE_MESSAGE_STATE_RECEIVED,
    );
    await harness.show(tester);
    await settle(tester);
    expect(find.text(pipeUnanswered), findsOneWidget, reason: 'its row');
    expect(
      _labeled('Acme has something waiting for your answer'),
      findsOneWidget,
    );
    await tester.tap(find.text('Needs you'));
    await settle(tester);
    await settle(tester);
    expect(
      find.text('$pipeUnanswered\nIs the launch still Friday?'),
      findsOneWidget,
    );
    await tester.tap(find.text('$pipeUnanswered\nIs the launch still Friday?'));
    await settle(tester);
    await settle(tester);
    expect(find.text(pipeChooseLead), findsOneWidget);
    await tester.ensureVisible(find.text('Pipe settings…'));
    await tester.tap(find.text('Pipe settings…'));
    await settle(tester);
    expect(find.text('Pipe settings'), findsOneWidget);
  });

  testWidgets("a chat's proposal to Rivet waits in Needs you under the "
      "chat's title; it's decided for the chat and opens it", (tester) async {
    final chat = Chat(
      id: 'chat1',
      title: 'Trip to Lisbon',
      waitingSeqs: [Int64(4)],
    );
    final harness = await show(tester, _workspace(chats: [chat]));
    harness.backend.chatEntriesById['chat1/4'] = _proposal(4);
    expect(_labeled('Something is waiting for your answer'), findsOneWidget);

    await tester.tap(find.text('Needs you'));
    await settle(tester);
    // The card, fetched from the chat.
    await settle(tester);
    final asked = [
      for (final message in harness.backend.sent)
        if (message.hasLoadChatEntries())
          '${message.loadChatEntries.chatId}/${message.loadChatEntries.fromSeq}',
    ];
    expect(asked, ['chat1/4']);
    // In Needs you (the sidebar lists the chat too).
    expect(
      find.descendant(
        of: find.byType(InboxView),
        matching: find.textContaining('Trip to Lisbon', findRichText: true),
      ),
      findsOneWidget,
    );
    expect(
      find.text('Proposes to Rivet: Add a designer agent'),
      findsOneWidget,
    );

    // Dismissed in one click, folded: the chat's, not an agent's.
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    final decided = harness.last.decideProposal;
    expect(decided.chatId, 'chat1');
    expect(decided.agentId, isEmpty);
    expect(decided.proposalId, 'prop4');
    expect(decided.approve, isFalse);

    // Opened, its card's own buttons are the chat's too.
    await tester.tap(find.text('Proposes to Rivet: Add a designer agent'));
    await settle(tester);
    await tester.ensureVisible(find.text('Open chat'));
    await settle(tester);
    expect(find.byType(ProposalCard), findsOneWidget);
    // It reads as a link at rest, before the pointer finds it: underlined,
    // in the muted text color (Matt).
    final palette = Palette.of(tester.element(find.text('Open chat')));
    final link = tester.widget<Text>(find.text('Open chat')).style!;
    expect(link.decoration, TextDecoration.underline);
    expect(link.color, palette.textMuted);
    expect(link.decorationColor, palette.textMuted);
    expect(link.fontWeight, FontWeight.w500);

    await tester.tap(find.text('Open chat'));
    await settle(tester);
    expect(harness.last.selectChat.chatId, 'chat1');
    expect(find.byType(InboxView), findsNothing);
  });

  testWidgets("on an iPad, Needs you's links are a finger's height and "
      'underlined, with no pointer to find them', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final chat = Chat(id: 'chat1', title: 'Trip', waitingSeqs: [Int64(4)]);
    final harness = Harness(_workspace(chats: [chat]));
    harness.backend.chatEntriesById['chat1/4'] = _proposal(4);
    await harness.show(tester, size: const Size(1180, 820));
    await settle(tester);
    await tester.tap(find.text('Needs you'));
    await settle(tester);
    await settle(tester);
    await tester.tap(find.text('Proposes to Rivet: Add a designer agent'));
    await settle(tester);
    final open = find.text('Open chat');
    await tester.ensureVisible(open);
    await settle(tester);
    final link = tester.widget<Text>(open).style!;
    expect(link.decoration, TextDecoration.underline);
    final target = find.ancestor(of: open, matching: find.byType(Pressable));
    expect(tester.getSize(target.first).height, greaterThanOrEqualTo(44));
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets("an agent's question in its conversation: dismissed, it's "
      "left to the agent's judgment", (tester) async {
    final harness = Harness(_workspace(june: [4], selected: 'june'));
    harness.backend.transcript.load(
      Transcript(agentId: 'june', entries: [_ask(4, wantsText: false)]),
    );
    await harness.show(tester);
    await settle(tester);

    expect(find.text('Which region should the bucket live in?'), findsWidgets);
    // Not asking for words: approving, with a comment if you like.
    await tester.tap(find.text('Dismiss').last);
    await settle(tester);
    final answer = harness.last.answerAsk;
    expect(answer.agentId, 'june');
    expect(answer.seq.toInt(), 4);
    expect(answer.dismiss, isTrue);

    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          _ask(
            4,
            wantsText: false,
            state: AskPersonState.ASK_PERSON_STATE_DISMISSED,
          ),
        ],
      ),
    );
    await settle(tester);
    expect(find.textContaining("Dismissed"), findsOneWidget);
  });

  testWidgets('a chip over the conversation says how many need you, '
      'anywhere; it drops the bubbles down without moving the chat', (
    tester,
  ) async {
    final harness = Harness(_workspace(omar: [2, 3], selected: 'june'));
    harness.backend.entriesById['omar/2'] = _proposal(2);
    harness.backend.entriesById['omar/3'] = _ask(3);
    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          for (var i = 1; i <= 40; i++)
            Entry(
              seq: Int64(i),
              assistant: AssistantText(text: 'Line $i of the conversation'),
            ),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);

    expect(find.text('2 need you'), findsOneWidget);
    expect(_labeled('2 things need your answer. Show them.'), findsOneWidget);
    final before = tester.getRect(find.text('Line 40 of the conversation'));

    await tester.tap(find.text('2 need you'));
    await settle(tester);
    await settle(tester);
    expect(
      find.descendant(
        of: find.byType(NeedsYouChip),
        matching: find.byType(NeedsYouList),
      ),
      findsOneWidget,
    );
    expect(
      find.text('Which region should the bucket live in?'),
      findsOneWidget,
    );
    // The conversation stays where it was.
    expect(tester.getRect(find.text('Line 40 of the conversation')), before);

    // A stale proposal goes in one click, folded.
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    final decided = harness.last.decideProposal;
    expect(decided.agentId, 'omar');
    expect(decided.approve, isFalse);

    // Folded away again.
    await tester.tap(find.text('2 need you'));
    await settle(tester);
    expect(find.byType(NeedsYouList), findsNothing);

    // Nothing waiting: no chip.
    await harness.apply(tester, _workspace(selected: 'june'));
    await settle(tester);
    expect(find.byType(NeedsYouChip), findsOneWidget);
    expect(find.textContaining('need'), findsNothing);
  });

  testWidgets("a question fits an iPhone SE at the largest text: its "
      'buttons go under the words', (tester) async {
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    tester.view.physicalSize = const Size(320, 700);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    final harness = Harness(_workspace(june: [4]));
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xFF000000),
        builder: (context, _) => AppScope(
          backend: harness.backend,
          login: harness.login,
          page: harness.page,
          appBuild: AppBuild(
            version: 'dev',
            bundlePath: '/Applications/Factory.app',
            quit: () {},
            updater: harness.updater,
          ),
          drafts: harness.drafts,
          history: harness.history,
          connections: harness.connections,
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(16),
            child: AskPersonCard(_ask(4).askPerson, agentId: 'june', seq: 4),
          ),
        ),
      ),
    );
    await settle(tester);
    expect(tester.takeException(), isNull);
    expect(find.text('Answer'), findsOneWidget);
  });

  test('what waits, everywhere: cards in agents and chats, pipe approvals, loops and held '
      'messages; archived projects wait out of sight', () {
    final workspace = Workspace(
      projects: [
        Project(id: 'p1', name: 'Factory'),
        Project(id: 'p2', name: 'Old', archived: true),
      ],
      agents: [
        _agent('june', 'June', waiting: [1, 2]),
        Agent(id: 'gone', projectId: 'p2', waitingSeqs: [Int64(9)]),
      ],
      chats: [
        Chat(id: 'c1', waitingSeqs: [Int64(5)]),
        Chat(id: 'c2'),
      ],
      pipes: [
        Pipe(
          id: 'a',
          projectId: 'p1',
          state: PipeState.PIPE_STATE_PAUSED,
          paused: PipePause.PIPE_PAUSE_LOOP,
          waitingSeqs: [Int64(3)],
        ),
        Pipe(id: 'b', projectId: 'p2', state: PipeState.PIPE_STATE_APPROVAL),
      ],
    );
    expect(
      [for (final item in needsYouItems(workspace)) item.key],
      ['agent:june/1', 'agent:june/2', 'chat:c1/5', 'pipe:a/loop', 'pipe:a/3'],
    );
    expect(needsYouCount(workspace), 5);
    expect(pipeColor, isNotNull);
  });
}
