import 'package:factory_app/backend.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/tasks.dart';
import 'package:factory_app/transcript.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart' show Text;
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

final _now = DateTime(2026, 3, 2, 14, 30);
Int64 _ago(Duration duration) =>
    Int64(_now.subtract(duration).millisecondsSinceEpoch);

ToolCall _call({
  String name = 'bash',
  ToolCallState state = ToolCallState.TOOL_CALL_STATE_RUNNING,
  Duration? startedAgo,
  Duration? endedAgo,
  int timeoutMs = 0,
}) => ToolCall(
  callId: 'c1',
  name: name,
  input: '{"command": "cargo test"}',
  state: state,
  started: startedAgo == null ? Int64.ZERO : _ago(startedAgo),
  ended: endedAgo == null ? Int64.ZERO : _ago(endedAgo),
  timeoutMs: Int64(timeoutMs),
);

Workspace tasksWorkspace() {
  final snapshot = workspace(
    agents: [
      builder(working: false)
        ..openPanel = 'tasks'
        ..extensions.add(AgentExtension(id: 'tasks', enabled: true)),
      Agent(
        id: 'a2',
        projectId: 'p1',
        name: 'Writer',
        color: 'green',
        modelAvailable: true,
      ),
    ],
  );
  snapshot.extensions.add(
    Extension(id: 'tasks', name: 'Tasks', icon: 'terminal'),
  );
  snapshot.openPanel = 'tasks';
  return snapshot;
}

BackgroundTask _task({
  String id = 'task-1',
  String agentId = 'a1',
  String command = 'npm run dev',
  required Duration startedAgo,
  Duration? ran,
  String status = '',
  String tail = '',
}) {
  final started = DateTime.now().subtract(startedAgo);
  return BackgroundTask(
    id: id,
    agentId: agentId,
    command: command,
    started: Int64(started.millisecondsSinceEpoch),
    ended: ran == null
        ? Int64.ZERO
        : Int64(started.add(ran).millisecondsSinceEpoch),
    running: ran == null,
    status: status,
    logPath: '/Users/x/.factory/agents/a1/tasks/$id.log',
    logTail: tail,
  );
}

ExtensionView tasksView(List<BackgroundTask> tasks) => ExtensionView(
  extensionId: 'tasks',
  agentId: 'a1',
  title: 'Tasks',
  tasks: Tasks(tasks: tasks),
);

Finder inPanel(Finder finder) =>
    find.descendant(of: find.byType(TasksView), matching: finder);

