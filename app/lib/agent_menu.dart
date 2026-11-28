// An agent's menu, ⋯ in its header: a Mac shows its
// rows in a menu under ⋯, an iPad in a popover with a finger's rows, both
// from [agentMenuItems]. A phone's ⋯ sheet builds its own rows
// (mobile/extension_list.dart).
import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'conversation_versions.dart' show showVersions;
import 'errands/browser_agent.dart' show browserOpen;
import 'errands/tab_screens.dart' show watchBrowser;
import 'extension_about.dart' show hasExtensionView;
import 'extensions.dart' show enabledExtensions;
import 'find.dart' show findShortcut;
import 'forms.dart' show AgentSettingsForm;
import 'highlights.dart';
import 'identity.dart';
import 'mobile/device.dart' as device;
import 'mobile/metrics.dart';
import 'mobile/popover.dart' show popoverHere, presentPopover;
import 'pinned_agents.dart' show movePin, pinLabel, togglePinLater;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart' show Unselectable;

/// One row of an agent's menu.
class AgentMenuItem {
  const AgentMenuItem({
    required this.id,
    required this.label,
    required this.glyph,
    required this.onSelect,
    this.semanticsLabel,
    this.checked = false,
    this.count = 0,
    this.shortcut = '',
    this.separatorBefore = false,
    this.highlightKey,
  });

  /// What it is, for tests and keys: `find`, `versions`, `file:<pane id>`,
  /// `browser`,
  /// `panel:<extension id>`, `pin`, `pin-up`, `pin-down`, `settings` or
  /// `extensions`.
  final String id;

  /// Its words ("Find…", "Other versions (3)", "Checklist").
  final String label;

  /// What a screen reader says, when it isn't [label].
  final String? semanticsLabel;

  /// Its glyph, in the color the row gives it.
  final Widget Function(Color color) glyph;

  /// What choosing it does, once the menu has closed.
  final VoidCallback onSelect;

  /// The open panel: a check before it.
  final bool checked;

  /// A number at its end: Checklist's open items, the tasks running.
  final int count;

  /// Its key, at its end on a Mac and an iPad with a keyboard ("⌘F").
  final String shortcut;

  /// A hairline goes above it.
  final bool separatorBefore;

  /// The highlight key that glows on it (`extension:<agent>/<id>`).
  final String? highlightKey;
}

/// [agent]'s extensions whose panels have something to show, in the
/// workspace's order: Secrets shows what it was given though the backend
/// sends no view for it, and Reports its reports. Those that only say what
/// they do (Web, Viewer…) aren't in the menu: Agent settings → Extensions
/// has their switches and options.
List<Extension> menuPanels(Workspace workspace, Agent agent) => [
  for (final extension in enabledExtensions(workspace, agent))
    if (hasExtensionView(extension) ||
        extension.id == 'secrets' ||
        extension.id == 'handoff')
      extension,
];

/// The count a panel's row and header button show: Checklist's open
/// items, the tasks running; 0 for the others.
int panelCount(Agent agent, String extension) => switch (extension) {
  'checklist' => agent.checklistOpen,
  'tasks' => agent.tasksRunning,
  _ => 0,
};

/// Opens [agent]'s settings, as they are now.
void showAgentSettings(
  BuildContext context,
  Agent agent, {
  bool extensions = false,
}) {
  final current = AppScope.of(context).workspace.agent(agent.id) ?? agent;
  unawaited(
    showModal(
      context,
      (context, close) => AgentSettingsForm(
        agent: current,
        close: close,
        showExtensions: extensions,
      ),
    ),
  );
}

