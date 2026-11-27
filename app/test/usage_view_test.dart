import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/usage_view.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

UsageTokens _tokens(int input, int output, [int read = 0, int write = 0]) =>
    UsageTokens(
      input: Int64(input),
      output: Int64(output),
      cacheRead: Int64(read),
      cacheWrite: Int64(write),
    );

UsageRow _row(
  String agent,
  String provider,
  String model,
  UsageTokens tokens, {
  String name = '',
  int requests = 1,
}) => UsageRow(
  agentId: agent,
  agentName: name,
  providerId: provider,
  model: model,
  tokens: tokens,
  requests: Int64(requests),
);

Workspace _workspace() => Workspace(
  agents: [
    Agent(id: 'june', name: 'June', color: 'pink', modelAvailable: true),
    Agent(id: 'mike', name: 'Mike', color: 'blue', modelAvailable: true),
  ],
  selectedAgentId: 'june',
  settings: Settings(userName: 'Matt'),
  providers: [
    Provider(id: 'anthropic', name: 'Anthropic'),
    Provider(id: 'openai-codex', name: 'ChatGPT'),
  ],
  models: [
    Model(id: 'anthropic/claude-opus-4-8', name: 'Claude Opus 4.8'),
    Model(id: 'anthropic/claude-haiku-4-5', name: 'Claude Haiku 4.5'),
    Model(id: 'openai-codex/gpt-5.5', name: 'GPT-5.5'),
  ],
);

UsageReport _report() => UsageReport(
  rows: [
    _row(
      'june',
      'anthropic',
      'claude-opus-4-8',
      _tokens(200000, 50000, 700000, 50000),
      requests: 40,
    ),
    _row(
      'june',
      'anthropic',
      'claude-haiku-4-5',
      _tokens(10000, 2000),
      requests: 3,
    ),
    _row('mike', 'openai-codex', 'gpt-5.5', _tokens(150000, 30000)),
    _row(
      'gone',
      'anthropic',
      'claude-opus-4-8',
      _tokens(5000, 1000),
      name: 'Wren',
    ),
    _row('', 'anthropic', 'claude-opus-4-8', _tokens(2000, 0)),
  ],
  series: [
    UsageBucket(startMs: Int64(0), tokens: _tokens(1000, 0)),
    UsageBucket(startMs: Int64(86400000), tokens: _tokens(0, 0)),
    UsageBucket(startMs: Int64(2 * 86400000), tokens: _tokens(5000, 100)),
  ],
  agentsSinceMs: Int64(DateTime(2026, 11, 20).millisecondsSinceEpoch),
);

