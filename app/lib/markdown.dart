import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart'
    show PointerEnterEventListener, PointerExitEventListener;
import 'package:flutter/widgets.dart';
import 'package:markdown/markdown.dart' as md;

import 'identity.dart';
import 'selection.dart' show CopyShape, Unselectable;
import 'mobile/metrics.dart';
import 'mobile/navigation.dart';
import 'loopback_forward.dart' show openLink;

/// Builds one block of rendered text: [span] is all it shows (with its
/// style), [index] its place among the blocks in reading order (from 0,
/// counted afresh each build), and [build] makes the plain widget for a
/// span. A viewer that comments on what it shows (annotations) uses it to
/// know each block's text, paint highlights and find where it is.
typedef MarkdownTextBlock = Widget Function(
  int index,
  TextSpan span,
  Widget Function(TextSpan span) build,
);

/// Renders agent markdown: paragraphs, headings, lists, quotes, tables,
/// emphasis, inline code, fenced code and links (a click opens them, with
/// [openLink]). Parsing happens in [build], so a streaming entry re-parses
/// at most once per frame however many deltas arrive.
class Markdown extends StatefulWidget {
  const Markdown(
    this.text, {
    super.key,
    this.color,
    this.textBlock,
    this.shortLinks = false,
  });

  final String text;

  /// Body text color; `text` by default.
  final Color? color;

  /// Builds each block of text instead of a plain [Text] (see
  /// [MarkdownTextBlock]); null for the plain ones.
  final MarkdownTextBlock? textBlock;

  /// A long bare address shows as its host and "/…" ("dash.cloudflare.com/…"),
  /// the whole of it in a tip while the pointer's on it. For cards' short
  /// texts only (a request's description): copied, the text has the short
  /// form, so never in conversations or viewers.
  final bool shortLinks;

  @override
  State<Markdown> createState() => _MarkdownState();
}

class _MarkdownState extends State<Markdown> {
  late final _links = Links(setState);

  /// The whole address of a shortened link, while the pointer's on it.
  final _tip = OverlayPortalController();

  /// The addresses of the links shown short, at the last build.
  final _shortened = <String>{};

  @override
  void initState() {
    super.initState();
    if (widget.shortLinks) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _tip.show();
      });
    }
  }

  @override
  void dispose() {
    _links.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final nodes = md.Document(
      extensionSet: md.ExtensionSet.gitHubFlavored,
      encodeHtml: false,
    ).parse(widget.text);
    final palette = Palette.of(context);
    final textBlock = widget.textBlock;
    final color = widget.color;
    // On a fill (a bubble's white text), links keep the text's color.
    final onFill =
        color != null &&
        color != palette.text &&
        color != palette.textMuted &&
        color != palette.textFaint;
    _links.start();
    final renderer = _Renderer(
      palette,
      color ?? palette.text,
      TypeScale.of(context),
      textBlock == null ? null : _Blocks(textBlock),
      (href) {
        final url = Uri.tryParse(href);
        if (url == null || !url.hasScheme) return null;
        if (url.scheme != 'http' && url.scheme != 'https') return null;
        return _links.next(
          href,
          () => openLink(context, url),
          onFill ? color : palette.link,
        );
      },
      widget.shortLinks ? (_shortened..clear()) : null,
    );
    final blocks = renderer.blocks(nodes);
    _links.finish();
    // Copied, its blocks are paragraphs: a blank line between them.
    final column = CopyShape(
      separator: '\n\n',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: blocks,
      ),
    );
    if (!widget.shortLinks) return column;
    return OverlayPortal(
      controller: _tip,
      overlayChildBuilder: _buildTip,
      child: column,
    );
  }

  /// The hovered short link's whole address, below the pointer (above it
  /// near the bottom), like a [HoverCard]'s card: after a moment, so it
  /// doesn't flash as the pointer passes by.
  Widget _buildTip(BuildContext context) {
    final href = _links.hoveredKey;
    final at = _links.hoveredAt;
    final overlay = Overlay.maybeOf(context)?.context.findRenderObject();
    if (href == null ||
        at == null ||
        !_shortened.contains(href) ||
        overlay is! RenderBox ||
        !overlay.hasSize) {
      return const SizedBox.shrink();
    }
    final palette = Palette.of(context);
    return IgnorePointer(
      child: CustomSingleChildLayout(
        delegate: _TipLayout(overlay.globalToLocal(at)),
        child: TweenAnimationBuilder<double>(
          key: ValueKey(href),
          tween: Tween(begin: 0, end: 1),
          duration: const Duration(milliseconds: 300) + Motion.fast,
          curve: const Interval(0.7, 1, curve: Curves.easeOut),
          builder: (context, t, child) => Opacity(opacity: t, child: child),
          child: Container(
            constraints: const BoxConstraints(maxWidth: 320),
            padding: const EdgeInsets.all(Space.m),
            decoration: BoxDecoration(
              color: palette.sidebar,
              borderRadius: BorderRadius.circular(Radii.panel),
              border: Border.all(color: palette.hairline),
              boxShadow: floatingShadow,
            ),
            child: Text(
              href,
              style: TextStyles.body.copyWith(color: palette.text),
            ),
          ),
        ),
      ),
    );
  }
}

