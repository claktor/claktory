// A busy team changes the Workspace several times a second (each agent's
// step, its mood, its context), and the Mac app gets every change. Each
// must build only what shows it, not every sidebar row and every message
// in view, so an idle app costs little CPU.

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/kept.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/workspace_patch.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'map_fixture.dart';

/// Claktory's own team (~70 agents), its first agent open with a
/// conversation, every other one working.
Future<Harness> _busy(WidgetTester tester) async {
  final workspace = claktoryWorkspace();
  final open = workspace.agents.first.id;
  workspace.selectedAgentId = open;
  for (final agent in workspace.agents.skip(1)) {
    agent.status = AgentStatus.AGENT_STATUS_WORKING;
  }
  final harness = Harness(workspace);
  harness.backend.transcript.load(
    Transcript(
      agentId: open,
      entries: [
        for (var i = 0; i < 30; i++)
          Entry(
            seq: Int64(i + 1),
            timestampMs: Int64(1000 + i),
            assistant: i.isOdd
                ? AssistantText(text: 'Reply $i with **markdown**.')
                : null,
            user: i.isEven ? UserText(text: 'Message $i') : null,
          ),
      ],
    ),
  );
  await harness.show(tester, size: const Size(1440, 900));
  await tester.pump(const Duration(seconds: 1));
  return harness;
}

final _agentsField = Workspace().info_.byName['agents']!.tagNumber;

/// One agent's step, as the backend sends it: a patch with that agent
/// changed and everything else kept.
void _step(Harness harness, String id, void Function(Agent agent) change) {
  final prev = harness.backend.workspace.workspace;
  final agent = prev.agents.firstWhere((agent) => agent.id == id).deepCopy();
  change(agent);
  final patch = WorkspacePatch(
    workspace: Workspace(
      agents: [
        for (final other in prev.agents)
          other.id == id ? agent : Agent(id: other.id),
      ],
    ),
    kept: [
      for (final number in Workspace().info_.fieldInfo.keys)
        if (number != _agentsField) number,
    ],
    keptAgents: [
      for (final other in prev.agents)
        if (other.id != id) other.id,
    ],
  );
  harness.backend.workspace.apply(patchWorkspace(prev, patch)!);
}

/// The widgets built, by type, while [during] runs.
Future<Map<String, int>> _built(Future<void> Function() during) async {
  final built = <String, int>{};
  debugOnRebuildDirtyWidget = (element, _) {
    final type = element.widget.runtimeType.toString();
    built[type] = (built[type] ?? 0) + 1;
  };
  try {
    await during();
  } finally {
    debugOnRebuildDirtyWidget = null;
  }
  return built;
}

void main() {
  testWidgets("another agent's step builds its own row and little else", (
    tester,
  ) async {
    final harness = await _busy(tester);
    final other = harness.backend.workspace.workspace.agents[5];
    final built = await _built(() async {
      _step(harness, other.id, (agent) {
        agent.contextTokens = Int64(4200);
        agent.characterMood = CharacterMood.CHARACTER_MOOD_THINKING;
      });
      await tester.pump();
    });
    expect(built['_AgentRow'], 1, reason: 'only its row: $built');
    for (final untouched in [
      'EntryView',
      'TranscriptView',
      'MessageBox',
      '_Header',
      '_ProjectRow',
      '_FooterButtons',
    ]) {
      expect(built[untouched], isNull, reason: '$untouched built: $built');
    }
    // Before, a step built ~1300 widgets here; now ~250.
    final total = built.values.fold(0, (sum, count) => sum + count);
    expect(total, lessThan(500), reason: '$built');
    // And its row shows what changed.
    _step(harness, other.id, (agent) => agent.role = 'Night shift');
    await tester.pump();
    expect(find.text('Night shift'), findsOneWidget);
  });

  testWidgets("the open agent's step shows in its header", (tester) async {
    final harness = await _busy(tester);
    final open = harness.backend.workspace.workspace.selectedAgentId;
    _step(harness, open, (agent) {
      agent.role = 'Keeps the lights on';
      agent.contextTokens = Int64(42000);
    });
    await tester.pump();
    final header = find.byType(AgentView);
    expect(
      find.descendant(of: header, matching: find.text('Keeps the lights on')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: header, matching: find.textContaining('42k')),
      findsOneWidget,
    );
  });

  testWidgets('the person\'s avatar follows the person', (tester) async {
    final harness = await _busy(tester);
    expect(find.text('MZ'), findsNothing);
    final next = harness.backend.workspace.workspace.deepCopy()
      ..person = Person(name: 'Matt', initials: 'MZ');
    harness.backend.workspace.apply(next);
    await tester.pump();
    expect(find.text('MZ'), findsWidgets);
  });

  group('Kept', () {
    testWidgets('keeps its child while its inputs are the same', (
      tester,
    ) async {
      var builds = 0;
      Widget kept(List<Object?> inputs, {TextDirection? direction}) =>
          Directionality(
            textDirection: direction ?? TextDirection.ltr,
            child: Kept(
              inputs: inputs,
              builder: (context) {
                builds++;
                return Text('${inputs.first} ${Directionality.of(context)}');
              },
            ),
          );
      final agent = Agent(id: 'a');
      await tester.pumpWidget(
        kept([
          agent,
          1,
          ['x', 'y'],
        ]),
      );
      expect(builds, 1);
      // The same objects, an equal number, a list with the same items.
      await tester.pumpWidget(
        kept([
          agent,
          1,
          ['x', 'y'],
        ]),
      );
      expect(builds, 1);
      // An equal agent that isn't the same object is the same input.
      await tester.pumpWidget(
        kept([
          agent.deepCopy(),
          1,
          ['x', 'y'],
        ]),
      );
      expect(builds, 1);
      await tester.pumpWidget(
        kept([
          agent,
          2,
          ['x', 'y'],
        ]),
      );
      expect(builds, 2);
      await tester.pumpWidget(
        kept([
          agent,
          2,
          ['x', 'z'],
        ]),
      );
      expect(builds, 3);
      await tester.pumpWidget(
        kept([
          Agent(id: 'b'),
          2,
          ['x', 'z'],
        ]),
      );
      expect(builds, 4);
      // What it reads through its context builds it again.
      await tester.pumpWidget(
        kept([
          Agent(id: 'b'),
          2,
          ['x', 'z'],
        ], direction: TextDirection.rtl),
      );
      expect(builds, 5);
      expect(find.textContaining('rtl'), findsOneWidget);
    });

    test('sameInputs compares lists item by item', () {
      expect(sameInputs([1, 'a'], [1, 'a']), isTrue);
      expect(sameInputs([1], [1, 'a']), isFalse);
      expect(
        sameInputs(
          [
            [1, 2],
          ],
          [
            [1, 2],
          ],
        ),
        isTrue,
      );
      expect(
        sameInputs(
          [
            [1, 2],
          ],
          [
            [2, 1],
          ],
        ),
        isFalse,
      );
      expect(sameInputs([null], [null]), isTrue);
      expect(sameInputs([null], [0]), isFalse);
    });
  });
}
