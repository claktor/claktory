import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/extensions.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'resources_test.dart' show resourcesWorkspace;

Workspace snapshot() => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/w')],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
    Agent(id: 'a1', projectId: 'p1', name: 'Builder', modelAvailable: true),
  ],
  selectedAgentId: 'a1',
);

void main() {
  testWidgets('the sidebar edge drags within its limits, is sent once, and '
      'a double click resets it', (tester) async {
    final harness = Harness(snapshot());
    await harness.show(tester);
    await settle(tester);
    final edge = find.byKey(const ValueKey('sidebar-edge'));
    final gesture = await tester.startGesture(tester.getCenter(edge));
    for (var i = 0; i < 10; i++) {
      await gesture.moveBy(const Offset(-20, 0));
      await tester.pump();
    }
    expect(tester.getSize(find.byType(Sidebar)).width, 200);
    expect(
      harness.backend.sent.where((message) => message.hasUpdateSettings()),
      isEmpty,
    );
    await gesture.moveBy(const Offset(1000, 0));
    await tester.pump();
    expect(tester.getSize(find.byType(Sidebar)).width, 300);
    await gesture.up();
    await settle(tester);
    final updates = [
      for (final message in harness.backend.sent)
        if (message.hasUpdateSettings()) message.updateSettings,
    ];
    expect(updates, hasLength(1));
    expect(updates.single.sidebarWidth, 300);

    await tester.tap(edge);
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tap(edge);
    await settle(tester);
    expect(harness.last.updateSettings.hasSidebarWidth(), isTrue);
    expect(harness.last.updateSettings.sidebarWidth, 0);
  });

  testWidgets('a kept sidebar width shows, within its limits', (tester) async {
    final harness = Harness(
      snapshot()..settings = (Settings()..sidebarWidth = 280),
    );
    await harness.show(tester);
    await settle(tester);
    expect(tester.getSize(find.byType(Sidebar)).width, 280);
    await harness.apply(
      tester,
      snapshot()..settings = (Settings()..sidebarWidth = 900),
    );
    await settle(tester);
    expect(tester.getSize(find.byType(Sidebar)).width, 300);
  });

  testWidgets('a dragged width stays until the backend has it, or refuses', (
    tester,
  ) async {
    final harness = Harness(snapshot());
    await harness.show(tester);
    await settle(tester);
    final edge = find.byKey(const ValueKey('sidebar-edge'));
    await tester.drag(edge, const Offset(30, 0));
    await settle(tester);
    double width() => tester.getSize(find.byType(Sidebar)).width;
    expect(harness.last.updateSettings.sidebarWidth, 278);
    expect(width(), 278, reason: 'no jump back while the backend saves it');

    await harness.apply(
      tester,
      snapshot()..settings = (Settings()..sidebarWidth = 278),
    );
    expect(width(), 278);
    // From now on it's the backend's again.
    await harness.apply(
      tester,
      snapshot()..settings = (Settings()..sidebarWidth = 220),
    );
    expect(width(), 220);

    harness.backend.refuse = 'settings are read-only';
    await tester.drag(edge, const Offset(40, 0));
    await settle(tester);
    expect(width(), 220, reason: 'refused: the setting in effect');
  });

  Future<Harness> showPanel(WidgetTester tester, {Settings? settings}) async {
    final snapshot = resourcesWorkspace();
    if (settings != null) snapshot.settings = settings;
    final harness = Harness(snapshot);
    harness.backend.extensionView.apply(
      ExtensionView(
        extensionId: 'resources',
        agentId: 'a1',
        title: 'Resources',
        // Little in it: the test font's wide letters would overflow it.
        resources: Resources(),
      ),
    );
    await harness.show(tester, size: const Size(1800, 900));
    await settle(tester);
    return harness;
  }

  testWidgets('the panel edge widens it far, narrows it a little, and resets', (
    tester,
  ) async {
    final harness = await showPanel(tester);
    final panel = find.byType(ExtensionPanel);
    expect(tester.getSize(panel).width, panelWidth);
    final edge = find.byKey(const ValueKey('panel-edge'));
    expect(tester.getCenter(edge).dx, 1800 - panelWidth);

    final gesture = await tester.startGesture(tester.getCenter(edge));
    await gesture.moveBy(const Offset(200, 0));
    await tester.pump();
    expect(tester.getSize(panel).width, panelMinWidth);
    await gesture.moveBy(const Offset(-1200, 0));
    await tester.pump();
    expect(tester.getSize(panel).width, panelMaxWidth);
    expect(
      harness.backend.sent.where((message) => message.hasUpdateSettings()),
      isEmpty,
    );
    await gesture.up();
    await settle(tester);
    final updates = [
      for (final message in harness.backend.sent)
        if (message.hasUpdateSettings()) message.updateSettings,
    ];
    expect(updates, hasLength(1));
    expect(updates.single.panelWidth, panelMaxWidth);
    expect(updates.single.hasSidebarWidth(), isFalse);

    await tester.tap(edge);
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tap(edge);
    await settle(tester);
    expect(harness.last.updateSettings.hasPanelWidth(), isTrue);
    expect(harness.last.updateSettings.panelWidth, 0);
    expect(tester.getSize(panel).width, panelWidth);
  });

  testWidgets('the panel never takes the conversation\'s room', (tester) async {
    await showPanel(tester, settings: Settings()..panelWidth = 800);
    expect(tester.getSize(find.byType(ExtensionPanel)).width, 800);
    // 1200 wide: 1200 - 248 sidebar - 40 bar - 440 conversation = 472.
    tester.view.physicalSize = const Size(1200, 900);
    await settle(tester);
    expect(tester.getSize(find.byType(ExtensionPanel)).width, 472);
  });
}
