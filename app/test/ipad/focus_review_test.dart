// A PDF or a picture a card asks to have reviewed, in focus mode on an
// iPad (#514, the spec's §6.4): the stage shows it in the document host,
// to draw on, with the iPad's review bar under it as the card's answer
// (no card, no second bar). ⌘↩ from the system's menu, or Approve, sends
// the drawings with the answer; nothing closes in the agent's layout, and
// the next card comes in once the backend has it. ⌘⌫ asks why first. A
// refusal keeps the card, the words and the ink.
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/ipad/commands.dart' show CommandMenu;
import 'package:factory_app/ipad/document_review.dart';
import 'package:factory_app/ipad/focus.dart';
import 'package:factory_app/ipad/ink_tools.dart' show InkTools;
import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/mobile/needs_you_phone.dart'
    show PhoneNeedsYou, phoneThumbDockKey;
import 'package:factory_app/inbox.dart' show NeedsYouCard;
import 'package:factory_app/needs_you_focus.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:fixnum/fixnum.dart';

import 'dart:convert' show base64Encode;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart' show Harness;
import '../panes_test.dart' show beside, snapshot;
import '../viewers_test.dart' show png;
import 'as_ipad.dart';
import 'document_native.dart';

const _menuChannel = MethodChannel(CommandMenu.channelName);

/// What the system's menu was last given: titles by id.
final _menu = <String, String>{};

/// Builder waits on the review of the paper (7), then on a question (8).
Workspace _waiting(List<int> seqs) {
  final workspace = snapshot();
  workspace.agents.firstWhere((agent) => agent.id == 'a1').waitingSeqs
    ..clear()
    ..addAll([for (final seq in seqs) Int64(seq)]);
  return workspace;
}

const _imagePath = '/w/shot.png';

Future<Harness> _show(WidgetTester tester, {bool image = false}) async {
  asIPad();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  // The system's menu takes the commands, as an iPad's does.
  _menu.clear();
  messenger.setMockMethodCallHandler(_menuChannel, (call) async {
    if (call.method != 'setCommands') return null;
    _menu.clear();
    for (final command in (call.arguments as Map)['commands'] as List) {
      final map = command as Map;
      _menu[map['id'] as String] = map['title'] as String;
    }
    return true;
  });
  addTearDown(() => messenger.setMockMethodCallHandler(_menuChannel, null));
  final harness = Harness(_waiting([7, 8]));
  final picture = image ? await png(tester, 400, 200) : null;
  harness.backend
    ..files[documentPath] = documentPdf()
    ..files[_imagePath] = FileContent(
      path: _imagePath,
      mime: 'image/png',
      size: Int64(picture?.length ?? 0),
      data: picture,
      sha256: 'v1',
    )
    ..entriesById['a1/7'] = Entry(
      seq: Int64(7),
      timestampMs: Int64(1000),
      askPerson: AskPerson(
        id: 'q1',
        title: 'Check the figures on page 3',
        body: 'The totals look off.',
        state: AskPersonState.ASK_PERSON_STATE_PENDING,
        reviewPath: image ? _imagePath : documentPath,
      ),
    )
    ..entriesById['a1/8'] = Entry(
      seq: Int64(8),
      timestampMs: Int64(2000),
      askPerson: AskPerson(id: 'q2', title: 'Ship it on Friday?'),
    );
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: const Size(1376, 1032),
    home: WorkbenchScope(
      focus: focus,
      child: const Shell(rail: SizedBox(width: 56)),
    ),
  );
  await settle(tester);
  setFocusMode(harness.backend, true);
  await settle(tester);
  return harness;
}

FocusSession _session(WidgetTester tester) =>
    tester.widget<IPadFocus>(find.byType(IPadFocus)).session;

/// The system's menu runs [id], as its key command does.
Future<void> _run(WidgetTester tester, String id) async {
  await TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .handlePlatformMessage(
        CommandMenu.channelName,
        const StandardMethodCodec().encodeMethodCall(
          MethodCall('run', {'id': id}),
        ),
        (_) {},
      );
  await settle(tester);
}

/// The backend has the answer: the card stops waiting.
Future<void> _answered(WidgetTester tester, Harness harness) async {
  harness.backend.workspace.apply(_waiting([8]));
  await settle(tester);
}

Finder _onStage(String text) =>
    find.descendant(of: find.byType(FocusStage), matching: find.text(text));

