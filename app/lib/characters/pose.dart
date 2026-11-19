import 'dart:math' as math;

import 'package:flutter/animation.dart';

import 'identity.dart';

/// Everything that moves in a character at one instant. Gaze is in box
/// units; the rest are fractions.
class Pose {
  const Pose({
    this.gazeX = 0,
    this.gazeY = 0,
    this.open = 1,
    this.lid = 0,
    this.droop = 0,
    this.crescent = 0,
    this.sleep = 0,
    this.breath = 1,
    this.scaleLeft = 1,
    this.scaleRight = 1,
  });

  final double gazeX;
  final double gazeY;

  /// How open the eyes are: 1 open, 0 shut mid-blink.
  final double open;

  /// How much of each eye the upper lid covers, from the top.
  final double lid;

  /// How far the upper lids slope down toward the nose (failed).
  final double droop;

  /// How much the lower edge curves up into a smile's crescent (news).
  final double crescent;

  /// How far the eyes have become closed, sleeping arcs.
  final double sleep;

  /// The body's height, as a fraction of its rest: an almost still breath,
  /// with its feet on the ground.
  final double breath;

  /// Each eye's size against its rest: a lid lift, or a widening.
  final double scaleLeft;
  final double scaleRight;

  static const rest = Pose();

  /// Part way ([t]) from this pose to [other].
  Pose lerp(Pose other, double t) {
    double mix(double a, double b) => a + (b - a) * t;
    return Pose(
      gazeX: mix(gazeX, other.gazeX),
      gazeY: mix(gazeY, other.gazeY),
      open: mix(open, other.open),
      lid: mix(lid, other.lid),
      droop: mix(droop, other.droop),
      crescent: mix(crescent, other.crescent),
      sleep: mix(sleep, other.sleep),
      breath: mix(breath, other.breath),
      scaleLeft: mix(scaleLeft, other.scaleLeft),
      scaleRight: mix(scaleRight, other.scaleRight),
    );
  }

  Pose copyWith({
    double? gazeX,
    double? gazeY,
    double? open,
    double? scaleLeft,
    double? scaleRight,
    double? breath,
  }) => Pose(
    gazeX: gazeX ?? this.gazeX,
    gazeY: gazeY ?? this.gazeY,
    open: open ?? this.open,
    lid: lid,
    droop: droop,
    crescent: crescent,
    sleep: sleep,
    breath: breath ?? this.breath,
    scaleLeft: scaleLeft ?? this.scaleLeft,
    scaleRight: scaleRight ?? this.scaleRight,
  );

  @override
  String toString() =>
      'Pose(gaze ${gazeX.toStringAsFixed(3)}, ${gazeY.toStringAsFixed(3)}; '
      'open ${open.toStringAsFixed(2)})';
}

/// Each mood's still pose: what reduced motion shows, and what the moving
/// ones circle around.
Pose stillPose(Mood mood) => switch (mood) {
  Mood.idle => Pose.rest,
  Mood.thinking => const Pose(gazeX: 0.028, gazeY: -0.035),
  Mood.working => const Pose(gazeY: 0.035, scaleLeft: 1.04, scaleRight: 1.04),
  Mood.news => const Pose(crescent: 1),
  Mood.failed => const Pose(gazeY: 0.04, droop: 1, lid: 0.12),
  Mood.noModel => const Pose(lid: 0.45),
  Mood.asleep => const Pose(sleep: 1),
};

/// How long a mood change takes to blend in from whatever showed.
const moodBlend = 0.22;

/// How long the news widening lasts.
const newsFlourish = 0.4;

/// How long a change of shape or face takes.
const shapeMorph = 0.25;

/// How long a blink takes.
const blinkDuration = 0.18;

/// Calm moods (all but thinking and working) move only in short windows
/// on one shared beat: every [calmBeat] seconds, for [calmWindow], some
/// characters blink or glance, each at its own moment within it. Between
/// windows every calm face holds still, so a sidebar of them asks for no
/// frames at all most of the time.
const calmBeat = 3.5;

