// Lays out the Map (ClaktoryMap, computed by the backend) like a circuit
// board: boxes placed from the containment tree, then every line routed at
// right angles through the gaps between them, each on its own track.
//
// It's a pure function of the map and the sidebar's order, in whole grid
// cells, with every list sorted by a stable key: the same Claktory gives
// the same picture, byte for byte, on every device.
import 'dart:math' as math;
import 'dart:typed_data';
import 'dart:ui' show Offset, Rect, Size;

import 'protocol/factory/v1/factory.pb.dart';

/// One grid cell, in logical pixels.
const mapCell = 8.0;

/// The kinds of line, each drawn in its own color and shown or hidden
/// together.
enum MapLayer { messages, leads, secrets, hierarchy, data, apps }

/// What a box on the board is.
enum MapBoxKind { workspace, project, folder, admins, vault, apps }

/// A rounded region: the workspace or a project (with its title), a folder
/// inside one, the vault, or the apps.
class MapBox {
  const MapBox({
    required this.id,
    required this.kind,
    required this.rect,
    required this.label,
    this.detail = '',
    this.outlined = true,
  });

  /// The node it stands for ("project:…", "folder:…/…", "workspace"), or
  /// "vault" / "admins" / "apps".
  final String id;
  final MapBoxKind kind;
  final Rect rect;
  final String label;
  final String detail;

  /// False for a project's own folder when it has no other: its agents sit
  /// straight in the project's box.
  final bool outlined;
}

/// A node drawn as a chip: an agent, a secret, an app or Claktory's
/// records.
class MapChip {
  const MapChip({required this.node, required this.rect, this.depth = 0});

  final MapNode node;
  final Rect rect;

  /// How deep it's nested under other agents in its folder.
  final int depth;

  String get id => node.id;
}

/// A drawn line: a polyline of right angles.
class MapWire {
  const MapWire({
    required this.layer,
    required this.points,
    required this.ends,
    this.bus = false,
    this.arrows = const [],
  });

  final MapLayer layer;
  final List<Offset> points;

  /// The nodes it connects, for highlighting: hovering one of them lights
  /// it up.
  final Set<String> ends;

  /// A bus or backbone: drawn thicker than the taps off it.
  final bool bus;

  /// Chevrons along it, each at a point, pointing the way a message goes.
  final List<MapArrow> arrows;
}

class MapArrow {
  const MapArrow(this.at, this.direction);

  final Offset at;

  /// A unit step: right (1, 0), down (0, 1), left or up.
  final Offset direction;
}

/// Where a line branches.
class MapDot {
  const MapDot(this.at, this.layer, this.ends);

  final Offset at;
  final MapLayer layer;
  final Set<String> ends;
}

class MapLayout {
  const MapLayout({
    required this.size,
    required this.boxes,
    required this.chips,
    required this.wires,
    required this.dots,
    required this.related,
    required this.reads,
    required this.writes,
    this.you,
  });

  final Size size;

  /// Where the person goes, centred over the admins (or the boxes); null
  /// when the board leaves them out.
  final Rect? you;
  final List<MapBox> boxes;
  final List<MapChip> chips;
  final List<MapWire> wires;
  final List<MapDot> dots;

  /// For each node, the nodes it has an edge with (either way), to light
  /// up with it.
  final Map<String, Set<String>> related;

  /// For each agent, the boxes (folder, project or workspace ids) it reads
  /// and the one it writes.
  final Map<String, Set<String>> reads;
  final Map<String, Set<String>> writes;

  static const empty = MapLayout(
    size: Size.zero,
    boxes: [],
    chips: [],
    wires: [],
    dots: [],
    related: {},
    reads: {},
    writes: {},
  );
}

/// The order things have in the sidebar: projects, each project's folders,
/// and agents. Ids are the map's ("project:…", "folder:p/f", "agent:…").
Map<String, int> sidebarOrder(Workspace workspace) {
  final order = <String, int>{};
  var next = 0;
  for (final project in workspace.projects) {
    order['project:${project.id}'] = next++;
    for (final folder in project.folders) {
      order['folder:${project.id}/${folder.id}'] = next++;
    }
  }
  for (final agent in workspace.agents) {
    order['agent:${agent.id}'] = next++;
  }
  return order;
}

/// The routing grid as text, for debugging a layout: chips and labels are
/// '#', outlines '.', each net's lines a letter, crossings '+'.
String debugMapGrid(ClaktoryMap map, {Map<String, int> order = const {}}) {
  final layout = _Layout(map, order)..run();
  final out = StringBuffer();
  for (var y = 0; y < layout.gh; y++) {
    for (var x = 0; x < layout.gw; x++) {
      final p = layout._p(x, y);
      final net = layout.netAt[p];
      out.write(
        layout.crossAt[p] != -1
            ? '+'
            : net != -1 && layout.blocked[p] != 1
            ? String.fromCharCode(97 + net % 26)
            : layout.blocked[p] == 1
            ? '#'
            : layout.blocked[p] == 2
            ? '.'
            : ' ',
      );
    }
    out.writeln();
  }
  return out.toString();
}

/// Lays out [map]; [order] (see [sidebarOrder]) puts things as the sidebar
/// has them, and anything it doesn't name follows by id.
MapLayout layoutMap(
  ClaktoryMap map, {
  Map<String, int> order = const {},
  bool you = false,
}) {
  if (map.nodes.isEmpty) return MapLayout.empty;
  return _Layout(map, order, you: you).run();
}

// Sizes, in cells.
const _chipW = 24;
const _chipH = 6;
const _rowGap = 3;
const _indent = 2;
const _maxDepth = 3;
const _margin = 4;

/// The person's node over the admins ([layoutMap]'s `you`), and the room
/// it takes above them.
const _youH = 7;
const _youBand = _youH + 3;
const _header = 6;
const _regionLabel = 3;
const _smallH = 4;

// Directions: right, down, left, up.
const _dx = [1, 0, -1, 0];
const _dy = [0, 1, 0, -1];
int _opposite(int d) => (d + 2) % 4;

class _Region {
  _Region(this.id, this.label, this.detail, {required this.outlined});

  final String id;
  final String label;
  final String detail;
  final bool outlined;
  final rows = <(MapNode, int)>[];
  int top = 0;
  int bottom = 0;
}

class _Box {
  _Box(this.id, this.kind, this.label, this.detail);

  final String id;
  final MapBoxKind kind;
  final String label;
  final String detail;
  final regions = <_Region>[];
  int x = 0;
  int y = 0;
  int width = 0;
  int height = 0;
  int gutterLeft = 4;
  int gutterRight = 3;
  int depth = 0;

  /// Agents on its message bus, in order.
  List<MapNode> bus = const [];

  /// The admin agents reach its whole bus: a drop off the backbone.
  bool drop = false;

