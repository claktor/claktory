import 'dart:async';
import 'dart:io' show FileSystemException;

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
import 'file_cache.dart';
import 'image.dart';
import 'loading_progress.dart';
import 'pdf.dart';
import 'preview_badge.dart';
import 'previews.dart';
import '../paced.dart' show FilePreviews, lostWhileReading;
import 'text.dart';
import 'video.dart';
import '../visible_codes.dart' show shownName;

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
        // In a pane all the same: on a phone its header, with Back, and
        // Share… to save it or open it in another app (Sale, Bruno 30855).
        : _problemPane(
            context,
            leaf.path,
            'To see this PDF, open it on a Mac, iPhone or iPad.',
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
    _problemPane(
      context,
      leaf.path,
      "Claktory can't show this kind of file here yet.",
    );

/// A pane for a file this app can't show: why, in a pane with its header
/// (Back on a phone), and Share… so it can still go to an app that can
/// (#418).
Widget _problemPane(BuildContext context, String path, String text) =>
    PaneScaffold(
      share: path,
      body: ColoredBox(
        color: Palette.of(context).canvas,
        child: ViewerProblem(text, path: path),
      ),
    );

/// What [leaf]'s pane is called: its title, else its file's name.
String viewerTitle(ViewLeaf leaf) {
  if (leaf.title.isNotEmpty) return shownName(leaf.title);
  return shownName(
    leaf.path.split('/').where((part) => part.isNotEmpty).lastOrNull ??
        leaf.path,
  );
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

  /// What Share… sends (#418): the file, once a read of it worked.
  String? get sharePath {
    final file = this.file;
    return file != null && file.error.isEmpty ? path : null;
  }

  /// The request failed outright (rather than the file being unreadable).
  String? failure;

  /// Why [failure] is, as the viewer shows it: the file's path under it,
  /// except for a read lost with the connection, which is no fault of the
  /// file (Zucchero's row 588).
  String get failurePath => lostWithConnection(failure) ? '' : path;

  /// From a drop, how long a read lost with it waits for the connection
  /// before it says it failed (#375, Zucchero's row 588).
  Timer? _giveUp;

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
    _giveUp?.cancel();
    super.dispose();
  }

  /// Connected again: what couldn't be read before is read now.
  void _connectedChanged() {
    final now = _backend?.connected.value ?? false;
    final connecting = now && !_wasConnected;
    final dropped = !now && _wasConnected;
    _wasConnected = now;
    // Dropped: a read lost with it waits for it, as a read in pieces does,
    // up to [lostReadWaits], then says it failed. A read made of one piece
    // (a video's next part) fails with its request, so this is what holds
    // it to the same time (Zucchero's row 588).
    if (dropped) {
      _giveUp?.cancel();
      _giveUp = Timer(lostReadWaits, _gaveUp);
    }
    if (now) {
      _giveUp?.cancel();
      _giveUp = null;
    }
    // A kept copy shown while offline is checked against the file now.
    if (connecting && (failure != null || file == null || _unchecked)) {
      // Lost with the connection, it carries on now it's back: loading,
      // not the failure, while it does.
      if (lostWithConnection(failure)) setState(() => failure = null);
      unawaited(reload());
    }
    // The original, given up on with the connection: on from where it
    // got, as Share carries on, rather than waiting for Try again (Bruno
    // 29790).
    if (connecting && originalLoad is OriginalFailed && _originalLost) {
      unawaited(loadOriginal());
    }
  }

  /// The connection didn't come back in time: a read lost with it says so.
  void _gaveUp() {
    _giveUp = null;
    if (!mounted || (_backend?.connected.value ?? true)) return;
    if (failure != null && lostWithConnection(failure)) {
      setState(() => failure = lostWhileReading);
    }
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
    if (by != null) {
      for (final agent in _backend?.workspace.workspace.agents ?? <Agent>[]) {
        if (agent.id == by) return '${agent.name} just updated this file';
      }
    }
    // The copy kept on this device, offline (#446).
    if (_unchecked) {
      return usual.isEmpty ? mayBeOutOfDate : '$mayBeOutOfDate · $usual';
    }
    return usual;
  }

  /// What shows is the copy this device kept (#446), and whether the file
  /// is still that version couldn't be asked (offline): the header says
  /// so, and it's asked once connected.
  var _unchecked = false;

  /// A viewer that reads its file its own way says whether it shows a
  /// kept copy that couldn't be checked.
  @protected
  set showingUnchecked(bool unchecked) => _unchecked = unchecked;

  /// The copy of [variants] of the file this device keeps, if any and
  /// still the file's version, or offline (#446): shown, and true; false
  /// when it has to be read.
  Future<bool> _showKept(Backend backend, List<String> variants) async {
    final cache = FileCache.of(backend);
    if (cache == null) return false;
    final found = await cache.lookup(backend, path, variants);
    if (!mounted || found == null) return false;
    final kept = found.kept.content;
    final shown = file;
    final FileContent read;
    if (shown != null &&
        shown.error.isEmpty &&
        !shown.truncated &&
        shown.sha256 == kept.sha256 &&
        shown.isPreview == kept.isPreview &&
        shown.size == kept.size) {
      // Already showing: nothing to decode again.
      read = shown;
    } else {
      try {
        read = await found.kept.read();
      } on FileSystemException {
        await cache.drop(path);
        return false;
      }
      if (!mounted) return false;
    }
    setState(() {
      file = read;
      failure = null;
      _progress = null;
      _unchecked = !found.current;
      if (!read.isPreview) originalLoad = const OriginalNotLoaded();
    });
    if (!identical(read, shown)) loaded(read);
    return true;
  }

  /// Keeps [read] on this device, for next time (#446).
  void _keep(Backend backend, FileContent read) {
    final cache = FileCache.of(backend);
    if (cache == null || read.truncated || read.error.isNotEmpty) return;
    if (!keeps(read)) return;
    final variant = read.isPreview
        ? previewVariant(previewScreen())
        : originalVariant;
    unawaited(cache.keepRead(path, variant, read));
  }

  /// Whether [read] may be kept on this device: a viewer that reads files
  /// it wasn't asked to open (a Markdown file's pictures, #540) keeps only
  /// what is what it says it is (Fugo, 41320).
  @protected
  bool keeps(FileContent read) => true;

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

  /// Whether the original failed for want of the connection, so it's
  /// asked for again once that's back.
  bool _originalLost = false;

  /// The original couldn't come ([error], when a request failed): the
  /// preview stays, and the badge says so. Lost with the connection, it
  /// carries on by itself once that's back.
  @protected
  void originalFailed(Object? error) {
    wantOriginal = false;
    _originalLost =
        error is RequestFailure &&
        (_lostWith.contains(error.message) ||
            !(_backend?.connected.value ?? true));
    if (mounted) setState(() => originalLoad = const OriginalFailed());
  }

  Future<void> reload() async {
    try {
      final backend = AppScope.of(context);
      // Kept on this device and still the file (#446): the original if
      // it's here, else the preview made for this screen.
      final kept = await _showKept(backend, [
        originalVariant,
        if (previews && !wantOriginal) previewVariant(previewScreen()),
      ]);
      if (kept || !mounted) return;
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
        _unchecked = false;
        if (!read.isPreview) originalLoad = const OriginalNotLoaded();
      });
      loaded(read);
      _keep(backend, read);
    } on RequestFailure catch (error) {
      if (mounted) {
        setState(() {
          failure = error.message;
          // Lost with the connection: how far it got stays shown while it
          // waits for it.
          if (!lostWithConnection(error.message)) _progress = null;
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
      final backend = AppScope.of(context);
      // Kept on this device (#446): at once.
      if (await _showKept(backend, [originalVariant]) || !mounted) return;
      final read = await backend.readFile(
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
        _unchecked = false;
      });
      loaded(read);
      _keep(backend, read);
    } on RequestFailure catch (error) {
      originalFailed(error);
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
    // Lost a moment before the connection says it's down, the same.
    // Given up on, it says so (Zucchero's row 588).
    if (failure != null &&
        failure != lostWhileReading &&
        connected != null &&
        (!connected.value || lostWithConnection(failure))) {
      return LoadingProgress(
        label: loadingLabel,
        received: _progress?.through ?? 0,
        size: _progress?.size ?? 0,
        connected: connected,
      );
    }
    if (failure != null) return ViewerProblem(failure!, path: failurePath);
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
        onTap: onTap,
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

/// The header's word on a copy kept on this device that couldn't be
/// checked against the file, offline (#446).
const mayBeOutOfDate = 'Saved copy, may be out of date';

/// How long after a drop a read lost with it waits for the connection
/// before it says it failed: as long as a read in pieces waits.
const lostReadWaits = Duration(seconds: 60);

/// Whether [failure] is a read's lost with its connection: given up on
/// after a drop, or asked for while not connected.
bool lostWithConnection(String? failure) => _lostWith.contains(failure);

/// What a read lost with its connection fails with: given up on after a
/// drop, or asked for while not connected.
const _lostWith = {
  lostWhileReading,
  'Connection lost.',
  'Not connected to the backend.',
};
