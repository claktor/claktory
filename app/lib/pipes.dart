import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_name.dart';
import 'avatar.dart';
import 'backend.dart';
import 'card_answer.dart'
    show AnswerNotSent, CardAnswer, PublishesAnswer, insteadLine, noComment;
import 'characters/character.dart';
import 'controls.dart';
import 'decision.dart';
import 'extensions.dart' show logTime;
import 'conversation_find.dart';
import 'find.dart' show FindHighlight, FindScope;
import 'forms.dart';
import 'identity.dart';
import 'mobile/header.dart' show HeaderButton, PhoneHeader;
import 'mobile/metrics.dart' show TypeScale;
import 'mobile/page_find.dart' show PhonePageFind, phonePageSearch;
import 'mobile/input.dart' show InputKind;
import 'mobile/lines.dart' show ActionLine;
import 'mobile/popover.dart' show presentPopover;
import 'account.dart' show askSignIn;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'pairing.dart' show PairingDigits;
import 'pipe_groups.dart' show groupOf, moveToGroup, removeFromGroup;
import 'pipe_start.dart' show PipeInviteQr, copyInviteCode, pipeInviteCheck;
import 'scan_code.dart';
import 'pipe_shares.dart'
    show PipeSharesField, PipeSharesStrip, ShareAnnouncement;
import 'pipe_reports.dart'
    show ForwardReportsField, PipeReportsStrip, forwardApprovalLine;
import 'pipe_timeline.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'resources.dart' show formatBytes;
import 'scope.dart';
import 'selection.dart';
import 'state.dart' show PipeChat;
import 'taint_label.dart' show TaintLabel, taintPages, taintRead;
import 'transcript.dart' show Bubble, userAvatarSize;
import 'usage.dart' show formatUntil;
import 'visible_codes.dart' show proseCodes, visibleCodes;
import 'waiting.dart' show EntryReveal;

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
String pipePeerName(Pipe pipe, {bool start = false}) {
  final peer = pipe.peer;
  // Their own words: what reorders text shows by its code (Fugo).
  if (peer.ownerName.isNotEmpty) return proseCodes(peer.ownerName);
  if (peer.claktoryName.isNotEmpty) return proseCodes(peer.claktoryName);
  // At the start of a sentence, capitalized (Zucchero's row 175: a pipe
  // whose invite isn't used yet has no name for them).
  return start ? 'The other side' : 'the other side';
}

/// Who on this side hears the person's decision on [message], held in
/// [pipe], with their comment: for one going out, the agent that wrote it
/// (null for the person's own: no one); for one coming in, the pipe's
/// lead (its liaison, when it has one). Never the other side.
String? pipeDecisionHearer(
  Workspace workspace,
  Pipe pipe,
  PipeMessage message,
) {
  final author = message.author;
  final String id;
  if (message.state == PipeMessageState.PIPE_MESSAGE_STATE_HELD) {
    if (author.kind != PipeAuthorKind.PIPE_AUTHOR_KIND_MY_AGENT) return null;
    id = author.agentId;
  } else {
    id = pipe.leadId;
  }
  final agent = workspace.agents.where((agent) => agent.id == id).firstOrNull;
  return agent?.name ?? (id.isEmpty ? null : 'Your agent');
}

/// The lead Claktory gives a pipe with [members] (those in its project)
/// when its own isn't one of them, as the backend picks it: only an agent
/// limited to the pipe ([limited]), never one that can run commands. Its
/// liaison ([liaisonId]) first, else the first of them; else "": no lead,
/// for the person to choose.
String pipeLeadFor(
  Iterable<String> members, {
  Set<String> limited = const {},
  String liaisonId = '',
}) {
  final contained = members.where(limited.contains);
  if (contained.contains(liaisonId)) return liaisonId;
  return contained.firstOrNull ?? '';
}

/// The other side of [pipe] as its Claktory is called ("Ana's Claktory"),
/// else its person's name: the words for what an agent read or is limited
/// to.
String pipePeerClaktory(Pipe pipe) => pipe.peer.claktoryName.isNotEmpty
    ? proseCodes(pipe.peer.claktoryName)
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

/// How many things in [pipe] wait for the person: messages held for them
/// or that no agent answers, its approval, the loop breaker's Continue,
/// and trusting its other side again on another account.
int pipeWaiting(Pipe pipe) =>
    pipe.waitingSeqs.length +
    pipe.unansweredSeqs.length +
    (pipeAccountChanged(pipe) ? 1 : 0) +
    (pipe.state == PipeState.PIPE_STATE_APPROVAL ? 1 : 0) +
    (pipe.state == PipeState.PIPE_STATE_PAUSED &&
            pipe.paused == PipePause.PIPE_PAUSE_LOOP
        ? 1
        : 0);

/// What waits for the person in [pipe] and in its liaison's conversation
/// (its cards: a question, a secret to allow), for the pipe's row, as the
/// liaison has none of its own.
int pipeWaitingAll(Workspace workspace, Pipe pipe) =>
    pipeWaiting(pipe) + (pipeLiaison(workspace, pipe)?.waitingSeqs.length ?? 0);

/// An open pipe with no lead: no agent is told of what arrives.
const pipeUnanswered = 'No one answers this pipe';

/// Under the settings' Lead while a pipe with agents has none: Claktory
/// never makes one that can run commands its lead.
const pipeNoLeadNote =
    'No agent sees what arrives. Choose a lead to answer it.';

/// What to do about it, under each message no agent answered.
const pipeChooseLead =
    'No agent has seen this message. Choose a lead in the pipe’s '
    'settings to have it answer.';

/// Where [pipe] stands, in a few words, for its sidebar row and header.
/// With its [lead] ([pipeLead]), open and online: the lead's usage wait in
/// place of "Online" (#343 U18), since nothing is answered meanwhile. Only
/// ever on this side: the wait's words never go to the other.
String pipeStatus(Pipe pipe, {Agent? lead, DateTime? now}) {
  final who = pipePeerName(pipe);
  // Past its daily spending limit, the lead answers nothing from the other
  // side until midnight (#357 L1).
  if (pipe.state == PipeState.PIPE_STATE_OPEN && pipeLeadOverLimit(lead)) {
    return pipeOverLimitLine;
  }
  if (pipe.state == PipeState.PIPE_STATE_OPEN &&
      pipe.online &&
      lead != null &&
      lead.usageWait.isNotEmpty) {
    return liaisonWaitLine(lead, now: now ?? DateTime.now());
  }
  // The other side's liaison waits (#343 U18): in our words, with this
  // side's own name for the pipe, never anything they wrote.
  if (pipe.state == PipeState.PIPE_STATE_OPEN &&
      pipe.online &&
      pipe.peerWaiting) {
    return peerWaitLine(pipe, now: now ?? DateTime.now());
  }
  return switch (pipe.state) {
    PipeState.PIPE_STATE_INVITED => 'Waiting for your invite to be used',
    PipeState.PIPE_STATE_JOINING => 'Connecting…',
    PipeState.PIPE_STATE_APPROVAL => 'Waiting for your approval',
    PipeState.PIPE_STATE_WAITING_PEER => 'Waiting for $who to approve',
    // No agent is told of what arrives: say so first.
    PipeState.PIPE_STATE_OPEN when pipe.leadId.isEmpty =>
      pipe.online ? pipeUnanswered : 'Offline · $pipeUnanswered',
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

/// A pipe's row while its lead is past its daily spending limit.
const pipeOverLimitLine = 'Paused until midnight: daily limit reached';

/// Whether [lead] is past its daily spending limit (ContainedSpend.over):
/// what the other side sends waits until midnight.
bool pipeLeadOverLimit(Agent? lead) =>
    lead != null && lead.hasSpend() && lead.spend.over;

/// The agent [pipe] wakes when something arrives (Pipe.lead_id, its
/// liaison when it has one); null when it has none.
Agent? pipeLead(Workspace workspace, Pipe pipe) {
  if (pipe.leadId.isEmpty) return null;
  for (final agent in workspace.agents) {
    if (agent.id == pipe.leadId) return agent;
  }
  return null;
}

/// A pipe's lead [liaison] held up by its provider's usage (#343 U18): "Kai
/// is waiting until Fri 18:00", or "Kai is waiting" when it waits for the
/// person (out of credit, a sign-in).
String liaisonWaitLine(Agent liaison, {required DateTime now}) {
  final name = proseCodes(liaison.name);
  final ms = liaison.usageWaitUntilMs.toInt();
  return ms > now.millisecondsSinceEpoch
      ? '$name is waiting until ${formatUntil(ms, now)}'
      : '$name is waiting';
}

/// [pipe]'s other side waiting to answer (Pipe.peer_waiting): "Acme is
/// waiting until Fri 18:00", or "Acme is waiting" with no known end, named
/// by this side's own name for the pipe.
String peerWaitLine(Pipe pipe, {required DateTime now}) {
  final name = pipe.name.isNotEmpty ? proseCodes(pipe.name) : 'The other side';
  final ms = pipe.peerWaitingUntilMs.toInt();
  return ms > now.millisecondsSinceEpoch
      ? '$name is waiting until ${formatUntil(ms, now)}'
      : '$name is waiting';
}

/// The name to prefill for [pipe]'s new liaison: Claktory's suggestion
/// (Pipe.suggested_liaison_name), else "Liaison".
String liaisonNameFor(Pipe pipe) => pipe.suggestedLiaisonName.isNotEmpty
    ? pipe.suggestedLiaisonName
    : 'Liaison';

/// While [pipe]'s other side can't be reached: since when and why, when
/// Claktory knows (Pipe.offline_reason), and that messages wait.
String pipeOfflineText(Pipe pipe) {
  final who = pipePeerName(pipe, start: true);
  final reason = pipe.offlineReason;
  final since = pipe.offlineSinceMs > 0
      ? ' since ${logTime(DateTime.fromMillisecondsSinceEpoch(pipe.offlineSinceMs.toInt()))}'
      : '';
  final head = reason.isEmpty
      ? '$who’s Claktory is offline.'
      : '$who’s Claktory has been offline$since: $reason.';
  return pipe.queued > 0
      ? '$head Messages wait until it’s back · ${pipe.queued} waiting to send'
      : '$head Messages wait until it’s back.';
}

/// What to check when a join can't get through: the Local Network
/// permission of the Mac the backend runs on, which a declined prompt
/// turns off. Shown only when that's a Mac.
const pipeLocalNetworkHint =
    'If the Mac Claktory runs on asked to find devices on your local '
    'network and you said no, allow it in System Settings → Privacy & '
    'Security → Local Network.';

/// Whether the backend runs on a Mac (Workspace.os); false while it
/// doesn't say.
bool backendOnMac(Workspace workspace) => workspace.os == 'macos';

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
    // Its liaison's conversation, with the pipe's messages among it, once
    // the pipe is open: the box writes to the liaison or through the pipe.
    final liaison = switch (pipe.state) {
      PipeState.PIPE_STATE_OPEN ||
      PipeState.PIPE_STATE_PAUSED ||
      PipeState.PIPE_STATE_CLOSED => pipeLiaison(store.workspace, pipe),
      _ => null,
    };
    final Widget body = switch (pipe.state) {
      PipeState.PIPE_STATE_APPROVAL => PipeApprovalCard(pipe: pipe),
      PipeState.PIPE_STATE_INVITED ||
      PipeState.PIPE_STATE_JOINING ||
      PipeState.PIPE_STATE_WAITING_PEER => _Waiting(pipe: pipe),
      _ when liaison != null => LiaisonPipeBody(
        key: ValueKey('liaison/${liaison.id}'),
        pipe: pipe,
        liaison: liaison,
        phone: onMenu != null,
      ),
      _ => _PipeChatView(pipe: pipe),
    };
    return DefaultTextStyle(
      style: TextStyles.body.copyWith(color: palette.text),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _PipeHeader(
            pipe: pipe,
            project: project,
            liaison: liaison,
            onMenu: onMenu,
          ),
          PipeIdentityNotice(pipe: pipe),
          // Its lead past its spending limit, with the limit to raise.
          PipeSpendNotice(pipe: pipe),
          // Websites its other side shares with this one, to open.
          PipeSharesStrip(pipe: pipe),
          // Its reports: where they go, and what was forwarded.
          PipeReportsStrip(pipe: pipe),
          Expanded(child: body),
          if (liaison == null) _PipeFoot(pipe: pipe),
        ],
      ),
    );
  }
}

class _PipeHeader extends StatelessWidget {
  const _PipeHeader({
    required this.pipe,
    required this.project,
    this.liaison,
    this.onMenu,
  });

  final Pipe pipe;
  final Project? project;

  /// The pipe's liaison, whose conversation shows: its face and its
  /// "…" (Liaison settings…, Clear conversation…) end the header, and the
  /// working wave runs along its line while it works.
  final Agent? liaison;
  final VoidCallback? onMenu;

