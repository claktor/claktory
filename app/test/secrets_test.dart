import 'package:factory_app/account.dart' show signInPutOff;
import 'package:factory_app/backend.dart';
import 'package:factory_app/controls.dart' show TextField, Toggle;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/secrets.dart';
import 'package:factory_app/update.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

SecretInfo _aws({List<SecretGrant> grants = const []}) => SecretInfo(
  id: 's1',
  name: 'AWS',
  description: "Matt's deploy account, eu-west-1",
  variables: [
    SecretVariable(name: 'AWS_ACCESS_KEY_ID', hint: '7XQA'),
    SecretVariable(name: 'AWS_SECRET_ACCESS_KEY', hint: '3f9a'),
  ],
  grants: grants,
);

Workspace _workspace({List<SecretInfo>? secrets}) => Workspace(
  agents: [
    Agent(id: 'june', name: 'June', color: 'pink', modelAvailable: true),
    Agent(id: 'mike', name: 'Mike', color: 'blue', modelAvailable: true),
  ],
  selectedAgentId: 'june',
  settings: Settings(userName: 'Matt'),
  vault: VaultState(exists: true, unlocked: true, secrets: secrets ?? [_aws()]),
);

Finder _labeled(String label) => find.byWidgetPredicate(
  (w) => w is Semantics && w.properties.label == label,
);

/// [child] in the window, as the app's views get it.
Future<void> _show(WidgetTester tester, Harness harness, Widget child) async {
  tester.view.physicalSize = const Size(1000, 1400);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    WidgetsApp(
      color: const Color(0xFF000000),
      builder: (context, _) => AppScope(
        backend: harness.backend,
        login: harness.login,
        page: harness.page,
        appBuild: AppBuild(
          version: 'dev',
          bundlePath: '/Applications/Factory.app',
          quit: () {},
          updater: harness.updater,
        ),
        drafts: harness.drafts,
        history: harness.history,
        connections: harness.connections,
        child: Overlay(
          initialEntries: [
            OverlayEntry(
              builder: (context) => SingleChildScrollView(
                padding: const EdgeInsets.all(24),
                child: child,
              ),
            ),
          ],
        ),
      ),
    ),
  );
  await tester.pump();
}

List<ClientMessage> _sent(Harness harness, bool Function(ClientMessage) of) => [
  for (final message in harness.backend.sent)
    if (of(message)) message,
];

