import 'package:factory_app/app_update.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/history.dart';
import 'package:factory_app/login.dart';
import 'package:factory_app/loopback_forward.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/update.dart';
import 'package:factory_app/zoom.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

/// A backend that records requests instead of sending them, and accepts them
/// all unless [refuse] is set.
class RecordingBackend extends Backend {
  RecordingBackend({super.localBackend})
    : super(
        workspace: WorkspaceStore(),
        transcript: TranscriptStore(),
        explorer: ExplorerStore(),
        extensionView: ExtensionViewStore(),
      ) {
    connected.value = true;
  }

  final sent = <ClientMessage>[];

  /// When set, every request fails with this reason.
  String? refuse;

  /// Runs when a request is sent, before it's answered: what the backend
  /// would broadcast because of it.
  void Function(ClientMessage message)? onSend;

  Future<void> _record(ClientMessage message) {
    sent.add(message);
    onSend?.call(message);
    final reason = refuse;
    return reason == null
        ? Future.value()
        : Future.error(RequestFailure(reason));
  }

  @override
  Future<void> createProject(String name) =>
      _record(ClientMessage(createProject: CreateProject(name: name)));

  @override
  Future<void> updateProject(UpdateProject update) =>
      _record(ClientMessage(updateProject: update));

  @override
  Future<void> deleteProject(String projectId) => _record(
    ClientMessage(deleteProject: DeleteProject(projectId: projectId)),
  );

  @override
  Future<void> createAgent(CreateAgent agent) =>
      _record(ClientMessage(createAgent: agent));

  @override
  Future<void> updateAgent(UpdateAgent update) =>
      _record(ClientMessage(updateAgent: update));

  @override
  Future<void> deleteAgent(String agentId) =>
      _record(ClientMessage(deleteAgent: DeleteAgent(agentId: agentId)));

  @override
  Future<void> selectAgent(String agentId) =>
      _record(ClientMessage(selectAgent: SelectAgent(agentId: agentId)));

  @override
  Future<void> sendMessage(
    String agentId,
    String text, {
    String clientId = '',
    List<Attachment> attachments = const [],
  }) => _record(
    ClientMessage(
      sendMessage: SendMessage(
        agentId: agentId,
        text: text,
        clientId: clientId,
        attachments: attachments,
      ),
    ),
  );

  @override
  Future<void> stopAgent(String agentId) =>
      _record(ClientMessage(stopAgent: StopAgent(agentId: agentId)));

  @override
  Future<void> decideProposal(
    String agentId,
    String proposalId, {
    required bool approve,
    String comment = '',
  }) => _record(
    ClientMessage(
      decideProposal: DecideProposal(
        agentId: agentId,
        proposalId: proposalId,
        approve: approve,
        comment: comment,
      ),
    ),
  );

  @override
  Future<void> stopTask(String agentId, String taskId) => _record(
    ClientMessage(
      stopTask: StopTask(agentId: agentId, taskId: taskId),
    ),
  );

  @override
  Future<void> changeMonitor(
    String agentId,
    int monitorId,
    MonitorAction action,
  ) => _record(
    ClientMessage(
      changeMonitor: ChangeMonitor(
        agentId: agentId,
        monitorId: monitorId,
        action: action,
      ),
    ),
  );

  /// The pieces of release archives asked for: (version, offset).
  final fetched = <(String, int)>[];

  @override
  Future<AppPiece> fetchApp(String version, int offset) async {
    fetched.add((version, offset));
    final reason = refuse;
    if (reason != null) throw RequestFailure(reason);
    return AppPiece(
      size: 3,
      sha256: 'ab',
      appName: 'Factory.app',
      data: [1, 2, 3],
    );
  }

  /// What [listFolders] answers, by the path asked; any other path doesn't
  /// exist.
  final folderLists = <String, FolderList>{};

  @override
  Future<FolderList> listFolders(String path) async {
    await _record(ClientMessage(listFolders: ListFolders(path: path)));
    final list = folderLists[path];
    if (list == null) {
      throw RequestFailure("Can't read $path: it doesn't exist");
    }
    return list;
  }

