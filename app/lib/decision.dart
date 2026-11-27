import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'card_answer.dart' show replyPlaceholder;
import 'controls.dart';
import 'dictation.dart';
import 'find.dart' show FindHighlight;
import 'identity.dart';
import 'mobile/lines.dart'
    show CardActions, CardFieldRoom, KeepInSight, cardFieldMaxLines;
import 'mobile/metrics.dart' show Metrics, TypeScale;

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

/// The field on a card an agent waits on (Lucy's Needs you cards, rule 4):
/// open from the start, one line growing to six, above the card's row
/// ([CardRow]), with the mic (and #511's paperclip). Its placeholder says
/// where the words go before anything's typed: "Reply to June". It never
/// takes focus by itself: in focus mode bare keys would type into it, and a
/// stray letter would turn Approve into Submit. The card owns [controller]
/// and sends its text trimmed; nothing is kept once the card is answered.
///
/// Return never decides (rule 6): with [onSubmit], where words alone are an
/// answer (Submit, Send, a written answer), Return does it once something
/// is typed; without it, Return does nothing. Shift+Return is a new line,
/// and so is a phone's on-screen return, which sends no key.
class DecisionComment extends StatefulWidget {
  const DecisionComment({
    super.key,
    required this.controller,
    required this.agent,
    this.peer = '',
    this.enabled = true,
    this.onSubmit,
    this.note,
    this.keep,
    this.attach,
    this.files,
    this.holdsFiles = false,
    this.placeholder,
    this.above,
    this.focusNode,
  });

  final TextEditingController controller;

  /// Who gets it: the agent's name.
  final String agent;

  /// For a pipe's card, the other side ("Ana's Claktory"), which never
  /// sees it: said under the field; empty otherwise.
  final String peer;

  /// False while the decision is being sent.
  final bool enabled;

  /// Where words alone are the card's answer (Submit, Send, Answer): Return
  /// in the field does it, once something is typed.
  final VoidCallback? onSubmit;

  /// A line under the field, where the placeholder can't say it all (who
  /// never sees it); none when null or empty.
  final String? note;

  /// The field's placeholder, in place of "Reply to June".
  final String? placeholder;

  /// A caption over the field (N4b: the card changed while typing).
  final String? above;

  /// What a recording for it is kept as until its words come (#401).
  final DictationFor? keep;

  /// Files with the comment (#511): the paperclip, beside the mic, and the
  /// files waiting, under the field; none on cards that take no files.
  final Widget? attach;
  final Widget? files;

  /// Files wait to go with it.
  final bool holdsFiles;

  /// The field's focus, where the card puts the cursor in it itself (a
  /// step's first Can't do this); its own otherwise.
  final FocusNode? focusNode;

  @override
  State<DecisionComment> createState() => _DecisionCommentState();
}

class _DecisionCommentState extends State<DecisionComment> {
  final _own = FocusNode();

  FocusNode get _focus => widget.focusNode ?? _own;

  @override
  void dispose() {
    _own.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final submit = widget.onSubmit;
    void submitted() {
      if (widget.enabled && commentTyped(widget.controller)) submit!();
    }

    final note =
        widget.note ??
        (widget.peer.isEmpty
            ? ''
            : 'Only ${widget.agent} sees it, not ${widget.peer}.');
    // A limit on its height (a phone's card field over the keyboard,
    // CardFieldRoom) shrinks the field, which then scrolls inside; with
    // none, a bound too far to reach, as a flexible field needs one.
    return ConstrainedBox(
      constraints: const BoxConstraints(maxHeight: 100000),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          if (widget.above case final above? when above.isNotEmpty) ...[
            Text(above, style: type.caption.copyWith(color: palette.textMuted)),
            const SizedBox(height: Space.xs),
          ],
          // What's typed stays when the card scrolls away and back. Or
          // said: the mic puts what's said in, to check before sending.
          Flexible(
            child: KeepWhileTyped(
              controller: widget.controller,
              child: Dictation(
                controller: widget.controller,
                focusNode: _focus,
                enabled: widget.enabled,
                keep: widget.keep,
                trailing: widget.attach,
                // Bounded above: a far bound with no limit.
                shrinks: true,
                child: CallbackShortcuts(
                  bindings: {
                    if (submit != null) ...{
                      const SingleActivator(LogicalKeyboardKey.enter):
                          submitted,
                      const SingleActivator(LogicalKeyboardKey.numpadEnter):
                          submitted,
                    } else ...{
                      // Return decides nothing here: no new line either, as
                      // Shift+Return gives one.
                      const SingleActivator(LogicalKeyboardKey.enter): () {},
                      const SingleActivator(LogicalKeyboardKey.numpadEnter):
                          () {},
                    },
                  },
                  child: TextField(
                    controller: widget.controller,
                    focusNode: _focus,
                    placeholder:
                        widget.placeholder ?? replyPlaceholder(widget.agent),
                    // Named once typed in, when the placeholder is gone.
                    semanticsLabel: 'A comment for ${widget.agent}',
                    minLines: 1,
                    // Three on a phone at large text, so the title stays in
                    // sight over the keyboard.
                    maxLines: cardFieldMaxLines(context),
                    // As much as the backend takes.
                    inputFormatters: [
                      LengthLimitingTextInputFormatter(decisionCommentLimit),
                    ],
                  ),
                ),
              ),
            ),
          ),
          ?widget.files,
          if (note.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            Text(note, style: type.caption.copyWith(color: palette.textFaint)),
          ],
        ],
      ),
    );
  }
}