  /// Where the first tap leaves its spine.
  int firstTap = 0;

  /// How many columns its agents are in, each this many cells wide.
  int columns = 1;
  int columnWidth = 0;

  Iterable<MapNode> get agents =>
      regions.expand((region) => region.rows.map((row) => row.$1));
  int columnX(int column) => x + column * columnWidth;
  int spineOf(int column) => columnX(column) + 2;
  int get spine => x + 2;
  int get rail => y + _header;
  int get chips => x + gutterLeft;
}

/// A chip's place in cells.
class _Cells {
  _Cells(this.x, this.y, this.w, this.h);

  final int x;
  final int y;
  final int w;
  final int h;
}

class _Job {
  _Job(
    this.layer,
    this.net,
    this.name,
    this.rootId,
    this.root,
    this.terminals,
    this.arrows,
  );

  final MapLayer layer;
  final int net;
  final String name;
  final String rootId;
  final _Port root;
  final List<(String, _Port)> terminals;
  final int arrows;
}

/// A point a line leaves a chip from: on its edge, stepping out [dir].
class _Port {
  _Port(this.x, this.y, this.dir);

  final int x;
  final int y;
  final int dir;
  int get sx => x + _dx[dir];
  int get sy => y + _dy[dir];
}

class _Layout {
  _Layout(this.map, this.order, {this.you = false});

  /// Whether the person has a node over the admins.
  final bool you;

  /// Where the admins (or the boxes) start: under the person's node.
  int get _top => _margin + (you ? _youBand : 0);

  _Cells? youCells;

  final ClaktoryMap map;
  final Map<String, int> order;

  final nodes = <String, MapNode>{};
  final cells = <String, _Cells>{};
  final depthOf = <String, int>{};
  final columnOf = <String, int>{};
  final boxes = <_Box>[];
  final outBoxes = <MapBox>[];
  final wires = <MapWire>[];
  final dots = <MapDot>[];
  List<MapNode> admins = const [];
  List<MapNode> secrets = const [];
  List<MapNode> apps = const [];
  MapNode? records;

  int compare(String a, String b) {
    final ra = order[a] ?? 1 << 30;
    final rb = order[b] ?? 1 << 30;
    return ra != rb ? ra.compareTo(rb) : a.compareTo(b);
  }

  List<MapNode> sorted(Iterable<MapNode> list) =>
      list.toList()..sort((a, b) => compare(a.id, b.id));

  List<MapEdge> edgesOf(MapEdgeKind kind) =>
      [
        for (final edge in map.edges)
          if (edge.kind == kind) edge,
      ]..sort((a, b) {
        final from = compare(a.fromId, b.fromId);
        return from != 0 ? from : compare(a.toId, b.toId);
      });

  /// The nodes of [kind], by name.
  List<MapNode> _byLabel(MapNodeKind kind) =>
      map.nodes.where((node) => node.kind == kind).toList()..sort((a, b) {
        final byName = a.label.toLowerCase().compareTo(b.label.toLowerCase());
        return byName != 0 ? byName : a.id.compareTo(b.id);
      });

  MapLayout run() {
    for (final node in map.nodes) {
      nodes[node.id] = node;
    }
    final agents = sorted(
      map.nodes.where((node) => node.kind == MapNodeKind.MAP_NODE_KIND_AGENT),
    );
    admins = [
      for (final agent in agents)
        if (agent.admin) agent,
    ];
    secrets = _byLabel(MapNodeKind.MAP_NODE_KIND_SECRET);
    apps = _byLabel(MapNodeKind.MAP_NODE_KIND_APP);
    records = nodes['data:records'];

    // Where each agent works: the node it writes that's a place.
    final place = <String, String>{};
    for (final edge in edgesOf(MapEdgeKind.MAP_EDGE_KIND_WRITES)) {
      final to = nodes[edge.toId];
      if (to == null || to.kind == MapNodeKind.MAP_NODE_KIND_DATA) continue;
      place.putIfAbsent(edge.fromId, () => edge.toId);
    }

    _boxes(agents, place);
    _messaging();
    _place();
    return _route();
  }

  // Step 1: what goes in which box.

  void _boxes(List<MapNode> agents, Map<String, String> place) {
    final projects = sorted(
      map.nodes.where((node) => node.kind == MapNodeKind.MAP_NODE_KIND_PROJECT),
    );
    final regionOf = <String, _Region>{};
    final workspace = _Box(
      'workspace',
      MapBoxKind.workspace,
      nodes['workspace']?.label ?? 'Workspace',
      nodes['workspace']?.detail ?? '',
    );
    workspace.regions.add(_Region('workspace', '', '', outlined: false));
    regionOf['workspace'] = workspace.regions.first;
    final projectBoxes = <_Box>[];
    for (final project in projects) {
      final box = _Box(
        project.id,
        MapBoxKind.project,
        project.label,
        project.detail,
      );
      final folders = sorted(
        map.nodes.where(
          (node) =>
              node.kind == MapNodeKind.MAP_NODE_KIND_FOLDER &&
              node.parentId == project.id,
        ),
      );
      final own = _Region(
        project.id,
        'Project folder',
        project.detail,
        outlined: folders.isNotEmpty,
      );
      box.regions.add(own);
      regionOf[project.id] = own;
      for (final folder in folders) {
        final region = _Region(
          folder.id,
          folder.label,
          folder.detail,
          outlined: true,
        );
        box.regions.add(region);
        regionOf[folder.id] = region;
      }
      projectBoxes.add(box);
    }

    // Each region's agents as a tree: an agent under its parent when the
    // parent works there too, else at the region's top level.
    final members = <_Region, List<MapNode>>{};
    for (final agent in agents) {
      if (agent.admin) continue;
      final region =
          regionOf[place[agent.id]] ??
          regionOf['project:${agent.projectId}'] ??
          regionOf['workspace']!;
      (members[region] ??= []).add(agent);
    }
    for (final MapEntry(key: region, value: list) in members.entries) {
      final here = {for (final agent in list) agent.id};
      void add(MapNode agent, int depth) {
        region.rows.add((agent, depth));
        depthOf[agent.id] = depth;
        for (final child in list) {
          if (child.parentId == agent.id) {
            add(child, math.min(depth + 1, _maxDepth));
          }
        }
      }

      for (final agent in list) {
        if (!here.contains(agent.parentId)) add(agent, 0);
      }
    }

    // A project's own folder shows only when agents work there, or when
    // it's all the project has.
    for (final box in projectBoxes) {
      final own = box.regions.first;
      if (own.rows.isEmpty && box.regions.length > 1) box.regions.remove(own);
    }
    if (workspace.agents.isNotEmpty) boxes.add(workspace);
    boxes.addAll(projectBoxes);
  }

  // Which messages a bus or the backbone says, and which need their own
  // line.

