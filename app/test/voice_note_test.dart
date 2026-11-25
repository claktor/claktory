// Typed text and a voice note in one message (#425): a recording made while
// the box has something written waits in it as its voice note (a chip with
// its length, and × to take it out) instead of going alone; Send sends both
// as one message. The floating bar's Send, away from the box, takes the
// box's text along and says so. Chats too. In the conversation, what was
// typed shows over the recording and what was heard.
import 'dart:io';

import 'package:factory_app/controls.dart' show GlyphButton, Pressable;
import 'package:factory_app/mobile/composer.dart' show SendButton;
import 'package:factory_app/mobile/voice_input.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/voice.dart';
import 'package:factory_app/voice_keep.dart';
import 'package:factory_app/voice_session.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'chats_test.dart' show chat, chatting;
import 'harness.dart';
import 'voice_test.dart' show FakeRecorder, io;

/// Builder (a1, selected) and June (a2), with voice messages.
Workspace _two({String selected = 'a1'}) => workspace(
  agents: [
    builder(working: false),
    Agent(
      id: 'a2',
      projectId: 'p1',
      name: 'June',
      model: 'anthropic/claude-opus-5-5',
      modelAvailable: true,
    ),
  ],
  selected: selected,
)..speechModel = SpeechModel(state: SpeechModelState.SPEECH_MODEL_STATE_READY);

final _mic = find.bySemanticsLabel('Record a voice message');
final _chip = find.byType(VoiceNoteChip);
final _barSend = find.byKey(const ValueKey('voice-bar-send'));
Finder _inBar(Finder finder) =>
    find.descendant(of: find.byType(VoiceBar), matching: finder);

