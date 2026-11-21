// A voice message from the Mac app to a real Claktory (the dev one), with a
// recording made beforehand standing in for the microphone (so macOS asks
// no one for it): the mic, the strip, Send, the upload, the queued bubble,
// then what Claktory heard in the person's bubble with ▶. The dev Claktory
// needs its speech model ready. Screenshots go to SHOTS_DIR (/tmp/voice
// unless set).
//
//   say -o /tmp/voice.m4a --file-format=m4af --data-format=aac \
//     'This is a test of voice messages. Reply with just OK.'
//   cd app && FLUTTER_XCODE_PRODUCT_BUNDLE_IDENTIFIER=dev.factory.app.itest \
//     flutter test integration_test/voice_test.dart -d macos \
//     --dart-define=FACTORY_DEV=true --dart-define=VOICE_FILE=/tmp/voice.m4a \
//     [--dart-define=AGENT=Rivet]
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/voice.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _agent = String.fromEnvironment('AGENT', defaultValue: 'Rivet');
const _voice = String.fromEnvironment('VOICE_FILE');
const _shots = String.fromEnvironment('SHOTS_DIR', defaultValue: '/tmp/voice');

/// "Records" by copying [_voice] where the recording goes.
class FileRecorder implements VoiceRecorder {
  final _events = StreamController<RecorderEvent>.broadcast();
  String? _path;

  @override
  Stream<RecorderEvent> get events => _events.stream;

  @override
  Future<MicAccess> access() async => MicAccess.granted;

  @override
  Future<bool> request() async => true;

  @override
  Future<void> openSettings() async {}

  @override
  Future<void> start(String path) async {
    await File(path).parent.create(recursive: true);
    await File(_voice).copy(path);
    _path = path;
    for (var i = 1; i <= 20; i++) {
      _events.add(
        RecorderEvent(
          level: (i % 5) / 5,
          elapsed: Duration(milliseconds: 50 * i),
        ),
      );
    }
  }

  @override
  Future<Recording?> stop() async {
    final path = _path;
    _path = null;
    return path == null
        ? null
        : Recording(path: path, duration: const Duration(seconds: 3));
  }

  @override
  Future<void> cancel() async => _path = null;
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  // Frames come from the test, not the screen, so a covered window works.
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.benchmark;

  testWidgets('a voice message reaches the agent as words', (tester) async {
    expect(_voice, isNotEmpty, reason: 'VOICE_FILE: a recording to send');
    deviceRecorder = FileRecorder();
    var clock = Duration.zero;
    Future<void> step() async {
      const wait = Duration(milliseconds: 100);
      await Future<void>.delayed(wait);
      clock += wait;
      await tester.pumpBenchmark(clock);
    }

    Future<void> until(
      String what,
      bool Function() done, {
      Duration timeout = const Duration(seconds: 120),
    }) async {
      final end = DateTime.now().add(timeout);
      await step();
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await step();
      }
    }

    Future<void> shot(String name) async {
      for (var i = 0; i < 5; i++) {
        await step();
      }
      final view = binding.renderViews.first;
      // The view's layer is already scaled to physical pixels.
      final image = await (view.debugLayer! as OffsetLayer).toImage(
        Offset.zero & view.size * view.flutterView.devicePixelRatio,
      );
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      final file = File('$_shots/$name.png');
      await file.create(recursive: true);
      await file.writeAsBytes(bytes!.buffer.asUint8List());
      // ignore: avoid_print
      print('screenshot: ${file.path}');
    }

    await app.main();
    await until('the agents', () => find.text(_agent).evaluate().isNotEmpty);
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    Agent agent() => backend.workspace.workspace.agents.firstWhere(
      (agent) => agent.name == _agent,
    );
    expect(
      backend.workspace.workspace.speechModel.state,
      SpeechModelState.SPEECH_MODEL_STATE_READY,
      reason: 'the dev Claktory needs its speech model',
    );
    await backend.selectAgent(agent().id);
    scope.page.value = AppPage.agent;
    final mic = find.bySemanticsLabel('Record a voice message');
    await until('the mic', () => mic.evaluate().isNotEmpty);
    final before = backend.transcript.length;
    final waiting = {
      for (final message in backend.queued.of(agent().id)) message.id,
    };

    await tester.tap(mic);
    await until(
      'the recording',
      () => find.byType(RecordingStrip).evaluate().isNotEmpty,
    );
    await shot('1-recording');
    await tester.tap(find.text('Send'));
    await until(
      'the queued voice message',
      () => backend.queued
          .of(agent().id)
          .any((message) => message.hasTranscription()),
      timeout: const Duration(seconds: 30),
    );
    await shot('2-transcribing');

    // What was heard: delivered to the agent, or still queued for one
    // that can't run now (a usage limit), as words.
    (String, Transcription, List<Attachment>)? heard() {
      for (var i = before; i < backend.transcript.length; i++) {
        final entry = backend.transcript[i].entry;
        if (entry.hasUser() && entry.user.hasVoice()) {
          return (entry.user.text, entry.user.voice, entry.user.attachments);
        }
      }
      for (final message in backend.queued.of(agent().id)) {
        if (waiting.contains(message.id)) continue;
        if (message.transcription.state ==
            TranscriptionState.TRANSCRIPTION_STATE_DONE) {
          return (message.text, message.transcription, message.attachments);
        }
      }
      return null;
    }

    await until('the words', () => heard() != null);
    final (text, voice, files) = heard()!;
    // ignore: avoid_print
    print(
      'heard: "$text" (${voice.durationMs} ms, ${voice.language}); '
      'delivered: ${backend.queued.of(agent().id).isEmpty}',
    );
    expect(text.toLowerCase(), contains('voice messages'));
    expect(files.single.name, endsWith('.m4a'));
    await until('the ▶', () => find.byType(VoicePlay).evaluate().isNotEmpty);
    await shot('3-heard');
  });
}
