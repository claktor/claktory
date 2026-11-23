// A document pane on an iPad (docs/ipad-workbench.md §6): a PDF, with ink
// on its pages, in Gelato's native surface (ink_surface.dart), under
// Claktory's own drawing tools (ink_tools.dart) and a line saying whether
// the ink is kept.
//
// What it takes from the native spike (docs/ipad-ink-spike.md):
// - One surface per leaf for the leaf's life, under the same GlobalKey, so
//   moving it between columns keeps its native view and its ink. The
//   window's DocumentSurfaces keeps them, outliving the pane's widgets.
// - A stroke cut off by a resize is lost; the pane says so.
// - While a stroke is down, the window's [DocumentSurfaces.drawing] is
//   true, so the panes hold their dividers.
// - The tools are Flutter's: PencilKit's picker takes the keyboard.
//
// What's drawn is kept on the backend as ink layers
// (docs/ipad-phase3-contract.md §3, §4) by Gelato's InkLayerSync, one per
// surface: the host tells it what native says (ready, a drawing changed, a
// new version shown, the view going) and shows what it says under the
// page, in Trish's words.
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'package:url_launcher/url_launcher.dart' show launchUrl;

import '../backend.dart';
import '../claktory_identity.dart' show claktoryName;
import '../controls.dart' show Button, ExtensionGlyph, GlyphButton;
import '../identity.dart';
import '../layout.dart' show chatOnly, leaves;
import '../mobile/device.dart' show Idiom, deviceCapabilities;
import '../mobile/metrics.dart' show TypeScale;
import '../pane_chrome.dart' show PaneAction, PaneScaffold;
// The bridge's InkComposite (what the native view exported) is this file's,
// not the protocol's review-item composite.
import '../protocol/factory/v1/factory.pb.dart' hide InkComposite;
import '../scope.dart';
import '../viewers/registry.dart' show ViewerProblem;
import 'ink_bridge.dart';
import 'ink_surface.dart';
import 'ink_sync.dart';
import 'ink_tools.dart';
import 'document_find.dart';
import 'document_review.dart';
import 'workbench.dart' show WorkbenchScope;

/// The pane kind a document drawn on is shown in (contract §2).
const documentKind = 'pdf';

/// [node]'s pane on an iPad when it's a document ([documentKind]): the
/// document host, which draws. Null anywhere else, for the viewers'.
/// [agentId] is whose layout it's in: the agent shown when null (the
/// workbench's panes); the phone routes of a narrow window name it.
Widget? ipadDocumentPane(
  BuildContext context,
  ViewNode node, {
  String? agentId,
}) {
  if (defaultTargetPlatform != TargetPlatform.iOS ||
      deviceCapabilities.idiom != Idiom.pad ||
      !node.hasLeaf() ||
      node.leaf.kind != documentKind) {
    return null;
  }
  return DocumentHost.forLeaf(context, node, agentId: agentId);
}

/// A project's PDF on an iPad, opened from its Files with no agent: the
/// document host, only to read (nothing draws, no review, nothing to
/// keep), so an iPad never says PDFs open elsewhere. Null anywhere else,
/// for the viewers'.
Widget? ipadProjectDocument(BuildContext context, ViewLeaf leaf) {
  if (defaultTargetPlatform != TargetPlatform.iOS ||
      deviceCapabilities.idiom != Idiom.pad ||
      leaf.kind != documentKind ||
      leaf.path.isEmpty) {
    return null;
  }
  return DocumentHost.forFile(context, leaf.path);
}

/// A range read's size: the backend's limit for a chunk.
@visibleForTesting
var documentChunk = 8 * 1024 * 1024;

/// The documents a window draws on, one [DocumentSurface] per agent's
/// leaf, kept while the leaf is in the agent's layout.
class DocumentSurfaces {
  final _surfaces = <String, DocumentSurface>{};

  /// Whether a stroke is down on any of them: the panes hold their
  /// dividers meanwhile, as a width change would drop it.
  ValueListenable<bool> get drawing => _drawing;
  final _drawing = ValueNotifier(false);

  /// [agentId]'s surface for [leaf], made the first time it's asked for.
  DocumentSurface of(String agentId, ViewNode leaf) =>
      _surfaces.putIfAbsent(_key(agentId, leaf), () {
        final surface = DocumentSurface._(leaf.id, leaf.leaf.path);
        surface._strokes.addListener(_restroke);
        return surface;
      });

  /// The surface for a project's file at [path], opened with no agent:
  /// only to read. Kept while its page is shown ([release]).
  DocumentSurface ofFile(String path) =>
      _surfaces.putIfAbsent('$_files$path', () {
        final surface = DocumentSurface._('file', path, readOnly: true);
        surface._strokes.addListener(_restroke);
        return surface;
      });

  /// The project file's page at [path] went.
  void release(String path) {
    final surface = _surfaces.remove('$_files$path');
    if (surface == null) return;
    surface._dispose();
    _restroke();
  }

  /// Keys of project files' surfaces: no agent id has a space.
  static const _files = 'file /';

  /// Drops [agentId]'s surfaces whose leaf has left its layout [root]
  /// (closed, or another file in it).
  void prune(String agentId, ViewNode root) {
    final kept = {for (final leaf in leaves(root)) _key(agentId, leaf)};
    final prefix = '$agentId/';
    final gone = [
      for (final key in _surfaces.keys)
        if (key.startsWith(prefix) && !kept.contains(key)) key,
    ];
    for (final key in gone) {
      _surfaces.remove(key)!._dispose();
    }
    if (gone.isNotEmpty) _restroke();
  }

