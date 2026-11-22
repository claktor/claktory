import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'avatar.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'decision.dart';
import 'dictation.dart';
import 'explorer.dart' show shortPath;
import 'extensions.dart' show logTime;
import 'identity.dart';
import 'ipad/workbench.dart' show WorkbenchScope;
import 'layout.dart' show layoutOf, open;
import 'markdown.dart';
import 'mobile/lines.dart';
import 'mobile/metrics.dart';
import 'mobile/navigation.dart' show PhoneRoutes;
import 'protocol/factory/v1/factory.pb.dart';
import 'review_answer.dart' show rememberReviewReturn;
import 'scope.dart';
import 'selection.dart' show SelectableArea;
import 'taint_label.dart';
import 'visible_codes.dart' show proseCodes, visibleCodes;

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

  /// The written answer's focus, or a step's what's in the way: never
  /// both on the card at once.
  final _focus = FocusNode(debugLabel: 'Ask answer');
  var _busy = false;
  String? _error;

  /// A procedure's Can't do this… was chosen: its comment field and the
  /// button that sends it show instead of Done.
  var _cant = false;

  /// Can't do this…: its field opens with the keys, so what's typed lands
  /// in it at once, as Add a comment's does (Abbacchio).
  void _openCant() {
    setState(() => _cant = true);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _cant) _focus.requestFocus();
    });
  }

  /// A procedure's finished steps are listed, not only counted.
  var _showSteps = false;

  @override
  void didUpdateWidget(AskPersonCard old) {
    super.didUpdateWidget(old);
    // The next step: a clean card for it.
    if (currentStep(old.ask) != currentStep(widget.ask)) {
      _text.clear();
      _cant = false;
      _error = null;
    }
  }

  @override
  void dispose() {
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  Future<void> _answer({required bool dismiss, int? step}) async {
    final text = _text.text.trim();
    if (_busy ||
        (step == null && !dismiss && widget.ask.wantsText && text.isEmpty)) {
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).answerAsk(
        widget.agentId,
        widget.seq,
        // Dismissed, what's typed goes too, as the person's comment.
        text: text,
        dismiss: dismiss,
        step: step,
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
                  ask.steps.isEmpty
                      ? '$name needs your answer'
                      : '$name needs you to do something',
                  style: type.caption.copyWith(
                    color: color,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          // Where the question may come from, when its agent read a pipe.
          TaintLabel(taint: ask.taint, agent: name),
          const SizedBox(height: Space.s),
          // What the agent wrote, to decide on: what reorders text shows by
          // its code (proseCodes, Fugo).
          Text(
            proseCodes(ask.title),
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
                  : Markdown(proseCodes(part.text), color: palette.textMuted),
            ],
          ],
          // A document it asks the person to review, with Review.
          if (ask.reviewPath.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            _ReviewFile(
              path: ask.reviewPath,
              onReview: () =>
                  openReview(context, widget.agentId, ask.reviewPath),
            ),
          ],
          const SizedBox(height: Space.m),
          if (ask.steps.isNotEmpty)
            ..._procedure(palette, type, color, name, pending, decided)
          else if (pending)
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
      // A written answer has its box; an approval, a comment under its
      // buttons for either.
      if (wantsText) ...[
        // Or said: the mic puts what's said in, to check before sending.
        Dictation(
          controller: _text,
          focusNode: _focus,
          enabled: !_busy,
          child: CallbackShortcuts(
            bindings: {
              const SingleActivator(LogicalKeyboardKey.enter): () =>
                  _answer(dismiss: false),
            },
            child: TextField(
              controller: _text,
              focusNode: _focus,
              placeholder: 'Your answer',
              minLines: 1,
              maxLines: 8,
            ),
          ),
        ),
        const SizedBox(height: Space.s),
      ],
      // On a phone at large text the buttons go under the words.
      ActionLine(
        lead: Text(
          _error ??
              (wantsText
                  ? '$name gets your answer and carries on.'
                  : widget.ask.reviewPath.isNotEmpty
                  ? 'Your comments on it go to $name.'
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
      if (!wantsText)
        DecisionComment(
          controller: _text,
          agent: name,
          enabled: !_busy,
          onSubmitted: () => _answer(dismiss: false),
        ),
    ];
  }

  /// A procedure (ask_person's steps): one step at a time, "Step 2 of 6",
  /// with Done and Can't do this…, the steps finished counted above it
  /// and listed on request; decided, how it ended, with every step.
  List<Widget> _procedure(
    Palette palette,
    TypeScale type,
    Color color,
    String name,
    bool pending,
    int decided,
  ) {
    final ask = widget.ask;
    final steps = ask.steps;
    final at = currentStep(ask);
    final finished = [
      for (final (i, step) in steps.indexed)
        if (!_open(step)) i,
    ];
    if (pending && at != null) {
      final step = steps[at];
      return [
        if (finished.isNotEmpty) ...[
          _StepsToggle(
            label: finished.length == 1
                ? '1 step done'
                : '${finished.length} steps done',
            open: _showSteps,
            onTap: () => setState(() => _showSteps = !_showSteps),
          ),
          if (_showSteps)
            for (final i in finished) _StepLine(index: i, step: steps[i]),
          const SizedBox(height: Space.s),
        ],
        Text(
          'Step ${at + 1} of ${steps.length}',
          style: type.caption.copyWith(
            color: color,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: Space.xs),
        for (final (i, part) in askBodyParts(step.text).indexed) ...[
          if (i > 0) const SizedBox(height: Space.s),
          part.command
              ? _Command(part.text)
              : Markdown(proseCodes(part.text), color: palette.text),
        ],
        const SizedBox(height: Space.m),
        if (_cant) ...[
          // What's in the way, for the agent to decide what's next; typed
          // or said.
          Dictation(
            controller: _text,
            focusNode: _focus,
            enabled: !_busy,
            child: TextField(
              controller: _text,
              focusNode: _focus,
              placeholder: "What's in the way? (optional)",
              minLines: 1,
              maxLines: 6,
            ),
          ),
          const SizedBox(height: Space.s),
          ActionLine(
            lead: Text(
              _error ?? '$name hears why and decides what to do next.',
              style: type.caption.copyWith(
                color: _error == null ? palette.textFaint : palette.danger,
              ),
            ),
            action: ControlsLine(
              children: [
                Button(
                  label: 'Back',
                  onPressed: _busy ? null : () => setState(() => _cant = false),
                ),
                Button(
                  label: "Can't do this",
                  primary: true,
                  onPressed: _busy
                      ? null
                      : () => _answer(dismiss: true, step: at),
                ),
              ],
            ),
          ),
        ] else ...[
          ActionLine(
            lead: Text(
              _error ??
                  (at + 1 < steps.length
                      ? 'The next step shows once this one is done.'
                      : '$name carries on once this is done.'),
              style: type.caption.copyWith(
                color: _error == null ? palette.textFaint : palette.danger,
              ),
            ),
            action: ControlsLine(
              children: [
                Button(
                  label: "Can't do this…",
                  onPressed: _busy ? null : _openCant,
                ),
                Button(
                  label: 'Done',
                  primary: true,
                  onPressed: _busy
                      ? null
                      : () => _answer(dismiss: false, step: at),
                ),
              ],
            ),
          ),
          DecisionComment(
            controller: _text,
            agent: name,
            enabled: !_busy,
            onSubmitted: () => _answer(dismiss: false, step: at),
          ),
        ],
      ];
    }
    // Decided: every step done, stopped at one, or left to the agent.
    final stopped = steps.indexed
        .where((each) => each.$2.state == AskStepState.ASK_STEP_STATE_CANT)
        .firstOrNull;
    final allDone =
        ask.state == AskPersonState.ASK_PERSON_STATE_ANSWERED &&
        stopped == null;
    final outcome = allDone
        ? steps.length == 2
              ? 'Both steps done'
              : 'All ${steps.length} steps done'
        : stopped != null
        ? "Couldn't do step ${stopped.$1 + 1} of ${steps.length}"
        : 'Left to $name';
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
                    text: outcome,
                    style: TextStyle(
                      color: allDone ? color : palette.textMuted,
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
      // What was in the way.
      if (stopped != null && stopped.$2.comment.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(
          stopped.$2.comment,
          style: type.body.copyWith(color: palette.textMuted),
        ),
      ],
      const SizedBox(height: Space.xs),
      _StepsToggle(
        label: 'Steps',
        open: _showSteps,
        onTap: () => setState(() => _showSteps = !_showSteps),
      ),
      if (_showSteps)
        for (final (i, step) in steps.indexed)
          _StepLine(index: i, step: step, reached: !_open(step)),
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
      // The answer, or the comment it was dismissed with.
      if (ask.answer.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(
          ask.answer,
          style: type.body.copyWith(
            color: dismissed ? palette.textMuted : palette.text,
          ),
        ),
      ],
    ];
  }
}

/// A procedure's step not answered yet.
bool _open(AskStep step) =>
    step.state == AskStepState.ASK_STEP_STATE_PENDING ||
    step.state == AskStepState.ASK_STEP_STATE_UNSPECIFIED;

/// The step [ask] shows now, 0-based: its first not answered while it
/// waits; null when it has no steps or isn't waiting.
int? currentStep(AskPerson ask) {
  if (ask.state != AskPersonState.ASK_PERSON_STATE_PENDING &&
      ask.state != AskPersonState.ASK_PERSON_STATE_UNSPECIFIED) {
    return null;
  }
  for (final (i, step) in ask.steps.indexed) {
    if (_open(step)) return i;
  }
  return null;
}

/// What a waiting ask says in a line (Needs you, the agent's list): its
/// title, and for a procedure the step it's at.
String askSummary(AskPerson ask) => switch (currentStep(ask)) {
  final at? => '${ask.title} · Step ${at + 1} of ${ask.steps.length}',
  null => ask.title,
};

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
    // Code an agent wrote, shown as tool calls show it: every hidden
    // character by its code (Fugo). Copy copies it as it is.
    lead: CodeBlock(visibleCodes(widget.command)),
    action: Button(
      label: _copied ? 'Copied' : 'Copy',
      onPressed: () {
        Clipboard.setData(ClipboardData(text: widget.command));
        setState(() => _copied = true);
      },
    ),
  );
}

/// Opens the file [agentId] asked the person to review ([path]) in that
/// agent's conversation, expanded to fill the window, so every comment
/// they leave on it goes to that agent. On an iPad's workbench, its pane
/// is the one shown.
void openReview(BuildContext context, String agentId, String path) {
  // On a phone, a page over what shows (Needs you stays under it, for
  // Back), with the review's bar at its foot.
  if (PhoneRoutes.of(context)?.showFile(agentId, path) ?? false) return;
  // Answered there, back here.
  rememberReviewReturn(context, agentId, path);
  final scope = AppScope.scope(context);
  final backend = scope.backend;
  if (backend.workspace.agent(agentId) == null) return;
  final workbench = context
      .getInheritedWidgetOfExactType<WorkbenchScope>()
      ?.notifier;
  // From Needs you or Settings, back to the conversation.
  if (scope.page.value != AppPage.agent) AppScope.closePage(context);
  fireAndLog(() async {
    if (backend.workspace.workspace.selectedAgentId != agentId) {
      await backend.selectAgent(agentId);
    }
    final agent = backend.workspace.agent(agentId);
    if (agent == null) return;
    final opened = open(layoutOf(agent), path);
    workbench?.focus(agentId, opened.paneId);
    // Open already and expanded: nothing to change.
    if (agent.layoutFocus == opened.paneId) return;
    await backend.setLayout(agentId, opened.root, focus: opened.paneId);
  }());
}

/// The file a review ask is about: its glyph, name and folder, and Review,
/// which opens it beside the asking agent's conversation, expanded.
class _ReviewFile extends StatelessWidget {
  const _ReviewFile({required this.path, required this.onReview});

  final String path;
  final VoidCallback onReview;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final slash = path.lastIndexOf('/');
    final name = slash < 0 ? path : path.substring(slash + 1);
    final folder = slash <= 0 ? '' : shortPath(path.substring(0, slash));
    return Container(
      padding: const EdgeInsets.fromLTRB(Space.m, Space.s, Space.s, Space.s),
      decoration: BoxDecoration(
        color: palette.canvas,
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
      child: ActionLine(
        lead: Row(
          children: [
            FileGlyph(color: palette.textMuted),
            const SizedBox(width: Space.s),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: type.label.copyWith(color: palette.text),
                  ),
                  if (folder.isNotEmpty)
                    Text(
                      folder,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: type.caption.copyWith(color: palette.textFaint),
                    ),
                ],
              ),
            ),
          ],
        ),
        action: Button(label: 'Review', onPressed: onReview),
      ),
    );
  }
}

