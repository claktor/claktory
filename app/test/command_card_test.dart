import 'package:factory_app/ask_card.dart' show askSummary;
import 'package:factory_app/command_card.dart';
import 'package:factory_app/identity.dart' show Palette;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// Execute cards: an agent asks the person to run a command on this
/// Claktory's computer. Claktory's heading and the command above the
/// agent's reason; Execute runs exactly what's shown, once, confirmed
/// again when Claktory warns; no reply; the run and its output on the card.
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
  String command = 'ls -la',
  List<CommandWarning> warnings = const [],
  AskPersonState state = AskPersonState.ASK_PERSON_STATE_PENDING,
  CommandRun? run,
}) => Entry(
  seq: Int64(4),
  timestampMs: Int64(1004),
  askPerson: AskPerson(
    id: 'ask4',
    title: 'List the build folder',
    body: 'To see why the **build** is so big.',
    state: state,
    revision: 2,
    command: AskCommand(
      command: command,
      cwd: '/Users/me/factory',
      machine: "Matt's Mac Studio",
      timeoutSeconds: 120,
      warnings: warnings,
      run: run,
    ),
  ),
);

Future<Harness> _show(WidgetTester tester, Entry card) async {
  final harness = Harness(_workspace());
  harness.backend.transcript.load(Transcript(agentId: 'june', entries: [card]));
  await harness.show(tester, size: const Size(1200, 900));
  await settle(tester);
  return harness;
}

Future<void> _again(Harness harness, WidgetTester tester, Entry card) async {
  harness.backend.transcript.load(Transcript(agentId: 'june', entries: [card]));
  await settle(tester);
}