  @override
  Widget build(BuildContext context) {
    if (onMenu case final onMenu?) return _phone(context, onMenu);
    final palette = Palette.of(context);
    final peer = pipe.peer;
    final ends = [
      if (project != null) project!.name,
      if (peer.projectName.isNotEmpty)
        '${proseCodes(peer.projectName)} in ${pipePeerClaktory(pipe)}',
    ].join(' ⇄ ');
    final liaison = this.liaison;
    final header = DecoratedBox(
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
                if (pipe.state != PipeState.PIPE_STATE_INVITED) ...[
                  PipeVerifiedMark(pipe: pipe),
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
                    pipeStatus(
                      pipe,
                      lead: pipeLead(
                        AppScope.of(context).workspace.workspace,
                        pipe,
                      ),
                    ),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyles.body.copyWith(color: palette.textMuted),
                  ),
                ),
                if (liaison != null) ...[
                  const SizedBox(width: Space.m),
                  Semantics(
                    label: liaison.name,
                    child: AgentCharacter(agent: liaison, size: 24, live: true),
                  ),
                ],
                // Always: Move to group… (#359), and the liaison's own.
                SizedBox(width: liaison == null ? Space.m : Space.xs),
                GlyphButton(
                  label: 'More',
                  active: false,
                  onTap: () => _pipeActions(context, pipe, liaison),
                  glyph: (color) => ExtensionGlyph(
                    icon: 'ellipsis',
                    name: 'More',
                    color: color,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
    if (liaison?.status != AgentStatus.AGENT_STATUS_WORKING) return header;
    return Stack(
      children: [
        header,
        const Positioned(
          left: 0,
          right: 0,
          bottom: 0,
          height: 2,
          child: WorkingWave(color: Color(0xFFFFFFFF), peak: 0.6),
        ),
      ],
    );
  }
}

extension on _PipeHeader {
  /// On a phone, as an agent's header: Menu, the portal and the name over
  /// one line of where it stands ("Online · Not verified"), then the
  /// liaison's face and its "…"; the name always keeps its room.
  Widget _phone(BuildContext context, VoidCallback onMenu) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final liaison = this.liaison;
    final status = [
      pipeStatus(
        pipe,
        lead: pipeLead(AppScope.of(context).workspace.workspace, pipe),
      ),
      if (pipe.state != PipeState.PIPE_STATE_INVITED) verifiedLabel(pipe),
      if (pipe.canRunCommandsIds.isNotEmpty)
        canRunCommandsText(pipe.canRunCommandsIds.length),
    ].where((part) => part.isNotEmpty).join(' · ');
    return PhoneHeader(
      background: Color.alphaBlend(
        pipeColor.withValues(alpha: 0.14),
        palette.canvas,
      ),
      hairline: false,
      leading: HeaderButton.menu(onTap: onMenu),
      title: Row(
        children: [
          const ExcludeSemantics(child: PortalGlyph(size: 22)),
          const SizedBox(width: Space.s + 2),
          Flexible(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  pipe.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: type.emphasis.copyWith(color: palette.text),
                ),
                if (status.isNotEmpty)
                  Text(
                    status,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: type.caption.copyWith(color: palette.textMuted),
                  ),
              ],
            ),
          ),
        ],
      ),
      titleLabel: [pipe.name, if (status.isNotEmpty) status].join(', '),
      titleHint: 'Opens pipe settings',
      onTitleTap: () => showPipeSettings(context, pipe),
      trailing: [
        if (liaison != null)
          ExcludeSemantics(
            child: AgentCharacter(agent: liaison, size: 24, live: true),
          ),
        HeaderButton.more(
          label: '${pipe.name}: more',
          onTap: () => _pipeActions(context, pipe, liaison),
        ),
      ],
      bottom: SizedBox(
        height: 2,
        child: Stack(
          fit: StackFit.expand,
          children: [
            ColoredBox(color: pipeColor.withValues(alpha: 0.7)),
            if (liaison?.status == AgentStatus.AGENT_STATUS_WORKING)
              const WorkingWave(color: Color(0xFFFFFFFF), peak: 0.6),
          ],
        ),
      ),
    );
  }
}

/// What can be done from the pipe's "…": on a phone, search the pipe;
/// move it to a group (#359), or out of its own; with its liaison, its
/// settings, and clearing its conversation.
Future<void> _pipeActions(
  BuildContext context,
  Pipe pipe,
  Agent? liaison,
) async {
  final search = phonePageSearch(context);
  final grouped =
      groupOf(AppScope.of(context).workspace.workspace, pipe) != null;
  final action = await showActions<String>(
    context,
    title: pipe.name,
    actions: [
      if (search != null)
        SheetAction(
          'Search',
          'search',
          glyph: (color) => ExtensionGlyph(
            icon: 'search',
            name: 'Search',
            color: color,
            size: 16,
          ),
        ),
      const SheetAction('Move to group…', 'group'),
      if (grouped) const SheetAction('Remove from group', 'ungroup'),
      if (liaison != null) ...const [
        SheetAction('Liaison settings…', 'settings'),
        SheetAction('Clear conversation…', 'clear'),
      ],
    ],
  );
  if (!context.mounted) return;
  switch (action) {
    case 'search':
      search?.call();
    case 'group':
      await moveToGroup(context, pipe);
    case 'ungroup':
      removeFromGroup(context, pipe);
    case 'settings' when liaison != null:
      await showModal(
        context,
        (context, close) => AgentSettingsForm(agent: liaison, close: close),
      );
    case 'clear' when liaison != null:
      await showModal(
        context,
        (context, close) => _ClearConversation(agent: liaison, close: close),
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
/// [choice]: under the three-way choice of the pipe's settings (#414),
/// rather than a switch.
String canRunCommandsWarning(String agent, Pipe pipe, {bool choice = false}) =>
    '$agent can run commands on this computer. If a message from '
    '${pipePeerClaktory(pipe)} misleads it, it could reach your files, '
    'secrets and network before you see anything. '
    '${choice ? 'Choose Limited to the pipe or Only sends' : 'Turn this off'} '
    'unless it needs to run commands.';

/// What "Not verified" means for [pipe]: what is and isn't checked about
/// the other side.
/// [signedIn]: whether this Claktory is signed in to claktory.com, so
/// only the other side's signing in is missing.
String notVerifiedNote(Pipe pipe, {required bool signedIn}) {
  final name = pipePeerName(pipe);
  return 'Messages here come only from the Claktory you connected to, but '
      "its name and email are only what it says about itself. Claktory "
      "hasn't confirmed whose it is. "
      '${signedIn ? 'When $name signs in' : "When you're both signed in"} '
      'to claktory.com, Claktory confirms it and shows their email here.';
}

/// What "Verified" means for [pipe].
String verifiedNote(Pipe pipe) =>
    "Claktory confirmed this pipe is with ${pipe.peer.verifiedEmail}'s "
    'Claktory.';

/// Whether this Claktory is signed in to its account service, which
/// vouches for whose Claktory is at a pipe's other end.
bool _signedIn(BuildContext context) {
  final workspace = AppScope.of(context).workspace.workspace;
  return workspace.hasAccount() &&
      workspace.account.state ==
          ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN;
}

/// [pipe]'s other side in a few words: "Verified · ana@example.com", or
/// "Not verified".
String verifiedLabel(Pipe pipe) =>
    pipe.peer.verified && pipe.peer.verifiedEmail.isNotEmpty
    ? 'Verified · ${pipe.peer.verifiedEmail}'
    : 'Not verified';

/// [pipe]'s mark: "Verified · ana@example.com" or "Not verified".
class PipeVerifiedMark extends StatelessWidget {
  const PipeVerifiedMark({super.key, required this.pipe});

  final Pipe pipe;

  @override
  Widget build(BuildContext context) =>
      pipe.peer.verified && pipe.peer.verifiedEmail.isNotEmpty
      ? _ExplainedMark(
          text: verifiedLabel(pipe),
          title: 'Verified',
          note: (_) => verifiedNote(pipe),
        )
      : NotVerifiedMark(pipe: pipe);
}

/// "Not verified", in a hairline capsule that opens what it means: a
/// popover pointing at it (Mac, iPad), a sheet on a phone. While this
/// Claktory isn't signed in, with Sign in…
class NotVerifiedMark extends StatelessWidget {
  const NotVerifiedMark({super.key, required this.pipe});

  final Pipe pipe;

  @override
  Widget build(BuildContext context) {
    final signedIn = _signedIn(context);
    return _ExplainedMark(
      text: 'Not verified',
      title: 'Not verified',
      note: (_) => notVerifiedNote(pipe, signedIn: signedIn),
      signIn: !signedIn,
    );
  }
}

/// [text] in a hairline capsule that opens [title] over [note].
class _ExplainedMark extends StatelessWidget {
  const _ExplainedMark({
    required this.text,
    required this.title,
    required this.note,
    this.signIn = false,
  });

  final String text;
  final String title;
  final String Function(BuildContext context) note;

  /// With Sign in…, for this Claktory to sign in to claktory.com.
  final bool signIn;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    void open() {
      Widget explained(BuildContext context, VoidCallback close) {
        final type = TypeScale.of(context);
        final palette = Palette.of(context);
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          spacing: Space.xs,
          children: [
            Semantics(
              header: true,
              child: Text(
                title,
                style: type.emphasis.copyWith(color: palette.text),
              ),
            ),
            Text(
              note(context),
              style: type.body.copyWith(color: palette.textMuted),
            ),
            if (signIn)
              Padding(
                padding: const EdgeInsets.only(top: Space.s),
                child: Align(
                  alignment: AlignmentDirectional.centerEnd,
                  child: Button(
                    label: 'Sign in…',
                    onPressed: () {
                      close();
                      askSignIn(AppScope.of(context));
                    },
                  ),
                ),
              ),
          ],
        );
      }

      if (isPhoneLayout(context)) {
        unawaited(
          showModal(
            context,
            (context, close) => Padding(
              padding: const EdgeInsets.all(Space.l),
              child: explained(context, close),
            ),
          ),
        );
        return;
      }
      unawaited(
        presentPopover<void>(
          context,
          builder: (context, close) => Padding(
            padding: const EdgeInsets.symmetric(
              horizontal: Space.l,
              vertical: Space.s,
            ),
            child: explained(context, close),
          ),
        ),
      );
    }

    return Unselectable(
      child: Semantics(
        button: true,
        label: text,
        hint: 'Says what this means',
        excludeSemantics: true,
        child: Pressable(
          onTap: open,
          builder: (context, hovered) => _Mark(
            text: text,
            color: hovered ? palette.text : palette.textMuted,
          ),
        ),
      ),
    );
  }
}

/// What a pipe's notice says about a change in who's at its other end
/// (PipePeer.identity_change): its title, what it means, and whether it's
/// the warning (another account). Empty while nothing changed.
(String, String, bool) pipeIdentityNoticeText(Pipe pipe) {
  final peer = pipe.peer;
  final was = peer.previousEmail;
  final now = peer.verifiedEmail;
  return switch (peer.identityChange) {
    PipeIdentityChange.PIPE_IDENTITY_CHANGE_LAPSED => (
      'No longer verified',
      "Claktory can no longer confirm this pipe is with $was's Claktory, "
          'perhaps because they signed out of claktory.com. Messages still '
          'come only from the same Claktory.',
      false,
    ),
    PipeIdentityChange.PIPE_IDENTITY_CHANGE_EMAIL_CHANGED => (
      'Email changed',
      "This pipe's Claktory is now $now, on the same account as $was. "
          "It's still verified.",
      false,
    ),
    PipeIdentityChange.PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED => (
      now.isEmpty
          ? "This pipe's Claktory is no longer confirmed as $was's."
          : "This pipe's Claktory is now confirmed as $now's, not $was's.",
      [
        'Check with ${pipePeerName(pipe)} before you go on.',
        'Until you check, their messages wait for you in Needs you, and '
            "your agents can't send here.",
      ].join(' '),
      true,
    ),
    _ => ('', '', false),
  };
}

/// Whether [pipe]'s other side is now confirmed as another account's, or
/// no longer as its own, and waits for the person to trust it again
/// (#471 G7); never for a closed pipe.
bool pipeAccountChanged(Pipe pipe) =>
    pipe.state != PipeState.PIPE_STATE_CLOSED &&
    pipe.peer.identityChange ==
        PipeIdentityChange.PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED;

/// Under a pipe's header while who's at its other end changed
/// (PipePeer.identity_change): no longer vouched for, or a new email, with
/// OK (the pin kept: "Not verified" until the same account vouches
/// again), or for another
/// account, a warning in `danger` with Trust this Claktory…, which asks
/// first. Nothing otherwise.
class PipeIdentityNotice extends StatefulWidget {
  const PipeIdentityNotice({
    super.key,
    required this.pipe,
    this.inline = false,
  });

  final Pipe pipe;

  /// In a card of its own (Needs you), not as a band under the header.
  final bool inline;

  @override
  State<PipeIdentityNotice> createState() => _PipeIdentityNoticeState();
}

class _PipeIdentityNoticeState extends State<PipeIdentityNotice>
    with PublishesAnswer {
  var _busy = false;
  String? _error;

  /// [rethrowing] for focus mode's keys (#514): refused, it throws too.
  Future<void> _accept({bool rethrowing = false}) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).acceptPipeIdentity(widget.pipe.id);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
      if (rethrowing) rethrow;
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// For focus mode's keys (#514): OK, or Trust this Claktory…, which asks
  /// first, as its button does (never one key, FS14).
  @override
  CardAnswer? answerNow() {
    final (title, _, danger) = pipeIdentityNoticeText(widget.pipe);
    if (title.isEmpty) return null;
    return CardAnswer(
      key: 'pipe:${widget.pipe.id}/identity',
      docked: false,
      yes: danger ? 'Trust this Claktory…' : 'OK',
      no: '',
      busy: _busy,
      error: _error,
      onYes: _busy
          ? null
          : danger
          ? () => _trust(rethrowing: true)
          : () => _accept(rethrowing: true),
    );
  }

  /// Asks before trusting the Claktory that changed; cancelled, nothing
  /// went ([AnswerNotSent] when [rethrowing]).
  Future<void> _trust({bool rethrowing = false}) async {
    final pipe = widget.pipe;
    final now = pipe.peer.verifiedEmail;
    Future<void>? accepting;
    await showModal(
      context,
      (context, close) => FormFrame(
                      title: now.isEmpty
                          ? 'Trust this Claktory?'
                          : "Trust this Claktory as $now's?",
                      body: [
                        'Messages waiting from it go through, and your '
                            'agents can send here again.',
                        // Nothing vouched for it: trusting it pins nothing.
                        if (now.isEmpty)
                          'The pipe then shows Not verified, since nothing '
                              'confirms whose Claktory it is now.',
                        'Do this only if ${pipePeerName(pipe)} told you '
                            'about the change.',
                      ].join(' '),
                      error: null,
                      fields: const [],
                      submitLabel: 'Trust',
                      onSubmit: () {
                        close();
                        accepting = _accept(rethrowing: rethrowing);
                      },
                      onCancel: close,
                    ),
    );
    final accepted = accepting;
    if (accepted == null) {
      if (rethrowing) throw const AnswerNotSent();
      return;
    }
    await accepted;
  }

  @override
  Widget build(BuildContext context) {
    publishAnswer();
    final pipe = widget.pipe;
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final (title, body, danger) = pipeIdentityNoticeText(pipe);
    if (title.isEmpty) return const SizedBox.shrink();
    final Widget action = danger
        ? Button(
            label: 'Trust this Claktory…',
            onPressed: _busy ? null : () => unawaited(_trust()),
          )
        : Button(label: 'OK', onPressed: _busy ? null : _accept);
    return Container(
      padding: widget.inline
          ? const EdgeInsets.all(Space.m)
          : const EdgeInsets.symmetric(horizontal: Space.xl, vertical: Space.m),
      decoration: BoxDecoration(
        color: danger ? palette.danger.withValues(alpha: 0.06) : null,
        border: widget.inline
            ? null
            : Border(bottom: BorderSide(color: palette.hairline)),
        borderRadius: widget.inline
            ? BorderRadius.circular(Radii.control)
            : null,
      ),
      child: ActionLine(
        action: action,
        lead: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          spacing: 2,
          children: [
            Text(
              title,
              style: type.body.copyWith(
                color: danger ? palette.danger : palette.text,
                fontWeight: FontWeight.w600,
              ),
            ),
            Text(
              body,
              style: type.body.copyWith(
                color: danger ? palette.text : palette.textMuted,
              ),
            ),
            if (_error case final error?)
              Text(error, style: type.caption.copyWith(color: palette.danger)),
          ],
        ),
      ),
    );
  }
}