void main() {
  test('tokens, short', () {
    expect(formatTokens(0), '0');
    expect(formatTokens(980), '980');
    expect(formatTokens(1000), '1K');
    expect(formatTokens(12345), '12.3K');
    expect(formatTokens(834000), '834K');
    expect(formatTokens(999700), '1M');
    expect(formatTokens(1200000), '1.2M');
    expect(formatTokens(3400000000), '3.4B');
  });

  test(
    'ranges start at local midnight; today goes by hour, the rest by day',
    () {
      final now = DateTime(2026, 11, 20, 15, 30);
      final today = usageAsk(UsageRange.today, now);
      expect(
        today.sinceMs.toInt(),
        DateTime(2026, 11, 20).millisecondsSinceEpoch,
      );
      expect(today.bucketMs.toInt(), 3600000);
      final week = usageAsk(UsageRange.week, now);
      expect(
        week.sinceMs.toInt(),
        DateTime(2026, 11, 14).millisecondsSinceEpoch,
      );
      expect(week.bucketMs.toInt(), 86400000);
      expect(usageAsk(UsageRange.all, now).sinceMs.toInt(), 0);
      expect(today.utcOffsetMinutes, now.timeZoneOffset.inMinutes);
    },
  );

  test('every grouping adds up to the same total', () {
    final rows = _report().rows;
    int sum(List<UsageLine> lines) =>
        lines.fold(0, (total, line) => total + line.total);
    final all = rows.fold(0, (total, row) => total + totalTokens(row.tokens));
    expect(sum(groupUsage(rows, (r) => r.agentId, (r) => r.agentId)), all);
    expect(
      sum(groupUsage(rows, (r) => r.providerId, (r) => r.providerId)),
      all,
    );
    expect(sum(groupUsage(rows, (r) => r.model, (r) => r.model)), all);
    final agents = groupUsage(rows, (r) => r.agentId, (r) => r.agentId);
    expect(agents.first.key, 'june', reason: 'largest first');
    expect(agents.first.requests, 43);
  });

  testWidgets("the Usage pane: what it asks, the total, and each agent, "
      'provider and model', (tester) async {
    final harness = Harness(_workspace());
    harness.backend.usageReport = _report();
    await harness.show(tester);
    harness.page.value = AppPage.usage;
    await settle(tester);
    await tester.pump(); // the report, answered after the first frame

    final ask = harness.backend.sent.lastWhere((m) => m.hasLoadUsage());
    expect(ask.loadUsage.bucketMs.toInt(), 86400000, reason: '7 days');
    expect(find.text('Usage'), findsWidgets);
    // 1,000,000 + 12,000 + 180,000 + 6,000 + 2,000.
    expect(find.text('1.2M  tokens', findRichText: true), findsOneWidget);
    expect(
      find.text(
        'Input 367K · Output 83K · Reused 700K · Kept ready 50K · '
        '46 requests',
      ),
      findsOneWidget,
    );
    for (final name in [
      'June',
      'Mike',
      'Wren',
      'Earlier',
      'Anthropic',
      'ChatGPT',
      'Claude Opus 4.8',
      'Claude Haiku 4.5',
      'GPT-5.5',
    ]) {
      expect(find.text(name), findsWidgets, reason: name);
    }
    expect(
      find.text('Claude Opus 4.8 1M · Claude Haiku 4.5 12K'),
      findsOneWidget,
      reason: "June's models",
    );
    expect(
      find.textContaining('Anything before that shows as Earlier'),
      findsOneWidget,
    );
    expect(find.text('By account'), findsNothing, reason: 'no accounts yet');
    // The ranges are tabs in one row.
    expect(
      tester.getCenter(find.text('All time')).dy,
      tester.getCenter(find.text('Today')).dy,
    );
    expect(
      tester.getTopLeft(find.text('All time')).dx,
      greaterThan(tester.getTopRight(find.text('30 days')).dx),
    );

    await tester.tap(find.text('Today'));
    await settle(tester);
    await tester.pump();
    final today = harness.backend.sent.lastWhere((m) => m.hasLoadUsage());
    expect(today.loadUsage.bucketMs.toInt(), 3600000);

    // Asked again while it shows.
    final asked = harness.backend.sent.where((m) => m.hasLoadUsage()).length;
    await tester.pump(usageRefresh);
    expect(
      harness.backend.sent.where((m) => m.hasLoadUsage()).length,
      asked + 1,
    );
    harness.page.value = AppPage.agent;
    await settle(tester);
  });

  testWidgets('nothing yet, and a refusal, say so', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    harness.page.value = AppPage.usage;
    await settle(tester);
    await tester.pump(); // the report, answered after the first frame
    expect(find.text('No usage in this period.'), findsOneWidget);
    harness.page.value = AppPage.agent;
    await settle(tester);

    harness.backend.refuse = "This Claktory can't count usage yet.";
    harness.page.value = AppPage.usage;
    await settle(tester);
    await tester.pump(); // the report, answered after the first frame
    expect(find.text("This Claktory can't count usage yet."), findsOneWidget);
    harness.page.value = AppPage.agent;
    await settle(tester);
  });

  testWidgets("unreachable, it says so in the app's words with Reconnect, "
      "never the connection's, and asks again once it's back (Zucchero, "
      'Bruno 52171)', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    harness.backend.refuse = "Can't reach your Claktory right now.";
    harness.page.value = AppPage.usage;
    await settle(tester);
    await tester.pump();
    expect(find.text("Can't reach your Claktory right now."), findsNothing);
    expect(find.textContaining("Can't reach"), findsOneWidget);
    expect(find.textContaining('right now.'), findsOneWidget);
    await tester.tap(find.text('Reconnect'));
    expect(harness.backend.reconnectsAsked, 1);
    harness.backend
      ..refuse = null
      ..usageReport = _report()
      ..connected.value = false;
    await tester.pump();
    harness.backend.connected.value = true;
    await settle(tester);
    await tester.pump();
    expect(find.textContaining("Can't reach"), findsNothing);
    expect(find.text('June'), findsWidgets);
    harness.page.value = AppPage.agent;
    await settle(tester);
  });

  testWidgets("the sidebar's Usage button opens it beside the conversation", (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    await openFromMore(tester, 'Usage');
    await settle(tester);
    final layout = harness.backend.sent.lastWhere((m) => m.hasSetLayout());
    expect(layout.setLayout.toString(), contains('usage'));
  });
}
