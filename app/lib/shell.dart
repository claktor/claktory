import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_view.dart';
import 'backend.dart' show fireAndLog;
import 'characters/clock.dart';
import 'changelog.dart';
import 'map.dart';
import 'pairing.dart';
import 'marketplace.dart';
import 'account.dart';
import 'name_step.dart';
import 'phone.dart';
import 'controls.dart';
import 'extensions.dart';
import 'forms.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'rail.dart' show railWidth;
import 'scope.dart';
import 'settings_view.dart';
import 'sidebar.dart';
import 'state.dart';
import 'update.dart';

/// The window: the sidebar on the left, the selected agent in the middle under
/// the update banner, and its open extension panel on the right. Hosts the
/// overlay that menus and dialogs float in.
class Shell extends StatefulWidget {
  const Shell({super.key, this.rail});

  /// The Claktories rail at the window's far left (rail.dart); the phone
  /// shows its own list instead.
  final Widget? rail;

  @override
  State<Shell> createState() => _ShellState();
}

class _ShellState extends State<Shell> {
  late final _root = OverlayEntry(builder: _buildWindow);

  /// The sidebar's and the panel's widths while the person drags their
  /// edge; null otherwise, when they're the backend's (`Settings`).
  final _sidebarDrag = ValueNotifier<double?>(null);
  final _panelDrag = ValueNotifier<double?>(null);

  /// The width a drag started from.
  var _dragFrom = 0.0;

  @override
  void dispose() {
    _root.remove();
    _root.dispose();
    for (final waiting in _settling.values) {
      _store?.removeListener(waiting);
    }
    _sidebarDrag.dispose();
    _panelDrag.dispose();
    super.dispose();
  }

  /// Sends the dragged width once the drag ends (0 resets it to the
  /// default). The dragged width stays until the backend's settings say
  /// it, so the edge doesn't jump back meanwhile, or until it's refused.
  void _keep(
    ValueNotifier<double?> drag,
    UpdateSettings update,
    double Function(Settings settings) kept,
  ) {
    final store = _store = AppScope.of(context).workspace;
    final wanted = update.hasSidebarWidth()
        ? update.sidebarWidth
        : update.panelWidth;
    void settle() {
      if (kept(store.workspace.settings) != wanted) return;
      store.removeListener(settle);
      // Unless a newer drag took over.
      if (_settling[drag] == settle) {
        _settling.remove(drag);
        drag.value = null;
      }
    }

    _stopSettling(drag);
    _settling[drag] = settle;
    store.addListener(settle);
    settle();
    fireAndLog(
      AppScope.of(context).updateSettings(update).catchError((Object error) {
        if (_settling[drag] == settle) {
          store.removeListener(settle);
          _settling.remove(drag);
          drag.value = null;
        }
        throw error;
      }),
    );
  }

  /// What waits for each width the backend hasn't said yet, on [_store].
  final _settling = <ValueNotifier<double?>, VoidCallback>{};
  WorkspaceStore? _store;

  /// Stops waiting for [drag]'s width: a new drag takes over.
  void _stopSettling(ValueNotifier<double?> drag) {
    final waiting = _settling.remove(drag);
    if (waiting != null) _store?.removeListener(waiting);
  }

  Widget _buildWindow(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([
      AppScope.of(context).workspace,
      signInPutOff(AppScope.of(context)),
    ]),
    builder: (context, _) {
      final window = _layout(context);
      final step = asksSignIn(context)
          ? const SignInStep()
          : asksName(context)
          ? const NameStep()
          : null;
      if (step == null) return window;
      // The first launch asks the person to sign in, then their name, over
      // everything.
      return Stack(
        fit: StackFit.expand,
        children: [
          ExcludeFocus(child: window),
          NameStepBarrier(child: step),
        ],
      );
    },
  );

