import 'dart:async';
import 'dart:developer' as developer;
import 'dart:io';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:protobuf/protobuf.dart';

import 'app_update.dart';
import 'connections.dart';
import 'device_kind.dart' show thisDeviceKindForHello;
import 'protocol/factory/v1/factory.pb.dart';
import 'highlights.dart';
import 'state.dart';
import 'tunnel_client.dart';

/// Must match `PROTOCOL_VERSION` in the backend's factory-protocol crate.
const _protocolVersion = 1;
const _clientVersion = '0.1.0';

/// This Mac's Factory. Dev builds pass `--dart-define=FACTORY_PORT=7421` to
/// talk to the dev backend.
const _port = String.fromEnvironment('FACTORY_PORT', defaultValue: '7420');
const _localEndpoint = 'ws://127.0.0.1:$_port';

/// This Mac's Factory's own pairing code, which its backend keeps in its
/// data folder (the dev one's for dev builds): the app reaches it through
/// the private tunnel like any other, with this code's token. Null when
/// there's none, from a backend before the tunnel.
Future<RemoteFactory?> readLocalPairing() async {
  final home = Platform.environment['HOME'] ?? '';
  const folder = bool.fromEnvironment('FACTORY_DEV')
      ? '.factory-dev'
      : '.factory';
  try {
    final code = await File('$home/$folder/remote/local-pairing')
        .readAsString();
    return parsePairingCode(code);
  } on IOException {
    return null;
  }
}

const _minRetryDelay = Duration(milliseconds: 250);
const _maxRetryDelay = Duration(seconds: 5);

/// The private tunnel couldn't even start on this device.
class TunnelOpenFailure implements Exception {
  const TunnelOpenFailure(this.message);

  final String message;
}

/// A request the backend refused, or that couldn't reach it.
class RequestFailure implements Exception {
  const RequestFailure(
    this.message, {
    this.code = ErrorCode.ERROR_CODE_UNSPECIFIED,
  });

  final String message;

  /// What kind of refusal, when the backend said: the vault's ask for the
  /// passphrase rather than show an error.
  final ErrorCode code;

  @override
  String toString() => message;
}

/// Keeps one connection to a backend open, reconnecting with backoff when it
/// drops, and routes what the backend sends into the stores. The app keeps
/// one for each Claktory it's connected to, all live at once (rail.dart). Speaks the
/// protocol in proto/factory/v1/factory.proto.
class Backend {
  Backend({
    required this.workspace,
    required this.transcript,
    required this.explorer,
    required this.extensionView,
    QueuedStore? queued,
    Tunnels? tunnels,
    this.localBackend = true,
    Future<RemoteFactory?> Function()? localPairing,
  }) : queued = queued ?? QueuedStore(),
       _tunnels = tunnels ?? RustTunnels(),
       _localPairing = localPairing ?? readLocalPairing,
       // Made now, so it sees every snapshot from the first.
       highlights = HighlightStore(
         workspace: workspace,
         extensionView: extensionView,
       );

  final WorkspaceStore workspace;
  final TranscriptStore transcript;
  final ExplorerStore explorer;
  final ExtensionViewStore extensionView;
  final QueuedStore queued;

  /// What glows as state changes, and what's said beside it.
  final HighlightStore highlights;

  /// True from the backend's `welcome` until the connection drops.
  final connected = ValueNotifier<bool>(false);

  /// True while it reconnects after the app came back from the background
  /// or a connection that was up dropped (a network change, say), for up to
  /// [quietReconnect]: what shows stays, and nothing says it can't connect.
  final reconnecting = ValueNotifier<bool>(false);

  /// How long a reconnect stays quiet before the app says it can't connect.
  static const quietReconnect = Duration(seconds: 8);

  /// How long a new connection has to open and be welcomed.
  Duration connectWithin = const Duration(seconds: 10);

  Timer? _quiet;

  void _reconnectQuietly() {
    reconnecting.value = true;
    _quiet?.cancel();
    _quiet = Timer(quietReconnect, () => reconnecting.value = false);
  }

  /// When the app went to the background, while it's there.
  DateTime? _hiddenAt;

  /// The app went to the background: iOS suspends it soon, and the
  /// connection dies without saying so.
  void hidden() => _hiddenAt ??= DateTime.now();

