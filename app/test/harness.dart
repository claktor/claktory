import 'package:cupertino_ui/cupertino_ui.dart'
    show DefaultCupertinoLocalizations;
import 'package:factory_app/app_update.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/device_account.dart' show DeviceAccount;
import 'package:factory_app/drafts.dart';
import 'package:factory_app/history.dart';
import 'package:factory_app/layout.dart' show leaves;
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

  /// How many times the person asked to reconnect (a tap on Reconnecting…,
  /// Reconnect): counted, not done, since nothing runs here.
  var reconnectsAsked = 0;

  @override
  void reconnectNow() => reconnectsAsked++;

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
    bool voice = false,
    int checklistItem = 0,
    String checklistAgentId = '',
  }) => _record(
    ClientMessage(
      sendMessage: SendMessage(
        agentId: agentId,
        text: text,
        clientId: clientId,
        attachments: attachments,
        voice: voice,
        checklistItem: checklistItem,
        checklistAgentId: checklistAgentId,
      ),
    ),
  );

  @override
  Future<void> installSpeechModel() =>
      _record(ClientMessage(installSpeechModel: InstallSpeechModel()));

  @override
  Future<void> removeSpeechModel() =>
      _record(ClientMessage(removeSpeechModel: RemoveSpeechModel()));

  @override
  Future<void> retryVoiceMessage(String agentId, Int64 messageId) => _record(
    ClientMessage(
      retryVoiceMessage: RetryVoiceMessage(
        agentId: agentId,
        messageId: messageId,
      ),
    ),
  );

  @override
  Future<void> discardVoiceMessage(String agentId, Int64 messageId) => _record(
    ClientMessage(
      discardVoiceMessage: DiscardVoiceMessage(
        agentId: agentId,
        messageId: messageId,
      ),
    ),
  );

  /// Uploads are taken whole at once, each put in the agent's uploads as
  /// it's named.
  @override
  Future<int> startUpload(
    String uploadId,
    String agentId, {
    required String name,
    required int size,
    String mime = '',
  }) async {
    await _record(
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
    _uploads[uploadId] = (agentId, name, size, mime);
    return 0;
  }

  final _uploads = <String, (String, String, int, String)>{};

  @override
  Future<int> uploadChunk(String uploadId, int offset, List<int> data) async =>
      offset + data.length;

  @override
  Future<Attachment> finishUpload(String uploadId, {String sha256 = ''}) async {
    await _record(
      ClientMessage(
        finishUpload: FinishUpload(uploadId: uploadId, sha256: sha256),
      ),
    );
    final (agentId, name, size, mime) = _uploads.remove(uploadId)!;
    return Attachment(
      path: '/uploads/$agentId/$name',
      name: name,
      size: Int64(size),
      mime: mime,
    );
  }

  @override
  Future<void> cancelUpload(String uploadId) {
    _uploads.remove(uploadId);
    return _record(
      ClientMessage(cancelUpload: CancelUpload(uploadId: uploadId)),
    );
  }

  @override
  Future<void> stopAgent(String agentId) =>
      _record(ClientMessage(stopAgent: StopAgent(agentId: agentId)));

  @override
  Future<void> decideProposal(
    String agentId,
    String proposalId, {
    required bool approve,
    String comment = '',
    String chatId = '',
  }) => _record(
    ClientMessage(
      decideProposal: DecideProposal(
        agentId: agentId,
        chatId: chatId,
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
    String expectedSha256 = '',
  }) async {
    await _record(
      ClientMessage(
        readFile: ReadFile(
          path: path,
          maxBytes: Int64(maxBytes),
          offset: Int64(offset),
          expectedSha256: expectedSha256,
        ),
      ),
    );
    final file = files[path];
    if (file == null) {
      return FileContent(path: path, error: 'No such file: $path');
    }
    // Another version now than the one being read (ReadFile.expected_sha256).
    if (expectedSha256.isNotEmpty && expectedSha256 != file.sha256) {
      return FileContent(path: path, error: 'changed');
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
  Future<void> setLayout(
    String agentId,
    ViewNode? root, {
    String? focus,
  }) async {
    await _record(
      ClientMessage(
        setLayout: SetLayout(agentId: agentId, root: root, focus: focus),
      ),
    );
    if (!appliesLayouts) return;
    // As the backend does: the agent's layout is it now, and its expanded
    // pane the one asked for, or kept while it's still there.
    final now = workspace.workspace.deepCopy();
    for (final agent in now.agents) {
      if (agent.id != agentId) continue;
      if (root == null) {
        agent.clearLayout();
      } else {
        agent.layout = root;
      }
      final expanded = focus ?? agent.layoutFocus;
      agent.layoutFocus =
          root != null && leaves(root).any((leaf) => leaf.id == expanded)
          ? expanded
          : '';
    }
    workspace.apply(now);
  }

  /// What [searchConversation] answers for each request; nothing found
  /// when unset.
  ConversationMatches Function(SearchConversation request)? matches;

  @override
  Future<ConversationMatches> searchConversation({
    String agentId = '',
    String chatId = '',
    String pipeId = '',
    required String query,
    int? beforeSeq,
    int limit = 100,
  }) async {
    final request = SearchConversation(
      agentId: agentId,
      chatId: chatId,
      pipeId: pipeId,
      query: query,
      beforeSeq: beforeSeq == null ? null : Int64(beforeSeq),
      limit: limit,
    );
    await _record(ClientMessage(searchConversation: request));
    return matches?.call(request) ??
        ConversationMatches(query: query, agentId: agentId, chatId: chatId);
  }

  /// The entries before the transcript's first, which [loadFrom] pages in
  /// (from the seq asked for); none when empty.
  var earlierEntries = <Entry>[];

  @override
  Future<void> loadFrom(String agentId, int seq) async {
    if (transcript.length == 0) return;
    final first = transcript[0].seq;
    await _record(
      ClientMessage(
        loadEntries: LoadEntries(
          agentId: agentId,
          beforeSeq: Int64(first),
          fromSeq: Int64(seq),
        ),
      ),
    );
    final page = [
      for (final entry in earlierEntries)
        if (entry.seq >= seq && entry.seq < first) entry,
    ];
    transcript.prepend(
      agentId,
      page,
      hasEarlier: earlierEntries.any((entry) => entry.seq < seq),
    );
  }

  /// What [reportReply] answers: what would go, as the backend has it.
  var replyReport = ReplyReport(
    text: 'A reply',
    model: 'Claude Opus 4.8',
    version: 'v0.1.17',
  );

  /// Why [reportReply] refuses a send, when it does.
  RequestFailure? refuseReport;

  @override
  Future<ReplyReport> reportReply({
    String agentId = '',
    String chatId = '',
    required int seq,
    String note = '',
    bool send = false,
  }) async {
    await _record(
      ClientMessage(
        reportReply: ReportReply(
          agentId: agentId,
          chatId: chatId,
          seq: Int64(seq),
          note: note,
          send: send,
        ),
      ),
    );
    if (send && refuseReport != null) throw refuseReport!;
    return replyReport.deepCopy()..note = note;
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

  /// What [showPairingCode] answers, by pairing id; a pairing not in it is
  /// refused as no longer waiting.
  final shownCodes = <String, PairingCode>{};

  @override
  Future<PairingCode> showPairingCode(String pairingId) async {
    await _record(
      ClientMessage(showPairingCode: ShowPairingCode(pairingId: pairingId)),
    );
    final code = shownCodes[pairingId];
    if (code == null) {
      throw const RequestFailure(
        'That pairing no longer waits.',
        code: ErrorCode.ERROR_CODE_NOT_FOUND,
      );
    }
    return code;
  }

  @override
  Future<void> skipModelStep() =>
      _record(ClientMessage(setOnboarding: SetOnboarding(skipModel: true)));

  @override
  Future<void> unpairDevice(String deviceId) =>
      _record(ClientMessage(unpairDevice: UnpairDevice(deviceId: deviceId)));

  @override
  Future<void> confirmDevice(String deviceId) =>
      _record(ClientMessage(confirmDevice: ConfirmDevice(deviceId: deviceId)));

  @override
  Future<void> confirmThisDevice(String assertion) => _record(
    ClientMessage(confirmDevice: ConfirmDevice(assertion: assertion)),
  );

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
  Future<void> decideSuggestion(
    String threadId, {
    required bool accept,
    String comment = '',
  }) => _record(
    ClientMessage(
      decideSuggestion: DecideSuggestion(
        threadId: threadId,
        accept: accept,
        comment: comment,
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
  Future<void> sendTestNotification() =>
      _record(ClientMessage(sendTestNotification: SendTestNotification()));

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
  Future<void> checkForUpdates() =>
      _record(ClientMessage(checkForUpdates: CheckForUpdates()));

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
  Future<void> screenOpened(String id, bool opened) => _record(
    ClientMessage(
      screenOpened: ScreenOpened(id: id, opened: opened),
    ),
  );

  @override
  Future<void> setZoom(double zoom) =>
      _record(ClientMessage(setZoom: SetZoom(zoom: zoom)));

  @override
  Future<void> shuffleAvatar({int? choice}) =>
      _record(ClientMessage(shuffleAvatar: ShuffleAvatar(choice: choice)));

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
    List<SecretEntry> entries, {
    bool askEachTime = false,
  }) => _record(
    ClientMessage(
      createSecret: CreateSecret(
        name: name,
        description: description,
        entries: entries,
        askEachTime: askEachTime,
      ),
    ),
  );

  @override
  Future<void> addGoal(String agentId, String text) => _record(
    ClientMessage(
      addGoal: AddGoal(agentId: agentId, text: text),
    ),
  );

  @override
  Future<void> updateGoal(
    String agentId,
    int goalId, {
    String? text,
    ChecklistStatus? status,
    bool remove = false,
  }) => _record(
    ClientMessage(
      updateGoal: UpdateGoal(
        agentId: agentId,
        goalId: goalId,
        text: text,
        status: status,
        remove: remove,
      ),
    ),
  );

  @override
  Future<void> setSecretAskEachTime(String id, bool on) => _record(
    ClientMessage(
      setSecretAskEachTime: SetSecretAskEachTime(id: id, askEachTime: on),
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
    Map<String, String> aliases = const {},
    bool asIs = false,
    bool approveOnly = false,
    bool? askEachTime,
    int passwordLength = 0,
    bool deleteOriginals = false,
    bool grantToAsker = false,
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
        aliases: aliases.entries,
        asIs: asIs,
        approveOnly: approveOnly,
        askEachTime: askEachTime,
        passwordLength: passwordLength,
        deleteOriginals: deleteOriginals,
        grantToAsker: grantToAsker,
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

  /// What [checkRecoveryKey] answers.
  var recoveryKeyCheck = RecoveryKeyCheck(
    checkId: 'check-1',
    group: 3,
    groups: 8,
  );

  @override
  Future<RecoveryKeyCheck> checkRecoveryKey() async {
    await _record(ClientMessage(checkRecoveryKey: CheckRecoveryKey()));
    return recoveryKeyCheck;
  }

  @override
  Future<void> confirmRecoveryKey(String checkId, String typed) => _record(
    ClientMessage(
      confirmRecoveryKey: ConfirmRecoveryKey(checkId: checkId, typed: typed),
    ),
  );

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

  /// Browser control's log, newest first, as [listBrowserLog] pages
  /// through it.
  var browserLog = <BrowserLogEntry>[];

  @override
  Future<BrowserLog> listBrowserLog(ListBrowserLog ask) async {
    await _record(ClientMessage(listBrowserLog: ask));
    final matching = [
      for (final entry in browserLog)
        if ((ask.beforeMs == 0 ||
                entry.atMs < ask.beforeMs ||
                (entry.atMs == ask.beforeMs &&
                    ask.beforeId != 0 &&
                    entry.id < ask.beforeId)) &&
            (ask.agentId.isEmpty || entry.agentId == ask.agentId))
          entry,
    ];
    final limit = ask.limit == 0 ? 100 : ask.limit;
    return BrowserLog(
      entries: matching.take(limit),
      more: matching.length > limit,
    );
  }

  @override
  Future<void> clearBrowserLog() async {
    await _record(ClientMessage(clearBrowserLog: ClearBrowserLog()));
    browserLog = [];
  }

  @override
  Future<void> setDiagnostics(SetDiagnostics ask) =>
      _record(ClientMessage(setDiagnostics: ask));

  @override
  Future<void> setPipeForward(SetPipeForward ask) =>
      _record(ClientMessage(setPipeForward: ask));

  @override
  Future<void> saveReportType(SaveReportType ask) =>
      _record(ClientMessage(saveReportType: ask));

  @override
  Future<void> deleteReportType(String id) =>
      _record(ClientMessage(deleteReportType: DeleteReportType(id: id)));

  @override
  Future<void> setSpendCap(SetSpendCap ask) =>
      _record(ClientMessage(setSpendCap: ask));

  @override
  Future<void> setReportReader(SetReportReader ask) =>
      _record(ClientMessage(setReportReader: ask));

  @override
  Future<void> decidePipeReports(String pipeId, {required bool send}) =>
      _record(
        ClientMessage(
          decidePipeReports: DecidePipeReports(pipeId: pipeId, send: send),
        ),
      );

  /// What [listPipeReports] pages through, newest first.
  var pipeReports = <PipeReport>[];

  /// The kinds' older versions [listPipeReports] answers with.
  var pipeReportSchemas = <ReportSchema>[];

  @override
  Future<PipeReportPage> listPipeReports(ListPipeReports ask) async {
    await _record(ClientMessage(listPipeReports: ask));
    final matching = [
      for (final report in pipeReports)
        if ((ask.pipeId.isEmpty || report.pipeId == ask.pipeId) &&
            (ask.recipientId.isEmpty ||
                report.recipientId == ask.recipientId) &&
            (ask.beforeId == 0 || report.id < ask.beforeId))
          report,
    ];
    final limit = ask.limit == 0 ? 50 : ask.limit;
    return PipeReportPage(
      reports: matching.take(limit),
      more: matching.length > limit,
      schemas: pipeReportSchemas,
    );
  }

  /// What [listNeedsYouHistory] answers: those matching the query (by
  /// agent name, title or reply), as the backend would.
  var needsYouHistory = <NeedsYouRecord>[];

  /// Set, [listNeedsYouHistory] refuses with it.
  String? refuseHistory;

  @override
  Future<NeedsYouHistory> listNeedsYouHistory(ListNeedsYouHistory ask) async {
    await _record(ClientMessage(listNeedsYouHistory: ask));
    if (refuseHistory case final why?) throw RequestFailure(why);
    final query = ask.query.toLowerCase();
    final matching = [
      for (final record in needsYouHistory)
        if ((ask.beforeMs == 0 || record.decidedMs < ask.beforeMs) &&
            (query.isEmpty ||
                record.agentName.toLowerCase().contains(query) ||
                record.title.toLowerCase().contains(query) ||
                record.reply.toLowerCase().contains(query)))
          record,
    ];
    final limit = ask.limit == 0 ? 50 : ask.limit;
    return NeedsYouHistory(
      records: matching.take(limit),
      more: matching.length > limit,
    );
  }

  /// What [listQuietCopies] pages through, newest first, as the backend
  /// would.
  var quietCopies = <QuietCopy>[];

  @override
  Future<QuietCopies> listQuietCopies(
    String agentId, {
    int beforeId = 0,
    int limit = 0,
  }) async {
    await _record(
      ClientMessage(
        listQuietCopies: ListQuietCopies(
          agentId: agentId,
          beforeId: Int64(beforeId),
          limit: limit,
        ),
      ),
    );
    final older = [
      for (final copy in quietCopies)
        if (beforeId == 0 || copy.id.toInt() < beforeId) copy,
    ];
    final page = limit == 0 ? 20 : limit;
    return QuietCopies(
      agentId: agentId,
      copies: older.take(page),
      more: older.length > page,
    );
  }

  /// What [installLocalApp] answers before it's approved.
  var appPermissions = AppPermissions();

  @override
  Future<AppPermissions?> installLocalApp(
    List<int> content, {
    bool approve = false,
    String sha256 = '',
    String bundled = '',
  }) async {
    await _record(
      ClientMessage(
        installLocalApp: InstallLocalApp(
          content: content,
          approve: approve,
          sha256: sha256,
          bundled: bundled,
        ),
      ),
    );
    return approve ? null : appPermissions;
  }

  @override
  Future<void> setAppRunning(String id, bool running) => _record(
    ClientMessage(
      setAppRunning: SetAppRunning(id: id, running: running),
    ),
  );

  @override
  Future<void> uninstallApp(String id, {bool keepData = false}) => _record(
    ClientMessage(
      uninstallApp: UninstallApp(id: id, keepData: keepData),
    ),
  );

  @override
  Future<void> grantApp(String appId, String agentId, List<String> scopes) =>
      _record(
        ClientMessage(
          grantApp: GrantApp(appId: appId, agentId: agentId, scopes: scopes),
        ),
      );

  @override
  Future<void> grantAppSource(
    String appId,
    String sourceId, {
    required bool granted,
  }) => _record(
    ClientMessage(
      grantAppSource: GrantAppSource(
        appId: appId,
        sourceId: sourceId,
        granted: granted,
      ),
    ),
  );

  /// Changes the dock as the backend does: pinned at a place (moved if
  /// there already; last when out of range), or taken off it.
  @override
  Future<void> pinApp(
    String appId, {
    bool pinned = true,
    int position = -1,
  }) async {
    await _record(
      ClientMessage(
        pinApp: PinApp(appId: appId, pinned: pinned, position: position),
      ),
    );
    final now = workspace.workspace.deepCopy();
    final pins = now.pinnedApps..remove(appId);
    if (pinned) {
      pins.insert(
        position < 0 || position > pins.length ? pins.length : position,
        appId,
      );
    }
    workspace.apply(now);
  }

  @override
  Future<void> subscribeApp(
    String appId,
    String eventKind,
    List<String> agentIds,
  ) => _record(
    ClientMessage(
      subscribeApp: SubscribeApp(
        appId: appId,
        eventKind: eventKind,
        agentIds: agentIds,
      ),
    ),
  );

  @override
  Future<void> setAppSecret(String appId, String name, String value) => _record(
    ClientMessage(
      setAppSecret: SetAppSecret(appId: appId, name: name, value: value),
    ),
  );

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

  // Pipes (#196, Priya's pipes_test).

  /// What [createPipeInvite] answers.
  PipeInvite pipeInvite = PipeInvite(
    pipeId: 'p1',
    code: 'claktory-pipe://?w=x&s=y',
  );

  @override
  Future<PipeInvite> createPipeInvite(
    String projectId,
    String name,
    String purpose, {
    String startId = '',
  }) async {
    await _record(
      ClientMessage(
        createPipeInvite: CreatePipeInvite(
          projectId: projectId,
          name: name,
          purpose: purpose,
          startId: startId,
        ),
      ),
    );
    return pipeInvite;
  }

  /// What [readPipeInvite] answers.
  PipeInvitePreview pipeInvitePreview = PipeInvitePreview();

  @override
  Future<PipeInvitePreview> readPipeInvite(String code) async {
    await _record(ClientMessage(readPipeInvite: ReadPipeInvite(code: code)));
    return pipeInvitePreview;
  }

  @override
  Future<void> acceptPipeInvite(
    String code,
    String projectId, {
    String name = '',
    String purpose = '',
    String startId = '',
  }) => _record(
    ClientMessage(
      acceptPipeInvite: AcceptPipeInvite(
        code: code,
        projectId: projectId,
        name: name,
        purpose: purpose,
        startId: startId,
      ),
    ),
  );

  @override
  Future<void> dismissPipeStart(
    String agentId,
    String startId, {
    String comment = '',
  }) => _record(
    ClientMessage(
      dismissPipeStart: DismissPipeStart(
        agentId: agentId,
        startId: startId,
        comment: comment,
      ),
    ),
  );

  @override
  Future<void> approvePipe(ApprovePipe approve) =>
      _record(ClientMessage(approvePipe: approve));

  @override
  Future<void> updatePipe(UpdatePipe update) =>
      _record(ClientMessage(updatePipe: update));

  @override
  Future<void> setPipeState(String pipeId, PipeAction action) => _record(
    ClientMessage(
      setPipeState: SetPipeState(pipeId: pipeId, action: action),
    ),
  );

  @override
  Future<void> acceptPipeIdentity(String pipeId) => _record(
    ClientMessage(acceptPipeIdentity: AcceptPipeIdentity(pipeId: pipeId)),
  );

  @override
  Future<void> selectPipe(String pipeId) =>
      _record(ClientMessage(selectPipe: SelectPipe(pipeId: pipeId)));

  @override
  Future<void> sendPipeMessage(String pipeId, String text) => _record(
    ClientMessage(
      sendPipeMessage: SendPipeMessage(pipeId: pipeId, text: text),
    ),
  );

  @override
  Future<void> decidePipeMessage(
    String pipeId,
    int seq,
    bool allow, {
    String comment = '',
  }) => _record(
    ClientMessage(
      decidePipeMessage: DecidePipeMessage(
        pipeId: pipeId,
        seq: Int64(seq),
        allow: allow,
        comment: comment,
      ),
    ),
  );

  @override
  Future<void> loadPipeMessages(String pipeId) => _record(
    ClientMessage(loadPipeMessages: LoadPipeMessages(pipeId: pipeId)),
  );

  @override
  Future<void> addPipeLiaison(String pipeId, String name) => _record(
    ClientMessage(
      addPipeLiaison: AddPipeLiaison(pipeId: pipeId, name: name),
    ),
  );

  /// What [fetchEntry] answers, by "agent/seq" (Priya's inbox_test).
  final entriesById = <String, Entry>{};

  @override
  Future<Entry?> fetchEntry(String agentId, int seq) async {
    await _record(
      ClientMessage(
        loadEntries: LoadEntries(
          agentId: agentId,
          beforeSeq: Int64(seq + 1),
          fromSeq: Int64(seq),
        ),
      ),
    );
    return entriesById['$agentId/$seq'];
  }

  /// What [fetchChatEntry] answers, by "chat/seq" (Priya's inbox_test).
  final chatEntriesById = <String, Entry>{};

  @override
  Future<Entry?> fetchChatEntry(String chatId, int seq) async {
    await _record(
      ClientMessage(
        loadChatEntries: LoadChatEntries(
          chatId: chatId,
          beforeSeq: Int64(seq + 1),
          fromSeq: Int64(seq),
        ),
      ),
    );
    return chatEntriesById['$chatId/$seq'];
  }

  /// What [fetchPipeMessage] answers, by "pipe/seq".
  final pipeMessagesById = <String, PipeMessage>{};

  @override
  Future<PipeMessage?> fetchPipeMessage(String pipeId, int seq) async {
    await _record(
      ClientMessage(
        loadPipeMessages: LoadPipeMessages(
          pipeId: pipeId,
          beforeSeq: Int64(seq + 1),
          limit: 1,
        ),
      ),
    );
    return pipeMessagesById['$pipeId/$seq'];
  }

  @override
  Future<void> answerAsk(
    String agentId,
    int seq, {
    String text = '',
    bool dismiss = false,
    List<Attachment> attachments = const [],
    int? step,
  }) => _record(
    ClientMessage(
      answerAsk: AnswerAsk(
        agentId: agentId,
        seq: Int64(seq),
        text: text,
        dismiss: dismiss,
        attachments: attachments,
        step: step,
      ),
    ),
  );

  @override
  Future<void> decideSecretApproval(
    String agentId,
    int seq,
    bool allow, {
    String comment = '',
  }) => _record(
    ClientMessage(
      decideSecretApproval: DecideSecretApproval(
        agentId: agentId,
        seq: Int64(seq),
        allow: allow,
        comment: comment,
      ),
    ),
  );

  @override
  Future<void> sendBrowser(ClientMessage message) => _record(message);

  @override
  Future<void> decideAppGrant(
    String agentId,
    int seq, {
    required bool approve,
    String comment = '',
  }) => _record(
    ClientMessage(
      decideAppGrant: DecideAppGrant(
        agentId: agentId,
        seq: Int64(seq),
        approve: approve,
        comment: comment,
      ),
    ),
  );

  // Chats (#213).

  @override
  Future<void> createChat({String text = '', String clientId = ''}) => _record(
    ClientMessage(
      createChat: CreateChat(text: text, clientId: clientId),
    ),
  );

  @override
  Future<void> selectChat(String chatId) =>
      _record(ClientMessage(selectChat: SelectChat(chatId: chatId)));

  @override
  Future<void> sendChatMessage(
    String chatId,
    String text, {
    String clientId = '',
  }) => _record(
    ClientMessage(
      sendChatMessage: SendChatMessage(
        chatId: chatId,
        text: text,
        clientId: clientId,
      ),
    ),
  );

  @override
  Future<void> stopChat(String chatId) =>
      _record(ClientMessage(stopChat: StopChat(chatId: chatId)));

  @override
  Future<void> deleteChat(String chatId) =>
      _record(ClientMessage(deleteChat: DeleteChat(chatId: chatId)));

  @override
  Future<void> renameChat(String chatId, String title) => _record(
    ClientMessage(
      renameChat: RenameChat(chatId: chatId, title: title),
    ),
  );

  @override
  Future<void> startMove(String code) =>
      _record(ClientMessage(startMove: StartMove(code: code)));

  @override
  Future<void> confirmMove() =>
      _record(ClientMessage(confirmMove: ConfirmMove()));

  @override
  Future<void> cancelMove() => _record(ClientMessage(cancelMove: CancelMove()));

  @override
  Future<void> freezeMove() => _record(ClientMessage(freezeMove: FreezeMove()));
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
    this._account,
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
  final DeviceAccount? _account;
  late final connections = Connections(
    connectionStorage,
    null,
    null,
    null,
    _account,
  );

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
  /// [home] in place of the whole window, to try a view on its own.
  Future<void> show(
    WidgetTester tester, {
    Size size = const Size(1200, 900),
    Widget? home,
  }) async {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1;
    // The window fills the screen, so what the screen says the device is
    // (a phone's 393 points, an iPad's 744 or more) matches the window.
    tester.view.display.size = size;
    tester.view.display.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    addTearDown(tester.view.display.reset);
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xFF000000),
        // As the app has them (main.dart): the iPhone's text menu's words.
        localizationsDelegates: const [DefaultCupertinoLocalizations.delegate],
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
          child: AppZoom(
            child: home == null
                ? const Shell()
                // The window's own overlay, as the shell has one.
                : Overlay(initialEntries: [OverlayEntry(builder: (_) => home)]),
          ),
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

  /// What it says after a refusal: denied, as an iPhone's does at once;
  /// undetermined, as Android's does after a first no.
  CameraAccess afterRefusal = CameraAccess.denied;
  var asked = 0;
  var settingsOpened = 0;
  ValueChanged<String>? _onCode;

  @override
  Future<CameraAccess> access() async => state;

  @override
  Future<bool> request() async {
    asked++;
    state = granting ? CameraAccess.granted : afterRefusal;
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
