// A Select's menu opens below its field, and above it when there's no room
// below (a bar at the window's foot, such as History's),
// always inside the window.
import 'package:factory_app/controls.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

Future<void> _show(
  WidgetTester tester,
  Alignment where, {
  int options = 3,
}) async {
  // A Mac's menu (a phone's choices are a sheet).
  debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  await tester.binding.setSurfaceSize(const Size(800, 500));
  addTearDown(() => tester.binding.setSurfaceSize(null));
  await tester.pumpWidget(
    Directionality(
      textDirection: TextDirection.ltr,
      child: MediaQuery(
        data: const MediaQueryData(size: Size(800, 500)),
        child: Overlay(
          initialEntries: [
            OverlayEntry(
              builder: (context) => Align(
                alignment: where,
                child: Padding(
                  padding: const EdgeInsets.all(8),
                  child: SizedBox(
                    width: 200,
                    child: Select<int>(
                      value: 0,
                      options: [
                        for (var i = 0; i < options; i++)
                          SelectOption(i, 'Choice $i'),
                      ],
                      onChanged: (_) {},
                    ),
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    ),
  );
  await tester.tap(find.text('Choice 0'));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('it opens below its field where there is room', (tester) async {
    await _show(tester, Alignment.topLeft);
    final field = tester.getRect(find.byType(Select<int>));
    final last = tester.getRect(find.text('Choice 2').last);
    expect(last.top, greaterThan(field.bottom));
    expect(last.bottom, lessThanOrEqualTo(500));
  });

  testWidgets('at the window\'s foot it opens above its field, inside the '
      'window', (tester) async {
    await _show(tester, Alignment.bottomLeft);
    final field = tester.getRect(find.byType(Select<int>));
    for (var i = 0; i < 3; i++) {
      final row = tester.getRect(find.text('Choice $i').last);
      expect(row.bottom, lessThan(field.top), reason: 'Choice $i');
      expect(row.top, greaterThanOrEqualTo(0), reason: 'Choice $i');
    }
    // Picked as anywhere else.
    expect(
      tester.getRect(find.text('Choice 1').last).top,
      greaterThan(tester.getRect(find.text('Choice 0').last).top),
    );
  });

  testWidgets('a long menu at the foot is kept inside the window and '
      'scrolls', (tester) async {
    await _show(tester, Alignment.bottomLeft, options: 40);
    final field = tester.getRect(find.byType(Select<int>));
    final menu = tester.getRect(
      find.ancestor(
        of: find.text('Choice 0').last,
        matching: find.byType(ListView),
      ),
    );
    expect(menu.bottom, lessThan(field.top));
    expect(menu.top, greaterThanOrEqualTo(0));
  });
}
