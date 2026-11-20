import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'layout.dart';
import 'pane_chrome.dart';
import 'panes.dart' show filesPane, withFiles;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'tell_rivet.dart';

/// A project's Files: its folder, its folders and its agents' checkouts,
/// as its agents see them, in a pane of the selected agent's layout (a
/// leaf of [filesKind] whose path is the project's id). Works like VS
/// Code's file tree: a click on a folder opens or closes it, and a file
/// opens in a viewer beside the pane. The rows come from the backend
/// already flattened and sorted, and follow the folders live.
class FilesPane extends StatelessWidget {
  const FilesPane({super.key, required this.projectId, required this.paneId});

  final String projectId;
  final String paneId;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: Listenable.merge([backend.explorer, backend.workspace]),
      builder: (context, _) {
        final project = projectById(backend.workspace.workspace, projectId);
        final explorer = backend.explorer.of(projectId);
        return LayoutBuilder(
          builder: (context, constraints) => PaneScaffold(
            details: project?.name ?? '',
            controls: [
              if (project != null && explorer != null)
                ShowEverything(
                  project: project,
                  explorer: explorer,
                  // A narrow pane keeps only the check, named on hover.
                  compact: constraints.maxWidth < 320,
                ),
            ],
            body: ColoredBox(
              color: palette.sidebar,
              child: project == null
                  ? const FilesNote('This project was deleted.')
                  : FilesTree(
                      project: project,
                      explorer: explorer,
                      onOpen: (path) => openFile(context, path, beside: paneId),
                    ),
            ),
          ),
        );
      },
    );
  }
}

/// The project with [id], or null.
Project? projectById(Workspace workspace, String id) {
  for (final project in workspace.projects) {
    if (project.id == id) return project;
  }
  return null;
}

/// Opens [project]'s Files: on a Mac a pane right of the selected agent's
/// conversation (or the pane glows when it's open already); on a phone
/// [phone] shows it as a page of its own.
void openFiles(
  BuildContext context,
  Project project, {
  void Function(Project project)? phone,
}) {
  if (phone != null) return phone(project);
  final scope = AppScope.scope(context);
  final backend = scope.backend;
  // With no agent selected (an empty project focused), beside one of the
  // project's agents, or any.
  final agent =
      backend.workspace.selectedAgent ??
      backend.workspace.agentsIn(project.id).firstOrNull ??
      backend.workspace.workspace.agents.firstOrNull;
  if (agent == null) return;
  if (backend.workspace.selectedAgent == null) {
    fireAndLog(backend.selectAgent(agent.id));
  }
  if (scope.page.value != AppPage.agent) AppScope.closePage(context);
  final root = layoutOf(agent);
  final shown = filesPane(root, project.id);
  if (shown != null) {
    backend.highlights.glow('pane:${agent.id}:${shown.id}', [
      agentColor(agent.color),
    ]);
    return;
  }
  fireAndLog(backend.setLayout(agent.id, withFiles(root, project)));
}

/// Whether [projectId]'s Files show in the selected agent's layout.
bool filesShown(BuildContext context, String projectId) {
  final agent = AppScope.of(context).workspace.selectedAgent;
  return agent != null && filesPane(layoutOf(agent), projectId) != null;
}

/// Shows the files .factoryignore hides, or hides them again: a check and
/// "Show hidden files", in a Files pane's header.
class ShowEverything extends StatelessWidget {
  const ShowEverything({
    super.key,
    required this.project,
    required this.explorer,
    this.compact = false,
  });

  final Project project;
  final Explorer explorer;

