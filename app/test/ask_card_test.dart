// An agent's question card (#238): its words select and copy wherever it
// shows, in a conversation and in Needs you, and a command in its body has
// Copy beside it.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'selection_test.dart' show copyItem, rightClick, watchClipboard;

const _command = 'sudo launchctl kickstart -k system/com.example.helper';

AskPerson _ask() => AskPerson(
  id: 'ask7',
  title: 'Restart the helper as root',
  body:
      'It stopped answering after the update. Run this in Terminal:\n\n'
      '```sh\n$_command\n```\n\n'
      'Then tell me when it **reports** ready.',
  state: AskPersonState.ASK_PERSON_STATE_PENDING,
);

Workspace _workspace({String selected = 'june'}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      projectId: 'p1',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      waitingSeqs: [Int64(7)],
    ),
  ],
  selectedAgentId: selected,
);

Entry _entry() =>
    Entry(seq: Int64(7), timestampMs: Int64(1000), askPerson: _ask());

/// The selectable regions [finder] is in.
int _regions(WidgetTester tester, Finder finder) => find
    .ancestor(of: finder, matching: find.byType(SelectableRegion))
    .evaluate()
    .length;

void main() {
  test("an ask's body splits at its fenced commands", () {
    expect(askBodyParts(_ask().body), [
      (
        text: 'It stopped answering after the update. Run this in Terminal:',
        command: false,
      ),
      (text: _command, command: true),
      (text: 'Then tell me when it **reports** ready.', command: false),
    ]);
    expect(askBodyParts('Just words, `inline` too.'), [
      (text: 'Just words, `inline` too.', command: false),
    ]);
    expect(askBodyParts('~~~\nls -la\n~~~'), [(text: 'ls -la', command: true)]);
    // An empty block is nothing to copy.
    expect(askBodyParts('Before\n```\n```\nAfter'), [
      (text: 'Before', command: false),
      (text: 'After', command: false),
    ]);
  });

  testWidgets('in Needs you its words select and copy, and Copy copies the '
      'command', (tester) async {
    final copied = watchClipboard(tester);
    final harness = Harness(_workspace(selected: ''));
    harness.backend.entriesById['june/7'] = _entry();
    await harness.show(tester);
    await settle(tester);
    await tester.tap(find.text('Needs you'));
    await settle(tester);
    await settle(tester);
    expect(find.byType(InboxView), findsOneWidget);
    await tester.tap(find.text('Restart the helper as root'));
    await settle(tester);
    // Opened all the way.
    await tester.pump(const Duration(seconds: 1));
    expect(find.byType(AskPersonCard), findsOneWidget);

    final title = find.descendant(
      of: find.byType(AskPersonCard),
      matching: find.text('Restart the helper as root'),
    );
    expect(_regions(tester, title), 1);
    // On a word of it (the line is the card's width).
    await rightClick(
      tester,
      title,
      at: tester.getTopLeft(title) + const Offset(20, 9),
    );
    await tester.tap(copyItem);
    await tester.pump();
    expect(copied.last, isNotEmpty);
    expect('Restart the helper as root', contains(copied.last));

    await tester.tap(find.text('Copy'));
    await tester.pump();
    expect(copied.last, _command);
    expect(find.text('Copied'), findsOneWidget);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets("in a conversation it's part of the conversation's selection", (
    tester,
  ) async {
    final harness = Harness(_workspace());
    harness.backend.transcript.load(
      Transcript(agentId: 'june', entries: [_entry()]),
    );
    await harness.show(tester);
    await settle(tester);
    final title = find.text('Restart the helper as root');
    expect(title, findsOneWidget);
    // The conversation's own region, not one of the card's inside it.
    expect(_regions(tester, title), 1);
    expect(find.text(_command), findsOneWidget);
    expect(find.text('Copy'), findsOneWidget);
  });

  testWidgets('on a phone at the largest text, Copy goes under the command', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final harness = Harness(_workspace());
    harness.backend.transcript.load(
      Transcript(agentId: 'june', entries: [_entry()]),
    );
    await harness.show(tester, size: const Size(320, 700));
    await settle(tester);
    final code = find.text(_command);
    await tester.ensureVisible(code);
    await settle(tester);
    expect(
      tester.getTopLeft(find.text('Copy')).dy,
      greaterThan(tester.getTopLeft(code).dy),
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('from an agent that read a pipe, it says where it may come '
      'from, under who asks', (tester) async {
    final workspace = _workspace()
      ..pipes.add(
        Pipe(
          id: 'pp',
          name: 'Acme launch',
          peer: PipePeer(claktoryName: "Ana's Claktory"),
        ),
      );
    final harness = Harness(workspace);
    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          _entry()
            ..askPerson.taint.addAll([
              PipeTaint(pipeName: 'Acme launch', pipeId: 'pp'),
              PipeTaint(
                pipeName: 'Old pipe',
                pipeId: 'gone',
                viaNames: ['Liaison', 'Bruno'],
              ),
            ]),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    final read = find.text(
      "from June, which reads the pipe with Ana's Claktory",
    );
    final heard = find.text(
      'from Bruno, which heard from the pipe “Old pipe” via Liaison',
    );
    expect(read, findsOneWidget);
    expect(heard, findsOneWidget);
    final header = tester.getTopLeft(find.text('June needs your answer')).dy;
    final title = tester.getTopLeft(find.text('Restart the helper as root')).dy;
    expect(tester.getTopLeft(read).dy, inExclusiveRange(header, title));
  });

  testWidgets('from an agent that read no pipe, no such line', (tester) async {
    final harness = Harness(_workspace());
    harness.backend.transcript.load(
      Transcript(agentId: 'june', entries: [_entry()]),
    );
    await harness.show(tester);
    await settle(tester);
    expect(find.textContaining('which reads the pipe'), findsNothing);
  });
}
