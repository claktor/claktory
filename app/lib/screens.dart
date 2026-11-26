import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart' show showModal;
import 'forms.dart' show AgentSettingsForm;
import 'identity.dart' show Motion;
import 'links.dart' show LinkOpener;
import 'pipes.dart' show showPipeSettings;
import 'setup_link.dart' show SetupLinkOpener;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// The Settings sections an agent can open by id (show_screen's
/// "settings" screen), as their titles read in lower case.
const settingsSections = {
  'profile',
  'general',
  'account',
  'providers',
  'secrets',
  'pipes',
  'defaults',
  'devices',
  'resources',
  'display',
  'voice',
};

/// When the person last used this app: a click, a touch, a scroll, the
/// pointer moving over it or a key. Only an app touched lately opens what
/// an agent shows, so a Mac left on while they're out with their phone
/// doesn't.
class InputClock {
  InputClock._();

  static DateTime? _last;
  static var _installed = false;

  /// The last input, or null if there's been none since the app started.
  static DateTime? get last => _last;

  /// Starts listening to every pointer event and key, once.
  static void ensureInstalled() {
    if (_installed) return;
    _installed = true;
    GestureBinding.instance.pointerRouter.addGlobalRoute(_pointer);
    HardwareKeyboard.instance.addHandler(_key);
  }

  static void _pointer(PointerEvent event) {
    if (event is PointerAddedEvent || event is PointerRemovedEvent) return;
    _last = DateTime.now();
  }

  static bool _key(KeyEvent event) {
    _last = DateTime.now();
    return false;
  }

  /// Sets the last input, for tests.
  @visibleForTesting
  static void setLast(DateTime? at) => _last = at;
}

/// Opens what an agent asks to show with its show_screen tool, in this
/// window: a section of Settings, the Map, Usage, the Marketplace, What's
/// new, an agent's conversation or its settings, a pipe or its settings.
/// It answers the backend
/// whether it did. It opens only while the window is on screen (on a
/// phone, in front) and the person used this app in the last
/// [recentInput], and never takes focus from another app or device.
class ScreenOpener extends StatefulWidget {
  const ScreenOpener({super.key, required this.covered, required this.child});

  /// Something is over the window (a first-launch step), so nothing
  /// opened under it would be seen.
  final bool covered;

  final Widget child;

  /// How lately the person must have used this app.
  static const recentInput = Duration(minutes: 5);

  /// What tells a phone's layout to push the selected agent's
  /// conversation over its list, once an agent's conversation is opened.
  static Listenable? conversations(BuildContext context) =>
      context.getInheritedWidgetOfExactType<_OpenedConversations>()?.pulse;

  /// Shows [agentId]'s conversation in place of any page, and on a phone
  /// pushes it over the list, as a click on its row does. [context] is
  /// below the window's [ScreenOpener].
  static void openConversation(BuildContext context, String agentId) => _show(
    context,
    agentId,
    context.getInheritedWidgetOfExactType<_OpenedConversations>()?.pulse,
  );

  static void _show(BuildContext context, String agentId, _Pulse? pushed) {
    final backend = AppScope.of(context);
    AppScope.closePage(context);
    if (backend.workspace.workspace.selectedAgentId != agentId) {
      fireAndLog(backend.selectAgent(agentId));
    }
    pushed?.pulse();
  }

  @override
  State<ScreenOpener> createState() => _ScreenOpenerState();
}

class _ScreenOpenerState extends State<ScreenOpener> {
  Backend? _backend;
  final _conversations = _Pulse();

  /// The agent whose settings this opened and are still up, if any.
  String? _agentSettings;

  /// The pipe whose settings this opened and are still up, if any.
  String? _pipeSettings;

  static Pipe? _pipe(Backend backend, String id) {
    for (final pipe in backend.workspace.workspace.pipes) {
      if (pipe.id == id) return pipe;
    }
    return null;
  }

  /// Shows [pipe] in place of any page, and on a phone over the list.
  void _showPipe(Backend backend, Pipe pipe) {
    AppScope.closePage(context);
    if (backend.workspace.workspace.selectedPipeId != pipe.id) {
      fireAndLog(backend.selectPipe(pipe.id));
    }
    _conversations.pulse();
  }

