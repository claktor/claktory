// The board History plays on (#458): the Map's own layout, computed once
// over every node and edge that ever was, so nothing moves while it plays;
// then, for any moment, which of its chips, boxes, lines and pads were
// there.
//
// An agent that moved to another folder or project has a chip in each
// place it worked, one shown at a time; between them it hops.
import 'dart:math' as math;
import 'dart:typed_data';
import 'dart:ui' show Offset, Rect;

import '../map_layout.dart';
import '../protocol/factory/v1/factory.pb.dart';
import 'timeline.dart';

/// How a line is drawn at a moment: not at all, as usual, or dotted
/// (reconstructed).
const hidden = 0;
const shown = 1;
const dotted = 2;

/// A chip on the board: a node, or one of the places a moved agent was.
class SceneChip {
  SceneChip(this.chip, this.node, this.place);

  final MapChip chip;

  /// Its node's event index.
  final int node;

  /// For one of a moved agent's chips, the edge (into the timeline's
  /// edges) that puts it here; -1 for every other chip.
  final int place;

  String get id => chip.id;
  Rect get rect => chip.rect;
}

/// What shows a line: an edge between its two chips ([edges]), both chips
/// ([both]), its chip with another on its bus ([chip]), or for a trunk,
/// enough of the agents on it (or a net's stub, any of its branches).
enum WireRule { edges, chip, trunk, both }

class SceneWire {
  SceneWire._(this.wire, this.rule, this.a, this.b, this.edges, this.ends);

  final MapWire wire;
  final WireRule rule;

  /// Its chips (indexes into the scene's chips): for an edge's line both
  /// ends, for a tap its chip (b -1); -1 when it has none.
  final int a;
  final int b;

  /// The edges it draws, any of which shows it.
  final List<int> edges;

  /// The chips on a bus, for a trunk.
  final List<int> ends;
}

/// A box: the workspace, a project, a folder, the admins, the vault or the
/// apps, with the chips inside it.
class SceneBox {
  SceneBox(this.box, this.node, this.chips, this.parent);

  final MapBox box;

  /// Its node's event index, or 0 when it isn't a node (the vault).
  final int node;
  final List<int> chips;

  /// For a folder, its project's box; else -1.
  final int parent;
}

/// A pipe: a pad at its project's box's top right.
class ScenePad {
  ScenePad(this.node, this.box, this.rect, this.label);

  final int node;
  final int box;
  final Rect rect;
  final String label;
}

/// Branch points: a dot shows where three or more shown lines meet.
class SceneDot {
  SceneDot(this.dot, this.wires, this.weights);

  final MapDot dot;
  final List<int> wires;

  /// 1 for a line that ends at the dot, 2 for one that runs through it.
  final List<int> weights;
}

/// What the board shows at one moment.
class SceneFrame {
  SceneFrame(int chips, int wires, int boxes, int pads, int dots)
    : chips = Uint8List(chips),
      wires = Uint8List(wires),
      boxes = Uint8List(boxes),
      pads = Uint8List(pads),
      dots = Uint8List(dots),
      boxRects = List.filled(boxes, Rect.zero),
      labels = List.filled(chips, ''),
      details = List.filled(chips, '');

  final Uint8List chips;
  final Uint8List wires;
  final Uint8List boxes;
  final Uint8List pads;
  final Uint8List dots;

  /// Each box drawn tight round what's in it now (its own margins kept),
  /// rather than as laid out for everyone who was ever there: chips stay
  /// put, and only a box's edges move as they come and go.
  final List<Rect> boxRects;
  final List<String> labels;
  final List<String> details;

  /// The same picture: what changes from frame to frame (pulses, glows)
  /// isn't in it.
  bool sameAs(SceneFrame? other) {
    if (other == null) return false;
    bool same(Uint8List a, Uint8List b) {
      if (a.length != b.length) return false;
      for (var i = 0; i < a.length; i++) {
        if (a[i] != b[i]) return false;
      }
      return true;
    }

    bool sameText(List<String> a, List<String> b) {
      for (var i = 0; i < a.length; i++) {
        if (a[i] != b[i]) return false;
      }
      return true;
    }

    return same(chips, other.chips) &&
        same(wires, other.wires) &&
        same(boxes, other.boxes) &&
        same(pads, other.pads) &&
        same(dots, other.dots) &&
        _sameRects(boxRects, other.boxRects) &&
        sameText(labels, other.labels) &&
        sameText(details, other.details);
  }
}

