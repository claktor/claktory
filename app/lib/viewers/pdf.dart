// The Mac's PDF viewer: the file brought from the machine Claktory runs on
// into the device's render cache, a range at a time from one version, then
// shown by PDFKit's own view (app/macos/Runner/PaperViewer.swift, through
// ipad/ink_surface.dart's surface, without ink). The trackpad scrolls and
// zooms its pages, their text selects and copies as anywhere on the Mac,
// and its links never open by themselves: the pane asks first, as on the
// iPad. Find is the app's own (find.dart): Search or ⌘F opens its bar at
// the pane's top right, PDFKit's search behind it ([_PdfFind]).
//
// An iPhone shows it the same way, as a page of its own, through PDFKit's
// iOS view with nothing drawing (ink_surface.dart's surface, its drawing
// policy none): a finger scrolls, zooms and selects. Search is in More,
// and opens the same find, whose bar the phone's page shows at its foot.
//
// On both, ink saved on an iPad shows over its page, only read: each
// page's latest layer of the version shown, loaded into the native view as
// the iPad loads it ([_PdfInk]), and again as it's saved anywhere.
import 'dart:async';
import 'dart:io';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'package:url_launcher/url_launcher.dart' show launchUrl;

import '../backend.dart';
import '../controls.dart';
import '../find.dart';
import '../identity.dart';
import '../ipad/ink_bridge.dart';
import '../ipad/ink_surface.dart';
import '../ipad/ink_sync.dart' show InkBlobs;
import '../pane_chrome.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../mobile/metrics.dart' show TypeScale;
import '../mobile/viewer_chrome.dart' show ChromeTap, ChromeTapState;
import '../paced.dart' show FilePreviews;
import '../resources.dart' show formatBytes;
import '../scope.dart';
import 'file_cache.dart';
import 'loading_progress.dart';
import 'previews.dart';
import 'registry.dart';

/// How much of a PDF one read brings (the backend's cap).
@visibleForTesting
var pdfChunk = 8 * 1024 * 1024;

/// Over this, a PDF on a phone (or on a slow link) shows its pages as
/// images made for the screen while it comes.
const pdfPagesFrom = 5 * 1000 * 1000;

/// Opens a link from a PDF (tests replace it).
@visibleForTesting
Future<bool> Function(Uri url) openPdfLink = launchUrl;

/// A PDF in a pane on a Mac, or a page on an iPhone: the pages, "Page 3
/// of 12" in the header, and Search.
class PdfViewer extends StatefulWidget {
  const PdfViewer({super.key, required this.path});

  final String path;

  @override
  State<PdfViewer> createState() => _PdfViewerState();
}

class _PdfViewerState extends State<PdfViewer> with FileLoading<PdfViewer> {
  @override
  String get path => widget.path;

  @override
  String get loadingLabel => 'Loading PDF…';

  /// The page's surface: a new one for each version of the file, since
  /// native opens its document once.
  InkSurfaceController? _controller;
  StreamSubscription<InkEvent>? _events;

  /// The iPad's ink over this version's pages.
  _PdfInk? _ink;
  InkParams? _params;
  Key? _surfaceKey;

  /// A phone's tap on the page, which a link tapped takes back.
  final _tap = GlobalKey<ChromeTapState>();

  /// Text on the page is selected: a tap ends that, nothing more.
  var _selected = false;

  String? _problem;
  ({int received, int size})? _progress;
  ({int page, int count})? _page;
  Uri? _link;

  /// Brings the file again whenever asked to reload.
  var _generation = 0;

  /// A big PDF's pages as images while the PDF itself comes; null
  /// once it's shown, or when it isn't big.
  _PagePreviews? _pages;

  /// Page images were due, but this Claktory can't make them.
  var _noPreview = false;

  /// Search was asked for while only the images show: it says when.
  var _searchEarly = false;
  Timer? _searchEarlyFor;

  /// Find's engine: PDFKit's search in the page's surface.
  late final _find = _PdfFind(() => _controller);

  /// This pane's find, whose bar the app shows (find.dart); null before
  /// its scope is built.
  FindSession? get _session => FindSessions.instance.within(context);