  @override
  void initState() {
    super.initState();
    InputClock.ensureInstalled();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (backend == _backend) return;
    _release();
    _backend = backend..showScreen = _open;
  }

  @override
  void dispose() {
    _release();
    _conversations.dispose();
    super.dispose();
  }

  void _release() {
    final backend = _backend;
    if (backend != null && backend.showScreen == _open) {
      backend.showScreen = null;
    }
    _backend = null;
  }

  /// Whether the window is on screen. A Mac's stays visible while another
  /// app is in front (inactive); a phone's only while it's in front.
  bool get _visible {
    final state = WidgetsBinding.instance.lifecycleState;
    // Not said yet, before the platform's first word on it: the person's
    // recent input speaks for it.
    if (state == null || state == AppLifecycleState.resumed) return true;
    final desktop = switch (defaultTargetPlatform) {
      TargetPlatform.iOS || TargetPlatform.android => false,
      _ => true,
    };
    return desktop && state == AppLifecycleState.inactive;
  }

  bool get _recentlyUsed {
    final last = InputClock.last;
    return last != null &&
        DateTime.now().difference(last) <= ScreenOpener.recentInput;
  }

  bool _open(OpenScreen request) {
    if (!mounted || widget.covered || !_visible || !_recentlyUsed) {
      return false;
    }
    final backend = _backend!;
    final target = request.target;
    final id = target.id;
    switch (target.kind) {
      // Apps moved from Settings to the Marketplace: an older backend's
      // settings/apps opens them there.
      case 'settings' when id == 'apps':
        AppScope.openPage(context, AppPage.marketplace);
        backend.marketplaceTab.value = 'apps';
        return true;
      case 'settings':
        AppScope.openPage(context, AppPage.settings);
        backend.settingsSection.value = settingsSections.contains(id)
            ? id
            : null;
        return true;
      case 'map':
        AppScope.openPage(context, AppPage.map);
        return true;
      case 'usage':
        AppScope.openPage(context, AppPage.usage);
        return true;
      case 'monitors':
        AppScope.openPage(context, AppPage.monitors);
        return true;
      case 'marketplace':
        AppScope.openPage(context, AppPage.marketplace);
        // A tab by its id ("apps"); the Marketplace ignores one it lacks.
        if (id.isNotEmpty) backend.marketplaceTab.value = id;
        return true;
      case 'changelog':
        AppScope.openPage(context, AppPage.changelog);
        return true;
      case 'agent':
        if (backend.workspace.agent(id) == null) return false;
        // Its own context is above what it provides: no lookup.
        ScreenOpener._show(context, id, _conversations);
        return true;
      // A pipe's view, as its row in the sidebar opens it (#471 G9); on
      // a phone over the list, as an agent's conversation is.
      case 'pipe':
        final pipe = _pipe(backend, id);
        if (pipe == null) return false;
        _showPipe(backend, pipe);
        return true;
      // Its settings, over its view (#471 G9).
      case 'pipe_settings':
        final pipe = _pipe(backend, id);
        if (pipe == null) return false;
        _showPipe(backend, pipe);
        // Already up: it stays as it is, with what the person typed.
        if (_pipeSettings == id) return true;
        _pipeSettings = id;
        showPipeSettings(context, pipe).whenComplete(() {
          if (_pipeSettings == id) _pipeSettings = null;
        });
        return true;
      case 'agent_settings':
        final agent = backend.workspace.agent(id);
        if (agent == null) return false;
        // Already up: it stays as it is, with what the person typed.
        if (_agentSettings == id) return true;
        _agentSettings = id;
        showModal(
          context,
          (context, close) => AgentSettingsForm(agent: agent, close: close),
        ).whenComplete(() {
          if (_agentSettings == id) _agentSettings = null;
        });
        return true;
    }
    return false;
  }

  @override
  Widget build(BuildContext context) => _OpenedConversations(
    pulse: _conversations,
    // Links that open the app (claktory://add?code=…) open here too, and
    // so does a setup on this Mac connecting it (setup_link.dart).
    child: LinkOpener(child: SetupLinkOpener(child: widget.child)),
  );
}

