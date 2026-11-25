// A conversation on a card (Matt via Bruno 29397, Ghiaccio's ask_reply): a
// card its agent replied on shows the talk so far, oldest first, above its
// own buttons, with Reply and Resolve beneath them; Resolve ends it with no
// decision ("Resolved", never "Approved"); every answer says where it came
// from; a reopened card rises in Needs you by its thread's last message;
// History says Resolved.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/decision.dart' show DecisionComment;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _now = DateTime.now();
int _ago(Duration ago) => _now.subtract(ago).millisecondsSinceEpoch;

const _mine = 'Why the EU bucket and not the US one?';
const _theirs = 'The customer data has to stay in the EU.';

List<AskThreadMessage> _thread() => [
  AskThreadMessage(text: _mine, atMs: Int64(_ago(const Duration(minutes: 30)))),
  AskThreadMessage(
    fromAgent: true,
    text: _theirs,
    atMs: Int64(_ago(const Duration(minutes: 20))),
  ),
];

AskPerson _ask({
  bool wantsText = false,
  List<AskThreadMessage>? thread,
  AskPersonState state = AskPersonState.ASK_PERSON_STATE_PENDING,
  bool resolved = false,
  String answer = '',
  List<AskStep> steps = const [],
}) => AskPerson(
  id: 'ask7',
  title: 'Use the EU bucket?',
  body: 'For the backups.',
  wantsText: wantsText,
  state: state,
  resolved: resolved,
  answer: answer,
  revision: thread == null ? 1 : 2,
  thread: thread ?? const [],
  decidedMs: Int64(state == AskPersonState.ASK_PERSON_STATE_PENDING ? 0 : 1000),
  steps: steps,
);

Agent _agent(String id, String name, {List<int> waiting = const []}) => Agent(
  id: id,
  name: name,
  projectId: 'p1',
  modelAvailable: true,
  status: AgentStatus.AGENT_STATUS_IDLE,
  waitingSeqs: [for (final seq in waiting) Int64(seq)],
);

Workspace _workspace({String selected = 'june'}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  agents: [
    _agent('june', 'June', waiting: [7]),
    _agent('omar', 'Omar', waiting: [9]),
  ],
  selectedAgentId: selected,
);

Harness _inConversation(AskPerson ask) {
  final harness = Harness(_workspace());
  harness.backend.transcript.load(
    Transcript(
      agentId: 'june',
      entries: [
        Entry(
          seq: Int64(7),
          timestampMs: Int64(_ago(const Duration(hours: 2))),
          askPerson: ask,
        ),
      ],
    ),
  );
  return harness;
}

Finder _inCard(Finder finder) =>
    find.descendant(of: find.byType(AskPersonCard), matching: finder);

final _comment = find.descendant(
  of: find.byType(DecisionComment),
  matching: find.byType(EditableText),
);

