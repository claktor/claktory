// Quiet messages: an agent's message sent with quiet on never interrupts
// its recipient. It says "Quiet" under it once delivered; while it waits,
// the agent's header and sidebar row say how many wait, and the
// conversation's queued tail says when it's read; the sender's line says
// "Sent quietly".
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/quiet_copies.dart';
import 'package:factory_app/transcript.dart' show TranscriptView;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'messaging_test.dart' show workspace;
import 'tool_views_test.dart' show call, inTranscript, showing;

/// Builder's workspace (a1) with [waiting] quiet messages waiting for it.
Workspace _workspace({int waiting = 0}) {
  final snapshot = workspace();
  // A short name: the test font draws every letter an em wide.
  snapshot.agents.firstWhere((agent) => agent.id == 'a1')
    ..name = 'Kai'
    ..quietWaiting = waiting;
  return snapshot;
}

String _name(Workspace workspace) =>
    workspace.agents.firstWhere((agent) => agent.id == 'a1').name;

void main() {
  test('how many, in words', () {
    expect(quietWaitingLabel(1), '1 quiet message');
    expect(quietWaitingLabel(4), '4 quiet messages');
    expect(quietWaitingShort(4), '4 quiet');
    expect(
      quietWaitingAbout(Agent(name: 'Kai', quietWaiting: 4)),
      '4 quiet messages waiting. Kai reads them the next time it starts '
      'working.',
    );
    expect(
      quietWaitingAbout(Agent(name: 'Kai', quietWaiting: 1)),
      '1 quiet message waiting. Kai reads it the next time it starts working.',
    );
    expect(provenanceLine('status', 0, quiet: true), 'Quiet · Status update');
    expect(provenanceLine('', 0, quiet: true), 'Quiet');
  });

  testWidgets('a quiet message, delivered, says Quiet under it', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            event: Event(
              source: EventSource.EVENT_SOURCE_AGENT,
              from: 'Kenji (Frontend)',
              fromId: 'kenji',
              text: 'Landed the header fix.',
              intent: 'status',
              quiet: true,
            ),
          ),
          Entry(
            seq: Int64(2),
            event: Event(
              source: EventSource.EVENT_SOURCE_AGENT,
              from: 'Kenji (Frontend)',
              fromId: 'kenji',
              text: 'Please review.',
            ),
          ),
        ],
      ),
    );
    await harness.show(tester, size: const Size(1400, 1600));
    await settle(tester);
    expect(find.text('Quiet · Status update'), findsOneWidget);
    expect(find.textContaining('Quiet'), findsOneWidget, reason: 'one only');
  });

  testWidgets('the header and sidebar say how many wait, until none do', (
    tester,
  ) async {
    final harness = Harness(_workspace(waiting: 4));
    await harness.show(tester, size: const Size(1400, 1000));
    await settle(tester);
    final name = _name(_workspace());
    expect(find.text('4 quiet messages'), findsOneWidget);
    expect(find.text('4 quiet'), findsOneWidget, reason: 'its sidebar row');
    expect(
      find.bySemanticsLabel(
        '4 quiet messages waiting. $name reads them the next time it starts '
        'working.',
      ),
      findsWidgets,
    );
    // Not a button: tapping it sends nothing.
    harness.backend.sent.clear();
    await tester.tap(find.text('4 quiet messages'));
    await tester.pump();
    expect(harness.backend.sent, isEmpty);

    await harness.apply(tester, _workspace(waiting: 1));
    expect(find.text('1 quiet message'), findsOneWidget);
    await harness.apply(tester, _workspace());
    expect(find.byType(QuietWaitingChip), findsNothing);
    expect(find.text('1 quiet'), findsNothing);
    expect(find.text('1 quiet message'), findsNothing);
  });

  testWidgets("in the sidebar it gives way before a long name's cut", (
    tester,
  ) async {
    final snapshot = _workspace(waiting: 4);
    snapshot.agents.firstWhere((a) => a.id == 'a1').name =
        'Bartholomew Featherstonehaugh';
    await Harness(snapshot).show(tester, size: const Size(1400, 1000));
    await settle(tester);
    expect(find.text('4 quiet messages'), findsOneWidget, reason: 'header');
    expect(find.text('4 quiet'), findsNothing, reason: 'no room in the row');
    expect(
      find.bySemanticsLabel(RegExp('Bartholomew Featherstonehaugh reads them')),
      findsWidgets,
      reason: 'VoiceOver still says it',
    );
  });

  testWidgets('on iPhone, shorter, and nothing cut at the largest text', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final snapshot = _workspace(waiting: 128)..openPanel = '';
    snapshot.agents.firstWhere((a) => a.id == 'a1').openPanel = '';
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    await Harness(snapshot).show(tester, size: const Size(320, 568));
    await pumpFrames(tester);
    expect(find.byType(PhoneShell), findsOneWidget);
    expect(tester.takeException(), isNull);
    expect(find.text('99+ quiet'), findsOneWidget);
  });

  testWidgets("one still waiting says when it's read, not Queued", (
    tester,
  ) async {
    final harness = Harness(_workspace(waiting: 1));
    harness.backend.transcript.load(Transcript(agentId: 'a1'));
    harness.backend.queued.apply(
      QueuedMessages(
        agentId: 'a1',
        messages: [
          QueuedMessage(
            id: Int64(1),
            source: 'agent',
            sender: 'Writer',
            senderId: 'a3',
            text: 'The notes are ready.',
            quiet: true,
          ),
          QueuedMessage(
            id: Int64(2),
            source: 'agent',
            sender: 'Writer',
            senderId: 'a3',
            text: 'And this one wakes it.',
          ),
        ],
      ),
    );
    await harness.show(tester, size: const Size(1400, 1600));
    await settle(tester);
    final name = _name(_workspace());
    expect(
      find.descendant(
        of: find.byType(TranscriptView),
        matching: find.text('Quiet · read when $name next starts working'),
      ),
      findsOneWidget,
    );
    expect(find.text('Queued'), findsOneWidget);
  });

  testWidgets('on iPhone, a long name wraps the waiting line at the '
      'largest text', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final snapshot = _workspace(waiting: 1)..openPanel = '';
    snapshot.agents.firstWhere((a) => a.id == 'a1')
      ..openPanel = ''
      ..name = 'Bartholomew Featherstonehaugh-Wolkenstein';
    final harness = Harness(snapshot);
    harness.backend.transcript.load(Transcript(agentId: 'a1'));
    harness.backend.queued.apply(
      QueuedMessages(
        agentId: 'a1',
        messages: [
          QueuedMessage(
            id: Int64(1),
            source: 'agent',
            sender: 'Writer',
            senderId: 'a3',
            text: 'The notes are ready.',
            quiet: true,
          ),
        ],
      ),
    );
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    expect(find.byType(PhoneShell), findsOneWidget);
    expect(tester.takeException(), isNull);
    expect(
      find.textContaining('Quiet · read when Bartholomew'),
      findsOneWidget,
    );
  });

  group("the sender's line", () {
    testWidgets('one recipient, idle or working: Sent quietly', (tester) async {
      await showing(tester, [
        for (final (seq, working) in [(1, false), (2, true)])
          call(
            'agents_send',
            {'to': 'builder', 'text': 'FYI', 'quiet': true},
            seq: seq,
            output: 'Sent quietly to Sam: …',
            details: {
              'to': 'builder',
              'name': 'Sam',
              'working': working,
              'quiet': true,
            },
          ),
      ]);
      expect(find.text('Sent quietly'), findsNWidgets(2));
      expect(find.text('Queued'), findsNothing);
    });

    testWidgets('several recipients, one failed', (tester) async {
      await showing(tester, [
        call(
          'agents_send',
          {
            'to': ['lena', 'omar', 'ghost'],
            'text': 'FYI',
            'quiet': true,
          },
          output: 'Lena (lena): sent quietly; …',
          details: {
            'quiet': true,
            'recipients': [
              {'id': 'lena', 'name': 'Lena', 'outcome': 'waiting'},
              {'id': 'omar', 'name': 'Omar', 'outcome': 'working'},
              {
                'to': 'ghost',
                'outcome': 'failed',
                'why': 'No agent is called ghost.',
              },
            ],
          },
        ),
      ]);
      expect(find.text('Sent quietly'), findsOneWidget);
      expect(inTranscript(find.text('Lena')), findsOneWidget);
      expect(inTranscript(find.text('Omar')), findsOneWidget);
    });
  });
}
