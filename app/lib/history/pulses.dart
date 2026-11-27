// What travels on History's board at a moment (#458): each message a
// pulse along its route, worked out afresh from the events for every
// frame, so scrubbing and playing show the same thing. Over the budget,
// pulses on the same route merge into one brighter pulse with a count.
import 'dart:math' as math;
import 'dart:ui' show Offset, Rect;

import 'routes.dart';
import 'scene.dart';
import 'timeline.dart';

/// Pulses in flight at most: on a Mac or iPad, and on a phone.
const pulseBudget = 400;
const phonePulseBudget = 150;

/// How long a pulse takes, in seconds of playing: longer for longer
/// routes.
double pulseSeconds(double length) => 0.5 + 0.3 * math.min(1.0, length / 1200);
const _longest = 0.8;

/// How long a tick on a chip lasts, in seconds.
const tickSeconds = 0.4;

/// What a pulse is: between agents, from the person, to the person (what
/// waits for their answer), through a pipe, or from an app.
enum PulseKind { agent, person, asks, pipe, app }

class Pulse {
  Pulse(
    this.route,
    this.at,
    this.kind, {
    this.quiet = false,
    this.count = 1,
    this.pad = -1,
  });

  /// The pipe's pad it goes through (an index into the scene's pads), or
  /// -1.
  final int pad;

  final Route route;

  /// How far along it is, 0 to 1.
  final double at;
  final PulseKind kind;
  final bool quiet;

  /// How many messages it stands for.
  final int count;

  Offset get point => route.at(at);
}

/// A task, monitor or Claktory event on a chip: [age] 0 to 1.
class Tick {
  const Tick(this.chip, this.age);

  final int chip;
  final double age;
}

/// The pulses and ticks at history time [t].
({List<Pulse> pulses, List<Tick> ticks}) pulsesAt({
  required HistoryScene scene,
  required HistoryLog log,
  required Routes routes,
  required double Function(double history) playAt,
  required double Function(double play) historyAt,
  required double t,
  int budget = pulseBudget,
  String? project,
}) {
  final now = playAt(t);
  final from = log.lowerBound(historyAt(now - _longest));
  final until = log.upperBound(t);
  final you = scene.layout.you;
  final ticks = <Tick>[];

  // With one project shown: its box, where what's outside it comes in or
  // goes out.
  final box = project == null ? -1 : scene.projectBox(project);
  final boxRect = box < 0 ? null : scene.boxes[box].box.rect;

  // Where an event's node is now: a chip, the person or a pad (then its
  // index, else -1); outside the project shown, the point on its box's edge
  // toward it ("o…").
  _Place? place(int node) {
    if (node == 0) return you == null ? null : ('you', you, -1, -1);
    _Place outside(String key, Rect rect) {
      final edge = Routes.edgeToward(boxRect!, rect.center);
      return (
        'o$key',
        Rect.fromCenter(center: edge, width: 0, height: 0),
        -1,
        -1,
      );
    }

    final pad = scene.padOf(node);
    if (pad >= 0) {
      if (project != null && scene.pads[pad].box != box) {
        return boxRect == null ? null : outside('p$pad', scene.pads[pad].rect);
      }
      return ('p$pad', scene.pads[pad].rect, -1, pad);
    }
    final chip = scene.chipAt(node, t);
    if (chip < 0) return null;
    if (project != null && !scene.chipInProject(chip, t, project)) {
      return boxRect == null ? null : outside('c$chip', scene.chips[chip].rect);
    }
    return ('c$chip', scene.chips[chip].rect, chip, -1);
  }

  // At a year or more played in a minute, tens of thousands of messages
  // are under way at once, between the same few hundred pairs: where each
  // node is, each pair's route, its length and its line, and the pulses
  // each route's messages merge into, are found once a frame (Polpo's
  // history-scale.md items 3 and 4), in lists by node rather than maps,
  // with no pulse made for each message. Every message still counts, for
  // the merged pulses' numbers.
  final nodes = scene.timeline.nodes.length + 1;
  final placed = List<_Place?>.filled(nodes, null);
  final known = List<bool>.filled(nodes, false);
  final placedLate = <int, _Place?>{};
  _Place? placeOf(int node) {
    if (node < 0 || node >= nodes) {
      return placedLate.containsKey(node)
          ? placedLate[node]
          : (placedLate[node] = place(node));
    }
    if (known[node]) return placed[node];
    known[node] = true;
    return placed[node] = place(node);
  }

  final ticking = <int, int>{};
  int tickChip(int node) => ticking[node] ??= () {
    final chip = scene.chipAt(node, t);
    if (chip < 0) return -1;
    if (project != null && !scene.chipInProject(chip, t, project)) return -1;
    return chip;
  }();
  // By where its ends are: legs between the same places share one.
  final routed = <(String, String, bool), _Tracked>{};
  final pulses = _Merging(budget);
  final legs = List<_Leg?>.filled(nodes * nodes, null);
  final legsLate = <int, _Leg>{};
  _Leg leg(int sender, int recipient) {
    final a = placeOf(sender);
    final b = placeOf(recipient);
    // Neither end in the project shown: not its message.
    if (a == null ||
        b == null ||
        b.$1 == a.$1 ||
        (a.$1.startsWith('o') && b.$1.startsWith('o'))) {
      return _Leg.none;
    }
    final pad = a.$4 >= 0 && b.$3 >= 0
        ? a.$4
        : b.$4 >= 0 && a.$3 >= 0
        ? b.$4
        : -1;
    if (a.$3 >= 0 && b.$3 >= 0) {
      return _Leg(
        pad,
        routed[(a.$1, b.$1, false)] ??= pulses.track(routes.across(a.$3, b.$3)),
        along: () => routed[(a.$1, b.$1, true)] ??= pulses.track(
          routes.chips(a.$3, b.$3),
        ),
        lines: scene.timeline.messagingSpans(sender, recipient),
      );
    }
    return _Leg(
      pad,
      routed[(a.$1, b.$1, false)] ??= pulses.track(_route(routes, a, b)),
    );
  }

  _Leg legOf(int sender, int recipient) {
    if (sender >= 0 && sender < nodes && recipient >= 0 && recipient < nodes) {
      final k = sender * nodes + recipient;
      return legs[k] ??= leg(sender, recipient);
    }
    return legsLate[sender * 0x100000 + recipient] ??= leg(sender, recipient);
  }

  for (var i = from; i < until; i++) {
    final time = log.time(i);
    final age = now - playAt(time);
    if (age < 0) continue;
    final kind = log.kind(i);
    final count = log.toCount(i);
    if (kind.tick) {
      if (age >= tickSeconds) continue;
      for (var k = 0; k < count; k++) {
        final chip = tickChip(log.to(i, k));
        if (chip >= 0) ticks.add(Tick(chip, age / tickSeconds));
      }
      continue;
    }
    // The longest way takes _longest seconds: one older than that is on
    // none (each route's own seconds decide the rest).
    if (age >= _longest) continue;
    final sender = log.from(i);
    if (placeOf(sender) == null) continue;
    final style = switch (kind) {
      EventKind.person => PulseKind.person,
      EventKind.pipeIn || EventKind.pipeOut => PulseKind.pipe,
      EventKind.app => PulseKind.app,
      _ when kind.asks => PulseKind.asks,
      _ => PulseKind.agent,
    };
    final quiet = log.flags(i) & eventQuiet != 0;
    for (var k = 0; k < count; k++) {
      final way = legOf(sender, log.to(i, k));
      final straight = way.route;
      if (straight == null) continue;
      // Along their line, or straight across when none is known then.
      final route = way.along != null && way.lined(time)
          ? way.alongRoute
          : straight;
      final at = age / route.seconds;
      if (at >= 1) continue;
      pulses.add(route, at, style, quiet, way.pad);
    }
  }
  return (pulses: pulses.done(), ticks: ticks);
}

