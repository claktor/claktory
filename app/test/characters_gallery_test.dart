import 'package:factory_app/characters/identity.dart';
import 'package:factory_app/characters/painter.dart';
import 'package:factory_app/characters/pose.dart';
import 'package:factory_app/identity.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

/// Every shape (rows, cycling through the eight colors) × mood × size
/// (columns), in each mood's still pose: the golden matrix to judge the
/// characters by eye. Regenerate with --update-goldens.
const _sizes = [20.0, 24.0, 34.0, 64.0];
const _cell = 72.0;

Widget gallery(Brightness brightness) {
  // The canvas each theme has.
  final palette = brightness == Brightness.dark
      ? const Color(0xFF0E0E10)
      : const Color(0xFFFFFFFF);
  final colors = agentColors.values.toList();
  return Directionality(
    textDirection: TextDirection.ltr,
    child: ColoredBox(
      color: palette,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final shape in CharacterShape.values)
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (final mood in Mood.values)
                  for (final size in _sizes)
                    SizedBox.square(
                      dimension: _cell,
                      child: Center(
                        child: CustomPaint(
                          size: Size.square(size),
                          painter: CharacterPainter(
                            spec: CharacterSpec(
                              shape: shape,
                              color: colors[shape.index % colors.length],
                              seed: 0,
                            ),
                            pose: () => stillPose(mood),
                          ),
                        ),
                      ),
                    ),
              ],
            ),
        ],
      ),
    ),
  );
}

void main() {
  for (final brightness in Brightness.values) {
    for (final ratio in [1.0, 2.0]) {
      testWidgets('gallery ${brightness.name} @${ratio.toInt()}x', (
        tester,
      ) async {
        const columns = 7 * 4, rows = 6;
        tester.view.physicalSize =
            const Size(columns * _cell, rows * _cell) * ratio;
        tester.view.devicePixelRatio = ratio;
        addTearDown(tester.view.reset);
        await tester.pumpWidget(gallery(brightness));
        await expectLater(
          find.byType(Column).first,
          matchesGoldenFile(
            'goldens/characters_${brightness.name}_${ratio.toInt()}x.png',
          ),
        );
      });
    }
  }
}
