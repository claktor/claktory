import 'dart:math' as math;

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart' show Pressable;
import '../highlights.dart' show HighlightGlow, HighlightStore;
import '../extensions.dart' show extensionBarWidthOf;
import '../identity.dart';
import '../layout.dart';
import '../mobile/device.dart' show WindowControls, windowControls;
import '../mobile/metrics.dart';
import '../mobile/permission.dart' show registerPermissionHost;
import '../mobile/popover.dart' show presentPopover;
import '../needs_you_focus.dart' show focusModeKey, focusModeOf, setFocusMode;
import '../panes.dart' show PaneProjection, paneTitle, paneGlyph;
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart' show AppScope;
import '../selection.dart' show Unselectable;
import 'columns.dart';
import 'columns.dart' as columns show project;
import 'commands.dart';
import 'document_host.dart' show DocumentSurfaces;

/// The iPad's window when it isn't compact (docs/ipad-workbench.md): the
/// Mac's parts (the Claktories rail, the projects sidebar, the extension
/// bar, the selected agent's view, its panel) composed for the window's
/// [WorkbenchTier], at a finger's sizes.
///
/// - full: the rail, the sidebar pinned, the content, the panel when it
///   fits. ⌃⌘S hides the sidebar for this session.
/// - paired: the rail, the content; the sidebar opens over them.
/// - focused: a toolbar (Sidebar, a Conversation/document switch, Panes),
///   the content showing one pane; the rail and sidebar open over it.
///
/// Every tier reaches every pane: the ones the window has no room for are
/// listed in Panes, and choosing one focuses it. Nothing here changes the
/// agent's layout; that's the person's Split, Open, Close and drags.

/// What the person is reading, per agent, and the sidebar's state, for
/// this session only: kept in memory like the page shown, never on the
/// device or sent anywhere (ipad-workbench.md §3, Focus).
class WorkbenchFocus extends ChangeNotifier {
  WorkbenchFocus({CommandMenu? menu}) : menu = menu ?? CommandMenu() {
    this.menu.addListener(notifyListeners);
  }

  /// The system's menu for the window's commands.
  final CommandMenu menu;

  /// The documents drawn on in this window, one per agent's document leaf
  /// (document_host.dart): kept while the leaf is in the agent's layout.
  final documents = DocumentSurfaces();

  final _focused = <String, String>{};
  final _documents = <String, String>{};
  final _projections = <String, Projection>{};

  /// Whether the person hid the pinned sidebar (full tier).
  bool get sidebarHidden => _sidebarHidden;
  var _sidebarHidden = false;

  /// Whether the sidebar is open over the window (paired and focused).
  bool get overlayOpen => _overlayOpen;
  var _overlayOpen = false;

  /// Settings while it's shown (settings_keys.dart's WorkbenchSettings):
  /// what the menu's Search Settings invokes its intent on.
  BuildContext? get settings => _settings.lastOrNull;
  final _settings = <BuildContext>[];

  /// [settings] came into the window, while it builds: the menu hears
  /// after the frame.
  void showSettings(BuildContext settings) {
    _settings.add(settings);
    _notifyAfterFrame();
  }

  /// [settings] went.
  void hideSettings(BuildContext settings) {
    if (_settings.remove(settings) && !_disposed) _notifyAfterFrame();
  }

  /// The leaf [agentId]'s layout [root] is read at ([focusIn]).
  String focusedIn(String agentId, ViewNode root) =>
      focusIn(root, _focused[agentId]);

  /// The document the Conversation/document switch goes to in [root]: the
  /// focused leaf when it isn't the conversation, else the one last
  /// focused, else the first; null when the layout is just the
  /// conversation.
  ViewNode? documentIn(String agentId, ViewNode root) {
    final others = [
      for (final leaf in leaves(root))
        if (leaf.leaf.kind != chatKind) leaf,
    ];
    if (others.isEmpty) return null;
    final focused = focusedIn(agentId, root);
    for (final id in [focused, _documents[agentId]]) {
      for (final leaf in others) {
        if (leaf.id == id) return leaf;
      }
    }
    return others.first;
  }

  void focus(String agentId, String leafId) {
    if (_focused[agentId] == leafId) return;
    _focused[agentId] = leafId;
    notifyListeners();
  }

  /// Remembers [leaf] as [agentId]'s document when it is one, so the
  /// switch returns to it from the conversation.
  void _noteDocument(String agentId, ViewNode leaf) {
    if (leaf.leaf.kind != chatKind) _documents[agentId] = leaf.id;
  }

  /// The last projection of [agentId]'s layout laid out, for Panes.
  Projection? projectionOf(String agentId) => _projections[agentId];

  /// [agentId]'s layout presented in [room] at [tier], as its panes lay it
  /// out (PaneLayout calls this through its PaneProjection on each build).
  /// Recorded for Panes, whose listeners hear of a change after the frame.
  Projection project(
    String agentId,
    ViewNode full, {
    required WorkbenchTier tier,
    required Size room,
    double textFactor = 1,
  }) {
    documents.prune(agentId, full);
    final projection = columns.project(
      full,
      tier: tier,
      room: room,
      focused: _focused[agentId],
      textFactor: textFactor,
    );
    final before = _projections[agentId];
    _projections[agentId] = projection;
    final changed =
        before == null ||
        before.focused != projection.focused ||
        !_same(before.hidden, projection.hidden);
    if (changed) _notifyAfterFrame();
    return projection;
  }

