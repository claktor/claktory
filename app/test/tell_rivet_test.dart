import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/tell_rivet.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart' show Size;
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;
import 'rivet_test.dart' show admin, box, selecting, snapshot;

const _broken = 'config.json line 3: expected `,` or `}`';

Workspace broken({List<Agent>? agents, String selected = 'a1'}) =>
    snapshot(selected: selected, agents: agents)..configError = _broken;

Future<void> openConfigError(WidgetTester tester) async {
  await tester.tap(find.text('Settings file has an error'));
  await settle(tester);
}

void main() {
  test('the request names where the error showed, then all of it', () {
    expect(
      errorRequest('boom', about: 'The settings file has an error.'),
      'Claktory shows this error. Please fix it:\n\n'
      'The settings file has an error.\nboom',
    );
    expect(
      errorRequest('boom'),
      'Claktory shows this error. Please fix it:\n\nboom',
    );
  });

  testWidgets('a broken settings file can be handed to Rivet, beside Restore', (
    tester,
  ) async {
    final harness = selecting(broken());
    await harness.show(tester);
    await openConfigError(tester);
    expect(find.text('Restore last working settings'), findsOneWidget);
    await tester.tap(find.text('Ask Rivet to fix'));
    await settle(tester);
    expect(harness.last.selectAgent.agentId, 'rivet');
    expect(
      box(tester).controller.text,
      errorRequest(_broken, about: 'The settings file has an error.'),
    );
    expect(box(tester).focusNode.hasFocus, isTrue);
    expect(
      find.text('Restore last working settings'),
      findsNothing,
      reason: 'the dialog closed',
    );
  });

  testWidgets('without a Rivet that can run, only Restore is offered', (
    tester,
  ) async {
    for (final agents in [
      [Agent(id: 'a1', projectId: 'p1', name: 'Builder', modelAvailable: true)],
      [
        admin(available: false),
        Agent(id: 'a1', projectId: 'p1', name: 'Builder', modelAvailable: true),
      ],
    ]) {
      final harness = selecting(broken(agents: agents));
      await harness.show(tester);
      await openConfigError(tester);
      expect(find.text('Restore last working settings'), findsOneWidget);
      expect(find.text('Ask Rivet to fix'), findsNothing);
      await tester.tap(find.text('Close'));
      await settle(tester);
    }
  });

  testWidgets("a failed run in an agent's conversation can be handed over", (
    tester,
  ) async {
    final harness = selecting(snapshot(selected: 'a1'));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            notice: Notice(
              level: NoticeLevel.NOTICE_LEVEL_ERROR,
              text: 'The model refused: 529 overloaded',
            ),
          ),
          Entry(
            seq: Int64(2),
            notice: Notice(text: 'Compacted the conversation.'),
          ),
        ],
      ),
    );
    await harness.show(tester);
    expect(
      find.text('Ask Rivet to fix'),
      findsOneWidget,
      reason: 'errors only',
    );
    await tester.tap(find.text('Ask Rivet to fix'));
    await settle(tester);
    expect(harness.last.selectAgent.agentId, 'rivet');
    expect(
      box(tester).controller.text,
      errorRequest(
        'The model refused: 529 overloaded',
        about: "In Builder's conversation (agent a1):",
      ),
    );
  });

  testWidgets('a refused or undone update can be handed over', (tester) async {
    final harness = selecting(
      snapshot(selected: 'a1')
        ..version = '2026.11.18-1600'
        ..latestRelease = '2026.11.18-1700'
        ..updateError = "Factory 2026.11.18-1700 didn't start.",
    );
    harness.appVersion = '2026.11.18-1700';
    await harness.show(tester);
    await tester.tap(find.text('Ask Rivet to fix'));
    await settle(tester);
    expect(
      box(tester).controller.text,
      errorRequest(
        "Factory 2026.11.18-1700 didn't start.",
        about: 'Updating to Claktory 2026.11.18-1700 was undone.',
      ),
    );
  });

  group('while Rivet waits out a usage limit (U5)', () {
    const said = 'Waiting for your Claude usage to reset at Fri 18:57';
    Agent waiting({bool alternative = true}) {
      final rivet = admin()
        ..status = AgentStatus.AGENT_STATUS_WORKING
        ..usageWait = said
        ..usageWaitNoOtherProvider = !alternative;
      if (alternative) {
        rivet.usageWaitAlternative = UsageWaitAlternative(
          providerName: 'OpenRouter',
          modelName: 'Claude Opus 4.8',
          sameModel: true,
        );
      }
      return rivet;
    }

    List<Agent> agents({bool alternative = true}) => [
      waiting(alternative: alternative),
      Agent(id: 'a1', projectId: 'p1', name: 'Builder', modelAvailable: true),
    ];
    testWidgets("a broken settings file says Rivet's wait, without Ask "
        'Rivet to fix, and without Continue on OpenRouter, which would '
        'change the broken file', (tester) async {
      await tester.runAsync(loadGeist);
      final harness = selecting(broken(agents: agents()));
      await harness.show(tester, size: const Size(1200, 800));
      await openConfigError(tester);
      expect(find.text('Ask Rivet to fix'), findsNothing);
      expect(
        find.text(
          'Rivet is waiting for your Claude usage to reset at Fri 18:57',
        ),
        findsOneWidget,
      );
      expect(find.text('Continue on OpenRouter'), findsNothing);
      expect(find.text('Restore last working settings'), findsOneWidget);
      expect(
        harness.backend.sent.where((m) => m.hasContinueOnAlternative()),
        isEmpty,
      );
    });

    testWidgets('with no other provider, Sign in… instead', (tester) async {
      await selecting(broken(agents: agents(alternative: false))).show(tester);
      await openConfigError(tester);
      expect(find.text('Sign in…'), findsOneWidget);
      expect(find.text('Continue on OpenRouter'), findsNothing);
      expect(find.text('Ask Rivet to fix'), findsNothing);
    });

    testWidgets("an error line's link becomes the wait, with its own link", (
      tester,
    ) async {
      final harness = selecting(snapshot(selected: 'a1', agents: agents()));
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [
            Entry(
              seq: Int64(1),
              notice: Notice(
                level: NoticeLevel.NOTICE_LEVEL_ERROR,
                text: 'The model refused: 529 overloaded',
              ),
            ),
          ],
        ),
      );
      await harness.show(tester);
      expect(find.text('Ask Rivet to fix'), findsNothing);
      expect(
        find.text(
          'Rivet is waiting for your Claude usage to reset at Fri 18:57',
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Continue on OpenRouter'));
      await settle(tester);
      expect(harness.last.continueOnAlternative.agentId, 'rivet');
      expect(harness.last.continueOnAlternative.allWaiting, isFalse);
      harness.backend.refuse = "Rivet isn't waiting anymore.";
      await tester.tap(find.text('Continue on OpenRouter'));
      await settle(tester);
      expect(find.text("Rivet isn't waiting anymore."), findsOneWidget);
    });
  });
}