bool _sameRects(List<Rect> a, List<Rect> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}

/// A pad's height, and its width with no name.
const padHeight = 20.0;
const _padBare = 4.0 + 12 + 4 + 8 + 2;
const _padMaxWidth = 150.0;

/// The corner of a board with its name, which the camera keeps in view:
/// a chip's width and the header's height (map_layout.dart's cells).
const _nameWidth = 24 * mapCell;
const _nameHeight = 6 * mapCell;

/// What a project keeps round a folder in it.
const _folderGap = 2 * mapCell;

class HistoryScene {
  HistoryScene._(
    this.timeline,
    this.layout,
    this.chips,
    this.wires,
    this.boxes,
    this.pads,
    this.dots,
    this._chipsOf,
    this._copies,
  );

  /// Lays out every node and edge [timeline] has, as the Map would, in
  /// [order] (the sidebar's); [measure] gives a pad's name's width.
  factory HistoryScene.build(
    HistoryTimeline timeline, {
    Map<String, int> order = const {},
    double Function(String text)? measure,
  }) {
    final (map, copies, placeOf) = unionMap(timeline);
    final ordered = {...order};
    for (final MapEntry(key: copy, value: origin) in copies.entries) {
      final rank = order[origin];
      if (rank != null) ordered[copy] = rank;
    }
    final layout = layoutMap(map, order: ordered, you: true);

    final chips = <SceneChip>[];
    final chipOf = <String, int>{};
    final chipsOf = <int, List<int>>{};
    for (final chip in layout.chips) {
      final node = timeline.indexOf[copies[chip.id] ?? chip.id] ?? 0;
      chipOf[chip.id] = chips.length;
      (chipsOf[node] ??= []).add(chips.length);
      chips.add(SceneChip(chip, node, placeOf[chip.id] ?? -1));
    }

    // Edges between two nodes, by kind, for the lines that draw them.
    final between = <String, List<int>>{};
    for (var i = 0; i < timeline.edges.length; i++) {
      final edge = timeline.edges[i].edge;
      final a = timeline.indexOf[edge.fromId] ?? -1;
      final b = timeline.indexOf[edge.toId] ?? -1;
      (between[_pairKey(a, b, _layerOf(edge.kind))] ??= []).add(i);
    }

    int at(Offset point) {
      for (var i = 0; i < chips.length; i++) {
        if (chips[i].rect.inflate(0.5).contains(point)) return i;
      }
      return -1;
    }

    final wires = <SceneWire>[];
    // A routed net's lines share one set of ends, its stub (from its root)
    // first: group them by it.
    final groups = <Set<String>, List<int>>{};
    for (var i = 0; i < layout.wires.length; i++) {
      // A bus's spines share theirs too, but aren't a routed net.
      if (layout.wires[i].bus) continue;
      (groups[layout.wires[i].ends] ??= []).add(i);
    }
    final rootOf = <int, int>{};
    for (final group in groups.values) {
      if (group.length < 2) continue;
      final root = at(layout.wires[group.first].points.first);
      for (final i in group) {
        rootOf[i] = root;
      }
    }
    for (var i = 0; i < layout.wires.length; i++) {
      final wire = layout.wires[i];
      final ends = [for (final id in wire.ends) ?chipOf[id]];
      final root = rootOf[i];
      if (root != null) {
        final own = at(wire.points.first);
        if (own == root || own < 0 || root < 0) {
          // The stub from its root: shown with any of its branches.
          wires.add(SceneWire._(wire, WireRule.trunk, root, -1, [], ends));
          continue;
        }
        if (wire.layer == MapLayer.hierarchy) {
          wires.add(SceneWire._(wire, WireRule.both, own, root, [], ends));
          continue;
        }
        final edges =
            between[_pairKey(chips[own].node, chips[root].node, wire.layer)] ??
            const <int>[];
        wires.add(
          SceneWire._(
            wire,
            edges.isEmpty ? WireRule.both : WireRule.edges,
            own,
            root,
            edges,
            ends,
          ),
        );
        continue;
      }
      // The bus: a tap from a chip, or a trunk.
      final first = at(wire.points.first);
      final own = first >= 0 ? first : at(wire.points.last);
      if (!wire.bus && own >= 0) {
        wires.add(SceneWire._(wire, WireRule.chip, own, -1, [], ends));
      } else {
        wires.add(SceneWire._(wire, WireRule.trunk, -1, -1, [], ends));
      }
    }
    // Stubs are shown with their net's branches: point them at them.
    for (final group in groups.values) {
      if (group.length < 2) continue;
      final branches = [
        for (final i in group.skip(1))
          if (wires[i].rule != WireRule.trunk) i,
      ];
      final stub = wires[group.first];
      if (stub.rule == WireRule.trunk && stub.a >= 0) {
        wires[group.first] = SceneWire._(
          stub.wire,
          WireRule.trunk,
          stub.a,
          -1,
          branches,
          stub.ends,
        );
      }
    }

    final boxes = <SceneBox>[];
    for (final box in layout.boxes) {
      boxes.add(
        SceneBox(box, timeline.indexOf[box.id] ?? 0, [
          for (var i = 0; i < chips.length; i++)
            if (box.rect.contains(chips[i].rect.center)) i,
        ], -1),
      );
    }
    for (var i = 0; i < boxes.length; i++) {
      final box = boxes[i];
      if (box.box.kind != MapBoxKind.folder) continue;
      final parent = boxes.indexWhere(
        (other) =>
            other.box.kind == MapBoxKind.project &&
            other.box.rect.contains(box.box.rect.center),
      );
      boxes[i] = SceneBox(box.box, box.node, box.chips, parent);
    }

    final dots = <SceneDot>[];
    for (final dot in layout.dots) {
      final touching = <int>[];
      final weights = <int>[];
      for (var i = 0; i < layout.wires.length; i++) {
        final weight = _touches(layout.wires[i].points, dot.at);
        if (weight > 0) {
          touching.add(i);
          weights.add(weight);
        }
      }
      dots.add(SceneDot(dot, touching, weights));
    }

    final pads = _pads(timeline, boxes, measure ?? (text) => text.length * 7.0);
    return HistoryScene._(
      timeline,
      layout,
      chips,
      wires,
      boxes,
      pads,
      dots,
      chipsOf,
      copies,
    );
  }

