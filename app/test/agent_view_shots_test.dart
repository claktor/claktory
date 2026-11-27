// Pictures of the agent's header, its model popover, its menu and its
// panel over the conversation, to check against the
// mock-ups. With SHOTS set they're drawn to PNGs there:
//   SHOTS=/tmp/agent-view flutter test test/agent_view_shots_test.dart
// Without it they only check that each scene lays out without an error.
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'agent_menu_test.dart' show header, said;
import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'phone_sizes_test.dart' show loadGeist;

Future<void> _shot(WidgetTester tester, String name) async {
  expect(tester.takeException(), isNull, reason: name);
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

void main() {
  setUpAll(loadGeist);
  setUp(() {
    final dispatcher = TestWidgetsFlutterBinding.instance.platformDispatcher;
    dispatcher.platformBrightnessTestValue = Brightness.dark;
    addTearDown(dispatcher.clearPlatformBrightnessTestValue);
    // No DEBUG ribbon over ⋯.
    WidgetsApp.debugAllowBannerOverride = false;
    addTearDown(() => WidgetsApp.debugAllowBannerOverride = true);
  });

  testWidgets('a Mac', (tester) async {
    final harness = Harness(
      header(checklistOpen: 2, tasksRunning: 2, branches: 2),
    );
    await harness.show(tester, size: const Size(1400, 640));
    await settle(tester);
    await _shot(tester, 'mac-1-header');

    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    addTearDown(mouse.removePointer);
    await mouse.moveTo(tester.getCenter(find.text('212k / 400k')));
    await tester.pump(const Duration(seconds: 1));
    await settle(tester);
    await _shot(tester, 'mac-2-meter');
    await mouse.moveTo(Offset.zero);
    await settle(tester);

    await tester.tap(said('Model: Claude Opus 4.8 · High'));
    await settle(tester);
    await _shot(tester, 'mac-3-model-popover');
    await tester.tap(find.text('High').last);
    await settle(tester);
    await tester.tap(find.text('Low').last);
    await settle(tester);
    await harness.apply(
      tester,
      header(checklistOpen: 2, tasksRunning: 2, branches: 2)
        ..agents.first.effort = 'low',
    );
    await tester.tapAt(const Offset(700, 500));
    await settle(tester);
    await _shot(tester, 'mac-4-undo');

    await harness.apply(
      tester,
      header(
        checklistOpen: 2,
        tasksRunning: 2,
        branches: 2,
        openPanel: 'checklist',
      ),
    );
    await tester.tap(said('More options for June'));
    await settle(tester);
    await _shot(tester, 'mac-5-menu');
  });

  testWidgets('a narrow Mac window', (tester) async {
    final harness = Harness(header(checklistOpen: 2, tasksRunning: 2));
    await harness.show(tester, size: const Size(900, 640));
    await settle(tester);
    await _shot(tester, 'mac-6-narrow');
    await harness.apply(
      tester,
      header(checklistOpen: 2, tasksRunning: 2, openPanel: 'checklist'),
    );
    await _shot(tester, 'mac-7-panel-over');
  });

  // The header's right-hand group ends at the conversation's right edge,
  // not at the messages' width.
  testWidgets('a wide Mac window', (tester) async {
    final harness = Harness(header(checklistOpen: 2, tasksRunning: 2));
    await harness.show(tester, size: const Size(1900, 900));
    await settle(tester);
    await _shot(tester, 'mac-8-wide');
    await harness.apply(
      tester,
      header(checklistOpen: 2, tasksRunning: 2, openPanel: 'checklist'),
    );
    await settle(tester);
    await _shot(tester, 'mac-9-wide-panel');
  });

  testWidgets("a 13-inch iPad's full screen", (tester) async {
    asIPad();
    final harness = Harness(header(checklistOpen: 2));
    await harness.show(tester, size: const Size(1376, 1032));
    await settle(tester);
    await _shot(tester, 'ipad-4-13in');
    await harness.apply(
      tester,
      header(checklistOpen: 2, openPanel: 'checklist'),
    );
    await settle(tester);
    await _shot(tester, 'ipad-5-13in-panel');
  });

  testWidgets('an iPad', (tester) async {
    asIPad();
    final harness = Harness(header(checklistOpen: 2));
    await harness.show(tester, size: const Size(1366, 1024));
    await settle(tester);
    await _shot(tester, 'ipad-1-header');
    await tester.tap(said('Model: Claude Opus 4.8 · High'));
    await settle(tester);
    await tester.pump(const Duration(seconds: 1));
    await _shot(tester, 'ipad-2-model-popover');
    await tester.tapAt(const Offset(1300, 1000));
    await settle(tester);
    await tester.tap(said('More options for June'));
    await settle(tester);
    await tester.pump(const Duration(seconds: 1));
    await _shot(tester, 'ipad-3-menu');
  });
}
