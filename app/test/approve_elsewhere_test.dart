// Position B on Windows: every
// surface that would show, take or approve a code to let something into a
// Claktory shows none on Windows, and says where it's done instead. Each is
// pumped as Windows, and the same surface as a Mac shows what Windows
// mustn't, so the check can't pass by finding nothing anywhere.
import 'package:factory_app/account.dart' show AccountSettings;
import 'package:factory_app/approve_elsewhere.dart';
import 'package:factory_app/ask_card.dart' show AskPersonCard;
import 'package:factory_app/claktory_problem.dart';
import 'package:factory_app/connect_step.dart' show ConnectStep;
import 'package:factory_app/connection.dart' show Connection;
import 'package:factory_app/connections.dart' show thisMacName;
import 'package:factory_app/controls.dart' show Button, Toggle;
import 'package:factory_app/errands/browser_agent.dart' show browserExtension;
import 'package:factory_app/forms.dart' show AgentSettingsForm;
import 'package:factory_app/devices.dart';
import 'package:factory_app/links.dart';
import 'package:factory_app/move.dart';
import 'package:factory_app/pairing.dart';
import 'package:factory_app/pairing_card.dart';
import 'package:factory_app/people/people_view.dart' show peopleAvailable;
import 'package:factory_app/pipe_reports.dart'
    show ForwardReportsField, StopReadingReportsField;
import 'package:factory_app/pipe_shares.dart'
    show PipeSharesField, ReceivedShare;
import 'package:factory_app/pipe_start.dart';
import 'package:factory_app/pipe_timeline.dart' show PipeTargetChooser;
import 'package:factory_app/pipes.dart'
    show
        ConnectPipeForm,
        PipeApprovalCard,
        PipeIdentityNotice,
        PipeLoopCard,
        PipeMessageView,
        PipeSettingsForm,
        PipeSpendNotice,
        PipeView;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' hide MovePhase;
