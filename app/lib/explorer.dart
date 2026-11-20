import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'layout.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'tell_rivet.dart';

/// Width of the explorer beside a focused project.
const explorerWidth = 260.0;

/// The focused project's folder, as its agents see it. Works like VS Code's
/// file tree: a click on a folder opens or closes it. The rows come from the
/// backend already flattened and sorted, and follow the folder live.
class ExplorerPane extends StatelessWidget {
  const ExplorerPane({super.key, required this.project});

  final Project project;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border(left: BorderSide(color: palette.hairline)),
      ),
      child: SizedBox(
        width: explorerWidth,
        child: ListenableBuilder(
          listenable: backend.explorer,
          builder: (context, _) {
            final explorer = backend.explorer.of(project.id);
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                _Header(project: project, explorer: explorer),
                Expanded(
                  child: _Tree(project: project, explorer: explorer),
                ),
              ],
            );
          },
        ),
      ),
    );
  }
}

/// The folder's name, and whether hidden files show.
class _Header extends StatelessWidget {
  const _Header({required this.project, required this.explorer});

  final Project project;
  final Explorer? explorer;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final explorer = this.explorer;
    final folder =
        project.path.split('/').where((part) => part.isNotEmpty).lastOrNull ??
        project.path;
    // Lines up with the agent's header bar beside it, at the top of the
    // window, so it drags the window too.
    return WindowDrag(
      child: Container(
        height: titleBarHeight,
        margin: const EdgeInsets.only(bottom: Space.s),
        padding: const EdgeInsets.fromLTRB(Space.l, 0, Space.m, 0),
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: palette.hairline)),
        ),
        child: Row(
          children: [
            Expanded(
              child: Text(
                folder,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyles.label.copyWith(color: palette.text),
              ),
            ),
            if (explorer != null)
              Pressable(
                onTap: () => fireAndLog(
                  backend.showAllFiles(project.id, !explorer.showAll),
                ),
                builder: (context, hovered) => Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    CheckMark(on: explorer.showAll),
                    const SizedBox(width: Space.xs + 2),
                    Text(
                      'Show everything',
                      style: TextStyles.caption.copyWith(
                        color: hovered || explorer.showAll
                            ? palette.text
                            : palette.textMuted,
                      ),
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

/// The project's own folder, then each of its folders, under the folder's
/// name and path.
class _Tree extends StatelessWidget {
  const _Tree({required this.project, required this.explorer});

  final Project project;
  final Explorer? explorer;

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
    if (explorer == null) {
      return Padding(
        padding: const EdgeInsets.symmetric(horizontal: Space.s),
        child: note('Reading the folder…'),
      );
    }
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
/// agent's `show` does, or makes its pane glow when it's open already.
void openFile(BuildContext context, String path) {
  final scope = AppScope.scope(context);
  final backend = scope.backend;
  final agent = backend.workspace.selectedAgent;
  if (agent == null) return;
  // From Settings or the Marketplace, back to the agent.
  if (scope.page.value != AppPage.agent) AppScope.closePage(context);
  final root = layoutOf(agent);
  final opened = open(root, path);
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
    required this.projectId,
    required this.folderId,
    required this.entry,
    required this.base,
    this.agentId = '',
  });

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
          : () => openFile(context, _join(base, entry.path)),
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