void main() {
  testWidgets('the PDF on the stage, drawn on; ⌘↩ sends the drawing with '
      'the approval, nothing closes, and the next card comes in', (
    tester,
  ) async {
    final native = DocumentNative();
    final harness = await _show(tester);
    final session = _session(tester);
    expect(session.current?.seq, 7);
    // The document with its tools, the review bar as the answer; no card,
    // no second bar.
    expect(find.byType(InkTools), findsOneWidget);
    expect(find.byType(DocumentReviewBar), findsOneWidget);
    expect(find.byType(NeedsYouCard), findsNothing);
    expect(_onStage('Check the figures on page 3'), findsOneWidget);
    expect(_onStage('Approve'), findsOneWidget);
    expect(session.answer.value?.yes, 'Approve');

    await native.drew(2, 3);
    await settle(tester);
    expect(find.text('Your drawing goes with it.'), findsOneWidget);
    await _run(tester, 'focus.yes');
    final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
    expect(answer.seq, Int64(7));
    expect(answer.dismiss, isFalse);
    expect([for (final a in answer.attachments) a.name], ['paper-p2.png']);
    // The agent's layout is left as it is: no pane closes.
    expect(sentOf(harness, (m) => m.hasSetLayout()), isEmpty);
    // Still here until the backend has it, then the next card.
    expect(session.current?.seq, 7);
    await _answered(tester, harness);
    expect(_onStage('Ship it on Friday?'), findsOneWidget);
    expect(find.byType(DocumentReviewBar), findsNothing);
    expect(find.byType(NeedsYouCard), findsOneWidget);
  });

  testWidgets('Approve on the bar goes through the session too', (
    tester,
  ) async {
    final native = DocumentNative();
    final harness = await _show(tester);
    // The agent's layout changes under focus mode (its window, offstage,
    // lays it out again): the stage's document isn't one of its panes, and
    // stays as it is.
    final views = native.views.length;
    final changed = _waiting([7, 8]);
    changed.agents.firstWhere((agent) => agent.id == 'a1').layout = beside();
    harness.backend.workspace.apply(changed);
    await settle(tester);
    expect(native.views.length, views);
    expect(find.byType(InkTools), findsOneWidget);
    await tester.tap(_onStage('Approve'));
    await settle(tester);
    final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
    expect(answer.dismiss, isFalse);
    expect(answer.reply, isFalse);
    expect(sentOf(harness, (m) => m.hasSetLayout()), isEmpty);
    await _answered(tester, harness);
    expect(_onStage('Ship it on Friday?'), findsOneWidget);
  });

  testWidgets('⌘⌫ asks why first; a reason goes as the dismissal', (
    tester,
  ) async {
    DocumentNative();
    final harness = await _show(tester);
    await _run(tester, 'focus.no');
    expect(sentOf(harness, (m) => m.hasAnswerAsk()), isEmpty);
    expect(_onStage(focusDismissReasons[1]), findsOneWidget);
    expect(_onStage(focusDismissWithoutReason), findsOneWidget);
    await tester.tap(_onStage(focusDismissReasons[1]));
    await settle(tester);
    final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
    expect(answer.dismiss, isTrue);
    expect(answer.reply, isFalse);
    expect(answer.text, focusDismissReasons[1]);
  });

  testWidgets('a comment typed is a reply, sent with Return; the menu '
      'says Submit for ⌘↩ then', (tester) async {
    DocumentNative();
    final harness = await _show(tester);
    expect(_menu['focus.yes'], 'Approve');
    expect(_onStage('Reply to Builder'), findsOneWidget);
    final field = find.descendant(
      of: find.byKey(const ValueKey('review-answer')),
      matching: find.byType(EditableText),
    );
    await tester.enterText(field, 'Page 3 adds up');
    await settle(tester);
    expect(_onStage('Submit'), findsOneWidget);
    expect(
      _onStage('Submit sends your words instead of approving.'),
      findsOneWidget,
    );
    expect(_menu['focus.yes'], 'Submit');
    await tester.testTextInput.receiveAction(TextInputAction.send);
    await settle(tester);
    final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
    expect(answer.text, 'Page 3 adds up');
    expect(answer.reply, isTrue);
  });

  testWidgets('a refusal keeps the card, the words and the ink', (
    tester,
  ) async {
    final native = DocumentNative();
    final harness = await _show(tester);
    await native.drew(1, 2);
    await settle(tester);
    harness.backend.onSend = (m) {
      if (m.hasAnswerAsk()) {
        throw const RequestFailure('Builder stopped asking.');
      }
    };
    await _run(tester, 'focus.yes');
    expect(sentOf(harness, (m) => m.hasAnswerAsk()), hasLength(1));
    expect(find.text('Builder stopped asking.'), findsOneWidget);
    final session = _session(tester);
    expect(session.current?.seq, 7);
    expect(find.byType(DocumentReviewBar), findsOneWidget);
    // The refusal is the line now (rule 9); the ink is still there.
    expect(find.text('Your drawing goes with it.'), findsNothing);
    // Answerable again: it isn't on its way.
    harness.backend.onSend = null;
    await tester.tap(_onStage('Approve'));
    await settle(tester);
    final sent = sentOf(harness, (m) => m.hasAnswerAsk());
    expect(sent, hasLength(2));
    expect(sent.last.answerAsk.attachments, hasLength(1));
  });

  testWidgets('answered on another device while drawn on: the document '
      'stays, the bar says how it ended, and Next goes on', (tester) async {
    final native = DocumentNative();
    final harness = await _show(tester);
    await native.drew(2, 1);
    await settle(tester);
    final views = native.views.length;
    await _answered(tester, harness);
    final session = _session(tester);
    expect(session.current?.seq, 7);
    expect(find.byType(InkTools), findsOneWidget);
    expect(native.views.length, views);
    expect(find.byKey(const ValueKey('review-next')), findsOneWidget);
    expect(_onStage('Approve'), findsNothing);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('document-review-ended')),
        matching: find.text(session.outcomeOf(session.current!.key)!.words),
      ),
      findsOneWidget,
    );
    expect(sentOf(harness, (m) => m.hasAnswerAsk()), isEmpty);
    await tester.tap(find.byKey(const ValueKey('review-next')));
    await settle(tester);
    expect(_onStage('Ship it on Friday?'), findsOneWidget);
  });

  testWidgets('Slide Over: the phone\'s screen shows the same document, '
      'drawing and all, with the review bar as its answer', (tester) async {
    final native = DocumentNative();
    final harness = await _show(tester);
    await native.drew(2, 1);
    await settle(tester);
    final views = native.views.length;
    tester.view.physicalSize = const Size(375, 1032);
    tester.view.display.size = const Size(375, 1032);
    addTearDown(tester.view.reset);
    await settle(tester);
    expect(find.byType(PhoneNeedsYou), findsOneWidget);
    expect(find.byType(InkTools), findsOneWidget);
    expect(find.byType(DocumentReviewBar), findsOneWidget);
    // No second bar: the phone's thumb dock stays empty.
    expect(find.byKey(phoneThumbDockKey), findsNothing);
    // The same native view, its drawing on it.
    expect(native.views.length, views);
    expect(find.text('Your drawing goes with it.'), findsOneWidget);
    await _run(tester, 'focus.yes');
    final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
    expect([for (final a in answer.attachments) a.name], ['paper-p2.png']);
  });

  testWidgets('leaving focus mode takes the document away and closes '
      'nothing in the agent\'s layout', (tester) async {
    DocumentNative();
    final harness = await _show(tester);
    expect(find.byType(InkTools), findsOneWidget);
    setFocusMode(harness.backend, false);
    await settle(tester);
    expect(find.byType(IPadFocus), findsNothing);
    expect(find.byType(InkTools), findsNothing);
    expect(sentOf(harness, (m) => m.hasSetLayout()), isEmpty);
  });

  testWidgets('a picture to review is drawn on in the stage, and the '
      'drawing goes with the answer', (tester) async {
    final native = DocumentNative();
    final harness = await _show(tester, image: true);
    expect(find.byType(DocumentReviewBar), findsOneWidget);
    expect(find.byType(NeedsYouCard), findsNothing);
    // Native's image surface is ready, and the person draws on it.
    Future<void> say(String method, Map<String, Object?> arguments) =>
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .handlePlatformMessage(
              'claktory/ink/${native.views.last}',
              const StandardMethodCodec().encodeMethodCall(
                MethodCall(method, arguments),
              ),
              (_) {},
            );
    await say('ready', {
      'surface': 'image',
      'source_sha256': 'v1',
      'sha256_verified': true,
      'page_count': 1,
      'pages': [
        {'page_1based': null, 'width': 400.0, 'height': 200.0},
      ],
      'drawing_space': 'oriented_image',
    });
    native.drawnSeq[null] = 1;
    await say('drawingChanged', {
      'page_1based': null,
      'ink_layer_id': null,
      'base_revision': null,
      'local_seq': 1,
      'cause': 'stroke',
      'stroke_count': 1,
      'drawing': {
        'format': 'pencilkit',
        'content_version': 1,
        'bytes_base64': base64Encode([1, 2, 3]),
        'sha256': 'd',
      },
    });
    await settle(tester);
    expect(find.text('Your drawing goes with it.'), findsOneWidget);
    await _run(tester, 'focus.yes');
    final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
    expect([for (final a in answer.attachments) a.name], ['shot-drawing.png']);
    expect(answer.dismiss, isFalse);
  });
}
