// The permission sheet (mobile/permission.dart, area 5a), drawn: with
// SHOTS set, PNGs of notifications before and declined, a restricted one,
// the microphone's popover over the message box, and the camera's first
// time and refusal on the pairing screen, on an iPhone SE, a Pro Max and
// an iPad, at the default text size and a large one:
//   SHOTS=/tmp/permissions flutter test test/mobile_permission_shots_test.dart
// test-bundle: alone (loads the app's fonts)
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/mobile/device_permissions.dart';
import 'package:factory_app/mobile/permission.dart';
import 'package:factory_app/mobile/push.dart' show PushRegistrar;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart' show MicAccess, VoiceRecorder;
import 'package:factory_app/scanner.dart' show CameraAccess;
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'pair_start.dart';
import 'phone_sizes_test.dart' show loadGeist;

Future<void> _shot(WidgetTester tester, String name) async {
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

/// Notifications' words, answering as the test says.
class _Notifications extends Permission {
  _Notifications(this.now);

  PermissionAccess now;

  @override
  PermissionWords words(BuildContext context) =>
      NotificationsPermission(_NoPushes()).words(context);

  @override
  Future<PermissionAccess> access() async => now;

  @override
  Future<PermissionAccess> request() async => now = PermissionAccess.denied;

  @override
  Future<void> openSettings() async {}
}

class _NoPushes implements PushRegistrar {
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _Mic implements VoiceRecorder {
  @override
  Future<MicAccess> access() async => MicAccess.undetermined;

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

Workspace _workspace() => Workspace(
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      role: 'Researcher',
      color: 'teal',
      modelAvailable: true,
      model: 'anthropic/claude-opus-4-8',
    ),
  ],
  selectedAgentId: 'june',
  settings: Settings(userName: 'Matt'),
);

void main() {
  setUpAll(loadGeist);

  for (final (device, size, padding) in const [
    ('se', Size(375, 667), FakeViewPadding(top: 20)),
    ('promax', Size(440, 956), FakeViewPadding(top: 62, bottom: 34)),
    ('ipad', Size(1032, 1376), FakeViewPadding(top: 24, bottom: 20)),
  ]) {
    for (final scale in const [1.0, 2.35]) {
      void setUpDevice(WidgetTester tester) {
        if (device == 'ipad') {
          asIPad();
        } else {
          debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
          addTearDown(() => debugDefaultTargetPlatformOverride = null);
        }
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        tester.view.padding = padding;
        addTearDown(tester.view.resetPadding);
      }

      testWidgets('$device at ${scale}x: notifications, before and off, and '
          'a restricted one', (tester) async {
        setUpDevice(tester);
        final harness = Harness(_workspace());
        await harness.show(tester, size: size);
        await pumpFrames(tester);
        final context = tester.element(find.byType(Text).last);
        final permission = _Notifications(PermissionAccess.undetermined);
        unawaited(askPermission(context, permission));
        await pumpFrames(tester);
        await tester.pump(const Duration(seconds: 1));
        expect(tester.takeException(), isNull);
        await _shot(tester, '$device-$scale-notifications-before');

        await tester.tap(find.text('Continue'));
        await pumpFrames(tester);
        await tester.pump(const Duration(seconds: 1));
        expect(tester.takeException(), isNull);
        await _shot(tester, '$device-$scale-notifications-off');
        await tester.tap(find.text('Not now'));
        await pumpFrames(tester);
        await tester.pump(const Duration(seconds: 1));

        unawaited(
          askPermission(context, _Notifications(PermissionAccess.restricted)),
        );
        await pumpFrames(tester);
        await tester.pump(const Duration(seconds: 1));
        expect(tester.takeException(), isNull);
        await _shot(tester, '$device-$scale-restricted');
      });

      testWidgets('$device at ${scale}x: the microphone\'s popover', (
        tester,
      ) async {
        setUpDevice(tester);
        final harness = Harness(_workspace());
        await harness.show(tester, size: size);
        await pumpFrames(tester);
        final mic = find.byKey(const ValueKey('mic'));
        // The mic where it is; without one (no speech model here), the
        // message box's end, where it would be.
        final anchor = mic.evaluate().isNotEmpty
            ? mic
            : find.byType(EditableText).first;
        unawaited(askMicrophone(tester.element(anchor), recorder: _Mic()));
        await pumpFrames(tester);
        await tester.pump(const Duration(seconds: 1));
        expect(tester.takeException(), isNull);
        await _shot(tester, '$device-$scale-mic');
      });

      if (device != 'ipad') {
        testWidgets('$device at ${scale}x: the camera, first and refused', (
          tester,
        ) async {
          setUpDevice(tester);
          final harness = Harness(Workspace(), localBackend: false);
          harness.camera
            ..state = CameraAccess.undetermined
            ..granting = false;
          await harness.show(tester, size: size);
          await pumpFrames(tester);
          await pastPairStart(tester);
          await pumpFrames(tester);
          expect(tester.takeException(), isNull);
          await _shot(tester, '$device-$scale-camera-before');
          await tester.ensureVisible(find.text('Continue'));
          await tester.tap(find.text('Continue'));
          await pumpFrames(tester);
          expect(tester.takeException(), isNull);
          await _shot(tester, '$device-$scale-camera-off');
        });
      }
    }
  }
}
