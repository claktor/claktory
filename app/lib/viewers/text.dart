import 'dart:async';
import 'dart:convert';

import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

import '../annotations.dart';
import '../backend.dart';
import '../controls.dart';
import '../find.dart';
import '../identity.dart';
import '../markdown.dart';
import '../pane_chrome.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../resources.dart' show formatBytes;
import '../selection.dart';
import '../mobile/viewer_chrome.dart' show ChromeTap, ViewerScreen;
import 'markdown_media.dart';
import 'registry.dart';

/// A text file: monospace with line numbers, or Markdown formatted with a
/// toggle to its plain source. Selectable, with the Copy menu; select a
/// passage to comment on it (annotations.dart). It follows the file: when
/// an agent (or anyone) changes it, it reads it again in place.
class TextViewer extends StatefulWidget {
  const TextViewer({super.key, required this.path, this.sealed});

  final String path;

  /// The seal whose kept version shows, rather than the file.
  final FileSeal? sealed;

  @override
  State<TextViewer> createState() => _TextViewerState();
}

class _TextViewerState extends State<TextViewer> with FileLoading<TextViewer> {
  @override
  String get path => widget.path;

  @override
  FileSeal? get sealed => widget.sealed;

  @override
  Widget sealedViewer(FileSeal sealed) =>
      TextViewer(path: path, sealed: sealed);

  var _raw = false;
  String _text = '';
  bool _binary = false;

  /// Some of the text is selected: a tap ends that, nothing more.
  var _selected = false;

  /// A comment was open as the finger came down: a tap closes it.
  var _hadComment = false;

  /// Find in the file (⌘F): what's shown, formatted or plain, searched
  /// here; the view shown answers for it ([_FindHook]).
  final _hook = _FindHook();
  late final _find = LocalFind(
    placeholder: 'Search this file',
    items: () => _hook.items?.call() ?? const [],
    go: (key, nth) => _hook.go?.call(key, nth),
  );

  /// Counts up each time the file is read again (Reload, or it changed):
  /// its pictures and videos are checked again with it, since a picture
  /// rewritten beside it is no file of a pane.
  final _mediaReloads = ValueNotifier(0);

  @override
  Future<void> reload() {
    _mediaReloads.value++;
    return super.reload();
  }

  @override
  void dispose() {
    _find.dispose();
    _mediaReloads.dispose();
    super.dispose();
  }

  /// What's shown changed (read again, or formatted or plain): found again
  /// once it's built.
  /// Shows the file as it's written ([raw]) or formatted. What was
  /// selected goes: the other view's text is laid out differently, so the
  /// same place in it would be other words.
  void _showRaw(bool raw) {
    if (raw == _raw) return;
    comments?.selection.value = null;
    setState(() => _raw = raw);
    // The occurrence the find is on, in sight in the view now shown.
    _shownChanged(reveal: true);
  }

