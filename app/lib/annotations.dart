import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'selection.dart';
import 'usage.dart' show formatAge;

// Comments on files (factory-research/annotations.md): the person selects a
// passage a viewer shows, comments on it, and sends the comments to an agent,
// which revises the file and replies. Threads live in the backend, by file;
// the app shows them beside the viewer (a margin on a wide pane, a sheet on a
// narrow one) and highlights their passages.

/// Every thread on each file the backend has told this device about.
class ThreadStore extends ChangeNotifier {
  final _lists = <String, ThreadList>{};

  /// [path]'s threads, and the file's version they're anchored to; null
  /// until the backend has sent them.
  ThreadList? of(String path) => _lists[path];

  void apply(ThreadList list) {
    _lists[list.path] = list;
    notifyListeners();
  }

  /// One thread, new or changed (the answer to creating one).
  void put(Thread thread) {
    final list = _lists[thread.path];
    if (list == null) {
      _lists[thread.path] = ThreadList(
        path: thread.path,
        versionSha256: thread.versionSha256,
        threads: [thread],
      );
    } else {
      final threads = list.threads;
      final at = threads.indexWhere((old) => old.id == thread.id);
      if (at == -1) {
        threads.add(thread);
      } else {
        threads[at] = thread;
      }
    }
    notifyListeners();
  }
}

/// A passage of a viewer's text the person selected: what a new thread is
/// anchored to. [start] and [end] are UTF-16 offsets into the text the
/// viewer shows ([kind] "markdown": the rendered text; "text": the file).
class Passage {
  factory Passage(String kind, String text, int a, int b) {
    final start = a < b ? a : b;
    final end = a < b ? b : a;
    const context = 32;
    return Passage._(
      kind: kind,
      start: start,
      end: end,
      exact: text.substring(start, end),
      prefix: text.substring(start - context < 0 ? 0 : start - context, start),
      suffix: text.substring(
        end,
        end + context > text.length ? text.length : end + context,
      ),
    );
  }

  const Passage._({
    required this.kind,
    required this.start,
    required this.end,
    required this.exact,
    required this.prefix,
    required this.suffix,
  });

  final String kind;
  final int start;
  final int end;
  final String exact;
  final String prefix;
  final String suffix;

  bool get isEmpty => exact.trim().isEmpty;

  /// As W3C Web Annotation selectors: a quote with its context, and where
  /// it is.
  List<Selector> get selectors => [
    Selector(
      json: jsonEncode({
        'type': 'TextQuoteSelector',
        'exact': exact,
        'prefix': prefix,
        'suffix': suffix,
      }),
    ),
    Selector(
      json: jsonEncode({
        'type': 'TextPositionSelector',
        'start': start,
        'end': end,
      }),
    ),
  ];

  @override
  bool operator ==(Object other) =>
      other is Passage &&
      other.kind == kind &&
      other.start == start &&
      other.end == end &&
      other.exact == exact;

  @override
  int get hashCode => Object.hash(kind, start, end, exact);
}

/// Where [anchor]'s passage is in [text]: its quote, at its place if it's
/// still there, else the match nearest it. Runs of whitespace match any
/// whitespace, since a viewer and the backend may join blocks differently.
/// Null when the quote isn't in [text].
TextRange? locatePassage(String text, TextAnchor anchor) {
  final exact = anchor.exact;
  if (exact.trim().isEmpty) return null;
  if (anchor.start >= 0 &&
      anchor.end <= text.length &&
      anchor.start <= anchor.end &&
      text.substring(anchor.start, anchor.end) == exact) {
    return TextRange(start: anchor.start, end: anchor.end);
  }
  final words = exact.trim().split(RegExp(r'\s+')).map(RegExp.escape);
  final pattern = RegExp(words.join(r'\s+'));
  TextRange? best;
  var distance = 0;
  for (final match in pattern.allMatches(text)) {
    final away = (match.start - anchor.start).abs();
    if (best == null || away < distance) {
      best = TextRange(start: match.start, end: match.end);
      distance = away;
    }
  }
  return best;
}

/// The anchor [thread] has in a viewer showing [kind]'s text.
TextAnchor? anchorIn(Thread thread, String kind) {
  if (thread.anchor == AnchorState.ANCHOR_STATE_DETACHED) return null;
  final anchor = kind == 'markdown' ? thread.rendered : thread.source;
  return anchor.exact.isEmpty ? null : anchor;
}

/// Whether [thread] has a comment of the person's not sent yet.
bool hasDraft(Thread thread) => thread.comments.any((c) => c.draft);

/// The viewer side of comments, for one file in one viewer: the threads on
/// it, where their passages are in the text it shows, the person's
/// selection, and what's being written. The viewer tells it its text, how
/// to find a y for an offset ([yOf]) and how to scroll to one ([reveal]),
/// and calls [relayout] when what it shows moves.
class CommentsController extends ChangeNotifier {
  CommentsController({required this.backend, required this.path}) {
    backend.threads.addListener(_threadsChanged);
    backend.connected.addListener(_reconnected);
  }

  final Backend backend;
  final String path;