  /// The app is back. After more than a moment away, the connection is
  /// made afresh at once (the old one is most likely dead), quietly, and
  /// the backend's snapshot brings the conversation up to date.
  void resumed({DateTime? now}) {
    final hiddenAt = _hiddenAt;
    _hiddenAt = null;
    if (hiddenAt == null ||
        (now ?? DateTime.now()).difference(hiddenAt) <
            const Duration(seconds: 3)) {
      return;
    }
    _restart();
  }

  /// Makes the connection afresh at once, quietly: the old one is most
  /// likely dead (the tunnel's too), and the backend's snapshot brings what
  /// shows up to date.
  void _restart() {
    _reconnectQuietly();
    unawaited(_tunnel?.reconnect());
    _retryDelay = _minRetryDelay;
    _redialed = _socket;
    unawaited(_socket?.close());
    _wakeUp();
  }

  /// The connection [_restart] closed, its tunnel already redialed.
  WebSocket? _redialed;

  /// How long the backend has to answer [check], and to welcome a new
  /// connection.
  Duration answerWithin = const Duration(seconds: 4);

  /// A [check] waiting for its answer.
  var _checking = false;

  /// Makes sure the connection is alive now, as when the window comes back
  /// to the front or the Mac wakes: a message the backend must answer
  /// within [answerWithin] (an empty one, which any backend answers as
  /// unknown); without an answer it reconnects. Not connected, it tries
  /// again at once rather than after its backoff.
  void check() {
    if (_closed || refused.value != null) return;
    final socket = _socket;
    if (socket == null || !connected.value) {
      _retryDelay = _minRetryDelay;
      _wakeUp();
      return;
    }
    if (_checking) return;
    _checking = true;
    final id = _nextId++;
    final answer = Completer<ServerMessage>();
    _pending[id] = answer;
    socket.add(ClientMessage(id: Int64(id)).writeToBuffer());
    answer.future
        .timeout(answerWithin)
        .then<void>(
          (_) {},
          onError: (Object error) {
            if (error is TimeoutException && identical(_socket, socket)) {
              _log('no answer within ${answerWithin.inSeconds}s: reconnecting');
              _restart();
            }
          },
        )
        .whenComplete(() {
          _pending.remove(id);
          _checking = false;
        });
  }

  /// Notices the Mac (or phone) slept: timers stop while it does, so the
  /// clock jumps ahead of them. Then the connection is checked at once.
  Timer? _sleepWatch;
  DateTime _lastTick = DateTime.now();
  static const _tick = Duration(seconds: 2);

  void _watchSleep() {
    _lastTick = DateTime.now();
    _sleepWatch ??= Timer.periodic(_tick, (_) {
      final now = DateTime.now();
      final slept = now.difference(_lastTick) > _tick * 4;
      _lastTick = now;
      if (slept) {
        _log('woke from sleep: checking the connection');
        check();
      }
    });
  }

  WebSocket? _socket;
  var _closed = false;
  var _nextId = 1;
  var _retryDelay = _minRetryDelay;
  final _pending = <int, Completer<ServerMessage>>{};

  /// The Factory on another machine this connects to; null for this Mac's.
  RemoteFactory? get target => _target;
  RemoteFactory? _target;

  /// Wakes the connection loop early: the target changed, or the person
  /// tries again after a refusal.
  Completer<void> _wake = Completer<void>();

  /// Why the Factory refused this device (it isn't paired with it any
  /// more); null while it hasn't. The app stops trying until the person
  /// picks a Factory again.
  final refused = ValueNotifier<String?>(null);

  /// On a Factory on another machine, the paired device this connection is
  /// (`Welcome.device_id`); empty on this Mac's.
  var deviceId = '';

  final Tunnels _tunnels;

  /// Reads this Mac's own pairing code, each time it connects: a backend
  /// that's been updated may have a new one.
  final Future<RemoteFactory?> Function() _localPairing;

  /// This app's key for this Mac's Factory: any key on this machine gets
  /// in with its code, so one made for each run.
  late final _localKey = newDeviceKey();

  /// Whether this device runs a Factory of its own to show when no other
  /// is chosen: a Mac does, a phone doesn't, and waits to be paired.
  final bool localBackend;

  /// This device's iroh key for a Factory reached through the tunnel; the
  /// app keeps one per pairing ([Connections.deviceKey]).
  String Function(RemoteFactory target) deviceKey = (_) => newDeviceKey();

