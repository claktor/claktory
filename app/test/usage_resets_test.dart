import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/usage.dart';
import 'package:factory_app/usage_resets.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'focus_test.dart' show focusedOn;
import 'harness.dart';
import 'usage_test.dart' show hover, later, weekly;

UsageReset _fiveHour({
  int left = 2,
  int total = 3,
  bool usable = true,
  String unavailable = '',
}) => UsageReset(
  id: 'g1',
  label: '5-hour reset',
  detail: 'Refills your 5-hour limit.',
  left: left,
  total: total,
  expiresAt: later(const Duration(days: 3)),
  usable: usable,
  unavailable: unavailable,
  windowIds: ['five_hour'],
);

UsageReset _weekly() => UsageReset(
  id: 'g2',
  label: 'Weekly reset',
  detail: 'Refills your weekly limit.',
  left: 1,
  usable: false,
  unavailable: 'It works only at a usage limit.',
  windowIds: ['weekly'],
);

/// Claude with one account, matt@example.com, at [percent] of its weekly
/// limit, with [resets].
Provider _claude(double percent, List<UsageReset> resets) => Provider(
  id: 'anthropic',
  name: 'Anthropic',
  signedInWith: 'Anthropic (Claude Pro/Max)',
  accounts: [
    Account(
      id: 'acct-1',
      label: 'matt@example.com',
      plan: 'Claude Max',
      usage: weekly(percent)..resets.addAll(resets),
    ),
  ],
);

Future<Harness> _show(WidgetTester tester, Provider provider) async {
  final harness = Harness(focusedOn('')..providers.add(provider));
  await harness.show(tester);
  return harness;
}

Finder _cue() => find.byWidgetPredicate(
  (widget) =>
      widget is Semantics &&
      widget.properties.label ==
          'Close to a limit. A usage reset can refill it.',
);

