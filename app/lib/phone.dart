import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_view.dart';
import 'backend.dart';
import 'changelog.dart';
import 'chats.dart' show ChatView, selectedChat;
import 'claktory_identity.dart' show claktoryName;
import 'connections.dart';
import 'controls.dart';
import 'explorer.dart';
import 'extensions.dart';
import 'identity.dart';
import 'ipad/columns.dart' show compactRoutes;
import 'ipad/workbench.dart' show WorkbenchFocus, WorkbenchScope;
import 'layout.dart';
import 'map.dart';
import 'marketplace.dart';
import 'mobile/back.dart' show PageTarget;
import 'mobile/drawer.dart';
import 'mobile/extension_list.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart' show PhoneType, TypeScale, withoutInsets;
import 'mobile/navigation.dart';
import 'mobile/platform_policy.dart';
import 'mobile/presentation.dart'
    show PhoneSheetHost, SheetOverlay, presentSheet;
import 'pairing.dart';
import 'panes.dart' show paneGlyph, paneTitle;
import 'pipes.dart' show PipeView, selectedPipe;
import 'protocol/factory/v1/factory.pb.dart';
import 'scanner.dart';
import 'screens.dart' show ScreenOpener;
import 'scope.dart';
import 'settings_view.dart';
import 'shell.dart' show NoAgentView;
import 'pane_chrome.dart';
import 'viewers/registry.dart';
import 'inbox.dart' show InboxView;
import 'usage_view.dart';
import 'your_claktories.dart';

/// The app at a phone's size (docs/IDENTITY.md, "Phone"): the selected
/// conversation (an agent's, or a pipe's) fills the screen; the drawer
/// ([PhoneDrawerHost]) holds the Claktories and this one's projects,
/// agents and pipes; Settings, Files, a viewer and the other pages are
/// pushed over the conversation with iOS's Back; panels open as sheets;
/// and the message box rides above the keyboard. A phone with nothing
/// paired opens to pairing.
class PhoneShell extends StatelessWidget {
  const PhoneShell({super.key});

  @override
  Widget build(BuildContext context) {
    // Over a Claktory's window the drawer is the list of Claktories'
    // (rail.dart), so it stays as Claktories switch; else its own.
    if (PhoneDrawerHost.of(context) != null) return const _PhoneScreens();
    return const PhoneDrawerHost(child: _PhoneScreens());
  }
}

/// A page pushed over the conversation: [id] tells it apart; [alive] says
/// whether what it shows is still there (a pane closed on another device
/// takes its page with it).
class _Entry {
  _Entry({
    required this.id,
    required this.builder,
    this.overKeyboard = false,
    this.fullscreenDialog = false,
    this.alive,
    this.done,
  }) : serial = ++_pushed;

  /// How many pages have been pushed: [serial] tells this push apart from
  /// another of the same [id] (Back's [PageTarget]).
  static var _pushed = 0;

  final String id;
  final int serial;
  final WidgetBuilder builder;
  final bool overKeyboard;
  final bool fullscreenDialog;
  final bool Function()? alive;
  final Completer<Object?>? done;
}

class _PhoneScreens extends StatefulWidget {
  const _PhoneScreens();

  @override
  State<_PhoneScreens> createState() => _PhoneScreensState();
}

class _PhoneScreensState extends State<_PhoneScreens> implements PhonePages {
  final _navigator = GlobalKey<NavigatorState>();

  /// What's pushed over the conversation, bottom first.
  final _stack = <_Entry>[];

  PhoneDrawerHostState? _host;
  var _attached = false;

  /// Where an agent's show_screen says it opened a conversation
  /// (screens.dart): it comes to the front.
  Listenable? _opened;
  ValueNotifier<AppPage>? _page;

  /// The drawer opened by itself once, for a Claktory with nothing chosen.
  var _offered = false;

