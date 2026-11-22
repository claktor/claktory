// Usage on a phone (#227, Matt): what's left of each subscription first,
// each account a row of the same width with its limits at a glance and the
// rest a tap away; the range a segmented control that fits; then the
// report. A Mac's page keeps its tabs, report and cards as they were.
import 'dart:io';

import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/usage_view.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _now = DateTime.now();
Int64 _later(Duration d) => Int64(_now.add(d).millisecondsSinceEpoch);
Int64 _ago(Duration d) => Int64(_now.subtract(d).millisecondsSinceEpoch);

ProviderUsage _usage(
  double five,
  double week, {
  List<UsageReset> resets = const [],
}) => ProviderUsage(
  updated: _ago(const Duration(minutes: 3)),
  windows: [
    UsageWindow(
      id: 'five_hour',
      label: '5-hour',
      usedPercent: five,
      resetsAt: _later(const Duration(hours: 2, minutes: 10)),
    ),
    UsageWindow(
      id: 'weekly',
      label: 'Weekly',
      usedPercent: week,
      resetsAt: _later(const Duration(days: 2, hours: 1)),
    ),
    UsageWindow(
      id: 'weekly_opus',
      label: 'Weekly (Opus)',
      usedPercent: week / 2,
      resetsAt: _later(const Duration(days: 2, hours: 1)),
    ),
  ],
  resets: resets,
);

Workspace _workspace() => Workspace(
  agents: [Agent(id: 'june', name: 'June', modelAvailable: true)],
  selectedAgentId: 'june',
  settings: Settings(userName: 'Matt'),
  providers: [
    Provider(
      id: 'anthropic',
      name: 'Anthropic',
      signedInWith: 'Anthropic (Claude Pro/Max)',
      accounts: [
        // One with usage resets, one without: the same width all the same.
        Account(
          id: 'a1',
          label: 'matt@claktory.com',
          plan: 'Claude Max 20x',
          inUse: true,
          usage: _usage(
            34,
            92,
            resets: [
              UsageReset(
                id: 'g1',
                label: '5-hour reset',
                left: 2,
                total: 3,
                usable: true,
                windowIds: ['five_hour'],
              ),
              UsageReset(
                id: 'g2',
                label: 'Weekly reset',
                left: 1,
                usable: true,
                windowIds: ['weekly'],
              ),
            ],
          ),
        ),
        Account(
          id: 'a2',
          label: 'matt@home.test',
          plan: 'Claude Max 5x',
          usage: _usage(5, 41),
        ),
      ],
    ),
    Provider(
      id: 'openai-codex',
      name: 'ChatGPT',
      signedInWith: 'ChatGPT Plus/Pro',
      accounts: [
        Account(
          id: 'c1',
          label: 'matt@openai.test',
          plan: 'Pro',
          exhaustedUntil: _later(const Duration(hours: 3)),
          usage: _usage(100, 60),
        ),
      ],
    ),
  ],
);

UsageReport _report() => UsageReport(
  rows: [
    UsageRow(
      agentId: 'june',
      providerId: 'anthropic',
      model: 'claude-opus-4-8',
      tokens: UsageTokens(input: Int64(200000), output: Int64(50000)),
      requests: Int64(40),
    ),
  ],
);

Future<Harness> _phone(
  WidgetTester tester, {
  Size size = const Size(375, 667),
  double text = 1,
}) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  tester.platformDispatcher.textScaleFactorTestValue = text;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  final harness = Harness(_workspace());
  harness.backend.usageReport = _report();
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  harness.page.value = AppPage.usage;
  await pumpFrames(tester);
  return harness;
}

Finder _segment(String label) => find.bySemanticsLabel(label);

Finder _row(String who) => find.ancestor(
  of: find.text(who),
  matching: find.byWidgetPredicate(
    (widget) => widget is Semantics && widget.properties.expanded != null,
  ),
);