  void _shownChanged({bool reveal = false}) {
    // After the frame's other callbacks: a view built anew records what
    // it shows in its own, so found before it would find nothing
    // (Plain to Formatted).
    WidgetsBinding.instance.addPostFrameCallback((_) {
      scheduleMicrotask(() {
        if (mounted) _find.refresh(reveal: reveal);
      });
    });
  }

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
    _shownChanged();
  }

  static String _decode(List<int> bytes) {
    // A cut may land inside a character; what's malformed shows as �.
    return const Utf8Decoder(allowMalformed: true).convert(bytes);
  }

  /// Comments are on whole text files Claktory read entirely: a cut file
  /// has no version to anchor to.
  bool get _commentable {
    final file = this.file;
    return file != null &&
        !_binary &&
        file.error.isEmpty &&
        file.size > 0 &&
        !file.truncated &&
        file.sha256.isNotEmpty;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final file = this.file;
    final problem = waiting();
    final comments = this.comments;
    final details = this.details(
      file == null || file.error.isNotEmpty ? '' : formatBytes(file.size),
    );
    final commentable = _commentable && comments != null;
    final controls = <Widget>[
      if (commentable) CommentsButton(controller: comments),
      if (_markdown && !_binary && problem == null)
        _Toggle(
          options: const ['Formatted', 'Plain'],
          selected: _raw ? 1 : 0,
          onChanged: (index) => _showRaw(index == 1),
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
      final formatted = _markdown && !_raw;
      final commenting = commentable ? comments : null;
      // Selected with the mouse and copied with ⌘C or a right click's
      // Copy, like the conversation; the line numbers stay out of it.
      final text = SelectableArea(
        // A region of its own for each view, so a selection never carries
        // over to the other's text.
        key: ValueKey(formatted),
        onSelectionChanged: (content) =>
            _selected = content != null && content.plainText.isNotEmpty,
        // On an iPhone, Comment and Suggest join the text's own menu, after
        // Copy: the comment's written in a sheet above the keyboard.
        extra: commenting == null
            ? null
            : (region) => [
                ContextMenuButtonItem(
                  label: 'Comment',
                  onPressed: () => _commentFrom(region, commenting),
                ),
                ContextMenuButtonItem(
                  label: 'Suggest',
                  onPressed: () =>
                      _commentFrom(region, commenting, suggest: true),
                ),
              ],
        child: formatted
            ? _FormattedText(
                path: path,
                mediaReloads: _mediaReloads,
                text: _text,
                version: file.sha256,
                note: note,
                comments: commenting,
                find: _find,
                hook: _hook,
              )
            : _NumberedText(
                text: _text,
                version: file.sha256,
                note: note,
                comments: commenting,
                find: _find,
                hook: _hook,
              ),
      );
      // On a phone, a tap that's nothing else's shows or hides the
      // page's header.
      final tappable = ChromeTap(
        taken: (at) => _selected || _hadComment || _linkAt(context, at),
        child: Listener(
          onPointerDown: (_) => _hadComment = comments?.focused != null,
          child: text,
        ),
      );
      body = commenting == null
          ? tappable
          : Commented(
              controller: commenting,
              insetInside: true,
              child: tappable,
            );
    }
    return PaneScaffold(
      share: sharePath,
      details: details,
      controls: controls,
      comments: commentable ? CommentsButton(controller: comments) : null,
      actions: [
        if (_markdown && !_binary && problem == null)
          PaneAction('Plain text', () => _showRaw(!_raw), active: _raw),
        PaneAction('Reload', reload),
        ...sealActions(),
      ],
      notice: sealNotice(),
      body: FindScope(
        target: _find,
        child: ColoredBox(color: palette.canvas, child: body),
      ),
    );
  }
}

/// What a phone's whole-screen page leaves at its foot for the home
/// indicator, for the text to scroll clear of; nothing elsewhere.
double _footInset(BuildContext context) =>
    ViewerScreen.wholeOf(context) ? MediaQuery.paddingOf(context).bottom : 0;

/// Whether a link's words are at [global]: a tap there opens it.
bool _linkAt(BuildContext context, Offset global) {
  final result = HitTestResult();
  WidgetsBinding.instance.hitTestInView(
    result,
    global,
    View.of(context).viewId,
  );
  return result.path.any(
    (entry) =>
        entry.target is TextSpan &&
        (entry.target as TextSpan).recognizer != null,
  );
}

/// Comment (or Suggest) from the iPhone's menu over a selection: the
/// passage is the selection's, as the controller last heard it; the menu
/// and the selection's handles go, the passage stays lit while it's
/// written on.
void _commentFrom(
  SelectableRegionState region,
  CommentsController comments, {
  bool suggest = false,
}) {
  comments.comment(suggest: suggest);
  region.hideToolbar();
  region.clearSelection();
}

/// The text under a selection changed (an agent rewrote the file): the
/// selection goes, rather than staying where it was over other words, or
/// over some blocks of it and not those between.
void _clearSelection(BuildContext context) {
  final region = context.findAncestorStateOfType<SelectableRegionState>();
  region?.hideToolbar();
  region?.clearSelection();
}

/// How the view shown answers the viewer's find: its items (blocks or
/// lines) and bringing an occurrence into sight.
class _FindHook {
  Iterable<(Object, String)> Function()? items;
  void Function(Object key, int nth)? go;

  void attach(
    Iterable<(Object, String)> Function() items,
    void Function(Object key, int nth) go,
  ) {
    this.items = items;
    this.go = go;
  }
}