  final mutual = <String>{};
  final oneWay = <String>{};
  final loose = <MapEdge>[];

  static String _pair(String a, String b) =>
      a.compareTo(b) < 0 ? '$a|$b' : '$b|$a';

  void _messaging() {
    final edges = edgesOf(MapEdgeKind.MAP_EDGE_KIND_MESSAGES);
    for (final edge in edges) {
      if (edge.bothWays) {
        mutual.add(_pair(edge.fromId, edge.toId));
      } else {
        oneWay.add('${edge.fromId}>${edge.toId}');
      }
    }
    bool reaches(String from, String to) =>
        oneWay.contains('$from>$to') || mutual.contains(_pair(from, to));

    for (final box in boxes) {
      // The largest set that all message each other: drop whoever has the
      // fewest partners until the rest do.
      final bus = box.agents.toList();
      while (bus.length > 1) {
        var worst = -1;
        var fewest = 1 << 30;
        for (var i = 0; i < bus.length; i++) {
          var partners = 0;
          for (final other in bus) {
            if (other != bus[i] &&
                mutual.contains(_pair(bus[i].id, other.id))) {
              partners++;
            }
          }
          if (partners <= fewest) {
            fewest = partners;
            worst = i;
          }
        }
        if (fewest == bus.length - 1) break;
        bus.removeAt(worst);
      }
      if (bus.length == 1 && box.agents.length > 1 && admins.isEmpty) {
        bus.clear();
      }
      box.bus = bus;
      box.drop =
          admins.isNotEmpty &&
          bus.isNotEmpty &&
          bus.every(
            (agent) => admins.every((admin) => reaches(admin.id, agent.id)),
          );
    }

    bool explained(MapEdge edge) {
      for (final box in boxes) {
        final onBus = box.bus.map((agent) => agent.id).toSet();
        if (box.bus.length > 1 &&
            edge.bothWays &&
            onBus.contains(edge.fromId) &&
            onBus.contains(edge.toId)) {
          return true;
        }
        if (box.drop &&
            !edge.bothWays &&
            admins.any((admin) => admin.id == edge.fromId) &&
            onBus.contains(edge.toId)) {
          return true;
        }
      }
      return false;
    }

    loose.addAll(
      edges.where(
        (edge) =>
            nodes.containsKey(edge.fromId) &&
            nodes.containsKey(edge.toId) &&
            !explained(edge),
      ),
    );
  }

  // Step 2: placing boxes and chips, in cells.

  int boardWidth = 0;
  int boardHeight = 0;
  int backboneY = 0;
  final rowTops = <int>[];
  int vaultTop = 0;

  List<String> _children(String id) => [
    for (final node in map.nodes)
      if (node.kind == MapNodeKind.MAP_NODE_KIND_AGENT && node.parentId == id)
        node.id,
  ];

  late final List<MapEdge> secretEdges = edgesOf(
    MapEdgeKind.MAP_EDGE_KIND_SECRET,
  );
  late final List<MapEdge> appEdges = edgesOf(MapEdgeKind.MAP_EDGE_KIND_APP);

  /// From each agent with a lead to its lead.
  late final List<MapEdge> leadEdges = edgesOf(MapEdgeKind.MAP_EDGE_KIND_LEAD);
  late final List<MapEdge> hearEdges = edgesOf(MapEdgeKind.MAP_EDGE_KIND_HEARS);
  late final List<MapEdge> recordReads = [
    for (final edge in edgesOf(MapEdgeKind.MAP_EDGE_KIND_READS))
      if (edge.toId == 'data:records') edge,
  ];

