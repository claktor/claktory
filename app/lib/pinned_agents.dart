import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

// Pinned agents (Lucy's ux/pinned-agents.md): agents from any project the
// person keeps under Rivet in the sidebar, in their order. The pins are the
// backend's (Settings.pinned_agents); the app only shows them and asks to
// change them.

/// The agents pinned under Rivet, in the person's order: each one still
/// here, in a project that isn't archived, and not a pipe's liaison (whose
/// pipe's row speaks for it).
List<Agent> pinnedAgents(Workspace workspace) {
  final byId = {for (final agent in workspace.agents) agent.id: agent};
  final archived = {
    for (final project in workspace.projects)
      if (project.archived) project.id,
  };
  final seen = <String>{};
  return [
    for (final id in workspace.settings.pinnedAgents)
      if (byId[id] case final agent?
          when agent.projectId.isNotEmpty &&
              agent.pipeId.isEmpty &&
              !archived.contains(agent.projectId) &&
              seen.add(id))
        agent,
  ];
}

/// The ids of [pinnedAgents].
Set<String> pinnedIds(Workspace workspace) => {
  for (final agent in pinnedAgents(workspace)) agent.id,
};

/// What the menus offer for [agent]: 'Pin', 'Unpin', or null where it
/// can't be pinned (a workspace agent, already at the top; a pipe's
/// liaison; an agent in an archived project).
String? pinLabel(Workspace workspace, Agent agent) {
  if (agent.projectId.isEmpty || agent.pipeId.isNotEmpty) return null;
  final archived = workspace.projects.any(
    (project) => project.id == agent.projectId && project.archived,
  );
  if (archived) return null;
  return pinnedIds(workspace).contains(agent.id) ? 'Unpin' : 'Pin';
}

/// The agent just pinned from a menu: its pin row glows and comes into
/// view once it shows, then clears this.
final justPinned = ValueNotifier<String?>(null);

/// Whether an agent is being dragged in the sidebar: true from the moment
/// a row is picked up (a finger's hold) until it's let go, so a phone's
/// drawer around it neither swipes nor closes meanwhile.
final sidebarDragging = ValueNotifier<bool>(false);

/// Pins [agent] at the end of the pins, or unpins it; a refusal says why.
Future<void> togglePin(BuildContext context, Agent agent) {
  final pinned = pinnedIds(AppScope.of(context).workspace.workspace)
      .contains(agent.id);
  return pinned ? _unpin(context, agent) : pinAgentBefore(context, agent, '');
}

/// Pins [agent] before the pin [beforeId] (empty: last), or moves its pin
/// there; a refusal says why.
Future<void> pinAgentBefore(
  BuildContext context,
  Agent agent,
  String beforeId,
) async {
  final backend = AppScope.of(context);
  final origin = _Origin(context);
  final wasPinned = pinnedIds(backend.workspace.workspace).contains(agent.id);
  try {
    await backend.pinAgent(agent.id, beforeId: beforeId);
    if (!wasPinned) {
      justPinned.value = agent.id;
      backend.highlights.glow('pin:${agent.id}', const [signal]);
    }
  } on RequestFailure catch (failure) {
    await origin.say(
      wasPinned ? "Couldn't move ${agent.name}" : "Couldn't pin ${agent.name}",
      _why(failure, agent),
    );
  }
}

Future<void> _unpin(BuildContext context, Agent agent) async {
  final backend = AppScope.of(context);
  final origin = _Origin(context);
  try {
    await backend.unpinAgent(agent.id);
  } on RequestFailure catch (failure) {
    await origin.say("Couldn't unpin ${agent.name}", _why(failure, agent));
  }
}

/// Moves [agent]'s pin one place up ([by] -1) or down (1); null at that
/// end of the pins, or when it isn't pinned.
VoidCallback? movePin(BuildContext context, Agent agent, int by) {
  final pins = [
    for (final pin in pinnedAgents(AppScope.of(context).workspace.workspace))
      pin.id,
  ];
  final at = pins.indexOf(agent.id);
  final to = at + by;
  if (at < 0 || to < 0 || to >= pins.length) return null;
  // Up: before the one above. Down: before the one after the next.
  final before = by < 0 ? pins[to] : (to + 1 < pins.length ? pins[to + 1] : '');
  return () => unawaited(pinAgentBefore(context, agent, before));
}

