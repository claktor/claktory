import 'dart:math' as math;

import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';

import '../identity.dart';
import 'drawer.dart' show largeText;

/// Something that takes the room left (words, a field) and an action at
/// its end, in a line: `Row([Expanded(lead), gap, action])`. Where both
/// can't fit side by side (an action wider than the line leaves it, as in a
/// narrow pane on a Mac, or a phone's largest text sizes), the action goes
/// under it, still at its end (under the thumb on a phone, and on the same
/// side whatever the text size), its controls together on one line while
/// the whole width holds them.
class ActionLine extends StatelessWidget {
  const ActionLine({
    super.key,
    required this.lead,
    required this.action,
    this.gap = Space.s,
  });

  final Widget lead;
  final Widget action;
  final double gap;

  @override
  Widget build(BuildContext context) {
    if (isPhoneLayout(context) && largeText(context)) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          lead,
          SizedBox(height: gap),
          Align(alignment: AlignmentDirectional.centerEnd, child: action),
        ],
      );
    }
    // A Mac's narrow pane too (four panes open): the lead kept a few words
    // wide rather than squeezed to one per line, the action under it.
    return _FittedLine(
      gap: gap,
      textDirection: Directionality.of(context),
      children: [lead, action],
    );
  }
}

/// The least the lead keeps beside the action before the action goes under
/// it: a few words.
const _leastLead = 96.0;

/// [ActionLine]'s line, but at a phone's largest text: lead and action in
/// a line when the action at its own width, the gap and [_leastLead] fit,
/// else the action under the lead at its end.
class _FittedLine extends MultiChildRenderObjectWidget {
  const _FittedLine({
    required this.gap,
    required this.textDirection,
    required super.children,
  });

  final double gap;
  final TextDirection textDirection;

  @override
  _RenderFittedLine createRenderObject(BuildContext context) =>
      _RenderFittedLine(gap: gap, textDirection: textDirection);

  @override
  void updateRenderObject(BuildContext context, _RenderFittedLine line) {
    line
      ..gap = gap
      ..textDirection = textDirection;
  }
}

class _LineData extends ContainerBoxParentData<RenderBox> {}

