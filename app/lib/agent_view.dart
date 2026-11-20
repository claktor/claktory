import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_name.dart';
import 'attachments.dart';
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'drafts.dart';
import 'forms.dart';
import 'identity.dart';
import 'panes.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'settings_view.dart';
import 'transcript.dart';
import 'tell_rivet.dart';
import 'usage_wait.dart';

/// The wash behind [agent]'s conversation, in its color.
Color conversationTint(Agent agent) =>
    agentColor(agent.color).withValues(alpha: conversationTintAlpha);

/// The selected agent: its header, transcript and composer. Esc stops the
/// agent while it works.
class AgentView extends StatefulWidget {
  const AgentView({super.key, required this.agent, this.onBack, this.onMore});

  final Agent agent;

  /// On a phone: back to the list of agents, and the agent's extensions.
  /// Its header then leads with a back button and ends with "…".
  final VoidCallback? onBack;
  final VoidCallback? onMore;

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

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final transcript = backend.transcript;
    final agent = widget.agent;
    // The conversation: one of the agent's panes.
    final chat = Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
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
            open: (id) => AppScope.scope(context).history.follow(agent.id, id),
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
        ArchivedLine(agent: agent),
        // One line above the box, so the box never changes as the agent
        // starts and stops: reconnecting, else working with Stop.
        ReconnectingLine(
          otherwise: _working
              ? WorkingLine(onStop: _stop, wait: usageWaitLine(agent))
              : const SizedBox.shrink(),
        ),
        _Composer(agent: agent, onSent: () => _follow.value++, onStop: _stop),
      ],
    );
    // Files dragged over the window go to this agent: the pane says so.
    final chatWithDrop = Stack(
      children: [
        Positioned.fill(child: chat),
        Positioned.fill(child: FileDropOverlay(agent: agent)),
      ],
    );
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): _stop},
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _Header(agent: agent, onBack: widget.onBack, onMore: widget.onMore),
          // A phone shows one pane at a time: the conversation is the
          // screen, and its viewers open from "…".
          Expanded(
            child: widget.onBack != null
                ? chatWithDrop
                : PaneLayout(agent: agent, chat: chatWithDrop),
          ),
        ],
      ),
    );
  }
}

/// Over an agent's conversation while files are dragged over the window
/// (they'll go to its message box): the pane dims, its edge takes the
/// agent's color, and a card in the middle says where they go and how
/// many. It never takes the pointer; the window's drag is native, so it
/// doesn't flicker as the pointer crosses what's under it.
class FileDropOverlay extends StatefulWidget {
  const FileDropOverlay({super.key, required this.agent});

  final Agent agent;

  @override
  State<FileDropOverlay> createState() => _FileDropOverlayState();
}

class _FileDropOverlayState extends State<FileDropOverlay> {
  /// How many files it last said, kept while it fades out.
  var _count = 0;

  /// In the tree: shown, or fading out.
  var _present = false;

  @override
  Widget build(BuildContext context) => IgnorePointer(
    child: ValueListenableBuilder<int>(
      valueListenable: FileDrop.count,
      builder: (context, count, _) {
        final shown = count > 0 && widget.agent.modelAvailable;
        if (shown) {
          _count = count;
          _present = true;
        }
        if (!_present) return const SizedBox.shrink();
        return TweenAnimationBuilder<double>(
          tween: Tween(begin: 0, end: shown ? 1 : 0),
          duration: Motion.fast,
          curve: Curves.easeOut,
          onEnd: () {
            if (!shown && mounted) setState(() => _present = false);
          },
          builder: (context, t, child) => Opacity(opacity: t, child: child),
          child: _card(context, _count),
        );
      },
    ),
  );