/// Opens the pinned agent [agentId]'s conversation, as a click on its row
/// does (an iPad's ⌘-digit, the Mac's ⌘1 to ⌘9): at what waits for the
/// person, if it's news.
/// Why a pin was refused: the backend's own words, but an agent gone
/// (its generic "no agent with id …") in the app's (Trish 49630).
String _why(RequestFailure failure, Agent agent) =>
    failure.message.startsWith('no agent with id')
    ? "${agent.name} isn't in this Claktory any more."
    : failure.message;

void openPinned(BuildContext context, String agentId) {
  final backend = AppScope.of(context);
  if (backend.workspace.agent(agentId) == null) return;
  AppScope.closePage(context);
  fireAndLog(() async {
    await backend.selectAgent(agentId);
    final now = backend.workspace.agent(agentId);
    if (now != null) backend.reveal.oldestWaiting(now);
  }());
}

/// The row's context while it's there, else the list it was in, else the
/// app's own navigator: a refusal that comes after the row went (unpinned
/// meanwhile) still has somewhere to be said.
class _Origin {
  _Origin(this.row)
    : list = row.findAncestorStateOfType<ScrollableState>()?.context,
      root = Navigator.maybeOf(row, rootNavigator: true)?.context;
  final BuildContext row;
  final BuildContext? list;
  final BuildContext? root;

  Future<void> say(String title, String why) async {
    for (final here in [row, list, root]) {
      if (here != null && here.mounted) {
        await showActions<void>(
          here,
          title: title,
          message: why,
          actions: const [],
          cancelLabel: 'OK',
        );
        return;
      }
    }
  }
}

/// A row of [showPointerMenu]; null in its list is a hairline between
/// groups.
class PointerMenuItem<T> {
  const PointerMenuItem(this.label, this.value, {this.glyph});

  final String label;
  final T value;

  /// A 16px glyph before the label, in the color given.
  final Widget Function(Color color)? glyph;
}

/// A right click's menu at [at] (global): a floating layer like the Copy
/// menu, 28px rows with their glyphs, hairlines between groups, kept inside
/// the window. A click elsewhere or Escape closes it. Completes with the
/// value chosen, or null.
Future<T?> showPointerMenu<T>(
  BuildContext context, {
  required Offset at,
  required List<PointerMenuItem<T>?> items,
  String? label,
}) {
  final overlay = Overlay.of(context);
  final box = overlay.context.findRenderObject();
  final local = box is RenderBox ? box.globalToLocal(at) : at;
  final done = Completer<T?>();
  late final OverlayEntry entry;
  void close([T? value]) {
    if (done.isCompleted) return;
    entry.remove();
    entry.dispose();
    done.complete(value);
  }

  entry = OverlayEntry(
    builder: (context) => Stack(
      children: [
        // A click elsewhere closes it, and does nothing else.
        Positioned.fill(
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTapDown: (_) => close(),
            onSecondaryTapDown: (_) => close(),
          ),
        ),
        CustomSingleChildLayout(
          delegate: _MenuLayout(local),
          child: _PointerMenu<T>(items: items, label: label, close: close),
        ),
      ],
    ),
  );
  overlay.insert(entry);
  return done.future;
}

class _PointerMenu<T> extends StatefulWidget {
  const _PointerMenu({required this.items, required this.close, this.label});

  final List<PointerMenuItem<T>?> items;
  final void Function([T?]) close;
  final String? label;

  @override
  State<_PointerMenu<T>> createState() => _PointerMenuState<T>();
}

class _PointerMenuState<T> extends State<_PointerMenu<T>> {
  final _scope = FocusScopeNode(debugLabel: 'pointer menu');

