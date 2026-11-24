import 'package:factory_app/identity.dart' show Space;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/usage.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'usage_test.dart' show later, weekly;

/// Anthropic signed in to two subscriptions and an API key, in that order.
Provider claude({List<Account>? accounts}) =>
    anthropic(signedInWith: 'Anthropic (Claude Pro/Max)', canSignOut: true)
      ..plan = 'Claude Max 20x'
      ..usage = weekly(100)
      // Agents run on it (one account is in use).
      ..agents = 2
      ..accounts.addAll(
        accounts ??
            [
              Account(
                id: 'a1',
                label: 'matt@work.test',
                plan: 'Claude Max 20x',
                usage: weekly(100),
                canSignOut: true,
                exhaustedUntil: later(const Duration(hours: 3)),
              ),
              Account(
                id: 'a2',
                label: 'matt@home.test',
                plan: 'Claude Pro',
                usage: weekly(25),
                canSignOut: true,
                inUse: true,
              ),
              Account(
                id: 'k1',
                label: 'sk-…a1b2',
                plan: 'API key',
                methodId: 'api_key',
                canSignOut: true,
              ),
            ],
      );

Future<Harness> showSettings(
  WidgetTester tester,
  Provider provider, {
  LoginFlow? login,
  // Tall enough for Profile, every account and where the models come from.
  Size size = const Size(1200, 1200),
}) async {
  final snapshot = workspace(login: login)
    ..providers.clear()
    ..providers.add(provider);
  final harness = Harness(snapshot);
  await harness.show(tester, size: size);
  harness.page.value = AppPage.settings;
  await settle(tester);
  return harness;
}

/// The grips that drag accounts, in the order of the rows.
final grips = find.byWidgetPredicate(
  (w) => w is Semantics && w.properties.label == 'Drag to reorder',
);

