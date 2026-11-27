// Pinned agents in the real phone drawer and conversation menu (iPhone and
// Android). Product widgets, shared PhoneBack and recorded backend requests;
// these tests are not OS gesture, simulator or physical-device acceptance.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/forms.dart' show AgentSettingsForm;
import 'package:factory_app/identity.dart' show Palette;
import 'package:factory_app/mobile/back.dart';
import 'package:factory_app/mobile/drawer.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/pinned_agents.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'android/support/android_harness.dart';
import 'harness.dart';

const _small = Size(375, 667);

Agent _agent(
  String id,
  String name,
  String project,
  String role, {
  String lead = '',
  bool collapsed = false,
  bool unseen = false,
  int waiting = 0,
}) => Agent(
  id: id,
  name: name,
  projectId: project,
  role: role,
  leadId: lead,
  parentId: lead == 'bruno' ? lead : '',
  collapsed: collapsed,
  unseen: unseen,
  modelAvailable: true,
  waitingSeqs: [for (var i = 1; i <= waiting; i++) Int64(i)],
  extensions: [
    if (id == 'rivet')
      AgentExtension(
        id: 'agents',
        enabled: true,
        options: {'admin': OptionValue(on: true)}.entries,
      ),
  ],
);

Workspace _workspace({
  List<String> pins = const ['bruno', 'dana', 'speed'],
  String selected = 'bruno',
  bool folded = false,
  bool projectsFolded = false,
  bool focused = false,
  int waiting = 0,
}) => Workspace(
  settings: Settings(pinnedAgents: pins, userName: 'Matt'),
  projects: [
    Project(id: 'p1', name: 'Claktory'),
    Project(id: 'p2', name: 'Insights'),
    Project(id: 'old', name: 'Archived', archived: true),
  ],
  agents: [
    _agent('rivet', 'Rivet', '', 'Admin'),
    _agent(
      'bruno',
      'Bruno',
      'p1',
      'Project Manager',
      lead: 'rivet',
      collapsed: folded,
    ),
    _agent(
      'speed',
      'Speedwagon',
      'p1',
      'Product Marketing',
      lead: 'bruno',
      waiting: waiting,
    ),
    _agent('dana', 'Dana', 'p2', 'Product Insights'),
    _agent('old-agent', 'Old agent', 'old', 'Helper'),
  ],
  selectedAgentId: selected,
  projectsCollapsed: projectsFolded,
  focusedProjectId: focused ? 'p2' : '',
);

Future<Harness> _show(
  WidgetTester tester,
  Workspace snapshot, {
  double scale = 1,
}) async {
  tester.platformDispatcher.textScaleFactorTestValue = scale;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  final harness = Harness(snapshot);
  if (defaultTargetPlatform == TargetPlatform.android) {
    await showAndroid(tester, harness: harness, size: _small);
  } else {
    await harness.show(tester, size: _small);
    await pumpFrames(tester);
  }
  expect(find.byType(AgentView), findsOneWidget);
  return harness;
}

Future<void> _openDrawer(WidgetTester tester) async {
  await tester.tap(find.bySemanticsLabel(RegExp(r'^Menu$')));
  await pumpFrames(tester);
  expect(find.byType(Sidebar), findsOneWidget);
}

Finder _inDrawer(String name) =>
    find.descendant(of: find.byType(Sidebar), matching: find.text(name));
Finder _pin(String name) =>
    find.bySemanticsLabel(RegExp('${RegExp.escape(name)}, [^\\n]*pinned'));
Finder _row(Finder contents) {
  final draggable = find.byWidgetPredicate((widget) => widget is Draggable);
  // The pin's merged semantics belongs to its outer custom-actions node;
  // a Text finder belongs inside the row. Reach the same draggable either way.
  final inside = find.descendant(of: contents, matching: draggable);
  return inside.evaluate().isNotEmpty
      ? inside.first
      : find.ancestor(of: contents, matching: draggable).first;
}

