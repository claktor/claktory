import 'dart:async';

import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart' show LiveClock;
import 'identity.dart';
import 'pose.dart';

/// Faces bigger than [smallFace] move at most this often.
const characterFps = 30;

/// Small faces, and every calm one, move at most this often.
const smallFaceFps = 20;

/// The biggest face that counts as small: the sidebar's and Messaging's.
const smallFace = 18.0;

/// A face is redrawn once its pose has moved at least this far, in logical
/// pixels; anything less wouldn't show, so it isn't worth a frame.
const minimumStep = 0.1;

/// The window's one clock for every character in it: a timer, at most
/// [characterFps], that redraws only the characters on screen whose faces
/// have visibly moved since they were drawn.
///
/// Its time is its own, in seconds, advanced by its ticks, so every copy
/// of a face shares one pose and a tick that changes nothing visible asks
/// for no frame at all: an idle sidebar costs a frame only while an eye
/// actually moves.
///
/// Calm faces (idle, news, failed, no model, asleep, and every reference)
/// move only in the short calm windows of one shared beat (see
/// [calmBeat]), so between them the clock sleeps and the window draws
/// nothing; thinking and working faces move all the time.
///
/// A character subscribes while it's mounted. A subscriber asked to repaint
/// that isn't painted in the frame that follows is off screen (scrolled
/// away, behind something, in a folded list) and goes dormant until
/// something paints it again. With nothing to move, the clock stops; if
/// only compact characters are showing, it sleeps until the next blink.
/// Reduced motion, a muted subtree ([TickerMode]) or the app hidden stops
/// it. While [LiveClock] runs (something working on screen), it moves on
/// that clock's ticks instead of its own timer, at that clock's rate, so
/// faces and waves move in the same frames.
class CharacterClock {
  CharacterClock() {
    LiveClock.instance.running.addListener(_schedule);
  }

  final _subscribers = <CharacterSubscription>{};
  final _tracks = <String, MoodTrack>{};
  Timer? _ticks;
  Timer? _sleep;

  /// Moving on [LiveClock]'s ticks, while it runs, instead of [_ticks]:
  /// faces and waves then move in the same frames.
  var _riding = false;
  var _base = 0.0;
  var _now = 0.0;
  var _frame = 0;
  var _still = false;
  var _hidden = false;
  var _muted = false;
  var _checking = false;
  var _repaints = 0;
  var _rate = characterFps;

  /// How many repaints the clock has asked for: for tests and profiling.
  int get repaints => _repaints;

  /// Each tick; subscribers note it when asked to repaint and when painted.
  int get frame => _frame;

  /// Seconds on this clock: the latest tick's time.
  double get now => _now;

  /// Reduced motion: every face holds its mood's still pose.
  bool get still => _still;

  set still(bool value) {
    if (value == _still) return;
    _still = value;
    // Every face redraws once in its still (or moving) pose.
    for (final subscriber in _subscribers) {
      subscriber._notify(_frame, null);
    }
    _schedule();
  }

  /// The window is hidden or minimized: nothing animates.
  set hidden(bool value) {
    if (value == _hidden) return;
    _hidden = value;
    _schedule();
  }

  /// Tickers are muted where the characters are ([TickerMode]): nothing
  /// animates.
  set muted(bool value) {
    if (value == _muted) return;
    _muted = value;
    _schedule();
  }

  /// Something started moving (a shape changing): run the clock for it.
  void wake() => _schedule();

  /// The one track per agent and kind that every copy of its face shares,
  /// so two live copies compute each pose once.
  MoodTrack track(String id, int seed, Mood mood, {required bool live}) {
    final key = '${live ? 'live' : 'ref'}/$id';
    return _tracks[key] ??= MoodTrack(seed: seed, mood: mood, compact: !live);
  }

  CharacterSubscription subscribe({
    required MoodTrack track,
    required bool live,
    required double size,
  }) {
    final subscription = CharacterSubscription._(this, track, live, size);
    _subscribers.add(subscription);
    _schedule();
    return subscription;
  }

  void _unsubscribe(CharacterSubscription subscription) {
    _subscribers.remove(subscription);
    _schedule();
  }

  /// How many subscribers are being woken: for tests and profiling.
  int get activeSubscribers =>
      _subscribers.where((subscriber) => subscriber._active).length;

  /// How many characters are mounted, woken or not: for profiling.
  int get subscribers => _subscribers.length;

  /// Whether the clock is ticking: for tests and profiling.
  bool get ticking => _ticks != null || _riding;

  /// Thinking or working: it moves all the time, not only in the calm
  /// windows.
  bool _lively(CharacterSubscription subscriber) =>
      subscriber.live &&
      !subscriber.track.compact &&
      isLively(subscriber.track.mood);

  bool _moving(CharacterSubscription subscriber, double t) =>
      _lively(subscriber) || subscriber.track.settling(t) || inCalmWindow(t);

  /// How often to tick for [subscriber] while it moves: big faces that
  /// think, work or change mood at [characterFps], the rest at
  /// [smallFaceFps].
  int _fps(CharacterSubscription subscriber, double t) =>
      subscriber.size > smallFace &&
          (_lively(subscriber) || subscriber.track.settling(t))
      ? characterFps
      : smallFaceFps;

  void _tick(Timer timer) => _advance(_base + timer.tick / _rate);

  void _beat() => _advance(_now + 1 / LiveClock.waveFps);

