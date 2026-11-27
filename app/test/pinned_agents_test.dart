// Pinned agents (Lucy's ux/pinned-agents.md, Bruno 46946): agents from any
// project the person keeps under Rivet in the sidebar. Pins sit under the
// workspace's agents, before Projects, each with its role and project; a
// right click (or a finger's hold let go) offers Pin, Unpin, Move up, Move
// down and Agent settings…; pins drag among themselves, and an agent
// dragged into the top group's gaps is pinned there. Folded rows leave the
// pins out while they're in sight; ⌘1 to ⌘9 open them on a Mac.
import 'package:factory_app/forms.dart' show ProjectSettingsForm;
import 'package:factory_app/pinned_agents.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent _agent(
  String id,
  String name, {
  String role = '',
  String project = 'p1',
  String lead = '',
  bool collapsed = false,
  int waiting = 0,
}) => Agent(
  id: id,
  name: name,
  role: role,
  projectId: project,
  leadId: lead,
  parentId: project.isEmpty || lead == 'rivet' ? '' : lead,
  collapsed: collapsed,
  modelAvailable: true,
  waitingSeqs: [for (var i = 0; i < waiting; i++) Int64(i + 1)],
);

Workspace _workspace({
  List<String> pins = const ['bruno', 'dana', 'oldie'],
  bool brunoFolded = false,
  List<Agent> extra = const [],
}) => Workspace(
  projects: [
    Project(id: 'p1', name: 'Claktory', path: '/w'),
    Project(id: 'p2', name: 'Insights', path: '/i'),
    Project(id: 'p3', name: 'Old', path: '/o', archived: true),
  ],
  agents: [
    _agent('rivet', 'Rivet', role: 'Admin', project: ''),
    _agent(
      'bruno',
      'Bruno',
      role: 'Project Manager',
      lead: 'rivet',
      collapsed: brunoFolded,
    ),
    _agent(
      'speed',
      'Speedwagon',
      role: 'Product Marketing',
      lead: 'bruno',
      waiting: 2,
    ),
    _agent('dana', 'Dana', role: 'Product Insights', project: 'p2'),
    _agent('oldie', 'Oldie', project: 'p3'),
    ...extra,
  ],
  settings: Settings(pinnedAgents: pins),
);

Future<Harness> _show(
  WidgetTester tester,
  Workspace snapshot, {
  Size size = const Size(1200, 900),
}) async {
  final harness = Harness(snapshot);
  await harness.show(tester, size: size);
  await settle(tester);
  return harness;
}

/// The pin's row and the agent's row in its project, top first.
Finder _rows(String name) => find.text(name);

Future<void> _rightClick(WidgetTester tester, Finder finder) async {
  await tester.tap(
    finder,
    buttons: kSecondaryMouseButton,
    kind: PointerDeviceKind.mouse,
  );
  await settle(tester);
}

