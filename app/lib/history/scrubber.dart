// History's scrubber (#458): the whole history as a bar, a sparkline of
// how busy each stretch was, the chapters marked over it, what was
// reconstructed shaded, and the playhead; a click or a drag goes there.
import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../message_time.dart';
import '../protocol/factory/v1/factory.pb.dart';
import 'controller.dart';

/// When [ms] was, on this device's clock: "Wed 19 Nov 09:05", with the
/// year when it isn't this one; [short], without the weekday.
String historyWhen(BuildContext context, double ms, {bool short = false}) {
  final at = DateTime.fromMillisecondsSinceEpoch(ms.round());
  const weekdays = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  const months = [
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
  ];
  final year = at.year == DateTime.now().year ? '' : ' ${at.year}';
  final clock = clockTime(
    ms.round(),
    h24: MediaQuery.maybeAlwaysUse24HourFormatOf(context) ?? true,
  );
  final day = short ? '' : '${weekdays[at.weekday - 1]} ';
  return '$day${at.day} ${months[at.month - 1]}$year $clock';
}

/// The chapter the playhead is in: the last one it passed.
HistoryChapter? chapterAt(List<HistoryChapter> chapters, double t) {
  HistoryChapter? found;
  for (final chapter in chapters) {
    if (chapter.atMs.toDouble() > t) break;
    found = chapter;
  }
  return found;
}

class HistoryScrubber extends StatelessWidget {
  const HistoryScrubber({
    super.key,
    required this.controller,
    this.height = 44,
    this.phone = false,
  });

  final HistoryController controller;
  final double height;

  /// A finger's: taller, no hover cards on the chapters.
  final bool phone;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        void seekTo(double x) {
          final timeline = controller.timeline;
          if (timeline == null || width <= 0) return;
          final f = (x / width).clamp(0.0, 1.0);
          controller.seek(
            timeline.start + f * (controller.last - timeline.start),
          );
        }

        double ahead(double fraction) {
          final timeline = controller.timeline;
          final t = controller.time.value;
          if (timeline == null) return t;
          return (t + fraction * (controller.last - timeline.start)).clamp(
            timeline.start,
            controller.last,
          );
        }

        void step(double fraction) => controller.seek(ahead(fraction));

        return Semantics(
          slider: true,
          label: 'Time',
          value: controller.fetching
              ? '${historyWhen(context, controller.time.value)}, loading'
              : historyWhen(context, controller.time.value),
          increasedValue: historyWhen(context, ahead(0.02)),
          decreasedValue: historyWhen(context, ahead(-0.02)),
          onIncrease: () => step(0.02),
          onDecrease: () => step(-0.02),
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTapDown: (details) => seekTo(details.localPosition.dx),
            onHorizontalDragStart: (details) =>
                seekTo(details.localPosition.dx),
            onHorizontalDragUpdate: (details) =>
                seekTo(details.localPosition.dx),
            child: SizedBox(
              height: height,
              child: Stack(
                children: [
                  Positioned.fill(
                    child: CustomPaint(
                      painter: _ScrubberPainter(controller, palette),
                    ),
                  ),
                  if (!phone)
                    for (final chapter in controller.chapters)
                      _mark(context, chapter, width),
                ],
              ),
            ),
          ),
        );
      },
    );
  }

  /// A chapter's mark, with its title on hover.
  Widget _mark(BuildContext context, HistoryChapter chapter, double width) {
    final timeline = controller.timeline!;
    final span = math.max(1.0, controller.last - timeline.start);
    final x = (chapter.atMs.toDouble() - timeline.start) / span * width;
    return Positioned(
      left: x - 6,
      top: 0,
      width: 12,
      height: 12,
      child: HoverCard(
        card: (context) => Text(
          '${chapter.title} · ${historyWhen(context, chapter.atMs.toDouble())}',
        ),
        child: const SizedBox.expand(),
      ),
    );
  }
}

class _ScrubberPainter extends CustomPainter {
  _ScrubberPainter(this.controller, this.palette)
    : super(repaint: Listenable.merge([controller, controller.time]));

  final HistoryController controller;
  final Palette palette;

  @override
  void paint(Canvas canvas, Size size) {
    final timeline = controller.timeline;
    if (timeline == null || size.width <= 0) return;
    final start = timeline.start;
    final span = math.max(1.0, controller.last - start);
    double x(double t) => (t - start) / span * size.width;
    const marks = 12.0;
    final top = marks;
    final bottom = size.height - 2;
    final bars = bottom - top;

    // What was reconstructed, shaded, under a faint line along its foot.
    if (timeline.reconstructed) {
      final until = x(timeline.recordingSince).clamp(0.0, size.width);
      canvas.drawRect(
        Rect.fromLTRB(0, top, until, bottom),
        Paint()..color = palette.hover,
      );
    }

    // The sparkline: events per stretch, a bar every 3 pixels.
    const step = 3.0;
    final columns = math.max(1, (size.width / step).floor());
    final counts = List<int>.filled(columns, 0);
    const hour = Duration.millisecondsPerHour;
    for (var k = 0; k < timeline.histogram.length; k++) {
      final at = timeline.histogramStart + k * hour + hour / 2;
      final column = (x(at) / step).floor();
      if (column >= 0 && column < columns) {
        counts[column] += timeline.histogram[k];
      }
    }
    final most = counts.fold(1, math.max);
    final played = x(controller.time.value);
    for (var c = 0; c < columns; c++) {
      if (counts[c] == 0) continue;
      final h = math.max(1.5, bars * math.sqrt(counts[c] / most));
      final left = c * step;
      canvas.drawRect(
        Rect.fromLTWH(left, bottom - h, step - 1, h),
        Paint()
          ..color = left < played
              ? palette.textMuted
              : palette.textFaint.withValues(alpha: 0.6),
      );
    }
    // The baseline.
    canvas.drawLine(
      Offset(0, bottom + 0.5),
      Offset(size.width, bottom + 0.5),
      Paint()..color = palette.hairline,
    );

    // Chapters, as small dots over the bars.
    for (final chapter in controller.chapters) {
      final at = x(chapter.atMs.toDouble());
      canvas.drawCircle(
        Offset(at, marks / 2),
        2.5,
        Paint()
          ..color = chapter.atMs.toDouble() <= controller.time.value
              ? palette.textMuted
              : palette.textFaint,
      );
    }

    // The playhead: hollow and faint while what's there is on its way.
    final head = played.clamp(1.0, size.width - 1);
    final waiting = controller.fetching;
    final color = waiting
        ? palette.signal.withValues(alpha: 0.45)
        : palette.signal;
    canvas
      ..drawLine(
        Offset(head, top - 2),
        Offset(head, bottom + 1),
        Paint()
          ..color = color
          ..strokeWidth = 2,
      )
      ..drawCircle(
        Offset(head, top - 2),
        3,
        waiting
            ? (Paint()
                ..color = palette.signal
                ..style = PaintingStyle.stroke
                ..strokeWidth = 1.5)
            : (Paint()..color = palette.signal),
      );
  }

  @override
  bool shouldRepaint(_ScrubberPainter old) =>
      old.controller != controller || old.palette != palette;
}
