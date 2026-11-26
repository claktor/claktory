// What's new shows while the window is reported unseen (friend bugs #31
// and #40): nothing waits on an animation that can't run while
// [TickerMode] is off, and the pointer over the window counts as seen.
import 'package:factory_app/controls.dart';
import 'package:factory_app/identity.dart' show Motion;
import 'package:factory_app/sight.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

/// [child] in what the app's widgets need, with [moving] as its
/// [TickerMode].
Widget _frame(Widget child, {required bool moving}) => MediaQuery(
  data: const MediaQueryData(),
  child: Directionality(
    textDirection: TextDirection.ltr,
    child: Overlay(
      initialEntries: [
        OverlayEntry(
          builder: (_) => TickerMode(
            enabled: moving,
            child: Center(child: child),
          ),
        ),
      ],
    ),
  ),
);

/// [child] with [moving] as its [TickerMode], no overlay (one keeps its
/// first entries, so a frame pumped again wouldn't reach them).
Widget _plain(Widget child, {required bool moving}) => Directionality(
  textDirection: TextDirection.ltr,
  child: TickerMode(
    enabled: moving,
    child: Center(child: child),
  ),
);

/// The opacity [text] is painted at: every [Opacity] above it, multiplied.
double _opacity(WidgetTester tester, String text) {
  var opacity = 1.0;
  for (final element
      in find
          .ancestor(of: find.text(text), matching: find.byType(Opacity))
          .evaluate()) {
    opacity *= (element.widget as Opacity).opacity;
  }
  return opacity;
}

void main() {
  testWidgets('something new is there at once while nothing moves', (
    tester,
  ) async {
    await tester.pumpWidget(
      _plain(
        const Arrival(animate: true, child: Text('new entry')),
        moving: false,
      ),
    );
    expect(find.text('new entry'), findsOneWidget);
    expect(_opacity(tester, 'new entry'), 1);
    // Moving, it eases in as before.
    await tester.pumpWidget(
      _plain(
        const Arrival(
          key: ValueKey('second'),
          animate: true,
          child: Text('another'),
        ),
        moving: true,
      ),
    );
    expect(find.text('another'), findsOneWidget);
    expect(_opacity(tester, 'another'), 0);
    await tester.pump(Motion.base);
    expect(_opacity(tester, 'another'), 1);
  });

  testWidgets('stopped partway, as the window goes unseen, it is all there', (
    tester,
  ) async {
    Widget arrival(bool moving) => _plain(
      const Arrival(animate: true, child: Text('new entry')),
      moving: moving,
    );
    await tester.pumpWidget(arrival(true));
    await tester.pump(const Duration(milliseconds: 40));
    expect(_opacity(tester, 'new entry'), lessThan(1));
    await tester.pumpWidget(arrival(false));
    expect(_opacity(tester, 'new entry'), 1);
  });

  testWidgets('a hover card shows whole while nothing moves (friend bug '
      '#31)', (tester) async {
    await tester.pumpWidget(
      _frame(
        HoverCard(
          card: (_) => const Text('usage card'),
          child: const SizedBox(width: 80, height: 40, child: Text('usage')),
        ),
        moving: false,
      ),
    );
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    addTearDown(mouse.removePointer);
    await mouse.moveTo(tester.getCenter(find.text('usage')));
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('usage card'), findsOneWidget);
    expect(_opacity(tester, 'usage card'), 1);
  });

  testWidgets('the pointer over a window reported unseen makes it seen', (
    tester,
  ) async {
    addTearDown(Sight.debugReset);
    Sight.shown.value = false;
    await tester.pumpWidget(
      const MediaQuery(
        data: MediaQueryData(),
        child: SightScope(
          child: Directionality(
            textDirection: TextDirection.ltr,
            child: SizedBox.expand(child: Text('window')),
          ),
        ),
      ),
    );
    expect(
      TickerMode.valuesOf(tester.element(find.text('window'))).enabled,
      isFalse,
    );
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(10, 10));
    addTearDown(mouse.removePointer);
    await mouse.moveTo(const Offset(20, 20));
    await tester.pump();
    expect(Sight.shown.value, isTrue);
    expect(
      TickerMode.valuesOf(tester.element(find.text('window'))).enabled,
      isTrue,
    );
  });

  testWidgets('on Windows the runner tells sight and less motion too', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    addTearDown(Sight.debugReset);
    const channel = MethodChannel('factory/sight');
    final messenger = tester.binding.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(channel, (call) async {
      if (call.method == 'state') {
        return {'shown': true, 'reduceMotion': true};
      }
      return null;
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
    Sight.start();
    await tester.pump();
    expect(Sight.shown.value, isTrue);
    expect(Sight.reduceMotion.value, isTrue);
    // The session locked: nothing moves until it's back.
    await messenger.handlePlatformMessage(
      'factory/sight',
      const StandardMethodCodec().encodeMethodCall(
        const MethodCall('sight', {'shown': false, 'reduceMotion': true}),
      ),
      (_) {},
    );
    expect(Sight.shown.value, isFalse);
  });
}
