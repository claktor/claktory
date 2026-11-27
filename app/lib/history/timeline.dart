// History replay (#458): what the backend says this Claktory was, over
// time (HistoryOverview), and everything that happened in it
// (HistoryEvents), kept as the app needs them to draw any moment fast:
// spans to test, and events in typed columns to binary search.
//
// The backend decides what existed when, what's reconstructed and how
// messages group; this only reads it (tenet 7).
import 'dart:math' as math;
import 'dart:typed_data';

import '../protocol/factory/v1/factory.pb.dart';

/// A stretch something was there: from [from] up to [to] (ms), open-ended
/// while it's still there.
class Span {
  const Span(this.from, this.to, {this.inferred = false});

  factory Span.of(HistoryInterval interval) => Span(
    interval.fromMs.toDouble(),
    interval.toMs == 0 ? double.infinity : interval.toMs.toDouble(),
    inferred: interval.inferred,
  );

  final double from;
  final double to;

  /// Reconstructed from what was lying around, before recording began.
  final bool inferred;

  bool contains(double t) => t >= from && t < to;
}

/// The first of [spans] that holds [t], or null.
Span? spanAt(List<Span> spans, double t) {
  for (final span in spans) {
    if (span.contains(t)) return span;
  }
  return null;
}

/// Every node and edge that ever was, with when; the messages per hour; the
/// chapters.
class HistoryTimeline {
  HistoryTimeline._({
    required this.start,
    required this.end,
    required this.recordingSince,
    required this.nodes,
    required this.nodeSpans,
    required this.indexOf,
    required this.edges,
    required this.edgeSpans,
    required this.histogram,
    required this.histogramStart,
    required this.chapters,
    required this.cursors,
    required this.messageCount,
    required this.version,
  });

  factory HistoryTimeline.of(HistoryOverview overview) {
    final start = overview.startMs.toDouble();
    final nodes = [for (final node in overview.nodes) node];
    final edges = [for (final edge in overview.edges) edge];
    final hour = Duration.millisecondsPerHour;
    return HistoryTimeline._(
      start: start,
      end: math.max(start, overview.endMs.toDouble()),
      recordingSince: overview.recordingSinceMs.toDouble(),
      nodes: nodes,
      nodeSpans: [
        for (final node in nodes)
          [for (final span in node.present) Span.of(span)],
      ],
      indexOf: {
        for (var i = 0; i < nodes.length; i++)
          if (nodes[i].node.id.isNotEmpty) nodes[i].node.id: i + 1,
      },
      edges: edges,
      edgeSpans: [
        for (final edge in edges)
          [for (final span in edge.present) Span.of(span)],
      ],
      histogram: Uint32List.fromList(overview.histogram),
      histogramStart: (overview.startMs.toInt() ~/ hour * hour).toDouble(),
      chapters: [...overview.chapters]
        ..sort((a, b) => a.atMs.compareTo(b.atMs)),
      cursors: [...overview.cursors],
      messageCount: overview.messageCount.toInt(),
      version: overview.historyVersion.toInt(),
    );
  }

  /// The first moment and the last one known (ms).
  final double start;
  final double end;

  /// Before this, what's drawn was reconstructed, and dotted where it was.
  final double recordingSince;

  /// The overview's nodes: event index i ≥ 1 is nodes[i - 1], 0 the person.
  final List<HistoryNode> nodes;
  final List<List<Span>> nodeSpans;

  /// Each node's event index, by its id.
  final Map<String, int> indexOf;
  final List<HistoryEdge> edges;
  final List<List<Span>> edgeSpans;

  /// Events per hour, hour 0 starting at [histogramStart].
  final Uint32List histogram;
  final double histogramStart;
  final List<HistoryChapter> chapters;
  final List<HistoryCursor> cursors;
  final int messageCount;
  final int version;

  /// Whether anything here was reconstructed.
  bool get reconstructed => recordingSince > start;

  /// The messaging edges between each pair of nodes, either way, keyed by
  /// [_pair].
  late final Map<int, List<int>> _messaging = () {
    final out = <int, List<int>>{};
    for (var i = 0; i < edges.length; i++) {
      final edge = edges[i].edge;
      if (edge.kind != MapEdgeKind.MAP_EDGE_KIND_MESSAGES) continue;
      final a = indexOf[edge.fromId];
      final b = indexOf[edge.toId];
      if (a == null || b == null) continue;
      (out[_pair(a, b)] ??= []).add(i);
    }
    return out;
  }();

  static int _pair(int a, int b) =>
      a < b ? a * 0x100000000 + b : b * 0x100000000 + a;

