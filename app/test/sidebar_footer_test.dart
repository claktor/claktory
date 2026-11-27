// The sidebar's foot on a Mac and an iPad: Settings and More, each with its
// label, then the person. More holds the other pages, the shown one checked,
// What's new after a hairline; what glows inside it glows on More; ⌘, opens
// Settings.
import 'dart:async';

import 'package:factory_app/controls.dart' show GlyphButton, showModal;
import 'package:factory_app/highlights.dart' show Glow;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart' show Text;
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Site', path: '/w/site')],
  agents: [Agent(id: 'a1', name: 'Ana', projectId: 'p1', modelAvailable: true)],
  selectedAgentId: 'a1',
);

Finder _button(String label) => find.byWidgetPredicate(
  (widget) => widget is GlyphButton && widget.label == label,
);

void main() {
  testWidgets('Settings and More, labelled; the pages are in More, What\'s '
      'new last, the one shown checked', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace());
    harness.backend.appliesLayouts = true;
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    expect(_button('Settings'), findsOneWidget);
    expect(_button('More'), findsOneWidget);
    for (final gone in ['Usage', 'Monitors', 'Map', 'Marketplace']) {
      expect(_button(gone), findsNothing, reason: gone);
    }
    // Labelled, at the narrowest sidebar too: nothing cut.
    expect(find.text('Settings'), findsOneWidget);
    expect(find.text('More'), findsOneWidget);

    await tester.tap(_button('More'));
    await settle(tester);
    final rows = ['Usage', 'Monitors', 'Map', 'Marketplace', "What's new"];
    for (final row in rows) {
      expect(find.text(row), findsOneWidget, reason: row);
    }
    expect(find.text('People'), findsNothing, reason: 'not offered here');
    final tops = [for (final row in rows) tester.getTopLeft(find.text(row)).dy];
    expect(tops, [...tops]..sort());
    // Above the button, as a menu from the foot of a window opens.
    expect(
      tester.getBottomLeft(find.text("What's new")).dy,
      lessThanOrEqualTo(tester.getTopLeft(_button('More')).dy + 1),
    );
    await tester.tap(find.text('Map'));
    await settle(tester);
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isNotEmpty);
    expect(tester.widget<GlyphButton>(_button('More')).active, isTrue);

    await tester.tap(_button('More'));
    await settle(tester);
    expect(find.text('✓'), findsOneWidget);
    expect(
      tester.getCenter(find.text('✓')).dy,
      closeTo(tester.getCenter(find.text('Map').last).dy, 2),
    );
  });

  testWidgets("what glows inside More glows on More", (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    final glow = tester.widget<Glow>(
      find.ancestor(of: _button('More'), matching: find.byType(Glow)).first,
    );
    for (final key in [
      'marketplace:',
      'changelog:',
      'map:',
      'usage:',
      'monitors:',
      'people:',
    ]) {
      expect(glow.matches(key), isTrue, reason: key);
    }
    expect(glow.matches('settings:'), isFalse);
  });

  testWidgets('⌘, opens Settings on a Mac', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace()..selectedAgentId = '');
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.comma);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    expect(harness.page.value, AppPage.settings);
    // Not drawn here: the page asks this Mac's kind of the system.
    harness.page.value = AppPage.agent;
    await settle(tester);

    // Not from a dialog over the window.
    unawaited(
      showModal(
        tester.element(find.byType(Sidebar)),
        (context, close) => const Text('A dialog'),
      ),
    );
    await settle(tester);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.comma);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    expect(harness.page.value, AppPage.agent);
  });
}
