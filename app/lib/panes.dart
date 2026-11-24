import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'app_views/app_view.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'explorer.dart' show FilesPane;
import 'highlights.dart';
import 'identity.dart';
import 'changelog.dart';
import 'layout.dart';
import 'marketplace.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart' show Unselectable;
import 'pane_chrome.dart';
import 'map.dart';
import 'monitors_overview.dart' show MonitorsOverview;
import 'mobile/back.dart'
    show BackHandle, CallbackBackToken, PhoneBack, PhoneBackScope;
import 'mobile/haptics.dart';
import 'mobile/device.dart' show WindowControls, windowControls;
import 'mobile/metrics.dart';
import 'settings_view.dart';
import 'viewers/registry.dart';
import 'zoom.dart' show ZoomScope;
import 'inbox.dart' show InboxView;
import 'ipad/columns.dart' show textFactorOf;
import 'ipad/document_host.dart' show ipadDocumentPane;
import 'ipad/workbench.dart' show keyboardOverWindow;
import 'review_answer.dart' show ReviewFrame;
import 'usage_view.dart';

/// A pane is at least this wide and this tall (docs/IDENTITY.md, "Panes
/// and viewers").
const paneMinWidth = 200.0;
const paneMinHeight = 120.0;

/// A page's pane (Settings…) is at least as wide as a small phone's screen,
/// which its rows are laid out for.
const pagePaneMinWidth = 320.0;

/// The conversation is never narrower than this at the default text size,
/// and grows with larger text ([chatLeastOf]): no pane beside it squeezes
/// it (Sale's A4, Bruno 23882).
const chatMinWidth = 360.0;

/// The conversation's least width at [context]'s text size.
double chatLeastOf(BuildContext context) =>
    chatMinWidth * textFactorOf(MediaQuery.textScalerOf(context).scale);

/// The room an agent's panes have across, and the conversation's least
/// there: what [withPage], [withFiles] and [withApp] fit a new pane into.
typedef PaneFit = ({double room, double chatLeast});

final _fits = Expando<PaneFit>('pane fit');

/// The room the shown agent's panes had when last laid out for [backend]:
/// kept while a page shows in the conversation's place, which has the same
/// room. Null until its panes are laid out without a [PaneProjection] (the
/// iPad's shows what fits by itself); nothing is closed to make room then.
PaneFit? paneFitOf(Backend backend) => _fits[backend];

/// Whether a pane at least [least] wide fits beside the conversation in
/// [fit], once the other panes give way. True while it isn't known.
bool fitsBesideChat(PaneFit? fit, double least) =>
    fit == null || fit.chatLeast + 1 + least <= fit.room;

/// A pane's header on a Mac.
const paneHeaderHeight = 32.0;

/// A pane's header: [paneHeaderHeight] with a pointer, at least a finger's
/// height on touch, growing with its text there.
double paneHeaderHeightOf(BuildContext context) =>
    touchFirst(context) ? Touch.target : paneHeaderHeight;

/// How a window presents an agent's layout when it hasn't room for all of
/// it: on an iPad, the panes that fit ([shown]) and the rest ([hidden]),
/// without changing the layout itself. The iPad's projection implements it
/// (ipad/columns.dart), so this file knows nothing of the iPad.
abstract interface class PaneView {
  /// The tree to lay out: the layout without its hidden leaves, splits and
  /// leaves keeping their ids.
  ViewNode get shown;

  /// The ids of the leaves not shown, in the layout's order.
  List<String> get hidden;

  /// The leaf the person is reading, whose header is marked on touch.
  String get focused;

  /// The whole layout after the person dragged the divider of [shown]'s
  /// split [splitId] to [shownShares]; an empty list resets it to equal
  /// shares among the shown.
  ViewNode resized(ViewNode full, String splitId, List<double> shownShares);

  /// The least [node] measures across ([row]) or down, as this window
  /// keeps it.
  double minExtent(ViewNode node, bool row);
}

/// Hands [PaneLayout] a way to present the layout in the room it has; with
/// none (the Mac), it lays out the whole layout. [project] is asked on
/// every build, with the room the panes have; [focus] is told of the pane
/// the person touches or clicks, and of one they just moved.
class PaneProjection extends InheritedWidget {
  const PaneProjection({
    super.key,
    required this.project,
    required this.focus,
    this.hold,
    required super.child,
  });

  final PaneView Function(ViewNode full, Size room) project;
  final void Function(String leafId) focus;

  /// While true, no pane's width may start to change (an iPad's Pencil
  /// stroke is down, and a width change drops it): the dividers and the
  /// headers start no drag. One already under way carries on.
  final ValueListenable<bool>? hold;

  static PaneProjection? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<PaneProjection>();

  @override
  bool updateShouldNotify(PaneProjection old) =>
      old.project != project || old.focus != focus || old.hold != hold;
}

