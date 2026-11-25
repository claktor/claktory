// A viewer's page on a phone (Bruno's v0.1.22 task, Matt: a landscape
// video showed less video than portrait): the content first, as Photos and
// the system's players have it.
//
// - A picture or a video ([PhoneViewerKind.media]) fills the screen, edge
//   to edge. Its header and its controls lie over it, inside the safe
//   areas, and a tap on it hides or shows them; while a video plays they
//   fade by themselves. Pulled down, it closes (mobile/swipe_close.dart).
// - A document ([PhoneViewerKind.document]: text, a PDF) keeps its header
//   above it, which slides away as it's read down and comes back as it's
//   scrolled up or reaches its start.
//
// Every utility stays one tap away: the header's comments and More (Share,
// Show original, Reload, Close everywhere), the bar under the content. With
// VoiceOver or TalkBack on, nothing hides.
import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../identity.dart';
import '../pane_chrome.dart';
import '../review_answer.dart' show ReviewAnswering;
import 'swipe_close.dart';

/// The bars over a picture or video, for tests: its header's and its
/// foot's.
const viewerTopBar = ValueKey('viewer-top-bar');
const viewerFootBar = ValueKey('viewer-foot-bar');

/// How a viewer's page lays out on a phone ([PhoneViewerScaffold]).
enum PhoneViewerKind {
  /// Read by scrolling: text, a PDF. Its header slides away while read.
  document,

  /// Looked at: a picture. Edge to edge, its chrome over it, on the
  /// canvas with it and black without, as in Photos.
  media,

  /// Watched: a video. As [media], on black, as the system's player.
  video,
}

/// Whether a viewer's chrome (its header, its bars) shows: a tap on the
/// content toggles it, a playing video fades it, and what needs it (a
/// comment being placed, a sheet open, VoiceOver) holds it.
class ViewerChrome extends ChangeNotifier {
  ViewerChrome({this.fadeAfter = const Duration(seconds: 3)});

  /// How long it stays after the last touch while it fades by itself.
  final Duration fadeAfter;

  var _shown = true;
  final _holds = <Object>{};
  final _still = <Object>{};

  /// Whether a pull down may close the page (a picture or video): not
  /// while zoomed in, placing a comment or answering.
  bool get canPull => _still.isEmpty;

  /// How far the page is pulled down to close (SwipeToClose's progress):
  /// the chrome fades with it.
  final pulled = ValueNotifier<double>(0);
  var _fading = false;
  Timer? _fade;

  /// Whether it shows now.
  bool get shown => _shown || _holds.isNotEmpty;

  /// A tap on the content: hidden, it shows; shown, it hides.
  void toggle() => shown ? hide() : show();

  void show() {
    _set(true);
    _restartFade();
  }

  void hide() {
    if (_holds.isNotEmpty) return;
    _fade?.cancel();
    _set(false);
  }

  /// Keeps a pull down from closing the page while [on], for [reason]
  /// (zoomed in, placing a comment).
  void keepStill(Object reason, bool on) {
    final was = canPull;
    if (on) {
      _still.add(reason);
    } else {
      _still.remove(reason);
    }
    if (was != canPull) notifyListeners();
  }

  /// Keeps it shown while [on], for [reason] (placing a comment, VoiceOver).
  void hold(Object reason, bool on) {
    final was = shown;
    if (on) {
      _holds.add(reason);
    } else {
      _holds.remove(reason);
    }
    if (was != shown) notifyListeners();
    if (!on) _restartFade();
  }

  /// While [on] (a video playing), it fades [fadeAfter] after the last
  /// touch, as the system's players' controls do.
  set fading(bool on) {
    if (on == _fading) return;
    _fading = on;
    if (on) {
      _restartFade();
    } else {
      _fade?.cancel();
    }
  }

  /// The content or a control was touched: the fade starts over.
  void touched() => _restartFade();

  void _restartFade() {
    _fade?.cancel();
    if (!_fading || !_shown) return;
    _fade = Timer(fadeAfter, hide);
  }

  void _set(bool value) {
    if (value == _shown) return;
    _shown = value;
    notifyListeners();
  }

  @override
  void dispose() {
    _fade?.cancel();
    pulled.dispose();
    super.dispose();
  }

  /// The page's, for what's in it; null outside a phone's viewer page.
  static ViewerChrome? maybeOf(BuildContext context) => context
      .dependOnInheritedWidgetOfExactType<_ViewerChromeScope>()
      ?.notifier;
}

class _ViewerChromeScope extends InheritedNotifier<ViewerChrome> {
  const _ViewerChromeScope({required super.notifier, required super.child});
}

/// While [when], from inside a viewer's content: keeps the page's chrome
/// shown ([chrome]: placing a comment, a moment picked) and a pull down
/// from closing it ([still]: zoomed in, placing a comment).
class HoldViewerChrome extends StatefulWidget {
  const HoldViewerChrome({
    super.key,
    required this.when,
    this.chrome = true,
    this.still = true,
    required this.child,
  });

