// The person's avatar against a real Claktory (the dev one), on an iPhone
// (the Simulator) already paired with it, or paired with PAIRING_CODE:
//
//   1. Workspace.person has initials and a gradient, and the avatar shows
//      in the sidebar's footer, beside the person's messages (AGENT's
//      conversation) and on the Map.
//   2. Shuffle colors in Settings → Profile gives it new colors, shown
//      everywhere at once. It prints them.
//   3. With EXPECT_FROM (what step 2 printed, after restarting the
//      backend), only checks the colors stayed.
//
//   cd app && flutter test integration_test/avatar_test.dart \
//     -d <simulator> --dart-define=FACTORY_DEV=true --dart-define=AGENT=Wren
import 'package:factory_app/avatar.dart';
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'phone_test.dart' show hold, waitFor;

const _code = String.fromEnvironment('PAIRING_CODE');
const _agent = String.fromEnvironment('AGENT', defaultValue: 'Wren');
const _expectFrom = String.fromEnvironment('EXPECT_FROM');

/// Holds the screen still at each step, for screenshots.
const _shots = bool.fromEnvironment('SHOTS');

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets("the person's avatar, everywhere, and Shuffle colors", (
    tester,
  ) async {
    await app.main();
    await tester.pump(const Duration(seconds: 1));
    if (find.text('Connect to your Claktory').evaluate().isNotEmpty) {
      expect(_code, isNotEmpty, reason: 'pass --dart-define=PAIRING_CODE=…');
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
      await tester.pump(const Duration(milliseconds: 500));
      // A code asks first: the confirmation's Connect.
      await tester.tap(find.text('Connect').last);
    }
    await waitFor(
      tester,
      find.byType(Sidebar),
      timeout: const Duration(minutes: 1),
    );
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    Person person() => backend.workspace.workspace.person;
    Future<void> until(
      String what,
      bool Function() done, {
      Duration timeout = const Duration(seconds: 30),
    }) async {
      final end = DateTime.now().add(timeout);
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await tester.pump(const Duration(milliseconds: 200));
      }
    }

    await until('connected', () => backend.connected.value);
    await until('the person', () => backend.workspace.workspace.hasPerson());
    // A Claktory that doesn't know the name yet asks first.
    await tester.pump(const Duration(seconds: 1));
    if (find.text("What's your name?").evaluate().isNotEmpty) {
      await tester.enterText(find.byType(EditableText).last, 'Matt Smith');
      await tester.pump();
      await tester.tap(find.text('Continue'));
      await waitFor(tester, find.text("What's your name?"), gone: true);
      await until('initials', () => person().initials == 'MS');
    }
    // ignore: avoid_print
    print(
      'person: "${person().name}" ${person().initials} '
      '${person().avatarFrom} → ${person().avatarTo}',
    );
    if (_expectFrom.isNotEmpty) {
      expect(person().avatarFrom, _expectFrom, reason: 'kept across restarts');
      return;
    }
    expect(hexColor(person().avatarFrom), isNotNull);
    expect(hexColor(person().avatarTo), isNotNull);
    if (backend.workspace.workspace.settings.userName.isNotEmpty) {
      expect(person().initials, isNotEmpty);
    }

    Iterable<Color> shown() => find.byType(PersonAvatar).evaluate().map((e) {
      final circle =
          find
                  .descendant(
                    of: find.byWidget(e.widget),
                    matching: find.byType(Container),
                  )
                  .evaluate()
                  .first
                  .widget
              as Container;
      return ((circle.decoration! as BoxDecoration).gradient! as LinearGradient)
          .colors
          .first;
    });

    // 1. The footer, the conversation, the Map.
    expect(find.byType(PersonAvatar), findsWidgets, reason: 'the footer');
    await waitFor(tester, find.text(_agent));
    await tester.tap(find.text(_agent).first);
    await hold(tester, seconds: _shots ? 5 : 2);
    expect(
      find.byType(PersonAvatar),
      findsWidgets,
      reason: "beside the person's messages",
    );
    scope.page.value = AppPage.map;
    await hold(tester, seconds: _shots ? 5 : 2);
    expect(find.text('You'), findsOneWidget);
    expect(find.byType(PersonAvatar), findsWidgets);

    // 2. Shuffle colors.
    scope.page.value = AppPage.settings;
    await hold(tester, seconds: _shots ? 5 : 2);
    final before = person().avatarFrom;
    await tester.tap(find.text('Shuffle colors'));
    await until('new colors', () => person().avatarFrom != before);
    await tester.pump(const Duration(milliseconds: 300));
    final now = hexColor(person().avatarFrom)!;
    expect(shown(), everyElement(now));
    await hold(tester, seconds: _shots ? 5 : 1);
    scope.page.value = AppPage.agent;
    await hold(tester, seconds: 1);
    expect(shown(), everyElement(now));
    // ignore: avoid_print
    print('shuffled to ${person().avatarFrom} → ${person().avatarTo}');
  });
}