  var _notifyScheduled = false;
  var _disposed = false;

  void toggleSidebar(WorkbenchTier tier) {
    if (tier == WorkbenchTier.full) {
      _sidebarHidden = !_sidebarHidden;
      _overlayOpen = false;
    } else {
      _overlayOpen = !_overlayOpen;
    }
    notifyListeners();
  }

  void closeOverlay() {
    if (!_overlayOpen) return;
    _overlayOpen = false;
    notifyListeners();
  }

  /// [closeOverlay] while the window is building (another agent chosen):
  /// what's building reads it now, the rest hear after the frame.
  void _closeOverlayWhileBuilding() {
    if (!_overlayOpen) return;
    _overlayOpen = false;
    _notifyAfterFrame();
  }

  void _notifyAfterFrame() {
    if (_notifyScheduled) return;
    _notifyScheduled = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _notifyScheduled = false;
      if (!_disposed) notifyListeners();
    });
    // Called outside a frame, one must come for the callback to run.
    WidgetsBinding.instance.ensureVisualUpdate();
  }

  @override
  void dispose() {
    _disposed = true;
    menu.removeListener(notifyListeners);
    menu.dispose();
    documents.dispose();
    super.dispose();
  }
}

bool _same(List<String> a, List<String> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}

/// Gives the window's [WorkbenchFocus] to what's below it. It sits above
/// the window's choice between the phone's routes and the workbench, so
/// the focus survives a window narrowing to compact and widening again.
class WorkbenchScope extends InheritedNotifier<WorkbenchFocus> {
  const WorkbenchScope({
    super.key,
    required WorkbenchFocus focus,
    required super.child,
  }) : super(notifier: focus);

  static WorkbenchFocus of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<WorkbenchScope>()!.notifier!;

  static WorkbenchFocus? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<WorkbenchScope>()?.notifier;
}

/// The tier of the window [context] is in ([window], else the window's
/// size): its size less the safe area, never less the keyboard, and the
/// person's text size. The same safe size Shell chooses the workbench by
/// (isPhoneLayout), so the two never disagree: not the workbench's own
/// constraints, which banners and the keyboard's arrival change.
WorkbenchTier windowTier(BuildContext context, [Size? window]) {
  window ??= MediaQuery.sizeOf(context);
  final padding = MediaQuery.viewPaddingOf(context);
  return tierFor(
    idiom: Idiom.pad,
    window: Size(
      window.width - padding.horizontal,
      window.height - padding.vertical,
    ),
    textFactor: textFactorOf(MediaQuery.textScalerOf(context).scale),
  );
}

/// The window's room for the workbench: clear of the status bar along
/// the top. The window reaches the bottom edge, so the conversation's own
/// color runs under the home indicator: what's inside keeps clear of the
/// indicator itself by the bottom padding it's left with (the message box
/// does). The window's controls aren't kept
/// clear here: the native side adds them to the view's top padding
/// ([controls].top, for screens with no toolbar), and the workbench takes
/// that back, starting at the status bar as Apple's apps do, while its
/// toolbar's leading items move past them ([_Toolbar]) and the rail's
/// column starts below them ([_RailColumn]). The on-screen keyboard
/// covers the rail and the sidebar, as it does a UIKit sidebar; the
/// content and the panel rise above it ([_aboveKeyboard]). What's inside
/// sees no keyboard left to avoid, and only the home indicator's padding.
class SafeWindow extends StatelessWidget {
  const SafeWindow({super.key, required this.controls, required this.child});

  final WindowControls controls;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final padding = MediaQuery.paddingOf(context);
    return Padding(
      padding: EdgeInsets.fromLTRB(
        padding.left,
        math.max(0.0, padding.top - controls.top),
        padding.right,
        0,
      ),
      child: WindowKeyboard(
        height: MediaQuery.viewInsetsOf(context).bottom,
        child: MediaQuery(
          data: MediaQuery.of(context)
              .removePadding(
                removeLeft: true,
                removeTop: true,
                removeRight: true,
              )
              .removeViewInsets(removeBottom: true),
          child: child,
        ),
      ),
    );
  }
}

/// The on-screen keyboard's height over the window, kept for what's laid
/// over the whole window from inside it (an expanded pane, in the
/// window's overlay), which sees no keyboard in its MediaQuery. The iPad's
/// workbench and the wide shell's row (an Android tablet's) both give it.
class WindowKeyboard extends InheritedWidget {
  const WindowKeyboard({super.key, required this.height, required super.child});

  final double height;

  @override
  bool updateShouldNotify(WindowKeyboard old) => old.height != height;
}

/// How much of the window's bottom the on-screen keyboard covers, for
/// what fills the whole window (ExpandedPane): inside the iPad's
/// workbench, the keyboard it took out of the MediaQuery; elsewhere, the
/// MediaQuery's own.
double keyboardOverWindow(BuildContext context) =>
    context.dependOnInheritedWidgetOfExactType<WindowKeyboard>()?.height ??
    MediaQuery.viewInsetsOf(context).bottom;

