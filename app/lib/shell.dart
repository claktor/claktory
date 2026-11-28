import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_view.dart';
import 'android/pair_step.dart';
import 'app_dock.dart' show AppHandOver;
import 'backend.dart' show Backend, fireAndLog;
import 'characters/clock.dart';
import 'changelog.dart';
import 'chats.dart' show ChatView, selectedChat;
import 'compatibility.dart';
import 'connect_step.dart';
import 'connection.dart' show Connection, Link;
import 'engine.dart' show WhenAppQuitsStep, asksWhenAppQuits;
import 'explorer.dart' show FilesPage;
import 'claktory_problem.dart'
    show
        CantConnectPage,
        ClaktorySignedOutPage,
        NoLongerAllowedPage,
        removeClaktory;
import 'history/export_job.dart' show ReplayJobBanner;
import 'map.dart';
import 'monitors_overview.dart' show MonitorsOverview;
import 'pairing.dart';
import 'marketplace.dart';
import 'account.dart';
import 'model_step.dart';
import 'kept.dart';
import 'mobile/device.dart' show windowControls;
import 'mobile/back.dart' show BackHandle, CallbackBackToken, PhoneBack;
import 'mobile/presentation.dart' show SheetOverlay;
import 'move.dart' show BackendMove, MoveGate;
import 'name_step.dart';
import 'older_app.dart' show OlderAppPage, olderAppOf;
import 'phone.dart';
import 'pipes.dart';
import 'controls.dart';
import 'extensions.dart';
import 'forms.dart';
import 'identity.dart';
import 'ipad/pair_step.dart';
import 'ipad/focus.dart' show IPadFocus;
import 'ipad/workbench.dart' show WindowKeyboard, Workbench, workbenchRailWidth;
import 'protected_places.dart' show ProtectedPlacesAtLaunch;
import 'protocol/factory/v1/factory.pb.dart';
import 'rail.dart' show railWidth;
import 'scope.dart';
import 'settings_view.dart';
import 'setup.dart';
import 'needs_you_focus.dart' show FocusModeLayer;
import 'sidebar.dart';
import 'state.dart';
import 'update.dart';
import 'screens.dart';
import 'inbox.dart' show InboxView;
import 'usage_view.dart';
import 'your_claktories.dart'
    show DeviceBanners, OtherAccountView, SignInRequiredView;
