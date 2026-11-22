// The system's share sheet for text (an invite code): on a touch device
// (an iPhone, an iPad, an Android phone) it asks factory/device to share,
// pointing at the button on an iPad; on a Mac there's none, and nothing is
// asked.
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/mobile/share.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  const channel = MethodChannel('factory/device');
  late List<MethodCall> calls;
  setUp(() {
    calls = [];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          calls.add(call);
          return true;
        });
  });
  tearDown(
    () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null),
  );

  Future<BuildContext> context(WidgetTester tester) async {
    await tester.pumpWidget(const SizedBox());
    return tester.element(find.byType(SizedBox));
  }

  testWidgets('on iOS: the text, and where the button is', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final c = await context(tester);
    expect(canShare(c), isTrue);
    expect(
      await shareText(
        c,
        'claktory-pipe://x',
        from: const Rect.fromLTWH(10, 20, 30, 40),
      ),
      isTrue,
    );
    expect(calls.single.method, 'share');
    expect(calls.single.arguments, {
      'text': 'claktory-pipe://x',
      'rect': [10.0, 20.0, 30.0, 40.0],
    });
  });

  testWidgets('on Android: the same call, with the text', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final saved = PhonePlatformPolicy.debugAndroidPolicy;
    PhonePlatformPolicy.debugAndroidPolicy = true;
    addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = saved);
    final c = await context(tester);
    expect(canShare(c), isTrue);
    expect(await shareText(c, 'claktory-pipe://x'), isTrue);
    expect(calls.single.method, 'share');
    expect(calls.single.arguments, {'text': 'claktory-pipe://x'});
  });

  testWidgets('on a Mac: no sheet, nothing asked', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final c = await context(tester);
    expect(canShare(c), isFalse);
    expect(await shareText(c, 'code'), isFalse);
    expect(calls, isEmpty);
  });

  testWidgets('nothing to share: nothing asked', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final c = await context(tester);
    expect(await shareText(c, ''), isFalse);
    expect(calls, isEmpty);
  });

  testWidgets('a share the system refuses says no', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          channel,
          (call) async => throw PlatformException(code: 'failed'),
        );
    final c = await context(tester);
    expect(await shareText(c, 'code'), isFalse);
  });

  testWidgets('an app built before share says no', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          channel,
          (call) async => throw MissingPluginException(),
        );
    final c = await context(tester);
    expect(await shareText(c, 'code'), isFalse);
  });
}
