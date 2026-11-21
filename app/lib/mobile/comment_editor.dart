import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart' show TextInputAction;
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import 'haptics.dart';
import 'input.dart';
import 'metrics.dart';
import 'presentation.dart';

/// What a phone's comment editor writes (research mobile.md §4, "Comments
/// and annotations"): a new comment, a suggested change, a reply, or an
/// edit of the person's own comment.
enum CommentEditing { comment, suggest, reply, edit }

/// Writes a comment on a phone, in a sheet that rises over the file and
/// sits above the keyboard: what it's on at its top ([quote]: the passage,
/// the part of the image, the moment), who it goes to, the field, and one
/// primary button. Return sends it, as on the Mac; a suggestion's new text
/// takes new lines, and its comment is optional.
///
/// [submit] gets the words (and a suggestion's new text) and completes
/// with whether they went; the sheet closes once they have, and stays
/// with what failed otherwise ([error]). Closed with something written
/// and not sent, it asks first.
Future<void> showCommentEditor(
  BuildContext context, {
  required CommentEditing editing,
  required Widget quote,
  required String recipient,
  required Future<bool> Function(String body, String? replacement) submit,
  required ValueListenable<String?> error,
  String initial = '',
  String? replacing,
}) async {
  var dirty = false;
  await presentSheet<void>(
    context,
    shouldClose: () async {
      if (!dirty) return true;
      final discard = await showActions<bool>(
        context,
        title: 'Discard this comment?',
        actions: const [SheetAction('Discard', true, destructive: true)],
        cancelLabel: 'Keep editing',
      );
      return discard ?? false;
    },
    title: switch (editing) {
      CommentEditing.comment => 'Comment',
      CommentEditing.suggest => 'Suggest a change',
      CommentEditing.reply => 'Reply',
      CommentEditing.edit => 'Edit comment',
    },
    builder: (context, close) => CommentEditor(
      editing: editing,
      quote: quote,
      recipient: recipient,
      initial: initial,
      replacing: replacing,
      error: error,
      changed: (changed) => dirty = changed,
      submit: (body, replacement) async {
        final ok = await submit(body, replacement);
        if (ok) close();
        return ok;
      },
    ),
  );
}

/// The comment editor's body, in its sheet ([showCommentEditor]).
class CommentEditor extends StatefulWidget {
  const CommentEditor({
    super.key,
    required this.editing,
    required this.quote,
    required this.recipient,
    required this.submit,
    required this.error,
    this.initial = '',
    this.replacing,
    this.changed,
  });

  final CommentEditing editing;
  final Widget quote;

  /// The agent's name it goes to; empty for an edit, which only saves.
  final String recipient;
  final Future<bool> Function(String body, String? replacement) submit;
  final ValueListenable<String?> error;
  final String initial;

  /// A suggestion: the text it replaces, which its new text starts from.
  final String? replacing;

  /// Whether something's written that would be lost, as it changes.
  final ValueChanged<bool>? changed;

  @override
  State<CommentEditor> createState() => _CommentEditorState();
}

class _CommentEditorState extends State<CommentEditor> {
  late final _text = TextEditingController(text: widget.initial)
    ..addListener(_changed);
  late final _replacement = widget.replacing == null
      ? null
      : (TextEditingController(text: widget.replacing)..addListener(_changed));
  var _busy = false;

  bool get _suggesting => _replacement != null;

  void _changed() => widget.changed?.call(
    (_text.text.trim().isNotEmpty && _text.text != widget.initial) ||
        (_replacement != null && _replacement.text != widget.replacing),
  );

  @override
  void dispose() {
    _text.dispose();
    _replacement?.dispose();
    super.dispose();
  }

  /// A suggestion needs a change; anything else, words.
  bool get _ready {
    final replacement = _replacement;
    if (replacement == null) return _text.text.trim().isNotEmpty;
    return replacement.text != widget.replacing;
  }

  Future<void> _submit() async {
    if (_busy || !_ready) return;
    setState(() => _busy = true);
    final ok = await widget.submit(_text.text, _replacement?.text);
    if (!mounted) return;
    setState(() => _busy = false);
    if (ok) unawaited(Haptics.light());
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final replacement = _replacement;
    final label = switch (widget.editing) {
      CommentEditing.comment => 'Comment',
      CommentEditing.suggest => 'Suggest',
      CommentEditing.reply => 'Reply',
      CommentEditing.edit => 'Save',
    };
    return SingleChildScrollView(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          widget.quote,
          const SizedBox(height: Space.m),
          if (widget.recipient.isNotEmpty) ...[
            Text(
              'To ${widget.recipient}',
              style: type.caption.copyWith(color: palette.textMuted),
            ),
            const SizedBox(height: Space.xs),
          ],
          if (replacement != null) ...[
            TextField(
              controller: replacement,
              placeholder: 'New text',
              kind: InputKind.multiline,
              minLines: 2,
              maxLines: 6,
              autofocus: true,
            ),
            const SizedBox(height: Space.s),
          ],
          TextField(
            controller: _text,
            placeholder: _suggesting
                ? 'Comment (optional)'
                : widget.editing == CommentEditing.reply
                ? 'Reply'
                : 'Comment',
            kind: InputKind.message,
            // Return sends, as on the Mac: a comment is a line or two.
            textInputAction: TextInputAction.send,
            onSubmitted: (_) => unawaited(_submit()),
            minLines: _suggesting ? 1 : 2,
            maxLines: 6,
            autofocus: !_suggesting,
          ),
          ValueListenableBuilder(
            valueListenable: widget.error,
            builder: (context, error, _) => error == null
                ? const SizedBox(height: Space.m)
                : Padding(
                    padding: const EdgeInsets.symmetric(vertical: Space.s),
                    child: Text(
                      error,
                      style: type.secondary.copyWith(color: palette.danger),
                    ),
                  ),
          ),
          ListenableBuilder(
            listenable: Listenable.merge([_text, ?replacement]),
            builder: (context, _) => Button(
              label: label,
              primary: true,
              onPressed: _busy || !_ready ? null : () => unawaited(_submit()),
            ),
          ),
        ],
      ),
    );
  }
}

/// The passage a comment is on, as the editor and a thread show it: in
/// quotes, or struck through for a suggestion's text it replaces.
class QuotedPassage extends StatelessWidget {
  const QuotedPassage(
    this.text, {
    super.key,
    this.struck = false,
    this.maxLines = 4,
  });

  final String text;
  final bool struck;
  final int maxLines;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Container(
      padding: const EdgeInsetsDirectional.only(start: Space.s),
      decoration: BoxDecoration(
        border: BorderDirectional(
          start: BorderSide(color: palette.signal, width: 3),
        ),
      ),
      child: Text(
        struck ? text.trim() : '“${text.trim()}”',
        maxLines: maxLines,
        overflow: TextOverflow.ellipsis,
        style: type.secondary.copyWith(
          color: palette.textMuted,
          fontStyle: struck ? null : FontStyle.italic,
          decoration: struck ? TextDecoration.lineThrough : null,
        ),
      ),
    );
  }
}