  /// "markdown" while the Markdown shows formatted, else "text".
  String _kind = 'text';
  String get kind => _kind;

  /// The text the viewer shows, in [kind]'s space.
  String _text = '';

  /// The file's version the viewer shows (FileContent.sha256).
  String _version = '';

  /// Where each thread's passage is in the text shown, by thread id.
  var located = <String, TextRange>{};

  /// The person's selection, while there is one.
  final selection = ValueNotifier<Passage?>(null);

  /// The passage a comment is being written on, while it is.
  Passage? composing;

  /// What's being written on [composing] suggests a change to it.
  var suggesting = false;

  /// The thread a reply is being written to, or a comment edited in.
  String? replyingTo;
  String? editing;

  /// The thread the person picked: its card and passage stand out.
  String? focused;

  /// Resolved threads show too.
  var showResolved = false;

  /// On a narrow pane, the sheet listing them is up.
  var sheetOpen = false;

  /// The pane is too narrow for a margin: the header's Comments button
  /// opens the sheet instead.
  final narrow = ValueNotifier<bool>(false);

  /// Why the last request failed, till the next one.
  String? error;

  /// Where an offset of the text shown is, as a y in the viewer's box;
  /// null when it isn't laid out.
  double? Function(int offset)? yOf;

  /// Scrolls the viewer so an offset is in view.
  void Function(int offset)? reveal;

  /// Fires when what the viewer shows moved (scrolled, laid out again).
  final layout = ChangeNotifier();

  void relayout() => layout.notifyListeners();

  @override
  void dispose() {
    backend.threads.removeListener(_threadsChanged);
    backend.connected.removeListener(_reconnected);
    selection.dispose();
    narrow.dispose();
    layout.dispose();
    super.dispose();
  }

  /// Whether the viewer has shown the file's text yet: only then are its
  /// threads asked for, so a file that can't take comments asks nothing.
  var _shown = false;

  /// Asks for the threads again (first shown, after a reconnect).
  void refresh() {
    if (!_shown || !backend.connected.value) return;
    backend.listThreads(path).catchError((Object error) {});
  }

  void _reconnected() {
    if (backend.connected.value) refresh();
  }

  /// What the viewer shows now.
  void show({
    required String kind,
    required String text,
    required String version,
  }) {
    if (!_shown) {
      _shown = true;
      refresh();
    }
    if (kind == _kind && text == _text && version == _version) return;
    final kindChanged = kind != _kind;
    _kind = kind;
    _text = text;
    _version = version;
    if (kindChanged) {
      selection.value = null;
      composing = null;
    }
    _locate();
    notifyListeners();
  }

  void _threadsChanged() {
    _locate();
    notifyListeners();
  }

  /// Every thread on the file, as the backend last sent them.
  List<Thread> get all => backend.threads.of(path)?.threads ?? const [];

  /// The threads to show: open ones, and resolved ones when asked.
  List<Thread> get shown => [
    for (final thread in all)
      if (showResolved || thread.status != ThreadStatus.THREAD_STATUS_RESOLVED)
        thread,
  ];

  /// Threads whose passage isn't in the text shown.
  List<Thread> get detached => [
    for (final thread in shown)
      if (!located.containsKey(thread.id)) thread,
  ];

  int get resolvedCount => all
      .where((thread) => thread.status == ThreadStatus.THREAD_STATUS_RESOLVED)
      .length;

  void _locate() {
    final list = backend.threads.of(path);
    final found = <String, TextRange>{};
    // Positions are for the version they were anchored to; the backend
    // sends them again for the new one after the file changes.
    if (list != null && (list.versionSha256 == _version || _version.isEmpty)) {
      for (final thread in list.threads) {
        final anchor = anchorIn(thread, _kind);
        if (anchor == null) continue;
        final range = locatePassage(_text, anchor);
        if (range != null) found[thread.id] = range;
      }
    }
    located = found;
  }

  /// The agent comments go to unless the person picks another: the one
  /// whose pane shows the file.
  String get defaultAgent =>
      backend.workspace.workspace.selectedAgentId.isNotEmpty
      ? backend.workspace.workspace.selectedAgentId
      : (backend.workspace.workspace.agents.firstOrNull?.id ?? '');

  /// Drafts on this file, by the agent they're addressed to.
  Map<String, int> get drafts {
    final counts = <String, int>{};
    for (final thread in all) {
      if (hasDraft(thread)) {
        counts[thread.agentId] = (counts[thread.agentId] ?? 0) + 1;
      }
    }
    return counts;
  }

  void focus(String? threadId) {
    if (focused == threadId) return;
    focused = threadId;
    final range = threadId == null ? null : located[threadId];
    if (range != null) reveal?.call(range.start);
    notifyListeners();
  }

  /// Starts a comment on the selection, or a suggested change to it.
  void comment({bool suggest = false}) {
    final passage = selection.value;
    if (passage == null || passage.isEmpty) return;
    composing = passage;
    suggesting = suggest;
    replyingTo = null;
    editing = null;
    focused = null;
    sheetOpen = true;
    notifyListeners();
  }