  final HistoryTimeline timeline;
  final MapLayout layout;
  final List<SceneChip> chips;
  final List<SceneWire> wires;
  final List<SceneBox> boxes;
  final List<ScenePad> pads;
  final List<SceneDot> dots;
  final Map<int, List<int>> _chipsOf;

  /// A moved agent's chips' ids, each to its agent's.
  final Map<String, String> _copies;

  /// The node a chip stands for ("agent:…"), for a moved agent's chips
  /// too.
  String originOf(String chipId) => _copies[chipId] ?? chipId;

  /// The chip [node] (an event index) has at [t]: for a moved agent, the
  /// one where it worked then; -1 when it has none.
  int chipAt(int node, double t) {
    final list = _chipsOf[node];
    if (list == null || list.isEmpty) return -1;
    if (list.length == 1) return list.first;
    var best = list.first;
    var bestFrom = double.negativeInfinity;
    for (final i in list) {
      final place = chips[i].place;
      if (place < 0) continue;
      for (final span in timeline.edgeSpans[place]) {
        if (span.contains(t)) return i;
        if (span.from <= t && span.from > bestFrom) {
          bestFrom = span.from;
          best = i;
        }
      }
    }
    return best;
  }

  /// Where [node]'s pad is, for a pipe; -1 for none.
  int padOf(int node) => pads.indexWhere((pad) => pad.node == node);

