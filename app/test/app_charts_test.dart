// Catalog v1's charts (C3): Chart, Stat and Table drawn from a data
// query's answer bound in the data model, in IDENTITY's look; bounded
// however much an app sends, with a caption for what's left out; a
// readout of what the pointer is on; and what assistive technology hears.
import 'dart:convert';
import 'dart:math' as math;

import 'package:factory_app/app_views/app_view.dart';
import 'package:factory_app/app_views/charts.dart';
import 'package:factory_app/app_views/transport.dart';
import 'package:factory_app/controls.dart'
    show ExtensionGlyph, HoverCard, Pressable;
import 'package:factory_app/identity.dart';
import 'package:factory_app/mobile/device.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' as pb;
import 'package:fl_chart/fl_chart.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart' show RenderParagraph, RendererBinding;
import 'package:flutter/services.dart' show LogicalKeyboardKey;
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

/// A dashboard's row of panels side by side.
/// The numbers on [chart]'s value axis, least first, each with the
/// height of its centre.
List<(double, double)> _valueLabels(WidgetTester tester, Finder chart) => [
  for (final e
      in find.descendant(of: chart, matching: find.byType(Text)).evaluate())
    if (double.tryParse((e.widget as Text).data ?? '') case final value?)
      (
        value,
        (e.renderObject! as RenderBox)
            .localToGlobal(
              (e.renderObject! as RenderBox).size.center(Offset.zero),
            )
            .dy,
      ),
]..sort((a, b) => a.$1.compareTo(b.$1));

