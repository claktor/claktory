import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

AgentExtension messagingOn({bool reachable = true}) => AgentExtension(
  id: 'agents',
  enabled: true,
  options: {'reachable': OptionValue(on: reachable)}.entries,
);

Workspace workspace({bool reachable = true, bool testerWorking = false}) =>
    Workspace(
      projects: [
        Project(id: 'p1', name: 'Factory', path: '/x'),
        Project(id: 'p2', name: 'Site', path: '/y'),
      ],
      agents: [
        Agent(id: 'rivet', name: 'Rivet', modelAvailable: true),
        Agent(
          id: 'a1',
          name: 'Builder',
          projectId: 'p1',
          modelAvailable: true,
          extensions: [messagingOn(reachable: reachable)],
          openPanel: 'agents',
        ),
        Agent(
          id: 'a2',
          name: 'Tester',
          projectId: 'p1',
          modelAvailable: true,
          status: testerWorking
              ? AgentStatus.AGENT_STATUS_WORKING
              : AgentStatus.AGENT_STATUS_IDLE,
        ),
        Agent(id: 'a3', name: 'Writer', projectId: 'p2'),
      ],
      extensions: [Extension(id: 'agents', name: 'Messaging', icon: 'users')],
      selectedAgentId: 'a1',
      openPanel: 'agents',
    );

ExtensionView view(List<String> reaches, List<String> reachedBy) =>
    ExtensionView(
      extensionId: 'agents',
      agentId: 'a1',
      title: 'Messaging',
      messaging: Messaging(reaches: reaches, reachedBy: reachedBy),
    );

void main() {
  Future<Harness> show(
    WidgetTester tester,
    Workspace snapshot,
    ExtensionView extensionView,
  ) async {
    final harness = Harness(snapshot);
    harness.backend.extensionView.apply(extensionView);
    await harness.show(tester);
    await settle(tester);
    return harness;
  }

  Finder inPanel(String text) => find.descendant(
    of: find.byWidgetPredicate(
      (w) => w.runtimeType.toString() == 'MessagingView',
    ),
    matching: find.text(text),
  );

  testWidgets('lists who it can message and who can message it', (
    tester,
  ) async {
    final harness = await show(
      tester,
      workspace(),
      view(['a2', 'a3'], ['rivet', 'a2']),
    );
    expect(inPanel('Can message'), findsOneWidget);
    expect(inPanel('Can be messaged by'), findsOneWidget);
    expect(inPanel('Tester'), findsNWidgets(2));
    expect(inPanel('Writer'), findsOneWidget);
    expect(inPanel('Site'), findsOneWidget, reason: 'another project');
    expect(inPanel('Factory'), findsNothing, reason: 'its own project');
    expect(inPanel('Workspace'), findsOneWidget, reason: "Rivet's place");
    // Every row's status lines up at the panel's edge, whatever the name.
    final rights = {
      for (final element in inPanel('idle').evaluate())
        tester.getTopRight(find.byWidget(element.widget)).dx,
      tester.getTopRight(inPanel('no model')).dx,
    };
    expect(rights, hasLength(1));

    // An admin agent is the Admin's, not the workspace's.
    final admin = workspace();
    admin.agents[0].extensions.add(
      AgentExtension(
        id: 'agents',
        enabled: true,
        options: {'admin': OptionValue(on: true)}.entries,
      ),
    );
    await harness.apply(tester, admin);
    expect(inPanel('Admin'), findsOneWidget);
    expect(inPanel('Workspace'), findsNothing);
    await harness.apply(tester, workspace());
    expect(inPanel('no model'), findsOneWidget, reason: 'Writer');
    expect(inPanel('idle'), findsNWidgets(3));

    // Status is live from the workspace.
    await harness.apply(tester, workspace(testerWorking: true));
    expect(inPanel('working'), findsNWidgets(2));
    // Each agent is its living character, thinking while it works.
    expect(
      find.descendant(
        of: find.byWidgetPredicate(
          (w) => w.runtimeType.toString() == 'MessagingView',
        ),
        matching: find.byWidgetPredicate(
          (w) => w is Semantics && w.properties.label == 'Tester — thinking',
        ),
      ),
      findsNWidgets(2),
    );
  });

  testWidgets('each list says when it is empty, and why', (tester) async {
    await show(tester, workspace(reachable: false), view([], []));
    expect(inPanel("It can't message any agent."), findsOneWidget);
    expect(
      find.textContaining('"Other agents can message it" is off'),
      findsOneWidget,
    );
  });
}
