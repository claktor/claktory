import 'package:flutter/widgets.dart';

import 'apps.dart' show labelInSentence;
import 'controls.dart';
import 'map_layout.dart' show sidebarOrder;
import 'identity.dart';
import 'mobile/drawer.dart' show largeText;
import 'mobile/metrics.dart' show TypeScale;
import 'protocol/factory/v1/factory.pb.dart';

/// One of an agent's extensions on its card: a badge's (what gives it
/// reach or power, listed first) or one of the rest.
typedef MapCardExtension = ({String label, String icon, bool important});

/// Everything the Map and the workspace say about an agent, for its card:
/// every extension it has on (the badges' first, in their order), the
/// folder it works in and the folders it only reads, its lead and the
/// agents it leads, whom it can message and the secrets it has.
class MapAgentDetails {
  const MapAgentDetails({
    required this.agent,
    required this.extensions,
    required this.folder,
    required this.reads,
    required this.everyone,
    required this.messages,
    required this.secrets,
    this.apps = const [],
    this.lead,
    this.reports = const [],
  });

  final MapNode agent;
  final List<MapCardExtension> extensions;

  /// Where it works: the workspace's, a project's or a folder's node.
  final MapNode? folder;

  /// Folders of its project it can read but not change.
  final List<MapNode> reads;

  /// An admin agent: it messages every agent.
  final bool everyone;
  final List<MapNode> messages;
  final List<MapNode> secrets;

  /// The agent it reports to (Agent.lead_id, the Map's lead line), and
  /// those that report to it.
  final MapNode? lead;
  final List<MapNode> reports;

  /// The apps given to it: each app's node, what it may do with it and
  /// the events it hears (the edges' labels; empty when none).
  final List<({MapNode app, String can, String hears})> apps;
}

/// The details of the agent [node] in [workspace]'s map.
MapAgentDetails mapAgentDetails(Workspace workspace, MapNode node) {
  final map = workspace.map;
  final nodes = {for (final node in map.nodes) node.id: node};
  final catalog = {
    for (final extension in workspace.extensions) extension.id: extension,
  };
  // A badge stands for its extension: "web" for Web, "secrets:give" for
  // Secrets, "memory:factory" for the Claktory manual.
  final covered = {
    for (final badge in node.badges)
      catalog.containsKey(badge.key) ? badge.key : badge.key.split(':').first,
  };
  final agent = workspace.agents.where((agent) => agent.id == node.refId);
  final extensions = <MapCardExtension>[
    for (final badge in node.badges)
      (label: badge.label, icon: badge.icon, important: true),
    if (agent.isNotEmpty)
      for (final extension in agent.first.extensions)
        if (extension.enabled && !covered.contains(extension.id))
          if (catalog[extension.id] case final known?)
            (label: known.name, icon: known.icon, important: false),
  ];
  MapNode? folder;
  MapNode? lead;
  final reports = <MapNode>[];
  final reads = <MapNode>[];
  final messages = <MapNode>[];
  final secrets = <MapNode>[];
  final can = <String, String>{};
  final hears = <String, String>{};
  for (final edge in map.edges) {
    final from = edge.fromId == node.id;
    final to = edge.toId == node.id;
    if (!from && !to) continue;
    final other = nodes[from ? edge.toId : edge.fromId];
    if (other == null) continue;
    final place = switch (other.kind) {
      MapNodeKind.MAP_NODE_KIND_WORKSPACE ||
      MapNodeKind.MAP_NODE_KIND_PROJECT ||
      MapNodeKind.MAP_NODE_KIND_FOLDER => true,
      _ => false,
    };
    switch (edge.kind) {
      case MapEdgeKind.MAP_EDGE_KIND_WRITES when from && place:
        folder = other;
      case MapEdgeKind.MAP_EDGE_KIND_READS when from && place:
        reads.add(other);
      case MapEdgeKind.MAP_EDGE_KIND_MESSAGES when from || edge.bothWays:
        if (!messages.contains(other)) messages.add(other);
      case MapEdgeKind.MAP_EDGE_KIND_SECRET when to:
        secrets.add(other);
      case MapEdgeKind.MAP_EDGE_KIND_LEAD when from:
        lead = other;
      case MapEdgeKind.MAP_EDGE_KIND_LEAD when to:
        reports.add(other);
      case MapEdgeKind.MAP_EDGE_KIND_APP when to:
        can[other.id] = edge.label;
      case MapEdgeKind.MAP_EDGE_KIND_HEARS when to:
        hears[other.id] = edge.label;
      default:
    }
  }
  // As the sidebar lists them: projects before their folders, agents in
  // their order.
  final order = sidebarOrder(workspace);
  int bySidebar(MapNode a, MapNode b) =>
      (order[a.id] ?? 1 << 30).compareTo(order[b.id] ?? 1 << 30);
  reads.sort(bySidebar);
  messages.sort(bySidebar);
  reports.sort(bySidebar);
  return MapAgentDetails(
    agent: node,
    extensions: extensions,
    folder: folder,
    reads: reads,
    everyone: node.admin,
    messages: messages,
    secrets: secrets,
    lead: lead,
    reports: reports,
    apps: [
      for (final id in {...can.keys, ...hears.keys})
        if (nodes[id] case final app?)
          (app: app, can: can[id] ?? '', hears: hears[id] ?? ''),
    ],
  );
}

