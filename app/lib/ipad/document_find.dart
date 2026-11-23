// A document's side panel on an iPad (docs/ipad-workbench.md §6): search
// the PDF, or go through its own contents. In a wide pane it sits at the
// page's start and the page narrows beside it; in a narrow pane it covers
// the page, and going to a place closes it. Native finds the matches and highlights them on the pages
// (InkSurfaceController.search); the panel lists them, each with its page
// and the sentence it's in, and tapping one shows it.
import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../mobile/metrics.dart' show Metrics, TypeScale;
import 'document_host.dart' show DocumentPanel, DocumentSurface;
import 'ink_bridge.dart';

/// The panel's width beside the page; a pane narrower than
/// [findPanelCovers] gives it all its width.
const findPanelWidth = 320.0;
const findPanelCovers = 560.0;

/// What native lists at most: more is said, not shown.
const findMostMatches = 200;

/// The keyboard's floating shortcut bar (a hardware keyboard's), over the
/// window's foot without taking room: the lists end this far below their
/// last row.
const _keyboardBar = 64.0;

class DocumentFindPanel extends StatefulWidget {
  const DocumentFindPanel({
    super.key,
    required this.surface,
    required this.covers,
  });

  final DocumentSurface surface;

  /// It covers the page: going somewhere closes it, to show the place.
  final bool covers;

  @override
  State<DocumentFindPanel> createState() => _DocumentFindPanelState();
}

class _DocumentFindPanelState extends State<DocumentFindPanel> {
  late final _field = TextEditingController(text: widget.surface.query)
    ..addListener(_typed);

  DocumentSurface get surface => widget.surface;

  /// The match or entry last gone to, tinted so the list says where the
  /// page is after jumping around; a new search starts with none.
  int? _current;

  void _typed() {
    if (_field.text != surface.query) _current = null;
    surface.search(_field.text);
  }

  @override
  void dispose() {
    _field.dispose();
    super.dispose();
  }

  /// Shows [page] at [region]: a search's match marked apart from the
  /// others, high in the view. A panel covering the page closes to show
  /// it, keeping the search's marks: Search, lit, opens on it again.
  void _go(int index, int page, {Rect? region}) {
    final searching = surface.panel == DocumentPanel.search;
    setState(() => _current = index);
    surface.goTo(page, region: region, match: searching);
    if (widget.covers) surface.closePanel(keepSearch: searching);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final panel = surface.panel;
    final searching = panel == DocumentPanel.search;
    final close = GlyphButton(
      key: const ValueKey('find-close'),
      label: 'Close',
      active: false,
      onTap: surface.closePanel,
      glyph: (color) => CloseGlyph(color: color),
    );
    final head = Padding(
      padding: const EdgeInsetsDirectional.fromSTEB(
        Space.m,
        Space.s,
        Space.xs,
        Space.s,
      ),
      child: Row(
        children: [
          Expanded(
            child: searching
                ? TextField(
                    controller: _field,
                    autofocus: true,
                    placeholder: 'Search this PDF',
                    textInputAction: TextInputAction.search,
                    onSubmitted: (_) {
                      final first = surface.found?.matches.firstOrNull;
                      if (first != null) {
                        _go(0, first.page1based, region: first.region);
                      }
                    },
                  )
                : Semantics(
                    header: true,
                    child: Text(
                      'Contents',
                      style: type.label.copyWith(color: palette.text),
                    ),
                  ),
          ),
          const SizedBox(width: Space.xs),
          close,
        ],
      ),
    );
    final Widget body;
    if (searching) {
      body = _results(context);
    } else {
      final outline = surface.outline;
      body = ListView.builder(
        key: const ValueKey('find-contents'),
        itemCount: outline.length,
        // Room to scroll the last one above the keyboard's floating
        // shortcut bar, which takes no room of its own.
        padding: const EdgeInsets.only(bottom: _keyboardBar),
        itemBuilder: (context, i) => _ContentsRow(
          entry: outline[i],
          selected: _current == i,
          onTap: () => _go(i, outline[i].page1based),
        ),
      );
    }
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): surface.closePanel,
      },
      child: FocusScope(
        child: Semantics(
          container: true,
          explicitChildNodes: true,
          child: Container(
            key: const ValueKey('find-panel'),
            decoration: BoxDecoration(
              color: palette.canvas,
              border: BorderDirectional(
                end: BorderSide(color: palette.hairline),
              ),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                head,
                Container(height: 1, color: palette.hairline),
                Expanded(child: body),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _results(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final query = surface.query.trim();
    if (query.isEmpty) return const SizedBox.shrink();
    final found = surface.found;
    final String count;
    if (found == null) {
      count = 'Searching…';
    } else if (found.matches.isEmpty) {
      count = 'No matches';
    } else if (found.truncated) {
      count =
          'Over $findMostMatches matches · showing the first '
          '${found.matches.length}';
    } else {
      count = found.matches.length == 1
          ? '1 match'
          : '${found.matches.length} matches';
    }
    final matches = found?.matches ?? const <InkSearchMatch>[];
    return ListView.builder(
      key: const ValueKey('find-results'),
      padding: const EdgeInsets.only(bottom: _keyboardBar),
      itemCount: matches.length + 1,
      itemBuilder: (context, i) {
        if (i == 0) {
          return Padding(
            padding: const EdgeInsets.fromLTRB(
              Space.m,
              Space.s,
              Space.m,
              Space.xs,
            ),
            child: Semantics(
              liveRegion: true,
              child: Text(
                count,
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            ),
          );
        }
        final match = matches[i - 1];
        // Which of its excerpt's matches this one is: the matches before
        // it on its page with the same excerpt (a line with the word
        // twice, or a page PDFKit reads as one block).
        var occurrence = 0;
        for (final before in matches.take(i - 1)) {
          if (before.page1based == match.page1based &&
              before.text == match.text) {
            occurrence++;
          }
        }
        return _ResultRow(
          match: match,
          query: query,
          occurrence: occurrence,
          selected: _current == i - 1,
          onTap: () => _go(i - 1, match.page1based, region: match.region),
        );
      },
    );
  }
}

/// A row to tap: a finger's height, its hover shown.
class _Row extends StatelessWidget {
  const _Row({
    required this.onTap,
    required this.child,
    this.indent = 0,
    this.selected = false,
  });

  final VoidCallback onTap;
  final Widget child;
  final double indent;

  /// The one last gone to.
  final bool selected;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      selected: selected,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          constraints: BoxConstraints(minHeight: Metrics.of(context).target),
          color: selected
              ? palette.selected
              : hovered
              ? palette.hover
              : null,
          padding: EdgeInsetsDirectional.fromSTEB(
            Space.m + indent,
            Space.xs,
            Space.m,
            Space.xs,
          ),
          alignment: AlignmentDirectional.centerStart,
          child: child,
        ),
      ),
    );
  }
}