import 'package:factory_app/recovery_key.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/transcript.dart' show ProposalCard;
import 'package:factory_app/report_types.dart' show ReportTypesSettings;
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
  List<Agent> agents = const [],
  List<PipeShare> shares = const [],
}) => Workspace(
  projects: [Project(id: 'p2', name: 'Website', path: '/Users/me/site')],
  agents: [
    Agent(
      id: 'a1',
      name: 'Rivet',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
    ),
    ...agents,
  ],
  pipes: pipes,
  pipeShares: shares,
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

Pipe _pipe(
  PipeState state, {
  PipePause paused = PipePause.PIPE_PAUSE_UNSPECIFIED,
  String leadId = '',
}) => Pipe(
  id: 'pp',
  name: 'Acme',
  projectId: 'p2',
  state: state,
  paused: paused,
  leadId: leadId,
  online: true,
  memberIds: const ['a1'],
);

/// No message the backend keeps to the person went (authority::class's
/// Person).
void _nothingPersonOnly(Harness harness) {
  for (final message in harness.backend.sent) {
    expect(
      message.hasSetPipeState() ||
          message.hasUpdatePipe() ||
          message.hasSetSpendCap() ||
          message.hasSendPipeMessage() ||
          message.hasAddPipeLiaison() ||
          message.hasSetPipeForward() ||
          message.hasSharePipeWebsite() ||
          message.hasOpenPipeShare() ||
          message.hasSetAgentContained() ||
          (message.updateAgent.extension_8.id == browserExtension &&
              message.updateAgent.extension_8.enabled) ||
          message.hasShowRecoveryCodes() ||
          message.hasKeepRecoveryCodes(),
      isFalse,
      reason: '$message',
    );
  }
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

  test("the footer says this PC on Windows, this Mac elsewhere", () {
    final harness = Harness(Workspace());
    harness.backend.connectedSince.value = DateTime(2026, 11, 20, 14, 2);
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    expect(Connection(harness.backend).line, 'Running on this PC');
    expect(
      Connection(harness.backend).details,
      startsWith('Claktory is running on this PC.'),
    );
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    expect(Connection(harness.backend).line, 'Running on this Mac');
    debugDefaultTargetPlatformOverride = null;
  });

  test(
    "this computer's own Claktory is This PC on Windows until it's named",
    () {
      debugDefaultTargetPlatformOverride = TargetPlatform.windows;
      expect(thisMacName, 'This PC');
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      expect(thisMacName, 'This Mac');
      debugDefaultTargetPlatformOverride = null;
    },
  );

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
      workspace: _workspace()..vault = VaultState(exists: true, unlocked: true),
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
    // Nor another account service (SetAccountService).
    expect(_buttons(tester), isNot(contains('Change…')));
    expect(_buttons(tester), isNot(contains('Show recovery key…')));
    expect(_buttons(tester), isNot(contains('Make a new recovery key…')));
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

  _windows('an open pipe takes no message from the person here, and a paused '
      'or closed one is neither resumed nor deleted', (tester) async {
    final open = _pipe(PipeState.PIPE_STATE_OPEN);
    // On a Mac: the composer Windows mustn't have.
    await _pump(
      tester,
      TargetPlatform.macOS,
      PipeView(pipe: open),
      workspace: _workspace(pipes: [open]),
    );
    expect(find.byType(EditableText), findsOneWidget);

    var harness = await _pump(
      tester,
      TargetPlatform.windows,
      PipeView(pipe: open),
      workspace: _workspace(pipes: [open]),
    );
    expect(find.byType(EditableText), findsNothing);
    expect(find.text(pipeChangeElsewhereText), findsOneWidget);

    for (final (pipe, label) in [
      (
        _pipe(PipeState.PIPE_STATE_PAUSED, paused: PipePause.PIPE_PAUSE_BY_ME),
        'Resume',
      ),
      (
        _pipe(PipeState.PIPE_STATE_PAUSED, paused: PipePause.PIPE_PAUSE_LOOP),
        'Continue',
      ),
      (_pipe(PipeState.PIPE_STATE_CLOSED), 'Delete…'),
    ]) {
      await _pump(
        tester,
        TargetPlatform.macOS,
        PipeView(pipe: pipe),
        workspace: _workspace(pipes: [pipe]),
      );
      expect(_buttons(tester), contains(label), reason: 'Mac: $label');
      harness = await _pump(
        tester,
        TargetPlatform.windows,
        PipeView(pipe: pipe),
        workspace: _workspace(pipes: [pipe]),
      );
      expect(_buttons(tester), isNot(contains(label)), reason: label);
      expect(find.text(pipeChangeElsewhereText), findsOneWidget);
    }

    final loop = _pipe(
      PipeState.PIPE_STATE_PAUSED,
      paused: PipePause.PIPE_PAUSE_LOOP,
    );
    harness = await _pump(
      tester,
      TargetPlatform.windows,
      PipeLoopCard(pipe: loop),
      workspace: _workspace(pipes: [loop]),
    );
    expect(_buttons(tester), isNot(contains('Continue')));
    expect(find.text(pipeChangeElsewhereText), findsOneWidget);
    _nothingPersonOnly(harness);
  });

  _windows("a pipe with a liaison: the box writes only to it", (tester) async {
    final open = _pipe(PipeState.PIPE_STATE_OPEN, leadId: 'kai');
    final liaison = Agent(
      id: 'kai',
      name: 'Kai',
      role: 'Liaison',
      projectId: 'p2',
      pipeId: 'pp',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
    );
    await _pump(
      tester,
      TargetPlatform.macOS,
      PipeView(pipe: open),
      workspace: _workspace(pipes: [open], agents: [liaison]),
    );
    expect(find.byType(PipeTargetChooser), findsOneWidget);

    final harness = await _pump(
      tester,
      TargetPlatform.windows,
      PipeView(pipe: open),
      workspace: _workspace(pipes: [open], agents: [liaison]),
    );
    expect(find.byType(PipeTargetChooser), findsNothing);
    expect(find.text(pipeChangeElsewhereText), findsOneWidget);
    _nothingPersonOnly(harness);
  });

  _windows("a pipe's settings show as they are, and change nothing", (
    tester,
  ) async {
    final open = _pipe(PipeState.PIPE_STATE_OPEN);
    await _pump(
      tester,
      TargetPlatform.macOS,
      PipeSettingsForm(pipeId: 'pp', close: () {}),
      workspace: _workspace(pipes: [open]),
    );
    for (final label in ['Save', 'Close pipe…', 'Pause pipe']) {
      expect(_buttons(tester), contains(label), reason: 'Mac: $label');
    }

    var closed = 0;
    final harness = await _pump(
      tester,
      TargetPlatform.windows,
      PipeSettingsForm(pipeId: 'pp', close: () => closed++),
      workspace: _workspace(pipes: [open]),
    );
    for (final label in [
      'Save',
      'Close pipe…',
      'Pause pipe',
      'Add a liaison agent…',
      'Share a website…',
    ]) {
      expect(_buttons(tester), isNot(contains(label)), reason: label);
    }
    expect(find.text(pipeChangeElsewhereText), findsOneWidget);
    // Its name shows, and can't be typed into.
    expect(find.text('Acme'), findsWidgets);
    await tester.tap(find.byType(EditableText).first, warnIfMissed: false);
    await tester.pump();
    expect(
      tester.testTextInput.isVisible || tester.testTextInput.hasAnyClients,
      isFalse,
    );
    await tester.tap(find.widgetWithText(Button, 'Done'));
    await tester.pump();
    expect(closed, 1);
    _nothingPersonOnly(harness);
  });

  _windows("a pipe's lead over its spending limit: no Raise limit here", (
    tester,
  ) async {
    final open = _pipe(PipeState.PIPE_STATE_OPEN, leadId: 'kai');
    final lead = Agent(
      id: 'kai',
      name: 'Kai',
      projectId: 'p2',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      spend: ContainedSpend(capUsd: 10, spentUsd: 11, over: true),
    );
    await _pump(
      tester,
      TargetPlatform.macOS,
      PipeSpendNotice(pipe: open),
      workspace: _workspace(pipes: [open], agents: [lead]),
    );
    expect(_buttons(tester), contains('Raise limit'));
    final harness = await _pump(
      tester,
      TargetPlatform.windows,
      PipeSpendNotice(pipe: open),
      workspace: _workspace(pipes: [open], agents: [lead]),
    );
    expect(_buttons(tester), isNot(contains('Raise limit')));
    expect(find.byType(EditableText), findsNothing);
    expect(find.text(pipeChangeElsewhereText), findsOneWidget);
    _nothingPersonOnly(harness);
  });

  _windows("reports: where they go is shown, never changed; no report types "
      'made, and no reader unmade', (tester) async {
    final open = _pipe(PipeState.PIPE_STATE_OPEN);
    final reader = Agent(
      id: 'ingrid',
      name: 'Ingrid',
      projectId: 'p2',
      readsReports: true,
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
    );
    final forwarding = open.deepCopy()
      ..forward = PipeForward(recipientId: 'ingrid');
    final workspace = _workspace(pipes: [forwarding], agents: [reader]);
    final harness = await _pump(
      tester,
      TargetPlatform.windows,
      ForwardReportsField(pipe: forwarding),
      workspace: workspace,
    );
    expect(find.text('Ingrid'), findsOneWidget);
    expect(find.text('Another agent…'), findsNothing);
    expect(find.text(reportsElsewhereText), findsOneWidget);

    await _pump(
      tester,
      TargetPlatform.windows,
      StopReadingReportsField(agent: reader),
      workspace: workspace,
    );
    expect(_buttons(tester), isNot(contains('Stop reading reports…')));
    expect(find.text(reportsElsewhereText), findsOneWidget);

    await _pump(
      tester,
      TargetPlatform.macOS,
      const SingleChildScrollView(child: ReportTypesSettings()),
    );
    expect(_buttons(tester), contains('New report type…'));
    await _pump(
      tester,
      TargetPlatform.windows,
      const SingleChildScrollView(child: ReportTypesSettings()),
    );
    expect(_buttons(tester), isNot(contains('New report type…')));
    expect(find.text(reportTypesElsewhereText), findsOneWidget);
    _nothingPersonOnly(harness);
  });

  _windows('websites through pipes: none shared or stopped from here, and '
      "the other side's open elsewhere", (tester) async {
    final open = _pipe(PipeState.PIPE_STATE_OPEN);
    final mine = PipeShare(
      id: 's1',
      name: 'Dashboard',
      port: 3000,
      mine: true,
      live: true,
      pipeIds: const ['pp'],
    );
    final theirs = PipeShare(
      id: 's2',
      name: 'Staging',
      live: true,
      pipeIds: const ['pp'],
      address: 'http://staging.localhost:7421',
    );
    final workspace = _workspace(pipes: [open], shares: [mine, theirs]);
    await _pump(
      tester,
      TargetPlatform.macOS,
      PipeSharesField(pipe: open),
      workspace: workspace,
    );
    for (final label in ['Share a website…', 'Stop', 'Change end…']) {
      expect(_buttons(tester), contains(label), reason: 'Mac: $label');
    }
    final harness = await _pump(
      tester,
      TargetPlatform.windows,
      PipeSharesField(pipe: open),
      workspace: workspace,
    );
    for (final label in ['Share a website…', 'Stop', 'Change end…']) {
      expect(_buttons(tester), isNot(contains(label)), reason: label);
    }

    await _pump(
      tester,
      TargetPlatform.windows,
      ReceivedShare(pipe: open, share: theirs),
      workspace: workspace,
    );
    expect(_buttons(tester), isNot(contains('Open')));
    expect(_buttons(tester), isNot(contains('Copy address')));
    expect(find.text(openShareElsewhereText), findsOneWidget);
    _nothingPersonOnly(harness);
  });

  _windows('an agent is given the browser only elsewhere, and it can be '
      'taken away here', (tester) async {
    Agent june({required bool browser}) => Agent(
      id: 'june',
      name: 'June',
      projectId: 'p2',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      extensions: [AgentExtension(id: browserExtension, enabled: browser)],
    );
    Workspace workspace({required bool browser}) =>
        _workspace(agents: [june(browser: browser)])
          ..extensions.add(Extension(id: browserExtension, name: 'Browser'));
    Future<Harness> open(TargetPlatform platform, {required bool browser}) =>
        _pump(
          tester,
          platform,
          AgentSettingsForm(
            agent: june(browser: browser),
            close: () {},
            showExtensions: true,
          ),
          workspace: workspace(browser: browser),
        );
    Finder toggle() => find.descendant(
      of: find.ancestor(
        of: find.text('Browser'),
        matching: find.byWidgetPredicate(
          (widget) => widget.runtimeType.toString() == '_ExtensionSettings',
        ),
      ),
      matching: find.byType(Toggle),
    );

    await open(TargetPlatform.macOS, browser: false);
    expect(find.text(browserElsewhereText('June')), findsNothing);
    expect(tester.widget<Toggle>(toggle()).onChanged, isNotNull);

    var harness = await open(TargetPlatform.windows, browser: false);
    expect(find.text(browserElsewhereText('June')), findsOneWidget);
    expect(tester.widget<Toggle>(toggle()).onChanged, isNull);
    _nothingPersonOnly(harness);

    harness = await open(TargetPlatform.windows, browser: true);
    expect(find.text(browserElsewhereText('June')), findsNothing);
    await tester.ensureVisible(toggle());
    await tester.tap(toggle());
    await pumpFrames(tester);
    await tester.tap(find.widgetWithText(Button, 'Save'));
    await pumpFrames(tester);
    await tester.tap(find.text('Turn off').last);
    await pumpFrames(tester);
    final off = harness.backend.sent.lastWhere((m) => m.hasUpdateAgent());
    expect(off.updateAgent.extension_8.enabled, isFalse);
    _nothingPersonOnly(harness);
  });

  _windows("an errand's recovery codes are never asked for or kept here", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    final harness = Harness(
      _workspace()..agents.first.waitingSeqs.add(Int64(4)),
    );
    harness.backend.recoveryCodes = ['a8f2-91kd'];
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(4),
            timestampMs: Int64(1004),
            askPerson: AskPerson(
              id: 'ask4',
              title: 'github.com recovery codes',
              body:
                  'Claktory keeps them, sealed in its vault, until you '
                  'choose.',
              state: AskPersonState.ASK_PERSON_STATE_PENDING,
              revision: 1,
              handoff: ErrandHandoff(
                errandId: 'e1',
                kind: ErrandHandoffKind.ERRAND_HANDOFF_KIND_RECOVERY_CODES,
                origin: 'https://github.com',
                seq: Int64(4),
              ),
            ),
          ),
        ],
      ),
    );
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    await settle(tester);
    expect(find.byType(AskPersonCard), findsOneWidget);
    expect(find.text('a8f2-91kd'), findsNothing);
    for (final label in ['Keep in Secrets', 'I saved them', 'Copy', 'Show']) {
      expect(_buttons(tester), isNot(contains(label)), reason: label);
    }
    expect(find.text(recoveryCodesElsewhereText), findsOneWidget);
    _nothingPersonOnly(harness);
  });

  _windows("Needs you's recovery key row says where it's saved", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.windows;
    final harness = Harness(
      Workspace(
        settings: Settings(name: 'nixos'),
        vault: VaultState(
          exists: true,
          unlocked: true,
          recoveryKeyUnconfirmed: true,
        ),
      ),
    );
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await settle(tester);
    expect(find.text('Save the recovery key for nixos'), findsOneWidget);
    expect(
      find.textContaining('To save it, use your phone, tablet or Mac.'),
      findsOneWidget,
    );
    expect(find.textContaining('password manager'), findsNothing);
  });

  _windows("a proposal that would widen a pipe is only dismissed here", (
    tester,
  ) async {
    Proposal proposal({required bool widens}) => Proposal(
      id: '7',
      text: 'Give the Acme pipe the Web Site project.',
      grants: [
        ProposalGrant(summary: 'Widen the Acme pipe', widensPipe: widens),
      ],
    );
    // A Mac approves it, with a comment field (the control).
    await _pump(
      tester,
      TargetPlatform.macOS,
      ProposalCard(proposal(widens: true), agentId: 'a1'),
    );
    expect(find.text('Approve'), findsOneWidget);
    expect(find.byType(EditableText), findsOneWidget);
    // Windows approves one that widens no pipe as before.
    await _pump(
      tester,
      TargetPlatform.windows,
      ProposalCard(proposal(widens: false), agentId: 'a1'),
    );
    expect(find.text('Approve'), findsOneWidget);
    // One that widens a pipe: no Approve and no reply, only Dismiss and
    // where to approve it (authority::local_may refuses the rest).
    final harness = await _pump(
      tester,
      TargetPlatform.windows,
      ProposalCard(proposal(widens: true), agentId: 'a1'),
    );
    expect(find.text('Approve'), findsNothing);
    expect(find.byType(EditableText), findsNothing);
    expect(find.text(approveElsewhereText), findsOneWidget);
    expect(
      find.textContaining('Widen the Acme pipe', findRichText: true),
      findsOneWidget,
    );
    await tester.tap(find.text('Dismiss'));
    await pumpFrames(tester);
    final decided = harness.backend.sent.last.decideProposal;
    expect(decided.proposalId, '7');
    expect(decided.approve || decided.reply, isFalse);
  });
}