void main() {
  group('the cue', () {
    UsageMeter meter(double percent, List<UsageReset> resets) {
      final provider = _claude(percent, resets);
      return UsageMeter(provider, provider.accounts.single);
    }

    test('only near a limit one of its resets refills', () {
      expect(resetCue(meter(95, [_weekly()])), isTrue);
      expect(resetCue(meter(90, [_weekly()])), isTrue);
      expect(resetCue(meter(89, [_weekly()])), isFalse);
      // The 5-hour limit is at 12%: its reset has nothing to refill.
      expect(resetCue(meter(95, [_fiveHour()])), isFalse);
      // None left.
      expect(resetCue(meter(95, [_weekly()..left = 0])), isFalse);
      // One that doesn't say refills every limit.
      expect(resetCue(meter(95, [_weekly()..windowIds.clear()])), isTrue);
      expect(resetCue(meter(95, [])), isFalse);
    });

    testWidgets('shows on the circle', (tester) async {
      await _show(tester, _claude(96, [_weekly()]));
      expect(_cue(), findsOneWidget);
    });

    testWidgets('not while far from the limit', (tester) async {
      await _show(tester, _claude(40, [_weekly()]));
      expect(_cue(), findsNothing);
    });
  });

  test('labels read in a sentence', () {
    expect(inSentence('Weekly reset'), 'weekly reset');
    expect(inSentence('Full reset'), 'full reset');
    expect(inSentence('Fable reset'), 'Fable reset');
    expect(inSentence('5-hour reset'), '5-hour reset');
  });

  testWidgets('the card lists every reset, even far from a limit', (
    tester,
  ) async {
    await _show(tester, _claude(40, [_fiveHour(), _weekly()]));
    await hover(tester, 'Anthropic', 'matt@example.com');
    expect(find.text('Usage resets'), findsOneWidget);
    expect(find.text('5-hour reset'), findsOneWidget);
    expect(find.textContaining('2 of 3 left, use by '), findsOneWidget);
    expect(find.text('Weekly reset'), findsOneWidget);
    expect(find.text('1 left'), findsOneWidget);
    expect(find.text('It works only at a usage limit.'), findsOneWidget);
    expect(find.text('Use…'), findsNWidgets(2));
  });

  testWidgets('no resets, no section', (tester) async {
    await _show(tester, _claude(40, []));
    await hover(tester, 'Anthropic', 'matt@example.com');
    expect(find.text('Weekly'), findsOneWidget);
    expect(find.text('Usage resets'), findsNothing);
  });

  testWidgets('Use… asks first, then uses it and says how it went', (
    tester,
  ) async {
    final harness = await _show(tester, _claude(96, [_fiveHour()]));
    harness.backend.usageResetAnswer = UsageResetAnswer(
      reset: true,
      message: 'Your 5-hour limit is reset. 1 left.',
    );
    await hover(tester, 'Anthropic', 'matt@example.com');
    await tester.tap(find.text('Use…'));
    await settle(tester);
    expect(find.text('Use a 5-hour reset?'), findsOneWidget);
    expect(find.text('Refills your 5-hour limit.'), findsOneWidget);
    expect(find.text('Anthropic, matt@example.com'), findsOneWidget);
    // Nothing is used before the person says so.
    expect(harness.backend.sent.where((m) => m.hasUseUsageReset()), isEmpty);

    await tester.tap(find.text('Use reset'));
    await settle(tester);
    final use = harness.last.useUsageReset;
    expect(use.providerId, 'anthropic');
    expect(use.accountId, 'acct-1');
    expect(use.resetId, 'g1');
    expect(find.text('Your 5-hour limit is reset. 1 left.'), findsOneWidget);
    await tester.tap(find.text('Done'));
    await settle(tester);
    expect(find.text('Your 5-hour limit is reset. 1 left.'), findsNothing);
  });

  testWidgets('one that says it can\'t be used now can be tried anyway', (
    tester,
  ) async {
    final harness = await _show(tester, _claude(96, [_weekly()]));
    harness.backend.usageResetAnswer = UsageResetAnswer(
      reset: false,
      message: "Claude didn't accept it.",
      fallbackUrl: 'https://clau.de/reset',
    );
    final opened = <Uri>[];
    openResetPage = (url) async {
      opened.add(url);
      return true;
    };
    addTearDown(() => openResetPage = (url) async => false);
    await hover(tester, 'Anthropic', 'matt@example.com');
    await tester.tap(find.text('Use…'));
    await settle(tester);
    expect(find.text('Use a weekly reset?'), findsOneWidget);
    expect(
      find.text('It works only at a usage limit. You can try anyway.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Use reset'));
    await settle(tester);
    expect(harness.last.useUsageReset.resetId, 'g2');
    expect(find.text("Claude didn't accept it."), findsOneWidget);
    await tester.tap(find.text('Open claude.ai'));
    await settle(tester);
    expect(opened, [Uri.parse('https://clau.de/reset')]);
  });

  testWidgets('a refusal shows in the dialog', (tester) async {
    final harness = await _show(tester, _claude(96, [_fiveHour()]));
    await hover(tester, 'Anthropic', 'matt@example.com');
    await tester.tap(find.text('Use…'));
    await settle(tester);
    harness.backend.refuse = 'A reset is already being used on this account.';
    await tester.tap(find.text('Use reset'));
    await settle(tester);
    expect(
      find.text('A reset is already being used on this account.'),
      findsOneWidget,
    );
    expect(find.text('Use reset'), findsOneWidget);
  });

  testWidgets('on a phone, a touch and hold opens the card until a tap '
      'elsewhere', (tester) async {
    final harness = Harness(
      focusedOn('')..providers.add(_claude(96, [_fiveHour()])),
    );
    await harness.show(tester, size: const Size(390, 844));
    await settle(tester);
    final usage = find.byWidgetPredicate(
      (widget) =>
          widget is Semantics &&
          widget.properties.label == 'Anthropic usage, matt@example.com',
    );
    await tester.longPress(usage);
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('Usage resets'), findsOneWidget);
    await tester.tap(find.text('Use…'));
    await settle(tester);
    expect(find.text('Use a 5-hour reset?'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    await tester.tapAt(const Offset(200, 300));
    await settle(tester);
    expect(find.text('Usage resets'), findsNothing);
  });

  test('counts and expiry read plainly', () {
    final now = DateTime(2026, 11, 20, 12);
    expect(resetCount(UsageReset(left: 1), now), '1 left');
    expect(
      resetCount(
        UsageReset(
          left: 2,
          total: 3,
          expiresAt: Int64(DateTime(2026, 11, 24, 14).millisecondsSinceEpoch),
        ),
        now,
      ),
      '2 of 3 left, use by Tue 14:00',
    );
  });
}
