// A move of this Claktory an agent started (#243 S1) waits for the
// person's own Confirm: besides floating over the window, it's in Needs
// you, opening to the same card.
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({String startedBy = 'rivet', bool here = false}) =>
    Workspace(
      projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
      agents: [
        Agent(
          id: 'rivet',
          name: 'Rivet',
          modelAvailable: true,
          status: AgentStatus.AGENT_STATUS_IDLE,
        ),
      ],
      move: Move(
        phase: MovePhase.MOVE_PHASE_CONFIRM,
        targetMachine: 'cloud-box',
        targetOs: 'linux',
        targetArch: 'x86_64',
        digits: '123 456',
        canCancel: true,
        startedByAgentId: startedBy,
        confirmedHere: here,
      ),
    );

Finder _inNeedsYou(Finder finder) =>
    find.descendant(of: find.byType(InboxView), matching: finder);

void main() {
  test('only an agent\'s move, until confirmed here', () {
    expect(needsYouCount(_workspace()), 1);
    expect(needsYouCount(_workspace(here: true)), 0);
    // The person's own move asks nothing of them here.
    expect(needsYouCount(_workspace(startedBy: '')), 0);
    expect(needsYouCount(Workspace()), 0);
  });

  testWidgets('it opens to the confirm card, which confirms or cancels '
      'there', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);

    final line = _inNeedsYou(find.text('Move this Claktory to cloud-box?'));
    expect(line, findsOneWidget);
    await tester.tap(line);
    await pumpFrames(tester);
    expect(
      _inNeedsYou(
        find.text('Move this Claktory to the computer named cloud-box?'),
      ),
      findsOneWidget,
    );
    expect(_inNeedsYou(find.text('Rivet started this move.')), findsOneWidget);
    expect(_inNeedsYou(find.text('Open conversation')), findsNothing);

    await tester.tap(_inNeedsYou(find.text('Confirm')));
    await settle(tester);
    expect(harness.last.hasConfirmMove(), isTrue);
  });
}
