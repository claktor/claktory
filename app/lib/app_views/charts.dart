import 'dart:math' as math;

import 'package:fl_chart/fl_chart.dart';
import 'package:flutter/widgets.dart';

import '../identity.dart';
import '../mobile/metrics.dart';
import '../scope.dart';

/// The chart components of catalog v1: Chart (a time series, bars, stacked
/// bars, a pie or a scatter), Stat (one number, with a spark line) and
/// Table, drawn with fl_chart in IDENTITY's look. An app picks colors by
/// role (the agent palette, an agent's own, signal, muted), never by
/// value, and its data is bounded here however much it sends: past the
/// bounds a caption says what's left out.

/// At most this many series in a chart.
const maxChartSeries = 12;

/// At most this many points in each series.
const maxChartPoints = 2000;

/// At most this many bars (categories) in a bar chart.
const maxChartBars = 100;

/// At most this many slices in a pie; the smallest past it are one, Other.
const maxPieSlices = 12;

/// At most this many rows in a Table.
const maxTableRows = 200;

/// At most this many points in a Stat's spark line.
const maxSparkPoints = 200;

/// A Table value: a data query's answer as it comes, `{"columns": [...],
/// "rows": [[...]], "more": bool}`. Null when [value] isn't one (a bound
/// value isn't checked by the backend): a column is a name, or an object
/// with one; every row as long as the columns.
class AppTable {
  const AppTable(this.columns, this.rows, {this.more = false});

  final List<String> columns;
  final List<List<Object?>> rows;

  /// The query had more rows than it answered with.
  final bool more;

  static AppTable? from(Object? value) {
    if (value is! Map) return null;
    final columns = value['columns'];
    final rows = value['rows'];
    if (columns is! List || columns.isEmpty || rows is! List) return null;
    final names = <String>[
      for (final column in columns)
        if (column is String)
          column
        else if (column is Map && column['name'] is String)
          column['name']! as String,
    ];
    if (names.length != columns.length) return null;
    final out = <List<Object?>>[];
    for (final row in rows) {
      if (row is! List || row.length != names.length) return null;
      out.add(row);
    }
    return AppTable(names, out, more: value['more'] == true);
  }

  /// Whether a column holds numbers (and nothing but numbers or nulls).
  bool numeric(int column) {
    var any = false;
    for (final row in rows) {
      final cell = row[column];
      if (cell == null) continue;
      if (cell is! num) return false;
      any = true;
    }
    return any;
  }
}

/// A cell as a number: a number, or a string that reads as one.
double? cellNumber(Object? cell) => switch (cell) {
  final num n when n.isFinite => n.toDouble(),
  final String s => double.tryParse(s.trim()),
  _ => null,
};

/// A cell as a time: milliseconds since 1970 (seconds when it's too small
/// to be milliseconds, as SQLite's unixepoch() gives), or an ISO date or
/// date and time ("2026-11-22", "2026-11-22 14:00:00").
DateTime? cellTime(Object? cell) {
  switch (cell) {
    case final num n when n.isFinite:
      final ms = n.abs() < 1e11 ? n * 1000 : n;
      return DateTime.fromMillisecondsSinceEpoch(ms.round());
    case final String s:
      return DateTime.tryParse(s.trim());
  }
  return null;
}

/// A cell as text: as it is, numbers written out short of float noise.
String cellText(Object? cell) => switch (cell) {
  null => '',
  final String s => s,
  final num n => fullNumber(n.toDouble()),
  _ => '$cell',
};

/// [value] for an axis: "980", "12K", "1.2M", "0.25".
String shortNumber(double value) {
  final sign = value < 0 ? '−' : '';
  final size = value.abs();
  String short(double scaled, String unit) {
    if (scaled >= 100) return '$sign${scaled.round()}$unit';
    final tenths = (scaled * 10).round() / 10;
    return tenths == tenths.roundToDouble()
        ? '$sign${tenths.round()}$unit'
        : '$sign$tenths$unit';
  }

  if (size >= 999500000) return short(size / 1e9, 'B');
  if (size >= 999500) return short(size / 1e6, 'M');
  if (size >= 1000) return short(size / 1e3, 'K');
  return '$sign${_trimmed(size)}';
}

/// [value] in full, thousands grouped: "12,345", "0.25", "−3".
String fullNumber(double value) {
  final sign = value < 0 ? '−' : '';
  final size = value.abs();
  if (size >= 1e15) return '$sign${size.toStringAsExponential(2)}';
  final text = _trimmed(size);
  final dot = text.indexOf('.');
  final whole = dot < 0 ? text : text.substring(0, dot);
  final out = StringBuffer(sign);
  for (var i = 0; i < whole.length; i++) {
    if (i > 0 && (whole.length - i) % 3 == 0) out.write(',');
    out.write(whole[i]);
  }
  if (dot >= 0) out.write(text.substring(dot));
  return out.toString();
}

/// Up to two decimals, none when it's whole.
String _trimmed(double size) {
  if (size == size.roundToDouble()) return '${size.round()}';
  var text = size.toStringAsFixed(size < 1 ? 3 : 2);
  while (text.endsWith('0')) {
    text = text.substring(0, text.length - 1);
  }
  return text.endsWith('.') ? text.substring(0, text.length - 1) : text;
}

