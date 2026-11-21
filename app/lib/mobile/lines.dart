import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';

import '../identity.dart';
import 'drawer.dart' show largeText;

/// Something that takes the room left (words, a field) and an action at
/// its end, in a line: `Row([Expanded(lead), gap, action])`. On a phone,
/// where both can't fit side by side (the largest text sizes, or an action
/// wider than the line leaves it), the action goes under it at its start
/// instead.
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
          Align(alignment: AlignmentDirectional.centerStart, child: action),
        ],
      );
    }
    if (isPhoneLayout(context)) {
      return _FittedLine(
        gap: gap,
        textDirection: Directionality.of(context),
        children: [lead, action],
      );
    }
    return Row(
      children: [
        Expanded(child: lead),
        SizedBox(width: gap),
        action,
      ],
    );
  }
}

/// The least the lead keeps beside the action before the action goes under
/// it: a few words.
const _leastLead = 96.0;

/// [ActionLine] on a phone: lead and action in a line when the action at
/// its own width, the gap and [_leastLead] fit, else the action under the
/// lead at its start.
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
  _RenderFittedLine({
    required this._gap,
    required this._textDirection,
  });

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

  /// Whether the action fits beside the lead in [width].
  bool _fits(double width) =>
      width.isInfinite ||
      _action.getMaxIntrinsicWidth(double.infinity) + _gap + _leastLead <=
          width;

  @override
  double computeMinIntrinsicWidth(double height) => [
    _lead.getMinIntrinsicWidth(height),
    _action.getMinIntrinsicWidth(height),
  ].reduce((a, b) => a > b ? a : b);

  @override
  double computeMaxIntrinsicWidth(double height) =>
      _lead.getMaxIntrinsicWidth(height) +
      _gap +
      _action.getMaxIntrinsicWidth(height);

  double _height(double width, double Function(RenderBox, double) of) {
    if (_fits(width)) {
      final action = _action.getMaxIntrinsicWidth(double.infinity);
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
      final action = layoutChild(
        _action,
        BoxConstraints(maxWidth: width - _gap - _leastLead),
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
    final action = layoutChild(_action, BoxConstraints(maxWidth: width));
    if (place) {
      (_lead.parentData! as _LineData).offset = Offset.zero;
      (_action.parentData! as _LineData).offset = Offset(
        rtl ? width - action.width : 0,
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

/// Controls side by side, as a Mac shows them (each [gap] apart); on a
/// phone they wrap onto more lines when they don't fit.
class ControlsLine extends StatelessWidget {
  const ControlsLine({super.key, required this.children, this.gap = Space.s});

  final List<Widget> children;
  final double gap;

  @override
  Widget build(BuildContext context) {
    if (isPhoneLayout(context)) {
      return Wrap(
        spacing: gap,
        runSpacing: Space.s,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: children,
      );
    }
    return Row(
      children: [
        for (final (i, child) in children.indexed) ...[
          if (i > 0) SizedBox(width: gap),
          child,
        ],
      ],
    );
  }
}
