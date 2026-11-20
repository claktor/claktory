// Drives Settings → Secrets on an iPhone (the Simulator) end to end against a
// real Claktory: adds a secret, pastes a .env, gives the secret to an agent, has the agent use it in bash, checks
// the value never reaches the conversation, and takes it back.
//
//   cd app && flutter test integration_test/secrets_test.dart -d <simulator> \
//     --dart-define=FACTORY_DEV=true --dart-define=PAIRING_CODE='factory://…' \
//     --dart-define=AGENT=Builder
//
// The agent's model runs for real. It holds each step for a few seconds, for
// screenshots (`xcrun simctl io booted screenshot …`).
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/secrets.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'phone_test.dart' show hold, waitFor;

const _code = String.fromEnvironment('PAIRING_CODE');
const _agent = String.fromEnvironment('AGENT', defaultValue: 'Builder');
Finder _labeled(String label) => find.byWidgetPredicate(
  (w) => w is Semantics && w.properties.label == label,
);

/// The text fields of the open form of type [form].
Finder _fields(Type form) =>
    find.descendant(of: find.byType(form), matching: find.byType(EditableText));

/// Scrolls Settings until [finder] shows.
Future<void> _scrollTo(WidgetTester tester, Finder finder) async {
  await tester.scrollUntilVisible(
    finder,
    300,
    scrollable: find
        .descendant(
          of: find.byType(SettingsView),
          matching: find.byType(Scrollable),
        )
        .first,
  );
  await tester.pump(const Duration(milliseconds: 300));
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('secrets: add, paste, give, use, take back', (tester) async {
    await app.main();
    await tester.pump(const Duration(seconds: 1));
    if (find.text('Connect to your Claktory').evaluate().isNotEmpty) {
      expect(_code, isNotEmpty, reason: 'pass --dart-define=PAIRING_CODE=…');
      for (var i = 0; i < 50; i++) {
        if (find.byType(EditableText).evaluate().isNotEmpty) break;
        if (find.text('Paste code instead').evaluate().isNotEmpty) {
          await tester.tap(find.text('Paste code instead'));
        }
        await tester.pump(const Duration(milliseconds: 200));
      }
      await tester.enterText(find.byType(EditableText), _code);
      await tester.pump();
      await tester.tap(find.text('Connect'));
    }
    await waitFor(
      tester,
      find.text(_agent),
      timeout: const Duration(minutes: 1),
    );
    if (find.text("What's your name?").evaluate().isNotEmpty) {
      await tester.enterText(find.byType(EditableText).last, 'Matt');
      await tester.pump();
      await tester.tap(find.text('Continue'));
      await waitFor(tester, find.text("What's your name?"), gone: true);
    }
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    VaultState vault() => backend.workspace.workspace.vault;
    final agent = backend.workspace.workspace.agents.firstWhere(
      (a) => a.name == _agent,
    );
    final stamp = DateTime.now().millisecondsSinceEpoch % 100000;
    final name = 'Demo$stamp';
    final value = 'demo-value-$stamp-xyzzy';

    // Settings → Secrets.
    scope.page.value = AppPage.settings;
    await hold(tester, seconds: 1);
    expect(vault().unlocked, isTrue);

    // Add a secret.
    await _scrollTo(tester, find.text('Add a secret…'));
    await tester.tap(find.text('Add a secret…'));
    await tester.pump(const Duration(milliseconds: 300));
    final form = _fields(SecretForm);
    await tester.enterText(form.at(0), name);
    await tester.enterText(form.at(1), 'A token for the secrets test');
    await tester.enterText(form.at(2), 'DEMO_TOKEN');
    await tester.enterText(form.at(3), value);
    await tester.pump();
    await hold(tester, seconds: 2);
    await tester.tap(
      find.descendant(of: find.byType(SecretForm), matching: find.text('Add')),
    );
    await waitFor(tester, find.byType(SecretForm), gone: true);
    final secret = vault().secrets.firstWhere((s) => s.name == name);
    expect(secret.variables.single.name, 'DEMO_TOKEN');
    expect(find.text(value), findsNothing);

    // Paste a .env, as one secret.
    await _scrollTo(tester, find.text('Paste .env…'));
    await tester.tap(find.text('Paste .env…'));
    await tester.pump(const Duration(milliseconds: 300));
    await tester.enterText(
      _fields(EnvForm).first,
      '# from the test\nexport DEMO_A=alpha-$stamp\nDEMO_B="beta $stamp"\n',
    );
    await tester.pump();
    await tester.tap(find.text('Preview'));
    await waitFor(tester, find.text('Found 2 values.'));
    expect(find.text('alpha-$stamp'), findsNothing);
    await tester.enterText(_fields(EnvForm).first, 'Env$stamp');
    await tester.pump();
    await hold(tester, seconds: 3);
    await tester.tap(
      find.descendant(of: find.byType(EnvForm), matching: find.text('Add')),
    );
    await waitFor(tester, find.byType(EnvForm), gone: true);
    final env = vault().secrets.firstWhere((s) => s.name == 'Env$stamp');
    expect([for (final v in env.variables) v.name], ['DEMO_A', 'DEMO_B']);

    // Give it to the agent.
    final row = find.byKey(ValueKey(secret.id));
    await _scrollTo(tester, row);
    await tester.tap(_labeled(name));
    await tester.pump(const Duration(milliseconds: 500));
    await hold(tester, seconds: 2);
    final give = find.descendant(of: row, matching: find.text('Give to…'));
    await _scrollTo(tester, give);
    await tester.tap(give);
    await tester.pump(const Duration(milliseconds: 300));
    await hold(tester, seconds: 2);
    await tester.tap(_labeled(_agent).last);
    await waitFor(
      tester,
      find.descendant(of: row, matching: find.textContaining(_agent)),
    );
    expect(
      vault().secrets
          .firstWhere((s) => s.id == secret.id)
          .grants
          .map((g) => g.agentId),
      contains(agent.id),
    );
    await hold(tester, seconds: 3);

    // The agent uses it, and never sees it.
    scope.page.value = AppPage.agent;
    await tester.pump(const Duration(milliseconds: 300));
    await tester.tap(find.text(_agent).first);
    await waitFor(tester, find.text('Send'));
    await tester.enterText(
      find.byType(EditableText).last,
      'This is a test of secrets from the iPhone app. In bash with the '
      'secret $name, run exactly: echo "token=\$DEMO_TOKEN"; echo '
      '"\${#DEMO_TOKEN}". Then reply with just the length it printed and '
      'the word ok$stamp, nothing else.',
    );
    await tester.pump();
    await tester.tap(find.text('Send'));
    await waitFor(
      tester,
      find.textContaining('ok$stamp', findRichText: true),
      timeout: const Duration(minutes: 3),
    );
    // Its own reply: the length, but not the value.
    await waitFor(
      tester,
      find.textContaining('${value.length}', findRichText: true),
      timeout: const Duration(minutes: 1),
    );
    await hold(tester, seconds: 4);
    final transcript = backend.transcript;
    final everything = [
      for (var i = 0; i < transcript.length; i++)
        transcript[i].entry.toString(),
    ].join('\n');
    expect(everything, contains('[secret $name/DEMO_TOKEN]'));
    expect(everything, isNot(contains(value)));
    expect(find.textContaining(value, findRichText: true), findsNothing);

    // Recent use shows it, then take it back.
    scope.page.value = AppPage.settings;
    await tester.pump(const Duration(milliseconds: 300));
    await _scrollTo(tester, row);
    if (find
        .descendant(of: row, matching: find.text('Give to…'))
        .evaluate()
        .isEmpty) {
      await tester.tap(_labeled(name));
      await tester.pump(const Duration(milliseconds: 500));
    }
    final takeBack = find.descendant(of: row, matching: find.text('Take back'));
    await _scrollTo(tester, takeBack);
    await hold(tester, seconds: 3);
    await tester.tap(takeBack.first);
    await tester.pump(const Duration(milliseconds: 300));
    await hold(tester, seconds: 2);
    await tester.tap(
      find.descendant(
        of: find.byType(TakeBackForm),
        matching: find.text('Take back'),
      ),
    );
    await waitFor(tester, find.byType(TakeBackForm), gone: true);
    await tester.pump(const Duration(milliseconds: 500));
    expect(
      vault().secrets.firstWhere((s) => s.id == secret.id).grants,
      isEmpty,
    );
    final uses = await backend.listSecretUses(secret.id);
    // ignore: avoid_print
    print('uses: ${[for (final use in uses) '${use.kind} ${use.detail}']}');
    expect(uses.map((u) => u.kind), containsAll(['grant', 'use', 'revoke']));
    await hold(tester, seconds: 5);
  });
}
