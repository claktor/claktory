// Extension panels on an iPhone (#198 phase 3A; research mobile.md §4,
// "Map and secondary features"): each panel a sheet with its name and
// Close, every control in it a finger's size and labelled for VoiceOver,
// its text the phone's; the Mac's panels unchanged.
import 'dart:ui' show Tristate;

import 'package:factory_app/extensions.dart' show ExtensionPanel;
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';

import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, transcript, workspace;
import 'extensions_test.dart' show checklistView;
import 'harness.dart';
import 'memory_test.dart' show memory, memoryView;
import 'monitors_test.dart' show monitor, monitorsView;
import 'resources_test.dart' show sample;

final _extensions = [
  Extension(id: 'checklist', name: 'Checklist', icon: 'list-checks'),
  Extension(id: 'tasks', name: 'Tasks', icon: 'terminal'),
  Extension(id: 'monitors', name: 'Monitors', icon: 'alarm-clock'),
  Extension(id: 'agents', name: 'Messaging', icon: 'users'),
  Extension(id: 'memory', name: 'Memory', icon: 'brain'),
  Extension(id: 'resources', name: 'Resources', icon: 'gauge'),
  Extension(id: 'database', name: 'Database', icon: 'database'),
  Extension(
    id: 'web',
    name: 'Web',
    icon: 'globe',
    description: 'Search the web.',
  ),
];

Workspace _workspace(String open, {bool working = false}) {
  final snapshot = workspace(
    agents: [
      builder(working: working)
        ..openPanel = open
        ..extensions.addAll([
          for (final extension in _extensions)
            AgentExtension(id: extension.id, enabled: true),
        ]),
      Agent(
        id: 'a2',
        projectId: 'p1',
        name: 'Writer',
        color: 'green',
        modelAvailable: true,
      ),
    ],
  );
  snapshot.extensions.addAll(_extensions);
  snapshot.openPanel = open;
  return snapshot;
}

Int64 _ago(Duration duration) =>
    Int64(DateTime.now().subtract(duration).millisecondsSinceEpoch);

ExtensionView _tasks() => ExtensionView(
  extensionId: 'tasks',
  agentId: 'a1',
  title: 'Tasks',
  tasks: Tasks(
    tasks: [
      BackgroundTask(
        id: 'task-1',
        agentId: 'a1',
        command: 'npm run dev',
        started: _ago(const Duration(minutes: 3)),
        running: true,
        logPath: '/Users/x/.factory/agents/a1/tasks/task-1.log',
        logTail: 'ready on :3000',
      ),
      BackgroundTask(
        id: 'task-2',
        agentId: 'a2',
        command: 'cargo test --workspace',
        started: _ago(const Duration(hours: 2)),
        ended: _ago(const Duration(hours: 1)),
        status: 'exit 1',
      ),
    ],
  ),
);

ExtensionView _messaging() => ExtensionView(
  extensionId: 'agents',
  agentId: 'a1',
  title: 'Messaging',
  messaging: Messaging(reaches: ['a2'], reachedBy: ['a2']),
);

ExtensionView _database() => ExtensionView(
  extensionId: 'database',
  agentId: 'a1',
  title: 'Database',
  database: Database(
    path: '/Users/me/.factory/agents/a1/data.db',
    tables: [
      DatabaseTable(
        name: 'invoices',
        rows: Int64(12345),
        columns: [
          DatabaseColumn(name: 'id', type: 'INTEGER'),
          DatabaseColumn(name: 'note'),
        ],
      ),
      DatabaseTable(name: 'unpaid', view: true, rows: Int64.ONE),
    ],
  ),
);

ExtensionView _resources() => ExtensionView(
  extensionId: 'resources',
  agentId: 'a1',
  title: 'Resources',
  resources: sample(),
);

final _views = <String, ExtensionView Function()>{
  'checklist': checklistView,
  'tasks': _tasks,
  'monitors': () => monitorsView([
    monitor(1, 'Nightly build', nextIn: const Duration(minutes: 40)),
    monitor(
      2,
      'Inbox',
      state: MonitorState.MONITOR_STATE_PAUSED,
      when: 'check every 5m',
    ),
  ]),
  'agents': _messaging,
  'memory': () => memoryView(memory()),
  'resources': _resources,
  'database': _database,
  'web': () => ExtensionView(extensionId: 'web', agentId: 'a1'),
};

