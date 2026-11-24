// Golden screenshots of the Charts components as a phone draws them: a
// dashboard's panels (Stat, time series with one and several series,
// scatter, bars) in Cards at an iPhone's width, in Geist as the app ships
// it. They pin the renderer's look so each step towards Grafana's defaults
// shows exactly what changed.
//
// Update them after an intended change of look:
//   flutter test --update-goldens test/app_charts_golden_test.dart
import 'dart:convert';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:factory_app/app_views/app_view.dart';
import 'package:factory_app/app_views/charts.dart';
import 'package:factory_app/app_views/transport.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' as pb;
import 'package:fl_chart/fl_chart.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_views_test.dart' show FakeAppViewTransport;
import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

const _canvas = Key('canvas');

/// An iPhone 16's width, in points.
const _phone = 393.0;

Future<void> _show(
  WidgetTester tester,
  List<Map<String, Object?>> components,
  Object? data, {
  double height = 852,
  double width = _phone,
  Brightness brightness = Brightness.dark,
}) async {
  tester.platformDispatcher.platformBrightnessTestValue = brightness;
  addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
  final snapshot = jsonEncode({
    'version': '1.0',
    'createSurface': {
      'surfaceId': 'charts-1',
      'catalogId': appViewCatalogV1,
      'components': components,
      'dataModel': data,
    },
  });
  final transport = FakeAppViewTransport()
    ..answer = (_) => AppSurfaceData(
      surfaceId: 'charts-1',
      appId: 'charts',
      title: 'Charts',
      state: AppViewState.open,
      snapshot: snapshot,
      seq: 0,
      error: '',
    );
  await Harness(_workspace).show(
    tester,
    size: Size(width, height),
    // On the window's canvas, as the app shows a view.
    home: Builder(
      builder: (context) => ColoredBox(
        key: _canvas,
        color: Palette.of(context).canvas,
        child: AppView(
          transport: transport,
          appId: 'charts',
          appName: 'Charts',
        ),
      ),
    ),
  );
  await tester.pump();
  await tester.pump(Motion.base);
  await tester.pump(const Duration(seconds: 1));
}

final _workspace = pb.Workspace()
  ..agents.addAll([
    pb.Agent(id: 'a1', name: 'June', color: 'pink'),
    pb.Agent(id: 'a2', name: 'Kai', color: 'teal'),
  ]);

/// A panel as the Charts app sends one: a Card around [child].
List<Map<String, Object?>> _card(String id, Map<String, Object?> child) => [
  {'id': id, 'component': 'Card', 'child': '${id}_body'},
  {...child, 'id': '${id}_body'},
];

/// A row of panels weighted as the Charts app's 12-column grid.
Map<String, Object?> _row(String id, List<(String, int)> cells) => {
  'id': id,
  'component': 'Row',
  'align': 'start',
  'children': [for (final (cell, _) in cells) cell],
};

List<Map<String, Object?>> _weighted(
  List<Map<String, Object?>> card,
  int weight,
) => [
  {...card.first, 'weight': weight},
  ...card.skip(1),
];

/// Hourly times over a day, as SQL gives them (local, no zone).
String _hour(int h) {
  final time = DateTime(2026, 11, 22, 0).add(Duration(hours: h));
  String two(int n) => n.toString().padLeft(2, '0');
  return '${time.year}-${two(time.month)}-${two(time.day)} '
      '${two(time.hour)}:00:00';
}

/// A smooth, deterministic wave around [base].
double _wave(int h, double base, double size, double phase) =>
    (base + size * math.sin(h / 4 + phase) + size / 3 * math.cos(h / 1.7))
        .roundToDouble();

Map<String, Object?> _series(int count) => {
  'columns': [
    'time',
    for (var s = 0; s < count; s++) ['cpu', 'mem', 'io'][s],
  ],
  'rows': [
    for (var h = 0; h <= 24; h++)
      [
        _hour(h),
        for (var s = 0; s < count; s++)
          _wave(h, 60 + 15.0 * s, 12, s.toDouble()),
      ],
  ],
};

