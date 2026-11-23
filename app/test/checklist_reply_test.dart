// Replying on a checklist item (Matt's ask): in the Checklist panel a
// field under the item sends the person's words to the agent, tied to the
// item (Needs you shows an item only as a card asking about it, answered
// there); in the conversation the message shows which
// item it's about, and that opens it.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/checklist_reply.dart' show ChecklistReply;
import 'package:factory_app/dictation.dart'
    show DictationFailure, dictationError, transcribeRecording;
import 'package:factory_app/recorder.dart' show deviceRecorder;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'extensions_test.dart' show at, checklistView, now, showWith, workspace;
import 'harness.dart';
import 'selection_test.dart' show watchClipboard;
import 'voice_test.dart' show FakeRecorder, io;

Future<void> _type(WidgetTester tester, String text) async {
  await tester.enterText(
    find
        .descendant(
          of: find.byType(Semantics).hitTestable(),
          matching: find.byType(EditableText),
        )
        .last,
    text,
  );
  await tester.pump();
}

void main() {
  testWidgets('on a phone, Show in checklist: the item comes into view, '
      'open to its reply, however late the checklist comes', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace());
    await harness.show(tester, size: const Size(393, 852));
    await settle(tester);
    // Pointed at first, as Show in checklist does; the sheet and its
    // items come after.
    harness.backend.highlights.pointedAt.value = (
      keys: ['checklist_item:a1#50'],
      at: DateTime.now(),
    );
    await harness.apply(
      tester,
      workspace(openPanel: 'checklist')..highlights = true,
    );
    await pumpFrames(tester);
    harness.backend.extensionView.apply(
      checklistView()
        ..checklist.items.addAll([
          ChecklistItem(
            id: 50,
            text: 'Pick the launch date',
            status: ChecklistStatus.CHECKLIST_STATUS_BLOCKED,
            created: at(now.subtract(const Duration(days: 2))),
            blockedOn: 'person',
            blockedKind: BlockedKind.BLOCKED_KIND_PERSON_DECISION,
          ),
          for (var i = 0; i < 40; i++)
            ChecklistItem(
              id: 100 + i,
              text: 'Step $i',
              created: at(DateTime(now.year, now.month, now.day, 1, i)),
            ),
        ]),
    );
    await pumpFrames(tester);
    await settle(tester);
    final item = find.text('Pick the launch date');
    final y = tester.getCenter(item).dy;
    expect(y, inInclusiveRange(0, 852));
    expect(find.text('Reply about this item'), findsOneWidget);
    expect(
      harness.backend.highlights.glows.map((glow) => glow.key),
      contains('checklist_item:a1#50'),
    );
    await tester.pump(const Duration(seconds: 2));
  });

  testWidgets('the Checklist panel: any item opens to its history and a '
      'reply, sent to whoever it is assigned to', (tester) async {
    final harness = await showWith(tester, workspace(openPanel: 'checklist'));
    harness.backend.extensionView.apply(checklistView());
    await settle(tester);
    expect(find.text('Reply about this item'), findsNothing);

    // An item with no history opens too.
    await tester.tap(find.text('Usage circles'));
    await settle(tester);
    expect(find.text('Reply about this item'), findsOneWidget);
    expect(
      find.text('Tester gets it as your message about this item.'),
      findsOneWidget,
    );
    await _type(tester, 'Make them smaller');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    final sent = harness.last.sendMessage;
    expect(sent.agentId, 'a2');
    expect(sent.checklistItem, 2);
    expect(sent.checklistAgentId, 'a1');

    // A click beside the field leaves it open; one on the item folds it.
    await tester.tap(find.text('Sent to Tester.'));
    await settle(tester);
    expect(find.text('Reply about this item'), findsOneWidget);
    await tester.tap(find.text('Usage circles'));
    await settle(tester);
    expect(find.text('Reply about this item'), findsNothing);
  });

  testWidgets('said rather than typed: the mic puts the words in after '
      "what's typed, and nothing goes until Send", (tester) async {
    final was = deviceRecorder;
    final recorder = FakeRecorder();
    deviceRecorder = recorder;
    addTearDown(() => deviceRecorder = was);
    final wasTranscribe = transcribeRecording;
    transcribeRecording = (backend, recording) async => 'please.';
    addTearDown(() => transcribeRecording = wasTranscribe);
    final harness = await showWith(
      tester,
      workspace(openPanel: 'checklist')
        ..speechModel = SpeechModel(
          state: SpeechModelState.SPEECH_MODEL_STATE_READY,
        ),
    );
    harness.backend.extensionView.apply(checklistView());
    await settle(tester);
    await tester.tap(find.text('Usage circles'));
    await settle(tester);
    await _type(tester, 'Smaller,');
    await tester.tap(find.bySemanticsLabel('Dictate').last);
    await io(tester);
    expect(recorder.recordingInto, isNotNull);
    await tester.tap(
      find.descendant(
        of: find.byType(ChecklistReply),
        matching: find.text('Done'),
      ),
    );
    await io(tester);
    await pumpFrames(tester);
    expect(find.text('Smaller, please.'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
    await tester.tap(find.text('Send'));
    await settle(tester);
    final sent = harness.last.sendMessage;
    expect(sent.text, 'Smaller, please.');
    expect(sent.checklistItem, 2);
  });

  for (final (seconds, says) in [(2, false), (6, true)]) {
    testWidgets('a ${seconds}s recording with no words in it '
        '${says ? 'says so' : 'goes without a word'} (Matt\'s)', (
      tester,
    ) async {
      final was = deviceRecorder;
      final recorder = FakeRecorder()..duration = Duration(seconds: seconds);
      deviceRecorder = recorder;
      addTearDown(() => deviceRecorder = was);
      final wasTranscribe = transcribeRecording;
      transcribeRecording = (backend, recording) async =>
          throw DictationFailure(
            dictationError('no_speech'),
            code: 'no_speech',
          );
      addTearDown(() => transcribeRecording = wasTranscribe);
      final harness = await showWith(
        tester,
        workspace(openPanel: 'checklist')
          ..speechModel = SpeechModel(
            state: SpeechModelState.SPEECH_MODEL_STATE_READY,
          ),
      );
      harness.backend.extensionView.apply(checklistView());
      await settle(tester);
      await tester.tap(find.text('Usage circles'));
      await settle(tester);
      await _type(tester, 'Smaller,');
      await tester.tap(find.bySemanticsLabel('Dictate').last);
      await io(tester);
      await tester.tap(
        find.descendant(
          of: find.byType(ChecklistReply),
          matching: find.text('Done'),
        ),
      );
      await io(tester);
      await pumpFrames(tester);
      expect(
        find.text("Didn't catch any words. Try again."),
        says ? findsOneWidget : findsNothing,
      );
      // The recording line is closed, and what was typed is as it was.
      expect(find.text('Smaller,'), findsOneWidget);
      expect(find.bySemanticsLabel('Dictate'), findsWidgets);
      expect(recorder.recordingInto, isNull);
    });
  }

  testWidgets('in the conversation, the message says which item it is '
      'about, and that opens it', (tester) async {
    final harness = Harness(workspace());
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            user: UserText(
              text: 'Make them smaller',
              checklistItem: ChecklistItemRef(
                agentId: 'a2',
                id: 2,
                text: 'Usage circles',
              ),
            ),
          ),
          Entry(
            seq: Int64(2),
            user: UserText(text: 'And thanks'),
          ),
        ],
      ),
    );
    final copied = watchClipboard(tester);
    await harness.show(tester);
    await settle(tester);
    expect(find.text('About: Usage circles'), findsOneWidget);
    // A copy of the conversation is its messages, not the reference.
    await pumpFrames(tester);
    tester
        .state<SelectableRegionState>(find.byType(SelectableRegion).first)
        .selectAll(SelectionChangedCause.keyboard);
    await tester.pump();
    Actions.invoke(
      tester.element(find.text('And thanks', findRichText: true).first),
      CopySelectionTextIntent.copy,
    );
    await tester.pump();
    expect(copied.single, contains('Make them smaller'));
    expect(copied.single, isNot(contains('About:')));
    await tester.tap(find.text('About: Usage circles'));
    await settle(tester);
    expect(
      harness.backend.sent.any((m) => m.selectAgent.agentId == 'a2'),
      isTrue,
    );
    final open = harness.last.openPanel;
    expect(open.agentId, 'a2');
    expect(open.extensionId, 'checklist');
  });

  testWidgets('a reply waiting in the queue says its item too', (tester) async {
    final snapshot = workspace();
    snapshot.agents.firstWhere((agent) => agent.id == 'a1').status =
        AgentStatus.AGENT_STATUS_WORKING;
    final harness = Harness(snapshot);
    harness.backend.transcript.load(Transcript(agentId: 'a1'));
    harness.backend.queued.apply(
      QueuedMessages(
        agentId: 'a1',
        messages: [
          QueuedMessage(
            id: Int64(1),
            source: 'person',
            text: 'Make them smaller',
            checklistItem: ChecklistItemRef(
              agentId: 'a1',
              id: 3,
              text: 'Someday',
            ),
          ),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    expect(find.text('About: Someday'), findsOneWidget);
  });
}
