// Every extension's icon, as factoryd names it, is one the app draws: one
// it doesn't shows as a bare letter (Sorbet: Comments showed "C").
import 'dart:io';

import 'package:factory_app/controls.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  final icons = {
    for (final file
        in Directory('../crates/factoryd/src/extensions')
            .listSync(recursive: true)
            .whereType<File>()
            .where((file) => file.path.endsWith('.rs')))
      for (final match in RegExp(
        r'\bicon:\s*"([a-z0-9-]+)"',
      ).allMatches(file.readAsStringSync()))
        match.group(1)!,
  };

  test('factoryd names its extensions\' icons', () {
    expect(icons, contains('message-square-text'));
    expect(icons.length, greaterThan(10));
  });

  for (final icon in icons) {
    testWidgets('$icon is drawn, not a letter', (tester) async {
      await tester.pumpWidget(
        Directionality(
          textDirection: TextDirection.ltr,
          child: ExtensionGlyph(
            icon: icon,
            name: 'Zz',
            color: const Color(0xFF000000),
          ),
        ),
      );
      expect(find.text('Z'), findsNothing);
      expect(
        find.descendant(
          of: find.byType(ExtensionGlyph),
          matching: find.byType(CustomPaint),
        ),
        findsOneWidget,
      );
    });
  }
}
