import 'dart:async';
import 'dart:io';

import 'package:factory_app/audio.dart';
import 'package:factory_app/controls.dart' show ExtensionGlyph;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/screens.dart' show SettingsScroller;
import 'package:factory_app/voice.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart' show EditableText, Scrollable;
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';

/// A microphone that records nothing, set as the test says.
class FakeRecorder implements VoiceRecorder {
  FakeRecorder({this.granted = MicAccess.granted, this.allow = true});

  MicAccess granted;
  bool allow;

  /// What it says after a refusal: denied, as a Mac's or an iPhone's does
  /// at once; undetermined, as Android's does after a first no.
  MicAccess afterRefusal = MicAccess.denied;
  final requests = <bool>[];
  String? recordingInto;
  var cancelled = 0;

  /// How long what it records lasts.
  Duration duration = const Duration(seconds: 42);
  RecordingFailure? failure;
  final _events = StreamController<RecorderEvent>.broadcast();

  void hear(double level, Duration at) =>
      _events.add(RecorderEvent(level: level, elapsed: at));

  void end(RecordingEnd how) => _events.add(RecorderEvent(ended: how));

  @override
  Stream<RecorderEvent> get events => _events.stream;

  @override
  Future<MicAccess> access() async => granted;

  @override
  Future<bool> request() async {
    requests.add(allow);
    granted = allow ? MicAccess.granted : afterRefusal;
    return allow;
  }

  @override
  Future<void> openSettings() async {}

  @override
  Future<void> start(String path) async {
    if (failure case final failure?) throw failure;
    recordingInto = path;
    // Something recorded, as the microphone would write.
    File(path)
      ..createSync(recursive: true)
      ..writeAsBytesSync(List.filled(64, 7));
  }

  @override
  Future<Recording?> stop() async {
    final path = recordingInto;
    recordingInto = null;
    return path == null ? null : Recording(path: path, duration: duration);
  }

  @override
  Future<void> cancel() async {
    recordingInto = null;
    cancelled++;
  }
}

/// Plays nothing, saying what it was asked.
class FakeAudio implements AudioOutput {
  final played = <String>[];
  final _events = StreamController<AudioPosition>.broadcast();

  void say(AudioPosition position) => _events.add(position);

  @override
  Stream<AudioPosition> get events => _events.stream;

  @override
  Future<void> play(String path, {Duration from = Duration.zero}) async =>
      played.add(path);

  @override
  Future<void> pause() async {}

  @override
  Future<void> seek(Duration position) async {}

  @override
  Future<void> stop() async {}
}

/// Lets real files be written and read.
Future<void> io(WidgetTester tester) async {
  for (var i = 0; i < 12; i++) {
    await tester.pump();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 20)),
    );
  }
  await tester.pump();
}

/// A Claktory whose speech model is in [state].
Workspace withVoice(
  SpeechModelState state, {
  int bytes = 0,
  String error = '',
}) => workspace()
  ..speechModel = SpeechModel(
    state: state,
    name: 'Parakeet v3 (25 languages)',
    bytes: Int64(bytes),
    total: Int64(740000000),
    error: error,
    attribution: 'Parakeet TDT 0.6B v3 by NVIDIA, CC BY 4.0',
  );

final mic = find.bySemanticsLabel('Record a voice message');

