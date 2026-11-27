// The Monitors page: every agent's monitors in one place,
// by agent, filtered by agent, project and state (ListMonitors), each
// agent's name opening its conversation, a held one saying it isn't
// running, and a monitor's history a page at a time.

import 'package:factory_app/controls.dart' show Select;
import 'package:factory_app/monitors_overview.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;
import 'monitors_test.dart' show monitor;

Workspace _workspace() => workspace(
  agents: [
    builder(working: false),
    Agent(
      id: 'a2',
      projectId: 'p2',
      name: 'Writer',
      color: 'green',
      modelAvailable: true,
    ),
  ],
);

/// Builder's m1 and m2 (paused), Writer's m1 (its Monitors off).
Harness _harness() {
  final harness = Harness(_workspace());
  harness.backend.monitorList.addAll([
    AgentMonitor(agentId: 'a1', monitor: monitor(1, 'Nightly build')),
    AgentMonitor(
      agentId: 'a1',
      monitor: monitor(
        2,
        'Weekly report',
        state: MonitorState.MONITOR_STATE_PAUSED,
        reason: 'by you',
      ),
    ),
    AgentMonitor(
      agentId: 'a2',
      monitor: monitor(1, 'Check production logs', when: 'every 6h'),
      held: MonitorHeld.MONITOR_HELD_EXTENSION_OFF,
    ),
  ]);
  return harness;
}

Future<void> _open(
  WidgetTester tester,
  Harness harness, {
  Size size = const Size(1200, 900),
}) async {
  await harness.show(tester, size: size);
  await settle(tester);
  harness.page.value = AppPage.monitors;
  await pumpFrames(tester);
}

Iterable<ListMonitors> _asked(Harness harness) => harness.backend.sent
    .where((m) => m.hasListMonitors())
    .map((m) => m.listMonitors);

/// Chooses [value] in the filter titled [title], as its menu would.
Future<void> _choose<T>(WidgetTester tester, String title, T value) async {
  tester
      .widget<Select<T>>(
        find.byWidgetPredicate((w) => w is Select<T> && w.title == title),
      )
      .onChanged(value);
  await pumpFrames(tester);
}

