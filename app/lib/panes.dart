import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'highlights.dart';
import 'identity.dart';
import 'changelog.dart';
import 'layout.dart';
import 'marketplace.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'pane_chrome.dart';
import 'settings_view.dart';
import 'viewers/registry.dart';

/// A pane is at least this wide and this tall (docs/IDENTITY.md, "Panes
/// and viewers").
const paneMinWidth = 200.0;
const paneMinHeight = 120.0;

/// A page's pane (Settings…) is at least as wide as a small phone's screen,
/// which its rows are laid out for.
const pagePaneMinWidth = 320.0;

/// A pane's header.
const paneHeaderHeight = 32.0;

/// Settings, the Marketplace and What's new open as panes in the selected
/// agent's layout on a Mac, right of its conversation, so it can be read
/// meanwhile (a phone shows them as pages). Their leaves have no path.
const pageKinds = {
  AppPage.settings: 'settings',
  AppPage.marketplace: 'marketplace',
  AppPage.changelog: 'changelog',
};

/// The page a leaf of [kind] shows, or null for a file's viewer or the
/// conversation.
AppPage? pageOfKind(String kind) {
  for (final MapEntry(key: page, value: pageKind) in pageKinds.entries) {
    if (pageKind == kind) return page;
  }
  return null;
}

/// The pane showing [page] in [root], or null.
ViewNode? pagePane(ViewNode root, AppPage page) {
  for (final leaf in leaves(root)) {
    if (leaf.leaf.kind == pageKinds[page]) return leaf;
  }
  return null;
}

/// [root] with a pane for [page] right of the conversation. In a row, it
/// takes half the conversation's share and the panes beside keep theirs;
/// else the two share the conversation's place. At [maxPanes] the oldest
/// pane other than the conversation gives way.
ViewNode withPage(ViewNode root, AppPage page) {
  final pane = ViewNode(
    id: newPaneId(pageKinds[page]!),
    leaf: ViewLeaf(kind: pageKinds[page]!),
  );
  var tree = root;
  if (leaves(tree).length >= maxPanes) {
    final oldest = leaves(tree)
        .firstWhere((leaf) => leaf.leaf.kind != chatKind);
    tree = close(tree, oldest.id) ?? tree;
  }
  final chat = leaves(tree).firstWhere((leaf) => leaf.leaf.kind == chatKind);
  ViewNode place(ViewNode node) {
    if (!node.hasSplit()) return node;
    final split = node.split;
    final at = split.children.indexWhere((child) => child.id == chat.id);
    if (at < 0 || split.axis == ViewAxis.VIEW_AXIS_COLUMN) {
      return ViewNode(
        id: node.id,
        split: ViewSplit(
          axis: split.axis,
          children: [for (final child in split.children) place(child)],
          weights: split.weights,
        ),
      );
    }
    final shares = sharesOf(split);
    return ViewNode(
      id: node.id,
      split: ViewSplit(
        axis: split.axis,
        children: [...split.children]..insert(at + 1, pane),
        weights: [...shares]
          ..[at] = shares[at] / 2
          ..insert(at + 1, shares[at] / 2),
      ),
    );
  }

  final placed = place(tree);
  if (find(placed, pane.id) != null) return placed;
  return insert(tree, pane, chat.id, DropEdge.right) ?? tree;
}

/// What a pane's header carries while it's dragged.
class PaneDrag {
  const PaneDrag(this.agentId, this.paneId);

  final String agentId;
  final String paneId;
}

/// [agent]'s view below its header: its panes as the backend lays them out
/// (`Agent.layout`), the conversation ([chat]) one of them. With only the
/// conversation there's no pane chrome at all. Each change (a drag, a
/// close, a resize once it ends) sends the whole tree (SetLayout), and
/// shows it meanwhile.
class PaneLayout extends StatefulWidget {
  const PaneLayout({super.key, required this.agent, required this.chat});

  final Agent agent;
  final Widget chat;

  @override
  State<PaneLayout> createState() => _PaneLayoutState();
}

class _PaneLayoutState extends State<PaneLayout> {
  /// The tree after a change, until the backend has it.
  ViewNode? _sent;