/// Under a pipe's header while its lead is past its daily spending limit
/// (#357 L1): that the other side's messages wait until midnight, and the
/// limit, to raise it now. Nothing otherwise.
class PipeSpendNotice extends StatefulWidget {
  const PipeSpendNotice({super.key, required this.pipe});

  final Pipe pipe;

  @override
  State<PipeSpendNotice> createState() => _PipeSpendNoticeState();
}

class _PipeSpendNoticeState extends State<PipeSpendNotice> {
  final _cap = TextEditingController();
  var _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _cap.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _cap.dispose();
    super.dispose();
  }

  /// The dollars typed, when they're an amount above [spend]'s limit.
  double? _raised(ContainedSpend spend) {
    final usd = double.tryParse(_cap.text.trim().replaceFirst(r'$', ''));
    return usd != null && usd.isFinite && usd > spend.capUsd ? usd : null;
  }

  Future<void> _raise(Agent lead, double usd) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      // Its token cap as it is: 0 there would put the default back.
      await AppScope.of(context).setSpendCap(
        SetSpendCap(
          agentId: lead.id,
          capUsd: usd,
          capTokens: lead.spend.capTokens,
        ),
      );
      _cap.clear();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final lead = pipeLead(
      AppScope.of(context).workspace.workspace,
      widget.pipe,
    );
    if (widget.pipe.state != PipeState.PIPE_STATE_OPEN ||
        !pipeLeadOverLimit(lead)) {
      return const SizedBox.shrink();
    }
    final spend = lead!.spend;
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final name = proseCodes(lead.name);
    final raised = _raised(spend);
    return Container(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.xl,
        vertical: Space.m,
      ),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      child: ActionLine(
        lead: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          spacing: 2,
          children: [
            Text(
              spend.capUsd > 0
                  ? '$name reached today’s \$${_capText(spend)} limit'
                  : '$name reached today’s limit',
              style: type.body.copyWith(
                color: palette.text,
                fontWeight: FontWeight.w600,
              ),
            ),
            Text(
              'Messages from ${pipePeerName(widget.pipe)} wait until '
              'midnight. Raise the limit to have $name answer now.',
              style: type.body.copyWith(color: palette.textMuted),
            ),
            if (_error case final error?)
              Text(error, style: type.caption.copyWith(color: palette.danger)),
          ],
        ),
        action: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(r'$', style: type.body.copyWith(color: palette.textMuted)),
            const SizedBox(width: Space.xs),
            SizedBox(
              width: 72,
              child: TextField(
                controller: _cap,
                placeholder: _capText(spend).isEmpty ? '10' : _capText(spend),
                kind: InputKind.number,
                semanticsLabel: 'Daily spending limit in dollars',
                onSubmitted: (_) {
                  if (!_busy && raised != null) _raise(lead, raised);
                },
              ),
            ),
            const SizedBox(width: Space.s),
            Button(
              label: 'Raise limit',
              onPressed: _busy || raised == null
                  ? null
                  : () => _raise(lead, raised),
            ),
          ],
        ),
      ),
    );
  }
}

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
      PipeState.PIPE_STATE_JOINING when pipe.offlineReason.isNotEmpty => (
        'Can’t reach $who yet',
        [
          pipe.inviteExpiresMs > 0
              ? 'Claktory keeps trying until the code expires at '
                    '${logTime(DateTime.fromMillisecondsSinceEpoch(pipe.inviteExpiresMs.toInt()))}.'
              : 'Claktory keeps trying until the code expires.',
          'Last try: ${pipe.offlineReason}.',
        ].join(' '),
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
              if (pipe.state == PipeState.PIPE_STATE_JOINING &&
                  pipe.offlineReason.isNotEmpty &&
                  backendOnMac(AppScope.of(context).workspace.workspace)) ...[
                const SizedBox(height: Space.s),
                Text(
                  pipeLocalNetworkHint,
                  style: TextStyles.caption.copyWith(color: palette.textFaint),
                ),
              ],
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
    this.answers,
  });

  final Pipe pipe;
  final PipeAction action;
  final String label;
  final bool primary;

  /// The Needs you item it answers, as its yes for focus mode's keys
  /// (#514): a paused pipe's Continue.
  final String? answers;

  @override
  State<_StateButton> createState() => _StateButtonState();
}

class _StateButtonState extends State<_StateButton> with PublishesAnswer {
  var _busy = false;
  String? _error;

  /// [rethrowing] for focus mode's keys (#514): refused, it throws too.
  Future<void> _run({bool rethrowing = false}) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).setPipeState(widget.pipe.id, widget.action);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
      if (rethrowing) rethrow;
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  CardAnswer? answerNow() {
    final key = widget.answers;
    if (key == null) return null;
    return CardAnswer(
      key: key,
      docked: false,
      yes: widget.label,
      no: '',
      busy: _busy,
      error: _error,
      onYes: _busy ? null : () => _run(rethrowing: true),
    );
  }

  @override
  Widget build(BuildContext context) {
    publishAnswer();
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
  const PipeApprovalCard({
    super.key,
    required this.pipe,
    this.inline = false,
    this.ask,
  });

  final Pipe pipe;

  /// Just the card, where something else places it (Needs you); else
  /// centred in the pipe's view.
  final bool inline;

  /// An agent's card asking the person to approve [pipe] (#471 G1): the
  /// card starts from what the agent proposes, not the pipe's settings,
  /// and approves through that card. Null: the pipe's own approval.
  final PipeApprovalAsk? ask;

  @override
  State<PipeApprovalCard> createState() => _PipeApprovalCardState();
}

/// An agent's card asking the person to approve a pipe (AskPerson's
/// pipe_approval, #471 G1), as [PipeApprovalCard] shows it: what the
/// agent proposes, each choice the person's to change, and the ask card's
/// own answers, Dismiss and a reply (its comment field, [comment]).
/// How wide an approval card's action line must be for its words to sit
/// beside its buttons; narrower, they go above them.
const _approvalBesideAt = 440.0;

class PipeApprovalAsk {
  const PipeApprovalAsk({
    required this.cardId,
    required this.agent,
    required this.proposal,
    required this.comment,
    required this.onDismiss,
    required this.onReply,
    this.busy = false,
    this.error,
    this.updated = false,
  });

  /// Its agent changed what it proposes since it first asked
  /// (AskPerson.updated_ms).
  final bool updated;

  /// AskPerson.id, sent with Approve (ApprovePipe.card_id).
  final String cardId;

  /// The agent that asks, by name ("Rivet").
  final String agent;

  final PipeApprovalProposal proposal;

  /// The ask card's comment: with Approve, the lead's note
  /// (ApprovePipe.comment); alone, a reply to [agent]; with Dismiss, the
  /// person's comment to it.
  final TextEditingController comment;

  final VoidCallback onDismiss;
  final VoidCallback onReply;

  /// The ask card is sending a Dismiss or a reply, or why it couldn't.
  final bool busy;
  final String? error;
}

/// An agent's card asking to approve a pipe (#471 G1), where the card's
/// answers go: the pipe's approval card started from what the agent
/// proposes, while the pipe waits for approval; else why it can't be
/// approved, with only Dismiss (Claktory also takes the card back).
class PipeApprovalAskView extends StatelessWidget {
  const PipeApprovalAskView({
    super.key,
    required this.pipeId,
    required this.ask,
  });

  final String pipeId;
  final PipeApprovalAsk ask;

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    // The pipe as it is now, followed as it changes.
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        Pipe? pipe;
        for (final each in store.workspace.pipes) {
          if (each.id == pipeId) pipe = each;
        }
        return _build(context, pipe);
      },
    );
  }

  Widget _build(BuildContext context, Pipe? pipe) {
    final palette = Palette.of(context);
    if (pipe != null && pipe.state == PipeState.PIPE_STATE_APPROVAL) {
      return PipeApprovalCard(
        key: ValueKey(('pipe-approval', ask.cardId)),
        pipe: pipe,
        inline: true,
        ask: ask,
      );
    }
    return Row(
      children: [
        Expanded(
          child: Text(
            ask.error ??
                (pipe == null
                    ? 'That pipe is gone, so there is nothing to approve.'
                    : "That pipe isn't waiting for your approval any more."),
            style: TextStyles.caption.copyWith(
              color: ask.error != null ? palette.danger : palette.textMuted,
            ),
          ),
        ),
        const SizedBox(width: Space.s),
        Button(label: 'Dismiss', onPressed: ask.busy ? null : ask.onDismiss),
      ],
    );
  }
}

/// An agent's card asking the person to accept a change in who's at a
/// pipe's other end (AskPerson's pipe_trust, #471 G7), as
/// [PipeTrustAskView] shows it, with the ask card's own Dismiss and reply.
class PipeTrustAsk {
  const PipeTrustAsk({
    required this.cardId,
    required this.agent,
    required this.change,
    required this.comment,
    required this.onDismiss,
    required this.onReply,
    this.reason,
    this.busy = false,
    this.error,
  });

  /// AskPerson.id, sent with Trust or OK (AcceptPipeIdentity.card_id).
  final String cardId;

  /// The agent that asks, by name ("Rivet").
  final String agent;

  /// The change it was asked for.
  final PipeIdentityChange change;

  /// The ask card's comment: alone, a reply to [agent]; with Dismiss, the
  /// person's comment to it.
  final TextEditingController comment;

  final VoidCallback onDismiss;
  final VoidCallback onReply;

  /// The agent's reason, as the ask card shows it: under Claktory's
  /// warning, never above it (Fugo).
  final Widget? reason;

  /// The ask card is sending a Dismiss or a reply, or why it couldn't.
  final bool busy;
  final String? error;
}

/// Where an agent's card asking to accept a change at a pipe's other end
/// (#471 G7) is answered: who it was and who it is now, from the pipe as
/// Claktory has it (never from the card), Claktory's warning to check
/// with them yourself, the agent's reason under it, then Dismiss and Trust
/// (or OK, for a lapse or a new email). A pipe gone, or whose change
/// isn't this one any more, shows why, with only Dismiss.
class PipeTrustAskView extends StatefulWidget {
  const PipeTrustAskView({super.key, required this.pipeId, required this.ask});

  final String pipeId;
  final PipeTrustAsk ask;

  @override
  State<PipeTrustAskView> createState() => _PipeTrustAskViewState();
}

class _PipeTrustAskViewState extends State<PipeTrustAskView> {
  var _busy = false;
  String? _error;

