import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart' show defaultTargetPlatform;
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart' show SchedulerBinding;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_name.dart';
import 'avatar.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'chats.dart'
    show
        ChatGlyph,
        ChatHistory,
        chatActions,
        chatTitle,
        chatWaiting,
        chatsNewestFirst,
        openChat,
        selectedChat,
        startNewChat;
import 'connection.dart';
import 'explorer.dart' show filesShown, openFiles, projectById;
import 'controls.dart';
import 'forms.dart';
import 'highlights.dart';
import 'inbox.dart' show needsYouCount, needsYouLabel;
import 'identity.dart';
import 'loopback_forward.dart' show ForwardingsLine;
import 'markdown.dart';
import 'quiet_copies.dart' show quietWaitingAbout, quietWaitingShort;
import 'mobile/drawer.dart' show largeText;
import 'mobile/haptics.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'mobile/navigation.dart' show PhoneRoute, PhoneRoutes;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'kept.dart';
import 'mobile/platform_policy.dart';
import 'people/people_view.dart' show peopleAvailable, showPeople;
import 'phone.dart' show PhoneProjects;
import 'pinned_agents.dart';
import 'pipe_groups.dart';
import 'pipe_timeline.dart' show pipeLiaison;
import 'pipes.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'settings_view.dart' show versionLabel;
import 'state.dart' show isAdminAgent;
import 'usage.dart';
import 'usage_wait.dart';
import 'waiting.dart' show NeedsYouMark, waitingIn, waitingLabel;
import 'team_activity.dart';
import 'tell_rivet.dart';

/// The workspace's own agents, then projects with their agents nested under
/// them. Agents can be dragged between the workspace and projects. With a
/// project focused, it shows only that project.
class Sidebar extends StatelessWidget {
  const Sidebar({super.key, this.width = sidebarWidth});

  /// The whole screen's width on a phone.
  final double width;

  @override
  Widget build(BuildContext context) {
    final scope = AppScope.scope(context);
    final backend = scope.backend;
    final palette = Palette.of(context);
    // On a phone it's the drawer's tree, which draws its own edge.
    final phone = isPhoneLayout(context);
    final sidebar = DecoratedBox(
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: phone
            ? null
            : Border(right: BorderSide(color: palette.hairline)),
      ),
      child: SizedBox(
        width: width,
        child: ListenableBuilder(
          listenable: Listenable.merge([
            backend.workspace,
            backend.connected,
            backend.reconnecting,
            backend.refused,
            scope.page,
          ]),
          builder: (context, _) {
            final workspace = backend.workspace.workspace;
            final page = scope.page.value;
            // An agent's row isn't selected while a pipe shows instead.
            final selected =
                page == AppPage.agent &&
                    selectedPipe(workspace) == null &&
                    selectedChat(workspace) == null
                ? workspace.selectedAgentId
                : '';
            Project? focused;
            for (final project in workspace.projects) {
              if (project.id == workspace.focusedProjectId) focused = project;
            }
            // Rivet stays reachable in a focused project, since it's the
            // one who changes the project for the person.
            final rivet = rivetIn(workspace);
            final rivetBeside =
                focused != null &&
                    rivet != null &&
                    isAdminAgent(rivet) &&
                    rivet.projectId != focused.id
                ? rivet
                : null;
            // The agents the person pinned under Rivet, from any project.
            final pins = pinnedAgents(workspace);
            final pinIds = {for (final agent in pins) agent.id};
            // Above both sections, where they don't scroll away (on a
            // Mac, while they take no more than a third of it): what waits
            // for the person, and the workspace's agents (Rivet), or Rivet
            // beside a focused project, then the pins.
            final topAgents = focused == null
                ? backend.workspace.agentsIn('')
                : [?rivetBeside];
            final top = <Widget>[
              // Everything that waits for the person in one place, on a
              // Mac and in a phone's drawer alike: there only while
              // something does.
              if (needsYouCount(workspace) case final count when count > 0) ...[
                _NeedsYouRow(count: count),
                SizedBox(height: phone ? Space.m : Space.s),
              ],
              if (topAgents.isNotEmpty)
                _Place(
                  projectId: focused == null ? '' : rivetBeside!.projectId,
                  header: null,
                  agents: topAgents,
                  selectedAgentId: selected,
                ),
              if (pins.isNotEmpty)
                _Pins(
                  pins: pins,
                  selectedAgentId: selected,
                  // In line with the agents above them.
                  gutter:
                      !phone &&
                      topAgents.any(
                        (agent) => topAgents.any(
                          (other) => other.parentId == agent.id,
                        ),
                      ),
                ),
              if (topAgents.isNotEmpty || pins.isNotEmpty)
                SizedBox(height: focused == null ? Space.l : Space.s),
            ];
            final projects = <Widget>[
              if (focused != null) ...[
                // What's out of sight elsewhere glows here: not Rivet
                // beside it, nor the pins while they stay in sight.
                Builder(
                  builder: (context) {
                    final shown = {
                      ?rivetBeside?.id,
                      if (_pinsStay(context)) ...pinIds,
                    };
                    return Glow(
                      store: backend.highlights,
                      matches: (key) =>
                          _elsewhere(key, workspace, focused!.id, shown: shown),
                      child: _QuietRow(
                        label: '‹ All projects',
                        waiting: waitingIn([
                          for (final agent in workspace.agents)
                            if (agent.projectId != focused!.id &&
                                !shown.contains(agent.id) &&
                                archivedProjectOf(workspace, agent) == null)
                              agent,
                        ]),
                        onTap: () {
                          AppScope.scope(context).history.clear();
                          fireAndLog(backend.focusProject(''));
                        },
                      ),
                    );
                  },
                ),
                const SizedBox(height: Space.s),
                _Place(
                  projectId: focused.id,
                  header: _ProjectHeading(project: focused),
                  folders: focused.folders,
                  agents: backend.workspace.agentsIn(focused.id),
                  selectedAgentId: selected,
                ),
              ] else ...[
                for (final project in workspace.projects)
                  if (!project.archived)
                    _ProjectSlot(
                      project: project,
                      order: [for (final p in workspace.projects) p.id],
                      shown: [
                        for (final p in workspace.projects)
                          if (!p.archived) p.id,
                      ],
                      place: (header) => _Place(
                        projectId: project.id,
                        header: header,
                        folders: project.folders,
                        agents: backend.workspace.agentsIn(project.id),
                        selectedAgentId: selected,
                        // On a phone, a small step, so the left edge isn't
                        // spent on depth.
                        indent: phone ? Space.s : Space.l,
                      ),
                    ),
                // Out of the way, and back with a click.
                if (workspace.projects.any((p) => p.archived))
                  _Archived(
                    projects: [
                      for (final project in workspace.projects)
                        if (project.archived) project,
                    ],
                  ),
              ],
            ];
            final chats = chatsNewestFirst(workspace);
            // The person's own chats, beside the projects.
            final openChat = page == AppPage.agent
                ? workspace.selectedChatId
                : '';
            final chatRows = Kept(
              inputs: [chats, openChat],
              builder: (_) => _Chats(chats: chats, selectedId: openChat),
            );
            // Each heading the same while what it shows is.
            final projectCount = workspace.projects
                .where((p) => !p.archived)
                .length;
            final inProjects = [
              for (final agent in workspace.agents)
                if (agent.projectId.isNotEmpty &&
                    archivedProjectOf(workspace, agent) == null)
                  agent,
            ];
            final projectsWaiting = waitingIn(inProjects);
            // Folded, it leaves out the pins while they stay in sight.
            final unpinnedWaiting = waitingIn([
              for (final agent in inProjects)
                if (!pinIds.contains(agent.id)) agent,
            ]);
            final projectsHeading = Kept(
              inputs: [
                workspace.projectsCollapsed,
                projectCount,
                projectsWaiting,
                unpinnedWaiting,
              ],
              builder: (context) => _SectionHeading(
                label: 'Projects',
                open: !workspace.projectsCollapsed,
                count: projectCount,
                waiting: _pinsStay(context) ? unpinnedWaiting : projectsWaiting,
                onToggle: () => fireAndLog(
                  backend.updateSettings(
                    UpdateSettings(
                      projectsCollapsed: !workspace.projectsCollapsed,
                    ),
                  ),
                ),
                trailing: _AskRivet(
                  starter: 'Set up a new project for ',
                  fallback: (context, close) => CreateProjectForm(close: close),
                ),
              ),
            );
            final chatsWaiting = chats.fold(
              0,
              (sum, chat) => sum + chatWaiting(chat),
            );
            final chatsHeading = Kept(
              inputs: [workspace.chatsCollapsed, chats.length, chatsWaiting],
              builder: (context) => _SectionHeading(
                label: 'Chats',
                open: !workspace.chatsCollapsed,
                count: chats.length,
                waiting: chatsWaiting,
                onToggle: () => fireAndLog(
                  backend.updateSettings(
                    UpdateSettings(chatsCollapsed: !workspace.chatsCollapsed),
                  ),
                ),
                trailing: _QuietPlus(
                  tip: 'New chat',
                  onTap: () => _Chats.startNew(context),
                ),
              ),
            );
            const inset = EdgeInsets.symmetric(horizontal: Space.s);
            if (phone) {
              // One list, as iOS has it: each section's heading stays at
              // the top while its section is in view.
              return CustomScrollView(
                slivers: [
                  SliverPadding(
                    padding: inset,
                    sliver: SliverList.list(children: top),
                  ),
                  _phoneSection(
                    context,
                    projectsHeading,
                    workspace.projectsCollapsed ? null : projects,
                  ),
                  _phoneSection(
                    context,
                    chatsHeading,
                    workspace.chatsCollapsed ? null : [chatRows],
                  ),
                  SliverPadding(
                    padding: const EdgeInsets.fromLTRB(
                      Space.s,
                      0,
                      Space.s,
                      Space.m,
                    ),
                    sliver: SliverToBoxAdapter(
                      child: _PhoneFooter(
                        workspace: workspace,
                        canAddProvider: _canAddProvider(backend, workspace),
                      ),
                    ),
                  ),
                ],
              );
            }
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                // The traffic lights' row.
                const TitleSpace(),
                Expanded(
                  child: FocusTraversalGroup(
                    child: LayoutBuilder(
                      builder: (context, box) => _Sections(
                        height: box.maxHeight,
                        top: top,
                        projectsHeading: projectsHeading,
                        projects: workspace.projectsCollapsed ? null : projects,
                        chatsHeading: chatsHeading,
                        chats: workspace.chatsCollapsed ? null : [chatRows],
                      ),
                    ),
                  ),
                ),
                _Footer(
                  workspace: workspace,
                  connected: backend.connected.value,
                  canAddProvider: _canAddProvider(backend, workspace),
                  page: page,
                ),
              ],
            );
          },
        ),
      ),
    );
    // ⌘1 to ⌘9 open the pins on a Mac; an iPad lists them in its own
    // commands.
    return defaultTargetPlatform == TargetPlatform.macOS
        ? _PinKeys(child: sidebar)
        : sidebar;
  }
}

/// ⌘1 to ⌘9 open the pins' conversations in order, as Messages does its
/// pinned conversations: wherever the keys are, a field's or nothing's,
/// but not under a dialog or a page opened over the window.
class _PinKeys extends StatefulWidget {
  const _PinKeys({required this.child});

  final Widget child;

  @override
  State<_PinKeys> createState() => _PinKeysState();
}

class _PinKeysState extends State<_PinKeys> {
  static const _digits = [
    LogicalKeyboardKey.digit1,
    LogicalKeyboardKey.digit2,
    LogicalKeyboardKey.digit3,
    LogicalKeyboardKey.digit4,
    LogicalKeyboardKey.digit5,
    LogicalKeyboardKey.digit6,
    LogicalKeyboardKey.digit7,
    LogicalKeyboardKey.digit8,
    LogicalKeyboardKey.digit9,
  ];

  @override
  void initState() {
    super.initState();
    HardwareKeyboard.instance.addHandler(_key);
  }

  @override
  void dispose() {
    HardwareKeyboard.instance.removeHandler(_key);
    super.dispose();
  }

