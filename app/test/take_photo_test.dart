// Take Photo from the paperclip (Matt, Bruno 46436): the device's menu
// offers it first where a box or card takes photos (`camera`), and answers
// `take_photo` when it's chosen; the camera is explained on the permission
// sheet before the system asks, refused it says how to turn it on, and only
// then does takePhoto open the camera. The same on an iPhone and Android
// (Risotto 46470's cases), with the channel faked.
import 'dart:async';

import 'package:factory_app/attachments.dart' show pickOnDevice;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart' show CameraAccess;
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// factory/device as a phone answers: pickAttachments with Take Photo
/// chosen, takePhoto with [photo] (or throwing [photoError]); the calls.
class _Device {
  _Device({this.photo = const ['/tmp/Photo 2026-11-26 at 21.04.33.jpg']}) {
    _messenger.setMockMethodCallHandler(_channel, (call) async {
      calls.add(call.method);
      arguments.add(call.arguments);
      switch (call.method) {
        case 'pickAttachments':
          throw PlatformException(code: 'take_photo');
        case 'takePhoto':
          beforePhoto?.call();
          final error = photoError;
          if (error != null) throw PlatformException(code: error);
          return photo;
      }
      return null;
    });
    addTearDown(() => _messenger.setMockMethodCallHandler(_channel, null));
  }

  static const _channel = MethodChannel('factory/device');
  static final _messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  final List<String> photo;
  String? photoError;
  VoidCallback? beforePhoto;
  final calls = <String>[];
  final arguments = <Object?>[];
}

void main() {
  for (final platform in [TargetPlatform.iOS, TargetPlatform.android]) {
    final device = platform == TargetPlatform.iOS ? 'iPhone' : 'Android';

    /// The screen, the camera as [access] says, and a pick started.
    Future<(Harness, _Device, Completer<List<String>>)> start(
      WidgetTester tester,
      CameraAccess access, {
      _Device? fake,
      void Function(Harness harness)? beforePhoto,
    }) async {
      debugDefaultTargetPlatformOverride = platform;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = Harness(Workspace());
      harness.camera.state = access;
      late BuildContext context;
      await harness.show(
        tester,
        size: const Size(393, 852),
        home: Builder(
          builder: (c) {
            context = c;
            return const SizedBox.expand();
          },
        ),
      );
      final channel = fake ?? _Device();
      if (beforePhoto != null) channel.beforePhoto = () => beforePhoto(harness);
      final result = Completer<List<String>>();
      // A failure waits for the test's expectLater, not reported meanwhile.
      result.future.ignore();
      unawaited(
        pickOnDevice(
          context,
          camera: true,
        ).then(result.complete, onError: result.completeError),
      );
      await pumpFrames(tester, const Duration(seconds: 1));
      return (harness, channel, result);
    }

    testWidgets('$device: the menu offers Take Photo; allowed already, the '
        'camera opens at once and its photo is attached', (tester) async {
      final (harness, channel, result) = await start(
        tester,
        CameraAccess.granted,
      );
      expect(channel.arguments.first, {'camera': true});
      expect(find.text('Take a photo'), findsNothing, reason: 'no sheet');
      expect(channel.calls, ['pickAttachments', 'takePhoto']);
      expect(await result.future, ['/tmp/Photo 2026-11-26 at 21.04.33.jpg']);
      expect(harness.camera.asked, 0);
    });

    testWidgets('$device: the first time, why first; Continue asks, and '
        'allowed, the camera opens', (tester) async {
      final (harness, channel, result) = await start(
        tester,
        CameraAccess.undetermined,
      );
      expect(find.text('Take a photo'), findsOneWidget);
      expect(channel.calls, ['pickAttachments'], reason: 'not yet');
      await tester.tap(find.text('Continue'));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(harness.camera.asked, 1);
      expect(channel.calls.last, 'takePhoto');
      expect(await result.future, hasLength(1));
    });

    testWidgets('$device: refused, and the device can ask again: why again; '
        'refused for good: what to do, and no camera', (tester) async {
      final fake = _Device();
      final (harness, channel, result) = await start(
        tester,
        CameraAccess.undetermined,
        fake: fake,
      );
      harness.camera
        ..granting = false
        ..afterRefusal = CameraAccess.undetermined;
      await tester.tap(find.text('Continue'));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.text('Take a photo'), findsOneWidget, reason: 'Before');
      harness.camera.afterRefusal = CameraAccess.denied;
      await tester.tap(find.text('Continue'));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.text('The camera is off for Claktory'), findsOneWidget);
      await tester.tap(find.text('Not now'));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(await result.future, isEmpty);
      expect(channel.calls, isNot(contains('takePhoto')));
    });

    testWidgets('$device: the camera turned off between the check and the '
        'camera: what to do', (tester) async {
      final fake = _Device()..photoError = 'camera_denied';
      final (_, _, result) = await start(
        tester,
        CameraAccess.granted,
        fake: fake,
        beforePhoto: (harness) => harness.camera.state = CameraAccess.denied,
      );
      expect(find.text('The camera is off for Claktory'), findsOneWidget);
      await tester.tap(find.text('Not now'));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(await result.future, isEmpty);
    });

    testWidgets('$device: cancelled in the camera, nothing', (tester) async {
      final (_, _, cancelled) = await start(
        tester,
        CameraAccess.granted,
        fake: _Device(photo: const []),
      );
      expect(await cancelled.future, isEmpty);
    });

    testWidgets("$device: a failure is the caller's to say", (tester) async {
      final (_, _, failed) = await start(
        tester,
        CameraAccess.granted,
        fake: _Device()..photoError = 'no_space',
      );
      await expectLater(failed.future, throwsA(isA<PlatformException>()));
    });
  }
}
