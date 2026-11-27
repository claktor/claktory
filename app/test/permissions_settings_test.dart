// This device's permissions in Settings (Lucy's permissions spec §2.1):
// in its own section, a row per permission it can ask for, each with its
// state as the system says it now and at most one button; read again when
// the app comes back to the front.
import 'package:factory_app/backend.dart';
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/diagnostics_record.dart' show PushStep;
import 'package:factory_app/notification_settings.dart'
    show DeviceNotifications;
import 'package:factory_app/recorder.dart';
import 'package:factory_app/scanner.dart' show CameraAccess;
import 'package:factory_app/scope.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'voice_test.dart' show FakeRecorder;

/// A device's notifications as the test says (the shots use it too).
class FakeNotifications implements DeviceNotifications {
  FakeNotifications(this.state);

  String state;
  var opened = 0;

  @override
  String get name => 'This Mac';

  @override
  bool get pushes => false;

  @override
  Future<String> permission() async => state;

  @override
  Future<String> ask() async {
    if (state == 'not_determined') state = 'allowed';
    return state;
  }

  @override
  Future<void> openSettings() async => opened++;

  @override
  ValueListenable<Map<Backend, PushStep>>? get setupFailures => null;

  @override
  void retrySetup(Backend backend) {}
}

Finder _button(String label) =>
    find.byWidgetPredicate((w) => w is Button && w.label == label);

/// The row [label]'s words, in its row.
Finder _inRow(String label, Finder finder) => find.descendant(
  of: find.byKey(ValueKey('permission:$label')),
  matching: finder,
);