  Widget _card(BuildContext context, int count) {
    final palette = Palette.of(context);
    final agent = widget.agent;
    final color = agentColor(agent.color);
    return Container(
      decoration: BoxDecoration(
        color: const Color(0x4D000000),
        border: Border.all(color: color, width: 2),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      alignment: Alignment.center,
      child: Container(
        padding: const EdgeInsets.symmetric(
          horizontal: Space.xl,
          vertical: Space.l,
        ),
        decoration: BoxDecoration(
          color: palette.sidebar,
          border: Border.all(color: palette.hairline),
          borderRadius: BorderRadius.circular(Radii.floating),
          boxShadow: floatingShadow,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 44,
              height: 44,
              decoration: BoxDecoration(
                color: color.withValues(alpha: 0.14),
                shape: BoxShape.circle,
              ),
              alignment: Alignment.center,
              child: ExtensionGlyph(
                icon: 'upload',
                name: 'Upload',
                color: color,
                size: 22,
              ),
            ),
            const SizedBox(height: Space.m),
            Text(
              'Drop to add to your message to ${agent.name}',
              style: TextStyles.emphasis.copyWith(color: palette.text),
            ),
            if (count > 1)
              Text(
                '$count files',
                style: TextStyles.caption.copyWith(color: palette.textMuted),
              ),
          ],
        ),
      ),
    );
  }
}

/// The agent's bar, in its color: its initial, name, model · effort and
/// context use. Opens the agent's settings. It's the top of the window, so
/// it also drags it.
class _Header extends StatelessWidget {
  const _Header({required this.agent, this.onBack, this.onMore});

  final Agent agent;
  final VoidCallback? onBack;
  final VoidCallback? onMore;

  bool get _phone => onBack != null;

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
            padding: EdgeInsets.symmetric(
              horizontal: _phone ? Space.xs : Space.xl,
            ),
            color: hovered ? palette.hover : null,
            child: Row(
              children: [
                if (_phone) ...[
                  GlyphButton(
                    label: 'Back',
                    active: false,
                    onTap: onBack!,
                    glyph: (color) => ExtensionGlyph(
                      icon: 'chevron-left',
                      name: 'Back',
                      color: color,
                    ),
                  ),
                  const SizedBox(width: Space.xs),
                ] else
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
                          style: TextStyles.label.copyWith(
                            color: palette.text,
                            fontWeight: FontWeight.w600,
                          ),
                          roleColor: palette.textMuted,
                        ),
                      ),
                      // A phone has room for the name alone.
                      if (!_phone) ...[
                        const SizedBox(width: Space.m),
                        Flexible(
                          child: Text(
                            details,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyles.body.copyWith(
                              color: palette.textMuted,
                            ),
                          ),
                        ),
                      ],
                      if (!agent.modelAvailable) ...[
                        const SizedBox(width: Space.s),
                        Button(
                          label: 'Sign in…',
                          onPressed: () => AppScope.openSettings(context),
                        ),
                      ],
                    ],
                  ),
                ),
                if (_phone) ...[
                  const SizedBox(width: Space.s),
                  GlyphButton(
                    label: 'Extensions',
                    active: false,
                    onTap: onMore!,
                    glyph: (color) => ExtensionGlyph(
                      icon: 'ellipsis',
                      name: 'Extensions',
                      color: color,
                    ),
                  ),
                ] else ...[
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
                        style: TextStyles.label.copyWith(color: palette.text),
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
          style: TextStyles.caption.copyWith(
            color: palette.textMuted,
            fontFeatures: const [FontFeature.tabularFigures()],
          ),
        ),
      ],
    );
  }
}

/// Multi-line input. Enter sends, Shift+Enter adds a line. Always enabled:
/// messages sent while the agent works wait for the current run.
class _Composer extends StatefulWidget {
  const _Composer({
    required this.agent,
    required this.onSent,
    required this.onStop,
  });

  final Agent agent;
  final VoidCallback onSent;
  final VoidCallback onStop;

  @override
  State<_Composer> createState() => _ComposerState();
}

class _ComposerState extends State<_Composer> {
  late final Drafts _drafts;

  /// Starts from the agent's draft, which it keeps up to date.
  late final TextEditingController _text;

  @override
  void initState() {
    super.initState();
    _drafts = AppScope.scope(context).drafts;
    _text = TextEditingController.fromValue(_draftValue(widget.agent.id))
      ..addListener(_typed);
    // A message started elsewhere (the sidebar's "+") lands here; the box
    // has the focus already, as it autofocuses.
    _drafts.takeFocus(widget.agent.id);
    _drafts.addListener(_draftChanged);
    // Files dropped on the window come here, for this agent.
    _drops = FileDrop.dropped.listen(_attach);
    unawaited(FileDrop.accept(true));
  }

