import 'dart:math' as math;

import 'protocol/factory/v1/factory.pb.dart';

/// An agent's panes (`Agent.layout`) and what the person does with them:
/// split one by dropping another on its edge, close one, resize a split,
/// open a file. Each change makes a new tree for the backend (SetLayout);
/// nothing here is kept.

/// The kind of the conversation's pane, the one that can't be closed.
const chatKind = 'chat';

/// The kind of a project's Files pane; its path is the project's id.
const filesKind = 'files';

/// Whether [node] is a file's viewer: not the conversation, a page
/// (Settings…, which have no path) or a project's Files.
bool _isViewer(ViewNode node) =>
    !_isChat(node) && node.leaf.kind != filesKind && node.leaf.path.isNotEmpty;

/// At most this many panes (the backend refuses more).
const maxPanes = 12;

/// Where a pane is dropped on another: the edge it goes to.
enum DropEdge { left, right, top, bottom }

/// [agent]'s layout, or just the conversation when it has none.
ViewNode layoutOf(Agent agent) =>
    agent.hasLayout() && _valid(agent.layout) ? agent.layout : chatOnly();

bool _valid(ViewNode node) => leaves(node).any((leaf) => _isChat(leaf));

/// The layout with only the conversation.
ViewNode chatOnly() => ViewNode(
  id: 'chat',
  leaf: ViewLeaf(kind: chatKind),
);

bool _isChat(ViewNode node) => node.hasLeaf() && node.leaf.kind == chatKind;

/// Whether [root] is just the conversation, with no chrome to show.
bool isChatOnly(ViewNode root) => root.hasLeaf() && _isChat(root);

/// Every leaf in [root], in order.
List<ViewNode> leaves(ViewNode root) => root.hasSplit()
    ? [for (final child in root.split.children) ...leaves(child)]
    : [root];

ViewNode? find(ViewNode root, String id) {
  if (root.id == id) return root;
  if (!root.hasSplit()) return null;
  for (final child in root.split.children) {
    final found = find(child, id);
    if (found != null) return found;
  }
  return null;
}

var _counter = 0;
final _random = math.Random();

/// A new pane's id, unique enough among a dozen panes and other devices'.
String newPaneId([String prefix = 'pane']) {
  _counter++;
  final salt = _random.nextInt(1 << 32).toRadixString(36);
  return '$prefix-${DateTime.now().microsecondsSinceEpoch.toRadixString(36)}'
      '$salt$_counter';
}

/// The leaf of a viewer for [path].
ViewNode viewerLeaf(String path, {String? kind}) => ViewNode(
  id: newPaneId(),
  leaf: ViewLeaf(
    kind: kind ?? viewerKindFor(path),
    path: path,
    title: path.split('/').where((part) => part.isNotEmpty).lastOrNull ?? path,
  ),
);

/// The viewer that shows [path]: images, videos and PDFs by their
/// extension, else text (the text viewer says when a file isn't text).
String viewerKindFor(String path) {
  final dot = path.lastIndexOf('.');
  final extension = dot < 0 ? '' : path.substring(dot + 1).toLowerCase();
  if (const {'png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp'}.contains(extension)) {
    return 'image';
  }
  if (const {'mp4', 'm4v', 'mov'}.contains(extension)) return 'video';
  if (extension == 'pdf') return 'pdf';
  return 'text';
}

/// [root] without the pane [id]: a split left with one child gives way to
/// it. The conversation can't be removed; null when nothing changed.
ViewNode? close(ViewNode root, String id) {
  final node = find(root, id);
  if (node == null || _isChat(node)) return null;
  return _simplify(_without(root, id)!);
}

ViewNode? _without(ViewNode node, String id) {
  if (node.id == id) return null;
  if (!node.hasSplit()) return node;
  final children = <ViewNode>[];
  final weights = <double>[];
  final split = node.split;
  for (var i = 0; i < split.children.length; i++) {
    final kept = _without(split.children[i], id);
    if (kept == null) continue;
    children.add(kept);
    if (split.weights.length == split.children.length) {
      weights.add(split.weights[i]);
    }
  }
  if (children.isEmpty) return null;
  return _split(node.id, split.axis, children, weights);
}

