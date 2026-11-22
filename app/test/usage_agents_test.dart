// Usage's Agents working chart: how many agents worked at once over each
// hour or day, over the token chart on the same time axis; pointing at a
// bucket says both, and lights it in each.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/selection.dart' show SelectableArea;
import 'package:factory_app/usage_view.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

UsageBucket _bucket(
  int day,
  int tokens, {
  double working = 0,
  int worked = 0,
}) => UsageBucket(
  startMs: Int64(DateTime(2026, 11, 16 + day).millisecondsSinceEpoch),
  tokens: UsageTokens(input: Int64(tokens)),
  workingAgents: working,
  activeAgents: worked,
);

final _series = [
  _bucket(0, 1000, working: 0.5, worked: 2),
  _bucket(1, 0),
  _bucket(2, 1200000, working: 4.2, worked: 9),
];

Future<void> _show(
  WidgetTester tester,
  List<UsageBucket> series, {
  Size size = const Size(800, 600),
  int since = 0,
}) => Harness(Workspace()).show(
  tester,
  size: size,
  home: Padding(
    padding: const EdgeInsets.all(24),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        UsageChart(series: series, hourly: false, agentsSinceMs: since),
      ],
    ),
  ),
);

/// Points at bucket [index] of [count] in the tokens chart, and holds.
Future<TestGesture> _point(WidgetTester tester, int index, int count) async {
  final chart = tester.getRect(find.byType(CustomPaint).last);
  final gesture = await tester.startGesture(
    Offset(chart.left + chart.width * (index + 0.5) / count, chart.center.dy),
  );
  await tester.pump();
  return gesture;
}

