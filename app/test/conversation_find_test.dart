// Find in a conversation (⌘F): Claktory searches the whole conversation;
// the bar says which match of how many, goes up to older ones, loads back
// to one older than what's held, and marks the matches in the text.
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/conversation_find.dart';
import 'package:factory_app/find.dart';
import 'package:factory_app/inbox.dart' show NeedsYouChip, NeedsYouList;
import 'package:factory_app/markdown.dart' show Markdown;
import 'package:factory_app/message_box.dart' show MessageBox;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/transcript.dart' show TranscriptView, EntryView;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'selection_test.dart' show rightClick;

Entry _user(int seq, String text) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(seq * 1000),
  user: UserText(text: text),
);

Entry _reply(int seq, String text) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(seq * 1000),
  assistant: AssistantText(text: text),
);

ConversationMatch _match(int seq, {int hits = 1, String snippet = ''}) =>
    ConversationMatch(
      seq: Int64(seq),
      snippet: snippet,
      hits: hits,
      timestampMs: Int64(seq * 1000),
    );

Harness _harness(List<Entry> entries, {bool hasEarlier = false}) {
  final harness = Harness(
    workspace(agents: [builder(working: false)])..selectedAgentId = 'a1',
  );
  harness.backend.transcript.load(
    Transcript(agentId: 'a1', entries: entries, hasEarlier: hasEarlier),
  );
  return harness;
}

Iterable<SearchConversation> _searches(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasSearchConversation()) message.searchConversation,
];

Future<void> _command(
  WidgetTester tester,
  LogicalKeyboardKey key, {
  bool shift = false,
}) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
  if (shift) await tester.sendKeyDownEvent(LogicalKeyboardKey.shift);
  await tester.sendKeyEvent(key);
  if (shift) await tester.sendKeyUpEvent(LogicalKeyboardKey.shift);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
  await tester.pump();
}

Future<void> _type(WidgetTester tester, String words) async {
  await tester.enterText(find.byType(EditableText).first, words);
  // Searched a moment after the last key.
  await tester.pump(const Duration(milliseconds: 200));
  await pumpFrames(tester, const Duration(milliseconds: 300));
}

/// The washes in the text of entry [seq]: (the words, whether it's the
/// match gone to).
List<(String, bool)> _washes(WidgetTester tester, int seq) {
  final found = <(String, bool)>[];
  final texts = find.descendant(
    of: find.byWidgetPredicate(
      (widget) => widget is EntryView && widget.model.seq == seq,
    ),
    matching: find.byType(RichText),
  );
  for (final element in texts.evaluate()) {
    (element.widget as RichText).text.visitChildren((span) {
      final background = span.style?.backgroundColor;
      if (span is TextSpan && background != null && span.text != null) {
        found.add((span.text!, background.a > 0.9));
      }
      return true;
    });
  }
  return found;
}

String _count(WidgetTester tester) {
  final bar = find.byType(FindBar);
  final session = tester.widget<FindBar>(bar).session;
  return session.target.countLabel;
}

