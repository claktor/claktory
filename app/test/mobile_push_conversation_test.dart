// A conversation opened from a page on a phone (a calendar entry's Open):
// a page over that page, so Back returns to it, with an extension's
// panel up over it when asked for; one conversation built at a time.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_test.dart' show showPhone, workspace;

PhoneRoutes _routes(WidgetTester tester) => PhoneRoutes.of(
  tester.element(find.byType(AgentView, skipOffstage: false).first),
)!;

const _calendar = 'Calendar page';

Future<void> _pushCalendar(WidgetTester tester) async {
  final routes = _routes(tester);
  routes.push<void>(
    PhoneRoute(
      id: 'calendar',
      builder: (context) => const Center(child: Text(_calendar)),
    ),
  );
  await pumpFrames(tester);
  expect(find.text(_calendar), findsOneWidget);
}

AgentView _shown(WidgetTester tester) =>
    tester.widget<AgentView>(find.byType(AgentView));

void main() {
  testWidgets('opened from a page, the conversation is a page over it: '
      'Back returns to that page, then to the conversation', (tester) async {
    final harness = await showPhone(tester, workspace());
    await _pushCalendar(tester);
    final routes = _routes(tester);

    expect(routes.pushConversation('mike'), isTrue);
    await harness.apply(tester, workspace()..selectedAgentId = 'mike');
    await pumpFrames(tester);
    expect(
      harness.backend.sent.where((m) => m.hasSelectAgent()).last.selectAgent,
      SelectAgent(agentId: 'mike'),
    );
    // One conversation built: its draft and find aren't in two places.
    expect(find.byType(AgentView), findsOneWidget);
    expect(_shown(tester).agent.id, 'mike');
    expect(_shown(tester).onBack, isNotNull);
    expect(find.text(_calendar), findsNothing);

    _shown(tester).onBack!();
    await pumpFrames(tester);
    expect(find.text(_calendar), findsOneWidget);

    routes.pop<void>();
    await pumpFrames(tester);
    expect(find.text(_calendar), findsNothing);
    expect(_shown(tester).agent.id, 'mike');
    expect(routes.canPop, isFalse);
  });

  testWidgets("with a panel, its sheet opens over the conversation's page, "
      'not over the page under it', (tester) async {
    final harness = await showPhone(tester, workspace());
    await _pushCalendar(tester);
    _routes(tester).pushConversation('mike', panel: 'checklist');
    expect(
      harness.backend.sent.where((m) => m.hasOpenPanel()).last.openPanel,
      OpenPanel(agentId: 'mike', extensionId: 'checklist'),
    );
    await harness.apply(
      tester,
      workspace(openPanel: 'checklist')..selectedAgentId = 'mike',
    );
    await pumpFrames(tester);
    expect(find.text('Checklist'), findsWidgets);

    _shown(tester).onBack!();
    await pumpFrames(tester);
    expect(find.text(_calendar), findsOneWidget);
    expect(find.text('Checklist'), findsNothing);
  });

  testWidgets('another agent chosen elsewhere takes the page away', (
    tester,
  ) async {
    final harness = await showPhone(tester, workspace());
    await _pushCalendar(tester);
    _routes(tester).pushConversation('mike');
    await harness.apply(tester, workspace()..selectedAgentId = 'mike');
    await pumpFrames(tester);
    await harness.apply(tester, workspace()..selectedAgentId = 'rivet');
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsNothing);
    expect(find.text(_calendar), findsOneWidget);
  });

  testWidgets('from the conversation itself it only chooses the agent, '
      'with nothing to go back to', (tester) async {
    final harness = await showPhone(tester, workspace());
    final routes = _routes(tester);
    expect(routes.pushConversation('mike'), isTrue);
    await harness.apply(tester, workspace()..selectedAgentId = 'mike');
    await pumpFrames(tester);
    expect(routes.canPop, isFalse);
    expect(_shown(tester).agent.id, 'mike');
    expect(routes.pushConversation('nobody'), isFalse);
  });
}
