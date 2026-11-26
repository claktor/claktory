// A review an agent asks of a PDF, answered on an iPad right where it's
// read, with what the person drew on it (#275; docs/ipad-workbench.md §6):
// at the foot of the document's pane while the question waits, who asks
// and what, the pages drawn on, and the answer field, always open. With
// nothing typed, Approve (or Send, for a question that wants words), and
// Dismiss quiet beside it; once a comment is typed, one Submit (Return
// too) sends it as a reply, never a yes or a no (#370). Each takes the
// drawings with it: each page with ink, whole,
// as a PNG with the ink on it, uploaded to the agent and named in the
// answer (AnswerAsk.attachments), so they reach the agent only when the
// person answers. Answered, the pane closes and the person is back where
// they were. Nothing here is a comment of its own. In Needs you's focus
// mode (#514) the bar is the stage's answer for a PDF or a picture: its
// answer goes in the stage's dock, ⌘↩, ⌘⌫ and its buttons go through the
// focus session, and answered, nothing closes; the next card comes in.
//
// Only a backend that takes the drawings accepts a PDF for review, so a
// question waiting on a PDF says the answer can carry them.
import 'dart:async';
import 'dart:io' show File, FileSystemException;
import 'dart:math' show Random, min;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../card_files.dart';
import '../card_answer.dart'
    show AnswerDock, CardAnswer, drawingGoesLine, insteadLine, replyPlaceholder;
import '../characters/character.dart';
import '../controls.dart';
import '../decision.dart' show SubmitButton, commentTyped, decisionCommentLimit;
import '../identity.dart';
import '../markdown.dart' show Markdown;
import '../mobile/lines.dart' show ActionLine, ControlsLine;
import '../mobile/metrics.dart' show Metrics, TypeScale;
import '../protocol/factory/v1/factory.pb.dart';
import '../dictation.dart' show Dictation, DictationHold;
import '../needs_you_focus.dart'
    show
        FocusOutcomeState,
        FocusSession,
        focusDismissReasons,
        focusDismissWithoutReason;
import '../outbox.dart' show decisionWaiting;
import '../review_answer.dart'
    show
        PendingReview,
        ReviewMovedOn,
        ReviewMoves,
        ReviewTalk,
        finishReview,
        heldEntry,
        newerEntry,
        reviewChosenChanged,
        reviewFailure,
        reviewHeld,
        reviewPick,
        reviewToFetch,
        reviewWaits,
        sendReview;
import 'document_host.dart' show DocumentSurface;
import '../visible_codes.dart' show proseCodes;

/// Reads a drawn page's composite, as native wrote it (tests replace it).
@visibleForTesting
Future<Uint8List> Function(String file) readComposite = (file) =>
    File(file).readAsBytes();

/// An upload's chunk: the backend takes at most 1 MiB at a time.
const _chunk = 1024 * 1024;

