// Answering a review where the file is (Matt's ask, after #275): while an
// agent's review ask waits on the file a viewer pane shows, a bar at the
// pane's foot (expanded or not) says who asks and what, with the answer's
// field: Return sends it (Approve, or the written answer when the ask
// wants one), Dismiss is quiet beside it. Nothing else is added to the
// file meanwhile: on media the field is the only input; on text, line
// comments stay and go with the answer. Answered, the pane closes and the
// person is back where they pressed Review. The same model as the iPhone's
// (mobile/review_bar.dart) and the iPad's PDFs (ipad/document_review.dart),
// in Trish's words. Nothing is kept: the ask is the agent's conversation's,
// fetched as it waits; where to go back to is navigation, in memory.

import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'dictation.dart';
import 'identity.dart';
import 'layout.dart';
import 'markdown.dart';
import 'mobile/metrics.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'visible_codes.dart' show proseCodes;

/// Whether a review ask waits on the file below: viewers then offer
/// nothing that doesn't answer it (no Comment on media). Set by the bars
/// that answer it, on every device.
class ReviewAnswering extends InheritedWidget {
  const ReviewAnswering({
    super.key,
    required this.answering,
    required super.child,
  });

  final bool answering;

  static bool of(BuildContext context) =>
      context
          .dependOnInheritedWidgetOfExactType<ReviewAnswering>()
          ?.answering ??
      false;

  @override
  bool updateShouldNotify(ReviewAnswering old) => old.answering != answering;
}

/// An agent's review ask waiting on a file: its entry's seq and the ask.
typedef PendingReview = ({int seq, AskPerson ask});

/// Where the person was when they pressed Review: the page and the agent
/// shown, to go back to once it's answered.
typedef _Return = ({
  String agentId,
  String path,
  AppPage page,
  String selected,
});

_Return? _return;

/// Notes where the person is as they open [path] to review it for
/// [agentId] (the ask card's Review), so answering it brings them back.
void rememberReviewReturn(BuildContext context, String agentId, String path) {
  final scope = AppScope.scope(context);
  _return = (
    agentId: agentId,
    path: path,
    page: scope.page.value,
    selected: scope.backend.workspace.workspace.selectedAgentId,
  );
}

/// After [agentId]'s review of [path] is answered: its pane closes (and
/// with it the expanded view), and the person is back where they pressed
/// Review, when they did.
Future<void> finishReview(
  BuildContext context,
  String agentId,
  String path,
) async {
  final scope = AppScope.scope(context);
  final backend = scope.backend;
  final back = _return;
  _return = null;
  final agent = backend.workspace.agent(agentId);
  if (agent != null) {
    final root = layoutOf(agent);
    var tree = root;
    for (final leaf in leaves(root)) {
      final kind = leaf.leaf.kind;
      if (leaf.leaf.path == path &&
          kind != chatKind &&
          kind != filesKind &&
          kind != 'app') {
        tree = close(tree, leaf.id) ?? tree;
      }
    }
    if (tree != root || agent.layoutFocus.isNotEmpty) {
      await backend.setLayout(
        agentId,
        isChatOnly(tree) ? null : tree,
        focus: '',
      );
    }
  }
  if (back == null || back.agentId != agentId || back.path != path) return;
  if (back.selected.isNotEmpty &&
      back.selected != backend.workspace.workspace.selectedAgentId &&
      backend.workspace.agent(back.selected) != null) {
    await backend.selectAgent(back.selected);
  }
  if (back.page != AppPage.agent) scope.page.value = back.page;
}

/// The oldest of [agentId]'s asks waiting on [path], as the agent's
/// waiting entries are fetched: null while none is.
class ReviewWatch extends ChangeNotifier {
  ReviewWatch(this.backend, this.agentId, this.path) {
    backend.workspace.addListener(_changed);
    _fetch();
  }

  final Backend backend;
  final String agentId;
  final String path;

  final _entries = <int, Entry>{};
  final _fetching = <int>{};
  var _disposed = false;

  List<int> get _waiting => [
    for (final seq in backend.workspace.agent(agentId)?.waitingSeqs ?? [])
      seq.toInt(),
  ];

