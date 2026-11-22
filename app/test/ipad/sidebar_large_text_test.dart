// The sidebar at an iPad's largest text (Illuso's find, a 1024×768 iPad
// on its side at 3.1x): with a workspace
// that has more in it and a short room, its top rows and headings alone
// are taller than the room. It then scrolls as one list, its last heading
// a scroll away, rather than spilling; with room, it keeps its sections.
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../messaging_test.dart' as messaging show workspace;
import 'as_ipad.dart';
import 'window_fixtures.dart' show TextFactor;

/// The sidebar alone, [height] tall, in an iPad window of [window].
Future<void> _show(
  WidgetTester tester,
  Size window, {
  required double text,
  required double height,
}) async {
  asIPad();
  tester.platformDispatcher.textScaleFactorTestValue = text;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  await Harness(messaging.workspace()).show(
    tester,
    size: window,
    home: Align(
      alignment: Alignment.topLeft,
      child: SizedBox(height: height, child: const Sidebar(width: 248)),
    ),
  );
  await pumpFrames(tester);
}

Finder get _oneList => find.byKey(const ValueKey('sidebar-one-list'));

Finder get _chats =>
    find.descendant(of: find.byType(Sidebar), matching: find.text('Chats'));

void main() {
  for (final window in const [Size(1366, 1024), Size(1024, 1366)]) {
    testWidgets('at the largest text in a $window window, a short sidebar '
        'scrolls as one list and Chats is a scroll away', (tester) async {
      // Its sections get 331pt, as in Illuso's run.
      await _show(tester, window, text: TextFactor.ax5, height: 772);
      expect(tester.takeException(), isNull);
      expect(_oneList, findsOneWidget);
      await tester.dragUntilVisible(_chats, _oneList, const Offset(0, -120));
      expect(_chats.hitTestable(), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets('at the largest text in a $window window, a tall sidebar '
        'keeps its sections', (tester) async {
      await _show(tester, window, text: TextFactor.ax5, height: window.height);
      expect(tester.takeException(), isNull);
      expect(_oneList, findsNothing);
      expect(_chats.hitTestable(), findsOneWidget);
    });
  }

  testWidgets('Illuso\'s run: a 1024×768 iPad on its side at 3.1x, the '
      'whole app', (tester) async {
    asIPad();
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await Harness(messaging.workspace())
        .show(tester, size: const Size(1024, 768));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    if (_oneList.evaluate().isNotEmpty) {
      await tester.dragUntilVisible(_chats, _oneList, const Offset(0, -120));
    }
    expect(_chats.hitTestable(), findsOneWidget);
  });

  testWidgets('at the default text the sections stay as they are, short or '
      'tall', (tester) async {
    for (final height in [500.0, 1024.0]) {
      await _show(tester, const Size(1366, 1024), text: 1, height: height);
      expect(tester.takeException(), isNull);
      expect(_oneList, findsNothing);
    }
  });
}
