import 'dart:math' as math;
import 'dart:ui' show Size;

import '../layout.dart';
import '../panes.dart' show PaneView, pageOfKind;
import '../protocol/factory/v1/factory.pb.dart';

/// The iPad workbench's columns: how an agent's backend layout
/// (`Agent.layout`, the same tree the Mac shows as panes) is presented in
/// a window of a given size, without changing it.
///
/// This holds three rules, and nothing here keeps state:
///
/// 1. [tierFor] picks the window's composition from its safe size and the
///    text size: compact (the phone's routes), one focused pane, rail +
///    two columns, or rail + sidebar + two columns.
/// 2. [project] picks which of the layout's leaves are shown in the room
///    the composition leaves for content, keeping their ids, order and
///    relative shares; the rest are listed as hidden, for the pane picker.
///    Resizing the window never sends a new layout: only the person's own
///    Split, Open, Close or divider drag does ([resizeBack]).
/// 3. [compactRoutes] gives the same focus as the phone's routes: the
///    conversation, and the focused viewer pushed over it.
///
/// Design: docs/ipad-workbench.md. Spec: the Research folder's ipad.md, §1.

/// What kind of device the app runs on, from the native idiom
/// (`UIDevice.userInterfaceIdiom` through the device channel), never from
/// the window's size: a narrow iPad window is still an iPad.
enum Idiom { pad, phone, other }

/// The window's composition.
enum WorkbenchTier {
  /// The phone's routes and drawer: the conversation, a viewer pushed
  /// over it, panels as sheets. Every capability stays reachable.
  compact,

  /// One focused pane at a time, with a Chat/Document switch and the
  /// navigation as an overlay.
  focused,

  /// The rail, then the content's columns; the projects sidebar opens
  /// over them.
  paired,

  /// The rail, the projects sidebar pinned, then the content's columns.
  full,
}

/// The starting geometry from ipad.md §1, at the default text size. They
/// are proposals to measure, not Apple's numbers.
abstract final class WorkbenchGeometry {
  /// The Claktories rail.
  static const rail = 56.0;

  /// The projects sidebar, for choosing the tier; the shown sidebar keeps
  /// the person's width (`Settings.sidebar_width`).
  static const sidebar = 232.0;

  /// A column of reading text: the conversation, a page, a text file.
  static const textColumn = 320.0;

  /// A column showing a fixed-layout document: a PDF, an image, a video.
  /// It zooms rather than reflowing, so it doesn't grow with the text.
  static const documentColumn = 400.0;

  /// A project's Files.
  static const filesColumn = 240.0;

  /// Any pane, top to bottom: a touch-sized header and some content.
  static const paneHeight = 200.0;

  /// Narrower than this the window takes the compact routes.
  static const compactBelow = 600.0;

  /// Shorter than this too: below it there's no room for a header, some
  /// content and a composer (an iPhone on its side is 430 or less).
  static const shortBelow = 440.0;

  /// From this width, at the default text size, the rail and two columns.
  static const pairedFrom = 840.0;

  /// From this width, at the default text size, the sidebar is pinned too.
  static const fullFrom = 1180.0;

  /// The conversation's column in the wide tiers, which the sidebar and
  /// conversation's growth with larger text push the thresholds by.
  static const collaboration = 360.0;
}

/// How much larger than the default the person's text is, at least 1:
/// the factor the reading columns grow by.
double textFactorOf(double Function(double fontSize) scale) =>
    math.max(1.0, scale(17) / 17);

/// The width from which the rail and two columns fit, at [textFactor].
double pairedFrom(double textFactor) =>
    WorkbenchGeometry.pairedFrom +
    (textFactor - 1) * WorkbenchGeometry.collaboration;

/// The width from which the sidebar is pinned too, at [textFactor].
double fullFrom(double textFactor) =>
    WorkbenchGeometry.fullFrom +
    (textFactor - 1) *
        (WorkbenchGeometry.sidebar + WorkbenchGeometry.collaboration);

