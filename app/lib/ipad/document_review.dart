// A review an agent asks of a PDF, answered on an iPad right where it's
// read, with what the person drew on it (#275; docs/ipad-workbench.md §6):
// at the foot of the document's pane while the question waits, who asks
// and what, the pages drawn on, and the answer field, always open. Return
// (or Approve, or Send) answers, Dismiss is quiet beside it; either takes
// the field's words and the drawings with it: each page with ink, whole,
// as a PNG with the ink on it, uploaded to the agent and named in the
// answer (AnswerAsk.attachments), so they reach the agent only when the
// person answers. Answered, the pane closes and the person is back where
// they were. Nothing here is a comment of its own.
//
// Only a backend that takes the drawings accepts a PDF for review, so a
// question waiting on a PDF says the answer can carry them.
import 'dart:async';
import 'dart:io' show File;
import 'dart:math' show Random, min;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../characters/character.dart';
import '../controls.dart';
import '../decision.dart' show decisionCommentLimit;
import '../identity.dart';
import '../markdown.dart' show Markdown;
import '../mobile/metrics.dart' show Metrics, TypeScale;
import '../protocol/factory/v1/factory.pb.dart';
import '../dictation.dart' show Dictation;
import '../review_answer.dart' show PendingReview, finishReview;
import 'document_host.dart' show DocumentSurface;
import '../visible_codes.dart' show proseCodes;

/// Reads a drawn page's composite, as native wrote it (tests replace it).
@visibleForTesting
Future<Uint8List> Function(String file) readComposite = (file) =>
    File(file).readAsBytes();

/// An upload's chunk: the backend takes at most 1 MiB at a time.
const _chunk = 1024 * 1024;

/// The question waiting on a file: [agentId]'s oldest pending review of
/// [path], fetched from its waiting entries as the workspace changes.
class ReviewWatch extends ChangeNotifier {
  ReviewWatch({required this.agentId, required this.path});

  final String agentId;
  final String path;

  Backend? _backend;

  /// The agent's waiting entries fetched so far, by seq.
  final _entries = <int, Entry>{};
  final _fetching = <int>{};
  var _disposed = false;

  /// Follows [backend]'s workspace (the window's; again when it changes).
  void follow(Backend backend) {
    if (backend == _backend) return;
    _backend?.workspace.removeListener(_changed);
    _backend = backend..workspace.addListener(_changed);
    _entries.clear();
    _fetching.clear();
    _fetch();
  }

  Backend? get backend => _backend;

  void _changed() {
    if (_disposed) return;
    notifyListeners();
    _fetch();
  }

  List<int> get _waiting => [
    for (final seq
        in _backend?.workspace.agent(agentId)?.waitingSeqs ?? const [])
      seq.toInt(),
  ];

  /// Fetches the waiting entries not seen yet: one may be the question
  /// about this file.
  void _fetch() {
    final backend = _backend;
    if (backend == null) return;
    for (final seq in _waiting) {
      if (_entries.containsKey(seq) || !_fetching.add(seq)) continue;
      unawaited(() async {
        try {
          final entry = await backend.fetchEntry(agentId, seq);
          if (entry != null && !_disposed && backend == _backend) {
            _entries[seq] = entry;
            notifyListeners();
          }
        } on RequestFailure {
          // Tried again when the workspace next changes.
        } finally {
          _fetching.remove(seq);
        }
      }());
    }
  }

  /// The oldest question still waiting on this file.
  PendingReview? get ask {
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

  @override
  void dispose() {
    _disposed = true;
    _backend?.workspace.removeListener(_changed);
    super.dispose();
  }
}

/// The bar at the foot of [surface]'s pane while [watch] has a question
/// waiting on its file. Nothing shows when none waits.
class DocumentReviewBar extends StatefulWidget {
  const DocumentReviewBar({
    super.key,
    required this.watch,
    required this.surface,
  });

  final ReviewWatch watch;
  final DocumentSurface surface;

  @override
  State<DocumentReviewBar> createState() => _DocumentReviewBarState();
}

class _DocumentReviewBarState extends State<DocumentReviewBar> {
  var _busy = false;

  /// The person's answer, sent with either button (Matt's rule: approve
  /// or dismiss, always with an optional comment); the answer itself when
  /// the agent wants words.
  final _comment = TextEditingController();
  final _field = FocusNode(debugLabel: 'review answer');

