import 'package:fixnum/fixnum.dart';
import 'package:factory_app/agent_view.dart' show WorkingLine;
import 'package:factory_app/controls.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:factory_app/usage_wait.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, showApp, workspace;
import 'focus_test.dart' show focusedOn;
import 'harness.dart';
import 'messaging_test.dart' as messaging;

/// As the backend says it, in its own time zone.
const _said = 'Waiting for your Claude usage to reset at Fri 18:57';

/// As the app shows it: at this device's time for the same moment.
final _wait = usageWaitLine(_held());

/// Builder, its run held up by usage limits: still working, asleep.
Agent _held() => builder()
  ..usageWait = _said
  ..usageWaitUntilMs = Int64(1795000000000)
  ..characterMood = CharacterMood.CHARACTER_MOOD_ASLEEP;

/// [text] in the sidebar.
Finder _inRow(String text) =>
    find.descendant(of: find.byType(Sidebar), matching: find.text(text));

void main() {
  group('the wait in words', () {
    // Friday 20 November 2026, 10:00 here.
    final now = DateTime(2026, 11, 20, 10);
    Agent held(DateTime? until, [String text = _said]) => Agent(
      status: AgentStatus.AGENT_STATUS_WORKING,
      usageWait: text,
      usageWaitUntilMs: Int64(until?.millisecondsSinceEpoch ?? 0),
    );

    test("says the time in this device's own time zone", () {
      expect(
        usageWaitLine(held(DateTime(2026, 11, 20, 18, 57)), now: now),
        'Waiting for your Claude usage to reset at 18:57',
      );
      expect(
        usageWaitLine(held(DateTime(2026, 11, 22, 9, 5)), now: now),
        'Waiting for your Claude usage to reset at Sun 09:05',
      );
    });

    test("is the backend's own words when there's no time", () {
      const credit = 'Waiting for you to add credit or another Claude account';
      expect(usageWaitLine(held(null, credit), now: now), credit);
      expect(usageWaitLine(held(null), now: now), _said);
      expect(usageWaitLine(Agent(), now: now), '');
    });

    test("an agent waiting isn't at work", () {
      expect(atWork(held(DateTime(2026, 11, 20, 18, 57))), isFalse);
      expect(atWork(Agent(status: AgentStatus.AGENT_STATUS_WORKING)), isTrue);
      expect(atWork(Agent(status: AgentStatus.AGENT_STATUS_IDLE)), isFalse);
    });
  });

  testWidgets('an agent waiting out usage limits says what it waits for, '
      'in its row and above its message box, and doesn\'t wave', (
    tester,
  ) async {
    final harness = await showApp(tester, workspace(agents: [_held()]));
    await settle(tester);
    expect(_inRow(_wait), findsOneWidget);
    expect(find.text(_wait), findsNWidgets(2), reason: 'row and the line');
    expect(find.text('Working'), findsNothing);
    expect(
      find.descendant(
        of: find.byType(WorkingLine),
        matching: find.byType(LiveDot),
      ),
      findsNothing,
    );
    expect(find.text('Stop  esc'), findsOneWidget, reason: 'Stop ends it');
    expect(find.byType(WorkingWave), findsNothing);

    // Over: back to work, the wave and Working.
    await harness.apply(tester, workspace(agents: [builder()]));
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text(_wait), findsNothing);
    expect(find.text('Working'), findsOneWidget);
    expect(find.byType(WorkingWave), findsWidgets);
  });

  testWidgets('with a role, the wait takes its place until it ends', (
    tester,
  ) async {
    final harness = await showApp(
      tester,
      workspace(agents: [_held()..role = 'Backend']),
    );
    await settle(tester);
    expect(_inRow('Backend'), findsNothing);
    expect(_inRow(_wait), findsOneWidget);
    await harness.apply(
      tester,
      workspace(agents: [builder(working: false)..role = 'Backend']),
    );
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('Backend'), findsWidgets);
    expect(find.text(_wait), findsNothing);
  });

  testWidgets('the Messaging panel says it waits, not that it works', (
    tester,
  ) async {
    final snapshot = messaging.workspace(testerWorking: true);
    snapshot.agents[2]
      ..usageWait = _said
      ..usageWaitUntilMs = Int64(1795000000000);
    final harness = Harness(snapshot);
    harness.backend.extensionView.apply(messaging.view(['a2'], []));
    await harness.show(tester);
    await settle(tester);
    Finder inPanel(String text) => find.descendant(
      of: find.byWidgetPredicate(
        (w) => w.runtimeType.toString() == 'MessagingView',
      ),
      matching: find.text(text),
    );
    expect(inPanel('on hold'), findsOneWidget);
    expect(inPanel('working'), findsNothing);
    expect(inPanel(usageWaitLine(snapshot.agents[2])), findsOneWidget);
  });

  testWidgets("an update doesn't count an agent waiting as working", (
    tester,
  ) async {
    final snapshot = focusedOn(
      '',
      version: '2026.11.18-1600',
      latest: '2026.11.18-1700',
      working: true,
    );
    snapshot.agents[0].usageWait = _said;
    final harness = Harness(snapshot)..appVersion = '2026.11.18-1700';
    await harness.show(tester);
    expect(find.textContaining('picks up where'), findsNothing);
    expect(find.text('Update'), findsOneWidget);
  });
}
