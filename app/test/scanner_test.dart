// The phone's camera for scanning a pairing code (scanner.dart): on an
// iPhone and on Android it goes to the native side over factory/device, and
// its picture is the native claktory/scanner view, each with its own
// channel for the codes it reads; anywhere else there's no camera and no
// call is made. Wire and view contract only, not a real camera reading a
// real QR code (that's the device run's).
import 'package:factory_app/scanner.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  const device = MethodChannel('factory/device');

  void on(TargetPlatform platform) {
    debugDefaultTargetPlatformOverride = platform;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
  }

  /// factory/device answering [answers] by method; every call kept.
  List<MethodCall> deviceAnswering(Map<String, Object?> answers) {
    final calls = <MethodCall>[];
    messenger.setMockMethodCallHandler(device, (call) async {
      calls.add(call);
      return answers[call.method];
    });
    addTearDown(() => messenger.setMockMethodCallHandler(device, null));
    return calls;
  }

  for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
    test('on ${platform.name}: every camera access the native side says, '
        'and asking and Settings go to it', () async {
      on(platform);
      const camera = DeviceCamera();
      for (final access in CameraAccess.values) {
        deviceAnswering({'cameraAccess': access.name});
        expect(await camera.access(), access);
      }
      deviceAnswering({'cameraAccess': 'something new'});
      expect(
        await camera.access(),
        CameraAccess.unavailable,
        reason: "an answer it doesn't know is no camera",
      );
      final calls = deviceAnswering({'requestCamera': true});
      expect(await camera.request(), isTrue);
      await camera.openSettings();
      expect(calls.map((call) => call.method), [
        'requestCamera',
        'openCameraSettings',
      ]);
    });
  }

  test("a native side that doesn't answer is no camera", () async {
    on(TargetPlatform.android);
    const camera = DeviceCamera();
    expect(await camera.access(), CameraAccess.unavailable);
    messenger.setMockMethodCallHandler(
      device,
      (call) async => throw PlatformException(code: 'broken'),
    );
    addTearDown(() => messenger.setMockMethodCallHandler(device, null));
    expect(await camera.access(), CameraAccess.unavailable);
  });

  test(
    'elsewhere: no camera, and nothing is asked of the native side',
    () async {
      on(TargetPlatform.macOS);
      final calls = deviceAnswering({});
      const camera = DeviceCamera();
      expect(await camera.access(), CameraAccess.unavailable);
      expect(await camera.request(), isFalse);
      await camera.openSettings();
      expect(calls, isEmpty);
    },
  );

  group('the picture', () {
    /// Platform views made: their creation calls.
    List<MethodCall> platformViews() {
      final made = <MethodCall>[];
      messenger.setMockMethodCallHandler(SystemChannels.platform_views, (
        call,
      ) async {
        if (call.method == 'create') {
          made.add(call);
          return 0;
        }
        return null;
      });
      addTearDown(
        () => messenger.setMockMethodCallHandler(
          SystemChannels.platform_views,
          null,
        ),
      );
      return made;
    }

    Future<void> show(WidgetTester tester, ValueChanged<String> onCode) =>
        tester.pumpWidget(
          Directionality(
            textDirection: TextDirection.ltr,
            child: Center(
              child: SizedBox(
                width: 300,
                height: 300,
                child: const DeviceCamera().preview(onCode),
              ),
            ),
          ),
        );

    /// The native view [id] says it read [text] (or calls [method]).
    Future<void> native(int id, Object? text, {String method = 'scanned'}) =>
        messenger.handlePlatformMessage(
          'claktory/scanner/$id',
          const StandardMethodCodec().encodeMethodCall(
            MethodCall(method, text),
          ),
          (_) {},
        );

    testWidgets('on Android: the native view, its codes reaching the screen '
        'until it goes', (tester) async {
      on(TargetPlatform.android);
      final made = platformViews();
      final codes = <String>[];
      await show(tester, codes.add);
      await tester.pump();
      expect(find.byType(AndroidView), findsOneWidget);
      expect(find.byType(UiKitView), findsNothing);
      expect(
        made.single.arguments,
        containsPair('viewType', 'claktory/scanner'),
      );
      final id = (made.single.arguments as Map)['id'] as int;

      await native(id, 'claktory://add?code=abc');
      await native(id, 42);
      await native(id, 'ignored', method: 'somethingElse');
      expect(codes, ['claktory://add?code=abc']);

      // Gone (the person pasted instead, or connected): no more codes.
      await tester.pumpWidget(const SizedBox());
      await native(id, 'claktory://add?code=late');
      expect(codes, ['claktory://add?code=abc']);
    });

    testWidgets('on an iPhone: the native UIKit view', (tester) async {
      on(TargetPlatform.iOS);
      final made = platformViews();
      final codes = <String>[];
      await show(tester, codes.add);
      await tester.pump();
      expect(find.byType(UiKitView), findsOneWidget);
      expect(find.byType(AndroidView), findsNothing);
      expect(
        made.single.arguments,
        containsPair('viewType', 'claktory/scanner'),
      );
      final id = (made.single.arguments as Map)['id'] as int;
      await native(id, 'claktory://add?code=abc');
      expect(codes, ['claktory://add?code=abc']);
    });

    testWidgets('elsewhere: no native view at all', (tester) async {
      on(TargetPlatform.macOS);
      final made = platformViews();
      await show(tester, (_) {});
      await tester.pump();
      expect(find.byType(AndroidView), findsNothing);
      expect(find.byType(UiKitView), findsNothing);
      expect(made, isEmpty);
    });
  });
}
