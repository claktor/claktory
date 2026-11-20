import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:markdown/markdown.dart' as md;

import 'identity.dart';
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
  const Markdown(this.text, {super.key, this.color, this.textBlock});

  final String text;

  /// Body text color; `text` by default.
  final Color? color;

  /// Builds each block of text instead of a plain [Text] (see
  /// [MarkdownTextBlock]); null for the plain ones.
  final MarkdownTextBlock? textBlock;

  @override
  State<Markdown> createState() => _MarkdownState();
}

class _MarkdownState extends State<Markdown> {
  /// The links' recognizers from the last build; made only for text that
  /// has links.
  final _recognizers = <TapGestureRecognizer>[];

  void _dropRecognizers() {
    for (final recognizer in _recognizers) {
      recognizer.dispose();
    }
    _recognizers.clear();
  }

  @override
  void dispose() {
    _dropRecognizers();
    super.dispose();
  }

  GestureRecognizer? _link(String href) {
    final url = Uri.tryParse(href);
    if (url == null || !url.hasScheme) return null;
    if (url.scheme != 'http' && url.scheme != 'https') return null;
    final recognizer = TapGestureRecognizer()
      ..onTap = () => openLink(context, url);
    _recognizers.add(recognizer);
    return recognizer;
  }

  @override
  Widget build(BuildContext context) {
    _dropRecognizers();
    final nodes = md.Document(
      extensionSet: md.ExtensionSet.gitHubFlavored,
      encodeHtml: false,
    ).parse(widget.text);
    final palette = Palette.of(context);
    final textBlock = widget.textBlock;
    final renderer = _Renderer(
      palette,
      widget.color ?? palette.text,
      textBlock == null ? null : _Blocks(textBlock),
      _link,
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: renderer.blocks(nodes),
    );
  }
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
    return Container(
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      child: SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.symmetric(
          horizontal: Space.m,
          vertical: Space.s + 2,
        ),
        child:
            text ??
            (links && webAddresses.hasMatch(code)
                ? _LinkedCode(
                    code,
                    style: TextStyles.code.copyWith(
                      color: color ?? palette.text,
                    ),
                  )
                : Text(
                    code,
                    softWrap: false,
                    style: TextStyles.code.copyWith(
                      color: color ?? palette.text,
                    ),
                  )),
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
  final _recognizers = <TapGestureRecognizer>[];

  void _dropRecognizers() {
    for (final recognizer in _recognizers) {
      recognizer.dispose();
    }
    _recognizers.clear();
  }

  @override
  void dispose() {
    _dropRecognizers();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    _dropRecognizers();
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
      final recognizer = TapGestureRecognizer()
        ..onTap = () => openLink(context, url);
      _recognizers.add(recognizer);
      spans.add(
        TextSpan(
          text: address,
          recognizer: recognizer,
          mouseCursor: SystemMouseCursors.click,
          style: TextStyle(
            decoration: TextDecoration.underline,
            decorationColor: Palette.of(context).textFaint,
          ),
        ),
      );
      at = match.start + address.length;
    }
    if (at < code.length) spans.add(TextSpan(text: code.substring(at)));
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

/// [span] and the spans in it opening [recognizer]'s link when clicked.
InlineSpan _linked(InlineSpan span, GestureRecognizer recognizer) {
  if (span is! TextSpan) return span;
  return TextSpan(
    text: span.text,
    style: span.style,
    children: span.children == null
        ? null
        : [for (final child in span.children!) _linked(child, recognizer)],
    recognizer: span.recognizer ?? recognizer,
    mouseCursor: SystemMouseCursors.click,
    semanticsLabel: span.semanticsLabel,
  );
}

class _Renderer {
  _Renderer(this.palette, this.color, [this.textBlocks, this.link]);

  final Palette palette;
  final Color color;

  /// What a click on a link to `href` does; null where links don't open.
  final GestureRecognizer? Function(String href)? link;

  /// Null when the text is plain [Text]s (the transcript).
  final _Blocks? textBlocks;

  TextStyle get _body => TextStyles.body.copyWith(color: color);

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
          TextStyles.emphasis.copyWith(
            color: color,
            fontSize: node.tag == 'h1' ? 18 : 16,
          ),
        );
      case 'h3':
      case 'h4':
      case 'h5':
      case 'h6':
        return _rich(
          node.children,
          TextStyles.label.copyWith(color: color, fontWeight: FontWeight.w600),
        );
      case 'pre':
        final code = _trimTrailingNewline(node.textContent);
        final blocks = textBlocks;
        return CodeBlock(
          code,
          text: blocks?.block(
            TextSpan(
              text: code,
              style: TextStyles.code.copyWith(color: palette.text),
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
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: _Renderer(
              palette,
              palette.textMuted,
              textBlocks,
              link,
            ).blocks(node.children ?? const []),
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
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SizedBox(
              width: ordered ? 22 : 14,
              child: Text(
                marker,
                style: _body.copyWith(color: palette.textMuted),
              ),
            ),
            Expanded(child: _listItem(item)),
          ],
        ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: items,
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
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: parts,
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
                header ? TextStyles.label.copyWith(color: color) : _body,
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
    return SingleChildScrollView(
      scrollDirection: Axis.horizontal,
      child: Table(
        defaultColumnWidth: const IntrinsicColumnWidth(),
        children: rows,
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
          final nested = <InlineSpan>[];
          _inline(children, style, nested);
          final recognizer = link?.call(node.attributes['href'] ?? '');
          out.add(
            TextSpan(
              style: inner,
              // A click lands on the text's own span, so each piece of the
              // link's text carries the recognizer.
              children: recognizer == null
                  ? nested
                  : [for (final span in nested) _linked(span, recognizer)],
              recognizer: recognizer,
              mouseCursor: recognizer == null
                  ? MouseCursor.defer
                  : SystemMouseCursors.click,
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
