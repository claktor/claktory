// Connecting a service end to end, against a real Claktory whose Cloudflare
// is a stand-in (FACTORY_CLOUDFLARE_BASE, --dev only), from an iPhone (the
// Simulator) already paired with it, plus a second device paired with
// PAIRING_CODE_2:
//
//   1. A helper agent with the Secrets option `connect` suggests Cloudflare.
//   2. Its card's Connect, in the app: the service's page "opens" (fetched
//      here, as a browser would, following its redirect) and comes back to
//      the app's own address on 127.0.0.1, the Mac's way.
//   3. The connection is in the vault and nobody has it.
//   4. Rivet, told of it, makes a narrower secret from it and gives it to
//      the helper, whose command then gets the token, hidden in its output.
//   5. A flow started on this device can't be finished from the other one.
//   6. The other device connects it again in place, the iPhone's way (the
//      sign-in sheet's com.claktory.app.dev: address).
//   7. Disconnecting removes it and what was made from it.
//
//   cd app && flutter test integration_test/connect_service_test.dart \
//     -d <simulator> --dart-define=FACTORY_DEV=true \
//     --dart-define=PAIRING_CODE_2='factory://…'
//
// The helper's and Rivet's models run for real.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/connectors.dart';
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart' show EditableText, Offset, Scrollable;
import 'package:factory_app/state.dart';
import 'package:factory_app/tunnel_client.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'phone_test.dart' show hold, waitFor;

const _code2 = String.fromEnvironment('PAIRING_CODE_2');

/// Holds the screen still at each step, for screenshots.
const _shots = bool.fromEnvironment('SHOTS');
const _helper = 'Cleo';
const _helperModel = String.fromEnvironment(
  'HELPER_MODEL',
  defaultValue: 'anthropic/claude-haiku-4-5',
);

/// What a browser does with the service's page: follows its redirects, to
/// the app's own address in the end.
Future<bool> _browse(Uri url) async {
  final client = HttpClient();
  try {
    final response = await (await client.getUrl(url)).close();
    await response.drain<void>();
    return true;
  } finally {
    client.close(force: true);
  }
}