  /// Whose extension's panel shows, so dismissing it closes that one even
  /// as it goes down.
  String? _panelAgent;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final host = PhoneDrawerHost.of(context);
    if (host != _host) {
      if (_attached) _host?.detach(this);
      _attached = false;
      _host = host;
    }
    final opened = ScreenOpener.conversations(context);
    if (opened != _opened) {
      _opened?.removeListener(_toFront);
      _opened = opened?..addListener(_toFront);
    }
    final page = AppScope.scope(context).page;
    if (page != _page) {
      _page?.removeListener(_pageChanged);
      _page = page..addListener(_pageChanged);
      _syncPage();
    }
  }

  @override
  void dispose() {
    if (_attached) _host?.detach(this);
    _opened?.removeListener(_toFront);
    _page?.removeListener(_pageChanged);
    for (final entry in _stack) {
      entry.done?.complete();
    }
    super.dispose();
  }

  /// After the frame when told during a build; else now.
  void _soon(VoidCallback action) {
    if (SchedulerBinding.instance.schedulerPhase ==
        SchedulerPhase.persistentCallbacks) {
      SchedulerBinding.instance.addPostFrameCallback((_) {
        if (mounted) action();
      });
    } else {
      action();
    }
  }

  void _changed() => _host?.pagesChanged();

  void _toFront() {
    _host?.closeDrawer();
    popAll();
  }

  void _pageChanged() {
    setState(_syncPage);
    if (_page?.value != AppPage.agent) _host?.closeDrawer();
    _changed();
  }

  /// Settings, the Map and the other pages: the one showing is a page over
  /// the conversation (one at a time, as on a Mac).
  void _syncPage() {
    final page = _page?.value ?? AppPage.agent;
    final id = page == AppPage.agent ? null : 'page:${page.name}';
    // Another page closes this one, and what it pushed (Settings' sections,
    // 'route:settings:…') with it.
    final gone = [
      for (final entry in _stack)
        if (entry.id.startsWith('page:') && entry.id != id ||
            entry.id.startsWith('route:') &&
                AppPage.values.any(
                  (other) =>
                      other != AppPage.agent &&
                      'page:${other.name}' != id &&
                      entry.id.startsWith('route:${other.name}:'),
                ))
          entry,
    ];
    _stack.removeWhere(gone.contains);
    for (final entry in gone) {
      _gone(entry);
    }
    if (id == null || _stack.any((entry) => entry.id == id)) return;
    _stack.add(
      _Entry(
        id: id,
        overKeyboard: page == AppPage.settings,
        builder: (context) => switch (page) {
          AppPage.settings => const SettingsView(),
          AppPage.marketplace => const MarketplaceView(),
          AppPage.changelog => const ChangelogView(),
          AppPage.map => const MapView(),
          AppPage.usage => const UsageView(),
          AppPage.inbox => const InboxView(),
          AppPage.agent => const SizedBox.shrink(),
        },
      ),
    );
  }

  /// [entry] went, by Back, a swipe or from here: what it showed closes.
  void _gone(_Entry entry, [Object? result]) {
    if (entry.id.startsWith('viewer:')) _viewerGone(entry.id);
    _soon(() {
      final page = _page;
      if (entry.id.startsWith('page:') &&
          page != null &&
          'page:${page.value.name}' == entry.id) {
        page.value = AppPage.agent;
      }
      if (entry.id.startsWith('files:')) {
        fireAndLog(AppScope.of(context).watchFiles(''));
      }
      if (entry.done case final done? when !done.isCompleted) {
        done.complete(result);
      }
      _changed();
    });
  }

  @override
  Future<T?> push<T>(PhoneRoute<T> route) async {
    final done = Completer<Object?>();
    final id = 'route:${route.id}';
    // The same page again, or one beside it ('settings:devices' over
    // 'settings:providers'), takes its place rather than stacking.
    final colon = route.id.lastIndexOf(':');
    final siblings = colon < 0
        ? null
        : 'route:${route.id.substring(0, colon + 1)}';
    final before = _stack
        .where(
          (entry) =>
              entry.id == id ||
              siblings != null && entry.id.startsWith(siblings),
        )
        .toList();
    setState(() {
      _stack.removeWhere(before.contains);
      _stack.add(
        _Entry(
          id: id,
          builder: route.builder,
          overKeyboard: true,
          fullscreenDialog: route.fullscreenDialog,
          done: done,
        ),
      );
    });
    for (final entry in before) {
      _gone(entry);
    }
    _changed();
    return await done.future as T?;
  }

  @override
  void pop<T>([T? result]) {
    if (_stack.isEmpty) return;
    final top = _stack.removeLast();
    setState(() {});
    _gone(top, result);
  }

  @override
  bool get canPop => _stack.isNotEmpty;

  @override
  PageTarget? get topPage {
    if (_stack.isEmpty) return null;
    final top = _stack.last;
    return PageTarget(
      id: top.id,
      serial: top.serial,
      fullscreenDialog: top.fullscreenDialog,
    );
  }

  @override
  void popAll() {
    if (_stack.isEmpty) return;
    final all = [..._stack];
    setState(_stack.clear);
    for (final entry in all.reversed) {
      _gone(entry);
    }
  }

  @override
  void showFiles(String projectId) {
    final backend = AppScope.of(context);
    final gone = [
      for (final entry in _stack)
        if (entry.id.startsWith('files:') || entry.id.startsWith('file:'))
          entry,
    ];
    setState(() {
      _stack.removeWhere(gone.contains);
      _stack.add(
        _Entry(
          id: 'files:$projectId',
          alive: () =>
              projectById(backend.workspace.workspace, projectId) != null,
          builder: (context) => ListenableBuilder(
            listenable: backend.workspace,
            builder: (context, _) {
              final project = projectById(
                backend.workspace.workspace,
                projectId,
              );
              if (project == null) return const SizedBox.shrink();
              return _FilesScreen(
                project: project,
                onBack: pop,
                onOpen: (path) => _showFile(projectId, path),
              );
            },
          ),
        ),
      );
    });
    fireAndLog(backend.watchFiles(projectId));
    _changed();
  }

  /// A file of a project's Files, over them.
  void _showFile(String projectId, String path) {
    final files = 'files:$projectId';
    setState(() {
      _stack.removeWhere((entry) => entry.id.startsWith('file:'));
      _stack.add(
        _Entry(
          id: 'file:$path',
          alive: () => _stack.any((entry) => entry.id == files),
          builder: (context) {
            final leaf = viewerLeaf(path).leaf;
            return _ViewerScreen(
              leaf: leaf,
              glyph: viewerIcon(leaf, Palette.of(context).textMuted),
              onBack: pop,
            );
          },
        ),
      );
    });
    _changed();
  }

  /// One of [agentId]'s viewers (a pane beside its conversation on a Mac),
  /// over its conversation. Back pops it on this phone only; its × removes
  /// the pane from the agent's layout, everywhere.
  void _showViewer(String agentId, String paneId) {
    _workbenchFocus?.focus(agentId, paneId);
    final backend = AppScope.of(context);
    Agent? agent() {
      final agent = backend.workspace.selectedAgent;
      return agent?.id == agentId ? agent : null;
    }

    setState(() {
      _stack.removeWhere((entry) => entry.id.startsWith('viewer:'));
      _stack.add(
        _Entry(
          id: 'viewer:$agentId/$paneId',
          alive: () {
            final shown = agent();
            return shown != null && find(layoutOf(shown), paneId) != null;
          },
          builder: (context) => ListenableBuilder(
            listenable: backend.workspace,
            builder: (context, _) {
              final shown = agent();
              final viewer = shown == null
                  ? null
                  : find(layoutOf(shown), paneId);
              if (shown == null || viewer == null) {
                return const SizedBox.shrink();
              }
              return _ViewerScreen(
                leaf: viewer.leaf,
                glyph: paneGlyph(
                  shown,
                  viewer.leaf,
                  Palette.of(context).textMuted,
                ),
                onBack: pop,
                onClose: () {
                  final closed = close(layoutOf(shown), viewer.id);
                  if (closed == null) return;
                  fireAndLog(
                    backend.setLayout(
                      shown.id,
                      isChatOnly(closed) ? null : closed,
                    ),
                  );
                },
              );
            },
          ),
        ),
      );
    });
    _changed();
  }

  /// On an iPad, what the person reads in this window (the workbench's
  /// focus), kept across the window narrowing to these routes and widening
  /// again; null elsewhere, where these routes are all there is.
  WorkbenchFocus? get _workbenchFocus => deviceIdiom(context) == Idiom.pad
      ? context.getInheritedWidgetOfExactType<WorkbenchScope>()?.notifier
      : null;

  /// The agent whose focus these routes took up last ([_takeUpFocus]).
  String? _focusTaken;

  /// A window narrowed with a viewer in front keeps it in front: the
  /// workbench's focused leaf, pushed over [agent]'s conversation, once.
  void _takeUpFocus(Agent agent) {
    final focus = _workbenchFocus;
    if (focus == null || _focusTaken == agent.id) return;
    _focusTaken = agent.id;
    final routes = compactRoutes(
      layoutOf(agent),
      focus.focusedIn(agent.id, layoutOf(agent)),
    );
    if (routes.length < 2) return;
    _soon(() => _showViewer(agent.id, routes.last));
  }

  /// [id]'s viewer went (Back, a swipe): the conversation is in front.
  void _viewerGone(String id) {
    final focus = _workbenchFocus;
    final slash = id.indexOf('/');
    if (focus == null || slash < 0) return;
    final agentId = id.substring('viewer:'.length, slash);
    final agent = AppScope.of(context).workspace.selectedAgent;
    if (agent == null || agent.id != agentId) return;
    final root = layoutOf(agent);
    focus.focus(agentId, compactRoutes(root, null).first);
  }

  /// The agent's extensions and the viewers beside its conversation, in a
  /// sheet: one opens its panel, or pushes the viewer.
  void _listExtensions(Agent agent) => unawaited(
    presentSheet<void>(
      context,
      padded: false,
      builder: (context, close) => PhoneExtensionList(
        agent: agent,
        chosen: close,
        viewed: (id) {
          close();
          _showViewer(agent.id, id);
        },
      ),
    ),
  );

  void _back(Agent agent) {
    final history = AppScope.scope(context).history;
    final back = history.back;
    // Back to the agent it came from by a message.
    if (back != null && back != agent.id) history.goBack(agent.id);
  }

  /// Drops pages whose content is gone (a project deleted, a pane closed
  /// elsewhere, another agent chosen); the navigator pops them.
  void _prune() {
    final dead = [
      for (final entry in _stack)
        if (entry.alive != null && !entry.alive!()) entry,
    ];
    if (dead.isEmpty) return;
    _stack.removeWhere(dead.contains);
    for (final entry in dead) {
      _gone(entry);
    }
  }

  void _attach() {
    if (_attached) return;
    _attached = true;
    _host?.attach(this);
  }

  void _detach() {
    if (!_attached) return;
    _attached = false;
    _host?.detach(this);
  }

  @override
  Widget build(BuildContext context) {
    final scope = AppScope.scope(context);
    final backend = scope.backend;
    return ListenableBuilder(
      listenable: Listenable.merge([
        backend.workspace,
        scope.connections,
        backend.pairing,
        backend.connected,
        backend.signInRequired,
      ]),
      builder: (context, _) {
        if (scope.connections.current == null && !backend.localBackend) {
          _soon(_detach);
          return const _Screen(overKeyboard: true, child: PairScreen());
        }
        final waiting = scope.connections.current;
        // Let in only once this phone's signed in to the account.
        if (backend.signInRequired.value != null && waiting != null) {
          _soon(_detach);
          return _Screen(
            overKeyboard: true,
            child: SignInRequiredView(
              onCancel: waiting.fromAccount
                  ? () => scope.connections.forget(waiting)
                  : null,
            ),
          );
        }
        // A code used, waiting to be allowed: the number to compare.
        if (backend.pairing.value != null && waiting != null) {
          _soon(_detach);
          return _Screen(
            child: PairingWaitView(
              onCancel: () => scope.connections.forget(waiting),
            ),
          );
        }
        _soon(_attach);
        _prune();
        final workspace = backend.workspace.workspace;
        final pipe = selectedPipe(workspace);
        final chat = selectedChat(workspace);
        final agent = pipe == null && chat == null
            ? backend.workspace.selectedAgent
            : null;
        // Nothing chosen yet: the drawer offers what there is, once.
        if (pipe == null &&
            chat == null &&
            agent == null &&
            !_offered &&
            backend.connected.value &&
            workspace.agents.isNotEmpty) {
          _offered = true;
          _soon(() => _host?.openDrawer());
        }
        if (agent != null) _takeUpFocus(agent);
        final panel = _stack.isEmpty ? openPanelOf(workspace, agent) : null;
        if (agent != null && panel != null) _panelAgent = agent.id;
        final policy = PhonePlatformPolicy.of(context);
        final screens = Stack(
          fit: StackFit.expand,
          children: [
            SheetOverlay(
              overlay: Overlay.of(context),
              child: Navigator(
                key: _navigator,
                pages: [
                  policy.page<void>(
                    key: const ValueKey('conversation'),
                    child: _Root(
                      onMenu: () => _host?.openDrawer(),
                      onBack: _back,
                      onMore: _listExtensions,
                    ),
                  ),
                  for (final entry in _stack)
                    policy.page<void>(
                      key: ValueKey(entry.id),
                      fullscreenDialog: entry.fullscreenDialog,
                      child: _Screen(
                        overKeyboard: entry.overKeyboard,
                        child: Builder(builder: entry.builder),
                      ),
                    ),
                ],
                onDidRemovePage: (page) {
                  final key = page.key;
                  if (key is! ValueKey<String>) return;
                  final index = _stack.indexWhere(
                    (entry) => entry.id == key.value,
                  );
                  // Already gone from here: it was taken away, not popped.
                  if (index < 0) return;
                  final entry = _stack.removeAt(index);
                  _soon(() => setState(() {}));
                  _gone(entry);
                },
              ),
            ),
            // The open extension's panel, in a sheet: dragged down, the
            // dimmed screen tapped or Esc closes it on the backend.
            PhoneSheetHost(
              onDismiss: () {
                final shown = _panelAgent;
                if (shown != null) {
                  fireAndLog(backend.openPanel(shown, ''));
                }
              },
              child: agent != null && panel != null
                  ? ExtensionPanel(
                      key: ValueKey('${panel.id}/${agent.id}'),
                      extension: panel,
                      agent: agent,
                      width: double.infinity,
                    )
                  : null,
            ),
          ],
        );
        // What the account needs from the person (a device to allow, this
        // one to confirm) along the top, over whatever page is in front.
        return DeviceBanners(child: screens);
      },
    );
  }
}