  @override
  void dispose() {
    _searchEarlyFor?.cancel();
    _find.dispose();
    _generation++;
    _drop();
    _hold(null);
    super.dispose();
  }

  void _drop() {
    _ink?.dispose();
    _ink = null;
    unawaited(_events?.cancel());
    _events = null;
    _controller?.dispose();
    _controller = null;
  }

  @override
  Future<void> reload() async {
    final generation = ++_generation;
    final backend = AppScope.of(context);
    final scope = AppScope.scope(context);
    final name = scope.connections.current?.name.trim() ?? '';
    final claktory = name.isEmpty ? 'local' : name;
    setState(() {
      _problem = null;
      failure = null;
      _pages = null;
      _noPreview = false;
    });
    try {
      // A file that changes as it comes is read again from its start, with
      // its new version, a few times before giving up.
      for (var attempt = 0; ; attempt++) {
        try {
          final params = await _fetch(backend, claktory, generation);
          if (params == null || !mounted || generation != _generation) return;
          _show(params);
          _hold(_fetched);
          return;
        } on _Changed {
          if (attempt >= 2) {
            throw const _Problem(
              'This file keeps changing while it opens. Try again once it '
              'stops changing.',
            );
          }
        }
      }
    } on _Problem catch (problem) {
      if (mounted && generation == _generation) {
        setState(() => _problem = problem.text);
      }
    } on RequestFailure catch (error) {
      // Not connected, say: FileLoading reads again when it connects.
      if (mounted && generation == _generation) {
        setState(() => failure = error.message);
      }
    } on PlatformException catch (error) {
      if (mounted && generation == _generation) {
        setState(
          () => _problem = error.code == 'hash_mismatch'
              ? _changedWhileOpening
              : _cantOpen,
        );
      }
    } on MissingPluginException {
      if (mounted && generation == _generation) {
        setState(() => _problem = _cantOpen);
      }
    } finally {
      if (mounted && generation == _generation) {
        setState(() => _progress = null);
      }
    }
  }