/// Splits left with one child give way to it; a split inside one on the
/// same axis joins it, its shares scaled to the share it had.
ViewNode _simplify(ViewNode node) {
  if (!node.hasSplit()) return node;
  final split = node.split;
  final children = [for (final child in split.children) _simplify(child)];
  if (children.length == 1) return children.single;
  final shares = _shares(split.weights, children.length);
  final flat = <ViewNode>[];
  final flatShares = <double>[];
  for (var i = 0; i < children.length; i++) {
    final child = children[i];
    if (child.hasSplit() && child.split.axis == split.axis) {
      final inner = _shares(child.split.weights, child.split.children.length);
      for (var j = 0; j < child.split.children.length; j++) {
        flat.add(child.split.children[j]);
        flatShares.add(shares[i] * inner[j]);
      }
    } else {
      flat.add(child);
      flatShares.add(shares[i]);
    }
  }
  final equal = flatShares.every(
    (share) => (share - flatShares.first).abs() < 1e-9,
  );
  return _split(node.id, split.axis, flat, equal ? const [] : flatShares);
}

/// Each child's share of a split, summing to 1.
List<double> _shares(List<double> weights, int count) {
  if (weights.length != count || weights.any((weight) => weight <= 0)) {
    return List.filled(count, 1 / count);
  }
  final total = weights.fold<double>(0, (sum, weight) => sum + weight);
  return [for (final weight in weights) weight / total];
}

/// Each child's share of [split], summing to 1.
List<double> sharesOf(ViewSplit split) =>
    _shares(split.weights, split.children.length);

ViewNode _split(
  String id,
  ViewAxis axis,
  List<ViewNode> children,
  List<double> weights,
) => ViewNode(
  id: id,
  split: ViewSplit(axis: axis, children: children, weights: weights),
);

/// [root] with [pane] put at [edge] of the pane [targetId], the two
/// sharing its place equally. Null when nothing changes.
ViewNode? insert(ViewNode root, ViewNode pane, String targetId, DropEdge edge) {
  if (find(root, targetId) == null) return null;
  final axis = edge == DropEdge.left || edge == DropEdge.right
      ? ViewAxis.VIEW_AXIS_ROW
      : ViewAxis.VIEW_AXIS_COLUMN;
  final before = edge == DropEdge.left || edge == DropEdge.top;
  ViewNode place(ViewNode node) {
    if (node.id == targetId) {
      return _split(newPaneId('split'), axis, [
        if (before) pane,
        node,
        if (!before) pane,
      ], const []);
    }
    if (!node.hasSplit()) return node;
    return _split(node.id, node.split.axis, [
      for (final child in node.split.children) place(child),
    ], node.split.weights);
  }

  return _simplifyKeepingEqual(place(root));
}

/// [_simplify], except a split just made keeps its own equal halves rather
/// than joining its parent's shares.
ViewNode _simplifyKeepingEqual(ViewNode node) {
  if (!node.hasSplit()) return node;
  final children = [
    for (final child in node.split.children) _simplifyKeepingEqual(child),
  ];
  if (children.length == 1) return children.single;
  return _split(node.id, node.split.axis, children, node.split.weights);
}

/// [root] with the pane [id] moved to [edge] of [targetId]. Null when
/// nothing changes (dropped on itself, or either is gone).
ViewNode? move(ViewNode root, String id, String targetId, DropEdge edge) {
  if (id == targetId) return null;
  final pane = find(root, id);
  if (pane == null || find(root, targetId) == null) return null;
  // The conversation moves like any pane; it just can't be closed.
  final without = _without(root, id);
  if (without == null) return null;
  return insert(_simplify(without), pane, targetId, edge);
}

/// [root] with the split [id]'s shares set to [shares].
ViewNode resize(ViewNode root, String id, List<double> shares) {
  ViewNode walk(ViewNode node) {
    if (!node.hasSplit()) return node;
    final children = [for (final child in node.split.children) walk(child)];
    final weights = node.id == id ? shares : node.split.weights;
    return _split(node.id, node.split.axis, children, weights);
  }

  return walk(root);
}