void main() {
  // The real type, so words take the room they take in the app.
  setUpAll(loadGeist);

  testWidgets('the sidebar opens it; every agent\'s monitors by agent, the '
      'same number twice told apart', (tester) async {
    final harness = _harness();
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    // Beside the selected agent's conversation, as Usage opens.
    await openFromMore(tester, 'Monitors');
    await pumpFrames(tester);
    expect(
      harness.backend.sent
          .lastWhere((m) => m.hasSetLayout())
          .setLayout
          .toString(),
      contains('monitors'),
    );
    harness.page.value = AppPage.monitors;
    await pumpFrames(tester);
    final asked = _asked(harness).single;
    expect(asked.agentId, isEmpty);
    expect(asked.projectId, isEmpty);
    expect(asked.states, isEmpty);
    expect(find.text('Builder'), findsWidgets);
    expect(find.text('Writer'), findsWidgets);
    expect(find.text('Nightly build'), findsOneWidget);
    expect(find.text('Weekly report'), findsOneWidget);
    expect(find.text('Check production logs'), findsOneWidget);
    expect(find.text('m1'), findsNWidgets(2));
    // Builder's above Writer's.
    expect(
      tester.getTopLeft(find.text('Nightly build')).dy,
      lessThan(tester.getTopLeft(find.text('Check production logs')).dy),
    );
  });

  testWidgets('a held monitor says it isn\'t running', (tester) async {
    final harness = _harness();
    await _open(tester, harness);
    expect(
      find.text('Not running: Monitors is turned off for this agent'),
      findsOneWidget,
    );
  });

  testWidgets('Pause goes to the right agent\'s m1', (tester) async {
    final harness = _harness();
    await _open(tester, harness);
    final row = find.ancestor(
      of: find.text('Check production logs'),
      matching: find.byType(Row),
    );
    await tester.tap(
      find.descendant(of: row.first, matching: find.text('Pause')),
    );
    await pumpFrames(tester);
    final change = harness.backend.sent
        .lastWhere((m) => m.hasChangeMonitor())
        .changeMonitor;
    expect(change.agentId, 'a2');
    expect(change.monitorId, 1);
    expect(change.action, MonitorAction.MONITOR_ACTION_PAUSE);
  });

  testWidgets('filters by agent, project and state ask the backend', (
    tester,
  ) async {
    final harness = _harness();
    await _open(tester, harness);

    await _choose(tester, 'State', MonitorsShown.ended);
    expect(_asked(harness).last.states, [
      MonitorState.MONITOR_STATE_DONE,
      MonitorState.MONITOR_STATE_CANCELLED,
    ]);
    expect(find.text('No monitors match.'), findsOneWidget);

    await _choose(tester, 'State', MonitorsShown.active);
    expect(_asked(harness).last.states, [MonitorState.MONITOR_STATE_ACTIVE]);
    expect(find.text('Nightly build'), findsOneWidget);
    expect(find.text('Weekly report'), findsNothing);

    await _choose(tester, 'Agent', 'a2');
    expect(_asked(harness).last.agentId, 'a2');
    expect(find.text('Nightly build'), findsNothing);
    expect(find.text('Check production logs'), findsOneWidget);

    // Another project's agent goes with the project.
    await _choose(tester, 'Project', 'p1');
    expect(_asked(harness).last.projectId, 'p1');
    expect(_asked(harness).last.agentId, isEmpty);
    expect(find.text('Nightly build'), findsOneWidget);
    expect(find.text('Check production logs'), findsNothing);
  });

  testWidgets('a change anywhere asks again', (tester) async {
    final harness = _harness();
    await _open(tester, harness);
    expect(_asked(harness), hasLength(1));
    harness.backend.monitorList.removeAt(0);
    harness.backend.monitorsChanged.value++;
    await pumpFrames(tester);
    expect(_asked(harness), hasLength(2));
    expect(find.text('Nightly build'), findsNothing);
  });

  testWidgets('an agent\'s name opens its conversation', (tester) async {
    final harness = _harness();
    await _open(tester, harness);
    // Writer's group is the second.
    await tester.tap(find.text('Open conversation').last);
    await pumpFrames(tester);
    expect(harness.last.selectAgent.agentId, 'a2');
    expect(harness.page.value, AppPage.agent);
  });

  testWidgets('opened, a monitor shows its history: Fired, never Ran; Show '
      'older pages back', (tester) async {
    final harness = _harness();
    final base = DateTime(2026, 11, 23, 9).millisecondsSinceEpoch;
    MonitorEvent event(int seq, MonitorEventKind kind, String detail) =>
        MonitorEvent(
          seq: Int64(seq),
          timeMs: Int64(base + seq * 60000),
          kind: kind,
          detail: detail,
        );
    harness.backend.monitorHistories['a1/1'] = [
      for (var seq = 5; seq >= 2; seq--)
        event(seq, MonitorEventKind.MONITOR_EVENT_KIND_FIRED, 'on schedule'),
      event(1, MonitorEventKind.MONITOR_EVENT_KIND_CREATED, 'every 1h'),
    ];
    // An agent's doing is named by the agent, never by the detail's id.
    harness.backend.monitorHistories['a1/1']!.insert(
      0,
      event(6, MonitorEventKind.MONITOR_EVENT_KIND_FIRED, 'fired by a2')
        ..actor = 'a2',
    );
    harness.backend.monitorHistoryPage = 3;
    await _open(tester, harness);
    await tester.tap(find.text('Nightly build'));
    await pumpFrames(tester);
    expect(find.text('History'), findsOneWidget);
    expect(find.textContaining('Fired by Writer'), findsOneWidget);
    expect(find.textContaining('fired by a2'), findsNothing);
    expect(find.textContaining('Fired: on schedule'), findsNWidgets(2));
    expect(find.textContaining('Ran'), findsNothing);
    expect(find.textContaining('Set: every 1h'), findsNothing);
    await tester.ensureVisible(find.text('Show older'));
    await tester.tap(find.text('Show older'));
    await pumpFrames(tester);
    final asked = harness.backend.sent
        .lastWhere((m) => m.hasListMonitorHistory())
        .listMonitorHistory;
    expect(asked.agentId, 'a1');
    expect(asked.monitorId, 1);
    expect(asked.beforeSeq, Int64(4));
    expect(find.textContaining('Fired: on schedule'), findsNWidgets(4));
    expect(find.textContaining('Set: every 1h'), findsOneWidget);
    expect(find.text('Show older'), findsNothing);
  });

  testWidgets("the person's doing reads as an agent's: Paused by Matt "
      "(Giorno, Zucchero's row 408)", (tester) async {
    final harness = _harness();
    final base = DateTime(2026, 11, 23, 9).millisecondsSinceEpoch;
    MonitorEvent event(int seq, MonitorEventKind kind, String detail) =>
        MonitorEvent(
          seq: Int64(seq),
          timeMs: Int64(base + seq * 60000),
          kind: kind,
          detail: detail,
        );
    harness.backend.monitorHistories['a1/1'] = [
      event(4, MonitorEventKind.MONITOR_EVENT_KIND_RESUMED, 'resumed by a2')
        ..actor = 'a2',
      event(3, MonitorEventKind.MONITOR_EVENT_KIND_PAUSED, 'by Matt'),
      event(2, MonitorEventKind.MONITOR_EVENT_KIND_FIRED, 'on schedule'),
      event(1, MonitorEventKind.MONITOR_EVENT_KIND_CREATED, 'every 1h'),
    ];
    await _open(tester, harness);
    await tester.tap(find.text('Nightly build'));
    await pumpFrames(tester);
    expect(find.textContaining('Paused by Matt'), findsOneWidget);
    expect(find.textContaining('Paused: by Matt'), findsNothing);
    expect(find.textContaining('Resumed by Writer'), findsOneWidget);
    // Any other detail still follows a colon.
    expect(find.textContaining('Fired: on schedule'), findsOneWidget);
  });

  testWidgets('none yet, and a refusal says why', (tester) async {
    final harness = Harness(_workspace());
    await _open(tester, harness);
    expect(find.textContaining('No monitors yet.'), findsOneWidget);
    harness.backend.refuse = 'Claktory is busy.';
    harness.backend.monitorsChanged.value++;
    await pumpFrames(tester);
    expect(find.text('Claktory is busy.'), findsOneWidget);
  });

  testWidgets('on a phone it\'s a page that fits', (tester) async {
    final harness = _harness();
    await _open(tester, harness, size: const Size(390, 844));
    expect(find.text('Monitors'), findsWidgets);
    expect(find.text('Nightly build'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  // iOS's Large (the default) to its largest before the accessibility
  // sizes, on an SE.
  for (final scale in const [1.0, 1.12, 1.24, 1.35]) {
    testWidgets('at text size ×$scale on a small phone, the State filter '
        "keeps its choice on one line (Zucchero's rows 390 and 409)", (
      tester,
    ) async {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = _harness();
      await _open(tester, harness, size: const Size(320, 568));
      await _choose(tester, 'State', MonitorsShown.paused);
      final chosen = find.text('Paused or failed');
      final one = find.text('All agents');
      expect(chosen, findsWidgets);
      expect(
        tester.getSize(chosen.first).height,
        tester.getSize(one.first).height,
      );
      expect(tester.takeException(), isNull);
    });
  }
}