  /// The surface's params once the file is in the render cache; null when
  /// a newer read took over or the file can't be read (then [file] says
  /// why).
  Future<InkParams?> _fetch(
    Backend backend,
    String claktory,
    int generation,
  ) async {
    bool stale() => !mounted || generation != _generation;
    void progress(int through, int size) {
      if (!stale()) {
        setState(() => _progress = (received: through, size: size));
      }
    }

    // From a Claktory on another machine, the copies this device keeps;
    // else the render cache.
    final cache = FileCache.of(backend);
    _fetched = null;
    // What it is first (its size and version), so one already in the
    // render cache isn't brought over the link again.
    final FileContent first;
    try {
      first = await backend.readFile(path, maxBytes: 1);
    } on RequestFailure {
      // Offline: the copy kept here, saying it may be out of date.
      final kept = await cache?.find(path, originalVariant);
      if (stale()) return null;
      if (cache == null || kept == null || !kept.file.existsSync()) rethrow;
      await cache.touch(kept);
      if (stale()) return null;
      _fetched = (cache: cache, file: kept.file);
      setState(() {
        file = kept.content.deepCopy();
        showingUnchecked = true;
      });
      return _paramsFor(claktory, kept.content, kept.file.path);
    }
    if (stale()) return null;
    // What the header and FileLoading compare a change with, without the
    // bytes.
    setState(() {
      file = first.deepCopy()..data = const [];
      showingUnchecked = false;
    });
    if (first.error.isNotEmpty) return null;
    final size = first.size.toInt();
    final sha = first.sha256;
    if (sha.isEmpty) {
      throw const _Problem(
        'To open PDFs here, update Claktory on the computer it runs on.',
      );
    }
    final name = path.split('/').last;
    String? cached;
    if (cache != null) {
      final kept = await cache.find(path, originalVariant);
      if (kept != null && kept.sha256 == sha && kept.file.existsSync()) {
        await cache.touch(kept);
        cached = kept.file.path;
        _fetched = (cache: cache, file: kept.file);
      } else if (kept != null) {
        await cache.drop(path);
      }
    } else {
      cached = await InkRenderCache.lookup(
        backend: claktory,
        sha256: sha,
        name: name,
      );
    }
    if (stale()) return null;
    // A big one on a phone, or on a slow link: its pages as images at
    // once, while the PDF comes behind them.
    if (cached == null &&
        size > pdfPagesFrom &&
        (touchDevice || slowFor(backend, size))) {
      final pages = _PagePreviews(
        backend: backend,
        path: path,
        sha256: sha,
        changed: () {
          if (stale()) return;
          final pages = _pages;
          setState(() {
            _noPreview = pages?.unavailable ?? false;
            final count = pages?.count;
            if (count != null) {
              _page = (page: _page?.page ?? 1, count: count);
            }
          });
        },
      );
      _pages = pages;
      unawaited(pages.want(_page?.page ?? 1));
    }
    if (cached == null) {
      final writer = cache != null
          ? null
          : await InkRenderCache.begin(
              backend: claktory,
              sha256: sha,
              name: name,
            );
      // Kept on this device: written here first, then moved in.
      final temp = cache == null
          ? null
          : File(
              '${Directory.systemTemp.path}/claktory-'
              '${sha.substring(0, sha.length.clamp(0, 16))}-$name',
            );
      final sink = temp?.openWrite();
      try {
        var offset = 0;
        while (offset < size) {
          if (stale()) throw const _Problem('');
          setState(() => _progress = (received: offset, size: size));
          final chunk = await backend.readFile(
            path,
            maxBytes: pdfChunk,
            offset: offset,
            expectedSha256: sha,
            onProgress: progress,
          );
          if (stale()) throw const _Problem('');
          if (chunk.error == 'changed' || chunk.sha256 != sha) {
            throw const _Changed();
          }
          if (chunk.error.isNotEmpty) throw _Problem(chunk.error);
          if (chunk.offset.toInt() != offset || chunk.data.isEmpty) {
            // A backend that can't read from the middle of a file.
            throw const _Problem(
              'To open PDFs here, update Claktory on the computer it runs '
              'on.',
            );
          }
          if (writer != null) {
            await writer.append(Uint8List.fromList(chunk.data));
          } else {
            sink!.add(chunk.data);
          }
          offset += chunk.data.length;
        }
        if (writer != null) {
          cached = await writer.finish();
        } else {
          await sink!.close();
          final kept = await cache!.adopt(
            path,
            originalVariant,
            first.deepCopy()..data = const [],
            temp!,
          );
          // Not kept (it can't be moved in): read from where it was
          // written, in the temporary folder the system clears.
          cached = kept?.file.path ?? temp.path;
          if (kept != null) _fetched = (cache: cache, file: kept.file);
        }
      } catch (_) {
        await writer?.cancel().catchError((Object _) {});
        if (sink != null) {
          await sink.close().catchError((Object _) {});
          await temp!.delete().catchError((Object _) => temp);
        }
        rethrow;
      }
    }
    return _paramsFor(claktory, first, cached);
  }

  /// The copy this device keeps that [_fetch] found or made: held
  /// while the surface shows it.
  ({FileCache cache, File file})? _fetched;
  ({FileCache cache, File file})? _held;

  void _hold(({FileCache cache, File file})? next) {
    if (next != null) next.cache.hold(next.file);
    final was = _held;
    _held = next;
    if (was != null) was.cache.release(was.file);
  }

  /// The surface's params for [first]'s version, its bytes at [cached].
  InkParams _paramsFor(String claktory, FileContent first, String cached) {
    final sha = first.sha256;
    return InkParams(
      leafId: 'pdf:$path',
      surface: InkSurfaceKind.pdf,
      source: InkSource(
        backend: claktory,
        path: path,
        sha256: sha,
        mime: first.mime,
        size: first.size.toInt(),
      ),
      document: InkDocument.file(file: cached, sha256: sha),
      persistence: 'online',
      // Only read: on an iPhone nothing draws (a Mac's view has no ink).
      drawingPolicy: defaultTargetPlatform == TargetPlatform.iOS
          ? InkDrawingPolicy.none
          : InkDrawingPolicy.standard,
      // A version after the first opens where the person was.
      initialPage: _page?.page,
    );
  }