  PendingReview? get pending {
    for (final seq in _waiting) {
      final entry = _entries[seq];
      if (entry == null || entry.whichKind() != Entry_Kind.askPerson) {
        continue;
      }
      final ask = entry.askPerson;
      if (ask.reviewPath == path &&
          ask.state == AskPersonState.ASK_PERSON_STATE_PENDING) {
        return (seq: seq, ask: ask);
      }
    }
    return null;
  }

  void _changed() {
    if (_disposed) return;
    notifyListeners();
    _fetch();
  }

  void _fetch() {
    for (final seq in _waiting) {
      if (_entries.containsKey(seq) || !_fetching.add(seq)) continue;
      unawaited(() async {
        try {
          final entry = await backend.fetchEntry(agentId, seq);
          if (entry != null && !_disposed) {
            _entries[seq] = entry;
            notifyListeners();
          }
        } on RequestFailure {
          // Asked again when the workspace next changes.
        } finally {
          _fetching.remove(seq);
        }
      }());
    }
  }

  @override
  void dispose() {
    _disposed = true;
    backend.workspace.removeListener(_changed);
    super.dispose();
  }
}

/// A viewer pane's file, for [agentId]: as it is, or while the agent's
/// review ask waits on [path], with the bar that answers it at its foot
/// and [ReviewAnswering] over it.
class ReviewFrame extends StatefulWidget {
  const ReviewFrame({
    super.key,
    required this.agentId,
    required this.path,
    required this.child,
  });

  final String agentId;
  final String path;
  final Widget child;

  @override
  State<ReviewFrame> createState() => _ReviewFrameState();
}

class _ReviewFrameState extends State<ReviewFrame> {
  ReviewWatch? _watch;

  /// The child keeps its state as the bar comes and goes.
  final _childKey = GlobalKey();

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _rewatch();
  }

  @override
  void didUpdateWidget(ReviewFrame old) {
    super.didUpdateWidget(old);
    if (old.agentId != widget.agentId || old.path != widget.path) {
      _rewatch(force: true);
    }
  }

  void _rewatch({bool force = false}) {
    final backend = AppScope.of(context);
    if (!force && _watch?.backend == backend) return;
    _watch?.dispose();
    _watch = ReviewWatch(backend, widget.agentId, widget.path);
  }

  @override
  void dispose() {
    _watch?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: _watch!,
    builder: (context, _) {
      final pending = _watch!.pending;
      final child = KeyedSubtree(key: _childKey, child: widget.child);
      if (pending == null) return child;
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(child: ReviewAnswering(answering: true, child: child)),
          ReviewBar(
            key: ValueKey(pending.seq),
            agentId: widget.agentId,
            path: widget.path,
            review: pending,
          ),
        ],
      );
    },
  );
}

/// Whether [path] is a text file's, whose line comments go with the
/// answer (rather than media, where the field is all there is).
bool reviewHasLineComments(String path) {
  final dot = path.lastIndexOf('.');
  final ext = dot < 0 ? '' : path.substring(dot + 1).toLowerCase();
  return !const {
    'png', 'jpg', 'jpeg', 'gif', 'webp', 'heic', 'bmp', 'tif', 'tiff', //
    'svg', 'mp4', 'mov', 'm4v', 'webm', 'pdf',
  }.contains(ext);
}

/// At a viewer's foot while [review] waits: the agent and its question,
/// the answer's field (Return sends it; Shift-Return a new line), Dismiss
/// and Approve, or Send once something's typed (Send only, when the ask
/// wants words). Answered, [finishReview].
class ReviewBar extends StatefulWidget {
  const ReviewBar({
    super.key,
    required this.agentId,
    required this.path,
    required this.review,
  });

  final String agentId;
  final String path;
  final PendingReview review;

  @override
  State<ReviewBar> createState() => _ReviewBarState();
}

class _ReviewBarState extends State<ReviewBar> {
  final _text = TextEditingController();
  final _focus = FocusNode(debugLabel: 'Review answer');
  var _busy = false;
  var _failed = false;

