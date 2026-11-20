// Drives Claktory on an iPhone (the Simulator) end to end: it pairs with
// the pairing code in PAIRING_CODE through the private tunnel, opens an
// agent, sends it a message and waits for its reply.
//
//   cd app && flutter test integration_test/phone_test.dart -d <simulator> \
//     --dart-define=FACTORY_DEV=true --dart-define=PAIRING_CODE='factory://…' \
//     --dart-define=AGENT=Builder [--dart-define=LIFECYCLE=true]
//
// With LIFECYCLE=true, after the reply it waits for the app to be sent to
// the background and brought back (`xcrun simctl launch booted
// com.apple.Preferences`, then `xcrun simctl launch booted
// com.claktory.app.dev`), checks it says nothing about connecting, and
// asks again.
//
// It holds the reply on screen for 20 seconds before it ends. Its output
// arrives only at the end, so for screenshots of each step take them every
// second or two while it runs (`xcrun simctl io booted screenshot …`).
// The phone follows the Simulator's light or dark appearance
// (`xcrun simctl ui booted appearance dark`), as the Mac app follows the Mac.
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _code = String.fromEnvironment('PAIRING_CODE');
const _agent = String.fromEnvironment('AGENT', defaultValue: 'Builder');

/// With LIFECYCLE=true it then waits for the app to go to the background
/// and come back, and talks to the agent again.
const _lifecycle = bool.fromEnvironment('LIFECYCLE');

/// With TOUR=true it then shows each main screen in turn, for screenshots,
/// with TOUR_FILE (a file on the Claktory's machine) open beside the
/// conversation.
const _tour = bool.fromEnvironment('TOUR');
const _tourFile = String.fromEnvironment('TOUR_FILE');

/// Asks the agent to reverse a word, and waits for its reply.
Future<void> askAndWait(WidgetTester tester) async {
  final stamp = DateTime.now().millisecondsSinceEpoch % 100000;
  final word = 'pong$stamp';
  await tester.enterText(
    find.byType(EditableText).last,
    'This is a test from the iPhone app. Reply with just the word $word '
    'reversed, and nothing else.',
  );
  await tester.pump();
  await tester.tap(find.text('Send'));
  final reversed = word.split('').reversed.join();
  // The reply alone (its thinking mentions the word too).
  await waitFor(
    tester,
    find.text(reversed, findRichText: true),
    timeout: const Duration(minutes: 3),
  );
}

/// Pumps until [finder] finds something (or nothing, when [gone]), or
/// fails after [timeout].
Future<void> waitFor(
  WidgetTester tester,
  Finder finder, {
  Duration timeout = const Duration(seconds: 60),
  bool gone = false,
}) async {
  final end = DateTime.now().add(timeout);
  while (DateTime.now().isBefore(end)) {
    await tester.pump(const Duration(milliseconds: 200));
    if (finder.evaluate().isNotEmpty != gone) return;
  }
  fail('Timed out waiting for $finder');
}

