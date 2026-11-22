import 'dart:async';
import 'dart:convert';

import 'package:flutter/widgets.dart';

import 'agent_view.dart' show ArchivedLine, ReconnectingLine, WorkingLine;
import 'attachments.dart' show LocalFile;
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'identity.dart';
import 'message_box.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'pipes.dart';
import 'mobile/pipe_target.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'transcript.dart';
import 'usage_wait.dart';

/// The agent bound to [pipe] (Agent.pipe_id): the liaison made for it,
/// whose conversation the pipe's view shows with the pipe's messages.
/// Null when it has none.
Agent? pipeLiaison(Workspace workspace, Pipe pipe) {
  for (final agent in workspace.agents) {
    if (agent.pipeId == pipe.id) return agent;
  }
  return null;
}

/// Whom a pipe's message box writes to.
enum PipeTarget {
  /// The pipe's liaison, as in its own conversation: only it sees it.
  liaison,

  /// The other side, through the pipe, as the person's own message.
  pipe,
}

/// Whom the box of a pipe with a liaison writes to: the liaison unless the
/// person picks the pipe, and the liaison again after every message, so
/// nothing goes to the other side by habit. A phone's chooser drives the
/// same one.
class PipeTargets extends ValueNotifier<PipeTarget> {
  PipeTargets() : super(PipeTarget.liaison);

  /// Back to the liaison.
  void reset() => value = PipeTarget.liaison;
}

/// The box's key for writing through [pipeId]: its draft's.
String pipeBoxKey(String pipeId) => 'pipe:$pipeId';

/// The box writing through a pipe: text only, as the person's own message
/// to the other side; blocked while the pipe isn't open.
class PipeMessageTarget extends MessageTarget {
  PipeMessageTarget(this.pipe, this.backend, {this.onFailed});

  final Pipe pipe;
  final Backend backend;

  /// A message that couldn't go: its text, and why.
  final void Function(String text, String why)? onFailed;

  @override
  String get key => pipeBoxKey(pipe.id);

  @override
  String get placeholder => 'Message ${pipePeerClaktory(pipe)}';

  @override
  bool get takesFiles => false;

  @override
  bool get takesVoice => false;

  /// Send in the pipe's color while the box writes through it.
  @override
  Color get accent => pipeColor;

  @override
  MessageBlocked? blocked(BuildContext context) =>
      pipe.state == PipeState.PIPE_STATE_OPEN
      ? null
      : MessageBlocked(pipeStatus(pipe));

  @override
  void send(String text, {List<LocalFile> files = const [], Duration? voice}) {
    if (text.isEmpty) return;
    unawaited(
      backend.sendPipeMessage(pipe.id, text).catchError((Object error) {
        onFailed?.call(
          text,
          error is RequestFailure ? error.message : 'It couldn’t be sent.',
        );
      }),
    );
  }
}

/// The pipe's seq a `pipe_send` call sent or held (its details' "seq"),
/// else null.
int? pipeSendSeq(ToolCall call) {
  if (call.name != 'pipe_send' || call.details.isEmpty) return null;
  try {
    final details = jsonDecode(call.details);
    if (details is Map && details['seq'] is int) return details['seq'] as int;
  } on FormatException {
    return null;
  }
  return null;
}

/// Whether [entry] says only what one of the pipe's cards already shows:
/// the pipe's wakes (each message from the other side is its card), and a
/// `pipe_send` whose message has its card among [seqs].
bool hiddenInPipe(Entry entry, Set<int> seqs) {
  if (entry.hasEvent()) {
    return entry.event.source == EventSource.EVENT_SOURCE_PIPE;
  }
  if (entry.hasToolCall()) {
    final seq = pipeSendSeq(entry.toolCall);
    return seq != null && seqs.contains(seq);
  }
  return false;
}

/// A pipe with its liaison: the liaison's conversation with the pipe's
/// messages among it as cards, in time order, and under it the box, which
/// writes to the liaison or through the pipe.
class LiaisonPipeBody extends StatefulWidget {
  const LiaisonPipeBody({
    super.key,
    required this.pipe,
    required this.liaison,
    this.phone = false,
  });

  final Pipe pipe;
  final Agent liaison;

  /// The screen of a phone: the box takes at most part of it.
  final bool phone;

  @override
  State<LiaisonPipeBody> createState() => _LiaisonPipeBodyState();
}

class _LiaisonPipeBodyState extends State<LiaisonPipeBody> {
  final _follow = ValueNotifier<int>(0);
  final _targets = PipeTargets();
  final _extras = ValueNotifier<List<TimelineItem>>(const []);

