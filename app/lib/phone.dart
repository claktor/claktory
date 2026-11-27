import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart' show defaultTargetPlatform;
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_view.dart';
import 'agent_menu.dart' show showAgentSettings;
import 'backend.dart';
import 'changelog.dart';
import 'chats.dart' show ChatView, selectedChat;
import 'claktory_identity.dart' show claktoryName;
import 'connections.dart';
import 'history/export_job.dart' show ReplayJobBanner;
import 'needs_you_entries.dart' show NeedsYouEntries;
import 'inbox.dart' show InboxView, needsYouItems;
import 'links.dart' show confirmPairing;
import 'controls.dart';
import 'conversation_versions.dart' show showVersions;
import 'devices.dart' show thisDeviceKind;
import 'explorer.dart';
import 'extensions.dart';
import 'forms.dart' show FittedHeading;
import 'identity.dart';
import 'ipad/camera_kept.dart' show CameraKeptBand;
import 'ipad/columns.dart' show compactRoutes;
import 'ipad/document_host.dart' show ipadDocumentPane, ipadProjectDocument;
import 'ipad/pair_step.dart' show pairScreenKey;
import 'ipad/workbench.dart' show WorkbenchFocus, WorkbenchScope;
import 'layout.dart';
import 'map.dart';
import 'monitors_overview.dart' show MonitorsOverview;
import 'marketplace.dart';
import 'mobile/back.dart'
    show BackHandle, CallbackBackToken, PageTarget, PhoneBack;
import 'mobile/drawer.dart';
import 'mobile/extension_list.dart';
import 'mobile/header.dart';
import 'mobile/local_network.dart' show connectNewClaktory;
import 'mobile/metrics.dart' show PhoneType, TypeScale, withoutInsets;
import 'mobile/navigation.dart';
import 'mobile/page_find.dart';
import 'mobile/device_permissions.dart' show CameraPermission;
import 'mobile/permission.dart'
    show
        PermissionAccess,
        PermissionContent,
        PermissionFlow,
        PermissionWayOut,
        registerPermissionHost;
import 'mobile/platform_policy.dart';
import 'mobile/needs_you_phone.dart' show PhoneNeedsYou;
import 'mobile/quick_actions.dart' show PhoneQuickActions;
import 'mobile/review_bar.dart';
import 'mobile/scroll_fade.dart' show ScrollsWithFade;
import 'mobile/share.dart' show globalRectOf, shareText;
import 'mobile/push.dart' show PushRegistrar;
import 'mobile/presentation.dart'
    show PhoneSheetHost, SheetOverlay, presentSheet;
import 'pairing.dart';
import 'pinned_agents.dart' show togglePin;
import 'pasted_code.dart';
import 'panes.dart' show AppPane, appKind, pageOfKind, paneGlyph, paneTitle;
import 'pipes.dart' show PipeView, selectedPipe;
import 'protocol/factory/v1/factory.pb.dart';
import 'rail.dart'
    show ClaktoriesBackStrip, ClaktoriesScope, confirmRemoveClaktory;
import 'scanner.dart';
import 'screens.dart' show ScreenOpener;
import 'scope.dart';
import 'settings_view.dart';
import 'shell.dart' show NoAgentView, turnedAway;
import 'pane_chrome.dart';
import 'viewers/registry.dart';
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
    // The Home Screen's quick action opens Needs you here (#514).
    if (PhoneDrawerHost.of(context) != null) {
      return const PhoneQuickActions(child: _PhoneScreens());
    }
    return const PhoneDrawerHost(
      child: PhoneQuickActions(child: _PhoneScreens()),
    );
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
    this.edgeToEdge = false,
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

  /// Drawn under the status bar, the notch and the home indicator: a
  /// picture or video's page, whose chrome keeps clear of them itself
  /// (mobile/viewer_chrome.dart).
  final bool edgeToEdge;
  final bool Function()? alive;
  final Completer<Object?>? done;
}

/// How many phone layouts are up (an iPad's windows can each be one).
var _phoneScreensUp = 0;

/// Whether the app is laid out as on a phone now, for what decides
/// outside the widgets (a tapped notification, notifications.dart).
bool get phoneLayoutShowing => _phoneScreensUp > 0;

class _PhoneScreens extends StatefulWidget {
  const _PhoneScreens();

  @override
  State<_PhoneScreens> createState() => _PhoneScreensState();
}

class _PhoneScreensState extends State<_PhoneScreens> implements PhonePages {
  final _navigator = GlobalKey<NavigatorState>();

  /// What's pushed over the conversation, bottom first.
  final _stack = <_Entry>[];

  /// The conversation's find: its ⋯ list offers Search when it can.
  final _rootFind = GlobalKey<PhoneFindHostState>();

  PhoneDrawerHostState? _host;
  var _attached = false;

  /// Where an agent's show_screen says it opened a conversation
  /// (screens.dart): it comes to the front.
  Listenable? _opened;
  ValueNotifier<AppPage>? _page;

  /// The page [_showViewer] showed for a page pane on an iPad, if any.
  AppPage? _pageOfPane;

  /// The drawer opened by itself once, for a Claktory with nothing chosen.
  var _offered = false;

  /// Whose extension's panel shows, so dismissing it closes that one even
  /// as it goes down.
  String? _panelAgent;

  /// Stops this screen being where a permission asked for away from any
  /// screen shows (notifications after the first pairing).
  late final VoidCallback _unhostPermissions;

