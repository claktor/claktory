// Words that scroll in a short box fade at its foot while there's more
// below (mobile/scroll_fade.dart), and reaching the end keeps its place.
import 'package:factory_app/mobile/scroll_fade.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  Future<void> show(WidgetTester tester, double height) => tester.pumpWidget(
    Directionality(
      textDirection: TextDirection.ltr,
      child: Center(
        child: SizedBox(
          width: 200,
          height: 120,
          child: ScrollsWithFade(child: SizedBox(height: height)),
        ),
      ),
    ),
  );

  ScrollPosition position(WidgetTester tester) =>
      tester.state<ScrollableState>(find.byType(Scrollable)).position;

  testWidgets('dragged to the end, it stays at the end (Sorbet 43852)', (
    tester,
  ) async {
    await show(tester, 400);
    await tester.pump();
    await tester.drag(find.byType(Scrollable), const Offset(0, -1000));
    await tester.pumpAndSettle();
    expect(position(tester).pixels, position(tester).maxScrollExtent);
    await tester.pump(const Duration(seconds: 1));
    expect(position(tester).pixels, position(tester).maxScrollExtent);
  });

  testWidgets('words that fit: nothing to scroll, nothing faded', (
    tester,
  ) async {
    await show(tester, 80);
    await tester.pump();
    expect(position(tester).maxScrollExtent, 0);
    expect(tester.takeException(), isNull);
  });
}
