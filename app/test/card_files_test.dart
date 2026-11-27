// Files with what the person writes on a card: the
// paperclip (and on a Mac files dropped or pasted on the window while the
// card's field has the keys) adds them under the field; they go up to the
// card's agent as the answer is sent, each once, and are named in it
// (AnswerAsk.attachments): with Approve or Dismiss, a written answer, a
// reply (files alone too), Resolve, and a step's Done or Can't do this.
// What was sent shows on the card: the talk's turns, the answer, a step.
import 'dart:io';

import 'package:factory_app/ask_card.dart' show AskPersonCard;
import 'package:factory_app/attachments.dart';
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/card_files.dart';
import 'package:factory_app/drop.dart' show WindowDropOverlay;
import 'package:factory_app/shell.dart' show Shell;
import 'package:factory_app/decision.dart' show DecisionComment;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'attachments_test.dart' show fromWindow, macWidgets;
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
      waitingSeqs: [Int64(4)],
    ),
  ],
  selectedAgentId: 'june',
);

Entry _card({
  bool wantsText = false,
  List<AskThreadMessage> thread = const [],
  List<AskStep> steps = const [],
  AskPersonState state = AskPersonState.ASK_PERSON_STATE_PENDING,
}) => Entry(
  seq: Int64(4),
  timestampMs: Int64(1004),
  askPerson: AskPerson(
    id: 'ask4',
    title: 'Is the new logo right?',
    state: state,
    revision: 1,
    wantsText: wantsText,
    thread: thread,
    steps: steps,
  ),
);

Future<Harness> _show(WidgetTester tester, Entry card) async {
  final harness = Harness(_workspace());
  harness.backend.transcript.load(Transcript(agentId: 'june', entries: [card]));
  await harness.show(tester, size: const Size(1200, 900));
  await settle(tester);
  return harness;
}

/// Lets real files be read: the sizes as they're added, the bytes as
/// they go up.
Future<void> _io(WidgetTester tester) async {
  for (var i = 0; i < 12; i++) {
    await tester.pump();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 10)),
    );
  }
  await tester.pump();
}

