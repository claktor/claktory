import 'package:factory_app/backend.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/secrets.dart';
import 'package:factory_app/update.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

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

  testWidgets("an agent's request for a new secret: its values, then give", (
    tester,
  ) async {
    final harness = Harness(_workspace());
    final request = SecretRequest(
      id: 'r1',
      agentId: 'june',
      newSecret: NewSecret(
        name: 'Resend',
        description: 'Sends mail from claktory.com',
        variables: [SecretVariable(name: 'RESEND_API_KEY')],
      ),
      reason: 'To send the **launch** email.',
      state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
    );
    await _show(tester, harness, SecretRequestCard(request));
    expect(find.text('June asks for a new secret: Resend'), findsOneWidget);
    expect(find.text('June never sees the value.'), findsOneWidget);
    await tester.tap(find.text('Add and give to June'));
    await tester.pump();
    expect(find.text('Enter a value for each variable.'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).first, 're_123456789');
    await tester.tap(find.text('Add and give to June'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.approve, isTrue);
    expect(decided.requestId, 'r1');
    expect(decided.entries.single.variable, 'RESEND_API_KEY');
    expect(decided.entries.single.value, 're_123456789');
    expect(decided.hasProof(), isFalse);

    // Decided, the backend sends it back.
    await tester.pumpWidget(const SizedBox());
    await _show(
      tester,
      harness,
      SecretRequestCard(
        request.deepCopy()
          ..state = SecretRequestState.SECRET_REQUEST_STATE_GRANTED
          ..decidedMs = Int64(
            DateTime(2026, 11, 20, 14, 2).millisecondsSinceEpoch,
          ),
      ),
    );
    expect(find.text('Add and give to June'), findsNothing);
    expect(find.textContaining('Given to June'), findsOneWidget);
  });

  testWidgets("an agent's request for a secret there is: Give AWS to June", (
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
    await tester.tap(find.text('Give AWS to June'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.approve, isTrue);
    expect(decided.entries, isEmpty);
  });

  testWidgets('requests waiting show at the top of Settings → Secrets', (
    tester,
  ) async {
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
    expect(find.text('June asks for AWS'), findsOneWidget);
    expect(
      tester.getTopLeft(find.text('June asks for AWS')).dy,
      lessThan(tester.getTopLeft(find.text('Add a secret…')).dy),
    );
    await tester.tap(find.text('Give AWS to June'));
    await pumpFrames(tester);
    final decided = _sent(
      harness,
      (m) => m.hasDecideSecretRequest(),
    ).single.decideSecretRequest;
    expect(decided.requestId, 'r3');
    expect(decided.approve, isTrue);

    // Decided, the backend no longer lists it.
    harness.backend.workspace.apply(_workspace());
    await pumpFrames(tester);
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

  test('a refusal keeps its code', () {
    const failure = RequestFailure(
      'no',
      code: ErrorCode.ERROR_CODE_NOT_GRANTED,
    );
    expect(failure.code, ErrorCode.ERROR_CODE_NOT_GRANTED);
  });
}
