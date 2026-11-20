import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_view.dart';
import 'backend.dart';
import 'changelog.dart';
import 'connections.dart';
import 'controls.dart';
import 'extensions.dart';
import 'identity.dart';
import 'layout.dart';
import 'map.dart';
import 'marketplace.dart';
import 'pairing.dart';
import 'panes.dart' show pageOfKind, paneGlyph, paneTitle;
import 'protocol/factory/v1/factory.pb.dart';
import 'rail.dart' show ClaktoriesBackRow;
import 'scanner.dart';
import 'scope.dart';
import 'settings_view.dart';
import 'sidebar.dart';
import 'pane_chrome.dart';
import 'viewers/registry.dart';

/// The app at a phone's size (docs/IDENTITY.md, "Phone"): the sidebar is
/// the first screen, an agent's conversation is pushed over it with a back
/// button, its panels open as sheets, and the message box rides above the
/// keyboard. A phone with nothing paired opens to pairing.
class PhoneShell extends StatefulWidget {
  const PhoneShell({super.key});

  @override
  State<PhoneShell> createState() => _PhoneShellState();
}

class _PhoneShellState extends State<PhoneShell> {
  /// Whether the selected agent's conversation is pushed over the list.
  final _conversation = ValueNotifier(false);

  /// Whether the sheet listing the agent's extensions is up.
  var _listing = false;

  /// The pane of the viewer pushed over the conversation, if one is.
  String? _viewer;

  @override
  void dispose() {
    _conversation.dispose();
    super.dispose();
  }

  void _back(Agent agent) {
    final history = AppScope.scope(context).history;
    final back = history.back;
    // Back to the agent it came from by a message, else to the list.
    if (back != null && back != agent.id) {
      history.goBack(agent.id);
    } else {
      _conversation.value = false;
    }
  }

  @override
  Widget build(BuildContext context) {
    final scope = AppScope.scope(context);
    final backend = scope.backend;
    final safe = MediaQuery.paddingOf(context);
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    return PhoneNavigation(
      conversation: _conversation,
      child: ListenableBuilder(
        listenable: Listenable.merge([
          backend.workspace,
          scope.page,
          scope.connections,
          _conversation,
          backend.pairing,
        ]),
        builder: (context, _) {
          // Under the status bar, and above the home indicator or the
          // keyboard, whichever is higher.
          // On its side, the notch and the rounded corners are at the
          // left and right.
          Widget screen(Widget child, {bool overKeyboard = false}) => Padding(
            padding: EdgeInsets.only(
              left: safe.left,
              right: safe.right,
              top: safe.top,
              bottom: overKeyboard
                  ? math.max(safe.bottom, keyboard)
                  : safe.bottom,
            ),
            child: MediaQuery.removePadding(
              context: context,
              removeTop: true,
              removeBottom: true,
              removeLeft: true,
              removeRight: true,
              child: child,
            ),
          );
          if (scope.connections.current == null && !backend.localBackend) {
            return screen(const PairScreen(), overKeyboard: true);
          }
          // A code used, waiting to be allowed: the number to compare.
          final waiting = scope.connections.current;
          if (backend.pairing.value != null && waiting != null) {
            return screen(
              PairingWaitView(
                onCancel: () => scope.connections.forget(waiting),
              ),
            );
          }
          switch (scope.page.value) {
            case AppPage.settings:
              return screen(const SettingsView(), overKeyboard: true);
            case AppPage.marketplace:
              return screen(const MarketplaceView());
            case AppPage.changelog:
              return screen(const ChangelogView());
            case AppPage.map:
              return screen(const MapView());
            case AppPage.agent:
          }
          final workspace = backend.workspace.workspace;
          final agent = _conversation.value
              ? backend.workspace.selectedAgent
              : null;
          final panel = openPanelOf(workspace, agent);
          final viewer = agent == null || _viewer == null
              ? null
              : find(layoutOf(agent), _viewer!);
          return Stack(
            fit: StackFit.expand,
            children: [
              screen(
                const Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    ClaktoriesBackRow(),
                    Expanded(child: Sidebar(width: double.infinity)),
                  ],
                ),
              ),
              AnimatedSwitcher(
                duration: Motion.base,
                switchInCurve: Curves.easeOut,
                switchOutCurve: Curves.easeIn,
                transitionBuilder: (child, animation) => SlideTransition(
                  position: Tween(
                    begin: const Offset(1, 0),
                    end: Offset.zero,
                  ).animate(animation),
                  child: child,
                ),
                child: agent == null
                    ? const SizedBox.shrink(key: ValueKey('list'))
                    : ColoredBox(
                        key: const ValueKey('conversation'),
                        color: Palette.of(context).canvas,
                        child: screen(
                          AnimatedContainer(
                            duration: Motion.base,
                            color: conversationTint(agent),
                            child: AgentView(
                              key: ValueKey(agent.id),
                              agent: agent,
                              onBack: () => _back(agent),
                              onMore: () => setState(() => _listing = true),
                            ),
                          ),
                          overKeyboard: true,
                        ),
                      ),
              ),
              AnimatedSwitcher(
                duration: Motion.base,
                switchInCurve: Curves.easeOut,
                switchOutCurve: Curves.easeIn,
                transitionBuilder: (child, animation) => SlideTransition(
                  position: Tween(
                    begin: const Offset(1, 0),
                    end: Offset.zero,
                  ).animate(animation),
                  child: child,
                ),
                child: agent == null || viewer == null
                    ? const SizedBox.shrink(key: ValueKey('no viewer'))
                    : ColoredBox(
                        key: ValueKey('viewer ${viewer.id}'),
                        color: Palette.of(context).canvas,
                        child: screen(
                          _ViewerScreen(
                            agent: agent,
                            pane: viewer,
                            onBack: () => setState(() => _viewer = null),
                          ),
                        ),
                      ),
              ),
              if (agent != null && _listing)
                _Sheet(
                  onClose: () => setState(() => _listing = false),
                  child: _ExtensionList(
                    agent: agent,
                    chosen: () => setState(() => _listing = false),
                    viewed: (id) => setState(() {
                      _listing = false;
                      _viewer = id;
                    }),
                  ),
                ),
              if (agent != null && panel != null)
                _Sheet(
                  tall: true,
                  onClose: () => fireAndLog(backend.openPanel(agent.id, '')),
                  child: ExtensionPanel(
                    key: ValueKey('${panel.id}/${agent.id}'),
                    extension: panel,
                    agent: agent,
                    width: double.infinity,
                  ),
                ),
            ],
          );
        },
      ),
    );
  }
}

