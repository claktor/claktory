// A small history to play in tests (#458): a project with three agents, a
// secret, an agent that moved to another folder, a pipe, and a page of
// events.
import 'package:fixnum/fixnum.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';

const hour = 3600000;

/// Day one at 00:00 UTC, 2026-11-18.
const day1 = 1795046400000;

HistoryInterval span(int from, [int to = 0, bool inferred = false]) =>
    HistoryInterval(fromMs: Int64(from), toMs: Int64(to), inferred: inferred);

HistoryNode node(
  String id,
  MapNodeKind kind,
  String label, {
  List<HistoryInterval> present = const [],
  String parent = '',
  String project = '',
  bool admin = false,
  String color = '',
  String detail = '',
  List<HistoryLabel> labels = const [],
}) => HistoryNode(
  node: MapNode(
    id: id,
    kind: kind,
    label: label,
    detail: detail,
    parentId: parent,
    projectId: project,
    admin: admin,
    color: color,
    refId: id.contains(':') ? id.substring(id.indexOf(':') + 1) : id,
  ),
  present: present,
  labels: labels,
);

HistoryEdge edge(
  String from,
  String to,
  MapEdgeKind kind,
  List<HistoryInterval> present, {
  bool both = false,
}) => HistoryEdge(
  edge: MapEdge(fromId: from, toId: to, kind: kind, bothWays: both),
  present: present,
);

/// Rivet from day one (reconstructed until hour 30), Kai and Ana in the
/// project from hours 2 and 5, Kai moving to the "web" folder at hour 40,
/// a secret given to Ana at hour 10 and a pipe at hour 20; recording from
/// hour 30.
HistoryOverview fixtureOverview() {
  const agent = MapNodeKind.MAP_NODE_KIND_AGENT;
  const messages = MapEdgeKind.MAP_EDGE_KIND_MESSAGES;
  const writes = MapEdgeKind.MAP_EDGE_KIND_WRITES;
  return HistoryOverview(
    startMs: Int64(day1),
    endMs: Int64(day1 + 48 * hour),
    recordingSinceMs: Int64(day1 + 30 * hour),
    nodes: [
      node(
        'workspace',
        MapNodeKind.MAP_NODE_KIND_WORKSPACE,
        'Workspace',
        present: [span(day1, 0, true)],
      ),
      node(
        'agent:rivet',
        agent,
        'Rivet',
        admin: true,
        color: 'blue',
        detail: 'Admin',
        present: [span(day1, 0, true)],
      ),
      node(
        'project:app',
        MapNodeKind.MAP_NODE_KIND_PROJECT,
        'App',
        present: [span(day1 + 2 * hour, 0, true)],
      ),
      node(
        'folder:app/web',
        MapNodeKind.MAP_NODE_KIND_FOLDER,
        'Web',
        parent: 'project:app',
        project: 'app',
        present: [span(day1 + 2 * hour, 0, true)],
      ),
      node(
        'agent:kai',
        agent,
        'Kai',
        color: 'green',
        parent: 'project:app',
        project: 'app',
        present: [span(day1 + 2 * hour, 0, true)],
        labels: [
          HistoryLabel(atMs: Int64(day1 + 2 * hour), label: 'Kai'),
          HistoryLabel(atMs: Int64(day1 + 35 * hour), label: 'Kaito'),
        ],
      ),
      node(
        'agent:ana',
        agent,
        'Ana',
        color: 'pink',
        parent: 'project:app',
        project: 'app',
        present: [span(day1 + 5 * hour, 0, true)],
      ),
      node(
        'secret:gh',
        MapNodeKind.MAP_NODE_KIND_SECRET,
        'GitHub',
        present: [span(day1 + 10 * hour)],
      ),
      node(
        'pipe:p1',
        MapNodeKind.MAP_NODE_KIND_PIPE,
        'Pipe',
        parent: 'project:app',
        project: 'app',
        detail: 'Another Claktory',
        present: [span(day1 + 20 * hour)],
      ),
    ],
    edges: [
      edge('agent:rivet', 'workspace', writes, [span(day1, 0, true)]),
      edge('agent:kai', 'project:app', writes, [
        span(day1 + 2 * hour, day1 + 40 * hour, true),
      ]),
      edge('agent:kai', 'folder:app/web', writes, [span(day1 + 40 * hour)]),
      edge('agent:ana', 'project:app', writes, [
        span(day1 + 5 * hour, 0, true),
      ]),
      edge('agent:ana', 'agent:kai', messages, [
        span(day1 + 5 * hour, 0, true),
      ], both: true),
      edge('agent:rivet', 'agent:kai', messages, [
        span(day1 + 2 * hour, 0, true),
      ]),
      edge('agent:rivet', 'agent:ana', messages, [
        span(day1 + 5 * hour, 0, true),
      ]),
      edge('secret:gh', 'agent:ana', MapEdgeKind.MAP_EDGE_KIND_SECRET, [
        span(day1 + 10 * hour),
      ]),
      edge('agent:ana', 'pipe:p1', MapEdgeKind.MAP_EDGE_KIND_PIPE, [
        span(day1 + 20 * hour),
      ]),
    ],
    histogram: [
      for (var h = 0; h < 48; h++) h == 3 || h == 6 || h == 41 ? 2 : 0,
    ],
    chapters: [
      HistoryChapter(
        atMs: Int64(day1 + 2 * hour),
        kind: HistoryChapterKind.HISTORY_CHAPTER_KIND_PROJECT,
        title: 'App started',
        nodeIds: ['project:app'],
      ),
      HistoryChapter(
        atMs: Int64(day1 + 30 * hour),
        kind: HistoryChapterKind.HISTORY_CHAPTER_KIND_RECORDING,
        title: 'Recording began',
      ),
    ],
    messageCount: Int64(6),
    historyVersion: Int64(1),
  );
}