  /// What shows at [t].
  SceneFrame at(double t) {
    final frame = SceneFrame(
      chips.length,
      wires.length,
      boxes.length,
      pads.length,
      dots.length,
    );
    final before = t < timeline.recordingSince;
    int state(List<Span> spans) {
      final span = spanAt(spans, t);
      if (span == null) return hidden;
      return span.inferred && before ? dotted : shown;
    }

    for (final MapEntry(key: node, value: list) in _chipsOf.entries) {
      if (node <= 0) {
        // Not a node History knows: there all along.
        for (final i in list) {
          frame.chips[i] = shown;
        }
        continue;
      }
      final present = state(timeline.nodeSpans[node - 1]);
      if (present == hidden) continue;
      final i = list.length == 1 ? list.first : chipAt(node, t);
      frame.chips[i] = present;
      final label = timeline.labelAt(node, t);
      frame.labels[i] = label.label;
      frame.details[i] = label.detail;
    }
    for (var i = 0; i < chips.length; i++) {
      if (frame.labels[i].isEmpty) {
        frame.labels[i] = chips[i].chip.node.label;
        frame.details[i] = chips[i].chip.node.detail;
      }
    }

    for (var i = 0; i < wires.length; i++) {
      final wire = wires[i];
      frame.wires[i] = switch (wire.rule) {
        WireRule.edges => _edgeState(wire, frame, state),
        WireRule.both => _both(frame.chips[wire.a], frame.chips[wire.b]),
        WireRule.chip =>
          frame.chips[wire.a] == hidden ||
                  !wire.ends.any((end) => end != wire.a && frame.chips[end] > 0)
              ? hidden
              : frame.chips[wire.a],
        WireRule.trunk => hidden,
      };
    }
    for (var i = 0; i < wires.length; i++) {
      final wire = wires[i];
      if (wire.rule != WireRule.trunk) continue;
      if (wire.a >= 0) {
        // A net's stub: with its branches.
        var best = hidden;
        for (final branch in wire.edges) {
          final s = frame.wires[branch];
          if (s == shown) {
            best = shown;
            break;
          }
          if (s == dotted) best = dotted;
        }
        frame.wires[i] = best;
        continue;
      }
      var count = 0;
      var solid = false;
      for (final end in wire.ends) {
        if (frame.chips[end] == hidden) continue;
        count++;
        if (frame.chips[end] == shown) solid = true;
      }
      frame.wires[i] = count < 2
          ? hidden
          : solid
          ? shown
          : dotted;
    }

    for (var i = 0; i < boxes.length; i++) {
      final box = boxes[i];
      var any = hidden;
      for (final chip in box.chips) {
        final s = frame.chips[chip];
        if (s == shown) {
          any = shown;
          break;
        }
        if (s == dotted) any = dotted;
      }
      var s = any;
      if (box.node > 0) {
        final own = state(timeline.nodeSpans[box.node - 1]);
        if (box.box.kind == MapBoxKind.folder) {
          // A folder shows with its project, while it's there.
          s = own;
        } else if (own == hidden) {
          s = hidden;
        } else if (own == dotted && s == shown) {
          s = dotted;
        }
      }
      frame.boxes[i] = s;
    }
    for (var i = 0; i < boxes.length; i++) {
      final parent = boxes[i].parent;
      if (parent >= 0 && frame.boxes[parent] == hidden) frame.boxes[i] = hidden;
    }

    for (var i = 0; i < pads.length; i++) {
      final pad = pads[i];
      frame.pads[i] = frame.boxes[pad.box] == hidden
          ? hidden
          : state(timeline.nodeSpans[pad.node - 1]);
    }
    // Folders first: their projects take them in.
    for (var i = 0; i < boxes.length; i++) {
      if (boxes[i].parent >= 0) frame.boxRects[i] = _tight(i, frame);
    }
    for (var i = 0; i < boxes.length; i++) {
      if (boxes[i].parent < 0) frame.boxRects[i] = _tight(i, frame);
    }

    for (var i = 0; i < dots.length; i++) {
      final dot = dots[i];
      var branches = 0;
      var solid = false;
      for (var k = 0; k < dot.wires.length; k++) {
        final s = frame.wires[dot.wires[k]];
        if (s == hidden) continue;
        branches += dot.weights[k];
        if (s == shown) solid = true;
      }
      frame.dots[i] = branches < 3
          ? hidden
          : solid
          ? shown
          : dotted;
    }
    return frame;
  }

  int _edgeState(
    SceneWire wire,
    SceneFrame frame,
    int Function(List<Span>) state,
  ) {
    final ends = _both(frame.chips[wire.a], frame.chips[wire.b]);
    if (ends == hidden) return hidden;
    var best = hidden;
    for (final edge in wire.edges) {
      final s = state(timeline.edgeSpans[edge]);
      if (s == shown) return ends;
      if (s == dotted) best = dotted;
    }
    return best;
  }

