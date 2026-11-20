import 'package:factory_app/forms.dart' show AgentSettingsForm;
import 'package:factory_app/layout.dart' show leaves;
import 'package:factory_app/map.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/screens.dart';
import 'package:factory_app/settings_view.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show snapshot;

/// An agent's show_screen opens a screen in the app the person is using
/// (OpenScreen), and the app answers whether it did (ScreenOpened).
void main() {
  setUp(() => InputClock.setLast(null));

  Future<Harness> showMac(
    WidgetTester tester, {
    Size size = const Size(1200, 900),
  }) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(snapshot());
    harness.backend.appliesLayouts = true;
    await harness.show(tester, size: size);
    await settle(tester);
    return harness;
  }

  /// The person moves the pointer over the window.
  Future<void> touch(WidgetTester tester) async {
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(600, 450));
    await mouse.moveTo(const Offset(610, 460));
    await mouse.removePointer();
    await tester.pump();
  }

  OpenScreen request(String kind, [String id = '']) => OpenScreen(
    id: 'r1',
    target: HighlightTarget(kind: kind, id: id),
    fromAgentId: 'rivet',
  );

  ScreenOpened answer(Harness harness) => harness.backend.sent
      .lastWhere((message) => message.hasScreenOpened())
      .screenOpened;

  List<String> kinds(Harness harness) {
    final agent = harness.backend.workspace.agent('a1')!;
    if (!agent.hasLayout()) return ['chat'];
    return [for (final leaf in leaves(agent.layout)) leaf.leaf.kind];
  }

  testWidgets('Settings opens beside the conversation, scrolled to the '
      'section asked for', (tester) async {
    final harness = await showMac(tester, size: const Size(1200, 500));
    await touch(tester);
    harness.backend.screenRequested(request('settings', 'devices'));
    await tester.pump();
    // Out of sight at first.
    expect(find.text('Devices'), findsNothing);
    await pumpFrames(tester);

    expect(answer(harness).id, 'r1');
    expect(answer(harness).opened, isTrue);
    expect(kinds(harness), ['chat', 'settings']);
    // Devices is far down; its title is now at the top of Settings.
    final settings = tester.getRect(find.byType(SettingsView));
    final title = tester.getRect(find.text('Devices'));
    expect(title.top, greaterThanOrEqualTo(settings.top));
    expect(title.top, lessThan(settings.top + 120));
    expect(harness.backend.settingsSection.value, isNull);
  });

  testWidgets('the Map opens as a pane', (tester) async {
    final harness = await showMac(tester);
    await touch(tester);
    harness.backend.screenRequested(request('map'));
    await settle(tester);
    expect(answer(harness).opened, isTrue);
    expect(kinds(harness), ['chat', 'map']);
    expect(find.byType(MapView), findsOneWidget);
  });

  testWidgets('an agent\'s conversation opens, and its settings', (
    tester,
  ) async {
    final harness = await showMac(tester);
    await touch(tester);
    harness.backend.screenRequested(request('agent', 'rivet'));
    await settle(tester);
    expect(answer(harness).opened, isTrue);
    expect(
      harness.backend.sent.any((m) => m.selectAgent.agentId == 'rivet'),
      isTrue,
    );

    harness.backend.screenRequested(request('agent_settings', 'a1'));
    await settle(tester);
    expect(answer(harness).opened, isTrue);
    expect(find.byType(AgentSettingsForm), findsOneWidget);
    // Asked again while it's up: it stays the one form.
    harness.backend.screenRequested(request('agent_settings', 'a1'));
    await settle(tester);
    expect(find.byType(AgentSettingsForm), findsOneWidget);

    // An agent that isn't there, or a screen this app doesn't know.
    harness.backend.screenRequested(request('agent', 'gone'));
    expect(answer(harness).opened, isFalse);
    harness.backend.screenRequested(request('tasks'));
    expect(answer(harness).opened, isFalse);
  });

  testWidgets('nothing opens in an app the person hasn\'t used lately', (
    tester,
  ) async {
    final harness = await showMac(tester);
    harness.backend.screenRequested(request('settings', 'devices'));
    await settle(tester);
    expect(answer(harness).opened, isFalse);
    expect(kinds(harness), ['chat']);

    // Used, but six minutes ago: that's not lately.
    InputClock.setLast(DateTime.now().subtract(const Duration(minutes: 6)));
    harness.backend.screenRequested(request('map'));
    await settle(tester);
    expect(answer(harness).opened, isFalse);

    await touch(tester);
    harness.backend.screenRequested(request('map'));
    await settle(tester);
    expect(answer(harness).opened, isTrue);
  });

  testWidgets('nothing opens while the window is hidden; a Mac in the '
      'background still opens it', (tester) async {
    final harness = await showMac(tester);
    await touch(tester);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    harness.backend.screenRequested(request('map'));
    await settle(tester);
    expect(answer(harness).opened, isFalse);

    // Back on screen behind another app.
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    harness.backend.screenRequested(request('map'));
    await settle(tester);
    expect(answer(harness).opened, isTrue);
  });

  testWidgets('nothing opens under a first-launch step', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(snapshot(), named: false);
    await harness.show(tester);
    await settle(tester);
    await touch(tester);
    harness.backend.screenRequested(request('map'));
    await settle(tester);
    expect(answer(harness).opened, isFalse);
  });

  testWidgets('with no window showing this Claktory, the answer is no', (
    tester,
  ) async {
    final harness = Harness(snapshot());
    harness.backend.screenRequested(request('map'));
    expect(answer(harness).opened, isFalse);
  });

  testWidgets('on a phone an agent\'s conversation is pushed over the list', (
    tester,
  ) async {
    final harness = Harness(snapshot());
    harness.backend.onSend = (message) {
      if (!message.hasSelectAgent()) return;
      harness.backend.workspace.apply(
        harness.backend.workspace.workspace.deepCopy()
          ..selectedAgentId = message.selectAgent.agentId,
      );
    };
    await harness.show(tester, size: const Size(390, 844));
    await settle(tester);
    await touch(tester);
    expect(find.text('Message Rivet'), findsNothing);
    harness.backend.screenRequested(request('agent', 'rivet'));
    await pumpFrames(tester);
    expect(answer(harness).opened, isTrue);
    expect(find.text('Message Rivet'), findsOneWidget);
  });
}