  void cancel() {
    composing = null;
    suggesting = false;
    replyingTo = null;
    editing = null;
    notifyListeners();
  }

  void startReply(String threadId) {
    replyingTo = threadId;
    editing = null;
    composing = null;
    focused = threadId;
    notifyListeners();
  }

  void startEdit(String commentId, String threadId) {
    editing = commentId;
    replyingTo = null;
    composing = null;
    focused = threadId;
    notifyListeners();
  }

  void toggleResolved() {
    showResolved = !showResolved;
    notifyListeners();
  }

  void openSheet(bool open) {
    if (sheetOpen == open) return;
    sheetOpen = open;
    if (!open) cancel();
    notifyListeners();
  }

  Future<bool> _run(Future<void> Function() request) async {
    try {
      await request();
      error = null;
      notifyListeners();
      return true;
    } on RequestFailure catch (failure) {
      error = failure.message;
      notifyListeners();
      return false;
    }
  }

  /// Saves the comment being written as a draft, or sends it at once.
  Future<void> submit(
    String body, {
    required bool sendNow,
    String? replacement,
  }) async {
    final text = body.trim();
    final suggestion = suggesting ? replacement : null;
    if (text.isEmpty && suggestion == null) return;
    final passage = composing;
    final reply = replyingTo;
    final edited = editing;
    final ok = await _run(() async {
      if (edited != null) {
        await backend.editComment(edited, text);
      } else if (reply != null) {
        await backend.replyThread(reply, text, sendNow: sendNow);
      } else if (passage != null) {
        final thread = await backend.createThread(
          CreateThread(
            agentId: defaultAgent,
            path: path,
            kind: passage.kind,
            versionSha256: _version,
            selectors: passage.selectors,
            body: text,
            sendNow: sendNow,
            suggestion: suggestion,
          ),
        );
        focused = thread.id;
      }
    });
    if (ok) {
      composing = null;
      suggesting = false;
      replyingTo = null;
      editing = null;
      selection.value = null;
      notifyListeners();
    }
  }

  Future<void> resolve(Thread thread, {required bool resolved}) =>
      _run(() => backend.setThreadStatus(thread.id, resolved: resolved));

  Future<void> readdress(Thread thread, String agentId) =>
      _run(() => backend.readdressThread(thread.id, agentId));

  Future<void> delete(ThreadComment comment) =>
      _run(() => backend.deleteComment(comment.id));

  /// Accepts [thread]'s suggested change, which Claktory makes in the
  /// file, or rejects it. The person's own, never sent, is taken back
  /// instead: deleting its comments deletes it.
  Future<void> decide(Thread thread, {required bool accept}) {
    final own = thread.suggestion.authorAgentId.isEmpty;
    if (!accept && own && thread.comments.every((c) => c.draft)) {
      return _run(() async {
        for (final comment in thread.comments) {
          await backend.deleteComment(comment.id);
        }
      });
    }
    return _run(() => backend.decideSuggestion(thread.id, accept: accept));
  }

  Future<void> send(String agentId) =>
      _run(() => backend.sendComments(agentId, path));

  /// Anchors a detached thread to the selection instead.
  Future<void> reattach(Thread thread) async {
    final passage = selection.value;
    if (passage == null || passage.isEmpty) return;
    final ok = await _run(
      () => backend.reattachThread(
        ReattachThread(
          threadId: thread.id,
          kind: passage.kind,
          versionSha256: _version,
          selectors: passage.selectors,
        ),
      ),
    );
    if (ok) selection.value = null;
  }
}

/// The highlight of a thread's passage: its agent's color, stronger while
/// it's the one picked.
Color passageColor(String agentColorName, {required bool focused}) =>
    agentColor(agentColorName).withValues(alpha: focused ? 0.40 : 0.20);

/// [span] with the ranges in [marks] (offsets into its plain text, from
/// [base]) painted behind in their colors. The text and its layout don't
/// change, only backgrounds.
TextSpan markSpan(
  TextSpan span,
  List<(int start, int end, Color color)> marks, {
  int base = 0,
}) {
  if (marks.isEmpty) return span;
  var offset = base;
  InlineSpan mark(InlineSpan piece) {
    if (piece is! TextSpan) {
      offset += 1;
      return piece;
    }
    final parts = <InlineSpan>[];
    final text = piece.text;
    if (text != null && text.isNotEmpty) {
      final start = offset;
      final end = offset + text.length;
      // Cut points: every mark edge inside this text.
      final cuts = <int>{start, end};
      for (final (from, to, _) in marks) {
        if (from > start && from < end) cuts.add(from);
        if (to > start && to < end) cuts.add(to);
      }
      final sorted = cuts.toList()..sort();
      for (var i = 0; i + 1 < sorted.length; i++) {
        final from = sorted[i];
        final to = sorted[i + 1];
        Color? color;
        for (final (markFrom, markTo, markColor) in marks) {
          if (markFrom <= from && markTo >= to) {
            color = color == null
                ? markColor
                : Color.alphaBlend(markColor, color);
          }
        }
        final cut = text.substring(from - start, to - start);
        parts.add(
          // A link stays one: the hit test finds the piece, not its parent.
          TextSpan(
            text: cut,
            style: color == null ? null : TextStyle(backgroundColor: color),
            recognizer: piece.recognizer,
            mouseCursor: piece.mouseCursor,
            onEnter: piece.onEnter,
            onExit: piece.onExit,
          ),
        );
      }
      offset = end;
    }
    final children = [
      for (final child in piece.children ?? const []) mark(child),
    ];
    return TextSpan(
      style: piece.style,
      recognizer: piece.recognizer,
      mouseCursor: piece.mouseCursor,
      onEnter: piece.onEnter,
      onExit: piece.onExit,
      semanticsLabel: piece.semanticsLabel,
      children: [...parts, ...children],
    );
  }

  return mark(span) as TextSpan;
}