void main() {
  testWidgets('each account shows who it is, its plan, usage and state', (
    tester,
  ) async {
    await showSettings(tester, claude());
    expect(grips, findsNWidgets(3));
    expect(find.text('matt@work.test'), findsOneWidget);
    expect(
      find.textContaining('Claude Max 20x · Limit reached, back at '),
      findsOneWidget,
    );
    expect(
      find.text('Claude Pro · In use · 25% of the weekly limit used'),
      findsOneWidget,
    );
    expect(find.text('sk-…a1b2'), findsOneWidget);
    expect(find.text('API key · Signed in'), findsOneWidget);
    expect(
      find.text(
        'When one account reaches its limit, Claktory switches to '
        'the next. When all have, agents wait until one resets.',
      ),
      findsOneWidget,
    );
    // In the order requests try them.
    double top(String text) => tester.getTopLeft(find.text(text)).dy;
    expect(top('matt@work.test'), lessThan(top('matt@home.test')));
    expect(top('matt@home.test'), lessThan(top('sk-…a1b2')));
    expect(find.textContaining('Sign in with'), findsNothing);
  });

  testWidgets('adding an account never replaces one', (tester) async {
    final harness = await showSettings(tester, claude());
    await tester.tap(find.text('Add another Claude subscription'));
    await settle(tester);
    expect(harness.last.startLogin.methodId, 'oauth');
    expect(harness.last.startLogin.addAccount, isTrue);

    await tester.ensureVisible(find.text('Add an API key'));
    await settle(tester);
    await tester.tap(find.text('Add an API key'));
    await settle(tester);
    expect(harness.last.startLogin.methodId, 'api_key');
    expect(harness.last.startLogin.addAccount, isTrue);
  });

  testWidgets('trying a failed addition again still adds', (tester) async {
    final harness = await showSettings(tester, claude());
    await tester.tap(find.text('Add another Claude subscription'));
    await settle(tester);
    final failed =
        workspace(
            login: LoginFlow(
              providerId: 'anthropic',
              methodId: 'oauth',
              error: 'Denied.',
            ),
          )
          ..providers.clear()
          ..providers.add(claude());
    await harness.apply(tester, failed);
    await tester.tap(find.text('Try again'));
    await settle(tester);
    expect(harness.last.startLogin.addAccount, isTrue);
  });

  testWidgets('a provider with no account is in the picker, to sign in', (
    tester,
  ) async {
    final harness = await showSettings(tester, anthropic());
    // No card for it: only the picker's list.
    expect(find.text('Not signed in'), findsNothing);
    expect(find.text('Add an API key'), findsNothing);
    await tester.tap(find.text('Anthropic (Claude Pro/Max)'));
    await settle(tester);
    expect(harness.last.startLogin.providerId, 'anthropic');
    expect(harness.last.startLogin.addAccount, isFalse);
  });

  testWidgets('one account has no grip, no Use this one and no hint', (
    tester,
  ) async {
    await showSettings(
      tester,
      claude(
        accounts: [
          Account(
            id: 'a1',
            label: 'matt@work.test',
            plan: 'Claude Max 20x',
            usage: weekly(10),
            canSignOut: true,
            inUse: true,
          ),
        ],
      ),
    );
    expect(grips, findsNothing);
    expect(find.text('Use this one'), findsNothing);
    expect(find.textContaining('switches to the next'), findsNothing);
    expect(find.text('Sign out'), findsOneWidget);
  });

  testWidgets('each account signs out on its own', (tester) async {
    final harness = await showSettings(tester, claude());
    expect(find.text('Sign out'), findsNWidgets(3));
    expect(find.text('Rename'), findsOneWidget, reason: 'only the key');
    await tester.tap(find.text('Sign out').at(1));
    await settle(tester);
    expect(harness.last.signOut.providerId, 'anthropic');
    expect(harness.last.signOut.accountId, 'a2');
  });

  testWidgets('Use this one moves an account to the front', (tester) async {
    final harness = await showSettings(tester, claude());
    expect(
      find.text('Use this one'),
      findsNWidgets(2),
      reason: 'not the first',
    );
    await tester.tap(find.text('Use this one').last);
    await settle(tester);
    expect(harness.last.moveAccount.accountId, 'k1');
    expect(harness.last.moveAccount.position, 0);
  });

  testWidgets('on a phone the actions sit under the account, each on one '
      'line, and its email reads whole', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    await showSettings(tester, claude(), size: const Size(375, 667));
    await pumpFrames(tester, const Duration(seconds: 1));
    await tester.tap(find.text('Providers').first);
    await pumpFrames(tester, const Duration(seconds: 1));
    final use = find.text('Use this one').first;
    await tester.ensureVisible(use);
    await settle(tester);
    // One line, not wrapped in a narrow column beside the name.
    expect(tester.getSize(use).height, lessThan(30));
    final email = find.text('matt@home.test');
    expect(
      tester.getTopLeft(use).dy,
      greaterThan(tester.getBottomLeft(email).dy),
    );
    // Its button starts under the name (the label inset in its button).
    expect(
      tester.getTopLeft(use).dx - tester.getTopLeft(email).dx,
      inInclusiveRange(0, Space.xl),
    );
  });

  testWidgets('dragging an account changes the order', (tester) async {
    final harness = await showSettings(tester, claude());
    final from = tester.getCenter(grips.at(0));
    final to = tester.getCenter(grips.at(2));
    final drag = await tester.startGesture(from);
    await tester.pump(const Duration(milliseconds: 50));
    for (var i = 1; i <= 10; i++) {
      await drag.moveTo(Offset.lerp(from, to + const Offset(0, 8), i / 10)!);
      await tester.pump(const Duration(milliseconds: 50));
    }
    await drag.up();
    await pumpFrames(tester);
    expect(harness.last.moveAccount.accountId, 'a1');
    expect(harness.last.moveAccount.position, 2);
  });

  testWidgets('a refusal shows under the provider', (tester) async {
    final harness = await showSettings(tester, claude());
    harness.backend.refuse = 'Claktory is busy with that account.';
    await tester.tap(find.text('Use this one').first);
    await settle(tester);
    expect(find.text('Claktory is busy with that account.'), findsOneWidget);
  });

  Provider withKeys() => claude(
    accounts: [
      Account(
        id: 'a1',
        label: 'matt@work.test',
        plan: 'Claude Max 20x',
        methodId: 'oauth',
        usage: weekly(10),
        canSignOut: true,
        inUse: true,
      ),
      Account(
        id: 'k1',
        label: 'sk-…a1b2',
        plan: 'API key',
        methodId: 'api_key',
        canSignOut: true,
        outOfCredit: true,
      ),
      Account(
        id: 'env',
        label: 'ANTHROPIC_API_KEY',
        plan: 'API key',
        methodId: 'api_key',
      ),
    ],
  );

  testWidgets('a key out of credit says where to add it, and tries again', (
    tester,
  ) async {
    final harness = await showSettings(tester, withKeys());
    expect(find.text('Out of credit. Add credit at '), findsOneWidget);
    await tester.tap(find.text('Anthropic').last);
    await settle(tester);
    expect(harness.opened, ['https://console.anthropic.com/settings/billing']);

    await tester.tap(find.text('Try again'));
    await settle(tester);
    expect(harness.last.updateAccount.accountId, 'k1');
    expect(harness.last.updateAccount.retry, isTrue);
    expect(harness.last.updateAccount.hasLabel(), isFalse);
  });

  testWidgets('a key from the environment stays put and last', (tester) async {
    await showSettings(tester, withKeys());
    expect(find.text('ANTHROPIC_API_KEY'), findsOneWidget);
    expect(grips, findsNWidgets(2), reason: 'not the environment key');
    expect(find.text('Use this one'), findsOneWidget, reason: 'only k1');
    expect(find.text('Sign out'), findsNWidgets(2));
    expect(find.text('Rename'), findsOneWidget);
  });

  testWidgets('a key is renamed in place; Esc keeps its name', (tester) async {
    final harness = await showSettings(tester, withKeys());
    await tester.tap(find.text('Rename'));
    await settle(tester);
    final field = find.byWidgetPredicate(
      (w) => w is EditableText && w.controller.text == 'sk-…a1b2',
    );
    expect(field, findsOneWidget);
    await tester.enterText(field, 'Work credits');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(find.text('sk-…a1b2'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasUpdateAccount()), isEmpty);

    await tester.tap(find.text('Rename'));
    await settle(tester);
    await tester.enterText(field, 'Work credits');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await settle(tester);
    expect(harness.last.updateAccount.accountId, 'k1');
    expect(harness.last.updateAccount.label, 'Work credits');
  });

  test('a key out of credit is a problem to act on', () {
    final state = accountState(
      withKeys(),
      withKeys().accounts[1],
      DateTime.now(),
    );
    expect(state.text, 'Out of credit. Add credit at Anthropic.');
    expect(state.problem, isTrue);
  });

  testWidgets('on a phone the actions wrap instead of overflowing', (
    tester,
  ) async {
    final harness = await showSettings(
      tester,
      withKeys(),
      size: const Size(390, 844),
    );
    // A phone lists the sections: Providers opens as a page of its own.
    harness.backend.settingsSection.value = 'providers';
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    // A phone's finger-sized controls put the accounts below the fold.
    await tester.scrollUntilVisible(
      find.text('Try again'),
      200,
      scrollable: find.byType(Scrollable).first,
    );
    await settle(tester);
    expect(tester.takeException(), isNull);
    expect(find.text('Try again'), findsOneWidget);
  });
}
