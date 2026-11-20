import 'package:factory_app/controls.dart';
import 'package:factory_app/monitors.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

Int64 _in(Duration duration) =>
    Int64(DateTime.now().add(duration).millisecondsSinceEpoch);

Monitor monitor(
  int id,
  String name, {
  MonitorState state = MonitorState.MONITOR_STATE_ACTIVE,
  String when = 'every 1h',
  String reason = '',
  Duration? nextIn,
  int fires = 0,
  int maxFires = 0,
  String createdBy = '',
}) => Monitor(
  id: id,
  name: name,
  prompt: 'Check the build and tell me what broke.',
  when: when,
  kind: 'every',
  state: state,
  reason: reason,
  nextDue: nextIn == null ? Int64.ZERO : _in(nextIn),
  fires: fires,
  maxFires: maxFires,
  createdBy: createdBy,
  created: Int64(DateTime(2026, 11, 18, 9).millisecondsSinceEpoch),
);

Workspace monitorsWorkspace() {
  final snapshot = workspace(
    agents: [
      builder(working: false)
        ..openPanel = 'monitors'
        ..extensions.add(AgentExtension(id: 'monitors', enabled: true)),
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
    Extension(id: 'monitors', name: 'Monitors', icon: 'alarm-clock'),
  );
  snapshot.openPanel = 'monitors';
  return snapshot;
}

ExtensionView monitorsView(List<Monitor> monitors, {List<String>? builtins}) =>
    ExtensionView(
      extensionId: 'monitors',
      agentId: 'a1',
      title: 'Monitors',
      monitors: Monitors(monitors: monitors, builtins: builtins ?? []),
    );

Finder inPanel(Finder finder) =>
    find.descendant(of: find.byType(MonitorsView), matching: finder);

Future<Harness> show(
  WidgetTester tester,
  List<Monitor> monitors, {
  List<String>? builtins,
}) async {
  final harness = Harness(monitorsWorkspace());
  harness.backend.extensionView.apply(
    monitorsView(monitors, builtins: builtins),
  );
  await harness.show(tester);
  await tester.pump(const Duration(milliseconds: 500));
  harness.backend.sent.clear();
  return harness;
}

Finder buttonIn(String row, String label) => find.descendant(
  of: find.ancestor(of: find.text(row), matching: find.byType(MonitorRow)),
  matching: find.text(label),
);

void main() {
  testWidgets('active, paused and ended monitors, then what else wakes it', (
    tester,
  ) async {
    final made = DateTime.now();
    await show(
      tester,
      [
        monitor(
          1,
          'Nightly build',
          nextIn: const Duration(minutes: 4, seconds: 12),
          fires: 3,
        ),
        monitor(
          2,
          'Tests',
          when: 'check `make test` every 5m, fires on failure',
          state: MonitorState.MONITOR_STATE_ERROR,
          reason: 'its command kept failing',
        ),
        monitor(
          3,
          'Inbox',
          state: MonitorState.MONITOR_STATE_PAUSED,
          reason: 'by the person',
          createdBy: 'a2',
        ),
        monitor(
          4,
          'Deploy',
          state: MonitorState.MONITOR_STATE_DONE,
          reason: 'its until passed',
          fires: 2,
          maxFires: 5,
        ),
      ],
      builtins: ['task-3 (background task) — wakes you when it exits'],
    );
    // Tall enough for every section at once.
    tester.view.physicalSize = const Size(1200, 1800);
    await tester.pump();
    double top(String text) => tester.getTopLeft(inPanel(find.text(text))).dy;
    expect(top('Active'), lessThan(top('Nightly build')));
    expect(top('Nightly build'), lessThan(top('Paused')));
    expect(top('Paused'), lessThan(top('Tests')));
    expect(top('Inbox'), lessThan(top('Ended')));
    expect(top('Ended'), lessThan(top('Deploy')));
    expect(top('Deploy'), lessThan(top('Also wakes it')));

    expect(inPanel(find.text('every 1h')), findsWidgets);
    // The real clock moves while the test runs (seconds, on a busy
    // machine), so any count from 4m 12s down to what it's been since.
    final next = inPanel(find.textContaining(RegExp(r'^next in 4m \d+s$')));
    expect(next, findsOneWidget);
    final left = int.parse(
      RegExp(r'(\d+)s$').firstMatch(tester.widget<Text>(next).data!)!.group(1)!,
    );
    final since = DateTime.now().difference(made).inSeconds;
    expect(left, inInclusiveRange(12 - since - 1, 12));
    expect(inPanel(find.text('3 fires')), findsOneWidget);
    expect(inPanel(find.text('2 of 5 fires')), findsOneWidget);
    expect(inPanel(find.text('never fired')), findsWidgets);
    expect(
      inPanel(find.text('Failed: its command kept failing')),
      findsOneWidget,
    );
    expect(inPanel(find.text('Paused by the person')), findsOneWidget);
    expect(inPanel(find.text('Done: its until passed')), findsOneWidget);
    expect(
      inPanel(find.text('task-3 (background task) — wakes you when it exits')),
      findsOneWidget,
    );

    // Buttons by state: an ended one has none.
    expect(buttonIn('Nightly build', 'Pause'), findsOneWidget);
    expect(buttonIn('Nightly build', 'Cancel'), findsOneWidget);
    expect(buttonIn('Tests', 'Resume'), findsOneWidget);
    expect(buttonIn('Inbox', 'Resume'), findsOneWidget);
    expect(buttonIn('Deploy', 'Cancel'), findsNothing);
    expect(buttonIn('Deploy', 'Resume'), findsNothing);

    // A click shows what it tells the agent, and who set it.
    expect(inPanel(find.text('Tells it')), findsNothing);
    await tester.tap(inPanel(find.text('Inbox')));
    await tester.pump();
    expect(inPanel(find.text('Tells it')), findsOneWidget);
    expect(
      inPanel(find.textContaining('Writer', findRichText: true)),
      findsOneWidget,
    );
  });

  testWidgets('the next fire counts down', (tester) async {
    await show(tester, [
      monitor(1, 'Soon', nextIn: const Duration(seconds: 10)),
    ]);
    expect(inPanel(find.byType(Ticking)), findsOneWidget);
    expect(
      inPanel(find.textContaining(RegExp(r'^next in (9|10)s$'))),
      findsOneWidget,
    );
  });

  testWidgets('Pause, Resume and Cancel ask the backend', (tester) async {
    final harness = await show(tester, [
      monitor(1, 'Nightly build', nextIn: const Duration(hours: 1)),
      monitor(3, 'Inbox', state: MonitorState.MONITOR_STATE_PAUSED),
    ]);
    await tester.tap(buttonIn('Nightly build', 'Pause'));
    await tester.pump();
    expect(
      harness.last.changeMonitor,
      ChangeMonitor(
        agentId: 'a1',
        monitorId: 1,
        action: MonitorAction.MONITOR_ACTION_PAUSE,
      ),
    );
    await tester.tap(buttonIn('Inbox', 'Resume'));
    await tester.pump();
    expect(harness.last.changeMonitor.monitorId, 3);
    expect(
      harness.last.changeMonitor.action,
      MonitorAction.MONITOR_ACTION_RESUME,
    );
    await tester.tap(buttonIn('Inbox', 'Cancel'), warnIfMissed: false);
    await tester.pump();
    // Asked once, it waits for the answer.
    expect(harness.backend.sent, hasLength(2));
  });

  testWidgets('a refused change says why', (tester) async {
    final harness = await show(tester, [
      monitor(1, 'Nightly build', nextIn: const Duration(hours: 1)),
    ]);
    harness.backend.refuse = 'm1 is already cancelled';
    await tester.tap(buttonIn('Nightly build', 'Cancel'));
    await tester.pump();
    expect(inPanel(find.text('m1 is already cancelled')), findsOneWidget);
    expect(buttonIn('Nightly build', 'Pause'), findsOneWidget);
  });

  testWidgets('says when there are none', (tester) async {
    await show(tester, []);
    expect(inPanel(find.textContaining('No monitors yet.')), findsOneWidget);
  });

  testWidgets('a monitor firing is a line in the conversation', (tester) async {
    final harness = Harness(workspace(agents: [builder(working: false)]));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            event: Event(
              source: EventSource.EVENT_SOURCE_MONITOR,
              from: 'Nightly build',
              fromId: 'm1',
              summary: 'm1 Nightly build fired',
              text: 'Check the build and tell me what broke.',
            ),
          ),
        ],
      ),
    );
    await harness.show(tester);
    expect(find.text('m1 Nightly build fired'), findsOneWidget);
    expect(find.textContaining('tell me what broke'), findsNothing);
    await tester.tap(find.text('m1 Nightly build fired'));
    await settle(tester);
    expect(find.textContaining('tell me what broke'), findsOneWidget);
  });

  test('state labels', () {
    expect(
      monitorStateLabel(
        monitor(
          1,
          'x',
          state: MonitorState.MONITOR_STATE_CANCELLED,
          reason: 'by coordinator',
        ),
      ),
      'Cancelled by coordinator',
    );
    expect(
      nextDueLabel(
        monitor(1, 'x', nextIn: const Duration(seconds: -5)),
        DateTime.now(),
      ),
      'due now',
    );
  });
}
