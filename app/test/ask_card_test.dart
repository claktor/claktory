// An agent's question card (#238): its words select and copy wherever it
// shows, in a conversation and in Needs you, and a command in its body has
// Copy beside it.
import 'dart:io';

import 'package:factory_app/ask_card.dart';
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/controls.dart' show Pressable;
import 'package:factory_app/dictation.dart'
    show dictationWaiting, transcribeRecording;
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart' show deviceRecorder;
import 'package:factory_app/voice_keep.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'selection_test.dart' show copyItem, rightClick, watchClipboard;
import 'voice_test.dart' show FakeRecorder, io;

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

  testWidgets('what reorders text shows by its code; Copy copies the '
      'command as it is', (tester) async {
    final copied = watchClipboard(tester);
    final harness = Harness(_workspace());
    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          Entry(
            seq: Int64(7),
            timestampMs: Int64(1000),
            askPerson: AskPerson(
              id: 'ask8',
              title: 'Approve evil\u202Etxt.exe',
              body: 'Because \u2066this\u2069.\n\n```sh\nrm x\u200B y\n```',
              state: AskPersonState.ASK_PERSON_STATE_PENDING,
            ),
          ),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    expect(find.text(r'Approve evil\u{202E}txt.exe'), findsOneWidget);
    expect(
      find.textContaining(r'Because \u{2066}this\u{2069}.', findRichText: true),
      findsOneWidget,
    );
    expect(find.text(r'rm x\u{200B} y'), findsOneWidget);
    await tester.tap(find.text('Copy'));
    await tester.pump();
    expect(copied.last, 'rm x\u200B y');
  });

  testWidgets("Needs you names the ask by its title, what reorders it by "
      'its code', (tester) async {
    final harness = Harness(_workspace(selected: ''));
    harness.backend.entriesById['june/7'] = _entry()
      ..askPerson.title = 'Restart evil\u202Etxt.exe';
    await harness.show(tester);
    await settle(tester);
    await tester.tap(find.text('Needs you'));
    await settle(tester);
    await settle(tester);
    expect(find.text(r'Restart evil\u{202E}txt.exe'), findsWidgets);
    expect(find.textContaining('\u202E', findRichText: true), findsNothing);
  });

  testWidgets('an approval with a comment typed: one Submit, and Submit or '
      'Return sends it as a reply, never a yes; replied, it says so (#370)', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          Entry(seq: Int64(7), timestampMs: Int64(1000), askPerson: _ask()),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    expect(find.text('Approve'), findsOneWidget);
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    await tester.enterText(
      find.descendant(
        of: find.byType(AskPersonCard),
        matching: find.byType(EditableText),
      ),
      'No, staging first',
    );
    await settle(tester);
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Dismiss'), findsNothing);
    expect(find.text('Submit'), findsOneWidget);
    expect(
      find.text('June gets your comment and carries on from it.'),
      findsOneWidget,
    );
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    final answer = harness.last.answerAsk;
    expect((answer.text, answer.reply), ('No, staging first', true));
    // A Claktory from before Submit takes it as a comment, never a yes.
    expect(answer.dismiss, isTrue);

    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          Entry(
            seq: Int64(7),
            timestampMs: Int64(1000),
            askPerson: _ask()
              ..state = AskPersonState.ASK_PERSON_STATE_ANSWERED
              ..replied = true
              ..answer = 'No, staging first',
          ),
        ],
      ),
    );
    await settle(tester);
    expect(find.textContaining('Replied'), findsOneWidget);
    expect(find.textContaining('Approved'), findsNothing);
    expect(find.text('No, staging first'), findsOneWidget);
  });

  testWidgets('answered or dismissed, a card no longer says it needs you '
      '(Zucchero row 412)', (tester) async {
    final harness = Harness(_workspace());
    void load(AskPersonState state, {bool steps = false}) {
      final ask = _ask()..state = state;
      if (steps) ask.steps.add(AskStep(text: 'Open the console'));
      harness.backend.transcript.load(
        Transcript(
          agentId: 'june',
          entries: [
            Entry(seq: Int64(7), timestampMs: Int64(1000), askPerson: ask),
          ],
        ),
      );
    }

    load(AskPersonState.ASK_PERSON_STATE_PENDING);
    await harness.show(tester);
    await settle(tester);
    expect(find.text('June needs your answer'), findsOneWidget);
    for (final state in [
      AskPersonState.ASK_PERSON_STATE_ANSWERED,
      AskPersonState.ASK_PERSON_STATE_DISMISSED,
    ]) {
      for (final steps in [false, true]) {
        load(state, steps: steps);
        await settle(tester);
        expect(find.text('June asked'), findsOneWidget);
        expect(find.textContaining('June needs'), findsNothing);
      }
    }
  });

  testWidgets('a written answer dismissed sends what was typed as the '
      'comment, and the dismissed card shows it', (tester) async {
    final harness = Harness(_workspace());
    final ask = _ask()..wantsText = true;
    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          Entry(seq: Int64(7), timestampMs: Int64(1000), askPerson: ask),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    // Its answer box is the comment; no second field.
    expect(find.text('Add a comment'), findsNothing);
    await tester.enterText(
      find.descendant(
        of: find.byType(AskPersonCard),
        matching: find.byType(EditableText),
      ),
      'Do it after the release',
    );
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    expect(harness.last.answerAsk.dismiss, isTrue);
    expect(harness.last.answerAsk.text, 'Do it after the release');

    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          Entry(
            seq: Int64(7),
            timestampMs: Int64(1000),
            askPerson: ask.deepCopy()
              ..state = AskPersonState.ASK_PERSON_STATE_DISMISSED
              ..answer = 'Do it after the release',
          ),
        ],
      ),
    );
    await settle(tester);
    expect(find.textContaining('Left to June'), findsOneWidget);
    expect(find.text('Do it after the release'), findsOneWidget);
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

  testWidgets('from an agent that read a web page, it says so under who '
      'asks; a pipe it read says nothing (#346)', (tester) async {
    final harness = Harness(_workspace());
    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          _entry()
            ..askPerson.taint.addAll([
              PipeTaint(pipeName: 'Acme launch', pipeId: 'pp'),
              PipeTaint(kind: 'browser', pipeName: 'https://example.com'),
            ]),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    final read = find.text(
      'from June, which read a page on https://example.com',
    );
    expect(read, findsOneWidget);
    expect(find.textContaining('the pipe'), findsNothing);
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

  group('a procedure, one step at a time', () {
    AskPerson procedure({int done = 1}) => AskPerson(
      id: 'ask9',
      title: 'Sign the release',
      body: 'Four steps on the Mac with the signing key.',
      state: AskPersonState.ASK_PERSON_STATE_PENDING,
      steps: [
        for (final (i, text) in [
          'Plug in the YubiKey',
          'Run this:\n\n```sh\nscripts/sign.sh\n```',
          'Touch the key when it blinks',
          'Check the notarization email',
        ].indexed)
          AskStep(
            text: text,
            state: i < done
                ? AskStepState.ASK_STEP_STATE_DONE
                : AskStepState.ASK_STEP_STATE_PENDING,
            comment: i == 0 && done > 0 ? 'The blue one' : '',
            decidedMs: i < done ? Int64(2000) : Int64(0),
          ),
      ],
    );

    Future<Harness> show(WidgetTester tester, AskPerson ask) async {
      final harness = Harness(_workspace());
      harness.backend.transcript.load(
        Transcript(
          agentId: 'june',
          entries: [
            Entry(seq: Int64(7), timestampMs: Int64(1000), askPerson: ask),
          ],
        ),
      );
      await harness.show(tester);
      await settle(tester);
      return harness;
    }

    void update(Harness harness, AskPerson ask) =>
        harness.backend.transcript.load(
          Transcript(
            agentId: 'june',
            entries: [
              Entry(seq: Int64(7), timestampMs: Int64(1000), askPerson: ask),
            ],
          ),
        );

    testWidgets('a step done shows whole in the list, its command too '
        '(Zucchero)', (tester) async {
      await show(tester, procedure(done: 2));
      await tester.tap(find.textContaining('2 steps done', findRichText: true));
      await settle(tester);
      expect(
        find.text('2. Run this: scripts/sign.sh · Done', findRichText: true),
        findsOneWidget,
      );
    });

    testWidgets('shows only the step it is at, with the ones done counted, '
        'and Done answers that step', (tester) async {
      final harness = await show(tester, procedure());
      expect(find.text('June needs you to do something'), findsOneWidget);
      expect(find.text('Step 2 of 4'), findsOneWidget);
      expect(find.text('scripts/sign.sh'), findsOneWidget);
      expect(find.text('Copy'), findsOneWidget);
      // Neither the steps after it, nor the one done, until asked.
      expect(find.textContaining('Touch the key'), findsNothing);
      expect(find.textContaining('Plug in', findRichText: true), findsNothing);
      await tester.tap(find.textContaining('1 step done', findRichText: true));
      await settle(tester);
      expect(
        find.text('1. Plug in the YubiKey · Done', findRichText: true),
        findsOneWidget,
      );
      expect(find.text('The blue one'), findsOneWidget);

      await tester.tap(find.text('Add a comment'));
      await settle(tester);
      await tester.enterText(
        find.descendant(
          of: find.byType(AskPersonCard),
          matching: find.byType(EditableText),
        ),
        'Ran it twice',
      );
      await tester.tap(find.text('Done'));
      await settle(tester);
      final answer = harness.last.answerAsk;
      expect(answer.step, 1);
      expect(answer.hasStep(), isTrue);
      expect(answer.dismiss, isFalse);
      expect(answer.text, 'Ran it twice');

      // The entry comes back with that step done: the next, on a clean card.
      update(harness, procedure(done: 2));
      await settle(tester);
      expect(find.text('Step 3 of 4'), findsOneWidget);
      expect(find.text('Touch the key when it blinks'), findsOneWidget);
      expect(
        find.textContaining('2 steps done', findRichText: true),
        findsOneWidget,
      );
      expect(find.text('Ran it twice'), findsNothing);
    });

    testWidgets("Can't do this… asks what's in the way, and sends it with "
        'the step', (tester) async {
      final harness = await show(tester, procedure(done: 2));
      await tester.tap(find.text("Can't do this…"));
      await settle(tester);
      expect(find.text('Done'), findsNothing);
      // Its field has the keys at once: what's typed lands in it.
      expect(
        tester
            .widget<EditableText>(
              find.descendant(
                of: find.byType(AskPersonCard),
                matching: find.byType(EditableText),
              ),
            )
            .focusNode
            .hasFocus,
        isTrue,
      );
      await tester.enterText(
        find.descendant(
          of: find.byType(AskPersonCard),
          matching: find.byType(EditableText),
        ),
        'The key never blinks',
      );
      await tester.tap(find.text("Can't do this"));
      await settle(tester);
      final answer = harness.last.answerAsk;
      expect(
        (answer.step, answer.dismiss, answer.text),
        (2, true, 'The key never blinks'),
      );

      // Stopped there: how it ended, and the steps not reached.
      final stopped = procedure(done: 2)
        ..state = AskPersonState.ASK_PERSON_STATE_DISMISSED
        ..decidedMs = Int64(3000);
      stopped.steps[2]
        ..state = AskStepState.ASK_STEP_STATE_CANT
        ..comment = 'The key never blinks';
      update(harness, stopped);
      await settle(tester);
      expect(find.textContaining("Couldn't do step 3 of 4"), findsOneWidget);
      expect(find.text('The key never blinks'), findsOneWidget);
      expect(find.text('Done'), findsNothing);
      await tester.tap(find.textContaining('Steps', findRichText: true).last);
      await settle(tester);
      expect(
        find.text(
          '4. Check the notarization email · Not reached',
          findRichText: true,
        ),
        findsOneWidget,
      );
    });

    testWidgets('a refusal shows under it, and the card stays', (tester) async {
      final harness = await show(tester, procedure());
      harness.backend.refuse = 'That step was already answered.';
      await tester.tap(find.text('Done'));
      await settle(tester);
      expect(find.text('That step was already answered.'), findsOneWidget);
      expect(find.text('Step 2 of 4'), findsOneWidget);
    });

    testWidgets('all done: said so, every step there to see', (tester) async {
      final done = procedure(done: 4)
        ..state = AskPersonState.ASK_PERSON_STATE_ANSWERED
        ..decidedMs = Int64(3000);
      await show(tester, done);
      expect(find.textContaining('All 4 steps done'), findsOneWidget);
      expect(find.text('Done'), findsNothing);
      await tester.tap(find.textContaining('Steps', findRichText: true).last);
      await settle(tester);
      expect(
        find.textContaining('· Done', findRichText: true),
        findsNWidgets(4),
      );
    });

    testWidgets('Needs you names it by its title and the step it is at', (
      tester,
    ) async {
      final harness = Harness(_workspace(selected: ''));
      harness.backend.entriesById['june/7'] = Entry(
        seq: Int64(7),
        timestampMs: Int64(1000),
        askPerson: procedure(),
      );
      await harness.show(tester);
      await settle(tester);
      await tester.tap(find.text('Needs you'));
      await settle(tester);
      await settle(tester);
      expect(find.text('Sign the release · Step 2 of 4'), findsWidgets);
      await tester.tap(find.text('Sign the release · Step 2 of 4').first);
      await settle(tester);
      await tester.pump(const Duration(seconds: 1));
      expect(find.text('Step 2 of 4'), findsOneWidget);
    });

    testWidgets("on a phone the steps' toggle is a finger's height, its "
        'words centred', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = Harness(_workspace());
      harness.backend.transcript.load(
        Transcript(
          agentId: 'june',
          entries: [
            Entry(
              seq: Int64(7),
              timestampMs: Int64(1000),
              askPerson: procedure(),
            ),
          ],
        ),
      );
      await harness.show(tester, size: const Size(393, 852));
      await settle(tester);
      // The steps' toggle has a finger's height, its words centred in it
      // (Prosciutto: no gap under them).
      final toggle = find.textContaining('1 step done', findRichText: true);
      final box = tester.getRect(
        find.ancestor(of: toggle, matching: find.byType(Pressable)).first,
      );
      expect(box.height, greaterThanOrEqualTo(44));
      // The words their own height, not stretched to the box's and drawn
      // at its top.
      expect(tester.getSize(toggle).height, lessThan(box.height - 8));
      expect(tester.getCenter(toggle).dy, closeTo(box.center.dy, 1));
    });

    testWidgets('on a phone at the largest text it fits', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = Harness(_workspace());
      harness.backend.transcript.load(
        Transcript(
          agentId: 'june',
          entries: [
            Entry(
              seq: Int64(7),
              timestampMs: Int64(1000),
              askPerson: procedure(),
            ),
          ],
        ),
      );
      await harness.show(tester, size: const Size(320, 700));
      await settle(tester);
      await tester.ensureVisible(find.text('Done'));
      await settle(tester);
      expect(tester.takeException(), isNull);
    });
  });

  group('a typed answer on a weak connection (#401)', () {
    Future<Harness> show(WidgetTester tester) async {
      final harness = Harness(_workspace());
      harness.backend.transcript.load(
        Transcript(
          agentId: 'june',
          entries: [
            Entry(
              seq: Int64(7),
              timestampMs: Int64(1000),
              askPerson: _ask()..wantsText = true,
            ),
          ],
        ),
      );
      await harness.show(tester);
      await settle(tester);
      return harness;
    }

    Finder inCard(Finder finder) =>
        find.descendant(of: find.byType(AskPersonCard), matching: finder);

    int answers(Harness harness) =>
        harness.backend.sent.where((m) => m.hasAnswerAsk()).length;

    testWidgets('cut off, it stays in its field, "Not sent yet", and goes by '
        'itself once the connection is back', (tester) async {
      final harness = await show(tester);
      harness.backend.refuse = 'Connection lost.';
      await tester.enterText(inCard(find.byType(EditableText)), 'Ship it');
      await tester.pump();
      await tester.tap(inCard(find.text('Answer')));
      await settle(tester);
      expect(inCard(find.text(askWaiting)), findsOneWidget);
      expect(find.text('Ship it'), findsOneWidget);
      final tried = answers(harness);

      harness.backend.refuse = null;
      harness.backend.connected.value = false;
      await tester.pump();
      expect(answers(harness), tried);
      harness.backend.connected.value = true;
      await settle(tester);
      expect(answers(harness), tried + 1);
      expect(harness.last.answerAsk.text, 'Ship it');
      expect(inCard(find.text(askWaiting)), findsNothing);
    });

    testWidgets('sent again, an answer the Claktory already has counts as '
        'delivered', (tester) async {
      final harness = await show(tester);
      harness.backend.refuse = 'Connection lost.';
      await tester.enterText(inCard(find.byType(EditableText)), 'Ship it');
      await tester.pump();
      await tester.tap(inCard(find.text('Answer')));
      await settle(tester);
      harness.backend.refuse = 'This question was already answered.';
      harness.backend.connected.value = false;
      await tester.pump();
      harness.backend.connected.value = true;
      await settle(tester);
      expect(inCard(find.text(askWaiting)), findsNothing);
      expect(find.text('This question was already answered.'), findsNothing);
    });

    testWidgets('a refusal says why, and waits for the person', (tester) async {
      final harness = await show(tester);
      harness.backend.refuse = 'The agent was deleted.';
      await tester.enterText(inCard(find.byType(EditableText)), 'Ship it');
      await tester.pump();
      await tester.tap(inCard(find.text('Answer')));
      await settle(tester);
      expect(inCard(find.text('The agent was deleted.')), findsOneWidget);
      final tried = answers(harness);
      harness.backend.connected.value = false;
      await tester.pump();
      harness.backend.connected.value = true;
      await settle(tester);
      expect(answers(harness), tried);
    });
  });

  group('said rather than typed', () {
    late FakeRecorder recorder;
    setUp(() {
      final was = deviceRecorder;
      recorder = FakeRecorder();
      deviceRecorder = recorder;
      final wasTranscribe = transcribeRecording;
      transcribeRecording = (backend, recording) async => 'after the release';
      addTearDown(() {
        deviceRecorder = was;
        transcribeRecording = wasTranscribe;
      });
    });

    Future<Harness> show(WidgetTester tester, AskPerson ask) async {
      final harness = Harness(
        _workspace()
          ..speechModel = SpeechModel(
            state: SpeechModelState.SPEECH_MODEL_STATE_READY,
          ),
      );
      harness.backend.transcript.load(
        Transcript(
          agentId: 'june',
          entries: [
            Entry(seq: Int64(7), timestampMs: Int64(1000), askPerson: ask),
          ],
        ),
      );
      await harness.show(tester);
      await settle(tester);
      return harness;
    }

    Finder inCard(Finder finder) =>
        find.descendant(of: find.byType(AskPersonCard), matching: finder);

    /// Types [typed], then says the rest with the card's mic.
    Future<void> say(WidgetTester tester, String typed) async {
      await tester.enterText(inCard(find.byType(EditableText)), typed);
      await tester.pump();
      await tester.tap(inCard(find.byKey(const ValueKey('dictate'))));
      await io(tester);
      expect(recorder.recordingInto, isNotNull);
      await tester.tap(inCard(find.text('Done')));
      await io(tester);
      await pumpFrames(tester);
    }

    group('kept until its words come (#401)', () {
      late Directory kept;
      final asked = <KeptVoice>[];

      setUp(() {
        kept = Directory.systemTemp.createTempSync('kept');
        final was = keptVoiceFolder;
        keptVoiceFolder = () async => kept.path;
        asked.clear();
        addTearDown(() {
          keptVoiceFolder = was;
          kept.deleteSync(recursive: true);
        });
      });

      /// The recordings' folders kept now.
      List<Directory> folders() =>
          kept.listSync().whereType<Directory>().toList();

      Finder inCard(Finder finder) =>
          find.descendant(of: find.byType(AskPersonCard), matching: finder);

      /// Fails with [failure] the first time, then hears the words.
      void failFirst(RequestFailure failure) {
        transcribeRecording = (backend, recording) async {
          asked.add(recording as KeptVoice);
          if (asked.length == 1) {
            // As a dropped connection is: down, then the failure.
            if (failure.message == 'Connection lost.') {
              backend.connected.value = false;
            }
            throw failure;
          }
          return 'after the release';
        };
      }

      testWidgets('the connection lost while it transcribes: "Not sent yet", '
          'kept on the device; back, it goes by itself, with the same ids', (
        tester,
      ) async {
        failFirst(const RequestFailure('Connection lost.'));
        final harness = await show(tester, _ask()..wantsText = true);
        await say(tester, 'Do it');
        expect(inCard(find.text(dictationWaiting)), findsOneWidget);
        expect(inCard(find.text('Retry')), findsOneWidget);
        expect(inCard(find.text('Delete')), findsOneWidget);
        expect(find.text('Do it'), findsOneWidget);
        final folder = folders().single;
        final saved = KeptVoice.read(folder)!;
        expect(File(saved.path).existsSync(), isTrue);
        expect(saved.target, 'ask/june/ask7');
        expect(saved.kind, KeptKind.dictation);

        harness.backend.connected.value = false;
        await tester.pump();
        harness.backend.connected.value = true;
        await io(tester);
        await pumpFrames(tester);
        expect(find.text('Do it after the release'), findsOneWidget);
        expect(inCard(find.text(dictationWaiting)), findsNothing);
        expect(asked, hasLength(2));
        expect(asked[1].uploadId, asked[0].uploadId);
        expect(asked[1].requestId, asked[0].requestId);
        await io(tester);
        expect(folders(), isEmpty, reason: 'deleted once its words came');
      });

      testWidgets("a refusal says why, and waits for Retry", (tester) async {
        failFirst(const RequestFailure('Too many dictations waiting.'));
        await show(tester, _ask()..wantsText = true);
        await say(tester, 'Do it');
        expect(
          inCard(find.text('Not sent yet. Too many dictations waiting.')),
          findsOneWidget,
        );
        expect(folders(), hasLength(1));
        await tester.tap(inCard(find.text('Retry')));
        await io(tester);
        await pumpFrames(tester);
        expect(find.text('Do it after the release'), findsOneWidget);
        await io(tester);
        expect(folders(), isEmpty);
      });

      testWidgets('Delete asks first, then the recording is gone', (
        tester,
      ) async {
        failFirst(const RequestFailure('Connection lost.'));
        final harness = await show(tester, _ask()..wantsText = true);
        await say(tester, 'Do it');
        // Still not connected, so it waits.
        harness.backend.connected.value = false;
        await tester.tap(inCard(find.text('Delete')));
        await pumpFrames(tester);
        expect(find.text('Delete this recording?'), findsOneWidget);
        await tester.tap(find.text('Delete').last);
        await io(tester);
        await pumpFrames(tester);
        expect(inCard(find.text(dictationWaiting)), findsNothing);
        expect(find.text('Do it'), findsOneWidget);
        expect(folders(), isEmpty);
        expect(asked, hasLength(1));
      });

      testWidgets('after a relaunch, a recording kept for the card comes '
          'back to it and goes', (tester) async {
        transcribeRecording = (backend, recording) async {
          asked.add(recording as KeptVoice);
          return 'after the release';
        };
        final folder = Directory('${kept.path}/1-abc')..createSync();
        File('${folder.path}/Voice.m4a').writeAsBytesSync([1, 2, 3]);
        // Real files: outside the test's fake time.
        await tester.runAsync(
          () => KeptVoice(
            path: '${folder.path}/Voice.m4a',
            duration: const Duration(seconds: 7),
            claktory: keptClaktoryKey(null),
            kind: KeptKind.dictation,
            target: 'ask/june/ask7',
            uploadId: 'upload-1',
            requestId: 'request-1',
            uploaded: true,
          ).save(),
        );
        await show(tester, _ask()..wantsText = true);
        await io(tester);
        await pumpFrames(tester);
        expect(find.text('after the release'), findsOneWidget);
        expect(asked.single.requestId, 'request-1');
        await io(tester);
        expect(folders(), isEmpty);
      });
    });

    testWidgets('a written answer: the words go in after what was typed, '
        'and nothing goes until Answer', (tester) async {
      final harness = await show(tester, _ask()..wantsText = true);
      await say(tester, 'Do it');
      expect(find.text('Do it after the release'), findsOneWidget);
      expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
      await tester.tap(find.text('Answer'));
      await settle(tester);
      expect(harness.last.answerAsk.text, 'Do it after the release');
      expect(harness.last.answerAsk.dismiss, isFalse);
    });

    testWidgets("a step's what's in the way, said: sent with the step", (
      tester,
    ) async {
      final harness = await show(
        tester,
        AskPerson(
          id: 'ask9',
          title: 'Sign the release',
          state: AskPersonState.ASK_PERSON_STATE_PENDING,
          steps: [
            AskStep(text: 'Plug in the YubiKey'),
            AskStep(text: 'Touch the key when it blinks'),
          ],
        ),
      );
      await tester.tap(find.text("Can't do this…"));
      await settle(tester);
      await say(tester, 'Only');
      await tester.tap(find.text("Can't do this"));
      await settle(tester);
      final answer = harness.last.answerAsk;
      expect(
        (answer.step, answer.dismiss, answer.text),
        (0, true, 'Only after the release'),
      );
    });
  });

  group(
    'what is typed stays when the card scrolls away and back (row 193)',
    () {
      /// [finder] in the middle of the conversation, clear of its header
      /// and message box.
      Future<void> center(WidgetTester tester, Finder finder) =>
          Scrollable.ensureVisible(tester.element(finder), alignment: 0.5);

      /// [ask] at seq 40 of a conversation of 80, in sight, typed into: the
      /// conversation then goes to the top, back down to the newest, and
      /// back to the card.
      Future<Harness> typeAndScroll(
        WidgetTester tester,
        AskPerson ask,
        String typed, {
        String? open,
      }) async {
        final harness = Harness(
          Workspace(
            projects: [Project(id: 'p1', name: 'Factory')],
            agents: [
              Agent(
                id: 'june',
                name: 'June',
                projectId: 'p1',
                modelAvailable: true,
                status: AgentStatus.AGENT_STATUS_IDLE,
                waitingSeqs: [Int64(40)],
              ),
            ],
            selectedAgentId: 'june',
          ),
        );
        Entry said(int seq) => Entry(
          seq: Int64(seq),
          user: UserText(text: 'message $seq'),
        );
        harness.backend.transcript.load(
          Transcript(
            agentId: 'june',
            entries: [
              for (var i = 0; i < 40; i++) said(i),
              Entry(seq: Int64(40), timestampMs: Int64(1000), askPerson: ask),
              for (var i = 41; i < 80; i++) said(i),
            ],
          ),
        );
        // On a phone, as Zucchero found it: dragging the conversation puts
        // the keyboard away, so nothing focused keeps the card.
        debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        await harness.show(tester, size: const Size(390, 844));
        await pumpFrames(tester);
        final conversation = find.byType(CustomScrollView).first;
        final card = find.byType(AskPersonCard);
        Finder field() =>
            find.descendant(of: card, matching: find.byType(EditableText));
        await tester.dragUntilVisible(
          find.text(ask.title),
          conversation,
          const Offset(0, 300),
        );
        await tester.pump();
        if (open != null) {
          final button = find.descendant(of: card, matching: find.text(open));
          await center(tester, button);
          await tester.pump();
          await tester.tap(button);
          await pumpFrames(tester);
        }
        await center(tester, field());
        await tester.pump();
        await tester.enterText(field(), typed);
        await tester.pump();
        expect(tester.widget<EditableText>(field()).focusNode.hasFocus, isTrue);

        for (var i = 0; i < 3; i++) {
          await tester.drag(conversation, const Offset(0, 3000));
          await tester.pump();
        }
        expect(card, findsNothing);
        // The drag put the keyboard away.
        for (final field
            in find.byType(EditableText, skipOffstage: false).evaluate()) {
          expect((field.widget as EditableText).focusNode.hasFocus, isFalse);
        }
        for (var i = 0; i < 6; i++) {
          await tester.drag(conversation, const Offset(0, -3000));
          await tester.pump();
        }
        expect(find.text('message 79'), findsOneWidget);
        expect(card, findsNothing);
        await tester.dragUntilVisible(
          find.text(ask.title),
          conversation,
          const Offset(0, 300),
        );
        await pumpFrames(tester);
        expect(
          find.descendant(of: card, matching: find.text(typed)),
          findsOneWidget,
        );
        return harness;
      }

      testWidgets('a written answer', (tester) async {
        final harness = await typeAndScroll(
          tester,
          _ask()..wantsText = true,
          'papaya',
        );
        await center(tester, find.text('Answer'));
        await tester.pump();
        await tester.tap(find.text('Answer'));
        await settle(tester);
        expect(harness.last.answerAsk.text, 'papaya');
        expect(harness.last.answerAsk.dismiss, isFalse);
      });

      testWidgets("a step's what's in the way", (tester) async {
        final harness = await typeAndScroll(
          tester,
          AskPerson(
            id: 'ask9',
            title: 'Sign the release',
            state: AskPersonState.ASK_PERSON_STATE_PENDING,
            steps: [
              AskStep(text: 'Plug in the YubiKey'),
              AskStep(text: 'Touch the key when it blinks'),
            ],
          ),
          'The key is at home',
          open: "Can't do this…",
        );
        await center(tester, find.text("Can't do this"));
        await tester.pump();
        await tester.tap(find.text("Can't do this"));
        await settle(tester);
        final answer = harness.last.answerAsk;
        expect(
          (answer.step, answer.dismiss, answer.text),
          (0, true, 'The key is at home'),
        );
      });

      testWidgets("an approval's comment", (tester) async {
        final harness = await typeAndScroll(
          tester,
          _ask(),
          'After the release',
          open: 'Add a comment',
        );
        // Typed: Submit, the words a reply (#370).
        await center(tester, find.text('Submit'));
        await tester.pump();
        await tester.tap(find.text('Submit'));
        await settle(tester);
        expect(harness.last.answerAsk.text, 'After the release');
        expect(harness.last.answerAsk.reply, isTrue);
      });
    },
  );
}