/// The composition for a window of [window] (its safe size: the view less
/// its safe-area padding, never less the keyboard) at [textFactor]
/// ([textFactorOf]). A phone always takes the compact routes, on its side
/// too; the workbench is the iPad's.
WorkbenchTier tierFor({
  required Idiom idiom,
  required Size window,
  double textFactor = 1,
}) {
  if (idiom != Idiom.pad) return WorkbenchTier.compact;
  if (window.width < WorkbenchGeometry.compactBelow ||
      window.height < WorkbenchGeometry.shortBelow) {
    return WorkbenchTier.compact;
  }
  if (window.width >= fullFrom(textFactor)) return WorkbenchTier.full;
  if (window.width >= pairedFrom(textFactor)) return WorkbenchTier.paired;
  return WorkbenchTier.focused;
}

/// The width left for the agent's panes in [tier], in a window [width]
/// wide, with the sidebar [sidebar] wide when it's pinned. One hairline
/// after the rail and after the sidebar.
double contentWidth(
  WorkbenchTier tier,
  double width, {
  double sidebar = WorkbenchGeometry.sidebar,
}) => switch (tier) {
  WorkbenchTier.full => width - WorkbenchGeometry.rail - sidebar - 2,
  WorkbenchTier.paired => width - WorkbenchGeometry.rail - 1,
  WorkbenchTier.focused || WorkbenchTier.compact => width,
};

bool _isChat(ViewNode node) => node.hasLeaf() && node.leaf.kind == chatKind;

/// The narrowest [leaf] is shown, at [textFactor].
double leafMinWidth(ViewNode leaf, double textFactor) {
  final kind = leaf.leaf.kind;
  if (kind == filesKind) return WorkbenchGeometry.filesColumn * textFactor;
  if (kind == chatKind || pageOfKind(kind) != null || kind == 'text') {
    return WorkbenchGeometry.textColumn * textFactor;
  }
  return WorkbenchGeometry.documentColumn;
}

double _minExtentOf(ViewNode node, bool row, double textFactor) =>
    minExtent(node, row, textFactor);

/// The smallest [node] can be along a row ([row]) or down a column, one
/// hairline between each two of a split's children.
double minExtent(ViewNode node, bool row, double textFactor) {
  if (!node.hasSplit()) {
    return row ? leafMinWidth(node, textFactor) : WorkbenchGeometry.paneHeight;
  }
  final along = (node.split.axis != ViewAxis.VIEW_AXIS_COLUMN) == row;
  final sizes = [
    for (final child in node.split.children) minExtent(child, row, textFactor),
  ];
  return along
      ? sizes.fold<double>(0, (sum, size) => sum + size) + sizes.length - 1
      : sizes.reduce(math.max);
}

/// The leaf the person is reading in [root]: [focused] while it's still
/// there, else the conversation. An agent's `show` adding a pane never
/// takes the focus by itself; the pane glows and waits in the picker.
String focusIn(ViewNode root, String? focused) {
  final all = leaves(root);
  if (focused != null && all.any((leaf) => leaf.id == focused)) return focused;
  return all.firstWhere(_isChat, orElse: () => all.first).id;
}

/// How an agent's layout is presented in the room its tier leaves.
class Projection {
  const Projection({
    required this.tier,
    required this.focused,
    required this.shown,
    required this.hidden,
  });

  final WorkbenchTier tier;

  /// The leaf the person is reading ([focusIn]).
  final String focused;

  /// The tree to lay out: the layout without its hidden leaves. Splits
  /// keep their ids, children their order, and visible children their
  /// shares relative to each other, raised where needed so each gets its
  /// least width. A split left with one child gives way to it; splits are
  /// never joined, so a shown split's id is always the layout's. Null in
  /// [WorkbenchTier.compact], whose routes are [compactRoutes].
  final ViewNode? shown;

  /// The leaves not shown, in the layout's order: what the pane picker
  /// lists. Never closed, never sent anywhere.
  final List<String> hidden;
}

