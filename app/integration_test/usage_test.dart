// The Usage page against a real Claktory (the dev one), on an iPhone (the
// Simulator) already paired with it, or paired with PAIRING_CODE:
//
//   1. Each range's report adds up: every grouping of its rows, and its
//      series, to the same total, and the page shows that total.
//   2. An agent works (AGENT, a cheap model is best): Today's total grows by
//      what its answer used, under its name.
//
//   cd app && flutter test integration_test/usage_test.dart \
//     -d <simulator> --dart-define=FACTORY_DEV=true --dart-define=AGENT=Wren
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/usage_view.dart';
import 'package:flutter/widgets.dart' show EditableText;
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'phone_test.dart' show hold, waitFor;

const _code = String.fromEnvironment('PAIRING_CODE');
const _agent = String.fromEnvironment('AGENT', defaultValue: 'Wren');

/// Holds the screen still at each step, for screenshots.
const _shots = bool.fromEnvironment('SHOTS');

int _total(Iterable<UsageTokens> all) =>
    all.fold(0, (sum, tokens) => sum + totalTokens(tokens));

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('the Usage page adds up, and follows an agent working', (
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
    }
    await waitFor(
      tester,
      find.byType(Sidebar),
      timeout: const Duration(minutes: 1),
    );
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    Future<void> until(
      String what,
      Future<bool> Function() done, {
      Duration timeout = const Duration(minutes: 1),
    }) async {
      final end = DateTime.now().add(timeout);
      while (!await done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await tester.pump(const Duration(milliseconds: 500));
      }
    }

    await until('connected', () async => backend.connected.value);

    // 1. Each range adds up, and the page shows it.
    scope.page.value = AppPage.usage;
    await hold(tester, seconds: 2);
    for (final range in UsageRange.values) {
      await tester.tap(find.text(range.label));
      await hold(tester, seconds: _shots ? 4 : 1);
      final report = await backend.loadUsage(usageAsk(range, DateTime.now()));
      final rows = report.rows;
      final all = _total(rows.map((row) => row.tokens));
      for (final key in <String Function(UsageRow)>[
        (row) => row.agentId,
        (row) => row.providerId,
        (row) => '${row.providerId}/${row.model}',
        (row) => '${row.providerId}/${row.accountId}',
      ]) {
        final lines = groupUsage(rows, key, key);
        expect(lines.fold(0, (sum, line) => sum + line.total), all);
      }
      expect(
        _total(report.series.map((bucket) => bucket.tokens)),
        all,
        reason: '${range.label}: the chart has every token',
      );
      if (all > 0) {
        expect(
          find.text('${formatTokens(all)}  tokens', findRichText: true),
          findsOneWidget,
          reason: range.label,
        );
      }
      // ignore: avoid_print
      print(
        '${range.label}: ${formatTokens(all)} tokens ($all), '
        '${rows.length} rows, ${report.series.length} buckets, '
        'agents since ${DateTime.fromMillisecondsSinceEpoch(report.agentsSinceMs.toInt())}',
      );
      for (final row in rows) {
        // ignore: avoid_print
        print(
          '  ${row.agentId.isEmpty ? 'Earlier' : row.agentName} '
          '${row.providerId}/${row.model} account "${row.accountId}" '
          '(${row.accountLabel}): ${totalTokens(row.tokens)} in '
          '${row.requests} requests',
        );
      }
    }

    // 2. An agent works: Today grows by its answer, under its name.
    final agent = backend.workspace.workspace.agents.firstWhere(
      (a) => a.name == _agent,
    );
    final today = usageAsk(UsageRange.today, DateTime.now());
    int mine(UsageReport report) => _total(
      report.rows.where((r) => r.agentId == agent.id).map((r) => r.tokens),
    );
    final before = await backend.loadUsage(today);
    final total = _total(before.rows.map((row) => row.tokens));
    await backend.sendMessage(
      agent.id,
      'This is a test of usage counting. Reply with only the word "ok".',
    );
    late UsageReport after;
    await until('its answer counted', () async {
      after = await backend.loadUsage(today);
      return mine(after) > mine(before);
    }, timeout: const Duration(minutes: 2));
    final grew = _total(after.rows.map((row) => row.tokens)) - total;
    // ignore: avoid_print
    print(
      '$_agent used ${mine(after) - mine(before)} tokens; '
      'Today grew by $grew',
    );
    expect(grew, greaterThanOrEqualTo(mine(after) - mine(before)));
    await tester.tap(find.text('7 days'));
    await tester.tap(find.text('Today'));
    await hold(tester, seconds: _shots ? 6 : 2);
    expect(find.text(_agent), findsWidgets);
    scope.page.value = AppPage.agent;
    await hold(tester, seconds: 1);
  });
}