  /// The private tunnel to [target], while it's reached through one; null
  /// otherwise. What it's doing: connected direct or through a relay, or
  /// why it can't connect.
  final tunnel = ValueNotifier<TunnelStatus?>(null);

  Tunnel? _tunnel;
  RemoteFactory? _tunnelTarget;
  Timer? _tunnelWatch;

  Future<void> run() async {
    _watchSleep();
    while (!_closed) {
      if (refused.value != null) {
        await _wake.future;
        continue;
      }
      final target = _target;
      if (target == null && !localBackend) {
        await _wake.future;
        continue;
      }
      // This Mac's Factory through the tunnel too, with its own code; the
      // open port is only for a backend from before it.
      final local = target == null ? await _localPairing() : null;
      final via = target ?? local;
      try {
        final WebSocket socket;
        if (via == null) {
          socket = await WebSocket.connect(_localEndpoint);
        } else if (via.tunnelled) {
          final tunnel = await _tunnelTo(
            via,
            key: target == null ? _localKey : null,
          );
          // Plain ws: iroh already encrypts it end to end, to the key the
          // pairing code names.
          // A dead connection inside the tunnel takes the handshake with
          // it: given up on, and the tunnel dials afresh.
          socket = await WebSocket.connect('ws://127.0.0.1:${tunnel.port}')
              .timeout(connectWithin);
        } else {
          socket = await WebSocket.connect(
            via.endpoint.toString(),
            customClient: via.client(),
          );
        }
        if (_closed || target != _target) {
          await socket.close();
          continue;
        }
        socket.pingInterval = const Duration(seconds: 5);
        _socket = socket;
        // Welcomed within [connectWithin], or it starts again.
        final welcome = Timer(connectWithin, () {
          if (identical(_socket, socket) && !connected.value) {
            _log('no welcome within ${connectWithin.inSeconds}s');
            unawaited(_tunnel?.reconnect());
            unawaited(socket.close());
          }
        });
        _watchTunnel();
        _send(
          ClientMessage(
            id: Int64(_nextId++),
            hello: Hello(
              protocolVersion: _protocolVersion,
              clientVersion: _clientVersion,
              token: via?.token,
              deviceKind: await thisDeviceKindForHello(),
            ),
          ),
        );
        try {
          await for (final frame in socket) {
            if (frame is List<int>) _receive(frame);
          }
        } finally {
          welcome.cancel();
        }
        _log('connection closed (code ${socket.closeCode})');
        // A live connection that dropped (unanswered pings, say): the
        // tunnel's connection under it is most likely dead too.
        if (connected.value &&
            !_closed &&
            via != null &&
            via.tunnelled &&
            !identical(_redialed, socket)) {
          unawaited(_tunnel?.reconnect());
        }
      } on HandshakeException catch (error) {
        _log("its certificate isn't the pinned one: $error");
      } on IOException catch (error) {
        _log('connection failed: $error');
      } on TunnelOpenFailure catch (failure) {
        _log("couldn't open the tunnel: ${failure.message}");
      } on TimeoutException {
        _log('no connection within ${connectWithin.inSeconds}s');
        unawaited(_tunnel?.reconnect());
      }
      _disconnected();
      _watchTunnel();
      // Turned away at the tunnel: like a refused Hello, trying again
      // won't help until the person pairs again.
      final status = tunnel.value;
      if (target != null &&
          target == _target &&
          status?.failure == TunnelFailure.refused) {
        refused.value = status!.error;
      }
      if (_closed) return;
      if (target != _target) continue;
      await Future.any([Future<void>.delayed(_retryDelay), _wake.future]);
      final doubled = _retryDelay * 2;
      _retryDelay = doubled > _maxRetryDelay ? _maxRetryDelay : doubled;
    }
  }

  /// Connects to [target] instead (null for this Mac's Factory). What the
  /// old one showed goes at once, so nothing of it shows as the new one's.
  void connectTo(RemoteFactory? target) {
    _target = target;
    refused.value = null;
    _retryDelay = _minRetryDelay;
    workspace.apply(Workspace());
    highlights.forgetKnown();
    transcript.load(Transcript());
    extensionView.apply(ExtensionView());
    queued.apply(QueuedMessages());
    unawaited(_socket?.close());
    if (target != _tunnelTarget) unawaited(_closeTunnel());
    _wakeUp();
  }

