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
import 'pair_start.dart' show pastPairStart;

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
        'Claktory runs on a Mac or a Linux server (or Windows, in '
        'preview). Set it up there, then '
        'connect this '
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
      'Set up Claktory on your Mac or Linux server (or Windows, in '
          'preview): '
          'https://claktory.com/download',
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

  testWidgets('on Android: a Mac or a Linux server, or Windows in preview, and that this phone '
      "doesn't get notifications yet", (tester) async {
    await _show(tester, TargetPlatform.android, camera: CameraAccess.granted);
    expect(
      find.text(
        'Claktory runs on a Mac or a Linux server (or Windows, in '
        'preview). Set it up there, then '
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

  testWidgets(
    'on Android the link says a Mac or a Linux server, or Windows in preview',
    (tester) async {
      final shared = _shared(tester);
      PhonePlatformPolicy.debugAndroidPolicy = true;
      addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
      await _show(tester, TargetPlatform.android, camera: CameraAccess.granted);
      await tester.tap(find.text('Send me the link'));
      await pumpFrames(tester);
      expect(shared, [
        'Set up Claktory on your Mac or Linux server (or Windows, in '
            'preview): '
            'https://claktory.com/download',
      ]);
    },
  );

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
        'Claktory runs on a Mac or a Linux server (or Windows, in '
        'preview). Set it up there, then '
        'connect this '
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

  testWidgets('a phone with a Claktory already, adding another, signed '
      'out: straight to the email sign-in, the code a link away (area 2, '
      '8.2)', (tester) async {
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
    // Your account first, a code second: never the scanner first.
    expect(find.byKey(const ValueKey('device-email')), findsOneWidget);
    expect(find.text('Use a code instead'), findsOneWidget);
    await tester.tap(find.text('Use a code instead'));
    await pumpFrames(tester);
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

  testWidgets('at the largest text in a 320-point window, the title wraps '
      'between words, and Paste code instead opens at its top (Sale, '
      '#363 F3)', (tester) async {
    tester.platformDispatcher.textScaleFactorTestValue = 3.12;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await _show(
      tester,
      TargetPlatform.iOS,
      camera: CameraAccess.unavailable,
      size: const Size(320, 1146),
    );
    expect(tester.takeException(), isNull);
    // Every word of the title on one line of its own or shared, never cut.
    const title = 'Connect to your Claktory';
    final paragraph = tester.renderObject<RenderParagraph>(find.text(title));
    var at = 0;
    for (final word in title.split(' ')) {
      final boxes = paragraph.getBoxesForSelection(
        TextSelection(baseOffset: at, extentOffset: at + word.length),
      );
      expect(boxes, hasLength(1), reason: word);
      at += word.length + 1;
    }
    // From the end of the first screen, the paste screen starts at its top.
    await tester.ensureVisible(find.text('Paste code instead'));
    await pumpFrames(tester);
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester);
    expect(find.textContaining('Paste the code for this iPhone.'), findsOne);
    expect(tester.getRect(find.bySemanticsLabel('Back')).top, greaterThan(0));
    expect(tester.getRect(find.text(title)).top, greaterThan(0));
  });

  testWidgets('"Paste code instead" under Continue is a node of its own, '
      "the link's size: TalkBack frames it, not the whole screen "
      "(Sale's row 195)", (tester) async {
    final semantics = tester.ensureSemantics();
    await _show(tester, TargetPlatform.android, size: const Size(411, 914));
    await pastPairStart(tester);
    final link = find.bySemanticsLabel('Paste code instead').last;
    expect(tester.getSemantics(link).rect.height, lessThan(100));
    final continueNode = tester.getSemantics(find.bySemanticsLabel('Continue'));
    expect(continueNode.parent?.label, isNot('Paste code instead'));
    semantics.dispose();
  });
}