  Future<void> _accept() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context)
          .acceptPipeIdentity(widget.pipeId, cardId: widget.ask.cardId);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    // The pipe as it is now, followed as it changes.
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        Pipe? pipe;
        for (final each in store.workspace.pipes) {
          if (each.id == widget.pipeId) pipe = each;
        }
        return _build(context, pipe);
      },
    );
  }

  Widget _build(BuildContext context, Pipe? pipe) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final ask = widget.ask;
    final busy = _busy || ask.busy;
    final error = _error ?? ask.error;
    if (pipe == null ||
        pipe.state == PipeState.PIPE_STATE_CLOSED ||
        pipe.peer.identityChange != ask.change) {
      return ActionLine(
        lead: Text(
          error ??
              (pipe == null
                  ? 'That pipe is gone, so there is nothing to accept.'
                  : pipe.state == PipeState.PIPE_STATE_CLOSED
                  ? 'That pipe is closed, so there is nothing to accept.'
                  : "That change isn't waiting for you any more."),
          style: type.caption.copyWith(
            color: error != null ? palette.danger : palette.textMuted,
          ),
        ),
        action: Button(
          label: 'Dismiss',
          onPressed: busy ? null : ask.onDismiss,
        ),
      );
    }
    final (title, body, danger) = pipeIdentityNoticeText(pipe);
    final name = pipePeerName(pipe);
    final account =
        ask.change == PipeIdentityChange.PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED;
    final now = pipe.peer.verifiedEmail;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          padding: const EdgeInsets.all(Space.m),
          decoration: BoxDecoration(
            color: danger
                ? palette.danger.withValues(alpha: 0.06)
                : palette.canvas,
            border: Border.all(
              color: danger
                  ? palette.danger.withValues(alpha: 0.4)
                  : palette.hairline,
            ),
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            spacing: Space.xs,
            children: [
              Row(
                children: [
                  const PortalGlyph(size: 14),
                  const SizedBox(width: Space.xs),
                  Flexible(
                    child: Text(
                      proseCodes(pipe.name),
                      style: type.caption.copyWith(
                        color: pipeColor,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                  const SizedBox(width: Space.s),
                  PipeVerifiedMark(pipe: pipe),
                ],
              ),
              Text(
                title,
                style: type.body.copyWith(
                  color: danger ? palette.danger : palette.text,
                  fontWeight: FontWeight.w600,
                ),
              ),
              if (account && now.isNotEmpty)
                Text(
                  'Their Claktory now signs in with a different account.',
                  style: type.body.copyWith(color: palette.text),
                ),
              // Claktory's warning, never the agent's word (Fugo).
              Text(
                'Check with $name yourself that this change is theirs.',
                style: type.body.copyWith(
                  color: palette.text,
                  fontWeight: FontWeight.w600,
                ),
              ),
              Text(
                account
                    ? [
                        'Until you do, their messages wait for you in Needs '
                            "you, and your agents can't send here. Trusting "
                            'it lets them through, and your agents can send '
                            'here again.',
                        if (now.isEmpty)
                          'The pipe then shows Not verified, since nothing '
                              'confirms whose Claktory it is now.',
                      ].join(' ')
                    : body,
                style: type.body.copyWith(color: palette.textMuted),
              ),
            ],
          ),
        ),
        // The agent's reason, under Claktory's warning (Fugo).
        if (ask.reason case final reason?) ...[
          const SizedBox(height: Space.s),
          reason,
        ],
        const SizedBox(height: Space.m),
        // The field, open from the start, above the row (Lucy's rules 2
        // and 4).
        CardAnswerBlock(
          field: (focus) => DecisionComment(
            controller: ask.comment,
            focusNode: focus,
            agent: ask.agent,
            enabled: !busy,
            onSubmit: ask.onReply,
          ),
          row: CommentTyped(
            controller: ask.comment,
            builder: (context, typed) => CardRow(
              line: CardLine(
                error ??
                    (typed
                        ? insteadLine(
                            account ? 'trusting' : 'keeping the pipe as it is',
                          )
                        : account
                        ? 'Trust it only once $name told you about the change.'
                        : 'OK keeps the pipe as it is.'),
                color: error != null ? palette.danger : null,
              ),
              quiet: [
                if (!typed)
                  Button(
                    label: 'Dismiss',
                    onPressed: busy ? null : ask.onDismiss,
                  ),
              ],
              primary: typed
                  ? SubmitButton(onPressed: busy ? null : ask.onReply)
                  : Button(
                      label: account ? 'Trust' : 'OK',
                      primary: true,
                      onPressed: busy ? null : _accept,
                    ),
            ),
          ),
        ),
      ],
    );
  }
}

/// The lead choice that makes a new liaison agent.
const _liaison = '\u0000liaison';

class _PipeApprovalCardState extends State<PipeApprovalCard>
    with PublishesAnswer {
  final _members = <String>{};

  /// Each member's "Can run commands on this computer", once the person
  /// turned it; else [_canRunByDefault].
  final _canRun = <String, bool>{};

  /// The members that only send to it (#414): kept as the pipe (or the
  /// agent's card) has them, shown as such, never the lead.
  final _sendOnly = <String>{};

  /// The lead the person chose; kept while it's unticked, so ticking it
  /// back brings it back. [_shownLead] is the one that counts.
  var _lead = _liaison;

  /// The lead as the form has it: the one chosen while it's a member, else
  /// the new liaison agent.
  String get _shownLead =>
      _lead == _liaison ||
          _members.contains(_lead) && !_sendOnly.contains(_lead)
      ? _lead
      : _liaison;

  /// Prefilled with the name Claktory suggests (a first name no agent of
  /// the project has).
  late final _liaisonName = TextEditingController(
    text: liaisonNameFor(widget.pipe),
  );

  var _reviewIncoming = false;
  var _reviewOutgoing = false;
  var _busy = false;
  String? _error;

  /// Whether the person changed the card: until then it shows the pipe's
  /// settings as they are, set before approval (Rivet's settings card,
  /// #470) or none yet, and follows them as they change.
  var _touched = false;

  @override
  void initState() {
    super.initState();
    if (widget.ask case final ask?) {
      _readProposal(ask.proposal);
      // Updated before this card was built (Needs you shows it afresh):
      // said too, until the person changes the card.
      _proposalChanged = ask.updated;
    } else {
      _read(widget.pipe);
    }
    _comment.addListener(_typing);
  }

  /// A comment typed or cleared turns the dock's Submit on or off.
  var _typed = false;
  void _typing() {
    final typed = _comment.text.trim().isNotEmpty;
    if (typed != _typed && mounted) setState(() => _typed = typed);
  }

  /// For focus mode's keys (#514), the pipe's own approval (an agent's card
  /// asking for it answers as that card): Decline, Submit once a comment's
  /// typed, and Approve, never from a key while there are digits to
  /// compare (FS14, Fugo's rule C): then only the card's own button.
  @override
  CardAnswer? answerNow() {
    final pipe = widget.pipe;
    if (widget.ask != null || pipe.state != PipeState.PIPE_STATE_APPROVAL) {
      return null;
    }
    final replyTo = pipe.replyAgentId.isEmpty
        ? null
        : AppScope.of(context).workspace.agent(pipe.replyAgentId)?.name ??
              'Your agent';
    return CardAnswer(
      key: 'pipe:${pipe.id}/approval',
      docked: false,
      yes: 'Approve',
      no: 'Decline',
      typed: _typed && replyTo != null,
      busy: _busy,
      error: _error,
      onYes: _busy || pipe.digits.isNotEmpty
          ? null
          : () => _decide(true, rethrowing: true),
      onSubmit: _busy || replyTo == null
          ? null
          : () => _reply(replyTo, rethrowing: true),
      onNo: _busy
          ? null
          : (reason) => _decide(false, reason: reason, rethrowing: true),
    );
  }

  @override
  void didUpdateWidget(PipeApprovalCard old) {
    super.didUpdateWidget(old);
    if (widget.ask case final ask?) {
      // The agent changed what it proposes (a second card for this pipe,
      // in place): read again, and said, so the person checks it again.
      if (ask.proposal != old.ask?.proposal) {
        _proposalChanged = true;
        _touched = false;
        _readProposal(ask.proposal);
      }
      return;
    }
    if (_settingsOf(widget.pipe) == _readFrom) return;
    // Changed under the person's own changes (Fugo): read again, never
    // approved over, and said, so they check it again.
    if (_touched) _changed = true;
    _touched = false;
    _read(widget.pipe);
  }

  /// The settings the card was last read from ([_settingsOf]).
  var _readFrom = '';

  /// The pipe's settings changed after the person changed the card: it
  /// read them again, and says so until they change it again.
  var _changed = false;

  /// An agent's card: what it proposes changed since the card was first
  /// shown, or before (its agent updated the card, in a Needs you built
  /// afresh); said until the person changes the card.
  var _proposalChanged = false;

  /// An agent's card: the pipe's settings when it was asked (or its
  /// agent last changed it), from Claktory, to tell what narrowed since.
  /// Null when it doesn't say.
  _PipeSettings? _askedFrom;

  /// What of [pipe] the card shows, to tell when it changes.
  static String _settingsOf(Pipe pipe) => _PipeSettings.of(pipe).key;

  /// The card as [pipe] is set: its members (those that only send to it
  /// too, kept as they are), who of them can run commands, its lead (else
  /// a new liaison) and its reviews, which start off on a new pipe.
  void _read(Pipe pipe) {
    _readFrom = _settingsOf(pipe);
    _members
      ..clear()
      ..addAll(pipe.memberIds);
    _sendOnly
      ..clear()
      ..addAll(pipe.sendOnlyIds.where(_members.contains));
    _canRun.clear();
    for (final id in pipe.memberIds) {
      _canRun[id] = !pipe.quarantinedIds.contains(id);
    }
    _lead = pipe.leadId.isEmpty ? _liaison : pipe.leadId;
    _reviewIncoming = pipe.reviewIncoming;
    _reviewOutgoing = pipe.reviewOutgoing;
  }

  /// The card as an agent proposes it (#471 G1): its members, who of them
  /// can run commands (the others limited to the pipe), its lead or a new
  /// liaison of the name it gives, the reviews and who only sends.
  void _readProposal(PipeApprovalProposal proposal) {
    _askedFrom = proposal.hasAskedFrom()
        ? _PipeSettings.asked(proposal.askedFrom)
        : null;
    _members
      ..clear()
      ..addAll(proposal.memberIds);
    _sendOnly
      ..clear()
      ..addAll(proposal.sendOnlyIds.where(_members.contains));
    _canRun.clear();
    for (final id in proposal.memberIds) {
      _canRun[id] = proposal.canRunCommandsIds.contains(id);
    }
    _lead = proposal.leadId.isEmpty ? _liaison : proposal.leadId;
    if (proposal.liaisonName.trim().isNotEmpty) {
      _liaisonName.text = proposal.liaisonName.trim();
    }
    _reviewIncoming = proposal.reviewIncoming;
    _reviewOutgoing = proposal.reviewOutgoing;
  }

  /// An agent's card: the choices wider than the pipe is now where the
  /// pipe narrowed since the card was asked (Fugo): a member taken out,
  /// limited to the pipe or made to only send, a review turned on.
  Set<String> _wider() {
    final then = _askedFrom;
    if (widget.ask == null || then == null) return const {};
    final now = _PipeSettings.of(widget.pipe);
    return {
      for (final id in _members) ...[
        if (then.members.contains(id) && !now.members.contains(id))
          'removed:$id',
        if (!then.quarantined.contains(id) &&
            now.quarantined.contains(id) &&
            !_sendOnly.contains(id) &&
            _canRun[id] == true)
          'commands:$id',
        if (!then.sendOnly.contains(id) &&
            now.sendOnly.contains(id) &&
            !_sendOnly.contains(id))
          'sendOnly:$id',
      ],
      if (!then.reviewIncoming && now.reviewIncoming && !_reviewIncoming)
        'reviewIncoming',
      if (!then.reviewOutgoing && now.reviewOutgoing && !_reviewOutgoing)
        'reviewOutgoing',
    };
  }

  void _change(VoidCallback change) => setState(() {
    _touched = true;
    _changed = false;
    _proposalChanged = false;
    change();
  });

  /// Who the last reply went to (#370), said until the next decision.
  String? _sentTo;

  /// The person's comment, for this side's lead with either answer; never
  /// the other side's.
  final _comment = TextEditingController();

  /// Where the comment is typed: an agent's card's own field
  /// ([PipeApprovalAsk.comment]), else the card's.
  TextEditingController get _commentField => widget.ask?.comment ?? _comment;

  @override
  void dispose() {
    _liaisonName.dispose();
    _comment.dispose();
    super.dispose();
  }

  /// [reason]: the no's, from focus mode's dock, with the comment;
  /// [rethrowing] for its keys: refused, it throws too, and stopped here
  /// (no liaison's name), [AnswerNotSent].
  Future<void> _decide(
    bool approve, {
    String? reason,
    bool rethrowing = false,
  }) async {
    final lead = _shownLead;
    final liaison = lead == _liaison;
    if (approve && liaison && _liaisonName.text.trim().isEmpty) {
      setState(() => _error = 'Name the liaison agent.');
      if (rethrowing) throw const AnswerNotSent();
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
          // Kept as the pipe (or the agent's card) has them.
          sendOnly: approve
              ? PipeMembers(
                  agentIds: [
                    for (final id in _sendOnly)
                      if (_members.contains(id)) id,
                  ],
                )
              : null,
          leadId: approve && !liaison ? lead : '',
          liaisonName: approve && liaison ? _liaisonName.text.trim() : '',
          quarantined: approve
              ? PipeMembers(
                  agentIds: [
                    for (final agent in AppScope.of(
                      context,
                    ).workspace.agentsIn(widget.pipe.projectId))
                      if (_members.contains(agent.id) &&
                          !_sendOnly.contains(agent.id) &&
                          !_canRunOf(agent))
                        agent.id,
                  ],
                )
              : null,
          reviewIncoming: approve ? _reviewIncoming : null,
          reviewOutgoing: approve ? _reviewOutgoing : null,
          comment: noComment(reason, _commentField.text),
          // From an agent's card (#471 G1): approved through it.
          cardId: widget.ask?.cardId,
        ),
      );
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
      if (rethrowing) rethrow;
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// The comment to [agent] (Pipe.reply_agent_id), as a reply: nothing
  /// approved or declined, the pipe still waiting (#370).
  Future<void> _reply(String agent, {bool rethrowing = false}) async {
    setState(() {
      _busy = true;
      _error = null;
      _sentTo = null;
    });
    try {
      await AppScope.of(context).approvePipe(
        ApprovePipe(
          pipeId: widget.pipe.id,
          approve: false,
          reply: true,
          comment: _comment.text.trim(),
        ),
      );
      if (mounted) {
        _comment.clear();
        setState(() => _sentTo = agent);
      }
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
      if (rethrowing) rethrow;
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
    publishAnswer();
    final palette = Palette.of(context);
    final pipe = widget.pipe;
    final peer = pipe.peer;
    final store = AppScope.of(context).workspace;
    final project = store.project(pipe.projectId);
    final agents = store.agentsIn(pipe.projectId);
    final lead = _shownLead;
    final ask = widget.ask;
    // Who a comment goes to as a reply; none, no Submit.
    final replyTo = ask != null
        ? ask.agent
        : pipe.replyAgentId.isEmpty
        ? null
        : store.agent(pipe.replyAgentId)?.name ?? 'Your agent';
    // An agent's card: the pipe changed since it asked, and what of the
    // card is wider than the pipe now (Fugo).
    final askedFrom = _askedFrom;
    final pipeChanged =
        ask != null &&
        askedFrom != null &&
        askedFrom.key != _PipeSettings.of(pipe).key;
    final wider = _wider();
    final busy = _busy || (ask?.busy ?? false);
    final error = _error ?? ask?.error;
    // Those of another project that only send to it are on the card too.
    final inProject = {for (final agent in agents) agent.id};
    final outside = [
      for (final id in _sendOnly)
        if (!inProject.contains(id)) ?store.agent(id),
    ];
    // Their words, on a card to decide on: what reorders text shows by
    // its code (proseCodes, Fugo).
    final who = [
      if (peer.ownerName.isNotEmpty) proseCodes(peer.ownerName),
      if (peer.ownerEmail.isNotEmpty) proseCodes(peer.ownerEmail),
      if (peer.claktoryName.isNotEmpty) proseCodes(peer.claktoryName),
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
          // Who, and the Verified mark beside it while both fit; on a
          // narrow card (a phone's Needs you) the mark goes under, so the
          // words keep their line (Zucchero's row 535).
          Wrap(
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: Space.s,
            runSpacing: Space.xs,
            children: [
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const PortalGlyph(size: 20),
                  const SizedBox(width: Space.xs + 2),
                  Flexible(
                    child: Text(
                      '${pipePeerName(pipe, start: true)} wants to connect',
                      style: TextStyles.caption.copyWith(
                        color: pipeColor,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                ],
              ),
              PipeVerifiedMark(pipe: pipe),
            ],
          ),
          const SizedBox(height: Space.m),
          Text(
            '${peer.projectName.isEmpty ? 'Their project' : proseCodes(peer.projectName)} '
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
              '“${proseCodes(peer.purpose)}”',
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
              ask != null
                  // Never on an agent's word (Fugo, #471).
                  ? 'Compare these with ${pipePeerName(pipe)} yourself, by '
                        "voice or in person. Don't approve because an agent "
                        'says they match.'
                  : 'Approve only if ${pipePeerName(pipe)} sees the same '
                        'digits.',
              textAlign: TextAlign.center,
              style: ask != null
                  ? TextStyles.caption.copyWith(color: palette.text)
                  : caption,
            ),
          ],
          const SizedBox(height: Space.l),
          Text(
            'Who may use it',
            style: TextStyles.label.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.xs),
          for (final agent in [...agents, ...outside])
            Padding(
              padding: const EdgeInsets.only(top: Space.xs),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _Narrowed(
                    note: wider.contains('removed:${agent.id}')
                        ? 'Since ${ask?.agent} asked, ${agent.name} was taken out '
                              'of the pipe.'
                        : wider.contains('sendOnly:${agent.id}')
                        ? 'Since ${ask?.agent} asked, ${agent.name} was made to '
                              'only send to the pipe.'
                        : null,
                    child: _MemberLine(
                      agent: agent,
                      value: _members.contains(agent.id),
                      held: pipeHeldReason(pipe, agent.id),
                      onChanged: (on) => _change(() {
                        if (on) {
                          _members.add(agent.id);
                        } else {
                          _members.remove(agent.id);
                        }
                      }),
                    ),
                  ),
                  if (_members.contains(agent.id))
                    if (_sendOnly.contains(agent.id))
                      _SendOnlyLine(agent: agent, pipe: pipe)
                    else
                      _Narrowed(
                        note: wider.contains('commands:${agent.id}')
                            ? 'Since ${ask?.agent} asked, ${agent.name} was '
                                  'limited to the pipe.'
                            : null,
                        child: _CommandsLine(
                          agent: agent,
                          pipe: pipe,
                          value: _canRunOf(agent),
                          onChanged: (on) =>
                              _change(() => _canRun[agent.id] = on),
                        ),
                      ),
                ],
              ),
            ),
          const SizedBox(height: Space.m),
          Field(
            label: 'Lead',
            note: lead == _liaison
                ? liaisonNote
                : 'Told of each message that comes in. The others '
                      'read the pipe when they choose.',
            child: Select<String>(
              value: lead,
              options: [
                const SelectOption(_liaison, 'A new liaison agent'),
                for (final agent in agents)
                  if (_members.contains(agent.id) &&
                      !_sendOnly.contains(agent.id))
                    SelectOption(agent.id, agent.name),
              ],
              onChanged: (lead) => _change(() => _lead = lead),
            ),
          ),
          if (lead == _liaison)
            Field(
              label: "Liaison's name",
              child: TextField(controller: _liaisonName),
            ),
          _Narrowed(
            note: wider.contains('reviewIncoming')
                ? 'Since ${ask?.agent} asked, Review incoming was turned on in '
                      'the pipe.'
                : null,
            child: _ReviewIncomingField(
              value: _reviewIncoming,
              onChanged: (on) => _change(() => _reviewIncoming = on),
            ),
          ),
          _Narrowed(
            note: wider.contains('reviewOutgoing')
                ? 'Since ${ask?.agent} asked, Review outgoing was turned on in '
                      'the pipe.'
                : null,
            child: _ReviewOutgoingField(
              value: _reviewOutgoing,
              onChanged: (on) => _change(() => _reviewOutgoing = on),
            ),
          ),
          // Its reports already go somewhere, from the project's default.
          if (forwardApprovalLine(
                store.workspace,
                pipe,
                liaison: lead == _liaison ? _liaisonName.text : '',
              )
              case final line?)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.m),
              child: Text(
                line,
                style: TextStyles.body.copyWith(color: palette.textMuted),
              ),
            ),
          // The field first, open from the start (Lucy's rule 4).
          // Approved, the lead chosen here hears it; declined, the agent
          // that set the pipe up (or Rivet): only this side's agents.
          DecisionComment(
            controller: _commentField,
            agent: 'your agents',
            note: 'Only your agents see it, not ${pipePeerClaktory(pipe)}.',
            enabled: !busy,
            onSubmit: replyTo == null
                ? null
                : ask != null
                ? ask.onReply
                : () => _reply(replyTo),
          ),
          const SizedBox(height: Space.s),
          // A comment typed, with someone to hear it: one Submit, the
          // pipe still waiting (#370).
          CommentTyped(
            controller: _commentField,
            builder: (context, typed) {
              final replying = typed && replyTo != null;
              // Said until the person changes the card; the pipe's own
              // change, whenever it differs from when the agent asked.
              final check = pipeChanged
                  ? "The pipe's settings changed since ${ask.agent} asked. "
                        'Check them again.'
                  : _proposalChanged
                  ? '${ask?.agent} changed what it proposes. Check it again.'
                  : _changed
                  ? "The pipe's settings changed since you started. Check "
                        'them again.'
                  : null;
              final words = Text(
                error ??
                    (replying
                        ? insteadLine('approving')
                        : check ??
                              (_sentTo != null
                                  ? 'Sent to $_sentTo.'
                                  : 'You can change this later in the '
                                        "pipe's settings.")),
                style: TextStyles.caption.copyWith(
                  color: error != null
                      ? palette.danger
                      : !replying && check != null
                      ? palette.text
                      : palette.textFaint,
                ),
              );
              // On the narrowest cards the two wrap, rather than spill.
              final buttons = Wrap(
                alignment: WrapAlignment.end,
                spacing: Space.s,
                runSpacing: Space.xs,
                children: [
                  if (replying)
                    SubmitButton(
                      onPressed: busy
                          ? null
                          : ask != null
                          ? ask.onReply
                          : () => _reply(replyTo),
                    )
                  else ...[
                    if (ask != null)
                      // The card's own: its agent is told, the pipe still
                      // waits.
                      Button(
                        label: 'Dismiss',
                        onPressed: busy ? null : ask.onDismiss,
                      )
                    else
                      Button(
                        label: 'Decline',
                        onPressed: busy ? null : () => _decide(false),
                      ),
                    Button(
                      label: 'Approve',
                      primary: true,
                      onPressed: busy ? null : () => _decide(true),
                    ),
                  ],
                ],
              );
              // Words beside the buttons while they keep a line's worth of
              // room; narrower, above them, whole (Zucchero's row 535).
              return LayoutBuilder(
                builder: (context, box) {
                  final beside = box.maxWidth >= _approvalBesideAt;
                  if (beside) {
                    return Row(
                      children: [
                        Expanded(child: words),
                        const SizedBox(width: Space.s),
                        buttons,
                      ],
                    );
                  }
                  return Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      words,
                      const SizedBox(height: Space.s),
                      Align(
                        alignment: AlignmentDirectional.centerEnd,
                        child: buttons,
                      ),
                    ],
                  );
                },
              );
            },
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