  /// [target] under another name the person gave it: the same Factory
  /// (pairing codes are equal), so the connection stays as it is.
  void relabel(RemoteFactory target) {
    if (target == _target) _target = target;
  }

  /// The tunnel to [target], opened the first time, as [key] (else this
  /// device's key for it).
  Future<Tunnel> _tunnelTo(RemoteFactory target, {String? key}) async {
    final open = _tunnel;
    if (open != null && _tunnelTarget == target) return open;
    await _closeTunnel();
    try {
      final opened = await _tunnels.open(
        deviceKey: key ?? deviceKey(target),
        endpointId: target.endpointId,
        relays: target.relays,
        addresses: target.addresses,
      );
      _tunnel = opened;
      _tunnelTarget = target;
      return opened;
    } catch (error) {
      tunnel.value = TunnelStatus(
        phase: TunnelPhase.failed,
        path: TunnelPath.none,
        error: "This Mac couldn't start the private tunnel: $error",
        failure: TunnelFailure.other,
        relayConnected: false,
      );
      throw TunnelOpenFailure('$error');
    }
  }

  /// Keeps [tunnel] current while one's open: whether it connects direct
  /// or through a relay can change at any time. It's read every second but
  /// set only when it changed, so nothing that shows it rebuilds while it
  /// stays the same.
  void _watchTunnel() {
    final open = _tunnel;
    if (open == null) return;
    _showTunnel(open.status);
    _tunnelWatch ??= Timer.periodic(const Duration(seconds: 1), (_) {
      final current = _tunnel;
      if (current != null) _showTunnel(current.status);
    });
  }

  void _showTunnel(TunnelStatus status) {
    final shown = tunnel.value;
    if (shown != null &&
        shown.phase == status.phase &&
        shown.path == status.path &&
        shown.error == status.error &&
        shown.failure == status.failure &&
        shown.relayConnected == status.relayConnected) {
      return;
    }
    tunnel.value = status;
  }

  Future<void> _closeTunnel() async {
    _tunnelWatch?.cancel();
    _tunnelWatch = null;
    final open = _tunnel;
    _tunnel = null;
    _tunnelTarget = null;
    tunnel.value = null;
    await open?.close();
  }

  /// After a refusal, tries the same Factory again.
  void retry() {
    refused.value = null;
    _retryDelay = _minRetryDelay;
    _wakeUp();
  }

  void _wakeUp() {
    if (!_wake.isCompleted) _wake.complete();
    _wake = Completer<void>();
  }

  Future<void> close() async {
    _closed = true;
    _quiet?.cancel();
    _sleepWatch?.cancel();
    _wakeUp();
    await _socket?.close();
    await _closeTunnel();
  }

  /// Creates a project. The backend names its folder after it.
  Future<void> createProject(String name) =>
      _request(ClientMessage(createProject: CreateProject(name: name)));

  Future<void> updateProject(UpdateProject update) =>
      _request(ClientMessage(updateProject: update));

  Future<void> deleteProject(String projectId) => _request(
    ClientMessage(deleteProject: DeleteProject(projectId: projectId)),
  );

  Future<void> createAgent(CreateAgent agent) =>
      _request(ClientMessage(createAgent: agent));

  Future<void> updateAgent(UpdateAgent update) =>
      _request(ClientMessage(updateAgent: update));

  Future<void> deleteAgent(String agentId) =>
      _request(ClientMessage(deleteAgent: DeleteAgent(agentId: agentId)));

  Future<void> selectAgent(String agentId) =>
      _request(ClientMessage(selectAgent: SelectAgent(agentId: agentId)));

  Future<void> sendMessage(String agentId, String text) => _request(
    ClientMessage(
      sendMessage: SendMessage(agentId: agentId, text: text),
    ),
  );

  Future<void> stopAgent(String agentId) =>
      _request(ClientMessage(stopAgent: StopAgent(agentId: agentId)));

  /// Approves or dismisses [agentId]'s proposal to Rivet. Approving sends
  /// Rivet [comment] and the proposal as the person's message; the entry
  /// comes back decided. Refused when it's already decided, or there's no
  /// Rivet that can run.
  Future<void> decideProposal(
    String agentId,
    String proposalId, {
    required bool approve,
    String comment = '',
  }) => _request(
    ClientMessage(
      decideProposal: DecideProposal(
        agentId: agentId,
        proposalId: proposalId,
        approve: approve,
        comment: comment,
      ),
    ),
  );

