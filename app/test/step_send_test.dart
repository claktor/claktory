// #533 (Squalo's needs-you-card-stories.md §1, N1, N2, N5, N6): a step takes
// words without deciding (Send: the step stays, its agent fixes or answers
// it), Return in its field never presses Done, the talk shows above the
// step it's about, an Execute card's Dismiss can say why (Execute never
// takes words), and words typed on a card taken back stay, to send to its
// agent or copy.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/decision.dart' show DecisionComment;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
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
  selectedAgentId: 'june',
);

/// June's procedure at step 2 of 3.
AskPerson _procedure({
  List<AskThreadMessage> thread = const [],
  AskPersonState state = AskPersonState.ASK_PERSON_STATE_PENDING,
  bool replied = false,
  String answer = '',
  bool withdrawn = false,
}) => AskPerson(
  id: 'ask7',
  title: 'Turn on the helper',
  body: 'Three steps in System Settings.',
  state: state,
  replied: replied,
  answer: answer,
  withdrawn: withdrawn,
  revision: 2,
  thread: thread,
  steps: [
    AskStep(
      text: 'Open System Settings',
      state: AskStepState.ASK_STEP_STATE_DONE,
      decidedMs: Int64(1000),
    ),
    AskStep(text: 'Choose Login Items'),
    AskStep(text: 'Turn on Claktory'),
  ],
);

AskPerson _execute() => AskPerson(
  id: 'ask7',
  title: 'Clean the build',
  body: 'It takes 40 GB.',
  state: AskPersonState.ASK_PERSON_STATE_PENDING,
  revision: 2,
  command: AskCommand(
    command: 'rm -rf build && make',
    cwd: '/Users/me/factory',
    machine: "Matt's Mac Studio",
    timeoutSeconds: 120,
  ),
);

void _load(Harness harness, AskPerson ask) => harness.backend.transcript.load(
  Transcript(
    agentId: 'june',
    entries: [Entry(seq: Int64(7), timestampMs: Int64(900), askPerson: ask)],
  ),
);

Future<Harness> _show(WidgetTester tester, AskPerson ask) async {
  final harness = Harness(_workspace());
  _load(harness, ask);
  await harness.show(tester, size: const Size(1200, 900));
  await settle(tester);
  return harness;
}

Finder _inCard(Finder finder) =>
    find.descendant(of: find.byType(AskPersonCard), matching: finder);

final _comment = find.descendant(
  of: find.byType(DecisionComment),
  matching: find.byType(EditableText),
);

Future<void> _type(WidgetTester tester, String words) async {
  await tester.enterText(_comment, words);
  await settle(tester);
}

Button _button(WidgetTester tester, String label) =>
    tester.widget<Button>(_inCard(find.widgetWithText(Button, label)));

int _answers(Harness harness) =>
    harness.backend.sent.where((message) => message.hasAnswerAsk()).length;

