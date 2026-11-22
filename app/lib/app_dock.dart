import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'apps.dart' show appStateLine;
import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'layout.dart';
import 'mobile/metrics.dart';
import 'mobile/navigation.dart' show PhoneRoutes;
import 'mobile/presentation.dart';
import 'panes.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'rail.dart' show railWidth;
import 'scope.dart';
import 'selection.dart' show Unselectable;
import 'state.dart';

// The app dock (C5): the apps the person pinned, at the foot of the
// Claktory rail on a Mac (a phone shows them as a strip, [AppStrip]), and
// the launcher listing every installed app. What's pinned, and in what
// order, is the backend's (Workspace.pinned_apps, PinApp): the same on
// every device. Shows only where the backend offers apps.

/// The installed apps pinned to the dock, in its order.
List<AppInfo> pinnedApps(Workspace workspace) => [
  for (final id in workspace.pinnedApps) ?appById(workspace, id),
];

AppInfo? appById(Workspace workspace, String id) {
  for (final app in workspace.apps) {
    if (app.id == id) return app;
  }
  return null;
}

/// How [app] is doing, in its tile's card and the launcher's row:
/// "Running", "Needs setup"…; one with no views says it works through
/// agents, since there's nothing of it to open.
String appCaption(AppInfo app) {
  final state = appStateLine(app.state);
  return app.views.isEmpty ? '$state · Works through your agents' : state;
}

/// More about [app]'s state, when there's more to say: why it's paused or
/// can't run, else its own line (how to finish its setup); empty for none.
String appDetail(AppInfo app) =>
    app.stateDetail.isNotEmpty ? app.stateDetail : app.status;

/// The dot on [app]'s tile: `signal` while it needs the person's setup,
/// `danger` while it's paused or can't run; null for none.
Color? appDot(AppInfo app, Palette palette) => switch (app.state) {
  AppState.APP_STATE_NEEDS_SETUP => palette.signal,
  AppState.APP_STATE_PAUSED_CRASHED ||
  AppState.APP_STATE_UNAVAILABLE => palette.danger,
  _ => null,
};

/// Opens [app]'s [view] (empty: its first) beside the selected agent's
/// conversation, as Files open, or makes its pane glow when it's open
/// already. An app with no views opens its place in Settings → Apps.
void openApp(BuildContext context, AppInfo app, [String view = '']) {
  final scope = AppScope.scope(context);
  final backend = scope.backend;
  if (app.views.isEmpty) {
    AppScope.openPage(context, AppPage.settings);
    backend.settingsSection.value = 'apps';
    return;
  }
  final workspace = backend.workspace;
  final agent =
      workspace.selectedAgent ?? workspace.workspace.agents.firstOrNull;
  if (agent == null) return;
  // On a phone, the view opens as a page over the agent's conversation
  // (which chooses the agent), rather than beside it.
  final phone = PhoneRoutes.of(context);
  if (phone == null && workspace.selectedAgent == null) {
    fireAndLog(backend.selectAgent(agent.id));
  }
  if (scope.page.value != AppPage.agent) AppScope.closePage(context);
  final root = layoutOf(agent);
  final shown = appPane(root, app.id, view, app.views.first.id);
  if (shown != null) {
    if (phone != null && phone.showPane(agent.id, shown.id)) return;
    backend.highlights.glow('pane:${agent.id}:${shown.id}', [
      agentColor(agent.color),
    ]);
    return;
  }
  final opened = withApp(root, app, view);
  fireAndLog(backend.setLayout(agent.id, opened));
  final pane = appPane(opened, app.id, view, app.views.first.id);
  if (phone != null && pane != null) phone.showPane(agent.id, pane.id);
}

/// The apps with a view open beside the selected agent's conversation.
Set<String> _shownApps(WorkspaceStore workspace) {
  final agent = workspace.selectedAgent;
  if (agent == null) return const {};
  return {
    for (final leaf in leaves(layoutOf(agent)))
      if (leaf.leaf.kind == appKind) leaf.leaf.appId,
  };
}