/// A sheet from the bottom of the screen over a dimmed view: a floating
/// layer (docs/IDENTITY.md), its top corners rounded. A tap on the dimmed
/// part closes it.
class _Sheet extends StatelessWidget {
  const _Sheet({required this.onClose, required this.child, this.tall = false});

  final VoidCallback onClose;
  final Widget child;

  /// Nearly the whole screen, for a panel; else as tall as its content.
  final bool tall;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final size = MediaQuery.sizeOf(context);
    final safe = MediaQuery.paddingOf(context);
    // A panel leaves the agent's header in view above it, dimmed, to tap
    // closed.
    final room = size.height - safe.top - titleBarHeight;
    final sheet = Container(
      constraints: BoxConstraints(maxHeight: room),
      height: tall ? room : null,
      padding: EdgeInsets.only(bottom: tall ? 0 : safe.bottom),
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: const BorderRadius.vertical(
          top: Radius.circular(Radii.floating),
        ),
        border: Border.all(color: palette.hairline),
        boxShadow: floatingShadow,
      ),
      clipBehavior: Clip.antiAlias,
      child: MediaQuery.removePadding(
        context: context,
        removeTop: true,
        child: child,
      ),
    );
    return TweenAnimationBuilder<double>(
      tween: Tween(begin: 0, end: 1),
      duration: Motion.base,
      curve: Curves.easeOut,
      builder: (context, shown, child) => Stack(
        fit: StackFit.expand,
        children: [
          GestureDetector(
            onTap: onClose,
            child: ColoredBox(
              color: const Color(0xFF000000).withValues(alpha: 0.3 * shown),
            ),
          ),
          Align(
            alignment: Alignment.bottomCenter,
            child: FractionalTranslation(
              translation: Offset(0, 1 - shown),
              child: child,
            ),
          ),
        ],
      ),
      child: sheet,
    );
  }
}

/// The agent's extensions, one 44pt row each (a finger's size): choosing
/// one opens its panel.
class _ExtensionList extends StatelessWidget {
  const _ExtensionList({
    required this.agent,
    required this.chosen,
    required this.viewed,
  });

  final Agent agent;
  final VoidCallback chosen;