/// Settings, the Marketplace and What's new open as panes in the selected
/// agent's layout on a Mac, right of its conversation, so it can be read
/// meanwhile (a phone shows them as pages). Their leaves have no path.
const pageKinds = {
  AppPage.settings: 'settings',
  AppPage.marketplace: 'marketplace',
  AppPage.changelog: 'changelog',
  AppPage.map: 'map',
  AppPage.usage: 'usage',
  AppPage.inbox: 'inbox',
  AppPage.monitors: 'monitors',
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
ViewNode withPage(ViewNode root, AppPage page, {PaneFit? fit}) => _besideChat(
  root,
  ViewNode(
    id: newPaneId(pageKinds[page]!),
    leaf: ViewLeaf(kind: pageKinds[page]!),
  ),
  fit: fit,
);

/// The pane showing [projectId]'s Files in [root], or null.
ViewNode? filesPane(ViewNode root, String projectId) {
  for (final leaf in leaves(root)) {
    if (leaf.leaf.kind == filesKind && leaf.leaf.path == projectId) {
      return leaf;
    }
  }
  return null;
}

/// [root] with [project]'s Files right of the conversation, as [withPage]
/// places a page, taking a quarter of the conversation's share: a file tree
/// needs less room than what's read beside it.
ViewNode withFiles(ViewNode root, Project project, {PaneFit? fit}) =>
    _besideChat(
      root,
      ViewNode(
        id: newPaneId(filesKind),
        leaf: ViewLeaf(kind: filesKind, path: project.id, title: project.name),
      ),
      share: 1 / 4,
      fit: fit,
    );

/// The pane showing [appId]'s [view] in [root] (empty: its first), or
/// null. [first] is the id of the app's first view, so a pane opened by
/// that id (an agent naming it) and one opened as "its first" are the
/// same pane.
ViewNode? appPane(
  ViewNode root,
  String appId, [
  String view = '',
  String first = '',
]) {
  String resolved(String id) => id.isEmpty ? first : id;
  for (final leaf in leaves(root)) {
    if (leaf.leaf.kind == appKind &&
        leaf.leaf.appId == appId &&
        resolved(leaf.leaf.view) == resolved(view)) {
      return leaf;
    }
  }
  return null;
}

/// [root] with [app]'s [view] (empty: its first) right of the
/// conversation, as [withPage] places a page.
ViewNode withApp(ViewNode root, AppInfo app, [String view = '', PaneFit? fit]) {
  final spec = view.isEmpty
      ? app.views.firstOrNull
      : app.views.where((v) => v.id == view).firstOrNull;
  return _besideChat(
    root,
    ViewNode(
      id: newPaneId(appKind),
      leaf: ViewLeaf(
        kind: appKind,
        appId: app.id,
        // Named, never "its first", so it's found by its id too.
        view: spec?.id ?? view,
        title: spec?.title ?? '',
      ),
    ),
    fit: fit,
  );
}

/// [root] with [pane] right of the conversation, taking [share] of the
/// conversation's share in a row. When it won't fit beside the
/// conversation and the others in [fit], the oldest panes give way
/// first, as at [maxPanes]; when it won't fit even alone, it's placed all
/// the same (the panes then scroll sideways, the conversation at its
/// least): [fitsBesideChat] says so first, for a page to open in place.
ViewNode _besideChat(
  ViewNode root,
  ViewNode pane, {
  double share = 0.5,
  PaneFit? fit,
}) {
  var tree = root;
  if (leaves(tree).length >= maxPanes) {
    final oldest = leaves(tree)
        .firstWhere((leaf) => leaf.leaf.kind != chatKind);
    tree = close(tree, oldest.id) ?? tree;
  }
  // Too narrow for it even alone: the others stay.
  if (fit != null && fit.chatLeast + 1 + minExtent(pane, true) > fit.room) {
    return _placeBesideChat(tree, pane, share);
  }
  while (fit != null) {
    final placed = _placeBesideChat(tree, pane, share);
    if (minExtent(placed, true, chatLeast: fit.chatLeast) <= fit.room) {
      return placed;
    }
    final oldest = leaves(tree)
        .where((leaf) => leaf.leaf.kind != chatKind)
        .firstOrNull;
    if (oldest == null) return placed;
    tree = close(tree, oldest.id) ?? tree;
  }
  return _placeBesideChat(tree, pane, share);
}

ViewNode _placeBesideChat(ViewNode tree, ViewNode pane, double share) {
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
    // Its room comes from the widest in the row, not the conversation
    // alone: a page opened beside a wide file takes from the file
    // (Abbacchio).
    var widest = at;
    for (var i = 0; i < shares.length; i++) {
      if (shares[i] > shares[widest]) widest = i;
    }
    final given = shares[widest] * share;
    final weights = [...shares]..[widest] = shares[widest] - given;
    return ViewNode(
      id: node.id,
      split: ViewSplit(
        axis: split.axis,
        children: [...split.children]..insert(at + 1, pane),
        weights: weights..insert(at + 1, given),
      ),
    );
  }

  final placed = place(tree);
  if (find(placed, pane.id) != null) return placed;
  final inserted = insert(tree, pane, chat.id, DropEdge.right);
  if (inserted == null) return tree;
  if (share == 0.5) return inserted;
  // The new split holds just the conversation and the pane.
  ViewNode? parent(ViewNode node) {
    if (!node.hasSplit()) return null;
    if (node.split.children.any((child) => child.id == pane.id)) return node;
    for (final child in node.split.children) {
      final found = parent(child);
      if (found != null) return found;
    }
    return null;
  }

  final split = parent(inserted);
  if (split == null || split.split.children.length != 2) return inserted;
  return resize(inserted, split.id, [1 - share, share]);
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

  /// The tree laid out while a split's divider is dragged: the shown one,
  /// when a [PaneView] presents the layout.
  ViewNode? _dragged;

  /// The dragged split's shares, as last moved.
  List<double> _dragShares = const [];

  /// Who's told of the pane the person touches ([PaneProjection.focus]).
  void Function(String leafId)? _focus;

  /// Each pane keeps its state (scroll, zoom, a draft) as it moves.
  final _chatKey = GlobalKey();
  final _keys = <String, GlobalKey>{};

  /// A divider's drag: the split's shares when it began, and its room.
  List<double> _from = const [];

  /// The pane expanded as last asked here ("" for none), until the backend
  /// has it.
  String? _sentFocus;

  /// The pane expanded to fill the window in this build, if any: it's laid
  /// out over the window ([_portal]), and a blank holds its place.
  String? _expanded;

  /// The panes there were as one was expanded: any since are what opened
  /// meanwhile, which the expanded view mentions.
  Set<String>? _before;

  /// Shows the expanded pane over the window, in the window's overlay, so
  /// dialogs and menus still float above it.
  final _portal = OverlayPortalController();

  /// The whole layout: as last changed here until the backend has it.
  ViewNode get _full => _sent ?? layoutOf(widget.agent);

  /// The window's Back, where there is one (Android, a phone), and while a
  /// pane is expanded, its hold on it: Back collapses the pane (#366 A3),
  /// as its own Back chip does. On Android, Back then closes the newest
  /// pane beside the conversation, one at a time, before it leaves the app
  /// (Sale's A3, Bruno 23882).
  PhoneBack? _back;
  BackHandle? _backHandle;

  /// The leaves as they appeared here, oldest first: between panes opened
  /// at the same time ([_newest]), the later is the newer. Those there when
  /// the agent was shown count in the layout's order.
  final _seen = <String>[];

  /// The conversation's least width at this text size.
  double _chatLeast = chatMinWidth;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final back = context
        .dependOnInheritedWidgetOfExactType<PhoneBackScope>()
        ?.back;
    if (identical(back, _back)) return;
    // Another window or layout owns Back now: hold it there instead.
    _backHandle?.release();
    _backHandle = null;
    _back = back;
  }

  /// Holds the window's Back while [expanded], else lets go of it.
  void _holdBack(bool expanded) {
    if (expanded == (_backHandle != null)) return;
    if (!expanded) {
      _backHandle!.release();
      _backHandle = null;
      return;
    }
    _backHandle = _back?.registerNavigation(
      CallbackBackToken(
        onBack: () {
          if (!mounted) return;
          if (_expanded != null) return _expand(null);
          final newest = _newest(_full);
          if (newest != null) _change(close(_full, newest));
        },
      ),
    );
  }

  /// The newest pane beside the conversation in [full], if any: the one
  /// opened last (`ViewLeaf.opened_ms`: by the person or an agent, a file
  /// shown again too), then the one that appeared here last (Sale's row 25).
  String? _newest(ViewNode full) {
    ViewNode? newest;
    var newestSeen = -1;
    for (final leaf in leaves(full)) {
      if (leaf.leaf.kind == chatKind) continue;
      final seen = _seen.indexOf(leaf.id);
      final opened = leaf.leaf.openedMs;
      if (newest == null ||
          opened > newest.leaf.openedMs ||
          (opened == newest.leaf.openedMs && seen > newestSeen)) {
        newest = leaf;
        newestSeen = seen;
      }
    }
    return newest?.id;
  }

  /// Notes the leaves of [full] that are new here, as the newest.
  void _see(ViewNode full) {
    final ids = [for (final leaf in leaves(full)) leaf.id];
    _seen
      ..removeWhere((id) => !ids.contains(id))
      ..addAll(ids.where((id) => !_seen.contains(id)));
  }

  /// Panes beside the conversation are Back targets on Android only: an
  /// iPad has no Back, and a phone's panes are pages of its own.
  bool get _panesTakeBack => defaultTargetPlatform == TargetPlatform.android;

  /// Notes the room the panes have across, for a pane opened next.
  void _room(double room) {
    if (!room.isFinite) return;
    _fits[AppScope.of(context)] = (room: room, chatLeast: _chatLeast);
  }

  @override
  void dispose() {
    _backHandle?.release();
    super.dispose();
  }

  @override
  void initState() {
    super.initState();
    _portal.show();
  }

  @override
  void didUpdateWidget(PaneLayout old) {
    super.didUpdateWidget(old);
    if (old.agent.id != widget.agent.id) {
      _seen.clear();
      _sent = null;
      _dragged = null;
      _sentFocus = null;
      _before = null;
    }
  }

  /// The pane [full] has expanded, if it's still there.
  String? _expandedIn(ViewNode full) {
    final id = _sentFocus ?? widget.agent.layoutFocus;
    if (id.isEmpty) return null;
    return leaves(full).any((leaf) => leaf.id == id) ? id : null;
  }

  /// Expands pane [id] to fill the window, or with null puts it back:
  /// the backend keeps it (`Agent.layout_focus`), so it lasts.
  void _expand(String? id) {
    final focus = id ?? '';
    // Over whatever floats in the window now (a hover card, say).
    if (id != null) {
      _portal
        ..hide()
        ..show();
    }
    setState(() => _sentFocus = focus);
    fireAndLog(
      AppScope.of(context)
          .setLayout(widget.agent.id, _full, focus: focus)
          .whenComplete(() {
            if (mounted && _sentFocus == focus) {
              setState(() => _sentFocus = null);
            }
          }),
    );
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
    final full = _full;
    final chat = KeyedSubtree(key: _chatKey, child: widget.chat);
    _chatLeast = chatLeastOf(context);
    final projection = PaneProjection.maybeOf(context);
    _see(full);
    if (isChatOnly(full)) {
      _keys.clear();
      _expanded = null;
      _holdBack(false);
      if (projection != null) return chat;
      return LayoutBuilder(
        builder: (context, constraints) {
          _room(constraints.maxWidth);
          return chat;
        },
      );
    }
    // Every leaf of the layout keeps its key, shown or not, so a pane the
    // projection moves to another split keeps its state. A hidden pane
    // isn't built: it comes back as it opens (a video stopped, not
    // playing unseen).
    final ids = {for (final leaf in leaves(full)) leaf.id};
    _keys.removeWhere((id, _) => !ids.contains(id));
    _focus = projection?.focus;
    final expanded = _expanded = _expandedIn(full);
    _holdBack(expanded != null || (_panesTakeBack && projection == null));
    if (expanded == null) _before = null;
    final before = expanded == null ? null : (_before ??= ids);
    return OverlayPortal(
      controller: _portal,
      overlayChildBuilder: (context) => expanded == null
          ? const SizedBox.shrink()
          : ExpandedPane(
              agent: widget.agent,
              opened: [
                for (final leaf in leaves(full))
                  if (!before!.contains(leaf.id)) leaf.leaf,
              ],
              onBack: () => _expand(null),
              child: _pane(
                full,
                null,
                leaves(full).firstWhere((leaf) => leaf.id == expanded),
                chat,
                expanded: true,
              ),
            ),
      child: projection == null
          ? LayoutBuilder(
              builder: (context, constraints) {
                _room(constraints.maxWidth);
                return _laid(full, null, _dragged ?? full, chat);
              },
            )
          : LayoutBuilder(
              builder: (context, constraints) {
                // Asked every time: what fits follows the room and the
                // focus.
                final view = projection.project(full, constraints.biggest);
                return _laid(full, view, _dragged ?? view.shown, chat);
              },
            ),
    );
  }

  /// [laid], the tree shown, presenting [full] through [view] if any.
  Widget _laid(ViewNode full, PaneView? view, ViewNode laid, Widget chat) =>
      isChatOnly(laid) ? chat : _node(full, view, laid, laid, chat);

  Widget _node(
    ViewNode full,
    PaneView? view,
    ViewNode laid,
    ViewNode node,
    Widget chat,
  ) {
    if (!node.hasSplit()) return _pane(full, view, node, chat);
    final split = node.split;
    final row = split.axis != ViewAxis.VIEW_AXIS_COLUMN;
    final count = split.children.length;
    final palette = Palette.of(context);
    return LayoutBuilder(
      builder: (context, constraints) {
        final total = row ? constraints.maxWidth : constraints.maxHeight;
        // One hairline between each two.
        final room = math.max(0.0, total - (count - 1));
        // Each at least as big as it can be used (the conversation squeezed
        // to a word a line beside two pages and a file, Abbacchio): the room
        // comes from those that have more.
        final least = [
          for (final child in split.children)
            minExtent(child, row, chatLeast: _chatLeast),
        ];
        final need = least.fold<double>(0, (sum, each) => sum + each);
        final holdsChat = split.children.indexWhere(
          (child) => leaves(child).any((leaf) => leaf.leaf.kind == chatKind),
        );
        // Too narrow for them all (a window narrowed since they opened):
        // the conversation keeps its least, and those beside it share the
        // rest, a page scrolling sideways ([PageSideways]), rather than the
        // conversation squeezed to a word a line (Sale's A4).
        final shares = row && need > room && holdsChat >= 0
            ? keepConversation(sharesOf(split), least, holdsChat, room)
            : fitShares(sharesOf(split), least, room);
        final children = <Widget>[];
        final handles = <Widget>[];
        var at = 0.0;
        for (var i = 0; i < count; i++) {
          final child = _node(full, view, laid, split.children[i], chat);
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
          final edge = at + 0.5 - ResizeHandle.widthOf(context) / 2;
          handles.add(
            Positioned(
              left: row ? edge : 0,
              right: row ? null : 0,
              top: row ? 0 : edge,
              bottom: row ? 0 : null,
              child: _Held(
                hold: PaneProjection.maybeOf(context)?.hold,
                child: ResizeHandle(
                  key: ValueKey('divider-${node.id}-$i'),
                  axis: row ? Axis.horizontal : Axis.vertical,
                  onDragStart: () => _from = shares,
                  onDrag: (delta) => setState(() {
                    _dragShares = _dividerMoved(
                      view,
                      split,
                      i,
                      delta,
                      room,
                      row,
                    );
                    _dragged = resize(laid, node.id, _dragShares);
                  }),
                  onDragEnd: () => _change(
                    _dragged == null || view == null
                        ? _dragged
                        : view.resized(full, node.id, _dragShares),
                  ),
                  onReset: () => _change(
                    view == null
                        ? resize(full, node.id, const [])
                        : view.resized(full, node.id, const []),
                  ),
                ),
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
    PaneView? view,
    ViewSplit split,
    int i,
    double delta,
    double room,
    bool row,
  ) {
    final shares = [..._from];
    if (room <= 0 || shares.length != split.children.length) return shares;
    final least =
        view?.minExtent ??
        (ViewNode node, bool row) =>
            minExtent(node, row, chatLeast: _chatLeast);
    final pair = shares[i] + shares[i + 1];
    final low = least(split.children[i], row) / room;
    final high = pair - least(split.children[i + 1], row) / room;
    if (low > high) return shares;
    final moved = (shares[i] + delta / room).clamp(low, high);
    shares[i] = moved;
    shares[i + 1] = pair - moved;
    return shares;
  }

  /// The pane for leaf [node]; Close and Move change [full], by id. The
  /// pane expanded over the window is built there ([expanded]), with a
  /// blank in its place here.
  Widget _pane(
    ViewNode full,
    PaneView? view,
    ViewNode node,
    Widget chat, {
    bool expanded = false,
  }) {
    if (!expanded && node.id == _expanded) {
      return ColoredBox(color: Palette.of(context).canvas);
    }
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
              // A canvas pans by itself: no sideways scroll around it.
              AppPage.map => const InPane(child: MapView()),
              AppPage.usage => const _PagePane(child: UsageView()),
              AppPage.monitors => const _PagePane(child: MonitorsOverview()),
              AppPage.inbox => const _PagePane(child: InboxView()),
              _ when leaf.kind == filesKind => FilesPane(
                projectId: leaf.path,
                paneId: node.id,
              ),
              _ when leaf.kind == appKind => AppPane(leaf: leaf),
              _ => Builder(
                builder: (context) {
                  // While an ask to review it waits, its answer's bar.
                  Widget viewer(BuildContext context) => ReviewFrame(
                    agentId: agent.id,
                    path: leaf.path,
                    child: viewerBody(context, leaf),
                  );
                  return ipadDocumentPane(context, node, viewer: viewer) ??
                      viewer(context);
                },
              ),
            },
          );
    return Pane(
      key: ValueKey('pane-${node.id}'),
      agent: agent,
      node: node,
      onClose: isChat ? null : () => _change(close(full, node.id)),
      onDrop: (id, edge) {
        _change(move(full, id, node.id, edge));
        // The pane moved is the one being read.
        if (view != null) _focus?.call(id);
      },
      focused: view != null && view.focused == node.id,
      onFocus: view == null ? null : () => _focus?.call(node.id),
      expanded: expanded,
      // The conversation is the view itself: nothing to expand it from.
      onExpand: isChat ? null : () => _expand(expanded ? null : node.id),
      child: content,
    );
  }
}

/// A pane expanded to fill the window, to read in peace: over everything
/// but the dialogs and menus floating in it, under a band with the way
/// back ("Back to June", or Esc) and, once panes open meanwhile, a quiet
/// note of them. The layout stays as it was under it.
class ExpandedPane extends StatefulWidget {
  const ExpandedPane({
    super.key,
    required this.agent,
    required this.opened,
    required this.onBack,
    required this.child,
  });

  final Agent agent;

  /// Panes opened since it was expanded, oldest first.
  final List<ViewLeaf> opened;
  final VoidCallback onBack;
  final Widget child;

  @override
  State<ExpandedPane> createState() => _ExpandedPaneState();
}

class _ExpandedPaneState extends State<ExpandedPane> {
  final _node = FocusNode(debugLabel: 'Expanded pane');

  @override
  void initState() {
    super.initState();
    // The keys come here, not to a message box left under it.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && !_node.hasFocus) _node.requestFocus();
    });
  }

  @override
  void dispose() {
    _node.dispose();
    super.dispose();
  }

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.escape) {
      // Typing in a field (an answer, a comment): Escape leaves the field,
      // its words kept; only the next goes back (Bruno, after Matt's
      // Space in a video's comment).
      if (typingInField()) {
        FocusManager.instance.primaryFocus?.unfocus();
        _node.requestFocus();
        return KeyEventResult.handled;
      }
      widget.onBack();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: windowControls,
    builder: (context, controls, _) => _build(context, controls),
  );

  Widget _build(BuildContext context, WindowControls controls) {
    final palette = Palette.of(context);
    // Where the window's controls sit, at its top left: the Mac's traffic
    // lights; a windowed iPad's dots, which the band goes beside, level
    // with them, as Apple's apps put their toolbars.
    // The lights don't zoom with the app: their room is fixed in the
    // window's points (Abbacchio: at 80% the chip slid under them).
    final zoom = ZoomScope.of(context)?.level ?? 1;
    final lights = defaultTargetPlatform == TargetPlatform.macOS
        ? 80.0 / zoom
        : Space.m + controls.leading;
    final band = WindowDrag(
      child: Container(
        // The title bar's height, taller when large text needs it.
        constraints: const BoxConstraints(minHeight: titleBarHeight),
        padding: EdgeInsets.only(
          left: lights,
          right: Space.m,
          top: Space.xs,
          bottom: Space.xs,
        ),
        decoration: BoxDecoration(
          color: palette.sidebar,
          border: Border(bottom: BorderSide(color: palette.hairline)),
        ),
        child: Row(
          children: [
            Flexible(
              child: _BandChip(
                label: 'Back to ${widget.agent.name}',
                onTap: widget.onBack,
                leading: (color) => Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    ExtensionGlyph(
                      icon: 'arrow-left',
                      name: 'Back',
                      color: color,
                      size: 14,
                    ),
                    const SizedBox(width: Space.xs + 2),
                    AgentCharacter(agent: widget.agent, size: 20),
                  ],
                ),
              ),
            ),
            const Spacer(),
            if (widget.opened.isNotEmpty)
              Flexible(
                child: _BandChip(
                  label: widget.opened.length == 1
                      ? '${widget.agent.name} opened '
                            '${paneTitle(widget.opened.single, AppScope.of(context).workspace.workspace)}'
                      : '${widget.agent.name} opened '
                            '${widget.opened.length} more',
                  onTap: widget.onBack,
                  leading: (_) => const Dot(color: signal, size: 7),
                ),
              ),
          ],
        ),
      ),
    );
    final still = MediaQuery.disableAnimationsOf(context);
    return Focus(
      focusNode: _node,
      onKeyEvent: _key,
      child: TweenAnimationBuilder<double>(
        tween: Tween(begin: still ? 1 : 0, end: 1),
        duration: Motion.base,
        curve: Curves.easeOut,
        builder: (context, opacity, child) =>
            Opacity(opacity: opacity, child: child),
        child: ColoredBox(
          color: palette.canvas,
          child: Padding(
            // Below an iPad's status bar, not its window controls: the
            // band sits beside them. Above the on-screen keyboard, so a
            // field being written in (a review's answer) stays in sight.
            padding: EdgeInsets.only(
              top: math.max(
                0,
                MediaQuery.paddingOf(context).top - controls.top,
              ),
              bottom: keyboardOverWindow(context),
            ),
            child: MediaQuery.removeViewInsets(
              context: context,
              removeBottom: true,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Unselectable(child: band),
                  Expanded(child: widget.child),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A 28px capsule in the expanded pane's band, like the agent bar's back
/// chip: what's at its start, then [label]. Taller when large text needs
/// it.
class _BandChip extends StatelessWidget {
  const _BandChip({
    required this.label,
    required this.onTap,
    required this.leading,
  });

  final String label;
  final VoidCallback onTap;
  final Widget Function(Color color) leading;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      label: label,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => AnimatedContainer(
          duration: Motion.fast,
          constraints: const BoxConstraints(minHeight: 28),
          padding: const EdgeInsets.only(
            left: Space.s - 2,
            right: Space.s + 2,
            top: 2,
            bottom: 2,
          ),
          decoration: BoxDecoration(
            color: hovered ? palette.selected : palette.canvas,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(color: palette.hairline),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              leading(hovered ? palette.text : palette.textMuted),
              const SizedBox(width: Space.xs + 2),
              Flexible(
                child: Text(
                  label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TypeScale.of(context).label
                      .copyWith(color: palette.text),
                ),
              ),
            ],
          ),
        ),
      ),
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
        return PageSideways(height: constraints.maxHeight, child: child);
      },
    ),
  );
}

