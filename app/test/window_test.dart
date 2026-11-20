import 'package:factory_app/controls.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'focus_test.dart' show focusedOn;
import 'harness.dart';

void main() {
  /// What the app asked of the native window.
  late List<String> calls;

  setUp(() {
    calls = [];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('factory/window'), (
          call,
        ) async {
          // Whether the window takes dropped files isn't moving it.
          if (call.method != 'takeFiles') calls.add(call.method);
          return null;
        });
  });

  Future<Harness> showFocused(WidgetTester tester) async {
    final harness = Harness(focusedOn('p1'));
    await harness.show(tester);
    await settle(tester);
    return harness;
  }

  testWidgets('header bars sit at the top of the window', (tester) async {
    final harness = await showFocused(tester);
    // The sidebar's band, the extension bar's and the agent's header.
    final bands = find.byType(WindowDrag);
    expect(bands, findsNWidgets(3));
    for (var i = 0; i < 3; i++) {
      expect(tester.getTopLeft(bands.at(i)).dy, 0);
    }
    expect(tester.getTopLeft(find.text('Builder').last).dy, lessThan(44));

    await harness.apply(tester, focusedOn(''));
    for (final page in [AppPage.settings, AppPage.marketplace]) {
      harness.page.value = page;
      await settle(tester);
      expect(
        tester.getTopLeft(find.text('Done')).dy,
        lessThan(titleBarHeight),
        reason: '$page starts with its header',
      );
    }
  });

  testWidgets('the update banner takes the top, the header follows', (
    tester,
  ) async {
    final harness = Harness(
      focusedOn('p1', version: '2026.11.18-1600', latest: '2026.11.18-1700'),
    );
    await harness.show(tester);
    await settle(tester);
    final banner = tester.getTopLeft(
      find.text('Claktory 2026.11.18-1700 is ready.'),
    );
    expect(banner.dy, lessThan(titleBarHeight));
    expect(
      tester.getTopLeft(find.text('Builder').last).dy,
      greaterThan(titleBarHeight),
    );
  });

  testWidgets('dragging the header moves the window, a click opens settings', (
    tester,
  ) async {
    await showFocused(tester);
    final header = find.text('Builder').last;

    await tester.drag(header, const Offset(80, 30));
    await settle(tester);
    expect(calls, ['drag']);
    expect(find.text('Prompt'), findsNothing, reason: 'a drag is not a click');

    calls.clear();
    await tester.tap(header);
    await settle(tester);
    expect(calls, isEmpty);
    expect(find.text('Prompt'), findsOneWidget);
  });

  testWidgets('double-clicking an empty band zooms, a control does not', (
    tester,
  ) async {
    final harness = await showFocused(tester);
    // The sidebar's band, right of the traffic lights.
    const band = Offset(150, 20);
    await tester.tapAt(band);
    await tester.pump(const Duration(milliseconds: 100));
    await tester.tapAt(band);
    await settle(tester);
    expect(calls, ['zoom']);

    // The update banner's Update, in the content's band.
    await harness.apply(
      tester,
      focusedOn('p1', version: '2026.11.18-1600', latest: '2026.11.18-1700'),
    );
    await settle(tester);
    calls.clear();
    final update = tester.getCenter(find.text('Update'));
    await tester.tapAt(update);
    await tester.pump(const Duration(milliseconds: 100));
    await tester.tapAt(update);
    await settle(tester);
    expect(calls, isEmpty);
  });
}