/// Shows or hides a procedure's steps: [label], and whether they show.
class _StepsToggle extends StatelessWidget {
  const _StepsToggle({
    required this.label,
    required this.open,
    required this.onTap,
  });

  final String label;
  final bool open;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Align(
      alignment: AlignmentDirectional.centerStart,
      child: Semantics(
        button: true,
        expanded: open,
        child: Pressable(
          onTap: onTap,
          builder: (context, hovered) => Container(
            // A finger's height on a phone or iPad, the words centred in
            // it (Prosciutto: not a gap under them).
            constraints: BoxConstraints(
              minHeight: touchFirst(context) ? Metrics.of(context).target : 0,
            ),
            alignment: AlignmentDirectional.centerStart,
            // One text, so it wraps at the largest sizes.
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: label,
                    style: TextStyle(
                      color: hovered ? palette.text : palette.textMuted,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  TextSpan(
                    text: open ? '  Hide' : '  Show',
                    style: TextStyle(color: palette.textFaint),
                  ),
                ],
              ),
              style: type.caption,
            ),
          ),
        ),
      ),
    );
  }
}

/// One of a procedure's steps, small: its number and words, how it went
/// (done, couldn't do, not reached), and the person's comment on it.
class _StepLine extends StatelessWidget {
  const _StepLine({required this.index, required this.step, this.reached});