const _months = [
  'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
  'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
];

String _clock(DateTime time) =>
    '${time.hour.toString().padLeft(2, '0')}:'
    '${time.minute.toString().padLeft(2, '0')}';

/// A time on an axis spanning [span]: "14:00" within a day and a half,
/// "22 Nov" past it.
String axisTime(DateTime time, Duration span) =>
    span <= const Duration(hours: 36)
    ? _clock(time)
    : '${time.day} ${_months[time.month - 1]}';

/// The step between a time axis's labels for [span]: a round one (5
/// minutes, an hour, a day, a week…) giving at most about five labels.
Duration timeStep(Duration span) {
  const steps = [
    Duration(minutes: 1),
    Duration(minutes: 5),
    Duration(minutes: 15),
    Duration(minutes: 30),
    Duration(hours: 1),
    Duration(hours: 3),
    Duration(hours: 6),
    Duration(hours: 12),
    Duration(days: 1),
    Duration(days: 2),
    Duration(days: 7),
    Duration(days: 14),
    Duration(days: 30),
    Duration(days: 91),
    Duration(days: 365),
  ];
  for (final step in steps) {
    if (span.inMilliseconds / step.inMilliseconds <= 5) return step;
  }
  return Duration(milliseconds: (span.inMilliseconds / 5).ceil());
}

/// A time in a readout: "22 Nov", or "22 Nov 14:00" when it has a time of
/// day.
String readoutTime(DateTime time) {
  final day = '${time.day} ${_months[time.month - 1]}';
  return time.hour == 0 && time.minute == 0 ? day : '$day ${_clock(time)}';
}

/// The agent palette's colors in order: series take them one by one.
const _paletteOrder = [
  'blue', 'violet', 'green', 'pink', 'teal', 'amber', 'red', 'slate', //
];

/// The [index]th series' color from the agent palette.
Color paletteColor(int index) =>
    agentColor(_paletteOrder[index % _paletteOrder.length]);

/// The color of the agent with [id] in this Claktory, or null when there's
/// no such agent here.
Color? agentIdColor(BuildContext context, String id) {
  final scope = context.getInheritedWidgetOfExactType<AppScope>();
  final agent = scope?.backend.workspace.agent(id);
  return agent == null ? null : agentColor(agent.color);
}

/// A role an app names as a color: "signal", "muted", "agent:`id`"; null
/// for one this app doesn't know.
Color? roleColor(BuildContext context, String role) {
  final palette = Palette.of(context);
  if (role == 'signal') return palette.signal;
  if (role == 'muted') return palette.textMuted;
  if (role.startsWith('agent:')) {
    return agentIdColor(context, role.substring('agent:'.length));
  }
  return null;
}

/// A step for [range] split into about [ticks]: 1, 2 or 5 times a power of
/// ten.
double niceStep(double range, {int ticks = 4}) {
  if (range <= 0 || !range.isFinite) return 1;
  final raw = range / ticks;
  final power = math.pow(10, (math.log(raw) / math.ln10).floor()).toDouble();
  final fraction = raw / power;
  final nice = fraction <= 1
      ? 1
      : fraction <= 2
      ? 2
      : fraction <= 5
      ? 5
      : 10;
  return nice * power;
}

/// One series of a chart: its name and its points, (x as the table has
/// it, y).
class ChartSeries {
  ChartSeries(this.name);

  final String name;
  final points = <(Object?, double)>[];
}

/// A chart's series out of its table: wide ([y] names a column per series,
/// every other numeric column without it) or long ([seriesBy] names the
/// column whose values make the series). Null when a named column isn't
/// there. Bounded to [maxChartSeries] series of [maxChartPoints] points;
/// what's left out is counted.
({List<ChartSeries> series, int x, int seriesLeft, bool pointsLeft})?
chartSeries(
  AppTable table, {
  String? x,
  List<String> y = const [],
  String? seriesBy,
}) {
  final xi = x == null ? 0 : table.columns.indexOf(x);
  if (xi < 0) return null;
  final series = <String, ChartSeries>{};
  final beyond = <String>{};
  var pointsLeft = false;
  void add(String name, Object? at, Object? cell) {
    final value = cellNumber(cell);
    if (value == null) return;
    var one = series[name];
    if (one == null) {
      if (series.length >= maxChartSeries) {
        beyond.add(name);
        return;
      }
      one = series[name] = ChartSeries(name);
    }
    if (one.points.length >= maxChartPoints) {
      pointsLeft = true;
      return;
    }
    one.points.add((at, value));
  }

  if (seriesBy != null) {
    final si = table.columns.indexOf(seriesBy);
    if (si < 0) return null;
    var yi = y.isEmpty ? -1 : table.columns.indexOf(y.first);
    if (y.isNotEmpty && yi < 0) return null;
    if (yi < 0) {
      for (var i = 0; i < table.columns.length; i++) {
        if (i != xi && i != si && table.numeric(i)) {
          yi = i;
          break;
        }
      }
    }
    if (yi < 0) {
      return (series: const [], x: xi, seriesLeft: 0, pointsLeft: false);
    }
    for (final row in table.rows) {
      add(cellText(row[si]), row[xi], row[yi]);
    }
  } else {
    final ys = <int>[
      if (y.isEmpty)
        for (var i = 0; i < table.columns.length; i++)
          if (i != xi && table.numeric(i)) i,
      for (final name in y) table.columns.indexOf(name),
    ];
    if (ys.contains(-1)) return null;
    for (final row in table.rows) {
      for (final yi in ys) {
        add(table.columns[yi], row[xi], row[yi]);
      }
    }
  }
  return (
    series: series.values.toList(),
    x: xi,
    seriesLeft: beyond.length,
    pointsLeft: pointsLeft,
  );
}

