// A pane's header fits its pane at any width: on touch, where Expand and
// × are a finger's target each, the viewer's controls give way to them
// rather than the header overflowing (Sorbet's iPad sweep).
import 'package:factory_app/controls.dart';
import 'package:factory_app/panes.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  for (final width in [220.0, 300.0, 420.0, 600.0]) {
    testWidgets('a narrow pane header with controls fits on touch ($width)', (
      tester,
    ) async {
      tester.view.physicalSize = const Size(1180, 820);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      Widget control(String icon) => GlyphButton(
        label: icon,
        active: false,
        onTap: () {},
        glyph: (color) => ExtensionGlyph(icon: icon, name: icon, color: color),
      );
      await tester.pumpWidget(
        Directionality(
          textDirection: TextDirection.ltr,
          child: Overlay(
            initialEntries: [
              OverlayEntry(
                builder: (context) => Align(
                  alignment: Alignment.topLeft,
                  child: SizedBox(
                    width: width,
                    child: PaneHeader(
                      drag: const PaneDrag('a1', 'p1'),
                      glyph: (color) => const SizedBox(width: 14, height: 14),
                      title: 'needs-you-waiting-rows.md',
                      details: 'coordination/reports',
                      controls: [
                        control('refresh-cw'),
                        control('message-square'),
                        control('search'),
                      ],
                      focused: true,
                      onExpand: () {},
                      onClose: () {},
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      );
      await tester.pump();
      expect(tester.takeException(), isNull);
      // Expand and × stay whole, inside the pane.
      for (final label in ['Expand needs-you-waiting-rows.md', 'Close']) {
        final button = find.bySemanticsLabel(label);
        expect(button, findsOneWidget);
        expect(tester.getRect(button).right, lessThanOrEqualTo(width + 0.5));
      }
      // The controls give way before the title does: its start stays in
      // sight (Abbacchio's rows 521 and 545).
      expect(
        tester.getSize(find.text('needs-you-waiting-rows.md')).width,
        greaterThanOrEqualTo(60),
      );
    }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));
  }
  // A Mac's pane of about 255 px (Abbacchio's row 521).
  for (final width in [200.0, 255.0]) {
    testWidgets('a narrow pane keeps its title before its controls ($width)', (
      tester,
    ) async {
      tester.view.physicalSize = const Size(1180, 820);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      Widget control(String icon) => GlyphButton(
        label: icon,
        active: false,
        onTap: () {},
        glyph: (color) => ExtensionGlyph(icon: icon, name: icon, color: color),
      );
      await tester.pumpWidget(
        Directionality(
          textDirection: TextDirection.ltr,
          child: Overlay(
            initialEntries: [
              OverlayEntry(
                builder: (context) => Align(
                  alignment: Alignment.topLeft,
                  child: SizedBox(
                    width: width,
                    child: PaneHeader(
                      drag: const PaneDrag('a1', 'p1'),
                      glyph: (color) => const SizedBox(width: 14, height: 14),
                      title: 'needs-you-waiting-rows.md',
                      controls: [
                        for (final icon in ['message-square', 'refresh-cw'])
                          control(icon),
                        control('search'),
                        control('search'),
                      ],
                      onExpand: () {},
                      onClose: () {},
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      );
      await tester.pump();
      expect(tester.takeException(), isNull);
      expect(
        tester.getSize(find.text('needs-you-waiting-rows.md')).width,
        greaterThanOrEqualTo(60),
      );
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }
}