  StreamSubscription<List<String>>? _drops;

  /// Puts the files at [paths] in this agent's box, saying why any couldn't
  /// go.
  Future<void> _attach(List<String> paths) async {
    if (paths.isEmpty || !widget.agent.modelAvailable) return;
    final left = await AppScope.of(context).attachments
        .add(widget.agent.id, paths);
    if (!mounted) return;
    setState(
      () => _error = left == 0
          ? null
          : left == paths.length && left == 1
          ? "Can't attach this: it's a folder or can't be read."
          : "Some items weren't attached: folders, and files that can't "
                'be read.',
    );
    _focus.requestFocus();
  }

  /// The paperclip: the system's picker for files (and photos, on iPhone).
  Future<void> _pick() async {
    final paths = defaultTargetPlatform == TargetPlatform.iOS
        ? await pickAttachments()
        : await pickFiles();
    await _attach(paths);
  }

  /// The draft changed from outside the box: a message started for this
  /// agent elsewhere.
  void _draftChanged() {
    final agentId = widget.agent.id;
    if (_drafts.of(agentId) != _text.text) _text.value = _draftValue(agentId);
    if (_drafts.takeFocus(agentId)) {
      _text.selection = TextSelection.collapsed(offset: _text.text.length);
      _focus.requestFocus();
    }
  }

  late final _focus = FocusNode(onKeyEvent: _onKey);
  String? _error;

  /// The slash-command menu, over the top of the box.
  final _menu = OverlayPortalController();
  final _box = LayerLink();

  /// The menu's highlighted command, among [_matches].
  var _highlighted = 0;

  /// Esc closed the menu; it stays closed until the text changes.
  var _dismissed = false;
  var _lastText = '';

  /// The commands matching what's typed: text starting with "/" and no
  /// space yet, each command whose name starts with what follows it.
  List<SlashCommand> get _matches {
    final text = _text.text;
    if (_dismissed || !text.startsWith('/') || text.contains(RegExp(r'\s'))) {
      return const [];
    }
    final typed = text.substring(1).toLowerCase();
    return [
      for (final command in widget.agent.commands)
        if (command.name.toLowerCase().startsWith(typed)) command,
    ];
  }

  /// The agent's draft, with the cursor at its end.
  TextEditingValue _draftValue(String agentId) {
    final draft = _drafts.of(agentId);
    return TextEditingValue(
      text: draft,
      selection: TextSelection.collapsed(offset: draft.length),
    );
  }

  void _typed() {
    _drafts.set(widget.agent.id, _text.text);
    if (_text.text == _lastText) return;
    _lastText = _text.text;
    _dismissed = false;
    _highlighted = 0;
    _syncMenu();
  }

  void _syncMenu() {
    final show = _matches.isNotEmpty;
    if (show && !_menu.isShowing) _menu.show();
    if (!show && _menu.isShowing) _menu.hide();
    if (mounted) setState(() {});
  }

  /// The command typed out in full, "/name" or "/name args", or null.
  (SlashCommand, String)? _typedCommand() {
    final match = RegExp(r'^/(\S+)(?:\s+([\s\S]*))?$')
        .firstMatch(_text.text.trim());
    if (match == null) return null;
    for (final command in widget.agent.commands) {
      if (command.name == match.group(1)) {
        return (command, (match.group(2) ?? '').trim());
      }
    }
    return null;
  }

  Future<void> _run(SlashCommand command, String args) async {
    final agentId = widget.agent.id;
    final typed = _text.text;
    final backend = AppScope.of(context);
    _text.clear();
    setState(() => _error = null);
    try {
      await backend.runCommand(agentId, command.name, args);
    } on RequestFailure catch (failure) {
      if (!mounted) return _giveBack(agentId, typed);
      // Give it back, so it can be fixed and tried again.
      if (_text.text.isEmpty && widget.agent.id == agentId) _text.text = typed;
      setState(() => _error = '/${command.name}: ${failure.message}');
    }
  }

