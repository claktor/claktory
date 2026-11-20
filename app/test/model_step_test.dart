import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'account_test.dart' show signedOut, withAccount;
import 'app_test.dart' show builder, opus, workspace;
import 'harness.dart';
import 'provider_picker_test.dart' show providers;

Agent _rivet() => Agent(id: 'rivet', name: 'Rivet', color: 'blue');

/// A new Claktory with no model: the person named, nothing signed in.
Workspace _fresh({
  OnboardingStep step = OnboardingStep.ONBOARDING_STEP_NEEDED,
}) => workspace(signedIn: false, agents: [_rivet(), builder(working: false)])
  ..providers.clear()
  ..providers.addAll(providers(anthropicSignedIn: false))
  ..settings.userName = 'Matt'
  ..settings.onboarding = Onboarding(model: step);

/// Signed in to Claude: a model, the plan, the step done.
Workspace _connected() => _fresh(step: OnboardingStep.ONBOARDING_STEP_DONE)
  ..models.add(opus)
  ..providers[0] = (Provider(
    id: 'anthropic',
    name: 'Anthropic',
    signedInWith: 'Anthropic (Claude Pro/Max)',
    accounts: [
      Account(
        id: 'c1',
        label: 'matt@example.com',
        plan: 'Claude Max 20x',
        usage: ProviderUsage(),
      ),
    ],
    methods: providers(anthropicSignedIn: false)[0].methods,
  ));

