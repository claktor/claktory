import 'package:factory_app/controls.dart' show Toggle;
import 'package:factory_app/login.dart' show ProviderCard;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/usage_wait.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

const _said = 'Waiting for your Claude usage to reset at Fri 18:57';

final _openRouter = Provider(
  id: 'openrouter',
  name: 'OpenRouter',
  signedInWith: 'OpenRouter API key',
  canSignOut: true,
);

UsageWaitAlternative _alternative({bool same = true, int others = 0}) =>
    UsageWaitAlternative(
      providerName: 'OpenRouter',
      model: same
          ? 'openrouter/anthropic/claude-opus-4.8'
          : 'openrouter/anthropic/claude-sonnet-4.6',
      modelName: same ? 'Claude Opus 4.8' : 'Claude Sonnet 4.6',
      sameModel: same,
      effort: 'high',
      othersWaiting: others,
    );

/// An agent waiting on Claude's limit that would carry on with OpenRouter.
Agent _held(String id, String name, {bool same = true, int others = 0}) =>
    builder()
      ..id = id
      ..name = name
      ..model = 'anthropic/claude-opus-5-5'
      ..usageWait = _said
      ..usageWaitAlternative = _alternative(same: same, others: others);

/// [text] on OpenRouter's card in Settings → Providers.
Finder _onOpenRouter(String text) => find.descendant(
  of: find.byWidgetPredicate(
    (w) => w is ProviderCard && w.provider.id == 'openrouter',
  ),
  matching: find.text(text),
);

Future<Harness> _showProviders(
  WidgetTester tester, {
  required List<Agent> agents,
  List<Chat> chats = const [],
}) async {
  final snapshot = workspace(agents: agents)
    ..providers.add(_openRouter)
    ..chats.addAll(chats);
  final harness = Harness(snapshot);
  await harness.show(tester, size: const Size(1200, 1100));
  harness.page.value = AppPage.settings;
  harness.backend.settingsSection.value = 'providers';
  await settle(tester);
  return harness;
}

void main() {
  testWidgets('the provider they would carry on with offers it to all who '
      'wait, moved at once with the count the backend checks', (tester) async {
    final harness = await _showProviders(
      tester,
      agents: [
        _held('a1', 'Builder', others: 2),
        _held('a2', 'Kai', others: 2),
      ],
      chats: [
        Chat(
          id: 'c1',
          model: 'anthropic/claude-opus-5-5',
          usageWait: _said,
          usageWaitAlternative: _alternative(others: 2),
        ),
      ],
    );
    expect(
      _onOpenRouter('2 agents and a chat are waiting for Claude.'),
      findsOneWidget,
    );
    expect(
      _onOpenRouter('OpenRouter charges you for what they use there.'),
      findsOneWidget,
      reason: 'the same model: nothing more to say',
    );
    expect(
      find.textContaining('are waiting for'),
      findsOneWidget,
      reason: "not on Anthropic's card",
    );
    await tester.tap(_onOpenRouter('Continue all on OpenRouter'));
    await settle(tester);
    final asked = [
      for (final message in harness.backend.sent)
        if (message.hasContinueOnAlternative()) message.continueOnAlternative,
    ];
    // One request for all: moving one wakes the others for a moment, so a
    // request each was refused as not waiting.
    expect(asked, hasLength(1));
    expect(asked.single.agentId, 'a1');
    expect(asked.single.allWaiting, isTrue);
    expect(asked.single.expectedOthers, 2);

    // More waiting by the tap: none moves, and the backend says so.
    harness.backend.refuse = '4 are waiting now, not 3. Choose again.';
    await tester.tap(_onOpenRouter('Continue all on OpenRouter'));
    await settle(tester);
    expect(
      _onOpenRouter('4 are waiting now, not 3. Choose again.'),
      findsOneWidget,
    );
    harness.backend.refuse = null;

    // Once they carry on, nothing waits: the offer goes.
    await harness.apply(
      tester,
      workspace(agents: [builder()])..providers.add(_openRouter),
    );
    await settle(tester);
    expect(find.textContaining('waiting for'), findsNothing);
    expect(_onOpenRouter('Continue all on OpenRouter'), findsNothing);
  });

  testWidgets('one agent: its name, the model it would use there, and a '
      'refusal as the backend words it', (tester) async {
    final harness = await _showProviders(
      tester,
      agents: [_held('a1', 'Builder', same: false)],
    );
    expect(_onOpenRouter('Builder is waiting for Claude.'), findsOneWidget);
    expect(
      _onOpenRouter(
        'OpenRouter charges you for what it uses there. On OpenRouter, it '
        'uses Claude Sonnet 4.6 instead.',
      ),
      findsOneWidget,
    );
    harness.backend.refuse = "Builder isn't waiting anymore.";
    await tester.tap(_onOpenRouter('Continue on OpenRouter'));
    await settle(tester);
    expect(_onOpenRouter("Builder isn't waiting anymore."), findsOneWidget);
  });

  testWidgets('no offer while nothing waits, or what waits would carry on '
      'elsewhere', (tester) async {
    await _showProviders(
      tester,
      agents: [
        builder(),
        _held('a2', 'Kai')
          ..usageWaitAlternative = (_alternative()..providerName = 'OpenAI'),
      ],
    );
    expect(find.byType(UsageWaitOffer), findsWidgets);
    expect(find.textContaining('waiting for Claude'), findsNothing);
  });

  testWidgets('Continue automatically (friend bug #8): off unless turned '
      'on, saying it charges without asking', (tester) async {
    final harness = await _showProviders(tester, agents: [builder()]);
    final toggle = find.byWidgetPredicate(
      (w) => w is Toggle && w.label == 'Continue automatically',
    );
    expect(tester.widget<Toggle>(toggle).value, isFalse);
    expect(
      find.textContaining('That provider charges you without asking first.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Continue automatically'));
    await settle(tester);
    expect(harness.last.setUsageAutoContinue.on, isTrue);

    await harness.apply(
      tester,
      workspace(agents: [builder()])
        ..providers.add(_openRouter)
        ..usageAutoContinue = true,
    );
    await settle(tester);
    expect(tester.widget<Toggle>(toggle).value, isTrue);
    await tester.tap(toggle);
    await settle(tester);
    expect(harness.last.setUsageAutoContinue.on, isFalse);
  });
}
