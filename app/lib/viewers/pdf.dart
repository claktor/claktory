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
import 'dart:async';

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
import '../pane_chrome.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import 'registry.dart';

/// How much of a PDF one read brings (the backend's cap).
@visibleForTesting
var pdfChunk = 8 * 1024 * 1024;

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

  /// The page's surface: a new one for each version of the file, since
  /// native opens its document once.
  InkSurfaceController? _controller;
  StreamSubscription<InkEvent>? _events;
  InkParams? _params;
  Key? _surfaceKey;

  String? _problem;
  ({int received, int size})? _progress;
  ({int page, int count})? _page;
  Uri? _link;

  /// Brings the file again whenever asked to reload.
  var _generation = 0;

  /// Find's engine: PDFKit's search in the page's surface.
  late final _find = _PdfFind(() => _controller);

  /// This pane's find, whose bar the app shows (find.dart); null before
  /// its scope is built.
  FindSession? get _session => FindSessions.instance.within(context);

  @override
  void dispose() {
    _find.dispose();
    _generation++;
    _drop();
    super.dispose();
  }

  void _drop() {
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
    });
    try {
      // A file that changes as it comes is read again from its start, with
      // its new version, a few times before giving up.
      for (var attempt = 0; ; attempt++) {
        try {
          final params = await _fetch(backend, claktory, generation);
          if (params == null || !mounted || generation != _generation) return;
          _show(params);
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
    final first = await backend.readFile(path, maxBytes: pdfChunk);
    if (stale()) return null;
    // What the header and FileLoading compare a change with, without the
    // bytes.
    setState(() => file = first.deepCopy()..data = const []);
    if (first.error.isNotEmpty) return null;
    final size = first.size.toInt();
    final sha = first.sha256;
    if (sha.isEmpty) {
      throw const _Problem(
        'To open PDFs here, update Claktory on the computer it runs on.',
      );
    }
    final name = path.split('/').last;
    var cached = await InkRenderCache.lookup(
      backend: claktory,
      sha256: sha,
      name: name,
    );
    if (stale()) return null;
    if (cached == null) {
      final writer = await InkRenderCache.begin(
        backend: claktory,
        sha256: sha,
        name: name,
      );
      try {
        await writer.append(Uint8List.fromList(first.data));
        var offset = first.data.length;
        while (offset < size) {
          if (stale()) throw const _Problem('');
          setState(() => _progress = (received: offset, size: size));
          final chunk = await backend.readFile(
            path,
            maxBytes: pdfChunk,
            offset: offset,
            expectedSha256: sha,
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
          await writer.append(Uint8List.fromList(chunk.data));
          offset += chunk.data.length;
        }
        cached = await writer.finish();
      } catch (_) {
        await writer.cancel().catchError((Object _) {});
        rethrow;
      }
    }
    return InkParams(
      leafId: 'pdf:$path',
      surface: InkSurfaceKind.pdf,
      source: InkSource(
        backend: claktory,
        path: path,
        sha256: sha,
        mime: first.mime,
        size: size,
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
    final controller = InkSurfaceController();
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
      case InkLinkTapped(:final url):
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
    } else if (params == null || controller == null) {
      final progress = _progress;
      body = Center(
        child: Text(
          progress == null || progress.size == 0
              ? 'Opening…'
              : 'Opening… ${(100 * progress.received / progress.size).floor()}%',
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
              child: InkSurface(
                key: _surfaceKey!,
                params: params,
                controller: controller,
              ),
            ),
          ),
        ],
      );
    }
    final showing = params != null && problem == null;
    void openFind() => _session?.open();
    return PaneScaffold(
      details: details(
        page == null || !showing
            ? ''
            : 'Page ${_count(page.page)} of ${_count(page.count)}',
      ),
      controls: [
        if (showing) HeaderTextButton('Search', openFind),
        reloadButton(),
      ],
      actions: [PaneAction('Reload', reload)],
      body: ColoredBox(color: palette.canvas, child: body),
    );
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