  static int _both(int a, int b) {
    if (a == hidden || b == hidden) return hidden;
    return a == dotted || b == dotted ? dotted : shown;
  }

  /// Each box's margins round its chips, as laid out: what a tight box
  /// keeps.
  late final List<({double l, double t, double r, double b})> _margins = [
    for (final box in boxes) _marginsOf(box),
  ];

  ({double l, double t, double r, double b}) _marginsOf(SceneBox box) {
    final rect = box.box.rect;
    Rect? inner;
    for (final chip in box.chips) {
      inner = inner?.expandToInclude(chips[chip].rect) ?? chips[chip].rect;
    }
    if (inner == null) return (l: 0, t: 0, r: 0, b: 0);
    return (
      l: inner.left - rect.left,
      t: inner.top - rect.top,
      r: rect.right - inner.right,
      b: rect.bottom - inner.bottom,
    );
  }

  /// Each project's folders.
  late final Map<int, List<int>> _folders = () {
    final out = <int, List<int>>{};
    for (var i = 0; i < boxes.length; i++) {
      final parent = boxes[i].parent;
      if (parent >= 0) (out[parent] ??= []).add(i);
    }
    return out;
  }();

  /// Box [i] in [frame], tight round its chips, folders and pads shown
  /// there; just its name's corner with none.
  Rect _tight(int i, SceneFrame frame) {
    final box = boxes[i];
    final whole = box.box.rect;
    final margins = _margins[i];
    Rect? inner;
    for (final chip in box.chips) {
      if (frame.chips[chip] == hidden) continue;
      inner = inner?.expandToInclude(chips[chip].rect) ?? chips[chip].rect;
    }
    Rect? out = inner == null
        ? null
        : Rect.fromLTRB(
            inner.left - margins.l,
            inner.top - margins.t,
            inner.right + margins.r,
            inner.bottom + margins.b,
          );
    void add(Rect rect) => out = out?.expandToInclude(rect) ?? rect;
    for (final folder in _folders[i] ?? const <int>[]) {
      if (frame.boxes[folder] == hidden) continue;
      add(frame.boxRects[folder].inflate(_folderGap));
    }
    for (var p = 0; p < pads.length; p++) {
      final pad = pads[p];
      if (pad.box != i || frame.pads[p] == hidden) continue;
      // On the board's top edge, as laid out.
      add(
        Rect.fromLTRB(
          pad.rect.left - mapCell,
          whole.top,
          pad.rect.right + mapCell,
          pad.rect.bottom,
        ),
      );
    }
    final shape =
        out ??
        Rect.fromLTWH(
          whole.left,
          whole.top,
          math.min(whole.width, _nameWidth),
          math.min(whole.height, _nameHeight),
        );
    // Room for its name, and never past where it was laid out.
    final right = math.max(shape.right, shape.left + _nameWidth);
    final bottom = math.max(shape.bottom, shape.top + _nameHeight);
    return Rect.fromLTRB(
      math.max(shape.left, whole.left),
      math.max(shape.top, whole.top),
      math.min(right, whole.right),
      math.min(bottom, whole.bottom),
    );
  }

  /// Everything shown in [frame]: what the camera frames.
  Rect? bounds(SceneFrame frame) {
    Rect? out = layout.you;
    void add(Rect rect) => out = out?.expandToInclude(rect) ?? rect;
    for (var i = 0; i < chips.length; i++) {
      if (frame.chips[i] != hidden) add(chips[i].rect);
    }
    for (var i = 0; i < boxes.length; i++) {
      if (frame.boxes[i] == hidden) continue;
      final kind = boxes[i].box.kind;
      // The workspace and projects as whole boards, tight as they're
      // drawn; the rest by their chips.
      if (kind == MapBoxKind.workspace || kind == MapBoxKind.project) {
        add(frame.boxRects[i]);
      }
    }
    for (var i = 0; i < pads.length; i++) {
      if (frame.pads[i] != hidden) add(pads[i].rect);
    }
    return out;
  }
}

String _pairKey(int a, int b, MapLayer? layer) =>
    a < b ? '$a|$b|${layer?.index}' : '$b|$a|${layer?.index}';