  /// The seqs of the pipe's messages held, for [hiddenInPipe].
  var _seqs = <int>{};

  /// Why the last message through the pipe didn't go.
  final _failed = ValueNotifier<String?>(null);

  late final Backend _backend;
  var _filling = false;

  @override
  void initState() {
    super.initState();
    _backend = AppScope.of(context);
    _backend.pipeChat.addListener(_chatChanged);
    _backend.transcript.addListener(_scheduleFill);
    _chatChanged();
  }

  @override
  void didUpdateWidget(LiaisonPipeBody old) {
    super.didUpdateWidget(old);
    // A message held for the person, or the pipe pausing, changes what the
    // cards and the box show.
    if (old.pipe != widget.pipe) _chatChanged();
  }

  @override
  void dispose() {
    _backend.pipeChat.removeListener(_chatChanged);
    _backend.transcript.removeListener(_scheduleFill);
    _follow.dispose();
    _targets.dispose();
    _extras.dispose();
    _failed.dispose();
    super.dispose();
  }

  void _chatChanged() {
    final chat = _backend.pipeChat;
    final messages = chat.pipeId == widget.pipe.id
        ? chat.messages
        : const <PipeMessage>[];
    _seqs = {for (final message in messages) message.seq.toInt()};
    final pipe = widget.pipe;
    _extras.value = [
      for (final message in messages)
        TimelineItem(
          key: pipeItemKey(message.seq.toInt()),
          timestampMs: message.timestampMs.toInt(),
          build: (context) => PipeMessageView(
            key: ValueKey(message.seq),
            pipe: pipe,
            message: message,
            framed: true,
          ),
        ),
    ]..sort((a, b) => a.timestampMs.compareTo(b.timestampMs));
    _scheduleFill();
  }