  /// Stops [agentId]'s background task [taskId]; refused when it isn't
  /// running.
  Future<void> stopTask(String agentId, String taskId) => _request(
    ClientMessage(
      stopTask: StopTask(agentId: agentId, taskId: taskId),
    ),
  );

  /// Pauses, resumes or cancels [agentId]'s monitor [monitorId], as the
  /// person; refused when its state doesn't allow it.
  Future<void> changeMonitor(
    String agentId,
    int monitorId,
    MonitorAction action,
  ) => _request(
    ClientMessage(
      changeMonitor: ChangeMonitor(
        agentId: agentId,
        monitorId: monitorId,
        action: action,
      ),
    ),
  );

  Future<void> updateSettings(UpdateSettings update) =>
      _request(ClientMessage(updateSettings: update));

  Future<void> restoreSettings() =>
      _request(ClientMessage(restoreSettings: RestoreSettings()));

  Future<void> startLogin(StartLogin start) =>
      _request(ClientMessage(startLogin: start));

  Future<void> answerLogin(String text) =>
      _request(ClientMessage(answerLogin: AnswerLogin(text: text)));

  Future<void> cancelLogin() =>
      _request(ClientMessage(cancelLogin: CancelLogin()));

  /// Signs out of [accountId] (`Account.id`), or of all the provider's
  /// accounts when it's empty.
  Future<void> signOut(String providerId, {String accountId = ''}) => _request(
    ClientMessage(
      signOut: SignOut(providerId: providerId, accountId: accountId),
    ),
  );

  /// Moves one of a provider's accounts to [position] (0 is first) in the
  /// order requests try them.
  Future<void> moveAccount(String providerId, String accountId, int position) =>
      _request(
        ClientMessage(
          moveAccount: MoveAccount(
            providerId: providerId,
            accountId: accountId,
            position: position,
          ),
        ),
      );

  /// Renames one of a provider's accounts (an API key, say), or tries an
  /// account that ran out of credit again.
  Future<void> updateAccount(UpdateAccount update) =>
      _request(ClientMessage(updateAccount: update));

  /// Opens secrets locked since a restart with the recovery key (or a
  /// passphrase the person set), when they couldn't open by themselves.
  Future<void> unlockVault(String key) =>
      _request(ClientMessage(unlockVault: UnlockVault(passphrase: key)));

  Future<void> createSecret(
    String name,
    String description,
    List<SecretEntry> entries,
  ) => _request(
    ClientMessage(
      createSecret: CreateSecret(
        name: name,
        description: description,
        entries: entries,
      ),
    ),
  );

  /// Renames or redescribes a secret; [entries], when given, replace its
  /// variables and values.
  Future<void> updateSecret(
    String id,
    String name,
    String description, [
    List<SecretEntry> entries = const [],
  ]) => _request(
    ClientMessage(
      updateSecret: UpdateSecret(
        id: id,
        name: name,
        description: description,
        entries: entries,
      ),
    ),
  );

  Future<void> deleteSecret(String id) =>
      _request(ClientMessage(deleteSecret: DeleteSecret(id: id)));

  Future<void> grantSecret(String id, String agentId) => _request(
    ClientMessage(
      grantSecret: GrantSecret(id: id, agentId: agentId),
    ),
  );

  Future<void> revokeSecret(
    String id,
    String agentId, {
    bool stopRunningTasks = false,
  }) => _request(
    ClientMessage(
      revokeSecret: RevokeSecret(
        id: id,
        agentId: agentId,
        stopRunningTasks: stopRunningTasks,
      ),
    ),
  );

  /// A secret's values, to this device only. Never keep them.
  Future<List<SecretEntry>> revealSecret(String id) async {
    final reply = await _call(
      ClientMessage(revealSecret: RevealSecret(id: id)),
    );
    if (!reply.hasSecretValue()) {
      throw const RequestFailure('Claktory sent no value.');
    }
    return reply.secretValue.entries;
  }