/// The comments beside a viewer: on a wide pane a margin at its right with
/// each thread's card level with its passage; on a narrow one (and the
/// phone) a sheet from the bottom, opened from the pane's header. [child]
/// is the viewer's body.
class Commented extends StatelessWidget {
  const Commented({super.key, required this.controller, required this.child});

  final CommentsController controller;
  final Widget child;

  /// The margin's width, and how wide a pane must be to have one.
  static const marginWidth = 260.0;
  static const wideFrom = 560.0;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return LayoutBuilder(
      builder: (context, constraints) {
        final wide = constraints.maxWidth >= wideFrom;
        if (controller.narrow.value == wide) {
          // Told after the frame: the header is built already.
          WidgetsBinding.instance.addPostFrameCallback((_) {
            controller.narrow.value = !wide;
            if (wide) controller.openSheet(false);
          });
        }
        if (wide) {
          return Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(child: child),
              Container(
                width: marginWidth,
                decoration: BoxDecoration(
                  color: palette.canvas,
                  border: Border(left: BorderSide(color: palette.hairline)),
                ),
                child: _Margin(controller: controller),
              ),
            ],
          );
        }
        return Stack(
          children: [
            Positioned.fill(child: child),
            Positioned(
              left: 0,
              right: 0,
              bottom: Space.l,
              child: _CommentPill(controller: controller),
            ),
            Positioned.fill(child: _Sheet(controller: controller)),
          ],
        );
      },
    );
  }
}

/// The header's control for the comments on a narrow pane: how many there
/// are; it opens the sheet.
class CommentsButton extends StatelessWidget {
  const CommentsButton({super.key, required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([controller, controller.narrow]),
    builder: (context, _) {
      if (!controller.narrow.value) return const SizedBox.shrink();
      final count = controller.shown.length;
      return GlyphButton(
        label: count == 0 ? 'Comments' : '$count comments',
        active: controller.sheetOpen,
        onTap: () => controller.openSheet(!controller.sheetOpen),
        glyph: (color) => ExtensionGlyph(
          icon: 'message-square',
          name: 'Comments',
          color: color,
        ),
      );
    },
  );
}

/// On a narrow pane, while something's selected: a pill to comment on it.
class _CommentPill extends StatelessWidget {
  const _CommentPill({required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: controller.selection,
    builder: (context, passage, _) {
      if (passage == null || passage.isEmpty || controller.sheetOpen) {
        return const SizedBox.shrink();
      }
      return Center(
        child: _Floating(
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Button(
                label: 'Comment',
                primary: true,
                onPressed: controller.comment,
              ),
              const SizedBox(width: Space.xs),
              Button(
                label: 'Suggest',
                onPressed: () => controller.comment(suggest: true),
              ),
            ],
          ),
        ),
      );
    },
  );
}

class _Floating extends StatelessWidget {
  const _Floating({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      padding: const EdgeInsets.all(Space.xs),
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
        boxShadow: floatingShadow,
      ),
      child: child,
    );
  }
}

/// The wide pane's margin: what's above the cards (detached threads, the
/// resolved ones), the cards level with their passages, and the Send bar.
class _Margin extends StatelessWidget {
  const _Margin({required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: Listenable.merge([
        controller,
        controller.layout,
        controller.selection,
      ]),
      builder: (context, _) {
        final cards = <Widget>[];
        final wanted = <double>[];
        // The person's selection: a button to comment on it, level with it.
        final passage = controller.selection.value;
        final composing = controller.composing;
        if (composing != null) {
          cards.add(_ComposerCard(controller: controller, passage: composing));
          wanted.add(controller.yOf?.call(composing.start) ?? 0);
        } else if (passage != null && !passage.isEmpty) {
          cards.add(
            Align(
              alignment: Alignment.centerLeft,
              child: Unselectable(
                child: Wrap(
                  spacing: Space.xs,
                  children: [
                    Button(label: 'Comment', onPressed: controller.comment),
                    Button(
                      label: 'Suggest',
                      onPressed: () => controller.comment(suggest: true),
                    ),
                  ],
                ),
              ),
            ),
          );
          wanted.add(controller.yOf?.call(passage.start) ?? 0);
        }
        final threads = [...controller.shown]
          ..sort((a, b) {
            final ra = controller.located[a.id];
            final rb = controller.located[b.id];
            if (ra == null || rb == null) return ra == null ? -1 : 1;
            return ra.start.compareTo(rb.start);
          });
        for (final thread in threads) {
          final range = controller.located[thread.id];
          cards.add(_ThreadCard(controller: controller, thread: thread));
          // Detached ones at the top, the rest level with their passages.
          wanted.add(
            range == null ? 0 : controller.yOf?.call(range.start) ?? 0,
          );
        }
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _MarginHeader(controller: controller),
            Expanded(
              child: ClipRect(
                child: CustomMultiChildLayout(
                  delegate: _Cards(wanted),
                  children: [
                    for (var i = 0; i < cards.length; i++)
                      LayoutId(id: i, child: cards[i]),
                  ],
                ),
              ),
            ),
            _SendBar(controller: controller),
          ],
        );
      },
    );
  }
}