  /// A new version's surface, in place of the last.
  void _show(InkParams params) {
    if (_params?.document.sha256 == params.document.sha256 &&
        _params?.document.file == params.document.file) {
      return;
    }
    _drop();
    _pages = null;
    _searchEarly = false;
    final controller = InkSurfaceController();
    _ink = _PdfInk(
      backend: AppScope.of(context),
      path: path,
      sha256: params.source.sha256 ?? '',
      controller: controller,
    );
    setState(() {
      _controller = controller;
      _events = controller.events.listen(_on);
      _params = params;
      _surfaceKey = ValueKey('pdf ${params.document.sha256}');
    });
    // The bar stays open on a new version, finding the same words.
    _find.again();
  }

  void _on(InkEvent event) {
    if (!mounted) return;
    switch (event) {
      case InkReady(:final pageCount):
        // A view made (or made again) has no ink yet.
        _ink?.ready();
        if (pageCount > 0) {
          setState(() => _page = (page: _page?.page ?? 1, count: pageCount));
        }
      case InkPageChanged(:final page1based, :final pageCount):
        setState(() => _page = (page: page1based, count: pageCount));
      case InkError(:final code) when code == 'document_too_large':
        final pages = event.pageCount;
        final most = event.maxPages;
        setState(
          () => _problem = pages == null || most == null
              ? 'This PDF is too long to open here.'
              : 'This PDF has ${_count(pages)} pages, more than the '
                    '${_count(most)} Claktory can open here.',
        );
      case InkError(:final code):
        setState(
          () => _problem = switch (code) {
            'document_hash_mismatch' ||
            'document_changed' => _changedWhileOpening,
            'document_locked' =>
              "This PDF needs a password, so Claktory can't open it here.",
            'os_too_old' => _osTooOld,
            _ => _cantOpen,
          },
        );
      case InkSelectionChanged():
        _selected = true;
      case InkLinkTapped(:final url):
        _tap.currentState?.cancel();
        final uri = Uri.tryParse(url);
        if (uri != null &&
            const {'http', 'https', 'mailto'}.contains(uri.scheme)) {
          setState(() => _link = uri);
        }
      // ⌘F, ⌘G and ⇧⌘G pressed on the page itself (AppKit has the keys
      // there).
      case InkUnknownEvent(:final method, :final arguments)
          when method == 'findRequested':
        final forward = arguments['forward'];
        if (forward is bool) {
          _session?.step(forward ? 1 : -1);
        } else {
          _session?.open();
        }
      default:
        return;
    }
  }

  // MARK: Build

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final page = _page;
    final params = _params;
    final controller = _controller;
    final shown = _problem;
    final unread = failure != null || (file?.error.isNotEmpty ?? false);
    final problem = shown != null && shown.isNotEmpty
        ? ViewerProblem(shown, path: path)
        : unread
        ? waiting()
        : null;
    final Widget body;
    if (problem != null) {
      body = problem;
    } else if ((params == null || controller == null) &&
        _pages?.count != null) {
      // A big PDF's pages as images while it comes.
      body = _PagesWhileLoading(
        pages: _pages!,
        page: page?.page ?? 1,
        onPage: (shown) => setState(
          () => _page = (page: shown, count: _pages?.count ?? shown),
        ),
        progress: _progress,
        searchEarly: _searchEarly,
      );
    } else if (params == null || controller == null) {
      final progress = _progress;
      // Coming over the link, a piece at a time; then opening it.
      body = progress != null
          ? LoadingProgress(
              label: 'Loading PDF…',
              received: progress.received,
              size: progress.size,
              connected: AppScope.of(context).connected,
              note: _noPreview
                  ? "Full size: this Claktory can't make a smaller copy."
                  : null,
            )
          : Center(
              child: Text(
                'Opening…',
                style: TextStyles.caption.copyWith(
                  color: palette.textMuted,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            );
    } else {
      body = Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (_link case final link?)
            _LinkLine(
              link: link,
              onDismiss: () => setState(() => _link = null),
              onOpen: () {
                setState(() => _link = null);
                unawaited(openPdfLink(link).then((_) {}, onError: (_) {}));
              },
            ),
          Expanded(
            // The bar floats over the page, under a link's line.
            child: FindScope(
              target: _find,
              // On a phone, a tap that's nothing else's shows or hides the
              // page's header; a double tap zooms, as PDFKit has it.
              child: ChromeTap(
                key: _tap,
                taken: (_) {
                  final selected = _selected;
                  _selected = false;
                  return selected;
                },
                child: InkSurface(
                  key: _surfaceKey!,
                  params: params,
                  controller: controller,
                ),
              ),
            ),
          ),
        ],
      );
    }
    final showing = params != null && problem == null;
    final previewing = !showing && problem == null && _pages?.count != null;
    void openFind() => _session?.open();
    // Search is there while the pages are images, and says when it works.
    void searchEarly() {
      _searchEarlyFor?.cancel();
      setState(() => _searchEarly = true);
      _searchEarlyFor = Timer(const Duration(seconds: 4), () {
        if (mounted) setState(() => _searchEarly = false);
      });
    }

