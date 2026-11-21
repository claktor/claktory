import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_name.dart';
import 'avatar.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'extensions.dart' show logTime;
import 'forms.dart';
import 'identity.dart';
import 'mobile/header.dart' show HeaderButton;
import 'pairing.dart' show PairingDigits;
import 'protocol/factory/v1/factory.pb.dart';
import 'resources.dart' show formatBytes;
import 'scope.dart';
import 'selection.dart';
import 'transcript.dart' show Bubble, userAvatarSize;

/// A pipe's color: its portal glyph, its header's bar and its line on the
/// Map. Deep cyan, apart from the agents' colors, white text 4.5:1 on it.
const pipeColor = Color(0xFF0E7C99);

/// The pipe the main pane shows, if any (Workspace.selected_pipe_id).
Pipe? selectedPipe(Workspace workspace) {
  final id = workspace.selectedPipeId;
  if (id.isEmpty) return null;
  for (final pipe in workspace.pipes) {
    if (pipe.id == id) return pipe;
  }
  return null;
}

/// The pipes from [projectId], in the order they were made.
List<Pipe> pipesIn(Workspace workspace, String projectId) => [
  for (final pipe in workspace.pipes)
    if (pipe.projectId == projectId) pipe,
]..sort((a, b) => a.createdMs.compareTo(b.createdMs));

/// Who is at the other end, in a few words: their person's name, else their
/// Claktory's; "the other side" before they've said.
String pipePeerName(Pipe pipe) {
  final peer = pipe.peer;
  if (peer.ownerName.isNotEmpty) return peer.ownerName;
  if (peer.claktoryName.isNotEmpty) return peer.claktoryName;
  return 'the other side';
}

/// The other side of [pipe] as its Claktory is called ("Ana's Claktory"),
/// else its person's name: the words for what an agent read or is limited
/// to.
String pipePeerClaktory(Pipe pipe) => pipe.peer.claktoryName.isNotEmpty
    ? pipe.peer.claktoryName
    : pipePeerName(pipe);

/// The pipes [agentId] is limited to (Pipe.quarantined_ids).
List<Pipe> pipesLimiting(Workspace workspace, String agentId) => [
  for (final pipe in workspace.pipes)
    if (pipe.quarantinedIds.contains(agentId)) pipe,
];

/// "Limited to the pipe with Ana's Claktory", or to several: "…the pipes
/// with Ana's Claktory and Lea's Claktory".
String pipeLimitText(List<Pipe> pipes) {
  final peers = [for (final pipe in pipes) pipePeerClaktory(pipe)];
  final names = peers.length < 2
      ? peers.join()
      : '${peers.sublist(0, peers.length - 1).join(', ')} and ${peers.last}';
  return peers.length < 2
      ? 'Limited to the pipe with $names'
      : 'Limited to the pipes with $names';
}

/// The agents that have read [pipe] (Agent.tainted_by_pipes), in the
/// sidebar's order.
List<Agent> pipeReaders(Workspace workspace, Pipe pipe) => [
  for (final agent in workspace.agents)
    if (agent.taintedByPipes.contains(pipe.id)) agent,
];

/// How many things in [pipe] wait for the person: messages held for them,
/// its approval, and the loop breaker's Continue.
int pipeWaiting(Pipe pipe) =>
    pipe.waitingSeqs.length +
    (pipe.state == PipeState.PIPE_STATE_APPROVAL ? 1 : 0) +
    (pipe.state == PipeState.PIPE_STATE_PAUSED &&
            pipe.paused == PipePause.PIPE_PAUSE_LOOP
        ? 1
        : 0);

/// Where [pipe] stands, in a few words, for its sidebar row and header.
String pipeStatus(Pipe pipe) {
  final who = pipePeerName(pipe);
  return switch (pipe.state) {
    PipeState.PIPE_STATE_INVITED => 'Waiting for your invite to be used',
    PipeState.PIPE_STATE_JOINING => 'Connecting…',
    PipeState.PIPE_STATE_APPROVAL => 'Waiting for your approval',
    PipeState.PIPE_STATE_WAITING_PEER => 'Waiting for $who to approve',
    PipeState.PIPE_STATE_OPEN =>
      pipe.online
          ? 'Online'
          : pipe.queued > 0
          ? 'Offline · ${pipe.queued} waiting'
          : 'Offline',
    PipeState.PIPE_STATE_PAUSED => switch (pipe.paused) {
      PipePause.PIPE_PAUSE_BY_PEER => 'Paused by $who',
      PipePause.PIPE_PAUSE_LOOP => 'Paused: agents kept talking on their own',
      _ => 'Paused',
    },
    PipeState.PIPE_STATE_CLOSED => 'Closed',
    _ => '',
  };
}

/// Opens [pipe] in the main pane.
void openPipe(BuildContext context, Pipe pipe) {
  AppScope.closePage(context);
  fireAndLog(AppScope.of(context).selectPipe(pipe.id));
}

/// A portal: an open ring with a smaller one inside, off center, as if
/// looking down a tunnel to somewhere else. A pipe's glyph wherever it
/// shows (the sidebar, its header, the Map).
class PortalGlyph extends StatelessWidget {
  const PortalGlyph({super.key, this.color = pipeColor, this.size = 16});

  final Color color;
  final double size;

  @override
  Widget build(BuildContext context) =>
      CustomPaint(size: Size.square(size), painter: _PortalPainter(color));
}

class _PortalPainter extends CustomPainter {
  const _PortalPainter(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final unit = size.shortestSide / 16;
    final stroke = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5 * unit;
    final center = size.center(Offset.zero);
    canvas
      ..drawOval(
        Rect.fromCenter(center: center, width: 14 * unit, height: 14 * unit),
        stroke,
      )
      ..drawOval(
        Rect.fromCenter(
          center: center + Offset(1.5 * unit, 0),
          width: 8 * unit,
          height: 9 * unit,
        ),
        stroke,
      )
      ..drawCircle(
        center + Offset(2.5 * unit, 0),
        1.6 * unit,
        Paint()..color = color,
      );
  }

  @override
  bool shouldRepaint(_PortalPainter old) => old.color != color;
}

/// A pipe in the main pane: its header, then what it waits for (approval,
/// the other side) or its chat, and the person's box to write in.
class PipeView extends StatelessWidget {
  const PipeView({super.key, required this.pipe, this.onMenu});

  final Pipe pipe;

  /// On a phone, opens the drawer: a Menu button leads the header.
  final VoidCallback? onMenu;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final store = AppScope.of(context).workspace;
    final project = store.project(pipe.projectId);
    final Widget body = switch (pipe.state) {
      PipeState.PIPE_STATE_APPROVAL => PipeApprovalCard(pipe: pipe),
      PipeState.PIPE_STATE_INVITED ||
      PipeState.PIPE_STATE_JOINING ||
      PipeState.PIPE_STATE_WAITING_PEER => _Waiting(pipe: pipe),
      _ => _PipeChatView(pipe: pipe),
    };
    return DefaultTextStyle(
      style: TextStyles.body.copyWith(color: palette.text),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _PipeHeader(pipe: pipe, project: project, onMenu: onMenu),
          _Readers(pipe: pipe),
          Expanded(child: body),
          _PipeFoot(pipe: pipe),
        ],
      ),
    );
  }
}

class _PipeHeader extends StatelessWidget {
  const _PipeHeader({required this.pipe, required this.project, this.onMenu});

