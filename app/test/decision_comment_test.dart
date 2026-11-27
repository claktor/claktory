// A card's field: open from the start,
// "Reply to June", never taking the focus by itself; Return sends only
// where words alone answer.
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/decision.dart';
import 'package:factory_app/dictation.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/voice.dart' show voiceMessagesOn;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'voice_test.dart' show FakeRecorder, io, withVoice;

void main() {
  testWidgets('the field is open from the start and never takes the focus '
      'by itself', (tester) async {
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
    expect(find.text('Add a comment'), findsNothing);
    expect(find.text('Reply to June'), findsOneWidget);
    final field = tester.widget<EditableText>(find.byType(EditableText));
    expect(field.focusNode.hasFocus, isFalse);
    expect(other.hasFocus, isTrue);
    await tester.enterText(find.byType(EditableText), 'Looks good');
    expect(comment.text, 'Looks good');
    // Without Submit, Return sends nothing and adds no line.
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
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
      await tester.tap(find.byKey(const ValueKey('dictate')));
      await io(tester);
      expect(find.text('Done'), findsOneWidget, reason: '$width at $scale');
      expect(find.text('0:00'), findsOneWidget, reason: '$width at $scale');
      expect(tester.takeException(), isNull, reason: '$width at $scale');
      await tester.tap(find.text('Cancel'));
      await io(tester);
    }
  });

  testWidgets('with Submit (#370): once typed, the buttons give way to one '
      'Submit, and Return is Submit', (tester) async {
    final comment = TextEditingController();
    addTearDown(comment.dispose);
    var submitted = 0;
    var approved = 0;
    void submit() => submitted++;
    await Harness(Workspace()).show(
      tester,
      home: Column(
        children: [
          CommentTyped(
            controller: comment,
            builder: (context, typed) => typed
                ? SubmitButton(onPressed: submit)
                : Button(label: 'Approve', onPressed: () => approved++),
          ),
          DecisionComment(controller: comment, agent: 'June', onSubmit: submit),
        ],
      ),
    );
    expect(find.text('Approve'), findsOneWidget);
    expect(find.text('Submit'), findsNothing);

    // Spaces alone aren't a comment.
    await tester.enterText(find.byType(EditableText), '   ');
    await tester.pump();
    expect(find.text('Approve'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    expect(submitted, 0);

    await tester.enterText(find.byType(EditableText), 'Staging first');
    await tester.pump();
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Submit'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    expect(submitted, 1);
    await tester.tap(find.text('Submit'));
    expect(submitted, 2);
    expect(approved, 0);

    // Cleared: the buttons come back.
    await tester.enterText(find.byType(EditableText), '');
    await tester.pump();
    expect(find.text('Approve'), findsOneWidget);
  });
}