/// Cards top to bottom, each level with what it's about if there's room,
/// else just below the one before.
class _Cards extends MultiChildLayoutDelegate {
  _Cards(this.wanted);

  final List<double> wanted;

  @override
  void performLayout(Size size) {
    final order = [for (var i = 0; i < wanted.length; i++) i]
      ..sort((a, b) => wanted[a].compareTo(wanted[b]));
    var next = Space.s;
    for (final i in order) {
      final card = layoutChild(
        i,
        BoxConstraints(maxWidth: size.width - Space.m * 2),
      );
      final y = wanted[i] > next ? wanted[i] : next;
      positionChild(i, Offset(Space.m, y));
      next = y + card.height + Space.s;
    }
  }

  @override
  bool shouldRelayout(_Cards old) => !listEquals(old.wanted, wanted);
}

/// Above the cards, when there's something: how many are resolved, to show
/// them, and why a request failed.
class _MarginHeader extends StatelessWidget {
  const _MarginHeader({required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final resolved = controller.resolvedCount;
    final error = controller.error;
    if (resolved == 0 && error == null) return const SizedBox.shrink();
    return Unselectable(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(Space.m, Space.s, Space.m, 0),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (resolved > 0)
              Pressable(
                onTap: controller.toggleResolved,
                builder: (context, hovered) => Text(
                  controller.showResolved
                      ? 'Hide resolved'
                      : 'Show $resolved resolved',
                  style: TextStyles.caption.copyWith(
                    color: hovered ? palette.text : palette.textMuted,
                  ),
                ),
              ),
            if (error != null)
              Text(
                error,
                style: TextStyles.caption.copyWith(color: palette.danger),
              ),
          ],
        ),
      ),
    );
  }
}

/// At the bottom of the margin (and the sheet) while there are drafts:
/// "Send 3 comments to June", one button for each agent they're to.
class _SendBar extends StatelessWidget {
  const _SendBar({required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final drafts = controller.drafts;
    if (drafts.isEmpty) return const SizedBox.shrink();
    return Container(
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        color: palette.canvas,
        border: Border(top: BorderSide(color: palette.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final MapEntry(key: agentId, value: count) in drafts.entries)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.xs),
              child: Button(
                label:
                    'Send $count ${count == 1 ? 'comment' : 'comments'} to '
                    '${_agentName(controller.backend, agentId)}',
                primary: true,
                onPressed: () => controller.send(agentId),
              ),
            ),
        ],
      ),
    );
  }
}

String _agentName(Backend backend, String agentId) {
  for (final agent in backend.workspace.workspace.agents) {
    if (agent.id == agentId) return agent.name;
  }
  return agentId;
}

Agent? _agent(Backend backend, String agentId) {
  for (final agent in backend.workspace.workspace.agents) {
    if (agent.id == agentId) return agent;
  }
  return null;
}

/// A thread: what it's about, its comments, and what the person can do
/// with it. Picked, it stands out and offers Reply and Resolve.
class _ThreadCard extends StatelessWidget {
  const _ThreadCard({required this.controller, required this.thread});