  String _key(String agentId, ViewNode leaf) =>
      '$agentId/${leaf.id}/${leaf.leaf.path}';

  void _restroke() =>
      _drawing.value = _surfaces.values.any((s) => s._strokes.value);

  void dispose() {
    for (final surface in _surfaces.values) {
      surface._dispose();
    }
    _surfaces.clear();
    _drawing.dispose();
  }
}

/// What a document's side panel shows.
enum DocumentPanel { search, contents }

/// How long typing pauses before a search goes to native.
const searchPause = Duration(milliseconds: 250);

/// Makes a surface's [InkLayerSync] (tests give it a backend of their
/// own).
@visibleForTesting
InkLayerSync Function({
  required Backend backend,
  required String path,
  required String machine,
})
newInkSync = InkLayerSync.new;

/// One leaf's document and ink: its native surface's key and controller,
/// its tools, the file as fetched, what's been drawn, and what to tell the
/// person. Outlives the pane's widgets; [DocumentSurfaces] disposes it.
class DocumentSurface extends ChangeNotifier {
  DocumentSurface._(this.leafId, this.path, {this.readOnly = false}) {
    _events = controller.events.listen(_on);
  }

  final String leafId;
  final String path;

  /// Only to read: nothing draws (a project's file, opened with no agent
  /// to answer).
  final bool readOnly;

  /// The pane's and the native view's keys, the same wherever the leaf is
  /// shown.
  final hostKey = GlobalKey(debugLabel: 'document host');
  final surfaceKey = GlobalKey(debugLabel: 'ink surface');
  final controller = InkSurfaceController();
  final tools = InkToolsController();
  late final StreamSubscription<InkEvent> _events;

  /// The surface's creation params, once the file is here: the version
  /// shown, and the drawing policy as the tools have it now, for a view
  /// made again.
  InkParams? get params {
    final version = _current;
    if (version == null) return null;
    return InkParams(
      leafId: leafId,
      surface: InkSurfaceKind.pdf,
      source: version.source,
      document: version.document,
      layers: const [],
      drawingPolicy: readOnly
          ? InkDrawingPolicy.none
          : tools.fingerDraws
          ? InkDrawingPolicy.anyInput
          : InkDrawingPolicy.standard,
      persistence: 'online',
      active: true,
      paper: 0xFFFFFFFF,
      appearance: 'light',
    );
  }

  /// The version of the file shown, in the render cache.
  _Version? _current;

  /// Why the document can't show, in the person's words.
  String? get problem => _problem;
  String? _problem;

  /// How much of the file has come, while it comes.
  ({int received, int size})? get progress => _progress;
  ({int received, int size})? _progress;

  /// The page shown and how many there are (PDFs).
  ({int page, int count})? get page => _page;
  ({int page, int count})? _page;

  /// The page shown when the pane last went from the window (another
  /// agent shown, the columns hid it): its view, made again, opens there
  /// rather than on page 1. In memory only.
  int? _resumePage;

  /// What the person typed in answer to each question on this file, by
  /// the ask's seq, while the pane stays in the agent's layout: showing
  /// another agent and coming back keeps it (Prosciutto's S5). In memory
  /// only, never kept on the device.
  final answers = <int, String>{};

  /// Keeps what's drawn on the backend, once the file is here; says
  /// whether it's kept ([InkLayerSync.state]).
  InkLayerSync? get sync => _sync;
  InkLayerSync? _sync;

  /// The Claktory's name, as "Saved to" says it.
  String get machine => _machine;
  String _machine = '';
  set machine(String name) {
    if (name == _machine) return;
    _machine = name;
    _sync?.machine = name;
    notifyListeners();
  }

  /// "Keep both" on pages another device saved first: the person's ink
  /// is kept beside theirs.
  void keepBoth(List<int?> pages) {
    for (final page in pages) {
      unawaited(_quietly(_sync?.keepBoth(page) ?? Future.value()));
    }
  }

  /// A link to outside the document the person tapped, to offer to open
  /// (http, https and mailto only), until opened or dismissed.
  Uri? get link => _link;
  Uri? _link;

  void dismissLink() {
    if (_link == null) return;
    _link = null;
    notifyListeners();
  }

  /// The PDF's own contents, as its view read them (empty when it has
  /// none).
  List<InkOutlineEntry> get outline => _outline;
  var _outline = const <InkOutlineEntry>[];

  /// What's open at the page's side: a search, the contents, or nothing.
  DocumentPanel? get panel => _panel;
  DocumentPanel? _panel;

  /// The search as typed, and what native found for it (null while it
  /// looks, or with nothing typed).
  String get query => _query;
  var _query = '';
  InkSearchResult? get found => _found;
  InkSearchResult? _found;
  Timer? _searchSoon;
  var _searchSeq = 0;

  /// Opens [panel] at the page's side; one search ends when the panel
  /// shows something else.
  void openPanel(DocumentPanel panel) {
    if (_panel == panel) return;
    // A search kept on the page ([searchKept]) carries on in Search.
    if (panel != DocumentPanel.search) _endSearch();
    _panel = panel;
    notifyListeners();
  }

