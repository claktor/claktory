// A conversation's timeline with more than its entries (#250): extras (a
// pipe's messages) interleaved by time, entries hidden where an extra
// shows them, and going to an extra however far back.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart' show TranscriptStore;
import 'package:factory_app/transcript.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

Entry _said(int seq, int at, String text) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(at),
  user: UserText(text: text),
);

Entry _replied(int seq, int at, String text) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(at),
  assistant: AssistantText(text: text),
);

Entry _ran(int seq, int at, String command, {int reply = 0}) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(at),
  reply: Int64(reply),
  toolCall: ToolCall(
    callId: 'c$seq',
    name: 'bash',
    input: '{"command": "$command"}',
    output: 'ok',
    state: ToolCallState.TOOL_CALL_STATE_DONE,
  ),
);

TimelineItem _card(String text, int at) => TimelineItem(
  key: 'pipe:$text',
  timestampMs: at,
  build: (context) => Text('Card $text'),
);

/// A conversation view with [extras], as a pipe's view holds them.
Future<Harness> _show(
  WidgetTester tester,
  List<Entry> entries,
  ValueNotifier<List<TimelineItem>> extras, {
  bool hasEarlier = false,
  bool Function(Entry)? hidden,
  Future<void> Function()? loadEarlier,
}) async {
  final harness = Harness(workspace(agents: [builder(working: false)]));
  final transcript = harness.backend.transcript;
  transcript.load(
    Transcript(agentId: 'a1', entries: entries, hasEarlier: hasEarlier),
  );
  await harness.show(
    tester,
    home: TranscriptView(
      transcript: transcript,
      working: false,
      follow: ChangeNotifier(),
      agentId: 'a1',
      reveal: harness.backend.reveal,
      extras: extras,
      hidden: hidden,
      loadEarlier: loadEarlier ?? () async {},
    ),
  );
  await settle(tester);
  return harness;
}

double _top(WidgetTester tester, String text) =>
    tester.getTopLeft(find.textContaining(text, findRichText: true)).dy;

void main() {
  testWidgets('extras sit among the entries by time', (tester) async {
    final extras = ValueNotifier([
      _card('early', 500),
      _card('middle', 2000),
      _card('tie', 3000),
      _card('late', 4000),
    ]);
    await _show(tester, [
      _said(1, 1000, 'Question'),
      _replied(2, 3000, 'Answer'),
    ], extras);
    final order = [
      'Card early',
      'Question',
      'Card middle',
      'Answer',
      'Card tie',
      'Card late',
    ].map((text) => _top(tester, text)).toList();
    for (var i = 1; i < order.length; i++) {
      expect(order[i], greaterThan(order[i - 1]), reason: 'item $i');
    }
  });

  testWidgets('a new extra shows where its time puts it, and an old one '
      'keeps its place', (tester) async {
    final extras = ValueNotifier([_card('held', 1500)]);
    final harness = await _show(tester, [
      _said(1, 1000, 'Question'),
      _replied(2, 2000, 'Answer'),
    ], extras);
    harness.backend.transcript.update(
      EntryUpdate(agentId: 'a1', entry: _replied(3, 5000, 'Later')),
    );
    extras.value = [...extras.value, _card('incoming', 6000)];
    await settle(tester);
    expect(_top(tester, 'Card held'), lessThan(_top(tester, 'Answer')));
    expect(_top(tester, 'Card incoming'), greaterThan(_top(tester, 'Later')));
  });

  testWidgets('a hidden entry is left out, and tool calls around it stay '
      'one block', (tester) async {
    await _show(
      tester,
      [
        _ran(1, 1000, 'ls', reply: 1),
        _ran(2, 1001, 'pipe_send', reply: 1),
        _ran(3, 1002, 'pwd', reply: 1),
      ],
      ValueNotifier([]),
      hidden: (entry) =>
          entry.hasToolCall() && entry.toolCall.input.contains('pipe_send'),
    );
    expect(find.textContaining('pipe_send', findRichText: true), findsNothing);
    final store = TranscriptStore()
      ..load(
        Transcript(
          agentId: 'a1',
          entries: [
            _ran(1, 1000, 'ls', reply: 1),
            _ran(2, 1001, 'pipe_send', reply: 1),
            _ran(3, 1002, 'pwd', reply: 1),
          ],
        ),
      );
    bool hide(Entry entry) => entry.seq == Int64(2);
    expect(
      toolBlockAt(store, 0, hidden: hide),
      const ToolBlock(first: true, last: false),
    );
    expect(
      toolBlockAt(store, 2, hidden: hide),
      const ToolBlock(first: false, last: true),
    );
  });

  testWidgets("one older than what's held waits for the page it's in", (
    tester,
  ) async {
    final extras = ValueNotifier([_card('old', 100)]);
    final harness = await _show(
      tester,
      [_said(10, 5000, 'Recent')],
      extras,
      hasEarlier: true,
    );
    expect(find.text('Card old'), findsNothing, reason: 'no gap');
    harness.backend.transcript.prepend('a1', [
      _said(1, 50, 'First'),
    ], hasEarlier: false);
    await settle(tester);
    expect(_top(tester, 'Card old'), greaterThan(_top(tester, 'First')));
    expect(_top(tester, 'Card old'), lessThan(_top(tester, 'Recent')));
  });

  testWidgets('with no entries yet, the extras show on their own', (
    tester,
  ) async {
    await _show(
      tester,
      [],
      ValueNotifier([_card('one', 100), _card('two', 200)]),
    );
    expect(find.text('Card one'), findsOneWidget);
    expect(_top(tester, 'Card two'), greaterThan(_top(tester, 'Card one')));
  });

  testWidgets('going to an extra far back brings it to the top', (
    tester,
  ) async {
    final extras = ValueNotifier([_card('wanted', 1500)]);
    final harness = await _show(tester, [
      _said(1, 1000, 'Question'),
      for (var seq = 2; seq < 60; seq++) _replied(seq, 2000 + seq, 'Line $seq'),
    ], extras);
    final card = find.text('Card wanted');
    final view = tester.getRect(find.byType(TranscriptView));
    bool inView() =>
        card.evaluate().isNotEmpty && view.overlaps(tester.getRect(card));
    expect(inView(), isFalse);
    harness.backend.reveal.reachExtra('a1', 'pipe:wanted');
    await settle(tester);
    await settle(tester);
    expect(inView(), isTrue);
    expect(tester.getTopLeft(card).dy - view.top, lessThan(40));
    expect(harness.backend.reveal.extra, isNull, reason: 'used once');
  });

  testWidgets('going to one older than what is held loads back to it', (
    tester,
  ) async {
    final extras = ValueNotifier([_card('older', 100)]);
    late Harness harness;
    var loads = 0;
    harness = await _show(
      tester,
      [for (var seq = 10; seq < 60; seq++) _replied(seq, 1000 + seq, 'L$seq')],
      extras,
      hasEarlier: true,
      loadEarlier: () async {
        loads++;
        harness.backend.transcript.prepend('a1', [
          _said(1, 50, 'First'),
        ], hasEarlier: false);
      },
    );
    harness.backend.reveal.reachExtra('a1', 'pipe:older');
    for (var i = 0; i < 4; i++) {
      await settle(tester);
    }
    expect(loads, greaterThan(0));
    final card = find.text('Card older');
    expect(card, findsOneWidget);
    final view = tester.getRect(find.byType(TranscriptView));
    expect(view.overlaps(tester.getRect(card)), isTrue);
  });
}
