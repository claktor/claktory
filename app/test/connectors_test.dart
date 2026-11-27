import 'dart:async';

import 'package:factory_app/connectors.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/secrets.dart';
import 'package:factory_app/update.dart';
import 'package:factory_app/web_auth.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
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
      id: 'read',
      label: 'Read only',
      why: 'See your accounts, zones and DNS records.',
    ),
    ConnectorAccess(
      id: 'deploy',
      label: 'Deploy websites',
      why: 'Publish sites and change DNS in the accounts you choose.',
      preselected: true,
    ),
  ],
  redirectUris: [
    'http://127.0.0.1:{port}/oauth/cloudflare',
    'com.claktory.app:/oauth/cloudflare',
    'com.claktory.app.dev:/oauth/cloudflare',
  ],
  variables: ['CLOUDFLARE_API_TOKEN', 'CLOUDFLARE_ACCOUNT_ID'],
);

final _aws = Connector(
  id: 'aws',
  name: 'AWS',
  description: 'Use your AWS accounts through IAM Identity Center.',
  flow: ConnectorFlowKind.CONNECTOR_FLOW_KIND_DEVICE,
  accesses: [
    ConnectorAccess(
      id: 'roles',
      label: 'Use roles',
      why: 'Get short-lived keys for the roles you can use.',
      preselected: true,
    ),
  ],
  fields: [
    ConnectorField(
      id: 'start_url',
      label: 'Access portal URL',
      placeholder: 'https://d-1234.awsapps.com/start',
    ),
    ConnectorField(id: 'region', label: 'Region', placeholder: 'us-east-1'),
  ],
  setupNote: 'Your AWS needs IAM Identity Center with permission sets.',
);

/// Cloudflare as Matt wants it: make a token on its page, paste it back.
final _pasteCloudflare = _cloudflare.deepCopy()
  ..flow = ConnectorFlowKind.CONNECTOR_FLOW_KIND_PASTE
  ..redirectUris.clear()
  ..tokenUrl =
      'https://dash.cloudflare.com/profile/api-tokens?permissionGroupKeys=x';

final _connection = SecretInfo(
  id: 'c1',
  name: 'Cloudflare (matt@example.com)',
  connection: SecretConnection(
    connectorId: 'cloudflare',
    account: 'matt@example.com',
    access: ['Publish sites and change DNS.'],
    resources: ['Cloudflare account Acme'],
    status: ConnectionStatus.CONNECTION_STATUS_READY,
  ),
);

ConnectProposal _proposal({
  ConnectProposalState state =
      ConnectProposalState.CONNECT_PROPOSAL_STATE_PENDING,
}) => ConnectProposal(
  id: 'p1',
  agentId: 'june',
  connectorId: 'cloudflare',
  accessId: 'deploy',
  purpose: 'Deploy the website to Cloudflare Pages.',
  access: 'Pages and DNS for example.com',
  reason: 'The site is ready and needs a home.',
  state: state,
  createdMs: Int64(1000),
);

Workspace _workspace({
  List<SecretInfo> secrets = const [],
  List<ConnectProposal> proposals = const [],
  List<ConnectFlow> flows = const [],
  Connector? cloudflare,
}) => Workspace(
  agents: [
    Agent(id: 'june', name: 'June', color: 'pink', modelAvailable: true),
    Agent(id: 'rivet', name: 'Rivet', color: 'blue', modelAvailable: true),
  ],
  selectedAgentId: 'june',
  settings: Settings(userName: 'Matt'),
  vault: VaultState(
    exists: true,
    unlocked: true,
    secrets: secrets,
    connectors: [cloudflare ?? _cloudflare, _aws],
    proposals: proposals,
    flows: flows,
  ),
);