/// A card's action row (Lucy's Needs you cards, rule 2), the same on every
/// card: [line], saying what pressing does, at the left; then [quiet] (the
/// no, then any second action); then [primary], filled, last, the one
/// filled button on the card. With no [line] (a plain card waiting, whose
/// buttons say it), the buttons alone at the end. When they don't fit
/// beside the line, the line goes above and the buttons stay together.
class CardRow extends StatelessWidget {
  const CardRow({
    super.key,
    this.line,
    this.quietLine = false,
    this.quiet = const [],
    this.primary,
  });

  final Widget? line;

  /// [line] is only a hint, which steps aside on a phone at large text
  /// while the card's field has the keyboard ([CardActions.quietLine]).
  final bool quietLine;
  final List<Widget> quiet;
  final Widget? primary;

  // On a phone, the thumb row (Prosciutto's CardActions, §5.2).
  @override
  Widget build(BuildContext context) => CardActions(
    line: line,
    quietLine: quietLine,
    quiet: quiet,
    primary: primary,
  );
}

/// A card's field ([field], given the focus it's to use) and its [row],
/// one block: on a phone, kept in sight together above the keyboard while
/// the field has it (§5.2). [focus]: the field's, where the card moves it
/// itself; else the block's own.
class CardAnswerBlock extends StatefulWidget {
  const CardAnswerBlock({
    super.key,
    this.focus,
    required this.field,
    required this.row,
  });

  final FocusNode? focus;
  final Widget Function(FocusNode focus) field;
  final Widget row;

  @override
  State<CardAnswerBlock> createState() => _CardAnswerBlockState();
}

class _CardAnswerBlockState extends State<CardAnswerBlock> {
  final _own = FocusNode();

  @override
  void dispose() {
    _own.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final focus = widget.focus ?? _own;
    return KeepInSight(
      focus: focus,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          // Shrinks, to one line at least, where the buttons wouldn't
          // otherwise fit above the keyboard (Lucy, §5.2).
          CardFieldRoom(child: widget.field(focus)),
          const SizedBox(height: Space.s),
          widget.row,
        ],
      ),
    );
  }
}

/// The words of a [CardRow]'s line: caption, `textFaint` unless [color]
/// says otherwise (`danger` for a refusal).
class CardLine extends StatelessWidget {
  const CardLine(this.text, {super.key, this.color});

  final String text;
  final Color? color;

  @override
  Widget build(BuildContext context) => Text(
    text,
    style: TypeScale.of(context).caption
        .copyWith(color: color ?? Palette.of(context).textFaint),
  );
}

/// One quiet link under a card's row (rule 3: Other ways…, Edit, Resolve):
/// underlined at rest, so it reads as something to press before a pointer
/// finds it; a finger's height on a touch screen.
class QuietLink extends StatelessWidget {
  const QuietLink({super.key, required this.label, required this.onTap});

  final String label;

  /// Null while it can't be pressed (the card's answer on its way).
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Align(
      alignment: AlignmentDirectional.centerStart,
      widthFactor: 1,
      child: Semantics(
        button: true,
        label: label,
        child: Pressable(
          onTap: onTap,
          builder: (context, hovered) {
            final color = hovered && onTap != null
                ? palette.text
                : palette.textMuted;
            return ConstrainedBox(
              constraints: BoxConstraints(
                minHeight: touchFirst(context) ? Metrics.of(context).target : 0,
              ),
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: Space.xs),
                child: Align(
                  alignment: AlignmentDirectional.centerStart,
                  widthFactor: 1,
                  heightFactor: 1,
                  child: ExcludeSemantics(
                    child: Text(
                      label,
                      style: TypeScale.of(context).caption.copyWith(
                        color: color,
                        fontWeight: FontWeight.w500,
                        decoration: TextDecoration.underline,
                        decorationColor: color,
                      ),
                    ),
                  ),
                ),
              ),
            );
          },
        ),
      ),
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

