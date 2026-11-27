import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';

import '../identity.dart';
import 'drawer.dart' show largeText;
import 'metrics.dart' show TypeScale;

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

  /// The lead's first line, so a label beside the line (a Settings row's)
  /// sits on it (Lucy 50427).
  @override
  double? computeDistanceToActualBaseline(TextBaseline baseline) =>
      defaultComputeDistanceToFirstActualBaseline(baseline);

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

/// A Needs you card's action row (needs-you-cards.md §2 rule 2, §5): what
/// pressing does ([line]), then the quiet ones ([quiet]: the no, then any
/// second action) and the one filled [primary], last.
///
/// On a Mac or a wide iPad, [ActionLine]'s: the line at the left, the
/// buttons together at its right, the line above them when they don't
/// fit. On a phone, a thumb row under the line: one quiet button and the
/// primary share it, the primary at the right and wider; two quiet ones
/// share it and the primary takes the whole width under them, nearest the
/// thumb; at the largest text sizes each takes a row of its own, the
/// primary at the bottom. The same row in a conversation, the Needs you
/// page and focus mode's foot, so they can't drift apart.
class CardActions extends StatelessWidget {
  const CardActions({
    super.key,
    this.line,
    this.quietLine = false,
    this.quiet = const [],
    this.primary,
  });

  /// What pressing the primary does now, or why it can't (in `danger`).
  final Widget? line;

  /// [line] is only a hint, words that don't change what pressing does
  /// ("June gets your answer and carries on."): on a phone at large text
  /// it steps aside while the card's field has the keyboard, so the
  /// buttons fit above it (Lucy, needs-you-cards.md §5.2). Never for a
  /// status, an error, or a line that says what typing changes.
  final bool quietLine;

  /// The quiet buttons, in their order: the no first.
  final List<Widget> quiet;

  /// The one filled button, if the card has one.
  final Widget? primary;

  @override
  Widget build(BuildContext context) {
    final primary = this.primary;
    final buttons = <Widget>[...quiet, ?primary];
    final inSight = KeepInSight._of(context);
    final cramped = inSight?.cramped ?? 0;
    final hidden =
        (quietLine && (inSight?.typingLarge ?? false)) || cramped >= 2;
    // No room for it whole as well as the buttons and a line of the
    // field: one line, cut, read whole by VoiceOver; still none, it steps
    // aside until the keyboard goes (Zucchero row 706).
    final line = hidden
        ? null
        : cramped == 1 && this.line != null
        ? DefaultTextStyle.merge(
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            child: this.line!,
          )
        : this.line;
    if (!isPhoneLayout(context)) {
      final row = ControlsLine(children: buttons);
      if (line == null) {
        return Align(alignment: AlignmentDirectional.centerEnd, child: row);
      }
      return ActionLine(lead: line, action: row);
    }
    const gap = Space.s;
    // Two to a row, a label never wrapping: each takes at least its
    // label's width, the second (the primary) the rest; when both can't
    // fit, one a row, the second at the bottom (Lucy's check of §5.2).
    Widget pair(List<Widget> children, List<int> flex) => _ThumbPair(
      gap: gap,
      firstShare: flex[0] / (flex[0] + flex[1]),
      children: children,
    );
    final List<Widget> rows;
    if (largeText(context) || buttons.length == 1 || quiet.length > 2) {
      // One a row, the primary at the bottom.
      rows = buttons;
    } else if (primary == null) {
      rows = [
        pair(quiet, [1, 1]),
      ];
    } else if (quiet.length == 1) {
      rows = [
        pair([quiet.single, primary], [2, 3]),
      ];
    } else {
      rows = [
        pair(quiet, [1, 1]),
        primary,
      ];
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (line != null) ...[line, const SizedBox(height: gap)],
        for (final (i, row) in rows.indexed) ...[
          if (i > 0) const SizedBox(height: gap),
          row,
        ],
      ],
    );
  }
}

/// Two buttons on one row, the first taking [firstShare] of it, or more
/// when its label needs it, the second the rest, both as tall as the
/// taller; stacked, one a row with [gap] between, when their labels can't
/// both fit on one line.
class _ThumbPair extends MultiChildRenderObjectWidget {
  const _ThumbPair({
    required this.gap,
    required this.firstShare,
    required super.children,
  });

