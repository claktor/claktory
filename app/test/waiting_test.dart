import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/transcript.dart';
import 'package:factory_app/waiting.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _height = 600.0;

Agent _agent(
  String id,
  String name, {
  String project = 'p1',
  String parent = '',
  String folder = '',
  bool collapsed = false,
  bool unseen = false,
  List<int> waiting = const [],
}) => Agent(
  id: id,
  name: name,
  projectId: project,
  parentId: parent,
  folderId: folder,
  collapsed: collapsed,
  unseen: unseen,
  modelAvailable: true,
  status: AgentStatus.AGENT_STATUS_IDLE,
  waitingSeqs: [for (final seq in waiting) Int64(seq)],
);

Workspace _workspace(List<Agent> agents, {String focused = ''}) => Workspace(
  projects: [
    Project(
      id: 'p1',
      name: 'Factory',
      path: '/Users/me/factory',
      folders: [ProjectFolder(id: 'docs', name: 'Docs', collapsed: true)],
    ),
    Project(id: 'p2', name: 'Web Site', path: '/Users/me/web'),
  ],
  agents: agents,
  focusedProjectId: focused,
);

Finder _labeled(String label) => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == label,
);

/// A line in the conversation that isn't the person's: what they haven't
/// answered yet.
Entry _entry(int seq) => Entry(
  seq: Int64(seq),
  notice: Notice(text: 'message $seq', level: NoticeLevel.NOTICE_LEVEL_INFO),
);

Transcript _transcript(String agentId, Iterable<int> seqs) => Transcript(
  agentId: agentId,
  entries: [for (final seq in seqs) _entry(seq)],
);

/// [text] is on screen, and near its top.
bool _atTop(WidgetTester tester, String text) {
  final found = find.text(text);
  if (found.evaluate().isEmpty) return false;
  final rect = tester.getRect(found);
  return rect.top >= 0 && rect.top < 120;
}

bool _onScreen(WidgetTester tester, String text, {double height = _height}) {
  final found = find.text(text);
  if (found.evaluate().isEmpty) return false;
  final rect = tester.getRect(found);
  return rect.top >= 0 && rect.bottom <= height;
}