  /// The tree while a split's divider is dragged.
  ViewNode? _dragged;

  /// Each pane keeps its state (scroll, zoom, a draft) as it moves.
  final _chatKey = GlobalKey();
  final _keys = <String, GlobalKey>{};

  /// A divider's drag: the split's shares when it began, and its room.
  List<double> _from = const [];

  ViewNode get _root => _dragged ?? _sent ?? layoutOf(widget.agent);

  @override
  void didUpdateWidget(PaneLayout old) {
    super.didUpdateWidget(old);
    if (old.agent.id != widget.agent.id) {
      _sent = null;
      _dragged = null;
    }
  }

  void _change(ViewNode? root) {
    _dragged = null;
    if (root == null) {
      setState(() {});
      return;
    }
    setState(() => _sent = root);
    fireAndLog(
      AppScope.of(context)
          .setLayout(widget.agent.id, isChatOnly(root) ? null : root)
          .whenComplete(() {
            if (mounted && _sent == root) setState(() => _sent = null);
          }),
    );
  }

  @override
  Widget build(BuildContext context) {
    final root = _root;
    final chat = KeyedSubtree(key: _chatKey, child: widget.chat);
    if (isChatOnly(root)) {
      _keys.clear();
      return chat;
    }
    final ids = {for (final leaf in leaves(root)) leaf.id};
    _keys.removeWhere((id, _) => !ids.contains(id));
    return _node(root, root, chat);
  }

  Widget _node(ViewNode root, ViewNode node, Widget chat) {
    if (!node.hasSplit()) return _pane(root, node, chat);
    final split = node.split;
    final row = split.axis != ViewAxis.VIEW_AXIS_COLUMN;
    final shares = sharesOf(split);
    final count = split.children.length;
    final palette = Palette.of(context);
    return LayoutBuilder(
      builder: (context, constraints) {
        final total = row ? constraints.maxWidth : constraints.maxHeight;
        // One hairline between each two.
        final room = math.max(0.0, total - (count - 1));
        final children = <Widget>[];
        final handles = <Widget>[];
        var at = 0.0;
        for (var i = 0; i < count; i++) {
          final child = _node(root, split.children[i], chat);
          if (i == count - 1) {
            children.add(Expanded(child: child));
            break;
          }
          final extent = (room * shares[i]).floorToDouble();
          children.add(
            SizedBox(
              width: row ? extent : null,
              height: row ? null : extent,
              child: child,
            ),
          );
          children.add(
            Container(
              width: row ? 1 : null,
              height: row ? null : 1,
              color: palette.hairline,
            ),
          );
          at += extent;
          final edge = at + 0.5 - ResizeHandle.width / 2;
          handles.add(
            Positioned(
              left: row ? edge : 0,
              right: row ? null : 0,
              top: row ? 0 : edge,
              bottom: row ? 0 : null,
              child: ResizeHandle(
                key: ValueKey('divider-${node.id}-$i'),
                axis: row ? Axis.horizontal : Axis.vertical,
                onDragStart: () => _from = shares,
                onDrag: (delta) => setState(
                  () => _dragged = resize(
                    root,
                    node.id,
                    _dividerMoved(split, i, delta, room, row),
                  ),
                ),
                onDragEnd: () => _change(_dragged),
                onReset: () => _change(resize(root, node.id, const [])),
              ),
            ),
          );
          at += 1;
        }
        return Stack(
          children: [
            Positioned.fill(
              child: Flex(
                direction: row ? Axis.horizontal : Axis.vertical,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: children,
              ),
            ),
            ...handles,
          ],
        );
      },
    );
  }

  /// The shares once the divider after child [i] has moved [delta] since
  /// its drag began: only the two beside it change, each kept at least its
  /// smallest size.
  List<double> _dividerMoved(
    ViewSplit split,
    int i,
    double delta,
    double room,
    bool row,
  ) {
    final shares = [..._from];
    if (room <= 0 || shares.length != split.children.length) return shares;
    final pair = shares[i] + shares[i + 1];
    final low = minExtent(split.children[i], row) / room;
    final high = pair - minExtent(split.children[i + 1], row) / room;
    if (low > high) return shares;
    final moved = (shares[i] + delta / room).clamp(low, high);
    shares[i] = moved;
    shares[i + 1] = pair - moved;
    return shares;
  }