    return PaneScaffold(
      share: sharePath,
      details: details(
        page == null || !(showing || previewing)
            ? ''
            : 'Page ${_count(page.page)} of ${_count(page.count)}',
      ),
      controls: [
        if (showing) HeaderTextButton('Search', openFind),
        if (previewing) HeaderTextButton('Search', searchEarly),
        reloadButton(),
      ],
      actions: [
        if (previewing) PaneAction('Search', searchEarly),
        PaneAction('Reload', reload),
      ],
      body: ColoredBox(color: palette.canvas, child: body),
    );
  }
}

/// The latest ink layer of each page of a PDF's version [sha256], by page:
/// the page's own (not a copy kept beside another after a conflict), at its
/// latest revision.
@visibleForTesting
Map<int, InkLayer> pdfInkLayers(Iterable<InkLayer> layers, String sha256) {
  final best = <int, InkLayer>{};
  for (final layer in layers) {
    final page = layer.space.page1based;
    if (layer.sourceSha256 != sha256 ||
        layer.conflictOf.isNotEmpty ||
        layer.space.kind != 'pdf_page' ||
        page < 1 ||
        layer.drawing.sha256.isEmpty) {
      continue;
    }
    final had = best[page];
    if (had == null ||
        layer.revision > had.revision ||
        (layer.revision == had.revision && layer.updatedMs > had.updatedMs)) {
      best[page] = layer;
    }
  }
  return best;
}

/// The ink saved on [path]'s pages (on an iPad), version [sha256], in the
/// native view: each page's latest layer, loaded once the view is ready
/// and again whenever one is saved or deleted, on any device. Only read:
/// nothing draws here. Nothing on a Claktory that keeps no drawings, and a
/// page whose drawing can't be read shows none.
class _PdfInk {
  _PdfInk({
    required this.backend,
    required this.path,
    required this.sha256,
    required this.controller,
  }) {
    backend.inkLayerLists.addListener(_listed);
    backend.connected.addListener(_connectedChanged);
    unawaited(_list());
  }

  final Backend backend;
  final String path;
  final String sha256;
  final InkSurfaceController controller;

  /// [path] as the backend names it, from its first answer.
  String? _canonical;
  InkLayerList? _layers;

  /// The native view is ready for drawings.
  var _ready = false;

  /// What each page shows now: its layer's id and revision.
  final _shown = <int, String>{};
  var _loading = false;
  var _again = false;
  var _retry = false;
  var _unsupported = false;
  var _disposed = false;

  void dispose() {
    _disposed = true;
    backend.inkLayerLists.removeListener(_listed);
    backend.connected.removeListener(_connectedChanged);
  }

  /// The view was made, or made again: none of the ink is on it.
  void ready() {
    _ready = true;
    _shown.clear();
    unawaited(_load());
  }