  final bool when;
  final bool chrome;
  final bool still;
  final Widget child;

  @override
  State<HoldViewerChrome> createState() => _HoldViewerChromeState();
}

class _HoldViewerChromeState extends State<HoldViewerChrome> {
  ViewerChrome? _chrome;

  void _sync() {
    final chrome = ViewerChrome.maybeOf(context);
    if (!identical(chrome, _chrome)) {
      _release();
      _chrome = chrome;
    }
    // After this frame: holding notifies, which rebuilds the page.
    final holding = widget.when;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _chrome?.hold(this, holding && widget.chrome);
      _chrome?.keepStill(this, holding && widget.still);
    });
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _sync();
  }

  @override
  void didUpdateWidget(HoldViewerChrome old) {
    super.didUpdateWidget(old);
    if (old.when != widget.when) _sync();
  }

  void _release() {
    _chrome?.hold(this, false);
    _chrome?.keepStill(this, false);
  }

  @override
  void dispose() {
    _release();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

/// Lets the page's chrome fade by itself while [when] (a video playing).
class FadeViewerChrome extends StatefulWidget {
  const FadeViewerChrome({super.key, required this.when, required this.child});

  final bool when;
  final Widget child;

  @override
  State<FadeViewerChrome> createState() => _FadeViewerChromeState();
}

class _FadeViewerChromeState extends State<FadeViewerChrome> {
  ViewerChrome? _chrome;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final chrome = ViewerChrome.maybeOf(context);
    if (!identical(chrome, _chrome)) _chrome?.fading = false;
    _chrome = chrome;
    _chrome?.fading = widget.when;
  }

  @override
  void didUpdateWidget(FadeViewerChrome old) {
    super.didUpdateWidget(old);
    _chrome?.fading = widget.when;
  }

  @override
  void dispose() {
    _chrome?.fading = false;
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

/// A viewer's page on a phone: [PaneScaffold]'s phone layout. [header] is
/// the page's bar (Back, the file's name over its details, comments,
/// More); [overlay] what lies over a picture or video's foot (a video's
/// controls); [toolbar] the bar under it (Comment).
class PhoneViewerScaffold extends StatefulWidget {
  const PhoneViewerScaffold({
    super.key,
    required this.kind,
    required this.header,
    required this.body,
    this.overlay,
    this.toolbar,
    this.onClose,
  });

  final PhoneViewerKind kind;
  final PhonePaneHeader header;
  final Widget body;
  final Widget? overlay;
  final Widget? toolbar;

  /// Back, for pulling a picture or video down to close it.
  final VoidCallback? onClose;

  @override
  State<PhoneViewerScaffold> createState() => _PhoneViewerScaffoldState();
}

class _PhoneViewerScaffoldState extends State<PhoneViewerScaffold> {
  final _chrome = ViewerChrome();

  /// The screen reader on: the chrome never hides.
  static const _reader = #reader;

  /// Answering an agent's review of the file: a pull doesn't close it.
  static const _review = #review;

  /// The system's bars hidden with the chrome (a picture or video only).
  var _systemHidden = false;

  @override
  void initState() {
    super.initState();
    _chrome.addListener(_changed);
    if (widget.kind != PhoneViewerKind.document) {
      // Android: a swipe from the edge brings its bars back (the first
      // swipe only shows them, as the system has it); the chrome comes
      // with them, so Back is there for the next.
      unawaited(
        SystemChrome.setSystemUIChangeCallback((visible) async {
          if (visible && mounted && _systemHidden) _chrome.show();
        }),
      );
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _chrome.hold(_reader, MediaQuery.accessibleNavigationOf(context));
    _chrome.keepStill(_review, ReviewAnswering.of(context));
  }

  void _changed() {
    if (!mounted) return;
    setState(() {});
    _syncSystemBars();
  }

  /// With the chrome gone from a picture or video, the status bar (and
  /// Android's navigation bar) go too, as in Photos; back with it.
  void _syncSystemBars() {
    final hide = widget.kind != PhoneViewerKind.document && !_chrome.shown;
    if (hide == _systemHidden) return;
    _systemHidden = hide;
    unawaited(
      SystemChrome.setEnabledSystemUIMode(
        hide ? SystemUiMode.immersive : SystemUiMode.edgeToEdge,
      ),
    );
  }

  @override
  void dispose() {
    _chrome.removeListener(_changed);
    if (widget.kind != PhoneViewerKind.document) {
      unawaited(SystemChrome.setSystemUIChangeCallback(null));
    }
    if (_systemHidden) {
      unawaited(SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge));
    }
    _chrome.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => _ViewerChromeScope(
    notifier: _chrome,
    child: widget.kind == PhoneViewerKind.document
        ? _document(context)
        : _media(context),
  );

  /// The header above, sliding away as it's read down; back on the way up
  /// or at the start.
  Widget _document(BuildContext context) {
    final shown = _chrome.shown;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ClipRect(
          child: AnimatedAlign(
            duration: Motion.base,
            curve: Curves.easeOutCubic,
            alignment: Alignment.bottomCenter,
            heightFactor: shown ? 1 : 0,
            child: widget.header,
          ),
        ),
        Expanded(
          child: NotificationListener<ScrollUpdateNotification>(
            onNotification: _scrolled,
            child: widget.body,
          ),
        ),
        ?widget.toolbar,
      ],
    );
  }

  /// Down a screen's worth past its start, a scroll down hides the header
  /// and one up shows it, as Safari's bars do; at the start it shows.
  bool _scrolled(ScrollUpdateNotification notification) {
    final metrics = notification.metrics;
    final delta = notification.scrollDelta ?? 0;
    if (metrics.axis != Axis.vertical || notification.dragDetails == null) {
      if (metrics.axis == Axis.vertical && metrics.pixels <= 0) _chrome.show();
      return false;
    }
    if (metrics.pixels <= 0) {
      _chrome.show();
    } else if (delta > 4 && metrics.pixels > 48) {
      _chrome.hide();
    } else if (delta < -4) {
      _chrome.show();
    }
    return false;
  }

  /// Edge to edge, the chrome over it: the header at the top, inside the
  /// status bar and the notch, the controls and the bar at the foot.
  /// How far down from the top a drag is the system's, where the status
  /// bar's inset says less (hidden, or turned).
  static const _systemEdge = 32.0;

  Widget _media(BuildContext context) {
    final palette = Palette.of(context);
    final shown = _chrome.shown;
    final safe = MediaQuery.paddingOf(context);
    final bar = palette.sidebar.withValues(alpha: 0.92);
    // Pulled down, it fades with the pull, gone by the line that closes.
    Widget fading(Widget child) => IgnorePointer(
      ignoring: !shown,
      child: ExcludeSemantics(
        excluding: !shown,
        child: AnimatedOpacity(
          duration: Motion.base,
          curve: Curves.easeOut,
          opacity: shown ? 1 : 0,
          child: ValueListenableBuilder(
            valueListenable: _chrome.pulled,
            builder: (context, pulled, child) => Opacity(
              opacity: (1 - pulled * 2).clamp(0.0, 1.0),
              child: child,
            ),
            child: child,
          ),
        ),
      ),
    );
    final foot = [?widget.overlay, ?widget.toolbar];
    return Stack(
      fit: StackFit.expand,
      children: [
        // A tap anywhere on it shows or hides the chrome; what's in it
        // (a comment's marker, Show original) takes its own taps first.
        Listener(
          behavior: HitTestBehavior.translucent,
          onPointerDown: (_) => _chrome.touched(),
          child: GestureDetector(
            behavior: HitTestBehavior.translucent,
            onTap: _chrome.toggle,
            child: SwipeToClose(
              enabled: widget.onClose != null && _chrome.canPull,
              onClose: widget.onClose ?? () {},
              progress: _chrome.pulled,
              // As Photos: the canvas with its chrome, black without;
              // pulled down, what's behind shows through.
              backdrop: shown && widget.kind == PhoneViewerKind.media
                  ? palette.canvas
                  : const Color(0xFF000000),
              child: MediaQuery.removePadding(
                context: context,
                removeTop: true,
                removeBottom: true,
                removeLeft: true,
                removeRight: true,
                child: widget.body,
              ),
            ),
          ),
        ),
        // The top edge is the system's: a swipe down from it brings the
        // status bar back (Android says so the first time), never a pull
        // that closes the page. A tap there still shows the chrome.
        Positioned(
          left: 0,
          right: 0,
          top: 0,
          height: math.max(MediaQuery.viewPaddingOf(context).top, _systemEdge),
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            excludeFromSemantics: true,
            onTap: _chrome.toggle,
            onVerticalDragStart: (_) {},
          ),
        ),
        Positioned(
          left: 0,
          right: 0,
          top: 0,
          child: fading(
            ColoredBox(
              key: viewerTopBar,
              color: bar,
              child: Padding(
                padding: EdgeInsets.only(
                  top: safe.top,
                  left: safe.left,
                  right: safe.right,
                ),
                child: MediaQuery.removePadding(
                  context: context,
                  removeTop: true,
                  removeLeft: true,
                  removeRight: true,
                  child: widget.header,
                ),
              ),
            ),
          ),
        ),
        if (foot.isNotEmpty)
          Positioned(
            left: 0,
            right: 0,
            bottom: 0,
            child: fading(
              Listener(
                onPointerDown: (_) => _chrome.touched(),
                child: DecoratedBox(
                  key: viewerFootBar,
                  decoration: BoxDecoration(
                    color: bar,
                    border: Border(top: BorderSide(color: palette.hairline)),
                  ),
                  child: Padding(
                    padding: EdgeInsets.only(
                      bottom: safe.bottom,
                      left: safe.left,
                      right: safe.right,
                    ),
                    child: MediaQuery.removePadding(
                      context: context,
                      removeBottom: true,
                      removeLeft: true,
                      removeRight: true,
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: foot,
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
      ],
    );
  }
}
