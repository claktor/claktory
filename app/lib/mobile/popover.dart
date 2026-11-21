import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../identity.dart';
import 'presentation.dart' show SheetOverlay;
import 'device.dart' as device;

/// Whether [context] is in an iPad's window wide enough for panes: its
/// action sheets are popovers there, pointing at what they're about, as
/// iPadOS shows them (docs/IDENTITY.md, "Popovers").
bool popoverHere(BuildContext context) =>
    defaultTargetPlatform == TargetPlatform.iOS &&
    deviceIdiom(context) == Idiom.pad &&
    !isPhoneLayout(context) &&
    touchFirst(context);

/// A popover's width: room for a row's words at the phone's type.
const popoverWidth = 320.0;

/// How far the arrow reaches out of the popover toward what it's about.
const _arrow = 10.0;

/// From the window's edges, and between the arrow's tip and the anchor.
const _margin = 8.0;

/// The least room beside its anchor a popover takes: two rows. Less, and
/// it shows in the window's middle instead.
const _leastRoom = 120.0;

/// Presents [builder] in a popover over the window (in its overlay), its
/// arrow pointing at [context]'s widget: under it when there's room, else
/// over it, else beside the middle of the window. Completes with what it's
/// closed with: by `close`, a tap anywhere outside it or Esc (null), or
/// null when it goes with the window's overlay (frozen for a move). Nothing behind it
/// dims, as iPadOS shows a popover; nothing behind it takes a tap while
/// it's up but the one that closes it.
Future<T?> presentPopover<T>(
  BuildContext context, {
  required Widget Function(BuildContext context, void Function([T?]) close)
  builder,
  Widget Function(Widget child)? wrap,
}) {
  // The window's overlay, which a window frozen for a move takes with it.
  final overlay = SheetOverlay.of(context);
  final box = context.findRenderObject() as RenderBox?;
  final overlayBox = overlay.context.findRenderObject() as RenderBox?;
  final anchor = box != null && box.hasSize && overlayBox != null
      ? MatrixUtils.transformRect(
          box.getTransformTo(overlayBox),
          Offset.zero & box.size,
        )
      : null;
  // The popover takes focus, so keys reach it rather than what's behind
  // it, except from a field while the keyboard on screen is up: taking it
  // there would drop the keyboard from under the window and move what the
  // popover points at. Once it's gone, focus goes back where it was.
  final before = FocusManager.instance.primaryFocus;
  final takesFocus =
      device.hardwareKeyboard.value ||
      before?.context?.findAncestorWidgetOfExactType<EditableText>() == null;
  final done = Completer<T?>();
  late final OverlayEntry entry;
  var removed = false;
  void close([T? value]) {
    if (done.isCompleted) return;
    done.complete(value);
    if (removed) return;
    removed = true;
    entry.remove();
    entry.dispose();
  }

  // Torn down without closing, with the overlay it was in (a window frozen
  // for a move takes everything in it): it ends as if dismissed, and the
  // entry is let go once the frame that took it is over.
  void gone() {
    if (!done.isCompleted) done.complete(null);
    if (removed) return;
    removed = true;
    scheduleMicrotask(() {
      entry.remove();
      entry.dispose();
    });
  }

  entry = OverlayEntry(
    builder: (context) {
      final popover = _Popover(
        anchor: anchor,
        takesFocus: takesFocus,
        close: close,
        gone: gone,
        child: Builder(builder: (context) => builder(context, close)),
      );
      return wrap == null ? popover : wrap(popover);
    },
  );
  overlay.insert(entry);
  return done.future;
}

class _Popover extends StatefulWidget {
  const _Popover({
    required this.anchor,
    required this.takesFocus,
    required this.close,
    required this.gone,
    required this.child,
  });

  /// What it's about, in the overlay's coordinates; null: the window's
  /// middle.
  final Rect? anchor;

  /// Whether it takes focus as it opens (see [presentPopover]).
  final bool takesFocus;
  final void Function() close;

  /// Called as it leaves the tree, however it goes.
  final void Function() gone;
  final Widget child;

