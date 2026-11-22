import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../app_dock.dart' show AppStrip;
import '../backend.dart';
import '../characters/clock.dart' show CharacterHost;
import '../claktory_identity.dart' show claktoryName;
import '../controls.dart';
import '../identity.dart';
import '../phone.dart' show PhoneProjects;
import '../pipes.dart' show selectedPipe;
import '../rail.dart' show ClaktoriesScope, OffAccountMark, PhoneConnectionWord;
import '../scope.dart';
import '../selection.dart' show Unselectable;
import '../sidebar.dart';
import 'back.dart';
import 'header.dart';
import 'input.dart';
import 'metrics.dart';
import 'navigation.dart';
import 'platform_policy.dart';
import 'presentation.dart' show sheetsPresented;

/// What a phone's pages do for its drawer ([PhoneDrawerHost]): the
/// conversation at the root, and what's pushed over it.
abstract class PhonePages {
  Future<T?> push<T>(PhoneRoute<T> route);
  void pop<T>([T? result]);
  bool get canPop;

  /// Back to the conversation: every page pushed goes.
  void popAll();

  /// Pushes a project's Files.
  void showFiles(String projectId);

  /// [PhoneRoutes.showFile].
  bool showFile(String agentId, String path);

  /// [PhoneRoutes.showPane].
  bool showPane(String agentId, String paneId);

  /// The page on top, as Back closes it; null with none.
  PageTarget? get topPage;
}

/// How wide the drawer is on a screen [width] wide: room for its rail and
/// its tree, with a strip of the conversation left in view to tap closed.
double drawerWidth(double width) => math.min(400, width - 24);

/// The rail of Claktories in the drawer: narrower on the smallest phones.
double drawerRailWidth(double screenWidth) => screenWidth <= 320 ? 52 : 56;

/// Whether the person reads at one of iOS's accessibility text sizes, where
/// the drawer's icon rail gives way to a labelled row.
bool largeText(BuildContext context) =>
    MediaQuery.textScalerOf(context).scale(17) > 17 * 1.5;

/// A phone's leading drawer over the window (docs/IDENTITY.md, "Phone"):
/// the Claktories ([rail]) beside this one's projects, agents and pipes, a
/// search over them and what needs the person. The Menu button opens it,
/// and so does a swipe from the screen's leading edge while the
/// conversation is in front; a swipe back, a tap on the dimmed
/// conversation, Close or Esc closes it. It's the phone's [PhoneRoutes].
///
/// It sits over the Claktory's window rather than in it, so it stays open
/// while another Claktory takes the window's place, and stays in reach over
/// a step that covers the window (signing in, a name).
class PhoneDrawerHost extends StatefulWidget {
  const PhoneDrawerHost({
    super.key,
    required this.child,
    this.rail,
    this.onClaktories,
  });

  final Widget child;

  /// The Claktories, beside the tree; none with nothing to switch to.
  final Widget? rail;

  /// At large text sizes the rail is a "Claktories" row: this opens the
  /// list of them.
  final VoidCallback? onClaktories;

  /// The drawer above [context], which a phone's pages attach to.
  static PhoneDrawerHostState? of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<_HostScope>()?.host;

  /// Closes any phone drawer that's open, for what comes from outside the
  /// app (a link, say) and asks the person something in a sheet: from
  /// above the drawer it would open behind it.
  static void closeAll() {
    for (final host in PhoneDrawerHostState._live.toList()) {
      host.closeDrawer();
    }
  }

  @override
  State<PhoneDrawerHost> createState() => PhoneDrawerHostState();
}