void main() {
  test('a count reads 9+ past nine', () {
    expect(waitingCount(1), '1');
    expect(waitingCount(9), '9');
    expect(waitingCount(10), '9+');
    expect(
      waitingLabel('June', 1),
      'June has something waiting for your answer',
    );
    expect(
      waitingLabel('June', 3),
      'June has 3 things waiting for your answer',
    );
  });

  Future<Harness> show(WidgetTester tester, Workspace snapshot) async {
    final harness = Harness(snapshot);
    await harness.show(tester);
    await settle(tester);
    return harness;
  }

  testWidgets('an agent with something waiting shows how many in signal, in '
      'place of its unseen dot', (tester) async {
    await show(
      tester,
      _workspace([
        _agent('june', 'June', unseen: true, waiting: [5, 9]),
        _agent('lena', 'Lena', unseen: true),
        _agent('omar', 'Omar', waiting: [for (var i = 0; i < 12; i++) i]),
      ]),
    );

    expect(
      _labeled('June has 2 things waiting for your answer'),
      findsOneWidget,
    );
    expect(_labeled('June has something new'), findsNothing);
    expect(
      find.descendant(
        of: _labeled('June has 2 things waiting for your answer'),
        matching: find.text('2'),
      ),
      findsOneWidget,
    );
    // Only news: the dot, as before.
    expect(_labeled('Lena has something new'), findsOneWidget);
    expect(
      _labeled('Omar has 12 things waiting for your answer'),
      findsOneWidget,
    );
    // Omar's 12, and Needs you's 14 at the top of the sidebar.
    expect(find.text('9+'), findsNWidgets(2));
    expect(_labeled('14 things are waiting for your answer'), findsOneWidget);
  });

  testWidgets('what waits out of sight shows where it is folded away', (
    tester,
  ) async {
    await show(
      tester,
      _workspace([
        _agent('lead', 'Lead', collapsed: true),
        _agent('back', 'Back', parent: 'lead', waiting: [3]),
        _agent('writer', 'Writer', folder: 'docs', waiting: [7, 8]),
      ]),
    );

    // A collapsed agent speaks for those under it, a folded folder for its
    // agents.
    expect(
      _labeled('Lead has something waiting for your answer'),
      findsOneWidget,
    );
    expect(
      _labeled('Docs has 2 things waiting for your answer'),
      findsOneWidget,
    );
  });

  testWidgets('in a focused project, All projects shows what waits in the '
      'others', (tester) async {
    await show(
      tester,
      _workspace([
        _agent('june', 'June'),
        _agent('web', 'Web', project: 'p2', waiting: [1]),
      ], focused: 'p1'),
    );

    expect(
      _labeled('Something in another project is waiting for your answer'),
      findsOneWidget,
    );
  });

  testWidgets('opening an agent with something waiting goes to the oldest '
      'card, once', (tester) async {
    final harness = Harness(
      _workspace([
        _agent('june', 'June'),
        _agent('lena', 'Lena', waiting: [40, 70]),
      ])..selectedAgentId = 'june',
    );
    harness.backend.transcript.load(_transcript('june', [1, 2]));
    // As the backend does: the selection, then the agent's transcript,
    // before the request is answered.
    harness.backend.onSend = (message) {
      if (!message.hasSelectAgent()) return;
      harness.backend.workspace.apply(
        harness.backend.workspace.workspace.deepCopy()
          ..selectedAgentId = message.selectAgent.agentId,
      );
      harness.backend.transcript.load(
        _transcript('lena', [for (var i = 0; i < 100; i++) i]),
      );
    };
    await harness.show(tester);
    await settle(tester);

    await tester.tap(find.text('Lena'));
    await settle(tester);
    expect(_atTop(tester, 'message 40'), isTrue);
    expect(find.text('message 99'), findsNothing);
    expect(harness.backend.reveal.value, isNull);

    // A new message doesn't take it back there; opening it again does.
    await tester.drag(find.text('message 45'), const Offset(0, -4000));
    await settle(tester);
    expect(_onScreen(tester, 'message 99', height: 900), isTrue);
    await tester.tap(find.text('Lena').first);
    await pumpFrames(tester);
    expect(_atTop(tester, 'message 40'), isTrue);
  });

  /// Lena's conversation, [seqs], with the person's own message at each of
  /// [mine]; selecting an agent loads its conversation, as the backend does.
  Harness withConversations(
    Workspace snapshot,
    Map<String, Transcript> conversations,
  ) {
    final harness = Harness(snapshot);
    harness.backend.onSend = (message) {
      if (!message.hasSelectAgent()) return;
      final id = message.selectAgent.agentId;
      harness.backend.workspace.apply(
        harness.backend.workspace.workspace.deepCopy()..selectedAgentId = id,
      );
      final conversation = conversations[id];
      if (conversation != null) harness.backend.transcript.load(conversation);
    };
    return harness;
  }

  Transcript lenas({List<int> mine = const []}) => Transcript(
    agentId: 'lena',
    entries: [
      for (var i = 0; i < 100; i++)
        mine.contains(i)
            ? Entry(
                seq: Int64(i),
                user: UserText(text: 'message $i'),
              )
            : _entry(i),
    ],
  );

  testWidgets("a card the person has written past is old news: the "
      "conversation opens at its end, and the card's mark goes to it", (
    tester,
  ) async {
    final harness = withConversations(
      _workspace([
        _agent('june', 'June'),
        _agent('lena', 'Lena', waiting: [40]),
      ])..selectedAgentId = 'june',
      {
        'lena': lenas(mine: [60]),
      },
    );
    harness.backend.transcript.load(_transcript('june', [1, 2]));
    await harness.show(tester);
    await settle(tester);

    await tester.tap(find.text('Lena'));
    await pumpFrames(tester);
    expect(_onScreen(tester, 'message 99', height: 900), isTrue);
    expect(find.text('message 40'), findsNothing);

    await tester.tap(_labeled('Lena has something waiting for your answer'));
    await pumpFrames(tester);
    expect(_atTop(tester, 'message 40'), isTrue);
  });

  testWidgets('selecting the open agent again keeps the conversation where '
      'it is', (tester) async {
    final harness = withConversations(
      _workspace([_agent('june', 'June'), _agent('lena', 'Lena')])
        ..selectedAgentId = 'june',
      {'lena': lenas()},
    );
    harness.backend.transcript.load(_transcript('june', [1, 2]));
    await harness.show(tester);
    await settle(tester);
    await tester.tap(find.text('Lena'));
    await pumpFrames(tester);
    await tester.drag(find.text('message 95'), const Offset(0, 600));
    await pumpFrames(tester);
    final shown = [
      for (var i = 0; i < 100; i++)
        if (_onScreen(tester, 'message $i', height: 900)) i,
    ].first;
    final place = tester.getTopLeft(find.text('message $shown'));
    final loads = harness.backend.transcript.loads;

    await tester.tap(find.text('Lena').first);
    await pumpFrames(tester);
    expect(harness.backend.transcript.loads, loads, reason: 'not reloaded');
    expect(tester.getTopLeft(find.text('message $shown')), place);
  });

  testWidgets("switching back after changing the other agent's settings "
      'opens at the end', (tester) async {
    final harness = withConversations(
      _workspace([_agent('mike', 'Mike'), _agent('rivet', 'Rivet')])
        ..selectedAgentId = 'rivet',
      {
        'rivet': Transcript(
          agentId: 'rivet',
          entries: [for (var i = 0; i < 100; i++) _entry(i)],
        ),
        'mike': Transcript(
          agentId: 'mike',
          entries: [for (var i = 0; i < 100; i++) _entry(i)],
        ),
      },
    );
    harness.backend.transcript.load(
      Transcript(
        agentId: 'rivet',
        entries: [for (var i = 0; i < 100; i++) _entry(i)],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    expect(_onScreen(tester, 'message 99', height: 900), isTrue);

    // Mike's model changes, as its settings form does.
    await harness.apply(
      tester,
      harness.backend.workspace.workspace.deepCopy()
        ..agents.firstWhere((a) => a.id == 'mike').model =
            'anthropic/claude-fable-5',
    );
    await tester.tap(find.text('Mike'));
    await pumpFrames(tester);
    expect(_onScreen(tester, 'message 99', height: 900), isTrue);
    expect(find.text('message 0'), findsNothing);

    await tester.tap(find.text('Rivet'));
    await pumpFrames(tester);
    expect(_onScreen(tester, 'message 99', height: 900), isTrue);
    expect(find.text('message 0'), findsNothing);
  });

  group('the conversation', () {
    Future<void> showView(
      WidgetTester tester,
      TranscriptStore store,
      EntryReveal reveal, {
      Future<void> Function(int seq)? loadFrom,
    }) async {
      tester.view.physicalSize = const Size(900, _height);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      await tester.pumpWidget(
        WidgetsApp(
          color: const Color(0xFF000000),
          builder: (context, _) => Overlay(
            initialEntries: [
              OverlayEntry(
                builder: (context) => TranscriptView(
                  transcript: store,
                  working: false,
                  follow: ValueNotifier(0),
                  loadEarlier: () async {},
                  agentId: 'a',
                  reveal: reveal,
                  loadFrom: loadFrom,
                ),
              ),
            ],
          ),
        ),
      );
      await tester.pumpAndSettle();
    }

    testWidgets('opens at the entry it is sent to, from the first frame', (
      tester,
    ) async {
      final store = TranscriptStore()
        ..load(_transcript('a', [for (var i = 0; i < 200; i++) i]));
      final reveal = EntryReveal()..value = (agentId: 'a', seq: 120);
      await showView(tester, store, reveal);

      expect(_atTop(tester, 'message 120'), isTrue);
      expect(reveal.value, isNull);

      // New entries don't move it; it follows again from the end.
      store.update(EntryUpdate(agentId: 'a', entry: _entry(200)));
      await tester.pumpAndSettle();
      expect(_atTop(tester, 'message 120'), isTrue);
    });

    testWidgets("loads back to an entry it doesn't hold yet", (tester) async {
      final store = TranscriptStore()
        ..load(
          _transcript('a', [for (var i = 300; i < 400; i++) i])
            ..hasEarlier = true,
        );
      final reveal = EntryReveal();
      final asked = <int>[];
      await showView(
        tester,
        store,
        reveal,
        loadFrom: (seq) async {
          asked.add(seq);
          store.prepend('a', [
            for (var i = seq; i < 300; i++) _entry(i),
          ], hasEarlier: true);
        },
      );
      expect(_onScreen(tester, 'message 399'), isTrue);

      reveal.reach(Agent(id: 'a', waitingSeqs: [Int64(150)]));
      // Earlier ones than it are still loading, so nothing settles.
      for (var i = 0; i < 10; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }
      expect(asked, [150]);
      expect(_atTop(tester, 'message 150'), isTrue);
      expect(reveal.value, isNull);
    });

    testWidgets('goes to the card in the very next frame, with no frame '
        'of the old place in between', (tester) async {
      final store = TranscriptStore()
        ..load(_transcript('a', [for (var i = 0; i < 200; i++) i]));
      final reveal = EntryReveal();
      await showView(tester, store, reveal);
      expect(_onScreen(tester, 'message 199'), isTrue);
      reveal.reach(Agent(id: 'a', waitingSeqs: [Int64(120)]));
      await tester.pump();
      expect(_atTop(tester, 'message 120'), isTrue);
    });

    testWidgets('opening an agent leaves a card older than what it opened '
        'with where it is', (tester) async {
      final store = TranscriptStore()
        ..load(
          _transcript('a', [for (var i = 300; i < 400; i++) i])
            ..hasEarlier = true,
        );
      final reveal = EntryReveal();
      final asked = <int>[];
      await showView(
        tester,
        store,
        reveal,
        loadFrom: (seq) async => asked.add(seq),
      );
      reveal.oldestWaiting(Agent(id: 'a', waitingSeqs: [Int64(150)]));
      await tester.pumpAndSettle();
      expect(asked, isEmpty);
      expect(_onScreen(tester, 'message 399'), isTrue);
      expect(reveal.value, isNull);
    });

    testWidgets("another agent's entry is left for its own conversation", (
      tester,
    ) async {
      final store = TranscriptStore()
        ..load(_transcript('a', [for (var i = 0; i < 200; i++) i]));
      final reveal = EntryReveal();
      await showView(tester, store, reveal);

      reveal.value = (agentId: 'b', seq: 20);
      await tester.pumpAndSettle();
      expect(_onScreen(tester, 'message 199'), isTrue);
      expect(reveal.value, isNotNull);
    });
  });
}