  final double gap;
  final double firstShare;

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderThumbPair(gap, firstShare, Directionality.of(context));

  @override
  void updateRenderObject(BuildContext context, _RenderThumbPair render) {
    render
      ..gap = gap
      ..firstShare = firstShare
      ..textDirection = Directionality.of(context);
  }
}

class _ThumbPairData extends ContainerBoxParentData<RenderBox> {}

class _RenderThumbPair extends RenderBox
    with
        ContainerRenderObjectMixin<RenderBox, _ThumbPairData>,
        RenderBoxContainerDefaultsMixin<RenderBox, _ThumbPairData> {
  _RenderThumbPair(this._gap, this._firstShare, this._textDirection);

  TextDirection _textDirection;
  set textDirection(TextDirection value) {
    if (value == _textDirection) return;
    _textDirection = value;
    markNeedsLayout();
  }

  double _gap;
  set gap(double value) {
    if (value == _gap) return;
    _gap = value;
    markNeedsLayout();
  }

  double _firstShare;
  set firstShare(double value) {
    if (value == _firstShare) return;
    _firstShare = value;
    markNeedsLayout();
  }

  @override
  void setupParentData(RenderBox child) {
    if (child.parentData is! _ThumbPairData) {
      child.parentData = _ThumbPairData();
    }
  }

  List<RenderBox> get _children => getChildrenAsList();

  /// The widths the two take side by side in [width], or null when their
  /// labels can't both fit on one line.
  (double, double)? _widths(double width) {
    final children = _children;
    if (children.length != 2) return null;
    final first = children[0].getMaxIntrinsicWidth(double.infinity);
    final second = children[1].getMaxIntrinsicWidth(double.infinity);
    final room = width - _gap;
    if (first + second > room) return null;
    final a = math.max(first, math.min(room * _firstShare, room - second));
    return (a, room - a);
  }

  @override
  double computeMinIntrinsicWidth(double height) => _children.fold(
    0,
    (most, child) => math.max(most, child.getMinIntrinsicWidth(height)),
  );

  @override
  double computeMaxIntrinsicWidth(double height) {
    final children = _children;
    return children.fold<double>(
          0,
          (sum, child) => sum + child.getMaxIntrinsicWidth(height),
        ) +
        _gap * math.max(0, children.length - 1);
  }

  double _height(double width, double Function(RenderBox, double) of) {
    final children = _children;
    final widths = _widths(width);
    if (widths == null) {
      return children.fold<double>(0, (sum, child) => sum + of(child, width)) +
          _gap * math.max(0, children.length - 1);
    }
    return math.max(of(children[0], widths.$1), of(children[1], widths.$2));
  }

  @override
  double computeMinIntrinsicHeight(double width) =>
      _height(width, (child, w) => child.getMinIntrinsicHeight(w));

  @override
  double computeMaxIntrinsicHeight(double width) =>
      _height(width, (child, w) => child.getMaxIntrinsicHeight(w));

  @override
  Size computeDryLayout(BoxConstraints constraints) {
    final width = constraints.maxWidth;
    return constraints.constrain(
      Size(width, _height(width, (child, w) => child.getMaxIntrinsicHeight(w))),
    );
  }

  @override
  void performLayout() {
    final width = constraints.maxWidth;
    final children = _children;
    final widths = _widths(width);
    if (widths == null) {
      var y = 0.0;
      for (final child in children) {
        child.layout(
          BoxConstraints.tightFor(width: width),
          parentUsesSize: true,
        );
        (child.parentData! as _ThumbPairData).offset = Offset(0, y);
        y += child.size.height + _gap;
      }
      size = constraints.constrain(
        Size(width, children.isEmpty ? 0 : y - _gap),
      );
      return;
    }
    final (a, b) = widths;
    final height = math.max(
      children[0].getMaxIntrinsicHeight(a),
      children[1].getMaxIntrinsicHeight(b),
    );
    children[0].layout(BoxConstraints.tight(Size(a, height)));
    children[1].layout(BoxConstraints.tight(Size(b, height)));
    final rtl = _textDirection == TextDirection.rtl;
    (children[0].parentData! as _ThumbPairData).offset = Offset(
      rtl ? b + _gap : 0,
      0,
    );
    (children[1].parentData! as _ThumbPairData).offset = Offset(
      rtl ? 0 : a + _gap,
      0,
    );
    size = constraints.constrain(Size(width, height));
  }

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) =>
      defaultHitTestChildren(result, position: position);

  @override
  void paint(PaintingContext context, Offset offset) =>
      defaultPaint(context, offset);
}