  /// Closes the side panel; a search's highlights go with it, unless
  /// [keepSearch]: a panel that covered the page closed to show a match,
  /// which stays marked among the others ([searchKept]).
  void closePanel({bool keepSearch = false}) {
    if (_panel == null) return;
    _panel = null;
    if (!keepSearch) _endSearch();
    notifyListeners();
  }

  /// A search's marks on the page with its panel closed: Search shows lit,
  /// and opens on it again; closing the panel there ends it.
  bool get searchKept => _panel == null && _query.isNotEmpty;

  void _endSearch() {
    if (_query.isNotEmpty) {
      _searchSoon?.cancel();
      _searchSeq++;
      _query = '';
      _found = null;
      unawaited(_quietly(controller.search('')));
    }
  }

  /// Searches for [query] once typing pauses; an answer to an older
  /// query, or one native superseded, is dropped.
  void search(String query, {Duration after = searchPause}) {
    if (query == _query) return;
    _query = query;
    _found = null;
    _searchSoon?.cancel();
    final seq = ++_searchSeq;
    notifyListeners();
    if (query.trim().isEmpty) {
      // Nothing to find: the highlights go at once.
      unawaited(_quietly(controller.search('')));
      return;
    }
    _searchSoon = Timer(after, () async {
      try {
        final found = await controller.search(query.trim());
        if (_disposed || seq != _searchSeq || found.superseded) return;
        _found = found;
      } on Object {
        if (_disposed || seq != _searchSeq) return;
        _found = InkSearchResult(
          query: query,
          matches: const [],
          truncated: false,
          superseded: false,
        );
      }
      notifyListeners();
    });
  }

  /// Shows [page], at [region] (0..1 of the page) when given; [match]: the
  /// region is the search's match gone to, marked apart from the others
  /// and brought high into the view.
  void goTo(int page, {Rect? region, bool match = false}) => unawaited(
    _quietly(controller.goToPage(page, region: region, match: match)),
  );

  /// Whether this iPad reads handwriting (iPadOS 27 and later), as its
  /// view said when ready.
  InkHandwriting get handwriting => _handwriting;
  var _handwriting = InkHandwriting.unavailable;

  /// The strokes chosen with the lasso, while some are.
  ({int? page, int strokes})? get strokeSelection => _strokeSelection;
  ({int? page, int strokes})? _strokeSelection;

  /// Counts the changes to what's drawn and chosen: a reading started
  /// before the latest is out of date.
  int get inkSeq => _inkSeq;
  var _inkSeq = 0;

  /// Reads the handwriting of the lasso's strokes, or of every drawn page,
  /// as one text, each page's on a line below the one before: '' when
  /// there's none, null when the ink changed while it read (an answer to
  /// drop, saying nothing). Native never applies or sends it.
  Future<String?> readHandwriting() async {
    final at = _inkSeq;
    final selection = _strokeSelection;
    final reads = selection != null
        ? [(selection.page, InkRecognitionScope.selection)]
        : [for (final page in inkedPages) (page, InkRecognitionScope.all)];
    final texts = <String>[];
    for (final (page, scope) in reads) {
      final read = await controller.recognizeText(page, scope: scope);
      if (_disposed || read.stale || _inkSeq != at) return null;
      if (!read.available) {
        // Below iPadOS 27 after all: no offer.
        _handwriting = InkHandwriting.unavailable;
        notifyListeners();
        return null;
      }
      if (read.candidate case final text?) texts.add(text);
    }
    return texts.join('\n');
  }

  /// A stroke was cut off and lost (the pane changed size).
  bool get strokeLost => _strokeLost;
  var _strokeLost = false;

  final _strokes = ValueNotifier(false);

  /// The pages with ink on them now, in order: drawn here, kept on the
  /// backend or saved from another device.
  List<int> get inkedPages => _sync?.inkedPages ?? const [];

  /// The pages drawn on, in order: those with ink now, and those whose ink
  /// was on an earlier version of the PDF ([earlier]).
  List<int> get drawnPages =>
      {...inkedPages, ..._earlier.keys}.toList()..sort();

  /// Pages whose ink was drawn on an earlier version of the PDF, which
  /// changed while it was open: each page as it was, with its ink, drawn
  /// out before the new version came. Native no longer shows that ink, and
  /// never moves it onto the new page; it still goes with a review's
  /// answer.
  Map<int, InkComposite> get earlier => _earlier;
  final _earlier = <int, InkComposite>{};

  /// [page] whole, with its ink, as a PNG small enough for an agent's
  /// model to read (contract §5's context image limits). A page with no
  /// ink now but ink on the earlier version gives that version's.
  Future<InkComposite> exportPage(int page) {
    if (!inkedPages.contains(page)) {
      if (_earlier[page] case final composite?) {
        return Future.value(composite);
      }
    }
    final layer = _sync?.layerOf(page);
    return controller.exportComposite(
      InkExportRequest(
        page1based: page,
        inkLayerId: layer?.inkLayerId,
        layerRevision: layer?.revision,
      ),
    );
  }

  var _fetching = false;
  var _disposed = false;

  /// The last read failed as a request (not connected, say): it's made
  /// again when the Claktory connects.
  bool get retry => _retry;
  var _retry = false;