import 'primary_key.dart';

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
  /// The sidebar's and the panel's widths while the person drags their
  /// edge; null otherwise, when they're the backend's (`Settings`).
  final _sidebarDrag = ValueNotifier<double?>(null);
  final _panelDrag = ValueNotifier<double?>(null);

  /// The width a drag started from.
  var _dragFrom = 0.0;

  @override
  void dispose() {
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

  Widget _buildSteps(BuildContext context) => ListenableBuilder(
    listenable: Listenable.merge([
      AppScope.of(context).workspace,
      // A Claktory that turns this device away: its screen takes the
      // first launch's questions' place at once, rather
      // than the name step waiting there, "Connecting…", until a relaunch.
      AppScope.of(context).signInRequired,
      AppScope.of(context).otherAccount,
      // Turned away or gone: a step that asked over the Claktory (its own
      // sign-in) leaves with it, never left over its problem page
      // .
      AppScope.of(context).connected,
      AppScope.of(context).refused,
      signInPutOff(AppScope.of(context)),
      modelStepStarted(AppScope.of(context)),
      AppScope.scope(context).page,
    ]),
    builder: (context, _) {
      // Setting this Mac up comes first, over its own Claktory only.
      final setup = SetupScope.of(context);
      final setupStep =
          setup != null && setup.showing && AppScope.of(context).target == null
          ? SetupStep(setup: setup)
          : null;
      // What's new, opened from the update banner, shows instead of the
      // first launch's questions until it's closed. First of them: this
      // app and its Claktory can't work together (compatibility.dart).
      final mismatch = mismatchIn(context);
      // A Mac with no Claktory of its own connects to one first; an iPad
      // the phone's way, which scans and signs in too.
      final question = asksToConnect(context)
          ? defaultTargetPlatform == TargetPlatform.iOS &&
                    deviceIdiom(context) == Idiom.pad
                ? const IPadPairStep()
                : defaultTargetPlatform == TargetPlatform.android
                ? const AndroidPairStep()
                : const ConnectStep()
          : AppScope.pageShown(context, AppPage.changelog)
          ? null
          : mismatch != null
          ? VersionStep(which: mismatch)
          : asksSignIn(context)
          ? const SignInStep()
          : asksName(context)
          ? const NameStep()
          // What quitting the Mac app does.
          : asksWhenAppQuits(context)
          ? const WhenAppQuitsStep()
          : asksModel(context)
          ? const ModelStep()
          : null;
      final step = setupStep ?? question;
      // Over the questions the update banner stays in reach: they don't
      // block updating.
      final laid = _layout(
        context,
        bannerOnTop: setupStep == null && question != null,
        covered: step != null,
      );
      // Focus mode over the window, which stays as it was under it;
      // a phone's Needs you is its own. An iPad's window keeps it at every
      // width, so the sitting and its card live through a resize into
      // Slide Over or ⅓ Split View and back, where IPadFocus
      // shows the phone's one-card screen.
      final window = isPhoneLayout(context) && !_workbench(context)
          ? laid
          : FocusModeLayer(
              // An iPad's: laid out for its window (ipad/focus.dart).
              focus: _workbench(context)
                  ? (session, leave) =>
                        IPadFocus(session: session, leave: leave)
                  : null,
              child: laid,
            );
      // What agents show the person opens here, unless a step covers it.
      if (step == null) return ScreenOpener(covered: false, child: window);
      // The first launch asks the person to sign in, then their name, over
      // everything but the rail: switching to another Claktory always
      // works, so one that can't go on (too old, say) never traps them.
      final rail = widget.rail == null || isPhoneLayout(context)
          ? 0.0
          : _workbench(context)
          ? workbenchRailWidth
          : railWidth;
      return ScreenOpener(
        covered: true,
        child: Stack(
          fit: StackFit.expand,
          children: [
            // Dimmed by the step's scrim, which takes every touch but the
            // rail's, and out of focus. A phone's window (no rail) is
            // wholly inert until the step is done: no touches, and
            // VoiceOver reads only the step and what's drawn above it.
            ExcludeFocus(
              child: isPhoneLayout(context)
                  ? IgnorePointer(child: window)
                  : window,
            ),
            Positioned.fill(
              left: rail,
              child: BlockSemantics(
                blocking: isPhoneLayout(context),
                child: NameStepBarrier(child: step),
              ),
            ),
            // A phone has no rail: the step's page (StepCard) has the way
            // back to its Claktories along its top.
          ],
        ),
      );
    },
  );

  /// The window. With [bannerOnTop], the update banner is drawn above
  /// what's laid over the window, so it's clickable over a step. [covered]
  /// by a step, VoiceOver reaches only what still works under it: the rail
  /// and the update banner.
  Widget _layout(
    BuildContext context, {
    bool bannerOnTop = false,
    bool covered = false,
  }) {
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
                // The panel belongs to the agent view (a pipe, shown
                // instead, has none).
                final agent =
                    scope.page.value == AppPage.agent &&
                        selectedPipe(workspace) == null &&
                        selectedChat(workspace) == null
                    ? store.selectedAgent
                    : null;
                final panel = openPanelOf(workspace, agent);
                // The right side holds the open panel, if any. The
                // conversation keeps at least this much room; in a narrower
                // window the panel comes over it instead.
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
                final room = constraints.maxWidth - rail - sidebar - minContent;
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
                // Under a step: dimmed, and silent to VoiceOver.
                Widget behind(Widget child) =>
                    covered ? ExcludeSemantics(child: child) : child;
                final railView = widget.rail;
                final offered = updateOffered(
                  workspace,
                  scope.appBuild.version,
                );
                // An iPhone's or iPad's app behind a Claktory that's up to
                // date: a quiet note in the banner's place, which stays laid
                // out when closed so what's under it keeps its state.
                final note =
                    !offered &&
                    appBehind(workspace, scope.appBuild.version) != null;
                final content = offered || note
                    ? Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          note
                              ? const AppBehindNote()
                              : bannerOnTop
                              ? const _LiftedBanner()
                              : const UpdateBanner(),
                          Expanded(
                            child: behind(
                              const DeviceBanners(
                                child: ReplayJobBanner(child: _Content()),
                              ),
                            ),
                          ),
                        ],
                      )
                    : behind(
                        const DeviceBanners(
                          child: ReplayJobBanner(child: _Content()),
                        ),
                      );
                // An iPad's window wide enough for panes: the workbench
                // (ipad/workbench.dart) lays out the same parts, finger-sized,
                // and decides where the panel fits.
                if (railView != null && _workbench(context)) {
                  final kept = keptWidth(
                    settings.panelWidth,
                    panelWidth,
                    panelMinWidth,
                    panelMaxWidth,
                  );
                  return _History(
                    back: back,
                    forward: forward,
                    child: Workbench(
                      rail: railView,
                      sidebar: (width) => behind(Sidebar(width: width)),
                      sidebarWidth: sidebar,
                      content: content,
                      panel: panel != null && agent != null
                          ? behind(
                              ExtensionPanel(
                                key: ValueKey('${panel.id}/${agent.id}'),
                                extension: panel,
                                agent: agent,
                                width: kept,
                              ),
                            )
                          : null,
                      panelWidth: kept,
                      agent: agent,
                    ),
                  );
                }
                // An on-screen keyboard (an Android tablet's) covers the
                // rail and the sidebar, as on an iPad; the content and the
                // pane end above it, so the message box being written in
                // and Send stay in sight. What's inside sees no keyboard
                // left to avoid; what fills the window from inside (an
                // expanded pane) still knows it. Always wrapped, even with
                // no keyboard (a Mac), so its coming and going never
                // rebuilds the conversation and takes the focus away.
                final keyboard = MediaQuery.viewInsetsOf(context).bottom;
                Widget aboveKeyboard(Widget child) => Padding(
                  padding: EdgeInsets.only(bottom: keyboard),
                  child: child,
                );
                final row = WindowKeyboard(
                  height: keyboard,
                  child: MediaQuery.removeViewInsets(
                    context: context,
                    removeBottom: true,
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        if (railView != null) part(railView),
                        // The sidebar follows the Workspace on its own.
                        part(
                          behind(
                            Kept(
                              inputs: [sidebar],
                              builder: (_) => Sidebar(width: sidebar),
                            ),
                          ),
                        ),
                        Expanded(child: aboveKeyboard(part(content))),
                        if (pane != null) aboveKeyboard(part(behind(pane))),
                      ],
                    ),
                  ),
                );
                return _History(
                  back: back,
                  forward: forward,
                  child: _ClearOfSystemBars(
                    child: Stack(
                      children: [
                        Positioned.fill(child: row),
                        Positioned(
                          left:
                              rail +
                              sidebar -
                              ResizeHandle.widthOf(context) / 2,
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
                            right: panelAt - ResizeHandle.widthOf(context) / 2,
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
                        // No room beside the conversation: over it, from the
                        // right edge, as an iPad's
                        // workbench shows it.
                        if (panel != null && agent != null && !panelShown)
                          Positioned(
                            key: const ValueKey('panel-over'),
                            right: 0,
                            top: 0,
                            bottom: keyboard,
                            child: part(
                              behind(
                                DecoratedBox(
                                  decoration: const BoxDecoration(
                                    boxShadow: floatingShadow,
                                  ),
                                  child: EscClosesPanel(
                                    agent: agent,
                                    child: ExtensionPanel(
                                      key: ValueKey('${panel.id}/${agent.id}'),
                                      extension: panel,
                                      agent: agent,
                                      // At most 85% of the window, as on an
                                      // iPad.
                                      width: math.min(
                                        keptWidth(
                                          settings.panelWidth,
                                          panelWidth,
                                          panelMinWidth,
                                          panelMaxWidth,
                                        ),
                                        constraints.maxWidth * 0.85,
                                      ),
                                    ),
                                  ),
                                ),
                              ),
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

  /// Whether the window is an iPad's wide enough for panes: the
  /// workbench's.
  static bool _workbench(BuildContext context) =>
      defaultTargetPlatform == TargetPlatform.iOS &&
      deviceIdiom(context) == Idiom.pad;

  @override
  Widget build(BuildContext context) =>
      // One clock for every agent character in the window.
      CharacterHost(
        // While the agents are paused for a move to another computer, its
        // screen takes the window, all but the rail: the backend serves
        // nothing else then. It takes the window's overlay with it, and
        // with that every sheet, dialog, menu and popover in it (and their
        // places in Back): nothing shown before the pause outlives it, and
        // the window that comes back has none.
        child: MoveGate(
          source: BackendMove(AppScope.of(context)),
          rail: isPhoneLayout(context) ? null : widget.rail,
          child: _WindowOverlay(builder: _buildSteps),
        ),
      );
}

/// The window's overlay, that menus, dialogs, sheets and popovers float in
/// ([SheetOverlay]): made with the window and gone with it.
class _WindowOverlay extends StatefulWidget {
  const _WindowOverlay({required this.builder});

  final WidgetBuilder builder;

  @override
  State<_WindowOverlay> createState() => _WindowOverlayState();
}

class _WindowOverlayState extends State<_WindowOverlay> {
  late final _root = OverlayEntry(
    builder: (context) => SheetOverlay(
      overlay: Overlay.of(context),
      // A Mac's places macOS protects, never checked: the sheet once.
      child: ProtectedPlacesAtLaunch(
        // An app's view open as the phone's layout went opens again here.
        child: AppHandOver(child: Builder(builder: widget.builder)),
      ),
    ),
  );

  @override
  void dispose() {
    _root.remove();
    _root.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Overlay(initialEntries: [_root]);
}

/// The selected agent or pipe, or what to do when there isn't one.
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
        backend.signInRequired,
        backend.refused,
        backend.otherAccount,
        scope.page,
      ]),
      builder: (context, _) {
        // Another account's Claktory: what it said, in place of
        // it, before anything that would ask this device to sign in.
        if (backend.otherAccount.value != null) {
          return const OtherAccountView();
        }
        final waiting = scope.connections.current;
        // Another Claktory that lets this device in only once it's signed
        // in to the person's account.
        if (backend.signInRequired.value != null && waiting != null) {
          return SignInRequiredView(
            // Its Remove… asks first, with the rail's question, never at
            // once.
            onCancel: waiting.fromAccount
                ? () => unawaited(removeClaktory(context, waiting))
                : null,
          );
        }
        // Another Claktory's code used, waiting to be allowed there: the
        // number to compare.
        if (backend.pairing.value != null && waiting != null) {
          return PairingWaitView(
            onCancel: () => scope.connections.forget(waiting),
          );
        }
        final page = switch (scope.page.value) {
          AppPage.settings => const SettingsView(),
          AppPage.marketplace => const MarketplaceView(),
          AppPage.changelog => const ChangelogView(),
          AppPage.map => const MapView(),
          AppPage.usage => const UsageView(),
          AppPage.monitors => const MonitorsOverview(),
          AppPage.inbox => const InboxView(),
          AppPage.files => FilesPage(
            key: ValueKey('files/${backend.filesPage.value}'),
            projectId: backend.filesPage.value,
          ),
          AppPage.agent => null,
        };
        if (page != null) return _PageBack(child: page);
        // Turned away: what was open can't be used any more, so it says
        // so instead.
        if (turnedAway(backend)) return const NoAgentView();
        final store = backend.workspace;
        final workspace = store.workspace;
        // One of the person's chats, in place of a conversation.
        final chat = selectedChat(workspace);
        if (chat != null) {
          return ChatView(key: ValueKey('chat/${chat.id}'), chat: chat);
        }
        // A pipe to another Claktory, opened from its project.
        final pipe = selectedPipe(workspace);
        if (pipe != null) {
          return PipeView(key: ValueKey('pipe/${pipe.id}'), pipe: pipe);
        }
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
        return const NoAgentView();
      },
    );
  }
}

/// A page (Settings, Usage…) in place of the conversation on a wide
/// window: Android's Back closes it, back to the conversation, after any
/// dialog or sheet over it. A phone's pages are its drawer's own.
class _PageBack extends StatefulWidget {
  const _PageBack({required this.child});

  final Widget child;

  @override
  State<_PageBack> createState() => _PageBackState();
}

class _PageBackState extends State<_PageBack> {
  PhoneBack? _back;
  BackHandle? _handle;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final back = isPhoneLayout(context) ? null : PhoneBack.maybeOf(context);
    if (identical(back, _back)) return;
    _handle?.release();
    _back = back;
    _handle = back?.registerNavigation(
      CallbackBackToken(
        onBack: () {
          if (mounted) AppScope.closePage(context);
        },
      ),
    );
  }

  @override
  void dispose() {
    _handle?.release();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

/// Whether the Claktory turned this device away (a code that expired, a
/// device removed there): it won't connect by trying, and nothing it shows
/// can be used until the person adds this device again.
bool turnedAway(Backend backend) =>
    backend.refused.value != null && !backend.connected.value;

/// What shows with no agent chosen: "Connecting…" (or, as a problem page,
/// "Can't connect to …", turned away or signed out: devices-signin.md §4),
/// how to start when there are no agents, or "Select an agent", with
/// signing in when no model can run.
class NoAgentView extends StatelessWidget {
  const NoAgentView({super.key});

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: Listenable.merge([
        backend.workspace,
        backend.connected,
        backend.refused,
        backend.olderApp,
        // The drawer's "Can't connect" shows here too.
        Connection.of(backend),
      ]),
      builder: (context, _) {
        final workspace = backend.workspace.workspace;
        // Turned away (a code that expired, a device removed there): it
        // won't connect by trying, so it says so, as the drawer does, with
        // the way to another Claktory.
        if (turnedAway(backend)) {
          // Signed out of the person's account (its sign-in lost to a
          // failing disk, say), it lets this device in
          // again once it's signed in: a new code would pair it twice. As
          // factoryd's `remote::SIGNED_OUT` says it.
          const signedOut = 'was signed out of your Claktory account';
          if (backend.refused.value?.contains(signedOut) ?? false) {
            return ClaktorySignedOutPage(backend: backend);
          }
          return NoLongerAllowedPage(backend: backend);
        }
        // An update that failed went back to a version this app can't
        // reach: its own app is the way back.
        if (olderAppOf(backend) case final offer?
            when !backend.connected.value) {
          return OlderAppPage(offer: offer);
        }
        // Until this Claktory's workspace is here, nothing is known about
        // it: an empty one isn't "no agents yet".
        if (!backend.connected.value || !backend.workspace.received) {
          // Once it says it can't connect, the pane says so too, not
          // "Connecting…" behind the drawer's "Can't connect".
          if (Connection(backend).link == Link.failing) {
            return CantConnectPage(backend: backend);
          }
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
                : "Choose Add an agent… in a project's ⋯ menu.",
            action: noProjects ? 'New project' : null,
            onAction: noProjects
                ? () => askRivetOrForm(
                    context,
                    'Set up a new project for ',
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
          child: _Scrolls(
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
                      onPressed: () => AppScope.openProviders(context),
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

/// The update banner in its place, drawn above the window's overlay (over
/// a first-launch step and its scrim) so it can still be clicked. Its
/// place keeps a hidden copy, for its height.
class _LiftedBanner extends StatefulWidget {
  const _LiftedBanner();

  @override
  State<_LiftedBanner> createState() => _LiftedBannerState();
}

class _LiftedBannerState extends State<_LiftedBanner> {
  final _portal = OverlayPortalController();
  final _link = LayerLink();

  @override
  void initState() {
    super.initState();
    _portal.show();
  }

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) => OverlayPortal(
      controller: _portal,
      overlayChildBuilder: (context) => Align(
        alignment: Alignment.topLeft,
        child: CompositedTransformFollower(
          link: _link,
          // Its own height, as in its place in the window's column: given
          // the overlay's height, an iPad's banner filled it, over the
          // whole step.
          child: UnconstrainedBox(
            constrainedAxis: Axis.horizontal,
            alignment: Alignment.topLeft,
            child: SizedBox(
              width: constraints.maxWidth,
              child: const UpdateBanner(),
            ),
          ),
        ),
      ),
      child: CompositedTransformTarget(
        link: _link,
        child: const IgnorePointer(
          child: ExcludeSemantics(
            child: Visibility(
              visible: false,
              maintainSize: true,
              maintainAnimation: true,
              maintainState: true,
              child: UpdateBanner(),
            ),
          ),
        ),
      ),
    ),
  );
}

/// [child] in the middle; on a phone, where large text can make it taller
/// than the screen, scrolling then.
class _Scrolls extends StatelessWidget {
  const _Scrolls({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    if (!isPhoneLayout(context)) return Center(child: child);
    return LayoutBuilder(
      builder: (context, constraints) => SingleChildScrollView(
        padding: const EdgeInsets.symmetric(horizontal: Space.l),
        child: ConstrainedBox(
          constraints: BoxConstraints(minHeight: constraints.maxHeight),
          child: Center(child: child),
        ),
      ),
    );
  }
}

/// Back and forward through the agents followed from messages, as in a
/// browser: ⌘[ and ⌘], or the mouse's side buttons.
class _History extends StatelessWidget {
  const _History({
    required this.back,
    required this.forward,
    required this.child,
  });

  final VoidCallback back;
  final VoidCallback forward;
  final Widget child;

  @override
  Widget build(BuildContext context) => CallbackShortcuts(
    bindings: {
      primaryKey(LogicalKeyboardKey.bracketLeft): back,
      primaryKey(LogicalKeyboardKey.bracketRight): forward,
      // Windows' and Linux's own Back and Forward, as their browsers have.
      if (!primaryIsMeta) ...{
        const SingleActivator(LogicalKeyboardKey.arrowLeft, alt: true): back,
        const SingleActivator(LogicalKeyboardKey.arrowRight, alt: true):
            forward,
      },
    },
    child: Listener(
      behavior: HitTestBehavior.translucent,
      onPointerDown: (event) {
        if (event.buttons & kBackMouseButton != 0) back();
        if (event.buttons & kForwardMouseButton != 0) forward();
      },
      child: child,
    ),
  );
}

/// The wide layout clear of the system's bars at the top and the sides,
/// as an Android tablet draws its window under them: the status bar's
/// clock and battery would sit over the header's controls. The window
/// still reaches the bottom edge, under the navigation bar, where what's
/// inside keeps clear of it by the bottom padding it's left with. Window
/// controls the native side adds to the top padding are left to what
/// goes beside them, as an iPad's [SafeWindow] does.
class _ClearOfSystemBars extends StatelessWidget {
  const _ClearOfSystemBars({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: windowControls,
    builder: (context, controls, child) {
      final padding = MediaQuery.paddingOf(context);
      return Padding(
        padding: EdgeInsets.fromLTRB(
          padding.left,
          math.max(0.0, padding.top - controls.top),
          padding.right,
          0,
        ),
        child: child,
      );
    },
    child: MediaQuery.removePadding(
      context: context,
      removeLeft: true,
      removeTop: true,
      removeRight: true,
      child: child,
    ),
  );
}