/// A page squeezed below [pagePaneMinWidth]: it keeps that width and
/// scrolls sideways, and the edge it's cut at fades out, so what's past
/// it reads as more to scroll to rather than cut off (horizontal scroll
/// views have no scrollbar).
@visibleForTesting
class PageSideways extends StatefulWidget {
  const PageSideways({super.key, required this.height, required this.child});

  final double height;
  final Widget child;

  @override
  State<PageSideways> createState() => PageSidewaysState();
}

@visibleForTesting
class PageSidewaysState extends State<PageSideways> {
  /// Whether some of the page is past the left edge, and past the right:
  /// that edge fades out.
  var _before = false;
  var _after = true;

  bool get fadesLeft => _before;
  bool get fadesRight => _after;

  bool _follow(ScrollNotification notification) {
    // Only this scroll view's own: the page's lists scroll inside it.
    if (notification.depth != 0) return false;
    _place(notification.metrics);
    return false;
  }

  bool _measured(ScrollMetricsNotification notification) {
    if (notification.depth == 0) _place(notification.metrics);
    return false;
  }

  void _place(ScrollMetrics metrics) {
    final before = metrics.extentBefore > 0.5;
    final after = metrics.extentAfter > 0.5;
    if (before != _before || after != _after) {
      setState(() {
        _before = before;
        _after = after;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    const shown = Color(0xFFFFFFFF);
    const gone = Color(0x00FFFFFF);
    return NotificationListener<ScrollMetricsNotification>(
      onNotification: _measured,
      child: NotificationListener<ScrollNotification>(
        onNotification: _follow,
        // Always masked, so the scroll view keeps its place in the tree
        // (and its offset) as the fades come and go.
        child: ShaderMask(
          blendMode: BlendMode.dstIn,
          shaderCallback: (bounds) {
            final fade = math.min(0.5, Space.xl / math.max(bounds.width, 1));
            return LinearGradient(
              colors: [
                _before ? gone : shown,
                shown,
                shown,
                _after ? gone : shown,
              ],
              stops: [0, fade, 1 - fade, 1],
            ).createShader(bounds);
          },
          child: SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: SizedBox(
              width: pagePaneMinWidth,
              height: widget.height,
              child: widget.child,
            ),
          ),
        ),
      ),
    );
  }
}

/// [shares] of [room], each raised to its least ([least]) where it can
/// be, the difference taken from those above theirs, in proportion to what
/// they have to spare. When even the least of each won't fit, as they are.
List<double> fitShares(List<double> shares, List<double> least, double room) {
  if (room <= 0 || shares.length != least.length) return shares;
  final need = least.fold<double>(0, (sum, each) => sum + each);
  if (need > room) return shares;
  final sizes = [for (final share in shares) share * room];
  // Raising one can push another under its least: settle in a few rounds.
  for (var round = 0; round < shares.length; round++) {
    var short = 0.0;
    var spare = 0.0;
    for (var i = 0; i < sizes.length; i++) {
      if (sizes[i] < least[i]) {
        short += least[i] - sizes[i];
        sizes[i] = least[i];
      } else {
        spare += sizes[i] - least[i];
      }
    }
    if (short < 0.5 || spare <= 0) break;
    final keep = math.max(0.0, 1 - short / spare);
    for (var i = 0; i < sizes.length; i++) {
      if (sizes[i] > least[i]) {
        sizes[i] = least[i] + (sizes[i] - least[i]) * keep;
      }
    }
  }
  return [for (final size in sizes) size / room];
}

/// [shares] of [room] when even the least of each won't fit: the one at
/// [keep] (holding the conversation) gets its least, leaving the others at
/// least a sliver each, and the others share the rest as they had it.
List<double> keepConversation(
  List<double> shares,
  List<double> least,
  int keep,
  double room,
) {
  if (room <= 0 || shares.length != least.length || shares.length < 2) {
    return shares;
  }
  final others = shares.length - 1;
  final sliver = math.min(paneMinWidth / 2, room / shares.length);
  final kept = math.max(0.0, math.min(least[keep], room - sliver * others));
  var theirs = 0.0;
  for (var i = 0; i < shares.length; i++) {
    if (i != keep) theirs += shares[i];
  }
  return [
    for (var i = 0; i < shares.length; i++)
      i == keep
          ? kept / room
          : theirs <= 0
          ? (1 - kept / room) / others
          : (1 - kept / room) * shares[i] / theirs,
  ];
}

/// The smallest [node] can be along a row ([row]) or a column, the
/// conversation at least [chatLeast] across.
double minExtent(ViewNode node, bool row, {double chatLeast = chatMinWidth}) {
  final own = !row
      ? paneMinHeight
      : node.leaf.kind == chatKind
      ? chatLeast
      : pageOfKind(node.leaf.kind) != null
      ? pagePaneMinWidth
      : paneMinWidth;
  if (!node.hasSplit()) return own;
  final split = node.split;
  final along = (split.axis != ViewAxis.VIEW_AXIS_COLUMN) == row;
  final sizes = [
    for (final child in split.children)
      minExtent(child, row, chatLeast: chatLeast),
  ];
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
    this.focused = false,
    this.onFocus,
    this.expanded = false,
    this.onExpand,
  });