  /// Fetches the file into the device's render cache, once: a range at a
  /// time, the same version throughout.
  Future<void> fetch(Backend backend, {required String claktory}) async {
    if (_fetching || _current != null || _disposed) return;
    _fetching = true;
    _problem = null;
    _retry = false;
    notifyListeners();
    try {
      final version = await _version(backend, claktory);
      if (_disposed) return;
      _current = version;
      _shown = version.source.sha256;
      if (_sync == null) {
        _sync = newInkSync(backend: backend, path: path, machine: _machine);
        _sync!.changes.addListener(notifyListeners);
      }
    } on _Problem catch (problem) {
      _problem = problem.text;
    } on RequestFailure catch (failure) {
      // Not connected, say: read again when it connects.
      _problem = failure.message;
      _retry = true;
    } on PlatformException catch (error) {
      _problem = error.code == 'hash_mismatch'
          ? 'This file changed while it was opening. Close it and open it again.'
          : "Claktory can't open this file here.";
    } finally {
      _fetching = false;
      _progress = null;
      if (!_disposed) notifyListeners();
    }
  }

  /// The version shown (its SHA-256), once there's one.
  String? get shown => _shown;
  String? _shown;

  /// The PDF changed while it was open and its new version shows, until
  /// the person draws. Which pages' ink is from the earlier version, the
  /// sync says ([InkOutdated]).
  bool get changed => _changed;
  var _changed = false;

  var _reloading = false;
  var _reloadAgain = false;

  /// The file changed on the machine (now [sha256], when known): fetches
  /// the new version and shows it in place, at the same page. Ink on pages
  /// that stay the same stays; ink on pages that changed or went is set
  /// aside ([earlier]), drawn out first, as it was. A new version that
  /// can't be shown leaves the one shown, and the next change tries again.
  Future<void> reload(
    Backend backend, {
    required String claktory,
    String? sha256,
  }) async {
    if (_disposed) return;
    if (_current == null) return fetch(backend, claktory: claktory);
    if (sha256 != null && sha256 == _shown) return;
    if (_reloading) {
      _reloadAgain = true;
      return;
    }
    _reloading = true;
    try {
      final version = await _version(backend, claktory);
      if (_disposed || version.source.sha256 == _shown) return;
      // Each page with ink, as drawn on the version shown: its ink won't
      // show on a page that changes.
      final before = <int, InkComposite>{};
      for (final page in inkedPages) {
        try {
          before[page] = await exportPage(page);
        } on Object {
          // Not drawn out: its ink can't go with an answer.
        }
      }
      if (_disposed) return;
      final reloaded = await controller.reloadDocument(
        version.source,
        version.document,
      );
      if (_disposed) return;
      _shown = reloaded.sourceSha256;
      // A view made again shows the new version.
      _current = version;
      for (final layer in reloaded.outdatedLayers) {
        final page = layer.page1based;
        if (page == null) continue;
        if (before[page] case final composite?) _earlier[page] = composite;
      }
      // The sync sets the earlier version's ink aside and carries the rest
      // onto the new one.
      unawaited(_quietly(_sync?.reloaded(reloaded) ?? Future.value()));
      _changed = true;
      _page = (
        page: reloaded.page1based ?? _page?.page ?? 1,
        count: reloaded.pageCount,
      );
      _strokeLost = false;
      _inkSeq++;
      // The new version's matches.
      if (_query.trim().isNotEmpty) {
        final query = _query;
        _query = '';
        search(query, after: Duration.zero);
      }
    } on _Problem {
      // Changing as it came: the next change tries again.
    } on RequestFailure {
      // Not connected: the next change, or connecting, tries again.
    } on PlatformException {
      // Native couldn't show it (too long, locked, another version than
      // asked for): the version shown stays.
    } finally {
      _reloading = false;
      _progress = null;
      if (!_disposed) {
        notifyListeners();
        if (_reloadAgain) {
          _reloadAgain = false;
          unawaited(reload(backend, claktory: claktory));
        }
      }
    }
  }

  /// The file as it is now, in the render cache. A file that changes as it
  /// comes is read again from its start, with its new version, a few times
  /// before giving up.
  Future<_Version> _version(Backend backend, String claktory) async {
    for (var attempt = 0; ; attempt++) {
      try {
        return await _versionOnce(backend, claktory);
      } on _Changed {
        if (attempt >= 2) throw const _Problem(_changing);
        _progress = null;
      }
    }
  }

  Future<_Version> _versionOnce(Backend backend, String claktory) async {
    final first = await backend.readFile(path, maxBytes: documentChunk);
    if (first.error.isNotEmpty) throw _Problem(first.error);
    final size = first.size.toInt();
    final sha = first.sha256;
    if (sha.isEmpty) throw const _Problem(_update);
    final name = path.split('/').last;
    final source = InkSource(
      backend: claktory,
      path: path,
      sha256: sha,
      mime: first.mime,
      size: size,
    );
    var file = await InkRenderCache.lookup(
      backend: claktory,
      sha256: sha,
      name: name,
    );
    if (file == null) {
      final writer = await InkRenderCache.begin(
        backend: claktory,
        sha256: sha,
        name: name,
      );
      try {
        await writer.append(Uint8List.fromList(first.data));
        var offset = first.data.length;
        while (offset < size) {
          _progress = (received: offset, size: size);
          if (!_disposed) notifyListeners();
          final chunk = await backend.readFile(
            path,
            maxBytes: documentChunk,
            offset: offset,
            expectedSha256: sha,
          );
          if (_disposed) throw const _Problem('');
          if (chunk.error == 'changed' || chunk.sha256 != sha) {
            throw const _Changed();
          }
          if (chunk.error.isNotEmpty) throw _Problem(chunk.error);
          if (chunk.offset.toInt() != offset || chunk.data.isEmpty) {
            throw const _Problem(_update);
          }
          await writer.append(Uint8List.fromList(chunk.data));
          offset += chunk.data.length;
        }
        file = await writer.finish();
      } catch (_) {
        await writer.cancel().catchError((Object _) {});
        rethrow;
      }
    }
    return (
      source: source,
      document: InkDocument.file(file: file, sha256: sha),
    );
  }