/// The conversation in front: the selected pipe's, or agent's, or what to
/// do while there's none. Its leading button opens the drawer; an agent
/// reached by a message has Back to the one it came from instead.
class _Root extends StatelessWidget {
  const _Root({
    required this.onMenu,
    required this.onBack,
    required this.onMore,
  });

  final VoidCallback onMenu;
  final void Function(Agent agent) onBack;
  final void Function(Agent agent) onMore;

  @override
  Widget build(BuildContext context) {
    final scope = AppScope.scope(context);
    final backend = scope.backend;
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: Listenable.merge([backend.workspace, scope.history]),
      builder: (context, _) {
        final workspace = backend.workspace.workspace;
        // One of the person's chats, as the conversation.
        final chat = selectedChat(workspace);
        if (chat != null) {
          return _Screen(
            overKeyboard: true,
            child: ChatView(
              key: ValueKey('chat/${chat.id}'),
              chat: chat,
              onMenu: onMenu,
            ),
          );
        }
        final pipe = selectedPipe(workspace);
        if (pipe != null) {
          return _Screen(
            overKeyboard: true,
            child: PipeView(
              key: ValueKey('pipe/${pipe.id}'),
              pipe: pipe,
              onMenu: onMenu,
            ),
          );
        }
        final agent = backend.workspace.selectedAgent;
        if (agent == null) {
          return _Screen(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                PhoneHeader(
                  leading: HeaderButton.menu(onTap: onMenu),
                  title: Text(
                    claktoryName(backend),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: PhoneType.emphasis.copyWith(color: palette.text),
                  ),
                ),
                const Expanded(child: NoAgentView()),
              ],
            ),
          );
        }
        final back = scope.history.back;
        return _Screen(
          overKeyboard: true,
          child: AnimatedContainer(
            duration: Motion.base,
            color: conversationTint(agent),
            child: AgentView(
              key: ValueKey(agent.id),
              agent: agent,
              // Back to the agent a message came from, else the drawer.
              onBack: back != null && back != agent.id
                  ? () => onBack(agent)
                  : null,
              onMenu: onMenu,
              onMore: () => onMore(agent),
            ),
          ),
        );
      },
    );
  }
}