  final Agent agent;
  final ViewNode node;
  final Widget child;

  /// Filling the window ([ExpandedPane]); [onExpand] expands it, or puts
  /// it back while it's expanded. Null for the conversation.
  final bool expanded;
  final VoidCallback? onExpand;

  /// The pane being read, among those a window shows (an iPad's): its
  /// header is marked on touch.
  final bool focused;

  /// Told when the pane is touched or clicked anywhere, without taking the
  /// touch from what's in it; null on a Mac.
  final VoidCallback? onFocus;

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
    Widget header(
      String details,
      List<Widget> controls, {
      List<Widget> pinned = const [],
    }) => PaneHeader(
      drag: PaneDrag(agent.id, node.id),
      glyph: (color) => paneGlyph(agent, node.leaf, color),
      title: paneTitle(node.leaf, backend.workspace.workspace),
      details: details,
      controls: controls,
      pinned: pinned,
      onClose: widget.onClose,
      focused: widget.focused,
      expanded: widget.expanded,
      onExpand: widget.onExpand,
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
    final target = DragTarget<PaneDrag>(
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
    final onFocus = widget.onFocus;
    if (onFocus == null) return target;
    return Listener(
      behavior: HitTestBehavior.translucent,
      onPointerDown: (_) => onFocus(),
      child: target,
    );
  }
}

/// A pane's kind for an app's view (ViewLeaf.app_id, view and params).
const appKind = 'app';

/// An app's view as Claktory's chrome names it: the app's name first,
/// always, then the view's title ("Charts · Days"), since the title is the
/// app's own words and could pass itself off as Claktory's (Fugo's rule);
/// "This app" for an app [workspace] doesn't have (or none was given). A
/// view titled as its app is named once ("Charts", not "Charts · Charts").
String appViewTitle(ViewLeaf leaf, Workspace? workspace) {
  final app = workspace == null ? null : _appOf(workspace, leaf.appId);
  final name = app == null || app.name.isEmpty ? 'This app' : app.name;
  final title = leaf.title.trim();
  return title.isEmpty || title.toLowerCase() == name.toLowerCase()
      ? name
      : '$name · $title';
}

/// An app's view in a pane: [AppView], over the backend, framed with the
/// app's name as Claktory knows it (never a title the app chose), or
/// "This app" while Claktory doesn't know it.
class AppPane extends StatelessWidget {
  const AppPane({super.key, required this.leaf});