/// Where the [nth] occurrence of [query] starts in [text], matched as
/// find matches; 0 when it isn't there.
int _occurrence(String text, String query, int nth) {
  final lower = text.toLowerCase();
  final needle = query.toLowerCase();
  var at = lower.indexOf(needle);
  for (var i = 0; i < nth && at >= 0; i++) {
    at = lower.indexOf(needle, at + needle.length);
  }
  return at < 0 ? 0 : at;
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

/// The thread highlights in [range] of a viewer's text, for [markSpan]:
/// each passage in its agent's color, and the one a comment is being
/// written on in [writing] (the selection's color, as it was selected).
List<(int, int, Color)> _marksIn(
  CommentsController? comments,
  Backend? backend,
  int start,
  int end, {
  required Color writing,
}) {
  if (comments == null) return const [];
  final marks = <(int, int, Color)>[];
  final composing = comments.composing;
  if (composing is Passage && composing.end > start && composing.start < end) {
    marks.add((composing.start, composing.end, writing));
  }
  for (final thread in comments.shown) {
    final range = comments.located[thread.id];
    if (range == null || range.end <= start || range.start >= end) continue;
    var color = '';
    for (final agent in backend?.workspace.workspace.agents ?? <Agent>[]) {
      if (agent.id == thread.agentId) color = agent.color;
    }
    marks.add((
      range.start,
      range.end,
      passageColor(color, focused: comments.focused == thread.id),
    ));
  }
  return marks;
}

/// The thread whose passage holds [offset], for a click on it.
String? _threadAt(CommentsController comments, int offset) {
  for (final MapEntry(key: id, value: range) in comments.located.entries) {
    if (offset >= range.start && offset < range.end) return id;
  }
  return null;
}

/// Whether [paragraph]'s layout can be read: it's in the tree and laid
/// out, and (checked only while debugging) not waiting to be laid out
/// again. Read while building, it's the last frame's; the viewer asks
/// for its comments to be placed again once it's laid out.
bool _laidOut(RenderParagraph paragraph) {
  if (!paragraph.attached || !paragraph.hasSize) return false;
  var current = true;
  assert(() {
    current = !paragraph.debugNeedsLayout;
    return true;
  }());
  return current;
}

/// One block of text the person can select and comment on: where it is
/// in the viewer's text ([base]), and what it is.
class _Block {
  final key = GlobalKey();
  final selection = SelectionListenerNotifier();
  int base = 0;
  int length = 0;

  /// The block's text as laid out: its render object, or the first
  /// paragraph under it (a link's hover region may wrap it).
  RenderParagraph? get paragraph {
    RenderParagraph? found;
    void look(RenderObject object) {
      if (found != null) return;
      if (object is RenderParagraph) {
        found = object;
      } else {
        object.visitChildren(look);
      }
    }

    final object = key.currentContext?.findRenderObject();
    if (object != null) look(object);
    return found;
  }

  void dispose() => selection.dispose();
}

/// What the blocks of a viewer's text have in common: telling the
/// controller what's selected and where an offset is.
mixin _Blocks<T extends StatefulWidget> on State<T> {
  CommentsController? get comments;
  String get kind;
  String get shownText;

  /// The box offsets are measured against: the viewer's.
  final viewer = GlobalKey();

  /// Every block made so far, by index; the ones in use are [inUse].
  final blocks = <_Block>[];
  int inUse = 0;

  _Block blockAt(int index) {
    while (blocks.length <= index) {
      final block = _Block();
      block.selection.addListener(_selectionChanged);
      blocks.add(block);
    }
    return blocks[index];
  }

  /// The text changed: its blocks are made afresh. A block reused would
  /// land somewhere else in the new text (a paragraph now in a quote), its
  /// notifier going to the listener there while the one it left still
  /// holds it (the pane's debug red box), and that one, gone,
  /// then unregistering it from the new one. The old ones are disposed
  /// after the frame, once their listeners are gone.
  void renewBlocks() {
    final old = List.of(blocks);
    blocks.clear();
    inUse = 0;
    if (old.isEmpty) return;
    SchedulerBinding.instance.addPostFrameCallback((_) {
      for (final block in old) {
        block.dispose();
      }
    });
  }

  @override
  void dispose() {
    for (final block in blocks) {
      block.dispose();
    }
    super.dispose();
  }

  var _selectionDue = false;

  /// A block's selection changed: read them all after the frame, once,
  /// since a block's range can't be read while the selectables are being
  /// updated (its paragraph laid out again under the selection).
  void _selectionChanged() {
    if (_selectionDue) return;
    _selectionDue = true;
    SchedulerBinding.instance
      ..addPostFrameCallback((_) {
        _selectionDue = false;
        if (mounted) _selected();
      })
      ..scheduleFrame();
  }

  /// The person's selection across the blocks, as a passage of the text.
  void _selected() {
    final comments = this.comments;
    if (comments == null) return;
    int? start;
    int? end;
    for (var i = 0; i < inUse && i < blocks.length; i++) {
      final block = blocks[i];
      final range = _rangeOf(block);
      if (range == null) continue;
      final from = range.startOffset < range.endOffset
          ? range.startOffset
          : range.endOffset;
      final to = range.startOffset < range.endOffset
          ? range.endOffset
          : range.startOffset;
      // A block the selection only touches at its edge isn't in it.
      if (from == to) continue;
      start ??= block.base + from;
      end = block.base + to;
    }
    final text = shownText;
    final passage =
        start == null || end == null || end <= start || end > text.length
        ? null
        : Passage(kind, text, start, end);
    if (comments.selection.value != passage) {
      comments.selection.value = passage;
    }
  }

  /// What of [block] is selected, if anything. Flutter's range can throw
  /// (a null check in MultiSelectableSelectionContainerDelegate) when the
  /// paragraph under the selection was drawn again, such as when a comment
  /// highlights it or the file reloads: that block counts as unselected
  /// till the next change.
  SelectedContentRange? _rangeOf(_Block block) {
    if (!block.selection.registered) return null;
    final selection = block.selection.selection;
    if (selection.status == SelectionStatus.none) return null;
    try {
      return selection.range;
    } on TypeError {
      return null;
    }
  }

  /// The block [offset] is in.
  _Block? blockOf(int offset) {
    for (var i = 0; i < inUse && i < blocks.length; i++) {
      final block = blocks[i];
      if (offset >= block.base && offset <= block.base + block.length) {
        return block;
      }
    }
    return null;
  }

  /// Where [offset] is, as a y in the viewer's box.
  double? yOf(int offset) {
    final block = blockOf(offset);
    final paragraph = block?.paragraph;
    final box = viewer.currentContext?.findRenderObject();
    if (block == null ||
        paragraph == null ||
        !_laidOut(paragraph) ||
        box is! RenderBox) {
      return null;
    }
    final local = (offset - block.base).clamp(0, block.length);
    final boxes = paragraph.getBoxesForSelection(
      TextSelection(baseOffset: local, extentOffset: local + 1),
    );
    final top = boxes.isEmpty ? 0.0 : boxes.first.top;
    return paragraph.localToGlobal(Offset(0, top), ancestor: box).dy;
  }

  /// Where [start] to [end] is on screen, global: from its first line's
  /// top to its last line's bottom, from where it starts to the furthest
  /// right its first and last lines reach.
  Rect? rectOf(int start, int end) {
    final first = boxAt(start);
    if (first == null) return null;
    final last = end - 1 > start ? (boxAt(end - 1) ?? first) : first;
    return Rect.fromLTRB(
      first.left,
      first.top,
      last.right > first.right ? last.right : first.right,
      last.bottom > first.bottom ? last.bottom : first.bottom,
    );
  }

  /// The box of the character at [offset], global; null when its block
  /// isn't laid out.
  Rect? boxAt(int offset) {
    final block = blockOf(offset);
    final paragraph = block?.paragraph;
    if (block == null || paragraph == null || !_laidOut(paragraph)) {
      return null;
    }
    final local = (offset - block.base).clamp(0, block.length);
    final boxes = paragraph.getBoxesForSelection(
      TextSelection(baseOffset: local, extentOffset: local + 1),
    );
    final Rect box;
    if (boxes.isEmpty) {
      // An empty block, or its end: where the caret would be.
      final at = paragraph.getOffsetForCaret(
        TextPosition(offset: local),
        Rect.zero,
      );
      final height = paragraph.getFullHeightForCaret(
        TextPosition(offset: local),
      );
      box = Rect.fromLTWH(at.dx, at.dy, 0, height);
    } else {
      box = boxes.first.toRect();
    }
    return box.shift(paragraph.localToGlobal(Offset.zero));
  }

  /// A click on a passage picks its thread; anywhere else lets go.
  void tapped(Offset global) {
    final comments = this.comments;
    if (comments == null) return;
    for (var i = 0; i < inUse && i < blocks.length; i++) {
      final block = blocks[i];
      final paragraph = block.paragraph;
      if (paragraph == null || !paragraph.attached) continue;
      final local = paragraph.globalToLocal(global);
      if (!(Offset.zero & paragraph.size).contains(local)) continue;
      final position = paragraph.getPositionForOffset(local);
      final id = _threadAt(comments, block.base + position.offset);
      if (id != null) {
        comments.focus(id);
        return;
      }
      break;
    }
    comments.letGo();
  }
}

/// Markdown, formatted: each block of text it renders is a [_Block], so a
/// selection becomes a passage of the rendered text (blocks joined by
/// line breaks) and threads' passages are highlighted where they are.
class _FormattedText extends StatefulWidget {
  const _FormattedText({
    required this.path,
    required this.mediaReloads,
    required this.text,
    required this.version,
    required this.note,
    required this.comments,
    required this.find,
    required this.hook,
  });

  /// The file's path, where its pictures and videos are found.
  final String path;

  /// Ticks when the file is read again, for its pictures and videos.
  final Listenable mediaReloads;
  final String text;
  final String version;
  final String? note;
  final CommentsController? comments;
  final LocalFind find;
  final _FindHook hook;

  @override
  State<_FormattedText> createState() => _FormattedTextState();
}

class _FormattedTextState extends State<_FormattedText>
    with _Blocks<_FormattedText> {
  final _scroll = ScrollController();

  @override
  CommentsController? get comments => widget.comments;

  @override
  String get kind => 'markdown';

  /// The rendered text, as the last build laid it out.
  @override
  String shownText = '';

  /// The blocks' texts as this build makes them.
  final _building = <String>[];
  var _offset = 0;

  @override
  void initState() {
    super.initState();
    _scroll.addListener(_moved);
    _attach();
    widget.hook.attach(_findItems, _findGo);
  }

  @override
  void didUpdateWidget(_FormattedText old) {
    super.didUpdateWidget(old);
    if (old.text != widget.text) {
      _clearSelection(context);
      renewBlocks();
    }
    if (old.comments != widget.comments) _attach();
    widget.hook.attach(_findItems, _findGo);
  }

  /// The rendered blocks, as the last build laid them out.
  var _shown = const <String>[];

  Iterable<(Object, String)> _findItems() => [
    for (final (index, text) in _shown.indexed) (index, text),
  ];

  /// Brings block [key]'s [nth] occurrence into sight: a quarter of the
  /// way down, unless it's in sight already.
  void _findGo(Object key, int nth) {
    if (key is! int || key >= _shown.length || key >= blocks.length) return;
    final block = blocks[key];
    final at = _occurrence(_shown[key], widget.find.marks.value.query, nth);
    final box = boxAt(block.base + at);
    final view = _scroll.hasClients
        ? _scroll.position.context.notificationContext?.findRenderObject()
        : null;
    if (box == null || view is! RenderBox || !view.attached) return;
    final top = view.localToGlobal(Offset.zero).dy;
    final height = view.size.height;
    if (box.top < top + Space.xxl || box.bottom > top + height - Space.l) {
      _scrollBy(box.top - (top + height * 0.25));
    }
    // In a block that scrolls sideways (a wide table, a code block), across
    // too, where it's past the edge.
    final context = block.key.currentContext;
    final across = context == null ? null : Scrollable.maybeOf(context);
    final band = across?.context.findRenderObject();
    if (across == null ||
        across.position.axis != Axis.horizontal ||
        band is! RenderBox ||
        !band.attached) {
      return;
    }
    final end = boxAt(
      block.base + at + widget.find.marks.value.query.length - 1,
    );
    final left = band.localToGlobal(Offset.zero).dx;
    final right = left + band.size.width;
    final from = box.left;
    final to = end != null && end.right > from ? end.right : box.right;
    if (from >= left + Space.m && to <= right - Space.m) return;
    final position = across.position;
    // In the middle, so what's around it reads too.
    final middle = (from + to) / 2 - (left + right) / 2;
    position.jumpTo(
      (position.pixels + middle).clamp(
        position.minScrollExtent,
        position.maxScrollExtent,
      ),
    );
  }

  void _attach() {
    final comments = widget.comments;
    if (comments == null) return;
    comments.yOf = yOf;
    comments.reveal = _reveal;
    comments.rectOfRange = rectOf;
    comments.scrollBy = _scrollBy;
  }

  @override
  void dispose() {
    final comments = widget.comments;
    if (comments?.yOf == yOf) comments?.yOf = null;
    if (comments?.rectOfRange == rectOf) comments?.rectOfRange = null;
    if (comments?.scrollBy == _scrollBy) comments?.scrollBy = null;
    _scroll.dispose();
    super.dispose();
  }

  void _moved() => widget.comments?.relayout();

  /// The screen's width as last built: turned, the text flows anew.
  double? _width;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final width = MediaQuery.sizeOf(context).width;
    if (_width != null && width != _width) _keepPlace();
    _width = width;
  }

  /// The text is about to flow to another width (a phone turned): the
  /// block at the view's top, and how far into it, stays there once it
  /// has, rather than the same distance from the start, which is other
  /// words now.
  void _keepPlace() {
    if (!_scroll.hasClients || _scroll.position.pixels <= 0) return;
    RenderBox? viewBox() {
      final view = _scroll.position.context.notificationContext
          ?.findRenderObject();
      return view is RenderBox && view.attached && view.hasSize ? view : null;
    }

    RenderBox? boxOf(GlobalKey key) {
      final box = key.currentContext?.findRenderObject();
      return box is RenderBox && box.attached && box.hasSize ? box : null;
    }

    final view = viewBox();
    if (view == null) return;
    final top = view.localToGlobal(Offset.zero).dy;
    for (var i = 0; i < inUse && i < blocks.length; i++) {
      final key = blocks[i].key;
      final box = boxOf(key);
      if (box == null) continue;
      final y = box.localToGlobal(Offset.zero).dy;
      final height = box.size.height;
      if (y + height <= top) continue;
      final into = height <= 0 ? 0.0 : ((top - y) / height).clamp(0.0, 1.0);
      WidgetsBinding.instance.addPostFrameCallback((_) {
        final view = viewBox();
        final box = boxOf(key);
        if (!mounted || !_scroll.hasClients || view == null || box == null) {
          return;
        }
        final now = box.localToGlobal(Offset.zero).dy;
        final top = view.localToGlobal(Offset.zero).dy;
        _scrollBy(now - top + into * box.size.height);
      });
      return;
    }
  }

  void _scrollBy(double dy) {
    if (!_scroll.hasClients) return;
    final position = _scroll.position;
    _scroll.jumpTo(
      (position.pixels + dy).clamp(
        position.minScrollExtent,
        position.maxScrollExtent,
      ),
    );
  }

  void _reveal(int offset) {
    final context = blockOf(offset)?.key.currentContext;
    if (context == null) return;
    unawaited(
      Scrollable.ensureVisible(
        context,
        alignment: 0.25,
        duration: Motion.base,
        curve: Curves.easeOut,
      ),
    );
  }

  Widget _block(int index, TextSpan span, Widget Function(TextSpan) build) {
    final found = widget.find.marks.value;
    if (index == 0) {
      _building.clear();
      _offset = 0;
      WidgetsBinding.instance.addPostFrameCallback((_) => _built());
    }
    final plain = span.toPlainText();
    final block = blockAt(index)
      ..base = _offset
      ..length = plain.length;
    _building.add(plain);
    _offset += plain.length + 1;
    inUse = index + 1;
    final marks = _marksIn(
      widget.comments,
      widget.comments?.backend,
      block.base,
      block.base + block.length,
      writing: _writing,
    );
    return SelectionListener(
      selectionNotifier: block.selection,
      child: KeyedSubtree(
        key: block.key,
        child: build(
          markFound(
            markSpan(span, marks, base: block.base),
            found.query,
            current: found.at == index,
            nth: found.nth,
          ),
        ),
      ),
    );
  }

  /// After a build: the rendered text, for the controller to find threads
  /// in, and where they now are.
  void _built() {
    if (!mounted) return;
    shownText = _building.join('\n');
    _shown = List.of(_building);
    widget.comments?.show(kind: kind, text: shownText, version: widget.version);
    widget.comments?.relayout();
  }

  /// The highlight of the passage a comment is being written on.
  Color _writing = const Color(0x00000000);

  @override
  Widget build(BuildContext context) {
    final note = widget.note;
    final comments = widget.comments;
    _writing = Palette.of(context).signal.withValues(alpha: 0.28);
    // Room at the right for the comments' cards, inside what scrolls.
    final inset = CommentsInset.of(context);
    Widget content() => SingleChildScrollView(
      controller: _scroll,
      // Its end clear of the home indicator on a phone's whole screen.
      padding: EdgeInsets.fromLTRB(
        Space.l,
        Space.l,
        Space.l + inset,
        Space.l + _footInset(context),
      ),
      // Selected where the pointer is at any zoom.
      child: ScrolledSelection(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // Its blocks are found in and marked, commented on or not.
            ValueListenableBuilder(
              valueListenable: widget.find.marks,
              builder: (context, _, _) => Markdown(
                widget.text,
                textBlock: _block,
                files: FileMarkdownMedia(
                  widget.path,
                  reloads: widget.mediaReloads,
                ),
              ),
            ),
            if (note != null) _CutNote(note),
          ],
        ),
      ),
    );
    if (comments == null) return content();
    return ListenableBuilder(
      // Highlights follow the threads and which one is picked.
      listenable: comments,
      builder: (context, _) => Listener(
        key: viewer,
        onPointerDown: (event) => _down = event.position,
        onPointerUp: (event) {
          final down = _down;
          if (down != null && (event.position - down).distance < 4) {
            tapped(event.position);
          }
        },
        child: content(),
      ),
    );
  }

  Offset? _down;
}

