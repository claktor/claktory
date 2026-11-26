// The app strip on an iPhone (C5): the pinned apps' chips in the menu,
// under its header. A chip opens its app's view as a page over the
// agent's conversation, never in its layout, which the person's other
// devices share (Bruno 42883); so does an app chosen from All apps. Back
// pops it.

import 'package:factory_app/app_views/app_view.dart';
import 'package:factory_app/layout.dart' show chatKind;
import 'package:factory_app/mobile/drawer.dart'
    show PhoneDrawerHost, PhoneDrawerHostState;
import 'package:factory_app/panes.dart' show appKind;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show leaf, row, split;

final _charts = AppInfo(
  id: 'charts',
  name: 'Charts',
  icon: 'chart-column',
  state: AppState.APP_STATE_RUNNING,
  views: [AppViewSpec(id: 'main', title: 'Charts')],
);

final _notes = AppInfo(
  id: 'notes',
  name: 'Notes',
  icon: 'book-open',
  state: AppState.APP_STATE_RUNNING,
  views: [AppViewSpec(id: 'main', title: 'Notes')],
);

Workspace _workspace({ViewNode? layout, bool apps = true}) => Workspace(
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      color: 'pink',
      modelAvailable: true,
      layout:
          layout ??
          ViewNode(
            id: 'chat',
            leaf: ViewLeaf(kind: chatKind),
          ),
    ),
  ],
  selectedAgentId: 'june',
  settings: Settings(userName: 'Matt'),
  apps: apps ? [_charts, _notes] : [],
  appsAvailable: apps,
  pinnedApps: apps ? ['charts'] : [],
);

Future<Harness> _phone(
  WidgetTester tester,
  Workspace workspace, {
  Size size = const Size(393, 852),
  double textScale = 1,
}) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  tester.platformDispatcher.textScaleFactorTestValue = textScale;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  final harness = Harness(workspace)..backend.appliesLayouts = true;
  tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
  addTearDown(tester.view.resetPadding);
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  return harness;
}

Future<void> _openMenu(WidgetTester tester) async {
  await tester.tap(find.bySemanticsLabel('Menu').last);
  await pumpFrames(tester, const Duration(seconds: 1));
  expect(find.byType(Sidebar), findsOneWidget);
}

Iterable<ClientMessage> _layouts(Harness harness) =>
    harness.backend.sent.where((m) => m.hasSetLayout());

/// Back, at the page's top left: the conversation again.
Future<void> _back(WidgetTester tester) async {
  await tester.tap(find.bySemanticsLabel('Back').last);
  await pumpFrames(tester, const Duration(seconds: 1));
  expect(find.byType(AppView), findsNothing);
}

void main() {
  testWidgets('a chip in the menu opens its view as a page over the '
      'conversation', (tester) async {
    final harness = await _phone(tester, _workspace());
    await _openMenu(tester);
    // Under the menu's header, above its search.
    final chip = find.bySemanticsLabel(RegExp('^Charts, '));
    expect(chip, findsOneWidget);
    expect(
      tester.getRect(chip).bottom,
      lessThanOrEqualTo(tester.getRect(find.bySemanticsLabel('Search')).top),
    );
    expect(tester.getSize(chip).height, greaterThanOrEqualTo(44));
    await tester.tap(chip);
    await pumpFrames(tester, const Duration(seconds: 1));
    // A page here, and nothing in the agent's layout.
    expect(_layouts(harness), isEmpty);
    expect(find.byType(Sidebar), findsNothing);
    expect(tester.widget<AppView>(find.byType(AppView)).appId, 'charts');
    expect(harness.backend.sent.where((m) => m.hasSelectAgent()), isEmpty);
    await _back(tester);
    expect(_layouts(harness), isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets('a view open already in the layout: a page all the same, the '
      'layout untouched', (
    tester,
  ) async {
    final harness = await _phone(
      tester,
      _workspace(
        layout: split('s1', row, [
          leaf('chat', chatKind),
          ViewNode(
            id: 'v1',
            leaf: ViewLeaf(kind: appKind, appId: 'charts'),
          ),
        ]),
      ),
    );
    await _openMenu(tester);
    await tester.tap(find.bySemanticsLabel(RegExp('^Charts, ')));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
    expect(tester.widget<AppView>(find.byType(AppView)).appId, 'charts');
  });

  testWidgets('an app chosen from All apps opens as a page too', (
    tester,
  ) async {
    final harness = await _phone(tester, _workspace());
    await _openMenu(tester);
    await tester.ensureVisible(find.text('All apps'));
    await pumpFrames(tester);
    await tester.tap(find.text('All apps'));
    await pumpFrames(tester, const Duration(seconds: 1));
    await tester.tap(find.text('Notes').last);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(_layouts(harness), isEmpty);
    expect(find.byType(Sidebar), findsNothing);
    expect(tester.widget<AppView>(find.byType(AppView)).appId, 'notes');
    await _back(tester);
    expect(tester.takeException(), isNull);
  });

  testWidgets('no apps: no strip', (tester) async {
    await _phone(tester, _workspace(apps: false));
    await _openMenu(tester);
    expect(find.text('All apps'), findsNothing);
  });

  testWidgets('at the largest text on a small iPhone, the strip fits', (
    tester,
  ) async {
    await _phone(
      tester,
      _workspace(),
      size: const Size(320, 568),
      textScale: 3.1,
    );
    await _openMenu(tester);
    expect(find.text('Charts'), findsWidgets);
    expect(tester.takeException(), isNull);
  });

  testWidgets('at the largest text the menu\'s top scrolls within 40% of '
      'it, leaving the list the rest', (tester) async {
    await _phone(
      tester,
      _workspace(),
      size: const Size(320, 568),
      textScale: 3.1,
    );
    await _openMenu(tester);
    final top = find.byKey(const ValueKey('drawer-top'));
    final menu = tester.getSize(find.bySemanticsLabel('Menu').first).height;
    expect(tester.getSize(top).height, lessThanOrEqualTo(568 * 0.4));
    expect(
      tester.getSize(find.byType(Sidebar)).height,
      greaterThan(menu * 0.3),
    );
    // What doesn't fit is a scroll away.
    await tester.drag(top, const Offset(0, -400));
    await pumpFrames(tester);
    expect(find.text('All apps'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('at default text the menu\'s top doesn\'t scroll', (
    tester,
  ) async {
    await _phone(tester, _workspace(), size: const Size(320, 568));
    await _openMenu(tester);
    final top = find.byKey(const ValueKey('drawer-top'));
    final position = tester
        .state<ScrollableState>(
          find.descendant(of: top, matching: find.byType(Scrollable)).first,
        )
        .position;
    expect(position.maxScrollExtent, 0);
  });

  testWidgets('closeAll closes an open drawer, for a sheet from outside', (
    tester,
  ) async {
    await _phone(tester, _workspace());
    await _openMenu(tester);
    final host = tester.state<PhoneDrawerHostState>(
      find.byType(PhoneDrawerHost),
    );
    expect(host.drawerOpen, isTrue);
    PhoneDrawerHost.closeAll();
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(host.drawerOpen, isFalse);
    // Closed already: nothing happens.
    PhoneDrawerHost.closeAll();
    await pumpFrames(tester);
    expect(host.drawerOpen, isFalse);
  });
}
