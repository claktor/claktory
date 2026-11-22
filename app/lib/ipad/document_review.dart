// A review an agent asks of a PDF, answered on an iPad with what the
// person drew on it (#275; docs/ipad-workbench.md §6): at the foot of the
// document's pane while the question waits, who asks and what, the pages
// drawn on, then Dismiss and Approve. Either answer takes the drawings
// with it: each page with ink, whole, as a PNG with the ink on it,
// uploaded to the agent and named in the answer (AnswerAsk.attachments),
// so they reach the agent only when the person answers.
//
// Only a backend that takes the drawings accepts a PDF for review, so a
// question waiting on a PDF says the answer can carry them.
import 'dart:async';
import 'dart:io' show File;
import 'dart:math' show Random, min;

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../characters/character.dart';
import '../controls.dart';
import '../decision.dart' show DecisionComment, decisionCommentLimit;
import '../identity.dart';
import '../mobile/metrics.dart' show Metrics, TypeScale;
import '../mobile/review_bar.dart' show ReviewAsk;
import '../protocol/factory/v1/factory.pb.dart';
import 'document_host.dart' show DocumentSurface;

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
  ReviewAsk? get ask {
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

  /// The person's comment, sent with either answer (Matt's rule: approve
  /// or dismiss, always with an optional comment).
  final _comment = TextEditingController();

  @override
  void dispose() {
    _comment.dispose();
    super.dispose();
  }

  /// The comment field, made again (open) once handwriting fills it.
  var _commentKey = 0;

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
        _commentKey++;
      });
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

  Future<bool> _answer(ReviewAsk ask, {bool dismiss = false}) async {
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
      return true;
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
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

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([widget.watch, widget.surface]),
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    final ask = widget.watch.ask;
    final agent = widget.watch.backend?.workspace.agent(widget.watch.agentId);
    if (ask == null || agent == null) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final drawn = widget.surface.drawnPages.length;
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
            if (ask.ask.title.isNotEmpty) ...[
              const SizedBox(height: Space.xs),
              Text(
                ask.ask.title,
                maxLines: 3,
                overflow: TextOverflow.ellipsis,
                style: type.body.copyWith(color: palette.text),
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
            if (_error case final error?) ...[
              const SizedBox(height: Space.xs),
              Text(
                error,
                style: type.secondary.copyWith(color: palette.danger),
              ),
            ],
            const SizedBox(height: Space.s),
            // The answers at the end, as a pane's other actions, under
            // each other when the pane is narrow or the text large; the
            // comment under them, as on every card (decision.dart).
            Align(
              alignment: AlignmentDirectional.centerEnd,
              child: Wrap(
                spacing: Space.s,
                runSpacing: Space.s,
                children: [
                  Button(
                    label: 'Dismiss',
                    onPressed: _busy
                        ? null
                        : () => unawaited(_answer(ask, dismiss: true)),
                  ),
                  Button(
                    label: 'Approve',
                    primary: true,
                    onPressed: _busy ? null : () => unawaited(_answer(ask)),
                  ),
                ],
              ),
            ),
            const SizedBox(height: Space.xs),
            DecisionComment(
              key: ValueKey(_commentKey),
              controller: _comment,
              agent: agent.name,
              enabled: !_busy,
              onSubmitted: _busy ? null : () => unawaited(_answer(ask)),
            ),
            ..._handwriting(context),
          ],
        ),
      ),
    );
  }
}

extension on _DocumentReviewBarState {
  /// The offer to turn what's handwritten into the comment, on an iPad
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
                          ? 'Turn selected handwriting into a comment'
                          : 'Turn handwriting into a comment',
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