/// A tip's place: just below [pointer], or above it where there's no room
/// below, kept inside the window.
class _TipLayout extends SingleChildLayoutDelegate {
  _TipLayout(this.pointer);

  final Offset pointer;

  @override
  BoxConstraints getConstraintsForChild(BoxConstraints constraints) =>
      constraints.loosen();

  @override
  Offset getPositionForChild(Size size, Size childSize) {
    const margin = Space.s;
    const below = 18.0;
    var top = pointer.dy + below;
    if (top + childSize.height > size.height - margin) {
      top = pointer.dy - Space.s - childSize.height;
    }
    final left = pointer.dx.clamp(
      margin,
      math.max(margin, size.width - margin - childSize.width).toDouble(),
    );
    return Offset(left, math.max(margin, top));
  }

  @override
  bool shouldRelayout(_TipLayout old) => old.pointer != pointer;
}

/// How a long bare address shows short: "dash.cloudflare.com/…" for
/// `https://dash.cloudflare.com/profile/api-tokens`; null when [text] isn't
/// the address itself, or is short enough to show whole.
String? shortAddress(String text, String href) {
  if (text.length <= 30) return null;
  final bare =
      text == href ||
      'http://$text' == href ||
      'https://$text' == href ||
      '$text/' == href;
  if (!bare) return null;
  final url = Uri.tryParse(href);
  if (url == null || url.host.isEmpty) return null;
  final host = url.host.startsWith('www.') ? url.host.substring(4) : url.host;
  final more =
      (url.path.isNotEmpty && url.path != '/') ||
      url.hasQuery ||
      url.hasFragment;
  return more ? '$host/…' : host;
}

/// How a link in rendered text looks and what its text carries: the
/// recognizer that opens it and the pointer's comings and goings.
@immutable
class LinkLook {
  const LinkLook({
    required this.recognizer,
    required this.style,
    required this.onEnter,
    required this.onExit,
  });

  final GestureRecognizer recognizer;
  final TextStyle style;
  final PointerEnterEventListener onEnter;
  final PointerExitEventListener onExit;
}

/// The web links in one piece of text, in the order they're built: in the
/// `link` color, underlined faintly, the underline full while the pointer
/// is on one, and the link dimmed while it's pressed. Each keeps its
/// recognizer and pointer callbacks across builds: hovering or pressing one
/// rebuilds the text, and a new recognizer would lose the tap, new
/// callbacks a new hover.
class Links {
  Links(this._rebuild);

  final void Function(VoidCallback change) _rebuild;
  var _disposed = false;

  /// The pointer can leave a link after its text is gone.
  void _setState(VoidCallback change) {
    if (!_disposed) _rebuild(change);
  }

  final _made = <_Link>[];
  var _next = 0;
  int? _hovered;
  Offset? _hoveredAt;
  int? _pressed;