void main() {
  testWidgets('the talk shows oldest first above the buttons, with Resolve '
      'beneath them; Resolve ends it, for this version, from here', (
    tester,
  ) async {
    final harness = _inConversation(_ask(thread: _thread()));
    await harness.show(tester);
    await settle(tester);
    final mine = tester.getTopLeft(_inCard(find.text(_mine)));
    final theirs = tester.getTopLeft(_inCard(find.text(_theirs)));
    expect(mine.dy, lessThan(theirs.dy));
    expect(
      _inCard(find.textContaining('You · ', findRichText: true)),
      findsOne,
    );
    expect(
      _inCard(find.textContaining('June · ', findRichText: true)),
      findsOne,
    );
    final approve = tester.getTopLeft(_inCard(find.text('Approve')));
    expect(theirs.dy, lessThan(approve.dy));
    final resolve = tester.getTopLeft(_inCard(find.text('Resolve')));
    expect(approve.dy, lessThan(resolve.dy));

    await tester.tap(_inCard(find.text('Resolve')));
    await settle(tester);
    final sent = harness.last.answerAsk;
    expect(sent.resolve, isTrue);
    expect(sent.reply, isFalse);
    expect(sent.dismiss, isFalse);
    expect(sent.revision, 2);
    expect(sent.from, AskAnsweredFrom.ASK_ANSWERED_FROM_CONVERSATION);
  });

  testWidgets('a card nobody replied on looks as it did: no thread, no '
      'Resolve; its answer still says where it came from', (tester) async {
    final harness = _inConversation(_ask());
    await harness.show(tester);
    await settle(tester);
    expect(find.byType(AskThread), findsNothing);
    expect(_inCard(find.text('Resolve')), findsNothing);
    await tester.tap(_inCard(find.text('Approve')));
    await settle(tester);
    expect(
      harness.last.answerAsk.from,
      AskAnsweredFrom.ASK_ANSWERED_FROM_CONVERSATION,
    );
  });

  testWidgets('a typed comment on an approval is the reply, sent with '
      'Submit', (tester) async {
    final harness = _inConversation(_ask(thread: _thread()));
    await harness.show(tester);
    await settle(tester);
    await tester.tap(_inCard(find.text('Add a comment')));
    await settle(tester);
    await tester.enterText(_comment, 'Fine, but encrypt them');
    await settle(tester);
    await tester.tap(_inCard(find.text('Submit')));
    await settle(tester);
    expect(harness.last.answerAsk.reply, isTrue);
    expect(harness.last.answerAsk.text, 'Fine, but encrypt them');
  });

  testWidgets('a written answer has Reply beside Resolve: Reply keeps '
      'talking, Answer answers, and the field starts empty once sent', (
    tester,
  ) async {
    final harness = _inConversation(_ask(wantsText: true, thread: _thread()));
    await harness.show(tester);
    await settle(tester);
    final field = _inCard(find.byType(EditableText)).first;
    expect(
      tester
          .widget<Button>(_inCard(find.widgetWithText(Button, 'Reply')))
          .onPressed,
      isNull,
    );
    await tester.enterText(field, 'And the cost?');
    await settle(tester);
    await tester.tap(_inCard(find.text('Reply')));
    await settle(tester);
    expect(harness.last.answerAsk.reply, isTrue);
    expect(harness.last.answerAsk.text, 'And the cost?');
    expect(
      tester.widget<EditableText>(field).controller.text,
      isEmpty,
      reason: 'what was sent is in the thread once it reopens',
    );
    await tester.enterText(field, 'Use it');
    await settle(tester);
    await tester.tap(_inCard(find.text('Answer')));
    await settle(tester);
    expect(harness.last.answerAsk.reply, isFalse);
    expect(harness.last.answerAsk.dismiss, isFalse);
    expect(harness.last.answerAsk.text, 'Use it');
  });

  testWidgets('a procedure with a thread has Reply and Resolve under Done', (
    tester,
  ) async {
    final harness = _inConversation(
      _ask(
        thread: _thread(),
        steps: [
          AskStep(text: 'Open the console'),
          AskStep(text: 'Restart'),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    expect(_inCard(find.text('Done')), findsOneWidget);
    expect(_inCard(find.text('Resolve')), findsOneWidget);
    await tester.tap(_inCard(find.text('Add a comment')));
    await settle(tester);
    await tester.enterText(_comment, 'Which console?');
    await settle(tester);
    await tester.tap(_inCard(find.text('Reply')));
    await settle(tester);
    expect(harness.last.answerAsk.reply, isTrue);
    expect(harness.last.answerAsk.hasStep(), isFalse);
  });

  testWidgets('a procedure still at its next step, its agent having '
      'answered the comment on the step before, shows the talk with Done, '
      'Reply and Resolve (Bruno 30251)', (tester) async {
    final harness = _inConversation(
      _ask(
        thread: [
          AskThreadMessage(
            text: 'Which console, the old one?',
            atMs: Int64(_ago(const Duration(minutes: 30))),
          ),
          AskThreadMessage(
            fromAgent: true,
            text: 'The new one, under Settings.',
            atMs: Int64(_ago(const Duration(minutes: 20))),
          ),
        ],
        steps: [
          AskStep(
            text: 'Open the console',
            state: AskStepState.ASK_STEP_STATE_DONE,
            comment: 'Which console, the old one?',
            decidedMs: Int64(_ago(const Duration(minutes: 30))),
          ),
          AskStep(text: 'Restart'),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    expect(_inCard(find.text('The new one, under Settings.')), findsOneWidget);
    expect(_inCard(find.text('Resolve')), findsOneWidget);
    expect(_inCard(find.text('Reply')), findsOneWidget);
    await tester.tap(_inCard(find.text('Done')));
    await settle(tester);
    expect(harness.last.answerAsk.step, 1);
    expect(harness.last.answerAsk.reply, isFalse);
    expect(harness.last.answerAsk.resolve, isFalse);
  });

  testWidgets('a procedure replied on says Replied, with the words, never '
      'that its steps are done', (tester) async {
    final ask = _ask(
      thread: _thread(),
      state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
      answer: 'Which console?',
      steps: [
        AskStep(text: 'Open the console'),
        AskStep(text: 'Restart'),
      ],
    )..replied = true;
    final harness = _inConversation(ask);
    await harness.show(tester);
    await settle(tester);
    expect(
      _inCard(find.textContaining('Replied', findRichText: true)),
      findsOne,
    );
    expect(_inCard(find.text('Which console?')), findsOneWidget);
    expect(
      _inCard(find.textContaining('steps done', findRichText: true)),
      findsNothing,
    );
  });

  testWidgets('resolved, it says Resolved, never Approved, under the talk', (
    tester,
  ) async {
    final harness = _inConversation(
      _ask(
        thread: _thread(),
        state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
        resolved: true,
        answer: 'We talked it through',
      ),
    );
    await harness.show(tester);
    await settle(tester);
    expect(_inCard(find.text(_theirs)), findsOneWidget);
    expect(
      _inCard(find.textContaining('Resolved', findRichText: true)),
      findsOne,
    );
    expect(
      _inCard(find.textContaining('Approved', findRichText: true)),
      findsNothing,
    );
    expect(_inCard(find.text('We talked it through')), findsOneWidget);
    expect(_inCard(find.text('Resolve')), findsNothing);
  });

  testWidgets("the agent's words show what reorders text by its code", (
    tester,
  ) async {
    final harness = _inConversation(
      _ask(
        thread: [
          AskThreadMessage(fromAgent: true, text: 'Open invoice\u202Efdp.exe'),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    expect(
      _inCard(find.textContaining('\\u{202E}', findRichText: true)),
      findsWidgets,
    );
  });

  group('Needs you', () {
    Future<Harness> open(WidgetTester tester, {Size? size}) async {
      final harness = Harness(_workspace(selected: ''));
      // June's card came first, but June replied on it since; Omar's came
      // in between.
      harness.backend.entriesById['june/7'] = Entry(
        seq: Int64(7),
        timestampMs: Int64(_ago(const Duration(hours: 3))),
        askPerson: _ask(thread: _thread()),
      );
      harness.backend.entriesById['omar/9'] = Entry(
        seq: Int64(9),
        timestampMs: Int64(_ago(const Duration(hours: 1))),
        askPerson: AskPerson(
          id: 'ask9',
          title: 'Deploy to staging?',
          state: AskPersonState.ASK_PERSON_STATE_PENDING,
        ),
      );
      if (size != null) {
        await harness.show(tester, size: size);
      } else {
        await harness.show(tester);
      }
      await settle(tester);
      harness.page.value = AppPage.inbox;
      await pumpFrames(tester);
      await settle(tester);
      await settle(tester);
      return harness;
    }

    testWidgets('a card its agent replied on rises by its last message', (
      tester,
    ) async {
      await open(tester);
      final june = tester.getTopLeft(find.text('Use the EU bucket?').first);
      final omar = tester.getTopLeft(find.text('Deploy to staging?').first);
      expect(june.dy, lessThan(omar.dy));
    });

    testWidgets('answered there, it says so', (tester) async {
      final harness = await open(tester);
      await tester.tap(find.text('Use the EU bucket?').first);
      await pumpFrames(tester);
      await tester.tap(_inCard(find.text('Resolve')));
      await settle(tester);
      expect(harness.last.answerAsk.resolve, isTrue);
      expect(
        harness.last.answerAsk.from,
        AskAnsweredFrom.ASK_ANSWERED_FROM_NEEDS_YOU,
      );
    });

    testWidgets('resolved there, its row says Resolved, not Answered', (
      tester,
    ) async {
      final harness = await open(tester);
      await tester.tap(find.text('Use the EU bucket?').first);
      await pumpFrames(tester);
      await tester.tap(_inCard(find.text('Resolve')));
      await settle(tester);
      expect(harness.last.answerAsk.resolve, isTrue);
      // The Claktory answers: the card is resolved.
      harness.backend.entriesById['june/7'] = Entry(
        seq: Int64(7),
        timestampMs: Int64(_ago(const Duration(hours: 3))),
        askPerson: _ask(
          thread: _thread(),
          state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
          resolved: true,
        ),
      );
      await harness.apply(
        tester,
        _workspace(selected: '')..agents.first.waitingSeqs.clear(),
      );
      await settle(tester);
      expect(find.text('Resolved'), findsWidgets);
      expect(find.text('Answered'), findsNothing);
    });

    testWidgets('on a phone, the talk and Resolve fit', (tester) async {
      final harness = await open(tester, size: const Size(390, 844));
      await tester.tap(find.text('Use the EU bucket?').first);
      await pumpFrames(tester);
      expect(_inCard(find.text(_theirs)), findsOneWidget);
      await tester.ensureVisible(_inCard(find.text('Resolve')));
      await pumpFrames(tester);
      await tester.tap(_inCard(find.text('Resolve')));
      await settle(tester);
      expect(harness.last.answerAsk.resolve, isTrue);
      expect(tester.takeException(), isNull);
    });

    testWidgets('History says Resolved', (tester) async {
      final harness = Harness(_workspace(selected: ''))
        ..backend.needsYouHistory = [
          NeedsYouRecord(
            kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_QUESTION,
            agentId: 'june',
            agentName: 'June',
            seq: Int64(7),
            title: 'Use the EU bucket?',
            outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_RESOLVED,
            reply: 'We talked it through',
            decidedMs: Int64(_ago(const Duration(hours: 1))),
          ),
        ];
      await harness.show(tester, size: const Size(1200, 900));
      await settle(tester);
      harness.page.value = AppPage.inbox;
      await pumpFrames(tester);
      await tester.tap(find.text('History'));
      await settle(tester);
      await settle(tester);
      expect(
        find.textContaining('Resolved', findRichText: true),
        findsOneWidget,
      );
    });
  });
}
