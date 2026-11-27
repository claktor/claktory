// Answering a review where the file is (Matt's ask, after #275): while an
// agent's review ask waits on the file a pane shows, the pane's foot has
// the question and the answer's field. Return sends what's typed (an empty
// field never approves by itself), Approve or Send and Dismiss beside it;
// answered, the pane closes and the person is back where they pressed
// Review. On media nothing else is offered meanwhile; on text, line
// comments go with the answer.
import 'dart:io';

import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/card_files.dart' show cardFilesKey;
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/dictation.dart' show transcribeRecording;
import 'package:factory_app/layout.dart' show leaves;
import 'package:factory_app/outbox.dart' show decisionWaiting;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart' show deviceRecorder;
import 'package:factory_app/review_answer.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'voice_test.dart' show FakeRecorder, io;

const _text = '/Users/me/factory/docs/plan.md';
const _video = '/Users/me/demo.mp4';
const _image = '/Users/me/options.png';

ViewNode _open(String path, String kind) => ViewNode(
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
        leaf: ViewLeaf(kind: kind, path: path),
      ),
    ],
  ),
);

Agent _fugo({ViewNode? layout, bool expanded = true, bool waiting = true}) =>
    Agent(
      id: 'fugo',
      name: 'Fugo',
      projectId: 'p1',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      waitingSeqs: [if (waiting) Int64(7)],
      layout: layout,
      layoutFocus: layout != null && expanded ? 'v1' : '',
    );

Workspace _workspace(Agent fugo, {String selected = 'fugo'}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  agents: [
    fugo,
    Agent(id: 'bruno', name: 'Bruno', projectId: 'p1', modelAvailable: true),
  ],
  selectedAgentId: selected,
);

Entry _ask(String path, {bool wantsText = false}) => Entry(
  seq: Int64(7),
  timestampMs: Int64(DateTime.now().millisecondsSinceEpoch),
  askPerson: AskPerson(
    id: 'ask7',
    title: 'Which option is better?',
    body: 'The first or the second.',
    wantsText: wantsText,
    state: AskPersonState.ASK_PERSON_STATE_PENDING,
    reviewPath: path,
    revision: 1,
  ),
);

Future<Harness> _showing(
  WidgetTester tester,
  String path,
  String kind, {
  bool wantsText = false,
  bool expanded = true,
  bool voice = false,
}) async {
  final workspace = _workspace(
    _fugo(layout: _open(path, kind), expanded: expanded),
  );
  if (voice) {
    workspace.speechModel = SpeechModel(
      state: SpeechModelState.SPEECH_MODEL_STATE_READY,
    );
  }
  final harness = Harness(workspace);
  harness.backend.entriesById['fugo/7'] = _ask(path, wantsText: wantsText);
  harness.backend.files[path] = FileContent(
    path: path,
    data: kind == 'text' ? 'The plan.\n'.codeUnits : const [],
    size: Int64(10),
    sha256: 'abc',
  );
  await harness.show(tester, size: const Size(1400, 900));
  await settle(tester);
  await pumpFrames(tester);
  return harness;
}

Finder get _field => find.descendant(
  of: find.byType(ReviewBar),
  matching: find.byType(EditableText),
);

/// What a held bar says (row 1325).
Finder get _held => find.descendant(
  of: find.byType(ReviewBar),
  matching: find.text('You replied. Waiting for Fugo to answer.'),
);

Button _button(String label) =>
    find
            .descendant(
              of: find.byType(ReviewBar),
              matching: find.widgetWithText(Button, label),
            )
            .evaluate()
            .single
            .widget
        as Button;

Future<void> _type(WidgetTester tester, String text) async {
  await tester.tap(_field);
  await tester.enterText(_field, text);
  await tester.pump();
}

