// VoiceOver's press is the control's semantics tap (VO-Space on a Mac,
// whose embedder has no fallback; a double tap on an iPhone or iPad). A
// control that reads as one button over its own parts (Semantics with
// button and excludeSemantics) drops those parts' tap with their words, so
// it gives its own (Abbacchio's row 1716). One of each kind is pressed
// here the way VoiceOver does; the scan keeps new ones from leaving it out.
import 'dart:io';

import 'package:factory_app/chats.dart' show ChatHistory;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/team_activity.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

/// Presses the node labelled [label] as VoiceOver does.
Future<void> _press(WidgetTester tester, Pattern label) async {
  final node = tester.getSemantics(find.bySemanticsLabel(label).last);
  expect(
    node.getSemanticsData().hasAction(SemanticsAction.tap),
    isTrue,
    reason: '$label has its own tap',
  );
  tester.binding.renderViews.first.owner!.semanticsOwner!.performAction(
    node.id,
    SemanticsAction.tap,
  );
  await settle(tester);
}

/// How many in a file read as one button but leave the press to
/// something with semantics of its own.
const _pressedElsewhere = {
  // The app list's row: its label sits inside the row's Pressable, whose
  // tap is on the same node.
  'lib/app_dock.dart': 1,
};

void main() {
  test('every control read as one button gives its own tap', () {
    final missing = <String>[];
    final allowed = Map.of(_pressedElsewhere);
    for (final file in Directory('lib').listSync(recursive: true)) {
      if (file is! File ||
          !file.path.endsWith('.dart') ||
          file.path.contains('/protocol/')) {
        continue;
      }
      final source = file.readAsStringSync();
      for (final match in RegExp(r'Semantics\(').allMatches(source)) {
        final rest = source.substring(
          match.end,
          (match.end + 800).clamp(0, source.length),
        );
        final end = rest.indexOf('child:');
        final args = end < 0 ? rest : rest.substring(0, end);
        if (!args.contains('button: true') ||
            !args.contains('excludeSemantics: true') ||
            args.contains('onTap')) {
          continue;
        }
        final line = '\n'.allMatches(source.substring(0, match.start)).length;
        final left = allowed[file.path] ?? 0;
        if (left > 0) {
          allowed[file.path] = left - 1;
          continue;
        }
        missing.add('${file.path}:${line + 1}');
      }
    }
    expect(missing, isEmpty);
  });

  testWidgets('a toggle: "Thought 3 times" opens', (tester) async {
    final harness = Harness(workspace(agents: [builder()]));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            user: UserText(text: 'Fix it'),
          ),
          for (var seq = 2; seq <= 4; seq++)
            Entry(
              seq: Int64(seq),
              thinking: Thinking(text: 'Idea $seq'),
            ),
          Entry(
            seq: Int64(5),
            assistant: AssistantText(text: 'Done.'),
          ),
        ],
      ),
    );
    final semantics = tester.ensureSemantics();
    await harness.show(tester);
    await settle(tester);
    await _press(tester, 'Thought 3 times. Show thoughts.');
    expect(
      find.bySemanticsLabel('Thought 3 times. Hide thoughts.'),
      findsOneWidget,
    );
    semantics.dispose();
  });

  testWidgets('a list row: a chat in the history opens', (tester) async {
    final harness = Harness(
      Workspace(
        projects: [Project(id: 'p1', name: 'Claktory', path: '/w')],
        agents: [
          Agent(
            id: 'mike',
            projectId: 'p1',
            name: 'Mike',
            modelAvailable: true,
          ),
        ],
        selectedAgentId: 'mike',
        chats: [
          Chat(
            id: 'chat-1',
            title: 'Tide tables',
            created: Int64(1001),
            updated: Int64(1001),
            modelAvailable: true,
          ),
        ],
      ),
    );
    final opened = <Chat>[];
    final semantics = tester.ensureSemantics();
    await harness.show(tester, home: ChatHistory(onOpen: opened.add));
    await pumpFrames(tester);
    await _press(tester, RegExp('^Tide tables'));
    expect([for (final chat in opened) chat.id], ['chat-1']);
    semantics.dispose();
  });

  testWidgets('a badge on a Mac: the team at work opens the Map', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final team = workspace(
      agents: [
        Agent(
          id: 'a',
          name: 'Ana',
          projectId: 'p1',
          modelAvailable: true,
          status: AgentStatus.AGENT_STATUS_WORKING,
        ),
      ],
      selected: '',
    );
    final harness = Harness(team);
    final semantics = tester.ensureSemantics();
    await harness.show(
      tester,
      home: Center(child: TeamActivity(workspace: team)),
    );
    await _press(tester, RegExp('agents working. Open the Map'));
    expect(harness.page.value, AppPage.map);
    semantics.dispose();
  });
}
