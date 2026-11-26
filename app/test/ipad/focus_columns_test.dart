import 'package:factory_app/ipad/columns.dart';
import 'package:factory_app/ipad/focus_columns.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

/// The composition the iPad's focus mode takes in a window [size], at the
/// default text size, as the window itself picks its tier.
FocusColumns _at(Size size) =>
    focusColumnsFor(tierFor(idiom: Idiom.pad, window: size), size.width);

void main() {
  test('a 13" landscape window has the queue, the stage and Context', () {
    final columns = _at(const Size(1376, 1032));
    expect(columns.composition, FocusComposition.columns);
    expect(columns.queue, FocusGeometry.queue);
    // Context keeps its least width; the stage takes the rest, a little
    // under its card's column and padding.
    expect(columns.context, FocusGeometry.context);
    expect(columns.stage, 1376 - 280 - 2 - 360);
    expect(columns.contextBeside, isTrue);
    expect(columns.queue + columns.stage + columns.context + 2, 1376);
  });

  test('a wider window (an external display) gives Context the rest', () {
    final columns = _at(const Size(1920, 1080));
    expect(columns.stage, 768);
    expect(columns.context, 1920 - 280 - 2 - 768);
  });

  test('an 11" landscape window keeps Context at its least width', () {
    final columns = _at(const Size(1210, 834));
    expect(columns.composition, FocusComposition.columns);
    expect(columns.context, FocusGeometry.context);
    expect(columns.stage, 1210 - 280 - 2 - 360);
    // Hidden, Context gives its room to the stage.
    expect(columns.stageAlone, 1210 - 280 - 1);
  });

  test('13" portrait puts Context over the stage', () {
    final columns = _at(const Size(1032, 1376));
    expect(columns.composition, FocusComposition.contextOver);
    expect(columns.queue, FocusGeometry.queueNarrow);
    expect(columns.stage, 1032 - 240 - 1);
    expect(columns.contextBeside, isFalse);
    expect(columns.contextPanel, FocusGeometry.contextPanel);
  });

  test('11" portrait and half a 13" show the stage alone', () {
    for (final size in const [Size(834, 1210), Size(688, 1032)]) {
      final columns = _at(size);
      expect(columns.composition, FocusComposition.stageOnly, reason: '$size');
      expect(columns.queue, 0);
      expect(columns.stage, size.width);
      expect(columns.contextPanel, size.width - FocusGeometry.stageStrip);
    }
  });

  test('Slide Over and a third of the screen take the phone\'s layout', () {
    for (final size in const [Size(375, 1032), Size(507, 1032)]) {
      expect(_at(size).composition, FocusComposition.phone, reason: '$size');
    }
  });

  test('a large text size moves to the next composition down', () {
    // 1210 is full at the default size, paired once the text is half again
    // as large, since the workbench's thresholds grow with it.
    final tier = tierFor(
      idiom: Idiom.pad,
      window: const Size(1210, 834),
      textFactor: 1.5,
    );
    final columns = focusColumnsFor(tier, 1210, textFactor: 1.5);
    expect(columns.composition, FocusComposition.contextOver);
    // The queue widens with the text, so its names still read.
    expect(columns.queue, FocusGeometry.queueNarrow * 1.5);
  });
}
