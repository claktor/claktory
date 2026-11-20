import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../controls.dart';
import '../identity.dart';
import '../pane_chrome.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../resources.dart' show formatBytes;
import '../scope.dart';
import 'image.dart';
import 'text.dart';

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

  /// `ViewLeaf.kind`: "text", "image".
  final String kind;

  /// "Text", "Image".
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

/// A file, read from the backend when the viewer opens and again on
/// [reload], so a viewer shows it as it is on the machine Claktory runs on.
mixin FileLoading<T extends StatefulWidget> on State<T> {
  String get path;

  FileContent? file;

  /// The request failed outright (rather than the file being unreadable).
  String? failure;

  Listenable? _reloads;

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
  }

  @override
  void dispose() {
    _reloads?.removeListener(reload);
    super.dispose();
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
