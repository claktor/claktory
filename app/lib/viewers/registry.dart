import 'dart:async';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart'
    show TargetPlatform, defaultTargetPlatform, protected;
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
import 'loading_progress.dart';
import 'pdf.dart';
import 'preview_badge.dart';
import 'previews.dart';
import '../paced.dart' show FilePreviews;
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
  // A PDF: an iPad shows one in its document host, where the person draws
  // on it (ipad/document_host.dart, ahead of this registry); a Mac and an
  // iPhone in PDFKit's own view (pdf.dart), only to read; elsewhere the
  // pane says so.
  Viewer(
    kind: 'pdf',
    label: 'PDF',
    icon: (color) => FileGlyph(color: color),
    build: (context, leaf) =>
        defaultTargetPlatform == TargetPlatform.macOS ||
            defaultTargetPlatform == TargetPlatform.iOS
        ? PdfViewer(key: ValueKey(leaf.path), path: leaf.path)
        : ViewerProblem(
            'To see this PDF, open it on a Mac, iPhone or iPad.',
            path: leaf.path,
          ),
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

  /// Whether [_backend] was connected when last heard from: a read that
  /// failed, or never came, before it connected is made again when it
  /// connects (a cold start over the tunnel), once per connection.
  bool _wasConnected = false;

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
      _backend?.connected.removeListener(_connectedChanged);
      comments?.dispose();
      _backend = backend
        ..fileChanged.addListener(_fileChanged)
        ..connected.addListener(_connectedChanged);
      _wasConnected = backend.connected.value;
      comments = CommentsController(backend: backend, path: path);
    }
  }

  @override
  void dispose() {
    _reloads?.removeListener(reload);
    _backend?.fileChanged.removeListener(_fileChanged);
    _backend?.connected.removeListener(_connectedChanged);
    comments?.dispose();
    _updatedFor?.cancel();
    super.dispose();
  }

  /// Connected again: what couldn't be read before is read now.
  void _connectedChanged() {
    final now = _backend?.connected.value ?? false;
    final connecting = now && !_wasConnected;
    _wasConnected = now;
    if (connecting && (failure != null || file == null)) unawaited(reload());
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

  /// How far the read on its way has got, while it's on its way (#375).
  ({int through, int size})? _progress;

  /// What [waiting] says is loading: "Loading image…".
  String get loadingLabel => 'Loading…';

  /// Whether this viewer's file may come as a preview made for this
  /// screen (#379; previews.dart says when).
  bool get previews => false;

  /// The original was asked for, by Show original or by zooming past the
  /// preview's sharpness: this pane reads it from then on.
  @protected
  var wantOriginal = false;

  /// Where loading the original in place of the preview has got.
  OriginalLoad originalLoad = const OriginalNotLoaded();

  Future<void> reload() async {
    try {
      final backend = AppScope.of(context);
      void progress(int through, int size) {
        if (!mounted) return;
        setState(() => _progress = (through: through, size: size));
      }

      // A phone or tablet asks for a preview made for its screen; a Mac
      // reads the original, and asks for one only once it's clear the
      // original would be slow to come (previews.dart).
      // An image too big to open whole comes as a preview too, rather
      // than its first 20 MB to no use (Zucchero, #375).
      final ask = previews && !wantOriginal;
      bool tooBig(int size) => size > wholeImageLimit;
      final slow = stopWhenSlow(backend);
      var read = await backend.readFile(
        path,
        preview: ask && touchDevice ? previewScreen() : null,
        carryOn: !ask
            ? null
            : touchDevice
            ? (through, size) => !tooBig(size)
            : (through, size) => !tooBig(size) && slow(through, size),
        onProgress: progress,
      );
      if (!mounted) return;
      final size = read.size.toInt();
      if (ask &&
          !touchDevice &&
          read.error.isEmpty &&
          read.truncated &&
          (tooBig(size) ||
              size >= previewFrom &&
                  slowFor(backend, size - read.data.length))) {
        read = await backend.readFile(
          path,
          preview: previewScreen(),
          onProgress: progress,
        );
        if (!mounted) return;
      }
      setState(() {
        file = read;
        failure = null;
        _progress = null;
        if (!read.isPreview) originalLoad = const OriginalNotLoaded();
      });
      loaded(read);
    } on RequestFailure catch (error) {
      if (mounted) {
        setState(() {
          failure = error.message;
          _progress = null;
        });
      }
    }
  }

  /// After a read, for what the viewer works out from the bytes.
  void loaded(FileContent file) {}

  /// The original in place of the preview shown, with how much has come;
  /// the preview stays until it's here, and if it can't come, says so.
  Future<void> loadOriginal() async {
    if (originalLoad is OriginalLoading) return;
    wantOriginal = true;
    setState(() => originalLoad = const OriginalLoading(0, 0));
    try {
      final read = await AppScope.of(context).readFile(
        path,
        onProgress: (through, size) {
          if (!mounted || originalLoad is! OriginalLoading) return;
          setState(() => originalLoad = OriginalLoading(through, size));
        },
      );
      if (!mounted) return;
      // Not all of it can come (an image over the 20 MB a plain read
      // takes): the preview stays.
      if (read.error.isNotEmpty || read.truncated) {
        throw RequestFailure(read.error);
      }
      setState(() {
        file = read;
        originalLoad = const OriginalNotLoaded();
      });
      loaded(read);
    } on RequestFailure {
      wantOriginal = false;
      if (mounted) setState(() => originalLoad = const OriginalFailed());
    }
  }

  /// "Preview · Show original · 3.3 MB" while [file] is a preview; null
  /// while it's the file itself.
  Widget? previewBadge() {
    final file = this.file;
    if (file == null || !file.isPreview) return null;
    return PreviewBadge(
      originalSize: file.originalSize.toInt(),
      load: originalLoad,
      onShowOriginal: () => unawaited(loadOriginal()),
    );
  }

  /// The body while there's nothing to show yet, or why there's nothing.
  Widget? waiting() {
    final file = this.file;
    final connected = _backend?.connected;
    // Lost with the connection: read again once it's back (#375), so it
    // waits for it rather than saying it failed.
    if (failure != null && connected != null && !connected.value) {
      return LoadingProgress(
        label: loadingLabel,
        received: 0,
        size: 0,
        connected: connected,
      );
    }
    if (failure != null) return ViewerProblem(failure!, path: path);
    // Coming over the link: how much has, once more than a piece is due.
    final progress = _progress;
    if (file == null && progress != null && connected != null) {
      return LoadingProgress(
        label: loadingLabel,
        received: progress.through,
        size: progress.size,
        connected: connected,
      );
    }
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