  /// What [readFile] answers, by path; any other path can't be read.
  final files = <String, FileContent>{};

  /// Whether [readFile] reads from an offset, as backends since videos do.
  var rangedReads = true;

  @override
  Future<FileContent> readFile(
    String path, {
    int maxBytes = 0,
    int offset = 0,
  }) async {
    await _record(
      ClientMessage(
        readFile: ReadFile(
          path: path,
          maxBytes: Int64(maxBytes),
          offset: Int64(offset),
        ),
      ),
    );
    final file = files[path];
    if (file == null) {
      return FileContent(path: path, error: 'No such file: $path');
    }
    if (offset == 0 && maxBytes == 0) return file;
    // A backend from before ranged reads answers from the start.
    if (!rangedReads) offset = 0;
    // A range of it, as the backend reads a video.
    final end = maxBytes == 0
        ? file.data.length
        : (offset + maxBytes).clamp(0, file.data.length);
    return file.deepCopy()
      ..data = file.data.sublist(offset.clamp(0, file.data.length), end)
      ..offset = Int64(offset)
      ..truncated = offset > 0 || end < file.data.length;
  }

  /// Whether [setLayout] changes the agent's layout, as the backend would
  /// (Settings and other pages open as panes); else it's only recorded.
  var appliesLayouts = false;

  @override
  Future<void> setLayout(String agentId, ViewNode? root) async {
    await _record(
      ClientMessage(
        setLayout: SetLayout(agentId: agentId, root: root),
      ),
    );
    if (!appliesLayouts) return;
    // As the backend does: the agent's layout is it now.
    final now = workspace.workspace.deepCopy();
    for (final agent in now.agents) {
      if (agent.id != agentId) continue;
      if (root == null) {
        agent.clearLayout();
      } else {
        agent.layout = root;
      }
    }
    workspace.apply(now);
  }

  /// The code [createPairing] answers.
  var pairingCode = PairingCode(
    code:
        'factory://100.64.0.2:7443?t=${'a' * 64}&fp=${'b' * 64}&name=mac-mini',
    deviceId: 'd9',
  );

  @override
  Future<PairingCode> createPairing(String name, String host) async {
    await _record(
      ClientMessage(
        createPairing: CreatePairing(name: name, host: host),
      ),
    );
    return pairingCode;
  }

  @override
  Future<void> unpairDevice(String deviceId) =>
      _record(ClientMessage(unpairDevice: UnpairDevice(deviceId: deviceId)));

  @override
  Future<void> approveDevice(String deviceId) =>
      _record(ClientMessage(approveDevice: ApproveDevice(deviceId: deviceId)));

  @override
  Future<void> denyDevice(String deviceId) =>
      _record(ClientMessage(denyDevice: DenyDevice(deviceId: deviceId)));

  @override
  Future<void> cancelPairing(String deviceId) =>
      _record(ClientMessage(cancelPairing: CancelPairing(deviceId: deviceId)));

  @override
  Future<void> listThreads(String path) =>
      _record(ClientMessage(listThreads: ListThreads(path: path)));

  /// The thread [createThread] answers: as asked, with ids t1, t2…
  var _threads = 0;

  @override
  Future<Thread> createThread(CreateThread thread) async {
    await _record(ClientMessage(createThread: thread));
    return Thread(
      id: 't${++_threads}',
      agentId: thread.agentId,
      path: thread.path,
      kind: thread.kind,
      versionSha256: thread.versionSha256,
      selectors: thread.selectors,
      status: ThreadStatus.THREAD_STATUS_OPEN,
      comments: [ThreadComment(body: thread.body, draft: !thread.sendNow)],
    );
  }

  @override
  Future<void> replyThread(
    String threadId,
    String body, {
    bool sendNow = false,
  }) => _record(
    ClientMessage(
      replyThread: ReplyThread(
        threadId: threadId,
        body: body,
        sendNow: sendNow,
      ),
    ),
  );