  void _place() {
    // Room in each box's gutters: a column for each line that leaves its
    // chips there.
    final boxOf = <String, _Box>{};
    for (final box in boxes) {
      for (final agent in box.agents) {
        boxOf[agent.id] = box;
      }
    }
    final looseIn = <_Box, int>{};
    for (final edge in loose) {
      for (final id in {edge.fromId, edge.toId}) {
        final box = boxOf[id];
        if (box != null) looseIn[box] = (looseIn[box] ?? 0) + 1;
      }
    }
    final rightIn = <_Box, Set<String>>{};
    void right(String agent, String net) {
      final box = boxOf[agent];
      if (box != null) (rightIn[box] ??= {}).add(net);
    }

    for (final node in map.nodes) {
      if (node.kind != MapNodeKind.MAP_NODE_KIND_AGENT) continue;
      final children = _children(node.id);
      if (children.isEmpty) continue;
      right(node.id, 'h:${node.id}');
      for (final child in children) {
        right(child, 'h:${node.id}');
      }
    }
    for (final edge in leadEdges) {
      right(edge.fromId, 'l:${edge.toId}');
      right(edge.toId, 'l:${edge.toId}');
    }
    for (final edge in [
      ...secretEdges,
      ...recordReads,
      ...appEdges,
      ...hearEdges,
    ]) {
      right(edge.toId, edge.fromId);
      right(edge.fromId, edge.toId);
    }

    for (final box in boxes) {
      box.gutterLeft = 4 + math.min(looseIn[box] ?? 0, 4);
      box.gutterRight = 3 + math.min(rightIn[box]?.length ?? 0, 4);
      box.depth = box.agents.fold(
        0,
        (deepest, agent) => math.max(deepest, depthOf[agent.id] ?? 0),
      );
    }

    final int crossing = math.min(
      loose.length +
          {for (final edge in leadEdges) edge.toId}.length +
          secrets.length +
          apps.length +
          (this.records == null ? 0 : 1),
      10,
    );
    final int gapX = 5 + crossing;
    final int channel = 5 + math.min(loose.length, 8);

    // The admins along the top, over the backbone.
    var y = _top;
    if (admins.isNotEmpty) {
      y += _chipH;
      backboneY = y + 3;
      y = backboneY + channel;
    }

    // Boxes left to right, four to a row.
    // Wide rather than tall, as screens are: about three boxes across for
    // every two down.
    final perRow = math.max(
      1,
      math.min(boxes.length, math.sqrt(boxes.length * 1.5).ceil()),
    );
    var right0 = 0;
    for (var start = 0; start < boxes.length; start += perRow) {
      final row = boxes.sublist(start, math.min(start + perRow, boxes.length));
      rowTops.add(y);
      var x = _margin + 2;
      var bottom = y;
      for (final box in row) {
        box.x = x;
        box.y = y;
        _fill(box);
        x += box.width + gapX;
        bottom = math.max(bottom, box.y + box.height);
      }
      right0 = math.max(right0, x - gapX);
      y = bottom + channel + 1;
    }

    // The vault and Claktory's records along the bottom.
    vaultTop = y + math.min(secrets.length, 6);
    var x = _margin + 2;
    var bottom = vaultTop;
    if (secrets.isNotEmpty) {
      final top = vaultTop + _regionLabel + 2;
      var cx = x + 2;
      var cy = top;
      final limit = math.max(right0, 80);
      for (final secret in secrets) {
        final w = math.max(
          12,
          math.min(_chipW, ((secret.label.length * 7 + 44) / mapCell).ceil()),
        );
        if (cx + w > limit && cx > x + 2) {
          cx = x + 2;
          cy += _smallH + _rowGap;
        }
        cells[secret.id] = _Cells(cx, cy, w, _smallH);
        cx += w + 3;
      }
      final vaultRight = cells.values
          .where((c) => c.h == _smallH)
          .fold(0, (r, c) => math.max(r, c.x + c.w));
      final vaultBottom = cy + _smallH + 2;
      outBoxes.add(
        MapBox(
          id: 'vault',
          kind: MapBoxKind.vault,
          rect: _rect(x, vaultTop, vaultRight + 2 - x, vaultBottom - vaultTop),
          label: 'Secrets',
        ),
      );
      x = vaultRight + 2 + gapX;
      bottom = vaultBottom;
    }
    final records = this.records;
    if (records != null) {
      final w = math.max(
        16,
        math.min(_chipW, ((records.label.length * 7 + 44) / mapCell).ceil()),
      );
      cells[records.id] = _Cells(x, vaultTop + _regionLabel + 2, w, _smallH);
      x += w + gapX;
      bottom = math.max(bottom, vaultTop + _regionLabel + 2 + _smallH);
    }
    // The apps beside them, along the board's bottom edge: the ways out of
    // Claktory, each wired to the agents it's given to.
    if (apps.isNotEmpty) {
      final top = vaultTop + _regionLabel + 2;
      var cx = x + 2;
      for (final app in apps) {
        final w = math.max(
          12,
          math.min(_chipW, ((app.label.length * 7 + 44) / mapCell).ceil()),
        );
        cells[app.id] = _Cells(cx, top, w, _smallH);
        cx += w + 3;
      }
      final appsBottom = top + _smallH + 2;
      outBoxes.add(
        MapBox(
          id: 'apps',
          kind: MapBoxKind.apps,
          rect: _rect(x, vaultTop, cx - 1 - x, appsBottom - vaultTop),
          label: 'Apps',
        ),
      );
      x = cx - 1;
      bottom = math.max(bottom, appsBottom);
    } else if (records != null) {
      x -= gapX;
    }
    right0 = math.max(right0, x);

    // The admins centred over the boxes.
    if (admins.isNotEmpty) {
      final width = admins.length * _chipW + (admins.length - 1) * 4;
      var ax = math.max(
        _margin + 2,
        _margin + 2 + (right0 - _margin - 2 - width) ~/ 2,
      );
      for (final admin in admins) {
        cells[admin.id] = _Cells(ax, _top, _chipW, _chipH);
        ax += _chipW + 4;
      }
      right0 = math.max(right0, ax - 4);
    }

    // The person, centred over the admins, or over the boxes.
    if (you) {
      final first = admins.isEmpty ? null : cells[admins.first.id];
      final last = admins.isEmpty ? null : cells[admins.last.id];
      final center = first != null && last != null
          ? (first.x + last.x + last.w) ~/ 2
          : (_margin + 2 + right0) ~/ 2;
      youCells = _Cells(
        math.max(_margin + 2, center - _chipW ~/ 2),
        _margin,
        _chipW,
        _youH,
      );
      right0 = math.max(right0, youCells!.x + _chipW);
    }

    boardWidth = right0 + _margin + 2;
    boardHeight = bottom + _margin + 2;
  }

  /// Lays out [box]'s regions and chips from its top left.
  void _fill(_Box box) {
    // A box with many agents spreads them over columns, so the board is
    // wider than tall, as screens are. Each region fills its columns top to
    // bottom, in sidebar order.
    final count = box.agents.length;
    box.columns = count <= 3 ? 1 : math.min(4, math.sqrt(count / 1.5).ceil());
    box.columnWidth =
        box.gutterLeft + box.depth * _indent + _chipW + 1 + box.gutterRight;
    // The rail along the top that joins the columns' spines.
    var y = box.y + _header + (box.columns > 1 ? 2 : 0);
    for (final region in box.regions) {
      region.top = y;
      final top = y + (region.outlined ? _regionLabel : 1);
      final perColumn = math.max(1, (region.rows.length / box.columns).ceil());
      var longest = 0;
      for (final (i, (agent, depth)) in region.rows.indexed) {
        final column = i ~/ perColumn;
        final row = i % perColumn;
        longest = math.max(longest, row + 1);
        columnOf[agent.id] = column;
        cells[agent.id] = _Cells(
          box.columnX(column) + box.gutterLeft + depth * _indent,
          top + row * (_chipH + _rowGap),
          _chipW,
          _chipH,
        );
      }
      region.bottom = region.rows.isEmpty
          ? y + (region.outlined ? _regionLabel + 1 : 3)
          : top +
                longest * (_chipH + _rowGap) -
                _rowGap +
                (region.outlined ? 2 : 1);
      y = region.bottom + 2;
    }
    final chipsRight =
        box.columnX(box.columns - 1) +
        box.gutterLeft +
        box.depth * _indent +
        _chipW;
    box.width = chipsRight + 1 + box.gutterRight - box.x;
    box.height = y - box.y;
    outBoxes.add(
      MapBox(
        id: box.id,
        kind: box.kind,
        rect: _rect(box.x, box.y, box.width, box.height),
        label: box.label,
        detail: box.detail,
      ),
    );
    for (final region in box.regions) {
      outBoxes.add(
        MapBox(
          id: region.id,
          kind: MapBoxKind.folder,
          rect: _rect(
            box.chips - 1,
            region.top,
            chipsRight + 2 - box.chips,
            region.bottom - region.top,
          ),
          label: region.label,
          detail: region.detail,
          outlined: region.outlined,
        ),
      );
    }
  }

  static Rect _rect(int x, int y, int w, int h) =>
      Rect.fromLTWH(x * mapCell, y * mapCell, w * mapCell, h * mapCell);

  // Step 3: routing on the grid.

  late int gw;
  late int gh;
  late Int32List netAt;
  late Int8List maskAt;
  late Int32List crossAt;
  late Int32List reserved;
  late Uint8List blocked;
  final netNames = <String>[];
  final netIds = <String, int>{};

  int _p(int x, int y) => y * gw + x;
  bool _in(int x, int y) => x >= 0 && y >= 0 && x < gw && y < gh;

  int _net(String name) =>
      netIds.putIfAbsent(name, () => (netNames..add(name)).length - 1);

  /// Chips' ports, by chip and side, as they're taken.
  final taken = <String, Set<int>>{};

