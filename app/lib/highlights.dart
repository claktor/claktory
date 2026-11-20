import 'dart:async';

import 'package:flutter/widgets.dart';

import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'state.dart';

/// What glows, and what's said beside it, so the person sees state change
/// as it happens: what the app sees appear or change (a new agent, project
/// or folder; a checklist item added or moved on), messages between agents,
/// and what an agent points out with its highlight tool.
///
/// A glow is for a target key: `agent:<id>`, `project:<id>`,
/// `folder:<project id>/<folder id>`, `checklist_item:<agent id>#<n>`,
/// `extension:<agent id>/<extension id>`, `pane:<agent id>:<pane id>` (a
/// viewer beside an agent's conversation), `settings:` or `marketplace:`.
/// Each [Glow] widget says which keys it shows, so a target that isn't on
/// screen (folded away, say) glows on what stands for it.
///
/// Nothing glows while the person has highlights off
/// (`Workspace.highlights`), or before the backend has sent the workspace.
class HighlightStore extends ChangeNotifier {
  HighlightStore({required this.workspace, required this.extensionView}) {
    workspace.addListener(_workspaceChanged);
    extensionView.addListener(_viewChanged);
  }

  final WorkspaceStore workspace;
  final ExtensionViewStore extensionView;

  /// How long a glow lasts: it fades in, holds, and fades out.
  static const duration = Duration(milliseconds: 1400);

  /// How long a callout stays unless clicked away.
  static const calloutDuration = Duration(seconds: 5);

  final _glows = <HighlightGlow>[];

  /// The keys the backend last pointed at, glowing or not: a viewer's pane
  /// reads its file again when an agent shows it again.
  final pointedAt = ValueNotifier<({List<String> keys, DateTime at})?>(null);
  HighlightCallout? _callout;
  var _nextId = 1;

  bool get enabled => workspace.received && workspace.workspace.highlights;

  /// The glows still showing, oldest first.
  List<HighlightGlow> get glows => List.unmodifiable(_glows);

  /// What's said beside a glowing target, if anything.
  HighlightCallout? get callout => _callout;

  /// Makes [key] glow in [colors] (one, or two blended across it).
  void glow(String key, List<Color> colors) {
    if (!enabled) return;
    final glow = HighlightGlow(
      id: _nextId++,
      key: key,
      colors: colors,
      started: DateTime.now(),
    );
    _glows.add(glow);
    Timer(duration, () {
      _glows.remove(glow);
      notifyListeners();
    });
    notifyListeners();
  }

  /// Shows [text] beside [key] for a few seconds.
  void say(String key, String text) {
    if (!enabled || text.isEmpty) return;
    final callout = HighlightCallout(id: _nextId++, key: key, text: text);
    _callout = callout;
    Timer(calloutDuration, () => dismiss(callout));
    notifyListeners();
  }

  /// Takes [callout] away, as when it's clicked.
  void dismiss(HighlightCallout callout) {
    if (_callout != callout) return;
    _callout = null;
    notifyListeners();
  }

  /// A highlight the backend sent: a message between agents, or what an
  /// agent pointed out.
  void apply(Highlight highlight) {
    final keys = [for (final target in highlight.targets) ?targetKey(target)];
    if (keys.isEmpty) return;
    // Pointed at, even with highlights off: a viewer reads its file again.
    pointedAt.value = (keys: keys, at: DateTime.now());
    switch (highlight.kind) {
      case HighlightKind.HIGHLIGHT_KIND_MESSAGE:
        // From the sender to the recipient, both rows in both colors.
        final colors = [for (final key in keys) _colorOf(key)];
        for (final key in keys) {
          glow(key, colors);
        }
      default:
        for (final key in keys) {
          glow(key, [_colorOf(key)]);
        }
        say(keys.first, highlight.text);
    }
  }

  /// The key for [target], or null for a kind this app doesn't show.
  static String? targetKey(HighlightTarget target) => switch (target.kind) {
    'agent' ||
    'project' ||
    'checklist_item' ||
    'extension' ||
    'pane' ||
    'settings' ||
    'marketplace' => '${target.kind}:${target.id}',
    _ => null,
  };

  /// The color a key glows in: its agent's, or signal for what isn't an
  /// agent's.
  Color _colorOf(String key) {
    final colon = key.indexOf(':');
    final kind = key.substring(0, colon);
    final id = key.substring(colon + 1);
    final agentId = switch (kind) {
      'agent' => id,
      'checklist_item' => id.split('#').first,
      'extension' => id.split('/').first,
      'pane' => id.split(':').first,
      _ => null,
    };
    final agent = agentId == null ? null : workspace.agent(agentId);
    return agent == null ? signal : agentColor(agent.color);
  }

