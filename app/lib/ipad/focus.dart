// Needs you's focus mode on an iPad (#514; the spec's §6): the Mac's
// queue, stage and Context (needs_you_focus.dart), laid out for the
// window's tier (focus_columns.dart), at a finger's sizes, with focus
// mode's ⌘-commands in the system's menu.
//
// Nothing here keeps a card, a draft or whether Context is open: the
// session does, above this, so a resize, a rotation or Split View only
// lays the same things out again.
import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/scheduler.dart' show SchedulerPhase;
import 'package:flutter/services.dart' show HardwareKeyboard, KeyDownEvent;
import 'package:flutter/widgets.dart';

import '../controls.dart' show Button, ExtensionGlyph;

import '../identity.dart';
import '../layout.dart' show viewerKindFor;
import '../protocol/factory/v1/factory.pb.dart' show Entry, ViewLeaf, ViewNode;
import '../inbox.dart' show NeedsYouItem;
import '../mobile/device.dart' show WindowControls, windowControls;
import '../mobile/metrics.dart' show TypeScale;
import '../mobile/needs_you_phone.dart' show PhoneNeedsYou;
import '../needs_you_focus.dart';
import '../voice_session.dart' show activeVoice;
import 'commands.dart';
import 'document_host.dart'
    show DocumentHost, DocumentSurfaces, documentKind, imageKind;
import 'columns.dart' show textFactorOf;
import 'focus_columns.dart';
import 'workbench.dart';

/// Focus mode over an iPad's window: [session]'s queue, card and Context
/// in [focusColumnsFor]'s composition; [leave] goes back to the window as
/// it was.
class IPadFocus extends StatefulWidget {
  const IPadFocus({super.key, required this.session, required this.leave});

  final FocusSession session;
  final VoidCallback leave;

  @override
  State<IPadFocus> createState() => _IPadFocusState();
}

class _IPadFocusState extends State<IPadFocus> {
  /// The queue as a popover from "3 of 7", where the stage is alone.
  var _queueOpen = false;

  FocusSession get _session => widget.session;

  /// Whether Context had a column of its own when focus mode was last laid
  /// out; null before the first time. It's shown by default only there
  /// (Bruno's Q2): entering in a narrower window, or narrowing to one, it
  /// closes rather than cover the card; opened over the card, it stays
  /// open through a resize (spec §6.2). Closed so by narrowing, it opens
  /// again as the window widens back.
  bool? _contextBeside;
  var _contextFolded = false;

  @override
  void initState() {
    super.initState();
    _session.addListener(_changed);
    _session.answer.addListener(_changed);
    // The menu's ⌘⌫, ⌥⌘↑/↓ and ⌘⇧C stand down while a field has the keys
    // or a recording is on, so their keys reach the field (§6.3 rule 3).
    FocusManager.instance.addListener(_changed);
    activeVoice.addListener(_changed);
    windowControls.addListener(_changed);
  }

  @override
  void didUpdateWidget(IPadFocus old) {
    super.didUpdateWidget(old);
    if (identical(old.session, widget.session)) return;
    old.session.removeListener(_changed);
    old.session.answer.removeListener(_changed);
    _session.addListener(_changed);
    _session.answer.addListener(_changed);
  }