  _Port _port(String id, int side) {
    final c = cells[id]!;
    final used = taken.putIfAbsent('$id:$side', () => {});
    if (side == 0 || side == 2) {
      // Left and right: rows inside the chip, middle first.
      final rows = [
        c.h ~/ 2,
        for (var i = 1; i < c.h; i++)
          if (i != c.h ~/ 2) i,
      ];
      for (final row in rows) {
        if (used.add(row)) {
          return _Port(
            side == 0 ? c.x + c.w : c.x,
            c.y + row,
            side == 0 ? 0 : 2,
          );
        }
      }
      // Out of rows: the bottom edge, from the side's end.
      return _port(id, 1);
    }
    final columns = [
      if (side == 3) c.w ~/ 2,
      for (
        var i = side == 1 ? c.w - 2 : 2;
        i > 0 && i < c.w;
        i += side == 1 ? -2 : 2
      )
        i,
    ];
    for (final column in columns) {
      if (used.add(column)) {
        return _Port(
          c.x + column,
          side == 1 ? c.y + c.h : c.y,
          side == 1 ? 1 : 3,
        );
      }
    }
    return _Port(
      c.x + c.w ~/ 2,
      side == 1 ? c.y + c.h : c.y,
      side == 1 ? 1 : 3,
    );
  }

  /// Draws a fixed line through [points] (a bus), marking the grid so
  /// routes go around or across it.
  void _lay(int net, List<(int, int)> points) {
    for (var i = 0; i + 1 < points.length; i++) {
      final (ax, ay) = points[i];
      final (bx, by) = points[i + 1];
      final d = bx > ax
          ? 0
          : by > ay
          ? 1
          : bx < ax
          ? 2
          : 3;
      var x = ax;
      var y = ay;
      while (x != bx || y != by) {
        _mark(net, x, y, d);
        x += _dx[d];
        y += _dy[d];
      }
    }
  }

  /// Each net's extent so far, as [left, top, right, bottom]: a search
  /// for it is never shorter than the way to this box.
  final bounds = <int, List<int>>{};

  /// Marks a step from (x, y) in direction [d].
  void _mark(int net, int x, int y, int d) {
    final nx = x + _dx[d];
    final ny = y + _dy[d];
    if (!_in(x, y) || !_in(nx, ny)) return;
    for (final (p, bit) in [
      (_p(x, y), 1 << d),
      (_p(nx, ny), 1 << _opposite(d)),
    ]) {
      if (netAt[p] == -1 || netAt[p] == net) {
        netAt[p] = net;
        maskAt[p] |= bit;
        final box = bounds[net] ??= [p % gw, p ~/ gw, p % gw, p ~/ gw];
        box[0] = math.min(box[0], p % gw);
        box[1] = math.min(box[1], p ~/ gw);
        box[2] = math.max(box[2], p % gw);
        box[3] = math.max(box[3], p ~/ gw);
      } else {
        crossAt[p] = net;
      }
    }
  }

  Offset _o(int x, int y) => Offset(x * mapCell, y * mapCell);

  MapLayout _route() {
    gw = boardWidth + 1;
    gh = boardHeight + 1;
    final size = gw * gh;
    netAt = Int32List(size)..fillRange(0, size, -1);
    maskAt = Int8List(size);
    crossAt = Int32List(size)..fillRange(0, size, -1);
    reserved = Int32List(size)..fillRange(0, size, -1);
    blocked = Uint8List(size);

    // Chips are solid; boxes' titles and region labels too, and their
    // outlines cost a little to run along.
    for (final c in cells.values) {
      for (var y = c.y; y <= c.y + c.h; y++) {
        for (var x = c.x; x <= c.x + c.w; x++) {
          if (_in(x, y)) blocked[_p(x, y)] = 1;
        }
      }
    }
    for (final box in outBoxes) {
      final x0 = (box.rect.left / mapCell).round();
      final y0 = (box.rect.top / mapCell).round();
      final x1 = (box.rect.right / mapCell).round();
      final y1 = (box.rect.bottom / mapCell).round();
      if (box.outlined) {
        for (var x = x0; x <= x1; x++) {
          for (final y in [y0, y1]) {
            if (blocked[_p(x, y)] == 0) blocked[_p(x, y)] = 2;
          }
        }
        for (var y = y0; y <= y1; y++) {
          for (final x in [x0, x1]) {
            if (blocked[_p(x, y)] == 0) blocked[_p(x, y)] = 2;
          }
        }
      }
      // Lines come down into the vault to their secret, rather than run
      // along its chips.
      if (box.kind == MapBoxKind.vault || box.kind == MapBoxKind.apps) {
        for (var y = y0 + 1; y < y1; y++) {
          for (var x = x0 + 1; x < x1; x++) {
            if (blocked[_p(x, y)] == 0) blocked[_p(x, y)] = 2;
          }
        }
      }
      // Its label: a box's title and path, a region's name and path. Lines
      // go around the words, not over them.
      if (box.label.isEmpty && box.detail.isEmpty) continue;
      final folder = box.kind == MapBoxKind.folder;
      final small =
          folder || box.kind == MapBoxKind.vault || box.kind == MapBoxKind.apps;
      final labelLeft = small ? x0 + 1 : x0 + _gutterOf(box.id);
      final words = math.max(
        box.label.length * 8,
        folder ? box.label.length * 7 + 12 : box.detail.length * 7,
      );
      final labelRight = math.min(
        x1 - 1,
        labelLeft + (words / mapCell).ceil() + 1,
      );
      final labelRows = small ? 2 : 4;
      for (var y = y0 + 1; y <= y0 + labelRows; y++) {
        for (var x = labelLeft; x < labelRight; x++) {
          blocked[_p(x, y)] = 1;
        }
      }
    }

    _buses();
    _hierarchy();
    _leads();
    _loose();
    _secrets();
    _records();
    _apps();
    // Every port's first step is its net's before anything is routed, so
    // no line blocks a chip that's routed later.
    for (final job in jobs) {
      for (final port in [
        job.root,
        for (final (_, port) in job.terminals) port,
      ]) {
        if (_in(port.sx, port.sy)) reserved[_p(port.sx, port.sy)] = job.net;
      }
    }
    jobs.forEach(_wire);

    // Dots where a net branches.
    final dotEnds = <int, Set<String>>{};
    for (final wire in wires) {
      for (final point in wire.points) {
        final x = (point.dx / mapCell).round();
        final y = (point.dy / mapCell).round();
        if (!_in(x, y)) continue;
        final p = _p(x, y);
        if (_bits(maskAt[p]) >= 3) (dotEnds[p] ??= {}).addAll(wire.ends);
      }
    }
    final layerOf = <int, MapLayer>{};
    for (final wire in wires) {
      for (final point in wire.points) {
        final x = (point.dx / mapCell).round();
        final y = (point.dy / mapCell).round();
        if (_in(x, y)) layerOf.putIfAbsent(_p(x, y), () => wire.layer);
      }
    }
    final dotPoints = dotEnds.keys.toList()..sort();
    for (final p in dotPoints) {
      dots.add(MapDot(_o(p % gw, p ~/ gw), layerOf[p]!, dotEnds[p]!));
    }

    final related = <String, Set<String>>{};
    void relate(String a, String b) {
      (related[a] ??= {}).add(b);
      (related[b] ??= {}).add(a);
    }

    final reads = <String, Set<String>>{};
    final writes = <String, Set<String>>{};
    for (final edge in map.edges) {
      switch (edge.kind) {
        case MapEdgeKind.MAP_EDGE_KIND_READS:
          (reads[edge.fromId] ??= {}).add(edge.toId);
          if (edge.toId == 'data:records') relate(edge.fromId, edge.toId);
        case MapEdgeKind.MAP_EDGE_KIND_WRITES:
          (writes[edge.fromId] ??= {}).add(edge.toId);
        default:
          relate(edge.fromId, edge.toId);
      }
    }
    for (final node in map.nodes) {
      if (node.kind == MapNodeKind.MAP_NODE_KIND_AGENT &&
          nodes[node.parentId]?.kind == MapNodeKind.MAP_NODE_KIND_AGENT) {
        relate(node.id, node.parentId);
      }
    }

    final chips = [
      for (final id in cells.keys.toList()..sort(compare))
        MapChip(
          node: nodes[id]!,
          rect: _rect(cells[id]!.x, cells[id]!.y, cells[id]!.w, cells[id]!.h),
          depth: depthOf[id] ?? 0,
        ),
    ];
    return MapLayout(
      size: Size(boardWidth * mapCell, boardHeight * mapCell),
      boxes: outBoxes,
      chips: chips,
      wires: wires,
      dots: dots,
      related: related,
      reads: reads,
      writes: writes,
      you: youCells == null
          ? null
          : _rect(youCells!.x, youCells!.y, youCells!.w, youCells!.h),
    );
  }