  bool _key(KeyEvent event) {
    if (event is! KeyDownEvent) return false;
    final keys = HardwareKeyboard.instance;
    if (!keys.isMetaPressed ||
        keys.isShiftPressed ||
        keys.isAltPressed ||
        keys.isControlPressed) {
      return false;
    }
    final at = _digits.indexOf(event.logicalKey);
    if (at < 0 || !mounted) return false;
    if (ModalRoute.of(context)?.isCurrent == false) return false;
    final pins = pinnedAgents(AppScope.of(context).workspace.workspace);
    if (at >= pins.length) return false;
    openPinned(context, pins[at].id);
    return true;
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

/// Opens [agent]'s conversation: at what waits for the person if it's
/// news, or with [reach] (its needs-you mark), however far back. On a
/// phone the drawer closes on it, and the phone taps.
void _selectAgent(BuildContext context, Agent agent, {bool reach = false}) {
  final backend = AppScope.of(context);
  AppScope.closePage(context);
  fireAndLog(() async {
    await backend.selectAgent(agent.id);
    // Its conversation is here now: it opens at what waits for the
    // person, if anything does.
    final now = backend.workspace.agent(agent.id);
    if (now == null) return;
    if (reach) {
      backend.reveal.reach(now);
    } else {
      backend.reveal.oldestWaiting(now);
    }
  }());
  _toConversation(context);
}

/// Opens [pipe] in place of the conversation, as [_selectAgent] does an
/// agent's.
void _selectPipe(BuildContext context, Pipe pipe) {
  openPipe(context, pipe);
  _toConversation(context);
}

/// On a phone, what was chosen comes to the front: the drawer closes and
/// pages pushed go.
void _toConversation(BuildContext context) {
  final front = PhoneNavigation.of(context);
  if (front == null) return;
  unawaited(Haptics.selection());
  front.value = true;
}

/// Whether [key] is something outside the focused project [projectId]:
/// another project, its folders and agents, or the workspace's agents,
/// except the agents [shown] beside it (Rivet, and the pins in sight).
bool _elsewhere(
  String key,
  Workspace workspace,
  String projectId, {
  Set<String> shown = const {},
}) {
  if (key.startsWith('project:')) return key != 'project:$projectId';
  if (key.startsWith('folder:')) return !key.startsWith('folder:$projectId/');
  final others = {
    for (final agent in workspace.agents)
      if (agent.projectId != projectId &&
          !shown.contains(agent.id) &&
          archivedProjectOf(workspace, agent) == null)
        agent.id,
  };
  return matchesAgents(key, others);
}

/// The sidebar's two sections, Projects and Chats, on a Mac or an iPad:
/// [top] stays above them, and each section's heading always shows,
/// however many projects or chats there are. Open, Projects takes what it
/// needs and scrolls past the rest of the room; Chats under it takes what
/// it needs up to 40% of the sidebar (room for three rows at least), and
/// scrolls past that. A folded section is only its heading. Null [projects]
/// or [chats] is folded.
class _Sections extends StatefulWidget {
  const _Sections({
    required this.height,
    required this.top,
    required this.projectsHeading,
    required this.projects,
    required this.chatsHeading,
    required this.chats,
  });

  final double height;
  final List<Widget> top;
  final Widget projectsHeading;
  final List<Widget>? projects;
  final Widget chatsHeading;
  final List<Widget>? chats;

  @override
  State<_Sections> createState() => _SectionsState();
}

/// When [top] and the headings alone are taller than the room (an iPad's
/// largest text in a short window), the sections can't keep their
/// headings in view: then they scroll as one list, all of it a scroll
/// away, rather than spilling.
class _SectionsState extends State<_Sections> {
  static const _inset = EdgeInsets.symmetric(horizontal: Space.s);

  /// The height the sections took when they last didn't fit, for what
  /// [_neededFor] says they held; null while they fit.
  double? _needed;
  Object? _neededFor;

  /// How tall [_Sections.top] last was: past a third of the sidebar (Rivet
  /// and many pins), it scrolls with the projects instead of staying put.
  double _topHeight = 0;

  bool get _topScrolls => _topHeight > widget.height / 3;

  /// What the sections measured while an agent was dragged, applied once
  /// it's let go: laid out again mid-drag (the dragged row's gap changing
  /// the top's height), the rows would be built anew and the drag lost
  /// with its end, and the menu a hold opens (Gelato, an iPad at AX5).
  double? _heldTop;
  double? _heldNeeded;

  /// Rivet and the pins keep their rows (and a drag one of them holds)
  /// when the sections change how they're laid out: staying put, scrolling
  /// with the projects, or one list (Gelato: a hold's drag lost, an iPad
  /// at AX5).
  final _topKey = GlobalKey(debugLabel: 'sidebar top');

  @override
  void initState() {
    super.initState();
    sidebarDragging.addListener(_dragged);
  }

  @override
  void dispose() {
    sidebarDragging.removeListener(_dragged);
    super.dispose();
  }

  void _dragged() {
    if (sidebarDragging.value || !mounted) return;
    final top = _heldTop;
    final needed = _heldNeeded;
    _heldTop = null;
    _heldNeeded = null;
    if (top == null && needed == null) return;
    setState(() {
      if (top != null) _topHeight = top;
      if (needed != null) _needed = needed;
    });
  }

  @override
  Widget build(BuildContext context) {
    final sections = _sections(context);
    // Folded rows leave out the pins only while they can't scroll away.
    return _PinsInSight(stay: !_topScrolls && _needed == null, child: sections);
  }

  Widget _measured(Widget top) => _SizeReport(
    onSize: (size) {
      if (!mounted || (size.height - _topHeight).abs() <= 0.5) return;
      if (sidebarDragging.value) {
        _heldTop = size.height;
        return;
      }
      setState(() => _topHeight = size.height);
    },
    child: top,
  );

  Widget _sections(BuildContext context) {
    final projects = widget.projects;
    final chats = widget.chats;
    // What they take changes with the text's size and what's fixed.
    final holding = (
      MediaQuery.textScalerOf(context).scale(100),
      widget.top.length,
      projects == null,
      chats == null,
    );
    if (holding != _neededFor) {
      _neededFor = holding;
      _needed = null;
    }
    final top = widget.top.isEmpty
        ? null
        : Padding(
            padding: _inset,
            child: _measured(
              Column(
                key: _topKey,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: widget.top,
              ),
            ),
          );
    if (_needed case final needed? when widget.height < needed) {
      return ListView(
        key: const ValueKey('sidebar-one-list'),
        padding: const EdgeInsets.only(bottom: Space.m),
        children: [
          ?top,
          Padding(padding: _inset, child: widget.projectsHeading),
          for (final row in projects ?? const <Widget>[])
            Padding(padding: _inset, child: row),
          Padding(
            padding: _inset.copyWith(top: Space.s),
            child: widget.chatsHeading,
          ),
          for (final row in chats ?? const <Widget>[])
            Padding(padding: _inset, child: row),
        ],
      );
    }
    Widget list(List<Widget> children, {double bottom = 0}) => ListView(
      shrinkWrap: true,
      padding: _inset.copyWith(bottom: bottom),
      children: children,
    );
    // Taller than a third of the sidebar, Rivet and the pins scroll with
    // the projects, above their heading.
    if (_topScrolls && top != null) {
      return _FitColumn(
        onOverflow: (needed) {
          if (!mounted || needed == _needed) return;
          if (sidebarDragging.value) {
            _heldNeeded = needed;
            return;
          }
          setState(() => _needed = needed);
        },
        children: [
          Flexible(
            child: ListView(
              shrinkWrap: true,
              children: [
                top,
                Padding(padding: _inset, child: widget.projectsHeading),
                for (final row in projects ?? const <Widget>[])
                  Padding(padding: _inset, child: row),
              ],
            ),
          ),
          Padding(
            padding: _inset.copyWith(top: Space.s),
            child: widget.chatsHeading,
          ),
          if (chats != null)
            ConstrainedBox(
              constraints: BoxConstraints(
                maxHeight: math.max(widget.height * 0.4, 3 * 28.0 + Space.m),
              ),
              child: list(chats, bottom: Space.m),
            ),
        ],
      );
    }
    return _FitColumn(
      onOverflow: (needed) {
        if (!mounted || needed == _needed) return;
        if (sidebarDragging.value) {
          _heldNeeded = needed;
          return;
        }
        setState(() => _needed = needed);
      },
      children: [
        ?top,
        Padding(padding: _inset, child: widget.projectsHeading),
        if (projects != null) Flexible(child: list(projects)),
        Padding(
          padding: _inset.copyWith(top: Space.s),
          child: widget.chatsHeading,
        ),
        if (chats != null)
          if (projects == null)
            Flexible(child: list(chats, bottom: Space.m))
          else
            ConstrainedBox(
              constraints: BoxConstraints(
                maxHeight: math.max(widget.height * 0.4, 3 * 28.0 + Space.m),
              ),
              child: list(chats, bottom: Space.m),
            ),
      ],
    );
  }
}

/// Whether the top group (Rivet and the pins) stays put above the
/// projects ([_Sections]): then folded rows don't count the pins again,
/// since they're in sight (Lucy's pinned-agents.md, rule 3). Where it
/// scrolls with the rest (always on a phone) they count them.
class _PinsInSight extends InheritedWidget {
  const _PinsInSight({required this.stay, required super.child});

  final bool stay;

  @override
  bool updateShouldNotify(_PinsInSight old) => stay != old.stay;
}

/// Whether folded rows here leave out the pinned agents ([_PinsInSight]).
bool _pinsStay(BuildContext context) =>
    context.dependOnInheritedWidgetOfExactType<_PinsInSight>()?.stay ?? false;

/// Says [child]'s size after each layout that changed it, once the frame
/// is done.
class _SizeReport extends SingleChildRenderObjectWidget {
  const _SizeReport({required this.onSize, super.child});

  final ValueChanged<Size> onSize;

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderSizeReport(onSize);

  @override
  void updateRenderObject(
    BuildContext context,
    covariant _RenderSizeReport renderObject,
  ) => renderObject.onSize = onSize;
}

class _RenderSizeReport extends RenderProxyBox {
  _RenderSizeReport(this.onSize);

  ValueChanged<Size> onSize;
  Size? _said;

  @override
  void performLayout() {
    super.performLayout();
    if (size == _said) return;
    _said = size;
    final report = onSize;
    final now = size;
    SchedulerBinding.instance.addPostFrameCallback((_) => report(now));
  }
}

/// A stretched column that says, after laying out, when its children
/// took more than its height ([onOverflow], with what they took, once the
/// frame is done), and clips them meanwhile rather than spilling.
class _FitColumn extends Flex {
  const _FitColumn({required this.onOverflow, super.children})
    : super(
        direction: Axis.vertical,
        crossAxisAlignment: CrossAxisAlignment.stretch,
      );

  final ValueChanged<double> onOverflow;

  @override
  RenderFlex createRenderObject(BuildContext context) => _RenderFitColumn(
    onOverflow: onOverflow,
    textDirection: getEffectiveTextDirection(context),
  );

  @override
  void updateRenderObject(BuildContext context, RenderFlex renderObject) {
    super.updateRenderObject(context, renderObject);
    (renderObject as _RenderFitColumn).onOverflow = onOverflow;
  }
}

class _RenderFitColumn extends RenderFlex {
  _RenderFitColumn({required this.onOverflow, super.textDirection})
    : super(
        direction: Axis.vertical,
        crossAxisAlignment: CrossAxisAlignment.stretch,
      );

  ValueChanged<double> onOverflow;
  var _overflows = false;

  @override
  void performLayout() {
    super.performLayout();
    var taken = 0.0;
    var child = firstChild;
    while (child != null) {
      taken += child.size.height;
      child = childAfter(child);
    }
    _overflows = taken > size.height + 0.5;
    if (_overflows) {
      final report = onOverflow;
      SchedulerBinding.instance.addPostFrameCallback((_) => report(taken));
    }
  }

  @override
  void paint(PaintingContext context, Offset offset) {
    if (!_overflows) return super.paint(context, offset);
    _clip.layer = context.pushClipRect(
      needsCompositing,
      offset,
      Offset.zero & size,
      defaultPaint,
      oldLayer: _clip.layer,
    );
  }

  final _clip = LayerHandle<ClipRectLayer>();

  @override
  void dispose() {
    _clip.layer = null;
    super.dispose();
  }
}

/// A section of the phone's drawer: its [heading], which stays at the top
/// while the section is in view, over its [rows] (null when folded).
Widget _phoneSection(
  BuildContext context,
  Widget heading,
  List<Widget>? rows,
) => SliverMainAxisGroup(
  slivers: [
    PinnedHeaderSliver(
      child: ColoredBox(
        color: Palette.of(context).sidebar,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          child: heading,
        ),
      ),
    ),
    if (rows != null)
      SliverPadding(
        padding: const EdgeInsets.fromLTRB(Space.s, 0, Space.s, Space.s),
        sliver: SliverList.list(children: rows),
      ),
  ],
);

/// A section's heading, Projects or Chats: a chevron that turns, the
/// section's name in caption, and its [trailing] "+". A tap folds the
/// section or opens it; folded, it says how many it holds and whether any
/// waits for the person. Open or folded is the backend's, the same on
/// every device.
class _SectionHeading extends StatelessWidget {
  const _SectionHeading({
    required this.label,
    required this.open,
    required this.count,
    required this.waiting,
    required this.onToggle,
    required this.trailing,
  });

  final String label;
  final bool open;

  /// How many it holds, said while folded.
  final int count;

  /// How many things in it wait for the person's answer, marked while
  /// folded.
  final int waiting;
  final VoidCallback onToggle;
  final Widget trailing;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return _Row(
      onTap: onToggle,
      builder: (context, hovered) {
        final color = hovered ? palette.textMuted : palette.textFaint;
        return Row(
          children: [
            Expanded(
              // Said by the row itself, one element its whole height;
              // its "+" an element of its own.
              child: Semantics(
                header: true,
                button: true,
                expanded: open,
                label: label,
                value: open || count == 0 ? null : '$count',
                child: ExcludeSemantics(
                  child: Row(
                    children: [
                      SizedBox(
                        width: Space.m + 2,
                        child: Align(
                          alignment: Alignment.centerLeft,
                          child: AnimatedRotation(
                            turns: open ? 0.25 : 0,
                            duration: Motion.fast,
                            curve: Curves.easeOut,
                            child: Chevron(color: color, down: false),
                          ),
                        ),
                      ),
                      Flexible(
                        child: Text(
                          label,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TypeScale.of(context).caption.copyWith(
                            color: color,
                            fontWeight: FontWeight.w500,
                          ),
                        ),
                      ),
                      if (!open && count > 0)
                        Padding(
                          padding: const EdgeInsets.only(left: Space.s),
                          child: Text(
                            '$count',
                            style: TypeScale.of(context).caption
                                .copyWith(color: palette.textFaint),
                          ),
                        ),
                    ],
                  ),
                ),
              ),
            ),
            if (!open && waiting > 0)
              _needsYou(waitingLabel(label, waiting), waiting),
            trailing,
          ],
        );
      },
    );
  }
}

/// A quiet "+" that hands setting something up to Rivet, the admin agent,
/// which can tailor it (prompts, models, extensions) to what the person
/// asks: it opens Rivet's conversation with [starter] in its message box,
/// for the person to finish and send. Without a Rivet that can run, it
/// opens [fallback], the form that sets it up directly, so it's never a dead
/// end.
class _AskRivet extends StatelessWidget {
  const _AskRivet({required this.starter, required this.fallback});

  final String starter;
  final Widget Function(BuildContext context, VoidCallback close) fallback;

  static const _tip = 'Ask Rivet to set it up';

  void _ask(BuildContext context) {
    if (askRivet(context, starter)) {
      // On a phone, Rivet's conversation comes to the front, the words
      // waiting in its message box.
      _toConversation(context);
    } else {
      showModal(context, fallback);
    }
  }

  @override
  Widget build(BuildContext context) =>
      _QuietPlus(tip: _tip, onTap: () => _ask(context));
}

/// A caption's quiet "+" on a Mac: a 20px square, its [tip] on hover. On a
/// phone, a [HeaderButton] at the row's end instead.
class _QuietPlus extends StatelessWidget {
  const _QuietPlus({required this.tip, required this.onTap});