  @override
  void initState() {
    super.initState();
    // The button's label follows what's typed.
    _comment.addListener(_typed);
  }

  /// The ask whose kept answer the field was given back
  /// ([DocumentSurface.answers]): what's typed is kept for it from then.
  int? _restored;

  void _typed() {
    final ask = widget.watch.ask;
    if (ask != null && ask.seq == _restored) {
      final text = _comment.text;
      if (text.trim().isEmpty) {
        widget.surface.answers.remove(ask.seq);
      } else {
        widget.surface.answers[ask.seq] = text;
      }
    }
    setState(() {});
  }

  /// The answer typed to [seq] before the pane went (another agent shown
  /// and back), given back to the field once.
  void _restore(int seq) {
    if (_restored == seq) return;
    _restored = seq;
    final kept = widget.surface.answers[seq];
    if (kept == null || _comment.text.isNotEmpty) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _comment.text.isEmpty) _comment.text = kept;
    });
  }

  @override
  void dispose() {
    _comment
      ..removeListener(_typed)
      ..dispose();
    _field.dispose();
    super.dispose();
  }

  /// Handwriting being read, and what came of the last reading when it
  /// gave no text.
  var _reading = false;
  ({String text, bool failed, int at})? _readNote;

  /// Reads the handwriting into the comment, after what's there, for the
  /// person to change before answering: never sent by itself. An answer
  /// the ink changed under is dropped without a word.
  Future<void> _readHandwriting() async {
    if (_reading || _busy) return;
    setState(() {
      _reading = true;
      _readNote = null;
    });
    try {
      final text = await widget.surface.readHandwriting();
      if (!mounted || text == null) return;
      if (text.isEmpty) {
        setState(
          () => _readNote = (
            text: 'No handwriting to read.',
            failed: false,
            at: widget.surface.inkSeq,
          ),
        );
        return;
      }
      final before = _comment.text.trimRight();
      var filled = before.isEmpty ? text : '$before\n$text';
      if (filled.length > decisionCommentLimit) {
        filled = filled.substring(0, decisionCommentLimit);
      }
      setState(() {
        _comment.value = TextEditingValue(
          text: filled,
          selection: TextSelection.collapsed(offset: filled.length),
        );
      });
      _field.requestFocus();
    } on Object {
      if (mounted) {
        setState(
          () => _readNote = (
            text: "Couldn't read your handwriting. Try again.",
            failed: true,
            at: widget.surface.inkSeq,
          ),
        );
      }
    } finally {
      if (mounted) setState(() => _reading = false);
    }
  }

  /// Drawn pages going up, while they go: how many are there.
  ({int sent, int of})? _sending;
  String? _error;

  /// Uploads each drawn page to the agent: what to name in the answer.
  Future<List<Attachment>> _drawings(Backend backend, int seq) async {
    final surface = widget.surface;
    final pages = surface.drawnPages;
    final attachments = <Attachment>[];
    final stem = _stem(surface.path);
    final tag = Random().nextInt(1 << 32).toRadixString(36);
    for (final page in pages) {
      if (mounted) {
        setState(() => _sending = (sent: attachments.length, of: pages.length));
      }
      final composite = await surface.exportPage(page);
      final bytes = await readComposite(composite.file);
      final id = 'review-$seq-p$page-$tag';
      var received = await backend.startUpload(
        id,
        widget.watch.agentId,
        name: '$stem-p$page.png',
        size: bytes.length,
        mime: 'image/png',
      );
      try {
        while (received < bytes.length) {
          received = await backend.uploadChunk(
            id,
            received,
            bytes.sublist(received, min(received + _chunk, bytes.length)),
          );
        }
        attachments.add(await backend.finishUpload(id));
      } catch (_) {
        unawaited(backend.cancelUpload(id).catchError((Object _) {}));
        rethrow;
      }
    }
    return attachments;
  }

  Future<bool> _answer(PendingReview ask, {bool dismiss = false}) async {
    final backend = widget.watch.backend;
    if (_busy || backend == null) return false;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final attachments = await _drawings(backend, ask.seq);
      await backend.answerAsk(
        widget.watch.agentId,
        ask.seq,
        text: _comment.text.trim(),
        dismiss: dismiss,
        attachments: attachments,
      );
      _comment.clear();
      // Answered: the file's pane closes and the person is back where
      // they pressed Review.
      if (mounted) {
        unawaited(
          finishReview(
            context,
            widget.watch.agentId,
            widget.watch.path,
          ).catchError((Object _) {}),
        );
      }
      return true;
    } on RequestFailure {
      if (mounted) {
        setState(() => _error = "Couldn't send your answer. Try again.");
      }
      return false;
    } on Object {
      // The surface couldn't draw a page out (it went, or native failed).
      if (mounted) {
        setState(
          () => _error =
              "Couldn't send your drawings, so your answer wasn't sent. "
              'Try again.',
        );
      }
      return false;
    } finally {
      if (mounted) {
        setState(() {
          _busy = false;
          _sending = null;
        });
      }
    }
  }

  /// Return in the field: answers with what's typed; nothing when it's
  /// empty, so a stray Return never approves.
  void _submit(PendingReview ask) {
    if (_busy || _comment.text.trim().isEmpty) return;
    unawaited(_answer(ask));
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([widget.watch, widget.surface]),
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    final ask = widget.watch.ask;
    final agent = widget.watch.backend?.workspace.agent(widget.watch.agentId);
    if (ask == null || agent == null) return const SizedBox.shrink();
    _restore(ask.seq);
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final drawn = widget.surface.drawnPages.length;
    final typed = _comment.text.trim().isNotEmpty;
    final sending = _sending;
    final String? line = sending != null
        ? (sending.of == 1
              ? 'Sending your drawing…'
              : 'Sending your drawings… ${sending.sent + 1} of ${sending.of}')
        : switch (drawn) {
            0 => null,
            1 => 'Your drawing on 1 page goes with your answer.',
            _ => 'Your drawings on $drawn pages go with your answer.',
          };
    return Semantics(
      container: true,
      explicitChildNodes: true,
      child: Container(
        key: const ValueKey('document-review'),
        decoration: BoxDecoration(
          color: palette.canvas,
          border: Border(top: BorderSide(color: palette.hairline)),
        ),
        padding: const EdgeInsets.symmetric(
          horizontal: Space.l,
          vertical: Space.s,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                ExcludeSemantics(child: AgentCharacter(agent: agent, size: 20)),
                const SizedBox(width: Space.s),
                Expanded(
                  child: Text(
                    '${agent.name} asks you to review this',
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: type.secondary.copyWith(color: palette.textMuted),
                  ),
                ),
              ],
            ),
            if (ask.ask.title.isNotEmpty || ask.ask.body.isNotEmpty) ...[
              const SizedBox(height: Space.xs),
              // The question; a long one scrolls in its own little room,
              // as on the Mac (review_answer.dart).
              ConstrainedBox(
                constraints: const BoxConstraints(maxHeight: 120),
                child: SingleChildScrollView(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      if (ask.ask.title.isNotEmpty)
                        // The agent's words: what reorders them shows by
                        // its code (proseCodes, Fugo).
                        Text(
                          proseCodes(ask.ask.title),
                          style: type.body.copyWith(color: palette.text),
                        ),
                      if (ask.ask.body.isNotEmpty) ...[
                        const SizedBox(height: 2),
                        Markdown(
                          proseCodes(ask.ask.body),
                          color: palette.textMuted,
                        ),
                      ],
                    ],
                  ),
                ),
              ),
            ],
            if (line != null) ...[
              const SizedBox(height: Space.xs),
              Semantics(
                liveRegion: sending != null,
                child: Text(
                  line,
                  style: type.secondary.copyWith(color: palette.textMuted),
                ),
              ),
            ],
            const SizedBox(height: Space.s),
            // The answer, always open: Return sends what's typed, Shift
            // and Return starts a new line on a hardware keyboard. Or said:
            // the mic at its end puts what's said in, to check first.
            Dictation(
              controller: _comment,
              focusNode: _field,
              enabled: !_busy,
              child: CallbackShortcuts(
                bindings: {
                  const SingleActivator(LogicalKeyboardKey.enter): () =>
                      _submit(ask),
                },
                child: Semantics(
                  hint: 'Return sends it to ${agent.name}.',
                  child: TextField(
                    key: const ValueKey('review-answer'),
                    controller: _comment,
                    focusNode: _field,
                    placeholder: 'Your answer to ${agent.name}',
                    semanticsLabel: 'Your answer to ${agent.name}',
                    minLines: 1,
                    maxLines: 6,
                    textInputAction: TextInputAction.send,
                    onSubmitted: (_) => _submit(ask),
                    inputFormatters: [
                      LengthLimitingTextInputFormatter(decisionCommentLimit),
                    ],
                  ),
                ),
              ),
            ),
            const SizedBox(height: Space.xs),
            Text(
              '${agent.name} gets it with your answer.',
              style: type.caption.copyWith(color: palette.textFaint),
            ),
            if (_error case final error?) ...[
              const SizedBox(height: Space.xs),
              Text(
                error,
                style: type.secondary.copyWith(color: palette.danger),
              ),
            ],
            const SizedBox(height: Space.s),
            // The answer at the end, as a pane's other actions; Dismiss
            // quiet before it. Under each other when the pane is narrow
            // or the text large.
            Align(
              alignment: AlignmentDirectional.centerEnd,
              child: Wrap(
                spacing: Space.s,
                runSpacing: Space.s,
                children: [
                  Button(
                    key: const ValueKey('review-dismiss'),
                    label: 'Dismiss',
                    onPressed: _busy
                        ? null
                        : () => unawaited(_answer(ask, dismiss: true)),
                  ),
                  Button(
                    key: const ValueKey('review-send'),
                    // No Approve when the agent wants words: they're the answer.
                    label: typed || ask.ask.wantsText ? 'Send' : 'Approve',
                    primary: true,
                    // Words are the answer when the agent wants them.
                    onPressed: _busy || (ask.ask.wantsText && !typed)
                        ? null
                        : () => unawaited(_answer(ask)),
                  ),
                ],
              ),
            ),
            ..._handwriting(context),
          ],
        ),
      ),
    );
  }
}