/// How long each calm window lasts.
const calmWindow = 0.4;

/// How long an idle glance takes to move to its new place.
const glanceDuration = 0.3;

/// Whether [mood] keeps moving all the time (thinking, working) rather
/// than only in the calm windows.
bool isLively(Mood mood) => mood == Mood.thinking || mood == Mood.working;

/// Whether [t] is inside a calm window, when calm faces may move.
bool inCalmWindow(double t) =>
    t - (t / calmBeat).floor() * calmBeat < calmWindow;

/// When the next calm window opens, after [t].
double nextCalmWindow(double t) => ((t / calmBeat).floor() + 1) * calmBeat;

/// The moment a calm face shows at [t]: [t] inside a window, else the end
/// of the last one; it holds still in between.
double _calmTime(double t) {
  final start = (t / calmBeat).floor() * calmBeat;
  return t - start < calmWindow ? t : start + calmWindow;
}

/// [mood]'s pose at [t] seconds on the window's clock, for the character
/// seeded [seed]. Pure: the same arguments always give the same pose. A
/// [compact] character (a reference in a list or bubble) only blinks, with
/// its resting face.
Pose samplePose(Mood mood, double t, int seed, {bool compact = false}) {
  if (compact) {
    return Pose(open: _calmBlink(_calmTime(t), seed, chance: 0.2));
  }
  final still = stillPose(mood);
  if (isLively(mood)) {
    // Each agent's clock runs at its own phase, so a sidebar full of them
    // doesn't blink or glance together.
    final local = t + (seed % 997) * 0.0731;
    return mood == Mood.thinking
        ? _thinking(still, local, seed)
        : _working(still, local, seed);
  }
  final calm = _calmTime(t);
  switch (mood) {
    case Mood.idle:
    case Mood.news:
      final (gx, gy) = _glance(calm, seed);
      return still.copyWith(
        gazeX: gx,
        gazeY: gy,
        open: _calmBlink(calm, seed, chance: 0.35),
        breath: _breath(calm + (seed % 997) * 0.0731),
      );
    case Mood.failed:
      return still.copyWith(open: _calmBlink(calm, seed, chance: 0.3));
    case Mood.noModel:
      // Rarely blinks, and no breath: unavailable, not resting.
      return still.copyWith(open: _calmBlink(calm, seed, chance: 0.17));
    case Mood.asleep:
      return still.copyWith(breath: _breath(calm + (seed % 997) * 0.0731));
    case Mood.thinking:
    case Mood.working:
      return still;
  }
}

/// How far apart two poses look on a character [size] logical pixels
/// across: the most any edge moves, in logical pixels.
double poseDelta(Pose a, Pose b, double size) {
  // Eyes are about a quarter of the body high; lids and blinks move
  // within them.
  final eye = size * 0.3;
  var most = 0.0;
  void edge(double from, double to, double scale) {
    final moved = (from - to).abs() * scale;
    if (moved > most) most = moved;
  }

  edge(a.gazeX, b.gazeX, size);
  edge(a.gazeY, b.gazeY, size);
  edge(a.breath, b.breath, size);
  edge(a.open, b.open, eye);
  edge(a.lid, b.lid, eye);
  edge(a.droop, b.droop, eye);
  edge(a.crescent, b.crescent, eye);
  edge(a.sleep, b.sleep, eye);
  edge(a.scaleLeft, b.scaleLeft, eye);
  edge(a.scaleRight, b.scaleRight, eye);
  return most;
}

/// A repeatable random number in [0, 1) for [seed]'s [k]th slot of [salt].
double _random(int seed, int k, int salt) {
  var h = (seed ^ (k * 0x9e3779b1) ^ (salt * 0x85ebca6b)) & 0xffffffff;
  h = ((h ^ (h >> 16)) * 0x7feb352d) & 0xffffffff;
  h = ((h ^ (h >> 15)) * 0x846ca68b) & 0xffffffff;
  h ^= h >> 16;
  return (h & 0xffffff) / 0x1000000;
}