  /// The variables a .env's [text] holds (names and hints, no values),
  /// and what's wrong with it.
  Future<EnvPreview> previewEnv(String text) async {
    final reply = await _call(
      ClientMessage(previewEnv: PreviewEnv(text: text)),
    );
    if (!reply.hasEnvPreview()) {
      throw const RequestFailure('Claktory sent no preview.');
    }
    return reply.envPreview;
  }

  Future<void> importEnv(
    String text,
    String name,
    String description, {
    bool onePerVariable = false,
  }) => _request(
    ClientMessage(
      importEnv: ImportEnv(
        text: text,
        name: name,
        description: description,
        onePerVariable: onePerVariable,
      ),
    ),
  );

  /// Gives an agent one of the person's secrets ([secretId]), or a new one
  /// made from [name], [description] and [entries], for its request; or
  /// dismisses the request ([approve] false), telling it [comment]. A request
  /// from before agents described what they need names its secret, and
  /// approving it needs neither.
  Future<void> decideSecretRequest(
    String agentId,
    String requestId, {
    required bool approve,
    List<SecretEntry> entries = const [],
    String comment = '',
    String secretId = '',
    String name = '',
    String description = '',
  }) => _request(
    ClientMessage(
      decideSecretRequest: DecideSecretRequest(
        agentId: agentId,
        requestId: requestId,
        approve: approve,
        entries: entries,
        comment: comment,
        secretId: secretId,
        name: name,
        description: description,
      ),
    ),
  );

  /// How [id] (empty: every secret) was used, given and shown, newest
  /// first.
  Future<List<SecretUse>> listSecretUses(String id, {int limit = 0}) async {
    final reply = await _call(
      ClientMessage(
        listSecretUses: ListSecretUses(id: id, limit: limit),
      ),
    );
    if (!reply.hasSecretUses()) {
      throw const RequestFailure('Claktory sent no uses.');
    }
    return reply.secretUses.uses;
  }

  /// Emails a code to sign this Claktory in to [email].
  Future<void> accountSignIn(String email) =>
      _request(ClientMessage(accountSignIn: AccountSignIn(email: email)));

  /// The emailed code: signs this Claktory in.
  Future<void> accountVerify(String code) =>
      _request(ClientMessage(accountVerify: AccountVerify(code: code)));

  /// Emails a new code to the same address.
  Future<void> accountResend() =>
      _request(ClientMessage(accountResend: AccountResend()));

  Future<void> accountSignOut() =>
      _request(ClientMessage(accountSignOut: AccountSignOut()));

  /// The account service to use; empty for claktory.com's.
  Future<void> setAccountService(String url) =>
      _request(ClientMessage(setAccountService: SetAccountService(url: url)));

  /// The vault's recovery key, to this device only. Never keep it.
  Future<String> showRecoveryKey() async {
    final reply = await _call(
      ClientMessage(showRecoveryKey: ShowRecoveryKey()),
    );
    if (!reply.hasRecoveryKey()) {
      throw const RequestFailure('Claktory sent no recovery key.');
    }
    return reply.recoveryKey.key;
  }

  /// Empties the build cache at [path].
  Future<void> clearBuildCache(String path) =>
      _request(ClientMessage(clearBuildCache: ClearBuildCache(path: path)));

  /// Installs the latest release. With [appPath], the backend replaces the app
  /// bundle there too, once this app quits.
  Future<void> installUpdate(String appPath) =>
      _request(ClientMessage(installUpdate: InstallUpdate(appPath: appPath)));

  /// A piece of release [version]'s app archive from [offset], to update
  /// this app from a Factory on another machine.
  Future<AppPiece> fetchApp(String version, int offset) async {
    final reply = await _call(
      ClientMessage(
        fetchApp: FetchApp(version: version, offset: Int64(offset)),
      ),
    );
    if (!reply.hasAppChunk()) {
      throw const RequestFailure('Claktory sent no piece of the app.');
    }
    final chunk = reply.appChunk;
    return AppPiece(
      preparing: chunk.preparing,
      size: chunk.size.toInt(),
      sha256: chunk.sha256,
      appName: chunk.appName,
      data: chunk.data,
    );
  }

  /// Focuses a project; an empty [projectId] leaves focus.
  Future<void> focusProject(String projectId) =>
      _request(ClientMessage(focusProject: FocusProject(projectId: projectId)));

