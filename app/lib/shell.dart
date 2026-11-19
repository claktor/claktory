import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_view.dart';
import 'characters/clock.dart';
import 'changelog.dart';
import 'marketplace.dart';
import 'controls.dart';
import 'explorer.dart';
import 'extensions.dart';
import 'forms.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'settings_view.dart';
import 'sidebar.dart';
import 'update.dart';

/// The window: the sidebar on the left, the selected agent in the middle under
/// the update banner, and the focused project's files on the right. Hosts the
/// overlay that menus and dialogs float in.
class Shell extends StatefulWidget {
  const Shell({super.key});

  @override
  State<Shell> createState() => _ShellState();
}

class _ShellState extends State<Shell> {
  late final _root = OverlayEntry(builder: _buildWindow);

  @override
  void dispose() {
    _root.remove();
    _root.dispose();
    super.dispose();
  }

  Widget _buildWindow(BuildContext context) {
    final palette = Palette.of(context);
    return DefaultTextStyle(
      style: TextStyles.body.copyWith(color: palette.text),
      child: ColoredBox(
        color: palette.canvas,
        child: LayoutBuilder(
          builder: (context, constraints) => ListenableBuilder(
            listenable: Listenable.merge([
              AppScope.of(context).workspace,
              AppScope.scope(context).page,
            ]),
            builder: (context, _) {
              final scope = AppScope.scope(context);
              final store = scope.backend.workspace;
              final workspace = store.workspace;
              Project? focused;
              for (final project in workspace.projects) {
                if (project.id == workspace.focusedProjectId) focused = project;
              }
              // The extension bar and panel belong to the agent view.
              final agent = scope.page.value == AppPage.agent
                  ? store.selectedAgent
                  : null;
              final panel = openPanelOf(workspace, agent);
              // The right side holds one pane: the open panel, else the
              // focused project's explorer. The conversation keeps at least
              // this much room; a narrower window sets the pane aside.
              const minContent = 440.0;
              final room =
                  constraints.maxWidth -
                  sidebarWidth -
                  extensionBarWidth -
                  minContent;
              final Widget? pane = panel != null
                  ? (room >= panelWidth
                        ? ExtensionPanel(
                            key: ValueKey('${panel.id}/${agent!.id}'),
                            extension: panel,
                            agent: agent,
                          )
                        : null)
                  : focused != null && room >= explorerWidth
                  ? ExplorerPane(key: ValueKey(focused.id), project: focused)
                  : null;
              // Back and forward through the agents followed from messages,
              // as in a browser: ⌘[ and ⌘], or the mouse's side buttons.
              void back() {
                if (agent != null) scope.history.goBack(agent.id);
              }

              void forward() {
                if (agent != null) scope.history.goForward(agent.id);
              }

              final row = Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  const Sidebar(),
                  ExtensionBar(agent: agent),
                  Expanded(
                    child: updateOffered(workspace, scope.appBuild.version)
                        ? const Column(
                            crossAxisAlignment: CrossAxisAlignment.stretch,
                            children: [
                              UpdateBanner(),
                              Expanded(child: _Content()),
                            ],
                          )
                        : const _Content(),
                  ),
                  ?pane,
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
                  child: row,
                ),
              );
            },
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
        scope.page,
      ]),
      builder: (context, _) {
        switch (scope.page.value) {
          case AppPage.settings:
            return const SettingsView();
          case AppPage.marketplace:
            return const MarketplaceView();
          case AppPage.changelog:
            return const ChangelogView();
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
            body:
                'An agent has a name and a prompt, and keeps its '
                'conversation. Agents work in a project, in its folder: '
                '${noProjects ? 'start one, then add' : 'add'} agents with '
                "the project's + Agent.",
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
                      'No model available',
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