/// A Chart component: [kind] timeseries, bars, stackedBars, pie or
/// scatter, of [table]'s columns. Over it a line with its [label] that
/// reads out what the pointer (or a finger) is on; under it a legend when
/// there's more than one series, and what was left out.
class AppChart extends StatefulWidget {
  const AppChart({
    super.key,
    required this.kind,
    required this.table,
    this.x,
    this.y = const [],
    this.seriesBy,
    this.color = 'palette',
    this.label = '',
    this.unit = '',
    this.height = 200,
  });

  final String kind;

  /// Null when the data isn't a table.
  final AppTable? table;
  final String? x;
  final List<String> y;
  final String? seriesBy;

  /// palette, agent, signal or muted.
  final String color;
  final String label;
  final String unit;
  final double height;

  @override
  State<AppChart> createState() => _AppChartState();
}

class _AppChartState extends State<AppChart> {
  /// What the pointer is on, in words; null when it's on nothing.
  String? _readout;

  /// The bar group or pie slice the pointer is on: the others dim.
  int? _touched;

  void _point(FlTouchEvent event, String? readout, int? touched) {
    final on = event.isInterestedForInteractions ? readout : null;
    final at = event.isInterestedForInteractions ? touched : null;
    if (on == _readout && at == _touched) return;
    setState(() {
      _readout = on;
      _touched = at;
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final caption = type.caption.copyWith(color: palette.textFaint);
    Widget note(String text) => Text(text, style: caption);
    final table = widget.table;
    final built = table == null
        ? null
        : chartSeries(
            table,
            x: widget.x,
            y: widget.y,
            seriesBy: widget.seriesBy,
          );
    if (table == null || built == null) {
      return note("Can't show this chart.");
    }
    final series = built.series;
    final colors = [
      for (var i = 0; i < series.length; i++)
        _colorOf(context, series[i].name, i),
    ];
    final drawn = series.isEmpty
        ? null
        : switch (widget.kind) {
            'timeseries' => _timeseries(context, series, colors),
            'bars' => _bars(context, series, colors, stacked: false),
            'stackedBars' => _bars(context, series, colors, stacked: true),
            'pie' => _pie(context, series.first),
            'scatter' => _scatter(context, series, colors),
            _ => null,
          };
    if (drawn == null) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        spacing: Space.xs,
        children: [
          if (widget.label.isNotEmpty) _heading(context, widget.label),
          note('Nothing to show.'),
        ],
      );
    }
    final left = [
      if (built.seriesLeft > 0)
        'Showing $maxChartSeries of ${series.length + built.seriesLeft} series.',
      if (built.pointsLeft)
        'Showing the first ${fullNumber(maxChartPoints.toDouble())} points of each series.',
      ...drawn.left,
    ];
    return Semantics(
      container: true,
      label: widget.label.isEmpty ? null : widget.label,
      value: drawn.summary,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        spacing: Space.xs,
        children: [
          // The label, or what the pointer is on: one line, so nothing
          // moves as it changes.
          ExcludeSemantics(
            child: _heading(
              context,
              _readout ?? widget.label,
              readout: _readout != null,
            ),
          ),
          ExcludeSemantics(
            child: SizedBox(height: widget.height, child: drawn.chart),
          ),
          if (widget.kind != 'pie' && series.length > 1)
            ExcludeSemantics(
              child: _Legend(
                entries: [
                  for (var i = 0; i < series.length; i++)
                    (_seriesName(context, series[i].name), colors[i]),
                ],
              ),
            ),
          for (final line in left) note(line),
        ],
      ),
    );
  }

  Widget _heading(BuildContext context, String text, {bool readout = false}) {
    final palette = Palette.of(context);
    return Text(
      text,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: TypeScale.of(context).caption.copyWith(
        color: readout ? palette.textMuted : palette.textFaint,
        fontWeight: readout ? null : FontWeight.w500,
        fontFeatures: const [FontFeature.tabularFigures()],
      ),
    );
  }

  /// A series' color: by the chart's color role, an agent's own when its
  /// series are agents.
  Color _colorOf(BuildContext context, String name, int index) =>
      switch (widget.color) {
        'agent' => agentIdColor(context, name) ?? paletteColor(index),
        'signal' => Palette.of(context).signal,
        'muted' => Palette.of(context).textMuted,
        _ => paletteColor(index),
      };

  /// A series' name as the person reads it: an agent's name when the
  /// series are agents.
  String _seriesName(BuildContext context, String name) {
    if (widget.color != 'agent') return name;
    final scope = context.getInheritedWidgetOfExactType<AppScope>();
    return scope?.backend.workspace.agent(name)?.name ?? name;
  }

  String _value(double value) => widget.unit.isEmpty
      ? fullNumber(value)
      : '${fullNumber(value)} ${widget.unit}';

  TextStyle _axis(BuildContext context) =>
      TypeScale.of(context).caption.copyWith(
        color: Palette.of(context).textFaint,
        fontSize: 11,
        fontFeatures: const [FontFeature.tabularFigures()],
      );

  FlGridData _grid(Palette palette, double step) => FlGridData(
    drawVerticalLine: false,
    horizontalInterval: step,
    getDrawingHorizontalLine: (_) =>
        FlLine(color: palette.hairline, strokeWidth: 1),
  );

  FlBorderData _border(Palette palette) => FlBorderData(
    show: true,
    border: Border(bottom: BorderSide(color: palette.hairline)),
  );

  AxisTitles _valueAxis(BuildContext context, double step) => AxisTitles(
    sideTitles: SideTitles(
      showTitles: true,
      reservedSize: 40,
      interval: step,
      getTitlesWidget: (value, meta) => SideTitleWidget(
        meta: meta,
        child: Text(shortNumber(value), style: _axis(context)),
      ),
    ),
  );

  static const _none = AxisTitles();

  /// The value axis's bounds: from 0 (or below, for negatives) to a step
  /// past the largest.
  ({double min, double max, double step}) _range(Iterable<double> values) {
    var low = 0.0;
    var high = 0.0;
    for (final value in values) {
      low = math.min(low, value);
      high = math.max(high, value);
    }
    if (low == high) high = low + 1;
    final step = niceStep(high - low);
    return (
      min: low < 0 ? (low / step).floor() * step : 0,
      max: (high / step).ceil() * step,
      step: step,
    );
  }

  _Drawn? _timeseries(
    BuildContext context,
    List<ChartSeries> series,
    List<Color> colors,
  ) {
    final palette = Palette.of(context);
    final lines = <List<FlSpot>>[];
    var first = double.infinity;
    var last = double.negativeInfinity;
    for (final one in series) {
      final spots = <FlSpot>[
        for (final (at, value) in one.points)
          if (cellTime(at) case final time?)
            FlSpot(time.millisecondsSinceEpoch.toDouble(), value),
      ]..sort((a, b) => a.x.compareTo(b.x));
      for (final spot in spots) {
        first = math.min(first, spot.x);
        last = math.max(last, spot.x);
      }
      lines.add(spots);
    }
    if (first > last) return null;
    if (first == last) {
      first -= 3600000;
      last += 3600000;
    }
    final span = Duration(milliseconds: (last - first).round());
    final step = timeStep(span).inMilliseconds.toDouble();
    final range = _range([
      for (final spots in lines)
        for (final spot in spots) spot.y,
    ]);
    String timeOf(double x) =>
        readoutTime(DateTime.fromMillisecondsSinceEpoch(x.round()));
    final chart = LineChart(
      duration: Motion.base,
      LineChartData(
        minX: first,
        maxX: last,
        minY: range.min,
        maxY: range.max,
        gridData: _grid(palette, range.step),
        borderData: _border(palette),
        titlesData: FlTitlesData(
          topTitles: _none,
          rightTitles: _none,
          leftTitles: _valueAxis(context, range.step),
          bottomTitles: AxisTitles(
            sideTitles: SideTitles(
              showTitles: true,
              reservedSize: 22,
              interval: step,
              getTitlesWidget: (value, meta) {
                // fl_chart puts labels on the step's multiples, and adds
                // both ends: those only when they fall on one too.
                final off = value % step;
                final onStep = off < step * 0.01 || off > step * 0.99;
                if ((value == meta.min || value == meta.max) && !onStep) {
                  return const SizedBox.shrink();
                }
                return SideTitleWidget(
                  meta: meta,
                  fitInside: SideTitleFitInsideData.fromTitleMeta(meta),
                  child: Text(
                    axisTime(
                      DateTime.fromMillisecondsSinceEpoch(value.round()),
                      span,
                    ),
                    style: _axis(context),
                  ),
                );
              },
            ),
          ),
        ),
        lineBarsData: [
          for (var i = 0; i < lines.length; i++)
            LineChartBarData(
              spots: lines[i],
              color: colors[i],
              barWidth: 1.5,
              isStrokeCapRound: true,
              dotData: FlDotData(show: lines[i].length == 1),
              belowBarData: BarAreaData(
                show: lines.length == 1,
                color: colors[i].withValues(alpha: 0.08),
              ),
            ),
        ],
        lineTouchData: LineTouchData(
          touchSpotThreshold: double.infinity,
          touchTooltipData: LineTouchTooltipData(
            getTooltipItems: (spots) => [for (final _ in spots) null],
          ),
          getTouchedSpotIndicator: (bar, indexes) => [
            for (final _ in indexes)
              TouchedSpotIndicatorData(
                FlLine(color: palette.textFaint, strokeWidth: 1),
                FlDotData(
                  getDotPainter: (spot, percent, bar, index) =>
                      FlDotCirclePainter(
                        radius: 3,
                        color: bar.color ?? palette.signal,
                        strokeWidth: 1.5,
                        strokeColor: palette.canvas,
                      ),
                ),
              ),
          ],
          touchCallback: (event, response) {
            final spots = response?.lineBarSpots;
            if (spots == null || spots.isEmpty) {
              return _point(event, null, null);
            }
            final named = series.length > 1;
            final readout = [
              timeOf(spots.first.x),
              for (final spot in spots)
                named
                    ? '${_seriesName(context, series[spot.barIndex].name)} ${_value(spot.y)}'
                    : _value(spot.y),
            ].join(' · ');
            _point(event, readout, null);
          },
        ),
      ),
    );
    final from = readoutTime(
      DateTime.fromMillisecondsSinceEpoch(first.round()),
    );
    final to = readoutTime(DateTime.fromMillisecondsSinceEpoch(last.round()));
    final summary = [
      'Line chart from $from to $to.',
      for (var i = 0; i < lines.length; i++)
        if (lines[i].isNotEmpty)
          '${_seriesName(context, series[i].name)}: latest ${_value(lines[i].last.y)}.',
    ].join(' ');
    return _Drawn(chart, summary);
  }

  _Drawn? _bars(
    BuildContext context,
    List<ChartSeries> series,
    List<Color> colors, {
    required bool stacked,
  }) {
    final palette = Palette.of(context);
    // The categories in the order they first come, each series' value in
    // each (summed, should one come twice).
    final categories = <String>[];
    final index = <String, int>{};
    var barsLeft = 0;
    final values = [for (final _ in series) <int, double>{}];
    for (var s = 0; s < series.length; s++) {
      for (final (at, value) in series[s].points) {
        final name = cellText(at);
        var i = index[name];
        if (i == null) {
          if (categories.length >= maxChartBars) {
            barsLeft++;
            index[name] = -1;
            continue;
          }
          i = index[name] = categories.length;
          categories.add(name);
        }
        if (i < 0) continue;
        values[s][i] = (values[s][i] ?? 0) + value;
      }
    }
    if (categories.isEmpty) return null;
    double total(int i, {bool positive = true}) =>
        [for (final one in values) one[i] ?? 0]
            .where((v) => positive ? v > 0 : v < 0)
            .fold(0, (sum, v) => sum + v);
    final range = _range([
      for (var i = 0; i < categories.length; i++)
        if (stacked) ...[
          total(i),
          total(i, positive: false),
        ] else
          for (final one in values) one[i] ?? 0,
    ]);
    final touched = _touched;
    Color dim(Color color, int group) => touched == null || touched == group
        ? color
        : color.withValues(alpha: 0.45);
    const radius = BorderRadius.vertical(top: Radius.circular(1.5));
    return _Drawn(
      LayoutBuilder(
        builder: (context, constraints) {
          final room =
              math.max(1.0, constraints.maxWidth - 40) / categories.length;
          final rods = stacked ? 1 : series.length;
          final width = (room * 0.7 / rods).clamp(2.0, 24.0);
          // Labels where there's room for them: every one, or every few.
          final every = math.max(1, (56 / room).ceil());
          return BarChart(
            duration: Motion.base,
            BarChartData(
              minY: range.min,
              maxY: range.max,
              alignment: BarChartAlignment.spaceAround,
              gridData: _grid(palette, range.step),
              borderData: _border(palette),
              titlesData: FlTitlesData(
                topTitles: _none,
                rightTitles: _none,
                leftTitles: _valueAxis(context, range.step),
                bottomTitles: AxisTitles(
                  sideTitles: SideTitles(
                    showTitles: true,
                    reservedSize: 22,
                    getTitlesWidget: (value, meta) {
                      final i = value.round();
                      if (i < 0 || i >= categories.length || i % every != 0) {
                        return const SizedBox.shrink();
                      }
                      return SideTitleWidget(
                        meta: meta,
                        child: ConstrainedBox(
                          constraints: BoxConstraints(maxWidth: room * every),
                          child: Text(
                            _category(categories[i]),
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: _axis(context),
                          ),
                        ),
                      );
                    },
                  ),
                ),
              ),
              barGroups: [
                for (var i = 0; i < categories.length; i++)
                  BarChartGroupData(
                    x: i,
                    barsSpace: 1,
                    barRods: stacked
                        ? [
                            BarChartRodData(
                              fromY: total(i, positive: false),
                              toY: total(i),
                              width: width,
                              borderRadius: radius,
                              color: dim(colors.first, i),
                              rodStackItems: _stack(values, i, colors, dim),
                              backDrawRodData: BackgroundBarChartRodData(
                                show: true,
                                fromY: range.min,
                                toY: range.max,
                                color: const Color(0x00000000),
                              ),
                            ),
                          ]
                        : [
                            for (var s = 0; s < series.length; s++)
                              BarChartRodData(
                                toY: values[s][i] ?? 0,
                                width: width,
                                borderRadius: radius,
                                color: dim(colors[s], i),
                                backDrawRodData: BackgroundBarChartRodData(
                                  show: true,
                                  fromY: range.min,
                                  toY: range.max,
                                  color: const Color(0x00000000),
                                ),
                              ),
                          ],
                  ),
              ],
              barTouchData: BarTouchData(
                allowTouchBarBackDraw: true,
                touchTooltipData: BarTouchTooltipData(
                  getTooltipItem: (group, groupIndex, rod, rodIndex) => null,
                ),
                touchCallback: (event, response) {
                  final i = response?.spot?.touchedBarGroupIndex;
                  if (i == null || i < 0 || i >= categories.length) {
                    return _point(event, null, null);
                  }
                  final named = series.length > 1;
                  final readout = [
                    _category(categories[i]),
                    for (var s = 0; s < series.length; s++)
                      if (values[s][i] case final value?)
                        named
                            ? '${_seriesName(context, series[s].name)} ${_value(value)}'
                            : _value(value),
                  ].join(' · ');
                  _point(event, readout, i);
                },
              ),
            ),
          );
        },
      ),
      [
        stacked ? 'Stacked bar chart.' : 'Bar chart.',
        for (var i = 0; i < math.min(categories.length, 20); i++)
          '${_category(categories[i])}: ${[for (var s = 0; s < series.length; s++)
            if (values[s][i] case final value?) series.length > 1 ? '${_seriesName(context, series[s].name)} ${_value(value)}' : _value(value)].join(', ')}.',
        if (categories.length > 20) 'And ${categories.length - 20} more.',
      ].join(' '),
      left: [
        if (barsLeft > 0)
          'Showing the first $maxChartBars of ${categories.length + barsLeft} bars.',
      ],
    );
  }

  /// A category's words: a time when it reads as one ("22 Nov").
  String _category(String name) {
    if (name.length >= 10 && RegExp(r'^\d{4}-\d{2}-\d{2}').hasMatch(name)) {
      final time = DateTime.tryParse(name);
      if (time != null) return readoutTime(time);
    }
    return name;
  }

  List<BarChartRodStackItem> _stack(
    List<Map<int, double>> values,
    int i,
    List<Color> colors,
    Color Function(Color, int) dim,
  ) {
    final items = <BarChartRodStackItem>[];
    var up = 0.0;
    var down = 0.0;
    for (var s = 0; s < values.length; s++) {
      final value = values[s][i] ?? 0;
      if (value >= 0) {
        items.add(BarChartRodStackItem(up, up + value, dim(colors[s], i)));
        up += value;
      } else {
        items.add(BarChartRodStackItem(down + value, down, dim(colors[s], i)));
        down += value;
      }
    }
    return items;
  }

  _Drawn? _pie(BuildContext context, ChartSeries series) {
    final palette = Palette.of(context);
    // Slices by name, largest first; past the bound the rest are Other.
    final sums = <String, double>{};
    for (final (at, value) in series.points) {
      if (value <= 0) continue;
      final name = cellText(at);
      sums[name] = (sums[name] ?? 0) + value;
    }
    if (sums.isEmpty) return null;
    final sorted = sums.entries.toList()
      ..sort((a, b) => b.value.compareTo(a.value));
    final slices = <(String, double)>[
      for (final entry in sorted.take(
        sorted.length > maxPieSlices ? maxPieSlices - 1 : maxPieSlices,
      ))
        (entry.key, entry.value),
      if (sorted.length > maxPieSlices)
        (
          'Other',
          sorted
              .skip(maxPieSlices - 1)
              .fold(0.0, (sum, entry) => sum + entry.value),
        ),
    ];
    final whole = slices.fold(0.0, (sum, slice) => sum + slice.$2);
    String share(double value) {
      final percent = value / whole * 100;
      return percent < 1 ? '<1%' : '${percent.round()}%';
    }

    final colors = [
      for (var i = 0; i < slices.length; i++)
        slices[i].$1 == 'Other' && sorted.length > maxPieSlices
            ? palette.textFaint
            : _colorOf(context, slices[i].$1, i),
    ];
    final touched = _touched;
    final size = widget.height;
    final chart = Row(
      spacing: Space.l,
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        SizedBox.square(
          dimension: size,
          child: PieChart(
            duration: Motion.base,
            PieChartData(
              startDegreeOffset: -90,
              sectionsSpace: 1,
              centerSpaceRadius: size / 2 * 0.58,
              sections: [
                for (var i = 0; i < slices.length; i++)
                  PieChartSectionData(
                    value: slices[i].$2,
                    color: touched == null || touched == i
                        ? colors[i]
                        : colors[i].withValues(alpha: 0.45),
                    radius: size / 2 * 0.42,
                    showTitle: false,
                  ),
              ],
              pieTouchData: PieTouchData(
                touchCallback: (event, response) {
                  final i = response?.touchedSection?.touchedSectionIndex;
                  if (i == null || i < 0 || i >= slices.length) {
                    return _point(event, null, null);
                  }
                  final (name, value) = slices[i];
                  _point(
                    event,
                    '${_seriesName(context, name)} · ${_value(value)} · ${share(value)}',
                    i,
                  );
                },
              ),
            ),
          ),
        ),
        Flexible(
          child: SingleChildScrollView(
            child: _Legend(
              vertical: true,
              entries: [
                for (var i = 0; i < slices.length; i++)
                  (
                    '${_seriesName(context, slices[i].$1)} · ${share(slices[i].$2)}',
                    colors[i],
                  ),
              ],
            ),
          ),
        ),
      ],
    );
    return _Drawn(
      chart,
      [
        'Pie chart.',
        for (final (name, value) in slices)
          '${_seriesName(context, name)}: ${_value(value)}, ${share(value)}.',
      ].join(' '),
      left: [
        if (sorted.length > maxPieSlices)
          'The smallest ${sorted.length - maxPieSlices + 1} are under Other.',
      ],
    );
  }

  _Drawn? _scatter(
    BuildContext context,
    List<ChartSeries> series,
    List<Color> colors,
  ) {
    final palette = Palette.of(context);
    final spots = <ScatterSpot>[];
    final whose = <int>[];
    for (var s = 0; s < series.length; s++) {
      for (final (at, value) in series[s].points) {
        final x = cellNumber(at);
        if (x == null) continue;
        spots.add(
          ScatterSpot(
            x,
            value,
            dotPainter: FlDotCirclePainter(
              radius: 3,
              color: colors[s],
              strokeWidth: 0,
            ),
          ),
        );
        whose.add(s);
      }
    }
    if (spots.isEmpty) return null;
    final ys = _range([for (final spot in spots) spot.y]);
    final xs = _range([for (final spot in spots) spot.x]);
    final chart = ScatterChart(
      duration: Motion.base,
      ScatterChartData(
        scatterSpots: spots,
        minX: xs.min,
        maxX: xs.max,
        minY: ys.min,
        maxY: ys.max,
        gridData: _grid(palette, ys.step),
        borderData: _border(palette),
        titlesData: FlTitlesData(
          topTitles: _none,
          rightTitles: _none,
          leftTitles: _valueAxis(context, ys.step),
          bottomTitles: AxisTitles(
            sideTitles: SideTitles(
              showTitles: true,
              reservedSize: 22,
              interval: xs.step,
              getTitlesWidget: (value, meta) => SideTitleWidget(
                meta: meta,
                child: Text(shortNumber(value), style: _axis(context)),
              ),
            ),
          ),
        ),
        scatterTouchData: ScatterTouchData(
          touchTooltipData: ScatterTouchTooltipData(
            getTooltipItems: (_) => null,
          ),
          touchCallback: (event, response) {
            final i = response?.touchedSpot?.spotIndex;
            if (i == null || i < 0 || i >= spots.length) {
              return _point(event, null, null);
            }
            final spot = spots[i];
            final name = series.length > 1
                ? '${_seriesName(context, series[whose[i]].name)} · '
                : '';
            _point(
              event,
              '$name${fullNumber(spot.x)}, ${_value(spot.y)}',
              null,
            );
          },
        ),
      ),
    );
    return _Drawn(
      chart,
      'Scatter chart, ${spots.length} points'
      '${series.length > 1 ? ' in ${series.length} series' : ''}.',
    );
  }
}