class _ResultRow extends StatelessWidget {
  const _ResultRow({
    required this.match,
    required this.query,
    required this.occurrence,
    required this.onTap,
    required this.selected,
  });

  final InkSearchMatch match;
  final String query;
  final int occurrence;
  final VoidCallback onTap;
  final bool selected;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final style = type.secondary.copyWith(color: palette.text);
    final text = excerptAround(match.text, query, occurrence);
    return _Row(
      onTap: onTap,
      selected: selected,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            'Page ${match.page1based}',
            style: type.caption.copyWith(color: palette.textMuted),
          ),
          Text.rich(
            TextSpan(children: excerptSpans(text, query, style)),
            maxLines: 3,
            overflow: TextOverflow.ellipsis,
            style: style,
          ),
        ],
      ),
    );
  }
}

/// What a result row shows of [text], the match's line as PDFKit gives
/// it: its spaces and line breaks run together, and, when it's long (a
/// page PDFKit reads as one block gives the whole page), cut to the
/// words around its [occurrence]th [query] (0 for the first), with "…"
/// where it's cut, so the match is in the row's three lines. Where the
/// query isn't found as typed (a match ignoring accents), its start.
String excerptAround(
  String text,
  String query,
  int occurrence, {
  int before = 40,
  int after = 80,
}) {
  final flat = text.replaceAll(RegExp(r'\s+'), ' ').trim();
  final find = query.replaceAll(RegExp(r'\s+'), ' ').trim().toLowerCase();
  final lower = flat.toLowerCase();
  var hit = find.isEmpty ? -1 : lower.indexOf(find);
  for (var n = 0; n < occurrence && hit >= 0; n++) {
    final next = lower.indexOf(find, hit + find.length);
    if (next < 0) break;
    hit = next;
  }
  if (hit < 0) hit = 0;
  var start = math.max(0, hit - before);
  var end = math.min(flat.length, hit + find.length + after);
  // Whole words: back to the space before the cut, forward from it.
  if (start > 0) {
    final space = flat.indexOf(' ', start);
    if (space >= 0 && space < hit) start = space + 1;
  }
  if (end < flat.length) {
    final space = flat.lastIndexOf(' ', end);
    if (space > hit + find.length) end = space;
  }
  return '${start > 0 ? '…' : ''}${flat.substring(start, end)}'
      '${end < flat.length ? '…' : ''}';
}

/// [text] with each [query] in it bold, ignoring case.
List<TextSpan> excerptSpans(String text, String query, TextStyle style) {
  final spans = <TextSpan>[];
  final lower = text.toLowerCase();
  final find = query.toLowerCase();
  var at = 0;
  while (find.isNotEmpty) {
    final hit = lower.indexOf(find, at);
    if (hit < 0) break;
    if (hit > at) spans.add(TextSpan(text: text.substring(at, hit)));
    spans.add(
      TextSpan(
        text: text.substring(hit, hit + find.length),
        style: style.copyWith(fontWeight: FontWeight.w600),
      ),
    );
    at = hit + find.length;
  }
  if (at < text.length) spans.add(TextSpan(text: text.substring(at)));
  return spans;
}

class _ContentsRow extends StatelessWidget {
  const _ContentsRow({
    required this.entry,
    required this.onTap,
    required this.selected,
  });

  final InkOutlineEntry entry;
  final VoidCallback onTap;
  final bool selected;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return _Row(
      onTap: onTap,
      selected: selected,
      indent: Space.m * entry.depth.clamp(0, 4),
      child: Row(
        children: [
          Expanded(
            child: Text(
              entry.title,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: (entry.depth == 0 ? type.body : type.secondary).copyWith(
                color: palette.text,
              ),
            ),
          ),
          const SizedBox(width: Space.s),
          Text(
            '${entry.page1based}',
            style: type.caption.copyWith(color: palette.textMuted),
          ),
        ],
      ),
    );
  }
}