  final String tip;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    if (isPhoneLayout(context)) {
      return HeaderButton(
        label: tip,
        inRow: true,
        onTap: onTap,
        glyph: (color) => ExtensionGlyph(icon: 'plus', name: '+', color: color),
      );
    }
    return _FingerTarget(
      // At its row's end: nothing after it that takes a tap.
      after: 0,
      child: HoverCard(
        card: (context) => Text(tip),
        child: Semantics(
          button: true,
          label: tip,
          child: Pressable(
            onTap: onTap,
            builder: (context, hovered) => AnimatedContainer(
              duration: Motion.fast,
              curve: Curves.easeOut,
              width: 20,
              height: 20,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: hovered ? palette.hover : const Color(0x00000000),
                borderRadius: BorderRadius.circular(Radii.control),
              ),
              child: Transform.scale(
                scale: 0.875,
                child: ExtensionGlyph(
                  icon: 'plus',
                  name: '+',
                  color: hovered ? palette.textMuted : palette.textFaint,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Where agents live: a project, or the workspace (empty [projectId]), whose
/// agents (Rivet) stand at the top of the sidebar with no heading. New
/// agents go in projects, through the project row's "+" ([_AskRivet]). Its
/// agents show as a tree, in the order the backend sends them, each under
/// its parent. Dropping an agent on the place itself (its header, or below
/// its agents) puts it at the place's top level; rows take drops before,
/// after and onto themselves.
class _Place extends StatelessWidget {
  const _Place({
    required this.projectId,
    required this.header,
    required this.agents,
    required this.selectedAgentId,
    this.folders = const [],
    this.indent = 0,
  });

  final String projectId;

  /// The project's folders: its agents in one show under its heading, after
  /// the agents in the project's own folder.
  final List<ProjectFolder> folders;

  /// The project's row; null for the workspace, which has no heading.
  final Widget? header;
  final List<Agent> agents;
  final String selectedAgentId;

  /// How far this place's agents sit in from its header.
  final double indent;

  /// The request that puts [agent] at this place's top level, last: out of
  /// any folder, and from under its lead.
  ArrangeAgent _toPlace(Agent agent) {
    final arrange = ArrangeAgent(agentId: agent.id);
    if (agent.projectId != projectId) arrange.projectId = projectId;
    if (agent.folderId.isNotEmpty) arrange.folderId = '';
    return arrange;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    // A pipe's liaison has no row: its pipe's row speaks for it.
    final agents = [
      for (final agent in this.agents)
        if (agent.pipeId.isEmpty) agent,
    ];
    // The agents in the project's own folder (or one it no longer has),
    // then each folder's. Each is its own tree: an agent whose parent is in
    // another folder stands at its folder's top level.
    final folderIds = {for (final folder in folders) folder.id};
    final workspace = backend.workspace.workspace;
    // Its pipes to other Claktories: each group with its pipes, then the
    // pipes in none.
    final pipeRows = projectId.isEmpty
        ? (groups: const <(PipeGroup, List<Pipe>)>[], loose: const <Pipe>[])
        : pipeRowsIn(workspace, projectId);
    final selectedPipeId = AppScope.scope(context).page.value == AppPage.agent
        ? workspace.selectedPipeId
        : '';
    final roots = _tree([
      for (final agent in agents)
        if (!folderIds.contains(agent.folderId)) agent,
    ]);
    // Rows keep room for a disclosure once any agent here has others under
    // it, so the dots stay in a column.
    final gutter = agents.any(
      (agent) => agents.any((other) => other.parentId == agent.id),
    );

    void select(Agent agent, {bool reach = false}) =>
        _selectAgent(context, agent, reach: reach);
    final pinIds = [for (final agent in pinnedAgents(workspace)) agent.id];

    // Reports sit in under their lead, up to a few levels: deeper ones
    // line up with the last, so names keep their room.
    final cap = touchFirst(context) ? 3 : 4;
    // On a phone each level is a small step, its disclosure at the row's
    // end as iOS's outlines have it, so depth doesn't eat the left edge.
    final phone = isPhoneLayout(context);
    Widget branch(_Node node, List<_Node> siblings, [int depth = 0]) {
      final children = node.children;
      final open = children.isNotEmpty && !node.agent.collapsed;
      final inset = depth < cap ? (phone ? Space.m : Space.l) : 0.0;
      final agent = node.agent;
      final folderId = folderIds.contains(agent.folderId) ? agent.folderId : '';
      final siblingIds = [for (final sibling in siblings) sibling.agent.id];
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // The same row while nothing it shows changed: another agent's
          // step builds only that agent's row.
          Kept(
            inputs: [
              agent,
              // Folded, it speaks for the agents under it; open, it needs
              // only which they are.
              agent.collapsed
                  ? _all(children)
                  : [for (final child in children) child.agent.id],
              backend.workspace.agent(agent.leadId),
              workspace.selectedAgentId,
              projectId,
              folderId,
              selectedAgentId,
              gutter && !phone,
              inset,
              siblingIds,
              // Pin or Unpin, and what a folded row leaves out.
              pinIds,
            ],
            builder: (_) => _AgentRow(
              agent: agent,
              projectId: projectId,
              folderId: folderId,
              selected: agent.id == selectedAgentId,
              onTap: () => select(agent),
              onReach: () => select(agent, reach: true),
              gutter: gutter && !phone,
              children: children,
              siblings: siblingIds,
              inset: inset,
            ),
          ),
          if (children.isNotEmpty)
            AnimatedSize(
              duration: Motion.base,
              curve: Curves.easeOut,
              alignment: Alignment.topCenter,
              child: open
                  ? Padding(
                      padding: EdgeInsets.only(left: inset),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          for (final child in children)
                            branch(child, children, depth + 1),
                        ],
                      ),
                    )
                  : const SizedBox(width: double.infinity),
            ),
        ],
      );
    }

    // In the top group, a project's agent dropped in a gap is pinned first
    // under it rather than moved out of its project: only Rivet's row
    // itself takes it out (Lucy's pinned-agents.md §5).
    bool pins(Agent agent) =>
        projectId.isEmpty && pinLabel(workspace, agent) != null;
    return DragTarget<String>(
      // Takes what the rows don't: to this place's top level, last.
      onWillAcceptWithDetails: (details) {
        final agent = backend.workspace.agent(details.data);
        return agent != null &&
            (pins(agent) ||
                agent.projectId != projectId ||
                agent.parentId.isNotEmpty ||
                agent.folderId.isNotEmpty);
      },
      onMove: (details) {
        final agent = backend.workspace.agent(details.data);
        if (agent == null) return;
        _dropSays.value = pins(agent)
            ? _sayPin(backend.workspace.workspace, agent)
            : _sayDrop(backend.workspace.workspace, agent, _toPlace(agent));
      },
      onLeave: (_) => _dropSays.value = null,
      onAcceptWithDetails: (details) {
        _dropSays.value = null;
        final agent = backend.workspace.agent(details.data);
        if (agent == null) return;
        if (pins(agent)) {
          unawaited(
            pinAgentBefore(context, agent, _firstPin(workspace, agent)),
          );
        } else {
          _arrangeOrSay(context, agent, _toPlace(agent));
        }
      },
      builder: (context, candidates, _) => AnimatedContainer(
        duration: Motion.fast,
        curve: Curves.easeOut,
        decoration: BoxDecoration(
          color: candidates.isEmpty ? null : palette.hover,
          borderRadius: BorderRadius.circular(Radii.panel),
          border: Border.all(
            color: candidates.isEmpty
                ? const Color(0x00000000)
                : palette.signal,
          ),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            ?header,
            Padding(
              padding: EdgeInsets.only(left: indent),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  // Its pipes to other Claktories first, within reach
                  // however many agents and folders follow: its groups,
                  // each with its pipes, then the pipes in none.
                  // Keyed, so a row stays itself when one above it goes
                  // (a menu open on it keeps its place).
                  for (final (group, inIt) in pipeRows.groups)
                    _PipeGroupRow(
                      key: ValueKey('pipe-group-slot/${group.id}'),
                      group: group,
                      pipes: inIt,
                      selectedPipeId: selectedPipeId,
                    ),
                  for (final pipe in pipeRows.loose)
                    _PipeRow(
                      key: ValueKey('pipe-slot/${pipe.id}'),
                      pipe: pipe,
                      selected: pipe.id == selectedPipeId,
                      onTap: () => _selectPipe(context, pipe),
                    ),
                  for (final root in roots) branch(root, roots),
                  for (final folder in folders)
                    _FolderGroup(
                      projectId: projectId,
                      folder: folder,
                      order: [for (final folder in folders) folder.id],
                      roots: _tree([
                        for (final agent in agents)
                          if (agent.folderId == folder.id) agent,
                      ]),
                      branch: branch,
                    ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A project's folder being dragged to another place in its list.
@immutable
class _FolderDrag {
  const _FolderDrag(this.projectId, this.folderId);

  final String projectId;
  final String folderId;
}

/// One of a project's folders in the sidebar: a quiet heading that folds
/// its agents away, and its agents nested under it. Dropping an agent on
/// the heading puts it in the folder, at its top level. The heading drags
/// (a finger holds it first) to put the folder elsewhere in the project's
/// list; another folder dropped on its upper or lower half goes before or
/// after it.
class _FolderGroup extends StatefulWidget {
  const _FolderGroup({
    required this.projectId,
    required this.folder,
    required this.order,
    required this.roots,
    required this.branch,
  });

  final String projectId;
  final ProjectFolder folder;

  /// The project's folders' ids, in order, this one's included.
  final List<String> order;
  final List<_Node> roots;
  final Widget Function(_Node node, List<_Node> siblings) branch;

  @override
  State<_FolderGroup> createState() => _FolderGroupState();
}

class _FolderGroupState extends State<_FolderGroup> {
  /// Where the folder being dragged over this one would go.
  _Drop? _drop;

  _Drop _zone(Offset global) {
    final box = context.findRenderObject()! as RenderBox;
    return box.globalToLocal(global).dy < box.size.height / 2
        ? _Drop.before
        : _Drop.after;
  }

  /// The folder [dragged] goes before when dropped at [drop] here ("" for
  /// last); null when that's where it is already.
  String? _beforeId(String dragged, _Drop drop) {
    final order = widget.order;
    final others = [
      for (final id in order)
        if (id != dragged) id,
    ];
    final at = others.indexOf(widget.folder.id) + (drop == _Drop.after ? 1 : 0);
    final before = at < others.length ? others[at] : '';
    final now = order.indexOf(dragged);
    final current = now + 1 < order.length ? order[now + 1] : '';
    return before == current ? null : before;
  }

  /// Puts the folder [folderId] before [beforeId] ("" for last), saying
  /// why if the backend won't.
  void _move(String folderId, String beforeId) {
    final backend = AppScope.of(context);
    final name = [
      for (final folder
          in backend.workspace.workspace.projects
              .where((project) => project.id == widget.projectId)
              .expand((project) => project.folders))
        if (folder.id == folderId) folder.name,
    ].firstOrNull;
    backend
        .updateFolder(
          UpdateFolder(
            projectId: widget.projectId,
            folderId: folderId,
            beforeId: beforeId,
          ),
        )
        .catchError((Object error) {
          if (!mounted || error is! RequestFailure) return;
          unawaited(
            showModal(
              context,
              (context, close) => _Refused(
                title: "Couldn't move ${name ?? 'the folder'}",
                why: error.message,
                close: close,
              ),
            ),
          );
        }, test: (error) => error is RequestFailure);
  }

  /// Moving this folder one place up or down, for VoiceOver (a drag
  /// otherwise); null at that end of the list.
  VoidCallback? _step(int by) {
    final order = widget.order;
    final at = order.indexOf(widget.folder.id);
    final to = at + by;
    if (at < 0 || to < 0 || to >= order.length) return null;
    // Up: before the one above. Down: before the one after the next.
    final before = by < 0
        ? order[to]
        : (to + 1 < order.length ? order[to + 1] : '');
    return () => _move(widget.folder.id, before);
  }

  /// The request that puts [agent] at the top of this folder's agents,
  /// last, and from under its lead.
  ArrangeAgent _toFolder(Agent agent) {
    final arrange = ArrangeAgent(agentId: agent.id, folderId: widget.folder.id);
    if (agent.projectId != widget.projectId) {
      arrange.projectId = widget.projectId;
    }
    return arrange;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final folder = widget.folder;
    final roots = widget.roots;
    // Pinned agents folded away are left out while the pins are in sight.
    final pinned = _pinsStay(context)
        ? pinnedIds(backend.workspace.workspace)
        : const <String>{};
    final hidden = folder.collapsed
        ? [
            for (final agent in _all(roots))
              if (!pinned.contains(agent.id)) agent,
          ]
        : const <Agent>[];
    final working = hidden.any(atWork);
    final unseen = hidden.any((agent) => agent.unseen);
    final waiting = waitingIn(hidden);
    // The same heading while what it shows is: another agent's step
    // doesn't build it again.
    final heading = Kept(
      inputs: [folder, widget.projectId, widget.order, hidden, roots.isEmpty],
      builder: (context) {
        final headingRow = _heading(
          context,
          folder,
          working: working,
          unseen: unseen,
          waiting: waiting,
          hidden: hidden,
          roots: roots,
        );
        return DragTarget<String>(
          onWillAcceptWithDetails: (details) {
            final agent = backend.workspace.agent(details.data);
            return agent != null &&
                (agent.folderId != folder.id ||
                    agent.projectId != widget.projectId ||
                    agent.parentId.isNotEmpty);
          },
          onMove: (details) {
            final agent = backend.workspace.agent(details.data);
            if (agent == null) return;
            _dropSays.value = _sayDrop(
              backend.workspace.workspace,
              agent,
              _toFolder(agent),
            );
          },
          onLeave: (_) => _dropSays.value = null,
          onAcceptWithDetails: (details) {
            _dropSays.value = null;
            final agent = backend.workspace.agent(details.data);
            if (agent != null) _arrangeOrSay(context, agent, _toFolder(agent));
          },
          builder: (context, candidates, _) => AnimatedContainer(
            duration: Motion.fast,
            curve: Curves.easeOut,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(Radii.control),
              border: Border.all(
                color: candidates.isEmpty
                    ? const Color(0x00000000)
                    : palette.signal,
              ),
            ),
            child: _SidebarDraggable<_FolderDrag>(
              data: _FolderDrag(widget.projectId, folder.id),
              dragAnchorStrategy: pointerDragAnchorStrategy,
              feedback: _DragFeedback(name: folder.name, palette: palette),
              childWhenDragging: Opacity(opacity: 0.4, child: headingRow),
              child: headingRow,
            ),
          ),
        );
      },
    );
    final hiddenIds = {for (final agent in hidden) agent.id};
    final shown = backend.workspace.workspace.selectedAgentId;
    final group = Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Glow(
          store: backend.highlights,
          matches: (key) =>
              key == 'folder:${widget.projectId}/${folder.id}' ||
              matchesAgents(key, hiddenIds, shownAgentId: shown),
          child: heading,
        ),
        AnimatedSize(
          duration: Motion.base,
          curve: Curves.easeOut,
          alignment: Alignment.topCenter,
          child: folder.collapsed
              ? const SizedBox(width: double.infinity)
              : Padding(
                  padding: EdgeInsets.only(
                    left: isPhoneLayout(context) ? Space.s : Space.m + 2,
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      for (final root in roots) widget.branch(root, roots),
                    ],
                  ),
                ),
        ),
      ],
    );
    bool ours(_FolderDrag drag) =>
        drag.projectId == widget.projectId &&
        widget.order.contains(drag.folderId);
    return DragTarget<_FolderDrag>(
      onWillAcceptWithDetails: (details) => ours(details.data),
      onMove: (details) {
        final drop = details.data.folderId == folder.id
            ? null
            : _zone(details.offset);
        if (drop != _drop) setState(() => _drop = drop);
      },
      onLeave: (_) => setState(() => _drop = null),
      onAcceptWithDetails: (details) {
        final dragged = details.data.folderId;
        final drop = dragged == folder.id
            ? null
            : _drop ?? _zone(details.offset);
        setState(() => _drop = null);
        if (drop == null) return;
        if (_beforeId(dragged, drop) case final before?) _move(dragged, before);
      },
      builder: (context, _, _) => Stack(
        clipBehavior: Clip.none,
        children: [
          group,
          // Between folders: a signal line where it would go.
          if (_drop != null)
            Positioned(
              left: 0,
              right: 0,
              top: _drop == _Drop.before ? -1 : null,
              bottom: _drop == _Drop.after ? -1 : null,
              child: Container(
                height: 2,
                decoration: BoxDecoration(
                  color: palette.signal,
                  borderRadius: BorderRadius.circular(1),
                ),
              ),
            ),
        ],
      ),
    );
  }

  Widget _heading(
    BuildContext context,
    ProjectFolder folder, {
    required bool working,
    required bool unseen,
    required int waiting,
    required List<Agent> hidden,
    required List<_Node> roots,
  }) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return Semantics(
      button: true,
      label: folder.collapsed
          ? 'Show agents in ${folder.name}'
          : 'Hide agents in ${folder.name}',
      customSemanticsActions: {
        const CustomSemanticsAction(label: 'Move up'): ?_step(-1),
        const CustomSemanticsAction(label: 'Move down'): ?_step(1),
      },
      child: _Row(
        // Folded, it speaks for its agents: a quiet wave while one
        // works, as an agent's row has.
        underlay: working
            ? WorkingWave(
                color: palette.text,
                peak: 0.07,
                radius: Radii.control - 1,
              )
            : null,
        onTap: () => fireAndLog(
          backend.setFolderCollapsed(
            widget.projectId,
            folder.id,
            !folder.collapsed,
          ),
        ),
        builder: (context, hovered) {
          final color = hovered ? palette.textMuted : palette.textFaint;
          return Row(
            children: [
              SizedBox(
                width: Space.m + 2,
                child: Align(
                  alignment: Alignment.centerLeft,
                  child: AnimatedRotation(
                    turns: folder.collapsed ? 0 : 0.25,
                    duration: Motion.fast,
                    curve: Curves.easeOut,
                    child: Chevron(color: color, down: false),
                  ),
                ),
              ),
              Expanded(
                child: Text(
                  folder.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TypeScale.of(context).caption.copyWith(
                    color: unseen || waiting > 0 ? palette.text : color,
                    fontWeight: unseen || waiting > 0
                        ? FontWeight.w600
                        : FontWeight.w500,
                  ),
                ),
              ),
              if (folder.collapsed && roots.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(left: Space.s),
                  child: Text(
                    '${hidden.length}',
                    style: TypeScale.of(context).caption
                        .copyWith(color: palette.textFaint),
                  ),
                ),
              if (waiting > 0)
                _needsYou(waitingLabel(folder.name, waiting), waiting),
            ],
          );
        },
      ),
    );
  }
}

/// An agent and the agents nested under it.
class _Node {
  _Node(this.agent);

  final Agent agent;
  final children = <_Node>[];
}

/// [agents] as a tree, each under its parent and in their order. An agent
/// whose parent isn't among them is at the top level, and so is one caught
/// in a loop, so every agent shows.
List<_Node> _tree(List<Agent> agents) {
  final nodes = {for (final agent in agents) agent.id: _Node(agent)};
  final parentOf = <String, _Node>{};
  for (final agent in agents) {
    final parent = nodes[agent.parentId];
    if (parent != null && agent.parentId != agent.id) {
      parentOf[agent.id] = parent;
    }
  }
  // Break loops: walk up from each agent; one that meets itself is cut
  // loose from its parent.
  for (final agent in agents) {
    final seen = <String>{agent.id};
    var at = parentOf[agent.id];
    while (at != null) {
      if (!seen.add(at.agent.id)) {
        parentOf.remove(agent.id);
        break;
      }
      at = parentOf[at.agent.id];
    }
  }
  final roots = <_Node>[];
  for (final agent in agents) {
    final node = nodes[agent.id]!;
    final parent = parentOf[agent.id];
    if (parent == null) {
      roots.add(node);
    } else {
      parent.children.add(node);
    }
  }
  return roots;
}

/// The ids of every agent nested under [agentId], however deep.
Set<String> _descendants(List<Agent> agents, String agentId) {
  final found = <String>{};
  var frontier = {agentId};
  while (frontier.isNotEmpty) {
    final next = <String>{};
    for (final agent in agents) {
      if (frontier.contains(agent.parentId) && found.add(agent.id)) {
        next.add(agent.id);
      }
    }
    frontier = next;
  }
  return found;
}

/// A focusable sidebar row: hover tint, signal focus ring, Enter activates.
class _Row extends StatefulWidget {
  const _Row({
    super.key,
    required this.onTap,
    required this.builder,
    this.selected = false,
    this.onSecondaryTap,
    this.onMenuKey,
    this.underlay,
    this.height = 28,
    this.holdForMenu = true,
  });

  final VoidCallback onTap;

  /// A finger's touch and hold is [onSecondaryTap], unless the row drags
  /// on a hold instead (a project's).
  final bool holdForMenu;

  /// Agents' rows are taller, for their characters.
  final double height;

  /// Behind its content, across the whole row, such as a [WorkingWave].
  final Widget? underlay;

  /// A right click.
  final VoidCallback? onSecondaryTap;

  /// The menu from the keyboard, the row focused: Shift-F10 or the menu
  /// key, where VoiceOver on a Mac reaches it (Abbacchio's row 1661); else
  /// [onSecondaryTap].
  final VoidCallback? onMenuKey;
  final bool selected;
  final Widget Function(BuildContext context, bool hovered) builder;

  @override
  State<_Row> createState() => _RowState();
}

/// Opens a row's menu from the keyboard.
class _MenuIntent extends Intent {
  const _MenuIntent();
}

class _RowState extends State<_Row> {
  var _hovered = false;
  var _focused = false;
  final _focus = FocusNode(debugLabel: 'sidebar row');

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = touchFirst(context);
    return FocusableActionDetector(
      focusNode: _focus,
      mouseCursor: SystemMouseCursors.click,
      // Not while an agent is dragged over it: only where it can land
      // says so (Abbacchio's pinned-agents NIT).
      onShowHoverHighlight: (value) =>
          setState(() => _hovered = value && !sidebarDragging.value),
      onShowFocusHighlight: (value) => setState(() => _focused = value),
      shortcuts: {
        if ((widget.onMenuKey ?? widget.onSecondaryTap) != null) ...{
          const SingleActivator(LogicalKeyboardKey.f10, shift: true):
              const _MenuIntent(),
          const SingleActivator(LogicalKeyboardKey.contextMenu):
              const _MenuIntent(),
        },
      },
      actions: {
        ActivateIntent: CallbackAction<ActivateIntent>(
          onInvoke: (_) {
            widget.onTap();
            return null;
          },
        ),
        _MenuIntent: CallbackAction<_MenuIntent>(
          onInvoke: (_) {
            (widget.onMenuKey ?? widget.onSecondaryTap)?.call();
            return null;
          },
        ),
      },
      // VoiceOver on a Mac moving the keyboard's focus to its cursor
      // (VO-Command-F4, or keyboard focus following it) tells the app only
      // that the row gained VoiceOver's focus: the row takes the
      // keyboard's then, so Shift-F10 opens its menu (Abbacchio's row
      // 1716).
      child: Semantics(
        onDidGainAccessibilityFocus:
            defaultTargetPlatform == TargetPlatform.macOS
            ? _focus.requestFocus
            : null,
        child: GestureDetector(
          onTap: widget.onTap,
          onSecondaryTap: widget.onSecondaryTap,
          // A phone's right click.
          onLongPress: widget.holdForMenu ? widget.onSecondaryTap : null,
          child: AnimatedContainer(
            duration: Motion.fast,
            curve: Curves.easeOut,
            // On a phone, a finger's size at least, growing with the text.
            height: phone ? null : widget.height,
            constraints: phone
                ? BoxConstraints(
                    minHeight: PhonePlatformPolicy.least(
                      context,
                      DenseList.of(context)
                          ? (widget.height > 28
                                ? Touch.denseRow
                                : Touch.denseLine)
                          : (widget.height > 28 ? Touch.row : Touch.target),
                    ),
                  )
                : null,
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            decoration: BoxDecoration(
              color: widget.selected
                  ? palette.selected
                  : _hovered
                  ? palette.hover
                  : const Color(0x00000000),
              borderRadius: BorderRadius.circular(Radii.control),
              border: Border.all(
                color: _focused ? palette.signal : const Color(0x00000000),
              ),
            ),
            child: widget.underlay == null
                ? widget.builder(context, _hovered)
                : Stack(
                    clipBehavior: Clip.none,
                    // A phone's row grows: its content stays in the middle.
                    alignment: phone
                        ? AlignmentDirectional.centerStart
                        : AlignmentDirectional.topStart,
                    children: [
                      // Out to the row's edges, through its padding.
                      Positioned(
                        left: -Space.s,
                        right: -Space.s,
                        top: 0,
                        bottom: 0,
                        child: widget.underlay!,
                      ),
                      widget.builder(context, _hovered),
                    ],
                  ),
          ),
        ),
      ),
    );
  }
}

void _openProjectSettings(BuildContext context, Project project) => showModal(
  context,
  (context, close) => ProjectSettingsForm(project: project, close: close),
);

/// A project's right click: its settings. With a finger, its More button
/// or a touch and hold: its actions (Files, Add an agent, Project
/// settings), in a sheet on a phone, in a popover by the project on an
/// iPad.
void _projectMenu(BuildContext context, Project project) {
  if (!touchFirst(context)) return _openProjectSettings(context, project);
  unawaited(_projectActions(context, project));
}

Future<void> _projectActions(BuildContext context, Project project) async {
  // In a drawer, the phone's Files page; on an iPad's wide layouts, a pane
  // beside the conversation, as the row's Files button opens (#540).
  final files = PhoneProjects.of(context)?.files;
  final chosen = await showActions<String>(
    context,
    title: project.name,
    actions: [
      SheetAction(
        'Files',
        'files',
        glyph: (color) => FolderGlyph(color: color),
      ),
      SheetAction(
        'Add an agent',
        'agent',
        glyph: (color) => ExtensionGlyph(icon: 'plus', name: '', color: color),
      ),
      SheetAction(
        'Project settings',
        'settings',
        glyph: (color) => CogGlyph(color: color),
      ),
    ],
  );
  if (!context.mounted) return;
  switch (chosen) {
    case 'files':
      openFiles(context, project, phone: files);
    case 'agent':
      _askForAgent(project)._ask(context);
    case 'settings':
      _openProjectSettings(context, project);
  }
}

/// A project's More on a phone, at its row's end: its actions.
class _ProjectMore extends StatelessWidget {
  const _ProjectMore({required this.project});

  final Project project;

  @override
  Widget build(BuildContext context) => HeaderButton.more(
    inRow: true,
    label: '${project.name} options',
    onTap: () => _projectMenu(context, project),
  );
}

/// The button on a project's row that opens its Files in a pane beside the
/// conversation, shown on hover, and kept tinted while the pane is open.
class _FilesButton extends StatelessWidget {
  const _FilesButton({required this.project, required this.shown});

  final Project project;
  final bool shown;

  static const _tip = 'Show files';

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return _FingerTarget(
      // The gap after it, when it ends the row.
      after: Space.xs,
      child: HoverCard(
        card: (context) => const Text(_tip),
        child: Semantics(
          button: true,
          label: _tip,
          child: Pressable(
            onTap: () => openFiles(
              context,
              project,
              phone: PhoneProjects.of(context)?.files,
            ),
            builder: (context, hovered) => AnimatedContainer(
              duration: Motion.fast,
              curve: Curves.easeOut,
              width: 20,
              height: 20,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: hovered ? palette.hover : const Color(0x00000000),
                borderRadius: BorderRadius.circular(Radii.control),
              ),
              child: Transform.scale(
                scale: 0.875,
                child: FileGlyph(
                  color: shown || hovered
                      ? palette.textMuted
                      : palette.textFaint,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A small button in a row (20 points square) that a finger can hit: with
/// touch, a tap within a finger's target of it ([Touch.target] wide, ending
/// [after] points past it, and as tall as its row lets it be) counts as on
/// it (#540, Sale). It looks and lays out as before; with a pointer alone,
/// only the button itself.
class _FingerTarget extends SingleChildRenderObjectWidget {
  const _FingerTarget({required this.after, required super.child});

  /// The room after the button within the [Row] it's in: a tap past the
  /// row's own end never reaches it (the padding around the row doesn't
  /// count), so the target reaches back before it instead.
  final double after;

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderFingerTarget(after: after, touch: touchFirst(context));

  @override
  void updateRenderObject(BuildContext context, RenderObject renderObject) {
    (renderObject as _RenderFingerTarget)
      ..after = after
      ..touch = touchFirst(context);
  }
}

class _RenderFingerTarget extends RenderProxyBox {
  _RenderFingerTarget({required this.after, required this.touch});

  double after;
  bool touch;

  @override
  bool hitTest(BoxHitTestResult result, {required Offset position}) {
    final child = this.child;
    if (!touch || child == null) {
      return super.hitTest(result, position: position);
    }
    final side = math.min(
      after,
      math.max(0.0, (Touch.target - size.width) / 2),
    );
    final dy = math.max(0.0, (Touch.target - size.height) / 2);
    final area = Rect.fromLTRB(
      math.min(0, size.width + side - Touch.target),
      -dy,
      size.width + side,
      size.height + dy,
    );
    if (!area.contains(position)) return false;
    // Anywhere in it is the button's middle.
    final middle = size.center(Offset.zero);
    if (!child.hitTest(result, position: middle)) return false;
    result.add(BoxHitTestEntry(this, position));
    return true;
  }
}

/// A project: a folder glyph and its name, and on hover a Files button and
/// a "+" that asks Rivet for a new agent in it. A click focuses it; a right
/// click opens its settings (rename, delete). A finger's touch and hold
/// picks it up to move ([_ProjectSlot]); let go without moving, it's the
/// project's menu as before.
class _ProjectRow extends StatelessWidget {
  const _ProjectRow({required this.project});

  final Project project;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return _glowFor(context, project, _row(context, palette));
  }

  Widget _row(BuildContext context, Palette palette) {
    return _Row(
      onTap: () {
        AppScope.scope(context).history.clear();
        fireAndLog(AppScope.of(context).focusProject(project.id));
      },
      onSecondaryTap: () => _projectMenu(context, project),
      holdForMenu: false,
      builder: (context, hovered) => Row(
        children: [
          SizedBox(
            width: Space.l + 2,
            child: Align(
              alignment: Alignment.centerLeft,
              child: FolderGlyph(
                color: hovered ? palette.text : palette.textMuted,
              ),
            ),
          ),
          Expanded(
            child: Text(
              project.name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TypeScale.of(context).body
                  .copyWith(color: hovered ? palette.text : palette.textMuted),
            ),
          ),
          // A phone has no hover: its actions are behind More.
          if (isPhoneLayout(context))
            _ProjectMore(project: project)
          else ...[
            if (hovered || filesShown(context, project.id)) ...[
              _FilesButton(
                project: project,
                shown: filesShown(context, project.id),
              ),
              const SizedBox(width: Space.xs),
            ],
            if (hovered) _askForAgent(project),
          ],
        ],
      ),
    );
  }
}

/// A project being dragged to another place in the projects list.
@immutable
class _ProjectDrag {
  const _ProjectDrag(this.projectId);

  final String projectId;
}

/// A project in the list, its row and its agents, that moves as folder
/// headings do (friend bug #12): its row drags (a finger holds it first),
/// and another project dropped on this one's upper or lower half goes
/// before or after it, a signal line showing where. VoiceOver has Move up
/// and Move down. The order is the backend's projects list, archived
/// projects keeping their place in it.
class _ProjectSlot extends StatefulWidget {
  const _ProjectSlot({
    required this.project,
    required this.order,
    required this.shown,
    required this.place,
  });

  final Project project;

  /// Every project's id, in order, archived ones too.
  final List<String> order;

  /// The projects listed here, in order: those not archived.
  final List<String> shown;

  /// The project's place, with [header] as its row.
  final Widget Function(Widget header) place;

  @override
  State<_ProjectSlot> createState() => _ProjectSlotState();
}

class _ProjectSlotState extends State<_ProjectSlot> {
  /// Where the project being dragged over this one would go.
  _Drop? _drop;

  /// Where and with what the row was last pressed: a finger that holds and
  /// lets go without moving opens the project's menu.
  Offset? _downAt;
  PointerDeviceKind? _downKind;

  _Drop _zone(Offset global) {
    final box = context.findRenderObject()! as RenderBox;
    return box.globalToLocal(global).dy < box.size.height / 2
        ? _Drop.before
        : _Drop.after;
  }

  /// The project [dragged] goes before when dropped at [drop] on [target]
  /// ("" for last); null when that's where it is already.
  String? _beforeId(String dragged, String target, _Drop drop) {
    final order = widget.order;
    final others = [
      for (final id in order)
        if (id != dragged) id,
    ];
    final at = others.indexOf(target) + (drop == _Drop.after ? 1 : 0);
    final before = at < others.length ? others[at] : '';
    final now = order.indexOf(dragged);
    final current = now + 1 < order.length ? order[now + 1] : '';
    return before == current ? null : before;
  }

  /// Puts the project [projectId] before [beforeId] ("" for last), saying
  /// why if the backend won't.
  void _move(String projectId, String beforeId) {
    final backend = AppScope.of(context);
    final name = [
      for (final project in backend.workspace.workspace.projects)
        if (project.id == projectId) project.name,
    ].firstOrNull;
    backend
        .updateProject(UpdateProject(projectId: projectId, beforeId: beforeId))
        .catchError((Object error) {
          if (!mounted || error is! RequestFailure) return;
          unawaited(
            showModal(
              context,
              (context, close) => _Refused(
                title: "Couldn't move ${name ?? 'the project'}",
                why: error.message,
                close: close,
              ),
            ),
          );
        }, test: (error) => error is RequestFailure);
  }

  /// Moving this project one place up or down among those listed, for
  /// VoiceOver (a drag otherwise); null at that end of the list.
  VoidCallback? _step(int by) {
    final id = widget.project.id;
    final shown = widget.shown;
    final at = shown.indexOf(id);
    final to = at + by;
    if (at < 0 || to < 0 || to >= shown.length) return null;
    final before = _beforeId(
      id,
      shown[to],
      by < 0 ? _Drop.before : _Drop.after,
    );
    if (before == null) return null;
    return () => _move(id, before);
  }

  /// A finger held the row and let go where it was: the project's menu,
  /// as a touch and hold was before it could move.
  void _ended(DraggableDetails details) {
    final at = _downAt;
    final touch =
        _downKind == PointerDeviceKind.touch ||
        _downKind == PointerDeviceKind.stylus;
    if (details.wasAccepted || !touch || at == null) return;
    if ((details.offset - at).distance > kTouchSlop) return;
    _projectMenu(context, widget.project);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final project = widget.project;
    // The same row while the project and its Files button are.
    final row = Kept(
      inputs: [project, filesShown(context, project.id)],
      builder: (_) => _ProjectRow(project: project),
    );
    final header = Semantics(
      customSemanticsActions: {
        const CustomSemanticsAction(label: 'Move up'): ?_step(-1),
        const CustomSemanticsAction(label: 'Move down'): ?_step(1),
      },
      child: Listener(
        onPointerDown: (event) {
          _downAt = event.position;
          _downKind = event.kind;
        },
        child: _SidebarDraggable<_ProjectDrag>(
          data: _ProjectDrag(project.id),
          dragAnchorStrategy: pointerDragAnchorStrategy,
          onDragEnd: _ended,
          feedback: _DragFeedback(name: project.name, palette: palette),
          childWhenDragging: Opacity(opacity: 0.4, child: row),
          child: row,
        ),
      ),
    );
    return DragTarget<_ProjectDrag>(
      onWillAcceptWithDetails: (details) =>
          details.data.projectId != project.id &&
          widget.shown.contains(details.data.projectId),
      onMove: (details) {
        final drop = _zone(details.offset);
        if (drop != _drop) setState(() => _drop = drop);
      },
      onLeave: (_) => setState(() => _drop = null),
      onAcceptWithDetails: (details) {
        final dragged = details.data.projectId;
        final drop = _drop ?? _zone(details.offset);
        setState(() => _drop = null);
        if (_beforeId(dragged, project.id, drop) case final before?) {
          _move(dragged, before);
        }
      },
      builder: (context, _, _) => Stack(
        clipBehavior: Clip.none,
        children: [
          widget.place(header),
          // Between projects: a signal line where it would go.
          if (_drop != null)
            Positioned(
              left: 0,
              right: 0,
              top: _drop == _Drop.before ? -1 : null,
              bottom: _drop == _Drop.after ? -1 : null,
              child: Container(
                height: 2,
                decoration: BoxDecoration(
                  color: palette.signal,
                  borderRadius: BorderRadius.circular(1),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// A project's row or heading, glowing when the project is highlighted.
Widget _glowFor(BuildContext context, Project project, Widget row) => Glow(
  store: AppScope.of(context).highlights,
  matches: (key) => key == 'project:${project.id}',
  child: row,
);

/// The "+" that asks Rivet for a new agent in [project].
_AskRivet _askForAgent(Project project) => _AskRivet(
  starter: 'Add an agent to ${project.name} that ',
  fallback: (context, close) =>
      CreateAgentForm(projectId: project.id, close: close),
);

/// The focused project's name, heading the sidebar, with a Files button on
/// hover (always, with a finger). A click or right click opens its settings (rename, delete); on a
/// phone, a menu with its Files too.
class _ProjectHeading extends StatelessWidget {
  const _ProjectHeading({required this.project});

  final Project project;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    void open() => _projectMenu(context, project);
    return _glowFor(context, project, _row(palette, open));
  }

  Widget _row(Palette palette, VoidCallback open) {
    return _Row(
      onTap: open,
      onSecondaryTap: open,
      builder: (context, hovered) => Row(
        children: [
          SizedBox(
            width: Space.l + 2,
            child: Align(
              alignment: Alignment.centerLeft,
              child: FolderGlyph(color: palette.text),
            ),
          ),
          Expanded(
            child: Text(
              project.name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TypeScale.of(context).label.copyWith(color: palette.text),
            ),
          ),
          if (isPhoneLayout(context))
            _ProjectMore(project: project)
          else ...[
            // A finger has no hover: the focused project keeps its Files
            // button in sight (#540).
            if (hovered ||
                touchFirst(context) ||
                filesShown(context, project.id)) ...[
              _FilesButton(
                project: project,
                shown: filesShown(context, project.id),
              ),
              const SizedBox(width: Space.xs),
            ],
            if (hovered) ...[
              _askForAgent(project),
              const SizedBox(width: Space.xs),
              MoreGlyph(color: palette.textMuted),
            ],
          ],
        ],
      ),
    );
  }
}

/// Where a dragged agent would land on a row: before it, nested in it, or
/// after it.
enum _Drop { before, onto, after }

/// An agent's row: its dot, name, queued count and unseen marker, and a
/// disclosure when agents are nested under it. Drag it to move it; drop
/// another agent on its upper or lower edge to place that one before or
/// after it, or on its middle to nest it here.
class _AgentRow extends StatefulWidget {
  const _AgentRow({
    required this.agent,
    required this.projectId,
    required this.folderId,
    required this.selected,
    required this.onTap,
    required this.onReach,
    required this.gutter,
    required this.children,
    required this.siblings,
    this.inset = Space.l,
    this.pinOrder,
  });

  /// Set on a pin under Rivet: the pins' ids in order. Its second line is
  /// its role and project, it has no disclosure, and it drags only among
  /// the pins.
  final List<String>? pinOrder;

  final Agent agent;

  /// How far in its reports sit (none past the deepest level shown).
  final double inset;

  /// The place it's in: a project's id, or empty for the workspace.
  final String projectId;

  /// The project's folder it shows in; empty for the project's own.
  final String folderId;
  final bool selected;
  final VoidCallback onTap;

  /// Its needs-you mark was clicked: opens its conversation at the oldest
  /// card waiting, however far back.
  final VoidCallback onReach;

  /// Keep room for the disclosure even without one.
  final bool gutter;

  /// The agents nested right under it.
  final List<_Node> children;

  /// Its siblings' ids in order, itself included.
  final List<String> siblings;

  @override
  State<_AgentRow> createState() => _AgentRowState();
}

class _AgentRowState extends State<_AgentRow> {
  /// Where the agent being dragged over this row would go.
  _Drop? _drop;

  /// Where and with what the row was last pressed: a right click's menu
  /// opens there, and a finger that holds and lets go without moving
  /// opens the row's menu.
  Offset? _downAt;
  PointerDeviceKind? _downKind;

  /// This row is the one being dragged.
  var _dragging = false;

  /// Scrolls the list the row is in while a drag holds near its edges.
  EdgeDraggingAutoScroller? _scroller;

  Agent get agent => widget.agent;

  bool get _pin => widget.pinOrder != null;

  bool get _open => widget.children.isNotEmpty && !agent.collapsed;

  _Drop _zone(Offset global) {
    final box = context.findRenderObject()! as RenderBox;
    final y = box.globalToLocal(global).dy / box.size.height;
    // Between pins only: nothing nests in a pin.
    if (_pin) return y < 0.5 ? _Drop.before : _Drop.after;
    if (y < 0.28) return _Drop.before;
    if (y > 0.72) return _Drop.after;
    return _Drop.onto;
  }

  /// The request that places [dragged] at [drop].
  ArrangeAgent _arrange(Agent dragged, _Drop drop) {
    final arrange = ArrangeAgent(agentId: dragged.id);
    switch (drop) {
      case _Drop.before:
        arrange
          ..parentId = agent.parentId
          ..beforeId = agent.id;
      case _Drop.onto:
        arrange.parentId = agent.id;
      case _Drop.after when _open:
        // Right under an open parent is its first child's place.
        final first = widget.children
            .map((child) => child.agent.id)
            .firstWhere((id) => id != dragged.id, orElse: () => '');
        arrange
          ..parentId = agent.id
          ..beforeId = first;
      case _Drop.after:
        final siblings = [
          for (final id in widget.siblings)
            if (id != dragged.id) id,
        ];
        final index = siblings.indexOf(agent.id);
        arrange
          ..parentId = agent.parentId
          ..beforeId = index + 1 < siblings.length ? siblings[index + 1] : '';
    }
    if (dragged.projectId != widget.projectId) {
      arrange.projectId = widget.projectId;
    }
    // Landing among this folder's agents puts it in the folder.
    if (dragged.folderId != widget.folderId) {
      arrange.folderId = widget.folderId;
    }
    return arrange;
  }

  /// Whether [dragged] may land here: not on itself, nor anywhere in what's
  /// nested under it.
  bool _accepts(String dragged) =>
      !_isOwn(dragged) && AppScope.of(context).workspace.agent(dragged) != null;

  /// This row is [dragged] itself or nested under it. Dropping here does
  /// nothing; the row still takes the drop, so it can't fall through to
  /// the place and move the agent out of where it is.
  bool _isOwn(String dragged) =>
      dragged == agent.id ||
      _descendants(
        AppScope.of(context).workspace.workspace.agents,
        dragged,
      ).contains(agent.id);

  /// The pin [dragged] goes before when dropped at [drop] on this pin
  /// ("" for last); null when that's where it is already.
  String? _pinBefore(String dragged, _Drop drop) {
    // Let go over itself: where it is.
    if (dragged == agent.id) return null;
    final order = widget.pinOrder!;
    final others = [
      for (final id in order)
        if (id != dragged) id,
    ];
    final at = others.indexOf(agent.id) + (drop == _Drop.after ? 1 : 0);
    final before = at < others.length ? others[at] : '';
    final now = order.indexOf(dragged);
    if (now < 0) return before;
    final current = now + 1 < order.length ? order[now + 1] : '';
    return before == current ? null : before;
  }

  /// Whether dropping [dragged] at [drop] here pins it: on a pin, or on
  /// the edge of a workspace agent's row (a gap in the top group). Onto
  /// Rivet's row is still the move to the workspace.
  bool _pins(Agent dragged, _Drop drop) {
    if (_pin) return true;
    return widget.projectId.isEmpty &&
        drop != _Drop.onto &&
        pinLabel(AppScope.of(context).workspace.workspace, dragged) != null;
  }

  /// What dropping [data] at [drop] does, for its feedback.
  String? _says(Object data, _Drop drop) {
    final backend = AppScope.of(context);
    if (data is _PinDrag) return null;
    final dragged = backend.workspace.agent(data as String);
    if (dragged == null) return null;
    if (_pins(dragged, drop)) {
      return _sayPin(backend.workspace.workspace, dragged);
    }
    return _sayDrop(
      backend.workspace.workspace,
      dragged,
      _arrange(dragged, drop),
    );
  }

  /// Whether this row takes [data]: a pin takes the pins (reordering) and
  /// any agent that can be pinned; any other row takes agents, as before.
  bool _takes(Object? data) {
    if (data is _PinDrag) return _pin;
    if (data is! String) return false;
    if (!_pin) return _accepts(data) || _isOwn(data);
    final dragged = AppScope.of(context).workspace.agent(data);
    return dragged != null &&
        pinLabel(AppScope.of(context).workspace.workspace, dragged) != null;
  }

  void _dropped(Object data, _Drop drop) {
    final backend = AppScope.of(context);
    final id = data is _PinDrag ? data.agentId : data as String;
    final dragged = backend.workspace.agent(id);
    if (dragged == null) return;
    if (_pin) {
      if (_pinBefore(id, drop) case final before?) {
        unawaited(pinAgentBefore(context, dragged, before));
      }
      return;
    }
    if (_isOwn(id)) return;
    if (_pins(dragged, drop)) {
      unawaited(
        pinAgentBefore(
          context,
          dragged,
          _firstPin(backend.workspace.workspace, dragged),
        ),
      );
      return;
    }
    _arrangeOrSay(context, dragged, _arrange(dragged, drop));
  }

  void _dragStarted() {
    _dragging = true;
    _touchDrag =
        _downKind == PointerDeviceKind.touch ||
        _downKind == PointerDeviceKind.stylus;
    sidebarDragging.value = true;
  }

  void _dragEnded(DraggableDetails details) {
    _dragDone();
    // A finger held the row and let go where it was: the row's menu.
    final at = _downAt;
    if (!_touchDrag || at == null) return;
    if ((details.offset - at).distance > kTouchSlop) return;
    unawaited(_agentMenu(context, agent, pinRow: _pin));
  }

  void _dragDone() {
    if (!_dragging) return;
    _dragging = false;
    _dropSays.value = null;
    sidebarDragging.value = false;
    _scroller?.stopAutoScroll();
    _scroller = null;
  }

  /// While a drag holds within [_edge] of the top or bottom of the list
  /// this row is in, the list scrolls that way, so the pins under Rivet
  /// can be reached from a project far down. Over anything outside the
  /// list (the top group above it, on a Mac) it doesn't.
  void _autoScroll(DragUpdateDetails details) {
    final scrollable = Scrollable.maybeOf(context);
    final box = scrollable?.context.findRenderObject();
    if (scrollable == null || box is! RenderBox || !box.hasSize) return;
    final view = box.localToGlobal(Offset.zero) & box.size;
    if (!view.contains(details.globalPosition)) {
      _scroller?.stopAutoScroll();
      return;
    }
    _scroller ??= EdgeDraggingAutoScroller(scrollable, velocityScalar: 20);
    _scroller!.startAutoScrollIfNecessary(
      Rect.fromCenter(
        center: details.globalPosition,
        width: 1,
        height: _edge * 2,
      ),
    );
  }

  static const _edge = 32.0;

  @override
  void dispose() {
    // Scrolled away mid-drag: what the drag set doesn't stay set.
    _dragDone();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final workspace = backend.workspace.workspace;
    // A pin, just pinned from a menu, comes into view.
    if (_pin && justPinned.value == agent.id) {
      justPinned.value = null;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) {
          unawaited(
            Scrollable.ensureVisible(
              context,
              duration: Motion.base,
              alignmentPolicy: ScrollPositionAlignmentPolicy.keepVisibleAtEnd,
            ),
          );
        }
      });
    }
    // It glows for itself, and for what's folded under it (the pins left
    // out while they're in sight).
    final pinned = _pinsStay(context) ? pinnedIds(workspace) : const <String>{};
    final stands = {
      agent.id,
      if (agent.collapsed)
        for (final hidden in _all(widget.children))
          if (!pinned.contains(hidden.id)) hidden.id,
    };
    final shown = workspace.selectedAgentId;
    final row = Glow(
      store: backend.highlights,
      matches: (key) =>
          (_pin && key == 'pin:${agent.id}') ||
          matchesAgents(key, stands, shownAgentId: shown),
      child: _row(palette, backend),
    );
    final draggable = _SidebarDraggable<Object>(
      data: _pin ? _PinDrag(agent.id) : agent.id,
      dragAnchorStrategy: pointerDragAnchorStrategy,
      onDragStarted: _dragStarted,
      onDragUpdate: _autoScroll,
      onDragEnd: _dragEnded,
      feedback: _DragFeedback(name: agent.name, palette: palette),
      childWhenDragging: Opacity(opacity: 0.4, child: row),
      child: row,
    );
    final target = DragTarget<Object>(
      onWillAcceptWithDetails: (details) => _takes(details.data),
      onMove: (details) {
        final data = details.data;
        // Another kind of drag (a project's), or a pin over a tree row.
        if (!_takes(data)) return;
        if (data is String && !_pin && !_accepts(data)) {
          _dropSays.value = null;
          return;
        }
        final drop = _zone(details.offset);
        _dropSays.value = _says(data, drop);
        if (drop != _drop) {
          // By touch, a tick each time the slot changes.
          if (_touchDrag && (_pin || _drop != null)) {
            unawaited(HapticFeedback.selectionClick());
          }
          setState(() => _drop = drop);
        }
      },
      onLeave: (data) {
        if (!_takes(data)) return;
        _dropSays.value = null;
        setState(() => _drop = null);
      },
      onAcceptWithDetails: (details) {
        _dropSays.value = null;
        final drop = _drop ?? _zone(details.offset);
        setState(() => _drop = null);
        _dropped(details.data, drop);
      },
      builder: (context, _, _) => Stack(
        clipBehavior: Clip.none,
        children: [
          // Nesting: the row is ringed, like a place taking a drop.
          AnimatedContainer(
            duration: Motion.fast,
            curve: Curves.easeOut,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(Radii.control),
              border: Border.all(
                color: _drop == _Drop.onto
                    ? palette.signal
                    : const Color(0x00000000),
              ),
            ),
            child: draggable,
          ),
          // Between rows: a signal line where it would go, indented to the
          // level it would land at.
          if (_drop == _Drop.before || _drop == _Drop.after)
            Positioned(
              left: _drop == _Drop.after && _open ? widget.inset : 0,
              right: 0,
              top: _drop == _Drop.before ? -1 : null,
              bottom: _drop == _Drop.after ? -1 : null,
              child: Container(
                height: 2,
                decoration: BoxDecoration(
                  color: palette.signal,
                  borderRadius: BorderRadius.circular(1),
                ),
              ),
            ),
        ],
      ),
    );
    return Listener(
      onPointerDown: (event) {
        _downAt = event.position;
        _downKind = event.kind;
      },
      child: Semantics(
        customSemanticsActions: _actions(workspace),
        child: target,
      ),
    );
  }

  /// Pin or Unpin, and on a pin Move up and Move down, for VoiceOver (the
  /// row's menu otherwise).
  Map<CustomSemanticsAction, VoidCallback> _actions(Workspace workspace) {
    final pin = pinLabel(workspace, agent);
    if (pin == null) return const {};
    return {
      CustomSemanticsAction(label: pin): () =>
          unawaited(togglePin(context, agent)),
      if (_pin) ...{
        const CustomSemanticsAction(label: 'Move up'): ?movePin(
          context,
          agent,
          -1,
        ),
        const CustomSemanticsAction(label: 'Move down'): ?movePin(
          context,
          agent,
          1,
        ),
      },
    };
  }

  Widget _row(Palette palette, Backend backend) {
    // A collapsed agent speaks for the agents hidden under it: it pulses
    // while one works and is marked while one has news.
    // Pinned agents folded under it are left out while the pins are in
    // sight.
    final pinned = _pinsStay(context)
        ? pinnedIds(backend.workspace.workspace)
        : const <String>{};
    final hidden = agent.collapsed
        ? [
            for (final other in _all(widget.children))
              if (!pinned.contains(other.id)) other,
          ]
        : const <Agent>[];
    final working = atWork(agent) || hidden.any(atWork);
    // Held up by usage limits: its line under the name says what it waits
    // for, in place of its role.
    final wait = usageWaitLine(agent);
    // A run ended since the person last opened the agent: its name stands
    // out and a dot in its color sits at the end of the row, until they
    // open it.
    final unseen = agent.unseen || hidden.any((other) => other.unseen);
    // Something in its conversation (or a folded agent's) waits for the
    // person's answer: a count in signal takes the dot's place.
    final waiting = waitingIn([agent, ...hidden]);
    final stands = unseen || waiting > 0;
    final color = agentColor(agent.color);
    // A pin's second line: its role (or wait), then its project.
    final project = _pin
        ? projectById(backend.workspace.workspace, agent.projectId)?.name ?? ''
        : '';
    return _Row(
      // Taller with a role or a wait, for its line under the name; a pin
      // always has its project, on two lines at the largest text.
      height: _pin
          ? (largeText(context) ? 56 : 40)
          : agent.role.isEmpty && wait.isEmpty
          ? 32
          : 40,
      selected: widget.selected,
      onTap: widget.onTap,
      onSecondaryTap: () => unawaited(
        _agentMenu(
          context,
          agent,
          pinRow: _pin,
          at:
              _downKind == PointerDeviceKind.touch ||
                  _downKind == PointerDeviceKind.stylus
              ? null
              : _downAt,
        ),
      ),
      // From the keyboard, under the row's start, as a menu bar's would.
      onMenuKey: () {
        final box = context.findRenderObject();
        unawaited(
          _agentMenu(
            context,
            agent,
            pinRow: _pin,
            at: box is RenderBox && box.hasSize
                ? box.localToGlobal(Offset(Space.l, box.size.height))
                : null,
          ),
        );
      },
      // A finger's hold picks the row up; let go there, it's the menu.
      holdForMenu: false,
      // While it (or an agent folded under it) works, a wave of its color
      // sweeps across the row; it says so plainer than a pulse.
      underlay: working
          ? WorkingWave(color: color, radius: Radii.control - 1)
          : null,
      builder: (context, hovered) {
        final nameStyle = TypeScale.of(context).body.copyWith(
          color: widget.selected || hovered || stands
              ? palette.text
              : palette.textMuted,
          fontWeight: stands
              ? FontWeight.w600
              : widget.selected
              ? FontWeight.w500
              : FontWeight.w400,
        );
        return Row(
          children: [
            if (widget.gutter)
              SizedBox(
                // On a phone, wide enough for a fingertip, the row's height.
                width: touchFirst(context)
                    ? _phoneAvatar(context)
                    : Space.m + 2,
                child: widget.children.isEmpty
                    ? null
                    : Semantics(
                        button: true,
                        label: agent.collapsed
                            ? "Show ${agent.name}'s reports"
                            : "Hide ${agent.name}'s reports",
                        child: GestureDetector(
                          behavior: HitTestBehavior.opaque,
                          onTap: () => fireAndLog(
                            backend.setAgentCollapsed(
                              agent.id,
                              !agent.collapsed,
                            ),
                          ),
                          child: Align(
                            alignment: Alignment.centerLeft,
                            child: AnimatedRotation(
                              turns: agent.collapsed ? 0 : 0.25,
                              duration: Motion.fast,
                              curve: Curves.easeOut,
                              child: Chevron(
                                color: hovered
                                    ? palette.textMuted
                                    : palette.textFaint,
                                down: false,
                              ),
                            ),
                          ),
                        ),
                      ),
              ),
            // The agent itself, living: its own mood, not its hidden agents'
            // (the wave and the unseen mark speak for those).
            Padding(
              padding: const EdgeInsets.only(right: Space.s - 2),
              child: AgentCharacter(
                agent: agent,
                size: touchFirst(context) ? _phoneAvatar(context) : 24,
                live: true,
              ),
            ),
            Expanded(
              child: _QuietAfterName(
                count: agent.quietWaiting,
                about: quietWaitingAbout(agent),
                name: _pin
                    ? Semantics(
                        label: [
                          agent.name,
                          if (wait.isNotEmpty)
                            wait
                          else if (agent.role.isNotEmpty)
                            agent.role,
                          if (project.isNotEmpty) 'in $project',
                          'pinned',
                        ].join(', '),
                        child: ExcludeSemantics(
                          child: _PinName(
                            name: agent.name,
                            role: wait.isEmpty ? agent.role : wait,
                            project: project,
                            style: nameStyle,
                            roleColor: widget.selected || hovered
                                ? palette.textMuted
                                : palette.textFaint,
                          ),
                        ),
                      )
                    : _NameOverRole(
                        name: agent.name,
                        role: wait.isEmpty ? agent.role : wait,
                        style: nameStyle,
                        roleColor: widget.selected || hovered
                            ? palette.textMuted
                            : palette.textFaint,
                      ),
                nameText: agent.name,
                nameStyle: nameStyle,
              ),
            ),
            // Whom it reports to, for VoiceOver: what the nesting shows, and
            // an admin lead (Rivet) the nesting can't. Not on a pin, which
            // isn't nested and says only what it shows (Sale row 197).
            if (!_pin)
              if (backend.workspace.agent(agent.leadId) case final lead?)
                Semantics(
                  label: 'reports to ${lead.name}',
                  child: const SizedBox(),
                ),
            // Held up by usage limits: Try now (or Sign in…) under the
            // pointer, to ask again without opening it. Its conversation
            // has it too, by touch as well.
            if (wait.isNotEmpty && hovered && !touchFirst(context))
              Padding(
                padding: const EdgeInsets.only(left: Space.xs),
                child: TryNow(wait: wait, waiting: agent.id),
              ),
            if (agent.queuedMessages > 0)
              Text(
                '${agent.queuedMessages}',
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textFaint),
              ),
            if (waiting > 0)
              GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: widget.onReach,
                child: MouseRegion(
                  cursor: SystemMouseCursors.click,
                  child: _needsYou(waitingLabel(agent.name, waiting), waiting),
                ),
              )
            // A status mark, not the agent's identity: it stays a dot.
            else if (unseen)
              Padding(
                padding: const EdgeInsets.only(left: Space.s, right: Space.xs),
                child: Semantics(
                  label: '${agent.name} has something new',
                  child: Dot(color: agentColor(agent.color), size: 7),
                ),
              ),
            // On a phone, its reports' disclosure at the row's end, a
            // fingertip wide: right while they're folded, down while shown.
            if (isPhoneLayout(context) && widget.children.isNotEmpty)
              Semantics(
                button: true,
                label: agent.collapsed
                    ? "Show ${agent.name}'s reports"
                    : "Hide ${agent.name}'s reports",
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: () => fireAndLog(
                    backend.setAgentCollapsed(agent.id, !agent.collapsed),
                  ),
                  child: SizedBox(
                    width: Touch.target - Space.s,
                    child: Center(
                      child: AnimatedRotation(
                        turns: agent.collapsed ? 0 : 0.25,
                        duration: Motion.fast,
                        curve: Curves.easeOut,
                        child: Chevron(color: palette.textFaint, down: false),
                      ),
                    ),
                  ),
                ),
              ),
          ],
        );
      },
    );
  }
}

/// A row's [name] with how many quiet messages wait for its agent at its
/// end ("4 quiet", faint), only while the name has room for both: the count
/// gives way first, VoiceOver still saying it.
class _QuietAfterName extends StatelessWidget {
  const _QuietAfterName({
    required this.count,
    required this.about,
    required this.name,
    required this.nameText,
    required this.nameStyle,
  });

  final int count;
  final String about;
  final Widget name;
  final String nameText;
  final TextStyle nameStyle;

  @override
  Widget build(BuildContext context) {
    if (count == 0) return name;
    final quiet = TypeScale.of(context).caption
        .copyWith(color: Palette.of(context).textFaint);
    final scaler = MediaQuery.textScalerOf(context);
    double width(String text, TextStyle style) {
      final painter = TextPainter(
        text: TextSpan(text: text, style: style),
        textDirection: TextDirection.ltr,
        textScaler: scaler,
        maxLines: 1,
      )..layout();
      final w = painter.width;
      painter.dispose();
      return w;
    }

    final words = quietWaitingShort(count);
    return LayoutBuilder(
      builder: (context, box) {
        final fits =
            width(nameText, nameStyle) + Space.s + width(words, quiet) <=
            box.maxWidth;
        return Semantics(
          // Read after the name, shown or not.
          label: about,
          child: Row(
            children: [
              Expanded(child: name),
              if (fits)
                Padding(
                  padding: const EdgeInsets.only(left: Space.s),
                  child: ExcludeSemantics(
                    child: Text(words, maxLines: 1, style: quiet),
                  ),
                ),
            ],
          ),
        );
      },
    );
  }
}

/// An agent's character, or a pipe's portal, on a phone: 32pt, 28 in the
/// drawer ([DenseList]).
double _phoneAvatar(BuildContext context) => DenseList.of(context) ? 28 : 32;

/// A row's name over its role (or how it's doing): [NameOverRole] on a
/// Mac; on a phone the phone's type, the role at 13pt rather than 11.
class _NameOverRole extends StatelessWidget {
  const _NameOverRole({
    required this.name,
    required this.role,
    required this.style,
    required this.roleColor,
  });

  final String name;
  final String role;
  final TextStyle style;
  final Color roleColor;

  @override
  Widget build(BuildContext context) {
    if (!touchFirst(context)) {
      return NameOverRole(
        name: name,
        role: role,
        style: style,
        roleColor: roleColor,
      );
    }
    final text = Text(
      name,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: role.isEmpty ? style : style.copyWith(height: 1.25),
    );
    if (role.isEmpty) return text;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        text,
        Text(
          role,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TypeScale.of(context).caption
              .copyWith(height: 1.25, color: roleColor),
        ),
      ],
    );
  }
}

/// Every agent in [nodes] and nested under them.
List<Agent> _all(List<_Node> nodes) => [
  for (final node in nodes) ...[node.agent, ..._all(node.children)],
];

/// What follows the pointer while an agent is dragged.
class _DragFeedback extends StatelessWidget {
  const _DragFeedback({required this.name, required this.palette});

  final String name;
  final Palette palette;

  @override
  Widget build(BuildContext context) {
    final type = TypeScale.of(context);
    return Transform.translate(
      offset: const Offset(8, -14),
      // What letting go there does, under the name, while it changes who
      // the agent reports to ("Reports to Mike", "Reports to no one"); nothing for
      // a plain reorder.
      child: ValueListenableBuilder<String?>(
        valueListenable: _dropSays,
        builder: (context, says, _) => Container(
          constraints: const BoxConstraints(minHeight: 28),
          padding: EdgeInsets.symmetric(
            horizontal: Space.m,
            vertical: says == null ? 0 : Space.xs,
          ),
          decoration: BoxDecoration(
            color: palette.sidebar,
            borderRadius: BorderRadius.circular(Radii.control),
            border: Border.all(
              color: says == null ? palette.hairline : palette.signal,
            ),
            boxShadow: floatingShadow,
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            mainAxisAlignment: MainAxisAlignment.center,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                name,
                style: type.body.copyWith(
                  color: palette.text,
                  decoration: TextDecoration.none,
                ),
              ),
              if (says != null)
                Text(
                  says,
                  style: type.caption.copyWith(
                    color: palette.signal,
                    fontWeight: FontWeight.w500,
                    decoration: TextDecoration.none,
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// What the drop under the dragged agent would do to its lead, for its
/// feedback; null while it only reorders.
final _dropSays = ValueNotifier<String?>(null);

/// What [arrange] does to [dragged]'s place in the hierarchy, said plainly:
/// "Reports to Mike" when it nests it, "Reports to no one" when it takes it out from
/// under its lead, "Moves to Claktory" into another project's top level;
/// null when it's only a reorder among the same agents.
String? _sayDrop(Workspace workspace, Agent dragged, ArrangeAgent arrange) {
  String nameOf(String id) =>
      workspace.agents.where((a) => a.id == id).firstOrNull?.name ?? '';
  if (arrange.parentId.isNotEmpty) {
    return arrange.parentId == dragged.parentId && !arrange.hasProjectId()
        ? null
        : 'Reports to ${nameOf(arrange.parentId)}';
  }
  if (arrange.hasProjectId()) {
    final project = workspace.projects
        .where((p) => p.id == arrange.projectId)
        .firstOrNull;
    return 'Moves to ${project?.name ?? 'the workspace'}';
  }
  return dragged.parentId.isEmpty ? null : 'Reports to no one';
}

/// A pin under Rivet being dragged: it reorders among the pins only, so
/// rows elsewhere don't take it, and dropped anywhere else it goes back.
@immutable
class _PinDrag {
  const _PinDrag(this.agentId);

  final String agentId;
}

/// Whether the drag going on started with a finger (or a pen): its slots
/// tick as they change.
var _touchDrag = false;

/// What dropping [dragged] in the top group does: "Pins June here", or
/// nothing to say when it only moves its pin.
String? _sayPin(Workspace workspace, Agent dragged) =>
    pinnedIds(workspace).contains(dragged.id)
    ? null
    : 'Pins ${dragged.name} here';

/// The pin [dragged] goes before when dropped in a gap above the pins:
/// the first other pin, or "" (last) when there's none.
String _firstPin(Workspace workspace, Agent dragged) =>
    pinnedAgents(workspace)
        .map((agent) => agent.id)
        .firstWhere((id) => id != dragged.id, orElse: () => '');

/// An agent row's menu: Pin or Unpin (on a pin also Move up and Move
/// down), then Agent settings…. With a pointer, a menu at [at]; by touch,
/// the actions sheet (a popover on an iPad's wide window), titled with the
/// agent, its role and project, since it shows twice when pinned.
Future<void> _agentMenu(
  BuildContext context,
  Agent agent, {
  required bool pinRow,
  Offset? at,
}) async {
  final workspace = AppScope.of(context).workspace.workspace;
  final pin = pinLabel(workspace, agent);
  final up = pinRow ? movePin(context, agent, -1) : null;
  final down = pinRow ? movePin(context, agent, 1) : null;
  final items = <PointerMenuItem<String>?>[
    if (pin != null)
      PointerMenuItem(
        pin,
        'pin',
        glyph: (color) => ExtensionGlyph(
          icon: pin == 'Pin' ? 'pin' : 'pin-off',
          name: '',
          color: color,
        ),
      ),
    if (up != null)
      PointerMenuItem(
        'Move up',
        'up',
        glyph: (color) =>
            ExtensionGlyph(icon: 'chevron-up', name: '', color: color),
      ),
    if (down != null)
      PointerMenuItem(
        'Move down',
        'down',
        glyph: (color) =>
            ExtensionGlyph(icon: 'chevron-down', name: '', color: color),
      ),
    if (pin != null) null,
    PointerMenuItem(
      'Agent settings…',
      'settings',
      glyph: (color) => CogGlyph(color: color),
    ),
  ];
  final project = projectById(workspace, agent.projectId)?.name ?? '';
  final title = at != null
      ? [agent.name, agent.role, project].where((s) => s.isNotEmpty).join(' · ')
      : _menuTitle(context, agent.name, agent.role, project);
  final chosen = at != null
      ? await showPointerMenu<String>(
          context,
          at: at,
          items: items,
          label: title,
        )
      : await showActions<String>(
          context,
          title: title,
          actions: sheetActionsOf(items),
          // On an iPad's wide window, a popover to the row's right.
          beside: true,
        );
  if (chosen == null || !context.mounted) return;
  switch (chosen) {
    case 'pin':
      await togglePin(context, agent);
    case 'up':
      up?.call();
    case 'down':
      down?.call();
    case 'settings':
      final now = AppScope.of(context).workspace.agent(agent.id) ?? agent;
      await showModal(
        context,
        (context, close) => AgentSettingsForm(agent: now, close: close),
      );
  }
}

/// The touch menu's caption: "Bruno · Project Manager · Claktory" in at
/// most two lines (Lucy 47558). The name stays whole; past two lines the
/// role goes first, then the project.
String _menuTitle(
  BuildContext context,
  String name,
  String role,
  String project,
) {
  // As the sheet draws it: its own type, never the drawer's denser one
  // (Zucchero's row 716: measured at 13pt, drawn at 15pt, it took three
  // lines at the largest text).
  final style = (touchFirst(context) ? PhoneType.secondary : TextStyles.body)
      .copyWith(fontWeight: FontWeight.w600);
  // The sheet's text runs the window's width less its margins; a popover
  // is narrower.
  final width = math.min(MediaQuery.sizeOf(context).width, 420.0) - 64;
  final candidates = [
    [name, role, project],
    [name, project],
    [name],
  ];
  for (final parts in candidates) {
    final title = parts.where((part) => part.isNotEmpty).join(' · ');
    final painter = TextPainter(
      text: TextSpan(text: title, style: style),
      textDirection: TextDirection.ltr,
      textScaler: MediaQuery.textScalerOf(context),
    )..layout(maxWidth: math.max(1, width));
    final lines = painter.computeLineMetrics().length;
    painter.dispose();
    if (lines <= 2) return title;
  }
  return name;
}

/// The agents pinned under Rivet, from any project, in the person's order
/// (Lucy's pinned-agents.md §3): each the agent row people know, its role
/// and project on its second line, with no disclosure and no reports
/// under it. They drag among themselves to reorder.
class _Pins extends StatelessWidget {
  const _Pins({
    required this.pins,
    required this.selectedAgentId,
    required this.gutter,
  });

  final List<Agent> pins;
  final String selectedAgentId;

  /// Room for a disclosure, as the agents above have.
  final bool gutter;

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final workspace = backend.workspace.workspace;
    final order = [for (final pin in pins) pin.id];
    // In line with the rows above, inside their place's 1px edge.
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 1),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final pin in pins)
            Kept(
              key: ValueKey('pin/${pin.id}'),
              inputs: [
                pin,
                projectById(workspace, pin.projectId)?.name,
                backend.workspace.agent(pin.leadId),
                selectedAgentId,
                order,
                gutter,
              ],
              builder: (_) => _AgentRow(
                agent: pin,
                projectId: pin.projectId,
                folderId: '',
                selected: pin.id == selectedAgentId,
                onTap: () => _selectAgent(context, pin),
                onReach: () => _selectAgent(context, pin, reach: true),
                gutter: gutter,
                children: const [],
                siblings: order,
                pinOrder: order,
              ),
            ),
        ],
      ),
    );
  }
}

/// A pin's name over its role and project ("Product Marketing ·
/// Claktory"): the role cut first, the project kept whole. At the largest
/// text, the role and the project each take a line, the role cut and the
/// project whole.
class _PinName extends StatelessWidget {
  const _PinName({
    required this.name,
    required this.role,
    required this.project,
    required this.style,
    required this.roleColor,
  });

  final String name;
  final String role;
  final String project;
  final TextStyle style;
  final Color roleColor;

  @override
  Widget build(BuildContext context) {
    final touch = touchFirst(context);
    final small = touch
        ? TypeScale.of(context).caption.copyWith(height: 1.25, color: roleColor)
        : roleStyle(roleColor);
    Text line(String text) => Text(
      text,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      softWrap: false,
      style: small,
    );
    final Widget second;
    if (role.isEmpty || project.isEmpty) {
      second = line(role.isEmpty ? project : role);
    } else if (largeText(context)) {
      second = Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [line(role), line(project)],
      );
    } else {
      second = LayoutBuilder(
        builder: (context, box) {
          final painter = TextPainter(
            text: TextSpan(text: ' · $project', style: small),
            textDirection: TextDirection.ltr,
            textScaler: MediaQuery.textScalerOf(context),
            maxLines: 1,
          )..layout();
          final tail = painter.width;
          painter.dispose();
          // No room even for the project: it alone, cut at its end.
          if (tail >= box.maxWidth) return line(project);
          return Row(
            children: [
              Flexible(child: line(role)),
              Text(' · $project', maxLines: 1, softWrap: false, style: small),
            ],
          );
        },
      );
    }
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          name,
          maxLines: largeText(context) ? null : 1,
          overflow: largeText(context) ? null : TextOverflow.ellipsis,
          style: style.copyWith(height: 1.25),
        ),
        second,
      ],
    );
  }
}

/// Sends [arrange]; a refusal (a lead the backend won't take) says why.
void _arrangeOrSay(BuildContext context, Agent dragged, ArrangeAgent arrange) {
  final backend = AppScope.of(context);
  backend.arrangeAgent(arrange).catchError((Object error) {
    if (!context.mounted || error is! RequestFailure) return;
    unawaited(
      showModal(
        context,
        (context, close) => _Refused(
          title: "Couldn't move ${dragged.name}",
          why: error.message,
          close: close,
        ),
      ),
    );
  }, test: (error) => error is RequestFailure);
}

/// A drop the backend refused: what didn't happen, why, and OK.
class _Refused extends StatelessWidget {
  const _Refused({required this.title, required this.why, required this.close});

  final String title;
  final String why;
  final VoidCallback close;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(title, style: type.emphasis.copyWith(color: palette.text)),
        const SizedBox(height: Space.s),
        Text(why, style: type.body.copyWith(color: palette.textMuted)),
        const SizedBox(height: Space.l),
        Align(
          alignment: Alignment.centerRight,
          child: Button(label: 'OK', primary: true, onPressed: close),
        ),
      ],
    );
  }
}

/// A pipe to another Claktory, under its project's agents: a portal in the
/// pipe's color where an agent's character would be, its name over where it
/// stands, and like an agent's row, a needs-you count for what waits for
/// the person (held messages, its approval, a loop's Continue) or a dot for
/// something new. A click opens it; a right click, or a touch and hold,
/// offers to move it to a group. With a pointer it drags onto a group's
/// row.
class _PipeRow extends StatelessWidget {
  const _PipeRow({
    super.key,
    required this.pipe,
    required this.selected,
    required this.onTap,
  });

  final Pipe pipe;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    // Its liaison, bound to it: no row of its own, its face at this row's
    // end, its work as this row's wave and its waiting in this row's count.
    final liaison = pipeLiaison(workspace, pipe);
    final waiting = pipeWaitingAll(workspace, pipe);
    final unseen = pipe.unseen || (liaison?.unseen ?? false);
    final stands = unseen || waiting > 0;
    final closed = pipe.state == PipeState.PIPE_STATE_CLOSED;
    final phone = touchFirst(context);
    final row = Semantics(
      label: liaison == null ? 'Pipe' : 'Pipe, liaison ${liaison.name}',
      child: _Row(
        key: ValueKey('pipe-row/${pipe.id}'),
        height: 40,
        selected: selected,
        onTap: onTap,
        onSecondaryTap: () => unawaited(pipeGroupActions(context, pipe)),
        underlay: liaison != null && atWork(liaison)
            ? WorkingWave(
                color: agentColor(liaison.color),
                radius: Radii.control - 1,
              )
            : null,
        builder: (context, hovered) => Row(
          children: [
            Padding(
              padding: const EdgeInsets.only(right: Space.s - 2),
              child: SizedBox.square(
                dimension: touchFirst(context) ? _phoneAvatar(context) : 24,
                child: Center(
                  child: PortalGlyph(
                    color: closed ? palette.textFaint : pipeColor,
                  ),
                ),
              ),
            ),
            Expanded(
              child: _NameOverRole(
                name: pipe.name,
                role: pipeStatus(pipe, lead: pipeLead(workspace, pipe)),
                style: TypeScale.of(context).body.copyWith(
                  color: selected || hovered || stands
                      ? palette.text
                      : palette.textMuted,
                  fontWeight: stands
                      ? FontWeight.w600
                      : selected
                      ? FontWeight.w500
                      : FontWeight.w400,
                ),
                roleColor: selected || hovered
                    ? palette.textMuted
                    : palette.textFaint,
              ),
            ),
            if (liaison != null)
              Padding(
                padding: const EdgeInsets.only(left: Space.s),
                child: ExcludeSemantics(
                  child: AgentCharacter(
                    agent: identityOf(liaison),
                    size: phone ? 28 : 20,
                  ),
                ),
              ),
            if (waiting > 0)
              _needsYou(waitingLabel(pipe.name, waiting), waiting)
            else if (unseen)
              Padding(
                padding: const EdgeInsets.only(left: Space.s, right: Space.xs),
                child: Semantics(
                  label: '${pipe.name} has something new',
                  child: const Dot(color: pipeColor, size: 7),
                ),
              ),
          ],
        ),
      ),
    );
    // A finger's touch and hold is the menu (Move to group…), so only a
    // pointer drags it.
    return _PointerDraggable<_PipeDrag>(
      data: _PipeDrag(pipe.id),
      dragAnchorStrategy: pointerDragAnchorStrategy,
      feedback: _DragFeedback(name: pipe.name, palette: palette),
      childWhenDragging: Opacity(opacity: 0.4, child: row),
      child: row,
    );
  }
}

/// A pipe being dragged onto a group's row.
@immutable
class _PipeDrag {
  const _PipeDrag(this.pipeId);