class _RenderFittedLine extends RenderBox
    with
        ContainerRenderObjectMixin<RenderBox, _LineData>,
        RenderBoxContainerDefaultsMixin<RenderBox, _LineData> {
  _RenderFittedLine({required this._gap, required this._textDirection});

  double _gap;
  set gap(double value) {
    if (_gap == value) return;
    _gap = value;
    markNeedsLayout();
  }

  TextDirection _textDirection;
  set textDirection(TextDirection value) {
    if (_textDirection == value) return;
    _textDirection = value;
    markNeedsLayout();
  }

  @override
  void setupParentData(RenderBox child) {
    if (child.parentData is! _LineData) child.parentData = _LineData();
  }

  RenderBox get _lead => firstChild!;
  RenderBox get _action => lastChild!;

  /// The action's width on one line ([ControlsLine]'s counts its gaps).
  double get _actionWidth => _action.getMaxIntrinsicWidth(double.infinity);

  /// Whether the action fits beside the lead in [width].
  bool _fits(double width) =>
      width.isInfinite || _actionWidth + _gap + _leastLead <= width;

  @override
  double computeMinIntrinsicWidth(double height) => [
    _lead.getMinIntrinsicWidth(height),
    _action.getMinIntrinsicWidth(height),
  ].reduce((a, b) => a > b ? a : b);

  @override
  double computeMaxIntrinsicWidth(double height) =>
      _lead.getMaxIntrinsicWidth(height) + _gap + _actionWidth;

  double _height(double width, double Function(RenderBox, double) of) {
    if (_fits(width)) {
      final action = _actionWidth;
      final lead = of(_lead, (width - _gap - action).clamp(0, width));
      final end = of(_action, action);
      return lead > end ? lead : end;
    }
    return of(_lead, width) + _gap + of(_action, width);
  }

  @override
  double computeMinIntrinsicHeight(double width) =>
      _height(width, (child, w) => child.getMinIntrinsicHeight(w));

  @override
  double computeMaxIntrinsicHeight(double width) =>
      _height(width, (child, w) => child.getMaxIntrinsicHeight(w));

  @override
  Size computeDryLayout(BoxConstraints constraints) =>
      _layout(constraints, ChildLayoutHelper.dryLayoutChild, place: false);

  @override
  void performLayout() {
    size = _layout(constraints, ChildLayoutHelper.layoutChild, place: true);
  }

  Size _layout(
    BoxConstraints constraints,
    ChildLayouter layoutChild, {
    required bool place,
  }) {
    final width = constraints.maxWidth;
    final rtl = _textDirection == TextDirection.rtl;
    if (_fits(width)) {
      // At its own width: a Mac's ControlsLine, a Row, would fill
      // whatever it's given and leave the lead only [_leastLead].
      final action = layoutChild(
        _action,
        BoxConstraints(maxWidth: _actionWidth),
      );
      final leadWidth = width - _gap - action.width;
      final lead = layoutChild(
        _lead,
        BoxConstraints.tightFor(width: leadWidth),
      );
      final height = lead.height > action.height ? lead.height : action.height;
      if (place) {
        (_lead.parentData! as _LineData).offset = Offset(
          rtl ? width - leadWidth : 0,
          (height - lead.height) / 2,
        );
        (_action.parentData! as _LineData).offset = Offset(
          rtl ? 0 : width - action.width,
          (height - action.height) / 2,
        );
      }
      return constraints.constrain(Size(width, height));
    }
    final lead = layoutChild(_lead, BoxConstraints.tightFor(width: width));
    final action = layoutChild(
      _action,
      BoxConstraints(maxWidth: math.min(_actionWidth, width)),
    );
    if (place) {
      (_lead.parentData! as _LineData).offset = Offset.zero;
      (_action.parentData! as _LineData).offset = Offset(
        rtl ? 0 : width - action.width,
        lead.height + _gap,
      );
    }
    return constraints.constrain(
      Size(width, lead.height + _gap + action.height),
    );
  }

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) =>
      defaultHitTestChildren(result, position: position);

  @override
  void paint(PaintingContext context, Offset offset) =>
      defaultPaint(context, offset);
}

/// Controls side by side, as a Mac shows them (each [gap] apart). When they
/// don't all fit on one line (a phone's large text, a Mac's narrow pane),
/// each takes the whole width, one under another in their order (the
/// primary, last, at the bottom), as iOS stacks an alert's buttons at large
/// text sizes.
class ControlsLine extends StatelessWidget {
  const ControlsLine({super.key, required this.children, this.gap = Space.s});

  final List<Widget> children;
  final double gap;

  @override
  Widget build(BuildContext context) => _StackingLine(
    gap: gap,
    runGap: Space.s,
    textDirection: Directionality.of(context),
    children: children,
  );
}

/// [ControlsLine]'s layout: its controls in a line, [gap] apart, while
/// the line holds them at their own widths, else stacked [runGap] apart,
/// each the whole width.
class _StackingLine extends MultiChildRenderObjectWidget {
  const _StackingLine({
    required this.gap,
    required this.runGap,
    required this.textDirection,
    required super.children,
  });

  final double gap;
  final double runGap;
  final TextDirection textDirection;

  @override
  _RenderStackingLine createRenderObject(BuildContext context) =>
      _RenderStackingLine(
        gap: gap,
        runGap: runGap,
        textDirection: textDirection,
      );

  @override
  void updateRenderObject(BuildContext context, _RenderStackingLine line) {
    line
      ..gap = gap
      ..runGap = runGap
      ..textDirection = textDirection;
  }
}

