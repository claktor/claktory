import 'dart:convert' show base64Decode;
import 'dart:math' as math;
import 'dart:typed_data' show Int16List;

import 'package:fl_chart/fl_chart.dart';
import 'package:flutter/foundation.dart' show mapEquals;
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

import '../identity.dart';
import '../mobile/metrics.dart';
import '../scope.dart';
import '../visible_codes.dart';

part 'heatmap.dart';
part 'map.dart';
part 'world.g.dart';

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
          visibleCodes(column)
        else if (column is Map && column['name'] is String)
          visibleCodes(column['name']! as String),
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

/// A cell as text: as it is, numbers written out short of float noise,
/// and characters that would reorder or hide words shown by their codes
/// (a CSV's label can't pass itself off as something else).
String cellText(Object? cell) => switch (cell) {
  null => '',
  final String s => visibleCodes(s),
  final num n => fullNumber(n.toDouble()),
  _ => '$cell',
};

/// The most a chart's axes' words grow with the text size.
const maxAxisTextScale = 1.5;

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

String _two(int n) => n.toString().padLeft(2, '0');

/// A time on an axis spanning [span] with labels [step] apart, as Grafana
/// writes them: "14:00" within a day, "11/22" for days, "11/22 12:00" for
/// hours across days, "14:05:30" under a minute.
String axisTime(DateTime time, Duration span, Duration step) {
  if (step < const Duration(minutes: 1)) {
    return '${_clock(time)}:${_two(time.second)}';
  }
  final day = '${_two(time.month)}/${_two(time.day)}';
  if (step >= const Duration(days: 1)) return day;
  if (span <= const Duration(days: 1)) return _clock(time);
  return '$day ${_clock(time)}';
}

/// A time in a chart's tooltip: "2026-11-22 14:00:00".
String tooltipTime(DateTime time) =>
    '${time.year}-${_two(time.month)}-${_two(time.day)} '
    '${_clock(time)}:${_two(time.second)}';

/// The step between a time axis's labels for [span]: a round one (5
/// minutes, an hour, a day, a week…) giving at most about [most] labels.
Duration timeStep(Duration span, {int most = 5}) {
  const steps = [
    Duration(minutes: 1),
    Duration(minutes: 5),
    Duration(minutes: 15),
    Duration(minutes: 30),
    Duration(hours: 1),
    Duration(hours: 2),
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
    if (span.inMilliseconds / step.inMilliseconds <= most) return step;
  }
  return Duration(milliseconds: (span.inMilliseconds / most).ceil());
}

/// A time in a readout: "22 Nov", or "22 Nov 14:00" when it has a time of
/// day.
String readoutTime(DateTime time) {
  final day = '${time.day} ${_months[time.month - 1]}';
  return time.hour == 0 && time.minute == 0 ? day : '$day ${_clock(time)}';
}

/// [text] as it fits on one line [width] wide in [style]: whole, or its
/// longest start that fits with "…", spaces before the "…" left out
/// ("3–5…", never "3–5 …"); "…" alone when not even a letter fits.
String cutText(
  String text,
  TextStyle style,
  double width,
  TextScaler scaler,
  TextDirection direction,
) {
  final painter = TextPainter(
    textScaler: scaler,
    textDirection: direction,
    maxLines: 1,
  );
  bool fits(String candidate) {
    painter
      ..text = TextSpan(text: candidate, style: style)
      ..layout();
    return painter.width <= width + 0.01;
  }

  try {
    if (fits(text)) return text;
    final letters = text.characters.toList();
    String cut(int n) => '${letters.take(n).join().trimRight()}…';
    var low = 0;
    var high = letters.length;
    while (low < high) {
      final mid = (low + high + 1) ~/ 2;
      if (fits(cut(mid))) {
        low = mid;
      } else {
        high = mid - 1;
      }
    }
    return cut(low);
  } finally {
    painter.dispose();
  }
}

/// The least a pie's legend keeps beside its donut: a few words.
const _leastLegend = 96.0;

/// Charts' own palette (IDENTITY, Color): Grafana's classic palette, each
/// name with its dark-theme and light-theme shade. Series take them in
/// [chartSeriesOrder]; apps name them, never a value.
const chartColors = <String, (Color, Color)>{
  'green': (Color(0xFF73BF69), Color(0xFF56A64B)),
  'semi-dark-yellow': (Color(0xFFF2CC0C), Color(0xFFE0B400)),
  'blue': (Color(0xFF5794F2), Color(0xFF3274D9)),
  'orange': (Color(0xFFFF9830), Color(0xFFFF780A)),
  'red': (Color(0xFFF2495C), Color(0xFFE02F44)),
  'purple': (Color(0xFFB877D9), Color(0xFFA352CC)),
  'yellow': (Color(0xFFFADE2A), Color(0xFFF2CC0C)),
  'dark-green': (Color(0xFF37872D), Color(0xFF19730E)),
  'dark-yellow': (Color(0xFFE0B400), Color(0xFFCC9D00)),
  'dark-blue': (Color(0xFF1F60C4), Color(0xFF1250B0)),
  'dark-orange': (Color(0xFFFA6400), Color(0xFFE55400)),
  'dark-red': (Color(0xFFC4162A), Color(0xFFAD0317)),
  'dark-purple': (Color(0xFF8F3BB8), Color(0xFF7C2EA3)),
};

/// The order series take [chartColors] in: the six hues, then their darker
/// shades.
const chartSeriesOrder = [
  'green', 'semi-dark-yellow', 'blue', 'orange', 'red', 'purple', //
  'dark-green', 'dark-yellow', 'dark-blue', 'dark-orange', 'dark-red',
  'dark-purple',
];

/// The chart color [name] in this theme; null for a name it doesn't have.
Color? chartColor(BuildContext context, String name) {
  final shades = chartColors[name];
  if (shades == null) return null;
  return MediaQuery.platformBrightnessOf(context) == Brightness.dark
      ? shades.$1
      : shades.$2;
}

/// The [index]th series' color: the charts' palette in order.
Color paletteColor(BuildContext context, int index) =>
    chartColor(context, chartSeriesOrder[index % chartSeriesOrder.length])!;

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

/// The units a chart reads as Grafana does; any other is words after the
/// number ("tokens"), and none at all is a plain number.
const chartUnits = {
  'none',
  'short',
  'percent',
  'percentunit',
  'bytes',
  'ms',
  's',
};

/// How many decimals [value] reads well with when none are set: enough for
/// two or three figures, none for a whole number.
int autoDecimals(double value) {
  final size = value.abs();
  if (size == 0 || !size.isFinite || size == size.roundToDouble()) return 0;
  final power = (math.log(size) / math.ln10).floor();
  var decimals = 1 - power;
  if (size * math.pow(10, decimals) > 2.25) decimals++;
  return decimals.clamp(0, 10);
}

/// The decimals an axis's labels take at [step] apart: as many as the
/// step has (0.25 → 2, 50 → 0).
int stepDecimals(double step) {
  for (var decimals = 0; decimals < 10; decimals++) {
    final scaled = step.abs() * math.pow(10, decimals);
    if ((scaled - scaled.roundToDouble()).abs() < 1e-6 * math.max(1, scaled)) {
      return decimals;
    }
  }
  return 10;
}

/// [value] with [decimals], thousands grouped or not.
String _fixedNumber(double value, int decimals, {bool group = false}) {
  var text = value.abs().toStringAsFixed(decimals.clamp(0, 10));

  final negative = value < 0 && double.parse(text) != 0;
  if (group) {
    final dot = text.indexOf('.');
    final whole = dot < 0 ? text : text.substring(0, dot);
    final out = StringBuffer();
    for (var i = 0; i < whole.length; i++) {
      if (i > 0 && (whole.length - i) % 3 == 0) out.write(',');
      out.write(whole[i]);
    }
    if (dot >= 0) out.write(text.substring(dot));
    text = out.toString();
  }
  return negative ? '−$text' : text;
}

/// [value] in [unit], as a chart writes it: Grafana's units scaled as it
/// scales them ("1.2 K", "3.4 MiB", "1.5 min", "45%"); other words after
/// the number, grouped ("12,345 tokens"). [decimals] when set, else
/// automatic; on an axis ([step], the labels' spacing) as many as the step
/// needs, and without words of its own unit, short ("12K").
String chartValue(double value, String unit, {int? decimals, double? step}) {
  final (number, suffix) = chartValueParts(
    value,
    unit,
    decimals: decimals,
    step: step,
  );
  return '$number$suffix';
}

/// [chartValue] in two parts: the number, and what follows it (" K", "%",
/// " tokens"), which a Stat draws smaller.
(String, String) chartValueParts(
  double value,
  String unit, {
  int? decimals,
  double? step,
}) {
  final axis = step != null;
  String number(double v, double? at) => _fixedNumber(
    v,
    decimals ?? (at != null ? stepDecimals(at) : autoDecimals(v)),
  );
  (String, String) scaled(double factor, List<String> names) {
    var i = 0;
    var size = value.abs();
    while (size >= factor && i < names.length - 1) {
      size /= factor;
      i++;
    }
    final by = math.pow(factor, i).toDouble();
    return (number(value / by, axis ? step / by : null), names[i]);
  }

  (String, String) timed(List<(double, String)> steps) {
    var by = steps.first.$1;
    var name = steps.first.$2;
    for (final (size, unitName) in steps) {
      if (value.abs() >= size) {
        by = size;
        name = unitName;
      }
    }
    return (number(value / by, axis ? step / by : null), ' $name');
  }

  switch (unit) {
    case 'none':
      return (number(value, step), '');
    case 'short':
      return scaled(1000, const ['', ' K', ' Mil', ' Bil', ' Tri']);
    case 'percent':
      return (number(value, step), '%');
    case 'percentunit':
      final percent = value * 100;
      return (
        _fixedNumber(
          percent,
          decimals ?? (axis ? stepDecimals(step * 100) : autoDecimals(percent)),
        ),
        '%',
      );
    case 'bytes':
      return scaled(1024, const [' B', ' KiB', ' MiB', ' GiB', ' TiB', ' PiB']);
    case 'ms':
      return timed(const [
        (1, 'ms'),
        (1000, 's'),
        (60000, 'min'),
        (3600000, 'hour'),
        (86400000, 'day'),
      ]);
    case 's':
      if (value != 0 && value.abs() < 1) {
        return (number(value * 1000, axis ? step * 1000 : null), ' ms');
      }
      return timed(const [
        (1, 's'),
        (60, 'min'),
        (3600, 'hour'),
        (86400, 'day'),
        (604800, 'week'),
        (31536000, 'year'),
      ]);
  }
  if (axis) {
    if (decimals != null) {
      return (_fixedNumber(value, decimals, group: true), '');
    }
    // Short (K, M, B, T), with as many decimals as the step needs there, so
    // labels a step apart never read the same ("1.025M" by "1.05M").
    const names = ['', 'K', 'M', 'B', 'T'];
    var i = 0;
    for (var size = value.abs(); size >= 1000 && i < names.length - 1; i++) {
      size /= 1000;
    }
    final by = math.pow(1000, i).toDouble();
    return (
      '${_fixedNumber(value / by, stepDecimals(step / by))}${names[i]}',
      '',
    );
  }
  // As many decimals as Grafana's would show, grouped as Claktory's are.
  final text = _fixedNumber(
    value,
    decimals ?? autoDecimals(value),
    group: true,
  );
  return (text, unit.isEmpty ? '' : ' $unit');
}

