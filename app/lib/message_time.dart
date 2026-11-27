import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';

import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;

/// When a message was sent, as a clock on this device: "14:32", or with
/// the device set to a 12-hour clock ([h24] false), "2:32 PM".
String clockTime(int ms, {bool h24 = true}) {
  final at = DateTime.fromMillisecondsSinceEpoch(ms);
  final minutes = at.minute.toString().padLeft(2, '0');
  if (h24) return '${at.hour.toString().padLeft(2, '0')}:$minutes';
  final hour = at.hour % 12 == 0 ? 12 : at.hour % 12;
  return '$hour:$minutes ${at.hour < 12 ? 'AM' : 'PM'}';
}

/// The day [ms] falls on, for a conversation's day line: "Today",
/// "Yesterday", the weekday within the last week ("Monday"), then
/// "Mon 17 Nov", with the year when it isn't this one.
String dayLine(int ms, DateTime now) {
  final at = DateTime.fromMillisecondsSinceEpoch(ms);
  final day = DateTime(at.year, at.month, at.day);
  final today = DateTime(now.year, now.month, now.day);
  final days = today.difference(day).inHours ~/ 24;
  if (days == 0) return 'Today';
  if (days == 1) return 'Yesterday';
  const weekdays = [
    'Monday',
    'Tuesday',
    'Wednesday',
    'Thursday',
    'Friday',
    'Saturday',
    'Sunday',
  ];
  if (days > 1 && days < 7) return weekdays[at.weekday - 1];
  const months = [
    'Jan',
    'Feb',
    'Mar',
    'Apr',
    'May',
    'Jun',
    'Jul',
    'Aug',
    'Sep',
    'Oct',
    'Nov',
    'Dec',
  ];
  final short = weekdays[at.weekday - 1].substring(0, 3);
  final date = '$short ${at.day} ${months[at.month - 1]}';
  return at.year == now.year ? date : '$date ${at.year}';
}

/// Whether [a] and [b] (ms) fall on the same day on this device.
bool sameDay(int a, int b) {
  final x = DateTime.fromMillisecondsSinceEpoch(a);
  final y = DateTime.fromMillisecondsSinceEpoch(b);
  return x.year == y.year && x.month == y.month && x.day == y.day;
}

/// Where a conversation's day changes: the day, centred, in a faint
/// caption.
class DayLine extends StatelessWidget {
  const DayLine({super.key, required this.ms});

  final int ms;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: Space.l),
      child: Center(
        child: Text(
          dayLine(ms, DateTime.now()),
          style: TypeScale.of(context).caption
              .copyWith(color: palette.textFaint),
        ),
      ),
    );
  }
}

/// A message's words with its time at their foot, as chat apps put it: at
/// the end of the last line when there's room beside it, else on a line of
/// its own at the right. The time is never selected with the words.
class TimeFoot extends StatelessWidget {
  const TimeFoot({
    super.key,
    required this.ms,
    required this.color,
    required this.child,
  });

