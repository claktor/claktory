import 'package:factory_app/pipe_timeline.dart';
import 'package:factory_app/pipes.dart' show PipeView;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/update.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent _agent(String id, String name, {String pipe = ''}) => Agent(
  id: id,
  name: name,
  projectId: 'p1',
  pipeId: pipe,
  quarantined: pipe.isNotEmpty,
  modelAvailable: true,
  status: AgentStatus.AGENT_STATUS_IDLE,
);

Pipe _pipe({
  PipeState state = PipeState.PIPE_STATE_OPEN,
  bool reviewOutgoing = false,
}) => Pipe(
  id: 'pipe1',
  name: 'Acme launch',
  projectId: 'p1',
  state: state,
  createdMs: Int64(1000),
  folder: 'acme-launch-ab12cd',
  peer: PipePeer(
    claktoryName: "Ana's Claktory",
    ownerName: 'Ana',
    projectName: 'Acme Site',
  ),
  memberIds: const ['kai', 'june'],
  leadId: 'kai',
  quarantinedIds: const ['kai'],
  canRunCommandsIds: const ['june'],
  limits: PipeLimits(messagesPerHour: 30, burst: 10),
  online: true,
  reviewOutgoing: reviewOutgoing,
);

Workspace _workspace({
  PipeState state = PipeState.PIPE_STATE_OPEN,
  bool reviewOutgoing = false,
}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/f')],
  agents: [
    _agent('june', 'June'),
    _agent('kai', 'Kai', pipe: 'pipe1'),
  ],
  pipes: [_pipe(state: state, reviewOutgoing: reviewOutgoing)],
  selectedPipeId: 'pipe1',
  selectedAgentId: 'kai',
);

PipeMessage _message(
  int seq,
  int at,
  String text,
  PipeAuthorKind kind, {
  PipeMessageState state = PipeMessageState.PIPE_MESSAGE_STATE_RECEIVED,
  String name = '',
  String agentId = '',
}) => PipeMessage(
  seq: Int64(seq),
  timestampMs: Int64(at),
  author: PipeAuthor(kind: kind, name: name, agentId: agentId),
  text: text,
  state: state,
);

Entry _event(int seq, int at, List<int> pipeSeqs) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(at),
  event: Event(
    source: EventSource.EVENT_SOURCE_PIPE,
    from: "Acme launch (Ana's Claktory)",
    text: 'WAKE FROM THE PIPE',
    pipeSeqs: [for (final seq in pipeSeqs) Int64(seq)],
  ),
);

Entry _say(int seq, int at, String text) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(at),
  assistant: AssistantText(text: text),
);

Entry _tool(int seq, int at, String name, {String details = ''}) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(at),
  toolCall: ToolCall(
    callId: 'c$seq',
    name: name,
    input: '{}',
    output: 'OUTPUT OF $name',
    state: ToolCallState.TOOL_CALL_STATE_DONE,
    details: details,
  ),
);