  void _on(InkEvent event) {
    if (_disposed) return;
    switch (event) {
      case InkReady(
        :final pageCount,
        :final surface,
        :final handwriting,
        :final outline,
      ):
        _handwriting = handwriting;
        _outline = outline;
        // A view made again has no highlights: search again.
        if (_query.trim().isNotEmpty) {
          final query = _query;
          _query = '';
          search(query, after: Duration.zero);
        }
        _strokeSelection = null;
        _inkSeq++;
        // What's kept of this version, or for a view made again what's
        // held, saved or not.
        if ((_sync, _shown) case (final sync?, final shown?)) {
          unawaited(_quietly(sync.ready(controller, sourceSha256: shown)));
        }
        unawaited(_quietly(controller.setTool(tools.choice)));
        final resume = _resumePage;
        _resumePage = null;
        if (surface == InkSurfaceKind.pdf && pageCount > 0) {
          _page = (page: resume ?? _page?.page ?? 1, count: pageCount);
          // A view made again goes back to the page it was on.
          if (resume != null && resume > 1 && resume <= pageCount) {
            goTo(resume);
          }
        }
      case InkPageChanged(:final page1based, :final pageCount):
        _page = (page: page1based, count: pageCount);
      case InkDrawingChanged():
        unawaited(
          _quietly(_sync?.changed(controller, event) ?? Future.value()),
        );
        _inkSeq++;
        _strokeLost = false;
        // Drawing on the new version: no need to say it's shown. Ink from
        // the earlier one is still said.
        _changed = false;
      case InkUndoState(:final canUndo, :final canRedo):
        tools.undoState(canUndo: canUndo, canRedo: canRedo);
        return;
      case InkToolChanged(:final tool):
        tools.follow(tool);
        return;
      case InkStrokeActive(:final active):
        _strokes.value = active;
        return;
      case InkStrokeSelectionChanged(:final page1based, :final strokeCount):
        _strokeSelection = strokeCount > 0
            ? (page: page1based, strokes: strokeCount)
            : null;
        _inkSeq++;
      case InkStrokeInterrupted(:final kept, :final reason):
        // Hidden or undone: the person did that. A resize or a rotation
        // took it from them.
        if (!kept && (reason == 'resize' || reason == 'rotate')) {
          _strokeLost = true;
        }
      case InkError(:final code) when code == 'document_too_large':
        final pages = event.pageCount;
        final most = event.maxPages;
        _problem = pages == null || most == null
            ? 'This PDF is too long to open here.'
            : 'This PDF has ${_count(pages)} pages, more than the '
                  '${_count(most)} Claktory can open here.';
      case InkError(:final code):
        _problem = switch (code) {
          'os_too_old' => 'Drawing on PDFs needs iPadOS 16 or later.',
          'document_hash_mismatch' || 'document_changed' => 'This file changed while it was opening. Close it and open it again.',
          'drawing_unreadable' => "Some drawings on this file can't be read.",
          _ => "Claktory can't open this file here.",
        };
      case InkLinkTapped(:final url):
        _link = _offerable(url);
      case InkSelectionChanged() || InkUnknownEvent():
        return;
    }
    notifyListeners();
  }

  /// Sends the person's tool to the surface.
  void setTool(InkTool tool) => unawaited(_quietly(controller.setTool(tool)));

  void undo() => unawaited(_quietly(controller.undo().then(_undoState)));

  void redo() => unawaited(_quietly(controller.redo().then(_undoState)));

  void _undoState(InkUndoState state) =>
      tools.undoState(canUndo: state.canUndo, canRedo: state.canRedo);

  void setFingerDraws(bool on) => unawaited(
    _quietly(
      controller.setDrawingPolicy(
        on ? InkDrawingPolicy.anyInput : InkDrawingPolicy.standard,
      ),
    ),
  );

  /// The pane went from the window (hidden by the columns, or the window
  /// narrowed): whatever native holds is sent before its view goes, and
  /// saved now.
  void hidden() {
    _resumePage = _page?.page;
    unawaited(_hide());
  }

  Future<void> _hide() async {
    await _quietly(controller.setActive(false));
    await _quietly(_sync?.flush() ?? Future.value());
  }

  void dismissStrokeLost() {
    if (!_strokeLost) return;
    _strokeLost = false;
    notifyListeners();
  }

  /// A command to a surface that went: nothing to tell anyone.
  Future<void> _quietly(Future<Object?> command) =>
      command.then<void>((_) {}, onError: (Object _) {});

