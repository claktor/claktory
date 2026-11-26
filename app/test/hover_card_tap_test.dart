// A hover card a finger can open (HoverCard.tapToShow; Lucy's agent view
// §3, the context meter on an iPad): a tap shows it, a tap on it again or
// anywhere else hides it, and a mouse still opens it by resting on it,
// its click leaving the card to hover.
import 'package:factory_app/controls.dart' show HoverCard;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart' show Harness;

Future<void> _show(WidgetTester tester) => Harness(Workspace()).show(
  tester,
  // Something under every finger, as the app's window has.
  home: Stack(
    children: [
      Positioned.fill(
        child: GestureDetector(behavior: HitTestBehavior.opaque, onTap: () {}),
      ),
      const Align(
        alignment: Alignment.center,
        child: HoverCard(
          tapToShow: true,
          card: _card,
          child: SizedBox(width: 120, height: 32, child: Text('212k / 400k')),
        ),
      ),
    ],
  ),
);

Widget _card(BuildContext context) => const Text('212k of 400k.');

Finder get _meter => find.text('212k / 400k');
Finder get _shown => find.text('212k of 400k.');

void main() {
  testWidgets('a tap shows it, another tap on it hides it', (tester) async {
    await _show(tester);
    expect(_shown, findsNothing);
    await tester.tap(_meter);
    await tester.pumpAndSettle();
    expect(_shown, findsOneWidget);
    await tester.tap(_meter);
    await tester.pumpAndSettle();
    expect(_shown, findsNothing);
  });

  testWidgets('a tap anywhere else hides it', (tester) async {
    await _show(tester);
    await tester.tap(_meter);
    await tester.pumpAndSettle();
    expect(_shown, findsOneWidget);
    await tester.tapAt(const Offset(20, 20));
    await tester.pumpAndSettle();
    expect(_shown, findsNothing);
  });

  testWidgets('a mouse opens it by resting on it; its click leaves it be, '
      'and moving away hides it', (tester) async {
    await _show(tester);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    addTearDown(mouse.removePointer);
    await mouse.addPointer(location: const Offset(1, 1));
    await mouse.moveTo(tester.getCenter(_meter));
    await tester.pump(const Duration(milliseconds: 400));
    await tester.pumpAndSettle();
    expect(_shown, findsOneWidget);
    // A click: still shown (not toggled away).
    await tester.tap(_meter, kind: PointerDeviceKind.mouse);
    await tester.pumpAndSettle();
    expect(_shown, findsOneWidget);
    await mouse.moveTo(const Offset(1, 1));
    await tester.pumpAndSettle();
    expect(_shown, findsNothing);
  });
}