/// The rows of [agent]'s menu, in order: Find… (when [onFind] can open
/// the conversation's find), Other versions while it has some, [files]
/// (a phone's files beside the conversation), Watch browser while its
/// browser is open, the panels with something to
/// show (the open one checked; choosing it closes it), then Pin or Unpin
/// and a pin's Move up and Move down, Agent settings… and Extensions….
List<AgentMenuItem> agentMenuItems(
  BuildContext context,
  Agent agent, {
  VoidCallback? onFind,
  List<AgentMenuItem> files = const [],
}) {
  final backend = AppScope.of(context);
  final workspace = backend.workspace.workspace;
  final panels = menuPanels(workspace, agent);
  // Pin or Unpin, and a pin's moves: here too, where
  // VoiceOver on a Mac reaches them.
  final pin = pinLabel(workspace, agent);
  // Taken now: the menu may outlive [context].
  final toggle = togglePinLater(context, agent);
  final up = movePin(context, agent, -1);
  final down = movePin(context, agent, 1);
  return [
    if (onFind != null)
      AgentMenuItem(
        id: 'find',
        label: 'Find…',
        semanticsLabel: 'Find',
        shortcut: findShortcut,
        glyph: (color) =>
            ExtensionGlyph(icon: 'search', name: 'Find', color: color),
        onSelect: onFind,
      ),
    if (agent.branches > 0)
      AgentMenuItem(
        id: 'versions',
        label: 'Other versions (${agent.branches})',
        semanticsLabel: 'Other versions, ${agent.branches}',
        glyph: (color) =>
            ExtensionGlyph(icon: 'history', name: 'Versions', color: color),
        onSelect: () => unawaited(showVersions(context, agent)),
      ),
    ...files,
    // Only while it's open: there's nothing to watch otherwise.
    if (browserOpen(workspace, agent.id))
      AgentMenuItem(
        id: 'browser',
        label: 'Watch browser',
        glyph: (color) =>
            ExtensionGlyph(icon: 'monitor', name: 'Browser', color: color),
        onSelect: () => watchBrowser(context, agent),
      ),
    for (final (index, extension) in panels.indexed)
      AgentMenuItem(
        id: 'panel:${extension.id}',
        label: extension.name,
        checked: agent.openPanel == extension.id,
        count: panelCount(agent, extension.id),
        separatorBefore: index == 0,
        highlightKey: 'extension:${agent.id}/${extension.id}',
        glyph: (color) => ExtensionGlyph(
          icon: extension.icon,
          name: extension.name,
          color: color,
        ),
        onSelect: () => fireAndLog(
          backend.openPanel(
            agent.id,
            agent.openPanel == extension.id ? '' : extension.id,
          ),
        ),
      ),
    if (pin != null)
      AgentMenuItem(
        id: 'pin',
        label: pin,
        separatorBefore: true,
        glyph: (color) => ExtensionGlyph(
          icon: pin == 'Pin' ? 'pin' : 'pin-off',
          name: pin,
          color: color,
        ),
        onSelect: () => unawaited(toggle()),
      ),
    if (up != null)
      AgentMenuItem(
        id: 'pin-up',
        label: 'Move up',
        glyph: (color) =>
            ExtensionGlyph(icon: 'chevron-up', name: 'Move up', color: color),
        onSelect: up,
      ),
    if (down != null)
      AgentMenuItem(
        id: 'pin-down',
        label: 'Move down',
        glyph: (color) => ExtensionGlyph(
          icon: 'chevron-down',
          name: 'Move down',
          color: color,
        ),
        onSelect: down,
      ),
    AgentMenuItem(
      id: 'settings',
      label: 'Agent settings…',
      semanticsLabel: 'Agent settings',
      // One group with Pin and the moves.
      separatorBefore: pin == null,
      glyph: (color) => CogGlyph(color: color),
      onSelect: () => showAgentSettings(context, agent),
    ),
    AgentMenuItem(
      id: 'extensions',
      label: 'Extensions…',
      semanticsLabel: 'Extensions',
      glyph: (color) =>
          ExtensionGlyph(icon: 'puzzle', name: 'Extensions', color: color),
      // At its extensions, unfolded.
      onSelect: () => showAgentSettings(context, agent, extensions: true),
    ),
  ];
}

/// Opens [agent]'s menu from ⋯ ([context] is the button): a popover
/// pointing at it on an iPad, a menu under it elsewhere. [onFind] opens
/// the conversation's find.
Future<void> showAgentMenu(
  BuildContext context,
  Agent agent, {
  VoidCallback? onFind,
}) async {
  final store = AppScope.of(context).workspace;
  // The rows as the agent is when they're drawn: a panel opened meanwhile
  // gets its check.
  Widget menu(BuildContext inner, void Function() close) => ListenableBuilder(
    listenable: store,
    builder: (_, _) {
      final now = store.agent(agent.id) ?? agent;
      return AgentMenu(
        items: agentMenuItems(context, now, onFind: onFind),
        close: close,
      );
    },
  );
  if (popoverHere(context)) {
    await presentPopover<void>(
      context,
      builder: (inner, close) => menu(inner, () => close()),
    );
    return;
  }
  await showFloating<void>(
    context,
    width: agentMenuWidth,
    end: true,
    builder: (inner, close) => menu(inner, () => close()),
  );
}

/// A Mac's agent menu's width.
const agentMenuWidth = 280.0;

/// A Mac's menu row.
const _rowHeight = 28.0;

/// The rows of an agent's menu, in a Mac's menu or an iPad's popover:
/// 28px rows with a pointer, a finger's on touch. Up and Down move, Enter
/// chooses; choosing one closes the menu first, then does it.
class AgentMenu extends StatefulWidget {
  const AgentMenu({super.key, required this.items, required this.close});