  final Pipe pipe;
  final Project? project;
  final VoidCallback? onMenu;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final peer = pipe.peer;
    final ends = [
      if (project != null) project!.name,
      if (peer.projectName.isNotEmpty)
        '${peer.projectName} in ${peer.claktoryName.isEmpty ? pipePeerName(pipe) : peer.claktoryName}',
    ].join(' ⇄ ');
    return DecoratedBox(
      decoration: BoxDecoration(
        gradient: LinearGradient(
          colors: [
            pipeColor.withValues(alpha: 0.14),
            pipeColor.withValues(alpha: 0),
          ],
          stops: const [0, 0.7],
        ),
        border: Border(
          bottom: BorderSide(color: pipeColor.withValues(alpha: 0.7), width: 2),
        ),
      ),
      child: WindowDrag(
        child: Pressable(
          onTap: () => showPipeSettings(context, pipe),
          builder: (context, hovered) => Container(
            height: titleBarHeight,
            padding: EdgeInsets.only(
              left: onMenu == null ? Space.xl : Space.xs,
              right: Space.xl,
            ),
            color: hovered ? palette.hover : null,
            child: Row(
              children: [
                if (onMenu case final onMenu?) HeaderButton.menu(onTap: onMenu),
                const PortalGlyph(size: 22),
                const SizedBox(width: Space.s + 2),
                Expanded(
                  child: NameOverRole(
                    name: pipe.name,
                    role: ends,
                    style: TextStyles.label.copyWith(
                      color: palette.text,
                      fontWeight: FontWeight.w600,
                    ),
                    roleColor: palette.textMuted,
                  ),
                ),
                if (pipe.state != PipeState.PIPE_STATE_INVITED &&
                    !pipe.peer.verified) ...[
                  _Mark(text: 'Not verified', color: palette.textMuted),
                  const SizedBox(width: Space.s),
                ],
                if (pipe.canRunCommandsIds.isNotEmpty) ...[
                  _Mark(
                    text: canRunCommandsText(pipe.canRunCommandsIds.length),
                    color: palette.textMuted,
                  ),
                  const SizedBox(width: Space.s),
                ],
                if (pipe.state == PipeState.PIPE_STATE_OPEN) ...[
                  Dot(
                    color: pipe.online ? pipeColor : palette.textFaint,
                    size: 7,
                  ),
                  const SizedBox(width: Space.xs + 2),
                ],
                Flexible(
                  child: Text(
                    pipeStatus(pipe),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyles.body.copyWith(color: palette.textMuted),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Under a pipe's header, the agents that have read it
/// (Agent.tainted_by_pipes): each with "Has read this pipe" and Clear
/// conversation…, so the person can start one afresh on purpose. Nothing
/// when none has.
class _Readers extends StatelessWidget {
  const _Readers({required this.pipe});

  final Pipe pipe;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final store = AppScope.of(context).workspace;
    final readers = pipeReaders(store.workspace, pipe);
    if (readers.isEmpty) return const SizedBox.shrink();
    return Container(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.xl,
        vertical: Space.xs,
      ),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final agent in readers)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 2),
              child: Row(
                children: [
                  AgentCharacter(agent: agent, size: 20),
                  const SizedBox(width: Space.s),
                  Expanded(
                    child: Text.rich(
                      TextSpan(
                        children: [
                          TextSpan(
                            text: agent.name,
                            style: TextStyle(
                              color: palette.text,
                              fontWeight: FontWeight.w500,
                            ),
                          ),
                          TextSpan(
                            text: '  Has read this pipe',
                            style: TextStyle(color: palette.textMuted),
                          ),
                        ],
                      ),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyles.caption,
                    ),
                  ),
                  const SizedBox(width: Space.s),
                  Button(
                    label: 'Clear conversation…',
                    onPressed: () => showModal(
                      context,
                      (context, close) =>
                          _ClearConversation(agent: agent, close: close),
                    ),
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// Asks before clearing [agent]'s conversation (its /clear), which starts
/// it afresh with nothing it read of a pipe.
class _ClearConversation extends StatefulWidget {
  const _ClearConversation({required this.agent, required this.close});

  final Agent agent;
  final VoidCallback close;

  @override
  State<_ClearConversation> createState() => _ClearConversationState();
}

class _ClearConversationState extends State<_ClearConversation> {
  var _busy = false;
  String? _error;

  Future<void> _clear() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).runCommand(widget.agent.id, 'clear', '');
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final name = widget.agent.name;
    return FormFrame(
      title: "Clear $name's conversation?",
      body:
          "$name stops what it's doing and starts a new, empty "
          'conversation, without anything it read from pipes. Its checklist '
          'and memory stay.',
      error: _error,
      fields: const [],
      submitLabel: 'Clear conversation',
      onSubmit: _busy ? null : _clear,
      onCancel: widget.close,
    );
  }
}

/// Beside an agent's name, while it's limited to pipes (Agent.quarantined):
/// a portal and "Limited to the pipe with Ana's Claktory" in a hairline
/// capsule, like "Not verified". Nothing otherwise.
class PipeLimitMark extends StatelessWidget {
  const PipeLimitMark({super.key, required this.agent});

  final Agent agent;

  @override
  Widget build(BuildContext context) {
    if (!agent.quarantined) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final pipes = pipesLimiting(
      AppScope.of(context).workspace.workspace,
      agent.id,
    );
    final text = pipes.isEmpty ? 'Limited to a pipe' : pipeLimitText(pipes);
    return Semantics(
      label: text,
      excludeSemantics: true,
      child: Container(
        padding: const EdgeInsets.symmetric(
          horizontal: Space.s - 2,
          vertical: 1,
        ),
        decoration: BoxDecoration(
          border: Border.all(color: palette.hairline),
          borderRadius: BorderRadius.circular(Radii.control),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            const PortalGlyph(size: 12),
            const SizedBox(width: Space.xs),
            Flexible(
              child: Text(
                text,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyles.caption.copyWith(color: palette.textMuted),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// "1 agent can run commands", "2 agents can run commands": the pipe's
/// members that aren't limited to it.
String canRunCommandsText(int count) =>
    '$count ${count == 1 ? 'agent' : 'agents'} can run commands';

/// What a member that can run commands could do if the other side misled
/// it, under its switch.
String canRunCommandsWarning(String agent, Pipe pipe) =>
    '$agent can run commands on this computer. If a message from '
    '${pipePeerClaktory(pipe)} misleads it, it could reach your files, '
    'secrets and network before you see anything. Turn this off unless it '
    'needs to run commands.';

/// A quiet mark in a hairline capsule, such as "Not verified".
class _Mark extends StatelessWidget {
  const _Mark({required this.text, required this.color});

  final String text;
  final Color color;

  @override
  Widget build(BuildContext context) => Container(
    padding: const EdgeInsets.symmetric(horizontal: Space.s - 2, vertical: 1),
    decoration: BoxDecoration(
      border: Border.all(color: Palette.of(context).hairline),
      borderRadius: BorderRadius.circular(Radii.control),
    ),
    child: Text(text, style: TextStyles.caption.copyWith(color: color)),
  );
}

/// The middle of a pipe that isn't open yet: what it waits for.
class _Waiting extends StatelessWidget {
  const _Waiting({required this.pipe});

  final Pipe pipe;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final who = pipePeerName(pipe);
    final (String title, String body) = switch (pipe.state) {
      PipeState.PIPE_STATE_INVITED => (
        'Waiting for your invite to be used',
        'Send the code to the person you want to connect with. They paste '
            'it in their Claktory, and you both approve. It works once'
            '${pipe.inviteExpiresMs > 0 ? ', until ${logTime(DateTime.fromMillisecondsSinceEpoch(pipe.inviteExpiresMs.toInt()))}' : ''}.',
      ),
      PipeState.PIPE_STATE_JOINING => (
        'Connecting to $who…',
        'Reaching their Claktory with the code.',
      ),
      _ => (
        'Waiting for $who to approve',
        'You approved. Nothing goes through until they do too.',
      ),
    };
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(Space.xl),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 440),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  if (pipe.state == PipeState.PIPE_STATE_JOINING) ...[
                    const LiveDot(color: pipeColor),
                    const SizedBox(width: Space.s),
                  ],
                  Expanded(
                    child: Text(
                      title,
                      style: TextStyles.emphasis.copyWith(color: palette.text),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: Space.s),
              Text(
                body,
                style: TextStyles.body.copyWith(color: palette.textMuted),
              ),
              if (pipe.state == PipeState.PIPE_STATE_WAITING_PEER &&
                  pipe.digits.isNotEmpty) ...[
                const SizedBox(height: Space.l),
                Text(
                  'They should see the same digits:',
                  style: TextStyles.caption.copyWith(color: palette.textFaint),
                ),
                const SizedBox(height: Space.s),
                PairingDigits(pipe.digits),
              ],
              const SizedBox(height: Space.xl),
              Align(
                alignment: Alignment.centerRight,
                child: _StateButton(
                  pipe: pipe,
                  action: PipeAction.PIPE_ACTION_CLOSE,
                  label: pipe.state == PipeState.PIPE_STATE_INVITED
                      ? 'Cancel invite'
                      : 'Cancel',
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A button that pauses, resumes, closes or deletes [pipe], with why it
/// was refused beside it.
class _StateButton extends StatefulWidget {
  const _StateButton({
    required this.pipe,
    required this.action,
    required this.label,
    this.primary = false,
  });

  final Pipe pipe;
  final PipeAction action;
  final String label;
  final bool primary;

  @override
  State<_StateButton> createState() => _StateButtonState();
}

class _StateButtonState extends State<_StateButton> {
  var _busy = false;
  String? _error;

  Future<void> _run() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).setPipeState(widget.pipe.id, widget.action);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (_error case final error?) ...[
          Flexible(
            child: Text(
              error,
              style: TextStyles.caption.copyWith(color: palette.danger),
            ),
          ),
          const SizedBox(width: Space.s),
        ],
        Button(
          label: widget.label,
          primary: widget.primary,
          onPressed: _busy ? null : _run,
        ),
      ],
    );
  }
}

/// A pipe waiting for this person's approval: who wants to connect, from
/// where and why, the digits both screens show, and which agents may use
/// it. Nothing goes through until both people approve.
class PipeApprovalCard extends StatefulWidget {
  const PipeApprovalCard({super.key, required this.pipe, this.inline = false});

  final Pipe pipe;

  /// Just the card, where something else places it (Needs you); else
  /// centred in the pipe's view.
  final bool inline;

  @override
  State<PipeApprovalCard> createState() => _PipeApprovalCardState();
}

/// The lead choice that makes a new liaison agent.
const _liaison = '\u0000liaison';

class _PipeApprovalCardState extends State<PipeApprovalCard> {
  final _members = <String>{};

  /// Each member's "Can run commands on this computer", once the person
  /// turned it; else [_canRunByDefault].
  final _canRun = <String, bool>{};
  var _lead = _liaison;
  final _liaisonName = TextEditingController(text: 'Liaison');
  var _busy = false;
  String? _error;

  @override
  void dispose() {
    _liaisonName.dispose();
    super.dispose();
  }

  Future<void> _decide(bool approve) async {
    final liaison = _lead == _liaison;
    if (approve && liaison && _liaisonName.text.trim().isEmpty) {
      setState(() => _error = 'Name the liaison agent.');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).approvePipe(
        ApprovePipe(
          pipeId: widget.pipe.id,
          approve: approve,
          memberIds: approve ? _members : const <String>[],
          leadId: approve && !liaison ? _lead : '',
          liaisonName: approve && liaison ? _liaisonName.text.trim() : '',
          quarantined: approve
              ? PipeMembers(
                  agentIds: [
                    for (final agent in AppScope.of(
                      context,
                    ).workspace.agentsIn(widget.pipe.projectId))
                      if (_members.contains(agent.id) && !_canRunOf(agent))
                        agent.id,
                  ],
                )
              : null,
        ),
      );
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// A liaison is limited to the pipe unless the person says otherwise;
  /// every other member can run commands, with the warning.
  static bool _canRunByDefault(Agent agent) =>
      agent.role.trim().toLowerCase() != 'liaison';

  bool _canRunOf(Agent agent) => _canRun[agent.id] ?? _canRunByDefault(agent);

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final pipe = widget.pipe;
    final peer = pipe.peer;
    final store = AppScope.of(context).workspace;
    final project = store.project(pipe.projectId);
    final agents = store.agentsIn(pipe.projectId);
    if (_lead != _liaison && !_members.contains(_lead)) _lead = _liaison;
    final who = [
      if (peer.ownerName.isNotEmpty) peer.ownerName,
      if (peer.ownerEmail.isNotEmpty) peer.ownerEmail,
      if (peer.claktoryName.isNotEmpty) peer.claktoryName,
    ];
    final caption = TextStyles.caption.copyWith(color: palette.textFaint);
    final card = Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        color: pipeColor.withValues(alpha: 0.06),
        border: Border.all(color: pipeColor.withValues(alpha: 0.55)),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const PortalGlyph(size: 20),
              const SizedBox(width: Space.xs + 2),
              Expanded(
                child: Text(
                  '${pipePeerName(pipe)} wants to connect',
                  style: TextStyles.caption.copyWith(
                    color: pipeColor,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              if (!peer.verified)
                _Mark(text: 'Not verified', color: palette.textMuted),
            ],
          ),
          const SizedBox(height: Space.m),
          Text(
            '${peer.projectName.isEmpty ? 'Their project' : peer.projectName} '
            'with your ${project?.name ?? 'project'}',
            style: TextStyles.emphasis.copyWith(color: palette.text),
          ),
          if (who.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            Text(
              who.join(' · '),
              style: TextStyles.body.copyWith(color: palette.textMuted),
            ),
          ],
          if (peer.purpose.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            Text(
              '“${peer.purpose}”',
              style: TextStyles.body.copyWith(color: palette.text),
            ),
          ],
          if (!peer.verified) ...[
            const SizedBox(height: Space.s),
            Text(
              "Claktory can't confirm who they are. Check with them "
              'another way that they sent the code.',
              style: caption,
            ),
          ],
          if (pipe.digits.isNotEmpty) ...[
            const SizedBox(height: Space.l),
            Text('Both screens should show:', style: caption),
            const SizedBox(height: Space.s),
            PairingDigits(pipe.digits),
            const SizedBox(height: Space.xs),
            Text(
              'Approve only if ${pipePeerName(pipe)} sees the same '
              'digits.',
              textAlign: TextAlign.center,
              style: caption,
            ),
          ],
          const SizedBox(height: Space.l),
          Text(
            'Who may use it',
            style: TextStyles.label.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.xs),
          for (final agent in agents)
            Padding(
              padding: const EdgeInsets.only(top: Space.xs),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _MemberLine(
                    agent: agent,
                    value: _members.contains(agent.id),
                    onChanged: (on) => setState(() {
                      if (on) {
                        _members.add(agent.id);
                      } else {
                        _members.remove(agent.id);
                      }
                    }),
                  ),
                  if (_members.contains(agent.id))
                    _CommandsLine(
                      agent: agent,
                      pipe: pipe,
                      value: _canRunOf(agent),
                      onChanged: (on) => setState(() => _canRun[agent.id] = on),
                    ),
                ],
              ),
            ),
          const SizedBox(height: Space.m),
          Field(
            label: 'Lead',
            note: _lead == _liaison
                ? liaisonNote
                : 'Told of each message that comes in. The others '
                      'read the pipe when they choose.',
            child: Select<String>(
              value: _lead,
              options: [
                const SelectOption(_liaison, 'A new liaison agent'),
                for (final agent in agents)
                  if (_members.contains(agent.id))
                    SelectOption(agent.id, agent.name),
              ],
              onChanged: (lead) => setState(() => _lead = lead),
            ),
          ),
          if (_lead == _liaison)
            Field(
              label: "Liaison's name",
              child: TextField(controller: _liaisonName),
            ),
          Row(
            children: [
              Expanded(
                child: Text(
                  _error ??
                      'You can change this later in the pipe\'s '
                          'settings.',
                  style: TextStyles.caption.copyWith(
                    color: _error == null ? palette.textFaint : palette.danger,
                  ),
                ),
              ),
              const SizedBox(width: Space.s),
              Button(
                label: 'Decline',
                onPressed: _busy ? null : () => _decide(false),
              ),
              const SizedBox(width: Space.s),
              Button(
                label: 'Approve',
                primary: true,
                onPressed: _busy ? null : () => _decide(true),
              ),
            ],
          ),
        ],
      ),
    );
    if (widget.inline) return card;
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(Space.xl),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 480),
          child: card,
        ),
      ),
    );
  }
}

/// A pipe's messages, newest at the bottom, built only as they show; the
/// earlier ones load as the person scrolls up.
class _PipeChatView extends StatefulWidget {
  const _PipeChatView({required this.pipe});

  final Pipe pipe;

  @override
  State<_PipeChatView> createState() => _PipeChatViewState();
}

class _PipeChatViewState extends State<_PipeChatView> {
  final _scroll = ScrollController();
  var _loading = false;

  @override
  void initState() {
    super.initState();
    _scroll.addListener(_maybeLoadEarlier);
  }

  @override
  void dispose() {
    _scroll.dispose();
    super.dispose();
  }

  void _maybeLoadEarlier() {
    final backend = AppScope.of(context);
    final chat = backend.pipeChat;
    if (_loading ||
        !chat.hasEarlier ||
        chat.pipeId != widget.pipe.id ||
        !_scroll.hasClients) {
      return;
    }
    final position = _scroll.position;
    // Reversed: the top is the far end.
    if (position.maxScrollExtent - position.pixels > 800) return;
    _loading = true;
    unawaited(
      backend
          .loadPipeMessages(widget.pipe.id)
          .catchError((Object _) {})
          .whenComplete(() => _loading = false),
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final chat = AppScope.of(context).pipeChat;
    return ListenableBuilder(
      listenable: chat,
      builder: (context, _) {
        final messages = chat.pipeId == widget.pipe.id
            ? chat.messages
            : const <PipeMessage>[];
        if (messages.isEmpty) {
          return Center(
            child: Padding(
              padding: const EdgeInsets.all(Space.xl),
              child: Text(
                widget.pipe.state == PipeState.PIPE_STATE_OPEN
                    ? 'Nothing here yet. Your lead hears each message that '
                          'comes in, and you can write here too.'
                    : '',
                textAlign: TextAlign.center,
                style: TextStyles.body.copyWith(color: palette.textMuted),
              ),
            ),
          );
        }
        if (chat.hasEarlier) {
          WidgetsBinding.instance.addPostFrameCallback(
            (_) => mounted ? _maybeLoadEarlier() : null,
          );
        }
        return SelectableArea(
          child: ListView.builder(
            controller: _scroll,
            reverse: true,
            padding: const EdgeInsets.symmetric(vertical: Space.l),
            itemCount: messages.length,
            itemBuilder: (context, index) {
              final message = messages[messages.length - 1 - index];
              return Align(
                alignment: Alignment.topCenter,
                child: ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: contentMaxWidth),
                  child: Padding(
                    padding: const EdgeInsets.fromLTRB(
                      Space.xl,
                      Space.m,
                      Space.xl,
                      0,
                    ),
                    child: PipeMessageView(
                      key: ValueKey(message.seq),
                      pipe: widget.pipe,
                      message: message,
                    ),
                  ),
                ),
              );
            },
          ),
        );
      },
    );
  }
}

/// One message in a pipe: theirs on the left, this side's on the right,
/// with who wrote it, what's become of it, and the person's decision when
/// one waits. Their text is shown as it is, never as Markdown or links:
/// it comes from outside.
class PipeMessageView extends StatefulWidget {
  const PipeMessageView({super.key, required this.pipe, required this.message});

  final Pipe pipe;
  final PipeMessage message;

  @override
  State<PipeMessageView> createState() => _PipeMessageViewState();
}

class _PipeMessageViewState extends State<PipeMessageView> {
  var _busy = false;
  String? _error;

  Future<void> _decide(bool allow) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context)
          .decidePipeMessage(widget.pipe.id, widget.message.seq.toInt(), allow);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final message = widget.message;
    final author = message.author;
    final time = logTime(
      DateTime.fromMillisecondsSinceEpoch(message.timestampMs.toInt()),
    );
    if (author.kind == PipeAuthorKind.PIPE_AUTHOR_KIND_CLAKTORY) {
      return Center(
        child: Text(
          '${message.text} · $time',
          textAlign: TextAlign.center,
          style: TextStyles.caption.copyWith(color: palette.textFaint),
        ),
      );
    }
    final mine =
        author.kind == PipeAuthorKind.PIPE_AUTHOR_KIND_ME ||
        author.kind == PipeAuthorKind.PIPE_AUTHOR_KIND_MY_AGENT;
    final store = AppScope.of(context).workspace;
    final agent = author.kind == PipeAuthorKind.PIPE_AUTHOR_KIND_MY_AGENT
        ? store.agent(author.agentId)
        : null;
    final Color color = switch (author.kind) {
      PipeAuthorKind.PIPE_AUTHOR_KIND_ME => palette.signal,
      PipeAuthorKind.PIPE_AUTHOR_KIND_MY_AGENT => agentColor(
        agent?.color ?? 'slate',
      ),
      _ => palette.selected,
    };
    final white = mine;
    final dropped =
        message.state == PipeMessageState.PIPE_MESSAGE_STATE_DISCARDED ||
        message.state == PipeMessageState.PIPE_MESSAGE_STATE_REFUSED;
    final caption = TextStyles.caption;
    final Widget? from = switch (author.kind) {
      PipeAuthorKind.PIPE_AUTHOR_KIND_ME => null,
      PipeAuthorKind.PIPE_AUTHOR_KIND_MY_AGENT => Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (agent != null) ...[
            AgentCharacter(agent: agent, size: 22),
            const SizedBox(width: Space.xs),
          ],
          Text(
            agent?.name ?? author.name,
            style: caption.copyWith(color: color, fontWeight: FontWeight.w600),
          ),
        ],
      ),
      PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_PERSON => Text(
        'Says it’s from ${author.name.isEmpty ? 'their person' : author.name}',
        style: caption.copyWith(
          color: palette.textMuted,
          fontWeight: FontWeight.w600,
        ),
      ),
      _ => NameThenRole(
        name: author.name.isEmpty ? 'Their agent' : author.name,
        role: author.role,
        style: caption.copyWith(
          color: palette.textMuted,
          fontWeight: FontWeight.w600,
        ),
        roleColor: palette.textFaint,
      ),
    };
    final bubble = Opacity(
      opacity: dropped ? 0.5 : 1,
      child: Bubble(
        color: color,
        outgoing: mine,
        child: Text(
          message.text,
          style: TextStyles.body.copyWith(
            color: white ? const Color(0xFFFFFFFF) : palette.text,
            decoration: dropped ? TextDecoration.lineThrough : null,
            decorationColor: white ? const Color(0xFFFFFFFF) : palette.text,
          ),
        ),
      ),
    );
    final side = mine ? CrossAxisAlignment.end : CrossAxisAlignment.start;
    final inset = EdgeInsets.only(
      left: mine ? 0 : Space.m,
      right: mine
          ? Space.m +
                (author.kind == PipeAuthorKind.PIPE_AUTHOR_KIND_ME
                    ? userAvatarSize + Space.xs
                    : 0)
          : 0,
    );
    return Column(
      crossAxisAlignment: side,
      children: [
        if (from != null)
          Padding(padding: inset.copyWith(bottom: 2), child: from),
        if (author.kind == PipeAuthorKind.PIPE_AUTHOR_KIND_ME)
          Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              Expanded(child: bubble),
              const SizedBox(width: Space.xs),
              const PersonAvatar(size: userAvatarSize),
            ],
          )
        else
          bubble,
        if (message.files.isNotEmpty)
          Padding(
            padding: inset.copyWith(top: Space.xs),
            child: Wrap(
              spacing: Space.xs,
              runSpacing: Space.xs,
              alignment: mine ? WrapAlignment.end : WrapAlignment.start,
              children: [
                for (final file in message.files)
                  _Mark(
                    text: '${file.name} · ${formatBytes(file.size)}',
                    color: palette.textMuted,
                  ),
              ],
            ),
          ),
        Padding(padding: inset.copyWith(top: 2), child: _state(palette, time)),
      ],
    );
  }

