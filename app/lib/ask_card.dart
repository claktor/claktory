import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'avatar.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'extensions.dart' show logTime;
import 'identity.dart';
import 'markdown.dart';
import 'mobile/lines.dart';
import 'mobile/metrics.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart' show SelectableArea;

/// An agent's question for the person, one it can't go on without: what it
/// needs (its title) and why (the body, Markdown), and the person's answer
/// (written, when it asks for one) or approval, or Dismiss, which leaves it
/// to the agent's judgment. A sibling of the proposal card, in the agent's
/// color. Its words select and copy like the conversation's, wherever it
/// shows, and a command in the body (a fenced code block) has Copy beside
/// it. The backend sends the entry back answered; the card keeps nothing
/// but what's being typed.
class AskPersonCard extends StatefulWidget {
  const AskPersonCard(
    this.ask, {
    super.key,
    required this.agentId,
    required this.seq,
  });

  final AskPerson ask;
  final String agentId;

  /// The entry's seq in the agent's conversation.
  final int seq;

  @override
  State<AskPersonCard> createState() => _AskPersonCardState();
}

class _AskPersonCardState extends State<AskPersonCard> {
  final _text = TextEditingController();
  var _busy = false;
  String? _error;

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _answer({required bool dismiss}) async {
    final text = _text.text.trim();
    if (_busy || (!dismiss && widget.ask.wantsText && text.isEmpty)) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).answerAsk(
        widget.agentId,
        widget.seq,
        text: dismiss ? '' : text,
        dismiss: dismiss,
      );
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final ask = widget.ask;
    final agent = AppScope.of(context).workspace.agent(widget.agentId);
    final color = agentColor(agent?.color ?? 'slate');
    final name = agent?.name ?? 'The agent';
    final pending =
        ask.state == AskPersonState.ASK_PERSON_STATE_PENDING ||
        ask.state == AskPersonState.ASK_PERSON_STATE_UNSPECIFIED;
    final dismissed = ask.state == AskPersonState.ASK_PERSON_STATE_DISMISSED;
    final decided = ask.decidedMs.toInt();
    final card = Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.06),
        border: Border.all(
          color: color.withValues(alpha: pending ? 0.55 : 0.25),
        ),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              if (agent != null) ...[
                AgentCharacter(agent: agent, size: 20),
                const SizedBox(width: Space.xs + 2),
              ],
              Flexible(
                child: Text(
                  '$name needs your answer',
                  style: type.caption.copyWith(
                    color: color,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.s),
          Text(
            ask.title,
            style: type.emphasis.copyWith(
              color: dismissed ? palette.textMuted : palette.text,
            ),
          ),
          if (ask.body.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            for (final (i, part) in askBodyParts(ask.body).indexed) ...[
              if (i > 0) const SizedBox(height: Space.s),
              part.command
                  ? _Command(part.text)
                  : Markdown(part.text, color: palette.textMuted),
            ],
          ],
          const SizedBox(height: Space.m),
          if (pending)
            ..._pending(palette, type, name)
          else
            ..._decided(palette, type, color, name, dismissed, decided),
        ],
      ),
    );
    // In a conversation its words are the conversation's to select; in
    // Needs you, where nothing around it is, its own (a selection needs an
    // overlay for its menu, which the app always has).
    if (SelectionContainer.maybeOf(context) != null ||
        Overlay.maybeOf(context) == null) {
      return card;
    }
    return SelectableArea(child: card);
  }

  List<Widget> _pending(Palette palette, TypeScale type, String name) {
    final wantsText = widget.ask.wantsText;
    return [
      CallbackShortcuts(
        bindings: {
          const SingleActivator(LogicalKeyboardKey.enter): () =>
              _answer(dismiss: false),
        },
        child: TextField(
          controller: _text,
          placeholder: wantsText
              ? 'Your answer'
              : 'A comment for $name, if you like',
          minLines: 1,
          maxLines: 8,
        ),
      ),
      const SizedBox(height: Space.s),
      // On a phone at large text the buttons go under the words.
      ActionLine(
        lead: Text(
          _error ??
              (wantsText
                  ? '$name gets your answer and carries on.'
                  : '$name carries on once you approve.'),
          style: type.caption.copyWith(
            color: _error == null ? palette.textFaint : palette.danger,
          ),
        ),
        action: ControlsLine(
          children: [
            Button(
              label: 'Dismiss',
              onPressed: _busy ? null : () => _answer(dismiss: true),
            ),
            ListenableBuilder(
              listenable: _text,
              builder: (context, _) => Button(
                label: wantsText ? 'Answer' : 'Approve',
                primary: true,
                onPressed: _busy || (wantsText && _text.text.trim().isEmpty)
                    ? null
                    : () => _answer(dismiss: false),
              ),
            ),
          ],
        ),
      ),
    ];
  }

  List<Widget> _decided(
    Palette palette,
    TypeScale type,
    Color color,
    String name,
    bool dismissed,
    int decided,
  ) {
    final ask = widget.ask;
    return [
      Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const PersonAvatar(size: 16),
          const SizedBox(width: Space.xs + 2),
          Flexible(
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: dismissed
                        ? 'Left to $name'
                        : ask.wantsText
                        ? 'Answered'
                        : 'Approved',
                    style: TextStyle(
                      color: dismissed ? palette.textMuted : color,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  if (decided > 0)
                    TextSpan(
                      text:
                          ' · ${logTime(DateTime.fromMillisecondsSinceEpoch(decided))}',
                      style: TextStyle(color: palette.textFaint),
                    ),
                ],
              ),
              style: type.caption,
            ),
          ),
        ],
      ),
      if (!dismissed && ask.answer.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(ask.answer, style: type.body.copyWith(color: palette.text)),
      ],
    ];
  }
}

/// A part of an ask's body: Markdown, or a command (a fenced code block's
/// contents) to show with Copy.
typedef AskBodyPart = ({String text, bool command});

/// [body] split at its fenced code blocks (``` or ~~~), in order; the
/// Markdown between them as it is.
List<AskBodyPart> askBodyParts(String body) {
  final fence = RegExp(
    r'^ {0,3}(`{3,}|~{3,})[^\n]*\n([\s\S]*?)^ {0,3}\1[ \t]*$',
    multiLine: true,
  );
  final parts = <AskBodyPart>[];
  var at = 0;
  for (final match in fence.allMatches(body)) {
    final before = body.substring(at, match.start).trim();
    if (before.isNotEmpty) parts.add((text: before, command: false));
    final code = match.group(2)!.replaceFirst(RegExp(r'\n$'), '');
    if (code.trim().isNotEmpty) parts.add((text: code, command: true));
    at = match.end;
  }
  final rest = body.substring(at).trim();
  if (rest.isNotEmpty) parts.add((text: rest, command: false));
  return parts;
}

/// A command from an ask's body in a code block, with Copy at its end (on
/// a phone at large text, under it): "Copied" once it's on the clipboard.
class _Command extends StatefulWidget {
  const _Command(this.command);

  final String command;

  @override
  State<_Command> createState() => _CommandState();
}

class _CommandState extends State<_Command> {
  var _copied = false;

  @override
  Widget build(BuildContext context) => ActionLine(
    lead: CodeBlock(widget.command),
    action: Button(
      label: _copied ? 'Copied' : 'Copy',
      onPressed: () {
        Clipboard.setData(ClipboardData(text: widget.command));
        setState(() => _copied = true);
      },
    ),
  );
}
