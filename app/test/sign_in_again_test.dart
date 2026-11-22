// A subscription sign-in the provider stopped accepting (Account.sign_in_
// again, P2): its row in Settings → Providers says the sign-in expired,
// with Sign in again…, which starts the usual sign-in adding, so the same
// account replaces itself and another never overwrites a working one.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/usage.dart' show accountState;
import 'package:flutter_test/flutter_test.dart';

import 'accounts_test.dart' show claude, showSettings;
import 'harness.dart';
import 'usage_test.dart' show weekly;

Provider _expired() => claude(
  accounts: [
    Account(
      id: 'a1',
      label: 'matt@work.test',
      plan: 'Claude Max 20x',
      methodId: 'oauth',
      usage: weekly(10),
      canSignOut: true,
      signInAgain: true,
    ),
    Account(
      id: 'a2',
      label: 'matt@home.test',
      plan: 'Claude Pro',
      methodId: 'oauth',
      usage: weekly(25),
      canSignOut: true,
      inUse: true,
    ),
  ],
);

void main() {
  testWidgets('an expired sign-in says so, with Sign in again… and Sign out', (
    tester,
  ) async {
    final harness = await showSettings(tester, _expired());
    expect(find.text('Sign-in expired'), findsOneWidget);
    expect(find.text('Sign in again…'), findsOneWidget);
    // Not "Signed in" or "In use" beside it.
    expect(
      find.text('Claude Max 20x · 10% of the weekly limit used'),
      findsOneWidget,
    );
    expect(find.text('Sign out'), findsNWidgets(2));

    await tester.tap(find.text('Sign in again…'));
    await settle(tester);
    final start = harness.last.startLogin;
    expect(start.providerId, 'anthropic');
    expect(start.methodId, 'oauth');
    expect(start.addAccount, isTrue);
  });

  testWidgets('a working account has neither', (tester) async {
    await showSettings(tester, claude());
    expect(find.text('Sign-in expired'), findsNothing);
    expect(find.text('Sign in again…'), findsNothing);
  });

  test('its state says so wherever it shows, as a problem', () {
    final provider = _expired();
    final state = accountState(
      provider,
      provider.accounts.first,
      DateTime.now(),
    );
    expect(state.text, 'Sign-in expired');
    expect(state.problem, isTrue);
  });
}
