// A review an agent asks of a file on an iPhone (#275): PhoneRoutes'
// showFile opens the file as a page over the agent's conversation (chosen
// first, the file put in its layout as its `show` would, unless it's
// there), and the viewer's foot says who asks and what, with the answer's
// field always open: Approve or Dismiss with nothing typed; a typed
// comment is one Submit (Return too), sent as a reply that carries out
// nothing (#370); and the viewer closes, back where the person was.
import 'dart:convert';

import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/mobile/needs_you_button.dart';
import 'package:factory_app/mobile/presentation.dart' show PhoneSheet;
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:factory_app/dictation.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/voice.dart' show voiceMessagesOn;

import 'harness.dart';
import 'voice_test.dart' show FakeRecorder, io;
import 'panes_test.dart' show beside, snapshot;
import 'tool_views_test.dart' show call;
import 'viewers_test.dart' show png;

const _plan = '/w/plan.md';

FileContent _file(String path) {
  const body = '# The plan\n\nShip it on Friday.\n';
  return FileContent(
    path: path,
    mime: 'text/markdown',
    size: Int64(utf8.encode(body).length),
    data: utf8.encode(body),
    sha256: 'v1',
  );
}

Entry _ask({String path = _plan, bool wantsText = false}) => Entry(
  seq: Int64(7),
  askPerson: AskPerson(
    id: 'q1',
    title: 'Review the release plan',
    wantsText: wantsText,
    state: AskPersonState.ASK_PERSON_STATE_PENDING,
    reviewPath: path,
  ),
);

Workspace _waiting(Workspace workspace, {String selected = 'a1'}) =>
    workspace.deepCopy()
      ..selectedAgentId = selected
      ..agents
          .firstWhere((agent) => agent.id == 'a1')
          .waitingSeqs
          .add(Int64(7));

/// Scratch shots (a zz test sets it).
Future<void> Function(WidgetTester, String)? shots;

PhoneRoutes _routes(WidgetTester tester) =>
    tester.widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope)).routes;

