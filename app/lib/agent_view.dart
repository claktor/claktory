import 'dart:async';

import 'package:flutter/gestures.dart' show kLongPressTimeout, kTouchSlop;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_name.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'inbox.dart' show NeedsYouChip, NeedsYouList;
import 'message_box.dart';
import 'mobile/drawer.dart' show largeText;
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'mobile/presentation.dart' show showPanel;
import 'model_step.dart' show showConnectModel;
import 'panes.dart';
import 'pipes.dart' show PipeLimitMark;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'settings_view.dart';
import 'transcript.dart';
import 'usage_wait.dart';

/// The wash behind [agent]'s conversation, in its color.
Color conversationTint(Agent agent) =>
    agentColor(agent.color).withValues(alpha: conversationTintAlpha);

/// The selected agent: its header, transcript and composer. Esc stops the
/// agent while it works.
class AgentView extends StatefulWidget {
  const AgentView({
    super.key,
    required this.agent,
    this.onBack,
    this.onMenu,
    this.onMore,
  });

  final Agent agent;

  /// On a phone, what the header leads with: Back, to the agent a message
  /// came from ([onBack]), else Menu, the drawer of Claktories and agents
  /// ([onMenu]). Either makes it a phone's conversation, the screen itself,
  /// with the agent's extensions behind "…" ([onMore]).
  final VoidCallback? onBack;
  final VoidCallback? onMenu;
  final VoidCallback? onMore;

  bool get _phone => onBack != null || onMenu != null;

  @override
  State<AgentView> createState() => _AgentViewState();
}

class _AgentViewState extends State<AgentView> {
  final _follow = ValueNotifier<int>(0);

  @override
  void dispose() {
    _follow.dispose();
    super.dispose();
  }

  bool get _working => widget.agent.status == AgentStatus.AGENT_STATUS_WORKING;

  void _stop() {
    if (_working) fireAndLog(AppScope.of(context).stopAgent(widget.agent.id));
  }