  Widget _pane(ViewNode root, ViewNode node, Widget chat) {
    final agent = widget.agent;
    final leaf = node.leaf;
    final isChat = leaf.kind == chatKind;
    final content = isChat
        ? chat
        : KeyedSubtree(
            key: _keys.putIfAbsent(node.id, GlobalKey.new),
            child: switch (pageOfKind(leaf.kind)) {
              AppPage.settings => const _PagePane(child: SettingsView()),
              AppPage.marketplace => const _PagePane(child: MarketplaceView()),
              AppPage.changelog => const _PagePane(child: ChangelogView()),
              _ => Builder(builder: (context) => viewerBody(context, leaf)),
            },
          );
    return Pane(
      key: ValueKey('pane-${node.id}'),
      agent: agent,
      node: node,
      onClose: isChat ? null : () => _change(close(root, node.id)),
      onDrop: (id, edge) => _change(move(root, id, node.id, edge)),
      child: content,
    );
  }
}

/// A page in a pane ([InPane]): squeezed narrower than [pagePaneMinWidth]
/// (a narrow window, many panes), it keeps that width and scrolls sideways
/// rather than breaking its rows.
class _PagePane extends StatelessWidget {
  const _PagePane({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) => InPane(
    child: LayoutBuilder(
      builder: (context, constraints) {
        if (constraints.maxWidth >= pagePaneMinWidth) return child;
        return SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          child: SizedBox(
            width: pagePaneMinWidth,
            height: constraints.maxHeight,
            child: child,
          ),
        );
      },
    ),
  );
}

/// The smallest [node] can be along a row ([row]) or a column.
double minExtent(ViewNode node, bool row) {
  final own = row
      ? (pageOfKind(node.leaf.kind) != null ? pagePaneMinWidth : paneMinWidth)
      : paneMinHeight;
  if (!node.hasSplit()) return own;
  final split = node.split;
  final along = (split.axis != ViewAxis.VIEW_AXIS_COLUMN) == row;
  final sizes = [for (final child in split.children) minExtent(child, row)];
  return along
      ? sizes.fold<double>(0, (sum, size) => sum + size) + sizes.length - 1
      : sizes.reduce(math.max);
}

/// One pane: its header (the handle it's dragged by), what it shows, the
/// preview of where a pane dragged over it would land, and its glow when
/// an agent points at it.
class Pane extends StatefulWidget {
  const Pane({
    super.key,
    required this.agent,
    required this.node,
    required this.child,
    required this.onDrop,
    this.onClose,
  });

  final Agent agent;
  final ViewNode node;
  final Widget child;

  /// The pane [paneId] dropped at [edge] of this one.
  final void Function(String paneId, DropEdge edge) onDrop;

  /// Null for the conversation, which can't be closed.
  final VoidCallback? onClose;

  @override
  State<Pane> createState() => _PaneState();
}

class _PaneState extends State<Pane> {
  DropEdge? _edge;

  /// The last edge previewed, kept while the preview fades out.
  DropEdge? _previewed;

  /// Counts up when an agent shows this pane's file again, to read it
  /// again.
  final _reloads = ValueNotifier(0);
  ValueNotifier<({List<String> keys, DateTime at})?>? _pointedAt;