Iterable<SendMessage> _sent(Harness harness) => harness.backend.sent
    .where((m) => m.hasSendMessage())
    .map((m) => m.sendMessage);

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

  group('on a Mac', () {
    /// Builder's box with [text] written, recording, 3 s so far.
    Future<Harness> recordingWith(WidgetTester tester, String text) async {
      final harness = Harness(_two());
      await harness.show(tester);
      await settle(tester);
      await tester.enterText(find.byType(EditableText).last, text);
      await tester.pump();
      await tester.tap(_mic);
      await io(tester);
      expect(recorder.recordingInto, isNotNull);
      recorder.hear(0.5, const Duration(seconds: 3));
      await tester.pump();
      return harness;
    }

    testWidgets('recorded with text written, it waits in the box as a voice '
        'note; Return sends both as one message', (tester) async {
      final semantics = tester.ensureSemantics();
      final harness = await recordingWith(tester, 'Look at the login page');
      // With something written, the strip's button adds rather than sends.
      expect(find.text('Add'), findsOneWidget);
      expect(find.text('Send'), findsNothing);
      await tester.tap(find.text('Add'));
      await io(tester);
      expect(_sent(harness), isEmpty, reason: 'nothing goes yet');
      expect(_chip, findsOneWidget);
      expect(
        find.descendant(of: _chip, matching: find.text('Voice note')),
        findsOneWidget,
      );
      expect(
        find.descendant(of: _chip, matching: find.text('0:42')),
        findsOneWidget,
      );
      expect(find.bySemanticsLabel('Voice note, 0:42'), findsOneWidget);
      expect(find.text('Look at the login page'), findsOneWidget);
      // One voice note to a message: no mic while there's one.
      expect(_mic, findsNothing);

      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await io(tester);
      final message = _sent(harness).single;
      expect(message.agentId, 'a1');
      expect(message.text, 'Look at the login page');
      expect(message.voice, isTrue);
      expect(message.attachments.single.name, endsWith('.m4a'));
      expect(_chip, findsNothing);
      expect(_mic, findsOneWidget);
      semantics.dispose();
    });

    testWidgets("× takes the voice note out: it's deleted, the mic is back", (
      tester,
    ) async {
      final harness = await recordingWith(tester, 'Hi');
      await tester.tap(find.text('Add'));
      await io(tester);
      final note = VoiceNotes.of(harness.backend).note('a1')!;
      await tester.tap(
        find.descendant(of: _chip, matching: find.byType(Pressable)),
      );
      await io(tester);
      // 42 s long: it asks first, as Cancel does while recording.
      expect(find.text('Delete this voice note?'), findsOneWidget);
      await tester.tap(find.text('Delete').last);
      await io(tester);
      expect(_chip, findsNothing);
      expect(VoiceNotes.of(harness.backend).note('a1'), isNull);
      expect(File(note.path).existsSync(), isFalse);
      expect(_mic, findsOneWidget);
      // The text alone goes.
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await io(tester);
      expect(_sent(harness).single.voice, isFalse);
      expect(_sent(harness).single.text, 'Hi');
    });

    testWidgets('with nothing written, it sends as before', (tester) async {
      final harness = Harness(_two());
      await harness.show(tester);
      await settle(tester);
      await tester.tap(_mic);
      await io(tester);
      recorder.hear(0.5, const Duration(seconds: 3));
      await tester.pump();
      expect(find.text('Add'), findsNothing);
      await tester.tap(find.text('Send'));
      await io(tester);
      final message = _sent(harness).single;
      expect(message.voice, isTrue);
      expect(message.text, '');
      expect(_chip, findsNothing);
    });

    testWidgets("Send in the bar, away from the box, takes the box's text "
        'along and says so', (tester) async {
      final harness = await recordingWith(tester, 'About the logo');
      await harness.apply(tester, _two(selected: 'a2'));
      await io(tester);
      expect(_barSend, findsOneWidget);
      await tester.tap(_barSend);
      await io(tester);
      final message = _sent(harness).single;
      expect(message.agentId, 'a1');
      expect(message.text, 'About the logo');
      expect(message.voice, isTrue);
      expect(
        _inBar(find.text('Sent to Builder with your message')),
        findsOneWidget,
      );
      // Builder's box is empty again.
      await harness.apply(tester, _two());
      await io(tester);
      expect(find.text('About the logo'), findsNothing);
    });
  });

  testWidgets('a chat takes a voice note with the text too', (tester) async {
    final start = chatting([chat()])
      ..speechModel = SpeechModel(
        state: SpeechModelState.SPEECH_MODEL_STATE_READY,
      );
    final harness = Harness(start);
    harness.backend.chatTranscript.load(Transcript(agentId: 'c1'));
    await harness.show(tester);
    await settle(tester);
    await tester.enterText(find.byType(EditableText).last, 'Translate this');
    await tester.pump();
    await tester.tap(
      find.byWidgetPredicate(
        (w) => w is GlyphButton && w.label == 'Record a voice message',
      ),
    );
    await io(tester);
    recorder.hear(0.5, const Duration(seconds: 3));
    await tester.pump();
    await tester.tap(find.text('Add'));
    await io(tester);
    expect(_chip, findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await io(tester);
    final sent = harness.backend.sent
        .where((m) => m.hasSendChatMessage())
        .single
        .sendChatMessage;
    expect(sent.chatId, 'c1');
    expect(sent.text, 'Translate this');
    expect(sent.voice, isTrue);
    expect(sent.attachments.single.name, endsWith('.m4a'));
  });

  group('on an iPhone', () {
    Workspace phoneWorkspace() => Workspace(
      projects: [Project(id: 'p1', name: 'Claktory', path: '/w')],
      agents: [
        Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
        Agent(
          id: 'mike',
          projectId: 'p1',
          name: 'Mike',
          modelAvailable: true,
          model: 'anthropic/claude-opus-5-5',
        ),
      ],
      models: [Model(id: 'anthropic/claude-opus-5-5', name: 'Opus 5.5')],
      selectedAgentId: 'mike',
      speechModel: SpeechModel(
        state: SpeechModelState.SPEECH_MODEL_STATE_READY,
      ),
    );

    Future<Harness> conversation(
      WidgetTester tester, {
      Size size = const Size(393, 852),
    }) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = Harness(phoneWorkspace());
      harness.backend.transcript.load(Transcript(agentId: 'mike'));
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      await harness.show(tester, size: size);
      await pumpFrames(tester);
      return harness;
    }

    final mic = find.byType(PhoneMicButton);

    testWidgets('the mic reads Record a voice message in an empty box, and '
        'Add a voice note beside Send', (tester) async {
      final semantics = tester.ensureSemantics();
      await conversation(tester);
      expect(find.bySemanticsLabel('Record a voice message'), findsOneWidget);
      expect(find.bySemanticsLabel('Add a voice note'), findsNothing);
      await tester.enterText(find.byType(EditableText).last, 'Hi Mike');
      await pumpFrames(tester);
      expect(find.bySemanticsLabel('Add a voice note'), findsOneWidget);
      expect(find.bySemanticsLabel('Record a voice message'), findsNothing);
      semantics.dispose();
    });

    testWidgets('held beside Send with text written, let go adds it; Send '
        'sends both', (tester) async {
      final harness = await conversation(tester);
      await tester.enterText(find.byType(EditableText).last, 'Hi Mike');
      await pumpFrames(tester);
      expect(find.byType(SendButton), findsOneWidget);
      final at = tester.getCenter(mic);
      final gesture = await tester.startGesture(at);
      await tester.pump(const Duration(milliseconds: 250));
      await io(tester);
      expect(recorder.recordingInto, isNotNull);
      expect(find.byType(PhoneRecordingRow), findsOneWidget);
      // Still under the finger.
      expect(tester.getCenter(mic), at);
      recorder.hear(0.5, const Duration(seconds: 2));
      await tester.pump();
      await gesture.up();
      await io(tester);
      expect(_sent(harness), isEmpty);
      expect(_chip, findsOneWidget);
      expect(mic, findsNothing, reason: 'one voice note to a message');

      await tester.tap(find.byType(SendButton));
      await io(tester);
      final message = _sent(harness).single;
      expect(message.text, 'Hi Mike');
      expect(message.voice, isTrue);
      expect(_chip, findsNothing);
    });

    testWidgets('tapped with text written, Add voice note takes Send\'s '
        'place', (tester) async {
      final semantics = tester.ensureSemantics();
      final harness = await conversation(tester);
      await tester.enterText(find.byType(EditableText).last, 'Hi Mike');
      await pumpFrames(tester);
      await tester.tap(mic);
      await io(tester);
      recorder.hear(0.5, const Duration(seconds: 3));
      await tester.pump();
      expect(find.bySemanticsLabel('Add voice note'), findsOneWidget);
      expect(find.byType(SendButton), findsNothing);
      await tester.tap(find.byKey(const ValueKey('add-voice-note')));
      await io(tester);
      expect(_sent(harness), isEmpty);
      expect(_chip, findsOneWidget);
      // A finger's size, its × too.
      expect(tester.getSize(_chip).height, greaterThanOrEqualTo(44));
      expect(find.bySemanticsLabel('Remove voice note'), findsOneWidget);
      semantics.dispose();
    });

    testWidgets('an iPhone SE at the largest text: +, the field, the mic and '
        'Send all whole, the field wide enough to write in', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.12;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await conversation(tester, size: const Size(375, 667));
      await tester.enterText(find.byType(EditableText).last, 'Hi Mike');
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      final add = find.bySemanticsLabel('Attach files');
      final send = find.byType(SendButton);
      final rects = [
        for (final finder in [add, mic, send]) tester.getRect(finder),
      ];
      for (final rect in rects) {
        expect(rect.width, greaterThanOrEqualTo(44));
        expect(rect.left, greaterThanOrEqualTo(0));
        expect(rect.right, lessThanOrEqualTo(375));
      }
      // In order, none over another.
      expect(rects[0].right, lessThanOrEqualTo(rects[1].left));
      expect(rects[1].right, lessThanOrEqualTo(rects[2].left));
      final field = tester.getRect(find.byType(EditableText).last);
      expect(field.width, greaterThanOrEqualTo(120));
      expect(field.right, lessThanOrEqualTo(rects[1].left));
    });

    testWidgets('an iPhone SE at the largest text: the voice note\'s chip '
        'fits, its length and × whole, and × takes it out', (tester) async {
      final semantics = tester.ensureSemantics();
      tester.platformDispatcher.textScaleFactorTestValue = 3.12;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await conversation(tester, size: const Size(375, 667));
      await tester.enterText(
        find.byType(EditableText).last,
        'Look at the login page',
      );
      await pumpFrames(tester);
      await tester.tap(mic);
      await io(tester);
      recorder.hear(0.5, const Duration(seconds: 3));
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('add-voice-note')));
      await io(tester);
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      expect(_chip, findsOneWidget);
      final chip = tester.getRect(_chip);
      expect(chip.left, greaterThanOrEqualTo(0));
      expect(chip.right, lessThanOrEqualTo(375));
      final note = tester.widget<VoiceNoteChip>(_chip);
      final length = tester.getRect(
        find.descendant(
          of: _chip,
          matching: find.text(voiceDuration(note.duration)),
        ),
      );
      expect(length.right, lessThanOrEqualTo(chip.right));
      expect(length.bottom, lessThanOrEqualTo(chip.bottom));
      final remove = find.bySemanticsLabel('Remove voice note');
      final x = tester.getRect(remove);
      expect(x.right, lessThanOrEqualTo(chip.right));
      expect(x.width, greaterThanOrEqualTo(44));
      await tester.tap(remove);
      await pumpFrames(tester);
      // Over 30 s, it asks first.
      expect(find.text('Delete this voice note?'), findsOneWidget);
      // The sheet scrolls to its Delete at this size.
      await tester.ensureVisible(find.text('Delete'));
      await pumpFrames(tester);
      await tester.tap(find.text('Delete'));
      await io(tester);
      await pumpFrames(tester);
      expect(_chip, findsNothing);
      semantics.dispose();
    });
    testWidgets('an iPhone SE at the largest text: recording, the chip, '
        'sent offline, queued and failed, nothing overflows', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.12;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = await conversation(tester, size: const Size(375, 667));
      await tester.enterText(
        find.byType(EditableText).last,
        'Look at the login page',
      );
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      final gesture = await tester.startGesture(tester.getCenter(mic));
      await tester.pump(const Duration(milliseconds: 250));
      await io(tester);
      recorder.hear(0.5, const Duration(seconds: 2));
      await tester.pump();
      await gesture.up();
      await io(tester);
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      expect(_chip, findsOneWidget);

      // Sent with the connection down: "Not sent yet", Retry and Delete.
      harness.backend.refuse = 'Not connected to the backend.';
      harness.backend.connected.value = false;
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      await tester.tap(find.byType(SendButton));
      await io(tester);
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      expect(find.text('Not sent yet'), findsOneWidget);
      harness.backend.refuse = null;
      harness.backend.connected.value = true;
      await io(tester);
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);

      QueuedMessages queued(Transcription transcription) => QueuedMessages(
        agentId: 'mike',
        messages: [
          QueuedMessage(
            id: Int64(7),
            source: 'person',
            text: 'Look at the login page',
            typed: 'Look at the login page',
            attachments: [Attachment(path: '/u/Voice.m4a', name: 'Voice.m4a')],
            transcription: transcription,
          ),
        ],
      );
      // Waiting to be heard: the transcribing line under it.
      harness.backend.queued.apply(
        queued(
          Transcription(
            state: TranscriptionState.TRANSCRIPTION_STATE_PENDING,
            durationMs: Int64(42000),
          ),
        ),
      );
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      harness.backend.queued.apply(
        queued(
          Transcription(
            state: TranscriptionState.TRANSCRIPTION_STATE_FAILED,
            error: 'The speech model could not read this recording.',
            durationMs: Int64(42000),
          ),
        ),
      );
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
    });
  });

  group('in the conversation', () {
    testWidgets('what was typed shows over the recording and what was '
        'heard', (tester) async {
      final harness = Harness(_two());
      const path = '/uploads/a1/Voice.m4a';
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [
            Entry(
              seq: Int64(1),
              user: UserText(
                text:
                    'Look at the login page\n\nVoice note: The button is '
                    'too small',
                typed: 'Look at the login page',
                heard: 'The button is too small',
                attachments: [
                  Attachment(path: path, name: 'Voice.m4a'),
                  Attachment(path: '/uploads/a1/shot.png', name: 'shot.png'),
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
      final typed = find.text('Look at the login page');
      final heard = find.text('The button is too small');
      expect(typed, findsOneWidget);
      expect(heard, findsOneWidget);
      expect(find.byType(VoicePlay), findsOneWidget);
      expect(
        tester.getTopLeft(typed).dy,
        lessThan(tester.getTopLeft(heard).dy),
      );
      expect(
        tester.getTopLeft(heard).dy,
        lessThan(tester.getTopLeft(find.byType(VoicePlay)).dy),
      );
      // Files sent with it are under it; the recording isn't one.
      expect(find.text('shot.png'), findsOneWidget);
      expect(find.text('Voice.m4a'), findsNothing);
    });

    testWidgets('waiting to be heard, the typed text shows over its length', (
      tester,
    ) async {
      final harness = Harness(_two());
      harness.backend.transcript.load(Transcript(agentId: 'a1'));
      await harness.show(tester);
      harness.backend.queued.apply(
        QueuedMessages(
          agentId: 'a1',
          messages: [
            QueuedMessage(
              id: Int64(7),
              source: 'person',
              text: 'Look at the login page',
              typed: 'Look at the login page',
              attachments: [
                Attachment(path: '/u/Voice.m4a', name: 'Voice.m4a'),
              ],
              transcription: Transcription(
                state: TranscriptionState.TRANSCRIPTION_STATE_PENDING,
                durationMs: Int64(42000),
              ),
            ),
          ],
        ),
      );
      await settle(tester);
      expect(find.text('Look at the login page'), findsOneWidget);
      expect(find.text('0:42'), findsOneWidget);

      // Heard: the typed text, then what was said, with ▶.
      harness.backend.queued.apply(
        QueuedMessages(
          agentId: 'a1',
          messages: [
            QueuedMessage(
              id: Int64(7),
              source: 'person',
              text:
                  'Look at the login page\n\nVoice note: The button is too '
                  'small',
              typed: 'Look at the login page',
              attachments: [
                Attachment(path: '/u/Voice.m4a', name: 'Voice.m4a'),
              ],
              transcription: Transcription(
                state: TranscriptionState.TRANSCRIPTION_STATE_DONE,
                durationMs: Int64(42000),
              ),
            ),
          ],
        ),
      );
      await settle(tester);
      expect(find.text('Look at the login page'), findsOneWidget);
      expect(find.text('The button is too small'), findsOneWidget);
      expect(find.byType(VoicePlay), findsOneWidget);
    });
  });
}