  @override
  void didUpdateWidget(_Composer old) {
    super.didUpdateWidget(old);
    if (old.agent.id != widget.agent.id) {
      _text.value = _draftValue(widget.agent.id);
      _error = null;
      _focus.requestFocus();
    }
  }

  @override
  void dispose() {
    unawaited(_drops?.cancel());
    unawaited(FileDrop.accept(false));
    _drafts.removeListener(_draftChanged);
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    // Enter while an input method is composing confirms the composition.
    final composing = _text.value.composing;
    if (composing.isValid && !composing.isCollapsed) {
      return KeyEventResult.ignored;
    }
    final matches = _matches;
    if (matches.isNotEmpty) {
      final count = matches.length;
      if (key == LogicalKeyboardKey.arrowDown) {
        setState(() => _highlighted = (_highlighted + 1) % count);
        return KeyEventResult.handled;
      }
      if (key == LogicalKeyboardKey.arrowUp) {
        setState(() => _highlighted = (_highlighted - 1 + count) % count);
        return KeyEventResult.handled;
      }
      if (key == LogicalKeyboardKey.escape) {
        _dismissed = true;
        _syncMenu();
        return KeyEventResult.handled;
      }
      if (key == LogicalKeyboardKey.tab) {
        // Complete the name, to type what follows it.
        final name = matches[_highlighted.clamp(0, count - 1)].name;
        _text.value = TextEditingValue(
          text: '/$name ',
          selection: TextSelection.collapsed(offset: name.length + 2),
        );
        return KeyEventResult.handled;
      }
    }
    if (key != LogicalKeyboardKey.enter ||
        HardwareKeyboard.instance.isShiftPressed) {
      return KeyEventResult.ignored;
    }
    if (matches.isNotEmpty) {
      _run(matches[_highlighted.clamp(0, matches.length - 1)], '');
    } else if (_typedCommand() case (final command, final args)) {
      _run(command, args);
    } else {
      _send();
    }
    return KeyEventResult.handled;
  }