/// [root] showing [path]: unchanged when a pane already shows it (that
/// pane's id is in [shown]), else with a viewer for it as an agent's `show`
/// adds one: beside the conversation, then stacked in a column right of
/// it. Opened from a project's Files pane ([beside]), the first viewer goes
/// right of that pane instead. At [maxPanes] the oldest viewer gives way.
({ViewNode root, String paneId}) open(
  ViewNode root,
  String path, {
  String? beside,
}) {
  for (final leaf in leaves(root)) {
    if (leaf.leaf.path == path && _isViewer(leaf)) {
      return (root: root, paneId: leaf.id);
    }
  }
  if (beside != null &&
      find(root, beside) != null &&
      !leaves(root).any(_isViewer)) {
    final pane = viewerLeaf(path);
    var tree = root;
    if (leaves(tree).length >= maxPanes) {
      final oldest = leaves(tree)
          .firstWhere((leaf) => !_isChat(leaf) && leaf.id != beside);
      tree = close(tree, oldest.id) ?? tree;
    }
    // In a row, right of the Files, taking half the room of the widest
    // pane there (the conversation, usually) so the Files keep theirs.
    final parent = _parentOf(tree, beside);
    if (parent != null && parent.split.axis == ViewAxis.VIEW_AXIS_ROW) {
      final split = parent.split;
      final shares = [..._shares(split.weights, split.children.length)];
      var widest = 0;
      for (var i = 1; i < shares.length; i++) {
        if (shares[i] > shares[widest]) widest = i;
      }
      final half = shares[widest] / 2;
      shares[widest] = half;
      final at = split.children.indexWhere((child) => child.id == beside);
      final children = [...split.children]..insert(at + 1, pane);
      shares.insert(at + 1, half);
      ViewNode place(ViewNode node) {
        if (!node.hasSplit()) return node;
        if (node.id == parent.id) {
          return _split(node.id, split.axis, children, shares);
        }
        return _split(node.id, node.split.axis, [
          for (final child in node.split.children) place(child),
        ], node.split.weights);
      }

      return (root: place(tree), paneId: pane.id);
    }
    return (
      root: insert(tree, pane, beside, DropEdge.right) ?? tree,
      paneId: pane.id,
    );
  }
  final pane = viewerLeaf(path);
  var tree = root;
  if (leaves(tree).length >= maxPanes) {
    final oldest = leaves(tree)
        .firstWhere((leaf) => !_isChat(leaf) && leaf.id != beside);
    tree = close(tree, oldest.id) ?? tree;
  }
  if (isChatOnly(tree)) {
    return (
      root: _split(newPaneId('split'), ViewAxis.VIEW_AXIS_ROW, [
        tree,
        pane,
      ], const []),
      paneId: pane.id,
    );
  }
  // Below the last viewer, so viewers stack beside the conversation.
  // From a Files pane, below the last file's viewer rather than a page.
  final last = leaves(tree).lastWhere(
    beside != null ? _isViewer : (leaf) => !_isChat(leaf),
    orElse: () => leaves(tree).last,
  );
  if (_isChat(last)) {
    return (
      root: insert(tree, pane, last.id, DropEdge.right)!,
      paneId: pane.id,
    );
  }
  final parent = _parentOf(tree, last.id);
  if (parent != null && parent.split.axis == ViewAxis.VIEW_AXIS_COLUMN) {
    // One more in the column, all sharing it equally.
    ViewNode append(ViewNode node) {
      if (!node.hasSplit()) return node;
      final children = [for (final child in node.split.children) append(child)];
      if (node.id != parent.id) {
        return _split(node.id, node.split.axis, children, node.split.weights);
      }
      return _split(node.id, node.split.axis, [...children, pane], const []);
    }

    return (root: append(tree), paneId: pane.id);
  }
  return (root: insert(tree, pane, last.id, DropEdge.bottom)!, paneId: pane.id);
}

/// The split holding the pane [id], or null at the top.
ViewNode? _parentOf(ViewNode node, String id) {
  if (!node.hasSplit()) return null;
  for (final child in node.split.children) {
    if (child.id == id) return node;
    final found = _parentOf(child, id);
    if (found != null) return found;
  }
  return null;
}