  /// The spans of the messaging edges between [a] and [b], either way;
  /// empty when they have none.
  List<List<Span>> messagingSpans(int a, int b) => [
    for (final edge in _messaging[_pair(a, b)] ?? const <int>[])
      edgeSpans[edge],
  ];

  /// Whether nodes [a] and [b] could message each other at [t], as far as
  /// is known: a message without such a line travels in an arc instead.
  bool messagingAt(int a, int b, double t) {
    final list = _messaging[_pair(a, b)];
    if (list == null) return false;
    for (final edge in list) {
      if (spanAt(edgeSpans[edge], t) != null) return true;
    }
    return false;
  }

  /// The node an event index names; null for the person (0).
  HistoryNode? node(int index) =>
      index <= 0 || index > nodes.length ? null : nodes[index - 1];

  /// [node]'s name, role and place at [t]: its latest change by then, or
  /// the earliest known before its first.
  ({String label, String detail, String parentId, String projectId}) labelAt(
    int index,
    double t,
  ) {
    final node = nodes[index - 1];
    HistoryLabel? at;
    for (final label in node.labels) {
      if (label.atMs.toDouble() > t) {
        at ??= label;
        break;
      }
      at = label;
    }
    if (at == null) {
      return (
        label: node.node.label,
        detail: node.node.detail,
        parentId: node.node.parentId,
        projectId: node.node.projectId,
      );
    }
    return (
      label: at.label,
      detail: at.detail,
      parentId: at.parentId,
      projectId: at.projectId,
    );
  }

  /// When [index]'s name last changed at or before [t], or null.
  double? renamedAt(int index, double t) {
    final labels = nodes[index - 1].labels;
    double? at;
    for (var i = 1; i < labels.length; i++) {
      final when = labels[i].atMs.toDouble();
      if (when > t) break;
      if (labels[i].label != labels[i - 1].label) at = when;
    }
    return at;
  }
}

/// What an event is: who it was from, to whom.
enum EventKind {
  agent,
  person,
  card,
  proposal,
  secretRequest,
  pipeIn,
  pipeOut,
  task,
  monitor,
  claktory,

  /// An app's event to an agent (a Telegram message, say), from the app.
  app;

  static EventKind of(HistoryEventKind kind) => switch (kind) {
    HistoryEventKind.HISTORY_EVENT_KIND_PERSON => person,
    HistoryEventKind.HISTORY_EVENT_KIND_CARD => card,
    HistoryEventKind.HISTORY_EVENT_KIND_PROPOSAL => proposal,
    HistoryEventKind.HISTORY_EVENT_KIND_SECRET_REQUEST => secretRequest,
    HistoryEventKind.HISTORY_EVENT_KIND_PIPE_IN => pipeIn,
    HistoryEventKind.HISTORY_EVENT_KIND_PIPE_OUT => pipeOut,
    HistoryEventKind.HISTORY_EVENT_KIND_TASK => task,
    HistoryEventKind.HISTORY_EVENT_KIND_MONITOR => monitor,
    HistoryEventKind.HISTORY_EVENT_KIND_CLAKTORY => claktory,
    HistoryEventKind.HISTORY_EVENT_KIND_APP => app,
    _ => agent,
  };

  /// A tick on the agent's chip rather than something travelling.
  bool get tick => this == task || this == monitor || this == claktory;

  /// To the person: something that waits for their answer.
  bool get asks => this == card || this == proposal || this == secretRequest;
}

/// HistoryEvents flags.
const eventQuiet = 1;
const eventReply = 2;

/// Every event loaded so far, in time order, in typed columns: about 20
/// bytes an event, however many there are.
class HistoryLog {
  var _time = Float64List(0);
  var _from = Uint32List(0);
  var _kind = Uint8List(0);
  var _flags = Uint8List(0);
  var _toStart = Uint32List(0);
  var _toCount = Uint16List(0);
  var _to = Uint32List(0);
  var _length = 0;
  var _toLength = 0;

  /// Each node's runs: starts and ends (ms), in order; an end of infinity
  /// while it runs. In typed arrays: a list of doubles boxes each one,
  /// three times the memory at millions of runs (Polpo).
  final _runs = <int, _Runs>{};

  /// The nodes with runs.
  Iterable<int> get runNodes => _runs.keys;

  /// [node]'s runs' starts and ends, in order (views, not copies).
  Float64List runStarts(int node) => _runs[node]?.starts ?? Float64List(0);
  Float64List runEnds(int node) => _runs[node]?.ends ?? Float64List(0);

  int get length => _length;

  /// When the last event loaded was, or -infinity.
  double get last =>
      _length == 0 ? double.negativeInfinity : _time[_length - 1];