  @override
  void dispose() {
    _session.removeListener(_changed);
    _session.answer.removeListener(_changed);
    FocusManager.instance.removeListener(_changed);
    activeVoice.removeListener(_changed);
    windowControls.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (!mounted) return;
    // FocusManager tells in the middle of a build sometimes.
    if (WidgetsBinding.instance.schedulerPhase ==
        SchedulerPhase.persistentCallbacks) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _changed());
      return;
    }
    setState(() {
      final current = _session.current?.key;
      if (current != _shown) _queueOpen = false;
      _shown = current;
    });
  }

  /// The card on the stage when last built: a row picked in the queue's
  /// popover changes it, and closes the popover.
  String? _shown;

  /// Focus mode's commands with modifiers that only the system's menu
  /// would run otherwise, bound here too: its key commands don't fire in
  /// the Simulator (Zucchero's row 701), and a press UIKit runs never
  /// reaches Flutter, nor one Flutter has the menu again (CommandMenu).
  /// ⌘↩ and ⌘⌫ are FocusKeys' (held, they record); ⌘⇧N FocusModeLayer's.
  static const _bound = {
    'focus.next',
    'focus.previous',
    'focus.context',
    'focus.go',
  };

  /// One of [_bound]'s keys going down runs it as it stands now, asked as
  /// the key comes (never in a build); one that can't run now (a field has
  /// the keys, no card) goes on to what has the keys.
  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent || !mounted) return KeyEventResult.ignored;
    for (final command in focusCommands(
      context,
      _session,
      leave: widget.leave,
    )) {
      if (!_bound.contains(command.id) ||
          !command.activator.accepts(event, HardwareKeyboard.instance)) {
        continue;
      }
      final run = command.run;
      if (run == null) return KeyEventResult.ignored;
      run();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final focus = WorkbenchScope.of(context);
    final menu = focus.menu;
    // The system's menu lists them as they are now: titled with the
    // card's own yes and no, disabled while they can't run. Read after
    // the frame: whether a field has the keys asks the focused widget,
    // which a window built in place of another (a resize across the
    // phone's layout) may still find in the tree going away.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      final commands = [
        for (final command in focusCommands(
          context,
          _session,
          leave: widget.leave,
        ))
          WorkbenchCommand(
            id: command.id,
            title: command.title,
            keys: command.activator,
            run: command.run,
          ),
      ];
      unawaited(menu.publish(commands));
    });
    final palette = Palette.of(context);
    final tier = windowTier(context);
    final keys = FocusKeys(
      session: _session,
      leave: widget.leave,
      // ⌘↩ and ⌘⌫ stay here, the system's menu or not (row 701).
      child: ColoredBox(
        color: palette.canvas,
        child: SafeWindow(
          controls: windowControls.value,
          child: LayoutBuilder(
            builder: (context, constraints) => _layout(
              context,
              focusColumnsFor(
                tier,
                constraints.maxWidth,
                textFactor: textFactorOf(
                  MediaQuery.textScalerOf(context).scale,
                ),
              ),
              constraints,
            ),
          ),
        ),
      ),
    );
    return Focus(
      canRequestFocus: false,
      skipTraversal: true,
      onKeyEvent: _key,
      child: keys,
    );
  }

  Widget _layout(
    BuildContext context,
    FocusColumns columns,
    BoxConstraints constraints,
  ) {
    final palette = Palette.of(context);
    var contextOpen = _session.contextOpen && _session.current != null;
    final wasBeside = _contextBeside ?? true;
    if (wasBeside != columns.contextBeside) {
      _contextBeside = columns.contextBeside;
      if (!columns.contextBeside && contextOpen) {
        // Closed from this first frame on; the session hears it after.
        contextOpen = false;
        _contextFolded = true;
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted) _session.contextOpen = false;
        });
      } else if (columns.contextBeside && _contextFolded) {
        _contextFolded = false;
        contextOpen = _session.current != null;
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted) _session.contextOpen = true;
        });
      }
    }
    switch (columns.composition) {
      case FocusComposition.columns:
        return Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            FocusQueue(
              session: _session,
              width: columns.queue,
              leave: widget.leave,
            ),
            Expanded(
              child: FocusStage(
                session: _session,
                leave: widget.leave,
                // A finger has no C: the button shows and hides it.
                showContextButton: true,
                reviewBody: _reviewBody,
              ),
            ),
            if (contextOpen)
              Container(
                width: columns.context,
                decoration: BoxDecoration(
                  border: Border(left: BorderSide(color: palette.hairline)),
                ),
                child: FocusContext(session: _session),
              ),
          ],
        );
      case FocusComposition.contextOver:
        return Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            FocusQueue(
              session: _session,
              width: columns.queue,
              leave: widget.leave,
            ),
            Expanded(
              child: _OverStage(
                stage: FocusStage(
                  session: _session,
                  leave: widget.leave,
                  showContextButton: true,
                  reviewBody: _reviewBody,
                ),
                panel: contextOpen ? FocusContext(session: _session) : null,
                panelWidth: columns.contextPanel,
                // Under the stage's header: its Context and Go to
                // conversation stay in reach.
                panelTop: _stageHeader,
                title: 'Context',
                onDismiss: () => _session.contextOpen = false,
              ),
            ),
          ],
        );
      case FocusComposition.phone:
        // Slide Over, ⅓ Split View, a small window: the phone's own
        // one-card screen (Pesci's, spec §7), on this sitting so the card
        // stays the same through a resize, with the iPad's keys and menu
        // (above) and its Pencil on a review. It's laid out as a phone's
        // screen is: under the window's controls, above the home
        // indicator or the keyboard, which it then doesn't see again.
        // Context (a tap on who asks, or ⌘⇧C) slides over it, as in a
        // narrow window.
        final media = MediaQuery.of(context);
        final phone = PhoneNeedsYou(
          session: _session,
          onClose: widget.leave,
          reviewBody: _reviewBody,
          // Who asks opens Context over the screen, as ⌘⇧C does.
          onContext: () => _session.contextOpen = true,
        );
        return Padding(
          padding: EdgeInsets.only(
            top: _controlsTop(windowControls.value),
            bottom: math.max(media.padding.bottom, keyboardOverWindow(context)),
          ),
          child: MediaQuery(
            data: media.removePadding(removeBottom: true),
            child: _OverStage(
              stage: phone,
              panel: contextOpen ? FocusContext(session: _session) : null,
              panelWidth: columns.contextPanel,
              title: 'Context',
              onDismiss: () => _session.contextOpen = false,
            ),
          ),
        );
      case FocusComposition.stageOnly:
        // The stage alone ("3 of 7" and ‹ › in its header, the queue a
        // popover from the count).
        final controls = windowControls.value;
        final stage = FocusStage(
          session: _session,
          leave: widget.leave,
          showContextButton: true,
          reviewBody: _reviewBody,
          // No queue beside it: Done stays at the header's end.
          done: true,
          trailing: [
            _Position(session: _session, onTap: _toggleQueue),
            _Step(
              label: 'Previous request',
              icon: 'chevron-up',
              onTap: _session.waiting.length > 1 ? _session.previous : null,
            ),
            _Step(
              label: 'Next request',
              icon: 'chevron-down',
              onTap: _session.waiting.length > 1 ? _session.next : null,
            ),
          ],
        );
        return Padding(
          // The window's own controls sit over the top left: the stage's
          // header starts under them.
          padding: EdgeInsets.only(top: _controlsTop(controls)),
          child: _OverStage(
            stage: stage,
            panel: _queueOpen
                ? FocusQueue(
                    session: _session,
                    width: math.min(
                      FocusGeometry.queuePopover,
                      constraints.maxWidth - 2 * Space.m,
                    ),
                    onPicked: () => setState(() => _queueOpen = false),
                  )
                : contextOpen
                ? FocusContext(session: _session)
                : null,
            panelWidth: _queueOpen
                ? math.min(
                    FocusGeometry.queuePopover,
                    constraints.maxWidth - 2 * Space.m,
                  )
                : columns.contextPanel,
            // The queue drops from the count, at the header's right; Context
            // slides over from the right edge, as wider.
            panelTop: _queueOpen ? titleBarHeight + Space.l : _stageHeader,
            title: _queueOpen || !contextOpen ? null : 'Context',
            onDismiss: () {
              if (_queueOpen) {
                setState(() => _queueOpen = false);
              } else {
                _session.contextOpen = false;
              }
            },
          ),
        );
    }
  }

  void _toggleQueue() => setState(() => _queueOpen = !_queueOpen);

  /// The stage's header at its least height (FocusStage's): Context over
  /// the stage starts under it. Taller at a large text size, where
  /// Context's own close button is the way out.
  static const _stageHeader = titleBarHeight + Space.l;

  /// A PDF or a picture a card asks to have reviewed, on the stage in the
  /// document host, to draw on with Pencil, with the iPad's review bar
  /// under it as the card's answer (spec §6.4): it answers the card
  /// itself. Anything else is the stage's own.
  FocusReviewBody? _reviewBody(
    BuildContext context,
    NeedsYouItem item,
    Entry card,
  ) {
    final path = card.askPerson.reviewPath;
    final kind = viewerKindFor(path);
    if (item.agentId.isEmpty || (kind != documentKind && kind != imageKind)) {
      return null;
    }
    final leaf = ViewNode(
      // No pane of the agent's: its own surface, for this card.
      id: '${DocumentSurfaces.focusLeaf}${card.seq}',
      leaf: ViewLeaf(
        kind: kind,
        path: path,
        title: path.split('/').where((part) => part.isNotEmpty).lastOrNull,
      ),
    );
    return FocusReviewBody(
      DocumentHost.forLeaf(
        context,
        leaf,
        agentId: item.agentId,
        session: _session,
      ),
      answers: true,
    );
  }

  /// How far the window's controls reach down, where they sit over the
  /// stage's header (Stage Manager, a window).
  static double _controlsTop(WindowControls controls) =>
      controls.leading > 0 ? controls.top : 0;
}