  /// Another Factory is about to show: its first snapshot only sets
  /// what's known again, rather than glowing as all new.
  void forgetKnown() {
    _agents = null;
    _projects = null;
    _folders = null;
    _checklistAgent = null;
  }

  // What the app sees change, diffed between one snapshot and the next.
  // The first snapshot only sets what's known.
  Set<String>? _agents;
  Set<String>? _projects;
  Set<String>? _folders;

  void _workspaceChanged() {
    final snapshot = workspace.workspace;
    final agents = {for (final agent in snapshot.agents) agent.id};
    final projects = {for (final project in snapshot.projects) project.id};
    final folders = {
      for (final project in snapshot.projects)
        for (final folder in project.folders) '${project.id}/${folder.id}',
    };
    if (_agents != null) {
      for (final agent in snapshot.agents) {
        if (!_agents!.contains(agent.id)) {
          glow('agent:${agent.id}', [agentColor(agent.color)]);
        }
      }
      for (final id in projects.difference(_projects!)) {
        glow('project:$id', const [signal]);
      }
      for (final id in folders.difference(_folders!)) {
        glow('folder:$id', const [signal]);
      }
    }
    _agents = agents;
    _projects = projects;
    _folders = folders;
  }

  /// The checklist last shown: whose, and each item's status by number.
  String? _checklistAgent;
  Map<int, ChecklistStatus> _checklist = const {};

  void _viewChanged() {
    final view = extensionView.view;
    if (view == null || !view.hasChecklist()) return;
    final items = {
      for (final item in view.checklist.items) item.id: item.status,
    };
    // Another agent's checklist is a switch, not a change.
    if (view.agentId == _checklistAgent) {
      final agent = workspace.agent(view.agentId);
      final color = agent == null ? signal : agentColor(agent.color);
      for (final MapEntry(key: id, value: status) in items.entries) {
        if (_checklist[id] != status) {
          glow('checklist_item:${view.agentId}#$id', [color]);
        }
      }
    }
    _checklistAgent = view.agentId;
    _checklist = items;
  }

  @override
  void dispose() {
    workspace.removeListener(_workspaceChanged);
    extensionView.removeListener(_viewChanged);
    super.dispose();
  }
}

class HighlightGlow {
  const HighlightGlow({
    required this.id,
    required this.key,
    required this.colors,
    required this.started,
  });

  final int id;
  final String key;
  final List<Color> colors;
  final DateTime started;
}

class HighlightCallout {
  const HighlightCallout({
    required this.id,
    required this.key,
    required this.text,
  });

  final int id;
  final String key;
  final String text;
}

/// Glows over [child] when a highlight for a key it [matches] comes: a soft
/// tint and halo in the highlight's colors that fades in and out (a still
/// tint, briefly, with reduced motion). It also shows a highlight's callout
/// beside [child], on its [side].
class Glow extends StatefulWidget {
  const Glow({
    super.key,
    required this.store,
    required this.matches,
    required this.child,
    this.radius = Radii.control,
    this.side = AxisDirection.right,
  });

  final HighlightStore store;
  final bool Function(String key) matches;
  final Widget child;
  final double radius;

  /// Where a callout goes: right of [child], or left of it.
  final AxisDirection side;

  @override
  State<Glow> createState() => _GlowState();
}

class _GlowState extends State<Glow> with SingleTickerProviderStateMixin {
  late final _fade = AnimationController(
    vsync: this,
    duration: HighlightStore.duration,
  );

  /// The newest glow shown, so each plays once.
  var _shown = 0;
  List<Color> _colors = const [signal];

  /// Reduced motion: a still tint, until this timer ends.
  Timer? _still;

  final _portal = OverlayPortalController();
  final _link = LayerLink();
  HighlightCallout? _callout;

