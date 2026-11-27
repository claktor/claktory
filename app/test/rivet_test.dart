import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/forms.dart';
import 'package:factory_app/sidebar.dart' show rivetPausedLine;
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent admin({String id = 'rivet', bool available = true}) => Agent(
  id: id,
  name: 'Rivet',
  modelAvailable: available,
  extensions: [
    AgentExtension(
      id: 'agents',
      enabled: true,
      options: {'admin': OptionValue(on: true)}.entries,
    ),
  ],
);

Workspace snapshot({String selected = '', List<Agent>? agents}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  agents:
      agents ??
      [
        admin(),
        Agent(id: 'a1', projectId: 'p1', name: 'Builder', modelAvailable: true),
      ],
  selectedAgentId: selected,
);

/// The backend selects what the app asks it to, as the real one does.
Harness selecting(Workspace start) {
  final harness = Harness(start);
  harness.backend.onSend = (message) {
    if (message.hasSelectAgent()) {
      harness.backend.workspace.apply(
        harness.backend.workspace.workspace.deepCopy()
          ..selectedAgentId = message.selectAgent.agentId,
      );
    }
  };
  return harness;
}

EditableText box(WidgetTester tester) =>
    tester.widget<EditableText>(find.byType(EditableText));

void main() {
  testWidgets("the Projects \"+\" starts a message to Rivet", (tester) async {
    final harness = selecting(snapshot(selected: 'a1'));
    await harness.show(tester);
    await tester.tap(askRivetButtons().first);
    await settle(tester);
    expect(harness.last.selectAgent.agentId, 'rivet');
    expect(box(tester).controller.text, 'Set up a new project for ');
    expect(
      box(tester).controller.selection,
      const TextSelection.collapsed(offset: 25),
      reason: 'the cursor at the end',
    );
    expect(box(tester).focusNode.hasFocus, isTrue);
  });

  testWidgets("a project's \"+\" asks for an agent in it", (tester) async {
    final harness = selecting(snapshot());
    await harness.show(tester);
    expect(
      askRivetButtons(),
      findsOneWidget,
      reason: "the project's shows on hover",
    );
    await tapProjectPlus(tester, 'Factory');
    expect(harness.last.selectAgent.agentId, 'rivet');
    expect(box(tester).controller.text, 'Add an agent to Factory that ');
  });

  testWidgets("Rivet's own draft is kept, the request after it", (
    tester,
  ) async {
    final harness = selecting(snapshot(selected: 'rivet'));
    await harness.show(tester);
    await tester.enterText(find.byType(EditableText), 'Also, rename Site');
    await tester.pump();
    // Rivet is open already: its box takes the request in place.
    await tester.tap(askRivetButtons().first);
    await settle(tester);
    expect(
      box(tester).controller.text,
      'Also, rename Site\nSet up a new project for ',
    );
    expect(box(tester).focusNode.hasFocus, isTrue);
    expect(box(tester).controller.selection.baseOffset, 43);
  });

  testWidgets('the "+" says what it does on hover', (tester) async {
    final harness = selecting(snapshot());
    await harness.show(tester);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: tester.getCenter(askRivetButtons().first));
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('New project'), findsOneWidget);
    await mouse.removePointer();
    await tester.pump();
  });

  test('Rivet is the workspace admin, or else the agent "rivet"', () {
    Workspace with_(List<Agent> agents) => Workspace(agents: agents);
    final helper = admin(id: 'helper');
    expect(rivetIn(with_([Agent(id: 'rivet'), helper]))?.id, 'helper');
    // An admin in a project isn't the workspace's.
    expect(
      rivetIn(with_([Agent(id: 'rivet'), helper..projectId = 'p1']))?.id,
      'rivet',
    );
    expect(rivetIn(with_([Agent(id: 'a1')])), isNull);
  });

  group('Rivet paused on a usage limit', () {
    Agent paused({UsageWaitAlternative? alternative}) {
      final rivet = admin()
        ..usageWait = 'Waiting for your Claude usage to reset'
        ..status = AgentStatus.AGENT_STATUS_WORKING;
      if (alternative != null) rivet.usageWaitAlternative = alternative;
      return rivet;
    }

    testWidgets('a "+" opens the form, saying why at its top', (tester) async {
      final harness = selecting(
        snapshot(
          selected: 'a1',
          agents: [
            paused(),
            Agent(id: 'a1', projectId: 'p1', name: 'Builder'),
          ],
        ),
      );
      await harness.show(tester);
      await tester.tap(askRivetButtons().first);
      await settle(tester);
      expect(find.byType(CreateProjectForm), findsOneWidget);
      expect(
        find.text(
          'Rivet is paused until your Claude limit resets. Set it up here '
          'instead.',
        ),
        findsOneWidget,
      );
      expect(harness.backend.sent.where((m) => m.hasSelectAgent()), isEmpty);
    });

    testWidgets('where its wait offers another provider, so does the form, '
        'and carrying on there asks Rivet after all', (tester) async {
      final harness = selecting(
        snapshot(
          selected: 'a1',
          agents: [
            paused(
              alternative: UsageWaitAlternative(
                providerName: 'OpenRouter',
                model: 'openrouter/claude',
                sameModel: true,
              ),
            ),
            Agent(id: 'a1', projectId: 'p1', name: 'Builder'),
          ],
        ),
      );
      await harness.show(tester);
      await tester.tap(askRivetButtons().first);
      await settle(tester);
      await tester.tap(find.text('Continue on OpenRouter'));
      await settle(tester);
      expect(
        harness.backend.sent
            .where((m) => m.hasContinueOnAlternative())
            .single
            .continueOnAlternative
            .agentId,
        'rivet',
      );
      expect(find.byType(CreateProjectForm), findsNothing);
      expect(harness.last.selectAgent.agentId, 'rivet');
      expect(box(tester).controller.text, 'Set up a new project for ');
    });

    test('its line names the provider, or says the wait as it is', () {
      expect(
        rivetPausedLine(
          'Rivet',
          'Waiting for your OpenAI spending limit to reset at 18:00',
        ),
        'Rivet is paused until your OpenAI limit resets. Set it up here '
        'instead.',
      );
      expect(
        rivetPausedLine('Rivet', 'Waiting for you to sign in to Claude'),
        'Rivet is paused: waiting for you to sign in to Claude. Set it up '
        'here instead.',
      );
    });
  });
}