/// Text in Geist Mono with its line numbers in a gutter beside it. Lines
/// are laid out as they scroll into view, so a long file opens at once.
/// Each line is a [_Block], so a selection becomes a passage of the file.
class _NumberedText extends StatefulWidget {
  const _NumberedText({
    required this.text,
    required this.version,
    this.note,
    this.comments,
    required this.find,
    required this.hook,
  });

  final String text;
  final String version;
  final String? note;
  final CommentsController? comments;
  final LocalFind find;
  final _FindHook hook;

  @override
  State<_NumberedText> createState() => _NumberedTextState();
}

class _NumberedTextState extends State<_NumberedText>
    with _Blocks<_NumberedText> {
  final _across = ScrollController();
  final _down = ScrollController();
  late List<String> _lines = _split(widget.text);
  late List<int> _starts = _startsOf(_lines);

  static const _lineHeight = 18.0;
  static const _padding = Space.s;

  @override
  CommentsController? get comments => widget.comments;

  @override
  String get kind => 'text';

  @override
  String get shownText => widget.text;

  static List<String> _split(String text) {
    final lines = text.split('\n');
    if (lines.length > 1 && lines.last.isEmpty) lines.removeLast();
    return lines;
  }

  static List<int> _startsOf(List<String> lines) {
    final starts = <int>[];
    var at = 0;
    for (final line in lines) {
      starts.add(at);
      at += line.length + 1;
    }
    return starts;
  }

  @override
  void initState() {
    super.initState();
    _down.addListener(_moved);
    _attach();
    _show();
    widget.hook.attach(_findItems, _findGo);
    widget.find.marks.addListener(_marked);
  }

  /// A find's marks moved: the lines show them.
  void _marked() {
    if (mounted) setState(() {});
  }

  Iterable<(Object, String)> _findItems() => [
    for (final (index, line) in _lines.indexed) (index, line),
  ];

  /// Brings line [key]'s [nth] occurrence into sight: down to it, and
  /// across when it's past the edge.
  void _findGo(Object key, int nth) {
    if (key is! int || key >= _lines.length || !_down.hasClients) return;
    final down = _down.position;
    final y = key * _lineHeight;
    if (y < down.pixels + Space.xxl ||
        y + _lineHeight > down.pixels + down.viewportDimension) {
      _down.jumpTo(
        (y - 3 * _lineHeight).clamp(down.minScrollExtent, down.maxScrollExtent),
      );
    }
    if (!_across.hasClients) return;
    final across = _across.position;
    final query = widget.find.marks.value.query;
    final x = _gutter + Space.m + _occurrence(_lines[key], query, nth) * 7.3;
    final width = query.length * 7.3;
    if (x < across.pixels + _gutter + Space.m ||
        x + width > across.pixels + across.viewportDimension - Space.l) {
      _across.jumpTo(
        (x - _gutter - Space.m - Space.xl).clamp(
          across.minScrollExtent,
          across.maxScrollExtent,
        ),
      );
    }
  }

  @override
  void didUpdateWidget(_NumberedText old) {
    super.didUpdateWidget(old);
    if (old.text != widget.text) {
      _clearSelection(context);
      _lines = _split(widget.text);
      _starts = _startsOf(_lines);
    }
    if (old.comments != widget.comments) _attach();
    widget.hook.attach(_findItems, _findGo);
    if (old.text != widget.text ||
        old.version != widget.version ||
        old.comments != widget.comments) {
      _show();
    }
  }

  void _attach() {
    final comments = widget.comments;
    if (comments == null) return;
    comments.yOf = _yOf;
    comments.reveal = _reveal;
    comments.rectOfRange = _rectOf;
    comments.scrollBy = _scrollBy;
  }

  /// A line not laid out (scrolled away) is still where its number says:
  /// the start of its text column.
  Rect? _rectOf(int start, int end) {
    final laidOut = rectOf(start, end);
    if (laidOut != null) return laidOut;
    final y = _yOf(start);
    final box = viewer.currentContext?.findRenderObject();
    if (y == null || box is! RenderBox || !box.attached) return null;
    final across = _across.hasClients ? _across.offset : 0.0;
    final lines = _lineOf(end) - _lineOf(start) + 1;
    final topLeft = box.localToGlobal(Offset(_gutter + Space.m - across, y));
    return Rect.fromLTWH(topLeft.dx, topLeft.dy, 0, lines * _lineHeight);
  }

  void _scrollBy(double dy) {
    if (!_down.hasClients) return;
    final position = _down.position;
    _down.jumpTo(
      (position.pixels + dy).clamp(
        position.minScrollExtent,
        position.maxScrollExtent,
      ),
    );
  }

  /// The line numbers' width, as the last build made it.
  double _gutter = 0;

  void _show() {
    final comments = widget.comments;
    if (comments == null) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      comments.show(kind: kind, text: widget.text, version: widget.version);
    });
  }

  @override
  void dispose() {
    final comments = widget.comments;
    if (comments?.yOf == _yOf) comments?.yOf = null;
    if (comments?.rectOfRange == _rectOf) comments?.rectOfRange = null;
    if (comments?.scrollBy == _scrollBy) comments?.scrollBy = null;
    widget.find.marks.removeListener(_marked);
    _across.dispose();
    _down.dispose();
    super.dispose();
  }

  void _moved() => widget.comments?.relayout();

  int _lineOf(int offset) {
    var low = 0;
    var high = _starts.length - 1;
    while (low < high) {
      final middle = (low + high + 1) >> 1;
      if (_starts[middle] <= offset) {
        low = middle;
      } else {
        high = middle - 1;
      }
    }
    return low;
  }

  /// Lines are all the same height, so a line's y is known before it's
  /// laid out.
  double? _yOf(int offset) {
    if (_starts.isEmpty) return null;
    final scrolled = _down.hasClients ? _down.offset : 0.0;
    return _padding + _lineOf(offset) * _lineHeight - scrolled;
  }

  void _reveal(int offset) {
    if (!_down.hasClients) return;
    final target = (_lineOf(offset) * _lineHeight - 3 * _lineHeight).clamp(
      0.0,
      _down.position.maxScrollExtent,
    );
    unawaited(
      _down.animateTo(target, duration: Motion.base, curve: Curves.easeOut),
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    const style = TextStyle(
      fontFamily: TextStyles.mono,
      fontSize: 12,
      height: 1.5,
    );
    final digits = '${_lines.length}'.length;
    final gutter = digits * 7.5 + Space.m;
    _gutter = gutter;
    // Room at the right for the comments' cards, inside what scrolls.
    final inset = CommentsInset.of(context);
    final writing = palette.signal.withValues(alpha: 0.28);
    final note = widget.note;
    final comments = widget.comments;
    inUse = comments == null ? 0 : _lines.length;
    // The widest line sets how far it scrolls sideways.
    var widest = 0;
    for (final line in _lines) {
      if (line.length > widest) widest = line.length;
    }
    final width = widest * 7.3 + Space.xl;
    final found = widget.find.marks.value;
    // A line's matches marked, the one gone to in full.
    TextSpan marked(TextSpan span, int index) => markFound(
      span,
      found.query,
      current: found.at == index,
      nth: found.nth,
    );
    Widget lineText(int index) {
      final text = Text.rich(
        marked(
          TextSpan(
            text: _lines[index],
            style: style.copyWith(color: palette.text),
          ),
          index,
        ),
        maxLines: 1,
        softWrap: false,
        overflow: TextOverflow.clip,
      );
      if (comments == null) return text;
      final block = blockAt(index)
        ..base = _starts[index]
        ..length = _lines[index].length;
      final marks = _marksIn(
        comments,
        comments.backend,
        block.base,
        block.base + block.length,
        writing: writing,
      );
      return SelectionListener(
        selectionNotifier: block.selection,
        child: KeyedSubtree(
          key: block.key,
          child: Text.rich(
            marked(
              markSpan(
                TextSpan(
                  text: _lines[index],
                  style: style.copyWith(color: palette.text),
                ),
                marks,
                base: block.base,
              ),
              index,
            ),
            maxLines: 1,
            softWrap: false,
            overflow: TextOverflow.clip,
          ),
        ),
      );
    }

    Widget view() => LayoutBuilder(
      builder: (context, constraints) {
        final room = constraints.maxWidth - gutter - Space.m - inset;
        final textWidth = (width > room ? width : room) + inset;
        return RawScrollbar(
          controller: _across,
          child: SingleChildScrollView(
            controller: _across,
            scrollDirection: Axis.horizontal,
            child: SizedBox(
              width: gutter + Space.m + textWidth,
              // Selected where the pointer is at any zoom, sideways
              // here and down each line.
              child: ScrolledSelection(
                child: ListView.builder(
                  controller: _down,
                  padding: EdgeInsets.only(
                    top: _padding,
                    bottom: _padding + _footInset(context),
                  ),
                  itemExtent: _lineHeight,
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
                    // Copied, each line ends its own, and the blank
                    // lines after it come along (they hold nothing to
                    // select).
                    var blanks = 0;
                    while (index + blanks + 1 < _lines.length &&
                        _lines[index + blanks + 1].isEmpty) {
                      blanks++;
                    }
                    return ScrolledSelection(
                      trail: '\n' * (blanks + 1),
                      child: Row(
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
                          Expanded(child: lineText(index)),
                        ],
                      ),
                    );
                  },
                ),
              ),
            ),
          ),
        );
      },
    );
    if (comments == null) return view();
    return ListenableBuilder(
      listenable: comments,
      builder: (context, _) => Listener(
        key: viewer,
        onPointerDown: (event) => _pressed = event.position,
        onPointerUp: (event) {
          final down = _pressed;
          if (down != null && (event.position - down).distance < 4) {
            tapped(event.position);
          }
        },
        child: view(),
      ),
    );
  }

  Offset? _pressed;
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
                onTap: () => onChanged(i),
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