/// How many lines a card's field grows to before it scrolls: six, but
/// three on a phone at the largest text sizes, so the card's title stays
/// in sight above the keyboard on a small iPhone (needs-you-cards.md §5.2).
int cardFieldMaxLines(BuildContext context) =>
    isPhoneLayout(context) && largeText(context) ? 3 : 6;

/// A card's field and its [CardActions] ([child]), kept in sight together
/// on a phone while [focus] has the keyboard: when the field takes it, and
/// again as the keyboard comes up, the scroll it's in moves just enough for
/// the last button to sit above the keyboard (and the composer), so it's
/// pressed without putting the keyboard away (Squalo's B10). Elsewhere
/// [child] as it is.
///
/// When the field, the line and the buttons can't all fit (a small iPhone
/// at large text), the buttons come first, then the line, then the field:
/// a hint line ([CardActions.quietLine]) steps aside at large text, and the
/// field in a [CardFieldRoom] shrinks, to as little as one line, scrolling
/// inside with its caret in view (Lucy, needs-you-cards.md §5.2).
class KeepInSight extends StatefulWidget {
  const KeepInSight({super.key, required this.focus, required this.child});

  final FocusNode focus;
  final Widget child;

  static _InSight? _of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<_InSight>();

  @override
  State<KeepInSight> createState() => _KeepInSightState();
}

class _KeepInSightState extends State<KeepInSight> with WidgetsBindingObserver {
  /// A reveal waits for the frame's layout (the keyboard's new room).
  var _scheduled = false;

  /// Once the keyboard has stopped moving: the last word, after the
  /// field's own scroll to its caret, which would otherwise leave the
  /// buttons under the keyboard.
  Timer? _settled;

  /// The field's [CardFieldRoom], measured to share out the room.
  BuildContext? _fieldRoom;

  /// The most the field may take while it has the keyboard and the whole
  /// block wouldn't fit; null while it needn't shrink.
  double? _fieldMax;

  /// The field has the keyboard, on a phone at large text: hint lines step
  /// aside.
  var _typingLarge = false;

  /// The buttons, the line and one line of the field don't all fit while
  /// it has the keyboard: 1, the line keeps one line of its own; 2, still
  /// not, it steps aside. Kept until the field lets the keyboard go, so the
  /// line doesn't come and go as the person types.
  var _cramped = 0;