  final List<AgentMenuItem> items;
  final VoidCallback close;

  @override
  State<AgentMenu> createState() => _AgentMenuState();
}

class _AgentMenuState extends State<AgentMenu> {
  /// The row the keys are on; -1 before any.
  var _at = -1;

  void _choose(AgentMenuItem item) {
    widget.close();
    item.onSelect();
  }

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final count = widget.items.length;
    if (count == 0) return KeyEventResult.ignored;
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.arrowDown) {
      setState(() => _at = (_at + 1) % count);
    } else if (key == LogicalKeyboardKey.arrowUp) {
      setState(() => _at = _at <= 0 ? count - 1 : _at - 1);
    } else if ((key == LogicalKeyboardKey.enter ||
            key == LogicalKeyboardKey.space) &&
        _at >= 0 &&
        _at < count) {
      _choose(widget.items[_at]);
    } else {
      return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final store = AppScope.of(context).highlights;
    return Focus(
      autofocus: true,
      onKeyEvent: _key,
      child: Unselectable(
        child: SingleChildScrollView(
          padding: const EdgeInsets.symmetric(vertical: Space.xs),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final (index, item) in widget.items.indexed) ...[
                if (item.separatorBefore && index > 0)
                  Container(
                    height: 1,
                    margin: const EdgeInsets.symmetric(
                      vertical: Space.xs,
                      horizontal: Space.s,
                    ),
                    color: palette.hairline,
                  ),
                _row(context, index, item, store),
              ],
            ],
          ),
        ),
      ),
    );
  }

  Widget _row(
    BuildContext context,
    int index,
    AgentMenuItem item,
    HighlightStore store,
  ) {
    final row = _MenuRow(
      key: ValueKey('agent-menu:${item.id}'),
      item: item,
      at: index == _at,
      onHover: () => setState(() => _at = index),
      onTap: () => _choose(item),
    );
    final key = item.highlightKey;
    if (key == null) return row;
    return Glow(store: store, matches: (glowing) => glowing == key, child: row);
  }
}

class _MenuRow extends StatelessWidget {
  const _MenuRow({
    super.key,
    required this.item,
    required this.at,
    required this.onHover,
    required this.onTap,
  });

  final AgentMenuItem item;

