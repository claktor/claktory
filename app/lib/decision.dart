import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'dictation.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show Metrics, TypeScale;
import 'scope.dart';
import 'voice_keep.dart' show KeptVoices;

/// The most a comment may say, as the backend checks too.
const decisionCommentLimit = 2000;

/// Whether something is typed in a card's [comment] (trimmed): its own
/// buttons then give way to one Submit, and what's typed goes as a reply,
/// never with a verdict (#370, Matt's).
bool commentTyped(TextEditingController comment) =>
    comment.text.trim().isNotEmpty;

/// Builds [builder] again whenever [controller]'s comment becomes typed or
/// empty ([commentTyped]), not at every key: a card's buttons, or one
/// Submit (#370).
class CommentTyped extends StatefulWidget {
  const CommentTyped({
    super.key,
    required this.controller,
    required this.builder,
  });

  final TextEditingController controller;
  final Widget Function(BuildContext context, bool typed) builder;

  @override
  State<CommentTyped> createState() => _CommentTypedState();
}

class _CommentTypedState extends State<CommentTyped> {
  late var _typed = commentTyped(widget.controller);

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_changed);
  }

  @override
  void didUpdateWidget(CommentTyped old) {
    super.didUpdateWidget(old);
    if (old.controller != widget.controller) {
      old.controller.removeListener(_changed);
      widget.controller.addListener(_changed);
      _typed = commentTyped(widget.controller);
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    final typed = commentTyped(widget.controller);
    if (typed != _typed) setState(() => _typed = typed);
  }

  @override
  Widget build(BuildContext context) => widget.builder(context, _typed);
}

/// The one button a card shows once a comment is typed (#370): it sends
/// the comment as a reply and carries out nothing.
class SubmitButton extends StatelessWidget {
  const SubmitButton({super.key, required this.onPressed});

  /// Null while it's being sent.
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) =>
      Button(label: 'Submit', primary: true, onPressed: onPressed);
}

/// A comment to go with the person's decision on a card an agent waits on
/// (Matt's: approve or dismiss, always with an optional comment): under
/// the card's buttons, "Add a comment", quiet; tapped, a field. The card
/// owns [controller] and sends its text trimmed; nothing is kept once the
/// card is answered.
///
/// With [onSubmit] (#370), once something is typed the card shows one
/// Submit in place of its buttons ([CommentTyped], [SubmitButton]), Return
/// in the field is Submit, and what's typed goes as a reply to [agent],
/// never as a yes or a no. Without it, whatever is typed goes with either
/// button, as the person's own words, to [agent] only.
class DecisionComment extends StatefulWidget {
  const DecisionComment({
    super.key,
    required this.controller,
    required this.agent,
    this.peer = '',
    this.enabled = true,
    this.onSubmitted,
    this.onSubmit,
    this.note,
    this.keep,
    this.attach,
    this.files,
    this.holdsFiles = false,
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

  /// The card's Submit: sends what's typed as a reply. Return in the field
  /// does it, once something is typed, and the note under the field goes:
  /// the line beside Submit says what happens.
  final VoidCallback? onSubmit;

  /// Who gets it, said otherwise, where it depends on the answer.
  final String? note;

  /// What a recording for it is kept as until its words come (#401): one
  /// kept from before opens the field, for it to come back to.
  final DictationFor? keep;

  /// Files with the comment (#511): the paperclip, beside the mic, and the
  /// files waiting, under the field; none on cards that take no files.
  final Widget? attach;
  final Widget? files;

  /// Files wait to go with it: the field shows, with them.
  final bool holdsFiles;

  @override
  State<DecisionComment> createState() => _DecisionCommentState();
}

class _DecisionCommentState extends State<DecisionComment> {
  late var _open = widget.controller.text.isNotEmpty || widget.holdsFiles;

  @override
  void didUpdateWidget(DecisionComment old) {
    super.didUpdateWidget(old);
    if (widget.holdsFiles && !_open) _open = true;
  }

  /// The field's, asked for once it opens: autofocus does nothing while
  /// something else in the scope has focus (a PDF's pane after a reload).
  final _focus = FocusNode();

  KeptVoices? _voices;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final voices = KeptVoices.of(AppScope.of(context));
    if (identical(voices, _voices)) return;
    _voices?.removeListener(_kept);
    _voices = voices..addListener(_kept);
    _open = _open || _holdsKept();
  }

  bool _holdsKept() {
    final key = widget.keep?.key;
    return key != null &&
        ((_voices?.holds(key) ?? false) || dictationLive(key));
  }

  /// A recording kept for this comment from before: the field opens, so
  /// it comes back to it.
  void _kept() {
    if (_open || !_holdsKept() || !mounted) return;
    setState(() => _open = true);
  }

  @override
  void dispose() {
    _voices?.removeListener(_kept);
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
    final submit = widget.onSubmit;
    final onSubmitted = submit == null
        ? widget.onSubmitted
        : () {
            if (widget.enabled && commentTyped(widget.controller)) submit();
          };
    final note = Text(
      widget.note ??
          (widget.peer.isEmpty
              ? '${widget.agent} gets it with your answer.'
              : 'Only ${widget.agent} sees it, not ${widget.peer}.'),
      style: type.caption.copyWith(color: palette.textFaint),
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        // Enter does the card's main action, where it has one, or Submit;
        // Shift+Enter starts a new line.
        // Or said: the mic puts what's said in, to check before sending.
        KeepWhileTyped(
          controller: widget.controller,
          child: Dictation(
            controller: widget.controller,
            focusNode: _focus,
            enabled: widget.enabled,
            keep: widget.keep,
            trailing: widget.attach,
            child: CallbackShortcuts(
              bindings: {
                const SingleActivator(LogicalKeyboardKey.enter): ?onSubmitted,
                if (submit != null)
                  const SingleActivator(LogicalKeyboardKey.numpadEnter):
                      onSubmitted!,
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
        ?widget.files,
        // With Submit, the line beside it says what happens once typed.
        if (submit == null) ...[
          const SizedBox(height: Space.xs),
          note,
        ] else
          CommentTyped(
            controller: widget.controller,
            builder: (context, typed) => typed
                ? const SizedBox.shrink()
                : Padding(
                    padding: const EdgeInsets.only(top: Space.xs),
                    child: note,
                  ),
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
