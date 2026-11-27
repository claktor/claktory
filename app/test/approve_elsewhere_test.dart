// Position B on Windows (windows.md section 1; Fugo 51697, 51723): every
// surface that would show, take or approve a code to let something into a
// Claktory shows none on Windows, and says where it's done instead. Each is
// pumped as Windows, and the same surface as a Mac shows what Windows
// mustn't, so the check can't pass by finding nothing anywhere.
import 'package:factory_app/account.dart' show AccountSettings;
import 'package:factory_app/approve_elsewhere.dart';
import 'package:factory_app/claktory_problem.dart';
import 'package:factory_app/connect_step.dart' show ConnectStep;
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/devices.dart';
import 'package:factory_app/links.dart';
import 'package:factory_app/move.dart';
import 'package:factory_app/pairing.dart';
import 'package:factory_app/pairing_card.dart';
import 'package:factory_app/people/people_view.dart' show peopleAvailable;
import 'package:factory_app/pipe_start.dart';
import 'package:factory_app/pipes.dart'
    show ConnectPipeForm, PipeApprovalCard, PipeIdentityNotice, PipeMessageView;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' hide MovePhase;
import 'package:factory_app/recovery_key.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _code = 'factory://?t=0123456789abcdef&name=mac-mini&e=abc';

final _now = DateTime.now().millisecondsSinceEpoch;

Workspace _workspace({
  List<Pipe> pipes = const [],
  List<PendingDevice> pending = const [],
}) => Workspace(
  projects: [Project(id: 'p2', name: 'Website', path: '/Users/me/site')],
  agents: [
    Agent(
      id: 'a1',
      name: 'Rivet',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
    ),
  ],
  pipes: pipes,
  remote: RemoteAccess(pendingDevices: pending),
  extensions: [Extension(id: 'dm')],
  selectedAgentId: 'a1',
);

final _asking = PendingDevice(
  deviceId: 'd9',
  name: 'Phone',
  number: '4821',
  requestedMs: Int64(_now),
);

DevicePairing _pairing(DevicePairingState state) => DevicePairing(
  id: 'p1',
  agentId: 'a1',
  deviceId: 'd9',
  name: 'Laptop',
  state: state,
  expiresMs: Int64(_now + 600000),
);

/// [home] alone in the app, as [platform].
Future<Harness> _pump(
  WidgetTester tester,
  TargetPlatform platform,
  Widget home, {
  Workspace? workspace,
  String deviceId = '',
}) async {
  debugDefaultTargetPlatformOverride = platform;
  // A fresh tree: the window's overlay keeps its first entry otherwise, so
  // a second surface in one test would never show.
  await tester.pumpWidget(const SizedBox());
  final harness = Harness(workspace ?? _workspace());
  harness.backend.shownCodes['p1'] = PairingCode(code: _code, deviceId: 'd9');
  harness.backend.deviceId = deviceId;
  await harness.show(tester, home: home);
  await pumpFrames(tester);
  return harness;
}

/// Every button's label showing.
Iterable<String> _buttons(WidgetTester tester) =>
    tester.widgetList<Button>(find.byType(Button)).map((b) => b.label);

/// What the surfaces would show or take, none of which Windows may.
void _noneShown(WidgetTester tester) {
  for (final text in [
    'Copy code',
    'Show QR code',
    'Paste code',
    'Scan code',
    'Use a code instead',
    'Use a code',
    'Use a new code',
    'Make invite',
    'I have a code',
    'Show code',
    'Allow',
    'Approve',
    'Confirm',
    'Add a device…',
    'Move to another computer…',
    'Show key',
  ]) {
    expect(find.text(text), findsNothing, reason: text);
    expect(_buttons(tester), isNot(contains(text)), reason: text);
  }
  for (final digits in ['4821', '1234', 'factoryd pair']) {
    expect(find.textContaining(digits), findsNothing, reason: digits);
  }
  expect(find.byType(QrCode).hitTestable(), findsNothing);
  expect(find.bySemanticsLabel('QR code'), findsNothing);
  expect(find.byType(DeviceNumberField), findsNothing);
}