  final ViewLeaf leaf;

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final name = _appOf(backend.workspace.workspace, leaf.appId)?.name;
        // The pane's header (or a phone's page header), as a viewer has
        // one; its title names the app ([appViewTitle]).
        return PaneScaffold(
          body: AppView(
            transport: backend,
            appId: leaf.appId,
            appName: name ?? '',
            view: leaf.view,
            params: leaf.params,
          ),
        );
      },
    );
  }
}

AppInfo? _appOf(Workspace workspace, String id) {
  for (final app in workspace.apps) {
    if (app.id == id) return app;
  }
  return null;
}

/// An app's own glyph, as Settings → Apps shows it: its Lucide icon, or
/// its name's first letter; a dot while Claktory doesn't know the app.
class _AppGlyph extends StatelessWidget {
  const _AppGlyph({required this.appId, required this.color});

  final String appId;
  final Color color;

  @override
  Widget build(BuildContext context) {
    final workspace = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: workspace,
      builder: (context, _) {
        final app = _appOf(workspace.workspace, appId);
        if (app == null) {
          return SizedBox.square(
            dimension: 14,
            child: Center(child: Dot(color: color)),
          );
        }
        return ExtensionGlyph(
          icon: app.icon,
          name: app.name,
          color: color,
          size: 14,
        );
      },
    );
  }
}