  void _dispose() {
    _disposed = true;
    _searchSoon?.cancel();
    unawaited(_events.cancel());
    tools.dispose();
    _strokes.dispose();
    final sync = _sync;
    if (sync == null) {
      controller.dispose();
    } else {
      // A save under way finishes first (its export is done; what's left
      // is the backend's), then the sync and the controller go.
      sync.changes.removeListener(notifyListeners);
      unawaited(
        sync
            .flush()
            .timeout(_lastSave, onTimeout: () {})
            .then((_) {}, onError: (Object _) {})
            .whenComplete(() {
              sync.dispose();
              controller.dispose();
            }),
      );
    }
    dispose();
  }

  /// How long a closed pane's last save gets.
  static const _lastSave = Duration(seconds: 30);
}

/// [url] when it's one to offer to open: a web page or an email.
Uri? _offerable(String url) {
  final uri = Uri.tryParse(url);
  if (uri == null) return null;
  return const {'http', 'https', 'mailto'}.contains(uri.scheme) ? uri : null;
}

/// 2400 as "2,400".
String _count(int n) =>
    n.toString().replaceAllMapped(RegExp(r'\B(?=(\d{3})+(?!\d))'), (_) => ',');

const _changing =
    'This file keeps changing while it opens. Try again once it stops changing.';

/// A version of the file, in the render cache.
typedef _Version = ({InkSource source, InkDocument document});

/// The file became another version while it came: start again.
class _Changed implements Exception {
  const _Changed();
}

const _update =
    "To draw on PDFs here, update Claktory on the computer it runs on.";

class _Problem implements Exception {
  const _Problem(this.text);
  final String text;
}

/// [leaf]'s document in its pane: the tools, the surface and what's kept.
/// On an iPad only, in a window with a workbench focus (main.dart's).
class DocumentHost extends StatelessWidget {
  const DocumentHost({super.key, required this.leaf});

  final ViewLeaf leaf;

  /// The surface for [leaf] of the agent shown, under its own key so the
  /// pane keeps its state wherever it moves.
  static Widget forLeaf(
    BuildContext context,
    ViewNode leaf, {
    String? agentId,
  }) {
    final focus = WorkbenchScope.maybeOf(context);
    final agent = agentId ?? AppScope.of(context).workspace.selectedAgent?.id;
    if (focus == null || agent == null) {
      return const ViewerProblem("Claktory can't show this kind of file here.");
    }
    final surface = focus.documents.of(agent, leaf);
    final store = AppScope.of(context).workspace;
    return _Host(
      key: surface.hostKey,
      agentId: agent,
      surface: surface,
      // The pane went: if it left the agent's layout (closed, even the last
      // one, which leaves no panes to lay out and so nothing else to drop
      // it), its surface goes, and its last save finishes or gives up.
      onGone: () {
        final now = store.agent(agent);
        focus.documents.prune(
          agent,
          now != null && now.hasLayout() ? now.layout : chatOnly(),
        );
      },
    );
  }

  /// A project's file at [path], with no agent: only to read.
  static Widget forFile(BuildContext context, String path) {
    final focus = WorkbenchScope.maybeOf(context);
    if (focus == null) {
      return const ViewerProblem("Claktory can't show this kind of file here.");
    }
    final surface = focus.documents.ofFile(path);
    return _Host(
      key: surface.hostKey,
      surface: surface,
      onGone: () => focus.documents.release(path),
    );
  }

  @override
  Widget build(BuildContext context) => const SizedBox.shrink();
}

class _Host extends StatefulWidget {
  const _Host({super.key, this.agentId, required this.surface, this.onGone});

  /// Whose pane it is; null for a project's file, which no agent asks
  /// about.
  final String? agentId;
  final DocumentSurface surface;

  /// The host went: a project's file's surface goes too; an agent's pane's
  /// goes if the pane left its layout.
  final VoidCallback? onGone;

  @override
  State<_Host> createState() => _HostState();
}

class _HostState extends State<_Host> {
  DocumentSurface get surface => widget.surface;

  Backend? _backend;
  var _claktory = 'local';