/// [root] presented in [room] at [tier]. The focused leaf is always shown;
/// in [WorkbenchTier.focused] only it, so a switch between the
/// conversation and the document moves [focused] rather than the layout.
/// Wider, as many leaves as fit at their least sizes are kept, first the
/// focused, then the conversation, then the rest in the layout's order.
Projection project(
  ViewNode root, {
  required WorkbenchTier tier,
  required Size room,
  String? focused,
  double textFactor = 1,
}) {
  final focus = focusIn(root, focused);
  final all = leaves(root);
  if (tier == WorkbenchTier.compact) {
    return Projection(
      tier: tier,
      focused: focus,
      shown: null,
      hidden: [
        for (final leaf in all)
          if (leaf.id != focus) leaf.id,
      ],
    );
  }
  final order = [
    focus,
    for (final leaf in all)
      if (_isChat(leaf) && leaf.id != focus) leaf.id,
    for (final leaf in all)
      if (!_isChat(leaf) && leaf.id != focus) leaf.id,
  ];
  final most = tier == WorkbenchTier.focused ? 1 : order.length;
  ViewNode? shown;
  var kept = <String>{focus};
  for (var count = most; count >= 1; count--) {
    final keep = order.take(count).toSet();
    final pruned = _prune(root, keep)!;
    final fits =
        minExtent(pruned, true, textFactor) <= room.width &&
        minExtent(pruned, false, textFactor) <= room.height;
    if (fits || count == 1) {
      shown = pruned;
      kept = keep;
      break;
    }
  }
  return Projection(
    tier: tier,
    focused: focus,
    shown: _fitShares(shown!, room, textFactor),
    hidden: [
      for (final leaf in all)
        if (!kept.contains(leaf.id)) leaf.id,
    ],
  );
}

/// [node] with only the leaves in [keep]; null when none of them is in it.
ViewNode? _prune(ViewNode node, Set<String> keep) {
  if (!node.hasSplit()) return keep.contains(node.id) ? node : null;
  final split = node.split;
  final shares = sharesOf(split);
  final children = <ViewNode>[];
  final kept = <double>[];
  for (var i = 0; i < split.children.length; i++) {
    final child = _prune(split.children[i], keep);
    if (child == null) continue;
    children.add(child);
    kept.add(shares[i]);
  }
  if (children.isEmpty) return null;
  if (children.length == 1) return children.single;
  final total = kept.fold<double>(0, (sum, share) => sum + share);
  return ViewNode(
    id: node.id,
    split: ViewSplit(
      axis: split.axis,
      children: children,
      weights: [for (final share in kept) share / total],
    ),
  );
}

/// [node] with each split's shares raised where a child would get less
/// than its least size in [room], taken from the others in proportion to
/// what they have above theirs. Only the presentation: the layout's own
/// shares are untouched until the person drags a divider.
ViewNode _fitShares(ViewNode node, Size room, double textFactor) {
  if (!node.hasSplit()) return node;
  final split = node.split;
  final row = split.axis != ViewAxis.VIEW_AXIS_COLUMN;
  final count = split.children.length;
  final total = math.max(0.0, (row ? room.width : room.height) - (count - 1));
  final shares = sharesOf(split);
  final mins = [
    for (final child in split.children)
      total <= 0 ? 0.0 : minExtent(child, row, textFactor) / total,
  ];
  final fitted = _raise(shares, mins);
  final children = <ViewNode>[];
  for (var i = 0; i < count; i++) {
    final along = total * fitted[i];
    children.add(
      _fitShares(
        split.children[i],
        row ? Size(along, room.height) : Size(room.width, along),
        textFactor,
      ),
    );
  }
  return ViewNode(
    id: node.id,
    split: ViewSplit(axis: split.axis, children: children, weights: fitted),
  );
}