/// A pipe's own settings, the ones its approval card shows: whom it has,
/// who of them is limited to it or only sends, its lead and its reviews.
class _PipeSettings {
  _PipeSettings.of(Pipe pipe)
    : members = {...pipe.memberIds},
      quarantined = {...pipe.quarantinedIds},
      sendOnly = {...pipe.sendOnlyIds},
      lead = pipe.leadId,
      reviewIncoming = pipe.reviewIncoming,
      reviewOutgoing = pipe.reviewOutgoing;

  _PipeSettings.asked(PipeApprovalBaseline asked)
    : members = {...asked.memberIds},
      quarantined = {...asked.quarantinedIds},
      sendOnly = {...asked.sendOnlyIds},
      lead = asked.leadId,
      reviewIncoming = asked.reviewIncoming,
      reviewOutgoing = asked.reviewOutgoing;

  final Set<String> members;
  final Set<String> quarantined;
  final Set<String> sendOnly;
  final String lead;
  final bool reviewIncoming;
  final bool reviewOutgoing;

  /// All of it in one string, to tell when it changes.
  String get key => [
    ([...members]..sort()).join(','),
    ([...quarantined]..sort()).join(','),
    ([...sendOnly]..sort()).join(','),
    lead,
    reviewIncoming,
    reviewOutgoing,
  ].join('|');
}