  void _scheduleFill() {
    if (_filling) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) unawaited(_fill());
    });
  }

  /// Loads the pipe's earlier messages back as far as the conversation
  /// goes, and all of them once it has nothing earlier: a card older than
  /// every entry shows only then, so neither log leaves a gap in the other.
  Future<void> _fill() async {
    if (_filling) return;
    _filling = true;
    try {
      while (mounted) {
        final chat = _backend.pipeChat;
        final transcript = _backend.transcript;
        if (chat.pipeId != widget.pipe.id ||
            transcript.agentId != widget.liaison.id ||
            !chat.hasEarlier ||
            chat.messages.isEmpty) {
          return;
        }
        final reaches =
            transcript.hasEarlier &&
            transcript.length > 0 &&
            chat.messages.first.timestampMs.toInt() <=
                transcript[0].entry.timestampMs.toInt();
        if (reaches) return;
        final before = chat.messages.length;
        await _backend.loadPipeMessages(widget.pipe.id);
        if (_backend.pipeChat.messages.length == before) return;
      }
    } on RequestFailure {
      // Tried again as either log changes.
    } finally {
      _filling = false;
    }
  }

  /// Pages the liaison's conversation back, then the pipe's messages with it.
  Future<void> _loadEarlier() async {
    await _backend.loadEarlier(widget.liaison.id);
    await _fill();
  }

  bool get _working =>
      widget.liaison.status == AgentStatus.AGENT_STATUS_WORKING;

  void _stop() {
    if (_working) fireAndLog(_backend.stopAgent(widget.liaison.id));
  }

  void _pipeFailed(String text, String why) {
    if (!mounted) return;
    final drafts = AppScope.scope(context).drafts;
    final key = pipeBoxKey(widget.pipe.id);
    final draft = drafts.of(key);
    drafts.set(key, draft.isEmpty ? text : '$text\n$draft');
    _targets.value = PipeTarget.pipe;
    _failed.value = why;
  }

  /// Moves what's written to the box of [target], so changing whom it goes
  /// to never loses it.
  void _choose(PipeTarget target) {
    if (_targets.value == target) return;
    final drafts = AppScope.scope(context).drafts;
    final from = _keyOf(_targets.value);
    final to = _keyOf(target);
    final text = drafts.of(from);
    if (text.isNotEmpty) {
      final there = drafts.of(to);
      drafts
        ..set(from, '')
        ..set(to, there.isEmpty ? text : '$there\n$text');
    }
    _failed.value = null;
    _targets.value = target;
  }

  String _keyOf(PipeTarget target) => target == PipeTarget.pipe
      ? pipeBoxKey(widget.pipe.id)
      : widget.liaison.id;

  @override
  Widget build(BuildContext context) {
    final backend = _backend;
    final transcript = backend.transcript;
    final liaison = widget.liaison;
    final pipe = widget.pipe;
    final open = pipe.state == PipeState.PIPE_STATE_OPEN;
    // Only an open pipe takes the person's messages.
    if (!open && _targets.value == PipeTarget.pipe) _targets.reset();
    final timeline = AgentTints(
      own: agentColor(liaison.color),
      byId: {
        for (final other in backend.workspace.workspace.agents)
          other.id: agentColor(other.color),
      },
      agents: {
        for (final other in backend.workspace.workspace.agents)
          other.id: identityOf(other),
      },
      open: (id) => AppScope.scope(context).history.follow(liaison.id, id),
      child: ListenableBuilder(
        listenable: Listenable.merge([
          transcript,
          backend.queued,
          backend.outbox,
          _extras,
        ]),
        builder: (context, _) {
          if (transcript.agentId != liaison.id) return const SizedBox.shrink();
          if (transcript.length == 0 &&
              _extras.value.isEmpty &&
              backend.queued.of(liaison.id).isEmpty &&
              backend.outbox.of(liaison.id).isEmpty &&
              !_working) {
            return _EmptyPipe(pipe: pipe, liaison: liaison);
          }
          return TranscriptView(
            key: ValueKey(transcript.loads),
            transcript: transcript,
            working: _working,
            follow: _follow,
            queued: backend.queued,
            outbox: backend.outbox,
            loadEarlier: _loadEarlier,
            agentId: liaison.id,
            reveal: backend.reveal,
            loadFrom: (seq) => backend.loadFrom(liaison.id, seq),
            extras: _extras,
            hidden: (entry) => hiddenInPipe(entry, _seqs),
          );
        },
      ),
    );
    return LayoutBuilder(
      builder: (context, constraints) {
        final bottom = ValueListenableBuilder<PipeTarget>(
          valueListenable: _targets,
          builder: (context, target, _) {
            final toPipe = target == PipeTarget.pipe;
            final MessageTarget boxTarget = toPipe
                ? PipeMessageTarget(pipe, backend, onFailed: _pipeFailed)
                : AgentMessageTarget(liaison, backend);
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                ArchivedLine(agent: liaison),
                ReconnectingLine(
                  otherwise: _working
                      ? WorkingLine(onStop: _stop, wait: usageWaitLine(liaison))
                      : const SizedBox.shrink(),
                ),
                ?pipeStateBar(context, pipe),
                if (open && !pipe.online)
                  _SeenBy(text: pipeOfflineText(pipe), top: true),
                if (open)
                  // On a phone, one chip at the box's leading edge.
                  isPhoneLayout(context)
                      ? PhonePipeTarget(
                          pipe: pipe,
                          liaison: liaison,
                          target: target,
                          onChanged: _choose,
                        )
                      : PipeTargetChooser(
                          pipe: pipe,
                          liaison: liaison,
                          target: target,
                          onChanged: _choose,
                        ),
                MessageBox(
                  target: boxTarget,
                  onSent: () {
                    _follow.value++;
                    _failed.value = null;
                    // Each message through the pipe is a choice made anew.
                    if (toPipe) _targets.reset();
                  },
                  maxFieldHeight: constraints.maxHeight.isFinite
                      ? constraints.maxHeight * 0.3
                      : null,
                ),
                ValueListenableBuilder<String?>(
                  valueListenable: _failed,
                  builder: (context, failed, _) => _SeenBy(
                    text:
                        failed ??
                        (toPipe
                            ? '${pipePeerClaktory(pipe)} sees this.'
                            : 'Only ${liaison.name} sees this.'),
                    failed: failed != null,
                  ),
                ),
              ],
            );
          },
        );
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(child: timeline),
            if (widget.phone && constraints.maxHeight.isFinite)
              ConstrainedBox(
                constraints: BoxConstraints(
                  maxHeight: constraints.maxHeight * 0.6,
                ),
                child: SingleChildScrollView(reverse: true, child: bottom),
              )
            else
              bottom,
          ],
        );
      },
    );
  }
}

/// The key of a pipe message's card among a conversation's extras, to go
/// to it ([EntryReveal.reachExtra]).
String pipeItemKey(int seq) => 'pipe:$seq';