/// [shares] (summing to 1) with each at least its [mins] share, the room
/// for it taken from the others' surplus in proportion. When the mins
/// alone exceed the whole, each gets its min's part of the whole.
List<double> _raise(List<double> shares, List<double> mins) {
  final need = mins.fold<double>(0, (sum, min) => sum + min);
  if (need >= 1) return [for (final min in mins) min / need];
  var result = [...shares];
  // A few rounds settle it: each raise can push another below its min.
  for (var round = 0; round < shares.length; round++) {
    final short = [
      for (var i = 0; i < result.length; i++)
        math.max(0.0, mins[i] - result[i]),
    ];
    final missing = short.fold<double>(0, (sum, value) => sum + value);
    if (missing <= 1e-9) break;
    final surplus = [
      for (var i = 0; i < result.length; i++)
        short[i] > 0 ? 0.0 : math.max(0.0, result[i] - mins[i]),
    ];
    final available = surplus.fold<double>(0, (sum, value) => sum + value);
    if (available <= 0) break;
    final take = math.min(missing, available);
    result = [
      for (var i = 0; i < result.length; i++)
        short[i] > 0
            ? result[i] + short[i] * (take / missing)
            : result[i] - surplus[i] * (take / available),
    ];
  }
  return result;
}

/// [root] after the person drags a divider of the shown split [splitId]
/// to [shownShares] (the shown children's, summing to 1): the layout's
/// split gets them for its shown children, scaled to the share those
/// children had together, and its hidden children keep theirs. An empty
/// [shownShares] (a double-click reset) shares that room equally among the
/// shown children. This is what's sent (SetLayout); resizing the window
/// sends nothing.
ViewNode resizeBack(
  ViewNode root,
  Projection projection,
  String splitId,
  List<double> shownShares,
) {
  final real = find(root, splitId);
  if (real == null || !real.hasSplit()) return root;
  final hidden = projection.hidden.toSet();
  final split = real.split;
  final shares = sharesOf(split);
  final visible = [
    for (var i = 0; i < split.children.length; i++)
      if (leaves(split.children[i]).any((leaf) => !hidden.contains(leaf.id))) i,
  ];
  // A reset (an empty list) shares the shown children's room equally.
  final dragged = shownShares.isEmpty
      ? List.filled(visible.length, 1 / visible.length)
      : shownShares;
  if (visible.isEmpty || visible.length != dragged.length) return root;
  final together = visible.fold<double>(0, (sum, i) => sum + shares[i]);
  final next = [...shares];
  for (var j = 0; j < visible.length; j++) {
    next[visible[j]] = together * dragged[j];
  }
  return resize(root, splitId, next);
}

/// The phone's routes for [root] in the compact tier, bottom first: the
/// conversation, and the focused leaf pushed over it when that's a viewer,
/// a page or Files. Moving between compact and wider windows keeps the
/// person on what they were reading.
List<String> compactRoutes(ViewNode root, String? focused) {
  final focus = focusIn(root, focused);
  final chat = leaves(root)
      .firstWhere(_isChat, orElse: () => leaves(root).first);
  return [chat.id, if (focus != chat.id) focus];
}

/// A [Projection] as the panes lay it out (PaneLayout's [PaneView]): its
/// shown tree, the least sizes a divider drag keeps at [textFactor], and a
/// drag mapped back to the whole layout ([resizeBack]). Never compact:
/// a compact window takes the phone's routes and builds no PaneLayout.
class ProjectedPanes implements PaneView {
  ProjectedPanes(this.projection, {this.textFactor = 1})
    : assert(projection.shown != null, 'A compact window has no panes');

  final Projection projection;
  final double textFactor;

  @override
  ViewNode get shown => projection.shown!;

  @override
  List<String> get hidden => projection.hidden;

  @override
  String get focused => projection.focused;

  @override
  ViewNode resized(ViewNode full, String splitId, List<double> shownShares) =>
      resizeBack(full, projection, splitId, shownShares);

  @override
  double minExtent(ViewNode node, bool row) =>
      _minExtentOf(node, row, textFactor);
}
