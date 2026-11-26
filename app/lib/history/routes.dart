// The ways messages travel on History's board (#458): along the Map's own
// messaging lines from the sender's chip to the recipient's, the shortest
// way; or, between things no line joins (the person, a pipe's pad, agents
// whose line wasn't known), a gentle arc.
import 'dart:math' as math;
import 'dart:typed_data';
import 'dart:ui' show Offset, Rect;

import '../map_layout.dart';
import 'scene.dart';

/// A path a pulse runs along: a polyline with its length so far at each
/// point.
class Route {
  Route(List<Offset> points)
    : _x = Float32List(points.length),
      _y = Float32List(points.length),
      _run = Float32List(points.length) {
    var run = 0.0;
    for (var i = 0; i < points.length; i++) {
      _x[i] = points[i].dx;
      _y[i] = points[i].dy;
      if (i > 0) run += (points[i] - points[i - 1]).distance;
      _run[i] = run;
    }
  }

  final Float32List _x;
  final Float32List _y;
  final Float32List _run;

  /// Its points, in order.
  List<Offset> get points => [
    for (var i = 0; i < _x.length; i++) Offset(_x[i], _y[i]),
  ];

  double get length => _run.isEmpty ? 0 : _run.last;
  Offset get start => Offset(_x.first, _y.first);
  Offset get end => Offset(_x.last, _y.last);

  /// The point [fraction] (0 to 1) of the way along.
  Offset at(double fraction) {
    if (_x.length == 1 || length == 0) return start;
    final want = fraction.clamp(0.0, 1.0) * length;
    var lo = 0;
    var hi = _run.length - 1;
    while (hi - lo > 1) {
      final mid = (lo + hi) >> 1;
      if (_run[mid] <= want) {
        lo = mid;
      } else {
        hi = mid;
      }
    }
    final span = _run[hi] - _run[lo];
    final f = span == 0 ? 0.0 : (want - _run[lo]) / span;
    return Offset(
      _x[lo] + (_x[hi] - _x[lo]) * f,
      _y[lo] + (_y[hi] - _y[lo]) * f,
    );
  }
}

/// Every route asked for, kept: the same pair always takes the same way.
class Routes {
  Routes(this.scene);

  final HistoryScene scene;
  final _cache = <String, Route>{};
  _Graph? _graph;

  /// From chip [a] to chip [b] (indexes into the scene's chips).
  Route chips(int a, int b) => _cache.putIfAbsent('c$a>c$b', () {
    final path = (_graph ??= _Graph(scene)).path(a, b);
    if (path != null && path.length > 1) return Route(path);
    return arc(scene.chips[a].rect, scene.chips[b].rect);
  });

  /// From chip [a] to chip [b] straight across, in an arc: for a message
  /// with no line known between them.
  Route across(int a, int b) => _cache.putIfAbsent(
    'c$a~c$b',
    () => arc(scene.chips[a].rect, scene.chips[b].rect),
  );

  /// Between a place that has no lines (the person, a pad) and chip [chip]:
  /// [key] names the place, [toChip] says which way it goes.
  Route place(String key, Rect from, int chip, {required bool toChip}) =>
      _cache.putIfAbsent(
        toChip ? '$key>c$chip' : 'c$chip>$key',
        () => toChip
            ? arc(from, scene.chips[chip].rect)
            : arc(scene.chips[chip].rect, from),
      );

  /// Through pipe [pad] (an index into the scene's pads), from or to chip
  /// [chip]: down its line from the ring above it, where the other Claktory
  /// is, to the pad, then from its edge across to the chip; or the other
  /// way, [toChip] false. So a message through a pipe is seen leaving the
  /// board, or coming in. The stretch inside the pad isn't drawn.
  Route pipe(int pad, int chip, {required bool toChip}) =>
      _cache.putIfAbsent(toChip ? 'p$pad>c$chip' : 'c$chip>p$pad', () {
        final rect = scene.pads[pad].rect;
        final top = pipeTop(rect);
        final ring = top - const Offset(0, pipeReach);
        final across = arc(rect, scene.chips[chip].rect).points;
        final inward = [ring, top, ...across];
        return Route(toChip ? inward : inward.reversed.toList());
      });

  /// An arc from [a]'s edge to [b]'s, bowing to one side.
  static Route arc(Rect a, Rect b) {
    final from = edgeToward(a, b.center);
    final to = edgeToward(b, a.center);
    final mid = (from + to) / 2;
    final d = to - from;
    final bow = Offset(-d.dy, d.dx) * 0.18;
    final control = mid + bow;
    return Route([
      for (var i = 0; i <= 16; i++) _bezier(from, control, to, i / 16),
    ]);
  }

  static Offset _bezier(Offset a, Offset c, Offset b, double t) {
    final u = 1 - t;
    return a * (u * u) + c * (2 * u * t) + b * (t * t);
  }

  /// Where the line from [rect]'s middle toward [toward] leaves it.
  static Offset edgeToward(Rect rect, Offset toward) {
    final c = rect.center;
    final d = toward - c;
    if (d.dx == 0 && d.dy == 0) return c;
    final sx = d.dx == 0 ? double.infinity : (rect.width / 2) / d.dx.abs();
    final sy = d.dy == 0 ? double.infinity : (rect.height / 2) / d.dy.abs();
    return c + d * math.min(1.0, math.min(sx, sy));
  }
}

