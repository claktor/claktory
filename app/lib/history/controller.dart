// History replay's state while it's open (#458): what's loaded from the
// backend and where the playhead is. Nothing of it is kept once it closes
// (tenet 7): it's all asked for again next time.
import 'dart:async';
import 'dart:math' as math;

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';

import '../backend.dart';
import '../protocol/factory/v1/factory.pb.dart';
import 'routes.dart';
import 'scene.dart';
import 'timeline.dart';
import 'warp.dart';

/// Where History's records come from: the backend, or a test's.
abstract class HistorySource {
  Future<HistoryOverview> overview();

  /// Events after [cursor] (from the first when empty), up to [until]
  /// (ms) when given.
  Future<HistoryEvents> events(List<int> cursor, {double? until});
  Future<void> watch(bool on);
  Stream<HistoryEvents> get pushes;
  ValueListenable<Int64> get changed;
}

class BackendHistory implements HistorySource {
  BackendHistory(this.backend);

  final Backend backend;

  @override
  Future<HistoryOverview> overview() => backend.historyOverview();

  @override
  Future<HistoryEvents> events(List<int> cursor, {double? until}) =>
      backend.historyEvents(
        cursor: cursor,
        untilMs: until == null ? null : Int64(until.ceil()),
      );

  @override
  Future<void> watch(bool on) => backend.watchHistory(on);

  @override
  Stream<HistoryEvents> get pushes => backend.historyPushes;

  @override
  ValueListenable<Int64> get changed => backend.historyChanged;
}

/// A stretch of the history's events being loaded, from [from] (the
/// first's: -infinity) up to [until] so far, or, [done], up to the next
/// stretch or the end.
class _Stretch {
  _Stretch(this.from, this.cursor);

  final double from;
  List<int> cursor;
  double until = double.negativeInfinity;
  var done = false;
}

/// Beyond the history known when it opened, time runs as it does: a
/// second a second.
const _liveRate = 1000.0;

class HistoryController extends ChangeNotifier {
  HistoryController(
    this.source, {
    this.order = const {},
    this.measure,
    DateTime Function()? clock,
  }) : _clock = clock ?? DateTime.now;

  final HistorySource source;

  /// The sidebar's order, for the board's layout.
  final Map<String, int> order;
  final double Function(String text)? measure;
  final DateTime Function() _clock;

  HistoryTimeline? timeline;
  HistoryScene? scene;
  Routes? routes;
  var log = HistoryLog();
  TimeWarp? warp;

  /// Why it couldn't load, in words to show; null while all's well.
  String? problem;

  /// Every page of events is in.
  var loaded = false;

  /// What's loaded, in order: from the start, and from wherever the
  /// playhead went ahead of it. Each reaching the next joins it.
  var _stretches = [_Stretch(double.negativeInfinity, const [])];

  /// Counts loads started: one begun again (the overview changed) ends
  /// the one before.
  var _loads = 0;
  var _opened = false;
  var _closed = false;
  final _waiting = <HistoryEvents>[];
  StreamSubscription<HistoryEvents>? _pushes;

  /// The playhead, in history time (ms since 1970).
  final time = ValueNotifier<double>(0);
  var playing = false;
  var speed = HistorySpeed.auto;

  /// At the end, following what happens now.
  var live = false;

  /// The one project shown, by id; null for all of them. The person's
  /// choice while History is open, never kept.
  String? project;

  /// Shows only [id]'s project (null: all of them).
  void showProject(String? id) {
    if (id == project) return;
    project = id;
    notifyListeners();
  }

  /// Loads the overview and the first page, then the rest in the
  /// background, and watches for what's new.
  Future<void> open() async {
    if (_opened) return;
    _opened = true;
    source.changed.addListener(_changed);
    _pushes = source.pushes.listen(_pushed);
    await _load();
  }

  Future<void> _load() async {
    problem = null;
    notifyListeners();
    try {
      final overview = await source.overview();
      if (_closed) return;
      _take(overview);
      time.value = timeline!.start;
      notifyListeners();
      unawaited(source.watch(true).catchError((_) {}));
      await _pages();
    } on RequestFailure catch (failure) {
      if (_closed) return;
      // A Claktory from before History knows no such request.
      problem = failure.code == ErrorCode.ERROR_CODE_UNKNOWN_MESSAGE
          ? 'Update Claktory to see its history.'
          : "Couldn't load the history. ${failure.message}";
      notifyListeners();
    }
  }

  /// Try again after [problem].
  Future<void> retry() => timeline == null ? _load() : _pages();

  void _take(HistoryOverview overview) {
    final next = HistoryTimeline.of(overview);
    timeline = next;
    scene = HistoryScene.build(next, order: order, measure: measure);
    routes = Routes(scene!);
    _rewarp();
  }