  @override
  Future<void> editComment(String commentId, String body) => _record(
    ClientMessage(
      editComment: EditComment(commentId: commentId, body: body),
    ),
  );

  @override
  Future<void> deleteComment(String commentId) => _record(
    ClientMessage(deleteComment: DeleteComment(commentId: commentId)),
  );

  @override
  Future<void> setThreadStatus(String threadId, {required bool resolved}) =>
      _record(
        ClientMessage(
          setThreadStatus: SetThreadStatus(
            threadId: threadId,
            resolved: resolved,
          ),
        ),
      );

  @override
  Future<void> readdressThread(String threadId, String agentId) => _record(
    ClientMessage(
      readdressThread: ReaddressThread(threadId: threadId, agentId: agentId),
    ),
  );

  @override
  Future<void> reattachThread(ReattachThread reattach) =>
      _record(ClientMessage(reattachThread: reattach));

  @override
  Future<void> sendComments(String agentId, String path) => _record(
    ClientMessage(
      sendComments: SendComments(agentId: agentId, path: path),
    ),
  );

  @override
  Future<void> decideSuggestion(String threadId, {required bool accept}) =>
      _record(
        ClientMessage(
          decideSuggestion: DecideSuggestion(
            threadId: threadId,
            accept: accept,
          ),
        ),
      );

  @override
  Future<void> setRemoteAccess(bool on, {int port = 0, String? listen}) =>
      _record(
        ClientMessage(
          setRemoteAccess: SetRemoteAccess(on: on, port: port, listen: listen),
        ),
      );

  @override
  Future<void> updateSettings(UpdateSettings update) =>
      _record(ClientMessage(updateSettings: update));

  @override
  Future<void> startLogin(StartLogin start) =>
      _record(ClientMessage(startLogin: start));

  @override
  Future<void> answerLogin(String text) =>
      _record(ClientMessage(answerLogin: AnswerLogin(text: text)));

  @override
  Future<void> cancelLogin() =>
      _record(ClientMessage(cancelLogin: CancelLogin()));

  @override
  Future<void> signOut(String providerId, {String accountId = ''}) => _record(
    ClientMessage(
      signOut: SignOut(providerId: providerId, accountId: accountId),
    ),
  );

  @override
  Future<void> moveAccount(String providerId, String accountId, int position) =>
      _record(
        ClientMessage(
          moveAccount: MoveAccount(
            providerId: providerId,
            accountId: accountId,
            position: position,
          ),
        ),
      );

  @override
  Future<void> updateAccount(UpdateAccount update) =>
      _record(ClientMessage(updateAccount: update));

  @override
  Future<void> clearBuildCache(String path) =>
      _record(ClientMessage(clearBuildCache: ClearBuildCache(path: path)));

  @override
  Future<void> installUpdate(String appPath) =>
      _record(ClientMessage(installUpdate: InstallUpdate(appPath: appPath)));

  @override
  Future<void> focusProject(String projectId) =>
      _record(ClientMessage(focusProject: FocusProject(projectId: projectId)));

