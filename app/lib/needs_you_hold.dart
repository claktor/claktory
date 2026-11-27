// Needs you's list holds still under the finger (#599, row 688; Lucy's
// needs-you-cards.md §4.5): a request coming in above the rows in view goes
// in with the scroll offset grown by its height, in the same layout, so
// nothing the person is about to tap moves; at the very top too. What came
// in above and isn't seen yet is said by a pill at the top ("↑ 1 new"),
// which scrolls up to it.
import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart' show SchedulerBinding;
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'identity.dart';
import 'mobile/haptics.dart';
import 'mobile/metrics.dart';
import 'mobile/platform_policy.dart';

/// [child], a list's rows in one box, as a sliver that keeps the row at the
/// top of the view where it is when what's above it changes height: the
/// offset moves by as much before anything is painted. Where the rows
/// can't scroll that far (a short list at its top), it makes the room
/// below, for as long as the offset needs it. [rows] gives each row's box
/// by its key, to tell which one is at the top.
class HeldBox extends SingleChildRenderObjectWidget {
  const HeldBox({
    super.key,
    required this.rows,
    this.movedUp,
    this.onPushedAbove,
    super.child,
  });

  final Map<String, RenderBox> Function() rows;

  /// Rows that moved up since the last build (their card changed or was
  /// replied on, so they're newer now): held for as one that came in
  /// (Squalo's R1).
  final Set<String> Function()? movedUp;

  /// Rows that showed and went above the top so those under a newcomer
  /// could stay (an urgent alert over it, say): told after the frame, for
  /// the pill to count them (Lucy 50118).
  final void Function(List<String> keys)? onPushedAbove;

  @override
  RenderHeldBox createRenderObject(BuildContext context) =>
      RenderHeldBox(rows: rows, movedUp: movedUp, onPushedAbove: onPushedAbove);

  @override
  void updateRenderObject(BuildContext context, RenderHeldBox renderObject) {
    renderObject
      ..rows = rows
      ..movedUp = movedUp
      ..onPushedAbove = onPushedAbove;
  }
}

class RenderHeldBox extends RenderSliverSingleBoxAdapter {
  RenderHeldBox({required this.rows, this.movedUp, this.onPushedAbove});

  Map<String, RenderBox> Function() rows;
  Set<String> Function()? movedUp;
  void Function(List<String> keys)? onPushedAbove;

  /// The rows in view as last laid out, top first, and where each was in
  /// the box.
  var _inView = const <(String, double)>[];

  /// Room below the rows, so the offset that holds them can be kept.
  double _room = 0;

  /// The rows as last laid out.
  var _known = const <String>{};

  bool _inside(RenderObject box) {
    for (RenderObject? at = box; at != null; at = at.parent) {
      if (identical(at, child)) return true;
    }
    return false;
  }

  double _top(RenderBox box) => box.getTransformTo(child).getTranslation().y;

  /// How far the offset moves so the rows in view stay where they were,
  /// each row being at [now] in the box: 0 when nothing needs holding.
  double _correction(Map<String, double> now, double offset) {
    if (_inView.isEmpty) return 0;
    // A row that came in, or moved up from further down, between [from]
    // and [to].
    final moved = movedUp?.call() ?? const <String>{};
    bool cameIn(double to, {double from = double.negativeInfinity}) =>
        now.entries.any(
          (row) =>
              (!_known.contains(row.key) || moved.contains(row.key)) &&
              row.value > from &&
              row.value < to,
        );

    // What's above the top row grew or shrank: the offset follows it. At
    // the very top only a row that came in above moves the offset: the
    // rows settling as they load (taking their order once each one's time
    // is known) leave the top where it is.
    final (topKey, topWas) = _inView.first;
    final topAt = now[topKey];
    if (topAt != null) {
      final moved = topAt - topWas;
      if (moved.abs() > 0.5) {
        if (offset <= 0 && !cameIn(topAt)) return 0;
        // Never above the start.
        return moved < 0 ? math.max(moved, -offset) : moved;
      }
    }
    // One came in among the rows in view (under an urgent alert at the
    // top, say: Zucchero's row 715): the rows from it down stay where they
    // were, and those above it go up by as much. Only for a row that came
    // in: one growing (opened, its card loaded) pushes what's under it
    // down, as it always did, and one leaving lets the rows close up.
    var previous = topAt ?? double.negativeInfinity;
    for (final (key, was) in _inView.skip(1)) {
      final at = now[key];
      if (at == null) continue;
      final moved = at - was;
      if (moved > 0.5) {
        if (!cameIn(at, from: previous)) return 0;
        // Those over it that go above the top: the pill counts them.
        final edge = offset + moved - constraints.precedingScrollExtent;
        final pushed = [
          for (final (above, _) in _inView.takeWhile((row) => row.$1 != key))
            if ((now[above] ?? edge) < edge - 1) above,
        ];
        if (pushed.isNotEmpty && onPushedAbove != null) {
          final tell = onPushedAbove!;
          SchedulerBinding.instance.addPostFrameCallback((_) => tell(pushed));
        }
        return moved;
      }
      previous = at;
    }
    return 0;
  }

