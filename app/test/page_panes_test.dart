import 'package:factory_app/changelog.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/layout.dart' show leaves;
import 'package:factory_app/map.dart';
import 'package:factory_app/marketplace.dart';
import 'package:factory_app/panes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, glyphButton, snapshot;

/// Settings, the Marketplace and What's new open as panes beside the
/// selected agent's conversation on a Mac, as the backend keeps them in its
/// layout; on a phone, or with no agent, they're pages of their own.
void main() {
  Future<Harness> showWith(WidgetTester tester, {ViewNode? layout}) async {
    final harness = Harness(snapshot(layout: layout));
    harness.backend.appliesLayouts = true;
    await harness.show(tester);
    await settle(tester);
    return harness;
  }

  List<String> kinds(Harness harness) {
    final agent = harness.backend.workspace.agent('a1')!;
    if (!agent.hasLayout()) return ['chat'];
    return [for (final leaf in leaves(agent.layout)) leaf.leaf.kind];
  }

  bool active(String label) => find
      .byWidgetPredicate(
        (w) => w is GlyphButton && w.label == label && w.active,
      )
      .evaluate()
      .isNotEmpty;

  testWidgets('the cog opens Settings right of the conversation, and closes '
      'it again', (tester) async {
    final harness = await showWith(tester);
    await tester.tap(glyphButton('Settings'));
    await settle(tester);

    final sent = harness.last.setLayout;
    expect(sent.agentId, 'a1');
    expect(sent.root.split.axis, ViewAxis.VIEW_AXIS_ROW);
    expect(kinds(harness), ['chat', 'settings']);
    expect(sent.root.split.children.last.leaf.path, isEmpty);
    // Both there, side by side; the page is the agent view still.
    expect(harness.page.value, AppPage.agent);
    expect(find.text('Conversation'), findsOneWidget);
    expect(find.byType(SettingsView), findsOneWidget);
    expect(find.text('Providers'), findsOneWidget);
    final chat = tester.getRect(find.byKey(const ValueKey('pane-chat')));
    final settings = tester.getRect(find.byType(SettingsView));
    expect(settings.left, greaterThan(chat.right));
    // The pane's header names it and closes it: no bar of its own.
    expect(find.text('Done'), findsNothing);
    expect(active('Settings'), isTrue);

    // Again, and it goes.
    await tester.tap(glyphButton('Settings'));
    await settle(tester);
    expect(harness.last.setLayout.hasRoot(), isFalse);
    expect(find.byType(SettingsView), findsNothing);
    expect(active('Settings'), isFalse);
  });

  testWidgets('opened again from elsewhere, it glows rather than opening a '
      'second one', (tester) async {
    final harness = await showWith(tester);
    await tester.tap(glyphButton('Settings'));
    await settle(tester);
    final sent = harness.backend.sent.length;
    AppScope.openPage(
      tester.element(find.byType(Sidebar).first),
      AppPage.settings,
    );
    await settle(tester);
    expect(harness.backend.sent.length, sent);
    expect(kinds(harness), ['chat', 'settings']);
    // The glow fades on its own.
    await tester.pump(const Duration(seconds: 10));
  });

  testWidgets("beside an open file it goes next to the conversation; its × "
      'closes it', (tester) async {
    final harness = await showWith(tester, layout: beside());
    await tester.tap(glyphButton("What's new"));
    await settle(tester);
    expect(kinds(harness), ['chat', 'changelog', 'text']);
    expect(find.byType(ChangelogView), findsOneWidget);
    expect(find.text("What's new"), findsWidgets);

    await tester.tap(glyphButton('Marketplace'));
    await settle(tester);
    expect(kinds(harness), ['chat', 'marketplace', 'changelog', 'text']);
    // Its tabs stay, under the pane's header.
    expect(find.byType(MarketplaceView), findsOneWidget);
    expect(find.text('Skills'), findsOneWidget);

    final close = find.descendant(
      of: find.ancestor(
        of: find.byType(MarketplaceView),
        matching: find.byType(Pane),
      ),
      matching: glyphButton('Close'),
    );
    await tester.tap(close);
    await settle(tester);
    expect(kinds(harness), ['chat', 'changelog', 'text']);
  });

  testWidgets('with no agent selected, Settings is a page of its own', (
    tester,
  ) async {
    final workspace = snapshot()..selectedAgentId = '';
    final harness = Harness(workspace);
    harness.backend.appliesLayouts = true;
    await harness.show(tester);
    await settle(tester);
    await tester.tap(glyphButton('Settings'));
    await settle(tester);
    expect(harness.page.value, AppPage.settings);
    expect(find.text('Done'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
  });

  testWidgets('the Map opens beside the conversation as a canvas of its '
      'own, and closes again', (tester) async {
    final harness = await showWith(tester);
    await tester.tap(glyphButton('Map'));
    await settle(tester);
    expect(kinds(harness), ['chat', 'map']);
    expect(find.byType(MapView), findsOneWidget);
    // No sideways scroll around it, and no bar of its own.
    expect(
      find.ancestor(
        of: find.byType(MapView),
        matching: find.byType(SingleChildScrollView),
      ),
      findsNothing,
    );
    expect(find.text('Done'), findsNothing);
    expect(active('Map'), isTrue);

    await tester.tap(glyphButton('Map'));
    await settle(tester);
    expect(find.byType(MapView), findsNothing);
    expect(active('Map'), isFalse);
  });
}