  /// What's under the conversation: on a phone, at most 60% of the room
  /// (a small phone on its side, the largest text), scrolling within that
  /// so nothing's cut off and some of the conversation stays in view.
  Widget _bottom(BoxConstraints constraints, List<Widget> children) {
    if (!widget._phone || !constraints.maxHeight.isFinite) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: children,
      );
    }
    return ConstrainedBox(
      constraints: BoxConstraints(maxHeight: constraints.maxHeight * 0.6),
      child: SingleChildScrollView(
        // Kept at the message box, its end.
        reverse: true,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: children,
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final transcript = backend.transcript;
    final agent = widget.agent;
    // The conversation: one of the agent's panes. On a phone the message
    // box takes at most 30% of what's in view above the keyboard.
    final chat = LayoutBuilder(
      builder: (context, constraints) => Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(
            child: _TapAwayKeyboard(
              enabled: widget._phone,
              child: AgentTints(
                own: agentColor(agent.color),
                byId: {
                  for (final other in backend.workspace.workspace.agents)
                    other.id: agentColor(other.color),
                },
                agents: {
                  for (final other in backend.workspace.workspace.agents)
                    other.id: identityOf(other),
                },
                // Following a message to its sender can be undone with Back.
                open: (id) =>
                    AppScope.scope(context).history.follow(agent.id, id),
                child: ListenableBuilder(
                  listenable: Listenable.merge([
                    transcript,
                    backend.queued,
                    backend.outbox,
                  ]),
                  builder: (context, _) {
                    if (transcript.agentId != agent.id) {
                      return const SizedBox.shrink();
                    }
                    // Nothing said yet: what the agent is for, and how to start.
                    if (transcript.length == 0 &&
                        backend.queued.of(agent.id).isEmpty &&
                        backend.outbox.of(agent.id).isEmpty &&
                        !_working) {
                      return EmptyConversation(agent: agent);
                    }
                    return TranscriptView(
                      key: ValueKey(transcript.loads),
                      transcript: transcript,
                      working: _working,
                      follow: _follow,
                      queued: backend.queued,
                      outbox: backend.outbox,
                      loadEarlier: () => backend.loadEarlier(agent.id),
                      agentId: agent.id,
                      reveal: backend.reveal,
                      loadFrom: (seq) => backend.loadFrom(agent.id, seq),
                    );
                  },
                ),
              ),
            ),
          ),
          _bottom(constraints, [
            ArchivedLine(agent: agent),
            // One line above the box, so the box never changes as the
            // agent starts and stops: reconnecting, else working with
            // Stop.
            ReconnectingLine(
              otherwise: _working
                  ? WorkingLine(onStop: _stop, wait: usageWaitLine(agent))
                  : const SizedBox.shrink(),
            ),
            MessageBox(
              target: AgentMessageTarget(agent, backend),
              onSent: () => _follow.value++,
              maxFieldHeight: constraints.maxHeight.isFinite
                  ? constraints.maxHeight * 0.3
                  : null,
            ),
          ]),
        ],
      ),
    );
    // Files dragged over the window go to this agent: the pane says so.
    final chatWithDrop = Stack(
      children: [
        Positioned.fill(child: chat),
        // On a Mac, how many things anywhere need the person, floating at
        // the conversation's top right; its bubbles drop down over the
        // conversation without moving it. On a phone the same chip, under
        // the header's "…" and clear of the drawer's edge on the left,
        // opens them in a sheet, in reach of the thumb.
        if (widget._phone)
          Positioned(
            top: Space.s,
            right: Space.s,
            // A control over the content, like a bar's: it grows with
            // the text to half again its size, not across the screen.
            child: MediaQuery.withClampedTextScaling(
              maxScaleFactor: 1.5,
              child: NeedsYouChip(onOpen: () => _showNeedsYou(context)),
            ),
          )
        else
          Positioned(
            top: Space.s,
            left: Space.m,
            right: Space.m,
            bottom: Space.m,
            child: const Align(
              alignment: Alignment.topRight,
              child: NeedsYouChip(),
            ),
          ),
      ],
    );
    final view = CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): _stop},
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          widget._phone
              ? _PhoneHeader(
                  agent: agent,
                  onBack: widget.onBack,
                  onMenu: widget.onMenu,
                  onMore: widget.onMore,
                )
              : _Header(agent: agent),
          // A phone shows one pane at a time: the conversation is the
          // screen, and its viewers open from "…".
          Expanded(
            child: widget._phone
                ? chatWithDrop
                : PaneLayout(agent: agent, chat: chatWithDrop),
          ),
        ],
      ),
    );
    // On a phone the conversation is the screen: one element for
    // VoiceOver, holding each of its own, none merged into another.
    return widget._phone
        ? Semantics(container: true, explicitChildNodes: true, child: view)
        : view;
  }
}

/// The agent's bar, in its color: its initial, name, model · effort and
/// context use. Opens the agent's settings. It's the top of the window, so
/// it also drags it.
class _Header extends StatelessWidget {
  const _Header({required this.agent});

