import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart' show RequestFailure, fireAndLog;
import '../controls.dart';
import '../identity.dart';
import '../mobile/back.dart' show BackHandle, CallbackBackToken, PhoneBack;
import '../mobile/device.dart' show WindowControls, windowControls;
import '../mobile/header.dart';
import '../mobile/lines.dart' show CardActions;
import '../mobile/metrics.dart' show PhoneType, TypeScale;
import '../mobile/navigation.dart';
import '../ipad/workbench.dart' show keyboardOverWindow;
import '../layout.dart' show layoutOf;
import '../panes.dart' show BandChip, browserPane, withBrowser, paneFitOf;
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import '../state.dart' show WorkspaceStore;
import '../visible_codes.dart' show runeCode, visibleCodes;
import '../zoom.dart' show ZoomScope;
import 'live_input.dart';
import 'live_view.dart';
import 'tab_view.dart';

// What the person sees of an agent's browser: its live view, watch only,
// beside its conversation; and the tab a check request hands them, filling
// the window, with their input. Above the page, Claktory's own line names
// the agent and the site, which the page can't draw over; the page's title,
// when there is one, goes under it as the page's own words.

/// The widest frame worth asking for in a box [width] points wide.
int _frameWidth(BuildContext context, double width) =>
    (width * MediaQuery.devicePixelRatioOf(context)).clamp(640, 2048).round();

/// The page's own title as shown: at most [pageTitleMost] characters, and
/// whatever could reorder it, hide in it or break its line written out as
/// codes, so it stays inside its quotes and reads as the page's.
String pageTitleShown(String title) {
  final runes = title.runes.toList();
  final cut = runes.length > pageTitleMost;
  final shown =
      visibleCodes(
        String.fromCharCodes(cut ? runes.take(pageTitleMost) : runes),
      ).replaceAllMapped(
        RegExp('[\n\r\t]'),
        (match) => runeCode(match[0]!.codeUnitAt(0)),
      );
  return cut ? '$shown…' : shown;
}

/// The most of a page's title shown.
const pageTitleMost = 120;

/// "Yukako's browser · x.com", then the page's title as its own. The site
/// is never cut: a long name gives way, and the site goes under it when
/// both don't fit.
class TabSiteLine extends StatelessWidget {
  const TabSiteLine({super.key, required this.name, required this.view});

  final String name;
  final AgentTabView view;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final muted = type.label.copyWith(color: palette.textMuted);
    return ListenableBuilder(
      listenable: Listenable.merge([view, view.frames]),
      builder: (context, _) {
        final title = view.frames.frame?.title ?? '';
        final site = view.site;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            Wrap(
              spacing: Space.xs,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                Text(
                  site.isEmpty ? "$name's browser" : "$name's browser ·",
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: muted,
                ),
                if (site.isNotEmpty)
                  Text(
                    site,
                    style: muted.copyWith(
                      color: palette.text,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
              ],
            ),
            if (title.isNotEmpty)
              Text(
                'The page says “${pageTitleShown(title)}”',
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: type.caption.copyWith(color: palette.textFaint),
              ),
          ],
        );
      },
    );
  }
}

/// A line of words where the page would be: starting, ended, moved.
class _Note extends StatelessWidget {
  const _Note(this.text, {this.action});

  final String text;
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(Space.xl),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              text,
              textAlign: TextAlign.center,
              style: TypeScale.of(context).body
                  .copyWith(color: palette.textMuted),
            ),
            if (action != null) ...[const SizedBox(height: Space.m), action!],
          ],
        ),
      ),
    );
  }
}

/// Claktory's words over the page, on a plate of their own.
class _Plate extends StatelessWidget {
  const _Plate(this.text, {this.liveRegion = false});

  final String text;

  /// Read out as it comes, by VoiceOver and TalkBack.
  final bool liveRegion;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      liveRegion: liveRegion,
      child: Container(
        padding: const EdgeInsets.all(Space.m),
        decoration: BoxDecoration(
          color: palette.sidebar,
          borderRadius: BorderRadius.circular(Radii.panel),
          border: Border.all(color: palette.hairline),
        ),
        child: Text(
          text,
          textAlign: TextAlign.center,
          style: TypeScale.of(context).body.copyWith(color: palette.text),
        ),
      ),
    );
  }
}