/// What a pane's header says it is. Give [workspace] wherever there is one:
/// an app's view is named for its app from it (see [appViewTitle]).
String paneTitle(ViewLeaf leaf, [Workspace? workspace]) {
  if (leaf.kind == chatKind) return 'Conversation';
  if (leaf.kind == filesKind) return 'Files';
  if (leaf.kind == appKind) return appViewTitle(leaf, workspace);
  return switch (pageOfKind(leaf.kind)) {
    AppPage.settings => 'Settings',
    AppPage.marketplace => 'Marketplace',
    AppPage.changelog => "What's new",
    AppPage.map => 'Map',
    AppPage.usage => 'Usage',
    AppPage.inbox => 'Needs you',
    AppPage.monitors => 'Monitors',
    _ => viewerTitle(leaf),
  };
}

/// A pane's 14px glyph: the agent for the conversation, else its viewer's.
Widget paneGlyph(Agent agent, ViewLeaf leaf, Color color) {
  if (leaf.kind == chatKind) return AgentCharacter(agent: agent, size: 14);
  if (leaf.kind == filesKind) return FolderGlyph(color: color);
  if (leaf.kind == appKind) return _AppGlyph(appId: leaf.appId, color: color);
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
    AppPage.map => ExtensionGlyph(
      icon: 'network',
      name: 'Map',
      color: color,
      size: 14,
    ),
    AppPage.usage => ExtensionGlyph(
      icon: 'chart-column',
      name: 'Usage',
      color: color,
      size: 14,
    ),
    AppPage.inbox => ExtensionGlyph(
      icon: 'inbox',
      name: 'Needs you',
      color: color,
      size: 14,
    ),
    AppPage.monitors => ExtensionGlyph(
      icon: 'alarm-clock',
      name: 'Monitors',
      color: color,
      size: 14,
    ),
    _ => viewerIcon(leaf, color),
  };
}

