// Needs you's title bar and its Waiting and
// History tabs fit a window 640 wide at the largest text (AX5).
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../extensions_test.dart' show workspace;
import '../harness.dart';
import 'as_ipad.dart';
import 'window_fixtures.dart' show TextFactor;

void main() {
  for (final width in const [600.0, 640.0, 680.0]) {
    testWidgets('at the largest text in a window $width wide, Needs you '
        'and its tabs fit', (tester) async {
      asIPad();
      tester.platformDispatcher.textScaleFactorTestValue = TextFactor.ax5;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = Harness(workspace());
      await harness.show(tester, size: Size(width, 1000));
      await settle(tester);
      harness.page.value = AppPage.inbox;
      await pumpFrames(tester);
      await settle(tester);
      expect(tester.takeException(), isNull);
      expect(find.text('Done').hitTestable(), findsOneWidget);
      expect(find.text('History').hitTestable(), findsOneWidget);

      await tester.tap(find.text('History'));
      await settle(tester);
      expect(tester.takeException(), isNull);
      expect(find.text('Waiting').hitTestable(), findsOneWidget);
    });
  }
}