  /// Loads the events: first where the playhead is, then the rest in
  /// order. Again from the start when [fresh]: what's here stays until the
  /// first page comes.
  Future<void> _pages({bool fresh = false}) async {
    final load = ++_loads;
    problem = null;
    if (fresh) {
      _stretches = [_Stretch(double.negativeInfinity, const [])];
    }
    try {
      while (!_closed && load == _loads) {
        final k = _next();
        if (k < 0) break;
        final stretch = _stretches[k];
        final asked = k + 1 < _stretches.length ? _stretches[k + 1] : null;
        final page = await source.events(stretch.cursor, until: asked?.from);
        if (_closed || load != _loads) return;
        // Where the playhead went meanwhile may have started a stretch
        // since: up to that one, then.
        final i = _stretches.indexOf(stretch);
        final after = i + 1 < _stretches.length ? _stretches[i + 1] : null;
        if (fresh) {
          log = HistoryLog();
          fresh = false;
        }
        final reached = log.add(page, before: after?.from);
        if (page.dt.isNotEmpty) {
          var at = page.baseMs.toDouble();
          for (final dt in page.dt) {
            at += dt.toDouble();
          }
          stretch.until = math.max(
            stretch.until,
            math.min(at, after?.from ?? at),
          );
        }
        stretch.cursor = page.nextCursor;
        if (reached || page.nextCursor.isEmpty || page.dt.isEmpty) {
          stretch.done = true;
          if (after != null) {
            // Up to the next: one stretch.
            stretch
              ..cursor = after.cursor
              ..until = after.until
              ..done = after.done;
            _stretches.removeAt(i + 1);
          }
        }
        _rewarp();
        notifyListeners();
      }
      if (_closed || load != _loads) return;
      loaded = true;
      for (final push in _waiting) {
        log.add(push, push: true);
      }
      _waiting.clear();
      _rewarp();
      notifyListeners();
    } on RequestFailure catch (failure) {
      if (_closed || load != _loads) return;
      problem = "Couldn't load the history. ${failure.message}";
      notifyListeners();
    }
  }

  /// The stretch to load from next: the playhead's, else the first not
  /// done; -1 when all are.
  int _next() {
    final k = _stretchAt(time.value);
    for (var i = k; i < _stretches.length; i++) {
      if (!_stretches[i].done) return i;
    }
    return _stretches.indexWhere((stretch) => !stretch.done);
  }

  /// The stretch [t] falls in: the last starting by then.
  int _stretchAt(double t) {
    var k = 0;
    for (var i = 1; i < _stretches.length; i++) {
      if (_stretches[i].from <= t) k = i;
    }
    return k;
  }

  /// Whether the events around [t] are in.
  bool covered(double t) {
    if (loaded) return true;
    final stretch = _stretches[_stretchAt(t)];
    return stretch.done || stretch.until >= t;
  }

  /// The playhead is where events are still on their way, or playing
  /// waits for them.
  bool get fetching =>
      timeline != null && !live && (_held || !covered(time.value));

  /// Playing waits at the playhead for what comes next.
  var _held = false;

  /// What's loaded, for the warp: (from, to) in order.
  List<(double, double)> get _loaded => [
    for (var k = 0; k < _stretches.length; k++)
      (
        _stretches[k].from,
        _stretches[k].done
            ? (k + 1 < _stretches.length
                  ? _stretches[k + 1].from
                  : double.infinity)
            : _stretches[k].until,
      ),
  ];

  /// The playhead went to [t], not loaded yet: loads from the place
  /// nearest before it, unless what's loading gets there first.
  void _reach(double t) {
    final timeline = this.timeline;
    if (timeline == null || loaded || covered(t)) return;
    final k = _stretchAt(t);
    HistoryCursor? from;
    for (final cursor in timeline.cursors) {
      if (cursor.atMs.toDouble() > t) break;
      from = cursor;
    }
    if (from == null) return;
    final at = from.atMs.toDouble();
    if (at <= _stretches[k].until || at == _stretches[k].from) return;
    _stretches.insert(k + 1, _Stretch(at, from.cursor));
    _rewarp();
  }

  void _pushed(HistoryEvents push) {
    if (!loaded) {
      _waiting.add(push);
      return;
    }
    log.add(push, push: true);
    notifyListeners();
  }

  /// The overview changed (a new agent, say): read it again, keeping the
  /// events when every node is where it was.
  Future<void> _changed() async {
    final known = timeline;
    if (known == null || source.changed.value.toInt() <= known.version) return;
    try {
      final overview = await source.overview();
      if (_closed) return;
      final same =
          overview.nodes.length >= known.nodes.length &&
          [
            for (var i = 0; i < known.nodes.length; i++)
              overview.nodes[i].node.id == known.nodes[i].node.id,
          ].every((same) => same);
      _take(overview);
      if (!same) {
        loaded = false;
        unawaited(_pages(fresh: true));
      }
      notifyListeners();
    } on RequestFailure {
      // The next change, or opening it again, reads it.
    }
  }

