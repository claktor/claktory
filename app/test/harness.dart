import 'package:factory_app/app_update.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/history.dart';
import 'package:factory_app/login.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/update.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

/// A backend that records requests instead of sending them, and accepts them
/// all unless [refuse] is set.
class RecordingBackend extends Backend {
  RecordingBackend()
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
  Future<void> sendMessage(String agentId, String text) => _record(
    ClientMessage(
      sendMessage: SendMessage(agentId: agentId, text: text),
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
  Future<void> signOut(String providerId) =>
      _record(ClientMessage(signOut: SignOut(providerId: providerId)));

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
  Future<void> showAllFiles(String projectId, bool showAll) => _record(
    ClientMessage(
      showAllFiles: ShowAllFiles(projectId: projectId, showAll: showAll),
    ),
  );
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
  Harness(Workspace snapshot, {Drafts? drafts})
    : drafts = drafts ?? Drafts(MemoryDraftStorage()) {
    backend.workspace.apply(snapshot);
    this.drafts.watch(backend.workspace);
    login = LoginLauncher(
      workspace: backend.workspace,
      send: backend.startLogin,
      open: (url) async {
        opened.add(url.toString());
        return true;
      },
    );
  }

  final backend = RecordingBackend();
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

  Future<void> show(WidgetTester tester) async {
    tester.view.physicalSize = const Size(1200, 900);
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
          child: const Shell(),
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
