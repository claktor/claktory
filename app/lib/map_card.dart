import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'map_layout.dart' show sidebarOrder;
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';

/// One of an agent's extensions on its card: a badge's (what gives it
/// reach or power, listed first) or one of the rest.
typedef MapCardExtension = ({String label, String icon, bool important});

/// Everything the Map and the workspace say about an agent, for its card:
/// every extension it has on (the badges' first, in their order), the
/// folder it works in and the folders it only reads, whom it can message
/// and the secrets it has.
class MapAgentDetails {
  const MapAgentDetails({
    required this.agent,
    required this.extensions,
    required this.folder,
    required this.reads,
    required this.everyone,
    required this.messages,
    required this.secrets,
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
  final reads = <MapNode>[];
  final messages = <MapNode>[];
  final secrets = <MapNode>[];
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
  return MapAgentDetails(
    agent: node,
    extensions: extensions,
    folder: folder,
    reads: reads,
    everyone: node.admin,
    messages: messages,
    secrets: secrets,
  );
}

/// An agent's card on the Map: a floating layer like a hover card, with
/// its name and role, then its extensions (the badges' in `text`, the
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
    final caption = TextStyles.caption.copyWith(color: palette.textMuted);
    final faint = TextStyles.caption.copyWith(color: palette.textFaint);
    Widget names(List<MapNode> nodes, String none) => Text(
      nodes.isEmpty ? none : nodes.map((node) => node.label).join(', '),
      style: nodes.isEmpty ? faint : caption,
    );
    final folder = details.folder;
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
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
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
                  style: TextStyles.label.copyWith(
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
                            Text(
                              extension.label,
                              style: TextStyles.caption.copyWith(
                                color: extension.important
                                    ? palette.text
                                    : palette.textFaint,
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
          _Section(title: 'Secrets', child: names(details.secrets, 'None')),
          if (actions != null && actions.isNotEmpty) ...[
            const SizedBox(height: Space.m),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                for (final (index, action) in actions.indexed) ...[
                  if (index > 0) const SizedBox(width: Space.s),
                  action,
                ],
              ],
            ),
          ],
        ],
      ),
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
            style: TextStyles.caption.copyWith(
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
