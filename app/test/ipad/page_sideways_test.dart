// A page pane squeezed below 320 on an iPad (PageSideways in panes.dart)
// scrolls sideways under a finger or a Pencil wherever the drag starts:
// on its text, on blank space or on a button. Its selectable text doesn't
// take the drag (on iOS a SelectableRegion's touch drag waits for every
// other gesture to lose), touch and hold still selects, and the page's
// own list still scrolls up and down.
//
// Run as Android (the tests' default platform), the first test fails: off
// iOS the region's touch drag claims victory at the touch slop, ahead of
// the scroll view around it. The iPad workbench itself never squeezes a
// page below 320 (it hides the panes that don't fit); this guards what the
// pane relies on should an iPad ever lay one out narrower.
import 'package:factory_app/panes.dart';
import 'package:factory_app/settings_view.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show beside, snapshot;
import 'as_ipad.dart';

void main() {
  /// Settings as a pane narrower than a page's least beside the
  /// conversation, as the B6 pass had it, on an iPad: a window too narrow
  /// for both their leasts (with room, a page gets its least).
  Future<Finder> show(WidgetTester tester) async {
    asIPad();
    final harness = Harness(snapshot(layout: beside('settings', '')));
    harness.backend.appliesLayouts = true;
    await harness.show(tester, size: const Size(925, 900));
    await settle(tester);
    final pane = find.byType(PageSideways);
    expect(tester.getSize(pane).width, lessThan(pagePaneMinWidth - 40));
    return pane;
  }

  ScrollPosition sideways(WidgetTester tester, Finder pane) => tester
      .state<ScrollableState>(
        find.descendant(of: pane, matching: find.byType(Scrollable)).first,
      )
      .position;

  ScrollPosition list(WidgetTester tester) => tester
      .state<ScrollableState>(
        find
            .descendant(
              of: find.byType(SettingsView),
              matching: find.byType(Scrollable),
            )
            .first,
      )
      .position;

  /// A drag of 100 to the left in steps, as a finger moves.
  Future<void> dragLeft(
    WidgetTester tester,
    Offset at, {
    PointerDeviceKind kind = PointerDeviceKind.touch,
  }) async {
    final gesture = await tester.startGesture(at, kind: kind);
    for (var i = 0; i < 10; i++) {
      await gesture.moveBy(const Offset(-10, 0));
      await tester.pump(const Duration(milliseconds: 16));
    }
    await gesture.up();
    await pumpFrames(tester, const Duration(seconds: 2));
  }

  Finder text(String words) => find.text(words, findRichText: true).first;

  testWidgets('a finger dragging left scrolls it sideways, starting on text, '
      'on blank space or on a button', (tester) async {
    final pane = await show(tester);
    final position = sideways(tester, pane);
    expect(
      position.maxScrollExtent,
      pagePaneMinWidth - tester.getSize(pane).width,
    );
    final rect = tester.getRect(pane);
    final starts = {
      'text': tester.getCenter(text('Your name')),
      'blank': Offset(rect.right - 8, rect.bottom - 16),
      'button': tester.getCenter(
        find.descendant(of: pane, matching: find.byType(GestureDetector)).first,
      ),
    };
    for (final MapEntry(key: name, value: at) in starts.entries) {
      position.jumpTo(0);
      await tester.pump();
      await dragLeft(tester, at);
      expect(
        position.pixels,
        position.maxScrollExtent,
        reason: 'a drag starting on $name',
      );
    }
    // A Pencil's drag too.
    position.jumpTo(0);
    await tester.pump();
    await dragLeft(tester, starts['text']!, kind: PointerDeviceKind.stylus);
    expect(position.pixels, position.maxScrollExtent, reason: 'a Pencil drag');
  });

  testWidgets('touch and hold, then dragging, still selects the text and '
      'leaves the pane where it was', (tester) async {
    final pane = await show(tester);
    final position = sideways(tester, pane);
    final words = text('Your agents call you by it.');
    final paragraph = tester.renderObject<RenderParagraph>(words);
    expect(paragraph.selections, isEmpty);

    final start = tester.getTopLeft(words) + const Offset(4, 8);
    final gesture = await tester.startGesture(start);
    await tester.pump(const Duration(milliseconds: 600));
    await gesture.moveBy(const Offset(30, 20));
    await tester.pump(const Duration(milliseconds: 16));
    await gesture.moveBy(const Offset(10, 10));
    await tester.pump(const Duration(milliseconds: 16));
    await gesture.up();
    await pumpFrames(tester);
    expect(paragraph.selections, isNotEmpty);
    expect(position.pixels, 0);
  });

  testWidgets('a finger dragging up scrolls Settings down, and the pane '
      'stays put sideways', (tester) async {
    final pane = await show(tester);
    final position = sideways(tester, pane);
    final down = list(tester);
    expect(down.pixels, 0);
    final gesture = await tester.startGesture(tester.getCenter(pane));
    for (var i = 0; i < 10; i++) {
      await gesture.moveBy(const Offset(0, -20));
      await tester.pump(const Duration(milliseconds: 16));
    }
    await gesture.up();
    await pumpFrames(tester);
    expect(down.pixels, greaterThan(100));
    expect(position.pixels, 0);
    expect(tester.state<PageSidewaysState>(pane).fadesRight, isTrue);
    expect(tester.state<PageSidewaysState>(pane).fadesLeft, isFalse);
  });
}
