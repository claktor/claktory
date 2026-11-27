// A project's row (Lucy's projects-sidebar.md §4, area 5b items 2 and 3):
// a click folds it, and folded it speaks for its agents; its ⋯ (on a
// Mac's hover, always by touch), a right click where the pointer is, and
// the keyboard open one menu: Files, Add an agent…, Connect a pipe…, New
// pipe group… once it has a pipe, Show only this project / Show all
// projects, Project settings….
import 'package:factory_app/controls.dart'
    show Chevron, ExtensionGlyph, FolderGlyph, WorkingWave;
import 'package:factory_app/forms.dart'
    show CreateProjectForm, FormFrame, ProjectSettingsForm;
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:factory_app/pipes.dart' show ConnectPipeForm;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'android/support/android_harness.dart';
import 'harness.dart';
import 'ipad/as_ipad.dart';

Agent _agent(
  String id,
  String name, {
  String project = 'p1',
  bool working = false,
  List<int> waiting = const [],
}) => Agent(
  id: id,
  name: name,
  projectId: project,
  modelAvailable: true,
  status: working
      ? AgentStatus.AGENT_STATUS_WORKING
      : AgentStatus.AGENT_STATUS_IDLE,
  waitingSeqs: [for (final seq in waiting) Int64(seq)],
);

Workspace _workspace({
  bool collapsed = false,
  bool pipe = false,
  bool working = false,
  String focused = '',
}) => Workspace(
  projects: [
    Project(id: 'p1', name: 'Site', path: '/w/site', collapsed: collapsed),
    Project(id: 'p2', name: 'Other', path: '/w/other'),
  ],
  agents: [
    _agent('ana', 'Ana', working: working, waiting: [3, 4]),
    _agent('ben', 'Ben'),
    _agent('cy', 'Cy', project: 'p2'),
  ],
  pipes: [
    if (pipe)
      Pipe(
        id: 'pipe1',
        name: 'Acme launch',
        projectId: 'p1',
        state: PipeState.PIPE_STATE_OPEN,
      ),
  ],
  focusedProjectId: focused,
);

Future<Harness> _show(WidgetTester tester, Workspace snapshot) async {
  final harness = Harness(snapshot);
  await harness.show(tester);
  await settle(tester);
  return harness;
}

Finder _labeled(String label) => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == label,
);

/// Hovers [text]'s row with a mouse, so its buttons show.
Future<TestGesture> _hover(WidgetTester tester, String text) async {
  FocusManager.instance.highlightStrategy =
      FocusHighlightStrategy.alwaysTraditional;
  addTearDown(
    () => FocusManager.instance.highlightStrategy =
        FocusHighlightStrategy.automatic,
  );
  final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
  await mouse.addPointer(location: tester.getCenter(find.text(text).first));
  addTearDown(mouse.removePointer);
  await tester.pump();
  return mouse;
}

const _menu = [
  'Files',
  'Add an agent…',
  'Connect a pipe…',
  'Show only this project',
  'Project settings…',
];

