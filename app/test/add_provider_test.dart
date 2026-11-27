// The + after the usage circles in the sidebar's footer (from a friend
// who had one AI provider and couldn't find where to add another): it
// opens Settings at Providers, on a Mac, an iPad and a phone's drawer,
// with one provider or several, and the footer is no taller for it with a
// pointer; with touch it's a finger's target. It stays at the end of the
// circles' first line when they wrap (PS11).
import 'package:factory_app/mobile/drawer.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/usage.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';

Workspace _withProviders(int count) {
  final team = workspace();
  if (count > 1) {
    team.providers.add(
      Provider(id: 'openai-codex', name: 'ChatGPT', signedInWith: 'ChatGPT'),
    );
  }
  return team;
}

Finder get _add => find.bySemanticsLabel(addProviderLabel);

void main() {
  for (final count in [1, 2]) {
    testWidgets('on a Mac with $count provider(s), the + opens Settings at '
        'Providers', (tester) async {
      final harness = Harness(_withProviders(count));
      await harness.show(tester);
      await pumpFrames(tester);
      expect(find.byType(UsageCircle), findsNWidgets(count));
      expect(_add, findsOneWidget);
      // A circle's cell: the row is no taller for it.
      expect(tester.getSize(find.byType(AddProviderButton)).height, 24);
      expect(tester.getSize(find.byType(ProviderUsageCircles)).height, 24);
      await tester.tap(find.byType(AddProviderButton));
      await pumpFrames(tester);
      expect(harness.backend.settingsSection.value, 'providers');
      // Beside the agent shown, as a pane of its layout.
      final layout = harness.backend.sent.lastWhere((m) => m.hasSetLayout());
      expect(layout.setLayout.toString(), contains('settings'));
    });
  }

  testWidgets('on an iPad, the sidebar\'s + opens Settings at Providers', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_withProviders(2));
    await harness.show(tester, size: const Size(1180, 820));
    await pumpFrames(tester);
    expect(_add, findsOneWidget);
    // A finger's target, the row as tall.
    expect(tester.getSize(find.byType(AddProviderButton)), const Size(44, 44));
    expect(tester.getSize(find.byType(ProviderUsageCircles)).height, 44);
    await tester.tap(find.byType(AddProviderButton));
    await pumpFrames(tester);
    expect(harness.backend.settingsSection.value, 'providers');
    expect(tester.takeException(), isNull);
  });

  testWidgets('with no provider yet, the + is there on its own', (
    tester,
  ) async {
    final harness = Harness(workspace(signedIn: false));
    await harness.show(tester);
    await pumpFrames(tester);
    expect(find.byType(UsageCircle), findsNothing);
    expect(_add, findsOneWidget);
  });

  for (final touch in [false, true]) {
    testWidgets('with circles on two lines${touch ? ' and touch' : ''}, the + '
        "stays at the first line's end (projects-sidebar.md §9, PS11)", (
      tester,
    ) async {
      if (touch) {
        debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
      }
      final providers = [
        for (var i = 0; i < 6; i++)
          Provider(id: 'p$i', name: 'P$i', signedInWith: 'P$i'),
      ];
      final harness = Harness(workspace());
      await harness.show(
        tester,
        home: Align(
          alignment: Alignment.topLeft,
          child: SizedBox(
            width: 150,
            child: ProviderUsageCircles(providers: providers, onAdd: () {}),
          ),
        ),
      );
      await pumpFrames(tester);
      final circles = find.byType(UsageCircle);
      final tops = {
        for (final element in circles.evaluate())
          tester.getTopLeft(find.byWidget(element.widget)).dy,
      };
      expect(tops, hasLength(greaterThan(1)), reason: 'they wrap');
      final row = tester.getRect(find.byType(ProviderUsageCircles));
      final add = tester.getRect(find.byType(AddProviderButton));
      expect(add.right, row.right, reason: 'at the right end');
      expect(
        add.center.dy,
        closeTo(tester.getCenter(circles.first).dy, 0.5),
        reason: 'level with the first line',
      );
      for (final element in circles.evaluate()) {
        expect(
          tester.getRect(find.byWidget(element.widget)).right,
          lessThanOrEqualTo(add.left),
          reason: 'no circle under or past it',
        );
      }
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('on a phone, the drawer\'s + opens Settings at Providers', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_withProviders(2));
    await harness.show(tester, size: const Size(320, 568));
    await pumpFrames(tester);
    tester
        .state<PhoneDrawerHostState>(find.byType(PhoneDrawerHost))
        .openDrawer();
    await pumpFrames(tester);
    await tester.scrollUntilVisible(
      find.byType(AddProviderButton),
      200,
      scrollable: find
          .descendant(
            of: find.byType(Sidebar),
            matching: find.byType(Scrollable),
          )
          .first,
    );
    await pumpFrames(tester);
    expect(_add, findsOneWidget);
    expect(tester.getSize(find.byType(AddProviderButton)), const Size(44, 44));
    expect(tester.getSize(find.byType(ProviderUsageCircles)).height, 44);
    await tester.tap(find.byType(AddProviderButton));
    await pumpFrames(tester);
    // Straight to the Providers page, not the list of sections.
    expect(harness.page.value, AppPage.settings);
    expect(find.text('Providers'), findsOneWidget);
    expect(find.text('Profile'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('not while the Claktory doesn\'t allow this device, or before '
      'it ever answered (Zucchero\'s row 216)', (tester) async {
    final harness = Harness(workspace(signedIn: false));
    await harness.show(tester);
    await pumpFrames(tester);
    expect(_add, findsOneWidget);
    harness.backend.refused.value = "mac-mini doesn't allow this device";
    await pumpFrames(tester);
    expect(_add, findsNothing);
    harness.backend.refused.value = null;
    harness.backend.connected.value = false;
    await pumpFrames(tester);
    expect(_add, findsNothing, reason: 'no circles, never answered');
  });

  testWidgets('a moment\'s reconnecting keeps it, with the circles', (
    tester,
  ) async {
    final harness = Harness(_withProviders(1));
    await harness.show(tester);
    await pumpFrames(tester);
    harness.backend.connected.value = false;
    await pumpFrames(tester);
    expect(find.byType(UsageCircle), findsOneWidget);
    expect(_add, findsOneWidget);
  });
}
