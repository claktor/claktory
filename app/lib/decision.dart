import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'dictation.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show Metrics, TypeScale;

/// The most a comment may say, as the backend checks too.
const decisionCommentLimit = 2000;

/// A comment to go with the person's decision on a card an agent waits on
/// (Matt's: approve or dismiss, always with an optional comment): under
/// the card's buttons, "Add a comment", quiet; tapped, a field. Whatever
/// is typed goes with either button, as the person's own words, to
/// [agent] only. The card owns [controller] and sends its text trimmed;
/// nothing is kept once the card is answered.
class DecisionComment extends StatefulWidget {
  const DecisionComment({
    super.key,
    required this.controller,
    required this.agent,
    this.peer = '',
    this.enabled = true,
    this.onSubmitted,
    this.note,
  });

  final TextEditingController controller;

  /// Who gets it: the agent's name.
  final String agent;

  /// For a pipe's card, the other side ("Ana's Claktory"), which never
  /// sees it; empty otherwise.
  final String peer;

  /// False while the decision is being sent.
  final bool enabled;

  /// Enter in the field, where the card has a main action for it.
  final VoidCallback? onSubmitted;

  /// Who gets it, said otherwise, where it depends on the answer.
  final String? note;

  @override
  State<DecisionComment> createState() => _DecisionCommentState();
}

class _DecisionCommentState extends State<DecisionComment> {
  late var _open = widget.controller.text.isNotEmpty;

  /// The field's, asked for once it opens: autofocus does nothing while
  /// something else in the scope has focus (a PDF's pane after a reload).
  final _focus = FocusNode();

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  void _openField() {
    setState(() => _open = true);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _focus.requestFocus();
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    if (!_open) {
      return Align(
        alignment: AlignmentDirectional.centerStart,
        child: Semantics(
          button: true,
          child: Pressable(
            onTap: widget.enabled ? _openField : null,
            builder: (context, hovered) => ConstrainedBox(
              // A finger's height on a phone or iPad.
              constraints: BoxConstraints(
                minHeight: touchFirst(context) ? Metrics.of(context).target : 0,
              ),
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: Space.xs),
                child: Align(
                  alignment: AlignmentDirectional.centerStart,
                  widthFactor: 1,
                  heightFactor: 1,
                  child: Text(
                    'Add a comment',
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
      );
    }
    final onSubmitted = widget.onSubmitted;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        // Enter does the card's main action, where it has one; Shift+Enter
        // starts a new line.
        // Or said: the mic puts what's said in, to check before sending.
        KeepWhileTyped(
          controller: widget.controller,
          child: Dictation(
            controller: widget.controller,
            focusNode: _focus,
            enabled: widget.enabled,
            child: CallbackShortcuts(
              bindings: {
                const SingleActivator(LogicalKeyboardKey.enter): ?onSubmitted,
              },
              child: TextField(
                controller: widget.controller,
                focusNode: _focus,
                placeholder: 'A comment for ${widget.agent}, if you like',
                // Named once typed in, when the placeholder is gone.
                semanticsLabel: 'A comment for ${widget.agent}',
                minLines: 1,
                maxLines: 6,
                // As much as the backend takes.
                inputFormatters: [
                  LengthLimitingTextInputFormatter(decisionCommentLimit),
                ],
              ),
            ),
          ),
        ),
        const SizedBox(height: Space.xs),
        Text(
          widget.note ??
              (widget.peer.isEmpty
                  ? '${widget.agent} gets it with your answer.'
                  : 'Only ${widget.agent} sees it, not ${widget.peer}.'),
          style: type.caption.copyWith(color: palette.textFaint),
        ),
      ],
    );
  }
}

/// Keeps the card around [child] while [controller] has something typed in
/// it, so what the person wrote is still there when the card scrolls out
/// of view and back (Zucchero's rows 189 and 193): a conversation or Needs
/// you otherwise builds it afresh. Nothing is kept anywhere else; it goes
/// with the conversation, and once the field is empty or gone (the card
/// answered) the card scrolls away like any entry.
class KeepWhileTyped extends StatefulWidget {
  const KeepWhileTyped({
    super.key,
    required this.controller,
    required this.child,
  });

  final TextEditingController controller;
  final Widget child;

  @override
  State<KeepWhileTyped> createState() => _KeepWhileTypedState();
}

class _KeepWhileTypedState extends State<KeepWhileTyped>
    with AutomaticKeepAliveClientMixin {
  late var _typed = widget.controller.text.isNotEmpty;

  @override
  bool get wantKeepAlive => _typed;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_changed);
  }

  @override
  void didUpdateWidget(KeepWhileTyped old) {
    super.didUpdateWidget(old);
    if (old.controller != widget.controller) {
      old.controller.removeListener(_changed);
      widget.controller.addListener(_changed);
      _changed();
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    final typed = widget.controller.text.isNotEmpty;
    if (typed == _typed) return;
    _typed = typed;
    updateKeepAlive();
  }

  @override
  Widget build(BuildContext context) {
    super.build(context);
    return widget.child;
  }
}