  /// What became of it, and the person's decision when one waits.
  Widget _state(Palette palette, String time) {
    final message = widget.message;
    final faint = TextStyles.caption.copyWith(color: palette.textFaint);
    final note = message.note;
    Widget line(String text, {Color? color}) => Text(
      text,
      style: color == null ? faint : TextStyles.caption.copyWith(color: color),
    );
    final decide = switch (message.state) {
      PipeMessageState.PIPE_MESSAGE_STATE_HELD => ('Discard', 'Send'),
      PipeMessageState.PIPE_MESSAGE_STATE_HELD_INCOMING => (
        'Discard',
        'Let in',
      ),
      _ => null,
    };
    if (decide != null) {
      final held = message.state == PipeMessageState.PIPE_MESSAGE_STATE_HELD;
      return Column(
        crossAxisAlignment: held
            ? CrossAxisAlignment.end
            : CrossAxisAlignment.start,
        children: [
          line(
            held
                ? note.isEmpty
                      ? 'Waiting for you to send it · $time'
                      : 'Waiting for you to send it · $note · $time'
                : note.isEmpty
                ? 'Held: over the pipe’s limit · $time'
                : 'Held: $note · $time',
            color: palette.textMuted,
          ),
          if (_error case final error?) line(error, color: palette.danger),
          const SizedBox(height: Space.xs),
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Button(
                label: decide.$1,
                onPressed: _busy ? null : () => _decide(false),
              ),
              const SizedBox(width: Space.s),
              Button(
                label: decide.$2,
                primary: true,
                onPressed: _busy ? null : () => _decide(true),
              ),
            ],
          ),
        ],
      );
    }
    return switch (message.state) {
      PipeMessageState.PIPE_MESSAGE_STATE_QUEUED => line(
        'Not delivered yet · $time',
      ),
      PipeMessageState.PIPE_MESSAGE_STATE_DELIVERED => line(
        'Delivered · $time',
      ),
      PipeMessageState.PIPE_MESSAGE_STATE_REFUSED => line(
        note.isEmpty ? 'Not sent · $time' : 'Not sent: $note · $time',
        color: palette.danger,
      ),
      PipeMessageState.PIPE_MESSAGE_STATE_DISCARDED => line(
        'Discarded · $time',
      ),
      _ => line(time),
    };
  }
}