  double time(int i) => _time[i];
  int from(int i) => _from[i];
  EventKind kind(int i) => EventKind.values[_kind[i]];
  int flags(int i) => _flags[i];
  int toCount(int i) => _toCount[i];
  int to(int i, int k) => _to[_toStart[i] + k];

  /// The times of every event loaded, in order (a view, not a copy).
  Float64List get times => Float64List.sublistView(_time, 0, _length);

  /// Adds [page]'s events (and runs) where they belong: after those
  /// already here, or, for a stretch loaded later than one after it, before
  /// that one ([before]: its first event's time; the page's events from
  /// then on are left out, as already here). For a [push] (new events
  /// while watched, which may have come while pages loaded), any not after
  /// the last one loaded are left out. Says whether the page reached
  /// [before].
  bool add(HistoryEvents page, {bool push = false, double? before}) {
    final count = page.dt.length;
    // What's added: times, the rest by column, typed: a page holds
    // thousands (Polpo).
    var recipients = 0;
    for (var i = 0; i < count; i++) {
      recipients += i < page.toCount.length ? math.max(1, page.toCount[i]) : 1;
    }
    final times = Float64List(count);
    final froms = Uint32List(count);
    final kinds = Uint8List(count);
    final flags = Uint8List(count);
    final counts = Uint16List(count);
    final tos = Uint32List(recipients);
    var n = 0;
    var t = 0;
    var reached = false;
    var at = page.baseMs.toDouble();
    var toAt = 0;
    final pageKinds = page.kind;
    for (var i = 0; i < count; i++) {
      at += page.dt[i].toDouble();
      final many = i < page.toCount.length ? math.max(1, page.toCount[i]) : 1;
      final skip = before != null && at >= before
          ? (reached = true)
          : push
          ? at <= last
          : before == null && at < last;
      if (skip) {
        toAt += many;
        continue;
      }
      times[n] = at;
      froms[n] = i < page.from.length ? page.from[i] : 0;
      kinds[n] = i < pageKinds.length
          ? EventKind.of(pageKinds[i]).index
          : EventKind.agent.index;
      flags[n] = i < page.flags.length ? page.flags[i] & 0xFF : 0;
      counts[n] = many;
      n++;
      for (var k = 0; k < many; k++) {
        tos[t++] = toAt < page.to.length ? page.to[toAt] : 0;
        toAt++;
      }
    }
    _insert(
      Float64List.sublistView(times, 0, n),
      Uint32List.sublistView(froms, 0, n),
      Uint8List.sublistView(kinds, 0, n),
      Uint8List.sublistView(flags, 0, n),
      Uint16List.sublistView(counts, 0, n),
      Uint32List.sublistView(tos, 0, t),
    );
    var start = page.baseMs.toDouble();
    for (var i = 0; i < page.runNode.length; i++) {
      start += i < page.runStart.length ? page.runStart[i].toDouble() : 0;
      if (before != null && start >= before) continue;
      final ms = i < page.runMs.length ? page.runMs[i].toDouble() : 0.0;
      _run(page.runNode[i], start, ms == 0 ? double.infinity : start + ms);
    }
    return reached;
  }

  /// Puts events (in order) in their place: at the end, or before the
  /// first one later than them.
  void _insert(
    Float64List times,
    Uint32List froms,
    Uint8List kinds,
    Uint8List flags,
    Uint16List counts,
    Uint32List tos,
  ) {
    final n = times.length;
    if (n == 0) return;
    _grow(_length + n, _toLength + tos.length);
    final at = times.first >= last ? _length : upperBound(times.first);
    final toAt = at == _length ? _toLength : _toStart[at];
    if (at < _length) {
      // Make room: everything after moves along.
      _time.setRange(at + n, _length + n, _time, at);
      _from.setRange(at + n, _length + n, _from, at);
      _kind.setRange(at + n, _length + n, _kind, at);
      _flags.setRange(at + n, _length + n, _flags, at);
      _toStart.setRange(at + n, _length + n, _toStart, at);
      _toCount.setRange(at + n, _length + n, _toCount, at);
      _to.setRange(toAt + tos.length, _toLength + tos.length, _to, toAt);
      for (var i = at + n; i < _length + n; i++) {
        _toStart[i] += tos.length;
      }
    }
    var to = toAt;
    for (var i = 0; i < n; i++) {
      _time[at + i] = times[i];
      _from[at + i] = froms[i];
      _kind[at + i] = kinds[i];
      _flags[at + i] = flags[i];
      _toStart[at + i] = to;
      _toCount[at + i] = counts[i];
      to += counts[i];
    }
    _to.setRange(toAt, toAt + tos.length, tos);
    _length += n;
    _toLength += tos.length;
  }