void main() {
  late Directory folder;
  late String shot;
  late String notes;
  final picked = <String>[];

  setUp(() {
    folder = Directory.systemTemp.createTempSync('card-files');
    shot = '${folder.path}/logo.png';
    notes = '${folder.path}/notes.txt';
    File(shot).writeAsBytesSync(List.filled(3000, 7));
    File(notes).writeAsStringSync('x' * 2048);
    picked
      ..clear()
      ..add(shot);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('factory/window'), (
          call,
        ) async {
          if (call.method == 'pickFiles') return [...picked];
          return null;
        });
  });

  tearDown(() {
    folder.deleteSync(recursive: true);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('factory/window'), null);
  });

  List<AnswerAsk> answers(Harness harness) => [
    for (final m in harness.backend.sent)
      if (m.hasAnswerAsk()) m.answerAsk,
  ];

  /// Opens Add a comment and attaches what the picker gives.
  Future<void> attach(WidgetTester tester) async {
    if (find.text('Add a comment').evaluate().isNotEmpty) {}
    await tester.tap(_cardClip());
    await _io(tester);
  }

  macWidgets('a file with Approve: up to June first, then named in the '
      'answer; the card forgets it once sent', (tester) async {
    final harness = await _show(tester, _card());
    await attach(tester);
    expect(find.text('logo.png'), findsOneWidget);
    expect(harness.backend.attachments.of(cardFilesKey('june', 'ask4')), [
      isA<LocalFile>().having((f) => f.name, 'name', 'logo.png'),
    ]);
    // Files alone keep Approve and Dismiss: they go with either.
    expect(find.text('Approve'), findsOneWidget);
    await tester.tap(find.text('Approve'));
    await _io(tester);
    final start = harness.backend.sent.firstWhere((m) => m.hasStartUpload());
    expect(
      (start.startUpload.agentId, start.startUpload.name),
      ('june', 'logo.png'),
    );
    final answer = answers(harness).single;
    expect(answer.dismiss, isFalse);
    expect(
      [for (final a in answer.attachments) a.path],
      ['/uploads/june/logo.png'],
    );
    expect(
      harness.backend.attachments.of(cardFilesKey('june', 'ask4')),
      isEmpty,
    );
  });

  macWidgets('× takes a file off before it goes', (tester) async {
    final harness = await _show(tester, _card());
    await attach(tester);
    await tester.tap(find.bySemanticsLabel('Remove logo.png'));
    await settle(tester);
    expect(find.text('logo.png'), findsNothing);
    await tester.tap(find.text('Dismiss'));
    await _io(tester);
    expect(answers(harness).single.attachments, isEmpty);
    expect(harness.backend.sent.where((m) => m.hasStartUpload()), isEmpty);
  });

  macWidgets('a written answer takes files too', (tester) async {
    final harness = await _show(tester, _card(wantsText: true));
    await tester.tap(_cardClip());
    await _io(tester);
    await tester.enterText(find.byType(EditableText).first, 'Use the blue');
    await settle(tester);
    await tester.tap(find.text('Answer'));
    await _io(tester);
    final answer = answers(harness).single;
    expect(answer.text, 'Use the blue');
    expect(answer.attachments.single.name, 'logo.png');
  });

  macWidgets('on a talk, files alone are a reply', (tester) async {
    final harness = await _show(
      tester,
      _card(
        wantsText: true,
        thread: [
          AskThreadMessage(text: 'Which blue?'),
          AskThreadMessage(fromAgent: true, text: 'The darker one.'),
        ],
      ),
    );
    await tester.tap(_cardClip());
    await _io(tester);
    await tester.tap(find.text('Reply'));
    await _io(tester);
    final answer = answers(harness).single;
    expect((answer.reply, answer.text), (true, ''));
    expect(answer.attachments.single.name, 'logo.png');
  });

  macWidgets("a step's Can't do this takes a screenshot of what's in the "
      'way', (tester) async {
    final harness = await _show(
      tester,
      _card(steps: [AskStep(text: 'Open Settings')]),
    );
    await tester.tap(_cardClip());
    await _io(tester);
    expect(find.text('logo.png'), findsOneWidget);
    await tester.tap(find.text("Can't do this"));
    await _io(tester);
    final answer = answers(harness).single;
    expect((answer.step, answer.dismiss), (0, true));
    expect(answer.attachments.single.name, 'logo.png');
  });

  macWidgets('refused, an answer sent again sends its files only once', (
    tester,
  ) async {
    final harness = await _show(tester, _card());
    await attach(tester);
    harness.backend.onSend = (m) {
      if (m.hasAnswerAsk()) throw const RequestFailure('Nope.');
    };
    await tester.tap(find.text('Approve'));
    await _io(tester);
    expect(find.text('logo.png'), findsOneWidget, reason: 'still there');
    harness.backend.onSend = null;
    await tester.tap(find.text('Approve'));
    await _io(tester);
    expect(harness.backend.sent.where((m) => m.hasStartUpload()), hasLength(1));
    expect(answers(harness).last.attachments.single.name, 'logo.png');
  });

  macWidgets("files dropped or pasted while the card's field has the keys "
      'go to the card, and the overlay says so', (tester) async {
    final harness = await _show(tester, _card());
    // The field never takes the keys by itself.
    expect(FileDrop.divertedTo.value, isNull);
    await tester.showKeyboard(
      find.descendant(
        of: find.byType(DecisionComment),
        matching: find.byType(EditableText),
      ),
    );
    await tester.pump();
    expect(FileDrop.divertedTo.value, 'your answer to June');
    FileDrop.deliver([notes]);
    await _io(tester);
    expect(
      harness.backend.attachments.of(cardFilesKey('june', 'ask4')).single.name,
      'notes.txt',
    );
    expect(harness.backend.attachments.of('june'), isEmpty);
    // The message box has the keys again: drops go to it.
    FocusManager.instance.primaryFocus?.unfocus();
    await settle(tester);
    expect(FileDrop.divertedTo.value, isNull);
  });

  macWidgets("files dropped from the Finder on a card's field that hasn't "
      "the keys go to the card, and the overlay says so; dropped elsewhere, "
      "to the message box (Abbacchio's row 1490)", (tester) async {
    final harness = Harness(_workspace());
    harness.backend.transcript.load(
      Transcript(agentId: 'june', entries: [_card(wantsText: true)]),
    );
    await harness.show(
      tester,
      size: const Size(1200, 900),
      home: const Stack(
        children: [
          Positioned.fill(child: Shell()),
          Positioned.fill(child: WindowDropOverlay()),
        ],
      ),
    );
    await settle(tester);
    expect(FileDrop.divertedTo.value, isNull, reason: 'nothing has the keys');
    final field = tester.getCenter(
      find
          .descendant(
            of: find.byType(AskPersonCard),
            matching: find.byType(EditableText),
          )
          .first,
    );
    await fromWindow(tester, 'filesOver', {
      'count': 1,
      'x': field.dx,
      'y': field.dy,
    });
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('Drop to add to your answer to June'), findsOneWidget);
    await fromWindow(tester, 'filesDropped', {
      'paths': [notes],
      'x': field.dx,
      'y': field.dy,
    });
    await _io(tester);
    expect(
      harness.backend.attachments.of(cardFilesKey('june', 'ask4')).single.name,
      'notes.txt',
    );
    expect(harness.backend.attachments.of('june'), isEmpty);
    // Off the card, over the transcript: the message box takes them.
    await fromWindow(tester, 'filesOver', {'count': 1, 'x': 700.0, 'y': 40.0});
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('Drop to add to your message to June'), findsOneWidget);
    await fromWindow(tester, 'filesDropped', {
      'paths': [shot],
      'x': 700.0,
      'y': 40.0,
    });
    await _io(tester);
    expect(harness.backend.attachments.of('june').single.name, 'logo.png');
    await tester.pump(const Duration(milliseconds: 200));
  });

  macWidgets('files dropped anywhere on a request go to it, not only on '
      'its field', (tester) async {
    final harness = Harness(_workspace());
    harness.backend.transcript.load(
      Transcript(agentId: 'june', entries: [_card(wantsText: true)]),
    );
    await harness.show(
      tester,
      size: const Size(1200, 900),
      home: const Stack(
        children: [
          Positioned.fill(child: Shell()),
          Positioned.fill(child: WindowDropOverlay()),
        ],
      ),
    );
    await settle(tester);
    final title = tester.getCenter(find.text('Is the new logo right?').last);
    await fromWindow(tester, 'filesOver', {
      'count': 1,
      'x': title.dx,
      'y': title.dy,
    });
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('Drop to add to your answer to June'), findsOneWidget);
    await fromWindow(tester, 'filesDropped', {
      'paths': [notes],
      'x': title.dx,
      'y': title.dy,
    });
    await _io(tester);
    expect(
      harness.backend.attachments.of(cardFilesKey('june', 'ask4')).single.name,
      'notes.txt',
    );
    expect(harness.backend.attachments.of('june'), isEmpty);
    await tester.pump(const Duration(milliseconds: 200));
  });

  macWidgets("a liaison's card says who else may get the files (Fugo)", (
    tester,
  ) async {
    final workspace = _workspace();
    workspace.agents.single.pipeId = 'pipe1';
    workspace.pipes.add(
      Pipe(
        id: 'pipe1',
        name: 'Acme launch',
        peer: PipePeer(claktoryName: "Ana's Claktory"),
      ),
    );
    final harness = Harness(workspace);
    harness.backend.transcript.load(
      Transcript(agentId: 'june', entries: [_card()]),
    );
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    expect(find.textContaining('can pass these files on'), findsNothing);
    await attach(tester);
    expect(
      find.text("June can pass these files on to Ana's Claktory."),
      findsOneWidget,
    );
  });

  testWidgets("on a phone, the card's paperclip opens the phone's picker, "
      'a finger\'s size', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('factory/device'), (
          call,
        ) async {
          if (call.method == 'pickAttachments') return [shot];
          return null;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('factory/device'),
            null,
          ),
    );
    final harness = Harness(_workspace());
    await harness.show(
      tester,
      size: const Size(393, 852),
      home: SingleChildScrollView(
        child: AskPersonCard(_card().askPerson, agentId: 'june', seq: 4),
      ),
    );
    await settle(tester);
    final clip = find.bySemanticsLabel('Attach files').first;
    expect(tester.getSize(clip).height, greaterThanOrEqualTo(44));
    await tester.tap(clip);
    await _io(tester);
    expect(
      harness.backend.attachments.of(cardFilesKey('june', 'ask4')).single.name,
      'logo.png',
    );
  });

  Attachment file(String name) =>
      Attachment(path: '/uploads/june/$name', name: name, size: Int64(10));

  testWidgets("what was sent shows on the card: the talk's turns and a "
      "step's comment", (tester) async {
    await _show(
      tester,
      _card(
        state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
        thread: [
          AskThreadMessage(text: 'See this', attachments: [file('a.txt')]),
          AskThreadMessage(fromAgent: true, text: 'Thanks.'),
        ],
        steps: [
          AskStep(
            text: 'Open Settings',
            state: AskStepState.ASK_STEP_STATE_CANT,
            comment: 'Greyed out',
            commentAttachments: [file('b.txt')],
          ),
        ],
      ),
    );
    expect(find.text('a.txt'), findsOneWidget);
    expect(find.text('b.txt'), findsOneWidget);
  });

  testWidgets('and with the answer', (tester) async {
    await _show(
      tester,
      _card(state: AskPersonState.ASK_PERSON_STATE_ANSWERED)
        ..askPerson.answer = 'Looks right'
        ..askPerson.answerAttachments.add(file('c.txt')),
    );
    expect(find.text('Looks right'), findsOneWidget);
    expect(find.text('c.txt'), findsOneWidget);
  });
}

/// The card's own paperclip: the first, above the message box's.
Finder _cardClip() => find.bySemanticsLabel('Attach files').first;
