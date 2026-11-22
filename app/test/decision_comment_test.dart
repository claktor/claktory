// "Add a comment" under a card's buttons opens its field with the focus,
// even while something else has it (Sorbet's iPad find: after a PDF's
// reload its pane held focus, and the field opened without it).
import 'package:factory_app/decision.dart';
import 'package:factory_app/dictation.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/voice.dart' show voiceMessagesOn;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'voice_test.dart' show FakeRecorder, io, withVoice;

void main() {
  testWidgets('the field opens focused though something else had focus', (
    tester,
  ) async {
    final comment = TextEditingController();
    addTearDown(comment.dispose);
    final other = FocusNode();
    addTearDown(other.dispose);
    await Harness(Workspace()).show(
      tester,
      home: Column(
        children: [
          Focus(focusNode: other, child: const SizedBox(height: 20)),
          DecisionComment(controller: comment, agent: 'June'),
        ],
      ),
    );
    other.requestFocus();
    await tester.pump();
    expect(other.hasFocus, isTrue);

    await tester.tap(find.text('Add a comment'));
    await pumpFrames(tester);
    final field = tester.widget<EditableText>(find.byType(EditableText));
    expect(field.focusNode.hasFocus, isTrue);
    expect(other.hasFocus, isFalse);
    await tester.enterText(find.byType(EditableText), 'Looks good');
    expect(comment.text, 'Looks good');
  });

  testWidgets('said rather than typed: the mic puts the words in, after '
      'what was typed', (tester) async {
    voiceMessagesOn = true;
    final was = deviceRecorder;
    final recorder = FakeRecorder();
    deviceRecorder = recorder;
    addTearDown(() => deviceRecorder = was);
    final wasTranscribe = transcribeRecording;
    transcribeRecording = (backend, recording) async => 'go ahead.';
    addTearDown(() => transcribeRecording = wasTranscribe);
    final comment = TextEditingController();
    addTearDown(comment.dispose);
    await Harness(withVoice(SpeechModelState.SPEECH_MODEL_STATE_READY)).show(
      tester,
      home: DecisionComment(controller: comment, agent: 'June'),
    );
    await tester.tap(find.text('Add a comment'));
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText), 'Yes,');
    await tester.tap(find.byKey(const ValueKey('dictate')));
    await io(tester);
    expect(find.text('Done'), findsOneWidget);
    await tester.tap(find.text('Done'));
    await io(tester);
    await pumpFrames(tester);
    expect(comment.text, 'Yes, go ahead.');
    expect(recorder.recordingInto, isNull);
  });

  testWidgets('recording in a narrow field (a 320-point panel): the time '
      'keeps its place, the waveform steps aside', (tester) async {
    voiceMessagesOn = true;
    final was = deviceRecorder;
    deviceRecorder = FakeRecorder();
    addTearDown(() => deviceRecorder = was);
    final comment = TextEditingController();
    addTearDown(comment.dispose);
    for (final (width, scale) in [
      for (final width in [200.0, 216.0, 240.0, 280.0, 320.0, 400.0])
        for (final scale in [1.0, 2.0]) (width, scale),
    ]) {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await Harness(withVoice(SpeechModelState.SPEECH_MODEL_STATE_READY)).show(
        tester,
        home: Align(
          alignment: Alignment.topLeft,
          child: SizedBox(
            width: width,
            child: DecisionComment(controller: comment, agent: 'June'),
          ),
        ),
      );
      if (find.text('Add a comment').evaluate().isNotEmpty) {
        await tester.tap(find.text('Add a comment'));
        await pumpFrames(tester);
      }
      await tester.tap(find.byKey(const ValueKey('dictate')));
      await io(tester);
      expect(find.text('Done'), findsOneWidget, reason: '$width at $scale');
      expect(find.text('0:00'), findsOneWidget, reason: '$width at $scale');
      expect(tester.takeException(), isNull, reason: '$width at $scale');
      await tester.tap(find.text('Cancel'));
      await io(tester);
    }
  });
}