  /// The question an agent asks of the file; none for a project's file.
  late final _review = widget.agentId == null
      ? null
      : ReviewWatch(agentId: widget.agentId!, path: surface.path);

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final scope = AppScope.scope(context);
    final name = scope.connections.current?.name.trim() ?? '';
    _claktory = name.isEmpty ? 'local' : name;
    if (scope.backend != _backend) {
      _backend?.connected.removeListener(_connectedChanged);
      _backend?.fileChanged.removeListener(_fileChanged);
      _backend = scope.backend
        ..connected.addListener(_connectedChanged)
        ..fileChanged.addListener(_fileChanged);
    }
    _review?.follow(scope.backend);
    // After the frame: the name republishes what's kept.
    final machine = claktoryName(scope.backend);
    if (machine != surface.machine) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) surface.machine = machine;
      });
    }
    _fetch();
  }

  void _fetch() {
    final backend = _backend;
    if (backend == null) return;
    unawaited(surface.fetch(backend, claktory: _claktory));
  }

  /// A read that failed before the Claktory connected is made again once
  /// it does (a cold start over the tunnel).
  void _connectedChanged() {
    if ((_backend?.connected.value ?? false) && surface.retry) _fetch();
  }

  /// The file changed on the machine: its new version, shown in place.
  /// One that's gone leaves the version shown.
  void _fileChanged() {
    final backend = _backend;
    final changed = backend?.fileChanged.value;
    if (backend == null || changed == null || changed.path != surface.path) {
      return;
    }
    if (changed.sha256.isEmpty) return;
    unawaited(
      surface.reload(backend, claktory: _claktory, sha256: changed.sha256),
    );
  }

  @override
  void dispose() {
    _backend?.connected.removeListener(_connectedChanged);
    _backend?.fileChanged.removeListener(_fileChanged);
    _review?.dispose();
    surface.hidden();
    final gone = widget.onGone;
    if (gone != null) {
      // After the frame: the page may be opened again meanwhile.
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (surface.hostKey.currentState == null) gone();
      });
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([surface, ?_review]),
    builder: (context, _) {
      final palette = Palette.of(context);
      final type = TypeScale.of(context);
      final problem = surface.problem;
      final params = surface.params;
      final page = surface.page;
      final Widget body;
      if (problem != null) {
        body = ViewerProblem(problem, path: surface.path);
      } else if (params == null) {
        final progress = surface.progress;
        body = Center(
          child: Text(
            progress == null
                ? 'Opening…'
                : 'Opening… ${(100 * progress.received / progress.size).floor()}%',
            style: type.label.copyWith(color: palette.textMuted),
          ),
        );
      } else {
        body = Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // Only to read: no tools.
            if (!surface.readOnly) ...[
              InkTools(
                controller: surface.tools,
                onChoice: surface.setTool,
                onUndo: surface.undo,
                onRedo: surface.redo,
                onFingerDraws: surface.setFingerDraws,
              ),
              Container(height: 1, color: palette.hairline),
            ],
            Expanded(
              child: LayoutBuilder(
                builder: (context, box) {
                  final covers = box.maxWidth < findPanelCovers;
                  final beside = surface.panel != null && !covers;
                  return Stack(
                    children: [
                      // Beside the panel in a wide pane, the page narrows to
                      // what's left, all of it still in view; a narrow pane's
                      // panel covers it. Either way the surface keeps its
                      // place in the tree.
                      PositionedDirectional(
                        start: beside ? findPanelWidth : 0,
                        end: 0,
                        top: 0,
                        bottom: 0,
                        child: InkSurface(
                          key: surface.surfaceKey,
                          params: params,
                          controller: surface.controller,
                        ),
                      ),
                      if (surface.panel != null)
                        PositionedDirectional(
                          start: 0,
                          top: 0,
                          bottom: 0,
                          width: covers ? box.maxWidth : findPanelWidth,
                          child: DocumentFindPanel(
                            key: ValueKey(surface.panel),
                            surface: surface,
                            covers: covers,
                          ),
                        ),
                    ],
                  );
                },
              ),
            ),
          ],
        );
      }
      void toggle(DocumentPanel panel) => surface.panel == panel
          ? surface.closePanel()
          : surface.openPanel(panel);
      final canSearch = params != null && problem == null;
      final hasContents = canSearch && surface.outline.isNotEmpty;
      final status = params == null ? null : _statusOf(surface);
      final foot = [
        if (params != null && surface.link != null)
          _LinkOffer(surface: surface),
        if (status != null)
          _StatusLine(text: status.text, keepBoth: status.keepBoth),
        // Answerable even when the file can't show.
        if (_review case final review? when review.ask != null)
          DocumentReviewBar(watch: review, surface: surface),
      ];
      return PaneScaffold(
        details: page == null ? '' : 'Page ${page.page} of ${page.count}',
        controls: [
          if (canSearch)
            GlyphButton(
              key: const ValueKey('document-search'),
              label: 'Search',
              active:
                  surface.panel == DocumentPanel.search || surface.searchKept,
              onTap: () => toggle(DocumentPanel.search),
              glyph: (color) =>
                  ExtensionGlyph(icon: 'search', name: '', color: color),
            ),
          if (hasContents)
            GlyphButton(
              key: const ValueKey('document-contents'),
              label: 'Contents',
              active: surface.panel == DocumentPanel.contents,
              onTap: () => toggle(DocumentPanel.contents),
              glyph: (color) =>
                  ExtensionGlyph(icon: 'menu', name: '', color: color),
            ),
        ],
        actions: [
          if (canSearch)
            PaneAction(
              'Search',
              () => surface.openPanel(DocumentPanel.search),
              active: surface.panel == DocumentPanel.search,
            ),
          if (hasContents)
            PaneAction(
              'Contents',
              () => surface.openPanel(DocumentPanel.contents),
              active: surface.panel == DocumentPanel.contents,
            ),
        ],
        body: LayoutBuilder(
          builder: (context, box) => Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(child: body),
              // The page runs on under the home indicator; what's under it
              // stays above. At the largest text in a short pane it takes
              // at most [footShare] of it, and scrolls, so the page stays.
              if (foot.isNotEmpty)
                ConstrainedBox(
                  constraints: BoxConstraints(
                    maxHeight: box.maxHeight * footShare,
                  ),
                  child: SingleChildScrollView(
                    child: Padding(
                      padding: EdgeInsets.only(
                        bottom: MediaQuery.paddingOf(context).bottom,
                      ),
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: foot,
                      ),
                    ),
                  ),
                ),
            ],
          ),
        ),
      );
    },
  );
}