/// A screen of the phone's own: under the status bar, above the home
/// indicator (or with [overKeyboard], the keyboard, whichever is higher),
/// and beside the notch on its side. It owns those insets: what's inside
/// reads none (mobile/navigation.dart).
class _Screen extends StatelessWidget {
  const _Screen({required this.child, this.overKeyboard = false});

  final Widget child;
  final bool overKeyboard;

  @override
  Widget build(BuildContext context) {
    final safe = MediaQuery.paddingOf(context);
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    final inner = withoutInsets(context, keyboard: overKeyboard, child: child);
    return ColoredBox(
      color: Palette.of(context).canvas,
      child: Padding(
        padding: EdgeInsets.only(
          left: safe.left,
          right: safe.right,
          top: safe.top,
          bottom: overKeyboard ? math.max(safe.bottom, keyboard) : safe.bottom,
        ),
        child: inner,
      ),
    );
  }
}

/// What a project's row does on a phone: its Files open as a page of their
/// own ([files]). The drawer gives it.
class PhoneProjects extends InheritedWidget {
  const PhoneProjects({super.key, required this.files, required super.child});

  final void Function(Project project) files;

  /// Null outside a phone's layout.
  static PhoneProjects? of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<PhoneProjects>();

  @override
  bool updateShouldNotify(PhoneProjects old) => false;
}