/// How long bars and slices take to move to new data: none with Reduce
/// motion, or while the window's hidden (a muted ticker would hold them
/// at the old data until it shows again).
Duration chartMotion(BuildContext context) =>
    MediaQuery.disableAnimationsOf(context) ||
        !TickerMode.valuesOf(context).enabled
    ? Duration.zero
    : Motion.base;

/// A value axis fitted to [values] as Grafana fits one: 10% of their
/// spread past each end, rounded out to a tenth of its order of magnitude,
/// and kept at zero when the padding alone would cross it. [min] and [max]
/// set either end instead.
({double min, double max}) fitRange(
  Iterable<double> values, {
  double? min,
  double? max,
}) {
  var low = double.infinity;
  var high = double.negativeInfinity;
  for (final value in values) {
    if (!value.isFinite) continue;
    low = math.min(low, value);
    high = math.max(high, value);
  }
  if (low > high) {
    final lo = min ?? (max == null ? 0.0 : max - 1);
    return (min: lo, max: max ?? lo + 1);
  }
  final spread = high - low;
  final size = spread > 0 ? spread : (high == 0 ? 1.0 : high.abs());
  final pad = spread > 0 ? spread * 0.1 : size * 0.1;
  final unit =
      math.pow(10, (math.log(size) / math.ln10).floor()).toDouble() / 10;
  var lo = ((low - pad) / unit).floor() * unit;
  var hi = ((high + pad) / unit).ceil() * unit;
  if (low >= 0 && lo < 0) lo = 0;
  if (high <= 0 && hi > 0) hi = 0;
  lo = min ?? lo;
  hi = max ?? hi;
  if (hi <= lo) {
    if (max == null) {
      hi = lo + size;
    } else {
      lo = hi - size;
    }
  }
  return (min: lo, max: hi);
}

/// The step between an axis's labels: the least of 1, 2, 2.5 or 5 times a
/// power of ten that keeps them at least [space] apart over [length].
double axisStep(double range, double length, {double space = 40}) {
  if (range <= 0 || !range.isFinite || length <= 0) return 1;
  final least = range * space / length;
  final power = math.pow(10, (math.log(least) / math.ln10).floor()).toDouble();
  for (final nice in const [1.0, 2.0, 2.5, 5.0, 10.0]) {
    if (nice * power >= least * (1 - 1e-9)) return nice * power;
  }
  return 10 * power;
}

/// One series of a chart: its name and its points, (x as the table has
/// it, y).
class ChartSeries {
  ChartSeries(this.name);

  final String name;
  final points = <(Object?, double)>[];

  /// Each point's row in the table, in step with [points]: categories read
  /// in the table's order, whichever series has them first.
  final rows = <int>[];
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
  void add(String name, Object? at, Object? cell, int row) {
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
    one.rows.add(row);
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
    for (final (r, row) in table.rows.indexed) {
      add(cellText(row[si]), row[xi], row[yi], r);
    }
  } else {
    final ys = <int>[
      if (y.isEmpty)
        for (var i = 0; i < table.columns.length; i++)
          if (i != xi && table.numeric(i)) i,
      for (final name in y) table.columns.indexOf(name),
    ];
    if (ys.contains(-1)) return null;
    for (final (r, row) in table.rows.indexed) {
      for (final yi in ys) {
        add(table.columns[yi], row[xi], row[yi], r);
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

/// The words in a chart panel (IDENTITY's chart palette), one color as
/// Grafana's are: its title, its axes' labels, its legend's names and its
/// tooltip.
Color chartText(BuildContext context) =>
    MediaQuery.platformBrightnessOf(context) == Brightness.dark
    ? const Color(0xFFCCCCDC)
    : const Color(0xFF24292E);

/// A chart's gridlines, as Grafana draws them: one device pixel, a faint
/// tint of the text over the panel.
FlLine chartGridLine(BuildContext context) => FlLine(
  color: MediaQuery.platformBrightnessOf(context) == Brightness.dark
      ? const Color(0x17F0FAFF)
      : const Color(0x1F24292E),
  strokeWidth: 1 / MediaQuery.devicePixelRatioOf(context),
);

/// A Chart's or Stat's label as Grafana's panel title: 14px, weight 500,
/// in [chartText].
TextStyle panelTitle(BuildContext context) => TypeScale.of(context).caption
    .copyWith(
      fontSize: 14,
      height: 1.3,
      fontWeight: FontWeight.w500,
      color: chartText(context),
    );

/// A Chart component: [kind] timeseries, bars, stackedBars, pie, scatter
/// or heatmap ([AppHeatmap]), of [table]'s columns, in Grafana's default look. Over it its
/// [label]; under it a legend and what was left out. A time series shows
/// what the pointer (or a finger) is on in a tooltip; the other kinds read
/// it out in place of the label.
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
    this.lineWidth = 1,
    this.fill = 0,
    this.points = 'auto',
    this.pointSize = 5,
    this.min,
    this.max,
    this.decimals,
    this.legend = true,
    this.timeFrom,
    this.timeTo,
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

  /// One of [chartUnits], or words after the number.
  final String unit;
  final double height;

  /// A time series' lines' width (0–10) and the opacity of the area under
  /// them (0–100).
  final double lineWidth;
  final double fill;

  /// auto (where they're far enough apart to see), always or never.
  final String points;

  /// A point's diameter.
  final double pointSize;

  /// The value axis's ends; each fitted to the data when null.
  final double? min;
  final double? max;

  /// The decimals values take; automatic when null.
  final int? decimals;

  /// Whether a legend shows under the chart.
  final bool legend;

  /// A time series' x axis ends, in milliseconds since 1970: the
  /// dashboard's time range, as Grafana's panels span it. Rows outside
  /// it aren't drawn; an end left null comes from the data.
  final double? timeFrom;
  final double? timeTo;

  @override
  State<AppChart> createState() => _AppChartState();
}

class _AppChartState extends State<AppChart> {
  /// What the pointer is on, in words; null when it's on nothing.
  String? _readout;

  /// The bar group or pie slice the pointer is on: the others dim.
  int? _touched;

  /// The one series shown, picked in the legend; null shows them all.
  int? _isolated;

  /// What a time series' tooltip is on: where the pointer is (or the
  /// finger was) in the chart, and the series and the point nearest it.
  ({Offset at, int series, int point})? _tip;

  /// A tap on the point the tooltip shows closes it as it lifts.
  bool _closing = false;

  /// Whether a finger (or a click) put the tooltip where it is, rather than
  /// a hovering pointer: it stays, with a close button.
  bool _pinned = false;

  void _unpin() => setState(() {
    _tip = null;
    _pinned = false;
  });

  /// The page this chart scrolls in, when it does.
  ScrollPosition? _page;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final page = Scrollable.maybeOf(context, axis: Axis.vertical)?.position;
    if (page == _page) return;
    _page?.removeListener(_scrolled);
    _page = page?..addListener(_scrolled);
  }

  @override
  void dispose() {
    _page?.removeListener(_scrolled);
    super.dispose();
  }

  /// The page scrolled under a still pointer: what it pointed at is no
  /// longer under it, so a hovering pointer's tooltip and readout go until
  /// it moves again. A finger's (pinned) tooltip moves with its chart.
  void _scrolled() {
    final hovering = _tip != null && !_pinned;
    if (!hovering && _readout == null && _touched == null) return;
    // A scroll corrected in layout waits for the frame's end.
    if (SchedulerBinding.instance.schedulerPhase ==
        SchedulerPhase.persistentCallbacks) {
      SchedulerBinding.instance.addPostFrameCallback((_) {
        if (mounted) _scrolled();
      });
      return;
    }
    setState(() {
      if (hovering) _tip = null;
      _readout = null;
      _touched = null;
    });
  }

  /// Whether the [index]th series shows (all, or only the one picked).
  bool _shown(int index) => _isolated == null || _isolated == index;

  /// A tap on a legend entry shows only its series; another on it, all of
  /// them again.
  void _isolate(int index) => setState(() {
    _isolated = _isolated == index ? null : index;
    _tip = null;
  });

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
    if (widget.kind == 'heatmap') {
      return AppHeatmap(
        table: widget.table,
        x: widget.x,
        y: widget.y,
        seriesBy: widget.seriesBy,
        label: widget.label,
        unit: widget.unit,
        decimals: widget.decimals,
        legend: widget.legend,
        height: widget.height,
      );
    }
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
    // Under its label, so a dashboard says which chart it is.
    Widget labelled(String text) => widget.label.isEmpty
        ? note(text)
        : Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            spacing: Space.xs,
            children: [_heading(context, widget.label, lines: 2), note(text)],
          );
    if (table == null || built == null) {
      return labelled("Can't show this chart.");
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
    if (drawn == null) return labelled('Nothing to show.');
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
          // The label, on two lines at most (large text), or in its
          // place what the pointer is on, in as many lines as the label
          // takes, so nothing moves as it changes.
          ExcludeSemantics(child: _labelOrReadout(context)),
          ExcludeSemantics(
            // Its axes' words grow with the text only so far: past that
            // they'd break up in their room and cover the label.
            child: SizedBox(
              height: widget.height,
              child: MediaQuery.withClampedTextScaling(
                maxScaleFactor: maxAxisTextScale,
                child: drawn.chart,
              ),
            ),
          ),
          if (widget.legend &&
              (widget.kind == 'timeseries' || widget.kind == 'scatter'))
            _Legend(
              pills: true,
              entries: [
                for (var i = 0; i < series.length; i++)
                  (_seriesName(context, series[i].name), colors[i]),
              ],
              shown: [for (var i = 0; i < series.length; i++) _shown(i)],
              onTap: series.length > 1 ? _isolate : null,
            )
          else if (widget.legend && widget.kind != 'pie' && series.length > 1)
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

  Widget _labelOrReadout(BuildContext context) {
    final readout = _readout;
    final label = _heading(context, widget.label, lines: 2);
    if (readout == null) return label;
    if (widget.label.isEmpty) return _heading(context, readout, readout: true);
    return Stack(
      children: [
        Visibility.maintain(visible: false, child: label),
        Positioned.fill(
          child: LayoutBuilder(
            builder: (context, constraints) {
              final style = _headingStyle(context, readout: true);
              final line = TextPainter(
                text: TextSpan(text: 'A', style: style),
                textScaler: MediaQuery.textScalerOf(context),
                textDirection: Directionality.of(context),
              )..layout();
              final lines = math.max(
                1,
                (constraints.maxHeight / line.height + 0.01).floor(),
              );
              line.dispose();
              return _heading(context, readout, readout: true, lines: lines);
            },
          ),
        ),
      ],
    );
  }

  /// The label as Grafana's panel title (14px, 500, `text`); what the
  /// pointer is on in its place in caption `textMuted`.
  TextStyle _headingStyle(BuildContext context, {bool readout = false}) {
    final palette = Palette.of(context);
    final caption = TypeScale.of(context).caption;
    return readout
        ? caption.copyWith(
            color: palette.textMuted,
            fontFeatures: const [FontFeature.tabularFigures()],
          )
        : panelTitle(context);
  }

  Widget _heading(
    BuildContext context,
    String text, {
    bool readout = false,
    int lines = 1,
  }) {
    return Text(
      text,
      maxLines: lines,
      overflow: TextOverflow.ellipsis,
      style: _headingStyle(context, readout: readout),
    );
  }

  /// A series' color: by the chart's color role, an agent's own when its
  /// series are agents.
  Color _colorOf(BuildContext context, String name, int index) =>
      switch (widget.color) {
        'agent' => agentIdColor(context, name) ?? paletteColor(context, index),
        'signal' => Palette.of(context).signal,
        'muted' => Palette.of(context).textMuted,
        _ => paletteColor(context, index),
      };

  /// A series' name as the person reads it: an agent's name when the
  /// series are agents.
  String _seriesName(BuildContext context, String name) {
    if (widget.color != 'agent') return name;
    final scope = context.getInheritedWidgetOfExactType<AppScope>();
    return scope?.backend.workspace.agent(name)?.name ?? name;
  }

  String _value(double value) =>
      chartValue(value, widget.unit, decimals: widget.decimals);

  /// How much bigger than at the standard size the axes' words are drawn
  /// (the text size, up to [maxAxisTextScale]), for the room they're given.
  double _axisScale(BuildContext context) =>
      MediaQuery.textScalerOf(context)
          .clamp(maxScaleFactor: maxAxisTextScale)
          .scale(12) /
      12;

  /// Axis labels: Grafana's 12px, in [chartText].
  TextStyle _axis(BuildContext context) =>
      TypeScale.of(context).caption.copyWith(
        color: chartText(context),
        fontSize: 12,
        height: 1.2,
        fontFeatures: const [FontFeature.tabularFigures()],
      );

  /// How tall a line of axis words is, at the axes' text size.
  double _axisLineHeight(BuildContext context) =>
      (MediaQuery.textScalerOf(context)
                  .clamp(maxScaleFactor: maxAxisTextScale)
                  .scale(12) *
              1.2)
          .ceilToDouble() +
      2;

  /// How wide [text] is drawn on an axis, at the axes' text size.
  double _axisWidth(BuildContext context, String text) {
    final painter = TextPainter(
      text: TextSpan(text: text, style: _axis(context)),
      textScaler: MediaQuery.textScalerOf(context)
          .clamp(maxScaleFactor: maxAxisTextScale),
      textDirection: Directionality.of(context),
      maxLines: 1,
    )..layout();
    final width = painter.width;
    painter.dispose();
    return width;
  }

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
      reservedSize: 40 * _axisScale(context),
      interval: step,
      getTitlesWidget: (value, meta) => SideTitleWidget(
        meta: meta,
        // The top one inside the chart, never over its label.
        fitInside: SideTitleFitInsideData.fromTitleMeta(meta),
        child: Text(
          shortNumber(value),
          maxLines: 1,
          softWrap: false,
          style: _axis(context),
        ),
      ),
    ),
  );