void main() {
  testWidgets('a Claktory that has its model, or is from before, asks '
      'nothing', (tester) async {
    for (final step in [
      OnboardingStep.ONBOARDING_STEP_DONE,
      OnboardingStep.ONBOARDING_STEP_SKIPPED,
      OnboardingStep.ONBOARDING_STEP_UNSPECIFIED,
    ]) {
      final harness = Harness(_fresh(step: step));
      await harness.show(tester);
      await pumpFrames(tester);
      expect(find.text('Connect a model'), findsNothing, reason: '$step');
      await tester.pumpWidget(const SizedBox());
    }
  });

  testWidgets('a new Claktory asks for a model after the name; signing in '
      'with Claude shows the plan, and Continue opens Rivet', (tester) async {
    final harness = Harness(_fresh());
    await harness.show(tester);
    await pumpFrames(tester);
    expect(find.text('Connect a model'), findsOneWidget);
    expect(find.text('Sign in with Claude'), findsOneWidget);
    expect(find.text('Pro, Max or Ultra'), findsOneWidget);
    expect(find.text('Sign in with ChatGPT'), findsOneWidget);
    expect(find.text('Use an API key'), findsOneWidget);
    expect(find.text('Not now'), findsOneWidget);

    await tester.tap(find.text('Sign in with Claude'));
    await pumpFrames(tester);
    expect(harness.last.startLogin.providerId, 'anthropic');
    expect(harness.last.startLogin.methodId, 'oauth');

    // The sign-in under way shows in the step.
    await harness.apply(
      tester,
      _fresh()
        ..login = LoginFlow(
          providerId: 'anthropic',
          methodId: 'oauth',
          url: 'https://claude.ai/oauth/authorize?x=1',
        ),
    );
    await pumpFrames(tester);
    expect(
      find.text('Signing in with Anthropic (Claude Pro/Max)…'),
      findsOneWidget,
    );
    expect(harness.opened, ['https://claude.ai/oauth/authorize?x=1']);

    // Signed in: the backend calls the step done; it shows what connected.
    await harness.apply(tester, _connected());
    await pumpFrames(tester);
    expect(find.text('Connected'), findsOneWidget);
    expect(find.text('Claude Max 20x'), findsOneWidget);
    expect(find.text('Rivet is ready.'), findsOneWidget);
    await tester.tap(find.text('Continue'));
    await pumpFrames(tester);
    expect(harness.last.selectAgent.agentId, 'rivet');
    expect(find.text('Connected'), findsNothing);
  });

  testWidgets('Not now skips on every device and opens Rivet', (tester) async {
    final harness = Harness(_fresh());
    await harness.show(tester);
    await pumpFrames(tester);
    await tester.tap(find.text('Not now'));
    await pumpFrames(tester);
    final sent = harness.backend.sent;
    expect(
      sent.where((m) => m.hasSetOnboarding()).single.setOnboarding.skipModel,
      isTrue,
    );
    expect(
      sent.where((m) => m.hasSelectAgent()).single.selectAgent.agentId,
      'rivet',
    );
    await harness.apply(
      tester,
      _fresh(step: OnboardingStep.ONBOARDING_STEP_SKIPPED),
    );
    await pumpFrames(tester);
    expect(find.text('Connect a model'), findsNothing);
  });

  testWidgets('Use an API key: the providers with keys, then that sign-in', (
    tester,
  ) async {
    final harness = Harness(_fresh());
    await harness.show(tester);
    await pumpFrames(tester);
    await tester.tap(find.text('Use an API key'));
    await pumpFrames(tester);
    expect(
      find.text("Choose where your key is from. You'll paste it next."),
      findsOneWidget,
    );
    // Keys only: no subscription sign-ins here.
    expect(find.textContaining('OpenRouter API key'), findsOneWidget);
    expect(find.textContaining('OpenRouter OAuth'), findsNothing);
    expect(find.textContaining('ChatGPT'), findsNothing);
    await tester.enterText(find.byType(EditableText).last, 'openrouter');
    await tester.pump();
    await tester.tap(find.textContaining('OpenRouter API key'));
    await pumpFrames(tester);
    expect(harness.last.startLogin.providerId, 'openrouter');
    expect(harness.last.startLogin.methodId, 'api_key');

    // Back goes back to the choices.
    await harness.apply(tester, _fresh());
    await tester.tap(find.text('Use an API key'));
    await pumpFrames(tester);
    await tester.tap(find.text('Back'));
    await pumpFrames(tester);
    expect(find.text('Sign in with Claude'), findsOneWidget);
  });

  testWidgets('a refused sign-in says why in the step', (tester) async {
    final harness = Harness(_fresh());
    await harness.show(tester);
    await pumpFrames(tester);
    harness.backend.refuse = 'Claktory is updating.';
    await tester.tap(find.text('Sign in with ChatGPT'));
    await pumpFrames(tester);
    expect(find.textContaining('Claktory is updating.'), findsOneWidget);
    expect(find.text('Connect a model'), findsOneWidget);
  });

  testWidgets(
    "skipped, Rivet's composer offers Connect a model…: the same choices "
    'in a dialog',
    (tester) async {
      final harness = Harness(
        _fresh(step: OnboardingStep.ONBOARDING_STEP_SKIPPED)
          ..selectedAgentId = 'rivet',
      );
      await harness.show(tester);
      await pumpFrames(tester);
      expect(find.text('Connect a model so Rivet can answer.'), findsOneWidget);
      expect(
        find.text("No model available, so Rivet can't run."),
        findsNothing,
      );
      await tester.tap(find.text('Connect a model…'));
      await pumpFrames(tester);
      expect(find.text('Sign in with Claude'), findsOneWidget);
      expect(find.text('Not now'), findsNothing);
      await tester.tap(find.text('Cancel'));
      await pumpFrames(tester);
      expect(find.text('Sign in with Claude'), findsNothing);

      // Signed in from the dialog: what connected, and Continue closes it.
      await tester.tap(find.text('Connect a model…'));
      await pumpFrames(tester);
      await tester.tap(find.text('Sign in with Claude'));
      await pumpFrames(tester);
      await harness.apply(tester, _connected()..selectedAgentId = 'rivet');
      await pumpFrames(tester);
      expect(find.text('Claude Max 20x'), findsOneWidget);
      expect(find.text('Your agents can work now.'), findsOneWidget);
      await tester.tap(find.text('Continue'));
      await pumpFrames(tester);
      expect(find.text('Connected'), findsNothing);
    },
  );

  testWidgets("with models, an agent whose own can't run still offers Sign "
      'in…', (tester) async {
    final harness = Harness(
      workspace(agents: [builder(working: false, modelAvailable: false)]),
    );
    await harness.show(tester);
    await pumpFrames(tester);
    expect(
      find.text("No model available, so Builder can't run."),
      findsOneWidget,
    );
    expect(find.text('Connect…'), findsNothing);
  });

  testWidgets('Not now on the sign-in step of a Claktory from before, with '
      'no agent open, lands on Rivet', (tester) async {
    final snapshot = withAccount(signedOut)
      ..account.skippable = true
      ..agents.add(_rivet())
      ..selectedAgentId = '';
    final harness = Harness(snapshot);
    await harness.show(tester);
    await pumpFrames(tester);
    await tester.tap(find.text('Not now'));
    await pumpFrames(tester);
    final sent = harness.backend.sent;
    expect(
      sent.where((m) => m.hasSelectAgent()).single.selectAgent.agentId,
      'rivet',
    );
    expect(sent.last.hasSkipAccountSignIn(), isTrue);
  });
}
