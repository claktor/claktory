// The iPad's window around the workbench (Prosciutto's no-go on the
// phase-1 pass): the toolbar and rail clear of the status bar or a window's
// controls, the message box above the on-screen keyboard, the keyboard not
// raised by opening an agent, and the device called an iPad.
import 'package:factory_app/devices.dart' show thisDeviceKind;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/message_box.dart' show MessageBox;
import 'package:factory_app/mobile/device.dart';
import 'package:factory_app/shell.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../phone_test.dart' show workspace;
import 'as_ipad.dart';

Widget _window(WorkbenchFocus focus) => WorkbenchScope(
  focus: focus,
  // A column of buttons as tall as the real rail's.
  child: Shell(
    rail: SizedBox(
      width: 56,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [for (var i = 0; i < 10; i++) const SizedBox(height: 44)],
      ),
    ),
  ),
);

/// The window at [size], with an iPad's status bar (32, or [top] under a
/// window's controls) and home indicator (20), and the on-screen keyboard
/// [keyboard] tall.
Future<void> _show(
  WidgetTester tester,
  Size size, {
  double keyboard = 0,
  double top = 32,
}) async {
  asIPad();
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  final harness = Harness(workspace());
  await harness.show(tester, size: size, home: _window(focus));
  tester.view.padding = FakeViewPadding(
    top: top,
    bottom: keyboard > 0 ? 0 : 20,
  );
  tester.view.viewPadding = FakeViewPadding(top: top, bottom: 20);
  tester.view.viewInsets = FakeViewPadding(bottom: keyboard);
  addTearDown(tester.view.resetPadding);
  addTearDown(tester.view.resetViewPadding);
  addTearDown(tester.view.resetViewInsets);
  await pumpFrames(tester);
  expect(find.byType(Workbench), findsOneWidget);
}

Rect _box(WidgetTester tester) =>
    tester.getRect(find.byType(EditableText).last);

void main() {
  for (final (size, keyboard, top) in [
    (const Size(744, 1133), 340.0, 32.0),
    (const Size(1133, 744), 428.0, 32.0),
    // A window in landscape on the iPad mini, under its controls: what the
    // keyboard leaves is shorter than the rail's column.
    (const Size(888, 744), 428.0, 75.0),
  ]) {
    testWidgets('${size.width.round()}×${size.height.round()}: the message '
        'box stays above a ${keyboard.round()}pt keyboard, the window\'s '
        'controls below the status bar', (tester) async {
      await _show(tester, size, keyboard: keyboard, top: top);
      expect(tester.takeException(), isNull);
      expect(_box(tester).bottom, lessThanOrEqualTo(size.height - keyboard));
      // Right on the keyboard: the home indicator's inset isn't counted
      // under it.
      expect(
        tester.getRect(find.byType(MessageBox).last).bottom,
        size.height - keyboard,
      );
      final sidebar = tester.getRect(
        find.byKey(const ValueKey('workbench-sidebar')),
      );
      expect(sidebar.top, greaterThanOrEqualTo(top));
    });

    testWidgets('${size.width.round()}×${size.height.round()}: without the '
        'keyboard, the message box stays above the home indicator', (
      tester,
    ) async {
      await _show(tester, size);
      expect(_box(tester).bottom, lessThanOrEqualTo(size.height - 20));
    });
  }

  testWidgets('opening an agent on an iPad leaves the keyboard down', (
    tester,
  ) async {
    await _show(tester, const Size(1366, 1024));
    final focused = FocusManager.instance.primaryFocus?.context;
    expect(focused, isNotNull);
    expect(focused!.findAncestorWidgetOfExactType<EditableText>(), isNull);
  });

  for (final (size, wide) in [
    (const Size(744, 1133), false),
    (const Size(1133, 744), true),
  ]) {
    testWidgets('${size.width.round()}×${size.height.round()}: under a '
        'window\'s controls the ${wide ? 'rail' : 'toolbar'} starts at the '
        'status bar, its leading items clear of them', (tester) async {
      windowControls.value = const WindowControls(leading: 66, top: 43);
      addTearDown(() => windowControls.value = WindowControls.none);
      // The native side adds the controls to the view's top padding.
      await _show(tester, size, top: 75);
      expect(tester.takeException(), isNull);
      final sidebar = tester.getRect(
        find.byKey(const ValueKey('workbench-sidebar')),
      );
      if (wide) {
        // The rail's column: its buttons below the controls.
        expect(sidebar.top, greaterThanOrEqualTo(75));
      } else {
        // The toolbar at the status bar, Sidebar past the controls.
        expect(sidebar.top, lessThan(75));
        expect(sidebar.top, greaterThanOrEqualTo(32));
        expect(sidebar.left, greaterThanOrEqualTo(66));
      }
    });
  }

  testWidgets('the conversation runs under the home indicator, its message '
      'box above it', (tester) async {
    const size = Size(744, 1133);
    await _show(tester, size);
    final content = tester.getRect(find.byType(MessageBox).last);
    expect(_box(tester).bottom, lessThanOrEqualTo(size.height - 20));
    // Nothing of the window's own shows below the conversation: the
    // message box's area reaches the bottom edge.
    expect(content.bottom, size.height);
  });

  test('this device is an iPad on an iPad, an iPhone on an iPhone', () {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() {
      debugDefaultTargetPlatformOverride = null;
      debugDeviceCapabilities = null;
    });
    debugDeviceCapabilities = const DeviceCapabilities(idiom: Idiom.pad);
    expect(thisDeviceKind, 'iPad');
    debugDeviceCapabilities = const DeviceCapabilities(idiom: Idiom.phone);
    expect(thisDeviceKind, 'iPhone');
  });
}
