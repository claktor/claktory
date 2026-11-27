// Scanning a code with the camera wherever one is entered (friend bug #2):
// scanCode says why before the camera's first use, then asks (area 5a,
// P7), takes the first code its check
// accepts, says why another can't be used, and says how to allow a refused
// camera, with Open Settings, starting once it's allowed.
import 'dart:async';

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/scan_code.dart';
import 'package:factory_app/scanner.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'phone_test.dart' show phone, workspace;

String? _check(String text) =>
    text.startsWith('code:') ? null : "That isn't a code for this.";

/// Opens the scanner from the conversation; its result completes [result].
Future<Completer<String?>> _scan(
  WidgetTester tester, {
  VoidCallback? onPasteInstead,
}) async {
  final result = Completer<String?>();
  final context = tester.element(find.byType(AgentView).first);
  unawaited(
    scanCode(
      context,
      title: 'Scan a code',
      check: _check,
      onPasteInstead: onPasteInstead,
    ).then(result.complete),
  );
  await pumpFrames(tester);
  return result;
}

void main() {
  testWidgets('on a phone it asks for the camera, says why a code is wrong '
      'and closes with the first right one', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace());
    harness.camera.state = CameraAccess.undetermined;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    final result = await _scan(tester);
    // Why first (area 5a, P7): nothing asked until Continue.
    expect(harness.camera.asked, 0);
    expect(find.text('Scan the code'), findsOneWidget);
    // Any code, not only a Claktory's own (Zucchero 44139).
    expect(
      find.text(
        'Claktory uses the camera to read the code. '
        'Your iPhone asks next.',
      ),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('camera')), findsNothing);
    await tester.tap(find.text('Continue'));
    await pumpFrames(tester);
    expect(harness.camera.asked, 1);
    expect(find.text('Scan a code'), findsOneWidget);
    expect(find.byKey(const ValueKey('camera')), findsOneWidget);
    // Square, at most half the screen's height.
    final picture = tester.getSize(find.byKey(const ValueKey('camera')));
    expect(picture.height, lessThanOrEqualTo(phone.height / 2));

    harness.camera.scan('nonsense');
    await pumpFrames(tester);
    expect(find.text("That isn't a code for this."), findsOneWidget);
    expect(result.isCompleted, isFalse);

    harness.camera.scan('  code:42 ');
    harness.camera.scan('code:43');
    await pumpFrames(tester);
    expect(await result.future, 'code:42');
    expect(find.text('Scan a code'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Close on a phone closes it with nothing', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace());
    harness.camera.state = CameraAccess.granted;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    final result = await _scan(tester);
    await tester.tap(find.bySemanticsLabel('Close').last);
    await pumpFrames(tester);
    expect(await result.future, isNull);
    expect(find.text('Scan a code'), findsNothing);
  });

  testWidgets('a refused camera says how to allow it, with Open Settings, '
      'and starts once allowed and back', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace());
    harness.camera.state = CameraAccess.denied;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    final result = await _scan(tester);
    expect(harness.camera.asked, 0);
    expect(find.text('The camera is off for Claktory'), findsOneWidget);
    // No Paste code instead here, so the words don't offer it.
    expect(find.text("Turn it on in Settings on this iPhone."), findsOneWidget);
    await tester.tap(find.text('Open Settings'));
    await pumpFrames(tester);
    expect(harness.camera.settingsOpened, 1);
    // A finger's size, both.
    final settings = find.ancestor(
      of: find.text('Open Settings'),
      matching: find.byType(Button),
    );
    expect(tester.getSize(settings).height, greaterThanOrEqualTo(44));
    expect(
      tester.getSize(find.bySemanticsLabel('Not now')).height,
      greaterThanOrEqualTo(44),
    );

    // Allowed in Settings, and back in the app.
    harness.camera.state = CameraAccess.granted;
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('camera')), findsOneWidget);
    harness.camera.scan('code:7');
    await pumpFrames(tester);
    expect(await result.future, 'code:7');
  });

  testWidgets('Back from a refused camera closes it with nothing', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace());
    harness.camera.state = CameraAccess.denied;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    final result = await _scan(tester);
    expect(find.text("Turn it on in Android's Settings."), findsOneWidget);
    await tester.tap(find.text('Not now'));
    await pumpFrames(tester);
    expect(await result.future, isNull);
  });

  testWidgets('on an iPad it is a dialog, with Cancel', (tester) async {
    asIPad();
    final harness = Harness(workspace());
    harness.camera.state = CameraAccess.granted;
    await harness.show(tester, size: const Size(1180, 820));
    await pumpFrames(tester);
    final result = await _scan(tester);
    expect(find.text('Scan a code'), findsOneWidget);
    expect(find.byKey(const ValueKey('camera')), findsOneWidget);
    harness.camera.scan('code:9');
    await pumpFrames(tester);
    expect(await result.future, 'code:9');

    final again = await _scan(tester);
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(await again.future, isNull);
    expect(tester.takeException(), isNull);
  });

  testWidgets('refused, with a field to paste in: Paste code instead '
      'closes it and pastes', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace());
    harness.camera.state = CameraAccess.denied;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    var pasted = 0;
    final result = await _scan(tester, onPasteInstead: () => pasted++);
    expect(
      find.text(
        "Turn it on in Settings on this iPhone, or paste the code instead.",
      ),
      findsOneWidget,
    );
    expect(find.text('Not now'), findsNothing);
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester);
    expect(await result.future, isNull);
    expect(pasted, 1);
  });

  testWidgets('not asked yet, with a field to paste in: Paste code instead '
      "is under Continue from the start (Sale's row 189c)", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace());
    harness.camera.state = CameraAccess.undetermined;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    var pasted = 0;
    final result = await _scan(tester, onPasteInstead: () => pasted++);
    expect(find.text('Continue'), findsOneWidget);
    final paste = find.text('Paste code instead');
    expect(paste, findsOneWidget);
    // Under Continue.
    expect(
      tester.getTopLeft(paste).dy,
      greaterThan(tester.getTopLeft(find.text('Continue')).dy),
    );
    await tester.tap(paste);
    await pumpFrames(tester);
    expect(harness.camera.asked, 0);
    expect(await result.future, isNull);
    expect(pasted, 1);
  });

  testWidgets('on an iPad, the camera kept from the app: Cancel and Paste '
      'code instead, until it comes back', (tester) async {
    asIPad();
    final harness = Harness(workspace());
    harness.camera.state = CameraAccess.granted;
    await harness.show(tester, size: const Size(1180, 820));
    await pumpFrames(tester);
    var pasted = 0;
    final result = await _scan(tester, onPasteInstead: () => pasted++);
    expect(find.text('Paste code instead'), findsNothing);
    final camera = tester.element(find.byKey(const ValueKey('camera')));
    const CameraAvailability('Make Claktory full screen').dispatch(camera);
    await pumpFrames(tester);
    expect(find.text('Paste code instead'), findsOneWidget);
    expect(find.text('Cancel'), findsOneWidget);
    const CameraAvailability(null).dispatch(camera);
    await pumpFrames(tester);
    expect(find.text('Paste code instead'), findsNothing);
    const CameraAvailability('Make Claktory full screen').dispatch(camera);
    await pumpFrames(tester);
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester);
    expect(await result.future, isNull);
    expect(pasted, 1);
  });

  testWidgets('a field with no Paste of its own: what the clipboard has '
      'goes in, then it reads it', (tester) async {
    var clipboard = 'code:5';
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async => call.method == 'Clipboard.getData'
          ? <String, Object?>{'text': clipboard}
          : null,
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    final field = TextEditingController();
    addTearDown(field.dispose);
    var read = 0;
    pasteCodeInto(field, then: () => read++)();
    await tester.pump();
    expect(field.text, 'code:5');
    expect(read, 1);
    clipboard = '  ';
    pasteCodeInto(field, then: () => read++)();
    await tester.pump();
    expect(field.text, 'code:5', reason: 'nothing to paste: left as it was');
    expect(read, 1);
  });

  testWidgets('Scan is offered with a camera, even a refused one, and not '
      'without one or where it is restricted', (tester) async {
    final harness = Harness(workspace());
    await harness.show(tester, size: phone);
    final context = tester.element(find.byType(AgentView).first);
    final offers = <CameraAccess, bool>{};
    for (final access in CameraAccess.values) {
      harness.camera.state = access;
      offers[access] = await canOfferScan(context);
    }
    expect(offers, {
      CameraAccess.granted: true,
      CameraAccess.undetermined: true,
      CameraAccess.denied: true,
      CameraAccess.restricted: false,
      CameraAccess.unavailable: false,
    });
  });
}
