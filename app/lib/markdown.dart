import 'package:flutter/widgets.dart';
import 'package:markdown/markdown.dart' as md;

import 'identity.dart';

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
/// emphasis, inline code and fenced code. Parsing happens in [build], so a
/// streaming entry re-parses at most once per frame however many deltas
/// arrive.
class Markdown extends StatelessWidget {
  const Markdown(this.text, {super.key, this.color, this.textBlock});

  final String text;

  /// Body text color; `text` by default.
  final Color? color;

  /// Builds each block of text instead of a plain [Text] (see
  /// [MarkdownTextBlock]); null for the plain ones.
  final MarkdownTextBlock? textBlock;

  @override
  Widget build(BuildContext context) {
    final nodes = md.Document(
      extensionSet: md.ExtensionSet.gitHubFlavored,
      encodeHtml: false,
    ).parse(text);
    final palette = Palette.of(context);
    final textBlock = this.textBlock;
    final renderer = _Renderer(
      palette,
      color ?? palette.text,
      textBlock == null ? null : _Blocks(textBlock),
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
  const CodeBlock(this.code, {super.key, this.color, this.text});

  final String code;
  final Color? color;

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
            Text(
              code,
              softWrap: false,
              style: TextStyles.code.copyWith(color: color ?? palette.text),
            ),
      ),
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

class _Renderer {
  _Renderer(this.palette, this.color, [this.textBlocks]);

  final Palette palette;
  final Color color;

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
