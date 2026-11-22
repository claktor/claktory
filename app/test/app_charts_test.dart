// Catalog v1's charts (C3): Chart, Stat and Table drawn from a data
// query's answer bound in the data model, in IDENTITY's look; bounded
// however much an app sends, with a caption for what's left out; a
// readout of what the pointer is on; and what assistive technology hears.
import 'dart:convert';

import 'package:factory_app/app_views/app_view.dart';
import 'package:factory_app/app_views/charts.dart';
import 'package:factory_app/app_views/transport.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' as pb;
import 'package:fl_chart/fl_chart.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_views_test.dart' show FakeAppViewTransport;
import 'harness.dart';

String _snapshot(List<Map<String, Object?>> components, Object? data) =>
    jsonEncode({
      'version': '1.0',
      'createSurface': {
        'surfaceId': 'charts-1',
        'catalogId': appViewCatalogV1,
        'components': components,
        'dataModel': data,
      },
    });

Map<String, Object?> _table(List<String> columns, List<List<Object?>> rows) => {
  'columns': columns,
  'rows': rows,
};

/// Tokens a day for two agents, long: (day, agent, tokens).
final _tokens = _table(
  ['day', 'agent_id', 'tokens'],
  [
    for (var day = 16; day <= 22; day++) ...[
      ['2026-11-$day', 'a1', 1000 * day],
      ['2026-11-$day', 'a2', 500 * day],
    ],
  ],
);

final _workspace = pb.Workspace()
  ..agents.addAll([
    pb.Agent(id: 'a1', name: 'June', color: 'pink'),
    pb.Agent(id: 'a2', name: 'Kai', color: 'teal'),
  ]);

Future<FakeAppViewTransport> _show(
  WidgetTester tester,
  List<Map<String, Object?>> components,
  Object? data, {
  Size size = const Size(1200, 1400),
}) async {
  final transport = FakeAppViewTransport()
    ..answer = (_) => AppSurfaceData(
      surfaceId: 'charts-1',
      appId: 'charts',
      title: 'Charts',
      state: AppViewState.open,
      snapshot: _snapshot(components, data),
      seq: 0,
      error: '',
    );
  await Harness(_workspace).show(
    tester,
    size: size,
    home: AppView(transport: transport, appId: 'charts', appName: 'Charts'),
  );
  await tester.pump();
  await tester.pump(Motion.base);
  return transport;
}

Map<String, Object?> _root(List<String> children) => {
  'id': 'root',
  'component': 'Column',
  'children': children,
};