void main() {
  group('tool call clock', () {
    test('bash running: elapsed and when it times out', () {
      final call = _call(
        startedAgo: const Duration(minutes: 1, seconds: 23),
        timeoutMs: 600000,
      );
      expect(toolClock(call, _now), '1m 23s · times out in 8m 37s');
    });

    test('in the background bash has no limit', () {
      final call = _call(startedAgo: const Duration(seconds: 4));
      expect(toolClock(call, _now), '4s · no limit');
    });

    test('other tools just count', () {
      final call = _call(
        name: 'read_file',
        startedAgo: const Duration(seconds: 2),
      );
      expect(toolClock(call, _now), '2s');
    });

    test('finished calls say how long they took', () {
      expect(
        toolClock(
          _call(
            state: ToolCallState.TOOL_CALL_STATE_DONE,
            startedAgo: const Duration(seconds: 3, milliseconds: 400),
            endedAgo: Duration.zero,
            timeoutMs: 600000,
          ),
          _now,
        ),
        '3.4s',
      );
      expect(
        toolClock(
          _call(
            state: ToolCallState.TOOL_CALL_STATE_FAILED,
            startedAgo: const Duration(hours: 1, minutes: 5),
            endedAgo: Duration.zero,
          ),
          _now,
        ),
        '1h 05m',
      );
    });

    test('nothing for calls without times', () {
      expect(toolClock(_call(), _now), '');
      expect(
        toolClock(
          _call(
            state: ToolCallState.TOOL_CALL_STATE_DONE,
            startedAgo: const Duration(seconds: 3),
          ),
          _now,
        ),
        '',
      );
    });

    testWidgets('a running call ticks on its row', (tester) async {
      final harness = Harness(workspace(agents: [builder()]));
      final call = ToolCall(
        callId: 'c1',
        name: 'bash',
        input: '{"command": "cargo test"}',
        state: ToolCallState.TOOL_CALL_STATE_RUNNING,
        timeoutMs: Int64(600000),
      );
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [Entry(seq: Int64(1), toolCall: call)],
        ),
      );
      // The row reads the wall clock as it builds, which a loaded machine
      // can delay by seconds: it shows the clock at some moment between
      // these two, one of the texts that moment can give.
      final before = DateTime.now();
      call.started = Int64(
        before.subtract(const Duration(seconds: 5)).millisecondsSinceEpoch,
      );
      await harness.show(tester);
      final after = DateTime.now();
      final possible = {
        for (
          var moment = before;
          !moment.isAfter(after.add(const Duration(seconds: 1)));
          moment = moment.add(const Duration(milliseconds: 250))
        )
          toolClock(call, moment),
      };
      expect(possible, contains('5s · times out in 9m 55s'));
      final shown = find.byWidgetPredicate(
        (widget) => widget is Text && possible.contains(widget.data),
      );
      expect(shown, findsOneWidget);
      expect(find.byType(Ticking), findsOneWidget);
    });
  });

  group('Tasks panel', () {
    Future<Harness> show(
      WidgetTester tester,
      List<BackgroundTask> tasks,
    ) async {
      final harness = Harness(tasksWorkspace());
      harness.backend.extensionView.apply(tasksView(tasks));
      await harness.show(tester);
      await tester.pump(const Duration(milliseconds: 500));
      return harness;
    }

    testWidgets('running first, then finished, each with how it went', (
      tester,
    ) async {
      await show(tester, [
        _task(
          id: 'task-3',
          command: 'npm run dev',
          startedAgo: const Duration(minutes: 2, seconds: 5),
        ),
        _task(
          id: 'task-2',
          command: 'cargo test',
          startedAgo: const Duration(hours: 1),
          ran: const Duration(seconds: 42),
          status: 'exit 1',
          tail: 'test result: FAILED',
        ),
        _task(
          id: 'task-1',
          agentId: 'a2',
          command: 'make build',
          startedAgo: const Duration(hours: 2),
          ran: const Duration(minutes: 3, seconds: 7),
          status: 'stopped by the person',
        ),
      ]);
      expect(inPanel(find.text('Running')), findsOneWidget);
      expect(inPanel(find.text('Finished')), findsOneWidget);
      expect(
        tester.getTopLeft(inPanel(find.text('npm run dev'))).dy,
        lessThan(tester.getTopLeft(inPanel(find.text('Finished'))).dy),
      );
      expect(inPanel(find.text('2m 5s')), findsOneWidget);
      expect(inPanel(find.text('42s')), findsOneWidget);
      expect(inPanel(find.text('exit 1')), findsOneWidget);
      expect(inPanel(find.text('3m 7s')), findsOneWidget);
      expect(inPanel(find.text('stopped by the person')), findsOneWidget);
      // Another agent's task says whose it is; the panel agent's don't.
      expect(inPanel(find.text('Writer')), findsOneWidget);
      expect(inPanel(find.text('Builder')), findsNothing);
      // Only the running one can be stopped.
      expect(inPanel(find.text('Stop')), findsOneWidget);

      // A click shows the last of its output.
      expect(inPanel(find.text('test result: FAILED')), findsNothing);
      await tester.tap(inPanel(find.text('cargo test')));
      await tester.pump();
      expect(inPanel(find.text('test result: FAILED')), findsOneWidget);
      expect(
        inPanel(find.text('~/.factory/agents/a1/tasks/task-2.log')),
        findsOneWidget,
      );
    });

    testWidgets('a running task ticks', (tester) async {
      await show(tester, [_task(startedAgo: const Duration(seconds: 10))]);
      expect(inPanel(find.text('10s')), findsOneWidget);
      expect(find.byType(Ticking), findsWidgets);
    });

    testWidgets('Stop stops it', (tester) async {
      final harness = await show(tester, [
        _task(id: 'task-3', startedAgo: const Duration(seconds: 10)),
      ]);
      harness.backend.sent.clear();
      await tester.tap(inPanel(find.text('Stop')));
      await tester.pump();
      expect(harness.last.stopTask.agentId, 'a1');
      expect(harness.last.stopTask.taskId, 'task-3');

      // Asked once, it can't be asked again while it stops.
      await tester.tap(inPanel(find.text('Stop')));
      await tester.pump();
      expect(harness.backend.sent, hasLength(1));
    });

    testWidgets('a refused Stop says why', (tester) async {
      final harness = await show(tester, [
        _task(id: 'task-3', startedAgo: const Duration(seconds: 10)),
      ]);
      harness.backend.refuse = 'task-3 is not running';
      await tester.tap(inPanel(find.text('Stop')));
      await tester.pump();
      expect(inPanel(find.text('task-3 is not running')), findsOneWidget);
    });

    testWidgets('says when there are none', (tester) async {
      await show(tester, []);
      expect(
        inPanel(find.text('No background tasks in the last day.')),
        findsOneWidget,
      );
    });
  });

  test('which statuses are failures', () {
    expect(taskFailed('exit 0'), isFalse);
    expect(taskFailed('exit 2'), isTrue);
    expect(taskFailed('timed out'), isTrue);
    expect(taskFailed('signal SIGKILL'), isTrue);
    expect(taskFailed('stopped by the person'), isFalse);
    expect(formatDuration(const Duration(milliseconds: 400)), '0.4s');
    expect(
      formatDuration(const Duration(milliseconds: 4400), whole: true),
      '4s',
    );
    expect(RequestFailure('x').message, 'x');
  });
}