void main() {
  testWidgets("Claktory's heading and the command above June's reason; "
      'Execute runs it, confirmed, with no reply to make', (tester) async {
    final harness = await _show(tester, _card());
    expect(find.text('June asks to run a command'), findsOneWidget);
    expect(find.text("Run this command on Matt's Mac Studio?"), findsOneWidget);
    expect(find.text('ls -la'), findsOneWidget);
    expect(find.text('Copy'), findsOneWidget);
    expect(
      find.text('In /Users/me/factory · Stops after 2 min'),
      findsOneWidget,
    );
    expect(find.text("June's reason"), findsOneWidget);
    expect(find.text('List the build folder'), findsOneWidget);
    // No approval to comment on, and no reply.
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Reply'), findsNothing);
    expect(
      find.byType(EditableText),
      findsNWidgets(2),
      reason: "the composer, and the card's field for why not",
    );

    await tester.tap(find.text('Execute'));
    await settle(tester);
    final answer = harness.last.answerAsk;
    expect(
      (
        answer.agentId,
        answer.seq.toInt(),
        answer.dismiss,
        answer.confirmed,
        answer.revision,
      ),
      ('june', 4, false, true, 2),
    );
  });

  testWidgets('Dismiss never runs it', (tester) async {
    final harness = await _show(tester, _card());
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    final answer = harness.last.answerAsk;
    expect(answer.dismiss, isTrue);
    expect(answer.confirmed, isFalse);
  });

  testWidgets("with Claktory's warnings, Execute asks again first", (
    tester,
  ) async {
    final harness = await _show(
      tester,
      _card(
        command: 'sudo rm -rf build',
        warnings: [
          CommandWarning(
            kind: CommandWarningKind.COMMAND_WARNING_KIND_ADMIN,
            text: 'Runs as administrator (sudo).',
          ),
          CommandWarning(
            kind: CommandWarningKind.COMMAND_WARNING_KIND_DESTRUCTIVE,
            text: 'Deletes files for good (rm -r).',
          ),
        ],
      ),
    );
    final warning = find.text('Runs as administrator (sudo).');
    expect(warning, findsOneWidget);
    final palette = Palette.of(tester.element(warning));
    expect(tester.widget<Text>(warning).style!.color, palette.danger);
    expect(find.text('Deletes files for good (rm -r).'), findsOneWidget);

    await tester.tap(find.text('Execute'));
    await settle(tester);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
    expect(
      find.text("Read the warnings above, then run it only if you're sure."),
      findsOneWidget,
    );
    // Cancel goes back, sending nothing.
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(find.text('Execute'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);

    await tester.tap(find.text('Execute'));
    await settle(tester);
    await tester.tap(find.text('Run anyway'));
    await settle(tester);
    expect(harness.last.answerAsk.confirmed, isTrue);
  });

  testWidgets('a hidden character in the command shows by its code', (
    tester,
  ) async {
    await _show(tester, _card(command: 'echo safe\u202E;rm x'));
    expect(find.textContaining('echo safe'), findsOneWidget);
    expect(find.textContaining('\u202E'), findsNothing);
  });

  testWidgets('its run on the card: running, what it waits for, and the end '
      'of its output', (tester) async {
    final harness = await _show(
      tester,
      _card(
        state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
        run: CommandRun(
          state: CommandRunState.COMMAND_RUN_STATE_RUNNING,
          durationMs: Int64(12000),
          device: "Matt's iPhone",
          waiting:
              "Waiting for a fingerprint or password on Matt's Mac Studio "
              '(sudo).',
          output: 'total 8\n',
        ),
      ),
    );
    expect(find.textContaining('Ran'), findsOneWidget);
    expect(find.text('Running · 12s'), findsOneWidget);
    expect(
      find.textContaining(" · Run from Matt's iPhone", findRichText: true),
      findsOneWidget,
    );
    // On the Mac it runs on, the sensor's right there.
    expect(
      find.text(
        "Waiting for a fingerprint or password on Matt's Mac Studio (sudo). "
        'Only touch the sensor if you started this.',
      ),
      findsOneWidget,
    );
    expect(find.text('total 8\n'), findsOneWidget);
    expect(find.text('Execute'), findsNothing);

    await _again(
      harness,
      tester,
      _card(
        state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
        run: CommandRun(
          state: CommandRunState.COMMAND_RUN_STATE_EXITED,
          exitCode: 1,
          durationMs: Int64(3400),
          output: 'the last lines\n',
          outputTruncated: true,
        ),
      ),
    );
    expect(
      find.text('Exited with code 1 · 3.4s', findRichText: true),
      findsOneWidget,
    );
    expect(find.text('Showing the end'), findsOneWidget);
    expect(find.textContaining('fingerprint'), findsNothing);

    await _again(
      harness,
      tester,
      _card(
        state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
        run: CommandRun(
          state: CommandRunState.COMMAND_RUN_STATE_FAILED_TO_START,
          error: 'Its folder, /Users/me/factory, isn\'t there any more.',
        ),
      ),
    );
    expect(find.text("Couldn't start", findRichText: true), findsOneWidget);
    expect(
      find.text("Its folder, /Users/me/factory, isn't there any more."),
      findsOneWidget,
    );
  });

  test("a run's output loses its terminal codes and shows hidden "
      'characters by their codes (Fugo)', () {
    expect(
      withoutTerminalCodes(
        '\x1B[31mFailed\x1B[0m \x1B]0;title\x07done\x1B]8;;https://x\x1B\\',
      ),
      'Failed done',
    );
    expect(
      commandHeading(AskCommand(machine: 'Mac\u202E')),
      'Run this command on Mac\\u{202E}?',
    );
  });

  testWidgets("the folder and the output: hidden characters by their codes", (
    tester,
  ) async {
    final card = _card(
      state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
      run: CommandRun(
        state: CommandRunState.COMMAND_RUN_STATE_EXITED,
        output: '\x1B[32mOK\x1B[0m\u202Edeliaf\n',
      ),
    )..askPerson.command.cwd = '/tmp/\u202Ex';
    await _show(tester, card);
    expect(find.textContaining('In /tmp/\\u{202E}x'), findsOneWidget);
    expect(find.textContaining('OK\\u{202E}deliaf'), findsOneWidget);
    expect(find.textContaining('\x1B'), findsNothing);
  });

  test('the run in a line, and how long it may run', () {
    AskCommand command(CommandRun run) =>
        AskCommand(timeoutSeconds: 90, run: run);
    expect(
      commandRunLine(
        command(
          CommandRun(
            state: CommandRunState.COMMAND_RUN_STATE_EXITED,
            durationMs: Int64(1200),
          ),
        ),
      ),
      'Finished · 1.2s',
    );
    expect(
      commandRunLine(
        command(CommandRun(state: CommandRunState.COMMAND_RUN_STATE_TIMED_OUT)),
      ),
      'Stopped after 1 min 30 s, as it ran too long',
    );
    expect(commandLimit(30), '30 s');
    expect(commandLimit(120), '2 min');
    expect(commandLimit(5400), '1 h 30 min');
    // Needs you names it by Claktory's heading, never the agent's title,
    // with the command's first line under it, so two for one machine can be
    // told apart.
    expect(
      askSummary(_card(command: '\n  ls -la ~/Downloads\nrm x').askPerson),
      "Run this command on Matt's Mac Studio?\nls -la ~/Downloads",
    );
    expect(
      askSummary(_card(command: 'echo \u202E').askPerson).split('\n').last,
      'echo \\u{202E}',
    );
  });

  test('while it runs, it counts up from when it started, here (row 487: '
      'the backend counts only for the conversation showing)', () {
    final started = DateTime(2026, 11, 26, 9);
    final run = CommandRun(
      state: CommandRunState.COMMAND_RUN_STATE_RUNNING,
      startedMs: Int64(started.millisecondsSinceEpoch),
    );
    String at(int ms) => commandRunLine(
      AskCommand(run: run),
      now: started.add(Duration(milliseconds: ms)),
    );
    expect(at(0), 'Running · 0s');
    expect(at(2400), 'Running · 2s');
    expect(at(65000), 'Running · 1m 5s');
    // What the backend said, when it's further along (a clock behind).
    run.durationMs = Int64(9000);
    expect(at(2400), 'Running · 9s');
    // Its own count, once it ends.
    run
      ..state = CommandRunState.COMMAND_RUN_STATE_EXITED
      ..durationMs = Int64(6100);
    expect(at(60000), 'Finished · 6.1s');
  });
}
