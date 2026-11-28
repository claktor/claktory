import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../mobile/metrics.dart';
import '../selection.dart' show SelectableArea;
import '../visible_codes.dart';
import 'catalog.dart';
import 'controller.dart';
import 'surface.dart';
import 'transport.dart';

/// An app's view, as a pane (Mac) or a screen (phone) shows it: opened
/// when it shows, closed when it goes, and whatever state it's in said in
/// plain words: opening, not running, failed (over its last good contents,
/// with Open again), closed, or a view this app can't show.
class AppView extends StatefulWidget {
  const AppView({
    super.key,
    required this.transport,
    required this.appId,
    required this.appName,
    this.view = '',
    this.params = '',
    this.chromeNamesApp = false,
  });

  final AppViewTransport transport;
  final String appId;

  /// The app's name (AppInfo.name), which the view always shows it's from.
  final String appName;
  final String view;
  final String params;

  /// Claktory's own header around the view names the app ("Calendar app",
  /// as appViewTitle words it): the view then goes without its "From"
  /// line and card. Set here, or by a [ChromeNamesApp] above it.
  final bool chromeNamesApp;

  @override
  State<AppView> createState() => _AppViewState();
}

class _AppViewState extends State<AppView> with WidgetsBindingObserver {
  late AppViewController _controller = _make();

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  /// Out of sight (a phone's app in the background, a Mac's window
  /// hidden), the view's surface closes, so the app stops its queries for
  /// it, as closing the view does; back in sight, it opens again.
  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    switch (state) {
      case AppLifecycleState.hidden || AppLifecycleState.paused:
        _controller.suspend();
      case AppLifecycleState.inactive || AppLifecycleState.resumed:
        _controller.resume();
      case AppLifecycleState.detached:
        break;
    }
  }

  AppViewController _make() => AppViewController(
    transport: widget.transport,
    appId: widget.appId,
    view: widget.view,
    params: widget.params,
  );

  @override
  void didUpdateWidget(AppView old) {
    super.didUpdateWidget(old);
    if (old.transport != widget.transport ||
        old.appId != widget.appId ||
        old.view != widget.view ||
        old.params != widget.params) {
      final suspended = _controller.suspended;
      _controller.dispose();
      _controller = _make();
      if (suspended) _controller.suspend();
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: _controller,
    builder: (context, _) {
      final surface = _controller.surface;
      if (surface == null) return _opening(context);
      return ListenableBuilder(
        listenable: surface,
        builder: (context, _) => _shown(context, surface),
      );
    },
  );

  Widget _opening(BuildContext context) {
    final failure = _controller.failure;
    if (failure != null) {
      return _Notice(
        title: "Couldn't open this view",
        body: failure,
        danger: true,
        action: ('Try again', _controller.open),
      );
    }
    return const _Notice(title: 'Opening…', live: true);
  }

  Widget _shown(BuildContext context, AppSurfaceModel surface) {
    void act(String id, Map<String, Object?> event, String scope) =>
        _controller.act(id, event, scope);
    final failure = _controller.failure;
    switch (surface.state) {
      case AppViewState.unsupported:
        return const _Notice(
          title: 'This view needs a newer Claktory',
          body: 'Update Claktory to see it.',
        );
      case AppViewState.stopped:
        return _Notice(
          title: "${_name(surface)} isn't running",
          body: surface.error.isEmpty
              ? 'It opens again by itself when the app starts.'
              : '${surface.error} It opens again by itself when the app '
                    'starts.',
          live: true,
        );
      case AppViewState.opening when !surface.hasContent:
        return const _Notice(title: 'Opening…', live: true);
      case AppViewState.failed || AppViewState.closed:
        final failed = surface.state == AppViewState.failed;
        final notice = _Notice(
          title: failed
              ? "${_name(surface)} couldn't show this"
              : '${_name(surface)} closed this view',
          body: surface.error,
          danger: failed,
          action: ('Open again', _controller.open),
        );
        if (!surface.hasContent) return notice;
        // The last good contents stay, dimmed, under why.
        return _Page(
          top: notice,
          child: Opacity(
            opacity: 0.5,
            child: _Framed(
              appName: _name(surface),
              named: _named,
              child: AppSurfaceView(
                surface: surface,
                onAction: act,
                appName: _name(surface),
                enabled: false,
              ),
            ),
          ),
        );
      case AppViewState.open || AppViewState.opening:
        return _Page(
          top: failure == null
              ? null
              : Text(
                  failure,
                  style: TypeScale.of(context).caption
                      .copyWith(color: Palette.of(context).danger),
                ),
          child: _Framed(
            appName: _name(surface),
            named: _named,
            child: AppSurfaceView(
              surface: surface,
              onAction: act,
              appName: _name(surface),
            ),
          ),
        );
    }
  }

  /// The app's name, or "This app" without one: never a title the app
  /// chose, which could pass for Claktory.
  String _name(AppSurfaceModel surface) =>
      visibleCodes(widget.appName.isNotEmpty ? widget.appName : 'This app');

  bool get _named => widget.chromeNamesApp || ChromeNamesApp.of(context);
}