/// What the on-screen keyboard leaves of the content's and the panel's
/// room: they end above it, so the message box being written in stays in
/// sight, while the rail and the sidebar beside them stay put under it (a
/// short window's columns would overflow if they rose too). [context] is
/// outside [SafeWindow], where the keyboard is still known.
Widget _aboveKeyboard(BuildContext context, Widget child) {
  final lift = MediaQuery.viewInsetsOf(context).bottom;
  return Padding(
    padding: EdgeInsets.only(bottom: lift),
    child: child,
  );
}

/// The width of the rail's column: the rail with room for a finger's
/// buttons above it.
const workbenchRailWidth = WorkbenchGeometry.rail;

/// Above this the panel sits beside the content; narrower, it opens over
/// it, so the paper isn't squeezed into a sliver (ipad.md §1).
const _contentMinWidth = 440.0;

class Workbench extends StatefulWidget {
  const Workbench({
    super.key,
    required this.rail,
    required this.sidebar,
    required this.sidebarWidth,
    required this.content,
    this.extensionBar,
    this.panel,
    this.panelWidth = 0,
    this.agent,
  });

  /// The Claktories rail (ClaktoryRail).
  final Widget rail;

  /// The projects sidebar at a width (Sidebar).
  final Widget Function(double width) sidebar;

  /// The person's sidebar width (`Settings.sidebar_width`, kept).
  final double sidebarWidth;

  /// The selected agent's view, or whatever shows instead, with its
  /// banners.
  final Widget content;

  /// The agent's extension bar, when it has one.
  final Widget? extensionBar;

  /// The open panel, [panelWidth] wide, when there is one.
  final Widget? panel;
  final double panelWidth;

  /// The selected agent, whose panes the switch and Panes go through.
  final Agent? agent;

  @override
  State<Workbench> createState() => _WorkbenchState();
}

class _WorkbenchState extends State<Workbench> {
  /// The Panes button shown now, if any: what Panes points at.
  final _panesAnchor = GlobalKey(debugLabel: 'Panes');

  /// The window's controls, as the native side says (mobile/device.dart):
  /// the toolbar's leading items clear them, and so does the rail's column.
  WindowControls get _controls => windowControls.value;

  /// Asks from outside a screen (notifications after pairing) explain
  /// themselves over the workbench (area 5a P3, Zucchero 44139).
  late final VoidCallback _unhostPermissions;

  @override
  void initState() {
    super.initState();
    windowControls.addListener(_controlsChanged);
    _unhostPermissions = registerPermissionHost(context);
  }

  @override
  void dispose() {
    windowControls.removeListener(_controlsChanged);
    _focusMode?.removeListener(_focusModeChanged);
    _highlights?.removeListener(_glowed);
    _highlights?.pointedAt.removeListener(_pointedAt);
    _unhostPermissions();
    super.dispose();
  }

  void _controlsChanged() => setState(() {});
  void _focusModeChanged() => setState(() {});

  /// What glows, and what the backend points at: a pane of the agent's
  /// shown again (an agent's show of a file already open, its show card
  /// tapped) comes to the front, as a new one does ([didUpdateWidget]).
  HighlightStore? _highlights;
  var _lastGlow = 0;

