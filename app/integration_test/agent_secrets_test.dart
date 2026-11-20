// An admin agent (Rivet) gives a secret with its secrets_grant tool, and the
// agent's settings then show it: on in the Secrets field, above the
// extensions, and named in the Secrets extension's box. Runs on an iPhone
// (the Simulator) already connected to a real Claktory:
//
//   cd app && flutter test integration_test/agent_secrets_test.dart \
//     -d <simulator> --dart-define=FACTORY_DEV=true \
//     --dart-define=ADMIN=Rivet --dart-define=AGENT=Builder
//
// The admin agent's model runs for real.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/forms.dart';
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'phone_test.dart' show hold, waitFor;

const _admin = String.fromEnvironment('ADMIN', defaultValue: 'Rivet');
const _agent = String.fromEnvironment('AGENT', defaultValue: 'Builder');

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('a secret the admin agent gives shows in the agent\'s settings', (
    tester,
  ) async {
    await app.main();
    await waitFor(
      tester,
      find.text(_agent),
      timeout: const Duration(minutes: 1),
    );
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    VaultState vault() => backend.workspace.workspace.vault;
    Agent named(String name) =>
        backend.workspace.workspace.agents.firstWhere((a) => a.name == name);
    expect(vault().unlocked, isTrue, reason: 'unlock the secrets first');
    // Left by a run that failed.
    for (final old in vault().secrets.where(
      (s) => s.name.startsWith('GRANT_'),
    )) {
      await backend.deleteSecret(old.id);
    }
    final stamp = DateTime.now().millisecondsSinceEpoch % 100000;
    final name = 'GRANT_$stamp';
    await backend.createSecret(name, 'For the grant test', [
      SecretEntry(variable: name, value: 'grant-value-$stamp'),
    ]);
    while (!vault().secrets.any((s) => s.name == name)) {
      await tester.pump(const Duration(milliseconds: 200));
    }

    // The admin agent gives it, with its tool.
    final admin = named(_admin);
    final agent = named(_agent);
    await backend.sendMessage(
      admin.id,
      'This is a test of secrets_grant from the iPhone app. Give the secret '
      '$name to $_agent with secrets_grant, then stop; no need to reply.',
    );
    bool given() => vault().secrets
        .firstWhere((s) => s.name == name)
        .grants
        .any((g) => g.agentId == agent.id);
    final end = DateTime.now().add(const Duration(minutes: 3));
    while (!given()) {
      if (DateTime.now().isAfter(end)) fail('$_admin never gave $name');
      await tester.pump(const Duration(milliseconds: 200));
    }

    // The agent's settings say so.
    await tester.tap(find.text(_agent).first);
    await waitFor(tester, find.byType(AgentView));
    showModal(
      tester.element(find.byType(AgentView).first),
      (context, close) => AgentSettingsForm(agent: agent, close: close),
    );
    await waitFor(tester, find.byType(AgentSettingsForm));
    final toggle = find.byWidgetPredicate(
      (w) => w is Toggle && w.label == name,
    );
    await tester.scrollUntilVisible(toggle, 300, scrollable: _scroller());
    await tester.pump(const Duration(milliseconds: 300));
    expect(tester.widget<Toggle>(toggle).value, isTrue);
    expect(
      tester.getTopLeft(toggle).dy,
      lessThan(tester.getTopLeft(find.text('Extensions')).dy),
    );
    await hold(tester, seconds: 3);
    final summary = find.textContaining(RegExp('^Has '));
    await tester.scrollUntilVisible(summary, 300, scrollable: _scroller());
    final said = tester.widget<Text>(summary).data!;
    // ignore: avoid_print
    print('Secrets extension: $said');
    expect(said, anyOf(contains(name), contains(' more.')));
    await hold(tester, seconds: 3);

    // Clean up.
    await backend.deleteSecret(
      vault().secrets.firstWhere((s) => s.name == name).id,
    );
  });
}

Finder _scroller() => find
    .descendant(
      of: find.byType(AgentSettingsForm),
      matching: find.byType(Scrollable),
    )
    .first;