  final CommentsController controller;
  final Thread thread;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final focused = controller.focused == thread.id;
    final resolved = thread.status == ThreadStatus.THREAD_STATUS_RESOLVED;
    final located = controller.located.containsKey(thread.id);
    final backend = controller.backend;
    final agent = _agent(backend, thread.agentId);
    final caption = TextStyles.caption.copyWith(color: palette.textMuted);
    final faint = TextStyles.caption.copyWith(color: palette.textFaint);
    final anchor = anchorIn(thread, controller.kind);
    final quote = located
        ? (anchor?.exact ?? thread.originalExact)
        : thread.originalExact;
    final suggestion = thread.hasSuggestion() ? thread.suggestion : null;
    final decision = suggestion?.decision;
    final undecided =
        decision == SuggestionDecision.SUGGESTION_DECISION_UNSPECIFIED;
    final lines = controller.kind == 'text' && thread.source.firstLine > 0
        ? thread.source.firstLine == thread.source.lastLine
              ? 'Line ${thread.source.firstLine}'
              : 'Lines ${thread.source.firstLine}–${thread.source.lastLine}'
        : null;
    final now = DateTime.now();
    return Opacity(
      opacity: resolved ? 0.6 : 1,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => controller.focus(focused ? null : thread.id),
        child: AnimatedContainer(
          duration: Motion.fast,
          padding: const EdgeInsets.all(Space.m),
          decoration: BoxDecoration(
            color: palette.canvas,
            borderRadius: BorderRadius.circular(Radii.panel),
            border: Border.all(
              color: focused ? palette.textMuted : palette.hairline,
            ),
            boxShadow: focused ? floatingShadow : null,
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Unselectable(
                child: Row(
                  children: [
                    Dot(color: agentColor(agent?.color ?? ''), size: 6),
                    const SizedBox(width: Space.xs + 2),
                    Expanded(
                      child: Text(
                        'To ${agent?.name ?? thread.agentId}',
                        style: faint,
                      ),
                    ),
                    if (decision ==
                        SuggestionDecision.SUGGESTION_DECISION_ACCEPTED)
                      Text('Accepted', style: faint)
                    else if (decision ==
                        SuggestionDecision.SUGGESTION_DECISION_REJECTED)
                      Text('Rejected', style: faint)
                    else if (resolved)
                      Text('Resolved', style: faint),
                  ],
                ),
              ),
              const SizedBox(height: Space.xs),
              if (!located)
                Text(
                  thread.anchor == AnchorState.ANCHOR_STATE_DETACHED
                      ? 'The text it was on is gone.'
                      : "The text it was on isn't in this version.",
                  style: TextStyles.caption.copyWith(color: palette.danger),
                )
              else if (thread.anchor == AnchorState.ANCHOR_STATE_MOVED)
                Text(
                  suggestion != null && undecided
                      ? 'The text changed after this was suggested.'
                      : 'The text changed since this comment.',
                  style: faint,
                ),
              if (suggestion != null)
                _Suggestion(
                  suggestion: suggestion,
                  lines: lines,
                  before: thread.originalExact.isNotEmpty
                      ? thread.originalExact
                      : quote,
                )
              else if (quote.isNotEmpty)
                Text(
                  [?lines, '“${quote.trim()}”'].join(' · '),
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: caption.copyWith(fontStyle: FontStyle.italic),
                ),
              // A suggestion's own comment may be empty: it only carries
              // the draft.
              for (final comment in thread.comments)
                if (comment.body.isNotEmpty ||
                    controller.editing == comment.id) ...[
                  const SizedBox(height: Space.s),
                  if (controller.editing == comment.id)
                    _Composer(
                      controller: controller,
                      initial: comment.body,
                      agentName: agent?.name ?? '',
                      canSendNow: false,
                    )
                  else
                    _Comment(
                      controller: controller,
                      thread: thread,
                      comment: comment,
                      now: now,
                    ),
                ],
              if (suggestion != null && undecided) ...[
                const SizedBox(height: Space.s),
                _Decide(controller: controller, thread: thread),
              ],
              if (controller.replyingTo == thread.id) ...[
                const SizedBox(height: Space.s),
                _Composer(
                  controller: controller,
                  agentName: agent?.name ?? '',
                  placeholder: 'Reply',
                ),
              ] else if (focused) ...[
                const SizedBox(height: Space.s),
                Unselectable(
                  child: Wrap(
                    spacing: Space.xs,
                    runSpacing: Space.xs,
                    children: [
                      Button(
                        label: 'Reply',
                        onPressed: () => controller.startReply(thread.id),
                      ),
                      Button(
                        label: resolved ? 'Reopen' : 'Resolve',
                        onPressed: () =>
                            controller.resolve(thread, resolved: !resolved),
                      ),
                      if (!located)
                        ValueListenableBuilder(
                          valueListenable: controller.selection,
                          builder: (context, passage, _) => Button(
                            label: 'Move to selected text',
                            onPressed: passage == null || passage.isEmpty
                                ? null
                                : () => controller.reattach(thread),
                          ),
                        ),
                      _Readdress(controller: controller, thread: thread),
                    ],
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// A suggested change: the text as it was, struck through, and what
/// replaces it.
class _Suggestion extends StatelessWidget {
  const _Suggestion({
    required this.suggestion,
    required this.before,
    this.lines,
  });

  final Suggestion suggestion;
  final String before;
  final String? lines;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final caption = TextStyles.caption.copyWith(color: palette.textMuted);
    final lines = this.lines;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text.rich(
          TextSpan(
            children: [
              if (lines != null) TextSpan(text: '$lines · '),
              TextSpan(
                text: before.trim(),
                style: const TextStyle(decoration: TextDecoration.lineThrough),
              ),
            ],
          ),
          maxLines: 3,
          overflow: TextOverflow.ellipsis,
          style: caption,
        ),
        const SizedBox(height: Space.xs),
        if (suggestion.replacement.isEmpty)
          Text(
            'Delete this text',
            style: caption.copyWith(fontStyle: FontStyle.italic),
          )
        else
          Text(
            suggestion.replacement,
            style: TextStyles.body.copyWith(color: palette.text),
          ),
      ],
    );
  }
}

/// A suggestion not decided yet: the agent's to accept or reject, the
/// person's own to apply or take back. Accepting needs the text as it was.
class _Decide extends StatelessWidget {
  const _Decide({required this.controller, required this.thread});

  final CommentsController controller;
  final Thread thread;

  @override
  Widget build(BuildContext context) {
    final own = thread.suggestion.authorAgentId.isEmpty;
    final canAccept =
        thread.anchor == AnchorState.ANCHOR_STATE_ANCHORED &&
        controller.located.containsKey(thread.id);
    return Unselectable(
      child: Wrap(
        spacing: Space.xs,
        runSpacing: Space.xs,
        children: [
          Button(
            label: own ? 'Apply' : 'Accept',
            primary: true,
            onPressed: canAccept
                ? () => controller.decide(thread, accept: true)
                : null,
          ),
          Button(
            label: own ? 'Discard' : 'Reject',
            onPressed: () => controller.decide(thread, accept: false),
          ),
        ],
      ),
    );
  }
}

/// Who the thread is to, to change it: a menu of the agents.
class _Readdress extends StatelessWidget {
  const _Readdress({required this.controller, required this.thread});

  final CommentsController controller;
  final Thread thread;

  @override
  Widget build(BuildContext context) {
    final agents = controller.backend.workspace.workspace.agents;
    if (agents.length < 2) return const SizedBox.shrink();
    return Select<String>(
      value: thread.agentId,
      options: [
        for (final agent in agents) SelectOption(agent.id, 'To ${agent.name}'),
      ],
      onChanged: (agentId) {
        if (agentId != thread.agentId) controller.readdress(thread, agentId);
      },
    );
  }
}

/// One comment: who wrote it and when, a draft's mark, and what it says.
/// The person's own can be edited and deleted.
class _Comment extends StatelessWidget {
  const _Comment({
    required this.controller,
    required this.thread,
    required this.comment,
    required this.now,
  });

  final CommentsController controller;
  final Thread thread;
  final ThreadComment comment;
  final DateTime now;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final mine = comment.authorAgentId.isEmpty;
    final author = mine
        ? 'You'
        : _agentName(controller.backend, comment.authorAgentId);
    final faint = TextStyles.caption.copyWith(color: palette.textFaint);
    final focused = controller.focused == thread.id;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Unselectable(
          child: Row(
            children: [
              Text(
                author,
                style: TextStyles.caption.copyWith(
                  color: palette.text,
                  fontWeight: FontWeight.w500,
                ),
              ),
              const SizedBox(width: Space.xs + 2),
              Expanded(
                child: Text(
                  comment.draft
                      ? 'Draft'
                      : formatAge(comment.createdMs.toInt(), now),
                  style: faint,
                ),
              ),
              if (mine && focused) ...[
                _TextAction(
                  label: 'Edit',
                  onTap: () => controller.startEdit(comment.id, thread.id),
                ),
                const SizedBox(width: Space.s),
                _TextAction(
                  label: 'Delete',
                  onTap: () => controller.delete(comment),
                ),
              ],
            ],
          ),
        ),
        const SizedBox(height: 2),
        Text(
          comment.body,
          style: TextStyles.body.copyWith(color: palette.text),
        ),
      ],
    );
  }
}

class _TextAction extends StatelessWidget {
  const _TextAction({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Pressable(
      onTap: onTap,
      builder: (context, hovered) => Text(
        label,
        style: TextStyles.caption.copyWith(
          color: hovered ? palette.text : palette.textMuted,
        ),
      ),
    );
  }
}

/// A new comment's card, level with its passage.
class _ComposerCard extends StatelessWidget {
  const _ComposerCard({required this.controller, required this.passage});

