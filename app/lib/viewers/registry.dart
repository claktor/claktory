import 'dart:async';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';

import '../annotations.dart';
import '../backend.dart';
import '../controls.dart';
import '../identity.dart';
import '../pane_chrome.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../resources.dart' show formatBytes;
import '../scope.dart';
import '../selection.dart';
import 'image.dart';
import 'text.dart';
import 'video.dart';

/// What a viewer pane shows: one entry per kind (`ViewLeaf.kind`). A new
/// kind (PDFs, documents, dashboards, extension panels) is a new entry, and
/// the panes around it stay as they are (docs/IDENTITY.md, "Panes and
/// viewers"). A viewer shows its body in a [PaneScaffold], which puts its
/// details and controls in the pane's header.
class Viewer {
  const Viewer({
    required this.kind,
    required this.label,
    required this.icon,
    required this.build,
  });

  /// `ViewLeaf.kind`: "text", "image", "video".
  final String kind;

  /// "Text", "Image", "Video".
  final String label;

  /// Its 14px glyph, in the pane's header and the phone's list.
  final Widget Function(Color color) icon;

  final Widget Function(BuildContext context, ViewLeaf leaf) build;
}

final _viewers = <Viewer>[
  Viewer(
    kind: 'text',
    label: 'Text',
    icon: (color) => FileGlyph(color: color),
    build: (context, leaf) =>
        TextViewer(key: ValueKey(leaf.path), path: leaf.path),
  ),
  Viewer(
    kind: 'image',
    label: 'Image',
    icon: (color) =>
        ExtensionGlyph(icon: 'image', name: 'Image', color: color, size: 14),
    build: (context, leaf) =>
        ImageViewer(key: ValueKey(leaf.path), path: leaf.path),
  ),
  Viewer(
    kind: 'video',
    label: 'Video',
    icon: (color) =>
        ExtensionGlyph(icon: 'film', name: 'Video', color: color, size: 14),
    build: (context, leaf) =>
        VideoViewer(key: ValueKey(leaf.path), path: leaf.path),
  ),
];

/// The viewer for [kind]; null for "chat" and kinds this app doesn't know.
Viewer? viewerFor(String kind) {
  for (final viewer in _viewers) {
    if (viewer.kind == kind) return viewer;
  }
  return null;
}

/// [leaf]'s viewer, or a line saying this app can't show its kind yet.
Widget viewerBody(BuildContext context, ViewLeaf leaf) =>
    viewerFor(leaf.kind)?.build(context, leaf) ??
    PaneScaffold(
      body: ViewerProblem(
        "Claktory can't show this kind of file here yet.",
        path: leaf.path,
      ),
    );

/// What [leaf]'s pane is called: its title, else its file's name.
String viewerTitle(ViewLeaf leaf) {
  if (leaf.title.isNotEmpty) return leaf.title;
  return leaf.path.split('/').where((part) => part.isNotEmpty).lastOrNull ??
      leaf.path;
}

/// [leaf]'s kind's glyph; a plain file's for a kind this app doesn't know.
Widget viewerIcon(ViewLeaf leaf, Color color) =>
    viewerFor(leaf.kind)?.icon(color) ?? FileGlyph(color: color);

/// Why a pane shows nothing: one centered line, and the path under it.
class ViewerProblem extends StatelessWidget {
  const ViewerProblem(this.text, {super.key, this.path = ''});