/// What [EntryReveal.reachExtra] names to open a pipe with no liaison at
/// one of its messages (pipeItemKey(seq)).
String pipeChatRevealId(String pipeId) => 'pipe:$pipeId';

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

  /// Find in the pipe's messages (⌘F; a phone's magnifier): Claktory
  /// searches all of them.
  late final _find = ConversationFind(
    search: (query, {required pipe, beforeSeq}) => AppScope.of(context)
        .searchConversation(
          pipeId: widget.pipe.id,
          query: query,
          beforeSeq: beforeSeq,
        ),
    pipe: true,
    conversation: false,
    go: (at, _, _) => _findGo(at.seq),
  );

  /// Each message's place, to go to one (from Needs you's History).
  final _keys = <int, GlobalKey>{};
  EntryReveal? _reveal;
  PipeChat? _chat;

  /// Steps taken towards the message asked for, so a search that can't
  /// find it ends.
  var _steps = 0;

  @override
  void initState() {
    super.initState();
    _scroll.addListener(_maybeLoadEarlier);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (backend.reveal != _reveal) {
      _reveal?.removeListener(_revealSoon);
      _reveal = backend.reveal..addListener(_revealSoon);
    }
    if (backend.pipeChat != _chat) {
      _chat?.removeListener(_revealSoon);
      _chat = backend.pipeChat..addListener(_revealSoon);
    }
    _revealSoon();
  }

  @override
  void dispose() {
    _reveal?.removeListener(_revealSoon);
    _chat?.removeListener(_revealSoon);
    _scroll.dispose();
    _find.dispose();
    super.dispose();
  }

  /// Brings a find's match, message [seq], into sight: where it's built,
  /// else loading back to it if it's older than what's held, then up the
  /// list until it is.
  void _findGo(int seq, [int steps = 0]) {
    if (!mounted || !_scroll.hasClients) return;
    final place = _keys[seq]?.currentContext;
    if (place != null) {
      final box = place.findRenderObject();
      final view = Scrollable.maybeOf(place)?.context.findRenderObject();
      if (box is RenderBox && view is RenderBox) {
        final top = box.localToGlobal(Offset.zero, ancestor: view).dy;
        // Clear of the find bar, with its first lines well above the foot.
        if (top >= Space.xxl && top <= view.size.height * 0.7) return;
      }
      Scrollable.ensureVisible(place, alignment: 0.5);
      return;
    }
    final chat = AppScope.of(context).pipeChat;
    final held = chat.pipeId == widget.pipe.id && chat.messages.isNotEmpty;
    if (!held || steps > 60) return;
    if (seq < chat.messages.first.seq.toInt()) {
      unawaited(() async {
        try {
          await AppScope.of(context).loadPipeFrom(widget.pipe.id, seq);
        } on RequestFailure {
          return;
        }
        WidgetsBinding.instance.addPostFrameCallback(
          (_) => _findGo(seq, steps + 1),
        );
      }());
      return;
    }
    // Held but not built: further up (reversed: towards the far end).
    final position = _scroll.position;
    position.jumpTo(
      (position.pixels + position.viewportDimension * 0.8).clamp(
        0,
        position.maxScrollExtent,
      ),
    );
    WidgetsBinding.instance.addPostFrameCallback(
      (_) => _findGo(seq, steps + 1),
    );
  }

  void _revealSoon() {
    if (_reveal?.extra?.agentId != pipeChatRevealId(widget.pipe.id)) return;
    WidgetsBinding.instance.addPostFrameCallback((_) => _revealMessage());
  }

  /// Goes to the message [EntryReveal.extra] names, once it's this pipe's:
  /// further up while it's above what's built (loading earlier pages as
  /// it goes), then puts it in the middle of the view.
  void _revealMessage() {
    final reveal = _reveal;
    final target = reveal?.extra;
    if (reveal == null ||
        target == null ||
        !mounted ||
        target.agentId != pipeChatRevealId(widget.pipe.id)) {
      return;
    }
    final chat = AppScope.of(context).pipeChat;
    // Not loaded yet: looked at again when it is.
    if (chat.pipeId != widget.pipe.id || !_scroll.hasClients) return;
    final seq = target.key is String
        ? int.tryParse((target.key as String).replaceFirst('pipe:', ''))
        : null;
    final messages = chat.messages;
    final here = messages.any((message) => message.seq.toInt() == seq);
    if (seq == null || (!here && !chat.hasEarlier) || _steps > 60) {
      // Gone, or not to be found: the pipe stays at its end.
      reveal.usedExtra(target);
      _steps = 0;
      return;
    }
    final place = _keys[seq]?.currentContext;
    if (place != null) {
      reveal.usedExtra(target);
      _steps = 0;
      Scrollable.ensureVisible(place, alignment: 0.5);
      return;
    }
    // Further up (reversed: towards the far end), which also loads the
    // earlier page once near the top; looked at again next frame.
    _steps++;
    final position = _scroll.position;
    position.jumpTo(
      (position.pixels + position.viewportDimension * 0.8).clamp(
        0,
        position.maxScrollExtent,
      ),
    );
    WidgetsBinding.instance.addPostFrameCallback((_) => _revealMessage());
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
    return FindScope(target: _find, child: _messages(palette, chat));
  }

  Widget _messages(Palette palette, PipeChat chat) {
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
                widget.pipe.state != PipeState.PIPE_STATE_OPEN
                    ? ''
                    : widget.pipe.leadId.isEmpty
                    // No lead: no agent hears what comes in (Abbacchio's
                    // row 638).
                    ? 'Nothing here yet. No agent hears what comes in until '
                          'you choose a lead in the pipe’s settings. You can '
                          'write here too.'
                    : 'Nothing here yet. Your lead hears each message that '
                          'comes in, and you can write here too.',
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
                    child: KeyedSubtree(
                      key: _keys.putIfAbsent(
                        message.seq.toInt(),
                        GlobalKey.new,
                      ),
                      // A find's matches marked, the match gone to fully.
                      child: ValueListenableBuilder(
                        valueListenable: _find.marks,
                        builder: (context, marks, child) => FindHighlight(
                          query: marks.query,
                          current: marks.at?.seq == message.seq.toInt(),
                          child: child!,
                        ),
                        child: PipeMessageView(
                          key: ValueKey(message.seq),
                          pipe: widget.pipe,
                          message: message,
                        ),
                      ),
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
  const PipeMessageView({
    super.key,
    required this.pipe,
    required this.message,
    this.framed = false,
  });

  final Pipe pipe;
  final PipeMessage message;

  /// As a card among a liaison's conversation ("From Ana's Claktory"),
  /// rather than a bubble in the pipe's own chat.
  final bool framed;

  @override
  State<PipeMessageView> createState() => _PipeMessageViewState();
}

class _PipeMessageViewState extends State<PipeMessageView>
    with PublishesAnswer {
  var _busy = false;
  String? _error;

  /// The person's comment with their decision, for this side's agent only.
  late final _comment = TextEditingController()..addListener(_typing);

  /// A comment typed or cleared turns the dock's Submit on or off.
  var _typed = false;
  void _typing() {
    final typed = _comment.text.trim().isNotEmpty;
    if (typed != _typed && mounted) setState(() => _typed = typed);
  }

  /// For focus mode's keys (#514), a held message: Send (Let in), Discard,
  /// and Submit once a comment's typed on an agent's, as its buttons.
  @override
  CardAnswer? answerNow() {
    final message = widget.message;
    final held = message.state == PipeMessageState.PIPE_MESSAGE_STATE_HELD;
    if (!held &&
        message.state != PipeMessageState.PIPE_MESSAGE_STATE_HELD_INCOMING) {
      return null;
    }
    final replies =
        held &&
        pipeDecisionHearer(
              AppScope.of(context).workspace.workspace,
              widget.pipe,
              message,
            ) !=
            null;
    return CardAnswer(
      key: 'pipe:${widget.pipe.id}/${message.seq}',
      docked: false,
      yes: held ? 'Send' : 'Let in',
      no: 'Discard',
      typed: replies && _typed,
      busy: _busy,
      error: _error,
      onYes: _busy ? null : () => _decide(true, rethrowing: true),
      onSubmit: _busy || !replies
          ? null
          : () => _decide(false, reply: true, rethrowing: true),
      onNo: _busy
          ? null
          : (reason) => _decide(false, reason: reason, rethrowing: true),
    );
  }

  @override
  void dispose() {
    _comment.dispose();
    super.dispose();
  }

  /// Sends it (lets it in), or discards it; [reply], on one of this
  /// side's agents' (#370), sends it back unsent to its writer with the
  /// comment, to write again. [reason]: the no's, from focus mode's dock,
  /// with the comment; [rethrowing] for its keys: refused, it throws too.
  Future<void> _decide(
    bool allow, {
    bool reply = false,
    String? reason,
    bool rethrowing = false,
  }) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).decidePipeMessage(
        widget.pipe.id,
        widget.message.seq.toInt(),
        allow,
        comment: noComment(reason, _comment.text),
        reply: reply,
      );
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
      if (rethrowing) rethrow;
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    publishAnswer();
    final palette = Palette.of(context);
    final message = widget.message;
    final author = message.author;
    // The other side's words, and what an agent wrote: what reorders text
    // shows by its code (proseCodes, Fugo).
    final text = proseCodes(message.text);
    final name = proseCodes(author.name);
    final role = proseCodes(author.role);
    final time = logTime(
      DateTime.fromMillisecondsSinceEpoch(message.timestampMs.toInt()),
    );
    if (author.kind == PipeAuthorKind.PIPE_AUTHOR_KIND_CLAKTORY) {
      final line = Center(
        child: Text(
          '$text · $time',
          textAlign: TextAlign.center,
          style: TextStyles.caption.copyWith(color: palette.textFaint),
        ),
      );
      // Announcing a website the other side shares (W2): Open and Copy
      // address on it, while the share lasts.
      if (message.shareId.isNotEmpty) {
        return ShareAnnouncement(shareId: message.shareId, line: line);
      }
      return line;
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
            agent?.name ?? name,
            style: caption.copyWith(color: color, fontWeight: FontWeight.w600),
          ),
        ],
      ),
      PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_PERSON => Text(
        'Says it’s from ${name.isEmpty ? 'their person' : name}',
        style: caption.copyWith(
          color: palette.textMuted,
          fontWeight: FontWeight.w600,
        ),
      ),
      _ => NameThenRole(
        name: name.isEmpty ? 'Their agent' : name,
        role: role,
        style: caption.copyWith(
          color: palette.textMuted,
          fontWeight: FontWeight.w600,
        ),
        roleColor: palette.textFaint,
      ),
    };
    if (widget.framed) {
      return _card(
        palette,
        time,
        mine: mine,
        dropped: dropped,
        who: switch (author.kind) {
          PipeAuthorKind.PIPE_AUTHOR_KIND_ME => 'You',
          PipeAuthorKind.PIPE_AUTHOR_KIND_MY_AGENT => agent?.name ?? name,
          PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_PERSON =>
            'Says it’s from ${name.isEmpty ? 'their person' : name}',
          _ => [
            name.isEmpty ? 'Their agent' : name,
            if (role.isNotEmpty) role,
          ].join(', '),
        },
      );
    }
    final bubble = Opacity(
      opacity: dropped ? 0.5 : 1,
      child: Bubble(
        color: color,
        outgoing: mine,
        // A find's matches marked in it (a pipe with no liaison's).
        child: Text.rich(
          FindHighlight.markText(
            context,
            text,
            TextStyles.body.copyWith(
              color: white ? const Color(0xFFFFFFFF) : palette.text,
              decoration: dropped ? TextDecoration.lineThrough : null,
              decorationColor: white ? const Color(0xFFFFFFFF) : palette.text,
            ),
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
                    text:
                        '${proseCodes(file.name)} · ${formatBytes(file.size)}',
                    color: palette.textMuted,
                  ),
              ],
            ),
          ),
        Padding(padding: inset.copyWith(top: 2), child: _state(palette, time)),
      ],
    );
  }

  /// The message as a card in a liaison's conversation: a frame in the
  /// pipe's color, "From Ana's Claktory" or "To Ana's Claktory" with the
  /// portal and who wrote it, the text as it is, its files, and what
  /// became of it.
  Widget _card(
    Palette palette,
    String time, {
    required bool mine,
    required bool dropped,
    required String who,
  }) {
    final message = widget.message;
    final text = proseCodes(message.text);
    final peer = pipePeerClaktory(widget.pipe);
    final caption = TextStyles.caption;
    return Container(
      padding: const EdgeInsets.fromLTRB(Space.m, Space.s, Space.m, Space.s),
      decoration: BoxDecoration(
        color: pipeColor.withValues(alpha: 0.05),
        border: Border.all(color: pipeColor.withValues(alpha: 0.45)),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const PortalGlyph(size: 14),
              const SizedBox(width: Space.xs + 2),
              Flexible(
                child: Text.rich(
                  TextSpan(
                    children: [
                      TextSpan(
                        text: mine ? 'To $peer' : 'From $peer',
                        style: const TextStyle(
                          color: pipeColor,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      if (who.isNotEmpty)
                        TextSpan(
                          text: '  $who',
                          style: TextStyle(color: palette.textMuted),
                        ),
                    ],
                  ),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: caption,
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.xs),
          Opacity(
            opacity: dropped ? 0.5 : 1,
            // A find in the conversation marks its matches here too.
            child: Text.rich(
              FindHighlight.markText(
                context,
                text,
                TextStyles.body.copyWith(
                  color: palette.text,
                  decoration: dropped ? TextDecoration.lineThrough : null,
                  decorationColor: palette.text,
                ),
              ),
            ),
          ),
          if (message.files.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: Space.xs),
              child: Wrap(
                spacing: Space.xs,
                runSpacing: Space.xs,
                children: [
                  for (final file in message.files)
                    _Mark(
                      text:
                          '${proseCodes(file.name)} · ${formatBytes(file.size)}',
                      color: palette.textMuted,
                    ),
                ],
              ),
            ),
          const SizedBox(height: Space.xs),
          _state(palette, time, start: true),
        ],
      ),
    );
  }

  /// The agent of this side that wrote it, by name.
  String _writer() {
    final author = widget.message.author;
    final agent = AppScope.of(context).workspace.agent(author.agentId);
    return proseCodes(agent?.name ?? author.name);
  }

  /// What became of it, and the person's decision when one waits; [start]
  /// lines it up at the start whichever way it went.
  Widget _state(Palette palette, String time, {bool start = false}) {
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
      final hearer = pipeDecisionHearer(
        AppScope.of(context).workspace.workspace,
        widget.pipe,
        message,
      );
      final choices = Row(
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
      );
      // An agent's held message with a comment typed: one Submit, which
      // sends it back unsent (#370). The other side's keeps Discard and
      // Let in: its lead reads it once it's in.
      final replies = held && hearer != null;
      return Column(
        crossAxisAlignment: held && !start
            ? CrossAxisAlignment.end
            : CrossAxisAlignment.start,
        children: [
          line(
            held
                ? note.isEmpty
                      ? 'Waiting for you to send it · $time'
                      : 'Waiting for you to send it · $note · $time'
                : note.isEmpty
                ? 'Over this pipe’s limits: let it in, or discard it. · $time'
                : '$note · $time',
            color: palette.textMuted,
          ),
          if (_error case final error?) line(error, color: palette.danger),
          const SizedBox(height: Space.xs),
          // The field first, open from the start (Lucy's rule 4).
          if (hearer != null)
            DecisionComment(
              controller: _comment,
              agent: hearer,
              peer: pipePeerClaktory(widget.pipe),
              enabled: !_busy,
              onSubmit: replies ? () => _decide(false, reply: true) : null,
            ),
          if (hearer != null) const SizedBox(height: Space.s),
          if (replies)
            CommentTyped(
              controller: _comment,
              builder: (context, typed) => typed
                  ? Wrap(
                      spacing: Space.s,
                      runSpacing: Space.xs,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        line(insteadLine('sending')),
                        SubmitButton(
                          onPressed: _busy
                              ? null
                              : () => _decide(false, reply: true),
                        ),
                      ],
                    )
                  : choices,
            )
          else
            choices,
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
      // Sent back to its writer with the person's comment (#370).
      PipeMessageState.PIPE_MESSAGE_STATE_DISCARDED when message.replied =>
        line(
          note.isEmpty
              ? 'Sent back to ${_writer()} · $time'
              : 'Sent back to ${_writer()} · “$note” · $time',
          color: palette.textMuted,
        ),
      // Taken back by the agent that wrote it (pipe_withdraw): Claktory's
      // "Lea took it back: …", its reason in the agent's words.
      PipeMessageState.PIPE_MESSAGE_STATE_DISCARDED when note.isNotEmpty =>
        line('${proseCodes(note)} · $time', color: palette.textMuted),
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
    final foot = pipe.state == PipeState.PIPE_STATE_OPEN
        ? _PipeComposer(pipe: pipe)
        : pipeStateBar(context, pipe) ?? const SizedBox.shrink();
    // On a phone, while its find bar is open, the bar takes the foot: the
    // composer steps aside, keeping what's typed, as a message box does.
    final finding = PhonePageFind.finding(context);
    return Offstage(
      offstage: finding,
      child: TickerMode(
        enabled: !finding,
        child: ExcludeFocus(excluding: finding, child: foot),
      ),
    );
  }
}

/// Why nothing goes through [pipe] and what the person can do (Continue,
/// Resume, Delete…), as a bar behind a top hairline; null while it's open
/// or before it is.
Widget? pipeStateBar(BuildContext context, Pipe pipe) {
  final palette = Palette.of(context);
  final who = pipePeerName(pipe, start: true);
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
      return null;
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
            answers: 'pipe:${pipe.id}/loop',
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
  Widget build(BuildContext context) {
    final pipe = widget.pipe;
    final liaison = pipeLiaison(AppScope.of(context).workspace.workspace, pipe);
    return FormFrame(
      title: 'Delete ${pipe.name}?',
      body: liaison == null
          ? 'Its messages are deleted from this Claktory. The other side '
                'keeps theirs.'
          : 'Its messages are deleted from this Claktory; the other side '
                'keeps theirs. ${liaison.name} goes with it, and its '
                'conversation is saved in the project’s folder, '
                '${pipe.folder.isEmpty ? 'under pipes' : 'in pipes/${pipe.folder}/conversation'}.',
      error: _error,
      fields: const [],
      submitLabel: 'Delete pipe',
      onSubmit: _busy ? null : _delete,
      onCancel: widget.close,
    );
  }
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
                _error ?? pipeOfflineText(pipe),
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
/// who leads, its reviews and its limits; and pausing, closing or
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

  /// The members that only send to it (Pipe.send_only_ids, #414), as the
  /// form has them.
  late final _sendOnly = {..._start.sendOnlyIds};

  /// The lead the person chose; kept while it's unticked, so ticking it
  /// back brings it back. [_shownLead] is the one that counts.
  late var _lead = _start.leadId;
  late var _review = _start.reviewOutgoing;
  late var _reviewIncoming = _start.reviewIncoming;
  late final _perHour = TextEditingController(
    text: '${_start.limits.messagesPerHour}',
  );
  late final _burst = TextEditingController(text: '${_start.limits.burst}');
  late final _fileMb = TextEditingController(
    text: '${_start.limits.fileMbPerDay}',
  );

  /// Its liaison's daily spending limit in dollars (ContainedSpend.cap_usd),
  /// set apart from the pipe (SetSpendCap) on Save.
  final _cap = TextEditingController();

  /// Whether [_cap] holds the limit yet: filled the first time it shows.
  var _capFilled = false;
  var _busy = false;
  String? _error;

  /// What the form is showing instead of the settings.
  var _confirmingClose = false;

  /// The member the person turned on "Can run commands", to confirm.
  Agent? _letting;
  var _addingLiaison = false;
  late final _liaisonName = TextEditingController(text: liaisonNameFor(_start));

  Pipe? get _pipe {
    for (final pipe in AppScope.of(context).workspace.workspace.pipes) {
      if (pipe.id == widget.pipeId) return pipe;
    }
    return null;
  }

  /// The lead as the form has it: the one chosen while it's a member, else
  /// the one Claktory gives a pipe whose lead leaves ([pipeLeadFor]); ""
  /// when none would, for the person to choose.
  String _shownLead(Pipe pipe) {
    // Only those still in its project count, as for the backend.
    final store = AppScope.of(context).workspace;
    // One that only sends never leads.
    final members = [
      for (final agent in store.agentsIn(pipe.projectId))
        if (_members.contains(agent.id) && !_sendOnly.contains(agent.id))
          agent.id,
    ];
    if (members.contains(_lead)) return _lead;
    return pipeLeadFor(
      [
        for (final id in _members)
          if (members.contains(id)) id,
      ],
      limited: _limited,
      liaisonId:
          pipeLiaison(AppScope.of(context).workspace.workspace, pipe)?.id ?? '',
    );
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
      _cap,
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
    final sendOnly = _sendOnly.intersection(_members);
    final limited = _limited.intersection(_members).difference(sendOnly);
    if (!_sameSet(limited, pipe.quarantinedIds)) {
      update.quarantined = PipeMembers(agentIds: limited);
    }
    // All of them too: a member left out reads the pipe again.
    if (!_sameSet(sendOnly, pipe.sendOnlyIds)) {
      update.sendOnly = PipeMembers(agentIds: sendOnly);
    }
    final lead = _shownLead(pipe);
    if (lead != pipe.leadId) update.leadId = lead;
    if (_review != pipe.reviewOutgoing) update.reviewOutgoing = _review;
    if (_reviewIncoming != pipe.reviewIncoming) {
      update.reviewIncoming = _reviewIncoming;
    }
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
    // Its liaison's daily spending limit, when it has one to change.
    final liaison = pipeLiaison(AppScope.of(context).workspace.workspace, pipe);
    SetSpendCap? cap;
    if (liaison != null && liaison.hasSpend()) {
      final typed = _cap.text.trim().replaceFirst(r'$', '');
      final usd = typed.isEmpty ? 0.0 : double.tryParse(typed);
      if (usd == null || usd < 0 || !usd.isFinite) {
        setState(() => _error = 'Enter an amount in dollars, such as 10.');
        return;
      }
      if (_capText(liaison.spend) != _capText(ContainedSpend(capUsd: usd))) {
        // Its token cap as it is: 0 there would put the default back.
        cap = SetSpendCap(
          agentId: liaison.id,
          capUsd: usd,
          capTokens: liaison.spend.capTokens,
        );
      }
    }
    _run((backend) async {
      if (cap != null) await backend.setSpendCap(cap);
      await backend.updatePipe(update);
    });
  }

  /// The spend of [pipe]'s liaison, when it has one.
  ContainedSpend? _spendOf(Pipe pipe) {
    final liaison = pipeLiaison(AppScope.of(context).workspace.workspace, pipe);
    return liaison != null && liaison.hasSpend() ? liaison.spend : null;
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
    // The liaison's limit as it is, in its field the first time it shows.
    if (!_capFilled) {
      if (_spendOf(pipe) case final spend?) {
        _cap.text = _capText(spend);
        _capFilled = true;
      }
    }
    // An ask over the settings keeps them as they were beneath it (where
    // they're scrolled to, what's typed), for Cancel to come back to
    // (Sale's row 77).
    Widget? over;
    if (_confirmingClose) {
      over = FormFrame(
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
    } else if (_letting case final agent?) {
      over = FormFrame(
        title: 'Let ${agent.name} run commands on this computer?',
        body: 'It will be able to reach your files, secrets and network.',
        error: null,
        fields: const [],
        submitLabel: 'Let it run commands',
        onSubmit: () => setState(() {
          _limited.remove(agent.id);
          _sendOnly.remove(agent.id);
          _letting = null;
        }),
        onCancel: () => setState(() => _letting = null),
      );
    } else if (_addingLiaison) {
      over = FormFrame(
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
    return Stack(
      children: [
        Offstage(
          offstage: over != null,
          child: TickerMode(enabled: over == null, child: _settings(pipe)),
        ),
        ?over,
      ],
    );
  }

  Widget _settings(Pipe pipe) {
    final agents = AppScope.of(context).workspace.agentsIn(pipe.projectId);
    // From outside its project, only to send (#414): the workspace's own
    // agents (Rivet among them), and any such member already.
    final outside = [
      for (final agent in AppScope.of(context).workspace.workspace.agents)
        if (agent.projectId != pipe.projectId &&
            (agent.projectId.isEmpty ||
                _members.contains(agent.id) ||
                pipe.memberIds.contains(agent.id)))
          agent,
    ];
    final lead = _shownLead(pipe);
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
        // Whose Claktory it is, unconfirmed: said where its settings open
        // from the header (a phone's has no mark of its own to tap).
        if (pipe.state != PipeState.PIPE_STATE_INVITED)
          Field(
            label: pipe.peer.verified && pipe.peer.verifiedEmail.isNotEmpty
                ? verifiedLabel(pipe)
                : 'Not verified',
            child: Text(
              pipe.peer.verified && pipe.peer.verifiedEmail.isNotEmpty
                  ? verifiedNote(pipe)
                  : notVerifiedNote(pipe, signedIn: _signedIn(context)),
              style: TypeScale.of(context).body
                  .copyWith(color: Palette.of(context).textMuted),
            ),
          ),
        Field(
          label: 'Name',
          // No keyboard raised by itself on a phone, over the settings.
          child: TextField(controller: _name, autofocus: !touchFirst(context)),
        ),
        Field(
          label: 'What it’s for',
          note: '${pipePeerName(pipe, start: true)} sees this.',
          child: TextField(controller: _purpose),
        ),
        Field(
          label: 'Instructions',
          note:
              'What your agents should know about this pipe. They get it '
              'after Claktory’s own safety rules. '
              '${pipePeerName(pipe, start: true)} never sees it.',
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
                        held: pipeHeldReason(pipe, agent.id),
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
                        _AccessLine(
                          agent: agent,
                          pipe: pipe,
                          value: _sendOnly.contains(agent.id)
                              ? PipeAccess.sendOnly
                              : _limited.contains(agent.id)
                              ? PipeAccess.limited
                              : PipeAccess.commands,
                          onChanged: (access) => setState(() {
                            switch (access) {
                              case PipeAccess.commands:
                                // Letting it out of the pipe is asked
                                // first.
                                _letting = agent;
                                _error = null;
                              case PipeAccess.limited:
                                _sendOnly.remove(agent.id);
                                _limited.add(agent.id);
                              case PipeAccess.sendOnly:
                                _limited.remove(agent.id);
                                _sendOnly.add(agent.id);
                            }
                          }),
                        ),
                    ],
                  ),
                ),
              for (final agent in outside)
                Padding(
                  padding: const EdgeInsets.only(bottom: Space.xs),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      _MemberLine(
                        agent: agent,
                        value: _members.contains(agent.id),
                        held: pipeHeldReason(pipe, agent.id),
                        onChanged: (on) => setState(() {
                          if (on) {
                            _members.add(agent.id);
                            _sendOnly.add(agent.id);
                          } else {
                            _members.remove(agent.id);
                            _sendOnly.remove(agent.id);
                          }
                        }),
                      ),
                      if (_members.contains(agent.id))
                        _AccessLine(
                          agent: agent,
                          pipe: pipe,
                          value: PipeAccess.sendOnly,
                          onChanged: null,
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
          note: lead.isEmpty && _members.isNotEmpty
              ? pipeNoLeadNote
              : 'Told of each message that comes in.',
          child: Select<String>(
            value: lead,
            options: [
              // Only while none leads it: never chosen on purpose.
              if (lead.isEmpty) const SelectOption('', 'No lead'),
              for (final agent in agents)
                if (_members.contains(agent.id) &&
                    !_sendOnly.contains(agent.id))
                  SelectOption(agent.id, agent.name),
            ],
            onChanged: (lead) => setState(() => _lead = lead),
          ),
        ),
        // Sent as it's chosen, apart from Save.
        ForwardReportsField(pipe: pipe),
        // So are its shared websites' Stop, Hide and Share a website….
        PipeSharesField(pipe: pipe),
        _ReviewIncomingField(
          value: _reviewIncoming,
          onChanged: (on) => setState(() => _reviewIncoming = on),
        ),
        _ReviewOutgoingField(
          value: _review,
          onChanged: (on) => setState(() => _review = on),
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
        if (_spendOf(pipe) case final spend?)
          Field(
            label: 'Daily spending limit',
            note: spendLimitNote(
              pipeLiaison(AppScope.of(context).workspace.workspace, pipe)!,
              spend,
            ),
            child: Row(
              children: [
                Text(
                  r'$',
                  style: TextStyles.body.copyWith(
                    color: Palette.of(context).textMuted,
                  ),
                ),
                const SizedBox(width: Space.xs),
                SizedBox(
                  width: 120,
                  child: TextField(controller: _cap, placeholder: '10'),
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

/// A liaison's daily spending limit as the field shows it: "10", "2.50".
String _capText(ContainedSpend? spend) {
  final usd = spend?.capUsd ?? 0;
  if (usd <= 0) return '';
  return usd == usd.roundToDouble() ? '${usd.round()}' : usd.toStringAsFixed(2);
}

/// Under a liaison's daily spending limit: what it used today, and what
/// happens past it.
String spendLimitNote(Agent liaison, ContainedSpend spend) =>
    '${liaison.name} has used about '
    '\$${spend.spentUsd.toStringAsFixed(2)} today. Past the limit, messages '
    'from the other side wait until midnight; yours still reach '
    '${liaison.name}.';

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

/// Under a member's line on the approval card: it only sends to the pipe
/// (#414), kept as it's set, with what that leaves out.
class _SendOnlyLine extends StatelessWidget {
  const _SendOnlyLine({required this.agent, required this.pipe});

  final Agent agent;
  final Pipe pipe;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      // Under the name, past the 20px character and its gap.
      padding: const EdgeInsets.only(left: 20 + Space.s, top: Space.xs),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            pipeAccessLabel(PipeAccess.sendOnly),
            style: TextStyles.caption.copyWith(color: palette.text),
          ),
          Padding(
            padding: const EdgeInsets.only(top: 2, right: Space.xl),
            child: Text(
              sendOnlyNote(agent.name, pipe),
              style: TextStyles.caption.copyWith(color: palette.textMuted),
            ),
          ),
        ],
      ),
    );
  }
}

/// A choice on an agent's approval card that's wider than the pipe now,
/// where the pipe narrowed since the agent asked (Fugo, #471): in the
/// consent cards' warning box, with what changed under it. [note] null:
/// just [child].
class _Narrowed extends StatelessWidget {
  const _Narrowed({required this.note, required this.child});

  final String? note;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final note = this.note;
    if (note == null) return child;
    final palette = Palette.of(context);
    return Container(
      margin: const EdgeInsets.only(top: Space.xs),
      padding: const EdgeInsets.all(Space.s),
      decoration: BoxDecoration(
        color: palette.signal.withValues(alpha: 0.06),
        border: Border.all(color: palette.signal.withValues(alpha: 0.4)),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          child,
          const SizedBox(height: Space.xs),
          Text(
            note,
            style: TextStyles.caption.copyWith(
              color: palette.text,
              fontWeight: FontWeight.w500,
            ),
          ),
        ],
      ),
    );
  }
}

/// Why [agentId] can't use [pipe] in any role, as Claktory says it: it's
/// another pipe's liaison (#414, Pipe.held_agents). Null when it can.
String? pipeHeldReason(Pipe pipe, String agentId) {
  for (final held in pipe.heldAgents) {
    if (held.agentId == agentId) return proseCodes(held.reason);
  }
  return null;
}

/// What a member of a pipe may do (#414): read it and run commands on this
/// computer, read it limited to the pipe, or only send to it.
enum PipeAccess { commands, limited, sendOnly }

/// What each [PipeAccess] is called where it's chosen.
String pipeAccessLabel(PipeAccess access) => switch (access) {
  PipeAccess.commands => 'Reads and runs commands',
  PipeAccess.limited => 'Limited to the pipe',
  PipeAccess.sendOnly => 'Only sends',
};

/// What an agent that only sends to a pipe can and can't do, under its
/// choice.
String sendOnlyNote(String agent, Pipe pipe) =>
    '$agent can send to ${pipePeerClaktory(pipe)} but never reads what '
    'comes back, and isn\'t told when it does.';

/// Under a member's line: what it may do with the pipe ([PipeAccess]),
/// indented to its name, with what that means: the risk while it can run
/// commands, and what sending only leaves out. [onChanged] null: one from
/// outside the pipe's project, which can only send.
class _AccessLine extends StatelessWidget {
  const _AccessLine({
    required this.agent,
    required this.pipe,
    required this.value,
    required this.onChanged,
  });

  final Agent agent;
  final Pipe pipe;
  final PipeAccess value;
  final ValueChanged<PipeAccess>? onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final onChanged = this.onChanged;
    final note = switch (value) {
      PipeAccess.commands => canRunCommandsWarning(
        agent.name,
        pipe,
        choice: true,
      ),
      PipeAccess.limited => null,
      PipeAccess.sendOnly => sendOnlyNote(agent.name, pipe),
    };
    return Padding(
      // Under the name, past the 20px character and its gap.
      padding: const EdgeInsets.only(left: 20 + Space.s, top: Space.xs),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (onChanged == null)
            Text(
              pipeAccessLabel(value),
              style: TextStyles.caption.copyWith(color: palette.text),
            )
          else
            Select<PipeAccess>(
              value: value,
              title: agent.name,
              options: [
                for (final access in PipeAccess.values)
                  SelectOption(access, pipeAccessLabel(access)),
              ],
              onChanged: onChanged,
            ),
          if (note != null)
            Padding(
              padding: const EdgeInsets.only(top: 2, right: Space.xl),
              child: Text(
                note,
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
    this.held,
  });

  final Agent agent;
  final bool value;
  final ValueChanged<bool> onChanged;

  /// Why it can't use the pipe (#414, [pipeHeldReason]): its switch is off
  /// and can't be turned on, with why under its name.
  final String? held;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    // One already on keeps its switch, to take it off.
    final held = value ? null : this.held;
    final line = _SwitchLine(
      label: agent.name,
      value: value,
      onChanged: held == null ? onChanged : null,
      leading: AgentCharacter(agent: agent, size: 20),
      child: NameThenRole(
        name: agent.name,
        role: agent.role,
        style: TextStyles.body.copyWith(
          color: held == null ? palette.text : palette.textMuted,
        ),
        roleColor: palette.textMuted,
      ),
    );
    if (held == null) return line;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        line,
        Padding(
          // Under the name, past the 20px character and its gap.
          padding: const EdgeInsets.only(
            left: 20 + Space.s,
            top: 2,
            right: Space.xl,
          ),
          child: Text(
            held,
            style: TextStyles.caption.copyWith(color: palette.textMuted),
          ),
        ),
      ],
    );
  }
}

/// Review incoming (Pipe.review_incoming): each message from the other
/// side waits for the person's Let in before any agent reads it.
class _ReviewIncomingField extends StatelessWidget {
  const _ReviewIncomingField({required this.value, required this.onChanged});

  final bool value;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) => Field(
    label: 'Review incoming',
    note: value
        ? 'Each message from the other side waits here until you let it in.'
        : 'Messages from the other side reach your agents as they arrive.',
    child: _SwitchLine(
      label: 'Review messages before your agents read them',
      value: value,
      onChanged: onChanged,
    ),
  );
}