  /// Opens or closes a folder in the project's own folder, or with
  /// [folderId] in that one of the project's folders.
  Future<void> expandFolder(
    String projectId,
    String path,
    bool expanded, {
    String folderId = '',
    String agentId = '',
  }) => _request(
    ClientMessage(
      expandFolder: ExpandFolder(
        projectId: projectId,
        path: path,
        expanded: expanded,
        folderId: folderId.isEmpty ? null : folderId,
        agentId: agentId.isEmpty ? null : agentId,
      ),
    ),
  );

  Future<void> showAllFiles(String projectId, bool showAll) => _request(
    ClientMessage(
      showAllFiles: ShowAllFiles(projectId: projectId, showAll: showAll),
    ),
  );

  /// Places an agent in the sidebar: under [ArrangeAgent.parentId], before
  /// [ArrangeAgent.beforeId], and in another project when that's set.
  Future<void> arrangeAgent(ArrangeAgent arrange) =>
      _request(ClientMessage(arrangeAgent: arrange));

  /// Collapses or expands the agents nested under an agent.
  Future<void> setAgentCollapsed(String agentId, bool collapsed) => _request(
    ClientMessage(
      setAgentCollapsed: SetAgentCollapsed(
        agentId: agentId,
        collapsed: collapsed,
      ),
    ),
  );

  Future<void> createFolder(CreateFolder folder) =>
      _request(ClientMessage(createFolder: folder));

  Future<void> updateFolder(UpdateFolder update) =>
      _request(ClientMessage(updateFolder: update));

  Future<void> deleteFolder(String projectId, String folderId) => _request(
    ClientMessage(
      deleteFolder: DeleteFolder(projectId: projectId, folderId: folderId),
    ),
  );

  /// Turns highlights on or off for the person (`Workspace.highlights`).
  Future<void> setHighlights(bool on) =>
      _request(ClientMessage(setHighlights: SetHighlights(on: on)));

  /// Folds a project folder's agents away in the sidebar, or shows them.
  Future<void> setFolderCollapsed(
    String projectId,
    String folderId,
    bool collapsed,
  ) => _request(
    ClientMessage(
      setFolderCollapsed: SetFolderCollapsed(
        projectId: projectId,
        folderId: folderId,
        collapsed: collapsed,
      ),
    ),
  );

  /// Runs one of an agent's slash commands (`Agent.commands`), such as
  /// "clear", with what was typed after its name.
  Future<void> runCommand(String agentId, String name, String args) => _request(
    ClientMessage(
      runCommand: RunCommand(agentId: agentId, name: name, args: args),
    ),
  );

  /// Opens an extension's panel beside [agentId], which remembers it; an
  /// empty [extensionId] closes it.
  Future<void> openPanel(String agentId, String extensionId) => _request(
    ClientMessage(
      openPanel: OpenPanel(agentId: agentId, extensionId: extensionId),
    ),
  );

  /// Pairs a new device named [name], reaching this Factory by [host] (one
  /// of `RemoteAccess.addresses`, or empty for the first). Answers its
  /// pairing code, which is shown only this once.
  Future<PairingCode> createPairing(String name, String host) async {
    final reply = await _call(
      ClientMessage(
        createPairing: CreatePairing(name: name, host: host),
      ),
    );
    if (!reply.hasPairingCode()) {
      throw const RequestFailure('Claktory sent no pairing code.');
    }
    return reply.pairingCode;
  }

  /// The folders in [path] on the machine Factory runs on: absolute, or
  /// from the home folder ("", "~" or "~/code").
  Future<FolderList> listFolders(String path) async {
    final reply = await _call(
      ClientMessage(listFolders: ListFolders(path: path)),
    );
    if (!reply.hasFolderList()) {
      throw const RequestFailure('Claktory sent no folders.');
    }
    return reply.folderList;
  }

  /// The file at [path] (absolute or "~/…"), or its start when it's long; a
  /// file that can't be read comes back with `error` set.
  Future<FileContent> readFile(String path, {int maxBytes = 0}) async {
    final reply = await _call(
      ClientMessage(
        readFile: ReadFile(path: path, maxBytes: Int64(maxBytes)),
      ),
    );
    if (!reply.hasFileContent()) {
      throw const RequestFailure('Claktory sent no file.');
    }
    return reply.fileContent;
  }