  @override
  void initState() {
    super.initState();
    widget.store.addListener(_changed);
    // A row that appears with its glow (a new agent) picks it up midway.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _changed();
    });
  }

  @override
  void didUpdateWidget(Glow old) {
    super.didUpdateWidget(old);
    if (old.store != widget.store) {
      old.store.removeListener(_changed);
      widget.store.addListener(_changed);
    }
  }

  @override
  void dispose() {
    widget.store.removeListener(_changed);
    _fade.dispose();
    _still?.cancel();
    super.dispose();
  }

  void _changed() {
    if (!mounted) return;
    HighlightGlow? newest;
    for (final glow in widget.store.glows) {
      if (glow.id > _shown && widget.matches(glow.key)) newest = glow;
    }
    if (newest != null) {
      _shown = newest.id;
      final elapsed = DateTime.now().difference(newest.started);
      final total = HighlightStore.duration.inMicroseconds;
      setState(() => _colors = newest!.colors);
      if (MediaQuery.disableAnimationsOf(context)) {
        _still?.cancel();
        _still = Timer(const Duration(milliseconds: 900), () {
          if (mounted) setState(() => _still = null);
        });
      } else {
        _fade.forward(from: (elapsed.inMicroseconds / total).clamp(0, 1));
      }
    }
    final callout = widget.store.callout;
    final mine = callout != null && widget.matches(callout.key)
        ? callout
        : null;
    if (mine != _callout) {
      setState(() => _callout = mine);
      if (mine != null) _portal.show();
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final colors = _colors;
    final halo = colors.length == 1
        ? colors.first
        : Color.lerp(colors.first, colors.last, 0.5)!;
    final light = DecoratedBox(
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(widget.radius),
        gradient: LinearGradient(
          colors: [
            for (final color
                in colors.length == 1 ? [colors.first, colors.first] : colors)
              color.withValues(alpha: 0.22),
          ],
        ),
        boxShadow: [
          BoxShadow(
            color: halo.withValues(alpha: 0.4),
            blurRadius: 12,
            spreadRadius: 1,
          ),
        ],
      ),
    );
    final callout = _callout;
    final left = widget.side == AxisDirection.left;
    return OverlayPortal(
      controller: _portal,
      overlayChildBuilder: (context) => CompositedTransformFollower(
        link: _link,
        targetAnchor: left ? Alignment.centerLeft : Alignment.centerRight,
        followerAnchor: left ? Alignment.centerRight : Alignment.centerLeft,
        offset: Offset(left ? -Space.s : Space.s, 0),
        child: Align(
          alignment: left ? Alignment.centerRight : Alignment.centerLeft,
          widthFactor: 1,
          heightFactor: 1,
          child: AnimatedOpacity(
            opacity: callout == null ? 0 : 1,
            duration: Motion.base,
            curve: Curves.easeOut,
            onEnd: () {
              if (_callout == null) _portal.hide();
            },
            child: _CalloutCard(
              text: callout?.text ?? '',
              palette: palette,
              onTap: () {
                if (callout != null) widget.store.dismiss(callout);
              },
            ),
          ),
        ),
      ),
      child: CompositedTransformTarget(
        link: _link,
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            widget.child,
            Positioned.fill(
              child: IgnorePointer(
                // Its own layer, faded without rebuilding: a rebuild each
                // frame, under the window's LayoutBuilder, would lay out
                // and repaint the whole window.
                child: RepaintBoundary(
                  child: _still != null
                      ? Opacity(opacity: 0.7, child: light)
                      : FadeTransition(
                          opacity: _fade.drive(const _Strength()),
                          child: light,
                        ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// The glow's strength over its life: in quickly, a moment held, out
/// slowly; none before it starts and after it ends.
class _Strength extends Animatable<double> {
  const _Strength();

  @override
  double transform(double t) {
    if (t <= 0 || t >= 1) return 0;
    if (t < 0.22) return Curves.easeOut.transform(t / 0.22);
    if (t < 0.45) return 1;
    return 1 - Curves.easeInOut.transform((t - 0.45) / 0.55);
  }
}

/// What an agent says about what it points out: a small floating card,
/// gone on a click.
class _CalloutCard extends StatelessWidget {
  const _CalloutCard({
    required this.text,
    required this.palette,
    required this.onTap,
  });

  final String text;
  final Palette palette;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: Container(
          constraints: const BoxConstraints(maxWidth: 260),
          padding: const EdgeInsets.symmetric(
            horizontal: Space.m,
            vertical: Space.s,
          ),
          decoration: BoxDecoration(
            color: palette.sidebar,
            borderRadius: BorderRadius.circular(Radii.panel),
            border: Border.all(color: palette.hairline),
            boxShadow: floatingShadow,
          ),
          child: Text(
            text,
            style: TextStyles.caption.copyWith(color: palette.text),
          ),
        ),
      ),
    );
  }
}

/// Whether [key] is one of [agentIds]' targets: the agent itself, or its
/// checklist items and extension icons when they aren't on screen, which
/// is the case for any but [shownAgentId], the agent whose bar and panel
/// show.
bool matchesAgents(
  String key,
  Set<String> agentIds, {
  String shownAgentId = '',
}) {
  final colon = key.indexOf(':');
  if (colon < 0) return false;
  final kind = key.substring(0, colon);
  final id = key.substring(colon + 1);
  final owner = switch (kind) {
    'agent' => id,
    'checklist_item' => id.split('#').first,
    'extension' => id.split('/').first,
    'pane' => id.split(':').first,
    _ => null,
  };
  if (owner == null || !agentIds.contains(owner)) return false;
  return kind == 'agent' || owner != shownAgentId;
}