  final Agent agent;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final model = AppScope.of(context).workspace.model(agent.model);
    // The model's name only when it can run; otherwise say so.
    final details = agent.modelAvailable
        ? [
            model == null ? agent.model : modelLabel(model),
            if (agent.effort.isNotEmpty) effortLabel(agent.effort),
          ].join(' · ')
        : 'No model available';
    final color = agentColor(agent.color);
    // Held up by usage limits, it sleeps rather than works: no wave.
    final working = atWork(agent);
    final header = DecoratedBox(
      decoration: BoxDecoration(
        gradient: LinearGradient(
          colors: [color.withValues(alpha: 0.14), color.withValues(alpha: 0)],
          stops: const [0, 0.7],
        ),
        border: Border(
          bottom: BorderSide(color: color.withValues(alpha: 0.7), width: 2),
        ),
      ),
      child: WindowDrag(
        child: Pressable(
          onTap: () => showModal(
            context,
            (context, close) => AgentSettingsForm(agent: agent, close: close),
          ),
          builder: (context, hovered) => Container(
            height: titleBarHeight,
            padding: const EdgeInsets.symmetric(horizontal: Space.xl),
            color: hovered ? palette.hover : null,
            child: Row(
              children: [
                _BackChip(current: agent.id),
                AgentCharacter(agent: agent, size: 34, live: true),
                const SizedBox(width: Space.s + 2),
                Expanded(
                  child: Row(
                    children: [
                      Flexible(
                        child: NameOverRole(
                          name: agent.name,
                          role: agent.role,
                          style: TypeScale.of(context).label.copyWith(
                            color: palette.text,
                            fontWeight: FontWeight.w600,
                          ),
                          roleColor: palette.textMuted,
                        ),
                      ),
                      const SizedBox(width: Space.m),
                      Flexible(
                        child: Text(
                          details,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TypeScale.of(context).body
                              .copyWith(color: palette.textMuted),
                        ),
                      ),
                      // Limited to a pipe: plainly, beside what it runs.
                      if (agent.quarantined) ...[
                        const SizedBox(width: Space.s),
                        Flexible(child: PipeLimitMark(agent: agent)),
                      ],
                      if (!agent.modelAvailable) ...[
                        const SizedBox(width: Space.s),
                        // No model at all: connect one, as the first
                        // launch offers.
                        AppScope.of(context).workspace.workspace.models.isEmpty
                            ? Button(
                                label: 'Connect…',
                                onPressed: () => showConnectModel(context),
                              )
                            : Button(
                                label: 'Sign in…',
                                onPressed: () =>
                                    AppScope.openProviders(context),
                              ),
                      ],
                    ],
                  ),
                ),
                const SizedBox(width: Space.l),
                _ContextMeter(
                  used: agent.contextTokens.toInt(),
                  limit: agent.compactAtTokens.toInt(),
                  label: agent.compactAtTokens.isZero
                      ? formatTokens(agent.contextTokens)
                      : '${formatTokens(agent.contextTokens)} / '
                            '${formatTokens(agent.compactAtTokens)}',
                ),
              ],
            ),
          ),
        ),
      ),
    );
    // While it works, a brighter stretch runs along its colored line, as
    // the wave does across its sidebar row, in step with it.
    return Stack(
      children: [
        header,
        if (working)
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

/// The agent's bar on a phone (docs/IDENTITY.md, "Phone"): Back or Menu,
/// its character with its name over its role, and "…" for its extensions,
/// each a finger's size, on its tint with its colored line (the working
/// wave runs along it). The name opens the agent's settings. The model and
/// context meter step aside; without a model, the message box says so and
/// offers what to do.
class _PhoneHeader extends StatelessWidget {
  const _PhoneHeader({
    required this.agent,
    this.onBack,
    this.onMenu,
    this.onMore,
  });

  final Agent agent;
  final VoidCallback? onBack;
  final VoidCallback? onMenu;
  final VoidCallback? onMore;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final color = agentColor(agent.color);
    final working = atWork(agent);
    return PhoneHeader(
      background: Color.alphaBlend(
        color.withValues(alpha: 0.14),
        palette.canvas,
      ),
      hairline: false,
      leading: onBack != null
          ? HeaderButton.back(onTap: onBack!)
          : HeaderButton.menu(onTap: onMenu ?? () {}),
      title: Row(
        children: [
          ExcludeSemantics(
            child: AgentCharacter(agent: agent, size: 28, live: true),
          ),
          const SizedBox(width: Space.s + 2),
          Flexible(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  agent.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: type.emphasis.copyWith(color: palette.text),
                ),
                if (agent.role.isNotEmpty)
                  Text(
                    agent.role,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: type.caption.copyWith(color: palette.textMuted),
                  ),
              ],
            ),
          ),
        ],
      ),
      titleLabel: [
        agent.name,
        if (agent.role.isNotEmpty) agent.role,
      ].join(', '),
      titleHint: 'Opens agent settings',
      onTitleTap: () => showModal(
        context,
        (context, close) => AgentSettingsForm(agent: agent, close: close),
      ),
      trailing: [
        if (onMore != null)
          HeaderButton.more(label: 'Extensions', onTap: onMore!),
      ],
      // Its colored line, with a brighter stretch running along it while
      // it works, in step with its sidebar row's wave.
      bottom: SizedBox(
        height: 2,
        child: Stack(
          fit: StackFit.expand,
          children: [
            ColoredBox(color: color.withValues(alpha: 0.7)),
            if (working) const WorkingWave(color: Color(0xFFFFFFFF), peak: 0.6),
          ],
        ),
      ),
    );
  }
}