Future<void> _golden(WidgetTester tester, String name) => expectLater(
  find.byKey(_canvas),
  matchesGoldenFile('goldens/charts_${name}_$_shade.png'),
);

/// The theme the scenes are drawn in: each runs in both.
var _shade = 'dark';

void main() {
  setUpAll(() async {
    await loadGeist();
    // No debug banner over the corner.
    WidgetsApp.debugAllowBannerOverride = false;
  });

  for (final brightness in Brightness.values) {
    testWidgets('${brightness.name}: stats', (tester) async {
      _shade = brightness.name;
      final runs = _card('runs', {
        'component': 'Stat',
        'label': 'Runs',
        'value': {'@path': '/runs'},
        'spark': {'@path': '/spark'},
        'height': 100,
      });
      final tokens = _card('tokens', {
        'component': 'Stat',
        'label': 'Tokens',
        'value': {'@path': '/tokens'},
        'unit': 'tokens',
        'spark': {'@path': '/spark'},
        'height': 100,
      });
      final alone = _card('alone', {
        'component': 'Stat',
        'label': 'Errors',
        'value': 3,
        'spark': [0, 1, 0, 2, 0],
        'thresholds': [
          {'color': 'green'},
          {'color': 'red', 'value': 1},
        ],
        'height': 100,
      });
      await _show(
        tester,
        [
          {
            'id': 'root',
            'component': 'Column',
            'children': ['r0', 'r1'],
          },
          _row('r0', [('runs', 6), ('tokens', 6)]),
          ..._weighted(runs, 6),
          ..._weighted(tokens, 6),
          _row('r1', [('alone', 6), ('gap', 6)]),
          ..._weighted(alone, 6),
          {'id': 'gap', 'component': 'Column', 'children': [], 'weight': 6},
        ],
        {
          'runs': 1284,
          'tokens': 3400000,
          'spark': [for (var h = 0; h <= 24; h++) _wave(h, 50, 20, 0)],
        },
        height: 520,
        brightness: brightness,
      );
      await _golden(tester, 'stats');
    });

    testWidgets('${brightness.name}: stat color modes', (tester) async {
      _shade = brightness.name;
      final stats = <(String, Map<String, Object?>)>[
        (
          'bg',
          {
            'label': 'Background',
            'value': 42.5,
            'unit': 'percent',
            'colorMode': 'background',
          },
        ),
        (
          'solid',
          {
            'label': 'Solid',
            'value': 91,
            'unit': 'percent',
            'colorMode': 'solid',
          },
        ),
        (
          'none',
          {
            'label': 'None',
            'value': 1536,
            'unit': 'bytes',
            'colorMode': 'none',
          },
        ),
        (
          'nodata',
          {
            'label': 'No rows',
            'value': {'@path': '/nothing'},
          },
        ),
        (
          'yellow',
          {
            'label': 'Yellow',
            'value': 61,
            'graph': 'none',
            'thresholds': [
              {'color': 'green'},
              {'color': 'yellow', 'value': 50},
              {'color': 'red', 'value': 80},
            ],
          },
        ),
        ('agent', {'label': 'June', 'value': 7, 'color': 'agent:a1'}),
      ];
      await _show(
        tester,
        [
          {
            'id': 'root',
            'component': 'Column',
            'children': ['r0', 'r1', 'r2'],
          },
          for (var r = 0; r < 3; r++)
            _row('r$r', [(stats[r * 2].$1, 6), (stats[r * 2 + 1].$1, 6)]),
          for (final (id, stat) in stats)
            ..._weighted(
              _card(id, {
                'component': 'Stat',
                'spark': [for (var h = 0; h <= 24; h++) _wave(h, 50, 20, 0)],
                'height': 100,
                ...stat,
              }),
              6,
            ),
        ],
        {'nothing': null},
        height: 440,
        brightness: brightness,
      );
      await _golden(tester, 'stat_modes');
    });

    testWidgets('${brightness.name}: a dashboard on a phone', (tester) async {
      _shade = brightness.name;
      Map<String, Object?> row(String id, List<String> cells) => {
        ..._row(id, const []),
        'children': cells,
        'stackBelow': 768,
      };
      await _show(
        tester,
        [
          {
            'id': 'root',
            'component': 'Column',
            'children': ['r0', 'r1'],
          },
          row('r0', ['runs', 'tokens']),
          ..._weighted(
            _card('runs', {
              'component': 'Stat',
              'label': 'Runs',
              'value': 1284,
              'spark': [for (var h = 0; h <= 24; h++) _wave(h, 50, 20, 0)],
              'thresholds': [
                {'color': 'green'},
              ],
              'height': 100,
            }),
            6,
          ),
          ..._weighted(
            _card('tokens', {
              'component': 'Stat',
              'label': 'Tokens',
              'value': 3400000,
              'unit': 'short',
              'spark': [for (var h = 0; h <= 24; h++) _wave(h, 50, 20, 1)],
              'thresholds': [
                {'color': 'green'},
              ],
              'height': 100,
            }),
            6,
          ),
          row('r1', ['cpu', 'gap']),
          ..._weighted(
            _card('cpu', {
              'component': 'Chart',
              'kind': 'timeseries',
              'label': 'CPU',
              'data': {'@path': '/data'},
              'x': 'time',
              'unit': 'percent',
              'height': 200,
            }),
            6,
          ),
          {'id': 'gap', 'component': 'Column', 'children': [], 'weight': 6},
        ],
        {'data': _series(2)},
        height: 760,
        brightness: brightness,
      );
      await _golden(tester, 'dashboard');
    });

    testWidgets('${brightness.name}: time series, one series', (tester) async {
      _shade = brightness.name;
      await _show(
        tester,
        [
          {
            'id': 'root',
            'component': 'Column',
            'children': ['p'],
          },
          ..._card('p', {
            'component': 'Chart',
            'kind': 'timeseries',
            'label': 'CPU',
            'data': {'@path': '/data'},
            'x': 'time',
            'color': 'palette',
            'unit': '%',
            'height': 240,
          }),
        ],
        {'data': _series(1)},
        height: 420,
        brightness: brightness,
      );
      await _golden(tester, 'timeseries_one');
    });

    testWidgets('${brightness.name}: a finger tooltip', (tester) async {
      _shade = brightness.name;
      await _show(
        tester,
        [
          {
            'id': 'root',
            'component': 'Column',
            'children': ['p'],
          },
          ..._card('p', {
            'component': 'Chart',
            'kind': 'timeseries',
            'label': 'Load',
            'data': {'@path': '/data'},
            'x': 'time',
            'height': 240,
          }),
        ],
        {'data': _series(3)},
        height: 440,
        brightness: brightness,
      );
      final rect = tester.getRect(find.byType(LineChart));
      await tester.tapAt(Offset(rect.center.dx, rect.top + 60));
      await tester.pump();
      await _golden(tester, 'tooltip');
    });

    testWidgets('${brightness.name}: time series, three series', (
      tester,
    ) async {
      _shade = brightness.name;
      await _show(
        tester,
        [
          {
            'id': 'root',
            'component': 'Column',
            'children': ['p'],
          },
          ..._card('p', {
            'component': 'Chart',
            'kind': 'timeseries',
            'label': 'Load',
            'data': {'@path': '/data'},
            'x': 'time',
            'color': 'palette',
            'height': 240,
          }),
        ],
        {'data': _series(3)},
        height: 440,
        brightness: brightness,
      );
      await _golden(tester, 'timeseries_three');
    });

    testWidgets('${brightness.name}: scatter', (tester) async {
      _shade = brightness.name;
      await _show(
        tester,
        [
          {
            'id': 'root',
            'component': 'Column',
            'children': ['p'],
          },
          ..._card('p', {
            'component': 'Chart',
            'kind': 'scatter',
            'label': 'Tokens by run length',
            'data': {'@path': '/data'},
            'x': 'seconds',
            'seriesBy': 'agent',
            'y': ['tokens'],
            'color': 'palette',
            'height': 240,
          }),
        ],
        {
          'data': {
            'columns': ['seconds', 'agent', 'tokens'],
            'rows': [
              for (var i = 0; i < 30; i++)
                [
                  20 + (i * 37) % 200,
                  i.isEven ? 'June' : 'Kai',
                  1000 + ((i * 7919) % 5000) + (i.isEven ? 2000 : 0),
                ],
            ],
          },
        },
        height: 440,
        brightness: brightness,
      );
      await _golden(tester, 'scatter');
    });

    testWidgets('${brightness.name}: bars', (tester) async {
      _shade = brightness.name;
      await _show(
        tester,
        [
          {
            'id': 'root',
            'component': 'Column',
            'children': ['p'],
          },
          ..._card('p', {
            'component': 'Chart',
            'kind': 'bars',
            'label': 'Runs a day',
            'data': {'@path': '/data'},
            'x': 'day',
            'color': 'palette',
            'height': 200,
          }),
        ],
        {
          'data': {
            'columns': ['day', 'runs'],
            'rows': [
              for (var d = 16; d <= 22; d++) ['2026-11-$d', 10 + (d * 13) % 25],
            ],
          },
        },
        height: 400,
        brightness: brightness,
      );
      await _golden(tester, 'bars');
    });

    for (final (width, scale) in [(402.0, 1.0), (320.0, 1.0), (320.0, 1.6)]) {
      testWidgets("${brightness.name}: a dashboard's bar on one line, "
          "${width.toInt()} wide, text ×$scale", (tester) async {
        _shade = brightness.name;
        debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
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
              'event': {'name': 'menu'},
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
        await _show(
          tester,
          [
            {
              'id': 'root',
              'component': 'Column',
              'children': ['bar', 'p1'],
            },
            {
              'id': 'bar',
              'component': 'Row',
              'children': ['name', 'range', 'agent', 'more'],
            },
            ...button('name', weight: 1, [
              {
                'id': 'name_t',
                'component': 'Text',
                'text': 'Agents at work',
                'variant': 'heading',
              },
              {'id': 'name_i', 'component': 'Icon', 'name': 'chevron-down'},
            ]),
            ...button('range', [
              {'id': 'range_t', 'component': 'Text', 'text': '30 days'},
              {'id': 'range_i', 'component': 'Icon', 'name': 'chevron-down'},
            ]),
            ...button('agent', [
              {'id': 'agent_t', 'component': 'Text', 'text': 'Agent: All'},
              {'id': 'agent_i', 'component': 'Icon', 'name': 'chevron-down'},
            ]),
            ...button('more', [
              {'id': 'more_l', 'component': 'Icon', 'name': 'ellipsis'},
            ]),
            ..._card('p1', {
              'component': 'Stat',
              'label': 'Runs',
              'value': 42,
              'spark': [1, 3, 2, 5, 4],
              'height': 100,
            }),
          ],
          null,
          width: width,
          height: 260,
          brightness: brightness,
        );
        expect(tester.takeException(), isNull);
        await _golden(tester, 'bar_${width.toInt()}_${(scale * 10).round()}');
      });
    }

    testWidgets("${brightness.name}: the dashboards' menu on a phone: "
        'Starred, then all, the home one marked', (tester) async {
      _shade = brightness.name;
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      const boards = [
        ('Cards', 'Starred', 'star'),
        ('Costs', 'Starred', 'star'),
        ('Agents at work', 'All dashboards', ''),
        ('Cards', 'All dashboards', 'star'),
        ('Claktory', 'All dashboards', 'house'),
        ('Costs', 'All dashboards', 'star'),
        ('Fidelity', 'All dashboards', ''),
      ];
      await _show(
        tester,
        [
          {
            'id': 'root',
            'component': 'Column',
            'children': ['name'],
          },
          {
            'id': 'name',
            'component': 'Button',
            'child': 'name_t',
            'variant': 'borderless',
            'menu': {
              'title': 'Dashboards',
              'options': [
                for (final (i, (label, section, icon)) in boards.indexed)
                  {
                    'label': label,
                    'section': section,
                    if (icon.isNotEmpty) 'icon': icon,
                    if (i == 4) 'selected': true,
                    'action': {
                      'event': {'name': 'board'},
                    },
                  },
              ],
            },
          },
          {'id': 'name_t', 'component': 'Text', 'text': 'Claktory'},
        ],
        null,
        height: 700,
        brightness: brightness,
      );
      await tester.tap(find.text('Claktory'));
      await tester.pumpAndSettle();
      await expectLater(
        find.byType(Overlay).first,
        matchesGoldenFile('goldens/charts_dashboards_menu_$_shade.png'),
      );
    });

    for (final (device, platform, width) in [
      ('phone', TargetPlatform.iOS, _phone),
      ('mac', TargetPlatform.macOS, 700.0),
    ]) {
      testWidgets("${brightness.name}: a Button's menu of 50 agents open on "
          "a $device", (tester) async {
        _shade = brightness.name;
        debugDefaultTargetPlatformOverride = platform;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        await _show(
          tester,
          [
            {
              'id': 'root',
              'component': 'Column',
              'children': ['bar', 'p1'],
            },
            {
              'id': 'bar',
              'component': 'List',
              'direction': 'horizontal',
              'align': 'center',
              'children': ['range', 'agent'],
            },
            for (final (id, words, title, options) in [
              (
                'range',
                'Last 7 days',
                'Time range',
                ['Last 1 hour', 'Last 24 hours', 'Last 7 days'],
              ),
              (
                'agent',
                'Agent: All',
                'Agent',
                ['All', for (var i = 1; i < 50; i++) 'agent-$i'],
              ),
            ]) ...[
              {
                'id': id,
                'component': 'Button',
                'child': '${id}_c',
                'variant': 'borderless',
                'menu': {
                  'title': title,
                  'options': [
                    for (final (i, label) in options.indexed)
                      {
                        'label': label,
                        if (i == (id == 'range' ? 2 : 0)) 'selected': true,
                        'action': {
                          'event': {'name': id},
                        },
                      },
                  ],
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
            ],
            ..._card('p1', {
              'component': 'Stat',
              'label': 'Runs',
              'value': 42,
              'spark': [1, 3, 2, 5, 4],
            }),
          ],
          null,
          width: width,
          height: 700,
          brightness: brightness,
        );
        await tester.tap(find.text('Agent: All'));
        await tester.pumpAndSettle();
        await expectLater(
          find.byType(Overlay).first,
          matchesGoldenFile('goldens/charts_button_menu_${device}_$_shade.png'),
        );
      });
    }

    testWidgets('${brightness.name}: a menu, its chosen row marked', (
      tester,
    ) async {
      _shade = brightness.name;
      const words = ['Last 1 hour', 'Last 24 hours', 'Last 7 days'];
      await _show(
        tester,
        [
          {
            'id': 'root',
            'component': 'Column',
            'children': ['menu'],
          },
          {'id': 'menu', 'component': 'Card', 'child': 'menu_col'},
          {
            'id': 'menu_col',
            'component': 'Column',
            'children': [for (var i = 0; i < words.length; i++) 'menu_o$i'],
          },
          for (final (i, text) in words.indexed) ...[
            {
              'id': 'menu_o$i',
              'component': 'Button',
              'child': 'menu_o${i}_c',
              'variant': i == 2 ? 'primary' : 'borderless',
              'action': {
                'event': {'name': 'range'},
              },
            },
            {
              'id': 'menu_o${i}_c',
              'component': 'List',
              'direction': 'horizontal',
              'children': ['menu_o${i}_t'],
            },
            {'id': 'menu_o${i}_t', 'component': 'Text', 'text': text},
          ],
        ],
        null,
        height: 260,
        brightness: brightness,
      );
      await _golden(tester, 'menu');
    });

    testWidgets('${brightness.name}: a top bar on a Mac, a menu button '
        'hovered', (tester) async {
      _shade = brightness.name;
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      List<Map<String, Object?>> menuButton(String id, String words) => [
        {
          'id': id,
          'component': 'Button',
          'child': '${id}_c',
          'variant': 'borderless',
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
          {
            'id': 'root',
            'component': 'Column',
            'children': ['bar', 'note'],
          },
          {
            'id': 'bar',
            'component': 'List',
            'direction': 'horizontal',
            'align': 'center',
            'children': ['board', 'range'],
          },
          ...menuButton('board', 'Fidelity'),
          ...menuButton('range', 'Last 6 hours'),
          {'id': 'note', 'component': 'Text', 'text': 'A line of the page'},
        ],
        null,
        height: 160,
        brightness: brightness,
      );
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: tester.getCenter(find.text('Fidelity')));
      addTearDown(mouse.removePointer);
      await tester.pump();
      await _golden(tester, 'top_bar_mac');
    });
  }

  for (final ratio in [2.0, 3.0]) {
    testWidgets("a filled Stat stays inside its card's hairline at "
        '${ratio.toInt()}x', (tester) async {
      tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
      addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
      await _show(
        tester,
        [
          {
            'id': 'root',
            'component': 'Column',
            'children': ['c'],
          },
          ..._card('c', {
            'component': 'Stat',
            'label': 'Solid',
            'value': 91,
            'colorMode': 'solid',
            'color': 'red',
            'height': 100,
          }),
        ],
        null,
        height: 300,
      );
      tester.view.physicalSize = const Size(_phone, 300) * ratio;
      tester.view.devicePixelRatio = ratio;
      await tester.pump();
      final card = tester.getRect(find.byType(AppStatCard));
      final image = await captureImage(find.byKey(_canvas).evaluate().single);
      final bytes = (await tester.runAsync(
        () => image.toByteData(format: ui.ImageByteFormat.rawRgba),
      ))!;
      Color at(int x, int y) {
        final i = (y * image.width + x) * 4;
        return Color.fromARGB(
          bytes.getUint8(i + 3),
          bytes.getUint8(i),
          bytes.getUint8(i + 1),
          bytes.getUint8(i + 2),
        );
      }

      // The bottom edge's middle: the hairline (#232327), not the red.
      final middle = (card.center.dx * ratio).round();
      final bottom = (card.bottom * ratio).round() - 1;
      final edge = at(middle, bottom - (ratio / 2).floor());
      expect(edge.r, lessThan(0.3), reason: 'hairline over the fill: $edge');
      // Just inside it: the red fill.
      final inside = at(middle, bottom - (ratio * 4).round());
      expect(inside.r, greaterThan(0.8), reason: 'filled: $inside');
      image.dispose();
    });
  }

  // Scolippi's F1 against Grafana 13.3.1, in Geist as the phone draws it.
  testWidgets('a time label each hour over six hours on a phone, the '
      'ends included', (tester) async {
    await _show(
      tester,
      [
        {
          'id': 'root',
          'component': 'Column',
          'children': ['cpu'],
        },
        {
          'id': 'cpu',
          'component': 'Chart',
          'kind': 'timeseries',
          'data': {'@path': '/cpu'},
          'x': 'time',
          'label': 'CPU',
          'height': 240,
        },
      ],
      {
        'cpu': {
          'columns': ['time', 'user'],
          'rows': [
            for (var m = 0; m <= 360; m += 5)
              [
                DateTime(
                  2026,
                  11,
                  22,
                  8,
                ).add(Duration(minutes: m)).millisecondsSinceEpoch,
                40 + m % 7,
              ],
          ],
        },
      },
      height: 400,
    );
    for (var h = 8; h <= 14; h++) {
      expect(
        find.text('${h.toString().padLeft(2, '0')}:00'),
        findsOneWidget,
        reason: '$h:00',
      );
    }
    // None over another.
    final labels = [
      for (var h = 8; h <= 14; h++)
        tester.getRect(find.text('${h.toString().padLeft(2, '0')}:00')),
    ];
    for (var k = 1; k < labels.length; k++) {
      expect(labels[k].left, greaterThan(labels[k - 1].right));
    }
    expect(labels.last.right, lessThanOrEqualTo(_phone));
  });
}