/// Geist, as the app ships it, so the ranges' words are as wide as on the
/// phone (the test font's squares are wider).
Future<void> _loadFonts() async {
  Future<void> family(String name, List<String> files) async {
    final loader = FontLoader(name);
    for (final file in files) {
      loader.addFont(
        Future.value(
          ByteData.sublistView(File('fonts/$file').readAsBytesSync()),
        ),
      );
    }
    await loader.load();
  }

  await family('Geist', [
    'Geist-Regular.ttf',
    'Geist-Medium.ttf',
    'Geist-SemiBold.ttf',
  ]);
}

/// A subscription's detail, opened under its row.
final _details = find.byWidgetPredicate(
  (widget) => widget.runtimeType.toString() == '_LimitDetail',
);

void main() {
  setUpAll(_loadFonts);

  testWidgets("the accounts' limits come first, then the range, then the "
      'report', (tester) async {
    await _phone(tester);
    final account = tester.getRect(find.text('matt@claktory.com'));
    final range = tester.getRect(find.text('7 days'));
    expect(account.top, lessThan(range.top));
    await tester.scrollUntilVisible(find.text('By agent'), 200);
    expect(
      tester.getRect(find.text('By agent')).top,
      greaterThan(tester.getRect(find.text('7 days')).top),
    );
    // Grouped under each provider.
    expect(find.text('Anthropic'), findsWidgets);
    expect(find.text('ChatGPT'), findsWidgets);
  });

  for (final (name, size) in [
    ('a small iPhone', const Size(375, 667)),
    ('a Pro Max', const Size(430, 932)),
  ]) {
    testWidgets('on $name every account row is the same width, resets or '
        'not', (tester) async {
      await _phone(tester, size: size);
      final widths = {
        for (final who in [
          'matt@claktory.com',
          'matt@home.test',
          'matt@openai.test',
        ])
          tester.getSize(_row(who).first).width,
      };
      expect(widths, hasLength(1));
      expect(widths.single, size.width - 2 * 16 - 2, reason: 'the column');
    });

    testWidgets('on $name the ranges sit side by side, 44pt tall', (
      tester,
    ) async {
      await _phone(tester, size: size);
      final rects = [
        for (final range in UsageRange.values)
          tester.getRect(_segment(range.label)),
      ];
      for (final rect in rects) {
        expect(rect.top, rects.first.top);
        expect(rect.width, closeTo(rects.first.width, 0.5));
        expect(rect.height, greaterThanOrEqualTo(44));
      }
      expect(rects.last.left, greaterThan(rects.first.right));
    });
  }

  testWidgets('at the largest text, the ranges are two rows of two and '
      'nothing overflows', (tester) async {
    await _phone(tester, text: 3.1);
    final today = tester.getRect(_segment('Today'));
    final week = tester.getRect(_segment('7 days'));
    final month = tester.getRect(_segment('30 days'));
    expect(week.top, today.top);
    expect(month.top, greaterThan(today.bottom - 1));
    expect(tester.takeException(), isNull);
  });

  testWidgets("a row says what's used at a glance, or why it's paused; a "
      'tap opens every limit and its resets, another folds them', (
    tester,
  ) async {
    await _phone(tester);
    expect(find.text('Used: weekly 92% · 5-hour 34%'), findsOneWidget);
    expect(find.text('3 usage resets left'), findsOneWidget);
    expect(find.textContaining('Limit reached, back at'), findsOneWidget);
    expect(find.text('Weekly (Opus)'), findsNothing);

    await tester.tap(find.text('matt@claktory.com'));
    await pumpFrames(tester);
    expect(find.text('Weekly (Opus)'), findsOneWidget);
    expect(find.text('Claude Max 20x · In use'), findsOneWidget);
    expect(find.text('Use…'), findsNWidgets(2));
    expect(
      tester.getSemantics(_row('matt@claktory.com').first),
      isSemantics(isButton: true, isExpanded: true, hasExpandedState: true),
    );

    await tester.tap(find.text('matt@claktory.com'));
    await pumpFrames(tester);
    expect(find.text('Weekly (Opus)'), findsNothing);
  });

  testWidgets('a range chosen asks for it, and reads as chosen', (
    tester,
  ) async {
    final harness = await _phone(tester);
    final asked = harness.backend.sent.where((m) => m.hasLoadUsage()).length;
    await tester.tap(_segment('30 days'));
    await pumpFrames(tester);
    final loads = harness.backend.sent.where((m) => m.hasLoadUsage()).toList();
    expect(loads.length, asked + 1);
    expect(
      tester.getSemantics(_segment('30 days')),
      isSemantics(isButton: true, isSelected: true, hasSelectedState: true),
    );
    expect(
      tester.getSemantics(_segment('7 days')),
      isSemantics(isButton: true, isSelected: false, hasSelectedState: true),
    );
  });

  testWidgets('of several subscriptions, the first says it is used first; '
      'Use this one on another puts it first (#277)', (tester) async {
    final harness = await _phone(tester);
    await tester.tap(find.text('matt@home.test'));
    await pumpFrames(tester);
    expect(
      find.text(
        'Claktory then uses it first whenever it has room, including after '
        'a reset.',
      ),
      findsOneWidget,
    );
    final use = find.bySemanticsLabel('Use this one');
    expect(tester.getSize(use).height, greaterThanOrEqualTo(44));
    await tester.tap(use);
    await pumpFrames(tester);
    expect(
      harness.backend.sent.where((m) => m.hasMoveAccount()).single.moveAccount,
      MoveAccount(providerId: 'anthropic', accountId: 'a2', position: 0),
    );
    // The first one says so, with nothing to tap.
    await tester.tap(find.text('matt@claktory.com'));
    await pumpFrames(tester);
    expect(
      find.text('Used first whenever it has room, including after a reset.'),
      findsOneWidget,
    );
    expect(find.bySemanticsLabel('Use this one'), findsOneWidget);
    // A provider with one subscription has no order to choose. The others
    // closed first, so what shows is only its own; a tap that misses fails.
    WidgetController.hitTestWarningShouldBeFatal = true;
    addTearDown(() => WidgetController.hitTestWarningShouldBeFatal = false);
    await tester.tap(find.text('matt@claktory.com'));
    await tester.tap(find.text('matt@home.test'));
    await pumpFrames(tester);
    expect(_details, findsNothing);
    expect(find.textContaining('Used first'), findsNothing);
    final openai = find.text('matt@openai.test');
    await tester.ensureVisible(openai);
    await pumpFrames(tester);
    await tester.tap(openai);
    await pumpFrames(tester);
    // Its detail opened: the only one open.
    expect(_details, findsOneWidget);
    expect(find.textContaining('Used first'), findsNothing);
    expect(find.bySemanticsLabel('Use this one'), findsNothing);
    expect(find.textContaining('uses it first'), findsNothing);
  });

  testWidgets("a Mac's cards have Use this one too", (tester) async {
    final harness = Harness(_workspace());
    harness.backend.usageReport = _report();
    await harness.show(tester, size: const Size(1200, 900));
    harness.page.value = AppPage.usage;
    await pumpFrames(tester);
    await tester.scrollUntilVisible(
      find.text('Use this one'),
      200,
      scrollable: find.byType(Scrollable).last,
    );
    expect(find.text('Use this one'), findsOneWidget);
    expect(
      find.text('Used first whenever it has room, including after a reset.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Use this one'));
    await pumpFrames(tester);
    expect(
      harness.backend.sent.where((m) => m.hasMoveAccount()).single.moveAccount,
      MoveAccount(providerId: 'anthropic', accountId: 'a2', position: 0),
    );
  });

  testWidgets("a Mac's page keeps its tabs first and its cards last", (
    tester,
  ) async {
    final harness = Harness(_workspace());
    harness.backend.usageReport = _report();
    await harness.show(tester, size: const Size(1200, 900));
    harness.page.value = AppPage.usage;
    await pumpFrames(tester);
    expect(find.text('Subscription limits'), findsOneWidget);
    expect(
      tester.getRect(find.text('7 days')).top,
      lessThan(tester.getRect(find.text('Subscription limits')).top),
    );
  });
}