/// Where Back returns to after following an agent's message: "← Coordinator",
/// with that agent's dot. Shown only while there's somewhere to go back to.
class _BackChip extends StatelessWidget {
  const _BackChip({required this.current});

  final String current;

  @override
  Widget build(BuildContext context) {
    final scope = AppScope.scope(context);
    final history = scope.history;
    return ListenableBuilder(
      listenable: history,
      builder: (context, _) {
        final id = history.back;
        final target = id == null || id == current
            ? null
            : scope.backend.workspace.agent(id);
        if (target == null) return const SizedBox.shrink();
        final palette = Palette.of(context);
        return Padding(
          padding: const EdgeInsets.only(right: Space.m),
          child: Semantics(
            button: true,
            label: 'Back to ${target.name}',
            excludeSemantics: true,
            child: Pressable(
              onTap: () => history.goBack(current),
              builder: (context, hovered) => AnimatedContainer(
                duration: Motion.fast,
                height: 28,
                constraints: const BoxConstraints(maxWidth: 180),
                padding: const EdgeInsets.only(
                  left: Space.s - 2,
                  right: Space.s + 2,
                ),
                decoration: BoxDecoration(
                  color: hovered ? palette.selected : palette.canvas,
                  borderRadius: BorderRadius.circular(14),
                  border: Border.all(color: palette.hairline),
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    ExtensionGlyph(
                      icon: 'arrow-left',
                      name: 'Back',
                      color: hovered ? palette.text : palette.textMuted,
                      size: 14,
                    ),
                    const SizedBox(width: Space.xs + 2),
                    AgentCharacter(agent: target, size: 20),
                    const SizedBox(width: Space.xs + 2),
                    Flexible(
                      child: Text(
                        target.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TypeScale.of(context).label
                            .copyWith(color: palette.text),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

class _ContextMeter extends StatelessWidget {
  const _ContextMeter({
    required this.used,
    required this.limit,
    required this.label,
  });

  final int used;
  final int limit;
  final String label;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final fraction = limit <= 0 ? 0.0 : (used / limit).clamp(0.0, 1.0);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (limit > 0) ...[
          Container(
            width: 40,
            height: 4,
            alignment: Alignment.centerLeft,
            decoration: BoxDecoration(
              color: palette.selected,
              borderRadius: BorderRadius.circular(2),
            ),
            child: FractionallySizedBox(
              widthFactor: fraction,
              heightFactor: 1,
              child: DecoratedBox(
                decoration: BoxDecoration(
                  color: palette.textMuted,
                  borderRadius: BorderRadius.circular(2),
                ),
              ),
            ),
          ),
          const SizedBox(width: Space.s),
        ],
        Text(
          label,
          style: TypeScale.of(context).caption.copyWith(
            color: palette.textMuted,
            fontFeatures: const [FontFeature.tabularFigures()],
          ),
        ),
      ],
    );
  }
}

/// While the connection is down (after a second, so a quick reconnect
/// doesn't flash it): a quiet line above the message box saying it's
/// reconnecting and that what shows may be out of date. The backend's
/// snapshot brings the conversation up to date once it's back.
class ReconnectingLine extends StatefulWidget {
  const ReconnectingLine({super.key, this.otherwise = const SizedBox.shrink()});

  /// What the line shows while the connection is fine.
  final Widget otherwise;

  /// How long the connection is down before the line shows.
  static const after = Duration(seconds: 1);

  @override
  State<ReconnectingLine> createState() => _ReconnectingLineState();
}

class _ReconnectingLineState extends State<ReconnectingLine> {
  Backend? _backend;
  Timer? _wait;
  var _shown = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (identical(backend, _backend)) return;
    _backend?.connected.removeListener(_changed);
    _backend?.refused.removeListener(_changed);
    _backend = backend
      ..connected.addListener(_changed)
      ..refused.addListener(_changed);
    _changed();
  }

  @override
  void dispose() {
    _wait?.cancel();
    _backend?.connected.removeListener(_changed);
    _backend?.refused.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    final backend = _backend!;
    // Turned away, the footer says so and what to do; this isn't that.
    final down = !backend.connected.value && backend.refused.value == null;
    if (!down) {
      _wait?.cancel();
      _wait = null;
      if (_shown) setState(() => _shown = false);
      return;
    }
    _wait ??= Timer(ReconnectingLine.after, () {
      if (mounted) setState(() => _shown = true);
    });
  }

  @override
  Widget build(BuildContext context) {
    if (!_shown) return widget.otherwise;
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.xl, Space.s, Space.xl, 0),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          const LiveDot(),
          const SizedBox(width: Space.s),
          Flexible(
            child: Text(
              reconnectingText,
              textAlign: TextAlign.center,
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textMuted),
            ),
          ),
        ],
      ),
    );
  }
}

/// While the agent works, a line above the message box at its end: the
/// live dot, "Working", and Stop (Esc on a Mac). Above the box rather than
/// in it, so the text being typed never moves as the agent starts and
/// stops. While usage limits hold it up, what it waits for instead, with
/// no dot: it isn't working meanwhile.
class WorkingLine extends StatelessWidget {
  const WorkingLine({super.key, required this.onStop, this.wait = ''});

  final VoidCallback onStop;

  /// What it waits for (Agent.usage_wait); empty while it works.
  final String wait;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    return Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: contentMaxWidth),
        child: Padding(
          // Its end lines up with the box's.
          padding: EdgeInsets.fromLTRB(
            phone ? Space.s : Space.xl,
            Space.s,
            phone ? Space.s : Space.xl,
            0,
          ),
          child: Builder(
            builder: (context) {
              final words = Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  if (wait.isEmpty) ...[
                    const LiveDot(),
                    const SizedBox(width: Space.s),
                  ],
                  Flexible(
                    child: Text(
                      wait.isEmpty ? 'Working' : wait,
                      style: TypeScale.of(context).caption
                          .copyWith(color: palette.textMuted),
                    ),
                  ),
                ],
              );
              final stop = Button(
                label: phone ? 'Stop' : 'Stop  esc',
                onPressed: onStop,
              );
              // At the largest text, Stop goes under the words, still at
              // the end, so neither is squeezed into a narrow column.
              if (phone && largeText(context)) {
                return Column(
                  crossAxisAlignment: CrossAxisAlignment.end,
                  children: [
                    words,
                    const SizedBox(height: Space.xs),
                    stop,
                  ],
                );
              }
              return Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  Flexible(child: words),
                  const SizedBox(width: Space.s),
                  stop,
                ],
              );
            },
          ),
        ),
      ),
    );
  }
}