/// The backend tells every device how the flow stands, with [secrets].
void _flow(
  Harness harness,
  ConnectFlowState state, {
  String error = '',
  List<SecretInfo> secrets = const [],
  List<ConnectProposal> proposals = const [],
  Connector? cloudflare,
}) => harness.backend.workspace.apply(
  _workspace(
    secrets: secrets,
    proposals: proposals,
    cloudflare: cloudflare,
    flows: [
      ConnectFlow(
        id: 'flow-1',
        connectorId: 'cloudflare',
        state: state,
        error: error,
        secretId: state == ConnectFlowState.CONNECT_FLOW_STATE_CONNECTED
            ? 'c1'
            : '',
      ),
    ],
  ),
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

List<T> _sent<T>(Harness harness, T? Function(ClientMessage) of) => [
  for (final message in harness.backend.sent)
    if (of(message) case final T found) found,
];

/// The Mac's address for the service's answer, without a real socket.
class _Receiver implements ConnectReceiver {
  _Receiver(String path) : address = Uri.parse('http://127.0.0.1:5555$path');

  @override
  final Uri address;
  final _back = Completer<Uri?>();
  var closed = false;

  @override
  Future<Uri?> get back => _back.future;

  void arrive(String query) => _back.complete(address.replace(query: query));

  @override
  Future<void> close() async {
    closed = true;
    if (!_back.isCompleted) _back.complete(null);
  }
}

void main() {
  late List<Uri> opened;
  late List<_Receiver> receivers;
  late List<(Uri, String)> sheets;
  Completer<Uri?>? sheet;

  setUp(() {
    opened = [];
    receivers = [];
    sheets = [];
    sheet = null;
    openConnectPage = (url) async {
      opened.add(url);
      return true;
    };
    connectInSheet = false;
    addTearDown(() => debugConnectInSheet = null);
    startConnectReceiver = (path) async {
      final receiver = _Receiver(path);
      receivers.add(receiver);
      return receiver;
    };
    connectSheet = (url, scheme) {
      sheets.add((url, scheme));
      return (sheet = Completer<Uri?>()).future;
    };
    closeConnectSheet = () async {
      if (sheet?.isCompleted == false) sheet!.complete(null);
    };
  });

  testWidgets('Connect a service, on the Mac: the page opens in the browser, '
      'its answer comes back to this computer and goes to the backend; '
      'nobody has the connection', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(find.text('Connect a service…'));
    await tester.pumpAndSettle();
    expect(find.text('Connect a service'), findsOneWidget);
    expect(find.text('AWS'), findsOneWidget);
    await tester.tap(find.text('Cloudflare'));
    await tester.pumpAndSettle();

    expect(find.text('Connect Cloudflare'), findsOneWidget);
    // The preselected access, and why.
    expect(find.text('Deploy websites'), findsOneWidget);
    expect(
      find.text('Publish sites and change DNS in the accounts you choose.'),
      findsOneWidget,
    );
    expect(
      find.textContaining('Connecting gives no agent access'),
      findsOneWidget,
    );
    await tester.tap(find.text('Connect'));
    await tester.pump();
    await tester.pump();

    final start = _sent(
      harness,
      (m) => m.hasStartConnect() ? m.startConnect : null,
    ).single;
    expect(start.connectorId, 'cloudflare');
    expect(start.accessId, 'deploy');
    expect(start.redirectUri, 'http://127.0.0.1:5555/oauth/cloudflare');
    expect(start.proposalId, isEmpty);
    expect(opened.single.toString(), harness.backend.connectStarted.url);
    expect(
      find.textContaining('Sign in to Cloudflare in your browser'),
      findsOneWidget,
    );

    // The browser comes back: the whole address goes to the backend.
    receivers.single.arrive('code=abc&state=s');
    await tester.pump();
    final finish = _sent(
      harness,
      (m) => m.hasFinishConnect() ? m.finishConnect : null,
    ).single;
    expect(finish.flowId, 'flow-1');
    expect(
      finish.callbackUrl,
      'http://127.0.0.1:5555/oauth/cloudflare?code=abc&state=s',
    );

    _flow(harness, ConnectFlowState.CONNECT_FLOW_STATE_CHECKING);
    await tester.pump();
    expect(find.text('Checking what Cloudflare allowed…'), findsOneWidget);

    _flow(
      harness,
      ConnectFlowState.CONNECT_FLOW_STATE_CONNECTED,
      secrets: [_connection],
    );
    await tester.pump();
    expect(
      find.text('Connected Cloudflare as Cloudflare (matt@example.com).'),
      findsOneWidget,
    );
    expect(find.textContaining('No agent has it yet.'), findsOneWidget);
    expect(_sent(harness, (m) => m.hasGrantSecret() ? m : null), isEmpty);

    await tester.tap(find.text('Done'));
    await tester.pumpAndSettle();
    expect(find.text('Connect Cloudflare'), findsNothing);
    expect(
      _sent(
        harness,
        (m) => m.hasCancelConnect() ? m.cancelConnect : null,
      ).single.flowId,
      'flow-1',
      reason: 'the ended flow leaves the list',
    );
  });

  testWidgets('cancelling stops listening and ends the flow', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(find.text('Connect a service…'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Cloudflare'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Connect'));
    await tester.pump();
    await tester.pump();
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(receivers.single.closed, isTrue);
    expect(
      _sent(harness, (m) => m.hasCancelConnect() ? m : null),
      hasLength(1),
    );
    expect(_sent(harness, (m) => m.hasFinishConnect() ? m : null), isEmpty);
  });

  testWidgets("a failure says the backend's words and offers Try again", (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(find.text('Connect a service…'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Cloudflare'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Connect'));
    await tester.pump();
    await tester.pump();
    _flow(
      harness,
      ConnectFlowState.CONNECT_FLOW_STATE_FAILED,
      error: "Cloudflare didn't allow it: you clicked Deny.",
    );
    await tester.pump();
    expect(
      find.text("Cloudflare didn't allow it: you clicked Deny."),
      findsOneWidget,
    );
    await tester.tap(find.text('Try again'));
    await tester.pump();
    await tester.pump();
    expect(_sent(harness, (m) => m.hasStartConnect() ? m : null), hasLength(2));
    expect(receivers, hasLength(2));
  });

  testWidgets('a refusal before there is a flow shows too', (tester) async {
    final harness = Harness(_workspace())
      ..backend.refuse = "This Claktory can't connect services yet.";
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(find.text('Connect a service…'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Cloudflare'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Connect'));
    await tester.pump();
    await tester.pump();
    expect(
      find.text("This Claktory can't connect services yet."),
      findsOneWidget,
    );
    expect(receivers.single.closed, isTrue, reason: 'nothing can come back');
  });

  // Left to the platform (no test override): a touch device's sign-in
  // sheet on the app's scheme (Android's Custom Tab, an iPhone's or iPad's
  // sheet), a Mac's address of its own.
  for (final (platform, sheeted) in [
    (TargetPlatform.android, true),
    (TargetPlatform.iOS, true),
    (TargetPlatform.macOS, false),
  ]) {
    testWidgets('on ${platform.name} the answer comes back '
        '${sheeted ? 'in the sign-in sheet' : 'to an address of its own'}', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = platform;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      if (platform == TargetPlatform.android) {
        PhonePlatformPolicy.debugAndroidPolicy = true;
        addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
      }
      debugConnectInSheet = null;
      expect(connectInSheet, sheeted);
      final harness = Harness(_workspace());
      await _show(tester, harness, const SecretsSettings());
      await tester.tap(find.text('Connect a service…'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Cloudflare'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Connect'));
      await tester.pump();
      await tester.pump();
      final start = _sent(
        harness,
        (m) => m.hasStartConnect() ? m.startConnect : null,
      ).single;
      if (!sheeted) {
        expect(sheets, isEmpty);
        expect(receivers, hasLength(1));
        expect(start.redirectUri, startsWith('http://127.0.0.1:'));
        return;
      }
      expect(start.redirectUri, 'com.claktory.app:/oauth/cloudflare');
      expect(receivers, isEmpty, reason: 'nothing listens on the device');
      expect(opened, isEmpty, reason: 'no browser beside the sheet');
      expect(sheets.single.$2, 'com.claktory.app');
      sheet!.complete(
        Uri.parse('com.claktory.app:/oauth/cloudflare?code=abc&state=s'),
      );
      await tester.pump();
      expect(
        _sent(
          harness,
          (m) => m.hasFinishConnect() ? m.finishConnect : null,
        ).single.callbackUrl,
        'com.claktory.app:/oauth/cloudflare?code=abc&state=s',
      );
    });
  }

  testWidgets('on iPhone the sign-in sheet catches the answer on the app '
      'scheme; closing it cancels', (tester) async {
    connectInSheet = true;
    final harness = Harness(_workspace());
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(find.text('Connect a service…'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Cloudflare'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Connect'));
    await tester.pump();
    await tester.pump();
    final start = _sent(
      harness,
      (m) => m.hasStartConnect() ? m.startConnect : null,
    ).single;
    expect(start.redirectUri, 'com.claktory.app:/oauth/cloudflare');
    expect(receivers, isEmpty);
    expect(sheets.single.$2, 'com.claktory.app');
    sheet!.complete(
      Uri.parse('com.claktory.app:/oauth/cloudflare?code=abc&state=s'),
    );
    await tester.pump();
    expect(
      _sent(
        harness,
        (m) => m.hasFinishConnect() ? m.finishConnect : null,
      ).single.callbackUrl,
      'com.claktory.app:/oauth/cloudflare?code=abc&state=s',
    );

    // Closed without finishing: the flow ends.
    _flow(harness, ConnectFlowState.CONNECT_FLOW_STATE_FAILED, error: 'No.');
    await tester.pump();
    await tester.tap(find.text('Try again'));
    await tester.pump();
    await tester.pump();
    sheet!.complete(null);
    await tester.pump();
    await tester.pump();
    expect(_sent(harness, (m) => m.hasCancelConnect() ? m : null), isNotEmpty);
  });

  testWidgets("AWS asks for its portal first, then shows the page's code", (
    tester,
  ) async {
    final harness = Harness(_workspace());
    harness.backend.connectStarted = ConnectStarted(
      flowId: 'flow-1',
      url: 'https://device.sso.us-east-1.amazonaws.com/?user_code=WXYZ-1234',
      userCode: 'WXYZ-1234',
    );
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(find.text('Connect a service…'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('AWS'));
    await tester.pumpAndSettle();
    expect(
      find.text('Your AWS needs IAM Identity Center with permission sets.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Connect'));
    await tester.pump();
    expect(find.text('Enter the access portal URL.'), findsOneWidget);
    expect(_sent(harness, (m) => m.hasStartConnect() ? m : null), isEmpty);

    final fields = find.byType(EditableText);
    await tester.enterText(fields.at(0), 'https://d-1234.awsapps.com/start');
    await tester.enterText(fields.at(1), 'eu-west-1');
    await tester.tap(find.text('Connect'));
    await tester.pump();
    await tester.pump();
    final start = _sent(
      harness,
      (m) => m.hasStartConnect() ? m.startConnect : null,
    ).single;
    expect(start.redirectUri, isEmpty, reason: 'the backend polls');
    expect(start.fields, {
      'start_url': 'https://d-1234.awsapps.com/start',
      'region': 'eu-west-1',
    });
    expect(receivers, isEmpty);
    expect(opened.single.queryParameters['user_code'], 'WXYZ-1234');
    expect(find.text('Check that the page shows WXYZ-1234.'), findsOneWidget);
  });

  testWidgets('pasting a token: the page that makes it opens, the token goes '
      'to the backend once and leaves the field; a refused one asks for '
      'another', (tester) async {
    final harness = Harness(_workspace(cloudflare: _pasteCloudflare));
    harness.backend.connectStarted = ConnectStarted(
      flowId: 'flow-1',
      url: _pasteCloudflare.tokenUrl,
    );
    void flow(ConnectFlowState state, {String error = ''}) => _flow(
      harness,
      state,
      error: error,
      cloudflare: _pasteCloudflare,
      secrets: state == ConnectFlowState.CONNECT_FLOW_STATE_CONNECTED
          ? [_connection]
          : const [],
    );
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(find.text('Connect a service…'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Cloudflare'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Connect'));
    await tester.pump();
    await tester.pump();
    flow(ConnectFlowState.CONNECT_FLOW_STATE_WAITING);
    await tester.pump();

    final start = _sent(
      harness,
      (m) => m.hasStartConnect() ? m.startConnect : null,
    ).single;
    expect(start.redirectUri, isEmpty);
    expect(receivers, isEmpty, reason: 'nothing comes back by itself');
    expect(opened.single.toString(), _pasteCloudflare.tokenUrl);
    expect(
      find.textContaining("Make a token on Cloudflare's page"),
      findsOneWidget,
    );

    // Nothing pasted yet.
    await tester.tap(find.text('Connect'));
    await tester.pump();
    expect(find.text('Paste the token to connect.'), findsOneWidget);
    expect(_sent(harness, (m) => m.hasFinishConnect() ? m : null), isEmpty);

    await tester.enterText(find.byType(EditableText).last, ' cf-token-1 ');
    await tester.tap(find.text('Connect'));
    await tester.pump();
    final finish = _sent(
      harness,
      (m) => m.hasFinishConnect() ? m.finishConnect : null,
    ).single;
    expect(finish.flowId, 'flow-1');
    expect(finish.token, 'cf-token-1');
    expect(finish.callbackUrl, isEmpty);
    expect(find.text('Checking the token with Cloudflare…'), findsOneWidget);

    flow(ConnectFlowState.CONNECT_FLOW_STATE_EXCHANGING);
    await tester.pump();
    expect(find.text('Checking the token with Cloudflare…'), findsOneWidget);
    expect(find.byType(EditableText), findsNothing);

    // Refused: the same flow waits for another, saying why.
    flow(
      ConnectFlowState.CONNECT_FLOW_STATE_WAITING,
      error: "Cloudflare didn't accept this token: it can't edit DNS.",
    );
    await tester.pump();
    expect(
      find.text("Cloudflare didn't accept this token: it can't edit DNS."),
      findsOneWidget,
    );
    final field = tester.widget<EditableText>(find.byType(EditableText).last);
    expect(field.controller.text, isEmpty, reason: 'never kept or shown');
    expect(field.obscureText, isTrue);

    await tester.enterText(find.byType(EditableText).last, 'cf-token-2');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pump();
    expect(
      _sent(
        harness,
        (m) => m.hasFinishConnect() ? m.finishConnect : null,
      ).last.token,
      'cf-token-2',
    );
    flow(ConnectFlowState.CONNECT_FLOW_STATE_CONNECTED);
    await tester.pump();
    expect(
      find.text('Connected Cloudflare as Cloudflare (matt@example.com).'),
      findsOneWidget,
    );
    await tester.pump(const Duration(seconds: 2));
  });

  testWidgets("pasting from an agent's proposal card, and another device's "
      'refusal', (tester) async {
    final harness = Harness(
      _workspace(proposals: [_proposal()], cloudflare: _pasteCloudflare),
    );
    harness.backend.connectStarted = ConnectStarted(
      flowId: 'flow-1',
      url: _pasteCloudflare.tokenUrl,
    );
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(find.text('Connect Cloudflare'));
    await tester.pump();
    await tester.pump();
    expect(
      _sent(
        harness,
        (m) => m.hasStartConnect() ? m.startConnect : null,
      ).single.proposalId,
      'p1',
    );
    expect(opened.single.toString(), _pasteCloudflare.tokenUrl);
    expect(find.text('Open the page again'), findsOneWidget);
    await tester.tap(find.text('Open the page again'));
    await tester.pump();
    expect(opened, hasLength(2));

    harness.backend.refuse = 'Only the device that started this can finish it.';
    await tester.enterText(find.byType(EditableText).first, 'cf-token');
    await tester.tap(find.text('Connect'));
    await tester.pump();
    await tester.pump();
    expect(
      find.text('Only the device that started this can finish it.'),
      findsOneWidget,
    );
    final field = tester.widget<EditableText>(find.byType(EditableText).first);
    expect(field.controller.text, isEmpty);

    harness.backend.refuse = null;
    await tester.tap(find.text('Cancel'));
    await tester.pump();
    await tester.pump();
    expect(
      _sent(harness, (m) => m.hasCancelConnect() ? m : null),
      hasLength(1),
    );
  });

  testWidgets("an agent's proposal: what for, the access, why; Connect "
      'answers it, Dismiss tells it why', (tester) async {
    final harness = Harness(_workspace(proposals: [_proposal()]));
    await _show(tester, harness, const SecretsSettings());
    expect(find.text('June suggests connecting Cloudflare'), findsOneWidget);
    expect(
      find.text('Deploy the website to Cloudflare Pages.'),
      findsOneWidget,
    );
    expect(find.text('Pages and DNS for example.com'), findsOneWidget);
    expect(
      find.textContaining('Connecting gives June nothing yet'),
      findsOneWidget,
    );

    await tester.tap(find.text('Connect Cloudflare'));
    await tester.pump();
    await tester.pump();
    final start = _sent(
      harness,
      (m) => m.hasStartConnect() ? m.startConnect : null,
    ).single;
    expect(start.proposalId, 'p1');
    expect(start.comment, isEmpty);
    expect(start.accessId, 'deploy');
    expect(opened, hasLength(1));
    await tester.tap(find.text('Cancel'));
    await tester.pump();
    await tester.pump();

    // A typed comment is a reply: one Submit, nothing connected.
    await tester.enterText(find.byType(EditableText).last, 'Only DNS, please');
    await tester.pump();
    expect(find.text('Connect Cloudflare'), findsNothing);
    expect(find.text('Dismiss'), findsNothing);
    expect(
      find.text('Submit sends your words instead of connecting.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Submit'));
    await tester.pump();
    final reply = _sent(
      harness,
      (m) => m.hasDismissConnectProposal() ? m.dismissConnectProposal : null,
    ).single;
    expect(reply.proposalId, 'p1');
    expect(reply.reply, isTrue);
    expect(reply.comment, 'Only DNS, please');
    // Nothing else went: no second connect.
    expect(
      _sent(harness, (m) => m.hasStartConnect() ? m.startConnect : null),
      hasLength(1),
    );
  });

  testWidgets('Dismiss with no comment dismisses; Return in a comment is '
      'Submit', (tester) async {
    final harness = Harness(_workspace(proposals: [_proposal()]));
    await _show(tester, harness, const SecretsSettings());
    await tester.tap(find.text('Dismiss'));
    await tester.pump();
    final dismiss = _sent(
      harness,
      (m) => m.hasDismissConnectProposal() ? m.dismissConnectProposal : null,
    ).single;
    expect(dismiss.reply, isFalse);
    expect(dismiss.comment, isEmpty);

    await tester.enterText(find.byType(EditableText).last, 'Not yet');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    final sent = _sent(
      harness,
      (m) => m.hasDismissConnectProposal() ? m.dismissConnectProposal : null,
    );
    expect(sent, hasLength(2));
    expect(sent.last.reply, isTrue);
    expect(sent.last.comment, 'Not yet');
    expect(
      _sent(harness, (m) => m.hasStartConnect() ? m.startConnect : null),
      isEmpty,
    );
  });

  testWidgets('a replied proposal says Replied, with the reply', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await _show(
      tester,
      harness,
      ConnectProposalCard(
        _proposal(state: ConnectProposalState.CONNECT_PROPOSAL_STATE_DISMISSED)
          ..replied = true
          ..comment = 'Only DNS, please'
          ..decidedMs = Int64(2000),
      ),
    );
    expect(find.textContaining('Replied'), findsOneWidget);
    expect(find.textContaining('Dismissed'), findsNothing);
    expect(find.textContaining('Only DNS, please'), findsOneWidget);
  });

  testWidgets('a decided proposal says what was connected', (tester) async {
    final harness = Harness(_workspace());
    await _show(
      tester,
      harness,
      ConnectProposalCard(
        _proposal(state: ConnectProposalState.CONNECT_PROPOSAL_STATE_CONNECTED)
          ..secretName = 'Cloudflare (matt@example.com)'
          ..decidedMs = Int64(2000),
      ),
    );
    expect(
      find.textContaining(
        'Connected Cloudflare as Cloudflare (matt@example.com)',
      ),
      findsOneWidget,
    );
    expect(find.text('Connect Cloudflare'), findsNothing);
    expect(find.text('Dismiss'), findsNothing);
  });

  testWidgets("an AWS proposal shows its region, and its exact statements "
      'once opened, conditions in words', (tester) async {
    final harness = Harness(_workspace());
    await _show(
      tester,
      harness,
      ConnectProposalCard(
        _proposal()
          ..region = 'eu-central-1'
          ..statements.addAll([
            PolicyStatement(
              actions: ['ec2:DescribeImages', 'ec2:DescribeInstances'],
              resources: ['*'],
              condition:
                  '{"StringEquals":{"aws:RequestedRegion":"eu-central-1"}}',
            ),
            PolicyStatement(
              actions: ['ec2:RunInstances'],
              resources: ['arn:aws:ec2:eu-central-1:*:instance/*'],
              condition:
                  '{"StringEquals":{"aws:RequestTag/claktory:service":'
                  '"account"},"StringEqualsIfExists":{"ec2:InstanceType":'
                  '["t3.small","t3a.small"]}}',
            ),
            PolicyStatement(
              actions: ['ec2:CreateSecurityGroup'],
              resources: ['arn:aws:ec2:eu-central-1:*:vpc/*'],
            ),
          ]),
      ),
    );
    expect(find.text('Region'), findsOneWidget);
    expect(find.text('eu-central-1'), findsOneWidget);
    // Folded until asked for.
    expect(find.textContaining('ec2:RunInstances'), findsNothing);
    // A link at rest: underlined, before any pointer finds it.
    expect(
      tester
          .widget<Text>(find.text('Show exact permissions (3 rules)'))
          .style!
          .decoration,
      TextDecoration.underline,
    );
    await tester.tap(find.text('Show exact permissions (3 rules)'));
    await tester.pump();
    expect(find.text('Allows'), findsNWidgets(3));
    expect(
      find.text('ec2:DescribeImages\nec2:DescribeInstances'),
      findsOneWidget,
    );
    expect(find.text('* (all resources)'), findsOneWidget);
    expect(find.text('arn:aws:ec2:eu-central-1:*:instance/*'), findsOneWidget);
    expect(find.text('aws:RequestedRegion is eu-central-1'), findsOneWidget);
    expect(
      find.text(
        'aws:RequestTag/claktory:service is account\n'
        'ec2:InstanceType is t3.small or t3a.small (if the request has it)',
      ),
      findsOneWidget,
    );
    // No condition, no "Only when".
    expect(find.text('Only when'), findsNWidgets(2));
    await tester.tap(find.text('Hide exact permissions'));
    await tester.pump();
    expect(find.text('Allows'), findsNothing);
  });

  test('a condition with operators of its own reads as its JSON', () {
    expect(conditionLines(''), isNull);
    expect(conditionLines('{"Bool":{"aws:SecureTransport":"true"}}'), [
      'aws:SecureTransport is true',
    ]);
    expect(
      conditionLines('{"ForAnyValue:StringEquals":{"aws:TagKeys":["a","b"]}}'),
      [
        '{\n  "ForAnyValue:StringEquals": {\n    "aws:TagKeys": [\n'
            '      "a",\n      "b"\n    ]\n  }\n}',
      ],
    );
    expect(conditionLines('not json'), ['not json']);
  });

  testWidgets('a connection in the list: its service, account and access, '
      'no values, never given; a secret made from it with chosen access', (
    tester,
  ) async {
    final harness = Harness(_workspace(secrets: [_connection]));
    harness.backend.connectChoices = ConnectChoices(
      secretId: 'c1',
      rules: 'Choose one account and at least one permission.',
      choices: [
        ConnectChoice(kind: 'account', id: 'acc1', label: 'Acme'),
        ConnectChoice(
          kind: 'zone',
          id: 'z1',
          label: 'example.com',
          parent: 'acc1',
        ),
        ConnectChoice(
          kind: 'permission',
          id: 'pages:write',
          label: 'Pages: edit',
          why: 'Publish and change Pages projects.',
        ),
        ConnectChoice(
          kind: 'permission',
          id: 'dns:write',
          label: 'DNS: edit',
          why: 'Change DNS records.',
        ),
      ],
    );
    await _show(tester, harness, const SecretsSettings());
    expect(
      find.text('Cloudflare connection · matt@example.com'),
      findsOneWidget,
    );
    expect(find.textContaining('No agent has access'), findsOneWidget);

    await tester.tap(find.text('Cloudflare (matt@example.com)'));
    await tester.pumpAndSettle();
    expect(find.text('Publish sites and change DNS.'), findsOneWidget);
    expect(find.text('Cloudflare account Acme'), findsOneWidget);
    expect(find.text('Show'), findsNothing);
    expect(find.text('Give to…'), findsNothing);

    await tester.tap(find.text('Make a secret from it…'));
    await tester.pumpAndSettle();
    expect(
      find.text('Choose one account and at least one permission.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Make secret'));
    await tester.pump();
    expect(find.text('Choose what it may do.'), findsOneWidget);
    await tester.tap(find.text('Pages: edit'));
    await tester.tap(find.text('example.com'));
    await tester.pump();
    await tester.tap(find.text('Make secret'));
    await tester.pumpAndSettle();
    final derive = _sent(
      harness,
      (m) => m.hasDeriveSecret() ? m.deriveSecret : null,
    ).single;
    expect(derive.secretId, 'c1');
    expect(derive.name, 'Cloudflare example.com', reason: 'follows the choice');
    expect(derive.permissions, ['pages:write']);
    expect(derive.resources, ['acc1', 'z1']);
    expect(_sent(harness, (m) => m.hasGrantSecret() ? m : null), isEmpty);

    await tester.tap(find.text('Disconnect…'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Disconnect'));
    await tester.pumpAndSettle();
    final disconnect = _sent(
      harness,
      (m) => m.hasDisconnectService() ? m.disconnectService : null,
    ).single;
    expect(disconnect.secretId, 'c1');
    expect(disconnect.revoke, isTrue);
  });

  testWidgets('a secret made from a connection says so', (tester) async {
    final harness = Harness(
      _workspace(
        secrets: [
          _connection,
          SecretInfo(
            id: 'e1',
            name: 'Cloudflare Pages',
            variables: [SecretVariable(name: 'CLOUDFLARE_API_TOKEN')],
            grants: [SecretGrant(agentId: 'june')],
            export: SecretExport(
              connectionId: 'c1',
              access: ['Publish Pages projects.'],
              expiresMs: Int64(4000000000000),
            ),
          ),
        ],
      ),
    );
    await _show(tester, harness, const SecretsSettings());
    expect(
      find.text(
        'Made from Cloudflare (matt@example.com) · Renewed before it expires',
      ),
      findsOneWidget,
    );
    expect(
      find.textContaining('June uses secrets made from it'),
      findsOneWidget,
    );
  });

  testWidgets('nothing of it shows while the backend offers no services', (
    tester,
  ) async {
    final harness = Harness(_workspace()..vault.connectors.clear());
    await _show(tester, harness, const SecretsSettings());
    expect(find.text('Add a secret…'), findsOneWidget);
    expect(find.text('Connect a service…'), findsNothing);
  });

  test('a connect run is kept by what it is for, per backend', () {
    final a = RecordingBackend();
    final b = RecordingBackend();
    expect(ConnectRun.of(a, 'new'), same(ConnectRun.of(a, 'new')));
    expect(ConnectRun.of(a, 'new'), isNot(same(ConnectRun.of(b, 'new'))));
    expect(
      ConnectRun.of(a, 'proposal:p1'),
      isNot(same(ConnectRun.of(a, 'new'))),
    );
  });
}