  final String pipeId;
}

/// A project's pipe group (#359): a row with the group's glyph, its name,
/// how many pipes it holds and a disclosure, its pipes under it. A tap
/// folds it away or opens it (the same on every device); a right click or
/// a touch and hold offers Rename… and Delete group…; a pipe dropped on it
/// moves in. Folded, it speaks for its pipes: the wave while a liaison
/// works, and what's new or waits.
class _PipeGroupRow extends StatelessWidget {
  const _PipeGroupRow({
    super.key,
    required this.group,
    required this.pipes,
    required this.selectedPipeId,
  });

  final PipeGroup group;
  final List<Pipe> pipes;
  final String selectedPipeId;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final workspace = backend.workspace.workspace;
    final folded = group.collapsed;
    final hidden = folded ? pipes : const <Pipe>[];
    final working = hidden.any((pipe) {
      final liaison = pipeLiaison(workspace, pipe);
      return liaison != null && atWork(liaison);
    });
    final waiting = hidden.fold(
      0,
      (sum, pipe) => sum + pipeWaitingAll(workspace, pipe),
    );
    final unseen = hidden.any(
      (pipe) => pipe.unseen || (pipeLiaison(workspace, pipe)?.unseen ?? false),
    );
    final stands = unseen || waiting > 0;
    final phone = touchFirst(context);
    final heading = Semantics(
      button: true,
      label: pipeGroupLabel(group, pipes.length, unseen: unseen),
      child: _Row(
        key: ValueKey('pipe-group/${group.id}'),
        underlay: working
            ? WorkingWave(color: pipeColor, radius: Radii.control - 1)
            : null,
        onTap: () => fireAndLog(
          backend.setPipeGroupCollapsed(group.id, !group.collapsed),
        ),
        onSecondaryTap: () => unawaited(pipeGroupMenu(context, group)),
        builder: (context, hovered) {
          final quiet = hovered ? palette.textMuted : palette.textFaint;
          return Row(
            children: [
              Padding(
                padding: const EdgeInsets.only(right: Space.s - 2),
                child: SizedBox(
                  width: phone ? _phoneAvatar(context) : 24,
                  child: const Center(child: PipeGroupGlyph()),
                ),
              ),
              Expanded(
                child: ExcludeSemantics(
                  child: Text(
                    group.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TypeScale.of(context).body.copyWith(
                      color: hovered || stands
                          ? palette.text
                          : palette.textMuted,
                      fontWeight: stands ? FontWeight.w600 : FontWeight.w500,
                    ),
                  ),
                ),
              ),
              Padding(
                padding: const EdgeInsets.only(left: Space.s),
                child: ExcludeSemantics(
                  child: Text(
                    '${pipes.length}',
                    style: TypeScale.of(context).caption.copyWith(color: quiet),
                  ),
                ),
              ),
              if (waiting > 0)
                _needsYou(waitingLabel(group.name, waiting), waiting)
              else if (unseen)
                const Padding(
                  padding: EdgeInsets.only(left: Space.s),
                  child: Dot(color: pipeColor, size: 7),
                ),
              SizedBox(
                width: phone ? Touch.target - Space.s : Space.l,
                child: Align(
                  alignment: phone ? Alignment.center : Alignment.centerRight,
                  child: AnimatedRotation(
                    turns: folded ? 0 : 0.25,
                    duration: Motion.fast,
                    curve: Curves.easeOut,
                    child: Chevron(color: quiet, down: false),
                  ),
                ),
              ),
            ],
          );
        },
      ),
    );
    final target = DragTarget<_PipeDrag>(
      onWillAcceptWithDetails: (details) {
        final pipe = _pipe(workspace, details.data.pipeId);
        return pipe != null &&
            pipe.projectId == group.projectId &&
            pipe.groupId != group.id;
      },
      onAcceptWithDetails: (details) {
        final pipe = _pipe(workspace, details.data.pipeId);
        if (pipe != null) dropPipeOnGroup(context, pipe, group);
      },
      builder: (context, candidates, _) => AnimatedContainer(
        duration: Motion.fast,
        curve: Curves.easeOut,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(Radii.control),
          border: Border.all(
            color: candidates.isEmpty
                ? const Color(0x00000000)
                : palette.signal,
          ),
        ),
        child: heading,
      ),
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        target,
        AnimatedSize(
          duration: Motion.base,
          curve: Curves.easeOut,
          alignment: Alignment.topCenter,
          child: folded
              ? const SizedBox(width: double.infinity)
              : Padding(
                  padding: EdgeInsets.only(
                    left: isPhoneLayout(context) ? Space.s : Space.m + 2,
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      for (final pipe in pipes)
                        _PipeRow(
                          key: ValueKey('pipe-slot/${pipe.id}'),
                          pipe: pipe,
                          selected: pipe.id == selectedPipeId,
                          onTap: () => _selectPipe(context, pipe),
                        ),
                      if (pipes.isEmpty)
                        Padding(
                          padding: const EdgeInsets.symmetric(
                            horizontal: Space.s,
                            vertical: Space.xs,
                          ),
                          child: Text(
                            'No pipes',
                            style: TypeScale.of(context).caption
                                .copyWith(color: palette.textFaint),
                          ),
                        ),
                    ],
                  ),
                ),
        ),
      ],
    );
  }

  static Pipe? _pipe(Workspace workspace, String id) {
    for (final pipe in workspace.pipes) {
      if (pipe.id == id) return pipe;
    }
    return null;
  }
}

