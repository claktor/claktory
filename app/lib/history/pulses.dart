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
  final pulses = <Pulse>[];
  final ticks = <Tick>[];

  // With one project shown: its box, where what's outside it comes in or
  // goes out.
  final box = project == null ? -1 : scene.projectBox(project);
  final boxRect = box < 0 ? null : scene.boxes[box].box.rect;

  // Where an event's node is now: a chip, the person or a pad (then its
  // index, else -1); outside the project shown, the point on its box's edge
  // toward it ("o…").
  (String, Rect, int, int)? place(int node) {
    if (node == 0) return you == null ? null : ('you', you, -1, -1);
    (String, Rect, int, int) outside(String key, Rect rect) {
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

  for (var i = from; i < until; i++) {
    final age = now - playAt(log.time(i));
    if (age < 0) continue;
    final kind = log.kind(i);
    if (kind.tick) {
      if (age >= tickSeconds) continue;
      for (var k = 0; k < log.toCount(i); k++) {
        final chip = scene.chipAt(log.to(i, k), t);
        if (chip < 0) continue;
        if (project != null && !scene.chipInProject(chip, t, project)) continue;
        ticks.add(Tick(chip, age / tickSeconds));
      }
      continue;
    }
    final sender = place(log.from(i));
    if (sender == null) continue;
    final style = switch (kind) {
      EventKind.person => PulseKind.person,
      EventKind.pipeIn || EventKind.pipeOut => PulseKind.pipe,
      EventKind.app => PulseKind.app,
      _ when kind.asks => PulseKind.asks,
      _ => PulseKind.agent,
    };
    final quiet = log.flags(i) & eventQuiet != 0;
    for (var k = 0; k < log.toCount(i); k++) {
      final to = log.to(i, k);
      final recipient = place(to);
      if (recipient == null || recipient.$1 == sender.$1) continue;
      // Neither end in the project shown: not its message.
      if (sender.$1.startsWith('o') && recipient.$1.startsWith('o')) continue;
      final Route route;
      if (sender.$3 >= 0 && recipient.$3 >= 0) {
        // Along their line, or straight across when none is known then.
        route = scene.timeline.messagingAt(log.from(i), to, log.time(i))
            ? routes.chips(sender.$3, recipient.$3)
            : routes.across(sender.$3, recipient.$3);
      } else if (recipient.$3 >= 0 && sender.$4 >= 0) {
        route = routes.pipe(sender.$4, recipient.$3, toChip: true);
      } else if (sender.$3 >= 0 && recipient.$4 >= 0) {
        route = routes.pipe(recipient.$4, sender.$3, toChip: false);
      } else if (recipient.$3 >= 0) {
        route = routes.place(sender.$1, sender.$2, recipient.$3, toChip: true);
      } else if (sender.$3 >= 0) {
        route = routes.place(
          recipient.$1,
          recipient.$2,
          sender.$3,
          toChip: false,
        );
      } else {
        route = Routes.arc(sender.$2, recipient.$2);
      }
      final at = age / pulseSeconds(route.length);
      if (at >= 1) continue;
      final pad = sender.$4 >= 0 && recipient.$3 >= 0
          ? sender.$4
          : recipient.$4 >= 0 && sender.$3 >= 0
          ? recipient.$4
          : -1;
      pulses.add(Pulse(route, at, style, quiet: quiet, pad: pad));
    }
  }
  return (pulses: merge(pulses, budget), ticks: ticks);
}

/// [pulses] within [budget]: those on the same route close together merge,
/// ever more widely until they fit; past that, the busiest routes' only.
List<Pulse> merge(List<Pulse> pulses, int budget) {
  if (pulses.length <= budget) return pulses;
  var out = pulses;
  for (final width in [0.25, 0.5, 1.0]) {
    final bins = <(Route, int, PulseKind), _Bin>{};
    for (final pulse in pulses) {
      final key = (pulse.route, (pulse.at / width).floor(), pulse.kind);
      (bins[key] ??= _Bin()).add(pulse);
    }
    out = [for (final bin in bins.values) bin.pulse];
    if (out.length <= budget) return out;
  }
  out.sort((a, b) => b.count.compareTo(a.count));
  return out.sublist(0, budget);
}

class _Bin {
  Pulse? _lead;
  var _count = 0;
  var _quiet = true;

  void add(Pulse pulse) {
    _count += pulse.count;
    _quiet &= pulse.quiet;
    final lead = _lead;
    if (lead == null || pulse.at > lead.at) _lead = pulse;
  }

  Pulse get pulse {
    final lead = _lead!;
    return Pulse(
      lead.route,
      lead.at,
      lead.kind,
      quiet: _quiet,
      count: _count,
      pad: lead.pad,
    );
  }
}