  /// When it was sent.
  final int ms;
  final Color color;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    // The device's own clock, 24-hour or not, as its Messages shows it.
    final time = clockTime(
      ms,
      h24: MediaQuery.maybeAlwaysUse24HourFormatOf(context) ?? true,
    );
    return _TimeFoot(
      children: [
        child,
        // At the largest text sizes it grows at most twice as big, so the
        // words keep the room.
        MediaQuery.withClampedTextScaling(
          maxScaleFactor: 2,
          child: SelectionContainer.disabled(
            child: Semantics(
              label: 'at $time',
              excludeSemantics: true,
              child: Text(
                time,
                style: TypeScale.of(context).caption.copyWith(
                  color: color,
                  fontSize: 11,
                  height: 1.2,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

class _TimeFoot extends MultiChildRenderObjectWidget {
  const _TimeFoot({required super.children});

  @override
  RenderObject createRenderObject(BuildContext context) =>
      RenderTimeFoot(Directionality.of(context));

  @override
  void updateRenderObject(BuildContext context, RenderTimeFoot renderObject) {
    renderObject.textDirection = Directionality.of(context);
  }
}

class _FootData extends ContainerBoxParentData<RenderBox> {}

/// Lays out the words, then the time at the end of their last line when
/// it fits there (the last line is a paragraph's, at the very bottom), or
/// under them at the right.
class RenderTimeFoot extends RenderBox
    with
        ContainerRenderObjectMixin<RenderBox, _FootData>,
        RenderBoxContainerDefaultsMixin<RenderBox, _FootData> {
  RenderTimeFoot(this._textDirection);

  TextDirection _textDirection;
  set textDirection(TextDirection value) {
    if (value == _textDirection) return;
    _textDirection = value;
    markNeedsLayout();
  }

  /// Between the last word and the time.
  static const gap = Space.s;

  /// Whether the time went on the last line, for tests.
  @visibleForTesting
  bool inline = false;

  @override
  void setupParentData(RenderBox child) {
    if (child.parentData is! _FootData) child.parentData = _FootData();
  }

  RenderBox get _content => firstChild!;
  RenderBox get _time => lastChild!;

  @override
  double computeMinIntrinsicWidth(double height) => math.max(
    _content.getMinIntrinsicWidth(height),
    _time.getMinIntrinsicWidth(height),
  );

  @override
  double computeMaxIntrinsicWidth(double height) =>
      _content.getMaxIntrinsicWidth(height) +
      gap +
      _time.getMaxIntrinsicWidth(height);

  @override
  double computeMinIntrinsicHeight(double width) =>
      _content.getMinIntrinsicHeight(width) +
      _time.getMinIntrinsicHeight(width);

  @override
  double computeMaxIntrinsicHeight(double width) =>
      _content.getMaxIntrinsicHeight(width) +
      _time.getMaxIntrinsicHeight(width);

  @override
  Size computeDryLayout(BoxConstraints constraints) {
    final content = _content.getDryLayout(constraints.loosen());
    final time = _time.getDryLayout(constraints.loosen());
    return constraints.constrain(
      Size(math.max(content.width, time.width), content.height + time.height),
    );
  }

  /// The right end and the bottom of the last line of the last paragraph
  /// in [_content], in its coordinates; null when there's none.
  (double, double)? _lastLine() {
    RenderParagraph? last;
    void visit(RenderObject object) {
      if (object is RenderParagraph) last = object;
      object.visitChildren(visit);
    }

    visit(_content);
    final paragraph = last;
    if (paragraph == null || !paragraph.hasSize) return null;
    // The end of its text: where a caret after the last character sits,
    // and its line's height.
    final end = TextPosition(
      offset: paragraph.text.toPlainText().length,
      affinity: TextAffinity.upstream,
    );
    final caret = paragraph.getOffsetForCaret(end, Rect.zero);
    final height = paragraph.getFullHeightForCaret(end);
    // How far the last line reaches on screen, which isn't where its text
    // ends logically when it changes direction: a run the person reversed
    // (U+202E) would otherwise run under the time.
    var dx = caret.dx;
    // Each box the height of its line, so runs in another font (Hebrew or
    // Arabic from a fallback font, taller or shorter) are on it too.
    final boxes = paragraph.getBoxesForSelection(
      TextSelection(baseOffset: 0, extentOffset: end.offset),
      boxHeightStyle: ui.BoxHeightStyle.max,
    );
    if (boxes.isNotEmpty) {
      // The last line: whatever reaches below the top of its lowest box.
      final top = boxes.map((box) => box.top).reduce(math.max);
      final line = boxes.where((box) => box.bottom > top + 1);
      dx = _textDirection == TextDirection.ltr
          ? line.map((box) => box.right).reduce(math.max)
          : line.map((box) => box.left).reduce(math.min);
    }
    final transform = paragraph.getTransformTo(_content);
    final corner = MatrixUtils.transformPoint(
      transform,
      Offset(dx, caret.dy + height),
    );
    return (corner.dx, corner.dy);
  }

  @override
  void performLayout() {
    final loose = constraints.loosen();
    _time.layout(loose, parentUsesSize: true);
    _content.layout(loose, parentUsesSize: true);
    final content = _content.size;
    final time = _time.size;
    final room = constraints.maxWidth;
    final line = _lastLine();
    final ltr = _textDirection == TextDirection.ltr;
    // The last line ends the words, and the time fits after it.
    final fits =
        line != null &&
        line.$2 >= content.height - 3 &&
        (ltr ? line.$1 : content.width - line.$1) + gap + time.width <= room;
    inline = fits;
    final (contentData, timeData) = (
      _content.parentData! as _FootData,
      _time.parentData! as _FootData,
    );
    if (fits) {
      final width = math.max(
        content.width,
        (ltr ? line.$1 : content.width - line.$1) + gap + time.width,
      );
      final height = math.max(content.height, time.height);
      size = constraints.constrain(Size(width, height));
      contentData.offset = Offset(
        ltr ? 0 : size.width - content.width,
        height - content.height,
      );
      timeData.offset = Offset(
        ltr ? size.width - time.width : 0,
        // Level with the last line's foot, a touch low.
        height - time.height + 1,
      );
    } else {
      size = constraints.constrain(
        Size(math.max(content.width, time.width), content.height + time.height),
      );
      contentData.offset = Offset(ltr ? 0 : size.width - content.width, 0);
      timeData.offset = Offset(
        ltr ? size.width - time.width : 0,
        content.height,
      );
    }
  }

  @override
  void paint(PaintingContext context, Offset offset) =>
      defaultPaint(context, offset);

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) =>
      defaultHitTestChildren(result, position: position);
}
