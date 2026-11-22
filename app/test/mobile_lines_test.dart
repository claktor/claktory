// ActionLine on a phone: the action beside its lead while both fit, under
// it, at its end, when the action is wider than the line leaves it (a
// question card's Dismiss and Approve in Needs you's sheet), never past the
// edge.
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

  testWidgets('under the lead, at its end, when the action is too wide '
      'to leave the lead room', (tester) async {
    // Dismiss and Approve, 322 wide, in 299: they wrap, the last under the
    // thumb.
    await line(tester, width: 299, actions: [150, 164]);
    expect(tester.takeException(), isNull);
    final lead = tester.getRect(find.byKey(const Key('lead')));
    final first = tester.getRect(find.byKey(const Key('a0')));
    expect(lead.width, 299);
    expect(first.top, greaterThanOrEqualTo(lead.bottom));
    expect(first.right, lessThanOrEqualTo(299));
  });

  testWidgets('under the lead, its controls together at its end, when '
      'their spacing leaves the lead no room', (tester) async {
    // 100 + 8 + 100 beside 96 of lead and the gap needs 312: in 310 they
    // go under it, side by side, rather than wrapping beside it.
    await line(tester, width: 310, actions: [100, 100]);
    expect(tester.takeException(), isNull);
    final lead = tester.getRect(find.byKey(const Key('lead')));
    final first = tester.getRect(find.byKey(const Key('a0')));
    final last = tester.getRect(find.byKey(const Key('a1')));
    expect(lead.width, 310);
    expect(first.top, greaterThanOrEqualTo(lead.bottom));
    expect(last.top, first.top);
    expect(last.right, 310);
    expect(first.right, last.left - 8);
  });

  testWidgets('a lone button under the lead stays at its end', (tester) async {
    await line(tester, width: 299, actions: [220]);
    final lead = tester.getRect(find.byKey(const Key('lead')));
    final button = tester.getRect(find.byKey(const Key('a0')));
    expect(button.top, greaterThanOrEqualTo(lead.bottom));
    expect(button.right, 299);
    expect(button.height, 44);
  });

  testWidgets('wider than the line itself, its controls stack, each the '
      'whole width, the last at the bottom', (tester) async {
    await line(tester, width: 299, actions: [200, 120]);
    expect(tester.takeException(), isNull);
    final lead = tester.getRect(find.byKey(const Key('lead')));
    final first = tester.getRect(find.byKey(const Key('a0')));
    final second = tester.getRect(find.byKey(const Key('a1')));
    expect(first.top, greaterThanOrEqualTo(lead.bottom));
    expect(second.top, first.bottom + 8);
    for (final control in [first, second]) {
      expect(control.left, 0);
      expect(control.right, 299);
    }
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
    // Under it, at its end: the left.
    await line(
      tester,
      width: 310,
      actions: [100, 100],
      direction: TextDirection.rtl,
    );
    expect(tester.getRect(find.byKey(const Key('a1'))).left, 0);
    expect(tester.takeException(), isNull);
  });
}
