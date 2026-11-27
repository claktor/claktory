// Inline agent mentions: "@Kenji" or "@frontend-2" in an agent's
// conversation is a tag with the agent's character, name and role in its
// color; a click opens its conversation, and copied it's the name.
import 'package:factory_app/markdown.dart' show markdownToPlain;
import 'package:factory_app/mentions.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'chats_test.dart' show answered, chat, showChat;
import 'harness.dart';
import 'selection_test.dart' show rightClick, watchClipboard;

Agent _agent(
  String id,
  String name, {
  String role = '',
  String project = 'p1',
}) =>
    Agent(id: id, name: name, role: role, projectId: project, color: 'violet');

final _agents = [
  builder(working: false),
  _agent('frontend-2', 'Kenji', role: 'Frontend'),
  // An id that is also a name: the id wins.
  _agent('ana', 'Ana', role: 'Writer'),
  _agent('site-ana', 'Ana', project: 'p2'),
  // One name, two agents, no such id: no one.
  _agent('backend', 'Sam', role: 'Backend'),
  _agent('site-backend', 'Sam', project: 'p2'),
  _agent('mobile-2', 'Tomás', role: 'Mobile'),
];

Future<Harness> _show(WidgetTester tester, String text) async {
  final harness = Harness(workspace(agents: _agents));
  harness.backend.transcript.load(
    Transcript(
      agentId: 'a1',
      entries: [
        Entry(
          seq: Int64(1),
          assistant: AssistantText(text: text),
        ),
      ],
    ),
  );
  await harness.show(tester);
  // Nested selection containers take in their text a frame per level.
  await pumpFrames(tester, const Duration(milliseconds: 100));
  return harness;
}

List<String> _chips(WidgetTester tester) => [
  for (final chip in tester.widgetList<MentionChip>(find.byType(MentionChip)))
    chip.agent.id,
];

void main() {
  test(
    'a mention names the agent with that id, else the one with that name',
    () {
      String? named(String word) => resolveMention(word, _agents)?.id;
      expect(named('frontend-2'), 'frontend-2');
      expect(named('Kenji'), 'frontend-2');
      expect(named('kenji'), 'frontend-2', reason: 'case is ignored');
      expect(
        named('ana'),
        'ana',
        reason: "the id wins over the other Ana's name",
      );
      expect(named('Ana'), 'ana');
      expect(named('site-ana'), 'site-ana');
      expect(named('Sam'), isNull, reason: 'two agents are named Sam');
      expect(named('backend'), 'backend');
      expect(named('tomás'), 'mobile-2');
      expect(named('nobody'), isNull);
    },
  );

  testWidgets('mentions in a reply are tags; the rest stays text', (
    tester,
  ) async {
    await _show(
      tester,
      'Ask @Kenji, then @frontend-2. @Ana and @site-ana, not @Sam or '
      '@nobody. Mail ana@example.com; `@Kenji` is code.\n\n'
      '```\n@media (max-width: 600px) @Kenji\n```\n\n'
      '- @Tomás has the phone',
    );
    expect(_chips(tester), [
      'frontend-2',
      'frontend-2',
      'ana',
      'site-ana',
      'mobile-2',
    ]);
    final kenji = find.byType(MentionChip).first;
    expect(
      find.descendant(of: kenji, matching: find.text('Kenji')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: kenji, matching: find.text('  Frontend')),
      findsOneWidget,
    );
    expect(
      find.textContaining('@Sam or @nobody', findRichText: true),
      findsOneWidget,
    );
    expect(
      find.textContaining('ana@example.com', findRichText: true),
      findsOneWidget,
    );
    expect(find.textContaining('@media', findRichText: true), findsOneWidget);
    // Sized to the line it's in.
    final line = tester.getSize(
      find.textContaining('Ask', findRichText: true).first,
    );
    expect(tester.getSize(kenji).height, lessThanOrEqualTo(line.height));
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets("copied, a mention is the agent's name", (tester) async {
    final copied = watchClipboard(tester);
    await _show(tester, 'Ask @Kenji to review it, then tell @nobody.');
    await rightClick(tester, find.textContaining('review', findRichText: true));
    await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyA);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
    await tester.pump();
    expect(copied.last, 'Ask Kenji to review it, then tell @nobody.');
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets("a click on a tag opens that agent's conversation", (
    tester,
  ) async {
    final harness = await _show(tester, 'Ask @Kenji.');
    await tester.tap(find.byType(MentionChip));
    await tester.pump();
    expect(harness.last.selectAgent.agentId, 'frontend-2');
  });

  testWidgets('in a chat, with no agents, a mention stays text', (
    tester,
  ) async {
    await showChat(
      tester,
      with_: chat(id: 'c1', title: 'Trains'),
      entries: [answered(2, 'Ask @Rivet about it.')],
    );
    expect(find.byType(MentionChip), findsNothing);
    expect(
      find.textContaining('Ask @Rivet about it.', findRichText: true),
      findsOneWidget,
    );
  });

  test("markdownToPlain gives a mention as the agent's name", () {
    expect(
      markdownToPlain('Ask @Kenji, not @Sam or `@Kenji`.', agents: _agents),
      'Ask Kenji, not @Sam or @Kenji.',
    );
    expect(markdownToPlain('Ask @Kenji.'), 'Ask @Kenji.');
  });
}
