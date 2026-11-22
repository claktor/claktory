// Drives Settings → Secrets on an iPhone (the Simulator) end to end against a
// real Claktory: adds a secret, pastes a .env, gives the secret to an agent,
// has the agent use it in bash, checks the value never reaches the
// conversation, takes it back, then has the agent ask for a new secret and
// gives it from the request waiting in Settings → Secrets.
//
//   cd app && flutter test integration_test/secrets_test.dart -d <simulator> \
//     --dart-define=FACTORY_DEV=true --dart-define=PAIRING_CODE='factory://…' \
//     --dart-define=AGENT=Builder [--dart-define=RECOVERY_KEY=…]
//
// The agent's model runs for real. It holds each step for a few seconds, for
// screenshots (`xcrun simctl io booted screenshot …`).
import 'package:factory_app/controls.dart' show TextField;
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

/// The recovery key, for when it finds secrets locked.
const _recoveryKey = String.fromEnvironment('RECOVERY_KEY');
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

  testWidgets('secrets: add, paste, give, use, take back, a request given', (
    tester,
  ) async {
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
      await tester.pump(const Duration(milliseconds: 500));
      // A code asks first: the confirmation's Connect.
      await tester.tap(find.text('Connect').last);
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
    // Locked after a restart (claktory.com out of reach, say): the banner
    // says why, and the recovery key opens them.
    if (!vault().unlocked) {
      // Locked, Claktory won't show it: whoever runs this gives it.
      expect(_recoveryKey, isNotEmpty, reason: 'pass RECOVERY_KEY=…');
      const key = _recoveryKey;
      await _scrollTo(tester, find.textContaining('Secrets are locked'));
      await hold(tester, seconds: 4);
      final unlock = find.textContaining('recovery key…');
      await tester.tap(unlock.first);
      await tester.pump(const Duration(milliseconds: 300));
      await tester.enterText(_fields(UnlockSecretsForm).first, 'not-the-key');
      await tester.tap(find.text('Unlock'));
      await waitFor(tester, find.textContaining("isn't right"));
      await hold(tester, seconds: 3);
      await tester.enterText(_fields(UnlockSecretsForm).first, key);
      await tester.tap(find.text('Unlock'));
      await waitFor(tester, find.byType(UnlockSecretsForm), gone: true);
      await waitFor(
        tester,
        find.textContaining('Secrets are locked'),
        gone: true,
      );
    }
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
    await hold(tester, seconds: 3);

    // The agent asks for a new secret; its request waits at the top of
    // Settings → Secrets, and is given there.
    final asked = 'Req$stamp';
    scope.page.value = AppPage.agent;
    await tester.pump(const Duration(milliseconds: 300));
    await waitFor(tester, find.text('Send'));
    await tester.enterText(
      find.byType(EditableText).last,
      'This is a test of secret requests from the iPhone app. Ask me for a '
      'new secret with secrets_request: name $asked, description "Token '
      'for the request test", one variable REQ_TOKEN, reason "To test '
      'requests." Then stop; no need to reply.',
    );
    await tester.pump();
    await tester.tap(find.text('Send'));
    final requested = DateTime.now().add(const Duration(minutes: 3));
    while (!vault().requests.any((r) => r.newSecret.name == asked)) {
      if (DateTime.now().isAfter(requested)) fail('No request for $asked');
      await tester.pump(const Duration(milliseconds: 200));
    }
    scope.page.value = AppPage.settings;
    await tester.pump(const Duration(milliseconds: 300));
    final card = find.text('$_agent asks for a secret: $asked');
    await _scrollTo(tester, card);
    // This request's card, among any others waiting.
    final settings = find.ancestor(
      of: find.descendant(of: find.byType(SettingsView), matching: card),
      matching: find.byType(SecretRequestCard),
    );
    expect(settings, findsOneWidget);
    // One of the person's secrets may look like it (one from an earlier
    // run, with the same variable): the card starts on it. New secret instead.
    final addNew = find.descendant(
      of: settings,
      matching: find.text('New secret'),
    );
    if (find
        .descendant(of: settings, matching: find.text('Add and give'))
        .evaluate()
        .isEmpty) {
      await hold(tester, seconds: 3);
      await tester.tap(addNew);
      await tester.pump(const Duration(milliseconds: 300));
    }
    // New secret, filled in from the request: only its value to paste.
    await tester.enterText(
      find.descendant(
        of: find.descendant(
          of: settings,
          matching: find.byWidgetPredicate(
            (w) => w is TextField && w.placeholder == 'Paste the value',
          ),
        ),
        matching: find.byType(EditableText),
      ),
      'req-value-$stamp',
    );
    await tester.pump();
    await hold(tester, seconds: 3);
    await tester.tap(
      find.descendant(of: settings, matching: find.text('Add and give')),
    );
    await waitFor(tester, card, gone: true);
    final made = vault().secrets.firstWhere((s) => s.name == asked);
    expect(made.grants.map((g) => g.agentId), contains(agent.id));
    await hold(tester, seconds: 5);
  });
}