  void _advance(double now) {
    _now = now;
    _frame++;
    var asked = false;
    for (final subscriber in _subscribers) {
      if (!subscriber._active) continue;
      final pose = subscriber.track.pose(_now);
      final shown = subscriber._shown;
      if (shown == null ||
          poseDelta(pose, shown, subscriber.size) >= minimumStep) {
        subscriber._notify(_frame, pose);
        _repaints++;
        asked = true;
      }
    }
    if (asked) _checkPainted();
    _schedule();
  }

  /// After the frame that follows, whoever was asked to repaint and wasn't
  /// painted is off screen: it goes dormant.
  void _checkPainted() {
    if (_checking) return;
    _checking = true;
    // A face that was never painted asks for no frame when told to
    // repaint, so make sure one comes to tell.
    SchedulerBinding.instance.ensureVisualUpdate();
    SchedulerBinding.instance.addPostFrameCallback((_) {
      _checking = false;
      var changed = false;
      for (final subscriber in _subscribers) {
        if (subscriber._active &&
            subscriber._notifiedAt > subscriber._paintedAt) {
          subscriber._active = false;
          changed = true;
        }
      }
      if (changed) _schedule();
    });
  }

  void _start(int fps) {
    // While the window's live clock runs (an agent working), faces move
    // on its ticks, at its rate: every frame on a Mac redraws the whole
    // window, so a timer of their own would add frames between its.
    if (LiveClock.instance.running.value) {
      _ticks?.cancel();
      _ticks = null;
      _ride(true);
      return;
    }
    _ride(false);
    if (_ticks != null && fps == _rate) return;
    _ticks?.cancel();
    _rate = fps;
    _base = _now;
    _ticks = Timer.periodic(Duration(microseconds: 1000000 ~/ fps), _tick);
  }

  void _stop() {
    _ticks?.cancel();
    _ticks = null;
    _ride(false);
  }

  void _ride(bool ride) {
    if (ride == _riding) return;
    _riding = ride;
    ride
        ? LiveClock.instance.beat.addListener(_beat)
        : LiveClock.instance.beat.removeListener(_beat);
  }

  /// Ticks while anything on screen moves, as fast as the fastest needs;
  /// otherwise stops, and sleeps until the next calm window opens.
  void _schedule() {
    _sleep?.cancel();
    _sleep = null;
    if (_still || _hidden || _muted) {
      _stop();
      return;
    }
    var any = false;
    int? fps;
    for (final subscriber in _subscribers) {
      if (!subscriber._active) continue;
      any = true;
      if (!_moving(subscriber, _now)) continue;
      fps = _fps(subscriber, _now);
      if (fps == characterFps) break;
    }
    if (fps != null) return _start(fps);
    _stop();
    if (!any) return;
    // Every face on screen is calm and still: nothing to draw until the
    // next window.
    final next = nextCalmWindow(_now);
    final wait = Duration(
      microseconds: ((next - _now) * 1e6).clamp(0, 60e6).round(),
    );
    _sleep = Timer(wait, () {
      _sleep = null;
      if (next > _now) _now = next;
      _schedule();
    });
  }

  void dispose() {
    LiveClock.instance.running.removeListener(_schedule);
    _sleep?.cancel();
    _stop();
  }
}

/// One mounted face's place on the clock: asked to repaint while it's on
/// screen and its face has moved.
class CharacterSubscription extends ChangeNotifier {
  CharacterSubscription._(this._clock, this.track, this.live, this.size);

  final CharacterClock _clock;
  final MoodTrack track;
  final bool live;

  /// How big it's drawn, in logical pixels: how far a pose moves on it.
  final double size;
  var _active = true;
  var _notifiedAt = -1;
  var _paintedAt = -1;

  /// The pose it was last asked to show; null to show whatever comes next.
  Pose? _shown;

  void _notify(int frame, Pose? pose) {
    _notifiedAt = frame;
    _shown = pose;
    notifyListeners();
  }

  /// It was painted: it's on screen, so it's woken again if it had gone
  /// dormant.
  void painted() {
    _paintedAt = _clock.frame;
    if (!_active) {
      _active = true;
      _shown = null;
      _clock._schedule();
    }
  }

  /// Off the clock, for good.
  void cancel() {
    _clock._unsubscribe(this);
    dispose();
  }
}

/// Gives the widgets below it the window's [CharacterClock], and stops it
/// for reduced motion, muted tickers and while the app is hidden.
class CharacterHost extends StatefulWidget {
  const CharacterHost({super.key, required this.child});

  final Widget child;

  static CharacterClock? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<_CharacterScope>()?.clock;

  @override
  State<CharacterHost> createState() => _CharacterHostState();
}

class _CharacterHostState extends State<CharacterHost>
    with WidgetsBindingObserver {
  final _clock = CharacterClock();

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _clock.still = MediaQuery.maybeDisableAnimationsOf(context) ?? false;
    _clock.muted = !TickerMode.valuesOf(context).enabled;
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // Losing focus (inactive) isn't hidden: the window still shows.
    _clock.hidden =
        state == AppLifecycleState.hidden || state == AppLifecycleState.paused;
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _clock.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) =>
      _CharacterScope(clock: _clock, child: widget.child);
}

class _CharacterScope extends InheritedWidget {
  const _CharacterScope({required this.clock, required super.child});

  final CharacterClock clock;

  @override
  bool updateShouldNotify(_CharacterScope old) => old.clock != clock;
}
