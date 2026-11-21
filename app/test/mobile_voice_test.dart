// Voice messages on an iPhone (#197 slice 5; research voice.md and
// mobile.md "Voice memo placement"): the mic in Send's place while the box
// is empty; hold to record, let go to send, slide toward the start to
// cancel; a tap records with Send and Cancel, which VoiceOver uses too; a
// call pauses it rather than sending; the microphone turned off offers
// Settings.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/mobile/composer.dart' show SendButton;
import 'package:factory_app/mobile/voice_input.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/voice.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'voice_test.dart' show FakeRecorder, io;

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Claktory', path: '/w')],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
    Agent(
      id: 'mike',
      projectId: 'p1',
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
  late FakeRecorder recorder;
  late VoiceRecorder was;
  setUp(() {
    voiceMessagesOn = true;
    was = deviceRecorder;
    recorder = FakeRecorder();
    deviceRecorder = recorder;
  });
  tearDown(() => deviceRecorder = was);

  /// An iPhone 16 with Mike's conversation open.
  Future<Harness> conversation(WidgetTester tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace());
    harness.backend.transcript.load(Transcript(agentId: 'mike'));
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    expect(find.byType(PhoneShell), findsOneWidget);
    if (find.byType(AgentView).evaluate().isEmpty) {
      await tester.tap(find.text('Mike').first);
      await pumpFrames(tester);
    }
    expect(find.byType(AgentView), findsOneWidget);
    return harness;
  }

  final mic = find.byType(PhoneMicButton);
  Iterable<SendMessage> sent(Harness harness) => harness.backend.sent
      .where((m) => m.hasSendMessage())
      .map((m) => m.sendMessage);

  /// Holds the mic until recording, then [during] with the finger down.
  Future<TestGesture> hold(WidgetTester tester) async {
    final gesture = await tester.startGesture(tester.getCenter(mic));
    await tester.pump(const Duration(milliseconds: 250));
    await io(tester);
    expect(recorder.recordingInto, isNotNull, reason: 'recording');
    return gesture;
  }

  testWidgets("the mic takes Send's place while there's nothing to send", (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    await conversation(tester);
    expect(mic, findsOneWidget);
    expect(tester.getSize(mic), const Size(44, 44));
    expect(find.bySemanticsLabel('Record a voice message'), findsOneWidget);
    expect(find.byType(SendButton), findsNothing);
    await tester.enterText(find.byType(EditableText).last, 'Hi');
    await pumpFrames(tester);
    expect(mic, findsNothing);
    expect(find.byType(SendButton), findsOneWidget);
    semantics.dispose();
  });

  testWidgets('hold to record, let go to send', (tester) async {
    final harness = await conversation(tester);
    final gesture = await hold(tester);
    expect(find.byType(PhoneRecordingRow), findsOneWidget);
    expect(find.text('‹ Slide to cancel'), findsOneWidget);
    expect(find.byType(EditableText), findsNothing, reason: 'the row instead');
    recorder.hear(0.5, const Duration(milliseconds: 2300));
    await tester.pump();
    expect(find.text('0:02'), findsOneWidget);

    await gesture.up();
    await io(tester);
    final message = sent(harness).single;
    expect(message.voice, isTrue);
    expect(message.text, '');
    expect(message.agentId, 'mike');
    expect(message.attachments.single.name, endsWith('.m4a'));
    expect(find.byType(PhoneRecordingRow), findsNothing);
    expect(mic, findsOneWidget);
  });

  testWidgets('slide toward the start to cancel', (tester) async {
    final harness = await conversation(tester);
    final gesture = await hold(tester);
    recorder.hear(0.5, const Duration(seconds: 2));
    await gesture.moveBy(const Offset(-60, 0));
    await tester.pump();
    expect(find.text('‹ Slide to cancel'), findsOneWidget, reason: 'not yet');
    await gesture.moveBy(const Offset(-60, 0));
    await tester.pump();
    expect(find.text('Let go to cancel'), findsOneWidget);
    // Back again: it would send.
    await gesture.moveBy(const Offset(60, 0));
    await tester.pump();
    expect(find.text('‹ Slide to cancel'), findsOneWidget);
    await gesture.moveBy(const Offset(-80, 0));
    await tester.pump();
    await gesture.up();
    await io(tester);
    expect(recorder.cancelled, 1);
    expect(sent(harness), isEmpty);
    expect(find.byType(PhoneRecordingRow), findsNothing);
  });

  testWidgets('a hold too short to say anything sends nothing, and says how', (
    tester,
  ) async {
    final harness = await conversation(tester);
    final gesture = await hold(tester);
    await gesture.up();
    await io(tester);
    expect(sent(harness), isEmpty);
    expect(recorder.cancelled, 1);
    expect(find.text('Hold to record, let go to send.'), findsOneWidget);
  });

  testWidgets('a tap records with Send and Cancel, nothing held', (
    tester,
  ) async {
    final harness = await conversation(tester);
    await tester.tap(mic);
    await io(tester);
    expect(recorder.recordingInto, isNotNull);
    expect(find.byType(PhoneRecordingRow), findsOneWidget);
    expect(find.text('‹ Slide to cancel'), findsNothing);
    final cancel = find.bySemanticsLabel('Cancel');
    expect(tester.getSize(cancel).height, greaterThanOrEqualTo(44));
    recorder.hear(0.5, const Duration(seconds: 3));
    await tester.pump();
    await tester.tap(find.byType(SendButton));
    await io(tester);
    expect(sent(harness).single.voice, isTrue);

    // And Cancel throws one away.
    await tester.tap(mic);
    await io(tester);
    await tester.tap(find.text('Cancel'));
    await io(tester);
    expect(recorder.cancelled, 1);
    expect(sent(harness), hasLength(1));
    expect(find.byType(PhoneRecordingRow), findsNothing);
  });

  testWidgets('a call pauses it: it waits for Send or Cancel', (tester) async {
    final harness = await conversation(tester);
    final gesture = await hold(tester);
    recorder.hear(0.5, const Duration(seconds: 4));
    recorder.end(RecordingEnd.interrupted);
    await io(tester);
    expect(find.text('Paused'), findsOneWidget);
    // Letting go isn't choosing to send.
    await gesture.up();
    await io(tester);
    expect(sent(harness), isEmpty);
    await tester.tap(find.byType(SendButton));
    await io(tester);
    expect(sent(harness).single.voice, isTrue);
  });

  testWidgets('the system taking the finger keeps it, with Send and Cancel', (
    tester,
  ) async {
    final harness = await conversation(tester);
    final gesture = await hold(tester);
    recorder.hear(0.5, const Duration(seconds: 2));
    await gesture.cancel();
    await io(tester);
    expect(sent(harness), isEmpty);
    expect(recorder.cancelled, 0);
    expect(find.byType(PhoneRecordingRow), findsOneWidget);
    expect(find.text('Cancel'), findsOneWidget);
  });

  testWidgets('at ten minutes it sends what was said', (tester) async {
    final harness = await conversation(tester);
    await hold(tester);
    recorder.hear(0.5, const Duration(minutes: 10));
    recorder.end(RecordingEnd.limit);
    await io(tester);
    expect(sent(harness).single.voice, isTrue);
  });

  testWidgets('the microphone turned off offers Settings', (tester) async {
    recorder.granted = MicAccess.denied;
    await conversation(tester);
    await tester.tap(mic);
    await io(tester);
    expect(recorder.recordingInto, isNull);
    expect(
      find.text("To record, allow microphone access in Settings → Claktory."),
      findsOneWidget,
    );
    expect(find.text('Open Settings'), findsOneWidget);
    expect(find.byType(PhoneRecordingRow), findsNothing);
  });
}