void main() {
  group('reading a table', () {
    test('a query answer, or nothing', () {
      final table = AppTable.from({
        'columns': [
          'day',
          {'name': 'runs', 'type': 'integer'},
        ],
        'rows': [
          ['2026-11-22', 4],
          ['2026-11-21', null],
        ],
        'more': true,
      })!;
      expect(table.columns, ['day', 'runs']);
      expect(table.more, isTrue);
      expect(table.numeric(1), isTrue);
      expect(table.numeric(0), isFalse);
      expect(AppTable.from('rows'), isNull);
      expect(AppTable.from({'columns': [], 'rows': []}), isNull);
      expect(
        AppTable.from({
          'columns': ['a', 'b'],
          'rows': [
            [1],
          ],
        }),
        isNull,
        reason: 'a row shorter than the columns',
      );
    });

    test('numbers and times as they come from SQL', () {
      expect(shortNumber(980), '980');
      expect(shortNumber(12345), '12.3K');
      expect(shortNumber(123456), '123K');
      expect(shortNumber(1234567), '1.2M');
      expect(shortNumber(0.25), '0.25');
      expect(shortNumber(-3000), '−3K');
      expect(fullNumber(12345.5), '12,345.5');
      expect(fullNumber(1234567), '1,234,567');
      expect(cellTime('2026-11-22')!.day, 22);
      expect(cellTime('2026-11-22 14:30:00')!.hour, 14);
      // unixepoch() seconds, and milliseconds.
      expect(cellTime(1795000000)!.year, cellTime(1795000000000)!.year);
      expect(readoutTime(DateTime(2026, 11, 22)), '22 Nov');
      expect(readoutTime(DateTime(2026, 11, 22, 14, 5)), '22 Nov 14:05');
      expect(niceStep(97), 50);
      expect(niceStep(10), 5);
    });

    test('series, long or wide, bounded', () {
      final long = chartSeries(
        AppTable.from(_tokens)!,
        x: 'day',
        seriesBy: 'agent_id',
      )!;
      expect([for (final one in long.series) one.name], ['a1', 'a2']);
      expect(long.series.first.points.length, 7);

      final wide = chartSeries(
        AppTable.from(
          _table(
            ['day', 'input', 'output', 'note'],
            [
              ['2026-11-22', 10, 4, 'ok'],
            ],
          ),
        )!,
      )!;
      expect([for (final one in wide.series) one.name], ['input', 'output']);

      final many = chartSeries(
        AppTable.from(
          _table(
            ['x', 'who', 'n'],
            [
              for (var i = 0; i < 15; i++) [i, 'agent $i', i],
            ],
          ),
        )!,
        seriesBy: 'who',
      )!;
      expect(many.series.length, maxChartSeries);
      expect(many.seriesLeft, 3);

      expect(
        chartSeries(AppTable.from(_tokens)!, x: 'week'),
        isNull,
        reason: 'a column that isn\'t there',
      );
    });
  });

  testWidgets('a dashboard: a stat with its spark line, tokens a day per '
      'agent in their colors, and a table', (tester) async {
    final semantics = tester.ensureSemantics();
    await _show(
      tester,
      [
        _root(['runs', 'tokens', 'table']),
        {
          'id': 'runs',
          'component': 'Stat',
          'label': 'Runs today',
          'value': {'@path': '/runs'},
          'unit': 'runs',
          'spark': {'@path': '/spark'},
          'color': 'signal',
        },
        {
          'id': 'tokens',
          'component': 'Chart',
          'kind': 'timeseries',
          'data': {'@path': '/tokens'},
          'x': 'day',
          'seriesBy': 'agent_id',
          'color': 'agent',
          'label': 'Tokens by agent',
          'unit': 'tokens',
        },
        {
          'id': 'table',
          'component': 'Table',
          'data': {'@path': '/tokens'},
          'columns': [
            {'name': 'day', 'title': 'Day', 'format': 'time'},
            {'name': 'tokens', 'title': 'Tokens', 'format': 'number'},
          ],
        },
      ],
      {
        'runs': 12345,
        'spark': [1, 4, 2, 8],
        'tokens': _tokens,
      },
    );

    expect(find.text('Runs today'), findsOneWidget);
    expect(find.text('12,345 runs', findRichText: true), findsOneWidget);
    expect(find.text('Tokens by agent'), findsOneWidget);
    expect(find.byType(LineChart), findsNWidgets(2), reason: 'and the spark');
    // The series are agents: their names and their own colors.
    expect(find.text('June'), findsOneWidget);
    expect(find.text('Kai'), findsOneWidget);
    final line = tester
        .widgetList<LineChart>(find.byType(LineChart))
        .firstWhere((chart) => chart.data.lineBarsData.length == 2);
    expect(line.data.lineBarsData.first.color, agentColor('pink'));
    expect(line.data.lineBarsData.last.color, agentColor('teal'));
    // The table: its titles, times as days, numbers grouped.
    expect(find.text('Day'), findsOneWidget);
    expect(find.text('Tokens'), findsOneWidget);
    expect(find.text('22 Nov'), findsWidgets);
    expect(find.text('22,000'), findsOneWidget);

    // What assistive technology hears.
    expect(
      tester.getSemantics(find.byType(AppChart)),
      matchesSemantics(
        label: 'Tokens by agent',
        value:
            'Line chart from 16 Nov to 22 Nov. June: latest 22,000 tokens. '
            'Kai: latest 11,000 tokens.',
      ),
    );
    expect(
      tester.getSemantics(find.byType(AppStat)),
      matchesSemantics(label: 'Runs today', value: '12,345 runs'),
    );
    semantics.dispose();
  });

  testWidgets('pointing at a chart reads out what it is on, in place of its '
      'label', (tester) async {
    await _show(
      tester,
      [
        _root(['bars']),
        {
          'id': 'bars',
          'component': 'Chart',
          'kind': 'bars',
          'data': {'@path': '/runs'},
          'label': 'Runs by agent',
        },
      ],
      {
        'runs': _table(
          ['agent', 'runs'],
          [
            ['Rivet', 12],
            ['Kai', 4],
          ],
        ),
      },
    );
    expect(find.text('Runs by agent'), findsOneWidget);
    final chart = tester.getRect(find.byType(BarChart));
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    addTearDown(mouse.removePointer);
    // Over the first bar's column (40px of value axis at the left).
    await mouse.moveTo(
      Offset(chart.left + 40 + (chart.width - 40) / 4, chart.bottom - 40),
    );
    await tester.pump();
    expect(find.text('Rivet · 12'), findsOneWidget);
    expect(find.text('Runs by agent'), findsNothing);
    // The other bar dims.
    final bars = tester.widget<BarChart>(find.byType(BarChart));
    expect(
      bars.data.barGroups[1].barRods.single.color!.a,
      lessThan(bars.data.barGroups[0].barRods.single.color!.a),
    );
    await mouse.moveTo(Offset.zero);
    await tester.pump();
    expect(find.text('Runs by agent'), findsOneWidget);
  });

  testWidgets('bounded however much an app sends, saying what is left out', (
    tester,
  ) async {
    await _show(
      tester,
      [
        _root(['bars', 'series', 'pie', 'table']),
        {
          'id': 'bars',
          'component': 'Chart',
          'kind': 'stackedBars',
          'data': {'@path': '/many'},
          'x': 'name',
          'y': ['n'],
        },
        {
          'id': 'series',
          'component': 'Chart',
          'kind': 'scatter',
          'data': {'@path': '/spread'},
          'x': 'x',
          'seriesBy': 'who',
        },
        {
          'id': 'pie',
          'component': 'Chart',
          'kind': 'pie',
          'data': {'@path': '/slices'},
        },
        {
          'id': 'table',
          'component': 'Table',
          'data': {'@path': '/many'},
        },
      ],
      {
        'many': _table(
          ['name', 'n'],
          [
            for (var i = 0; i < 250; i++) ['item $i', i],
          ],
        ),
        'spread': _table(
          ['x', 'who', 'y'],
          [
            for (var i = 0; i < 14; i++) [i, 'who $i', i * 2],
          ],
        ),
        'slices': _table(
          ['model', 'tokens'],
          [
            for (var i = 0; i < 14; i++) ['model $i', 100 - i],
          ],
        ),
      },
    );
    expect(find.text('Showing the first 100 of 250 bars.'), findsOneWidget);
    expect(
      tester.widget<BarChart>(find.byType(BarChart)).data.barGroups.length,
      maxChartBars,
    );
    expect(find.text('Showing 12 of 14 series.'), findsOneWidget);
    expect(find.text('The smallest 3 are under Other.'), findsOneWidget);
    expect(
      tester.widget<PieChart>(find.byType(PieChart)).data.sections.length,
      maxPieSlices,
    );
    expect(find.textContaining('Other · '), findsOneWidget);
    expect(find.text('and 50 more'), findsOneWidget);
    expect(find.text('item 199'), findsOneWidget);
    expect(find.text('item 200'), findsNothing);
  });

  testWidgets("data that isn't a table, or no rows, says so", (tester) async {
    await _show(
      tester,
      [
        _root(['bad', 'empty', 'badTable']),
        {
          'id': 'bad',
          'component': 'Chart',
          'kind': 'timeseries',
          'data': {'@path': '/nothing'},
        },
        {
          'id': 'empty',
          'component': 'Chart',
          'kind': 'bars',
          'label': 'Runs',
          'data': {'@path': '/empty'},
        },
        {
          'id': 'badTable',
          'component': 'Table',
          'data': {'@path': '/nothing'},
        },
      ],
      {
        'empty': _table(['day', 'runs'], []),
      },
    );
    expect(find.text("Can't show this chart."), findsOneWidget);
    expect(find.text('Runs'), findsOneWidget);
    expect(find.text('Nothing to show.'), findsOneWidget);
    expect(find.text("Can't show this table."), findsOneWidget);
  });

  testWidgets('it follows the data model as the app refreshes it', (
    tester,
  ) async {
    final transport = await _show(
      tester,
      [
        _root(['runs']),
        {
          'id': 'runs',
          'component': 'Stat',
          'label': 'Runs',
          'value': {'@path': '/runs'},
        },
      ],
      {'runs': 4},
    );
    expect(find.text('4', findRichText: true), findsOneWidget);
    transport.update(1, {
      'updateDataModel': {'path': '/runs', 'value': 2500000},
    });
    await tester.pump();
    expect(find.text('2.5M', findRichText: true), findsOneWidget);
  });

  testWidgets('a pie and a scatter on a phone fit', (tester) async {
    await _show(
      tester,
      [
        _root(['pie', 'scatter']),
        {
          'id': 'pie',
          'component': 'Chart',
          'kind': 'pie',
          'data': {'@path': '/models'},
          'color': 'agent',
        },
        {
          'id': 'scatter',
          'component': 'Chart',
          'kind': 'scatter',
          'data': {'@path': '/runs'},
        },
      ],
      {
        'models': _table(
          ['agent', 'tokens'],
          [
            ['a1', 300],
            ['a2', 100],
          ],
        ),
        'runs': _table(
          ['seconds', 'tokens'],
          [
            [3, 1200],
            [40, 9000],
          ],
        ),
      },
      size: const Size(390, 844),
    );
    expect(tester.takeException(), isNull);
    expect(find.text('June · 75%'), findsOneWidget);
    expect(find.text('Kai · 25%'), findsOneWidget);
    expect(find.byType(ScatterChart), findsOneWidget);
  });
}
