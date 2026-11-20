import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'scope.dart';
import 'settings_view.dart' show SettingRow;

/// Zooming the whole app, as a browser zooms a page: ⌘+ and ⌘− (Ctrl
/// elsewhere) step through [zoomLevels], ⌘0 goes back to actual size, and
/// the View menu and Settings → Display do the same. Text, icons, spacing
/// and panes all scale together, and the layout reflows for the space the
/// window has at that size, so a smaller zoom fits more.
///
/// Each device keeps its own zoom, since each has its own screen: in the
/// backend's `Settings.zoom` under this device's id ([Backend.zoomKey]).
/// The window shows the zoom of the Claktory it shows, for this device.

/// Chrome's zoom levels.
const zoomLevels = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// "125%".
String zoomPercent(double zoom) => '${(zoom * 100).round()}%';

/// The level after [zoom], toward bigger ([direction] 1) or smaller (−1);
/// [zoom] itself at either end.
double stepZoom(double zoom, int direction) {
  const near = 0.005;
  if (direction > 0) {
    for (final level in zoomLevels) {
      if (level > zoom + near) return level;
    }
    return zoomLevels.last;
  }
  for (final level in zoomLevels.reversed) {
    if (level < zoom - near) return level;
  }
  return zoomLevels.first;
}

/// Whether the app zooms on this device: not on iPhone, where the
/// system's text size does it.
bool get zoomsHere => defaultTargetPlatform != TargetPlatform.iOS;

/// The zoom of [backend]'s Claktory for this device, and changes to it.
/// What it shows is the backend's, except for a change on its way there.
class ZoomController extends ChangeNotifier {
  ZoomController(this.backend) {
    backend.workspace.addListener(_changed);
    _shown = level;
  }

  final Backend backend;

  /// A level asked for and not yet the backend's.
  double? _pending;

  /// The backend has answered for [_pending]: its next Workspace says how
  /// it is.
  var _answered = false;
  late double _shown;

  /// The backend's zoom for this device: 1 when it keeps none.
  double get stored {
    final zoom = backend.workspace.workspace.settings.zoom[backend.zoomKey];
    if (zoom == null || !zoom.isFinite || zoom <= 0) return 1;
    return zoom.clamp(zoomLevels.first, zoomLevels.last);
  }

  /// The zoom the window shows.
  double get level => zoomsHere ? (_pending ?? stored) : 1;

  void _changed() {
    final pending = _pending;
    if (pending != null && ((stored - pending).abs() < 0.001 || _answered)) {
      _pending = null;
      _answered = false;
    }
    _notifyIfChanged();
  }

  void _notifyIfChanged() {
    if (level == _shown) return;
    _shown = level;
    notifyListeners();
  }

  /// Zooms to [zoom] (1: actual size).
  Future<void> set(double zoom) async {
    zoom = zoom.clamp(zoomLevels.first, zoomLevels.last);
    if ((zoom - level).abs() < 0.001) return;
    _pending = zoom;
    _answered = false;
    _notifyIfChanged();
    try {
      await backend.setZoom(zoom);
      if (_pending == zoom) {
        if ((stored - zoom).abs() < 0.001) {
          _pending = null;
        } else {
          _answered = true;
        }
      }
    } on Object catch (error) {
      if (_pending == zoom) _pending = null;
      _notifyIfChanged();
      if (error is! RequestFailure) rethrow;
      debugPrint('zoom: ${error.message}');
    }
  }

  Future<void> zoomIn() => set(stepZoom(level, 1));
  Future<void> zoomOut() => set(stepZoom(level, -1));
  Future<void> actualSize() => set(1);

  @override
  void dispose() {
    backend.workspace.removeListener(_changed);
    super.dispose();
  }
}

/// The window's zoom, for what's in it (Settings → Display).
class ZoomScope extends InheritedNotifier<ZoomController> {
  const ZoomScope({
    super.key,
    required ZoomController controller,
    required super.child,
  }) : super(notifier: controller);

  static ZoomController? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<ZoomScope>()?.notifier;
}

/// Zooms [child], the whole window, by the zoom of the Claktory shown:
/// with the keys, the View menu's items (the `factory/menu` channel) and
/// [ZoomScope] for Settings.
class AppZoom extends StatefulWidget {
  const AppZoom({super.key, required this.child});

