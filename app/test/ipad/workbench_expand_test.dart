// Polnareff's Expand (#272) in the iPad's workbench: a pane fills the
// window and Back puts it back, in each tier.
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/panes.dart';
import 'package:factory_app/shell.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show beside, glyphButton, snapshot, text;
import 'as_ipad.dart';

void main() {
  for (final size in [const Size(1133, 744), const Size(744, 1133)]) {
    testWidgets('${size.width.round()}×${size.height.round()}: Expand fills '
        'the window with the pane, Back puts it back', (tester) async {
      asIPad();
      final focus = WorkbenchFocus();
      addTearDown(focus.dispose);
      final harness = Harness(snapshot(layout: beside()))
        ..backend.appliesLayouts = true;
      harness.backend.files['/w/main.rs'] = text(
        '/w/main.rs',
        'fn main() {}\n',
      );
      await harness.show(
        tester,
        size: size,
        home: WorkbenchScope(
          focus: focus,
          child: const Shell(rail: SizedBox(width: 56)),
        ),
      );
      await settle(tester);
      expect(find.byType(Workbench), findsOneWidget);
      if (glyphButton('Expand main.rs').evaluate().isEmpty) {
        // One pane at a time: the document first.
        await tester.tap(find.text('main.rs').first);
        await settle(tester);
      }
      await tester.tap(glyphButton('Expand main.rs'));
      await settle(tester);
      expect(find.byType(ExpandedPane), findsOneWidget);
      await tester.tap(find.text('Back to Builder'));
      await settle(tester);
      expect(find.byType(ExpandedPane), findsNothing);
    });
  }
}