  /// Just the check, its name in a hover card.
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    const label = 'Show hidden files';
    final toggle = Semantics(
      button: true,
      checked: explorer.showAll,
      label: compact ? label : null,
      child: Pressable(
        onTap: () =>
            fireAndLog(backend.showAllFiles(project.id, !explorer.showAll)),
        builder: (context, hovered) => Padding(
          padding: const EdgeInsets.symmetric(horizontal: Space.xs),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              CheckMark(on: explorer.showAll),
              if (!compact) ...[
                const SizedBox(width: Space.xs + 2),
                Text(
                  label,
                  style: TextStyles.caption.copyWith(
                    color: hovered || explorer.showAll
                        ? palette.text
                        : palette.textMuted,
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
    if (!compact) return toggle;
    return HoverCard(card: (context) => const Text(label), child: toggle);
  }
}

/// A faint line where the files would be.
class FilesNote extends StatelessWidget {
  const FilesNote(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.all(Space.m),
    child: Text(
      text,
      style: TextStyles.body.copyWith(color: Palette.of(context).textFaint),
    ),
  );
}

/// The project's own folder, then each of its folders, under the folder's
/// name and path.
class FilesTree extends StatelessWidget {
  const FilesTree({
    super.key,
    required this.project,
    required this.explorer,
    required this.onOpen,
  });

  final Project project;
  final Explorer? explorer;

  /// A file was clicked: its absolute path.
  final ValueChanged<String> onOpen;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final explorer = this.explorer;
    Widget note(String text, {bool error = false}) => Padding(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.s, Space.s, 0),
      child: Text(
        text,
        style: TextStyles.body.copyWith(
          color: error ? palette.danger : palette.textFaint,
        ),
      ),
    );
    if (explorer == null) return const FilesNote('Reading the folder…');
    // Rows are built as they scroll into view.
    final rows = <Widget Function()>[];
    void folder(
      String error,
      List<FileEntry> entries,
      String folderId,
      String base, {
      String agentId = '',
    }) {
      if (error.isNotEmpty) {
        rows.add(
          () => Padding(
            padding: const EdgeInsets.fromLTRB(Space.s, Space.s, Space.s, 0),
            child: ErrorLine(
              error,
              about:
                  "The file explorer can't show a folder of ${project.name}.",
              style: TextStyles.body.copyWith(color: palette.danger),
            ),
          ),
        );
      } else if (entries.isEmpty) {
        rows.add(
          () => note(
            explorer.showAll
                ? 'This folder is empty.'
                : 'Nothing here, apart from hidden files.',
          ),
        );
      } else {
        for (final entry in entries) {
          rows.add(
            () => SizedBox(
              height: 24,
              child: _FileRow(
                onOpen: onOpen,
                projectId: project.id,
                folderId: folderId,
                agentId: agentId,
                base: base,
                entry: entry,
              ),
            ),
          );
        }
      }
    }

    // The project's own folder shows when there's something in it to see,
    // or nothing else to show.
    final ownFolder =
        explorer.folders.isEmpty ||
        explorer.entries.isNotEmpty ||
        explorer.error.isNotEmpty;
    if (ownFolder) folder(explorer.error, explorer.entries, '', project.path);
    final names = {
      for (final folder in project.folders) folder.id: folder.name,
    };
    final store = AppScope.of(context).workspace;
    for (final root in explorer.folders) {
      final first = rows.isEmpty;
      // An agent's own checkout (Own checkout) after the project's folders.
      final checkout = root.agentId.isNotEmpty;
      final owner = checkout ? store.agent(root.agentId) : null;
      rows.add(
        () => _RootHeading(
          name: checkout
              ? "${owner?.name ?? root.agentId}'s checkout"
              : names[root.folderId] ?? root.folderId,
          path: root.path,
          branch: checkout ? root.branch : '',
          color: owner == null ? null : agentColor(owner.color),
          first: first,
        ),
      );
      folder(
        root.error,
        root.entries,
        root.folderId,
        root.path,
        agentId: root.agentId,
      );
    }
    return ListView.builder(
      padding: const EdgeInsets.fromLTRB(Space.s, 0, Space.s, Space.m),
      itemCount: rows.length,
      itemBuilder: (context, index) => rows[index](),
    );
  }
}

/// Heads one of the project's folders: its name, and its path, faint.
class _RootHeading extends StatelessWidget {
  const _RootHeading({
    required this.name,
    required this.path,
    required this.first,
    this.branch = '',
    this.color,
  });

  final String name;
  final String path;

  /// An agent's checkout: the branch it's on, and the agent's color.
  final String branch;
  final Color? color;

  /// Nothing is above it, so it needs no hairline or space to set it apart.
  final bool first;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      margin: EdgeInsets.only(top: first ? 0 : Space.l),
      padding: EdgeInsets.fromLTRB(
        Space.xs,
        first ? Space.xs : Space.s,
        Space.s,
        Space.xs,
      ),
      decoration: BoxDecoration(
        border: first ? null : Border(top: BorderSide(color: palette.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              SizedBox(
                width: 14,
                child: Center(
                  child: branch.isEmpty
                      ? FolderGlyph(color: palette.textMuted)
                      : ExtensionGlyph(
                          icon: 'git-branch',
                          name: name,
                          color: color ?? palette.textMuted,
                          size: 14,
                        ),
                ),
              ),
              const SizedBox(width: Space.xs),
              Expanded(
                child: Text(
                  name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyles.label.copyWith(color: palette.text),
                ),
              ),
            ],
          ),
          Padding(
            padding: const EdgeInsets.only(left: 14 + Space.xs, top: 2),
            child: Text(
              branch.isEmpty ? shortPath(path) : '$branch · ${shortPath(path)}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          ),
        ],
      ),
    );
  }
}

String _join(String base, String path) =>
    base.endsWith('/') ? '$base$path' : '$base/$path';

/// Opens [path] in a viewer beside the selected agent's conversation, as an
/// agent's `show` does, or makes its pane glow when it's open already. From
/// a Files pane ([beside]), the first viewer goes right of it.
void openFile(BuildContext context, String path, {String? beside}) {
  final scope = AppScope.scope(context);
  final backend = scope.backend;
  final agent = backend.workspace.selectedAgent;
  if (agent == null) return;
  // From Settings or the Marketplace, back to the agent.
  if (scope.page.value != AppPage.agent) AppScope.closePage(context);
  final root = layoutOf(agent);
  final opened = open(root, path, beside: beside);
  if (identical(opened.root, root)) {
    backend.highlights.glow('pane:${agent.id}:${opened.paneId}', [
      agentColor(agent.color),
    ]);
    return;
  }
  fireAndLog(backend.setLayout(agent.id, opened.root));
}

/// [path] with the home folder as "~", so its end has room to show.
String shortPath(String path) =>
    path.replaceFirst(RegExp(r'^/(Users|home)/[^/]+(?=/|$)'), '~');

class _FileRow extends StatelessWidget {
  const _FileRow({
    required this.onOpen,
    required this.projectId,
    required this.folderId,
    required this.entry,
    required this.base,
    this.agentId = '',
  });

  final ValueChanged<String> onOpen;
  final String projectId;

  /// The folder the entry's path is from.
  final String base;

  /// The agent whose checkout it's in, if it's in one.
  final String agentId;

  /// Which of the project's folders it's in; empty for the project's own.
  final String folderId;
  final FileEntry entry;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return Pressable(
      onTap: entry.isDir
          ? () => fireAndLog(
              backend.expandFolder(
                projectId,
                entry.path,
                !entry.expanded,
                folderId: folderId,
                agentId: agentId,
              ),
            )
          : () => onOpen(_join(base, entry.path)),
      builder: (context, hovered) {
        final color = hovered ? palette.text : palette.textMuted;
        return Opacity(
          opacity: entry.hidden ? 0.5 : 1,
          child: Container(
            padding: EdgeInsets.only(
              left: Space.xs + entry.depth * 12.0,
              right: Space.s,
            ),
            decoration: BoxDecoration(
              color: hovered ? palette.hover : const Color(0x00000000),
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Row(
              children: [
                SizedBox(
                  width: 14,
                  child: entry.isDir
                      ? Center(
                          child: Chevron(
                            color: palette.textFaint,
                            down: entry.expanded,
                          ),
                        )
                      : null,
                ),
                const SizedBox(width: Space.xs),
                entry.isDir
                    ? FolderGlyph(color: color)
                    : FileGlyph(color: color),
                const SizedBox(width: Space.s - 2),
                Expanded(
                  child: Text(
                    entry.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyles.body.copyWith(color: color),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}