  /// The menu of matching commands, just above the box and as wide.
  Widget _buildMenu(BuildContext context, double width) {
    final palette = Palette.of(context);
    final matches = _matches;
    return CompositedTransformFollower(
      link: _box,
      targetAnchor: Alignment.topLeft,
      followerAnchor: Alignment.bottomLeft,
      offset: const Offset(0, -Space.xs),
      child: Align(
        alignment: Alignment.bottomLeft,
        child: Container(
          width: width,
          constraints: const BoxConstraints(maxHeight: 280),
          padding: const EdgeInsets.all(Space.xs),
          decoration: BoxDecoration(
            color: palette.sidebar,
            borderRadius: BorderRadius.circular(Radii.panel),
            border: Border.all(color: palette.hairline),
            boxShadow: floatingShadow,
          ),
          child: ListView(
            shrinkWrap: true,
            children: [
              for (var i = 0; i < matches.length; i++)
                MouseRegion(
                  cursor: SystemMouseCursors.click,
                  onEnter: (_) => setState(() => _highlighted = i),
                  child: GestureDetector(
                    onTap: () => _run(matches[i], ''),
                    child: Container(
                      padding: const EdgeInsets.symmetric(
                        horizontal: Space.s,
                        vertical: Space.xs + 2,
                      ),
                      decoration: BoxDecoration(
                        color: i == _highlighted
                            ? palette.selected
                            : const Color(0x00000000),
                        borderRadius: BorderRadius.circular(Radii.control),
                      ),
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            '/${matches[i].name}',
                            style: TextStyles.label.copyWith(
                              color: palette.text,
                            ),
                          ),
                          if (matches[i].argsHint.isNotEmpty) ...[
                            const SizedBox(width: Space.xs),
                            Text(
                              matches[i].argsHint,
                              style: TextStyles.body.copyWith(
                                color: palette.textFaint,
                              ),
                            ),
                          ],
                          const SizedBox(width: Space.m),
                          Expanded(
                            child: Text(
                              matches[i].description,
                              style: TextStyles.body.copyWith(
                                color: palette.textMuted,
                              ),
                            ),
                          ),
                        ],
                      ),
                    ),
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }

  /// A message or command that failed after its box was gone: back to its
  /// draft, unless something new was typed there since.
  void _giveBack(String agentId, String text) {
    if (_drafts.of(agentId).isEmpty) _drafts.set(agentId, text);
  }

  Future<void> _send() async {
    final text = _text.text.trim();
    final agentId = widget.agent.id;
    final backend = AppScope.of(context);
    // Files can go on their own.
    if (text.isEmpty && backend.attachments.of(agentId).isEmpty) return;
    _text.clear();
    setState(() => _error = null);
    widget.onSent();
    // It shows in the conversation at once, its files uploading; one that
    // isn't sent stays there, with Retry.
    backend.outbox.send(
      agentId,
      text,
      files: backend.attachments.take(agentId),
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final agent = widget.agent;
    final hasModels = AppScope.of(context)
        .workspace
        .workspace
        .models
        .isNotEmpty;
    // A phone has no Enter key to send with, nor room to spare.
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
            phone ? Space.s : Space.l,
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (_error != null) _Note(_error!, color: palette.danger),
              LayoutBuilder(
                builder: (context, constraints) => CompositedTransformTarget(
                  link: _box,
                  child: OverlayPortal(
                    controller: _menu,
                    overlayChildBuilder: (context) =>
                        _buildMenu(context, constraints.maxWidth),
                    child: ValueListenableBuilder(
                      valueListenable: FileDrop.over,
                      builder: (context, filesOver, child) => AnimatedContainer(
                        duration: Motion.fast,
                        // Hugs its content: one row, as tall as one line
                        // (or a button) when empty, growing with the text.
                        padding: const EdgeInsets.fromLTRB(
                          Space.m,
                          Space.xs,
                          Space.xs,
                          Space.xs,
                        ),
                        decoration: BoxDecoration(
                          color: palette.sidebar,
                          borderRadius: BorderRadius.circular(Radii.panel),
                          // Files dragged over the window land here.
                          border: Border.all(
                            color: filesOver && agent.modelAvailable
                                ? palette.signal
                                : palette.hairline,
                          ),
                        ),
                        child: child,
                      ),
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          _Attachments(agentId: agent.id),
                          ConstrainedBox(
                            constraints: const BoxConstraints(
                              minHeight: _composerRow,
                            ),
                            child: Row(
                              crossAxisAlignment: CrossAxisAlignment.end,
                              children: [
                                Expanded(
                                  child: agent.modelAvailable
                                      ? Container(
                                          constraints: const BoxConstraints(
                                            minHeight: _composerRow,
                                          ),
                                          alignment: Alignment.centerLeft,
                                          padding: const EdgeInsets.symmetric(
                                            vertical: 4.5,
                                          ),
                                          child: TextField(
                                            controller: _text,
                                            focusNode: _focus,
                                            placeholder:
                                                agent.placeholder.isEmpty
                                                ? 'Message ${agent.name}'
                                                : agent.placeholder,
                                            // On a phone the keyboard would
                                            // cover the conversation at once.
                                            autofocus: !phone,
                                            minLines: 1,
                                            maxLines: 12,
                                            bordered: false,
                                          ),
                                        )
                                      // Sending couldn't work, so don't offer to.
                                      : SizedBox(
                                          height: _composerRow,
                                          child: Align(
                                            alignment: Alignment.centerLeft,
                                            child: Text(
                                              "No model available, so "
                                              "${agent.name} can't run.",
                                              maxLines: 1,
                                              overflow: TextOverflow.ellipsis,
                                              style: TextStyles.body.copyWith(
                                                color: palette.textMuted,
                                              ),
                                            ),
                                          ),
                                        ),
                                ),
                                // At the end of the row, level with the last
                                // line: what it's doing, and what can be done.
                                SizedBox(
                                  height: _composerRow,
                                  child: Row(
                                    mainAxisSize: MainAxisSize.min,
                                    children: [
                                      if (agent.modelAvailable) ...[
                                        const SizedBox(width: Space.xs),
                                        GlyphButton(
                                          label: 'Attach files',
                                          active: false,
                                          onTap: _pick,
                                          glyph: (color) => ExtensionGlyph(
                                            icon: 'paperclip',
                                            name: 'Attach files',
                                            color: color,
                                          ),
                                        ),
                                      ],
                                      if (phone && agent.modelAvailable) ...[
                                        const SizedBox(width: Space.s),
                                        ListenableBuilder(
                                          listenable: Listenable.merge([
                                            _text,
                                            AppScope.of(context).attachments,
                                          ]),
                                          builder: (context, _) => Button(
                                            label: 'Send',
                                            primary: true,
                                            onPressed:
                                                _text.text.trim().isEmpty &&
                                                    AppScope.of(context)
                                                        .attachments
                                                        .of(agent.id)
                                                        .isEmpty
                                                ? null
                                                : _send,
                                          ),
                                        ),
                                      ],
                                      if (!agent.modelAvailable) ...[
                                        if (hasModels) ...[
                                          const SizedBox(width: Space.s),
                                          Button(
                                            label: 'Change model…',
                                            onPressed: () => showModal(
                                              context,
                                              (context, close) =>
                                                  AgentSettingsForm(
                                                    agent: agent,
                                                    close: close,
                                                  ),
                                            ),
                                          ),
                                        ],
                                        const SizedBox(width: Space.xs),
                                        Button(
                                          label: 'Sign in…',
                                          primary: true,
                                          onPressed: () =>
                                              AppScope.openSettings(context),
                                        ),
                                      ],
                                    ],
                                  ),
                                ),
                              ],
                            ),
                          ),
                        ],
                      ),
                    ),
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

/// The files in an agent's message box, as chips over its text, each with
/// × to take it out.
class _Attachments extends StatelessWidget {
  const _Attachments({required this.agentId});

  final String agentId;

  @override
  Widget build(BuildContext context) {
    final attachments = AppScope.of(context).attachments;
    return ListenableBuilder(
      listenable: attachments,
      builder: (context, _) {
        final files = attachments.of(agentId);
        if (files.isEmpty) return const SizedBox.shrink();
        return Padding(
          padding: const EdgeInsets.only(top: 2, right: Space.xs, bottom: 2),
          child: Wrap(
            spacing: Space.xs,
            runSpacing: Space.xs,
            children: [
              for (final file in files)
                AttachmentChip(
                  name: file.name,
                  size: file.size,
                  onRemove: () => attachments.remove(agentId, file),
                ),
            ],
          ),
        );
      },
    );
  }
}

/// The height of the message box's row: a button's, which also fits one
/// line of text with room around it.
const _composerRow = 28.0;

class _Note extends StatelessWidget {
  const _Note(this.text, {required this.color});

  final String text;
  final Color color;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.s, left: Space.xs),
      child: ErrorLine(text, style: TextStyles.caption.copyWith(color: color)),
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
              style: TextStyles.caption.copyWith(color: palette.textMuted),
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
          child: Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              if (wait.isEmpty) ...[
                const LiveDot(),
                const SizedBox(width: Space.s),
              ],
              Flexible(
                child: Text(
                  wait.isEmpty ? 'Working' : wait,
                  style: TextStyles.caption.copyWith(color: palette.textMuted),
                ),
              ),
              const SizedBox(width: Space.s),
              Button(label: phone ? 'Stop' : 'Stop  esc', onPressed: onStop),
            ],
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
                style: TextStyles.caption.copyWith(color: palette.textMuted),
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
                style: TextStyles.label.copyWith(color: palette.text),
              ),
              if (agent.role.isNotEmpty)
                Text(
                  agent.role,
                  textAlign: TextAlign.center,
                  style: TextStyles.caption.copyWith(color: palette.textMuted),
                ),
              const SizedBox(height: Space.s),
              Text(
                emptyConversationHint(name),
                textAlign: TextAlign.center,
                style: TextStyles.body.copyWith(color: palette.textMuted),
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
