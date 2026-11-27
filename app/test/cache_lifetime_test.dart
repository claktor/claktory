// How long a provider keeps a conversation ready, in Settings →
// Providers: a choice from what the backend offers (Provider.
// cache_ttl_choices), or, where nothing can be chosen, a line saying how
// long (Provider.cache_lifetime).
import 'package:factory_app/login.dart'
    show cacheCaption, cacheChoices, cacheLifetimeLabel;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter_test/flutter_test.dart';

import 'accounts_test.dart' show claude, showSettings;
import 'harness.dart';
import 'idle_compaction_test.dart' show reveal;

void main() {
  test('labels, and the choices from a backend before them', () {
    expect(cacheLifetimeLabel('30m'), '30 minutes');
    expect(cacheLifetimeLabel('12h'), '12 hours');
    expect(cacheLifetimeLabel('2h'), '2h');
    // An older backend: Anthropic's two, through cache_ttl alone.
    expect(cacheChoices(Provider(cacheTtl: '5m')), ['5m', '1h']);
    expect(cacheChoices(Provider(cacheLifetime: '30m')), isEmpty);
    expect(
      cacheCaption(Provider(cacheTtlChoices: ['30m', '12h'])),
      startsWith('12 hours costs about twice as much'),
    );
  });

  testWidgets('the choices the backend offers, and the one in effect', (
    tester,
  ) async {
    final harness = await showSettings(
      tester,
      claude()
        ..cacheTtl = '5m'
        ..cacheLifetime = '5m'
        ..cacheTtlChoices.addAll(['5m', '1h']),
    );
    await reveal(tester, find.text('Keep conversations ready'));
    expect(find.text('5 minutes'), findsOneWidget);
    expect(find.textContaining('An hour costs a bit more'), findsOneWidget);
    await tester.tap(find.text('5 minutes'));
    await settle(tester);
    await tester.tap(find.text('1 hour').last);
    await settle(tester);
    expect(
      harness.last.updateSettings,
      UpdateSettings(
        cacheTtl: CacheTtl(key: 'anthropic', ttl: '1h'),
      ),
    );
  });

  testWidgets('nothing to choose: a line saying how long', (tester) async {
    await showSettings(tester, claude()..cacheLifetime = '30m');
    expect(find.text('Keep conversations ready'), findsNothing);
    await reveal(
      tester,
      find.text('Anthropic keeps conversations ready for 30 minutes.'),
    );
    expect(
      find.text('Anthropic keeps conversations ready for 30 minutes.'),
      findsOneWidget,
    );
  });
}
