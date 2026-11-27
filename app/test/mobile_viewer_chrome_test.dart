// A viewer's page on a phone (a landscape video once showed less video
// than portrait): a picture or video fills the screen
// with its chrome over it; a tap hides or shows the chrome, a playing
// video fades it, placing a comment and VoiceOver keep it; a pull down
// closes the page; turning the phone keeps the video playing; a
// document's header slides away while it's read.
import 'package:factory_app/mobile/viewer_chrome.dart';
import 'package:factory_app/pane_chrome.dart';
import 'package:factory_app/viewers/video.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'mobile_viewer_shots_test.dart';
import 'video_test.dart' show FakePlayback;

/// Whether the bar keyed [key] shows (its fade's opacity above 0).
bool _shows(WidgetTester tester, Key key) {
  final bar = find.byKey(key);
  if (bar.evaluate().isEmpty) return false;
  return tester
      .widgetList<AnimatedOpacity>(
        find.ancestor(of: bar, matching: find.byType(AnimatedOpacity)),
      )
      .every((fade) => fade.opacity > 0);
}

Future<void> _fade(WidgetTester tester) async {
  await tester.pump(const Duration(milliseconds: 400));
  await pumpFrames(tester);
}

void main() {
  setUp(() {
    FakePlayback.made.clear();
    newVideoPlayback = CardPlayback.new;
  });

  testWidgets('a landscape video fills the screen\'s height, its header and '
      'controls over it; a tap hides them and another brings them back', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    await openOnPhone(tester, ViewerKind.video, phoneLandscape);
    final frame = tester.getRect(find.byKey(videoFrame));
    expect(frame.height, 393);
    expect(frame.width, closeTo(393 * 16 / 9, 1));
    expect(_shows(tester, viewerTopBar), isTrue);
    expect(_shows(tester, viewerFootBar), isTrue);
    // Inside the safe areas: clear of the notch, over the home indicator.
    final back = tester.getRect(find.bySemanticsLabel('Back').last);
    expect(back.left, greaterThanOrEqualTo(59));
    expect(
      tester.getRect(find.bySemanticsLabel('Play').last).bottom,
      lessThanOrEqualTo(393 - 21),
    );

    await tester.tapAt(frame.center + const Offset(0, 80));
    await _fade(tester);
    expect(_shows(tester, viewerTopBar), isFalse);
    expect(_shows(tester, viewerFootBar), isFalse);
    // Hidden, they take no taps and VoiceOver doesn't find them.
    expect(find.semantics.byLabel('Back'), findsNothing);

    await tester.tapAt(frame.center + const Offset(0, 80));
    await _fade(tester);
    expect(_shows(tester, viewerTopBar), isTrue);
    expect(find.semantics.byLabel('Back'), findsOne);
    semantics.dispose();
  });

  testWidgets('playing, its controls fade by themselves; paused, they stay', (
    tester,
  ) async {
    await openOnPhone(tester, ViewerKind.video, phonePortrait);
    await tester.pump(const Duration(seconds: 4));
    expect(_shows(tester, viewerFootBar), isTrue, reason: 'paused');
    await tester.tap(find.bySemanticsLabel('Play').last);
    await tester.pump();
    expect(FakePlayback.made.last.state.value.playing, isTrue);
    await tester.pump(const Duration(seconds: 2));
    expect(_shows(tester, viewerFootBar), isTrue);
    await tester.pump(const Duration(seconds: 2));
    await _fade(tester);
    expect(_shows(tester, viewerFootBar), isFalse);
    expect(_shows(tester, viewerTopBar), isFalse);
  });

  testWidgets('turned while it plays, the video plays on in the same player', (
    tester,
  ) async {
    await openOnPhone(tester, ViewerKind.video, phonePortrait);
    await tester.tap(find.bySemanticsLabel('Play').last);
    await tester.pump();
    final player = FakePlayback.made.single;
    tester.view.padding = phoneLandscape.padding;
    tester.view.physicalSize =
        phoneLandscape.size * tester.view.devicePixelRatio;
    await pumpFrames(tester);
    expect(FakePlayback.made.single, same(player));
    expect(player.state.value.playing, isTrue);
    expect(tester.getRect(find.byKey(videoFrame)).height, 393);
  });

  testWidgets('placing a comment keeps the controls and the bar that says '
      'how, and a pull down does nothing meanwhile', (tester) async {
    await openOnPhone(tester, ViewerKind.video, phonePortrait);
    await tester.tap(find.text('Comment'));
    await _fade(tester);
    expect(find.text('Cancel'), findsOneWidget);
    final frame = tester.getRect(find.byKey(videoFrame));
    await tester.tapAt(frame.center + const Offset(0, 60));
    await _fade(tester);
    expect(_shows(tester, viewerFootBar), isTrue);
    await tester.dragFrom(frame.center, const Offset(0, 400));
    await pumpFrames(tester);
    expect(find.byType(PhoneViewerScaffold), findsOneWidget);
  });

  testWidgets('pulled down, a video closes; the conversation is back', (
    tester,
  ) async {
    await openOnPhone(tester, ViewerKind.video, phonePortrait);
    final frame = tester.getRect(find.byKey(videoFrame));
    await tester.timedDragFrom(
      frame.center,
      const Offset(0, 320),
      const Duration(milliseconds: 300),
    );
    await pumpFrames(tester);
    expect(find.byType(PhoneViewerScaffold), findsNothing);
  });

  testWidgets('a picture zoomed in pans; fitted, a pull down closes it', (
    tester,
  ) async {
    await openOnPhone(tester, ViewerKind.image, phonePortrait);
    final picture = tester.getRect(find.byType(RawImage).last);
    // Zoomed in with a double tap: a drag moves it, the page stays.
    await tester.tapAt(picture.center);
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tapAt(picture.center);
    await pumpFrames(tester);
    await tester.dragFrom(picture.center, const Offset(0, 300));
    await pumpFrames(tester);
    expect(find.byType(PhoneViewerScaffold), findsOneWidget);
    // Fitted again: a pull down closes it.
    await tester.tapAt(tester.getCenter(find.byType(PhoneViewerScaffold)));
    await tester.pump(const Duration(milliseconds: 50));
    await tester.tapAt(tester.getCenter(find.byType(PhoneViewerScaffold)));
    await pumpFrames(tester);
    await tester.timedDragFrom(
      tester.getCenter(find.byType(PhoneViewerScaffold)),
      const Offset(0, 320),
      const Duration(milliseconds: 300),
    );
    await pumpFrames(tester);
    expect(find.byType(PhoneViewerScaffold), findsNothing);
  });

  testWidgets('fitted, two fingers zoom a picture', (tester) async {
    await openOnPhone(tester, ViewerKind.image, phonePortrait);
    final picture = tester.getRect(find.byType(RawImage).last);
    // A pinch first: it zooms, and the page stays.
    final a = await tester.startGesture(picture.center - const Offset(20, 0));
    final b = await tester.startGesture(picture.center + const Offset(20, 0));
    for (var i = 0; i < 5; i++) {
      await a.moveBy(const Offset(-15, 0));
      await b.moveBy(const Offset(15, 0));
      await tester.pump(const Duration(milliseconds: 16));
    }
    await a.up();
    await b.up();
    await pumpFrames(tester);
    expect(
      tester.getSize(find.byType(RawImage).last).width,
      greaterThan(picture.width * 1.5),
    );
    expect(find.byType(PhoneViewerScaffold), findsOneWidget);
  });

  // Pinched smaller than fitting, it stayed so;
  // as in Photos, let go it goes back to fitted.
  testWidgets('pinched smaller than fitting, it springs back', (tester) async {
    await openOnPhone(tester, ViewerKind.image, phonePortrait);
    final picture = tester.getRect(find.byType(RawImage).last);
    final a = await tester.startGesture(picture.center - const Offset(80, 0));
    final b = await tester.startGesture(picture.center + const Offset(80, 0));
    for (var i = 0; i < 5; i++) {
      await a.moveBy(const Offset(12, 0));
      await b.moveBy(const Offset(-12, 0));
      await tester.pump(const Duration(milliseconds: 16));
    }
    expect(
      tester.getRect(find.byType(RawImage).last).width,
      lessThan(picture.width * 0.8),
    );
    await a.up();
    await b.up();
    await pumpFrames(tester);
    final settled = tester.getRect(find.byType(RawImage).last);
    expect(settled.width, closeTo(picture.width, 0.5));
    expect(settled.center.dy, closeTo(picture.center.dy, 0.5));
  });

  // On Android, a fast pull, its first move past both the
  // pull's slop and the viewer's pan's at once, went to the pan, which
  // didn't move the fitted picture: the page never closed.
  testWidgets('fitted, a pull in big steps closes a picture', (tester) async {
    await openOnPhone(tester, ViewerKind.image, phonePortrait);
    final picture = tester.getRect(find.byType(RawImage).last);
    final pull = await tester.startGesture(picture.center);
    for (var i = 0; i < 10; i++) {
      await pull.moveBy(const Offset(0, 40));
      await tester.pump(const Duration(milliseconds: 16));
    }
    await pull.up();
    await pumpFrames(tester);
    expect(find.byType(PhoneViewerScaffold), findsNothing);
  });

  // On Android, hidden, the first swipe from the edge
  // only brings the system's bars back; the chrome comes with them, so
  // the next swipe or Back closes it.
  testWidgets("the system's bars swiped back bring the chrome", (tester) async {
    final calls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        calls.add(call);
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    await openOnPhone(tester, ViewerKind.video, phonePortrait);
    final frame = tester.getRect(find.byKey(videoFrame));
    await tester.tapAt(frame.center + const Offset(0, 60));
    await _fade(tester);
    expect(_shows(tester, viewerTopBar), isFalse);
    expect(
      calls
          .where((c) => c.method == 'SystemChrome.setEnabledSystemUIMode')
          .last
          .arguments,
      'SystemUiMode.immersive',
    );
    await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
      SystemChannels.platform.name,
      SystemChannels.platform.codec.encodeMethodCall(
        const MethodCall('SystemChrome.systemUIChange', [true]),
      ),
      (_) {},
    );
    await _fade(tester);
    expect(_shows(tester, viewerTopBar), isTrue);
  });

  // Android's hint says to swipe down from the top to
  // bring its bars back; a long such swipe closed the video.
  testWidgets('a swipe down from the top edge never closes it', (tester) async {
    await openOnPhone(tester, ViewerKind.video, phonePortrait);
    final frame = tester.getRect(find.byKey(videoFrame));
    await tester.tapAt(frame.center + const Offset(0, 60));
    await _fade(tester);
    expect(_shows(tester, viewerTopBar), isFalse);
    await tester.timedDragFrom(
      const Offset(196, 20),
      const Offset(0, 600),
      const Duration(milliseconds: 400),
    );
    await pumpFrames(tester);
    expect(find.byType(PhoneViewerScaffold), findsOneWidget);
    // From lower down, the same pull closes it.
    await tester.timedDragFrom(
      const Offset(196, 120),
      const Offset(0, 600),
      const Duration(milliseconds: 400),
    );
    await pumpFrames(tester);
    expect(find.byType(PhoneViewerScaffold), findsNothing);
  });

  testWidgets('with VoiceOver on, the chrome never hides', (tester) async {
    tester.platformDispatcher.accessibilityFeaturesTestValue =
        const FakeAccessibilityFeatures(accessibleNavigation: true);
    addTearDown(tester.platformDispatcher.clearAccessibilityFeaturesTestValue);
    await openOnPhone(tester, ViewerKind.video, phonePortrait);
    final frame = tester.getRect(find.byKey(videoFrame));
    await tester.tapAt(frame.center + const Offset(0, 60));
    await _fade(tester);
    expect(_shows(tester, viewerTopBar), isTrue);
    expect(_shows(tester, viewerFootBar), isTrue);
  });

  testWidgets('a phone in landscape has the header\'s comments button, not '
      'the side cards a wide pane has', (tester) async {
    await openOnPhone(tester, ViewerKind.image, phoneLandscape);
    expect(
      find.descendant(
        of: find.byType(PhonePaneHeader),
        matching: find.bySemanticsLabel(RegExp('[Cc]omments')),
      ),
      findsOneWidget,
    );
  });

  testWidgets('at the largest text on a small iPhone, a video\'s controls '
      'fit in one row with room for its timeline', (tester) async {
    tester.platformDispatcher.textScaleFactorTestValue = 3.12;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await openOnPhone(tester, ViewerKind.video, (
      size: const Size(375, 667),
      padding: const FakeViewPadding(top: 20),
    ));
    expect(tester.takeException(), isNull);
    expect(
      tester.getSize(find.byType(Timeline)).width,
      greaterThanOrEqualTo(44),
    );
    expect(
      tester.getSize(find.bySemanticsLabel('Play').last).height,
      greaterThanOrEqualTo(44),
    );
  });

  testWidgets('read down, a document\'s header slides away; scrolled back '
      'up, it returns', (tester) async {
    await openOnPhone(tester, ViewerKind.text, phonePortrait);
    double header() => tester
        .widget<AnimatedAlign>(
          find.ancestor(
            of: find.byType(PhonePaneHeader),
            matching: find.byType(AnimatedAlign),
          ),
        )
        .heightFactor!;
    expect(header(), 1);
    final body = tester.getCenter(find.byType(PhoneViewerScaffold));
    await tester.dragFrom(body, const Offset(0, -300));
    await _fade(tester);
    expect(header(), 0);
    await tester.dragFrom(body, const Offset(0, 120));
    await _fade(tester);
    expect(header(), 1);
  });

  test('the chrome: a hold keeps it, fading waits for the last touch', () {
    final chrome = ViewerChrome(fadeAfter: Duration.zero);
    expect(chrome.shown, isTrue);
    chrome.toggle();
    expect(chrome.shown, isFalse);
    chrome.hold(#comment, true);
    expect(chrome.shown, isTrue);
    chrome.hide();
    expect(chrome.shown, isTrue);
    chrome.hold(#comment, false);
    chrome.keepStill(#zoom, true);
    expect(chrome.canPull, isFalse);
    chrome.keepStill(#zoom, false);
    expect(chrome.canPull, isTrue);
    chrome.dispose();
  });

  tearDown(() => debugDefaultTargetPlatformOverride = null);
}