  @override
  void initState() {
    super.initState();
    _unhostPermissions = registerPermissionHost(context);
    _phoneScreensUp++;
  }

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
    _followNeedsYou(AppScope.of(context));
  }

  @override
  void dispose() {
    _followNeedsYou(null);
    _phoneScreensUp--;
    _unhostPermissions();
    if (_attached) _host?.detach(this);
    _opened?.removeListener(_toFront);
    _page?.removeListener(_pageChanged);
    // An iPad window widening: a page shown for a pane of the layout
    // ([_showViewer]) goes back to its pane, in the workbench.
    final page = _page, ofPane = _pageOfPane;
    if (page != null && ofPane != null && page.value == ofPane) {
      SchedulerBinding.instance.addPostFrameCallback((_) {
        if (page.value == ofPane) page.value = AppPage.agent;
      });
    }
    _waitingPaneTimer?.cancel();
    for (final entry in _stack) {
      entry.done?.complete();
    }
    super.dispose();
  }

  /// After this frame, with one asked for. Told from builds: a build can
  /// run outside a frame too (the app's first, at launch, or one the
  /// framework runs between frames, while the phase reads idle), so it's
  /// never run straight away.
  void _soon(VoidCallback action) {
    SchedulerBinding.instance
      ..addPostFrameCallback((_) {
        if (mounted) action();
      })
      ..ensureVisualUpdate();
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
    _stack.add(_pageEntry(page));
  }

  /// [page]'s place on the stack, over the conversation.
  _Entry _pageEntry(AppPage page) => _Entry(
    id: 'page:${page.name}',
    // Their fields (a reply, an answer) stay above the keyboard.
    overKeyboard: page == AppPage.settings || page == AppPage.inbox,
    builder: (context) => switch (page) {
      AppPage.settings => const SettingsView(),
      AppPage.marketplace => const MarketplaceView(),
      AppPage.changelog => const ChangelogView(),
      AppPage.map => const MapView(),
      AppPage.usage => const UsageView(),
      AppPage.monitors => const MonitorsOverview(),
      // The list of what waits first, as everywhere (#599); on an iPhone
      // (and a narrow iPad) a request tapped opens alone on its own screen,
      // where its answer rides the keyboard. Android opens it in place.
      AppPage.inbox => InboxView(
        onOpen: defaultTargetPlatform == TargetPlatform.iOS
            ? (item) => _openRequest(item.key)
            : null,
      ),
      AppPage.agent => const SizedBox.shrink(),
    },
  );

  /// The Claktory whose Needs you this follows: a request a notification
  /// asks for.
  Backend? _needsYouOf;

  static const _requestRoute = 'needs-you:request';
  static const _focusRoute = 'needs-you-focus';

  /// The request showing alone ([_openRequest]), by its key.
  String? _request;

  void _followNeedsYou(Backend? backend) {
    if (backend == _needsYouOf) return;
    _needsYouOf?.needsYouCard.removeListener(_requestAsked);
    _needsYouOf = backend;
    backend?.needsYouCard.addListener(_requestAsked);
  }

  /// Focus mode on an iPhone (#599, Squalo's L1): its sitting on a screen
  /// of its own over the list, for as long as it shows, never kept as the
  /// device's focus mode (that would bring it back at the next launch).
  /// Back, or Done once nothing is left, comes back to the list. An
  /// iPad's window, narrow too, has its own (shell.dart's FocusModeLayer):
  /// false there.
  @override
  bool openFocus() {
    if (defaultTargetPlatform != TargetPlatform.iOS ||
        deviceIdiom(context) == Idiom.pad) {
      return false;
    }
    if (_stack.any((entry) => entry.id == 'route:$_focusRoute')) return true;
    unawaited(
      push<void>(
        PhoneRoute(
          id: _focusRoute,
          builder: (context) =>
              PhoneNeedsYou(onClose: () => _removePage('route:$_focusRoute')),
        ),
      ),
    );
    return true;
  }

  /// A notification asked for a request (notifications.dart): it alone,
  /// over the list, once the list is up (it's opened in the same breath).
  /// One answered or taken back meanwhile leaves the list showing; in
  /// focus mode, the sitting shows it itself.
  void _requestAsked() {
    final backend = _needsYouOf;
    final key = backend?.needsYouCard.value;
    if (backend == null || key == null || key == _request) return;
    if (defaultTargetPlatform != TargetPlatform.iOS) return;
    if (_stack.any((entry) => entry.id == 'route:$_focusRoute')) return;
    _soon(() {
      if (backend.needsYouCard.value != key) return;
      final waiting = needsYouItems(backend.workspace.workspace);
      final item = waiting.where((item) => item.key == key).firstOrNull;
      if (item == null) {
        backend.needsYouCard.value = null;
        return;
      }
      // As it is now (Squalo's B4), as one tapped in the list is.
      NeedsYouEntries.of(backend).fetch(item);
      _openRequest(key);
    });
  }

  /// The request at [key] alone, over what shows (the list): Back, or its
  /// answer accepted, comes back there.
  void _openRequest(String key) {
    _request = key;
    unawaited(
      push<void>(
        PhoneRoute(
          id: _requestRoute,
          builder: (context) => PhoneNeedsYou(
            only: key,
            onClose: () => _removePage('route:$_requestRoute'),
          ),
        ),
      ).then((_) {
        if (_request == key) _request = null;
      }),
    );
  }

  /// Takes the page [id] off the stack, wherever it is.
  void _removePage(String id) {
    final at = _stack.lastIndexWhere((entry) => entry.id == id);
    if (at < 0) return;
    final entry = _stack.removeAt(at);
    setState(() {});
    _gone(entry);
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
    _pushFile(path, () => _stack.any((entry) => entry.id == files));
  }

  /// [path]'s viewer over what's in front, apart from any layout, while
  /// [alive].
  void _pushFile(String path, bool Function() alive) {
    setState(() {
      _stack.removeWhere((entry) => entry.id.startsWith('file:'));
      _stack.add(
        _Entry(
          id: 'file:$path',
          edgeToEdge: _edgeToEdge(viewerLeaf(path).leaf.kind),
          alive: alive,
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

  /// An app's view as a page of its own over what's in front, apart from
  /// any layout: opening an app on a phone never adds it beside an agent's
  /// conversation on the person's other devices.
  @override
  void showApp(ViewLeaf leaf) {
    setState(() {
      _stack.removeWhere((entry) => entry.id.startsWith('app:'));
      _stack.add(
        _Entry(
          id: 'app:${leaf.appId}/${leaf.view}',
          builder: (context) {
            final agent = AppScope.of(context).workspace.selectedAgent;
            final color = Palette.of(context).textMuted;
            return _ViewerScreen(
              leaf: leaf,
              glyph: agent == null
                  ? const SizedBox.shrink()
                  : paneGlyph(agent, leaf, color),
              onBack: pop,
            );
          },
        ),
      );
    });
    _changed();
  }

  /// Whether a viewer of [kind] fills the screen: a picture or video on a
  /// phone (mobile/viewer_chrome.dart). An iPad in a narrow window keeps
  /// them as they were, where its document host may show them.
  bool _edgeToEdge(String kind) =>
      (kind == 'image' || kind == 'video') && deviceIdiom(context) != Idiom.pad;

  /// A pane [showPane] (or [showFile]) asked for, to push once it's in its
  /// agent's layout; given up on after a while ([_waitingPaneTimer]).
  ({String agentId, String paneId})? _waitingPane;
  Timer? _waitingPaneTimer;

  @override
  bool showFile(String agentId, String path) {
    final agent = AppScope.of(context).workspace.agent(agentId);
    if (agent == null) return false;
    // In the agent's layout as its `show` puts it, so the Mac and the iPad
    // have it too; already there, it's that pane.
    final backend = AppScope.of(context);
    final root = layoutOf(agent);
    final opened = open(root, path);
    if (!identical(opened.root, root)) {
      // Offline the layout can't take it: the file opens here alone, from
      // the copy this device kept (#446, Sale's row 127), and the layout
      // is left as it is.
      if (!backend.connected.value) {
        _pushFile(path, () => backend.workspace.agent(agentId) != null);
        return true;
      }
      fireAndLog(backend.setLayout(agentId, opened.root));
    }
    return showPane(agentId, opened.paneId);
  }

  @override
  bool showPane(String agentId, String paneId) {
    final backend = AppScope.of(context);
    if (backend.workspace.agent(agentId) == null) return false;
    final workspace = backend.workspace.workspace;
    if (workspace.selectedAgentId != agentId ||
        selectedPipe(workspace) != null ||
        workspace.selectedChatId.isNotEmpty) {
      fireAndLog(backend.selectAgent(agentId));
    }
    _waitingPane = (agentId: agentId, paneId: paneId);
    _waitingPaneTimer?.cancel();
    _waitingPaneTimer = Timer(
      const Duration(seconds: 10),
      () => _waitingPane = null,
    );
    _showWaitingPane();
    return true;
  }

  /// Pushes [_waitingPane] once its agent shows and the pane is there.
  void _showWaitingPane() {
    final waiting = _waitingPane;
    if (waiting == null || !mounted) return;
    final agent = AppScope.of(context).workspace.selectedAgent;
    if (agent == null ||
        agent.id != waiting.agentId ||
        find(layoutOf(agent), waiting.paneId) == null) {
      return;
    }
    _waitingPane = null;
    _waitingPaneTimer?.cancel();
    _soon(() => _showViewer(waiting.agentId, waiting.paneId));
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

    // A page in the layout (Needs you beside the conversation on a Mac or
    // a wide iPad) is that page here, as the drawer opens it: Back goes to
    // the conversation and leaves the pane in the layout.
    final shown = agent();
    final pane = shown == null ? null : find(layoutOf(shown), paneId);
    final page = pane == null ? null : pageOfKind(pane.leaf.kind);
    if (page != null) {
      _page?.value = page;
      // On an iPad, widening the window again shows it in its pane.
      if (_workbenchFocus != null) _pageOfPane = page;
      return;
    }
    // A project's Files pane (beside the conversation on a Mac or a wide
    // iPad) is the phone's Files page here, as the drawer's ⋯ → Files
    // opens it, never a viewer that can't show it (#540, Sale): Back goes
    // to the conversation and leaves the pane in the layout.
    if (pane != null && pane.leaf.kind == 'files') {
      showFiles(pane.leaf.path);
      return;
    }

    setState(() {
      _stack.removeWhere((entry) => entry.id.startsWith('viewer:'));
      _stack.add(
        _Entry(
          id: 'viewer:$agentId/$paneId',
          edgeToEdge: _edgeToEdge(pane?.leaf.kind ?? ''),
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
                node: viewer,
                agentId: shown.id,
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
    // A page's pane (the Map beside the conversation) is its page here,
    // in front from this very frame: a frame of the conversation alone
    // would let the pane's page go, and with it where it was (Sale's row
    // 149). The page's value follows after the frame, as it can't change
    // in a build.
    final pane = find(layoutOf(agent), routes.last);
    final page = pane == null ? null : pageOfKind(pane.leaf.kind);
    if (page != null) {
      _pageOfPane = page;
      if (!_stack.any((entry) => entry.id == 'page:${page.name}')) {
        _stack.add(_pageEntry(page));
      }
    }
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
  /// sheet, then Pin/Unpin and its settings. Actions use this owner after
  /// the sheet closes, never the sheet's going context or another Claktory.
  void _listExtensions(Agent agent) {
    final owner = context;
    final backend = AppScope.of(owner);
    bool currentOwner() => mounted && identical(AppScope.of(owner), backend);
    unawaited(
      presentSheet<void>(
        owner,
        padded: false,
        builder: (context, close) => PhoneExtensionList(
          agent: agent,
          chosen: close,
          onPin: (now) {
            close();
            if (currentOwner()) unawaited(togglePin(owner, now));
          },
          onSettings: (now) {
            close();
            if (currentOwner()) showAgentSettings(owner, now);
          },
          // The conversation can be searched: Search first.
          onSearch: switch (_rootFind.currentState?.session) {
            final session? => () {
              close();
              session.open();
            },
            null => null,
          },
          // Its other versions, while it has any (friend bug #9).
          onVersions: agent.branches > 0
              ? () {
                  close();
                  unawaited(showVersions(this.context, agent));
                }
              : null,
          viewed: (id) {
            close();
            _showViewer(agent.id, id);
            // Opened: it's the newest, on top of this list next time.
            fireAndLog(
              AppScope.scope(this.context).backend.paneOpened(agent.id, id),
            );
          },
        ),
      ),
    );
  }

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
        backend.otherAccount,
      ]),
      builder: (context, _) {
        if (scope.connections.current == null && !backend.localBackend) {
          _soon(_detach);
          // Its key carries it, at its step, across a change to the wide
          // layout and back (ipad/pair_step.dart).
          return _Screen(
            overKeyboard: true,
            child: PairScreen(key: pairScreenKey),
          );
        }
        // Another account's Claktory (#529, S4): what it said, before any
        // sign-in.
        if (backend.otherAccount.value != null) {
          _soon(_detach);
          return const _Screen(backStrip: true, child: OtherAccountView());
        }
        final waiting = scope.connections.current;
        // Let in only once this phone's signed in to the account.
        if (backend.signInRequired.value != null && waiting != null) {
          _soon(_detach);
          // ‹ Claktories over it, to the others, which this one doesn't
          // block (Zucchero row 647); giving up on one found through the
          // account asks first, as Remove… does everywhere.
          return _Screen(
            overKeyboard: true,
            backStrip: true,
            child: SignInRequiredView(
              onCancel: waiting.fromAccount
                  ? () => unawaited(_remove(context, waiting))
                  : null,
            ),
          );
        }
        // A code used, waiting to be allowed: the number to compare.
        if (backend.pairing.value != null && waiting != null) {
          _soon(_detach);
          return _Screen(
            backStrip: true,
            child: PairingWaitView(
              onCancel: () => scope.connections.forget(waiting),
            ),
          );
        }
        _soon(_attach);
        _prune();
        _showWaitingPane();
        final workspace = backend.workspace.workspace;
        final pipe = selectedPipe(workspace);
        final chat = selectedChat(workspace);
        final agent = pipe == null && chat == null
            ? backend.workspace.selectedAgent
            : null;
        // Nothing chosen yet: the drawer offers what there is, once. A
        // first-launch step can replace these screens after a page was
        // chosen in the drawer; don't offer the drawer over that page.
        if (_stack.isEmpty &&
            pipe == null &&
            chat == null &&
            agent == null &&
            !_offered &&
            backend.connected.value &&
            workspace.agents.isNotEmpty) {
          _offered = true;
          _soon(() {
            // A page may have been chosen since this offer was queued.
            if (_stack.isEmpty) _host?.openDrawer();
          });
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
                      find: _rootFind,
                    ),
                  ),
                  for (final entry in _stack)
                    policy.page<void>(
                      key: ValueKey(entry.id),
                      fullscreenDialog: entry.fullscreenDialog,
                      child: _Screen(
                        overKeyboard: entry.overKeyboard,
                        edgeToEdge: entry.edgeToEdge,
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
        // one to confirm), and a History video being made, along the top,
        // over whatever page is in front.
        return DeviceBanners(child: ReplayJobBanner(child: screens));
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
    required this.find,
  });

  /// The conversation's find, for Search in its ⋯ list.
  final GlobalKey<PhoneFindHostState> find;

  final VoidCallback onMenu;
  final void Function(Agent agent) onBack;
  final void Function(Agent agent) onMore;

  @override
  Widget build(BuildContext context) {
    final scope = AppScope.scope(context);
    final backend = scope.backend;
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: Listenable.merge([
        backend.workspace,
        scope.history,
        backend.refused,
        backend.connected,
      ]),
      builder: (context, _) {
        final workspace = backend.workspace.workspace;
        // The Claktory with no agent open: what to do, or why it can't be
        // used. Turned away, also in place of whatever was open, since
        // nothing there can be used (Zucchero's row 232).
        Widget noAgent() => _Screen(
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
        if (turnedAway(backend)) return noAgent();
        // One of the person's chats, as the conversation.
        final chat = selectedChat(workspace);
        if (chat != null) {
          return _Screen(
            overKeyboard: true,
            find: find,
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
            find: find,
            child: PipeView(
              key: ValueKey('pipe/${pipe.id}'),
              pipe: pipe,
              onMenu: onMenu,
            ),
          );
        }
        final agent = backend.workspace.selectedAgent;
        if (agent == null) return noAgent();
        final back = scope.history.back;
        return _Screen(
          overKeyboard: true,
          find: find,
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
///
/// A page whose pane can be searched shows the find bar at its foot while
/// its find is open ([PhoneFindHost]).
class _Screen extends StatelessWidget {
  const _Screen({
    required this.child,
    this.overKeyboard = false,
    this.edgeToEdge = false,
    this.backStrip = false,
    this.find,
  });

  final Widget child;
  final bool overKeyboard;

  /// "‹ Claktories" across its top, back to the list, when there's one
  /// (a screen in place of a Claktory's, which mustn't hide the others).
  final bool backStrip;

  /// The whole screen, its safe areas left in the [MediaQuery] for what's
  /// over the content ([_Entry.edgeToEdge]).
  final bool edgeToEdge;

  /// The conversation's own, so its ⋯ list can offer Search.
  final GlobalKey<PhoneFindHostState>? find;

  @override
  Widget build(BuildContext context) {
    if (edgeToEdge) {
      return ColoredBox(
        color: Palette.of(context).canvas,
        child: PhoneFindHost(key: find, child: child),
      );
    }
    final safe = MediaQuery.paddingOf(context);
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    final inner = withoutInsets(
      context,
      keyboard: overKeyboard,
      child: PhoneFindHost(key: find, child: child),
    );
    final bottom = overKeyboard ? math.max(safe.bottom, keyboard) : safe.bottom;
    if (backStrip) {
      return ColoredBox(
        color: Palette.of(context).canvas,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // The strip keeps clear of the status bar itself; where there's
            // none (one Claktory), room for the status bar.
            Stack(
              children: [
                SizedBox(height: safe.top),
                const ClaktoriesBackStrip(),
              ],
            ),
            Expanded(
              child: Padding(
                padding: EdgeInsets.only(
                  left: safe.left,
                  right: safe.right,
                  bottom: bottom,
                ),
                child: inner,
              ),
            ),
          ],
        ),
      );
    }
    return ColoredBox(
      color: Palette.of(context).canvas,
      child: Padding(
        padding: EdgeInsets.only(
          left: safe.left,
          right: safe.right,
          top: safe.top,
          bottom: bottom,
        ),
        child: inner,
      ),
    );
  }
}

/// Removes [target] after the rail's own question, never without it; with
/// no Claktory for it in the list, as it is.
Future<void> _remove(BuildContext context, RemoteFactory target) async {
  final found = ClaktoriesScope.of(context)?.all
      .where((claktory) => claktory.target == target)
      .firstOrNull;
  if (found == null) {
    return AppScope.scope(context).connections.forget(target);
  }
  await confirmRemoveClaktory(context, found);
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
                // A narrow window (an iPad's Split View) or large text keeps
                // only Show hidden files' check, named on hover.
                child: LayoutBuilder(
                  builder: (context, box) => Row(
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
                        ShowEverything(
                          project: project,
                          explorer: explorer,
                          compact:
                              box.maxWidth < 380 ||
                              MediaQuery.textScalerOf(context).scale(1) > 1.3,
                        ),
                    ],
                  ),
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
    this.agentId,
    this.node,
  });

  final ViewLeaf leaf;

  /// [leaf]'s node in the agent's layout: on an iPad in a narrow window,
  /// a document in it opens in the document host, which draws and has
  /// its own review bar. Null for a project's file.
  final ViewNode? node;

  /// The agent whose pane it is: a review it asks of the file is answered
  /// at its foot ([PhoneReviewBar]). Null for a project's file.
  final String? agentId;
  final Widget glyph;
  final VoidCallback onBack;
  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    return PaneChrome(
      phone: PhonePane(
        title: paneTitle(leaf, AppScope.of(context).workspace.workspace),
        onBack: onBack,
        onRemove: onClose, // null for a project's file: nothing to remove
      ),
      child: Builder(
        builder: (context) {
          final node = this.node, agentId = this.agentId;
          Widget viewer(BuildContext context) {
            // An app's view (C1F) is its own pane; any other leaf a viewer.
            final body = leaf.kind == appKind
                ? AppPane(leaf: leaf)
                : viewerBody(context, leaf);
            if (agentId == null || leaf.path.isEmpty) return body;
            return PhoneReviewBar(
              agentId: agentId,
              path: leaf.path,
              kind: leaf.kind,
              child: body,
            );
          }

          final document = node != null && agentId != null
              ? ipadDocumentPane(
                  context,
                  node,
                  agentId: agentId,
                  viewer: viewer,
                )
              // A project's file on an iPad: the host, only to read.
              : node == null && agentId == null
              ? ipadProjectDocument(context, leaf)
              : null;
          return document ?? viewer(context);
        },
      ),
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
  const PairScreen({super.key, this.code = false});

  /// Open at "Use a code instead": where a new code is the way back (a
  /// Claktory that no longer allows this phone: Use a new code, §4).
  final bool code;

  @override
  State<PairScreen> createState() => _PairScreenState();
}

class _PairScreenState extends State<PairScreen> with WidgetsBindingObserver {
  final _code = TextEditingController();

  /// Whether the camera may be used; null until known.
  CameraAccess? _access;

  /// This phone can't scan (no camera, as in the Simulator or an emulator
  /// without one): it says where the code is to paste.
  bool get _pasteOnly => _access == CameraAccess.unavailable;

  /// The person chose to paste the code instead.
  var _pasting = false;

  /// Connecting through the account (sign in, pick one of "Your
  /// Claktories") rather than with a code: at first when this phone is
  /// signed in already.
  bool? _withAccount;

  /// Why the last code scanned can't be used.
  String? _scanError;

  /// What keeps the camera from the app now, as its picture says
  /// ([CameraAvailability]); null while it doesn't.
  String? _cameraKept;

  /// The camera's picture, kept across the square and the band.
  final _previewKey = GlobalKey(debugLabel: 'camera');

  /// A way to connect was chosen on the first screen ([_start]), shown
  /// while this device has no Claktory yet (#363 F1, F2).
  var _chose = false;

  /// A way chosen from the first screen: Back (the chevron, or Android's
  /// Back) returns to it, its choices and Send me the link.
  bool get _backToStart =>
      _chose && AppScope.scope(context).connections.saved.isEmpty;

  void _toStart() {
    if (!mounted) return;
    setState(() {
      _chose = false;
      _pasting = false;
      _withAccount = null;
      _scanError = null;
    });
  }

  /// Android's Back, while there's a first screen to go back to.
  BackHandle? _backHandle;

  void _syncBack() {
    if (!mounted) return;
    final policy = PhonePlatformPolicy.of(context);
    final back = policy.touch && !policy.drawerEdgeSwipe
        ? PhoneBack.maybeOf(context)
        : null;
    if (back != null && _backToStart) {
      _backHandle ??= back.registerNavigation(
        CallbackBackToken(onBack: _toStart),
      );
    } else {
      _backHandle?.release();
      _backHandle = null;
    }
  }

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    WidgetsBinding.instance.addPostFrameCallback((_) => _check());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _backHandle?.release();
    _code.dispose();
    super.dispose();
  }

  // Back from Settings, the camera may be allowed now.
  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) _check();
  }

  /// Finds out whether the camera may be used, asking nothing: its screen
  /// asks, after saying why ([_cameraPermission]).
  Future<void> _check() async {
    if (!mounted) return;
    final access = await AppScope.scope(context).camera.access();
    if (!mounted || access == _access) return;
    setState(() {
      _access = access;
      if (access != CameraAccess.undetermined &&
          access != CameraAccess.denied) {
        _cameraFlow = null;
      }
    });
  }

  /// The camera's permission, while its screen explains or says it's off.
  PermissionFlow? _cameraFlow;

  /// The camera's screen before it's allowed (area 5a, P7): the first time,
  /// why, with Continue, which brings the system's prompt; refused, how to
  /// turn it on, or paste the code instead.
  Widget _cameraPermission(BuildContext context, CameraAccess access) {
    final flow = _cameraFlow ??= PermissionFlow(
      CameraPermission(AppScope.scope(context).camera),
      from: access == CameraAccess.denied
          ? PermissionAccess.denied
          : PermissionAccess.undetermined,
    );
    return PermissionContent(
      key: ObjectKey(flow),
      flow: flow,
      fill: true,
      wayOut: PermissionWayOut(
        'Paste code instead',
        () => setState(() => _pasting = true),
      ),
      onDone: () {
        // Allowed, or set aside: the screen follows the camera from here.
        _cameraFlow = null;
        unawaited(_check());
      },
    );
  }

  Future<void> _paste() async {
    final pasted = await Clipboard.getData(Clipboard.kTextPlain);
    final text = pasted?.text?.trim() ?? '';
    if (text.isNotEmpty) _code.text = text;
  }

  void _scanned(String code) {
    final text = code.trim();
    // The camera keeps reading while the question is up, and the code the
    // person just turned down stays in view: neither asks again.
    if (_confirming || text == _declined) return;
    final problem = pairingProblem(text, scanned: true);
    if (problem != null) {
      setState(() => _scanError = problem);
      return;
    }
    unawaited(_connect(pastedPairingCode(text)!, code: text));
  }

  /// While the person is asked whether to connect.
  var _confirming = false;

  /// The code the person last said no to, not asked about again while the
  /// camera still sees it.
  String? _declined;

  /// Connects to [target] once the person says so: a code, scanned or
  /// pasted, asks first, as a link does (links.dart), so nothing connects
  /// without their tap.
  Future<void> _connect(RemoteFactory target, {String? code}) async {
    if (_confirming) return;
    _confirming = true;
    try {
      final yes = await confirmPairing(context, target);
      if (!mounted) return;
      if (yes) {
        // The first: why Claktory asks for the local network, before iOS
        // does.
        await connectNewClaktory(context, target);
      } else {
        _declined = code;
      }
    } finally {
      _confirming = false;
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final access = _access;
    // Your account first, a code second (area 2, §3 and 8.2): signed in,
    // or with a Claktory already here, it opens on the account (signed
    // out, its email sign-in), never on the scanner; with nothing here yet
    // and signed out, the first screen's choices.
    final connections = AppScope.scope(context).connections;
    final withAccount = _withAccount ??=
        !widget.code &&
        (connections.account.signedIn || connections.saved.isNotEmpty);
    // Nothing here yet, from the store: what's needed first, and the ways
    // in, email first (#363).
    final start =
        !_chose &&
        !withAccount &&
        AppScope.scope(context).connections.saved.isEmpty;
    final Widget body;
    if (start) {
      body = _start(
        context,
        // One this phone can't use (none, or restricted): pasting only.
        camera:
            access != CameraAccess.unavailable &&
            access != CameraAccess.restricted,
      );
    } else if (withAccount) {
      body = Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          YourClaktories(
            // Picked from the person's own account, after its own
            // "Connect to …?": nothing more to ask.
            onConnect: (target) =>
                unawaited(connectNewClaktory(context, target)),
            offerDelete: true,
          ),
          const SizedBox(height: Space.m),
          _Link(
            'Use a code instead',
            onTap: () => setState(() => _withAccount = false),
          ),
        ],
      );
    } else if (_pasting || _pasteOnly) {
      body = _pasteForm(context, noCamera: _pasteOnly);
    } else if (access == CameraAccess.granted) {
      body = _scanner(context);
    } else if (access == CameraAccess.denied ||
        access == CameraAccess.undetermined) {
      body = _cameraPermission(context, access!);
    } else if (access == CameraAccess.restricted) {
      body = _cameraRestricted(context);
    } else {
      // Finding out whether there's a camera.
      body = const SizedBox(height: 120);
    }
    WidgetsBinding.instance.addPostFrameCallback((_) => _syncBack());
    // Which screen shows: another one opens at its top, its title and Back
    // in sight, not at the scroll the last one was left at (Sale's #363
    // finding b: "Paste code instead" from the end of the first screen).
    final screen = start
        ? 'start'
        : withAccount
        ? 'account'
        : _pasting || _pasteOnly
        ? 'paste'
        : '$access';
    // The camera's own screen says what it is: no heading over it.
    final explains =
        !start &&
        !withAccount &&
        !_pasting &&
        !_pasteOnly &&
        (access == CameraAccess.undetermined || access == CameraAccess.denied);
    // The camera kept from the app: the band saying why and Paste code
    // instead stay in sight at every size, the words above scrolling when
    // they must (area 5a, P7; Lucy 48961).
    if (!start &&
        !withAccount &&
        !_pasting &&
        !_pasteOnly &&
        access == CameraAccess.granted &&
        _cameraKept != null) {
      return Padding(
        key: const ValueKey('camera-kept'),
        padding: const EdgeInsets.all(Space.xl),
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (_backToStart)
                  Align(
                    alignment: Alignment.centerLeft,
                    child: _BackButton(onBack: _toStart),
                  ),
                Flexible(
                  child: ScrollsWithFade(
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        FittedHeading(
                          'Connect to your Claktory',
                          style: TypeScale.of(context).title
                              .copyWith(color: palette.text),
                        ),
                        const SizedBox(height: Space.s),
                        Text(
                          _scanWhere,
                          style: TypeScale.of(context).body
                              .copyWith(color: palette.textMuted),
                        ),
                      ],
                    ),
                  ),
                ),
                // Why the camera's away stays in sight with the way on;
                // the words above scroll first.
                const SizedBox(height: Space.xl),
                // Twice the words' share when both must give way.
                Flexible(flex: 2, child: body),
                const SizedBox(height: Space.l),
                Button(
                  label: 'Paste code instead',
                  primary: true,
                  onPressed: () => setState(() {
                    _pasting = true;
                    // The camera says again whether it's kept, when it's
                    // back.
                    _cameraKept = null;
                  }),
                ),
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
    // The camera's own screen fills the height: its words in the middle,
    // Continue at the foot, under the thumb (area 5a, P7).
    if (explains) {
      return Padding(
        key: ValueKey(screen),
        padding: const EdgeInsets.all(Space.xl),
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (_backToStart)
                  Align(
                    alignment: Alignment.centerLeft,
                    child: _BackButton(onBack: _toStart),
                  ),
                Expanded(child: body),
                const SizedBox(height: Space.s),
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
    return Center(
      child: SingleChildScrollView(
        key: ValueKey(screen),
        padding: const EdgeInsets.all(Space.xl),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 420),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // Back to the first screen's choices, where a finger and
              // the eye expect it (#363, Sale's row 21).
              if (_backToStart)
                Align(
                  alignment: Alignment.centerLeft,
                  child: _BackButton(onBack: _toStart),
                ),
              // Wraps between words, never inside one: at the largest text
              // size in a 320-point window (Slide Over) it's drawn just
              // small enough for "Claktory" to fit (Sale's #363 finding a).
              if (!explains)
                FittedHeading(
                  'Connect to your Claktory',
                  style: TypeScale.of(context).title
                      .copyWith(color: palette.text),
                ),
              if (start) ...[
                const SizedBox(height: Space.s),
                Text(
                  'Claktory runs on a Mac or a Linux server (or Windows, in '
                  'preview). Set it up there, then connect this '
                  '$thisDeviceKind to it.',
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.textMuted),
                ),
              ] else if (!withAccount && !explains) ...[
                const SizedBox(height: Space.s),
                Text(
                  _pasteOnly || _pasting
                      ? 'Paste the code for this $thisDeviceKind. Find it in Settings → '
                            "Devices → Add a device on a device that's "
                            'already connected, or where you set up your '
                            'Claktory.'
                      : _scanWhere,
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.textMuted),
                ),
              ],
              SizedBox(height: explains ? Space.l : Space.xl),
              body,
              if (!withAccount && !start)
                _Link(
                  'Sign in with your email instead',
                  onTap: () => setState(() => _withAccount = true),
                ),
              // An Android phone pushes can't reach (no Google Play
              // services, a build without Firebase): what its people need
              // to know first. Every other phone gets notifications.
              if (defaultTargetPlatform == TargetPlatform.android)
                ValueListenableBuilder<String?>(
                  valueListenable:
                      PushRegistrar.current?.permissionNow ??
                      const AlwaysStoppedAnimation<String?>('unavailable'),
                  builder: (context, permission, _) =>
                      permission != 'unavailable'
                      ? const SizedBox.shrink()
                      : Padding(
                          padding: const EdgeInsets.only(top: Space.l),
                          child: Text(
                            "This $thisDeviceKind can't get notifications "
                            'without Google Play services. Keep Claktory '
                            'open to see what your agents need.',
                            style: TypeScale.of(context).caption
                                .copyWith(color: palette.textMuted),
                          ),
                        ),
                ),
            ],
          ),
        ),
      ),
    );
  }

  /// Where the code to scan is.
  String get _scanWhere =>
      // Android is often paired to a Claktory on a server, with no Mac in
      // sight.
      defaultTargetPlatform == TargetPlatform.android
      ? 'Scan the code for this $thisDeviceKind. Find it in Settings → '
            "Devices → Add a device on a device that's already connected, "
            'or where you set up your Claktory.'
      : 'On your Mac, open Claktory and go to Settings → Devices → Add a '
            'device. Then scan the code.';

  Widget _scanner(BuildContext context) {
    final palette = Palette.of(context);
    final error = _scanError;
    final kept = _cameraKept;
    // The same native view in the square and in the band, so the camera
    // coming back finds it still listening (Pesci).
    final preview = KeyedSubtree(
      key: _previewKey,
      // The camera kept from the app (an iPad sharing its screen, another
      // app using it): its words in a band, and Paste code instead the way
      // on until it's back (area 5a, P7; Lucy 48961).
      child: NotificationListener<CameraAvailability>(
        onNotification: (notice) {
          if (notice.unavailable != _cameraKept) {
            setState(() => _cameraKept = notice.unavailable);
          }
          return true;
        },
        child: AppScope.scope(context).camera.preview(_scanned),
      ),
    );
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (kept != null)
          // Shorter when it must be (Flexible in [build]'s kept layout),
          // its words scrolling.
          Flexible(
            child: CameraKeptBand(words: kept, preview: preview),
          )
        else
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
                  child: preview,
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
        // Kept, the way on is pinned under it instead ([build]).
        if (kept == null) ...[
          const SizedBox(height: Space.m),
          _Link(
            'Paste code instead',
            onTap: () => setState(() => _pasting = true),
          ),
        ],
      ],
    );
  }

  /// Restrictions keep the camera off (Screen Time, a managed device):
  /// Settings can't help, so pasting is the way.
  Widget _cameraRestricted(BuildContext context) {
    final palette = Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          "The camera isn't available on this $thisDeviceKind. Paste the "
          'code instead.',
          style: TypeScale.of(context).body.copyWith(color: palette.text),
        ),
        const SizedBox(height: Space.m),
        _Link(
          'Paste code instead',
          onTap: () => setState(() => _pasting = true),
        ),
      ],
    );
  }

  /// The first screen while this device has no Claktory (#363): Send me
  /// the link, to set Claktory up on a Mac or a Linux server (or Windows);
  /// then, for one already set up,
  /// Sign in with your email first, and scanning or pasting a code.
  Widget _start(BuildContext context, {required bool camera}) {
    final palette = Palette.of(context);
    void choose({bool account = false, bool paste = false}) {
      setState(() {
        _chose = true;
        _withAccount = account;
        _pasting = paste;
      });
      // The camera's screen: why first, the first time (area 5a, P7).
      if (!account && !paste) unawaited(_check());
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Builder(
          builder: (button) => Button(
            label: 'Send me the link',
            onPressed: () => unawaited(
              shareText(
                button,
                'Set up Claktory on your Mac or Linux server (or Windows, in '
                'preview): $downloadLink',
                from: globalRectOf(button),
              ),
            ),
          ),
        ),
        const SizedBox(height: Space.xl),
        Semantics(
          header: true,
          child: Text(
            'Already set up?',
            style: TypeScale.of(context).label
                .copyWith(color: palette.text, fontWeight: FontWeight.w600),
          ),
        ),
        const SizedBox(height: Space.s),
        Button(
          label: 'Sign in with your email',
          primary: true,
          onPressed: () => choose(account: true),
        ),
        const SizedBox(height: Space.m),
        if (camera) _Link('Scan code instead', onTap: choose),
        _Link('Paste code instead', onTap: () => choose(paste: true)),
      ],
    );
  }

  Widget _pasteForm(BuildContext context, {required bool noCamera}) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: _code,
      builder: (context, _) {
        final pasted = _code.text.trim();
        final error = pasted.isEmpty ? null : pairingProblem(pasted);
        final target = error == null ? pastedPairingCode(pasted) : null;
        void connect() {
          if (target != null) unawaited(_connect(target));
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
            // Never side by side on a phone (§7): Connect full width, then
            // Paste a link under it (Lucy 49738).
            Button(
              label: 'Connect',
              primary: true,
              onPressed: target != null ? connect : null,
            ),
            const SizedBox(height: Space.s),
            _Link('Paste', onTap: _paste),
            // Back to the camera, even one refused so far (friend bug #2):
            // it says how to allow it, with Open Settings. Only a camera
            // this phone can't have (none, or restricted) has no way back.
            if (!noCamera && _access != CameraAccess.restricted) ...[
              const SizedBox(height: Space.m),
              _Link(
                'Scan code instead',
                onTap: () {
                  setState(() {
                    _pasting = false;
                    _scanError = null;
                  });
                  // Allowed in Settings meanwhile: straight to the camera.
                  unawaited(_check());
                },
              ),
            ],
          ],
        );
      },
    );
  }
}

