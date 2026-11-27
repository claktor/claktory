import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart'
    show TargetPlatform, defaultTargetPlatform;
import 'package:flutter/gestures.dart' show kLongPressTimeout, kTouchSlop;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_menu.dart';
import 'agent_name.dart';
import 'backend.dart';
import 'browser/browser_view.dart' show BrowserSessionStrip;
import 'characters/character.dart';
import 'connection.dart';
import 'controls.dart';
import 'conversation_find.dart' show ConversationFind;
import 'errands/browser_session.dart'
    show agentBrowserAvailable, runningErrandOf;
import 'errands/errand_controls.dart' show ErrandHeaderControls;
import 'find.dart' show ClearOfFindBar, FindSessions;
import 'forms.dart';
import 'highlights.dart' show Glow;
import 'identity.dart';
import 'inbox.dart' show NeedsYouChip;
import 'kept.dart';
import 'message_box.dart';
import 'mobile/agent_header.dart' show PhoneAgentHeader;
import 'mobile/lines.dart' show ActionLine, ControlsLine;
import 'mobile/metrics.dart';
import 'mobile/typing_above.dart';
import 'model_popover.dart' show showModelPopover;
import 'model_step.dart' show showConnectModel;
import 'panes.dart';
import 'pipe_reports.dart' show ReadsReportsMark;
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

  /// The conversation: the same widget while the agent is, so another
  /// agent's step (several a second while a team works) doesn't build
  /// every message in view again. It follows the conversation itself on
  /// its own.
  Widget _conversationOf(Agent agent, Backend backend) {
    final transcript = backend.transcript;
    return Kept(
      inputs: [agent],
      builder: (_) => ListenableBuilder(
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
    );
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final agent = widget.agent;
    final workspace = backend.workspace.workspace;
    // The conversation: one of the agent's panes. On a phone the message
    // box takes at most 30% of what's in view above the keyboard.
    // On a phone the box steps aside while a field in the conversation (a
    // card's) has the keyboard (TypingAbove).
    final chat = LayoutBuilder(
      builder: (context, constraints) => TypingAbove(
        enabled: widget._phone,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(
              child: TypingAboveArea(
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
                    child: _conversationOf(agent, backend),
                  ),
                ),
              ),
            ),
            _bottom(constraints, [
              // Its browser sessions: waiting, running, just ended.
              BrowserSessionStrip(agentId: agent.id),
              ArchivedLine(agent: agent),
              // Back to its own provider once it has room (#341).
              UsageSwitchLine(id: agent.id),
              // One line above the box, so the box never changes as the
              // agent starts and stops: reconnecting, else working with
              // Stop.
              ReconnectingLine(
                otherwise: _working
                    ? WorkingLine(
                        onStop: _stop,
                        wait: usageWaitLine(agent),
                        waiting: agent.id,
                      )
                    : const SizedBox.shrink(),
              ),
              // The same box while what it shows from the Workspace is.
              TypingAboveBox(
                child: Kept(
                  inputs: [
                    agent,
                    workspace.models.isEmpty,
                    workspace.speechModel,
                    constraints.maxHeight,
                  ],
                  builder: (_) => MessageBox(
                    target: AgentMessageTarget(agent, backend),
                    onSent: () => _follow.value++,
                    maxFieldHeight: constraints.maxHeight.isFinite
                        ? constraints.maxHeight * 0.3
                        : null,
                  ),
                ),
              ),
            ]),
          ],
        ),
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
        // A phone keeps it in the header instead (PhoneNeedsYouButton).
        // It steps aside while the conversation's find bar is open there.
        if (!widget._phone)
          Positioned(
            top: Space.s,
            left: Space.m,
            right: Space.m,
            bottom: Space.m,
            child: const Align(
              alignment: Alignment.topRight,
              child: ClearOfFindBar(child: NeedsYouChip()),
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
              ? PhoneAgentHeader(
                  agent: agent,
                  onBack: widget.onBack,
                  onMenu: widget.onMenu,
                  onMore: widget.onMore,
                )
              : Kept(
                  inputs: [
                    agent,
                    backend.workspace.model(agent.model),
                    workspace.models.isEmpty,
                    workspace.pipes,
                    workspace.extensions,
                    // Its errand's controls (Claktory's browser).
                    workspace.errands,
                    workspace.agentBrowser,
                  ],
                  builder: (_) => _Header(
                    agent: agent,
                    // The pane last tapped in this view, as ⌘F finds in
                    // (Settings beside it, say: ST14); else the
                    // conversation's, below the header, rather than a pane
                    // that's only beside it.
                    // Once ⋯'s menu, which it's chosen from, is gone from
                    // the screen: it would cover the pane tapped last.
                    onFind: () => unawaited(
                      WidgetsBinding.instance.endOfFrame.then((_) {
                        if (!context.mounted) return;
                        (FindSessions.instance.pressedWithin(context) ??
                                FindSessions.instance.within(
                                  context,
                                  only: (target) => target is ConversationFind,
                                ))
                            ?.open();
                      }),
                    ),
                  ),
                ),
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

/// The agent's bar, in its color (agent-view.md §3): its character and
/// name, a button to its settings; its model · effort, a control that
/// changes them in place; its marks; then its Checklist (and Tasks, while
/// they run), its context meter and ⋯, its menu. Nothing else in it is a
/// button. It's the top of the window, so the space between drags it.
class _Header extends StatelessWidget {
  const _Header({required this.agent, required this.onFind});

  final Agent agent;

  /// Opens the find bar of the conversation (as ⌘F does).
  final VoidCallback onFind;

  @override
  Widget build(BuildContext context) {
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
        // At least the title bar's height, and taller when large text
        // needs it (an iPad at accessibility sizes), never cut.
        child: Container(
          constraints: const BoxConstraints(minHeight: titleBarHeight),
          padding: const EdgeInsetsDirectional.only(
            start: Space.xl - _nameInset,
            end: Space.m,
            top: Space.xs,
            bottom: Space.xs,
          ),
          child: LayoutBuilder(
            builder: (context, constraints) {
              final way = giveWay(context, constraints.maxWidth);
              return _row(context, way);
            },
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

  /// How far the name button's tint reaches past the character.
  static const _nameInset = Space.xs + 2;

  /// The empty space kept between the marks and the panel buttons, to
  /// drag the window by (Polnareff).
  static const dragRoom = 80.0;

  /// How much has given way in [width] (agent-view.md §3), in order: 1,
  /// Tasks drops its words to its glyph and count; 2, Checklist does; 3,
  /// the meter goes; 4, the effort leaves the model control; 5, the role
  /// under the name goes. Past that the name is cut. The model control and
  /// ⋯ never go. Past all that, the name is cut, then the space kept to
  /// drag by shrinks from [dragRoom], and only then is the model control
  /// cut, to [GiveWay.model].
  GiveWay giveWay(BuildContext context, double width) {
    final workspace = AppScope.of(context).workspace.workspace;
    final type = TypeScale.of(context);
    final scaler = MediaQuery.textScalerOf(context);
    final touch = touchFirst(context);
    double measure(String text, TextStyle style) {
      if (text.isEmpty) return 0;
      final painter = TextPainter(
        text: TextSpan(text: text, style: style),
        textScaler: scaler,
        textDirection: TextDirection.ltr,
        maxLines: 1,
      )..layout();
      final width = painter.width;
      painter.dispose();
      return width.ceilToDouble();
    }

    final labelStyle = type.label.copyWith(fontWeight: FontWeight.w600);
    final roleWidth = measure(agent.role, roleStyle(const Color(0xFF000000)));
    final nameWidth = measure(agent.name, labelStyle);
    final panels = _panels(workspace);
    final target = touch ? Metrics.of(context).target : 28.0;
    final errand =
        agentBrowserAvailable(workspace) &&
        runningErrandOf(workspace, agent.id) != null;
    double need(int level) {
      var total = Space.xl - _nameInset + Space.m + dragRoom;
      // Character and name, in their button.
      total += 2 * _nameInset + 34 + Space.s + 2;
      total += level >= 5 ? nameWidth : math.max(nameWidth, roleWidth);
      // The model control.
      total +=
          Space.s +
          _ModelControl.width(
            measure,
            type.body,
            agent,
            workspace,
            effort: level < 4,
            touch: touch,
          );
      for (final (id, _) in panels) {
        final compact = id == 'tasks' ? level >= 1 : level >= 2;
        total +=
            Space.s +
            _PanelButton.width(
              measure,
              type.label,
              _PanelButton.words(id, agent),
              _PanelButton.number(id, agent, compact: compact),
              compact: compact,
              touch: touch,
            );
      }
      if (level < 3) {
        total += Space.m + 40 + Space.s + measure(_meterLabel, type.caption);
      }
      // Its errand in Claktory's browser, while it runs one: the errand
      // and Stop.
      if (errand) {
        total +=
            2 * Space.s +
            target +
            2 * (touch ? Touch.gutter : Space.m) +
            measure('Stop', touch ? PhoneType.label : TextStyles.label);
      }
      // ⋯.
      return total + Space.s + target;
    }

    for (var level = 0; level < 5; level++) {
      if (need(level) <= width) {
        return (level: level, drag: dragRoom, model: null);
      }
    }
    var short = need(5) - width;
    // The name, to a few letters (its Flexible cuts it).
    short -= math.min(math.max(short, 0), math.max(0, nameWidth - 48));
    final drag = dragRoom - math.min(math.max(short, 0), dragRoom - Space.m);
    short -= dragRoom - drag;
    final model = short <= 0
        ? null
        : math.max(
            48.0,
            _ModelControl.width(
                  measure,
                  type.body,
                  agent,
                  workspace,
                  effort: false,
                  touch: touch,
                ) -
                short,
          );
    return (level: 5, drag: drag, model: model);
  }

  String get _meterLabel => agent.compactAtTokens.isZero
      ? formatTokens(agent.contextTokens)
      : '${formatTokens(agent.contextTokens)} / '
            '${formatTokens(agent.compactAtTokens)}';

  /// "This conversation: 212k of 400k. At 400k, June summarizes it and
  /// carries on."
  String get _meterCard {
    final used = formatTokens(agent.contextTokens);
    if (agent.compactAtTokens.isZero) return 'This conversation: $used.';
    final at = formatTokens(agent.compactAtTokens);
    return 'This conversation: $used of $at. At $at, ${agent.name} '
        'summarizes it and carries on.';
  }

  /// The header's panel buttons: Checklist when it's on, Tasks while tasks
  /// run, each with its extension.
  List<(String, Extension)> _panels(Workspace workspace) => [
    for (final extension in menuPanels(workspace, agent))
      if (extension.id == 'checklist' ||
          (extension.id == 'tasks' && agent.tasksRunning > 0))
        (extension.id, extension),
  ];

  Widget _row(BuildContext context, GiveWay way) {
    final (:level, :drag, model: modelMax) = way;
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final workspace = backend.workspace.workspace;
    final panels = _panels(workspace);
    final open = agent.openPanel;
    final name = _NameButton(agent: agent, role: level < 5);
    final model = agent.modelAvailable
        ? _ModelControl(
            key: const ValueKey('model-control'),
            agent: agent,
            effort: level < 4,
          )
        // It can't run: said plainly, and in the control's place what to
        // do: connect a model when there's none at all, as the first
        // launch offers, else sign in again.
        : Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Flexible(
                child: Text(
                  'No model available',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.textMuted),
                ),
              ),
              const SizedBox(width: Space.s),
              workspace.models.isEmpty
                  ? Button(
                      label: 'Connect…',
                      onPressed: () => showConnectModel(context),
                    )
                  : Button(
                      label: 'Sign in…',
                      onPressed: () => AppScope.openProviders(context),
                    ),
            ],
          );
    final marks = [
      // Limited to a pipe: plainly, beside what it runs.
      if (agent.quarantined) PipeLimitMark(agent: agent),
      // Reads pipes' reports: from how many, to open them.
      if (agent.readsReports) ReadsReportsMark(agent: agent),
    ];
    final left = Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        _BackChip(current: agent.id),
        // The name is cut first, the model only past that.
        Flexible(flex: 3, child: name),
        const SizedBox(width: Space.s),
        modelMax == null
            ? model
            : ConstrainedBox(
                constraints: BoxConstraints(maxWidth: modelMax),
                child: model,
              ),
        if (marks.isNotEmpty)
          Flexible(
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (final mark in marks) ...[
                  const SizedBox(width: Space.s),
                  Flexible(child: mark),
                ],
              ],
            ),
          ),
      ],
    );
    // Anything an agent points at in a panel without a button here glows
    // on ⋯; the checklist's on its button.
    final buttons = {for (final (id, _) in panels) id};
    final right = Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        // Its errand in Claktory's browser: the errand, and Stop.
        ErrandHeaderControls(agent: agent),
        for (final (id, extension) in panels) ...[
          const SizedBox(width: Space.s),
          Glow(
            store: backend.highlights,
            side: AxisDirection.left,
            matches: (key) =>
                key == 'extension:${agent.id}/$id' ||
                (id == 'checklist' &&
                    open != 'checklist' &&
                    key.startsWith('checklist_item:${agent.id}#')),
            child: _PanelButton(
              key: ValueKey('panel-button:$id'),
              extension: extension,
              agent: agent,
              compact: id == 'tasks' ? level >= 1 : level >= 2,
            ),
          ),
        ],
        if (level < 3) ...[
          const SizedBox(width: Space.m),
          HoverCard(
            end: true,
            // On touch a tap shows it, and a tap elsewhere closes it.
            tapToShow: touchFirst(context),
            card: (context) => ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 280),
              child: Text(_meterCard),
            ),
            // Its own element, saying what the card says.
            child: Semantics(
              container: true,
              label: _meterCard,
              excludeSemantics: true,
              child: _ContextMeter(
                used: agent.contextTokens.toInt(),
                limit: agent.compactAtTokens.toInt(),
                label: _meterLabel,
              ),
            ),
          ),
        ],
        const SizedBox(width: Space.s),
        Glow(
          store: backend.highlights,
          side: AxisDirection.left,
          matches: (key) {
            const prefix = 'extension:';
            if (!key.startsWith('$prefix${agent.id}/')) return false;
            final id = key.substring(prefix.length + agent.id.length + 1);
            return !buttons.contains(id);
          },
          child: Builder(
            builder: (context) => GlyphButton(
              key: const ValueKey('agent-menu-button'),
              label: 'More options for ${agent.name}',
              active: false,
              onTap: () =>
                  unawaited(showAgentMenu(context, agent, onFind: onFind)),
              glyph: (color) =>
                  ExtensionGlyph(icon: 'ellipsis', name: 'More', color: color),
            ),
          ),
        ),
      ],
    );
    return DefaultTextStyle.merge(
      style: TextStyle(color: palette.text),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        children: [
          Flexible(child: left),
          Padding(
            padding: EdgeInsetsDirectional.only(start: drag),
            child: right,
          ),
        ],
      ),
    );
  }
}