/// Pins or unpins [app], or asks what to do with it (a right click on its
/// tile): Unpin, or its settings.
Future<void> _tileActions(BuildContext context, AppInfo app) async {
  final chosen = await showActions<String>(
    context,
    title: app.name,
    actions: [
      SheetAction(
        'Unpin',
        'unpin',
        glyph: (color) => _lucide('pin-off', color),
      ),
      SheetAction(
        'App settings…',
        'settings',
        glyph: (color) => CogGlyph(color: color),
      ),
    ],
  );
  if (!context.mounted) return;
  switch (chosen) {
    case 'unpin':
      fireAndLog(AppScope.of(context).pinApp(app.id, pinned: false));
    case 'settings':
      _openAppSettings(context);
  }
}

void _openAppSettings(BuildContext context) {
  AppScope.openPage(context, AppPage.settings);
  AppScope.of(context).settingsSection.value = 'apps';
}

Widget _lucide(String icon, Color color, [double size = 16]) =>
    ExtensionGlyph(icon: icon, name: '', color: color, size: size);

/// An app dragged in the dock, or from the launcher onto it.
class _PinDrag {
  const _PinDrag(this.appId);

  final String appId;
}

/// The dock at the foot of the Claktory rail: a short hairline, a 28px
/// tile for each pinned app (dragged to reorder, right-clicked to unpin),
/// then the launcher. Nothing while the backend offers no apps.
class AppDock extends StatelessWidget {
  const AppDock({super.key});

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final workspace = backend.workspace.workspace;
        if (!workspace.appsAvailable || workspace.apps.isEmpty) {
          return const SizedBox.shrink();
        }
        final palette = Palette.of(context);
        final pinned = pinnedApps(workspace);
        final shown = _shownApps(backend.workspace);
        return Unselectable(
          child: Padding(
            padding: const EdgeInsets.only(bottom: Space.s),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Container(
                  width: 16,
                  height: 1,
                  margin: const EdgeInsets.only(bottom: Space.s),
                  color: palette.hairline,
                ),
                for (final (i, app) in pinned.indexed)
                  Padding(
                    padding: const EdgeInsets.only(bottom: Space.xs),
                    child: _DockSlot(
                      key: ValueKey('dock-${app.id}'),
                      app: app,
                      position: i,
                      shown: shown.contains(app.id),
                    ),
                  ),
                const _LauncherTile(),
              ],
            ),
          ),
        );
      },
    );
  }
}

/// A pinned app's place in the dock: its tile, which drags to another
/// place, and where another app dropped on it goes.
class _DockSlot extends StatelessWidget {
  const _DockSlot({
    super.key,
    required this.app,
    required this.position,
    required this.shown,
  });

  final AppInfo app;
  final int position;
  final bool shown;

  @override
  Widget build(BuildContext context) {
    final tile = _DockTile(app: app, shown: shown);
    return DragTarget<_PinDrag>(
      onWillAcceptWithDetails: (details) => details.data.appId != app.id,
      onAcceptWithDetails: (details) => fireAndLog(
        AppScope.of(context).pinApp(details.data.appId, position: position),
      ),
      builder: (context, candidates, _) => Stack(
        clipBehavior: Clip.none,
        children: [
          Draggable<_PinDrag>(
            data: _PinDrag(app.id),
            feedback: _DragFeedback(app: app),
            childWhenDragging: Opacity(opacity: 0.3, child: tile),
            child: tile,
          ),
          // Where it lands: a signal line over the tile it takes the
          // place of.
          if (candidates.isNotEmpty)
            Positioned(
              left: 10,
              right: 10,
              top: -3,
              child: Container(height: 2, color: Palette.of(context).signal),
            ),
        ],
      ),
    );
  }
}

class _DragFeedback extends StatelessWidget {
  const _DragFeedback({required this.app});

  final AppInfo app;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      width: 28,
      height: 28,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.control),
        border: Border.all(color: palette.hairline),
        boxShadow: floatingShadow,
      ),
      child: ExtensionGlyph(
        icon: app.icon,
        name: app.name,
        color: palette.text,
      ),
    );
  }
}

/// A pinned app's 28px tile, like a Claktory's: its icon, the signal mark
/// at the window's edge while its view is open, a dot while it needs the
/// person; its name and state in a card on hover.
class _DockTile extends StatelessWidget {
  const _DockTile({required this.app, required this.shown});