/// What [ReconnectingLine] says.
const reconnectingText = "Reconnecting… You can send once it's back.";

/// For an agent in an archived project (opened from a link, say), a quiet
/// line above the message box saying so, with Unarchive. Messages sent
/// meanwhile wait for it.
class ArchivedLine extends StatelessWidget {
  const ArchivedLine({super.key, required this.agent});

  final Agent agent;

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final project = archivedProjectOf(backend.workspace.workspace, agent);
        if (project == null) return const SizedBox.shrink();
        final palette = Palette.of(context);
        return Padding(
          padding: const EdgeInsets.fromLTRB(Space.xl, Space.s, Space.xl, 0),
          child: Wrap(
            alignment: WrapAlignment.center,
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: Space.s,
            runSpacing: Space.xs,
            children: [
              Text(
                '${project.name} is archived, so its agents don\'t work until '
                'you unarchive it. Messages you send wait until then.',
                textAlign: TextAlign.center,
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textMuted),
              ),
              Button(
                label: 'Unarchive',
                onPressed: () => fireAndLog(unarchiveProject(backend, project)),
              ),
            ],
          ),
        );
      },
    );
  }
}

/// A conversation with nothing in it yet: the agent, what it's for (its
/// role), and that you start by telling it what to do. Quiet, centered.
class EmptyConversation extends StatelessWidget {
  const EmptyConversation({super.key, required this.agent});

