// A field inside a phone's conversation (a card's Add a comment) while the
// person types in it: "Latest" and the message box step aside, and the
// field shows whole above the keyboard (Zucchero's #321, sa-23; the box,
// Lucy's needs-you-cards.md §5.2).
import 'package:factory_app/controls.dart' show TextField;
import 'package:factory_app/identity.dart' show Space;
import 'package:factory_app/message_box.dart' show MessageBox;
import 'package:factory_app/mobile/header.dart' show PhoneHeader;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/transcript.dart' show TranscriptView;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _se = Size(375, 667);
const _keyboard = 260.0;

AskPerson _procedure() => AskPerson(
  id: 'ask9',
  title: 'Set up the hardware key',
  body: 'Six short steps; tell me when each is done.',
  state: AskPersonState.ASK_PERSON_STATE_PENDING,
  steps: [
    for (final (i, text) in [
      'Plug in the key',
      'Open Terminal and run:\n\n```sh\nykman info\n```',
      'Touch the key when it blinks',
    ].indexed)
      AskStep(
        text: text,
        state: i < 1
            ? AskStepState.ASK_STEP_STATE_DONE
            : AskStepState.ASK_STEP_STATE_PENDING,
        decidedMs: i < 1 ? Int64(2000) : Int64(0),
      ),
  ],
);

Workspace _workspace() => Workspace(
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
);

Future<Harness> _show(WidgetTester tester, {double scale = 1}) async {
  tester.platformDispatcher.textScaleFactorTestValue = scale;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  final harness = Harness(_workspace());
  harness.backend.transcript.load(
    Transcript(
      agentId: 'june',
      entries: [
        Entry(seq: Int64(7), timestampMs: Int64(1000), askPerson: _procedure()),
        // Enough after it that the card isn't the conversation's end.
        for (var i = 8; i < 14; i++)
          Entry(
            seq: Int64(i),
            timestampMs: Int64(1000 + i),
            assistant: AssistantText(
              text:
                  'Reply $i, long enough to take a few lines on a small '
                  'phone so the conversation runs well past the card.',
            ),
          ),
      ],
    ),
  );
  await harness.show(tester, size: _se);
  await pumpFrames(tester);
  return harness;
}

Finder get _latest => find.bySemanticsLabel(RegExp('latest'));

Finder get _field => find.descendant(
  of: find.descendant(
    of: find.byType(TranscriptView),
    matching: find.byType(TextField),
  ),
  matching: find.byType(EditableText),
);

/// Scrolls up the conversation until the card's Add a comment shows.
Future<void> _toCard(WidgetTester tester) async {
  for (var i = 0; i < 60; i++) {
    final add = find.text('Add a comment').hitTestable();
    if (add.evaluate().isNotEmpty &&
        tester.getRect(add).bottom <
            tester.getRect(find.byType(MessageBox)).top) {
      break;
    }
    await tester.drag(find.byType(TranscriptView), const Offset(0, 300));
    await pumpFrames(tester, const Duration(milliseconds: 300));
  }
  await pumpFrames(tester);
}

/// Opens the card's Add a comment and brings the keyboard up, as a finger
/// would.
Future<void> _comment(WidgetTester tester) async {
  // Low on the screen, just above the message box, as it was in sa-22.
  final gap =
      tester.getRect(find.byType(MessageBox)).top -
      tester.getRect(find.text('Add a comment')).bottom;
  await tester.drag(
    find.byType(TranscriptView),
    Offset(0, gap - Space.s),
    touchSlopY: 0,
  );
  await pumpFrames(tester);
  await tester.tap(find.text('Add a comment'));
  await pumpFrames(tester);
  tester.view.viewInsets = const FakeViewPadding(bottom: _keyboard);
  addTearDown(tester.view.resetViewInsets);
  await pumpFrames(tester, const Duration(milliseconds: 1200));
}

void main() {
  for (final scale in [1.0, 2.0]) {
    testWidgets('typing a comment at ${scale}x: Latest and the message '
        'box step aside, and the field shows whole above the keyboard', (
      tester,
    ) async {
      await _show(tester, scale: scale);
      await _toCard(tester);
      // Away from the end, Latest offers the way back.
      expect(_latest.hitTestable(), findsOneWidget);

      await _comment(tester);
      expect(_field, findsOneWidget);
      final focused = FocusManager.instance.primaryFocus?.context?.widget;
      expect(focused, isNotNull);
      // Out of the way while the field has focus.
      expect(_latest.hitTestable(), findsNothing);
      // The message box too, kept as it was (TypingAbove).
      expect(find.byType(MessageBox), findsNothing);
      expect(find.byType(MessageBox, skipOffstage: false), findsOneWidget);
      // Whole, between the header and the keyboard.
      final field = tester.getRect(_field);
      final keyboardTop = _se.height - _keyboard;
      // With room under it for the line beneath, not flush with the keyboard.
      expect(field.bottom + Space.l, lessThanOrEqualTo(keyboardTop));
      final header = tester.getRect(find.byType(PhoneHeader)).bottom;
      expect(field.top, greaterThanOrEqualTo(header));

      // A comment that runs to more lines grows the field; it stays whole.
      await tester.enterText(_field, 'Serial 1234567 shows.');
      await pumpFrames(tester);
      final grown = tester.getRect(_field);
      expect(grown.height, greaterThan(field.height));
      expect(grown.top, greaterThanOrEqualTo(header));
      expect(grown.bottom, lessThanOrEqualTo(keyboardTop));

      // Done typing: back again.
      FocusManager.instance.primaryFocus?.unfocus();
      tester.view.resetViewInsets();
      await pumpFrames(tester);
      expect(_latest.hitTestable(), findsOneWidget);
      expect(find.byType(MessageBox), findsOneWidget);
    });
  }
}