/// Under a pipe: the person's box to write in while it's open, or why
/// nothing goes through and what they can do.
class _PipeFoot extends StatelessWidget {
  const _PipeFoot({required this.pipe});

  final Pipe pipe;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final who = pipePeerName(pipe);
    Widget bar(String text, List<Widget> actions) => Container(
      padding: const EdgeInsets.fromLTRB(Space.xl, Space.m, Space.xl, Space.m),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: palette.hairline)),
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(
              text,
              style: TextStyles.body.copyWith(color: palette.textMuted),
            ),
          ),
          for (final action in actions) ...[
            const SizedBox(width: Space.s),
            action,
          ],
        ],
      ),
    );
    switch (pipe.state) {
      case PipeState.PIPE_STATE_OPEN:
        return _PipeComposer(pipe: pipe);
      case PipeState.PIPE_STATE_PAUSED:
        return switch (pipe.paused) {
          PipePause.PIPE_PAUSE_LOOP => bar(pipeLoopText(pipe), [
            _StateButton(
              pipe: pipe,
              action: PipeAction.PIPE_ACTION_RESUME,
              label: 'Continue',
              primary: true,
            ),
          ]),
          PipePause.PIPE_PAUSE_BY_PEER => bar(
            '$who paused this pipe. Only they can resume it.',
            const [],
          ),
          _ => bar('You paused this pipe. Nothing goes through either way.', [
            _StateButton(
              pipe: pipe,
              action: PipeAction.PIPE_ACTION_RESUME,
              label: 'Resume',
              primary: true,
            ),
          ]),
        };
      case PipeState.PIPE_STATE_CLOSED:
        return bar('This pipe is closed. Its messages stay here.', [
          Button(
            label: 'Delete…',
            onPressed: () => showModal(
              context,
              (context, close) => _DeletePipe(pipe: pipe, close: close),
            ),
          ),
        ]);
      default:
        return const SizedBox.shrink();
    }
  }
}

