import 'package:factory_app/agent_view.dart' show AgentView;
import 'package:factory_app/app_dock.dart';
import 'package:factory_app/app_views/app_view.dart'
    show AppView, ChromeNamesApp;
import 'package:factory_app/layout.dart' show chatKind, leaves;
import 'package:factory_app/mobile/back.dart';
import 'package:factory_app/open_place.dart';
import 'package:factory_app/panes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';

// C5, the app dock: pinned apps at the foot of the rail (a strip on a
// phone), and the launcher with every installed app.

final _charts = AppInfo(
  id: 'charts',
  name: 'Charts',
  icon: 'chart-column',
  state: AppState.APP_STATE_RUNNING,
  views: [
    AppViewSpec(id: 'main', title: 'Charts'),
    AppViewSpec(id: 'days', title: 'Days'),
  ],
);

final _notes = AppInfo(
  id: 'notes',
  name: 'Notes',
  icon: 'book-open',
  state: AppState.APP_STATE_NEEDS_SETUP,
  status: 'Choose a folder for your notes.',
  views: [AppViewSpec(id: 'main', title: 'Notes')],
);

// No views: it works through agents only.
final _telegram = AppInfo(
  id: 'telegram',
  name: 'Telegram',
  icon: 'send',
  state: AppState.APP_STATE_PAUSED_CRASHED,
  stateDetail: 'It quit 3 times in a minute.',
);

Workspace _workspace({
  bool available = true,
  List<String> pinned = const ['charts', 'notes'],
}) => Workspace(
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      color: 'pink',
      modelAvailable: true,
      layout: ViewNode(
        id: 'chat',
        leaf: ViewLeaf(kind: chatKind),
      ),
    ),
  ],
  selectedAgentId: 'june',
  settings: Settings(userName: 'Matt'),
  apps: available ? [_charts.deepCopy(), _notes, _telegram] : [],
  appsAvailable: available,
  pinnedApps: available ? pinned : [],
);

Future<Harness> _show(
  WidgetTester tester,
  Workspace workspace, {
  Widget dock = const AppDock(),
  Size size = const Size(900, 700),
}) async {
  final harness = Harness(workspace)..backend.appliesLayouts = true;
  await harness.show(
    tester,
    size: size,
    // At the window's foot, as the rail has it.
    home: Align(
      alignment: Alignment.bottomLeft,
      child: dock is AppDock
          ? SizedBox(width: 40, child: dock)
          : SizedBox(width: size.width, child: dock),
    ),
  );
  await settle(tester);
  return harness;
}

/// The app's view under a header of Claktory's that names the app.
Finder _named() => find.ancestor(
  of: find.byType(AppView),
  matching: find.byType(ChromeNamesApp),
);

Finder _labeled(String label) => find.byWidgetPredicate(
  (w) => w is Semantics && w.properties.label == label,
);

List<PinApp> _pins(Harness harness) => [
  for (final m in harness.backend.sent)
    if (m.hasPinApp()) m.pinApp,
];