/// A chart as drawn, what it says to assistive technology, and what was
/// left out of it.
class _Drawn {
  const _Drawn(this.chart, this.summary, {this.left = const []});

  final Widget chart;
  final String summary;
  final List<String> left;
}

/// A legend: each series' 8px dot in its color and its name in caption
/// `textMuted`, in a wrapping row (or a column, beside a pie).
class _Legend extends StatelessWidget {
  const _Legend({required this.entries, this.vertical = false});

  final List<(String, Color)> entries;
  final bool vertical;

  @override
  Widget build(BuildContext context) {
    final style = TypeScale.of(context).caption.copyWith(
      color: Palette.of(context).textMuted,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final items = [
      for (final (name, color) in entries)
        Row(
          mainAxisSize: MainAxisSize.min,
          spacing: Space.xs + 2,
          children: [
            Container(
              width: 8,
              height: 8,
              decoration: BoxDecoration(color: color, shape: BoxShape.circle),
            ),
            Flexible(
              child: Text(
                name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: style,
              ),
            ),
          ],
        ),
    ];
    return vertical
        ? Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            spacing: Space.xs,
            children: items,
          )
        : Wrap(spacing: Space.m, runSpacing: Space.xs, children: items);
  }
}

/// A Stat component: its [label] in caption `textMuted`, the [value] in
/// title `text` with its [unit] in body `textMuted` (the Usage total's
/// look), and under them a 32px spark line of [spark] in [color].
class AppStat extends StatelessWidget {
  const AppStat({
    super.key,
    required this.label,
    required this.value,
    this.unit = '',
    this.spark = const [],
    this.color,
  });

