// Find in a pipe with no liaison (Zucchero, via Prosciutto): ⌘F searches
// the pipe's own messages, all of them, goes to a match far up the chat
// and marks it.
import 'package:factory_app/find.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

PipeMessage _message(int seq, String text) => PipeMessage(
  seq: Int64(seq),
  timestampMs: Int64(1000 + seq),
  author: PipeAuthor(
    kind: PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_AGENT,
    name: 'Bo',
  ),
  text: text,
  state: PipeMessageState.PIPE_MESSAGE_STATE_RECEIVED,
);

void main() {
  testWidgets('⌘F in a pipe with no liaison searches its messages, goes to '
      'one far up and marks it', (tester) async {
    final harness = Harness(
      Workspace(
        projects: [Project(id: 'p1', name: 'Factory', path: '/w')],
        agents: [
          Agent(
            id: 'june',
            name: 'June',
            projectId: 'p1',
            modelAvailable: true,
          ),
        ],
        pipes: [
          Pipe(
            id: 'pipe1',
            name: 'Bugs',
            projectId: 'p1',
            state: PipeState.PIPE_STATE_OPEN,
            createdMs: Int64(1000),
            peer: PipePeer(claktoryName: "Ana's Claktory", ownerName: 'Ana'),
            online: true,
          ),
        ],
        selectedPipeId: 'pipe1',
        selectedAgentId: 'june',
      ),
    );
    harness.backend.matches = (request) => ConversationMatches(
      query: request.query,
      pipeId: request.pipeId,
      matches: [
        ConversationMatch(
          seq: Int64(2),
          hits: 1,
          timestampMs: Int64(1002),
        ),
      ],
      total: 1,
    );
    await harness.show(tester, size: const Size(1200, 700));
    harness.backend.pipeChat.load(
      PipeTranscript(
        pipeId: 'pipe1',
        messages: [
          for (var i = 1; i <= 60; i++)
            _message(
              i,
              i == 2
                  ? 'The crash is in the uploader'
                  : 'Message $i, nothing to see',
            ),
        ],
      ),
    );
    await settle(tester);
    expect(find.text('The crash is in the uploader'), findsNothing);

    // Writing in the pipe's box, as one does.
    await tester.tap(find.byType(EditableText).last);
    await tester.pump();
    await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
    await pumpFrames(tester);
    final session = tester.widget<FindBar>(find.byType(FindBar)).session;
    await tester.enterText(
      find.byWidgetPredicate(
        (widget) => widget is EditableText && widget.focusNode == session.field,
      ),
      'crash',
    );
    await tester.pump(const Duration(milliseconds: 200));
    await pumpFrames(tester, const Duration(milliseconds: 500));

    final search = harness.backend.sent
        .lastWhere((message) => message.hasSearchConversation())
        .searchConversation;
    expect(search.pipeId, 'pipe1');
    expect(search.agentId, '');
    expect(search.query, 'crash');
    expect(session.target.countLabel, '1 of 1');
    // Gone up to it, and marked.
    final message = find.textContaining('The crash', findRichText: true);
    expect(message.hitTestable(), findsOneWidget);
    final washes = <(String, bool)>[];
    tester.widget<RichText>(message.first).text.visitChildren((span) {
      final background = span.style?.backgroundColor;
      if (span is TextSpan && background != null && span.text != null) {
        washes.add((span.text!, background.a > 0.9));
      }
      return true;
    });
    expect(washes, [('crash', true)]);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
}
