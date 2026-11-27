import 'package:factory_app/controls.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart' show sidebarHoldToDrag;
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent agent(
  String id,
  String name, {
  String parent = '',
  bool collapsed = false,
  bool working = false,
  bool unseen = false,
}) => Agent(
  id: id,
  name: name,
  projectId: 'p1',
  parentId: parent,
  collapsed: collapsed,
  unseen: unseen,
  modelAvailable: true,
  status: working
      ? AgentStatus.AGENT_STATUS_WORKING
      : AgentStatus.AGENT_STATUS_IDLE,
);

Workspace workspace({
  bool collapsed = false,
  bool backWorking = false,
  bool frontUnseen = false,
}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  agents: [
    agent('lead', 'Lead', collapsed: collapsed),
    agent('back', 'Back', parent: 'lead', working: backWorking),
    agent('front', 'Front', parent: 'lead', unseen: frontUnseen),
    agent('solo', 'Solo'),
  ],
);

Finder labeled(String label) => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == label,
);

void main() {
  test('the working wave goes there and back, never out of view', () {
    const width = 248.0;
    final band = waveBand(width);
    expect(waveCenter(0, width), 0);
    expect(waveCenter(0.25, width), closeTo(width / 2, 0.01));
    expect(waveCenter(0.5, width), width);
    expect(waveCenter(0.75, width), closeTo(width / 2, 0.01));
    var last = waveCenter(0, width);
    for (var i = 1; i <= 1000; i++) {
      final center = waveCenter(i / 1000, width);
      // At least half the band is always on the row…
      expect(center - band / 2, lessThanOrEqualTo(width - band / 2));
      expect(center + band / 2, greaterThanOrEqualTo(band / 2));
      // …and it moves smoothly, with no jump back to the start.
      expect((center - last).abs(), lessThan(width / 100));
      last = center;
    }
  });

  Future<Harness> show(WidgetTester tester, Workspace snapshot) async {
    final harness = Harness(snapshot);
    await harness.show(tester);
    await settle(tester);
    return harness;
  }

  /// Drags [name]'s row to [target], in small steps as a hand would.
  Future<void> drag(
    WidgetTester tester,
    String name,
    Offset target, {
    PointerDeviceKind kind = PointerDeviceKind.mouse,
    Duration hold = const Duration(milliseconds: 50),
  }) async {
    final from = tester.getCenter(find.text(name));
    final gesture = await tester.startGesture(from, kind: kind);
    await tester.pump(hold);
    for (var i = 1; i <= 10; i++) {
      await gesture.moveTo(Offset.lerp(from, target, i / 10)!);
      await tester.pump(const Duration(milliseconds: 16));
    }
    await gesture.up();
    await settle(tester);
  }

  Rect row(WidgetTester tester, String name) => tester.getRect(
    find.ancestor(of: find.text(name), matching: find.byType(Row)).first,
  );

  testWidgets('agents show in order, nested ones indented', (tester) async {
    await show(tester, workspace());
    final names = ['Lead', 'Back', 'Front', 'Solo'];
    for (var i = 1; i < names.length; i++) {
      expect(
        tester.getTopLeft(find.text(names[i - 1])).dy,
        lessThan(tester.getTopLeft(find.text(names[i])).dy),
      );
    }
    final lead = tester.getTopLeft(find.text('Lead')).dx;
    expect(tester.getTopLeft(find.text('Back')).dx, greaterThan(lead));
    expect(tester.getTopLeft(find.text('Solo')).dx, lead);
  });

  testWidgets('a parent collapses, and speaks for what it hides', (
    tester,
  ) async {
    final harness = await show(tester, workspace());
    await tester.tap(labeled("Hide Lead's reports"));
    await settle(tester);
    expect(harness.last.setAgentCollapsed.agentId, 'lead');
    expect(harness.last.setAgentCollapsed.collapsed, isTrue);

    await harness.apply(
      tester,
      workspace(collapsed: true, backWorking: true, frontUnseen: true),
    );
    await settle(tester);
    expect(find.text('Back'), findsNothing);
    expect(find.text('Front'), findsNothing);
    expect(labeled("Show Lead's reports"), findsOneWidget);
    // Back works and Front has news, out of sight: Lead says so.
    expect(find.byType(WorkingWave), findsOneWidget, reason: "on Lead's row");
    expect(labeled('Lead has something new'), findsOneWidget);
  });

  testWidgets('dropping onto an agent nests it there', (tester) async {
    final harness = await show(tester, workspace());
    await drag(tester, 'Front', row(tester, 'Solo').center);
    final arrange = harness.last.arrangeAgent;
    expect(arrange.agentId, 'front');
    expect(arrange.parentId, 'solo');
    expect(arrange.beforeId, '');
    expect(arrange.hasProjectId(), isFalse, reason: 'same project');
  });

  testWidgets('dropping between rows places it there', (tester) async {
    final harness = await show(tester, workspace());
    // On Lead's upper edge: before Lead, at the top level.
    await drag(
      tester,
      'Solo',
      row(tester, 'Lead').topCenter + const Offset(0, 3),
    );
    expect(harness.last.arrangeAgent.parentId, '');
    expect(harness.last.arrangeAgent.beforeId, 'lead');

    // On Back's lower edge: after Back, among Lead's agents.
    await drag(
      tester,
      'Solo',
      row(tester, 'Back').bottomCenter - const Offset(0, 3),
    );
    expect(harness.last.arrangeAgent.parentId, 'lead');
    expect(harness.last.arrangeAgent.beforeId, 'front');

    // On the project itself: out to the top level, last.
    await drag(tester, 'Back', tester.getCenter(find.text('Factory').last));
    expect(harness.last.arrangeAgent.agentId, 'back');
    expect(harness.last.arrangeAgent.parentId, '');
    expect(harness.last.arrangeAgent.beforeId, '');
  });

  testWidgets('an agent is never dropped into what is nested under it', (
    tester,
  ) async {
    final harness = await show(tester, workspace());
    await drag(tester, 'Lead', row(tester, 'Back').center);
    await drag(
      tester,
      'Lead',
      row(tester, 'Front').bottomCenter - const Offset(0, 3),
    );
    expect(harness.backend.sent, isEmpty);
  });

  testWidgets('clicking a nested, unseen agent opens it and keeps it nested', (
    tester,
  ) async {
    Workspace snapshot({required bool unseen, String selected = 'front'}) {
      final base = workspace(frontUnseen: false);
      return base
        ..selectedAgentId = selected
        ..agents[1].unseen = unseen;
    }

    final harness = await show(tester, snapshot(unseen: true));
    final indent = tester.getTopLeft(find.text('Back')).dx;
    expect(indent, greaterThan(tester.getTopLeft(find.text('Lead')).dx));

    // A click with the slight jiggle a real hand gives it.
    final mouse = await tester.startGesture(
      tester.getCenter(find.text('Back')),
      kind: PointerDeviceKind.mouse,
    );
    await mouse.moveBy(const Offset(2, 1));
    await tester.pump();
    await mouse.up();
    await settle(tester);
    expect(harness.backend.sent, hasLength(1));
    expect(harness.last.selectAgent.agentId, 'back');

    await harness.apply(tester, snapshot(unseen: false, selected: 'back'));
    await settle(tester);
    // The sidebar's row, not the header that now shows it too.
    expect(tester.getTopLeft(find.text('Back').first).dx, indent);
  });

  testWidgets('a finger that moves at once scrolls; one that holds first '
      'moves the agent', (tester) async {
    final harness = await show(tester, workspace());
    final target = row(tester, 'Back').bottomCenter - const Offset(0, 3);
    await drag(tester, 'Solo', target, kind: PointerDeviceKind.touch);
    expect(harness.backend.sent, isEmpty);

    await drag(
      tester,
      'Solo',
      target,
      kind: PointerDeviceKind.touch,
      hold: sidebarHoldToDrag + const Duration(milliseconds: 50),
    );
    expect(harness.last.arrangeAgent.agentId, 'solo');
    expect(harness.last.arrangeAgent.parentId, 'lead');
  });

  testWidgets('a drag let go over its own row changes nothing', (tester) async {
    final harness = await show(tester, workspace());
    final back = tester.getCenter(find.text('Back'));
    final mouse = await tester.startGesture(
      back,
      kind: PointerDeviceKind.mouse,
    );
    for (final step in [const Offset(30, 0), const Offset(-30, 2)]) {
      await mouse.moveBy(step);
      await tester.pump(const Duration(milliseconds: 16));
    }
    await mouse.up();
    await settle(tester);
    expect(harness.backend.sent, isEmpty);
  });

  group('folders (Lucy projects-sidebar.md §3)', () {
    Workspace withFolders() {
      final snapshot = workspace();
      snapshot.projects[0].folders.addAll([
        ProjectFolder(
          id: 'research',
          name: 'Research',
          path: '/Users/me/notes',
        ),
        ProjectFolder(id: 'site', name: 'Site', path: '/Users/me/site'),
      ]);
      // Back works in Research, under Lead in the project's own folder;
      // Scout works in Research at the top level, Pilot in Research under
      // Scout, like its lead, and Deck in the project's own under Scout.
      // (The tests' font is wide: a short role.)
      snapshot.agents[1].folderId = 'research';
      snapshot.agents.add(
        agent('scout', 'Scout')
          ..folderId = 'research'
          ..role = 'Lab',
      );
      snapshot.agents.add(
        agent('pilot', 'Pilot', parent: 'scout')..folderId = 'research',
      );
      snapshot.agents.add(agent('deck', 'Deck', parent: 'scout'));
      return snapshot;
    }

    testWidgets('one tree by lead, whatever folder: a folder is no '
        "heading, it's a tag on the row where it differs from the lead's", (
      tester,
    ) async {
      await show(tester, withFolders());
      double top(String text) => tester.getTopLeft(find.text(text)).dy;
      // In config order, each under its lead: no folder splits the team.
      expect(top('Lead'), lessThan(top('Back')));
      expect(top('Back'), lessThan(top('Front')));
      expect(top('Solo'), lessThan(top('Scout')));
      expect(top('Scout'), lessThan(top('Pilot')));
      expect(
        tester.getTopLeft(find.text('Pilot')).dx,
        greaterThan(tester.getTopLeft(find.text('Scout')).dx),
      );
      // Back (under Lead, in the project's folder) and Scout (top level)
      // name Research; Pilot, in Research like Scout, names nothing; Deck,
      // in the project's own under Scout, names the project's.
      expect(find.text('Research'), findsNWidgets(2));
      expect(find.text('Site'), findsNothing, reason: 'no heading');
      expect(find.text('Lab · '), findsOneWidget);
      expect(find.text('Factory'), findsNWidgets(2), reason: 'row and tag');
      expect(labeled('works in Research'), findsNWidgets(2));
      expect(labeled('works in Factory'), findsOneWidget);
      // Nothing to fold a folder with.
      expect(find.bySemanticsLabel(RegExp('agents in Research')), findsNothing);
    });

    testWidgets('the folder is cut before the role, the role before the '
        'name', (tester) async {
      final long = withFolders();
      long.agents[1].role = 'A role long enough to fill the row on its own';
      await show(tester, long);
      expect(
        labeled(
          'A role long enough to fill the row on its own, works in Research',
        ),
        findsOneWidget,
        reason: 'the folder said, not shown, where the role takes the line',
      );
      expect(find.text('Back'), findsOneWidget);
      expect(labeled('works in Research'), findsOneWidget, reason: 'Scout');
      expect(tester.takeException(), isNull);
    });

    testWidgets('a drop never changes where an agent works (§5.1)', (
      tester,
    ) async {
      final harness = await show(tester, withFolders());
      // Under Scout, by its row's middle.
      await drag(tester, 'Solo', tester.getCenter(find.text('Scout')));
      expect(harness.last.arrangeAgent.agentId, 'solo');
      expect(harness.last.arrangeAgent.parentId, 'scout');
      expect(harness.last.arrangeAgent.hasFolderId(), isFalse);

      // Between Scout's reports.
      await drag(
        tester,
        'Solo',
        row(tester, 'Deck').topCenter + const Offset(0, 3),
      );
      expect(harness.last.arrangeAgent.beforeId, 'deck');
      expect(harness.last.arrangeAgent.hasFolderId(), isFalse);

      // On the project itself: from under its lead, its folder kept.
      await drag(tester, 'Pilot', tester.getCenter(find.text('Factory').first));
      expect(harness.last.arrangeAgent.agentId, 'pilot');
      expect(harness.last.arrangeAgent.hasFolderId(), isFalse);
    });
  });

  testWidgets('a working agent has a wave of its color across its row', (
    tester,
  ) async {
    final harness = await show(tester, workspace(backWorking: true));
    final wave = tester.widget<WorkingWave>(find.byType(WorkingWave));
    expect(wave.color, agentColor(''));
    expect(
      tester.getSize(find.byType(WorkingWave)).width,
      greaterThan(tester.getSize(find.text('Back')).width),
      reason: 'across the whole row',
    );
    // It moves: a frame later it's painted elsewhere.
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.byType(CustomPaint), findsWidgets);

    // Done: no wave.
    await harness.apply(tester, workspace());
    expect(find.byType(WorkingWave), findsNothing);
  });

  testWidgets('with reduced motion the wave is a still tint', (tester) async {
    tester.platformDispatcher.accessibilityFeaturesTestValue =
        const FakeAccessibilityFeatures(disableAnimations: true);
    addTearDown(tester.platformDispatcher.clearAccessibilityFeaturesTestValue);
    await show(tester, workspace(backWorking: true));
    expect(
      find.descendant(
        of: find.byType(WorkingWave),
        matching: find.byType(ColoredBox),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: find.byType(WorkingWave),
        matching: find.byType(CustomPaint),
      ),
      findsNothing,
    );
  });
}