/// "Needs you" at the top of the sidebar while anything, anywhere, waits
/// for the person's answer: the inbox glyph, the words, and the count in
/// the needs-you pill. A click shows the Needs you page beside the
/// conversation, or closes it; in a phone's drawer a tap pushes the page,
/// the row in the drawer's sizes as its footer's places.
class _NeedsYouRow extends StatelessWidget {
  const _NeedsYouRow({required this.count});

  final int count;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    return _Row(
      key: const ValueKey('needs-you'),
      // On a phone a full row, a finger's height, as an agent's.
      height: phone ? 40 : 28,
      selected: !phone && AppScope.pageShown(context, AppPage.inbox),
      onTap: phone
          ? () => AppScope.openPage(context, AppPage.inbox)
          : () => AppScope.togglePage(context, AppPage.inbox),
      builder: (context, hovered) => Row(
        children: [
          SizedBox(
            width: phone ? 32 : Space.l + 2,
            child: Align(
              alignment: AlignmentDirectional.centerStart,
              child: ExtensionGlyph(
                icon: 'inbox',
                name: 'Needs you',
                color: palette.text,
                size: phone ? 18 : 14,
              ),
            ),
          ),
          Expanded(
            child: Text(
              'Needs you',
              maxLines: phone ? 2 : 1,
              overflow: TextOverflow.ellipsis,
              style: (phone ? TypeScale.of(context).body : TextStyles.body)
                  .copyWith(color: palette.text, fontWeight: FontWeight.w600),
            ),
          ),
          _needsYou(needsYouLabel(count), count),
        ],
      ),
    );
  }
}

