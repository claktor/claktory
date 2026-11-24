// Where Android's Back swipe doesn't start (Sale's row 86): the phone's
// hold-to-record mic is taken out of the system's Back gesture while it's
// on screen, and only the mic: its own box, sent to the host as the
// window's logical pixels, kept where the mic moves, given back when it
// goes, and never another's when one of several goes. Off Android nothing
// is sent, and the mic's hold and slide work through it as before.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/android/gesture_exclusion.dart';
import 'package:factory_app/mobile/voice_input.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/voice.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'voice_test.dart' show FakeRecorder, io;

Workspace _workspace() => Workspace(
  agents: [
    Agent(
      id: 'mike',
      name: 'Mike',
      role: 'Coordinator',
      modelAvailable: true,
      model: 'anthropic/claude-opus-5-5',
    ),
  ],
  models: [Model(id: 'anthropic/claude-opus-5-5', name: 'Opus 5.5')],
  selectedAgentId: 'mike',
  speechModel: SpeechModel(state: SpeechModelState.SPEECH_MODEL_STATE_READY),
);

void main() {
  const channel = MethodChannel('factory/back-gesture');
  late List<List<List<double>>> calls;

  setUp(() {
    BackGestureExclusions.instance.reset();
    calls = [];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          expect(call.method, 'exclude');
          final rects = (call.arguments as Map)['rects'] as List;
          calls.add([
            for (final rect in rects)
              [for (final n in rect as List) (n as num).toDouble()],
          ]);
          return true;
        });
  });

  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });

  /// As [platform] for this test only.
  void on(TargetPlatform platform) {
    debugDefaultTargetPlatformOverride = platform;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
  }

  /// [boxes] as positioned squares of 48, each wrapped.
  Widget screen(List<Offset> boxes, {Widget? inside}) => Directionality(
    textDirection: TextDirection.ltr,
    child: Stack(
      children: [
        for (final (i, at) in boxes.indexed)
          Positioned(
            key: ValueKey(i),
            left: at.dx,
            top: at.dy,
            child: AndroidBackGestureExclusion(
              child: SizedBox.square(dimension: 48, child: inside),
            ),
          ),
      ],
    ),
  );

  testWidgets('its box is excluded, and follows it when it moves', (
    tester,
  ) async {
    on(TargetPlatform.android);
    await tester.pumpWidget(screen([const Offset(337, 700)]));
    await tester.pump();
    expect(calls.last, [
      [337.0, 700.0, 48.0, 48.0],
    ]);

    // The keyboard comes up and the box rises.
    await tester.pumpWidget(screen([const Offset(337, 420)]));
    await tester.pump();
    expect(calls.last, [
      [337.0, 420.0, 48.0, 48.0],
    ]);

    // Nothing new: nothing sent.
    final count = calls.length;
    await tester.pump();
    await tester.pump();
    expect(calls.length, count);
  });

  testWidgets('one of several going keeps the others; the last gives the '
      'edges back', (tester) async {
    on(TargetPlatform.android);
    await tester.pumpWidget(
      screen([const Offset(337, 700), const Offset(10, 100)]),
    );
    await tester.pump();
    expect(calls.last, [
      [337.0, 700.0, 48.0, 48.0],
      [10.0, 100.0, 48.0, 48.0],
    ]);
    await tester.pumpWidget(screen([const Offset(337, 700)]));
    await tester.pump();
    expect(calls.last, [
      [337.0, 700.0, 48.0, 48.0],
    ]);
    await tester.pumpWidget(screen([]));
    await tester.pump();
    expect(calls.last, isEmpty);
  });

  testWidgets('touches reach what it wraps', (tester) async {
    on(TargetPlatform.android);
    var taps = 0;
    await tester.pumpWidget(
      screen(
        [const Offset(337, 300)],
        inside: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: () => taps++,
        ),
      ),
    );
    await tester.tapAt(const Offset(361, 324));
    expect(taps, 1);
  });

  testWidgets('off Android nothing is sent', (tester) async {
    for (final platform in [TargetPlatform.iOS, TargetPlatform.macOS]) {
      on(platform);
      await tester.pumpWidget(screen([const Offset(337, 700)]));
      await tester.pump();
      await tester.pumpWidget(screen([]));
      await tester.pump();
    }
    expect(calls, isEmpty);
  });

  group("the phone's mic", () {
    late FakeRecorder recorder;
    late VoiceRecorder was;
    setUp(() {
      voiceMessagesOn = true;
      was = deviceRecorder;
      recorder = FakeRecorder();
      deviceRecorder = recorder;
    });
    tearDown(() => deviceRecorder = was);

    final mic = find.byType(PhoneMicButton);

    Future<Harness> conversation(
      WidgetTester tester,
      TargetPlatform platform,
    ) async {
      on(platform);
      final harness = Harness(_workspace());
      harness.backend.transcript.load(Transcript(agentId: 'mike'));
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      await harness.show(tester, size: const Size(411, 891));
      await pumpFrames(tester);
      if (find.byType(AgentView).evaluate().isEmpty) {
        await tester.tap(find.text('Mike').first);
        await pumpFrames(tester);
      }
      expect(mic, findsOneWidget);
      return harness;
    }

    testWidgets('on Android, only the mic is excluded, and holding and '
        'sliding it still record and cancel', (tester) async {
      await conversation(tester, TargetPlatform.android);
      await tester.pump();
      final place = tester.getRect(mic);
      expect(calls.last, [
        [place.left, place.top, place.width, place.height],
      ]);
      // Its own target, not the composer's row.
      expect(place.width, lessThanOrEqualTo(48));
      expect(place.height, lessThanOrEqualTo(48));

      final gesture = await tester.startGesture(place.center);
      await tester.pump(const Duration(milliseconds: 250));
      await io(tester);
      expect(recorder.recordingInto, isNotNull, reason: 'recording');
      await gesture.moveBy(const Offset(-120, 0));
      await tester.pump();
      expect(find.text('Let go to cancel'), findsOneWidget);
      await gesture.up();
      await io(tester);
      expect(recorder.cancelled, 1);
    });

    testWidgets('on the iPhone it builds as before and sends nothing', (
      tester,
    ) async {
      await conversation(tester, TargetPlatform.iOS);
      await tester.pump();
      expect(calls, isEmpty);
      expect(tester.getSize(mic).width, lessThanOrEqualTo(48));
    });
  });
}