  @override
  void initState() {
    super.initState();
    // It takes the keys as it opens: Esc closes it, the arrows and Return
    // choose.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && !_scope.hasFocus) _scope.requestFocus();
    });
  }

  @override
  void dispose() {
    _scope.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final items = widget.items;
    final label = widget.label;
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): widget.close,
      },
      child: FocusScope(
        node: _scope,
        autofocus: true,
        child: _menu(palette, items, label),
      ),
    );
  }

  Widget _menu(
    Palette palette,
    List<PointerMenuItem<T>?> items,
    String? label,
  ) {
    return Semantics(
      scopesRoute: true,
      namesRoute: true,
      explicitChildNodes: true,
      label: label,
      child: IntrinsicWidth(
        child: Container(
          constraints: const BoxConstraints(minWidth: 180),
          padding: const EdgeInsets.all(Space.xs),
          decoration: BoxDecoration(
            color: palette.sidebar,
            borderRadius: BorderRadius.circular(Radii.panel),
            border: Border.all(color: palette.hairline),
            boxShadow: floatingShadow,
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final (index, item) in items.indexed)
                if (item != null)
                  _PointerMenuRow<T>(item: item, close: widget.close)
                else if (index > 0 && index < items.length - 1)
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: Space.xs),
                    child: Container(height: 1, color: palette.hairline),
                  ),
            ],
          ),
        ),
      ),
    );
  }
}

class _PointerMenuRow<T> extends StatefulWidget {
  const _PointerMenuRow({required this.item, required this.close});

  final PointerMenuItem<T> item;
  final void Function([T?]) close;

  @override
  State<_PointerMenuRow<T>> createState() => _PointerMenuRowState<T>();
}

class _PointerMenuRowState<T> extends State<_PointerMenuRow<T>> {
  var _hovered = false;
  var _focused = false;

  void _choose() => widget.close(widget.item.value);

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final item = widget.item;
    return Semantics(
      button: true,
      label: item.label,
      // Its own tap, as excluding drops the row's: VoiceOver's VO-Space
      // (Abbacchio's row 1716).
      onTap: _choose,
      excludeSemantics: true,
      child: FocusableActionDetector(
        mouseCursor: SystemMouseCursors.click,
        onShowHoverHighlight: (value) => setState(() => _hovered = value),
        onShowFocusHighlight: (value) => setState(() => _focused = value),
        actions: {
          ActivateIntent: CallbackAction<ActivateIntent>(
            onInvoke: (_) {
              _choose();
              return null;
            },
          ),
        },
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: _choose,
          child: Container(
            constraints: const BoxConstraints(minHeight: 28),
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            decoration: BoxDecoration(
              color: _hovered || _focused
                  ? palette.selected
                  : const Color(0x00000000),
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Row(
              children: [
                if (item.glyph case final glyph?) ...[
                  SizedBox.square(
                    dimension: 16,
                    child: glyph(palette.textMuted),
                  ),
                  const SizedBox(width: Space.s),
                ],
                Expanded(
                  child: Text(
                    item.label,
                    style: TypeScale.of(context).body
                        .copyWith(color: palette.text),
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

/// Puts the menu's top left at the anchor, kept inside the window.
class _MenuLayout extends SingleChildLayoutDelegate {
  _MenuLayout(this.anchor);

  final Offset anchor;

  @override
  BoxConstraints getConstraintsForChild(BoxConstraints constraints) =>
      constraints.loosen();

  @override
  Offset getPositionForChild(Size size, Size child) => Offset(
    anchor.dx.clamp(0, math.max(0, size.width - child.width)),
    anchor.dy.clamp(0, math.max(0, size.height - child.height)),
  );

  @override
  bool shouldRelayout(_MenuLayout old) => old.anchor != anchor;
}

/// What the row menus offer, in order, for SheetAction lists on touch:
/// the same rows as [PointerMenuItem]s without the hairlines.
List<SheetAction<T>> sheetActionsOf<T>(List<PointerMenuItem<T>?> items) => [
  for (final item in items)
    if (item != null) SheetAction(item.label, item.value, glyph: item.glyph),
];

/// What archiving the project [projectId] does to the pins (Trish 46504):
/// "Bruno is unpinned.", "Bruno and June are unpinned.", up to three names
/// and then "and 2 more"; empty when none of its agents is pinned.
String unpinnedOnArchive(Workspace workspace, String projectId) {
  final names = [
    for (final agent in pinnedAgents(workspace))
      if (agent.projectId == projectId) agent.name,
  ];
  if (names.isEmpty) return '';
  final String who;
  if (names.length == 1) {
    who = names.single;
  } else if (names.length <= 3) {
    who = '${names.sublist(0, names.length - 1).join(', ')} and ${names.last}';
  } else {
    who = '${names.take(3).join(', ')} and ${names.length - 3} more';
  }
  return names.length == 1 ? '$who is unpinned.' : '$who are unpinned.';
}