void main() {
  testWidgets('a click folds it; folded, only its row shows, with what '
      'waits and a wave while one of its agents works', (tester) async {
    final harness = await _show(tester, _workspace());
    expect(find.text('Ana'), findsOneWidget);
    await tester.tap(find.text('Site'));
    await settle(tester);
    expect(
      harness.last.setProjectCollapsed,
      SetProjectCollapsed(projectId: 'p1', collapsed: true),
    );
    expect(harness.backend.sent.where((m) => m.hasFocusProject()), isEmpty);

    await harness.apply(tester, _workspace(collapsed: true, working: true));
    await settle(tester);
    expect(find.text('Ana'), findsNothing);
    expect(find.text('Ben'), findsNothing);
    expect(find.text('Cy'), findsOneWidget, reason: 'another project');
    expect(
      _labeled('Site has 2 things waiting for your answer'),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: find.ancestor(of: find.text('Site'), matching: find.byType(Stack)),
        matching: find.byType(WorkingWave),
      ),
      findsWidgets,
    );
    // VoiceOver hears it folded.
    final semantics = tester.ensureSemantics();
    expect(
      tester.getSemantics(find.text('Site')),
      isSemantics(hasExpandedState: true, isExpanded: false),
    );
    semantics.dispose();

    // A click shows them again.
    await tester.tap(find.text('Site'));
    await settle(tester);
    expect(harness.last.setProjectCollapsed.collapsed, isFalse);
  });

  testWidgets("on a Mac, + and ⋯ on hover; ⋯ opens the project's menu, with "
      'no New pipe group… until it has a pipe', (tester) async {
    await _show(tester, _workspace());
    expect(find.bySemanticsLabel('Site options'), findsNothing);
    final mouse = await _hover(tester, 'Site');
    expect(find.bySemanticsLabel('Site options'), findsOneWidget);
    expect(find.bySemanticsLabel('New agent in Site'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Site options'));
    await settle(tester);
    for (final item in _menu) {
      expect(find.text(item), findsOneWidget, reason: item);
    }
    expect(find.text('New pipe group…'), findsNothing);
    // Its ⋯ stays while the menu is open, the pointer gone.
    await mouse.moveTo(const Offset(700, 500));
    await tester.pump();
    expect(find.bySemanticsLabel('Site options'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(find.text('Connect a pipe…'), findsNothing);
    expect(find.bySemanticsLabel('Site options'), findsNothing);
  });

  testWidgets('a right click opens the menu where the pointer is', (
    tester,
  ) async {
    await _show(tester, _workspace(pipe: true));
    final at = tester.getCenter(find.text('Site')) + const Offset(30, 0);
    await tester.tapAt(
      at,
      buttons: kSecondaryButton,
      kind: PointerDeviceKind.mouse,
    );
    await settle(tester);
    expect(find.text('New pipe group…'), findsOneWidget);
    final first = tester.getTopLeft(find.text('Files'));
    expect((first.dy - at.dy).abs(), lessThan(40), reason: 'by the pointer');
    expect(first.dx, greaterThan(at.dx - 1));
    expect(
      find.byType(ProjectSettingsForm),
      findsNothing,
      reason: 'no longer its settings at once',
    );
  });

  testWidgets("the menu's items do what they say", (tester) async {
    final harness = await _show(tester, _workspace(pipe: true));
    Future<void> choose(String item) async {
      await tester.tap(find.text('Site'), buttons: kSecondaryButton);
      await settle(tester);
      await tester.tap(find.text(item));
      await settle(tester);
    }

    await choose('Connect a pipe…');
    expect(find.byType(ConnectPipeForm), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);

    await choose('New pipe group…');
    expect(find.text('New group'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);

    await choose('Project settings…');
    expect(find.byType(ProjectSettingsForm), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);

    await choose('Show only this project');
    expect(harness.last.focusProject.projectId, 'p1');
    await harness.apply(tester, _workspace(pipe: true, focused: 'p1'));
    await settle(tester);
    // Focused, its heading opens the same menu, which goes back.
    await tester.tap(find.text('Site').first);
    await settle(tester);
    expect(find.text('Show only this project'), findsNothing);
    await tester.tap(find.text('Show all projects'));
    await settle(tester);
    expect(harness.last.focusProject.projectId, '');
    expect(find.byType(FormFrame), findsNothing);
  });

  testWidgets('from the keyboard: ← folds, → unfolds, Return opens the menu', (
    tester,
  ) async {
    final harness = await _show(tester, _workspace());
    FocusManager.instance.highlightStrategy =
        FocusHighlightStrategy.alwaysTraditional;
    addTearDown(
      () => FocusManager.instance.highlightStrategy =
          FocusHighlightStrategy.automatic,
    );
    final row = find.ancestor(
      of: find.text('Site'),
      matching: find.byType(FocusableActionDetector),
    );
    tester.widget<FocusableActionDetector>(row.first).focusNode!.requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await settle(tester);
    expect(harness.last.setProjectCollapsed.collapsed, isTrue);
    await harness.apply(tester, _workspace(collapsed: true));
    await settle(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await settle(tester);
    expect(harness.last.setProjectCollapsed.collapsed, isFalse);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(find.text('Connect a pipe…'), findsOneWidget);
    expect(
      harness.backend.sent.where((m) => m.hasSetProjectCollapsed()),
      hasLength(2),
      reason: 'Return opens the menu, and folds nothing',
    );
  });

  testWidgets("on an iPad, ⋯ is always there, a finger's 44 points", (
    tester,
  ) async {
    asIPad();
    final harness = Harness(_workspace());
    await harness.show(tester, size: const Size(1194, 834));
    await pumpFrames(tester);
    final more = find.bySemanticsLabel('Site options');
    expect(more, findsOneWidget);
    expect(tester.getSize(more).height, greaterThanOrEqualTo(44));
    await tester.tap(more);
    await pumpFrames(tester);
    expect(find.text('Connect a pipe…'), findsOneWidget);
  });

  testWidgets('on an iPad, while its Files show, its row has the glyph that '
      'closes them', (tester) async {
    asIPad();
    final snapshot = _workspace()..selectedAgentId = 'ana';
    final harness = Harness(snapshot);
    await harness.show(tester, size: const Size(1194, 834));
    await pumpFrames(tester);
    expect(_labeled('Hide files'), findsNothing);
    snapshot.agents.first.layout = ViewNode(
      id: 's1',
      split: ViewSplit(
        axis: ViewAxis.VIEW_AXIS_ROW,
        children: [
          ViewNode(
            id: 'chat',
            leaf: ViewLeaf(kind: 'chat'),
          ),
          ViewNode(
            id: 'f1',
            leaf: ViewLeaf(kind: 'files', path: 'p1'),
          ),
        ],
      ),
    );
    await harness.apply(tester, snapshot);
    await pumpFrames(tester);
    expect(_labeled('Hide files'), findsOneWidget);
    await tester.tap(_labeled('Hide files'));
    await pumpFrames(tester);
    expect(harness.last.setLayout.root.leaf.kind, 'chat');
  });

  group('a phone focuses no project', () {
    var androidPolicy = false;
    setUp(() {
      androidPolicy = PhonePlatformPolicy.debugAndroidPolicy;
      PhonePlatformPolicy.debugAndroidPolicy = true;
    });
    tearDown(() => PhonePlatformPolicy.debugAndroidPolicy = androidPolicy);

    Future<Harness> onPhone(WidgetTester tester, Workspace snapshot) async {
      final harness = Harness(snapshot..selectedAgentId = 'ana');
      if (defaultTargetPlatform == TargetPlatform.android) {
        await showAndroid(tester, harness: harness, size: const Size(390, 844));
      } else {
        await harness.show(tester, size: const Size(390, 844));
      }
      await pumpFrames(tester);
      await tester.tap(find.bySemanticsLabel(RegExp(r'^Menu$')));
      await pumpFrames(tester);
      return harness;
    }

    Finder inDrawer(String text) =>
        find.descendant(of: find.byType(Sidebar), matching: find.text(text));

    testWidgets(
      'every project shows, though one is focused elsewhere, and its menu '
      'offers no Show only this project',
      (tester) async {
        await onPhone(tester, _workspace(focused: 'p1'));
        expect(inDrawer('Other'), findsOneWidget);
        expect(inDrawer('Cy'), findsOneWidget);
        expect(inDrawer('‹ All projects'), findsNothing);
        await tester.tap(find.bySemanticsLabel('Site options'));
        await pumpFrames(tester);
        expect(find.text('Project settings…'), findsOneWidget);
        expect(find.text('Show only this project'), findsNothing);
        expect(find.text('Show all projects'), findsNothing);
      },
      variant: TargetPlatformVariant.only(TargetPlatform.iOS),
    );

    testWidgets('its glyphs grow with the text, as SF Symbols do', (
      tester,
    ) async {
      tester.platformDispatcher.textScaleFactorTestValue = 1.5;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final snapshot = _workspace()
        ..agents.add(
          Agent(
            id: 'kid',
            name: 'Kid',
            projectId: 'p1',
            leadId: 'ben',
            parentId: 'ben',
            modelAvailable: true,
          ),
        );
      await onPhone(tester, snapshot);
      double side<T>({String? icon}) => tester
          .getRect(
            find
                .descendant(
                  of: find.byType(Sidebar),
                  matching: find.byWidgetPredicate(
                    (widget) =>
                        widget is T &&
                        (icon == null ||
                            widget is ExtensionGlyph && widget.icon == icon),
                  ),
                )
                .first,
          )
          .width;
      expect(side<FolderGlyph>(), moreOrLessEquals(14 * 1.5, epsilon: 0.5));
      for (final chevron
          in find
              .descendant(
                of: find.byType(Sidebar),
                matching: find.byType(Chevron),
              )
              .evaluate()) {
        expect(
          tester.getRect(find.byWidget(chevron.widget)).width,
          moreOrLessEquals(10 * 1.5, epsilon: 0.5),
        );
      }
      expect(
        side<ExtensionGlyph>(icon: 'ellipsis'),
        moreOrLessEquals(20 * 1.5, epsilon: 0.5),
      );
      expect(
        side<ExtensionGlyph>(icon: 'inbox'),
        moreOrLessEquals(18 * 1.5, epsilon: 0.5),
      );
      expect(tester.takeException(), isNull);
    }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));

    testWidgets('on Android too', (tester) async {
      await onPhone(tester, _workspace(focused: 'p1'));
      expect(inDrawer('Other'), findsOneWidget);
      expect(inDrawer('‹ All projects'), findsNothing);
      await tester.tap(find.bySemanticsLabel('Site options'));
      await pumpFrames(tester);
      expect(find.text('Project settings…'), findsOneWidget);
      expect(find.text('Show only this project'), findsNothing);
    }, variant: TargetPlatformVariant.only(TargetPlatform.android));

    testWidgets('a project made on a phone is made, not focused', (
      tester,
    ) async {
      final harness = Harness(_workspace());
      harness.backend.onSend = (message) {
        if (message.hasCreateProject()) {
          harness.backend.workspace.apply(
            _workspace()..projects.add(Project(id: 'p3', name: 'Bakery')),
          );
        }
      };
      await harness.show(
        tester,
        size: const Size(390, 844),
        home: CreateProjectForm(close: () {}),
      );
      await pumpFrames(tester);
      await tester.enterText(find.byType(EditableText).first, 'Bakery');
      await tester.pump();
      await tester.tap(find.text('Create').last);
      await pumpFrames(tester);
      expect(harness.last.createProject.name, 'Bakery');
      expect(harness.backend.sent.where((m) => m.hasFocusProject()), isEmpty);
    }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));
  });
}