  final Agent agent;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final name = agent.name.isEmpty ? agent.id : agent.name;
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(Space.xl),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 360),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              AgentCharacter(agent: identityOf(agent), size: 48),
              const SizedBox(height: Space.m),
              Text(
                name,
                textAlign: TextAlign.center,
                style: TypeScale.of(context).label
                    .copyWith(color: palette.text),
              ),
              if (agent.role.isNotEmpty)
                Text(
                  agent.role,
                  textAlign: TextAlign.center,
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textMuted),
                ),
              const SizedBox(height: Space.s),
              Text(
                emptyConversationHint(name),
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

/// What the empty conversation says under the agent.
String emptyConversationHint(String name) =>
    "Tell $name what you'd like done. It remembers this conversation, "
    'so you can pick up anytime.';

/// On a phone, everything that waits for the person in a sheet, from the
/// conversation's Needs you chip: answered there, each stays, showing how
/// it ended, until the sheet closes. Opening a conversation or a pipe from
/// it (or another device opening one) takes the sheet down with it.
Future<void> _showNeedsYou(BuildContext context) {
  final backend = AppScope.of(context);
  final workspace = backend.workspace;
  return showPanel(
    context,
    title: 'Needs you',
    builder: (context, close) => _ClosesOnOpen(
      opened: Listenable.merge([workspace, backend.reveal]),
      where: () => (
        workspace.workspace.selectedAgentId,
        workspace.workspace.selectedPipeId,
        backend.reveal.value,
      ),
      onOpened: close,
      child: const NeedsYouList(shrinkWrap: true, padding: EdgeInsets.zero),
    ),
  );
}

/// Calls [onOpened] once [where] (what's open) changes.
class _ClosesOnOpen extends StatefulWidget {
  const _ClosesOnOpen({
    required this.opened,
    required this.where,
    required this.onOpened,
    required this.child,
  });

  final Listenable opened;
  final Object? Function() where;
  final VoidCallback onOpened;
  final Widget child;

  @override
  State<_ClosesOnOpen> createState() => _ClosesOnOpenState();
}

class _ClosesOnOpenState extends State<_ClosesOnOpen> {
  late final Object? _at = widget.where();
  var _closed = false;

  @override
  void initState() {
    super.initState();
    widget.opened.addListener(_check);
  }

  @override
  void dispose() {
    widget.opened.removeListener(_check);
    super.dispose();
  }

  void _check() {
    if (_closed || widget.where() == _at) return;
    _closed = true;
    widget.onOpened();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

/// On a phone, a tap on the conversation (not a drag, not a hold) tucks
/// the keyboard away, as Messages does; what's tapped still gets the tap.
/// Elsewhere [child] as it is.
class _TapAwayKeyboard extends StatefulWidget {
  const _TapAwayKeyboard({required this.enabled, required this.child});

  final bool enabled;
  final Widget child;

  @override
  State<_TapAwayKeyboard> createState() => _TapAwayKeyboardState();
}

class _TapAwayKeyboardState extends State<_TapAwayKeyboard> {
  int? _pointer;
  Offset _down = Offset.zero;
  DateTime _at = DateTime(0);

  @override
  Widget build(BuildContext context) {
    if (!widget.enabled) return widget.child;
    return Listener(
      onPointerDown: (event) {
        _pointer = event.pointer;
        _down = event.position;
        _at = DateTime.now();
      },
      onPointerUp: (event) {
        final tap =
            event.pointer == _pointer &&
            (event.position - _down).distance < kTouchSlop &&
            DateTime.now().difference(_at) < kLongPressTimeout;
        _pointer = null;
        // Only a text field's focus: the conversation's own selection
        // keeps its focus, or a double tap would lose the word it picked.
        final focus = FocusManager.instance.primaryFocus;
        if (tap &&
            focus?.context?.findAncestorStateOfType<EditableTextState>() !=
                null) {
          focus!.unfocus();
        }
      },
      onPointerCancel: (_) => _pointer = null,
      child: widget.child,
    );
  }
}