class PhoneDrawerHostState extends State<PhoneDrawerHost>
    with SingleTickerProviderStateMixin
    implements PhoneRoutes {
  late final _drawer = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 280),
  );

  /// Where the drawer is, or is going.
  var _open = false;

  /// A finger is moving it.
  var _dragging = false;

  /// It was out when the finger took it.
  var _dragFromOpen = false;

  var _holds = 0;
  var _version = 0;
  PhonePages? _pages;

  /// Whether the conversation is in front (no drawer, no page over it):
  /// [PhoneNavigation]'s, which sets it to bring the conversation back.
  final _front = ValueNotifier(true);
  var _settingFront = false;

  final _scope = FocusScopeNode(debugLabel: 'Drawer');

  /// The phone's Back: the sheets, then this drawer, then the pages.
  late final back = PhoneBack(
    drawerOpen: () => _open,
    closeDrawer: closeDrawer,
    topPage: () => _pages?.topPage,
    popPage: pop,
  );

  /// Every drawer host showing now ([PhoneDrawerHost.closeAll]).
  static final _live = <PhoneDrawerHostState>{};

  @override
  void initState() {
    super.initState();
    _live.add(this);
    _front.addListener(_frontAsked);
  }

  @override
  void dispose() {
    _live.remove(this);
    back.dispose();
    _front.dispose();
    _drawer.dispose();
    _scope.dispose();
    super.dispose();
  }

  /// [pages] are the phone's pages from now on; the drawer works while
  /// they're there.
  void attach(PhonePages pages) {
    if (_pages == pages) return;
    _pages = pages;
    pagesChanged();
  }

  /// [pages] are going: the drawer closes with them, unless others take
  /// their place by the next frame (another Claktory's window).
  void detach(PhonePages pages) {
    if (_pages != pages) return;
    _pages = null;
    SchedulerBinding.instance.addPostFrameCallback((_) {
      if (!mounted || _pages != null || !_open) return;
      _open = false;
      _drawer.value = 0;
      pagesChanged();
    });
    pagesChanged();
  }

  /// A page came or went.
  void pagesChanged() {
    void apply() {
      if (!mounted) return;
      setState(() => _version++);
      _syncFront();
      back.update();
    }

    // Told from a build below, which can run outside a frame too (at
    // launch, while the phase reads idle): always after the frame.
    SchedulerBinding.instance
      ..addPostFrameCallback((_) => apply())
      ..ensureVisualUpdate();
  }

  void _syncFront() {
    final front = !_open && !canPop;
    if (_front.value == front) return;
    _settingFront = true;
    _front.value = front;
    _settingFront = false;
  }

  /// Someone asked for the conversation ([PhoneNavigation]).
  void _frontAsked() {
    if (_settingFront || !_front.value) return;
    closeDrawer();
    _pages?.popAll();
    pagesChanged();
  }

  Duration get _duration =>
      MediaQuery.maybeDisableAnimationsOf(context) ?? false
      ? Duration.zero
      : const Duration(milliseconds: 280);

  void _settle(bool open) {
    _open = open;
    _drawer.animateTo(
      open ? 1 : 0,
      duration: _duration,
      curve: Curves.easeOutCubic,
    );
    if (open) {
      // The keyboard goes; what was typed stays.
      FocusManager.instance.primaryFocus?.unfocus();
      SchedulerBinding.instance.addPostFrameCallback((_) {
        if (mounted && _open) _scope.requestFocus();
      });
    } else if (_scope.hasFocus) {
      _scope.unfocus();
    }
    pagesChanged();
  }

  @override
  bool get drawerOpen => _open;

  @override
  void openDrawer() {
    if (_pages == null || _open) return;
    _settle(true);
  }

  @override
  void closeDrawer() {
    if (!_open && _drawer.value == 0) return;
    _settle(false);
  }

  @override
  Future<T?> push<T>(PhoneRoute<T> route) async {
    final pages = _pages;
    if (pages == null) return null;
    closeDrawer();
    return pages.push(route);
  }

  @override
  void pop<T>([T? result]) => _pages?.pop(result);

  @override
  bool get canPop => _pages?.canPop ?? false;

  @override
  void openConversation(String agentId, {int? seq}) {
    final scope = AppScope.scope(context);
    final backend = scope.backend;
    AppScope.closePage(context);
    final workspace = backend.workspace.workspace;
    if (seq != null) backend.reveal.value = (agentId: agentId, seq: seq);
    if (workspace.selectedAgentId != agentId ||
        selectedPipe(workspace) != null ||
        workspace.selectedChatId.isNotEmpty) {
      fireAndLog(backend.selectAgent(agentId));
    }
    closeDrawer();
    _pages?.popAll();
    pagesChanged();
  }

  /// Pushes [projectId]'s Files over the conversation.
  void showFiles(String projectId) {
    final pages = _pages;
    if (pages == null) return;
    closeDrawer();
    pages.showFiles(projectId);
  }

  @override
  bool showFile(String agentId, String path) {
    final pages = _pages;
    if (pages == null) return false;
    closeDrawer();
    return pages.showFile(agentId, path);
  }

  @override
  bool showPane(String agentId, String paneId) {
    final pages = _pages;
    if (pages == null) return false;
    closeDrawer();
    return pages.showPane(agentId, paneId);
  }

  @override
  DrawerHold holdGestures() {
    _holds++;
    return _Hold(() => _holds--);
  }

  /// Whether the leading edge takes a swipe: with the conversation in
  /// front, where the platform's edge isn't its Back.
  bool get _edgeShown =>
      _pages != null &&
      !_open &&
      !canPop &&
      PhonePlatformPolicy.of(context).drawerEdgeSwipe;

  /// Whether a swipe from the edge may open it now: nothing else holds the
  /// finger, and no sheet is up.
  bool get _swipeAllowed =>
      _edgeShown && _holds == 0 && sheetsPresented.value == 0;

  double get _width => drawerWidth(MediaQuery.sizeOf(context).width);

  double get _sign =>
      Directionality.of(context) == TextDirection.rtl ? -1.0 : 1.0;

  void _dragStart(DragStartDetails details) {
    _dragging = true;
    _dragFromOpen = _open;
    _drawer.stop();
    setState(() {});
  }

  void _dragUpdate(DragUpdateDetails details) {
    if (!_dragging) return;
    _drawer.value += _sign * (details.primaryDelta ?? 0) / _width;
  }

  void _dragEnd(DragEndDetails details) {
    if (!_dragging) return;
    _dragging = false;
    final velocity = _sign * (details.primaryVelocity ?? 0);
    final bool open;
    if (velocity.abs() > 700) {
      open = velocity > 0;
    } else {
      // A third of the way is enough, either way.
      open = _dragFromOpen ? _drawer.value > 0.65 : _drawer.value > 0.35;
    }
    _settle(open);
  }

  // Also called for a tap that never became a drag: nothing moved then.
  void _dragCancel() {
    if (!_dragging) return;
    _dragging = false;
    _settle(_dragFromOpen);
  }

  @override
  Widget build(BuildContext context) {
    final width = _width;
    final rtl = _sign < 0;
    final drawer = CharacterHost(
      child: FocusScope(
        node: _scope,
        child: CallbackShortcuts(
          bindings: {
            const SingleActivator(LogicalKeyboardKey.escape): closeDrawer,
          },
          // Another Claktory's tree is a new one: nothing of the last
          // one's (its search, where it was scrolled) carries over.
          // A notch smaller than the phone's pages, so more is in view.
          child: DenseList(
            child: _DrawerPanel(
              key: ObjectKey(AppScope.of(context)),
              width: width,
              rail: widget.rail,
              onClaktories: widget.onClaktories,
              onClose: closeDrawer,
            ),
          ),
        ),
      ),
    );
    return _HostScope(
      host: this,
      child: PhoneRoutesScope(
        routes: this,
        version: _version,
        child: PhoneBackScope(
          back: back,
          child: PhoneNavigation(
            conversation: _front,
            child: PhoneProjects(
              files: (project) => showFiles(project.id),
              child: PhonePlatformPolicy.of(context).backHost(
                back: back,
                child: Stack(
                  fit: StackFit.expand,
                  children: [
                    // Under the open drawer, out of VoiceOver's and the
                    // keyboard's reach.
                    ExcludeSemantics(
                      excluding: _open,
                      child: ExcludeFocus(
                        excluding: _open,
                        child: widget.child,
                      ),
                    ),
                    if (_edgeShown)
                      PositionedDirectional(
                        start: 0,
                        top: 0,
                        bottom: 0,
                        width: 20,
                        child: RawGestureDetector(
                          behavior: HitTestBehavior.translucent,
                          gestures: {
                            _EdgeDrag:
                                GestureRecognizerFactoryWithHandlers<_EdgeDrag>(
                                  () => _EdgeDrag(allowed: () => _swipeAllowed),
                                  (drag) => drag
                                    ..onStart = _dragStart
                                    ..onUpdate = _dragUpdate
                                    ..onEnd = _dragEnd
                                    ..onCancel = _dragCancel,
                                ),
                          },
                        ),
                      ),
                    AnimatedBuilder(
                      animation: _drawer,
                      child: drawer,
                      builder: (context, drawer) {
                        final shown = _drawer.value;
                        final out = shown > 0 || _dragging;
                        return Offstage(
                          offstage: !out,
                          child: TickerMode(
                            enabled: out,
                            child: BlockSemantics(
                              blocking: _open,
                              child: GestureDetector(
                                onHorizontalDragStart: _dragStart,
                                onHorizontalDragUpdate: _dragUpdate,
                                onHorizontalDragEnd: _dragEnd,
                                onHorizontalDragCancel: _dragCancel,
                                child: Stack(
                                  fit: StackFit.expand,
                                  children: [
                                    Semantics(
                                      container: true,
                                      button: true,
                                      label: 'Close menu',
                                      onTap: closeDrawer,
                                      excludeSemantics: true,
                                      child: GestureDetector(
                                        onTap: closeDrawer,
                                        child: ColoredBox(
                                          color: const Color(0xFF000000)
                                              .withValues(alpha: 0.3 * shown),
                                        ),
                                      ),
                                    ),
                                    Align(
                                      alignment:
                                          AlignmentDirectional.centerStart,
                                      child: FractionalTranslation(
                                        translation: Offset(
                                          (rtl ? 1 : -1) * (1 - shown),
                                          0,
                                        ),
                                        child: SizedBox(
                                          width: width,
                                          child: drawer,
                                        ),
                                      ),
                                    ),
                                  ],
                                ),
                              ),
                            ),
                          ),
                        );
                      },
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

class _Hold implements DrawerHold {
  _Hold(this._release);

  VoidCallback? _release;

  @override
  void release() {
    _release?.call();
    _release = null;
  }
}

class _HostScope extends InheritedWidget {
  const _HostScope({required this.host, required super.child});

  final PhoneDrawerHostState host;

  @override
  bool updateShouldNotify(_HostScope old) => old.host != host;
}

/// The leading edge's swipe: a horizontal drag that gives the finger up as
/// soon as something else holds the drawer's swipe ([NoDrawerGesture]),
/// which takes its hold as the finger lands.
class _EdgeDrag extends HorizontalDragGestureRecognizer {
  _EdgeDrag({required this.allowed});

  final bool Function() allowed;

  @override
  void handleEvent(PointerEvent event) {
    if (event is PointerMoveEvent && !allowed()) {
      resolve(GestureDisposition.rejected);
      stopTrackingPointer(event.pointer);
      return;
    }
    super.handleEvent(event);
  }
}

/// What the drawer shows: the rail of Claktories at its leading edge, then
/// this Claktory's name and connection, Close, a search field and the tree
/// of its projects, agents and pipes ([Sidebar]), or what matches the
/// search ([SidebarSearch]).
/// How far the app strip's words grow with the text size: its chips are
/// 44pt high, like a tab bar's buttons.
const _appStripTextScale = 1.35;

/// How much of the drawer's height its top (the Claktory's name, the
/// Claktories row and the apps) may take before it scrolls within that.
const _topShare = 0.4;

/// The drawer's top when it can scroll: what's past its foot fades out,
/// so a line cut there reads as more to come, not as broken.
class _ScrollingTop extends StatefulWidget {
  const _ScrollingTop({required this.child});

  final Widget child;

  @override
  State<_ScrollingTop> createState() => _ScrollingTopState();
}

class _ScrollingTopState extends State<_ScrollingTop> {
  final _scroll = ScrollController();
  var _more = false;

  @override
  void dispose() {
    _scroll.dispose();
    super.dispose();
  }

  bool _measured(ScrollMetricsNotification notification) {
    _follow(notification.metrics);
    return false;
  }

  bool _scrolled(ScrollUpdateNotification notification) {
    _follow(notification.metrics);
    return false;
  }

  void _follow(ScrollMetrics metrics) {
    final more = metrics.extentAfter > 0.5;
    if (more != _more) setState(() => _more = more);
  }

  @override
  Widget build(BuildContext context) {
    final scroll = SingleChildScrollView(
      key: const ValueKey('drawer-top'),
      controller: _scroll,
      primary: false,
      child: widget.child,
    );
    return NotificationListener<ScrollMetricsNotification>(
      onNotification: _measured,
      child: NotificationListener<ScrollUpdateNotification>(
        onNotification: _scrolled,
        // Always masked, so the scroll view keeps its place in the tree
        // (and its offset) as the fade comes and goes.
        child: ShaderMask(
          blendMode: BlendMode.dstIn,
          shaderCallback: (bounds) => LinearGradient(
            begin: Alignment.topCenter,
            end: Alignment.bottomCenter,
            colors: [
              const Color(0xFFFFFFFF),
              _more ? const Color(0x00FFFFFF) : const Color(0xFFFFFFFF),
            ],
            stops: [math.max(0, 1 - Space.xl / math.max(bounds.height, 1)), 1],
          ).createShader(bounds),
          child: scroll,
        ),
      ),
    );
  }
}

class _DrawerPanel extends StatefulWidget {
  const _DrawerPanel({
    super.key,
    required this.width,
    required this.rail,
    required this.onClaktories,
    required this.onClose,
  });

  final double width;
  final Widget? rail;
  final VoidCallback? onClaktories;
  final VoidCallback onClose;

  @override
  State<_DrawerPanel> createState() => _DrawerPanelState();
}

class _DrawerPanelState extends State<_DrawerPanel> {
  final _query = TextEditingController();

  @override
  void dispose() {
    _query.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final safe = MediaQuery.paddingOf(context);
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    final backend = AppScope.of(context);
    final large = largeText(context);
    final rail = widget.rail;
    final railWidth = drawerRailWidth(MediaQuery.sizeOf(context).width);
    return Semantics(
      scopesRoute: true,
      namesRoute: true,
      explicitChildNodes: true,
      label: 'Menu',
      child: Container(
        decoration: BoxDecoration(
          color: palette.sidebar,
          border: BorderDirectional(end: BorderSide(color: palette.hairline)),
          boxShadow: floatingShadow,
        ),
        padding: EdgeInsets.only(
          left: safe.left,
          top: safe.top,
          bottom: math.max(safe.bottom, keyboard),
        ),
        child: withoutInsets(
          context,
          child: DefaultTextStyle(
            style: type.body.copyWith(color: palette.text),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (rail != null && !large) ...[
                  SizedBox(width: railWidth, child: rail),
                  Container(width: 1, color: palette.hairline),
                ],
                Expanded(
                  child: LayoutBuilder(
                    builder: (context, constraints) => Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        // The top (name, Claktories, apps) takes at most
                        // [_topShare] of the drawer and scrolls within it,
                        // so at the largest text the list keeps the rest.
                        ConstrainedBox(
                          constraints: BoxConstraints(
                            maxHeight: constraints.maxHeight * _topShare,
                          ),
                          child: _ScrollingTop(
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.stretch,
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                ListenableBuilder(
                                  listenable: backend.workspace,
                                  builder: (context, _) => _DrawerHeader(
                                    name: claktoryName(backend),
                                    backend: backend,
                                    onClose: widget.onClose,
                                  ),
                                ),
                                if (rail != null &&
                                    large &&
                                    widget.onClaktories != null)
                                  _ClaktoriesRow(onTap: widget.onClaktories!),
                                // The pinned apps, then All apps: nothing without
                                // apps. Its chips keep their height, as a tab bar's
                                // do, so their words grow only so far.
                                MediaQuery.withClampedTextScaling(
                                  maxScaleFactor: _appStripTextScale,
                                  child: const AppStrip(),
                                ),
                              ],
                            ),
                          ),
                        ),
                        Padding(
                          padding: const EdgeInsets.fromLTRB(
                            Space.s,
                            Space.xs,
                            Space.s,
                            Space.s,
                          ),
                          child: _SearchField(controller: _query),
                        ),
                        Expanded(
                          child: ListenableBuilder(
                            listenable: _query,
                            builder: (context, _) {
                              final query = _query.text.trim();
                              return query.isEmpty
                                  ? const Sidebar(width: double.infinity)
                                  : SidebarSearch(query: query);
                            },
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// The drawer's top: the Claktory's name over how it's connected, and
/// Close.
class _DrawerHeader extends StatelessWidget {
  const _DrawerHeader({
    required this.name,
    required this.backend,
    required this.onClose,
  });

  final String name;
  final Backend backend;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Unselectable(
      child: Padding(
        padding: const EdgeInsetsDirectional.only(
          start: Space.l,
          end: Space.xs,
          top: Space.xs,
        ),
        child: Row(
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Semantics(
                    header: true,
                    child: Text(
                      name,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: type.title.copyWith(color: palette.text),
                    ),
                  ),
                  // Not on the person's Claktory account: marked here too.
                  if (ClaktoriesScope.of(context)?.selected case final shown?
                      when identical(shown.backend, backend))
                    // Kept to about two lines at the largest text: the
                    // list, the form and VoiceOver have it whole.
                    MediaQuery.withClampedTextScaling(
                      maxScaleFactor: 1.6,
                      child: OffAccountMark(claktory: shown),
                    ),
                  Transform.translate(
                    offset: const Offset(-Space.xs, 0),
                    child: PhoneConnectionWord(backend),
                  ),
                ],
              ),
            ),
            HeaderButton.close(onTap: onClose),
          ],
        ),
      ),
    );
  }
}

/// At accessibility text sizes, in the rail's place: a row to the list of
/// Claktories.
class _ClaktoriesRow extends StatelessWidget {
  const _ClaktoriesRow({required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Semantics(
      button: true,
      label: 'Claktories',
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          constraints: const BoxConstraints(minHeight: Touch.target),
          margin: const EdgeInsets.symmetric(horizontal: Space.s),
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          decoration: BoxDecoration(
            color: hovered ? palette.hover : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  'Claktories',
                  style: type.label.copyWith(color: palette.text),
                ),
              ),
              Transform.rotate(
                angle: math.pi,
                child: ExtensionGlyph(
                  icon: 'chevron-left',
                  name: '',
                  color: palette.textMuted,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// "Search", over the drawer's tree: what's typed narrows it to the agents,
/// projects and pipes whose names match, here on the phone.
class _SearchField extends StatefulWidget {
  const _SearchField({required this.controller});

  final TextEditingController controller;

  @override
  State<_SearchField> createState() => _SearchFieldState();
}

class _SearchFieldState extends State<_SearchField> {
  final _focus = FocusNode(debugLabel: 'Search');

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final controller = widget.controller;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: palette.hover,
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Row(
        children: [
          // The box up to Clear is the field: a tap anywhere in it, or
          // VoiceOver's and TalkBack's, brings the keyboard.
          Expanded(
            child: TextFieldBox(
              focusNode: _focus,
              child: ConstrainedBox(
                // iOS's search field's height, growing with the text;
                // Android's target where that's taller.
                constraints: BoxConstraints(
                  minHeight: PhonePlatformPolicy.least(
                    context,
                    Touch.denseLine,
                  ),
                ),
                child: Padding(
                  padding: const EdgeInsetsDirectional.only(start: Space.m),
                  child: Row(
                    children: [
                      ExcludeSemantics(
                        child: ExtensionGlyph(
                          icon: 'search',
                          name: '',
                          color: palette.textFaint,
                        ),
                      ),
                      Expanded(
                        child: TextField(
                          controller: controller,
                          focusNode: _focus,
                          placeholder: 'Search',
                          // Still "Search" once something's typed.
                          semanticsLabel: 'Search',
                          bordered: false,
                          kind: InputKind.text,
                          textInputAction: TextInputAction.search,
                          autocorrect: false,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
          ListenableBuilder(
            listenable: controller,
            builder: (context, _) => controller.text.isEmpty
                ? const SizedBox.shrink()
                : GlyphButton(
                    label: 'Clear search',
                    active: false,
                    onTap: controller.clear,
                    glyph: (color) =>
                        ExtensionGlyph(icon: 'x', name: '', color: color),
                  ),
          ),
        ],
      ),
    );
  }
}
