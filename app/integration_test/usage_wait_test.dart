// An agent waiting out its usage limits, against a real Claktory (the dev
// one) whose accounts are all at their limit, on a Mac: messaged, the agent
// waits instead of failing, and the app says what it waits for, with the
// time in this device's own time zone, on its row and above its message
// box, without the working wave. Stop ends the wait. It saves screenshots
// in SHOTS_DIR (/tmp/usage-wait unless set).
//
//   cd app && FLUTTER_XCODE_PRODUCT_BUNDLE_IDENTIFIER=dev.factory.app.itest \
//     flutter test integration_test/usage_wait_test.dart -d macos \
//     --dart-define=FACTORY_DEV=true [--dart-define=AGENT=Wren]
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/agent_view.dart' show WorkingLine;
import 'package:factory_app/controls.dart' show WorkingWave;
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/usage_wait.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _agent = String.fromEnvironment('AGENT', defaultValue: 'Wren');
const _shots = String.fromEnvironment(
  'SHOTS_DIR',
  defaultValue: '/tmp/usage-wait',
);

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  // Frames come from the test, not the screen, so a covered window works.
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.benchmark;

  testWidgets('an agent waiting out its usage limits says until when', (
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
      Duration timeout = const Duration(seconds: 60),
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
    // Already waiting (an event woke it, say), it needs no message.
    if (agent().usageWait.isEmpty) {
      await backend.sendMessage(agent().id, 'Reply with only the word ok.');
    }
    await until('the wait', () => agent().usageWait.isNotEmpty);
    final line = usageWaitLine(agent());
    // ignore: avoid_print
    print('backend: ${agent().usageWait}\napp:     $line');
    await until(
      'the wait on its row',
      () => find
          .descendant(of: find.byType(Sidebar), matching: find.text(line))
          .evaluate()
          .isNotEmpty,
    );
    expect(
      find.descendant(of: find.byType(WorkingLine), matching: find.text(line)),
      findsOneWidget,
    );
    expect(atWork(agent()), isFalse);
    expect(
      find.descendant(
        of: find.byType(Sidebar),
        matching: find.byType(WorkingWave),
      ),
      findsNothing,
    );
    await shot('1-on-hold');

    await backend.stopAgent(agent().id);
    await until(
      'the wait over',
      () => agent().status != AgentStatus.AGENT_STATUS_WORKING,
    );
  });
}