/// A pipe the loop breaker paused, for the person to look at and continue:
/// what happened, and Continue. The pipe's foot says it too.
class PipeLoopCard extends StatelessWidget {
  const PipeLoopCard({super.key, required this.pipe});

  final Pipe pipe;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          pipeLoopText(pipe),
          style: TextStyles.body.copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.s),
        Align(
          alignment: Alignment.centerRight,
          child: _StateButton(
            pipe: pipe,
            action: PipeAction.PIPE_ACTION_RESUME,
            label: 'Continue',
            primary: true,
          ),
        ),
      ],
    );
  }
}

/// Why the loop breaker paused [pipe].
String pipeLoopText(Pipe pipe) =>
    'Your agents and ${pipePeerName(pipe)}’s kept talking with no person '
    'writing, so Claktory paused the pipe. Read what they said before you '
    'continue.';

class _DeletePipe extends StatefulWidget {
  const _DeletePipe({required this.pipe, required this.close});

  final Pipe pipe;
  final VoidCallback close;

  @override
  State<_DeletePipe> createState() => _DeletePipeState();
}

class _DeletePipeState extends State<_DeletePipe> {
  var _busy = false;
  String? _error;

  Future<void> _delete() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context)
          .setPipeState(widget.pipe.id, PipeAction.PIPE_ACTION_DELETE);
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => FormFrame(
    title: 'Delete ${widget.pipe.name}?',
    body:
        'Its messages are deleted from this Claktory. The other side keeps '
        'theirs.',
    error: _error,
    fields: const [],
    submitLabel: 'Delete pipe',
    onSubmit: _busy ? null : _delete,
    onCancel: widget.close,
  );
}

/// The person's box in an open pipe: what they write goes out as theirs.
class _PipeComposer extends StatefulWidget {
  const _PipeComposer({required this.pipe});

  final Pipe pipe;

  @override
  State<_PipeComposer> createState() => _PipeComposerState();
}

