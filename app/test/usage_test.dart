import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/usage.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'focus_test.dart' show focusedOn;
import 'harness.dart';

final now = DateTime.now();
Int64 ago(Duration duration) =>
    Int64(now.subtract(duration).millisecondsSinceEpoch);
Int64 later(Duration duration) =>
    Int64(now.add(duration).millisecondsSinceEpoch);

Provider subscription({
  ProviderUsage? usage,
  String id = 'anthropic',
  String name = 'Anthropic',
}) => Provider(
  id: id,
  name: name,
  signedInWith: 'Anthropic (Claude Pro/Max)',
  usage: usage,
);

ProviderUsage weekly(double percent, {String error = ''}) => ProviderUsage(
  updated: ago(const Duration(minutes: 3)),
  error: error,
  windows: [
    UsageWindow(
      id: 'five_hour',
      label: '5-hour',
      usedPercent: 12,
      resetsAt: later(const Duration(hours: 2, minutes: 10)),
    ),
    UsageWindow(
      id: 'weekly',
      label: 'Weekly',
      usedPercent: percent,
      resetsAt: later(const Duration(days: 2, hours: 1)),
    ),
  ],
);

Finder circle(String provider, [String who = '']) =>
    find.byWidgetPredicate((widget) {
      if (widget is! Semantics) return false;
      final name = who.isEmpty ? '$provider usage' : '$provider usage, $who';
      // The account requests go to now, among several, says so after.
      final label = widget.properties.label;
      return label == name || label == '$name, in use';
    });

/// Hovers over [provider]'s circle until its card shows.
Future<void> hover(
  WidgetTester tester,
  String provider, [
  String who = '',
]) async {
  final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
  addTearDown(mouse.removePointer);
  await mouse.addPointer(location: Offset.zero);
  await mouse.moveTo(tester.getCenter(circle(provider, who)));
  await tester.pump(const Duration(milliseconds: 400));
  await tester.pump(const Duration(milliseconds: 200));
}

Future<void> showProviders(WidgetTester tester, List<Provider> providers) =>
    Harness(focusedOn('')..providers.addAll(providers)).show(tester);