  /// The address of the link the pointer's on, if any.
  String? get hoveredKey {
    final index = _hovered;
    return index == null || index >= _made.length ? null : _made[index].key;
  }

  /// Where the pointer came onto it, in global coordinates.
  Offset? get hoveredAt => _hovered == null ? null : _hoveredAt;

  /// Before a build.
  void start() => _next = 0;

  /// After a build: links it no longer has go.
  void finish() {
    while (_made.length > _next) {
      _made.removeLast().recognizer.dispose();
    }
  }

  void dispose() {
    _disposed = true;
    _next = 0;
    finish();
  }

  /// The next link in the text, to [key] (its address), opening with
  /// [open], in [color].
  LinkLook next(String key, VoidCallback open, Color color) {
    final index = _next++;
    if (index >= _made.length || _made[index].key != key) {
      final made = _Link(
        key,
        TapGestureRecognizer(),
        (event) {
          _setState(() {
            _hovered = index;
            _hoveredAt = event.position;
          });
        },
        (_) {
          if (_hovered == index) _setState(() => _hovered = null);
        },
      );
      if (index < _made.length) {
        _made[index].recognizer.dispose();
        _made[index] = made;
      } else {
        _made.add(made);
      }
    }
    final made = _made[index];
    made.recognizer
      ..onTapDown = ((_) => _setState(() => _pressed = index))
      ..onTapCancel = (() => _setState(() => _pressed = null))
      ..onTap = () {
        _setState(() => _pressed = null);
        open();
      };
    final pressed = _pressed == index;
    final hovered = _hovered == index;
    final shown = pressed ? color.withValues(alpha: 0.7) : color;
    return LinkLook(
      recognizer: made.recognizer,
      style: TextStyle(
        color: shown,
        decoration: TextDecoration.underline,
        decorationColor: hovered || pressed
            ? shown
            : color.withValues(alpha: 0.4),
      ),
      onEnter: made.onEnter,
      onExit: made.onExit,
    );
  }
}

class _Link {
  _Link(this.key, this.recognizer, this.onEnter, this.onExit);

  final String key;
  final TapGestureRecognizer recognizer;
  final PointerEnterEventListener onEnter;
  final PointerExitEventListener onExit;
}

/// A fenced code block: Geist Mono on a panel, scrolling sideways rather
/// than wrapping.
class CodeBlock extends StatelessWidget {
  const CodeBlock(
    this.code, {
    super.key,
    this.color,
    this.text,
    this.links = false,
  });

  final String code;
  final Color? color;

  /// Whether web addresses in [code] open when clicked, with [openLink]
  /// (a command's output, where a CLI prints the page to sign in on).
  final bool links;

  /// Shown instead of the plain [Text] of [code] (see
  /// [Markdown.textBlock]).
  final Widget? text;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final code = TypeScale.of(context).code;
    return Container(
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      // Its sideways scroll is its own: a phone's drawer doesn't take it.
      child: NoDrawerGesture(
        child: SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          padding: const EdgeInsets.symmetric(
            horizontal: Space.m,
            vertical: Space.s + 2,
          ),
          child:
              text ??
              (links && webAddresses.hasMatch(this.code)
                  ? _LinkedCode(
                      this.code,
                      style: code.copyWith(color: color ?? palette.text),
                    )
                  : Text(
                      this.code,
                      softWrap: false,
                      style: code.copyWith(color: color ?? palette.text),
                    )),
        ),
      ),
    );
  }
}

/// http and https addresses in plain text, up to the first space, quote or
/// angle bracket; [_trimAddress] drops the punctuation after one.
@visibleForTesting
final webAddresses = RegExp(r'''https?://[^\s<>"'`]+''');

/// [match] without the punctuation that ends a sentence or closes a
/// bracket it didn't open: "(see https://a.com/x)." is https://a.com/x.
String _trimAddress(String match) {
  var end = match.length;
  while (end > 0) {
    final last = match[end - 1];
    if ('.,;:!?'.contains(last)) {
      end--;
    } else if (last == ')' || last == ']' || last == '}') {
      final open = {')': '(', ']': '[', '}': '{'}[last]!;
      final text = match.substring(0, end);
      if (open.allMatches(text).length >= last.allMatches(text).length) break;
      end--;
    } else {
      break;
    }
  }
  return match.substring(0, end);
}

