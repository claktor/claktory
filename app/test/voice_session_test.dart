// A voice message or spoken answer keeps recording while the person moves
// around the app (#409): away from its box or field, a floating bar says
// what it's for, how long, with Cancel and Send (Done); Send sends it where
// it was started, through the outbox, and says so; its name goes back
// there; Cancel asks first only for a long one.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/ask_card.dart' show AskPersonCard;
import 'package:factory_app/dictation.dart';
import 'package:factory_app/mobile/composer.dart' show SendButton;
import 'package:factory_app/mobile/voice_input.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/voice.dart';
import 'package:factory_app/voice_keep.dart';
import 'package:factory_app/voice_session.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
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
final _barSend = find.byKey(const ValueKey('voice-bar-send'));
final _barCancel = find.byKey(const ValueKey('voice-bar-cancel'));
Finder _inBar(Finder finder) =>
    find.descendant(of: find.byType(VoiceBar), matching: finder);

Iterable<SendMessage> _sent(Harness harness) => harness.backend.sent
    .where((m) => m.hasSendMessage())
    .map((m) => m.sendMessage);

/// A recording to Mike from no box at all.
class _Session extends VoiceSession {
  _Session() : super(key: 'mike', recording: VoiceRecording());

  @override
  String get title => 'Mike';

  @override
  bool get message => true;

  @override
  Future<bool> send() async => true;

