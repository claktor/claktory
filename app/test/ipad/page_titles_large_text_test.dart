// Pages' title bars at an iPad's largest text in a narrow window (Sorbet's
// S5 sweep): What's new, Usage and Marketplace cut their titles short
// rather than spill past the window, Done kept at the end.
import 'package:factory_app/scope.dart' show AppPage;
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../messaging_test.dart' as messaging show workspace;
import 'as_ipad.dart';
import 'window_fixtures.dart' show TextFactor;

void main() {
  for (final (page, title) in [
    (AppPage.changelog, "What's new"),
    (AppPage.usage, 'Usage'),
    (AppPage.marketplace, 'Marketplace'),
  ]) {
    for (final width in [600.0, 640.0, 680.0]) {
      testWidgets('$title at AX5, $width wide: the title fits its bar, '
          'Done in reach', (tester) async {
        asIPad();
        tester.platformDispatcher.textScaleFactorTestValue = TextFactor.ax5;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        final harness = Harness(messaging.workspace());
        await harness.show(tester, size: Size(width, 1000));
        harness.page.value = page;
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
        final heading = find.text(title);
        expect(heading, findsWidgets);
        final bar = tester.getRect(heading.first);
        expect(bar.right, lessThanOrEqualTo(width));
        final done = find.text('Done').hitTestable();
        expect(done, findsOneWidget);
        expect(tester.getRect(done).right, lessThanOrEqualTo(width));
        // Done at the bar's end, not pushed in by the title's room.
        expect(tester.getRect(done).right, greaterThan(width - 60));
      });
    }

    testWidgets('$title at the default text on a Mac: the whole title, Done '
        'at the end', (tester) async {
      final harness = Harness(messaging.workspace());
      await harness.show(tester, size: const Size(1200, 800));
      harness.page.value = page;
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      final heading = tester.renderObject<RenderParagraph>(
        find.text(title).first,
      );
      expect(heading.didExceedMaxLines, isFalse);
      expect(tester.getRect(find.text('Done')).right, greaterThan(1200 - 60));
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }
}