  final String label;

  /// A number, or words as they are.
  final Object? value;
  final String unit;
  final List<double> spark;

  /// The spark line's; `textMuted` without one.
  final Color? color;

  /// The value as it reads: a number grouped ("12,345") up to a million,
  /// short past it ("1.2M").
  static String words(Object? value) => switch (value) {
    final num n when n.isFinite && n.abs() >= 1e6 => shortNumber(n.toDouble()),
    final num n when n.isFinite => fullNumber(n.toDouble()),
    final String s => s,
    null => '',
    _ => '$value',
  };

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final shown = words(value);
    final points = spark.take(maxSparkPoints).toList();
    final line = color ?? palette.textMuted;
    var low = double.infinity;
    var high = double.negativeInfinity;
    for (final point in points) {
      low = math.min(low, point);
      high = math.max(high, point);
    }
    if (low == high) {
      low -= 1;
      high += 1;
    }
    return Semantics(
      container: true,
      label: label,
      value: unit.isEmpty ? shown : '$shown $unit',
      child: ExcludeSemantics(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          spacing: Space.xs,
          children: [
            if (label.isNotEmpty)
              Text(
                label,
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: shown,
                    style: type.title.copyWith(
                      color: palette.text,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                  if (unit.isNotEmpty)
                    TextSpan(
                      text: ' $unit',
                      style: type.body.copyWith(color: palette.textMuted),
                    ),
                ],
              ),
            ),
            if (points.length >= 2)
              SizedBox(
                height: 32,
                child: LineChart(
                  duration: Motion.base,
                  LineChartData(
                    minX: 0,
                    maxX: points.length - 1,
                    minY: low,
                    maxY: high,
                    gridData: const FlGridData(show: false),
                    borderData: FlBorderData(show: false),
                    titlesData: const FlTitlesData(show: false),
                    lineTouchData: const LineTouchData(enabled: false),
                    lineBarsData: [
                      LineChartBarData(
                        spots: [
                          for (var i = 0; i < points.length; i++)
                            FlSpot(i.toDouble(), points[i]),
                        ],
                        color: line,
                        barWidth: 1.5,
                        isStrokeCapRound: true,
                        dotData: const FlDotData(show: false),
                        belowBarData: BarAreaData(
                          show: true,
                          color: line.withValues(alpha: 0.08),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// One of a Table component's columns: which of the table's, its heading,
/// and how its cells read (text, number or time).
typedef AppTableColumn = ({int index, String title, String format});

/// A Table component: a heading row in caption 500 `textFaint`, then a row
/// per row of [table] in body `text` between hairlines, numbers right
/// aligned in tabular figures, times as "22 Nov 14:00"; at most
/// [maxTableRows], then how many more. Scrolls sideways when it's wider
/// than its place.
class AppTableView extends StatelessWidget {
  const AppTableView({super.key, required this.table, this.columns});

  /// Null when the data isn't a table.
  final AppTable? table;

  /// The columns shown, in order; every column, titled by its name, when
  /// null.
  final List<AppTableColumn>? columns;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final table = this.table;
    final caption = type.caption.copyWith(color: palette.textFaint);
    if (table == null) {
      return Text("Can't show this table.", style: caption);
    }
    final shown =
        columns ??
        [
          for (var i = 0; i < table.columns.length; i++)
            (
              index: i,
              title: table.columns[i],
              format: table.numeric(i) ? 'number' : 'text',
            ),
        ];
    if (shown.isEmpty) return const SizedBox.shrink();
    final rows = table.rows.take(maxTableRows).toList();
    final left = table.rows.length - rows.length;
    String cell(Object? value, String format) => switch (format) {
      'time' => switch (cellTime(value)) {
        final time? => readoutTime(time),
        null => cellText(value),
      },
      'number' => switch (cellNumber(value)) {
        final number? => fullNumber(number),
        null => cellText(value),
      },
      _ => cellText(value),
    };
    Widget padded(Widget child, int column, {bool end = false}) => Padding(
      padding: EdgeInsets.fromLTRB(
        column == 0 ? 0 : Space.m,
        Space.xs,
        0,
        Space.xs,
      ),
      child: Align(
        alignment: end ? Alignment.centerRight : Alignment.centerLeft,
        child: child,
      ),
    );
    final tabular = const [FontFeature.tabularFigures()];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      spacing: Space.xs,
      children: [
        SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          child: Table(
            defaultColumnWidth: const IntrinsicColumnWidth(),
            defaultVerticalAlignment: TableCellVerticalAlignment.middle,
            border: TableBorder(
              horizontalInside: BorderSide(color: palette.hairline),
            ),
            children: [
              TableRow(
                children: [
                  for (var c = 0; c < shown.length; c++)
                    Semantics(
                      header: true,
                      child: padded(
                        Text(
                          shown[c].title,
                          style: type.caption.copyWith(
                            color: palette.textFaint,
                            fontWeight: FontWeight.w500,
                          ),
                        ),
                        c,
                        end: shown[c].format == 'number',
                      ),
                    ),
                ],
              ),
              for (final row in rows)
                TableRow(
                  children: [
                    for (var c = 0; c < shown.length; c++)
                      padded(
                        Text(
                          shown[c].index < row.length
                              ? cell(row[shown[c].index], shown[c].format)
                              : '',
                          style: type.body.copyWith(
                            color: palette.text,
                            fontFeatures: shown[c].format == 'text'
                                ? null
                                : tabular,
                          ),
                        ),
                        c,
                        end: shown[c].format == 'number',
                      ),
                  ],
                ),
            ],
          ),
        ),
        if (rows.isEmpty) Text('Nothing to show.', style: caption),
        if (left > 0 || table.more)
          Text(
            left > 0
                ? 'and ${fullNumber(left.toDouble())}${table.more ? '+' : ''} more'
                : 'and more',
            style: caption,
          ),
      ],
    );
  }
}