  /// Whether Needs you's focus mode is over the window (#514): leaving it,
  /// the workbench's commands are the menu's again.
  ValueListenable<bool>? _focusMode;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final focusMode = focusModeOf(AppScope.of(context));
    if (!identical(focusMode, _focusMode)) {
      _focusMode?.removeListener(_focusModeChanged);
      _focusMode = focusMode..addListener(_focusModeChanged);
    }
    final highlights = context
        .getInheritedWidgetOfExactType<AppScope>()
        ?.backend
        .highlights;
    if (highlights == _highlights) return;
    _highlights?.removeListener(_glowed);
    _highlights?.pointedAt.removeListener(_pointedAt);
    _highlights = highlights
      ?..addListener(_glowed)
      ..pointedAt.addListener(_pointedAt);
    _lastGlow = highlights?.glows.lastOrNull?.id ?? 0;
  }

  void _glowed() {
    final glows = _highlights?.glows ?? const <HighlightGlow>[];
    final fresh = [
      for (final glow in glows)
        if (glow.id > _lastGlow) glow.key,
    ];
    if (glows.isNotEmpty) _lastGlow = math.max(_lastGlow, glows.last.id);
    fresh.forEach(_toFront);
  }

  void _pointedAt() => _highlights?.pointedAt.value?.keys.forEach(_toFront);

  /// Brings `pane:<agent>:<pane>` to the front when it's a pane of the
  /// agent shown here; not while a stroke is down on a document.
  void _toFront(String key) {
    final agent = widget.agent;
    if (!mounted || agent == null) return;
    final prefix = 'pane:${agent.id}:';
    if (!key.startsWith(prefix)) return;
    final pane = find(layoutOf(agent), key.substring(prefix.length));
    if (pane == null) return;
    final focus = context
        .getInheritedWidgetOfExactType<WorkbenchScope>()
        ?.notifier;
    if (focus == null || focus.documents.drawing.value) return;
    focus._noteDocument(agent.id, pane);
    focus.focus(agent.id, pane.id);
  }

  /// Panes, pointing at its button, or in the window's middle when none
  /// shows (⌥⌘P with every pane laid out).
  void _choose(BuildContext context, Agent agent) =>
      _choosePane(_panesAnchor.currentContext ?? context, agent);

  @override
  void didUpdateWidget(Workbench old) {
    super.didUpdateWidget(old);
    // Choosing an agent in the sidebar over the window closes it.
    if (old.agent?.id != widget.agent?.id) {
      WorkbenchScope.of(context)._closeOverlayWhileBuilding();
      return;
    }
    // A pane that comes into the layout (an agent's show, Needs you opened
    // from the sidebar over the window) or takes another's place comes
    // into sight, even where the window shows fewer panes than the layout
    // has; the sidebar over the window goes, as for an agent chosen there.
    // Not while a stroke is down on a document: it stays in front.
    final agent = widget.agent;
    final focus = WorkbenchScope.of(context);
    if (agent == null || old.agent == null) return;
    final before = {
      for (final leaf in leaves(layoutOf(old.agent!))) leaf.id: leaf.leaf,
    };
    final added = [
      for (final leaf in leaves(layoutOf(agent)))
        if (before[leaf.id] != leaf.leaf) leaf,
    ];
    if (added.isEmpty) return;
    if (focus.overlayOpen) focus._closeOverlayWhileBuilding();
    if (focus.documents.drawing.value) return;
    focus._focused[agent.id] = added.last.id;
    focus._noteDocument(agent.id, added.last);
  }

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final focus = WorkbenchScope.of(context);
      // A compact window takes the phone's routes (Shell); one that
      // reaches here anyway (a test's host) lays out as one pane.
      final tier = switch (windowTier(context)) {
        WorkbenchTier.compact => WorkbenchTier.focused,
        final tier => tier,
      };
      final pinned = tier == WorkbenchTier.full && !focus.sidebarHidden;
      final agent = widget.agent;
      void showPanes() {
        if (agent != null) _choose(context, agent);
      }

      final commands = workbenchCommands(
        tier: tier,
        focus: focus,
        agent: agent,
        showPanes: showPanes,
      );
      final textFactor = textFactorOf(MediaQuery.textScalerOf(context).scale);
      // The agent's panes show what fits this window, focused as the person
      // left them; touching a pane makes it the one being read.
      final content = agent == null
          ? widget.content
          : PaneProjection(
              project: (full, room) => ProjectedPanes(
                focus.project(
                  agent.id,
                  full,
                  tier: tier,
                  room: room,
                  textFactor: textFactor,
                ),
                textFactor: textFactor,
              ),
              focus: (leaf) => focus.focus(agent.id, leaf),
              // No divider drag starts while a stroke is down on a
              // document: a width change would drop it.
              hold: focus.documents.drawing,
              child: widget.content,
            );
      final body = switch (tier) {
        WorkbenchTier.focused => _focused(context, focus, content),
        _ => _wide(context, focus, constraints, content, pinned: pinned),
      };
      // The system's menu lists them as they are now, with Needs you's
      // focus mode to enter (#514). While it's on, the workbench waits
      // under it, out of sight, and focus mode's commands are the menu's
      // (ipad/focus.dart): this one publishes nothing.
      final backend = AppScope.of(context);
      final focusMode = focusModeOf(backend);
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted || focusMode.value) return;
        focus.menu.publish([
          ...commands,
          WorkbenchCommand(
            id: 'focus.toggle',
            title: 'Focus on Needs You',
            keys: focusModeKey,
            run: () => setFocusMode(backend, true),
          ),
        ]);
      });
      return WorkbenchShortcuts(
        commands: commands,
        native: focus.menu.native,
        escape: focus.overlayOpen && !pinned ? focus.closeOverlay : null,
        child: SafeWindow(
          controls: _controls,
          child: Stack(
            children: [
              Positioned.fill(child: body),
              if (focus.overlayOpen && !pinned)
                ..._sidebarOverlay(
                  context,
                  focus,
                  constraints,
                  withRail: tier == WorkbenchTier.focused,
                ),
            ],
          ),
        ),
      );
    },
  );

  /// The full and paired tiers: the rail's column, the sidebar when it's
  /// pinned, the extension bar, the content and the panel when it fits.
  Widget _wide(
    BuildContext context,
    WorkbenchFocus focus,
    BoxConstraints constraints,
    Widget content, {
    required bool pinned,
  }) {
    final palette = Palette.of(context);
    final tier = switch (windowTier(context)) {
      WorkbenchTier.compact => WorkbenchTier.focused,
      final tier => tier,
    };
    final sidebar = pinned ? _sidebarWidth(constraints.maxWidth) : 0.0;
    final room =
        constraints.maxWidth - workbenchRailWidth - sidebar - _bar(context);
    final panel = widget.panel;
    final panelFits =
        panel != null && room - widget.panelWidth >= _contentMinWidth;
    final hidden = _hiddenCount(focus);
    return Stack(
      children: [
        Positioned.fill(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _RailColumn(
                windowControls: _controls,
                sidebarShown: pinned || focus.overlayOpen,
                onSidebar: () => focus.toggleSidebar(tier),
                hidden: hidden,
                onPanes: widget.agent == null
                    ? null
                    : () => _choose(context, widget.agent!),
                panesAnchor: _panesAnchor,
                rail: widget.rail,
              ),
              Container(width: 1, color: palette.hairline),
              if (pinned) ...[
                SizedBox(width: sidebar, child: widget.sidebar(sidebar)),
                Container(width: 1, color: palette.hairline),
              ],
              ?_extensionBar,
              Expanded(child: _aboveKeyboard(context, content)),
              if (panelFits)
                SizedBox(
                  width: widget.panelWidth,
                  child: _aboveKeyboard(context, panel),
                ),
            ],
          ),
        ),
        if (panel != null && !panelFits)
          _PanelOver(
            width: widget.panelWidth,
            child: _aboveKeyboard(context, panel),
          ),
      ],
    );
  }

  /// The focused tier: the toolbar over the content, which shows one pane.
  Widget _focused(BuildContext context, WorkbenchFocus focus, Widget content) {
    final palette = Palette.of(context);
    final agent = widget.agent;
    final panel = widget.panel;
    return Stack(
      children: [
        Positioned.fill(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _Toolbar(
                windowControls: _controls,
                agent: agent,
                focus: focus,
                hidden: _hiddenCount(focus),
                onPanes: agent == null ? null : () => _choose(context, agent),
                panesAnchor: _panesAnchor,
              ),
              Container(height: 1, color: palette.hairline),
              Expanded(
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    ?_extensionBar,
                    Expanded(child: _aboveKeyboard(context, content)),
                  ],
                ),
              ),
            ],
          ),
        ),
        if (panel != null)
          _PanelOver(
            width: widget.panelWidth,
            child: _aboveKeyboard(context, panel),
          ),
      ],
    );
  }

  /// The extension bar, scrolling when the window is too short for all
  /// its glyphs (a short Stage Manager window).
  Widget? get _extensionBar {
    final bar = widget.extensionBar;
    return bar == null ? null : WorkbenchBarScroll(child: bar);
  }

  /// The extension bar's width, when there is one.
  double _bar(BuildContext context) =>
      widget.extensionBar == null ? 0 : extensionBarWidthOf(context);

  double _sidebarWidth(double window) =>
      math.min(widget.sidebarWidth, math.max(200.0, window * 0.4));

  int _hiddenCount(WorkbenchFocus focus) {
    final agent = widget.agent;
    if (agent == null) return 0;
    return focus.projectionOf(agent.id)?.hidden.length ?? 0;
  }

  /// The sidebar over the window, with the rail beside it in the focused
  /// tier: a tap on the dimmed window, Esc or choosing an agent closes it.
  List<Widget> _sidebarOverlay(
    BuildContext context,
    WorkbenchFocus focus,
    BoxConstraints constraints, {
    required bool withRail,
  }) {
    final palette = Palette.of(context);
    final width = math.min(
      _sidebarWidth(constraints.maxWidth) +
          (withRail ? workbenchRailWidth + 1 : 0),
      constraints.maxWidth * 0.85,
    );
    final left = withRail ? 0.0 : workbenchRailWidth + 1;
    return [
      Positioned.fill(
        left: left,
        child: Semantics(
          button: true,
          label: 'Close sidebar',
          child: GestureDetector(
            key: const ValueKey('workbench-scrim'),
            behavior: HitTestBehavior.opaque,
            onTap: focus.closeOverlay,
            child: const ColoredBox(color: Color(0x33000000)),
          ),
        ),
      ),
      Positioned(
        key: const ValueKey('workbench-sidebar-over'),
        left: left,
        top: 0,
        bottom: 0,
        width: width,
        child: CallbackShortcuts(
          bindings: {
            const SingleActivator(LogicalKeyboardKey.escape):
                focus.closeOverlay,
          },
          child: _TakesFocus(
            child: DecoratedBox(
              decoration: BoxDecoration(
                color: palette.sidebar,
                boxShadow: floatingShadow,
              ),
              child: DenseList(
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    if (withRail) ...[
                      SizedBox(
                        width: workbenchRailWidth,
                        child: Padding(
                          padding: EdgeInsets.only(top: _controls.top),
                          child: Center(child: widget.rail),
                        ),
                      ),
                      Container(width: 1, color: palette.hairline),
                    ],
                    Expanded(
                      child: LayoutBuilder(
                        builder: (context, box) => widget.sidebar(box.maxWidth),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    ];
  }
}

/// Lists every pane of [agent]'s layout, the ones the window shows marked;
/// choosing one focuses it, which shows it.
Future<void> _choosePane(BuildContext context, Agent agent) {
  final focus = WorkbenchScope.of(context);
  return showPanes(context, agent: agent, focus: focus);
}

/// Panes: [agent]'s panes, in a popover pointing at [context]'s widget
/// (the Panes button), to choose one to read.
Future<void> showPanes(
  BuildContext context, {
  required Agent agent,
  required WorkbenchFocus focus,
}) => presentPopover<void>(
  context,
  // Takes the keyboard's focus from what holds it (the composer, say), so
  // Esc reaches the popover.
  builder: (context, close) => _TakesFocus(
    child: PanesList(
      agent: agent,
      focus: focus,
      onChosen: (leaf) {
        focus._noteDocument(agent.id, leaf);
        focus.focus(agent.id, leaf.id);
        close();
      },
    ),
  ),
);

/// Takes the keyboard's focus as it opens, so Esc and the arrows reach
/// what opened over the window rather than what's under it.
class _TakesFocus extends StatefulWidget {
  const _TakesFocus({required this.child});

  final Widget child;

  @override
  State<_TakesFocus> createState() => _TakesFocusState();
}

class _TakesFocusState extends State<_TakesFocus> {
  final _scope = FocusScopeNode(debugLabel: 'workbench overlay');

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _scope.requestFocus();
    });
  }

  @override
  void dispose() {
    _scope.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) =>
      FocusScope(node: _scope, child: widget.child);
}

/// The list Panes shows: each pane's glyph and title, and whether the
/// window shows it now.
class PanesList extends StatelessWidget {
  const PanesList({
    super.key,
    required this.agent,
    required this.focus,
    required this.onChosen,
  });

  final Agent agent;
  final WorkbenchFocus focus;
  final ValueChanged<ViewNode> onChosen;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final root = layoutOf(agent);
    final hidden = focus.projectionOf(agent.id)?.hidden.toSet() ?? const {};
    final focused = focus.focusedIn(agent.id, root);
    return Semantics(
      scopesRoute: true,
      namesRoute: true,
      explicitChildNodes: true,
      label: 'Panes',
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(
              Space.l,
              Space.s,
              Space.l,
              Space.s,
            ),
            child: Text(
              'Panes',
              style: type.emphasis.copyWith(color: palette.text),
            ),
          ),
          // A short window holds the popover to its room: the rows scroll.
          Flexible(
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  for (final leaf in leaves(root))
                    _PaneRow(
                      agent: agent,
                      leaf: leaf,
                      focused: leaf.id == focused,
                      shown: !hidden.contains(leaf.id),
                      onTap: () => onChosen(leaf),
                    ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _PaneRow extends StatelessWidget {
  const _PaneRow({
    required this.agent,
    required this.leaf,
    required this.focused,
    required this.shown,
    required this.onTap,
  });

  final Agent agent;
  final ViewNode leaf;
  final bool focused;
  final bool shown;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final title = paneTitle(leaf.leaf, _workspace(context));
    return Semantics(
      button: true,
      selected: focused,
      label: shown ? title : '$title, not shown',
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          constraints: const BoxConstraints(minHeight: Touch.target),
          padding: const EdgeInsets.symmetric(horizontal: Space.l),
          color: focused
              ? palette.selected
              : hovered
              ? palette.hover
              : const Color(0x00000000),
          child: Row(
            children: [
              paneGlyph(agent, leaf.leaf, palette.textMuted),
              const SizedBox(width: Space.m),
              Expanded(
                child: Padding(
                  padding: const EdgeInsets.symmetric(vertical: Space.s),
                  child: Text(
                    title,
                    style: type.label.copyWith(color: palette.text),
                  ),
                ),
              ),
              if (!shown)
                Text(
                  'Not shown',
                  style: type.caption.copyWith(color: palette.textMuted),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A column of glyphs (the extension bar) in a window too short for all
/// of them: it keeps every glyph at its size and scrolls, and the edge it's
/// cut at fades out, so what's past it reads as more to scroll to rather
/// than cut off. Tall enough, it fills the height as it would unwrapped.
@visibleForTesting
class WorkbenchBarScroll extends StatefulWidget {
  const WorkbenchBarScroll({super.key, required this.child});

  final Widget child;

  @override
  State<WorkbenchBarScroll> createState() => WorkbenchBarScrollState();
}

@visibleForTesting
class WorkbenchBarScrollState extends State<WorkbenchBarScroll> {
  /// Whether some of the column is past the top edge, and past the
  /// bottom: that edge fades out.
  var _above = false;
  var _below = false;

  bool get fadesTop => _above;
  bool get fadesBottom => _below;

  bool _follow(ScrollNotification notification) {
    if (notification.depth == 0) _place(notification.metrics);
    return false;
  }

  bool _measured(ScrollMetricsNotification notification) {
    if (notification.depth == 0) _place(notification.metrics);
    return false;
  }

  void _place(ScrollMetrics metrics) {
    final above = metrics.extentBefore > 0.5;
    final below = metrics.extentAfter > 0.5;
    if (above != _above || below != _below) {
      setState(() {
        _above = above;
        _below = below;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    const shown = Color(0xFFFFFFFF);
    const gone = Color(0x00FFFFFF);
    return LayoutBuilder(
      builder: (context, constraints) =>
          NotificationListener<ScrollMetricsNotification>(
            onNotification: _measured,
            child: NotificationListener<ScrollNotification>(
              onNotification: _follow,
              // Always masked, so the scroll view keeps its offset as the
              // fades come and go.
              child: ShaderMask(
                blendMode: BlendMode.dstIn,
                shaderCallback: (bounds) {
                  final fade = math.min(
                    0.5,
                    Space.xl / math.max(bounds.height, 1),
                  );
                  return LinearGradient(
                    begin: Alignment.topCenter,
                    end: Alignment.bottomCenter,
                    colors: [
                      _above ? gone : shown,
                      shown,
                      shown,
                      _below ? gone : shown,
                    ],
                    stops: [0, fade, 1 - fade, 1],
                  ).createShader(bounds);
                },
                child: SingleChildScrollView(
                  child: ConstrainedBox(
                    // At least the window's height, so its background and
                    // hairline run to the foot.
                    constraints: BoxConstraints(
                      minHeight: constraints.maxHeight,
                    ),
                    child: widget.child,
                  ),
                ),
              ),
            ),
          ),
    );
  }
}

/// The rail's column in the wide tiers: Sidebar and, when the window
/// can't show every pane, Panes, above the rail.
class _RailColumn extends StatelessWidget {
  const _RailColumn({
    required this.windowControls,
    required this.sidebarShown,
    required this.onSidebar,
    required this.hidden,
    required this.onPanes,
    required this.panesAnchor,
    required this.rail,
  });

  final WindowControls windowControls;
  final bool sidebarShown;
  final VoidCallback onSidebar;
  final int hidden;
  final VoidCallback? onPanes;
  final GlobalKey panesAnchor;
  final Widget rail;

  @override
  Widget build(BuildContext context) => SizedBox(
    width: workbenchRailWidth,
    child: Column(
      children: [
        SizedBox(height: windowControls.top),
        ToolButton(
          key: const ValueKey('workbench-sidebar'),
          label: sidebarShown ? 'Hide sidebar' : 'Show sidebar',
          active: sidebarShown,
          onTap: onSidebar,
          glyph: (color) => _Glyph(color: color, draw: _sidebarGlyph),
        ),
        if (hidden > 0 && onPanes != null)
          KeyedSubtree(
            key: panesAnchor,
            child: ToolButton(
              key: const ValueKey('workbench-panes'),
              label: 'Panes, $hidden not shown',
              active: false,
              onTap: onPanes!,
              glyph: (color) => _Glyph(color: color, draw: _panesGlyph),
              badge: hidden,
            ),
          ),
        Expanded(child: Center(child: rail)),
      ],
    ),
  );
}

/// The focused tier's toolbar: Sidebar at its start, the switch between
/// the conversation and the document in its middle, Panes at its end.
class _Toolbar extends StatelessWidget {
  const _Toolbar({
    required this.windowControls,
    required this.agent,
    required this.focus,
    required this.hidden,
    required this.onPanes,
    required this.panesAnchor,
  });

  final WindowControls windowControls;
  final Agent? agent;
  final WorkbenchFocus focus;
  final int hidden;
  final VoidCallback? onPanes;
  final GlobalKey panesAnchor;

  @override
  Widget build(BuildContext context) {
    final agent = this.agent;
    final root = agent == null ? null : layoutOf(agent);
    final document = agent == null ? null : focus.documentIn(agent.id, root!);
    return ConstrainedBox(
      constraints: const BoxConstraints(minHeight: Touch.target + Space.s),
      child: Padding(
        padding: EdgeInsets.only(
          left: Space.xs + windowControls.leading,
          right: Space.xs,
        ),
        child: Row(
          children: [
            ToolButton(
              key: const ValueKey('workbench-sidebar'),
              label: focus.overlayOpen ? 'Hide sidebar' : 'Show sidebar',
              active: focus.overlayOpen,
              onTap: () => focus.toggleSidebar(WorkbenchTier.focused),
              glyph: (color) => _Glyph(color: color, draw: _sidebarGlyph),
            ),
            Expanded(
              child: Center(
                child: agent == null || document == null
                    ? const SizedBox.shrink()
                    : PaneSwitch(
                        agent: agent,
                        document: document,
                        documentShown:
                            focus.focusedIn(agent.id, root!) == document.id,
                        onConversation: () => focus.focus(
                          agent.id,
                          leaves(root)
                              .firstWhere((leaf) => leaf.leaf.kind == chatKind)
                              .id,
                        ),
                        onDocument: () {
                          focus._noteDocument(agent.id, document);
                          focus.focus(agent.id, document.id);
                        },
                      ),
              ),
            ),
            if (onPanes != null && agent != null && leaves(root!).length > 1)
              KeyedSubtree(
                key: panesAnchor,
                child: ToolButton(
                  key: const ValueKey('workbench-panes'),
                  label: 'Panes',
                  active: false,
                  onTap: onPanes!,
                  glyph: (color) => _Glyph(color: color, draw: _panesGlyph),
                ),
              )
            else
              const SizedBox(width: Touch.target),
          ],
        ),
      ),
    );
  }
}

/// Two segments: the conversation, and the document beside it (named by
/// its title). Tapping one shows it.
class PaneSwitch extends StatelessWidget {
  const PaneSwitch({
    super.key,
    required this.agent,
    required this.document,
    required this.documentShown,
    required this.onConversation,
    required this.onDocument,
  });

  final Agent agent;
  final ViewNode document;
  final bool documentShown;
  final VoidCallback onConversation;
  final VoidCallback onDocument;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    Widget segment(String label, bool selected, VoidCallback onTap, Key key) =>
        Flexible(
          child: Semantics(
            button: true,
            selected: selected,
            label: label,
            excludeSemantics: true,
            child: Pressable(
              key: key,
              onTap: onTap,
              builder: (context, hovered) => Container(
                constraints: const BoxConstraints(
                  minHeight: Touch.target - Space.s,
                  minWidth: 88,
                ),
                alignment: Alignment.center,
                padding: const EdgeInsets.symmetric(horizontal: Space.m),
                decoration: BoxDecoration(
                  color: selected
                      ? palette.canvas
                      : hovered
                      ? palette.hover
                      : const Color(0x00000000),
                  borderRadius: BorderRadius.circular(Radii.control),
                ),
                child: Text(
                  label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TypeScale.of(context).label.copyWith(
                    color: selected ? palette.text : palette.textMuted,
                  ),
                ),
              ),
            ),
          ),
        );
    return Unselectable(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 420),
        child: Container(
          padding: const EdgeInsets.all(2),
          decoration: BoxDecoration(
            color: palette.selected,
            borderRadius: BorderRadius.circular(Radii.control + 2),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              segment(
                'Conversation',
                !documentShown,
                onConversation,
                const ValueKey('workbench-switch-conversation'),
              ),
              segment(
                paneTitle(document.leaf, _workspace(context)),
                documentShown,
                onDocument,
                const ValueKey('workbench-switch-document'),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A finger's 44pt button around a 16pt glyph, with a count beside it when
/// [badge] is more than 0.
/// A 44pt control with a 32pt glyph plate: selected when [active], dimmed
/// and inert when [onTap] is null.
class ToolButton extends StatelessWidget {
  const ToolButton({
    super.key,
    required this.label,
    required this.active,
    required this.onTap,
    required this.glyph,
    this.badge = 0,
  });

  final String label;
  final bool active;
  final VoidCallback? onTap;
  final Widget Function(Color color) glyph;
  final int badge;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Unselectable(
      child: Semantics(
        container: true,
        button: true,
        enabled: onTap != null,
        selected: active,
        label: label,
        excludeSemantics: true,
        child: Pressable(
          onTap: onTap,
          builder: (context, hovered) => SizedBox.square(
            dimension: Touch.target,
            child: Center(
              child: AnimatedContainer(
                duration: Motion.fast,
                width: 32,
                height: 32,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: active
                      ? palette.selected
                      : hovered
                      ? palette.hover
                      : const Color(0x00000000),
                  borderRadius: BorderRadius.circular(Radii.control),
                ),
                child: Stack(
                  clipBehavior: Clip.none,
                  alignment: Alignment.center,
                  children: [
                    glyph(
                      onTap == null
                          ? palette.textFaint
                          : active || hovered
                          ? palette.text
                          : palette.textMuted,
                    ),
                    if (badge > 0)
                      Positioned(
                        right: -8,
                        top: -6,
                        child: Container(
                          padding: const EdgeInsets.symmetric(horizontal: 4),
                          constraints: const BoxConstraints(minWidth: 14),
                          decoration: BoxDecoration(
                            color: palette.text,
                            borderRadius: BorderRadius.circular(7),
                          ),
                          child: Text(
                            '$badge',
                            textAlign: TextAlign.center,
                            textScaler: TextScaler.noScaling,
                            style: TextStyles.caption.copyWith(
                              color: palette.canvas,
                              fontSize: 10,
                              height: 1.4,
                            ),
                          ),
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The panel over the content from the trailing edge, when the window has
/// no room for it beside the document.
class _PanelOver extends StatelessWidget {
  const _PanelOver({required this.width, required this.child});

  final double width;
  final Widget child;

  @override
  Widget build(BuildContext context) => Positioned(
    key: const ValueKey('workbench-panel-over'),
    right: 0,
    top: 0,
    bottom: 0,
    child: LayoutBuilder(
      builder: (context, _) {
        final window = MediaQuery.sizeOf(context).width;
        return DecoratedBox(
          decoration: const BoxDecoration(boxShadow: floatingShadow),
          child: SizedBox(width: math.min(width, window * 0.85), child: child),
        );
      },
    ),
  );
}

/// A Lucide glyph in its 24-unit grid, at 16pt with a 1.5pt stroke, as
/// the app's own glyphs are drawn.
class _Glyph extends StatelessWidget {
  const _Glyph({required this.color, required this.draw});

  final Color color;
  final void Function(Canvas canvas, Paint paint) draw;

  @override
  Widget build(BuildContext context) => SizedBox.square(
    dimension: 16,
    child: CustomPaint(painter: _GlyphPainter(color, draw)),
  );
}

class _GlyphPainter extends CustomPainter {
  const _GlyphPainter(this.color, this.draw);

  final Color color;
  final void Function(Canvas canvas, Paint paint) draw;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.scale(size.width / 24);
    draw(
      canvas,
      Paint()
        ..color = color
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.5 * 24 / size.width
        ..strokeCap = StrokeCap.round
        ..strokeJoin = StrokeJoin.round,
    );
  }

  @override
  bool shouldRepaint(_GlyphPainter old) => old.color != color;
}

/// Lucide's panel-left.
void _sidebarGlyph(Canvas canvas, Paint paint) {
  canvas.drawRRect(
    RRect.fromLTRBR(3, 3, 21, 21, const Radius.circular(2)),
    paint,
  );
  canvas.drawLine(const Offset(9, 3), const Offset(9, 21), paint);
}

/// Lucide's columns-3.
void _panesGlyph(Canvas canvas, Paint paint) {
  canvas.drawRRect(
    RRect.fromLTRBR(3, 3, 21, 21, const Radius.circular(2)),
    paint,
  );
  canvas.drawLine(const Offset(9, 3), const Offset(9, 21), paint);
  canvas.drawLine(const Offset(15, 3), const Offset(15, 21), paint);
}

/// The window's workspace, for pane titles that name an app (none in a
/// test without one).
Workspace? _workspace(BuildContext context) => context
    .getInheritedWidgetOfExactType<AppScope>()
    ?.backend
    .workspace
    .workspace;