/// A project's Files as a page pushed over the list: a bar with a back
/// button, "Files", the project's name and Show hidden files, then its tree.
/// A file opens as a page over it.
class _FilesScreen extends StatelessWidget {
  const _FilesScreen({
    required this.project,
    required this.onBack,
    required this.onOpen,
  });

  final Project project;
  final VoidCallback onBack;
  final ValueChanged<String> onOpen;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.explorer,
      builder: (context, _) {
        final explorer = backend.explorer.of(project.id);
        return ColoredBox(
          color: palette.sidebar,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Container(
                height: titleBarHeight,
                padding: const EdgeInsets.symmetric(horizontal: Space.xs),
                decoration: BoxDecoration(
                  border: Border(bottom: BorderSide(color: palette.hairline)),
                ),
                child: Row(
                  children: [
                    _BackButton(onBack: onBack),
                    const SizedBox(width: Space.xs),
                    FolderGlyph(color: palette.textMuted),
                    const SizedBox(width: Space.s),
                    Text(
                      'Files',
                      style: TextStyles.label.copyWith(color: palette.text),
                    ),
                    const SizedBox(width: Space.s),
                    Expanded(
                      child: Text(
                        project.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyles.caption.copyWith(
                          color: palette.textFaint,
                        ),
                      ),
                    ),
                    if (explorer != null)
                      ShowEverything(project: project, explorer: explorer),
                  ],
                ),
              ),
              const SizedBox(height: Space.s),
              Expanded(
                child: FilesTree(
                  project: project,
                  explorer: explorer,
                  onOpen: onOpen,
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

/// A viewer pushed over the conversation, or over a project's Files: a bar
/// with a back button, its title, its controls and, for a pane of the
/// agent's layout, its × ([onClose]); then the viewer.
class _ViewerScreen extends StatelessWidget {
  const _ViewerScreen({
    required this.leaf,
    required this.glyph,
    required this.onBack,
    this.onClose,
  });

  final ViewLeaf leaf;
  final Widget glyph;
  final VoidCallback onBack;
  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    return PaneChrome(
      phone: PhonePane(
        title: paneTitle(leaf),
        onBack: onBack,
        onRemove: onClose, // null for a project's file: nothing to remove
      ),
      child: Builder(builder: (context) => viewerBody(context, leaf)),
    );
  }
}

class _BackButton extends StatelessWidget {
  const _BackButton({required this.onBack});

  final VoidCallback onBack;

  @override
  Widget build(BuildContext context) => GlyphButton(
    label: 'Back',
    active: false,
    onTap: onBack,
    glyph: (color) =>
        ExtensionGlyph(icon: 'chevron-left', name: 'Back', color: color),
  );
}

/// A phone with nothing paired: the camera scanning the pairing code the
/// Mac shows, or pasting it instead. Only through the private tunnel: a
/// code without the backend's key, from a Claktory before it, is turned
/// down.
class PairScreen extends StatefulWidget {
  const PairScreen({super.key});

  @override
  State<PairScreen> createState() => _PairScreenState();
}

class _PairScreenState extends State<PairScreen> with WidgetsBindingObserver {
  final _code = TextEditingController();

  /// Whether the camera may be used; null until known.
  CameraAccess? _access;

  /// The person chose to paste the code instead.
  var _pasting = false;

  /// Connecting through the account (sign in, pick one of "Your
  /// Claktories") rather than with a code: at first when this phone is
  /// signed in already.
  bool? _withAccount;

  /// Why the last code scanned can't be used.
  String? _scanError;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    WidgetsBinding.instance.addPostFrameCallback((_) => _check());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _code.dispose();
    super.dispose();
  }

  // Back from Settings, the camera may be allowed now.
  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) _check();
  }

  /// Finds out whether the camera may be used, asking the person ([ask])
  /// when they haven't answered yet.
  Future<void> _check({bool ask = false}) async {
    if (!mounted) return;
    final camera = AppScope.scope(context).camera;
    var access = await camera.access();
    if (ask && access == CameraAccess.undetermined) {
      access = await camera.request()
          ? CameraAccess.granted
          : CameraAccess.denied;
    }
    if (mounted && access != _access) setState(() => _access = access);
  }

  Future<void> _paste() async {
    final pasted = await Clipboard.getData(Clipboard.kTextPlain);
    final text = pasted?.text?.trim() ?? '';
    if (text.isNotEmpty) _code.text = text;
  }

  void _scanned(String code) {
    final problem = pairingProblem(code.trim(), scanned: true);
    if (problem != null) {
      setState(() => _scanError = problem);
      return;
    }
    _connect(parsePairingCode(code.trim())!);
  }

  void _connect(RemoteFactory target) =>
      AppScope.switchFactory(context, target);

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final access = _access;
    final withAccount = _withAccount ??= AppScope.scope(context)
        .connections
        .account
        .signedIn;
    final Widget body;
    if (withAccount) {
      body = Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          YourClaktories(onConnect: _connect),
          const SizedBox(height: Space.m),
          _Link(
            'Use a code instead',
            onTap: () => setState(() => _withAccount = false),
          ),
        ],
      );
    } else if (_pasting || access == CameraAccess.unavailable) {
      body = _pasteForm(context, noCamera: access == CameraAccess.unavailable);
    } else if (access == CameraAccess.granted) {
      body = _scanner(context);
    } else if (access == CameraAccess.denied ||
        access == CameraAccess.restricted) {
      body = _cameraRefused(
        context,
        restricted: access == CameraAccess.restricted,
      );
    } else if (access == CameraAccess.undetermined) {
      body = _scanFirst(context);
    } else {
      // Finding out whether there's a camera.
      body = const SizedBox(height: 120);
    }
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(Space.xl),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 420),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                'Connect to your Claktory',
                style: TypeScale.of(context).title
                    .copyWith(color: palette.text),
              ),
              if (!withAccount) ...[
                const SizedBox(height: Space.s),
                Text(
                  'On your Mac, open Claktory and go to Settings → Devices → '
                  'Add a device. Then scan the code.',
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.textMuted),
                ),
              ],
              const SizedBox(height: Space.xl),
              body,
              if (!withAccount)
                _Link(
                  'Sign in with your email instead',
                  onTap: () => setState(() => _withAccount = true),
                ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _scanner(BuildContext context) {
    final palette = Palette.of(context);
    final error = _scanError;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Square, and on a phone on its side no more than half the
        // screen's height, so what's under it stays in view.
        Center(
          child: ConstrainedBox(
            constraints: BoxConstraints(
              maxHeight: MediaQuery.sizeOf(context).height / 2,
            ),
            child: AspectRatio(
              aspectRatio: 1,
              child: Container(
                clipBehavior: Clip.antiAlias,
                decoration: BoxDecoration(
                  color: const Color(0xFF000000),
                  borderRadius: BorderRadius.circular(Radii.panel),
                  border: Border.all(color: palette.hairline),
                ),
                child: AppScope.scope(context).camera.preview(_scanned),
              ),
            ),
          ),
        ),
        if (error != null) ...[
          const SizedBox(height: Space.s),
          Text(
            error,
            style: TypeScale.of(context).caption
                .copyWith(color: palette.danger),
          ),
        ],
        const SizedBox(height: Space.m),
        _Link(
          'Paste code instead',
          onTap: () => setState(() => _pasting = true),
        ),
      ],
    );
  }

  Widget _cameraRefused(BuildContext context, {required bool restricted}) {
    final palette = Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          restricted
              ? "The camera isn't available on this iPhone. Paste the code "
                    'instead.'
              : 'To scan, allow camera access in Settings → Claktory.',
          style: TypeScale.of(context).body.copyWith(color: palette.text),
        ),
        const SizedBox(height: Space.l),
        if (!restricted)
          Align(
            alignment: Alignment.centerRight,
            child: Button(
              label: 'Open Settings',
              primary: true,
              onPressed: () =>
                  fireAndLog(AppScope.scope(context).camera.openSettings()),
            ),
          ),
        const SizedBox(height: Space.m),
        _Link(
          'Paste code instead',
          onTap: () => setState(() => _pasting = true),
        ),
      ],
    );
  }

  /// Before the camera is asked for: the person starts it, so the system's
  /// prompt comes when they expect it.
  Widget _scanFirst(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Button(
        label: 'Scan code',
        primary: true,
        onPressed: () => _check(ask: true),
      ),
      const SizedBox(height: Space.m),
      _Link('Paste code instead', onTap: () => setState(() => _pasting = true)),
    ],
  );

  Widget _pasteForm(BuildContext context, {required bool noCamera}) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: _code,
      builder: (context, _) {
        final pasted = _code.text.trim();
        final error = pasted.isEmpty ? null : pairingProblem(pasted);
        final target = error == null ? parsePairingCode(pasted) : null;
        void connect() {
          if (target != null) _connect(target);
        }

        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            TextField(
              controller: _code,
              placeholder: 'Paste code',
              // A code, not words: nothing to correct or capitalize.
              autocorrect: false,
              textCapitalization: TextCapitalization.none,
              onSubmitted: (_) => connect(),
            ),
            if (error != null) ...[
              const SizedBox(height: Space.s),
              Text(
                error,
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.danger),
              ),
            ],
            const SizedBox(height: Space.l),
            // At the largest text, one above the other, Connect first.
            if (largeText(context))
              Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Button(
                    label: 'Connect',
                    primary: true,
                    onPressed: target != null ? connect : null,
                  ),
                  const SizedBox(height: Space.s),
                  Button(label: 'Paste', onPressed: _paste),
                ],
              )
            else
              Row(
                children: [
                  Button(label: 'Paste', onPressed: _paste),
                  const Spacer(),
                  Button(
                    label: 'Connect',
                    primary: true,
                    onPressed: target != null ? connect : null,
                  ),
                ],
              ),
            if (!noCamera &&
                _access != CameraAccess.denied &&
                _access != CameraAccess.restricted) ...[
              const SizedBox(height: Space.m),
              _Link(
                'Scan code instead',
                onTap: () => setState(() {
                  _pasting = false;
                  _scanError = null;
                }),
              ),
            ],
          ],
        );
      },
    );
  }
}