  /// Following now with nothing on its way: nothing to draw until
  /// something new comes.
  bool get idle => live && _clock().millisecondsSinceEpoch - log.last > 2000;

  /// The end of the known history: where playing turns live.
  double get end => warp?.end ?? 0;

  /// The scrubber's end: the known history, or now while live.
  double get last => math.max(end, live ? time.value : log.last);

  void _rewarp() {
    final timeline = this.timeline;
    if (timeline == null) return;
    warp = TimeWarp.build(
      start: timeline.start,
      end: math.max(timeline.end, loaded ? log.last : timeline.end),
      speed: speed,
      times: log.times,
      loaded: loaded ? [(double.negativeInfinity, double.infinity)] : _loaded,
      histogram: timeline.histogram,
      histogramStart: timeline.histogramStart,
    );
  }

  /// When history time [h] plays, in seconds: past the known end, a second
  /// a second.
  double playAt(double h) {
    final warp = this.warp;
    if (warp == null) return 0;
    if (h <= warp.end) return warp.playAt(h);
    return warp.duration + (h - warp.end) / _liveRate;
  }

  double historyAt(double p) {
    final warp = this.warp;
    if (warp == null) return 0;
    if (p <= warp.duration) return warp.historyAt(p);
    return warp.end + (p - warp.duration) * _liveRate;
  }

  void play() {
    if (timeline == null) return;
    // From the start again once it's played to the end.
    if (!live && time.value >= end) time.value = timeline!.start;
    playing = true;
    notifyListeners();
  }

  void pause() {
    playing = false;
    notifyListeners();
  }

  void toggle() => playing ? pause() : play();

  void setSpeed(HistorySpeed value) {
    if (value == speed) return;
    speed = value;
    _rewarp();
    notifyListeners();
  }

  /// Moves the playhead to [t]; past the end, it follows now.
  void seek(double t) {
    final timeline = this.timeline;
    if (timeline == null) return;
    final now = _clock().millisecondsSinceEpoch.toDouble();
    final wasLive = live;
    final wasFetching = fetching;
    live = t >= end;
    time.value = live
        ? math.max(end, math.min(t, now))
        : math.max(t, timeline.start);
    _held = false;
    _reach(time.value);
    // Whether it waits for what's there shows.
    if (live != wasLive || fetching != wasFetching) notifyListeners();
  }

  /// To now, following it.
  void goLive() {
    seek(_clock().millisecondsSinceEpoch.toDouble());
    playing = true;
    notifyListeners();
  }

  /// [seconds] of playing have passed.
  void advance(double seconds) {
    if (!playing || warp == null) return;
    if (live) {
      time.value = _clock().millisecondsSinceEpoch.toDouble();
      return;
    }
    final next = historyAt(playAt(time.value) + seconds);
    // Waits where it is for what's still on its way, rather than playing
    // on without it.
    if (next < end && !covered(next)) {
      if (!_held) {
        _held = true;
        notifyListeners();
      }
      return;
    }
    if (_held) {
      _held = false;
      notifyListeners();
    }
    if (next >= end) {
      live = true;
      time.value = math.max(end, _clock().millisecondsSinceEpoch.toDouble());
      notifyListeners();
      return;
    }
    time.value = next;
  }

  /// The chapters, in order; with [project], only those of its own.
  List<HistoryChapter> get chapters {
    final all = timeline?.chapters ?? const <HistoryChapter>[];
    final project = this.project;
    final scene = this.scene;
    if (project == null || scene == null) return all;
    return scene.chaptersIn(all, project);
  }

  /// To the next chapter after the playhead ([forward]) or the one before.
  void chapter({required bool forward}) {
    final t = time.value;
    HistoryChapter? to;
    for (final chapter in chapters) {
      final at = chapter.atMs.toDouble();
      if (forward && at > t + 1) {
        to = chapter;
        break;
      }
      // Back: the one before, past the one just reached.
      if (!forward && at < t - 1000) to = chapter;
    }
    if (to != null) seek(to.atMs.toDouble());
  }

  /// How many messages had been sent by the playhead.
  int get messagesSoFar => log.upperBound(time.value);

  @override
  void dispose() {
    _closed = true;
    source.changed.removeListener(_changed);
    unawaited(_pushes?.cancel());
    if (_opened) unawaited(source.watch(false).catchError((_) {}));
    time.dispose();
    super.dispose();
  }
}
