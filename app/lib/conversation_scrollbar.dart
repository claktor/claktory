import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

import 'identity.dart';

/// The heights of a conversation's items as they've been laid out, kept
/// after they scroll away, so the conversation's length can be told from
/// what was seen rather than from what's on screen now.
///
/// Why: a lazily built list knows only the items laid out around the view,
/// and Flutter's estimate of its length is their average height times the
/// count. A conversation's items range from a 24px tool call to a reply
/// thousands of pixels tall, so each scroll changes the average, the
/// estimate swings by tens of thousands of pixels, and the scrollbar the
/// estimate drives jumps up and down and changes size as you scroll.
/// Heights remembered once seen change only when an item does.
class ItemHeights {
  final _heights = <Object, double>{};
  final _attached = <Object, RenderBox>{};
  var _sum = 0.0;

  /// Something was laid out anew; the scrollbar looks again after the frame.
  final changed = ValueNotifier(0);
  var _scheduled = false;

  double? operator [](Object id) => _heights[id];

  /// The average height of the items seen; 0 before any.
  double get average => _heights.isEmpty ? 0 : _sum / _heights.length;

  void _record(Object id, double height, RenderBox box) {
    final old = _heights[id];
    _attached[id] = box;
    if (old == height) return;
    _sum += height - (old ?? 0);
    _heights[id] = height;
    if (_scheduled) return;
    _scheduled = true;
    SchedulerBinding.instance.addPostFrameCallback((_) {
      _scheduled = false;
      changed.value++;
    });
  }

  void _detach(Object id, RenderBox box) {
    if (_attached[id] == box) _attached.remove(id);
  }

  /// The items laid out now, by id.
  Map<Object, RenderBox> get attached => _attached;

  void dispose() => changed.dispose();
}

/// One item of a conversation, its height kept in [heights] under [id].
class MeasuredItem extends SingleChildRenderObjectWidget {
  const MeasuredItem({
    super.key,
    required this.id,
    required this.heights,
    super.child,
  });

  final Object id;
  final ItemHeights heights;

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderMeasured(id, heights);

  @override
  void updateRenderObject(
    BuildContext context,
    covariant RenderObject renderObject,
  ) {
    if (renderObject is! _RenderMeasured) return;
    if (renderObject.id != id) {
      renderObject.heights._detach(renderObject.id, renderObject);
    }
    renderObject
      ..id = id
      ..heights = heights;
  }
}

class _RenderMeasured extends RenderProxyBox {
  _RenderMeasured(this.id, this.heights);

  Object id;
  ItemHeights heights;

  @override
  void performLayout() {
    super.performLayout();
    heights._record(id, size.height, this);
  }

  @override
  void detach() {
    heights._detach(id, this);
    super.detach();
  }
}

/// A conversation's scrollbar, in place of the app's own (see
/// [ItemHeights] for why): the same thin, quiet thumb, placed by the
/// heights of every item seen, the ones not seen yet at their average. Its
/// thumb never moves against the scroll: an estimate put right as items
/// are seen waits until the scroll catches up with it. At the very top and
/// end it's at the top and end. Dragging it scrolls the conversation by the
/// same share; a click in its track pages toward the click.
class ConversationScrollbar extends StatefulWidget {
  const ConversationScrollbar({
    super.key,
    required this.controller,
    required this.heights,
    required this.order,
    this.unseen,
    this.after = 0,
    required this.child,
  });

  final ScrollController controller;
  final ItemHeights heights;

  /// The items' ids in reading order, top to bottom.
  final List<Object> Function() order;

  /// How many items come before the first in [order] that aren't held yet
  /// (a long conversation's earlier pages), counted at the average height.
  final int Function()? unseen;

  /// Room after the last item (the list's bottom padding).
  final double after;

  /// The conversation's scroll view; its own scrollbar is turned off.
  final Widget child;

  @override
  State<ConversationScrollbar> createState() => ConversationScrollbarState();
}