  final String text;
  final String path;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(Space.xl),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              text,
              textAlign: TextAlign.center,
              style: TextStyles.body.copyWith(color: palette.textMuted),
            ),
            if (path.isNotEmpty) ...[
              const SizedBox(height: Space.xs),
              Text(
                path,
                textAlign: TextAlign.center,
                style: TextStyles.caption.copyWith(color: palette.textFaint),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// A file, read from the backend when the viewer opens, again on
/// [reload], and by itself when it changes on the machine (FileChanged),
/// so a viewer shows it as it is on the machine Claktory runs on. Its
/// [comments] are the file's comments as this viewer shows them.
mixin FileLoading<T extends StatefulWidget> on State<T> {
  String get path;

  FileContent? file;

  /// The request failed outright (rather than the file being unreadable).
  String? failure;

  Listenable? _reloads;

  Backend? _backend;

  /// The comments on the file, for the viewer to show.
  CommentsController? comments;

  /// Who changed the file last, while the header says so.
  String? _updatedBy;
  Timer? _updatedFor;

  @override
  void initState() {
    super.initState();
    reload();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final reloads = PaneChrome.of(context)?.reloads;
    if (reloads != _reloads) {
      _reloads?.removeListener(reload);
      _reloads = reloads?..addListener(reload);
    }
    final backend = AppScope.of(context);
    if (backend != _backend) {
      _backend?.fileChanged.removeListener(_fileChanged);
      comments?.dispose();
      _backend = backend..fileChanged.addListener(_fileChanged);
      comments = CommentsController(backend: backend, path: path);
    }
  }

  @override
  void dispose() {
    _reloads?.removeListener(reload);
    _backend?.fileChanged.removeListener(_fileChanged);
    comments?.dispose();
    _updatedFor?.cancel();
    super.dispose();
  }

  /// The file changed on the machine: read it again, and say who did it.
  void _fileChanged() {
    final changed = _backend?.fileChanged.value;
    if (changed == null || changed.path != path) return;
    if (changed.sha256.isNotEmpty && changed.sha256 == file?.sha256) return;
    unawaited(reload());
    if (changed.byAgentId.isEmpty) return;
    setState(() => _updatedBy = changed.byAgentId);
    _updatedFor?.cancel();
    _updatedFor = Timer(const Duration(seconds: 6), () {
      if (mounted) setState(() => _updatedBy = null);
    });
  }

  /// The header's details: [usual], or for a few seconds after an agent
  /// changed the file, who did.
  String details(String usual) {
    final by = _updatedBy;
    if (by == null) return usual;
    for (final agent in _backend?.workspace.workspace.agents ?? <Agent>[]) {
      if (agent.id == by) return '${agent.name} just updated this file';
    }
    return usual;
  }

  Future<void> reload() async {
    try {
      final read = await AppScope.of(context).readFile(path);
      if (!mounted) return;
      setState(() {
        file = read;
        failure = null;
      });
      loaded(read);
    } on RequestFailure catch (error) {
      if (mounted) setState(() => failure = error.message);
    }
  }

  /// After a read, for what the viewer works out from the bytes.
  void loaded(FileContent file) {}

  /// The body while there's nothing to show yet, or why there's nothing.
  Widget? waiting() {
    final file = this.file;
    if (failure != null) return ViewerProblem(failure!, path: path);
    if (file == null) return const SizedBox.shrink();
    if (file.error.isNotEmpty) return ViewerProblem(file.error, path: path);
    return null;
  }

  Widget reloadButton() => GlyphButton(
    label: 'Reload',
    active: false,
    onTap: reload,
    glyph: (color) =>
        ExtensionGlyph(icon: 'rotate-cw', name: 'Reload', color: color),
  );

  /// "Showing the first 2.0 MB of 14.3 MB."
  String? cutNote(FileContent file) => file.truncated
      ? 'Showing the first ${formatBytes(Int64(file.data.length))} of '
            '${formatBytes(file.size)}.'
      : null;
}

/// A quiet text button for a pane's header: "Fit", "100%".
class HeaderTextButton extends StatelessWidget {
  const HeaderTextButton(
    this.label,
    this.onTap, {
    super.key,
    this.active = false,
  });

  final String label;
  final VoidCallback onTap;

  /// Tinted while what it turns on is on.
  final bool active;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Unselectable(
      child: Semantics(
        button: true,
        label: label,
        excludeSemantics: true,
        child: Pressable(
          onTap: onTap,
          builder: (context, hovered) => Container(
            height: 28,
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: active
                  ? palette.selected
                  : hovered
                  ? palette.hover
                  : const Color(0x00000000),
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Text(
              label,
              style: TextStyles.caption.copyWith(
                color: active || hovered ? palette.text : palette.textMuted,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
        ),
      ),
    );
  }
}
