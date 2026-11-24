// #343 U6: Needs you's item for the agents and chats waiting on a
// provider's usage limit or credit, one per provider, with the ways
// forward the waiters have: Continue on another provider for all of them
// (the count asked, expected_others sent), a usage reset, Try now, Sign
// in… when there's no other provider. On a Mac and on a phone.
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/provider_wait_card.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

const _said = 'Waiting for your Claude usage to reset at Fri 18:00';

ProviderWait _wait({bool credit = false}) => ProviderWait(
  providerId: 'anthropic',
  providerName: 'Claude',
  credit: credit,
  untilMs: Int64(credit ? 0 : 1795000000000),
  agentIds: ['a1', 'a2'],
  title: 'Claude usage limit reached',
  text: 'Builder and Kai wait until it resets at Fri 18:00.',
);

Agent _waiting(String id, String name) => builder()
  ..id = id
  ..name = name
  ..usageWait = _said
  ..usageWaitUntilMs = Int64(1795000000000)
  ..characterMood = CharacterMood.CHARACTER_MOOD_ASLEEP;

Workspace _snapshot({
  UsageWaitAlternative? alternative,
  bool noOtherProvider = false,
  List<UsageReset> resets = const [],
}) {
  final first = _waiting('a1', 'Builder')
    ..usageWaitNoOtherProvider = noOtherProvider;
  if (alternative != null) first.usageWaitAlternative = alternative;
  final snapshot = workspace(
    agents: [first, _waiting('a2', 'Kai')],
    selected: '',
  );
  snapshot.providers.first.accounts.add(
    Account(
      id: 'acc1',
      label: 'me@example.com',
      plan: 'Claude Max',
      usage: ProviderUsage(resets: resets),
    ),
  );
  snapshot.providerWaits.add(_wait());
  return snapshot;
}

final _openRouter = UsageWaitAlternative(
  providerName: 'OpenRouter',
  model: 'openrouter/anthropic/claude-opus-4.8',
  modelName: 'Claude Opus 4.8',
  sameModel: true,
  effort: 'high',
  othersWaiting: 1,
);

Future<Harness> _open(
  WidgetTester tester,
  Workspace snapshot, {
  Size size = const Size(1200, 900),
}) async {
  final harness = Harness(snapshot);
  await harness.show(tester, size: size);
  await settle(tester);
  harness.page.value = AppPage.inbox;
  await settle(tester);
  expect(find.text('Claude usage limit reached'), findsOneWidget);
  await _tap(tester, 'Claude usage limit reached');
  expect(find.byType(ProviderWaitCard), findsOneWidget);
  return harness;
}

/// Taps [label], scrolled into view first.
Future<void> _tap(WidgetTester tester, String label) async {
  await tester.ensureVisible(find.text(label).last);
  await settle(tester);
  await tester.tap(find.text(label).last);
  await settle(tester);
}

void main() {
  test('one item per provider waited on, counted once', () {
    final snapshot = _snapshot();
    snapshot.providerWaits.add(
      ProviderWait(
        providerId: 'openrouter',
        providerName: 'OpenRouter',
        credit: true,
        agentIds: ['a2'],
        title: 'OpenRouter is out of credit',
      ),
    );
    final items = [
      for (final item in needsYouItems(snapshot))
        if (item.kind == NeedsYouKind.providerWait) item.key,
    ];
    expect(items, ['usage:anthropic', 'usage:openrouter']);
    expect(needsYouCount(snapshot), 2);
  });

  testWidgets('Continue on OpenRouter asks whether both go, and sends the '
      'count it showed', (tester) async {
    final harness = await _open(tester, _snapshot(alternative: _openRouter));
    expect(
      find.text('Builder and Kai wait until it resets at Fri 18:00.'),
      findsOneWidget,
    );
    await _tap(tester, 'Continue on OpenRouter');
    expect(
      find.text('OpenRouter charges you for what they use there.'),
      findsOneWidget,
    );
    expect(find.text('Only Builder'), findsNothing, reason: 'all, or none');
    expect(
      harness.backend.sent.where((m) => m.hasContinueOnAlternative()),
      isEmpty,
    );
    await _tap(tester, 'Both');
    final asked = harness.last.continueOnAlternative;
    expect(asked.agentId, 'a1');
    expect(asked.allWaiting, isTrue);
    expect(asked.expectedOthers, 1);
  });

  testWidgets('Try now asks again with the first waiter', (tester) async {
    final harness = await _open(tester, _snapshot());
    expect(find.text('Continue on OpenRouter'), findsNothing);
    await _tap(tester, 'Try now');
    expect(harness.last.tryAgainNow.agentId, 'a1');
  });

  testWidgets('a usage reset the account has is a button that asks first', (
    tester,
  ) async {
    await _open(
      tester,
      _snapshot(
        resets: [
          UsageReset(id: 'r5', label: '5-hour reset', left: 1, usable: true),
          UsageReset(id: 'rx', label: 'Weekly reset', left: 0, usable: true),
        ],
      ),
    );
    expect(find.text('Use 5-hour reset…'), findsOneWidget);
    expect(
      find.textContaining('eekly reset'),
      findsNothing,
      reason: 'none left',
    );
    await _tap(tester, 'Use 5-hour reset…');
    expect(find.byType(ProviderWaitCard), findsOneWidget);
    expect(find.textContaining('5-hour reset'), findsWidgets);
  });

  testWidgets('with no other provider, Sign in… opens Providers', (
    tester,
  ) async {
    final harness = await _open(tester, _snapshot(noOtherProvider: true));
    expect(
      find.text(
        'Sign in to another provider, such as OpenRouter, to carry on '
        'now.',
      ),
      findsOneWidget,
    );
    await _tap(tester, 'Sign in…');
    expect(harness.page.value, AppPage.settings);
  });

  testWidgets('once no one waits, it says they carry on', (tester) async {
    final harness = await _open(tester, _snapshot());
    final done = _snapshot()..providerWaits.clear();
    await harness.apply(tester, done);
    await settle(tester);
    expect(find.byType(ProviderWaitCard), findsNothing);
  });

  testWidgets('on an iPhone SE at the largest text it fits, buttons under '
      'the words', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 2;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await _open(
      tester,
      _snapshot(
        alternative: _openRouter,
        resets: [
          UsageReset(id: 'r5', label: '5-hour reset', left: 1, usable: true),
        ],
      ),
      size: const Size(320, 568),
    );
    expect(tester.takeException(), isNull);
    for (final label in [
      'Continue on OpenRouter',
      'Use 5-hour reset…',
      'Try now',
    ]) {
      final rect = tester.getRect(find.text(label));
      expect(rect.right, lessThanOrEqualTo(320), reason: label);
    }
  });
}
