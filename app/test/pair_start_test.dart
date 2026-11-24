// #363 (Squalo's first hour from the stores): a phone with no Claktory
// starts with what's needed (Claktory runs on a Mac; Send me the link),
// then the ways in, signing in with an email first. A phone that has one
// already, adding another, goes straight to the ways in.
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';

Future<Harness> _show(
  WidgetTester tester,
  TargetPlatform platform, {
  CameraAccess camera = CameraAccess.undetermined,
  Size size = const Size(393, 852),
}) async {
  debugDefaultTargetPlatformOverride = platform;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(Workspace(), localBackend: false);
  harness.camera.state = camera;
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  return harness;
}

/// What the share sheet was asked to send, through factory/device.
List<String> _shared(WidgetTester tester) {
  final shared = <String>[];
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    const MethodChannel('factory/device'),
    (call) async {
      if (call.method != 'share') return null;
      shared.add((call.arguments as Map)['text'] as String);
      return true;
    },
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      const MethodChannel('factory/device'),
      null,
    ),
  );
  return shared;
}

/// [label]'s link holds all of its caption, at least 44 points tall.
Future<void> _linkWhole(WidgetTester tester, String label) async {
  final text = find.text(label);
  await tester.ensureVisible(text);
  await pumpFrames(tester);
  final box = tester.getRect(
    find.ancestor(of: text, matching: find.byType(ConstrainedBox)).first,
  );
  final caption = tester.getRect(text);
  final paragraph = tester.renderObject<RenderParagraph>(text);
  expect(box.height, greaterThanOrEqualTo(44), reason: label);
  // Laid out as tall as its lines need, with nothing cut.
  expect(
    paragraph.size.height,
    greaterThanOrEqualTo(
      paragraph.getMaxIntrinsicHeight(paragraph.size.width) - 0.5,
    ),
    reason: label,
  );
  expect(caption.top, greaterThanOrEqualTo(box.top), reason: label);
  expect(caption.bottom, lessThanOrEqualTo(box.bottom), reason: label);
}