  /// A viewer beside the conversation was chosen: its pane.
  final ValueChanged<String> viewed;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final extensions = enabledExtensions(backend.workspace.workspace, agent);
    final viewers = [
      for (final pane in leaves(layoutOf(agent)))
        // Settings and other pages a Mac shows beside the conversation are
        // the phone's own screens here.
        if (pane.leaf.kind != chatKind && pageOfKind(pane.leaf.kind) == null)
          pane,
    ];
    Widget row({
      required Widget glyph,
      required String name,
      required VoidCallback onTap,
    }) => Pressable(
      onTap: onTap,
      builder: (context, hovered) => Container(
        height: 44,
        padding: const EdgeInsets.symmetric(horizontal: Space.s),
        decoration: BoxDecoration(
          color: hovered ? palette.hover : null,
          borderRadius: BorderRadius.circular(Radii.control),
        ),
        child: Row(
          children: [
            SizedBox(width: 16, child: Center(child: glyph)),
            const SizedBox(width: Space.m),
            Expanded(
              child: Text(
                name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyles.body.copyWith(color: palette.text),
              ),
            ),
          ],
        ),
      ),
    );
    Widget heading(String text) => Padding(
      padding: const EdgeInsets.all(Space.s),
      child: Text(
        text,
        style: TextStyles.caption.copyWith(
          color: palette.textMuted,
          fontWeight: FontWeight.w500,
        ),
      ),
    );
    return SingleChildScrollView(
      padding: const EdgeInsets.all(Space.s),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (viewers.isNotEmpty) ...[
            heading('Files'),
            for (final pane in viewers)
              row(
                glyph: paneGlyph(agent, pane.leaf, palette.textMuted),
                name: paneTitle(pane.leaf),
                onTap: () => viewed(pane.id),
              ),
            const SizedBox(height: Space.s),
          ],
          heading(
            extensions.isEmpty
                ? '${agent.name} has no extensions turned on.'
                : "${agent.name}'s extensions",
          ),
          for (final extension in extensions)
            row(
              glyph: ExtensionGlyph(
                icon: extension.icon,
                name: extension.name,
                color: palette.textMuted,
              ),
              name: extension.name,
              onTap: () {
                chosen();
                fireAndLog(backend.openPanel(agent.id, extension.id));
              },
            ),
        ],
      ),
    );
  }
}

/// A viewer pushed over the conversation: a bar with a back button, its
/// title, its controls and its ×, then the viewer.
class _ViewerScreen extends StatelessWidget {
  const _ViewerScreen({
    required this.agent,
    required this.pane,
    required this.onBack,
  });

  final Agent agent;
  final ViewNode pane;
  final VoidCallback onBack;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    Widget bar(String details, List<Widget> controls) => Container(
      height: titleBarHeight,
      padding: const EdgeInsets.symmetric(horizontal: Space.xs),
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      child: Row(
        children: [
          GlyphButton(
            label: 'Back',
            active: false,
            onTap: onBack,
            glyph: (color) => ExtensionGlyph(
              icon: 'chevron-left',
              name: 'Back',
              color: color,
            ),
          ),
          const SizedBox(width: Space.xs),
          paneGlyph(agent, pane.leaf, palette.textMuted),
          const SizedBox(width: Space.s),
          Expanded(
            child: Text(
              paneTitle(pane.leaf),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyles.label.copyWith(color: palette.text),
            ),
          ),
          ...controls,
          GlyphButton(
            label: 'Close',
            active: false,
            onTap: () {
              onBack();
              final closed = close(layoutOf(agent), pane.id);
              if (closed != null) {
                fireAndLog(
                  backend.setLayout(
                    agent.id,
                    isChatOnly(closed) ? null : closed,
                  ),
                );
              }
            },
            glyph: (color) =>
                ExtensionGlyph(icon: 'x', name: 'Close', color: color),
          ),
        ],
      ),
    );
    return PaneChrome(
      header: bar,
      child: Builder(builder: (context) => viewerBody(context, pane.leaf)),
    );
  }
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
    final Widget body;
    if (_pasting || access == CameraAccess.unavailable) {
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
                style: TextStyles.title.copyWith(color: palette.text),
              ),
              const SizedBox(height: Space.s),
              Text(
                'On your Mac, open Claktory and go to Settings → Devices → '
                'Add a device. Then scan the code.',
                style: TextStyles.body.copyWith(color: palette.textMuted),
              ),
              const SizedBox(height: Space.xl),
              body,
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
            style: TextStyles.caption.copyWith(color: palette.danger),
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
          style: TextStyles.body.copyWith(color: palette.text),
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
              onSubmitted: (_) => connect(),
            ),
            if (error != null) ...[
              const SizedBox(height: Space.s),
              Text(
                error,
                style: TextStyles.caption.copyWith(color: palette.danger),
              ),
            ],
            const SizedBox(height: Space.l),
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
              style: TextStyles.caption.copyWith(
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