/// Holds the screen still for [seconds], for a screenshot.
Future<void> hold(WidgetTester tester, {required int seconds}) async {
  final end = DateTime.now().add(Duration(seconds: seconds));
  while (DateTime.now().isBefore(end)) {
    await tester.pump(const Duration(milliseconds: 250));
    await Future<void>.delayed(const Duration(milliseconds: 250));
  }
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('pairs, opens an agent, and gets its reply', (tester) async {
    expect(_code, isNotEmpty, reason: 'pass --dart-define=PAIRING_CODE=…');
    await app.main();
    await tester.pump(const Duration(seconds: 1));

    // A phone with nothing paired opens to pairing.
    if (find.text('Connect to your Claktory').evaluate().isNotEmpty) {
      if (_tour) await hold(tester, seconds: 5);
      // The Simulator has no camera to scan with: paste it.
      for (var i = 0; i < 50; i++) {
        if (find.byType(EditableText).evaluate().isNotEmpty) break;
        if (find.text('Paste code instead').evaluate().isNotEmpty) {
          await tester.tap(find.text('Paste code instead'));
        }
        await tester.pump(const Duration(milliseconds: 200));
      }
      await tester.enterText(find.byType(EditableText), _code);
      await tester.pump();
      await tester.tap(find.text('Connect'));
    }

    // The sidebar, over the tunnel; first the person's name, if this
    // Claktory doesn't know it yet.
    await waitFor(
      tester,
      find.text(_agent),
      timeout: const Duration(minutes: 1),
    );
    if (find.text("What's your name?").evaluate().isNotEmpty) {
      await tester.enterText(find.byType(EditableText).last, 'Matt');
      await tester.pump();
      await tester.tap(find.text('Continue'));
      await waitFor(tester, find.text("What's your name?"), gone: true);
    }
    await tester.tap(find.text(_agent).first);
    await waitFor(tester, find.text('Send'));
    await askAndWait(tester);

    if (_lifecycle) {
      // Whoever runs this sends the app to the background for a while
      // (another app in front), then back: it reconnects at once, without
      // a word, and the conversation carries on.
      final states = <AppLifecycleState>[];
      final listener = AppLifecycleListener(onStateChange: states.add);
      final end = DateTime.now().add(const Duration(minutes: 3));
      while (DateTime.now().isBefore(end) &&
          !(states.contains(AppLifecycleState.paused) &&
              states.last == AppLifecycleState.resumed)) {
        await tester.pump(const Duration(milliseconds: 200));
        await Future<void>.delayed(const Duration(milliseconds: 200));
      }
      listener.dispose();
      expect(states, contains(AppLifecycleState.paused));
      // ignore: avoid_print
      print('back from the background: $states');
      final quiet = DateTime.now().add(const Duration(seconds: 6));
      while (DateTime.now().isBefore(quiet)) {
        await tester.pump(const Duration(milliseconds: 200));
        expect(find.textContaining("Can't connect"), findsNothing);
        expect(find.textContaining('Connecting'), findsNothing);
      }
      await askAndWait(tester);
    }
    if (_tour) await tour(tester);
    await hold(tester, seconds: 20);
  });
}

/// The main screens, each held still for a screenshot: the conversation,
/// its ⋯ sheet, a file pushed over it, the list, and Settings.
Future<void> tour(WidgetTester tester) async {
  expect(_tourFile, isNotEmpty, reason: 'pass --dart-define=TOUR_FILE=…');
  await hold(tester, seconds: 5);
  final context = tester.element(find.byType(Sidebar).first);
  final scope = AppScope.scope(context);
  final agent = scope.backend.workspace.selectedAgent!;
  await scope.backend.setLayout(
    agent.id,
    ViewNode(
      id: 'tour',
      split: ViewSplit(
        axis: ViewAxis.VIEW_AXIS_ROW,
        children: [
          ViewNode(
            id: 'chat',
            leaf: ViewLeaf(kind: 'chat'),
          ),
          ViewNode(
            id: 'readme',
            leaf: ViewLeaf(
              kind: 'text',
              path: _tourFile,
              title: _tourFile.split('/').last,
            ),
          ),
        ],
      ),
    ),
  );
  await tester.tap(find.bySemanticsLabel('Extensions'));
  await hold(tester, seconds: 5);
  await tester.tap(find.text(_tourFile.split('/').last));
  await hold(tester, seconds: 6);
  await tester.tap(find.bySemanticsLabel('Back').last);
  await hold(tester, seconds: 2);
  await tester.tap(find.bySemanticsLabel('Back').last);
  await hold(tester, seconds: 5);
  scope.page.value = AppPage.settings;
  await hold(tester, seconds: 5);
  await tester.drag(find.byType(Scrollable).first, const Offset(0, -700));
  await hold(tester, seconds: 5);
  scope.page.value = AppPage.agent;
}
