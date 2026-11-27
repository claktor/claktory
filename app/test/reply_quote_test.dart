// Replies linked to what they answer: over an agent's reply, the
// person's message it answers, as chat apps quote it; a click goes there.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/transcript.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

Entry _asked(int seq, String text) => Entry(
  seq: Int64(seq),
  user: UserText(text: text),
);

/// An agent's reply, answering the message [answers] when given.
Entry _answer(int seq, String text, {int? answers}) {
  final entry = Entry(
    seq: Int64(seq),
    assistant: AssistantText(text: text),
  );
  if (answers != null) entry.repliedTo = Int64(answers);
  return entry;
}

Entry _fromKenji(int seq) => Entry(
  seq: Int64(seq),
  event: Event(
    source: EventSource.EVENT_SOURCE_AGENT,
    from: 'Kenji',
    fromId: 'frontend-2',
    text: 'The build is green.',
  ),
);

Entry _ran(int seq) => Entry(
  seq: Int64(seq),
  toolCall: ToolCall(
    callId: 'c$seq',
    name: 'bash',
    input: '{"command": "ls"}',
    output: 'Cargo.toml',
    state: ToolCallState.TOOL_CALL_STATE_DONE,
  ),
);

Future<Harness> _show(
  WidgetTester tester,
  List<Entry> entries, {
  bool hasEarlier = false,
}) async {
  final harness = Harness(workspace(agents: [builder(working: false)]));
  harness.backend.transcript.load(
    Transcript(agentId: 'a1', entries: entries, hasEarlier: hasEarlier),
  );
  await harness.show(tester);
  return harness;
}

Finder _quote(String text) => find.descendant(
  of: find.byType(ReplyQuote),
  matching: find.textContaining(text, findRichText: true),
);

void main() {
  test("a reply's link is whether it's set, so seq 0 counts", () {
    expect(repliedTo(_answer(5, 'Hi', answers: 0)), 0);
    expect(repliedTo(_answer(5, 'Hi', answers: 3)), 3);
    expect(repliedTo(_answer(5, 'Hi')), isNull);
  });

  testWidgets('with messages between, a reply quotes what it answers', (
    tester,
  ) async {
    await _show(tester, [
      _asked(0, 'What time is the  launch?\nAnd where?'),
      _fromKenji(1),
      _ran(2),
      _answer(3, 'At noon, in Lyon.', answers: 0),
    ]);
    expect(_quote('You'), findsOneWidget);
    expect(
      _quote('What time is the launch? And where?'),
      findsOneWidget,
      reason: 'its words on one line',
    );
  });

  testWidgets('right under its question, a reply has no quote', (tester) async {
    await _show(tester, [
      _asked(1, 'What time is the launch?'),
      _ran(2),
      _answer(3, 'At noon.', answers: 1),
    ]);
    expect(_quote('You'), findsNothing);
  });

  testWidgets('a reply that answers nothing has no quote', (tester) async {
    await _show(tester, [
      _asked(1, 'What time is the launch?'),
      _fromKenji(2),
      _answer(3, 'At noon.'),
    ]);
    expect(find.byType(ReplyQuote), findsNothing);
  });

  testWidgets('a click on the quote goes to the message', (tester) async {
    await _show(tester, [
      _asked(1, 'What time is the launch?'),
      for (var seq = 2; seq < 40; seq++) _fromKenji(seq),
      _answer(40, 'At noon.', answers: 1),
    ]);
    final question = find.text('What time is the launch?');
    final view = tester.getRect(find.byType(TranscriptView));
    bool inView() =>
        question.evaluate().isNotEmpty &&
        view.overlaps(tester.getRect(question));
    expect(inView(), isFalse, reason: 'far above the end');
    await tester.tap(_quote('You'));
    await settle(tester);
    expect(inView(), isTrue);
  });

  testWidgets(
    "one older than what's held is an earlier message, and shows once it's "
    'paged in',
    (tester) async {
      final harness = await _show(tester, [
        _fromKenji(10),
        _answer(11, 'At noon.', answers: 4),
      ], hasEarlier: true);
      expect(_quote('Earlier message'), findsOneWidget);
      final asked = <Object?>[];
      harness.backend.reveal.addListener(
        () => asked.add(harness.backend.reveal.value),
      );
      await tester.tap(_quote('Earlier message'));
      await tester.pump();
      expect(asked.first, (agentId: 'a1', seq: 4));

      harness.backend.transcript.prepend('a1', [
        _asked(4, 'What time is the launch?'),
      ], hasEarlier: false);
      await tester.pump();
      expect(_quote('What time is the launch?'), findsOneWidget);
      expect(_quote('Earlier message'), findsNothing);
    },
  );

  testWidgets("a message with only files quotes the files' names", (
    tester,
  ) async {
    await _show(tester, [
      Entry(
        seq: Int64(1),
        user: UserText(
          attachments: [
            Attachment(name: 'plan.pdf'),
            Attachment(name: 'map.png'),
          ],
        ),
      ),
      _fromKenji(2),
      _answer(3, 'Read them.', answers: 1),
    ]);
    expect(_quote('plan.pdf, map.png'), findsOneWidget);
  });
}
