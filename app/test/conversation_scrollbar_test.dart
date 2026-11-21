// The conversation's scrollbar (#237): steady while scrolling a long
// conversation of very different heights, where the app's own scrollbar,
// driven by Flutter's estimate of a lazily built list, jumped up and down.
import 'package:factory_app/conversation_scrollbar.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/transcript.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

/// A long conversation as agents make them: questions, runs of tool calls,
/// short replies and now and then a very long one.
List<Entry> _conversation(int count) => [
  for (var i = 0; i < count; i++)
    switch (i % 7) {
      0 => Entry(
        seq: Int64(i),
        user: UserText(text: 'Question $i'),
      ),
      1 || 2 || 3 => Entry(
        seq: Int64(i),
        toolCall: ToolCall(
          callId: 'c$i',
          name: 'bash',
          input: '{"command":"ls"}',
          output: 'x',
          state: ToolCallState.TOOL_CALL_STATE_DONE,
        ),
      ),
      4 => Entry(
        seq: Int64(i),
        assistant: AssistantText(
          text: List.filled(
            i % 3 == 0 ? 40 : 2,
            'A long line of reply text that wraps across the bubble.',
          ).join('\n\n'),
        ),
      ),
      _ => Entry(
        seq: Int64(i),
        assistant: AssistantText(text: 'Short $i'),
      ),
    },
];

Future<ScrollPosition> _show(WidgetTester tester, List<Entry> entries) async {
  final harness = Harness(workspace(agents: [builder(working: false)]));
  harness.backend.transcript.load(Transcript(agentId: 'a1', entries: entries));
  await harness.show(tester);
  await settle(tester);
  return tester
      .state<ScrollableState>(
        find
            .descendant(
              of: find.byType(TranscriptView),
              matching: find.byType(Scrollable),
            )
            .first,
      )
      .position;
}

Thumb _thumb(WidgetTester tester) => tester
    .state<ConversationScrollbarState>(find.byType(ConversationScrollbar))
    .thumb!;

void main() {
  testWidgets('scrolling up a long conversation, the thumb only rises, '
      'steadily, and keeps its size', (tester) async {
    final position = await _show(tester, _conversation(300));
    var last = _thumb(tester);
    for (var step = 0; step < 300; step++) {
      position.jumpTo(position.pixels - 40);
      await tester.pump();
      final thumb = _thumb(tester);
      expect(thumb.top, lessThanOrEqualTo(last.top), reason: 'step $step');
      expect(last.top - thumb.top, lessThan(8), reason: 'step $step');
      expect(thumb.extent, closeTo(last.extent, 1), reason: 'step $step');
      last = thumb;
    }
    // And back down.
    for (var step = 0; step < 100; step++) {
      position.jumpTo(position.pixels + 40);
      await tester.pump();
      final thumb = _thumb(tester);
      expect(thumb.top, greaterThanOrEqualTo(last.top), reason: 'step $step');
      last = thumb;
    }
  });

  testWidgets("at the end it's at the bottom, at the start at the top", (
    tester,
  ) async {
    final position = await _show(tester, _conversation(60));
    final view = tester.getSize(find.byType(ConversationScrollbar));
    var thumb = _thumb(tester);
    expect(thumb.top + thumb.extent, closeTo(view.height - 2, 1));
    position.jumpTo(position.minScrollExtent);
    await tester.pump();
    // The start is laid out now: the list knows where it is.
    position.jumpTo(position.minScrollExtent);
    await tester.pump();
    thumb = _thumb(tester);
    expect(thumb.top, closeTo(2, 1));
  });

  testWidgets('dragging the thumb scrolls the conversation', (tester) async {
    final position = await _show(tester, _conversation(120));
    final before = position.pixels;
    final thumb = _thumb(tester);
    final bar = tester.getTopRight(find.byType(ConversationScrollbar));
    final gesture = await tester.startGesture(
      Offset(bar.dx - 5, bar.dy + thumb.top + thumb.extent / 2),
    );
    await gesture.moveBy(const Offset(0, -20));
    await gesture.moveBy(const Offset(0, -100));
    await tester.pump();
    await gesture.up();
    await tester.pump();
    expect(position.pixels, lessThan(before - 1000));
    expect(_thumb(tester).top, lessThan(thumb.top - 90));
  });

  testWidgets("the app's own scrollbar is off for the conversation", (
    tester,
  ) async {
    await _show(tester, _conversation(60));
    expect(
      find.descendant(
        of: find.byType(ConversationScrollbar),
        matching: find.byType(RawScrollbar),
      ),
      findsNothing,
    );
  });
}