/// The layer an edge of [kind] is drawn in; null for those drawn as
/// places (works in, reads a folder).
MapLayer? _layerOf(MapEdgeKind kind) => switch (kind) {
  MapEdgeKind.MAP_EDGE_KIND_MESSAGES => MapLayer.messages,
  MapEdgeKind.MAP_EDGE_KIND_LEAD => MapLayer.leads,
  MapEdgeKind.MAP_EDGE_KIND_SECRET => MapLayer.secrets,
  MapEdgeKind.MAP_EDGE_KIND_READS => MapLayer.data,
  MapEdgeKind.MAP_EDGE_KIND_APP ||
  MapEdgeKind.MAP_EDGE_KIND_HEARS => MapLayer.apps,
  _ => null,
};

/// How [points] meets [at]: 1 when it ends there, 2 when it runs through
/// it, 0 when it doesn't.
int _touches(List<Offset> points, Offset at) {
  if (points.isEmpty) return 0;
  if ((points.first - at).distance < 0.5 || (points.last - at).distance < 0.5) {
    return 1;
  }
  for (var i = 0; i + 1 < points.length; i++) {
    final a = points[i];
    final b = points[i + 1];
    final onX =
        (a.dx - at.dx).abs() < 0.5 &&
        (b.dx - at.dx).abs() < 0.5 &&
        at.dy >= (a.dy < b.dy ? a.dy : b.dy) - 0.5 &&
        at.dy <= (a.dy > b.dy ? a.dy : b.dy) + 0.5;
    final onY =
        (a.dy - at.dy).abs() < 0.5 &&
        (b.dy - at.dy).abs() < 0.5 &&
        at.dx >= (a.dx < b.dx ? a.dx : b.dx) - 0.5 &&
        at.dx <= (a.dx > b.dx ? a.dx : b.dx) + 0.5;
    if (onX || onY) return 2;
  }
  return 0;
}

/// Every node and edge [timeline] has, as one map to lay out: pipes left
/// out (they're pads), and an agent that worked in more than one place
/// split into a chip for each ("agent:kai@1"), each with its own place and
/// the agent's other lines. Gives the map, each split chip's agent, and
/// each split chip's place (an index into the timeline's edges).
(ClaktoryMap, Map<String, String>, Map<String, int>) unionMap(
  HistoryTimeline timeline,
) {
  final nodes = <String, MapNode>{};
  for (final node in timeline.nodes) {
    if (node.node.id.isEmpty ||
        node.node.kind == MapNodeKind.MAP_NODE_KIND_PIPE) {
      continue;
    }
    nodes[node.node.id] = node.node;
  }
  // Where each agent worked, in the order it first did.
  final places = <String, List<int>>{};
  for (var i = 0; i < timeline.edges.length; i++) {
    final edge = timeline.edges[i].edge;
    if (edge.kind != MapEdgeKind.MAP_EDGE_KIND_WRITES) continue;
    final from = nodes[edge.fromId];
    final to = nodes[edge.toId];
    if (from == null ||
        to == null ||
        from.kind != MapNodeKind.MAP_NODE_KIND_AGENT ||
        to.kind == MapNodeKind.MAP_NODE_KIND_DATA) {
      continue;
    }
    (places[edge.fromId] ??= []).add(i);
  }
  double first(int edge) {
    final spans = timeline.edgeSpans[edge];
    return spans.isEmpty ? double.infinity : spans.first.from;
  }

  final copies = <String, String>{};
  final placeOf = <String, int>{};
  final split = <String, List<String>>{};
  for (final MapEntry(key: agent, value: list) in places.entries) {
    final distinct = <String, int>{};
    for (final edge in list..sort((a, b) => first(a).compareTo(first(b)))) {
      distinct.putIfAbsent(timeline.edges[edge].edge.toId, () => edge);
    }
    if (distinct.length < 2) continue;
    final ids = <String>[];
    var k = 0;
    for (final edge in distinct.values) {
      final id = k == 0 ? agent : '$agent@$k';
      k++;
      ids.add(id);
      copies[id] = agent;
      placeOf[id] = edge;
    }
    split[agent] = ids;
  }

  final outNodes = <MapNode>[];
  for (final node in nodes.values) {
    final ids = split[node.id];
    final parentSplit = split.containsKey(node.parentId);
    if (ids == null) {
      outNodes.add(parentSplit ? (node.deepCopy()..parentId = '') : node);
      continue;
    }
    for (final id in ids) {
      outNodes.add(
        node.deepCopy()
          ..id = id
          ..parentId = parentSplit ? '' : node.parentId
          ..projectId = _projectOf(nodes, timeline.edges[placeOf[id]!].edge),
      );
    }
  }
  final outEdges = <MapEdge>[];
  final seen = <String>{};
  for (var i = 0; i < timeline.edges.length; i++) {
    final edge = timeline.edges[i].edge;
    if (!nodes.containsKey(edge.fromId) || !nodes.containsKey(edge.toId)) {
      continue;
    }
    if (edge.kind == MapEdgeKind.MAP_EDGE_KIND_WRITES &&
        split.containsKey(edge.fromId)) {
      // Each of its chips works where its place says.
      for (final id in split[edge.fromId]!) {
        if (timeline.edges[placeOf[id]!].edge.toId == edge.toId) {
          _addEdge(outEdges, seen, edge, from: id);
        }
      }
      continue;
    }
    for (final from in split[edge.fromId] ?? [edge.fromId]) {
      for (final to in split[edge.toId] ?? [edge.toId]) {
        _addEdge(outEdges, seen, edge, from: from, to: to);
      }
    }
  }
  return (ClaktoryMap(nodes: outNodes, edges: outEdges), copies, placeOf);
}

