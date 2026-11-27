// Sale rows 8 and 7: a fresh Android tablet reaches the shared store welcome,
// not the Mac's ConnectStep, and its code field stays above the keyboard.
// Recording backends and fake native channels, not camera/device acceptance.
import 'package:factory_app/android/pair_step.dart';
import 'package:factory_app/connect_step.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/drafts.dart';
import 'package:factory_app/main.dart';
import 'package:factory_app/mobile/device.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/phone.dart' show PairScreen;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart' show DeviceDrafts;
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../phone_sizes_test.dart' show loadGeist;

const _code =
    'factory://?t=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
    '&name=Home&e=cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc';

class _Fresh {
  final connections = Connections(MemoryConnectionStorage());
  final calls = <MethodCall>[];
  final backends = <RecordingBackend>[];
  var camera = 'undetermined';

  Future<void> show(
    WidgetTester tester, {
    Size size = const Size(1280, 800),
    Idiom idiom = Idiom.pad,
  }) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    debugDeviceCapabilities = DeviceCapabilities(idiom: idiom);
    PhonePlatformPolicy.debugAndroidPolicy = true;
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    const channel = MethodChannel('factory/device');
    messenger.setMockMethodCallHandler(channel, (call) async {
      calls.add(call);
      switch (call.method) {
        case 'cameraAccess':
          return camera;
        case 'requestCamera':
          camera = 'denied';
          return false;
        case 'share':
          return false; // Chooser cancellation is not delivery.
      }
      return null;
    });
    addTearDown(() {
      debugDefaultTargetPlatformOverride = null;
      debugDeviceCapabilities = null;
      PhonePlatformPolicy.debugAndroidPolicy = false;
      messenger.setMockMethodCallHandler(channel, null);
    });
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1;
    tester.view.display.size = idiom == Idiom.phone
        ? size
        : const Size(1280, 800);
    tester.view.display.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    addTearDown(tester.view.display.reset);
    await tester.pumpWidget(
      FactoryApp(
        drafts: DeviceDrafts(MemoryDraftStorage()),
        connections: connections,
        localBackend: false,
        open: (target) {
          final backend = RecordingBackend(localBackend: false)
            ..connectTo(target);
          backend.workspace.apply(
            Workspace(settings: Settings(userName: 'Robin')),
          );
          backends.add(backend);
          return backend;
        },
      ),
    );
    await pumpFrames(tester);
    expect(find.byType(PairScreen), findsOneWidget);
    expect(find.byType(ConnectStep), findsNothing);
  }

  void noSend() {
    for (final backend in backends) {
      expect(
        backend.sent.where((m) => m.hasSendMessage() || m.hasSendChatMessage()),
        isEmpty,
      );
    }
  }
}

Future<void> _tap(WidgetTester tester, String text) async {
  final choice = find.text(text);
  await tester.ensureVisible(choice);
  await tester.pump();
  await tester.tap(choice);
  await pumpFrames(tester);
}

