import 'dart:async';

import 'package:flutter/services.dart' show TextInputAction;
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../characters/character.dart';
import '../controls.dart';
import '../decision.dart' show SubmitButton, commentTyped;
import '../dictation.dart';
import '../identity.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../review_answer.dart'
    show PendingReview, ReviewAnswering, ReviewWatch, finishReview;
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

  final _answer = TextEditingController();
  final _focus = FocusNode();
  var _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _answer.addListener(_changed);
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
    _answer.dispose();
    _focus.dispose();
    super.dispose();
  }

  /// Answers [ask]: Approve with nothing typed, the words a question that
  /// wants them asked for, Dismiss, or a typed comment as a reply (#370),
  /// which carries out nothing. A failure keeps the words and says so
  /// under the field.
  Future<void> _send(PendingReview ask, {bool dismiss = false}) async {
    final text = _answer.text.trim();
    if (_busy || !dismiss && ask.ask.wantsText && text.isEmpty) return;
    final reply = !dismiss && !ask.ask.wantsText && text.isNotEmpty;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await _watch!.backend.answerAsk(
        widget.agentId,
        ask.seq,
        text: text,
        dismiss: dismiss,
        reply: reply,
        // The card as the person read it (#428).
        revision: ask.ask.revision,
      );
    } on RequestFailure catch (failure) {
      unawaited(Haptics.warning());
      if (mounted) {
        setState(() {
          _busy = false;
          // Its agent changed it meanwhile: the bar shows the new one.
          _error = cardChanged(failure)
              ? failure.message
              : "Couldn't send your answer. Try again.";
        });
      }
      return;
    }
    unawaited(dismiss ? Haptics.light() : Haptics.success());
    if (!mounted) return;
    _focus.unfocus();
    _answer.clear();
    setState(() => _busy = false);
    // Its pane closes, everywhere, and this page with it; back where the
    // person pressed Review.
    await finishReview(context, widget.agentId, widget.path);
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
            data: asking ? media.removeViewInsets(removeBottom: true) : media,
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
    final typed = commentTyped(_answer);
    // Approve with nothing typed, unless the question wants words: then
    // Send, faint until there are some. A comment typed on one that
    // doesn't is a reply: Submit alone, Dismiss gone with Approve.
    final replying = typed && !ask.ask.wantsText;
    final send = replying
        ? SubmitButton(onPressed: _busy ? null : () => unawaited(_send(ask)))
        : Button(
            label: ask.ask.wantsText ? 'Send' : 'Approve',
            primary: true,
            onPressed: _busy || ask.ask.wantsText && !typed
                ? null
                : () => unawaited(_send(ask)),
          );
    final dismiss = Button(
      label: 'Dismiss',
      onPressed: _busy ? null : () => unawaited(_send(ask, dismiss: true)),
    );
    final field = Semantics(
      hint: 'Return sends it to $name.',
      child: TextField(
        controller: _answer,
        focusNode: _focus,
        placeholder: 'Your answer to $name',
        kind: InputKind.message,
        textInputAction: TextInputAction.send,
        // Return with nothing typed does nothing: Approve is its button.
        onSubmitted: (_) {
          if (_answer.text.trim().isEmpty) {
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
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    // A bar over the file, as iOS's toolbars: it grows with the text only
    // so far, so the file keeps its room at the largest sizes. A viewer's
    // page ends at the home indicator, not the keyboard: the bar rises by
    // the keyboard itself.
    return Padding(
      padding: EdgeInsets.only(bottom: keyboard),
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
                const SizedBox(height: Space.s),
                // Or said: the mic puts what's said in, to check first.
                if (stacked) ...[
                  Dictation(
                    controller: _answer,
                    focusNode: _focus,
                    enabled: !_busy,
                    child: field,
                  ),
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
                    controller: _answer,
                    focusNode: _focus,
                    enabled: !_busy,
                    // The answer's button last, under the thumb.
                    trailing: send,
                    child: field,
                  ),
                if (widget.kind == 'text') ...[
                  const SizedBox(height: Space.xs),
                  Text(
                    'Your comments on lines go to $name with your answer.',
                    style: type.caption.copyWith(color: palette.textFaint),
                  ),
                ],
                if (_error case final error?) ...[
                  const SizedBox(height: Space.xs),
                  Text(
                    error,
                    style: type.secondary.copyWith(color: palette.danger),
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