/// The most of a document's pane what's under the page (the link offer,
/// the status, the review) takes; past it, that scrolls.
const footShare = 0.6;

/// Opens a link from a document (tests replace it).
@visibleForTesting
Future<bool> Function(Uri url) openDocumentLink = launchUrl;

/// Under the page, after a tap on a link to outside the document: where it
/// goes, to open or not. Nothing opens by itself.
class _LinkOffer extends StatelessWidget {
  const _LinkOffer({required this.surface});

  final DocumentSurface surface;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final link = surface.link!;
    final where = link.scheme == 'mailto'
        ? link.path
        : (link.host.isEmpty ? link.toString() : link.host);
    return Container(
      key: const ValueKey('document-link'),
      padding: const EdgeInsets.symmetric(
        horizontal: Space.l,
        vertical: Space.s,
      ),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: palette.hairline)),
      ),
      // The words, then the buttons under them at the end: fits any pane
      // at any text size.
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            link.scheme == 'mailto' ? 'Email $where?' : 'Open $where?',
            style: type.label.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.s),
          Wrap(
            alignment: WrapAlignment.end,
            spacing: Space.s,
            runSpacing: Space.s,
            children: [
              Button(label: 'Not now', onPressed: surface.dismissLink),
              Button(
                label: 'Open',
                primary: true,
                onPressed: () {
                  surface.dismissLink();
                  unawaited(
                    openDocumentLink(link).then((_) {}, onError: (_) {}),
                  );
                },
              ),
            ],
          ),
        ],
      ),
    );
  }
}

/// What to say under the page, the most urgent first: a stroke lost,
/// ink another device saved first (with "Keep both"), ink not kept, ink
/// from the PDF's earlier version, the new version shown, then saving and
/// saved. Nothing while nothing's been drawn. A project's PDF, only to
/// read, says only that it changed.
({String text, VoidCallback? keepBoth})? _statusOf(DocumentSurface surface) {
  if (surface.strokeLost) {
    return (
      text:
          'Your last stroke was lost when the pane changed size. '
          'Draw it again.',
      keepBoth: null,
    );
  }
  final state = surface.readOnly ? null : surface.sync?.state.value;
  final machine = surface.machine;
  String pages(List<int?> pages) => pageList(pages.nonNulls.toList());
  final urgent = switch (state) {
    InkConflict(pages: final at) => (
      text: at.length == 1
          ? 'Page ${pages(at)} was saved on another device first. '
                "Your drawing on it isn't saved yet."
          : 'Pages ${pages(at)} were saved on another device first. '
                "Your drawings on them aren't saved yet.",
      keepBoth: () => surface.keepBoth(at),
    ),
    InkUnsaved(:final reason) => (
      text: switch (reason) {
        InkOffline() =>
          'Not connected to $machine. Your drawing is kept on this iPad '
              'and saves the next time this PDF is open and $machine is back.',
        InkUnsupported() =>
          'Not saved. To keep drawings, update Claktory on the computer it '
              'runs on.',
        InkRefused(:final message) => 'Not saved. $message',
        InkFailed() =>
          "Couldn't save your drawing. Claktory tries again the next time "
              'you draw.',
      },
      keepBoth: null,
    ),
    InkOutdated(pages: final at) => (
      text:
          'This PDF changed. Your drawing on '
          '${at.length == 1 ? 'page' : 'pages'} ${pages(at)} is from the '
          'earlier version.',
      keepBoth: null,
    ),
    _ => null,
  };
  if (urgent != null) return urgent;
  if (surface.changed) {
    return (text: 'This PDF changed. Showing the new version.', keepBoth: null);
  }
  return switch (state) {
    InkSaveInProgress(pages: final at) => (
      text: at.length > 1 ? 'Saving your drawings…' : 'Saving your drawing…',
      keepBoth: null,
    ),
    InkSaved() => (text: 'Saved to $machine', keepBoth: null),
    _ => null,
  };
}

/// [5] as "5", [5, 7] as "5 and 7", [2, 5, 7] as "2, 5 and 7".
String pageList(List<int> pages) => switch (pages) {
  [] => '',
  [final one] => '$one',
  _ => '${pages.sublist(0, pages.length - 1).join(', ')} and ${pages.last}',
};

/// Under the page: [_statusOf], with "Keep both" when another device
/// saved first.
class _StatusLine extends StatelessWidget {
  const _StatusLine({required this.text, this.keepBoth});

  final String text;
  final VoidCallback? keepBoth;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Semantics(
      liveRegion: true,
      child: Container(
        key: const ValueKey('document-status'),
        constraints: const BoxConstraints(minHeight: 32),
        padding: const EdgeInsets.symmetric(
          horizontal: Space.l,
          vertical: Space.s,
        ),
        decoration: BoxDecoration(
          border: Border(top: BorderSide(color: palette.hairline)),
        ),
        // The words, then the button under them at the end: fits any pane
        // at any text size.
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(text, style: type.caption.copyWith(color: palette.textMuted)),
            if (keepBoth case final onPressed?) ...[
              const SizedBox(height: Space.s),
              Align(
                alignment: AlignmentDirectional.centerEnd,
                child: Button(
                  key: const ValueKey('document-keep-both'),
                  label: 'Keep both',
                  onPressed: onPressed,
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