/// [code] with its web addresses underlined and opening when clicked;
/// recognizers only for the addresses.
class _LinkedCode extends StatefulWidget {
  const _LinkedCode(this.code, {required this.style});

  final String code;
  final TextStyle style;

  @override
  State<_LinkedCode> createState() => _LinkedCodeState();
}

class _LinkedCodeState extends State<_LinkedCode> {
  late final _links = Links(setState);

  @override
  void dispose() {
    _links.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    _links.start();
    final palette = Palette.of(context);
    final code = widget.code;
    final spans = <InlineSpan>[];
    var at = 0;
    for (final match in webAddresses.allMatches(code)) {
      final address = _trimAddress(match[0]!);
      final url = Uri.tryParse(address);
      if (url == null || url.host.isEmpty) continue;
      if (match.start > at) {
        spans.add(TextSpan(text: code.substring(at, match.start)));
      }
      final look = _links.next(
        address,
        () => openLink(context, url),
        palette.link,
      );
      spans.add(
        TextSpan(
          text: address,
          recognizer: look.recognizer,
          mouseCursor: SystemMouseCursors.click,
          onEnter: look.onEnter,
          onExit: look.onExit,
          style: look.style,
        ),
      );
      at = match.start + address.length;
    }
    if (at < code.length) spans.add(TextSpan(text: code.substring(at)));
    _links.finish();
    return Text.rich(
      TextSpan(style: widget.style, children: spans),
      softWrap: false,
    );
  }
}

/// [Markdown.textBlock] with the count of blocks built so far, shared by
/// a quote's renderer so blocks are counted in reading order.
class _Blocks {
  _Blocks(this.build);

  final MarkdownTextBlock build;
  var _next = 0;

  Widget block(TextSpan span, Widget Function(TextSpan span) plain) =>
      build(_next++, span, plain);
}

Widget _plainRich(TextSpan span) => Text.rich(span);

/// [span] and the spans in it opening [look]'s link when clicked, and
/// telling it when the pointer's on them.
InlineSpan _linked(InlineSpan span, LinkLook look) {
  if (span is! TextSpan) return span;
  return TextSpan(
    text: span.text,
    style: span.style,
    children: span.children == null
        ? null
        : [for (final child in span.children!) _linked(child, look)],
    recognizer: span.recognizer ?? look.recognizer,
    mouseCursor: SystemMouseCursors.click,
    onEnter: look.onEnter,
    onExit: look.onExit,
    semanticsLabel: span.semanticsLabel,
  );
}

class _Renderer {
  _Renderer(
    this.palette,
    this.color,
    this.type, [
    this.textBlocks,
    this.link,
    this.shortened,
  ]);

  final Palette palette;
  final Color color;

  /// Its type: 17pt reading text on a phone, the Mac's 13pt elsewhere.
  final TypeScale type;

  /// How a link to `href` looks and opens; null where links don't open.
  final LinkLook? Function(String href)? link;

  /// Null when the text is plain [Text]s (the transcript).
  final _Blocks? textBlocks;

  /// Where a long bare address shows short ([Markdown.shortLinks]), the
  /// addresses shown so; null where they show whole.
  final Set<String>? shortened;

  TextStyle get _body => type.body.copyWith(color: color);

  List<Widget> blocks(List<md.Node> nodes) {
    final widgets = <Widget>[];
    for (final node in nodes) {
      final widget = _block(node);
      if (widget == null) continue;
      if (widgets.isNotEmpty) widgets.add(const SizedBox(height: Space.m));
      widgets.add(widget);
    }
    return widgets;
  }

