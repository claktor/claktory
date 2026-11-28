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
import 'package:factory_app/controls.dart' show ExtensionGlyph, Pressable;
import 'package:factory_app/mobile/device.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' as pb;
import 'package:fl_chart/fl_chart.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/rendering.dart' show RenderParagraph;
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

/// Where [lon], [lat] is on the one map shown: Web Mercator from 76°N
/// to 56°S, as the map draws the world.
Offset _onMap(WidgetTester tester, double lon, double lat) {
  final box = tester.getRect(
    find.descendant(of: find.byType(AppMap), matching: find.byType(ClipRect)),
  );
  double mercator(double lat) =>
      math.log(math.tan(math.pi / 4 + lat * math.pi / 360));
  return Offset(
    box.left + (lon + 180) / 360 * box.width,
    box.top +
        (mercator(76) - mercator(lat)) /
            (mercator(76) - mercator(-56)) *
            box.height,
  );
}

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

    testWidgets('${brightness.name}: a world map, one country tapped', (
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
            'component': 'Map',
            'label': 'Users by country',
            'data': {'@path': '/data'},
            'location': 'country',
            'value': 'users',
          }),
        ],
        {
          'data': {
            'columns': ['country', 'users'],
            'rows': [
              ['US', 412],
              ['GB', 120],
              ['DE', 96],
              ['FR', 70],
              ['IT', 41],
              ['BRA', 55],
              ['IN', 88],
              ['JP', 33],
              ['AU', 18],
              ['ZA', 9],
              ['ca', 47],
              ['Hidden (fewer than 5)', 12],
            ],
          },
        },
        height: 420,
        brightness: brightness,
      );
      await tester.tapAt(_onMap(tester, 10.4, 51.1)); // Germany
      await tester.pump();
      await _golden(tester, 'map');
    });

    testWidgets('${brightness.name}: an hour by weekday heatmap', (
      tester,
    ) async {
      _shade = brightness.name;
      const days = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
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
            'kind': 'heatmap',
            'label': 'Sessions by hour',
            'data': {'@path': '/data'},
            'x': 'hour',
            'seriesBy': 'day',
            'y': ['sessions'],
            'height': 200,
          }),
        ],
        {
          'data': {
            'columns': ['day', 'hour', 'sessions'],
            'rows': [
              for (final (d, day) in days.indexed)
                for (var h = 0; h < 24; h++)
                  if (!(d == 6 && h < 6))
                    [
                      day,
                      h.toString().padLeft(2, '0'),
                      math
                          .max(
                            0,
                            (d < 5 ? 30 : 12) *
                                    math.sin((h - 6) / 15 * math.pi) +
                                (h * 7 + d * 3) % 5,
                          )
                          .round(),
                    ],
            ],
          },
        },
        height: 400,
        brightness: brightness,
      );
      await _golden(tester, 'heatmap');
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

  // A dashboard like Fred's "Right now" (long stat titles,
  // a table with a sentence a row) at a phone's, a narrow iPad's and a
  // Mac's widths, at the default and the largest text.
  for (final (width, scale, brightness, open) in [
    for (final width in [390.0, 560.0, 320.0, 1024.0])
      for (final scale in [1.0, 3.12]) (width, scale, Brightness.dark, false),
    (390.0, 1.0, Brightness.light, false),
    (390.0, 1.0, Brightness.dark, true),
    (560.0, 1.0, Brightness.dark, true),
  ]) {
    final name =
        'fred_${width.toInt()}_x${scale.toString().replaceAll('.', '_')}'
        '${open ? '_open' : ''}';
    testWidgets('${brightness.name}: $name (friend bug #62)', (tester) async {
      _shade = brightness.name;
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      Map<String, Object?> row(String id, List<String> cells) => {
        ..._row(id, const []),
        'children': cells,
        'stackBelow': 768,
      };
      const long =
          'Rebuilding the release images for the Android beta, signing them '
          'with the upload key, uploading them to the Play Console and then '
          'telling Bruno which build number went out.';
      final stats = [
        ('working', 'Agents working right now', 3),
        ('waiting', 'Requests waiting for you', 2),
        ('tokens', 'Tokens used today, all agents', 1400000),
        ('spend', 'Spend this month', 212),
      ];
      await _show(
        tester,
        [
          {
            'id': 'root',
            'component': 'Column',
            'children': ['r0', 'r1'],
          },
          row('r0', [for (final (id, _, _) in stats) id]),
          for (final (id, label, value) in stats)
            ..._weighted(
              _card(id, {
                'component': 'Stat',
                'label': label,
                'value': value,
                'spark': [for (var h = 0; h <= 24; h++) _wave(h, 50, 20, 0)],
                'thresholds': [
                  {'color': 'green'},
                ],
                // As Charts sends a 3 by 3 panel of its 12-column grid.
                'height': 140,
              }),
              3,
            ),
          row('r1', ['agents']),
          {
            'id': 'agents',
            'component': 'Card',
            'child': 'agents_col',
            'weight': 12,
          },
          {
            'id': 'agents_col',
            'component': 'Column',
            'children': ['agents_title', 'agents_table'],
          },
          {
            'id': 'agents_title',
            'component': 'Text',
            'text': 'Agents',
            'variant': 'title',
          },
          {
            'id': 'agents_table',
            'component': 'Table',
            'data': {'@path': '/agents'},
            'columns': [
              {'name': 'agent', 'title': 'Agent'},
              {'name': 'status', 'title': 'Status'},
              {'name': 'step', 'title': 'Last step', 'format': 'time'},
              {'name': 'doing', 'title': "What it's doing"},
            ],
          },
        ],
        {
          'agents': {
            'columns': ['agent', 'status', 'step', 'doing'],
            'rows': [
              ['Pericolo', 'Working', '2026-11-22 14:02:00', long],
              [
                'June',
                'Waiting',
                '2026-11-22 13:40:00',
                'Waiting for your answer on the pricing page before it '
                    'publishes.',
              ],
              ['Bruno', 'Idle', '2026-11-22 12:15:00', 'Handed off.'],
            ],
          },
        },
        width: width,
        height: scale > 1 ? 3600 : 1400,
        brightness: brightness,
      );
      expect(tester.takeException(), isNull);
      if (open) {
        await tester.tap(find.text('Pericolo'));
        await tester.pump();
      }
      await _golden(tester, name);
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

      // The bottom edge's middle: the hairline, not the red.
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

  // A range's start isn't a step, however near one.
  testWidgets('at a 90-day range starting hours before a step, the axis '
      'labels its steps alone, never its start beside the first', (
    tester,
  ) async {
    final from = DateTime(2026, 8, 26, 21, 13);
    final to = from.add(const Duration(days: 90));
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    await _show(
      tester,
      [
        {
          'id': 'root',
          'component': 'Column',
          'children': ['c'],
        },
        {
          'id': 'c',
          'component': 'Chart',
          'kind': 'timeseries',
          'data': {'@path': '/d'},
          'x': 'day',
          'height': 296,
          'timeFrom': from.millisecondsSinceEpoch,
          'timeTo': to.millisecondsSinceEpoch,
        },
      ],
      {
        'd': {
          'columns': ['day', 'tokens'],
          'rows': [
            for (var d = 20; d <= 24; d++) ['2026-11-$d', 1000 * d],
          ],
        },
      },
      width: 560,
      height: 500,
    );
    final labels = [
      for (final text in find.byType(Text).evaluate())
        if (RegExp(r'^\d\d/\d\d$').hasMatch((text.widget as Text).data ?? ''))
          (
            (text.widget as Text).data!,
            tester.getRect(find.byWidget(text.widget)),
          ),
    ]..sort((a, b) => a.$2.left.compareTo(b.$2.left));
    expect(labels.first.$1, '08/27');
    expect(labels.map((label) => label.$1), isNot(contains('08/26')));
    for (var i = 1; i < labels.length; i++) {
      expect(
        labels[i].$2.left,
        greaterThanOrEqualTo(labels[i - 1].$2.right),
        reason: '${labels[i - 1]} and ${labels[i]} overlap',
      );
    }
  });

  // Against Grafana 13.3.1, in Geist as the phone draws it.
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

  // A dashboard's bar on an SE (320 points), as a phone draws it in Geist:
  // at the normal text size on one line, the name giving way alone, the
  // range and variable whole; larger, every button keeps at least a letter
  // and "…", and at the largest the name takes a line of its own above the
  // others, as Grafana's title does. At 375 too, as the SE 3rd generation
  // is: one letter doesn't say where you are, so a line holds only while
  // each keeps three. Letters, not spaces: "30 …" keeps two.
  for (final (width, scale, filters, title, span) in [
    for (final width in [320.0, 375.0])
      for (final scale in [1.0, 1.6, 2.0, 2.35, 3.12])
        (width, scale, 1, 'Claktory usage', '7 days'),
    (320.0, 3.12, 2, 'Claktory usage', '7 days'),
    (320.0, 1.0, 1, 'Agents at work', '30 days'),
    (375.0, 3.12, 1, 'Claktory', '30 days'),
  ]) {
    testWidgets("a dashboard's bar ${width.toInt()} wide at text ×$scale, "
        '"$title", "$span" and $filters variable(s): every button keeps '
        'three letters', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      debugDeviceCapabilities = DeviceCapabilities(idiom: Idiom.phone);
      addTearDown(() {
        debugDefaultTargetPlatformOverride = null;
        debugDeviceCapabilities = null;
      });
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      List<Map<String, Object?>> button(
        String id,
        List<Map<String, Object?>> face, {
        int? weight,
      }) => [
        {
          'id': id,
          'component': 'Button',
          'child': '${id}_c',
          'variant': 'borderless',
          'weight': ?weight,
          'action': {
            'event': {'name': 'menu', 'context': <String, Object?>{}},
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
      List<Map<String, Object?>> menu(String id, String text) => button(id, [
        {'id': '${id}_t', 'component': 'Text', 'text': text},
        {'id': '${id}_i', 'component': 'Icon', 'name': 'chevron-down'},
      ]);
      final variables = [
        if (title == 'Claktory usage') 'Agent: All' else 'Filters (2)',
        'Fruit: Apples',
      ].take(filters);
      await _show(tester, width: width, [
        {
          'id': 'root',
          'component': 'Column',
          'children': ['bar'],
        },
        {
          'id': 'bar',
          'component': 'Row',
          'children': [
            'name',
            'range',
            for (var k = 0; k < filters; k++) 'v$k',
            'more',
          ],
        },
        ...button('name', weight: 1, [
          {
            'id': 'name_t',
            'component': 'Text',
            'text': title,
            'variant': 'heading',
          },
          {'id': 'name_i', 'component': 'Icon', 'name': 'chevron-down'},
        ]),
        ...menu('range', span),
        for (final (k, text) in variables.indexed) ...menu('v$k', text),
        ...button('more', [
          {'id': 'more_i', 'component': 'Icon', 'name': 'ellipsis'},
        ]),
      ], null);
      expect(tester.takeException(), isNull);
      Rect button$(Finder of) => tester.getRect(
        find.ancestor(of: of, matching: find.byType(Pressable)).first,
      );
      final labels = [title, span, ...variables];
      for (final text in labels) {
        final paragraph = tester.renderObject<RenderParagraph>(
          find.descendant(of: find.text(text), matching: find.byType(RichText)),
        );
        // Three letters before its "…", or its words whole.
        final shown = paragraph.didExceedMaxLines
            ? paragraph
                  .getPositionForOffset(
                    Offset(paragraph.size.width, paragraph.size.height / 2),
                  )
                  .offset
            : text.length;
        final letters = text
            .substring(0, shown)
            .replaceAll(RegExp(r'\s'), '')
            .length;
        expect(
          letters,
          greaterThanOrEqualTo(math.min(3, text.replaceAll(' ', '').length)),
          reason: '$text at ×$scale',
        );
        final rect = button$(find.text(text));
        expect(rect.left, greaterThanOrEqualTo(0), reason: text);
        expect(rect.right, lessThanOrEqualTo(width), reason: text);
      }
      final more = button$(
        find.byWidgetPredicate(
          (w) => w is ExtensionGlyph && w.icon == 'ellipsis',
        ),
      );
      expect(more.right, lessThanOrEqualTo(width));
      final name = button$(find.text(title));
      final range = button$(find.text(span));
      if (scale == 1.0 && width == 320 && title == 'Claktory usage') {
        // One line; "Clak…" as before, the others whole.
        expect(range.center.dy, moreOrLessEquals(name.center.dy, epsilon: 1));
        final paragraph = tester.renderObject<RenderParagraph>(
          find.descendant(
            of: find.text(title),
            matching: find.byType(RichText),
          ),
        );
        expect(
          paragraph
              .getPositionForOffset(
                Offset(paragraph.size.width, paragraph.size.height / 2),
              )
              .offset,
          greaterThanOrEqualTo(4),
        );
        for (final text in labels.skip(1)) {
          expect(
            tester
                .renderObject<RenderParagraph>(
                  find.descendant(
                    of: find.text(text),
                    matching: find.byType(RichText),
                  ),
                )
                .didExceedMaxLines,
            isFalse,
            reason: text,
          );
        }
      }
      if (scale > 2) {
        // The name on its own line, the others under it.
        expect(range.top, greaterThanOrEqualTo(name.bottom));
        expect(more.top, greaterThanOrEqualTo(name.bottom));
      }
    });
  }
}