  int _gutterOf(String id) {
    for (final box in boxes) {
      if (box.id == id) return box.gutterLeft;
    }
    return 1;
  }

  static int _bits(int mask) =>
      (mask & 1) + (mask >> 1 & 1) + (mask >> 2 & 1) + (mask >> 3 & 1);

  /// The buses: the backbone under the admins, a drop into each box its
  /// admins reach whole, and each box's spine with a tap to every agent on
  /// it.
  void _buses() {
    final net = _net('bus');
    final adminIds = {for (final admin in admins) admin.id};
    final dropBoxes = [
      for (final box in boxes)
        if (box.drop) box,
    ];
    final reached = {
      for (final box in dropBoxes)
        for (final agent in box.bus) agent.id,
    };

    // Taps and spines first: they sit inside the boxes. Each column has a
    // spine down its left, joined to the others by a rail along the top.
    for (final box in boxes) {
      if (box.bus.isEmpty) continue;
      final ends = {
        for (final agent in box.bus) agent.id,
        if (box.drop) ...adminIds,
      };
      final taps = [
        for (final agent in box.bus) (agent.id, _port(agent.id, 2)),
      ];
      final columns = <int>{
        for (final (id, _) in taps) columnOf[id] ?? 0,
      }.toList()..sort();
      final railed = columns.length > 1 || columns.first != 0;
      box.firstTap = railed ? box.rail : taps.first.$2.y;
      if (railed) {
        final last = box.spineOf(columns.last);
        _lay(net, [(box.spine, box.rail), (last, box.rail)]);
        wires.add(
          MapWire(
            layer: MapLayer.messages,
            points: [_o(box.spine, box.rail), _o(last, box.rail)],
            ends: ends,
            bus: true,
          ),
        );
      }
      for (final column in columns) {
        final mine = [
          for (final (id, port) in taps)
            if ((columnOf[id] ?? 0) == column) (id, port),
        ];
        final x = box.spineOf(column);
        // The first column's spine starts at the box's top edge when the
        // drop comes in there.
        final top = railed
            ? box.rail
            : box.drop
            ? box.y
            : mine.first.$2.y;
        final start = railed ? box.rail : mine.first.$2.y;
        final bottom = mine.last.$2.y;
        if (bottom > top) _lay(net, [(x, top), (x, bottom)]);
        if (bottom > start) {
          wires.add(
            MapWire(
              layer: MapLayer.messages,
              points: [_o(x, start), _o(x, bottom)],
              ends: ends,
              bus: true,
            ),
          );
        }
        for (final (id, port) in mine) {
          _lay(net, [(x, port.y), (port.x, port.y)]);
          wires.add(
            MapWire(
              layer: MapLayer.messages,
              points: [_o(x, port.y), _o(port.x, port.y)],
              ends: {id, ...ends},
            ),
          );
        }
      }
    }
    if (admins.isEmpty) return;

    // The admins' stubs onto the backbone.
    final stubs = <int>[];
    for (final admin in admins) {
      final port = _port(admin.id, 1);
      stubs.add(port.x);
      _lay(net, [(port.x, port.y), (port.x, backboneY)]);
      wires.add(
        MapWire(
          layer: MapLayer.messages,
          points: [_o(port.x, port.y), _o(port.x, backboneY)],
          ends: {admin.id, ...reached},
        ),
      );
    }
    if (dropBoxes.isEmpty) return;

    // The backbone: along the top, and down the left to later rows.
    final rowOf = <_Box, int>{};
    for (final box in dropBoxes) {
      final row = rowTops.indexOf(box.y);
      rowOf[box] = row < 0 ? 0 : row;
    }
    final lastRow = rowOf.values.fold(0, math.max);
    final channelY = [
      for (var row = 0; row < rowTops.length; row++)
        row == 0 ? backboneY : rowTops[row] - 3,
    ];
    final left = _margin;
    for (var row = 0; row <= lastRow; row++) {
      final xs = [
        for (final box in dropBoxes)
          if (rowOf[box] == row) box.spine,
        if (row == 0) ...stubs,
        if (lastRow > 0) left,
      ]..sort();
      if (xs.length < 2) continue;
      _lay(net, [(xs.first, channelY[row]), (xs.last, channelY[row])]);
      wires.add(
        MapWire(
          layer: MapLayer.messages,
          points: [_o(xs.first, channelY[row]), _o(xs.last, channelY[row])],
          ends: {...adminIds, ...reached},
          bus: true,
        ),
      );
    }
    if (lastRow > 0) {
      _lay(net, [(left, channelY[0]), (left, channelY[lastRow])]);
      wires.add(
        MapWire(
          layer: MapLayer.messages,
          points: [_o(left, channelY[0]), _o(left, channelY[lastRow])],
          ends: {...adminIds, ...reached},
          bus: true,
        ),
      );
    }
    for (final box in dropBoxes) {
      final from = channelY[rowOf[box]!];
      final into = box.firstTap == box.rail ? box.rail : box.y;
      _lay(net, [(box.spine, from), (box.spine, into)]);
      wires.add(
        MapWire(
          layer: MapLayer.messages,
          points: [_o(box.spine, from), _o(box.spine, box.firstTap)],
          ends: {...adminIds, for (final agent in box.bus) agent.id},
          arrows: [
            MapArrow(_o(box.spine, (from + box.y) ~/ 2), const Offset(0, 1)),
          ],
        ),
      );
    }
  }

