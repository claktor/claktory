import 'dart:async';
import 'dart:io' show FileSystemException;
import 'dart:math' as math;

import 'package:flutter/services.dart' show TextInputAction;
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../card_files.dart';
import '../characters/character.dart';
import '../controls.dart';
import '../decision.dart' show SubmitButton, commentTyped;
import '../dictation.dart';
import '../identity.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../outbox.dart' show decisionWaiting;
import '../card_answer.dart' show CardDrafts, cardKey;
import '../review_answer.dart'
    show
        PendingReview,
        ReviewAnswering,
        ReviewMovedOn,
        ReviewMoves,
        ReviewTalk,
        ReviewWatch,
        finishReview,
        reviewFailure,
        reviewHeld,
        reviewWaits,
        sendReview;
import '../scope.dart';
import '../visible_codes.dart' show proseCodes;
import 'drawer.dart' show largeText;
import 'haptics.dart';
import 'input.dart';
import 'metrics.dart' show TypeScale;

/// A file's viewer on a phone, and at its foot, while [agentId] waits for
/// the person's review of it, the answer: who asks and what, then a field
/// that's always open. With nothing typed, Approve (or Send, for a
/// question that wants words) and Dismiss, quieter. Once a comment is
/// typed, one Submit (Return too) sends it as a reply, never a yes or a
/// no (#370); a question that wants words keeps Send and Dismiss. Answered, the viewer closes ([finishReview]),
/// back where the person was. While the question waits, nothing else is added
/// to the file: on text, comments on lines go with the answer.
class PhoneReviewBar extends StatefulWidget {
  const PhoneReviewBar({
    super.key,
    required this.agentId,
    required this.path,
    required this.child,
    this.kind = '',
  });

  final String agentId;
  final String path;

  /// The viewer: it keeps its place in the tree whether a question waits
  /// or not, so a video or a PDF keeps where it was.
  final Widget child;

  /// The viewer's kind ("text", "image"…): text says where its comments go.
  final String kind;

  @override
  State<PhoneReviewBar> createState() => _PhoneReviewBarState();
}

class _PhoneReviewBarState extends State<PhoneReviewBar> {
  /// The question waiting on the file, as the agent's entries come.
  ReviewWatch? _watch;

  /// What's typed, each card's own draft ([CardDrafts]): a card closed
  /// while the bar held it leaves its words with it, never in the next
  /// card's field, where they'd answer the wrong question (Zucchero's row
  /// 513); kept while the app runs, wherever the card's reopened (Bruno's
  /// Q11).
  final _answers = <TextEditingController>{};

  TextEditingController _answerFor(PendingReview ask) {
    final field = CardDrafts.of(_watch!.backend)
        .field(cardKey(widget.agentId, ask.ask.id));
    if (_answers.add(field)) field.addListener(_changed);
    return field;
  }

  final _focus = FocusNode();