void main() {
  test("download progress reads in the total's unit", () {
    expect(bytesOf(Int64(312000000), Int64(740000000)), '312 of 740 MB');
    expect(bytesOf(Int64(0), Int64(740000000)), '0 of 740 MB');
    expect(
      transcribingText(
        SpeechModel(
          state: SpeechModelState.SPEECH_MODEL_STATE_DOWNLOADING,
          bytes: Int64(312000000),
          total: Int64(740000000),
        ),
      ),
      'Downloading the speech model… 312 of 740 MB',
    );
    expect(
      transcribingText(
        SpeechModel(state: SpeechModelState.SPEECH_MODEL_STATE_READY),
      ),
      'Transcribing…',
    );
  });

  group('in the app', () {
    late FakeRecorder recorder;
    late VoiceRecorder was;
    setUp(() {
      voiceMessagesOn = true;
      was = deviceRecorder;
      recorder = FakeRecorder();
      deviceRecorder = recorder;
    });
    tearDown(() {
      deviceRecorder = was;
      voiceMessagesOn = true;
    });

    testWidgets("a Claktory that doesn't know voice messages has no mic", (
      tester,
    ) async {
      await Harness(workspace()).show(tester);
      expect(mic, findsNothing);
      // Nor Settings → Voice.
      expect(find.text('Voice'), findsNothing);
    });

    testWidgets('with voice messages off, no mic', (tester) async {
      voiceMessagesOn = false;
      await Harness(withVoice(SpeechModelState.SPEECH_MODEL_STATE_READY))
          .show(tester);
      expect(mic, findsNothing);
    });

    testWidgets('the first time, it asks before the download; then it '
        'records, and Send sends a voice message', (tester) async {
      final harness = Harness(
        withVoice(SpeechModelState.SPEECH_MODEL_STATE_ABSENT),
      );
      await harness.show(tester);
      await tester.tap(mic);
      await settle(tester);
      expect(
        find.textContaining('Voice messages need a speech model (740 MB) on'),
        findsOneWidget,
      );
      await tester.tap(find.text('Download'));
      await io(tester);
      expect(
        harness.backend.sent.where((m) => m.hasInstallSpeechModel()),
        hasLength(1),
      );
      // Recording meanwhile, in the box's place.
      expect(recorder.recordingInto, isNotNull);
      expect(find.byType(RecordingStrip), findsOneWidget);
      expect(find.byType(EditableText), findsNothing);
      recorder.hear(0.6, const Duration(milliseconds: 1500));
      await tester.pump();
      await tester.pump();
      expect(find.text('0:01'), findsOneWidget);

      // The download is under way now: the next time, nothing is asked.
      await tester.tap(find.text('Send'));
      await io(tester);
      final sent = harness.backend.sent
          .where((m) => m.hasSendMessage())
          .single
          .sendMessage;
      expect(sent.voice, isTrue);
      expect(sent.text, '');
      expect(sent.attachments.single.name, endsWith('.m4a'));
      expect(
        harness.backend.sent
            .where((m) => m.hasStartUpload())
            .single
            .startUpload
            .mime,
        'audio/mp4',
      );
      expect(find.byType(RecordingStrip), findsNothing);
      expect(find.byType(EditableText), findsOneWidget);
    });

    testWidgets('Esc cancels a recording', (tester) async {
      final harness = Harness(
        withVoice(SpeechModelState.SPEECH_MODEL_STATE_READY),
      );
      await harness.show(tester);
      await tester.tap(mic);
      await io(tester);
      expect(find.byType(RecordingStrip), findsOneWidget);
      // No question: the model is there.
      expect(
        harness.backend.sent.where((m) => m.hasInstallSpeechModel()),
        isEmpty,
      );
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await io(tester);
      expect(recorder.cancelled, 1);
      expect(find.byType(RecordingStrip), findsNothing);
      expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
    });

    testWidgets('a download that failed is picked up again without asking', (
      tester,
    ) async {
      final harness = Harness(
        withVoice(SpeechModelState.SPEECH_MODEL_STATE_FAILED, error: 'Offline'),
      );
      await harness.show(tester);
      await tester.tap(mic);
      await io(tester);
      expect(find.textContaining('Download the speech model?'), findsNothing);
      expect(
        harness.backend.sent.where((m) => m.hasInstallSpeechModel()),
        hasLength(1),
      );
      expect(find.byType(RecordingStrip), findsOneWidget);
    });

    testWidgets("where it can't run, the mic is grey and says why", (
      tester,
    ) async {
      final harness = Harness(
        withVoice(
          SpeechModelState.SPEECH_MODEL_STATE_UNSUPPORTED,
          error: "This Claktory's computer can't run speech recognition.",
        ),
      );
      await harness.show(tester);
      await tester.tap(mic);
      await io(tester);
      expect(recorder.recordingInto, isNull);
      expect(
        find.text("This Claktory's computer can't run speech recognition."),
        findsOneWidget,
      );
    });

    testWidgets('a voice message waiting says how its transcription goes; '
        'one that failed offers Retry and Delete', (tester) async {
      final harness = Harness(
        withVoice(
          SpeechModelState.SPEECH_MODEL_STATE_DOWNLOADING,
          bytes: 312000000,
        ),
      );
      QueuedMessage voice(TranscriptionState state) => QueuedMessage(
        id: Int64(7),
        source: 'person',
        attachments: [Attachment(path: '/u/Voice.m4a', name: 'Voice.m4a')],
        transcription: Transcription(
          state: state,
          durationMs: Int64(42000),
          error: "Didn't hear any speech.",
          errorCode: 'no_speech',
        ),
      );
      harness.backend.transcript.load(Transcript(agentId: 'a1'));
      await harness.show(tester);
      harness.backend.queued.apply(
        QueuedMessages(
          agentId: 'a1',
          messages: [voice(TranscriptionState.TRANSCRIPTION_STATE_PENDING)],
        ),
      );
      await settle(tester);
      expect(find.text('0:42'), findsOneWidget);
      expect(
        find.text('Downloading the speech model… 312 of 740 MB'),
        findsOneWidget,
      );
      expect(find.text('Voice.m4a'), findsNothing);

      await harness.apply(
        tester,
        withVoice(SpeechModelState.SPEECH_MODEL_STATE_READY),
      );
      expect(find.text('Transcribing…'), findsOneWidget);

      harness.backend.queued.apply(
        QueuedMessages(
          agentId: 'a1',
          messages: [voice(TranscriptionState.TRANSCRIPTION_STATE_FAILED)],
        ),
      );
      await settle(tester);
      expect(find.text("Didn't hear any speech."), findsOneWidget);
      expect(find.text('Transcribing…'), findsNothing);
      await tester.tap(find.text('Retry'));
      await tester.pump();
      expect(harness.last.retryVoiceMessage.messageId, Int64(7));
      expect(harness.last.retryVoiceMessage.agentId, 'a1');
      await tester.tap(find.text('Delete'));
      await tester.pump();
      expect(harness.last.discardVoiceMessage.messageId, Int64(7));

      // Heard, and waiting for the agent: its words, with ▶.
      harness.backend.queued.apply(
        QueuedMessages(
          agentId: 'a1',
          messages: [
            voice(TranscriptionState.TRANSCRIPTION_STATE_DONE)
              ..text = 'Ship it on Friday',
          ],
        ),
      );
      await settle(tester);
      expect(find.text('Ship it on Friday'), findsOneWidget);
      expect(find.byType(VoicePlay), findsOneWidget);
      expect(find.text('Queued'), findsOneWidget);
      expect(find.text('Retry'), findsNothing);
    });

    testWidgets('what was heard shows as text, with ▶ to play the recording', (
      tester,
    ) async {
      final audio = FakeAudio();
      final temp = Directory.systemTemp.createTempSync('voice');
      addTearDown(() => temp.deleteSync(recursive: true));
      final player = voicePlayer;
      voicePlayer = VoicePlayer(output: audio, temp: temp);
      addTearDown(() => voicePlayer = player);
      final harness = Harness(
        withVoice(SpeechModelState.SPEECH_MODEL_STATE_READY),
      );
      const path = '/uploads/a1/Voice 2026-11-21 14.02.m4a';
      harness.backend.files[path] = FileContent(
        path: path,
        size: Int64(5),
        sha256: 'abcdef0123456789',
        data: [1, 2, 3, 4, 5],
      );
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [
            Entry(
              seq: Int64(1),
              user: UserText(
                text: 'Ship it on Friday',
                attachments: [
                  Attachment(path: path, name: 'Voice 2026-11-21 14.02.m4a'),
                ],
                voice: Transcription(
                  state: TranscriptionState.TRANSCRIPTION_STATE_DONE,
                  durationMs: Int64(42000),
                ),
              ),
            ),
          ],
        ),
      );
      await harness.show(tester);
      await settle(tester);
      expect(find.text('Ship it on Friday'), findsOneWidget);
      expect(find.text('0:42'), findsOneWidget);
      // The recording isn't a file beside it.
      expect(find.text('Voice 2026-11-21 14.02.m4a'), findsNothing);

      await tester.tap(find.byType(VoicePlay));
      await io(tester);
      expect(audio.played, hasLength(1));
      expect(File(audio.played.single).readAsBytesSync(), [1, 2, 3, 4, 5]);
      audio.say(
        AudioPosition(
          path: audio.played.single,
          position: const Duration(seconds: 3),
          duration: const Duration(seconds: 42),
          state: AudioState.playing,
        ),
      );
      await tester.pump();
      await tester.pump();
      expect(find.text('0:03'), findsOneWidget);
      expect(
        tester
            .widget<ExtensionGlyph>(
              find.descendant(
                of: find.byType(VoicePlay),
                matching: find.byType(ExtensionGlyph),
              ),
            )
            .icon,
        'pause',
      );
    });

    testWidgets('Settings → Voice: the model, Remove, and its credit', (
      tester,
    ) async {
      final harness = Harness(
        withVoice(SpeechModelState.SPEECH_MODEL_STATE_READY, bytes: 740000000),
      );
      harness.page.value = AppPage.settings;
      await harness.show(tester);
      await settle(tester);
      await tester.scrollUntilVisible(
        find.text('Speech model'),
        300,
        scrollable: find
            .descendant(
              of: find.byType(SettingsScroller),
              matching: find.byType(Scrollable),
            )
            .first,
      );
      expect(
        find.textContaining('Parakeet v3 (25 languages), 740 MB, on'),
        findsOneWidget,
      );
      expect(
        find.text('Parakeet TDT 0.6B v3 by NVIDIA, CC BY 4.0'),
        findsOneWidget,
      );
      await tester.ensureVisible(find.text('Remove'));
      await tester.pump();
      await tester.tap(find.text('Remove'));
      await tester.pump();
      expect(harness.last.hasRemoveSpeechModel(), isTrue);

      await harness.apply(
        tester,
        withVoice(SpeechModelState.SPEECH_MODEL_STATE_ABSENT),
      );
      await tester.ensureVisible(find.text('Download'));
      await tester.pump();
      await tester.tap(find.text('Download'));
      await tester.pump();
      expect(harness.last.hasInstallSpeechModel(), isTrue);
    });
  });

  test('a recording is named for when it was made', () {
    final path = voiceFilePath(DateTime(2026, 11, 21, 14, 2), folder: '/tmp');
    expect(path, startsWith('/tmp/claktory-voice/'));
    expect(path, endsWith('/Voice 2026-11-21 14.02.m4a'));
    expect(voiceDuration(const Duration(seconds: 42)), '0:42');
    expect(voiceDuration(const Duration(minutes: 10)), '10:00');
  });

  test('it asks for the microphone once, records, keeps the waveform, '
      'and hands the file over', () async {
    final mic = FakeRecorder(granted: MicAccess.undetermined);
    final voice = VoiceRecording(
      recorder: mic,
      now: () => DateTime(2026, 11, 21, 14, 2),
    );
    expect(await voice.start(), isTrue);
    expect(mic.requests, [true]);
    expect(voice.recording, isTrue);
    expect(mic.recordingInto, endsWith('Voice 2026-11-21 14.02.m4a'));

    for (var i = 0; i < VoiceRecording.samples + 10; i++) {
      mic.hear(i.isEven ? 0.8 : 0.1, Duration(milliseconds: 50 * i));
    }
    await pumpEventQueue();
    expect(voice.levels, hasLength(VoiceRecording.samples));
    expect(voice.elapsed, const Duration(milliseconds: 50 * 129));

    final recording = await voice.finish();
    expect(recording!.duration, const Duration(seconds: 42));
    expect(voice.recording, isFalse);
  });

  test('with the microphone refused, it says where to allow it', () async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final voice = VoiceRecording(
      recorder: FakeRecorder(granted: MicAccess.undetermined, allow: false),
    );
    expect(await voice.start(), isFalse);
    expect(voice.error, contains('Privacy & Security → Microphone'));
    expect(voice.recording, isFalse);
  });

  test('refused for good, each platform says where to allow it', () {
    final said = {
      for (final platform in TargetPlatform.values)
        platform: () {
          debugDefaultTargetPlatformOverride = platform;
          try {
            return micRefusedForGood();
          } finally {
            debugDefaultTargetPlatformOverride = null;
          }
        }(),
    };
    expect(
      said[TargetPlatform.macOS],
      "Claktory can't use the microphone. Allow it in System Settings → "
      'Privacy & Security → Microphone.',
    );
    expect(
      said[TargetPlatform.iOS],
      'To record, allow microphone access in Settings → Claktory.',
    );
    expect(
      said[TargetPlatform.android],
      'To record, allow Claktory to use the microphone in Settings.',
    );
    expect(
      said[TargetPlatform.linux],
      "Claktory can't use the microphone. Allow it in this device's settings.",
    );
  });

  test('a first no on Android asks again on the next tap; a second is for '
      'good', () async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final mic = FakeRecorder(granted: MicAccess.undetermined, allow: false)
      ..afterRefusal = MicAccess.undetermined;
    final voice = VoiceRecording(recorder: mic);
    expect(await voice.start(), isFalse);
    expect(
      voice.error,
      'Claktory needs the microphone to record. Tap the microphone again to '
      'allow it.',
    );
    mic.afterRefusal = MicAccess.denied;
    expect(await voice.start(), isFalse);
    expect(mic.requests, [false, false]);
    expect(
      voice.error,
      'To record, allow Claktory to use the microphone in Settings.',
    );
  });

  test("restrictions say so, with nothing to allow", () async {
    final voice = VoiceRecording(
      recorder: FakeRecorder(granted: MicAccess.restricted),
    );
    expect(await voice.start(), isFalse);
    expect(
      voice.error,
      "This device's restrictions don't allow Claktory to use the microphone.",
    );
  });

  test("a microphone that won't start says why", () async {
    final mic = FakeRecorder()
      ..failure = const RecordingFailure('Busy elsewhere.', code: 'busy');
    final voice = VoiceRecording(recorder: mic);
    expect(await voice.start(), isFalse);
    expect(voice.error, 'Busy elsewhere.');
  });

  test('cancel throws it away; the time limit is heard', () async {
    final mic = FakeRecorder();
    final voice = VoiceRecording(recorder: mic);
    final ends = <RecordingEnd>[];
    voice.onEnded = ends.add;
    await voice.start();
    mic.end(RecordingEnd.limit);
    await pumpEventQueue();
    expect(ends, [RecordingEnd.limit]);
    await voice.cancel();
    expect(mic.cancelled, 1);
    expect(voice.recording, isFalse);
  });
}