  Future<void> _list() async {
    if (_unsupported || _disposed) return;
    try {
      final list = await backend.listInkLayers(path);
      if (_disposed) return;
      _retry = false;
      _canonical = list.path;
      _layers = list;
      unawaited(_load());
    } on RequestFailure catch (failure) {
      // A Claktory before ink layers: nothing to show, ever.
      if (failure.code == ErrorCode.ERROR_CODE_UNKNOWN_MESSAGE) {
        _unsupported = true;
      } else {
        _retry = true;
      }
    } on Object {
      _retry = true;
    }
  }

  void _connectedChanged() {
    if (backend.connected.value && _retry) unawaited(_list());
  }

  /// A layer of this file was saved or deleted, on any device.
  void _listed() {
    final list = backend.inkLayerLists.value;
    if (list == null || _disposed) return;
    if (list.path != (_canonical ?? path)) return;
    _layers = list;
    unawaited(_load());
  }

  /// Brings the view to the latest layers: the pages whose layer changed,
  /// and none on a page whose layer went. One load at a time; a change
  /// meanwhile loads again after it.
  Future<void> _load() async {
    final list = _layers;
    if (!_ready || list == null || _disposed) return;
    if (_loading) {
      _again = true;
      return;
    }
    _loading = true;
    try {
      final wanted = pdfInkLayers(list.layers, sha256);
      final changes = <InkLayerDrawing>[];
      final shown = <int, String>{};
      for (final MapEntry(key: page, value: layer) in wanted.entries) {
        final id = '${layer.id}@${layer.revision}';
        if (_shown[page] == id) continue;
        final InkDrawingData drawing;
        try {
          final bytes = await InkBlobs(backend).read(layer.drawing.sha256);
          drawing = InkDrawingData.bytes(
            bytes,
            sha256: layer.drawing.sha256,
            contentVersion: layer.drawing.contentVersion,
          );
        } on Object {
          // Unreadable now: the next save tries again.
          continue;
        }
        if (_disposed) return;
        changes.add(
          InkLayerDrawing(
            page1based: page,
            inkLayerId: layer.id,
            revision: layer.revision,
            drawing: drawing,
          ),
        );
        shown[page] = id;
      }
      final gone = [
        for (final page in _shown.keys)
          if (!wanted.containsKey(page)) page,
      ];
      for (final page in gone) {
        changes.add(InkLayerDrawing.cleared(page1based: page));
      }
      if (changes.isEmpty) return;
      try {
        await controller.loadDrawing(changes);
      } on Object {
        // The view went, or can't show ink: its next ready loads it all.
        return;
      }
      if (_disposed) return;
      gone.forEach(_shown.remove);
      _shown.addAll(shown);
    } finally {
      _loading = false;
      if (_again && !_disposed) {
        _again = false;
        unawaited(_load());
      }
    }
  }
}

/// Find in a PDF: PDFKit's search in the page's surface, its matches
/// highlighted there and each gone to in turn, top to bottom.
class _PdfFind extends FindTarget {
  _PdfFind(this._surface);

  final InkSurfaceController? Function() _surface;

  InkSearchResult? _found;
  var _at = -1;
  var _query = '';
  var _searching = false;

  /// Which search an answer is for: an older one's is dropped.
  var _generation = 0;

  @override
  String get placeholder => 'Search this PDF';

  @override
  int get count => _found?.matches.length ?? 0;

  @override
  bool get capped => _found?.truncated ?? false;

  @override
  int get current => _at;

  @override
  bool get searching => _searching;

  @override
  bool get searched => _found != null;

  @override
  void search(String query) {
    _query = query;
    final generation = ++_generation;
    _found = null;
    _at = -1;
    final surface = _surface();
    if (query.isEmpty || surface == null) {
      _searching = false;
      notifyListeners();
      if (surface != null) unawaited(_quietly(surface.search('')));
      return;
    }
    _searching = true;
    notifyListeners();
    surface
        .search(query)
        .then(
          (found) {
            if (generation != _generation || found.superseded) return;
            _found = found;
            _searching = false;
            notifyListeners();
            step(1);
          },
          onError: (Object _) {
            // The page went meanwhile.
            if (generation != _generation) return;
            _searching = false;
            notifyListeners();
          },
        );
  }

