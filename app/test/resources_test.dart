import 'package:factory_app/forms.dart' show AgentListField;
import 'package:factory_app/identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/resources.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

const gb = 1000 * 1000 * 1000;

Workspace resourcesWorkspace() {
  final snapshot = workspace(
    agents: [
      builder(working: false)
        ..openPanel = 'resources'
        ..extensions.add(AgentExtension(id: 'resources', enabled: true)),
    ],
  );
  snapshot.extensions.add(
    Extension(id: 'resources', name: 'Resources', icon: 'gauge'),
  );
  snapshot.openPanel = 'resources';
  snapshot.settings
    ..ramPercent = 90
    ..diskFreeGb = 10
    ..diskFreePercent = 10
    ..cpuPercent = 0;
  return snapshot;
}

Resources sample({List<String> alerts = const [], bool measured = true}) {
  final today = DateTime.now();
  final now = DateTime(
    today.year,
    today.month,
    today.day,
    14,
    2,
  ).millisecondsSinceEpoch;
  return Resources(
    sampled: Int64(now),
    measured: Int64(measured ? now - 5 * 60000 : 0),
    alerts: alerts,
    machine: Machine(
      cpuPercent: 34.4,
      cores: 10,
      memoryTotal: Int64(32 * gb),
      memoryUsed: Int64(30 * gb),
      swapTotal: Int64(4 * gb),
      swapUsed: Int64(1200 * 1000 * 1000),
      volumes: [
        Volume(
          mount: '/',
          total: Int64(994 * gb),
          available: Int64(212 * gb),
          uses: ["Claktory's data", 'Site'],
        ),
        Volume(
          mount: '/Volumes/Work',
          total: Int64(500 * gb),
          available: Int64(8 * gb),
          uses: ['Site'],
        ),
      ],
    ),
    factory: [
      ResourceProcess(
        label: 'Claktory service',
        kind: 'backend',
        pid: 100,
        processes: 1,
        cpuPercent: 3.2,
        memory: Int64(180 * 1000 * 1000),
      ),
      ResourceProcess(
        label: 'task-3 npm run dev',
        kind: 'task',
        agentId: 'a1',
        pid: 200,
        processes: 4,
        cpuPercent: 112,
        memory: Int64(1500 * 1000 * 1000),
      ),
    ],
    others: [
      ResourceProcess(
        label: 'Docker',
        kind: 'other',
        processes: 1,
        cpuPercent: 8,
        memory: Int64(4 * gb),
      ),
    ],
    history: [
      for (var i = 0; i < 60; i++)
        ResourceSample(
          time: Int64(now - (59 - i) * 60000),
          cpuPercent: 20 + i / 3,
          memoryUsed: Int64(28 * gb + i * gb ~/ 30),
        ),
    ],
    disk: [
      DiskUsage(
        label: "Claktory's data",
        kind: 'home',
        path: '/Users/me/.factory',
        bytes: Int64(42 * gb),
        parts: [
          DiskUsage(label: 'Releases', kind: 'releases', bytes: Int64(2 * gb)),
          DiskUsage(
            label: 'Checkouts',
            kind: 'checkout',
            bytes: Int64(38 * gb),
          ),
        ],
      ),
      DiskUsage(
        label: 'Builder',
        kind: 'agent',
        id: 'a1',
        path: '/Users/me/.factory/agents/a1',
        bytes: Int64(19 * gb),
        parts: [
          DiskUsage(
            label: 'Conversations',
            kind: 'conversations',
            id: 'a1',
            bytes: Int64(120 * 1000 * 1000),
          ),
          DiskUsage(
            label: 'Checkout',
            kind: 'checkout',
            id: 'a1',
            bytes: Int64(18 * gb),
          ),
          DiskUsage(
            label: 'Build cache (target)',
            kind: 'build_cache',
            id: 'a1',
            path: '/Users/me/.factory/agents/a1/checkout/target',
            bytes: Int64(9 * gb),
          ),
        ],
      ),
      DiskUsage(
        label: 'Factory',
        kind: 'project',
        id: 'p1',
        path: '/Users/me/factory',
        bytes: Int64(3 * gb),
        partial: true,
      ),
    ],
  );
}

