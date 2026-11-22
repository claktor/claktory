// The Mac's PDF viewer: the file brought from the machine Claktory runs on
// into the device's render cache, a range at a time from one version, then
// shown by PDFKit's own view (app/macos/Runner/PaperViewer.swift, through
// ipad/ink_surface.dart's surface, without ink). The trackpad scrolls and
// zooms its pages, their text selects and copies as anywhere on the Mac,
// and its links never open by themselves: the pane asks first, as on the
// iPad. Search (or ⌘F) puts a field in the header; Return and ⇧Return go
// from match to match, Escape closes it.
//
// An iPhone shows it the same way, as a page of its own, through PDFKit's
// iOS view with nothing drawing (ink_surface.dart's surface, its drawing
// policy none): a finger scrolls, zooms and selects. Search is in More,
// and opens a find bar at the page's foot (mobile/find_bar.dart).
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'package:url_launcher/url_launcher.dart' show launchUrl;

import '../backend.dart';
import '../controls.dart';
import '../identity.dart';
import '../ipad/ink_bridge.dart';
import '../ipad/ink_surface.dart';
import '../pane_chrome.dart';
import '../mobile/find_bar.dart';
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

  // Find.
  var _finding = false;
  final _query = TextEditingController();
  final _field = FocusNode(debugLabel: 'Search this PDF');
  final _pane = FocusNode(debugLabel: 'PDF');
  Timer? _typing;
  String _searched = '';
  InkSearchResult? _found;
  var _at = -1;

  @override
  void initState() {
    _query.addListener(_queryChanged);
    super.initState();
  }

  @override
  void dispose() {
    _typing?.cancel();
    _generation++;
    _drop();
    _query.dispose();
    _field.dispose();
    _pane.dispose();
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
      _found = null;
      _searched = '';
      _at = -1;
    });
    // The field stays open on a new version, finding the same words.
    if (_finding && _query.text.trim().isNotEmpty) unawaited(_search());
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
          _step(forward ? 1 : -1);
        } else {
          _openFind();
        }
      default:
        return;
    }
  }

  // MARK: Find

  void _openFind() {
    if (_params == null) return;
    setState(() => _finding = true);
    _query.selection = TextSelection(
      baseOffset: 0,
      extentOffset: _query.text.length,
    );
    // The field is built this frame.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _finding) _field.requestFocus();
    });
  }

  void _closeFind() {
    if (!_finding) return;
    _typing?.cancel();
    setState(() {
      _finding = false;
      _found = null;
      _searched = '';
      _at = -1;
    });
    unawaited(_quietly(_controller?.search('')));
    _pane.requestFocus();
  }

  void _typed(String _) {
    _typing?.cancel();
    _typing = Timer(const Duration(milliseconds: 250), () => _search());
  }

  /// Finds the field's words, highlighting them, and shows the first.
  Future<void> _search() async {
    final controller = _controller;
    if (controller == null) return;
    final query = _query.text.trim();
    _typing?.cancel();
    _searched = query;
    if (query.isEmpty) {
      setState(() {
        _found = null;
        _at = -1;
      });
      await _quietly(controller.search(''));
      return;
    }
    try {
      final found = await controller.search(query);
      if (!mounted || found.superseded || query != _searched) return;
      setState(() {
        _found = found;
        _at = -1;
      });
      _step(1);
    } catch (_) {
      // The page went meanwhile.
    }
  }

  /// The next match ([by] 1) or the one before (-1), round the ends.
  void _step(int by) {
    if (!_finding) return _openFind();
    final found = _found;
    if (found == null || _query.text.trim() != _searched) {
      unawaited(_search());
      return;
    }
    final matches = found.matches;
    if (matches.isEmpty) return;
    final at = (_at + by) % matches.length;
    setState(() => _at = at);
    final match = matches[at];
    unawaited(
      _quietly(_controller?.goToPage(match.page1based, region: match.region)),
    );
  }

  String _findCount() {
    final found = _found;
    if (found == null || _query.text.trim().isEmpty) return '';
    final n = found.matches.length;
    if (n == 0) return 'No matches';
    final more = found.truncated ? '+' : '';
    return _at < 0 ? '$n$more matches' : '${_at + 1} of $n$more';
  }

  Future<void> _quietly(Future<Object?>? command) async {
    try {
      await command;
    } catch (_) {
      // A surface that went: nothing to tell anyone.
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
            child: InkSurface(
              key: _surfaceKey!,
              params: params,
              controller: controller,
            ),
          ),
        ],
      );
    }
    final showing = params != null && problem == null;
    final phone = PaneChrome.of(context)?.phone != null;
    final count = _findCount();
    final controls = <Widget>[
      if (showing && _finding) ...[
        if (count.isNotEmpty)
          Text(
            count,
            key: const ValueKey('pdf-find-count'),
            style: TextStyles.caption.copyWith(
              color: palette.textMuted,
              fontFeatures: const [FontFeature.tabularFigures()],
            ),
          ),
        SizedBox(
          width: 180,
          child: CallbackShortcuts(
            bindings: {
              const SingleActivator(LogicalKeyboardKey.enter): () => _step(1),
              const SingleActivator(
                LogicalKeyboardKey.enter,
                shift: true,
              ): () =>
                  _step(-1),
              const SingleActivator(LogicalKeyboardKey.escape): _closeFind,
            },
            child: TextField(
              key: const ValueKey('pdf-find'),
              controller: _query,
              focusNode: _field,
              placeholder: 'Search this PDF',
              semanticsLabel: 'Search this PDF',
              onSubmitted: (_) => _step(1),
            ),
          ),
        ),
        HeaderTextButton('Done', _closeFind),
      ] else if (showing)
        HeaderTextButton('Search', _openFind),
      reloadButton(),
    ];
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.keyF, meta: true): _openFind,
        const SingleActivator(LogicalKeyboardKey.keyG, meta: true): () =>
            _step(1),
        const SingleActivator(
          LogicalKeyboardKey.keyG,
          meta: true,
          shift: true,
        ): () =>
            _step(-1),
        const SingleActivator(LogicalKeyboardKey.escape): _closeFind,
      },
      child: Focus(
        focusNode: _pane,
        // A tap in the pane (its header, say) brings the keys here, for
        // ⌘F; a tap on the find field keeps them there. The page's own
        // keys are AppKit's, which sends ⌘F on (findRequested).
        child: Listener(
          behavior: HitTestBehavior.translucent,
          onPointerDown: (_) => scheduleMicrotask(() {
            if (mounted && !_pane.hasFocus) _pane.requestFocus();
          }),
          child: PaneScaffold(
            details: details(
              page == null || !showing
                  ? ''
                  : 'Page ${_count(page.page)} of ${_count(page.count)}',
            ),
            controls: controls,
            // On a phone, at the page's foot, above the keyboard.
            toolbar: showing && _finding && phone
                ? PhoneFindBar(
                    controller: _query,
                    focusNode: _field,
                    placeholder: 'Search this PDF',
                    count: count,
                    onStep: _step,
                    onDone: _closeFind,
                  )
                : null,
            actions: [
              if (showing) PaneAction('Search', _openFind, active: _finding),
              PaneAction('Reload', reload),
            ],
            body: ColoredBox(color: palette.canvas, child: body),
          ),
        ),
      ),
    );
  }

  /// What's typed: found as it's typed, a moment after the last key.
  String _lastTyped = '';
  void _queryChanged() {
    if (_query.text == _lastTyped) return;
    _lastTyped = _query.text;
    _typed(_query.text);
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