void main() {
  Future<Harness> phone(
    WidgetTester tester,
    String open, {
    Size size = const Size(393, 852),
  }) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace(open));
    harness.backend.extensionView.apply(_views[open]!());
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    await harness.show(tester, size: size);
    await pumpFrames(tester);
    expect(find.byType(PhoneShell), findsOneWidget);
    return harness;
  }

  /// Opens every row that opens (a task's output, a table's columns…),
  /// scrolling through the panel.
  Future<void> openAll(WidgetTester tester) async {
    final rows = find.semantics.byPredicate(
      (node) => node.flagsCollection.isExpanded == Tristate.isFalse,
    );
    final list = find.descendant(
      of: find.byType(ExtensionPanel),
      matching: find.byType(Scrollable),
    );
    // Down the panel a screen at a time, opening what's in view: a list
    // builds only what shows.
    for (var page = 0; page < 30; page++) {
      for (var i = 0; i < 12 && rows.evaluate().isNotEmpty; i++) {
        tester.semantics.performAction(rows.first, SemanticsAction.tap);
        await pumpFrames(tester);
      }
      if (list.evaluate().isEmpty) return;
      final position = tester.state<ScrollableState>(list.first).position;
      if (position.pixels >= position.maxScrollExtent) return;
      position.jumpTo(
        (position.pixels + position.viewportDimension * 0.8).clamp(
          0,
          position.maxScrollExtent,
        ),
      );
      await pumpFrames(tester);
    }
  }

  for (final open in _views.keys) {
    testWidgets('$open: fits a small phone at the largest text, opened', (
      tester,
    ) async {
      final semantics = tester.ensureSemantics();
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await phone(tester, open, size: const Size(320, 568));
      await openAll(tester);
      // Anything cut off fails it.
      semantics.dispose();
    });
  }

  for (final open in _views.keys) {
    testWidgets('$open: every control a finger\'s size, and labelled', (
      tester,
    ) async {
      final semantics = tester.ensureSemantics();
      await phone(tester, open);
      await expectLater(tester, meetsGuideline(iOSTapTargetGuideline));
      await expectLater(tester, meetsGuideline(labeledTapTargetGuideline));
      semantics.dispose();
    });
  }

  group('tool calls in a conversation', () {
    Transcript calls() => Transcript(
      agentId: 'a1',
      entries: [
        ...transcript().entries,
        for (final (i, (name, input, output)) in [
          (
            'send_message',
            '{"to": "a2", "text": "The build passes now."}',
            'Sent.',
          ),
          (
            'checklist_add',
            '{"items": [{"text": "Fix the header gap"}, {"text": "Usage circles"}]}',
            'Added #1, #2.',
          ),
          (
            'web_search',
            '{"queries": ["flutter sheets"]}',
            '1. Sheets\nhttps://example.com',
          ),
          ('memory_read', '{"slug": "deploy-steps"}', 'Run release.sh.'),
          (
            'monitors_add',
            '{"monitors": [{"name": "Nightly", "when": {"every": "24h"}}]}',
            'm1',
          ),
          ('tasks_list', '{}', '[]'),
          (
            'suggest_talk',
            '{"agent": "a2", "message": "Hi", "reason": "Yours"}',
            'Shown.',
          ),
        ].indexed)
          Entry(
            seq: Int64(100 + i),
            toolCall: ToolCall(
              callId: 'x$i',
              name: name,
              input: input,
              output: output,
              state: ToolCallState.TOOL_CALL_STATE_DONE,
            ),
          ),
      ],
    );

    Future<void> conversation(WidgetTester tester, {Size? size}) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      // At work: the line with Stop over the message box too.
      final harness = Harness(_workspace('', working: true));
      harness.backend.transcript.load(calls());
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      await harness.show(tester, size: size ?? const Size(393, 852));
      await pumpFrames(tester);
    }

    testWidgets("each a finger's size, and labelled", (tester) async {
      final semantics = tester.ensureSemantics();
      await conversation(tester);
      await expectLater(tester, meetsGuideline(iOSTapTargetGuideline));
      await expectLater(tester, meetsGuideline(labeledTapTargetGuideline));
      semantics.dispose();
    });

    testWidgets('they fit a small phone at the largest text, opened', (
      tester,
    ) async {
      final semantics = tester.ensureSemantics();
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await conversation(tester, size: const Size(320, 568));
      final closed = find.semantics.byPredicate(
        (node) => node.flagsCollection.isExpanded == Tristate.isFalse,
      );
      // The whole conversation, a screen at a time from its start: a list
      // builds only what shows.
      final list = find.byType(Scrollable).first;
      final position = tester.state<ScrollableState>(list).position;
      position.jumpTo(position.minScrollExtent);
      await pumpFrames(tester);
      var opened = 0;
      for (var page = 0; page < 60; page++) {
        for (var i = 0; i < 6 && closed.evaluate().isNotEmpty; i++) {
          tester.semantics.performAction(closed.first, SemanticsAction.tap);
          opened++;
          await pumpFrames(tester);
        }
        if (position.pixels >= position.maxScrollExtent) break;
        position.jumpTo(
          (position.pixels + position.viewportDimension * 0.8).clamp(
            position.minScrollExtent,
            position.maxScrollExtent,
          ),
        );
        await pumpFrames(tester);
      }
      expect(opened, greaterThan(5));
      semantics.dispose();
    });
  });

  testWidgets("⋯ lists the files, then what has something to show, then "
      'the rest behind Show more', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = await phone(tester, 'tasks');
    // The panel closed: the conversation in front.
    await harness.backend.openPanel('a1', '');
    harness.backend.workspace.apply(_workspace(''));
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel(RegExp('^More options for ')));
    await pumpFrames(tester);
    expect(find.text("Builder's extensions"), findsOneWidget);
    // The sheet has no title: its heading names it for VoiceOver, and
    // only that one.
    final heading = tester.getSemantics(find.text("Builder's extensions"));
    expect(heading.flagsCollection.namesRoute, isTrue);
    expect(heading.flagsCollection.isHeader, isTrue);
    for (final other in find.text('Files').evaluate()) {
      final node = tester.getSemantics(find.byWidget(other.widget));
      expect(node.flagsCollection.namesRoute, isFalse);
    }
    for (final name in [
      'Checklist',
      'Tasks',
      'Monitors',
      'Messaging',
      'Memory',
      'Resources',
      'Database',
    ]) {
      expect(find.text(name), findsWidgets, reason: name);
    }
    // Web only says what it does: behind Show more.
    expect(find.text('Web'), findsNothing);
    await expectLater(tester, meetsGuideline(iOSTapTargetGuideline));
    await tester.tap(find.text('Show 1 more'));
    await pumpFrames(tester);
    expect(find.text('Web'), findsOneWidget);
    await tester.tap(find.text('Memory'));
    await pumpFrames(tester);
    expect(
      harness.backend.sent
          .where((m) => m.hasOpenPanel())
          .last
          .openPanel
          .extensionId,
      'memory',
    );
    semantics.dispose();
  });

  testWidgets("a task's Stop is under it once open, and asks first", (
    tester,
  ) async {
    final harness = await phone(tester, 'tasks');
    expect(find.text('Stop'), findsNothing);
    await tester.tap(find.text('npm run dev'));
    await pumpFrames(tester);
    await tester.tap(find.text('Stop'));
    await pumpFrames(tester);
    expect(find.text('It stops right away.'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasStopTask()), isEmpty);
    await tester.tap(find.text('Stop task'));
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasStopTask()), hasLength(1));
  });

  testWidgets("a monitor's Cancel asks first; Keep monitor keeps it", (
    tester,
  ) async {
    final harness = await phone(tester, 'monitors');
    await tester.tap(find.text('Nightly build'));
    await pumpFrames(tester);
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(find.text("It won't fire again."), findsOneWidget);
    await tester.tap(find.text('Keep monitor'));
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasChangeMonitor()), isEmpty);
    await tester.tap(find.text('Pause'));
    await pumpFrames(tester);
    expect(
      harness.backend.sent.where((m) => m.hasChangeMonitor()),
      hasLength(1),
    );
  });

  testWidgets("a panel's header: its name and Close, a finger's size", (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    final harness = await phone(tester, 'memory');
    expect(find.text('Memory'), findsWidgets);
    final close = find.bySemanticsLabel('Close Memory');
    expect(tester.getSize(close).height, greaterThanOrEqualTo(44));
    await tester.tap(close);
    await pumpFrames(tester);
    expect(
      harness.backend.sent
          .where((m) => m.hasOpenPanel())
          .last
          .openPanel
          .extensionId,
      '',
    );
    semantics.dispose();
  });

  testWidgets("the Mac's panels are as they were: Stop beside the task, at "
      'once', (tester) async {
    final harness = Harness(_workspace('tasks'));
    harness.backend.extensionView.apply(_tasks());
    await harness.show(tester);
    await pumpFrames(tester);
    await tester.tap(find.text('Stop'));
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasStopTask()), hasLength(1));
    expect(find.text('It stops right away.'), findsNothing);
  });
}