/// The most lines a card's body shows before it folds its middle (Lucy's
/// Needs you cards): its first lines, Show all, then its last paragraph,
/// which stays in sight because agents usually put the question last.
const cardBodyFoldLines = 8;

/// A card's body ([body], Markdown, drawn a piece at a time by [build]),
/// folded as [cardBodyFoldLines] says when it runs longer, everywhere a
/// card shows. Find opens a fold its words are in.
class FoldedBody extends StatefulWidget {
  const FoldedBody({super.key, required this.body, required this.build});

  final String body;

  /// Draws a run of the body's paragraphs, as Markdown.
  final Widget Function(String text) build;

  @override
  State<FoldedBody> createState() => _FoldedBodyState();
}

class _FoldedBodyState extends State<FoldedBody> {
  var _open = false;

  @override
  Widget build(BuildContext context) {
    final whole = widget.build(widget.body);
    if (_open) return whole;
    final paragraphs = bodyParagraphs(widget.body);
    if (paragraphs.length < 3) return whole;
    final style = TypeScale.of(context).body;
    final scaler = MediaQuery.textScalerOf(context);
    final direction = Directionality.of(context);
    return LayoutBuilder(
      builder: (context, constraints) {
        int lines(String text) {
          if (text.trimLeft().startsWith('```')) {
            return '\n'.allMatches(text).length + 1;
          }
          final painter = TextPainter(
            text: TextSpan(text: text, style: style),
            textDirection: direction,
            textScaler: scaler,
          )..layout(maxWidth: constraints.maxWidth);
          final count = painter.computeLineMetrics().length;
          painter.dispose();
          return count;
        }

        final counts = [for (final each in paragraphs) lines(each)];
        if (counts.fold(0, (sum, n) => sum + n) <= cardBodyFoldLines) {
          return whole;
        }
        // The last paragraph stays, unless it's long itself; the first
        // ones as far as the fold leaves room, at least one.
        final last = counts.last <= cardBodyFoldLines ~/ 2 ? 1 : 0;
        final budget = cardBodyFoldLines - (last == 1 ? counts.last : 0) - 1;
        var room = budget;
        var head = 0;
        while (head < paragraphs.length - last - 1 &&
            (head == 0 || counts[head] <= room)) {
          room -= counts[head];
          head++;
        }
        final hidden = paragraphs.sublist(head, paragraphs.length - last);
        // Find's words in what's folded away: it opens.
        final find = context
            .dependOnInheritedWidgetOfExactType<FindHighlight>()
            ?.query
            .toLowerCase();
        if (find != null &&
            find.isNotEmpty &&
            hidden.any((each) => each.toLowerCase().contains(find))) {
          return whole;
        }
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          mainAxisSize: MainAxisSize.min,
          children: [
            // A first paragraph longer than the fold shows its first lines.
            if (head == 1 && counts.first > budget)
              ClipRect(
                child: ConstrainedBox(
                  constraints: BoxConstraints(
                    maxHeight:
                        scaler.scale(style.fontSize ?? 14) *
                        (style.height ?? 1.4) *
                        budget.clamp(2, cardBodyFoldLines),
                  ),
                  child: SingleChildScrollView(
                    physics: const NeverScrollableScrollPhysics(),
                    child: widget.build(paragraphs.first),
                  ),
                ),
              )
            else
              widget.build(paragraphs.sublist(0, head).join('\n\n')),
            QuietLink(
              label: 'Show all',
              onTap: () => setState(() => _open = true),
            ),
            if (last == 1) widget.build(paragraphs.last),
          ],
        );
      },
    );
  }
}

/// [body]'s paragraphs, split at blank lines outside code fences.
List<String> bodyParagraphs(String body) {
  final paragraphs = <String>[];
  final current = <String>[];
  var fenced = false;
  void end() {
    if (current.any((line) => line.trim().isNotEmpty)) {
      paragraphs.add(current.join('\n'));
    }
    current.clear();
  }

  for (final line in body.split('\n')) {
    if (line.trimLeft().startsWith('```')) fenced = !fenced;
    if (!fenced && line.trim().isEmpty) {
      end();
    } else {
      current.add(line);
    }
  }
  end();
  return paragraphs;
}