  final CommentsController controller;
  final Passage passage;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final suggesting = controller.suggesting;
    return Container(
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        color: palette.canvas,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.textMuted),
        boxShadow: floatingShadow,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            suggesting ? passage.exact.trim() : '“${passage.exact.trim()}”',
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: TextStyles.caption.copyWith(
              color: palette.textMuted,
              fontStyle: suggesting ? null : FontStyle.italic,
              decoration: suggesting ? TextDecoration.lineThrough : null,
            ),
          ),
          const SizedBox(height: Space.s),
          _Composer(
            // A new one when it turns from a comment to a suggestion.
            key: ValueKey(suggesting),
            controller: controller,
            agentName: _agentName(controller.backend, controller.defaultAgent),
            replacement: suggesting ? passage.exact : null,
          ),
        ],
      ),
    );
  }
}

/// Where a comment is written: Enter makes a new line, ⌘Enter sends it to
/// the agent at once, Comment keeps it as a draft to send with the rest,
/// Esc gives up. Suggesting a change, the new text comes first, and the
/// comment is optional.
class _Composer extends StatefulWidget {
  const _Composer({
    super.key,
    required this.controller,
    required this.agentName,
    this.initial = '',
    this.placeholder = 'Comment',
    this.canSendNow = true,
    this.replacement,
  });

  final CommentsController controller;
  final String agentName;
  final String initial;
  final String placeholder;

