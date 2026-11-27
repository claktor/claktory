// A HoverCard's card stays inside the screen's safe area: held on a
// footer circle past the middle of a phone, it's shifted in from the
// right edge rather than cut off; with no room above, it goes below; it
// never covers the status bar or the home indicator; and where there's
// room it's where it always was, left-aligned above its child.
import 'package:factory_app/controls.dart';
import 'package:factory_app/identity.dart' show Space;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' show Workspace;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _screen = Size(430, 932);
const _safe = EdgeInsets.only(top: 59, bottom: 34);

/// A 24pt circle at [at], whose card is a long line held to show it.
Widget _circle(Offset at, {bool beside = false, bool end = false}) => Stack(
  children: [
    Positioned(
      left: at.dx,
      top: at.dy,
      child: HoverCard(
        holdToShow: true,
        beside: beside,
        end: end,
        card: (context) =>
            const SizedBox(key: ValueKey('card'), width: 280, height: 90),
        child: const SizedBox(key: ValueKey('circle'), width: 24, height: 24),
      ),
    ),
  ],
);

Future<Rect> _hold(WidgetTester tester, Widget home) async {
  tester.view.padding = FakeViewPadding(
    top: _safe.top * tester.view.devicePixelRatio,
    bottom: _safe.bottom * tester.view.devicePixelRatio,
  );
  addTearDown(tester.view.resetPadding);
  await Harness(Workspace()).show(tester, size: _screen, home: home);
  final gesture = await tester.startGesture(
    tester.getCenter(find.byKey(const ValueKey('circle'))),
  );
  await tester.pump(const Duration(milliseconds: 600));
  await tester.pumpAndSettle();
  final card = find.byKey(const ValueKey('card'));
  expect(card, findsOneWidget);
  // The card's box, its padding and border round the content.
  final rect = tester.getRect(
    find.ancestor(of: card, matching: find.byType(Container)).first,
  );
  await gesture.up();
  await tester.pump();
  return rect;
}

void expectInside(Rect card) {
  final safe = Rect.fromLTRB(
    0,
    _safe.top,
    _screen.width,
    _screen.height - _safe.bottom,
  );
  expect(card.left, greaterThanOrEqualTo(safe.left));
  expect(card.right, lessThanOrEqualTo(safe.right));
  expect(card.top, greaterThanOrEqualTo(safe.top));
  expect(card.bottom, lessThanOrEqualTo(safe.bottom));
}

void main() {
  testWidgets('where there is room, left-aligned just above its child', (
    tester,
  ) async {
    final card = await _hold(tester, _circle(const Offset(40, 800)));
    expect(card.left, 40);
    expect(card.bottom, lessThan(800));
    expect(card.bottom, greaterThan(800 - Space.m));
    expectInside(card);
  });

  testWidgets('past the middle, shifted in from the right edge, not cut off', (
    tester,
  ) async {
    final card = await _hold(tester, _circle(const Offset(360, 800)));
    expectInside(card);
    expect(card.right, closeTo(_screen.width - Space.s, 0.5));
    expect(card.bottom, lessThan(800));
  });

  testWidgets('with no room above, below its child, clear of the status bar', (
    tester,
  ) async {
    final card = await _hold(tester, _circle(const Offset(200, 70)));
    expectInside(card);
    expect(card.top, greaterThan(70 + 24));
  });

  testWidgets('beside, it goes left where there is no room at the right', (
    tester,
  ) async {
    final card = await _hold(
      tester,
      _circle(const Offset(380, 400), beside: true),
    );
    expectInside(card);
    expect(card.right, lessThanOrEqualTo(380));
  });
}