/// An agent's card on the Map: a floating layer like a hover card, with
/// its name and role, then whom it reports to and whom it leads, its
/// extensions (the badges' in `text`, the
/// rest faint), where it works, whom it can message and its secrets.
/// [actions] go along the bottom (on a phone, where it's tapped open).
class MapAgentCard extends StatelessWidget {
  const MapAgentCard({
    super.key,
    required this.details,
    this.actions,
    this.width = 300,
  });

  final MapAgentDetails details;
  final List<Widget>? actions;

  /// Null to fill the width it's given.
  final double? width;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final agent = details.agent;
    final caption = TypeScale.of(context).caption
        .copyWith(color: palette.textMuted);
    final faint = TypeScale.of(context).caption
        .copyWith(color: palette.textFaint);
    Widget names(List<MapNode> nodes, String none) => Text(
      nodes.isEmpty ? none : nodes.map((node) => node.label).join(', '),
      style: nodes.isEmpty ? faint : caption,
    );
    final folder = details.folder;
    final lead = details.lead;
    final actions = this.actions;
    return Container(
      width: width,
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
        boxShadow: floatingShadow,
      ),
      child: _CardBody(
        scrolls: width == null,
        actions: actions == null || actions.isEmpty
            ? null
            : Padding(
                padding: const EdgeInsets.only(top: Space.m),
                // Side by side at the end, under each other when they
                // don't fit (a phone's finger-sized buttons).
                child: Wrap(
                  alignment: WrapAlignment.end,
                  spacing: Space.s,
                  runSpacing: Space.s,
                  children: actions,
                ),
              ),
        children: [
          Row(
            children: [
              Dot(color: agentColor(agent.color), size: 8),
              const SizedBox(width: Space.s),
              Expanded(
                child: Text(
                  agent.label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TypeScale.of(context).label.copyWith(
                    color: palette.text,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          if (agent.detail.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(left: 8 + Space.s),
              child: Text(agent.detail, style: caption),
            ),
          if (lead != null)
            _Section(title: 'Reports to', child: names([lead], '')),
          if (details.reports.isNotEmpty)
            _Section(title: 'Leads', child: names(details.reports, '')),
          _Section(
            title: 'Extensions',
            child: details.extensions.isEmpty
                ? Text('None', style: faint)
                : Wrap(
                    spacing: Space.m,
                    runSpacing: Space.xs,
                    children: [
                      for (final extension in details.extensions)
                        Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            ExtensionGlyph(
                              icon: extension.icon,
                              name: extension.label,
                              color: extension.important
                                  ? palette.textMuted
                                  : palette.textFaint,
                              size: 12,
                            ),
                            const SizedBox(width: Space.xs),
                            Flexible(
                              child: Text(
                                extension.label,
                                style: TypeScale.of(context).caption.copyWith(
                                  color: extension.important
                                      ? palette.text
                                      : palette.textFaint,
                                ),
                              ),
                            ),
                          ],
                        ),
                    ],
                  ),
          ),
          if (folder != null)
            _Section(
              title: 'Works in',
              child: Text.rich(
                TextSpan(
                  children: [
                    TextSpan(text: folder.label, style: caption),
                    if (folder.detail.isNotEmpty)
                      TextSpan(text: '  ${folder.detail}', style: faint),
                  ],
                ),
              ),
            ),
          if (details.reads.isNotEmpty)
            _Section(title: 'Also reads', child: names(details.reads, '')),
          _Section(
            title: 'Can message',
            child: details.everyone
                ? Text('Every agent', style: caption)
                : names(details.messages, 'No one'),
          ),
          // Each with what it grants, so none reads as more than it is.
          _Section(
            title: 'Secrets',
            child: details.secrets.isEmpty
                ? names(details.secrets, 'None')
                : Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      for (final secret in details.secrets)
                        Text.rich(
                          TextSpan(
                            children: [
                              TextSpan(text: secret.label),
                              if (secret.detail.isNotEmpty)
                                TextSpan(
                                  text: '  ${secret.detail}',
                                  style: faint,
                                ),
                            ],
                          ),
                          style: caption,
                        ),
                    ],
                  ),
          ),
          if (details.apps.isNotEmpty)
            _Section(
              title: 'Apps',
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  for (final (:app, :can, :hears) in details.apps)
                    Text.rich(
                      TextSpan(
                        children: [
                          TextSpan(text: app.label),
                          if (can.isNotEmpty)
                            TextSpan(text: '  $can', style: faint),
                          if (hears.isNotEmpty)
                            TextSpan(
                              text: '  Hears: ${labelInSentence(hears)}',
                              style: faint,
                            ),
                        ],
                      ),
                      style: caption,
                    ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// The card's details, then its [actions]. Where it [scrolls] (a phone's,
/// as wide as the screen and at most as tall as it's let be), the details
/// scroll and the actions stay in reach under them.
class _CardBody extends StatelessWidget {
  const _CardBody({
    required this.scrolls,
    required this.children,
    this.actions,
  });

  final bool scrolls;
  final List<Widget> children;
  final Widget? actions;

  @override
  Widget build(BuildContext context) {
    final details = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: children,
    );
    final actions = this.actions;
    if (!scrolls) {
      return Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [...children, ?actions],
      );
    }
    // At the largest text sizes the actions alone may fill it: they scroll
    // with the rest.
    if (largeText(context)) {
      return SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [...children, ?actions],
        ),
      );
    }
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Flexible(child: SingleChildScrollView(child: details)),
        ?actions,
      ],
    );
  }
}