PhoneDrawerHostState _host(WidgetTester tester) =>
    tester.state(find.byType(PhoneDrawerHost));

Future<void> _holdMenu(WidgetTester tester, Finder row) async {
  await tester.ensureVisible(row);
  final finger = await tester.startGesture(tester.getCenter(row));
  await tester.pump(kLongPressTimeout + const Duration(milliseconds: 100));
  expect(sidebarDragging.value, isTrue);
  await finger.up();
  await pumpFrames(tester);
  expect(sidebarDragging.value, isFalse);
}

Future<void> _more(WidgetTester tester) async {
  await tester.tap(find.bySemanticsLabel(RegExp('^More options for ')).last);
  await pumpFrames(tester);
}

List<String> _actions(SemanticsNode node, String label) {
  final results = <String>[];
  bool includes(SemanticsNode current) {
    if (current.label.contains(label)) return true;
    var found = false;
    current.visitChildren((child) {
      found = includes(child) || found;
      return true;
    });
    return found;
  }

  if (includes(node)) {
    for (final id
        in node.getSemanticsData().customSemanticsActionIds ?? <int>[]) {
      results.add(CustomSemanticsAction.getAction(id)!.label!);
    }
    node.visitChildren((child) {
      results.addAll(_actions(child, label));
      return true;
    });
  }
  return results;
}

