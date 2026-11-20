import 'dart:ui';

import 'package:factory_app/map_layout.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter_test/flutter_test.dart';

import 'map_fixture.dart';

/// Every grid step a wire takes, as "x,y>x,y" (smaller end first).
Iterable<String> _steps(MapWire wire) sync* {
  for (var i = 0; i + 1 < wire.points.length; i++) {
    final a = wire.points[i];
    final b = wire.points[i + 1];
    final n = ((b - a).distance / mapCell).round();
    final step = n == 0 ? Offset.zero : (b - a) / n.toDouble();
    for (var k = 0; k < n; k++) {
      final p = a + step * k.toDouble();
      final q = a + step * (k + 1).toDouble();
      final first = p.dx < q.dx || (p.dx == q.dx && p.dy < q.dy) ? p : q;
      final second = first == p ? q : p;
      yield '${first.dx},${first.dy}>${second.dx},${second.dy}';
    }
  }
}

String _describe(MapLayout layout) => [
  for (final chip in layout.chips) '${chip.id} ${chip.rect}',
  for (final box in layout.boxes) '${box.id} ${box.rect}',
  for (final wire in layout.wires)
    '${wire.layer.name} ${wire.points} ${wire.arrows.map((a) => '${a.at}${a.direction}')}',
  for (final dot in layout.dots) 'dot ${dot.at}',
].join('\n');

void main() {
  test('Matt\'s Claktory lays out with every line routed', () {
    final watch = Stopwatch()..start();
    final layout = layoutMap(claktoryMap(), order: claktoryOrder());
    watch.stop();
    // ignore: avoid_print
    print(
      'layout ${watch.elapsedMilliseconds} ms, ${layout.size}, '
      '${layout.wires.length} wires, ${layout.dots.length} dots',
    );
    expect(layout.chips.length, 16 + 3 + 1);
    // Every secret holder, every nested agent and the loose grant has a
    // line.
    final secrets = layout.wires.where((w) => w.layer == MapLayer.secrets);
    expect(secrets.length, 3 + 5); // a stub per secret, a branch per holder
    final hierarchy = layout.wires.where((w) => w.layer == MapLayer.hierarchy);
    expect(hierarchy.length, 1 + 11);
    final loose = layout.wires.where(
      (w) => w.layer == MapLayer.messages && w.arrows.isNotEmpty,
    );
    // Two drops off the backbone, and Arjun's grant to Disk Scout.
    expect(loose.length, 3);
  });

  for (final (name, map) in [
    ("Matt's", () => layoutMap(claktoryMap(), order: claktoryOrder())),
    ('a crowded', () => layoutMap(crowdedMap())),
  ]) {
    test('$name Claktory: no line runs through a chip, no two lines share '
        'a stretch, and no chips overlap', () {
      _clean(map());
    });
  }

  test('a crowded Claktory keeps every agent, secret and box', () {
    final map = crowdedMap();
    final layout = layoutMap(map);
    final drawn = {for (final chip in layout.chips) chip.id};
    for (final node in map.nodes) {
      if (node.kind == MapNodeKind.MAP_NODE_KIND_AGENT ||
          node.kind == MapNodeKind.MAP_NODE_KIND_SECRET) {
        expect(drawn, contains(node.id));
      }
    }
    // The workspace's agents have a box of their own; the admins sit on top.
    expect(layout.boxes.map((box) => box.id), contains('workspace'));
    final top = layout.chips.firstWhere((chip) => chip.id == 'agent:boss');
    final rivet = layout.chips.firstWhere((chip) => chip.id == 'agent:rivet');
    expect(top.rect.top, rivet.rect.top);
    // Long names don't make chips longer.
    final long = layout.chips.firstWhere((chip) => chip.id == 'secret:long');
    expect(long.rect.width, lessThanOrEqualTo(24 * mapCell));
  });

  test('the same map gives the same picture', () {
    final a = _describe(layoutMap(claktoryMap(), order: claktoryOrder()));
    final shuffled = claktoryMap();
    final nodes = shuffled.nodes.reversed.toList();
    final edges = shuffled.edges.reversed.toList();
    shuffled.nodes
      ..clear()
      ..addAll(nodes);
    shuffled.edges
      ..clear()
      ..addAll(edges);
    expect(_describe(layoutMap(shuffled, order: claktoryOrder())), a);
  });

  test('a project that messages itself is one bus, not every pair', () {
    final layout = layoutMap(claktoryMap(), order: claktoryOrder());
    final buses = layout.wires.where((w) => w.bus).toList();
    // The backbone, and Claktory's rail and its columns' spines; Disk has
    // one agent, and so a tap off the drop, not a bus.
    expect(buses.where((w) => w.ends.contains('agent:disk-scout')).length, 1);
    final claktory = buses.where((w) => w.ends.contains('agent:coordinator'));
    expect(claktory.length, greaterThan(2));
    // Messages inside a project are on its bus, never pair by pair.
    final pairs = layout.wires.where(
      (w) =>
          w.layer == MapLayer.messages &&
          w.ends.contains('agent:builder') &&
          w.ends.contains('agent:frontend') &&
          w.ends.length == 2,
    );
    expect(pairs, isEmpty);
  });

  test('an empty map lays out as nothing', () {
    expect(layoutMap(ClaktoryMap()).size, Size.zero);
  });
}

void _clean(MapLayout layout) {
  for (final wire in layout.wires) {
    for (var i = 0; i + 1 < wire.points.length; i++) {
      final a = wire.points[i];
      final b = wire.points[i + 1];
      expect(a.dx == b.dx || a.dy == b.dy, isTrue, reason: 'right angles');
      final segment = Rect.fromPoints(a, b);
      for (final chip in layout.chips) {
        final inside = chip.rect.deflate(0.5);
        final crosses = segment.width == 0
            ? segment.left > inside.left &&
                  segment.left < inside.right &&
                  segment.top < inside.bottom &&
                  segment.bottom > inside.top
            : segment.top > inside.top &&
                  segment.top < inside.bottom &&
                  segment.left < inside.right &&
                  segment.right > inside.left;
        expect(crosses, isFalse, reason: '${wire.points} through ${chip.id}');
      }
    }
  }
  final owner = <String, MapWire>{};
  for (final wire in layout.wires) {
    for (final step in _steps(wire)) {
      final other = owner[step];
      if (other != null && !identical(other, wire)) {
        // Only the same net may run there twice (a bus and its taps).
        expect(
          other.layer == wire.layer &&
              other.ends.intersection(wire.ends).isNotEmpty,
          isTrue,
          reason: '$step: ${other.points} and ${wire.points}',
        );
      }
      owner[step] = wire;
    }
  }
  for (final a in layout.chips) {
    for (final b in layout.chips) {
      if (identical(a, b)) continue;
      expect(
        a.rect.deflate(1).overlaps(b.rect.deflate(1)),
        isFalse,
        reason: '${a.id} and ${b.id}',
      );
    }
  }
}
