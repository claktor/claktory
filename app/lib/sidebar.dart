import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/gestures.dart';
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
        chatTitle,
        chatWaiting,
        chatsNewestFirst,
        deleteChat,
        openChat,
        renameChat,
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
import 'mobile/drawer.dart' show largeText;
import 'mobile/haptics.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'mobile/navigation.dart' show PhoneRoute, PhoneRoutes;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'mobile/platform_policy.dart';
import 'phone.dart' show PhoneProjects;
import 'pipes.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart' show isAdminAgent;
import 'usage.dart';
import 'usage_wait.dart';
import 'waiting.dart' show NeedsYouMark, waitingIn, waitingLabel;
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
    return DecoratedBox(
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
            final pinned =
                focused != null &&
                    rivet != null &&
                    isAdminAgent(rivet) &&
                    rivet.projectId != focused.id
                ? rivet
                : null;
            // On a phone: what waits for the person first, and the
            // footer's places as rows at the end of the list.
            final needsYou = phone ? _NeedsYou.of(workspace) : const <Object>[];
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                // The traffic lights' row.
                if (!phone) const TitleSpace(),
                Expanded(
                  child: FocusTraversalGroup(
                    child: ListView(
                      padding: const EdgeInsets.fromLTRB(
                        Space.s,
                        0,
                        Space.s,
                        Space.m,
                      ),
                      children: [
                        if (needsYou.isNotEmpty) _NeedsYou(waiting: needsYou),
                        // On a Mac, everything that waits for the person in
                        // one place: there only while something does.
                        if (!phone)
                          if (needsYouCount(workspace) case final count
                              when count > 0) ...[
                            _NeedsYouRow(count: count),
                            const SizedBox(height: Space.s),
                          ],
                        ...(focused != null
                            ? [
                                if (pinned != null) ...[
                                  _Place(
                                    projectId: pinned.projectId,
                                    header: null,
                                    agents: [pinned],
                                    selectedAgentId: selected,
                                  ),
                                  const SizedBox(height: Space.s),
                                ],
                                // What's out of sight elsewhere glows here.
                                Glow(
                                  store: backend.highlights,
                                  matches: (key) => _elsewhere(
                                    key,
                                    workspace,
                                    focused!.id,
                                    shown: pinned?.id,
                                  ),
                                  child: _QuietRow(
                                    label: '‹ All projects',
                                    waiting: waitingIn([
                                      for (final agent in workspace.agents)
                                        if (agent.projectId != focused.id &&
                                            agent.id != pinned?.id &&
                                            archivedProjectOf(
                                                  workspace,
                                                  agent,
                                                ) ==
                                                null)
                                          agent,
                                    ]),
                                    onTap: () {
                                      AppScope.scope(context).history.clear();
                                      fireAndLog(backend.focusProject(''));
                                    },
                                  ),
                                ),
                                const SizedBox(height: Space.s),
                                _Place(
                                  projectId: focused.id,
                                  header: _ProjectHeading(project: focused),
                                  folders: focused.folders,
                                  agents: backend.workspace.agentsIn(
                                    focused.id,
                                  ),
                                  selectedAgentId: selected,
                                ),
                              ]
                            : [
                                // Rivet and any other workspace agents, on
                                // their own at the top.
                                if (backend.workspace
                                    .agentsIn('')
                                    .isNotEmpty) ...[
                                  _Place(
                                    projectId: '',
                                    header: null,
                                    agents: backend.workspace.agentsIn(''),
                                    selectedAgentId: selected,
                                  ),
                                  const SizedBox(height: Space.l),
                                ],
                                _Caption(
                                  'Projects',
                                  trailing: _AskRivet(
                                    starter: 'Set up a new project for ',
                                    fallback: (context, close) =>
                                        CreateProjectForm(close: close),
                                  ),
                                ),
                                for (final project in workspace.projects)
                                  if (!project.archived)
                                    _Place(
                                      projectId: project.id,
                                      header: _ProjectRow(project: project),
                                      folders: project.folders,
                                      agents: backend.workspace.agentsIn(
                                        project.id,
                                      ),
                                      selectedAgentId: selected,
                                      indent: Space.l,
                                    ),
                                // Out of the way, and back with a click.
                                if (workspace.projects.any((p) => p.archived))
                                  _Archived(
                                    projects: [
                                      for (final project in workspace.projects)
                                        if (project.archived) project,
                                    ],
                                  ),
                              ]),
                        // The person's own chats, under every project.
                        _Chats(
                          chats: chatsNewestFirst(workspace),
                          selectedId: page == AppPage.agent
                              ? workspace.selectedChatId
                              : '',
                        ),
                        if (phone) _PhoneFooter(workspace: workspace),
                      ],
                    ),
                  ),
                ),
                if (!phone)
                  _Footer(
                    workspace: workspace,
                    connected: backend.connected.value,
                    page: page,
                  ),
              ],
            );
          },
        ),
      ),
    );
  }
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
/// except the agent [shown] beside it (Rivet, pinned at the top).
bool _elsewhere(
  String key,
  Workspace workspace,
  String projectId, {
  String? shown,
}) {
  if (key.startsWith('project:')) return key != 'project:$projectId';
  if (key.startsWith('folder:')) return !key.startsWith('folder:$projectId/');
  final others = {
    for (final agent in workspace.agents)
      if (agent.projectId != projectId &&
          agent.id != shown &&
          archivedProjectOf(workspace, agent) == null)
        agent.id,
  };
  return matchesAgents(key, others);
}