  void _run(int node, double start, double end) {
    final runs = _runs.putIfAbsent(node, _Runs.new);
    final starts = runs.starts;
    final ends = runs.ends;
    final i = _upper(starts, start);
    // The same run again (a push saying it ended): its end now.
    if (i > 0 && starts[i - 1] == start) {
      ends[i - 1] = i < starts.length ? math.min(end, starts[i]) : end;
      return;
    }
    // A run still open when a later one starts ended by then.
    if (i > 0 && ends[i - 1] > start) ends[i - 1] = start;
    runs.insert(i, start, i < starts.length ? math.min(end, starts[i]) : end);
  }

  /// Whether [node] was working at [t].
  bool running(int node, double t) {
    final runs = _runs[node];
    if (runs == null || runs.length == 0) return false;
    final i = _upper(runs.starts, t) - 1;
    return i >= 0 && t < runs.ends[i];
  }

  /// The index of the first event at or after [t].
  int lowerBound(double t) {
    var lo = 0;
    var hi = _length;
    while (lo < hi) {
      final mid = (lo + hi) >> 1;
      if (_time[mid] < t) {
        lo = mid + 1;
      } else {
        hi = mid;
      }
    }
    return lo;
  }

  /// The index of the first event after [t].
  int upperBound(double t) {
    var lo = 0;
    var hi = _length;
    while (lo < hi) {
      final mid = (lo + hi) >> 1;
      if (_time[mid] <= t) {
        lo = mid + 1;
      } else {
        hi = mid;
      }
    }
    return lo;
  }

  static int _upper(Float64List list, double t) {
    var lo = 0;
    var hi = list.length;
    while (lo < hi) {
      final mid = (lo + hi) >> 1;
      if (list[mid] <= t) {
        lo = mid + 1;
      } else {
        hi = mid;
      }
    }
    return lo;
  }

  void _grow(int events, int recipients) {
    if (events > _time.length) {
      final size = math.max(events, _time.length * 2);
      _time = Float64List(size)..setRange(0, _length, _time);
      _from = Uint32List(size)..setRange(0, _length, _from);
      _kind = Uint8List(size)..setRange(0, _length, _kind);
      _flags = Uint8List(size)..setRange(0, _length, _flags);
      _toStart = Uint32List(size)..setRange(0, _length, _toStart);
      _toCount = Uint16List(size)..setRange(0, _length, _toCount);
    }
    if (recipients > _to.length) {
      final size = math.max(recipients, _to.length * 2);
      _to = Uint32List(size)..setRange(0, _toLength, _to);
    }
  }

  /// How many messages [node] sent and got up to [t], and whom it
  /// exchanged most with (an event index; -1 for none).
  ({int sent, int received, int partner}) countsFor(int node, double t) {
    final end = upperBound(t);
    var sent = 0;
    var received = 0;
    final with_ = <int, int>{};
    for (var i = 0; i < end; i++) {
      final kind = this.kind(i);
      if (kind.tick) {
        continue;
      }
      final from = _from[i];
      final count = _toCount[i];
      final start = _toStart[i];
      if (from == node) {
        sent++;
        for (var k = 0; k < count; k++) {
          final to = _to[start + k];
          with_[to] = (with_[to] ?? 0) + 1;
        }
        continue;
      }
      for (var k = 0; k < count; k++) {
        if (_to[start + k] == node) {
          received++;
          with_[from] = (with_[from] ?? 0) + 1;
          break;
        }
      }
    }
    var partner = -1;
    var most = 0;
    for (final MapEntry(:key, :value) in with_.entries) {
      if (value > most || (value == most && key < partner)) {
        most = value;
        partner = key;
      }
    }
    return (sent: sent, received: received, partner: partner);
  }
}

/// One node's runs, in order: growable typed arrays.
class _Runs {
  var _starts = Float64List(4);
  var _ends = Float64List(4);
  var length = 0;

  Float64List get starts => Float64List.sublistView(_starts, 0, length);
  Float64List get ends => Float64List.sublistView(_ends, 0, length);

  /// Puts a run at [i]: at the end as pages come in order, else what's
  /// after it moves along.
  void insert(int i, double start, double end) {
    if (length == _starts.length) {
      final size = length * 2;
      _starts = Float64List(size)..setRange(0, length, _starts);
      _ends = Float64List(size)..setRange(0, length, _ends);
    }
    if (i < length) {
      _starts.setRange(i + 1, length + 1, _starts, i);
      _ends.setRange(i + 1, length + 1, _ends, i);
    }
    _starts[i] = start;
    _ends[i] = end;
    length++;
  }
}
