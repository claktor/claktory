import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_name.dart';
import 'avatar.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'connection.dart';
import 'explorer.dart' show filesShown, openFiles;
import 'controls.dart';
import 'forms.dart';
import 'highlights.dart';
import 'identity.dart';
import 'loopback_forward.dart' show ForwardingsLine;
import 'markdown.dart';
import 'phone.dart' show PhoneProjects;
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
    return DecoratedBox(
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border(right: BorderSide(color: palette.hairline)),
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
            final selected = page == AppPage.agent
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
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                // The traffic lights' row.
                const TitleSpace(),
                Expanded(
                  child: FocusTraversalGroup(
                    child: ListView(
                      padding: const EdgeInsets.fromLTRB(
                        Space.s,
                        0,
                        Space.s,
                        Space.m,
                      ),
                      children: focused != null
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
                                          archivedProjectOf(workspace, agent) ==
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
                                agents: backend.workspace.agentsIn(focused.id),
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
                            ],
                    ),
                  ),
                ),
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
  const _Caption(this.text, {this.trailing});

  final String text;

  /// A small button at its end, such as [_AskRivet].
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final label = Text(
      text,
      style: TextStyles.caption.copyWith(
        color: palette.textFaint,
        fontWeight: FontWeight.w500,
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
    if (!askRivet(context, starter)) showModal(context, fallback);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return HoverCard(
      card: (context) => const Text(_tip),
      child: Semantics(
        button: true,
        label: _tip,
        child: Pressable(
          onTap: () => _ask(context),
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

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    // The agents in the project's own folder (or one it no longer has),
    // then each folder's. Each is its own tree: an agent whose parent is in
    // another folder stands at its folder's top level.
    final folderIds = {for (final folder in folders) folder.id};
    final roots = _tree([
      for (final agent in agents)
        if (!folderIds.contains(agent.folderId)) agent,
    ]);
    // Rows keep room for a disclosure once any agent here has others under
    // it, so the dots stay in a column.
    final gutter = agents.any(
      (agent) => agents.any((other) => other.parentId == agent.id),
    );
    void select(Agent agent) {
      AppScope.closePage(context);
      fireAndLog(() async {
        await backend.selectAgent(agent.id);
        // Its conversation is here now: it opens at what waits for the
        // person, if anything does.
        final now = backend.workspace.agent(agent.id);
        if (now != null) backend.reveal.oldestWaiting(now);
      }());
      // On a phone, its conversation is pushed over the list.
      PhoneNavigation.of(context)?.value = true;
    }

    Widget branch(_Node node, List<_Node> siblings) {
      final children = node.children;
      final open = children.isNotEmpty && !node.agent.collapsed;
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
            gutter: gutter,
            children: children,
            siblings: [for (final sibling in siblings) sibling.agent.id],
          ),
          if (children.isNotEmpty)
            AnimatedSize(
              duration: Motion.base,
              curve: Curves.easeOut,
              alignment: Alignment.topCenter,
              child: open
                  ? Padding(
                      padding: const EdgeInsets.only(left: Space.l),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          for (final child in children) branch(child, children),
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
      onAcceptWithDetails: (details) {
        final agent = backend.workspace.agent(details.data);
        final arrange = ArrangeAgent(agentId: details.data);
        if (agent != null && agent.projectId != projectId) {
          arrange.projectId = projectId;
        }
        // Out of any folder, into the project's own.
        if (agent != null && agent.folderId.isNotEmpty) arrange.folderId = '';
        fireAndLog(backend.arrangeAgent(arrange));
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
      onAcceptWithDetails: (details) {
        final agent = backend.workspace.agent(details.data);
        final arrange = ArrangeAgent(
          agentId: details.data,
          folderId: folder.id,
        );
        if (agent != null && agent.projectId != widget.projectId) {
          arrange.projectId = widget.projectId;
        }
        fireAndLog(backend.arrangeAgent(arrange));
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
                      style: TextStyles.caption.copyWith(
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
                        style: TextStyles.caption.copyWith(
                          color: palette.textFaint,
                        ),
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
          height: widget.height,
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

/// A project's right click: its settings; on a phone, a touch and hold
/// offers its Files too.
void _projectMenu(BuildContext context, Project project) {
  final phone = PhoneProjects.of(context);
  if (phone != null) return phone.menu(project);
  _openProjectSettings(context, project);
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
              style: TextStyles.body.copyWith(
                color: hovered ? palette.text : palette.textMuted,
              ),
            ),
          ),
          if (hovered || filesShown(context, project.id)) ...[
            _FilesButton(
              project: project,
              shown: filesShown(context, project.id),
            ),
            const SizedBox(width: Space.xs),
          ],
          if (hovered) _askForAgent(project),
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
Widget _askForAgent(Project project) => _AskRivet(
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
              style: TextStyles.label.copyWith(color: palette.text),
            ),
          ),
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
    required this.gutter,
    required this.children,
    required this.siblings,
  });

  final Agent agent;

  /// The place it's in: a project's id, or empty for the workspace.
  final String projectId;

  /// The project's folder it shows in; empty for the project's own.
  final String folderId;
  final bool selected;
  final VoidCallback onTap;

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
      feedback: _DragFeedback(name: agent.name, palette: palette),
      childWhenDragging: Opacity(opacity: 0.4, child: row),
      child: row,
    );
    return DragTarget<String>(
      onWillAcceptWithDetails: (details) =>
          _accepts(details.data) || _isOwn(details.data),
      onMove: (details) {
        if (!_accepts(details.data)) return;
        final drop = _zone(details.offset);
        if (drop != _drop) setState(() => _drop = drop);
      },
      onLeave: (_) => setState(() => _drop = null),
      onAcceptWithDetails: (details) {
        if (_isOwn(details.data)) return;
        final dragged = backend.workspace.agent(details.data);
        final drop = _drop ?? _zone(details.offset);
        setState(() => _drop = null);
        if (dragged != null) {
          fireAndLog(backend.arrangeAgent(_arrange(dragged, drop)));
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
              left: _drop == _Drop.after && _open ? Space.l : 0,
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
              width: Space.m + 2,
              child: widget.children.isEmpty
                  ? null
                  : Semantics(
                      button: true,
                      label: agent.collapsed
                          ? 'Show agents under ${agent.name}'
                          : 'Hide agents under ${agent.name}',
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
            child: AgentCharacter(agent: agent, size: 24, live: true),
          ),
          Expanded(
            child: NameOverRole(
              name: agent.name,
              role: wait.isEmpty ? agent.role : wait,
              style: TextStyles.body.copyWith(
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
          if (agent.queuedMessages > 0)
            Text(
              '${agent.queuedMessages}',
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          if (waiting > 0)
            _needsYou(waitingLabel(agent.name, waiting), waiting)
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
    return Transform.translate(
      offset: const Offset(8, -14),
      child: Container(
        height: 28,
        padding: const EdgeInsets.symmetric(horizontal: Space.m),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: palette.sidebar,
          borderRadius: BorderRadius.circular(Radii.control),
          border: Border.all(color: palette.hairline),
          boxShadow: floatingShadow,
        ),
        child: Text(
          name,
          style: TextStyles.body.copyWith(
            color: palette.text,
            decoration: TextDecoration.none,
          ),
        ),
      ),
    );
  }
}

class _QuietRow extends StatelessWidget {
  const _QuietRow({required this.label, required this.onTap, this.waiting = 0});

  final String label;
  final VoidCallback onTap;

  /// What waits for the person out of sight behind it: marked at its end.
  final int waiting;

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
              style: TextStyles.body.copyWith(
                color: hovered ? palette.textMuted : palette.textFaint,
              ),
            ),
          ),
          if (waiting > 0)
            _needsYou(
              'Something in another project is waiting for your answer',
              waiting,
            ),
        ],
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
                    style: TextStyles.body.copyWith(
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
                        style: TextStyles.body.copyWith(
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
              style: TextStyles.caption.copyWith(
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
    final configError = workspace.configError;
    final scope = AppScope.scope(context);
    final appVersion = scope.appBuild.version;
    void openSettings() => AppScope.openProviders(context);
    final status = <Widget>[];
    if (workspace.hasLogin() &&
        !AppScope.pageShown(context, AppPage.settings)) {
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
                  style: TextStyles.caption.copyWith(
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
            (context, close) =>
                _ConfigError(message: configError, close: close),
          ),
          builder: (context, hovered) => Text(
            'Settings file has an error',
            style: TextStyles.caption.copyWith(
              color: palette.danger,
              decoration: hovered ? TextDecoration.underline : null,
              decorationColor: palette.danger,
            ),
          ),
        ),
      );
    }
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
          Row(
            children: [
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
              const SizedBox(width: Space.xs),
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
              const SizedBox(width: Space.xs),
              GlyphButton(
                label: "What's new",
                active: AppScope.pageShown(context, AppPage.changelog),
                onTap: () => AppScope.togglePage(context, AppPage.changelog),
                glyph: (color) =>
                    ExtensionGlyph(icon: 'scroll-text', name: '', color: color),
              ),
              const SizedBox(width: Space.xs),
              GlyphButton(
                label: 'Map',
                active: AppScope.pageShown(context, AppPage.map),
                onTap: () => AppScope.togglePage(context, AppPage.map),
                glyph: (color) =>
                    ExtensionGlyph(icon: 'network', name: 'Map', color: color),
              ),
              const SizedBox(width: Space.xs),
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
              const Spacer(),
              // The person, at the end: their profile is at the top of
              // Settings.
              GlyphButton(
                label: 'Profile',
                active: false,
                onTap: () => AppScope.openPage(context, AppPage.settings),
                glyph: (_) =>
                    const ExcludeSemantics(child: PersonAvatar(size: 20)),
              ),
            ],
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
/// click with a slight jiggle became a drag (and no click).
class _SidebarDraggable<T extends Object> extends Draggable<T> {
  const _SidebarDraggable({
    required super.child,
    required super.feedback,
    super.data,
    super.childWhenDragging,
    super.dragAnchorStrategy,
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
      _SlopPointerState(event.position, event.kind, gestureSettings);

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