  /// Whether the bar moved on to its card by itself.
  final _moves = ReviewMoves();
  var _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _rewatch();
  }

  @override
  void didUpdateWidget(PhoneReviewBar old) {
    super.didUpdateWidget(old);
    if (old.agentId != widget.agentId || old.path != widget.path) _rewatch();
  }

  void _rewatch() {
    final backend = AppScope.of(context);
    final watch = _watch;
    if (watch != null &&
        watch.backend == backend &&
        watch.agentId == widget.agentId &&
        watch.path == widget.path) {
      return;
    }
    watch?.dispose();
    _watch = ReviewWatch(backend, widget.agentId, widget.path)
      ..addListener(_changed);
  }

  @override
  void dispose() {
    _watch?.dispose();
    // The drafts are the cards': they stay.
    for (final answer in _answers) {
      answer.removeListener(_changed);
    }
    _focus.dispose();
    super.dispose();
  }

  /// Answers [ask]: Approve with nothing typed, the words a question that
  /// wants them asked for, Dismiss, or a typed comment as a reply (#370),
  /// which carries out nothing. A failure keeps the words and says so
  /// under the field.
  Future<void> _send(PendingReview ask, {bool dismiss = false}) async {
    final answer = _answerFor(ask);
    final text = answer.text.trim();
    if (_busy || _held(ask) || !dismiss && ask.ask.wantsText && text.isEmpty) {
      return;
    }
    final reply = !dismiss && !ask.ask.wantsText && text.isNotEmpty;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final watch = _watch!;
      // The card as the bar shows it (#428), and its talk (#492).
      await sendReview(
        watch.backend,
        widget.agentId,
        ask,
        refresh: () => watch.refresh(ask.seq),
        text: text,
        dismiss: dismiss,
        reply: reply,
        // Cut off: "Not sent yet", and it goes once the connection's back.
        onWaiting: () {
          if (mounted) setState(() => _error = decisionWaiting);
        },
      );
    } on RequestFailure catch (failure) {
      unawaited(Haptics.warning());
      if (mounted) {
        setState(() {
          _busy = false;
          // Its agent changed it meanwhile: the bar shows the new one.
          _error = reviewFailure(failure);
        });
      }
      return;
    } on FileSystemException catch (error) {
      // A file to go with it couldn't be read (#511).
      unawaited(Haptics.warning());
      if (mounted) {
        setState(() {
          _busy = false;
          _error = cardFilesFailure(error);
        });
      }
      return;
    }
    unawaited(dismiss ? Haptics.light() : Haptics.success());
    if (!mounted) return;
    _focus.unfocus();
    answer.clear();
    setState(() {
      _busy = false;
      _error = null;
    });
    // Its pane closes, everywhere, and this page with it; back where the
    // person pressed Review.
    await finishReview(context, widget.agentId, widget.path);
  }

  /// A reply has [ask] wait on its agent: the bar holds, nothing to send
  /// until the agent answers (Abbacchio's row 1325).
  bool _held(PendingReview ask) {
    final backend = _watch?.backend;
    return backend == null || !reviewWaits(backend, widget.agentId, ask.seq);
  }

  @override
  Widget build(BuildContext context) {
    final ask = _watch?.pending;
    final agent = _watch?.backend.workspace.agent(widget.agentId);
    final asking = ask != null && agent != null;
    final media = MediaQuery.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          // The bar takes the keyboard's room: the viewer above it ends at
          // the bar either way. The same tree with or without a question,
          // so the viewer keeps its place.
          child: MediaQuery(
            data: asking
                ? media
                      .removeViewInsets(removeBottom: true)
                      .removePadding(removeBottom: true)
                : media,
            // Meanwhile nothing else is added to the file: no comments
            // on media.
            child: ReviewAnswering(answering: asking, child: widget.child),
          ),
        ),
        if (asking) _bar(context, ask, agent),
      ],
    );
  }

  Widget _bar(BuildContext context, PendingReview ask, Agent agent) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final name = agent.name;
    final answer = _answerFor(ask);
    final typed = commentTyped(answer);
    // Approve with nothing typed, unless the question wants words: then
    // Send, faint until there are some. A comment typed on one that
    // doesn't is a reply: Submit alone, Dismiss gone with Approve.
    final replying = typed && !ask.ask.wantsText;
    final held = _held(ask);
    final off = _busy || held;
    final send = replying
        ? SubmitButton(onPressed: off ? null : () => unawaited(_send(ask)))
        : Button(
            label: ask.ask.wantsText ? 'Send' : 'Approve',
            primary: true,
            onPressed: off || ask.ask.wantsText && !typed
                ? null
                : () => unawaited(_send(ask)),
          );
    final dismiss = Button(
      label: 'Dismiss',
      onPressed: off ? null : () => unawaited(_send(ask, dismiss: true)),
    );
    final field = Semantics(
      hint: 'Return sends it to $name.',
      child: TextField(
        controller: answer,
        focusNode: _focus,
        placeholder: 'Your answer to $name',
        kind: InputKind.message,
        textInputAction: TextInputAction.send,
        // Return with nothing typed does nothing: Approve is its button.
        onSubmitted: (_) {
          if (answer.text.trim().isEmpty) {
            _focus.requestFocus();
            return;
          }
          unawaited(_send(ask));
        },
        minLines: 1,
        maxLines: 4,
      ),
    );
    final stacked = largeText(context);
    final asks = '$name asks you to review this';
    // Files with the answer (#511), the card's: what was added on it comes
    // here too.
    final files = cardFilesKey(widget.agentId, ask.ask.id);
    void filesNote(String? note) {
      if (mounted) setState(() => _error = note);
    }

    final attach = CardAttachButton(
      card: files,
      enabled: !_busy,
      onNote: filesNote,
    );
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    // A bar over the file, as iOS's toolbars: it grows with the text only
    // so far, so the file keeps its room at the largest sizes. A viewer's
    // page ends at the home indicator, not the keyboard: the bar rises by
    // the keyboard itself.
    // A page that runs edge to edge (a picture, a video) leaves the safe
    // areas to the bar: it keeps clear of the home indicator and, turned,
    // of the notch.
    final safe = MediaQuery.paddingOf(context);
    return Padding(
      padding: EdgeInsets.only(
        left: safe.left,
        right: safe.right,
        bottom: math.max(keyboard, safe.bottom),
      ),
      child: MediaQuery.withClampedTextScaling(
        maxScaleFactor: 2,
        child: Semantics(
          container: true,
          explicitChildNodes: true,
          child: Container(
            key: const ValueKey('phone-review-bar'),
            decoration: BoxDecoration(
              color: palette.canvas,
              border: Border(top: BorderSide(color: palette.hairline)),
            ),
            padding: const EdgeInsets.fromLTRB(
              Space.m,
              Space.s,
              Space.m,
              Space.s,
            ),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                // At the largest sizes the question alone, so the file
                // keeps some room above it.
                if (!stacked || ask.ask.title.isEmpty)
                  Row(
                    children: [
                      ExcludeSemantics(
                        child: AgentCharacter(agent: agent, size: 20),
                      ),
                      const SizedBox(width: Space.s),
                      Expanded(
                        child: Text(
                          asks,
                          maxLines: stacked ? 1 : 2,
                          overflow: TextOverflow.ellipsis,
                          style: type.secondary.copyWith(
                            color: palette.textMuted,
                          ),
                        ),
                      ),
                      if (!stacked && !replying) ...[
                        const SizedBox(width: Space.s),
                        dismiss,
                      ],
                    ],
                  ),
                if (_moves.movedTo(widget.agentId, ask.seq)) ...[
                  const SizedBox(height: Space.xs),
                  ReviewMovedOn(name: name),
                ],
                if (ask.ask.title.isNotEmpty) ...[
                  if (!stacked) const SizedBox(height: Space.xs),
                  // The agent's words: what reorders them shows by its code
                  // (proseCodes, Fugo).
                  Text(
                    proseCodes(ask.ask.title),
                    maxLines: stacked ? 2 : 3,
                    overflow: TextOverflow.ellipsis,
                    semanticsLabel: stacked
                        ? '$asks: ${proseCodes(ask.ask.title)}'
                        : null,
                    style: type.body.copyWith(color: palette.text),
                  ),
                ],
                if (!stacked && ask.ask.body.isNotEmpty)
                  Text(
                    ask.ask.body,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: type.secondary.copyWith(color: palette.textMuted),
                  ),
                // What was said on its card since the person saw it, the
                // newest showing: so "Look at it again" shows what changed.
                ReviewTalk(
                  agentId: widget.agentId,
                  review: ask,
                  agent: agent,
                  name: name,
                  maxHeight: stacked ? 72 : 96,
                ),
                const SizedBox(height: Space.s),
                // Or said: the mic puts what's said in, to check first.
                if (stacked) ...[
                  Dictation(
                    controller: answer,
                    focusNode: _focus,
                    enabled: !_busy,
                    trailing: attach,
                    child: field,
                  ),
                  CardFileChips(card: files, busy: _busy),
                  const SizedBox(height: Space.s),
                  Row(
                    children: [
                      if (!replying) ...[
                        Expanded(child: dismiss),
                        const SizedBox(width: Space.s),
                      ],
                      Expanded(child: send),
                    ],
                  ),
                ] else
                  Dictation(
                    controller: answer,
                    focusNode: _focus,
                    enabled: !_busy,
                    // The answer's button last, under the thumb.
                    trailing: Row(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.end,
                      children: [
                        attach,
                        const SizedBox(width: Space.xs),
                        send,
                      ],
                    ),
                    child: field,
                  ),
                if (!stacked) CardFileChips(card: files, busy: _busy),
                if (widget.kind == 'text') ...[
                  const SizedBox(height: Space.xs),
                  Text(
                    'Your comments on lines go to $name with your answer.',
                    style: type.caption.copyWith(color: palette.textFaint),
                  ),
                ],
                if (_error ?? (held ? reviewHeld(ask.ask, name) : null)
                    case final error?) ...[
                  const SizedBox(height: Space.xs),
                  Text(
                    error,
                    style: type.secondary.copyWith(
                      color: error == decisionWaiting || _error == null
                          ? palette.textMuted
                          : palette.danger,
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}