class _PipeComposerState extends State<_PipeComposer> {
  final _text = TextEditingController();
  var _sending = false;
  String? _error;

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _send() async {
    final text = _text.text.trim();
    if (text.isEmpty || _sending) return;
    setState(() {
      _sending = true;
      _error = null;
    });
    try {
      await AppScope.of(context).sendPipeMessage(widget.pipe.id, text);
      _text.clear();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _sending = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final pipe = widget.pipe;
    final who = pipePeerName(pipe);
    return Container(
      padding: const EdgeInsets.fromLTRB(Space.xl, Space.s, Space.xl, Space.m),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: palette.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (!pipe.online || _error != null)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.xs),
              child: Text(
                _error ??
                    (pipe.queued > 0
                        ? '$who’s Claktory is offline · ${pipe.queued} '
                              'waiting to deliver'
                        : '$who’s Claktory is offline. Messages wait until '
                              "it's back."),
                style: TextStyles.caption.copyWith(
                  color: _error == null ? palette.textFaint : palette.danger,
                ),
              ),
            ),
          Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              Expanded(
                child: CallbackShortcuts(
                  bindings: {
                    const SingleActivator(LogicalKeyboardKey.enter): _send,
                  },
                  child: TextField(
                    controller: _text,
                    placeholder: 'Write to $who',
                    minLines: 1,
                    maxLines: 8,
                  ),
                ),
              ),
              const SizedBox(width: Space.s),
              ListenableBuilder(
                listenable: _text,
                builder: (context, _) => Button(
                  label: 'Send',
                  primary: true,
                  onPressed: _sending || _text.text.trim().isEmpty
                      ? null
                      : _send,
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

/// Opens [pipe]'s settings.
Future<void> showPipeSettings(BuildContext context, Pipe pipe) => showModal(
  context,
  (context, close) => PipeSettingsForm(pipeId: pipe.id, close: close),
);

/// A pipe's settings: its name, what it's for (the other side sees it),
/// this side's prompt (only this side's agents see it), who may use it and
/// who leads, Review outgoing and its limits; and pausing, closing or
/// deleting it.
class PipeSettingsForm extends StatefulWidget {
  const PipeSettingsForm({
    super.key,
    required this.pipeId,
    required this.close,
  });

  final String pipeId;
  final VoidCallback close;

  @override
  State<PipeSettingsForm> createState() => _PipeSettingsFormState();
}

class _PipeSettingsFormState extends State<PipeSettingsForm> {
  late final Pipe _start = _pipe!;
  late final _name = TextEditingController(text: _start.name);
  late final _purpose = TextEditingController(text: _start.purpose);
  late final _prompt = TextEditingController(text: _start.prompt);
  late final _members = {..._start.memberIds};

  /// The members limited to the pipe (Pipe.quarantined_ids), as the form
  /// has them.
  late final _limited = {..._start.quarantinedIds};
  late var _lead = _start.leadId;
  late var _review = _start.reviewOutgoing;
  late final _perHour = TextEditingController(
    text: '${_start.limits.messagesPerHour}',
  );
  late final _burst = TextEditingController(text: '${_start.limits.burst}');
  late final _fileMb = TextEditingController(
    text: '${_start.limits.fileMbPerDay}',
  );
  var _busy = false;
  String? _error;

  /// What the form is showing instead of the settings.
  var _confirmingClose = false;

  /// The member the person turned on "Can run commands", to confirm.
  Agent? _letting;
  var _addingLiaison = false;
  final _liaisonName = TextEditingController(text: 'Liaison');

  Pipe? get _pipe {
    for (final pipe in AppScope.of(context).workspace.workspace.pipes) {
      if (pipe.id == widget.pipeId) return pipe;
    }
    return null;
  }

  @override
  void dispose() {
    for (final controller in [
      _name,
      _purpose,
      _prompt,
      _perHour,
      _burst,
      _fileMb,
      _liaisonName,
    ]) {
      controller.dispose();
    }
    super.dispose();
  }

  Future<void> _run(Future<void> Function(Backend backend) request) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await request(AppScope.of(context));
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _save() {
    final pipe = _pipe ?? _start;
    final update = UpdatePipe(pipeId: pipe.id);
    final name = _name.text.trim();
    if (name.isEmpty) {
      setState(() => _error = 'Name the pipe.');
      return;
    }
    if (name != pipe.name) update.name = name;
    if (_purpose.text.trim() != pipe.purpose) {
      update.purpose = _purpose.text.trim();
    }
    if (_prompt.text != pipe.prompt) update.prompt = _prompt.text;
    if (!_sameSet(_members, pipe.memberIds)) {
      update.members = PipeMembers(agentIds: _members);
    }
    // All of them: a member left out is let run commands again.
    final limited = _limited.intersection(_members);
    if (!_sameSet(limited, pipe.quarantinedIds)) {
      update.quarantined = PipeMembers(agentIds: limited);
    }
    if (_lead != pipe.leadId) update.leadId = _lead;
    if (_review != pipe.reviewOutgoing) update.reviewOutgoing = _review;
    final limits = PipeLimits(
      messagesPerHour: int.tryParse(_perHour.text.trim()) ?? 0,
      burst: int.tryParse(_burst.text.trim()) ?? 0,
      fileMbPerDay: int.tryParse(_fileMb.text.trim()) ?? 0,
    );
    if (limits.messagesPerHour == 0 || limits.burst == 0) {
      setState(() => _error = 'Limits must be at least 1.');
      return;
    }
    if (limits.messagesPerHour != pipe.limits.messagesPerHour ||
        limits.burst != pipe.limits.burst ||
        limits.fileMbPerDay != pipe.limits.fileMbPerDay) {
      update.limits = limits;
    }
    _run((backend) => backend.updatePipe(update));
  }

  static bool _sameSet(Set<String> a, List<String> b) =>
      a.length == b.toSet().length && a.containsAll(b);

  @override
  Widget build(BuildContext context) {
    final pipe = _pipe;
    if (pipe == null) {
      return FormFrame(
        title: 'Pipe settings',
        body: 'This pipe was deleted.',
        error: null,
        fields: const [],
        submitLabel: 'Close',
        onSubmit: widget.close,
        onCancel: widget.close,
      );
    }
    final who = pipePeerName(pipe);
    if (_confirmingClose) {
      return FormFrame(
        title: 'Close ${pipe.name}?',
        body:
            'Nothing goes through it again, either way, and $who is told. '
            'Its messages stay here until you delete them.',
        error: _error,
        fields: const [],
        submitLabel: 'Close pipe',
        onSubmit: _busy
            ? null
            : () => _run(
                (backend) =>
                    backend.setPipeState(pipe.id, PipeAction.PIPE_ACTION_CLOSE),
              ),
        onCancel: () => setState(() => _confirmingClose = false),
      );
    }
    if (_letting case final agent?) {
      final read = agent.taintedByPipes.contains(pipe.id);
      return FormFrame(
        title: 'Let ${agent.name} run commands on this computer?',
        body: [
          'It will be able to reach your files, secrets and network.',
          if (read)
            'It has read messages from ${pipePeerClaktory(pipe)}, so clear '
                'its conversation first to start it afresh.',
        ].join(' '),
        error: null,
        fields: const [],
        submitLabel: 'Let it run commands',
        onSubmit: () => setState(() {
          _limited.remove(agent.id);
          _letting = null;
        }),
        onCancel: () => setState(() => _letting = null),
      );
    }
    if (_addingLiaison) {
      return FormFrame(
        title: 'Add a liaison agent',
        body:
            'A new agent to lead this pipe and do nothing else. It can only '
            "read and answer the pipe: it can't run commands, reach your "
            'files or secrets, use the web or message your other agents.',
        error: _error,
        fields: [
          Field(
            label: 'Name',
            child: TextField(controller: _liaisonName, autofocus: true),
          ),
        ],
        submitLabel: 'Add agent',
        onSubmit: _busy
            ? null
            : () => _run(
                (backend) =>
                    backend.addPipeLiaison(pipe.id, _liaisonName.text.trim()),
              ),
        onCancel: () => setState(() => _addingLiaison = false),
      );
    }
    final agents = AppScope.of(context).workspace.agentsIn(pipe.projectId);
    if (_lead.isNotEmpty && !_members.contains(_lead)) _lead = '';
    final open =
        pipe.state == PipeState.PIPE_STATE_OPEN ||
        pipe.state == PipeState.PIPE_STATE_PAUSED;
    return FormFrame(
      title: 'Pipe settings',
      error: _error,
      submitLabel: 'Save',
      onSubmit: _busy ? null : _save,
      onCancel: widget.close,
      secondary: pipe.state == PipeState.PIPE_STATE_CLOSED
          ? null
          : Button(
              label: 'Close pipe…',
              onPressed: () => setState(() {
                _confirmingClose = true;
                _error = null;
              }),
            ),
      fields: [
        Field(
          label: 'Name',
          child: TextField(controller: _name, autofocus: true),
        ),
        Field(
          label: 'What it’s for',
          note: '$who sees this.',
          child: TextField(controller: _purpose),
        ),
        Field(
          label: 'Instructions',
          note:
              'What your agents should know about this pipe. They get it '
              'after Claktory’s own safety rules. $who never sees it.',
          child: TextField(
            controller: _prompt,
            minLines: 3,
            maxLines: 8,
            placeholder:
                'They may ask for launch dates and screenshots; never send '
                'customer data or code.',
          ),
        ),
        Field(
          label: 'Who may use it',
          note: agents.isEmpty ? 'No agents in this project yet.' : null,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              for (final agent in agents)
                Padding(
                  padding: const EdgeInsets.only(bottom: Space.xs),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      _MemberLine(
                        agent: agent,
                        value: _members.contains(agent.id),
                        onChanged: (on) => setState(() {
                          if (on) {
                            _members.add(agent.id);
                            // A liaison is limited unless the person
                            // lets it run commands.
                            if (!pipe.memberIds.contains(agent.id) &&
                                agent.role.trim().toLowerCase() == 'liaison') {
                              _limited.add(agent.id);
                            }
                          } else {
                            _members.remove(agent.id);
                          }
                        }),
                      ),
                      if (_members.contains(agent.id))
                        _CommandsLine(
                          agent: agent,
                          pipe: pipe,
                          value: !_limited.contains(agent.id),
                          onChanged: (on) => setState(() {
                            if (on) {
                              // Letting it out of the pipe is asked first.
                              _letting = agent;
                              _error = null;
                            } else {
                              _limited.add(agent.id);
                            }
                          }),
                        ),
                    ],
                  ),
                ),
              if (open)
                Button(
                  label: 'Add a liaison agent…',
                  onPressed: () => setState(() {
                    _addingLiaison = true;
                    _error = null;
                  }),
                ),
            ],
          ),
        ),
        Field(
          label: 'Lead',
          note: 'Told of each message that comes in.',
          child: Select<String>(
            value: _lead,
            options: [
              const SelectOption('', 'No lead'),
              for (final agent in agents)
                if (_members.contains(agent.id))
                  SelectOption(agent.id, agent.name),
            ],
            onChanged: (lead) => setState(() => _lead = lead),
          ),
        ),
        Field(
          label: 'Review outgoing',
          note: _review
              ? 'Each message from your agents waits here for your Send.'
              : 'Your agents’ messages go out without asking you.',
          child: _SwitchLine(
            label: 'Review messages before they go out',
            value: _review,
            onChanged: (on) => setState(() => _review = on),
          ),
        ),
        Field(
          label: 'Limits',
          note:
              '${pipe.limits.messagesLeft} messages and '
              '${pipe.limits.fileMbLeft} MB of files left now. Beyond them, '
              'messages wait for you.',
          child: Row(
            children: [
              Expanded(
                child: _NumberField(label: 'An hour', controller: _perHour),
              ),
              const SizedBox(width: Space.s),
              Expanded(
                child: _NumberField(label: 'In a row', controller: _burst),
              ),
              const SizedBox(width: Space.s),
              Expanded(
                child: _NumberField(label: 'MB a day', controller: _fileMb),
              ),
            ],
          ),
        ),
        if (pipe.state == PipeState.PIPE_STATE_OPEN ||
            (pipe.state == PipeState.PIPE_STATE_PAUSED &&
                pipe.paused == PipePause.PIPE_PAUSE_BY_ME))
          Field(
            label: 'Pause',
            note: 'Nothing goes through either way while it’s paused.',
            child: Align(
              alignment: Alignment.centerLeft,
              child: Button(
                label: pipe.state == PipeState.PIPE_STATE_OPEN
                    ? 'Pause pipe'
                    : 'Resume pipe',
                onPressed: _busy
                    ? null
                    : () => _run(
                        (backend) => backend.setPipeState(
                          pipe.id,
                          pipe.state == PipeState.PIPE_STATE_OPEN
                              ? PipeAction.PIPE_ACTION_PAUSE
                              : PipeAction.PIPE_ACTION_RESUME,
                        ),
                      ),
              ),
            ),
          ),
      ],
    );
  }
}