double _offset(int seed, int k, double from, double to) =>
    from + (to - from) * _random(seed, k, 1);

/// Eye openness at [t]: one blink in every [cell] seconds, at a random
/// moment between [from] and [to] into it, so blinks come 3–7s apart.
double _blink(
  double t,
  int seed, {
  double cell = 5,
  double from = 1.5,
  double to = 3.5,
}) {
  final k = (t / cell).floor();
  final start = k * cell + _offset(seed, k, from, to);
  final u = (t - start) / blinkDuration;
  if (u <= 0 || u >= 1) return 1;
  // Closes and opens smoothly; never a one-frame cut.
  return 1 - math.sin(math.pi * u);
}

/// The almost still seated breath: ±0.5% over 6s.
double _breath(double t) => 1 + 0.005 * math.sin(2 * math.pi * t / 6);

/// Eye openness at calm moment [t]: in each calm window this character
/// blinks with [chance], at its own moment within the window.
double _calmBlink(double t, int seed, {required double chance}) {
  final k = (t / calmBeat).floor();
  if (_random(seed, k, 5) >= chance) return 1;
  final start =
      k * calmBeat + _random(seed, k, 6) * (calmWindow - blinkDuration);
  final u = (t - start) / blinkDuration;
  if (u <= 0 || u >= 1) return 1;
  // Closes and opens smoothly; never a one-frame cut.
  return 1 - math.sin(math.pi * u);
}

/// Every this many beats a character glances at least once, so where it
/// looks is never more than a few windows back.
const _glanceAtLeast = 8;

/// Whether this character glances in calm window [k], and when.
double? _glanceStart(int seed, int k) {
  final glances = (k + seed) % _glanceAtLeast == 0 || _random(seed, k, 7) < 0.3;
  if (!glances) return null;
  return k * calmBeat + _random(seed, k, 8) * (calmWindow - glanceDuration);
}

/// Where a glance in window [k] looks: within ±4% of the body across,
/// ±3% up and down.
(double, double) _glanceTarget(int seed, int k) => (
  (_random(seed, k, 3) * 2 - 1) * 0.04,
  (_random(seed, k, 4) * 2 - 1) * 0.03,
);

/// The latest glance that has started by calm moment [t], at or before
/// window [k]: its window, or null if none is near.
int? _lastGlance(int seed, int k, double t) {
  for (var j = k; j > k - _glanceAtLeast - 1; j--) {
    final start = _glanceStart(seed, j);
    if (start != null && start <= t) return j;
  }
  return null;
}

/// Idle gaze at calm moment [t]: held where the last glance went, easing
/// to each new place in [glanceDuration].
(double, double) _glance(double t, int seed) {
  final k = (t / calmBeat).floor();
  final last = _lastGlance(seed, k, t);
  if (last == null) return (0, 0);
  final (tx, ty) = _glanceTarget(seed, last);
  final before = _lastGlance(seed, last - 1, double.infinity);
  final (fx, fy) = before == null ? (0.0, 0.0) : _glanceTarget(seed, before);
  final u = Curves.easeInOut.transform(
    ((t - _glanceStart(seed, last)!) / glanceDuration).clamp(0.0, 1.0),
  );
  return (fx + (tx - fx) * u, fy + (ty - fy) * u);
}

/// Thinking: look up and to the side, hold, come back, every 3s; one lid
/// lifts a little, the other side next time.
Pose _thinking(Pose still, double t, int seed) {
  const cycle = 3.0;
  final k = (t / cycle).floor();
  final u = t - k * cycle;
  final double away;
  if (u < 0.6) {
    away = Curves.easeInOut.transform(u / 0.6);
  } else if (u < 1.8) {
    away = 1;
  } else if (u < 2.4) {
    away = 1 - Curves.easeInOut.transform((u - 1.8) / 0.6);
  } else {
    away = 0;
  }
  final lift = 1 + 0.1 * away;
  final left = k.isEven;
  return still.copyWith(
    gazeX: 0.006 + (still.gazeX - 0.006) * away,
    gazeY: -0.01 + (still.gazeY + 0.01) * away,
    open: _blink(t, seed),
    scaleLeft: left ? lift : 1,
    scaleRight: left ? 1 : lift,
  );
}