  /// The keys are on it.
  final bool at;
  final VoidCallback onHover;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final touch = touchFirst(context);
    final type = TypeScale.of(context);
    // A key only where there's a keyboard to press it on.
    final end = item.count > 0
        ? '${item.count}'
        : !touch || device.hardwareKeyboard.value
        ? item.shortcut
        : '';
    return Semantics(
      button: true,
      selected: item.checked,
      label: item.semanticsLabel ?? item.label,
      value: item.count > 0 ? '${item.count}' : null,
      // Its own tap: excluding the row's semantics drops the Pressable's,
      // and VoiceOver's VO-Space then did nothing.
      onTap: onTap,
      excludeSemantics: true,
      child: MouseRegion(
        onEnter: (_) => onHover(),
        child: Pressable(
          onTap: onTap,
          builder: (context, hovered) => Container(
            constraints: BoxConstraints(
              minHeight: touch ? Metrics.of(context).target : _rowHeight,
            ),
            margin: const EdgeInsets.symmetric(horizontal: Space.xs),
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            decoration: BoxDecoration(
              color: at || hovered ? palette.selected : null,
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Row(
              children: [
                SizedBox(
                  width: 18,
                  child: item.checked
                      ? ExtensionGlyph(
                          icon: 'check',
                          name: 'Open',
                          color: palette.text,
                          size: 14,
                        )
                      : null,
                ),
                const SizedBox(width: Space.xs),
                item.glyph(palette.textMuted),
                const SizedBox(width: Space.s + 2),
                Expanded(
                  child: Text(
                    item.label,
                    maxLines: touch ? null : 1,
                    overflow: touch ? null : TextOverflow.ellipsis,
                    style: type.body.copyWith(color: palette.text),
                  ),
                ),
                if (end.isNotEmpty) ...[
                  const SizedBox(width: Space.m),
                  Text(
                    end,
                    style: type.caption.copyWith(
                      color: palette.textFaint,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Shows [builder] in a floating layer under [context]'s widget, [width]
/// wide, at its left edge (its right edge when [end]); above it when
/// there's more room there, never past the window's safe area. A click
/// anywhere outside it, or Esc, closes it, and nothing behind takes that
/// click; a dialog opened from inside it (the model list) comes over it.
/// Completes with what it's closed with; focus goes back where it was.
Future<T?> showFloating<T>(
  BuildContext context, {
  required double width,
  bool end = false,
  required Widget Function(BuildContext context, void Function([T?]) close)
  builder,
}) {
  final overlay = Overlay.of(context);
  final before = FocusManager.instance.primaryFocus;
  final done = Completer<T?>();
  late final OverlayEntry entry;
  var removed = false;
  void close([T? value]) {
    if (done.isCompleted) return;
    done.complete(value);
    if (removed) return;
    removed = true;
    entry.remove();
    entry.dispose();
    if (before != null && before.context != null) before.requestFocus();
  }

  entry = OverlayEntry(
    builder: (inner) {
      // Where its anchor is now: a window resized meanwhile moves it.
      MediaQuery.sizeOf(inner);
      final anchor = _rectIn(context, overlay);
      if (anchor == null) {
        scheduleMicrotask(close);
        return const SizedBox.shrink();
      }
      return _Floating(
        anchor: anchor,
        width: width,
        end: end,
        close: close,
        child: Builder(builder: (inner) => builder(inner, close)),
      );
    },
  );
  overlay.insert(entry);
  return done.future;
}

/// [context]'s widget's box in [overlay]'s coordinates, as last laid out;
/// null once it's gone.
Rect? _rectIn(BuildContext context, OverlayState overlay) {
  if (!context.mounted) return null;
  final box = context.findRenderObject();
  final overlayBox = overlay.context.findRenderObject();
  if (box is! RenderBox ||
      !box.attached ||
      !box.hasSize ||
      overlayBox is! RenderBox) {
    return null;
  }
  return MatrixUtils.transformRect(
    box.getTransformTo(overlayBox),
    Offset.zero & box.size,
  );
}

class _Floating extends StatelessWidget {
  const _Floating({
    required this.anchor,
    required this.width,
    required this.end,
    required this.close,
    required this.child,
  });

  final Rect anchor;
  final double width;
  final bool end;
  final VoidCallback close;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Stack(
      children: [
        Positioned.fill(
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTapDown: (_) => close(),
            onSecondaryTapDown: (_) => close(),
          ),
        ),
        Positioned.fill(
          child: CustomSingleChildLayout(
            delegate: _FloatingPlacement(
              anchor: anchor,
              end: end,
              safe: MediaQuery.paddingOf(context),
            ),
            child: CallbackShortcuts(
              bindings: {
                const SingleActivator(LogicalKeyboardKey.escape): close,
              },
              child: FocusScope(
                autofocus: true,
                child: Container(
                  width: width,
                  decoration: BoxDecoration(
                    color: palette.sidebar,
                    borderRadius: BorderRadius.circular(Radii.panel),
                    border: Border.all(color: palette.hairline),
                    boxShadow: floatingShadow,
                  ),
                  child: ClipRRect(
                    borderRadius: BorderRadius.circular(Radii.panel),
                    child: child,
                  ),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// Under its anchor, a little apart, at its left or right edge; above it
/// when there's more room there; shifted in from the window's edges and
/// never taller than the room it has.
class _FloatingPlacement extends SingleChildLayoutDelegate {
  _FloatingPlacement({
    required this.anchor,
    required this.end,
    required this.safe,
  });

  final Rect anchor;
  final bool end;
  final EdgeInsets safe;

  static const _margin = Space.s;
  static const _gap = Space.xs;

  double _below(Size size) =>
      size.height - safe.bottom - _margin - anchor.bottom - _gap;
  double _above(Size size) => anchor.top - _gap - safe.top - _margin;

  bool _under(Size size) => _below(size) >= 240 || _below(size) >= _above(size);

  @override
  BoxConstraints getConstraintsForChild(BoxConstraints constraints) {
    final size = constraints.biggest;
    final room = _under(size) ? _below(size) : _above(size);
    return BoxConstraints(
      maxWidth: math.max(0, size.width - safe.horizontal - 2 * _margin),
      maxHeight: math.max(0, room),
    );
  }

  @override
  Offset getPositionForChild(Size size, Size child) {
    final left = end ? anchor.right - child.width : anchor.left;
    final x = left
        .clamp(
          safe.left + _margin,
          math.max(
            safe.left + _margin,
            size.width - safe.right - _margin - child.width,
          ),
        )
        .toDouble();
    final y = _under(size)
        ? anchor.bottom + _gap
        : anchor.top - _gap - child.height;
    return Offset(x, y);
  }

  @override
  bool shouldRelayout(_FloatingPlacement old) =>
      old.anchor != anchor || old.end != end || old.safe != safe;
}
