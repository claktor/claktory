// A pipe's liaison is bound to it: no row of its own in the sidebar
// or the drawer; its pipe's row carries its face, its work and what it
// waits on; a search for its name finds the pipe; on a phone, Needs you
// lists the pipe, not the agent. The phone's box chooses whom it writes to
// with one chip.
import 'package:factory_app/characters/character.dart';
import 'package:factory_app/mobile/pipe_target.dart';
import 'package:factory_app/pipe_timeline.dart' show PipeTarget;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({bool waiting = false}) => Workspace(
  projects: [Project(id: 'p1', name: 'Claktory', path: '/w')],
  agents: [
    Agent(id: 'mike', name: 'Mike', projectId: 'p1', modelAvailable: true),
    Agent(
      id: 'kai',
      name: 'Kai',
      role: 'Liaison',
      projectId: 'p1',
      pipeId: 'pipe1',
      modelAvailable: true,
      waitingSeqs: [if (waiting) Int64(7)],
    ),
  ],
  pipes: [
    Pipe(
      id: 'pipe1',
      name: 'Acme launch',
      projectId: 'p1',
      state: PipeState.PIPE_STATE_OPEN,
      online: true,
      peer: PipePeer(claktoryName: "Ana's Claktory"),
    ),
  ],
  selectedAgentId: 'mike',
);

void main() {
  testWidgets('the liaison has no row; its face is on its pipe\'s row', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    final harness = Harness(_workspace(waiting: true));
    await harness.show(tester);
    await settle(tester);
    expect(find.text('Kai'), findsNothing);
    final row = find.byKey(const ValueKey('pipe-row/pipe1'));
    expect(row, findsOneWidget);
    expect(
      find.descendant(of: row, matching: find.byType(AgentCharacter)),
      findsOneWidget,
    );
    // Its waiting counts on the pipe's row.
    expect(
      find.bySemanticsLabel(
        RegExp('Acme launch has something waiting for your answer'),
      ),
      findsWidgets,
    );
    expect(find.bySemanticsLabel(RegExp('Pipe, liaison Kai')), findsWidgets);
    semantics.dispose();
  });

  testWidgets("in the phone's drawer: search finds the pipe by its "
      "liaison's name; Needs you lists the pipe", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace(waiting: true));
    await harness.show(tester, size: const Size(390, 844));
    await settle(tester);
    await tester.tap(find.bySemanticsLabel('Menu'));
    await pumpFrames(tester);
    // Needs you: the pipe, never Kai on a row of its own.
    expect(find.text('Kai'), findsNothing);
    expect(find.text('Acme launch'), findsWidgets);

    await tester.enterText(find.byType(EditableText).first, 'kai');
    await pumpFrames(tester);
    expect(find.text('Acme launch'), findsWidgets);
    expect(find.text('Kai'), findsNothing);
    expect(find.textContaining('Nothing matches'), findsNothing);
  });

  testWidgets("the phone's chip: whom the box writes to, a finger's size", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace());
    PipeTarget? chosen;
    await harness.show(tester, size: const Size(390, 844));
    await settle(tester);
    final workspace = harness.backend.workspace.workspace;
    // Shown in the app's own overlay and scope, as the pipe view does.
    final context = tester.element(find.bySemanticsLabel('Menu'));
    final entry = OverlayEntry(
      builder: (_) => Center(
        child: PhonePipeTarget(
          pipe: workspace.pipes.single,
          liaison: workspace.agents.last,
          target: PipeTarget.liaison,
          onChanged: (target) => chosen = target,
        ),
      ),
    );
    Overlay.of(context).insert(entry);
    addTearDown(entry.remove);
    await pumpFrames(tester);
    expect(find.text('To Kai'), findsOneWidget);
    expect(
      tester.getSize(find.byType(PhonePipeTarget)).height,
      greaterThanOrEqualTo(44),
    );
    await tester.tap(find.text('To Kai'));
    await pumpFrames(tester);
    await tester.tap(find.text("To Ana's Claktory").last);
    await pumpFrames(tester);
    expect(chosen, PipeTarget.pipe);
  });
}