final _evenRow = find.byWidgetPredicate(
  (widget) => widget.runtimeType.toString() == '_EvenRow',
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
    // In its place: the label keeps its room, unseen.
    expect(find.text('Runs by agent').hitTestable(), findsNothing);
    expect(
      tester
          .widget<Visibility>(
            find.ancestor(
              of: find.text('Runs by agent'),
              matching: find.byType(Visibility),
            ),
          )
          .visible,
      isFalse,
    );
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

  Future<void> showBars(WidgetTester tester, List<String> days) => _show(
    tester,
    [
      _root(['bars']),
      {
        'id': 'bars',
        'component': 'Chart',
        'kind': 'bars',
        'data': {'@path': '/runs'},
        'label': 'Runs by day',
      },
    ],
    {
      'runs': _table(
        ['day', 'runs'],
        [
          for (final (k, day) in days.indexed) [day, k + 1],
        ],
      ),
    },
  );

  testWidgets("bars over days date their axis as a line chart's does, "
      '"10/27"; the readout keeps its words (Zucchero row 378)', (
    tester,
  ) async {
    await showBars(tester, ['2026-10-27', '2026-10-28', '2026-10-29']);
    expect(find.text('10/27'), findsOneWidget);
    expect(find.text('27 Oct'), findsNothing);
    final chart = tester.getRect(find.byType(BarChart));
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    addTearDown(mouse.removePointer);
    await mouse.moveTo(
      Offset(chart.left + 40 + (chart.width - 40) / 6, chart.bottom - 40),
    );
    await tester.pump();
    expect(find.text('27 Oct · 1'), findsOneWidget);
  });

  testWidgets('bars over hours within a day show the hour', (tester) async {
    await showBars(tester, ['2026-10-27 13:00:00', '2026-10-27 14:00:00']);
    expect(find.text('13:00'), findsOneWidget);
    expect(find.text('14:00'), findsOneWidget);
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

  Future<void> showPages(
    WidgetTester tester,
    List<List<Object?>> rows, {
    Size size = const Size(480, 844),
  }) => _show(
    tester,
    [
      _root(['pages']),
      {
        'id': 'pages',
        'component': 'Table',
        'data': {'@path': '/pages'},
        'columns': [
          {'name': 'page', 'title': 'Page', 'format': 'text'},
          {'name': 'views', 'title': 'Views', 'format': 'number'},
        ],
      },
    ],
    {
      'pages': _table(['page', 'views'], rows),
    },
    size: size,
  );

  RenderParagraph paragraph(WidgetTester tester, String text) =>
      tester.renderObject(
        find.descendant(of: find.text(text), matching: find.byType(RichText)),
      );

  testWidgets('a long cell wraps to 3 lines, then "…"; a tap on its row '
      'opens it whole and another closes it; a row with nothing cut takes no '
      'tap; numbers whole, nothing scrolls sideways (friend bug #62)', (
    tester,
  ) async {
    const long =
        'Rebuilding the release images for the Android beta, signing them '
        'with the upload key, uploading them to the Play Console and then '
        'telling Bruno which build number went out';
    const short = 'Two lines of words, wrapped in the column';
    final semantics = tester.ensureSemantics();
    await showPages(tester, [
      [long, 1234567],
      [short, 3],
      ['Home', 12],
    ]);
    expect(tester.takeException(), isNull);
    final cell = tester.widget<Text>(find.text(long));
    expect(cell.maxLines, AppTableView.cellLines);
    expect(paragraph(tester, long).didExceedMaxLines, isTrue);
    final lineHeight = tester.getSize(find.text('Home')).height;
    expect(
      tester.getSize(find.text(long)).height,
      closeTo(lineHeight * 3, 0.5),
    );
    // A shorter one wraps without "…", and its row takes no tap.
    expect(paragraph(tester, short).didExceedMaxLines, isFalse);
    expect(tester.getSize(find.text(short)).height, greaterThan(lineHeight));
    expect(paragraph(tester, '1,234,567').didExceedMaxLines, isFalse);
    // Beside a wrapped cell, the row's others read from its first line.
    expect(
      tester.getTopLeft(find.text('1,234,567')).dy,
      tester.getTopLeft(find.text(long)).dy,
    );
    expect(
      tester.getRect(find.text('1,234,567')).right,
      lessThanOrEqualTo(480),
    );
    expect(
      find.descendant(
        of: find.byType(AppTableView),
        matching: find.byType(Scrollable),
      ),
      findsNothing,
    );
    // Read whole.
    expect(find.bySemanticsLabel(long), findsOneWidget);
    // A row with nothing cut: no hand, no tap.
    for (final whole in ['Home', short]) {
      expect(
        find.ancestor(
          of: find.text(whole),
          matching: find.byType(FocusableActionDetector),
        ),
        findsNothing,
      );
    }
    // A tap opens the row, tinted; another closes it.
    await tester.tap(find.text('1,234,567'));
    await tester.pump();
    expect(tester.widget<Text>(find.text(long)).maxLines, isNull);
    expect(paragraph(tester, long).didExceedMaxLines, isFalse);
    expect(
      tester.getSize(find.text(long)).height,
      greaterThan(lineHeight * 3.5),
    );
    expect(
      find.ancestor(
        of: find.text(long),
        matching: find.byWidgetPredicate(
          (w) => w is ColoredBox && w.color.a > 0,
        ),
      ),
      findsOneWidget,
    );
    await tester.tap(find.text(long));
    await tester.pump();
    expect(tester.widget<Text>(find.text(long)).maxLines, 3);
    semantics.dispose();
  });

  testWidgets('on a Mac, a row that opens shows the hand, and ⏎ opens it '
      'when focused (friend bug #62)', (tester) async {
    const long =
        'Rebuilding the release images for the Android beta, signing them '
        'with the upload key, uploading them to the Play Console and then '
        'telling Bruno which build number went out';
    await showPages(tester, [
      [long, 1],
    ]);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: tester.getCenter(find.text(long)));
    addTearDown(mouse.removePointer);
    await tester.pump();
    expect(
      RendererBinding.instance.mouseTracker.debugDeviceActiveCursor(1),
      SystemMouseCursors.click,
    );
    Focus.of(tester.element(find.text(long))).requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(tester.widget<Text>(find.text(long)).maxLines, isNull);
  });

  testWidgets('a table whose columns don\'t fit at their least reads as '
      'blocks: the first cell its title, each other column its heading and '
      'value, short ones beside it, long ones under it; one element read '
      'whole; nothing cut to a letter (friend bug #62, Scolippi 31339)', (
    tester,
  ) async {
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    const long = 'Compare the phone bar against Grafana at the largest size';
    final semantics = tester.ensureSemantics();
    await showPages(tester, [
      [long, 12],
      ['Home', null],
    ], size: const Size(320, 844));
    expect(tester.takeException(), isNull);
    final place = tester.getRect(find.byType(AppTableView));
    // The title whole, wrapped, never cut.
    final title = tester.widget<Text>(find.text(long));
    expect(title.maxLines, isNull);
    expect(title.style!.fontWeight, FontWeight.w500);
    expect(
      tester.getRect(find.text(long)).right,
      lessThanOrEqualTo(place.right + 0.5),
    );
    // Views and its heading too wide for one line at this size: under it,
    // inside the place.
    final views = tester.getRect(find.text('12'));
    final heading = tester.getRect(find.text('Views'));
    expect(views.right, lessThanOrEqualTo(place.right + 0.5));
    expect(views.top, greaterThanOrEqualTo(heading.bottom - 0.5));
    expect(heading.top, greaterThan(tester.getRect(find.text(long)).bottom));
    // An empty cell leaves its line out: one "Views" only.
    expect(find.text('Views'), findsOneWidget);
    // No heading row: Page isn't shown.
    expect(find.text('Page'), findsNothing);
    expect(find.bySemanticsLabel('$long. Views, 12'), findsOneWidget);
    expect(find.bySemanticsLabel('Home'), findsOneWidget);
    semantics.dispose();
  });

  testWidgets('in blocks a long value goes under its heading, 3 lines and '
      '"…", and a tap on the block opens it (friend bug #62)', (tester) async {
    const long =
        'Rebuilding the release images for the Android beta, signing them '
        'with the upload key, uploading them to the Play Console and then '
        'telling Bruno which build number went out';
    await _show(
      tester,
      [
        _root(['agents']),
        {
          'id': 'agents',
          'component': 'Table',
          'data': {'@path': '/agents'},
        },
      ],
      {
        'agents': _table(
          ['Agent', 'Status', 'Last step', 'What it is doing'],
          [
            ['Pericolo', 'Working', 1402, long],
          ],
        ),
      },
      size: const Size(390, 844),
    );
    expect(tester.takeException(), isNull);
    final place = tester.getRect(find.byType(AppTableView));
    final label = tester.getRect(find.text('What it is doing'));
    final value = tester.getRect(find.text(long));
    expect(value.top, greaterThanOrEqualTo(label.bottom));
    expect(value.left, place.left);
    expect(tester.widget<Text>(find.text(long)).maxLines, 3);
    expect(paragraph(tester, long).didExceedMaxLines, isTrue);
    // Short ones beside their heading, at the right edge.
    for (final (heading, value) in [
      ('Status', 'Working'),
      ('Last step', '1,402'),
    ]) {
      final at = tester.getRect(find.text(value));
      expect(at.right, closeTo(place.right, 0.5), reason: value);
      expect(
        at.center.dy,
        closeTo(tester.getRect(find.text(heading)).center.dy, 3),
        reason: value,
      );
    }
    await tester.tap(find.text('Pericolo'));
    await tester.pump();
    expect(tester.widget<Text>(find.text(long)).maxLines, isNull);
    expect(paragraph(tester, long).didExceedMaxLines, isFalse);
  });

  testWidgets('a narrow table keeps its cells whole and spreads its columns '
      "across its place, as Grafana's share a panel's width (Scolippi #14)", (
    tester,
  ) async {
    await _show(
      tester,
      [
        _root(['pages']),
        {
          'id': 'pages',
          'component': 'Table',
          'data': {'@path': '/pages'},
        },
      ],
      {
        'pages': _table(
          ['page', 'views'],
          [
            ['Home', 12],
          ],
        ),
      },
    );
    expect(tester.widget<Text>(find.text('Home')).maxLines, isNull);
    final table = tester.getRect(find.byType(AppTableView));
    expect(table.width, greaterThan(1000));
    // The number at the right edge, the words at the left, the two
    // columns half each.
    expect(tester.getRect(find.text('12')).right, closeTo(table.right, 0.5));
    expect(tester.getRect(find.text('views')).right, closeTo(table.right, 0.5));
    expect(tester.getRect(find.text('Home')).left, table.left);
    double column(String text) => tester
        .widget<SizedBox>(
          find
              .ancestor(
                of: find.text(text),
                matching: find.byWidgetPredicate(
                  (w) => w is SizedBox && w.width != null,
                ),
              )
              .first,
        )
        .width!;
    expect(column('Home'), closeTo(table.width / 2, 0.5));
    expect(column('Home') + column('12'), closeTo(table.width, 0.01));
  });

  testWidgets('a table with a height fills it, as a Grafana table panel '
      'does: its heading stays and its rows scroll inside; a few rows '
      'leave room under them (Zucchero row 454)', (tester) async {
    await _show(
      tester,
      [
        _root(['many', 'few', 'plain']),
        {
          'id': 'many',
          'component': 'Table',
          'data': {'@path': '/many'},
          'height': 200,
        },
        {
          'id': 'few',
          'component': 'Table',
          'data': {'@path': '/few'},
          'height': 300,
        },
        {
          'id': 'plain',
          'component': 'Table',
          'data': {'@path': '/few'},
        },
      ],
      {
        'many': _table(
          ['country', 'users'],
          [
            for (var i = 1; i <= 30; i++) ['Country $i', 100 - i],
          ],
        ),
        'few': _table(
          ['place', 'visits'],
          [
            ['Finland', 1],
            ['Brazil', 1],
          ],
        ),
      },
      size: const Size(393, 1400),
    );
    final tables = find.byType(AppTableView);
    expect(tester.getSize(tables.at(0)).height, 200);
    expect(tester.getSize(tables.at(1)).height, 300);
    // Without one, as tall as its rows.
    expect(tester.getSize(tables.at(2)).height, lessThan(150));
    final many = tester.getRect(tables.at(0));
    final heading = find.text('country');
    final headingAt = tester.getRect(heading).top;
    final last = find.text('Country 30');
    expect(tester.getRect(last).top, greaterThan(many.bottom));
    // Scrolled inside: the last row comes up within the table, under its
    // heading, which stays.
    await tester.drag(find.text('Country 3'), const Offset(0, -2000));
    await tester.pumpAndSettle();
    expect(tester.getRect(heading).top, headingAt);
    expect(tester.getRect(last).bottom, lessThanOrEqualTo(many.bottom));
    expect(
      tester.getRect(find.text('Country 3')).top,
      lessThan(tester.getRect(heading).bottom),
    );
    expect(tester.getRect(tables.at(0)), many);
    // A few rows at the top of their place, the room under them.
    final few = tester.getRect(tables.at(1));
    expect(
      tester.getRect(find.text('Brazil').first).bottom,
      lessThan(few.top + 120),
    );
  });

  testWidgets('a drag past the end of a table that scrolls inside goes on '
      'to scroll the page, as on iOS (Zucchero row 467)', (tester) async {
    await _show(
      tester,
      [
        _root(['many', 'more']),
        {
          'id': 'many',
          'component': 'Table',
          'data': {'@path': '/many'},
          'height': 200,
        },
        {
          'id': 'more',
          'component': 'Table',
          'data': {'@path': '/many'},
          'height': 480,
        },
      ],
      {
        'many': _table(
          ['country', 'users'],
          [
            for (var i = 1; i <= 40; i++) ['Row $i', 100 - i],
          ],
        ),
      },
      size: const Size(393, 500),
    );
    final page = tester.state<ScrollableState>(find.byType(Scrollable).first);
    expect(page.position.maxScrollExtent, greaterThan(100));
    // Within the table: only the table scrolls.
    await tester.drag(find.text('Row 3').first, const Offset(0, -200));
    await tester.pumpAndSettle();
    expect(page.position.pixels, 0);
    // Past its end: the page carries on.
    await tester.drag(find.text('Row 8').first, const Offset(0, -3000));
    await tester.pumpAndSettle();
    expect(page.position.pixels, greaterThan(100));
    expect(
      tester.getRect(find.text('Row 40').first).bottom,
      lessThanOrEqualTo(tester.getRect(find.byType(AppTableView).first).bottom),
    );
  });

  testWidgets("a panel's title over a table or an error draws as a chart's "
      'label, as near what it titles, so their headings line up; a Column '
      'without one keeps its spacing (Scolippi T6)', (tester) async {
    final semantics = tester.ensureSemantics();
    await _show(
      tester,
      [
        _root(['row', 'plain']),
        {
          'id': 'row',
          'component': 'Row',
          'align': 'start',
          'children': ['chart', 'table', 'error'],
          'stackBelow': 768,
        },
        {'id': 'chart', 'component': 'Card', 'child': 'chart_b', 'weight': 4},
        {
          'id': 'chart_b',
          'component': 'Chart',
          'kind': 'bars',
          'label': 'Runs',
          'data': {'@path': '/t'},
          'height': 200,
        },
        {'id': 'table', 'component': 'Card', 'child': 'table_c', 'weight': 4},
        {
          'id': 'table_c',
          'component': 'Column',
          'children': ['table_t', 'table_b'],
        },
        {
          'id': 'table_t',
          'component': 'Text',
          'text': 'Countries',
          'variant': 'title',
        },
        {
          'id': 'table_b',
          'component': 'Table',
          'data': {'@path': '/t'},
          'height': 200,
        },
        {'id': 'error', 'component': 'Card', 'child': 'error_c', 'weight': 4},
        {
          'id': 'error_c',
          'component': 'Column',
          'children': ['error_t', 'error_b'],
        },
        {
          'id': 'error_t',
          'component': 'Text',
          'text': 'Agents',
          'variant': 'title',
        },
        {'id': 'error_b', 'component': 'Text', 'text': "Couldn't load."},
        {
          'id': 'plain',
          'component': 'Column',
          'children': ['one', 'two'],
        },
        {'id': 'one', 'component': 'Text', 'text': 'One line'},
        {'id': 'two', 'component': 'Text', 'text': 'Another'},
      ],
      {
        't': _table(
          ['day', 'runs'],
          [
            ['Mon', 3],
            ['Tue', 5],
          ],
        ),
      },
    );
    final label = tester.widget<Text>(find.text('Runs').first);
    for (final title in ['Countries', 'Agents']) {
      final text = tester.widget<Text>(find.text(title));
      expect(text.style, label.style, reason: title);
      expect(text.maxLines, 2, reason: title);
      expect(
        tester.getSemantics(find.text(title)).flagsCollection.isHeader,
        isTrue,
      );
    }
    // Level: each title at its card's top, the table's heading as near
    // under its title as the chart's plot is under its label.
    final runs = tester.getRect(find.text('Runs').first);
    final countries = tester.getRect(find.text('Countries'));
    expect(countries.top, runs.top);
    final chart = tester.getRect(
      find.descendant(
        of: find.byType(AppChart),
        matching: find.byType(BarChart),
      ),
    );
    final table = tester.getRect(find.byType(AppTableView));
    expect(table.top - countries.bottom, Space.xs);
    expect(chart.top - runs.bottom, Space.xs);
    expect(
      tester.getRect(find.text("Couldn't load.")).top -
          tester.getRect(find.text('Agents')).bottom,
      Space.xs,
    );
    // Without a title, a Column's own spacing.
    expect(
      tester.getRect(find.text('Another')).top -
          tester.getRect(find.text('One line')).bottom,
      Space.s,
    );
    semantics.dispose();
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
          'label': 'Tokens by agent',
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
    // Under its label, so a dashboard says which one.
    expect(find.text('Tokens by agent'), findsOneWidget);
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

  testWidgets("new rows redraw a time series at once, as Grafana's do; bars "
      'too while the window is hidden (a muted ticker)', (tester) async {
    final day = DateTime(2026, 11, 23, 22, 55).millisecondsSinceEpoch;
    final transport = FakeAppViewTransport()
      ..answer = (_) => AppSurfaceData(
        surfaceId: 'charts-1',
        appId: 'charts',
        title: 'Charts',
        state: AppViewState.open,
        snapshot: _snapshot(
          [
            _root(['ms', 'bars']),
            {
              'id': 'ms',
              'component': 'Chart',
              'kind': 'timeseries',
              'data': {'@path': '/d'},
              'x': 'time',
            },
            {
              'id': 'bars',
              'component': 'Chart',
              'kind': 'bars',
              'data': {'@path': '/b'},
              'x': 'day',
            },
          ],
          {
            'd': _table(
              ['time', 'runs'],
              [
                [day - 86400000 * 3, 26],
                [day - 86400000 * 2, 20],
              ],
            ),
            'b': _table(
              ['day', 'runs'],
              [
                ['mon', 1],
                ['tue', 2],
              ],
            ),
          },
        ),
        seq: 0,
        error: '',
      );
    await Harness(_workspace).show(
      tester,
      home: TickerMode(
        enabled: false,
        child: AppView(
          transport: transport,
          appId: 'charts',
          appName: 'Charts',
        ),
      ),
    );
    await tester.pump();
    transport
      ..update(1, {
        'updateDataModel': {
          'path': '/d',
          'value': _table(
            ['time', 'runs'],
            [
              for (var i = 0; i < 20; i++) [day + i * 300000, (i * 7) % 5],
            ],
          ),
        },
      })
      ..update(2, {
        'updateDataModel': {
          'path': '/b',
          'value': _table(
            ['day', 'runs'],
            [
              ['mon', 5],
              ['tue', 6],
              ['wed', 7],
            ],
          ),
        },
      });
    await tester.pump();
    await tester.pump();
    // What's painted (the chart's animated data), not only what it's given.
    T drawn<T>(String leaf) =>
        (tester.renderObject(
              find.byWidgetPredicate((w) => w.runtimeType.toString() == leaf),
            ) as dynamic).data
            as T;
    final line = drawn<LineChartData>('LineChartLeaf');
    expect(line.lineBarsData.first.spots, hasLength(20));
    expect(line.minX, day);
    final bars = drawn<BarChartData>('BarChartLeaf');
    expect(bars.barGroups, hasLength(3));
    expect(bars.barGroups.last.barRods.first.toY, 7);
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

  testWidgets("bars' categories read in the table's order, a millisecond "
      'time as a time', (tester) async {
    final at = [
      for (final hour in [12, 15, 18]) DateTime(2026, 11, 20, hour),
    ].map((time) => time.millisecondsSinceEpoch).toList();
    await _show(
      tester,
      [
        _root(['bars']),
        {
          'id': 'bars',
          'component': 'Chart',
          'kind': 'stackedBars',
          'data': {'@path': '/runs'},
          'x': 'hour',
          'seriesBy': 'agent',
        },
      ],
      {
        // Kai's first run comes after June's last bucket: still in order.
        'runs': _table(
          ['hour', 'agent', 'runs'],
          [
            [at[0], 'June', 1],
            [at[1], 'June', 2],
            [at[1], 'Kai', 3],
            [at[2], 'Kai', 4],
          ],
        ),
      },
    );
    final bars = tester.widget<BarChart>(find.byType(BarChart));
    expect(bars.data.barGroups, hasLength(3));
    // The last bucket holds only Kai's 4: it comes last, not before his 3.
    expect(bars.data.barGroups.last.barRods.single.toY, 4);
    expect(find.text('12:00'), findsOneWidget);
    expect(find.textContaining('1,7'), findsNothing, reason: 'no raw ms');
  });

  testWidgets("a half-width pie on a phone fits beside its legend", (
    tester,
  ) async {
    await _show(
      tester,
      [
        {
          'id': 'root',
          'component': 'Row',
          'children': ['pie', 'other'],
        },
        {
          'id': 'pie',
          'component': 'Chart',
          'kind': 'pie',
          'data': {'@path': '/models'},
          'weight': 1,
        },
        {'id': 'other', 'component': 'Text', 'text': 'Beside it', 'weight': 1},
      ],
      {
        'models': _table(
          ['model', 'tokens'],
          [
            ['Opus', 300],
            ['Sonnet', 100],
          ],
        ),
      },
      size: const Size(390, 844),
    );
    expect(tester.takeException(), isNull);
    expect(find.text('Opus · 75%'), findsOneWidget);
  });

  testWidgets("a dashboard's weighted stats go one under another on a "
      'phone, side by side on a Mac; an unweighted child keeps its width', (
    tester,
  ) async {
    final components = [
      {
        'id': 'root',
        'component': 'Column',
        'children': ['grid', 'pair'],
      },
      {
        'id': 'grid',
        'component': 'Row',
        'children': ['s1', 's2', 's3', 's4'],
      },
      for (final id in ['s1', 's2', 's3', 's4'])
        {
          'id': id,
          'component': 'Text',
          'text': 'Tokens this week',
          'weight': 3,
        },
      {
        'id': 'pair',
        'component': 'Row',
        'children': ['wide', 'tag'],
      },
      {'id': 'wide', 'component': 'Text', 'text': 'Wide', 'weight': 1},
      {'id': 'tag', 'component': 'Text', 'text': 'Tag'},
    ];
    await _show(tester, components, null, size: const Size(390, 844));
    final stats = find.text('Tokens this week');
    final tops = {
      for (final element in stats.evaluate())
        tester.getTopLeft(find.byWidget(element.widget)).dy,
    };
    expect(tops, hasLength(4), reason: 'one under another');
    // Beside a weighted one, not a sliver of its width.
    expect(tester.getSize(find.text('Tag')).width, greaterThan(40));

    await _show(tester, components, null);
    final wide = {
      for (final element in stats.evaluate())
        tester.getTopLeft(find.byWidget(element.widget)).dy,
    };
    expect(wide, hasLength(1), reason: 'side by side');
  });

  List<Map<String, Object?>> statRow(
    List<String> titles, {
    bool gap = false,
    double? stackBelow = 768,
  }) => [
    _root(['grid']),
    {
      'id': 'grid',
      'component': 'Row',
      'stackBelow': ?stackBelow,
      'children': [
        for (var i = 0; i < titles.length; i++) 'c$i',
        if (gap) 'gap',
      ],
    },
    for (final (i, title) in titles.indexed) ...[
      {'id': 'c$i', 'component': 'Card', 'child': 's$i', 'weight': 4},
      {
        'id': 's$i',
        'component': 'Stat',
        'label': title,
        'value': i + 1,
        'height': 100,
      },
    ],
    if (gap) {'id': 'gap', 'component': 'Column', 'children': [], 'weight': 4},
  ];

  /// The Stat cards' lines: how many cards each has, top to bottom.
  List<int> statLines(WidgetTester tester) {
    final tops = <double, int>{};
    for (final card in find.byType(AppStatCard).evaluate()) {
      final top = tester.getTopLeft(find.byWidget(card.widget)).dy;
      tops[top] = (tops[top] ?? 0) + 1;
    }
    return [for (final top in tops.keys.toList()..sort()) tops[top]!];
  }

  testWidgets("a Stat's title takes 2 lines, its panel growing by the "
      'second; past 2 it ends in "…", whole on a hold and read whole '
      '(friend bug #62)', (tester) async {
    const one = 'Spend';
    const two = 'Agents working right now on this Claktory';
    final three = 'Tokens used today by every agent on this Claktory ' * 3;
    final semantics = tester.ensureSemantics();
    await _show(
      tester,
      statRow([one, two, three]),
      null,
      size: const Size(390, 1400),
    );
    expect(tester.takeException(), isNull);
    expect(statLines(tester), [1, 1, 1], reason: 'one under another');
    double tall(String title) => tester
        .getSize(
          find.ancestor(
            of: find.text(title),
            matching: find.byType(AppStatCard),
          ),
        )
        .height;
    final line = tester.getSize(find.text(one)).height;
    expect(tester.getSize(find.text(two)).height, closeTo(line * 2, 0.5));
    expect(tall(two) - tall(one), closeTo(line, 0.5));
    expect(tall(three), closeTo(tall(two), 0.5));
    // Only the one past 2 lines is cut, with the whole on a hold.
    expect(
      tester
          .renderObject<RenderParagraph>(
            find.descendant(
              of: find.text(three),
              matching: find.byType(RichText),
            ),
          )
          .didExceedMaxLines,
      isTrue,
    );
    for (final (title, held) in [
      (two, findsNothing),
      (three, findsOneWidget),
    ]) {
      expect(
        find.ancestor(of: find.text(title), matching: find.byType(HoverCard)),
        held,
        reason: title,
      );
    }
    expect(find.bySemanticsLabel(three), findsOneWidget);
    semantics.dispose();
  });

  testWidgets('a row of Stat cards too narrow for 160 each, or for its '
      'titles on 2 lines, takes fewer to a line, balanced, the gaps left out; '
      'wide, it stays one line; below stackBelow, one under another '
      '(friend bug #62)', (tester) async {
    final short = [for (var i = 0; i < 6; i++) 'Runs $i'];
    // An iPad in portrait: 6 to a line would be about 140 each.
    await _show(tester, statRow(short), null, size: const Size(900, 1200));
    expect(tester.takeException(), isNull);
    expect(statLines(tester), [3, 3]);
    // Every card in a line as wide as the others, the line's width full.
    final first = find
        .byType(AppStatCard)
        .evaluate()
        .map((e) => tester.getRect(find.byWidget(e.widget)));
    expect(first.map((r) => r.width.round()).toSet(), hasLength(1));
    // 5 and a gap: 3 + 2, the gap gone.
    await tester.pumpWidget(const SizedBox());
    await _show(
      tester,
      statRow(short.take(5).toList(), gap: true),
      null,
      size: const Size(900, 1200),
    );
    expect(statLines(tester), [3, 2]);
    final last = tester.getRect(find.byType(AppStatCard).last);
    expect(
      last.right,
      closeTo(tester.getRect(find.byType(AppStatCard).at(2)).right, 0.5),
    );
    // Titles that would need a third line at 3 to a line: 2 to a line.
    final long = [
      for (var i = 0; i < 6; i++) 'Tokens used today by all of the agents $i',
    ];
    await tester.pumpWidget(const SizedBox());
    await _show(tester, statRow(long), null, size: const Size(900, 1200));
    expect(statLines(tester), [2, 2, 2]);
    for (final title in long) {
      expect(
        tester
            .renderObject<RenderParagraph>(
              find.descendant(
                of: find.text(title),
                matching: find.byType(RichText),
              ),
            )
            .didExceedMaxLines,
        isFalse,
        reason: title,
      );
    }
    // Wide: one line, as before.
    await tester.pumpWidget(const SizedBox());
    await _show(tester, statRow(short), null, size: const Size(1600, 1200));
    expect(statLines(tester), [6]);
    // Below stackBelow: one under another, as Grafana's phone layout.
    await tester.pumpWidget(const SizedBox());
    await _show(tester, statRow(short), null, size: const Size(700, 1600));
    expect(statLines(tester), [1, 1, 1, 1, 1, 1]);
    // Without stackBelow, the same rule rather than one per line.
    await tester.pumpWidget(const SizedBox());
    await _show(
      tester,
      statRow(short, stackBelow: null),
      null,
      size: const Size(700, 1600),
    );
    expect(statLines(tester), [3, 3]);
  });

  testWidgets('at the largest text on a phone, half-width charts stack, '
      "and their axes' words stay whole and under their labels", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final components = [
      _root(['pair']),
      {
        'id': 'pair',
        'component': 'Row',
        'children': ['tokens', 'runs'],
      },
      {
        'id': 'tokens',
        'component': 'Chart',
        'kind': 'timeseries',
        'data': {'@path': '/tokens'},
        'x': 'day',
        'seriesBy': 'agent_id',
        'label': 'Tokens by agent',
        'weight': 6,
      },
      {
        'id': 'runs',
        'component': 'Chart',
        'kind': 'bars',
        'data': {'@path': '/runs'},
        'label': 'Runs by agent',
        'weight': 6,
      },
    ];
    await _show(tester, components, {
      'tokens': _tokens,
      'runs': _table(
        ['agent', 'runs'],
        [
          for (var day = 16; day <= 22; day++) ['2026-11-$day', day],
        ],
      ),
    }, size: const Size(393, 852));
    expect(tester.takeException(), isNull);
    final tokens = tester.getRect(find.text('Tokens by agent'));
    final runs = tester.getRect(find.text('Runs by agent'));
    expect(runs.top, greaterThan(tokens.bottom), reason: 'stacked');
    // Every axis word on one line, below its chart's label.
    for (final label in [tokens, runs]) {
      for (final element in find.byType(Text).evaluate()) {
        final text = element.widget as Text;
        if (text.maxLines != 1 || text.softWrap != false) continue;
        final rect = tester.getRect(find.byWidget(text));
        expect(
          rect.overlaps(label),
          isFalse,
          reason: '"${text.data}" over a label',
        );
      }
    }
    final axis = find.byWidgetPredicate(
      (w) => w is Text && w.data == '20K' && w.softWrap == false,
    );
    expect(axis, findsWidgets);
    expect(
      tester.getSize(axis.first).height,
      lessThan(40),
      reason: 'one line, not a character a line',
    );
  });

  testWidgets("at the largest text on a phone, a chart's long label takes "
      "two lines, and a table's headings wrap between words to fit", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 2;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final components = [
      _root(['tokens', 'tasks']),
      {
        'id': 'tokens',
        'component': 'Chart',
        'kind': 'timeseries',
        'data': {'@path': '/tokens'},
        'x': 'day',
        'seriesBy': 'agent_id',
        'label': 'Tokens by agent this week',
      },
      {
        'id': 'tasks',
        'component': 'Table',
        'data': {'@path': '/tasks'},
      },
    ];
    await _show(tester, components, {
      'tokens': _tokens,
      'tasks': _table(['Agent', 'Task', 'Command started'], []),
    }, size: const Size(500, 1400));
    expect(tester.takeException(), isNull);
    RenderParagraph paragraph(String text) =>
        tester.renderObject<RenderParagraph>(
          find.descendant(of: find.text(text), matching: find.byType(RichText)),
        );
    // (The test font's letters are each as wide as they're tall: at 2x on
    // 500 it takes two lines, as the largest does on a phone.)
    final label = paragraph('Tokens by agent this week');
    expect(label.didExceedMaxLines, isFalse, reason: 'not cut');
    expect(
      tester.getSize(find.text('Tokens by agent this week')).height,
      greaterThan(tester.getSize(find.text('Agent')).height * 1.5),
    );
    // Every heading whole on screen, the long one over two lines; the
    // table no wider than its place.
    for (final heading in ['Agent', 'Task', 'Command started']) {
      expect(paragraph(heading).didExceedMaxLines, isFalse);
      expect(tester.getRect(find.text(heading)).right, lessThanOrEqualTo(500));
    }
    expect(
      tester.getSize(find.text('Command started')).height,
      greaterThan(tester.getSize(find.text('Agent')).height * 1.5),
    );
    // Nothing scrolls sideways.
    expect(
      find.descendant(
        of: find.byType(AppTableView),
        matching: find.byType(Scrollable),
      ),
      findsNothing,
    );
  });

  testWidgets('chips are as wide as their words, several to a line', (
    tester,
  ) async {
    await _show(
      tester,
      [
        _root(['show']),
        {
          'id': 'show',
          'component': 'ChoicePicker',
          'label': 'Show as',
          'variant': 'mutuallyExclusive',
          'displayStyle': 'chips',
          'options': [
            for (final word in ['Bars', 'Lines', 'Pie'])
              {'label': word, 'value': word.toLowerCase()},
          ],
          'value': {'@path': '/show'},
        },
      ],
      {
        'show': ['bars'],
      },
    );
    final bars = tester.getRect(find.text('Bars'));
    final lines = tester.getRect(find.text('Lines'));
    expect(lines.top, bars.top, reason: 'on one line');
    expect(lines.left, greaterThan(bars.right));
  });

  group("Grafana's defaults", () {
    test('a value axis fits the data with 10% padding, kept at zero', () {
      // 45–75: 3 either side, rounded out to units.
      expect(fitRange([45, 60, 75]), (min: 42.0, max: 78.0));
      // Padding that would cross zero stops at it, either side.
      expect(fitRange([2, 100]), (min: 0.0, max: 110.0));
      expect(fitRange([-100, -2]), (min: -110.0, max: 0.0));
      // Data crossing zero is padded past it.
      expect(fitRange([-10, 10]), (min: -12.0, max: 12.0));
      // One value, or none, still gives a range.
      expect(fitRange([50]), (min: 45.0, max: 55.0));
      expect(fitRange([]), (min: 0.0, max: 1.0));
      // Set ends win; the other is fitted.
      expect(fitRange([45, 75], min: 0), (min: 0.0, max: 78.0));
      expect(fitRange([45, 75], max: 100), (min: 42.0, max: 100.0));
    });

    test('axis steps keep labels 40px apart, at 1, 2, 2.5 or 5', () {
      expect(axisStep(80, 200), 20);
      expect(axisStep(36, 200), 10);
      expect(axisStep(1, 200), 0.2);
      expect(axisStep(12, 200), 2.5);
      expect(axisStep(0.84, 200), 0.2);
    });

    test("numbers in Grafana's units", () {
      expect(autoDecimals(45.32), 1);
      expect(autoDecimals(3.14159), 2);
      expect(autoDecimals(1234.5), 0);
      expect(autoDecimals(12), 0);
      expect(chartValue(45.32, 'none'), '45.3');
      expect(chartValue(12000, 'none'), '12000');
      // On an axis with no Grafana unit: short, with the step's decimals,
      // so labels a step apart never read the same.
      expect(
        [
          for (var v = 1000000.0; v <= 1100000; v += 25000)
            chartValue(v, '', step: 25000),
        ],
        ['1.000M', '1.025M', '1.050M', '1.075M', '1.100M'],
      );
      expect(chartValue(1500, 'runs', step: 500), '1.5K');
      expect(chartValue(400, '', step: 100), '400');
      expect(chartValue(0.25, '', step: 0.05), '0.25');
      expect(chartValue(1234, 'short'), '1.23 K');
      expect(chartValue(3400000, 'short'), '3.40 Mil');
      expect(chartValue(950, 'short'), '950');
      expect(chartValue(45, 'percent'), '45%');
      expect(chartValue(0.452, 'percentunit'), '45.2%');
      expect(chartValue(1536, 'bytes'), '1.50 KiB');
      expect(chartValue(512, 'bytes'), '512 B');
      expect(chartValue(250, 'ms'), '250 ms');
      expect(chartValue(1500, 'ms'), '1.50 s');
      expect(chartValue(90, 's'), '1.50 min');
      expect(chartValue(0.25, 's'), '250 ms');
      // Other words follow the number, grouped, as before.
      expect(chartValue(12345, 'tokens'), '12,345 tokens');
      expect(chartValue(12345, ''), '12,345');
      // Set decimals.
      expect(chartValue(45.326, 'none', decimals: 2), '45.33');
      expect(chartValue(12345.6, 'tokens', decimals: 0), '12,346 tokens');
      // On an axis: the step's decimals; other words left off, short.
      expect(chartValue(60, 'none', step: 2.5), '60.0');
      expect(chartValue(1500, 'short', step: 500), '1.5 K');
      expect(chartValue(20000, 'tokens', step: 5000), '20K');
      expect(chartValue(-3, 'none'), '−3');
    });

    test('time labels as Grafana writes them', () {
      final at = DateTime(2026, 11, 22, 14, 5, 30);
      expect(
        axisTime(at, const Duration(hours: 6), const Duration(hours: 1)),
        '14:05',
      );
      expect(
        axisTime(at, const Duration(days: 7), const Duration(days: 1)),
        '11/22',
      );
      expect(
        axisTime(at, const Duration(days: 2), const Duration(hours: 12)),
        '11/22 14:05',
      );
      expect(
        axisTime(at, const Duration(minutes: 2), const Duration(seconds: 30)),
        '14:05:30',
      );
      expect(tooltipTime(at), '2026-11-22 14:05:30');
    });
  });

  Map<String, Object?> cpu({Map<String, Object?> more = const {}}) => {
    'id': 'cpu',
    'component': 'Chart',
    'kind': 'timeseries',
    'data': {'@path': '/cpu'},
    'x': 'time',
    'label': 'CPU',
    ...more,
  };
  final cpuData = {
    'cpu': _table(
      ['time', 'user', 'system'],
      [
        for (var h = 0; h <= 12; h++)
          ['2026-11-22 ${h.toString().padLeft(2, '0')}:00:00', 40 + h, 10 + h],
      ],
    ),
  };

  testWidgets("a time series in Grafana's look: the classic palette, 1px "
      'lines without fill, points where they fit, a fitted axis, a grid both '
      'ways and a legend', (tester) async {
    tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
    addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
    await _show(tester, [
      _root(['cpu']),
      cpu(),
    ], cpuData);
    final chart = tester.widget<LineChart>(find.byType(LineChart));
    final bars = chart.data.lineBarsData;
    expect(bars.first.color, const Color(0xFF73BF69));
    expect(bars.last.color, const Color(0xFFF2CC0C));
    expect(bars.first.barWidth, 1);
    expect(bars.first.belowBarData.show, isFalse);
    expect(bars.first.dotData.show, isTrue, reason: '13 points, far apart');
    // 10–52: 4.2 either side, rounded out to units, kept at zero.
    expect(chart.data.minY, 5);
    expect(chart.data.maxY, 57);
    expect(chart.data.gridData.drawVerticalLine, isTrue);
    expect(chart.data.borderData.show, isFalse);
    // The legend, with a pill for each series.
    expect(find.text('user'), findsOneWidget);
    expect(find.text('system'), findsOneWidget);
  });

  testWidgets('the light theme takes the palette\'s light shades, and a lone '
      'series still has its legend', (tester) async {
    tester.platformDispatcher.platformBrightnessTestValue = Brightness.light;
    addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
    await _show(tester, [
      _root(['cpu']),
      cpu(
        more: {
          'y': ['user'],
        },
      ),
    ], cpuData);
    final chart = tester.widget<LineChart>(find.byType(LineChart));
    expect(chart.data.lineBarsData.single.color, const Color(0xFF56A64B));
    expect(find.text('user'), findsOneWidget);
  });

  testWidgets('the value axis\'s bottom label sits on its line, with a '
      'gridline at the foot, as Grafana keeps 0 on the baseline; the top one '
      'stays inside (Zucchero row 794)', (tester) async {
    await _show(tester, [
      _root(['cpu']),
      cpu(more: {'min': 0, 'max': 100, 'legend': false}),
    ], cpuData);
    final line = find.byType(LineChart);
    final labels = _valueLabels(tester, line);
    expect(labels.first.$1, 0);
    expect(labels.last.$1, 100);
    final gap = labels[2].$2 - labels[1].$2;
    // On its line: as far from the next as each is from the one after.
    expect(labels[1].$2 - labels[0].$2, closeTo(gap, 0.5));
    // The plot's foot: the chart less the time axis's room (to a point:
    // every label's centre is as near its line, the text's own rounding).
    expect(labels.first.$2, closeTo(tester.getRect(line).bottom - 22, 1));
    // The top one is moved down inside, never over the panel's title.
    expect(
      labels.last.$2 - tester.getRect(line).top,
      greaterThan(tester.getSize(find.text('100')).height / 2),
    );
    final border = tester.widget<LineChart>(line).data.borderData;
    expect(border.show, isTrue);
    expect(border.border.bottom.width, greaterThan(0));
    expect(border.border.top.width, 0);
  });

  testWidgets("bars' bottom value label sits on its line too "
      '(Zucchero row 794)', (tester) async {
    await showBars(tester, ['2026-10-27', '2026-10-28', '2026-10-29']);
    final bars = _valueLabels(tester, find.byType(BarChart));
    expect(bars.first.$1, 0);
    expect(bars[1].$2 - bars[0].$2, closeTo(bars[2].$2 - bars[1].$2, 0.5));
  });

  testWidgets("a panel's own settings: line width, fill, points, axis ends, "
      'decimals and no legend; values out of range are left out', (
    tester,
  ) async {
    await _show(tester, [
      _root(['cpu', 'bad']),
      cpu(
        more: {
          'lineWidth': 3,
          'fill': 20,
          'points': 'never',
          'min': 0,
          'max': 100,
          'decimals': 2,
          'unit': 'percent',
          'legend': false,
        },
      ),
      {
        ...cpu(
          more: {
            'lineWidth': 40,
            'fill': 'lots',
            'points': 'some',
            'min': 'low',
            'decimals': 1.5,
            'legend': 'no',
          },
        ),
        'id': 'bad',
        'label': 'Bad',
      },
    ], cpuData);
    final charts = tester
        .widgetList<LineChart>(find.byType(LineChart))
        .toList();
    final set = charts.first.data;
    expect(set.lineBarsData.first.barWidth, 3);
    expect(set.lineBarsData.first.belowBarData.show, isTrue);
    expect(set.lineBarsData.first.belowBarData.color!.a, closeTo(0.2, 0.01));
    expect(set.lineBarsData.first.dotData.show, isFalse);
    expect(set.minY, 0);
    expect(set.maxY, 100);
    expect(find.text('100.00%'), findsOneWidget, reason: 'the top label');
    // The bad ones fall back to the defaults.
    final bad = charts.last.data;
    expect(bad.lineBarsData.first.barWidth, 1);
    expect(bad.lineBarsData.first.belowBarData.show, isFalse);
    expect(bad.lineBarsData.first.dotData.show, isTrue);
    expect(bad.minY, 5);
    // One legend: the bad one's ("no" isn't false).
    expect(find.text('user'), findsOneWidget);
  });

  testWidgets('points show only where they are far enough apart', (
    tester,
  ) async {
    await _show(
      tester,
      [
        _root(['cpu']),
        cpu(),
      ],
      {
        'cpu': _table(
          ['time', 'user'],
          [
            for (var m = 0; m < 600; m++)
              [
                DateTime(
                  2026,
                  11,
                  22,
                  10,
                ).add(Duration(minutes: m)).millisecondsSinceEpoch,
                m % 7,
              ],
          ],
        ),
      },
      size: const Size(390, 844),
    );
    final chart = tester.widget<LineChart>(find.byType(LineChart));
    expect(chart.data.lineBarsData.single.dotData.show, isFalse);
  });

  testWidgets('a tooltip shows the time and the nearest series; a mouse\'s '
      'goes as it leaves', (tester) async {
    await _show(tester, [
      _root(['cpu']),
      cpu(),
    ], cpuData);
    final rect = tester.getRect(find.byType(LineChart));
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    addTearDown(mouse.removePointer);
    // Near the right end, high up: user's last point (52 at 12:00).
    await mouse.moveTo(Offset(rect.right - 4, rect.top + 20));
    await tester.pump();
    expect(find.text('2026-11-22 12:00:00'), findsOneWidget);
    expect(find.text('user'), findsNWidgets(2), reason: 'legend, tooltip');
    expect(find.text('52'), findsOneWidget);
    // Low down: system's (22).
    await mouse.moveTo(Offset(rect.right - 4, rect.bottom - 30));
    await tester.pump();
    expect(find.text('22'), findsOneWidget);
    expect(find.text('system'), findsNWidgets(2));
    await mouse.moveTo(Offset.zero);
    await tester.pump();
    expect(find.text('2026-11-22 12:00:00'), findsNothing);
  });

  testWidgets("a tooltip takes the time under the pointer first: a series' "
      "lone point far off in time doesn't win on its value", (tester) async {
    await _show(
      tester,
      [
        _root(['cpu']),
        cpu(
          more: {
            'y': ['steady', 'lone'],
          },
        ),
      ],
      {
        'cpu': _table(
          ['time', 'steady', 'lone'],
          [
            for (var h = 0; h <= 12; h++)
              [
                '2026-11-22 ${h.toString().padLeft(2, '0')}:00:00',
                1 + (h % 2) * 0.2,
                h == 12 ? 2 : null,
              ],
          ],
        ),
      },
    );
    final rect = tester.getRect(find.byType(LineChart));
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    addTearDown(mouse.removePointer);
    // Mid-chart, at the height of lone's 2.
    await mouse.moveTo(Offset(rect.center.dx, rect.top + 20));
    await tester.pump();
    expect(find.textContaining('2026-11-22 0'), findsOneWidget);
    expect(find.text('2026-11-22 12:00:00'), findsNothing);
    expect(find.text('steady'), findsNWidgets(2), reason: 'legend, tooltip');
    // At its own time, lone's point shows.
    await mouse.moveTo(Offset(rect.right - 4, rect.top + 20));
    await tester.pump();
    expect(find.text('2026-11-22 12:00:00'), findsOneWidget);
    expect(find.text('lone'), findsNWidgets(2));
  });

  testWidgets("a finger's tooltip stays once lifted, and a tap on the same "
      'point closes it', (tester) async {
    await _show(tester, [
      _root(['cpu']),
      cpu(),
    ], cpuData);
    final rect = tester.getRect(find.byType(LineChart));
    final at = Offset(rect.right - 4, rect.top + 20);
    await tester.tapAt(at);
    await tester.pump();
    expect(find.text('2026-11-22 12:00:00'), findsOneWidget);
    await tester.pump(const Duration(seconds: 1));
    expect(find.text('2026-11-22 12:00:00'), findsOneWidget);
    await tester.tapAt(at);
    await tester.pump();
    expect(find.text('2026-11-22 12:00:00'), findsNothing);
  });

  testWidgets("the x axis spans the dashboard's range (timeFrom, timeTo): "
      'rows outside it are left out, the value axis fits what is left; '
      'bad ends are ignored', (tester) async {
    double at(int hour) =>
        DateTime(2026, 11, 22, hour).millisecondsSinceEpoch.toDouble();
    final ends = <Map<String, Object?>>[
      {'timeFrom': at(6), 'timeTo': at(18)},
      // One end: the other from the data.
      {'timeFrom': at(-12)},
      // Out of order, negative or not whole: as if unset.
      {'timeFrom': at(18), 'timeTo': at(6)},
      {'timeFrom': -1},
      {'timeTo': at(18) + 0.5},
      {'timeFrom': 'yesterday'},
      // Nothing inside the range: nothing to show.
      {'timeFrom': at(13), 'timeTo': at(20)},
    ];
    await _show(
      tester,
      [
        _root([for (var i = 0; i < ends.length; i++) 'c$i']),
        for (final (i, more) in ends.indexed) {...cpu(more: more), 'id': 'c$i'},
      ],
      cpuData,
      size: const Size(1200, 3000),
    );
    LineChartData data(int i) =>
        tester.widgetList<LineChart>(find.byType(LineChart)).elementAt(i).data;
    expect(data(0).minX, at(6));
    expect(data(0).maxX, at(18), reason: 'past the last row, at 12:00');
    expect(data(0).lineBarsData.first.spots.first.x, at(6));
    expect(data(0).lineBarsData.first.spots, hasLength(7));
    // 16–52 left: 3.6 either side, rounded out to units.
    expect(data(0).minY, 12);
    expect(data(0).maxY, 56);
    expect(data(1).minX, at(-12));
    expect(data(1).maxX, at(12));
    for (var i = 2; i < 6; i++) {
      expect(data(i).minX, at(0), reason: '${ends[i]}');
      expect(data(i).maxX, at(12), reason: '${ends[i]}');
    }
    expect(find.byType(LineChart), findsNWidgets(6));
    expect(find.text('Nothing to show.'), findsOneWidget);
  });

  testWidgets('a finger scrolling the dashboard over a chart opens no '
      'tooltip (P1); a tap still does', (tester) async {
    await _show(
      tester,
      [
        // More than the page holds, so it scrolls.
        _root(['cpu', 'cpu2', 'cpu3', 'cpu4']),
        cpu(),
        {...cpu(), 'id': 'cpu2'},
        {...cpu(), 'id': 'cpu3'},
        {...cpu(), 'id': 'cpu4'},
      ],
      cpuData,
      size: const Size(390, 844),
    );
    final chart = find.byType(LineChart).first;
    final rect = tester.getRect(chart);
    final at = Offset(rect.right - 4, rect.top + 20);
    final before = tester.getTopLeft(chart);
    final gesture = await tester.startGesture(at);
    for (var i = 0; i < 10; i++) {
      await gesture.moveBy(const Offset(0, -12));
      await tester.pump(const Duration(milliseconds: 16));
    }
    await gesture.up();
    await tester.pumpAndSettle();
    expect(tester.getTopLeft(chart).dy, lessThan(before.dy));
    expect(find.textContaining('2026-11-22'), findsNothing);
    final moved = tester.getRect(chart);
    // On the part of it still on screen.
    await tester.tapAt(Offset(moved.right - 4, math.max(moved.top, 0) + 20));
    await tester.pump();
    expect(find.text('2026-11-22 12:00:00'), findsOneWidget);
  });

  testWidgets("the page scrolling under a still mouse takes its tooltip "
      "away; a tap's pinned one stays with its chart", (tester) async {
    await _show(
      tester,
      [
        _root(['cpu', 'cpu2', 'cpu3', 'cpu4']),
        cpu(),
        {...cpu(), 'id': 'cpu2'},
        {...cpu(), 'id': 'cpu3'},
        {...cpu(), 'id': 'cpu4'},
      ],
      cpuData,
      size: const Size(800, 700),
    );
    final rect = tester.getRect(find.byType(LineChart).first);
    final at = Offset(rect.right - 4, rect.top + 20);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: at);
    addTearDown(mouse.removePointer);
    await mouse.moveTo(at + const Offset(0, 1));
    await tester.pump();
    expect(find.text('2026-11-22 12:00:00'), findsOneWidget);
    tester.binding.handlePointerEvent(
      PointerScrollEvent(
        position: at + const Offset(0, 1),
        scrollDelta: const Offset(0, 120),
      ),
    );
    await tester.pumpAndSettle();
    expect(
      tester.getRect(find.byType(LineChart).first).top,
      lessThan(rect.top),
    );
    expect(find.text('2026-11-22 12:00:00'), findsNothing);
    // Pinned by a tap, it stays through a scroll.
    await mouse.moveTo(Offset.zero);
    await tester.pump();
    final moved = tester.getRect(find.byType(LineChart).at(1));
    await tester.tapAt(Offset(moved.right - 4, moved.top + 20));
    await tester.pump();
    expect(find.text('2026-11-22 12:00:00'), findsOneWidget);
    tester.binding.handlePointerEvent(
      PointerScrollEvent(
        position: moved.center,
        scrollDelta: const Offset(0, 40),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('2026-11-22 12:00:00'), findsOneWidget);
  });

  testWidgets('a tap on a legend entry shows only its series, fitting the '
      'axis to it; another shows them all', (tester) async {
    final semantics = tester.ensureSemantics();
    await _show(tester, [
      _root(['cpu']),
      cpu(),
    ], cpuData);
    LineChartData data() =>
        tester.widget<LineChart>(find.byType(LineChart)).data;
    expect(data().lineBarsData.every((bar) => bar.show), isTrue);
    expect(
      tester.getSemantics(find.text('system')),
      matchesSemantics(
        label: 'system',
        isButton: true,
        hasTapAction: true,
        hasSelectedState: true,
      ),
    );
    await tester.tap(find.text('system'));
    await tester.pump();
    expect([for (final bar in data().lineBarsData) bar.show], [false, true]);
    // 10–22: 1.2 either side.
    expect(data().minY, 8);
    expect(data().maxY, 24);
    await tester.tap(find.text('system'));
    await tester.pump();
    expect(data().lineBarsData.every((bar) => bar.show), isTrue);
    semantics.dispose();
  });

  testWidgets('at the largest text on the smallest phone, a time series '
      'keeps its time labels apart and its legend wraps', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await _show(
      tester,
      [
        _root(['cpu']),
        cpu(more: {'unit': 'short'}),
      ],
      {
        'cpu': _table(
          ['time', 'user time of the busiest agent', 'system'],
          [
            for (var h = 0; h <= 48; h++)
              [
                DateTime(
                  2026,
                  11,
                  21,
                ).add(Duration(hours: h)).millisecondsSinceEpoch,
                40000 + h * 100,
                10 + h,
              ],
          ],
        ),
      },
      size: const Size(320, 700),
    );
    expect(tester.takeException(), isNull);
    final labels = [
      for (final element in find.byType(Text).evaluate())
        if ((element.widget as Text).softWrap == false)
          tester.getRect(find.byWidget(element.widget)),
    ];
    for (var a = 0; a < labels.length; a++) {
      for (var b = a + 1; b < labels.length; b++) {
        expect(labels[a].overlaps(labels[b]), isFalse);
      }
    }
    expect(find.text('user time of the busiest agent'), findsOneWidget);
  });

  group("a Stat in Grafana's look", () {
    Map<String, Object?> stat(String id, Map<String, Object?> more) => {
      'id': id,
      'component': 'Stat',
      'label': id,
      'value': {'@path': '/$id'},
      'spark': [1, 3, 2, 5],
      ...more,
    };
    Color? valueColor(WidgetTester tester, String label) {
      final rich = tester.widget<RichText>(
        find.descendant(
          of: find.ancestor(
            of: find.text(label),
            matching: find.byType(AppStat),
          ),
          matching: find.byWidgetPredicate(
            (w) => w is RichText && w.maxLines == 1 && w.softWrap == false,
          ),
        ),
      );
      Color? color;
      rich.text.visitChildren((span) {
        if (span is TextSpan && span.text != null) {
          color = span.style?.color;
          return false;
        }
        return true;
      });
      return color;
    }

    testWidgets('its value takes the color of its threshold: green, red '
        'from 80 when it sets none', (tester) async {
      tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      await _show(
        tester,
        [
          _root(['low', 'high', 'set', 'fixed', 'agent']),
          stat('low', {}),
          stat('high', {}),
          stat('set', {
            'thresholds': [
              {'color': 'blue'},
              {'color': 'yellow', 'value': 10},
              {'color': 'purple', 'value': 20},
            ],
          }),
          stat('fixed', {'color': 'orange'}),
          stat('agent', {'color': 'agent:a1'}),
        ],
        {'low': 79.5, 'high': 80, 'set': 15, 'fixed': 500, 'agent': 3},
      );
      expect(valueColor(tester, 'low'), const Color(0xFF73BF69));
      expect(valueColor(tester, 'high'), const Color(0xFFF2495C));
      expect(valueColor(tester, 'set'), const Color(0xFFFADE2A));
      expect(valueColor(tester, 'fixed'), const Color(0xFFFF9830));
      expect(valueColor(tester, 'agent'), agentColor('pink'));
      // Its spark line, in the same color, 1px over a 20% fill.
      final spark = tester
          .widgetList<LineChart>(
            find.descendant(
              of: find.ancestor(
                of: find.text('low'),
                matching: find.byType(AppStat),
              ),
              matching: find.byType(LineChart),
            ),
          )
          .single
          .data
          .lineBarsData
          .single;
      expect(spark.color, const Color(0xFF73BF69));
      expect(spark.barWidth, 1);
      expect(spark.belowBarData.color!.a, closeTo(0.2, 0.01));
    });

    testWidgets('no value says No data; Grafana units, decimals, and no '
        'spark line with graph none', (tester) async {
      final semantics = tester.ensureSemantics();
      await _show(
        tester,
        [
          _root(['empty', 'bytes', 'plain']),
          stat('empty', {}),
          stat('bytes', {'unit': 'bytes', 'graph': 'none'}),
          stat('plain', {'unit': 'runs', 'decimals': 1}),
        ],
        {'empty': null, 'bytes': 1536, 'plain': 12345},
      );
      expect(find.text('No data'), findsOneWidget);
      expect(
        tester.getSemantics(
          find.ancestor(of: find.text('empty'), matching: find.byType(AppStat)),
        ),
        matchesSemantics(label: 'empty', value: 'No data'),
      );
      expect(find.text('1.50 KiB', findRichText: true), findsOneWidget);
      expect(
        find.descendant(
          of: find.ancestor(
            of: find.text('bytes'),
            matching: find.byType(AppStat),
          ),
          matching: find.byType(LineChart),
        ),
        findsNothing,
      );
      expect(find.text('12,345.0 runs', findRichText: true), findsOneWidget);
      semantics.dispose();
    });

    testWidgets("Grafana's automatic decimals (P3), and an all-equal spark "
        'line flat along the bottom edge (P2)', (tester) async {
      await _show(
        tester,
        [
          _root(['b', 'whole', 'runs', 'big', 'flat']),
          stat('b', {}),
          stat('whole', {}),
          stat('runs', {'unit': 'runs'}),
          stat('big', {}),
          stat('flat', {
            'spark': [0, 0, 0, 0],
          }),
        ],
        {
          'b': 32.21,
          'whole': 12345,
          'runs': 0.04567,
          'big': 2500000,
          'flat': 0,
        },
      );
      expect(find.text('32.2', findRichText: true), findsOneWidget);
      expect(find.text('12,345', findRichText: true), findsOneWidget);
      expect(find.text('0.0457 runs', findRichText: true), findsOneWidget);
      expect(find.text('2.5M', findRichText: true), findsOneWidget);
      final data = tester
          .widget<LineChart>(
            find.descendant(
              of: find.ancestor(
                of: find.text('flat'),
                matching: find.byType(AppStat),
              ),
              matching: find.byType(LineChart),
            ),
          )
          .data;
      expect(data.minY, 0, reason: 'the line on the bottom edge');
      expect(data.maxY, greaterThan(0));
    });

    testWidgets('its value grows to fill the panel, and shrinks to fit its '
        'width', (tester) async {
      await _show(
        tester,
        [
          _root(['small', 'tall', 'long']),
          stat('small', {'height': 80}),
          stat('tall', {'height': 300}),
          stat('long', {'height': 300}),
        ],
        {'small': 7, 'tall': 7, 'long': 123456789012},
        size: const Size(390, 1000),
      );
      double sizeOf(String label) => tester
          .getSize(
            find.descendant(
              of: find.ancestor(
                of: find.text(label),
                matching: find.byType(AppStat),
              ),
              matching: find.byWidgetPredicate(
                (w) => w is RichText && w.softWrap == false,
              ),
            ),
          )
          .height;
      expect(sizeOf('tall'), greaterThan(sizeOf('small') * 2));
      expect(tester.getSize(find.byType(AppStat).at(1)).height, 300);
      final long = tester.getRect(
        find.descendant(
          of: find.ancestor(
            of: find.text('long'),
            matching: find.byType(AppStat),
          ),
          matching: find.byWidgetPredicate(
            (w) => w is RichText && w.softWrap == false,
          ),
        ),
      );
      expect(long.width, lessThanOrEqualTo(390));
      expect(tester.takeException(), isNull);
    });

    testWidgets('at the largest text a panel of a set height grows by its '
        'title, so the value keeps its size (Zucchero, row 325)', (
      tester,
    ) async {
      Future<(double, double)> measure(double scale) async {
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        await _show(
          tester,
          [
            _root(['card']),
            {'id': 'card', 'component': 'Card', 'child': 'Rate'},
            stat('Rate', {'height': 140}),
          ],
          {'Rate': 43.8},
          size: const Size(375, 1400),
        );
        final value = find.descendant(
          of: find.byType(AppStat),
          matching: find.byWidgetPredicate(
            (w) => w is RichText && w.softWrap == false,
          ),
        );
        double? fontSize;
        tester.widget<RichText>(value).text.visitChildren((span) {
          if (span is TextSpan && span.text == '43.8') {
            fontSize = span.style!.fontSize;
          }
          return fontSize == null;
        });
        return (fontSize!, tester.getSize(find.byType(AppStat)).height);
      }

      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final (size, height) = await measure(1);
      expect(height, 140);
      final (largest, taller) = await measure(3.1);
      expect(largest, moreOrLessEquals(size, epsilon: 0.01));
      expect(taller, greaterThan(140));
      expect(tester.takeException(), isNull);
    });

    testWidgets('background and solid color modes fill the panel; words on '
        'them read in white or black', (tester) async {
      tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      await _show(
        tester,
        [
          _root(['bg', 'solid', 'pale']),
          stat('bg', {'colorMode': 'background'}),
          stat('solid', {'colorMode': 'solid', 'color': 'red'}),
          stat('pale', {'colorMode': 'solid', 'color': 'yellow'}),
        ],
        {'bg': 1, 'solid': 1, 'pale': 1},
      );
      BoxDecoration fill(String label) => tester
          .widgetList<DecoratedBox>(
            find.descendant(
              of: find.ancestor(
                of: find.text(label),
                matching: find.byType(AppStat),
              ),
              matching: find.byType(DecoratedBox),
            ),
          )
          .map((box) => box.decoration as BoxDecoration)
          .first;
      expect(fill('bg').gradient, isNotNull);
      expect(fill('solid').color, const Color(0xFFF2495C));
      expect(valueColor(tester, 'solid'), const Color(0xFFFFFFFF));
      expect(valueColor(tester, 'pale'), const Color(0xFF111217));
    });

    testWidgets("in a Card it reaches the card's edges, under the card's "
        'hairline; a plain Card keeps its padding', (tester) async {
      await _show(
        tester,
        [
          _root(['statCard', 'textCard']),
          {'id': 'statCard', 'component': 'Card', 'child': 'runs'},
          stat('runs', {'height': 100}),
          {'id': 'textCard', 'component': 'Card', 'child': 'words'},
          {'id': 'words', 'component': 'Text', 'text': 'Words'},
        ],
        {'runs': 5},
      );
      final card = tester.getRect(find.byType(AppStatCard));
      final drawn = tester.getRect(find.byType(AppStat));
      expect(drawn, card, reason: 'edge to edge');
      // The label keeps a plain card's inset (its padding inside the
      // hairline).
      expect(tester.getTopLeft(find.text('runs')).dx - card.left, Space.m + 1);
      expect(tester.getTopLeft(find.text('runs')).dy - card.top, Space.m + 1);
      // The hairline is drawn over the Stat.
      final border = tester
          .widgetList<DecoratedBox>(
            find.descendant(
              of: find.byType(AppStatCard),
              matching: find.byType(DecoratedBox),
            ),
          )
          .firstWhere(
            (box) => (box.decoration as BoxDecoration).border != null,
          );
      expect(border.position, DecorationPosition.foreground);
      // A Card holding anything else is as it was.
      final words = tester.getRect(find.text('Words'));
      final plain = tester.getRect(
        find
            .ancestor(of: find.text('Words'), matching: find.byType(Container))
            .first,
      );
      expect(words.left - plain.left, Space.m + 1);
      expect(words.top - plain.top, Space.m + 1);
    });

    testWidgets('at the largest text on a phone, stat cards stack and their '
        'values fit', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await _show(
        tester,
        [
          _root(['row']),
          {
            'id': 'row',
            'component': 'Row',
            'children': ['c1', 'c2'],
          },
          {'id': 'c1', 'component': 'Card', 'child': 'tokens', 'weight': 6},
          stat('tokens', {'unit': 'short', 'height': 100}),
          {'id': 'c2', 'component': 'Card', 'child': 'messages', 'weight': 6},
          stat('messages', {'height': 100}),
        ],
        {'tokens': 3400000, 'messages': 1284},
        size: const Size(320, 700),
      );
      expect(tester.takeException(), isNull);
      final cards = find.byType(AppStatCard);
      expect(
        tester.getTopLeft(cards.at(1)).dy,
        greaterThan(tester.getBottomLeft(cards.at(0)).dy),
        reason: 'stacked',
      );
      for (final label in ['tokens', 'messages']) {
        final box = tester.getRect(
          find.ancestor(of: find.text(label), matching: find.byType(AppStat)),
        );
        final value = tester.getRect(
          find.descendant(
            of: find.ancestor(
              of: find.text(label),
              matching: find.byType(AppStat),
            ),
            matching: find.byWidgetPredicate(
              (w) => w is RichText && w.softWrap == false,
            ),
          ),
        );
        expect(
          box.contains(value.topLeft) &&
              box.contains(value.bottomRight - const Offset(1, 1)),
          isTrue,
        );
      }
    });
  });

  testWidgets("a scatter in Grafana's look: 5px circles at half their "
      'color with a 1px ring, both axes fitted and named, a legend', (
    tester,
  ) async {
    tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
    addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
    await _show(
      tester,
      [
        _root(['runs']),
        {
          'id': 'runs',
          'component': 'Chart',
          'kind': 'scatter',
          'data': {'@path': '/runs'},
          'x': 'seconds',
          'seriesBy': 'who',
          'y': ['tokens'],
          'unit': 'tokens',
        },
      ],
      {
        'runs': _table(
          ['seconds', 'who', 'tokens'],
          [
            [100, 'June', 5000],
            [200, 'June', 6000],
            [150, 'Kai', 2000],
          ],
        ),
      },
    );
    ScatterChartData data() =>
        tester.widget<ScatterChart>(find.byType(ScatterChart)).data;
    final dot = data().scatterSpots.first.dotPainter as FlDotCirclePainter;
    // 5px across, its 1px ring inside.
    expect(dot.radius + dot.strokeWidth, 2.5);
    expect(dot.color, const Color(0xFF73BF69).withValues(alpha: 0.5));
    expect(dot.strokeWidth, 1);
    expect(dot.strokeColor, const Color(0xFF73BF69));
    // 100–200 and 2000–6000, 10% either side.
    expect((data().minX, data().maxX), (90.0, 210.0));
    expect((data().minY, data().maxY), (1600.0, 6400.0));
    expect(find.text('June'), findsOneWidget);
    expect(find.text('Kai'), findsOneWidget);
    // Each axis named for its column.
    expect(find.text('seconds'), findsOneWidget);
    expect(find.text('tokens'), findsOneWidget);
    // Kai alone, its axis refitted; then all again.
    await tester.tap(find.text('Kai'));
    await tester.pump();
    expect(data().scatterSpots, hasLength(1), reason: 'only Kai');
    expect((data().minY, data().maxY), (1800.0, 2200.0));
    await tester.tap(find.text('Kai'));
    await tester.pump();
    expect(data().scatterSpots, hasLength(3));
  });

  testWidgets("a scatter's tooltip: a finger on a point shows it, a tap on "
      'nothing closes it', (tester) async {
    await _show(
      tester,
      [
        _root(['runs']),
        {
          'id': 'runs',
          'component': 'Chart',
          'kind': 'scatter',
          'data': {'@path': '/runs'},
          'x': 'seconds',
          'unit': 'tokens',
        },
      ],
      {
        'runs': _table(
          ['seconds', 'tokens'],
          [
            [0, 0],
            [100, 1000],
          ],
        ),
      },
    );
    final chart = tester.widget<ScatterChart>(find.byType(ScatterChart));
    final rect = tester.getRect(find.byType(ScatterChart));
    // Where (100, 1000) is drawn: the plot's right of the value axis.
    final data = chart.data;
    final left = data.titlesData.leftTitles;
    final under = data.titlesData.bottomTitles;
    final axis = left.sideTitles.reservedSize + left.axisNameSize;
    final bottom = under.sideTitles.reservedSize + under.axisNameSize;
    Offset at(double x, double y) => Offset(
      rect.left +
          axis +
          (x - data.minX) / (data.maxX - data.minX) * (rect.width - axis),
      rect.top +
          (data.maxY - y) / (data.maxY - data.minY) * (rect.height - bottom),
    );
    await tester.tapAt(at(100, 1000));
    await tester.pump();
    expect(find.text('seconds 100'), findsOneWidget);
    expect(find.text('1,000 tokens'), findsOneWidget);
    await tester.tapAt(at(50, 500) + const Offset(0, -30));
    await tester.pump();
    expect(find.text('seconds 100'), findsNothing);
  });

  testWidgets("a menu's rows (C4): full width, the chosen one filled with an "
      'accent bar on its left, its words in line with the others', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final semantics = tester.ensureSemantics();
    Map<String, Object?> option(int i, String words, {bool chosen = false}) => {
      'id': 'menu_o$i',
      'component': 'Button',
      'child': 'menu_o${i}_c',
      'variant': chosen ? 'primary' : 'borderless',
      'action': {
        'event': {
          'name': 'range',
          'context': {'pick': i},
        },
      },
    };
    final transport = await _show(
      tester,
      [
        _root(['menu']),
        {'id': 'menu', 'component': 'Card', 'child': 'menu_col'},
        {
          'id': 'menu_col',
          'component': 'Column',
          'children': ['menu_o0', 'menu_o1'],
        },
        option(0, 'Last 24 hours'),
        option(1, 'Last 7 days', chosen: true),
        for (final (i, words) in ['Last 24 hours', 'Last 7 days'].indexed) ...[
          {
            'id': 'menu_o${i}_c',
            'component': 'List',
            'direction': 'horizontal',
            'children': ['menu_o${i}_t'],
          },
          {'id': 'menu_o${i}_t', 'component': 'Text', 'text': words},
        ],
      ],
      null,
      size: const Size(390, 844),
    );
    BoxDecoration row(String words) =>
        tester
                .widget<Container>(
                  find
                      .ancestor(
                        of: find.text(words),
                        matching: find.byType(Container),
                      )
                      .first,
                )
                .decoration!
            as BoxDecoration;
    Rect rect(String words) => tester.getRect(
      find
          .ancestor(of: find.text(words), matching: find.byType(Container))
          .first,
    );
    final palette = Palette.of(tester.element(find.text('Last 7 days')));
    final chosen = row('Last 7 days');
    expect(chosen.color, palette.selected);
    expect((chosen.border! as Border).left.width, 3);
    expect((chosen.border! as Border).left.color, palette.signal);
    expect(row('Last 24 hours').border, isNull);
    expect(row('Last 24 hours').color, isNull);
    expect(rect('Last 24 hours').width, rect('Last 7 days').width);
    expect(
      rect('Last 24 hours').width,
      greaterThan(300),
      reason: 'the menu\'s width',
    );
    expect(
      tester.getTopLeft(find.text('Last 7 days')).dx,
      tester.getTopLeft(find.text('Last 24 hours')).dx,
    );
    expect(
      tester.getSemantics(find.text('Last 7 days')),
      matchesSemantics(
        label: 'Last 7 days',
        isButton: true,
        hasTapAction: true,
        hasSelectedState: true,
        isSelected: true,
        hasEnabledState: true,
        isEnabled: true,
        isFocusable: true,
      ),
    );
    // A finger's height on a phone.
    expect(rect('Last 24 hours').height, greaterThanOrEqualTo(44));
    await tester.tap(find.text('Last 24 hours'));
    await tester.pump();
    final (_, message) = transport.actions.single;
    expect((message['action']! as Map)['context'], {'pick': 0});
    // Focused, Return presses it.
    Focus.of(tester.element(find.text('Last 7 days'))).requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(transport.actions, hasLength(2));
    expect((transport.actions.last.$2['action']! as Map)['context'], {
      'pick': 1,
    });
    semantics.dispose();
  });

  testWidgets("a top bar's borderless menu buttons: words in line with the "
      "page's, a target tall, the row's spacing between them; other "
      'buttons keep their padding', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    List<Map<String, Object?>> menuButton(
      String id,
      String words, {
      String variant = 'borderless',
    }) => [
      {
        'id': id,
        'component': 'Button',
        'child': '${id}_c',
        'variant': variant,
        'action': {
          'event': {'name': 'menu'},
        },
      },
      {
        'id': '${id}_c',
        'component': 'List',
        'direction': 'horizontal',
        'align': 'center',
        'children': ['${id}_t', '${id}_i'],
      },
      {'id': '${id}_t', 'component': 'Text', 'text': words},
      {'id': '${id}_i', 'component': 'Icon', 'name': 'chevron-down'},
    ];
    await _show(
      tester,
      [
        _root(['bar', 'note', 'other']),
        {
          'id': 'bar',
          'component': 'List',
          'direction': 'horizontal',
          'align': 'center',
          'children': ['board', 'range'],
        },
        ...menuButton('board', 'Board'),
        ...menuButton('range', 'Last 6h'),
        {'id': 'note', 'component': 'Text', 'text': 'A line of the page'},
        ...menuButton('other', 'Plain', variant: 'default'),
      ],
      null,
      size: const Size(390, 844),
    );
    final page = tester.getTopLeft(find.text('A line of the page')).dx;
    expect(tester.getTopLeft(find.text('Board')).dx, page);
    Rect button(String words) => tester.getRect(
      find
          .ancestor(of: find.text(words), matching: find.byType(Pressable))
          .first,
    );
    expect(button('Board').height, greaterThanOrEqualTo(44));
    expect(
      button('Last 6h').left - button('Board').right,
      Space.s,
      reason: "the row's own spacing",
    );
    expect(
      tester.getTopLeft(find.text('Plain')).dx,
      page + Space.xs,
      reason: 'another variant keeps its padding',
    );
  });

  group("a bar: a Row of Buttons, one weighted (M1)", () {
    List<Map<String, Object?>> button(
      String id,
      List<Map<String, Object?>> face, {
      num? weight,
    }) => [
      {
        'id': id,
        'component': 'Button',
        'child': '${id}_c',
        'variant': 'borderless',
        'weight': ?weight,
        'action': {
          'event': {
            'name': 'menu',
            'context': {'which': id},
          },
        },
      },
      {
        'id': '${id}_c',
        'component': 'List',
        'direction': 'horizontal',
        'align': 'center',
        'children': [for (final part in face) part['id']],
      },
      ...face,
    ];
    Map<String, Object?> words(String id, String text, {String? variant}) => {
      'id': id,
      'component': 'Text',
      'text': text,
      'variant': ?variant,
    };
    Map<String, Object?> icon(String id, String name) => {
      'id': id,
      'component': 'Icon',
      'name': name,
    };
    List<Map<String, Object?>> bar(String name) => [
      _root(['bar', 'note']),
      {
        'id': 'bar',
        'component': 'Row',
        'children': ['name', 'range', 'filters', 'more'],
      },
      ...button('name', weight: 1, [
        words('name_t', name, variant: 'heading'),
        icon('name_i', 'chevron-down'),
      ]),
      ...button('range', [
        icon('range_l', 'clock'),
        words('range_t', '30 days'),
        icon('range_i', 'chevron-down'),
      ]),
      ...button('filters', [
        words('filters_t', 'Filters (2)'),
        icon('filters_i', 'chevron-down'),
      ]),
      ...button('more', [icon('more_l', 'ellipsis')]),
      {'id': 'note', 'component': 'Text', 'text': 'A line of the page'},
    ];

    // The test's font is wide: 620 is about an SE's 320 in the app's (the
    // SE itself, in Geist, is in app_charts_golden_test).
    for (final (width, scale) in [(700.0, 1.0), (620.0, 1.0), (1100.0, 2.0)]) {
      testWidgets('one line at $width wide, text ×$scale: the name gives way, '
          '••• stays at the end', (tester) async {
        debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        final transport = await _show(
          tester,
          bar('A dashboard with a long name'),
          null,
          size: Size(width, 844),
        );
        expect(tester.takeException(), isNull);
        final page = tester.getRect(find.text('A line of the page'));
        final inset = page.left;
        Rect of(String id) => tester.getRect(
          find
              .ancestor(of: find.text(id), matching: find.byType(Pressable))
              .first,
        );
        final name = of('A dashboard with a long name');
        final range = of('30 days');
        final filters = of('Filters (2)');
        final more = tester.getRect(
          find
              .ancestor(
                of: find.byWidgetPredicate(
                  (w) => w is ExtensionGlyph && w.icon == 'ellipsis',
                ),
                matching: find.byType(Pressable),
              )
              .first,
        );
        // One line: every button's middle on the same height.
        for (final rect in [range, filters, more]) {
          expect(rect.center.dy, moreOrLessEquals(name.center.dy, epsilon: 1));
        }
        expect(name.left, moreOrLessEquals(inset, epsilon: 0.5));
        expect(more.right, moreOrLessEquals(width - inset, epsilon: 0.5));
        expect(name.right, lessThanOrEqualTo(range.left));
        expect(range.right, lessThanOrEqualTo(filters.left));
        expect(filters.right, lessThanOrEqualTo(more.left));
        // The name's words end in "…" on one line.
        final paragraph = tester.renderObject<RenderParagraph>(
          find.descendant(
            of: find.text('A dashboard with a long name'),
            matching: find.byType(RichText),
          ),
        );
        expect(paragraph.maxLines, 1);
        expect(paragraph.didExceedMaxLines, isTrue);
        // Its chevron stays.
        final chevrons = find.byWidgetPredicate(
          (w) => w is ExtensionGlyph && w.icon == 'chevron-down',
        );
        expect(chevrons, findsNWidgets(3));
        for (final chevron in chevrons.evaluate()) {
          final rect = tester.getRect(find.byWidget(chevron.widget));
          expect(rect.right, lessThanOrEqualTo(width - inset));
        }
        // Each still a button.
        await tester.tap(find.text('30 days'));
        await tester.pump();
        expect((transport.actions.single.$2['action']! as Map)['context'], {
          'which': 'range',
        });
      });
    }

    testWidgets('only the name gives way while the others fit: their words '
        'stay whole (Scolippi 27597)', (tester) async {
      // The test's font is wide: 520 is about an SE's 320 in the app's.
      await _show(
        tester,
        bar('A dashboard with a long name'),
        null,
        size: const Size(520, 844),
      );
      expect(tester.takeException(), isNull);
      bool cut(String words) => tester
          .renderObject<RenderParagraph>(
            find.descendant(
              of: find.text(words),
              matching: find.byType(RichText),
            ),
          )
          .didExceedMaxLines;
      expect(cut('A dashboard with a long name'), isTrue);
      expect(cut('30 days'), isFalse);
      expect(cut('Filters (2)'), isFalse);
    });

    for (final scale in [2.0, 3.1]) {
      testWidgets('at text ×$scale on an SE the name keeps its first letters: '
          'it gives way to the floor the others have, then all evenly '
          '(Zucchero row 381)', (tester) async {
        debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        await _show(
          tester,
          bar('Zucchero dashboard'),
          null,
          size: const Size(320, 844),
        );
        expect(tester.takeException(), isNull);
        Rect of(String id) => tester.getRect(
          find
              .ancestor(of: find.text(id), matching: find.byType(Pressable))
              .first,
        );
        final name = of('Zucchero dashboard').width;
        final range = of('30 days').width;
        final filters = of('Filters (2)').width;
        // Never its chevron alone: as much as the least of the others.
        expect(name, greaterThan(24));
        expect(
          name,
          greaterThanOrEqualTo(math.min(range, filters) - 0.5),
          reason: 'name $name, range $range, filters $filters',
        );
      });
    }

    testWidgets("on a phone the ••• takes taps a finger's target around it, "
        'nothing moving, the button beside it keeping its own (Zucchero '
        'row 444)', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      debugDeviceCapabilities = DeviceCapabilities(idiom: Idiom.phone);
      addTearDown(() {
        debugDefaultTargetPlatformOverride = null;
        debugDeviceCapabilities = null;
      });
      final transport = await _show(
        tester,
        bar('Costs'),
        null,
        size: const Size(393, 844),
      );
      expect(tester.takeException(), isNull);
      final more = tester.getRect(
        find
            .ancestor(
              of: find.byWidgetPredicate(
                (w) => w is ExtensionGlyph && w.icon == 'ellipsis',
              ),
              matching: find.byType(Pressable),
            )
            .first,
      );
      final filters = tester.getRect(
        find
            .ancestor(
              of: find.text('Filters (2)'),
              matching: find.byType(Pressable),
            )
            .first,
      );
      // Drawn as before: narrower than a finger, at the page's edge.
      expect(more.width, lessThan(44));
      Object? which() =>
          (transport.actions.last.$2['action']! as Map)['context'];
      // Past its end, in the page's edge.
      await tester.tapAt(Offset(more.right + 7, more.center.dy));
      await tester.pump();
      expect(transport.actions, hasLength(1));
      expect(which(), {'which': 'more'});
      // In the gap before it.
      await tester.tapAt(Offset(more.left - 3, more.center.dy));
      await tester.pump();
      expect(transport.actions, hasLength(2));
      expect(which(), {'which': 'more'});
      // The button beside it keeps its taps to its very end.
      await tester.tapAt(Offset(filters.right - 1, filters.center.dy));
      await tester.pump();
      expect(which(), {'which': 'filters'});
      // Far off, nothing.
      await tester.tapAt(Offset(more.right + 40, more.center.dy));
      await tester.pump();
      expect(transport.actions, hasLength(3));
      // In the margin beside anything else (a line of the page, a button
      // that isn't the bar's): nothing.
      final note = tester.getRect(find.text('A line of the page'));
      await tester.tapAt(Offset(note.left - 7, note.center.dy));
      await tester.tapAt(Offset(more.right + 7, note.center.dy));
      await tester.pump();
      expect(transport.actions, hasLength(3));
      expect(tester.takeException(), isNull);
    });

    testWidgets('a short name keeps its words, the rest of the line '
        'empty before the others', (tester) async {
      // The test's font is wide: 700 is a phone's 402 in the app's.
      await _show(tester, bar('Costs'), null, size: const Size(700, 844));
      final paragraph = tester.renderObject<RenderParagraph>(
        find.descendant(
          of: find.text('Costs'),
          matching: find.byType(RichText),
        ),
      );
      expect(paragraph.didExceedMaxLines, isFalse);
      expect(tester.takeException(), isNull);
    });
  });

  testWidgets("a horizontal List's line wraps rather than run off the screen "
      "at the largest text (Zucchero, row 326); its buttons keep their own "
      'width', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await _show(
      tester,
      [
        _root(['bar', 'form']),
        {
          'id': 'bar',
          'component': 'List',
          'direction': 'horizontal',
          'align': 'center',
          'children': ['range', 'agent'],
        },
        {
          'id': 'range',
          'component': 'Button',
          'child': 'range_c',
          'variant': 'borderless',
        },
        {
          'id': 'range_c',
          'component': 'List',
          'direction': 'horizontal',
          'align': 'center',
          'children': ['range_t', 'range_i'],
        },
        {'id': 'range_t', 'component': 'Text', 'text': '23 Nov 08:00 – 14:00'},
        {'id': 'range_i', 'component': 'Icon', 'name': 'chevron-down'},
        {
          'id': 'agent',
          'component': 'Button',
          'child': 'agent_t',
          'variant': 'borderless',
        },
        {'id': 'agent_t', 'component': 'Text', 'text': 'Agent'},
        {
          'id': 'form',
          'component': 'List',
          'direction': 'horizontal',
          'children': ['save', 'cancel'],
        },
        {'id': 'save', 'component': 'Button', 'child': 'save_t'},
        {'id': 'save_t', 'component': 'Text', 'text': 'Save'},
        {'id': 'cancel', 'component': 'Button', 'child': 'cancel_t'},
        {'id': 'cancel_t', 'component': 'Text', 'text': 'No'},
      ],
      null,
      size: const Size(375, 1400),
    );
    expect(tester.takeException(), isNull);
    final screen = Offset.zero & const Size(375, 1400);
    for (final words in ['23 Nov 08:00 – 14:00', 'Agent', 'Save', 'No']) {
      final rect = tester.getRect(find.text(words));
      expect(screen.contains(rect.topLeft), isTrue, reason: words);
      expect(rect.right, lessThanOrEqualTo(375), reason: words);
    }
    // The range takes more than the line: the Agent goes under it.
    expect(
      tester.getTopLeft(find.text('Agent')).dy,
      greaterThan(tester.getBottomLeft(find.text('23 Nov 08:00 – 14:00')).dy),
    );
    // Two short buttons that fit stay side by side.
    expect(
      tester.getCenter(find.text('No')).dy,
      tester.getCenter(find.text('Save')).dy,
    );
    expect(
      tester.getTopLeft(find.text('No')).dx,
      greaterThan(tester.getTopRight(find.text('Save')).dx),
    );
  });

  group("a dashboard's rows on a phone (stackBelow)", () {
    List<Map<String, Object?>> grid({Object? stackBelow}) => [
      _root(['row']),
      {
        'id': 'row',
        'component': 'Row',
        'align': 'start',
        'children': ['a', 'b', 'gap'],
        'stackBelow': ?stackBelow,
      },
      {'id': 'a', 'component': 'Text', 'text': 'Panel A', 'weight': 3},
      {'id': 'b', 'component': 'Text', 'text': 'Panel B', 'weight': 3},
      {'id': 'gap', 'component': 'Column', 'children': [], 'weight': 6},
    ];
    Rect rect(WidgetTester tester, String text) =>
        tester.getRect(find.text(text));

    testWidgets('under it, panels go full width one under another and the '
        "grid's gaps go", (tester) async {
      await _show(
        tester,
        grid(stackBelow: 768),
        null,
        size: const Size(390, 844),
      );
      final a = rect(tester, 'Panel A');
      final b = rect(tester, 'Panel B');
      expect(b.top, greaterThan(a.bottom), reason: 'one under another');
      expect(a.left, b.left);
      final column = tester.widget<Column>(
        find
            .ancestor(of: find.text('Panel A'), matching: find.byType(Column))
            .first,
      );
      expect(column.children, hasLength(2), reason: 'the gap left out');
      expect(b.top - a.bottom, 20, reason: "Grafana's gap on a phone (P4)");
    });

    testWidgets('two stacked rows keep the same 20 between them (P4)', (
      tester,
    ) async {
      final one = grid(stackBelow: 768);
      await _show(
        tester,
        [
          _root(['row', 'row2']),
          ...one.skip(1),
          {
            ...one[1],
            'id': 'row2',
            'children': ['c', 'd'],
          },
          {'id': 'c', 'component': 'Text', 'text': 'Panel C', 'weight': 6},
          {'id': 'd', 'component': 'Text', 'text': 'Panel D', 'weight': 6},
        ],
        null,
        size: const Size(390, 844),
      );
      expect(rect(tester, 'Panel C').top - rect(tester, 'Panel B').bottom, 20);
      expect(rect(tester, 'Panel D').top - rect(tester, 'Panel C').bottom, 20);
    });

    testWidgets('at or above it, the weighted row keeps its gaps', (
      tester,
    ) async {
      await _show(tester, grid(stackBelow: 768), null);
      final a = rect(tester, 'Panel A');
      final b = rect(tester, 'Panel B');
      expect(a.top, b.top, reason: 'side by side');
      // A quarter each, the gap the other half.
      final row = tester.getRect(_evenRow.first);
      expect(b.left - row.left, closeTo((row.width - 16) / 4 + 8, 2));
    });

    testWidgets('side by side, every panel is as tall as the tallest, as '
        "Grafana's grid lines up panels of one height; one under another, "
        'each keeps its own (Zucchero row 467)', (tester) async {
      final panels = [
        _root(['row']),
        {
          'id': 'row',
          'component': 'Row',
          'align': 'start',
          'children': ['a', 'b'],
          'stackBelow': 768,
        },
        {'id': 'a', 'component': 'Card', 'child': 'a_t', 'weight': 8},
        {'id': 'a_t', 'component': 'Text', 'text': 'Map'},
        {'id': 'b', 'component': 'Card', 'child': 'b_col', 'weight': 4},
        {
          'id': 'b_col',
          'component': 'Column',
          'children': ['b1', 'b2', 'b3'],
        },
        {'id': 'b1', 'component': 'Text', 'text': 'Netherlands'},
        {'id': 'b2', 'component': 'Text', 'text': 'United States'},
        {'id': 'b3', 'component': 'Text', 'text': 'Iceland'},
      ];
      Rect card(String text) => tester.getRect(
        find
            .ancestor(of: find.text(text), matching: find.byType(Container))
            .first,
      );
      await _show(tester, panels, null, size: const Size(1024, 800));
      final map = card('Map');
      final list = card('Netherlands');
      expect(list.height, greaterThan(60), reason: 'three lines');
      expect(map.top, list.top);
      expect(map.height, list.height, reason: 'as tall as the tallest');
      // Each its weight's share, 8 between.
      expect(map.width, closeTo(2 * list.width, 0.5));
      expect(list.left - map.right, closeTo(8, 0.5));
      // Its words at the top, the room under them.
      expect(
        tester.getRect(find.text('Map')).top - card('Map').top,
        Space.m + 1,
        reason: 'its padding inside its hairline',
      );
      await _show(tester, panels, null, size: const Size(390, 800));
      expect(card('Map').height, lessThan(card('Netherlands').height));
    });

    testWidgets('it grows with the text size', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 1.5;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await _show(
        tester,
        grid(stackBelow: 768),
        null,
        size: const Size(1000, 900),
      );
      expect(
        rect(tester, 'Panel B').top,
        greaterThan(rect(tester, 'Panel A').bottom),
        reason: '1000 < 768 × 1.5',
      );
    });

    testWidgets('without it, a Row lays out as before, on a phone and at the '
        'largest text', (tester) async {
      await _show(tester, grid(), null, size: const Size(440, 956));
      // Over the least 120 each: side by side, the gap keeping its half.
      expect(rect(tester, 'Panel A').top, rect(tester, 'Panel B').top);
      final row = tester.getRect(find.byType(Row).first);
      expect(rect(tester, 'Panel B').right, lessThan(row.center.dx + 1));
      // At the largest text, the old rule stacks them, gap and all.
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await _show(tester, grid(), null, size: const Size(390, 844));
      expect(
        rect(tester, 'Panel B').top,
        greaterThan(rect(tester, 'Panel A').bottom),
      );
      final column = tester.widget<Column>(
        find
            .ancestor(of: find.text('Panel A'), matching: find.byType(Column))
            .first,
      );
      expect(column.children, hasLength(3), reason: 'the gap kept, as before');
    });

    testWidgets('a row of buttons with it stacks under it too', (tester) async {
      await _show(
        tester,
        [
          _root(['row']),
          {
            'id': 'row',
            'component': 'Row',
            'children': ['one', 'two'],
            'stackBelow': 768,
          },
          {'id': 'one', 'component': 'Text', 'text': 'One'},
          {'id': 'two', 'component': 'Text', 'text': 'Two'},
        ],
        null,
        size: const Size(390, 844),
      );
      expect(rect(tester, 'Two').top, greaterThan(rect(tester, 'One').bottom));
    });
  });

  group("Scolippi's fidelity review against Grafana 13.3.1", () {
    testWidgets(
      "F2, F3, F10: axis labels and legend names in the title's color; "
      'gridlines one device pixel at 9% white',
      (tester) async {
        tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
        addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
        await _show(tester, [
          _root(['cpu']),
          cpu(),
        ], cpuData);
        final title = tester.widget<Text>(find.text('CPU')).style!.color;
        expect(title, const Color(0xFFCCCCDC), reason: "F10: Grafana's text");
        expect(tester.widget<Text>(find.text('user')).style!.color, title);
        final axis = tester.widget<Text>(find.text('00:00').first);
        expect(axis.style!.color, title);
        final chart = tester.widget<LineChart>(find.byType(LineChart)).data;
        final line = chart.gridData.getDrawingHorizontalLine(10);
        expect(line.color, const Color(0x17F0FAFF));
        expect(line.strokeWidth, 1, reason: 'one pixel at 1x');
      },
    );

    testWidgets("F4, F6, F7: a stat's value as large as Grafana's at its "
        'height; a fill below the title, a horizontal gradient', (
      tester,
    ) async {
      tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      await _show(
        tester,
        [
          _root(['c1', 'c2']),
          {'id': 'c1', 'component': 'Card', 'child': 'plain'},
          {
            'id': 'plain',
            'component': 'Stat',
            'label': 'Plain',
            'value': 43.8,
            'spark': [1, 3, 2],
            'height': 139,
          },
          {'id': 'c2', 'component': 'Card', 'child': 'filled'},
          {
            'id': 'filled',
            'component': 'Stat',
            'label': 'Filled',
            'value': 43.8,
            'colorMode': 'background',
            'height': 139,
          },
        ],
        null,
        size: const Size(393, 852),
      );
      double fontOf(String label) {
        final rich = tester.widget<RichText>(
          find.descendant(
            of: find.ancestor(
              of: find.text(label),
              matching: find.byType(AppStat),
            ),
            matching: find.byWidgetPredicate(
              (w) => w is RichText && w.softWrap == false,
            ),
          ),
        );
        double? size;
        rich.text.visitChildren((span) {
          size = span.style?.fontSize;
          return size == null;
        });
        return size!;
      }

      // Grafana's 43.8 at a 139pt full-width panel: about 33px.
      expect(fontOf('Plain'), inInclusiveRange(30, 37));
      // The fill starts under the title, which keeps the panel's colors.
      final stat = tester.getRect(
        find.ancestor(of: find.text('Filled'), matching: find.byType(AppStat)),
      );
      final fill = find.descendant(
        of: find.ancestor(
          of: find.text('Filled'),
          matching: find.byType(AppStat),
        ),
        matching: find.byWidgetPredicate(
          (w) =>
              w is DecoratedBox &&
              (w.decoration as BoxDecoration).gradient != null,
        ),
      );
      expect(
        tester.getRect(fill).top,
        greaterThan(tester.getRect(find.text('Filled')).bottom),
      );
      expect(tester.getRect(fill).bottom, stat.bottom);
      expect(
        tester.widget<Text>(find.text('Filled')).style!.color,
        tester.widget<Text>(find.text('Plain')).style!.color,
      );
      final gradient =
          (tester.widget<DecoratedBox>(fill).decoration as BoxDecoration)
                  .gradient!
              as LinearGradient;
      expect(
        (gradient.begin, gradient.end),
        (Alignment.centerLeft, Alignment.centerRight),
      );
      // Grafana's green: #429A43 to #6AB455.
      expect(gradient.colors.first.r, closeTo(0x42 / 255, 0.02));
      expect(gradient.colors.first.g, closeTo(0x9A / 255, 0.02));
      expect(gradient.colors.last.g, closeTo(0xB4 / 255, 0.02));
    });
  });

  group("Scolippi's tooltip review (T1-T5)", () {
    final closeButton = find.byWidgetPredicate(
      (w) => w is CustomPaint && w.painter.runtimeType.toString() == '_Cross',
    );
    testWidgets("a finger's tooltip: a crosshair and the point marked, "
        "Grafana's rows and tones, and a close button", (tester) async {
      tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      await _show(
        tester,
        [
          _root(['cpu']),
          cpu(),
        ],
        {
          'cpu': _table(
            ['time', 'user'],
            [
              for (var h = 0; h <= 12; h++)
                [
                  '2026-11-22 ${h.toString().padLeft(2, '0')}:00:00',
                  37.8234 + h,
                ],
            ],
          ),
        },
      );
      final rect = tester.getRect(find.byType(LineChart));
      await tester.tapAt(Offset(rect.right - 4, rect.top + 20));
      await tester.pump();
      final crosshair = find.byWidgetPredicate(
        (w) =>
            w is CustomPaint &&
            w.painter.runtimeType.toString() == '_Crosshair',
      );
      expect(crosshair, findsOneWidget);
      // T3: the time in the panel's text color, the name at 65%, the value
      // 500 at the same size. T4: Grafana's decimals.
      final time = tester.widget<Text>(find.text('2026-11-22 12:00:00'));
      expect(time.style!.color, const Color(0xFFCCCCDC));
      final value = tester.widget<Text>(find.text('49.8'));
      expect(value.style!.fontWeight, FontWeight.w500);
      expect(value.style!.fontSize, time.style!.fontSize);
      final names = tester.widgetList<Text>(find.text('user')).toList();
      expect(
        names.map((t) => t.style!.color!.a),
        contains(closeTo(0.65 * 1, 0.01)),
      );
      // T5: pinned by a finger, it has a close button.
      await tester.tap(closeButton);
      await tester.pump();
      expect(find.text('2026-11-22 12:00:00'), findsNothing);
      expect(crosshair, findsNothing);
    });

    testWidgets("a mouse's tooltip has no close button", (tester) async {
      await _show(tester, [
        _root(['cpu']),
        cpu(),
      ], cpuData);
      final rect = tester.getRect(find.byType(LineChart));
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: Offset.zero);
      addTearDown(mouse.removePointer);
      await mouse.moveTo(Offset(rect.right - 4, rect.top + 20));
      await tester.pump();
      expect(find.text('2026-11-22 12:00:00'), findsOneWidget);
      expect(closeButton, findsNothing);
    });

    test('a 2 hour step between 1 and 3 (F1), and Grafana decimals in a '
        'tooltip (T4)', () {
      expect(
        timeStep(const Duration(hours: 6), most: 4),
        const Duration(hours: 2),
      );
      expect(chartValue(37.8234, ''), '37.8');
      expect(chartValue(12345.6, 'tokens'), '12,346 tokens');
    });
  });

  group('a world map', () {
    Map<String, Object?> map(Map<String, Object?> props) => {
      'id': 'm',
      'component': 'Map',
      'data': {'@path': '/data'},
      ...props,
    };

    String? said(WidgetTester tester) =>
        tester.getSemantics(find.byType(AppMap)).getSemanticsData().value;

    test('a country by its code or its English name, as GeoIP spells it', () {
      expect(countryName('gbr'), 'United Kingdom');
      expect(countryName(' tr '), 'Turkey');
      expect(countryName('Türkiye'), 'Turkey');
      expect(countryName('TURKIYE'), 'Turkey');
      expect(countryName('Netherlands'), 'Netherlands');
      expect(countryName('DR Congo'), 'Democratic Republic of the Congo');
      expect(countryName('Congo Republic'), 'Congo');
      expect(countryName('Ivory Coast'), "Côte d'Ivoire");
      expect(countryName('Côte d’Ivoire'), "Côte d'Ivoire");
      expect(countryName('cote  d\'ivoire'), "Côte d'Ivoire");
      expect(countryName('Hashemite Kingdom of Jordan'), 'Jordan');
      expect(countryName('United States of America'), 'United States');
      expect(countryName('UK'), 'United Kingdom');
      expect(countryName('Bosnia and Herzegovina'), 'Bosnia and Herzegovina');
      expect(countryName('Réunion'), 'Réunion');
      expect(countryName('Unknown'), isNull);
      expect(countryName('Other'), isNull);
      expect(countryName('XX'), isNull);
    });

    testWidgets('places countries by code or name in any case, '
        'sums rows of one country, and lists in words what it cannot '
        'place', (tester) async {
      final semantics = tester.ensureSemantics();
      await _show(
        tester,
        [
          _root(['m']),
          map({'label': 'Users', 'unit': 'users'}),
        ],
        {
          'data': _table(
            ['country', 'users'],
            [
              ['US', 38],
              ['usa', 2],
              ['United States', 2],
              ['de', 12],
              ['FRA', 7],
              ['Hidden (fewer than 5)', 9],
              ['Unknown\u202E', 3],
              ['IT', null],
              ['', 1],
            ],
          ),
        },
        size: const Size(393, 852),
      );
      expect(
        said(tester),
        'United States 42 users, Germany 12 users, France 7 users. Not on '
        'the map: Hidden (fewer than 5) 9 users, Unknown\\u{202E} 3 users, '
        'Italy: no value, No country 1 users',
      );
      expect(
        find.text(
          'Not on the map: Hidden (fewer than 5) 9 users · '
          'Unknown\\u{202E} 3 users · Italy: no value · No country 1 users',
        ),
        findsOneWidget,
      );
      // The legend: the least value to the most.
      expect(find.text('7 users'), findsOneWidget);
      expect(find.text('42 users'), findsOneWidget);
      semantics.dispose();
    });

    testWidgets('a tap on a dot shows its country and value, and a tap on '
        'it again closes it', (tester) async {
      await _show(
        tester,
        [
          _root(['m']),
          // Taller than the world at a phone's width: no bands of sea.
          map({'value': 'n', 'height': 372}),
        ],
        {
          'data': _table(
            ['where', 'n'],
            [
              ['BR', 5],
              ['JP', 3],
            ],
          ),
        },
        size: const Size(393, 852),
      );
      final box = tester.getRect(
        find.descendant(
          of: find.byType(AppMap),
          matching: find.byType(ClipRect),
        ),
      );
      double mercator(double lat) =>
          math.log(math.tan(math.pi / 4 + lat * math.pi / 360));
      expect(
        box.height,
        moreOrLessEquals(
          box.width * (mercator(76) - mercator(-56)) / (2 * math.pi),
          epsilon: 0.5,
        ),
      );
      // Brazil's dot, where the map puts it.
      final brazil = Offset(
        box.left + (-49.6 + 180) / 360 * box.width,
        box.top +
            (mercator(76) - mercator(-12.1)) /
                (mercator(76) - mercator(-56)) *
                box.height,
      );
      await tester.tapAt(brazil);
      await tester.pump();
      expect(find.text('Brazil'), findsOneWidget);
      await tester.tapAt(brazil);
      await tester.pump();
      expect(find.text('Brazil'), findsNothing);
      // Far from any dot: nothing.
      await tester.tapAt(box.topLeft + const Offset(4, 4));
      await tester.pump();
      expect(find.text('Japan'), findsNothing);
    });

    testWidgets("thresholds color the dots and say their ranges; a map "
        "without a table says it can't show", (tester) async {
      await _show(
        tester,
        [
          _root(['m', 'bad']),
          map({
            'thresholds': [
              {'color': 'green'},
              {'color': 'red', 'value': 10},
            ],
          }),
          {'id': 'bad', 'component': 'Map', 'data': 'not a table'},
        ],
        {
          'data': _table(
            ['country', 'n'],
            [
              ['IN', 4],
              ['CN', 20],
            ],
          ),
        },
        size: const Size(393, 852),
      );
      expect(find.text('< 10'), findsOneWidget);
      expect(find.text('10+'), findsOneWidget);
      expect(find.text("Can't show this map."), findsOneWidget);
    });
  });

  group('a heatmap', () {
    testWidgets('rows and columns in the order the data has them, cells '
        'summed, an empty cell left empty, and a tap says a cell', (
      tester,
    ) async {
      final semantics = tester.ensureSemantics();
      await _show(
        tester,
        [
          _root(['h']),
          {
            'id': 'h',
            'component': 'Chart',
            'kind': 'heatmap',
            'label': 'Sessions',
            'data': {'@path': '/data'},
            'x': 'hour',
            'seriesBy': 'day',
            'y': ['n'],
          },
        ],
        {
          'data': _table(
            ['day', 'hour', 'n'],
            [
              ['Tue', '09', 3],
              ['Mon', '09', 1],
              ['Mon', '10', 2],
              ['Mon', '10', 5],
              ['Tue', '10', null],
              // A row nothing is known for keeps its place, empty.
              ['Wed', '09', null],
              ['Wed', '10', null],
            ],
          ),
        },
        size: const Size(393, 852),
      );
      expect(
        tester.getSemantics(find.byType(AppHeatmap)).getSemanticsData().value,
        '3 by 2, from 1 to 7; the most, 7, at Mon · 10',
      );
      // Tue comes first, as the data has it: its row is on top.
      final tue = tester.getRect(find.text('Tue'));
      final mon = tester.getRect(find.text('Mon'));
      expect(tue.top, lessThan(mon.top));
      expect(tester.getRect(find.text('Wed')).top, greaterThan(mon.top));
      final ten = tester.getRect(find.text('10').first);
      await tester.tapAt(Offset(ten.center.dx, mon.center.dy));
      await tester.pump();
      expect(find.text('Mon · 10'), findsOneWidget);
      semantics.dispose();
    });

    testWidgets('nothing to show without values', (tester) async {
      await _show(
        tester,
        [
          _root(['h']),
          {
            'id': 'h',
            'component': 'Chart',
            'kind': 'heatmap',
            'data': {'@path': '/data'},
            'y': ['n'],
          },
        ],
        {
          'data': _table(
            ['day', 'hour', 'n'],
            [
              ['Mon', '09', null],
            ],
          ),
        },
      );
      expect(find.text('Nothing to show.'), findsOneWidget);
    });
  });
}
