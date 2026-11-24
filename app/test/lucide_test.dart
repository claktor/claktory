import 'package:factory_app/controls.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    "the app knows Charts' menu icons (#419), and says which it doesn't",
    () {
      for (final name in ['star', 'house', 'home', 'clock', 'copy']) {
        expect(hasLucideIcon(name), isTrue, reason: name);
      }
      expect(hasLucideIcon('no-such-icon'), isFalse);
    },
  );

  testWidgets('a known icon is drawn, an unknown one is its letter', (
    tester,
  ) async {
    await tester.pumpWidget(
      const Directionality(
        textDirection: TextDirection.ltr,
        child: Row(
          children: [
            ExtensionGlyph(
              icon: 'home',
              name: 'Home',
              color: Color(0xFFFFFFFF),
            ),
            ExtensionGlyph(
              icon: 'nope',
              name: 'Nope',
              color: Color(0xFFFFFFFF),
            ),
          ],
        ),
      ),
    );
    expect(
      find.descendant(
        of: find.byType(ExtensionGlyph).first,
        matching: find.byType(CustomPaint),
      ),
      findsOneWidget,
    );
    expect(find.text('N'), findsOneWidget);
  });
}
