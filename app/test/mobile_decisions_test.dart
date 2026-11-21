// What asks the person to decide or connect, on a phone at the largest
// text (#198, phase 2A): a secret asked for, a service an agent proposes
// to connect, a connection's details, a new device's code. Nothing runs
// off the screen or overflows.
import 'package:factory_app/connectors.dart';
import 'package:factory_app/devices.dart' show PairingCodeView, PairingWay;
import 'package:factory_app/login.dart' show LoginPanel;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/secrets.dart';
import 'package:factory_app/update.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _cloudflare = Connector(
  id: 'cloudflare',
  name: 'Cloudflare',
  description: 'Deploy sites and manage DNS in your Cloudflare accounts.',
  flow: ConnectorFlowKind.CONNECTOR_FLOW_KIND_BROWSER,
  accesses: [
    ConnectorAccess(
      id: 'deploy',
      label: 'Deploy websites',
      why: 'Publish sites and change DNS in the accounts you choose.',
      preselected: true,
    ),
  ],
  variables: ['CLOUDFLARE_API_TOKEN', 'CLOUDFLARE_ACCOUNT_ID'],
);

final _connection = SecretInfo(
  id: 'c1',
  name: 'Cloudflare (matthew.long-surname@example.com)',
  connection: SecretConnection(
    connectorId: 'cloudflare',
    account: 'matthew.long-surname@example.com',
    access: ['Publish sites and change DNS.'],
    resources: ['Cloudflare account Acme Corporation International'],
    status: ConnectionStatus.CONNECTION_STATUS_READY,
  ),
);

Workspace _workspace() => Workspace(
  agents: [
    Agent(id: 'june', name: 'June', color: 'pink', modelAvailable: true),
    Agent(id: 'rivet', name: 'Rivet', color: 'blue', modelAvailable: true),
  ],
  selectedAgentId: 'june',
  settings: Settings(userName: 'Matt'),
  vault: VaultState(
    exists: true,
    unlocked: true,
    secrets: [
      _connection,
      // Chosen for requests that ask for other variable names.
      SecretInfo(
        id: 'dns',
        name: 'CF_CLAKTORY_DNS',
        variables: [SecretVariable(name: 'CF_CLAKTORY_DNS')],
      ),
      SecretInfo(
        id: 'aws',
        name: 'AWS deploy',
        variables: [
          SecretVariable(name: 'AWS_ACCESS_KEY_ID'),
          SecretVariable(name: 'AWS_SECRET_ACCESS_KEY'),
        ],
      ),
    ],
    connectors: [_cloudflare],
    proposals: [
      ConnectProposal(
        id: 'p1',
        agentId: 'june',
        connectorId: 'cloudflare',
        accessId: 'deploy',
        purpose: 'Deploy the website to Cloudflare Pages.',
        access: 'Pages and DNS for example.com',
        reason: 'The site is ready and needs a home.',
        state: ConnectProposalState.CONNECT_PROPOSAL_STATE_PENDING,
        createdMs: Int64(1000),
      ),
    ],
  ),
);

/// [child] in a phone's column, scrolled through, at the largest text;
/// answers where anything overflowed.
Future<List<String>> _overflows(
  WidgetTester tester,
  double width,
  Widget child,
) async {
  tester.platformDispatcher.textScaleFactorTestValue = 3.1;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  tester.view.physicalSize = Size(width, 700);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  final errors = <String>[];
  final old = FlutterError.onError;
  FlutterError.onError = (details) {
    errors.add(
      details
          .toString()
          .split('\n')
          .where((line) => line.contains('.dart:'))
          .take(2)
          .join(' '),
    );
    old?.call(details);
  };
  addTearDown(() => FlutterError.onError = old);
  final harness = Harness(_workspace());
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
                padding: const EdgeInsets.all(16),
                child: child,
              ),
            ),
          ],
        ),
      ),
    ),
  );
  await pumpFrames(tester);
  for (var i = 0; i < 12; i++) {
    await tester.drag(
      find.byType(SingleChildScrollView).first,
      const Offset(0, -400),
      warnIfMissed: false,
    );
    await pumpFrames(tester);
  }
  tester.takeException();
  return errors;
}

