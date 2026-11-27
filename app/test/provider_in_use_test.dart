// Which account requests go to, and saying so only when it's true: "In
// use" only while an agent or chat runs on the provider's models, Fable's
// own pool counted and its limit shown per account, and a provider signed
// in to that none of the agents use says so.
import 'package:factory_app/login.dart' show ProviderCard;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/usage.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';

final _claude = Provider(id: 'anthropic', name: 'Claude');

Finder _onOpenRouter(String text) => find.descendant(
  of: find.byWidgetPredicate(
    (w) => w is ProviderCard && w.provider.id == 'openrouter',
  ),
  matching: find.text(text),
);

void main() {
  final now = DateTime(2026, 11, 23, 12);
  final friday = DateTime(2026, 11, 27, 18).millisecondsSinceEpoch;
  final at14 = DateTime(2026, 11, 23, 14).millisecondsSinceEpoch;

  test('In use only while requests go to it, Fable counted', () {
    String state(Account account) => accountState(_claude, account, now).text;
    expect(state(Account(id: 'a')), 'Signed in');
    expect(state(Account(id: 'a', inUse: true)), 'In use');
    // Only Fable agents on it: in use, for Fable.
    expect(state(Account(id: 'a', fableInUse: true)), 'In use for Fable');
  });

  test("Fable's own limit shows on the account it was reached on", () {
    String state(Account account) => accountState(_claude, account, now).text;
    // A: its other agents still on it; Fable's went to B.
    expect(
      state(Account(id: 'a', inUse: true, fableExhaustedUntil: Int64(friday))),
      'In use · Fable: limit reached, back at Fri 18:00',
    );
    expect(state(Account(id: 'b', fableInUse: true)), 'In use for Fable');
    // A past limit says nothing.
    expect(
      state(Account(id: 'a', fableExhaustedUntil: Int64(1000))),
      'Signed in',
    );
    // Its normal limit reached while Fable still runs on it.
    expect(
      state(Account(id: 'a', exhaustedUntil: Int64(at14), fableInUse: true)),
      'Limit reached, back at 14:00 · In use for Fable',
    );
  });

  test('the dot marks the account Fable agents use', () {
    final provider = Provider(
      id: 'anthropic',
      name: 'Claude',
      accounts: [
        Account(id: 'a', fableExhaustedUntil: Int64(friday)),
        Account(id: 'b', fableInUse: true),
      ],
    );
    final meters = usageMeters([provider]);
    expect([for (final m in meters) m.inUseOfMany], [false, true]);
  });

  testWidgets('a provider none of the agents use says so on its card', (
    tester,
  ) async {
    final openRouter = Provider(
      id: 'openrouter',
      name: 'OpenRouter',
      signedInWith: 'OpenRouter API key',
      canSignOut: true,
    );
    final snapshot = workspace()..providers.add(openRouter);
    final harness = Harness(snapshot);
    await harness.show(tester, size: const Size(1200, 1100));
    harness.page.value = AppPage.settings;
    harness.backend.settingsSection.value = 'providers';
    await settle(tester);
    const line = 'No agents or chats use OpenRouter models yet.';
    expect(_onOpenRouter(line), findsOneWidget);

    // An agent runs on it: no line.
    await harness.apply(
      tester,
      workspace()..providers.add(openRouter.deepCopy()..agents = 2),
    );
    await settle(tester);
    expect(_onOpenRouter(line), findsNothing);
  });
}