class _QuietRow extends StatelessWidget {
  const _QuietRow({
    required this.label,
    required this.onTap,
    this.waiting = 0,
    this.waitingLabel =
        'Something in another project is waiting for your answer',
  });

  final String label;
  final VoidCallback onTap;

  /// What waits for the person out of sight behind it: marked at its end.
  final int waiting;

  /// What the mark says to VoiceOver.
  final String waitingLabel;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return _Row(
      onTap: onTap,
      builder: (context, hovered) => Row(
        children: [
          Expanded(
            child: Text(
              label,
              style: TypeScale.of(context).body.copyWith(
                color: hovered ? palette.textMuted : palette.textFaint,
              ),
            ),
          ),
          if (waiting > 0) _needsYou(waitingLabel, waiting),
        ],
      ),
    );
  }
}

/// How many chats the sidebar lists, the newest; "Show all" opens the rest.
const chatsListed = 20;

/// The person's chats (#213): conversations of their own beside the agents,
/// for a question that isn't any agent's job. Under "Chats" (its "+" starts
/// a new one), the [chatsListed] newest, each a row with a speech bubble
/// and its title, no character and no color, since a chat is a session,
/// not someone; then "Show all" when there are more. With none yet, one
/// quiet "New chat" row.
class _Chats extends StatelessWidget {
  const _Chats({required this.chats, required this.selectedId});