  Widget? _block(md.Node node) {
    if (node is md.Text) {
      final text = node.text.trim();
      if (text.isEmpty) return null;
      final blocks = textBlocks;
      return blocks == null
          ? Text(text, style: _body)
          : blocks.block(TextSpan(text: text, style: _body), _plainRich);
    }
    if (node is! md.Element) return null;
    switch (node.tag) {
      case 'p':
        return _rich(node.children, _body);
      case 'h1':
      case 'h2':
        return _rich(
          node.children,
          type.emphasis.copyWith(
            color: color,
            // A step and two over the text: 18 and 16 over the Mac's 13,
            // 22 and 20 over a phone's 17.
            fontSize: type.body.fontSize! + (node.tag == 'h1' ? 5 : 3),
          ),
        );
      case 'h3':
      case 'h4':
      case 'h5':
      case 'h6':
        return _rich(
          node.children,
          type.label.copyWith(color: color, fontWeight: FontWeight.w600),
        );
      case 'pre':
        final code = _trimTrailingNewline(node.textContent);
        final blocks = textBlocks;
        return CodeBlock(
          code,
          text: blocks?.block(
            TextSpan(
              text: code,
              style: type.code.copyWith(color: palette.text),
            ),
            (span) => Text.rich(span, softWrap: false),
          ),
        );
      case 'ul':
      case 'ol':
        return _list(node);
      case 'blockquote':
        return Container(
          padding: const EdgeInsets.only(left: Space.m),
          decoration: BoxDecoration(
            border: Border(left: BorderSide(color: palette.hairline, width: 2)),
          ),
          child: CopyShape(
            separator: '\n\n',
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: _Renderer(
                palette,
                palette.textMuted,
                type,
                textBlocks,
                link,
                shortened,
              ).blocks(node.children ?? const []),
            ),
          ),
        );
      case 'hr':
        return Container(height: 1, color: palette.hairline);
      case 'table':
        return _table(node);
      default:
        return _rich(node.children, _body);
    }
  }

  Widget _list(md.Element list) {
    final ordered = list.tag == 'ol';
    var number = int.tryParse(list.attributes['start'] ?? '') ?? 1;
    final items = <Widget>[];
    for (final item in list.children ?? const <md.Node>[]) {
      if (item is! md.Element || item.tag != 'li') continue;
      final marker = ordered ? '${number++}.' : '•';
      if (items.isNotEmpty) items.add(const SizedBox(height: Space.xs));
      items.add(
        // Copied with more than itself, an item keeps its marker, its
        // other lines under its text; the marker isn't selected on its own.
        CopyShape(
          lead: '$marker ',
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(
                width: ordered ? 22 : 14,
                child: Unselectable(
                  child: Text(
                    marker,
                    style: _body.copyWith(color: palette.textMuted),
                  ),
                ),
              ),
              Expanded(child: _listItem(item)),
            ],
          ),
        ),
      );
    }
    return CopyShape(
      separator: '\n',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: items,
      ),
    );
  }

  /// List items hold either inline content (tight lists) or blocks (loose
  /// lists and nested lists).
  Widget _listItem(md.Element item) {
    final children = item.children ?? const <md.Node>[];
    final inline = <md.Node>[];
    final parts = <Widget>[];
    void flushInline() {
      if (inline.isEmpty) return;
      parts.add(_rich(List.of(inline), _body));
      inline.clear();
    }

    for (final child in children) {
      if (child is md.Element && _isBlock(child.tag)) {
        flushInline();
        final block = _block(child);
        if (block == null) continue;
        if (parts.isNotEmpty) parts.add(const SizedBox(height: Space.xs));
        parts.add(block);
      } else {
        inline.add(child);
      }
    }
    flushInline();
    return CopyShape(
      separator: '\n',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: parts,
      ),
    );
  }

  Widget _table(md.Element table) {
    final rows = <TableRow>[];
    void addRows(md.Element section, {required bool header}) {
      for (final row in section.children ?? const <md.Node>[]) {
        if (row is! md.Element || row.tag != 'tr') continue;
        final cells = <Widget>[];
        for (final cell in row.children ?? const <md.Node>[]) {
          if (cell is! md.Element) continue;
          cells.add(
            Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: Space.s,
                vertical: Space.xs + 2,
              ),
              child: _rich(
                cell.children,
                header ? type.label.copyWith(color: color) : _body,
              ),
            ),
          );
        }
        if (rows.isNotEmpty && cells.length != rows.first.children.length) {
          continue;
        }
        rows.add(
          TableRow(
            decoration: BoxDecoration(
              border: Border(bottom: BorderSide(color: palette.hairline)),
            ),
            children: cells,
          ),
        );
      }
    }

    for (final section in table.children ?? const <md.Node>[]) {
      if (section is! md.Element) continue;
      addRows(section, header: section.tag == 'thead');
    }
    if (rows.isEmpty) return const SizedBox.shrink();
    return NoDrawerGesture(
      child: SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        // Copied, a row's cells are tab-separated, rows on lines of their
        // own, as a spreadsheet takes them.
        child: CopyShape(
          cells: true,
          child: Table(
            defaultColumnWidth: const IntrinsicColumnWidth(),
            children: rows,
          ),
        ),
      ),
    );
  }

  Widget _rich(List<md.Node>? nodes, TextStyle style) {
    final spans = <InlineSpan>[];
    _inline(nodes ?? const [], style, spans);
    final span = TextSpan(style: style, children: spans);
    final blocks = textBlocks;
    return blocks == null ? Text.rich(span) : blocks.block(span, _plainRich);
  }

  void _inline(List<md.Node> nodes, TextStyle style, List<InlineSpan> out) {
    for (final node in nodes) {
      if (node is md.Text) {
        out.add(TextSpan(text: node.text));
        continue;
      }
      if (node is! md.Element) continue;
      final children = node.children ?? const <md.Node>[];
      final TextStyle? inner = switch (node.tag) {
        'strong' => const TextStyle(fontWeight: FontWeight.w600),
        'em' => const TextStyle(fontStyle: FontStyle.italic),
        'del' => const TextStyle(decoration: TextDecoration.lineThrough),
        'a' => TextStyle(
          decoration: TextDecoration.underline,
          decorationColor: palette.textFaint,
        ),
        _ => null,
      };
      switch (node.tag) {
        case 'code':
          out.add(
            TextSpan(
              text: node.textContent,
              style: TextStyle(
                fontFamily: TextStyles.mono,
                fontSize: (style.fontSize ?? 13) - 1,
                backgroundColor: palette.selected,
              ),
            ),
          );
        case 'br':
          out.add(const TextSpan(text: '\n'));
        case 'img':
          out.add(TextSpan(text: node.attributes['alt'] ?? ''));
        case 'input':
          final checked = node.attributes['checked'] != null;
          out.add(TextSpan(text: checked ? '☑ ' : '☐ '));
        case 'a':
          var nested = <InlineSpan>[];
          _inline(children, style, nested);
          final href = node.attributes['href'] ?? '';
          final look = link?.call(href);
          final short = look == null || shortened == null
              ? null
              : shortAddress(node.textContent, href);
          if (short != null) {
            nested = [TextSpan(text: short)];
            shortened!.add(href);
          }
          out.add(
            TextSpan(
              style: look?.style ?? inner,
              // A click lands on the text's own span, so each piece of the
              // link's text carries the recognizer.
              children: look == null
                  ? nested
                  : [for (final span in nested) _linked(span, look)],
              recognizer: look?.recognizer,
              mouseCursor: look == null
                  ? MouseCursor.defer
                  : SystemMouseCursors.click,
              onEnter: look?.onEnter,
              onExit: look?.onExit,
            ),
          );
        default:
          final nested = <InlineSpan>[];
          _inline(children, style, nested);
          out.add(TextSpan(style: inner, children: nested));
      }
    }
  }

  static bool _isBlock(String tag) => const {
    'p',
    'ul',
    'ol',
    'pre',
    'blockquote',
    'h1',
    'h2',
    'h3',
    'h4',
    'h5',
    'h6',
    'hr',
    'table',
  }.contains(tag);
}

String _trimTrailingNewline(String text) =>
    text.endsWith('\n') ? text.substring(0, text.length - 1) : text;
