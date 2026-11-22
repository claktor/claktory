// The team's activity in the sidebar's footer (#254): how many agents work
// out of all of them, across projects, live; who on hover; the Map on a
// click.
import 'package:factory_app/controls.dart' show LiveDot;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/team_activity.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';

Agent _agent(
  String id, {
  String project = 'p1',
  bool working = false,
  String usageWait = '',
}) => Agent(
  id: id,
  name: 'Agent $id',
  projectId: project,
  color: 'blue',
  modelAvailable: true,
  usageWait: usageWait,
  status: working
      ? AgentStatus.AGENT_STATUS_WORKING
      : AgentStatus.AGENT_STATUS_IDLE,
);

Workspace _team(List<Agent> agents) {
  final team = workspace(agents: agents, selected: '');
  team.projects.add(Project(id: 'p3', name: 'Old', archived: true));
  return team;
}

Future<Harness> _show(WidgetTester tester, Workspace team) async {
  final harness = Harness(team);
  await harness.show(
    tester,
    home: Center(child: TeamActivity(workspace: team)),
  );
  return harness;
}

void main() {
  test('it counts the agents at work across projects, not archived ones '
      'or ones waiting for usage', () {
    final (:working, :total) = teamActivity(
      _team([
        _agent('a', working: true),
        _agent('b', project: 'p2', working: true),
        _agent('c', project: '', working: true),
        _agent('d'),
        _agent('e', working: true, usageWait: 'Waiting for your usage'),
        _agent('f', project: 'p3', working: true),
      ]),
    );
    expect([for (final agent in working) agent.id], ['a', 'b', 'c']);
    expect(total, 5);
  });

  testWidgets('it shows how many work out of all, live while any do', (
    tester,
  ) async {
    await _show(
      tester,
      _team([_agent('a', working: true), _agent('b'), _agent('c')]),
    );
    expect(find.text('1/3'), findsOneWidget);
    expect(find.byType(LiveDot), findsOneWidget);
  });

  testWidgets('with no one at work, no live dot', (tester) async {
    await _show(tester, _team([_agent('a'), _agent('b')]));
    expect(find.text('0/2'), findsOneWidget);
    expect(find.byType(LiveDot), findsNothing);
  });

  testWidgets('hovering it names who works, the first eight', (tester) async {
    await _show(
      tester,
      _team([for (var i = 0; i < 11; i++) _agent('$i', working: true)]),
    );
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    addTearDown(mouse.removePointer);
    await mouse.moveTo(tester.getCenter(find.text('11/11')));
    await pumpFrames(tester, const Duration(milliseconds: 1200));
    expect(find.text('11 of 11 agents working'), findsOneWidget);
    expect(find.text('Agent 0'), findsOneWidget);
    expect(find.text('Agent 7'), findsOneWidget);
    expect(find.text('Agent 8'), findsNothing);
    expect(find.text('and 3 more'), findsOneWidget);
  });

  testWidgets('a click opens the Map', (tester) async {
    final harness = await _show(
      tester,
      _team([_agent('a', working: true), _agent('b')]),
    );
    await tester.tap(find.text('1/2'));
    await tester.pump();
    expect(harness.page.value, AppPage.map);
    expect(
      tester.getSemantics(find.byType(TeamActivity)).label,
      contains('1 of 2 agents working'),
    );
  });
}