/// A heading in caption, faint and weight 500, over what it lists.
class _Section extends StatelessWidget {
  const _Section({required this.title, required this.child});

  final String title;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: Space.m),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            title,
            style: TypeScale.of(context).caption.copyWith(
              color: palette.textFaint,
              fontWeight: FontWeight.w500,
            ),
          ),
          const SizedBox(height: 2),
          child,
        ],
      ),
    );
  }
}

/// Puts a card beside [target] (in the view's coordinates): to its right,
/// else its left, top-aligned with it, kept inside the view.
class MapCardPlacement extends SingleChildLayoutDelegate {
  const MapCardPlacement(this.target);

  final Rect target;

  @override
  BoxConstraints getConstraintsForChild(BoxConstraints constraints) =>
      constraints.loosen();

  @override
  Offset getPositionForChild(Size size, Size childSize) {
    const gap = Space.s;
    double x;
    if (target.right + gap + childSize.width <= size.width - gap) {
      x = target.right + gap;
    } else if (target.left - gap - childSize.width >= gap) {
      x = target.left - gap - childSize.width;
    } else {
      x = size.width - gap - childSize.width;
    }
    final maxY = size.height - gap - childSize.height;
    final y = maxY < gap ? gap : target.top.clamp(gap, maxY);
    return Offset(x < gap ? gap : x, y);
  }

  @override
  bool shouldRelayout(MapCardPlacement old) => old.target != target;
}