typedef _Place = (String, Rect, int, int);

/// A route as a frame's pulses use it: its index among the frame's routes,
/// for their bins, and how long a pulse takes along it.
class _Tracked {
  _Tracked(this.route, this.id) : seconds = pulseSeconds(route.length);

  final Route route;
  final int id;
  final double seconds;
}

/// Messages from one node to another this frame: their route, straight
/// across between two chips, and along their line ([lines]' spans) when
/// they have one then; or none, when nothing is drawn for them.
class _Leg {
  _Leg(this.pad, this.route, {this.along, this.lines = const []});
  _Leg._none() : pad = -1, route = null, along = null, lines = const [];

  static final none = _Leg._none();

  final int pad;
  final _Tracked? route;
  final _Tracked Function()? along;
  late final _Tracked alongRoute = along!();
  final List<List<Span>> lines;

  /// Since and until when [_lined] holds.
  var _from = double.infinity;
  var _until = double.negativeInfinity;
  var _lined = false;

  /// Whether they had a line at [t]: worked out again only past the stretch
  /// the last answer holds for, as a frame's times come in order.
  bool lined(double t) {
    if (t >= _from && t < _until) return _lined;
    var from = double.negativeInfinity;
    var until = double.infinity;
    var lined = false;
    for (final spans in lines) {
      for (final span in spans) {
        if (span.contains(t)) {
          if (!lined) {
            lined = true;
            from = span.from;
            until = span.to;
          } else {
            from = math.min(from, span.from);
            until = math.max(until, span.to);
          }
        } else if (!lined) {
          if (span.to <= t) from = math.max(from, span.to);
          if (span.from > t) until = math.min(until, span.from);
        }
      }
    }
    _from = from;
    _until = until;
    return _lined = lined;
  }
}