/// A pane's 32px header (at least 44 on touch): glyph, title, details, the
/// viewer's controls and its ×. It's the pane's handle: dragging it moves
/// the pane; on touch after a hold, so a swipe across it still scrolls.
class PaneHeader extends StatelessWidget {
  const PaneHeader({
    super.key,
    required this.drag,
    required this.glyph,
    required this.title,
    this.details = '',
    this.controls = const [],
    this.pinned = const [],
    this.onClose,
    this.focused = false,
    this.expanded = false,
    this.onExpand,
  });

  final PaneDrag drag;
  final Widget Function(Color color) glyph;
  final String title;
  final String details;
  final List<Widget> controls;

  /// After [controls], each as wide as Expand: never clipped as they are
  /// on a narrow pane; they give way after Expand, before the title.
  final List<Widget> pinned;
  final VoidCallback? onClose;

  /// The pane being read: on touch its header takes `selected`, so moving
  /// between panes (⌃Tab) visibly goes somewhere.
  final bool focused;

  /// The pane fills the window: its header isn't a handle to move it by,
  /// and [onExpand] (Collapse) puts it back.
  final bool expanded;

  /// Expand (or Collapse) before the ×, in the same place on every pane;
  /// null for none.
  final VoidCallback? onExpand;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final touch = touchFirst(context);
    final height = paneHeaderHeightOf(context);
    final titleStyle = TextStyles.label.copyWith(color: palette.text);
    // Expand's or ×'s width with its gap: a finger's target on touch.
    final double button = (touch ? Metrics.of(context).target : 28) + 2;
    // The title's room, which the viewer's controls never take: as wide as
    // it is, up to the start of a name, so a narrow pane keeps saying what
    // it shows (Abbacchio's rows 521 and 545).
    final titleRoom = () {
      final painter = TextPainter(
        text: TextSpan(text: title, style: titleStyle),
        textDirection: Directionality.of(context),
        textScaler: MediaQuery.textScalerOf(context),
        maxLines: 1,
      )..layout();
      final width = painter.width;
      painter.dispose();
      return math.min(width, 96.0);
    }();
    Widget named({required bool withDetails}) => Row(
      children: [
        SizedBox(width: 14, child: Center(child: glyph(palette.textMuted))),
        const SizedBox(width: Space.s),
        // The title first: the details beside it give way before it does.
        Expanded(
          child: LayoutBuilder(
            builder: (context, constraints) => Row(
              children: [
                ConstrainedBox(
                  constraints: BoxConstraints(maxWidth: constraints.maxWidth),
                  child: Text(
                    title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: titleStyle,
                  ),
                ),
                if (withDetails && details.isNotEmpty)
                  Flexible(
                    child: Padding(
                      padding: const EdgeInsets.only(left: Space.s),
                      child: Text(
                        details,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyles.caption.copyWith(
                          color: palette.textFaint,
                        ),
                      ),
                    ),
                  ),
              ],
            ),
          ),
        ),
      ],
    );
    final chip = DefaultTextStyle(
      style: titleStyle.copyWith(decoration: TextDecoration.none),
      child: Opacity(
        opacity: 0.8,
        child: Container(
          width: 240,
          height: height,
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
    final feedback = Transform.translate(
      offset: Offset(-Space.l, -height / 2),
      child: chip,
    );
    final bar = MouseRegion(
      cursor: SystemMouseCursors.grab,
      child: Container(
        height: touch ? null : height,
        constraints: touch ? BoxConstraints(minHeight: height) : null,
        padding: const EdgeInsets.only(left: Space.m, right: Space.xs),
        decoration: BoxDecoration(
          color: touch && focused ? palette.selected : palette.sidebar,
          border: Border(bottom: BorderSide(color: palette.hairline)),
        ),
        child: LayoutBuilder(
          builder: (context, constraints) {
            // Squeezed narrower than the glyph, a little of the title and
            // both buttons (a pane beside the conversation as the window
            // narrowed, Sale's row 26): Expand goes first, then the name,
            // and × stays whole.
            const least = 14 + Space.s + 24;
            final double close = onClose == null ? 0 : button;
            final pins = pinned.length * button;
            final withExpand =
                onExpand != null &&
                constraints.maxWidth >= least + close + button + pins;
            final expand = withExpand ? button : 0;
            final withPinned =
                pinned.isNotEmpty &&
                constraints.maxWidth >= least + close + expand + pins;
            final withName =
                constraints.maxWidth >=
                least + close + expand + (withPinned ? pins : 0);
            return _headerRow(
              constraints,
              named: withName ? named(withDetails: true) : null,
              withExpand: withExpand,
              withPinned: withPinned,
              button: button,
              titleRoom: titleRoom,
            );
          },
        ),
      ),
    );
    // Filling the window, there's nowhere to move it.
    if (expanded) return bar;
    final hold = PaneProjection.maybeOf(context)?.hold;
    if (hold != null) {
      return ValueListenableBuilder<bool>(
        valueListenable: hold,
        builder: (context, held, _) => _draggable(touch, feedback, bar, held),
      );
    }
    return _draggable(touch, feedback, bar, false);
  }

  /// The header's row in [constraints]: [named] (the glyph and title; null
  /// leaves their room empty), the viewer's controls, Expand when
  /// [withExpand], and ×.
  Widget _headerRow(
    BoxConstraints constraints, {
    required Widget? named,
    required bool withExpand,
    required bool withPinned,
    required double button,
    required double titleRoom,
  }) => Row(
    children: [
      Expanded(child: named ?? const SizedBox.shrink()),
      if (controls.isNotEmpty && named != null)
        // A narrow pane clips the viewer's controls at their end
        // (the first, the most used, stay) rather than overflowing:
        // the glyph (and the gaps beside the title), the start of
        // the title, Expand and × show while they fit.
        ConstrainedBox(
          constraints: BoxConstraints(
            maxWidth: math.max(
              0,
              constraints.maxWidth -
                  (withExpand ? button : 0) -
                  (withPinned ? pinned.length * button : 0) -
                  (onClose == null ? 0 : button) -
                  14 -
                  2 * Space.s -
                  titleRoom,
            ),
          ),
          child: UnconstrainedBox(
            constrainedAxis: Axis.vertical,
            alignment: Alignment.centerLeft,
            clipBehavior: Clip.hardEdge,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (final control in controls)
                  Padding(
                    padding: const EdgeInsets.only(left: 2),
                    child: control,
                  ),
              ],
            ),
          ),
        ),
      if (withPinned)
        for (final pin in pinned)
          Padding(padding: const EdgeInsets.only(left: 2), child: pin),
      if (withExpand)
        Padding(
          padding: const EdgeInsets.only(left: 2),
          child: GlyphButton(
            label: '${expanded ? 'Collapse' : 'Expand'} $title',
            active: false,
            onTap: onExpand!,
            glyph: (color) => ExtensionGlyph(
              icon: expanded ? 'minimize-2' : 'maximize-2',
              name: expanded ? 'Collapse' : 'Expand',
              color: color,
            ),
          ),
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
  );

  /// [bar] as the pane's handle; [held], it starts no drag.
  Widget _draggable(bool touch, Widget feedback, Widget bar, bool held) {
    if (!touch) {
      return Draggable<PaneDrag>(
        maxSimultaneousDrags: held ? 0 : null,
        data: drag,
        dragAnchorStrategy: pointerDragAnchorStrategy,
        feedback: feedback,
        child: bar,
      );
    }
    return LongPressDraggable<PaneDrag>(
      maxSimultaneousDrags: held ? 0 : null,
      data: drag,
      dragAnchorStrategy: pointerDragAnchorStrategy,
      feedback: feedback,
      // The lift, felt: the iPhone's selection tap, Android's own elsewhere.
      hapticFeedbackOnStart: defaultTargetPlatform != TargetPlatform.iOS,
      onDragStarted: () => fireAndLog(Haptics.selection()),
      child: bar,
    );
  }
}

/// [child], a divider, taking no new drag while [hold] is true; it stays
/// drawn as it is (a stroke flips it each time).
class _Held extends StatelessWidget {
  const _Held({required this.hold, required this.child});

  final ValueListenable<bool>? hold;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final hold = this.hold;
    if (hold == null) return child;
    return ValueListenableBuilder<bool>(
      valueListenable: hold,
      // Pointer events of a drag already under way still reach it: they
      // go where the pointer went down.
      builder: (context, held, child) =>
          IgnorePointer(ignoring: held, child: child),
      child: child,
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