/// Where Claktory's own header names the app around the views under it
/// (an expanded pane, a phone's page): it stays put, outside the app's
/// surface, so the views go without their "From" line and card.
class ChromeNamesApp extends InheritedWidget {
  const ChromeNamesApp({super.key, required super.child});

  static bool of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<ChromeNamesApp>() != null;

  @override
  bool updateShouldNotify(ChromeNamesApp oldWidget) => false;
}

/// What an app draws, always framed as the app's: "From Charts" over it,
/// inside a hairline, so nothing in it passes for Claktory's own cards. On
/// a phone the caption alone frames it: its panels sit on the page, 16pt
/// from the edge, as a dashboard's do in Grafana, not inset
/// again in a card. Under a header that names the app ([named]), neither.
class _Framed extends StatelessWidget {
  const _Framed({
    required this.appName,
    required this.named,
    required this.child,
  });

  final String appName;
  final bool named;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    if (named) return child;
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Column(
      spacing: Space.xs,
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'From $appName',
          style: type.caption.copyWith(
            color: palette.textMuted,
            fontWeight: FontWeight.w500,
          ),
        ),
        if (isPhoneLayout(context))
          child
        else
          Container(
            padding: EdgeInsets.all(touchFirst(context) ? Space.m : Space.l),
            decoration: BoxDecoration(
              border: Border.all(color: palette.hairline),
              borderRadius: BorderRadius.circular(Radii.panel),
            ),
            child: child,
          ),
      ],
    );
  }
}

/// A view's contents, scrolling, 16px in (24 on a Mac), with [top] over
/// them.
class _Page extends StatelessWidget {
  const _Page({required this.child, this.top});

  final Widget child;
  final Widget? top;

  @override
  Widget build(BuildContext context) {
    final inset = touchFirst(context) ? Space.l : Space.xl;
    return SingleChildScrollView(
      child: _EdgeTaps(
        inset: inset,
        child: Padding(
          padding: EdgeInsets.all(inset),
          child: SelectableArea(
            child: Column(
              spacing: Space.l,
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [?top, child],
            ),
          ),
        ),
      ),
    );
  }
}

/// A page's side margins [inset] wide: a tap there within half a finger's
/// target of the content that hits nothing goes to the content's nearest
/// edge, kept only where an [EdgeTapTarget] takes it (a bar's ••• at the
/// line's end). Nothing else gains the margin's taps.
class _EdgeTaps extends SingleChildRenderObjectWidget {
  const _EdgeTaps({required this.inset, required super.child});

  final double inset;

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderEdgeTaps(inset);

  @override
  void updateRenderObject(BuildContext context, _RenderEdgeTaps renderObject) {
    renderObject.inset = inset;
  }
}

class _RenderEdgeTaps extends RenderProxyBox {
  _RenderEdgeTaps(this.inset);

  double inset;

  /// How far into the margin a tap still counts: half a finger's target.
  static const _reach = 22.0;

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) {
    if (super.hitTestChildren(result, position: position)) return true;
    final child = this.child;
    if (child == null) return false;
    final x = position.dx;
    final Offset edge;
    if (x < inset && x >= inset - _reach) {
      edge = Offset(inset + 0.5, position.dy);
    } else if (x > size.width - inset && x <= size.width - inset + _reach) {
      edge = Offset(size.width - inset - 0.5, position.dy);
    } else {
      return false;
    }
    // Tried first on the side: kept only where a target that wants the
    // margin's taps takes it.
    final tried = BoxHitTestResult();
    if (!child.hitTest(tried, position: edge) ||
        !tried.path.any((entry) => entry.target is EdgeTapTarget)) {
      return false;
    }
    return child.hitTest(result, position: edge);
  }
}

/// A view's state in words, centered: a live dot while it waits, its
/// title in emphasis, why in body, and what to do.
class _Notice extends StatelessWidget {
  const _Notice({
    required this.title,
    this.body = '',
    this.danger = false,
    this.live = false,
    this.action,
  });

  final String title;
  final String body;
  final bool danger;
  final bool live;
  final (String, Future<void> Function())? action;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final action = this.action;
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(Space.xl),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 420),
          child: Column(
            spacing: Space.s,
            mainAxisSize: MainAxisSize.min,
            children: [
              if (live) const LiveDot(),
              Semantics(
                header: true,
                child: Text(
                  title,
                  textAlign: TextAlign.center,
                  style: type.emphasis.copyWith(color: palette.text),
                ),
              ),
              if (body.isNotEmpty)
                Text(
                  body,
                  textAlign: TextAlign.center,
                  style: type.body.copyWith(
                    color: danger ? palette.danger : palette.textMuted,
                  ),
                ),
              if (action != null) ...[
                const SizedBox(height: Space.s),
                Button(label: action.$1, onPressed: action.$2),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