  final Widget child;

  /// The Mac's View menu (MainFlutterWindow.swift).
  static const menu = MethodChannel('factory/menu');

  /// The window's top band follows the zoom, so the traffic lights stay
  /// centered in it (MainFlutterWindow.swift).
  static const window = MethodChannel('factory/window');

  @override
  State<AppZoom> createState() => _AppZoomState();
}

class _AppZoomState extends State<AppZoom> {
  ZoomController? _zoom;

  /// The band the window has: 44 until the zoom changes it.
  double _band = titleBarHeight;

  @override
  void initState() {
    super.initState();
    AppZoom.menu.setMethodCallHandler(_menu);
  }

  Future<void> _menu(MethodCall call) async {
    final zoom = _zoom;
    if (zoom == null) return;
    switch (call.method) {
      case 'zoomIn':
        await zoom.zoomIn();
      case 'zoomOut':
        await zoom.zoomOut();
      case 'actualSize':
        await zoom.actualSize();
    }
  }

  /// The zoom of the Claktory shown: a new one when the window shows
  /// another.
  ZoomController _zoomFor(Backend backend) {
    final zoom = _zoom;
    if (zoom != null && zoom.backend == backend) return zoom;
    if (zoom != null) {
      // Once nothing listens to it any more.
      WidgetsBinding.instance.addPostFrameCallback((_) => zoom.dispose());
    }
    return _zoom = ZoomController(backend);
  }

  @override
  void dispose() {
    AppZoom.menu.setMethodCallHandler(null);
    _zoom?.dispose();
    super.dispose();
  }

  void _placeBand(double zoom) {
    final band = titleBarHeight * zoom;
    if (band == _band) return;
    _band = band;
    AppZoom.window.invokeMethod<void>('band', band).catchError((Object _) {});
  }

  @override
  Widget build(BuildContext context) {
    final zoom = _zoomFor(AppScope.of(context));
    final keys = defaultTargetPlatform == TargetPlatform.macOS
        ? (meta: true, control: false)
        : (meta: false, control: true);
    SingleActivator key(LogicalKeyboardKey key, {bool shift = false}) =>
        SingleActivator(
          key,
          meta: keys.meta,
          control: keys.control,
          shift: shift,
        );
    return ZoomScope(
      controller: zoom,
      child: CallbackShortcuts(
        bindings: zoomsHere
            ? {
                key(LogicalKeyboardKey.equal): zoom.zoomIn,
                key(LogicalKeyboardKey.equal, shift: true): zoom.zoomIn,
                key(LogicalKeyboardKey.add): zoom.zoomIn,
                key(LogicalKeyboardKey.numpadAdd): zoom.zoomIn,
                key(LogicalKeyboardKey.minus): zoom.zoomOut,
                key(LogicalKeyboardKey.numpadSubtract): zoom.zoomOut,
                key(LogicalKeyboardKey.digit0): zoom.actualSize,
                key(LogicalKeyboardKey.numpad0): zoom.actualSize,
              }
            : const {},
        child: ListenableBuilder(
          listenable: zoom,
          builder: (context, child) {
            if (zoomsHere) _placeBand(zoom.level);
            return Zoomed(zoom: zoom.level, child: child!);
          },
          child: widget.child,
        ),
      ),
    );
  }
}

/// [child] laid out at 1/[zoom] of the space it has and drawn [zoom] times
/// as big, with the screen's size and insets it reads scaled to match. The
/// tree is the same at every zoom, so nothing in it starts over on a change.
class Zoomed extends StatelessWidget {
  const Zoomed({super.key, required this.zoom, required this.child});

  final double zoom;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final media = MediaQuery.of(context);
    EdgeInsets scaled(EdgeInsets insets) => insets / zoom;
    return _ZoomBox(
      zoom: zoom,
      child: MediaQuery(
        data: media.copyWith(
          size: media.size / zoom,
          padding: scaled(media.padding),
          viewPadding: scaled(media.viewPadding),
          viewInsets: scaled(media.viewInsets),
          systemGestureInsets: scaled(media.systemGestureInsets),
          // Pictures decode for the pixels they cover.
          devicePixelRatio: media.devicePixelRatio * zoom,
        ),
        child: child,
      ),
    );
  }
}