  static const _none = AxisTitles();

  /// Between a scatter's y axis name and its labels.
  static const _nameGap = 14.0;

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
    final lines = <List<FlSpot>>[];
    final (start, end) = (widget.timeFrom, widget.timeTo);
    var first = double.infinity;
    var last = double.negativeInfinity;
    for (final one in series) {
      final spots = <FlSpot>[
        for (final (at, value) in one.points)
          if (cellTime(at) case final time?)
            if (time.millisecondsSinceEpoch.toDouble() case final x
                when x >= (start ?? x) && x <= (end ?? x))
              FlSpot(x, value),
      ]..sort((a, b) => a.x.compareTo(b.x));
      for (final spot in spots) {
        first = math.min(first, spot.x);
        last = math.max(last, spot.x);
      }
      lines.add(spots);
    }
    if (first > last) return null;
    first = start ?? first;
    last = end ?? last;
    if (first == last) {
      first -= 3600000;
      last += 3600000;
    }
    final span = Duration(milliseconds: (last - first).round());
    // The value axis fits the series shown.
    final range = fitRange(
      [
        for (var i = 0; i < lines.length; i++)
          if (_shown(i))
            for (final spot in lines[i]) spot.y,
      ],
      min: widget.min,
      max: widget.max,
    );
    // Time steps fall on the local clock's hours and days, not UTC's.
    final local = DateTime.fromMillisecondsSinceEpoch(first.round())
        .timeZoneOffset
        .inMilliseconds
        .toDouble();
    final scale = _axisScale(context);
    final timeRoom = 22 * scale;
    final chart = LayoutBuilder(
      builder: (context, constraints) {
        final plotHeight = math.max(1.0, widget.height - timeRoom);
        final step = axisStep(range.max - range.min, plotHeight);
        String valueLabel(double value) => chartValue(
          value,
          widget.unit,
          decimals: widget.decimals,
          step: step,
        );
        // The value axis as wide as its widest label.
        var widest = 0.0;
        for (
          var value = (range.min / step).ceil() * step;
          value <= range.max + step * 1e-6;
          value += step
        ) {
          widest = math.max(widest, _axisWidth(context, valueLabel(value)));
        }
        final valueRoom = widest.ceilToDouble() + Space.s;
        final plotWidth = math.max(1.0, constraints.maxWidth - valueRoom);
        // Time labels as far apart as the widest takes, and some.
        final sample = axisTime(
          DateTime(2026, 12, 28, 23, 58, 58),
          span,
          timeStep(span),
        );
        // About a label's width and a 12pt gap apart, as Grafana's are.
        final half = _axisWidth(context, sample) / 2;
        final apart = half * 2 + 12 * scale;
        final most = (plotWidth / apart).floor().clamp(1, 12);
        final time = timeStep(span, most: most);
        final every = time.inMilliseconds.toDouble();
        // Points where they're far enough apart to see, as Grafana's
        // "auto": at least two of their own widths between them.
        bool dotted(List<FlSpot> spots) => switch (widget.points) {
          'always' => true,
          'never' => spots.length == 1,
          _ =>
            spots.length == 1 ||
                plotWidth *
                        (spots.last.x - spots.first.x) /
                        (last - first) /
                        (spots.length - 1) >=
                    widget.pointSize * 2,
        };
        final held = _tip;
        // A tooltip on a point the data no longer has goes.
        final tip =
            held != null &&
                held.series < lines.length &&
                held.point < lines[held.series].length &&
                _shown(held.series)
            ? held
            : null;
        bool onStep(double value, double base, double interval) {
          final off = (value - base) % interval;
          return off < interval * 0.01 || off > interval * 0.99;
        }

        // A time on a step to the second: the axis's own ends (fl_chart
        // labels them too) are hours off one at a 90-day range, which a
        // share of the step would let through beside the step's own.
        bool onTimeStep(double value) {
          final off = (value + local) % every;
          return off < 1000 || off > every - 1000;
        }

        return Stack(
          clipBehavior: Clip.none,
          children: [
            // Grafana redraws on new data, with no tween: nor does a
            // muted ticker (a hidden window) leave the old line drawn.
            LineChart(
              duration: Duration.zero,
              LineChartData(
                minX: first,
                maxX: last,
                minY: range.min,
                maxY: range.max,
                baselineX: -local,
                gridData: FlGridData(
                  horizontalInterval: step,
                  verticalInterval: every,
                  getDrawingHorizontalLine: (_) => chartGridLine(context),
                  getDrawingVerticalLine: (_) => chartGridLine(context),
                ),
                borderData: FlBorderData(show: false),
                titlesData: FlTitlesData(
                  topTitles: _none,
                  rightTitles: _none,
                  leftTitles: AxisTitles(
                    sideTitles: SideTitles(
                      showTitles: true,
                      reservedSize: valueRoom,
                      interval: step,
                      getTitlesWidget: (value, meta) {
                        if (!onStep(value, 0, step)) {
                          return const SizedBox.shrink();
                        }
                        return SideTitleWidget(
                          meta: meta,
                          space: Space.s,
                          fitInside: SideTitleFitInsideData.fromTitleMeta(meta),
                          child: Text(
                            valueLabel(value),
                            maxLines: 1,
                            softWrap: false,
                            style: _axis(context),
                          ),
                        );
                      },
                    ),
                  ),
                  bottomTitles: AxisTitles(
                    sideTitles: SideTitles(
                      showTitles: true,
                      reservedSize: timeRoom,
                      interval: every,
                      getTitlesWidget: (value, meta) {
                        // On a step, centred under it (one near the left
                        // has the value axis's room). One too near the right
                        // end to be is drawn inside it, as Grafana's last
                        // is, while that leaves it clear of the one before.
                        final x = (value - first) / (last - first) * plotWidth;
                        final shift = half - (plotWidth - x);
                        final between = every / (last - first) * plotWidth;
                        if (!onTimeStep(value) ||
                            (shift > 0 && between - shift < half * 2 + 2) ||
                            x + valueRoom < half) {
                          return const SizedBox.shrink();
                        }
                        return SideTitleWidget(
                          meta: meta,
                          fitInside: shift > 0
                              ? SideTitleFitInsideData.fromTitleMeta(
                                  meta,
                                  distanceFromEdge: 0,
                                )
                              : SideTitleFitInsideData.disable(),
                          child: Text(
                            axisTime(
                              DateTime.fromMillisecondsSinceEpoch(
                                value.round(),
                              ),
                              span,
                              time,
                            ),
                            maxLines: 1,
                            softWrap: false,
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
                      show: _shown(i),
                      spots: lines[i],
                      color: colors[i],
                      barWidth: widget.lineWidth,
                      isStrokeCapRound: true,
                      isStrokeJoinRound: true,
                      dotData: FlDotData(
                        show: dotted(lines[i]),
                        getDotPainter: (spot, percent, bar, index) =>
                            FlDotCirclePainter(
                              radius: widget.pointSize / 2,
                              color: colors[i],
                              strokeWidth: 0,
                            ),
                      ),
                      belowBarData: BarAreaData(
                        show: widget.fill > 0,
                        color: colors[i].withValues(alpha: widget.fill / 100),
                      ),
                    ),
                ],
                lineTouchData: LineTouchData(
                  handleBuiltInTouches: false,
                  touchSpotThreshold: double.infinity,
                  touchCallback: (event, response) =>
                      _touchLine(event, response, lines),
                ),
              ),
            ),
            if (tip != null)
              ..._tipLayer(
                plot: Rect.fromLTWH(valueRoom, 0, plotWidth, plotHeight),
                point: Offset(
                  (lines[tip.series][tip.point].x - first) / (last - first),
                  (range.max - lines[tip.series][tip.point].y) /
                      (range.max - range.min),
                ),
                color: colors[tip.series],
                tooltip: _Tooltip(
                  time: tooltipTime(
                    DateTime.fromMillisecondsSinceEpoch(
                      lines[tip.series][tip.point].x.round(),
                    ),
                  ),
                  name: _seriesName(context, series[tip.series].name),
                  color: colors[tip.series],
                  value: _value(lines[tip.series][tip.point].y),
                  onClose: _pinned ? _unpin : null,
                ),
              ),
          ],
        );
      },
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

  /// What marks the point a tooltip is on, as Grafana's cursor does: a
  /// dashed crosshair through it across [plot] (where the chart draws), the
  /// point larger in its [color] with a halo; then the tooltip beside it.
  /// [point] is where in [plot], as fractions of its width and height.
  List<Widget> _tipLayer({
    required Rect plot,
    required Offset point,
    required Color color,
    required _Tooltip tooltip,
  }) => [
    Positioned.fromRect(
      rect: plot,
      child: IgnorePointer(
        child: CustomPaint(
          painter: _Crosshair(
            at: point,
            color: color,
            line: MediaQuery.platformBrightnessOf(context) == Brightness.dark
                ? const Color(0xFF607D8B)
                : const Color(0xFF8E9BA6),
            width: 1 / MediaQuery.devicePixelRatioOf(context),
          ),
        ),
      ),
    ),
    // Beside the point rather than under the finger, so neither the finger
    // nor the box hides it.
    Positioned.fill(
      child: CustomSingleChildLayout(
        delegate: _TooltipPlace(
          Offset(
            plot.left + point.dx * plot.width,
            plot.top + point.dy * plot.height,
          ),
        ),
        child: tooltip,
      ),
    ),
  ];

  /// A time series' tooltip follows the pointer, on the series nearest it
  /// (Grafana's single mode). A mouse's goes as it leaves; a finger's stays
  /// once lifted, so what it was on can be read, until a tap on the same
  /// point closes it.
  void _touchLine(
    FlTouchEvent event,
    LineTouchResponse? response,
    List<List<FlSpot>> lines,
  ) {
    ({Offset at, int series, int point})? next;
    final spots = [
      for (final spot in response?.lineBarSpots ?? const <TouchLineBarSpot>[])
        if (_shown(spot.barIndex)) spot,
    ];
    if (response != null && spots.isNotEmpty) {
      final (x, y) = (
        response.touchChartCoordinate.dx,
        response.touchChartCoordinate.dy,
      );
      // The time under the pointer first, as Grafana's: only the series
      // with a point at the nearest time (a series' lone point far off
      // in time doesn't win on its value), then the nearest value.
      var first = double.infinity;
      var last = double.negativeInfinity;
      for (final line in lines) {
        if (line.isEmpty) continue;
        first = math.min(first, line.first.x);
        last = math.max(last, line.last.x);
      }
      final closest = spots.map((spot) => (spot.x - x).abs()).reduce(math.min);
      final slack = closest + (last - first) * 0.005;
      final atTime = [
        for (final spot in spots)
          if ((spot.x - x).abs() <= slack) spot,
      ];
      var near = atTime.first;
      for (final spot in atTime) {
        if ((spot.y - y).abs() < (near.y - y).abs()) near = spot;
      }
      next = (
        at: response.touchLocation,
        series: near.barIndex,
        point: near.spotIndex,
      );
    }
    // Between a line chart's points the nearest still shows.
    _moveTip(event, next, keepOnNothing: true);
  }

  /// Moves the tooltip to [next] (what the pointer or finger is on), or
  /// away. A mouse's goes as it leaves; a finger's stays once lifted, until
  /// a tap on the same point closes it (or, unless [keepOnNothing], a tap
  /// on nothing).
  void _moveTip(
    FlTouchEvent event,
    ({Offset at, int series, int point})? next, {
    bool keepOnNothing = false,
  }) {
    final tip = _tip;
    final hover = event is FlPointerHoverEvent || event is FlPointerEnterEvent;
    void show() {
      if (next == null ||
          (tip != null &&
              tip.series == next.series &&
              tip.point == next.point &&
              tip.at == next.at &&
              _pinned == !hover)) {
        return;
      }
      setState(() {
        _tip = next;
        _pinned = !hover;
      });
    }

    switch (event) {
      // A hovering pointer's goes as it leaves; a pinned one stays.
      case FlPointerExitEvent():
        if (tip != null && !_pinned) setState(() => _tip = null);
      // A press shows nothing yet: it may become a scroll of the page,
      // which must leave no tooltip. It only decides whether its tap, if
      // it is one, closes the tooltip (on its point, or on nothing).
      case FlPanDownEvent():
        _closing =
            tip != null &&
            (next == null
                ? !keepOnNothing
                : tip.series == next.series && tip.point == next.point);
      case FlTapUpEvent():
        if (_closing) {
          _closing = false;
          _unpin();
        } else {
          show();
        }
      // A finger moving on the chart (the chart, not the page, took the
      // drag) or holding still on it shows what it's on; so does a
      // hovering pointer, unless a click pinned the tooltip.
      case FlPanStartEvent() ||
          FlPanUpdateEvent() ||
          FlLongPressStart() ||
          FlLongPressMoveUpdate():
        _closing = false;
        show();
      case FlPointerHoverEvent() || FlPointerEnterEvent():
        if (_pinned) return;
        if (next == null) {
          if (!keepOnNothing && tip != null) setState(() => _tip = null);
          return;
        }
        show();
      default:
        return;
    }
  }

  _Drawn? _bars(
    BuildContext context,
    List<ChartSeries> series,
    List<Color> colors, {
    required bool stacked,
  }) {
    final palette = Palette.of(context);
    // The categories in the table's order (the row each first comes in,
    // whichever series has it: a later series' new buckets don't go to the
    // end), each series' value in each (summed, should one come twice).
    final first = <String, int>{};
    for (final one in series) {
      for (final (k, (at, _)) in one.points.indexed) {
        final name = cellText(at);
        final row = k < one.rows.length ? one.rows[k] : k;
        final seen = first[name];
        if (seen == null || row < seen) first[name] = row;
      }
    }
    final ordered = first.keys.toList()
      ..sort((a, b) => first[a]!.compareTo(first[b]!));
    final categories = ordered.take(maxChartBars).toList();
    final barsLeft = ordered.length - categories.length;
    final index = {for (final (i, name) in categories.indexed) name: i};
    final values = [for (final _ in series) <int, double>{}];
    for (var s = 0; s < series.length; s++) {
      for (final (at, value) in series[s].points) {
        final i = index[cellText(at)];
        if (i == null) continue;
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
    // Times on the axis as the time axis writes them ("10/27", "14:00"),
    // apart by the least gap between two bars.
    final times = [for (final name in categories) _categoryTime(name)];
    final timed = times.every((time) => time != null);
    var span = Duration.zero;
    Duration? gap;
    if (timed) {
      final sorted = [for (final time in times) time!]..sort();
      span = sorted.last.difference(sorted.first);
      for (var k = 1; k < sorted.length; k++) {
        final apart = sorted[k].difference(sorted[k - 1]);
        if (apart > Duration.zero && (gap == null || apart < gap)) gap = apart;
      }
    }
    return _Drawn(
      LayoutBuilder(
        builder: (context, constraints) {
          final scale = _axisScale(context);
          final room =
              math.max(1.0, constraints.maxWidth - 40 * scale) /
              categories.length;
          final rods = stacked ? 1 : series.length;
          final width = (room * 0.7 / rods).clamp(2.0, 24.0);
          // Labels where there's room for them: every one, or every few.
          final every = math.max(1, (56 * scale / room).ceil());
          return BarChart(
            duration: chartMotion(context),
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
                    reservedSize: 22 * _axisScale(context),
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
                            timed
                                ? axisTime(
                                    times[i]!,
                                    span,
                                    (gap ?? const Duration(days: 1)) * every,
                                  )
                                : _category(categories[i]),
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

  /// A category's words: a time when it reads as one ("22 Nov", "20 Nov
  /// 12:00"): an ISO date, or a number big enough to be milliseconds since
  /// 1970, as the time axis reads them.
  String _category(String name) => switch (_categoryTime(name)) {
    final time? => readoutTime(time),
    null => name,
  };

  /// The time a category names, if it reads as one.
  static DateTime? _categoryTime(String name) {
    if (name.length >= 10 && RegExp(r'^\d{4}-\d{2}-\d{2}').hasMatch(name)) {
      if (DateTime.tryParse(name) case final time?) return time;
    }
    // As cellText writes a number: grouped ("1,795,132,800,000").
    final number = double.tryParse(name.replaceAll(',', ''));
    if (number != null && number.abs() >= 1e11 && number.abs() < 1e14) {
      return DateTime.fromMillisecondsSinceEpoch(number.round());
    }
    return null;
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
    // The donut at the chart's height, smaller where the width can't give
    // it that beside a legend of a few words (a phone, half a row).
    final chart = LayoutBuilder(
      builder: (context, constraints) {
        final donut = math.min(
          widget.height,
          math.max(48.0, constraints.maxWidth - Space.l - _leastLegend),
        );
        return Row(
          spacing: Space.l,
          crossAxisAlignment: CrossAxisAlignment.center,
          children: [
            SizedBox.square(
              dimension: donut,
              child: PieChart(
                duration: chartMotion(context),
                PieChartData(
                  startDegreeOffset: -90,
                  sectionsSpace: 1,
                  centerSpaceRadius: donut / 2 * 0.58,
                  sections: [
                    for (var i = 0; i < slices.length; i++)
                      PieChartSectionData(
                        value: slices[i].$2,
                        color: touched == null || touched == i
                            ? colors[i]
                            : colors[i].withValues(alpha: 0.45),
                        radius: donut / 2 * 0.42,
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
      },
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
    // Each series' points, as numbers.
    final points = [
      for (final one in series)
        <(double, double)>[
          for (final (at, value) in one.points)
            if (cellNumber(at) case final x?) (x, value),
        ],
    ];
    if (points.every((one) => one.isEmpty)) return null;
    final shown = [
      for (var s = 0; s < points.length; s++)
        if (_shown(s)) ...points[s],
    ];
    final ys = fitRange(
      [for (final (_, y) in shown) y],
      min: widget.min,
      max: widget.max,
    );
    final xs = fitRange([for (final (x, _) in shown) x]);
    final table = widget.table!;
    final xName = widget.x ?? table.columns.first;
    // The y axis's column: the one named, the only series' own, or (long)
    // the first number column that's neither x nor the series'.
    final seriesBy = widget.seriesBy;
    final yName = widget.y.length == 1
        ? widget.y.single
        : seriesBy == null
        ? (series.length == 1 ? series.single.name : '')
        : [
                for (var i = 0; i < table.columns.length; i++)
                  if (table.columns[i] != xName &&
                      table.columns[i] != seriesBy &&
                      table.numeric(i))
                    table.columns[i],
              ].firstOrNull ??
              '';
    // Each axis named in its column's name, as Grafana's XY chart does.
    final nameRoom = _axisLineHeight(context);
    Widget axisName(String name) => Text(
      name,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: _axis(context),
    );
    final scale = _axisScale(context);
    final xRoom = 22 * scale;
    final chart = LayoutBuilder(
      builder: (context, constraints) {
        final plotHeight = math.max(1.0, widget.height - xRoom - nameRoom);
        final yStep = axisStep(ys.max - ys.min, plotHeight);
        String yLabel(double value) => chartValue(
          value,
          widget.unit,
          decimals: widget.decimals,
          step: yStep,
        );
        var widest = 0.0;
        for (
          var value = (ys.min / yStep).ceil() * yStep;
          value <= ys.max + yStep * 1e-6;
          value += yStep
        ) {
          widest = math.max(widest, _axisWidth(context, yLabel(value)));
        }
        final valueRoom = widest.ceilToDouble() + Space.s;
        final plotWidth = math.max(
          1.0,
          constraints.maxWidth -
              valueRoom -
              (yName.isEmpty ? 0 : nameRoom + _nameGap),
        );
        // x labels as far apart as the widest takes, and some.
        final guess = axisStep(xs.max - xs.min, plotWidth, space: 64 * scale);
        String xLabel(double value, double step) =>
            chartValue(value, '', step: step);
        var xWidest = 0.0;
        for (final value in [xs.min, xs.max, xs.min + guess]) {
          xWidest = math.max(
            xWidest,
            _axisWidth(context, xLabel(value, guess)),
          );
        }
        final xStep = axisStep(
          xs.max - xs.min,
          plotWidth,
          space: xWidest + 24 * scale,
        );
        final half = xWidest / 2;
        bool onStep(double value, double step) {
          final off = value % step;
          return off.abs() < step * 0.01 || (step - off).abs() < step * 0.01;
        }

        final held = _tip;
        final tip =
            held != null &&
                held.series < points.length &&
                held.point < points[held.series].length &&
                _shown(held.series)
            ? held
            : null;
        // The drawn spots, and whose each is.
        final spots = <ScatterSpot>[];
        final whose = <(int, int)>[];
        for (var s = 0; s < points.length; s++) {
          if (!_shown(s)) continue;
          for (final (k, (x, y)) in points[s].indexed) {
            spots.add(
              ScatterSpot(
                x,
                y,
                // pointSize across, its 1px ring inside it.
                dotPainter: FlDotCirclePainter(
                  radius: math.max(0.5, widget.pointSize / 2 - 1),
                  color: colors[s].withValues(alpha: 0.5),
                  strokeWidth: 1,
                  strokeColor: colors[s],
                ),
              ),
            );
            whose.add((s, k));
          }
        }
        return Stack(
          clipBehavior: Clip.none,
          children: [
            ScatterChart(
              duration: Duration.zero,
              ScatterChartData(
                scatterSpots: spots,
                minX: xs.min,
                maxX: xs.max,
                minY: ys.min,
                maxY: ys.max,
                gridData: FlGridData(
                  horizontalInterval: yStep,
                  verticalInterval: xStep,
                  getDrawingHorizontalLine: (_) => chartGridLine(context),
                  getDrawingVerticalLine: (_) => chartGridLine(context),
                ),
                borderData: FlBorderData(show: false),
                titlesData: FlTitlesData(
                  topTitles: _none,
                  rightTitles: _none,
                  leftTitles: AxisTitles(
                    // Turned up the left, its foot (the bottom, turned)
                    // well clear of the labels, as Grafana's sits by the
                    // panel's edge.
                    axisNameWidget: yName.isEmpty
                        ? null
                        : Padding(
                            padding: const EdgeInsets.only(bottom: _nameGap),
                            child: axisName(yName),
                          ),
                    axisNameSize: yName.isEmpty ? 16 : nameRoom + _nameGap,
                    sideTitles: SideTitles(
                      showTitles: true,
                      reservedSize: valueRoom,
                      interval: yStep,
                      getTitlesWidget: (value, meta) {
                        if (!onStep(value, yStep)) {
                          return const SizedBox.shrink();
                        }
                        return SideTitleWidget(
                          meta: meta,
                          space: Space.s,
                          fitInside: SideTitleFitInsideData.fromTitleMeta(meta),
                          child: Text(
                            yLabel(value),
                            maxLines: 1,
                            softWrap: false,
                            style: _axis(context),
                          ),
                        );
                      },
                    ),
                  ),
                  bottomTitles: AxisTitles(
                    axisNameWidget: axisName(xName),
                    axisNameSize: nameRoom,
                    sideTitles: SideTitles(
                      showTitles: true,
                      reservedSize: xRoom,
                      interval: xStep,
                      getTitlesWidget: (value, meta) {
                        final x =
                            (value - xs.min) / (xs.max - xs.min) * plotWidth;
                        if (!onStep(value, xStep) ||
                            plotWidth - x < half ||
                            x + valueRoom < half) {
                          return const SizedBox.shrink();
                        }
                        return SideTitleWidget(
                          meta: meta,
                          child: Text(
                            xLabel(value, xStep),
                            maxLines: 1,
                            softWrap: false,
                            style: _axis(context),
                          ),
                        );
                      },
                    ),
                  ),
                ),
                scatterTouchData: ScatterTouchData(
                  handleBuiltInTouches: false,
                  touchSpotThreshold: math.max(12, widget.pointSize),
                  touchCallback: (event, response) {
                    final i = response?.touchedSpot?.spotIndex;
                    _moveTip(
                      event,
                      i == null || i < 0 || i >= whose.length
                          ? null
                          : (
                              at: response!.touchLocation,
                              series: whose[i].$1,
                              point: whose[i].$2,
                            ),
                    );
                  },
                ),
              ),
            ),
            if (tip != null)
              ..._tipLayer(
                plot: Rect.fromLTWH(
                  valueRoom + (yName.isEmpty ? 0 : nameRoom + _nameGap),
                  0,
                  plotWidth,
                  plotHeight,
                ),
                point: Offset(
                  (points[tip.series][tip.point].$1 - xs.min) /
                      (xs.max - xs.min),
                  (ys.max - points[tip.series][tip.point].$2) /
                      (ys.max - ys.min),
                ),
                color: colors[tip.series],
                tooltip: _Tooltip(
                  time:
                      '$xName ${chartValue(points[tip.series][tip.point].$1, '')}',
                  name: _seriesName(context, series[tip.series].name),
                  color: colors[tip.series],
                  value: _value(points[tip.series][tip.point].$2),
                  onClose: _pinned ? _unpin : null,
                ),
              ),
          ],
        );
      },
    );
    final count = points.fold(0, (sum, one) => sum + one.length);
    return _Drawn(
      chart,
      'Scatter chart, $count points'
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

/// A legend: each series' marker in its color and its name in caption
/// `textMuted`, in a wrapping row (or a column, beside a pie). The marker
/// is an 8px dot, or with [pills] Grafana's 14×4px pill. With [onTap] an
/// entry is a button picking its series; a series not [shown] is faint.
class _Legend extends StatelessWidget {
  const _Legend({
    required this.entries,
    this.vertical = false,
    this.pills = false,
    this.shown,
    this.onTap,
  });

  final List<(String, Color)> entries;
  final bool vertical;
  final bool pills;
  final List<bool>? shown;
  final void Function(int index)? onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final style = TypeScale.of(context).caption.copyWith(
      color: pills ? chartText(context) : palette.textMuted,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final tap = onTap;
    final items = <Widget>[
      for (final (i, (name, color)) in entries.indexed)
        () {
          final on = shown == null || shown![i];
          Widget item = Opacity(
            opacity: on ? 1 : 0.4,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              spacing: Space.xs + 2,
              children: [
                Container(
                  width: pills ? 14 : 8,
                  height: pills ? 4 : 8,
                  decoration: BoxDecoration(
                    color: color,
                    borderRadius: pills ? BorderRadius.circular(2) : null,
                    shape: pills ? BoxShape.rectangle : BoxShape.circle,
                  ),
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
          );
          if (tap != null) {
            item = Semantics(
              button: true,
              selected:
                  on && shown != null && shown!.where((s) => s).length == 1,
              child: MouseRegion(
                cursor: SystemMouseCursors.click,
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: () => tap(i),
                  child: Padding(
                    padding: const EdgeInsets.symmetric(vertical: Space.xs),
                    child: item,
                  ),
                ),
              ),
            );
          } else if (pills) {
            item = ExcludeSemantics(child: item);
          }
          return item;
        }(),
    ];
    return vertical
        ? Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            spacing: Space.xs,
            children: items,
          )
        : Wrap(
            spacing: Space.l,
            runSpacing: tap == null ? Space.xs : 0,
            children: items,
          );
  }
}

/// A chart's tooltip, as Grafana's: the time (or a scatter's x) in
/// [chartText], a divider, then the series' pill, its name at 65% and its
/// value right-aligned, all 12px, in a box of Grafana's tooltip tones (with
/// a close button when it stays put).
class _Tooltip extends StatelessWidget {
  const _Tooltip({
    required this.time,
    required this.name,
    required this.color,
    required this.value,
    this.onClose,
  });

  final String time;
  final String name;
  final Color color;
  final String value;

  /// Closes it; null when it goes by itself (a hovering pointer's).
  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    final dark = MediaQuery.platformBrightnessOf(context) == Brightness.dark;
    final text = chartText(context);
    final fill = dark ? const Color(0xFF22252B) : const Color(0xFFFFFFFF);
    final edge = dark ? const Color(0xFF373941) : const Color(0x1F24292E);
    final style = TypeScale.of(context).caption.copyWith(
      fontSize: 12,
      height: 1.3,
      color: text,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final close = onClose;
    return Container(
      constraints: const BoxConstraints(maxWidth: 280),
      decoration: BoxDecoration(
        color: fill,
        border: Border.all(color: edge),
        borderRadius: BorderRadius.circular(Radii.control),
        boxShadow: floatingShadow,
      ),
      // As wide as its widest row, the value at the box's right.
      child: IntrinsicWidth(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          mainAxisSize: MainAxisSize.min,
          children: [
            Padding(
              padding: EdgeInsets.fromLTRB(
                Space.s,
                Space.xs + 2,
                close == null ? Space.s : Space.xs,
                Space.xs + 2,
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                spacing: Space.s,
                children: [
                  Expanded(
                    child: IgnorePointer(
                      child: Text(time, maxLines: 1, style: style),
                    ),
                  ),
                  // (The chart's left out of what assistive technology reads,
                  // which hears its summary instead.)
                  if (close != null)
                    MouseRegion(
                      cursor: SystemMouseCursors.click,
                      child: GestureDetector(
                        behavior: HitTestBehavior.opaque,
                        onTap: close,
                        child: Padding(
                          padding: const EdgeInsets.all(2),
                          child: CustomPaint(
                            size: const Size.square(12),
                            painter: _Cross(text.withValues(alpha: 0.65)),
                          ),
                        ),
                      ),
                    ),
                ],
              ),
            ),
            SizedBox(height: 1, child: ColoredBox(color: edge)),
            IgnorePointer(
              child: Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: Space.s,
                  vertical: Space.xs + 2,
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Container(
                      width: 14,
                      height: 4,
                      decoration: BoxDecoration(
                        color: color,
                        borderRadius: BorderRadius.circular(2),
                      ),
                    ),
                    const SizedBox(width: Space.s),
                    Expanded(
                      child: Text(
                        name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: style.copyWith(
                          color: text.withValues(alpha: 0.65),
                        ),
                      ),
                    ),
                    const SizedBox(width: Space.l),
                    Text(
                      value,
                      maxLines: 1,
                      textAlign: TextAlign.end,
                      style: style.copyWith(fontWeight: FontWeight.w500),
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

/// A small ×, for a tooltip's close button.
class _Cross extends CustomPainter {
  const _Cross(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()
      ..color = color
      ..strokeWidth = 1.5
      ..strokeCap = StrokeCap.round;
    const inset = 2.5;
    canvas
      ..drawLine(
        const Offset(inset, inset),
        Offset(size.width - inset, size.height - inset),
        paint,
      )
      ..drawLine(
        Offset(size.width - inset, inset),
        Offset(inset, size.height - inset),
        paint,
      );
  }

  @override
  bool shouldRepaint(_Cross old) => old.color != color;
}

/// Grafana's cursor on a point: dashed lines through it, across and down
/// the plot, in [line] and [width], and the point a 10px dot in its
/// [color]: a solid centre and a see-through halo. [at] is where, as
/// fractions of the plot's width and height.
class _Crosshair extends CustomPainter {
  const _Crosshair({
    required this.at,
    required this.color,
    required this.line,
    required this.width,
  });

  final Offset at;
  final Color color;
  final Color line;
  final double width;

  @override
  void paint(Canvas canvas, Size size) {
    final point = Offset(at.dx * size.width, at.dy * size.height);
    final paint = Paint()
      ..color = line
      ..strokeWidth = width;
    const dash = 4.0;
    for (var x = 0.0; x < size.width; x += dash * 2) {
      canvas.drawLine(
        Offset(x, point.dy),
        Offset(math.min(x + dash, size.width), point.dy),
        paint,
      );
    }
    for (var y = 0.0; y < size.height; y += dash * 2) {
      canvas.drawLine(
        Offset(point.dx, y),
        Offset(point.dx, math.min(y + dash, size.height)),
        paint,
      );
    }
    canvas
      ..drawCircle(point, 5, Paint()..color = color.withValues(alpha: 0.3))
      ..drawCircle(point, 3, Paint()..color = color);
  }

  @override
  bool shouldRepaint(_Crosshair old) =>
      old.at != at ||
      old.color != color ||
      old.line != line ||
      old.width != width;
}

/// Puts a tooltip beside the point [at]: below and right of it, or left
/// and above where it would go past the chart's edges.
class _TooltipPlace extends SingleChildLayoutDelegate {
  const _TooltipPlace(this.at);

  final Offset at;

  static const _gap = 12.0;

  @override
  BoxConstraints getConstraintsForChild(BoxConstraints constraints) =>
      constraints.loosen();

  @override
  Offset getPositionForChild(Size size, Size child) {
    var x = at.dx + _gap;
    if (x + child.width > size.width) x = at.dx - _gap - child.width;
    var y = at.dy + _gap;
    if (y + child.height > size.height) y = at.dy - _gap - child.height;
    return Offset(
      x.clamp(0, math.max(0, size.width - child.width)),
      y.clamp(-child.height, math.max(0, size.height - child.height)),
    );
  }

  @override
  bool shouldRelayout(_TooltipPlace old) => old.at != at;
}

/// One of a Stat's thresholds: from [value] up (none: the base, below
/// every other) its value takes [color], a chart palette name or "text".
typedef StatStep = ({double? value, String color});

/// Grafana's thresholds when a Stat sets none: green, red from 80.
const defaultStatSteps = <StatStep>[
  (value: null, color: 'green'),
  (value: 80, color: 'red'),
];

/// A color a Stat names: a chart palette name, "text", or a role (signal,
/// muted, agent:`id`); null for one this app doesn't know.
Color? statColor(BuildContext context, String name) => name == 'text'
    ? Palette.of(context).text
    : chartColor(context, name) ?? roleColor(context, name);

/// Inside a Card, a Stat reaches the card's edges: [AppStatCard] gives it
/// the whole card, and this tells it to keep its words in from them itself.
class _InCard extends InheritedWidget {
  const _InCard({required super.child});

  @override
  bool updateShouldNotify(_InCard old) => false;
}

/// A Card holding a Stat: the card's look (the sidebar's color, a hairline,
/// [Radii.panel]) with no padding, its child clipped inside the hairline,
/// so the Stat's background and spark line reach the card's edges as a
/// Grafana panel's do.
class AppStatCard extends StatelessWidget {
  const AppStatCard({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      position: DecorationPosition.background,
      child: DecoratedBox(
        // The hairline over what the Stat paints, never under it.
        position: DecorationPosition.foreground,
        decoration: BoxDecoration(
          border: Border.all(color: palette.hairline),
          borderRadius: BorderRadius.circular(Radii.panel),
        ),
        child: ClipRRect(
          borderRadius: BorderRadius.circular(Radii.panel),
          child: _InCard(child: child),
        ),
      ),
    );
  }
}

/// A Stat component in Grafana's look: its [label] at the top, the [value]
/// in the color its [thresholds] give it (or its one [color]), as large as
/// the panel lets it, its [unit] smaller after it, and under it a spark
/// line of [spark] along the panel's bottom edge, 1px over a 20% fill.
/// [colorMode] value colors the value; background (a gradient) or solid
/// fill the panel with the color instead; none leaves the value in `text`.
/// No value says "No data".
class AppStat extends StatelessWidget {
  const AppStat({
    super.key,
    required this.label,
    required this.value,
    this.unit = '',
    this.spark = const [],
    this.color,
    this.thresholds,
    this.colorMode = 'value',
    this.graph = 'area',
    this.decimals,
    this.height,
  });

  final String label;

  /// A number, or words as they are; null for no data.
  final Object? value;

  /// One of [chartUnits], or words after the number.
  final String unit;
  final List<double> spark;

  /// The one color it takes when it has no [thresholds]: a chart palette
  /// name, "text" or a role.
  final String? color;

  /// Its thresholds, base first; Grafana's [defaultStatSteps] when null
  /// and it has no [color].
  final List<StatStep>? thresholds;

  /// value, background, solid or none.
  final String colorMode;

  /// area (the spark line) or none.
  final String graph;

  /// The decimals its value takes; automatic when null.
  final int? decimals;

  /// The panel's height; its own when null.
  final double? height;

  /// The value as it reads: a number grouped ("12,345") up to a million,
  /// short past it ("1.2M").
  static String words(Object? value) => switch (value) {
    final num n when n.isFinite && n.abs() >= 1e6 => shortNumber(n.toDouble()),
    final num n when n.isFinite => fullNumber(n.toDouble()),
    final String s => s,
    null => '',
    _ => '$value',
  };

  /// The value's number and what follows it.
  (String, String) _parts() {
    final v = value;
    if (v is num && v.isFinite) {
      if (chartUnits.contains(unit) || decimals != null) {
        return chartValueParts(v.toDouble(), unit, decimals: decimals);
      }
      // Grafana's automatic decimals (32.21 → 32.2), grouped; short past
      // a million.
      final number = v.abs() >= 1e6
          ? words(v)
          : _fixedNumber(v.toDouble(), autoDecimals(v.toDouble()), group: true);
      return (number, unit.isEmpty ? '' : ' $unit');
    }
    return (words(v), unit.isEmpty || v == null ? '' : ' $unit');
  }

  /// The color the value takes: its threshold's, or its one color.
  Color _color(BuildContext context) {
    final fixed = color;
    final steps = thresholds ?? (fixed == null ? defaultStatSteps : null);
    if (steps == null || steps.isEmpty) {
      return statColor(context, fixed ?? 'green') ?? Palette.of(context).text;
    }
    var name = steps.first.color;
    final v = value;
    if (v is num) {
      for (final step in steps.skip(1)) {
        final from = step.value;
        if (from != null && v >= from) name = step.color;
      }
    }
    return statColor(context, name) ?? Palette.of(context).text;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final inCard = context.getInheritedWidgetOfExactType<_InCard>() != null;
    // A plain Card's inset: its padding inside its hairline.
    final inset = inCard ? Space.m + 1 : 0.0;
    final (number, suffix) = _parts();
    final noData = value == null;
    final points = spark.take(maxSparkPoints).toList();
    final shade = _color(context);
    final filled = colorMode == 'background' || colorMode == 'solid';
    // On a filled panel, words in black or white, whichever reads.
    final onFill = shade.computeLuminance() > 0.5
        ? const Color(0xFF111217)
        : const Color(0xFFFFFFFF);
    final valueColor = noData
        ? palette.textMuted
        : filled
        ? onFill
        : colorMode == 'none'
        ? palette.text
        : shade;
    final line = filled ? onFill.withValues(alpha: 0.7) : shade;
    final area = filled
        ? onFill.withValues(alpha: 0.4)
        : shade.withValues(alpha: 0.2);
    final scaler = MediaQuery.textScalerOf(context);
    // The title stays in the panel's own colors, filled or not, as
    // Grafana's header does.
    final labelStyle = panelTitle(context);
    final labelHeight = label.isEmpty
        ? 0.0
        : scaler.scale(labelStyle.fontSize!) * labelStyle.height!;
    // What larger text adds to the title: a panel of a set height grows
    // by it, so the value keeps the room it has at the default size.
    final grown = label.isEmpty
        ? 0.0
        : math.max(
            0.0,
            labelHeight - labelStyle.fontSize! * labelStyle.height!,
          );
    // The title's row, as Grafana's panel header: the value, its spark
    // line and a fill take what's under it.
    final head = label.isEmpty ? inset : inset + labelHeight + Space.s;
    final drawsGraph = graph != 'none' && points.length >= 2 && !noData;
    // With no height of its own: room for a value about a title's size
    // over its spark line.
    final own = head + scaler.scale(95);
    final total = math.max(height == null ? own : height! + grown, head + 24);
    final semantics = noData ? 'No data' : '$number$suffix';
    return Semantics(
      container: true,
      label: label,
      value: semantics,
      child: ExcludeSemantics(
        child: SizedBox(
          height: total,
          child: LayoutBuilder(
            builder: (context, constraints) {
              final width = constraints.maxWidth.isFinite
                  ? constraints.maxWidth
                  : 200.0;
              final room = math.max(1.0, total - head);
              // The spark line takes the bottom half, as Grafana's does,
              // shown only where there's room for it.
              final graphHeight = drawsGraph && room >= 40 ? room * 0.5 : 0.0;
              // The value as Grafana sizes it for the room under the
              // title (measured against it from 100 to 300 points), less
              // where the width is short of it; centred in that room, over
              // the spark line.
              final size = noData
                  ? type.body.fontSize!
                  : _fitted(
                      context,
                      number,
                      suffix,
                      width - inset * 2,
                      math.min(room / 1.15, math.max(12, room * 0.55 - 20)),
                    );
              final valueStyle = TextStyle(
                fontFamily: TextStyles.sans,
                fontSize: size,
                height: 1.15,
                fontWeight: FontWeight.w500,
                color: valueColor,
                fontFeatures: const [FontFeature.tabularFigures()],
              );
              final text = noData
                  ? Text(
                      'No data',
                      textScaler: TextScaler.noScaling,
                      style: type.body.copyWith(color: palette.textMuted),
                    )
                  : Text.rich(
                      TextSpan(
                        children: [
                          TextSpan(text: number, style: valueStyle),
                          if (suffix.isNotEmpty)
                            TextSpan(
                              text: suffix,
                              style: valueStyle.copyWith(
                                fontSize: size * _suffixScale,
                              ),
                            ),
                        ],
                      ),
                      maxLines: 1,
                      softWrap: false,
                      // Sized to its room already, at any text size.
                      textScaler: TextScaler.noScaling,
                    );
              return Stack(
                children: [
                  if (filled)
                    Positioned(
                      left: 0,
                      right: 0,
                      top: label.isEmpty ? 0 : head,
                      bottom: 0,
                      child: DecoratedBox(
                        decoration: BoxDecoration(
                          color: colorMode == 'solid' ? shade : null,
                          gradient: colorMode == 'background'
                              ? LinearGradient(colors: _gradient(shade))
                              : null,
                          borderRadius: inCard
                              ? null
                              : BorderRadius.circular(Radii.control),
                        ),
                      ),
                    ),
                  if (graphHeight > 0)
                    Positioned(
                      left: 0,
                      right: 0,
                      bottom: 0,
                      height: graphHeight,
                      child: _Spark(points: points, line: line, area: area),
                    ),
                  if (label.isNotEmpty)
                    Positioned(
                      left: inset,
                      right: inset,
                      top: inset,
                      child: Text(
                        label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: labelStyle,
                      ),
                    ),
                  Positioned(
                    left: inset,
                    right: inset,
                    top: head,
                    bottom: 0,
                    child: Center(
                      child: FittedBox(fit: BoxFit.scaleDown, child: text),
                    ),
                  ),
                ],
              );
            },
          ),
        ),
      ),
    );
  }

  /// A unit after the value is this much of its size, as Grafana's is.
  static const _suffixScale = 0.6;

  /// The value's size (in pixels): [most], or less where [width] is short
  /// of it.
  static double _fitted(
    BuildContext context,
    String number,
    String suffix,
    double width,
    double most,
  ) {
    const probe = 100.0;
    final style = TextStyle(
      fontFamily: TextStyles.sans,
      fontSize: probe,
      height: 1.15,
      fontWeight: FontWeight.w500,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final painter = TextPainter(
      text: TextSpan(
        children: [
          TextSpan(text: number, style: style),
          if (suffix.isNotEmpty)
            TextSpan(
              text: suffix,
              style: style.copyWith(fontSize: probe * _suffixScale),
            ),
        ],
      ),
      textDirection: Directionality.of(context),
      maxLines: 1,
    )..layout();
    final byWidth = probe * math.max(1.0, width) / math.max(1.0, painter.width);
    painter.dispose();
    return math.max(10, math.min(byWidth, most)).floorToDouble();
  }

  /// A background gradient, left to right as Grafana's runs (measured
  /// against it): from the color warmer and darker, through it darker, to it
  /// cooler and a little darker.
  static List<Color> _gradient(Color color) {
    final hsl = HSLColor.fromColor(color);
    HSLColor shade(double spin, double darker) => hsl
        .withHue((hsl.hue + spin + 360) % 360)
        .withLightness((hsl.lightness - darker).clamp(0, 1));
    return [
      shade(8, 0.15).toColor(),
      shade(0, 0.105).toColor(),
      shade(-7, 0.06).toColor(),
    ];
  }
}

/// A Stat's spark line: [points] evenly across, from a little under the
/// lowest to the highest, a 1px [line] over an [area] fill down to the
/// bottom.
class _Spark extends StatelessWidget {
  const _Spark({required this.points, required this.line, required this.area});

  final List<double> points;
  final Color line;
  final Color area;

  @override
  Widget build(BuildContext context) {
    var low = double.infinity;
    var high = double.negativeInfinity;
    for (final point in points) {
      low = math.min(low, point);
      high = math.max(high, point);
    }
    // A tenth of the spread under the lowest point; an all-equal series
    // lies flat along the bottom edge, as Grafana's does, with no fill to
    // see.
    final (bottom, top) = low == high
        ? (low, low + 1)
        : (low - (high - low) * 0.1, high);
    return LineChart(
      duration: Duration.zero,
      LineChartData(
        minX: 0,
        maxX: points.length - 1,
        minY: bottom,
        maxY: top,
        gridData: const FlGridData(show: false),
        borderData: FlBorderData(show: false),
        titlesData: const FlTitlesData(show: false),
        lineTouchData: const LineTouchData(enabled: false),
        clipData: const FlClipData.all(),
        lineBarsData: [
          LineChartBarData(
            spots: [
              for (var i = 0; i < points.length; i++)
                FlSpot(i.toDouble(), points[i]),
            ],
            color: line,
            barWidth: 1,
            isStrokeJoinRound: true,
            dotData: const FlDotData(show: false),
            belowBarData: BarAreaData(show: true, color: area),
          ),
        ],
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
  const AppTableView({
    super.key,
    required this.table,
    this.columns,
    this.height,
  });

  /// Null when the data isn't a table.
  final AppTable? table;

  /// The whole table's height, as Grafana's table panel fills its place:
  /// its heading stays and its rows scroll inside, and fewer rows leave room
  /// under them. As tall as its rows when null.
  final double? height;

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
    final headStyle = type.caption.copyWith(
      color: palette.textFaint,
      fontWeight: FontWeight.w500,
    );
    TextStyle bodyStyle(String format) => type.body.copyWith(
      color: palette.text,
      fontFeatures: format == 'text' ? null : tabular,
    );
    final cells = [
      for (final row in rows)
        [
          for (final column in shown)
            column.index < row.length
                ? cell(row[column.index], column.format)
                : '',
        ],
    ];
    // Each column as wide as its widest cell, or its heading's longest
    // word: a long heading wraps between words (at large text, say)
    // rather than widening the table past its place.
    final scaler = MediaQuery.textScalerOf(context);
    final direction = Directionality.of(context);
    double measured(String text, TextStyle style) {
      final painter = TextPainter(
        text: TextSpan(text: text, style: style),
        textScaler: scaler,
        textDirection: direction,
        maxLines: 1,
      )..layout();
      final width = painter.width;
      painter.dispose();
      return width;
    }

    final heads = [
      for (var c = 0; c < shown.length; c++)
        (c == 0 ? 0 : Space.m) +
            [
              for (final word in shown[c].title.split(RegExp(r'\s+')))
                measured(word, headStyle),
            ].fold(0.0, math.max).ceilToDouble(),
    ];
    final natural = [
      for (var c = 0; c < shown.length; c++)
        math.max(
          heads[c],
          (c == 0 ? 0 : Space.m) +
              [
                for (final row in cells)
                  measured(row[c], bodyStyle(shown[c].format)),
              ].fold(0.0, math.max).ceilToDouble(),
        ),
    ];
    // Where the table is wider than its place, its text columns give way,
    // the widest first, down to a few words (and their heading's longest
    // word), and their long cells end in "…", as Grafana's do; numbers
    // and times keep their width. Past that it scrolls sideways.
    final least = scaler.scale(_leastTextColumn);
    // A letter and "…" at the text size: the least a text column keeps.
    final letter = measured('A…', bodyStyle('text')).ceilToDouble();
    return LayoutBuilder(
      builder: (context, constraints) {
        // Never more than the text columns' share of what the numbers and
        // times leave, so a cut cell shows its "…" and the numbers stay on
        // screen, as Grafana's columns share a phone's width; down to a
        // letter and "…", past which the table scrolls.
        var whole = 0.0;
        var texts = 0;
        for (var c = 0; c < shown.length; c++) {
          if (shown[c].format == 'text') {
            texts++;
          } else {
            whole += natural[c];
          }
        }
        final share = texts == 0 ? 0.0 : (constraints.maxWidth - whole) / texts;
        final widths = _fitColumns(natural, [
          for (var c = 0; c < shown.length; c++)
            shown[c].format == 'text'
                ? math.min(
                    natural[c],
                    math.max(
                      (c == 0 ? 0 : Space.m) + letter,
                      math.min(
                        math.max(heads[c], (c == 0 ? 0 : Space.m) + least),
                        share,
                      ),
                    ),
                  )
                : null,
        ], constraints.maxWidth);
        return _table(
          context,
          shown: shown,
          cells: cells,
          rows: rows,
          left: left,
          more: table.more,
          widths: widths,
          natural: natural,
          padded: padded,
          headStyle: headStyle,
          bodyStyle: bodyStyle,
          caption: caption,
        );
      },
    );
  }

  static double _textWidth(
    String text,
    TextStyle style,
    TextScaler scaler,
    TextDirection direction,
  ) {
    final painter = TextPainter(
      text: TextSpan(text: text, style: style),
      textScaler: scaler,
      textDirection: direction,
      maxLines: 1,
    )..layout();
    final width = painter.width;
    painter.dispose();
    return width;
  }

  /// The least a text column gives way to, at the normal text size.
  static const _leastTextColumn = 96.0;

  /// Each column's width on a line [room] wide: [natural], or where they
  /// don't fit, the ones with a [floor] (text) cut to one level, never
  /// below their floor.
  static List<double> _fitColumns(
    List<double> natural,
    List<double?> floor,
    double room,
  ) {
    final total = natural.fold(0.0, (sum, w) => sum + w);
    if (!room.isFinite || natural.isEmpty) return natural;
    if (total <= room) return _spread(natural, room);
    double at(double level) => [
      for (var c = 0; c < natural.length; c++)
        switch (floor[c]) {
          final least? => math.min(natural[c], math.max(least, level)),
          null => natural[c],
        },
    ].fold(0.0, (sum, w) => sum + w);
    var low = 0.0;
    var high = natural.fold(0.0, math.max);
    for (var k = 0; k < 40; k++) {
      final mid = (low + high) / 2;
      if (at(mid) > room) {
        high = mid;
      } else {
        low = mid;
      }
    }
    return [
      for (var c = 0; c < natural.length; c++)
        switch (floor[c]) {
          final least? => math.min(natural[c], math.max(least, low)),
          null => natural[c],
        },
    ];
  }

  /// Columns narrower than [room] in all spread across it, as Grafana's
  /// share a panel's width: the narrow ones grow to one width, the wide
  /// ones keep theirs, and the last ends at [room]'s edge.
  static List<double> _spread(List<double> natural, double room) {
    double at(double level) =>
        natural.fold(0.0, (sum, w) => sum + math.max(w, level));
    var low = 0.0;
    var high = room;
    for (var k = 0; k < 40; k++) {
      final mid = (low + high) / 2;
      if (at(mid) > room) {
        high = mid;
      } else {
        low = mid;
      }
    }
    final widths = [for (final w in natural) math.max(w, low)];
    // The last takes what rounding leaves, so it meets the edge exactly.
    final before = widths
        .take(widths.length - 1)
        .fold(0.0, (sum, w) => sum + w);
    widths[widths.length - 1] = math.max(natural.last, room - before);
    return widths;
  }

  Widget _table(
    BuildContext context, {
    required List<AppTableColumn> shown,
    required List<List<String>> cells,
    required List<List<Object?>> rows,
    required int left,
    required bool more,
    required List<double> widths,
    required List<double> natural,
    required Widget Function(Widget, int, {bool end}) padded,
    required TextStyle headStyle,
    required TextStyle Function(String) bodyStyle,
    required TextStyle caption,
  }) {
    final palette = Palette.of(context);
    final scaler = MediaQuery.textScalerOf(context);
    final direction = Directionality.of(context);
    // The room for words in column [c]: its width less its gap.
    double room(int c) => widths[c] - (c == 0 ? 0 : Space.m);
    // A heading wraps between words while its longest fits, else keeps
    // one line cut with "…": never broken mid-word.
    Widget heading(int c) {
      final title = shown[c].title;
      final words = title.split(RegExp(r'\s+'));
      final fits = words.every(
        (word) => _textWidth(word, headStyle, scaler, direction) <= room(c),
      );
      return Text(
        fits ? title : cutText(title, headStyle, room(c), scaler, direction),
        semanticsLabel: fits ? null : title,
        maxLines: fits ? null : 1,
        textAlign: shown[c].format == 'number'
            ? TextAlign.end
            : TextAlign.start,
        style: headStyle,
      );
    }

    Widget body(String text, int c) {
      final style = bodyStyle(shown[c].format);
      if (widths[c] >= natural[c]) return Text(text, style: style);
      final cut = cutText(text, style, room(c), scaler, direction);
      return Text(
        cut,
        semanticsLabel: cut == text ? null : text,
        style: style,
        maxLines: 1,
        softWrap: false,
        overflow: TextOverflow.clip,
      );
    }

    final head = TableRow(
      children: [
        for (var c = 0; c < shown.length; c++)
          Semantics(
            header: true,
            child: padded(heading(c), c, end: shown[c].format == 'number'),
          ),
      ],
    );
    final lines = [
      for (final row in cells)
        TableRow(
          children: [
            for (var c = 0; c < shown.length; c++)
              padded(body(row[c], c), c, end: shown[c].format == 'number'),
          ],
        ),
    ];
    final line = BorderSide(color: palette.hairline);
    Table grid(List<TableRow> children, TableBorder border) => Table(
      columnWidths: {
        for (final (c, width) in widths.indexed) c: FixedColumnWidth(width),
      },
      defaultVerticalAlignment: TableCellVerticalAlignment.middle,
      border: border,
      children: children,
    );
    final notes = [
      if (rows.isEmpty) Text('Nothing to show.', style: caption),
      if (left > 0 || more)
        Text(
          left > 0
              ? 'and ${fullNumber(left.toDouble())}${more ? '+' : ''} more'
              : 'and more',
          style: caption,
        ),
    ];
    final height = this.height;
    if (height == null) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        spacing: Space.xs,
        children: [
          SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: grid([head, ...lines], TableBorder(horizontalInside: line)),
          ),
          ...notes,
        ],
      );
    }
    // A height: the heading stays and the rows scroll under it, the two
    // sharing their columns' widths (and sideways scrolling); the notes
    // under them, all within the height.
    return SizedBox(
      height: height,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        spacing: Space.xs,
        children: [
          Flexible(
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  grid(
                    [head],
                    TableBorder(bottom: lines.isEmpty ? BorderSide.none : line),
                  ),
                  Flexible(
                    child: _HandOn(
                      child: SingleChildScrollView(
                        // No bounce at its ends: a drag past them goes on
                        // to the page.
                        physics: const ClampingScrollPhysics(),
                        child: grid(lines, TableBorder(horizontalInside: line)),
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
          ...notes,
        ],
      ),
    );
  }
}

/// Around a scroll inside the page (a table's rows): a drag that goes past
/// its end carries on scrolling the page, and lets it glide on when let go,
/// as a scroll in a scroll does on iOS. Flutter's scrolls don't hand on by
/// themselves, so a phone's page would stay put under a finger on the rows.
class _HandOn extends StatefulWidget {
  const _HandOn({required this.child});

  /// A vertical scroll with clamping physics, which tells of a drag past
  /// its ends.
  final Widget child;

  @override
  State<_HandOn> createState() => _HandOnState();
}

class _HandOnState extends State<_HandOn> {
  /// Whether this drag has scrolled the page.
  bool _handed = false;

  ScrollPosition? get _page =>
      Scrollable.maybeOf(context, axis: Axis.vertical)?.position;

  bool _on(ScrollNotification note) {
    if (note.depth != 0 || note.metrics.axis != Axis.vertical) return false;
    switch (note) {
      case ScrollStartNotification():
        _handed = false;
      case OverscrollNotification(:final overscroll, dragDetails: _?):
        final page = _page;
        if (page == null || !page.hasContentDimensions) break;
        final to = (page.pixels + overscroll).clamp(
          page.minScrollExtent,
          page.maxScrollExtent,
        );
        if (to != page.pixels) {
          page.jumpTo(to);
          _handed = true;
        }
      case ScrollEndNotification(:final dragDetails?) when _handed:
        _handed = false;
        if (_page case final ScrollPositionWithSingleContext page) {
          page.goBallistic(-(dragDetails.primaryVelocity ?? 0));
        }
      default:
    }
    return false;
  }

  @override
  Widget build(BuildContext context) =>
      NotificationListener<ScrollNotification>(
        onNotification: _on,
        child: widget.child,
      );
}