/// Why [code] can't connect this phone, or null when it can: it isn't a
/// code, or it's from a Claktory before the private tunnel.
String? pairingProblem(String code, {bool scanned = false}) {
  final target = parsePairingCode(code);
  if (target == null) {
    // A scanned one is most likely another QR code, such as a Wi-Fi one.
    return scanned
        ? "That's not a Claktory code. Scan the one in Settings → Devices "
              'on your Mac.'
        : "That isn't a Claktory code. Use the one in Settings → Devices "
              'on your Mac.';
  }
  if (!target.tunnelled) {
    return 'This code is from an older Claktory. Update Claktory on your '
        'Mac, then add this iPhone again.';
  }
  return null;
}

/// A small text action, in caption: "Paste code instead". Tall enough
/// for a finger.
class _Link extends StatelessWidget {
  const _Link(this.label, {required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      label: label,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => SizedBox(
          height: 44,
          child: Align(
            alignment: Alignment.centerLeft,
            widthFactor: 1,
            child: Text(
              label,
              style: TypeScale.of(context).caption.copyWith(
                color: hovered ? palette.text : palette.textMuted,
                fontWeight: FontWeight.w500,
                decoration: TextDecoration.underline,
                decorationColor: hovered ? palette.text : palette.textMuted,
              ),
            ),
          ),
        ),
      ),
    );
  }
}
