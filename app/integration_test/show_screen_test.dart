// An agent's show_screen against a real Claktory (the dev one), on a Mac:
// Rivet (or AGENT) is asked to open Settings → Devices, and this app, used
// just now, opens Settings scrolled to Devices, while Claktory Dev, which
// no one touched, stays as it is. It saves screenshots in SHOTS_DIR
// (/tmp/show-screen unless set). AGENT needs Highlights and a model that
// runs.
//
//   cd app && FLUTTER_XCODE_PRODUCT_BUNDLE_IDENTIFIER=dev.factory.app.itest \
//     flutter test integration_test/show_screen_test.dart -d macos \
//     --dart-define=FACTORY_DEV=true [--dart-define=AGENT=Rivet] \
//     [--dart-define=DIRECT=true]
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _agent = String.fromEnvironment('AGENT', defaultValue: 'Rivet');

/// Hand the app the OpenScreen directly, without asking an agent.
const _direct = bool.fromEnvironment('DIRECT');
const _shots = String.fromEnvironment(
  'SHOTS_DIR',
  defaultValue: '/tmp/show-screen',
);

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  // Frames come from the test, not the screen, so a covered window works.
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.benchmark;

  testWidgets('an agent opens Settings → Devices in the app in use', (
    tester,
  ) async {
    var clock = Duration.zero;
    Future<void> step() async {
      const wait = Duration(milliseconds: 100);
      await Future<void>.delayed(wait);
      clock += wait;
      await tester.pumpBenchmark(clock);
    }

    Future<void> until(
      String what,
      bool Function() done, {
      Duration timeout = const Duration(seconds: 120),
    }) async {
      final end = DateTime.now().add(timeout);
      await step();
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await step();
      }
    }

    Future<void> shot(String name) async {
      for (var i = 0; i < 5; i++) {
        await step();
      }
      final view = binding.renderViews.first;
      final image = await (view.debugLayer! as OffsetLayer).toImage(
        Offset.zero & view.size,
        pixelRatio: view.flutterView.devicePixelRatio,
      );
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      final file = File('$_shots/$name.png');
      await file.create(recursive: true);
      await file.writeAsBytes(bytes!.buffer.asUint8List());
      // ignore: avoid_print
      print('screenshot: ${file.path}');
    }

    await app.main();
    await until('the agents', () => find.text(_agent).evaluate().isNotEmpty);
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    Agent agent() => backend.workspace.workspace.agents.firstWhere(
      (agent) => agent.name == _agent,
    );
    await backend.selectAgent(agent().id);
    scope.page.value = AppPage.agent;
    await step();
    expect(find.byType(SettingsView), findsNothing);

    // The person just used this app: the pointer moves over it.
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(400, 300));
    await mouse.moveTo(const Offset(420, 320));
    await mouse.removePointer();

    // An agent that can run asks; with the dev accounts at their limits,
    // the app is handed the same OpenScreen the backend would send.
    if (agent().usageWait.isNotEmpty) await backend.stopAgent(agent().id);
    final canRun = agent().modelAvailable && !_direct;
    if (canRun) {
      await backend.sendMessage(
        agent().id,
        'Use your show_screen tool to open Settings at the Devices section '
        '(screen "settings", id "devices"). Then reply with only what the '
        'tool said.',
      );
    } else {
      backend.screenRequested(
        OpenScreen(
          id: 'itest',
          target: HighlightTarget(kind: 'settings', id: 'devices'),
          fromAgentId: agent().id,
        ),
      );
    }
    await until(
      'Settings',
      () => find.byType(SettingsView).evaluate().isNotEmpty,
    );
    await until('Devices in view', () {
      final settings = find.byType(SettingsView);
      final title = find.descendant(
        of: settings,
        matching: find.text('Devices'),
      );
      if (title.evaluate().isEmpty) return false;
      final top = tester.getRect(title.first).top;
      return top - tester.getRect(settings).top < 120;
    });
    await shot('1-settings-devices');
    if (!canRun) return;
    await until(
      'the reply',
      () => agent().status != AgentStatus.AGENT_STATUS_WORKING,
    );
    // ignore: avoid_print
    print('agent status: ${agent().status} usage wait: ${agent().usageWait}');
  });
}