  /// A new version of the file: its matches found again.
  void again() {
    if (_query.isNotEmpty) search(_query);
  }

  @override
  void step(int by) {
    final matches = _found?.matches ?? const <InkSearchMatch>[];
    if (matches.isEmpty) return;
    _at = (_at + by) % matches.length;
    notifyListeners();
    final match = matches[_at];
    unawaited(
      _quietly(
        _surface()?.goToPage(
          match.page1based,
          region: match.region,
          match: true,
        ),
      ),
    );
  }

  @override
  void clear() {
    _query = '';
    _generation++;
    _found = null;
    _at = -1;
    _searching = false;
    notifyListeners();
    unawaited(_quietly(_surface()?.search('')));
  }
}

Future<void> _quietly(Future<Object?>? command) async {
  try {
    await command;
  } catch (_) {
    // A surface that went: nothing to tell anyone.
  }
}

/// Above the page, after a tap on a link to outside the PDF: where it goes,
/// to open or not. Nothing opens by itself.
class _LinkLine extends StatelessWidget {
  const _LinkLine({
    required this.link,
    required this.onDismiss,
    required this.onOpen,
  });

  final Uri link;
  final VoidCallback onDismiss;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final where = link.scheme == 'mailto'
        ? link.path
        : (link.host.isEmpty ? link.toString() : link.host);
    return Container(
      key: const ValueKey('pdf-link'),
      padding: const EdgeInsets.symmetric(
        horizontal: Space.l,
        vertical: Space.s,
      ),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      // The words, then the buttons, wrapping under them in a narrow pane.
      child: Wrap(
        crossAxisAlignment: WrapCrossAlignment.center,
        alignment: WrapAlignment.spaceBetween,
        spacing: Space.m,
        runSpacing: Space.s,
        children: [
          Text(
            link.scheme == 'mailto' ? 'Email $where?' : 'Open $where?',
            style: TextStyles.caption.copyWith(color: palette.text),
          ),
          Wrap(
            spacing: Space.s,
            children: [
              Button(label: 'Not now', onPressed: onDismiss),
              Button(label: 'Open', primary: true, onPressed: onOpen),
            ],
          ),
        ],
      ),
    );
  }
}

const _cantOpen = "Claktory can't open this file here.";

/// An iPhone below iOS 16, where PDFKit's view can't be used this way.
const _osTooOld = 'To see PDFs here, update this iPhone to iOS 16 or later.';

const _changedWhileOpening =
    'This file changed while it was opening. Close it and open it again.';

/// 2400 as "2,400".
String _count(int n) =>
    n.toString().replaceAllMapped(RegExp(r'\B(?=(\d{3})+(?!\d))'), (_) => ',');

/// The file became another version while it came: start again.
class _Changed implements Exception {
  const _Changed();
}

class _Problem implements Exception {
  const _Problem(this.text);
  final String text;
}

/// A big PDF's pages as images made for this screen, each
/// asked for when it's first wanted. [changed] when one comes, or when
/// it's clear none will.
class _PagePreviews {
  _PagePreviews({
    required this.backend,
    required this.path,
    required this.sha256,
    required this.changed,
  });

  final Backend backend;
  final String path;
  final String sha256;
  final VoidCallback changed;

  /// How many pages it has, once a page has said.
  int? count;

  /// This Claktory can't make page images.
  var unavailable = false;

  final images = <int, Uint8List>{};

  /// Each page's size in PDF points, as it shows, for its shape.
  final sizes = <int, Size>{};
  final _asked = <int>{};

