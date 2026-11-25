// A picture or video pulled down closes on a phone, as in Photos
// (mobile/swipe_close.dart): one finger's vertical drag only; taps,
// horizontal drags and pinches are the child's.
import 'package:factory_app/mobile/swipe_close.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

const _size = Size(390, 844);
final _ios = TargetPlatformVariant.only(TargetPlatform.iOS);
final _mac = TargetPlatformVariant.only(TargetPlatform.macOS);

class _Harness {
  var closed = 0;
  var taps = 0;
  var horizontal = 0;
  var pinched = false;
  final progress = ValueNotifier(0.0);
}

Future<_Harness> _show(
  WidgetTester tester, {
  bool enabled = true,
  Widget? child,
  _Harness? harness,
  bool reduceMotion = false,
  bool accessible = false,
  Size size = _size,
}) async {
  final h = harness ?? _Harness();
  tester.view
    ..physicalSize = size * 3
    ..devicePixelRatio = 3;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    MediaQuery(
      data: MediaQueryData(
        size: size,
        disableAnimations: reduceMotion,
        accessibleNavigation: accessible,
      ),
      child: Directionality(
        textDirection: TextDirection.ltr,
        child: SwipeToClose(
          enabled: enabled,
          progress: h.progress,
          backdrop: const Color(0xFF000000),
          onClose: () => h.closed++,
          child:
              child ??
              GestureDetector(
                onTap: () => h.taps++,
                onHorizontalDragEnd: (_) => h.horizontal++,
                child: const ColoredBox(
                  color: Color(0xFF336699),
                  child: SizedBox.expand(),
                ),
              ),
        ),
      ),
    ),
  );
  return h;
}

/// Where the child is drawn now, by its box's top left on screen.
Offset _childAt(WidgetTester tester) =>
    tester.getTopLeft(find.byType(ColoredBox).last);

/// A slow drag, as a finger pulls: no fling at the end.
Future<void> _pull(WidgetTester tester, Offset by) async {
  final gesture = await tester.startGesture(const Offset(195, 400));
  const steps = 20;
  for (var i = 0; i < steps; i++) {
    await gesture.moveBy(by / steps.toDouble());
    await tester.pump(const Duration(milliseconds: 16));
  }
  // Held still before letting go.
  await tester.pump(const Duration(milliseconds: 200));
  await gesture.up();
}

