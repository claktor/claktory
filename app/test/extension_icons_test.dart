// Every extension's icon, as factoryd names it, and every icon Claktory's
// own apps name (their Icon components and their own icon), is one the app
// draws: one it doesn't shows as a bare letter (Sorbet: Comments showed
// "C"), or in an app's view as a dot (Scolippi: Charts' menus' chevrons).
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
    // Claktory's own apps: what their views name, and their own icon.
    for (final file
        in Directory('../apps')
            .listSync(recursive: true)
            .whereType<File>()
            .where(
              (file) =>
                  file.path.endsWith('.rs') && !file.path.contains('/target/'),
            ))
      for (final match in RegExp(
        r'"component":\s*"Icon",\s*"name":\s*"([a-z0-9-]+)"|"icon":\s*"([a-z0-9-]+)"',
      ).allMatches(file.readAsStringSync()))
        (match.group(1) ?? match.group(2))!,
    // The chevrons, every way.
    'chevron-left',
    'chevron-right',
    'chevron-up',
    'chevron-down',
  };

  test('factoryd and Claktory\'s apps name their icons', () {
    expect(icons, contains('message-square-text'));
    // Charts' menus.
    expect(icons, contains('chevron-down'));
    expect(icons, contains('chart-column'));
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