/// Where Claktory is downloaded for a Mac or a server, sent from a phone's
/// first screen (Send me the link).
const downloadLink = 'https://claktory.com/download';

/// Why [code] can't connect this phone, or null when it can: it isn't a
/// code, or it's from a Claktory before the private tunnel.
String? pairingProblem(String code, {bool scanned = false}) {
  final target = pastedPairingCode(code);
  // Another of Claktory's codes (friend bug #2): what it is, and where
  // this one's code is.
  if (target == null) {
    final other = otherCodeProblem(
      code,
      device: thisDeviceKind,
      where: defaultTargetPlatform == TargetPlatform.iOS ? onMac : whereItRuns,
      scanned: scanned,
    );
    if (other != null) return other;
  }
  // Android is often paired to a Claktory on a server, with no Mac in
  // sight.
  final android = defaultTargetPlatform == TargetPlatform.android;
  if (target == null && android) {
    return scanned
        ? "That isn't a Claktory code. Scan the one shown for this $thisDeviceKind in "
              'Settings → Devices → Add a device.'
        : "That isn't a Claktory code. Copy it again, all of it, and paste "
              'it here.';
  }
  if (target != null && !target.tunnelled && android) {
    return 'This code is from an older Claktory. Update it where it runs, '
        'then add this $thisDeviceKind again.';
  }
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
      // Its own node, the link's size, never the screen's (row 195).
      container: true,
      button: true,
      label: label,
      // Its own tap, which excludeSemantics would drop with Pressable's:
      // TalkBack and VoiceOver can activate it (Sale, #363).
      onTap: onTap,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        // At least a finger's 44, and taller for a caption that wraps at
        // large text, so none of it is cut.
        builder: (context, hovered) => ConstrainedBox(
          constraints: const BoxConstraints(minHeight: 44),
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