void main() {
  testWidgets('a pull past a fifth of the height closes, once', (tester) async {
    final h = await _show(tester);
    await _pull(tester, const Offset(0, 844 * 0.3));
    await tester.pumpAndSettle();
    expect(h.closed, 1);
  }, variant: _ios);

  testWidgets('a short pull springs back to its place', (tester) async {
    final h = await _show(tester);
    final rest = _childAt(tester);
    final gesture = await tester.startGesture(const Offset(195, 400));
    for (var i = 0; i < 10; i++) {
      await gesture.moveBy(const Offset(0, 8));
      await tester.pump(const Duration(milliseconds: 16));
    }
    // Following the finger meanwhile, shrunk a little.
    expect(_childAt(tester).dy, greaterThan(rest.dy + 40));
    await tester.pump(const Duration(milliseconds: 200));
    await gesture.up();
    await tester.pumpAndSettle();
    expect(h.closed, 0);
    expect(_childAt(tester), rest);
    expect(h.progress.value, 0);
  }, variant: _ios);

  testWidgets('a quick flick down closes, however short', (tester) async {
    final h = await _show(tester);
    await tester.flingFrom(const Offset(195, 400), const Offset(0, 80), 1500);
    await tester.pumpAndSettle();
    expect(h.closed, 1);
  }, variant: _ios);

  testWidgets('pulled up, it follows a little and never closes', (
    tester,
  ) async {
    final h = await _show(tester);
    await tester.flingFrom(const Offset(195, 600), const Offset(0, -300), 1500);
    await tester.pumpAndSettle();
    expect(h.closed, 0);
    expect(_childAt(tester), Offset.zero);
  }, variant: _ios);

  testWidgets('a tap is the child\'s', (tester) async {
    final h = await _show(tester);
    await tester.tapAt(const Offset(195, 400));
    await tester.pumpAndSettle();
    expect(h.taps, 1);
    expect(h.closed, 0);
  }, variant: _ios);

  testWidgets('a horizontal drag is the child\'s', (tester) async {
    final h = await _show(tester);
    await _pull(tester, const Offset(-200, 20));
    await tester.pumpAndSettle();
    expect(h.horizontal, 1);
    expect(h.closed, 0);
  }, variant: _ios);

  testWidgets('two fingers are a pinch, the child\'s: no pull', (tester) async {
    final h = _Harness();
    var scaled = 1.0;
    await _show(
      tester,
      harness: h,
      child: GestureDetector(
        onScaleUpdate: (details) {
          if (details.pointerCount == 2) {
            h.pinched = true;
            scaled = details.scale;
          }
        },
        child: const ColoredBox(
          color: Color(0xFF336699),
          child: SizedBox.expand(),
        ),
      ),
    );
    final a = await tester.startGesture(const Offset(195, 300), pointer: 1);
    final b = await tester.startGesture(const Offset(195, 500), pointer: 2);
    for (var i = 0; i < 10; i++) {
      await a.moveBy(const Offset(0, -10));
      await b.moveBy(const Offset(0, 20));
      await tester.pump(const Duration(milliseconds: 16));
    }
    await a.up();
    await b.up();
    await tester.pumpAndSettle();
    expect(h.pinched, isTrue);
    expect(scaled, greaterThan(1.2));
    expect(h.closed, 0);
    expect(h.progress.value, 0);
  }, variant: _ios);

  testWidgets('a second finger landing mid-pull gives the pull up: it '
      'springs back, and letting go doesn\'t close', (tester) async {
    final h = await _show(tester);
    final a = await tester.startGesture(const Offset(195, 300), pointer: 1);
    for (var i = 0; i < 20; i++) {
      await a.moveBy(const Offset(0, 15));
      await tester.pump(const Duration(milliseconds: 16));
    }
    expect(h.progress.value, greaterThan(0.5));
    final b = await tester.startGesture(const Offset(100, 500), pointer: 2);
    await tester.pumpAndSettle();
    expect(h.progress.value, 0);
    await a.moveBy(const Offset(0, 100));
    await tester.pump();
    await a.up();
    await b.up();
    await tester.pumpAndSettle();
    expect(h.closed, 0);
    expect(_childAt(tester), Offset.zero);
  }, variant: _ios);

  testWidgets('turned off, it lets everything through and moves nothing', (
    tester,
  ) async {
    final h = await _show(tester, enabled: false);
    await _pull(tester, const Offset(0, 400));
    await tester.flingFrom(const Offset(195, 400), const Offset(0, 80), 1500);
    await tester.tapAt(const Offset(195, 400));
    await tester.pumpAndSettle();
    expect(h.closed, 0);
    expect(h.taps, 1);
    expect(h.progress.value, 0);
  }, variant: _ios);

  testWidgets('with VoiceOver or TalkBack, nothing: Back is there', (
    tester,
  ) async {
    final h = await _show(tester, accessible: true);
    await _pull(tester, const Offset(0, 400));
    await tester.pumpAndSettle();
    expect(h.closed, 0);
  }, variant: _ios);

  testWidgets('outside a phone\'s layout (a Mac), nothing', (tester) async {
    final h = await _show(tester, size: const Size(1200, 800));
    await _pull(tester, const Offset(0, 400));
    await tester.pumpAndSettle();
    expect(h.closed, 0);
  }, variant: _mac);

  testWidgets('progress follows the pull, and the backdrop fades with it', (
    tester,
  ) async {
    final h = await _show(tester);
    final seen = <double>[];
    h.progress.addListener(() => seen.add(h.progress.value));
    final gesture = await tester.startGesture(const Offset(195, 300));
    for (var i = 0; i < 20; i++) {
      await gesture.moveBy(const Offset(0, 10));
      await tester.pump(const Duration(milliseconds: 16));
    }
    // 200 points of a close at 0.2 × 844 = 169: past it, more than half.
    expect(h.progress.value, closeTo(200 / (844 * 0.4), 0.06));
    // Rising as it's pulled.
    for (var i = 1; i < seen.length; i++) {
      expect(seen[i], greaterThanOrEqualTo(seen[i - 1]));
    }
    final backdrop = tester.widget<ColoredBox>(find.byType(ColoredBox).first);
    expect(backdrop.color.a, closeTo(1 - h.progress.value, 0.01));
    // Smaller, but never under 0.85.
    final box = find.byType(ColoredBox).last;
    expect(
      tester.getTopRight(box).dx - tester.getTopLeft(box).dx,
      inInclusiveRange(390 * 0.85, 390 * 0.99),
    );
    await gesture.up();
    await tester.pumpAndSettle();
  }, variant: _ios);

  testWidgets('with reduced motion it fades, never moving or shrinking', (
    tester,
  ) async {
    final h = await _show(tester, reduceMotion: true);
    final gesture = await tester.startGesture(const Offset(195, 300));
    for (var i = 0; i < 10; i++) {
      await gesture.moveBy(const Offset(0, 10));
      await tester.pump(const Duration(milliseconds: 16));
    }
    expect(h.progress.value, greaterThan(0));
    expect(_childAt(tester), Offset.zero);
    expect(tester.getSize(find.byType(ColoredBox).last), _size);
    expect(find.byType(Opacity), findsOneWidget);
    await tester.pump(const Duration(milliseconds: 200));
    await gesture.up();
    await tester.pump();
    // Back at once, no spring.
    expect(h.progress.value, 0);
    expect(h.closed, 0);
  }, variant: _ios);

  testWidgets('a mouse drag on a phone-sized window doesn\'t pull', (
    tester,
  ) async {
    final h = await _show(tester);
    final gesture = await tester.startGesture(
      const Offset(195, 300),
      kind: PointerDeviceKind.mouse,
    );
    await gesture.moveBy(const Offset(0, 400));
    await gesture.up();
    await tester.pumpAndSettle();
    expect(h.closed, 0);
  }, variant: _ios);
}
