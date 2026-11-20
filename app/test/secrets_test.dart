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

  testWidgets("a request like none of the person's secrets: Add new, filled "
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
    expect(find.text('Tell June why (optional)'), findsNothing);
    expect(
      find.textContaining('Gave Resend mail to June · ', findRichText: true),
      findsOneWidget,
    );
  });

  testWidgets('a request like one of them: Choose one, picked, Give it', (
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

  testWidgets('Choose one with nothing chosen says what to do', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, SecretRequestCard(resend()));
    await tester.tap(find.text('Choose one'));
    await tester.pump();
    expect(find.text('Choose a secret'), findsOneWidget);
    await tester.tap(find.text('Give'));
    await tester.pump();
    expect(
      find.text('Choose a secret to give, or add a new one.'),
      findsOneWidget,
    );
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

  testWidgets("no secrets: Choose one says so, and it starts on Add new", (
    tester,
  ) async {
    final harness = Harness(_workspace(secrets: []));
    await _show(tester, harness, SecretRequestCard(resend()));
    expect(find.text('Add and give to June'), findsOneWidget);
    await tester.tap(find.text('Choose one'));
    await tester.pump();
    expect(find.text("You don't have any secrets yet."), findsOneWidget);
  });

  testWidgets('Dismiss tells the agent why, and the card shows it', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    final request = resend();
    await _show(tester, harness, SecretRequestCard(request));
    await tester.enterText(
      field('Tell June why (optional)'),
      'Use the test account.',
    );
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
    expect(find.text('Choose one'), findsNothing);
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
    expect(find.text('Choose one'), findsNothing);
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
