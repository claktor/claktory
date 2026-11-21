// Needs you, against a real Claktory (the dev one), on a Mac: an agent asks
// for a secret with its tool, and while the request waits
//
//   1. the agent's sidebar row shows the orange count, in dark and light,
//      and its Claktory's tile in the rail an orange dot;
//   2. clicking the agent opens its conversation at the request's card;
//   3. dismissing the request takes the count away.
//
// It saves a screenshot of the window at each step in SHOTS_DIR
// (/tmp/needs-you unless set). The agent's model runs for real.
//
// The test drives frames itself, so it runs with its window in the
// background (macOS draws nothing for a window that's covered):
//
//   cd app && FLUTTER_XCODE_PRODUCT_BUNDLE_IDENTIFIER=dev.factory.app.itest \
//     flutter test integration_test/needs_you_test.dart -d macos \
//     --dart-define=FACTORY_DEV=true [--dart-define=AGENT=Wren] \
//     [--dart-define=ELSEWHERE=Builder]
//
// (Its own bundle id, so it never shares the real Claktory's Keychain.)
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/secrets.dart';
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/transcript.dart';
import 'package:factory_app/waiting.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _agent = String.fromEnvironment('AGENT', defaultValue: 'Wren');
const _elsewhere = String.fromEnvironment('ELSEWHERE', defaultValue: 'Builder');
const _shots = String.fromEnvironment(
  'SHOTS_DIR',
  defaultValue: '/tmp/needs-you',
);

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  // Frames come from the test, not the screen.
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.benchmark;

  testWidgets('a request waiting for the person shows, opens and goes', (
    tester,
  ) async {
    var clock = Duration.zero;

    /// Lets [wait] pass, then draws a frame.
    Future<void> step([
      Duration wait = const Duration(milliseconds: 100),
    ]) async {
      await Future<void>.delayed(wait);
      clock += wait;
      await tester.pumpBenchmark(clock);
    }

    Future<void> until(
      String what,
      bool Function() done, {
      Duration timeout = const Duration(seconds: 30),
    }) async {
      final end = DateTime.now().add(timeout);
      await step();
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await step();
      }
    }

    Future<void> hold(Duration time) async {
      final end = DateTime.now().add(time);
      while (DateTime.now().isBefore(end)) {
        await step();
      }
    }

    /// The whole window as it is now, as a PNG in [_shots].
    Future<void> shot(String name) async {
      await hold(const Duration(milliseconds: 500));
      final view = binding.renderViews.first;
      final layer = view.debugLayer! as OffsetLayer;
      // The view's layer is already scaled to physical pixels.
      final image = await layer.toImage(
        Offset.zero & view.size * view.flutterView.devicePixelRatio,
      );
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      final file = File('$_shots/$name.png');
      await file.create(recursive: true);
      await file.writeAsBytes(bytes!.buffer.asUint8List());
      // ignore: avoid_print
      print('screenshot: ${file.path}');
    }

    await app.main();
    await until(
      'the agents',
      () => find.text(_agent).evaluate().isNotEmpty,
      timeout: const Duration(minutes: 1),
    );
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    Workspace workspace() => backend.workspace.workspace;
    Agent named(String name) =>
        workspace().agents.firstWhere((agent) => agent.name == name);
    final agent = named(_agent);

    // One left waiting (by a run that stopped early, say) will do;
    // otherwise the agent asks.
    if (named(_agent).waitingSeqs.isEmpty) {
      // Somewhere else while it asks.
      await backend.selectAgent(named(_elsewhere).id);
      final stamp = DateTime.now().millisecondsSinceEpoch % 100000;
      await backend.sendMessage(
        agent.id,
        'This is a test of the app. Call your secrets_request tool once, '
        'right now, for a new secret named NEEDS_YOU_$stamp with one '
        'variable, NEEDS_YOU_$stamp, described as "A test secret; grants '
        'nothing", with the reason "Testing how the app shows a waiting '
        'request". Then stop; don\'t do anything else.',
      );
      await until(
        'the request',
        () => named(_agent).waitingSeqs.isNotEmpty,
        timeout: const Duration(minutes: 3),
      );
    } else {
      await backend.selectAgent(named(_elsewhere).id);
    }
    final waiting = named(_agent).waitingSeqs.length;

    // 1. The count on its row, and the dot on the rail, in both themes.
    Finder labeled(String text) => find.byWidgetPredicate(
      (widget) => widget is Semantics && widget.properties.label == text,
    );
    await until(
      'the count on its row',
      () => labeled(waitingLabel(_agent, waiting)).evaluate().isNotEmpty,
    );
    expect(
      find
          .descendant(
            of: find.byType(ClaktoryRail),
            matching: find.byType(Semantics),
          )
          .evaluate()
          .where(
            (element) => ((element.widget as Semantics).properties.label ?? '')
                .endsWith('waiting for your answer'),
          ),
      isNotEmpty,
      reason: "the rail's tile",
    );
    for (final brightness in [ui.Brightness.dark, ui.Brightness.light]) {
      binding.platformDispatcher.platformBrightnessTestValue = brightness;
      await shot('1-waiting-${brightness.name}');
    }
    binding.platformDispatcher.clearPlatformBrightnessTestValue();

    // 2. Clicking it opens its conversation at the card.
    await tester.tap(
      find.descendant(of: find.byType(Sidebar), matching: find.text(_agent)),
    );
    await until(
      'its conversation',
      () =>
          workspace().selectedAgentId == agent.id &&
          backend.transcript.agentId == agent.id &&
          find.byType(SecretRequestCard).evaluate().isNotEmpty,
    );
    await hold(const Duration(seconds: 1));
    // Earlier runs' cards, answered, may sit above it.
    final card = find
        .byWidgetPredicate(
          (widget) =>
              widget is SecretRequestCard &&
              widget.request.state ==
                  SecretRequestState.SECRET_REQUEST_STATE_PENDING,
        )
        .first;
    final view = tester.getRect(find.byType(TranscriptView));
    final top = tester.getRect(card).top;
    // ignore: avoid_print
    print('card at ${top - view.top}px from the top of ${view.height}px');
    expect(top, greaterThanOrEqualTo(view.top - 1));
    expect(top, lessThan(view.top + view.height / 3), reason: 'at the top');
    expect(backend.reveal.value, isNull);
    await shot('2-opened');

    // 3. Answered, the count goes.
    await tester.tap(find.descendant(of: card, matching: find.text('Dismiss')));
    await until('answered', () => named(_agent).waitingSeqs.length < waiting);
    await until(
      'the count gone',
      () => waiting > 1 || find.byType(NeedsYouMark).evaluate().isEmpty,
    );
    await shot('3-answered');
  });
}