/// Where the thumb is, for tests.
typedef Thumb = ({double top, double extent});

class ConversationScrollbarState extends State<ConversationScrollbar>
    with SingleTickerProviderStateMixin {
  static const _thickness = 6.0;
  static const _margin = 2.0;
  static const _minExtent = 32.0;

  late final _fade = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 300),
  );
  Timer? _fadeOut;
  var _hovered = false;
  var _dragging = false;

  /// The thumb's place as a share of its travel, and its length as a share
  /// of the track; null when the conversation fits.
  double? _place;
  double _length = 1;

  /// The scroll offset [_place] was last worked out at, to tell a scroll's
  /// direction.
  double? _lastPixels;

  /// The conversation's estimated length and the view's place in it, at the
  /// last look.
  double _total = 0;
  double _viewport = 0;

  @visibleForTesting
  Thumb? get thumb {
    final place = _place;
    if (place == null) return null;
    final track = _viewport - 2 * _margin;
    final extent = math.min(track, math.max(_minExtent, track * _length));
    return (top: _margin + (track - extent) * place, extent: extent);
  }

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_scrolled);
    widget.heights.changed.addListener(_look);
  }

  @override
  void didUpdateWidget(ConversationScrollbar oldWidget) {
    super.didUpdateWidget(oldWidget);
    final old = oldWidget;
    if (old.controller != widget.controller) {
      old.controller.removeListener(_scrolled);
      widget.controller.addListener(_scrolled);
    }
    if (old.heights != widget.heights) {
      old.heights.changed.removeListener(_look);
      widget.heights.changed.addListener(_look);
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(_scrolled);
    widget.heights.changed.removeListener(_look);
    _fadeOut?.cancel();
    _fade.dispose();
    super.dispose();
  }

  void _scrolled() {
    _show();
    // Once the frame has laid the items out where the scroll put them.
    if (_looking) return;
    _looking = true;
    SchedulerBinding.instance.addPostFrameCallback((_) {
      _looking = false;
      _look();
    });
  }

  var _looking = false;

  /// Shows the thumb, and fades it after a moment of stillness unless the
  /// pointer is on it or it's being dragged.
  void _show() {
    _fade.forward();
    _fadeOut?.cancel();
    if (_hovered || _dragging) return;
    _fadeOut = Timer(const Duration(milliseconds: 600), () {
      if (mounted && !_hovered && !_dragging) _fade.reverse();
    });
  }

  /// Works out the thumb from the heights seen and where the view is.
  void _look() {
    if (!mounted || !widget.controller.hasClients) return;
    final position = widget.controller.position;
    if (!position.hasContentDimensions || !position.hasViewportDimension) {
      return;
    }
    final heights = widget.heights;
    final viewportBox = context.findRenderObject();
    if (viewportBox is! RenderBox || !viewportBox.hasSize) return;
    final average = heights.average;
    final order = widget.order();
    final unseen = widget.unseen?.call() ?? 0;

    // The estimated top of each item, and the whole length.
    var top = unseen * average;
    final tops = <Object, double>{};
    for (final id in order) {
      tops[id] = top;
      top += heights[id] ?? average;
    }
    final total = top + widget.after;
    final viewport = position.viewportDimension;

    // Where the view is in it: from the item laid out across its top edge
    // (or the first below it), whose place on screen is known exactly.
    double? at;
    double? best;
    for (final MapEntry(key: id, value: box) in heights.attached.entries) {
      final estimate = tops[id];
      if (estimate == null || !box.attached || !box.hasSize) continue;
      final y = box.localToGlobal(Offset.zero, ancestor: viewportBox).dy;
      // The one whose top is nearest the view's top, from above first.
      final score = y <= 0 ? -y : 1e9 + y;
      if (best == null || score < best) {
        best = score;
        at = estimate - y;
      }
    }
    final room = total - viewport;
    double? place;
    if (at != null &&
        room > 1 &&
        position.maxScrollExtent > position.minScrollExtent) {
      place = (at / room).clamp(0.0, 1.0);
      final pixels = position.pixels;
      if (pixels >= position.maxScrollExtent - 0.5) {
        place = 1;
      } else if (pixels <= position.minScrollExtent + 0.5 && unseen == 0) {
        place = 0;
      } else if (_place case final last? when _lastPixels != null) {
        // Never against the scroll: an estimate put right as items are
        // seen waits for the scroll to catch up.
        final moved = pixels - _lastPixels!;
        if (moved == 0 || (moved < 0 ? place > last : place < last)) {
          place = last;
        }
      }
      _lastPixels = pixels;
    }
    final length = total <= 0 ? 1.0 : (viewport / total).clamp(0.0, 1.0);
    if (place == _place &&
        (length - _length).abs() < 0.001 &&
        total == _total &&
        viewport == _viewport) {
      return;
    }
    setState(() {
      _place = place;
      _length = length;
      _total = total;
      _viewport = viewport;
    });
  }

  void _dragBy(double dy) {
    final thumb = this.thumb;
    if (thumb == null || !widget.controller.hasClients) return;
    final travel = _viewport - 2 * _margin - thumb.extent;
    if (travel <= 0) return;
    final position = widget.controller.position;
    final by = dy * (_total - _viewport) / travel;
    position.jumpTo(
      (position.pixels + by).clamp(
        position.minScrollExtent,
        position.maxScrollExtent,
      ),
    );
  }

  /// A click in the track, off the thumb: a page toward it.
  void _page(double y) {
    final thumb = this.thumb;
    if (thumb == null || !widget.controller.hasClients) return;
    if (y >= thumb.top && y <= thumb.top + thumb.extent) return;
    final position = widget.controller.position;
    final page = position.viewportDimension * 0.8;
    unawaited(
      position.animateTo(
        (position.pixels + (y < thumb.top ? -page : page)).clamp(
          position.minScrollExtent,
          position.maxScrollExtent,
        ),
        duration: Motion.base,
        curve: Curves.easeOut,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final thumb = this.thumb;
    final color = Palette.of(context).textFaint.withValues(alpha: 0.5);
    return Stack(
      children: [
        ScrollConfiguration(
          behavior: ScrollConfiguration.of(context).copyWith(scrollbars: false),
          child: widget.child,
        ),
        if (thumb != null && _length < 1)
          Positioned(
            top: 0,
            bottom: 0,
            right: 0,
            width: _thickness + 2 * _margin,
            child: MouseRegion(
              onEnter: (_) {
                _hovered = true;
                _show();
              },
              onExit: (_) {
                _hovered = false;
                _show();
              },
              child: FadeTransition(
                opacity: _fade,
                child: GestureDetector(
                  behavior: HitTestBehavior.translucent,
                  onTapUp: (details) => _page(details.localPosition.dy),
                  onVerticalDragStart: (_) {
                    _dragging = true;
                    _show();
                  },
                  onVerticalDragUpdate: (details) => _dragBy(details.delta.dy),
                  onVerticalDragEnd: (_) {
                    _dragging = false;
                    _show();
                  },
                  onVerticalDragCancel: () {
                    _dragging = false;
                    _show();
                  },
                  child: CustomPaint(
                    painter: _ThumbPainter(
                      top: thumb.top,
                      extent: thumb.extent,
                      color: color,
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

class _ThumbPainter extends CustomPainter {
  _ThumbPainter({required this.top, required this.extent, required this.color});

  final double top;
  final double extent;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRRect(
      RRect.fromRectAndRadius(
        Rect.fromLTWH(
          ConversationScrollbarState._margin,
          top,
          ConversationScrollbarState._thickness,
          extent,
        ),
        const Radius.circular(3),
      ),
      Paint()..color = color,
    );
  }

  @override
  bool shouldRepaint(_ThumbPainter old) =>
      old.top != top || old.extent != extent || old.color != color;
}