  @override
  void initState() {
    super.initState();
    widget.focus.addListener(_focusChanged);
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void didUpdateWidget(KeepInSight old) {
    super.didUpdateWidget(old);
    if (old.focus != widget.focus) {
      old.focus.removeListener(_focusChanged);
      widget.focus.addListener(_focusChanged);
    }
  }

  @override
  void dispose() {
    widget.focus.removeListener(_focusChanged);
    WidgetsBinding.instance.removeObserver(this);
    _settled?.cancel();
    super.dispose();
  }

  void _focusChanged() {
    if (!mounted) return;
    final typingLarge =
        widget.focus.hasFocus && isPhoneLayout(context) && largeText(context);
    if (typingLarge != _typingLarge ||
        (!widget.focus.hasFocus && (_fieldMax != null || _cramped > 0))) {
      setState(() {
        _typingLarge = typingLarge;
        if (!widget.focus.hasFocus) {
          _fieldMax = null;
          _cramped = 0;
        }
      });
    }
    if (widget.focus.hasFocus) _reveal();
  }

  /// The keyboard moving (each frame of its slide): the room changed.
  @override
  void didChangeMetrics() {
    if (widget.focus.hasFocus) _reveal();
  }

  void _reveal() {
    if (!mounted || !isPhoneLayout(context)) return;
    _settled?.cancel();
    _settled = Timer(
      const Duration(milliseconds: 250),
      () => _show(const Duration(milliseconds: 150)),
    );
    if (_scheduled) return;
    _scheduled = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _scheduled = false;
      _show(Duration.zero);
    });
  }

  void _show(Duration duration) {
    if (!mounted || !widget.focus.hasFocus) return;
    final scrollable = Scrollable.maybeOf(context);
    if (scrollable == null) return;
    if (_share(scrollable)) {
      // The field's room changed: reveal once it's laid out again.
      _reveal();
      return;
    }
    // Its bottom edge above the viewport's bottom, as far as it needs to
    // move (Flutter turns "end" round for a conversation's scroll, which
    // runs up from its newest).
    unawaited(
      Scrollable.ensureVisible(
        context,
        duration: duration,
        curve: Curves.easeOut,
        alignmentPolicy: ScrollPositionAlignmentPolicy.keepVisibleAtEnd,
      ),
    );
  }

  /// Shares out the room between the header and the keyboard: what isn't
  /// the field (the line, the buttons) keeps its height, and the field
  /// takes what's left, at least one line. True when that changed.
  bool _share(ScrollableState scrollable) {
    final block = context.findRenderObject();
    final room = scrollable.context.findRenderObject();
    final field = _fieldRoom?.findRenderObject();
    double? most;
    if (block is RenderBox &&
        room is RenderBox &&
        field is RenderBox &&
        block.hasSize &&
        room.hasSize &&
        field.hasSize) {
      final rest = block.size.height - field.size.height;
      final type = TypeScale.of(context).body;
      // One line of the field: its text, its padding and its border.
      final line =
          MediaQuery.textScalerOf(context).scale(type.fontSize ?? 13) *
              (type.height ?? 1.45) +
          2 * 10 +
          2;
      // The room in sight: the conversation's, above the keyboard where
      // the conversation runs on under it (Zucchero's row 706).
      final screen = MediaQueryData.fromView(View.of(context));
      final top = room.localToGlobal(Offset.zero).dy;
      final bottom = math.min(
        top + room.size.height,
        screen.size.height - screen.viewInsets.bottom,
      );
      final left = bottom - top - rest - Space.s;
      if (left < line && _typingLarge && _cramped < 2) {
        setState(() => _cramped++);
        return true;
      }
      most = math.max(line, left);
    }
    if ((most ?? -1) == (_fieldMax ?? -1) ||
        (most != null && _fieldMax != null && (most - _fieldMax!).abs() < 1)) {
      return false;
    }
    setState(() => _fieldMax = most);
    return true;
  }

  @override
  Widget build(BuildContext context) => _InSight(
    state: this,
    typingLarge: _typingLarge,
    cramped: _cramped,
    fieldMax: _fieldMax,
    // The block changing size as the person types (a line that says what
    // typing changes, the field growing) shares out the room again.
    child: NotificationListener<SizeChangedLayoutNotification>(
      onNotification: (_) {
        if (widget.focus.hasFocus) _reveal();
        return true;
      },
      child: SizeChangedLayoutNotifier(child: widget.child),
    ),
  );
}

class _InSight extends InheritedWidget {
  const _InSight({
    required this.state,
    required this.typingLarge,
    required this.cramped,
    required this.fieldMax,
    required super.child,
  });

  final _KeepInSightState state;
  final bool typingLarge;
  final int cramped;
  final double? fieldMax;

  @override
  bool updateShouldNotify(_InSight old) =>
      old.typingLarge != typingLarge ||
      old.cramped != cramped ||
      old.fieldMax != fieldMax;
}

/// A card's field inside a [KeepInSight]: while it has the keyboard and
/// the card's buttons wouldn't otherwise fit above it, it takes only the
/// room left, as little as one line, and scrolls inside with its caret in
/// view. Elsewhere [child] as it is.
class CardFieldRoom extends StatelessWidget {
  const CardFieldRoom({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final inSight = KeepInSight._of(context);
    if (inSight == null) return child;
    inSight.state._fieldRoom = context;
    // Always the same box around the field, so its limit coming and going
    // never builds the field again (what's typed and the focus stay).
    return ConstrainedBox(
      constraints: BoxConstraints(
        maxHeight: inSight.fieldMax ?? double.infinity,
      ),
      child: child,
    );
  }
}