  final AppInfo app;
  final bool shown;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final dot = appDot(app, palette);
    final detail = appDetail(app);
    return SizedBox(
      width: railWidth,
      height: 28,
      child: Stack(
        clipBehavior: Clip.none,
        alignment: Alignment.center,
        children: [
          Positioned(
            left: 0,
            top: 6,
            bottom: 6,
            child: AnimatedOpacity(
              duration: Motion.fast,
              opacity: shown ? 1 : 0,
              child: Container(
                width: 2,
                decoration: BoxDecoration(
                  color: palette.signal,
                  borderRadius: const BorderRadius.horizontal(
                    right: Radius.circular(1),
                  ),
                ),
              ),
            ),
          ),
          HoverCard(
            beside: true,
            card: (context) => Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  app.name,
                  style: TextStyles.label.copyWith(color: palette.text),
                ),
                Text(
                  appCaption(app),
                  style: TextStyles.caption.copyWith(
                    color: dot == palette.danger
                        ? palette.danger
                        : palette.textMuted,
                  ),
                ),
                if (detail.isNotEmpty)
                  ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 260),
                    child: Text(
                      detail,
                      maxLines: 3,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyles.caption.copyWith(
                        color: palette.textFaint,
                      ),
                    ),
                  ),
              ],
            ),
            child: Semantics(
              button: true,
              selected: shown,
              label: '${app.name}, ${appCaption(app)}',
              excludeSemantics: true,
              child: Pressable(
                onTap: () => openApp(context, app),
                onSecondaryTap: () => _tileActions(context, app),
                builder: (context, hovered) => AnimatedContainer(
                  duration: Motion.fast,
                  width: 28,
                  height: 28,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: shown
                        ? palette.selected
                        : hovered
                        ? palette.hover
                        : null,
                    borderRadius: BorderRadius.circular(Radii.control),
                    border: Border.all(
                      color: shown ? const Color(0x00000000) : palette.hairline,
                    ),
                  ),
                  child: Stack(
                    clipBehavior: Clip.none,
                    alignment: Alignment.center,
                    children: [
                      ExtensionGlyph(
                        icon: app.icon,
                        name: app.name,
                        color: shown || hovered
                            ? palette.text
                            : palette.textMuted,
                      ),
                      if (dot != null)
                        Positioned(right: -4, top: -4, child: _Dot(color: dot)),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// A 7px dot in a ring of the sidebar's color, as on a Claktory's tile.
class _Dot extends StatelessWidget {
  const _Dot({required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) => Container(
    width: 11,
    height: 11,
    alignment: Alignment.center,
    decoration: BoxDecoration(
      color: Palette.of(context).sidebar,
      shape: BoxShape.circle,
    ),
    child: Dot(color: color, size: 7),
  );
}

/// The dock's last tile: every installed app, in a floating list beside
/// it. An app dropped on it is pinned last.
class _LauncherTile extends StatefulWidget {
  const _LauncherTile();

  @override
  State<_LauncherTile> createState() => _LauncherTileState();
}

class _LauncherTileState extends State<_LauncherTile> {
  final _menu = OverlayPortalController();
  final _link = LayerLink();
  final _region = Object();

  static const _label = 'All apps';

  void _toggle() => setState(() {
    if (_menu.isShowing) {
      _menu.hide();
    } else {
      _menu.show();
    }
  });

  void _close() {
    if (_menu.isShowing) setState(_menu.hide);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return DragTarget<_PinDrag>(
      onAcceptWithDetails: (details) =>
          fireAndLog(AppScope.of(context).pinApp(details.data.appId)),
      builder: (context, candidates, _) => TapRegion(
        groupId: _region,
        onTapOutside: (_) => _close(),
        child: CompositedTransformTarget(
          link: _link,
          child: OverlayPortal(
            controller: _menu,
            overlayChildBuilder: (context) => CompositedTransformFollower(
              link: _link,
              targetAnchor: Alignment.bottomRight,
              followerAnchor: Alignment.bottomLeft,
              offset: const Offset(Space.s, 0),
              child: Align(
                alignment: Alignment.bottomLeft,
                child: TapRegion(
                  groupId: _region,
                  child: CallbackShortcuts(
                    bindings: {
                      const SingleActivator(LogicalKeyboardKey.escape): _close,
                    },
                    child: FocusScope(
                      autofocus: true,
                      child: _Floating(child: AppLauncherList(onDone: _close)),
                    ),
                  ),
                ),
              ),
            ),
            child: HoverCard(
              beside: true,
              card: (context) => const Text(_label),
              child: GlyphButton(
                label: _label,
                active: _menu.isShowing || candidates.isNotEmpty,
                onTap: _toggle,
                glyph: (color) => _lucide(
                  'layout-grid',
                  _menu.isShowing ? color : palette.textFaint,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A floating layer: the sidebar's color, a hairline, a 10px radius and
/// the floating shadow.
class _Floating extends StatelessWidget {
  const _Floating({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      width: 300,
      constraints: const BoxConstraints(maxHeight: 440),
      padding: const EdgeInsets.all(Space.xs),
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
        boxShadow: floatingShadow,
      ),
      child: child,
    );
  }
}

/// Every installed app, by name: a 44px row each with its icon, name and
/// state, opening it; Pin or Unpin on an app with views; then Manage
/// apps…, opening Settings → Apps. The launcher's list on a Mac, and the
/// sheet the phone's strip opens.
class AppLauncherList extends StatelessWidget {
  const AppLauncherList({super.key, required this.onDone});

  /// Called once a row has done what it does (to close the list).
  final VoidCallback onDone;

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final workspace = backend.workspace.workspace;
        final apps = [
          ...workspace.apps,
        ]..sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
        final pinned = workspace.pinnedApps.toSet();
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Flexible(
              child: ListView(
                shrinkWrap: true,
                children: [
                  for (final app in apps)
                    _LauncherRow(
                      app: app,
                      pinned: pinned.contains(app.id),
                      onDone: onDone,
                    ),
                ],
              ),
            ),
            Container(
              height: 1,
              margin: const EdgeInsets.symmetric(vertical: Space.xs),
              color: Palette.of(context).hairline,
            ),
            _ManageRow(
              onTap: () {
                onDone();
                _openAppSettings(context);
              },
            ),
          ],
        );
      },
    );
  }
}

class _LauncherRow extends StatelessWidget {
  const _LauncherRow({
    required this.app,
    required this.pinned,
    required this.onDone,
  });

  final AppInfo app;
  final bool pinned;
  final VoidCallback onDone;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final dot = appDot(app, palette);
    final row = Pressable(
      onTap: () {
        onDone();
        openApp(context, app);
      },
      builder: (context, hovered) => Container(
        constraints: const BoxConstraints(minHeight: 44),
        padding: const EdgeInsets.symmetric(horizontal: Space.s),
        decoration: BoxDecoration(
          color: hovered ? palette.hover : null,
          borderRadius: BorderRadius.circular(Radii.control),
        ),
        child: Row(
          children: [
            SizedBox.square(
              dimension: 20,
              child: Stack(
                clipBehavior: Clip.none,
                alignment: Alignment.center,
                children: [
                  ExtensionGlyph(
                    icon: app.icon,
                    name: app.name,
                    color: palette.text,
                  ),
                  if (dot != null)
                    Positioned(right: -5, top: -5, child: _Dot(color: dot)),
                ],
              ),
            ),
            const SizedBox(width: Space.m),
            Expanded(
              child: Semantics(
                button: true,
                label: '${app.name}, ${appCaption(app)}',
                excludeSemantics: true,
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      app.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: type.body.copyWith(color: palette.text),
                    ),
                    Text(
                      appCaption(app),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: type.caption.copyWith(
                        color: dot == palette.danger
                            ? palette.danger
                            : palette.textMuted,
                      ),
                    ),
                  ],
                ),
              ),
            ),
            // Only an app with something to open goes in the dock.
            if (app.views.isNotEmpty)
              GlyphButton(
                label: pinned ? 'Unpin ${app.name}' : 'Pin ${app.name}',
                active: pinned,
                onTap: () => fireAndLog(
                  AppScope.of(context).pinApp(app.id, pinned: !pinned),
                ),
                glyph: (color) => _lucide(pinned ? 'pin-off' : 'pin', color),
              ),
          ],
        ),
      ),
    );
    if (app.views.isEmpty || touchFirst(context)) return row;
    // Dragged onto the dock, it's pinned where it's dropped.
    return Draggable<_PinDrag>(
      data: _PinDrag(app.id),
      feedback: _DragFeedback(app: app),
      child: row,
    );
  }
}

class _ManageRow extends StatelessWidget {
  const _ManageRow({required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          constraints: const BoxConstraints(minHeight: 32),
          alignment: Alignment.centerLeft,
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          decoration: BoxDecoration(
            color: hovered ? palette.hover : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Text(
            'Manage apps…',
            style: TypeScale.of(context).body
                .copyWith(color: hovered ? palette.text : palette.textMuted),
          ),
        ),
      ),
    );
  }
}

/// On a phone, the dock as a strip for the sidebar (where Prosciutto puts
/// it): a 44pt chip with its icon and name for each pinned app, scrolling
/// sideways, then All apps, which opens the launcher's list as a sheet. A
/// long press on a chip unpins it or opens its settings. Nothing while
/// the backend offers no apps.
class AppStrip extends StatelessWidget {
  const AppStrip({super.key});

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final workspace = backend.workspace.workspace;
        if (!workspace.appsAvailable || workspace.apps.isEmpty) {
          return const SizedBox.shrink();
        }
        final shown = _shownApps(backend.workspace);
        return SizedBox(
          height: 44,
          child: ListView(
            scrollDirection: Axis.horizontal,
            padding: const EdgeInsets.symmetric(horizontal: Space.m),
            children: [
              for (final app in pinnedApps(workspace))
                Padding(
                  padding: const EdgeInsets.only(right: Space.s),
                  child: _Chip(
                    icon: app.icon,
                    label: app.name,
                    semantics: '${app.name}, ${appCaption(app)}',
                    selected: shown.contains(app.id),
                    dot: appDot(app, Palette.of(context)),
                    onTap: () => openApp(context, app),
                    onLongPress: () => _tileActions(context, app),
                  ),
                ),
              _Chip(
                icon: 'layout-grid',
                label: 'All apps',
                semantics: 'All apps',
                onTap: () => showModal(
                  context,
                  (context, close) => _LauncherSheet(close: close),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _LauncherSheet extends StatelessWidget {
  const _LauncherSheet({required this.close});

  final VoidCallback close;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'Apps',
          textAlign: TextAlign.center,
          style: TypeScale.of(context).emphasis.copyWith(color: palette.text),
        ),
        const SizedBox(height: Space.s),
        Flexible(child: AppLauncherList(onDone: close)),
      ],
    );
  }
}

class _Chip extends StatelessWidget {
  const _Chip({
    required this.icon,
    required this.label,
    required this.semantics,
    required this.onTap,
    this.onLongPress,
    this.selected = false,
    this.dot,
  });

  final String icon;
  final String label;
  final String semantics;
  final VoidCallback onTap;
  final VoidCallback? onLongPress;
  final bool selected;
  final Color? dot;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final dot = this.dot;
    return Semantics(
      button: true,
      selected: selected,
      label: semantics,
      excludeSemantics: true,
      child: GestureDetector(
        onLongPress: onLongPress,
        child: Pressable(
          onTap: onTap,
          builder: (context, pressed) => Container(
            padding: const EdgeInsets.symmetric(horizontal: Space.m),
            decoration: BoxDecoration(
              color: selected
                  ? palette.selected
                  : pressed
                  ? palette.hover
                  : null,
              borderRadius: BorderRadius.circular(22),
              border: Border.all(
                color: selected ? const Color(0x00000000) : palette.hairline,
              ),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                ExtensionGlyph(
                  icon: icon,
                  name: label,
                  color: selected ? palette.text : palette.textMuted,
                ),
                const SizedBox(width: Space.s),
                Text(
                  label,
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.text),
                ),
                if (dot != null) ...[
                  const SizedBox(width: Space.s),
                  Dot(color: dot, size: 7),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}
