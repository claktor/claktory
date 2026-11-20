import 'dart:convert';

import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../markdown.dart';
import '../pane_chrome.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../resources.dart' show formatBytes;
import '../selection.dart';
import 'registry.dart';

/// A text file: monospace with line numbers, or Markdown formatted with a
/// toggle to its plain source. Selectable, with the Copy menu.
class TextViewer extends StatefulWidget {
  const TextViewer({super.key, required this.path});

  final String path;

  @override
  State<TextViewer> createState() => _TextViewerState();
}

class _TextViewerState extends State<TextViewer> with FileLoading<TextViewer> {
  @override
  String get path => widget.path;

  var _raw = false;
  String _text = '';
  bool _binary = false;

  bool get _markdown {
    final mime = file?.mime ?? '';
    return mime == 'text/markdown' ||
        path.toLowerCase().endsWith('.md') ||
        path.toLowerCase().endsWith('.markdown');
  }

  @override
  void loaded(FileContent file) {
    final mime = file.mime;
    _binary =
        !(mime.startsWith('text/') ||
            mime == 'application/json' ||
            mime == 'image/svg+xml' ||
            mime.endsWith('+xml') ||
            mime.endsWith('+json') ||
            mime == 'application/xml' ||
            mime == 'application/javascript');
    _text = _binary ? '' : _decode(file.data);
  }

  static String _decode(List<int> bytes) {
    // A cut may land inside a character; what's malformed shows as �.
    return const Utf8Decoder(allowMalformed: true).convert(bytes);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final file = this.file;
    final problem = waiting();
    final details = file == null || file.error.isNotEmpty
        ? ''
        : formatBytes(file.size);
    final controls = <Widget>[
      if (_markdown && !_binary && problem == null)
        _Toggle(
          options: const ['Formatted', 'Plain'],
          selected: _raw ? 1 : 0,
          onChanged: (index) => setState(() => _raw = index == 1),
        ),
      reloadButton(),
    ];
    final Widget body;
    if (problem != null) {
      body = problem;
    } else if (_binary) {
      body = ViewerProblem(
        "Claktory can't show this kind of file here yet.",
        path: path,
      );
    } else if (file!.size == 0) {
      body = ViewerProblem('This file is empty.', path: path);
    } else {
      final note = cutNote(file);
      // Selected with the mouse and copied with ⌘C or a right click's
      // Copy, like the conversation; the line numbers stay out of it.
      body = SelectableArea(
        child: _markdown && !_raw
            ? SingleChildScrollView(
                padding: const EdgeInsets.all(Space.l),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [Markdown(_text), if (note != null) _CutNote(note)],
                ),
              )
            : _NumberedText(text: _text, note: note),
      );
    }
    return PaneScaffold(
      details: details,
      controls: controls,
      body: ColoredBox(color: palette.canvas, child: body),
    );
  }
}

class _CutNote extends StatelessWidget {
  const _CutNote(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(top: Space.m),
    child: Text(
      text,
      style: TextStyles.caption.copyWith(color: Palette.of(context).textFaint),
    ),
  );
}

/// Text in Geist Mono with its line numbers in a gutter beside it. Lines
/// are laid out as they scroll into view, so a long file opens at once.
class _NumberedText extends StatefulWidget {
  const _NumberedText({required this.text, this.note});

  final String text;
  final String? note;

  @override
  State<_NumberedText> createState() => _NumberedTextState();
}

class _NumberedTextState extends State<_NumberedText> {
  final _across = ScrollController();
  late List<String> _lines = _split(widget.text);

  static List<String> _split(String text) {
    final lines = text.split('\n');
    if (lines.length > 1 && lines.last.isEmpty) lines.removeLast();
    return lines;
  }

  @override
  void didUpdateWidget(_NumberedText old) {
    super.didUpdateWidget(old);
    if (old.text != widget.text) _lines = _split(widget.text);
  }

  @override
  void dispose() {
    _across.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    const style = TextStyle(
      fontFamily: TextStyles.mono,
      fontSize: 12,
      height: 1.5,
    );
    const lineHeight = 18.0;
    final digits = '${_lines.length}'.length;
    final gutter = digits * 7.5 + Space.m;
    final note = widget.note;
    // The widest line sets how far it scrolls sideways.
    var widest = 0;
    for (final line in _lines) {
      if (line.length > widest) widest = line.length;
    }
    final width = widest * 7.3 + Space.xl;
    return LayoutBuilder(
      builder: (context, constraints) {
        final textWidth = width > constraints.maxWidth - gutter - Space.m
            ? width
            : constraints.maxWidth - gutter - Space.m;
        return RawScrollbar(
          controller: _across,
          child: SingleChildScrollView(
            controller: _across,
            scrollDirection: Axis.horizontal,
            child: SizedBox(
              width: gutter + Space.m + textWidth,
              child: ListView.builder(
                padding: const EdgeInsets.symmetric(vertical: Space.s),
                itemExtent: lineHeight,
                itemCount: _lines.length + (note == null ? 0 : 1),
                itemBuilder: (context, index) {
                  if (index == _lines.length) {
                    return Padding(
                      padding: EdgeInsets.only(left: gutter + Space.m),
                      child: Text(
                        note!,
                        style: TextStyles.caption.copyWith(
                          color: palette.textFaint,
                        ),
                      ),
                    );
                  }
                  return Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      SizedBox(
                        width: gutter,
                        child: Unselectable(
                          child: Text(
                            '${index + 1}',
                            textAlign: TextAlign.right,
                            style: style.copyWith(color: palette.textFaint),
                          ),
                        ),
                      ),
                      const SizedBox(width: Space.m),
                      Expanded(
                        child: Text(
                          _lines[index],
                          maxLines: 1,
                          softWrap: false,
                          overflow: TextOverflow.clip,
                          style: style.copyWith(color: palette.text),
                        ),
                      ),
                    ],
                  );
                },
              ),
            ),
          ),
        );
      },
    );
  }
}

/// Two or more choices, one picked, as a pill of caption labels.
class _Toggle extends StatelessWidget {
  const _Toggle({
    required this.options,
    required this.selected,
    required this.onChanged,
  });

  final List<String> options;
  final int selected;
  final ValueChanged<int> onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Unselectable(
      child: Container(
        height: 24,
        padding: const EdgeInsets.all(2),
        decoration: BoxDecoration(
          border: Border.all(color: palette.hairline),
          borderRadius: BorderRadius.circular(Radii.control),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            for (var i = 0; i < options.length; i++)
              Semantics(
                button: true,
                selected: i == selected,
                label: options[i],
                excludeSemantics: true,
                child: Pressable(
                  onTap: () => onChanged(i),
                  builder: (context, hovered) => Container(
                    padding: const EdgeInsets.symmetric(horizontal: Space.s),
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      color: i == selected
                          ? palette.selected
                          : hovered
                          ? palette.hover
                          : const Color(0x00000000),
                      borderRadius: BorderRadius.circular(Radii.control - 2),
                    ),
                    child: Text(
                      options[i],
                      style: TextStyles.caption.copyWith(
                        color: i == selected ? palette.text : palette.textMuted,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}
