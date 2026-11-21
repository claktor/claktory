// The iPad's workbench seam (docs/ipad-workbench.md §3, hook 2): an iPad
// window wide enough for panes lays out through the Workbench; an iPhone
// and a Mac don't; and the window's focus outlives the window narrowing
// to the phone's routes.
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/mobile/device.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/shell.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_test.dart' show workspace;

/// The window as main.dart builds it: the focus above the Shell, which has
/// a rail.
Widget _window(WorkbenchFocus focus) => WorkbenchScope(
  focus: focus,
  child: const Shell(rail: SizedBox(width: 56)),
);

void main() {
  void as(TargetPlatform platform, Idiom idiom) {
    debugDefaultTargetPlatformOverride = platform;
    debugDeviceCapabilities = DeviceCapabilities(idiom: idiom);
    addTearDown(() {
      debugDefaultTargetPlatformOverride = null;
      debugDeviceCapabilities = null;
    });
  }

  testWidgets('an iPad, wide: the workbench, in a focus that stays when '
      'the window narrows to the phone and widens again', (tester) async {
    as(TargetPlatform.iOS, Idiom.pad);
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    final harness = Harness(workspace());
    await harness.show(
      tester,
      size: const Size(1366, 1024),
      home: _window(focus),
    );
    await pumpFrames(tester);
    expect(find.byType(Workbench), findsOneWidget);

    await harness.show(
      tester,
      size: const Size(375, 1024),
      home: _window(focus),
    );
    await pumpFrames(tester);
    expect(find.byType(Workbench), findsNothing);
    expect(find.byType(PhoneShell), findsOneWidget);
    expect(
      WorkbenchScope.of(tester.element(find.byType(PhoneShell))),
      same(focus),
    );

    await harness.show(
      tester,
      size: const Size(1366, 1024),
      home: _window(focus),
    );
    await pumpFrames(tester);
    expect(find.byType(Workbench), findsOneWidget);
    expect(
      WorkbenchScope.of(tester.element(find.byType(Workbench))),
      same(focus),
    );
  });

  testWidgets('a Mac and an iPhone keep their own', (tester) async {
    as(TargetPlatform.macOS, Idiom.other);
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    final harness = Harness(workspace());
    await harness.show(
      tester,
      size: const Size(1366, 1024),
      home: _window(focus),
    );
    await pumpFrames(tester);
    expect(find.byType(Workbench), findsNothing);
    expect(find.byType(Shell), findsOneWidget);

    as(TargetPlatform.iOS, Idiom.phone);
    await harness.show(
      tester,
      size: const Size(932, 430),
      home: _window(focus),
    );
    await pumpFrames(tester);
    expect(find.byType(Workbench), findsNothing);
    expect(find.byType(PhoneShell), findsOneWidget);
  });
}
