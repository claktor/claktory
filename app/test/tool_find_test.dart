// Find in a conversation, inside tool calls (#320): a match in a folded
// call opens it, once, and its words are marked as a message's are, in the
// header, the input and the output, escaped first; the person's own close
// wins; and an agents_send's message opens to its match.
import 'dart:convert';

import 'package:factory_app/find.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/transcript.dart' show EntryView;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

Entry _call(
  int seq,
  String name,
  Map<String, Object?> input, {
  String output = '',
}) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(seq * 1000),
  toolCall: ToolCall(
    callId: 'c$seq',
    name: name,
    input: jsonEncode(input),
    output: output,
    state: ToolCallState.TOOL_CALL_STATE_DONE,
  ),
);

Harness _harness(List<Entry> entries) {
  final harness = Harness(
    workspace(agents: [builder(working: false)])..selectedAgentId = 'a1',
  );
  harness.backend.transcript.load(Transcript(agentId: 'a1', entries: entries));
  return harness;
}

/// Answers every search with [seqs], newest first, one hit each.
void _answer(Harness harness, List<int> seqs) =>
    harness.backend.matches = (request) => ConversationMatches(
      query: request.query,
      agentId: request.agentId,
      matches: [
        for (final seq in seqs)
          ConversationMatch(
            seq: Int64(seq),
            hits: 1,
            timestampMs: Int64(seq * 1000),
          ),
      ],
      total: seqs.length,
    );

Future<void> _find(WidgetTester tester, String words) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
  await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
  await pumpFrames(tester);
  await tester.enterText(find.byType(EditableText).first, words);
  await tester.pump(const Duration(milliseconds: 200));
  await pumpFrames(tester, const Duration(milliseconds: 300));
  await settle(tester);
}

Finder _entry(int seq) => find.byWidgetPredicate(
  (widget) => widget is EntryView && widget.model.seq == seq,
);

/// The washes in entry [seq]'s text: (the words, whether it's the match
/// gone to).
List<(String, bool)> _washes(WidgetTester tester, int seq) {
  final found = <(String, bool)>[];
  final texts = find.descendant(
    of: _entry(seq),
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

/// Every piece of text entry [seq] shows, joined.
String _shown(WidgetTester tester, int seq) {
  final buffer = StringBuffer();
  final texts = find.descendant(
    of: _entry(seq),
    matching: find.byType(RichText),
  );
  for (final element in texts.evaluate()) {
    buffer.write((element.widget as RichText).text.toPlainText());
    buffer.write('\n');
  }
  return buffer.toString();
}

bool _open(WidgetTester tester, int seq) => find
    .descendant(of: _entry(seq), matching: find.text('Output'))
    .evaluate()
    .isNotEmpty;

/// ⌘F is the Mac's.
final _mac = TargetPlatformVariant.only(TargetPlatform.macOS);

void main() {
  testWidgets('a match inside a folded tool call opens it and is marked in '
      'its header, input and output; moving on leaves it open', (tester) async {
    final harness = _harness([
      _call(1, 'bash', {
        'command': 'grep -n needle notes.txt',
      }, output: '3: the needle is here'),
      _call(2, 'bash', {'command': 'ls'}, output: 'a needle.txt'),
    ]);
    _answer(harness, [2, 1]);
    await harness.show(tester);
    await settle(tester);
    expect(_open(tester, 1), isFalse);
    expect(_open(tester, 2), isFalse);

    await _find(tester, 'needle');
    expect(
      tester.widget<FindBar>(find.byType(FindBar)).session.target.countLabel,
      '1 of 2',
    );
    // The newest, gone to, opens; the other stays folded.
    expect(_open(tester, 2), isTrue);
    expect(_open(tester, 1), isFalse);
    expect(_washes(tester, 2), contains(('needle', true)));
    // Folded, its header still shows the command, marked.
    expect(_washes(tester, 1), [('needle', false)]);

    // On to the older: it opens, and the newer one stays open.
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(_open(tester, 1), isTrue);
    expect(_open(tester, 2), isTrue);
    // The header, the input and the output.
    expect(
      _washes(tester, 1).where((wash) => wash.$1 == 'needle'),
      hasLength(3),
    );
    expect(_washes(tester, 1).every((wash) => wash.$2), isTrue);
  }, variant: _mac);

  testWidgets("the person's own close wins: a find doesn't fold or reopen "
      'it while it stays on the same match', (tester) async {
    final harness = _harness([
      _call(1, 'bash', {'command': 'echo hi'}, output: 'a needle'),
    ]);
    _answer(harness, [1]);
    await harness.show(tester);
    await settle(tester);
    await _find(tester, 'needle');
    expect(_open(tester, 1), isTrue);

    // Closed by a click on its header.
    await tester.tap(
      find.descendant(of: _entry(1), matching: find.text('echo hi')),
    );
    await settle(tester);
    expect(_open(tester, 1), isFalse);
    await pumpFrames(tester);
    expect(_open(tester, 1), isFalse);
  }, variant: _mac);

  testWidgets('what a tool call shows is escaped before it is marked: a '
      'hidden character shows as its code, never split by a match, and a '
      'query with one raw matches nothing', (tester) async {
    final harness = _harness([
      _call(1, 'bash', {'command': 'cat notes'}, output: 'abc\u202Edef needle'),
    ]);
    _answer(harness, [1]);
    await harness.show(tester);
    await settle(tester);
    await _find(tester, 'needle');
    expect(_open(tester, 1), isTrue);
    expect(_washes(tester, 1), [('needle', true)]);
    // The override shows as its code, whole, and isn't in the text raw.
    expect(_shown(tester, 1), contains(r'\u{202E}'));
    expect(_shown(tester, 1), isNot(contains('\u202E')));

    // Searching for the raw character marks nothing.
    await tester.enterText(find.byType(EditableText).first, 'c\u202Ed');
    await tester.pump(const Duration(milliseconds: 200));
    await settle(tester);
    expect(_washes(tester, 1), isEmpty);
  }, variant: _mac);

  testWidgets("an agents_send's message, folded to its first line, opens "
      'to a match further in', (tester) async {
    final harness = _harness([
      _call(1, 'agents_send', {
        'to': 'a1',
        'text': 'Please look at this.\n\nThe needle is in the second part.',
      }),
    ]);
    _answer(harness, [1]);
    await harness.show(tester);
    await settle(tester);
    expect(find.text('Show less'), findsNothing);

    await _find(tester, 'needle');
    expect(find.text('Show less'), findsOneWidget);
    expect(_washes(tester, 1), contains(('needle', true)));
  }, variant: _mac);
}