  /// Newest first.
  final List<Chat> chats;
  final String selectedId;

  /// Opens a new chat (on a phone, in front).
  static void startNew(BuildContext context) {
    startNewChat(context);
    _toConversation(context);
  }

  void _new(BuildContext context) => startNew(context);

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final listed = chats.take(chatsListed).toList();
    final rest = chats.skip(chatsListed).toList();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (chats.isEmpty)
          _Row(
            onTap: () => _new(context),
            builder: (context, hovered) => Row(
              children: [
                _ChatMark(
                  color: hovered ? palette.textMuted : palette.textFaint,
                ),
                Expanded(
                  child: Text(
                    'New chat',
                    style: TypeScale.of(context).body.copyWith(
                      color: hovered ? palette.textMuted : palette.textFaint,
                    ),
                  ),
                ),
              ],
            ),
          ),
        for (final chat in listed)
          _ChatRow(
            key: ValueKey('chat-row/${chat.id}'),
            chat: chat,
            selected: chat.id == selectedId,
          ),
        if (rest.isNotEmpty)
          _QuietRow(
            // One word for it in the drawer, as Needs you's See all.
            label: isPhoneLayout(context) ? 'See all' : 'Show all',
            waiting: rest.fold<int>(0, (sum, chat) => sum + chatWaiting(chat)),
            waitingLabel: 'A chat not shown here is waiting for your answer',
            onTap: () => _showAll(context),
          ),
      ],
    );
  }

  /// Every chat: on a phone a page of its own over the conversation, on a
  /// Mac a dialog.
  void _showAll(BuildContext context) {
    final routes = PhoneRoutes.of(context);
    if (routes != null) {
      routes.closeDrawer();
      unawaited(
        routes.push<void>(
          PhoneRoute(id: 'chats', builder: (context) => const _ChatsPage()),
        ),
      );
      return;
    }
    showModal(
      context,
      (context, close) => SizedBox(
        width: 440,
        height: 520,
        child: ChatHistory(
          onOpen: (chat) {
            close();
            openChat(context, chat);
          },
        ),
      ),
    );
  }
}

/// On a phone, every chat as a page (Back, "Chats", New chat): a tap opens
/// one as the conversation.
class _ChatsPage extends StatelessWidget {
  const _ChatsPage();

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    void back() => PhoneRoutes.of(context)?.pop<void>();
    return ColoredBox(
      color: palette.sidebar,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          PhoneHeader(
            leading: HeaderButton.back(onTap: back),
            title: Text(
              'Chats',
              style: PhoneType.emphasis.copyWith(color: palette.text),
            ),
            trailing: [
              HeaderButton(
                label: 'New chat',
                onTap: () {
                  startNewChat(context);
                  _toConversation(context);
                },
                glyph: (color) =>
                    ExtensionGlyph(icon: 'plus', name: '+', color: color),
              ),
            ],
          ),
          Expanded(
            child: ChatHistory(
              onOpen: (chat) {
                openChat(context, chat);
                _toConversation(context);
              },
            ),
          ),
        ],
      ),
    );
  }
}

/// A chat's mark where an agent's character would be: a speech bubble,
/// in the row's quiet color.
class _ChatMark extends StatelessWidget {
  const _ChatMark({required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    final phone = touchFirst(context);
    return Padding(
      padding: const EdgeInsets.only(right: Space.s - 2),
      child: SizedBox.square(
        dimension: phone ? _phoneAvatar(context) : 24,
        child: Center(child: ChatGlyph(color: color)),
      ),
    );
  }
}

/// One chat: its bubble and title (bold while something new or waiting
/// is in it), a faint wave across it while it replies, then the
/// needs-you count of a proposal waiting, or a dot for a reply not yet
/// read. A tap opens it; a right click, or a touch and hold, offers Rename
/// and Delete.
class _ChatRow extends StatelessWidget {
  const _ChatRow({super.key, required this.chat, required this.selected});