void _addEdge(
  List<MapEdge> into,
  Set<String> seen,
  MapEdge edge, {
  required String from,
  String? to,
}) {
  final key = '$from|${to ?? edge.toId}|${edge.kind.value}|${edge.bothWays}';
  if (!seen.add(key)) return;
  into.add(
    from == edge.fromId && (to == null || to == edge.toId)
        ? edge
        : (edge.deepCopy()
            ..fromId = from
            ..toId = to ?? edge.toId),
  );
}

/// The project a place is in ("project:x", "folder:x/f" → "x").
String _projectOf(Map<String, MapNode> nodes, MapEdge writes) {
  final place = nodes[writes.toId];
  if (place == null) return '';
  if (place.kind == MapNodeKind.MAP_NODE_KIND_PROJECT) return place.refId;
  return place.projectId;
}

/// Each pipe's pad on its project's box: right to left along its top, in
/// the order they were made, as the Map has them; past half the box they
/// show only their portals.
List<ScenePad> _pads(
  HistoryTimeline timeline,
  List<SceneBox> boxes,
  double Function(String) measure,
) {
  final pads = <ScenePad>[];
  for (var b = 0; b < boxes.length; b++) {
    final box = boxes[b].box;
    if (box.kind != MapBoxKind.project) continue;
    final here = <int>[
      for (var i = 0; i < timeline.nodes.length; i++)
        if (timeline.nodes[i].node.kind == MapNodeKind.MAP_NODE_KIND_PIPE &&
            timeline.nodes[i].node.parentId == box.id)
          i + 1,
    ];
    if (here.isEmpty) continue;
    double made(int node) {
      final spans = timeline.nodeSpans[node - 1];
      return spans.isEmpty ? double.infinity : spans.first.from;
    }

    here.sort((a, b) {
      final by = made(a).compareTo(made(b));
      return by != 0 ? by : a.compareTo(b);
    });
    final labels = [
      for (final node in here) timeline.nodes[node - 1].node.label,
    ];
    final widths = [
      for (final label in labels)
        (_padBare + measure(label)).clamp(padHeight, _padMaxWidth),
    ];
    final room = box.rect.width / 2;
    final full =
        widths.fold(0.0, (sum, width) => sum + width) +
            4.0 * (here.length - 1) <=
        room;
    var right = box.rect.right - mapCell;
    for (var i = 0; i < here.length; i++) {
      final width = full ? widths[i] : padHeight;
      final left = right - width;
      if (left < box.rect.left + mapCell * 4) break;
      pads.add(
        ScenePad(
          here[i],
          b,
          Rect.fromLTWH(left, box.rect.top + mapCell - 1, width, padHeight),
          full ? labels[i] : '',
        ),
      );
      right = left - 4;
    }
  }
  return pads;
}