void main() {
  testWidgets('only signed-in providers get a circle', (tester) async {
    await showProviders(tester, [
      subscription(usage: weekly(40)),
      Provider(id: 'openai', name: 'OpenAI'),
      Provider(
        id: 'openrouter',
        name: 'OpenRouter',
        signedInWith: 'OPENROUTER_API_KEY',
      ),
    ]);
    expect(circle('Anthropic'), findsOneWidget);
    expect(circle('OpenAI'), findsNothing);
    expect(circle('OpenRouter'), findsOneWidget);
  });

  testWidgets('the hover names the plan once it is known', (tester) async {
    await showProviders(tester, [
      subscription(usage: weekly(40))..plan = 'Claude Max 20x',
    ]);
    await hover(tester, 'Anthropic');
    expect(find.text('Claude Max 20x'), findsOneWidget);
    expect(find.textContaining('Signed in with'), findsNothing);
  });

  test('the circle fills with the weekly limit', () {
    for (final percent in [0.0, 50.0, 100.0]) {
      expect(usageMark(weekly(percent)), (UsageMark.meter, percent / 100));
    }
    expect(usageMark(weekly(140)).$2, 1, reason: 'never more than full');
  });

  test('no numbers is never drawn as an empty meter', () {
    expect(
      usageMark(ProviderUsage()).$1,
      UsageMark.unknown,
      reason: 'never read',
    );
    expect(
      usageMark(ProviderUsage(error: 'HTTP 500')).$1,
      UsageMark.unknown,
      reason: 'failed before any read',
    );
    expect(
      usageMark(null).$1,
      UsageMark.noLimits,
      reason: 'an API key has no limits',
    );
  });

  for (final percent in [0.0, 50.0, 100.0]) {
    testWidgets('hovering at $percent% gives every limit and its reset', (
      tester,
    ) async {
      await showProviders(tester, [subscription(usage: weekly(percent))]);
      await hover(tester, 'Anthropic');
      expect(find.text('Signed in with Anthropic (Claude Pro/Max)'), findsOne);
      expect(find.text('Weekly'), findsOneWidget);
      expect(find.text('${percent.round()}%'), findsOneWidget);
      expect(find.text('5-hour'), findsOneWidget);
      expect(find.text('12%'), findsOneWidget);
      expect(find.textContaining('resets in 2 h '), findsOneWidget);
      expect(find.textContaining('resets in 2 days, '), findsOneWidget);
      expect(find.text('Checked 3 min ago'), findsOneWidget);
      // The weekly limit leads.
      expect(
        tester.getTopLeft(find.text('Weekly')).dy,
        lessThan(tester.getTopLeft(find.text('5-hour')).dy),
      );
    });
  }

  testWidgets('a failed read keeps the last numbers, with their age', (
    tester,
  ) async {
    await showProviders(tester, [
      subscription(usage: weekly(63, error: 'HTTP 429')),
    ]);
    await hover(tester, 'Anthropic');
    expect(find.text("Couldn't check usage: HTTP 429"), findsOneWidget);
    expect(find.text('63%'), findsOneWidget);
    expect(find.text('Last checked 3 min ago'), findsOneWidget);
  });

  testWidgets('hovering says what extra usage may spend past the limits', (
    tester,
  ) async {
    await showProviders(tester, [
      subscription(
        usage: weekly(100)
          ..extraUsage = ExtraUsage(
            enabled: true,
            usedMinor: Int64(80),
            currency: 'USD',
            decimalPlaces: 2,
          ),
      ),
    ]);
    await hover(tester, 'Anthropic');
    expect(find.text('Extra usage on · used \$0.80 this month'), findsOne);
  });

  test('extra usage in a line: the amounts in their currency, the cap '
      'when there is one', () {
    String? line(ExtraUsage extra) =>
        extraUsageLine(ProviderUsage(extraUsage: extra))?.text;
    expect(extraUsageLine(ProviderUsage()), isNull);
    expect(line(ExtraUsage()), 'Extra usage off');
    expect(
      line(
        ExtraUsage(
          enabled: true,
          usedMinor: Int64(1205),
          limitMinor: Int64(10000),
          currency: 'GBP',
          decimalPlaces: 2,
        ),
      ),
      'Extra usage on · used £12.05 of £100 this month',
    );
    expect(
      line(
        ExtraUsage(
          enabled: true,
          usedMinor: Int64(1500),
          limitMinor: Int64(1500),
          currency: 'JPY',
          limitReached: true,
        ),
      ),
      'Extra usage limit reached · used ¥1500 of ¥1500 this month',
    );
    expect(
      line(
        ExtraUsage(
          enabled: true,
          usedMinor: Int64(0),
          limitMinor: Int64(2550),
          currency: 'CHF',
          decimalPlaces: 2,
        ),
      ),
      'Extra usage on · used 0.00 CHF of 25.50 CHF this month',
    );
  });

  testWidgets('usage never read says so, with no numbers', (tester) async {
    await showProviders(tester, [subscription(usage: ProviderUsage())]);
    await hover(tester, 'Anthropic');
    expect(find.text('Usage not checked yet.'), findsOneWidget);
    expect(find.textContaining('%'), findsNothing);
    expect(find.textContaining('Read'), findsNothing);
  });

  testWidgets('an API key says how it is signed in', (tester) async {
    await showProviders(tester, [
      Provider(
        id: 'anthropic',
        name: 'Anthropic',
        signedInWith: 'ANTHROPIC_API_KEY',
      ),
    ]);
    await hover(tester, 'Anthropic');
    expect(find.text('Signed in with ANTHROPIC_API_KEY'), findsOneWidget);
    expect(find.text('No usage limits to show.'), findsOneWidget);
    expect(find.textContaining('%'), findsNothing);
  });

  testWidgets('the card goes away when the pointer leaves', (tester) async {
    await showProviders(tester, [subscription(usage: weekly(10))]);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    addTearDown(mouse.removePointer);
    await mouse.addPointer(location: Offset.zero);
    await mouse.moveTo(tester.getCenter(circle('Anthropic')));
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('Weekly'), findsNothing, reason: 'not while passing by');
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('Weekly'), findsOneWidget);
    await mouse.moveTo(Offset.zero);
    await tester.pump();
    // A moment to reach the card's buttons first.
    expect(find.text('Weekly'), findsOneWidget);
    await tester.pump(const Duration(milliseconds: 250));
    expect(find.text('Weekly'), findsNothing);
  });

  test('times read plainly', () {
    final at = DateTime(2026, 11, 18, 12);
    int plus(Duration d) => at.add(d).millisecondsSinceEpoch;
    expect(formatReset(0, at), 'reset time unknown');
    expect(
      formatReset(plus(const Duration(minutes: 5)), at),
      'resets in 5 min',
    );
    expect(formatReset(plus(const Duration(hours: 3)), at), 'resets in 3 h');
    expect(
      formatReset(plus(const Duration(days: 1, hours: 2)), at),
      'resets in 1 day 2 h, Thu\u00A014:00',
    );
    // Monday 10:00 to Thursday 06:00 is 2 days 20 h, not
    // "2 days".
    final monday = DateTime(2026, 11, 16, 10);
    expect(
      formatReset(DateTime(2026, 11, 19, 6).millisecondsSinceEpoch, monday),
      'resets in 2 days 20 h, Thu\u00A006:00',
    );
    expect(
      formatReset(DateTime(2026, 11, 19, 10).millisecondsSinceEpoch, monday),
      'resets in 3 days, Thu\u00A010:00',
    );
    expect(formatReset(plus(const Duration(hours: -1)), at), 'has reset since');
    expect(formatAge(plus(const Duration(seconds: -20)), at), 'just now');
    expect(formatAge(plus(const Duration(hours: -5)), at), '5 h ago');
    expect(formatPercent(0.4), '0.4%');
    expect(formatPercent(99.6), '100%');
  });

  test('each provider mark parses whole, inside its grid', () {
    for (final (id, grid) in [
      ('anthropic', 24.0),
      ('openai-codex', 40.0),
      ('openrouter', 24.0),
    ]) {
      final mark = providerMark(id)!;
      var length = 0.0;
      for (final metric in mark.computeMetrics()) {
        length += metric.length;
      }
      final bounds = mark.getBounds();
      expect(length, greaterThan(grid * 2), reason: id);
      expect(bounds.width, greaterThan(grid * 0.5), reason: id);
      expect(bounds.left, greaterThanOrEqualTo(-0.5), reason: id);
      expect(bounds.right, lessThanOrEqualTo(grid + 0.5), reason: id);
    }
    expect(providerMark('someone-new'), isNull);
  });

  testWidgets('many subscriptions wrap on their own row, never overlapping', (
    tester,
  ) async {
    await showProviders(tester, [
      for (var i = 0; i < 20; i++)
        subscription(
          id: 'anthropic',
          name: 'Claude $i',
          usage: weekly(5.0 * i),
        ),
    ]);
    expect(tester.takeException(), isNull);
    final rects = [
      for (var i = 0; i < 20; i++) tester.getRect(circle('Claude $i')),
    ];
    final settings = tester.getRect(
      find.byWidgetPredicate(
        (w) => w is Semantics && w.properties.label == 'Settings',
      ),
    );
    for (var i = 0; i < rects.length; i++) {
      expect(rects[i].right, lessThanOrEqualTo(248), reason: 'in the sidebar');
      expect(rects[i].bottom, lessThanOrEqualTo(settings.top), reason: 'above');
      for (var j = i + 1; j < rects.length; j++) {
        expect(rects[i].overlaps(rects[j]), isFalse, reason: '$i and $j');
      }
    }
    expect(rects.last.top, greaterThan(rects.first.top), reason: 'wrapped');
  });

  group('several accounts', () {
    Provider claude() => subscription(usage: weekly(40))
      ..plan = 'Claude Max 20x'
      ..accounts.addAll([
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
        Account(id: 'k1', label: 'sk-ant-…a1b2', canSignOut: true),
      ]);

    testWidgets('each account gets its own circle, in failover order', (
      tester,
    ) async {
      await showProviders(tester, [claude()]);
      expect(circle('Anthropic'), findsNothing);
      final work = tester.getRect(circle('Anthropic', 'matt@work.test'));
      final home = tester.getRect(circle('Anthropic', 'matt@home.test'));
      final key = tester.getRect(circle('Anthropic', 'sk-ant-…a1b2'));
      expect(work.left, lessThan(home.left));
      expect(home.left, lessThan(key.left));
    });

    test('a key has no ring; a subscription meters its own week', () {
      final meters = usageMeters([claude()]);
      expect(
        [for (final m in meters) usageMark(m.usage)],
        [
          (UsageMark.meter, 1.0),
          (UsageMark.meter, 0.25),
          (UsageMark.noLimits, 0.0),
        ],
      );
    });

    testWidgets('hovering names the account, its plan and its state', (
      tester,
    ) async {
      await showProviders(tester, [claude()]);
      await hover(tester, 'Anthropic', 'matt@home.test');
      expect(find.text('matt@home.test'), findsOneWidget);
      expect(find.text('Claude Pro'), findsOneWidget);
      expect(find.text('In use'), findsOneWidget);
      expect(find.text('25%'), findsOneWidget);
      expect(find.textContaining('Signed in with'), findsNothing);
    });

    testWidgets('the one in use has a dot under its circle, and says so', (
      tester,
    ) async {
      await showProviders(tester, [claude()]);
      expect(
        find.bySemanticsLabel('Anthropic usage, matt@home.test, in use'),
        findsOneWidget,
      );
      expect(
        find.bySemanticsLabel(RegExp('matt@work.test, in use')),
        findsNothing,
      );
      final dots = find.descendant(
        of: find.byType(UsageRing),
        matching: find.byWidgetPredicate(
          (widget) =>
              widget is DecoratedBox &&
              widget.decoration is BoxDecoration &&
              (widget.decoration as BoxDecoration).shape == BoxShape.circle,
        ),
      );
      expect(dots, findsOneWidget);
      final dot = tester.getRect(dots);
      final home = tester.getRect(circle('Anthropic', 'matt@home.test'));
      expect(dot.width, inUseDot);
      expect(dot.center.dx, closeTo(home.center.dx, 0.5));
      expect(dot.top, greaterThan(home.center.dy + 9), reason: 'under it');
    });

    testWidgets('a provider with one account marks nothing', (tester) async {
      await showProviders(tester, [
        subscription(usage: weekly(40))
          ..accounts.add(
            Account(id: 'a1', label: 'matt@work.test', inUse: true),
          ),
      ]);
      expect(find.bySemanticsLabel(RegExp('in use')), findsNothing);
    });

    testWidgets('an account at its limit says until when', (tester) async {
      await showProviders(tester, [claude()]);
      await hover(tester, 'Anthropic', 'matt@work.test');
      expect(find.textContaining('Limit reached, back at '), findsOneWidget);
      expect(find.text('100%'), findsOneWidget);
    });

    testWidgets('a key has no limits to show', (tester) async {
      await showProviders(tester, [claude()]);
      await hover(tester, 'Anthropic', 'sk-ant-…a1b2');
      expect(find.text('sk-ant-…a1b2'), findsOneWidget);
      expect(find.text('No usage limits to show.'), findsOneWidget);
    });
  });

  // An OpenRouter key says what it has to spend.
  group('an API key with credit', () {
    Provider openrouter(AccountCredit credit) => Provider(
      id: 'openrouter',
      name: 'OpenRouter',
      signedInWith: 'API key',
      accounts: [
        Account(
          id: 'k1',
          label: 'sk-or-…9f2c',
          canSignOut: true,
          credit: credit,
        ),
      ],
    );

    test('its ring fills as its spending limit is used, only with one', () {
      expect(creditMark(AccountCredit(limited: true, limit: 10, left: 2.5)), (
        UsageMark.meter,
        0.75,
      ));
      expect(creditMark(AccountCredit(limited: true, limit: 10, left: -1)), (
        UsageMark.meter,
        1.0,
      ));
      expect(creditMark(AccountCredit(spentToday: 3)), (
        UsageMark.noLimits,
        0.0,
      ));
      expect(
        creditLine(
          AccountCredit(
            limited: true,
            limit: 10,
            left: 3.2,
            limitReset: 'daily',
            spentToday: 0.8,
          ),
        ),
        '\$3.20 left of \$10 a day · \$0.80 today',
      );
      expect(
        creditLine(AccountCredit(spentToday: 0.8)),
        '\$0.80 today · no spending limit on this key',
      );
    });

    testWidgets('hovering says what it has to spend, and why it can\'t', (
      tester,
    ) async {
      await showProviders(tester, [
        openrouter(
          AccountCredit(
            spentToday: 0.8,
            readMs: ago(const Duration(minutes: 2)),
          ),
        ),
      ]);
      await hover(tester, 'OpenRouter', 'sk-or-…9f2c');
      expect(
        find.text('\$0.80 today · no spending limit on this key'),
        findsOneWidget,
      );
      expect(find.text('No usage limits to show.'), findsNothing);
      expect(find.textContaining('Checked '), findsOneWidget);
    });

    testWidgets('a key that can\'t run paid models says so', (tester) async {
      const warning = 'This key has no credit yet. Add credit at OpenRouter.';
      await showProviders(tester, [
        openrouter(
          AccountCredit(
            neverPaid: true,
            warning: warning,
            readMs: ago(const Duration(minutes: 2)),
            error: 'OpenRouter couldn\'t be reached.',
          ),
        ),
      ]);
      await hover(tester, 'OpenRouter', 'sk-or-…9f2c');
      expect(find.text(warning), findsOneWidget);
      expect(
        find.text("Couldn't check credit: OpenRouter couldn't be reached."),
        findsOneWidget,
      );
      expect(find.textContaining('Last checked '), findsOneWidget);
    });
  });

  test('times to come read as a clock, a weekday or a date', () {
    final at = DateTime(2026, 11, 18, 12);
    int plus(Duration d) => at.add(d).millisecondsSinceEpoch;
    expect(formatUntil(plus(const Duration(hours: 2)), at), '14:00');
    expect(formatUntil(plus(const Duration(days: 1)), at), 'Thu 12:00');
    expect(formatUntil(plus(const Duration(days: 9)), at), '27 Nov 12:00');
  });
}