void main() {
  // Which card's Review was pressed is the app's, across tests.
  tearDown(forgetReviewChoices);

  testWidgets('the expanded file shows the question and its answer field; '
      'typed, one Submit, and Return sends the words as a reply, never a yes '
      '(#370); the pane closes', (tester) async {
    final harness = await _showing(tester, _text, 'text');
    expect(find.text('Fugo asks you to review this'), findsOneWidget);
    expect(find.text('Which option is better?'), findsOneWidget);
    expect(find.text('Your answer to Fugo'), findsOneWidget);
    expect(
      find.text('Your comments on lines go to Fugo with your answer.'),
      findsOneWidget,
    );
    expect(find.text('Approve'), findsOneWidget);

    await _type(tester, 'I like the second one better');
    expect(find.text('Submit'), findsOneWidget);
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Dismiss'), findsNothing);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);

    final answer = harness.backend.sent
        .lastWhere((m) => m.hasAnswerAsk())
        .answerAsk;
    expect(
      (answer.agentId, answer.seq.toInt(), answer.text, answer.reply),
      ('fugo', 7, 'I like the second one better', true),
    );
    // A Claktory from before Submit takes it as a comment, never a yes.
    expect(answer.dismiss, isTrue);
    final set = harness.last.setLayout;
    expect(set.agentId, 'fugo');
    expect(set.hasRoot(), isFalse, reason: 'only the conversation is left');
    expect(set.focus, '');
  });

  testWidgets('Escape while typing the answer leaves the field and keeps '
      'the words; the next Escape goes back', (tester) async {
    final harness = await _showing(tester, _text, 'text');
    await _type(tester, 'Half an answer');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    // Still expanded, the words still there, the field no longer typing.
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
    expect(find.text('Fugo asks you to review this'), findsOneWidget);
    expect(
      tester.widget<EditableText>(_field).controller.text,
      'Half an answer',
    );
    expect(tester.widget<EditableText>(_field).focusNode.hasFocus, isFalse);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(harness.last.setLayout.focus, '');
  });

  testWidgets('what reorders the question shows by its code', (tester) async {
    final harness = Harness(_workspace(_fugo(layout: _open(_text, 'text'))));
    harness.backend.entriesById['fugo/7'] = _ask(_text)
      ..askPerson.title = 'Approve evil\u202Etxt.exe'
      ..askPerson.body = 'See \u2066this\u2069.';
    harness.backend.files[_text] = FileContent(
      path: _text,
      data: 'The plan.\n'.codeUnits,
      size: Int64(10),
      sha256: 'abc',
    );
    await harness.show(tester, size: const Size(1400, 900));
    await settle(tester);
    await pumpFrames(tester);
    final bar = find.byType(ReviewBar);
    expect(
      find.descendant(
        of: bar,
        matching: find.text(r'Approve evil\u{202E}txt.exe'),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: bar,
        matching: find.textContaining(
          r'See \u{2066}this\u{2069}.',
          findRichText: true,
        ),
      ),
      findsOneWidget,
    );
  });

  testWidgets('said rather than typed: the mic records in the field\'s '
      'place (its buttons aside), Return is Done, the words go in at the '
      'caret, and nothing goes until Submit', (tester) async {
    final was = deviceRecorder;
    final recorder = FakeRecorder();
    deviceRecorder = recorder;
    addTearDown(() => deviceRecorder = was);
    final wasTranscribe = transcribeRecording;
    transcribeRecording = (backend, recording) async => 'the second one';
    addTearDown(() => transcribeRecording = wasTranscribe);
    final harness = await _showing(tester, _text, 'text', voice: true);
    await _type(tester, 'I like');
    final dictate = find.descendant(
      of: find.byType(ReviewBar),
      matching: find.byKey(const ValueKey('dictate')),
    );
    await tester.tap(dictate);
    await io(tester);
    expect(recorder.recordingInto, isNotNull);
    expect(find.text('Submit'), findsNothing);
    expect(find.text('Dismiss'), findsNothing);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await io(tester);
    await pumpFrames(tester);
    expect(find.text('I like the second one'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
    await tester.tap(find.text('Submit'));
    await settle(tester);
    expect(
      harness.backend.sent.lastWhere((m) => m.hasAnswerAsk()).answerAsk.text,
      'I like the second one',
    );
  });

  testWidgets('Return on an empty field approves nothing; Approve does', (
    tester,
  ) async {
    final harness = await _showing(tester, _text, 'text', expanded: false);
    await tester.tap(_field);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
    await tester.tap(find.text('Approve'));
    await settle(tester);
    final answer = harness.backend.sent
        .lastWhere((m) => m.hasAnswerAsk())
        .answerAsk;
    expect((answer.text, answer.dismiss), ('', false));
    // Opened from the file, not its card: no talk seen (#492).
    expect(answer.hasTalkSeen(), isTrue);
    expect(answer.talkSeen, 0);
  });

  testWidgets('an ask that wants words: Send, faint until there are some', (
    tester,
  ) async {
    final harness = await _showing(tester, _text, 'text', wantsText: true);
    expect(find.text('Approve'), findsNothing);
    await tester.tap(find.text('Send'));
    await settle(tester);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
    await _type(tester, 'The second');
    await tester.tap(find.text('Send'));
    await settle(tester);
    expect(
      harness.backend.sent.lastWhere((m) => m.hasAnswerAsk()).answerAsk.text,
      'The second',
    );
  });

  testWidgets('typed, Dismiss gives way to Submit; cleared, it comes back '
      'and dismisses', (tester) async {
    final harness = await _showing(tester, _text, 'text');
    await _type(tester, 'Not now');
    expect(find.text('Dismiss'), findsNothing);
    await _type(tester, '');
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    final answer = harness.backend.sent
        .lastWhere((m) => m.hasAnswerAsk())
        .answerAsk;
    expect((answer.text, answer.dismiss, answer.reply), ('', true, false));
    expect(harness.last.hasSetLayout(), isTrue);
  });

  testWidgets('cut off by the connection, the answer waits as "Not sent '
      'yet", keeps the text and the file, and goes by itself once the '
      "connection's back (Abbacchio's rows 1298-1301)", (tester) async {
    final harness = await _showing(tester, _text, 'text');
    await _type(tester, 'The second');
    harness.backend.refuse = 'Connection lost.';
    await tester.tap(find.text('Submit'));
    await settle(tester);
    expect(find.text(decisionWaiting), findsOneWidget);
    expect(find.text("Couldn't send your answer. Try again."), findsNothing);
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
    expect(tester.widget<EditableText>(_field).controller.text, 'The second');
    final tries = harness.backend.sent.where((m) => m.hasAnswerAsk()).length;

    // Back: it goes again; the first got there meanwhile, so the
    // Claktory's "already" means it went, and the pane closes.
    harness.backend.refuse = null;
    harness.backend.onSend = (m) {
      if (m.hasAnswerAsk()) {
        throw const RequestFailure('This question was already answered.');
      }
    };
    harness.backend.connected.value = false;
    await tester.pump();
    harness.backend.connected.value = true;
    await settle(tester);
    expect(
      harness.backend.sent.where((m) => m.hasAnswerAsk()).length,
      tries + 1,
    );
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isNotEmpty);
    expect(find.text(decisionWaiting), findsNothing);
  });

  testWidgets('cut off, then refused once back: the refusal shows, not '
      '"Couldn\'t send"', (tester) async {
    final harness = await _showing(tester, _text, 'text');
    harness.backend.refuse = 'Connection lost.';
    await tester.tap(find.text('Approve'));
    await settle(tester);
    expect(find.text(decisionWaiting), findsOneWidget);
    const refusal =
        'Fugo replied on this card since you looked at it. Read the reply, '
        'then answer again.';
    harness.backend.refuse = refusal;
    harness.backend.connected.value = false;
    await tester.pump();
    harness.backend.connected.value = true;
    await settle(tester);
    expect(find.text(refusal), findsOneWidget);
    expect(find.text(decisionWaiting), findsNothing);
    harness.backend.refuse = null;
  });

  testWidgets(
    'an answer to a card changed meanwhile says so (#428), and keeps the '
    'text',
    (tester) async {
      final harness = await _showing(tester, _text, 'text');
      await _type(tester, 'The second');
      harness.backend.refuse = 'This card changed. Look at it again.';
      await tester.tap(find.text('Submit'));
      await settle(tester);
      expect(find.text('This card changed. Look at it again.'), findsOneWidget);
      expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
      expect(tester.widget<EditableText>(_field).controller.text, 'The second');
    },
  );

  testWidgets("refused because its agent replied on the card since, the bar "
      "says so in Claktory's words, never as a failure to send (D2)", (
    tester,
  ) async {
    final harness = await _showing(tester, _text, 'text');
    const refusal =
        'Fugo replied on this card since you looked at it. Read the reply on '
        'the card, then answer again.';
    harness.backend.refuse = refusal;
    await tester.tap(find.text('Approve'));
    await settle(tester);
    expect(find.text(refusal), findsOneWidget);
    expect(find.text("Couldn't send your answer. Try again."), findsNothing);
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
  });

  testWidgets('on media the field is all there is: no Comment meanwhile', (
    tester,
  ) async {
    await _showing(tester, _video, 'video');
    expect(find.byType(ReviewBar), findsOneWidget);
    expect(find.text('Comment'), findsNothing);
    expect(
      find.text('Your comments on lines go to Fugo with your answer.'),
      findsNothing,
    );
    expect(reviewHasLineComments(_image), isFalse);
    expect(reviewHasLineComments(_text), isTrue);
  });

  testWidgets('nothing waits on the file: no bar', (tester) async {
    final harness = Harness(
      _workspace(_fugo(layout: _open(_text, 'text'), waiting: false)),
    );
    harness.backend.files[_text] = FileContent(
      path: _text,
      data: 'The plan.\n'.codeUnits,
      size: Int64(10),
      sha256: 'abc',
    );
    await harness.show(tester, size: const Size(1400, 900));
    await settle(tester);
    expect(find.byType(ReviewBar), findsNothing);
  });

  testWidgets('from Needs you, answered, back to Needs you and the agent '
      'shown before', (tester) async {
    final harness = Harness(_workspace(_fugo(), selected: 'bruno'));
    harness.backend.entriesById['fugo/7'] = _ask(_text);
    harness.backend.files[_text] = FileContent(
      path: _text,
      data: 'The plan.\n'.codeUnits,
      size: Int64(10),
      sha256: 'abc',
    );
    await harness.show(tester, size: const Size(1400, 900));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await settle(tester);
    await tester.tap(find.text('Which option is better?'));
    await pumpFrames(tester);
    await tester.tap(find.text('Review'));
    await settle(tester);
    // The backend shows what Review asked for.
    final opened = harness.last.setLayout;
    await harness.apply(
      tester,
      _workspace(
        _fugo()
          ..layout = opened.root
          ..layoutFocus = opened.focus,
      ),
    );
    await settle(tester);
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.agent);
    expect(
      leaves(opened.root).where((leaf) => leaf.leaf.path == _text),
      hasLength(1),
    );

    await _type(tester, 'Looks right');
    await tester.tap(find.text('Submit'));
    await settle(tester);
    expect(
      harness.backend.sent.lastWhere((m) => m.hasAnswerAsk()).answerAsk.text,
      'Looks right',
    );
    expect(
      harness.backend.sent.lastWhere((m) => m.hasSelectAgent()).selectAgent,
      SelectAgent(agentId: 'bruno'),
    );
    expect(harness.page.value, AppPage.inbox);
  });

  testWidgets("Review from a card with a talk: the answer says how much of "
      'it the card showed (#492)', (tester) async {
    final harness = Harness(_workspace(_fugo(), selected: 'bruno'));
    harness.backend.entriesById['fugo/7'] = _ask(_text)
      ..askPerson.thread.addAll([
        AskThreadMessage(text: 'Why the second?'),
        AskThreadMessage(fromAgent: true, text: 'It is shorter.'),
      ]);
    harness.backend.files[_text] = FileContent(
      path: _text,
      data: 'The plan.\n'.codeUnits,
      size: Int64(10),
      sha256: 'abc',
    );
    await harness.show(tester, size: const Size(1400, 900));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await settle(tester);
    await tester.tap(find.text('Which option is better?'));
    await pumpFrames(tester);
    await tester.ensureVisible(find.text('Review'));
    await tester.tap(find.text('Review'));
    await settle(tester);
    final opened = harness.last.setLayout;
    await harness.apply(
      tester,
      _workspace(
        _fugo()
          ..layout = opened.root
          ..layoutFocus = opened.focus,
      ),
    );
    await settle(tester);
    await pumpFrames(tester);
    await tester.tap(find.text('Approve'));
    await settle(tester);
    final answer = harness.backend.sent
        .lastWhere((m) => m.hasAnswerAsk())
        .answerAsk;
    expect((answer.text, answer.dismiss), ('', false));
    expect(answer.talkSeen, 2);
  });

  group('a card its agent replied on while the file was open (row 485)', () {
    const changed = 'This card changed. Look at it again.';

    /// Fugo's card after his reply on it: a new revision, the reply its
    /// talk.
    Entry replied() => _ask(_text)
      ..askPerson.revision = 2
      ..askPerson.thread.add(
        AskThreadMessage(fromAgent: true, text: 'The second is shorter.'),
      );

    /// The backend: anything but the latest revision is refused.
    void latestOnly(Harness harness) => harness.backend.onSend = (m) {
      if (m.hasAnswerAsk() && m.answerAsk.revision != 2) {
        throw const RequestFailure(changed);
      }
    };

    List<AnswerAsk> answers(Harness harness) => [
      for (final m in harness.backend.sent)
        if (m.hasAnswerAsk()) m.answerAsk,
    ];

    testWidgets('Dismiss goes with its latest revision, and the pane closes', (
      tester,
    ) async {
      final harness = await _showing(tester, _text, 'text');
      harness.backend.entriesById['fugo/7'] = replied();
      latestOnly(harness);
      await tester.tap(find.text('Dismiss'));
      await settle(tester);
      final sent = answers(harness);
      expect([for (final a in sent) a.revision], [1, 2]);
      expect((sent.last.dismiss, sent.last.reply), (true, false));
      expect(find.text(changed), findsNothing);
      expect(harness.last.hasSetLayout(), isTrue);
    });

    testWidgets('a typed reply goes too', (tester) async {
      final harness = await _showing(tester, _text, 'text');
      harness.backend.entriesById['fugo/7'] = replied();
      latestOnly(harness);
      await _type(tester, 'Why shorter?');
      await tester.tap(find.text('Submit'));
      await settle(tester);
      final sent = answers(harness);
      expect([for (final a in sent) a.revision], [1, 2]);
      expect((sent.last.reply, sent.last.text), (true, 'Why shorter?'));
      expect(harness.last.hasSetLayout(), isTrue);
    });

    testWidgets('Approve is refused, and the bar shows the reply; approved '
        'again, it goes with the reply seen', (tester) async {
      final harness = await _showing(tester, _text, 'text');
      expect(find.text('The second is shorter.'), findsNothing);
      harness.backend.entriesById['fugo/7'] = replied();
      latestOnly(harness);
      await tester.tap(find.text('Approve'));
      await settle(tester);
      expect(answers(harness), hasLength(1));
      expect(find.text(changed), findsOneWidget);
      expect(
        find.descendant(
          of: find.byType(ReviewBar),
          matching: find.textContaining(
            'The second is shorter.',
            findRichText: true,
          ),
        ),
        findsOneWidget,
      );
      expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);

      await tester.tap(find.text('Approve'));
      await settle(tester);
      final last = answers(harness).last;
      expect((last.revision, last.talkSeen, last.dismiss), (2, 1, false));
      expect(harness.last.hasSetLayout(), isTrue);
    });

    testWidgets('with its conversation open, the reply shows as it comes, '
        'and Dismiss goes at once', (tester) async {
      final harness = await _showing(tester, _text, 'text');
      harness.backend.transcript.load(
        Transcript(agentId: 'fugo', entries: [_ask(_text)]),
      );
      await tester.pump();
      harness.backend.transcript.update(
        EntryUpdate(agentId: 'fugo', entry: replied()),
      );
      await tester.pump();
      expect(
        find.descendant(
          of: find.byType(ReviewBar),
          matching: find.textContaining(
            'The second is shorter.',
            findRichText: true,
          ),
        ),
        findsOneWidget,
      );
      latestOnly(harness);
      await tester.tap(
        find.descendant(
          of: find.byType(ReviewBar),
          matching: find.text('Dismiss'),
        ),
      );
      await settle(tester);
      expect([for (final a in answers(harness)) a.revision], [2]);
    });
  });

  testWidgets("what's typed stays while a reply from another device takes "
      "the card out of waiting and its agent's answer brings it back "
      "(Zucchero's row 496)", (tester) async {
    final harness = await _showing(tester, _text, 'text');
    harness.backend.transcript.load(
      Transcript(agentId: 'fugo', entries: [_ask(_text)]),
    );
    await tester.pump();
    await _type(tester, 'Fine by me');
    // Replied from the Mac: it waits on Fugo, the bar held (row 1325).
    harness.backend.transcript.update(
      EntryUpdate(
        agentId: 'fugo',
        entry: _ask(_text)
          ..askPerson.state = AskPersonState.ASK_PERSON_STATE_ANSWERED
          ..askPerson.replied = true
          ..askPerson.answer = 'From the Mac',
      ),
    );
    await harness.apply(
      tester,
      _workspace(_fugo(layout: _open(_text, 'text'), waiting: false)),
    );
    await settle(tester);
    expect(find.byType(ReviewBar), findsOneWidget);
    expect(_held, findsOneWidget);
    expect(_button('Submit').onPressed, isNull);
    // Fugo answers on the card: it waits again, the bar with it.
    await harness.apply(
      tester,
      _workspace(_fugo(layout: _open(_text, 'text'))),
    );
    harness.backend.transcript.update(
      EntryUpdate(
        agentId: 'fugo',
        entry: _ask(_text)
          ..askPerson.revision = 2
          ..askPerson.thread.addAll([
            AskThreadMessage(text: 'From the Mac'),
            AskThreadMessage(fromAgent: true, text: 'Noted.'),
          ]),
      ),
    );
    await settle(tester);
    expect(find.byType(ReviewBar), findsOneWidget);
    expect(_held, findsNothing);
    expect(tester.widget<EditableText>(_field).controller.text, 'Fine by me');
    expect(_button('Submit').onPressed, isNotNull);
  });

  testWidgets('the bar says what Approve means after the talk, as the card '
      'does (Bruno 37281)', (tester) async {
    final harness = Harness(_workspace(_fugo(layout: _open(_text, 'text'))));
    harness.backend.entriesById['fugo/7'] = _ask(_text)
      ..askPerson.approveFollowsTalk = true
      ..askPerson.thread.addAll([
        AskThreadMessage(text: 'Why the second?'),
        AskThreadMessage(fromAgent: true, text: 'It is shorter.'),
      ]);
    harness.backend.files[_text] = FileContent(
      path: _text,
      data: 'The plan.\n'.codeUnits,
      size: Int64(10),
      sha256: 'abc',
    );
    await harness.show(tester, size: const Size(1400, 900));
    await settle(tester);
    await pumpFrames(tester);
    expect(
      find.descendant(
        of: find.byType(ReviewBar),
        matching: find.text("Approving agrees with Fugo's last reply."),
      ),
      findsOneWidget,
    );
  });

  testWidgets("two cards waiting on one file: the bar answers the one whose "
      "Review was pressed (Abbacchio's row 1312)", (tester) async {
    final fugo = _fugo(layout: _open(_text, 'text'))..waitingSeqs.add(Int64(8));
    final harness = Harness(_workspace(fugo));
    harness.backend.entriesById['fugo/7'] = _ask(_text);
    harness.backend.entriesById['fugo/8'] = Entry(
      seq: Int64(8),
      timestampMs: Int64(DateTime.now().millisecondsSinceEpoch),
      askPerson: AskPerson(
        id: 'ask8',
        title: 'And the revised one?',
        state: AskPersonState.ASK_PERSON_STATE_PENDING,
        reviewPath: _text,
        revision: 1,
      ),
    );
    harness.backend.files[_text] = FileContent(
      path: _text,
      data: 'The plan.\n'.codeUnits,
      size: Int64(10),
      sha256: 'abc',
    );
    await harness.show(tester, size: const Size(1400, 900));
    await settle(tester);
    await pumpFrames(tester);
    Finder inBar(String text) =>
        find.descendant(of: find.byType(ReviewBar), matching: find.text(text));
    // Opened from the file: the oldest.
    expect(inBar('Which option is better?'), findsOneWidget);
    // Card 8's Review.
    rememberReviewTalk('fugo', 8, 0);
    await settle(tester);
    expect(inBar('And the revised one?'), findsOneWidget);
    await tester.tap(find.text('Approve'));
    await settle(tester);
    final answer = harness.backend.sent
        .lastWhere((m) => m.hasAnswerAsk())
        .answerAsk;
    expect(answer.seq.toInt(), 8);
  });

  group('the card whose Review was pressed, replied on from another device '
      "(Abbacchio's row 1325)", () {
    Entry card8({
      AskPersonState state = AskPersonState.ASK_PERSON_STATE_PENDING,
      bool replied = false,
      int revision = 1,
    }) => Entry(
      seq: Int64(8),
      timestampMs: Int64(DateTime.now().millisecondsSinceEpoch),
      askPerson: AskPerson(
        id: 'ask8',
        title: 'And the revised one?',
        state: state,
        replied: replied,
        answer: replied ? 'From the Mac' : '',
        reviewPath: _text,
        revision: revision,
      ),
    );

    Workspace waiting(List<int> seqs) => _workspace(
      _fugo(layout: _open(_text, 'text'), waiting: false)
        ..waitingSeqs.addAll([for (final seq in seqs) Int64(seq)]),
    );

    Finder inBar(String text) =>
        find.descendant(of: find.byType(ReviewBar), matching: find.text(text));

    Future<Harness> pressed(WidgetTester tester) async {
      final harness = Harness(waiting([7, 8]));
      harness.backend.entriesById['fugo/7'] = _ask(_text);
      harness.backend.entriesById['fugo/8'] = card8();
      harness.backend.files[_text] = FileContent(
        path: _text,
        data: 'The plan.\n'.codeUnits,
        size: Int64(10),
        sha256: 'abc',
      );
      await harness.show(tester, size: const Size(1400, 900));
      await settle(tester);
      await pumpFrames(tester);
      rememberReviewTalk('fugo', 8, 0);
      await settle(tester);
      expect(inBar('And the revised one?'), findsOneWidget);
      await _type(tester, 'Half done');
      // Replied from the Mac: card 8 waits on Fugo, as fetched again.
      harness.backend.entriesById['fugo/8'] = card8(
        state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
        replied: true,
      );
      await harness.apply(tester, waiting([7]));
      await settle(tester);
      return harness;
    }

    testWidgets('the bar keeps it, held, never the older card; back with '
        "what's typed once its agent answers", (tester) async {
      final harness = await pressed(tester);
      expect(inBar('And the revised one?'), findsOneWidget);
      expect(inBar('Which option is better?'), findsNothing);
      expect(_held, findsOneWidget);
      expect(_button('Submit').onPressed, isNull);
      // The reply, as the person's, in the talk until the agent answers
      // (Abbacchio's row 1376).
      expect(
        find.descendant(
          of: find.byType(ReviewTalk),
          matching: find.textContaining('From the Mac', findRichText: true),
        ),
        findsOneWidget,
      );
      // Return sends nothing either.
      final before = harness.backend.sent.where((m) => m.hasAnswerAsk()).length;
      await tester.tap(_field);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await settle(tester);
      expect(
        harness.backend.sent.where((m) => m.hasAnswerAsk()),
        hasLength(before),
      );
      // Fugo answers on it: waiting again, at a new revision.
      harness.backend.entriesById['fugo/8'] = card8(revision: 2);
      await harness.apply(tester, waiting([7, 8]));
      await settle(tester);
      expect(inBar('And the revised one?'), findsOneWidget);
      expect(_held, findsNothing);
      expect(tester.widget<EditableText>(_field).controller.text, 'Half done');
      await tester.tap(find.text('Submit'));
      await settle(tester);
      final answer = harness.backend.sent
          .lastWhere((m) => m.hasAnswerAsk())
          .answerAsk;
      expect(answer.seq.toInt(), 8);
      expect(answer.revision, 2);
    });

    testWidgets('closed instead, the bar moves on to the older card, '
        "saying so, its field empty: the closed card's words stay with it "
        "(Zucchero's row 513)", (tester) async {
      final harness = await pressed(tester);
      // Fugo closes it, settled in the talk (#496).
      harness.backend.entriesById['fugo/8'] = card8(
        state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
        replied: true,
      )..askPerson.resolved = true;
      await harness.apply(tester, waiting([7]));
      await settle(tester);
      expect(inBar('Which option is better?'), findsOneWidget);
      expect(_held, findsNothing);
      expect(
        inBar('That request was closed. This is another one from Fugo.'),
        findsOneWidget,
      );
      expect(tester.widget<EditableText>(_field).controller.text, isEmpty);
      expect(_button('Approve').onPressed, isNotNull);
    });

    testWidgets("the file's pane built again after the card stopped waiting "
        "still holds it (Abbacchio's row 1359)", (tester) async {
      final harness = Harness(waiting([7]));
      harness.backend.entriesById['fugo/7'] = _ask(_text);
      harness.backend.entriesById['fugo/8'] = card8(
        state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
        replied: true,
      );
      harness.backend.files[_text] = FileContent(
        path: _text,
        data: 'The plan.\n'.codeUnits,
        size: Int64(10),
        sha256: 'abc',
      );
      // Card 8's Review pressed; the pane comes once it's been replied on.
      rememberReviewTalk('fugo', 8, 0);
      await harness.show(tester, size: const Size(1400, 900));
      await settle(tester);
      await pumpFrames(tester);
      expect(inBar('And the revised one?'), findsOneWidget);
      expect(inBar('Which option is better?'), findsNothing);
      expect(_held, findsOneWidget);
      expect(_button('Approve').onPressed, isNull);
    });
  });

  testWidgets("files added to the card (#511) go with the review's answer, "
      'under the field meanwhile', (tester) async {
    final folder = Directory.systemTemp.createTempSync('review-files');
    addTearDown(() => folder.deleteSync(recursive: true));
    final shot = '${folder.path}/shot.png';
    File(shot).writeAsBytesSync(List.filled(100, 1));
    final harness = await _showing(tester, _text, 'text');
    await tester.runAsync(
      () =>
          harness.backend.attachments.add(cardFilesKey('fugo', 'ask7'), [shot]),
    );
    await tester.pump();
    expect(
      find.descendant(
        of: find.byType(ReviewBar),
        matching: find.text('shot.png'),
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Approve'));
    for (var i = 0; i < 12; i++) {
      await tester.pump();
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 10)),
      );
    }
    await settle(tester);
    final answer = harness.backend.sent
        .lastWhere((m) => m.hasAnswerAsk())
        .answerAsk;
    expect([for (final a in answer.attachments) a.name], ['shot.png']);
    expect(
      harness.backend.attachments.of(cardFilesKey('fugo', 'ask7')),
      isEmpty,
    );
  });
}