  @override
  Future<void> expandFolder(
    String projectId,
    String path,
    bool expanded, {
    String folderId = '',
    String agentId = '',
  }) => _record(
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

  @override
  Future<void> arrangeAgent(ArrangeAgent arrange) =>
      _record(ClientMessage(arrangeAgent: arrange));

  @override
  Future<void> setAgentCollapsed(String agentId, bool collapsed) => _record(
    ClientMessage(
      setAgentCollapsed: SetAgentCollapsed(
        agentId: agentId,
        collapsed: collapsed,
      ),
    ),
  );

  @override
  Future<void> createFolder(CreateFolder folder) =>
      _record(ClientMessage(createFolder: folder));

  @override
  Future<void> updateFolder(UpdateFolder update) =>
      _record(ClientMessage(updateFolder: update));

  @override
  Future<void> deleteFolder(String projectId, String folderId) => _record(
    ClientMessage(
      deleteFolder: DeleteFolder(projectId: projectId, folderId: folderId),
    ),
  );

  @override
  Future<void> setFolderCollapsed(
    String projectId,
    String folderId,
    bool collapsed,
  ) => _record(
    ClientMessage(
      setFolderCollapsed: SetFolderCollapsed(
        projectId: projectId,
        folderId: folderId,
        collapsed: collapsed,
      ),
    ),
  );

  @override
  Future<void> setHighlights(bool on) =>
      _record(ClientMessage(setHighlights: SetHighlights(on: on)));

  @override
  Future<void> setZoom(double zoom) =>
      _record(ClientMessage(setZoom: SetZoom(zoom: zoom)));

  /// What [useUsageReset] answers.
  var usageResetAnswer = UsageResetAnswer(reset: true, message: 'Reset.');

  @override
  Future<UsageResetAnswer> useUsageReset(UseUsageReset use) async {
    await _record(ClientMessage(useUsageReset: use));
    return usageResetAnswer;
  }

  @override
  Future<void> runCommand(String agentId, String name, String args) => _record(
    ClientMessage(
      runCommand: RunCommand(agentId: agentId, name: name, args: args),
    ),
  );

  @override
  Future<void> openPanel(String agentId, String extensionId) => _record(
    ClientMessage(
      openPanel: OpenPanel(agentId: agentId, extensionId: extensionId),
    ),
  );

  @override
  Future<void> watchFiles(String projectId) =>
      _record(ClientMessage(watchFiles: WatchFiles(projectId: projectId)));

  @override
  Future<void> showAllFiles(String projectId, bool showAll) => _record(
    ClientMessage(
      showAllFiles: ShowAllFiles(projectId: projectId, showAll: showAll),
    ),
  );

  // The vault, as the backend keeps it.

  /// What [revealSecret] answers, by secret id.
  final secretValues = <String, List<SecretEntry>>{};

  /// What [previewEnv] answers.
  var envPreview = EnvPreview();

  /// The key [unlockVault] takes; anything else is refused as the backend
  /// does.
  var unlockKey = 'ABCD-EFGH-IJKL-MNOP';

  @override
  Future<void> unlockVault(String key) async {
    await _record(ClientMessage(unlockVault: UnlockVault(passphrase: key)));
    if (key != unlockKey) {
      throw const RequestFailure(
        "That passphrase isn't right.",
        code: ErrorCode.ERROR_CODE_WRONG_PROOF,
      );
    }
  }

  @override
  Future<void> createSecret(
    String name,
    String description,
    List<SecretEntry> entries,
  ) => _record(
    ClientMessage(
      createSecret: CreateSecret(
        name: name,
        description: description,
        entries: entries,
      ),
    ),
  );

  @override
  Future<void> updateSecret(
    String id,
    String name,
    String description, [
    List<SecretEntry> entries = const [],
  ]) => _record(
    ClientMessage(
      updateSecret: UpdateSecret(
        id: id,
        name: name,
        description: description,
        entries: entries,
      ),
    ),
  );

  @override
  Future<void> deleteSecret(String id) =>
      _record(ClientMessage(deleteSecret: DeleteSecret(id: id)));

  @override
  Future<void> grantSecret(String id, String agentId) => _record(
    ClientMessage(
      grantSecret: GrantSecret(id: id, agentId: agentId),
    ),
  );

  @override
  Future<void> revokeSecret(
    String id,
    String agentId, {
    bool stopRunningTasks = false,
  }) => _record(
    ClientMessage(
      revokeSecret: RevokeSecret(
        id: id,
        agentId: agentId,
        stopRunningTasks: stopRunningTasks,
      ),
    ),
  );

  @override
  Future<List<SecretEntry>> revealSecret(String id) async {
    await _record(ClientMessage(revealSecret: RevealSecret(id: id)));
    return secretValues[id] ?? const [];
  }

  @override
  Future<EnvPreview> previewEnv(String text) async {
    await _record(ClientMessage(previewEnv: PreviewEnv(text: text)));
    return envPreview;
  }

  @override
  Future<void> importEnv(
    String text,
    String name,
    String description, {
    bool onePerVariable = false,
  }) => _record(
    ClientMessage(
      importEnv: ImportEnv(
        text: text,
        name: name,
        description: description,
        onePerVariable: onePerVariable,
      ),
    ),
  );

  @override
  Future<void> decideSecretRequest(
    String agentId,
    String requestId, {
    required bool approve,
    List<SecretEntry> entries = const [],
    String comment = '',
    String secretId = '',
    String name = '',
    String description = '',
  }) => _record(
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

  /// What [listSecretUses] answers.
  var secretUses = <SecretUse>[];

  @override
  Future<List<SecretUse>> listSecretUses(String id, {int limit = 0}) async {
    await _record(
      ClientMessage(
        listSecretUses: ListSecretUses(id: id, limit: limit),
      ),
    );
    return secretUses;
  }

  @override
  Future<void> accountSignIn(String email) =>
      _record(ClientMessage(accountSignIn: AccountSignIn(email: email)));

  @override
  Future<void> accountVerify(String code) =>
      _record(ClientMessage(accountVerify: AccountVerify(code: code)));

  @override
  Future<void> accountResend() =>
      _record(ClientMessage(accountResend: AccountResend()));

  @override
  Future<void> accountSignOut() =>
      _record(ClientMessage(accountSignOut: AccountSignOut()));

  @override
  Future<void> setAccountService(String url) =>
      _record(ClientMessage(setAccountService: SetAccountService(url: url)));

  /// What [showRecoveryKey] answers.
  var recoveryKey = 'ABCD-EFGH-IJKL-MNOP';

  @override
  Future<String> showRecoveryKey() async {
    await _record(ClientMessage(showRecoveryKey: ShowRecoveryKey()));
    return recoveryKey;
  }

  /// What [startConnect] answers (Priya's connectors_test).
  var connectStarted = ConnectStarted(
    flowId: 'flow-1',
    url: 'https://dash.cloudflare.com/oauth2/auth?state=s',
  );

  @override
  Future<ConnectStarted> startConnect(StartConnect start) async {
    await _record(ClientMessage(startConnect: start));
    return connectStarted;
  }

  @override
  Future<void> finishConnect(
    String flowId,
    String callbackUrl, {
    String token = '',
  }) => _record(
    ClientMessage(
      finishConnect: FinishConnect(
        flowId: flowId,
        callbackUrl: callbackUrl,
        token: token,
      ),
    ),
  );

  @override
  Future<void> cancelConnect(String flowId) =>
      _record(ClientMessage(cancelConnect: CancelConnect(flowId: flowId)));

  @override
  Future<void> dismissConnectProposal(
    String proposalId, {
    String comment = '',
  }) => _record(
    ClientMessage(
      dismissConnectProposal: DismissConnectProposal(
        proposalId: proposalId,
        comment: comment,
      ),
    ),
  );

  /// What [loadUsage] answers (Priya's usage_view_test).
  var usageReport = UsageReport();

  @override
  Future<UsageReport> loadUsage(LoadUsage ask) async {
    await _record(ClientMessage(loadUsage: ask));
    return usageReport;
  }

  /// What [listConnectChoices] answers.
  var connectChoices = ConnectChoices();

  @override
  Future<ConnectChoices> listConnectChoices(String secretId) async {
    await _record(
      ClientMessage(listConnectChoices: ListConnectChoices(secretId: secretId)),
    );
    return connectChoices;
  }

  @override
  Future<void> deriveSecret(DeriveSecret derive) =>
      _record(ClientMessage(deriveSecret: derive));

  @override
  Future<void> disconnectService(String secretId, {bool revoke = false}) =>
      _record(
        ClientMessage(
          disconnectService: DisconnectService(
            secretId: secretId,
            revoke: revoke,
          ),
        ),
      );

  /// The ports [forwardPort] was asked for; it answers what [forwarded]
  /// makes, by default that the Claktory can't forward.
  final forwardedPorts = <int>[];
  Future<PortForward> Function(int port) forwarded = (_) async =>
      throw const ForwardFailure(ForwardProblem.unsupported);

  @override
  Future<PortForward> forwardPort(int port) {
    forwardedPorts.add(port);
    return forwarded(port);
  }
}

/// Updates this app from another machine without touching the disk: it
/// asks the backend for the first piece, reports [steps] 100ms apart, and
/// records what it stages, replaces and discards.
class FakeUpdater extends AppUpdater {
  FakeUpdater();

  var steps = const [
    UpdateProgress(UpdateStage.preparing),
    UpdateProgress(UpdateStage.downloading, received: 12300000, size: 48000000),
    UpdateProgress(UpdateStage.checking),
  ];

  /// When set, staging fails with this reason after its steps.
  String? fails;

  final replaced = <String>[];
  final discarded = <String>[];

  @override
  Future<StagedApp> stage({
    required FetchAppPiece fetch,
    required String version,
    required String bundlePath,
    void Function(UpdateProgress progress)? onProgress,
  }) async {
    await fetch(version, 0);
    for (final step in steps) {
      onProgress?.call(step);
      await Future<void>.delayed(const Duration(milliseconds: 100));
    }
    final reason = fails;
    if (reason != null) throw UpdateFailure(reason);
    return StagedApp(app: '$bundlePath.staged', staging: '$bundlePath.update');
  }

  @override
  Future<void> replace(StagedApp staged, {required String app}) async =>
      replaced.add('${staged.app} -> $app');

  @override
  void discard(StagedApp staged) => discarded.add(staged.app);
}

/// The app's window around a [RecordingBackend], with browser opening
/// recorded in [opened].
class Harness {
  /// The person has a name ([named]) unless the test says otherwise, so
  /// the first launch's question (NameStep) doesn't cover every test.
  Harness(
    Workspace snapshot, {
    Drafts? drafts,
    bool localBackend = true,
    bool named = true,
  }) : drafts = drafts ?? Drafts(MemoryDraftStorage()),
       backend = RecordingBackend(localBackend: localBackend) {
    // Whatever a test or the backend applies later gets the name too.
    if (named) {
      backend.workspace.addListener(() {
        final now = backend.workspace.workspace;
        if (now.settings.userName.isNotEmpty) return;
        if (now.isFrozen) {
          backend.workspace.apply(
            now.deepCopy()..ensureSettings().userName = 'Matt',
          );
        } else {
          // In place, so a test that changes its snapshot and applies it
          // again still sees the same object.
          now.ensureSettings().userName = 'Matt';
        }
      });
    }
    backend.workspace.apply(snapshot);
    this.drafts.watch(backend.workspace);
    // As the app's Claktories do (rail.dart), the window shows the
    // Claktory chosen: here the one backend connects to it instead.
    connections.addListener(() {
      final chosen = connections.current;
      if (chosen?.code != backend.target?.code) backend.connectTo(chosen);
    });
    login = LoginLauncher(
      workspace: backend.workspace,
      send: backend.startLogin,
      forwards: backend.forwards,
      open: (url) async {
        opened.add(url.toString());
        return true;
      },
    );
  }

  final RecordingBackend backend;
  final opened = <String>[];
  final page = ValueNotifier(AppPage.agent);
  late final history = AgentHistory(
    workspace: backend.workspace,
    select: (id) => backend.selectAgent(id),
  );
  late final LoginLauncher login;

  /// Which Factory shows, kept in memory as a device would keep it.
  final connectionStorage = MemoryConnectionStorage();
  late final connections = Connections(connectionStorage);

  /// The drafts kept on this device.
  final Drafts drafts;

  /// This app's build; [quits] counts how often it quit to be updated.
  String appVersion = 'dev';
  String bundlePath = '/Applications/Factory.app';
  var quits = 0;
  final updater = FakeUpdater();

  /// The phone's camera: none unless a test gives it one.
  final camera = FakeCamera();

  /// A Mac window; a phone's screen is `Size(390, 844)`.
  Future<void> show(
    WidgetTester tester, {
    Size size = const Size(1200, 900),
  }) async {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xFF000000),
        builder: (context, _) => AppScope(
          backend: backend,
          login: login,
          page: page,
          appBuild: AppBuild(
            version: appVersion,
            bundlePath: bundlePath,
            quit: () => quits++,
            updater: updater,
          ),
          drafts: drafts,
          history: history,
          connections: connections,
          camera: camera,
          child: const AppZoom(child: Shell()),
        ),
      ),
    );
    await tester.pump();
  }

  /// Replaces the workspace, as a new snapshot from the backend would.
  Future<void> apply(WidgetTester tester, Workspace snapshot) async {
    backend.workspace.apply(snapshot);
    await tester.pump();
  }

  /// The body of the last request sent.
  ClientMessage get last => backend.sent.last;
}