class _RenderStackingLine extends RenderBox
    with
        ContainerRenderObjectMixin<RenderBox, _LineData>,
        RenderBoxContainerDefaultsMixin<RenderBox, _LineData> {
  _RenderStackingLine({
    required this._gap,
    required this._runGap,
    required this._textDirection,
  });

  double _gap;
  set gap(double value) {
    if (_gap == value) return;
    _gap = value;
    markNeedsLayout();
  }

  double _runGap;
  set runGap(double value) {
    if (_runGap == value) return;
    _runGap = value;
    markNeedsLayout();
  }

  TextDirection _textDirection;
  set textDirection(TextDirection value) {
    if (_textDirection == value) return;
    _textDirection = value;
    markNeedsLayout();
  }

  @override
  void setupParentData(RenderBox child) {
    if (child.parentData is! _LineData) child.parentData = _LineData();
  }

  /// The controls at their own widths, [_gap] apart.
  double get _lineWidth {
    var width = 0.0;
    for (final child in getChildrenAsList()) {
      width += child.getMaxIntrinsicWidth(double.infinity);
    }
    return width + _gap * (childCount > 1 ? childCount - 1 : 0);
  }

  bool _fits(double width) => width.isInfinite || _lineWidth <= width;

  @override
  double computeMinIntrinsicWidth(double height) {
    var width = 0.0;
    for (final child in getChildrenAsList()) {
      final own = child.getMinIntrinsicWidth(height);
      if (own > width) width = own;
    }
    return width;
  }

  @override
  double computeMaxIntrinsicWidth(double height) => _lineWidth;

  double _height(double width, double Function(RenderBox, double) of) {
    final children = getChildrenAsList();
    if (children.isEmpty) return 0;
    if (_fits(width)) {
      var height = 0.0;
      for (final child in children) {
        final own = of(child, child.getMaxIntrinsicWidth(double.infinity));
        if (own > height) height = own;
      }
      return height;
    }
    var height = _runGap * (children.length - 1);
    for (final child in children) {
      height += of(child, width);
    }
    return height;
  }

  @override
  double computeMinIntrinsicHeight(double width) =>
      _height(width, (child, w) => child.getMinIntrinsicHeight(w));

  @override
  double computeMaxIntrinsicHeight(double width) =>
      _height(width, (child, w) => child.getMaxIntrinsicHeight(w));

  @override
  Size computeDryLayout(BoxConstraints constraints) =>
      _layout(constraints, ChildLayoutHelper.dryLayoutChild, place: false);

  @override
  void performLayout() {
    size = _layout(constraints, ChildLayoutHelper.layoutChild, place: true);
  }

  Size _layout(
    BoxConstraints constraints,
    ChildLayouter layoutChild, {
    required bool place,
  }) {
    final children = getChildrenAsList();
    if (children.isEmpty) return constraints.smallest;
    final width = constraints.maxWidth;
    if (_fits(width)) {
      final sizes = [
        for (final child in children)
          layoutChild(
            child,
            BoxConstraints(
              maxWidth: child.getMaxIntrinsicWidth(double.infinity),
            ),
          ),
      ];
      final height = sizes.fold(0.0, (a, b) => a > b.height ? a : b.height);
      final lineWidth =
          sizes.fold(0.0, (a, b) => a + b.width) + _gap * (sizes.length - 1);
      if (place) {
        final rtl = _textDirection == TextDirection.rtl;
        var x = rtl ? lineWidth : 0.0;
        for (final (i, child) in children.indexed) {
          final own = sizes[i];
          if (rtl) x -= own.width;
          (child.parentData! as _LineData).offset = Offset(
            x,
            (height - own.height) / 2,
          );
          x += rtl ? -_gap : own.width + _gap;
        }
      }
      return constraints.constrain(Size(lineWidth, height));
    }
    var y = 0.0;
    for (final (i, child) in children.indexed) {
      if (i > 0) y += _runGap;
      final own = layoutChild(child, BoxConstraints.tightFor(width: width));
      if (place) (child.parentData! as _LineData).offset = Offset(0, y);
      y += own.height;
    }
    return constraints.constrain(Size(width, y));
  }

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) =>
      defaultHitTestChildren(result, position: position);

  @override
  void paint(PaintingContext context, Offset offset) =>
      defaultPaint(context, offset);
}