/// A section label. Labels name groups; rows are things you can act on.
class _Caption extends StatelessWidget {
  const _Caption(this.text, {this.trailing, this.header = false});

  final String text;

  /// It heads a section, for VoiceOver.
  final bool header;

  /// A small button at its end, such as [_AskRivet].
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final Widget label = Semantics(
      // A heading is an element of its own, not read into the row under it.
      container: header,
      header: header,
      child: Text(
        text,
        style: TypeScale.of(context).caption
            .copyWith(color: palette.textFaint, fontWeight: FontWeight.w500),
      ),
    );
    final trailing = this.trailing;
    if (trailing == null) {
      return Padding(
        padding: const EdgeInsets.fromLTRB(Space.s, Space.s, Space.s, Space.xs),
        child: label,
      );
    }
    // The same 28px as without it: the 20px button takes the padding's
    // place.
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.xs, Space.xs, Space.xs),
      child: Row(
        children: [
          Expanded(child: label),
          trailing,
        ],
      ),
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
    return HoverCard(
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
    // The agents in the project's own folder (or one it no longer has),
    // then each folder's. Each is its own tree: an agent whose parent is in
    // another folder stands at its folder's top level.
    final folderIds = {for (final folder in folders) folder.id};
    final workspace = backend.workspace.workspace;
    final pipes = projectId.isEmpty
        ? const <Pipe>[]
        : pipesIn(workspace, projectId);
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

    // Reports sit in under their lead, up to a few levels: deeper ones
    // line up with the last, so names keep their room.
    final cap = touchFirst(context) ? 3 : 4;
    Widget branch(_Node node, List<_Node> siblings, [int depth = 0]) {
      final children = node.children;
      final open = children.isNotEmpty && !node.agent.collapsed;
      final inset = depth < cap ? Space.l : 0.0;
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _AgentRow(
            agent: node.agent,
            projectId: projectId,
            folderId: folderIds.contains(node.agent.folderId)
                ? node.agent.folderId
                : '',
            selected: node.agent.id == selectedAgentId,
            onTap: () => select(node.agent),
            onReach: () => select(node.agent, reach: true),
            gutter: gutter,
            children: children,
            siblings: [for (final sibling in siblings) sibling.agent.id],
            inset: inset,
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

    return DragTarget<String>(
      // Takes what the rows don't: to this place's top level, last.
      onWillAcceptWithDetails: (details) {
        final agent = backend.workspace.agent(details.data);
        return agent != null &&
            (agent.projectId != projectId ||
                agent.parentId.isNotEmpty ||
                agent.folderId.isNotEmpty);
      },
      onMove: (details) {
        final agent = backend.workspace.agent(details.data);
        if (agent == null) return;
        _dropSays.value = _sayDrop(
          backend.workspace.workspace,
          agent,
          _toPlace(agent),
        );
      },
      onLeave: (_) => _dropSays.value = null,
      onAcceptWithDetails: (details) {
        _dropSays.value = null;
        final agent = backend.workspace.agent(details.data);
        if (agent != null) _arrangeOrSay(context, agent, _toPlace(agent));
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
                  for (final root in roots) branch(root, roots),
                  for (final folder in folders)
                    _FolderGroup(
                      projectId: projectId,
                      folder: folder,
                      roots: _tree([
                        for (final agent in agents)
                          if (agent.folderId == folder.id) agent,
                      ]),
                      branch: branch,
                    ),
                  // Its pipes to other Claktories, after its agents.
                  for (final pipe in pipes)
                    _PipeRow(
                      pipe: pipe,
                      selected: pipe.id == selectedPipeId,
                      onTap: () => _selectPipe(context, pipe),
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

/// One of a project's folders in the sidebar: a quiet heading that folds
/// its agents away, and its agents nested under it. Dropping an agent on
/// the heading puts it in the folder, at its top level.
class _FolderGroup extends StatefulWidget {
  const _FolderGroup({
    required this.projectId,
    required this.folder,
    required this.roots,
    required this.branch,
  });

  final String projectId;
  final ProjectFolder folder;
  final List<_Node> roots;
  final Widget Function(_Node node, List<_Node> siblings) branch;

  @override
  State<_FolderGroup> createState() => _FolderGroupState();
}

class _FolderGroupState extends State<_FolderGroup> {
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
    final hidden = folder.collapsed ? _all(roots) : const <Agent>[];
    final working = hidden.any(atWork);
    final unseen = hidden.any((agent) => agent.unseen);
    final waiting = waitingIn(hidden);
    final heading = DragTarget<String>(
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
        child: Semantics(
          button: true,
          label: folder.collapsed
              ? 'Show agents in ${folder.name}'
              : 'Hide agents in ${folder.name}',
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
        ),
      ),
    );
    final hiddenIds = {for (final agent in hidden) agent.id};
    final shown = backend.workspace.workspace.selectedAgentId;
    return Column(
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
                  padding: const EdgeInsets.only(left: Space.m + 2),
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
    this.underlay,
    this.height = 28,
  });

  final VoidCallback onTap;

  /// Agents' rows are taller, for their characters.
  final double height;

  /// Behind its content, across the whole row, such as a [WorkingWave].
  final Widget? underlay;

  /// A right click.
  final VoidCallback? onSecondaryTap;
  final bool selected;
  final Widget Function(BuildContext context, bool hovered) builder;

  @override
  State<_Row> createState() => _RowState();
}

class _RowState extends State<_Row> {
  var _hovered = false;
  var _focused = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = touchFirst(context);
    return FocusableActionDetector(
      mouseCursor: SystemMouseCursors.click,
      onShowHoverHighlight: (value) => setState(() => _hovered = value),
      onShowFocusHighlight: (value) => setState(() => _focused = value),
      actions: {
        ActivateIntent: CallbackAction<ActivateIntent>(
          onInvoke: (_) {
            widget.onTap();
            return null;
          },
        ),
      },
      child: GestureDetector(
        onTap: widget.onTap,
        onSecondaryTap: widget.onSecondaryTap,
        // A phone's right click.
        onLongPress: widget.onSecondaryTap,
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
  final files = PhoneProjects.of(context)?.files;
  final chosen = await showActions<String>(
    context,
    title: project.name,
    actions: [
      if (files != null)
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
      files?.call(project);
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
    return HoverCard(
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
                color: shown || hovered ? palette.textMuted : palette.textFaint,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A project: a folder glyph and its name, and on hover a Files button and
/// a "+" that asks Rivet for a new agent in it. A click focuses it; a right
/// click opens its settings (rename, delete).
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
/// hover. A click or right click opens its settings (rename, delete); on a
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
            if (hovered || filesShown(context, project.id)) ...[
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
  });

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

  Agent get agent => widget.agent;

  bool get _open => widget.children.isNotEmpty && !agent.collapsed;

  _Drop _zone(Offset global) {
    final box = context.findRenderObject()! as RenderBox;
    final y = box.globalToLocal(global).dy / box.size.height;
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

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    // It glows for itself, and for what's folded under it.
    final stands = {
      agent.id,
      if (agent.collapsed)
        for (final hidden in _all(widget.children)) hidden.id,
    };
    final shown = backend.workspace.workspace.selectedAgentId;
    final row = Glow(
      store: backend.highlights,
      matches: (key) => matchesAgents(key, stands, shownAgentId: shown),
      child: _row(palette, backend),
    );
    final draggable = _SidebarDraggable<String>(
      data: agent.id,
      dragAnchorStrategy: pointerDragAnchorStrategy,
      onDragEnd: (_) => _dropSays.value = null,
      feedback: _DragFeedback(name: agent.name, palette: palette),
      childWhenDragging: Opacity(opacity: 0.4, child: row),
      child: row,
    );
    return DragTarget<String>(
      onWillAcceptWithDetails: (details) =>
          _accepts(details.data) || _isOwn(details.data),
      onMove: (details) {
        if (!_accepts(details.data)) {
          _dropSays.value = null;
          return;
        }
        final drop = _zone(details.offset);
        final dragged = backend.workspace.agent(details.data)!;
        _dropSays.value = _sayDrop(
          backend.workspace.workspace,
          dragged,
          _arrange(dragged, drop),
        );
        if (drop != _drop) setState(() => _drop = drop);
      },
      onLeave: (_) {
        _dropSays.value = null;
        setState(() => _drop = null);
      },
      onAcceptWithDetails: (details) {
        _dropSays.value = null;
        if (_isOwn(details.data)) return;
        final dragged = backend.workspace.agent(details.data);
        final drop = _drop ?? _zone(details.offset);
        setState(() => _drop = null);
        if (dragged != null) {
          _arrangeOrSay(context, dragged, _arrange(dragged, drop));
        }
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
  }

  Widget _row(Palette palette, Backend backend) {
    // A collapsed agent speaks for the agents hidden under it: it pulses
    // while one works and is marked while one has news.
    final hidden = agent.collapsed ? _all(widget.children) : const <Agent>[];
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
    return _Row(
      // Taller with a role or a wait, for its line under the name.
      height: agent.role.isEmpty && wait.isEmpty ? 32 : 40,
      selected: widget.selected,
      onTap: widget.onTap,
      // While it (or an agent folded under it) works, a wave of its color
      // sweeps across the row; it says so plainer than a pulse.
      underlay: working
          ? WorkingWave(color: color, radius: Radii.control - 1)
          : null,
      builder: (context, hovered) => Row(
        children: [
          if (widget.gutter)
            SizedBox(
              // On a phone, wide enough for a fingertip, the row's height.
              width: touchFirst(context) ? _phoneAvatar(context) : Space.m + 2,
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
                          backend.setAgentCollapsed(agent.id, !agent.collapsed),
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
            child: _NameOverRole(
              name: agent.name,
              role: wait.isEmpty ? agent.role : wait,
              style: TypeScale.of(context).body.copyWith(
                color: widget.selected || hovered || stands
                    ? palette.text
                    : palette.textMuted,
                fontWeight: stands
                    ? FontWeight.w600
                    : widget.selected
                    ? FontWeight.w500
                    : FontWeight.w400,
              ),
              roleColor: widget.selected || hovered
                  ? palette.textMuted
                  : palette.textFaint,
            ),
          ),
          // Whom it reports to, for VoiceOver: what the nesting shows, and
          // an admin lead (Rivet) the nesting can't.
          if (backend.workspace.agent(agent.leadId) case final lead?)
            Semantics(
              label: 'reports to ${lead.name}',
              child: const SizedBox(),
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
        ],
      ),
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
/// something new. A click opens it.
class _PipeRow extends StatelessWidget {
  const _PipeRow({
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
    final waiting = pipeWaiting(pipe);
    final unseen = pipe.unseen;
    final stands = unseen || waiting > 0;
    final closed = pipe.state == PipeState.PIPE_STATE_CLOSED;
    return Semantics(
      label: 'Pipe',
      child: _Row(
        key: ValueKey('pipe-row/${pipe.id}'),
        height: 40,
        selected: selected,
        onTap: onTap,
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
                role: pipeStatus(pipe),
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
  }
}

/// On a Mac, "Needs you" at the top of the sidebar while anything,
/// anywhere, waits for the person's answer: the inbox glyph, the words, and
/// the count in the needs-you pill. A click shows the Needs you page beside
/// the conversation, or closes it.
class _NeedsYouRow extends StatelessWidget {
  const _NeedsYouRow({required this.count});

  final int count;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return _Row(
      key: const ValueKey('needs-you'),
      selected: AppScope.pageShown(context, AppPage.inbox),
      onTap: () => AppScope.togglePage(context, AppPage.inbox),
      builder: (context, hovered) => Row(
        children: [
          SizedBox(
            width: Space.l + 2,
            child: Align(
              alignment: Alignment.centerLeft,
              child: ExtensionGlyph(
                icon: 'inbox',
                name: 'Needs you',
                color: palette.text,
                size: 14,
              ),
            ),
          ),
          Expanded(
            child: Text(
              'Needs you',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyles.body.copyWith(
                color: palette.text,
                fontWeight: FontWeight.w600,
              ),
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

  void _new(BuildContext context) {
    startNewChat(context);
    _toConversation(context);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final listed = chats.take(chatsListed).toList();
    final rest = chats.skip(chatsListed).toList();
    return Padding(
      padding: const EdgeInsets.only(top: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _Caption(
            'Chats',
            header: true,
            trailing: _QuietPlus(tip: 'New chat', onTap: () => _new(context)),
          ),
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
              waiting: rest.fold<int>(
                0,
                (sum, chat) => sum + chatWaiting(chat),
              ),
              waitingLabel: 'A chat not shown here is waiting for your answer',
              onTap: () => _showAll(context),
            ),
        ],
      ),
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
    return Semantics(
      label: 'Chat',
      child: _Row(
        selected: selected,
        onTap: () {
          openChat(context, chat);
          _toConversation(context);
        },
        onSecondaryTap: () => _chatMenu(context, chat),
        // While it replies; in no one's color, as a chat is no one.
        underlay: chat.status == AgentStatus.AGENT_STATUS_WORKING
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
              child: Text(
                title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
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
              ),
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

/// A chat's right click or touch and hold: Rename or Delete.
Future<void> _chatMenu(BuildContext context, Chat chat) async {
  final chosen = await showActions<String>(
    context,
    title: chatTitle(chat),
    actions: [
      const SheetAction('Rename', 'rename'),
      const SheetAction('Delete', 'delete', destructive: true),
    ],
  );
  if (!context.mounted) return;
  switch (chosen) {
    case 'rename':
      renameChat(context, chat);
    case 'delete':
      deleteChat(context, chat);
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

/// On a phone, what waits for the person's answer, first in the drawer:
/// "Needs you" and a row for each agent or pipe with something waiting,
/// which opens it at the oldest card waiting.
class _NeedsYou extends StatelessWidget {
  const _NeedsYou({required this.waiting});

  /// The agents, pipes and chats waiting ([_NeedsYou.of]).
  final List<Object> waiting;

  /// The agents, pipes and chats with something waiting for the person,
  /// in the sidebar's order; archived projects' wait.
  static List<Object> of(Workspace workspace) => [
    for (final agent in workspace.agents)
      if (waitingIn([agent]) > 0 && archivedProjectOf(workspace, agent) == null)
        agent,
    for (final pipe in workspace.pipes)
      if (pipeWaiting(pipe) > 0) pipe,
    for (final chat in chatsNewestFirst(workspace))
      if (chatWaiting(chat) > 0) chat,
  ];

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: Space.l),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Every card waiting, on one page, to answer there: See all at the
        // heading's end, or under it at the largest text sizes.
        if (largeText(context)) ...[
          const _Caption('Needs you', header: true),
          const Align(
            alignment: AlignmentDirectional.centerStart,
            child: _SeeAll(),
          ),
        ] else
          const _Caption('Needs you', header: true, trailing: _SeeAll()),
        for (final item in waiting)
          if (item is Chat)
            _ChatRow(
              key: ValueKey('needs-you/chat/${item.id}'),
              chat: item,
              selected: false,
            )
          else
            _FlatRow(item: item, reach: true),
      ],
    ),
  );
}

/// On a phone, "See all" at the end of the drawer's Needs you: the Needs
/// you page, every card waiting, to answer there without opening each
/// conversation. A finger's size; the words in `signal`, as iOS tints a
/// section's See All.
class _SeeAll extends StatelessWidget {
  const _SeeAll();

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    void open() => AppScope.openPage(context, AppPage.inbox);
    // An element of its own for VoiceOver, with its own tap: not merged
    // into the section around it.
    return Semantics(
      container: true,
      button: true,
      label: 'See everything that needs you',
      onTap: open,
      excludeSemantics: true,
      child: Pressable(
        onTap: open,
        builder: (context, pressed) => ConstrainedBox(
          constraints: BoxConstraints(
            minHeight: PhonePlatformPolicy.least(
              context,
              DenseList.of(context) ? Touch.denseLine : Touch.target,
            ),
            minWidth: PhonePlatformPolicy.least(context, Touch.target),
          ),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            child: Center(
              widthFactor: 1,
              child: Text(
                'See all',
                style: TypeScale.of(context).secondary.copyWith(
                  color: pressed
                      ? palette.signal.withValues(alpha: 0.6)
                      : palette.signal,
                  fontWeight: FontWeight.w500,
                ),
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
            if (archivedProjectOf(workspace, agent) == null &&
                (has(agent.name) ||
                    has(agent.role) ||
                    has(projectName(agent.projectId))))
              agent,
          for (final pipe in workspace.pipes)
            if (has(pipe.name) || has(projectName(pipe.projectId))) pipe,
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

/// An agent or a pipe on a row of its own, outside the tree (Needs you, a
/// search): its character or portal, its name over its role or status
/// (and with [project], its project's name), and what waits. A tap opens
/// it; with [reach], at the oldest card waiting.
class _FlatRow extends StatelessWidget {
  const _FlatRow({
    required this.item,
    this.reach = false,
    this.project = false,
  });

  /// An [Agent] or a [Pipe].
  final Object item;
  final bool reach;
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
      waiting = pipeWaiting(item);
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
      onTap = () => _selectAgent(context, agent, reach: reach);
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
  const _PhoneFooter({required this.workspace});

  final Workspace workspace;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    final type = TypeScale.of(context);
    Widget place(
      AppPage page,
      String label,
      Widget Function(Color color) glyph, {
      String? glow,
    }) {
      final row = _Row(
        onTap: () => AppScope.openPage(context, page),
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
          // How much of each provider's limits is used, a touch and hold
          // for its resets.
          if (workspace.providers.any((p) => p.signedInWith.isNotEmpty))
            Padding(
              padding: const EdgeInsets.fromLTRB(
                Space.xs,
                Space.s,
                Space.xs,
                0,
              ),
              child: ProviderUsageCircles(providers: workspace.providers),
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
    required this.page,
  });

  final Workspace workspace;
  final bool connected;
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
          if (workspace.providers.any((p) => p.signedInWith.isNotEmpty))
            Padding(
              padding: const EdgeInsets.fromLTRB(
                Space.xs,
                0,
                Space.xs,
                Space.xs,
              ),
              child: ProviderUsageCircles(providers: workspace.providers),
            ),
          _FooterButtons(
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
                glyph: (color) =>
                    ExtensionGlyph(icon: 'scroll-text', name: '', color: color),
              ),
              GlyphButton(
                label: 'Map',
                active: AppScope.pageShown(context, AppPage.map),
                onTap: () => AppScope.togglePage(context, AppPage.map),
                glyph: (color) =>
                    ExtensionGlyph(icon: 'network', name: 'Map', color: color),
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
/// line of 28px buttons; with a finger's targets (an iPad's sidebar,
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
    return Row(
      children: [
        for (final (i, page) in pages.indexed) ...[
          if (i > 0) const SizedBox(width: Space.xs),
          page,
        ],
        const Spacer(),
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
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              Button(label: 'Close', onPressed: widget.close),
              const SizedBox(width: Space.s),
              TellRivet(
                error: widget.message,
                about: 'The settings file has an error.',
                onTold: widget.close,
              ),
              const SizedBox(width: Space.s),
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
  if (backend.isEmpty) return 'Claktory · app $app';
  return backend == app ? 'Claktory $backend' : 'Claktory $backend · app $app';
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
    super.onDragEnd,
  });

  @override
  MultiDragGestureRecognizer createRecognizer(
    GestureMultiDragStartCallback onStart,
  ) =>
      _SlopDragRecognizer(allowedButtonsFilter: allowedButtonsFilter)
        ..onStart = onStart;
}

/// How far the pointer moves before a press becomes a drag.
const _dragSlop = 4.0;

class _SlopDragRecognizer extends MultiDragGestureRecognizer {
  _SlopDragRecognizer({super.allowedButtonsFilter}) : super(debugOwner: null);

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