void main() {
  setUpAll(loadGeist);
  for (final (name, size, wide) in const [
    ('landscape tablet', Size(1280, 800), true),
    ('portrait tablet', Size(800, 1280), true),
    ('tablet split', Size(635, 800), false),
    ('phone', Size(393, 852), false),
  ]) {
    testWidgets('$name: the real welcome has every way in', (tester) async {
      final fresh = _Fresh();
      await fresh.show(
        tester,
        size: size,
        idiom: name == 'phone' ? Idiom.phone : Idiom.pad,
      );
      expect(
        find.byType(AndroidPairStep),
        wide ? findsOneWidget : findsNothing,
      );
      for (final text in [
        'Send me the link',
        'Sign in with your email',
        'Scan code instead',
        'Paste code instead',
      ]) {
        expect(find.text(text), findsOneWidget);
      }
      // Its pushes not unavailable (Google Play services there): no line
      // saying it can't get notifications (pair_start_test has the words).
      expect(find.textContaining("can't get notifications"), findsNothing);
      expect(
        find.textContaining('Mac or a Linux server (or Windows, in preview)'),
        findsOneWidget,
      );
      expect(fresh.calls.where((c) => c.method == 'requestCamera'), isEmpty);
      expect(fresh.connections.saved, isEmpty);
      expect(tester.takeException(), isNull);
      fresh.noSend();
    });
  }

  testWidgets(
    'an Android phone identity in a wide window still gets the store welcome',
    (tester) async {
      final fresh = _Fresh();
      // A foldable can keep the phone idiom loaded at boot after unfolding.
      await fresh.show(tester, idiom: Idiom.phone);
      expect(find.byType(AndroidPairStep), findsOneWidget);
      expect(find.text('Sign in with your email'), findsOneWidget);
      expect(find.text('Scan code instead'), findsOneWidget);
      expect(fresh.connections.saved, isEmpty);
      fresh.noSend();
    },
  );

  testWidgets('tablet Paste reaches a visible field above the keyboard and '
      'asks before connecting', (tester) async {
    final fresh = _Fresh();
    await fresh.show(tester);
    await _tap(tester, 'Paste code instead');
    final field = find.descendant(
      of: find.byType(PairScreen),
      matching: find.byType(EditableText),
    );
    expect(field, findsOneWidget);
    await tester.enterText(field, _code);
    tester.view.viewInsets = const FakeViewPadding(bottom: 368);
    await pumpFrames(tester);
    expect(tester.getRect(find.byType(PairScreen)).bottom, 800 - 368);
    await tester.ensureVisible(field);
    await pumpFrames(tester);
    expect(tester.getRect(field).bottom, lessThanOrEqualTo(800 - 368));
    expect(tester.widget<EditableText>(field).controller.text, _code);
    expect(fresh.connections.saved, isEmpty);
    await _tap(tester, 'Connect');
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(fresh.connections.current, isNull);
    await tester.tap(find.widgetWithText(Button, 'Connect').last);
    await pumpFrames(tester);
    expect(fresh.connections.current?.code, _code);
    expect(find.byType(PairScreen), findsNothing);
    fresh.noSend();
  });

  testWidgets('tablet Scan reaches the shared permission-denied fallback', (
    tester,
  ) async {
    final fresh = _Fresh();
    await fresh.show(tester);
    await _tap(tester, 'Scan code instead');
    // Why first (area 5a, P7): Continue asks.
    expect(fresh.calls.where((c) => c.method == 'requestCamera'), isEmpty);
    await _tap(tester, 'Continue');
    expect(fresh.calls.where((c) => c.method == 'requestCamera'), hasLength(1));
    expect(find.text('Open Settings'), findsOneWidget);
    expect(find.text('Paste code instead'), findsOneWidget);
    expect(fresh.connections.saved, isEmpty);
    fresh.noSend();
  });

  testWidgets('tablet email choice reaches the shared email editor', (
    tester,
  ) async {
    final fresh = _Fresh();
    await fresh.show(tester);
    await _tap(tester, 'Sign in with your email');
    expect(find.text('Send code'), findsOneWidget);
    expect(
      tester.widget<EditableText>(find.byType(EditableText)).keyboardType,
      TextInputType.emailAddress,
    );
    expect(fresh.connections.saved, isEmpty);
    fresh.noSend();
  });

  testWidgets(
    'tablet Send me the link opens the existing native chooser only',
    (tester) async {
      final fresh = _Fresh();
      await fresh.show(tester);
      await _tap(tester, 'Send me the link');
      final call = fresh.calls.singleWhere((c) => c.method == 'share');
      expect((call.arguments as Map)['text'], contains('https://claktory.com'));
      expect(find.byType(PairScreen), findsOneWidget);
      expect(fresh.connections.saved, isEmpty);
      fresh.noSend();
    },
  );
}
