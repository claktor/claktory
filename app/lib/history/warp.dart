// How history time maps to playing time (#458): busy stretches play at the
// chosen speed, and quiet ones (no event for over 20 minutes) are squeezed
// to at most 0.4 s, as Gource's auto-skip does. A piecewise-linear map,
// rebuilt as events load; the playhead keeps its place in history time.
import 'dart:math' as math;
import 'dart:typed_data';

/// How fast history plays: so much history every 10 seconds, or the whole
/// of it in about a minute.
enum HistorySpeed {
  auto('Whole history in a minute', 'Auto', 0),
  hour('An hour every 10 seconds', '1 h', Duration.millisecondsPerHour),
  sixHours('6 hours every 10 seconds', '6 h', 6 * Duration.millisecondsPerHour),
  day('A day every 10 seconds', '1 day', Duration.millisecondsPerDay),
  week('A week every 10 seconds', '1 week', 7 * Duration.millisecondsPerDay);

  const HistorySpeed(this.title, this.short, this.per10s);

  final String title;

  /// For the speed control: "1 h" (per 10 s).
  final String short;
  final int per10s;
}

/// A quiet stretch, squeezed: from [from] to [to] (history ms).
class Gap {
  const Gap(this.from, this.to);

  final double from;
  final double to;
  double get length => to - from;
}

/// Longer than this with nothing happening is squeezed.
const quietGap = 20 * Duration.millisecondsPerMinute;

/// What a quiet stretch plays in at most, in seconds.
const gapPlay = 0.4;

/// The whole history in about this many seconds, at Auto.
const autoSeconds = 60.0;

/// Auto's limits: 2 minutes to a week of history a second.
const autoSlowest = 2.0 * Duration.millisecondsPerMinute;
const autoFastest = 7.0 * Duration.millisecondsPerDay;

class TimeWarp {
  TimeWarp._(this._h, this._p, this.gaps, this.rate);

  /// The map for history from [start] to [end] at [speed]. [times] are the
  /// events loaded (in order), in the [loaded] stretches (from, to), or up
  /// to [loadedUntil]; between them, the hourly [histogram] (from
  /// [histogramStart]) says where the quiet hours are.
  factory TimeWarp.build({
    required double start,
    required double end,
    required HistorySpeed speed,
    Float64List? times,
    double loadedUntil = double.negativeInfinity,
    List<(double, double)>? loaded,
    Uint32List? histogram,
    double histogramStart = 0,
    double seconds = autoSeconds,
    bool limited = true,
  }) {
    final gaps = <Gap>[];
    final events = times ?? Float64List(0);
    // The last moment something happened, or may have.
    var previous = start;
    void busy(double from, double to) {
      if (from - previous > quietGap) gaps.add(Gap(previous, from));
      previous = math.max(previous, to);
    }

    // Not loaded: whole quiet hours, or all of it when nothing says.
    void unknown(double from, double to) {
      if (to <= from) return;
      if (histogram == null) {
        busy(from, to);
        return;
      }
      const hour = Duration.millisecondsPerHour;
      for (var k = 0; k < histogram.length; k++) {
        final a = histogramStart + k * hour;
        final b = a + hour;
        if (b <= from) continue;
        if (a >= to) break;
        if (histogram[k] > 0) busy(math.max(a, from), math.min(b, to));
      }
    }

    var at = start;
    var e = 0;
    for (final (from, to)
        in loaded ?? [(double.negativeInfinity, loadedUntil)]) {
      final a = math.max(from, start);
      final b = math.min(to, end);
      if (b < a) continue;
      if (a > at) unknown(at, a);
      while (e < events.length && events[e] < a) {
        e++;
      }
      for (; e < events.length && events[e] <= b; e++) {
        busy(events[e], events[e]);
      }
      at = math.max(at, b);
    }
    if (at < end) unknown(at, end);
    if (end - previous > quietGap) gaps.add(Gap(previous, end));
    final total = math.max(0.0, end - start);
    final quiet = gaps.fold(0.0, (sum, gap) => sum + gap.length);
    // Auto: the busy part in [seconds], within Auto's limits unless not
    // [limited] (a video, whatever its history's length).
    final even = math.max(1.0, total - quiet) / seconds;
    final rate = speed != HistorySpeed.auto
        ? speed.per10s / 10
        : limited
        ? even.clamp(autoSlowest, autoFastest)
        : even;
    final h = <double>[start];
    final p = <double>[0];
    for (final gap in gaps) {
      final at = p.last + (gap.from - h.last) / rate;
      h.add(gap.from);
      p.add(at);
      h.add(gap.to);
      p.add(at + math.min(gap.length / rate, gapPlay));
    }
    if (end > h.last || h.length == 1) {
      p.add(p.last + (end - h.last) / rate);
      h.add(math.max(end, h.last));
    }
    return TimeWarp._(
      Float64List.fromList(h),
      Float64List.fromList(p),
      gaps,
      rate,
    );
  }

  final Float64List _h;
  final Float64List _p;

  /// The squeezed stretches, in order.
  final List<Gap> gaps;

  /// History ms a second plays, outside the quiet stretches.
  final double rate;

  double get start => _h.first;
  double get end => _h.last;

  /// How long the whole history plays, in seconds.
  double get duration => _p.last;

  /// When [h] (history ms) plays, in seconds from the start.
  double playAt(double h) {
    if (h <= _h.first) return 0;
    if (h >= _h.last) return _p.last + (h - _h.last) / rate;
    final i = _segment(_h, h);
    return _lerp(_h[i], _h[i + 1], _p[i], _p[i + 1], h);
  }

  /// What history time plays at [p] seconds.
  double historyAt(double p) {
    if (p <= 0) return _h.first;
    if (p >= _p.last) return _h.last + (p - _p.last) * rate;
    final i = _segment(_p, p);
    return _lerp(_p[i], _p[i + 1], _h[i], _h[i + 1], p);
  }

  /// The quiet stretch [h] is in or came out of most recently, or null.
  Gap? gapBefore(double h) {
    Gap? found;
    for (final gap in gaps) {
      if (gap.from >= h) break;
      found = gap;
    }
    return found;
  }

  static double _lerp(double a0, double a1, double b0, double b1, double a) =>
      a1 == a0 ? b1 : b0 + (b1 - b0) * (a - a0) / (a1 - a0);

  /// The i with list[i] <= x < list[i + 1].
  static int _segment(Float64List list, double x) {
    var lo = 0;
    var hi = list.length - 1;
    while (hi - lo > 1) {
      final mid = (lo + hi) >> 1;
      if (list[mid] <= x) {
        lo = mid;
      } else {
        hi = mid;
      }
    }
    return lo;
  }
}
