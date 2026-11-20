import 'dart:async';
import 'dart:developer' as developer;
import 'dart:io';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:protobuf/protobuf.dart';

import 'annotations.dart' show ThreadStore;
import 'app_update.dart';
import 'attachments.dart';
import 'connections.dart';
import 'loopback_forward.dart';
import 'device_kind.dart' show thisDeviceKindForHello;
import 'protocol/factory/v1/factory.pb.dart';
import 'highlights.dart';
import 'outbox.dart';
import 'pairing.dart';
import 'state.dart';
import 'tunnel_client.dart';
import 'waiting.dart' show EntryReveal;
import 'workspace_patch.dart';

/// Must match `PROTOCOL_VERSION` in the backend's factory-protocol crate.
const _protocolVersion = 1;
const _clientVersion = '0.1.0';

/// This Mac's Factory's port. Only a release build talks to the real one
/// (7420) unless told otherwise; debug and profile builds and tests default
/// to the dev one (7421), so a stray run never knocks on the person's
/// Claktory. `--dart-define=FACTORY_PORT=…` picks one either way.
@visibleForTesting
const localPort = String.fromEnvironment(
  'FACTORY_PORT',
  defaultValue: kReleaseMode ? '7420' : '7421',
);
const _localEndpoint = 'ws://127.0.0.1:$localPort';

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

  /// Files waiting in the agents' message boxes, on this device.
  final attachments = AttachmentDrafts();

  /// The person's messages on their way, shown in the conversation at once.
  late final outbox = Outbox(BackendOutbox(this));

  /// What glows as state changes, and what's said beside it.
  final HighlightStore highlights;

  /// Comments on files, by path, as the backend last sent them
  /// (annotations.dart).
  final threads = ThreadStore();

  /// Where the next conversation shown opens, when it's not at its end:
  /// at what waits for the person (waiting.dart).
  final reveal = EntryReveal();

  /// The last file in any agent's panes the backend said changed.
  final fileChanged = ValueNotifier<FileChanged?>(null);

  /// True from the backend's `welcome` until the connection drops.
  final connected = ValueNotifier<bool>(false);

  /// Whether to ask for Workspace patches; off, the backend sends whole
  /// Workspaces, as to apps before them (to compare the two in tests).
  bool askForPatches = true;

  /// Whether this connection has had a whole Workspace, which its patches
  /// change: the first state after connecting always is one.
  var _wholeWorkspace = false;

  /// When the backend last welcomed this connection; null while it's down.
  final connectedSince = ValueNotifier<DateTime?>(null);

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

  /// The Claktory's name and color as it said on connecting
  /// (`Welcome.name`, `Welcome.color`), until its Settings carry them.
  var welcomeName = '';
  var welcomeColor = '';

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

  /// The ports this device forwards to the backend's machine, for browser
  /// flows that come back to localhost there (loopback_forward.dart).
  late final forwards = LoopbackForwards(
    remote: () => target != null,
    forward: forwardPort,
  );

  /// Listens on [port] of this device's loopback and carries each
  /// connection through the tunnel to the same port on the backend's
  /// machine (throws [ForwardFailure]).
  Future<PortForward> forwardPort(int port) async {
    final open = _tunnel;
    if (open == null) {
      throw const ForwardFailure(
        ForwardProblem.failed,
        'Not connected to it right now.',
      );
    }
    return open.forwardPort(port);
  }

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
        // Welcomed within [connectWithin], or it starts again; a device
        // waiting to be allowed waits as long as its code lasts.
        final welcome = _welcomeTimer = Timer(connectWithin, () {
          if (identical(_socket, socket) && !connected.value) {
            _log('no welcome within ${connectWithin.inSeconds}s');
            unawaited(_tunnel?.reconnect());
            unawaited(socket.close());
          }
        });
        _watchTunnel();
        _helloVia = via;
        _helloKey = via == null || !via.tunnelled
            ? null
            : target == null
            ? _localKey
            : deviceKey(via);
        _send(
          ClientMessage(
            id: Int64(_nextId++),
            hello: Hello(
              protocolVersion: _protocolVersion,
              clientVersion: _clientVersion,
              token: _helloVia?.token,
              deviceKind: await thisDeviceKindForHello(),
              // Changes come as patches to the last Workspace, not whole.
              workspacePatches: askForPatches,
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

  /// The connection's Welcome timeout, cancelled while it waits to be
  /// allowed.
  Timer? _welcomeTimer;

  /// What the current connection's Hello was sent to, and with which
  /// device key: the pairing number is worked out from them.
  RemoteFactory? _helloVia;
  String? _helloKey;

  /// This device used a pairing code and waits for the person to allow it
  /// on that Claktory (PairingPending), with the number to compare; null
  /// otherwise. It stays set when the device is turned away ([refused]
  /// says why), until it connects or another Claktory is picked.
  final pairing = ValueNotifier<PairingWait?>(null);

  Future<void> _pairingPending(PairingPending pending) async {
    _welcomeTimer?.cancel();
    final via = _helloVia;
    final key = _helloKey;
    var number = '';
    if (via != null && key != null) {
      try {
        final ids = await _tunnels.pairingIds(
          deviceKey: key,
          endpointId: via.endpointId,
        );
        number = pairingNumber(
          backendId: ids.backend,
          deviceId: ids.device,
          token: via.token,
        );
      } catch (error) {
        _log("can't work out the pairing number: $error");
      }
    }
    if (_closed || !identical(via, _helloVia)) return;
    pairing.value = PairingWait(pending, number);
  }

  /// Connects to [target] instead (null for this Mac's Factory). What the
  /// old one showed goes at once, so nothing of it shows as the new one's.
  void connectTo(RemoteFactory? target) {
    _target = target;
    refused.value = null;
    pairing.value = null;
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
    // Its forwards go with it.
    if (open != null) await forwards.stopAll();
    await open?.close();
  }

  /// Reconnects at once, when the person asks: a live connection is made
  /// afresh (the tunnel's too), and one that's down tries again now rather
  /// than after its backoff.
  void reconnectNow() {
    if (_closed) return;
    if (refused.value != null) return retry();
    if (connected.value) return _restart();
    unawaited(_tunnel?.reconnect());
    _retryDelay = _minRetryDelay;
    _wakeUp();
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
    await forwards.stopAll();
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

  /// Sends the person's message; [clientId] comes back on the queued
  /// message and the entry it becomes (see [outbox]).
  Future<void> sendMessage(
    String agentId,
    String text, {
    String clientId = '',
    List<Attachment> attachments = const [],
  }) => _request(
    ClientMessage(
      sendMessage: SendMessage(
        agentId: agentId,
        text: text,
        clientId: clientId,
        attachments: attachments,
      ),
    ),
  );

  /// Starts sending a file to [agentId]'s scratch space as [uploadId], or
  /// picks it up where it stopped: answers how many of its bytes the
  /// backend has.
  Future<int> startUpload(
    String uploadId,
    String agentId, {
    required String name,
    required int size,
    String mime = '',
  }) async {
    final reply = await _call(
      ClientMessage(
        startUpload: StartUpload(
          uploadId: uploadId,
          agentId: agentId,
          name: name,
          size: Int64(size),
          mime: mime,
        ),
      ),
    );
    return reply.uploadProgress.received.toInt();
  }

  /// Sends the file's [data] from [offset]: answers how many of its bytes
  /// the backend has.
  Future<int> uploadChunk(String uploadId, int offset, List<int> data) async {
    final reply = await _call(
      ClientMessage(
        uploadChunk: UploadChunk(
          uploadId: uploadId,
          offset: Int64(offset),
          data: data,
        ),
      ),
    );
    return reply.uploadProgress.received.toInt();
  }

  /// The whole file is there: answers where the backend put it, to send
  /// with the message.
  Future<Attachment> finishUpload(String uploadId) async {
    final reply = await _call(
      ClientMessage(finishUpload: FinishUpload(uploadId: uploadId)),
    );
    if (!reply.hasUploadedFile()) {
      throw const RequestFailure('Claktory sent nothing back for the file.');
    }
    return reply.uploadedFile.attachment;
  }

  /// Drops what the backend has of [uploadId].
  Future<void> cancelUpload(String uploadId) =>
      _request(ClientMessage(cancelUpload: CancelUpload(uploadId: uploadId)));

  /// Loads the entries before the first one [transcript] holds, when it's
  /// [agentId]'s: the backend sends only the latest at first. Throws
  /// [RequestFailure] when it can't.
  Future<void> loadEarlier(String agentId) => _loadBefore(agentId);

  /// Loads every entry from [seq] up to the first one [transcript] holds,
  /// when it's [agentId]'s, so the view can go to an older entry with no
  /// gap after it. Throws [RequestFailure] when it can't.
  Future<void> loadFrom(String agentId, int seq) =>
      _loadBefore(agentId, from: seq);

  Future<void> _loadBefore(String agentId, {int? from}) async {
    if (transcript.agentId != agentId || transcript.length == 0) return;
    final loads = transcript.loads;
    final first = transcript[0].entry.seq;
    if (from != null && from >= first.toInt()) return;
    final reply = await _call(
      ClientMessage(
        loadEntries: LoadEntries(
          agentId: agentId,
          beforeSeq: first,
          fromSeq: from == null ? null : Int64(from),
        ),
      ),
    );
    // A new snapshot meanwhile starts somewhere else: this page may not
    // join onto it.
    if (transcript.loads != loads ||
        transcript.length == 0 ||
        transcript[0].entry.seq != first) {
      return;
    }
    final page = reply.entryPage;
    transcript.prepend(page.agentId, page.entries, hasEarlier: page.hasEarlier);
  }

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
    Map<String, String> aliases = const {},
    bool asIs = false,
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
        aliases: aliases.entries,
        asIs: asIs,
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

  /// Starts connecting a service (docs/CONNECTORS.md): the flow, and the
  /// page to open on this device. Only this device gets it.
  Future<ConnectStarted> startConnect(StartConnect start) async {
    final reply = await _call(ClientMessage(startConnect: start));
    if (!reply.hasConnectStarted()) {
      throw const RequestFailure('Claktory sent no page to open.');
    }
    return reply.connectStarted;
  }

  /// The whole address the browser came back to, as it is (BROWSER), or
  /// the token the person pasted (PASTE).
  Future<void> finishConnect(
    String flowId,
    String callbackUrl, {
    String token = '',
  }) => _request(
    ClientMessage(
      finishConnect: FinishConnect(
        flowId: flowId,
        callbackUrl: callbackUrl,
        token: token,
      ),
    ),
  );

  /// Stops a flow, or takes an ended one off the list.
  Future<void> cancelConnect(String flowId) =>
      _request(ClientMessage(cancelConnect: CancelConnect(flowId: flowId)));

  Future<void> dismissConnectProposal(
    String proposalId, {
    String comment = '',
  }) => _request(
    ClientMessage(
      dismissConnectProposal: DismissConnectProposal(
        proposalId: proposalId,
        comment: comment,
      ),
    ),
  );

  /// What a secret can be made of from the connection [secretId]: its
  /// accounts, zones, roles and permissions, and how to pick.
  Future<ConnectChoices> listConnectChoices(String secretId) async {
    final reply = await _call(
      ClientMessage(listConnectChoices: ListConnectChoices(secretId: secretId)),
    );
    if (!reply.hasConnectChoices()) {
      throw const RequestFailure('Claktory sent no choices.');
    }
    return reply.connectChoices;
  }

  /// Uses one of an account's usage resets, as the person asked (never on
  /// its own): how it went, as the provider said. The account's usage is
  /// read again either way.
  Future<UsageResetAnswer> useUsageReset(UseUsageReset use) async {
    final reply = await _call(ClientMessage(useUsageReset: use));
    if (!reply.hasUsageResetAnswer()) {
      throw const RequestFailure('Claktory sent no answer.');
    }
    return reply.usageResetAnswer;
  }

  /// Makes a secret from a connection, with only the access asked for.
  Future<void> deriveSecret(DeriveSecret derive) =>
      _request(ClientMessage(deriveSecret: derive));

  /// Removes a connection and the secrets made from it; with [revoke], at
  /// the service too.
  Future<void> disconnectService(String secretId, {bool revoke = false}) =>
      _request(
        ClientMessage(
          disconnectService: DisconnectService(
            secretId: secretId,
            revoke: revoke,
          ),
        ),
      );

  /// How many tokens went where over a range (the Usage pane), to this
  /// device only.
  Future<UsageReport> loadUsage(LoadUsage ask) async {
    final reply = await _call(ClientMessage(loadUsage: ask));
    if (!reply.hasUsageReport()) {
      throw const RequestFailure('Claktory sent no usage.');
    }
    return reply.usageReport;
  }

  /// Installs an app from a .clapp's [content]: without [approve], what it
  /// can reach, for the person to review; approved, with that review's
  /// [sha256], installed (and null).
  Future<AppPermissions?> installLocalApp(
    List<int> content, {
    bool approve = false,
    String sha256 = '',
  }) async {
    final message = ClientMessage(
      installLocalApp: InstallLocalApp(
        content: content,
        approve: approve,
        sha256: sha256,
      ),
    );
    if (approve) {
      await _request(message);
      return null;
    }
    final reply = await _call(message);
    if (!reply.hasAppPermissions()) {
      throw const RequestFailure('Claktory sent nothing to review.');
    }
    return reply.appPermissions;
  }

  Future<void> setAppRunning(String id, bool running) => _request(
    ClientMessage(
      setAppRunning: SetAppRunning(id: id, running: running),
    ),
  );

  Future<void> uninstallApp(String id, {bool keepData = false}) => _request(
    ClientMessage(
      uninstallApp: UninstallApp(id: id, keepData: keepData),
    ),
  );

  /// Gives [agentId] these [scopes] of the app, in place of what it had;
  /// none takes the app away from it.
  Future<void> grantApp(String appId, String agentId, List<String> scopes) =>
      _request(
        ClientMessage(
          grantApp: GrantApp(appId: appId, agentId: agentId, scopes: scopes),
        ),
      );

  /// The agents that hear the app's events of [eventKind], in place of
  /// those that did.
  Future<void> subscribeApp(
    String appId,
    String eventKind,
    List<String> agentIds,
  ) => _request(
    ClientMessage(
      subscribeApp: SubscribeApp(
        appId: appId,
        eventKind: eventKind,
        agentIds: agentIds,
      ),
    ),
  );

  /// One of the secrets the app's setup asks for.
  Future<void> setAppSecret(String appId, String name, String value) =>
      _request(
        ClientMessage(
          setAppSecret: SetAppSecret(appId: appId, name: name, value: value),
        ),
      );

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

  /// The project whose Files this device shows on their own (the phone's
  /// Files page), or empty: the backend sends its Explorer meanwhile, on
  /// this connection and the next.
  Future<void> watchFiles(String projectId) {
    _watching = projectId;
    return _request(
      ClientMessage(watchFiles: WatchFiles(projectId: projectId)),
    );
  }

  var _watching = '';

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

  /// Zooms the whole app on this device ([zoom] 1 is actual size): kept
  /// for this device only (`Settings.zoom`, see zoom.dart).
  Future<void> setZoom(double zoom) =>
      _request(ClientMessage(setZoom: SetZoom(zoom: zoom)));

  /// Gives the person's avatar new colors (Workspace.person), the same on
  /// every device.
  Future<void> shuffleAvatar() =>
      _request(ClientMessage(shuffleAvatar: ShuffleAvatar()));

  /// This device's key in `Settings.zoom`: its paired id, or "local" for
  /// this machine's own app.
  String get zoomKey => deviceId.isEmpty ? 'local' : deviceId;

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

  /// The file at [path] (absolute or "~/…"), or its start when it's long,
  /// or from [offset] on (a video, a chunk at a time); a file that can't be
  /// read comes back with `error` set.
  Future<FileContent> readFile(
    String path, {
    int maxBytes = 0,
    int offset = 0,
  }) async {
    final reply = await _call(
      ClientMessage(
        readFile: ReadFile(
          path: path,
          maxBytes: Int64(maxBytes),
          offset: Int64(offset),
        ),
      ),
    );
    if (!reply.hasFileContent()) {
      throw const RequestFailure('Claktory sent no file.');
    }
    return reply.fileContent;
  }

  /// Asks for the comments on [path]; they arrive in [threads].
  Future<void> listThreads(String path) =>
      _call(ClientMessage(listThreads: ListThreads(path: path)));

  /// Comments on a passage of [path] (see annotations.dart); answers the
  /// new thread, also put in [threads].
  Future<Thread> createThread(CreateThread thread) async {
    final reply = await _call(ClientMessage(createThread: thread));
    if (!reply.hasThread()) {
      throw const RequestFailure('Claktory sent no comment.');
    }
    return reply.thread;
  }

  Future<void> replyThread(
    String threadId,
    String body, {
    bool sendNow = false,
  }) => _request(
    ClientMessage(
      replyThread: ReplyThread(
        threadId: threadId,
        body: body,
        sendNow: sendNow,
      ),
    ),
  );

  Future<void> editComment(String commentId, String body) => _request(
    ClientMessage(
      editComment: EditComment(commentId: commentId, body: body),
    ),
  );

  Future<void> deleteComment(String commentId) => _request(
    ClientMessage(deleteComment: DeleteComment(commentId: commentId)),
  );

  Future<void> setThreadStatus(String threadId, {required bool resolved}) =>
      _request(
        ClientMessage(
          setThreadStatus: SetThreadStatus(
            threadId: threadId,
            resolved: resolved,
          ),
        ),
      );

  Future<void> readdressThread(String threadId, String agentId) => _request(
    ClientMessage(
      readdressThread: ReaddressThread(threadId: threadId, agentId: agentId),
    ),
  );

  Future<void> reattachThread(ReattachThread reattach) =>
      _request(ClientMessage(reattachThread: reattach));

  /// Sends [agentId] every draft comment on [path] as one message.
  Future<void> sendComments(String agentId, String path) => _request(
    ClientMessage(
      sendComments: SendComments(agentId: agentId, path: path),
    ),
  );

  /// Accepts a suggested change (Claktory makes it in the file) or rejects
  /// it; either resolves its thread.
  Future<void> decideSuggestion(String threadId, {required bool accept}) =>
      _request(
        ClientMessage(
          decideSuggestion: DecideSuggestion(
            threadId: threadId,
            accept: accept,
          ),
        ),
      );

  /// Lays out [agentId]'s panes; null (or a lone chat leaf) clears it.
  Future<void> setLayout(String agentId, ViewNode? root) => _request(
    ClientMessage(
      setLayout: SetLayout(agentId: agentId, root: root),
    ),
  );

  /// Unpairs a device: it can't connect any more.
  Future<void> unpairDevice(String deviceId) =>
      _request(ClientMessage(unpairDevice: UnpairDevice(deviceId: deviceId)));

  /// Lets in the device waiting with [deviceId]'s code
  /// (RemoteAccess.pending_devices): it's paired from now on.
  Future<void> approveDevice(String deviceId) =>
      _request(ClientMessage(approveDevice: ApproveDevice(deviceId: deviceId)));

  /// Turns away the device waiting with [deviceId]'s code: the code stops
  /// working, and a new one is needed to try again.
  Future<void> denyDevice(String deviceId) =>
      _request(ClientMessage(denyDevice: DenyDevice(deviceId: deviceId)));

  /// Ends [deviceId]'s code as Add a device closes, unless a device was
  /// already let in with it.
  Future<void> cancelPairing(String deviceId) =>
      _request(ClientMessage(cancelPairing: CancelPairing(deviceId: deviceId)));

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
        welcomeName = message.welcome.name;
        welcomeColor = message.welcome.color;
        pairing.value = null;
        connectedSince.value = DateTime.now();
        connected.value = true;
        _log('connected to factoryd ${message.welcome.serverVersion}');
        // The backend forgets a watch when the connection closes.
        if (_watching.isNotEmpty) fireAndLog(watchFiles(_watching));
      case ServerMessage_Body.ack:
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.pairingPending:
        unawaited(_pairingPending(message.pairingPending));
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
        _wholeWorkspace = true;
        workspace.apply(message.workspace);
      case ServerMessage_Body.workspacePatch:
        final next = _wholeWorkspace
            ? patchWorkspace(workspace.workspace, message.workspacePatch)
            : null;
        if (next == null) {
          // It can't be applied: start again, from a whole one.
          _log('a workspace patch that does not fit: reconnecting');
          _wholeWorkspace = false;
          _restart();
          return;
        }
        workspace.apply(next);
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
      case ServerMessage_Body.entryPage:
      case ServerMessage_Body.uploadProgress:
      case ServerMessage_Body.uploadedFile:
      case ServerMessage_Body.connectStarted:
      case ServerMessage_Body.connectChoices:
      case ServerMessage_Body.usageReport:
      case ServerMessage_Body.usageResetAnswer:
      case ServerMessage_Body.appPermissions:
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.threadList:
        threads.apply(message.threadList);
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.thread:
        threads.put(message.thread);
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.fileChanged:
        fileChanged.value = message.fileChanged;
      case ServerMessage_Body.notSet:
        // A message kind from a newer backend; nothing to do with it yet.
        break;
    }
  }

  void _disconnected() {
    _socket = null;
    _wholeWorkspace = false;
    // A connection that was up and dropped is most likely a network change:
    // it comes back without a word, if it can.
    if (connected.value && !_closed) _reconnectQuietly();
    connectedSince.value = null;
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