  @override
  State<_Popover> createState() => _PopoverState();
}

class _PopoverState extends State<_Popover>
    with SingleTickerProviderStateMixin {
  late final _shown = AnimationController(vsync: this, duration: Motion.fast)
    ..forward();

  /// Where the arrow is, as laid out: on top (the popover under the
  /// anchor) or at the bottom, and how far along.
  final _arrowAt = ValueNotifier<({bool up, double x})?>(null);

  /// The popover's own focus, taken after its first frame when it takes
  /// focus at all (autofocus wouldn't: something behind it has focus).
  final _scope = FocusScopeNode(debugLabel: 'Popover');

  /// Open popovers, the newest last: Esc closes only that one.
  static final _open = <_PopoverState>[];

  @override
  void initState() {
    super.initState();
    _open.add(this);
    FocusManager.instance.addEarlyKeyEventHandler(_key);
    if (widget.takesFocus) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _scope.requestFocus();
      });
    }
  }

  /// Esc closes it wherever focus is (a field behind it, a shortcut's
  /// node), before anything behind it hears the key.
  KeyEventResult _key(KeyEvent event) {
    if (event.logicalKey != LogicalKeyboardKey.escape ||
        _open.isEmpty ||
        _open.last != this) {
      return KeyEventResult.ignored;
    }
    if (event is KeyDownEvent) widget.close();
    return KeyEventResult.handled;
  }

  @override
  void dispose() {
    _open.remove(this);
    FocusManager.instance.removeEarlyKeyEventHandler(_key);
    _scope.dispose();
    widget.gone();
    _shown.dispose();
    _arrowAt.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final safe = MediaQuery.paddingOf(context);
    final reduced = MediaQuery.disableAnimationsOf(context);
    final curve = CurvedAnimation(parent: _shown, curve: Curves.easeOut);
    final panel = Container(
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.floating),
        border: Border.all(color: palette.hairline),
        boxShadow: floatingShadow,
      ),
      clipBehavior: Clip.antiAlias,
      padding: const EdgeInsets.symmetric(vertical: Space.s),
      child: MediaQuery.removePadding(
        context: context,
        removeTop: true,
        removeBottom: true,
        removeLeft: true,
        removeRight: true,
        child: widget.child,
      ),
    );
    return BlockSemantics(
      child: Stack(
        fit: StackFit.expand,
        children: [
          // Outside it: a tap closes it, and does nothing else.
          Semantics(
            container: true,
            button: true,
            label: 'Close',
            onTap: widget.close,
            excludeSemantics: true,
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: widget.close,
            ),
          ),
          CustomMultiChildLayout(
            delegate: _PopoverLayout(
              anchor: widget.anchor,
              safe: safe,
              arrowAt: _arrowAt,
            ),
            children: [
              LayoutId(
                id: _Part.panel,
                child: FocusScope(
                  node: _scope,
                  child: AnimatedBuilder(
                    animation: curve,
                    builder: (context, child) => Opacity(
                      opacity: curve.value,
                      child: reduced
                          ? child
                          : Transform.scale(
                              scale: 0.96 + 0.04 * curve.value,
                              child: child,
                            ),
                    ),
                    child: SizedBox(width: popoverWidth, child: panel),
                  ),
                ),
              ),
              LayoutId(
                id: _Part.arrow,
                child: FadeTransition(
                  opacity: curve,
                  child: CustomPaint(
                    size: const Size(_arrow * 2, _arrow),
                    painter: _ArrowPainter(
                      at: _arrowAt,
                      fill: palette.sidebar,
                      edge: palette.hairline,
                    ),
                  ),
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

enum _Part { panel, arrow }

/// Lays the popover out by its anchor: under it if it fits there, else
/// over it if it fits there, else on whichever side has more room, its
/// height then held to that room; across, centered on the anchor and kept
/// inside the window.
class _PopoverLayout extends MultiChildLayoutDelegate {
  _PopoverLayout({
    required this.anchor,
    required this.safe,
    required this.arrowAt,
  });

  final Rect? anchor;
  final EdgeInsets safe;
  final ValueNotifier<({bool up, double x})?> arrowAt;

  @override
  void performLayout(Size size) {
    final top = safe.top + _margin;
    final bottom = size.height - safe.bottom - _margin;
    var anchor = this.anchor;
    // What it's about leaves too little room on either side (a long
    // message held): in the window's middle, over it.
    if (anchor != null &&
        math.max(
              size.height - safe.bottom - anchor.bottom,
              anchor.top - safe.top,
            ) <
            _leastRoom + _arrow + 2 * _margin) {
      anchor = null;
    }
    if (anchor == null) {
      final panel = layoutChild(
        _Part.panel,
        BoxConstraints.loose(Size(size.width - 2 * _margin, bottom - top)),
      );
      positionChild(
        _Part.panel,
        Offset(
          (size.width - panel.width) / 2,
          (size.height - panel.height) / 2,
        ),
      );
      layoutChild(_Part.arrow, BoxConstraints.tight(Size.zero));
      positionChild(_Part.arrow, Offset.zero);
      _tell(null);
      return;
    }
    final below = bottom - (anchor.bottom + _arrow + _margin);
    final above = (anchor.top - _arrow - _margin) - top;
    final width = math.min(popoverWidth, size.width - 2 * _margin);
    // Held to the larger side's room, which it takes when it fits neither.
    final panel = layoutChild(
      _Part.panel,
      BoxConstraints(
        maxWidth: width,
        maxHeight: math.max(0.0, math.max(below, above)),
      ),
    );
    final down =
        panel.height <= below || panel.height > above && below >= above;
    final x = (anchor.center.dx - panel.width / 2)
        .clamp(_margin, math.max(_margin, size.width - panel.width - _margin))
        .toDouble();
    final y = down
        ? anchor.bottom + _arrow + _margin
        : anchor.top - _arrow - _margin - panel.height;
    positionChild(_Part.panel, Offset(x, y));
    // The arrow at the anchor's middle, kept clear of the rounded corners.
    final tip = anchor.center.dx
        .clamp(
          x + Radii.floating + _arrow,
          math.max(
            x + Radii.floating + _arrow,
            x + panel.width - Radii.floating - _arrow,
          ),
        )
        .toDouble();
    layoutChild(
      _Part.arrow,
      BoxConstraints.tight(const Size(_arrow * 2, _arrow + 1)),
    );
    positionChild(
      _Part.arrow,
      Offset(tip - _arrow, down ? y - _arrow : y + panel.height - 1),
    );
    _tell((up: down, x: tip));
  }

  void _tell(({bool up, double x})? at) {
    if (arrowAt.value != at) arrowAt.value = at;
  }

  @override
  bool shouldRelayout(_PopoverLayout old) =>
      old.anchor != anchor || old.safe != safe;
}

/// The arrow: a triangle in the popover's color, its two outer edges in
/// its hairline, pointing up (the popover under the anchor) or down.
class _ArrowPainter extends CustomPainter {
  _ArrowPainter({required this.at, required this.fill, required this.edge})
    : super(repaint: at);

  final ValueNotifier<({bool up, double x})?> at;
  final Color fill;
  final Color edge;

  @override
  void paint(Canvas canvas, Size size) {
    final where = at.value;
    if (where == null || size.isEmpty) return;
    final w = size.width;
    final h = size.height - 1;
    final Path outline;
    final Path shape;
    if (where.up) {
      outline = Path()
        ..moveTo(0, h + 1)
        ..lineTo(w / 2, 0)
        ..lineTo(w, h + 1);
      shape = Path.from(outline)..close();
    } else {
      outline = Path()
        ..moveTo(0, 0)
        ..lineTo(w / 2, h + 1)
        ..lineTo(w, 0);
      shape = Path.from(outline)..close();
    }
    canvas
      ..drawPath(shape, Paint()..color = fill)
      ..drawPath(
        outline,
        Paint()
          ..color = edge
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1,
      );
  }

  @override
  bool shouldRepaint(_ArrowPainter old) =>
      old.fill != fill || old.edge != edge || old.at != at;
}