void main() {
  Future<Harness> phone(
    WidgetTester tester, {
    ViewNode? layout,
    String selected = 'a1',
    Size size = const Size(393, 852),
  }) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(
      _waiting(snapshot(layout: layout), selected: selected),
    );
    harness.backend
      ..appliesLayouts = true
      ..files[_plan] = _file(_plan)
      ..entriesById['a1/7'] = _ask();
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    await harness.show(tester, size: size);
    await pumpFrames(tester);
    return harness;
  }

  testWidgets('showFile opens the file in the layout and pushes it; '
      'the review shows at its foot', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = await phone(tester);
    expect(_routes(tester).showFile('a1', _plan), isTrue);
    await pumpFrames(tester);
    final layouts = harness.backend.sent.where((m) => m.hasSetLayout());
    expect(layouts, hasLength(1));
    expect(layouts.single.setLayout.root.split.children.last.leaf.path, _plan);
    expect(harness.backend.sent.where((m) => m.hasSelectAgent()), isEmpty);
    expect(find.text('plan.md'), findsWidgets);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
    expect(find.text('Review the release plan'), findsOneWidget);
    expect(find.text('Your answer to Builder'), findsOneWidget);
    await shots?.call(tester, 'review-default');
    expect(
      find.text('Your comments on lines go to Builder with your answer.'),
      findsOneWidget,
    );
    final approve = find.bySemanticsLabel('Approve').last;
    final dismiss = find.bySemanticsLabel('Dismiss').last;
    for (final button in [approve, dismiss]) {
      expect(tester.getSize(button).height, greaterThanOrEqualTo(44));
    }
    // The answer's button last, under the thumb, beside the field; both
    // above the home indicator.
    final field = tester.getRect(find.byType(EditableText).last);
    expect(tester.getCenter(approve).dx, greaterThan(field.right));
    expect(tester.getRect(approve).bottom, lessThanOrEqualTo(852 - 34));
    semantics.dispose();
  });

  testWidgets('a pane already open is pushed as it is', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    expect(_routes(tester).showFile('a1', _plan), isTrue);
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
  });

  testWidgets("a show row's file name opens the file as a page, as Files "
      'in ⋯ does (#379, Zucchero)', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          call(
            'show',
            {'path': _plan},
            seq: 3,
            output: 'Showing plan.md to Matt.',
            details: {
              'path': _plan,
              'kind': 'text',
              'pane': 'v1',
              'agent': 'a1',
            },
          ),
        ],
      ),
    );
    await pumpFrames(tester);
    expect(find.text('Showing'), findsOneWidget);
    expect(find.text('Ship it on Friday.'), findsNothing);
    await tester.tap(find.text('plan.md').last);
    await pumpFrames(tester);
    expect(find.text('Ship it on Friday.'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
  });

  testWidgets('another agent showing: it is chosen, and the file pushed '
      'once it shows', (tester) async {
    final harness = await phone(tester, selected: 'rivet');
    expect(_routes(tester).showFile('a1', _plan), isTrue);
    await pumpFrames(tester);
    expect(
      harness.backend.sent.where((m) => m.hasSelectAgent()).single,
      ClientMessage(selectAgent: SelectAgent(agentId: 'a1')),
    );
    expect(find.text('Builder asks you to review this'), findsNothing);
    // The backend chooses it.
    await harness.apply(
      tester,
      harness.backend.workspace.workspace.deepCopy()..selectedAgentId = 'a1',
    );
    await pumpFrames(tester);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
  });

  testWidgets('from Needs you, Back returns to it', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
    expect(harness.page.value, AppPage.inbox);
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(find.text('Builder asks you to review this'), findsNothing);
    expect(find.text('Needs you'), findsWidgets);
    expect(harness.page.value, AppPage.inbox);
  });

  testWidgets("Needs you's Review opens the file over it, with the bar; "
      'Back returns to Needs you', (tester) async {
    final harness = await phone(tester);
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.tap(find.text('Builder').first);
    await pumpFrames(tester);
    await tester.tap(find.text('Review').last);
    await pumpFrames(tester);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
    expect(harness.page.value, AppPage.inbox);
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(find.text('Builder asks you to review this'), findsNothing);
    expect(find.text('Review'), findsWidgets);
  });

  testWidgets("from the header's Needs you, Review shows the file on top "
      'at once, and Back returns to Needs you (Matt\'s bug)', (tester) async {
    final harness = await phone(tester, size: const Size(320, 568));
    await tester.tap(find.byType(PhoneNeedsYouButton));
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.inbox);
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    await tester.tap(find.text('Review').last);
    await pumpFrames(tester);
    // On top: its bar takes a tap, with nothing over it.
    final bar = find.text('Builder asks you to review this');
    expect(bar, findsOneWidget);
    expect(
      tester
          .hitTestOnBinding(tester.getCenter(bar))
          .path
          .any((entry) => entry.target is RenderParagraph),
      isTrue,
    );
    expect(find.byType(PhoneSheet), findsNothing);
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(bar, findsNothing);
    expect(harness.page.value, AppPage.inbox);
    expect(find.text('Review'), findsWidgets);
  });

  /// The layout's last SetLayout, and whether it still holds the file.
  bool planShown(Harness harness) {
    bool holds(ViewNode node) =>
        node.hasLeaf() && node.leaf.path == _plan ||
        node.hasSplit() && node.split.children.any(holds);
    final layouts = harness.backend.sent.where((m) => m.hasSetLayout());
    return layouts.isEmpty ||
        layouts.last.setLayout.hasRoot() && holds(layouts.last.setLayout.root);
  }

  testWidgets('typed, Return sends the words as a reply, never a yes, and '
      'the viewer closes back to the conversation', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, 'Go ahead.');
    await pumpFrames(tester);
    expect(find.text('Submit'), findsOneWidget);
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Dismiss'), findsNothing);
    await tester.testTextInput.receiveAction(TextInputAction.send);
    await pumpFrames(tester);
    final answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
    expect(answer.answerAsk.reply, isTrue);
    // An older Claktory reads no yes in it.
    expect(answer.answerAsk.dismiss, isTrue);
    expect(answer.answerAsk.text, 'Go ahead.');
    expect(answer.answerAsk.seq, Int64(7));
    expect(planShown(harness), isFalse, reason: 'its pane closed');
    expect(find.text('Builder asks you to review this'), findsNothing);
    expect(_routes(tester).canPop, isFalse);
  });

  testWidgets('nothing typed: Return does nothing, Approve approves', (
    tester,
  ) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    await tester.tap(find.byType(EditableText).last);
    await pumpFrames(tester);
    await tester.testTextInput.receiveAction(TextInputAction.send);
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
    await tester.tap(find.text('Approve'));
    await pumpFrames(tester);
    final answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
    expect(answer.answerAsk.dismiss, isFalse);
    expect(answer.answerAsk.reply, isFalse);
    expect(answer.answerAsk.text, isEmpty);
    expect(find.text('Builder asks you to review this'), findsNothing);
  });

  testWidgets('Submit sends the words as a reply; cleared, Approve and '
      'Dismiss come back, and Dismiss dismisses', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, 'Not this week.');
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, '');
    await pumpFrames(tester);
    expect(find.text('Submit'), findsNothing);
    expect(find.text('Approve'), findsOneWidget);
    await tester.tap(find.text('Dismiss'));
    await pumpFrames(tester);
    var answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
    expect(answer.answerAsk.dismiss, isTrue);
    expect(answer.answerAsk.reply, isFalse);
    expect(planShown(harness), isFalse);

    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, 'Not this week.');
    await pumpFrames(tester);
    await tester.tap(find.text('Submit'));
    await pumpFrames(tester);
    answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
    expect(answer.answerAsk.reply, isTrue);
    expect(answer.answerAsk.text, 'Not this week.');
  });

  testWidgets('a question that wants words: Send, faint until some are '
      'typed', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    harness.backend.entriesById['a1/7'] = _ask(wantsText: true);
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    expect(find.text('Approve'), findsNothing);
    await tester.tap(find.text('Send'));
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
    await tester.enterText(find.byType(EditableText).last, 'The second.');
    await pumpFrames(tester);
    await tester.tap(find.text('Send'));
    await pumpFrames(tester);
    final answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
    expect(answer.answerAsk.text, 'The second.');
    // The words it asked for are an answer, not a reply.
    expect(answer.answerAsk.reply, isFalse);
    expect(answer.answerAsk.dismiss, isFalse);
  });

  testWidgets('a failed answer keeps the words and the viewer, and says '
      'so', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, 'Go ahead.');
    await pumpFrames(tester);
    harness.backend.refuse = 'Offline.';
    await tester.tap(find.text('Submit'));
    await pumpFrames(tester);
    expect(find.text("Couldn't send your answer. Try again."), findsOneWidget);
    expect(find.text('Go ahead.'), findsOneWidget);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
    harness.backend.refuse = null;
  });

  testWidgets(
    'an answer to a card changed meanwhile says so in the bar (#428), '
    'which stays',
    (tester) async {
      final harness = await phone(tester, layout: beside('text', _plan));
      _routes(tester).showFile('a1', _plan);
      await pumpFrames(tester);
      await tester.enterText(find.byType(EditableText).last, 'Go ahead.');
      await pumpFrames(tester);
      harness.backend.refuse = 'This card changed. Look at it again.';
      await tester.tap(find.text('Submit'));
      await pumpFrames(tester);
      expect(find.text('This card changed. Look at it again.'), findsOneWidget);
      expect(find.text('Go ahead.'), findsOneWidget);
      expect(find.text('Builder asks you to review this'), findsOneWidget);
      harness.backend.refuse = null;
    },
  );

  testWidgets('from Needs you, an answer returns to Needs you', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    await tester.tap(find.text('Approve'));
    await pumpFrames(tester);
    expect(find.text('Builder asks you to review this'), findsNothing);
    expect(harness.page.value, AppPage.inbox);
    expect(find.text('Needs you'), findsWidgets);
  });

  testWidgets('the field and the bar sit above the keyboard', (tester) async {
    await phone(tester, layout: beside('text', _plan));
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    // As iOS has it: the home indicator's inset goes while the keyboard
    // is up.
    tester.view.viewInsets = FakeViewPadding(
      bottom: 336 * tester.view.devicePixelRatio,
    );
    tester.view.padding = const FakeViewPadding(top: 59);
    addTearDown(tester.view.resetViewInsets);
    await pumpFrames(tester);
    await tester.enterText(
      find.byType(EditableText).last,
      'I like the second one better.',
    );
    await pumpFrames(tester);
    await shots?.call(tester, 'review-keyboard');
    final bar = tester.getRect(find.byKey(const ValueKey('phone-review-bar')));
    expect(bar.bottom, closeTo(852 - 336, 1));
    expect(tester.takeException(), isNull);
  });

  testWidgets('on a picture, the answer is the only input: no Comment '
      'while the question waits, and no note about lines', (tester) async {
    const chart = '/w/chart.png';
    final harness = await phone(tester, layout: beside('image', chart));
    final data = await png(tester, 400, 200);
    harness.backend
      ..files[chart] = FileContent(
        path: chart,
        mime: 'image/png',
        size: Int64(data.length),
        data: data,
        sha256: 'v1',
      )
      ..entriesById['a1/7'] = _ask(path: chart);
    _routes(tester).showFile('a1', chart);
    await pumpFrames(tester);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
    await shots?.call(tester, 'review-picture');
    expect(find.text('Comment'), findsNothing);
    expect(
      find.text('Your comments on lines go to Builder with your answer.'),
      findsNothing,
    );
    await tester.tap(find.text('Approve'));
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), hasLength(1));
  });

  testWidgets('no review waits on another file: no bar', (tester) async {
    final harness = await phone(tester, layout: beside('text', _plan));
    harness.backend.entriesById['a1/7'] = _ask(path: '/w/other.md');
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    expect(find.text('plan.md'), findsWidgets);
    expect(find.text('Builder asks you to review this'), findsNothing);
  });

  testWidgets('at the largest text on a small phone, stacked, '
      'nothing overflows', (tester) async {
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await phone(
      tester,
      layout: beside('text', _plan),
      size: const Size(375, 667),
    );
    _routes(tester).showFile('a1', _plan);
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    final field = tester.getRect(find.byType(EditableText).last);
    final approve = tester.getRect(find.text('Approve'));
    final dismiss = tester.getRect(find.text('Dismiss'));
    await shots?.call(tester, 'review-largest');
    expect(field.bottom, lessThanOrEqualTo(approve.top));
    // Approve last, under the thumb.
    expect(approve.left, greaterThan(dismiss.right));
  });

  group('answering by voice', () {
    late FakeRecorder recorder;
    late VoiceRecorder was;
    late Transcribe wasTranscribe;
    final heard = <Recording>[];
    setUp(() {
      voiceMessagesOn = true;
      was = deviceRecorder;
      recorder = FakeRecorder();
      deviceRecorder = recorder;
      wasTranscribe = transcribeRecording;
      heard.clear();
    });
    tearDown(() {
      deviceRecorder = was;
      transcribeRecording = wasTranscribe;
    });

    Future<Harness> voicePhone(WidgetTester tester) async {
      final harness = await phone(tester, layout: beside('text', _plan));
      final spoken = harness.backend.workspace.workspace.deepCopy()
        ..speechModel = SpeechModel(
          state: SpeechModelState.SPEECH_MODEL_STATE_READY,
        );
      await harness.apply(tester, spoken);
      _routes(tester).showFile('a1', _plan);
      await pumpFrames(tester);
      return harness;
    }

    testWidgets('the mic records; Done puts the words in at the caret, '
        'nothing sent until Submit', (tester) async {
      transcribeRecording = (backend, recording) async {
        heard.add(recording);
        return 'the second one';
      };
      final harness = await voicePhone(tester);
      await tester.enterText(find.byType(EditableText).last, 'I like');
      await pumpFrames(tester);
      final dictate = find.bySemanticsLabel('Dictate').last;
      expect(tester.getSize(dictate).height, greaterThanOrEqualTo(44));
      await tester.tap(dictate);
      await io(tester);
      expect(recorder.recordingInto, isNotNull);
      await shots?.call(tester, 'dictation-recording');
      expect(find.text('Done'), findsOneWidget);
      expect(find.text('Cancel'), findsOneWidget);
      await tester.tap(find.text('Done'));
      await io(tester);
      await pumpFrames(tester);
      expect(heard, hasLength(1));
      expect(find.text('I like the second one'), findsOneWidget);
      await shots?.call(tester, 'dictation-done');
      expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
      await tester.tap(find.text('Submit'));
      await pumpFrames(tester);
      final answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
      expect(answer.answerAsk.text, 'I like the second one');
    });

    testWidgets('Cancel throws the recording away; a failure says why and '
        'keeps what was typed', (tester) async {
      transcribeRecording = (backend, recording) async =>
          throw DictationFailure(dictationError('too_long'));
      await voicePhone(tester);
      await tester.enterText(find.byType(EditableText).last, 'Typed');
      await tester.tap(find.bySemanticsLabel('Dictate').last);
      await io(tester);
      await tester.tap(find.text('Cancel'));
      await io(tester);
      expect(recorder.cancelled, 1);
      expect(find.text('Typed'), findsOneWidget);
      await tester.tap(find.bySemanticsLabel('Dictate').last);
      await io(tester);
      await tester.tap(find.text('Done'));
      await io(tester);
      await pumpFrames(tester);
      expect(
        find.text('That was too long. Try a shorter answer.'),
        findsOneWidget,
      );
      expect(find.text('Typed'), findsOneWidget);
    });

    testWidgets('a Claktory that knows no voice: no mic', (tester) async {
      await phone(tester, layout: beside('text', _plan));
      _routes(tester).showFile('a1', _plan);
      await pumpFrames(tester);
      expect(find.text('Builder asks you to review this'), findsOneWidget);
      expect(find.bySemanticsLabel('Dictate'), findsNothing);
    });
  });
}