  final Chat chat;
  final bool selected;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final title = chatTitle(chat);
    final waiting = chatWaiting(chat);
    final stands = chat.unseen || waiting > 0;
    // Held up by usage limits: under its title, what it waits for, as an
    // agent's row says it in its role's place.
    final wait = chatUsageWaitLine(chat);
    return Semantics(
      label: 'Chat',
      child: _Row(
        // Taller with the wait, for its line under the title.
        height: wait.isEmpty ? 28 : 40,
        selected: selected,
        onTap: () {
          openChat(context, chat);
          _toConversation(context);
        },
        // As its own "…": Rename…, Change model… and Delete….
        onSecondaryTap: () => chatActions(context, chat, search: false),
        // While it replies (not while it waits); in no one's color, as a
        // chat is no one.
        underlay: chatAtWork(chat)
            ? WorkingWave(color: palette.textFaint, radius: Radii.control - 1)
            : null,
        builder: (context, hovered) => Row(
          children: [
            _ChatMark(
              color: selected || hovered
                  ? palette.textMuted
                  : palette.textFaint,
            ),
            Expanded(
              child: _NameOverRole(
                name: title,
                role: wait,
                style: TypeScale.of(context).body.copyWith(
                  color: selected || hovered || stands
                      ? palette.text
                      : palette.textMuted,
                  fontWeight: stands
                      ? FontWeight.w600
                      : selected
                      ? FontWeight.w500
                      : FontWeight.w400,
                ),
                roleColor: selected || hovered
                    ? palette.textMuted
                    : palette.textFaint,
              ),
            ),
            // As an agent's row: Try now (or Sign in…) under the pointer.
            if (wait.isNotEmpty && hovered && !touchFirst(context))
              Padding(
                padding: const EdgeInsets.only(left: Space.xs),
                child: TryNow(wait: wait, waiting: chat.id),
              ),
            if (waiting > 0)
              _needsYou(waitingLabel(title, waiting), waiting)
            else if (chat.unseen)
              Padding(
                padding: const EdgeInsets.only(left: Space.s, right: Space.xs),
                child: Semantics(
                  label: '$title has something new',
                  child: Dot(color: palette.textMuted, size: 7),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// The needs-you mark at the end of a sidebar row, where the unseen dot
/// would be.
Widget _needsYou(String label, int count) => Padding(
  padding: const EdgeInsets.only(left: Space.s, right: Space.xs),
  child: NeedsYouMark(count: count, label: label),
);

/// Archived projects, at the bottom of the project list: "Archived (N)",
/// which opens in place to list them, each with Unarchive. A click on one
/// opens its settings.
class _Archived extends StatefulWidget {
  const _Archived({required this.projects});

  final List<Project> projects;

  @override
  State<_Archived> createState() => _ArchivedState();
}

class _ArchivedState extends State<_Archived> {
  var _open = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: Space.s),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _Row(
            onTap: () => setState(() => _open = !_open),
            builder: (context, hovered) => Row(
              children: [
                Expanded(
                  child: Text(
                    'Archived (${widget.projects.length})',
                    style: TypeScale.of(context).body.copyWith(
                      color: hovered ? palette.textMuted : palette.textFaint,
                    ),
                  ),
                ),
                Chevron(
                  color: hovered ? palette.textMuted : palette.textFaint,
                  down: _open,
                ),
              ],
            ),
          ),
          if (_open)
            for (final project in widget.projects)
              _Row(
                key: ValueKey('archived:${project.id}'),
                onTap: () => _openProjectSettings(context, project),
                onSecondaryTap: () => _openProjectSettings(context, project),
                builder: (context, hovered) => Row(
                  children: [
                    SizedBox(
                      width: Space.l + 2,
                      child: Align(
                        alignment: Alignment.centerLeft,
                        child: FolderGlyph(color: palette.textFaint),
                      ),
                    ),
                    Expanded(
                      child: Text(
                        project.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TypeScale.of(context).body.copyWith(
                          color: hovered
                              ? palette.textMuted
                              : palette.textFaint,
                        ),
                      ),
                    ),
                    _Unarchive(project: project),
                  ],
                ),
              ),
        ],
      ),
    );
  }
}

/// A quiet "Unarchive" at the end of an archived project's row.
class _Unarchive extends StatelessWidget {
  const _Unarchive({required this.project});

  final Project project;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return Semantics(
      button: true,
      label: 'Unarchive ${project.name}',
      child: Pressable(
        onTap: () => fireAndLog(unarchiveProject(backend, project)),
        builder: (context, hovered) => Padding(
          padding: const EdgeInsets.symmetric(horizontal: Space.xs),
          child: ExcludeSemantics(
            child: Text(
              'Unarchive',
              style: TypeScale.of(context).caption.copyWith(
                color: hovered ? palette.text : palette.textMuted,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// On a phone, the drawer's tree narrowed to [query]: the agents and
/// pipes whose names (or roles, or projects' names) have it in them, each
/// with its project, here on the phone.
class SidebarSearch extends StatelessWidget {
  const SidebarSearch({super.key, required this.query});

  final String query;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final workspace = backend.workspace.workspace;
        final wanted = query.toLowerCase();
        bool has(String text) => text.toLowerCase().contains(wanted);
        String projectName(String id) =>
            id.isEmpty ? '' : projectById(workspace, id)?.name ?? '';
        final found = <Object>[
          for (final agent in workspace.agents)
            if (agent.pipeId.isEmpty &&
                archivedProjectOf(workspace, agent) == null &&
                (has(agent.name) ||
                    has(agent.role) ||
                    has(projectName(agent.projectId))))
              agent,
          // A pipe is found by its liaison's name too.
          for (final pipe in workspace.pipes)
            if (has(pipe.name) ||
                has(projectName(pipe.projectId)) ||
                has(pipeLiaison(workspace, pipe)?.name ?? ''))
              pipe,
          for (final chat in chatsNewestFirst(workspace))
            if (has(chatTitle(chat))) chat,
        ];
        if (found.isEmpty) {
          return Padding(
            padding: const EdgeInsets.all(Space.l),
            child: Text(
              'Nothing matches “$query”.',
              style: TypeScale.of(context).body
                  .copyWith(color: palette.textMuted),
            ),
          );
        }
        return ListView(
          padding: const EdgeInsets.fromLTRB(Space.s, 0, Space.s, Space.m),
          children: [
            for (final item in found)
              if (item is Chat)
                _ChatRow(
                  key: ValueKey('chat-row/${item.id}'),
                  chat: item,
                  selected: false,
                )
              else
                _FlatRow(item: item, project: true),
          ],
        );
      },
    );
  }
}

/// An agent or a pipe on a row of its own, outside the tree (a search):
/// its character or portal, its name over its role or status (and with
/// [project], its project's name), and what waits. A tap opens it.
class _FlatRow extends StatelessWidget {
  const _FlatRow({required this.item, this.project = false});

  /// An [Agent] or a [Pipe].
  final Object item;
  final bool project;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final item = this.item;
    final String name;
    final String line;
    final String projectId;
    final int waiting;
    final Widget glyph;
    final VoidCallback onTap;
    if (item is Pipe) {
      name = item.name;
      line = pipeStatus(item);
      projectId = item.projectId;
      waiting = pipeWaitingAll(workspace, item);
      glyph = PortalGlyph(
        color: item.state == PipeState.PIPE_STATE_CLOSED
            ? palette.textFaint
            : pipeColor,
      );
      onTap = () => _selectPipe(context, item);
    } else {
      final agent = item as Agent;
      name = agent.name;
      final wait = usageWaitLine(agent);
      line = wait.isEmpty ? agent.role : wait;
      projectId = agent.projectId;
      waiting = waitingIn([agent]);
      glyph = AgentCharacter(
        agent: agent,
        size: _phoneAvatar(context),
        live: true,
      );
      onTap = () => _selectAgent(context, agent);
    }
    final where = project && projectId.isNotEmpty
        ? (projectById(workspace, projectId)?.name ?? '')
        : '';
    final under = [line, where].where((part) => part.isNotEmpty).join(' · ');
    return _Row(
      key: ValueKey(item is Pipe ? 'pipe/${item.id}' : (item as Agent).id),
      height: 40,
      onTap: onTap,
      builder: (context, hovered) => Row(
        children: [
          Padding(
            padding: const EdgeInsets.only(right: Space.s - 2),
            child: SizedBox.square(
              dimension: _phoneAvatar(context),
              child: Center(child: glyph),
            ),
          ),
          Expanded(
            child: _NameOverRole(
              name: name,
              role: under,
              style: TypeScale.of(context).body.copyWith(
                color: palette.text,
                fontWeight: waiting > 0 ? FontWeight.w600 : FontWeight.w400,
              ),
              roleColor: palette.textMuted,
            ),
          ),
          if (waiting > 0) _needsYou(waitingLabel(name, waiting), waiting),
        ],
      ),
    );
  }
}

/// On a phone, the footer's places at the end of the drawer's list, one
/// labelled row each (Map, Usage, Marketplace, Settings, What's new), then
/// what's going on with sign-in and the settings file, and the versions.
class _PhoneFooter extends StatelessWidget {
  const _PhoneFooter({required this.workspace, required this.canAddProvider});

  final Workspace workspace;

  /// Whether the + after the usage circles shows ([_canAddProvider]).
  final bool canAddProvider;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    final type = TypeScale.of(context);
    Widget place(
      AppPage? page,
      String label,
      Widget Function(Color color) glyph, {
      String? glow,
      VoidCallback? onTap,
    }) {
      final row = _Row(
        onTap: onTap ?? () => AppScope.openPage(context, page!),
        builder: (context, hovered) => Row(
          children: [
            SizedBox(
              width: 32,
              child: Align(
                alignment: AlignmentDirectional.centerStart,
                child: glyph(palette.textMuted),
              ),
            ),
            Expanded(
              child: Text(
                label,
                style: type.body.copyWith(color: palette.text),
              ),
            ),
          ],
        ),
      );
      if (glow == null) return row;
      return Glow(
        store: scope.backend.highlights,
        matches: (key) => key == glow,
        child: row,
      );
    }

    final status = _footerStatus(context, workspace);
    return Padding(
      padding: const EdgeInsets.only(top: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(height: 1, color: palette.hairline),
          const SizedBox(height: Space.s),
          place(
            AppPage.map,
            'Map',
            (color) => ExtensionGlyph(icon: 'network', name: '', color: color),
          ),
          place(
            AppPage.usage,
            'Usage',
            (color) =>
                ExtensionGlyph(icon: 'chart-column', name: '', color: color),
          ),
          place(
            AppPage.monitors,
            'Monitors',
            (color) =>
                ExtensionGlyph(icon: 'alarm-clock', name: '', color: color),
          ),
          // People, only where the backend offers direct messages (its
          // preview, FACTORY_DMS_PREVIEW) until it ships.
          if (peopleAvailable(workspace))
            place(
              null,
              'People',
              (color) => ExtensionGlyph(icon: 'users', name: '', color: color),
              onTap: () => showPeople(context),
            ),
          place(
            AppPage.marketplace,
            'Marketplace',
            (color) => BagGlyph(color: color),
            glow: 'marketplace:',
          ),
          place(
            AppPage.settings,
            'Settings',
            (color) => CogGlyph(color: color),
            glow: 'settings:',
          ),
          place(
            AppPage.changelog,
            "What's new",
            (color) =>
                ExtensionGlyph(icon: 'scroll-text', name: '', color: color),
          ),
          // Who's at work (a tap opens the Map), then how much of each
          // provider's limits is used; a touch and hold for more.
          Padding(
            padding: const EdgeInsets.fromLTRB(Space.xs, Space.s, Space.xs, 0),
            child: _ActivityAndUsage(
              workspace: workspace,
              canAddProvider: canAddProvider,
            ),
          ),
          ForwardingsLine(
            forwards: scope.backend.forwards,
            except: workspace.hasLogin() ? workspace.login.url : '',
            padding: const EdgeInsets.fromLTRB(Space.s, Space.s, Space.s, 0),
          ),
          for (final line in status)
            Padding(
              padding: const EdgeInsets.fromLTRB(Space.s, Space.s, Space.s, 0),
              child: line,
            ),
          Padding(
            padding: const EdgeInsets.fromLTRB(Space.s, Space.m, Space.s, 0),
            child: Text(
              _versions(workspace.version, scope.appBuild.version),
              style: type.caption.copyWith(color: palette.textFaint),
            ),
          ),
        ],
      ),
    );
  }
}

/// Who's at work, then each account's usage circle and the + to add a
/// provider, in one row that wraps from its left edge ([ProviderUsageCircles]).
/// Kept while the providers, the +, and who's at work stay the same.
class _ActivityAndUsage extends StatelessWidget {
  const _ActivityAndUsage({
    required this.workspace,
    required this.canAddProvider,
  });

  final Workspace workspace;
  final bool canAddProvider;

  @override
  Widget build(BuildContext context) {
    final (:working, :total) = teamActivity(workspace);
    return Kept(
      inputs: [workspace.providers, canAddProvider, working, total],
      builder: (context) => ProviderUsageCircles(
        leading: TeamActivity(workspace: workspace),
        providers: workspace.providers,
        onAdd: canAddProvider ? () => AppScope.openProviders(context) : null,
      ),
    );
  }
}

/// Whether the footer offers to add an AI provider (the + after the usage
/// circles): once this Claktory has shown what it has, as the circles do.
/// Not while it doesn't allow this device, or before it ever answered,
/// where Providers would read as if it had none (Zucchero's row 216); a
/// moment's reconnecting keeps it, as it keeps the circles.
bool _canAddProvider(Backend backend, Workspace workspace) =>
    backend.refused.value == null &&
    (backend.connected.value || usageMeters(workspace.providers).isNotEmpty);

/// The footer's lines about sign-in and the settings file, when there's
/// something to say: "Signing in to …", "Settings file has an error".
List<Widget> _footerStatus(BuildContext context, Workspace workspace) {
  final palette = Palette.of(context);
  final configError = workspace.configError;
  void openSettings() => AppScope.openProviders(context);
  final status = <Widget>[];
  if (workspace.hasLogin() && !AppScope.pageShown(context, AppPage.settings)) {
    final login = workspace.login;
    var provider = login.providerId;
    for (final candidate in workspace.providers) {
      if (candidate.id == login.providerId) provider = candidate.name;
    }
    final failed = login.error.isNotEmpty;
    status.add(
      Pressable(
        onTap: openSettings,
        builder: (context, hovered) => Row(
          children: [
            if (!failed) ...[const LiveDot(), const SizedBox(width: Space.s)],
            Flexible(
              child: Text(
                failed
                    ? "Couldn't sign in to $provider"
                    : 'Signing in to $provider…',
                style: TypeScale.of(context).caption.copyWith(
                  color: failed
                      ? palette.danger
                      : hovered
                      ? palette.text
                      : palette.textMuted,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
  if (configError.isNotEmpty) {
    status.add(
      Pressable(
        onTap: () => showModal(
          context,
          (context, close) => _ConfigError(message: configError, close: close),
        ),
        builder: (context, hovered) => Text(
          'Settings file has an error',
          style: TypeScale.of(context).caption.copyWith(
            color: palette.danger,
            decoration: hovered ? TextDecoration.underline : null,
            decorationColor: palette.danger,
          ),
        ),
      ),
    );
  }
  return status;
}

/// Settings and the Marketplace, the version, and sign-in, connection and
/// settings-file state when there is any.
class _Footer extends StatelessWidget {
  const _Footer({
    required this.workspace,
    required this.connected,
    required this.canAddProvider,
    required this.page,
  });

  final Workspace workspace;
  final bool connected;

  /// Whether the + after the usage circles shows ([_canAddProvider]).
  final bool canAddProvider;
  final AppPage page;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    final appVersion = scope.appBuild.version;
    final status = _footerStatus(context, workspace);
    return Container(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.s, Space.s, Space.m),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: palette.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // Links opened from conversations whose pages come back to
          // localhost on the Claktory's machine (a sign-in's own show in
          // Settings); nothing while there are none.
          ForwardingsLine(
            forwards: scope.backend.forwards,
            except: workspace.hasLogin() ? workspace.login.url : '',
            padding: const EdgeInsets.fromLTRB(Space.s, 0, Space.s, Space.s),
          ),
          for (final line in status)
            Padding(
              padding: const EdgeInsets.fromLTRB(Space.s, 0, Space.s, Space.s),
              child: line,
            ),
          // Who's at work, then how much of each provider's limits is used,
          // wrapping together as accounts multiply.
          Padding(
            padding: const EdgeInsets.fromLTRB(Space.xs, 0, Space.xs, Space.xs),
            child: _ActivityAndUsage(
              workspace: workspace,
              canAddProvider: canAddProvider,
            ),
          ),
          // The same buttons while what they show is: the page shown, and
          // the open agent's panes, where a page can show beside it.
          Kept(
            inputs: [
              page,
              scope.backend.workspace.selectedAgent,
              workspace.selectedPipeId,
              workspace.selectedChatId,
              peopleAvailable(workspace),
            ],
            builder: (context) => _FooterButtons(
              pages: [
                Glow(
                  store: scope.backend.highlights,
                  matches: (key) => key == 'settings:',
                  child: GlyphButton(
                    label: 'Settings',
                    active: AppScope.pageShown(context, AppPage.settings),
                    onTap: () => AppScope.togglePage(context, AppPage.settings),
                    glyph: (color) => CogGlyph(color: color),
                  ),
                ),
                Glow(
                  store: scope.backend.highlights,
                  matches: (key) => key == 'marketplace:',
                  child: GlyphButton(
                    label: 'Marketplace',
                    active: AppScope.pageShown(context, AppPage.marketplace),
                    onTap: () =>
                        AppScope.togglePage(context, AppPage.marketplace),
                    glyph: (color) => BagGlyph(color: color),
                  ),
                ),
                GlyphButton(
                  label: "What's new",
                  active: AppScope.pageShown(context, AppPage.changelog),
                  onTap: () => AppScope.togglePage(context, AppPage.changelog),
                  glyph: (color) => ExtensionGlyph(
                    icon: 'scroll-text',
                    name: '',
                    color: color,
                  ),
                ),
                GlyphButton(
                  label: 'Map',
                  active: AppScope.pageShown(context, AppPage.map),
                  onTap: () => AppScope.togglePage(context, AppPage.map),
                  glyph: (color) => ExtensionGlyph(
                    icon: 'network',
                    name: 'Map',
                    color: color,
                  ),
                ),
                GlyphButton(
                  label: 'Usage',
                  active: AppScope.pageShown(context, AppPage.usage),
                  onTap: () => AppScope.togglePage(context, AppPage.usage),
                  glyph: (color) => ExtensionGlyph(
                    icon: 'chart-column',
                    name: 'Usage',
                    color: color,
                  ),
                ),
                GlyphButton(
                  label: 'Monitors',
                  active: AppScope.pageShown(context, AppPage.monitors),
                  onTap: () => AppScope.togglePage(context, AppPage.monitors),
                  glyph: (color) => ExtensionGlyph(
                    icon: 'alarm-clock',
                    name: 'Monitors',
                    color: color,
                  ),
                ),
                // People, only where the backend offers direct messages (its
                // preview) until it ships.
                if (peopleAvailable(workspace))
                  GlyphButton(
                    label: 'People',
                    active: false,
                    onTap: () => showPeople(context),
                    glyph: (color) => ExtensionGlyph(
                      icon: 'users',
                      name: 'People',
                      color: color,
                    ),
                  ),
              ],
              // The person, at the end: their profile is at the top of
              // Settings.
              profile: GlyphButton(
                label: 'Profile',
                active: false,
                onTap: () => AppScope.openPage(context, AppPage.settings),
                glyph: (_) =>
                    const ExcludeSemantics(child: PersonAvatar(size: 20)),
              ),
            ),
          ),
          // How it's connected, always in sight: which Claktory this is,
          // and whether what shows is live.
          Padding(
            padding: const EdgeInsets.fromLTRB(Space.s, Space.xs, Space.s, 0),
            child: ConnectionLine(backend: scope.backend),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(Space.s, 2, Space.s, 0),
            // The versions: plain text, as the rail switches and adds
            // Claktories (a phone's list of them).
            child: Text(
              _versions(workspace.version, appVersion),
              style: TextStyles.caption.copyWith(
                color: palette.textFaint,
                fontSize: 11,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// The footer's buttons: the pages, then the person at the end. On a Mac a
/// line of 28px buttons (two in a narrow sidebar); with a finger's targets (an iPad's sidebar,
/// [touchFirst]) the pages wrap onto more lines in the room beside the
/// person rather than run past the sidebar's edge.
class _FooterButtons extends StatelessWidget {
  const _FooterButtons({required this.pages, required this.profile});

  final List<Widget> pages;
  final Widget profile;

  @override
  Widget build(BuildContext context) {
    if (touchFirst(context)) {
      return Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(child: Wrap(children: pages)),
          profile,
        ],
      );
    }
    // In a sidebar too narrow for all of them in a line, the last pages
    // go on a second line beside the person, rather than past the edge.
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(
          child: Wrap(spacing: Space.xs, runSpacing: Space.xs, children: pages),
        ),
        const SizedBox(width: Space.xs),
        profile,
      ],
    );
  }
}

class _ConfigError extends StatefulWidget {
  const _ConfigError({required this.message, required this.close});

  final String message;
  final VoidCallback close;

  @override
  State<_ConfigError> createState() => _ConfigErrorState();
}

class _ConfigErrorState extends State<_ConfigError> {
  bool restoring = false;
  String? failure;

  Future<void> restore() async {
    setState(() {
      restoring = true;
      failure = null;
    });
    try {
      await AppScope.of(context).restoreSettings();
      widget.close();
    } on RequestFailure catch (error) {
      if (!mounted) return;
      setState(() {
        restoring = false;
        failure = error.message;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): widget.close,
      },
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Your settings file has an error',
            style: TextStyles.emphasis.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.s),
          Text(
            'Claktory is still using your last working settings. Restore '
            'them to replace the broken file. A copy of it is kept.',
            style: TextStyles.body.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.l),
          CodeBlock(widget.message, color: palette.danger),
          if (failure != null) ...[
            const SizedBox(height: Space.m),
            ErrorLine(
              failure!,
              about: "Restoring the last working settings didn't work.",
              style: TextStyles.body.copyWith(color: palette.danger),
            ),
          ],
          const SizedBox(height: Space.xl),
          // A line of its own for what doesn't fit beside the rest (Rivet's
          // wait, while it waits).
          Wrap(
            alignment: WrapAlignment.end,
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: Space.s,
            runSpacing: Space.s,
            children: [
              Button(label: 'Close', onPressed: widget.close),
              TellRivet(
                error: widget.message,
                about: 'The settings file has an error.',
                onTold: widget.close,
              ),
              Button(
                label: 'Restore last working settings',
                onPressed: restoring ? null : restore,
                primary: true,
              ),
            ],
          ),
        ],
      ),
    );
  }
}

/// Which Factory this is: the backend's version, and the app's too when it
/// differs.
String _versions(String backend, String app) {
  // Versions carry their "v" (settings.md, Words).
  final (b, a) = (versionLabel(backend), versionLabel(app));
  if (backend.isEmpty) return 'Claktory · app $a';
  return backend == app ? 'Claktory $b' : 'Claktory $b · app $a';
}

/// A [Draggable] that starts only once the pointer has moved a few pixels,
/// like a native drag. Flutter's starts after one pixel for a mouse, so a
/// click with a slight jiggle became a drag (and no click). A finger (or
/// pen) starts one only by holding still a moment, as on iPhone: a finger
/// that moves first scrolls the list.
class _SidebarDraggable<T extends Object> extends Draggable<T> {
  const _SidebarDraggable({
    required super.child,
    required super.feedback,
    super.data,
    super.childWhenDragging,
    super.dragAnchorStrategy,
    super.onDragStarted,
    super.onDragUpdate,
    super.onDragEnd,
  });

  @override
  MultiDragGestureRecognizer createRecognizer(
    GestureMultiDragStartCallback onStart,
  ) =>
      _SlopDragRecognizer(allowedButtonsFilter: allowedButtonsFilter)
        ..onStart = onStart;
}

/// A [_SidebarDraggable] a pointer drags (a mouse, a trackpad) and a
/// finger doesn't: its touch and hold is the row's menu.
class _PointerDraggable<T extends Object> extends Draggable<T> {
  const _PointerDraggable({
    required super.child,
    required super.feedback,
    super.data,
    super.childWhenDragging,
    super.dragAnchorStrategy,
  });

  @override
  MultiDragGestureRecognizer createRecognizer(
    GestureMultiDragStartCallback onStart,
  ) => _SlopDragRecognizer(
    allowedButtonsFilter: allowedButtonsFilter,
    supportedDevices: const {
      PointerDeviceKind.mouse,
      PointerDeviceKind.trackpad,
    },
  )..onStart = onStart;
}

/// How far the pointer moves before a press becomes a drag.
const _dragSlop = 4.0;

class _SlopDragRecognizer extends MultiDragGestureRecognizer {
  _SlopDragRecognizer({super.allowedButtonsFilter, super.supportedDevices})
    : super(debugOwner: null);

  @override
  MultiDragPointerState createNewPointerState(PointerDownEvent event) =>
      switch (event.kind) {
        PointerDeviceKind.touch ||
        PointerDeviceKind.stylus ||
        PointerDeviceKind.invertedStylus => _HoldPointerState(
          event.position,
          event.kind,
          gestureSettings,
        ),
        _ => _SlopPointerState(event.position, event.kind, gestureSettings),
      };

  @override
  String get debugDescription => 'sidebar drag';
}

class _SlopPointerState extends MultiDragPointerState {
  _SlopPointerState(super.initialPosition, super.kind, super.gestureSettings);

  @override
  void checkForResolutionAfterMove() {
    if (pendingDelta!.distance >
        math.max(_dragSlop, computeHitSlop(kind, gestureSettings))) {
      resolve(GestureDisposition.accepted);
    }
  }

  @override
  void accepted(GestureMultiDragStartCallback starter) =>
      starter(initialPosition);
}

/// How long a finger holds still before it picks a row up.
const sidebarHoldToDrag = kLongPressTimeout;

/// A finger: it picks the row up once it has held still for
/// [sidebarHoldToDrag]; moving first lets the list scroll instead.
class _HoldPointerState extends MultiDragPointerState {
  _HoldPointerState(super.initialPosition, super.kind, super.gestureSettings) {
    _timer = Timer(sidebarHoldToDrag, _held);
  }

  Timer? _timer;
  GestureMultiDragStartCallback? _starter;

  void _held() {
    _timer = null;
    final starter = _starter;
    if (starter != null) {
      _starter = null;
      _pickUp(starter);
    } else {
      resolve(GestureDisposition.accepted);
    }
  }

  /// The row comes up under the finger, with a tick.
  void _pickUp(GestureMultiDragStartCallback starter) {
    HapticFeedback.selectionClick();
    starter(initialPosition);
  }

  @override
  void checkForResolutionAfterMove() {
    if (_timer == null) return;
    if (pendingDelta!.distance > computeHitSlop(kind, gestureSettings)) {
      // Moved before holding: a scroll, not a drag.
      _timer?.cancel();
      _timer = null;
      resolve(GestureDisposition.rejected);
    }
  }

  @override
  void accepted(GestureMultiDragStartCallback starter) {
    if (_timer == null) {
      _pickUp(starter);
    } else {
      // Won the arena before the hold ended: start once it does.
      _starter = starter;
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    _timer = null;
    super.dispose();
  }
}