/// The messaging lines as a graph: a vertex wherever a line bends, ends or
/// another joins it (never where two only cross), the edges between them
/// as long as they are.
class _Graph {
  _Graph(HistoryScene scene) {
    final wires = [
      for (final wire in scene.wires)
        if (wire.wire.layer == MapLayer.messages) wire.wire,
    ];
    final ends = <int>{};
    for (final wire in wires) {
      if (wire.points.isEmpty) continue;
      ends
        ..add(_key(wire.points.first))
        ..add(_key(wire.points.last));
    }
    for (final wire in wires) {
      final points = wire.points;
      for (var i = 0; i + 1 < points.length; i++) {
        final a = _cell(points[i]);
        final b = _cell(points[i + 1]);
        final dx = (b.$1 - a.$1).sign;
        final dy = (b.$2 - a.$2).sign;
        var last = _vertex(a);
        var x = a.$1;
        var y = a.$2;
        var run = 0;
        while (x != b.$1 || y != b.$2) {
          x += dx;
          y += dy;
          run++;
          final key = (x << 20) | y;
          if ((x == b.$1 && y == b.$2) || ends.contains(key)) {
            final next = _vertex((x, y));
            _link(last, next, run);
            last = next;
            run = 0;
          }
        }
      }
    }
    // Where each chip's lines leave it.
    for (var c = 0; c < scene.chips.length; c++) {
      final rect = scene.chips[c].rect.inflate(0.5);
      final mine = <int>[];
      for (final key in ends) {
        final point = Offset((key >> 20) * mapCell, (key & 0xFFFFF) * mapCell);
        if (rect.contains(point)) {
          final v = _ids[key];
          if (v != null) mine.add(v);
        }
      }
      _ports[c] = mine;
    }
  }

  final _ids = <int, int>{};
  final _points = <Offset>[];
  final _next = <List<int>>[];
  final _cost = <List<int>>[];
  final _ports = <int, List<int>>{};

  static (int, int) _cell(Offset point) =>
      ((point.dx / mapCell).round(), (point.dy / mapCell).round());

  static int _key(Offset point) {
    final (x, y) = _cell(point);
    return (x << 20) | y;
  }

  int _vertex((int, int) cell) {
    final key = (cell.$1 << 20) | cell.$2;
    return _ids.putIfAbsent(key, () {
      _points.add(Offset(cell.$1 * mapCell, cell.$2 * mapCell));
      _next.add([]);
      _cost.add([]);
      return _points.length - 1;
    });
  }

  void _link(int a, int b, int cost) {
    if (a == b) return;
    _next[a].add(b);
    _cost[a].add(cost);
    _next[b].add(a);
    _cost[b].add(cost);
  }

  /// The shortest way from chip [a] to chip [b], or null.
  List<Offset>? path(int a, int b) {
    final from = _ports[a] ?? const [];
    final to = (_ports[b] ?? const []).toSet();
    if (from.isEmpty || to.isEmpty) return null;
    final n = _points.length;
    final dist = Int32List(n)..fillRange(0, n, 1 << 30);
    final prev = Int32List(n)..fillRange(0, n, -1);
    final heap = _Heap();
    for (final v in from) {
      dist[v] = 0;
      heap.push(0, v);
    }
    var goal = -1;
    while (heap.isNotEmpty) {
      final (d, v) = heap.pop();
      if (d != dist[v]) continue;
      if (to.contains(v)) {
        goal = v;
        break;
      }
      final next = _next[v];
      final cost = _cost[v];
      for (var i = 0; i < next.length; i++) {
        final w = next[i];
        final nd = d + cost[i];
        if (nd < dist[w]) {
          dist[w] = nd;
          prev[w] = v;
          heap.push(nd, w);
        }
      }
    }
    if (goal < 0) return null;
    final out = <Offset>[];
    for (var v = goal; v != -1; v = prev[v]) {
      out.add(_points[v]);
    }
    return out.reversed.toList();
  }
}

/// A binary min-heap of (cost, vertex).
class _Heap {
  final _cost = <int>[];
  final _item = <int>[];

  bool get isNotEmpty => _cost.isNotEmpty;

  void push(int cost, int item) {
    _cost.add(cost);
    _item.add(item);
    var i = _cost.length - 1;
    while (i > 0) {
      final parent = (i - 1) ~/ 2;
      if (_cost[parent] <= _cost[i]) break;
      _swap(i, parent);
      i = parent;
    }
  }

  (int, int) pop() {
    final top = (_cost.first, _item.first);
    final last = _cost.length - 1;
    _swap(0, last);
    _cost.removeLast();
    _item.removeLast();
    var i = 0;
    while (true) {
      final l = 2 * i + 1;
      final r = l + 1;
      var m = i;
      if (l < _cost.length && _cost[l] < _cost[m]) m = l;
      if (r < _cost.length && _cost[r] < _cost[m]) m = r;
      if (m == i) break;
      _swap(i, m);
      i = m;
    }
    return top;
  }

  void _swap(int i, int j) {
    final c = _cost[i];
    _cost[i] = _cost[j];
    _cost[j] = c;
    final t = _item[i];
    _item[i] = _item[j];
    _item[j] = t;
  }
}
