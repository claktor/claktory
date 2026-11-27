// The header's model popover chooses an effort in one press: a segmented
// control, a finger's 44pt on an iPad, 28px on a Mac, its efforts in rows as
// even as can be where they won't fit across.
import 'package:factory_app/ipad/effort_segments.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart' show Harness;
import 'as_ipad.dart';

const _all = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'];

Future<List<String>> _show(
  WidgetTester tester, {
  List<String> efforts = const ['low', 'medium', 'high'],
  String value = 'medium',
  double width = 280,
  bool iPad = true,
}) async {
  if (iPad) asIPad();
  final chosen = <String>[];
  await Harness(Workspace()).show(
    tester,
    size: const Size(1032, 1376),
    home: Align(
      alignment: Alignment.topLeft,
      child: SizedBox(
        width: width,
        child: EffortSegments(
          efforts: efforts,
          value: value,
          onChanged: chosen.add,
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
  return chosen;
}

Rect _rect(WidgetTester tester, String effort) =>
    tester.getRect(find.byKey(ValueKey('effort-$effort')));

void main() {
  testWidgets('one press chooses another effort; the chosen one does '
      'nothing', (tester) async {
    final chosen = await _show(tester);
    expect(find.text('Medium'), findsOneWidget);
    await tester.tap(find.text('High'));
    await tester.tap(find.text('Medium'));
    expect(chosen, ['high']);
  });

  testWidgets('on an iPad each segment is 44pt tall', (tester) async {
    await _show(tester);
    expect(_rect(tester, 'low').height, 44);
  });

  testWidgets('on a Mac 28px', (tester) async {
    await _show(tester, iPad: false);
    expect(_rect(tester, 'low').height, 28);
  });

  testWidgets("seven efforts that don't all fit across take rows as even "
      'as can be, lined up under the first', (tester) async {
    // Whatever the font (the test's, or Geist that another test loaded),
    // 320 is too narrow for seven: what's checked is the shape.
    await _show(tester, efforts: _all, value: 'high', width: 320);
    final rects = [for (final effort in _all) _rect(tester, effort)];
    final tops = {for (final rect in rects) rect.top}.toList()..sort();
    expect(tops.length, greaterThan(1));
    final rows = [
      for (final top in tops) rects.where((rect) => rect.top == top).toList(),
    ];
    final first = rows.first;
    for (final row in rows) {
      // As even as can be: no row more than one shorter than the first.
      expect(row.length, inInclusiveRange(first.length - 1, first.length));
      // Lined up: each segment as wide as the first row's, from its left.
      for (final (index, rect) in row.indexed) {
        expect(rect.left, closeTo(first[index].left, 0.01));
        expect(rect.width, closeTo(first[index].width, 0.01));
      }
    }
    expect(find.text('Extra high'), findsOneWidget);
  });

  testWidgets('VoiceOver hears each as a button, the chosen one selected', (
    tester,
  ) async {
    final handle = tester.ensureSemantics();
    await _show(tester);
    expect(
      tester.getSemantics(find.byKey(const ValueKey('effort-medium'))),
      matchesSemantics(
        label: 'Medium',
        isButton: true,
        isSelected: true,
        hasSelectedState: true,
        isInMutuallyExclusiveGroup: true,
        isEnabled: true,
        hasEnabledState: true,
        hasTapAction: true,
      ),
    );
    handle.dispose();
  });
}