  @override
  void performLayout() {
    final child = this.child;
    if (child == null) {
      geometry = SliverGeometry.zero;
      return;
    }
    final constraints = this.constraints;
    child.layout(constraints.asBoxConstraints(), parentUsesSize: true);
    final extent = constraints.axis == Axis.vertical
        ? child.size.height
        : child.size.width;
    final boxes = rows();
    final offset = constraints.scrollOffset;

    final now = <String, double>{
      for (final MapEntry(:key, value: box) in boxes.entries)
        if (box.attached && box.hasSize && _inside(box)) key: _top(box),
    };
    final correction = _correction(now, offset);
    if (correction.abs() > 0.5) {
      // Laid out again at the offset that holds them, where these are
      // the rows in view as they were.
      _inView = [
        for (final (key, _) in _inView)
          if (now[key] case final at?) (key, at),
      ];
      _room = _roomFor(offset + correction, extent);
      geometry = SliverGeometry(scrollOffsetCorrection: correction);
      return;
    }

    _room = offset <= 0 ? 0 : _roomFor(offset, extent);
    final total = extent + _room;
    final painted = calculatePaintOffset(constraints, from: 0, to: total);
    geometry = SliverGeometry(
      scrollExtent: total,
      paintExtent: painted,
      cacheExtent: calculateCacheOffset(constraints, from: 0, to: total),
      maxPaintExtent: total,
      hitTestExtent: painted,
      hasVisualOverflow: total > constraints.remainingPaintExtent || offset > 0,
    );
    setChildParentData(child, constraints, geometry!);

    // The rows in view now, each from its top to where the next starts
    // (a row's own size can't be read here: only its parent may, in
    // layout).
    final tops = [
      for (final MapEntry(:key, :value) in now.entries) (key, value),
    ]..sort((a, b) => a.$2.compareTo(b.$2));
    final end = offset + constraints.remainingPaintExtent;
    _inView = [
      for (var i = 0; i < tops.length; i++)
        if ((i + 1 < tops.length ? tops[i + 1].$2 : extent) > offset + 0.5 &&
            tops[i].$2 < end)
          tops[i],
    ];
    _known = now.keys.toSet();
  }

  /// The room past the rows that lets the view sit at [offset].
  double _roomFor(double offset, double extent) {
    final room =
        offset +
        constraints.viewportMainAxisExtent -
        constraints.precedingScrollExtent -
        extent;
    return room > 0 ? room : 0;
  }
}

/// What the pill says of what came in above: "↑ 1 new", "↑ 3 new"; an
/// urgent alert among them, only those: "↑ 1 urgent" (Trish 48801).
String newAboveWords({required int count, required int urgent}) =>
    urgent > 0 ? '↑ $urgent urgent' : '↑ $count new';