void main() {
  test('pinned apps in the dock’s order; captions and dots', () {
    final workspace = _workspace(pinned: ['notes', 'gone', 'charts']);
    expect(
      [for (final app in pinnedApps(workspace)) app.id],
      ['notes', 'charts'],
    );
    expect(appCaption(_charts), 'Running');
    expect(appCaption(_notes), 'Needs setup');
    expect(
      appCaption(_telegram),
      'Paused because it kept quitting · Works through your agents',
    );
    expect(appDetail(_notes), 'Choose a folder for your notes.');
    expect(appDetail(_telegram), 'It quit 3 times in a minute.');
  });

  test('an app’s view opens beside the conversation, once', () {
    final root = ViewNode(
      id: 'chat',
      leaf: ViewLeaf(kind: chatKind),
    );
    final opened = withApp(root, _charts);
    final pane = appPane(opened, 'charts', '', 'main')!;
    expect(pane.leaf.kind, appKind);
    expect(pane.leaf.title, 'Charts');
    // Stored by its id, so it's found by its id too.
    expect(pane.leaf.view, 'main');
    expect(appPane(opened, 'charts', 'main'), isNotNull);
    final days = withApp(opened, _charts, 'days');
    expect(appPane(days, 'charts', 'days')!.leaf.title, 'Days');
    expect(appPane(days, 'notes'), isNull);
  });

  test('"its first view" and the first view by id are the same pane', () {
    // An agent opened it by name.
    final root = ViewNode(
      id: 'chat',
      leaf: ViewLeaf(kind: chatKind),
    );
    final byName = withApp(root, _charts, 'main');
    expect(appPane(byName, 'charts', '', 'main'), isNotNull);
    // One opened as "its first", as before.
    final split = byName.deepCopy();
    for (final leaf in leaves(split)) {
      if (leaf.leaf.kind == appKind) leaf.leaf.view = '';
    }
    expect(appPane(split, 'charts', 'main', 'main'), isNotNull);
    expect(appPane(split, 'charts', 'days', 'main'), isNull);
  });

  test('a view titled as its app is named once', () {
    final workspace = Workspace(apps: [_charts]);
    expect(
      appViewTitle(
        ViewLeaf(kind: appKind, appId: 'charts', title: 'Charts'),
        workspace,
      ),
      'Charts app',
    );
    expect(
      appViewTitle(
        ViewLeaf(kind: appKind, appId: 'charts', title: 'Days'),
        workspace,
      ),
      'Charts app · Days',
    );
  });

  testWidgets('nothing while the backend offers no apps', (tester) async {
    await _show(tester, _workspace(available: false));
    expect(find.byType(GestureDetector), findsNothing);
    expect(_labeled('All apps'), findsNothing);
  });

  testWidgets('a pinned app\'s view fills the window; the layout stays', (
    tester,
  ) async {
    final harness = await _show(tester, _workspace());
    expect(_labeled('Charts, Running'), findsOneWidget);
    expect(_labeled('Notes, Needs setup'), findsOneWidget);
    await tester.tap(_labeled('Charts, Running'));
    await settle(tester);
    // Expanded, as a pane's Expand does: the band's way back, and the
    // view's header with Collapse.
    expect(find.byType(ExpandedPane), findsOneWidget);
    expect(_labeled('Back to June'), findsOneWidget);
    expect(_labeled('Collapse Charts app'), findsOneWidget);
    expect(find.byType(AppPane), findsOneWidget);
    // The band and header name the app: the view goes without its own
    // "From" line.
    expect(_named(), findsOneWidget);
    expect(find.text('Charts app'), findsWidgets);
    // June's layout isn't touched.
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
    // Esc goes back.
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(find.byType(ExpandedPane), findsNothing);
    // So do Collapse and Back to June.
    await tester.tap(_labeled('Charts, Running'));
    await settle(tester);
    await tester.tap(_labeled('Collapse Charts app'));
    await settle(tester);
    expect(find.byType(ExpandedPane), findsNothing);
    await tester.tap(_labeled('Charts, Running'));
    await settle(tester);
    await tester.tap(_labeled('Back to June'));
    await settle(tester);
    expect(find.byType(ExpandedPane), findsNothing);
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
    expect(
      harness.backend.workspace.workspace.agents.single.layout,
      _workspace().agents.single.layout,
    );
  });

  testWidgets('an app\'s view opens its entries\' places: the view filling '
      'the window closes, and the place opens', (tester) async {
    final harness = await _show(tester, _workspace());
    await tester.tap(_labeled('Charts, Running'));
    await settle(tester);
    final place = tester.widget<OpenPlace>(find.byType(OpenPlace));
    expect(
      await place.gone(const OpenAgent('gone')),
      'This agent was deleted.',
    );
    expect(await place.gone(const OpenAgent('june')), isNull);

    place.open(
      tester.element(find.byType(AppPane)),
      const OpenMonitor('june', 2),
    );
    await settle(tester);
    expect(find.byType(ExpandedPane), findsNothing);
    expect(
      harness.backend.sent.lastWhere((m) => m.hasSetLayout()).toString(),
      contains('monitors'),
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets("on a phone, the page's header naming the app stays as its "
      'view scrolls down', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel(RegExp(r'^(Menu|Back)$')).first);
    await pumpFrames(tester, const Duration(seconds: 1));
    await tester.ensureVisible(find.text('Charts').last);
    await tester.tap(find.text('Charts').last);
    await pumpFrames(tester, const Duration(seconds: 1));
    final header = find.text('Charts app').hitTestable();
    expect(header, findsOneWidget);
    // As a dashboard read down a screen and more would say it.
    final view = tester.element(find.byType(AppView));
    for (var at = 100.0; at <= 1000; at += 100) {
      ScrollUpdateNotification(
        metrics: FixedScrollMetrics(
          minScrollExtent: 0,
          maxScrollExtent: 2000,
          pixels: at,
          viewportDimension: 700,
          axisDirection: AxisDirection.down,
          devicePixelRatio: 3,
        ),
        context: view,
        scrollDelta: 100,
        dragDetails: DragUpdateDetails(globalPosition: Offset.zero),
      ).dispatch(view);
      await tester.pump();
    }
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(header, findsOneWidget);
  });

  testWidgets("filling the window, an app's view hides the conversation "
      'under it from VoiceOver; Esc brings it back', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = Harness(_workspace())..backend.appliesLayouts = true;
    await harness.show(tester, size: const Size(1200, 800));
    await settle(tester);
    final covered = find.semantics.byLabel('More options for June');
    expect(covered, findsOne);

    openApp(tester.element(find.byType(AgentView)), _charts);
    await settle(tester);
    // Past its fade in, when it's read.
    await tester.pump(const Duration(seconds: 1));
    expect(find.byType(ExpandedPane), findsOneWidget);
    expect(covered, findsNothing);
    // The view's own controls are read.
    expect(find.semantics.byLabel('Back to June'), findsOne);

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(find.byType(ExpandedPane), findsNothing);
    expect(covered, findsOne);
    semantics.dispose();
  });

  testWidgets("filling the window, an app's view takes the window's Back, "
      'below a dialog over it; closed, it lets go', (tester) async {
    final back = PhoneBack.root();
    addTearDown(back.dispose);
    final harness = Harness(_workspace())..backend.appliesLayouts = true;
    await harness.show(
      tester,
      size: const Size(1200, 800),
      home: PhoneBackScope(back: back, child: const Shell()),
    );
    await settle(tester);
    expect(back.target, isNull);
    final element = tester.element(find.byType(AgentView));

    openApp(element, _charts);
    await settle(tester);
    // Another app in its place holds it once.
    openApp(element, _notes);
    await settle(tester);
    expect(find.byType(ExpandedPane), findsOneWidget);
    var dialog = 0;
    final modal = back.register(CallbackBackToken(onBack: () => dialog++));
    expect(back.commit(back.target!), isTrue);
    expect(dialog, 1);
    expect(find.byType(ExpandedPane), findsOneWidget);
    modal.release();

    expect(back.commit(back.target!), isTrue);
    await settle(tester);
    expect(find.byType(ExpandedPane), findsNothing);
    expect(back.target, isNull);

    // Closed by its own Back chip, it leaves the window's Back to the app.
    openApp(element, _charts);
    await settle(tester);
    expect(back.target, isNotNull);
    await tester.tap(_labeled('Back to June'));
    await settle(tester);
    expect(find.byType(ExpandedPane), findsNothing);
    expect(back.target, isNull);
  });

  testWidgets('another app opened meanwhile takes its place', (tester) async {
    await _show(tester, _workspace());
    final element = tester.element(_labeled('Charts, Running'));
    openApp(element, _charts);
    await settle(tester);
    openApp(element, _charts);
    await settle(tester);
    expect(find.byType(ExpandedPane), findsOneWidget);
    openApp(element, _notes);
    await settle(tester);
    expect(find.byType(ExpandedPane), findsOneWidget);
    expect(_labeled('Collapse Notes app'), findsOneWidget);
    await tester.tap(_labeled('Back to June'));
    await settle(tester);
    expect(find.byType(ExpandedPane), findsNothing);
  });

  testWidgets('the launcher lists every app; pins and unpins', (tester) async {
    final harness = await _show(tester, _workspace(pinned: ['charts']));
    await tester.tap(_labeled('All apps'));
    await settle(tester);
    expect(find.text('Telegram'), findsOneWidget);
    expect(
      find.text('Paused because it kept quitting · Works through your agents'),
      findsOneWidget,
    );
    // Only an app with views can be pinned.
    expect(_labeled('Pin Telegram'), findsNothing);
    await tester.tap(_labeled('Pin Notes'));
    await settle(tester);
    expect(
      _pins(harness).last,
      PinApp(appId: 'notes', pinned: true, position: -1),
    );
    expect(harness.backend.workspace.workspace.pinnedApps, ['charts', 'notes']);
    expect(_labeled('Unpin Notes'), findsOneWidget);
    await tester.tap(_labeled('Unpin Charts'));
    await settle(tester);
    expect(_pins(harness).last.pinned, isFalse);
    expect(find.byKey(const ValueKey('dock-charts')), findsNothing);
  });

  testWidgets('an app with no views opens the Marketplace\'s Apps', (
    tester,
  ) async {
    final harness = await _show(tester, _workspace());
    await tester.tap(_labeled('All apps'));
    await settle(tester);
    await tester.tap(find.text('Telegram'));
    await settle(tester);
    expect(harness.backend.marketplaceTab.value, 'apps');
    final layout = harness.backend.sent.lastWhere((m) => m.hasSetLayout());
    expect(
      pagePane(layout.setLayout.root, AppPage.marketplace),
      isNotNull,
      reason: 'the Marketplace opens beside June',
    );
    // The launcher closed.
    expect(find.text('Manage apps…'), findsNothing);
  });

  testWidgets('dragging a tile onto another moves it there', (tester) async {
    final harness = await _show(tester, _workspace());
    final from = tester.getCenter(_labeled('Notes, Needs setup'));
    final to = tester.getCenter(_labeled('Charts, Running'));
    final gesture = await tester.startGesture(from);
    await tester.pump(const Duration(milliseconds: 50));
    await gesture.moveTo(to);
    await tester.pump();
    await gesture.up();
    await settle(tester);
    final pin = _pins(harness).single;
    expect((pin.appId, pin.pinned, pin.position), ('notes', true, 0));
    expect(harness.backend.workspace.workspace.pinnedApps, ['notes', 'charts']);
  });

  testWidgets('on a phone, a strip of chips and All apps as a sheet', (
    tester,
  ) async {
    final harness = await _show(
      tester,
      _workspace(),
      dock: const AppStrip(),
      size: const Size(393, 852),
    );
    expect(find.text('Charts'), findsOneWidget);
    expect(find.text('Notes'), findsOneWidget);
    // Last in the strip, which scrolls sideways.
    await tester.ensureVisible(find.text('All apps'));
    await settle(tester);
    await tester.tap(find.text('All apps'));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Telegram'), findsOneWidget);
    expect(find.text('Manage apps…'), findsOneWidget);
    await tester.tap(find.text('Telegram'));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(harness.backend.marketplaceTab.value, 'apps');
    expect(tester.takeException(), isNull);
  });

  // On a phone, the strip's app opened as a page of its own,
  // never as a pane in the agent's layout, which the person's Mac and
  // iPad share. An iPad's narrow window takes the phone's routes.
  for (final (name, size, ipad) in [
    ('a phone', const Size(393, 852), false),
    ('an iPad\'s narrow window', const Size(500, 1024), true),
  ]) {
    testWidgets('on $name, an app opens as a page; the layout stays', (
      tester,
    ) async {
      if (ipad) asIPad();
      final harness = Harness(_workspace());
      await harness.show(tester, size: size);
      await pumpFrames(tester);
      await tester.tap(find.bySemanticsLabel(RegExp(r'^(Menu|Back)$')).first);
      await pumpFrames(tester, const Duration(seconds: 1));
      await tester.ensureVisible(find.text('Charts').last);
      await tester.tap(find.text('Charts').last);
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.byType(AppPane), findsOneWidget);
      // Its page's header names the app, so the view goes without.
      expect(_named(), findsOneWidget);
      expect(find.text('Charts app'), findsOneWidget);
      expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
      expect(
        harness.backend.workspace.workspace.agents.single.layout,
        _workspace().agents.single.layout,
      );
      // Its header: the app's name, with Back to the conversation.
      expect(find.bySemanticsLabel('Back'), findsOneWidget);
      await tester.tap(find.bySemanticsLabel('Back'));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.byType(AppPane), findsNothing);
      expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
      expect(tester.takeException(), isNull);
    });
  }
}