  /// Each agent with others nested under it: a net from its right side to
  /// theirs.
  void _hierarchy() {
    final parents = sorted(
      map.nodes.where(
        (node) =>
            node.kind == MapNodeKind.MAP_NODE_KIND_AGENT &&
            cells.containsKey(node.id) &&
            _children(node.id).any(cells.containsKey),
      ),
    );
    for (final parent in parents) {
      final children = [
        for (final child in _children(parent.id))
          if (cells.containsKey(child)) child,
      ]..sort(compare);
      _job(
        MapLayer.hierarchy,
        'h:${parent.id}',
        parent.id,
        _port(parent.id, 0),
        [for (final child in children) (child, _port(child, 0))],
      );
    }
  }

  /// Each lead: a net from its right side to those of the agents that
  /// report to it, a chevron on each one's branch pointing to the lead.
  void _leads() {
    final reports = <String, List<String>>{};
    for (final edge in leadEdges) {
      if (cells.containsKey(edge.fromId) && cells.containsKey(edge.toId)) {
        (reports[edge.toId] ??= []).add(edge.fromId);
      }
    }
    for (final lead in reports.keys.toList()..sort(compare)) {
      _job(MapLayer.leads, 'l:$lead', lead, _port(lead, 0), [
        for (final report in reports[lead]!) (report, _port(report, 0)),
      ], arrows: -1);
    }
  }

  /// Messages no bus says: each its own line, from left side to left side.
  void _loose() {
    for (final edge in loose) {
      if (!cells.containsKey(edge.fromId) || !cells.containsKey(edge.toId)) {
        continue;
      }
      // Each leaves from the side facing the other, or the left (as the
      // bus does) when they're one above the other.
      final from = cells[edge.fromId]!;
      final to = cells[edge.toId]!;
      final apart = (to.x - from.x).abs() > _chipW;
      final side = !apart
          ? 2
          : to.x > from.x
          ? 0
          : 2;
      _job(
        MapLayer.messages,
        'm:${edge.fromId}>${edge.toId}',
        edge.fromId,
        _port(edge.fromId, side),
        [(edge.toId, _port(edge.toId, apart ? 2 - side : 2))],
        arrows: edge.bothWays ? 2 : 1,
      );
    }
  }

  /// Each secret: a net from its chip in the vault to the agents that have
  /// it.
  void _secrets() {
    for (final secret in secrets) {
      final holders = [
        for (final edge in secretEdges)
          if (edge.fromId == secret.id && cells.containsKey(edge.toId))
            edge.toId,
      ];
      if (holders.isEmpty || !cells.containsKey(secret.id)) continue;
      _job(MapLayer.secrets, secret.id, secret.id, _port(secret.id, 3), [
        for (final holder in holders) (holder, _port(holder, 0)),
      ]);
    }
  }

  void _records() {
    final records = this.records;
    if (records == null || !cells.containsKey(records.id)) return;
    final readers = [
      for (final edge in recordReads)
        if (cells.containsKey(edge.fromId)) edge.fromId,
    ];
    if (readers.isEmpty) return;
    _job(MapLayer.data, records.id, records.id, _port(records.id, 3), [
      for (final reader in readers) (reader, _port(reader, 0)),
    ]);
  }

  /// Each app: a net from its chip to the agents it's given to, and a
  /// line with an arrow into each agent that hears its events.
  void _apps() {
    for (final app in apps) {
      if (!cells.containsKey(app.id)) continue;
      final given = [
        for (final edge in appEdges)
          if (edge.fromId == app.id && cells.containsKey(edge.toId)) edge.toId,
      ];
      if (given.isNotEmpty) {
        _job(MapLayer.apps, app.id, app.id, _port(app.id, 3), [
          for (final agent in given) (agent, _port(agent, 0)),
        ]);
      }
      for (final edge in hearEdges) {
        if (edge.fromId != app.id || !cells.containsKey(edge.toId)) continue;
        _job(
          MapLayer.apps,
          'hears:${app.id}>${edge.toId}',
          app.id,
          _port(app.id, 3),
          [(edge.toId, _port(edge.toId, 0))],
          arrows: 1,
        );
      }
    }
  }

  final jobs = <_Job>[];

  /// A net to route from [root] to each of [terminals]. [arrows] puts a
  /// chevron (1) or two opposite ones (2) on a two-ended net, pointing from
  /// root to terminal, or (-1) one on each terminal's branch, pointing back
  /// toward the root.
  void _job(
    MapLayer layer,
    String name,
    String rootId,
    _Port root,
    List<(String, _Port)> terminals, {
    int arrows = 0,
  }) =>
      jobs.add(_Job(layer, _net(name), name, rootId, root, terminals, arrows));

  /// Routes [job]'s terminals, nearest first: each joins what's already
  /// laid, at a T.
  void _wire(_Job job) {
    final _Job(:layer, :net, :root, :terminals, :arrows) = job;
    final ends = {job.rootId, for (final (id, _) in terminals) id};
    _mark(net, root.x, root.y, root.dir);
    wires.add(
      MapWire(
        layer: layer,
        points: [_o(root.x, root.y), _o(root.sx, root.sy)],
        ends: ends,
      ),
    );
    final ordered = [...terminals]
      ..sort((a, b) {
        final da = (a.$2.sx - root.sx).abs() + (a.$2.sy - root.sy).abs();
        final db = (b.$2.sx - root.sx).abs() + (b.$2.sy - root.sy).abs();
        return da != db ? da.compareTo(db) : compare(a.$1, b.$1);
      });
    for (final (_, port) in ordered) {
      final path = _search(net, port);
      // No way through (a crowded board): that branch goes undrawn rather
      // than through a chip.
      if (path == null) continue;
      _mark(net, port.x, port.y, port.dir);
      for (var i = 0; i + 1 < path.length; i++) {
        final (ax, ay) = path[i];
        final (bx, by) = path[i + 1];
        _mark(
          net,
          ax,
          ay,
          bx > ax
              ? 0
              : by > ay
              ? 1
              : bx < ax
              ? 2
              : 3,
        );
      }
      final points = _corners([(port.x, port.y), ...path]);
      final offsets = [for (final (x, y) in points) _o(x, y)];
      wires.add(
        MapWire(
          layer: layer,
          points: offsets,
          ends: ends,
          arrows: arrows == 0 ? const [] : _chevrons(offsets, arrows),
        ),
      );
    }
  }