void main() {
  testWidgets('on an iPhone with no Claktory: what it needs, Send me the '
      'link, then email first; no camera asked for', (tester) async {
    final shared = _shared(tester);
    final harness = await _show(tester, TargetPlatform.iOS);
    expect(find.text('Connect to your Claktory'), findsOneWidget);
    expect(
      find.text(
        'Claktory runs on a Mac. Set it up there, then connect this '
        'iPhone to it.',
      ),
      findsOneWidget,
    );
    // The scan step's words wait for the scan step.
    expect(find.textContaining('On your Mac, open Claktory'), findsNothing);
    double top(String text) => tester.getTopLeft(find.text(text)).dy;
    expect(top('Send me the link'), lessThan(top('Already set up?')));
    expect(top('Already set up?'), lessThan(top('Sign in with your email')));
    expect(top('Sign in with your email'), lessThan(top('Scan code instead')));
    expect(top('Scan code instead'), lessThan(top('Paste code instead')));
    expect(find.text('Sign in with your email instead'), findsNothing);
    expect(find.textContaining('notifications'), findsNothing);
    expect(harness.camera.asked, 0);

    await tester.tap(find.text('Send me the link'));
    await pumpFrames(tester);
    expect(shared, [
      'Set up Claktory on your Mac: https://claktory.com/download',
    ]);
  });

  testWidgets('Sign in with your email opens signing in; Use a code instead '
      'goes to scanning', (tester) async {
    await _show(tester, TargetPlatform.iOS, camera: CameraAccess.granted);
    await tester.tap(find.text('Sign in with your email'));
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('device-email')), findsOneWidget);
    expect(find.text('Send me the link'), findsNothing);
    await tester.tap(find.text('Use a code instead'));
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('camera')), findsOneWidget);
    expect(find.textContaining('On your Mac, open Claktory'), findsOneWidget);
  });

  testWidgets('on Android: a Mac or a Linux server, and that this phone '
      "doesn't get notifications yet", (tester) async {
    await _show(tester, TargetPlatform.android, camera: CameraAccess.granted);
    expect(
      find.text(
        'Claktory runs on a Mac or a Linux server. Set it up there, then '
        'connect this phone to it.',
      ),
      findsOneWidget,
    );
    expect(
      find.text(
        "This phone doesn't get notifications yet. Keep Claktory open to "
        'see what your agents need.',
      ),
      findsOneWidget,
    );
    // Still there once a way is chosen: it's the end of pairing.
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester);
    expect(
      find.textContaining("doesn't get notifications yet"),
      findsOneWidget,
    );
  });

  testWidgets('a way chosen has Back to the first screen; pasting says '
      'paste (Sale, #363)', (tester) async {
    await _show(tester, TargetPlatform.iOS, camera: CameraAccess.granted);
    expect(find.bySemanticsLabel('Back'), findsNothing);
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester);
    expect(
      find.textContaining('Paste the code for this iPhone.'),
      findsOneWidget,
    );
    expect(find.textContaining('scan the code'), findsNothing);
    await tester.tap(find.bySemanticsLabel('Back'));
    await pumpFrames(tester);
    expect(find.text('Send me the link'), findsOneWidget);
    expect(find.bySemanticsLabel('Back'), findsNothing);
    await tester.tap(find.text('Sign in with your email'));
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel('Back'));
    await pumpFrames(tester);
    expect(find.text('Send me the link'), findsOneWidget);
  });

  testWidgets('on Android the link says a Mac or a Linux server', (
    tester,
  ) async {
    final shared = _shared(tester);
    PhonePlatformPolicy.debugAndroidPolicy = true;
    addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
    await _show(tester, TargetPlatform.android, camera: CameraAccess.granted);
    await tester.tap(find.text('Send me the link'));
    await pumpFrames(tester);
    expect(shared, [
      'Set up Claktory on your Mac or Linux server: '
          'https://claktory.com/download',
    ]);
  });

  testWidgets('with no camera, only pasting is offered', (tester) async {
    await _show(
      tester,
      TargetPlatform.android,
      camera: CameraAccess.unavailable,
    );
    expect(find.text('Scan code instead'), findsNothing);
    expect(find.text('Paste code instead'), findsOneWidget);
  });

  testWidgets('on an iPad its words say iPad, not phone', (tester) async {
    asIPad();
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera.state = CameraAccess.unavailable;
    await harness.show(tester, size: const Size(390, 844));
    await pumpFrames(tester);
    expect(
      find.text(
        'Claktory runs on a Mac. Set it up there, then connect this '
        'iPad to it.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester);
    expect(find.textContaining('Paste the code for this iPad.'), findsOne);
    expect(find.textContaining('phone'), findsNothing);
    expect(find.textContaining('iPhone'), findsNothing);
  });

  testWidgets('a phone with a Claktory already, adding another: straight to '
      'the ways in', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    final kept = parsePairingCode(
      'factory://?t=${'a' * 64}&e=${'c' * 64}&name=mac-mini',
    )!;
    harness.connections
      ..use(kept)
      ..use(null);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    expect(find.byType(PairScreen), findsOneWidget);
    expect(find.text('Already set up?'), findsNothing);
    expect(find.text('Send me the link'), findsNothing);
    expect(find.text('Sign in with your email instead'), findsOneWidget);
  });

  for (final size in [const Size(320, 568), const Size(430, 932)]) {
    testWidgets('at the largest text on a ${size.width.toInt()}-point phone '
        'it fits, and scrolls to all of it', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.12;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await _show(tester, TargetPlatform.iOS, size: size);
      expect(tester.takeException(), isNull);
      for (final label in [
        'Send me the link',
        'Sign in with your email',
        'Paste code instead',
      ]) {
        await tester.ensureVisible(find.text(label));
        await pumpFrames(tester);
        final rect = tester.getRect(find.text(label));
        expect(rect.left, greaterThanOrEqualTo(0), reason: label);
        expect(rect.right, lessThanOrEqualTo(size.width), reason: label);
      }
      // The links grow with their caption, wrapped or not, rather than
      // cutting it at 44 points (row 256); the next screen's too.
      await _linkWhole(tester, 'Paste code instead');
      await tester.tap(find.text('Paste code instead'));
      await pumpFrames(tester);
      await _linkWhole(tester, 'Sign in with your email instead');
    });
  }
}