/// What a new liaison agent is, beside the choice that makes one.
const liaisonNote =
    'Safest. A new agent just for this pipe, which can only read and answer '
    "it: it can't run commands, reach your files or secrets, use the web or "
    'message your other agents.';

/// Under a member's line: whether it can run commands on this computer
/// (off, it's limited to the pipe), indented to its name, with what that
/// risks while it's on.
class _CommandsLine extends StatelessWidget {
  const _CommandsLine({
    required this.agent,
    required this.pipe,
    required this.value,
    required this.onChanged,
  });

  final Agent agent;
  final Pipe pipe;
  final bool value;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      // Under the name, past the 20px character and its gap.
      padding: const EdgeInsets.only(left: 20 + Space.s, top: Space.xs),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _SwitchLine(
            label: 'Can run commands on this computer',
            value: value,
            onChanged: onChanged,
            child: Text(
              'Can run commands on this computer',
              style: TextStyles.caption.copyWith(color: palette.text),
            ),
          ),
          if (value)
            Padding(
              padding: const EdgeInsets.only(top: 2, right: Space.xl),
              child: Text(
                canRunCommandsWarning(agent.name, pipe),
                style: TextStyles.caption.copyWith(color: palette.textMuted),
              ),
            ),
        ],
      ),
    );
  }
}

/// One of the project's agents, to let use a pipe or not: its character,
/// name and role, and a switch; a click anywhere on it turns it over.
class _MemberLine extends StatelessWidget {
  const _MemberLine({
    required this.agent,
    required this.value,
    required this.onChanged,
  });

  final Agent agent;
  final bool value;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return _SwitchLine(
      label: agent.name,
      value: value,
      onChanged: onChanged,
      leading: AgentCharacter(agent: agent, size: 20),
      child: NameThenRole(
        name: agent.name,
        role: agent.role,
        style: TextStyles.body.copyWith(color: palette.text),
        roleColor: palette.textMuted,
      ),
    );
  }
}

/// Words beside a switch, the whole line one click target for it.
class _SwitchLine extends StatelessWidget {
  const _SwitchLine({
    required this.label,
    required this.value,
    required this.onChanged,
    this.leading,
    this.child,
  });

  final String label;
  final bool value;
  final ValueChanged<bool> onChanged;
  final Widget? leading;

  /// In place of [label]'s plain text.
  final Widget? child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ToggleTap(
      value: value,
      onChanged: onChanged,
      child: Row(
        children: [
          if (leading case final leading?) ...[
            leading,
            const SizedBox(width: Space.s),
          ],
          Expanded(
            child:
                child ??
                Text(
                  label,
                  style: TextStyles.body.copyWith(color: palette.text),
                ),
          ),
          const SizedBox(width: Space.m),
          Toggle(value: value, onChanged: onChanged, label: label),
        ],
      ),
    );
  }
}

class _NumberField extends StatelessWidget {
  const _NumberField({required this.label, required this.controller});

  final String label;
  final TextEditingController controller;

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.start,
    children: [
      Text(
        label,
        style: TextStyles.caption.copyWith(
          color: Palette.of(context).textMuted,
        ),
      ),
      const SizedBox(height: Space.xs),
      TextField(controller: controller),
    ],
  );
}

/// Connect a pipe…, from a project: make an invite for someone else to use,
/// or use one they sent.
class ConnectPipeForm extends StatefulWidget {
  const ConnectPipeForm({
    super.key,
    required this.project,
    required this.close,
  });

  final Project project;
  final VoidCallback close;

  @override
  State<ConnectPipeForm> createState() => _ConnectPipeFormState();
}

enum _Step { choose, invite, code, preview, made }

class _ConnectPipeFormState extends State<ConnectPipeForm> {
  var _step = _Step.choose;
  late final _name = TextEditingController(text: '${widget.project.name} ⇄ ');
  final _purpose = TextEditingController();
  final _code = TextEditingController();
  PipeInvitePreview? _preview;
  PipeInvite? _invite;
  var _copied = false;
  var _busy = false;
  String? _error;

  @override
  void dispose() {
    _name.dispose();
    _purpose.dispose();
    _code.dispose();
    super.dispose();
  }

  Future<void> _run(Future<void> Function(Backend backend) request) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await request(AppScope.of(context));
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _go(_Step step) => setState(() {
    _step = step;
    _error = null;
  });