  /// The path's corners only.
  static List<(int, int)> _corners(List<(int, int)> path) {
    final out = <(int, int)>[path.first];
    for (var i = 1; i + 1 < path.length; i++) {
      final (ax, ay) = out.last;
      final (bx, by) = path[i];
      final (cx, cy) = path[i + 1];
      final straight = (ax == bx && bx == cx) || (ay == by && by == cy);
      if (!straight) out.add(path[i]);
    }
    if (path.length > 1) out.add(path.last);
    return out;
  }

  /// Chevrons on the longest straight run of [points] (drawn from the
  /// terminal back to the root): one pointing at the terminal, two
  /// opposite ones for a pair that message each other, or (-1) one
  /// pointing away from it, toward the root.
  static List<MapArrow> _chevrons(List<Offset> points, int count) {
    var best = 0;
    var length = -1.0;
    for (var i = 0; i + 1 < points.length; i++) {
      final run = (points[i + 1] - points[i]).distance;
      if (run > length) {
        length = run;
        best = i;
      }
    }
    final a = points[best];
    final b = points[best + 1];
    final step = (b - a) / math.max(length, 1);
    // The route runs from the terminal: toward it is backwards.
    final toward = -step;
    if (count == -1) return [MapArrow((a + b) / 2, step)];
    if (count == 1) return [MapArrow((a + b) / 2, toward)];
    return [
      MapArrow(a + (b - a) / 3, toward),
      MapArrow(a + (b - a) * 2 / 3, step),
    ];
  }

  var _cost = Int32List(0);
  var _prev = Int32List(0);
  var _seen = Int32List(0);
  var _search_ = 0;

  /// The cheapest way from [from]'s first step to anything of [net]
  /// already laid: length, plus bends, crossings and running along
  /// outlines, which all cost extra. Null when there's none.
  List<(int, int)>? _search(int net, _Port from) {
    const bend = 6;
    const cross = 10;
    const outline = 3;
    final states = gw * gh * 4;
    if (_cost.length != states) {
      _cost = Int32List(states);
      _prev = Int32List(states);
      _seen = Int32List(states);
    }
    _search_++;
    final cost = _cost;
    final prev = _prev;
    final seen = _seen;
    final round = _search_;
    int costOf(int s) => seen[s] == round ? cost[s] : 1 << 30;
    final heap = _Heap();
    if (!_in(from.sx, from.sy)) return null;
    final start = _p(from.sx, from.sy) * 4 + from.dir;
    cost[start] = 0;
    prev[start] = -1;
    seen[start] = round;
    final box = bounds[net];
    // The search heads for the net's box: A*, with the distance to it as
    // the estimate, which never overestimates.
    int estimate(int p) {
      if (box == null) return 0;
      final x = p % gw;
      final y = p ~/ gw;
      final dx = x < box[0] ? box[0] - x : (x > box[2] ? x - box[2] : 0);
      final dy = y < box[1] ? box[1] - y : (y > box[3] ? y - box[3] : 0);
      return dx + dy;
    }

    heap.push(estimate(start ~/ 4), start);
    while (heap.isNotEmpty) {
      final (f, s) = heap.pop();
      final c = costOf(s);
      if (f != c + estimate(s ~/ 4)) continue;
      final p = s ~/ 4;
      final d = s % 4;
      final x = p % gw;
      final y = p ~/ gw;
      final free = netAt[p] == -1 && crossAt[p] == -1;
      for (final nd in [d, (d + 1) % 4, (d + 3) % 4]) {
        final turning = nd != d;
        // Lines only turn where nothing else is.
        if (turning && !free) continue;
        final nx = x + _dx[nd];
        final ny = y + _dy[nd];
        if (!_in(nx, ny)) continue;
        final q = _p(nx, ny);
        if (blocked[q] == 1) continue;
        if (reserved[q] != -1 && reserved[q] != net) continue;
        var step = 1 + (turning ? bend : 0) + (blocked[q] == 2 ? outline : 0);
        if (netAt[q] == net) {
          // Joining its own net: never at a crossing, never a fourth
          // branch.
          final bit = 1 << _opposite(nd);
          if (crossAt[q] != -1 || maskAt[q] & bit != 0) continue;
          if (_bits(maskAt[q] | bit) > 3) continue;
          return _path(prev, s, (nx, ny));
        }
        if (netAt[q] != -1) {
          // Across another line: straight over a straight stretch of it.
          final mask = maskAt[q];
          final across = nd % 2 == 0 ? 0xA : 0x5; // up+down, or left+right
          if (mask != across || crossAt[q] != -1) continue;
          step += cross;
        }
        final ns = q * 4 + nd;
        final nc = c + step;
        if (nc < costOf(ns)) {
          cost[ns] = nc;
          seen[ns] = round;
          prev[ns] = s;
          heap.push(nc + estimate(q), ns);
        }
      }
    }
    return null;
  }

  List<(int, int)> _path(Int32List prev, int last, (int, int) goal) {
    final out = <(int, int)>[goal];
    var s = last;
    while (s != -1) {
      final p = s ~/ 4;
      out.add((p % gw, p ~/ gw));
      s = prev[s];
    }
    return out.reversed.toList();
  }
}

/// A binary min-heap of (cost, state), ties broken by state, so the search
/// is the same every time.
class _Heap {
  final _cost = <int>[];
  final _state = <int>[];

  bool get isNotEmpty => _cost.isNotEmpty;

  bool _less(int i, int j) =>
      _cost[i] < _cost[j] || (_cost[i] == _cost[j] && _state[i] < _state[j]);

  void _swap(int i, int j) {
    final c = _cost[i];
    _cost[i] = _cost[j];
    _cost[j] = c;
    final s = _state[i];
    _state[i] = _state[j];
    _state[j] = s;
  }

  void push(int cost, int state) {
    _cost.add(cost);
    _state.add(state);
    var i = _cost.length - 1;
    while (i > 0) {
      final parent = (i - 1) ~/ 2;
      if (!_less(i, parent)) break;
      _swap(i, parent);
      i = parent;
    }
  }

  (int, int) pop() {
    final top = (_cost.first, _state.first);
    final last = _cost.length - 1;
    _swap(0, last);
    _cost.removeLast();
    _state.removeLast();
    var i = 0;
    while (true) {
      final l = 2 * i + 1;
      final r = l + 1;
      var m = i;
      if (l < _cost.length && _less(l, m)) m = l;
      if (r < _cost.length && _less(r, m)) m = r;
      if (m == i) break;
      _swap(i, m);
      i = m;
    }
    return top;
  }
}
