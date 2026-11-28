import 'dart:async';
import 'dart:io' show FileSystemException;

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart'
    show TargetPlatform, defaultTargetPlatform, protected;
import 'package:flutter/widgets.dart';

import '../annotations.dart';
import '../backend.dart';
import '../controls.dart';
import '../diagnostics_record.dart';
import '../identity.dart';
import '../mobile/navigation.dart' show PhoneRoute, PhoneRoutes;
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
import 'seal.dart';
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

  /// The viewer for [leaf]'s file, or for its version kept by a seal.
  final Widget Function(BuildContext context, ViewLeaf leaf, FileSeal? sealed)
  build;
}

final _viewers = <Viewer>[
  Viewer(
    kind: 'text',
    label: 'Text',
    icon: (color) => FileGlyph(color: color),
    build: (context, leaf, sealed) =>
        TextViewer(key: _key(leaf, sealed), path: leaf.path, sealed: sealed),
  ),
  Viewer(
    kind: 'image',
    label: 'Image',
    icon: (color) =>
        ExtensionGlyph(icon: 'image', name: 'Image', color: color, size: 14),
    build: (context, leaf, sealed) =>
        ImageViewer(key: _key(leaf, sealed), path: leaf.path, sealed: sealed),
  ),
  // A PDF: an iPad shows one in its document host, where the person draws
  // on it (ipad/document_host.dart, ahead of this registry); a Mac and an
  // iPhone in PDFKit's own view (pdf.dart), only to read; elsewhere the
  // pane says so.
  Viewer(
    kind: 'pdf',
    label: 'PDF',
    icon: (color) => FileGlyph(color: color),
    build: (context, leaf, sealed) =>
        defaultTargetPlatform == TargetPlatform.macOS ||
            defaultTargetPlatform == TargetPlatform.iOS
        ? PdfViewer(key: _key(leaf, sealed), path: leaf.path, sealed: sealed)
        // In a pane all the same: on a phone its header, with Back, and
        // Share… to save it or open it in another app.
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
    build: (context, leaf, sealed) =>
        VideoViewer(key: _key(leaf, sealed), path: leaf.path, sealed: sealed),
  ),
];

/// A viewer's key: its file, and the sealed version it shows, so showing
/// one starts a viewer of its own.
Key _key(ViewLeaf leaf, FileSeal? sealed) =>
    ValueKey(sealed == null ? leaf.path : '${leaf.path}@${sealed.sha256}');

/// The viewer for [kind]; null for "chat" and kinds this app doesn't know.
Viewer? viewerFor(String kind) {
  for (final viewer in _viewers) {
    if (viewer.kind == kind) return viewer;
  }
  return null;
}

/// [leaf]'s viewer, or a line saying this app can't show its kind yet.
Widget viewerBody(BuildContext context, ViewLeaf leaf) {
  final viewer = viewerFor(leaf.kind);
  if (viewer == null) {
    return _problemPane(
      context,
      leaf.path,
      "Claktory can't show this kind of file here yet.",
    );
  }
  // A sealed version of the file shows in the pane's place, until Show
  // current version.
  return SealedVersionHost(
    builder: (context, sealed) => sealed == null
        ? viewer.build(context, leaf, null)
        : viewer.build(context, leaf, sealed),
  );
}

