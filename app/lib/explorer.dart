import 'dart:async';
import 'dart:io' show File;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_menu.dart' show AgentMenu, AgentMenuItem, showFloating;
import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'ipad/document_host.dart' show ipadProjectDocument;
import 'ipad/workbench.dart' show WorkbenchScope;
import 'layout.dart';
import 'mobile/back.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'mobile/navigation.dart' show PhoneRoutes;
import 'pane_chrome.dart';
import 'panes.dart' show filesPane, withFiles;
import 'protocol/factory/v1/factory.pb.dart';
import 'needs_you_focus.dart' show leaveFocusModeToOpen;
import 'scope.dart';
import 'share_files.dart' show ShareHold, showInFinder;
import 'tell_rivet.dart';
import 'viewers/registry.dart' show viewerBody, viewerIcon, viewerTitle;
import 'visible_codes.dart' show shownName;

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
                  : Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        FilesFolderLine(project: project),
                        Expanded(
                          child: FilesTree(
                            project: project,
                            explorer: explorer,
                            onOpen: (path) =>
                                openFile(context, path, beside: paneId),
                          ),
                        ),
                      ],
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

/// Opens [project]'s Files: on a Mac or an iPad a pane right of the
/// selected agent's conversation when that agent is in the project (or the
/// pane glows when it's open already), else a page of their own
/// ([FilesPage]) that selects no other agent; on a phone [phone] shows it
/// as a page of its own.
void openFiles(
  BuildContext context,
  Project project, {
  void Function(Project project)? phone,
}) {
  if (phone != null) return phone(project);
  final scope = AppScope.scope(context);
  final backend = scope.backend;
  final workspace = backend.workspace.workspace;
  final agent = backend.workspace.selectedAgent;
  // A pipe or a chat in the agent's place would hide a pane beside it.
  if (agent == null ||
      agent.projectId != project.id ||
      workspace.selectedPipeId.isNotEmpty ||
      workspace.selectedChatId.isNotEmpty) {
    // At an iPad's focused tier the sidebar over the window goes, as for
    // an agent chosen there.
    WorkbenchScope.maybeOf(context)?.closeOverlay();
    scope.history.clear();
    backend.filesPage.value = project.id;
    scope.page.value = AppPage.files;
    return;
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

/// Whether [projectId]'s Files show: as the page, or in the selected
/// agent's layout.
bool filesShown(BuildContext context, String projectId) {
  final scope = AppScope.scope(context);
  if (scope.page.value == AppPage.files) {
    return scope.backend.filesPage.value == projectId;
  }
  final agent = scope.backend.workspace.selectedAgent;
  return agent != null && filesPane(layoutOf(agent), projectId) != null;
}

/// A project's Files as a page of their own, in the content area as
/// Settings and Usage are, when the agent shown isn't in the project: the
/// project's name and Done over the tree. A file clicked opens on the page
/// itself, in place of the list, under "‹ Files", which goes back to the
/// list at the same folder and scroll; Done closes both. Which file shows
/// is navigation, like which page shows, so it's kept here and gone once
/// the page closes.
class FilesPage extends StatefulWidget {
  const FilesPage({super.key, required this.projectId});

  final String projectId;

  @override
  State<FilesPage> createState() => _FilesPageState();
}

class _FilesPageState extends State<FilesPage> {
  AppScope? _scope;

  /// The file open on the page, by its absolute path.
  String? _file;

  /// Back (an Android tablet's) goes to the list first while a file shows.
  PhoneBack? _back;
  BackHandle? _backHandle;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_scope == null) {
      _scope = AppScope.scope(context);
      // The backend sends the project's Explorer while the page shows.
      fireAndLog(_scope!.backend.watchFiles(widget.projectId));
    }
    final back = isPhoneLayout(context) ? null : PhoneBack.maybeOf(context);
    if (!identical(back, _back)) {
      _back = back;
      _backHandle?.release();
      _backHandle = null;
      _holdBack();
    }
  }

  void _holdBack() {
    if (_file == null) {
      _backHandle?.release();
      _backHandle = null;
    } else {
      _backHandle ??= _back?.registerNavigation(
        CallbackBackToken(
          onBack: () {
            if (mounted) _closeFile();
          },
        ),
      );
    }
  }

  @override
  void dispose() {
    _backHandle?.release();
    // Another project's Files in its place watch their own.
    final scope = _scope;
    if (scope != null && scope.page.value != AppPage.files) {
      fireAndLog(scope.backend.watchFiles(''));
    }
    super.dispose();
  }

  void _openFile(String path) {
    setState(() => _file = path);
    _holdBack();
  }

  void _closeFile() {
    setState(() => _file = null);
    _holdBack();
  }

  void _close() => AppScope.closePage(context);

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final phone = isPhoneLayout(context);
    final file = _file;
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): () =>
            _file != null ? _closeFile() : _close(),
      },
      child: FocusScope(
        autofocus: true,
        child: ListenableBuilder(
          listenable: Listenable.merge([backend.explorer, backend.workspace]),
          builder: (context, _) {
            final project = projectById(
              backend.workspace.workspace,
              widget.projectId,
            );
            final explorer = backend.explorer.of(widget.projectId);
            final list = Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                _bar(
                  context,
                  glyph: (color) => FolderGlyph(color: color),
                  title: 'Files',
                  details: project?.name ?? '',
                  controls: [
                    if (project != null && explorer != null)
                      ShowEverything(
                        project: project,
                        explorer: explorer,
                        // Large text keeps only the check, named on hover.
                        compact:
                            phone ||
                            MediaQuery.textScalerOf(context).scale(1) > 1.3,
                      ),
                  ],
                ),
                Expanded(
                  child: ColoredBox(
                    color: palette.sidebar,
                    child: project == null
                        ? const FilesNote('This project was deleted.')
                        : Column(
                            crossAxisAlignment: CrossAxisAlignment.stretch,
                            children: [
                              FilesFolderLine(project: project),
                              Expanded(
                                child: FilesTree(
                                  project: project,
                                  explorer: explorer,
                                  onOpen: _openFile,
                                ),
                              ),
                            ],
                          ),
                  ),
                ),
              ],
            );
            return Stack(
              fit: StackFit.expand,
              children: [
                // Kept while a file shows, so Back finds the list as it was.
                Offstage(
                  offstage: file != null,
                  child: TickerMode(enabled: file == null, child: list),
                ),
                if (file != null && project != null)
                  KeyedSubtree(
                    key: ValueKey('file/$file'),
                    child: _viewer(context, file),
                  ),
              ],
            );
          },
        ),
      ),
    );
  }

  /// [path]'s viewer on the page, its header "‹ Files", its name, then its
  /// details and controls; a phone's viewer page in a phone's window.
  Widget _viewer(BuildContext context, String path) {
    final leaf = viewerLeaf(path).leaf;
    Widget body(BuildContext context) =>
        ipadProjectDocument(context, leaf) ?? viewerBody(context, leaf);
    if (isPhoneLayout(context)) {
      return PaneChrome(
        phone: PhonePane(title: viewerTitle(leaf), onBack: _closeFile),
        child: Builder(builder: body),
      );
    }
    return PaneChrome(
      header: (details, controls, {pinned = const []}) => _bar(
        context,
        back: true,
        glyph: (color) => viewerIcon(leaf, color),
        title: viewerTitle(leaf),
        details: details,
        controls: [...controls, ...pinned],
      ),
      child: Builder(builder: body),
    );
  }

  /// The page's title bar: [back] ("‹ Files") while a file shows, the
  /// glyph, [title] and [details], the [controls], and Done.
  Widget _bar(
    BuildContext context, {
    bool back = false,
    required Widget Function(Color color) glyph,
    required String title,
    String details = '',
    List<Widget> controls = const [],
  }) {
    final palette = Palette.of(context);
    if (isPhoneLayout(context)) {
      return PhoneHeader(
        leading: HeaderButton.back(onTap: back ? _closeFile : _close),
        title: Text(
          title,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: PhoneType.emphasis.copyWith(color: palette.text),
        ),
        trailing: controls,
      );
    }
    return WindowDrag(
      child: Container(
        height: titleBarHeight,
        padding: EdgeInsets.only(
          left: back ? Space.m : Space.xl,
          right: Space.m,
        ),
        decoration: BoxDecoration(
          color: palette.canvas,
          border: Border(bottom: BorderSide(color: palette.hairline)),
        ),
        child: LayoutBuilder(
          builder: (context, box) => Row(
            children: [
              if (back) ...[
                _BackToFiles(onTap: _closeFile),
                const SizedBox(width: Space.s),
              ],
              SizedBox(
                width: 14,
                child: Center(child: glyph(palette.textMuted)),
              ),
              const SizedBox(width: Space.s),
              // The words take what's left, so the controls and Done stay at
              // the end however short they are.
              Expanded(
                child: Row(
                  children: [
                    Flexible(
                      child: Text(
                        title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TypeScale.of(context).label.copyWith(
                          color: palette.text,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    if (details.isNotEmpty) ...[
                      const SizedBox(width: Space.s),
                      Flexible(
                        child: Text(
                          details,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyles.caption.copyWith(
                            color: palette.textFaint,
                          ),
                        ),
                      ),
                    ],
                  ],
                ),
              ),
              // A narrow window cuts the viewer's controls from their end,
              // never Done.
              ConstrainedBox(
                constraints: BoxConstraints(
                  maxWidth: (box.maxWidth / 2).clamp(0, double.infinity),
                ),
                child: SingleChildScrollView(
                  scrollDirection: Axis.horizontal,
                  physics: const NeverScrollableScrollPhysics(),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: controls,
                  ),
                ),
              ),
              const SizedBox(width: Space.s),
              Button(label: 'Done', onPressed: _close),
            ],
          ),
        ),
      ),
    );
  }
}

/// "‹ Files" at the top left of a file open on the Files page: back to the
/// list.
class _BackToFiles extends StatelessWidget {
  const _BackToFiles({required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final touch = touchFirst(context);
    return Semantics(
      button: true,
      label: 'Back to Files',
      onTap: onTap,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          constraints: BoxConstraints(
            minHeight: touch ? Metrics.of(context).target : 24,
          ),
          padding: const EdgeInsets.only(left: Space.xs, right: Space.s),
          decoration: BoxDecoration(
            color: hovered ? palette.hover : const Color(0x00000000),
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              ExtensionGlyph(
                icon: 'chevron-left',
                name: '',
                color: hovered ? palette.text : palette.textMuted,
                size: 14,
              ),
              const SizedBox(width: 2),
              Text(
                'Files',
                style: TextStyles.label.copyWith(
                  color: hovered ? palette.text : palette.textMuted,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Whether this window can show the agents' files in the Finder (Explorer
/// on Windows): it's on the computer that runs them.
bool showsInFinder(Backend backend) =>
    backend.localBackend &&
    backend.target == null &&
    (defaultTargetPlatform == TargetPlatform.macOS ||
        defaultTargetPlatform == TargetPlatform.windows);

/// "Show in Finder", or Explorer on Windows.
String get showInFinderLabel => defaultTargetPlatform == TargetPlatform.windows
    ? 'Show in Explorer'
    : 'Show in Finder';

/// Where a project's files are, at the top of its Files: the folder's
/// path, cut in the middle when long (the whole of it on hover), and Show
/// in Finder on the computer that runs the agents.
class FilesFolderLine extends StatelessWidget {
  const FilesFolderLine({super.key, required this.project});

  final Project project;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final style = TextStyles.caption.copyWith(color: palette.textMuted);
    return Padding(
      padding: const EdgeInsets.fromLTRB(
        Space.s + Space.xs,
        Space.xs,
        Space.s + Space.xs,
        Space.xs,
      ),
      child: Row(
        children: [
          Expanded(
            child: HoverCard(
              card: (context) => Text(project.path),
              child: MiddleCut(shortPath(project.path), style: style),
            ),
          ),
          if (showsInFinder(backend)) ...[
            const SizedBox(width: Space.s),
            Semantics(
              button: true,
              label: showInFinderLabel,
              onTap: () => unawaited(showInFinder([File(project.path)])),
              excludeSemantics: true,
              child: Pressable(
                onTap: () => unawaited(showInFinder([File(project.path)])),
                builder: (context, hovered) => ConstrainedBox(
                  constraints: BoxConstraints(
                    minHeight: touchFirst(context)
                        ? Metrics.of(context).target
                        : 20,
                  ),
                  child: Align(
                    widthFactor: 1,
                    child: Text(
                      showInFinderLabel,
                      style: style.copyWith(
                        color: hovered ? palette.text : palette.textMuted,
                        decoration: TextDecoration.underline,
                        decorationColor: hovered
                            ? palette.text
                            : palette.textFaint,
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ],
        ],
      ),
    );
  }
}

/// [text] on one line, cut in its middle with "…" when it doesn't fit, so
/// a path keeps both its start and its end.
class MiddleCut extends StatelessWidget {
  const MiddleCut(this.text, {super.key, required this.style});

  final String text;
  final TextStyle style;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, box) {
      final direction = Directionality.of(context);
      final scaler = MediaQuery.textScalerOf(context);
      bool fits(String shown) {
        final painter = TextPainter(
          text: TextSpan(text: shown, style: style),
          textDirection: direction,
          textScaler: scaler,
          maxLines: 1,
        )..layout();
        final width = painter.width;
        painter.dispose();
        return width <= box.maxWidth;
      }

      String cut(int kept) {
        final head = (kept + 1) ~/ 2;
        return '${text.substring(0, head)}…'
            '${text.substring(text.length - (kept - head))}';
      }

      var shown = text;
      if (box.maxWidth.isFinite && !fits(text)) {
        // The most characters that fit, around the cut.
        var low = 0, high = text.length - 1;
        while (low < high) {
          final mid = (low + high + 1) ~/ 2;
          if (fits(cut(mid))) {
            low = mid;
          } else {
            high = mid - 1;
          }
        }
        shown = cut(low);
      }
      return Text(
        shown,
        maxLines: 1,
        softWrap: false,
        overflow: TextOverflow.clip,
        semanticsLabel: text,
        style: style,
      );
    },
  );
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

/// [path], relative to [base], as a full path in [base]'s own separators:
/// a folder on Windows (`C:\…`, `\\server\…`) joins with backslashes.
String _join(String base, String path) {
  final windows =
      RegExp(r'^[A-Za-z]:\\').hasMatch(base) || base.startsWith(r'\\');
  if (!windows) return base.endsWith('/') ? '$base$path' : '$base/$path';
  final relative = path.replaceAll('/', r'\');
  return base.endsWith(r'\') ? '$base$relative' : '$base\\$relative';
}

/// Opens [path] in a viewer beside the selected agent's conversation (from
/// focus mode, the request's, focus mode left first), as an agent's `show`
/// does, or makes its pane glow when it's open already. From
/// a Files pane ([beside]), the first viewer goes right of it. On a phone,
/// a page over the conversation.
void openFile(BuildContext context, String path, {String? beside}) {
  final fromFocusMode = leaveFocusModeToOpen(context);
  final scope = AppScope.scope(context);
  final backend = scope.backend;
  final agent = fromFocusMode == null
      ? backend.workspace.selectedAgent
      : backend.workspace.agent(fromFocusMode);
  if (agent == null) return;
  if (beside == null &&
      (PhoneRoutes.of(context)?.showFile(agent.id, path) ?? false)) {
    return;
  }
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
    final row = Pressable(
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
                    shownName(entry.name),
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
    // A file's row, touched and held: Share….
    if (entry.isDir) return row;
    final path = _join(base, entry.path);
    final held = ShareHold(path: path, child: row);
    if (!showsInFinder(backend)) return held;
    // On the computer that runs the agents, a right click: Show in Finder.
    return Builder(
      builder: (context) => GestureDetector(
        behavior: HitTestBehavior.opaque,
        onSecondaryTapUp: (_) => unawaited(
          showFloating<void>(
            context,
            width: 200,
            builder: (inner, close) => AgentMenu(
              close: close,
              items: [
                AgentMenuItem(
                  id: 'finder',
                  label: showInFinderLabel,
                  glyph: (color) => ExtensionGlyph(
                    icon: 'folder-open',
                    name: '',
                    color: color,
                  ),
                  onSelect: () => unawaited(showInFinder([File(path)])),
                ),
              ],
            ),
          ),
        ),
        child: held,
      ),
    );
  }
}