/// The question waiting on a file: [agentId]'s oldest pending review of
/// [path], fetched from its waiting entries as the workspace changes, or
/// as the agent's open conversation holds it, kept up to date.
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
    _backend?.transcript.revision.removeListener(_held);
    _backend = backend
      ..workspace.addListener(_changed)
      ..transcript.revision.addListener(_held);
    reviewChosenChanged
      ..removeListener(_held)
      ..addListener(_held);
    _entries.clear();
    _fetching.clear();
    _shown = null;
    final now = ask;
    _heard = now?.ask;
    _shown = now?.seq;
    _fetch();
  }

  Backend? get backend => _backend;

  /// The ask listeners last heard of: the open conversation changes often
  /// (its streaming), the ask seldom.
  AskPerson? _heard;

  /// The card the bar showed last: it stays, held, while a reply has it
  /// wait on its agent ([reviewPick]).
  int? _shown;

  void _notify() {
    final now = ask;
    _heard = now?.ask;
    _shown = now?.seq;
    notifyListeners();
  }

  void _held() {
    if (_disposed || identical(ask?.ask, _heard)) return;
    _notify();
  }

  void _changed() {
    if (_disposed) return;
    _notify();
    _fetch();
  }

  /// Fetches [seq] again, as it is now (its agent changed it, say): what
  /// waits on the file once it's come.
  Future<PendingReview?> refresh(int seq) async {
    final backend = _backend;
    if (backend == null) return null;
    final entry = await backend.fetchEntry(agentId, seq);
    if (_disposed || backend != _backend) return null;
    if (entry != null) _entries[seq] = entry;
    _notify();
    return ask;
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
    // What waits, what the bar holds, and what changed since it came.
    for (final seq in reviewToFetch(
      agentId: agentId,
      waiting: _waiting,
      entries: _entries,
      shown: _shown,
    ).toList()) {
      if (!_fetching.add(seq)) continue;
      unawaited(() async {
        try {
          final entry = await backend.fetchEntry(agentId, seq);
          if (entry != null && !_disposed && backend == _backend) {
            _entries[seq] = entry;
            _notify();
          }
        } on RequestFailure {
          // Tried again when the workspace next changes.
        } finally {
          _fetching.remove(seq);
        }
      }());
    }
  }

  /// The question on this file the bar answers: the card whose Review was
  /// pressed (Abbacchio's row 1312), else the oldest; held while a reply
  /// has it wait on its agent (row 1325, [reviewPick]).
  PendingReview? get ask {
    final backend = _backend;
    return reviewPick(
      agentId: agentId,
      path: path,
      waiting: _waiting,
      entryOf: (seq) => newerEntry(
        backend == null ? null : heldEntry(backend.transcript, agentId, seq),
        _entries[seq],
      ),
      shown: _shown,
    );
  }

  /// Whether [review] waits on the person: not, while a reply has it wait
  /// on its agent; its bar holds, nothing to send.
  bool waits(PendingReview review) {
    final backend = _backend;
    return backend != null && reviewWaits(backend, agentId, review.seq);
  }

  @override
  void dispose() {
    _disposed = true;
    _backend?.workspace.removeListener(_changed);
    _backend?.transcript.revision.removeListener(_held);
    reviewChosenChanged.removeListener(_held);
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
    this.session,
  });

  final ReviewWatch watch;
  final DocumentSurface surface;

  /// Focus mode's, when the bar is its stage's answer (#514, spec §6.4):
  /// it puts the card's answer in the stage's dock, so ⌘↩, ⌘⌫ and its own
  /// buttons go through the session, which moves on once the answer is
  /// accepted; answered, nothing closes (the next card comes in).
  final FocusSession? session;

  @override
  State<DocumentReviewBar> createState() => _DocumentReviewBarState();
}

class _DocumentReviewBarState extends State<DocumentReviewBar> {
  var _busy = false;

  /// Whether the bar moved on to its card by itself.
  final _moves = ReviewMoves();

  /// The person's answer, sent with either button (Matt's rule: approve
  /// or dismiss, always with an optional comment); the answer itself when
  /// the agent wants words.
  final _comment = TextEditingController();
  final _field = FocusNode(debugLabel: 'review answer');

  /// The field's dictation: what holding ⌘↩ or ⌘⌫ records into, in focus
  /// mode.
  final _dictation = GlobalKey();