  final int index;
  final AskStep step;

  /// Whether it was answered; null where only answered ones are listed.
  final bool? reached;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final outcome = switch (step.state) {
      AskStepState.ASK_STEP_STATE_DONE => 'Done',
      AskStepState.ASK_STEP_STATE_CANT => "Couldn't do",
      _ => reached == false ? 'Not reached' : '',
    };
    // The whole step on one line, its command too ("Open Terminal and run:
    // ykman info"), without a code block's fences.
    final first = proseCodes(
      [
        for (final line in step.text.split('\n'))
          if (line.trim().isNotEmpty && !line.trim().startsWith('```'))
            line.trim(),
      ].join(' '),
    );
    return Padding(
      padding: const EdgeInsets.only(top: Space.xs),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text.rich(
            TextSpan(
              children: [
                TextSpan(text: '${index + 1}. $first'),
                if (outcome.isNotEmpty)
                  TextSpan(
                    text: ' · $outcome',
                    style: TextStyle(
                      color: step.state == AskStepState.ASK_STEP_STATE_CANT
                          ? palette.danger
                          : palette.textFaint,
                    ),
                  ),
              ],
            ),
            style: type.caption.copyWith(color: palette.textMuted),
          ),
          if (step.comment.isNotEmpty)
            Padding(
              padding: const EdgeInsetsDirectional.only(start: Space.m),
              child: Text(
                step.comment,
                style: type.caption.copyWith(color: palette.textFaint),
              ),
            ),
        ],
      ),
    );
  }
}