/// [stage], with [panel] over its right side when there is one: a tap on
/// the stage around it ([onDismiss]) closes it, as the workbench's
/// sidebar overlay does. A [title]d panel (Context) has a header of its
/// own with a close button, so a finger or VoiceOver has a way out that
/// isn't a tap beside it (Zucchero's row 666).
class _OverStage extends StatelessWidget {
  const _OverStage({
    required this.stage,
    required this.panel,
    required this.panelWidth,
    required this.onDismiss,
    this.panelTop = 0,
    this.title,
  });

  final Widget stage;
  final Widget? panel;
  final double panelWidth;
  final double panelTop;
  final VoidCallback onDismiss;
  final String? title;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final panel = this.panel;
    final title = this.title;
    return Stack(
      children: [
        Positioned.fill(child: stage),
        if (panel != null) ...[
          Positioned.fill(
            top: panelTop,
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: onDismiss,
              excludeFromSemantics: true,
            ),
          ),
          Positioned(
            top: panelTop + Space.s,
            right: Space.s,
            bottom: Space.s,
            width: panelWidth - Space.s,
            child: Container(
              clipBehavior: Clip.antiAlias,
              decoration: BoxDecoration(
                color: palette.canvas,
                borderRadius: BorderRadius.circular(Radii.floating),
                border: Border.all(color: palette.hairline),
                boxShadow: floatingShadow,
              ),
              child: title == null
                  ? panel
                  : Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Container(
                          padding: const EdgeInsetsDirectional.only(
                            start: Space.l,
                            end: Space.xs,
                          ),
                          decoration: BoxDecoration(
                            border: Border(
                              bottom: BorderSide(color: palette.hairline),
                            ),
                          ),
                          child: Row(
                            children: [
                              Expanded(
                                child: Semantics(
                                  header: true,
                                  child: Text(
                                    title,
                                    style: type.label.copyWith(
                                      color: palette.textMuted,
                                    ),
                                  ),
                                ),
                              ),
                              _Step(
                                label: 'Hide $title',
                                icon: 'x',
                                onTap: onDismiss,
                              ),
                            ],
                          ),
                        ),
                        Expanded(child: panel),
                      ],
                    ),
            ),
          ),
        ],
      ],
    );
  }
}

/// "3 of 7" in the stage's header, where the queue isn't beside it: a tap
/// shows the queue.
class _Position extends StatelessWidget {
  const _Position({required this.session, required this.onTap});

  final FocusSession session;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final (at, of) = session.position;
    if (of == 0) return const SizedBox.shrink();
    return Button(label: at == 0 ? '$of' : '$at of $of', onPressed: onTap);
  }
}

/// ˄ or ˅ in the stage's header: the previous or next waiting card, as
/// Mail moves between messages.
class _Step extends StatelessWidget {
  const _Step({required this.label, required this.icon, required this.onTap});

  final String label;
  final String icon;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) => ToolButton(
    label: label,
    active: false,
    onTap: onTap,
    glyph: (color) => ExtensionGlyph(icon: icon, name: '', color: color),
  );
}
