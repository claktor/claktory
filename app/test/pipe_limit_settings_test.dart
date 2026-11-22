// An agent limited to a pipe (Agent.quarantined, P1) in its settings: its
// secrets and extensions don't work, so they show greyed and take no taps,
// under a line saying why; Ask, which still works, stays as it is.
import 'package:factory_app/controls.dart' show Toggle, showModal;
import 'package:factory_app/forms.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _note =
    "Limited to the pipe with Ana's Claktory: it can only read and answer "
    "it, and ask you. Its secrets and extensions are off until you let it run "
    "commands in the pipe's settings.";

Pipe _pipe(String id, String claktory, {List<String> limited = const []}) =>
    Pipe(
      id: id,
      name: 'Acme launch',
      projectId: 'p1',
      peer: PipePeer(claktoryName: claktory),
      memberIds: ['liaison'],
      quarantinedIds: limited,
    );

Workspace _workspace({required bool limited}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    Agent(
      id: 'liaison',
      name: 'Liaison',
      projectId: 'p1',
      modelAvailable: true,
      quarantined: limited,
      extensions: [
        AgentExtension(id: 'ask', enabled: true),
        AgentExtension(id: 'web', enabled: true),
        AgentExtension(id: 'checklist'),
      ],
    ),
  ],
  selectedAgentId: 'liaison',
  extensions: [
    Extension(id: 'ask', name: 'Ask', description: 'Asks you.'),
    Extension(id: 'web', name: 'Web', description: 'Searches the web.'),
    Extension(id: 'checklist', name: 'Checklist', description: 'A list.'),
  ],
  pipes: [
    _pipe('pp', "Ana's Claktory", limited: limited ? ['liaison'] : []),
  ],
  settings: Settings(userName: 'Matt'),
  vault: VaultState(
    exists: true,
    unlocked: true,
    secrets: [
      SecretInfo(
        id: 'aws',
        name: 'AWS',
        grants: [SecretGrant(agentId: 'liaison')],
      ),
      SecretInfo(id: 'stripe', name: 'Stripe'),
    ],
  ),
);

Future<Harness> _settings(WidgetTester tester, {required bool limited}) async {
  final workspace = _workspace(limited: limited);
  final harness = Harness(workspace);
  await harness.show(tester, size: const Size(1200, 1600));
  await pumpFrames(tester);
  showModal(
    tester.element(find.text('Liaison').first),
    (context, close) =>
        AgentSettingsForm(agent: workspace.agents.first, close: close),
  );
  await pumpFrames(tester);
  return harness;
}

Toggle _toggle(WidgetTester tester, String label) => tester.widget<Toggle>(
  find.byWidgetPredicate((w) => w is Toggle && w.label == label),
);

void main() {
  testWidgets("limited to a pipe, its secrets and extensions are greyed and "
      "take no taps, under a line saying why; Ask isn't", (tester) async {
    final harness = await _settings(tester, limited: true);
    expect(find.text(_note), findsOneWidget);
    // Above the secrets.
    expect(
      tester.getTopLeft(find.text(_note)).dy,
      lessThan(tester.getTopLeft(find.text('AWS')).dy),
    );

    for (final secret in ['AWS', 'Stripe']) {
      expect(_toggle(tester, secret).onChanged, isNull, reason: secret);
    }
    expect(_toggle(tester, 'AWS').value, isTrue, reason: 'still given');
    await tester.tap(find.text('Stripe'));
    await tester.tap(find.text('AWS'));
    await pumpFrames(tester);
    expect(
      harness.backend.sent.where(
        (m) => m.hasGrantSecret() || m.hasRevokeSecret(),
      ),
      isEmpty,
    );

    expect(_toggle(tester, 'Web').onChanged, isNull);
    expect(_toggle(tester, 'Web').value, isTrue, reason: 'back once lifted');
    expect(_toggle(tester, 'Checklist').onChanged, isNull);
    expect(_toggle(tester, 'Ask').onChanged, isNotNull, reason: 'it works');
  });

  testWidgets('not limited, nothing changes', (tester) async {
    await _settings(tester, limited: false);
    expect(find.text(_note), findsNothing);
    for (final label in ['AWS', 'Stripe', 'Ask', 'Web', 'Checklist']) {
      expect(_toggle(tester, label).onChanged, isNotNull, reason: label);
    }
  });

  test('the line names every pipe it is limited to', () {
    expect(pipeLimitNote([_pipe('a', "Ana's Claktory")]), _note);
    expect(
      pipeLimitNote([
        _pipe('a', "Ana's Claktory"),
        _pipe('b', "Lea's Claktory"),
      ]),
      "Limited to the pipes with Ana's Claktory and Lea's Claktory: it can "
      'only read and answer them, and ask you. Its secrets and extensions '
      'are off until you let it run commands in their settings.',
    );
    expect(
      pipeLimitNote([]),
      startsWith('Limited to a pipe: it can only read and answer it'),
    );
  });
}