void main() {
  testWidgets('a step with words typed has Send, which replies and marks '
      'nothing; the line says the step waits for its agent', (tester) async {
    final harness = await _show(tester, _procedure());
    expect(_inCard(find.text('Send')), findsNothing);
    expect(_button(tester, 'Done').primary, isTrue);

    await _type(tester, "The menu isn't there");
    expect(_inCard(find.text("Can't do this")), findsOneWidget);
    expect(_button(tester, 'Done').primary, isFalse);
    expect(_button(tester, 'Send').primary, isTrue);
    expect(
      _inCard(
        find.text(
          "Send gives June your words. Step 2 waits for June's answer.",
        ),
      ),
      findsOneWidget,
    );

    await tester.tap(_inCard(find.text('Send')));
    await settle(tester);
    final answer = harness.last.answerAsk;
    expect(answer.reply, isTrue);
    expect(answer.hasStep(), isFalse);
    expect(answer.text, "The menu isn't there");
  });

  testWidgets("Return in a step's comment sends the words, never Done; "
      'with nothing typed it does nothing (N1)', (tester) async {
    final harness = await _show(tester, _procedure());
    await tester.showKeyboard(_comment);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(_answers(harness), 0);

    await tester.enterText(_comment, "The menu isn't there");
    await settle(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(_answers(harness), 1);
    expect(harness.last.answerAsk.reply, isTrue);
    expect(harness.last.answerAsk.hasStep(), isFalse);
  });

  testWidgets('Done with words typed still marks the step done, the words '
      'its comment', (tester) async {
    final harness = await _show(tester, _procedure());
    await _type(tester, 'Added the passkey too');
    await tester.tap(_inCard(find.text('Done')));
    await settle(tester);
    final answer = harness.last.answerAsk;
    expect((answer.step, answer.reply, answer.dismiss), (1, false, false));
    expect(answer.text, 'Added the passkey too');
  });

  testWidgets("a step's talk shows above the step, under the steps done; "
      'Resolve is there, Reply is Send', (tester) async {
    await _show(
      tester,
      _procedure(
        thread: [
          AskThreadMessage(text: "The menu isn't there", atMs: Int64(2000)),
          AskThreadMessage(
            fromAgent: true,
            text: "It's under General now: I changed the step.",
            atMs: Int64(3000),
          ),
        ],
      ),
    );
    final done = tester.getTopLeft(
      _inCard(find.textContaining('1 step done', findRichText: true)),
    );
    final talk = tester.getTopLeft(
      _inCard(find.text("It's under General now: I changed the step.")),
    );
    final step = tester.getTopLeft(_inCard(find.text('Step 2 of 3')));
    expect(done.dy, lessThan(talk.dy));
    expect(talk.dy, lessThan(step.dy));
    expect(_inCard(find.text('Resolve')), findsOneWidget);
    expect(_inCard(find.text('Reply')), findsNothing);
  });

  testWidgets('a turn about another step than the one waiting says which; '
      "one about the step waiting doesn't", (tester) async {
    await _show(
      tester,
      _procedure(
        thread: [
          AskThreadMessage(
            text: 'Which settings, the old ones?',
            atMs: Int64(1500),
            step: 1,
          ),
          AskThreadMessage(
            fromAgent: true,
            text: 'The new ones.',
            atMs: Int64(1600),
            step: 1,
          ),
          AskThreadMessage(
            text: "The menu isn't there",
            atMs: Int64(2000),
            step: 2,
          ),
          AskThreadMessage(
            fromAgent: true,
            text: 'Under General now.',
            atMs: Int64(3000),
            step: 2,
          ),
        ],
      ),
    );
    expect(
      _inCard(find.textContaining('On step 1', findRichText: true)),
      findsNWidgets(2),
    );
    expect(
      _inCard(find.textContaining('On step 2', findRichText: true)),
      findsNothing,
    );
  });

  testWidgets('sent on a step, the card says where it waits', (tester) async {
    await _show(
      tester,
      _procedure(
        state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
        replied: true,
        answer: "The menu isn't there",
      ),
    );
    expect(
      _inCard(
        find.textContaining(
          'Replied on step 2 · waiting for June',
          findRichText: true,
        ),
      ),
      findsOne,
    );
    expect(_inCard(find.text("The menu isn't there")), findsOneWidget);
  });

  testWidgets("an Execute card's comment goes with Dismiss, never with "
      'Execute (N5)', (tester) async {
    final harness = await _show(tester, _execute());
    expect(
      _inCard(find.text('Why not? June sees this only if you dismiss.')),
      findsOneWidget,
    );
    expect(_inCard(find.text('June sees what it prints.')), findsOneWidget);
    await tester.enterText(_comment, 'Not on main, use the worktree');
    await settle(tester);
    expect(
      _inCard(
        find.text(
          'Your words go only with Dismiss. Execute runs the command exactly '
          'as shown.',
        ),
      ),
      findsOneWidget,
    );
    await tester.tap(_inCard(find.text('Execute')));
    await settle(tester);
    var answer = harness.last.answerAsk;
    expect((answer.confirmed, answer.dismiss, answer.text), (true, false, ''));

    // Dismissed instead, the words go with it.
    _load(harness, _execute());
    await settle(tester);
    // The field stays open on the card.
    await tester.enterText(_comment, 'Not on main, use the worktree');
    await settle(tester);
    await tester.tap(_inCard(find.text('Dismiss')));
    await settle(tester);
    answer = harness.last.answerAsk;
    expect(answer.dismiss, isTrue);
    expect(answer.confirmed, isFalse);
    expect(answer.text, 'Not on main, use the worktree');
  });

  testWidgets('words typed on a card its agent takes back stay, with Copy '
      'and Send to its agent (N6)', (tester) async {
    final harness = await _show(tester, _procedure());
    await _type(tester, 'Half of what I meant to say');
    _load(harness, _procedure(withdrawn: true));
    await settle(tester);
    expect(_inCard(find.textContaining('Withdrawn by June')), findsOne);
    expect(
      _inCard(
        find.textContaining(
          'Not sent: Half of what I meant to say',
          findRichText: true,
        ),
      ),
      findsOne,
    );

    String? copied;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          copied = (call.arguments as Map)['text'] as String?;
        }
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    await tester.tap(_inCard(find.text('Copy')));
    await tester.pump();
    expect(copied, 'Half of what I meant to say');
    await settle(tester);

    expect(_answers(harness), 0);
    await tester.tap(_inCard(find.text('Send to June')));
    await settle(tester);
    final message = harness.backend.sent.lastWhere(
      (message) => message.hasSendMessage(),
    );
    expect(message.sendMessage.agentId, 'june');
    expect(message.sendMessage.text, 'Half of what I meant to say');
    expect(_inCard(find.text('Sent to June.')), findsOneWidget);
    expect(_inCard(find.textContaining('Not sent')), findsNothing);
  });
}