/// A pane for a file this app can't show: why, in a pane with its header
/// (Back on a phone), and Share… so it can still go to an app that can
/// .
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

  /// The seal whose kept version this viewer shows, rather than the file
  /// as it is: read as it was sealed, never changing, without comments;
  /// null for the file.
  FileSeal? get sealed => null;

  /// [sealed]'s SHA-256, for a read of that version; empty for the file.
  String get sealedSha256 => sealed?.sha256 ?? '';

  /// A viewer of [sealed]'s version of this file, for a phone's page.
  @protected
  Widget sealedViewer(FileSeal sealed);

  /// The file's seals and whether it still matches its latest, as the last
  /// read found them, or a seal made since; null before a read, and from
  /// a Claktory that doesn't say.
  FileSeals? seals;

  /// The copies of files this device keeps for [backend]'s Claktory, where
  /// this viewer's file may be kept: none for a sealed version, which is
  /// never the file's.
  FileCache? keptCopies(Backend backend) =>
      sealed == null ? FileCache.of(backend) : null;

  /// What Share… sends: the file, once a read of it worked; nothing for a
  /// sealed version, which isn't the file.
  String? get sharePath {
    final file = this.file;
    return file != null && file.error.isEmpty && sealed == null ? path : null;
  }

  /// The request failed outright (rather than the file being unreadable).
  String? failure;

  /// Why [failure] is, as the viewer shows it: the file's path under it,
  /// except for a read lost with the connection, which is no fault of the
  /// file.
  String get failurePath => lostWithConnection(failure) ? '' : path;

  /// From a drop, how long a read lost with it waits for the connection
  /// before it says it failed.
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

  /// Nothing came within [loadingShowsAfter]: the body says it's loading.
  var _slow = false;
  Timer? _slowFor;

  /// Since this viewer asked for its file, till it first shows
  /// (diagnostics).
  final _opening = Stopwatch()..start();

  /// Whether the file has shown in this viewer, and diagnostics said so.
  var _shownNoted = false;

  /// How long the first read waited for the connection, when it did.
  Duration? _waited;

  /// Whether another file's bulk read was on its way when this one's read
  /// was asked for.
  bool? _bulkWhenAsked;

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
      _backend?.fileSeals.removeListener(_sealsChanged);
      _backend?.connected.removeListener(_connectedChanged);
      comments?.dispose();
      _backend = backend
        ..fileChanged.addListener(_fileChanged)
        ..fileSeals.addListener(_sealsChanged)
        ..connected.addListener(_connectedChanged);
      _wasConnected = backend.connected.value;
      // Comments are on the file as it is, never on a sealed version.
      comments = sealed == null
          ? CommentsController(backend: backend, path: path)
          : null;
    }
  }

  @override
  void dispose() {
    _reloads?.removeListener(reload);
    _backend?.fileChanged.removeListener(_fileChanged);
    _backend?.fileSeals.removeListener(_sealsChanged);
    _backend?.connected.removeListener(_connectedChanged);
    comments?.dispose();
    _updatedFor?.cancel();
    _giveUp?.cancel();
    _slowFor?.cancel();
    super.dispose();
  }

  /// Connected again: what couldn't be read before is read now.
  void _connectedChanged() {
    final now = _backend?.connected.value ?? false;
    final connecting = now && !_wasConnected;
    final dropped = !now && _wasConnected;
    _wasConnected = now;
    if (connecting && !_shownNoted) _waited = _opening.elapsed;
    // Dropped: a read lost with it waits for it, as a read in pieces does,
    // up to [lostReadWaits], then says it failed. A read made of one piece
    // (a video's next part) fails with its request, so this is what holds
    // it to the same time.
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
    // got, as Share carries on, rather than waiting for Try again.
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
    if (changed == null || changed.path != path || sealed != null) return;
    if (changed.sha256.isNotEmpty && changed.sha256 == file?.sha256) return;
    unawaited(reload());
    if (changed.byAgentId.isEmpty) return;
    setState(() => _updatedBy = changed.byAgentId);
    _updatedFor?.cancel();
    _updatedFor = Timer(const Duration(seconds: 6), () {
      if (mounted) setState(() => _updatedBy = null);
    });
  }

  /// A seal was made: this file's, by its canonical path, shows it now.
  void _sealsChanged() {
    final made = _backend?.fileSeals.value;
    final shown = seals;
    if (made == null || shown == null || sealed != null) return;
    if (made.path != shown.path) return;
    setState(() => seals = made);
  }

  /// The header's details: [usual], or for a few seconds after an agent
  /// changed the file, who did. A sealed version says so first, and a
  /// file still as it was sealed says "Sealed".
  String details(String usual) {
    String and(String first, String then) =>
        then.isEmpty ? first : '$first · $then';
    if (sealed != null) return and(sealedVersionDetail, usual);
    final by = _updatedBy;
    if (by != null) {
      for (final agent in _backend?.workspace.workspace.agents ?? <Agent>[]) {
        if (agent.id == by) return '${agent.name} just updated this file';
      }
    }
    final shown = stillSealed(seals) ? and(sealedDetail, usual) : usual;
    // The copy kept on this device, offline.
    if (_unchecked) return and(mayBeOutOfDate, shown);
    return shown;
  }

  /// The line under the header about the file's seal: what changed since
  /// it was sealed, with Open sealed version; for a sealed version, whose
  /// it is. Null when there's nothing to say.
  Widget? sealNotice() {
    if (sealed case final seal?) {
      final versions = SealedVersions.of(context);
      return SealNotice(
        kind: SealNoticeKind.version,
        seal: seal,
        // In the pane's place: back to the file. A page has Back.
        onAction: versions != null && versions.shown == seal
            ? () => versions.show(null)
            : null,
      );
    }
    final file = this.file;
    if (file == null || file.error.isNotEmpty) return null;
    if (!changedSinceSealed(seals)) return null;
    final seal = latestSeal(seals)!;
    return SealNotice(
      kind: SealNoticeKind.changed,
      seal: seal,
      // A version too big to keep was sealed by its hash alone.
      onAction: seal.kept ? () => openSealed(seal) : null,
      // On a phone it's in More, apart from the line's one action, as
      // iOS keeps what destroys out of the way.
      onDelete: seal.kept && PaneChrome.of(context)?.phone == null
          ? () => unawaited(_deleteKeptCopy(seal))
          : null,
      error: _sealError,
    );
  }

  /// A phone's More for the seal: Delete kept copy…, while there's one to
  /// delete.
  List<PaneAction> sealActions() {
    final seal = latestSeal(seals);
    if (sealed != null || seal == null || !seal.kept) return const [];
    if (!changedSinceSealed(seals)) return const [];
    return [
      PaneAction(deleteKeptCopyLabel, () => unawaited(_deleteKeptCopy(seal))),
    ];
  }

  /// Why the kept copy of a sealed version couldn't be deleted.
  String? _sealError;

  /// Deletes the copy kept of [seal]'s version, once the person says so;
  /// the file's seals come again without it.
  Future<void> _deleteKeptCopy(FileSeal seal) async {
    if (!await confirmDeleteKeptCopy(context) || !mounted) return;
    setState(() => _sealError = null);
    try {
      await AppScope.of(context).deleteSealedCopy(path, seal.sha256);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _sealError = failure.message);
    }
  }

  /// Shows [seal]'s version of the file: on a phone as a page over this
  /// one, so Back comes back; elsewhere in this pane's place.
  void openSealed(FileSeal seal) {
    final routes = PhoneRoutes.of(context);
    if (routes == null) {
      SealedVersions.of(context)?.show(seal);
      return;
    }
    final title =
        PaneChrome.of(context)?.phone?.title ??
        shownName(path.split('/').where((part) => part.isNotEmpty).last);
    unawaited(
      routes.push<void>(
        PhoneRoute(
          id: 'sealed:$path@${seal.sha256}',
          edgeToEdge: deviceIdiom(context) != Idiom.pad,
          builder: (context) => PaneChrome(
            phone: PhonePane(title: title, onBack: routes.pop),
            child: sealedViewer(seal),
          ),
        ),
      ),
    );
  }

  /// What shows is the copy this device kept, and whether the file
  /// is still that version couldn't be asked (offline): the header says
  /// so, and it's asked once connected.
  var _unchecked = false;

  /// A viewer that reads its file its own way says whether it shows a
  /// kept copy that couldn't be checked.
  @protected
  set showingUnchecked(bool unchecked) => _unchecked = unchecked;

  /// The copy of [variants] of the file this device keeps, if any: shown
  /// at once, then checked against the file. True when it's still the
  /// file's version, or that can't be asked (offline); false when there's
  /// none, or the file changed since, for it to be read (the copy stays
  /// shown till then).
  Future<bool> _showKept(
    Backend backend,
    List<String> variants, {
    Future<FileContent> Function(String unless)? fetch,
  }) async {
    final cache = keptCopies(backend);
    if (cache == null) return false;
    final found = await cache.firstOf(path, variants);
    if (!mounted || found == null) return false;
    final kept = found.content;
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
        read = await found.read();
      } on FileSystemException {
        await cache.drop(path);
        return false;
      }
      if (!mounted) return false;
    }
    // Before asking whether it's still the file: on a far link that's a
    // round trip, most of a second, and the copy is almost always it.
    setState(() {
      file = read;
      failure = null;
      _progress = null;
      if (read.hasSeals()) seals = read.seals;
      if (!read.isPreview) originalLoad = const OriginalNotLoaded();
    });
    if (!identical(read, shown)) loaded(read);
    noteShown(read, from: ViewedFrom.kept);
    if (fetch != null) return _askKept(backend, cache, found, read, fetch);
    final bulk = backend.bulkReading;
    final checking = Stopwatch()..start();
    final checked = await cache.check(backend, found);
    backend.noteViewer(ViewerKind.checked, {
      ViewerKey.file: viewedFile(read.mime),
      ViewerKey.kept: switch (checked?.current) {
        null => KeptCheck.changed,
        true => KeptCheck.current,
        false => KeptCheck.unasked,
      },
      ViewerKey.bulk: bulk,
    }, checking.elapsed);
    if (!mounted) return true;
    if (checked == null) return false;
    setState(() {
      _unchecked = !checked.current;
      if (checked.seals case final now?) seals = now;
    });
    return true;
  }

  /// Asks for the file the way the viewer reads it, unless it's still
  /// [found]'s version ([shown] from it): one round trip either way, the
  /// new version coming with the answer when it changed.
  Future<bool> _askKept(
    Backend backend,
    FileCache cache,
    KeptFile found,
    FileContent shown,
    Future<FileContent> Function(String unless) fetch,
  ) async {
    void noteChecked(KeptCheck kept, bool bulk, Duration took) =>
        backend.noteViewer(ViewerKind.checked, {
          ViewerKey.file: viewedFile(shown.mime),
          ViewerKey.kept: kept,
          ViewerKey.bulk: bulk,
        }, took);
    final bulk = backend.bulkReading;
    final checking = Stopwatch()..start();
    FileContent? answer;
    if (backend.connected.value) {
      try {
        answer = await fetch(found.sha256);
      } on RequestFailure {
        answer = null;
      }
    }
    if (answer == null) {
      noteChecked(KeptCheck.unasked, bulk, checking.elapsed);
      await cache.touch(found);
      if (mounted) setState(() => _unchecked = true);
      return true;
    }
    // A Claktory from before says so by its hash.
    final current =
        answer.unchanged ||
        answer.error.isEmpty && answer.sha256 == found.sha256;
    noteChecked(
      current ? KeptCheck.current : KeptCheck.changed,
      bulk,
      checking.elapsed,
    );
    if (current) {
      await cache.touch(found);
      if (!mounted) return true;
      setState(() {
        _unchecked = false;
        if (answer!.hasSeals()) seals = answer.seals;
      });
      return true;
    }
    // Gone: its copies go. Changed: the new version takes the copy's place.
    if (answer.error.isNotEmpty) await cache.drop(path);
    if (!mounted) {
      _keep(backend, answer);
      return true;
    }
    _showRead(backend, answer);
    return true;
  }

  /// [read] came over the connection: shown, and kept for next time.
  void _showRead(Backend backend, FileContent read) {
    setState(() {
      file = read;
      failure = null;
      _progress = null;
      _unchecked = false;
      seals = read.hasSeals() ? read.seals : null;
      if (!read.isPreview) originalLoad = const OriginalNotLoaded();
    });
    loaded(read);
    noteShown(read, from: ViewedFrom.read, bytes: read.data.length);
    _keep(backend, read);
  }

  /// Keeps [read] on this device, for next time.
  void _keep(Backend backend, FileContent read) {
    final cache = keptCopies(backend);
    if (cache == null || read.truncated || read.error.isNotEmpty) return;
    if (!keeps(read)) return;
    final variant = read.isPreview
        ? previewVariant(previewScreen())
        : originalVariant;
    unawaited(cache.keepRead(path, variant, read));
  }

  /// Whether [read] may be kept on this device: a viewer that reads files
  /// it wasn't asked to open (a Markdown file's pictures) keeps only
  /// what is what it says it is.
  @protected
  bool keeps(FileContent read) => true;

  /// How far the read on its way has got, while it's on its way.
  ({int through, int size})? _progress;

  /// What [waiting] says is loading: "Loading image…".
  String get loadingLabel => 'Loading…';

  /// Whether this viewer's file may come as a preview made for this
  /// screen (previews.dart says when).
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
    if (file == null && !_slow) {
      _slowFor ??= Timer(loadingShowsAfter, () {
        if (mounted && file == null) setState(() => _slow = true);
      });
    }
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
      // than its first 20 MB to no use.
      // A sealed version comes whole, never as a preview.
      final ask = previews && !wantOriginal && sealed == null;
      bool tooBig(int size) => size > wholeImageLimit;
      final slow = stopWhenSlow(backend);
      // With [unless], the version kept here: nothing comes while it's
      // still that one.
      Future<FileContent> fetch({String unless = ''}) async {
        var read = await backend.readFile(
          path,
          sealedSha256: sealedSha256,
          unlessSha256: unless,
          preview: ask && touchDevice ? previewScreen() : null,
          carryOn: !ask
              ? null
              : touchDevice
              ? (through, size) => !tooBig(size)
              : (through, size) => !tooBig(size) && slow(through, size),
          // A Claktory from before reads it as asked: its first piece's
          // hash says whether it's still the kept version.
          onFirst: unless.isEmpty ? null : (first) => first.sha256 != unless,
          onProgress: progress,
        );
        if (read.unchanged || !mounted) return read;
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
        }
        return read;
      }

      askingFor(backend);
      // Kept on this device: the original if it's here, else the preview
      // made for this screen, shown at once while asked about.
      final kept = await _showKept(backend, [
        originalVariant,
        if (previews && !wantOriginal) previewVariant(previewScreen()),
      ], fetch: (unless) => fetch(unless: unless));
      if (kept || !mounted) return;
      final read = await fetch();
      // Closed before it came: kept all the same, for next time.
      if (!mounted) return _keep(backend, read);
      _showRead(backend, read);
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

  /// [read] is on screen, from [from], [bytes] of it having come over the
  /// connection: diagnostics say how long it took, the first time in this
  /// viewer, once the frame it's in is drawn.
  @protected
  void noteShown(FileContent read, {required ViewedFrom from, int bytes = 0}) {
    if (_shownNoted || read.error.isNotEmpty) return;
    _shownNoted = true;
    final backend = _backend;
    if (backend == null) return;
    final bulk = _bulkWhenAsked ?? false;
    final waited = _waited;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      backend.noteViewer(ViewerKind.shown, {
        ViewerKey.file: viewedFile(read.mime),
        ViewerKey.from: from,
        ViewerKey.preview: read.isPreview,
        if (waited != null) ViewerKey.waitMs: waited.inMilliseconds,
        ViewerKey.bytes: bytes,
        ViewerKey.size: read.size.toInt(),
        ViewerKey.bulk: bulk,
      }, _opening.elapsed);
    });
  }

  /// Notes, before a viewer reading its file its own way asks for it,
  /// whether another file's bulk read is on its way ([noteShown]).
  @protected
  void askingFor(Backend backend) => _bulkWhenAsked ??= backend.bulkReading;

  /// The original in place of the preview shown, with how much has come;
  /// the preview stays until it's here, and if it can't come, says so.
  Future<void> loadOriginal() async {
    if (originalLoad is OriginalLoading) return;
    wantOriginal = true;
    setState(() => originalLoad = const OriginalLoading(0, 0));
    try {
      final backend = AppScope.of(context);
      // Kept on this device: at once.
      if (await _showKept(backend, [originalVariant]) || !mounted) return;
      final read = await backend.readFile(
        path,
        sealedSha256: sealedSha256,
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
    // Lost with the connection: read again once it's back, so it
    // waits for it rather than saying it failed.
    // Lost a moment before the connection says it's down, the same.
    // Given up on, it says so.
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
    // Nothing yet, past the moment a file on a near link takes: it's
    // loading, rather than a blank page.
    if (file == null && _slow && connected != null) {
      return LoadingProgress(
        label: loadingLabel,
        received: 0,
        size: 0,
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

/// The header's word on a file still the version its latest seal names.
const sealedDetail = 'Sealed';

/// The header's word on a sealed version of a file, shown in its place.
const sealedVersionDetail = 'Sealed version';

/// The header's word on a copy kept on this device that couldn't be
/// checked against the file, offline.
const mayBeOutOfDate = 'Saved copy, may be out of date';

/// How long a viewer shows nothing while its file comes: past it, that
/// it's loading. A file that comes sooner never flashes the loading state.
const loadingShowsAfter = Duration(milliseconds: 150);

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
  "Can't reach your Claktory right now.",
};
