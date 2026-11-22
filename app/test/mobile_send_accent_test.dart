// Send in a phone's message box takes the target's accent (a pipe's
// color) in place of signal, with a white arrow on it.

import 'package:factory_app/controls.dart' show ExtensionGlyph;
import 'package:factory_app/identity.dart';
import 'package:factory_app/mobile/composer.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  Future<(Color?, Color)> pump(WidgetTester tester, {Color? accent}) async {
    await tester.pumpWidget(
      MediaQuery(
        data: const MediaQueryData(size: Size(390, 844)),
        child: Directionality(
          textDirection: TextDirection.ltr,
          child: Center(
            child: SendButton(onSend: () {}, accent: accent),
          ),
        ),
      ),
    );
    final disc = tester.widget<AnimatedContainer>(
      find.descendant(
        of: find.byType(SendButton),
        matching: find.byType(AnimatedContainer),
      ),
    );
    final glyph = tester.widget<ExtensionGlyph>(find.byType(ExtensionGlyph));
    return ((disc.decoration as BoxDecoration?)?.color, glyph.color);
  }

  testWidgets('Send is signal with a black arrow by default', (tester) async {
    final (fill, arrow) = await pump(tester);
    final context = tester.element(find.byType(SendButton));
    expect(fill, Palette.of(context).signal);
    expect(arrow, const Color(0xFF000000));
  });

  testWidgets("a target's accent fills Send, the arrow white on it", (
    tester,
  ) async {
    const pipe = Color(0xFF0E7C99);
    final (fill, arrow) = await pump(tester, accent: pipe);
    expect(fill, pipe);
    expect(arrow, const Color(0xFFFFFFFF));
  });
}