/// What the sign-in sheet catches: the service's redirect to the app's
/// scheme, which a browser can't follow.
Future<Uri?> _sheet(Uri url, String scheme) async {
  final client = HttpClient();
  try {
    final request = await client.getUrl(url)
      ..followRedirects = false;
    final response = await request.close();
    await response.drain<void>();
    final location = response.headers.value('location');
    expect(location, startsWith('$scheme:'));
    return Uri.parse(location!);
  } finally {
    client.close(force: true);
  }
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('connecting Cloudflare, end to end', (tester) async {
    await app.main();
    await waitFor(
      tester,
      find.byType(Sidebar),
      timeout: const Duration(minutes: 1),
    );
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    Workspace now() => backend.workspace.workspace;
    VaultState vault() => now().vault;
    Future<void> until(
      String what,
      bool Function() done, {
      Duration timeout = const Duration(minutes: 1),
    }) async {
      final end = DateTime.now().add(timeout);
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await tester.pump(const Duration(milliseconds: 200));
      }
    }

    await until('connected', () => backend.connected.value);
    final cloudflare = vault().connectors.firstWhere(
      (connector) => connector.id == 'cloudflare',
      orElse: () => fail('start the dev backend with FACTORY_CLOUDFLARE_BASE'),
    );

    // Left by a run that failed.
    for (final old in vault().secrets.where((s) => s.hasConnection())) {
      await backend.disconnectService(old.id);
    }
    for (final old in now().agents.where((a) => a.name == _helper)) {
      await backend.deleteAgent(old.id);
    }

    // 1. The helper suggests Cloudflare.
    await backend.createAgent(
      CreateAgent(
        name: _helper,
        role: 'Tester',
        model: _helperModel,
        prompt: 'You help test Claktory. Do exactly what you are asked.',
      ),
    );
    await until('the helper', () => now().agents.any((a) => a.name == _helper));
    final helper = now().agents.firstWhere((a) => a.name == _helper);
    await backend.updateAgent(
      UpdateAgent(
        agentId: helper.id,
        extension_8: ExtensionChange(
          id: 'secrets',
          optionId: 'connect',
          value: OptionValue(on: true),
        ),
      ),
    );
    await backend.sendMessage(
      helper.id,
      'This is a test. Call secrets_propose_connection once, with service '
      '"cloudflare", purpose "Publish the test site claktory.com", access '
      '"DNS edit on the claktory.com zone only", reason "End-to-end test '
      'of connecting services". Then stop; no need to reply.',
    );
    await until(
      'a suggestion',
      () => vault().proposals.any((p) => p.agentId == helper.id),
      timeout: const Duration(minutes: 2),
    );
    final proposal = vault().proposals.firstWhere(
      (p) => p.agentId == helper.id,
    );
    expect(proposal.connectorId, 'cloudflare');

    // 2. Connect, from the card in its conversation.
    connectInSheet = false;
    openConnectPage = _browse;
    await tester.tap(find.text(_helper).first);
    final card = find.byType(ConnectProposalCard);
    await waitFor(tester, card, timeout: const Duration(seconds: 30));
    await hold(tester, seconds: _shots ? 6 : 2);
    final connect = find.descendant(of: card, matching: find.text('Connect'));
    await tester.ensureVisible(connect.first);
    await tester.tap(connect.first);
    await until(
      'connected',
      () => vault().secrets.any(
        (s) => s.hasConnection() && s.connection.proposalId == proposal.id,
      ),
    );
    await waitFor(tester, find.textContaining('Connected Cloudflare as'));
    await hold(tester, seconds: _shots ? 6 : 2);
    final connection = vault().secrets.firstWhere(
      (s) => s.hasConnection() && s.connection.proposalId == proposal.id,
    );

    // 3. Nobody has it.
    expect(connection.grants, isEmpty);
    expect(connection.variables, isEmpty);
    expect(connection.connection.connectorId, 'cloudflare');
    // ignore: avoid_print
    print(
      'connected ${connection.name}: ${connection.connection.access} '
      '${connection.connection.resources}',
    );

    // 4. Rivet makes a narrower secret and gives it to the helper.
    bool made(SecretInfo s) =>
        s.hasExport() && s.export.connectionId == connection.id;
    await until(
      'Rivet giving a secret made from it',
      () => vault().secrets.any(
        (s) => made(s) && s.grants.any((g) => g.agentId == helper.id),
      ),
      timeout: const Duration(minutes: 5),
    );
    final export = vault().secrets.firstWhere(made);
    if (_shots) {
      // Settings → Secrets: the connection, opened, and what Rivet made.
      scope.page.value = AppPage.settings;
      await hold(tester, seconds: 1);
      final row = find.text(connection.name);
      await tester.scrollUntilVisible(
        row,
        300,
        scrollable: find.byType(Scrollable).first,
      );
      await tester.tap(row);
      await hold(tester, seconds: 1);
      await tester.drag(find.byType(Scrollable).first, const Offset(0, -150));
      await hold(tester, seconds: 6);
      scope.page.value = AppPage.agent;
      await hold(tester, seconds: 1);
    }
    // ignore: avoid_print
    print(
      'Rivet made ${export.name}: ${export.export.permissions} '
      '${export.export.resources}',
    );
    await backend.sendMessage(
      helper.id,
      'Now run this exact command with bash, passing secrets '
      '["${export.name}"]: printf "token=%s\\n" "\$CLOUDFLARE_API_TOKEN"; '
      'printf "%s" "\$CLOUDFLARE_API_TOKEN" | wc -c\n'
      'Then reply with only its output.',
    );
    ToolCall? ran;
    await until('the helper running it', () {
      final transcript = backend.transcript;
      if (transcript.agentId != helper.id) return false;
      for (var i = transcript.length - 1; i >= 0; i--) {
        final entry = transcript[i].entry;
        if (entry.hasToolCall() &&
            entry.toolCall.name == 'bash' &&
            entry.toolCall.output.contains('token=') &&
            entry.toolCall.state != ToolCallState.TOOL_CALL_STATE_RUNNING) {
          ran = entry.toolCall;
          return true;
        }
      }
      return false;
    }, timeout: const Duration(minutes: 2));
    // ignore: avoid_print
    print('the command said: ${ran!.output}');
    expect(ran!.output, isNot(contains('minted-')), reason: 'hidden');
    expect(ran!.output, contains('[secret'));
    expect(
      RegExp(r'^\s*(\d+)\s*$', multiLine: true).firstMatch(ran!.output),
      isNotNull,
      reason: 'it had a value',
    );

    // 5. A flow belongs to the device that started it.
    expect(_code2, isNotEmpty, reason: 'pass --dart-define=PAIRING_CODE_2=…');
    final key = newDeviceKey();
    final other =
        Backend(
            workspace: WorkspaceStore(),
            transcript: TranscriptStore(),
            explorer: ExplorerStore(),
            extensionView: ExtensionViewStore(),
            localBackend: false,
            localPairing: () async => null,
          )
          ..deviceKey = ((_) => key)
          ..connectTo(parsePairingCode(_code2));
    unawaited(other.run());
    await until('the other device waiting', () {
      return now().remote.pendingDevices.isNotEmpty || other.connected.value;
    });
    for (final pending in now().remote.pendingDevices) {
      await backend.approveDevice(pending.deviceId);
    }
    await until(
      'the other device connected',
      () =>
          other.connected.value &&
          other.workspace.workspace.vault.connectors.isNotEmpty,
    );
    final started = await backend.startConnect(
      StartConnect(
        connectorId: 'cloudflare',
        redirectUri: 'http://127.0.0.1:5999/oauth/cloudflare',
      ),
    );
    Object? refused;
    try {
      await other.finishConnect(
        started.flowId,
        'http://127.0.0.1:5999/oauth/cloudflare?code=good-code&state=x',
      );
    } on RequestFailure catch (failure) {
      refused = failure;
    }
    // ignore: avoid_print
    print('another device finishing it: $refused');
    expect(refused, isNotNull);
    await backend.cancelConnect(started.flowId);

    // 6. The other device connects it again, the iPhone's way.
    connectInSheet = true;
    connectSheet = _sheet;
    final again = ConnectRun.of(other, 'secret:${connection.id}');
    await again.start(cloudflare, secretId: connection.id);
    await until(
      'connected again',
      () =>
          again.started != null &&
          again.state == ConnectFlowState.CONNECT_FLOW_STATE_CONNECTED,
    );
    expect(again.error, isNull);
    expect(
      vault().secrets.where((s) => s.hasConnection()),
      hasLength(1),
      reason: 'in place',
    );
    expect(
      vault().secrets.firstWhere(made).grants.map((g) => g.agentId),
      contains(helper.id),
      reason: 'what was made from it, and who has it, stay',
    );
    await again.clear();

    // 7. Disconnect, cancelling it at Cloudflare.
    await backend.disconnectService(connection.id, revoke: true);
    await until(
      'gone',
      () => !vault().secrets.any((s) => s.id == connection.id || made(s)),
    );
    await hold(tester, seconds: 2);
    await backend.deleteAgent(helper.id);
    await other.close();
    // ignore: avoid_print
    print('connected, made, given, used, bound, reconnected and disconnected');
  });

  testWidgets('connecting AWS through its device flow, from Settings', (
    tester,
  ) async {
    await app.main();
    await waitFor(
      tester,
      find.byType(Sidebar),
      timeout: const Duration(minutes: 1),
    );
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    Workspace now() => backend.workspace.workspace;
    VaultState vault() => now().vault;
    Future<void> until(
      String what,
      bool Function() done, {
      Duration timeout = const Duration(minutes: 1),
    }) async {
      final end = DateTime.now().add(timeout);
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await tester.pump(const Duration(milliseconds: 200));
      }
    }

    await until('connected', () => backend.connected.value);
    expect(
      vault().connectors.map((c) => c.id),
      contains('aws'),
      reason: 'start the dev backend with FACTORY_AWS_BASE',
    );
    for (final old in vault().secrets.where((s) => s.hasConnection())) {
      await backend.disconnectService(old.id);
    }
    for (final old in now().agents.where((a) => a.name == _helper)) {
      await backend.deleteAgent(old.id);
    }
    final scroller = find.byType(Scrollable).first;
    Future<void> tapText(String text) async {
      final found = find.text(text);
      await tester.scrollUntilVisible(found, 200, scrollable: scroller);
      await tester.pump(const Duration(milliseconds: 300));
      await tester.tap(found.last);
      await tester.pump(const Duration(milliseconds: 500));
    }

    scope.page.value = AppPage.settings;
    await hold(tester, seconds: 1);

    // Started and cancelled while it waits: nothing comes of it.
    openConnectPage = (url) async => true;
    await tapText('Connect a service…');
    await tester.tap(find.text('AWS').last);
    await tester.pump(const Duration(milliseconds: 500));
    final fields = find.descendant(
      of: find.byType(ConnectServiceForm),
      matching: find.byType(EditableText),
    );
    await tester.enterText(fields.at(0), 'd-1234567890');
    await tester.enterText(fields.at(1), 'us-east-1');
    await tester.tap(find.text('Connect').last);
    await waitFor(tester, find.textContaining('Check that the page shows'));
    final cancelled = ConnectRun.of(backend, 'new').started!.flowId;
    await tester.tap(find.text('Cancel').last);
    await until(
      'the flow cancelled',
      () => !vault().flows.any(
        (f) =>
            f.id == cancelled &&
            f.state == ConnectFlowState.CONNECT_FLOW_STATE_WAITING,
      ),
    );
    expect(vault().secrets.where((s) => s.hasConnection()), isEmpty);

    // Connected: the page with its code is approved a few seconds later.
    openConnectPage = _browse;
    await tapText('Connect a service…');
    await tester.tap(find.text('AWS').last);
    await tester.pump(const Duration(milliseconds: 500));
    await tester.tap(find.text('Connect').last);
    await tester.pump();
    expect(find.text('Enter the access portal URL.'), findsOneWidget);
    await tester.enterText(fields.at(0), 'd-1234567890');
    await tester.enterText(fields.at(1), 'us-east-1');
    await tester.tap(find.text('Connect').last);
    await waitFor(tester, find.textContaining('Check that the page shows'));
    await hold(tester, seconds: _shots ? 3 : 1);
    await waitFor(
      tester,
      find.textContaining('Connected AWS as'),
      timeout: const Duration(seconds: 40),
    );
    await hold(tester, seconds: _shots ? 4 : 1);
    await tester.tap(find.text('Done').last);
    await tester.pump(const Duration(milliseconds: 500));
    final connection = vault().secrets.firstWhere((s) => s.hasConnection());
    // ignore: avoid_print
    print(
      'connected ${connection.name}: ${connection.connection.access} '
      '${connection.connection.resources}',
    );
    expect(connection.grants, isEmpty);

    // A secret made from it, for one role, as the person.
    await tapText(connection.name);
    await tapText('Make a secret from it…');
    await waitFor(tester, find.text('Choose an account'));
    await tester.tap(find.text('Choose an account'));
    await tester.pump(const Duration(milliseconds: 500));
    await tester.tap(find.textContaining('111122223333').last);
    await tester.pump(const Duration(milliseconds: 500));
    await tester.tap(find.text('Choose a role'));
    await tester.pump(const Duration(milliseconds: 500));
    await tester.tap(find.text('DeploySite').last);
    await tester.pump(const Duration(milliseconds: 500));
    await hold(tester, seconds: _shots ? 4 : 1);
    await tester.tap(find.text('Make secret'));
    bool made(SecretInfo s) =>
        s.hasExport() && s.export.connectionId == connection.id;
    await until('a secret made from it', () => vault().secrets.any(made));
    final export = vault().secrets.firstWhere(made);
    // ignore: avoid_print
    print(
      'made ${export.name}: ${export.variables.map((v) => v.name)} '
      '${export.export.resources} per use ${export.export.perUse}',
    );
    expect(export.export.perUse, isTrue);
    await hold(tester, seconds: _shots ? 5 : 1);

    // Given to the helper, whose command gets fresh keys, hidden.
    await backend.createAgent(
      CreateAgent(
        name: _helper,
        role: 'Tester',
        model: _helperModel,
        prompt: 'You help test Claktory. Do exactly what you are asked.',
      ),
    );
    await until('the helper', () => now().agents.any((a) => a.name == _helper));
    final helper = now().agents.firstWhere((a) => a.name == _helper);
    await backend.grantSecret(export.id, helper.id);
    await backend.selectAgent(helper.id);
    await backend.sendMessage(
      helper.id,
      'Run this exact command with bash, passing secrets '
      '["${export.name}"]: printf "key=%s region=%s\\n" '
      '"\$AWS_ACCESS_KEY_ID" "\$AWS_REGION"\n'
      'Then reply with only its output.',
    );
    ToolCall? ran;
    await until('the helper running it', () {
      final transcript = backend.transcript;
      if (transcript.agentId != helper.id) return false;
      for (var i = transcript.length - 1; i >= 0; i--) {
        final entry = transcript[i].entry;
        if (entry.hasToolCall() &&
            entry.toolCall.name == 'bash' &&
            entry.toolCall.output.contains('key=') &&
            entry.toolCall.state != ToolCallState.TOOL_CALL_STATE_RUNNING) {
          ran = entry.toolCall;
          return true;
        }
      }
      return false;
    }, timeout: const Duration(minutes: 2));
    // ignore: avoid_print
    print('the command said: ${ran!.output}');
    expect(ran!.output, isNot(contains('ASIAFAKE')), reason: 'hidden');
    expect(ran!.output, contains('[secret'));
    expect(ran!.output, contains('region=us-east-1'));

    // Disconnected from its row.
    scope.page.value = AppPage.settings;
    await hold(tester, seconds: 1);
    if (find.text('Disconnect…').evaluate().isEmpty) {
      await tapText(connection.name);
    }
    await tapText('Disconnect…');
    await tester.tap(find.text('Disconnect').last);
    await until(
      'gone',
      () => !vault().secrets.any((s) => s.id == connection.id || made(s)),
    );
    scope.page.value = AppPage.agent;
    await backend.deleteAgent(helper.id);
    // ignore: avoid_print
    print('AWS: cancelled, connected, made, given, used and disconnected');
  });
}