/// Review outgoing (Pipe.review_outgoing): each message from this side's
/// agents waits for the person's Send.
class _ReviewOutgoingField extends StatelessWidget {
  const _ReviewOutgoingField({required this.value, required this.onChanged});

  final bool value;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) => Field(
    label: 'Review outgoing',
    note: value
        ? 'Each message from your agents waits here for your Send.'
        : 'Your agents’ messages go out without asking you.',
    child: _SwitchLine(
      label: 'Review messages before they go out',
      value: value,
      onChanged: onChanged,
    ),
  );
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

  /// Null: it can't be turned.
  final ValueChanged<bool>? onChanged;
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

  /// The invite's code shows as a QR too, to scan in person.
  var _qr = false;

  /// A code can be scanned here, with the camera (friend bug #2).
  var _canScan = false;
  var _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    canOfferScan(context).then((can) {
      if (mounted && can) setState(() => _canScan = true);
    });
  }

  @override
  void dispose() {
    _name.dispose();
    _purpose.dispose();
    _code.dispose();
    super.dispose();
  }

  /// Reads the code pasted or scanned: who it says it's from, to Connect.
  void _readCode() => _run((backend) async {
    final preview = await backend.readPipeInvite(_code.text.trim());
    if (!mounted) return;
    _name.text = preview.name;
    setState(() {
      _preview = preview;
      _step = _Step.preview;
    });
  });

  /// Scans the code someone sent, then reads it as a pasted one is.
  Future<void> _scan() async {
    final code = await scanCode(
      context,
      title: 'Scan a pipe invite',
      check: pipeInviteCheck,
      onPasteInstead: pasteCodeInto(_code, then: _readCode),
    );
    if (code == null || !mounted) return;
    _code.text = code;
    _readCode();
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
            // Making one takes a moment (Claktory reaches the relay): it
            // says so, rather than looking as if the tap did nothing.
            submitLabel: _busy ? 'Making…' : 'Make invite',
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
          // Copy code where Make invite was, and Done (which opens the
          // pipe) at the other end: a second tap on Make invite, as the
          // invite was being made, copies the code rather than closing it
          // unseen (Zucchero's row 157). It shows only this once.
          submitLabel: _copied ? 'Copied' : 'Copy code',
          onSubmit: () async {
            final pipe = AppScope.of(context).workspace.workspace.pipes
                .where((pipe) => pipe.id == invite.pipeId)
                .firstOrNull;
            await copyInviteCode(
              invite.code,
              expiresMs: pipe?.inviteExpiresMs.toInt() ?? 0,
            );
            if (mounted) setState(() => _copied = true);
          },
          onCancel: () {
            _openInvited(AppScope.of(context));
            widget.close();
          },
          cancelLabel: 'Done',
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
            if (!_qr) ...[
              const SizedBox(height: Space.s),
              Align(
                alignment: AlignmentDirectional.centerStart,
                child: Button(
                  label: 'Show QR code',
                  onPressed: () => setState(() => _qr = true),
                ),
              ),
            ],
            if (_qr) ...[
              const SizedBox(height: Space.l),
              PipeInviteQr(code: invite.code),
            ],
          ],
        );
      case _Step.code:
        return ListenableBuilder(
          listenable: _code,
          builder: (context, _) => FormFrame(
            title: 'Use a pipe code',
            body: _canScan
                ? 'Paste the code you were sent, or scan it.'
                : 'Paste the code you were sent.',
            error: _error,
            submitLabel: 'Continue',
            onSubmit: _busy || _code.text.trim().isEmpty ? null : _readCode,
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
              if (_canScan)
                Align(
                  alignment: Alignment.centerLeft,
                  child: Button(
                    label: 'Scan code',
                    onPressed: _busy ? null : _scan,
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

/// An agent ran a command with a secret set to ask each time, or after
/// reading a web page: Claktory held it for the person. A sibling of the
/// secret request card.
class SecretApprovalCard extends StatefulWidget {
  const SecretApprovalCard(this.approval, {super.key, required this.seq});

  final SecretApproval approval;

  /// The entry's seq in the agent's conversation.
  final int seq;

  @override
  State<SecretApprovalCard> createState() => _SecretApprovalCardState();
}

class _SecretApprovalCardState extends State<SecretApprovalCard>
    with PublishesAnswer {
  var _busy = false;
  String? _error;

  /// The person's comment, sent with Allow once or Don't allow.
  late final _comment = TextEditingController()..addListener(_typing);
  var _typed = false;

  /// A comment typed or cleared turns the dock's Submit on or off.
  void _typing() {
    final typed = _comment.text.trim().isNotEmpty;
    if (typed != _typed && mounted) setState(() => _typed = typed);
  }

  @override
  void dispose() {
    _comment.dispose();
    super.dispose();
  }

  /// Allow once and Don't allow for focus mode's keys (#514), and Submit
  /// once a comment's typed, as the card's buttons.
  @override
  CardAnswer? answerNow() {
    final approval = widget.approval;
    if (approval.state != SecretApprovalState.SECRET_APPROVAL_STATE_PENDING) {
      return null;
    }
    return CardAnswer(
      key: 'agent:${approval.agentId}/${widget.seq}',
      docked: false,
      yes: 'Allow once',
      no: "Don't allow",
      typed: _typed,
      busy: _busy,
      error: _error,
      onYes: _busy ? null : () => _decide(true, rethrowing: true),
      onSubmit: _busy
          ? null
          : () => _decide(false, reply: true, rethrowing: true),
      onNo: _busy
          ? null
          : (reason) => _decide(false, reason: reason, rethrowing: true),
    );
  }

  /// "AWS", "AWS and Stripe", "AWS, Stripe and GitHub".
  static String _listed(List<String> names) => names.length < 2
      ? names.join()
      : '${names.sublist(0, names.length - 1).join(', ')} and ${names.last}';

  /// The web pages [name] read ("a page on https://example.com", "3 web
  /// pages"); empty when it read none. A pipe or direct messages, on a card
  /// from before #346, no longer count.
  String _read() {
    final pages = taintPages(widget.approval.taint);
    if (pages.isEmpty) return '';
    return pages.length == 1
        ? taintRead(pages.single)!
        : '${pages.length} web pages';
  }

  /// Why Claktory asks: [name] read a web page, or the person set some of
  /// its secrets to ask each time, or both.
  String _why(String name) {
    final asks = widget.approval.askEachTimeNames;
    const expect = 'Allow it only if this command is what you expect.';
    final read = _read();
    if (asks.isEmpty) {
      return read.isEmpty
          ? 'Claktory asks you before $name uses a secret. $expect'
          : '$name has read $read, so Claktory asks you before it uses a '
                'secret. $expect';
    }
    final set = 'you set ${_listed(asks)} to ask each time';
    return read.isNotEmpty
        ? '$name has read $read, and $set. $expect'
        : '${set[0].toUpperCase()}${set.substring(1)}. $expect';
  }

  /// Allows it once or doesn't; [reply] (#370) doesn't, sending the
  /// comment as the answer. [reason]: the no's, from focus mode's dock,
  /// with the comment; [rethrowing] for its keys: refused, it throws too.
  Future<void> _decide(
    bool allow, {
    bool reply = false,
    String? reason,
    bool rethrowing = false,
  }) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).decideSecretApproval(
        widget.approval.agentId,
        widget.seq,
        allow,
        comment: noComment(reason, _comment.text),
        reply: reply,
      );
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
      if (rethrowing) rethrow;
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    publishAnswer();
    final palette = Palette.of(context);
    final approval = widget.approval;
    final agent = AppScope.of(context).workspace.agent(approval.agentId);
    final color = agentColor(agent?.color ?? 'slate');
    final name = agent?.name ?? approval.agentId;
    final pending =
        approval.state == SecretApprovalState.SECRET_APPROVAL_STATE_PENDING;
    final secrets = approval.secretNames.join(', ');
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
          // Where the command may come from: the web pages it read.
          TaintLabel(taint: approval.taint, agent: name),
          const SizedBox(height: Space.xs),
          Text(
            _why(name),
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
            // The command to approve, as tool calls show it: every hidden
            // character by its code (Fugo).
            child: Text(
              visibleCodes(approval.command),
              style: TextStyles.caption.copyWith(
                fontFamily: TextStyles.mono,
                color: palette.text,
              ),
            ),
          ),
          // Running on after it starts, it holds the secret all along.
          if (approval.keepsRunning) ...[
            const SizedBox(height: Space.xs),
            Text(
              'It keeps running after it starts, and can use '
              '${_listed(approval.secretNames)} the whole time.',
              style: TextStyles.caption.copyWith(color: palette.textMuted),
            ),
          ],
          const SizedBox(height: Space.m),
          if (pending) ...[
            // The field, open from the start, above the row (Lucy's rules
            // 2 and 4); a comment typed: one Submit, which doesn't allow it
            // (#370).
            CardAnswerBlock(
              field: (focus) => DecisionComment(
                controller: _comment,
                focusNode: focus,
                agent: name,
                enabled: !_busy,
                onSubmit: () => _decide(false, reply: true),
              ),
              row: CommentTyped(
                controller: _comment,
                builder: (context, typed) => CardRow(
                  line: CardLine(
                    _error ??
                        (typed
                            ? insteadLine('allowing')
                            : 'If you allow it, this command runs once.'),
                    color: _error == null ? null : palette.danger,
                  ),
                  quiet: [
                    if (!typed)
                      Button(
                        label: "Don't allow",
                        onPressed: _busy ? null : () => _decide(false),
                      ),
                  ],
                  primary: typed
                      ? SubmitButton(
                          onPressed: _busy
                              ? null
                              : () => _decide(false, reply: true),
                        )
                      : Button(
                          label: 'Allow once',
                          primary: true,
                          onPressed: _busy ? null : () => _decide(true),
                        ),
                ),
              ),
            ),
          ] else
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
                        // Not allowed with a comment (#370): the comment.
                        if (approval.replied && approval.comment.isNotEmpty)
                          TextSpan(
                            text: ' · “${approval.comment}”',
                            style: TextStyle(color: palette.text),
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