/// Above the box of a pipe with a liaison: whom it writes to, "To Kai" or
/// "To Ana's Claktory", as two pills; the liaison's is chosen until the
/// person picks the pipe.
class PipeTargetChooser extends StatelessWidget {
  const PipeTargetChooser({
    super.key,
    required this.pipe,
    required this.liaison,
    required this.target,
    required this.onChanged,
  });

  final Pipe pipe;
  final Agent liaison;
  final PipeTarget target;
  final ValueChanged<PipeTarget> onChanged;

  @override
  Widget build(BuildContext context) {
    final phone = isPhoneLayout(context);
    return Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: contentMaxWidth),
        child: Padding(
          padding: EdgeInsets.fromLTRB(
            phone ? Space.s : Space.xl,
            Space.s,
            phone ? Space.s : Space.xl,
            0,
          ),
          child: Wrap(
            spacing: Space.xs,
            runSpacing: Space.xs,
            children: [
              _TargetPill(
                label: 'To ${liaison.name}',
                leading: AgentCharacter(agent: identityOf(liaison), size: 14),
                selected: target == PipeTarget.liaison,
                onTap: () => onChanged(PipeTarget.liaison),
              ),
              _TargetPill(
                label: 'To ${pipePeerClaktory(pipe)}',
                leading: const PortalGlyph(size: 14),
                selected: target == PipeTarget.pipe,
                color: pipeColor,
                onTap: () => onChanged(PipeTarget.pipe),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// One of [PipeTargetChooser]'s pills: 22px (taller with larger text), a
/// 14px glyph and the words in
/// caption; `selected` fill while chosen (the pipe's tint for the pipe),
/// `hover` on hover.
class _TargetPill extends StatelessWidget {
  const _TargetPill({
    required this.label,
    required this.leading,
    required this.selected,
    required this.onTap,
    this.color,
  });

  final String label;
  final Widget leading;
  final bool selected;
  final VoidCallback onTap;

  /// The pipe's color, for the pipe's pill.
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final color = this.color;
    return Semantics(
      button: true,
      selected: selected,
      label: label,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => AnimatedContainer(
          duration: Motion.fast,
          constraints: const BoxConstraints(minHeight: 22),
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          decoration: BoxDecoration(
            color: selected
                ? color?.withValues(alpha: 0.14) ?? palette.selected
                : hovered
                ? palette.hover
                : null,
            // A pill at any height: larger text makes it taller.
            borderRadius: BorderRadius.circular(999),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              leading,
              const SizedBox(width: Space.xs + 2),
              Flexible(
                child: Text(
                  label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TypeScale.of(context).caption.copyWith(
                    color: selected
                        ? color ?? palette.text
                        : hovered
                        ? palette.text
                        : palette.textMuted,
                    fontWeight: selected ? FontWeight.w600 : null,
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// By the box: who sees what's written in it, why the last message
/// through the pipe didn't go, or that the other side is offline.
class _SeenBy extends StatelessWidget {
  const _SeenBy({required this.text, this.failed = false, this.top = false});

  final String text;
  final bool failed;

  /// Above the box rather than under it.
  final bool top;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    return Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: contentMaxWidth),
        child: Padding(
          padding: EdgeInsets.fromLTRB(
            phone ? Space.s : Space.xl,
            top ? Space.s : 0,
            phone ? Space.s : Space.xl,
            top ? 0 : Space.s,
          ),
          child: Text(
            text,
            style: TypeScale.of(context).caption
                .copyWith(color: failed ? palette.danger : palette.textFaint),
          ),
        ),
      ),
    );
  }
}

/// A pipe with a liaison and nothing said either way yet.
class _EmptyPipe extends StatelessWidget {
  const _EmptyPipe({required this.pipe, required this.liaison});

  final Pipe pipe;
  final Agent liaison;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(Space.xl),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 360),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              AgentCharacter(agent: identityOf(liaison), size: 48),
              const SizedBox(height: Space.m),
              Text(
                liaison.name,
                textAlign: TextAlign.center,
                style: TypeScale.of(context).label
                    .copyWith(color: palette.text),
              ),
              const SizedBox(height: Space.s),
              Text(
                pipe.state == PipeState.PIPE_STATE_OPEN
                    ? 'Nothing here yet. ${liaison.name} reads and answers '
                          'what ${pipePeerClaktory(pipe)} sends. Write to '
                          '${liaison.name}, or straight to '
                          '${pipePeerClaktory(pipe)}.'
                    : pipeStatus(pipe),
                textAlign: TextAlign.center,
                style: TypeScale.of(context).body
                    .copyWith(color: palette.textMuted),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