void main() {
  group('on a Mac', () {
    testWidgets('⌘F opens the bar; matches are counted, marked and gone '
        'through upward; Esc closes it', (tester) async {
      final harness = _harness([
        _user(1, 'Ship it today'),
        _reply(2, 'I shipped it, and the ship sailed.'),
        _reply(3, 'Anything else?'),
      ]);
      harness.backend.matches = (request) => ConversationMatches(
        query: request.query,
        agentId: request.agentId,
        matches: [_match(2, hits: 2), _match(1)],
        total: 3,
      );
      await harness.show(tester);
      await settle(tester);

      await _command(tester, LogicalKeyboardKey.keyF);
      await pumpFrames(tester);
      expect(find.byType(FindBar), findsOneWidget);
      expect(find.text('Search this conversation'), findsOneWidget);

      await _type(tester, 'ship');
      final search = _searches(harness).last;
      expect(search.agentId, 'a1');
      expect(search.query, 'ship');
      expect(search.hasBeforeSeq(), isFalse);
      expect(_count(tester), '1 of 3');
      // Every occurrence washed; the one gone to, the newest message's
      // last, strongly.
      expect(_washes(tester, 2), [('ship', false), ('ship', true)]);
      expect(_washes(tester, 1), [('Ship', false)]);
      expect(_washes(tester, 3), isEmpty);

      // Enter: the next, up the conversation; the same entry's first.
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(_count(tester), '2 of 3');
      expect(_washes(tester, 2), [('ship', true), ('ship', false)]);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(_count(tester), '3 of 3');
      expect(_washes(tester, 1), [('Ship', true)]);
      expect(_washes(tester, 2), [('ship', false), ('ship', false)]);
      // Past the oldest, round to the newest; ⇧⌘G back.
      await _command(tester, LogicalKeyboardKey.keyG);
      expect(_count(tester), '1 of 3');
      await _command(tester, LogicalKeyboardKey.keyG, shift: true);
      expect(_count(tester), '3 of 3');

      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await pumpFrames(tester);
      expect(find.byType(FindBar), findsNothing);
      expect(_washes(tester, 1), isEmpty);
      expect(_washes(tester, 2), isEmpty);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('in a long message, the occurrence gone to is brought into '
        'sight, clear of the foot, and so is the next one up in it', (
      tester,
    ) async {
      final filler = [for (var i = 0; i < 60; i++) 'Line $i of the log.'];
      final harness = _harness([
        _user(1, 'Where did it go?'),
        _reply(
          2,
          ['To the harbour.', ...filler, 'Back to the harbour.'].join('\n\n'),
        ),
        // Below it, enough to fill the view.
        _reply(3, filler.take(30).join('\n\n')),
      ]);
      harness.backend.matches = (request) => ConversationMatches(
        query: request.query,
        agentId: request.agentId,
        matches: [_match(2, hits: 2)],
        total: 2,
      );
      await harness.show(tester);
      await settle(tester);

      /// Where the [nth] occurrence shown is, in the conversation's height
      /// (0 its top, 1 its foot).
      double at(int nth) {
        final view = tester.getRect(find.byType(TranscriptView));
        final root = tester.renderObject(find.byType(TranscriptView));
        final (paragraph, start, end) = shownMatches(root, 'harbour')[nth];
        final rect = foundRect(paragraph, start, end)!;
        final top = paragraph.localToGlobal(rect.topLeft).dy;
        return (top - view.top) / view.height;
      }

      await _command(tester, LogicalKeyboardKey.keyF);
      await pumpFrames(tester);
      await _type(tester, 'harbour');
      expect(_count(tester), '1 of 2');
      // The last one, gone to first: in sight, above the foot.
      expect(at(1), inInclusiveRange(0.05, 0.7));
      expect(at(0), lessThan(0));

      // Enter: the one at the message's top, far up it.
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await pumpFrames(tester);
      expect(_count(tester), '2 of 2');
      expect(at(0), inInclusiveRange(0.05, 0.7));
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets("a match at the conversation's very end stays followed: "
        'Latest never shows over it', (tester) async {
      final filler = [for (var i = 0; i < 60; i++) 'Line $i of the log.'];
      final harness = _harness([
        _user(1, 'Where did it go?'),
        _reply(2, [...filler, 'Back to the harbour.'].join('\n\n')),
      ]);
      harness.backend.matches = (request) => ConversationMatches(
        query: request.query,
        agentId: request.agentId,
        matches: [_match(2)],
        total: 1,
      );
      await harness.show(tester);
      await settle(tester);
      double latest() => tester
          .widget<AnimatedOpacity>(
            find.ancestor(
              of: find.byKey(const ValueKey('latest')),
              matching: find.byType(AnimatedOpacity),
            ),
          )
          .opacity;
      expect(latest(), 0);

      await _command(tester, LogicalKeyboardKey.keyF);
      await pumpFrames(tester);
      await _type(tester, 'harbour');
      expect(_count(tester), '1 of 1');
      expect(latest(), 0);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('in a message full of matches, only the occurrence gone to '
        'is marked strongly, and each step moves it; where the message '
        "shows other than what was found, all of it is", (tester) async {
      final harness = _harness([
        _user(1, 'Say it often'),
        _reply(2, 'word one word two\n\nword three\n\n- word four'),
      ]);
      var hits = 4;
      harness.backend.matches = (request) => ConversationMatches(
        query: request.query,
        agentId: request.agentId,
        matches: [_match(2, hits: hits)],
        total: hits,
      );
      await harness.show(tester);
      await settle(tester);
      await _command(tester, LogicalKeyboardKey.keyF);
      await pumpFrames(tester);
      await _type(tester, 'word');
      await pumpFrames(tester);
      expect(_count(tester), '1 of 4');
      List<bool> strong() => [for (final (_, s) in _washes(tester, 2)) s];
      // The last, in the list, first: up the message from there.
      expect(strong(), [false, false, false, true]);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await pumpFrames(tester);
      expect(_count(tester), '2 of 4');
      expect(strong(), [false, false, true, false]);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await pumpFrames(tester);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await pumpFrames(tester);
      expect(_count(tester), '4 of 4');
      expect(strong(), [true, false, false, false]);

      // Found 5 where it shows 4: which is which can't be told.
      hits = 5;
      await _type(tester, 'wor');
      await _type(tester, 'word');
      await pumpFrames(tester);
      expect(_count(tester), '1 of 5');
      expect(strong(), [true, true, true, true]);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('a match in a tool call is gone to at its line, over the '
        'input and output it opens', (tester) async {
      final output = [for (var i = 0; i < 80; i++) 'line $i of notes.md'];
      final harness = _harness([
        _user(1, 'Show me the notes'),
        Entry(
          seq: Int64(2),
          timestampMs: Int64(2000),
          toolCall: ToolCall(
            callId: 'c2',
            name: 'read_file',
            input: '{"path": "notes.md"}',
            output: output.join('\n'),
            state: ToolCallState.TOOL_CALL_STATE_DONE,
          ),
        ),
        _reply(3, [for (var i = 0; i < 40; i++) 'Point $i.'].join('\n\n')),
      ]);
      harness.backend.matches = (request) => ConversationMatches(
        query: request.query,
        agentId: request.agentId,
        matches: [_match(2)],
        total: 1,
      );
      await harness.show(tester);
      await settle(tester);
      await _command(tester, LogicalKeyboardKey.keyF);
      await pumpFrames(tester);
      await _type(tester, 'notes.md');
      await settle(tester);
      expect(_count(tester), '1 of 1');
      // Its first occurrence, the call's line, in sight under the bar.
      final view = tester.getRect(find.byType(TranscriptView));
      final root = tester.renderObject(find.byType(TranscriptView));
      final (paragraph, start, end) = shownMatches(root, 'notes.md').first;
      final top = paragraph
          .localToGlobal(foundRect(paragraph, start, end)!.topLeft)
          .dy;
      expect((top - view.top) / view.height, inInclusiveRange(0.1, 0.7));
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets("Needs you's chip steps aside while the bar is open, so "
        'its arrows and Done can be tapped (an iPad by touch)', (tester) async {
      final harness = Harness(
        workspace(agents: [builder(working: false)..waitingSeqs.add(Int64(1))])
          ..selectedAgentId = 'a1',
      );
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [_user(1, 'Ship it today'), _reply(2, 'Shipped.')],
        ),
      );
      harness.backend.matches = (request) => ConversationMatches(
        query: request.query,
        agentId: request.agentId,
        matches: [_match(1)],
        total: 1,
      );
      await harness.show(tester);
      await settle(tester);
      final chip = find.byType(NeedsYouChip);
      expect(chip, findsOneWidget);

      await _command(tester, LogicalKeyboardKey.keyF);
      await pumpFrames(tester);
      await _type(tester, 'ship');
      expect(chip, findsNothing);
      expect(find.text('Done').hitTestable(), findsOneWidget);
      // Done by touch closes it, and the chip is back.
      await tester.tap(find.text('Done').hitTestable());
      await pumpFrames(tester);
      expect(find.byType(FindBar), findsNothing);
      expect(chip, findsOneWidget);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('⌘F in the message box finds in the conversation', (
      tester,
    ) async {
      final harness = _harness([_reply(1, 'Hello there')]);
      await harness.show(tester);
      await settle(tester);
      final box = find.descendant(
        of: find.byType(MessageBox),
        matching: find.byType(EditableText),
      );
      await tester.tap(box);
      await tester.pump();
      expect(tester.widget<EditableText>(box).focusNode.hasFocus, isTrue);
      await _command(tester, LogicalKeyboardKey.keyF);
      await pumpFrames(tester);
      expect(find.byType(FindBar), findsOneWidget);
      final session = tester.widget<FindBar>(find.byType(FindBar)).session;
      expect(session.field.hasFocus, isTrue);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets("the header's Find and the conversation's right-click "
        'Find… open the bar, its field focused', (tester) async {
      final harness = _harness([_reply(1, 'Hello there')]);
      await harness.show(tester);
      await settle(tester);
      await _findFromMenu(tester);
      await pumpFrames(tester);
      expect(find.byType(FindBar), findsOneWidget);
      var session = tester.widget<FindBar>(find.byType(FindBar)).session;
      expect(session.field.hasFocus, isTrue);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await pumpFrames(tester);
      expect(find.byType(FindBar), findsNothing);

      await rightClick(
        tester,
        find.textContaining('Hello there', findRichText: true).first,
      );
      await pumpFrames(tester);
      await tester.tap(find.text('Find…'));
      await pumpFrames(tester);
      expect(find.byType(FindBar), findsOneWidget);
      session = tester.widget<FindBar>(find.byType(FindBar)).session;
      expect(session.field.hasFocus, isTrue);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    // Needs you open as a pane beside the conversation.
    ViewNode beside() => ViewNode(
      id: 's1',
      split: ViewSplit(
        axis: ViewAxis.VIEW_AXIS_ROW,
        children: [
          ViewNode(
            id: 'chat',
            leaf: ViewLeaf(kind: 'chat'),
          ),
          ViewNode(
            id: 'v1',
            leaf: ViewLeaf(kind: 'inbox'),
          ),
        ],
      ),
    );

    Harness besideNeedsYou() {
      final harness = Harness(
        workspace(agents: [builder(working: false)..layout = beside()])
          ..selectedAgentId = 'a1',
      );
      harness.backend.transcript.load(
        Transcript(agentId: 'a1', entries: [_reply(1, 'Hello there')]),
      );
      return harness;
    }

    FindTarget openTarget(WidgetTester tester) =>
        tester.widget<FindBar>(find.byType(FindBar)).session.target;

    testWidgets("with Needs you beside it, the header's Find opens the "
        "conversation's bar, not Needs you's", (tester) async {
      final harness = besideNeedsYou();
      await harness.show(tester, size: const Size(1400, 900));
      await settle(tester);
      expect(find.byType(NeedsYouList), findsOneWidget);
      await _findFromMenu(tester);
      await pumpFrames(tester);
      expect(find.byType(FindBar), findsOneWidget);
      expect(openTarget(tester), isA<ConversationFind>());
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets("a match older than what's held loads back to it and shows "
        'it; nothing found says so', (tester) async {
      final harness = _harness([
        for (var seq = 60; seq < 80; seq++) _reply(seq, 'Line $seq'),
      ], hasEarlier: true);
      harness.backend.earlierEntries = [
        for (var seq = 0; seq < 60; seq++)
          _reply(seq, seq == 4 ? 'The needle is here' : 'Line $seq'),
      ];
      harness.backend.matches = (request) => ConversationMatches(
        query: request.query,
        agentId: request.agentId,
        matches: [if (request.query == 'needle') _match(4)],
        total: request.query == 'needle' ? 1 : 0,
      );
      await harness.show(tester);
      await settle(tester);

      await _command(tester, LogicalKeyboardKey.keyF);
      await pumpFrames(tester);
      await _type(tester, 'nothing like it');
      expect(_count(tester), 'No matches');

      await _type(tester, 'needle');
      await pumpFrames(tester);
      final loads = [
        for (final message in harness.backend.sent)
          if (message.hasLoadEntries()) message.loadEntries,
      ];
      expect(loads.single.fromSeq.toInt(), 4);
      expect(_count(tester), '1 of 1');
      final entry = find.byWidgetPredicate(
        (widget) => widget is EntryView && widget.model.seq == 4,
      );
      expect(entry, findsOneWidget);
      final view = tester.getRect(find.byType(TranscriptView));
      final at = tester.getRect(entry);
      // In sight, below the bar.
      expect(at.top, greaterThan(view.top + 40));
      expect(at.top, lessThan(view.bottom - 40));
      expect(_washes(tester, 4), [('needle', true)]);
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  });

  group('the matches', () {
    late List<({bool pipe, int? beforeSeq})> asked;
    late List<FoundAt> gone;

    ConversationFind find(
      Map<(bool, int?), ConversationMatches> pages, {
      bool pipe = false,
    }) {
      asked = [];
      gone = [];
      return ConversationFind(
        pipe: pipe,
        search: (query, {required pipe, beforeSeq}) async {
          asked.add((pipe: pipe, beforeSeq: beforeSeq));
          return pages[(pipe, beforeSeq)] ?? ConversationMatches();
        },
        go: (at, _, _) => gone.add(at),
      );
    }

    ConversationMatch at(int seq, int ms, {int hits = 1}) =>
        ConversationMatch(seq: Int64(seq), hits: hits, timestampMs: Int64(ms));

    test("page on as the next match needs it, older than the last match's "
        'seq', () async {
      final target = find({
        (false, null): ConversationMatches(
          matches: [at(9, 900), at(7, 700)],
          more: true,
          total: 3,
        ),
        (false, 7): ConversationMatches(matches: [at(2, 200)]),
      });
      target.search('x');
      await pumpEventQueue();
      expect(target.countLabel, '1 of 3');
      expect(gone, [(pipe: false, seq: 9, queued: null)]);
      target.step(1);
      expect(asked, hasLength(1));
      target.step(1);
      await pumpEventQueue();
      expect(asked.last, (pipe: false, beforeSeq: 7));
      expect(target.countLabel, '3 of 3');
      expect(gone.last, (pipe: false, seq: 2, queued: null));
      // Round to the newest.
      target.step(1);
      expect(gone.last, (pipe: false, seq: 9, queued: null));
    });

    test('back from the newest goes to the oldest, loading all', () async {
      final target = find({
        (false, null): ConversationMatches(
          matches: [at(9, 900)],
          more: true,
          total: 2,
        ),
        (false, 9): ConversationMatches(matches: [at(2, 200)]),
      });
      target.search('x');
      await pumpEventQueue();
      target.step(-1);
      await pumpEventQueue();
      expect(target.countLabel, '2 of 2');
      expect(gone.last, (pipe: false, seq: 2, queued: null));
    });

    test("in a pipe, its messages and the liaison's conversation in time "
        'order; one log ahead of the other waits for it', () async {
      final target = find({
        (false, null): ConversationMatches(
          matches: [at(50, 5000), at(40, 4000)],
          more: true,
          total: 3,
        ),
        (true, null): ConversationMatches(
          matches: [at(8, 4500), at(3, 1000)],
          total: 2,
        ),
        (false, 40): ConversationMatches(matches: [at(10, 2000)]),
      }, pipe: true);
      target.search('x');
      await pumpEventQueue();
      expect(target.countLabel, '1 of 5');
      for (var i = 0; i < 4; i++) {
        target.step(1);
        await pumpEventQueue();
      }
      expect(gone, [
        (pipe: false, seq: 50, queued: null),
        (pipe: true, seq: 8, queued: null),
        (pipe: false, seq: 40, queued: null),
        // The conversation's next page came before the pipe's older one.
        (pipe: false, seq: 10, queued: null),
        (pipe: true, seq: 3, queued: null),
      ]);
      expect(asked, [
        (pipe: false, beforeSeq: null),
        (pipe: true, beforeSeq: null),
        (pipe: false, beforeSeq: 40),
      ]);
    });

    test(
      "an answer to an earlier search is dropped; a failure says why",
      () async {
        final target = ConversationFind(
          search: (query, {required pipe, beforeSeq}) async {
            if (query == 'slow') {
              await Future<void>.delayed(const Duration(milliseconds: 10));
              return ConversationMatches(matches: [at(1, 100)], total: 1);
            }
            throw const RequestFailure("Can't search right now.");
          },
          go: (_, _, _) {},
        );
        target.search('slow');
        target.search('');
        await Future<void>.delayed(const Duration(milliseconds: 20));
        expect(target.count, 0);
        expect(target.countLabel, '');
        target.search('other');
        await pumpEventQueue();
        expect(target.countLabel, "Can't search right now.");
      },
    );

    test('a Claktory from before search says to update it, not its word '
        'for a request it doesn\'t know', () async {
      final target = ConversationFind(
        search: (query, {required pipe, beforeSeq}) async =>
            throw const RequestFailure(
              'unknown message',
              code: ErrorCode.ERROR_CODE_UNKNOWN_MESSAGE,
            ),
        go: (_, _, _) {},
      );
      target.search('plum');
      await pumpEventQueue();
      expect(target.countLabel, 'Update Claktory to search');
    });
  });

  group('marked text', () {
    test('the one occurrence at a place in the words: its own words before '
        "its children's, a placeholder one character", () {
      final span = TextSpan(
        text: 'ab x ',
        children: [
          const WidgetSpan(child: SizedBox()),
          const TextSpan(text: 'x', style: TextStyle(fontSize: 9)),
          TextSpan(
            text: ' x',
            children: [const TextSpan(text: ' x')],
          ),
        ],
      );
      List<bool> strong(int at) {
        final found = <bool>[];
        markFound(span, 'x', current: true, at: at).visitChildren((span) {
          final background = span.style?.backgroundColor;
          if (span is TextSpan && background != null) {
            found.add(background.a > 0.9);
          }
          return true;
        });
        return found;
      }

      // "ab x " then the placeholder (5), "x" (6), " x" (8), " x" (10).
      expect(span.toPlainText().indexOf('x', 6), 6);
      expect(strong(3), [true, false, false, false]);
      expect(strong(6), [false, true, false, false]);
      expect(strong(8), [false, false, true, false]);
      expect(strong(10), [false, false, false, true]);
      expect(strong(-1), [false, false, false, false]);
    });

    List<String> washed(WidgetTester tester) => [
      for (final element in find.byType(RichText).evaluate())
        ...() {
          final found = <String>[];
          (element.widget as RichText).text.visitChildren((span) {
            if (span is TextSpan &&
                span.style?.backgroundColor != null &&
                span.text != null) {
              found.add(span.text!);
            }
            return true;
          });
          return found;
        }(),
    ];

    testWidgets("markdown's paragraphs, lists and code are marked, and "
        'rebuilt as the query changes', (tester) async {
      final query = ValueNotifier('');
      await tester.pumpWidget(
        WidgetsApp(
          color: const Color(0xFF000000),
          builder: (context, _) => ValueListenableBuilder(
            valueListenable: query,
            builder: (context, query, child) =>
                FindHighlight(query: query, child: child!),
            child: const Markdown(
              'A **bold** needle here.\n\n- a needle listed\n\n'
              '```\nlet needle = 1;\n```',
            ),
          ),
        ),
      );
      expect(washed(tester), isEmpty);
      query.value = 'NEEDLE';
      await tester.pump();
      expect(washed(tester), ['needle', 'needle', 'needle']);
      query.value = 'bold needle';
      await tester.pump();
      // Across two styles: not marked.
      expect(washed(tester), isEmpty);
    });

    test('a span with no match comes back as it was', () {
      const span = TextSpan(
        text: 'nothing here',
        children: [TextSpan(text: ' or here')],
      );
      expect(identical(markFound(span, 'needle'), span), isTrue);
      final marked = markFound(span, 'here');
      expect(identical(marked, span), isFalse);
      expect(marked.toPlainText(), span.toPlainText());
    });
  });

  testWidgets("the messages queued at its end are found too, newest of all "
      "(Zucchero's row 207)", (tester) async {
    final harness = _harness([_user(1, 'FYI an old one'), _reply(2, 'Noted.')]);
    harness.backend.matches = (request) => ConversationMatches(
      query: request.query,
      agentId: request.agentId,
      matches: [_match(1)],
      total: 1,
    );
    harness.backend.queued.apply(
      QueuedMessages(
        agentId: 'a1',
        messages: [
          QueuedMessage(
            id: Int64(1),
            source: 'agent',
            sender: 'Writer',
            senderId: 'a3',
            text: 'FYI the build is green.',
            quiet: true,
          ),
          QueuedMessage(
            id: Int64(2),
            source: 'person',
            text: 'FYI, and fyi again',
          ),
        ],
      ),
    );
    await harness.show(tester, size: const Size(1200, 1400));
    await settle(tester);

    /// The washes in the bubble showing [words].
    List<(String, bool)> washes(String words) {
      final found = <(String, bool)>[];
      for (final element
          in find
              .byWidgetPredicate(
                (widget) =>
                    widget is RichText &&
                    widget.text.toPlainText().contains(words),
              )
              .evaluate()) {
        (element.widget as RichText).text.visitChildren((span) {
          final background = span.style?.backgroundColor;
          if (span is TextSpan && background != null && span.text != null) {
            found.add((span.text!, background.a > 0.9));
          }
          return true;
        });
      }
      return found;
    }

    await _command(tester, LogicalKeyboardKey.keyF);
    await pumpFrames(tester);
    await _type(tester, 'fyi');
    // The person's queued message (newest) twice, Writer's once, the
    // conversation's once.
    expect(_count(tester), '1 of 4');
    expect(washes('and fyi again'), [('FYI', false), ('fyi', true)]);
    expect(washes('the build is green'), [('FYI', false)]);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(_count(tester), '2 of 4');
    expect(washes('and fyi again'), [('FYI', true), ('fyi', false)]);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(_count(tester), '3 of 4');
    expect(washes('the build is green'), [('FYI', true)]);
    // Its "Quiet · read when …" line is no match.
    expect(washes('read when'), isEmpty);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(_count(tester), '4 of 4');
    expect(_washes(tester, 1), [('FYI', true)]);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets("on an iPad, the keyboard's Return goes to the next match, the "
      "field kept (Zucchero's row 214)", (tester) async {
    asIPad();
    final harness = _harness([
      _user(1, 'Ship it today'),
      _reply(2, 'I shipped it, and the ship sailed.'),
    ]);
    harness.backend.matches = (request) => ConversationMatches(
      query: request.query,
      agentId: request.agentId,
      matches: [_match(2, hits: 2), _match(1)],
      total: 3,
    );
    await harness.show(tester, size: const Size(1180, 820));
    await settle(tester);
    // Find… in the header's ⋯, where its magnifying glass went.
    await _findFromMenu(tester);
    await pumpFrames(tester);
    await _type(tester, 'ship');
    expect(_count(tester), '1 of 3');
    final field = tester.widget<FindBar>(find.byType(FindBar)).session.field;
    expect(field.hasFocus, isTrue);
    await tester.testTextInput.receiveAction(TextInputAction.search);
    await tester.pump();
    expect(_count(tester), '2 of 3');
    expect(field.hasFocus, isTrue, reason: 'the keyboard stays');
    await tester.testTextInput.receiveAction(TextInputAction.search);
    await tester.pump();
    expect(_count(tester), '3 of 3');
  });
}

/// Find… in the agent's ⋯ (agent-view.md §4), where the header's
/// magnifying glass was.
Future<void> _findFromMenu(WidgetTester tester) async {
  await tester.tap(
    find.byWidgetPredicate(
      (widget) => widget is GlyphButton && widget.label.startsWith('More options for '),
    ),
  );
  await pumpFrames(tester);
  await tester.tap(find.text('Find…'));
}
