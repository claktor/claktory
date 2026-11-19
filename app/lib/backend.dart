import 'dart:async';
import 'dart:developer' as developer;
import 'dart:io';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:protobuf/protobuf.dart';

import 'app_update.dart';
import 'connections.dart';
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

const _minRetryDelay = Duration(milliseconds: 250);
const _maxRetryDelay = Duration(seconds: 5);

/// The private tunnel couldn't even start on this device.
class TunnelOpenFailure implements Exception {
  const TunnelOpenFailure(this.message);

  final String message;
}

/// A request the backend refused, or that couldn't reach it.
class RequestFailure implements Exception {
  const RequestFailure(this.message);

  final String message;

  @override
  String toString() => message;
}

/// Keeps one connection to the backend open, reconnecting with backoff when it
/// drops, and routes what the backend sends into the stores. Speaks the
/// protocol in proto/factory/v1/factory.proto.
class Backend {
  Backend({
    required this.workspace,
    required this.transcript,
    required this.explorer,
    required this.extensionView,
    QueuedStore? queued,
    Tunnels? tunnels,
  }) : queued = queued ?? QueuedStore(),
       _tunnels = tunnels ?? RustTunnels(),
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
    while (!_closed) {
      if (refused.value != null) {
        await _wake.future;
        continue;
      }
      final target = _target;
      try {
        final WebSocket socket;
        if (target == null) {
          socket = await WebSocket.connect(_localEndpoint);
        } else if (target.tunnelled) {
          final tunnel = await _tunnelTo(target);
          // Plain ws: iroh already encrypts it end to end, to the key the
          // pairing code names.
          socket = await WebSocket.connect('ws://127.0.0.1:${tunnel.port}');
        } else {
          socket = await WebSocket.connect(
            target.endpoint.toString(),
            customClient: target.client(),
          );
        }
        if (_closed || target != _target) {
          await socket.close();
          continue;
        }
        socket.pingInterval = const Duration(seconds: 5);
        _socket = socket;
        _watchTunnel();
        _send(
          ClientMessage(
            id: Int64(_nextId++),
            hello: Hello(
              protocolVersion: _protocolVersion,
              clientVersion: _clientVersion,
              token: target?.token,
            ),
          ),
        );
        await for (final frame in socket) {
          if (frame is List<int>) _receive(frame);
        }
        _log('connection closed (code ${socket.closeCode})');
      } on HandshakeException catch (error) {
        _log("its certificate isn't the pinned one: $error");
      } on IOException catch (error) {
        _log('connection failed: $error');
      } on TunnelOpenFailure catch (failure) {
        _log("couldn't open the tunnel: ${failure.message}");
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

  /// The tunnel to [target], opened the first time.
  Future<Tunnel> _tunnelTo(RemoteFactory target) async {
    final open = _tunnel;
    if (open != null && _tunnelTarget == target) return open;
    await _closeTunnel();
    try {
      final opened = await _tunnels.open(
        deviceKey: deviceKey(target),
        endpointId: target.endpointId,
        relays: target.relays,
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
  /// or through a relay can change at any time.
  void _watchTunnel() {
    final open = _tunnel;
    if (open == null) return;
    tunnel.value = open.status;
    _tunnelWatch ??= Timer.periodic(const Duration(seconds: 1), (_) {
      final current = _tunnel;
      if (current != null) tunnel.value = current.status;
    });
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

  Future<void> signOut(String providerId) =>
      _request(ClientMessage(signOut: SignOut(providerId: providerId)));

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
          pending.completeError(RequestFailure(message.error.message));
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
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.notSet:
        // A message kind from a newer backend; nothing to do with it yet.
        break;
    }
  }

  void _disconnected() {
    _socket = null;
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