Future<Harness> show(WidgetTester tester, Resources resources) async {
  final harness = Harness(resourcesWorkspace());
  harness.backend.extensionView.apply(
    ExtensionView(
      extensionId: 'resources',
      agentId: 'a1',
      title: 'Resources',
      resources: resources,
    ),
  );
  await harness.show(tester);
  await tester.pump(const Duration(milliseconds: 500));
  return harness;
}

Finder inPanel(String text) =>
    find.descendant(of: find.byType(ResourcesView), matching: find.text(text));

Future<void> reveal(WidgetTester tester, String text) =>
    tester.scrollUntilVisible(
      inPanel(text),
      200,
      scrollable: find.descendant(
        of: find.byType(ResourcesView),
        matching: find.byType(Scrollable),
      ),
    );

void main() {
  test('bytes read like the Finder', () {
    expect(formatBytes(Int64(0)), '0 bytes');
    expect(formatBytes(Int64(1)), '1 byte');
    expect(formatBytes(Int64(830000)), '830 KB');
    expect(formatBytes(Int64(212000000000)), '212 GB');
    expect(formatBytes(Int64(12400000000)), '12.4 GB');
    expect(formatBytes(Int64(999600000)), '1.0 GB');
    expect(formatBytes(Int64(1200000000000)), '1.2 TB');
  });

  testWidgets(
    "alerts first, then the machine's gauges, past a threshold in danger",
    (tester) async {
      await show(
        tester,
        sample(alerts: ['Memory is 94% used: 30.0 GB of 32.0 GB.']),
      );
      expect(
        inPanel('Memory is 94% used: 30.0 GB of 32.0 GB.'),
        findsOneWidget,
      );
      expect(inPanel('Updated at 14:02, every minute'), findsOneWidget);
      expect(inPanel('34% of 10 cores'), findsOneWidget);
      expect(inPanel('30.0 GB of 32.0 GB'), findsOneWidget);
      expect(inPanel('1.2 GB of 4.0 GB'), findsOneWidget);
      expect(inPanel('212 GB free of 994 GB'), findsOneWidget);
      expect(inPanel("Claktory's data, Site"), findsOneWidget);
      expect(inPanel('8.0 GB free of 500 GB'), findsOneWidget);

      final palette = Palette.of(tester.element(find.byType(ResourcesView)));
      Color colorOf(String text) =>
          tester.widget<Text>(inPanel(text)).style!.color!;
      expect(colorOf('30.0 GB of 32.0 GB'), palette.danger, reason: 'over 90%');
      expect(
        colorOf('34% of 10 cores'),
        palette.textMuted,
        reason: 'alert off',
      );
      expect(colorOf('212 GB free of 994 GB'), palette.textMuted);
      expect(
        colorOf('8.0 GB free of 500 GB'),
        palette.danger,
        reason: 'under 10 GB',
      );
      double top(String text) => tester.getTopLeft(inPanel(text)).dy;
      expect(
        top('Memory is 94% used: 30.0 GB of 32.0 GB.'),
        lessThan(top('Machine')),
      );
    },
  );

  testWidgets(
    "the last hour, then Factory's processes and the biggest others",
    (tester) async {
      await show(tester, sample());
      await reveal(tester, 'Last hour');
      Finder line(String label) => find.byWidgetPredicate(
        (widget) => widget is Semantics && widget.properties.label == label,
      );
      expect(line('Memory over the last hour'), findsOneWidget);
      expect(line('Processors over the last hour'), findsOneWidget);
      expect(inPanel('30.0 GB'), findsOneWidget, reason: 'the latest memory');
      expect(inPanel('40%'), findsOneWidget, reason: 'the latest load');

      await reveal(tester, 'Docker');
      expect(inPanel('Claktory service'), findsOneWidget);
      expect(inPanel('180 MB'), findsOneWidget);
      expect(inPanel('task-3 npm run dev'), findsOneWidget);
      expect(
        inPanel('Builder · 4 processes'),
        findsOneWidget,
        reason: "a task says whose it is",
      );
      expect(inPanel('112%'), findsOneWidget);
      expect(inPanel('1.5 GB'), findsOneWidget);
      expect(inPanel('Other processes'), findsOneWidget);
      expect(inPanel('4.0 GB'), findsOneWidget);
    },
  );

  testWidgets(
    'the disk: an agent opens to its parts; checkouts say they share',
    (tester) async {
      await show(tester, sample());
      await reveal(tester, '~/factory');
      expect(inPanel('measured at 13:57'), findsOneWidget);
      expect(inPanel('42.0 GB'), findsOneWidget);
      expect(inPanel('19.0 GB'), findsOneWidget);
      expect(inPanel('at least 3.0 GB'), findsOneWidget, reason: 'cut short');
      expect(inPanel('Conversations'), findsNothing);

      await tester.tap(inPanel('Builder').last);
      await tester.pump();
      await reveal(tester, 'Checkout');
      expect(inPanel('Conversations'), findsOneWidget);
      expect(inPanel('120 MB'), findsOneWidget);
      expect(inPanel('18.0 GB'), findsOneWidget);
      expect(
        find.descendant(
          of: find.byType(ResourcesView),
          matching: find.textContaining('Shares most of its space'),
        ),
        findsOneWidget,
      );
    },
  );

  testWidgets('a build cache cleans now, and says why it could not', (
    tester,
  ) async {
    const path = '/Users/me/.factory/agents/a1/checkout/target';
    final harness = await show(tester, sample());
    await reveal(tester, '~/factory');
    expect(inPanel('Clean'), findsNothing, reason: 'only build caches');
    await tester.tap(inPanel('Builder').last);
    await tester.pump();
    await tester.ensureVisible(inPanel('Clean'));
    await tester.pump();
    expect(inPanel('9.0 GB'), findsOneWidget);
    await tester.tap(inPanel('Clean'));
    await tester.pump();
    expect(harness.last.clearBuildCache.path, path);
    expect(inPanel('Clean'), findsOneWidget, reason: 'done, ready again');

    harness.backend.refuse = 'An agent is working there.';
    await tester.tap(inPanel('Clean'));
    await tester.pump();
    expect(inPanel('An agent is working there.'), findsOneWidget);
    final palette = Palette.of(tester.element(find.byType(ResourcesView)));
    expect(
      tester.widget<Text>(inPanel('An agent is working there.')).style!.color,
      palette.danger,
    );

    harness.backend.refuse = null;
    await tester.tap(inPanel('Clean'));
    await tester.pump();
    expect(inPanel('An agent is working there.'), findsNothing);
  });

  testWidgets('before the disk is measured, it says so', (tester) async {
    await show(tester, sample(measured: false));
    await reveal(tester, 'Disk');
    expect(
      inPanel('Measuring… This takes a minute after Claktory starts.'),
      findsOneWidget,
    );
  });

  testWidgets('Settings sets the thresholds and shows a refusal', (
    tester,
  ) async {
    final harness = Harness(resourcesWorkspace());
    await harness.show(tester);
    harness.page.value = AppPage.settings;
    await settle(tester);
    await revealInSettings(
      tester,
      find.text('Processors'),
      section: 'resources',
    );
    expect(find.text('Resources'), findsWidgets);
    expect(find.text('Off'), findsOneWidget, reason: 'the processors alert');

    final fields = find.descendant(
      of: find.ancestor(of: find.text('% used'), matching: find.byType(Wrap)),
      matching: find.byType(EditableText),
    );
    await tester.enterText(fields, '80');
    await tester.pump();
    await tester.tap(find.text('Save'));
    await settle(tester);
    final update = harness.last.updateSettings;
    expect(update.ramPercent, 80);
    expect(update.hasCpuPercent(), isFalse);

    harness.backend.refuse = 'resources: cpu_percent must be from 0 to 100';
    final cpu = find.descendant(
      of: find.ancestor(of: find.text('% busy'), matching: find.byType(Wrap)),
      matching: find.byType(EditableText),
    );
    await tester.enterText(cpu, '120');
    await tester.pump();
    await tester.tap(find.text('Save').last);
    await settle(tester);
    expect(harness.last.updateSettings.cpuPercent, 120);
    expect(
      find.text('resources: cpu_percent must be from 0 to 100'),
      findsOneWidget,
    );

    await tester.enterText(cpu, 'lots');
    await tester.pump();
    await tester.tap(find.text('Save').last);
    await settle(tester);
    expect(find.text('Enter a whole number.'), findsOneWidget);
  });

  testWidgets('Settings sets how big build caches grow', (tester) async {
    final snapshot = resourcesWorkspace();
    snapshot.settings.buildCacheGb = 25;
    final harness = Harness(snapshot);
    await harness.show(tester);
    harness.page.value = AppPage.settings;
    await settle(tester);
    final note = find.text(
      "When one grows past this, Claktory clears it while nothing's using "
      'it. 0 means never.',
    );
    await revealInSettings(tester, note);
    expect(find.text('Build caches'), findsOneWidget);
    final field = find.descendant(
      of: find.ancestor(of: find.text('GB each'), matching: find.byType(Wrap)),
      matching: find.byType(EditableText),
    );
    expect(tester.widget<EditableText>(field).controller.text, '25');
    await tester.enterText(field, '40');
    await tester.pump();
    expect(note, findsNothing, reason: 'Save stands in its place');
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(harness.last.updateSettings.buildCacheGb, 40);
    expect(harness.last.updateSettings.hasRamPercent(), isFalse);
  });

  testWidgets('Settings says who hears the alerts and why one is not told, '
      'and changes who (friend bug #54)', (tester) async {
    final snapshot = resourcesWorkspace();
    snapshot.agents.addAll([
      Agent(id: 'rivet', name: 'Rivet', modelAvailable: true),
      Agent(id: 'mista', name: 'Mista', projectId: 'p1', modelAvailable: true),
    ]);
    snapshot.settings
      ..name = 'Mac Studio'
      ..resourceAlertsToAdmins = true
      ..resourceAlertAgents.addAll([
        ResourceAlertAgent(agentId: 'rivet', told: true, admin: true),
        ResourceAlertAgent(
          agentId: 'mista',
          held: ResourceAlertHeld.RESOURCE_ALERT_HELD_RESOURCES_OFF,
        ),
      ]);
    final harness = Harness(snapshot);
    await harness.show(tester, size: const Size(1200, 1600));
    harness.page.value = AppPage.settings;
    await settle(tester);
    final heard = find.text('Who hears');
    await revealInSettings(tester, heard);
    expect(
      find.text('Rivet tells you when Mac Studio runs low. Alert when:'),
      findsOneWidget,
    );
    expect(
      find.text("Resources is off for Mista, so it isn't told."),
      findsOneWidget,
    );
    final field = find.byType(AgentListField);
    expect(
      find.descendant(of: field, matching: find.text('Mista')),
      findsOneWidget,
      reason: 'the chosen agent, as a chip',
    );

    // The whole row in sight.
    await tester.ensureVisible(find.text('Memory'));
    await settle(tester);
    // Admin agents off.
    await tester.tap(find.bySemanticsLabel('Admin agents'));
    await settle(tester);
    expect(harness.last.updateSettings.resourceAlertsToAdmins, isFalse);
    expect(harness.last.updateSettings.hasResourceAlertAgents(), isFalse);

    // Builder chosen too: the whole list goes, in the sidebar's order.
    await tester.tap(find.descendant(of: field, matching: find.text('Mista')));
    await settle(tester);
    await tester.tap(find.text('Builder').last);
    await settle(tester);
    expect(harness.last.updateSettings.resourceAlertAgents.agentIds, [
      'a1',
      'mista',
    ]);

    // Nobody told: Claktory says so itself.
    final nobody = snapshot.deepCopy();
    nobody.settings
      ..resourceAlertsToAdmins = false
      ..resourceAlertAgents.removeAt(0);
    await harness.apply(tester, nobody);
    expect(
      find.text('Claktory notifies you when Mac Studio runs low. Alert when:'),
      findsOneWidget,
    );
    expect(
      find.text(
        'No agent can hear the alerts now, so Claktory notifies you itself.',
      ),
      findsOneWidget,
    );
  });
}