void main() {
  final views = <String, Widget Function()>{
    'a secret asked for': () => SecretRequestCard(
      SecretRequest(
        id: 'r1',
        agentId: 'june',
        secretName: 'Resend production mail',
        newSecret: NewSecret(
          name: 'Resend production mail',
          description: 'Sends mail from claktory.com',
          variables: [SecretVariable(name: 'RESEND_PRODUCTION_API_KEY')],
        ),
        reason: 'To send the **launch** email.',
        state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
      ),
    ),
    'a secret that sets another name': () => SecretRequestCard(
      SecretRequest(
        id: 'r2',
        agentId: 'june',
        secretName: 'CF_CLAKTORY_DNS',
        newSecret: NewSecret(
          name: 'CF_CLAKTORY_DNS',
          variables: [SecretVariable(name: 'ANTHROPIC_API_KEY')],
        ),
        state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
      ),
    ),
    'a secret that sets other names': () => SecretRequestCard(
      SecretRequest(
        id: 'r3',
        agentId: 'june',
        secretName: 'AWS deploy',
        newSecret: NewSecret(
          name: 'AWS deploy',
          variables: [
            SecretVariable(name: 'AWS_KEY'),
            SecretVariable(name: 'AWS_SECRET'),
          ],
        ),
        state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
      ),
    ),
    'a need to approve': () => SecretRequestCard(
      SecretRequest(
        id: 'r4',
        agentId: 'june',
        adminAgentId: 'rivet',
        secretName: 'Anthropic API key',
        newSecret: NewSecret(
          name: 'Anthropic API key',
          description:
              'An Anthropic API key that can run Claude models a few '
              'times.',
          variables: [SecretVariable(name: 'ANTHROPIC_API_KEY')],
        ),
        reason: 'To run the live image test.',
        state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
      ),
    ),
    'a need approved': () => SecretRequestCard(
      SecretRequest(
        id: 'r5',
        agentId: 'june',
        adminAgentId: 'rivet',
        secretName: 'Anthropic API key',
        newSecret: NewSecret(
          name: 'Anthropic API key',
          variables: [SecretVariable(name: 'ANTHROPIC_API_KEY')],
        ),
        state: SecretRequestState.SECRET_REQUEST_STATE_APPROVED,
        approvedMs: Int64(1000),
        collecting: true,
      ),
    ),
    'a secret to add for another agent': () => SecretRequestCard(
      SecretRequest(
        id: 'r6',
        agentId: 'june',
        adminAgentId: 'rivet',
        secretName: 'Anthropic API key',
        newSecret: NewSecret(
          name: 'Anthropic API key',
          description: 'An Anthropic API key that can run Claude models.',
          variables: [SecretVariable(name: 'ANTHROPIC_API_KEY')],
        ),
        state: SecretRequestState.SECRET_REQUEST_STATE_APPROVED,
        approvedMs: Int64(1000),
        collecting: true,
      ),
      shownIn: 'rivet',
    ),
    'a connection proposed': () =>
        ConnectProposalCard(_workspace().vault.proposals.first),
    "a connection's details": () => ConnectionDetails(secret: _connection),
    "a new device's code": () => PairingCodeView(
      code: 'factory://100.64.0.2:7443?t=abc&fp=def&name=mac%20mini',
      device: "Matthew's iPhone 16 Pro Max",
      close: () {},
    ),
    "a new computer's code": () => PairingCodeView(
      code: 'factory://100.64.0.2:7443?t=abc&fp=def&name=mac%20mini',
      device: "Matthew's MacBook Pro 16-inch",
      way: PairingWay.paste,
      close: () {},
    ),
    'a sign-in asking for a key': () => LoginPanel(
      flow: LoginFlow(
        providerId: 'anthropic',
        methodId: 'api_key',
        prompt: LoginPrompt(
          kind: LoginPromptKind.LOGIN_PROMPT_KIND_SECRET,
          message: 'Paste your Anthropic API key.',
        ),
      ),
      method: 'Anthropic API key',
    ),
    'a sign-in with a code': () => LoginPanel(
      flow: LoginFlow(
        providerId: 'openai-codex',
        methodId: 'device',
        userCode: 'ABCD-1234',
        verificationUrl: 'https://auth.openai.com/codex/device',
        instructions: 'Enter the code on the page that opens.',
      ),
      method: 'OpenAI (ChatGPT Plus/Pro)',
    ),
    'a sign-in in the browser': () => LoginPanel(
      flow: LoginFlow(
        providerId: 'anthropic',
        methodId: 'oauth',
        url: 'https://claude.ai/oauth/authorize?code=true&client_id=x',
        prompt: LoginPrompt(
          kind: LoginPromptKind.LOGIN_PROMPT_KIND_CODE,
          message: 'Paste the code or the address the browser ends on.',
        ),
      ),
      method: 'Anthropic (Claude Pro/Max)',
    ),
    'unlocking the secrets': () => UnlockSecretsForm(close: () {}),
  };
  for (final MapEntry(key: name, value: view) in views.entries) {
    for (final width in [320.0, 440.0]) {
      testWidgets('$name fits $width points at the largest text', (
        tester,
      ) async {
        expect(await _overflows(tester, width, view()), isEmpty);
      });
    }
  }
}
