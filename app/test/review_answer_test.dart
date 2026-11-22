// Answering a review where the file is (Matt's ask, after #275): while an
// agent's review ask waits on the file a pane shows, the pane's foot has
// the question and the answer's field. Return sends what's typed (an empty
// field never approves by itself), Approve or Send and Dismiss beside it;
// answered, the pane closes and the person is back where they pressed
// Review. On media nothing else is offered meanwhile; on text, line
// comments go with the answer.
import 'package:factory_app/dictation.dart' show transcribeRecording;
import 'package:factory_app/layout.dart' show leaves;
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

Future<void> _type(WidgetTester tester, String text) async {
  await tester.tap(_field);
  await tester.enterText(_field, text);
  await tester.pump();
}

void main() {
  testWidgets('the expanded file shows the question and its answer field; '
      'Return sends the answer and the pane closes', (tester) async {
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
    expect(find.text('Send'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);

    final answer = harness.backend.sent
        .lastWhere((m) => m.hasAnswerAsk())
        .answerAsk;
    expect(
      (answer.agentId, answer.seq.toInt(), answer.text, answer.dismiss),
      ('fugo', 7, 'I like the second one better', false),
    );
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
      'place (Dismiss and Send aside), Return is Done, the words go in at '
      'the caret, and nothing goes until Send', (tester) async {
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
    expect(find.text('Send'), findsNothing);
    expect(find.text('Dismiss'), findsNothing);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await io(tester);
    await pumpFrames(tester);
    expect(find.text('I like the second one'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
    await tester.tap(find.text('Send'));
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

  testWidgets('Dismiss takes what was typed as its comment', (tester) async {
    final harness = await _showing(tester, _text, 'text');
    await _type(tester, 'Not now');
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    final answer = harness.backend.sent
        .lastWhere((m) => m.hasAnswerAsk())
        .answerAsk;
    expect((answer.text, answer.dismiss), ('Not now', true));
    expect(harness.last.hasSetLayout(), isTrue);
  });

  testWidgets('a failed answer keeps the text and the file', (tester) async {
    final harness = await _showing(tester, _text, 'text');
    await _type(tester, 'The second');
    harness.backend.refuse = 'Claktory is restarting.';
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(find.text("Couldn't send your answer. Try again."), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
    expect(tester.widget<EditableText>(_field).controller.text, 'The second');
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
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
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
}