class _Pulse extends ChangeNotifier {
  void pulse() => notifyListeners();
}

/// Where a phone's layout hears that a conversation was opened. Its
/// listeners subscribe themselves; nothing below rebuilds for it.
class _OpenedConversations extends InheritedWidget {
  const _OpenedConversations({required this.pulse, required super.child});

  final _Pulse pulse;

  @override
  bool updateShouldNotify(_OpenedConversations oldWidget) =>
      pulse != oldWidget.pulse;
}

/// Settings' scroll view, which brings the section an agent opened
/// ([Backend.settingsSection]) into view, its title at the top, in `base`.
/// [builder] makes the scroll view with the controller it's given; each
/// section in it is a [SettingsSectionAnchor].
class SettingsScroller extends StatefulWidget {
  const SettingsScroller({super.key, required this.builder});

  final Widget Function(BuildContext context, ScrollController controller)
  builder;

  @override
  State<SettingsScroller> createState() => _SettingsScrollerState();
}

class _SettingsScrollerState extends State<SettingsScroller> {
  final _controller = ScrollController();
  final _anchors = <String, BuildContext>{};
  ValueNotifier<String?>? _section;

  /// How many screens down it looks for a section not built yet (the
  /// list builds only what's near the view).
  static const _maxSteps = 20;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final section = AppScope.of(context).settingsSection;
    if (section == _section) return;
    _section?.removeListener(_wanted);
    _section = section..addListener(_wanted);
    // Opened for it: once the first frame has laid the list out.
    _wanted();
  }

  @override
  void dispose() {
    _section?.removeListener(_wanted);
    _controller.dispose();
    super.dispose();
  }

  void _wanted() {
    if (_section?.value == null) return;
    WidgetsBinding.instance.addPostFrameCallback((_) => _seek(0));
  }

  void _seek(int step) {
    final section = _section;
    final id = section?.value;
    if (!mounted || section == null || id == null) return;
    final anchor = _anchors[id];
    if (anchor != null && anchor.mounted) {
      section.value = null;
      Scrollable.ensureVisible(
        anchor,
        alignment: 0,
        duration: Motion.base,
        curve: Curves.easeOut,
      );
      return;
    }
    // Not built yet: a screen further down, then look again.
    if (_controller.hasClients && step < _maxSteps) {
      final position = _controller.position;
      if (position.pixels < position.maxScrollExtent) {
        _controller.jumpTo(
          (position.pixels + position.viewportDimension).clamp(
            position.minScrollExtent,
            position.maxScrollExtent,
          ),
        );
        WidgetsBinding.instance.addPostFrameCallback((_) => _seek(step + 1));
        return;
      }
    }
    // Not in Settings now (Account while signed out, say).
    section.value = null;
  }

  @override
  Widget build(BuildContext context) => widget.builder(context, _controller);
}

/// Marks a section of Settings by its id ("devices"), so
/// [SettingsScroller] can bring it into view.
class SettingsSectionAnchor extends StatefulWidget {
  const SettingsSectionAnchor({
    super.key,
    required this.id,
    required this.child,
  });

  final String id;
  final Widget child;

  @override
  State<SettingsSectionAnchor> createState() => _SettingsSectionAnchorState();
}

class _SettingsSectionAnchorState extends State<SettingsSectionAnchor> {
  _SettingsScrollerState? _scroller;

  @override
  void initState() {
    super.initState();
    _scroller = context.findAncestorStateOfType<_SettingsScrollerState>();
    _scroller?._anchors[widget.id] = context;
  }

  @override
  void didUpdateWidget(SettingsSectionAnchor oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.id == widget.id) return;
    _forget(oldWidget.id);
    _scroller?._anchors[widget.id] = context;
  }

  @override
  void dispose() {
    _forget(widget.id);
    super.dispose();
  }

  void _forget(String id) {
    final anchors = _scroller?._anchors;
    if (anchors != null && anchors[id] == context) anchors.remove(id);
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