void main() {
  late FakeRecorder recorder;
  late VoiceRecorder was;
  setUp(() {
    was = deviceRecorder;
    recorder = FakeRecorder();
    deviceRecorder = recorder;
  });
  tearDown(() {
    deviceRecorder = was;
    DeviceNotifications.debugOverride = null;
  });

  Future<Harness> show(
    WidgetTester tester, {
    required TargetPlatform platform,
    required Size size,
    String notifications = 'allowed',
  }) async {
    debugDefaultTargetPlatformOverride = platform;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    DeviceNotifications.debugOverride = FakeNotifications(notifications);
    final harness = Harness(workspace());
    await harness.show(tester, size: size);
    harness.page.value = AppPage.settings;
    await pumpFrames(tester);
    harness.backend.settingsSection.value = 'device#permissions';
    await pumpFrames(tester);
    return harness;
  }

  testWidgets('an iPhone: notifications on, the microphone off with Open '
      "Settings and what that stops, the camera not asked yet: Turn on… "
      'asks, and it reads On', (tester) async {
    recorder.granted = MicAccess.denied;
    final harness = await show(
      tester,
      platform: TargetPlatform.iOS,
      size: const Size(393, 852),
    );
    harness.camera.state = CameraAccess.undetermined;
    // Read again as the app comes back to the front.
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await pumpFrames(tester);

    expect(find.text('Permissions'), findsWidgets);
    expect(_inRow('Notifications', find.text('On')), findsOneWidget);
    expect(_inRow('Microphone', find.text('Off')), findsOneWidget);
    expect(_inRow('Microphone', _button('Open Settings')), findsOneWidget);
    expect(
      _inRow(
        'Microphone',
        find.text("You can't record voice messages or say your answers."),
      ),
      findsOneWidget,
    );
    expect(_inRow('Camera', find.text('Not asked yet')), findsOneWidget);

    final turnOn = _inRow('Camera', _button('Turn on…'));
    await tester.ensureVisible(turnOn);
    await pumpFrames(tester);
    await tester.tap(turnOn);
    await pumpFrames(tester);
    // The camera's own sheet, before the system asks, in Settings' words:
    // scanning codes and taking photos (Trish 49821).
    expect(find.text('Use the camera'), findsOneWidget);
    expect(find.text('Scan the code'), findsNothing);
    await tester.tap(_button('Continue'));
    await pumpFrames(tester);
    expect(harness.camera.asked, 1);
    expect(_inRow('Camera', find.text('On')), findsOneWidget);
    expect(_inRow('Camera', _button('Turn on…')), findsNothing);
  });

  testWidgets('a Mac: notifications off with Open System Settings, no camera '
      'row, the microphone not allowed (Screen Time) with why and no '
      'button; turned on in System Settings, it reads On once back', (
    tester,
  ) async {
    recorder.granted = MicAccess.restricted;
    await show(
      tester,
      platform: TargetPlatform.macOS,
      size: const Size(1200, 900),
      notifications: 'denied',
    );
    expect(_inRow('Notifications', find.text('Off')), findsOneWidget);
    final open = _inRow('Notifications', _button('Open System Settings'));
    expect(open, findsOneWidget);
    await tester.tap(open);
    await pumpFrames(tester);
    final device = DeviceNotifications.debugOverride! as FakeNotifications;
    expect(device.opened, 1);
    expect(find.byKey(const ValueKey('permission:Camera')), findsNothing);
    expect(_inRow('Microphone', find.text('Not allowed')), findsOneWidget);
    expect(
      _inRow('Microphone', find.textContaining('Screen Time')),
      findsOneWidget,
    );
    expect(_inRow('Microphone', find.byType(Button)), findsNothing);

    device.state = 'allowed';
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await pumpFrames(tester);
    expect(_inRow('Notifications', find.text('On')), findsOneWidget);
  });

  testWidgets('on Android, a first no that it will still ask after reads '
      'Off, with Turn on… asking again (Lucy ruling 6)', (tester) async {
    recorder.granted = MicAccess.undetermined;
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    var refused = true;
    messenger.setMockMethodCallHandler(
      const MethodChannel('factory/recorder'),
      (call) async => call.method == 'refusedOnce' ? refused : null,
    );
    addTearDown(
      () => messenger.setMockMethodCallHandler(
        const MethodChannel('factory/recorder'),
        null,
      ),
    );
    await show(
      tester,
      platform: TargetPlatform.android,
      size: const Size(412, 915),
    );
    expect(_inRow('Microphone', find.text('Off')), findsOneWidget);
    expect(
      _inRow(
        'Microphone',
        find.text("You can't record voice messages or say your answers."),
      ),
      findsOneWidget,
    );
    expect(_inRow('Microphone', _button('Turn on…')), findsOneWidget);
    expect(_inRow('Microphone', _button('Open Settings')), findsNothing);

    // Never asked: as before.
    refused = false;
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await pumpFrames(tester);
    expect(_inRow('Microphone', find.text('Not asked yet')), findsOneWidget);
  });

  testWidgets('with the microphone on, the camera row and its Turn on… stay '
      "their own for TalkBack and VoiceOver (Sale's row 201)", (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = await show(
      tester,
      platform: TargetPlatform.iOS,
      size: const Size(393, 852),
    );
    harness.camera.state = CameraAccess.undetermined;
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await pumpFrames(tester);
    expect(_inRow('Microphone', find.text('On')), findsOneWidget);
    final mic = tester.getSemantics(
      find.byKey(const ValueKey('permission:Microphone')),
    );
    final camera = tester.getSemantics(
      find.byKey(const ValueKey('permission:Camera')),
    );
    expect(mic, isNot(same(camera)));
    expect(mic.label, isNot(contains('Camera')));
    final button = tester.getSemantics(_inRow('Camera', _button('Turn on…')));
    expect(button.label, 'Turn on…');
    semantics.dispose();
  });

  testWidgets("a row's label shares the line with its state and button, "
      'the caption under them (Lucy 50427)', (tester) async {
    recorder.granted = MicAccess.denied;
    await show(
      tester,
      platform: TargetPlatform.macOS,
      size: const Size(1200, 900),
      notifications: 'denied',
    );
    final label = tester.getRect(_inRow('Microphone', find.text('Microphone')));
    final state = tester.getRect(_inRow('Microphone', find.text('Off')));
    final caption = tester.getRect(
      _inRow(
        'Microphone',
        find.text("You can't record voice messages or say your answers."),
      ),
    );
    expect((label.bottom - state.bottom).abs(), lessThan(3));
    expect(caption.top, greaterThan(label.bottom));
  });

  testWidgets('nothing it can read: no heading, no rows', (tester) async {
    recorder.granted = MicAccess.unavailable;
    DeviceNotifications.debugOverride = null;
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace());
    await harness.show(tester, size: const Size(412, 915));
    harness.page.value = AppPage.settings;
    await pumpFrames(tester);
    harness.backend.settingsSection.value = 'device';
    await pumpFrames(tester);
    expect(find.text('Permissions'), findsNothing);
    expect(find.byKey(const ValueKey('permission:Microphone')), findsNothing);
  });
}