void main() {
  test('how many at once, short', () {
    expect(agentsAtOnce(0), '0');
    expect(agentsAtOnce(0.25), '0.3');
    expect(agentsAtOnce(0.021), '<0.1', reason: 'never 0 when some worked');
    expect(agentsAtOnce(0.05), '0.1');
    expect(agentsAtOnce(1), '1');
    expect(agentsAtOnce(4.2), '4.2');
    expect(agentsAtOnce(12.4), '12');
  });

  test("a bucket's details", () {
    expect(
      bucketDetails(_series[2], 'Wed Nov 18', agents: true),
      'Wed Nov 18 · 4.2 agents at once on average · 9 worked · 1.2M tokens',
    );
    expect(
      bucketDetails(_bucket(0, 10, working: 1, worked: 1), 'X', agents: true),
      'X · 1 agent at once on average · 1 worked · 10 tokens',
    );
    expect(
      bucketDetails(_series[2], 'Wed Nov 18', agents: true, short: true),
      'Wed Nov 18 · 4.2 at once · 9 worked · 1.2M tokens',
    );
    expect(
      bucketDetails(_series[2], 'Wed Nov 18', agents: false),
      'Wed Nov 18 · 1.2M tokens',
    );
  });

  testWidgets('agents working over tokens, one bucket pointed at in both', (
    tester,
  ) async {
    await _show(tester, _series);
    expect(find.text('Agents working'), findsOneWidget);
    expect(find.text('Tokens'), findsOneWidget);
    expect(find.text('Mon Nov 16 – Wed Nov 18'), findsOneWidget);
    // Over the tokens chart, the same width.
    final charts = find.byType(CustomPaint);
    final agents = tester.getRect(charts.at(charts.evaluate().length - 2));
    final tokens = tester.getRect(charts.last);
    expect(agents.bottom, lessThan(tokens.top));
    expect(agents.left, tokens.left);
    expect(agents.width, tokens.width);
    expect(agents.height, 40);
    expect(tokens.height, 72);

    final gesture = await _point(tester, 2, 3);
    expect(
      find.text(
        'Wed Nov 18 · 4.2 agents at once on average · 9 worked · 1.2M tokens',
      ),
      findsOneWidget,
    );
    await gesture.up();
    await tester.pump();
    expect(find.text('Mon Nov 16 – Wed Nov 18'), findsOneWidget);
  });

  test('counted from the later of runs kept and calls by agent', () {
    expect(agentsCountedSince(UsageReport()), 0);
    expect(
      agentsCountedSince(
        UsageReport(workingSinceMs: Int64(20), agentsSinceMs: Int64(10)),
      ),
      20,
    );
    expect(agentsCountedSince(UsageReport(agentsSinceMs: Int64(10))), 10);
  });

  testWidgets('a range from before agents were counted says from when, and '
      "its earlier buckets say nothing of agents", (tester) async {
    final since = DateTime(2026, 11, 17, 9).millisecondsSinceEpoch;
    await _show(tester, _series, since: since);
    expect(find.text('Agents working starts on Nov 17.'), findsOneWidget);
    var gesture = await _point(tester, 0, 3);
    expect(find.text('Mon Nov 16 · 1K tokens'), findsOneWidget);
    await gesture.up();
    // The day counting began in is counted.
    gesture = await _point(tester, 1, 3);
    expect(
      find.text(
        'Tue Nov 17 · 0 agents at once on average · 0 worked · '
        '0 tokens',
      ),
      findsOneWidget,
    );
    await gesture.up();
  });

  testWidgets('a sliver reads "<0.1", and VoiceOver says "under 0.1"', (
    tester,
  ) async {
    await _show(tester, [_bucket(0, 1000, working: 0.021, worked: 1)]);
    final gesture = await _point(tester, 0, 1);
    expect(
      find.text(
        'Mon Nov 16 · <0.1 agents at once on average · 1 worked · '
        '1K tokens',
      ),
      findsOneWidget,
    );
    final caption = tester.widget<Text>(
      find.textContaining('<0.1 agents at once'),
    );
    expect(
      caption.semanticsLabel,
      'Mon Nov 16 · under 0.1 agents at once on average · 1 worked · '
      '1K tokens',
    );
    await gesture.up();
  });

  testWidgets('counted all along: no line', (tester) async {
    await _show(
      tester,
      _series,
      since: DateTime(2026, 11, 16).millisecondsSinceEpoch,
    );
    expect(find.textContaining('starts on'), findsNothing);
  });

  testWidgets('a backend that counts no agents: tokens alone, as before', (
    tester,
  ) async {
    await _show(tester, [_bucket(0, 1000), _bucket(1, 500)]);
    expect(find.text('Agents working'), findsNothing);
    expect(find.text('Tokens'), findsNothing);
    final gesture = await _point(tester, 0, 2);
    expect(find.text('Mon Nov 16 · 1K tokens'), findsOneWidget);
    await gesture.up();
  });

  testWidgets('a long press on a bar reads it, selecting no words nearby', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final selected = <String>[];
    await Harness(Workspace()).show(
      tester,
      size: const Size(393, 852),
      home: SelectableArea(
        onSelectionChanged: (content) {
          if (content != null && content.plainText.isNotEmpty) {
            selected.add(content.plainText);
          }
        },
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              const Text('Input 12K · 10 requests'),
              UsageChart(series: _series, hourly: false, agentsSinceMs: 0),
              const Text('Below the chart'),
            ],
          ),
        ),
      ),
    );
    await pumpFrames(tester);
    final charts = find.byType(CustomPaint);
    for (final chart in [
      charts.at(charts.evaluate().length - 2),
      charts.last,
    ]) {
      final rect = tester.getRect(chart);
      await tester.longPressAt(
        Offset(rect.left + rect.width * 5 / 6, rect.center.dy),
      );
      await tester.pumpAndSettle();
      expect(
        find.text('Wed Nov 18 · 4.2 at once · 9 worked · 1.2M tokens'),
        findsOneWidget,
      );
      expect(selected, isEmpty, reason: 'no word selected');
    }
  });

  testWidgets('on a phone, shorter', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    await _show(tester, _series, size: const Size(393, 852));
    final gesture = await _point(tester, 2, 3);
    const details = 'Wed Nov 18 · 4.2 at once · 9 worked · 1.2M tokens';
    expect(find.text(details), findsOneWidget);
    await gesture.up();
    await tester.pump();
    expect(find.text(details), findsOneWidget, reason: 'stays after a tap');

    // A long press on the agents chart reads its bar too, and selects
    // no words.
    final agents = tester.getRect(
      find
          .byType(CustomPaint)
          .at(find.byType(CustomPaint).evaluate().length - 2),
    );
    await tester.longPressAt(
      Offset(agents.left + agents.width / 6, agents.center.dy),
    );
    await tester.pumpAndSettle();
    expect(
      find.text('Mon Nov 16 · 0.5 at once · 2 worked · 1K tokens'),
      findsOneWidget,
    );
    expect(find.text('Copy'), findsNothing);
  });
}
