import 'package:fixnum/fixnum.dart';
import 'package:factory_app/agent_view.dart' show WorkingLine;
import 'package:factory_app/controls.dart';
import 'package:factory_app/mobile/drawer.dart' show largeText;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:factory_app/usage_wait.dart';
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, showApp, workspace;
import 'focus_test.dart' show focusedOn;
import 'harness.dart';
import 'ipad/window_fixtures.dart' show TextFactor;
import 'messaging_test.dart' as messaging;
import 'phone_sizes_test.dart' show loadGeist;

/// As the backend says it, in its own time zone.
const _said = 'Waiting for your Claude usage to reset at Fri 18:57';

/// As the app shows it: at this device's time for the same moment.
final _wait = usageWaitLine(_held());

/// Builder, its run held up by usage limits: still working, asleep.
Agent _held() => builder()
  ..usageWait = _said
  ..usageWaitUntilMs = Int64(1795000000000)
  ..characterMood = CharacterMood.CHARACTER_MOOD_ASLEEP;

/// [text] in the line above the message box.
Finder _inLine(String text) =>
    find.descendant(of: find.byType(WorkingLine), matching: find.text(text));

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

  testWidgets('Try now beside the wait asks again at once; a second tap '
      'after the wait ended says nothing', (tester) async {
    final harness = await showApp(tester, workspace(agents: [_held()]));
    await settle(tester);
    expect(_inLine('Try now'), findsOneWidget);
    expect(_inLine('Sign in…'), findsNothing);
    await tester.tap(_inLine('Try now'));
    await tester.pump();
    expect(harness.last.tryAgainNow.agentId, 'a1');

    // Refused (it isn't waiting anymore): nothing shows, the line is gone.
    harness.backend.refuse = "Builder isn't waiting anymore.";
    await tester.tap(_inLine('Try now'));
    await tester.pump();
    expect(tester.takeException(), isNull);
    expect(find.text("Builder isn't waiting anymore."), findsNothing);
  });

  testWidgets('waiting for a sign-in, Sign in… opens Settings at Providers '
      'instead of Try now', (tester) async {
    final harness = await showApp(
      tester,
      workspace(
        agents: [
          builder()
            ..usageWait = 'Waiting for you to sign in to Claude'
            ..characterMood = CharacterMood.CHARACTER_MOOD_ASLEEP,
        ],
      ),
    );
    await settle(tester);
    expect(_inLine('Try now'), findsNothing);
    await tester.tap(_inLine('Sign in…'));
    await settle(tester);
    expect(harness.backend.settingsSection.value, 'providers');
    expect(
      harness.backend.sent.where((message) => message.hasTryAgainNow()),
      isEmpty,
    );
  });

  testWidgets("Try now in the agent's row, under the pointer", (tester) async {
    final harness = await showApp(tester, workspace(agents: [_held()]));
    await settle(tester);
    expect(_inRow('Try now'), findsNothing);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: tester.getCenter(_inRow(_wait)));
    addTearDown(mouse.removePointer);
    await tester.pump();
    expect(_inRow('Try now'), findsOneWidget);
    await tester.tap(_inRow('Try now'));
    await tester.pump();
    expect(harness.last.tryAgainNow.agentId, 'a1');
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  for (final width in [320.0, 375.0]) {
    testWidgets('on a $width-point phone at the largest text, Try now and '
        'Stop fit under the wait', (tester) async {
      await tester.runAsync(loadGeist);
      tester.platformDispatcher.textScaleFactorTestValue = TextFactor.ax5;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = Harness(workspace(agents: [_held()]));
      await harness.show(tester, size: Size(width, 667));
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      expect(largeText(tester.element(find.byType(WorkingLine))), isTrue);
      final line = tester.getRect(find.byType(WorkingLine));
      for (final label in ['Try now', 'Stop']) {
        final button = tester.getRect(_inLine(label));
        expect(button.left, greaterThanOrEqualTo(line.left), reason: label);
        expect(button.right, lessThanOrEqualTo(line.right), reason: label);
        // Under the words, not squeezed beside them.
        expect(
          button.top,
          greaterThan(tester.getRect(_inLine(_wait)).top),
          reason: label,
        );
      }
      await tester.tap(_inLine('Try now'));
      await tester.pump();
      expect(harness.last.tryAgainNow.agentId, 'a1');
    });
  }

  testWidgets('on an iPhone SE at standard text, Try now and Stop sit side '
      'by side in one row', (tester) async {
    await tester.runAsync(loadGeist);
    final harness = Harness(workspace(agents: [_held()]));
    await harness.show(tester, size: const Size(375, 667));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    final again = tester.getRect(_inLine('Try now'));
    final stop = tester.getRect(_inLine('Stop'));
    expect(again.center.dy, closeTo(stop.center.dy, 0.5));
    expect(again.right, lessThan(stop.left));
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

  group('carrying on with another provider (#341)', () {
    UsageWaitAlternative alternative({
      bool same = true,
      bool effortChanged = false,
      int others = 0,
    }) => UsageWaitAlternative(
      providerName: 'OpenRouter',
      model: same
          ? 'openrouter/anthropic/claude-opus-4.8'
          : 'openrouter/anthropic/claude-sonnet-4.6',
      modelName: same ? 'Claude Opus 4.8' : 'Claude Sonnet 4.6',
      sameModel: same,
      effort: effortChanged ? 'medium' : 'high',
      effortChanged: effortChanged,
      othersWaiting: others,
    );

    testWidgets('Continue on OpenRouter beside Try now moves it at once; '
        'the same model says nothing more', (tester) async {
      final harness = await showApp(
        tester,
        workspace(agents: [_held()..usageWaitAlternative = alternative()]),
      );
      await settle(tester);
      expect(_inLine('Try now'), findsOneWidget);
      expect(
        tester.getRect(_inLine('Continue on OpenRouter')).right,
        lessThan(tester.getRect(_inLine('Try now')).left),
      );
      expect(find.textContaining('On OpenRouter'), findsNothing);
      await tester.tap(_inLine('Continue on OpenRouter'));
      await tester.pump();
      final asked = harness.last.continueOnAlternative;
      expect(asked.agentId, 'a1');
      expect(asked.allWaiting, isFalse);
      expect(asked.expectedOthers, 0);
    });

    testWidgets('another model or effort is said under the wait; with '
        'others waiting, it asks whether all go', (tester) async {
      final harness = await showApp(
        tester,
        workspace(
          agents: [
            _held()
              ..usageWaitAlternative = alternative(
                same: false,
                effortChanged: true,
                others: 2,
              ),
          ],
        ),
      );
      await settle(tester);
      expect(
        _inLine(
          'On OpenRouter, it uses Claude Sonnet 4.6 instead, at medium effort.',
        ),
        findsOneWidget,
      );
      await tester.tap(_inLine('Continue on OpenRouter'));
      await settle(tester);
      expect(find.text('Only Builder'), findsOneWidget);
      expect(
        harness.backend.sent.where((m) => m.hasContinueOnAlternative()),
        isEmpty,
        reason: 'nothing until chosen',
      );
      await tester.tap(find.text('All 3'));
      await settle(tester);
      expect(harness.last.continueOnAlternative.allWaiting, isTrue);
      // The count shown goes with it: more waiting by now moves none.
      expect(harness.last.continueOnAlternative.expectedOthers, 2);
    });

    testWidgets('the same model at another effort says only the effort', (
      tester,
    ) async {
      await showApp(
        tester,
        workspace(
          agents: [
            _held()..usageWaitAlternative = alternative(effortChanged: true),
          ],
        ),
      );
      await settle(tester);
      expect(
        _inLine('On OpenRouter, it runs at medium effort.'),
        findsOneWidget,
      );
    });

    testWidgets('a refusal is said in the line, as the backend words it', (
      tester,
    ) async {
      final harness = await showApp(
        tester,
        workspace(agents: [_held()..usageWaitAlternative = alternative()]),
      );
      await settle(tester);
      harness.backend.refuse = "OpenRouter can't run that model right now.";
      await tester.tap(_inLine('Continue on OpenRouter'));
      await settle(tester);
      expect(
        _inLine("OpenRouter can't run that model right now."),
        findsOneWidget,
      );
    });

    testWidgets('with no other provider signed in, it says one would let it '
        'carry on, and Sign in… opens Providers', (tester) async {
      final harness = await showApp(
        tester,
        workspace(agents: [_held()..usageWaitNoOtherProvider = true]),
      );
      await settle(tester);
      expect(
        _inLine(
          'Sign in to another provider, such as OpenRouter, to carry on now.',
        ),
        findsOneWidget,
      );
      expect(_inLine('Try now'), findsOneWidget);
      expect(_inLine('Continue on OpenRouter'), findsNothing);
      await tester.tap(_inLine('Sign in…'));
      await settle(tester);
      expect(harness.backend.settingsSection.value, 'providers');
    });

    testWidgets('Back to Claude once Claude has room again, not before', (
      tester,
    ) async {
      UsageSwitch switched({bool ready = true, int others = 0}) => UsageSwitch(
        providerName: 'Claude',
        model: 'anthropic/claude-opus-4-8',
        modelName: 'Claude Opus 4.8',
        ready: ready,
        othersReady: others,
      );
      final harness = await showApp(
        tester,
        workspace(agents: [builder()..usageSwitch = switched(ready: false)]),
      );
      await settle(tester);
      expect(find.text('Back to Claude'), findsNothing);

      await harness.apply(
        tester,
        workspace(agents: [builder()..usageSwitch = switched()]),
      );
      await settle(tester);
      expect(find.text('Your Claude usage has room again.'), findsOneWidget);
      await tester.tap(find.text('Back to Claude'));
      await tester.pump();
      expect(harness.last.switchBack.agentId, 'a1');
      expect(harness.last.switchBack.all, isFalse);

      await harness.apply(
        tester,
        workspace(agents: [builder()..usageSwitch = switched(others: 1)]),
      );
      await settle(tester);
      await tester.tap(find.text('Back to Claude'));
      await settle(tester);
      await tester.tap(find.text('Both'));
      await settle(tester);
      expect(harness.last.switchBack.all, isTrue);
      expect(harness.last.switchBack.expectedOthers, 1);
    });

    for (final width in [320.0, 375.0]) {
      testWidgets('on a $width-point phone at the largest text, every '
          'control fits under the wait', (tester) async {
        await tester.runAsync(loadGeist);
        tester.platformDispatcher.textScaleFactorTestValue = TextFactor.ax5;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        final harness = Harness(
          workspace(
            agents: [
              _held()
                ..usageWaitAlternative = alternative(
                  same: false,
                  effortChanged: true,
                  others: 2,
                ),
            ],
          ),
        );
        await harness.show(tester, size: Size(width, 667));
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
        final line = tester.getRect(find.byType(WorkingLine));
        for (final label in ['Continue on OpenRouter', 'Try now', 'Stop']) {
          final button = tester.getRect(_inLine(label));
          expect(button.left, greaterThanOrEqualTo(line.left), reason: label);
          expect(button.right, lessThanOrEqualTo(line.right), reason: label);
        }
      });
    }
  });
}