void main() {
  Future<Harness> show(
    WidgetTester tester, {
    PipeState state = PipeState.PIPE_STATE_OPEN,
    List<Entry> entries = const [],
    List<PipeMessage> messages = const [],
    bool chatHasEarlier = false,
    bool reviewOutgoing = false,
  }) async {
    final harness = Harness(
      _workspace(state: state, reviewOutgoing: reviewOutgoing),
    );
    await harness.show(tester);
    harness.backend.transcript.load(
      Transcript(agentId: 'kai', entries: entries),
    );
    harness.backend.pipeChat.load(
      PipeTranscript(
        pipeId: 'pipe1',
        messages: messages,
        hasEarlier: chatHasEarlier,
      ),
    );
    await settle(tester);
    return harness;
  }

  Finder field() => find.byType(EditableText).last;

  testWidgets("the liaison's conversation with the pipe's messages among it "
      'as cards, in time order, leaving out what the cards say', (
    tester,
  ) async {
    await show(
      tester,
      entries: [
        _event(1, 2000, [1]),
        _tool(2, 2100, 'pipe_read'),
        _say(3, 2200, 'They want the deck. I will answer.'),
        _tool(4, 2300, 'pipe_send', details: '{"pipe":"pipe1","seq":2}'),
        _say(5, 2400, 'Sent.'),
      ],
      messages: [
        _message(
          1,
          2000,
          'Can you send the deck?',
          PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_AGENT,
          name: 'Bo',
        ),
        _message(
          2,
          2300,
          'Here it is.',
          PipeAuthorKind.PIPE_AUTHOR_KIND_MY_AGENT,
          agentId: 'kai',
          state: PipeMessageState.PIPE_MESSAGE_STATE_DELIVERED,
        ),
      ],
    );

    expect(find.byType(LiaisonPipeBody), findsOneWidget);
    expect(find.text('Can you send the deck?'), findsOneWidget);
    expect(find.text('Here it is.'), findsOneWidget);
    expect(
      find.textContaining("From Ana's Claktory", findRichText: true),
      findsOneWidget,
    );
    expect(
      find.textContaining("To Ana's Claktory", findRichText: true),
      findsWidgets,
    );
    // The wake and the send are the cards; reading the pipe stays.
    expect(find.textContaining('WAKE FROM THE PIPE'), findsNothing);
    expect(find.text('pipe_send'), findsNothing);
    expect(find.text('pipe_read'), findsOneWidget);

    // In time order: their message, the liaison's words, its message.
    final theirs = tester.getTopLeft(find.text('Can you send the deck?')).dy;
    final words = tester
        .getTopLeft(find.text('They want the deck. I will answer.'))
        .dy;
    final mine = tester.getTopLeft(find.text('Here it is.')).dy;
    final sent = tester.getTopLeft(find.text('Sent.')).dy;
    expect(theirs, lessThan(words));
    expect(words, lessThan(mine));
    expect(mine, lessThan(sent));
  });

  testWidgets("the other side's words show what reorders text by its "
      'code', (tester) async {
    await show(
      tester,
      entries: [
        _event(1, 2000, [1]),
      ],
      messages: [
        _message(
          1,
          2000,
          'Approve evil\u202Etxt.exe',
          PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_AGENT,
          name: 'B\u202Eo',
        ),
      ],
    );
    expect(find.text(r'Approve evil\u{202E}txt.exe'), findsOneWidget);
    expect(
      find.textContaining(r'B\u{202E}o', findRichText: true),
      findsWidgets,
    );
    expect(find.textContaining('\u202E', findRichText: true), findsNothing);
  });

  testWidgets("the person's words to the liaison say they went to it "
      'only, not through the pipe', (tester) async {
    await show(
      tester,
      entries: [
        Entry(
          seq: Int64(1),
          timestampMs: Int64(2000),
          user: UserText(
            text: 'Tell Ana hi',
            sentFrom: SentFrom(
              deviceName: 'MacBook',
              kind: DeviceKind.DEVICE_KIND_LAPTOP,
            ),
          ),
        ),
        _say(2, 2100, 'I will.'),
      ],
    );
    expect(find.text('Tell Ana hi'), findsOneWidget);
    expect(find.text('To Kai only'), findsOneWidget);
    // Under its bubble.
    expect(
      tester.getTopLeft(find.text('To Kai only')).dy,
      greaterThan(tester.getBottomLeft(find.text('Tell Ana hi')).dy),
    );
  });

  testWidgets('the box writes to the liaison; picking the pipe carries the '
      'draft over, sends it through the pipe, then goes back to the '
      'liaison', (tester) async {
    final harness = await show(tester);
    expect(
      find.text("Goes to Kai, who can pass it on to Ana's Claktory."),
      findsOneWidget,
    );

    await tester.enterText(field(), 'Ask them for dates');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(harness.last.sendMessage.agentId, 'kai');
    expect(harness.last.sendMessage.text, 'Ask them for dates');

    await tester.enterText(field(), 'Hello Ana');
    await tester.pump();
    await tester.tap(find.text("To Ana's Claktory"));
    await settle(tester);
    expect(find.text("Ana's Claktory sees this."), findsOneWidget);
    // What was written goes with the choice.
    expect(tester.widget<EditableText>(field()).controller.text, 'Hello Ana');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(harness.last.sendPipeMessage.pipeId, 'pipe1');
    expect(harness.last.sendPipeMessage.text, 'Hello Ana');
    // The next message goes to the liaison again.
    expect(
      find.text("Goes to Kai, who can pass it on to Ana's Claktory."),
      findsOneWidget,
    );
  });

  testWidgets('with Review outgoing on, the box says what the liaison passes '
      'on waits for you (friend bug #1)', (tester) async {
    await show(tester, reviewOutgoing: true);
    expect(
      find.text(
        "Goes to Kai. If Kai passes it on to Ana's Claktory, you approve it "
        'first.',
      ),
      findsOneWidget,
    );
  });

  testWidgets("a message the pipe refuses comes back to the box, saying why", (
    tester,
  ) async {
    final harness = await show(tester);
    await tester.tap(find.text("To Ana's Claktory"));
    await settle(tester);
    harness.backend.refuse = 'Over the pipe’s limit.';
    await tester.enterText(field(), 'Hello Ana');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(find.text('Over the pipe’s limit.'), findsOneWidget);
    expect(tester.widget<EditableText>(field()).controller.text, 'Hello Ana');
  });

  testWidgets('a closed pipe: its bar, no pipe to write to, and Delete… '
      'says the liaison goes and where its conversation is kept', (
    tester,
  ) async {
    final harness = await show(tester, state: PipeState.PIPE_STATE_CLOSED);
    expect(
      find.text('This pipe is closed. Its messages stay here.'),
      findsOneWidget,
    );
    expect(find.text("To Ana's Claktory"), findsNothing);
    expect(
      find.text("Goes to Kai, who can pass it on to Ana's Claktory."),
      findsOneWidget,
    );

    await tester.tap(find.text('Delete…'));
    await settle(tester);
    expect(
      find.textContaining(
        'Kai goes with it, and its conversation is saved in the project’s '
        'folder, in pipes/acme-launch-ab12cd/conversation.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Delete pipe'));
    await settle(tester);
    expect(harness.last.setPipeState.action, PipeAction.PIPE_ACTION_DELETE);
  });

  testWidgets("the header's More clears the liaison's conversation", (
    tester,
  ) async {
    final harness = await show(tester);
    await tester.tap(find.bySemanticsLabel('More'));
    await settle(tester);
    expect(find.text('Liaison settings…'), findsOneWidget);
    await tester.tap(find.text('Clear conversation…'));
    await settle(tester);
    await tester.tap(find.text('Clear conversation'));
    await settle(tester);
    expect(harness.last.runCommand.agentId, 'kai');
    expect(harness.last.runCommand.name, 'clear');
  });

  testWidgets("with the liaison's whole conversation held, the pipe's "
      'earlier messages load too', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    var loads = 0;
    harness.backend.onSend = (message) {
      if (!message.hasLoadPipeMessages()) return;
      loads++;
      harness.backend.pipeChat.prepend(
        PipeMessagePage(
          pipeId: 'pipe1',
          messages: [
            _message(
              1,
              500,
              'The very first',
              PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_AGENT,
              name: 'Bo',
            ),
          ],
        ),
      );
    };
    harness.backend.transcript.load(
      Transcript(agentId: 'kai', entries: [_say(1, 3000, 'Hello')]),
    );
    harness.backend.pipeChat.load(
      PipeTranscript(
        pipeId: 'pipe1',
        hasEarlier: true,
        messages: [
          _message(
            2,
            2000,
            'Later',
            PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_AGENT,
            name: 'Bo',
          ),
        ],
      ),
    );
    await settle(tester);
    await settle(tester);
    expect(loads, 1);
    expect(find.text('The very first'), findsOneWidget);
  });

  testWidgets("Needs you: the liaison's question is the pipe's, and a held "
      'message opens at its card among the conversation', (tester) async {
    final snapshot = _workspace()
      ..selectedPipeId = ''
      ..selectedAgentId = '';
    snapshot.agents[1].waitingSeqs.add(Int64(7));
    snapshot.pipes[0].waitingSeqs.add(Int64(3));
    final harness = Harness(snapshot);
    harness.backend.entriesById['kai/7'] = Entry(
      seq: Int64(7),
      timestampMs: Int64(4000),
      askPerson: AskPerson(
        id: 'ask7',
        title: 'May I share the launch date?',
        state: AskPersonState.ASK_PERSON_STATE_PENDING,
      ),
    );
    harness.backend.pipeMessagesById['pipe1/3'] = _message(
      3,
      3000,
      'Please send the deck',
      PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_AGENT,
      name: 'Bo',
      state: PipeMessageState.PIPE_MESSAGE_STATE_HELD_INCOMING,
    );
    await harness.show(tester);
    await settle(tester);
    await tester.tap(find.text('Needs you'));
    await settle(tester);
    await settle(tester);

    expect(
      find.textContaining('Kai · Acme launch', findRichText: true),
      findsOneWidget,
    );
    expect(
      find.text("Let in a message from Ana's Claktory?\nPlease send the deck"),
      findsOneWidget,
    );

    // The liaison's question opens in its pipe.
    await tester.tap(
      find.textContaining('Kai · Acme launch', findRichText: true),
    );
    await pumpFrames(tester);
    await tester.tap(find.text('Open pipe'));
    await settle(tester);
    expect(harness.last.selectPipe.pipeId, 'pipe1');
    expect(harness.backend.reveal.value?.seq, 7);

    await tester.tap(find.text('Needs you'));
    await settle(tester);
    await settle(tester);
    await tester.tap(
      find.text("Let in a message from Ana's Claktory?\nPlease send the deck"),
    );
    await pumpFrames(tester);
    await tester.tap(find.text('Open pipe'));
    await settle(tester);
    expect(harness.last.selectPipe.pipeId, 'pipe1');
    expect(harness.backend.reveal.extra?.key, 'pipe:3');
    expect(harness.backend.reveal.extra?.agentId, 'kai');
  });

  testWidgets("on a phone at the largest text, the pipe's header keeps its "
      "name, with where it stands on one line, the liaison's face and its "
      'More', (tester) async {
    final harness = Harness(_workspace());
    var menus = 0;
    tester.view.physicalSize = const Size(390, 844);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    tester.platformDispatcher.textScaleFactorTestValue = 2;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xFF000000),
        // The sheets (the liaison's More) open in an overlay.
        builder: (context, _) => Overlay(
          initialEntries: [
            OverlayEntry(
              builder: (context) => AppScope(
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
                camera: harness.camera,
                child: PipeView(pipe: _pipe(), onMenu: () => menus++),
              ),
            ),
          ],
        ),
      ),
    );
    await settle(tester);
    expect(tester.takeException(), isNull);
    expect(find.text('Acme launch'), findsOneWidget);
    expect(
      find.text('Online · Not verified · 1 agent can run commands'),
      findsOneWidget,
    );
    await tester.tap(find.bySemanticsLabel('Menu'));
    await settle(tester);
    expect(menus, 1);
    await tester.tap(find.bySemanticsLabel('Kai: more'));
    await settle(tester);
    expect(find.text('Clear conversation…'), findsOneWidget);
  });

  test('what the cards already say is left out of the conversation', () {
    expect(hiddenInPipe(_event(1, 0, [1]), {}), isTrue);
    expect(
      hiddenInPipe(
        _tool(2, 0, 'pipe_send', details: '{"pipe":"pipe1","seq":4}'),
        {4},
      ),
      isTrue,
    );
    // Its card not loaded: the call shows.
    expect(
      hiddenInPipe(
        _tool(2, 0, 'pipe_send', details: '{"pipe":"pipe1","seq":4}'),
        {3},
      ),
      isFalse,
    );
    expect(hiddenInPipe(_tool(3, 0, 'pipe_read'), {4}), isFalse);
    expect(hiddenInPipe(_say(4, 0, 'Hi'), {4}), isFalse);
    expect(
      pipeSendSeq(
        ToolCall(
          name: 'pipe_send',
          details: '{"pipe":"p","seq":7,"held":true}',
        ),
      ),
      7,
    );
    expect(pipeSendSeq(ToolCall(name: 'pipe_send', details: 'nope')), isNull);
  });
}