  Widget _layout(BuildContext context) {
    final palette = Palette.of(context);
    if (isPhoneLayout(context)) {
      return DefaultTextStyle(
        style: TextStyles.body.copyWith(color: palette.text),
        child: ColoredBox(color: palette.canvas, child: const PhoneShell()),
      );
    }
    return DefaultTextStyle(
      style: TextStyles.body.copyWith(color: palette.text),
      child: ColoredBox(
        color: palette.canvas,
        // What this LayoutBuilder repaints stays below it.
        child: RepaintBoundary(
          child: LayoutBuilder(
            builder: (context, constraints) => ListenableBuilder(
              listenable: Listenable.merge([
                AppScope.of(context).workspace,
                AppScope.scope(context).page,
                _sidebarDrag,
                _panelDrag,
              ]),
              builder: (context, _) {
                final scope = AppScope.scope(context);
                final store = scope.backend.workspace;
                final workspace = store.workspace;
                // The extension bar and panel belong to the agent view.
                final agent = scope.page.value == AppPage.agent
                    ? store.selectedAgent
                    : null;
                final panel = openPanelOf(workspace, agent);
                // The right side holds the open panel, if any. The
                // conversation keeps at least this much room; a narrower
                // window sets the panel aside.
                const minContent = 440.0;
                final settings = workspace.settings;
                final sidebar =
                    _sidebarDrag.value ??
                    keptWidth(
                      settings.sidebarWidth,
                      sidebarWidth,
                      sidebarMinWidth,
                      sidebarMaxWidth,
                    );
                final rail = widget.rail == null ? 0.0 : railWidth;
                final room =
                    constraints.maxWidth -
                    rail -
                    sidebar -
                    extensionBarWidth -
                    minContent;
                // The panel as wide as it was left, as far as there's room.
                final panelRoom = math.min(panelMaxWidth, room);
                final panelShown =
                    panel != null && agent != null && room >= panelMinWidth;
                final panelAt = math.min(
                  _panelDrag.value ??
                      keptWidth(
                        settings.panelWidth,
                        panelWidth,
                        panelMinWidth,
                        panelMaxWidth,
                      ),
                  panelRoom,
                );
                final Widget? pane = panelShown
                    ? ExtensionPanel(
                        key: ValueKey('${panel.id}/${agent.id}'),
                        extension: panel,
                        agent: agent,
                        width: panelAt,
                      )
                    : null;
                // Back and forward through the agents followed from messages,
                // as in a browser: ⌘[ and ⌘], or the mouse's side buttons.
                void back() {
                  if (agent != null) scope.history.goBack(agent.id);
                }

                void forward() {
                  if (agent != null) scope.history.goForward(agent.id);
                }

                // Each part on its own layer: anything that rebuilds under
                // this LayoutBuilder (a hover's fade, say) lays it out again,
                // which would otherwise repaint the whole window each frame.
                Widget part(Widget child) => RepaintBoundary(child: child);
                final railView = widget.rail;
                final row = Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    if (railView != null) part(railView),
                    part(Sidebar(width: sidebar)),
                    part(ExtensionBar(agent: agent)),
                    Expanded(
                      child: part(
                        updateOffered(workspace, scope.appBuild.version)
                            ? const Column(
                                crossAxisAlignment: CrossAxisAlignment.stretch,
                                children: [
                                  UpdateBanner(),
                                  Expanded(child: _Content()),
                                ],
                              )
                            : const _Content(),
                      ),
                    ),
                    if (pane != null) part(pane),
                  ],
                );
                return CallbackShortcuts(
                  bindings: {
                    const SingleActivator(
                      LogicalKeyboardKey.bracketLeft,
                      meta: true,
                    ): back,
                    const SingleActivator(
                      LogicalKeyboardKey.bracketRight,
                      meta: true,
                    ): forward,
                  },
                  child: Listener(
                    behavior: HitTestBehavior.translucent,
                    onPointerDown: (event) {
                      if (event.buttons & kBackMouseButton != 0) back();
                      if (event.buttons & kForwardMouseButton != 0) forward();
                    },
                    child: Stack(
                      children: [
                        Positioned.fill(child: row),
                        Positioned(
                          left: rail + sidebar - ResizeHandle.width / 2,
                          top: 0,
                          bottom: 0,
                          child: ResizeHandle(
                            key: const ValueKey('sidebar-edge'),
                            onDragStart: () {
                              _stopSettling(_sidebarDrag);
                              _dragFrom = sidebar;
                            },
                            onDrag: (dx) =>
                                _sidebarDrag.value = (_dragFrom + dx)
                                    .clamp(sidebarMinWidth, sidebarMaxWidth)
                                    .roundToDouble(),
                            onDragEnd: () {
                              final width = _sidebarDrag.value;
                              if (width == null) return;
                              _keep(
                                _sidebarDrag,
                                UpdateSettings(sidebarWidth: width),
                                (settings) => settings.sidebarWidth,
                              );
                            },
                            onReset: () {
                              _sidebarDrag.value = sidebarWidth;
                              _keep(
                                _sidebarDrag,
                                UpdateSettings(sidebarWidth: 0),
                                (settings) => settings.sidebarWidth,
                              );
                            },
                          ),
                        ),
                        if (panelShown)
                          Positioned(
                            right: panelAt - ResizeHandle.width / 2,
                            top: 0,
                            bottom: 0,
                            child: ResizeHandle(
                              key: const ValueKey('panel-edge'),
                              onDragStart: () {
                                _stopSettling(_panelDrag);
                                _dragFrom = panelAt;
                              },
                              onDrag: (dx) =>
                                  _panelDrag.value = (_dragFrom - dx)
                                      .clamp(
                                        panelMinWidth,
                                        math.max(panelMinWidth, panelRoom),
                                      )
                                      .roundToDouble(),
                              onDragEnd: () {
                                final width = _panelDrag.value;
                                if (width == null) return;
                                _keep(
                                  _panelDrag,
                                  UpdateSettings(panelWidth: width),
                                  (settings) => settings.panelWidth,
                                );
                              },
                              onReset: () {
                                _panelDrag.value = panelWidth;
                                _keep(
                                  _panelDrag,
                                  UpdateSettings(panelWidth: 0),
                                  (settings) => settings.panelWidth,
                                );
                              },
                            ),
                          ),
                      ],
                    ),
                  ),
                );
              },
            ),
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) =>
      // One clock for every agent character in the window.
      CharacterHost(child: Overlay(initialEntries: [_root]));
}

/// The selected agent, or what to do when there isn't one.
class _Content extends StatelessWidget {
  const _Content();

  @override
  Widget build(BuildContext context) {
    final scope = AppScope.scope(context);
    final backend = scope.backend;
    return ListenableBuilder(
      listenable: Listenable.merge([
        backend.workspace,
        backend.connected,
        backend.pairing,
        scope.page,
      ]),
      builder: (context, _) {
        // Another Claktory's code used, waiting to be allowed there: the
        // number to compare.
        final waiting = scope.connections.current;
        if (backend.pairing.value != null && waiting != null) {
          return PairingWaitView(
            onCancel: () => scope.connections.forget(waiting),
          );
        }
        switch (scope.page.value) {
          case AppPage.settings:
            return const SettingsView();
          case AppPage.marketplace:
            return const MarketplaceView();
          case AppPage.changelog:
            return const ChangelogView();
          case AppPage.map:
            return const MapView();
          case AppPage.agent:
        }
        final store = backend.workspace;
        final workspace = store.workspace;
        final agent = store.selectedAgent;
        if (agent != null) {
          // The whole conversation is faintly washed in the agent's color, so
          // it's always clear whose it is; it eases to the next agent's.
          return AnimatedContainer(
            duration: Motion.base,
            curve: Curves.easeOut,
            color: conversationTint(agent),
            child: AgentView(key: ValueKey(agent.id), agent: agent),
          );
        }
        if (!backend.connected.value) {
          return const _EmptyState(title: 'Connecting…');
        }
        final signIn = workspace.models.isEmpty;
        if (workspace.agents.isEmpty) {
          // Agents are added in projects, so start one if there's none.
          final noProjects = workspace.projects.isEmpty;
          return _EmptyState(
            title: 'No agents yet',
            body: noProjects
                ? 'Agents work in projects. Start a project, then add agents '
                      'to it.'
                : 'Add one with + Agent in a project.',
            action: noProjects ? 'New project' : null,
            onAction: noProjects
                ? () => showModal(
                    context,
                    (context, close) => CreateProjectForm(close: close),
                  )
                : null,
            signIn: signIn,
          );
        }
        return _EmptyState(title: 'Select an agent', signIn: signIn);
      },
    );
  }
}

class _EmptyState extends StatelessWidget {
  const _EmptyState({
    required this.title,
    this.body,
    this.action,
    this.onAction,
    this.signIn = false,
  });

  final String title;
  final String? body;
  final String? action;
  final VoidCallback? onAction;

  /// No provider is signed in, so no model can run: say so and offer
  /// signing in.
  final bool signIn;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // With no header bar, the window is dragged by this band.
        const TitleSpace(),
        Expanded(
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 360),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(
                    title,
                    textAlign: TextAlign.center,
                    style: TextStyles.emphasis.copyWith(
                      color: body == null && action == null
                          ? palette.textFaint
                          : palette.text,
                      fontWeight: body == null && action == null
                          ? FontWeight.w400
                          : FontWeight.w600,
                      fontSize: body == null && action == null ? 13 : 15,
                    ),
                  ),
                  if (body != null) ...[
                    const SizedBox(height: Space.s),
                    Text(
                      body!,
                      textAlign: TextAlign.center,
                      style: TextStyles.body.copyWith(color: palette.textMuted),
                    ),
                  ],
                  if (action != null) ...[
                    const SizedBox(height: Space.l),
                    Button(label: action!, onPressed: onAction, primary: true),
                  ],
                  if (signIn) ...[
                    const SizedBox(height: Space.xl),
                    Text(
                      'Sign in to a provider so your agents can work.',
                      textAlign: TextAlign.center,
                      style: TextStyles.body.copyWith(color: palette.textMuted),
                    ),
                    const SizedBox(height: Space.s),
                    Button(
                      label: 'Sign in…',
                      onPressed: () => AppScope.openSettings(context),
                    ),
                  ],
                ],
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// A width the person left ([kept], `Settings`), kept in [min]–[max]; 0
/// (never set) is [fallback], the default.
double keptWidth(double kept, double fallback, double min, double max) =>
    kept > 0 && kept.isFinite ? kept.clamp(min, max) : fallback;