extension on _DocumentReviewBarState {
  /// The offer to turn what's handwritten into the answer, on an iPad
  /// that reads handwriting (iPadOS 27 and later) with something drawn or
  /// chosen with the lasso; nothing at all on one that doesn't.
  List<Widget> _handwriting(BuildContext context) {
    final surface = widget.surface;
    final selected = surface.strokeSelection != null;
    if (!surface.handwriting.available ||
        (!selected && surface.inkedPages.isEmpty && !_reading)) {
      return const [];
    }
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    // What the last reading said, until the ink changes.
    final note = _readNote?.at == surface.inkSeq ? _readNote : null;
    return [
      if (_reading)
        ConstrainedBox(
          constraints: BoxConstraints(minHeight: Metrics.of(context).target),
          child: Align(
            alignment: AlignmentDirectional.centerStart,
            child: Semantics(
              liveRegion: true,
              child: Text(
                'Reading your handwriting…',
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            ),
          ),
        )
      else
        Align(
          alignment: AlignmentDirectional.centerStart,
          child: Semantics(
            button: true,
            child: Pressable(
              key: const ValueKey('read-handwriting'),
              onTap: _busy ? null : () => unawaited(_readHandwriting()),
              builder: (context, hovered) => ConstrainedBox(
                constraints: BoxConstraints(
                  minHeight: Metrics.of(context).target,
                ),
                child: Padding(
                  padding: const EdgeInsets.symmetric(vertical: Space.xs),
                  child: Align(
                    alignment: AlignmentDirectional.centerStart,
                    widthFactor: 1,
                    heightFactor: 1,
                    child: Text(
                      selected
                          ? 'Turn selected handwriting into your answer'
                          : 'Turn handwriting into your answer',
                      style: type.caption.copyWith(
                        color: hovered ? palette.text : palette.textMuted,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      if (note != null)
        Semantics(
          liveRegion: true,
          child: Text(
            note.text,
            style: type.caption.copyWith(
              color: note.failed ? palette.danger : palette.textMuted,
            ),
          ),
        ),
    ];
  }
}

/// "paper" for /…/paper.pdf: the drawn pages are named after it.
String _stem(String path) {
  final name = path.split('/').last;
  final dot = name.lastIndexOf('.');
  final stem = dot > 0 ? name.substring(0, dot) : name;
  return stem.isEmpty ? 'document' : stem;
}
