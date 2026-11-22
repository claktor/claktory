import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../mobile/lines.dart' show ControlsLine;
import '../mobile/metrics.dart';
import '../user_apps/session.dart';
import 'chat.dart';
import 'contacts.dart' show PeoplePerson;
import 'problems.dart';
import 'review.dart';

// D3d: in a chat, above the box, what agents suggest there: a message one
// wants sent (Send, Edit, Decline) and a reply one drafted (Edit,
// Dismiss). The friend it goes to is this chat's, from the person's
// own conversations, never the proposal's word; the agent's name is its
// Claktory's, quoted as it says. Every hidden character shows by its code.

/// [text] as the person reviews it: as it is, each hidden character by
/// its code in Geist Mono `danger` ("‹U+202E›").
Widget reviewText(BuildContext context, String text, TextStyle style) {
  final palette = Palette.of(context);
  return Text.rich(
    TextSpan(
      children: [
        for (final run in reviewRuns(text))
          run.hidden
              ? TextSpan(
                  text: run.text,
                  style: TextStyle(
                    fontFamily: TextStyles.mono,
                    color: palette.danger,
                  ),
                )
              : TextSpan(text: run.text),
      ],
    ),
    style: style,
  );
}

/// What came of a decision that didn't go, in the person's words; the
/// agent that changed it named when its name shows as it is.
String reviewProblemText(String problem, {String agent = ''}) =>
    switch (problem) {
      'changed' when agent.trim().isNotEmpty && !hasHidden(agent) =>
        '$agent changed it since you read it. Check it again, then send.',
      'changed' => 'It changed since you read it. Check it again, then send.',
      'unsendable' => "This message can't be sent anymore.",
      _ => problemText(
        UserAppError(
          UserAppProblem.values
                  .where((each) => each.name == problem)
                  .firstOrNull ??
              UserAppProblem.invalid,
        ),
      ),
    };

/// A message an agent wants sent to [to], for the person to decide.
class ProposalCard extends StatelessWidget {
  const ProposalCard({
    super.key,
    required this.proposal,
    required this.to,
    required this.thread,
  });

  final DmProposal proposal;

  /// Whom it goes to: this chat's friend, with their tag.
  final PeoplePerson to;
  final ChatThread thread;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final busy = thread.deciding.contains(proposal.id);
    final hidden = hasHidden(proposal.text) || hasHidden(proposal.agentName);
    return _ReviewCard(
      heading: Text.rich(
        TextSpan(
          children: [
            ..._name(context, proposal.agentName),
            TextSpan(text: ' wants to send this to ${_to(to.name)}'),
            // Their tag, which they can't choose, as the chat's header:
            // a name alone could be anyone's (Fugo).
            if (to.tag.isNotEmpty)
              TextSpan(
                text: ' · ${to.tag}',
                style: TextStyle(color: palette.textFaint),
              ),
          ],
        ),
        style: type.caption.copyWith(
          color: palette.signal,
          fontWeight: FontWeight.w600,
        ),
      ),
      text: proposal.text,
      notes: [
        if (hidden)
          Text(
            'This message has hidden characters, shown in red as codes. '
            'Sending it sends them too.',
            style: type.caption.copyWith(color: palette.danger),
          ),
        Text(
          '${_to(to.name, start: true)} sees it as sent by your agent.',
          style: type.caption.copyWith(color: palette.textFaint),
        ),
      ],
      actions: [
        Button(
          label: 'Decline',
          onPressed: busy ? null : () => thread.decline(proposal),
        ),
        Button(
          label: 'Edit',
          onPressed: busy ? null : () => thread.useProposal(proposal),
        ),
        Button(
          label: 'Send',
          primary: true,
          onPressed: busy || !thread.canSend
              ? null
              : () => thread.approve(proposal),
        ),
      ],
    );
  }
}

/// A reply an agent drafted here, for the person to use or not.
class AgentDraftCard extends StatelessWidget {
  const AgentDraftCard({super.key, required this.draft, required this.thread});

  final DmAgentDraft draft;
  final ChatThread thread;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final busy = thread.deciding.contains(draft.id);
    return _ReviewCard(
      heading: Text.rich(
        TextSpan(
          children: [
            ..._name(context, draft.agentName),
            const TextSpan(text: ' suggests a reply'),
          ],
        ),
        style: type.caption.copyWith(
          color: palette.textMuted,
          fontWeight: FontWeight.w600,
        ),
      ),
      text: draft.text,
      notes: [
        if (hasHidden(draft.text))
          Text(
            'This reply has hidden characters, shown in red as codes.',
            style: type.caption.copyWith(color: palette.danger),
          ),
      ],
      actions: [
        Button(
          label: 'Dismiss',
          onPressed: busy ? null : () => thread.dismissAgentDraft(draft),
        ),
        Button(
          label: 'Edit',
          primary: true,
          onPressed: busy || !thread.canSend
              ? null
              : () => thread.useAgentDraft(draft),
        ),
      ],
    );
  }
}

/// "your friend" when the chat's friend has no name.
String _to(String name, {bool start = false}) =>
    name.isNotEmpty ? name : (start ? 'Your friend' : 'your friend');

/// The agent's name as its Claktory gave it, hidden characters by their
/// codes; "An agent" with none.
List<InlineSpan> _name(BuildContext context, String name) {
  if (name.trim().isEmpty) return const [TextSpan(text: 'An agent')];
  final palette = Palette.of(context);
  return [
    for (final run in reviewRuns(name))
      run.hidden
          ? TextSpan(
              text: run.text,
              style: TextStyle(
                fontFamily: TextStyles.mono,
                color: palette.danger,
              ),
            )
          : TextSpan(text: run.text),
  ];
}

/// The shape both share: a 10px-radius card on a 6% `signal` wash with a
/// 55% border, the heading, the text in a box of its own, the notes, then
/// the actions at the right (on a phone, stacked when they don't fit).
class _ReviewCard extends StatelessWidget {
  const _ReviewCard({
    required this.heading,
    required this.text,
    required this.notes,
    required this.actions,
  });

  final Widget heading;
  final String text;
  final List<Widget> notes;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: Space.s),
      child: Container(
        padding: const EdgeInsets.all(Space.m),
        decoration: BoxDecoration(
          color: palette.signal.withValues(alpha: 0.06),
          border: Border.all(color: palette.signal.withValues(alpha: 0.55)),
          borderRadius: BorderRadius.circular(Radii.panel),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            heading,
            const SizedBox(height: Space.s),
            Container(
              padding: const EdgeInsets.symmetric(
                horizontal: Space.m,
                vertical: Space.s,
              ),
              decoration: BoxDecoration(
                color: palette.canvas,
                border: Border.all(color: palette.hairline),
                borderRadius: BorderRadius.circular(Radii.control),
              ),
              child: reviewText(
                context,
                text,
                type.body.copyWith(color: palette.text),
              ),
            ),
            for (final note in notes) ...[
              const SizedBox(height: Space.xs),
              note,
            ],
            const SizedBox(height: Space.s),
            if (isPhoneLayout(context))
              ControlsLine(children: actions)
            else
              Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  for (final (i, action) in actions.indexed) ...[
                    if (i > 0) const SizedBox(width: Space.s),
                    action,
                  ],
                ],
              ),
          ],
        ),
      ),
    );
  }
}
