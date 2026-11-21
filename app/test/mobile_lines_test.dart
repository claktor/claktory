// ActionLine on a phone: the action beside its lead while both fit, under
// it when the action is wider than the line leaves it (a question card's
// Dismiss and Approve in Needs you's sheet), never past the edge.
import 'package:factory_app/mobile/lines.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  Future<void> line(
    WidgetTester tester, {
    required double width,
    required List<double> actions,
    Size screen = const Size(393, 852),
    TextDirection direction = TextDirection.ltr,
  }) async {
    tester.view
      ..physicalSize = screen
      ..devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      MediaQuery(
        data: MediaQueryData(size: screen),
        child: Directionality(
          textDirection: direction,
          child: Align(
            alignment: Alignment.topLeft,
            child: SizedBox(
              width: width,
              child: ActionLine(
                lead: const SizedBox(
                  key: Key('lead'),
                  height: 20,
                  child: Text('Asked just now'),
                ),
                action: ControlsLine(
                  children: [
                    for (final (i, w) in actions.indexed)
                      SizedBox(key: Key('a$i'), width: w, height: 44),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  testWidgets('beside the lead while it fits', (tester) async {
    await line(tester, width: 299, actions: [80, 80]);
    final lead = tester.getRect(find.byKey(const Key('lead')));
    final last = tester.getRect(find.byKey(const Key('a1')));
    expect(last.right, 299);
    expect(lead.left, 0);
    expect(lead.right, lessThanOrEqualTo(299 - 8 - 168));
    expect(lead.center.dy, closeTo(last.center.dy, 0.01));
  });

  testWidgets('under the lead, at its start, when the action is too wide '
      'to leave the lead room', (tester) async {
    // Dismiss and Approve, 322 wide, in 299.
    await line(tester, width: 299, actions: [150, 164]);
    expect(tester.takeException(), isNull);
    final lead = tester.getRect(find.byKey(const Key('lead')));
    final first = tester.getRect(find.byKey(const Key('a0')));
    expect(lead.width, 299);
    expect(first.left, 0);
    expect(first.top, greaterThanOrEqualTo(lead.bottom));
  });

  testWidgets('wider than the line itself, its controls wrap', (tester) async {
    await line(tester, width: 299, actions: [200, 200]);
    expect(tester.takeException(), isNull);
    final first = tester.getRect(find.byKey(const Key('a0')));
    final second = tester.getRect(find.byKey(const Key('a1')));
    expect(second.top, greaterThan(first.top));
    expect(second.right, lessThanOrEqualTo(299));
  });

  testWidgets('right to left, mirrored', (tester) async {
    await line(
      tester,
      width: 299,
      actions: [80, 80],
      direction: TextDirection.rtl,
    );
    final lead = tester.getRect(find.byKey(const Key('lead')));
    expect(lead.right, 299);
    await line(
      tester,
      width: 299,
      actions: [150, 164],
      direction: TextDirection.rtl,
    );
    final first = tester.getRect(find.byKey(const Key('a0')));
    expect(first.right, lessThanOrEqualTo(299));
    expect(tester.takeException(), isNull);
  });
}