/// What VoiceOver says of it, once: "1 new request above", "3 new
/// requests above", "1 new alert above", "2 new above" when they're mixed,
/// "1 urgent alert above".
String newAboveSpoken({
  required int requests,
  required int alerts,
  required int urgent,
}) {
  String many(int n, String one) => '$n $one${n == 1 ? '' : 's'} above';
  if (urgent > 0) return many(urgent, 'urgent alert');
  if (alerts == 0) return many(requests, 'new request');
  if (requests == 0) return many(alerts, 'new alert');
  return '${requests + alerts} new above';
}

/// The pill at the top of the list while something new is above what
/// shows: a tap scrolls up to it. Said once to VoiceOver as it comes or
/// grows.
class NewAbovePill extends StatefulWidget {
  const NewAbovePill({
    super.key,
    required this.requests,
    required this.alerts,
    required this.urgent,
    required this.onTap,
  });

  final int requests;
  final int alerts;
  final int urgent;
  final VoidCallback onTap;

  @override
  State<NewAbovePill> createState() => _NewAbovePillState();
}

class _NewAbovePillState extends State<NewAbovePill> {
  var _said = '';

  /// What it last said, kept as it fades once nothing is above.
  var _words = '';

  int get _count => widget.requests + widget.alerts;

  @override
  void initState() {
    super.initState();
    _say();
  }

  @override
  void didUpdateWidget(NewAbovePill old) {
    super.didUpdateWidget(old);
    final grew =
        widget.requests + widget.alerts > old.requests + old.alerts ||
        widget.urgent > old.urgent;
    if (grew) _say();
  }

  void _say() {
    if (_count == 0) return;
    final words = newAboveSpoken(
      requests: widget.requests,
      alerts: widget.alerts,
      urgent: widget.urgent,
    );
    if (words == _said) return;
    _said = words;
    final view = View.maybeOf(context);
    if (view == null) return;
    unawaited(
      SemanticsService.sendAnnouncement(view, words, TextDirection.ltr),
    );
  }

  @override
  Widget build(BuildContext context) {
    final shown = _count > 0;
    if (shown) {
      _words = newAboveWords(count: _count, urgent: widget.urgent);
    }
    final palette = Palette.of(context);
    final urgent = widget.urgent > 0;
    final phone = isPhoneLayout(context);
    final target = phone
        ? PhonePlatformPolicy.of(context).touchTarget
        : Metrics.of(context).target;
    final color = urgent ? palette.danger : palette.text;
    return IgnorePointer(
      ignoring: !shown,
      child: AnimatedOpacity(
        opacity: shown ? 1 : 0,
        duration: Motion.fast,
        child: ExcludeSemantics(
          excluding: !shown,
          child: Semantics(
            container: true,
            button: true,
            label: newAboveSpoken(
              requests: widget.requests,
              alerts: widget.alerts,
              urgent: widget.urgent,
            ),
            onTap: widget.onTap,
            excludeSemantics: true,
            child: Pressable(
              key: const ValueKey('needs-you-new-above'),
              onTap: () {
                if (phone) unawaited(Haptics.selection());
                widget.onTap();
              },
              // A floating control: its words grow only so far, so it
              // never covers the rows it leads to.
              builder: (context, pressed) => MediaQuery.withClampedTextScaling(
                maxScaleFactor: 1.5,
                child: Container(
                  constraints: BoxConstraints(minHeight: target),
                  padding: const EdgeInsets.symmetric(horizontal: Space.l),
                  decoration: BoxDecoration(
                    color: pressed ? palette.selected : palette.sidebar,
                    borderRadius: BorderRadius.circular(target / 2),
                    border: Border.all(
                      color: urgent ? palette.danger : palette.hairline,
                    ),
                    boxShadow: const [
                      BoxShadow(
                        color: Color(0x40000000),
                        blurRadius: 12,
                        offset: Offset(0, 4),
                      ),
                    ],
                  ),
                  child: Center(
                    widthFactor: 1,
                    child: Text(
                      _words,
                      style:
                          (phone
                                  ? TypeScale.of(context).label
                                  : TextStyles.label)
                              .copyWith(
                                color: color,
                                fontWeight: FontWeight.w500,
                                fontFeatures: const [
                                  FontFeature.tabularFigures(),
                                ],
                              ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