/// Live indicators pulse forever, so wait out the UI's own motion instead of
/// waiting for every animation to stop.
Future<void> settle(WidgetTester tester) =>
    tester.pump(const Duration(milliseconds: 500));

/// Runs [total] of frames one by one, for animations that need every frame
/// (scrolls, menus); pumpAndSettle never settles while characters live.
Future<void> pumpFrames(
  WidgetTester tester, [
  Duration total = const Duration(milliseconds: 800),
]) async {
  const frame = Duration(milliseconds: 16);
  for (var elapsed = Duration.zero; elapsed < total; elapsed += frame) {
    await tester.pump(frame);
  }
}

Provider anthropic({String signedInWith = '', bool canSignOut = false}) =>
    Provider(
      id: 'anthropic',
      name: 'Anthropic',
      signedInWith: signedInWith,
      canSignOut: canSignOut,
      methods: [
        LoginMethod(id: 'oauth', label: 'Anthropic (Claude Pro/Max)'),
        LoginMethod(id: 'api_key', label: 'Anthropic API key'),
      ],
    );

/// Hovers the sidebar row of the project named [project] and clicks the "+"
/// that shows on it.
Future<void> tapProjectPlus(WidgetTester tester, String project) async {
  // Rows show hover as a mouse-driven desktop app does.
  FocusManager.instance.highlightStrategy =
      FocusHighlightStrategy.alwaysTraditional;
  addTearDown(
    () => FocusManager.instance.highlightStrategy =
        FocusHighlightStrategy.automatic,
  );
  final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
  await mouse.addPointer(location: tester.getCenter(find.text(project).first));
  await tester.pump();
  await tester.tap(askRivetButtons().last);
  await settle(tester);
  await mouse.removePointer();
}

/// The sidebar's "+" buttons that ask Rivet to set something up.
Finder askRivetButtons() => find.byWidgetPredicate(
  (widget) =>
      widget is Semantics &&
      widget.properties.label == 'Ask Rivet to set it up',
);

/// A camera for tests: [access] is what it answers, [granting] what asking
/// for it gets; [scan] reads a code as if the camera saw it.
class FakeCamera implements Camera {
  CameraAccess state = CameraAccess.unavailable;
  bool granting = true;
  var asked = 0;
  var settingsOpened = 0;
  ValueChanged<String>? _onCode;

  @override
  Future<CameraAccess> access() async => state;

  @override
  Future<bool> request() async {
    asked++;
    state = granting ? CameraAccess.granted : CameraAccess.denied;
    return granting;
  }

  @override
  Future<void> openSettings() async => settingsOpened++;

  @override
  Widget preview(ValueChanged<String> onCode) {
    _onCode = onCode;
    return const SizedBox.expand(key: ValueKey('camera'));
  }

  void scan(String code) => _onCode?.call(code);
}
