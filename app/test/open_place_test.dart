// Where an entry elsewhere (a calendar's, a row of Device activity) opens:
// an agent's conversation, its checklist at an item, the Monitors page,
// its Tasks; and why one that has gone can't be opened.

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/layout.dart' show chatKind, chatOnly, leaves;
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/open_place.dart';
import 'package:factory_app/panes.dart' show pagePane, withPage;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:flutter/widgets.dart'
    show Center, Size, SizedBox, Text, ValueKey;
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'monitors_test.dart' show monitor;
import 'phone_test.dart' as phone;

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
)..selectedAgentId = 'a1';

void main() {
  test('a target whose agent or monitor has gone says so', () async {
    final harness = Harness(_workspace());
    harness.backend.monitorList.addAll([
      AgentMonitor(agentId: 'a2', monitor: monitor(3, 'Logs')),
      AgentMonitor(
        agentId: 'a2',
        monitor: monitor(
          5,
          'Backups',
          state: MonitorState.MONITOR_STATE_CANCELLED,
        ),
      ),
    ]);
    final backend = harness.backend;
    expect(await whyGone(backend, const OpenAgent('a2')), isNull);
    expect(
      await whyGone(backend, const OpenAgent('gone')),
      'This agent was deleted.',
    );
    expect(
      await whyGone(backend, const OpenItem('gone', 1)),
      'This agent was deleted.',
    );
    expect(await whyGone(backend, const OpenMonitor('a2', 3)), isNull);
    expect(
      await whyGone(backend, const OpenMonitor('a2', 4)),
      'This monitor was cancelled.',
    );
    expect(
      await whyGone(backend, const OpenMonitor('a1', 3)),
      'This monitor was cancelled.',
    );
    // Still listed once cancelled, with nothing left to open.
    expect(
      await whyGone(backend, const OpenMonitor('a2', 5)),
      'This monitor was cancelled.',
    );
  });

  test('an item or a task that has gone says so', () async {
    final harness = Harness(_workspace());
    final backend = harness.backend;
    backend.checklists['a1'] = Checklist(
      items: [ChecklistItem(id: 4, text: 'Ship it')],
    );
    backend.taskLists['a1'] = Tasks(
      tasks: [
        BackgroundTask(id: 'task-3', agentId: 'a1'),
        BackgroundTask(id: 'task-9', agentId: 'a2'),
      ],
    );
    expect(await whyGone(backend, const OpenItem('a1', 4)), isNull);
    expect(
      await whyGone(backend, const OpenItem('a1', 5)),
      'This item was deleted.',
    );
    // Its checklist off: nothing of it to open.
    expect(
      await whyGone(backend, const OpenItem('a2', 4)),
      'This item was deleted.',
    );
    expect(await whyGone(backend, const OpenTask('a1', 'task-3')), isNull);
    expect(
      await whyGone(backend, const OpenTask('a1', 'task-4')),
      'This task has ended.',
    );
    // Another agent's task, listed for one that sees all, is that agent's.
    expect(
      await whyGone(backend, const OpenTask('a1', 'task-9')),
      'This task has ended.',
    );
  });

  testWidgets('each target opens its place', (tester) async {
    final harness = Harness(_workspace());
    harness.backend.appliesLayouts = true;
    await harness.show(tester);
    await settle(tester);
    final context = tester.element(find.byType(Sidebar));
    List<ClientMessage> sent() => [...harness.backend.sent];

    harness.page.value = AppPage.usage;
    openTarget(context, const OpenAgent('a2'));
    await settle(tester);
    expect(harness.page.value, AppPage.agent);
    expect(sent().last.selectAgent.agentId, 'a2');

    openTarget(context, const OpenItem('a1', 4));
    await settle(tester);
    expect(
      sent().where((m) => m.hasOpenPanel()).last.openPanel,
      OpenPanel(agentId: 'a1', extensionId: 'checklist'),
    );
    expect(harness.backend.highlights.pointedAt.value?.keys, [
      'checklist_item:a1#4',
    ]);

    openTarget(context, const OpenTask('a2', 'task-3'));
    await settle(tester);
    expect(
      sent().where((m) => m.hasOpenPanel()).last.openPanel,
      OpenPanel(agentId: 'a2', extensionId: 'tasks'),
    );

    openTarget(context, const OpenMonitor('a2', 3));
    await settle(tester);
    expect(AppScope.pageShown(context, AppPage.monitors), isTrue);
    expect(harness.backend.highlights.pointedAt.value?.keys, ['monitor:a2#3']);
  });
  testWidgets("on an iPad, the sidebar over the window goes as a place "
      'opens from it', (tester) async {
    asIPad();
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    final harness = Harness(phone.workspace()..selectedAgentId = 'mike')
      ..backend.appliesLayouts = true;
    await harness.show(
      tester,
      size: const Size(744, 1133),
      home: WorkbenchScope(
        focus: focus,
        child: const Shell(rail: SizedBox(width: 56)),
      ),
    );
    await pumpFrames(tester);
    await tester.tap(find.byKey(const ValueKey('workbench-sidebar')));
    await pumpFrames(tester);
    expect(focus.overlayOpen, isTrue);

    // The agent shown already: its layout doesn't change, so nothing else
    // closes the sidebar.
    openTarget(
      tester.element(find.byType(Sidebar)),
      const OpenTask('mike', 'task-3'),
    );
    await pumpFrames(tester);
    expect(focus.overlayOpen, isFalse);
    expect(
      harness.backend.sent.where((m) => m.hasOpenPanel()).last.openPanel,
      OpenPanel(agentId: 'mike', extensionId: 'tasks'),
    );
  });

  testWidgets("on an iPad, a monitor or a conversation open as a tab behind "
      'another comes to the front', (tester) async {
    asIPad();
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    final shown = phone.workspace()..selectedAgentId = 'mike';
    final mike = shown.agents.firstWhere((agent) => agent.id == 'mike');
    mike.layout = withPage(chatOnly(), AppPage.monitors);
    final monitors = pagePane(mike.layout, AppPage.monitors)!.id;
    final chat = leaves(mike.layout)
        .firstWhere((leaf) => leaf.leaf.kind == chatKind)
        .id;
    final harness = Harness(shown)..backend.appliesLayouts = true;
    await harness.show(
      tester,
      size: const Size(1366, 1024),
      home: WorkbenchScope(
        focus: focus,
        child: const Shell(rail: SizedBox(width: 56)),
      ),
    );
    await pumpFrames(tester);
    expect(focus.focusedIn('mike', mike.layout), chat);

    openTarget(
      tester.element(find.byType(Sidebar)),
      const OpenMonitor('mike', 2),
    );
    await pumpFrames(tester);
    expect(focus.focusedIn('mike', mike.layout), monitors);

    openTarget(tester.element(find.byType(Sidebar)), const OpenAgent('mike'));
    await pumpFrames(tester);
    expect(focus.focusedIn('mike', mike.layout), chat);
  });

  testWidgets('on a phone, each place opens over the page it was opened '
      'from, and Back returns to that page', (tester) async {
    final harness = await phone.showPhone(tester, phone.workspace());
    const calendar = 'Calendar page';
    final routes = PhoneRoutes.of(
      tester.element(find.byType(AgentView, skipOffstage: false).first),
    )!;
    for (final target in const <OpenTarget>[
      OpenAgent('mike'),
      OpenItem('mike', 4),
      OpenTask('mike', 'task-3'),
      OpenMonitor('mike', 2),
    ]) {
      routes.push<void>(
        PhoneRoute(
          id: 'calendar',
          builder: (context) => const Center(child: Text(calendar)),
        ),
      );
      await pumpFrames(tester);
      openTarget(tester.element(find.text(calendar)), target);
      await harness.apply(tester, phone.workspace()..selectedAgentId = 'mike');
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.text(calendar), findsNothing, reason: '$target');
      expect(routes.canPop, isTrue, reason: '$target');
      routes.pop<void>();
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.text(calendar), findsOneWidget, reason: '$target');
      routes.pop<void>();
      await pumpFrames(tester, const Duration(seconds: 1));
    }
    final panels = [
      for (final m in harness.backend.sent)
        if (m.hasOpenPanel()) m.openPanel.extensionId,
    ];
    expect(panels, containsAllInOrder(['checklist', 'tasks']));
    expect(harness.backend.highlights.pointedAt.value?.keys, [
      'monitor:mike#2',
    ]);
  });
}