  String get _key => 'pane:${widget.agent.id}:${widget.node.id}';

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final pointedAt = AppScope.of(context).highlights.pointedAt;
    if (pointedAt != _pointedAt) {
      _pointedAt?.removeListener(_pointed);
      _pointedAt = pointedAt..addListener(_pointed);
    }
  }

  void _pointed() {
    if (_pointedAt?.value?.keys.contains(_key) ?? false) _reloads.value++;
  }

  @override
  void dispose() {
    _pointedAt?.removeListener(_pointed);
    _reloads.dispose();
    super.dispose();
  }

  /// The edge a pane dropped at [global] would go to: the one nearest the
  /// pointer, outside the middle third, if the halves would be big enough.
  DropEdge? _edgeAt(Offset global) {
    final box = context.findRenderObject() as RenderBox?;
    if (box == null || !box.hasSize) return null;
    final size = box.size;
    final local = box.globalToLocal(global);
    final x = local.dx / size.width;
    final y = local.dy / size.height;
    if (x < 0 || x > 1 || y < 0 || y > 1) return null;
    if (x > 1 / 3 && x < 2 / 3 && y > 1 / 3 && y < 2 / 3) return null;
    final distances = {
      DropEdge.left: x,
      DropEdge.right: 1 - x,
      DropEdge.top: y,
      DropEdge.bottom: 1 - y,
    };
    final edge = distances.entries
        .reduce((a, b) => b.value < a.value ? b : a)
        .key;
    final across = edge == DropEdge.left || edge == DropEdge.right;
    final room = across ? size.width : size.height;
    final least = across ? paneMinWidth : paneMinHeight;
    return room >= 2 * least + 1 ? edge : null;
  }

  @override
  Widget build(BuildContext context) {
    final agent = widget.agent;
    final node = widget.node;
    final isChat = node.leaf.kind == chatKind;
    // The conversation and a page (Settings…) under a header of their own;
    // a file's viewer fills its header with its details and controls.
    final ownHeader = isChat || pageOfKind(node.leaf.kind) != null;
    final backend = AppScope.of(context);
    Widget header(String details, List<Widget> controls) => PaneHeader(
      drag: PaneDrag(agent.id, node.id),
      glyph: (color) => paneGlyph(agent, node.leaf, color),
      title: paneTitle(node.leaf),
      details: details,
      controls: controls,
      onClose: widget.onClose,
    );
    final body = ownHeader
        ? Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              header('', const []),
              Expanded(child: widget.child),
            ],
          )
        : PaneChrome(header: header, reloads: _reloads, child: widget.child);
    return DragTarget<PaneDrag>(
      onWillAcceptWithDetails: (details) =>
          details.data.agentId == agent.id && details.data.paneId != node.id,
      onMove: (details) {
        final edge = _edgeAt(details.offset);
        if (edge != _edge) {
          setState(() {
            _edge = edge;
            if (edge != null) _previewed = edge;
          });
        }
      },
      onLeave: (_) => setState(() => _edge = null),
      onAcceptWithDetails: (details) {
        final edge = _edgeAt(details.offset);
        setState(() => _edge = null);
        if (edge != null) widget.onDrop(details.data.paneId, edge);
      },
      builder: (context, candidates, rejected) => Glow(
        store: backend.highlights,
        matches: (key) => key == _key,
        radius: 0,
        side: AxisDirection.left,
        child: Stack(
          fit: StackFit.expand,
          children: [
            body,
            Positioned.fill(
              child: IgnorePointer(
                child: DropPreview(
                  edge: _previewed,
                  shown: candidates.isNotEmpty && _edge != null,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// What a pane's header says it is.
String paneTitle(ViewLeaf leaf) {
  if (leaf.kind == chatKind) return 'Conversation';
  return switch (pageOfKind(leaf.kind)) {
    AppPage.settings => 'Settings',
    AppPage.marketplace => 'Marketplace',
    AppPage.changelog => "What's new",
    _ => viewerTitle(leaf),
  };
}

/// A pane's 14px glyph: the agent for the conversation, else its viewer's.
Widget paneGlyph(Agent agent, ViewLeaf leaf, Color color) {
  if (leaf.kind == chatKind) return AgentCharacter(agent: agent, size: 14);
  return switch (pageOfKind(leaf.kind)) {
    AppPage.settings => SizedBox.square(
      dimension: 14,
      child: CogGlyph(color: color),
    ),
    AppPage.marketplace => SizedBox.square(
      dimension: 14,
      child: BagGlyph(color: color),
    ),
    AppPage.changelog => ExtensionGlyph(
      icon: 'scroll-text',
      name: '',
      color: color,
      size: 14,
    ),
    _ => viewerIcon(leaf, color),
  };
}

/// A pane's 32px header: glyph, title, details, the viewer's controls and
/// its ×. It's the pane's handle: dragging it moves the pane.
class PaneHeader extends StatelessWidget {
  const PaneHeader({
    super.key,
    required this.drag,
    required this.glyph,
    required this.title,
    this.details = '',
    this.controls = const [],
    this.onClose,
  });

  final PaneDrag drag;
  final Widget Function(Color color) glyph;
  final String title;
  final String details;
  final List<Widget> controls;
  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final titleStyle = TextStyles.label.copyWith(color: palette.text);
    Widget named({required bool withDetails}) => Row(
      children: [
        SizedBox(width: 14, child: Center(child: glyph(palette.textMuted))),
        const SizedBox(width: Space.s),
        Flexible(
          child: Text(
            title,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: titleStyle,
          ),
        ),
        if (withDetails && details.isNotEmpty) ...[
          const SizedBox(width: Space.s),
          Flexible(
            child: Text(
              details,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          ),
        ],
      ],
    );
    final chip = DefaultTextStyle(
      style: titleStyle.copyWith(decoration: TextDecoration.none),
      child: Opacity(
        opacity: 0.8,
        child: Container(
          width: 240,
          height: paneHeaderHeight,
          padding: const EdgeInsets.symmetric(horizontal: Space.m),
          decoration: BoxDecoration(
            color: palette.sidebar,
            border: Border.all(color: palette.hairline),
            borderRadius: BorderRadius.circular(Radii.control),
            boxShadow: floatingShadow,
          ),
          child: named(withDetails: false),
        ),
      ),
    );
    return Draggable<PaneDrag>(
      data: drag,
      dragAnchorStrategy: pointerDragAnchorStrategy,
      feedback: Transform.translate(
        offset: const Offset(-Space.l, -paneHeaderHeight / 2),
        child: chip,
      ),
      child: MouseRegion(
        cursor: SystemMouseCursors.grab,
        child: Container(
          height: paneHeaderHeight,
          padding: const EdgeInsets.only(left: Space.m, right: Space.xs),
          decoration: BoxDecoration(
            color: palette.sidebar,
            border: Border(bottom: BorderSide(color: palette.hairline)),
          ),
          child: Row(
            children: [
              Expanded(child: named(withDetails: true)),
              for (final control in controls)
                Padding(
                  padding: const EdgeInsets.only(left: 2),
                  child: control,
                ),
              if (onClose != null)
                Padding(
                  padding: const EdgeInsets.only(left: 2),
                  child: GlyphButton(
                    label: 'Close',
                    active: false,
                    onTap: onClose!,
                    glyph: (color) =>
                        ExtensionGlyph(icon: 'x', name: 'Close', color: color),
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Where a dropped pane would land: that half of the pane, in signal.
class DropPreview extends StatelessWidget {
  const DropPreview({super.key, required this.edge, required this.shown});

  final DropEdge? edge;
  final bool shown;

  @override
  Widget build(BuildContext context) {
    final edge = this.edge;
    final box = DecoratedBox(
      decoration: BoxDecoration(
        color: signal.withValues(alpha: 0.12),
        border: Border.all(color: signal, width: 2),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
    );
    return AnimatedOpacity(
      opacity: shown && edge != null ? 1 : 0,
      duration: Motion.fast,
      curve: Curves.easeOut,
      child: edge == null
          ? const SizedBox.shrink()
          : Padding(
              padding: const EdgeInsets.all(Space.xs),
              child: FractionallySizedBox(
                alignment: switch (edge) {
                  DropEdge.left => Alignment.centerLeft,
                  DropEdge.right => Alignment.centerRight,
                  DropEdge.top => Alignment.topCenter,
                  DropEdge.bottom => Alignment.bottomCenter,
                },
                widthFactor: edge == DropEdge.left || edge == DropEdge.right
                    ? 0.5
                    : 1,
                heightFactor: edge == DropEdge.top || edge == DropEdge.bottom
                    ? 0.5
                    : 1,
                child: box,
              ),
            ),
    );
  }
}