  @override
  Future<void> cancel() => recording.cancel();
}

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

  /// Records a voice message to Builder on a Mac, [seconds] long so far.
  Future<Harness> recording(WidgetTester tester, {int seconds = 3}) async {
    final harness = Harness(_two());
    await harness.show(tester);
    await settle(tester);
    await tester.tap(_mic);
    await io(tester);
    expect(recorder.recordingInto, isNotNull);
    recorder.hear(0.5, Duration(seconds: seconds));
    await tester.pump();
    return harness;
  }

  /// Opens June's conversation instead, as the sidebar would.
  Future<void> toJune(WidgetTester tester, Harness harness) async {
    await harness.apply(tester, _two(selected: 'a2'));
    await io(tester);
  }

  testWidgets('in its box, the recording shows in place, with no bar', (
    tester,
  ) async {
    await recording(tester);
    expect(find.byType(RecordingStrip), findsOneWidget);
    expect(_barSend, findsNothing);
  });

  testWidgets('away from its box it goes on in the bar: Send sends it to '
      'whom it was for, through the outbox, and says so', (tester) async {
    final harness = await recording(tester);
    await toJune(tester, harness);
    expect(find.byType(RecordingStrip), findsNothing, reason: "June's box");
    expect(recorder.recordingInto, isNotNull, reason: 'still recording');
    expect(_inBar(find.text('Builder')), findsOneWidget);
    expect(_inBar(find.text('0:03')), findsOneWidget);
    expect(_inBar(find.text('Send')), findsOneWidget);

    await tester.tap(_barSend);
    await io(tester);
    final sent = _sent(harness).single;
    expect(sent.agentId, 'a1');
    expect(sent.voice, isTrue);
    expect(sent.attachments.single.name, endsWith('.m4a'));
    expect(_barSend, findsNothing);
    expect(_inBar(find.text('Sent to Builder')), findsOneWidget);
    await tester.pump(voiceNoteFor + const Duration(milliseconds: 100));
    expect(_inBar(find.text('Sent to Builder')), findsNothing);
  });

  testWidgets("the bar's name goes back to the conversation, where the box "
      'shows the recording again and sends it', (tester) async {
    final harness = await recording(tester);
    await toJune(tester, harness);
    await tester.tap(_inBar(find.text('Builder')));
    await io(tester);
    expect(
      harness.backend.sent.where((m) => m.hasSelectAgent()).last.selectAgent,
      SelectAgent(agentId: 'a1'),
    );
    expect(harness.page.value, AppPage.agent);
    await harness.apply(tester, _two());
    await io(tester);
    expect(find.byType(RecordingStrip), findsOneWidget);
    expect(_barSend, findsNothing);
    await tester.tap(find.text('Send'));
    await io(tester);
    expect(_sent(harness).single.agentId, 'a1');
    // Sent from its box: the box shows it, so the bar says nothing.
    expect(_inBar(find.text('Sent to Builder')), findsNothing);
  });

  testWidgets('over Settings too, which takes the conversation\'s place', (
    tester,
  ) async {
    final harness = await recording(tester);
    harness.page.value = AppPage.settings;
    await io(tester);
    expect(find.byType(RecordingStrip), findsNothing);
    expect(_barSend, findsOneWidget);
    await tester.tap(_barSend);
    await io(tester);
    expect(_sent(harness).single.agentId, 'a1');
  });

  testWidgets('Cancel throws a short one away at once', (tester) async {
    final harness = await recording(tester);
    await toJune(tester, harness);
    await tester.tap(_barCancel);
    await io(tester);
    expect(recorder.cancelled, 1);
    expect(_barSend, findsNothing);
    expect(_sent(harness), isEmpty);
  });

  testWidgets('Cancel asks first for a long one: Keep goes on recording, '
      'Delete throws it away', (tester) async {
    final harness = await recording(tester, seconds: 45);
    await toJune(tester, harness);
    await tester.tap(_barCancel);
    await io(tester);
    expect(_inBar(find.text('Delete this 0:45 recording?')), findsOneWidget);
    expect(recorder.cancelled, 0);
    await tester.tap(_inBar(find.text('Keep')));
    await io(tester);
    expect(_barSend, findsOneWidget);
    expect(recorder.recordingInto, isNotNull);

    await tester.tap(_barCancel);
    await io(tester);
    await tester.tap(_inBar(find.text('Delete')));
    await io(tester);
    expect(recorder.cancelled, 1);
    expect(_barSend, findsNothing);
    expect(_sent(harness), isEmpty);
  });

  testWidgets('one recording at a time: another box says which goes on', (
    tester,
  ) async {
    final harness = await recording(tester);
    await toJune(tester, harness);
    await tester.tap(_mic);
    await io(tester);
    expect(
      find.text(
        "You're recording a voice message to Builder. Send or cancel it "
        'first.',
      ),
      findsOneWidget,
    );
    // Nothing went wrong: no Ask Rivet to fix (Zucchero's row 346).
    expect(find.textContaining('to fix'), findsNothing);
    expect(_inBar(find.text('Builder')), findsOneWidget);
  });

  testWidgets('VoiceOver reads what it records and for whom', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = await recording(tester);
    await toJune(tester, harness);
    expect(
      find.bySemanticsLabel('Recording Voice message to Builder, 0:03'),
      findsOneWidget,
    );
    expect(find.bySemanticsLabel('Send'), findsWidgets);
    semantics.dispose();
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

    Future<Harness> conversation(WidgetTester tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = Harness(phoneWorkspace());
      harness.backend.transcript.load(Transcript(agentId: 'mike'));
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      await harness.show(tester, size: const Size(393, 852));
      await pumpFrames(tester);
      return harness;
    }

    testWidgets('a tapped recording goes on with the drawer open; Send in '
        'the bar sends it to Mike', (tester) async {
      final harness = await conversation(tester);
      await tester.tap(find.byType(PhoneMicButton));
      await io(tester);
      expect(find.byType(PhoneRecordingRow), findsOneWidget);
      expect(_barSend, findsNothing);
      recorder.hear(0.5, const Duration(seconds: 4));
      await tester.pump();

      await tester.tap(find.bySemanticsLabel('Menu').first);
      await pumpFrames(tester);
      expect(_inBar(find.text('Mike')), findsOneWidget);
      // A finger's size, its words under the status bar.
      expect(tester.getSize(_barSend).height, greaterThanOrEqualTo(44));
      expect(tester.getTopLeft(_barSend).dy, greaterThan(59));
      await tester.tap(_barSend);
      await io(tester);
      final sent = _sent(harness).single;
      expect(sent.agentId, 'mike');
      expect(sent.voice, isTrue);
      expect(_inBar(find.text('Sent to Mike')), findsOneWidget);
      expect(find.byType(PhoneRecordingRow), findsNothing);
    });

    testWidgets('at large text the bar wraps rather than cutting off', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      tester.platformDispatcher.textScaleFactorTestValue = 3;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = Harness(phoneWorkspace());
      await harness.show(
        tester,
        size: const Size(393, 852),
        home: const SizedBox.expand(),
      );
      final session = _Session();
      addTearDown(session.dispose);
      unawaited(session.recording.start());
      await io(tester);
      expect(_barSend, findsOneWidget);
      expect(_barCancel, findsOneWidget);
      // Under the name and the time.
      expect(
        tester.getTopLeft(_barSend).dy,
        greaterThan(tester.getBottomLeft(_inBar(find.text('Mike'))).dy),
      );
    });

    testWidgets("in another conversation, the bar takes a strip over the "
        "header rather than hiding its buttons", (tester) async {
      final harness = await conversation(tester);
      final menuTop = tester.getTopLeft(find.bySemanticsLabel('Menu').first).dy;
      await tester.tap(find.byType(PhoneMicButton));
      await io(tester);
      recorder.hear(0.5, const Duration(seconds: 3));
      await harness.apply(tester, phoneWorkspace()..selectedAgentId = 'rivet');
      await pumpFrames(tester);
      expect(_inBar(find.text('Mike')), findsOneWidget);
      final barBottom = tester.getBottomLeft(find.byType(VoiceBar)).dy;
      final menu = find.bySemanticsLabel('Menu').first;
      expect(tester.getTopLeft(menu).dy, greaterThanOrEqualTo(barBottom));
      expect(tester.getTopLeft(menu).dy, greaterThan(menuTop));
      // Sent, the strip goes after its note, and the header is back.
      await tester.tap(_barSend);
      await io(tester);
      expect(_inBar(find.text('Sent to Mike')), findsOneWidget);
      await tester.pump(voiceNoteFor + const Duration(milliseconds: 100));
      await pumpFrames(tester);
      expect(find.byType(VoiceBar), findsNothing);
      expect(tester.getTopLeft(menu).dy, lessThan(barBottom));
    });

    /// Mike's question card, said into, then the drawer opened over it.
    Future<Harness> cardBehindDrawer(
      WidgetTester tester, {
      String title = 'Ship it?',
      Size size = const Size(393, 852),
    }) async {
      final start = phoneWorkspace()..agents[1].waitingSeqs.add(Int64(7));
      final harness = Harness(start);
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      harness.backend.transcript.load(
        Transcript(
          agentId: 'mike',
          entries: [
            Entry(
              seq: Int64(7),
              timestampMs: Int64(1000),
              askPerson: AskPerson(
                id: 'ask7',
                title: title,
                wantsText: true,
                state: AskPersonState.ASK_PERSON_STATE_PENDING,
              ),
            ),
          ],
        ),
      );
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      await harness.show(tester, size: size);
      await pumpFrames(tester);
      final dictate = find.descendant(
        of: find.byType(AskPersonCard),
        matching: find.byKey(const ValueKey('dictate')),
      );
      await tester.ensureVisible(dictate);
      await pumpFrames(tester);
      await tester.tap(dictate);
      await io(tester);
      expect(recorder.recordingInto, isNotNull);
      expect(_barSend, findsNothing, reason: 'the card shows it');
      recorder.hear(0.5, const Duration(seconds: 3));
      await tester.pump();
      await tester.tap(find.bySemanticsLabel('Menu').first);
      await pumpFrames(tester);
      return harness;
    }

    testWidgets("an answer said into a card goes on behind the drawer, in "
        "the bar (Sale's row 80); Done there says where the words go "
        "(Zucchero's row 348)", (tester) async {
      await cardBehindDrawer(tester);
      expect(
        _inBar(find.text('Your answer to Mike: Ship it?')),
        findsOneWidget,
      );
      expect(_inBar(find.text('Done')), findsOneWidget);
      await tester.tap(_barSend);
      await io(tester);
      expect(
        _inBar(
          find.text('Done. Your words go in your answer to Mike: Ship it?'),
        ),
        findsOneWidget,
      );
    });

    testWidgets("its name goes back to the card's conversation "
        "(Zucchero's row 349)", (tester) async {
      final semantics = tester.ensureSemantics();
      final harness = await cardBehindDrawer(tester);
      expect(
        find.bySemanticsLabel(RegExp('^Recording Your answer to Mike')),
        findsOneWidget,
      );
      await tester.tap(_inBar(find.text('Your answer to Mike: Ship it?')));
      await io(tester);
      expect(
        harness.backend.sent.where((m) => m.hasSelectAgent()).last.selectAgent,
        SelectAgent(agentId: 'mike'),
      );
      // Back in the conversation, the card shows it again.
      expect(_barSend, findsNothing);
      semantics.dispose();
    });

    testWidgets("on an SE at large text the bar keeps whose answer it is: "
        "the name has the line, the time goes down by the buttons "
        "(Zucchero's row 347)", (tester) async {
      // The test font's letters are a square each, wider than the
      // system's: 2x here is about as wide as the largest size there.
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await cardBehindDrawer(tester, title: '', size: const Size(375, 667));
      final title = _inBar(find.text('Your answer to Mike'));
      expect(title, findsOneWidget);
      expect(
        tester.renderObject<RenderParagraph>(title).didExceedMaxLines,
        isFalse,
      );
      // The time is under it, beside Cancel and Done.
      final time = _inBar(find.text('0:03'));
      expect(
        tester.getTopLeft(time).dy,
        greaterThan(tester.getBottomLeft(title).dy),
      );
      expect(
        _inBar(
          find.byWidgetPredicate(
            (w) => w is CustomPaint && w.painter is Waveform,
          ),
        ),
        findsNothing,
        reason: 'no waveform',
      );
      expect(tester.takeException(), isNull);
    });

    testWidgets('held, it stays with the finger: no bar, let go sends', (
      tester,
    ) async {
      final harness = await conversation(tester);
      final gesture = await tester.startGesture(
        tester.getCenter(find.byType(PhoneMicButton)),
      );
      await tester.pump(const Duration(milliseconds: 250));
      await io(tester);
      expect(recorder.recordingInto, isNotNull);
      recorder.hear(0.5, const Duration(seconds: 2));
      await tester.pump();
      expect(_barSend, findsNothing);
      await gesture.up();
      await io(tester);
      expect(_sent(harness).single.voice, isTrue);
      expect(_barSend, findsNothing);
      expect(_inBar(find.text('Sent to Mike')), findsNothing);
    });

    testWidgets('a call pauses it away from the box: the bar says so and '
        'still sends', (tester) async {
      final semantics = tester.ensureSemantics();
      final harness = await conversation(tester);
      await tester.tap(find.byType(PhoneMicButton));
      await io(tester);
      recorder.hear(0.5, const Duration(seconds: 4));
      await tester.tap(find.bySemanticsLabel('Menu').first);
      await pumpFrames(tester);
      recorder.end(RecordingEnd.interrupted);
      await io(tester);
      expect(
        find.bySemanticsLabel('Recording paused, Voice message to Mike, 0:04'),
        findsOneWidget,
      );
      expect(_sent(harness), isEmpty);
      await tester.tap(_barSend);
      await io(tester);
      expect(_sent(harness).single.voice, isTrue);
      expect(find.byType(SendButton), findsNothing);
      semantics.dispose();
    });
  });

  group('an answer said into a card', () {
    late Directory kept;
    var asked = 0;

    setUp(() {
      kept = Directory.systemTemp.createTempSync('kept');
      final wasFolder = keptVoiceFolder;
      keptVoiceFolder = () async => kept.path;
      final wasTranscribe = transcribeRecording;
      asked = 0;
      transcribeRecording = (backend, recording) async {
        asked++;
        return 'after the release';
      };
      addTearDown(() {
        keptVoiceFolder = wasFolder;
        transcribeRecording = wasTranscribe;
        kept.deleteSync(recursive: true);
      });
    });

    Workspace withAsk() =>
        Workspace(
            projects: [Project(id: 'p1', name: 'Factory')],
            agents: [
              Agent(
                id: 'june',
                name: 'June',
                projectId: 'p1',
                modelAvailable: true,
                status: AgentStatus.AGENT_STATUS_IDLE,
                waitingSeqs: [Int64(7)],
              ),
            ],
            selectedAgentId: 'june',
          )
          ..speechModel = SpeechModel(
            state: SpeechModelState.SPEECH_MODEL_STATE_READY,
          );

    testWidgets('Done in the bar with the card gone: its words go in when '
        'the card is back', (tester) async {
      final harness = Harness(withAsk());
      harness.backend.transcript.load(
        Transcript(
          agentId: 'june',
          entries: [
            Entry(
              seq: Int64(7),
              timestampMs: Int64(1000),
              askPerson: AskPerson(
                id: 'ask7',
                title: 'Ship it?',
                wantsText: true,
                state: AskPersonState.ASK_PERSON_STATE_PENDING,
              ),
            ),
          ],
        ),
      );
      await harness.show(tester);
      await settle(tester);
      Finder inCard(Finder finder) =>
          find.descendant(of: find.byType(AskPersonCard), matching: finder);
      await tester.tap(inCard(find.byKey(const ValueKey('dictate'))));
      await io(tester);
      expect(recorder.recordingInto, isNotNull);
      expect(_barSend, findsNothing, reason: 'the card shows it');

      harness.page.value = AppPage.settings;
      await io(tester);
      expect(find.byType(AskPersonCard), findsNothing);
      expect(
        _inBar(find.text('Your answer to June: Ship it?')),
        findsOneWidget,
      );
      expect(_inBar(find.text('Done')), findsOneWidget);
      await tester.tap(_barSend);
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 100)),
      );
      await io(tester);
      expect(_inBar(find.text(keptForField)), findsOneWidget);
      expect(asked, 1, reason: 'its words asked for at once');
      expect(kept.listSync().whereType<Directory>(), hasLength(1));

      harness.page.value = AppPage.agent;
      await io(tester);
      await pumpFrames(tester);
      expect(inCard(find.text('after the release')), findsOneWidget);
      expect(asked, 1, reason: 'the words already heard');
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 100)),
      );
      expect(kept.listSync().whereType<Directory>(), isEmpty);
    });
  });
}