/// The route between [sender] and [recipient] (as pulsesAt places them:
/// key, rect, chip, pad) when they aren't two chips.
Route _route(
  Routes routes,
  (String, Rect, int, int) sender,
  (String, Rect, int, int) recipient,
) {
  if (recipient.$3 >= 0 && sender.$4 >= 0) {
    return routes.pipe(sender.$4, recipient.$3, toChip: true);
  }
  if (sender.$3 >= 0 && recipient.$4 >= 0) {
    return routes.pipe(recipient.$4, sender.$3, toChip: false);
  }
  if (recipient.$3 >= 0) {
    return routes.place(sender.$1, sender.$2, recipient.$3, toChip: true);
  }
  if (sender.$3 >= 0) {
    return routes.place(recipient.$1, recipient.$2, sender.$3, toChip: false);
  }
  return Routes.arc(sender.$2, recipient.$2);
}

/// [pulses] within [budget]: those on the same route close together merge,
/// ever more widely until they fit; past that, the busiest routes' only.
List<Pulse> merge(List<Pulse> pulses, int budget) {
  final merging = _Merging(budget);
  final tracked = Map<Route, _Tracked>.identity();
  for (final pulse in pulses) {
    merging.add(
      tracked[pulse.route] ??= merging.track(pulse.route),
      pulse.at,
      pulse.kind,
      pulse.quiet,
      pulse.pad,
      count: pulse.count,
    );
  }
  return merging.done(pulses.length <= budget ? pulses : null);
}

/// Pulses merging as they come: each kept as it is while they fit in
/// [budget], and in bins by route, a quarter of the way along and kind
/// meanwhile, so wider bins are those added up.
class _Merging {
  _Merging(this.budget);

  final int budget;
  final _pulses = <Pulse>[];
  var _added = 0;
  var _routes = 0;

  /// Bins by key ((route * 4 + quarter) * kinds + kind), and their keys in
  /// the order each was made.
  final _bins = <_Bin?>[];
  final _order = <int>[];

  static final _kinds = PulseKind.values.length;

  /// [route], numbered for this frame's bins.
  _Tracked track(Route route) => _Tracked(route, _routes++);

  void add(
    _Tracked route,
    double at,
    PulseKind kind,
    bool quiet,
    int pad, {
    int count = 1,
  }) {
    final seq = _added++;
    if (seq < budget + 1 && seq == _pulses.length) {
      _pulses.add(
        Pulse(route.route, at, kind, quiet: quiet, count: count, pad: pad),
      );
    }
    // at is 0 to 1: a quarter is 0 to 3.
    final key = (route.id * 4 + (at / 0.25).floor()) * _kinds + kind.index;
    if (key >= _bins.length) {
      _bins.length = math.max(key + 1, _bins.length * 2);
    }
    var bin = _bins[key];
    if (bin == null) {
      bin = _bins[key] = _Bin(route.route, kind, seq);
      _order.add(key);
    }
    bin.add(at, quiet, pad, count, seq);
  }

  /// The pulses: [all] (or each as added) when they fit.
  List<Pulse> done([List<Pulse>? all]) {
    if (_added <= budget) return all ?? _pulses;
    var bins = [for (final key in _order) _bins[key]!];
    for (final quarters in [1, 2, 4]) {
      if (quarters > 1) {
        // Bins in the order their first pulse came, as each wider one is.
        final wider = <int, _Bin>{};
        final kinds = _kinds;
        for (final key in _order) {
          final bin = _bins[key]!;
          final quarter = key ~/ kinds % 4;
          final id = key ~/ kinds ~/ 4;
          final at = (id * 4 + quarter ~/ quarters) * kinds + key % kinds;
          (wider[at] ??= _Bin(bin.route, bin.kind, bin.first)).join(bin);
        }
        bins = wider.values.toList();
      }
      if (bins.length <= budget) return [for (final bin in bins) bin.pulse];
    }
    final out = [for (final bin in bins) bin.pulse]
      ..sort((a, b) => b.count.compareTo(a.count));
    return out.sublist(0, budget);
  }
}

class _Bin {
  _Bin(this.route, this.kind, this.first);

  final Route route;
  final PulseKind kind;

  /// When its first pulse came, and its lead's (the furthest along).
  final int first;
  var _lead = -1;
  var _at = 0.0;
  var _pad = -1;
  var _count = 0;
  var _quiet = true;

  void add(double at, bool quiet, int pad, int count, int seq) {
    _count += count;
    _quiet &= quiet;
    if (_lead < 0 || at > _at || (at == _at && seq < _lead)) {
      _lead = seq;
      _at = at;
      _pad = pad;
    }
  }

  void join(_Bin bin) {
    _count += bin._count - 1;
    add(bin._at, bin._quiet, bin._pad, 1, bin._lead);
  }

  Pulse get pulse =>
      Pulse(route, _at, kind, quiet: _quiet, count: _count, pad: _pad);
}