  /// Lays out [agentId]'s panes; null (or a lone chat leaf) clears it.
  Future<void> setLayout(String agentId, ViewNode? root) => _request(
    ClientMessage(
      setLayout: SetLayout(agentId: agentId, root: root),
    ),
  );

  /// Unpairs a device: it can't connect any more.
  Future<void> unpairDevice(String deviceId) =>
      _request(ClientMessage(unpairDevice: UnpairDevice(deviceId: deviceId)));

  /// Turns remote access on or off; [port] 0 keeps it, and [listen] (an
  /// address to listen on only, or empty for every one) is kept when null.
  Future<void> setRemoteAccess(bool on, {int port = 0, String? listen}) =>
      _request(
        ClientMessage(
          setRemoteAccess: SetRemoteAccess(on: on, port: port, listen: listen),
        ),
      );

  /// Sends a request and completes when the backend acknowledges it, or
  /// throws [RequestFailure] if it refuses or the connection drops first.
  Future<void> _request(ClientMessage message) => _call(message);

  /// Sends a request and answers the backend's reply to it.
  Future<ServerMessage> _call(ClientMessage message) {
    if (!connected.value) {
      return Future.error(
        const RequestFailure('Not connected to the backend.'),
      );
    }
    final id = _nextId++;
    message.id = Int64(id);
    final completer = Completer<ServerMessage>();
    _pending[id] = completer;
    _send(message);
    return completer.future;
  }

  void _send(ClientMessage message) => _socket?.add(message.writeToBuffer());

  void _receive(List<int> frame) {
    final ServerMessage message;
    try {
      message = ServerMessage.fromBuffer(frame);
    } on InvalidProtocolBufferException catch (error) {
      _log('undecodable frame: ${error.message}');
      return;
    }
    switch (message.whichBody()) {
      case ServerMessage_Body.welcome:
        _retryDelay = _minRetryDelay;
        _quiet?.cancel();
        reconnecting.value = false;
        deviceId = message.welcome.deviceId;
        connected.value = true;
        _log('connected to factoryd ${message.welcome.serverVersion}');
      case ServerMessage_Body.ack:
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.error:
        if (message.error.code == ErrorCode.ERROR_CODE_UNAUTHORIZED) {
          // Not (or no longer) a paired device: retrying won't help.
          refused.value = message.error.message;
        }
        final pending = _pending.remove(message.replyTo.toInt());
        if (pending != null) {
          pending.completeError(
            RequestFailure(message.error.message, code: message.error.code),
          );
        } else {
          _log('error ${message.error.code.name}: ${message.error.message}');
        }
      case ServerMessage_Body.workspace:
        workspace.apply(message.workspace);
      case ServerMessage_Body.transcript:
        transcript.load(message.transcript);
      case ServerMessage_Body.entry:
        transcript.update(message.entry);
      case ServerMessage_Body.delta:
        transcript.delta(message.delta);
      case ServerMessage_Body.explorer:
        explorer.apply(message.explorer);
      case ServerMessage_Body.extensionView:
        extensionView.apply(message.extensionView);
      case ServerMessage_Body.queuedMessages:
        queued.apply(message.queuedMessages);
      case ServerMessage_Body.highlight:
        highlights.apply(message.highlight);
      case ServerMessage_Body.pairingCode:
      case ServerMessage_Body.appChunk:
      case ServerMessage_Body.folderList:
      case ServerMessage_Body.fileContent:
      case ServerMessage_Body.vaultSession:
      case ServerMessage_Body.secretValue:
      case ServerMessage_Body.envPreview:
      case ServerMessage_Body.recoveryKey:
      case ServerMessage_Body.secretUses:
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.notSet:
        // A message kind from a newer backend; nothing to do with it yet.
        break;
    }
  }

  void _disconnected() {
    _socket = null;
    // A connection that was up and dropped is most likely a network change:
    // it comes back without a word, if it can.
    if (connected.value && !_closed) _reconnectQuietly();
    connected.value = false;
    final pending = _pending.values.toList();
    _pending.clear();
    for (final completer in pending) {
      completer.completeError(const RequestFailure('Connection lost.'));
    }
  }
}

/// Sends a request nothing waits on, logging it if the backend refuses.
void fireAndLog(Future<void> request) {
  request.catchError(
    (Object error) => _log('request failed: $error'),
    test: (error) => error is RequestFailure,
  );
}

void _log(String message) => developer.log(message, name: 'factory.backend');