  /// What the bar put in focus mode's dock, and the dock.
  CardAnswer? _published;
  ValueNotifier<CardAnswer?>? _dock;

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
    final another = _restored != null;
    _restored = seq;
    final kept = widget.surface.answers[seq];
    // From another card, its words stay with it (kept above), never in
    // this one's field, where they'd answer the wrong question (Zucchero's
    // row 513): this card's own, or none.
    if (!another && (kept == null || _comment.text.isNotEmpty)) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || _restored != seq) return;
      if (another) {
        _comment.text = kept ?? '';
      } else if (_comment.text.isEmpty) {
        _comment.text = kept!;
      }
    });
  }

  @override
  void dispose() {
    if (widget.session case final session?) {
      if (identical(session.say, _dictation.currentState)) session.say = null;
      if (session.intoField == _field.requestFocus) session.intoField = null;
      // After the frame: the dock's listeners rebuild. Focus mode may have
      // ended meanwhile (its session goes after the frame that stopped
      // showing it): then there's nothing to tell.
      final dock = _dock;
      final published = _published;
      if (dock != null && published != null) {
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (!session.disposed && identical(dock.value, published)) {
            dock.value = null;
          }
        });
      }
    }
    _comment
      ..removeListener(_typed)
      ..dispose();
    _field.dispose();
    super.dispose();
  }

  /// In focus mode, the card's answer in the stage's [dock] (what ⌘↩ and
  /// ⌘⌫ do), again whenever it changes: what's typed, sending, an error.
  /// And the field for R and for holding ⌘↩ or ⌘⌫ to say the answer.
  void _publish(
    FocusSession session,
    ValueNotifier<CardAnswer?> dock,
    String itemKey,
    PendingReview ask, {
    required String placeholder,
    required String? error,
  }) {
    final wantsText = ask.ask.wantsText;
    final next = CardAnswer(
      key: itemKey,
      // The bar is the stage's: it draws the field and the buttons, and the
      // quick reasons of a no (FocusReviewBody answers).
      docked: true,
      field: _comment,
      placeholder: placeholder,
      wantsText: wantsText,
      yes: wantsText ? 'Answer' : 'Approve',
      no: 'Dismiss',
      typed: _comment.text.trim().isNotEmpty,
      busy: _busy,
      error: error,
      // Words are the answer when the agent wants them: nothing to
      // approve without.
      onYes: wantsText ? null : _sendOrThrow,
      onSubmit: _sendOrThrow,
      onNo: (reason) => _sendOrThrow(dismiss: true, reason: reason),
      filesKey: cardFilesKey(widget.watch.agentId, ask.ask.id),
    );
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || session.disposed) return;
      final state = _dictation.currentState;
      session.say = state is DictationHold ? state as DictationHold : null;
      session.intoField = _field.requestFocus;
      final now = dock.value;
      if (identical(dock, _dock) &&
          now != null &&
          identical(now, _published) &&
          now.same(next)) {
        return;
      }
      _dock = dock;
      dock.value = _published = next;
    });
  }

  /// The answer, for focus mode's session: thrown back as a refusal when
  /// it wasn't accepted, so the card stays, with what the bar says.
  Future<void> _sendOrThrow({bool dismiss = false, String? reason}) async {
    final ask = widget.watch.ask;
    if (!mounted || ask == null) {
      throw const RequestFailure("Your answer wasn't sent. Try again.");
    }
    if (!await _answer(ask, dismiss: dismiss, reason: reason)) {
      throw RequestFailure(_error ?? "Your answer wasn't sent. Try again.");
    }
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
    final pages = surface.drawn;
    final attachments = <Attachment>[];
    final stem = _stem(surface.path);
    final tag = Random().nextInt(1 << 32).toRadixString(36);
    for (final page in pages) {
      if (mounted) {
        setState(() => _sending = (sent: attachments.length, of: pages.length));
      }
      final composite = await surface.exportPage(page);
      final bytes = await readComposite(composite.file);
      // An image's one drawing has no page.
      final at = page == null ? 'drawing' : 'p$page';
      final id = 'review-$seq-$at-$tag';
      var received = await backend.startUpload(
        id,
        widget.watch.agentId,
        name: '$stem-$at.png',
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

  Future<bool> _answer(
    PendingReview ask, {
    bool dismiss = false,
    String? reason,
  }) async {
    final backend = widget.watch.backend;
    if (_busy || backend == null || !widget.watch.waits(ask)) return false;
    setState(() {
      _busy = true;
      _error = null;
    });
    // The drawings went up: what fails after is the answer's own.
    var drawn = false;
    try {
      final attachments = await _drawings(backend, ask.seq);
      drawn = true;
      final typed = _comment.text.trim();
      // A quick reason for a no (focus mode's), before anything typed.
      final text = reason == null
          ? typed
          : typed.isEmpty
          ? reason
          : '$reason\n\n$typed';
      final watch = widget.watch;
      // The card as the bar shows it (#428), and its talk (#492).
      await sendReview(
        backend,
        watch.agentId,
        ask,
        refresh: () => watch.refresh(ask.seq),
        text: text,
        dismiss: dismiss,
        // A comment typed on a question that doesn't want words is a
        // reply (#370), the drawings with it; it carries out nothing.
        reply: !dismiss && !ask.ask.wantsText && text.isNotEmpty,
        attachments: attachments,
        // Cut off: "Not sent yet", and it goes once the connection's back.
        onWaiting: () {
          if (mounted) setState(() => _error = decisionWaiting);
        },
      );
      _comment.clear();
      if (mounted) setState(() => _error = null);
      // Answered: the file's pane closes and the person is back where
      // they pressed Review. In focus mode nothing closes: the next card
      // comes in once this one stops waiting.
      // (Illuso's forgetReview goes here then, once it lands.)
      if (mounted && widget.session == null) {
        unawaited(
          finishReview(
            context,
            widget.watch.agentId,
            widget.watch.path,
          ).catchError((Object _) {}),
        );
      }
      return true;
    } on RequestFailure catch (failure) {
      if (mounted) {
        // Its agent changed it meanwhile: the words and the ink stay, for
        // the person to look again and send.
        setState(() => _error = reviewFailure(failure));
      }
      return false;
    } on FileSystemException catch (error) {
      // A drawn page couldn't be read, or a file added to the answer
      // (#511), after the drawings went up.
      if (mounted) {
        setState(
          () => _error = drawn
              ? cardFilesFailure(error)
              : "Couldn't send your drawings, so your answer wasn't sent. "
                    'Try again.',
        );
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

  /// Return in the field: Submit, the typed comment as a reply (or the
  /// words a question asked for); nothing when it's empty, so a stray
  /// Return never approves.
  void _submit(PendingReview ask) {
    if (_busy || _comment.text.trim().isEmpty) return;
    final session = widget.session;
    unawaited(session != null ? session.yes() : _answer(ask));
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([
      widget.watch,
      widget.surface,
      ?widget.session,
    ]),
    builder: (context, _) => _build(context),
  );

  Widget _build(BuildContext context) {
    // Focus mode's card answered on another device or taken back while
    // it was read: how it ended, and Next; the document stays, its ink
    // saving as ever (§6.6).
    if (widget.session case final session?) {
      final key = session.current?.key;
      final outcome = key == null ? null : session.outcomeOf(key);
      if (outcome != null && outcome.state == FocusOutcomeState.ended) {
        return _ended(context, session, outcome.words);
      }
    }
    final ask = widget.watch.ask;
    final agent = widget.watch.backend?.workspace.agent(widget.watch.agentId);
    if (ask == null || agent == null) return const SizedBox.shrink();
    _restore(ask.seq);
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final drawn = widget.surface.drawn.length;
    final typed = commentTyped(_comment);
    // A comment on a question that doesn't want words: Submit alone.
    final replying = typed && !ask.ask.wantsText;
    // Where the words go, before anything's typed (rule 4).
    final placeholder = ask.ask.wantsText
        ? 'Your answer'
        : replyPlaceholder(agent.name);
    // Held while a reply has it wait on its agent (Abbacchio's row 1325).
    final held = !widget.watch.waits(ask);
    final session = widget.session;
    if (session != null) {
      final dock = AnswerDock.of(context);
      _publish(
        session,
        dock?.answer ?? session.answer,
        dock?.itemKey ?? session.current?.key ?? '',
        ask,
        placeholder: placeholder,
        error: _error,
      );
    }
    // In focus mode, a card changed a moment ago takes no answer yet (§8):
    // the main button says so.
    final guarded = session?.guarded ?? false;
    final off = _busy || held || (session != null && !session.answerable);
    // Through focus mode's session when there's one: it marks the card
    // sent, and moves on once it's accepted.
    void yes() => unawaited(session != null ? session.yes() : _answer(ask));
    void no() =>
        unawaited(session != null ? session.no() : _answer(ask, dismiss: true));
    final sending = _sending;
    final error = _error ?? (held ? reviewHeld(ask.ask, agent.name) : null);
    // The line at the row's start says what pressing does (the Needs you
    // cards' rule 2): sending, then what went wrong, then what Submit
    // does instead and the drawings that go with any answer (§3.1: a
    // drawing leaves the buttons as they are). Nothing when the buttons
    // say it all.
    final String? line = sending != null
        ? (sending.of == 1
              ? 'Sending your drawing…'
              : 'Sending your drawings… ${sending.sent + 1} of ${sending.of}')
        : error ??
              [
                if (replying) insteadLine('approving'),
                if (drawn == 1) drawingGoesLine,
                if (drawn > 1) 'Your drawings on $drawn pages go with it.',
              ].join(' ').emptyAsNull;
    final lineColor =
        sending == null &&
            error != null &&
            error != decisionWaiting &&
            _error != null
        ? palette.danger
        : palette.textMuted;
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
            // Focus mode's header already says who asks.
            if (session == null)
              Row(
                children: [
                  ExcludeSemantics(
                    child: AgentCharacter(agent: agent, size: 20),
                  ),
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
            if (_moves.movedTo(widget.watch.agentId, ask.seq)) ...[
              const SizedBox(height: Space.xs),
              ReviewMovedOn(name: agent.name),
            ],
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
            // What was said on its card since the person saw it, the
            // newest showing: so "Look at it again" shows what changed.
            ReviewTalk(
              agentId: widget.watch.agentId,
              review: ask,
              agent: agent,
              name: agent.name,
            ),
            const SizedBox(height: Space.s),
            // The answer, always open: Return sends what's typed, Shift
            // and Return starts a new line on a hardware keyboard. Or said:
            // the mic at its end puts what's said in, to check first.
            Dictation(
              key: _dictation,
              controller: _comment,
              focusNode: _field,
              enabled: !_busy,
              // Files with the answer (#511), the card's: what was added
              // on it comes here too. The drawings go with them.
              trailing: CardAttachButton(
                card: cardFilesKey(widget.watch.agentId, ask.ask.id),
                enabled: !_busy,
                onNote: (note) {
                  if (mounted) setState(() => _error = note);
                },
              ),
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
                    placeholder: placeholder,
                    semanticsLabel: placeholder,
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
            CardFileChips(
              card: cardFilesKey(widget.watch.agentId, ask.ask.id),
              busy: _busy,
            ),
            const SizedBox(height: Space.s),
            // The row (the Needs you cards' rule 2): the line, then Dismiss
            // quiet, then the answer at the end, gone to one Submit while a
            // reply is typed. The buttons go under the line when the pane
            // is narrow or the text large, and stack as on a phone.
            ActionLine(
              lead: Semantics(
                liveRegion: sending != null || error != null,
                child: Text(
                  line ?? '',
                  style: type.caption.copyWith(color: lineColor),
                ),
              ),
              action: _rowOf(
                reasons: session != null && session.reasonsShowing,
                [
                  // Focus mode's no, asked why first (§5.4): the quick
                  // reasons, or none.
                  if (session != null && session.reasonsShowing) ...[
                    for (final (index, reason) in focusDismissReasons.indexed)
                      Button(
                        key: ValueKey('review-reason-$index'),
                        label: reason,
                        onPressed: off
                            ? null
                            : () => unawaited(session.no(reason: reason)),
                      ),
                    Button(
                      key: const ValueKey('review-no-reason'),
                      label: focusDismissWithoutReason,
                      onPressed: off
                          ? null
                          : () => unawaited(session.no(askWhy: false)),
                    ),
                  ] else if (replying)
                    SubmitButton(
                      key: const ValueKey('review-send'),
                      onPressed: off ? null : yes,
                    )
                  else ...[
                    Button(
                      key: const ValueKey('review-dismiss'),
                      label: 'Dismiss',
                      onPressed: off ? null : no,
                    ),
                    Button(
                      key: const ValueKey('review-send'),
                      // No Approve when the agent wants words: they're
                      // the answer.
                      label: guarded
                          ? 'Updated'
                          : ask.ask.wantsText
                          ? 'Answer'
                          : 'Approve',
                      primary: true,
                      // Words are the answer when the agent wants them.
                      onPressed: off || (ask.ask.wantsText && !typed)
                          ? null
                          : yes,
                    ),
                  ],
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

extension on String {
  String? get emptyAsNull => isEmpty ? null : this;
}

/// The row's buttons: in a line while they fit, stacked as on a phone when
/// they don't; focus mode's quick [reasons] wrap instead.
Widget _rowOf(List<Widget> buttons, {required bool reasons}) => reasons
    ? Wrap(
        alignment: WrapAlignment.end,
        spacing: Space.s,
        runSpacing: Space.s,
        children: buttons,
      )
    : ControlsLine(children: buttons);

extension on _DocumentReviewBarState {
  /// Focus mode's card that ended elsewhere: [words], and Next.
  Widget _ended(BuildContext context, FocusSession session, String words) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Container(
      key: const ValueKey('document-review-ended'),
      decoration: BoxDecoration(
        color: palette.canvas,
        border: Border(top: BorderSide(color: palette.hairline)),
      ),
      padding: const EdgeInsets.symmetric(
        horizontal: Space.l,
        vertical: Space.s,
      ),
      child: Wrap(
        alignment: WrapAlignment.spaceBetween,
        crossAxisAlignment: WrapCrossAlignment.center,
        spacing: Space.m,
        runSpacing: Space.s,
        children: [
          Semantics(
            liveRegion: true,
            child: Text(
              words,
              style: type.secondary.copyWith(color: palette.textMuted),
            ),
          ),
          Button(
            key: const ValueKey('review-next'),
            label: 'Next',
            primary: true,
            onPressed: session.waiting.isEmpty ? null : session.next,
          ),
        ],
      ),
    );
  }

  /// The offer to turn what's handwritten into the answer, on an iPad
  /// that reads handwriting (iPadOS 27 and later) with something drawn or
  /// chosen with the lasso; nothing at all on one that doesn't.
  List<Widget> _handwriting(BuildContext context) {
    final surface = widget.surface;
    final selected = surface.strokeSelection != null;
    if (!surface.handwriting.available ||
        (!selected && surface.inked.isEmpty && !_reading)) {
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
