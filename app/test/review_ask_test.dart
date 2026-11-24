// #275: an agent asks the person to review a document. Its question card
// names the file, with Review, which opens it beside that agent's
// conversation, expanded, so every comment on it goes to the agent; the
// person ends it with Approve (a note if they like) or Dismiss.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/layout.dart' show leaves, viewerKindFor;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _path = '/Users/me/factory/coordination/security/threat-model.md';

Agent _agent(String id, String name, {List<int> waiting = const []}) => Agent(
  id: id,
  name: name,
  projectId: 'p1',
  modelAvailable: true,
  status: AgentStatus.AGENT_STATUS_IDLE,
  waitingSeqs: [for (final seq in waiting) Int64(seq)],
);

Workspace _workspace({Agent? fugo}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  agents: [
    fugo ?? _agent('fugo', 'Fugo', waiting: [7]),
    _agent('bruno', 'Bruno'),
  ],
  selectedAgentId: 'bruno',
);

Entry _review() => Entry(
  seq: Int64(7),
  timestampMs: Int64(DateTime.now().millisecondsSinceEpoch),
  askPerson: AskPerson(
    id: 'ask7',
    title: 'Review the threat model',
    body: 'Comment on anything that looks wrong.',
    state: AskPersonState.ASK_PERSON_STATE_PENDING,
    reviewPath: _path,
  ),
);

Future<Harness> _inNeedsYou(WidgetTester tester, Harness harness) async {
  harness.backend.entriesById['fugo/7'] = _review();
  await harness.show(tester);
  await settle(tester);
  harness.page.value = AppPage.inbox;
  await settle(tester);
  await settle(tester);
  await tester.tap(find.text('Review the threat model'));
  await pumpFrames(tester);
  return harness;
}

void main() {
  testWidgets('the card names the file; Review opens it beside the asking '
      "agent's conversation, expanded", (tester) async {
    final harness = await _inNeedsYou(tester, Harness(_workspace()));
    expect(find.text('threat-model.md'), findsOneWidget);
    expect(find.text('~/factory/coordination/security'), findsOneWidget);
    expect(find.text('Your comments on it go to Fugo.'), findsOneWidget);
    expect(find.text('Approve'), findsOneWidget);

    await tester.tap(find.text('Review'));
    await settle(tester);
    expect(harness.page.value, AppPage.agent);
    expect(
      harness.backend.sent.any((m) => m.selectAgent.agentId == 'fugo'),
      isTrue,
    );
    final set = harness.last.setLayout;
    expect(set.agentId, 'fugo');
    final viewer = [
      for (final leaf in leaves(set.root))
        if (leaf.leaf.path == _path) leaf,
    ];
    expect(viewer, hasLength(1));
    expect(set.focus, viewer.single.id);
  });

  testWidgets('already open and expanded, Review changes nothing', (
    tester,
  ) async {
    final open = ViewNode(
      id: 'root',
      split: ViewSplit(
        axis: ViewAxis.VIEW_AXIS_ROW,
        children: [
          ViewNode(
            id: 'chat',
            leaf: ViewLeaf(kind: 'chat'),
          ),
          ViewNode(
            id: 'v1',
            leaf: ViewLeaf(kind: 'text', path: _path),
          ),
        ],
      ),
    );
    final fugo = _agent('fugo', 'Fugo', waiting: [7])
      ..layout = open
      ..layoutFocus = 'v1';
    final harness = await _inNeedsYou(tester, Harness(_workspace(fugo: fugo)));
    await tester.tap(find.text('Review'));
    await settle(tester);
    expect(harness.backend.sent.any((m) => m.hasSetLayout()), isFalse);
  });

  testWidgets('a typed note is sent as a reply to the agent that asked', (
    tester,
  ) async {
    final harness = await _inNeedsYou(tester, Harness(_workspace()));
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    await tester.enterText(
      find.descendant(
        of: find.byType(AskPersonCard),
        matching: find.byType(EditableText),
      ),
      'Looks right',
    );
    await tester.pump();
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Dismiss'), findsNothing);
    await tester.tap(find.text('Submit'));
    await settle(tester);
    final answer = harness.last.answerAsk;
    expect(answer.agentId, 'fugo');
    expect(answer.seq.toInt(), 7);
    expect(answer.text, 'Looks right');
    expect(answer.reply, isTrue);
    expect(answer.dismiss, isTrue);
  });

  testWidgets('Approve with no comment approves', (tester) async {
    final harness = await _inNeedsYou(tester, Harness(_workspace()));
    await tester.tap(find.text('Approve'));
    await settle(tester);
    final answer = harness.last.answerAsk;
    expect(answer.agentId, 'fugo');
    expect(answer.text, '');
    expect(answer.dismiss, isFalse);
    expect(answer.reply, isFalse);
  });

  testWidgets('an ordinary question has no file', (tester) async {
    await tester.pumpWidget(const SizedBox());
    final harness = Harness(_workspace());
    harness.backend.entriesById['fugo/7'] = _review()
      ..askPerson.reviewPath = '';
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await settle(tester);
    await tester.tap(find.text('Review the threat model'));
    await settle(tester);
    expect(find.text('Review'), findsNothing);
    expect(find.text('Fugo carries on once you approve.'), findsOneWidget);
  });

  test('videos open in the video viewer, images in the image one, the '
      'rest as text', () {
    expect(viewerKindFor('/Users/me/demo.MP4'), 'video');
    expect(viewerKindFor('/Users/me/demo.mov'), 'video');
    expect(viewerKindFor('/Users/me/demo.m4v'), 'video');
    expect(viewerKindFor('/Users/me/shot.png'), 'image');
    expect(viewerKindFor('/Users/me/notes.md'), 'text');
  });

  testWidgets('Review opens a video in a video pane', (tester) async {
    final harness = Harness(_workspace());
    final entry = _review()..askPerson.reviewPath = '/Users/me/demo.mp4';
    harness.backend.entriesById['fugo/7'] = entry;
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await settle(tester);
    await tester.tap(find.text('Review the threat model'));
    await pumpFrames(tester);
    await tester.tap(find.text('Review'));
    await settle(tester);
    final set = harness.last.setLayout;
    final leaf = leaves(set.root)
        .singleWhere((leaf) => leaf.leaf.path == '/Users/me/demo.mp4');
    expect(leaf.leaf.kind, 'video');
    expect(set.focus, leaf.id);
  });
}