/// How much of the header has given way ([_Header.giveWay]): its [level],
/// the room kept to drag the window by, and the model control's width at
/// most, when even it must be cut.
typedef GiveWay = ({int level, double drag, double? model});

/// The agent's character and name over its role (without the role when
/// the header is narrow): a button to its settings, with its own hover
/// tint.
class _NameButton extends StatelessWidget {
  const _NameButton({required this.agent, required this.role});

  final Agent agent;
  final bool role;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      label: [
        agent.name,
        if (role && agent.role.isNotEmpty) agent.role,
      ].join(', '),
      hint: 'Opens agent settings',
      // Its own element, which VoiceOver presses as a click does
      // (Abbacchio's row 1615).
      container: true,
      onTap: () => showAgentSettings(context, agent),
      excludeSemantics: true,
      child: Pressable(
        onTap: () => showAgentSettings(context, agent),
        builder: (context, hovered) => AnimatedContainer(
          duration: Motion.fast,
          padding: const EdgeInsets.symmetric(horizontal: _Header._nameInset),
          decoration: BoxDecoration(
            color: hovered ? palette.hover : const Color(0x00000000),
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              AgentCharacter(agent: agent, size: 34, live: true),
              const SizedBox(width: Space.s + 2),
              Flexible(
                child: NameOverRole(
                  name: agent.name,
                  role: role ? agent.role : '',
                  style: TypeScale.of(context).label.copyWith(
                    color: palette.text,
                    fontWeight: FontWeight.w600,
                  ),
                  roleColor: palette.textMuted,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// How long "Changed to … · Undo" stays after a change from the popover.
const modelUndoTime = Duration(seconds: 6);

/// The agent's model · effort with a chevron: a quiet button that opens
/// the model popover. For [modelUndoTime] after a change from it, it reads
/// "Changed to Haiku 4.5 · Undo", and Undo sets both back as they were.
class _ModelControl extends StatefulWidget {
  const _ModelControl({super.key, required this.agent, required this.effort});

  final Agent agent;

  /// The effort shows after the model (it goes first when narrow).
  final bool effort;

  /// Its words: "Claude Opus 4.8 · High".
  static String label(Agent agent, Workspace workspace, {bool effort = true}) {
    final model = workspace.models
        .where((model) => model.id == agent.model)
        .firstOrNull;
    return [
      model == null ? agent.model : modelLabel(model),
      if (effort && agent.effort.isNotEmpty) effortLabel(agent.effort),
    ].join(' · ');
  }

  /// Its width at rest, for the header's give-way.
  static double width(
    double Function(String, TextStyle) measure,
    TextStyle style,
    Agent agent,
    Workspace workspace, {
    required bool effort,
    required bool touch,
  }) => agent.modelAvailable
      ? 2 * (Space.s + 2) +
            measure(label(agent, workspace, effort: effort), style) +
            Space.s +
            10 +
            2
      // "No model available" and Connect… or Sign in….
      : measure('No model available', style) +
            Space.s +
            2 * (touch ? Touch.gutter : Space.m) +
            measure(
              workspace.models.isEmpty ? 'Connect…' : 'Sign in…',
              touch ? PhoneType.label : TextStyles.label,
            );

  @override
  State<_ModelControl> createState() => _ModelControlState();
}

class _ModelControlState extends State<_ModelControl> {
  /// What the model and effort were before the popover changed them, while
  /// Undo shows.
  ({String model, String effort})? _before;
  Timer? _timer;

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  void _changed(String model, String effort) {
    if (!mounted) return;
    setState(() => _before ??= (model: model, effort: effort));
    _timer?.cancel();
    _timer = Timer(modelUndoTime, () {
      if (mounted) setState(() => _before = null);
    });
  }

  void _undo() {
    final before = _before;
    if (before == null) return;
    _timer?.cancel();
    setState(() => _before = null);
    final agent = widget.agent;
    final update = UpdateAgent(agentId: agent.id);
    if (before.model != agent.model) update.model = before.model;
    if (before.effort != agent.effort) update.effort = before.effort;
    if (update.hasModel() || update.hasEffort()) {
      fireAndLog(AppScope.of(context).updateAgent(update));
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final agent = widget.agent;
    final touch = touchFirst(context);
    // Set back meanwhile (elsewhere, or by Undo): nothing to undo.
    final before =
        _before != null &&
            (_before!.model != agent.model || _before!.effort != agent.effort)
        ? _before
        : null;
    if (before != null) {
      // What it changed to: the model, or the effort when only it did.
      final to = before.model != agent.model
          ? _ModelControl.label(agent, workspace, effort: false)
          : effortLabel(agent.effort);
      return Container(
        constraints: BoxConstraints(
          minHeight: touch ? Metrics.of(context).target : 28,
        ),
        padding: const EdgeInsets.symmetric(horizontal: Space.s + 2),
        alignment: Alignment.center,
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Flexible(
              child: Semantics(
                container: true,
                child: Text(
                  'Changed to $to · ',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: type.body.copyWith(color: palette.textMuted),
                ),
              ),
            ),
            Semantics(
              button: true,
              container: true,
              onTap: _undo,
              label:
                  'Undo, back to '
                  '${before.model != agent.model ? _modelName(workspace, before.model) : effortLabel(before.effort)}',
              excludeSemantics: true,
              child: Pressable(
                onTap: _undo,
                builder: (context, hovered) => Text(
                  'Undo',
                  style: type.body.copyWith(
                    color: palette.text,
                    decoration: hovered ? TextDecoration.underline : null,
                  ),
                ),
              ),
            ),
          ],
        ),
      );
    }
    final label = _ModelControl.label(agent, workspace, effort: widget.effort);
    return Builder(
      builder: (context) {
        void open() =>
            unawaited(showModelPopover(context, agent, changed: _changed));
        return Semantics(
          button: true,
          label: 'Model: ${_ModelControl.label(agent, workspace)}',
          hint: 'Changes the model',
          container: true,
          onTap: open,
          excludeSemantics: true,
          child: Pressable(
            onTap: open,
            builder: (context, hovered) => AnimatedContainer(
              duration: Motion.fast,
              constraints: BoxConstraints(
                minHeight: touch ? Metrics.of(context).target : 28,
              ),
              padding: const EdgeInsets.symmetric(horizontal: Space.s + 2),
              decoration: BoxDecoration(
                color: hovered ? palette.hover : const Color(0x00000000),
                borderRadius: BorderRadius.circular(Radii.control),
                border: Border.all(color: palette.hairline),
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Flexible(
                    child: Text(
                      label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: type.body.copyWith(
                        color: hovered ? palette.text : palette.textMuted,
                      ),
                    ),
                  ),
                  const SizedBox(width: Space.s),
                  Chevron(color: palette.textMuted, down: true),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

String _modelName(Workspace workspace, String id) {
  final model = workspace.models.where((model) => model.id == id).firstOrNull;
  return model == null ? id : modelLabel(model);
}

/// A panel's button in the header, Checklist or Tasks: its glyph and words
/// ("Checklist 4", "Tasks · 2 running"), just the glyph and the count when
/// [compact]; tinted while its panel is open. A click opens the panel, or
/// closes it when it's open.
class _PanelButton extends StatelessWidget {
  const _PanelButton({
    super.key,
    required this.extension,
    required this.agent,
    required this.compact,
  });

  final Extension extension;
  final Agent agent;
  final bool compact;

  /// Its words beside the glyph.
  static String words(String id, Agent agent) => switch (id) {
    'tasks' => 'Tasks · ${agent.tasksRunning} running',
    _ => 'Checklist',
  };

  /// The number after the words (Checklist's open items), or after the
  /// glyph alone when [compact] (the tasks running too); empty for none.
  static String number(String id, Agent agent, {required bool compact}) {
    final count = panelCount(agent, id);
    if (count == 0 || (id == 'tasks' && !compact)) return '';
    return '$count';
  }

  /// Its width, for the header's give-way.
  static double width(
    double Function(String, TextStyle) measure,
    TextStyle style,
    String words,
    String number, {
    required bool compact,
    required bool touch,
  }) {
    final shown = [
      if (!compact) measure(words, style),
      if (number.isNotEmpty) measure(number, style),
    ];
    final content =
        16 +
        shown.fold(0.0, (sum, width) => sum + width) +
        shown.length * (Space.xs + 2);
    return math.max(content + 2 * Space.s, touch ? 44 : 28);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final id = extension.id;
    final active = agent.openPanel == id;
    final words = _PanelButton.words(id, agent);
    final number = _PanelButton.number(id, agent, compact: compact);
    final touch = touchFirst(context);
    final said = switch (id) {
      'tasks' => 'Tasks, ${agent.tasksRunning} running',
      _ when agent.checklistOpen == 0 => extension.name,
      _ => '${extension.name}, ${agent.checklistOpen} open',
    };
    return Semantics(
      button: true,
      // A toggle without it reads to iOS as a dimmed switch (Zucchero's
      // row 695).
      enabled: true,
      toggled: active,
      label: said,
      container: true,
      onTap: () => fireAndLog(backend.openPanel(agent.id, active ? '' : id)),
      excludeSemantics: true,
      child: Pressable(
        onTap: () => fireAndLog(backend.openPanel(agent.id, active ? '' : id)),
        builder: (context, hovered) {
          final color = active || hovered ? palette.text : palette.textMuted;
          return AnimatedContainer(
            duration: Motion.fast,
            constraints: BoxConstraints(
              minHeight: touch ? Metrics.of(context).target : 28,
              minWidth: touch ? Metrics.of(context).target : 28,
            ),
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            decoration: BoxDecoration(
              color: active
                  ? palette.selected
                  : hovered
                  ? palette.hover
                  : const Color(0x00000000),
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                ExtensionGlyph(
                  icon: extension.icon,
                  name: extension.name,
                  color: color,
                ),
                if (!compact) ...[
                  const SizedBox(width: Space.xs + 2),
                  Text(
                    words,
                    maxLines: 1,
                    style: type.label.copyWith(color: color),
                  ),
                ],
                if (number.isNotEmpty) ...[
                  const SizedBox(width: Space.xs + 2),
                  Text(
                    number,
                    maxLines: 1,
                    style: type.label.copyWith(
                      color: palette.textMuted,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                ],
              ],
            ),
          );
        },
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
      // The Workspace too, for the name of the agent it goes back to.
      listenable: Listenable.merge([history, scope.backend.workspace]),
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
            container: true,
            onTap: () => history.goBack(current),
            excludeSemantics: true,
            child: Pressable(
              onTap: () => history.goBack(current),
              builder: (context, hovered) => AnimatedContainer(
                duration: Motion.fast,
                // 28 tall, taller when large text needs it.
                constraints: const BoxConstraints(minHeight: 28, maxWidth: 180),
                padding: const EdgeInsets.only(
                  left: Space.s - 2,
                  right: Space.s + 2,
                  top: 2,
                  bottom: 2,
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
    final text = Text(
      label,
      maxLines: 1,
      softWrap: false,
      overflow: TextOverflow.ellipsis,
      style: TypeScale.of(context).caption.copyWith(
        color: palette.textMuted,
        fontFeatures: const [FontFeature.tabularFigures()],
      ),
    );
    return LayoutBuilder(
      builder: (context, constraints) {
        // The bar only where the words fit beside it.
        final painter = TextPainter(
          text: TextSpan(text: label, style: text.style),
          textScaler: MediaQuery.textScalerOf(context),
          textDirection: Directionality.of(context),
          maxLines: 1,
        )..layout();
        final words = painter.width;
        painter.dispose();
        final bar = limit > 0 && constraints.maxWidth >= 40 + Space.s + words;
        return Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (bar) ...[
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
            Flexible(child: text),
          ],
        );
      },
    );
  }
}

/// While the connection is down (after a second, so a quick reconnect
/// doesn't flash it): a quiet line above the message box saying it's
/// reconnecting and that what shows may be out of date. The backend's
/// snapshot brings the conversation up to date once it's back. Once its
/// tries fail (the machine off, asleep or offline), it says it can't
/// connect and what to do, as the Claktory's connection line does. A tap
/// (a click on a Mac) reconnects from scratch at once, as relaunching the
/// app would (#214).
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
  Listenable? _link;
  Timer? _wait;
  var _shown = false;

  /// The connection's state, while the line shows: failing says so.
  Connection? _connection;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (identical(backend, _backend)) return;
    _link?.removeListener(_changed);
    _backend = backend;
    _link = Connection.of(backend)..addListener(_changed);
    _changed();
  }

  @override
  void dispose() {
    _wait?.cancel();
    _link?.removeListener(_changed);
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
    final connection = Connection(backend);
    final was = _connection;
    _connection = connection;
    if (_shown &&
        (was?.link != connection.link || was?.details != connection.details)) {
      setState(() {});
    }
    _wait ??= Timer(ReconnectingLine.after, () {
      if (mounted) setState(() => _shown = true);
    });
  }

  /// Reconnects from scratch at once: the person tapped the line.
  void _reconnect() {
    if (touchPlatform) unawaited(HapticFeedback.selectionClick());
    _backend?.reconnectNow();
  }

  /// The line, [child], as one control: a finger's height on a phone, its
  /// words for VoiceOver, and a tap that reconnects.
  Widget _tappable(String label, Widget child, {bool live = false}) {
    final least = touchPlatform ? Touch.target : 0.0;
    return Semantics(
      button: true,
      liveRegion: live,
      label: label,
      hint: reconnectHint,
      onTap: _reconnect,
      excludeSemantics: true,
      child: Pressable(
        onTap: _reconnect,
        builder: (context, _) => ConstrainedBox(
          constraints: BoxConstraints(minHeight: least),
          child: Center(child: child),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    if (!_shown) return widget.otherwise;
    final palette = Palette.of(context);
    final connection = _connection;
    if (connection != null && connection.link == Link.failing) {
      return _failing(context, connection);
    }
    final text = reconnectingText(touch: touchPlatform);
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.xl, Space.s, Space.xl, 0),
      child: _tappable(
        text,
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            const LiveDot(),
            const SizedBox(width: Space.s),
            Flexible(
              child: Text(
                text,
                textAlign: TextAlign.center,
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textMuted),
              ),
            ),
          ],
        ),
      ),
    );
  }

  /// Its tries failing: "Can't connect to mac-mini" by a red dot, and what
  /// to do under it, as the connection line says them, ending with the
  /// tap that tries again.
  Widget _failing(BuildContext context, Connection connection) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final advice = tryAgainAdvice(connection.details, touch: touchPlatform);
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.xl, Space.s, Space.xl, 0),
      child: _tappable(
        '${connection.line}. $advice',
        live: true,
        Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Dot(color: palette.danger),
                const SizedBox(width: Space.s),
                Flexible(
                  child: Text(
                    connection.line,
                    textAlign: TextAlign.center,
                    style: type.caption.copyWith(
                      color: palette.text,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(height: Space.xs),
            Text(
              advice,
              textAlign: TextAlign.center,
              style: type.caption.copyWith(color: palette.textMuted),
            ),
          ],
        ),
      ),
    );
  }
}

/// Whether this is a touch device (iPhone, iPad, Android): "Tap", else
/// "Click".
bool get touchPlatform =>
    defaultTargetPlatform == TargetPlatform.iOS ||
    defaultTargetPlatform == TargetPlatform.android;

/// What [ReconnectingLine] says while it reconnects.
String reconnectingText({required bool touch}) =>
    'Reconnecting… ${touch ? 'Tap' : 'Click'} to try now.';

/// The connection's advice ([Connection.details]), its "Trying again…"
/// made the tap that tries again.
String tryAgainAdvice(String details, {required bool touch}) {
  final advice = details.replaceFirst(RegExp(r'\s*Trying again…$'), '');
  final cue = '${touch ? 'Tap' : 'Click'} to try again.';
  return advice.isEmpty ? cue : '$advice $cue';
}

/// VoiceOver's hint on [ReconnectingLine].
const reconnectHint = 'Tries to reconnect now';

/// While the agent works, a line above the message box at its end: the
/// live dot, "Working", and Stop (Esc on a Mac). Above the box rather than
/// in it, so the text being typed never moves as the agent starts and
/// stops. While usage limits hold it up, what it waits for instead, with
/// no dot: it isn't working meanwhile, and Try now beside Stop, to ask
/// again at once (after upgrading a plan, say), or Sign in… while it waits
/// for a sign-in.
class WorkingLine extends StatelessWidget {
  const WorkingLine({
    super.key,
    required this.onStop,
    this.wait = '',
    this.waiting = '',
  });

  final VoidCallback onStop;

  /// What it waits for (Agent.usage_wait); empty while it works.
  final String wait;

  /// Who waits, an agent's or a chat's id, for Try now; empty for none.
  final String waiting;

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
              final stop = Button(
                label: phone ? 'Stop' : 'Stop  esc',
                onPressed: onStop,
              );
              // Held up by usage: what it waits for, and what can be done
              // about it (#341).
              if (wait.isNotEmpty && waiting.isNotEmpty) {
                return UsageWaitActionLine(
                  wait: wait,
                  waiting: waiting,
                  stop: stop,
                );
              }
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
              final again = wait.isEmpty || waiting.isEmpty
                  ? null
                  : TryNow(wait: wait, waiting: waiting);
              // Where the words and buttons don't fit in a line (the
              // largest text, a narrow pane), the buttons go under the
              // words, still at the end, so neither is squeezed.
              return ActionLine(
                lead: words,
                // Try now over Stop where even the whole line can't hold
                // both (a small phone at the largest text).
                action: ControlsLine(children: [?again, stop]),
              );
            },
          ),
        ),
      ),
    );
  }
}

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
