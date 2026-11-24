import 'package:factory_app/account.dart' show signInPutOff;
import 'package:factory_app/backend.dart';
import 'package:factory_app/controls.dart' show TextField, Toggle;
import 'package:factory_app/loopback_forward.dart' show linkLauncher;
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

  testWidgets("what reorders a request's words shows by its code", (
    tester,
  ) async {
    final harness = Harness(_workspace());
    final request = resend()
      ..newSecret.name = 'Re\u202Esend'
      ..newSecret.description = 'Mail \u2066x\u2069'
      ..reason = 'For evil\u202Etxt.exe';
    await _show(tester, harness, SecretRequestCard(request));
    expect(
      find.text(r'June asks for a secret: Re\u{202E}send'),
      findsOneWidget,
    );
    expect(
      find.textContaining(r'For evil\u{202E}txt.exe', findRichText: true),
      findsOneWidget,
    );
    expect(
      find.textContaining(r'Mail \u{2066}x\u{2069}', findRichText: true),
      findsOneWidget,
    );
  });

  testWidgets("a request like none of the person's secrets: the card is the "
      'form, filled in from it (#360 K2)', (tester) async {
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
    await tester.enterText(field('Paste it here'), 're_123456789');
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
          ..added = true
          ..decidedMs = Int64(
            DateTime(2026, 11, 20, 14, 2).millisecondsSinceEpoch,
          ),
      ),
    );
    expect(find.text('Add and give to June'), findsNothing);
    expect(find.text('Add a comment'), findsNothing);
    expect(
      find.textContaining('Added and given to June · ', findRichText: true),
      findsOneWidget,
    );
  });

  testWidgets("a request's fields keep their names for a screen reader once "
      'typed in: the labels over them may have scrolled away', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = Harness(_workspace());
    await _show(tester, harness, SecretRequestCard(resend()));
    await tester.enterText(field('Paste it here'), 're_123456789');
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

  testWidgets('a secret that fits, as Claktory found it: Give it (#360 K4)', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    final request = resend()
      ..newSecret.name = 'AWS deploy'
      ..secretName = 'AWS deploy'
      ..fittingSecretId = 's1'
      ..fittingSecretName = 'AWS';
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
    fittingSecretId: 'cf',
    fittingSecretName: 'CF_CLAKTORY_DNS',
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
          ..fittingSecretId = 'aws'
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
    // Handed back to the person (its admin /cleared or deleted): approved
    // with no admin, it asks them again, as its card does (#327).
    final handedBack = resend()
      ..state = SecretRequestState.SECRET_REQUEST_STATE_APPROVED;
    expect(secretRequestSummary(handedBack, workspace), 'Asks for Resend');
  });

  group('with an admin agent to pass it to', () {
    SecretRequest approval() => resend()..adminAgentId = 'mike';

    testWidgets('nothing fits: the form, no Approve; Let Mike choose is under '
        'Give one of yours… (#360 K2, K4)', (tester) async {
      final harness = Harness(_workspace());
      await _show(tester, harness, SecretRequestCard(approval()));
      expect(find.text('June needs a secret: Resend'), findsOneWidget);
      expect(find.text('Approve'), findsNothing);
      expect(find.text('Your secrets'), findsNothing);
      expect(find.text('Add and give to June'), findsOneWidget);
      expect(field('Paste it here'), findsOneWidget);
      await tester.tap(find.text('Give one of yours…'));
      await pumpFrames(tester);
      expect(find.text('Add a new one'), findsNothing, reason: 'it is one');
      await tester.tap(find.text('Let Mike choose'));
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

    testWidgets('one fits: Give it; Give another… for the others, a new one '
        'or Mike\'s choice', (tester) async {
      final harness = Harness(
        _workspace(
          secrets: [
            _aws(),
            SecretInfo(
              id: 's2',
              name: 'Resend old',
              variables: [SecretVariable(name: 'RESEND_API_KEY')],
            ),
          ],
        ),
      );
      await _show(
        tester,
        harness,
        SecretRequestCard(
          approval()
            ..fittingSecretId = 's2'
            ..fittingSecretName = 'Resend old',
        ),
      );
      expect(find.text('Give Resend old to June'), findsOneWidget);
      expect(find.text('Sets RESEND_API_KEY'), findsOneWidget);
      expect(field('Paste it here'), findsNothing);
      await tester.tap(find.text('Give another…'));
      await pumpFrames(tester);
      expect(find.text('Let Mike choose'), findsOneWidget);
      expect(find.text('Resend old'), findsNothing, reason: 'on the button');
      await tester.tap(find.text('Add a new one'));
      await pumpFrames(tester);
      expect(find.text('Add and give to June'), findsOneWidget);
      await tester.tap(find.text('Give one of yours…'));
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
    });

    testWidgets('a connected service that can make it: Approve, and Mike makes '
        'one, as before', (tester) async {
      final harness = Harness(
        _workspace(
          secrets: [
            SecretInfo(
              id: 'cfc',
              name: 'Cloudflare',
              connection: SecretConnection(connectorId: 'cloudflare'),
            ),
          ],
        ),
      );
      await _show(
        tester,
        harness,
        SecretRequestCard(
          approval()
            ..fittingSecretId = 'cfc'
            ..fittingSecretName = 'Cloudflare',
        ),
      );
      expect(
        find.text('Mike finds the right one, or asks you to add it.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Approve'));
      await pumpFrames(tester);
      expect(
        _sent(
          harness,
          (m) => m.hasDecideSecretRequest(),
        ).single.decideSecretRequest.approveOnly,
        isTrue,
      );
    });

    for (final (name, admin, line) in [
      (
        'idle',
        Agent(id: 'mike', name: 'Mike'),
        'Approved. Mike is finding it.',
      ),
      (
        'working',
        Agent(
          id: 'mike',
          name: 'Mike',
          status: AgentStatus.AGENT_STATUS_WORKING,
        ),
        "Mike will do this when it finishes what it's doing.",
      ),
      (
        'waiting for usage',
        Agent(
          id: 'mike',
          name: 'Mike',
          status: AgentStatus.AGENT_STATUS_WORKING,
          usageWait: 'Waiting for your Claude usage to reset',
        ),
        'Mike is waiting for your Claude usage to reset.',
      ),
    ]) {
      testWidgets('approved, with Mike $name: says so, and Do it here opens '
          'the form (#360 K5)', (tester) async {
        final workspace = _workspace();
        workspace.agents[1] = admin;
        final harness = Harness(workspace);
        await _show(
          tester,
          harness,
          SecretRequestCard(
            approval()
              ..state = SecretRequestState.SECRET_REQUEST_STATE_APPROVED
              ..approvedMs = Int64(1000),
          ),
        );
        expect(find.text(line), findsOneWidget);
        expect(find.text('Approve'), findsNothing);
        expect(find.text('Dismiss'), findsOneWidget);
        expect(field('Paste it here'), findsNothing);
        await tester.tap(find.text('Do it here'));
        await pumpFrames(tester);
        await tester.enterText(field('Paste it here'), 're_1');
        await tester.tap(find.text('Add and give to June'));
        await pumpFrames(tester);
        final decided = _sent(
          harness,
          (m) => m.hasDecideSecretRequest(),
        ).single.decideSecretRequest;
        expect(decided.approveOnly, isFalse);
        expect(decided.entries.single.value, 're_1');
      });
    }

    testWidgets("the agent's link: Get it on its host, its steps quoted as "
        "the agent's; never a link that isn't https (#360 K3)", (tester) async {
      final opened = <Uri>[];
      linkLauncher = (url) async {
        opened.add(url);
        return true;
      };
      addTearDown(() => linkLauncher = null);
      final harness = Harness(_workspace(secrets: []));
      const link =
          'https://github.com/settings/tokens/new?scopes=repo&description=June';
      await _show(
        tester,
        harness,
        SecretRequestCard(
          approval()
            ..link = link
            ..linkHost = 'github.com'
            ..steps = 'Tick repo, then Generate.',
        ),
      );
      expect(find.text('June: “Tick repo, then Generate.”'), findsOneWidget);
      await tester.tap(find.text('Get it on github.com'));
      await pumpFrames(tester);
      expect(opened, [Uri.parse(link)]);

      await tester.pumpWidget(const SizedBox());
      await _show(
        tester,
        harness,
        SecretRequestCard(
          approval()
            ..link = 'http://github.com/settings/tokens/new'
            ..linkHost = 'github.com',
        ),
      );
      expect(find.textContaining('Get it on'), findsNothing);
    });

    test("a link goes only where its button says (Fugo)", () {
      bool opens(String link, String host) =>
          opensLink(SecretRequest(link: link, linkHost: host));
      expect(
        opens('https://github.com/settings/tokens/new', 'github.com'),
        isTrue,
      );
      expect(
        opens('http://github.com/settings/tokens/new', 'github.com'),
        isFalse,
      );
      expect(opens('https://evil.example/github.com', 'github.com'), isFalse);
      expect(opens('https://github.com/x', ''), isFalse);
      expect(opens('', 'github.com'), isFalse);
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

  testWidgets('no secrets: the form, and nothing else to give', (tester) async {
    final harness = Harness(_workspace(secrets: []));
    await _show(tester, harness, SecretRequestCard(resend()));
    expect(find.text('Add and give to June'), findsOneWidget);
    expect(find.text('Give one of yours…'), findsNothing);
    expect(find.text('Your secrets'), findsNothing);
  });

  testWidgets('a typed comment: one Submit, a reply that gives nothing '
      '(#370)', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, SecretRequestCard(resend()));
    await tester.enterText(field('Paste it here'), 're_123456789');
    await tester.tap(find.text('Add a comment'));
    await tester.pump();
    await tester.enterText(field(_comment), ' Use the test account. ');
    await pumpFrames(tester);
    expect(find.text('Add and give to June'), findsNothing);
    expect(find.text('Give one of yours…'), findsNothing);
    expect(find.text('Dismiss'), findsNothing);
    expect(
      find.text('Nothing is given. June gets your comment and can ask again.'),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Submit'));
    await tester.tap(find.text('Submit'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.reply, isTrue);
    // An older backend that ignores reply dismisses it.
    expect(decided.approve, isFalse);
    expect(decided.entries, isEmpty);
    expect(decided.comment, 'Use the test account.');
  });

  testWidgets('Return in the comment is Submit, never Give', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, SecretRequestCard(resend()));
    await tester.enterText(field('Paste it here'), 're_123456789');
    await tester.tap(find.text('Add a comment'));
    await tester.pump();
    await tester.enterText(field(_comment), 'Check the sender domain first.');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.reply, isTrue);
    expect(decided.approve, isFalse);
    expect(decided.entries, isEmpty);
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
    // A typed comment hides Generate (#370): Submit replies instead.
    await tester.tap(find.text('Add a comment'));
    await tester.pump();
    await tester.enterText(field(_comment), 'Only main.');
    await tester.pump();
    expect(find.text('Generate'), findsNothing);
    await tester.enterText(field(_comment), '');
    await tester.pump();
    await tester.tap(find.text('Generate'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.approve, isTrue);
    expect(decided.reply, isFalse);
    expect(decided.name, 'deploy-key-2');
    expect(decided.askEachTime, isTrue, reason: 'turned on in the card');
    expect(decided.comment, isEmpty);
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

  SecretRequest password({
    String allowed = '',
    int maxLength = 0,
    int longest = 64,
  }) => SecretRequest(
    id: 'r3',
    agentId: 'june',
    generate: 'password',
    newSecret: NewSecret(
      name: 'smtp-password',
      variables: [SecretVariable(name: 'PASSWORD')],
    ),
    password: PasswordSpec(
      length: 32,
      lowercase: true,
      uppercase: true,
      digits: true,
      symbols: allowed.isEmpty,
      allowed: allowed,
      requireEach: true,
      maxLength: maxLength,
      alphabetSize: 90,
      bits: 207.7,
      shortest: 16,
      longest: longest,
      bitsPerCharacter: 6.49,
    ),
    reason: 'To send mail.',
    state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
  );

  test('strength in a word', () {
    expect(passwordStrength(72), 'fair');
    expect(passwordStrength(96), 'strong');
    expect(passwordStrength(128), 'very strong');
    expect(passwordStrength(60), 'weak');
  });

  testWidgets('a password to generate says what it is and how strong, and '
      'its length moves between the bounds', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, SecretRequestCard(password(longest: 33)));
    expect(find.text('June asks for a new password'), findsOneWidget);
    expect(
      find.text('A 32-character password: letters, digits and symbols'),
      findsOneWidget,
    );
    expect(find.text('Strength: very strong'), findsOneWidget);
    expect(find.text("Also the key's comment"), findsNothing);
    expect(find.text('No agent ever sees it, June included.'), findsOneWidget);

    await tester.tap(find.bySemanticsLabel('Longer'));
    await tester.pump();
    expect(
      find.text('A 33-character password: letters, digits and symbols'),
      findsOneWidget,
    );
    expect(find.text('Strength: very strong'), findsOneWidget);
    // Not past the longest.
    await tester.tap(find.bySemanticsLabel('Longer'));
    await tester.pump();
    expect(find.textContaining('33-character'), findsOneWidget);
    for (var i = 0; i < 20; i++) {
      await tester.tap(find.bySemanticsLabel('Shorter'));
      await tester.pump();
    }
    // Nor below the shortest.
    expect(find.textContaining('16-character'), findsOneWidget);
    expect(find.text('Strength: strong'), findsOneWidget);

    await tester.tap(find.text('Generate'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.approve, isTrue);
    expect(decided.name, 'smtp-password');
    expect(decided.passwordLength, 16);
    expect(decided.entries, isEmpty);
  });

  testWidgets('a password left as asked sends no length, and says what the '
      'service limits', (tester) async {
    final harness = Harness(_workspace());
    await _show(
      tester,
      harness,
      SecretRequestCard(
        password(allowed: 'abcXYZ123', maxLength: 40, longest: 40),
      ),
    );
    expect(find.text('Only these characters: abcXYZ123'), findsOneWidget);
    expect(
      find.text('The service accepts at most 40 characters.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Generate'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.passwordLength, 0);
  });

  testWidgets('a length chosen in the conversation stays when the card '
      'scrolls out of view and back (row 189)', (tester) async {
    final harness = Harness(_workspace());
    Entry said(int seq) => Entry(
      seq: Int64(seq),
      user: UserText(text: 'message $seq'),
    );
    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          for (var i = 0; i < 40; i++) said(i),
          Entry(seq: Int64(40), secretRequest: password()),
          for (var i = 41; i < 80; i++) said(i),
        ],
      ),
    );
    await harness.show(tester);
    await pumpFrames(tester);
    final conversation = find.byType(CustomScrollView).first;
    final shorter = find.bySemanticsLabel('Shorter');
    await tester.dragUntilVisible(shorter, conversation, const Offset(0, 300));
    await tester.pump();
    await tester.tap(shorter);
    await tester.pump();
    await tester.tap(shorter);
    await tester.pump();
    expect(find.textContaining('30-character'), findsOneWidget);

    // Up past it, then back down to the newest.
    for (var i = 0; i < 3; i++) {
      await tester.drag(conversation, const Offset(0, 3000));
      await tester.pump();
    }
    expect(find.textContaining('-character password'), findsNothing);
    for (var i = 0; i < 6; i++) {
      await tester.drag(conversation, const Offset(0, -3000));
      await tester.pump();
    }
    expect(find.text('message 79'), findsOneWidget);
    expect(find.textContaining('-character password'), findsNothing);

    await tester.dragUntilVisible(
      find.text('Generate'),
      conversation,
      const Offset(0, 300),
    );
    await tester.pump();
    expect(find.textContaining('30-character'), findsOneWidget);
    await tester.tap(find.text('Generate'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.passwordLength, 30);
  });

  testWidgets('a generated key: to whom it went, then where to add it, with '
      'Copy key, the link and Done (#360 K6)', (tester) async {
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
    expect(find.text('Next, add it where June will use it'), findsOneWidget);
    expect(
      find.text("June can't use the key until it's added there."),
      findsOneWidget,
    );
    expect(find.textContaining('Open '), findsNothing, reason: 'no link');
    await tester.tap(find.text('Copy key'));
    await tester.pump();
    expect(copied, [line]);
    expect(find.text('Copied'), findsOneWidget);
    await tester.tap(find.text('Done'));
    await pumpFrames(tester);
    expect(
      _sent(harness, (m) => m.hasSecretRequestDone()).single.secretRequestDone,
      SecretRequestDone(agentId: 'june', requestId: 'r2'),
    );

    // With where it goes and a link; then done.
    await tester.pumpWidget(const SizedBox());
    await _show(
      tester,
      harness,
      SecretRequestCard(
        sshKey()
          ..state = SecretRequestState.SECRET_REQUEST_STATE_GRANTED
          ..publicKey = line
          ..place = 'claktory/factory'
          ..link = 'https://github.com/claktory/factory/settings/keys/new'
          ..linkHost = 'github.com',
      ),
    );
    expect(
      find.text("Next, add it to claktory/factory's deploy keys"),
      findsOneWidget,
    );
    expect(find.text('Open github.com'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
    await _show(
      tester,
      harness,
      SecretRequestCard(
        sshKey()
          ..state = SecretRequestState.SECRET_REQUEST_STATE_GRANTED
          ..publicKey = line
          ..doneMs = Int64(5000),
      ),
    );
    expect(find.text('Done. June knows the key is in place.'), findsOneWidget);
    expect(find.text('Next, add it where June will use it'), findsNothing);
    expect(
      find.text('Public key: safe to share, such as with GitHub or a server.'),
      findsOneWidget,
    );

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

  group('files to import (vault import)', () {
    SecretRequest importing({
      bool tainted = false,
      String path = '/Users/me/keys/release.p12',
      String sensitive = '',
      String problem = '',
      SecretRequestState state =
          SecretRequestState.SECRET_REQUEST_STATE_PENDING,
    }) => SecretRequest(
      id: 'r3',
      agentId: 'june',
      adminAgentId: 'mike',
      newSecret: NewSecret(
        name: 'android-release',
        variables: [
          SecretVariable(name: 'KEYSTORE', file: true),
          SecretVariable(name: 'KEY_PROPERTIES', file: true),
        ],
      ),
      reason: 'To sign the Android release.',
      state: state,
      askerReadPipes: tainted,
      imports: [
        SecretImport(
          variable: 'KEYSTORE',
          path: path,
          size: Int64(4096),
          modifiedMs: Int64(DateTime(2026, 11, 20, 9).millisecondsSinceEpoch),
          fingerprint: '3f9a 21c0 7d44',
          sensitive: sensitive,
          problem: problem,
        ),
        SecretImport(
          variable: 'KEY_PROPERTIES',
          path: '/Users/me/keys/key.properties',
          size: Int64(120),
          modifiedMs: Int64(DateTime(2026, 11, 20, 9).millisecondsSinceEpoch),
          fingerprint: '88b2 0c1d e4f7',
        ),
      ],
    );

    Toggle toggle(WidgetTester tester, String label) => tester.widget<Toggle>(
      find.byWidgetPredicate((w) => w is Toggle && w.label == label),
    );

    testWidgets('each file in full, nothing deleted unless turned on, then '
        'Import only (#360 K7)', (tester) async {
      final harness = Harness(_workspace());
      await _show(tester, harness, SecretRequestCard(importing()));
      expect(find.text('June asks to import 2 files'), findsOneWidget);
      expect(find.text('/Users/me/keys/release.p12'), findsOneWidget);
      expect(find.text('/Users/me/keys/key.properties'), findsOneWidget);
      expect(find.text('KEYSTORE (file)'), findsOneWidget);
      expect(find.textContaining('4.1 KB'), findsOneWidget);
      expect(find.textContaining('fingerprint 3f9a 21c0 7d44'), findsOneWidget);
      expect(
        find.text('Files are stored exactly as they are, binary files too.'),
        findsOneWidget,
      );
      // Never a values form, a choice of secrets or the admin agent.
      for (final gone in [
        'Your secrets',
        'New secret',
        'Paste the file’s contents',
        'Approve',
        'Choose a secret…',
      ]) {
        expect(find.text(gone), findsNothing, reason: gone);
      }
      expect(find.text('Import and give to June'), findsOneWidget);
      expect(
        toggle(tester, 'Delete the originals after they’re imported').value,
        isFalse,
      );
      expect(toggle(tester, askEachTimeLabel).value, isFalse);
      await tester.tap(find.text('Import only'));
      await pumpFrames(tester);
      final decided = _sent(
        harness,
        (m) => m.hasDecideSecretRequest(),
      ).single.decideSecretRequest;
      expect(decided.approve, isTrue);
      expect(decided.name, 'android-release');
      expect(decided.grantToAsker, isFalse);
      expect(decided.deleteOriginals, isFalse);
      expect(decided.entries, isEmpty);
      expect(decided.approveOnly, isFalse);
    });

    testWidgets('Import and give to June, the originals deleted when turned '
        'on', (tester) async {
      final harness = Harness(_workspace());
      await _show(tester, harness, SecretRequestCard(importing()));
      await tester.tap(_labeled('Delete the originals after they’re imported'));
      await tester.pump();
      await tester.tap(find.text('Import and give to June'));
      await pumpFrames(tester);
      final decided = _sent(
        harness,
        (m) => m.hasDecideSecretRequest(),
      ).single.decideSecretRequest;
      expect(decided.grantToAsker, isTrue);
      expect(decided.deleteOriginals, isTrue);
    });

    testWidgets('a path with hidden characters shows them as codes; where '
        'keys are kept, and an asker that read others, are said', (
      tester,
    ) async {
      final harness = Harness(_workspace());
      await _show(
        tester,
        harness,
        SecretRequestCard(
          importing(
            tainted: true,
            path: '/Users/me/.ssh/id_\u202Eyek.pub',
            sensitive: 'ssh',
          ),
        ),
      );
      expect(find.text('/Users/me/.ssh/id_\\u{202E}yek.pub'), findsOneWidget);
      expect(find.text('This is in your SSH keys folder.'), findsOneWidget);
      expect(
        find.textContaining('June had read a web page when it asked'),
        findsOneWidget,
      );
    });

    testWidgets('asked to go to no one: Import only, never a way to give it', (
      tester,
    ) async {
      final harness = Harness(_workspace());
      await _show(
        tester,
        harness,
        SecretRequestCard(importing()..grantToNobody = true),
      );
      expect(find.text('Import and give to June'), findsNothing);
      // Never says it's given (Abbacchio's row 730).
      expect(find.textContaining('Given to June'), findsNothing);
      expect(
        find.text(
          "No one sees what's in it. No agent gets it until you give it to one.",
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Import only'));
      await pumpFrames(tester);
      expect(
        _sent(
          harness,
          (m) => m.hasDecideSecretRequest(),
        ).single.decideSecretRequest.grantToAsker,
        isFalse,
      );
    });

    testWidgets("a file that can't be imported says why, and Import is off", (
      tester,
    ) async {
      final harness = Harness(_workspace());
      await _show(
        tester,
        harness,
        SecretRequestCard(importing(problem: 'It changed since June asked.')),
      );
      expect(find.text('It changed since June asked.'), findsOneWidget);
      expect(find.text("It can't be imported as it is."), findsOneWidget);
      await tester.tap(find.text('Import only'));
      await tester.tap(find.text('Import and give to June'));
      await pumpFrames(tester);
      expect(_sent(harness, (m) => m.hasDecideSecretRequest()), isEmpty);
    });

    testWidgets('imported: to whom, and the files', (tester) async {
      final harness = Harness(_workspace());
      await _show(
        tester,
        harness,
        SecretRequestCard(
          importing(state: SecretRequestState.SECRET_REQUEST_STATE_GENERATED),
        ),
      );
      expect(find.text('Imported. No agent has it yet'), findsOneWidget);
      expect(
        find.text(
          '/Users/me/keys/release.p12 · 4.1 KB · fingerprint 3f9a 21c0 7d44',
        ),
        findsOneWidget,
      );
      expect(find.text('Import only'), findsNothing);
    });

    testWidgets('a file changed since: the refusal shows, and it can be '
        'imported again', (tester) async {
      final harness = Harness(_workspace());
      await _show(tester, harness, SecretRequestCard(importing()));
      harness.backend.refuse =
          '/Users/me/keys/release.p12 changed since it was asked for, so it '
          "wasn't imported. Ask for it again.";
      await tester.tap(find.text('Import and give to June'));
      await pumpFrames(tester);
      expect(find.textContaining("so it wasn't imported"), findsOneWidget);
      harness.backend.refuse = null;
      await tester.tap(find.text('Import and give to June'));
      await pumpFrames(tester);
      expect(_sent(harness, (m) => m.hasDecideSecretRequest()), hasLength(2));
    });

    test('the summary in Needs you', () {
      expect(
        secretRequestSummary(importing(), _workspace()),
        'Asks to import 2 files',
      );
      expect(importPath('/a/\u200Bb\u0007'), '/a/\\u{200B}b\\u{0007}');
      expect(importPath('a\u2028b\u3164'), 'a\\u{2028}b\\u{3164}');
    });
  });

  testWidgets('Dismiss dismisses, and the card shows a comment from '
      'before, or Replied for a reply', (tester) async {
    final harness = Harness(_workspace());
    final request = resend();
    await _show(tester, harness, SecretRequestCard(request));
    await tester.tap(find.text('Dismiss'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.approve, isFalse);
    expect(decided.reply, isFalse);
    expect(decided.comment, isEmpty);

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

    await tester.pumpWidget(const SizedBox());
    await _show(
      tester,
      harness,
      SecretRequestCard(
        request.deepCopy()
          ..state = SecretRequestState.SECRET_REQUEST_STATE_DISMISSED
          ..replied = true
          ..comment = 'Use the test account.'
          ..decidedMs = Int64(
            DateTime(2026, 11, 20, 14, 2).millisecondsSinceEpoch,
          ),
      ),
    );
    expect(
      find.textContaining('Replied · ', findRichText: true),
      findsOneWidget,
    );
    expect(find.textContaining('Dismissed'), findsNothing);
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