  /// Asks for [page]'s image, once.
  Future<void> want(int page) async {
    if (unavailable || page < 1 || !_asked.add(page)) return;
    if (count case final count? when page > count) return;
    try {
      final read = await backend.readFile(
        path,
        expectedSha256: sha256,
        preview: previewScreen(),
        previewPage: page,
        // No image (a Claktory that can't make them, or one from before):
        // the answer is the PDF's start instead, wanted only from the
        // PDF's own read, so none of it is read here past one piece.
        onFirst: (first) => first.isPreview,
      );
      if (read.pageCount > 0) count = read.pageCount;
      if (read.isPreview && read.data.isNotEmpty) {
        images[page] = Uint8List.fromList(read.data);
        if (read.originalPixelWidth > 0 && read.originalPixelHeight > 0) {
          sizes[page] = Size(
            read.originalPixelWidth.toDouble(),
            read.originalPixelHeight.toDouble(),
          );
        }
      } else if (read.preview == FilePreview.FILE_PREVIEW_UNAVAILABLE ||
          (read.error.isEmpty && !read.isPreview)) {
        // None from this Claktory (or one before page images): the PDF
        // alone, as before.
        unavailable = true;
      } else {
        _asked.remove(page);
      }
    } on RequestFailure {
      _asked.remove(page);
    }
    changed();
  }
}

/// The pages as images, one under another a screen at a time, while the
/// PDF comes: "Preview · Loading the full PDF… 2.1 MB of 22.4 MB" at the
/// foot, over its progress.
class _PagesWhileLoading extends StatefulWidget {
  const _PagesWhileLoading({
    required this.pages,
    required this.page,
    required this.onPage,
    required this.progress,
    required this.searchEarly,
  });

  final _PagePreviews pages;
  final int page;
  final ValueChanged<int> onPage;
  final ({int received, int size})? progress;
  final bool searchEarly;

  @override
  State<_PagesWhileLoading> createState() => _PagesWhileLoadingState();
}

class _PagesWhileLoadingState extends State<_PagesWhileLoading> {
  late final _controller = PageController(initialPage: widget.page - 1);

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final pages = widget.pages;
    final style = TypeScale.of(context).caption.copyWith(
      color: palette.textMuted,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final progress = widget.progress;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          child: PageView.builder(
            controller: _controller,
            scrollDirection: Axis.vertical,
            itemCount: pages.count ?? 1,
            onPageChanged: (index) {
              widget.onPage(index + 1);
              // This one, and the next, so it's there when it's turned to.
              unawaited(pages.want(index + 1));
              unawaited(pages.want(index + 2));
            },
            itemBuilder: (context, index) {
              final image = pages.images[index + 1];
              if (image == null) {
                return Center(
                  child: Text(
                    'Loading page ${_count(index + 1)}…',
                    style: style,
                  ),
                );
              }
              return InteractiveViewer(
                maxScale: 4,
                child: Padding(
                  padding: const EdgeInsets.all(Space.s),
                  child: Center(
                    child: Image.memory(
                      image,
                      fit: BoxFit.contain,
                      gaplessPlayback: true,
                      semanticLabel: 'Page ${_count(index + 1)}',
                    ),
                  ),
                ),
              );
            },
          ),
        ),
        Container(
          key: const ValueKey('pdf-pages-bar'),
          padding: const EdgeInsets.fromLTRB(
            Space.l,
            Space.s,
            Space.l,
            Space.s,
          ),
          decoration: BoxDecoration(
            color: palette.sidebar,
            border: Border(top: BorderSide(color: palette.hairline)),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            children: [
              Semantics(
                liveRegion: widget.searchEarly,
                child: Text(
                  widget.searchEarly
                      ? 'You can search once the full PDF has loaded.'
                      : progress == null || progress.size == 0
                      ? 'Preview · Loading the full PDF…'
                      : 'Preview · Loading the full PDF… '
                            '${formatBytes(Int64(progress.received))} of '
                            '${formatBytes(Int64(progress.size))}',
                  style: style,
                ),
              ),
              const SizedBox(height: Space.xs),
              ClipRRect(
                borderRadius: BorderRadius.circular(2),
                child: SizedBox(
                  height: 4,
                  child: Stack(
                    children: [
                      Positioned.fill(
                        child: ColoredBox(color: palette.selected),
                      ),
                      FractionallySizedBox(
                        widthFactor: progress == null || progress.size == 0
                            ? 0
                            : (progress.received / progress.size).clamp(
                                0.0,
                                1.0,
                              ),
                        child: ColoredBox(color: palette.signal),
                      ),
                    ],
                  ),
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}