/// Working: eyes on the work, shifting between two spots every 1.25s.
Pose _working(Pose still, double t, int seed) {
  const cycle = 2.5;
  final u = t % cycle;
  double toward(double from, double to, double start) =>
      from +
      (to - from) *
          Curves.easeInOut.transform(((u - start) / 0.3).clamp(0.0, 1.0));
  final x = u < 1.25 ? toward(0.02, -0.02, 0) : toward(-0.02, 0.02, 1.25);
  return still.copyWith(gazeX: x, open: _blink(t, seed));
}

/// One agent's face over time: its mood, and a blend from whatever showed
/// when the mood last changed, so a quick thinking → working → thinking
/// stays continuous.
class MoodTrack {
  MoodTrack({required this.seed, required this.mood, this.compact = false});

  final int seed;

  /// A reference's face: resting, only blinking, whatever the mood.
  final bool compact;
  Mood mood;

  Pose? _from;
  double _since = -1e9;
  double _flourish = -1e9;
  double _reshape = -1e9;
  double? _cachedAt;
  Pose _cached = Pose.rest;

  /// Shows [next] from [t] on. The first mood a track gets just shows; a
  /// later change blends in from the pose on screen, and arriving news
  /// widens the eyes once, if it's [fresh] (it just happened, rather than
  /// while the app wasn't looking).
  void change(Mood next, double t, {bool fresh = true}) {
    if (next == mood || compact) return;
    _from = pose(t);
    _since = t;
    if (next == Mood.news && fresh) _flourish = t;
    mood = next;
    _cachedAt = null;
  }

  /// The shape or face changed at [t]: a blink hides the switch while the
  /// body eases into its new shape over [shapeMorph].
  void reshape(double t) {
    if (t - _reshape < shapeMorph) return;
    _reshape = t;
    _cachedAt = null;
  }

  /// Whether a blend, flourish or reshape is still under way at [t].
  bool settling(double t) =>
      t - _since < moodBlend ||
      t - _flourish < newsFlourish ||
      t - _reshape < shapeMorph;

  /// The pose at [t], sampled once per moment however many copies show it.
  Pose pose(double t, {bool still = false}) {
    if (still) return compact ? Pose.rest : stillPose(mood);
    if (_cachedAt == t) return _cached;
    var pose = samplePose(mood, t, seed, compact: compact);
    final from = _from;
    final blend = (t - _since) / moodBlend;
    if (from != null && blend < 1) {
      pose = from.lerp(pose, Curves.easeOutCubic.transform(blend.clamp(0, 1)));
    }
    // A new shape or face: a blink as it starts.
    final reshape = t - _reshape;
    if (reshape >= 0 && reshape < blinkDuration) {
      pose = pose.copyWith(
        open: pose.open * (1 - math.sin(math.pi * reshape / blinkDuration)),
      );
    }
    // News arriving: a soft blink, then the eyes widen once and settle.
    final flourish = t - _flourish;
    if (flourish >= 0 && flourish < blinkDuration) {
      pose = pose.copyWith(
        open: pose.open * (1 - math.sin(math.pi * flourish / blinkDuration)),
      );
    } else if (flourish >= blinkDuration && flourish < newsFlourish) {
      final u = (flourish - blinkDuration) / (newsFlourish - blinkDuration);
      final widen = 1 + 0.12 * math.sin(math.pi * u);
      pose = pose.copyWith(
        scaleLeft: pose.scaleLeft * widen,
        scaleRight: pose.scaleRight * widen,
      );
    }
    _cachedAt = t;
    _cached = pose;
    return pose;
  }
}