void main() {
  var previousAndroidPolicy = false;
  setUp(() {
    previousAndroidPolicy = PhonePlatformPolicy.debugAndroidPolicy;
    PhonePlatformPolicy.debugAndroidPolicy = true;
  });
  tearDown(() {
    sidebarDragging.value = false;
    justPinned.value = null;
    PhonePlatformPolicy.debugAndroidPolicy = previousAndroidPolicy;
  });

  for (final platform in [TargetPlatform.iOS, TargetPlatform.android]) {
    final variant = TargetPlatformVariant.only(platform);
    final name = platform.name;

    testWidgets(
      '$name pins follow Rivet and backend order, without a Pinned heading',
      (tester) async {
        final h = await _show(tester, _workspace());
        await _openDrawer(tester);
        final rivet = tester.getTopLeft(_inDrawer('Rivet')).dy;
        final bruno = tester.getTopLeft(_pin('Bruno')).dy;
        final dana = tester.getTopLeft(_pin('Dana')).dy;
        final speed = tester.getTopLeft(_pin('Speedwagon')).dy;
        expect(rivet, lessThan(bruno));
        expect(bruno, lessThan(dana));
        expect(dana, lessThan(speed));
        expect(speed, lessThan(tester.getTopLeft(_inDrawer('Projects')).dy));
        expect(find.text('Pinned'), findsNothing);
        expect(_inDrawer('Old agent'), findsNothing);
        await h.apply(tester, _workspace(pins: ['dana', 'bruno']));
        await pumpFrames(tester);
        expect(
          tester.getTopLeft(_pin('Dana')).dy,
          lessThan(tester.getTopLeft(_pin('Bruno')).dy),
        );
        expect(
          h.backend.sent.where((m) => m.hasPinAgent() || m.hasUnpinAgent()),
          isEmpty,
        );
      },
      variant: variant,
    );

    testWidgets(
      '$name project row hold offers Pin without changing its project or lead',
      (tester) async {
        final h = await _show(tester, _workspace(pins: []));
        await _openDrawer(tester);
        await _holdMenu(tester, _inDrawer('Speedwagon'));
        expect(find.text('Pin'), findsOneWidget);
        expect(find.text('Unpin'), findsNothing);
        expect(find.text('Move up'), findsNothing);
        expect(find.text('Move down'), findsNothing);
        expect(find.text('Agent settings…'), findsOneWidget);
        await tester.tap(find.text('Pin'));
        await pumpFrames(tester);
        expect(
          (h.last.pinAgent.agentId, h.last.pinAgent.beforeId),
          ('speed', ''),
        );
        expect(h.backend.sent.where((m) => m.hasArrangeAgent()), isEmpty);
        expect(h.backend.workspace.agent('speed')!.projectId, 'p1');
        expect(h.backend.workspace.agent('speed')!.leadId, 'bruno');
        expect(_host(tester).drawerOpen, isTrue);
      },
      variant: variant,
    );

    testWidgets(
      '$name selected agent has the same selection in pin and project rows',
      (tester) async {
        await _show(tester, _workspace());
        await _openDrawer(tester);
        final selectedColor = Palette.of(tester.element(_pin('Bruno')))
            .selected;
        final selected = find.byWidgetPredicate(
          (widget) =>
              widget is AnimatedContainer &&
              widget.decoration is BoxDecoration &&
              (widget.decoration! as BoxDecoration).color == selectedColor,
        );
        expect(
          find.descendant(of: _row(_pin('Bruno')), matching: selected),
          findsOneWidget,
        );
        await tester.ensureVisible(_inDrawer('Bruno').last);
        expect(
          find.descendant(
            of: _row(_inDrawer('Bruno').last),
            matching: selected,
          ),
          findsOneWidget,
        );
      },
      variant: variant,
    );

    testWidgets(
      '$name hold then release opens Unpin, applicable Move rows and settings',
      (tester) async {
        final h = await _show(tester, _workspace());
        await _openDrawer(tester);
        await _holdMenu(tester, _pin('Bruno'));
        expect(find.text('Bruno · Project Manager · Claktory'), findsOneWidget);
        expect(find.text('Unpin'), findsOneWidget);
        expect(find.text('Move up'), findsNothing);
        expect(find.text('Move down'), findsOneWidget);
        expect(find.text('Agent settings…'), findsOneWidget);
        expect(
          tester.getTopLeft(find.text('Unpin')).dy,
          lessThan(tester.getTopLeft(find.text('Move down')).dy),
        );
        expect(
          tester.getTopLeft(find.text('Move down')).dy,
          lessThan(tester.getTopLeft(find.text('Agent settings…')).dy),
        );
        await tester.tap(find.text('Move down'));
        await pumpFrames(tester);
        expect(
          (h.last.pinAgent.agentId, h.last.pinAgent.beforeId),
          ('bruno', 'speed'),
        );
        expect(_host(tester).drawerOpen, isTrue);
        await _holdMenu(tester, _pin('Speedwagon'));
        expect(find.text('Move down'), findsNothing);
        expect(find.text('Move up'), findsOneWidget);
        await tester.tap(find.text('Move up'));
        await pumpFrames(tester);
        expect(
          (h.last.pinAgent.agentId, h.last.pinAgent.beforeId),
          ('speed', 'dana'),
        );
      },
      variant: variant,
    );

    testWidgets(
      '$name conversation menu pins without navigating or optimistic local pin state',
      (tester) async {
        final h = await _show(tester, _workspace(pins: []));
        await _more(tester);
        expect(find.text('Pin'), findsOneWidget);
        expect(find.text('Agent settings…'), findsOneWidget);
        expect(
          tester.getTopLeft(find.text('Pin')).dy,
          lessThan(tester.getTopLeft(find.text('Agent settings…')).dy),
        );
        await tester.tap(find.text('Pin'));
        await pumpFrames(tester);
        expect(
          (h.last.pinAgent.agentId, h.last.pinAgent.beforeId),
          ('bruno', ''),
        );
        expect(find.text('Pin'), findsNothing);
        expect(find.byType(AgentView), findsOneWidget);
        await _openDrawer(tester);
        expect(_pin('Bruno'), findsNothing);
        await h.apply(tester, _workspace(pins: ['bruno']));
        await pumpFrames(tester);
        expect(_pin('Bruno'), findsOneWidget);
        await tester.tapAt(const Offset(370, 450));
        await pumpFrames(tester);
        await _more(tester);
        expect(find.text('Unpin'), findsOneWidget);
        await tester.tap(find.text('Unpin'));
        await pumpFrames(tester);
        expect(h.last.unpinAgent.agentId, 'bruno');
        expect(h.backend.sent.where((m) => m.hasSelectAgent()), isEmpty);
      },
      variant: variant,
    );

    testWidgets(
      '$name open conversation menu follows a pin changed on another device',
      (tester) async {
        final h = await _show(tester, _workspace(pins: []));
        await _more(tester);
        expect(find.text('Pin'), findsOneWidget);
        await h.apply(tester, _workspace(pins: ['bruno']));
        await pumpFrames(tester);
        expect(find.text('Pin'), findsNothing);
        expect(find.text('Unpin'), findsOneWidget);
        await tester.tap(find.text('Unpin'));
        await pumpFrames(tester);
        expect(h.last.unpinAgent.agentId, 'bruno');
      },
      variant: variant,
    );

    testWidgets(
      '$name settings opens after the conversation menu closes; workspace agent has no Pin',
      (tester) async {
        await _show(tester, _workspace(selected: 'rivet'));
        await _more(tester);
        expect(find.text('Pin'), findsNothing);
        expect(find.text('Unpin'), findsNothing);
        await tester.tap(find.text('Agent settings…'));
        await pumpFrames(tester);
        expect(find.byType(AgentSettingsForm), findsOneWidget);
        expect(find.text("Rivet's extensions"), findsNothing);
      },
      variant: variant,
    );

    testWidgets(
      '$name drag owns scrim, Close, sideways swipe and a captured Back until release',
      (tester) async {
        await _show(tester, _workspace());
        await _openDrawer(tester);
        final host = _host(tester);
        // The shared sidebar raises this while a row is picked up. Exercise
        // competing inputs independently without borrowing its drag recognizer.
        sidebarDragging.value = true;
        await tester.pump();
        final back = host.back.target!;
        expect(back, isA<TokenTarget>());
        expect(back.canClose, isFalse);
        expect(host.back.commit(back), isTrue);
        expect(host.drawerOpen, isTrue);
        await tester.tapAt(const Offset(370, 450));
        await tester.tap(find.bySemanticsLabel('Close').last);
        await tester.dragFrom(const Offset(340, 400), const Offset(-250, 0));
        await pumpFrames(tester);
        expect(host.drawerOpen, isTrue);
        sidebarDragging.value = false;
        await tester.pump();
        expect(
          host.back.commit(back),
          isFalse,
          reason: 'A completed drag must not retarget its pending Back',
        );
        expect(host.drawerOpen, isTrue);
        final next = host.back.target!;
        expect(next, isA<DrawerTarget>());
        expect(host.back.commit(next), isTrue);
        await pumpFrames(tester);
        expect(find.byType(Sidebar), findsNothing);
      },
      variant: variant,
    );

    testWidgets(
      '$name a real held drag dropped outside pins leaves order and drawer intact',
      (tester) async {
        final h = await _show(tester, _workspace());
        await _openDrawer(tester);
        final finger = await tester.startGesture(
          tester.getCenter(_pin('Bruno')),
        );
        await tester.pump(
          kLongPressTimeout + const Duration(milliseconds: 100),
        );
        expect(sidebarDragging.value, isTrue);
        await finger.moveTo(const Offset(370, 430));
        await pumpFrames(tester);
        expect(_host(tester).drawerOpen, isTrue);
        await finger.up();
        await pumpFrames(tester);
        expect(sidebarDragging.value, isFalse);
        expect(_host(tester).drawerOpen, isTrue);
        expect(find.text('Unpin'), findsNothing);
        expect(
          h.backend.sent.where(
            (m) => m.hasPinAgent() || m.hasUnpinAgent() || m.hasArrangeAgent(),
          ),
          isEmpty,
        );
      },
      variant: variant,
    );

    testWidgets(
      '$name a real held drag reorders only pins and releases the drawer hold',
      (tester) async {
        final h = await _show(tester, _workspace());
        await _openDrawer(tester);
        final from = tester.getCenter(_pin('Bruno'));
        final target =
            tester.getRect(_row(_pin('Dana'))).bottomCenter -
            const Offset(0, 2);
        final finger = await tester.startGesture(from);
        await tester.pump(
          kLongPressTimeout + const Duration(milliseconds: 100),
        );
        expect(sidebarDragging.value, isTrue);
        await finger.moveTo(target);
        await tester.pump(const Duration(milliseconds: 60));
        await finger.up();
        await pumpFrames(tester);
        expect(
          (h.last.pinAgent.agentId, h.last.pinAgent.beforeId),
          ('bruno', 'speed'),
        );
        expect(h.backend.sent.where((m) => m.hasArrangeAgent()), isEmpty);
        expect(sidebarDragging.value, isFalse);
        await tester.tapAt(const Offset(370, 450));
        await pumpFrames(tester);
        expect(find.byType(Sidebar), findsNothing);
      },
      variant: variant,
    );

    testWidgets(
      '$name pins remain when Projects is folded and when another project is focused',
      (tester) async {
        final h = await _show(tester, _workspace(projectsFolded: true));
        await _openDrawer(tester);
        expect(_pin('Bruno'), findsOneWidget);
        expect(_pin('Dana'), findsOneWidget);
        await h.apply(tester, _workspace(focused: true));
        await pumpFrames(tester);
        expect(_pin('Bruno'), findsOneWidget);
        expect(_pin('Dana'), findsOneWidget);
        expect(_inDrawer('Rivet'), findsOneWidget);
      },
      variant: variant,
    );

    testWidgets(
      '$name scrollable phone pins do not subtract folded reports waiting count',
      (tester) async {
        await _show(
          tester,
          _workspace(pins: ['speed'], folded: true, waiting: 2),
        );
        await _openDrawer(tester);
        expect(_pin('Speedwagon'), findsOneWidget);
        await tester.ensureVisible(_inDrawer('Bruno'));
        expect(
          find.bySemanticsLabel('Bruno has 2 things waiting for your answer'),
          findsOneWidget,
        );
      },
      variant: variant,
    );

    testWidgets(
      '$name AX5 pin role and project each get a line and actions stay reachable',
      (tester) async {
        await _show(tester, _workspace(), scale: 2.35);
        await _openDrawer(tester);
        await tester.ensureVisible(_pin('Bruno'));
        final project = find.descendant(
          of: _row(_pin('Bruno')),
          matching: find.text('Claktory'),
        );
        final role = find.descendant(
          of: _row(_pin('Bruno')),
          matching: find.text('Project Manager'),
        );
        expect(project, findsOneWidget);
        expect(role, findsOneWidget);
        expect(
          tester.getTopLeft(project).dy,
          greaterThan(tester.getTopLeft(role).dy),
        );
        final paragraph = tester.renderObject<RenderParagraph>(project);
        expect(paragraph.didExceedMaxLines, isFalse);
        await _holdMenu(tester, _pin('Bruno'));
        await tester.ensureVisible(find.text('Unpin'));
        final unpin = tester.renderObject<RenderParagraph>(find.text('Unpin'));
        expect(unpin.didExceedMaxLines, isFalse);
        await tester.ensureVisible(find.text('Move down'));
        expect(find.text('Move down'), findsOneWidget);
        await tester.ensureVisible(find.text('Agent settings…'));
        final settings = tester.renderObject<RenderParagraph>(
          find.text('Agent settings…'),
        );
        expect(settings.didExceedMaxLines, isFalse);
        expect(tester.takeException(), isNull);
      },
      variant: variant,
    );

    testWidgets('$name screen reader exposes pin and reorder actions', (
      tester,
    ) async {
      await _show(tester, _workspace());
      await _openDrawer(tester);
      final actions = _actions(
        tester
            .binding
            .renderViews
            .first
            .owner!
            .semanticsOwner!
            .rootSemanticsNode!,
        'Dana, Product Insights, in Insights, pinned',
      );
      expect(actions, containsAll(['Unpin', 'Move up', 'Move down']));
      expect(actions, isNot(contains('Pin')));
    }, variant: variant);
  }
}