class _ZoomBox extends SingleChildRenderObjectWidget {
  const _ZoomBox({required this.zoom, super.child});

  final double zoom;

  @override
  RenderObject createRenderObject(BuildContext context) => _RenderZoom(zoom);

  @override
  void updateRenderObject(BuildContext context, _RenderZoom renderObject) {
    renderObject.zoom = zoom;
  }
}

class _RenderZoom extends RenderProxyBox {
  _RenderZoom(this._zoom);

  double _zoom;

  set zoom(double value) {
    if (value == _zoom) return;
    _zoom = value;
    markNeedsLayout();
  }

  Matrix4 get _transform => Matrix4.diagonal3Values(_zoom, _zoom, 1);

  @override
  void performLayout() {
    final child = this.child;
    if (child == null) {
      size = constraints.smallest;
      return;
    }
    child.layout(constraints / _zoom, parentUsesSize: true);
    size = constraints.constrain(child.size * _zoom);
  }

  @override
  Size computeDryLayout(BoxConstraints constraints) {
    final child = this.child;
    if (child == null) return constraints.smallest;
    return constraints.constrain(
      child.getDryLayout(constraints / _zoom) * _zoom,
    );
  }

  @override
  double computeMinIntrinsicWidth(double height) =>
      (child?.getMinIntrinsicWidth(height / _zoom) ?? 0) * _zoom;

  @override
  double computeMaxIntrinsicWidth(double height) =>
      (child?.getMaxIntrinsicWidth(height / _zoom) ?? 0) * _zoom;

  @override
  double computeMinIntrinsicHeight(double width) =>
      (child?.getMinIntrinsicHeight(width / _zoom) ?? 0) * _zoom;

  @override
  double computeMaxIntrinsicHeight(double width) =>
      (child?.getMaxIntrinsicHeight(width / _zoom) ?? 0) * _zoom;

  @override
  void paint(PaintingContext context, Offset offset) {
    if (child == null) return;
    layer = context.pushTransform(
      needsCompositing,
      offset,
      _transform,
      super.paint,
      oldLayer: layer is TransformLayer ? layer! as TransformLayer : null,
    );
  }

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) {
    final child = this.child;
    if (child == null) return false;
    return result.addWithPaintTransform(
      transform: _transform,
      position: position,
      hitTest: (result, position) => child.hitTest(result, position: position),
    );
  }

  @override
  void applyPaintTransform(RenderBox child, Matrix4 transform) {
    transform.multiply(_transform);
  }
}

/// Settings → Display's Zoom: this device's zoom, a menu of the levels,
/// and Actual size.
class ZoomSetting extends StatelessWidget {
  const ZoomSetting({super.key});

  @override
  Widget build(BuildContext context) {
    final zoom = ZoomScope.of(context);
    if (zoom == null || !zoomsHere) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final level = zoom.level;
    final mac = defaultTargetPlatform == TargetPlatform.macOS;
    final note = mac
        ? 'Only on this Mac. Press ⌘+ or ⌘− to zoom, and ⌘0 for actual size.'
        : 'Only on this computer. Press Ctrl+ or Ctrl− to zoom, and Ctrl+0 '
              'for actual size.';
    return SettingRow(
      label: 'Zoom',
      child: Row(
        children: [
          SizedBox(
            width: 120,
            child: Select<double>(
              value: _nearest(level),
              options: [
                for (final level in zoomLevels)
                  SelectOption(level, zoomPercent(level)),
              ],
              onChanged: (level) => unawaited(zoom.set(level)),
            ),
          ),
          if ((level - 1).abs() > 0.001) ...[
            const SizedBox(width: Space.s),
            Button(
              label: 'Actual size',
              onPressed: () => unawaited(zoom.actualSize()),
            ),
          ],
          const SizedBox(width: Space.m),
          Expanded(
            child: Text(
              note,
              style: TextStyles.caption.copyWith(color: palette.textMuted),
            ),
          ),
        ],
      ),
    );
  }

  /// The level [zoom] is, or the nearest (the backend keeps any).
  static double _nearest(double zoom) =>
      zoomLevels.reduce((a, b) => (a - zoom).abs() <= (b - zoom).abs() ? a : b);
}