/// Indexes into the fixture's nodes, as events name them.
const you = 0;
const rivet = 2;
const kai = 5;
const ana = 6;
const pipe = 8;

/// Six events: Rivet to Kai at 3:00 and 3:10, the person to Kai at 6:00,
/// Kai to Ana and Rivet (a list send) at 6:05, Ana's card at 41:00 and a
/// pipe message in to Ana at 41:30; and Kai's run from 3:00 to 3:20.
HistoryEvents fixtureEvents() {
  const minute = 60000;
  final times = [
    day1 + 3 * hour,
    day1 + 3 * hour + 10 * minute,
    day1 + 6 * hour,
    day1 + 6 * hour + 5 * minute,
    day1 + 41 * hour,
    day1 + 41 * hour + 30 * minute,
  ];
  final dt = <Int64>[];
  var last = day1;
  for (final t in times) {
    dt.add(Int64(t - last));
    last = t;
  }
  return HistoryEvents(
    baseMs: Int64(day1),
    dt: dt,
    from: [rivet, rivet, you, kai, ana, pipe],
    kind: [
      HistoryEventKind.HISTORY_EVENT_KIND_AGENT,
      HistoryEventKind.HISTORY_EVENT_KIND_AGENT,
      HistoryEventKind.HISTORY_EVENT_KIND_PERSON,
      HistoryEventKind.HISTORY_EVENT_KIND_AGENT,
      HistoryEventKind.HISTORY_EVENT_KIND_CARD,
      HistoryEventKind.HISTORY_EVENT_KIND_PIPE_IN,
    ],
    flags: [0, 1, 0, 0, 0, 0],
    toCount: [1, 1, 1, 2, 1, 1],
    to: [kai, kai, kai, ana, rivet, you, ana],
    runNode: [kai],
    runStart: [Int64(3 * hour)],
    runMs: [Int64(20 * minute)],
  );
}

/// [fixtureEvents] in three pages of two, as the backend pages them:
/// cursor [k] reads page k; Kai's run is in the first.
List<HistoryEvents> fixturePages() {
  final all = fixtureEvents();
  final pages = <HistoryEvents>[];
  var at = all.baseMs;
  var to = 0;
  for (var p = 0; p < 3; p++) {
    final base = at;
    final page = HistoryEvents(baseMs: base);
    for (var i = 2 * p; i < 2 * p + 2; i++) {
      at += all.dt[i];
      page.dt.add(i == 2 * p ? at - base : all.dt[i]);
      page.from.add(all.from[i]);
      page.kind.add(all.kind[i]);
      page.flags.add(all.flags[i]);
      page.toCount.add(all.toCount[i]);
      for (var k = 0; k < all.toCount[i]; k++) {
        page.to.add(all.to[to++]);
      }
    }
    if (p == 0) {
      page
        ..runNode.addAll(all.runNode)
        ..runStart.addAll(all.runStart)
        ..runMs.addAll(all.runMs);
    }
    page.nextCursor = p < 2 ? [p + 1] : [];
    pages.add(page);
  }
  return pages;
}

/// Where the second and third pages start, as the overview says.
List<HistoryCursor> fixtureCursors() => [
  HistoryCursor(atMs: Int64(day1 + 6 * hour), cursor: [1]),
  HistoryCursor(atMs: Int64(day1 + 41 * hour), cursor: [2]),
];
