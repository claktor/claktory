// Pairing an Android phone: it scans the code when it has a camera the app
// may use, and opens on pasting when it has none; either way its words name
// no iPhone or Mac: the code comes from a device already connected, or from
// where the Claktory was set up (a server, often). The iPhone keeps its own
// words.
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'pair_start.dart';

Future<Harness> _show(
  WidgetTester tester,
  TargetPlatform platform, {
  CameraAccess? camera,
}) async {
  debugDefaultTargetPlatformOverride = platform;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(Workspace(), localBackend: false);
  harness.camera.state =
      camera ??
      (platform == TargetPlatform.android
          ? CameraAccess.unavailable
          : CameraAccess.granted);
  await harness.show(tester, size: const Size(393, 852));
  await pumpFrames(tester);
  return harness;
}

void main() {
  testWidgets('on Android with no camera: straight to pasting, in words '
      'for a server', (tester) async {
    await _show(tester, TargetPlatform.android);
    await pastPairStart(tester, paste: true);
    expect(find.textContaining('Paste the code for this phone'), findsOne);
    expect(find.textContaining('iPhone'), findsNothing);
    expect(find.textContaining('Mac'), findsNothing);
    expect(find.textContaining('scan'), findsNothing);
    expect(find.byType(EditableText), findsOneWidget);

    await tester.enterText(find.byType(EditableText), 'not a code');
    await pumpFrames(tester);
    expect(
      find.text(
        "That isn't a Claktory code. Copy it again, all of it, and paste it "
        'here.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('on Android with the camera allowed: scanning, in words for '
      'a server, with pasting a tap away', (tester) async {
    await _show(tester, TargetPlatform.android, camera: CameraAccess.granted);
    await pastPairStart(tester);
    expect(find.textContaining('Scan the code for this phone'), findsOne);
    expect(find.textContaining('iPhone'), findsNothing);
    expect(find.textContaining('Mac'), findsNothing);
    expect(find.byType(EditableText), findsNothing);
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester);
    expect(find.byType(EditableText), findsOneWidget);
  });

  testWidgets(
    "on Android, a scanned code that isn't Claktory's says which to scan",
    (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.android;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      // A phone's display, so it's "this phone" (a tablet's says tablet).
      tester.view.display.size = const Size(1080, 2400);
      tester.view.display.devicePixelRatio = 3;
      addTearDown(tester.view.display.reset);
      expect(
        pairingProblem('WIFI:S:home;;', scanned: true),
        "That isn't a Claktory code. Scan the one shown for this phone in "
        'Settings → Devices → Add a device.',
      );
    },
  );

  testWidgets('on Android, a first no asks again; a second sends the '
      'person to Settings', (tester) async {
    final harness = await _show(
      tester,
      TargetPlatform.android,
      camera: CameraAccess.undetermined,
    );
    harness.camera
      ..granting = false
      ..afterRefusal = CameraAccess.undetermined;
    // The first screen's way to scan says why first; Continue asks.
    await tester.tap(find.text('Scan code instead'));
    await pumpFrames(tester);
    expect(harness.camera.asked, 0);
    expect(find.text('Scan the code'), findsOneWidget);
    await tester.tap(find.text('Continue'));
    await pumpFrames(tester);
    expect(harness.camera.asked, 1);
    // Android can ask once more: why again, with Continue.
    expect(find.text('Continue'), findsOneWidget);
    expect(find.text('Open Settings'), findsNothing);

    harness.camera.afterRefusal = CameraAccess.denied;
    await tester.tap(find.text('Continue'));
    await pumpFrames(tester);
    expect(harness.camera.asked, 2);
    expect(find.text('The camera is off for Claktory'), findsOneWidget);
    expect(find.text('Open Settings'), findsOneWidget);
    expect(find.text('Continue'), findsNothing);
  });

  testWidgets('on an iPhone, a no sends the person to Settings at once', (
    tester,
  ) async {
    final harness = await _show(
      tester,
      TargetPlatform.iOS,
      camera: CameraAccess.undetermined,
    );
    harness.camera.granting = false;
    await tester.tap(find.text('Scan code instead'));
    await pumpFrames(tester);
    await tester.tap(find.text('Continue'));
    await pumpFrames(tester);
    expect(harness.camera.asked, 1);
    expect(find.text('Open Settings'), findsOneWidget);
    expect(
      find.text(
        "Turn it on in Settings on this iPhone, or paste the code instead.",
      ),
      findsOneWidget,
    );
  });

  testWidgets('on Android with the camera refused: Settings, in its words', (
    tester,
  ) async {
    await _show(tester, TargetPlatform.android, camera: CameraAccess.denied);
    await pastPairStart(tester);
    expect(
      find.text(
        "Turn it on in Android's Settings, or paste the code instead.",
      ),
      findsOneWidget,
    );
    expect(find.text('Open Settings'), findsOneWidget);
    expect(find.textContaining('Mac'), findsNothing);
  });

  testWidgets('on iPhone: its own words, scanning first', (tester) async {
    await _show(tester, TargetPlatform.iOS);
    await pastPairStart(tester);
    expect(find.textContaining('On your Mac'), findsOne);
    expect(find.textContaining('Paste the code for this phone'), findsNothing);
  });
}