/// Why [view] shows no page, as a note; null while it shows one.
Widget? _viewNote(AgentTabView view, String name) {
  final error = view.error;
  if (error != null) {
    return _Note(
      error,
      action: Button(label: 'Try again', onPressed: view.again),
    );
  }
  final ended = view.ended;
  if (ended != null) {
    if (ended.reason == AgentTabViewEnd.AGENT_TAB_VIEW_END_MOVED) {
      final device = ended.device;
      return _Note(
        device.isEmpty
            ? 'Showing on another device now.'
            : 'Showing on $device now.',
        action: Button(label: 'Show here', onPressed: view.again),
      );
    }
    return _Note(
      ended.message.isNotEmpty ? ended.message : "$name's browser is closed.",
    );
  }
  if (view.frames.frame == null) return const _Note('Starting…');
  return null;
}

/// [agentId]'s browser, live and watch only: in a pane beside its
/// conversation, or as a phone's page ([showBrowserWatch]).
class BrowserWatch extends StatefulWidget {
  const BrowserWatch({super.key, required this.agentId, this.onBack});

  final String agentId;

  /// A phone's page: its header has Back.
  final VoidCallback? onBack;

  @override
  State<BrowserWatch> createState() => _BrowserWatchState();
}

class _BrowserWatchState extends State<BrowserWatch> {
  AgentTabView? _view;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _view ??= AgentTabViews.of(AppScope.of(context)).open(
      widget.agentId,
      maxWidth: _frameWidth(context, MediaQuery.sizeOf(context).width),
    );
  }

  @override
  void dispose() {
    _view?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final view = _view!;
    final name =
        AppScope.of(context).workspace.agent(widget.agentId)?.name ??
        'The agent';
    final onBack = widget.onBack;
    return ColoredBox(
      color: palette.canvas,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (onBack != null)
            PhoneHeader(
              leading: HeaderButton.back(onTap: onBack),
              title: Text(
                "$name's browser",
                style: PhoneType.emphasis.copyWith(color: palette.text),
              ),
            ),
          Container(
            padding: const EdgeInsets.symmetric(
              horizontal: Space.l,
              vertical: Space.s,
            ),
            decoration: BoxDecoration(
              border: Border(bottom: BorderSide(color: palette.hairline)),
            ),
            child: TabSiteLine(name: name, view: view),
          ),
          Expanded(
            // A picture of the page: VoiceOver and TalkBack say what it is.
            child: Semantics(
              container: true,
              image: true,
              label: "$name's browser, live",
              child: ListenableBuilder(
                listenable: Listenable.merge([view, view.frames]),
                builder: (context, _) =>
                    _viewNote(view, name) ??
                    ColoredBox(
                      color: palette.sidebar,
                      child: LiveScreen(
                        frames: view.frames,
                        acting: false,
                        onInput: (_) {},
                        maxScale: 1 / MediaQuery.devicePixelRatioOf(context),
                        // Read as a document: from the top.
                        alignment: Alignment.topCenter,
                        zoom:
                            defaultTargetPlatform == TargetPlatform.iOS ||
                            defaultTargetPlatform == TargetPlatform.android,
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

/// [agentId]'s browser as a phone's page over its conversation; false
/// where there are no phone pages (a Mac's or an iPad's window, where it's
/// a pane).
bool showBrowserWatch(BuildContext context, String agentId) {
  final routes = PhoneRoutes.of(context);
  if (routes == null) return false;
  pushBrowserWatch(routes, agentId);
  return true;
}

/// [agentId]'s browser pushed as a page on [routes].
void pushBrowserWatch(PhoneRoutes routes, String agentId) {
  routes.closeDrawer();
  unawaited(
    routes.push<void>(
      PhoneRoute(
        id: 'browser:$agentId',
        builder: (context) =>
            BrowserWatch(agentId: agentId, onBack: () => routes.pop<void>()),
      ),
    ),
  );
}

/// Shows [agent]'s browser, watch only: a page on a phone, else a pane
/// beside its conversation (glowing when it's there already), the agent
/// chosen first.
void watchBrowser(BuildContext context, Agent agent) {
  if (showBrowserWatch(context, agent.id)) return;
  final scope = AppScope.scope(context);
  final backend = scope.backend;
  if (scope.page.value != AppPage.agent) AppScope.closePage(context);
  if (backend.workspace.workspace.selectedAgentId != agent.id) {
    fireAndLog(backend.selectAgent(agent.id));
  }
  final root = layoutOf(agent);
  final shown = browserPane(root);
  if (shown != null) {
    backend.highlights.glow('pane:${agent.id}:${shown.id}', [
      agentColor(agent.color),
    ]);
    return;
  }
  fireAndLog(
    backend.setLayout(agent.id, withBrowser(root, fit: paneFitOf(backend))),
  );
}

/// The check request at [seq] in [agentId]'s conversation: the site it's
/// on, and the agent's words about it.
typedef CheckRequest = ({String agentId, int seq, String site, String what});

/// Opens the tab [check] holds, live, for the person to solve: filling the
/// window with [back] ("Back to Yukako", "Back to Needs you") over it, or
/// a phone's page. Esc or Back leaves the request waiting; Done and Can't
/// do this answer it, and the view closes.
void showCheckView(
  BuildContext context,
  CheckRequest check, {
  required String back,
}) {
  final routes = PhoneRoutes.of(context);
  if (routes != null) {
    routes.closeDrawer();
    unawaited(
      routes.push<void>(
        PhoneRoute(
          id: 'check:${check.agentId}:${check.seq}',
          builder: (context) => CheckView(
            check: check,
            back: back,
            onClose: () => routes.pop<void>(),
          ),
        ),
      ),
    );
    return;
  }
  final overlay = Overlay.of(context);
  late final OverlayEntry entry;
  BackHandle? handle;
  var closed = false;
  void close() {
    if (closed) return;
    closed = true;
    handle?.release();
    entry.remove();
    entry.dispose();
  }

  entry = OverlayEntry(
    // What it covers isn't read either: VoiceOver and TalkBack stay on the
    // tab, as on a dialog.
    builder: (context) => BlockSemantics(
      child: CheckView(check: check, back: back, onClose: close),
    ),
  );
  overlay.insert(entry);
  handle = PhoneBack.maybeOf(context)
      ?.register(CallbackBackToken(onBack: close, onRetired: close));
}

/// The check's tab, live, taking the person's pointer, wheel and keys.
class CheckView extends StatefulWidget {
  const CheckView({
    super.key,
    required this.check,
    required this.back,
    required this.onClose,
  });

  final CheckRequest check;
  final String back;
  final VoidCallback onClose;

  @override
  State<CheckView> createState() => _CheckViewState();
}

class _CheckViewState extends State<CheckView> {
  AgentTabView? _view;
  final _focus = FocusNode(debugLabel: 'Check');
  final _typing = TextEditingController();
  var _typed = '';
  var _busy = false;
  String? _error;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final view = _view ??= AgentTabViews.of(AppScope.of(context)).open(
      widget.check.agentId,
      requestSeq: widget.check.seq,
      maxWidth: _frameWidth(context, MediaQuery.sizeOf(context).width),
    );
    view.removeListener(_ended);
    view.addListener(_ended);
    final workspace = AppScope.of(context).workspace;
    if (!identical(workspace, _workspace)) {
      _workspace?.removeListener(_waiting);
      _workspace = workspace..addListener(_waiting);
    }
  }

  WorkspaceStore? _workspace;
  var _closed = false;

  void _close() {
    if (_closed) return;
    _closed = true;
    widget.onClose();
  }

  /// Answered or taken back on another device, which was showing the tab:
  /// Claktory tells only the connection watching it, and every one that
  /// the request no longer waits.
  void _waiting() {
    final agent = _workspace?.agent(widget.check.agentId);
    final seq = widget.check.seq;
    if (agent == null ||
        !agent.waitingSeqs.any((waiting) => waiting.toInt() == seq)) {
      _close();
    }
  }

  @override
  void initState() {
    super.initState();
    _typing.addListener(() => _typedChanged(_typing.text));
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && !_focus.hasFocus) _focus.requestFocus();
    });
  }

  /// Answered or taken back elsewhere: nothing left to do here.
  void _ended() {
    if (_view?.ended?.reason ==
        AgentTabViewEnd.AGENT_TAB_VIEW_END_REQUEST_ENDED) {
      _close();
    }
  }

  @override
  void dispose() {
    _workspace?.removeListener(_waiting);
    _view?.removeListener(_ended);
    _view?.dispose();
    _focus.dispose();
    _typing.dispose();
    super.dispose();
  }

  Future<void> _answer({required bool dismiss}) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context)
          .answerAsk(widget.check.agentId, widget.check.seq, dismiss: dismiss);
      if (mounted) _close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  /// A phone's or a touch iPad's keyboard: what was typed in the field
  /// goes to the page as text, a letter taken out as Backspace.
  void _typedChanged(String now) {
    final view = _view;
    final before = _typed;
    if (view == null || now == before) return;
    _typed = now;
    var same = 0;
    while (same < before.length &&
        same < now.length &&
        before.codeUnitAt(same) == now.codeUnitAt(same)) {
      same++;
    }
    final events = <LiveEvent>[
      for (var i = same; i < before.length; i++) ...const [
        LiveKey('Backspace', down: true),
        LiveKey('Backspace', down: false),
      ],
      if (now.length > same)
        for (final piece in liveTextPieces(now.substring(same)))
          LiveTyped(piece),
    ];
    view.input(events);
  }

  void _enter() {
    _view!.input(const [
      LiveKey('Enter', down: true),
      LiveKey('Enter', down: false),
    ]);
    _typed = '';
    _typing.clear();
  }

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (event is KeyDownEvent &&
        event.logicalKey == LogicalKeyboardKey.escape) {
      _close();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final view = _view!;
    final check = widget.check;
    final backend = AppScope.of(context);
    final name = backend.workspace.agent(check.agentId)?.name ?? 'The agent';
    final phone = isPhoneLayout(context);
    final touch =
        phone ||
        defaultTargetPlatform == TargetPlatform.iOS ||
        defaultTargetPlatform == TargetPlatform.android;
    final heading = Text.rich(
      TextSpan(
        children: [
          const TextSpan(text: 'Solve a check on '),
          TextSpan(
            text: check.site,
            style: const TextStyle(fontWeight: FontWeight.w600),
          ),
        ],
      ),
      style: (phone ? PhoneType.emphasis : type.title).copyWith(
        color: palette.text,
      ),
    );
    final cant = Button(
      label: "Can't do this",
      onPressed: _busy ? null : () => _answer(dismiss: true),
    );
    final done = Button(
      label: 'Done',
      primary: true,
      onPressed: _busy ? null : () => _answer(dismiss: false),
    );
    // What the card said ("Yukako waits…") isn't said again here: only
    // an answer that failed.
    final error = _error;
    final line = error == null
        ? null
        : Text(error, style: type.caption.copyWith(color: palette.danger));
    // A phone's page reads from the top, the window's tab sits in the
    // middle of it.
    final alignment = phone ? Alignment.topCenter : Alignment.center;
    // A picture of the page: VoiceOver and TalkBack say what it is.
    final screen = Semantics(
      container: true,
      image: true,
      label: 'The page on ${check.site}, live',
      child: _screen(context, view, name, alignment, touch),
    );
    final keyboard = touch
        ? Padding(
            padding: const EdgeInsets.fromLTRB(Space.l, Space.s, Space.l, 0),
            child: TextField(
              controller: _typing,
              placeholder: 'Type into the page',
              autocorrect: false,
              enableSuggestions: false,
              smartPunctuation: false,
              onSubmitted: (_) => _enter(),
            ),
          )
        : null;
    if (phone) {
      return ColoredBox(
        color: palette.canvas,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            PhoneHeader(
              leading: HeaderButton.back(onTap: _close),
              title: heading,
            ),
            Padding(
              padding: const EdgeInsets.symmetric(
                horizontal: Space.l,
                vertical: Space.s,
              ),
              child: TabSiteLine(name: name, view: view),
            ),
            Expanded(child: screen),
            ?keyboard,
            Padding(
              padding: const EdgeInsets.all(Space.l),
              child: CardActions(line: line, quiet: [cant], primary: done),
            ),
          ],
        ),
      );
    }
    return ValueListenableBuilder(
      valueListenable: windowControls,
      builder: (context, controls, _) => _window(
        context,
        controls,
        heading,
        cant,
        done,
        line,
        screen,
        keyboard,
      ),
    );
  }

  /// The tab, live, or why it isn't; with Claktory's note over it.
  Widget _screen(
    BuildContext context,
    AgentTabView view,
    String name,
    Alignment alignment,
    bool touch,
  ) {
    final palette = Palette.of(context);
    final check = widget.check;
    return ListenableBuilder(
      listenable: Listenable.merge([view, view.frames]),
      builder: (context, _) {
        final left =
            view.ended?.reason == AgentTabViewEnd.AGENT_TAB_VIEW_END_LEFT_SITE;
        if (left) {
          return Stack(
            fit: StackFit.expand,
            children: [
              ColoredBox(
                color: palette.sidebar,
                child: LiveScreen(
                  frames: view.frames,
                  acting: false,
                  onInput: (_) {},
                  maxScale: 1 / MediaQuery.devicePixelRatioOf(context),
                  alignment: alignment,
                ),
              ),
              ColoredBox(color: palette.canvas.withValues(alpha: 0.85)),
              Center(
                child: Padding(
                  padding: const EdgeInsets.all(Space.l),
                  child: _Plate(
                    view.ended!.message.isNotEmpty
                        ? view.ended!.message
                        : "The page left ${check.site}, so you can't use "
                              'it here any more.',
                  ),
                ),
              ),
            ],
          );
        }
        final note = view.note;
        final shown =
            _viewNote(view, name) ??
            ColoredBox(
              color: palette.sidebar,
              child: LiveScreen(
                frames: view.frames,
                acting: true,
                onInput: view.input,
                mac:
                    defaultTargetPlatform == TargetPlatform.macOS ||
                    defaultTargetPlatform == TargetPlatform.iOS,
                // One frame pixel to a screen pixel where it fits.
                maxScale: 1 / MediaQuery.devicePixelRatioOf(context),
                alignment: alignment,
                zoom: touch,
                focusNode: _focus,
              ),
            );
        return Stack(
          fit: StackFit.expand,
          children: [
            shown,
            // Why that click or those keys did nothing, for a moment; the
            // page under it still takes the next.
            if (note != null)
              Positioned(
                top: Space.m,
                left: Space.l,
                right: Space.l,
                child: IgnorePointer(
                  child: Center(child: _Plate(note, liveRegion: true)),
                ),
              ),
          ],
        );
      },
    );
  }

  /// A Mac's or an iPad's: filling the window as an expanded pane does,
  /// its band below an iPad's status bar and beside the window's controls.
  Widget _window(
    BuildContext context,
    WindowControls controls,
    Widget heading,
    Widget cant,
    Widget done,
    Widget? line,
    Widget screen,
    Widget? keyboard,
  ) {
    final palette = Palette.of(context);
    final view = _view!;
    final name =
        AppScope.of(context).workspace.agent(widget.check.agentId)?.name ??
        'The agent';
    // The traffic lights don't zoom with the app.
    final zoom = ZoomScope.of(context)?.level ?? 1;
    final lights = defaultTargetPlatform == TargetPlatform.macOS
        ? 80.0 / zoom
        : Space.m + controls.leading;
    return Focus(
      onKeyEvent: _key,
      child: ColoredBox(
        color: palette.canvas,
        child: Padding(
          // Above the on-screen keyboard, so Type into the page stays in
          // sight.
          padding: EdgeInsets.only(
            top: math.max(0, MediaQuery.paddingOf(context).top - controls.top),
            bottom: keyboardOverWindow(context),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              WindowDrag(
                child: Container(
                  constraints: const BoxConstraints(minHeight: titleBarHeight),
                  padding: EdgeInsets.only(
                    left: lights,
                    right: Space.m,
                    top: Space.xs,
                    bottom: Space.xs,
                  ),
                  decoration: BoxDecoration(
                    color: palette.sidebar,
                    border: Border(bottom: BorderSide(color: palette.hairline)),
                  ),
                  child: Row(
                    children: [
                      Flexible(
                        child: BandChip(
                          label: widget.back,
                          onTap: _close,
                          leading: (color) => ExtensionGlyph(
                            icon: 'arrow-left',
                            name: 'Back',
                            color: color,
                            size: 14,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
              Container(
                padding: const EdgeInsets.symmetric(
                  horizontal: Space.xl,
                  vertical: Space.m,
                ),
                decoration: BoxDecoration(
                  border: Border(bottom: BorderSide(color: palette.hairline)),
                ),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          heading,
                          const SizedBox(height: Space.xs),
                          TabSiteLine(name: name, view: view),
                          if (line != null) ...[
                            const SizedBox(height: Space.xs),
                            line,
                          ],
                        ],
                      ),
                    ),
                    const SizedBox(width: Space.m),
                    cant,
                    const SizedBox(width: Space.s),
                    done,
                  ],
                ),
              ),
              Expanded(child: screen),
              if (keyboard != null) ...[
                keyboard,
                const SizedBox(height: Space.m),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