  /// Opens the pipe the invite made.
  void _openInvited(Backend backend) {
    final invite = _invite;
    if (invite == null) return;
    AppScope.closePage(context);
    fireAndLog(backend.selectPipe(invite.pipeId));
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final project = widget.project;
    switch (_step) {
      case _Step.choose:
        return FormFrame(
          title: 'Connect a pipe',
          body:
              'A pipe is a chat between ${project.name} and a project in '
              "someone else's Claktory. You choose which of your agents may "
              'use it, and nothing goes through until you both approve.',
          error: null,
          fields: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    'Someone sent you a code?',
                    style: TextStyles.body.copyWith(color: palette.textMuted),
                  ),
                ),
                const SizedBox(width: Space.s),
                Button(
                  label: 'I have a code',
                  onPressed: () => _go(_Step.code),
                ),
              ],
            ),
          ],
          submitLabel: 'Make an invite',
          onSubmit: () => _go(_Step.invite),
          onCancel: widget.close,
        );
      case _Step.invite:
        return ListenableBuilder(
          listenable: _name,
          builder: (context, _) => FormFrame(
            title: 'Make an invite',
            error: _error,
            submitLabel: 'Make invite',
            onSubmit: _busy || _name.text.trim().isEmpty
                ? null
                : () => _run((backend) async {
                    final invite = await backend.createPipeInvite(
                      project.id,
                      _name.text.trim(),
                      _purpose.text.trim(),
                    );
                    if (!mounted) return;
                    setState(() {
                      _invite = invite;
                      _step = _Step.made;
                    });
                  }),
            onCancel: () => _go(_Step.choose),
            cancelLabel: 'Back',
            fields: [
              Field(
                label: 'Name',
                child: TextField(controller: _name, autofocus: true),
              ),
              Field(
                label: 'What it’s for',
                note: 'One line. The other person sees it before they accept.',
                child: TextField(
                  controller: _purpose,
                  placeholder: 'We run the website; ask us for launch dates.',
                ),
              ),
            ],
          ),
        );
      case _Step.made:
        final invite = _invite!;
        return FormFrame(
          title: 'Send this code',
          body:
              'Send it to the person you want to connect with, privately. '
              'They paste it in their Claktory, in Connect a pipe… → I have a '
              'code. It works once, for 10 minutes, and this is the only '
              'time it shows.',
          error: null,
          submitLabel: 'Done',
          onSubmit: () {
            _openInvited(AppScope.of(context));
            widget.close();
          },
          onCancel: widget.close,
          cancelLabel: 'Close',
          fields: [
            Container(
              padding: const EdgeInsets.all(Space.m),
              decoration: BoxDecoration(
                border: Border.all(color: palette.hairline),
                borderRadius: BorderRadius.circular(Radii.control),
              ),
              child: SelectableArea(
                child: Text(
                  invite.code,
                  style: TextStyles.caption.copyWith(
                    fontFamily: TextStyles.mono,
                    color: palette.text,
                  ),
                ),
              ),
            ),
            const SizedBox(height: Space.s),
            Align(
              alignment: Alignment.centerLeft,
              child: Button(
                label: _copied ? 'Copied' : 'Copy code',
                onPressed: () async {
                  await Clipboard.setData(ClipboardData(text: invite.code));
                  if (mounted) setState(() => _copied = true);
                },
              ),
            ),
          ],
        );
      case _Step.code:
        return ListenableBuilder(
          listenable: _code,
          builder: (context, _) => FormFrame(
            title: 'Use a pipe code',
            body: 'Paste the code you were sent.',
            error: _error,
            submitLabel: 'Continue',
            onSubmit: _busy || _code.text.trim().isEmpty
                ? null
                : () => _run((backend) async {
                    final preview = await backend.readPipeInvite(
                      _code.text.trim(),
                    );
                    if (!mounted) return;
                    _name.text = preview.name;
                    setState(() {
                      _preview = preview;
                      _step = _Step.preview;
                    });
                  }),
            onCancel: () => _go(_Step.choose),
            cancelLabel: 'Back',
            fields: [
              Field(
                label: 'Code',
                child: TextField(
                  controller: _code,
                  autofocus: true,
                  placeholder: 'claktory-pipe://…',
                ),
              ),
            ],
          ),
        );
      case _Step.preview:
        final preview = _preview!;
        final peer = preview.peer;
        final who = [
          if (peer.ownerName.isNotEmpty) peer.ownerName,
          if (peer.ownerEmail.isNotEmpty) peer.ownerEmail,
          if (peer.claktoryName.isNotEmpty) peer.claktoryName,
        ].join(' · ');
        return FormFrame(
          title:
              'Connect ${project.name} with '
              '${peer.projectName.isEmpty ? 'their project' : peer.projectName}?',
          body: [
            if (who.isNotEmpty) 'From $who (not verified).',
            if (peer.purpose.isNotEmpty) '“${peer.purpose}”',
            'You both approve next, comparing digits, before anything goes '
                'through.',
          ].join('\n\n'),
          error: _error,
          submitLabel: 'Connect',
          onSubmit: _busy
              ? null
              : () => _run((backend) async {
                  await backend.acceptPipeInvite(
                    preview.code,
                    project.id,
                    name: _name.text.trim(),
                    purpose: _purpose.text.trim(),
                  );
                  // The backend selects the new pipe itself.
                  if (!mounted) return;
                  AppScope.closePage(this.context);
                  widget.close();
                }),
          onCancel: () => _go(_Step.code),
          cancelLabel: 'Back',
          fields: [
            Field(
              label: 'Name',
              child: TextField(controller: _name),
            ),
            Field(
              label: 'What it’s for',
              note:
                  '${peer.ownerName.isEmpty ? 'They see' : '${peer.ownerName} sees'} this.',
              child: TextField(controller: _purpose, autofocus: true),
            ),
          ],
        );
    }
  }
}

/// An agent that read a pipe ran a command with a secret: Claktory held it
/// for the person. A sibling of the secret request card.
class SecretApprovalCard extends StatefulWidget {
  const SecretApprovalCard(this.approval, {super.key, required this.seq});

  final SecretApproval approval;

  /// The entry's seq in the agent's conversation.
  final int seq;

  @override
  State<SecretApprovalCard> createState() => _SecretApprovalCardState();
}

class _SecretApprovalCardState extends State<SecretApprovalCard> {
  var _busy = false;
  String? _error;

  Future<void> _decide(bool allow) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context)
          .decideSecretApproval(widget.approval.agentId, widget.seq, allow);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final approval = widget.approval;
    final agent = AppScope.of(context).workspace.agent(approval.agentId);
    final color = agentColor(agent?.color ?? 'slate');
    final name = agent?.name ?? approval.agentId;
    final pending =
        approval.state == SecretApprovalState.SECRET_APPROVAL_STATE_PENDING;
    final secrets = approval.secretNames.join(', ');
    final pipes = approval.pipeNames.join(', ');
    final decided = approval.decidedMs.toInt();
    return Container(
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
                  '$name wants to use $secrets',
                  style: TextStyles.caption.copyWith(
                    color: color,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.xs),
          Text(
            '$name has read the pipe $pipes, so Claktory asks you before it '
            'uses a secret. Allow it only if this command is what you '
            'expect.',
            style: TextStyles.body.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.s),
          Container(
            padding: const EdgeInsets.all(Space.s),
            decoration: BoxDecoration(
              color: palette.sidebar,
              border: Border.all(color: palette.hairline),
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Text(
              approval.command,
              style: TextStyles.caption.copyWith(
                fontFamily: TextStyles.mono,
                color: palette.text,
              ),
            ),
          ),
          const SizedBox(height: Space.m),
          if (pending)
            Row(
              children: [
                Expanded(
                  child: Text(
                    _error ?? 'If you allow it, this command runs once.',
                    style: TextStyles.caption.copyWith(
                      color: _error == null
                          ? palette.textFaint
                          : palette.danger,
                    ),
                  ),
                ),
                const SizedBox(width: Space.s),
                Button(
                  label: "Don't allow",
                  onPressed: _busy ? null : () => _decide(false),
                ),
                const SizedBox(width: Space.s),
                Button(
                  label: 'Allow once',
                  primary: true,
                  onPressed: _busy ? null : () => _decide(true),
                ),
              ],
            )
          else
            Row(
              children: [
                if (approval.state !=
                    SecretApprovalState.SECRET_APPROVAL_STATE_EXPIRED) ...[
                  const PersonAvatar(size: 16),
                  const SizedBox(width: Space.xs + 2),
                ],
                Flexible(
                  child: Text.rich(
                    TextSpan(
                      children: [
                        TextSpan(
                          text: switch (approval.state) {
                            SecretApprovalState.SECRET_APPROVAL_STATE_ALLOWED =>
                              'Allowed once',
                            SecretApprovalState.SECRET_APPROVAL_STATE_DENIED =>
                              'Not allowed',
                            _ => 'Expired',
                          },
                          style: TextStyle(
                            color:
                                approval.state ==
                                    SecretApprovalState
                                        .SECRET_APPROVAL_STATE_ALLOWED
                                ? color
                                : palette.textMuted,
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
                    style: TextStyles.caption,
                  ),
                ),
              ],
            ),
        ],
      ),
    );
  }
}