/// Runs [body] as Windows, resetting the platform after.
void _windows(String name, Future<void> Function(WidgetTester) body) =>
    testWidgets(name, (tester) async {
      try {
        await body(tester);
      } finally {
        debugDefaultTargetPlatformOverride = null;
      }
    });

void main() {
  test('only Windows leaves codes and approvals elsewhere', () {
    for (final platform in TargetPlatform.values) {
      debugDefaultTargetPlatformOverride = platform;
      expect(approvesElsewhere, platform == TargetPlatform.windows);
    }
    debugDefaultTargetPlatformOverride = null;
  });

  _windows("an agent's device card: no code fetched, shown or copied, and "
      'no number or Allow', (tester) async {
    final harness = await _pump(
      tester,
      TargetPlatform.windows,
      DevicePairingCard(
        _pairing(DevicePairingState.DEVICE_PAIRING_STATE_WAITING),
      ),
    );
    expect(harness.backend.sent.where((m) => m.hasShowPairingCode()), isEmpty);
    expect(find.textContaining('mac-mini'), findsNothing);
    _noneShown(tester);
    expect(find.text(approveElsewhereText), findsOneWidget);

    await _pump(
      tester,
      TargetPlatform.windows,
      DevicePairingCard(
        _pairing(DevicePairingState.DEVICE_PAIRING_STATE_ASKING),
      ),
      workspace: _workspace(pending: [_asking]),
    );
    _noneShown(tester);
    expect(find.text(approveElsewhereText), findsOneWidget);
  });

  _windows("a waiting device in Needs you: no number field, no Allow", (
    tester,
  ) async {
    // On a Mac: what Windows mustn't show.
    await _pump(tester, TargetPlatform.macOS, AllowDeviceCard(device: _asking));
    expect(find.byType(DeviceNumberField), findsOneWidget);
    expect(_buttons(tester), contains('Allow'));

    await _pump(
      tester,
      TargetPlatform.windows,
      AllowDeviceCard(device: _asking),
    );
    _noneShown(tester);
    expect(find.text(approveElsewhereText), findsOneWidget);
  });

  _windows('Settings → Devices: no Add a device, no code, no QR code, and '
      "no Confirm for another device", (tester) async {
    final workspace = _workspace()
      ..remote.devices.addAll([
        PairedDevice(
          id: 'other',
          name: 'iPad',
          confirmByMs: Int64(_now + 86400000),
        ),
        // This PC itself, not confirmed yet: not from here either.
        PairedDevice(
          id: 'this-pc',
          name: 'Windows PC',
          confirmByMs: Int64(_now + 86400000),
        ),
      ]);
    await _pump(
      tester,
      TargetPlatform.windows,
      DevicesSettings(workspace: workspace),
      workspace: workspace,
      deviceId: 'this-pc',
    );
    _noneShown(tester);
    expect(find.text(addDeviceElsewhereText), findsOneWidget);
    expect(find.text("Confirm it's yours"), findsNothing);
    expect(find.text('Confirm…'), findsNothing);
    // Nor Remove, which could leave no phone or Mac to approve with.
    expect(_buttons(tester), isNot(contains('Remove')));

    // Every way to a code shows none.
    await _pump(tester, TargetPlatform.windows, PairDeviceForm(close: () {}));
    _noneShown(tester);
    await _pump(
      tester,
      TargetPlatform.windows,
      PairingCodeView(code: _code, device: 'Laptop', close: () {}),
    );
    _noneShown(tester);
    expect(find.textContaining('mac-mini'), findsNothing);
    await _pump(
      tester,
      TargetPlatform.windows,
      const QrCode(text: _code, size: 200),
    );
    _noneShown(tester);
  });

  _windows('Add a Claktory: the account only, never a code, even when asked '
      'to open at one', (tester) async {
    await _pump(
      tester,
      TargetPlatform.windows,
      FactoryChooser(close: () {}, code: true),
    );
    _noneShown(tester);
    expect(find.text('Add a Claktory with a code'), findsNothing);
    expect(find.text('Add a Claktory'), findsOneWidget);

    // The first run's Connect to your Claktory too.
    await _pump(tester, TargetPlatform.windows, const ConnectStep());
    _noneShown(tester);
  });

  _windows('a claktory://add link is refused unread, saying where it is done', (
    tester,
  ) async {
    final links = AppLinks()..receive('claktory://add?code=$_code');
    await _pump(
      tester,
      TargetPlatform.windows,
      LinkOpener(links: links, child: const SizedBox.expand()),
    );
    await settle(tester);
    expect(find.text(pairingCheckTitle), findsNothing);
    expect(find.text("Can't use this link"), findsOneWidget);
    expect(find.text(addClaktoryElsewhereText), findsOneWidget);
    expect(find.textContaining('mac-mini'), findsNothing);
  });

  _windows('a Claktory that turned this PC away: no new code, no factoryd '
      'pair', (tester) async {
    final harness = await _pump(
      tester,
      TargetPlatform.windows,
      const SizedBox(),
    );
    await _pump(
      tester,
      TargetPlatform.windows,
      NoLongerAllowedPage(backend: harness.backend),
    );
    _noneShown(tester);
    await _pump(
      tester,
      TargetPlatform.windows,
      ClaktorySignedOutPage(backend: harness.backend),
    );
    _noneShown(tester);
  });

  _windows('moves: no Move to another computer, and an agent\'s move shows no '
      'digits or Confirm', (tester) async {
    final harness = await _pump(
      tester,
      TargetPlatform.windows,
      const SizedBox(),
    );
    await _pump(
      tester,
      TargetPlatform.windows,
      MoveSettingRow(source: BackendMove(harness.backend)),
    );
    _noneShown(tester);
    expect(find.text(moveElsewhereText), findsOneWidget);

    await _pump(
      tester,
      TargetPlatform.windows,
      MoveConfirmCard(
        move: const MoveStatus(
          phase: MovePhase.confirm,
          targetMachine: 'nixos',
          digits: '1234',
          startedByAgentId: 'a1',
        ),
        actions: BackendMove(harness.backend),
      ),
    );
    _noneShown(tester);
    expect(find.text(approveElsewhereText), findsOneWidget);
  });

  _windows('pipes: no invite made, shown, scanned or pasted, and no Approve '
      'or digits', (tester) async {
    for (final kind in [
      PipeStartKind.PIPE_START_KIND_INVITE,
      PipeStartKind.PIPE_START_KIND_ACCEPT,
    ]) {
      await _pump(
        tester,
        TargetPlatform.windows,
        PipeStartCard(
          PipeStart(
            id: 's1',
            agentId: 'a1',
            kind: kind,
            projectId: 'p2',
            name: 'Launch',
            state: PipeStartState.PIPE_START_STATE_WAITING,
          ),
        ),
      );
      _noneShown(tester);
      expect(find.text(pipeElsewhereText), findsOneWidget);
    }
    await _pump(
      tester,
      TargetPlatform.windows,
      ConnectPipeForm(
        project: Project(id: 'p2', name: 'Website'),
        close: () {},
      ),
    );
    _noneShown(tester);
    await _pump(
      tester,
      TargetPlatform.windows,
      const PipeInviteQr(code: 'claktory-pipe://x'),
    );
    _noneShown(tester);

    final pipe = Pipe(
      id: 'pp',
      name: 'Acme',
      projectId: 'p2',
      state: PipeState.PIPE_STATE_APPROVAL,
      digits: '1234',
    );
    await _pump(
      tester,
      TargetPlatform.windows,
      PipeApprovalCard(pipe: pipe),
      workspace: _workspace(pipes: [pipe]),
    );
    _noneShown(tester);

    // A changed identity: never trusted from here.
    final changed = Pipe(
      id: 'pc',
      name: 'Acme',
      projectId: 'p2',
      state: PipeState.PIPE_STATE_OPEN,
      peer: PipePeer(
        identityChange: PipeIdentityChange.PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED,
        verifiedEmail: 'eve@example.com',
      ),
    );
    await _pump(
      tester,
      TargetPlatform.windows,
      PipeIdentityNotice(pipe: changed),
      workspace: _workspace(pipes: [changed]),
    );
    expect(find.text('Trust this Claktory…'), findsNothing);
    expect(find.text('OK'), findsNothing);
    expect(find.text(approveElsewhereText), findsOneWidget);

    // A held message, theirs or an agent's: neither let in, sent nor
    // discarded from here.
    for (final state in [
      PipeMessageState.PIPE_MESSAGE_STATE_HELD,
      PipeMessageState.PIPE_MESSAGE_STATE_HELD_INCOMING,
    ]) {
      await _pump(
        tester,
        TargetPlatform.windows,
        PipeMessageView(
          pipe: pipe,
          message: PipeMessage(seq: Int64(3), text: 'Hello', state: state),
        ),
        workspace: _workspace(pipes: [pipe]),
      );
      for (final label in ['Send', 'Let in', 'Discard', 'Submit']) {
        expect(_buttons(tester), isNot(contains(label)), reason: label);
      }
      expect(find.text(pipeElsewhereText), findsOneWidget);
    }
  });

  _windows("this Claktory's account: a first sign-in only, never signing in "
      'again or switching to another account', (tester) async {
    Future<void> settings(ClaktoryAccount account) => _pump(
      tester,
      TargetPlatform.windows,
      SingleChildScrollView(child: AccountSettings(account: account)),
    );
    // Never signed in: the documented first sign-in stays.
    await settings(
      ClaktoryAccount(
        state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_OUT,
      ),
    );
    expect(_buttons(tester), contains('Sign in…'));
    // Signed in before, then out: not again from here.
    await settings(
      ClaktoryAccount(
        state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_OUT,
        lastCheckInMs: Int64(_now),
      ),
    );
    expect(_buttons(tester), isNot(contains('Sign in…')));
    expect(find.text(signInElsewhereText), findsOneWidget);
    // Signed in: no Switch account….
    await settings(
      ClaktoryAccount(
        state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN,
        email: 'matt@example.com',
        lastCheckInMs: Int64(_now),
      ),
    );
    expect(_buttons(tester), isNot(contains('Switch account…')));
    expect(_buttons(tester), isNot(contains('Sign out…')));
    expect(_buttons(tester), isNot(contains('Show recovery key…')));
    // Removed at the account: not signed in again from here.
    await settings(
      ClaktoryAccount(
        state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_REVOKED,
        email: 'matt@example.com',
        lastCheckInMs: Int64(_now),
      ),
    );
    expect(_buttons(tester), isNot(contains('Sign in again')));
  });

  _windows('the recovery key never shows, and is never saved or used here', (
    tester,
  ) async {
    await _pump(
      tester,
      TargetPlatform.windows,
      SaveRecoveryKeyForm(close: () {}),
    );
    _noneShown(tester);
    expect(find.text(recoveryElsewhereText), findsOneWidget);
    await _pump(
      tester,
      TargetPlatform.windows,
      const RecoveryKeyBox('ABCD-EFGH-IJKL'),
    );
    expect(find.textContaining('ABCD'), findsNothing);
  });

  _windows("a waiting device's digits and People never show", (tester) async {
    await _pump(tester, TargetPlatform.windows, const PairingDigits('1234'));
    _noneShown(tester);
    expect(peopleAvailable(_workspace()), isFalse);
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    expect(peopleAvailable(_workspace()), isTrue);
  });
}
