import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

Entry _thought(int seq, String text) =>
    Entry(seq: Int64(seq), thinking: Thinking(text: text));

Entry _reply(int seq, String text) =>
    Entry(seq: Int64(seq), assistant: AssistantText(text: text));

Entry _user(int seq, String text) =>
    Entry(seq: Int64(seq), user: UserText(text: text));

Future<Harness> _show(
  WidgetTester tester,
  List<Entry> entries, {
  bool working = false,
}) async {
  final harness = Harness(workspace(agents: [builder(working: working)]));
  harness.backend.transcript.load(
    Transcript(agentId: 'a1', entries: entries),
  );
  await harness.show(tester);
  await settle(tester);
  return harness;
}

void main() {
  testWidgets('thoughts in a row show as one line, opened to each as '
      'before', (tester) async {
    await _show(tester, [
      _user(1, 'Fix it'),
      _thought(2, 'First idea'),
      _thought(3, 'Second idea'),
      // A reply with no words between them shows nothing: still one run.
      _reply(4, ''),
      _thought(5, 'Third idea'),
      _reply(6, 'Done.'),
      _thought(7, 'Alone'),
    ]);
    expect(find.text('Thought 3 times'), findsOneWidget);
    // One thought on its own looks as it did.
    expect(find.text('Thought'), findsOneWidget);
    expect(find.text('First idea'), findsNothing);
    expect(
      find.bySemanticsLabel('Thought 3 times. Show thoughts.'),
      findsOneWidget,
    );

    await tester.tap(find.text('Thought 3 times'));
    await settle(tester);
    expect(
      find.bySemanticsLabel('Thought 3 times. Hide thoughts.'),
      findsOneWidget,
    );
    // Each thought as it shows alone, closed until clicked.
    expect(find.text('Thought'), findsNWidgets(4));
    expect(find.text('Second idea'), findsNothing);
    await tester.tap(find.text('Thought').at(1));
    await settle(tester);
    expect(find.text('Second idea'), findsOneWidget);

    await tester.tap(find.text('Thought 3 times'));
    await settle(tester);
    expect(find.text('Thought'), findsOneWidget);
    expect(find.text('Second idea'), findsNothing);
  });

  testWidgets('while the agent thinks, the line counts up in place', (
    tester,
  ) async {
    final harness = await _show(tester, [
      _user(1, 'Fix it'),
      _thought(2, 'First idea'),
    ], working: true);
    expect(find.text('Thinking…'), findsOneWidget);

    harness.backend.transcript.update(
      EntryUpdate(agentId: 'a1', entry: _thought(3, 'Second idea')),
    );
    await settle(tester);
    expect(find.text('Thinking… · 2 so far'), findsOneWidget);
    expect(find.text('Thinking…'), findsNothing);

    harness.backend.transcript.update(
      EntryUpdate(agentId: 'a1', entry: _thought(4, 'Third idea')),
    );
    await settle(tester);
    expect(find.text('Thinking… · 3 so far'), findsOneWidget);

    // Its answer comes: the thinking is done.
    harness.backend.transcript.update(
      EntryUpdate(agentId: 'a1', entry: _reply(5, 'Found it.')),
    );
    await settle(tester);
    expect(find.text('Thought 3 times'), findsOneWidget);
  });

  testWidgets('a tool call or a message between thoughts ends the run', (
    tester,
  ) async {
    await _show(tester, [
      _thought(1, 'One'),
      Entry(
        seq: Int64(2),
        toolCall: ToolCall(
          callId: 'c1',
          name: 'bash',
          input: '{"command": "ls"}',
          state: ToolCallState.TOOL_CALL_STATE_DONE,
        ),
      ),
      _thought(3, 'Two'),
      _user(4, 'Hurry'),
      _thought(5, 'Three'),
    ]);
    expect(find.text('Thought'), findsNWidgets(3));
    expect(find.textContaining('times'), findsNothing);
  });
}