void main() {
  test("an agent's secrets, in a line", () {
    expect(agentSecretsSummary([]), 'Has no secrets yet.');
    expect(agentSecretsSummary(['AWS']), 'Has AWS.');
    expect(agentSecretsSummary(['A', 'B', 'C']), 'Has A, B and C.');
    expect(agentSecretsSummary(['A', 'B', 'C', 'D']), 'Has A, B and 2 more.');
  });

  test('a name is suggested from the variables', () {
    expect(suggestSecretName(['AWS_ACCESS_KEY_ID', 'AWS_REGION']), 'AWS');
    expect(suggestSecretName(['CLOUDFLARE_API_TOKEN']), 'Cloudflare');
    expect(suggestSecretName(['A_KEY', 'B_KEY']), '');
    expect(suggestSecretName([]), '');
  });

  testWidgets('Settings → Secrets opens to the list: nothing to set up, '
      'unlock or type', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, const SecretsSettings());
    expect(find.text('AWS'), findsOneWidget);
    expect(find.text('Add a secret…'), findsOneWidget);
    expect(find.text('Paste .env…'), findsOneWidget);
    for (final gone in [
      'Set up secrets…',
      'Secrets are locked',
      'Stay unlocked for',
      'Lock now',
      'Change passphrase…',
      'New recovery key…',
    ]) {
      expect(find.text(gone), findsNothing, reason: gone);
    }
  });

  testWidgets('Give to… gives it at once', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, const SecretsSettings());
    expect(find.text('AWS'), findsOneWidget);
    expect(find.textContaining('Not given to any agent'), findsOneWidget);
    await tester.tap(_labeled('AWS'));
    await pumpFrames(tester);
    expect(find.text('••••••3f9a'), findsOneWidget, reason: 'its hint');

    await tester.tap(find.text('Give to…'));
    await pumpFrames(tester);
    await tester.tap(_labeled('June').first);
    await pumpFrames(tester);
    expect(find.text('Enter your passphrase'), findsNothing);
    final given = _sent(harness, (m) => m.hasGrantSecret()).single.grantSecret;
    expect(given.id, 's1');
    expect(given.agentId, 'june');
    expect(given.hasProof(), isFalse);
  });

  testWidgets('Show shows the values for 30 seconds; Copy clears in a minute', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    harness.backend.secretValues['s1'] = [
      SecretEntry(variable: 'AWS_ACCESS_KEY_ID', value: 'AKIA0000007XQA'),
      SecretEntry(variable: 'AWS_SECRET_ACCESS_KEY', value: 'abcdefgh3f9a'),
    ];
    final clipboard = <String>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          clipboard.add((call.arguments as Map)['text'] as String);
        }
        if (call.method == 'Clipboard.getData') {
          return {'text': clipboard.isEmpty ? '' : clipboard.last};
        }
        return null;
      },
    );
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(_labeled('AWS'));
    await pumpFrames(tester);
    await tester.tap(find.text('Show'));
    await pumpFrames(tester);
    expect(find.text('abcdefgh3f9a'), findsOneWidget);
    expect(find.textContaining('Hides in'), findsOneWidget);
    expect(find.text('Hide'), findsOneWidget);

    await tester.pump(const Duration(seconds: 31));
    expect(find.text('abcdefgh3f9a'), findsNothing);
    expect(find.text('••••••3f9a'), findsOneWidget);

    // Copy: gone from the clipboard after a minute.
    await tester.tap(find.text('Copy').last);
    await pumpFrames(tester);
    expect(clipboard.last, 'abcdefgh3f9a');
    expect(
      find.text('Copied. The clipboard clears in a minute.'),
      findsOneWidget,
    );
    await tester.pump(const Duration(seconds: 61));
    await tester.pump();
    expect(clipboard.last, '');
  });

  testWidgets('taking back can stop the tasks using it', (tester) async {
    final harness = Harness(
      _workspace(
        secrets: [
          _aws(grants: [SecretGrant(agentId: 'june', runningTasks: 2)]),
        ],
      ),
    );
    await _show(tester, harness, const SecretsSettings());
    expect(find.textContaining('Given to June'), findsOneWidget);
    await tester.tap(_labeled('AWS'));
    await pumpFrames(tester);
    expect(find.textContaining('2 running tasks using it'), findsOneWidget);
    await tester.tap(find.text('Take back'));
    await pumpFrames(tester);
    expect(find.text('Take back AWS from June?'), findsOneWidget);
    expect(
      find.text("Also stop June's 2 tasks that are using it"),
      findsOneWidget,
    );
    await tester.tap(find.text('Take back').last);
    await pumpFrames(tester);
    final revoked = _sent(harness, (m) => m.hasRevokeSecret()).single;
    expect(revoked.revokeSecret.agentId, 'june');
    expect(revoked.revokeSecret.stopRunningTasks, isTrue);
  });

  testWidgets('a pasted .env is previewed by name, then added as one secret', (
    tester,
  ) async {
    final harness = Harness(_workspace(secrets: []));
    harness.backend.envPreview = EnvPreview(
      variables: [
        SecretVariable(name: 'AWS_ACCESS_KEY_ID', hint: '7XQA'),
        SecretVariable(name: 'AWS_REGION'),
      ],
      warnings: ['AWS_REGION is too short to hide if an agent prints it.'],
    );
    await _show(tester, harness, const SecretsSettings());
    expect(find.text('No secrets yet.'), findsOneWidget);
    await tester.tap(find.text('Paste .env…'));
    await pumpFrames(tester);
    const text = 'AWS_ACCESS_KEY_ID=AKIA0000007XQA\nAWS_REGION=eu-west-1\n';
    await tester.enterText(find.byType(EditableText).last, text);
    await tester.tap(find.text('Preview'));
    await pumpFrames(tester);
    expect(find.text('Found 2 values.'), findsOneWidget);
    expect(find.text('AWS_REGION'), findsOneWidget);
    expect(find.textContaining('too short to hide'), findsOneWidget);
    expect(find.text('eu-west-1'), findsNothing, reason: 'never shown back');
    await tester.tap(find.text('Add'));
    await pumpFrames(tester);
    final imported = _sent(harness, (m) => m.hasImportEnv()).single.importEnv;
    expect(imported.text, text);
    expect(imported.name, 'AWS', reason: 'suggested from the variables');
    expect(imported.onePerVariable, isFalse);
  });

  testWidgets('Ask each time: a switch on an opened secret, said in its '
      'summary', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, const SecretsSettings());
    expect(find.textContaining('Asks each time'), findsNothing);
    await tester.tap(_labeled('AWS'));
    await pumpFrames(tester);
    expect(find.text(askEachTimeNote), findsOneWidget);
    await tester.tap(_labeled(askEachTimeLabel));
    await pumpFrames(tester);
    final set = _sent(
      harness,
      (m) => m.hasSetSecretAskEachTime(),
    ).single.setSecretAskEachTime;
    expect(set.id, 's1');
    expect(set.askEachTime, isTrue);
    expect(set.hasProof(), isFalse);

    // As the backend has it now.
    await harness.apply(
      tester,
      harness.backend.workspace.workspace.deepCopy()
        ..vault.secrets.single.askEachTime = true,
    );
    await pumpFrames(tester);
    expect(find.textContaining('· Asks each time'), findsOneWidget);
    await tester.tap(_labeled(askEachTimeLabel));
    await pumpFrames(tester);
    expect(
      _sent(
        harness,
        (m) => m.hasSetSecretAskEachTime(),
      ).last.setSecretAskEachTime.askEachTime,
      isFalse,
    );
  });

  testWidgets('turning Ask each time off can be refused: it says why', (
    tester,
  ) async {
    final harness = Harness(_workspace(secrets: [_aws()..askEachTime = true]));
    harness.backend.onSend = (sent) {
      if (sent.hasSetSecretAskEachTime()) {
        throw RequestFailure('Unlock your secrets first.');
      }
    };
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(_labeled('AWS'));
    await pumpFrames(tester);
    await tester.tap(_labeled(askEachTimeLabel));
    await pumpFrames(tester);
    expect(find.text('Unlock your secrets first.'), findsOneWidget);
  });

  testWidgets('a new secret can ask each time from the start', (tester) async {
    final harness = Harness(_workspace(secrets: []));
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(find.text('Add a secret…'));
    await pumpFrames(tester);
    final fields = find.byType(EditableText);
    await tester.enterText(fields.at(0), 'Stripe');
    await tester.enterText(fields.at(2), 'STRIPE_KEY');
    await tester.enterText(fields.at(3), 'sk-live-1234');
    await tester.tap(_labeled(askEachTimeLabel));
    await pumpFrames(tester);
    await tester.tap(find.text('Add'));
    await pumpFrames(tester);
    final made = _sent(harness, (m) => m.hasCreateSecret()).single.createSecret;
    expect(made.name, 'Stripe');
    expect(made.askEachTime, isTrue);
  });

  testWidgets('adding a secret by hand', (tester) async {
    final harness = Harness(_workspace(secrets: []));
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(find.text('Add a secret…'));
    await pumpFrames(tester);
    final fields = find.byType(EditableText);
    await tester.enterText(fields.at(0), 'Cloudflare');
    await tester.enterText(fields.at(1), 'Token for the claktory.com zone');
    await tester.enterText(fields.at(2), 'CLOUDFLARE_API_TOKEN');
    await tester.tap(find.text('Add'));
    await tester.pump();
    expect(find.text('Enter a value for each variable.'), findsOneWidget);
    await tester.enterText(fields.at(3), 'cf-token-value-1234');
    await tester.tap(find.text('Add'));
    await pumpFrames(tester);
    final made = _sent(harness, (m) => m.hasCreateSecret()).single.createSecret;
    expect(made.name, 'Cloudflare');
    expect(made.entries.single.variable, 'CLOUDFLARE_API_TOKEN');
    expect(made.entries.single.value, 'cf-token-value-1234');
  });

  testWidgets('a key file saved as a file keeps every line', (tester) async {
    final harness = Harness(_workspace(secrets: []));
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(find.text('Add a secret…'));
    await pumpFrames(tester);
    final fields = find.byType(EditableText);
    await tester.enterText(fields.at(0), 'App Store Connect');
    await tester.enterText(fields.at(2), 'ASC_KEY_PATH');
    await tester.tap(_labeled('Save as a file'));
    await pumpFrames(tester);
    expect(find.text('Paste the file’s contents'), findsOneWidget);
    const key =
        '-----BEGIN PRIVATE KEY-----\nMIGTAgEAMBMGByqGSM49\n'
        '-----END PRIVATE KEY-----\n';
    await tester.enterText(find.byType(EditableText).at(3), key);
    await tester.tap(find.text('Add'));
    await pumpFrames(tester);
    final made = _sent(harness, (m) => m.hasCreateSecret()).single.createSecret;
    expect(made.entries.single.variable, 'ASC_KEY_PATH');
    expect(made.entries.single.file, isTrue);
    expect(made.entries.single.value, key);
  });

  testWidgets('in agent settings, a toggle gives a secret', (tester) async {
    final harness = Harness(_workspace());
    await _show(
      tester,
      harness,
      AgentSecretsField(agent: harness.backend.workspace.agent('june')!),
    );
    expect(
      find.text('Secrets this agent can use. It never sees their values.'),
      findsOneWidget,
    );
    await tester.tap(_labeled('AWS'));
    await pumpFrames(tester);
    final given = _sent(harness, (m) => m.hasGrantSecret()).single;
    expect(given.grantSecret.agentId, 'june');
  });

  testWidgets("an agent's settings list its secrets, above the extensions "
      'and in the Secrets extension', (tester) async {
    final w = workspace(agents: [builder(working: false)])
      ..extensions.add(
        Extension(
          id: 'secrets',
          name: 'Secrets',
          description: 'Asks you for secrets it needs.',
        ),
      )
      ..vault = VaultState(
        exists: true,
        unlocked: true,
        secrets: [
          for (final name in ['ASC_KEY_ID', 'CLAKTORY_TEAM_ID'])
            SecretInfo(
              id: name,
              name: name,
              grants: [SecretGrant(agentId: 'a1')],
              variables: [SecretVariable(name: name)],
            ),
          SecretInfo(id: 'other', name: 'Stripe'),
        ],
      );
    w.agents.single.extensions.add(AgentExtension(id: 'secrets'));
    final harness = Harness(w);
    await harness.show(tester);
    await settle(tester);
    await tester.tap(find.text('Opus 5.5 · High'));
    await settle(tester);
    // Even with the extension off.
    expect(find.text('Has ASC_KEY_ID and CLAKTORY_TEAM_ID.'), findsOneWidget);
    bool on(String name) => tester
        .widget<Toggle>(
          find.byWidgetPredicate((w) => w is Toggle && w.label == name),
        )
        .value;
    expect(on('ASC_KEY_ID'), isTrue);
    expect(on('CLAKTORY_TEAM_ID'), isTrue);
    expect(on('Stripe'), isFalse);
    expect(
      tester.getTopLeft(find.text('Stripe')).dy,
      lessThan(tester.getTopLeft(find.text('Extensions')).dy),
    );
  });

  testWidgets("the Secrets panel beside an agent lists what it has, however "
      'it was given', (tester) async {
    // Rivet's secrets_grant, a decided request and Settings → Secrets all
    // make the same grant in the vault the workspace carries.
    final w = workspace(agents: [builder(working: false)])
      ..extensions.add(
        Extension(
          id: 'secrets',
          name: 'Secrets',
          description: 'Asks you for secrets it needs.',
        ),
      )
      ..vault = VaultState(
        exists: true,
        unlocked: true,
        secrets: [
          for (final name in ['CF_CLAKTORY_DNS', 'HETZNER', 'ASC_KEY_ID'])
            SecretInfo(
              id: name,
              name: name,
              grants: [SecretGrant(agentId: 'a1')],
              variables: [SecretVariable(name: '${name}_TOKEN')],
            ),
          SecretInfo(id: 'other', name: 'Stripe'),
        ],
      );
    w.agents.single
      ..extensions.add(AgentExtension(id: 'secrets', enabled: true))
      ..openPanel = 'secrets';
    final harness = Harness(w);
    await harness.show(tester);
    await settle(tester);
    // No view comes for it: the panel isn't left blank.
    for (final name in ['CF_CLAKTORY_DNS', 'HETZNER', 'ASC_KEY_ID']) {
      expect(find.text(name), findsOneWidget, reason: name);
      expect(find.text('${name}_TOKEN'), findsOneWidget, reason: name);
    }
    expect(find.text('Stripe'), findsNothing);
    expect(find.text('Give a secret…'), findsOneWidget);
  });

  testWidgets('the Secrets panel: when each was given, Take back, and giving '
      'another', (tester) async {
    final now = DateTime(2026, 11, 20, 12);
    final given = Int64(
      now.subtract(const Duration(days: 2)).millisecondsSinceEpoch,
    );
    final used = Int64(
      now.subtract(const Duration(hours: 3)).millisecondsSinceEpoch,
    );
    final harness = Harness(
      _workspace(
        secrets: [
          _aws(
              grants: [
                SecretGrant(agentId: 'mike', grantedMs: given),
                SecretGrant(agentId: 'june', grantedMs: given, runningTasks: 1),
              ],
            )
            ..lastUsedBy = 'june'
            ..lastUsedMs = used,
          SecretInfo(id: 's2', name: 'Stripe', description: 'Test mode'),
        ],
      ),
    );
    await _show(
      tester,
      harness,
      SizedBox(
        height: 800,
        child: AgentSecretsPanel(agentId: 'june', now: now),
      ),
    );
    expect(find.text('AWS'), findsOneWidget);
    expect(find.text("Matt's deploy account, eu-west-1"), findsOneWidget);
    expect(
      find.text('AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY'),
      findsOneWidget,
    );
    expect(
      find.text(
        'Given 2 days ago · Last used 3 h ago · Used by 1 running task',
      ),
      findsOneWidget,
    );
    expect(find.text('Stripe'), findsNothing);

    await tester.tap(find.text('Take back'));
    await pumpFrames(tester);
    expect(find.text('Take back AWS from June?'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);

    await tester.tap(find.text('Give a secret…'));
    await pumpFrames(tester);
    expect(find.text('Give a secret to June'), findsOneWidget);
    expect(find.text('Test mode'), findsOneWidget);
    await tester.tap(_labeled('Stripe'));
    await pumpFrames(tester);
    final grant = _sent(harness, (m) => m.hasGrantSecret()).single.grantSecret;
    expect(grant.id, 's2');
    expect(grant.agentId, 'june');
  });

  testWidgets('the Secrets panel of an agent with none', (tester) async {
    final harness = Harness(_workspace());
    await _show(
      tester,
      harness,
      const SizedBox(height: 800, child: AgentSecretsPanel(agentId: 'june')),
    );
    expect(find.text('June has no secrets yet.'), findsOneWidget);
    expect(find.text('AWS'), findsNothing);
    expect(find.text('Give a secret…'), findsOneWidget);
  });

  testWidgets('no secrets: agent settings say where to add them', (
    tester,
  ) async {
    final harness = Harness(_workspace(secrets: []));
    await _show(
      tester,
      harness,
      AgentSecretsField(agent: harness.backend.workspace.agent('june')!),
    );
    expect(
      find.text('No secrets yet. Add them in Settings → Secrets.'),
      findsOneWidget,
    );
  });

  test('the secret that looks like the one asked for', () {
    final dns = SecretInfo(
      id: 'cf',
      name: 'Cloudflare DNS',
      variables: [SecretVariable(name: 'CF_TOKEN')],
    );
    final secrets = [_aws(), dns];
    expect(closestSecret(NewSecret(name: 'Cloudflare token'), secrets), dns);
    expect(closestSecret(NewSecret(name: 'aws'), secrets)?.id, 's1');
    expect(
      closestSecret(
        NewSecret(
          name: 'Deploy keys',
          variables: [SecretVariable(name: 'AWS_ACCESS_KEY_ID')],
        ),
        secrets,
      )?.id,
      's1',
    );
    // A word every secret's name may have says nothing.
    expect(closestSecret(NewSecret(name: 'Resend API key'), secrets), isNull);
    expect(closestSecret(NewSecret(name: 'Resend'), []), isNull);
  });

  SecretRequest resend() => SecretRequest(
    id: 'r1',
    agentId: 'june',
    secretName: 'Resend',
    newSecret: NewSecret(
      name: 'Resend',
      description: 'Sends mail from claktory.com',
      variables: [SecretVariable(name: 'RESEND_API_KEY')],
    ),
    reason: 'To send the **launch** email.',
    state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
  );

  Finder field(String placeholder) => find.descendant(
    of: find.byWidgetPredicate(
      (w) => w is TextField && w.placeholder == placeholder,
    ),
    matching: find.byType(EditableText),
  );

  testWidgets("a request like none of the person's secrets: New secret, filled "
      'in from it', (tester) async {
    final harness = Harness(_workspace());
    final request = resend();
    await _show(tester, harness, SecretRequestCard(request));
    expect(find.text('June asks for a secret: Resend'), findsOneWidget);
    // Under the header, and in its field to change.
    expect(find.text('Sends mail from claktory.com'), findsNWidgets(2));
    expect(
      find.text("Filled in from June's request. Change anything you need."),
      findsOneWidget,
    );
    expect(find.text('Resend'), findsOneWidget, reason: 'the name, to edit');
    await tester.tap(find.text('Add and give to June'));
    await tester.pump();
    expect(find.text('Enter a value for each variable.'), findsOneWidget);
    await tester.enterText(field('Paste the value'), 're_123456789');
    await tester.enterText(find.byType(EditableText).first, 'Resend mail');
    await tester.tap(find.text('Add and give to June'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.approve, isTrue);
    expect(decided.requestId, 'r1');
    expect(decided.secretId, isEmpty);
    expect(decided.name, 'Resend mail');
    expect(decided.description, 'Sends mail from claktory.com');
    expect(decided.entries.single.variable, 'RESEND_API_KEY');
    expect(decided.entries.single.value, 're_123456789');

    // Decided, the backend sends it back.
    await tester.pumpWidget(const SizedBox());
    await _show(
      tester,
      harness,
      SecretRequestCard(
        request.deepCopy()
          ..state = SecretRequestState.SECRET_REQUEST_STATE_GRANTED
          ..givenSecretId = 's9'
          ..givenSecretName = 'Resend mail'
          ..decidedMs = Int64(
            DateTime(2026, 11, 20, 14, 2).millisecondsSinceEpoch,
          ),
      ),
    );
    expect(find.text('Add and give to June'), findsNothing);
    expect(find.text('Add a comment'), findsNothing);
    expect(
      find.textContaining('Gave Resend mail to June · ', findRichText: true),
      findsOneWidget,
    );
  });

  testWidgets("a request's fields keep their names for a screen reader once "
      'typed in: the labels over them may have scrolled away', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = Harness(_workspace());
    await _show(tester, harness, SecretRequestCard(resend()));
    await tester.enterText(field('Paste the value'), 're_123456789');
    await tester.tap(find.text('Add a comment'));
    await tester.pump();
    await tester.enterText(field(_comment), 'Not yet');
    await tester.pump();
    for (final label in [
      'Name',
      secretGrantsLabel,
      'RESEND_API_KEY',
      'A comment for June',
    ]) {
      expect(
        find.semantics.byPredicate(
          (node) => node.label == label && node.flagsCollection.isTextField,
        ),
        findsOne,
        reason: label,
      );
    }
    semantics.dispose();
  });

  testWidgets('a request like one of them: Your secrets, picked, Give it', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    final request = resend()
      ..newSecret.name = 'AWS deploy'
      ..secretName = 'AWS deploy';
    await _show(tester, harness, SecretRequestCard(request));
    expect(
      find.text(
        'Filled in from June\'s request. Change anything you '
        'need.',
      ),
      findsNothing,
    );
    await tester.tap(find.text('Give AWS to June'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.approve, isTrue);
    expect(decided.secretId, 's1');
    expect(decided.entries, isEmpty);
    expect(decided.name, isEmpty);
  });

  SecretRequest cloudflare() => SecretRequest(
    id: 'r2',
    agentId: 'june',
    secretName: 'Claktory DNS',
    newSecret: NewSecret(
      name: 'Claktory DNS',
      description: 'Cloudflare: DNS Edit on claktory.com',
      variables: [SecretVariable(name: 'CLOUDFLARE_API_TOKEN')],
    ),
    state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
  );

  Workspace dns() => _workspace(
    secrets: [
      SecretInfo(
        id: 'cf',
        name: 'CF_CLAKTORY_DNS',
        variables: [SecretVariable(name: 'CF_CLAKTORY_DNS')],
      ),
    ],
  );

  testWidgets('a secret with other names than asked: given under the asked '
      'name too, to start with', (tester) async {
    final harness = Harness(dns());
    await _show(tester, harness, SecretRequestCard(cloudflare()));
    expect(
      find.text(
        'This secret sets CF_CLAKTORY_DNS, not CLOUDFLARE_API_TOKEN. Check '
        "it's the one June needs.",
      ),
      findsOneWidget,
    );
    expect(find.text('Also give it as CLOUDFLARE_API_TOKEN'), findsOneWidget);
    expect(
      find.text(
        'June gets its value as CF_CLAKTORY_DNS and CLOUDFLARE_API_TOKEN.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Give CF_CLAKTORY_DNS to June'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.secretId, 'cf');
    expect(decided.aliases, {'CLOUDFLARE_API_TOKEN': 'CF_CLAKTORY_DNS'});
    expect(decided.asIs, isFalse);
  });

  test('what a secret chosen for a request leaves out, in its variables', () {
    String line(List<String> asked, List<String> sets) =>
        mismatchLine(agent: 'Sam', asked: asked, sets: sets);
    expect(
      line(['ANTHROPIC_API_KEY'], ['CF_CLAKTORY_DNS']),
      'This secret sets CF_CLAKTORY_DNS, not ANTHROPIC_API_KEY. Check '
      "it's the one Sam needs.",
    );
    expect(
      line(['AWS_KEY', 'AWS_SECRET'], ['AWS_ACCESS_KEY_ID', 'AWS_SECRET_KEY']),
      "This secret doesn't set AWS_KEY or AWS_SECRET. Check it's the one Sam "
      'needs.',
    );
    expect(
      line(['A', 'B', 'C'], ['D']),
      "This secret doesn't set A, B or C. Check it's the one Sam needs.",
    );
    // Some match: only what it leaves out, no check.
    expect(line(['A', 'B'], ['A', 'C']), "This secret doesn't set B.");
  });

  testWidgets('a secret with several variables, for several names: a row '
      'per name, the same as which of them', (tester) async {
    final harness = Harness(
      _workspace(
        secrets: [
          SecretInfo(
            id: 'aws',
            name: 'AWS deploy',
            variables: [
              SecretVariable(name: 'AWS_ACCESS_KEY_ID'),
              SecretVariable(name: 'AWS_SECRET_ACCESS_KEY'),
            ],
          ),
        ],
      ),
    );
    await _show(
      tester,
      harness,
      SecretRequestCard(
        resend()
          ..newSecret.name = 'AWS deploy'
          ..newSecret.variables.clear()
          ..newSecret.variables.addAll([
            SecretVariable(name: 'AWS_KEY'),
            SecretVariable(name: 'AWS_SECRET'),
          ]),
      ),
    );
    expect(
      find.text(
        "This secret doesn't set AWS_KEY or AWS_SECRET. Check it's the one "
        'June needs.',
      ),
      findsOneWidget,
    );
    expect(
      find.text('Also give it under the names June asked for'),
      findsOneWidget,
    );
    expect(find.text('AWS_KEY'), findsOneWidget);
    expect(find.text('Leave it out'), findsNWidgets(2));
    await tester.tap(find.text('Leave it out').first);
    await pumpFrames(tester);
    expect(find.text('Same as AWS_ACCESS_KEY_ID'), findsWidgets);
  });

  testWidgets('or as it is', (tester) async {
    final harness = Harness(dns());
    await _show(tester, harness, SecretRequestCard(cloudflare()));
    await tester.tap(
      find.byWidgetPredicate(
        (w) => w is Toggle && w.label == 'Also give it as CLOUDFLARE_API_TOKEN',
      ),
    );
    await pumpFrames(tester);
    expect(find.text('June gets it only as CF_CLAKTORY_DNS.'), findsOneWidget);
    await tester.tap(find.text('Give CF_CLAKTORY_DNS to June'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.aliases, isEmpty);
    expect(decided.asIs, isTrue);
  });

  testWidgets('a grant under an asked name says so', (tester) async {
    final harness = Harness(
      _workspace(
        secrets: [
          SecretInfo(
            id: 'cf',
            name: 'CF_CLAKTORY_DNS',
            variables: [SecretVariable(name: 'CF_CLAKTORY_DNS')],
            grants: [
              SecretGrant(
                agentId: 'june',
                aliases: {'CLOUDFLARE_API_TOKEN': 'CF_CLAKTORY_DNS'}.entries,
              ),
            ],
          ),
        ],
      ),
    );
    await _show(
      tester,
      harness,
      const Column(
        children: [
          SecretsSettings(),
          SizedBox(height: 400, child: AgentSecretsPanel(agentId: 'june')),
        ],
      ),
    );
    await tester.tap(_labeled('CF_CLAKTORY_DNS'));
    await pumpFrames(tester);
    expect(
      find.textContaining('also as CLOUDFLARE_API_TOKEN', findRichText: true),
      findsWidgets,
    );
    expect(
      find.text('CF_CLAKTORY_DNS, also as CLOUDFLARE_API_TOKEN'),
      findsOneWidget,
    );
  });

  test('a request in Needs you, in a line', () {
    final workspace = _workspace();
    final asked = resend();
    expect(secretRequestSummary(asked, workspace), 'Asks for Resend');
    final passed = resend()..adminAgentId = 'mike';
    expect(secretRequestSummary(passed, workspace), 'Needs a secret: Resend');
    passed.state = SecretRequestState.SECRET_REQUEST_STATE_APPROVED;
    expect(
      secretRequestSummary(passed, workspace),
      'Mike is finding it: Resend',
    );
    passed.collecting = true;
    expect(
      secretRequestSummary(passed, workspace, shownIn: 'mike'),
      'Asks you to add a secret for June: Resend',
    );
    expect(
      secretRequestSummary(passed, workspace, shownIn: 'june'),
      'Mike is finding it: Resend',
    );
  });

  group('with an admin agent to pass it to', () {
    SecretRequest approval() => resend()..adminAgentId = 'mike';

    testWidgets('the need is approved, not matched: Approve passes it on', (
      tester,
    ) async {
      final harness = Harness(_workspace());
      await _show(tester, harness, SecretRequestCard(approval()));
      expect(find.text('June needs a secret: Resend'), findsOneWidget);
      expect(find.text('Sets RESEND_API_KEY'), findsOneWidget);
      expect(
        find.text('Mike finds the right one, or asks you to add it.'),
        findsOneWidget,
      );
      // No matching on this card.
      expect(find.text('Your secrets'), findsNothing);
      expect(find.text('Paste the value'), findsNothing);
      await tester.tap(find.text('Approve'));
      await pumpFrames(tester);
      final decided = _sent(
        harness,
        (m) => m.hasDecideSecretRequest(),
      ).single.decideSecretRequest;
      expect(decided.approve, isTrue);
      expect(decided.approveOnly, isTrue);
      expect(decided.agentId, 'june');
      expect(decided.entries, isEmpty);
    });

    testWidgets('approved, it says who is on it and keeps Dismiss and the '
        'direct way', (tester) async {
      final harness = Harness(_workspace());
      await _show(
        tester,
        harness,
        SecretRequestCard(
          approval()
            ..state = SecretRequestState.SECRET_REQUEST_STATE_APPROVED
            ..approvedMs = Int64(1000),
        ),
      );
      expect(find.text('Approved. Mike is finding it.'), findsOneWidget);
      expect(find.text('Approve'), findsNothing);
      expect(find.text('Dismiss'), findsOneWidget);
      expect(find.text('Choose a secret…'), findsOneWidget);
    });

    testWidgets("Choose a secret… gives one directly, in a dialog that "
        'closes once given', (tester) async {
      final harness = Harness(_workspace());
      await _show(tester, harness, SecretRequestCard(approval()));
      await tester.tap(find.text('Choose a secret…'));
      await pumpFrames(tester);
      expect(find.text('Choose a secret for June'), findsOneWidget);
      expect(find.text('Your secrets'), findsOneWidget);
      await tester.tap(find.text('Your secrets'));
      await pumpFrames(tester);
      await tester.tap(find.text('Choose a secret'));
      await pumpFrames(tester);
      await tester.tap(find.text('AWS').last);
      await pumpFrames(tester);
      await tester.tap(find.text('Give AWS to June'));
      await pumpFrames(tester);
      final decided = _sent(
        harness,
        (m) => m.hasDecideSecretRequest(),
      ).single.decideSecretRequest;
      expect(decided.secretId, 's1');
      expect(decided.approveOnly, isFalse);
      expect(find.text('Choose a secret for June'), findsNothing);
    });

    testWidgets("in the admin agent's conversation, the secret it collects: "
        'added and given from here; the asker\'s card says where', (
      tester,
    ) async {
      final harness = Harness(_workspace());
      final collecting = approval()
        ..state = SecretRequestState.SECRET_REQUEST_STATE_APPROVED
        ..approvedMs = Int64(1000)
        ..collecting = true;
      await _show(
        tester,
        harness,
        Column(
          children: [
            SecretRequestCard(collecting, shownIn: 'june'),
            const SizedBox(height: 24),
            SecretRequestCard(collecting, shownIn: 'mike'),
          ],
        ),
      );
      expect(
        find.text("Waiting for you to add it in Mike's conversation."),
        findsOneWidget,
      );
      expect(
        find.text('Mike asks you to add a secret for June: Resend'),
        findsOneWidget,
      );
      // Only the new secret, filled in.
      expect(find.text('Your secrets'), findsNothing);
      await tester.enterText(field('Paste the value'), 're_123');
      await tester.tap(find.text('Add and give to June'));
      await pumpFrames(tester);
      final decided = _sent(
        harness,
        (m) => m.hasDecideSecretRequest(),
      ).single.decideSecretRequest;
      expect(decided.agentId, 'june');
      expect(decided.requestId, 'r1');
      expect(decided.approveOnly, isFalse);
      expect(decided.entries.single.variable, 'RESEND_API_KEY');
      expect(decided.entries.single.value, 're_123');
    });

    testWidgets('given after approval, it says what the agent got', (
      tester,
    ) async {
      final harness = Harness(_workspace());
      await _show(
        tester,
        harness,
        SecretRequestCard(
          approval()
            ..state = SecretRequestState.SECRET_REQUEST_STATE_GRANTED
            ..approvedMs = Int64(1000)
            ..givenSecretName = 'Resend mail'
            ..decidedMs = Int64(2000),
        ),
      );
      expect(
        find.textContaining('June got Resend mail · ', findRichText: true),
        findsOneWidget,
      );
    });
  });

  testWidgets('a request the agent withdrew says so and takes no answer', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await _show(
      tester,
      harness,
      SecretRequestCard(
        resend()
          ..state = SecretRequestState.SECRET_REQUEST_STATE_WITHDRAWN
          ..comment = 'Found another way.'
          ..decidedMs = Int64(
            DateTime(2026, 11, 20, 14, 2).millisecondsSinceEpoch,
          ),
      ),
    );
    expect(
      find.textContaining('June withdrew it · ', findRichText: true),
      findsOneWidget,
    );
    expect(find.text('“Found another way.”'), findsOneWidget);
    expect(find.text('Dismiss'), findsNothing);
    expect(find.text('Your secrets'), findsNothing);
  });

  testWidgets('a request the agent updated says when, and its form follows', (
    tester,
  ) async {
    final harness = Harness(_workspace(secrets: []));
    final shown = ValueNotifier(resend());
    await _show(
      tester,
      harness,
      ValueListenableBuilder(
        valueListenable: shown,
        builder: (context, request, _) => SecretRequestCard(request),
      ),
    );
    expect(find.textContaining('Updated'), findsNothing);
    shown.value = resend()
      ..updatedMs = Int64(DateTime(2026, 11, 20, 14, 5).millisecondsSinceEpoch)
      ..newSecret.name = 'Resend production'
      ..newSecret.variables.add(SecretVariable(name: 'RESEND_DOMAIN'));
    await pumpFrames(tester);
    expect(find.textContaining('Updated '), findsOneWidget);
    expect(find.textContaining('14:05'), findsOneWidget);
    expect(find.text('RESEND_DOMAIN'), findsOneWidget);
    expect(
      tester
          .widgetList<EditableText>(find.byType(EditableText))
          .map((field) => field.controller.text),
      contains('Resend production'),
    );
  });

  testWidgets('Your secrets with nothing chosen says what to do', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, SecretRequestCard(resend()));
    await tester.tap(find.text('Your secrets'));
    await tester.pump();
    expect(find.text('Choose a secret'), findsOneWidget);
    await tester.tap(find.text('Give'));
    await tester.pump();
    expect(find.text('Choose a secret, or add a new one.'), findsOneWidget);
    expect(_sent(harness, (m) => m.hasDecideSecretRequest()), isEmpty);
    await tester.tap(find.text('Choose a secret'));
    await pumpFrames(tester);
    await tester.tap(find.text('AWS').last);
    await pumpFrames(tester);
    await tester.tap(find.text('Give AWS to June'));
    await pumpFrames(tester);
    expect(
      _sent(
        harness,
        (m) => m.hasDecideSecretRequest(),
      ).single.decideSecretRequest.secretId,
      's1',
    );
  });

  testWidgets("no secrets: Your secrets says so, and it starts on New secret", (
    tester,
  ) async {
    final harness = Harness(_workspace(secrets: []));
    await _show(tester, harness, SecretRequestCard(resend()));
    expect(find.text('Add and give to June'), findsOneWidget);
    await tester.tap(find.text('Your secrets'));
    await tester.pump();
    expect(
      find.text("You don't have any secrets yet. Add one under New secret."),
      findsOneWidget,
    );
  });

  testWidgets('a note goes to the agent with the secret given', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, SecretRequestCard(resend()));
    await tester.enterText(field('Paste the value'), 're_123456789');
    await tester.tap(find.text('Add a comment'));
    await tester.pump();
    await tester.enterText(field(_comment), 'Check the sender domain first.');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await pumpFrames(tester);
    expect(
      _sent(harness, (m) => m.hasDecideSecretRequest()),
      isEmpty,
      reason: 'Enter in the note decides nothing',
    );
    await tester.tap(find.text('Add and give to June'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.approve, isTrue);
    expect(decided.comment, 'Check the sender domain first.');
  });

  SecretRequest sshKey({bool nobody = false}) => SecretRequest(
    id: 'r2',
    agentId: 'june',
    adminAgentId: 'mike',
    generate: 'ssh-ed25519',
    grantToNobody: nobody,
    newSecret: NewSecret(
      name: 'deploy-key',
      variables: [SecretVariable(name: 'SSH_KEY', file: true)],
    ),
    reason: 'To push to the deploy server.',
    state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
  );

  testWidgets('a key to generate: its name, whom it is for, Ask each time '
      'off, Generate', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, SecretRequestCard(sshKey()));
    expect(find.text('June asks for a new SSH key'), findsOneWidget);
    expect(find.text('To push to the deploy server.'), findsOneWidget);
    expect(find.text("Also the key's comment"), findsOneWidget);
    expect(find.text('For June'), findsOneWidget);
    // Never a values form, a choice of secrets or the admin agent.
    for (final gone in [
      'Your secrets',
      'New secret',
      'Paste the value',
      'Approve',
      'Choose a secret…',
    ]) {
      expect(find.text(gone), findsNothing, reason: gone);
    }
    expect(
      tester
          .widget<Toggle>(
            find.byWidgetPredicate(
              (w) => w is Toggle && w.label == askEachTimeLabel,
            ),
          )
          .value,
      isFalse,
    );

    final name = find.descendant(
      of: find.byWidgetPredicate(
        (w) => w is TextField && w.semanticsLabel == 'Name',
      ),
      matching: find.byType(EditableText),
    );
    expect(tester.widget<EditableText>(name).controller.text, 'deploy-key');
    await tester.enterText(name, 'deploy-key-2');
    await tester.tap(_labeled(askEachTimeLabel));
    await tester.tap(find.text('Add a comment'));
    await tester.pump();
    await tester.enterText(field(_comment), 'Only main.');
    await tester.tap(find.text('Generate'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.approve, isTrue);
    expect(decided.name, 'deploy-key-2');
    expect(decided.askEachTime, isTrue, reason: 'turned on in the card');
    expect(decided.comment, 'Only main.');
    expect(decided.entries, isEmpty);
    expect(decided.approveOnly, isFalse);
  });

  testWidgets('a key the agent asked to go to no one says so, and Ask each '
      'time left off is sent off', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, SecretRequestCard(sshKey(nobody: true)));
    expect(find.text('For no one'), findsOneWidget);
    expect(find.text('For June'), findsNothing);
    await tester.tap(find.text('Generate'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.askEachTime, isFalse);
    expect(decided.hasAskEachTime(), isTrue);
  });

  testWidgets('a generated key shows to whom it went and its public key, '
      'with Copy', (tester) async {
    final copied = <String>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          copied.add((call.arguments as Map)['text'] as String);
        }
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    const line = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample deploy-key';
    final harness = Harness(_workspace());
    await _show(
      tester,
      harness,
      SecretRequestCard(
        sshKey()
          ..state = SecretRequestState.SECRET_REQUEST_STATE_GRANTED
          ..approvedMs = Int64(1)
          ..givenSecretName = 'deploy-key'
          ..publicKey = line,
      ),
    );
    expect(
      find.textContaining('Given to June', findRichText: true),
      findsOneWidget,
    );
    expect(find.text(line), findsOneWidget);
    expect(
      find.text('Public key: safe to share, such as with GitHub or a server.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Copy'));
    await tester.pump();
    expect(copied, [line]);
    expect(find.text('Copied'), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
    await _show(
      tester,
      harness,
      SecretRequestCard(
        sshKey(nobody: true)
          ..state = SecretRequestState.SECRET_REQUEST_STATE_GENERATED
          ..publicKey = line,
      ),
    );
    expect(
      find.textContaining('Given to no one', findRichText: true),
      findsOneWidget,
    );
    expect(find.text(line), findsOneWidget);
  });

  test('a key to generate, in Needs you', () {
    expect(
      secretRequestSummary(sshKey(), _workspace()),
      'Asks for a new SSH key',
    );
  });

  testWidgets('Dismiss tells the agent why, and the card shows it', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    final request = resend();
    await _show(tester, harness, SecretRequestCard(request));
    await tester.tap(find.text('Add a comment'));
    await tester.pump();
    await tester.enterText(field(_comment), 'Use the test account.');
    await tester.tap(find.text('Dismiss'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.approve, isFalse);
    expect(decided.comment, 'Use the test account.');

    await tester.pumpWidget(const SizedBox());
    await _show(
      tester,
      harness,
      SecretRequestCard(
        request.deepCopy()
          ..state = SecretRequestState.SECRET_REQUEST_STATE_DISMISSED
          ..comment = 'Use the test account.'
          ..decidedMs = Int64(
            DateTime(2026, 11, 20, 14, 2).millisecondsSinceEpoch,
          ),
      ),
    );
    expect(
      find.textContaining('Dismissed · ', findRichText: true),
      findsOneWidget,
    );
    expect(find.text('“Use the test account.”'), findsOneWidget);
  });

  testWidgets("a request's description shows its links as conversations do, "
      'a long address as its host', (tester) async {
    const page = 'https://dash.cloudflare.com/profile/api-tokens';
    final harness = Harness(_workspace(secrets: []));
    await _show(
      tester,
      harness,
      SecretRequestCard(
        resend()
          ..newSecret.description =
              'A token from $page, see [the guide](https://a.test/guide).',
      ),
    );
    final shown = [
      for (final text in tester.widgetList<RichText>(find.byType(RichText)))
        text.text.toPlainText(),
    ];
    expect(
      shown,
      contains('A token from dash.cloudflare.com/…, see the guide.'),
    );
    expect(shown.where((text) => text.contains(page)), isEmpty);
  });

  testWidgets('an expired request says so and takes no answer', (tester) async {
    final harness = Harness(_workspace());
    await _show(
      tester,
      harness,
      SecretRequestCard(
        resend()
          ..state = SecretRequestState.SECRET_REQUEST_STATE_EXPIRED
          ..decidedMs = Int64(
            DateTime(2026, 11, 20, 14, 2).millisecondsSinceEpoch,
          ),
      ),
    );
    expect(
      find.textContaining('Expired · ', findRichText: true),
      findsOneWidget,
    );
    expect(find.text('Dismiss'), findsNothing);
    expect(find.textContaining('Give'), findsNothing);
    expect(find.text('Your secrets'), findsNothing);
  });

  testWidgets('a decision on a request that just expired shows why', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, SecretRequestCard(resend()));
    harness.backend.refuse = 'This request expired.';
    await tester.tap(find.text('Dismiss'));
    await pumpFrames(tester);
    expect(find.text('This request expired.'), findsOneWidget);
  });

  testWidgets("an old request for a secret there is: Give AWS to June", (
    tester,
  ) async {
    final harness = Harness(_workspace());
    final request = SecretRequest(
      id: 'r2',
      agentId: 'june',
      secretId: 's1',
      secretName: 'AWS',
      reason: 'To deploy.',
      state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
    );
    await _show(tester, harness, SecretRequestCard(request));
    expect(find.text('June asks for AWS'), findsOneWidget);
    expect(find.text('Your secrets'), findsNothing);
    await tester.tap(find.text('Give AWS to June'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.approve, isTrue);
    expect(decided.entries, isEmpty);
    expect(decided.secretId, isEmpty);
  });

  testWidgets("requests waiting don't show in Settings → Secrets, only in "
      'their conversations', (tester) async {
    final workspace = _workspace();
    workspace.vault.requests.add(
      SecretRequest(
        id: 'r3',
        agentId: 'june',
        secretId: 's1',
        secretName: 'AWS',
        reason: 'To deploy the site.',
        state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
      ),
    );
    final harness = Harness(workspace);
    await _show(tester, harness, const SecretsSettings());
    expect(find.text('Add a secret…'), findsOneWidget);
    expect(find.text('June asks for AWS'), findsNothing);
  });

  testWidgets('Delete… deletes once confirmed', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(_labeled('AWS'));
    await pumpFrames(tester);
    await tester.tap(find.text('Delete…'));
    await pumpFrames(tester);
    expect(find.text('Delete AWS?'), findsOneWidget);
    await tester.tap(find.text('Delete').last);
    await pumpFrames(tester);
    expect(
      _sent(harness, (m) => m.hasDeleteSecret()).single.deleteSecret.id,
      's1',
    );
    expect(find.text('Delete AWS?'), findsNothing);
  });

  testWidgets('locked after a restart: why, and the recovery key opens them', (
    tester,
  ) async {
    final workspace = _workspace();
    workspace.vault.unlocked = false;
    final harness = Harness(workspace);
    await _show(tester, harness, const SecretsSettings());
    expect(
      find.text(
        "Secrets are locked. Agents can't use them until you unlock them.",
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Unlock with recovery key…'));
    await pumpFrames(tester);
    expect(find.text('Unlock with recovery key'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, 'nope');
    await tester.tap(find.text('Unlock'));
    await pumpFrames(tester);
    // The backend's words, as it sends them.
    expect(find.text("That passphrase isn't right."), findsOneWidget);
    await tester.enterText(
      find.byType(EditableText).last,
      harness.backend.unlockKey,
    );
    await tester.tap(find.text('Unlock'));
    await pumpFrames(tester);
    expect(find.text('Unlock with recovery key'), findsNothing);
    final sent = _sent(harness, (m) => m.hasUnlockVault());
    expect(sent.last.unlockVault.passphrase, harness.backend.unlockKey);
  });

  testWidgets("locked: the backend's own words for why, in place of the "
      "account's", (tester) async {
    final workspace = _workspace()
      ..account = ClaktoryAccount(
        state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN,
      );
    workspace.vault
      ..unlocked = false
      ..lockedWhy = "Claktory is getting your secrets' key from claktory.com.";
    final harness = Harness(workspace);
    await _show(tester, harness, const SecretsSettings());
    expect(
      find.text(
        "Secrets are locked. Claktory is getting your secrets' key from "
        'claktory.com.',
      ),
      findsOneWidget,
    );
    expect(find.text('Unlock with recovery key…'), findsOneWidget);
  });

  testWidgets("with a Claktory account: the backend's reason, or signing in "
      'again when this Claktory was removed', (tester) async {
    final workspace = _workspace()
      ..account = ClaktoryAccount(
        state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN,
        problem:
            "Can't reach claktory.com. Claktory keeps working and tries "
            'again on its own.',
      );
    workspace.vault.unlocked = false;
    final harness = Harness(workspace);
    await _show(tester, harness, const SecretsSettings());
    expect(
      find.text(
        "Secrets are locked. Can't reach claktory.com. Claktory keeps "
        'working and tries again on its own.',
      ),
      findsOneWidget,
    );
    expect(find.text('Unlock with recovery key…'), findsOneWidget);
    expect(find.text('Sign in again…'), findsNothing);

    final removed = harness.backend.workspace.workspace.deepCopy()
      ..account.state = ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_REVOKED;
    harness.backend.workspace.apply(removed);
    await pumpFrames(tester);
    expect(find.textContaining('removed from your account'), findsOneWidget);
    expect(find.text('Use recovery key…'), findsOneWidget);
    signInPutOff(harness.backend).value = true;
    await tester.tap(find.text('Sign in again…'));
    await tester.pump();
    expect(signInPutOff(harness.backend).value, isFalse);

    // Opened (by itself, or with the key): no more banner.
    harness.backend.workspace.apply(
      harness.backend.workspace.workspace.deepCopy()..vault.unlocked = true,
    );
    await pumpFrames(tester);
    expect(find.textContaining('Secrets are locked'), findsNothing);
  });

  test('a refusal keeps its code', () {
    const failure = RequestFailure(
      'no',
      code: ErrorCode.ERROR_CODE_NOT_GRANTED,
    );
    expect(failure.code, ErrorCode.ERROR_CODE_NOT_GRANTED);
  });
}

/// The comment's placeholder, once Add a comment is tapped.
const _comment = 'A comment for June, if you like';