  @override
  void dispose() {
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  bool get _wantsText => widget.review.ask.wantsText;

  Future<void> _answer({required bool dismiss}) async {
    final text = _text.text.trim();
    if (_busy || (!dismiss && _wantsText && text.isEmpty)) return;
    setState(() {
      _busy = true;
      _failed = false;
    });
    try {
      await AppScope.of(context).answerAsk(
        widget.agentId,
        widget.review.seq,
        text: text,
        dismiss: dismiss,
      );
      if (!mounted) return;
      await finishReview(context, widget.agentId, widget.path);
    } on RequestFailure {
      if (mounted) setState(() => _failed = true);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// Return sends what's typed: an empty field never approves by itself.
  void _return() {
    if (_text.text.trim().isEmpty) return;
    unawaited(_answer(dismiss: false));
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final agent = AppScope.of(context).workspace.agent(widget.agentId);
    final name = agent?.name ?? 'The agent';
    final color = agentColor(agent?.color ?? 'slate');
    final ask = widget.review.ask;
    final touch = touchFirst(context);
    return Container(
      padding: EdgeInsets.fromLTRB(
        Space.l,
        Space.m,
        Space.l,
        Space.m + MediaQuery.paddingOf(context).bottom,
      ),
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border(top: BorderSide(color: palette.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            children: [
              if (agent != null) ...[
                AgentCharacter(agent: agent, size: 20),
                const SizedBox(width: Space.xs + 2),
              ],
              Flexible(
                child: Text(
                  '$name asks you to review this',
                  style: type.caption.copyWith(
                    color: color,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.xs),
          // The question; a long one scrolls in its own little room so
          // the file keeps its.
          ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 120),
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  // The agent's words: what reorders them shows by its code
                  // (proseCodes, Fugo).
                  Text(
                    proseCodes(ask.title),
                    style: type.emphasis.copyWith(color: palette.text),
                  ),
                  if (ask.body.isNotEmpty) ...[
                    const SizedBox(height: 2),
                    Markdown(proseCodes(ask.body), color: palette.textMuted),
                  ],
                ],
              ),
            ),
          ),
          const SizedBox(height: Space.s),
          // Typed or said: the mic puts what's said in, to check before
          // sending; Dismiss and Send step aside while it records.
          Dictation(
            controller: _text,
            focusNode: _focus,
            enabled: !_busy,
            trailing: Row(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                _Sized(
                  touch: touch,
                  child: Button(
                    label: 'Dismiss',
                    onPressed: _busy ? null : () => _answer(dismiss: true),
                  ),
                ),
                const SizedBox(width: Space.xs),
                ListenableBuilder(
                  listenable: _text,
                  builder: (context, _) {
                    final empty = _text.text.trim().isEmpty;
                    return _Sized(
                      touch: touch,
                      child: Button(
                        label: empty && !_wantsText ? 'Approve' : 'Send',
                        primary: true,
                        onPressed: _busy || (_wantsText && empty)
                            ? null
                            : () => _answer(dismiss: false),
                      ),
                    );
                  },
                ),
              ],
            ),
            child: CallbackShortcuts(
              bindings: {
                const SingleActivator(LogicalKeyboardKey.enter): _return,
                const SingleActivator(LogicalKeyboardKey.numpadEnter): _return,
              },
              child: Semantics(
                hint: 'Return sends it to $name.',
                child: TextField(
                  controller: _text,
                  focusNode: _focus,
                  placeholder: 'Your answer to $name',
                  minLines: 1,
                  maxLines: 6,
                  textInputAction: TextInputAction.send,
                  onSubmitted: (_) => _return(),
                  semanticsLabel: 'Your answer to $name',
                ),
              ),
            ),
          ),
          if (_failed || reviewHasLineComments(widget.path)) ...[
            const SizedBox(height: Space.xs),
            Text(
              _failed
                  ? "Couldn't send your answer. Try again."
                  : 'Your comments on lines go to $name with your answer.',
              style: type.caption.copyWith(
                color: _failed ? palette.danger : palette.textFaint,
              ),
            ),
          ],
        ],
      ),
    );
  }
}

/// A button at touch size on a touch-first device (44 pt high).
class _Sized extends StatelessWidget {
  const _Sized({required this.touch, required this.child});

  final bool touch;
  final Widget child;

  @override
  Widget build(BuildContext context) => touch
      ? ConstrainedBox(
          constraints: const BoxConstraints(minHeight: Touch.target),
          child: Center(widthFactor: 1, child: child),
        )
      : child;
}