void main() {
  testWidgets('pins sit under Rivet, before Projects, with their role and '
      'project; an archived project has none', (tester) async {
    await _show(tester, _workspace());
    // Bruno twice: pinned, and in Claktory.
    expect(_rows('Bruno'), findsNWidgets(2));
    expect(_rows('Dana'), findsNWidgets(2));
    expect(find.text('Oldie'), findsNothing);
    final rivet = tester.getTopLeft(find.text('Rivet')).dy;
    final pin = tester.getTopLeft(_rows('Bruno').first).dy;
    final dana = tester.getTopLeft(_rows('Dana').first).dy;
    final projects = tester.getTopLeft(find.text('Projects')).dy;
    expect(rivet, lessThan(pin));
    expect(pin, lessThan(dana));
    expect(dana, lessThan(projects));
    // Its second line: the role, then the project.
    expect(find.text(' · Claktory'), findsOneWidget);
    expect(find.text(' · Insights'), findsOneWidget);
    // In line with Rivet.
    expect(
      tester.getTopLeft(_rows('Bruno').first).dx,
      tester.getTopLeft(find.text('Rivet')).dx,
    );
  });

  testWidgets('nothing pinned: nothing shows', (tester) async {
    await _show(tester, _workspace(pins: []));
    expect(_rows('Bruno'), findsOneWidget);
    expect(find.text(' · Claktory'), findsNothing);
  });

  testWidgets('a right click on an agent in a project: Pin, then Agent '
      'settings…; Pin pins it last', (tester) async {
    final harness = await _show(tester, _workspace());
    await _rightClick(tester, find.text('Speedwagon'));
    expect(find.text('Pin'), findsOneWidget);
    expect(find.text('Agent settings…'), findsOneWidget);
    expect(find.text('Move up'), findsNothing);
    await tester.tap(find.text('Pin'));
    await settle(tester);
    final pin = harness.last.pinAgent;
    expect((pin.agentId, pin.beforeId), ('speed', ''));
  });

  // Abbacchio's row 1661: the row focused, Shift-F10 or the menu key
  // opens its menu, where VoiceOver on a Mac reaches it.
  testWidgets('a focused row opens its menu from Shift-F10 and the menu '
      'key', (tester) async {
    final harness = await _show(tester, _workspace());
    Focus.of(tester.element(find.text('Speedwagon'))).requestFocus();
    await settle(tester);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.f10);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
    await settle(tester);
    expect(find.text('Pin'), findsOneWidget);
    // Its items take the keys: Return chooses the first.
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(harness.last.pinAgent.agentId, 'speed');
    Focus.of(tester.element(find.text('Speedwagon').last)).requestFocus();
    await settle(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.contextMenu);
    await settle(tester);
    expect(find.text('Agent settings…'), findsOneWidget);
  });

  testWidgets("a pin's menu: Unpin, Move up and Move down where they go", (
    tester,
  ) async {
    final harness = await _show(tester, _workspace());
    // The first pin: no Move up.
    await _rightClick(tester, _rows('Bruno').first);
    expect(find.text('Unpin'), findsOneWidget);
    expect(find.text('Move up'), findsNothing);
    expect(find.text('Move down'), findsOneWidget);
    await tester.tap(find.text('Move down'));
    await settle(tester);
    // After Dana, the last: before none.
    expect(
      (harness.last.pinAgent.agentId, harness.last.pinAgent.beforeId),
      ('bruno', ''),
    );

    // The last: Move up goes before the one above.
    await _rightClick(tester, _rows('Dana').first);
    expect(find.text('Move down'), findsNothing);
    await tester.tap(find.text('Move up'));
    await settle(tester);
    expect(
      (harness.last.pinAgent.agentId, harness.last.pinAgent.beforeId),
      ('dana', 'bruno'),
    );

    // Unpin from the agent's row in its project too.
    await _rightClick(tester, _rows('Dana').last);
    await tester.tap(find.text('Unpin'));
    await settle(tester);
    expect(harness.last.unpinAgent.agentId, 'dana');
  });

  testWidgets("Rivet's menu has no Pin: it's at the top already", (
    tester,
  ) async {
    await _show(tester, _workspace());
    await _rightClick(tester, find.text('Rivet'));
    expect(find.text('Pin'), findsNothing);
    expect(find.text('Agent settings…'), findsOneWidget);
  });

  testWidgets('a refused pin says why', (tester) async {
    final harness = await _show(tester, _workspace());
    harness.backend.refuse = "Dana's project is archived.";
    await _rightClick(tester, find.text('Speedwagon'));
    await tester.tap(find.text('Pin'));
    await settle(tester);
    expect(find.text("Couldn't pin Speedwagon"), findsOneWidget);
    expect(find.text("Dana's project is archived."), findsOneWidget);
  });

  // Abbacchio's NIT, Trish's words: the backend's generic "no agent with id"
  // reads as the agent gone.
  testWidgets('a pin refused as the agent is gone says so in words', (
    tester,
  ) async {
    final harness = await _show(tester, _workspace());
    harness.backend.refuse = 'no agent with id "speed"';
    await _rightClick(tester, find.text('Speedwagon'));
    await tester.tap(find.text('Pin'));
    await settle(tester);
    expect(find.text("Couldn't pin Speedwagon"), findsOneWidget);
    expect(
      find.text("Speedwagon isn't in this Claktory any more."),
      findsOneWidget,
    );
  });

  testWidgets('drag an agent into the gap under Rivet: pinned there, and the '
      'drag says so first', (tester) async {
    final harness = await _show(tester, _workspace());
    final from = tester.getCenter(find.text('Speedwagon'));
    // The lower edge of Rivet's row: between Rivet and the first pin.
    final rivet = tester.getRect(
      find.ancestor(of: find.text('Rivet'), matching: find.byType(Row)).first,
    );
    final to = rivet.bottomCenter - const Offset(0, 3);
    final mouse = await tester.startGesture(
      from,
      kind: PointerDeviceKind.mouse,
    );
    for (var i = 1; i <= 10; i++) {
      await mouse.moveTo(Offset.lerp(from, to, i / 10)!);
      await tester.pump(const Duration(milliseconds: 16));
    }
    expect(find.text('Pins Speedwagon here'), findsOneWidget);
    await mouse.up();
    await settle(tester);
    final pin = harness.last.pinAgent;
    expect((pin.agentId, pin.beforeId), ('speed', 'bruno'));
    expect(harness.backend.sent.where((m) => m.hasArrangeAgent()), isEmpty);
  });

  testWidgets('pins reorder among themselves; dropped in a project, a pin '
      'goes back with nothing changed', (tester) async {
    final harness = await _show(tester, _workspace());
    Future<void> drag(Offset to) async {
      final from = tester.getCenter(_rows('Dana').first);
      final mouse = await tester.startGesture(
        from,
        kind: PointerDeviceKind.mouse,
      );
      for (var i = 1; i <= 10; i++) {
        await mouse.moveTo(Offset.lerp(from, to, i / 10)!);
        await tester.pump(const Duration(milliseconds: 16));
      }
      await mouse.up();
      await settle(tester);
    }

    // Onto Speedwagon's row in Claktory: nothing.
    await drag(tester.getCenter(find.text('Speedwagon')));
    expect(harness.backend.sent, isEmpty);

    // Above Bruno's pin: before it.
    final bruno = tester.getRect(
      find.ancestor(of: _rows('Bruno').first, matching: find.byType(Row)).first,
    );
    await drag(bruno.topCenter + const Offset(0, 3));
    expect(
      (harness.last.pinAgent.agentId, harness.last.pinAgent.beforeId),
      ('dana', 'bruno'),
    );
  });

  testWidgets("a folded lead leaves out its pinned report's count while the "
      'pins are in sight', (tester) async {
    // Speedwagon, folded under Bruno, waits on 2; pinned, it shows its own.
    await _show(tester, _workspace(pins: ['speed'], brunoFolded: true));
    // Needs you's and the pin's: Bruno's folded row says nothing more.
    final projects = tester.getTopLeft(find.text('Projects')).dy;
    expect(find.text('2'), findsNWidgets(2));
    for (final count in find.text('2').evaluate()) {
      expect(
        tester.getTopLeft(find.byWidget(count.widget)).dy,
        lessThan(projects),
      );
    }
  });

  testWidgets('unpinned, the folded lead speaks for it again', (tester) async {
    await _show(tester, _workspace(pins: [], brunoFolded: true));
    final projects = tester.getTopLeft(find.text('Projects')).dy;
    expect(tester.getTopLeft(find.text('2').last).dy, greaterThan(projects));
    expect(find.text('Speedwagon'), findsNothing);
  });

  testWidgets('pins taller than a third of the sidebar scroll with the '
      'projects, and folded rows count them again', (tester) async {
    final many = [
      for (var i = 0; i < 12; i++)
        _agent('a$i', 'Agent $i', role: 'Helper', lead: 'bruno'),
    ];
    await _show(
      tester,
      _workspace(
        pins: [for (final agent in many) agent.id, 'speed'],
        brunoFolded: true,
        extra: many,
      ),
      size: const Size(1200, 700),
    );
    expect(
      find.ancestor(
        of: find.text('Agent 0'),
        matching: find.byType(Scrollable),
      ),
      findsWidgets,
    );
    // Bruno's folded row speaks for Speedwagon again: the pin can scroll
    // away.
    await tester.scrollUntilVisible(
      find.text('Projects'),
      200,
      scrollable: find
          .ancestor(of: find.text('Agent 0'), matching: find.byType(Scrollable))
          .first,
    );
    final projects = tester.getTopLeft(find.text('Projects')).dy;
    expect(
      find
          .text('2')
          .evaluate()
          .where(
            (count) =>
                tester.getTopLeft(find.byWidget(count.widget)).dy > projects,
          ),
      isNotEmpty,
    );
  });

  testWidgets('VoiceOver: the pin says it is pinned, with Unpin, Move up and '
      'Move down', (tester) async {
    final semantics = tester.ensureSemantics();
    await _show(tester, _workspace());
    expect(
      find.bySemanticsLabel(
        RegExp(r'Bruno, Project Manager, in Claktory, pinned'),
      ),
      findsOneWidget,
    );
    // The actions on the node around Dana's pin.
    List<String> actionsAround(SemanticsNode node, String label) {
      var holds = node.label.contains(label);
      final found = <String>[];
      node.visitChildren((child) {
        final inner = actionsAround(child, label);
        if (inner.isNotEmpty || _holds(child, label)) holds = true;
        found.addAll(inner);
        return true;
      });
      if (holds && found.isEmpty) {
        for (final id
            in node.getSemanticsData().customSemanticsActionIds ?? <int>[]) {
          found.add(CustomSemanticsAction.getAction(id)!.label!);
        }
      }
      return found;
    }

    final actions = actionsAround(
      tester
          .binding
          .renderViews
          .first
          .owner!
          .semanticsOwner!
          .rootSemanticsNode!,
      'Dana, Product Insights, in Insights, pinned',
    );
    expect(actions, containsAll(['Unpin', 'Move up']));
    expect(actions, isNot(contains('Move down')));
    semantics.dispose();
  });

  testWidgets('by touch: hold and let go, the sheet titled with the agent', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = await _show(tester, _workspace());
    final finger = await tester.startGesture(
      tester.getCenter(find.text('Speedwagon')),
      kind: PointerDeviceKind.touch,
    );
    await tester.pump(const Duration(milliseconds: 700));
    await finger.up();
    await settle(tester);
    expect(
      find.text('Speedwagon · Product Marketing · Claktory'),
      findsOneWidget,
    );
    await tester.tap(find.text('Pin'));
    await settle(tester);
    expect(harness.last.pinAgent.agentId, 'speed');
  });

  testWidgets("a phone's drawer at the largest text: a pin's sheet caption "
      'takes two lines at most, measured in the type the sheet draws it in '
      "(Zucchero's row 716)", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 2.35;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    // "Ida · Lead · Insights" takes two lines in the drawer's 13pt and
    // three in the sheet's 15pt: the role goes.
    await _show(
      tester,
      _workspace(
        pins: ['bruno', 'ida', 'dana'],
        extra: [_agent('ida', 'Ida', role: 'Lead', project: 'p2')],
      ),
      size: const Size(375, 667),
    );
    for (final name in ['Bruno', 'Ida']) {
      final pin = find.text(name).first;
      await tester.ensureVisible(pin);
      await pumpFrames(tester);
      final finger = await tester.startGesture(
        tester.getCenter(pin),
        kind: PointerDeviceKind.touch,
      );
      await tester.pump(const Duration(milliseconds: 700));
      await finger.up();
      await pumpFrames(tester);
      final caption = find.byWidgetPredicate(
        (widget) =>
            widget is Text &&
            widget.style?.fontWeight == FontWeight.w600 &&
            (widget.data == name ||
                (widget.data?.startsWith('$name · ') ?? false)),
      );
      expect(caption, findsOneWidget, reason: name);
      final style = tester.widget<Text>(caption).style!;
      final line = style.fontSize! * 2.35 * style.height!;
      expect(
        tester.getSize(caption).height,
        lessThanOrEqualTo(line * 2 + 0.5),
        reason: tester.widget<Text>(caption).data,
      );
      if (name == 'Ida') {
        expect(tester.widget<Text>(caption).data, 'Ida · Insights');
      }
      await tester.tap(find.text('Cancel'));
      await pumpFrames(tester);
    }
  });

  testWidgets('⌘1 and ⌘2 open the pins on a Mac', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = await _show(tester, _workspace());
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.digit2);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    await settle(tester);
    expect(harness.last.selectAgent.agentId, 'dana');
    // Past the pins: nothing.
    harness.backend.sent.clear();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.digit5);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    await settle(tester);
    expect(harness.backend.sent, isEmpty);
  });

  testWidgets("archiving a project says whose pins go", (tester) async {
    final harness = Harness(_workspace(pins: ['bruno', 'speed', 'dana']));
    await harness.show(
      tester,
      home: ProjectSettingsForm(
        project: Project(id: 'p1', name: 'Claktory', path: '/w'),
        close: () {},
      ),
    );
    await settle(tester);
    expect(
      find.textContaining('Bruno and Speedwagon are unpinned.'),
      findsOneWidget,
    );
  });

  test('the archive note names up to three, then how many more', () {
    Workspace pinned(int count) => Workspace(
      projects: [Project(id: 'p1', name: 'Claktory')],
      agents: [
        for (var i = 0; i < count; i++)
          Agent(id: 'a$i', name: 'A$i', projectId: 'p1'),
      ],
      settings: Settings(pinnedAgents: [for (var i = 0; i < count; i++) 'a$i']),
    );
    expect(unpinnedOnArchive(pinned(0), 'p1'), '');
    expect(unpinnedOnArchive(pinned(1), 'p1'), 'A0 is unpinned.');
    expect(unpinnedOnArchive(pinned(3), 'p1'), 'A0, A1 and A2 are unpinned.');
    expect(
      unpinnedOnArchive(pinned(5), 'p1'),
      'A0, A1, A2 and 2 more are unpinned.',
    );
  });
}

bool _holds(SemanticsNode node, String label) {
  if (node.label.contains(label)) return true;
  var found = false;
  node.visitChildren((child) {
    if (_holds(child, label)) found = true;
    return !found;
  });
  return found;
}