  /// False while editing a comment: an edit is only saved.
  final bool canSendNow;

  /// Suggesting a change: the text it starts from.
  final String? replacement;

  @override
  State<_Composer> createState() => _ComposerState();
}

class _ComposerState extends State<_Composer> {
  late final _text = TextEditingController(text: widget.initial);
  late final _replacement = widget.replacement == null
      ? null
      : TextEditingController(text: widget.replacement);
  var _busy = false;

  @override
  void dispose() {
    _text.dispose();
    _replacement?.dispose();
    super.dispose();
  }

  /// A suggestion needs a change; a comment needs words.
  bool get _ready {
    final replacement = _replacement;
    if (replacement == null) return _text.text.trim().isNotEmpty;
    return replacement.text != widget.replacement;
  }

  Future<void> _submit({required bool sendNow}) async {
    if (_busy || !_ready) return;
    setState(() => _busy = true);
    await widget.controller.submit(
      _text.text,
      sendNow: sendNow,
      replacement: _replacement?.text,
    );
    if (mounted) setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final replacement = _replacement;
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.enter, meta: true): () =>
            _submit(sendNow: widget.canSendNow),
        const SingleActivator(LogicalKeyboardKey.escape):
            widget.controller.cancel,
      },
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (replacement != null) ...[
            TextField(
              controller: replacement,
              placeholder: 'New text',
              minLines: 2,
              maxLines: 8,
              autofocus: true,
            ),
            const SizedBox(height: Space.s),
          ],
          TextField(
            controller: _text,
            placeholder: replacement != null
                ? 'Comment (optional)'
                : widget.placeholder,
            minLines: replacement != null ? 1 : 2,
            maxLines: 8,
            autofocus: replacement == null,
          ),
          const SizedBox(height: Space.s),
          Unselectable(
            child: Wrap(
              alignment: WrapAlignment.end,
              spacing: Space.xs,
              runSpacing: Space.xs,
              children: [
                Button(label: 'Cancel', onPressed: widget.controller.cancel),
                ListenableBuilder(
                  listenable: Listenable.merge([_text, ?replacement]),
                  builder: (context, _) => Button(
                    label: replacement != null
                        ? 'Suggest'
                        : widget.initial.isEmpty
                        ? 'Comment'
                        : 'Save',
                    primary: true,
                    onPressed: _busy || !_ready
                        ? null
                        : () => _submit(sendNow: false),
                  ),
                ),
              ],
            ),
          ),
          if (widget.canSendNow && widget.agentName.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            Text(
              '⌘↩ sends it to ${widget.agentName} now',
              textAlign: TextAlign.right,
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          ],
        ],
      ),
    );
  }
}

/// The narrow pane's comments, from the bottom: the new comment being
/// written, then every thread in the order of its passage, then the Send
/// bar. A tap above it closes it.
class _Sheet extends StatelessWidget {
  const _Sheet({required this.controller});

  final CommentsController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: controller,
      builder: (context, _) {
        if (!controller.sheetOpen) return const SizedBox.shrink();
        final composing = controller.composing;
        final threads = [...controller.shown]
          ..sort((a, b) {
            final ra = controller.located[a.id];
            final rb = controller.located[b.id];
            if (ra == null || rb == null) return ra == null ? -1 : 1;
            return ra.start.compareTo(rb.start);
          });
        return LayoutBuilder(
          builder: (context, constraints) => Stack(
            children: [
              Positioned.fill(
                child: GestureDetector(
                  onTap: () => controller.openSheet(false),
                  child: const ColoredBox(color: Color(0x4D000000)),
                ),
              ),
              Align(
                alignment: Alignment.bottomCenter,
                child: Container(
                  constraints: BoxConstraints(
                    maxHeight: constraints.maxHeight * 0.85,
                  ),
                  decoration: BoxDecoration(
                    color: palette.sidebar,
                    borderRadius: const BorderRadius.vertical(
                      top: Radius.circular(Radii.floating),
                    ),
                    border: Border.all(color: palette.hairline),
                    boxShadow: floatingShadow,
                  ),
                  clipBehavior: Clip.antiAlias,
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      _MarginHeader(controller: controller),
                      Flexible(
                        child: ListView(
                          shrinkWrap: true,
                          padding: const EdgeInsets.all(Space.m),
                          children: [
                            if (composing != null) ...[
                              _ComposerCard(
                                controller: controller,
                                passage: composing,
                              ),
                              const SizedBox(height: Space.s),
                            ],
                            if (composing == null && threads.isEmpty)
                              Padding(
                                padding: const EdgeInsets.all(Space.m),
                                child: Text(
                                  'Select text to comment on it.',
                                  textAlign: TextAlign.center,
                                  style: TextStyles.body.copyWith(
                                    color: palette.textMuted,
                                  ),
                                ),
                              ),
                            for (final thread in threads) ...[
                              _ThreadCard(
                                controller: controller,
                                thread: thread,
                              ),
                              const SizedBox(height: Space.s),
                            ],
                          ],
                        ),
                      ),
                      _SendBar(controller: controller),
                    ],
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}
