// This is a generated file - do not edit.
//
// Generated from factory/v1/factory.proto.

// @dart = 3.3

// ignore_for_file: annotate_overrides, camel_case_types, comment_references
// ignore_for_file: constant_identifier_names
// ignore_for_file: curly_braces_in_flow_control_structures
// ignore_for_file: deprecated_member_use_from_same_package, library_prefixes
// ignore_for_file: non_constant_identifier_names, prefer_relative_imports

import 'dart:core' as $core;

import 'package:fixnum/fixnum.dart' as $fixnum;
import 'package:protobuf/protobuf.dart' as $pb;

import 'factory.pbenum.dart';

export 'package:protobuf/protobuf.dart' show GeneratedMessageGenericExtensions;

export 'factory.pbenum.dart';

enum ClientMessage_Body {
  hello,
  createProject,
  createAgent,
  updateAgent,
  selectAgent,
  sendMessage,
  stopAgent,
  startLogin,
  answerLogin,
  cancelLogin,
  signOut,
  deleteAgent,
  deleteProject,
  updateProject,
  updateSettings,
  restoreSettings,
  installUpdate,
  focusProject,
  expandFolder,
  showAllFiles,
  openPanel,
  arrangeAgent,
  setAgentCollapsed,
  runCommand,
  createFolder,
  updateFolder,
  deleteFolder,
  setFolderCollapsed,
  setHighlights,
  stopTask,
  changeMonitor,
  createPairing,
  unpairDevice,
  setRemoteAccess,
  fetchApp,
  listFolders,
  decideProposal,
  moveAccount,
  clearBuildCache,
  readFile,
  setLayout,
  updateAccount,
  createVault,
  unlockVault,
  lockVaultSession,
  createSecret,
  updateSecret,
  deleteSecret,
  grantSecret,
  revokeSecret,
  revealSecret,
  previewEnv,
  importEnv,
  decideSecretRequest,
  listSecretUses,
  changePassphrase,
  newRecoveryKey,
  setVaultLockedMode,
  accountSignIn,
  accountVerify,
  accountResend,
  accountSignOut,
  setAccountService,
  showRecoveryKey,
  approveDevice,
  denyDevice,
  cancelPairing,
  loadEntries,
  notSet
}

class ClientMessage extends $pb.GeneratedMessage {
  factory ClientMessage({
    $fixnum.Int64? id,
    Hello? hello,
    CreateProject? createProject,
    CreateAgent? createAgent,
    UpdateAgent? updateAgent,
    SelectAgent? selectAgent,
    SendMessage? sendMessage,
    StopAgent? stopAgent,
    StartLogin? startLogin,
    AnswerLogin? answerLogin,
    CancelLogin? cancelLogin,
    SignOut? signOut,
    DeleteAgent? deleteAgent,
    DeleteProject? deleteProject,
    UpdateProject? updateProject,
    UpdateSettings? updateSettings,
    RestoreSettings? restoreSettings,
    InstallUpdate? installUpdate,
    FocusProject? focusProject,
    ExpandFolder? expandFolder,
    ShowAllFiles? showAllFiles,
    OpenPanel? openPanel,
    ArrangeAgent? arrangeAgent,
    SetAgentCollapsed? setAgentCollapsed,
    RunCommand? runCommand,
    CreateFolder? createFolder,
    UpdateFolder? updateFolder,
    DeleteFolder? deleteFolder,
    SetFolderCollapsed? setFolderCollapsed,
    SetHighlights? setHighlights,
    StopTask? stopTask,
    ChangeMonitor? changeMonitor,
    CreatePairing? createPairing,
    UnpairDevice? unpairDevice,
    SetRemoteAccess? setRemoteAccess,
    FetchApp? fetchApp,
    ListFolders? listFolders,
    DecideProposal? decideProposal,
    MoveAccount? moveAccount,
    ClearBuildCache? clearBuildCache,
    ReadFile? readFile,
    SetLayout? setLayout,
    UpdateAccount? updateAccount,
    CreateVault? createVault,
    UnlockVault? unlockVault,
    LockVaultSession? lockVaultSession,
    CreateSecret? createSecret,
    UpdateSecret? updateSecret,
    DeleteSecret? deleteSecret,
    GrantSecret? grantSecret,
    RevokeSecret? revokeSecret,
    RevealSecret? revealSecret,
    PreviewEnv? previewEnv,
    ImportEnv? importEnv,
    DecideSecretRequest? decideSecretRequest,
    ListSecretUses? listSecretUses,
    ChangePassphrase? changePassphrase,
    NewRecoveryKey? newRecoveryKey,
    SetVaultLockedMode? setVaultLockedMode,
    AccountSignIn? accountSignIn,
    AccountVerify? accountVerify,
    AccountResend? accountResend,
    AccountSignOut? accountSignOut,
    SetAccountService? setAccountService,
    ShowRecoveryKey? showRecoveryKey,
    ApproveDevice? approveDevice,
    DenyDevice? denyDevice,
    CancelPairing? cancelPairing,
    LoadEntries? loadEntries,
  }) {
    final result = ClientMessage._();
    if (id != null) result.id = id;
    if (hello != null) result.hello = hello;
    if (createProject != null) result.createProject = createProject;
    if (createAgent != null) result.createAgent = createAgent;
    if (updateAgent != null) result.updateAgent = updateAgent;
    if (selectAgent != null) result.selectAgent = selectAgent;
    if (sendMessage != null) result.sendMessage = sendMessage;
    if (stopAgent != null) result.stopAgent = stopAgent;
    if (startLogin != null) result.startLogin = startLogin;
    if (answerLogin != null) result.answerLogin = answerLogin;
    if (cancelLogin != null) result.cancelLogin = cancelLogin;
    if (signOut != null) result.signOut = signOut;
    if (deleteAgent != null) result.deleteAgent = deleteAgent;
    if (deleteProject != null) result.deleteProject = deleteProject;
    if (updateProject != null) result.updateProject = updateProject;
    if (updateSettings != null) result.updateSettings = updateSettings;
    if (restoreSettings != null) result.restoreSettings = restoreSettings;
    if (installUpdate != null) result.installUpdate = installUpdate;
    if (focusProject != null) result.focusProject = focusProject;
    if (expandFolder != null) result.expandFolder = expandFolder;
    if (showAllFiles != null) result.showAllFiles = showAllFiles;
    if (openPanel != null) result.openPanel = openPanel;
    if (arrangeAgent != null) result.arrangeAgent = arrangeAgent;
    if (setAgentCollapsed != null) result.setAgentCollapsed = setAgentCollapsed;
    if (runCommand != null) result.runCommand = runCommand;
    if (createFolder != null) result.createFolder = createFolder;
    if (updateFolder != null) result.updateFolder = updateFolder;
    if (deleteFolder != null) result.deleteFolder = deleteFolder;
    if (setFolderCollapsed != null)
      result.setFolderCollapsed = setFolderCollapsed;
    if (setHighlights != null) result.setHighlights = setHighlights;
    if (stopTask != null) result.stopTask = stopTask;
    if (changeMonitor != null) result.changeMonitor = changeMonitor;
    if (createPairing != null) result.createPairing = createPairing;
    if (unpairDevice != null) result.unpairDevice = unpairDevice;
    if (setRemoteAccess != null) result.setRemoteAccess = setRemoteAccess;
    if (fetchApp != null) result.fetchApp = fetchApp;
    if (listFolders != null) result.listFolders = listFolders;
    if (decideProposal != null) result.decideProposal = decideProposal;
    if (moveAccount != null) result.moveAccount = moveAccount;
    if (clearBuildCache != null) result.clearBuildCache = clearBuildCache;
    if (readFile != null) result.readFile = readFile;
    if (setLayout != null) result.setLayout = setLayout;
    if (updateAccount != null) result.updateAccount = updateAccount;
    if (createVault != null) result.createVault = createVault;
    if (unlockVault != null) result.unlockVault = unlockVault;
    if (lockVaultSession != null) result.lockVaultSession = lockVaultSession;
    if (createSecret != null) result.createSecret = createSecret;
    if (updateSecret != null) result.updateSecret = updateSecret;
    if (deleteSecret != null) result.deleteSecret = deleteSecret;
    if (grantSecret != null) result.grantSecret = grantSecret;
    if (revokeSecret != null) result.revokeSecret = revokeSecret;
    if (revealSecret != null) result.revealSecret = revealSecret;
    if (previewEnv != null) result.previewEnv = previewEnv;
    if (importEnv != null) result.importEnv = importEnv;
    if (decideSecretRequest != null)
      result.decideSecretRequest = decideSecretRequest;
    if (listSecretUses != null) result.listSecretUses = listSecretUses;
    if (changePassphrase != null) result.changePassphrase = changePassphrase;
    if (newRecoveryKey != null) result.newRecoveryKey = newRecoveryKey;
    if (setVaultLockedMode != null)
      result.setVaultLockedMode = setVaultLockedMode;
    if (accountSignIn != null) result.accountSignIn = accountSignIn;
    if (accountVerify != null) result.accountVerify = accountVerify;
    if (accountResend != null) result.accountResend = accountResend;
    if (accountSignOut != null) result.accountSignOut = accountSignOut;
    if (setAccountService != null) result.setAccountService = setAccountService;
    if (showRecoveryKey != null) result.showRecoveryKey = showRecoveryKey;
    if (approveDevice != null) result.approveDevice = approveDevice;
    if (denyDevice != null) result.denyDevice = denyDevice;
    if (cancelPairing != null) result.cancelPairing = cancelPairing;
    if (loadEntries != null) result.loadEntries = loadEntries;
    return result;
  }

  ClientMessage._();

  factory ClientMessage.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ClientMessage()..mergeFromBuffer(data, registry);
  factory ClientMessage.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ClientMessage()..mergeFromJson(json, registry);

  static const $core.Map<$core.int, ClientMessage_Body>
      _ClientMessage_BodyByTag = {
    2: ClientMessage_Body.hello,
    3: ClientMessage_Body.createProject,
    4: ClientMessage_Body.createAgent,
    5: ClientMessage_Body.updateAgent,
    6: ClientMessage_Body.selectAgent,
    7: ClientMessage_Body.sendMessage,
    8: ClientMessage_Body.stopAgent,
    9: ClientMessage_Body.startLogin,
    10: ClientMessage_Body.answerLogin,
    11: ClientMessage_Body.cancelLogin,
    12: ClientMessage_Body.signOut,
    13: ClientMessage_Body.deleteAgent,
    14: ClientMessage_Body.deleteProject,
    15: ClientMessage_Body.updateProject,
    16: ClientMessage_Body.updateSettings,
    17: ClientMessage_Body.restoreSettings,
    18: ClientMessage_Body.installUpdate,
    19: ClientMessage_Body.focusProject,
    20: ClientMessage_Body.expandFolder,
    21: ClientMessage_Body.showAllFiles,
    22: ClientMessage_Body.openPanel,
    23: ClientMessage_Body.arrangeAgent,
    24: ClientMessage_Body.setAgentCollapsed,
    25: ClientMessage_Body.runCommand,
    26: ClientMessage_Body.createFolder,
    27: ClientMessage_Body.updateFolder,
    28: ClientMessage_Body.deleteFolder,
    29: ClientMessage_Body.setFolderCollapsed,
    30: ClientMessage_Body.setHighlights,
    31: ClientMessage_Body.stopTask,
    32: ClientMessage_Body.changeMonitor,
    33: ClientMessage_Body.createPairing,
    34: ClientMessage_Body.unpairDevice,
    35: ClientMessage_Body.setRemoteAccess,
    36: ClientMessage_Body.fetchApp,
    37: ClientMessage_Body.listFolders,
    38: ClientMessage_Body.decideProposal,
    39: ClientMessage_Body.moveAccount,
    40: ClientMessage_Body.clearBuildCache,
    41: ClientMessage_Body.readFile,
    42: ClientMessage_Body.setLayout,
    43: ClientMessage_Body.updateAccount,
    44: ClientMessage_Body.createVault,
    45: ClientMessage_Body.unlockVault,
    46: ClientMessage_Body.lockVaultSession,
    47: ClientMessage_Body.createSecret,
    48: ClientMessage_Body.updateSecret,
    49: ClientMessage_Body.deleteSecret,
    50: ClientMessage_Body.grantSecret,
    51: ClientMessage_Body.revokeSecret,
    52: ClientMessage_Body.revealSecret,
    53: ClientMessage_Body.previewEnv,
    54: ClientMessage_Body.importEnv,
    55: ClientMessage_Body.decideSecretRequest,
    56: ClientMessage_Body.listSecretUses,
    57: ClientMessage_Body.changePassphrase,
    58: ClientMessage_Body.newRecoveryKey,
    59: ClientMessage_Body.setVaultLockedMode,
    60: ClientMessage_Body.accountSignIn,
    61: ClientMessage_Body.accountVerify,
    62: ClientMessage_Body.accountResend,
    63: ClientMessage_Body.accountSignOut,
    64: ClientMessage_Body.setAccountService,
    65: ClientMessage_Body.showRecoveryKey,
    66: ClientMessage_Body.approveDevice,
    67: ClientMessage_Body.denyDevice,
    68: ClientMessage_Body.cancelPairing,
    69: ClientMessage_Body.loadEntries,
    0: ClientMessage_Body.notSet
  };
  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ClientMessage',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ClientMessage.$_createMessage)
    ..oo(0, [
      2,
      3,
      4,
      5,
      6,
      7,
      8,
      9,
      10,
      11,
      12,
      13,
      14,
      15,
      16,
      17,
      18,
      19,
      20,
      21,
      22,
      23,
      24,
      25,
      26,
      27,
      28,
      29,
      30,
      31,
      32,
      33,
      34,
      35,
      36,
      37,
      38,
      39,
      40,
      41,
      42,
      43,
      44,
      45,
      46,
      47,
      48,
      49,
      50,
      51,
      52,
      53,
      54,
      55,
      56,
      57,
      58,
      59,
      60,
      61,
      62,
      63,
      64,
      65,
      66,
      67,
      68,
      69
    ])
    ..a<$fixnum.Int64>(1, _omitFieldNames ? '' : 'id', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..aOM<Hello>(2, _omitFieldNames ? '' : 'hello',
        subBuilder: Hello.$_createMessage)
    ..aOM<CreateProject>(3, _omitFieldNames ? '' : 'createProject',
        subBuilder: CreateProject.$_createMessage)
    ..aOM<CreateAgent>(4, _omitFieldNames ? '' : 'createAgent',
        subBuilder: CreateAgent.$_createMessage)
    ..aOM<UpdateAgent>(5, _omitFieldNames ? '' : 'updateAgent',
        subBuilder: UpdateAgent.$_createMessage)
    ..aOM<SelectAgent>(6, _omitFieldNames ? '' : 'selectAgent',
        subBuilder: SelectAgent.$_createMessage)
    ..aOM<SendMessage>(7, _omitFieldNames ? '' : 'sendMessage',
        subBuilder: SendMessage.$_createMessage)
    ..aOM<StopAgent>(8, _omitFieldNames ? '' : 'stopAgent',
        subBuilder: StopAgent.$_createMessage)
    ..aOM<StartLogin>(9, _omitFieldNames ? '' : 'startLogin',
        subBuilder: StartLogin.$_createMessage)
    ..aOM<AnswerLogin>(10, _omitFieldNames ? '' : 'answerLogin',
        subBuilder: AnswerLogin.$_createMessage)
    ..aOM<CancelLogin>(11, _omitFieldNames ? '' : 'cancelLogin',
        subBuilder: CancelLogin.$_createMessage)
    ..aOM<SignOut>(12, _omitFieldNames ? '' : 'signOut',
        subBuilder: SignOut.$_createMessage)
    ..aOM<DeleteAgent>(13, _omitFieldNames ? '' : 'deleteAgent',
        subBuilder: DeleteAgent.$_createMessage)
    ..aOM<DeleteProject>(14, _omitFieldNames ? '' : 'deleteProject',
        subBuilder: DeleteProject.$_createMessage)
    ..aOM<UpdateProject>(15, _omitFieldNames ? '' : 'updateProject',
        subBuilder: UpdateProject.$_createMessage)
    ..aOM<UpdateSettings>(16, _omitFieldNames ? '' : 'updateSettings',
        subBuilder: UpdateSettings.$_createMessage)
    ..aOM<RestoreSettings>(17, _omitFieldNames ? '' : 'restoreSettings',
        subBuilder: RestoreSettings.$_createMessage)
    ..aOM<InstallUpdate>(18, _omitFieldNames ? '' : 'installUpdate',
        subBuilder: InstallUpdate.$_createMessage)
    ..aOM<FocusProject>(19, _omitFieldNames ? '' : 'focusProject',
        subBuilder: FocusProject.$_createMessage)
    ..aOM<ExpandFolder>(20, _omitFieldNames ? '' : 'expandFolder',
        subBuilder: ExpandFolder.$_createMessage)
    ..aOM<ShowAllFiles>(21, _omitFieldNames ? '' : 'showAllFiles',
        subBuilder: ShowAllFiles.$_createMessage)
    ..aOM<OpenPanel>(22, _omitFieldNames ? '' : 'openPanel',
        subBuilder: OpenPanel.$_createMessage)
    ..aOM<ArrangeAgent>(23, _omitFieldNames ? '' : 'arrangeAgent',
        subBuilder: ArrangeAgent.$_createMessage)
    ..aOM<SetAgentCollapsed>(24, _omitFieldNames ? '' : 'setAgentCollapsed',
        subBuilder: SetAgentCollapsed.$_createMessage)
    ..aOM<RunCommand>(25, _omitFieldNames ? '' : 'runCommand',
        subBuilder: RunCommand.$_createMessage)
    ..aOM<CreateFolder>(26, _omitFieldNames ? '' : 'createFolder',
        subBuilder: CreateFolder.$_createMessage)
    ..aOM<UpdateFolder>(27, _omitFieldNames ? '' : 'updateFolder',
        subBuilder: UpdateFolder.$_createMessage)
    ..aOM<DeleteFolder>(28, _omitFieldNames ? '' : 'deleteFolder',
        subBuilder: DeleteFolder.$_createMessage)
    ..aOM<SetFolderCollapsed>(29, _omitFieldNames ? '' : 'setFolderCollapsed',
        subBuilder: SetFolderCollapsed.$_createMessage)
    ..aOM<SetHighlights>(30, _omitFieldNames ? '' : 'setHighlights',
        subBuilder: SetHighlights.$_createMessage)
    ..aOM<StopTask>(31, _omitFieldNames ? '' : 'stopTask',
        subBuilder: StopTask.$_createMessage)
    ..aOM<ChangeMonitor>(32, _omitFieldNames ? '' : 'changeMonitor',
        subBuilder: ChangeMonitor.$_createMessage)
    ..aOM<CreatePairing>(33, _omitFieldNames ? '' : 'createPairing',
        subBuilder: CreatePairing.$_createMessage)
    ..aOM<UnpairDevice>(34, _omitFieldNames ? '' : 'unpairDevice',
        subBuilder: UnpairDevice.$_createMessage)
    ..aOM<SetRemoteAccess>(35, _omitFieldNames ? '' : 'setRemoteAccess',
        subBuilder: SetRemoteAccess.$_createMessage)
    ..aOM<FetchApp>(36, _omitFieldNames ? '' : 'fetchApp',
        subBuilder: FetchApp.$_createMessage)
    ..aOM<ListFolders>(37, _omitFieldNames ? '' : 'listFolders',
        subBuilder: ListFolders.$_createMessage)
    ..aOM<DecideProposal>(38, _omitFieldNames ? '' : 'decideProposal',
        subBuilder: DecideProposal.$_createMessage)
    ..aOM<MoveAccount>(39, _omitFieldNames ? '' : 'moveAccount',
        subBuilder: MoveAccount.$_createMessage)
    ..aOM<ClearBuildCache>(40, _omitFieldNames ? '' : 'clearBuildCache',
        subBuilder: ClearBuildCache.$_createMessage)
    ..aOM<ReadFile>(41, _omitFieldNames ? '' : 'readFile',
        subBuilder: ReadFile.$_createMessage)
    ..aOM<SetLayout>(42, _omitFieldNames ? '' : 'setLayout',
        subBuilder: SetLayout.$_createMessage)
    ..aOM<UpdateAccount>(43, _omitFieldNames ? '' : 'updateAccount',
        subBuilder: UpdateAccount.$_createMessage)
    ..aOM<CreateVault>(44, _omitFieldNames ? '' : 'createVault',
        subBuilder: CreateVault.$_createMessage)
    ..aOM<UnlockVault>(45, _omitFieldNames ? '' : 'unlockVault',
        subBuilder: UnlockVault.$_createMessage)
    ..aOM<LockVaultSession>(46, _omitFieldNames ? '' : 'lockVaultSession',
        subBuilder: LockVaultSession.$_createMessage)
    ..aOM<CreateSecret>(47, _omitFieldNames ? '' : 'createSecret',
        subBuilder: CreateSecret.$_createMessage)
    ..aOM<UpdateSecret>(48, _omitFieldNames ? '' : 'updateSecret',
        subBuilder: UpdateSecret.$_createMessage)
    ..aOM<DeleteSecret>(49, _omitFieldNames ? '' : 'deleteSecret',
        subBuilder: DeleteSecret.$_createMessage)
    ..aOM<GrantSecret>(50, _omitFieldNames ? '' : 'grantSecret',
        subBuilder: GrantSecret.$_createMessage)
    ..aOM<RevokeSecret>(51, _omitFieldNames ? '' : 'revokeSecret',
        subBuilder: RevokeSecret.$_createMessage)
    ..aOM<RevealSecret>(52, _omitFieldNames ? '' : 'revealSecret',
        subBuilder: RevealSecret.$_createMessage)
    ..aOM<PreviewEnv>(53, _omitFieldNames ? '' : 'previewEnv',
        subBuilder: PreviewEnv.$_createMessage)
    ..aOM<ImportEnv>(54, _omitFieldNames ? '' : 'importEnv',
        subBuilder: ImportEnv.$_createMessage)
    ..aOM<DecideSecretRequest>(55, _omitFieldNames ? '' : 'decideSecretRequest',
        subBuilder: DecideSecretRequest.$_createMessage)
    ..aOM<ListSecretUses>(56, _omitFieldNames ? '' : 'listSecretUses',
        subBuilder: ListSecretUses.$_createMessage)
    ..aOM<ChangePassphrase>(57, _omitFieldNames ? '' : 'changePassphrase',
        subBuilder: ChangePassphrase.$_createMessage)
    ..aOM<NewRecoveryKey>(58, _omitFieldNames ? '' : 'newRecoveryKey',
        subBuilder: NewRecoveryKey.$_createMessage)
    ..aOM<SetVaultLockedMode>(59, _omitFieldNames ? '' : 'setVaultLockedMode',
        subBuilder: SetVaultLockedMode.$_createMessage)
    ..aOM<AccountSignIn>(60, _omitFieldNames ? '' : 'accountSignIn',
        subBuilder: AccountSignIn.$_createMessage)
    ..aOM<AccountVerify>(61, _omitFieldNames ? '' : 'accountVerify',
        subBuilder: AccountVerify.$_createMessage)
    ..aOM<AccountResend>(62, _omitFieldNames ? '' : 'accountResend',
        subBuilder: AccountResend.$_createMessage)
    ..aOM<AccountSignOut>(63, _omitFieldNames ? '' : 'accountSignOut',
        subBuilder: AccountSignOut.$_createMessage)
    ..aOM<SetAccountService>(64, _omitFieldNames ? '' : 'setAccountService',
        subBuilder: SetAccountService.$_createMessage)
    ..aOM<ShowRecoveryKey>(65, _omitFieldNames ? '' : 'showRecoveryKey',
        subBuilder: ShowRecoveryKey.$_createMessage)
    ..aOM<ApproveDevice>(66, _omitFieldNames ? '' : 'approveDevice',
        subBuilder: ApproveDevice.$_createMessage)
    ..aOM<DenyDevice>(67, _omitFieldNames ? '' : 'denyDevice',
        subBuilder: DenyDevice.$_createMessage)
    ..aOM<CancelPairing>(68, _omitFieldNames ? '' : 'cancelPairing',
        subBuilder: CancelPairing.$_createMessage)
    ..aOM<LoadEntries>(69, _omitFieldNames ? '' : 'loadEntries',
        subBuilder: LoadEntries.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ClientMessage clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ClientMessage copyWith(void Function(ClientMessage) updates) =>
      super.copyWith((message) => updates(message as ClientMessage))
          as ClientMessage;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ClientMessage() / ClientMessage.new instead')
  static ClientMessage create() => ClientMessage._();
  static $pb.GeneratedMessage $_createMessage() => ClientMessage._();
  @$core.override
  ClientMessage createEmptyInstance() => ClientMessage._();
  @$core.pragma('dart2js:noInline')
  static ClientMessage getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ClientMessage>(
          ClientMessage.$_createMessage);
  static ClientMessage? _defaultInstance;

  @$pb.TagNumber(2)
  @$pb.TagNumber(3)
  @$pb.TagNumber(4)
  @$pb.TagNumber(5)
  @$pb.TagNumber(6)
  @$pb.TagNumber(7)
  @$pb.TagNumber(8)
  @$pb.TagNumber(9)
  @$pb.TagNumber(10)
  @$pb.TagNumber(11)
  @$pb.TagNumber(12)
  @$pb.TagNumber(13)
  @$pb.TagNumber(14)
  @$pb.TagNumber(15)
  @$pb.TagNumber(16)
  @$pb.TagNumber(17)
  @$pb.TagNumber(18)
  @$pb.TagNumber(19)
  @$pb.TagNumber(20)
  @$pb.TagNumber(21)
  @$pb.TagNumber(22)
  @$pb.TagNumber(23)
  @$pb.TagNumber(24)
  @$pb.TagNumber(25)
  @$pb.TagNumber(26)
  @$pb.TagNumber(27)
  @$pb.TagNumber(28)
  @$pb.TagNumber(29)
  @$pb.TagNumber(30)
  @$pb.TagNumber(31)
  @$pb.TagNumber(32)
  @$pb.TagNumber(33)
  @$pb.TagNumber(34)
  @$pb.TagNumber(35)
  @$pb.TagNumber(36)
  @$pb.TagNumber(37)
  @$pb.TagNumber(38)
  @$pb.TagNumber(39)
  @$pb.TagNumber(40)
  @$pb.TagNumber(41)
  @$pb.TagNumber(42)
  @$pb.TagNumber(43)
  @$pb.TagNumber(44)
  @$pb.TagNumber(45)
  @$pb.TagNumber(46)
  @$pb.TagNumber(47)
  @$pb.TagNumber(48)
  @$pb.TagNumber(49)
  @$pb.TagNumber(50)
  @$pb.TagNumber(51)
  @$pb.TagNumber(52)
  @$pb.TagNumber(53)
  @$pb.TagNumber(54)
  @$pb.TagNumber(55)
  @$pb.TagNumber(56)
  @$pb.TagNumber(57)
  @$pb.TagNumber(58)
  @$pb.TagNumber(59)
  @$pb.TagNumber(60)
  @$pb.TagNumber(61)
  @$pb.TagNumber(62)
  @$pb.TagNumber(63)
  @$pb.TagNumber(64)
  @$pb.TagNumber(65)
  @$pb.TagNumber(66)
  @$pb.TagNumber(67)
  @$pb.TagNumber(68)
  @$pb.TagNumber(69)
  ClientMessage_Body whichBody() => _ClientMessage_BodyByTag[$_whichOneof(0)]!;
  @$pb.TagNumber(2)
  @$pb.TagNumber(3)
  @$pb.TagNumber(4)
  @$pb.TagNumber(5)
  @$pb.TagNumber(6)
  @$pb.TagNumber(7)
  @$pb.TagNumber(8)
  @$pb.TagNumber(9)
  @$pb.TagNumber(10)
  @$pb.TagNumber(11)
  @$pb.TagNumber(12)
  @$pb.TagNumber(13)
  @$pb.TagNumber(14)
  @$pb.TagNumber(15)
  @$pb.TagNumber(16)
  @$pb.TagNumber(17)
  @$pb.TagNumber(18)
  @$pb.TagNumber(19)
  @$pb.TagNumber(20)
  @$pb.TagNumber(21)
  @$pb.TagNumber(22)
  @$pb.TagNumber(23)
  @$pb.TagNumber(24)
  @$pb.TagNumber(25)
  @$pb.TagNumber(26)
  @$pb.TagNumber(27)
  @$pb.TagNumber(28)
  @$pb.TagNumber(29)
  @$pb.TagNumber(30)
  @$pb.TagNumber(31)
  @$pb.TagNumber(32)
  @$pb.TagNumber(33)
  @$pb.TagNumber(34)
  @$pb.TagNumber(35)
  @$pb.TagNumber(36)
  @$pb.TagNumber(37)
  @$pb.TagNumber(38)
  @$pb.TagNumber(39)
  @$pb.TagNumber(40)
  @$pb.TagNumber(41)
  @$pb.TagNumber(42)
  @$pb.TagNumber(43)
  @$pb.TagNumber(44)
  @$pb.TagNumber(45)
  @$pb.TagNumber(46)
  @$pb.TagNumber(47)
  @$pb.TagNumber(48)
  @$pb.TagNumber(49)
  @$pb.TagNumber(50)
  @$pb.TagNumber(51)
  @$pb.TagNumber(52)
  @$pb.TagNumber(53)
  @$pb.TagNumber(54)
  @$pb.TagNumber(55)
  @$pb.TagNumber(56)
  @$pb.TagNumber(57)
  @$pb.TagNumber(58)
  @$pb.TagNumber(59)
  @$pb.TagNumber(60)
  @$pb.TagNumber(61)
  @$pb.TagNumber(62)
  @$pb.TagNumber(63)
  @$pb.TagNumber(64)
  @$pb.TagNumber(65)
  @$pb.TagNumber(66)
  @$pb.TagNumber(67)
  @$pb.TagNumber(68)
  @$pb.TagNumber(69)
  void clearBody() => $_clearField($_whichOneof(0));

  @$pb.TagNumber(1)
  $fixnum.Int64 get id => $_getI64(0);
  @$pb.TagNumber(1)
  set id($fixnum.Int64 value) => $_setInt64(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  Hello get hello => $_getN(1);
  @$pb.TagNumber(2)
  set hello(Hello value) => $_setField(2, value);
  @$pb.TagNumber(2)
  $core.bool hasHello() => $_has(1);
  @$pb.TagNumber(2)
  void clearHello() => $_clearField(2);
  @$pb.TagNumber(2)
  Hello ensureHello() => $_ensure(1);

  @$pb.TagNumber(3)
  CreateProject get createProject => $_getN(2);
  @$pb.TagNumber(3)
  set createProject(CreateProject value) => $_setField(3, value);
  @$pb.TagNumber(3)
  $core.bool hasCreateProject() => $_has(2);
  @$pb.TagNumber(3)
  void clearCreateProject() => $_clearField(3);
  @$pb.TagNumber(3)
  CreateProject ensureCreateProject() => $_ensure(2);

  @$pb.TagNumber(4)
  CreateAgent get createAgent => $_getN(3);
  @$pb.TagNumber(4)
  set createAgent(CreateAgent value) => $_setField(4, value);
  @$pb.TagNumber(4)
  $core.bool hasCreateAgent() => $_has(3);
  @$pb.TagNumber(4)
  void clearCreateAgent() => $_clearField(4);
  @$pb.TagNumber(4)
  CreateAgent ensureCreateAgent() => $_ensure(3);

  @$pb.TagNumber(5)
  UpdateAgent get updateAgent => $_getN(4);
  @$pb.TagNumber(5)
  set updateAgent(UpdateAgent value) => $_setField(5, value);
  @$pb.TagNumber(5)
  $core.bool hasUpdateAgent() => $_has(4);
  @$pb.TagNumber(5)
  void clearUpdateAgent() => $_clearField(5);
  @$pb.TagNumber(5)
  UpdateAgent ensureUpdateAgent() => $_ensure(4);

  @$pb.TagNumber(6)
  SelectAgent get selectAgent => $_getN(5);
  @$pb.TagNumber(6)
  set selectAgent(SelectAgent value) => $_setField(6, value);
  @$pb.TagNumber(6)
  $core.bool hasSelectAgent() => $_has(5);
  @$pb.TagNumber(6)
  void clearSelectAgent() => $_clearField(6);
  @$pb.TagNumber(6)
  SelectAgent ensureSelectAgent() => $_ensure(5);

  @$pb.TagNumber(7)
  SendMessage get sendMessage => $_getN(6);
  @$pb.TagNumber(7)
  set sendMessage(SendMessage value) => $_setField(7, value);
  @$pb.TagNumber(7)
  $core.bool hasSendMessage() => $_has(6);
  @$pb.TagNumber(7)
  void clearSendMessage() => $_clearField(7);
  @$pb.TagNumber(7)
  SendMessage ensureSendMessage() => $_ensure(6);

  @$pb.TagNumber(8)
  StopAgent get stopAgent => $_getN(7);
  @$pb.TagNumber(8)
  set stopAgent(StopAgent value) => $_setField(8, value);
  @$pb.TagNumber(8)
  $core.bool hasStopAgent() => $_has(7);
  @$pb.TagNumber(8)
  void clearStopAgent() => $_clearField(8);
  @$pb.TagNumber(8)
  StopAgent ensureStopAgent() => $_ensure(7);

  @$pb.TagNumber(9)
  StartLogin get startLogin => $_getN(8);
  @$pb.TagNumber(9)
  set startLogin(StartLogin value) => $_setField(9, value);
  @$pb.TagNumber(9)
  $core.bool hasStartLogin() => $_has(8);
  @$pb.TagNumber(9)
  void clearStartLogin() => $_clearField(9);
  @$pb.TagNumber(9)
  StartLogin ensureStartLogin() => $_ensure(8);

  @$pb.TagNumber(10)
  AnswerLogin get answerLogin => $_getN(9);
  @$pb.TagNumber(10)
  set answerLogin(AnswerLogin value) => $_setField(10, value);
  @$pb.TagNumber(10)
  $core.bool hasAnswerLogin() => $_has(9);
  @$pb.TagNumber(10)
  void clearAnswerLogin() => $_clearField(10);
  @$pb.TagNumber(10)
  AnswerLogin ensureAnswerLogin() => $_ensure(9);

  @$pb.TagNumber(11)
  CancelLogin get cancelLogin => $_getN(10);
  @$pb.TagNumber(11)
  set cancelLogin(CancelLogin value) => $_setField(11, value);
  @$pb.TagNumber(11)
  $core.bool hasCancelLogin() => $_has(10);
  @$pb.TagNumber(11)
  void clearCancelLogin() => $_clearField(11);
  @$pb.TagNumber(11)
  CancelLogin ensureCancelLogin() => $_ensure(10);

  @$pb.TagNumber(12)
  SignOut get signOut => $_getN(11);
  @$pb.TagNumber(12)
  set signOut(SignOut value) => $_setField(12, value);
  @$pb.TagNumber(12)
  $core.bool hasSignOut() => $_has(11);
  @$pb.TagNumber(12)
  void clearSignOut() => $_clearField(12);
  @$pb.TagNumber(12)
  SignOut ensureSignOut() => $_ensure(11);

  @$pb.TagNumber(13)
  DeleteAgent get deleteAgent => $_getN(12);
  @$pb.TagNumber(13)
  set deleteAgent(DeleteAgent value) => $_setField(13, value);
  @$pb.TagNumber(13)
  $core.bool hasDeleteAgent() => $_has(12);
  @$pb.TagNumber(13)
  void clearDeleteAgent() => $_clearField(13);
  @$pb.TagNumber(13)
  DeleteAgent ensureDeleteAgent() => $_ensure(12);

  @$pb.TagNumber(14)
  DeleteProject get deleteProject => $_getN(13);
  @$pb.TagNumber(14)
  set deleteProject(DeleteProject value) => $_setField(14, value);
  @$pb.TagNumber(14)
  $core.bool hasDeleteProject() => $_has(13);
  @$pb.TagNumber(14)
  void clearDeleteProject() => $_clearField(14);
  @$pb.TagNumber(14)
  DeleteProject ensureDeleteProject() => $_ensure(13);

  @$pb.TagNumber(15)
  UpdateProject get updateProject => $_getN(14);
  @$pb.TagNumber(15)
  set updateProject(UpdateProject value) => $_setField(15, value);
  @$pb.TagNumber(15)
  $core.bool hasUpdateProject() => $_has(14);
  @$pb.TagNumber(15)
  void clearUpdateProject() => $_clearField(15);
  @$pb.TagNumber(15)
  UpdateProject ensureUpdateProject() => $_ensure(14);

  @$pb.TagNumber(16)
  UpdateSettings get updateSettings => $_getN(15);
  @$pb.TagNumber(16)
  set updateSettings(UpdateSettings value) => $_setField(16, value);
  @$pb.TagNumber(16)
  $core.bool hasUpdateSettings() => $_has(15);
  @$pb.TagNumber(16)
  void clearUpdateSettings() => $_clearField(16);
  @$pb.TagNumber(16)
  UpdateSettings ensureUpdateSettings() => $_ensure(15);

  @$pb.TagNumber(17)
  RestoreSettings get restoreSettings => $_getN(16);
  @$pb.TagNumber(17)
  set restoreSettings(RestoreSettings value) => $_setField(17, value);
  @$pb.TagNumber(17)
  $core.bool hasRestoreSettings() => $_has(16);
  @$pb.TagNumber(17)
  void clearRestoreSettings() => $_clearField(17);
  @$pb.TagNumber(17)
  RestoreSettings ensureRestoreSettings() => $_ensure(16);

  @$pb.TagNumber(18)
  InstallUpdate get installUpdate => $_getN(17);
  @$pb.TagNumber(18)
  set installUpdate(InstallUpdate value) => $_setField(18, value);
  @$pb.TagNumber(18)
  $core.bool hasInstallUpdate() => $_has(17);
  @$pb.TagNumber(18)
  void clearInstallUpdate() => $_clearField(18);
  @$pb.TagNumber(18)
  InstallUpdate ensureInstallUpdate() => $_ensure(17);

  @$pb.TagNumber(19)
  FocusProject get focusProject => $_getN(18);
  @$pb.TagNumber(19)
  set focusProject(FocusProject value) => $_setField(19, value);
  @$pb.TagNumber(19)
  $core.bool hasFocusProject() => $_has(18);
  @$pb.TagNumber(19)
  void clearFocusProject() => $_clearField(19);
  @$pb.TagNumber(19)
  FocusProject ensureFocusProject() => $_ensure(18);

  @$pb.TagNumber(20)
  ExpandFolder get expandFolder => $_getN(19);
  @$pb.TagNumber(20)
  set expandFolder(ExpandFolder value) => $_setField(20, value);
  @$pb.TagNumber(20)
  $core.bool hasExpandFolder() => $_has(19);
  @$pb.TagNumber(20)
  void clearExpandFolder() => $_clearField(20);
  @$pb.TagNumber(20)
  ExpandFolder ensureExpandFolder() => $_ensure(19);

  @$pb.TagNumber(21)
  ShowAllFiles get showAllFiles => $_getN(20);
  @$pb.TagNumber(21)
  set showAllFiles(ShowAllFiles value) => $_setField(21, value);
  @$pb.TagNumber(21)
  $core.bool hasShowAllFiles() => $_has(20);
  @$pb.TagNumber(21)
  void clearShowAllFiles() => $_clearField(21);
  @$pb.TagNumber(21)
  ShowAllFiles ensureShowAllFiles() => $_ensure(20);

  @$pb.TagNumber(22)
  OpenPanel get openPanel => $_getN(21);
  @$pb.TagNumber(22)
  set openPanel(OpenPanel value) => $_setField(22, value);
  @$pb.TagNumber(22)
  $core.bool hasOpenPanel() => $_has(21);
  @$pb.TagNumber(22)
  void clearOpenPanel() => $_clearField(22);
  @$pb.TagNumber(22)
  OpenPanel ensureOpenPanel() => $_ensure(21);

  @$pb.TagNumber(23)
  ArrangeAgent get arrangeAgent => $_getN(22);
  @$pb.TagNumber(23)
  set arrangeAgent(ArrangeAgent value) => $_setField(23, value);
  @$pb.TagNumber(23)
  $core.bool hasArrangeAgent() => $_has(22);
  @$pb.TagNumber(23)
  void clearArrangeAgent() => $_clearField(23);
  @$pb.TagNumber(23)
  ArrangeAgent ensureArrangeAgent() => $_ensure(22);

  @$pb.TagNumber(24)
  SetAgentCollapsed get setAgentCollapsed => $_getN(23);
  @$pb.TagNumber(24)
  set setAgentCollapsed(SetAgentCollapsed value) => $_setField(24, value);
  @$pb.TagNumber(24)
  $core.bool hasSetAgentCollapsed() => $_has(23);
  @$pb.TagNumber(24)
  void clearSetAgentCollapsed() => $_clearField(24);
  @$pb.TagNumber(24)
  SetAgentCollapsed ensureSetAgentCollapsed() => $_ensure(23);

  @$pb.TagNumber(25)
  RunCommand get runCommand => $_getN(24);
  @$pb.TagNumber(25)
  set runCommand(RunCommand value) => $_setField(25, value);
  @$pb.TagNumber(25)
  $core.bool hasRunCommand() => $_has(24);
  @$pb.TagNumber(25)
  void clearRunCommand() => $_clearField(25);
  @$pb.TagNumber(25)
  RunCommand ensureRunCommand() => $_ensure(24);

  @$pb.TagNumber(26)
  CreateFolder get createFolder => $_getN(25);
  @$pb.TagNumber(26)
  set createFolder(CreateFolder value) => $_setField(26, value);
  @$pb.TagNumber(26)
  $core.bool hasCreateFolder() => $_has(25);
  @$pb.TagNumber(26)
  void clearCreateFolder() => $_clearField(26);
  @$pb.TagNumber(26)
  CreateFolder ensureCreateFolder() => $_ensure(25);

  @$pb.TagNumber(27)
  UpdateFolder get updateFolder => $_getN(26);
  @$pb.TagNumber(27)
  set updateFolder(UpdateFolder value) => $_setField(27, value);
  @$pb.TagNumber(27)
  $core.bool hasUpdateFolder() => $_has(26);
  @$pb.TagNumber(27)
  void clearUpdateFolder() => $_clearField(27);
  @$pb.TagNumber(27)
  UpdateFolder ensureUpdateFolder() => $_ensure(26);

  @$pb.TagNumber(28)
  DeleteFolder get deleteFolder => $_getN(27);
  @$pb.TagNumber(28)
  set deleteFolder(DeleteFolder value) => $_setField(28, value);
  @$pb.TagNumber(28)
  $core.bool hasDeleteFolder() => $_has(27);
  @$pb.TagNumber(28)
  void clearDeleteFolder() => $_clearField(28);
  @$pb.TagNumber(28)
  DeleteFolder ensureDeleteFolder() => $_ensure(27);

  @$pb.TagNumber(29)
  SetFolderCollapsed get setFolderCollapsed => $_getN(28);
  @$pb.TagNumber(29)
  set setFolderCollapsed(SetFolderCollapsed value) => $_setField(29, value);
  @$pb.TagNumber(29)
  $core.bool hasSetFolderCollapsed() => $_has(28);
  @$pb.TagNumber(29)
  void clearSetFolderCollapsed() => $_clearField(29);
  @$pb.TagNumber(29)
  SetFolderCollapsed ensureSetFolderCollapsed() => $_ensure(28);

  @$pb.TagNumber(30)
  SetHighlights get setHighlights => $_getN(29);
  @$pb.TagNumber(30)
  set setHighlights(SetHighlights value) => $_setField(30, value);
  @$pb.TagNumber(30)
  $core.bool hasSetHighlights() => $_has(29);
  @$pb.TagNumber(30)
  void clearSetHighlights() => $_clearField(30);
  @$pb.TagNumber(30)
  SetHighlights ensureSetHighlights() => $_ensure(29);

  @$pb.TagNumber(31)
  StopTask get stopTask => $_getN(30);
  @$pb.TagNumber(31)
  set stopTask(StopTask value) => $_setField(31, value);
  @$pb.TagNumber(31)
  $core.bool hasStopTask() => $_has(30);
  @$pb.TagNumber(31)
  void clearStopTask() => $_clearField(31);
  @$pb.TagNumber(31)
  StopTask ensureStopTask() => $_ensure(30);

  @$pb.TagNumber(32)
  ChangeMonitor get changeMonitor => $_getN(31);
  @$pb.TagNumber(32)
  set changeMonitor(ChangeMonitor value) => $_setField(32, value);
  @$pb.TagNumber(32)
  $core.bool hasChangeMonitor() => $_has(31);
  @$pb.TagNumber(32)
  void clearChangeMonitor() => $_clearField(32);
  @$pb.TagNumber(32)
  ChangeMonitor ensureChangeMonitor() => $_ensure(31);

  @$pb.TagNumber(33)
  CreatePairing get createPairing => $_getN(32);
  @$pb.TagNumber(33)
  set createPairing(CreatePairing value) => $_setField(33, value);
  @$pb.TagNumber(33)
  $core.bool hasCreatePairing() => $_has(32);
  @$pb.TagNumber(33)
  void clearCreatePairing() => $_clearField(33);
  @$pb.TagNumber(33)
  CreatePairing ensureCreatePairing() => $_ensure(32);

  @$pb.TagNumber(34)
  UnpairDevice get unpairDevice => $_getN(33);
  @$pb.TagNumber(34)
  set unpairDevice(UnpairDevice value) => $_setField(34, value);
  @$pb.TagNumber(34)
  $core.bool hasUnpairDevice() => $_has(33);
  @$pb.TagNumber(34)
  void clearUnpairDevice() => $_clearField(34);
  @$pb.TagNumber(34)
  UnpairDevice ensureUnpairDevice() => $_ensure(33);

  @$pb.TagNumber(35)
  SetRemoteAccess get setRemoteAccess => $_getN(34);
  @$pb.TagNumber(35)
  set setRemoteAccess(SetRemoteAccess value) => $_setField(35, value);
  @$pb.TagNumber(35)
  $core.bool hasSetRemoteAccess() => $_has(34);
  @$pb.TagNumber(35)
  void clearSetRemoteAccess() => $_clearField(35);
  @$pb.TagNumber(35)
  SetRemoteAccess ensureSetRemoteAccess() => $_ensure(34);

  @$pb.TagNumber(36)
  FetchApp get fetchApp => $_getN(35);
  @$pb.TagNumber(36)
  set fetchApp(FetchApp value) => $_setField(36, value);
  @$pb.TagNumber(36)
  $core.bool hasFetchApp() => $_has(35);
  @$pb.TagNumber(36)
  void clearFetchApp() => $_clearField(36);
  @$pb.TagNumber(36)
  FetchApp ensureFetchApp() => $_ensure(35);

  @$pb.TagNumber(37)
  ListFolders get listFolders => $_getN(36);
  @$pb.TagNumber(37)
  set listFolders(ListFolders value) => $_setField(37, value);
  @$pb.TagNumber(37)
  $core.bool hasListFolders() => $_has(36);
  @$pb.TagNumber(37)
  void clearListFolders() => $_clearField(37);
  @$pb.TagNumber(37)
  ListFolders ensureListFolders() => $_ensure(36);

  @$pb.TagNumber(38)
  DecideProposal get decideProposal => $_getN(37);
  @$pb.TagNumber(38)
  set decideProposal(DecideProposal value) => $_setField(38, value);
  @$pb.TagNumber(38)
  $core.bool hasDecideProposal() => $_has(37);
  @$pb.TagNumber(38)
  void clearDecideProposal() => $_clearField(38);
  @$pb.TagNumber(38)
  DecideProposal ensureDecideProposal() => $_ensure(37);

  @$pb.TagNumber(39)
  MoveAccount get moveAccount => $_getN(38);
  @$pb.TagNumber(39)
  set moveAccount(MoveAccount value) => $_setField(39, value);
  @$pb.TagNumber(39)
  $core.bool hasMoveAccount() => $_has(38);
  @$pb.TagNumber(39)
  void clearMoveAccount() => $_clearField(39);
  @$pb.TagNumber(39)
  MoveAccount ensureMoveAccount() => $_ensure(38);

  @$pb.TagNumber(40)
  ClearBuildCache get clearBuildCache => $_getN(39);
  @$pb.TagNumber(40)
  set clearBuildCache(ClearBuildCache value) => $_setField(40, value);
  @$pb.TagNumber(40)
  $core.bool hasClearBuildCache() => $_has(39);
  @$pb.TagNumber(40)
  void clearClearBuildCache() => $_clearField(40);
  @$pb.TagNumber(40)
  ClearBuildCache ensureClearBuildCache() => $_ensure(39);

  @$pb.TagNumber(41)
  ReadFile get readFile => $_getN(40);
  @$pb.TagNumber(41)
  set readFile(ReadFile value) => $_setField(41, value);
  @$pb.TagNumber(41)
  $core.bool hasReadFile() => $_has(40);
  @$pb.TagNumber(41)
  void clearReadFile() => $_clearField(41);
  @$pb.TagNumber(41)
  ReadFile ensureReadFile() => $_ensure(40);

  @$pb.TagNumber(42)
  SetLayout get setLayout => $_getN(41);
  @$pb.TagNumber(42)
  set setLayout(SetLayout value) => $_setField(42, value);
  @$pb.TagNumber(42)
  $core.bool hasSetLayout() => $_has(41);
  @$pb.TagNumber(42)
  void clearSetLayout() => $_clearField(42);
  @$pb.TagNumber(42)
  SetLayout ensureSetLayout() => $_ensure(41);

  @$pb.TagNumber(43)
  UpdateAccount get updateAccount => $_getN(42);
  @$pb.TagNumber(43)
  set updateAccount(UpdateAccount value) => $_setField(43, value);
  @$pb.TagNumber(43)
  $core.bool hasUpdateAccount() => $_has(42);
  @$pb.TagNumber(43)
  void clearUpdateAccount() => $_clearField(43);
  @$pb.TagNumber(43)
  UpdateAccount ensureUpdateAccount() => $_ensure(42);

  /// The vault (Settings → Secrets). The backend makes it by itself, and
  /// the person's requests need no proof: see VaultProof.
  @$pb.TagNumber(44)
  CreateVault get createVault => $_getN(43);
  @$pb.TagNumber(44)
  set createVault(CreateVault value) => $_setField(44, value);
  @$pb.TagNumber(44)
  $core.bool hasCreateVault() => $_has(43);
  @$pb.TagNumber(44)
  void clearCreateVault() => $_clearField(44);
  @$pb.TagNumber(44)
  CreateVault ensureCreateVault() => $_ensure(43);

  @$pb.TagNumber(45)
  UnlockVault get unlockVault => $_getN(44);
  @$pb.TagNumber(45)
  set unlockVault(UnlockVault value) => $_setField(45, value);
  @$pb.TagNumber(45)
  $core.bool hasUnlockVault() => $_has(44);
  @$pb.TagNumber(45)
  void clearUnlockVault() => $_clearField(45);
  @$pb.TagNumber(45)
  UnlockVault ensureUnlockVault() => $_ensure(44);

  @$pb.TagNumber(46)
  LockVaultSession get lockVaultSession => $_getN(45);
  @$pb.TagNumber(46)
  set lockVaultSession(LockVaultSession value) => $_setField(46, value);
  @$pb.TagNumber(46)
  $core.bool hasLockVaultSession() => $_has(45);
  @$pb.TagNumber(46)
  void clearLockVaultSession() => $_clearField(46);
  @$pb.TagNumber(46)
  LockVaultSession ensureLockVaultSession() => $_ensure(45);

  @$pb.TagNumber(47)
  CreateSecret get createSecret => $_getN(46);
  @$pb.TagNumber(47)
  set createSecret(CreateSecret value) => $_setField(47, value);
  @$pb.TagNumber(47)
  $core.bool hasCreateSecret() => $_has(46);
  @$pb.TagNumber(47)
  void clearCreateSecret() => $_clearField(47);
  @$pb.TagNumber(47)
  CreateSecret ensureCreateSecret() => $_ensure(46);

  @$pb.TagNumber(48)
  UpdateSecret get updateSecret => $_getN(47);
  @$pb.TagNumber(48)
  set updateSecret(UpdateSecret value) => $_setField(48, value);
  @$pb.TagNumber(48)
  $core.bool hasUpdateSecret() => $_has(47);
  @$pb.TagNumber(48)
  void clearUpdateSecret() => $_clearField(48);
  @$pb.TagNumber(48)
  UpdateSecret ensureUpdateSecret() => $_ensure(47);

  @$pb.TagNumber(49)
  DeleteSecret get deleteSecret => $_getN(48);
  @$pb.TagNumber(49)
  set deleteSecret(DeleteSecret value) => $_setField(49, value);
  @$pb.TagNumber(49)
  $core.bool hasDeleteSecret() => $_has(48);
  @$pb.TagNumber(49)
  void clearDeleteSecret() => $_clearField(49);
  @$pb.TagNumber(49)
  DeleteSecret ensureDeleteSecret() => $_ensure(48);

  @$pb.TagNumber(50)
  GrantSecret get grantSecret => $_getN(49);
  @$pb.TagNumber(50)
  set grantSecret(GrantSecret value) => $_setField(50, value);
  @$pb.TagNumber(50)
  $core.bool hasGrantSecret() => $_has(49);
  @$pb.TagNumber(50)
  void clearGrantSecret() => $_clearField(50);
  @$pb.TagNumber(50)
  GrantSecret ensureGrantSecret() => $_ensure(49);

  @$pb.TagNumber(51)
  RevokeSecret get revokeSecret => $_getN(50);
  @$pb.TagNumber(51)
  set revokeSecret(RevokeSecret value) => $_setField(51, value);
  @$pb.TagNumber(51)
  $core.bool hasRevokeSecret() => $_has(50);
  @$pb.TagNumber(51)
  void clearRevokeSecret() => $_clearField(51);
  @$pb.TagNumber(51)
  RevokeSecret ensureRevokeSecret() => $_ensure(50);

  @$pb.TagNumber(52)
  RevealSecret get revealSecret => $_getN(51);
  @$pb.TagNumber(52)
  set revealSecret(RevealSecret value) => $_setField(52, value);
  @$pb.TagNumber(52)
  $core.bool hasRevealSecret() => $_has(51);
  @$pb.TagNumber(52)
  void clearRevealSecret() => $_clearField(52);
  @$pb.TagNumber(52)
  RevealSecret ensureRevealSecret() => $_ensure(51);

  @$pb.TagNumber(53)
  PreviewEnv get previewEnv => $_getN(52);
  @$pb.TagNumber(53)
  set previewEnv(PreviewEnv value) => $_setField(53, value);
  @$pb.TagNumber(53)
  $core.bool hasPreviewEnv() => $_has(52);
  @$pb.TagNumber(53)
  void clearPreviewEnv() => $_clearField(53);
  @$pb.TagNumber(53)
  PreviewEnv ensurePreviewEnv() => $_ensure(52);

  @$pb.TagNumber(54)
  ImportEnv get importEnv => $_getN(53);
  @$pb.TagNumber(54)
  set importEnv(ImportEnv value) => $_setField(54, value);
  @$pb.TagNumber(54)
  $core.bool hasImportEnv() => $_has(53);
  @$pb.TagNumber(54)
  void clearImportEnv() => $_clearField(54);
  @$pb.TagNumber(54)
  ImportEnv ensureImportEnv() => $_ensure(53);

  @$pb.TagNumber(55)
  DecideSecretRequest get decideSecretRequest => $_getN(54);
  @$pb.TagNumber(55)
  set decideSecretRequest(DecideSecretRequest value) => $_setField(55, value);
  @$pb.TagNumber(55)
  $core.bool hasDecideSecretRequest() => $_has(54);
  @$pb.TagNumber(55)
  void clearDecideSecretRequest() => $_clearField(55);
  @$pb.TagNumber(55)
  DecideSecretRequest ensureDecideSecretRequest() => $_ensure(54);

  @$pb.TagNumber(56)
  ListSecretUses get listSecretUses => $_getN(55);
  @$pb.TagNumber(56)
  set listSecretUses(ListSecretUses value) => $_setField(56, value);
  @$pb.TagNumber(56)
  $core.bool hasListSecretUses() => $_has(55);
  @$pb.TagNumber(56)
  void clearListSecretUses() => $_clearField(56);
  @$pb.TagNumber(56)
  ListSecretUses ensureListSecretUses() => $_ensure(55);

  @$pb.TagNumber(57)
  ChangePassphrase get changePassphrase => $_getN(56);
  @$pb.TagNumber(57)
  set changePassphrase(ChangePassphrase value) => $_setField(57, value);
  @$pb.TagNumber(57)
  $core.bool hasChangePassphrase() => $_has(56);
  @$pb.TagNumber(57)
  void clearChangePassphrase() => $_clearField(57);
  @$pb.TagNumber(57)
  ChangePassphrase ensureChangePassphrase() => $_ensure(56);

  @$pb.TagNumber(58)
  NewRecoveryKey get newRecoveryKey => $_getN(57);
  @$pb.TagNumber(58)
  set newRecoveryKey(NewRecoveryKey value) => $_setField(58, value);
  @$pb.TagNumber(58)
  $core.bool hasNewRecoveryKey() => $_has(57);
  @$pb.TagNumber(58)
  void clearNewRecoveryKey() => $_clearField(58);
  @$pb.TagNumber(58)
  NewRecoveryKey ensureNewRecoveryKey() => $_ensure(57);

  @$pb.TagNumber(59)
  SetVaultLockedMode get setVaultLockedMode => $_getN(58);
  @$pb.TagNumber(59)
  set setVaultLockedMode(SetVaultLockedMode value) => $_setField(59, value);
  @$pb.TagNumber(59)
  $core.bool hasSetVaultLockedMode() => $_has(58);
  @$pb.TagNumber(59)
  void clearSetVaultLockedMode() => $_clearField(59);
  @$pb.TagNumber(59)
  SetVaultLockedMode ensureSetVaultLockedMode() => $_ensure(58);

  /// This Claktory's sign-in at claktory.com (#116): see ClaktoryAccount.
  @$pb.TagNumber(60)
  AccountSignIn get accountSignIn => $_getN(59);
  @$pb.TagNumber(60)
  set accountSignIn(AccountSignIn value) => $_setField(60, value);
  @$pb.TagNumber(60)
  $core.bool hasAccountSignIn() => $_has(59);
  @$pb.TagNumber(60)
  void clearAccountSignIn() => $_clearField(60);
  @$pb.TagNumber(60)
  AccountSignIn ensureAccountSignIn() => $_ensure(59);

  @$pb.TagNumber(61)
  AccountVerify get accountVerify => $_getN(60);
  @$pb.TagNumber(61)
  set accountVerify(AccountVerify value) => $_setField(61, value);
  @$pb.TagNumber(61)
  $core.bool hasAccountVerify() => $_has(60);
  @$pb.TagNumber(61)
  void clearAccountVerify() => $_clearField(61);
  @$pb.TagNumber(61)
  AccountVerify ensureAccountVerify() => $_ensure(60);

  @$pb.TagNumber(62)
  AccountResend get accountResend => $_getN(61);
  @$pb.TagNumber(62)
  set accountResend(AccountResend value) => $_setField(62, value);
  @$pb.TagNumber(62)
  $core.bool hasAccountResend() => $_has(61);
  @$pb.TagNumber(62)
  void clearAccountResend() => $_clearField(62);
  @$pb.TagNumber(62)
  AccountResend ensureAccountResend() => $_ensure(61);

  @$pb.TagNumber(63)
  AccountSignOut get accountSignOut => $_getN(62);
  @$pb.TagNumber(63)
  set accountSignOut(AccountSignOut value) => $_setField(63, value);
  @$pb.TagNumber(63)
  $core.bool hasAccountSignOut() => $_has(62);
  @$pb.TagNumber(63)
  void clearAccountSignOut() => $_clearField(63);
  @$pb.TagNumber(63)
  AccountSignOut ensureAccountSignOut() => $_ensure(62);

  @$pb.TagNumber(64)
  SetAccountService get setAccountService => $_getN(63);
  @$pb.TagNumber(64)
  set setAccountService(SetAccountService value) => $_setField(64, value);
  @$pb.TagNumber(64)
  $core.bool hasSetAccountService() => $_has(63);
  @$pb.TagNumber(64)
  void clearSetAccountService() => $_clearField(64);
  @$pb.TagNumber(64)
  SetAccountService ensureSetAccountService() => $_ensure(63);

  @$pb.TagNumber(65)
  ShowRecoveryKey get showRecoveryKey => $_getN(64);
  @$pb.TagNumber(65)
  set showRecoveryKey(ShowRecoveryKey value) => $_setField(65, value);
  @$pb.TagNumber(65)
  $core.bool hasShowRecoveryKey() => $_has(64);
  @$pb.TagNumber(65)
  void clearShowRecoveryKey() => $_clearField(65);
  @$pb.TagNumber(65)
  ShowRecoveryKey ensureShowRecoveryKey() => $_ensure(64);

  /// Pairing a device: allowing or turning away the device that used a
  /// code, and cancelling a code (see PendingDevice).
  @$pb.TagNumber(66)
  ApproveDevice get approveDevice => $_getN(65);
  @$pb.TagNumber(66)
  set approveDevice(ApproveDevice value) => $_setField(66, value);
  @$pb.TagNumber(66)
  $core.bool hasApproveDevice() => $_has(65);
  @$pb.TagNumber(66)
  void clearApproveDevice() => $_clearField(66);
  @$pb.TagNumber(66)
  ApproveDevice ensureApproveDevice() => $_ensure(65);

  @$pb.TagNumber(67)
  DenyDevice get denyDevice => $_getN(66);
  @$pb.TagNumber(67)
  set denyDevice(DenyDevice value) => $_setField(67, value);
  @$pb.TagNumber(67)
  $core.bool hasDenyDevice() => $_has(66);
  @$pb.TagNumber(67)
  void clearDenyDevice() => $_clearField(67);
  @$pb.TagNumber(67)
  DenyDevice ensureDenyDevice() => $_ensure(66);

  @$pb.TagNumber(68)
  CancelPairing get cancelPairing => $_getN(67);
  @$pb.TagNumber(68)
  set cancelPairing(CancelPairing value) => $_setField(68, value);
  @$pb.TagNumber(68)
  $core.bool hasCancelPairing() => $_has(67);
  @$pb.TagNumber(68)
  void clearCancelPairing() => $_clearField(68);
  @$pb.TagNumber(68)
  CancelPairing ensureCancelPairing() => $_ensure(67);

  /// Earlier entries of a transcript (see Transcript.has_earlier).
  @$pb.TagNumber(69)
  LoadEntries get loadEntries => $_getN(68);
  @$pb.TagNumber(69)
  set loadEntries(LoadEntries value) => $_setField(69, value);
  @$pb.TagNumber(69)
  $core.bool hasLoadEntries() => $_has(68);
  @$pb.TagNumber(69)
  void clearLoadEntries() => $_clearField(69);
  @$pb.TagNumber(69)
  LoadEntries ensureLoadEntries() => $_ensure(68);
}

enum ServerMessage_Body {
  welcome,
  error,
  ack,
  workspace,
  transcript,
  entry,
  delta,
  explorer,
  extensionView,
  queuedMessages,
  highlight,
  pairingCode,
  appChunk,
  folderList,
  fileContent,
  vaultSession,
  secretValue,
  envPreview,
  recoveryKey,
  secretUses,
  pairingPending,
  entryPage,
  notSet
}

class ServerMessage extends $pb.GeneratedMessage {
  factory ServerMessage({
    $fixnum.Int64? replyTo,
    Welcome? welcome,
    Error? error,
    Ack? ack,
    Workspace? workspace,
    Transcript? transcript,
    EntryUpdate? entry,
    EntryDelta? delta,
    Explorer? explorer,
    ExtensionView? extensionView,
    QueuedMessages? queuedMessages,
    Highlight? highlight,
    PairingCode? pairingCode,
    AppChunk? appChunk,
    FolderList? folderList,
    FileContent? fileContent,
    VaultSession? vaultSession,
    SecretValue? secretValue,
    EnvPreview? envPreview,
    RecoveryKey? recoveryKey,
    SecretUses? secretUses,
    PairingPending? pairingPending,
    EntryPage? entryPage,
  }) {
    final result = ServerMessage._();
    if (replyTo != null) result.replyTo = replyTo;
    if (welcome != null) result.welcome = welcome;
    if (error != null) result.error = error;
    if (ack != null) result.ack = ack;
    if (workspace != null) result.workspace = workspace;
    if (transcript != null) result.transcript = transcript;
    if (entry != null) result.entry = entry;
    if (delta != null) result.delta = delta;
    if (explorer != null) result.explorer = explorer;
    if (extensionView != null) result.extensionView = extensionView;
    if (queuedMessages != null) result.queuedMessages = queuedMessages;
    if (highlight != null) result.highlight = highlight;
    if (pairingCode != null) result.pairingCode = pairingCode;
    if (appChunk != null) result.appChunk = appChunk;
    if (folderList != null) result.folderList = folderList;
    if (fileContent != null) result.fileContent = fileContent;
    if (vaultSession != null) result.vaultSession = vaultSession;
    if (secretValue != null) result.secretValue = secretValue;
    if (envPreview != null) result.envPreview = envPreview;
    if (recoveryKey != null) result.recoveryKey = recoveryKey;
    if (secretUses != null) result.secretUses = secretUses;
    if (pairingPending != null) result.pairingPending = pairingPending;
    if (entryPage != null) result.entryPage = entryPage;
    return result;
  }

  ServerMessage._();

  factory ServerMessage.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ServerMessage()..mergeFromBuffer(data, registry);
  factory ServerMessage.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ServerMessage()..mergeFromJson(json, registry);

  static const $core.Map<$core.int, ServerMessage_Body>
      _ServerMessage_BodyByTag = {
    2: ServerMessage_Body.welcome,
    3: ServerMessage_Body.error,
    4: ServerMessage_Body.ack,
    5: ServerMessage_Body.workspace,
    6: ServerMessage_Body.transcript,
    7: ServerMessage_Body.entry,
    8: ServerMessage_Body.delta,
    9: ServerMessage_Body.explorer,
    10: ServerMessage_Body.extensionView,
    11: ServerMessage_Body.queuedMessages,
    12: ServerMessage_Body.highlight,
    13: ServerMessage_Body.pairingCode,
    14: ServerMessage_Body.appChunk,
    15: ServerMessage_Body.folderList,
    16: ServerMessage_Body.fileContent,
    20: ServerMessage_Body.vaultSession,
    21: ServerMessage_Body.secretValue,
    22: ServerMessage_Body.envPreview,
    23: ServerMessage_Body.recoveryKey,
    24: ServerMessage_Body.secretUses,
    25: ServerMessage_Body.pairingPending,
    26: ServerMessage_Body.entryPage,
    0: ServerMessage_Body.notSet
  };
  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ServerMessage',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ServerMessage.$_createMessage)
    ..oo(0, [
      2,
      3,
      4,
      5,
      6,
      7,
      8,
      9,
      10,
      11,
      12,
      13,
      14,
      15,
      16,
      20,
      21,
      22,
      23,
      24,
      25,
      26
    ])
    ..a<$fixnum.Int64>(1, _omitFieldNames ? '' : 'replyTo', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..aOM<Welcome>(2, _omitFieldNames ? '' : 'welcome',
        subBuilder: Welcome.$_createMessage)
    ..aOM<Error>(3, _omitFieldNames ? '' : 'error',
        subBuilder: Error.$_createMessage)
    ..aOM<Ack>(4, _omitFieldNames ? '' : 'ack', subBuilder: Ack.$_createMessage)
    ..aOM<Workspace>(5, _omitFieldNames ? '' : 'workspace',
        subBuilder: Workspace.$_createMessage)
    ..aOM<Transcript>(6, _omitFieldNames ? '' : 'transcript',
        subBuilder: Transcript.$_createMessage)
    ..aOM<EntryUpdate>(7, _omitFieldNames ? '' : 'entry',
        subBuilder: EntryUpdate.$_createMessage)
    ..aOM<EntryDelta>(8, _omitFieldNames ? '' : 'delta',
        subBuilder: EntryDelta.$_createMessage)
    ..aOM<Explorer>(9, _omitFieldNames ? '' : 'explorer',
        subBuilder: Explorer.$_createMessage)
    ..aOM<ExtensionView>(10, _omitFieldNames ? '' : 'extensionView',
        subBuilder: ExtensionView.$_createMessage)
    ..aOM<QueuedMessages>(11, _omitFieldNames ? '' : 'queuedMessages',
        subBuilder: QueuedMessages.$_createMessage)
    ..aOM<Highlight>(12, _omitFieldNames ? '' : 'highlight',
        subBuilder: Highlight.$_createMessage)
    ..aOM<PairingCode>(13, _omitFieldNames ? '' : 'pairingCode',
        subBuilder: PairingCode.$_createMessage)
    ..aOM<AppChunk>(14, _omitFieldNames ? '' : 'appChunk',
        subBuilder: AppChunk.$_createMessage)
    ..aOM<FolderList>(15, _omitFieldNames ? '' : 'folderList',
        subBuilder: FolderList.$_createMessage)
    ..aOM<FileContent>(16, _omitFieldNames ? '' : 'fileContent',
        subBuilder: FileContent.$_createMessage)
    ..aOM<VaultSession>(20, _omitFieldNames ? '' : 'vaultSession',
        subBuilder: VaultSession.$_createMessage)
    ..aOM<SecretValue>(21, _omitFieldNames ? '' : 'secretValue',
        subBuilder: SecretValue.$_createMessage)
    ..aOM<EnvPreview>(22, _omitFieldNames ? '' : 'envPreview',
        subBuilder: EnvPreview.$_createMessage)
    ..aOM<RecoveryKey>(23, _omitFieldNames ? '' : 'recoveryKey',
        subBuilder: RecoveryKey.$_createMessage)
    ..aOM<SecretUses>(24, _omitFieldNames ? '' : 'secretUses',
        subBuilder: SecretUses.$_createMessage)
    ..aOM<PairingPending>(25, _omitFieldNames ? '' : 'pairingPending',
        subBuilder: PairingPending.$_createMessage)
    ..aOM<EntryPage>(26, _omitFieldNames ? '' : 'entryPage',
        subBuilder: EntryPage.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ServerMessage clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ServerMessage copyWith(void Function(ServerMessage) updates) =>
      super.copyWith((message) => updates(message as ServerMessage))
          as ServerMessage;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ServerMessage() / ServerMessage.new instead')
  static ServerMessage create() => ServerMessage._();
  static $pb.GeneratedMessage $_createMessage() => ServerMessage._();
  @$core.override
  ServerMessage createEmptyInstance() => ServerMessage._();
  @$core.pragma('dart2js:noInline')
  static ServerMessage getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ServerMessage>(
          ServerMessage.$_createMessage);
  static ServerMessage? _defaultInstance;

  @$pb.TagNumber(2)
  @$pb.TagNumber(3)
  @$pb.TagNumber(4)
  @$pb.TagNumber(5)
  @$pb.TagNumber(6)
  @$pb.TagNumber(7)
  @$pb.TagNumber(8)
  @$pb.TagNumber(9)
  @$pb.TagNumber(10)
  @$pb.TagNumber(11)
  @$pb.TagNumber(12)
  @$pb.TagNumber(13)
  @$pb.TagNumber(14)
  @$pb.TagNumber(15)
  @$pb.TagNumber(16)
  @$pb.TagNumber(20)
  @$pb.TagNumber(21)
  @$pb.TagNumber(22)
  @$pb.TagNumber(23)
  @$pb.TagNumber(24)
  @$pb.TagNumber(25)
  @$pb.TagNumber(26)
  ServerMessage_Body whichBody() => _ServerMessage_BodyByTag[$_whichOneof(0)]!;
  @$pb.TagNumber(2)
  @$pb.TagNumber(3)
  @$pb.TagNumber(4)
  @$pb.TagNumber(5)
  @$pb.TagNumber(6)
  @$pb.TagNumber(7)
  @$pb.TagNumber(8)
  @$pb.TagNumber(9)
  @$pb.TagNumber(10)
  @$pb.TagNumber(11)
  @$pb.TagNumber(12)
  @$pb.TagNumber(13)
  @$pb.TagNumber(14)
  @$pb.TagNumber(15)
  @$pb.TagNumber(16)
  @$pb.TagNumber(20)
  @$pb.TagNumber(21)
  @$pb.TagNumber(22)
  @$pb.TagNumber(23)
  @$pb.TagNumber(24)
  @$pb.TagNumber(25)
  @$pb.TagNumber(26)
  void clearBody() => $_clearField($_whichOneof(0));

  @$pb.TagNumber(1)
  $fixnum.Int64 get replyTo => $_getI64(0);
  @$pb.TagNumber(1)
  set replyTo($fixnum.Int64 value) => $_setInt64(0, value);
  @$pb.TagNumber(1)
  $core.bool hasReplyTo() => $_has(0);
  @$pb.TagNumber(1)
  void clearReplyTo() => $_clearField(1);

  @$pb.TagNumber(2)
  Welcome get welcome => $_getN(1);
  @$pb.TagNumber(2)
  set welcome(Welcome value) => $_setField(2, value);
  @$pb.TagNumber(2)
  $core.bool hasWelcome() => $_has(1);
  @$pb.TagNumber(2)
  void clearWelcome() => $_clearField(2);
  @$pb.TagNumber(2)
  Welcome ensureWelcome() => $_ensure(1);

  @$pb.TagNumber(3)
  Error get error => $_getN(2);
  @$pb.TagNumber(3)
  set error(Error value) => $_setField(3, value);
  @$pb.TagNumber(3)
  $core.bool hasError() => $_has(2);
  @$pb.TagNumber(3)
  void clearError() => $_clearField(3);
  @$pb.TagNumber(3)
  Error ensureError() => $_ensure(2);

  @$pb.TagNumber(4)
  Ack get ack => $_getN(3);
  @$pb.TagNumber(4)
  set ack(Ack value) => $_setField(4, value);
  @$pb.TagNumber(4)
  $core.bool hasAck() => $_has(3);
  @$pb.TagNumber(4)
  void clearAck() => $_clearField(4);
  @$pb.TagNumber(4)
  Ack ensureAck() => $_ensure(3);

  @$pb.TagNumber(5)
  Workspace get workspace => $_getN(4);
  @$pb.TagNumber(5)
  set workspace(Workspace value) => $_setField(5, value);
  @$pb.TagNumber(5)
  $core.bool hasWorkspace() => $_has(4);
  @$pb.TagNumber(5)
  void clearWorkspace() => $_clearField(5);
  @$pb.TagNumber(5)
  Workspace ensureWorkspace() => $_ensure(4);

  @$pb.TagNumber(6)
  Transcript get transcript => $_getN(5);
  @$pb.TagNumber(6)
  set transcript(Transcript value) => $_setField(6, value);
  @$pb.TagNumber(6)
  $core.bool hasTranscript() => $_has(5);
  @$pb.TagNumber(6)
  void clearTranscript() => $_clearField(6);
  @$pb.TagNumber(6)
  Transcript ensureTranscript() => $_ensure(5);

  @$pb.TagNumber(7)
  EntryUpdate get entry => $_getN(6);
  @$pb.TagNumber(7)
  set entry(EntryUpdate value) => $_setField(7, value);
  @$pb.TagNumber(7)
  $core.bool hasEntry() => $_has(6);
  @$pb.TagNumber(7)
  void clearEntry() => $_clearField(7);
  @$pb.TagNumber(7)
  EntryUpdate ensureEntry() => $_ensure(6);

  @$pb.TagNumber(8)
  EntryDelta get delta => $_getN(7);
  @$pb.TagNumber(8)
  set delta(EntryDelta value) => $_setField(8, value);
  @$pb.TagNumber(8)
  $core.bool hasDelta() => $_has(7);
  @$pb.TagNumber(8)
  void clearDelta() => $_clearField(8);
  @$pb.TagNumber(8)
  EntryDelta ensureDelta() => $_ensure(7);

  @$pb.TagNumber(9)
  Explorer get explorer => $_getN(8);
  @$pb.TagNumber(9)
  set explorer(Explorer value) => $_setField(9, value);
  @$pb.TagNumber(9)
  $core.bool hasExplorer() => $_has(8);
  @$pb.TagNumber(9)
  void clearExplorer() => $_clearField(9);
  @$pb.TagNumber(9)
  Explorer ensureExplorer() => $_ensure(8);

  @$pb.TagNumber(10)
  ExtensionView get extensionView => $_getN(9);
  @$pb.TagNumber(10)
  set extensionView(ExtensionView value) => $_setField(10, value);
  @$pb.TagNumber(10)
  $core.bool hasExtensionView() => $_has(9);
  @$pb.TagNumber(10)
  void clearExtensionView() => $_clearField(10);
  @$pb.TagNumber(10)
  ExtensionView ensureExtensionView() => $_ensure(9);

  @$pb.TagNumber(11)
  QueuedMessages get queuedMessages => $_getN(10);
  @$pb.TagNumber(11)
  set queuedMessages(QueuedMessages value) => $_setField(11, value);
  @$pb.TagNumber(11)
  $core.bool hasQueuedMessages() => $_has(10);
  @$pb.TagNumber(11)
  void clearQueuedMessages() => $_clearField(11);
  @$pb.TagNumber(11)
  QueuedMessages ensureQueuedMessages() => $_ensure(10);

  @$pb.TagNumber(12)
  Highlight get highlight => $_getN(11);
  @$pb.TagNumber(12)
  set highlight(Highlight value) => $_setField(12, value);
  @$pb.TagNumber(12)
  $core.bool hasHighlight() => $_has(11);
  @$pb.TagNumber(12)
  void clearHighlight() => $_clearField(12);
  @$pb.TagNumber(12)
  Highlight ensureHighlight() => $_ensure(11);

  /// The answer to CreatePairing, to the asker only.
  @$pb.TagNumber(13)
  PairingCode get pairingCode => $_getN(12);
  @$pb.TagNumber(13)
  set pairingCode(PairingCode value) => $_setField(13, value);
  @$pb.TagNumber(13)
  $core.bool hasPairingCode() => $_has(12);
  @$pb.TagNumber(13)
  void clearPairingCode() => $_clearField(13);
  @$pb.TagNumber(13)
  PairingCode ensurePairingCode() => $_ensure(12);

  /// The answer to FetchApp, to the asker only.
  @$pb.TagNumber(14)
  AppChunk get appChunk => $_getN(13);
  @$pb.TagNumber(14)
  set appChunk(AppChunk value) => $_setField(14, value);
  @$pb.TagNumber(14)
  $core.bool hasAppChunk() => $_has(13);
  @$pb.TagNumber(14)
  void clearAppChunk() => $_clearField(14);
  @$pb.TagNumber(14)
  AppChunk ensureAppChunk() => $_ensure(13);

  /// The answer to ListFolders, to the asker only.
  @$pb.TagNumber(15)
  FolderList get folderList => $_getN(14);
  @$pb.TagNumber(15)
  set folderList(FolderList value) => $_setField(15, value);
  @$pb.TagNumber(15)
  $core.bool hasFolderList() => $_has(14);
  @$pb.TagNumber(15)
  void clearFolderList() => $_clearField(15);
  @$pb.TagNumber(15)
  FolderList ensureFolderList() => $_ensure(14);

  /// The answer to ReadFile, to the asker only.
  @$pb.TagNumber(16)
  FileContent get fileContent => $_getN(15);
  @$pb.TagNumber(16)
  set fileContent(FileContent value) => $_setField(16, value);
  @$pb.TagNumber(16)
  $core.bool hasFileContent() => $_has(15);
  @$pb.TagNumber(16)
  void clearFileContent() => $_clearField(16);
  @$pb.TagNumber(16)
  FileContent ensureFileContent() => $_ensure(15);

  /// The answer, to the asker only, to UnlockVault, ChangePassphrase,
  /// DeleteSecret, GrantSecret, SetVaultLockedMode and LockVaultSession;
  /// always 0 now (see VaultSession).
  @$pb.TagNumber(20)
  VaultSession get vaultSession => $_getN(16);
  @$pb.TagNumber(20)
  set vaultSession(VaultSession value) => $_setField(20, value);
  @$pb.TagNumber(20)
  $core.bool hasVaultSession() => $_has(16);
  @$pb.TagNumber(20)
  void clearVaultSession() => $_clearField(20);
  @$pb.TagNumber(20)
  VaultSession ensureVaultSession() => $_ensure(16);

  /// The answer to RevealSecret, to the asker only.
  @$pb.TagNumber(21)
  SecretValue get secretValue => $_getN(17);
  @$pb.TagNumber(21)
  set secretValue(SecretValue value) => $_setField(21, value);
  @$pb.TagNumber(21)
  $core.bool hasSecretValue() => $_has(17);
  @$pb.TagNumber(21)
  void clearSecretValue() => $_clearField(21);
  @$pb.TagNumber(21)
  SecretValue ensureSecretValue() => $_ensure(17);

  /// The answer to PreviewEnv, to the asker only.
  @$pb.TagNumber(22)
  EnvPreview get envPreview => $_getN(18);
  @$pb.TagNumber(22)
  set envPreview(EnvPreview value) => $_setField(22, value);
  @$pb.TagNumber(22)
  $core.bool hasEnvPreview() => $_has(18);
  @$pb.TagNumber(22)
  void clearEnvPreview() => $_clearField(22);
  @$pb.TagNumber(22)
  EnvPreview ensureEnvPreview() => $_ensure(18);

  /// The answer to CreateVault and NewRecoveryKey, to the asker only.
  @$pb.TagNumber(23)
  RecoveryKey get recoveryKey => $_getN(19);
  @$pb.TagNumber(23)
  set recoveryKey(RecoveryKey value) => $_setField(23, value);
  @$pb.TagNumber(23)
  $core.bool hasRecoveryKey() => $_has(19);
  @$pb.TagNumber(23)
  void clearRecoveryKey() => $_clearField(23);
  @$pb.TagNumber(23)
  RecoveryKey ensureRecoveryKey() => $_ensure(19);

  /// The answer to ListSecretUses, to the asker only.
  @$pb.TagNumber(24)
  SecretUses get secretUses => $_getN(20);
  @$pb.TagNumber(24)
  set secretUses(SecretUses value) => $_setField(24, value);
  @$pb.TagNumber(24)
  $core.bool hasSecretUses() => $_has(20);
  @$pb.TagNumber(24)
  void clearSecretUses() => $_clearField(24);
  @$pb.TagNumber(24)
  SecretUses ensureSecretUses() => $_ensure(20);

  /// To a device's Hello, with its pairing code's token, while it waits for
  /// the person to allow it (see PendingDevice).
  @$pb.TagNumber(25)
  PairingPending get pairingPending => $_getN(21);
  @$pb.TagNumber(25)
  set pairingPending(PairingPending value) => $_setField(25, value);
  @$pb.TagNumber(25)
  $core.bool hasPairingPending() => $_has(21);
  @$pb.TagNumber(25)
  void clearPairingPending() => $_clearField(25);
  @$pb.TagNumber(25)
  PairingPending ensurePairingPending() => $_ensure(21);

  /// The answer to LoadEntries, to the asker only.
  @$pb.TagNumber(26)
  EntryPage get entryPage => $_getN(22);
  @$pb.TagNumber(26)
  set entryPage(EntryPage value) => $_setField(26, value);
  @$pb.TagNumber(26)
  $core.bool hasEntryPage() => $_has(22);
  @$pb.TagNumber(26)
  void clearEntryPage() => $_clearField(26);
  @$pb.TagNumber(26)
  EntryPage ensureEntryPage() => $_ensure(22);
}

class Hello extends $pb.GeneratedMessage {
  factory Hello({
    $core.int? protocolVersion,
    $core.String? clientVersion,
    $core.String? token,
    DeviceKind? deviceKind,
  }) {
    final result = Hello._();
    if (protocolVersion != null) result.protocolVersion = protocolVersion;
    if (clientVersion != null) result.clientVersion = clientVersion;
    if (token != null) result.token = token;
    if (deviceKind != null) result.deviceKind = deviceKind;
    return result;
  }

  Hello._();

  factory Hello.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Hello()..mergeFromBuffer(data, registry);
  factory Hello.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Hello()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Hello',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Hello.$_createMessage)
    ..aI(1, _omitFieldNames ? '' : 'protocolVersion',
        fieldType: $pb.PbFieldType.OU3)
    ..aOS(2, _omitFieldNames ? '' : 'clientVersion')
    ..aOS(3, _omitFieldNames ? '' : 'token')
    ..aE<DeviceKind>(4, _omitFieldNames ? '' : 'deviceKind',
        enumValues: DeviceKind.values)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Hello clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Hello copyWith(void Function(Hello) updates) =>
      super.copyWith((message) => updates(message as Hello)) as Hello;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Hello() / Hello.new instead')
  static Hello create() => Hello._();
  static $pb.GeneratedMessage $_createMessage() => Hello._();
  @$core.override
  Hello createEmptyInstance() => Hello._();
  @$core.pragma('dart2js:noInline')
  static Hello getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Hello>(Hello.$_createMessage);
  static Hello? _defaultInstance;

  @$pb.TagNumber(1)
  $core.int get protocolVersion => $_getIZ(0);
  @$pb.TagNumber(1)
  set protocolVersion($core.int value) => $_setUnsignedInt32(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProtocolVersion() => $_has(0);
  @$pb.TagNumber(1)
  void clearProtocolVersion() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get clientVersion => $_getSZ(1);
  @$pb.TagNumber(2)
  set clientVersion($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasClientVersion() => $_has(1);
  @$pb.TagNumber(2)
  void clearClientVersion() => $_clearField(2);

  /// The device's token from its pairing code (`factory://…?t=<token>`),
  /// through the private tunnel (iroh); on the loopback port (7420) it isn't
  /// needed.
  @$pb.TagNumber(3)
  $core.String get token => $_getSZ(2);
  @$pb.TagNumber(3)
  set token($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasToken() => $_has(2);
  @$pb.TagNumber(3)
  void clearToken() => $_clearField(3);

  /// What kind of device this is, so agents can fit their replies to it
  /// (shorter on a phone). Its name the backend knows: the paired device's,
  /// or this computer's for its own app. Unspecified from apps before it.
  @$pb.TagNumber(4)
  DeviceKind get deviceKind => $_getN(3);
  @$pb.TagNumber(4)
  set deviceKind(DeviceKind value) => $_setField(4, value);
  @$pb.TagNumber(4)
  $core.bool hasDeviceKind() => $_has(3);
  @$pb.TagNumber(4)
  void clearDeviceKind() => $_clearField(4);
}

/// Where the person sent a message from: the device their app ran on.
class SentFrom extends $pb.GeneratedMessage {
  factory SentFrom({
    $core.String? deviceName,
    DeviceKind? kind,
  }) {
    final result = SentFrom._();
    if (deviceName != null) result.deviceName = deviceName;
    if (kind != null) result.kind = kind;
    return result;
  }

  SentFrom._();

  factory SentFrom.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SentFrom()..mergeFromBuffer(data, registry);
  factory SentFrom.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SentFrom()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SentFrom',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SentFrom.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'deviceName')
    ..aE<DeviceKind>(2, _omitFieldNames ? '' : 'kind',
        enumValues: DeviceKind.values)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SentFrom clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SentFrom copyWith(void Function(SentFrom) updates) =>
      super.copyWith((message) => updates(message as SentFrom)) as SentFrom;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SentFrom() / SentFrom.new instead')
  static SentFrom create() => SentFrom._();
  static $pb.GeneratedMessage $_createMessage() => SentFrom._();
  @$core.override
  SentFrom createEmptyInstance() => SentFrom._();
  @$core.pragma('dart2js:noInline')
  static SentFrom getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<SentFrom>(SentFrom.$_createMessage);
  static SentFrom? _defaultInstance;

  /// The paired device's name (Settings → Devices), or this computer's name
  /// (its Computer Name on a Mac) for the app running on it.
  @$pb.TagNumber(1)
  $core.String get deviceName => $_getSZ(0);
  @$pb.TagNumber(1)
  set deviceName($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasDeviceName() => $_has(0);
  @$pb.TagNumber(1)
  void clearDeviceName() => $_clearField(1);

  /// As the app said in Hello; unspecified from apps before it.
  @$pb.TagNumber(2)
  DeviceKind get kind => $_getN(1);
  @$pb.TagNumber(2)
  set kind(DeviceKind value) => $_setField(2, value);
  @$pb.TagNumber(2)
  $core.bool hasKind() => $_has(1);
  @$pb.TagNumber(2)
  void clearKind() => $_clearField(2);
}

class Welcome extends $pb.GeneratedMessage {
  factory Welcome({
    $core.int? protocolVersion,
    $core.String? serverVersion,
    $core.String? deviceId,
  }) {
    final result = Welcome._();
    if (protocolVersion != null) result.protocolVersion = protocolVersion;
    if (serverVersion != null) result.serverVersion = serverVersion;
    if (deviceId != null) result.deviceId = deviceId;
    return result;
  }

  Welcome._();

  factory Welcome.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Welcome()..mergeFromBuffer(data, registry);
  factory Welcome.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Welcome()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Welcome',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Welcome.$_createMessage)
    ..aI(1, _omitFieldNames ? '' : 'protocolVersion',
        fieldType: $pb.PbFieldType.OU3)
    ..aOS(2, _omitFieldNames ? '' : 'serverVersion')
    ..aOS(3, _omitFieldNames ? '' : 'deviceId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Welcome clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Welcome copyWith(void Function(Welcome) updates) =>
      super.copyWith((message) => updates(message as Welcome)) as Welcome;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Welcome() / Welcome.new instead')
  static Welcome create() => Welcome._();
  static $pb.GeneratedMessage $_createMessage() => Welcome._();
  @$core.override
  Welcome createEmptyInstance() => Welcome._();
  @$core.pragma('dart2js:noInline')
  static Welcome getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Welcome>(Welcome.$_createMessage);
  static Welcome? _defaultInstance;

  @$pb.TagNumber(1)
  $core.int get protocolVersion => $_getIZ(0);
  @$pb.TagNumber(1)
  set protocolVersion($core.int value) => $_setUnsignedInt32(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProtocolVersion() => $_has(0);
  @$pb.TagNumber(1)
  void clearProtocolVersion() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get serverVersion => $_getSZ(1);
  @$pb.TagNumber(2)
  set serverVersion($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasServerVersion() => $_has(1);
  @$pb.TagNumber(2)
  void clearServerVersion() => $_clearField(2);

  /// Through the tunnel, the paired device this connection is (see
  /// RemoteAccess.devices), so the app can mark it "This device"; "local" for
  /// this machine's own app (its code from <data folder>/remote/local-pairing);
  /// empty on the loopback port.
  @$pb.TagNumber(3)
  $core.String get deviceId => $_getSZ(2);
  @$pb.TagNumber(3)
  set deviceId($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasDeviceId() => $_has(2);
  @$pb.TagNumber(3)
  void clearDeviceId() => $_clearField(3);
}

/// Pairs a new device with this Factory. Answered with PairingCode. Refused
/// while remote access is off (RemoteAccess.on).
class CreatePairing extends $pb.GeneratedMessage {
  factory CreatePairing({
    $core.String? name,
    $core.String? host,
  }) {
    final result = CreatePairing._();
    if (name != null) result.name = name;
    if (host != null) result.host = host;
    return result;
  }

  CreatePairing._();

  factory CreatePairing.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CreatePairing()..mergeFromBuffer(data, registry);
  factory CreatePairing.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CreatePairing()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'CreatePairing',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: CreatePairing.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'name')
    ..aOS(2, _omitFieldNames ? '' : 'host')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CreatePairing clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CreatePairing copyWith(void Function(CreatePairing) updates) =>
      super.copyWith((message) => updates(message as CreatePairing))
          as CreatePairing;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use CreatePairing() / CreatePairing.new instead')
  static CreatePairing create() => CreatePairing._();
  static $pb.GeneratedMessage $_createMessage() => CreatePairing._();
  @$core.override
  CreatePairing createEmptyInstance() => CreatePairing._();
  @$core.pragma('dart2js:noInline')
  static CreatePairing getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<CreatePairing>(
          CreatePairing.$_createMessage);
  static CreatePairing? _defaultInstance;

  /// What the device is called, e.g. "Laptop".
  @$pb.TagNumber(1)
  $core.String get name => $_getSZ(0);
  @$pb.TagNumber(1)
  set name($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasName() => $_has(0);
  @$pb.TagNumber(1)
  void clearName() => $_clearField(1);

  /// No longer used: devices reach this machine through the tunnel, by its
  /// endpoint id (RemoteAccess.endpoint_id), wherever they are.
  @$pb.TagNumber(2)
  $core.String get host => $_getSZ(1);
  @$pb.TagNumber(2)
  set host($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasHost() => $_has(1);
  @$pb.TagNumber(2)
  void clearHost() => $_clearField(2);
}

/// A new device's pairing code. Sent only to the client that asked, once:
/// Factory keeps only a hash of the key in it.
class PairingCode extends $pb.GeneratedMessage {
  factory PairingCode({
    $core.String? code,
    $core.String? deviceId,
  }) {
    final result = PairingCode._();
    if (code != null) result.code = code;
    if (deviceId != null) result.deviceId = deviceId;
    return result;
  }

  PairingCode._();

  factory PairingCode.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      PairingCode()..mergeFromBuffer(data, registry);
  factory PairingCode.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      PairingCode()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'PairingCode',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: PairingCode.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'code')
    ..aOS(2, _omitFieldNames ? '' : 'deviceId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PairingCode clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PairingCode copyWith(void Function(PairingCode) updates) =>
      super.copyWith((message) => updates(message as PairingCode))
          as PairingCode;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use PairingCode() / PairingCode.new instead')
  static PairingCode create() => PairingCode._();
  static $pb.GeneratedMessage $_createMessage() => PairingCode._();
  @$core.override
  PairingCode createEmptyInstance() => PairingCode._();
  @$core.pragma('dart2js:noInline')
  static PairingCode getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<PairingCode>(
          PairingCode.$_createMessage);
  static PairingCode? _defaultInstance;

  /// factory://?t=<token>&name=<this machine>&e=<endpoint id>[&r=<relay
  /// URL>…]: what the device pastes or scans to connect, through the tunnel.
  /// e= is the backend's iroh endpoint id (what iroh's EndpointId parses);
  /// each r= a relay it can be reached through (percent-encoded). Until it's
  /// first used it expires (PairedDevice.code_expires_ms); the first device
  /// to connect with it binds its key to it, and no other key can use it.
  /// That device gets in once the person allows it (PendingDevice).
  @$pb.TagNumber(1)
  $core.String get code => $_getSZ(0);
  @$pb.TagNumber(1)
  set code($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasCode() => $_has(0);
  @$pb.TagNumber(1)
  void clearCode() => $_clearField(1);

  /// The new device's id in RemoteAccess.devices.
  @$pb.TagNumber(2)
  $core.String get deviceId => $_getSZ(1);
  @$pb.TagNumber(2)
  set deviceId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasDeviceId() => $_has(1);
  @$pb.TagNumber(2)
  void clearDeviceId() => $_clearField(2);
}

/// Unpairs a device: it can't connect any more, and its open connections
/// close. Answered with Ack.
class UnpairDevice extends $pb.GeneratedMessage {
  factory UnpairDevice({
    $core.String? deviceId,
  }) {
    final result = UnpairDevice._();
    if (deviceId != null) result.deviceId = deviceId;
    return result;
  }

  UnpairDevice._();

  factory UnpairDevice.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UnpairDevice()..mergeFromBuffer(data, registry);
  factory UnpairDevice.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UnpairDevice()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'UnpairDevice',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: UnpairDevice.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'deviceId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UnpairDevice clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UnpairDevice copyWith(void Function(UnpairDevice) updates) =>
      super.copyWith((message) => updates(message as UnpairDevice))
          as UnpairDevice;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use UnpairDevice() / UnpairDevice.new instead')
  static UnpairDevice create() => UnpairDevice._();
  static $pb.GeneratedMessage $_createMessage() => UnpairDevice._();
  @$core.override
  UnpairDevice createEmptyInstance() => UnpairDevice._();
  @$core.pragma('dart2js:noInline')
  static UnpairDevice getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<UnpairDevice>(
          UnpairDevice.$_createMessage);
  static UnpairDevice? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get deviceId => $_getSZ(0);
  @$pb.TagNumber(1)
  set deviceId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasDeviceId() => $_has(0);
  @$pb.TagNumber(1)
  void clearDeviceId() => $_clearField(1);
}

/// Allows a device that's waiting (RemoteAccess.pending_devices): it's
/// paired, and its waiting connection gets Welcome. Answered with Ack;
/// ERROR_CODE_NOT_FOUND when no device with that id is waiting (its code
/// expired or was cancelled meanwhile).
class ApproveDevice extends $pb.GeneratedMessage {
  factory ApproveDevice({
    $core.String? deviceId,
  }) {
    final result = ApproveDevice._();
    if (deviceId != null) result.deviceId = deviceId;
    return result;
  }

  ApproveDevice._();

  factory ApproveDevice.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ApproveDevice()..mergeFromBuffer(data, registry);
  factory ApproveDevice.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ApproveDevice()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ApproveDevice',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ApproveDevice.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'deviceId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ApproveDevice clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ApproveDevice copyWith(void Function(ApproveDevice) updates) =>
      super.copyWith((message) => updates(message as ApproveDevice))
          as ApproveDevice;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ApproveDevice() / ApproveDevice.new instead')
  static ApproveDevice create() => ApproveDevice._();
  static $pb.GeneratedMessage $_createMessage() => ApproveDevice._();
  @$core.override
  ApproveDevice createEmptyInstance() => ApproveDevice._();
  @$core.pragma('dart2js:noInline')
  static ApproveDevice getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ApproveDevice>(
          ApproveDevice.$_createMessage);
  static ApproveDevice? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get deviceId => $_getSZ(0);
  @$pb.TagNumber(1)
  set deviceId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasDeviceId() => $_has(0);
  @$pb.TagNumber(1)
  void clearDeviceId() => $_clearField(1);
}

/// Turns away a device that's waiting: its code stops working, the device is
/// forgotten (make a new code to pair it), and its waiting connection gets
/// ERROR_CODE_UNAUTHORIZED. Answered with Ack; ERROR_CODE_NOT_FOUND when no
/// device with that id is waiting.
class DenyDevice extends $pb.GeneratedMessage {
  factory DenyDevice({
    $core.String? deviceId,
  }) {
    final result = DenyDevice._();
    if (deviceId != null) result.deviceId = deviceId;
    return result;
  }

  DenyDevice._();

  factory DenyDevice.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DenyDevice()..mergeFromBuffer(data, registry);
  factory DenyDevice.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DenyDevice()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'DenyDevice',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: DenyDevice.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'deviceId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DenyDevice clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DenyDevice copyWith(void Function(DenyDevice) updates) =>
      super.copyWith((message) => updates(message as DenyDevice)) as DenyDevice;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use DenyDevice() / DenyDevice.new instead')
  static DenyDevice create() => DenyDevice._();
  static $pb.GeneratedMessage $_createMessage() => DenyDevice._();
  @$core.override
  DenyDevice createEmptyInstance() => DenyDevice._();
  @$core.pragma('dart2js:noInline')
  static DenyDevice getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<DenyDevice>(DenyDevice.$_createMessage);
  static DenyDevice? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get deviceId => $_getSZ(0);
  @$pb.TagNumber(1)
  set deviceId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasDeviceId() => $_has(0);
  @$pb.TagNumber(1)
  void clearDeviceId() => $_clearField(1);
}

/// Cancels a pairing code that hasn't let a device in yet, when the person
/// closes Add a device: the code stops working and the device is forgotten
/// (a device waiting with it is turned away). A code exists only while the
/// person is pairing: this, or its 10 minutes running out, ends it. Answered
/// with Ack, also when the device is already gone; a device already allowed
/// stays paired.
class CancelPairing extends $pb.GeneratedMessage {
  factory CancelPairing({
    $core.String? deviceId,
  }) {
    final result = CancelPairing._();
    if (deviceId != null) result.deviceId = deviceId;
    return result;
  }

  CancelPairing._();

  factory CancelPairing.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CancelPairing()..mergeFromBuffer(data, registry);
  factory CancelPairing.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CancelPairing()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'CancelPairing',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: CancelPairing.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'deviceId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CancelPairing clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CancelPairing copyWith(void Function(CancelPairing) updates) =>
      super.copyWith((message) => updates(message as CancelPairing))
          as CancelPairing;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use CancelPairing() / CancelPairing.new instead')
  static CancelPairing create() => CancelPairing._();
  static $pb.GeneratedMessage $_createMessage() => CancelPairing._();
  @$core.override
  CancelPairing createEmptyInstance() => CancelPairing._();
  @$core.pragma('dart2js:noInline')
  static CancelPairing getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<CancelPairing>(
          CancelPairing.$_createMessage);
  static CancelPairing? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get deviceId => $_getSZ(0);
  @$pb.TagNumber(1)
  set deviceId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasDeviceId() => $_has(0);
  @$pb.TagNumber(1)
  void clearDeviceId() => $_clearField(1);
}

/// The answer to a device's Hello while it waits to be allowed. The
/// connection stays open: Welcome follows (replying to the Hello) once the
/// person allows it, or Error ERROR_CODE_UNAUTHORIZED, with why, once it's
/// turned away, cancelled or its code expires. Meanwhile the device shows
/// PendingDevice.number, worked out on its own side.
class PairingPending extends $pb.GeneratedMessage {
  factory PairingPending({
    $core.String? deviceId,
    $core.String? name,
    $core.String? machine,
    $fixnum.Int64? expiresMs,
  }) {
    final result = PairingPending._();
    if (deviceId != null) result.deviceId = deviceId;
    if (name != null) result.name = name;
    if (machine != null) result.machine = machine;
    if (expiresMs != null) result.expiresMs = expiresMs;
    return result;
  }

  PairingPending._();

  factory PairingPending.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      PairingPending()..mergeFromBuffer(data, registry);
  factory PairingPending.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      PairingPending()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'PairingPending',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: PairingPending.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'deviceId')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'machine')
    ..aInt64(4, _omitFieldNames ? '' : 'expiresMs')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PairingPending clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PairingPending copyWith(void Function(PairingPending) updates) =>
      super.copyWith((message) => updates(message as PairingPending))
          as PairingPending;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use PairingPending() / PairingPending.new instead')
  static PairingPending create() => PairingPending._();
  static $pb.GeneratedMessage $_createMessage() => PairingPending._();
  @$core.override
  PairingPending createEmptyInstance() => PairingPending._();
  @$core.pragma('dart2js:noInline')
  static PairingPending getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<PairingPending>(
          PairingPending.$_createMessage);
  static PairingPending? _defaultInstance;

  /// Its id in RemoteAccess.devices.
  @$pb.TagNumber(1)
  $core.String get deviceId => $_getSZ(0);
  @$pb.TagNumber(1)
  set deviceId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasDeviceId() => $_has(0);
  @$pb.TagNumber(1)
  void clearDeviceId() => $_clearField(1);

  /// The name it was given when its code was made.
  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// The name of the computer Claktory runs on, which asks the person
  /// (RemoteAccess.machine).
  @$pb.TagNumber(3)
  $core.String get machine => $_getSZ(2);
  @$pb.TagNumber(3)
  set machine($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasMachine() => $_has(2);
  @$pb.TagNumber(3)
  void clearMachine() => $_clearField(3);

  /// When its code stops working, in ms since 1970.
  @$pb.TagNumber(4)
  $fixnum.Int64 get expiresMs => $_getI64(3);
  @$pb.TagNumber(4)
  set expiresMs($fixnum.Int64 value) => $_setInt64(3, value);
  @$pb.TagNumber(4)
  $core.bool hasExpiresMs() => $_has(3);
  @$pb.TagNumber(4)
  void clearExpiresMs() => $_clearField(4);
}

/// Turns remote access (devices on other machines reaching this Factory
/// through the private tunnel) on or off, in config.json's `remote`. Answered
/// with Ack.
class SetRemoteAccess extends $pb.GeneratedMessage {
  factory SetRemoteAccess({
    $core.bool? on,
    $core.int? port,
    $core.String? listen,
    $core.Iterable<$core.String>? relays,
    $core.bool? setRelays,
  }) {
    final result = SetRemoteAccess._();
    if (on != null) result.on = on;
    if (port != null) result.port = port;
    if (listen != null) result.listen = listen;
    if (relays != null) result.relays.addAll(relays);
    if (setRelays != null) result.setRelays = setRelays;
    return result;
  }

  SetRemoteAccess._();

  factory SetRemoteAccess.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetRemoteAccess()..mergeFromBuffer(data, registry);
  factory SetRemoteAccess.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetRemoteAccess()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SetRemoteAccess',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SetRemoteAccess.$_createMessage)
    ..aOB(1, _omitFieldNames ? '' : 'on')
    ..aI(2, _omitFieldNames ? '' : 'port', fieldType: $pb.PbFieldType.OU3)
    ..aOS(3, _omitFieldNames ? '' : 'listen')
    ..pPS(4, _omitFieldNames ? '' : 'relays')
    ..aOB(5, _omitFieldNames ? '' : 'setRelays')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetRemoteAccess clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetRemoteAccess copyWith(void Function(SetRemoteAccess) updates) =>
      super.copyWith((message) => updates(message as SetRemoteAccess))
          as SetRemoteAccess;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SetRemoteAccess() / SetRemoteAccess.new instead')
  static SetRemoteAccess create() => SetRemoteAccess._();
  static $pb.GeneratedMessage $_createMessage() => SetRemoteAccess._();
  @$core.override
  SetRemoteAccess createEmptyInstance() => SetRemoteAccess._();
  @$core.pragma('dart2js:noInline')
  static SetRemoteAccess getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<SetRemoteAccess>(
          SetRemoteAccess.$_createMessage);
  static SetRemoteAccess? _defaultInstance;

  @$pb.TagNumber(1)
  $core.bool get on => $_getBF(0);
  @$pb.TagNumber(1)
  set on($core.bool value) => $_setBool(0, value);
  @$pb.TagNumber(1)
  $core.bool hasOn() => $_has(0);
  @$pb.TagNumber(1)
  void clearOn() => $_clearField(1);

  /// No longer used (there's no listening port); kept in config.json.
  @$pb.TagNumber(2)
  $core.int get port => $_getIZ(1);
  @$pb.TagNumber(2)
  set port($core.int value) => $_setUnsignedInt32(1, value);
  @$pb.TagNumber(2)
  $core.bool hasPort() => $_has(1);
  @$pb.TagNumber(2)
  void clearPort() => $_clearField(2);

  /// No longer used; kept in config.json.
  @$pb.TagNumber(3)
  $core.String get listen => $_getSZ(2);
  @$pb.TagNumber(3)
  set listen($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasListen() => $_has(2);
  @$pb.TagNumber(3)
  void clearListen() => $_clearField(3);

  /// The relays to use, as URLs (config.json `remote.relays`), when
  /// set_relays; empty for the default ones (RemoteAccess.relays).
  @$pb.TagNumber(4)
  $pb.PbList<$core.String> get relays => $_getList(3);

  @$pb.TagNumber(5)
  $core.bool get setRelays => $_getBF(4);
  @$pb.TagNumber(5)
  set setRelays($core.bool value) => $_setBool(4, value);
  @$pb.TagNumber(5)
  $core.bool hasSetRelays() => $_has(4);
  @$pb.TagNumber(5)
  void clearSetRelays() => $_clearField(5);
}

/// Remote access: devices on other machines connecting to this Factory,
/// through the private tunnel (iroh): end-to-end encrypted, by the backend's
/// key, directly or through a relay that can't read it. Factory listens on no
/// port for them.
class RemoteAccess extends $pb.GeneratedMessage {
  factory RemoteAccess({
    $core.bool? on,
    $core.int? port,
    $core.String? listen,
    $core.String? error,
    $core.String? fingerprint,
    $core.Iterable<$core.String>? addresses,
    $core.Iterable<PairedDevice>? devices,
    $core.String? machine,
    $core.String? endpointId,
    $core.Iterable<$core.String>? relays,
    $core.bool? customRelays,
    $core.String? homeRelay,
    $core.bool? relayConnected,
    $core.Iterable<PendingDevice>? pendingDevices,
  }) {
    final result = RemoteAccess._();
    if (on != null) result.on = on;
    if (port != null) result.port = port;
    if (listen != null) result.listen = listen;
    if (error != null) result.error = error;
    if (fingerprint != null) result.fingerprint = fingerprint;
    if (addresses != null) result.addresses.addAll(addresses);
    if (devices != null) result.devices.addAll(devices);
    if (machine != null) result.machine = machine;
    if (endpointId != null) result.endpointId = endpointId;
    if (relays != null) result.relays.addAll(relays);
    if (customRelays != null) result.customRelays = customRelays;
    if (homeRelay != null) result.homeRelay = homeRelay;
    if (relayConnected != null) result.relayConnected = relayConnected;
    if (pendingDevices != null) result.pendingDevices.addAll(pendingDevices);
    return result;
  }

  RemoteAccess._();

  factory RemoteAccess.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      RemoteAccess()..mergeFromBuffer(data, registry);
  factory RemoteAccess.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      RemoteAccess()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'RemoteAccess',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: RemoteAccess.$_createMessage)
    ..aOB(1, _omitFieldNames ? '' : 'on')
    ..aI(2, _omitFieldNames ? '' : 'port', fieldType: $pb.PbFieldType.OU3)
    ..aOS(3, _omitFieldNames ? '' : 'listen')
    ..aOS(4, _omitFieldNames ? '' : 'error')
    ..aOS(5, _omitFieldNames ? '' : 'fingerprint')
    ..pPS(6, _omitFieldNames ? '' : 'addresses')
    ..pPM<PairedDevice>(7, _omitFieldNames ? '' : 'devices',
        subBuilder: PairedDevice.$_createMessage)
    ..aOS(8, _omitFieldNames ? '' : 'machine')
    ..aOS(9, _omitFieldNames ? '' : 'endpointId')
    ..pPS(10, _omitFieldNames ? '' : 'relays')
    ..aOB(11, _omitFieldNames ? '' : 'customRelays')
    ..aOS(12, _omitFieldNames ? '' : 'homeRelay')
    ..aOB(13, _omitFieldNames ? '' : 'relayConnected')
    ..pPM<PendingDevice>(14, _omitFieldNames ? '' : 'pendingDevices',
        subBuilder: PendingDevice.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  RemoteAccess clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  RemoteAccess copyWith(void Function(RemoteAccess) updates) =>
      super.copyWith((message) => updates(message as RemoteAccess))
          as RemoteAccess;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use RemoteAccess() / RemoteAccess.new instead')
  static RemoteAccess create() => RemoteAccess._();
  static $pb.GeneratedMessage $_createMessage() => RemoteAccess._();
  @$core.override
  RemoteAccess createEmptyInstance() => RemoteAccess._();
  @$core.pragma('dart2js:noInline')
  static RemoteAccess getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<RemoteAccess>(
          RemoteAccess.$_createMessage);
  static RemoteAccess? _defaultInstance;

  /// config.json has `remote`: paired devices may connect. Off, only this
  /// machine's app can (the tunnel then binds to 127.0.0.1 only).
  @$pb.TagNumber(1)
  $core.bool get on => $_getBF(0);
  @$pb.TagNumber(1)
  set on($core.bool value) => $_setBool(0, value);
  @$pb.TagNumber(1)
  $core.bool hasOn() => $_has(0);
  @$pb.TagNumber(1)
  void clearOn() => $_clearField(1);

  /// No longer used (config.json's `remote.port` is kept, unused).
  @$pb.TagNumber(2)
  $core.int get port => $_getIZ(1);
  @$pb.TagNumber(2)
  set port($core.int value) => $_setUnsignedInt32(1, value);
  @$pb.TagNumber(2)
  $core.bool hasPort() => $_has(1);
  @$pb.TagNumber(2)
  void clearPort() => $_clearField(2);

  /// No longer used.
  @$pb.TagNumber(3)
  $core.String get listen => $_getSZ(2);
  @$pb.TagNumber(3)
  set listen($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasListen() => $_has(2);
  @$pb.TagNumber(3)
  void clearListen() => $_clearField(3);

  /// The tunnel can't run, and why; empty while all's well.
  @$pb.TagNumber(4)
  $core.String get error => $_getSZ(3);
  @$pb.TagNumber(4)
  set error($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasError() => $_has(3);
  @$pb.TagNumber(4)
  void clearError() => $_clearField(4);

  /// No longer used (there's no certificate to pin): empty.
  @$pb.TagNumber(5)
  $core.String get fingerprint => $_getSZ(4);
  @$pb.TagNumber(5)
  set fingerprint($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasFingerprint() => $_has(4);
  @$pb.TagNumber(5)
  void clearFingerprint() => $_clearField(5);

  /// No longer used: empty.
  @$pb.TagNumber(6)
  $pb.PbList<$core.String> get addresses => $_getList(5);

  @$pb.TagNumber(7)
  $pb.PbList<PairedDevice> get devices => $_getList(6);

  /// This machine's name, as pairing codes give it ("mac-mini").
  @$pb.TagNumber(8)
  $core.String get machine => $_getSZ(7);
  @$pb.TagNumber(8)
  set machine($core.String value) => $_setString(7, value);
  @$pb.TagNumber(8)
  $core.bool hasMachine() => $_has(7);
  @$pb.TagNumber(8)
  void clearMachine() => $_clearField(8);

  /// This backend's iroh endpoint id, as pairing codes give it (e=).
  @$pb.TagNumber(9)
  $core.String get endpointId => $_getSZ(8);
  @$pb.TagNumber(9)
  set endpointId($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasEndpointId() => $_has(8);
  @$pb.TagNumber(9)
  void clearEndpointId() => $_clearField(9);

  /// The relays it uses while on: config.json's `remote.relays`, or the
  /// default ones (n0's for now) when that's empty.
  @$pb.TagNumber(10)
  $pb.PbList<$core.String> get relays => $_getList(9);

  /// config.json sets its own relays (relays are those); false for the
  /// default ones.
  @$pb.TagNumber(11)
  $core.bool get customRelays => $_getBF(10);
  @$pb.TagNumber(11)
  set customRelays($core.bool value) => $_setBool(10, value);
  @$pb.TagNumber(11)
  $core.bool hasCustomRelays() => $_has(10);
  @$pb.TagNumber(11)
  void clearCustomRelays() => $_clearField(11);

  /// The relay it's connected to now (its home relay), and whether it is;
  /// empty and false while off or offline.
  @$pb.TagNumber(12)
  $core.String get homeRelay => $_getSZ(11);
  @$pb.TagNumber(12)
  set homeRelay($core.String value) => $_setString(11, value);
  @$pb.TagNumber(12)
  $core.bool hasHomeRelay() => $_has(11);
  @$pb.TagNumber(12)
  void clearHomeRelay() => $_clearField(12);

  @$pb.TagNumber(13)
  $core.bool get relayConnected => $_getBF(12);
  @$pb.TagNumber(13)
  set relayConnected($core.bool value) => $_setBool(12, value);
  @$pb.TagNumber(13)
  $core.bool hasRelayConnected() => $_has(12);
  @$pb.TagNumber(13)
  void clearRelayConnected() => $_clearField(13);

  /// Devices that used a pairing code and wait for the person to allow them,
  /// oldest first. Each is also in `devices`, with its code_expires_ms.
  @$pb.TagNumber(14)
  $pb.PbList<PendingDevice> get pendingDevices => $_getList(13);
}

/// A device that connected with a pairing code and waits to be allowed: a
/// code lets a device in only once the person allows it, on a device that's
/// already in (ApproveDevice), or turns it away (DenyDevice). Only the first
/// device to use a code asks; any other is turned away. The new device shows
/// the same number (see `number`), so the person can tell it's theirs and
/// not one that copied the code.
class PendingDevice extends $pb.GeneratedMessage {
  factory PendingDevice({
    $core.String? deviceId,
    $core.String? name,
    DeviceKind? kind,
    $core.String? number,
    $fixnum.Int64? requestedMs,
    $fixnum.Int64? expiresMs,
  }) {
    final result = PendingDevice._();
    if (deviceId != null) result.deviceId = deviceId;
    if (name != null) result.name = name;
    if (kind != null) result.kind = kind;
    if (number != null) result.number = number;
    if (requestedMs != null) result.requestedMs = requestedMs;
    if (expiresMs != null) result.expiresMs = expiresMs;
    return result;
  }

  PendingDevice._();

  factory PendingDevice.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      PendingDevice()..mergeFromBuffer(data, registry);
  factory PendingDevice.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      PendingDevice()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'PendingDevice',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: PendingDevice.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'deviceId')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aE<DeviceKind>(3, _omitFieldNames ? '' : 'kind',
        enumValues: DeviceKind.values)
    ..aOS(4, _omitFieldNames ? '' : 'number')
    ..aInt64(5, _omitFieldNames ? '' : 'requestedMs')
    ..aInt64(6, _omitFieldNames ? '' : 'expiresMs')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PendingDevice clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PendingDevice copyWith(void Function(PendingDevice) updates) =>
      super.copyWith((message) => updates(message as PendingDevice))
          as PendingDevice;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use PendingDevice() / PendingDevice.new instead')
  static PendingDevice create() => PendingDevice._();
  static $pb.GeneratedMessage $_createMessage() => PendingDevice._();
  @$core.override
  PendingDevice createEmptyInstance() => PendingDevice._();
  @$core.pragma('dart2js:noInline')
  static PendingDevice getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<PendingDevice>(
          PendingDevice.$_createMessage);
  static PendingDevice? _defaultInstance;

  /// Its id in RemoteAccess.devices (PairingCode.device_id).
  @$pb.TagNumber(1)
  $core.String get deviceId => $_getSZ(0);
  @$pb.TagNumber(1)
  set deviceId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasDeviceId() => $_has(0);
  @$pb.TagNumber(1)
  void clearDeviceId() => $_clearField(1);

  /// The name it was given when its code was made.
  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// What it says it is, from its Hello.
  @$pb.TagNumber(3)
  DeviceKind get kind => $_getN(2);
  @$pb.TagNumber(3)
  set kind(DeviceKind value) => $_setField(3, value);
  @$pb.TagNumber(3)
  $core.bool hasKind() => $_has(2);
  @$pb.TagNumber(3)
  void clearKind() => $_clearField(3);

  /// Four digits, e.g. "0427", that the new device shows too. Both sides
  /// work it out: SHA-256 of the UTF-8 text "claktory pairing\n" + this
  /// Claktory's endpoint id (the code's e=, lowercase hex) + "\n" + the
  /// device's own endpoint id (lowercase hex) + "\n" + the SHA-256 of the
  /// code's token (t=, the SHA-256 of its text) in lowercase hex; its first
  /// four bytes as a big-endian unsigned number, modulo 10000, with leading
  /// zeros. For example, with endpoint ids of 64 "a"s and 64 "b"s and the
  /// token "0123456789abcdef": "0722".
  @$pb.TagNumber(4)
  $core.String get number => $_getSZ(3);
  @$pb.TagNumber(4)
  set number($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasNumber() => $_has(3);
  @$pb.TagNumber(4)
  void clearNumber() => $_clearField(4);

  /// When it asked, in ms since 1970.
  @$pb.TagNumber(5)
  $fixnum.Int64 get requestedMs => $_getI64(4);
  @$pb.TagNumber(5)
  set requestedMs($fixnum.Int64 value) => $_setInt64(4, value);
  @$pb.TagNumber(5)
  $core.bool hasRequestedMs() => $_has(4);
  @$pb.TagNumber(5)
  void clearRequestedMs() => $_clearField(5);

  /// When its code stops working, and it's turned away, unless allowed.
  @$pb.TagNumber(6)
  $fixnum.Int64 get expiresMs => $_getI64(5);
  @$pb.TagNumber(6)
  set expiresMs($fixnum.Int64 value) => $_setInt64(5, value);
  @$pb.TagNumber(6)
  $core.bool hasExpiresMs() => $_has(5);
  @$pb.TagNumber(6)
  void clearExpiresMs() => $_clearField(6);
}

class PairedDevice extends $pb.GeneratedMessage {
  factory PairedDevice({
    $core.String? id,
    $core.String? name,
    $fixnum.Int64? pairedMs,
    $fixnum.Int64? lastSeenMs,
    $core.bool? connected,
    $core.String? lastAddress,
    DevicePath? path,
    $fixnum.Int64? codeExpiresMs,
  }) {
    final result = PairedDevice._();
    if (id != null) result.id = id;
    if (name != null) result.name = name;
    if (pairedMs != null) result.pairedMs = pairedMs;
    if (lastSeenMs != null) result.lastSeenMs = lastSeenMs;
    if (connected != null) result.connected = connected;
    if (lastAddress != null) result.lastAddress = lastAddress;
    if (path != null) result.path = path;
    if (codeExpiresMs != null) result.codeExpiresMs = codeExpiresMs;
    return result;
  }

  PairedDevice._();

  factory PairedDevice.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      PairedDevice()..mergeFromBuffer(data, registry);
  factory PairedDevice.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      PairedDevice()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'PairedDevice',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: PairedDevice.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aInt64(3, _omitFieldNames ? '' : 'pairedMs')
    ..aInt64(4, _omitFieldNames ? '' : 'lastSeenMs')
    ..aOB(5, _omitFieldNames ? '' : 'connected')
    ..aOS(6, _omitFieldNames ? '' : 'lastAddress')
    ..aE<DevicePath>(7, _omitFieldNames ? '' : 'path',
        enumValues: DevicePath.values)
    ..aInt64(8, _omitFieldNames ? '' : 'codeExpiresMs')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PairedDevice clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PairedDevice copyWith(void Function(PairedDevice) updates) =>
      super.copyWith((message) => updates(message as PairedDevice))
          as PairedDevice;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use PairedDevice() / PairedDevice.new instead')
  static PairedDevice create() => PairedDevice._();
  static $pb.GeneratedMessage $_createMessage() => PairedDevice._();
  @$core.override
  PairedDevice createEmptyInstance() => PairedDevice._();
  @$core.pragma('dart2js:noInline')
  static PairedDevice getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<PairedDevice>(
          PairedDevice.$_createMessage);
  static PairedDevice? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// When it was paired, in ms since 1970.
  @$pb.TagNumber(3)
  $fixnum.Int64 get pairedMs => $_getI64(2);
  @$pb.TagNumber(3)
  set pairedMs($fixnum.Int64 value) => $_setInt64(2, value);
  @$pb.TagNumber(3)
  $core.bool hasPairedMs() => $_has(2);
  @$pb.TagNumber(3)
  void clearPairedMs() => $_clearField(3);

  /// When it last connected; 0 if it never has.
  @$pb.TagNumber(4)
  $fixnum.Int64 get lastSeenMs => $_getI64(3);
  @$pb.TagNumber(4)
  set lastSeenMs($fixnum.Int64 value) => $_setInt64(3, value);
  @$pb.TagNumber(4)
  $core.bool hasLastSeenMs() => $_has(3);
  @$pb.TagNumber(4)
  void clearLastSeenMs() => $_clearField(4);

  /// It's connected now.
  @$pb.TagNumber(5)
  $core.bool get connected => $_getBF(4);
  @$pb.TagNumber(5)
  set connected($core.bool value) => $_setBool(4, value);
  @$pb.TagNumber(5)
  $core.bool hasConnected() => $_has(4);
  @$pb.TagNumber(5)
  void clearConnected() => $_clearField(5);

  /// The IP address it last connected from; empty if it never has, or when
  /// only a relay carried it.
  @$pb.TagNumber(6)
  $core.String get lastAddress => $_getSZ(5);
  @$pb.TagNumber(6)
  set lastAddress($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasLastAddress() => $_has(5);
  @$pb.TagNumber(6)
  void clearLastAddress() => $_clearField(6);

  /// How it's connected now; NONE when it isn't.
  @$pb.TagNumber(7)
  DevicePath get path => $_getN(6);
  @$pb.TagNumber(7)
  set path(DevicePath value) => $_setField(7, value);
  @$pb.TagNumber(7)
  $core.bool hasPath() => $_has(6);
  @$pb.TagNumber(7)
  void clearPath() => $_clearField(7);

  /// Its pairing code hasn't been used yet and stops working at this time
  /// (ms since 1970); 0 once a device has used it. Past it, the device can't
  /// connect: pair it again.
  @$pb.TagNumber(8)
  $fixnum.Int64 get codeExpiresMs => $_getI64(7);
  @$pb.TagNumber(8)
  set codeExpiresMs($fixnum.Int64 value) => $_setInt64(7, value);
  @$pb.TagNumber(8)
  $core.bool hasCodeExpiresMs() => $_has(7);
  @$pb.TagNumber(8)
  void clearCodeExpiresMs() => $_clearField(8);
}

/// Creates a project. Its folder is named after it, inside Factory's
/// workspace folder, and is created if it doesn't exist. Clients send only the
/// name; `path` is for callers that need a specific folder.
class CreateProject extends $pb.GeneratedMessage {
  factory CreateProject({
    $core.String? name,
    $core.String? path,
  }) {
    final result = CreateProject._();
    if (name != null) result.name = name;
    if (path != null) result.path = path;
    return result;
  }

  CreateProject._();

  factory CreateProject.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CreateProject()..mergeFromBuffer(data, registry);
  factory CreateProject.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CreateProject()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'CreateProject',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: CreateProject.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'name')
    ..aOS(2, _omitFieldNames ? '' : 'path')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CreateProject clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CreateProject copyWith(void Function(CreateProject) updates) =>
      super.copyWith((message) => updates(message as CreateProject))
          as CreateProject;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use CreateProject() / CreateProject.new instead')
  static CreateProject create() => CreateProject._();
  static $pb.GeneratedMessage $_createMessage() => CreateProject._();
  @$core.override
  CreateProject createEmptyInstance() => CreateProject._();
  @$core.pragma('dart2js:noInline')
  static CreateProject getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<CreateProject>(
          CreateProject.$_createMessage);
  static CreateProject? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get name => $_getSZ(0);
  @$pb.TagNumber(1)
  set name($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasName() => $_has(0);
  @$pb.TagNumber(1)
  void clearName() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get path => $_getSZ(1);
  @$pb.TagNumber(2)
  set path($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasPath() => $_has(1);
  @$pb.TagNumber(2)
  void clearPath() => $_clearField(2);
}

/// Creates an agent and selects it. An empty `project_id` puts it in the
/// workspace, outside any project. Empty `model` or `effort` means the Factory
/// default.
class CreateAgent extends $pb.GeneratedMessage {
  factory CreateAgent({
    $core.String? projectId,
    $core.String? name,
    $core.String? prompt,
    $core.String? model,
    $core.String? effort,
    $core.String? color,
    $core.String? folderId,
    $core.String? role,
  }) {
    final result = CreateAgent._();
    if (projectId != null) result.projectId = projectId;
    if (name != null) result.name = name;
    if (prompt != null) result.prompt = prompt;
    if (model != null) result.model = model;
    if (effort != null) result.effort = effort;
    if (color != null) result.color = color;
    if (folderId != null) result.folderId = folderId;
    if (role != null) result.role = role;
    return result;
  }

  CreateAgent._();

  factory CreateAgent.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CreateAgent()..mergeFromBuffer(data, registry);
  factory CreateAgent.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CreateAgent()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'CreateAgent',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: CreateAgent.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'projectId')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'prompt')
    ..aOS(4, _omitFieldNames ? '' : 'model')
    ..aOS(5, _omitFieldNames ? '' : 'effort')
    ..aOS(6, _omitFieldNames ? '' : 'color')
    ..aOS(7, _omitFieldNames ? '' : 'folderId')
    ..aOS(8, _omitFieldNames ? '' : 'role')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CreateAgent clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CreateAgent copyWith(void Function(CreateAgent) updates) =>
      super.copyWith((message) => updates(message as CreateAgent))
          as CreateAgent;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use CreateAgent() / CreateAgent.new instead')
  static CreateAgent create() => CreateAgent._();
  static $pb.GeneratedMessage $_createMessage() => CreateAgent._();
  @$core.override
  CreateAgent createEmptyInstance() => CreateAgent._();
  @$core.pragma('dart2js:noInline')
  static CreateAgent getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<CreateAgent>(
          CreateAgent.$_createMessage);
  static CreateAgent? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get projectId => $_getSZ(0);
  @$pb.TagNumber(1)
  set projectId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProjectId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProjectId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// The agent's prompt: instructions added to its system prompt.
  @$pb.TagNumber(3)
  $core.String get prompt => $_getSZ(2);
  @$pb.TagNumber(3)
  set prompt($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasPrompt() => $_has(2);
  @$pb.TagNumber(3)
  void clearPrompt() => $_clearField(3);

  @$pb.TagNumber(4)
  $core.String get model => $_getSZ(3);
  @$pb.TagNumber(4)
  set model($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasModel() => $_has(3);
  @$pb.TagNumber(4)
  void clearModel() => $_clearField(4);

  @$pb.TagNumber(5)
  $core.String get effort => $_getSZ(4);
  @$pb.TagNumber(5)
  set effort($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasEffort() => $_has(4);
  @$pb.TagNumber(5)
  void clearEffort() => $_clearField(5);

  /// One of the agent colors (see Agent.color); empty lets Factory pick.
  @$pb.TagNumber(6)
  $core.String get color => $_getSZ(5);
  @$pb.TagNumber(6)
  set color($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasColor() => $_has(5);
  @$pb.TagNumber(6)
  void clearColor() => $_clearField(6);

  /// One of its project's folders (Project.folders) to work in; empty for the
  /// project's own folder.
  @$pb.TagNumber(7)
  $core.String get folderId => $_getSZ(6);
  @$pb.TagNumber(7)
  set folderId($core.String value) => $_setString(6, value);
  @$pb.TagNumber(7)
  $core.bool hasFolderId() => $_has(6);
  @$pb.TagNumber(7)
  void clearFolderId() => $_clearField(7);

  /// What it does, shown with its name (see Agent.role); empty for none.
  @$pb.TagNumber(8)
  $core.String get role => $_getSZ(7);
  @$pb.TagNumber(8)
  set role($core.String value) => $_setString(7, value);
  @$pb.TagNumber(8)
  $core.bool hasRole() => $_has(7);
  @$pb.TagNumber(8)
  void clearRole() => $_clearField(8);
}

/// Changes an agent's settings. Unset fields stay as they are. Takes effect
/// from the agent's next step (its next model call), mid-run too; what's
/// display only (color, placeholder, avatar) at once.
class UpdateAgent extends $pb.GeneratedMessage {
  factory UpdateAgent({
    $core.String? agentId,
    $core.String? name,
    $core.String? prompt,
    $core.String? model,
    $core.String? effort,
    $core.String? projectId,
    $core.String? color,
    ExtensionChange? extension_8,
    $core.String? placeholder,
    $core.String? folder,
    $core.String? folderId,
    $core.String? avatarShape,
    $core.String? avatarFace,
    $core.String? role,
  }) {
    final result = UpdateAgent._();
    if (agentId != null) result.agentId = agentId;
    if (name != null) result.name = name;
    if (prompt != null) result.prompt = prompt;
    if (model != null) result.model = model;
    if (effort != null) result.effort = effort;
    if (projectId != null) result.projectId = projectId;
    if (color != null) result.color = color;
    if (extension_8 != null) result.extension_8 = extension_8;
    if (placeholder != null) result.placeholder = placeholder;
    if (folder != null) result.folder = folder;
    if (folderId != null) result.folderId = folderId;
    if (avatarShape != null) result.avatarShape = avatarShape;
    if (avatarFace != null) result.avatarFace = avatarFace;
    if (role != null) result.role = role;
    return result;
  }

  UpdateAgent._();

  factory UpdateAgent.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UpdateAgent()..mergeFromBuffer(data, registry);
  factory UpdateAgent.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UpdateAgent()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'UpdateAgent',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: UpdateAgent.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'prompt')
    ..aOS(4, _omitFieldNames ? '' : 'model')
    ..aOS(5, _omitFieldNames ? '' : 'effort')
    ..aOS(6, _omitFieldNames ? '' : 'projectId')
    ..aOS(7, _omitFieldNames ? '' : 'color')
    ..aOM<ExtensionChange>(8, _omitFieldNames ? '' : 'extension',
        subBuilder: ExtensionChange.$_createMessage)
    ..aOS(9, _omitFieldNames ? '' : 'placeholder')
    ..aOS(10, _omitFieldNames ? '' : 'folder')
    ..aOS(11, _omitFieldNames ? '' : 'folderId')
    ..aOS(12, _omitFieldNames ? '' : 'avatarShape')
    ..aOS(13, _omitFieldNames ? '' : 'avatarFace')
    ..aOS(14, _omitFieldNames ? '' : 'role')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UpdateAgent clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UpdateAgent copyWith(void Function(UpdateAgent) updates) =>
      super.copyWith((message) => updates(message as UpdateAgent))
          as UpdateAgent;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use UpdateAgent() / UpdateAgent.new instead')
  static UpdateAgent create() => UpdateAgent._();
  static $pb.GeneratedMessage $_createMessage() => UpdateAgent._();
  @$core.override
  UpdateAgent createEmptyInstance() => UpdateAgent._();
  @$core.pragma('dart2js:noInline')
  static UpdateAgent getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<UpdateAgent>(
          UpdateAgent.$_createMessage);
  static UpdateAgent? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.String get prompt => $_getSZ(2);
  @$pb.TagNumber(3)
  set prompt($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasPrompt() => $_has(2);
  @$pb.TagNumber(3)
  void clearPrompt() => $_clearField(3);

  @$pb.TagNumber(4)
  $core.String get model => $_getSZ(3);
  @$pb.TagNumber(4)
  set model($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasModel() => $_has(3);
  @$pb.TagNumber(4)
  void clearModel() => $_clearField(4);

  @$pb.TagNumber(5)
  $core.String get effort => $_getSZ(4);
  @$pb.TagNumber(5)
  set effort($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasEffort() => $_has(4);
  @$pb.TagNumber(5)
  void clearEffort() => $_clearField(5);

  /// Moves the agent to another project; empty moves it to the workspace. Its
  /// conversation goes with it, and it works in the new folder from its next
  /// step.
  @$pb.TagNumber(6)
  $core.String get projectId => $_getSZ(5);
  @$pb.TagNumber(6)
  set projectId($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasProjectId() => $_has(5);
  @$pb.TagNumber(6)
  void clearProjectId() => $_clearField(6);

  /// One of the agent colors (see Agent.color).
  @$pb.TagNumber(7)
  $core.String get color => $_getSZ(6);
  @$pb.TagNumber(7)
  set color($core.String value) => $_setString(6, value);
  @$pb.TagNumber(7)
  $core.bool hasColor() => $_has(6);
  @$pb.TagNumber(7)
  void clearColor() => $_clearField(7);

  /// Changes one of the agent's extensions.
  @$pb.TagNumber(8)
  ExtensionChange get extension_8 => $_getN(7);
  @$pb.TagNumber(8)
  set extension_8(ExtensionChange value) => $_setField(8, value);
  @$pb.TagNumber(8)
  $core.bool hasExtension_8() => $_has(7);
  @$pb.TagNumber(8)
  void clearExtension_8() => $_clearField(8);
  @$pb.TagNumber(8)
  ExtensionChange ensureExtension_8() => $_ensure(7);

  /// What its message box says when empty (see Agent.placeholder); empty
  /// goes back to "Message <name>".
  @$pb.TagNumber(9)
  $core.String get placeholder => $_getSZ(8);
  @$pb.TagNumber(9)
  set placeholder($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasPlaceholder() => $_has(8);
  @$pb.TagNumber(9)
  void clearPlaceholder() => $_clearField(9);

  /// Deprecated and ignored: agents work in their project's folders
  /// (folder_id) instead.
  @$pb.TagNumber(10)
  $core.String get folder => $_getSZ(9);
  @$pb.TagNumber(10)
  set folder($core.String value) => $_setString(9, value);
  @$pb.TagNumber(10)
  $core.bool hasFolder() => $_has(9);
  @$pb.TagNumber(10)
  void clearFolder() => $_clearField(10);

  /// The project folder it works in (see Agent.folder_id): one of its
  /// project's folders, or empty for the project's own folder. Moving the agent
  /// to another project without this puts it in that project's own folder.
  @$pb.TagNumber(11)
  $core.String get folderId => $_getSZ(10);
  @$pb.TagNumber(11)
  set folderId($core.String value) => $_setString(10, value);
  @$pb.TagNumber(11)
  $core.bool hasFolderId() => $_has(10);
  @$pb.TagNumber(11)
  void clearFolderId() => $_clearField(11);

  /// Its character's body shape (see Agent.avatar_shape); empty goes back to
  /// automatic. Display only, like color.
  @$pb.TagNumber(12)
  $core.String get avatarShape => $_getSZ(11);
  @$pb.TagNumber(12)
  set avatarShape($core.String value) => $_setString(11, value);
  @$pb.TagNumber(12)
  $core.bool hasAvatarShape() => $_has(11);
  @$pb.TagNumber(12)
  void clearAvatarShape() => $_clearField(12);

  /// Unused: characters no longer have faces, and this is ignored.
  @$pb.TagNumber(13)
  $core.String get avatarFace => $_getSZ(12);
  @$pb.TagNumber(13)
  set avatarFace($core.String value) => $_setString(12, value);
  @$pb.TagNumber(13)
  $core.bool hasAvatarFace() => $_has(12);
  @$pb.TagNumber(13)
  void clearAvatarFace() => $_clearField(13);

  /// What it does (see Agent.role); empty clears it.
  @$pb.TagNumber(14)
  $core.String get role => $_getSZ(13);
  @$pb.TagNumber(14)
  set role($core.String value) => $_setString(13, value);
  @$pb.TagNumber(14)
  $core.bool hasRole() => $_has(13);
  @$pb.TagNumber(14)
  void clearRole() => $_clearField(14);
}

/// Turns an extension on or off for an agent, and/or sets one of its options.
class ExtensionChange extends $pb.GeneratedMessage {
  factory ExtensionChange({
    $core.String? id,
    $core.bool? enabled,
    $core.String? optionId,
    OptionValue? value,
  }) {
    final result = ExtensionChange._();
    if (id != null) result.id = id;
    if (enabled != null) result.enabled = enabled;
    if (optionId != null) result.optionId = optionId;
    if (value != null) result.value = value;
    return result;
  }

  ExtensionChange._();

  factory ExtensionChange.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ExtensionChange()..mergeFromBuffer(data, registry);
  factory ExtensionChange.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ExtensionChange()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ExtensionChange',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ExtensionChange.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOB(2, _omitFieldNames ? '' : 'enabled')
    ..aOS(3, _omitFieldNames ? '' : 'optionId')
    ..aOM<OptionValue>(4, _omitFieldNames ? '' : 'value',
        subBuilder: OptionValue.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ExtensionChange clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ExtensionChange copyWith(void Function(ExtensionChange) updates) =>
      super.copyWith((message) => updates(message as ExtensionChange))
          as ExtensionChange;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ExtensionChange() / ExtensionChange.new instead')
  static ExtensionChange create() => ExtensionChange._();
  static $pb.GeneratedMessage $_createMessage() => ExtensionChange._();
  @$core.override
  ExtensionChange createEmptyInstance() => ExtensionChange._();
  @$core.pragma('dart2js:noInline')
  static ExtensionChange getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ExtensionChange>(
          ExtensionChange.$_createMessage);
  static ExtensionChange? _defaultInstance;

  /// The extension's id, e.g. "agents".
  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// Turns it on or off; unset leaves it as it is.
  @$pb.TagNumber(2)
  $core.bool get enabled => $_getBF(1);
  @$pb.TagNumber(2)
  set enabled($core.bool value) => $_setBool(1, value);
  @$pb.TagNumber(2)
  $core.bool hasEnabled() => $_has(1);
  @$pb.TagNumber(2)
  void clearEnabled() => $_clearField(2);

  /// The option to set to `value`; empty sets none.
  @$pb.TagNumber(3)
  $core.String get optionId => $_getSZ(2);
  @$pb.TagNumber(3)
  set optionId($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasOptionId() => $_has(2);
  @$pb.TagNumber(3)
  void clearOptionId() => $_clearField(3);

  @$pb.TagNumber(4)
  OptionValue get value => $_getN(3);
  @$pb.TagNumber(4)
  set value(OptionValue value) => $_setField(4, value);
  @$pb.TagNumber(4)
  $core.bool hasValue() => $_has(3);
  @$pb.TagNumber(4)
  void clearValue() => $_clearField(4);
  @$pb.TagNumber(4)
  OptionValue ensureValue() => $_ensure(3);
}

/// Selects the agent every client shows. The server answers with that agent's
/// `transcript`.
class SelectAgent extends $pb.GeneratedMessage {
  factory SelectAgent({
    $core.String? agentId,
  }) {
    final result = SelectAgent._();
    if (agentId != null) result.agentId = agentId;
    return result;
  }

  SelectAgent._();

  factory SelectAgent.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SelectAgent()..mergeFromBuffer(data, registry);
  factory SelectAgent.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SelectAgent()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SelectAgent',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SelectAgent.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SelectAgent clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SelectAgent copyWith(void Function(SelectAgent) updates) =>
      super.copyWith((message) => updates(message as SelectAgent))
          as SelectAgent;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SelectAgent() / SelectAgent.new instead')
  static SelectAgent create() => SelectAgent._();
  static $pb.GeneratedMessage $_createMessage() => SelectAgent._();
  @$core.override
  SelectAgent createEmptyInstance() => SelectAgent._();
  @$core.pragma('dart2js:noInline')
  static SelectAgent getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<SelectAgent>(
          SelectAgent.$_createMessage);
  static SelectAgent? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);
}

/// Gives an agent a message. If it's busy, the message waits for the current
/// run to finish.
class SendMessage extends $pb.GeneratedMessage {
  factory SendMessage({
    $core.String? agentId,
    $core.String? text,
    $core.String? clientId,
  }) {
    final result = SendMessage._();
    if (agentId != null) result.agentId = agentId;
    if (text != null) result.text = text;
    if (clientId != null) result.clientId = clientId;
    return result;
  }

  SendMessage._();

  factory SendMessage.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SendMessage()..mergeFromBuffer(data, registry);
  factory SendMessage.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SendMessage()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SendMessage',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SendMessage.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..aOS(2, _omitFieldNames ? '' : 'text')
    ..aOS(3, _omitFieldNames ? '' : 'clientId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SendMessage clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SendMessage copyWith(void Function(SendMessage) updates) =>
      super.copyWith((message) => updates(message as SendMessage))
          as SendMessage;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SendMessage() / SendMessage.new instead')
  static SendMessage create() => SendMessage._();
  static $pb.GeneratedMessage $_createMessage() => SendMessage._();
  @$core.override
  SendMessage createEmptyInstance() => SendMessage._();
  @$core.pragma('dart2js:noInline')
  static SendMessage getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<SendMessage>(
          SendMessage.$_createMessage);
  static SendMessage? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get text => $_getSZ(1);
  @$pb.TagNumber(2)
  set text($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasText() => $_has(1);
  @$pb.TagNumber(2)
  void clearText() => $_clearField(2);

  /// Made up by the app for this send, so it can show the message at once and
  /// match it to the queued message and transcript entry it becomes, which
  /// carry it (QueuedMessage.client_id, UserText.client_id). Optional.
  @$pb.TagNumber(3)
  $core.String get clientId => $_getSZ(2);
  @$pb.TagNumber(3)
  set clientId($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasClientId() => $_has(2);
  @$pb.TagNumber(3)
  void clearClientId() => $_clearField(3);
}

/// Stops the agent's current run.
class StopAgent extends $pb.GeneratedMessage {
  factory StopAgent({
    $core.String? agentId,
  }) {
    final result = StopAgent._();
    if (agentId != null) result.agentId = agentId;
    return result;
  }

  StopAgent._();

  factory StopAgent.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      StopAgent()..mergeFromBuffer(data, registry);
  factory StopAgent.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      StopAgent()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'StopAgent',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: StopAgent.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  StopAgent clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  StopAgent copyWith(void Function(StopAgent) updates) =>
      super.copyWith((message) => updates(message as StopAgent)) as StopAgent;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use StopAgent() / StopAgent.new instead')
  static StopAgent create() => StopAgent._();
  static $pb.GeneratedMessage $_createMessage() => StopAgent._();
  @$core.override
  StopAgent createEmptyInstance() => StopAgent._();
  @$core.pragma('dart2js:noInline')
  static StopAgent getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<StopAgent>(StopAgent.$_createMessage);
  static StopAgent? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);
}

/// Deletes an agent and its conversation. A running agent is stopped first.
class DeleteAgent extends $pb.GeneratedMessage {
  factory DeleteAgent({
    $core.String? agentId,
  }) {
    final result = DeleteAgent._();
    if (agentId != null) result.agentId = agentId;
    return result;
  }

  DeleteAgent._();

  factory DeleteAgent.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DeleteAgent()..mergeFromBuffer(data, registry);
  factory DeleteAgent.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DeleteAgent()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'DeleteAgent',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: DeleteAgent.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DeleteAgent clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DeleteAgent copyWith(void Function(DeleteAgent) updates) =>
      super.copyWith((message) => updates(message as DeleteAgent))
          as DeleteAgent;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use DeleteAgent() / DeleteAgent.new instead')
  static DeleteAgent create() => DeleteAgent._();
  static $pb.GeneratedMessage $_createMessage() => DeleteAgent._();
  @$core.override
  DeleteAgent createEmptyInstance() => DeleteAgent._();
  @$core.pragma('dart2js:noInline')
  static DeleteAgent getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<DeleteAgent>(
          DeleteAgent.$_createMessage);
  static DeleteAgent? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);
}

/// Removes a project and deletes its agents. The project's folder and its files
/// are left alone.
class DeleteProject extends $pb.GeneratedMessage {
  factory DeleteProject({
    $core.String? projectId,
  }) {
    final result = DeleteProject._();
    if (projectId != null) result.projectId = projectId;
    return result;
  }

  DeleteProject._();

  factory DeleteProject.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DeleteProject()..mergeFromBuffer(data, registry);
  factory DeleteProject.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DeleteProject()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'DeleteProject',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: DeleteProject.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'projectId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DeleteProject clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DeleteProject copyWith(void Function(DeleteProject) updates) =>
      super.copyWith((message) => updates(message as DeleteProject))
          as DeleteProject;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use DeleteProject() / DeleteProject.new instead')
  static DeleteProject create() => DeleteProject._();
  static $pb.GeneratedMessage $_createMessage() => DeleteProject._();
  @$core.override
  DeleteProject createEmptyInstance() => DeleteProject._();
  @$core.pragma('dart2js:noInline')
  static DeleteProject getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<DeleteProject>(
          DeleteProject.$_createMessage);
  static DeleteProject? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get projectId => $_getSZ(0);
  @$pb.TagNumber(1)
  set projectId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProjectId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProjectId() => $_clearField(1);
}

/// Adds a folder to a project (see Project.folders). An empty `path` puts it
/// in the project's folder, named after it. The folder is created if missing.
class CreateFolder extends $pb.GeneratedMessage {
  factory CreateFolder({
    $core.String? projectId,
    $core.String? name,
    $core.String? path,
  }) {
    final result = CreateFolder._();
    if (projectId != null) result.projectId = projectId;
    if (name != null) result.name = name;
    if (path != null) result.path = path;
    return result;
  }

  CreateFolder._();

  factory CreateFolder.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CreateFolder()..mergeFromBuffer(data, registry);
  factory CreateFolder.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CreateFolder()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'CreateFolder',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: CreateFolder.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'projectId')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'path')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CreateFolder clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CreateFolder copyWith(void Function(CreateFolder) updates) =>
      super.copyWith((message) => updates(message as CreateFolder))
          as CreateFolder;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use CreateFolder() / CreateFolder.new instead')
  static CreateFolder create() => CreateFolder._();
  static $pb.GeneratedMessage $_createMessage() => CreateFolder._();
  @$core.override
  CreateFolder createEmptyInstance() => CreateFolder._();
  @$core.pragma('dart2js:noInline')
  static CreateFolder getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<CreateFolder>(
          CreateFolder.$_createMessage);
  static CreateFolder? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get projectId => $_getSZ(0);
  @$pb.TagNumber(1)
  set projectId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProjectId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProjectId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// Absolute; anywhere on the machine (a repository elsewhere, say).
  @$pb.TagNumber(3)
  $core.String get path => $_getSZ(2);
  @$pb.TagNumber(3)
  set path($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasPath() => $_has(2);
  @$pb.TagNumber(3)
  void clearPath() => $_clearField(3);
}

/// Renames a project's folder, or points it at another folder on disk (files
/// aren't moved). Unset fields stay as they are.
class UpdateFolder extends $pb.GeneratedMessage {
  factory UpdateFolder({
    $core.String? projectId,
    $core.String? folderId,
    $core.String? name,
    $core.String? path,
  }) {
    final result = UpdateFolder._();
    if (projectId != null) result.projectId = projectId;
    if (folderId != null) result.folderId = folderId;
    if (name != null) result.name = name;
    if (path != null) result.path = path;
    return result;
  }

  UpdateFolder._();

  factory UpdateFolder.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UpdateFolder()..mergeFromBuffer(data, registry);
  factory UpdateFolder.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UpdateFolder()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'UpdateFolder',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: UpdateFolder.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'projectId')
    ..aOS(2, _omitFieldNames ? '' : 'folderId')
    ..aOS(3, _omitFieldNames ? '' : 'name')
    ..aOS(4, _omitFieldNames ? '' : 'path')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UpdateFolder clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UpdateFolder copyWith(void Function(UpdateFolder) updates) =>
      super.copyWith((message) => updates(message as UpdateFolder))
          as UpdateFolder;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use UpdateFolder() / UpdateFolder.new instead')
  static UpdateFolder create() => UpdateFolder._();
  static $pb.GeneratedMessage $_createMessage() => UpdateFolder._();
  @$core.override
  UpdateFolder createEmptyInstance() => UpdateFolder._();
  @$core.pragma('dart2js:noInline')
  static UpdateFolder getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<UpdateFolder>(
          UpdateFolder.$_createMessage);
  static UpdateFolder? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get projectId => $_getSZ(0);
  @$pb.TagNumber(1)
  set projectId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProjectId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProjectId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get folderId => $_getSZ(1);
  @$pb.TagNumber(2)
  set folderId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasFolderId() => $_has(1);
  @$pb.TagNumber(2)
  void clearFolderId() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.String get name => $_getSZ(2);
  @$pb.TagNumber(3)
  set name($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasName() => $_has(2);
  @$pb.TagNumber(3)
  void clearName() => $_clearField(3);

  @$pb.TagNumber(4)
  $core.String get path => $_getSZ(3);
  @$pb.TagNumber(4)
  set path($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasPath() => $_has(3);
  @$pb.TagNumber(4)
  void clearPath() => $_clearField(4);
}

/// Removes a folder from its project. Its agents go to the project's own
/// folder; no files are deleted.
class DeleteFolder extends $pb.GeneratedMessage {
  factory DeleteFolder({
    $core.String? projectId,
    $core.String? folderId,
  }) {
    final result = DeleteFolder._();
    if (projectId != null) result.projectId = projectId;
    if (folderId != null) result.folderId = folderId;
    return result;
  }

  DeleteFolder._();

  factory DeleteFolder.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DeleteFolder()..mergeFromBuffer(data, registry);
  factory DeleteFolder.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DeleteFolder()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'DeleteFolder',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: DeleteFolder.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'projectId')
    ..aOS(2, _omitFieldNames ? '' : 'folderId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DeleteFolder clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DeleteFolder copyWith(void Function(DeleteFolder) updates) =>
      super.copyWith((message) => updates(message as DeleteFolder))
          as DeleteFolder;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use DeleteFolder() / DeleteFolder.new instead')
  static DeleteFolder create() => DeleteFolder._();
  static $pb.GeneratedMessage $_createMessage() => DeleteFolder._();
  @$core.override
  DeleteFolder createEmptyInstance() => DeleteFolder._();
  @$core.pragma('dart2js:noInline')
  static DeleteFolder getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<DeleteFolder>(
          DeleteFolder.$_createMessage);
  static DeleteFolder? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get projectId => $_getSZ(0);
  @$pb.TagNumber(1)
  set projectId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProjectId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProjectId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get folderId => $_getSZ(1);
  @$pb.TagNumber(2)
  set folderId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasFolderId() => $_has(1);
  @$pb.TagNumber(2)
  void clearFolderId() => $_clearField(2);
}

/// Renames a project. Its folder keeps its name.
class UpdateProject extends $pb.GeneratedMessage {
  factory UpdateProject({
    $core.String? projectId,
    $core.String? name,
    $core.bool? archived,
  }) {
    final result = UpdateProject._();
    if (projectId != null) result.projectId = projectId;
    if (name != null) result.name = name;
    if (archived != null) result.archived = archived;
    return result;
  }

  UpdateProject._();

  factory UpdateProject.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UpdateProject()..mergeFromBuffer(data, registry);
  factory UpdateProject.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UpdateProject()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'UpdateProject',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: UpdateProject.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'projectId')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOB(3, _omitFieldNames ? '' : 'archived')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UpdateProject clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UpdateProject copyWith(void Function(UpdateProject) updates) =>
      super.copyWith((message) => updates(message as UpdateProject))
          as UpdateProject;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use UpdateProject() / UpdateProject.new instead')
  static UpdateProject create() => UpdateProject._();
  static $pb.GeneratedMessage $_createMessage() => UpdateProject._();
  @$core.override
  UpdateProject createEmptyInstance() => UpdateProject._();
  @$core.pragma('dart2js:noInline')
  static UpdateProject getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<UpdateProject>(
          UpdateProject.$_createMessage);
  static UpdateProject? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get projectId => $_getSZ(0);
  @$pb.TagNumber(1)
  set projectId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProjectId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProjectId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// Archives the project (true) or brings it back (false); see
  /// Project.archived.
  @$pb.TagNumber(3)
  $core.bool get archived => $_getBF(2);
  @$pb.TagNumber(3)
  set archived($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasArchived() => $_has(2);
  @$pb.TagNumber(3)
  void clearArchived() => $_clearField(3);
}

/// Changes Factory-wide settings. Unset fields stay as they are. An empty
/// `default_effort` picks one the default model accepts.
class UpdateSettings extends $pb.GeneratedMessage {
  factory UpdateSettings({
    $core.String? defaultModel,
    $core.String? defaultEffort,
    $fixnum.Int64? compactAtTokens,
    $core.int? ramPercent,
    $core.int? diskFreeGb,
    $core.int? diskFreePercent,
    $core.int? cpuPercent,
    $core.double? sidebarWidth,
    $core.double? panelWidth,
    $core.String? userName,
    $core.double? buildCacheGb,
    $core.int? vaultUnlockMinutes,
    $core.bool? idleCompaction,
    $core.bool? idleCompactionShortCache,
    CacheTtl? cacheTtl,
  }) {
    final result = UpdateSettings._();
    if (defaultModel != null) result.defaultModel = defaultModel;
    if (defaultEffort != null) result.defaultEffort = defaultEffort;
    if (compactAtTokens != null) result.compactAtTokens = compactAtTokens;
    if (ramPercent != null) result.ramPercent = ramPercent;
    if (diskFreeGb != null) result.diskFreeGb = diskFreeGb;
    if (diskFreePercent != null) result.diskFreePercent = diskFreePercent;
    if (cpuPercent != null) result.cpuPercent = cpuPercent;
    if (sidebarWidth != null) result.sidebarWidth = sidebarWidth;
    if (panelWidth != null) result.panelWidth = panelWidth;
    if (userName != null) result.userName = userName;
    if (buildCacheGb != null) result.buildCacheGb = buildCacheGb;
    if (vaultUnlockMinutes != null)
      result.vaultUnlockMinutes = vaultUnlockMinutes;
    if (idleCompaction != null) result.idleCompaction = idleCompaction;
    if (idleCompactionShortCache != null)
      result.idleCompactionShortCache = idleCompactionShortCache;
    if (cacheTtl != null) result.cacheTtl = cacheTtl;
    return result;
  }

  UpdateSettings._();

  factory UpdateSettings.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UpdateSettings()..mergeFromBuffer(data, registry);
  factory UpdateSettings.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UpdateSettings()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'UpdateSettings',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: UpdateSettings.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'defaultModel')
    ..aOS(2, _omitFieldNames ? '' : 'defaultEffort')
    ..a<$fixnum.Int64>(
        3, _omitFieldNames ? '' : 'compactAtTokens', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..aI(4, _omitFieldNames ? '' : 'ramPercent', fieldType: $pb.PbFieldType.OU3)
    ..aI(5, _omitFieldNames ? '' : 'diskFreeGb', fieldType: $pb.PbFieldType.OU3)
    ..aI(6, _omitFieldNames ? '' : 'diskFreePercent',
        fieldType: $pb.PbFieldType.OU3)
    ..aI(7, _omitFieldNames ? '' : 'cpuPercent', fieldType: $pb.PbFieldType.OU3)
    ..aD(8, _omitFieldNames ? '' : 'sidebarWidth')
    ..aD(9, _omitFieldNames ? '' : 'panelWidth')
    ..aOS(10, _omitFieldNames ? '' : 'userName')
    ..aD(11, _omitFieldNames ? '' : 'buildCacheGb')
    ..aI(20, _omitFieldNames ? '' : 'vaultUnlockMinutes',
        fieldType: $pb.PbFieldType.OU3)
    ..aOB(23, _omitFieldNames ? '' : 'idleCompaction')
    ..aOB(24, _omitFieldNames ? '' : 'idleCompactionShortCache')
    ..aOM<CacheTtl>(25, _omitFieldNames ? '' : 'cacheTtl',
        subBuilder: CacheTtl.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UpdateSettings clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UpdateSettings copyWith(void Function(UpdateSettings) updates) =>
      super.copyWith((message) => updates(message as UpdateSettings))
          as UpdateSettings;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use UpdateSettings() / UpdateSettings.new instead')
  static UpdateSettings create() => UpdateSettings._();
  static $pb.GeneratedMessage $_createMessage() => UpdateSettings._();
  @$core.override
  UpdateSettings createEmptyInstance() => UpdateSettings._();
  @$core.pragma('dart2js:noInline')
  static UpdateSettings getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<UpdateSettings>(
          UpdateSettings.$_createMessage);
  static UpdateSettings? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get defaultModel => $_getSZ(0);
  @$pb.TagNumber(1)
  set defaultModel($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasDefaultModel() => $_has(0);
  @$pb.TagNumber(1)
  void clearDefaultModel() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get defaultEffort => $_getSZ(1);
  @$pb.TagNumber(2)
  set defaultEffort($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasDefaultEffort() => $_has(1);
  @$pb.TagNumber(2)
  void clearDefaultEffort() => $_clearField(2);

  @$pb.TagNumber(3)
  $fixnum.Int64 get compactAtTokens => $_getI64(2);
  @$pb.TagNumber(3)
  set compactAtTokens($fixnum.Int64 value) => $_setInt64(2, value);
  @$pb.TagNumber(3)
  $core.bool hasCompactAtTokens() => $_has(2);
  @$pb.TagNumber(3)
  void clearCompactAtTokens() => $_clearField(3);

  /// The Resources alerts' thresholds (see Settings); cpu_percent 0 turns its
  /// alert off. Percentages run 1 to 100.
  @$pb.TagNumber(4)
  $core.int get ramPercent => $_getIZ(3);
  @$pb.TagNumber(4)
  set ramPercent($core.int value) => $_setUnsignedInt32(3, value);
  @$pb.TagNumber(4)
  $core.bool hasRamPercent() => $_has(3);
  @$pb.TagNumber(4)
  void clearRamPercent() => $_clearField(4);

  @$pb.TagNumber(5)
  $core.int get diskFreeGb => $_getIZ(4);
  @$pb.TagNumber(5)
  set diskFreeGb($core.int value) => $_setUnsignedInt32(4, value);
  @$pb.TagNumber(5)
  $core.bool hasDiskFreeGb() => $_has(4);
  @$pb.TagNumber(5)
  void clearDiskFreeGb() => $_clearField(5);

  @$pb.TagNumber(6)
  $core.int get diskFreePercent => $_getIZ(5);
  @$pb.TagNumber(6)
  set diskFreePercent($core.int value) => $_setUnsignedInt32(5, value);
  @$pb.TagNumber(6)
  $core.bool hasDiskFreePercent() => $_has(5);
  @$pb.TagNumber(6)
  void clearDiskFreePercent() => $_clearField(6);

  @$pb.TagNumber(7)
  $core.int get cpuPercent => $_getIZ(6);
  @$pb.TagNumber(7)
  set cpuPercent($core.int value) => $_setUnsignedInt32(6, value);
  @$pb.TagNumber(7)
  $core.bool hasCpuPercent() => $_has(6);
  @$pb.TagNumber(7)
  void clearCpuPercent() => $_clearField(7);

  /// The sidebar's and the right panel's widths (see Settings); 0 for the
  /// app's default. Negative or not finite is refused.
  @$pb.TagNumber(8)
  $core.double get sidebarWidth => $_getN(7);
  @$pb.TagNumber(8)
  set sidebarWidth($core.double value) => $_setDouble(7, value);
  @$pb.TagNumber(8)
  $core.bool hasSidebarWidth() => $_has(7);
  @$pb.TagNumber(8)
  void clearSidebarWidth() => $_clearField(8);

  @$pb.TagNumber(9)
  $core.double get panelWidth => $_getN(8);
  @$pb.TagNumber(9)
  set panelWidth($core.double value) => $_setDouble(8, value);
  @$pb.TagNumber(9)
  $core.bool hasPanelWidth() => $_has(8);
  @$pb.TagNumber(9)
  void clearPanelWidth() => $_clearField(9);

  /// The user's first name (see Settings); "" removes it.
  @$pb.TagNumber(10)
  $core.String get userName => $_getSZ(9);
  @$pb.TagNumber(10)
  set userName($core.String value) => $_setString(9, value);
  @$pb.TagNumber(10)
  $core.bool hasUserName() => $_has(9);
  @$pb.TagNumber(10)
  void clearUserName() => $_clearField(10);

  /// The build caches' budget (see Settings); 0 never clears them. Negative
  /// is refused.
  @$pb.TagNumber(11)
  $core.double get buildCacheGb => $_getN(10);
  @$pb.TagNumber(11)
  set buildCacheGb($core.double value) => $_setDouble(10, value);
  @$pb.TagNumber(11)
  $core.bool hasBuildCacheGb() => $_has(10);
  @$pb.TagNumber(11)
  void clearBuildCacheGb() => $_clearField(11);

  /// Unused: requests need no vault passphrase. Still kept in config.json.
  @$pb.TagNumber(20)
  $core.int get vaultUnlockMinutes => $_getIZ(11);
  @$pb.TagNumber(20)
  set vaultUnlockMinutes($core.int value) => $_setUnsignedInt32(11, value);
  @$pb.TagNumber(20)
  $core.bool hasVaultUnlockMinutes() => $_has(11);
  @$pb.TagNumber(20)
  void clearVaultUnlockMinutes() => $_clearField(20);

  /// See Settings.
  @$pb.TagNumber(23)
  $core.bool get idleCompaction => $_getBF(12);
  @$pb.TagNumber(23)
  set idleCompaction($core.bool value) => $_setBool(12, value);
  @$pb.TagNumber(23)
  $core.bool hasIdleCompaction() => $_has(12);
  @$pb.TagNumber(23)
  void clearIdleCompaction() => $_clearField(23);

  @$pb.TagNumber(24)
  $core.bool get idleCompactionShortCache => $_getBF(13);
  @$pb.TagNumber(24)
  set idleCompactionShortCache($core.bool value) => $_setBool(13, value);
  @$pb.TagNumber(24)
  $core.bool hasIdleCompactionShortCache() => $_has(13);
  @$pb.TagNumber(24)
  void clearIdleCompactionShortCache() => $_clearField(24);

  /// Sets one provider's or model's cache lifetime: ttl "5m" or "1h"; "" goes
  /// back to the default (its provider's, or the provider's own).
  @$pb.TagNumber(25)
  CacheTtl get cacheTtl => $_getN(14);
  @$pb.TagNumber(25)
  set cacheTtl(CacheTtl value) => $_setField(25, value);
  @$pb.TagNumber(25)
  $core.bool hasCacheTtl() => $_has(14);
  @$pb.TagNumber(25)
  void clearCacheTtl() => $_clearField(25);
  @$pb.TagNumber(25)
  CacheTtl ensureCacheTtl() => $_ensure(14);
}

/// Replaces a settings file that has an error (`Workspace.config_error`) with
/// the settings Factory is still using. The broken file is kept next to it as
/// config.broken.json.
class RestoreSettings extends $pb.GeneratedMessage {
  factory RestoreSettings() => RestoreSettings._();

  RestoreSettings._();

  factory RestoreSettings.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      RestoreSettings()..mergeFromBuffer(data, registry);
  factory RestoreSettings.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      RestoreSettings()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'RestoreSettings',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: RestoreSettings.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  RestoreSettings clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  RestoreSettings copyWith(void Function(RestoreSettings) updates) =>
      super.copyWith((message) => updates(message as RestoreSettings))
          as RestoreSettings;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use RestoreSettings() / RestoreSettings.new instead')
  static RestoreSettings create() => RestoreSettings._();
  static $pb.GeneratedMessage $_createMessage() => RestoreSettings._();
  @$core.override
  RestoreSettings createEmptyInstance() => RestoreSettings._();
  @$core.pragma('dart2js:noInline')
  static RestoreSettings getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<RestoreSettings>(
          RestoreSettings.$_createMessage);
  static RestoreSettings? _defaultInstance;
}

/// Installs `Workspace.latest_release`: Factory swaps in its new backend and
/// restarts (the connection drops and comes back). With `app_path`, the app
/// bundle at that path on Factory's computer is replaced too and reopened once
/// the app quits, so the app quits after the ack. From a device on another
/// machine `app_path` is ignored: its app updates itself (see FetchApp).
class InstallUpdate extends $pb.GeneratedMessage {
  factory InstallUpdate({
    $core.String? appPath,
  }) {
    final result = InstallUpdate._();
    if (appPath != null) result.appPath = appPath;
    return result;
  }

  InstallUpdate._();

  factory InstallUpdate.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      InstallUpdate()..mergeFromBuffer(data, registry);
  factory InstallUpdate.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      InstallUpdate()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'InstallUpdate',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: InstallUpdate.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'appPath')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  InstallUpdate clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  InstallUpdate copyWith(void Function(InstallUpdate) updates) =>
      super.copyWith((message) => updates(message as InstallUpdate))
          as InstallUpdate;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use InstallUpdate() / InstallUpdate.new instead')
  static InstallUpdate create() => InstallUpdate._();
  static $pb.GeneratedMessage $_createMessage() => InstallUpdate._();
  @$core.override
  InstallUpdate createEmptyInstance() => InstallUpdate._();
  @$core.pragma('dart2js:noInline')
  static InstallUpdate getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<InstallUpdate>(
          InstallUpdate.$_createMessage);
  static InstallUpdate? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get appPath => $_getSZ(0);
  @$pb.TagNumber(1)
  set appPath($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAppPath() => $_has(0);
  @$pb.TagNumber(1)
  void clearAppPath() => $_clearField(1);
}

/// Asks for a chunk of the app of release `version` (Workspace.latest_release),
/// for an app on another machine to update itself: it fetches the archive
/// chunk by chunk, checks its size and SHA-256, unpacks it, sends
/// InstallUpdate (with no app_path) and swaps itself. Answered with AppChunk,
/// or an Error (NOT_FOUND) saying why this Factory can't send that app (no
/// such release, no app in it, packing or downloading it failed).
class FetchApp extends $pb.GeneratedMessage {
  factory FetchApp({
    $core.String? version,
    $fixnum.Int64? offset,
  }) {
    final result = FetchApp._();
    if (version != null) result.version = version;
    if (offset != null) result.offset = offset;
    return result;
  }

  FetchApp._();

  factory FetchApp.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      FetchApp()..mergeFromBuffer(data, registry);
  factory FetchApp.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      FetchApp()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'FetchApp',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: FetchApp.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'version')
    ..a<$fixnum.Int64>(2, _omitFieldNames ? '' : 'offset', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  FetchApp clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  FetchApp copyWith(void Function(FetchApp) updates) =>
      super.copyWith((message) => updates(message as FetchApp)) as FetchApp;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use FetchApp() / FetchApp.new instead')
  static FetchApp create() => FetchApp._();
  static $pb.GeneratedMessage $_createMessage() => FetchApp._();
  @$core.override
  FetchApp createEmptyInstance() => FetchApp._();
  @$core.pragma('dart2js:noInline')
  static FetchApp getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<FetchApp>(FetchApp.$_createMessage);
  static FetchApp? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get version => $_getSZ(0);
  @$pb.TagNumber(1)
  set version($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasVersion() => $_has(0);
  @$pb.TagNumber(1)
  void clearVersion() => $_clearField(1);

  /// Where the chunk starts, in bytes: 0 first, then the end of the last.
  @$pb.TagNumber(2)
  $fixnum.Int64 get offset => $_getI64(1);
  @$pb.TagNumber(2)
  set offset($fixnum.Int64 value) => $_setInt64(1, value);
  @$pb.TagNumber(2)
  $core.bool hasOffset() => $_has(1);
  @$pb.TagNumber(2)
  void clearOffset() => $_clearField(2);
}

/// A chunk of a release's app archive: a gzipped tar with the app bundle at
/// its top level. The archive is made the first time any app asks for it;
/// until it's ready, the answer is only `version` and `preparing`.
class AppChunk extends $pb.GeneratedMessage {
  factory AppChunk({
    $core.String? version,
    $fixnum.Int64? offset,
    $core.List<$core.int>? data,
    $fixnum.Int64? size,
    $core.String? sha256,
    $core.String? appName,
    $core.bool? preparing,
  }) {
    final result = AppChunk._();
    if (version != null) result.version = version;
    if (offset != null) result.offset = offset;
    if (data != null) result.data = data;
    if (size != null) result.size = size;
    if (sha256 != null) result.sha256 = sha256;
    if (appName != null) result.appName = appName;
    if (preparing != null) result.preparing = preparing;
    return result;
  }

  AppChunk._();

  factory AppChunk.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AppChunk()..mergeFromBuffer(data, registry);
  factory AppChunk.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AppChunk()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'AppChunk',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: AppChunk.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'version')
    ..a<$fixnum.Int64>(2, _omitFieldNames ? '' : 'offset', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..a<$core.List<$core.int>>(
        3, _omitFieldNames ? '' : 'data', $pb.PbFieldType.OY)
    ..a<$fixnum.Int64>(4, _omitFieldNames ? '' : 'size', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..aOS(5, _omitFieldNames ? '' : 'sha256')
    ..aOS(6, _omitFieldNames ? '' : 'appName')
    ..aOB(7, _omitFieldNames ? '' : 'preparing')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AppChunk clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AppChunk copyWith(void Function(AppChunk) updates) =>
      super.copyWith((message) => updates(message as AppChunk)) as AppChunk;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use AppChunk() / AppChunk.new instead')
  static AppChunk create() => AppChunk._();
  static $pb.GeneratedMessage $_createMessage() => AppChunk._();
  @$core.override
  AppChunk createEmptyInstance() => AppChunk._();
  @$core.pragma('dart2js:noInline')
  static AppChunk getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<AppChunk>(AppChunk.$_createMessage);
  static AppChunk? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get version => $_getSZ(0);
  @$pb.TagNumber(1)
  set version($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasVersion() => $_has(0);
  @$pb.TagNumber(1)
  void clearVersion() => $_clearField(1);

  @$pb.TagNumber(2)
  $fixnum.Int64 get offset => $_getI64(1);
  @$pb.TagNumber(2)
  set offset($fixnum.Int64 value) => $_setInt64(1, value);
  @$pb.TagNumber(2)
  $core.bool hasOffset() => $_has(1);
  @$pb.TagNumber(2)
  void clearOffset() => $_clearField(2);

  /// min(1 MiB, size - offset) bytes of the archive from `offset`.
  @$pb.TagNumber(3)
  $core.List<$core.int> get data => $_getN(2);
  @$pb.TagNumber(3)
  set data($core.List<$core.int> value) => $_setBytes(2, value);
  @$pb.TagNumber(3)
  $core.bool hasData() => $_has(2);
  @$pb.TagNumber(3)
  void clearData() => $_clearField(3);

  /// The whole archive's size in bytes, and its SHA-256 (hex).
  @$pb.TagNumber(4)
  $fixnum.Int64 get size => $_getI64(3);
  @$pb.TagNumber(4)
  set size($fixnum.Int64 value) => $_setInt64(3, value);
  @$pb.TagNumber(4)
  $core.bool hasSize() => $_has(3);
  @$pb.TagNumber(4)
  void clearSize() => $_clearField(4);

  @$pb.TagNumber(5)
  $core.String get sha256 => $_getSZ(4);
  @$pb.TagNumber(5)
  set sha256($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasSha256() => $_has(4);
  @$pb.TagNumber(5)
  void clearSha256() => $_clearField(5);

  /// The bundle at the archive's top level: "Factory.app", or "Factory
  /// Dev.app" for the dev Factory.
  @$pb.TagNumber(6)
  $core.String get appName => $_getSZ(5);
  @$pb.TagNumber(6)
  set appName($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasAppName() => $_has(5);
  @$pb.TagNumber(6)
  void clearAppName() => $_clearField(6);

  /// The archive is still being made (on a Linux machine, downloaded): ask
  /// again in a second or two.
  @$pb.TagNumber(7)
  $core.bool get preparing => $_getBF(6);
  @$pb.TagNumber(7)
  set preparing($core.bool value) => $_setBool(6, value);
  @$pb.TagNumber(7)
  $core.bool hasPreparing() => $_has(6);
  @$pb.TagNumber(7)
  void clearPreparing() => $_clearField(7);
}

/// Lists the folders in a folder of the machine Factory runs on, so an app on
/// another machine can pick a project's folder there by browsing. Answered
/// with FolderList, or an Error (INVALID_REQUEST) saying why it can't be
/// listed: not a full path, doesn't exist, isn't a folder, permission denied.
class ListFolders extends $pb.GeneratedMessage {
  factory ListFolders({
    $core.String? path,
  }) {
    final result = ListFolders._();
    if (path != null) result.path = path;
    return result;
  }

  ListFolders._();

  factory ListFolders.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ListFolders()..mergeFromBuffer(data, registry);
  factory ListFolders.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ListFolders()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ListFolders',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ListFolders.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'path')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ListFolders clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ListFolders copyWith(void Function(ListFolders) updates) =>
      super.copyWith((message) => updates(message as ListFolders))
          as ListFolders;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ListFolders() / ListFolders.new instead')
  static ListFolders create() => ListFolders._();
  static $pb.GeneratedMessage $_createMessage() => ListFolders._();
  @$core.override
  ListFolders createEmptyInstance() => ListFolders._();
  @$core.pragma('dart2js:noInline')
  static ListFolders getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ListFolders>(
          ListFolders.$_createMessage);
  static ListFolders? _defaultInstance;

  /// An absolute path, or one starting at the home folder: "" or "~" for the
  /// home folder itself, "~/code" below it. ".." and links are resolved.
  @$pb.TagNumber(1)
  $core.String get path => $_getSZ(0);
  @$pb.TagNumber(1)
  set path($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasPath() => $_has(0);
  @$pb.TagNumber(1)
  void clearPath() => $_clearField(1);
}

class FolderList extends $pb.GeneratedMessage {
  factory FolderList({
    $core.String? path,
    $core.String? parent,
    $core.Iterable<FolderEntry>? folders,
    $core.String? home,
    $core.bool? truncated,
  }) {
    final result = FolderList._();
    if (path != null) result.path = path;
    if (parent != null) result.parent = parent;
    if (folders != null) result.folders.addAll(folders);
    if (home != null) result.home = home;
    if (truncated != null) result.truncated = truncated;
    return result;
  }

  FolderList._();

  factory FolderList.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      FolderList()..mergeFromBuffer(data, registry);
  factory FolderList.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      FolderList()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'FolderList',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: FolderList.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'path')
    ..aOS(2, _omitFieldNames ? '' : 'parent')
    ..pPM<FolderEntry>(3, _omitFieldNames ? '' : 'folders',
        subBuilder: FolderEntry.$_createMessage)
    ..aOS(4, _omitFieldNames ? '' : 'home')
    ..aOB(5, _omitFieldNames ? '' : 'truncated')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  FolderList clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  FolderList copyWith(void Function(FolderList) updates) =>
      super.copyWith((message) => updates(message as FolderList)) as FolderList;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use FolderList() / FolderList.new instead')
  static FolderList create() => FolderList._();
  static $pb.GeneratedMessage $_createMessage() => FolderList._();
  @$core.override
  FolderList createEmptyInstance() => FolderList._();
  @$core.pragma('dart2js:noInline')
  static FolderList getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<FolderList>(FolderList.$_createMessage);
  static FolderList? _defaultInstance;

  /// The folder listed, as a full path with links and ".." resolved.
  @$pb.TagNumber(1)
  $core.String get path => $_getSZ(0);
  @$pb.TagNumber(1)
  set path($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasPath() => $_has(0);
  @$pb.TagNumber(1)
  void clearPath() => $_clearField(1);

  /// Its parent folder; empty at "/".
  @$pb.TagNumber(2)
  $core.String get parent => $_getSZ(1);
  @$pb.TagNumber(2)
  set parent($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasParent() => $_has(1);
  @$pb.TagNumber(2)
  void clearParent() => $_clearField(2);

  /// The folders in it (links to folders too; files are left out), sorted by
  /// name, case aside.
  @$pb.TagNumber(3)
  $pb.PbList<FolderEntry> get folders => $_getList(2);

  /// The home folder of the user Factory runs as, for a Home button.
  @$pb.TagNumber(4)
  $core.String get home => $_getSZ(3);
  @$pb.TagNumber(4)
  set home($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasHome() => $_has(3);
  @$pb.TagNumber(4)
  void clearHome() => $_clearField(4);

  /// There were more than 5,000 folders; only the first 5,000 are listed.
  @$pb.TagNumber(5)
  $core.bool get truncated => $_getBF(4);
  @$pb.TagNumber(5)
  set truncated($core.bool value) => $_setBool(4, value);
  @$pb.TagNumber(5)
  $core.bool hasTruncated() => $_has(4);
  @$pb.TagNumber(5)
  void clearTruncated() => $_clearField(5);
}

class FolderEntry extends $pb.GeneratedMessage {
  factory FolderEntry({
    $core.String? name,
    $core.bool? hidden,
  }) {
    final result = FolderEntry._();
    if (name != null) result.name = name;
    if (hidden != null) result.hidden = hidden;
    return result;
  }

  FolderEntry._();

  factory FolderEntry.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      FolderEntry()..mergeFromBuffer(data, registry);
  factory FolderEntry.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      FolderEntry()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'FolderEntry',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: FolderEntry.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'name')
    ..aOB(2, _omitFieldNames ? '' : 'hidden')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  FolderEntry clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  FolderEntry copyWith(void Function(FolderEntry) updates) =>
      super.copyWith((message) => updates(message as FolderEntry))
          as FolderEntry;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use FolderEntry() / FolderEntry.new instead')
  static FolderEntry create() => FolderEntry._();
  static $pb.GeneratedMessage $_createMessage() => FolderEntry._();
  @$core.override
  FolderEntry createEmptyInstance() => FolderEntry._();
  @$core.pragma('dart2js:noInline')
  static FolderEntry getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<FolderEntry>(
          FolderEntry.$_createMessage);
  static FolderEntry? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get name => $_getSZ(0);
  @$pb.TagNumber(1)
  set name($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasName() => $_has(0);
  @$pb.TagNumber(1)
  void clearName() => $_clearField(1);

  /// Its name starts with ".": a hidden folder, which pickers usually hide.
  @$pb.TagNumber(2)
  $core.bool get hidden => $_getBF(1);
  @$pb.TagNumber(2)
  set hidden($core.bool value) => $_setBool(1, value);
  @$pb.TagNumber(2)
  $core.bool hasHidden() => $_has(1);
  @$pb.TagNumber(2)
  void clearHidden() => $_clearField(2);
}

/// Focuses a project: clients show only it, with its folder in the explorer.
/// If the selected agent isn't in it, its first agent is selected (or none).
/// An empty `project_id` leaves focus.
class FocusProject extends $pb.GeneratedMessage {
  factory FocusProject({
    $core.String? projectId,
  }) {
    final result = FocusProject._();
    if (projectId != null) result.projectId = projectId;
    return result;
  }

  FocusProject._();

  factory FocusProject.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      FocusProject()..mergeFromBuffer(data, registry);
  factory FocusProject.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      FocusProject()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'FocusProject',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: FocusProject.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'projectId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  FocusProject clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  FocusProject copyWith(void Function(FocusProject) updates) =>
      super.copyWith((message) => updates(message as FocusProject))
          as FocusProject;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use FocusProject() / FocusProject.new instead')
  static FocusProject create() => FocusProject._();
  static $pb.GeneratedMessage $_createMessage() => FocusProject._();
  @$core.override
  FocusProject createEmptyInstance() => FocusProject._();
  @$core.pragma('dart2js:noInline')
  static FocusProject getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<FocusProject>(
          FocusProject.$_createMessage);
  static FocusProject? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get projectId => $_getSZ(0);
  @$pb.TagNumber(1)
  set projectId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProjectId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProjectId() => $_clearField(1);
}

/// Opens or closes a folder in a project's explorer. `path` is relative to the
/// root it's in, with "/" between names.
class ExpandFolder extends $pb.GeneratedMessage {
  factory ExpandFolder({
    $core.String? projectId,
    $core.String? path,
    $core.bool? expanded,
    $core.String? folderId,
    $core.String? agentId,
  }) {
    final result = ExpandFolder._();
    if (projectId != null) result.projectId = projectId;
    if (path != null) result.path = path;
    if (expanded != null) result.expanded = expanded;
    if (folderId != null) result.folderId = folderId;
    if (agentId != null) result.agentId = agentId;
    return result;
  }

  ExpandFolder._();

  factory ExpandFolder.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ExpandFolder()..mergeFromBuffer(data, registry);
  factory ExpandFolder.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ExpandFolder()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ExpandFolder',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ExpandFolder.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'projectId')
    ..aOS(2, _omitFieldNames ? '' : 'path')
    ..aOB(3, _omitFieldNames ? '' : 'expanded')
    ..aOS(4, _omitFieldNames ? '' : 'folderId')
    ..aOS(5, _omitFieldNames ? '' : 'agentId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ExpandFolder clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ExpandFolder copyWith(void Function(ExpandFolder) updates) =>
      super.copyWith((message) => updates(message as ExpandFolder))
          as ExpandFolder;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ExpandFolder() / ExpandFolder.new instead')
  static ExpandFolder create() => ExpandFolder._();
  static $pb.GeneratedMessage $_createMessage() => ExpandFolder._();
  @$core.override
  ExpandFolder createEmptyInstance() => ExpandFolder._();
  @$core.pragma('dart2js:noInline')
  static ExpandFolder getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ExpandFolder>(
          ExpandFolder.$_createMessage);
  static ExpandFolder? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get projectId => $_getSZ(0);
  @$pb.TagNumber(1)
  set projectId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProjectId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProjectId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get path => $_getSZ(1);
  @$pb.TagNumber(2)
  set path($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasPath() => $_has(1);
  @$pb.TagNumber(2)
  void clearPath() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.bool get expanded => $_getBF(2);
  @$pb.TagNumber(3)
  set expanded($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasExpanded() => $_has(2);
  @$pb.TagNumber(3)
  void clearExpanded() => $_clearField(3);

  /// The root: empty for the project's own folder, else one of its folders
  /// (Project.folders, Explorer.folders).
  @$pb.TagNumber(4)
  $core.String get folderId => $_getSZ(3);
  @$pb.TagNumber(4)
  set folderId($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasFolderId() => $_has(3);
  @$pb.TagNumber(4)
  void clearFolderId() => $_clearField(4);

  /// Or an agent's own checkout (ExplorerRoot.agent_id); folder_id is empty then.
  @$pb.TagNumber(5)
  $core.String get agentId => $_getSZ(4);
  @$pb.TagNumber(5)
  set agentId($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasAgentId() => $_has(4);
  @$pb.TagNumber(5)
  void clearAgentId() => $_clearField(5);
}

/// Shows the files the project's .factoryignore hides, or hides them again.
class ShowAllFiles extends $pb.GeneratedMessage {
  factory ShowAllFiles({
    $core.String? projectId,
    $core.bool? showAll,
  }) {
    final result = ShowAllFiles._();
    if (projectId != null) result.projectId = projectId;
    if (showAll != null) result.showAll = showAll;
    return result;
  }

  ShowAllFiles._();

  factory ShowAllFiles.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ShowAllFiles()..mergeFromBuffer(data, registry);
  factory ShowAllFiles.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ShowAllFiles()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ShowAllFiles',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ShowAllFiles.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'projectId')
    ..aOB(2, _omitFieldNames ? '' : 'showAll')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ShowAllFiles clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ShowAllFiles copyWith(void Function(ShowAllFiles) updates) =>
      super.copyWith((message) => updates(message as ShowAllFiles))
          as ShowAllFiles;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ShowAllFiles() / ShowAllFiles.new instead')
  static ShowAllFiles create() => ShowAllFiles._();
  static $pb.GeneratedMessage $_createMessage() => ShowAllFiles._();
  @$core.override
  ShowAllFiles createEmptyInstance() => ShowAllFiles._();
  @$core.pragma('dart2js:noInline')
  static ShowAllFiles getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ShowAllFiles>(
          ShowAllFiles.$_createMessage);
  static ShowAllFiles? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get projectId => $_getSZ(0);
  @$pb.TagNumber(1)
  set projectId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProjectId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProjectId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.bool get showAll => $_getBF(1);
  @$pb.TagNumber(2)
  set showAll($core.bool value) => $_setBool(1, value);
  @$pb.TagNumber(2)
  $core.bool hasShowAll() => $_has(1);
  @$pb.TagNumber(2)
  void clearShowAll() => $_clearField(2);
}

/// Moves an agent in the sidebar: to a place among its siblings, under another
/// agent, or into another project. Its children (and theirs) come with it.
/// Refused, with nothing changed, if an id is unknown or it would be placed
/// under itself or one of its own children.
class ArrangeAgent extends $pb.GeneratedMessage {
  factory ArrangeAgent({
    $core.String? agentId,
    $core.String? parentId,
    $core.String? beforeId,
    $core.String? projectId,
    $core.String? folderId,
  }) {
    final result = ArrangeAgent._();
    if (agentId != null) result.agentId = agentId;
    if (parentId != null) result.parentId = parentId;
    if (beforeId != null) result.beforeId = beforeId;
    if (projectId != null) result.projectId = projectId;
    if (folderId != null) result.folderId = folderId;
    return result;
  }

  ArrangeAgent._();

  factory ArrangeAgent.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ArrangeAgent()..mergeFromBuffer(data, registry);
  factory ArrangeAgent.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ArrangeAgent()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ArrangeAgent',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ArrangeAgent.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..aOS(2, _omitFieldNames ? '' : 'parentId')
    ..aOS(3, _omitFieldNames ? '' : 'beforeId')
    ..aOS(4, _omitFieldNames ? '' : 'projectId')
    ..aOS(5, _omitFieldNames ? '' : 'folderId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ArrangeAgent clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ArrangeAgent copyWith(void Function(ArrangeAgent) updates) =>
      super.copyWith((message) => updates(message as ArrangeAgent))
          as ArrangeAgent;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ArrangeAgent() / ArrangeAgent.new instead')
  static ArrangeAgent create() => ArrangeAgent._();
  static $pb.GeneratedMessage $_createMessage() => ArrangeAgent._();
  @$core.override
  ArrangeAgent createEmptyInstance() => ArrangeAgent._();
  @$core.pragma('dart2js:noInline')
  static ArrangeAgent getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ArrangeAgent>(
          ArrangeAgent.$_createMessage);
  static ArrangeAgent? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  /// The agent to nest it under; empty for the top level. In another project,
  /// that moves it to that project.
  @$pb.TagNumber(2)
  $core.String get parentId => $_getSZ(1);
  @$pb.TagNumber(2)
  set parentId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasParentId() => $_has(1);
  @$pb.TagNumber(2)
  void clearParentId() => $_clearField(2);

  /// The sibling to place it before; empty to place it last.
  @$pb.TagNumber(3)
  $core.String get beforeId => $_getSZ(2);
  @$pb.TagNumber(3)
  set beforeId($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasBeforeId() => $_has(2);
  @$pb.TagNumber(3)
  void clearBeforeId() => $_clearField(3);

  /// Moves it to this project ("" for the workspace) at the top level or under
  /// `parent_id`, which must then be in that project. Unset keeps its project,
  /// or takes `parent_id`'s.
  @$pb.TagNumber(4)
  $core.String get projectId => $_getSZ(3);
  @$pb.TagNumber(4)
  set projectId($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasProjectId() => $_has(3);
  @$pb.TagNumber(4)
  void clearProjectId() => $_clearField(4);

  /// Puts it in this folder of its (new) project (see Agent.folder_id; "" for
  /// the project's own folder), in the same step. Unset keeps its folder, or
  /// the project's own folder when it moves to another project.
  @$pb.TagNumber(5)
  $core.String get folderId => $_getSZ(4);
  @$pb.TagNumber(5)
  set folderId($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasFolderId() => $_has(4);
  @$pb.TagNumber(5)
  void clearFolderId() => $_clearField(5);
}

/// Folds an agent's children away in the sidebar, or shows them.
class SetAgentCollapsed extends $pb.GeneratedMessage {
  factory SetAgentCollapsed({
    $core.String? agentId,
    $core.bool? collapsed,
  }) {
    final result = SetAgentCollapsed._();
    if (agentId != null) result.agentId = agentId;
    if (collapsed != null) result.collapsed = collapsed;
    return result;
  }

  SetAgentCollapsed._();

  factory SetAgentCollapsed.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetAgentCollapsed()..mergeFromBuffer(data, registry);
  factory SetAgentCollapsed.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetAgentCollapsed()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SetAgentCollapsed',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SetAgentCollapsed.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..aOB(2, _omitFieldNames ? '' : 'collapsed')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetAgentCollapsed clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetAgentCollapsed copyWith(void Function(SetAgentCollapsed) updates) =>
      super.copyWith((message) => updates(message as SetAgentCollapsed))
          as SetAgentCollapsed;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SetAgentCollapsed() / SetAgentCollapsed.new instead')
  static SetAgentCollapsed create() => SetAgentCollapsed._();
  static $pb.GeneratedMessage $_createMessage() => SetAgentCollapsed._();
  @$core.override
  SetAgentCollapsed createEmptyInstance() => SetAgentCollapsed._();
  @$core.pragma('dart2js:noInline')
  static SetAgentCollapsed getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<SetAgentCollapsed>(
          SetAgentCollapsed.$_createMessage);
  static SetAgentCollapsed? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.bool get collapsed => $_getBF(1);
  @$pb.TagNumber(2)
  set collapsed($core.bool value) => $_setBool(1, value);
  @$pb.TagNumber(2)
  $core.bool hasCollapsed() => $_has(1);
  @$pb.TagNumber(2)
  void clearCollapsed() => $_clearField(2);
}

/// Turns highlights (Workspace.highlights) on or off for this Factory.
class SetHighlights extends $pb.GeneratedMessage {
  factory SetHighlights({
    $core.bool? on,
  }) {
    final result = SetHighlights._();
    if (on != null) result.on = on;
    return result;
  }

  SetHighlights._();

  factory SetHighlights.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetHighlights()..mergeFromBuffer(data, registry);
  factory SetHighlights.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetHighlights()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SetHighlights',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SetHighlights.$_createMessage)
    ..aOB(1, _omitFieldNames ? '' : 'on')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetHighlights clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetHighlights copyWith(void Function(SetHighlights) updates) =>
      super.copyWith((message) => updates(message as SetHighlights))
          as SetHighlights;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SetHighlights() / SetHighlights.new instead')
  static SetHighlights create() => SetHighlights._();
  static $pb.GeneratedMessage $_createMessage() => SetHighlights._();
  @$core.override
  SetHighlights createEmptyInstance() => SetHighlights._();
  @$core.pragma('dart2js:noInline')
  static SetHighlights getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<SetHighlights>(
          SetHighlights.$_createMessage);
  static SetHighlights? _defaultInstance;

  @$pb.TagNumber(1)
  $core.bool get on => $_getBF(0);
  @$pb.TagNumber(1)
  set on($core.bool value) => $_setBool(0, value);
  @$pb.TagNumber(1)
  $core.bool hasOn() => $_has(0);
  @$pb.TagNumber(1)
  void clearOn() => $_clearField(1);
}

/// Stops an agent's background task (its whole process group). The agent gets
/// the usual event that it finished, "stopped by the person".
class StopTask extends $pb.GeneratedMessage {
  factory StopTask({
    $core.String? agentId,
    $core.String? taskId,
  }) {
    final result = StopTask._();
    if (agentId != null) result.agentId = agentId;
    if (taskId != null) result.taskId = taskId;
    return result;
  }

  StopTask._();

  factory StopTask.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      StopTask()..mergeFromBuffer(data, registry);
  factory StopTask.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      StopTask()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'StopTask',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: StopTask.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..aOS(2, _omitFieldNames ? '' : 'taskId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  StopTask clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  StopTask copyWith(void Function(StopTask) updates) =>
      super.copyWith((message) => updates(message as StopTask)) as StopTask;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use StopTask() / StopTask.new instead')
  static StopTask create() => StopTask._();
  static $pb.GeneratedMessage $_createMessage() => StopTask._();
  @$core.override
  StopTask createEmptyInstance() => StopTask._();
  @$core.pragma('dart2js:noInline')
  static StopTask getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<StopTask>(StopTask.$_createMessage);
  static StopTask? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  /// Its id, such as "task-3".
  @$pb.TagNumber(2)
  $core.String get taskId => $_getSZ(1);
  @$pb.TagNumber(2)
  set taskId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasTaskId() => $_has(1);
  @$pb.TagNumber(2)
  void clearTaskId() => $_clearField(2);
}

/// Pauses, resumes or cancels one of an agent's monitors (the Monitors
/// panel's buttons), as the person. Pause and cancel stop a watch's command;
/// resume picks up from now: a time monitor that came due meanwhile fires
/// once. Refused (INVALID_REQUEST) when the monitor's state doesn't allow it,
/// such as resuming an active one or changing an ended one.
class ChangeMonitor extends $pb.GeneratedMessage {
  factory ChangeMonitor({
    $core.String? agentId,
    $core.int? monitorId,
    MonitorAction? action,
  }) {
    final result = ChangeMonitor._();
    if (agentId != null) result.agentId = agentId;
    if (monitorId != null) result.monitorId = monitorId;
    if (action != null) result.action = action;
    return result;
  }

  ChangeMonitor._();

  factory ChangeMonitor.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ChangeMonitor()..mergeFromBuffer(data, registry);
  factory ChangeMonitor.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ChangeMonitor()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ChangeMonitor',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ChangeMonitor.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..aI(2, _omitFieldNames ? '' : 'monitorId', fieldType: $pb.PbFieldType.OU3)
    ..aE<MonitorAction>(3, _omitFieldNames ? '' : 'action',
        enumValues: MonitorAction.values)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChangeMonitor clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChangeMonitor copyWith(void Function(ChangeMonitor) updates) =>
      super.copyWith((message) => updates(message as ChangeMonitor))
          as ChangeMonitor;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ChangeMonitor() / ChangeMonitor.new instead')
  static ChangeMonitor create() => ChangeMonitor._();
  static $pb.GeneratedMessage $_createMessage() => ChangeMonitor._();
  @$core.override
  ChangeMonitor createEmptyInstance() => ChangeMonitor._();
  @$core.pragma('dart2js:noInline')
  static ChangeMonitor getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ChangeMonitor>(
          ChangeMonitor.$_createMessage);
  static ChangeMonitor? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  /// Its number: 3 for m3.
  @$pb.TagNumber(2)
  $core.int get monitorId => $_getIZ(1);
  @$pb.TagNumber(2)
  set monitorId($core.int value) => $_setUnsignedInt32(1, value);
  @$pb.TagNumber(2)
  $core.bool hasMonitorId() => $_has(1);
  @$pb.TagNumber(2)
  void clearMonitorId() => $_clearField(2);

  @$pb.TagNumber(3)
  MonitorAction get action => $_getN(2);
  @$pb.TagNumber(3)
  set action(MonitorAction value) => $_setField(3, value);
  @$pb.TagNumber(3)
  $core.bool hasAction() => $_has(2);
  @$pb.TagNumber(3)
  void clearAction() => $_clearField(3);
}

/// Clears one build cache now (a `DiskUsage` of kind "build_cache", by its
/// `path`): moved aside at once, deleted in the background; the agents
/// working there hear of it at their next run. Answered with Ack, or an error
/// saying why not: it isn't a build cache Factory knows, or an agent is
/// working there, or a build is running in it.
class ClearBuildCache extends $pb.GeneratedMessage {
  factory ClearBuildCache({
    $core.String? path,
  }) {
    final result = ClearBuildCache._();
    if (path != null) result.path = path;
    return result;
  }

  ClearBuildCache._();

  factory ClearBuildCache.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ClearBuildCache()..mergeFromBuffer(data, registry);
  factory ClearBuildCache.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ClearBuildCache()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ClearBuildCache',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ClearBuildCache.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'path')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ClearBuildCache clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ClearBuildCache copyWith(void Function(ClearBuildCache) updates) =>
      super.copyWith((message) => updates(message as ClearBuildCache))
          as ClearBuildCache;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ClearBuildCache() / ClearBuildCache.new instead')
  static ClearBuildCache create() => ClearBuildCache._();
  static $pb.GeneratedMessage $_createMessage() => ClearBuildCache._();
  @$core.override
  ClearBuildCache createEmptyInstance() => ClearBuildCache._();
  @$core.pragma('dart2js:noInline')
  static ClearBuildCache getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ClearBuildCache>(
          ClearBuildCache.$_createMessage);
  static ClearBuildCache? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get path => $_getSZ(0);
  @$pb.TagNumber(1)
  set path($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasPath() => $_has(0);
  @$pb.TagNumber(1)
  void clearPath() => $_clearField(1);
}

/// The person approves or dismisses an agent's proposal to Rivet (a
/// `Proposal` entry in `agent_id`'s transcript), answered with Ack. Approved,
/// Rivet gets it as the person's own message: `comment` (if any), a blank
/// line, "<Agent> (<role>) proposes:" and the proposal. Either way the agent
/// is told. An error when it's no longer pending, or when approving and
/// there's no Rivet or Rivet has no model (it stays pending).
class DecideProposal extends $pb.GeneratedMessage {
  factory DecideProposal({
    $core.String? agentId,
    $core.String? proposalId,
    $core.bool? approve,
    $core.String? comment,
  }) {
    final result = DecideProposal._();
    if (agentId != null) result.agentId = agentId;
    if (proposalId != null) result.proposalId = proposalId;
    if (approve != null) result.approve = approve;
    if (comment != null) result.comment = comment;
    return result;
  }

  DecideProposal._();

  factory DecideProposal.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DecideProposal()..mergeFromBuffer(data, registry);
  factory DecideProposal.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DecideProposal()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'DecideProposal',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: DecideProposal.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..aOS(2, _omitFieldNames ? '' : 'proposalId')
    ..aOB(3, _omitFieldNames ? '' : 'approve')
    ..aOS(4, _omitFieldNames ? '' : 'comment')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DecideProposal clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DecideProposal copyWith(void Function(DecideProposal) updates) =>
      super.copyWith((message) => updates(message as DecideProposal))
          as DecideProposal;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use DecideProposal() / DecideProposal.new instead')
  static DecideProposal create() => DecideProposal._();
  static $pb.GeneratedMessage $_createMessage() => DecideProposal._();
  @$core.override
  DecideProposal createEmptyInstance() => DecideProposal._();
  @$core.pragma('dart2js:noInline')
  static DecideProposal getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<DecideProposal>(
          DecideProposal.$_createMessage);
  static DecideProposal? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get proposalId => $_getSZ(1);
  @$pb.TagNumber(2)
  set proposalId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasProposalId() => $_has(1);
  @$pb.TagNumber(2)
  void clearProposalId() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.bool get approve => $_getBF(2);
  @$pb.TagNumber(3)
  set approve($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasApprove() => $_has(2);
  @$pb.TagNumber(3)
  void clearApprove() => $_clearField(3);

  /// What the person adds for Rivet; may be empty.
  @$pb.TagNumber(4)
  $core.String get comment => $_getSZ(3);
  @$pb.TagNumber(4)
  set comment($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasComment() => $_has(3);
  @$pb.TagNumber(4)
  void clearComment() => $_clearField(4);
}

/// Folds a project folder's agents away in the sidebar, or shows them.
class SetFolderCollapsed extends $pb.GeneratedMessage {
  factory SetFolderCollapsed({
    $core.String? projectId,
    $core.String? folderId,
    $core.bool? collapsed,
  }) {
    final result = SetFolderCollapsed._();
    if (projectId != null) result.projectId = projectId;
    if (folderId != null) result.folderId = folderId;
    if (collapsed != null) result.collapsed = collapsed;
    return result;
  }

  SetFolderCollapsed._();

  factory SetFolderCollapsed.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetFolderCollapsed()..mergeFromBuffer(data, registry);
  factory SetFolderCollapsed.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetFolderCollapsed()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SetFolderCollapsed',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SetFolderCollapsed.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'projectId')
    ..aOS(2, _omitFieldNames ? '' : 'folderId')
    ..aOB(3, _omitFieldNames ? '' : 'collapsed')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetFolderCollapsed clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetFolderCollapsed copyWith(void Function(SetFolderCollapsed) updates) =>
      super.copyWith((message) => updates(message as SetFolderCollapsed))
          as SetFolderCollapsed;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SetFolderCollapsed() / SetFolderCollapsed.new instead')
  static SetFolderCollapsed create() => SetFolderCollapsed._();
  static $pb.GeneratedMessage $_createMessage() => SetFolderCollapsed._();
  @$core.override
  SetFolderCollapsed createEmptyInstance() => SetFolderCollapsed._();
  @$core.pragma('dart2js:noInline')
  static SetFolderCollapsed getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<SetFolderCollapsed>(
          SetFolderCollapsed.$_createMessage);
  static SetFolderCollapsed? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get projectId => $_getSZ(0);
  @$pb.TagNumber(1)
  set projectId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProjectId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProjectId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get folderId => $_getSZ(1);
  @$pb.TagNumber(2)
  set folderId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasFolderId() => $_has(1);
  @$pb.TagNumber(2)
  void clearFolderId() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.bool get collapsed => $_getBF(2);
  @$pb.TagNumber(3)
  set collapsed($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasCollapsed() => $_has(2);
  @$pb.TagNumber(3)
  void clearCollapsed() => $_clearField(3);
}

/// Runs one of the agent's slash commands (`Agent.commands`), as the person
/// typing "/<name> <args>" in its message box. A command that can't run now
/// is an INVALID_REQUEST error whose message can be shown as it is.
class RunCommand extends $pb.GeneratedMessage {
  factory RunCommand({
    $core.String? agentId,
    $core.String? name,
    $core.String? args,
  }) {
    final result = RunCommand._();
    if (agentId != null) result.agentId = agentId;
    if (name != null) result.name = name;
    if (args != null) result.args = args;
    return result;
  }

  RunCommand._();

  factory RunCommand.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      RunCommand()..mergeFromBuffer(data, registry);
  factory RunCommand.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      RunCommand()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'RunCommand',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: RunCommand.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'args')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  RunCommand clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  RunCommand copyWith(void Function(RunCommand) updates) =>
      super.copyWith((message) => updates(message as RunCommand)) as RunCommand;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use RunCommand() / RunCommand.new instead')
  static RunCommand create() => RunCommand._();
  static $pb.GeneratedMessage $_createMessage() => RunCommand._();
  @$core.override
  RunCommand createEmptyInstance() => RunCommand._();
  @$core.pragma('dart2js:noInline')
  static RunCommand getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<RunCommand>(RunCommand.$_createMessage);
  static RunCommand? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  /// "clear", without the slash.
  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// What follows the name, trimmed; empty if nothing.
  @$pb.TagNumber(3)
  $core.String get args => $_getSZ(2);
  @$pb.TagNumber(3)
  set args($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasArgs() => $_has(2);
  @$pb.TagNumber(3)
  void clearArgs() => $_clearField(3);
}

/// Reads a file on Factory's machine, for a viewer: the app may be on another
/// machine. Answered to the asker only with FileContent (whose `error` says
/// why it couldn't be read), never with Error.
class ReadFile extends $pb.GeneratedMessage {
  factory ReadFile({
    $core.String? path,
    $fixnum.Int64? maxBytes,
  }) {
    final result = ReadFile._();
    if (path != null) result.path = path;
    if (maxBytes != null) result.maxBytes = maxBytes;
    return result;
  }

  ReadFile._();

  factory ReadFile.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ReadFile()..mergeFromBuffer(data, registry);
  factory ReadFile.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ReadFile()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ReadFile',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ReadFile.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'path')
    ..a<$fixnum.Int64>(
        2, _omitFieldNames ? '' : 'maxBytes', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ReadFile clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ReadFile copyWith(void Function(ReadFile) updates) =>
      super.copyWith((message) => updates(message as ReadFile)) as ReadFile;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ReadFile() / ReadFile.new instead')
  static ReadFile create() => ReadFile._();
  static $pb.GeneratedMessage $_createMessage() => ReadFile._();
  @$core.override
  ReadFile createEmptyInstance() => ReadFile._();
  @$core.pragma('dart2js:noInline')
  static ReadFile getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<ReadFile>(ReadFile.$_createMessage);
  static ReadFile? _defaultInstance;

  /// Absolute, or from the home folder ("~/notes.md").
  @$pb.TagNumber(1)
  $core.String get path => $_getSZ(0);
  @$pb.TagNumber(1)
  set path($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasPath() => $_has(0);
  @$pb.TagNumber(1)
  void clearPath() => $_clearField(1);

  /// At most this many bytes; 0 for the default. Text is read up to 2 MB and
  /// images up to 20 MB, whatever is asked.
  @$pb.TagNumber(2)
  $fixnum.Int64 get maxBytes => $_getI64(1);
  @$pb.TagNumber(2)
  set maxBytes($fixnum.Int64 value) => $_setInt64(1, value);
  @$pb.TagNumber(2)
  $core.bool hasMaxBytes() => $_has(1);
  @$pb.TagNumber(2)
  void clearMaxBytes() => $_clearField(2);
}

class FileContent extends $pb.GeneratedMessage {
  factory FileContent({
    $core.String? path,
    $core.String? mime,
    $fixnum.Int64? size,
    $core.List<$core.int>? data,
    $core.bool? truncated,
    $fixnum.Int64? modifiedMs,
    $core.String? error,
  }) {
    final result = FileContent._();
    if (path != null) result.path = path;
    if (mime != null) result.mime = mime;
    if (size != null) result.size = size;
    if (data != null) result.data = data;
    if (truncated != null) result.truncated = truncated;
    if (modifiedMs != null) result.modifiedMs = modifiedMs;
    if (error != null) result.error = error;
    return result;
  }

  FileContent._();

  factory FileContent.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      FileContent()..mergeFromBuffer(data, registry);
  factory FileContent.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      FileContent()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'FileContent',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: FileContent.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'path')
    ..aOS(2, _omitFieldNames ? '' : 'mime')
    ..a<$fixnum.Int64>(3, _omitFieldNames ? '' : 'size', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..a<$core.List<$core.int>>(
        4, _omitFieldNames ? '' : 'data', $pb.PbFieldType.OY)
    ..aOB(5, _omitFieldNames ? '' : 'truncated')
    ..aInt64(6, _omitFieldNames ? '' : 'modifiedMs')
    ..aOS(7, _omitFieldNames ? '' : 'error')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  FileContent clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  FileContent copyWith(void Function(FileContent) updates) =>
      super.copyWith((message) => updates(message as FileContent))
          as FileContent;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use FileContent() / FileContent.new instead')
  static FileContent create() => FileContent._();
  static $pb.GeneratedMessage $_createMessage() => FileContent._();
  @$core.override
  FileContent createEmptyInstance() => FileContent._();
  @$core.pragma('dart2js:noInline')
  static FileContent getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<FileContent>(
          FileContent.$_createMessage);
  static FileContent? _defaultInstance;

  /// As asked, with "~" expanded.
  @$pb.TagNumber(1)
  $core.String get path => $_getSZ(0);
  @$pb.TagNumber(1)
  set path($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasPath() => $_has(0);
  @$pb.TagNumber(1)
  void clearPath() => $_clearField(1);

  /// Sniffed from its content and extension: "text/plain", "text/markdown",
  /// "application/json", "image/png", "image/jpeg", "image/gif",
  /// "image/webp", "application/pdf", "application/octet-stream"…
  @$pb.TagNumber(2)
  $core.String get mime => $_getSZ(1);
  @$pb.TagNumber(2)
  set mime($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasMime() => $_has(1);
  @$pb.TagNumber(2)
  void clearMime() => $_clearField(2);

  /// The whole file's size in bytes.
  @$pb.TagNumber(3)
  $fixnum.Int64 get size => $_getI64(2);
  @$pb.TagNumber(3)
  set size($fixnum.Int64 value) => $_setInt64(2, value);
  @$pb.TagNumber(3)
  $core.bool hasSize() => $_has(2);
  @$pb.TagNumber(3)
  void clearSize() => $_clearField(3);

  /// Its first bytes, up to the limit.
  @$pb.TagNumber(4)
  $core.List<$core.int> get data => $_getN(3);
  @$pb.TagNumber(4)
  set data($core.List<$core.int> value) => $_setBytes(3, value);
  @$pb.TagNumber(4)
  $core.bool hasData() => $_has(3);
  @$pb.TagNumber(4)
  void clearData() => $_clearField(4);

  /// `data` is only the start of it.
  @$pb.TagNumber(5)
  $core.bool get truncated => $_getBF(4);
  @$pb.TagNumber(5)
  set truncated($core.bool value) => $_setBool(4, value);
  @$pb.TagNumber(5)
  $core.bool hasTruncated() => $_has(4);
  @$pb.TagNumber(5)
  void clearTruncated() => $_clearField(5);

  /// When it was last changed, in ms since 1970.
  @$pb.TagNumber(6)
  $fixnum.Int64 get modifiedMs => $_getI64(5);
  @$pb.TagNumber(6)
  set modifiedMs($fixnum.Int64 value) => $_setInt64(5, value);
  @$pb.TagNumber(6)
  $core.bool hasModifiedMs() => $_has(5);
  @$pb.TagNumber(6)
  void clearModifiedMs() => $_clearField(6);

  /// Why it couldn't be read ("No such file: …", "… is a folder", "Can't
  /// read …: permission denied"); empty when it was. The rest is empty then.
  @$pb.TagNumber(7)
  $core.String get error => $_getSZ(6);
  @$pb.TagNumber(7)
  set error($core.String value) => $_setString(6, value);
  @$pb.TagNumber(7)
  $core.bool hasError() => $_has(6);
  @$pb.TagNumber(7)
  void clearError() => $_clearField(7);
}

/// Sets an agent's layout (`Agent.layout`) for every device, answered with
/// Ack. Refused unless it has exactly one "chat" leaf, at most 12 leaves,
/// unique non-empty ids, and splits of two or more children with as many
/// positive weights (or none). An unset root, or a lone chat leaf, goes back
/// to just the conversation.
class SetLayout extends $pb.GeneratedMessage {
  factory SetLayout({
    $core.String? agentId,
    ViewNode? root,
  }) {
    final result = SetLayout._();
    if (agentId != null) result.agentId = agentId;
    if (root != null) result.root = root;
    return result;
  }

  SetLayout._();

  factory SetLayout.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetLayout()..mergeFromBuffer(data, registry);
  factory SetLayout.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetLayout()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SetLayout',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SetLayout.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..aOM<ViewNode>(2, _omitFieldNames ? '' : 'root',
        subBuilder: ViewNode.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetLayout clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetLayout copyWith(void Function(SetLayout) updates) =>
      super.copyWith((message) => updates(message as SetLayout)) as SetLayout;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SetLayout() / SetLayout.new instead')
  static SetLayout create() => SetLayout._();
  static $pb.GeneratedMessage $_createMessage() => SetLayout._();
  @$core.override
  SetLayout createEmptyInstance() => SetLayout._();
  @$core.pragma('dart2js:noInline')
  static SetLayout getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<SetLayout>(SetLayout.$_createMessage);
  static SetLayout? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  @$pb.TagNumber(2)
  ViewNode get root => $_getN(1);
  @$pb.TagNumber(2)
  set root(ViewNode value) => $_setField(2, value);
  @$pb.TagNumber(2)
  $core.bool hasRoot() => $_has(1);
  @$pb.TagNumber(2)
  void clearRoot() => $_clearField(2);
  @$pb.TagNumber(2)
  ViewNode ensureRoot() => $_ensure(1);
}

enum ViewNode_Node { split, leaf, notSet }

/// A pane of an agent's main area, or a split of several.
class ViewNode extends $pb.GeneratedMessage {
  factory ViewNode({
    $core.String? id,
    ViewSplit? split,
    ViewLeaf? leaf,
  }) {
    final result = ViewNode._();
    if (id != null) result.id = id;
    if (split != null) result.split = split;
    if (leaf != null) result.leaf = leaf;
    return result;
  }

  ViewNode._();

  factory ViewNode.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ViewNode()..mergeFromBuffer(data, registry);
  factory ViewNode.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ViewNode()..mergeFromJson(json, registry);

  static const $core.Map<$core.int, ViewNode_Node> _ViewNode_NodeByTag = {
    2: ViewNode_Node.split,
    3: ViewNode_Node.leaf,
    0: ViewNode_Node.notSet
  };
  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ViewNode',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ViewNode.$_createMessage)
    ..oo(0, [2, 3])
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOM<ViewSplit>(2, _omitFieldNames ? '' : 'split',
        subBuilder: ViewSplit.$_createMessage)
    ..aOM<ViewLeaf>(3, _omitFieldNames ? '' : 'leaf',
        subBuilder: ViewLeaf.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ViewNode clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ViewNode copyWith(void Function(ViewNode) updates) =>
      super.copyWith((message) => updates(message as ViewNode)) as ViewNode;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ViewNode() / ViewNode.new instead')
  static ViewNode create() => ViewNode._();
  static $pb.GeneratedMessage $_createMessage() => ViewNode._();
  @$core.override
  ViewNode createEmptyInstance() => ViewNode._();
  @$core.pragma('dart2js:noInline')
  static ViewNode getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<ViewNode>(ViewNode.$_createMessage);
  static ViewNode? _defaultInstance;

  @$pb.TagNumber(2)
  @$pb.TagNumber(3)
  ViewNode_Node whichNode() => _ViewNode_NodeByTag[$_whichOneof(0)]!;
  @$pb.TagNumber(2)
  @$pb.TagNumber(3)
  void clearNode() => $_clearField($_whichOneof(0));

  /// Unique within the layout; the app keeps a pane by it.
  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  ViewSplit get split => $_getN(1);
  @$pb.TagNumber(2)
  set split(ViewSplit value) => $_setField(2, value);
  @$pb.TagNumber(2)
  $core.bool hasSplit() => $_has(1);
  @$pb.TagNumber(2)
  void clearSplit() => $_clearField(2);
  @$pb.TagNumber(2)
  ViewSplit ensureSplit() => $_ensure(1);

  @$pb.TagNumber(3)
  ViewLeaf get leaf => $_getN(2);
  @$pb.TagNumber(3)
  set leaf(ViewLeaf value) => $_setField(3, value);
  @$pb.TagNumber(3)
  $core.bool hasLeaf() => $_has(2);
  @$pb.TagNumber(3)
  void clearLeaf() => $_clearField(3);
  @$pb.TagNumber(3)
  ViewLeaf ensureLeaf() => $_ensure(2);
}

class ViewSplit extends $pb.GeneratedMessage {
  factory ViewSplit({
    ViewAxis? axis,
    $core.Iterable<ViewNode>? children,
    $core.Iterable<$core.double>? weights,
  }) {
    final result = ViewSplit._();
    if (axis != null) result.axis = axis;
    if (children != null) result.children.addAll(children);
    if (weights != null) result.weights.addAll(weights);
    return result;
  }

  ViewSplit._();

  factory ViewSplit.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ViewSplit()..mergeFromBuffer(data, registry);
  factory ViewSplit.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ViewSplit()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ViewSplit',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ViewSplit.$_createMessage)
    ..aE<ViewAxis>(1, _omitFieldNames ? '' : 'axis',
        enumValues: ViewAxis.values)
    ..pPM<ViewNode>(2, _omitFieldNames ? '' : 'children',
        subBuilder: ViewNode.$_createMessage)
    ..p<$core.double>(3, _omitFieldNames ? '' : 'weights', $pb.PbFieldType.KD)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ViewSplit clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ViewSplit copyWith(void Function(ViewSplit) updates) =>
      super.copyWith((message) => updates(message as ViewSplit)) as ViewSplit;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ViewSplit() / ViewSplit.new instead')
  static ViewSplit create() => ViewSplit._();
  static $pb.GeneratedMessage $_createMessage() => ViewSplit._();
  @$core.override
  ViewSplit createEmptyInstance() => ViewSplit._();
  @$core.pragma('dart2js:noInline')
  static ViewSplit getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<ViewSplit>(ViewSplit.$_createMessage);
  static ViewSplit? _defaultInstance;

  @$pb.TagNumber(1)
  ViewAxis get axis => $_getN(0);
  @$pb.TagNumber(1)
  set axis(ViewAxis value) => $_setField(1, value);
  @$pb.TagNumber(1)
  $core.bool hasAxis() => $_has(0);
  @$pb.TagNumber(1)
  void clearAxis() => $_clearField(1);

  @$pb.TagNumber(2)
  $pb.PbList<ViewNode> get children => $_getList(1);

  /// Each child's share, as many as children; empty for equal shares.
  @$pb.TagNumber(3)
  $pb.PbList<$core.double> get weights => $_getList(2);
}

class ViewLeaf extends $pb.GeneratedMessage {
  factory ViewLeaf({
    $core.String? kind,
    $core.String? path,
    $core.String? title,
  }) {
    final result = ViewLeaf._();
    if (kind != null) result.kind = kind;
    if (path != null) result.path = path;
    if (title != null) result.title = title;
    return result;
  }

  ViewLeaf._();

  factory ViewLeaf.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ViewLeaf()..mergeFromBuffer(data, registry);
  factory ViewLeaf.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ViewLeaf()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ViewLeaf',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ViewLeaf.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'kind')
    ..aOS(2, _omitFieldNames ? '' : 'path')
    ..aOS(3, _omitFieldNames ? '' : 'title')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ViewLeaf clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ViewLeaf copyWith(void Function(ViewLeaf) updates) =>
      super.copyWith((message) => updates(message as ViewLeaf)) as ViewLeaf;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ViewLeaf() / ViewLeaf.new instead')
  static ViewLeaf create() => ViewLeaf._();
  static $pb.GeneratedMessage $_createMessage() => ViewLeaf._();
  @$core.override
  ViewLeaf createEmptyInstance() => ViewLeaf._();
  @$core.pragma('dart2js:noInline')
  static ViewLeaf getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<ViewLeaf>(ViewLeaf.$_createMessage);
  static ViewLeaf? _defaultInstance;

  /// "chat" (the conversation, which every layout has once and can't be
  /// closed), "text" or "image"; more kinds will come (pdf, documents,
  /// dashboards, extension panels). An app that doesn't know a kind shows
  /// that it can't show it here.
  @$pb.TagNumber(1)
  $core.String get kind => $_getSZ(0);
  @$pb.TagNumber(1)
  set kind($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasKind() => $_has(0);
  @$pb.TagNumber(1)
  void clearKind() => $_clearField(1);

  /// The file it shows, absolute; empty for "chat".
  @$pb.TagNumber(2)
  $core.String get path => $_getSZ(1);
  @$pb.TagNumber(2)
  set path($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasPath() => $_has(1);
  @$pb.TagNumber(2)
  void clearPath() => $_clearField(2);

  /// What its tab or header says: the file's name, unless set otherwise.
  @$pb.TagNumber(3)
  $core.String get title => $_getSZ(2);
  @$pb.TagNumber(3)
  set title($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasTitle() => $_has(2);
  @$pb.TagNumber(3)
  void clearTitle() => $_clearField(3);
}

/// Opens an extension's panel beside an agent (`Agent.open_panel`); an empty id
/// closes it. Each agent keeps its own.
class OpenPanel extends $pb.GeneratedMessage {
  factory OpenPanel({
    $core.String? extensionId,
    $core.String? agentId,
  }) {
    final result = OpenPanel._();
    if (extensionId != null) result.extensionId = extensionId;
    if (agentId != null) result.agentId = agentId;
    return result;
  }

  OpenPanel._();

  factory OpenPanel.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      OpenPanel()..mergeFromBuffer(data, registry);
  factory OpenPanel.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      OpenPanel()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'OpenPanel',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: OpenPanel.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'extensionId')
    ..aOS(2, _omitFieldNames ? '' : 'agentId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  OpenPanel clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  OpenPanel copyWith(void Function(OpenPanel) updates) =>
      super.copyWith((message) => updates(message as OpenPanel)) as OpenPanel;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use OpenPanel() / OpenPanel.new instead')
  static OpenPanel create() => OpenPanel._();
  static $pb.GeneratedMessage $_createMessage() => OpenPanel._();
  @$core.override
  OpenPanel createEmptyInstance() => OpenPanel._();
  @$core.pragma('dart2js:noInline')
  static OpenPanel getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<OpenPanel>(OpenPanel.$_createMessage);
  static OpenPanel? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get extensionId => $_getSZ(0);
  @$pb.TagNumber(1)
  set extensionId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasExtensionId() => $_has(0);
  @$pb.TagNumber(1)
  void clearExtensionId() => $_clearField(1);

  /// The agent; empty for the selected one.
  @$pb.TagNumber(2)
  $core.String get agentId => $_getSZ(1);
  @$pb.TagNumber(2)
  set agentId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasAgentId() => $_has(1);
  @$pb.TagNumber(2)
  void clearAgentId() => $_clearField(2);
}

/// Starts signing in to a provider with one of its `Provider.methods`.
class StartLogin extends $pb.GeneratedMessage {
  factory StartLogin({
    $core.String? providerId,
    $core.String? methodId,
    $core.bool? addAccount,
  }) {
    final result = StartLogin._();
    if (providerId != null) result.providerId = providerId;
    if (methodId != null) result.methodId = methodId;
    if (addAccount != null) result.addAccount = addAccount;
    return result;
  }

  StartLogin._();

  factory StartLogin.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      StartLogin()..mergeFromBuffer(data, registry);
  factory StartLogin.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      StartLogin()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'StartLogin',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: StartLogin.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'providerId')
    ..aOS(2, _omitFieldNames ? '' : 'methodId')
    ..aOB(3, _omitFieldNames ? '' : 'addAccount')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  StartLogin clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  StartLogin copyWith(void Function(StartLogin) updates) =>
      super.copyWith((message) => updates(message as StartLogin)) as StartLogin;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use StartLogin() / StartLogin.new instead')
  static StartLogin create() => StartLogin._();
  static $pb.GeneratedMessage $_createMessage() => StartLogin._();
  @$core.override
  StartLogin createEmptyInstance() => StartLogin._();
  @$core.pragma('dart2js:noInline')
  static StartLogin getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<StartLogin>(StartLogin.$_createMessage);
  static StartLogin? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get providerId => $_getSZ(0);
  @$pb.TagNumber(1)
  set providerId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProviderId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProviderId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get methodId => $_getSZ(1);
  @$pb.TagNumber(2)
  set methodId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasMethodId() => $_has(1);
  @$pb.TagNumber(2)
  void clearMethodId() => $_clearField(2);

  /// Signs in to another account of the provider, keeping the ones signed in
  /// (a second Claude subscription, say). False signs in as before: the
  /// account replaces the first one, unless it's one already signed in, which
  /// is signed in again.
  @$pb.TagNumber(3)
  $core.bool get addAccount => $_getBF(2);
  @$pb.TagNumber(3)
  set addAccount($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasAddAccount() => $_has(2);
  @$pb.TagNumber(3)
  void clearAddAccount() => $_clearField(3);
}

/// Answers `LoginFlow.prompt`: the pasted code or redirect address, the secret,
/// the text, or the chosen option's id.
class AnswerLogin extends $pb.GeneratedMessage {
  factory AnswerLogin({
    $core.String? text,
  }) {
    final result = AnswerLogin._();
    if (text != null) result.text = text;
    return result;
  }

  AnswerLogin._();

  factory AnswerLogin.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AnswerLogin()..mergeFromBuffer(data, registry);
  factory AnswerLogin.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AnswerLogin()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'AnswerLogin',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: AnswerLogin.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'text')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AnswerLogin clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AnswerLogin copyWith(void Function(AnswerLogin) updates) =>
      super.copyWith((message) => updates(message as AnswerLogin))
          as AnswerLogin;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use AnswerLogin() / AnswerLogin.new instead')
  static AnswerLogin create() => AnswerLogin._();
  static $pb.GeneratedMessage $_createMessage() => AnswerLogin._();
  @$core.override
  AnswerLogin createEmptyInstance() => AnswerLogin._();
  @$core.pragma('dart2js:noInline')
  static AnswerLogin getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<AnswerLogin>(
          AnswerLogin.$_createMessage);
  static AnswerLogin? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get text => $_getSZ(0);
  @$pb.TagNumber(1)
  set text($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasText() => $_has(0);
  @$pb.TagNumber(1)
  void clearText() => $_clearField(1);
}

/// Cancels the sign-in in progress, or dismisses one that failed.
class CancelLogin extends $pb.GeneratedMessage {
  factory CancelLogin() => CancelLogin._();

  CancelLogin._();

  factory CancelLogin.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CancelLogin()..mergeFromBuffer(data, registry);
  factory CancelLogin.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CancelLogin()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'CancelLogin',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: CancelLogin.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CancelLogin clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CancelLogin copyWith(void Function(CancelLogin) updates) =>
      super.copyWith((message) => updates(message as CancelLogin))
          as CancelLogin;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use CancelLogin() / CancelLogin.new instead')
  static CancelLogin create() => CancelLogin._();
  static $pb.GeneratedMessage $_createMessage() => CancelLogin._();
  @$core.override
  CancelLogin createEmptyInstance() => CancelLogin._();
  @$core.pragma('dart2js:noInline')
  static CancelLogin getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<CancelLogin>(
          CancelLogin.$_createMessage);
  static CancelLogin? _defaultInstance;
}

/// Removes the credential Factory stored for a provider.
class SignOut extends $pb.GeneratedMessage {
  factory SignOut({
    $core.String? providerId,
    $core.String? accountId,
  }) {
    final result = SignOut._();
    if (providerId != null) result.providerId = providerId;
    if (accountId != null) result.accountId = accountId;
    return result;
  }

  SignOut._();

  factory SignOut.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SignOut()..mergeFromBuffer(data, registry);
  factory SignOut.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SignOut()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SignOut',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SignOut.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'providerId')
    ..aOS(2, _omitFieldNames ? '' : 'accountId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SignOut clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SignOut copyWith(void Function(SignOut) updates) =>
      super.copyWith((message) => updates(message as SignOut)) as SignOut;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SignOut() / SignOut.new instead')
  static SignOut create() => SignOut._();
  static $pb.GeneratedMessage $_createMessage() => SignOut._();
  @$core.override
  SignOut createEmptyInstance() => SignOut._();
  @$core.pragma('dart2js:noInline')
  static SignOut getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<SignOut>(SignOut.$_createMessage);
  static SignOut? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get providerId => $_getSZ(0);
  @$pb.TagNumber(1)
  set providerId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProviderId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProviderId() => $_clearField(1);

  /// The account to sign out of (`Account.id`); empty signs out of all of the
  /// provider's accounts.
  @$pb.TagNumber(2)
  $core.String get accountId => $_getSZ(1);
  @$pb.TagNumber(2)
  set accountId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasAccountId() => $_has(1);
  @$pb.TagNumber(2)
  void clearAccountId() => $_clearField(2);
}

/// Moves one of a provider's accounts to `position` (0 is first) in the
/// order requests try them: the first with capacity is used. Answered with Ack.
class MoveAccount extends $pb.GeneratedMessage {
  factory MoveAccount({
    $core.String? providerId,
    $core.String? accountId,
    $core.int? position,
  }) {
    final result = MoveAccount._();
    if (providerId != null) result.providerId = providerId;
    if (accountId != null) result.accountId = accountId;
    if (position != null) result.position = position;
    return result;
  }

  MoveAccount._();

  factory MoveAccount.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MoveAccount()..mergeFromBuffer(data, registry);
  factory MoveAccount.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MoveAccount()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'MoveAccount',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: MoveAccount.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'providerId')
    ..aOS(2, _omitFieldNames ? '' : 'accountId')
    ..aI(3, _omitFieldNames ? '' : 'position', fieldType: $pb.PbFieldType.OU3)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MoveAccount clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MoveAccount copyWith(void Function(MoveAccount) updates) =>
      super.copyWith((message) => updates(message as MoveAccount))
          as MoveAccount;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use MoveAccount() / MoveAccount.new instead')
  static MoveAccount create() => MoveAccount._();
  static $pb.GeneratedMessage $_createMessage() => MoveAccount._();
  @$core.override
  MoveAccount createEmptyInstance() => MoveAccount._();
  @$core.pragma('dart2js:noInline')
  static MoveAccount getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<MoveAccount>(
          MoveAccount.$_createMessage);
  static MoveAccount? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get providerId => $_getSZ(0);
  @$pb.TagNumber(1)
  set providerId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProviderId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProviderId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get accountId => $_getSZ(1);
  @$pb.TagNumber(2)
  set accountId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasAccountId() => $_has(1);
  @$pb.TagNumber(2)
  void clearAccountId() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.int get position => $_getIZ(2);
  @$pb.TagNumber(3)
  set position($core.int value) => $_setUnsignedInt32(2, value);
  @$pb.TagNumber(3)
  $core.bool hasPosition() => $_has(2);
  @$pb.TagNumber(3)
  void clearPosition() => $_clearField(3);
}

/// Changes one of a provider's accounts. Answered with Ack.
class UpdateAccount extends $pb.GeneratedMessage {
  factory UpdateAccount({
    $core.String? providerId,
    $core.String? accountId,
    $core.String? label,
    $core.bool? retry,
  }) {
    final result = UpdateAccount._();
    if (providerId != null) result.providerId = providerId;
    if (accountId != null) result.accountId = accountId;
    if (label != null) result.label = label;
    if (retry != null) result.retry = retry;
    return result;
  }

  UpdateAccount._();

  factory UpdateAccount.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UpdateAccount()..mergeFromBuffer(data, registry);
  factory UpdateAccount.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UpdateAccount()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'UpdateAccount',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: UpdateAccount.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'providerId')
    ..aOS(2, _omitFieldNames ? '' : 'accountId')
    ..aOS(3, _omitFieldNames ? '' : 'label')
    ..aOB(4, _omitFieldNames ? '' : 'retry')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UpdateAccount clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UpdateAccount copyWith(void Function(UpdateAccount) updates) =>
      super.copyWith((message) => updates(message as UpdateAccount))
          as UpdateAccount;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use UpdateAccount() / UpdateAccount.new instead')
  static UpdateAccount create() => UpdateAccount._();
  static $pb.GeneratedMessage $_createMessage() => UpdateAccount._();
  @$core.override
  UpdateAccount createEmptyInstance() => UpdateAccount._();
  @$core.pragma('dart2js:noInline')
  static UpdateAccount getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<UpdateAccount>(
          UpdateAccount.$_createMessage);
  static UpdateAccount? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get providerId => $_getSZ(0);
  @$pb.TagNumber(1)
  set providerId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProviderId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProviderId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get accountId => $_getSZ(1);
  @$pb.TagNumber(2)
  set accountId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasAccountId() => $_has(1);
  @$pb.TagNumber(2)
  void clearAccountId() => $_clearField(2);

  /// A name for it, such as "Work credits"; empty goes back to the one
  /// Factory gives it (an API key's last characters). Unset leaves it.
  @$pb.TagNumber(3)
  $core.String get label => $_getSZ(2);
  @$pb.TagNumber(3)
  set label($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasLabel() => $_has(2);
  @$pb.TagNumber(3)
  void clearLabel() => $_clearField(3);

  /// Try it again now: an API key out of credit (`Account.out_of_credit`)
  /// gets requests again, as if topped up.
  @$pb.TagNumber(4)
  $core.bool get retry => $_getBF(3);
  @$pb.TagNumber(4)
  set retry($core.bool value) => $_setBool(3, value);
  @$pb.TagNumber(4)
  $core.bool hasRetry() => $_has(3);
  @$pb.TagNumber(4)
  void clearRetry() => $_clearField(4);
}

class Ack extends $pb.GeneratedMessage {
  factory Ack() => Ack._();

  Ack._();

  factory Ack.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Ack()..mergeFromBuffer(data, registry);
  factory Ack.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Ack()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Ack',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Ack.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Ack clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Ack copyWith(void Function(Ack) updates) =>
      super.copyWith((message) => updates(message as Ack)) as Ack;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Ack() / Ack.new instead')
  static Ack create() => Ack._();
  static $pb.GeneratedMessage $_createMessage() => Ack._();
  @$core.override
  Ack createEmptyInstance() => Ack._();
  @$core.pragma('dart2js:noInline')
  static Ack getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Ack>(Ack.$_createMessage);
  static Ack? _defaultInstance;
}

class Error extends $pb.GeneratedMessage {
  factory Error({
    ErrorCode? code,
    $core.String? message,
  }) {
    final result = Error._();
    if (code != null) result.code = code;
    if (message != null) result.message = message;
    return result;
  }

  Error._();

  factory Error.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Error()..mergeFromBuffer(data, registry);
  factory Error.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Error()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Error',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Error.$_createMessage)
    ..aE<ErrorCode>(1, _omitFieldNames ? '' : 'code',
        enumValues: ErrorCode.values)
    ..aOS(2, _omitFieldNames ? '' : 'message')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Error clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Error copyWith(void Function(Error) updates) =>
      super.copyWith((message) => updates(message as Error)) as Error;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Error() / Error.new instead')
  static Error create() => Error._();
  static $pb.GeneratedMessage $_createMessage() => Error._();
  @$core.override
  Error createEmptyInstance() => Error._();
  @$core.pragma('dart2js:noInline')
  static Error getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Error>(Error.$_createMessage);
  static Error? _defaultInstance;

  @$pb.TagNumber(1)
  ErrorCode get code => $_getN(0);
  @$pb.TagNumber(1)
  set code(ErrorCode value) => $_setField(1, value);
  @$pb.TagNumber(1)
  $core.bool hasCode() => $_has(0);
  @$pb.TagNumber(1)
  void clearCode() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get message => $_getSZ(1);
  @$pb.TagNumber(2)
  set message($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasMessage() => $_has(1);
  @$pb.TagNumber(2)
  void clearMessage() => $_clearField(2);
}

/// Everything the sidebar and headers need. Sent in full after `welcome` and
/// whenever any part changes.
///
/// The workspace is Factory's own folder. Each project is a folder inside it,
/// and agents work in their project's folder, or in the workspace folder when
/// they aren't in a project.
class Workspace extends $pb.GeneratedMessage {
  factory Workspace({
    $core.Iterable<Project>? projects,
    $core.Iterable<Agent>? agents,
    $core.Iterable<Model>? models,
    $core.String? selectedAgentId,
    $core.String? configError,
    $core.Iterable<Provider>? providers,
    LoginFlow? login,
    Settings? settings,
    $core.String? version,
    $core.String? latestRelease,
    $core.String? focusedProjectId,
    $core.String? updateError,
    $core.String? latestBackend,
    $core.String? nextAgentColor,
    $core.Iterable<Extension>? extensions,
    $core.String? openPanel,
    $core.bool? highlights,
    $core.Iterable<ChangelogVersion>? changelog,
    RemoteAccess? remote,
    VaultState? vault,
    ClaktoryAccount? account,
    ClaktoryMap? map,
  }) {
    final result = Workspace._();
    if (projects != null) result.projects.addAll(projects);
    if (agents != null) result.agents.addAll(agents);
    if (models != null) result.models.addAll(models);
    if (selectedAgentId != null) result.selectedAgentId = selectedAgentId;
    if (configError != null) result.configError = configError;
    if (providers != null) result.providers.addAll(providers);
    if (login != null) result.login = login;
    if (settings != null) result.settings = settings;
    if (version != null) result.version = version;
    if (latestRelease != null) result.latestRelease = latestRelease;
    if (focusedProjectId != null) result.focusedProjectId = focusedProjectId;
    if (updateError != null) result.updateError = updateError;
    if (latestBackend != null) result.latestBackend = latestBackend;
    if (nextAgentColor != null) result.nextAgentColor = nextAgentColor;
    if (extensions != null) result.extensions.addAll(extensions);
    if (openPanel != null) result.openPanel = openPanel;
    if (highlights != null) result.highlights = highlights;
    if (changelog != null) result.changelog.addAll(changelog);
    if (remote != null) result.remote = remote;
    if (vault != null) result.vault = vault;
    if (account != null) result.account = account;
    if (map != null) result.map = map;
    return result;
  }

  Workspace._();

  factory Workspace.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Workspace()..mergeFromBuffer(data, registry);
  factory Workspace.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Workspace()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Workspace',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Workspace.$_createMessage)
    ..pPM<Project>(1, _omitFieldNames ? '' : 'projects',
        subBuilder: Project.$_createMessage)
    ..pPM<Agent>(2, _omitFieldNames ? '' : 'agents',
        subBuilder: Agent.$_createMessage)
    ..pPM<Model>(3, _omitFieldNames ? '' : 'models',
        subBuilder: Model.$_createMessage)
    ..aOS(4, _omitFieldNames ? '' : 'selectedAgentId')
    ..aOS(5, _omitFieldNames ? '' : 'configError')
    ..pPM<Provider>(6, _omitFieldNames ? '' : 'providers',
        subBuilder: Provider.$_createMessage)
    ..aOM<LoginFlow>(7, _omitFieldNames ? '' : 'login',
        subBuilder: LoginFlow.$_createMessage)
    ..aOM<Settings>(8, _omitFieldNames ? '' : 'settings',
        subBuilder: Settings.$_createMessage)
    ..aOS(9, _omitFieldNames ? '' : 'version')
    ..aOS(10, _omitFieldNames ? '' : 'latestRelease')
    ..aOS(11, _omitFieldNames ? '' : 'focusedProjectId')
    ..aOS(12, _omitFieldNames ? '' : 'updateError')
    ..aOS(13, _omitFieldNames ? '' : 'latestBackend')
    ..aOS(14, _omitFieldNames ? '' : 'nextAgentColor')
    ..pPM<Extension>(15, _omitFieldNames ? '' : 'extensions',
        subBuilder: Extension.$_createMessage)
    ..aOS(16, _omitFieldNames ? '' : 'openPanel')
    ..aOB(17, _omitFieldNames ? '' : 'highlights')
    ..pPM<ChangelogVersion>(18, _omitFieldNames ? '' : 'changelog',
        subBuilder: ChangelogVersion.$_createMessage)
    ..aOM<RemoteAccess>(19, _omitFieldNames ? '' : 'remote',
        subBuilder: RemoteAccess.$_createMessage)
    ..aOM<VaultState>(20, _omitFieldNames ? '' : 'vault',
        subBuilder: VaultState.$_createMessage)
    ..aOM<ClaktoryAccount>(21, _omitFieldNames ? '' : 'account',
        subBuilder: ClaktoryAccount.$_createMessage)
    ..aOM<ClaktoryMap>(22, _omitFieldNames ? '' : 'map',
        subBuilder: ClaktoryMap.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Workspace clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Workspace copyWith(void Function(Workspace) updates) =>
      super.copyWith((message) => updates(message as Workspace)) as Workspace;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Workspace() / Workspace.new instead')
  static Workspace create() => Workspace._();
  static $pb.GeneratedMessage $_createMessage() => Workspace._();
  @$core.override
  Workspace createEmptyInstance() => Workspace._();
  @$core.pragma('dart2js:noInline')
  static Workspace getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Workspace>(Workspace.$_createMessage);
  static Workspace? _defaultInstance;

  @$pb.TagNumber(1)
  $pb.PbList<Project> get projects => $_getList(0);

  @$pb.TagNumber(2)
  $pb.PbList<Agent> get agents => $_getList(1);

  /// Models from providers Factory has credentials for.
  @$pb.TagNumber(3)
  $pb.PbList<Model> get models => $_getList(2);

  /// Empty when no agent is selected.
  @$pb.TagNumber(4)
  $core.String get selectedAgentId => $_getSZ(3);
  @$pb.TagNumber(4)
  set selectedAgentId($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasSelectedAgentId() => $_has(3);
  @$pb.TagNumber(4)
  void clearSelectedAgentId() => $_clearField(4);

  /// Set when the config file on disk has an error; the last valid config stays
  /// in effect until it's fixed.
  @$pb.TagNumber(5)
  $core.String get configError => $_getSZ(4);
  @$pb.TagNumber(5)
  set configError($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasConfigError() => $_has(4);
  @$pb.TagNumber(5)
  void clearConfigError() => $_clearField(5);

  /// Every provider Factory can use, signed in or not.
  @$pb.TagNumber(6)
  $pb.PbList<Provider> get providers => $_getList(5);

  /// The sign-in in progress; unset when there is none.
  @$pb.TagNumber(7)
  LoginFlow get login => $_getN(6);
  @$pb.TagNumber(7)
  set login(LoginFlow value) => $_setField(7, value);
  @$pb.TagNumber(7)
  $core.bool hasLogin() => $_has(6);
  @$pb.TagNumber(7)
  void clearLogin() => $_clearField(7);
  @$pb.TagNumber(7)
  LoginFlow ensureLogin() => $_ensure(6);

  @$pb.TagNumber(8)
  Settings get settings => $_getN(7);
  @$pb.TagNumber(8)
  set settings(Settings value) => $_setField(8, value);
  @$pb.TagNumber(8)
  $core.bool hasSettings() => $_has(7);
  @$pb.TagNumber(8)
  void clearSettings() => $_clearField(8);
  @$pb.TagNumber(8)
  Settings ensureSettings() => $_ensure(7);

  /// This backend's build, e.g. "2026.11.18-1652"; "dev" for an unstamped build.
  /// Versions of stamped builds sort in release order.
  @$pb.TagNumber(9)
  $core.String get version => $_getSZ(8);
  @$pb.TagNumber(9)
  set version($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasVersion() => $_has(8);
  @$pb.TagNumber(9)
  void clearVersion() => $_clearField(9);

  /// The newest release published for this Factory (scripts/release.sh); empty
  /// when there is none. An app or backend older than it should update.
  @$pb.TagNumber(10)
  $core.String get latestRelease => $_getSZ(9);
  @$pb.TagNumber(10)
  set latestRelease($core.String value) => $_setString(9, value);
  @$pb.TagNumber(10)
  $core.bool hasLatestRelease() => $_has(9);
  @$pb.TagNumber(10)
  void clearLatestRelease() => $_clearField(10);

  /// The focused project; empty when none is. Its explorer follows as Explorer.
  @$pb.TagNumber(11)
  $core.String get focusedProjectId => $_getSZ(10);
  @$pb.TagNumber(11)
  set focusedProjectId($core.String value) => $_setString(10, value);
  @$pb.TagNumber(11)
  $core.bool hasFocusedProjectId() => $_has(10);
  @$pb.TagNumber(11)
  void clearFocusedProjectId() => $_clearField(11);

  /// Set when the last update's backend didn't start and Factory went back to
  /// the one before; says which versions and why.
  @$pb.TagNumber(12)
  $core.String get updateError => $_getSZ(11);
  @$pb.TagNumber(12)
  set updateError($core.String value) => $_setString(11, value);
  @$pb.TagNumber(12)
  $core.bool hasUpdateError() => $_has(11);
  @$pb.TagNumber(12)
  void clearUpdateError() => $_clearField(12);

  /// The backend in `latest_release`. Older than `latest_release` when the
  /// release only changed the app: installing it doesn't restart the backend.
  @$pb.TagNumber(13)
  $core.String get latestBackend => $_getSZ(12);
  @$pb.TagNumber(13)
  set latestBackend($core.String value) => $_setString(12, value);
  @$pb.TagNumber(13)
  $core.bool hasLatestBackend() => $_has(12);
  @$pb.TagNumber(13)
  void clearLatestBackend() => $_clearField(13);

  /// The color a new agent gets unless the person picks one: the one least
  /// like any agent's color, so agents stay easy to tell apart.
  @$pb.TagNumber(14)
  $core.String get nextAgentColor => $_getSZ(13);
  @$pb.TagNumber(14)
  set nextAgentColor($core.String value) => $_setString(13, value);
  @$pb.TagNumber(14)
  $core.bool hasNextAgentColor() => $_has(13);
  @$pb.TagNumber(14)
  void clearNextAgentColor() => $_clearField(14);

  /// Every extension Factory has, whether or not any agent uses it.
  @$pb.TagNumber(15)
  $pb.PbList<Extension> get extensions => $_getList(14);

  /// The selected agent's `Agent.open_panel`, for older apps; empty when no
  /// agent is selected or its panel is closed.
  @$pb.TagNumber(16)
  $core.String get openPanel => $_getSZ(15);
  @$pb.TagNumber(16)
  set openPanel($core.String value) => $_setString(15, value);
  @$pb.TagNumber(16)
  $core.bool hasOpenPanel() => $_has(15);
  @$pb.TagNumber(16)
  void clearOpenPanel() => $_clearField(16);

  /// The person wants highlights shown (see Highlight). Factory sends them
  /// either way; when this is off, the app doesn't show them.
  @$pb.TagNumber(17)
  $core.bool get highlights => $_getBF(16);
  @$pb.TagNumber(17)
  set highlights($core.bool value) => $_setBool(16, value);
  @$pb.TagNumber(17)
  $core.bool hasHighlights() => $_has(16);
  @$pb.TagNumber(17)
  void clearHighlights() => $_clearField(17);

  /// What changed in each version, newest first, from the changelog of the
  /// newest release (`latest_release`): every version up to it, so the app can
  /// show what an update brings (the versions newer than `version`) and what
  /// the running one brought. Empty when there's no release or it has no
  /// changelog. Only a dev release has an unreleased entry, first.
  @$pb.TagNumber(18)
  $pb.PbList<ChangelogVersion> get changelog => $_getList(17);

  /// Devices on other machines: whether they can connect, and which are paired.
  @$pb.TagNumber(19)
  RemoteAccess get remote => $_getN(18);
  @$pb.TagNumber(19)
  set remote(RemoteAccess value) => $_setField(19, value);
  @$pb.TagNumber(19)
  $core.bool hasRemote() => $_has(18);
  @$pb.TagNumber(19)
  void clearRemote() => $_clearField(19);
  @$pb.TagNumber(19)
  RemoteAccess ensureRemote() => $_ensure(18);

  /// The vault's state and its secrets' metadata, never values.
  @$pb.TagNumber(20)
  VaultState get vault => $_getN(19);
  @$pb.TagNumber(20)
  set vault(VaultState value) => $_setField(20, value);
  @$pb.TagNumber(20)
  $core.bool hasVault() => $_has(19);
  @$pb.TagNumber(20)
  void clearVault() => $_clearField(20);
  @$pb.TagNumber(20)
  VaultState ensureVault() => $_ensure(19);

  /// This Claktory's sign-in at claktory.com: the same on every device.
  @$pb.TagNumber(21)
  ClaktoryAccount get account => $_getN(20);
  @$pb.TagNumber(21)
  set account(ClaktoryAccount value) => $_setField(21, value);
  @$pb.TagNumber(21)
  $core.bool hasAccount() => $_has(20);
  @$pb.TagNumber(21)
  void clearAccount() => $_clearField(21);
  @$pb.TagNumber(21)
  ClaktoryAccount ensureAccount() => $_ensure(20);

  /// How this Claktory is arranged, for the Map: computed here, laid out by
  /// the app.
  @$pb.TagNumber(22)
  ClaktoryMap get map => $_getN(21);
  @$pb.TagNumber(22)
  set map(ClaktoryMap value) => $_setField(22, value);
  @$pb.TagNumber(22)
  $core.bool hasMap() => $_has(21);
  @$pb.TagNumber(22)
  void clearMap() => $_clearField(22);
  @$pb.TagNumber(22)
  ClaktoryMap ensureMap() => $_ensure(21);
}

class ChangelogVersion extends $pb.GeneratedMessage {
  factory ChangelogVersion({
    $core.String? version,
    $fixnum.Int64? released,
    $core.Iterable<ChangelogItem>? items,
    $core.bool? unreleased,
  }) {
    final result = ChangelogVersion._();
    if (version != null) result.version = version;
    if (released != null) result.released = released;
    if (items != null) result.items.addAll(items);
    if (unreleased != null) result.unreleased = unreleased;
    return result;
  }

  ChangelogVersion._();

  factory ChangelogVersion.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ChangelogVersion()..mergeFromBuffer(data, registry);
  factory ChangelogVersion.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ChangelogVersion()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ChangelogVersion',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ChangelogVersion.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'version')
    ..aInt64(2, _omitFieldNames ? '' : 'released')
    ..pPM<ChangelogItem>(3, _omitFieldNames ? '' : 'items',
        subBuilder: ChangelogItem.$_createMessage)
    ..aOB(4, _omitFieldNames ? '' : 'unreleased')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChangelogVersion clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChangelogVersion copyWith(void Function(ChangelogVersion) updates) =>
      super.copyWith((message) => updates(message as ChangelogVersion))
          as ChangelogVersion;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ChangelogVersion() / ChangelogVersion.new instead')
  static ChangelogVersion create() => ChangelogVersion._();
  static $pb.GeneratedMessage $_createMessage() => ChangelogVersion._();
  @$core.override
  ChangelogVersion createEmptyInstance() => ChangelogVersion._();
  @$core.pragma('dart2js:noInline')
  static ChangelogVersion getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ChangelogVersion>(
          ChangelogVersion.$_createMessage);
  static ChangelogVersion? _defaultInstance;

  /// e.g. "2026.11.18-2123"; empty for the unreleased entry.
  @$pb.TagNumber(1)
  $core.String get version => $_getSZ(0);
  @$pb.TagNumber(1)
  set version($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasVersion() => $_has(0);
  @$pb.TagNumber(1)
  void clearVersion() => $_clearField(1);

  /// When it was released (the time in its version), in ms since 1970; 0 for
  /// the unreleased entry.
  @$pb.TagNumber(2)
  $fixnum.Int64 get released => $_getI64(1);
  @$pb.TagNumber(2)
  set released($fixnum.Int64 value) => $_setInt64(1, value);
  @$pb.TagNumber(2)
  $core.bool hasReleased() => $_has(1);
  @$pb.TagNumber(2)
  void clearReleased() => $_clearField(2);

  /// In the changelog's order.
  @$pb.TagNumber(3)
  $pb.PbList<ChangelogItem> get items => $_getList(2);

  /// Changes not in a release yet (dev releases only).
  @$pb.TagNumber(4)
  $core.bool get unreleased => $_getBF(3);
  @$pb.TagNumber(4)
  set unreleased($core.bool value) => $_setBool(3, value);
  @$pb.TagNumber(4)
  $core.bool hasUnreleased() => $_has(3);
  @$pb.TagNumber(4)
  void clearUnreleased() => $_clearField(4);
}

class ChangelogItem extends $pb.GeneratedMessage {
  factory ChangelogItem({
    ChangelogKind? kind,
    $core.String? text,
    $core.String? raw,
  }) {
    final result = ChangelogItem._();
    if (kind != null) result.kind = kind;
    if (text != null) result.text = text;
    if (raw != null) result.raw = raw;
    return result;
  }

  ChangelogItem._();

  factory ChangelogItem.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ChangelogItem()..mergeFromBuffer(data, registry);
  factory ChangelogItem.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ChangelogItem()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ChangelogItem',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ChangelogItem.$_createMessage)
    ..aE<ChangelogKind>(1, _omitFieldNames ? '' : 'kind',
        enumValues: ChangelogKind.values)
    ..aOS(2, _omitFieldNames ? '' : 'text')
    ..aOS(3, _omitFieldNames ? '' : 'raw')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChangelogItem clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChangelogItem copyWith(void Function(ChangelogItem) updates) =>
      super.copyWith((message) => updates(message as ChangelogItem))
          as ChangelogItem;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ChangelogItem() / ChangelogItem.new instead')
  static ChangelogItem create() => ChangelogItem._();
  static $pb.GeneratedMessage $_createMessage() => ChangelogItem._();
  @$core.override
  ChangelogItem createEmptyInstance() => ChangelogItem._();
  @$core.pragma('dart2js:noInline')
  static ChangelogItem getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ChangelogItem>(
          ChangelogItem.$_createMessage);
  static ChangelogItem? _defaultInstance;

  @$pb.TagNumber(1)
  ChangelogKind get kind => $_getN(0);
  @$pb.TagNumber(1)
  set kind(ChangelogKind value) => $_setField(1, value);
  @$pb.TagNumber(1)
  $core.bool hasKind() => $_has(0);
  @$pb.TagNumber(1)
  void clearKind() => $_clearField(1);

  /// The change without its "Added"/"Changed:"/"Fixed:" prefix, starting with
  /// a capital, e.g. "The Tasks panel lists background tasks".
  @$pb.TagNumber(2)
  $core.String get text => $_getSZ(1);
  @$pb.TagNumber(2)
  set text($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasText() => $_has(1);
  @$pb.TagNumber(2)
  void clearText() => $_clearField(2);

  /// The line as written, e.g. "Added the Tasks panel…".
  @$pb.TagNumber(3)
  $core.String get raw => $_getSZ(2);
  @$pb.TagNumber(3)
  set raw($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasRaw() => $_has(2);
  @$pb.TagNumber(3)
  void clearRaw() => $_clearField(3);
}

/// Parts of the app to make glow briefly, so the person sees what happened or
/// what an agent is pointing at. Transient: never stored or sent again, and
/// only to the clients connected now.
class Highlight extends $pb.GeneratedMessage {
  factory Highlight({
    $core.Iterable<HighlightTarget>? targets,
    $core.String? text,
    $core.String? fromAgentId,
    HighlightKind? kind,
  }) {
    final result = Highlight._();
    if (targets != null) result.targets.addAll(targets);
    if (text != null) result.text = text;
    if (fromAgentId != null) result.fromAgentId = fromAgentId;
    if (kind != null) result.kind = kind;
    return result;
  }

  Highlight._();

  factory Highlight.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Highlight()..mergeFromBuffer(data, registry);
  factory Highlight.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Highlight()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Highlight',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Highlight.$_createMessage)
    ..pPM<HighlightTarget>(1, _omitFieldNames ? '' : 'targets',
        subBuilder: HighlightTarget.$_createMessage)
    ..aOS(2, _omitFieldNames ? '' : 'text')
    ..aOS(3, _omitFieldNames ? '' : 'fromAgentId')
    ..aE<HighlightKind>(4, _omitFieldNames ? '' : 'kind',
        enumValues: HighlightKind.values)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Highlight clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Highlight copyWith(void Function(Highlight) updates) =>
      super.copyWith((message) => updates(message as Highlight)) as Highlight;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Highlight() / Highlight.new instead')
  static Highlight create() => Highlight._();
  static $pb.GeneratedMessage $_createMessage() => Highlight._();
  @$core.override
  Highlight createEmptyInstance() => Highlight._();
  @$core.pragma('dart2js:noInline')
  static Highlight getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Highlight>(Highlight.$_createMessage);
  static Highlight? _defaultInstance;

  @$pb.TagNumber(1)
  $pb.PbList<HighlightTarget> get targets => $_getList(0);

  /// A short caption an agent gave (AGENT kind); may be empty.
  @$pb.TagNumber(2)
  $core.String get text => $_getSZ(1);
  @$pb.TagNumber(2)
  set text($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasText() => $_has(1);
  @$pb.TagNumber(2)
  void clearText() => $_clearField(2);

  /// The agent it came from: the sender of a message, or the agent that
  /// called the highlight tool.
  @$pb.TagNumber(3)
  $core.String get fromAgentId => $_getSZ(2);
  @$pb.TagNumber(3)
  set fromAgentId($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasFromAgentId() => $_has(2);
  @$pb.TagNumber(3)
  void clearFromAgentId() => $_clearField(3);

  @$pb.TagNumber(4)
  HighlightKind get kind => $_getN(3);
  @$pb.TagNumber(4)
  set kind(HighlightKind value) => $_setField(4, value);
  @$pb.TagNumber(4)
  $core.bool hasKind() => $_has(3);
  @$pb.TagNumber(4)
  void clearKind() => $_clearField(4);
}

/// What to highlight. Kinds the app doesn't know are skipped. `kind` and `id`:
///
/// - "agent": an agent's id (its sidebar row)
/// - "project": a project's id
/// - "checklist_item": "<agent id>#<item number>", e.g. "backend#4"
/// - "extension": "<agent id>/<extension id>", that extension's icon for the agent, e.g. "backend/checklist"
/// - "settings", "marketplace": the sidebar's buttons; `id` is empty
class HighlightTarget extends $pb.GeneratedMessage {
  factory HighlightTarget({
    $core.String? kind,
    $core.String? id,
  }) {
    final result = HighlightTarget._();
    if (kind != null) result.kind = kind;
    if (id != null) result.id = id;
    return result;
  }

  HighlightTarget._();

  factory HighlightTarget.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      HighlightTarget()..mergeFromBuffer(data, registry);
  factory HighlightTarget.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      HighlightTarget()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'HighlightTarget',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: HighlightTarget.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'kind')
    ..aOS(2, _omitFieldNames ? '' : 'id')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  HighlightTarget clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  HighlightTarget copyWith(void Function(HighlightTarget) updates) =>
      super.copyWith((message) => updates(message as HighlightTarget))
          as HighlightTarget;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use HighlightTarget() / HighlightTarget.new instead')
  static HighlightTarget create() => HighlightTarget._();
  static $pb.GeneratedMessage $_createMessage() => HighlightTarget._();
  @$core.override
  HighlightTarget createEmptyInstance() => HighlightTarget._();
  @$core.pragma('dart2js:noInline')
  static HighlightTarget getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<HighlightTarget>(
          HighlightTarget.$_createMessage);
  static HighlightTarget? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get kind => $_getSZ(0);
  @$pb.TagNumber(1)
  set kind($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasKind() => $_has(0);
  @$pb.TagNumber(1)
  void clearKind() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get id => $_getSZ(1);
  @$pb.TagNumber(2)
  set id($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasId() => $_has(1);
  @$pb.TagNumber(2)
  void clearId() => $_clearField(2);
}

/// A capability agents can have, such as messaging other agents. Each agent
/// has it on or off (`Agent.extensions`), with its own option values.
class Extension extends $pb.GeneratedMessage {
  factory Extension({
    $core.String? id,
    $core.String? name,
    $core.String? description,
    $core.bool? defaultEnabled,
    $core.Iterable<ExtensionOption>? options,
    $core.String? icon,
    $core.String? base,
    $core.bool? adminDefault,
  }) {
    final result = Extension._();
    if (id != null) result.id = id;
    if (name != null) result.name = name;
    if (description != null) result.description = description;
    if (defaultEnabled != null) result.defaultEnabled = defaultEnabled;
    if (options != null) result.options.addAll(options);
    if (icon != null) result.icon = icon;
    if (base != null) result.base = base;
    if (adminDefault != null) result.adminDefault = adminDefault;
    return result;
  }

  Extension._();

  factory Extension.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Extension()..mergeFromBuffer(data, registry);
  factory Extension.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Extension()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Extension',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Extension.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'description')
    ..aOB(4, _omitFieldNames ? '' : 'defaultEnabled')
    ..pPM<ExtensionOption>(5, _omitFieldNames ? '' : 'options',
        subBuilder: ExtensionOption.$_createMessage)
    ..aOS(6, _omitFieldNames ? '' : 'icon')
    ..aOS(7, _omitFieldNames ? '' : 'base')
    ..aOB(8, _omitFieldNames ? '' : 'adminDefault')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Extension clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Extension copyWith(void Function(Extension) updates) =>
      super.copyWith((message) => updates(message as Extension)) as Extension;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Extension() / Extension.new instead')
  static Extension create() => Extension._();
  static $pb.GeneratedMessage $_createMessage() => Extension._();
  @$core.override
  Extension createEmptyInstance() => Extension._();
  @$core.pragma('dart2js:noInline')
  static Extension getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Extension>(Extension.$_createMessage);
  static Extension? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.String get description => $_getSZ(2);
  @$pb.TagNumber(3)
  set description($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasDescription() => $_has(2);
  @$pb.TagNumber(3)
  void clearDescription() => $_clearField(3);

  /// Agents have it unless it's turned off for them.
  @$pb.TagNumber(4)
  $core.bool get defaultEnabled => $_getBF(3);
  @$pb.TagNumber(4)
  set defaultEnabled($core.bool value) => $_setBool(3, value);
  @$pb.TagNumber(4)
  $core.bool hasDefaultEnabled() => $_has(3);
  @$pb.TagNumber(4)
  void clearDefaultEnabled() => $_clearField(4);

  @$pb.TagNumber(5)
  $pb.PbList<ExtensionOption> get options => $_getList(4);

  /// A Lucide icon name, e.g. "users".
  @$pb.TagNumber(6)
  $core.String get icon => $_getSZ(5);
  @$pb.TagNumber(6)
  set icon($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasIcon() => $_has(5);
  @$pb.TagNumber(6)
  void clearIcon() => $_clearField(6);

  /// For an instance of an extension (a named second use of it, on or off on
  /// its own, such as "memory:factory", Factory's manual as a read-only
  /// memory): the extension it belongs to, e.g. "memory". Its `id` is
  /// "<base>:<name>" and works wherever an extension id does: AgentExtension,
  /// ExtensionChange, OpenPanel, Agent.open_panel and ExtensionView. Empty for
  /// an extension itself. Instances have no options and come right after
  /// their extension.
  @$pb.TagNumber(7)
  $core.String get base => $_getSZ(6);
  @$pb.TagNumber(7)
  set base($core.String value) => $_setString(6, value);
  @$pb.TagNumber(7)
  $core.bool hasBase() => $_has(6);
  @$pb.TagNumber(7)
  void clearBase() => $_clearField(7);

  /// Admin agents (Messaging's `admin` option, such as Rivet) have it unless
  /// it's turned off for them; other agents only once it's turned on. Set for
  /// instances and for extensions alike (Resources is one).
  @$pb.TagNumber(8)
  $core.bool get adminDefault => $_getBF(7);
  @$pb.TagNumber(8)
  set adminDefault($core.bool value) => $_setBool(7, value);
  @$pb.TagNumber(8)
  $core.bool hasAdminDefault() => $_has(7);
  @$pb.TagNumber(8)
  void clearAdminDefault() => $_clearField(8);
}

class ExtensionOption extends $pb.GeneratedMessage {
  factory ExtensionOption({
    $core.String? id,
    $core.String? label,
    $core.String? description,
    $core.bool? defaultValue,
    ExtensionOptionKind? kind,
    $core.bool? includesProject,
  }) {
    final result = ExtensionOption._();
    if (id != null) result.id = id;
    if (label != null) result.label = label;
    if (description != null) result.description = description;
    if (defaultValue != null) result.defaultValue = defaultValue;
    if (kind != null) result.kind = kind;
    if (includesProject != null) result.includesProject = includesProject;
    return result;
  }

  ExtensionOption._();

  factory ExtensionOption.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ExtensionOption()..mergeFromBuffer(data, registry);
  factory ExtensionOption.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ExtensionOption()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ExtensionOption',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ExtensionOption.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'label')
    ..aOS(3, _omitFieldNames ? '' : 'description')
    ..aOB(4, _omitFieldNames ? '' : 'defaultValue')
    ..aE<ExtensionOptionKind>(5, _omitFieldNames ? '' : 'kind',
        enumValues: ExtensionOptionKind.values)
    ..aOB(6, _omitFieldNames ? '' : 'includesProject')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ExtensionOption clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ExtensionOption copyWith(void Function(ExtensionOption) updates) =>
      super.copyWith((message) => updates(message as ExtensionOption))
          as ExtensionOption;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ExtensionOption() / ExtensionOption.new instead')
  static ExtensionOption create() => ExtensionOption._();
  static $pb.GeneratedMessage $_createMessage() => ExtensionOption._();
  @$core.override
  ExtensionOption createEmptyInstance() => ExtensionOption._();
  @$core.pragma('dart2js:noInline')
  static ExtensionOption getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ExtensionOption>(
          ExtensionOption.$_createMessage);
  static ExtensionOption? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// e.g. "Other agents can message it".
  @$pb.TagNumber(2)
  $core.String get label => $_getSZ(1);
  @$pb.TagNumber(2)
  set label($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasLabel() => $_has(1);
  @$pb.TagNumber(2)
  void clearLabel() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.String get description => $_getSZ(2);
  @$pb.TagNumber(3)
  set description($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasDescription() => $_has(2);
  @$pb.TagNumber(3)
  void clearDescription() => $_clearField(3);

  /// A SWITCH's default.
  @$pb.TagNumber(4)
  $core.bool get defaultValue => $_getBF(3);
  @$pb.TagNumber(4)
  set defaultValue($core.bool value) => $_setBool(3, value);
  @$pb.TagNumber(4)
  $core.bool hasDefaultValue() => $_has(3);
  @$pb.TagNumber(4)
  void clearDefaultValue() => $_clearField(4);

  @$pb.TagNumber(5)
  ExtensionOptionKind get kind => $_getN(4);
  @$pb.TagNumber(5)
  set kind(ExtensionOptionKind value) => $_setField(5, value);
  @$pb.TagNumber(5)
  $core.bool hasKind() => $_has(4);
  @$pb.TagNumber(5)
  void clearKind() => $_clearField(5);

  /// AGENTS: the agent's own project's agents always count, whether listed or
  /// not; show them checked and greyed, as "in its project".
  @$pb.TagNumber(6)
  $core.bool get includesProject => $_getBF(5);
  @$pb.TagNumber(6)
  set includesProject($core.bool value) => $_setBool(5, value);
  @$pb.TagNumber(6)
  $core.bool hasIncludesProject() => $_has(5);
  @$pb.TagNumber(6)
  void clearIncludesProject() => $_clearField(6);
}

class OptionValue extends $pb.GeneratedMessage {
  factory OptionValue({
    $core.bool? on,
    $core.Iterable<$core.String>? agentIds,
  }) {
    final result = OptionValue._();
    if (on != null) result.on = on;
    if (agentIds != null) result.agentIds.addAll(agentIds);
    return result;
  }

  OptionValue._();

  factory OptionValue.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      OptionValue()..mergeFromBuffer(data, registry);
  factory OptionValue.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      OptionValue()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'OptionValue',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: OptionValue.$_createMessage)
    ..aOB(1, _omitFieldNames ? '' : 'on')
    ..pPS(2, _omitFieldNames ? '' : 'agentIds')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  OptionValue clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  OptionValue copyWith(void Function(OptionValue) updates) =>
      super.copyWith((message) => updates(message as OptionValue))
          as OptionValue;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use OptionValue() / OptionValue.new instead')
  static OptionValue create() => OptionValue._();
  static $pb.GeneratedMessage $_createMessage() => OptionValue._();
  @$core.override
  OptionValue createEmptyInstance() => OptionValue._();
  @$core.pragma('dart2js:noInline')
  static OptionValue getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<OptionValue>(
          OptionValue.$_createMessage);
  static OptionValue? _defaultInstance;

  /// For a SWITCH.
  @$pb.TagNumber(1)
  $core.bool get on => $_getBF(0);
  @$pb.TagNumber(1)
  set on($core.bool value) => $_setBool(0, value);
  @$pb.TagNumber(1)
  $core.bool hasOn() => $_has(0);
  @$pb.TagNumber(1)
  void clearOn() => $_clearField(1);

  /// For an AGENTS list: agent ids.
  @$pb.TagNumber(2)
  $pb.PbList<$core.String> get agentIds => $_getList(1);
}

/// An extension as one agent has it, with its options' effective values.
class AgentExtension extends $pb.GeneratedMessage {
  factory AgentExtension({
    $core.String? id,
    $core.bool? enabled,
    $core.Iterable<$core.MapEntry<$core.String, OptionValue>>? options,
  }) {
    final result = AgentExtension._();
    if (id != null) result.id = id;
    if (enabled != null) result.enabled = enabled;
    if (options != null) result.options.addEntries(options);
    return result;
  }

  AgentExtension._();

  factory AgentExtension.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AgentExtension()..mergeFromBuffer(data, registry);
  factory AgentExtension.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AgentExtension()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'AgentExtension',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: AgentExtension.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOB(2, _omitFieldNames ? '' : 'enabled')
    ..m<$core.String, OptionValue>(3, _omitFieldNames ? '' : 'options',
        entryClassName: 'AgentExtension.OptionsEntry',
        keyFieldType: $pb.PbFieldType.OS,
        valueFieldType: $pb.PbFieldType.OM,
        valueCreator: OptionValue.$_createMessage,
        valueDefaultOrMaker: OptionValue.getDefault,
        packageName: const $pb.PackageName('factory.v1'))
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AgentExtension clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AgentExtension copyWith(void Function(AgentExtension) updates) =>
      super.copyWith((message) => updates(message as AgentExtension))
          as AgentExtension;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use AgentExtension() / AgentExtension.new instead')
  static AgentExtension create() => AgentExtension._();
  static $pb.GeneratedMessage $_createMessage() => AgentExtension._();
  @$core.override
  AgentExtension createEmptyInstance() => AgentExtension._();
  @$core.pragma('dart2js:noInline')
  static AgentExtension getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<AgentExtension>(
          AgentExtension.$_createMessage);
  static AgentExtension? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.bool get enabled => $_getBF(1);
  @$pb.TagNumber(2)
  set enabled($core.bool value) => $_setBool(1, value);
  @$pb.TagNumber(2)
  $core.bool hasEnabled() => $_has(1);
  @$pb.TagNumber(2)
  void clearEnabled() => $_clearField(2);

  /// Every option of the extension, by option id.
  @$pb.TagNumber(3)
  $pb.PbMap<$core.String, OptionValue> get options => $_getMap(2);
}

enum ExtensionView_Body {
  checklist,
  memory,
  messaging,
  tasks,
  monitors,
  database,
  resources,
  notSet
}

/// What an extension's panel shows for an agent. Sent when the selected agent
/// has the extension in `Workspace.open_panel` enabled, and again whenever it
/// changes; a client shows it only while it matches the open panel and the
/// selected agent.
class ExtensionView extends $pb.GeneratedMessage {
  factory ExtensionView({
    $core.String? extensionId,
    $core.String? agentId,
    $core.String? title,
    Checklist? checklist,
    Memory? memory,
    Messaging? messaging,
    Tasks? tasks,
    Monitors? monitors,
    Database? database,
    Resources? resources,
  }) {
    final result = ExtensionView._();
    if (extensionId != null) result.extensionId = extensionId;
    if (agentId != null) result.agentId = agentId;
    if (title != null) result.title = title;
    if (checklist != null) result.checklist = checklist;
    if (memory != null) result.memory = memory;
    if (messaging != null) result.messaging = messaging;
    if (tasks != null) result.tasks = tasks;
    if (monitors != null) result.monitors = monitors;
    if (database != null) result.database = database;
    if (resources != null) result.resources = resources;
    return result;
  }

  ExtensionView._();

  factory ExtensionView.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ExtensionView()..mergeFromBuffer(data, registry);
  factory ExtensionView.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ExtensionView()..mergeFromJson(json, registry);

  static const $core.Map<$core.int, ExtensionView_Body>
      _ExtensionView_BodyByTag = {
    10: ExtensionView_Body.checklist,
    11: ExtensionView_Body.memory,
    12: ExtensionView_Body.messaging,
    13: ExtensionView_Body.tasks,
    14: ExtensionView_Body.monitors,
    15: ExtensionView_Body.database,
    16: ExtensionView_Body.resources,
    0: ExtensionView_Body.notSet
  };
  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ExtensionView',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ExtensionView.$_createMessage)
    ..oo(0, [10, 11, 12, 13, 14, 15, 16])
    ..aOS(1, _omitFieldNames ? '' : 'extensionId')
    ..aOS(2, _omitFieldNames ? '' : 'agentId')
    ..aOS(3, _omitFieldNames ? '' : 'title')
    ..aOM<Checklist>(10, _omitFieldNames ? '' : 'checklist',
        subBuilder: Checklist.$_createMessage)
    ..aOM<Memory>(11, _omitFieldNames ? '' : 'memory',
        subBuilder: Memory.$_createMessage)
    ..aOM<Messaging>(12, _omitFieldNames ? '' : 'messaging',
        subBuilder: Messaging.$_createMessage)
    ..aOM<Tasks>(13, _omitFieldNames ? '' : 'tasks',
        subBuilder: Tasks.$_createMessage)
    ..aOM<Monitors>(14, _omitFieldNames ? '' : 'monitors',
        subBuilder: Monitors.$_createMessage)
    ..aOM<Database>(15, _omitFieldNames ? '' : 'database',
        subBuilder: Database.$_createMessage)
    ..aOM<Resources>(16, _omitFieldNames ? '' : 'resources',
        subBuilder: Resources.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ExtensionView clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ExtensionView copyWith(void Function(ExtensionView) updates) =>
      super.copyWith((message) => updates(message as ExtensionView))
          as ExtensionView;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ExtensionView() / ExtensionView.new instead')
  static ExtensionView create() => ExtensionView._();
  static $pb.GeneratedMessage $_createMessage() => ExtensionView._();
  @$core.override
  ExtensionView createEmptyInstance() => ExtensionView._();
  @$core.pragma('dart2js:noInline')
  static ExtensionView getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ExtensionView>(
          ExtensionView.$_createMessage);
  static ExtensionView? _defaultInstance;

  @$pb.TagNumber(10)
  @$pb.TagNumber(11)
  @$pb.TagNumber(12)
  @$pb.TagNumber(13)
  @$pb.TagNumber(14)
  @$pb.TagNumber(15)
  @$pb.TagNumber(16)
  ExtensionView_Body whichBody() => _ExtensionView_BodyByTag[$_whichOneof(0)]!;
  @$pb.TagNumber(10)
  @$pb.TagNumber(11)
  @$pb.TagNumber(12)
  @$pb.TagNumber(13)
  @$pb.TagNumber(14)
  @$pb.TagNumber(15)
  @$pb.TagNumber(16)
  void clearBody() => $_clearField($_whichOneof(0));

  @$pb.TagNumber(1)
  $core.String get extensionId => $_getSZ(0);
  @$pb.TagNumber(1)
  set extensionId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasExtensionId() => $_has(0);
  @$pb.TagNumber(1)
  void clearExtensionId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get agentId => $_getSZ(1);
  @$pb.TagNumber(2)
  set agentId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasAgentId() => $_has(1);
  @$pb.TagNumber(2)
  void clearAgentId() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.String get title => $_getSZ(2);
  @$pb.TagNumber(3)
  set title($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasTitle() => $_has(2);
  @$pb.TagNumber(3)
  void clearTitle() => $_clearField(3);

  @$pb.TagNumber(10)
  Checklist get checklist => $_getN(3);
  @$pb.TagNumber(10)
  set checklist(Checklist value) => $_setField(10, value);
  @$pb.TagNumber(10)
  $core.bool hasChecklist() => $_has(3);
  @$pb.TagNumber(10)
  void clearChecklist() => $_clearField(10);
  @$pb.TagNumber(10)
  Checklist ensureChecklist() => $_ensure(3);

  @$pb.TagNumber(11)
  Memory get memory => $_getN(4);
  @$pb.TagNumber(11)
  set memory(Memory value) => $_setField(11, value);
  @$pb.TagNumber(11)
  $core.bool hasMemory() => $_has(4);
  @$pb.TagNumber(11)
  void clearMemory() => $_clearField(11);
  @$pb.TagNumber(11)
  Memory ensureMemory() => $_ensure(4);

  @$pb.TagNumber(12)
  Messaging get messaging => $_getN(5);
  @$pb.TagNumber(12)
  set messaging(Messaging value) => $_setField(12, value);
  @$pb.TagNumber(12)
  $core.bool hasMessaging() => $_has(5);
  @$pb.TagNumber(12)
  void clearMessaging() => $_clearField(12);
  @$pb.TagNumber(12)
  Messaging ensureMessaging() => $_ensure(5);

  @$pb.TagNumber(13)
  Tasks get tasks => $_getN(6);
  @$pb.TagNumber(13)
  set tasks(Tasks value) => $_setField(13, value);
  @$pb.TagNumber(13)
  $core.bool hasTasks() => $_has(6);
  @$pb.TagNumber(13)
  void clearTasks() => $_clearField(13);
  @$pb.TagNumber(13)
  Tasks ensureTasks() => $_ensure(6);

  @$pb.TagNumber(14)
  Monitors get monitors => $_getN(7);
  @$pb.TagNumber(14)
  set monitors(Monitors value) => $_setField(14, value);
  @$pb.TagNumber(14)
  $core.bool hasMonitors() => $_has(7);
  @$pb.TagNumber(14)
  void clearMonitors() => $_clearField(14);
  @$pb.TagNumber(14)
  Monitors ensureMonitors() => $_ensure(7);

  @$pb.TagNumber(15)
  Database get database => $_getN(8);
  @$pb.TagNumber(15)
  set database(Database value) => $_setField(15, value);
  @$pb.TagNumber(15)
  $core.bool hasDatabase() => $_has(8);
  @$pb.TagNumber(15)
  void clearDatabase() => $_clearField(15);
  @$pb.TagNumber(15)
  Database ensureDatabase() => $_ensure(8);

  @$pb.TagNumber(16)
  Resources get resources => $_getN(9);
  @$pb.TagNumber(16)
  set resources(Resources value) => $_setField(16, value);
  @$pb.TagNumber(16)
  $core.bool hasResources() => $_has(9);
  @$pb.TagNumber(16)
  void clearResources() => $_clearField(16);
  @$pb.TagNumber(16)
  Resources ensureResources() => $_ensure(9);
}

/// A database's tables, read only: the agent's own (extension "database"),
/// or Factory's records (instance "database:factory").
class Database extends $pb.GeneratedMessage {
  factory Database({
    $core.Iterable<DatabaseTable>? tables,
    $core.String? path,
  }) {
    final result = Database._();
    if (tables != null) result.tables.addAll(tables);
    if (path != null) result.path = path;
    return result;
  }

  Database._();

  factory Database.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Database()..mergeFromBuffer(data, registry);
  factory Database.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Database()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Database',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Database.$_createMessage)
    ..pPM<DatabaseTable>(1, _omitFieldNames ? '' : 'tables',
        subBuilder: DatabaseTable.$_createMessage)
    ..aOS(2, _omitFieldNames ? '' : 'path')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Database clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Database copyWith(void Function(Database) updates) =>
      super.copyWith((message) => updates(message as Database)) as Database;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Database() / Database.new instead')
  static Database create() => Database._();
  static $pb.GeneratedMessage $_createMessage() => Database._();
  @$core.override
  Database createEmptyInstance() => Database._();
  @$core.pragma('dart2js:noInline')
  static Database getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Database>(Database.$_createMessage);
  static Database? _defaultInstance;

  /// By name; empty when it has none yet.
  @$pb.TagNumber(1)
  $pb.PbList<DatabaseTable> get tables => $_getList(0);

  /// Where the file is, for the person who wants to open it elsewhere.
  @$pb.TagNumber(2)
  $core.String get path => $_getSZ(1);
  @$pb.TagNumber(2)
  set path($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasPath() => $_has(1);
  @$pb.TagNumber(2)
  void clearPath() => $_clearField(2);
}

class DatabaseTable extends $pb.GeneratedMessage {
  factory DatabaseTable({
    $core.String? name,
    $core.bool? view,
    $fixnum.Int64? rows,
    $core.Iterable<DatabaseColumn>? columns,
  }) {
    final result = DatabaseTable._();
    if (name != null) result.name = name;
    if (view != null) result.view = view;
    if (rows != null) result.rows = rows;
    if (columns != null) result.columns.addAll(columns);
    return result;
  }

  DatabaseTable._();

  factory DatabaseTable.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DatabaseTable()..mergeFromBuffer(data, registry);
  factory DatabaseTable.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DatabaseTable()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'DatabaseTable',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: DatabaseTable.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'name')
    ..aOB(2, _omitFieldNames ? '' : 'view')
    ..a<$fixnum.Int64>(3, _omitFieldNames ? '' : 'rows', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..pPM<DatabaseColumn>(4, _omitFieldNames ? '' : 'columns',
        subBuilder: DatabaseColumn.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DatabaseTable clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DatabaseTable copyWith(void Function(DatabaseTable) updates) =>
      super.copyWith((message) => updates(message as DatabaseTable))
          as DatabaseTable;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use DatabaseTable() / DatabaseTable.new instead')
  static DatabaseTable create() => DatabaseTable._();
  static $pb.GeneratedMessage $_createMessage() => DatabaseTable._();
  @$core.override
  DatabaseTable createEmptyInstance() => DatabaseTable._();
  @$core.pragma('dart2js:noInline')
  static DatabaseTable getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<DatabaseTable>(
          DatabaseTable.$_createMessage);
  static DatabaseTable? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get name => $_getSZ(0);
  @$pb.TagNumber(1)
  set name($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasName() => $_has(0);
  @$pb.TagNumber(1)
  void clearName() => $_clearField(1);

  /// A view rather than a table.
  @$pb.TagNumber(2)
  $core.bool get view => $_getBF(1);
  @$pb.TagNumber(2)
  set view($core.bool value) => $_setBool(1, value);
  @$pb.TagNumber(2)
  $core.bool hasView() => $_has(1);
  @$pb.TagNumber(2)
  void clearView() => $_clearField(2);

  @$pb.TagNumber(3)
  $fixnum.Int64 get rows => $_getI64(2);
  @$pb.TagNumber(3)
  set rows($fixnum.Int64 value) => $_setInt64(2, value);
  @$pb.TagNumber(3)
  $core.bool hasRows() => $_has(2);
  @$pb.TagNumber(3)
  void clearRows() => $_clearField(3);

  @$pb.TagNumber(4)
  $pb.PbList<DatabaseColumn> get columns => $_getList(3);
}

/// How the machine and Factory use its resources (the Resources extension),
/// sampled every minute; disk use is measured every 20 minutes.
class Resources extends $pb.GeneratedMessage {
  factory Resources({
    Machine? machine,
    $core.Iterable<ResourceProcess>? factory,
    $core.Iterable<ResourceProcess>? others,
    $core.Iterable<DiskUsage>? disk,
    $core.Iterable<ResourceSample>? history,
    $fixnum.Int64? sampled,
    $fixnum.Int64? measured,
    $core.Iterable<$core.String>? alerts,
  }) {
    final result = Resources._();
    if (machine != null) result.machine = machine;
    if (factory != null) result.factory.addAll(factory);
    if (others != null) result.others.addAll(others);
    if (disk != null) result.disk.addAll(disk);
    if (history != null) result.history.addAll(history);
    if (sampled != null) result.sampled = sampled;
    if (measured != null) result.measured = measured;
    if (alerts != null) result.alerts.addAll(alerts);
    return result;
  }

  Resources._();

  factory Resources.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Resources()..mergeFromBuffer(data, registry);
  factory Resources.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Resources()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Resources',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Resources.$_createMessage)
    ..aOM<Machine>(1, _omitFieldNames ? '' : 'machine',
        subBuilder: Machine.$_createMessage)
    ..pPM<ResourceProcess>(2, _omitFieldNames ? '' : 'factory',
        subBuilder: ResourceProcess.$_createMessage)
    ..pPM<ResourceProcess>(3, _omitFieldNames ? '' : 'others',
        subBuilder: ResourceProcess.$_createMessage)
    ..pPM<DiskUsage>(4, _omitFieldNames ? '' : 'disk',
        subBuilder: DiskUsage.$_createMessage)
    ..pPM<ResourceSample>(5, _omitFieldNames ? '' : 'history',
        subBuilder: ResourceSample.$_createMessage)
    ..aInt64(6, _omitFieldNames ? '' : 'sampled')
    ..aInt64(7, _omitFieldNames ? '' : 'measured')
    ..pPS(8, _omitFieldNames ? '' : 'alerts')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Resources clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Resources copyWith(void Function(Resources) updates) =>
      super.copyWith((message) => updates(message as Resources)) as Resources;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Resources() / Resources.new instead')
  static Resources create() => Resources._();
  static $pb.GeneratedMessage $_createMessage() => Resources._();
  @$core.override
  Resources createEmptyInstance() => Resources._();
  @$core.pragma('dart2js:noInline')
  static Resources getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Resources>(Resources.$_createMessage);
  static Resources? _defaultInstance;

  @$pb.TagNumber(1)
  Machine get machine => $_getN(0);
  @$pb.TagNumber(1)
  set machine(Machine value) => $_setField(1, value);
  @$pb.TagNumber(1)
  $core.bool hasMachine() => $_has(0);
  @$pb.TagNumber(1)
  void clearMachine() => $_clearField(1);
  @$pb.TagNumber(1)
  Machine ensureMachine() => $_ensure(0);

  /// Factory's processes, biggest first: its backend, supervisor and app, and
  /// agents' background tasks, monitors' watches and commands running now.
  @$pb.TagNumber(2)
  $pb.PbList<ResourceProcess> get factory => $_getList(1);

  /// The machine's other processes using the most memory.
  @$pb.TagNumber(3)
  $pb.PbList<ResourceProcess> get others => $_getList(2);

  /// What Factory's data takes (its home folder, then by part), then each
  /// project's folders.
  @$pb.TagNumber(4)
  $pb.PbList<DiskUsage> get disk => $_getList(3);

  /// The last hour's samples, oldest first.
  @$pb.TagNumber(5)
  $pb.PbList<ResourceSample> get history => $_getList(4);

  /// When `machine` and the processes were sampled, and when the disk was
  /// last measured (0 until it has been), in ms since 1970.
  @$pb.TagNumber(6)
  $fixnum.Int64 get sampled => $_getI64(5);
  @$pb.TagNumber(6)
  set sampled($fixnum.Int64 value) => $_setInt64(5, value);
  @$pb.TagNumber(6)
  $core.bool hasSampled() => $_has(5);
  @$pb.TagNumber(6)
  void clearSampled() => $_clearField(6);

  @$pb.TagNumber(7)
  $fixnum.Int64 get measured => $_getI64(6);
  @$pb.TagNumber(7)
  set measured($fixnum.Int64 value) => $_setInt64(6, value);
  @$pb.TagNumber(7)
  $core.bool hasMeasured() => $_has(6);
  @$pb.TagNumber(7)
  void clearMeasured() => $_clearField(7);

  /// The alerts in effect now, as the agents were told them.
  @$pb.TagNumber(8)
  $pb.PbList<$core.String> get alerts => $_getList(7);
}

class Machine extends $pb.GeneratedMessage {
  factory Machine({
    $core.double? cpuPercent,
    $core.int? cores,
    $fixnum.Int64? memoryTotal,
    $fixnum.Int64? memoryUsed,
    $fixnum.Int64? swapTotal,
    $fixnum.Int64? swapUsed,
    $core.Iterable<Volume>? volumes,
  }) {
    final result = Machine._();
    if (cpuPercent != null) result.cpuPercent = cpuPercent;
    if (cores != null) result.cores = cores;
    if (memoryTotal != null) result.memoryTotal = memoryTotal;
    if (memoryUsed != null) result.memoryUsed = memoryUsed;
    if (swapTotal != null) result.swapTotal = swapTotal;
    if (swapUsed != null) result.swapUsed = swapUsed;
    if (volumes != null) result.volumes.addAll(volumes);
    return result;
  }

  Machine._();

  factory Machine.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Machine()..mergeFromBuffer(data, registry);
  factory Machine.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Machine()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Machine',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Machine.$_createMessage)
    ..aD(1, _omitFieldNames ? '' : 'cpuPercent')
    ..aI(2, _omitFieldNames ? '' : 'cores', fieldType: $pb.PbFieldType.OU3)
    ..a<$fixnum.Int64>(
        3, _omitFieldNames ? '' : 'memoryTotal', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..a<$fixnum.Int64>(
        4, _omitFieldNames ? '' : 'memoryUsed', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..a<$fixnum.Int64>(
        5, _omitFieldNames ? '' : 'swapTotal', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..a<$fixnum.Int64>(
        6, _omitFieldNames ? '' : 'swapUsed', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..pPM<Volume>(7, _omitFieldNames ? '' : 'volumes',
        subBuilder: Volume.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Machine clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Machine copyWith(void Function(Machine) updates) =>
      super.copyWith((message) => updates(message as Machine)) as Machine;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Machine() / Machine.new instead')
  static Machine create() => Machine._();
  static $pb.GeneratedMessage $_createMessage() => Machine._();
  @$core.override
  Machine createEmptyInstance() => Machine._();
  @$core.pragma('dart2js:noInline')
  static Machine getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Machine>(Machine.$_createMessage);
  static Machine? _defaultInstance;

  /// How busy all its processors are, 0 to 100.
  @$pb.TagNumber(1)
  $core.double get cpuPercent => $_getN(0);
  @$pb.TagNumber(1)
  set cpuPercent($core.double value) => $_setDouble(0, value);
  @$pb.TagNumber(1)
  $core.bool hasCpuPercent() => $_has(0);
  @$pb.TagNumber(1)
  void clearCpuPercent() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.int get cores => $_getIZ(1);
  @$pb.TagNumber(2)
  set cores($core.int value) => $_setUnsignedInt32(1, value);
  @$pb.TagNumber(2)
  $core.bool hasCores() => $_has(1);
  @$pb.TagNumber(2)
  void clearCores() => $_clearField(2);

  /// In bytes.
  @$pb.TagNumber(3)
  $fixnum.Int64 get memoryTotal => $_getI64(2);
  @$pb.TagNumber(3)
  set memoryTotal($fixnum.Int64 value) => $_setInt64(2, value);
  @$pb.TagNumber(3)
  $core.bool hasMemoryTotal() => $_has(2);
  @$pb.TagNumber(3)
  void clearMemoryTotal() => $_clearField(3);

  @$pb.TagNumber(4)
  $fixnum.Int64 get memoryUsed => $_getI64(3);
  @$pb.TagNumber(4)
  set memoryUsed($fixnum.Int64 value) => $_setInt64(3, value);
  @$pb.TagNumber(4)
  $core.bool hasMemoryUsed() => $_has(3);
  @$pb.TagNumber(4)
  void clearMemoryUsed() => $_clearField(4);

  @$pb.TagNumber(5)
  $fixnum.Int64 get swapTotal => $_getI64(4);
  @$pb.TagNumber(5)
  set swapTotal($fixnum.Int64 value) => $_setInt64(4, value);
  @$pb.TagNumber(5)
  $core.bool hasSwapTotal() => $_has(4);
  @$pb.TagNumber(5)
  void clearSwapTotal() => $_clearField(5);

  @$pb.TagNumber(6)
  $fixnum.Int64 get swapUsed => $_getI64(5);
  @$pb.TagNumber(6)
  set swapUsed($fixnum.Int64 value) => $_setInt64(5, value);
  @$pb.TagNumber(6)
  $core.bool hasSwapUsed() => $_has(5);
  @$pb.TagNumber(6)
  void clearSwapUsed() => $_clearField(6);

  /// The volumes Factory uses.
  @$pb.TagNumber(7)
  $pb.PbList<Volume> get volumes => $_getList(6);
}

class Volume extends $pb.GeneratedMessage {
  factory Volume({
    $core.String? mount,
    $fixnum.Int64? total,
    $fixnum.Int64? available,
    $core.Iterable<$core.String>? uses,
  }) {
    final result = Volume._();
    if (mount != null) result.mount = mount;
    if (total != null) result.total = total;
    if (available != null) result.available = available;
    if (uses != null) result.uses.addAll(uses);
    return result;
  }

  Volume._();

  factory Volume.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Volume()..mergeFromBuffer(data, registry);
  factory Volume.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Volume()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Volume',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Volume.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'mount')
    ..a<$fixnum.Int64>(2, _omitFieldNames ? '' : 'total', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..a<$fixnum.Int64>(
        3, _omitFieldNames ? '' : 'available', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..pPS(4, _omitFieldNames ? '' : 'uses')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Volume clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Volume copyWith(void Function(Volume) updates) =>
      super.copyWith((message) => updates(message as Volume)) as Volume;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Volume() / Volume.new instead')
  static Volume create() => Volume._();
  static $pb.GeneratedMessage $_createMessage() => Volume._();
  @$core.override
  Volume createEmptyInstance() => Volume._();
  @$core.pragma('dart2js:noInline')
  static Volume getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Volume>(Volume.$_createMessage);
  static Volume? _defaultInstance;

  /// Where it's mounted, e.g. "/" or "/Volumes/Work".
  @$pb.TagNumber(1)
  $core.String get mount => $_getSZ(0);
  @$pb.TagNumber(1)
  set mount($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasMount() => $_has(0);
  @$pb.TagNumber(1)
  void clearMount() => $_clearField(1);

  /// In bytes.
  @$pb.TagNumber(2)
  $fixnum.Int64 get total => $_getI64(1);
  @$pb.TagNumber(2)
  set total($fixnum.Int64 value) => $_setInt64(1, value);
  @$pb.TagNumber(2)
  $core.bool hasTotal() => $_has(1);
  @$pb.TagNumber(2)
  void clearTotal() => $_clearField(2);

  @$pb.TagNumber(3)
  $fixnum.Int64 get available => $_getI64(2);
  @$pb.TagNumber(3)
  set available($fixnum.Int64 value) => $_setInt64(2, value);
  @$pb.TagNumber(3)
  $core.bool hasAvailable() => $_has(2);
  @$pb.TagNumber(3)
  void clearAvailable() => $_clearField(3);

  /// What of Factory's is on it: "Factory's data", then project names.
  @$pb.TagNumber(4)
  $pb.PbList<$core.String> get uses => $_getList(3);
}

/// A process of Factory's (with what it started) or another one.
class ResourceProcess extends $pb.GeneratedMessage {
  factory ResourceProcess({
    $core.String? label,
    $core.String? kind,
    $core.String? agentId,
    $core.int? pid,
    $core.int? processes,
    $core.double? cpuPercent,
    $fixnum.Int64? memory,
  }) {
    final result = ResourceProcess._();
    if (label != null) result.label = label;
    if (kind != null) result.kind = kind;
    if (agentId != null) result.agentId = agentId;
    if (pid != null) result.pid = pid;
    if (processes != null) result.processes = processes;
    if (cpuPercent != null) result.cpuPercent = cpuPercent;
    if (memory != null) result.memory = memory;
    return result;
  }

  ResourceProcess._();

  factory ResourceProcess.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ResourceProcess()..mergeFromBuffer(data, registry);
  factory ResourceProcess.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ResourceProcess()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ResourceProcess',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ResourceProcess.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'label')
    ..aOS(2, _omitFieldNames ? '' : 'kind')
    ..aOS(3, _omitFieldNames ? '' : 'agentId')
    ..aI(4, _omitFieldNames ? '' : 'pid', fieldType: $pb.PbFieldType.OU3)
    ..aI(5, _omitFieldNames ? '' : 'processes', fieldType: $pb.PbFieldType.OU3)
    ..aD(6, _omitFieldNames ? '' : 'cpuPercent')
    ..a<$fixnum.Int64>(7, _omitFieldNames ? '' : 'memory', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ResourceProcess clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ResourceProcess copyWith(void Function(ResourceProcess) updates) =>
      super.copyWith((message) => updates(message as ResourceProcess))
          as ResourceProcess;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ResourceProcess() / ResourceProcess.new instead')
  static ResourceProcess create() => ResourceProcess._();
  static $pb.GeneratedMessage $_createMessage() => ResourceProcess._();
  @$core.override
  ResourceProcess createEmptyInstance() => ResourceProcess._();
  @$core.pragma('dart2js:noInline')
  static ResourceProcess getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ResourceProcess>(
          ResourceProcess.$_createMessage);
  static ResourceProcess? _defaultInstance;

  /// "Factory's backend", "Supervisor", "App", "task-3 npm run dev",
  /// "Monitor m2 tail -f app.log", "Commands running now", or another
  /// process's name, such as "Docker".
  @$pb.TagNumber(1)
  $core.String get label => $_getSZ(0);
  @$pb.TagNumber(1)
  set label($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasLabel() => $_has(0);
  @$pb.TagNumber(1)
  void clearLabel() => $_clearField(1);

  /// backend, supervisor, app, task, monitor, command or other.
  @$pb.TagNumber(2)
  $core.String get kind => $_getSZ(1);
  @$pb.TagNumber(2)
  set kind($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasKind() => $_has(1);
  @$pb.TagNumber(2)
  void clearKind() => $_clearField(2);

  /// The agent a task, monitor or command is of; empty otherwise.
  @$pb.TagNumber(3)
  $core.String get agentId => $_getSZ(2);
  @$pb.TagNumber(3)
  set agentId($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasAgentId() => $_has(2);
  @$pb.TagNumber(3)
  void clearAgentId() => $_clearField(3);

  /// Its main process (a task's or monitor's process group).
  @$pb.TagNumber(4)
  $core.int get pid => $_getIZ(3);
  @$pb.TagNumber(4)
  set pid($core.int value) => $_setUnsignedInt32(3, value);
  @$pb.TagNumber(4)
  $core.bool hasPid() => $_has(3);
  @$pb.TagNumber(4)
  void clearPid() => $_clearField(4);

  /// How many processes it counts.
  @$pb.TagNumber(5)
  $core.int get processes => $_getIZ(4);
  @$pb.TagNumber(5)
  set processes($core.int value) => $_setUnsignedInt32(4, value);
  @$pb.TagNumber(5)
  $core.bool hasProcesses() => $_has(4);
  @$pb.TagNumber(5)
  void clearProcesses() => $_clearField(5);

  /// Over the last minute; 100 is one processor busy.
  @$pb.TagNumber(6)
  $core.double get cpuPercent => $_getN(5);
  @$pb.TagNumber(6)
  set cpuPercent($core.double value) => $_setDouble(5, value);
  @$pb.TagNumber(6)
  $core.bool hasCpuPercent() => $_has(5);
  @$pb.TagNumber(6)
  void clearCpuPercent() => $_clearField(6);

  /// Resident memory, in bytes.
  @$pb.TagNumber(7)
  $fixnum.Int64 get memory => $_getI64(6);
  @$pb.TagNumber(7)
  set memory($fixnum.Int64 value) => $_setInt64(6, value);
  @$pb.TagNumber(7)
  $core.bool hasMemory() => $_has(6);
  @$pb.TagNumber(7)
  void clearMemory() => $_clearField(7);
}

class DiskUsage extends $pb.GeneratedMessage {
  factory DiskUsage({
    $core.String? label,
    $core.String? kind,
    $core.String? id,
    $core.String? path,
    $fixnum.Int64? bytes,
    $core.Iterable<DiskUsage>? parts,
    $core.bool? partial,
  }) {
    final result = DiskUsage._();
    if (label != null) result.label = label;
    if (kind != null) result.kind = kind;
    if (id != null) result.id = id;
    if (path != null) result.path = path;
    if (bytes != null) result.bytes = bytes;
    if (parts != null) result.parts.addAll(parts);
    if (partial != null) result.partial = partial;
    return result;
  }

  DiskUsage._();

  factory DiskUsage.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DiskUsage()..mergeFromBuffer(data, registry);
  factory DiskUsage.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DiskUsage()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'DiskUsage',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: DiskUsage.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'label')
    ..aOS(2, _omitFieldNames ? '' : 'kind')
    ..aOS(3, _omitFieldNames ? '' : 'id')
    ..aOS(4, _omitFieldNames ? '' : 'path')
    ..a<$fixnum.Int64>(5, _omitFieldNames ? '' : 'bytes', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..pPM<DiskUsage>(6, _omitFieldNames ? '' : 'parts',
        subBuilder: DiskUsage.$_createMessage)
    ..aOB(7, _omitFieldNames ? '' : 'partial')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DiskUsage clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DiskUsage copyWith(void Function(DiskUsage) updates) =>
      super.copyWith((message) => updates(message as DiskUsage)) as DiskUsage;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use DiskUsage() / DiskUsage.new instead')
  static DiskUsage create() => DiskUsage._();
  static $pb.GeneratedMessage $_createMessage() => DiskUsage._();
  @$core.override
  DiskUsage createEmptyInstance() => DiskUsage._();
  @$core.pragma('dart2js:noInline')
  static DiskUsage getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<DiskUsage>(DiskUsage.$_createMessage);
  static DiskUsage? _defaultInstance;

  /// "Factory's data", "Releases", an agent's or project's name, a folder's.
  @$pb.TagNumber(1)
  $core.String get label => $_getSZ(0);
  @$pb.TagNumber(1)
  set label($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasLabel() => $_has(0);
  @$pb.TagNumber(1)
  void clearLabel() => $_clearField(1);

  /// home, releases, agent, project or folder; among an agent's, project's
  /// or folder's parts, "build_cache" for each build cache in it (its
  /// checkout's, or the folder's), which ClearBuildCache clears by `path`.
  @$pb.TagNumber(2)
  $core.String get kind => $_getSZ(1);
  @$pb.TagNumber(2)
  set kind($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasKind() => $_has(1);
  @$pb.TagNumber(2)
  void clearKind() => $_clearField(2);

  /// The agent's or project's id.
  @$pb.TagNumber(3)
  $core.String get id => $_getSZ(2);
  @$pb.TagNumber(3)
  set id($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasId() => $_has(2);
  @$pb.TagNumber(3)
  void clearId() => $_clearField(3);

  @$pb.TagNumber(4)
  $core.String get path => $_getSZ(3);
  @$pb.TagNumber(4)
  set path($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasPath() => $_has(3);
  @$pb.TagNumber(4)
  void clearPath() => $_clearField(4);

  /// In bytes. Checkouts are copy-on-write clones: their bytes are counted
  /// in full though they share most of them.
  @$pb.TagNumber(5)
  $fixnum.Int64 get bytes => $_getI64(4);
  @$pb.TagNumber(5)
  set bytes($fixnum.Int64 value) => $_setInt64(4, value);
  @$pb.TagNumber(5)
  $core.bool hasBytes() => $_has(4);
  @$pb.TagNumber(5)
  void clearBytes() => $_clearField(5);

  /// An agent's: its conversations, task logs, database and checkout, and its
  /// checkout's build caches; a project's or folder's: its build caches.
  @$pb.TagNumber(6)
  $pb.PbList<DiskUsage> get parts => $_getList(5);

  /// The walk took too long and stopped: `bytes` is at least this.
  @$pb.TagNumber(7)
  $core.bool get partial => $_getBF(6);
  @$pb.TagNumber(7)
  set partial($core.bool value) => $_setBool(6, value);
  @$pb.TagNumber(7)
  $core.bool hasPartial() => $_has(6);
  @$pb.TagNumber(7)
  void clearPartial() => $_clearField(7);
}

class ResourceSample extends $pb.GeneratedMessage {
  factory ResourceSample({
    $fixnum.Int64? time,
    $core.double? cpuPercent,
    $fixnum.Int64? memoryUsed,
    $fixnum.Int64? swapUsed,
    $fixnum.Int64? factoryMemory,
  }) {
    final result = ResourceSample._();
    if (time != null) result.time = time;
    if (cpuPercent != null) result.cpuPercent = cpuPercent;
    if (memoryUsed != null) result.memoryUsed = memoryUsed;
    if (swapUsed != null) result.swapUsed = swapUsed;
    if (factoryMemory != null) result.factoryMemory = factoryMemory;
    return result;
  }

  ResourceSample._();

  factory ResourceSample.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ResourceSample()..mergeFromBuffer(data, registry);
  factory ResourceSample.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ResourceSample()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ResourceSample',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ResourceSample.$_createMessage)
    ..aInt64(1, _omitFieldNames ? '' : 'time')
    ..aD(2, _omitFieldNames ? '' : 'cpuPercent')
    ..a<$fixnum.Int64>(
        3, _omitFieldNames ? '' : 'memoryUsed', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..a<$fixnum.Int64>(
        4, _omitFieldNames ? '' : 'swapUsed', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..a<$fixnum.Int64>(
        5, _omitFieldNames ? '' : 'factoryMemory', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ResourceSample clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ResourceSample copyWith(void Function(ResourceSample) updates) =>
      super.copyWith((message) => updates(message as ResourceSample))
          as ResourceSample;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ResourceSample() / ResourceSample.new instead')
  static ResourceSample create() => ResourceSample._();
  static $pb.GeneratedMessage $_createMessage() => ResourceSample._();
  @$core.override
  ResourceSample createEmptyInstance() => ResourceSample._();
  @$core.pragma('dart2js:noInline')
  static ResourceSample getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ResourceSample>(
          ResourceSample.$_createMessage);
  static ResourceSample? _defaultInstance;

  @$pb.TagNumber(1)
  $fixnum.Int64 get time => $_getI64(0);
  @$pb.TagNumber(1)
  set time($fixnum.Int64 value) => $_setInt64(0, value);
  @$pb.TagNumber(1)
  $core.bool hasTime() => $_has(0);
  @$pb.TagNumber(1)
  void clearTime() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.double get cpuPercent => $_getN(1);
  @$pb.TagNumber(2)
  set cpuPercent($core.double value) => $_setDouble(1, value);
  @$pb.TagNumber(2)
  $core.bool hasCpuPercent() => $_has(1);
  @$pb.TagNumber(2)
  void clearCpuPercent() => $_clearField(2);

  @$pb.TagNumber(3)
  $fixnum.Int64 get memoryUsed => $_getI64(2);
  @$pb.TagNumber(3)
  set memoryUsed($fixnum.Int64 value) => $_setInt64(2, value);
  @$pb.TagNumber(3)
  $core.bool hasMemoryUsed() => $_has(2);
  @$pb.TagNumber(3)
  void clearMemoryUsed() => $_clearField(3);

  @$pb.TagNumber(4)
  $fixnum.Int64 get swapUsed => $_getI64(3);
  @$pb.TagNumber(4)
  set swapUsed($fixnum.Int64 value) => $_setInt64(3, value);
  @$pb.TagNumber(4)
  $core.bool hasSwapUsed() => $_has(3);
  @$pb.TagNumber(4)
  void clearSwapUsed() => $_clearField(4);

  /// All of Factory's processes together.
  @$pb.TagNumber(5)
  $fixnum.Int64 get factoryMemory => $_getI64(4);
  @$pb.TagNumber(5)
  set factoryMemory($fixnum.Int64 value) => $_setInt64(4, value);
  @$pb.TagNumber(5)
  $core.bool hasFactoryMemory() => $_has(4);
  @$pb.TagNumber(5)
  void clearFactoryMemory() => $_clearField(5);
}

class DatabaseColumn extends $pb.GeneratedMessage {
  factory DatabaseColumn({
    $core.String? name,
    $core.String? type,
  }) {
    final result = DatabaseColumn._();
    if (name != null) result.name = name;
    if (type != null) result.type = type;
    return result;
  }

  DatabaseColumn._();

  factory DatabaseColumn.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DatabaseColumn()..mergeFromBuffer(data, registry);
  factory DatabaseColumn.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DatabaseColumn()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'DatabaseColumn',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: DatabaseColumn.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'name')
    ..aOS(2, _omitFieldNames ? '' : 'type')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DatabaseColumn clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DatabaseColumn copyWith(void Function(DatabaseColumn) updates) =>
      super.copyWith((message) => updates(message as DatabaseColumn))
          as DatabaseColumn;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use DatabaseColumn() / DatabaseColumn.new instead')
  static DatabaseColumn create() => DatabaseColumn._();
  static $pb.GeneratedMessage $_createMessage() => DatabaseColumn._();
  @$core.override
  DatabaseColumn createEmptyInstance() => DatabaseColumn._();
  @$core.pragma('dart2js:noInline')
  static DatabaseColumn getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<DatabaseColumn>(
          DatabaseColumn.$_createMessage);
  static DatabaseColumn? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get name => $_getSZ(0);
  @$pb.TagNumber(1)
  set name($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasName() => $_has(0);
  @$pb.TagNumber(1)
  void clearName() => $_clearField(1);

  /// As declared, e.g. "INTEGER"; may be empty.
  @$pb.TagNumber(2)
  $core.String get type => $_getSZ(1);
  @$pb.TagNumber(2)
  set type($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasType() => $_has(1);
  @$pb.TagNumber(2)
  void clearType() => $_clearField(2);
}

/// Whom an agent can message and who can message it, by the same rule as its
/// tools. Names, colors and status come from the workspace's agents.
class Messaging extends $pb.GeneratedMessage {
  factory Messaging({
    $core.Iterable<$core.String>? reaches,
    $core.Iterable<$core.String>? reachedBy,
  }) {
    final result = Messaging._();
    if (reaches != null) result.reaches.addAll(reaches);
    if (reachedBy != null) result.reachedBy.addAll(reachedBy);
    return result;
  }

  Messaging._();

  factory Messaging.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Messaging()..mergeFromBuffer(data, registry);
  factory Messaging.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Messaging()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Messaging',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Messaging.$_createMessage)
    ..pPS(1, _omitFieldNames ? '' : 'reaches')
    ..pPS(2, _omitFieldNames ? '' : 'reachedBy')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Messaging clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Messaging copyWith(void Function(Messaging) updates) =>
      super.copyWith((message) => updates(message as Messaging)) as Messaging;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Messaging() / Messaging.new instead')
  static Messaging create() => Messaging._();
  static $pb.GeneratedMessage $_createMessage() => Messaging._();
  @$core.override
  Messaging createEmptyInstance() => Messaging._();
  @$core.pragma('dart2js:noInline')
  static Messaging getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Messaging>(Messaging.$_createMessage);
  static Messaging? _defaultInstance;

  /// Agent ids it can message, in sidebar order.
  @$pb.TagNumber(1)
  $pb.PbList<$core.String> get reaches => $_getList(0);

  /// Agent ids that can message it, in sidebar order.
  @$pb.TagNumber(2)
  $pb.PbList<$core.String> get reachedBy => $_getList(1);
}

/// An agent's checklist: everything the person asked for, oldest first.
class Checklist extends $pb.GeneratedMessage {
  factory Checklist({
    $core.Iterable<ChecklistItem>? items,
  }) {
    final result = Checklist._();
    if (items != null) result.items.addAll(items);
    return result;
  }

  Checklist._();

  factory Checklist.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Checklist()..mergeFromBuffer(data, registry);
  factory Checklist.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Checklist()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Checklist',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Checklist.$_createMessage)
    ..pPM<ChecklistItem>(1, _omitFieldNames ? '' : 'items',
        subBuilder: ChecklistItem.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Checklist clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Checklist copyWith(void Function(Checklist) updates) =>
      super.copyWith((message) => updates(message as Checklist)) as Checklist;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Checklist() / Checklist.new instead')
  static Checklist create() => Checklist._();
  static $pb.GeneratedMessage $_createMessage() => Checklist._();
  @$core.override
  Checklist createEmptyInstance() => Checklist._();
  @$core.pragma('dart2js:noInline')
  static Checklist getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Checklist>(Checklist.$_createMessage);
  static Checklist? _defaultInstance;

  @$pb.TagNumber(1)
  $pb.PbList<ChecklistItem> get items => $_getList(0);
}

class ChecklistItem extends $pb.GeneratedMessage {
  factory ChecklistItem({
    $core.int? id,
    $core.String? text,
    ChecklistStatus? status,
    $core.String? owner,
    $fixnum.Int64? created,
    $fixnum.Int64? updated,
    $core.Iterable<ChecklistUpdate>? log,
    $core.String? note,
  }) {
    final result = ChecklistItem._();
    if (id != null) result.id = id;
    if (text != null) result.text = text;
    if (status != null) result.status = status;
    if (owner != null) result.owner = owner;
    if (created != null) result.created = created;
    if (updated != null) result.updated = updated;
    if (log != null) result.log.addAll(log);
    if (note != null) result.note = note;
    return result;
  }

  ChecklistItem._();

  factory ChecklistItem.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ChecklistItem()..mergeFromBuffer(data, registry);
  factory ChecklistItem.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ChecklistItem()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ChecklistItem',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ChecklistItem.$_createMessage)
    ..aI(1, _omitFieldNames ? '' : 'id', fieldType: $pb.PbFieldType.OU3)
    ..aOS(2, _omitFieldNames ? '' : 'text')
    ..aE<ChecklistStatus>(3, _omitFieldNames ? '' : 'status',
        enumValues: ChecklistStatus.values)
    ..aOS(4, _omitFieldNames ? '' : 'owner')
    ..aInt64(5, _omitFieldNames ? '' : 'created')
    ..aInt64(6, _omitFieldNames ? '' : 'updated')
    ..pPM<ChecklistUpdate>(7, _omitFieldNames ? '' : 'log',
        subBuilder: ChecklistUpdate.$_createMessage)
    ..aOS(8, _omitFieldNames ? '' : 'note')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChecklistItem clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChecklistItem copyWith(void Function(ChecklistItem) updates) =>
      super.copyWith((message) => updates(message as ChecklistItem))
          as ChecklistItem;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ChecklistItem() / ChecklistItem.new instead')
  static ChecklistItem create() => ChecklistItem._();
  static $pb.GeneratedMessage $_createMessage() => ChecklistItem._();
  @$core.override
  ChecklistItem createEmptyInstance() => ChecklistItem._();
  @$core.pragma('dart2js:noInline')
  static ChecklistItem getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ChecklistItem>(
          ChecklistItem.$_createMessage);
  static ChecklistItem? _defaultInstance;

  /// Small and per agent: #1, #2…
  @$pb.TagNumber(1)
  $core.int get id => $_getIZ(0);
  @$pb.TagNumber(1)
  set id($core.int value) => $_setUnsignedInt32(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get text => $_getSZ(1);
  @$pb.TagNumber(2)
  set text($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasText() => $_has(1);
  @$pb.TagNumber(2)
  void clearText() => $_clearField(2);

  @$pb.TagNumber(3)
  ChecklistStatus get status => $_getN(2);
  @$pb.TagNumber(3)
  set status(ChecklistStatus value) => $_setField(3, value);
  @$pb.TagNumber(3)
  $core.bool hasStatus() => $_has(2);
  @$pb.TagNumber(3)
  void clearStatus() => $_clearField(3);

  /// The agent id of who's on it; empty when unassigned.
  @$pb.TagNumber(4)
  $core.String get owner => $_getSZ(3);
  @$pb.TagNumber(4)
  set owner($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasOwner() => $_has(3);
  @$pb.TagNumber(4)
  void clearOwner() => $_clearField(4);

  /// When it was added and last changed, in ms since 1970.
  @$pb.TagNumber(5)
  $fixnum.Int64 get created => $_getI64(4);
  @$pb.TagNumber(5)
  set created($fixnum.Int64 value) => $_setInt64(4, value);
  @$pb.TagNumber(5)
  $core.bool hasCreated() => $_has(4);
  @$pb.TagNumber(5)
  void clearCreated() => $_clearField(5);

  @$pb.TagNumber(6)
  $fixnum.Int64 get updated => $_getI64(5);
  @$pb.TagNumber(6)
  set updated($fixnum.Int64 value) => $_setInt64(5, value);
  @$pb.TagNumber(6)
  $core.bool hasUpdated() => $_has(5);
  @$pb.TagNumber(6)
  void clearUpdated() => $_clearField(6);

  /// What happened to it, oldest first.
  @$pb.TagNumber(7)
  $pb.PbList<ChecklistUpdate> get log => $_getList(6);

  /// While it's blocked: the latest note made since it last became blocked
  /// (what it waits on), the note's own text. Empty otherwise.
  @$pb.TagNumber(8)
  $core.String get note => $_getSZ(7);
  @$pb.TagNumber(8)
  set note($core.String value) => $_setString(7, value);
  @$pb.TagNumber(8)
  $core.bool hasNote() => $_has(7);
  @$pb.TagNumber(8)
  void clearNote() => $_clearField(8);
}

class ChecklistUpdate extends $pb.GeneratedMessage {
  factory ChecklistUpdate({
    $fixnum.Int64? time,
    $core.String? text,
    $core.String? author,
  }) {
    final result = ChecklistUpdate._();
    if (time != null) result.time = time;
    if (text != null) result.text = text;
    if (author != null) result.author = author;
    return result;
  }

  ChecklistUpdate._();

  factory ChecklistUpdate.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ChecklistUpdate()..mergeFromBuffer(data, registry);
  factory ChecklistUpdate.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ChecklistUpdate()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ChecklistUpdate',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ChecklistUpdate.$_createMessage)
    ..aInt64(1, _omitFieldNames ? '' : 'time')
    ..aOS(2, _omitFieldNames ? '' : 'text')
    ..aOS(3, _omitFieldNames ? '' : 'author')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChecklistUpdate clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChecklistUpdate copyWith(void Function(ChecklistUpdate) updates) =>
      super.copyWith((message) => updates(message as ChecklistUpdate))
          as ChecklistUpdate;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ChecklistUpdate() / ChecklistUpdate.new instead')
  static ChecklistUpdate create() => ChecklistUpdate._();
  static $pb.GeneratedMessage $_createMessage() => ChecklistUpdate._();
  @$core.override
  ChecklistUpdate createEmptyInstance() => ChecklistUpdate._();
  @$core.pragma('dart2js:noInline')
  static ChecklistUpdate getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ChecklistUpdate>(
          ChecklistUpdate.$_createMessage);
  static ChecklistUpdate? _defaultInstance;

  /// ms since 1970.
  @$pb.TagNumber(1)
  $fixnum.Int64 get time => $_getI64(0);
  @$pb.TagNumber(1)
  set time($fixnum.Int64 value) => $_setInt64(0, value);
  @$pb.TagNumber(1)
  $core.bool hasTime() => $_has(0);
  @$pb.TagNumber(1)
  void clearTime() => $_clearField(1);

  /// e.g. "Added", "Done: shipped", "Owner: frontend", or a note.
  @$pb.TagNumber(2)
  $core.String get text => $_getSZ(1);
  @$pb.TagNumber(2)
  set text($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasText() => $_has(1);
  @$pb.TagNumber(2)
  void clearText() => $_clearField(2);

  /// The agent id of who made it; empty for the person.
  @$pb.TagNumber(3)
  $core.String get author => $_getSZ(2);
  @$pb.TagNumber(3)
  set author($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasAuthor() => $_has(2);
  @$pb.TagNumber(3)
  void clearAuthor() => $_clearField(3);
}

/// An agent's own memory: the person's preferences it learned and its
/// knowledge notes. Only that agent reads and writes it.
class Memory extends $pb.GeneratedMessage {
  factory Memory({
    $core.Iterable<MemoryPreference>? preferences,
    $core.Iterable<MemoryNote>? notes,
    $core.Iterable<MemoryChange>? changes,
    $core.bool? readOnly,
  }) {
    final result = Memory._();
    if (preferences != null) result.preferences.addAll(preferences);
    if (notes != null) result.notes.addAll(notes);
    if (changes != null) result.changes.addAll(changes);
    if (readOnly != null) result.readOnly = readOnly;
    return result;
  }

  Memory._();

  factory Memory.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Memory()..mergeFromBuffer(data, registry);
  factory Memory.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Memory()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Memory',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Memory.$_createMessage)
    ..pPM<MemoryPreference>(1, _omitFieldNames ? '' : 'preferences',
        subBuilder: MemoryPreference.$_createMessage)
    ..pPM<MemoryNote>(2, _omitFieldNames ? '' : 'notes',
        subBuilder: MemoryNote.$_createMessage)
    ..pPM<MemoryChange>(3, _omitFieldNames ? '' : 'changes',
        subBuilder: MemoryChange.$_createMessage)
    ..aOB(4, _omitFieldNames ? '' : 'readOnly')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Memory clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Memory copyWith(void Function(Memory) updates) =>
      super.copyWith((message) => updates(message as Memory)) as Memory;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Memory() / Memory.new instead')
  static Memory create() => Memory._();
  static $pb.GeneratedMessage $_createMessage() => Memory._();
  @$core.override
  Memory createEmptyInstance() => Memory._();
  @$core.pragma('dart2js:noInline')
  static Memory getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Memory>(Memory.$_createMessage);
  static Memory? _defaultInstance;

  /// By id.
  @$pb.TagNumber(1)
  $pb.PbList<MemoryPreference> get preferences => $_getList(0);

  /// By slug.
  @$pb.TagNumber(2)
  $pb.PbList<MemoryNote> get notes => $_getList(1);

  /// Newest first, the last 50.
  @$pb.TagNumber(3)
  $pb.PbList<MemoryChange> get changes => $_getList(2);

  /// Factory's manual (instance "memory:factory"): notes only, which are
  /// Factory's own docs; nothing is learned or changed, so there are no
  /// preferences or changes.
  @$pb.TagNumber(4)
  $core.bool get readOnly => $_getBF(3);
  @$pb.TagNumber(4)
  set readOnly($core.bool value) => $_setBool(3, value);
  @$pb.TagNumber(4)
  $core.bool hasReadOnly() => $_has(3);
  @$pb.TagNumber(4)
  void clearReadOnly() => $_clearField(4);
}

class MemoryPreference extends $pb.GeneratedMessage {
  factory MemoryPreference({
    $core.int? id,
    $core.String? rule,
    $core.String? why,
    $fixnum.Int64? learned,
    $fixnum.Int64? updated,
  }) {
    final result = MemoryPreference._();
    if (id != null) result.id = id;
    if (rule != null) result.rule = rule;
    if (why != null) result.why = why;
    if (learned != null) result.learned = learned;
    if (updated != null) result.updated = updated;
    return result;
  }

  MemoryPreference._();

  factory MemoryPreference.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MemoryPreference()..mergeFromBuffer(data, registry);
  factory MemoryPreference.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MemoryPreference()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'MemoryPreference',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: MemoryPreference.$_createMessage)
    ..aI(1, _omitFieldNames ? '' : 'id', fieldType: $pb.PbFieldType.OU3)
    ..aOS(2, _omitFieldNames ? '' : 'rule')
    ..aOS(3, _omitFieldNames ? '' : 'why')
    ..aInt64(4, _omitFieldNames ? '' : 'learned')
    ..aInt64(5, _omitFieldNames ? '' : 'updated')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MemoryPreference clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MemoryPreference copyWith(void Function(MemoryPreference) updates) =>
      super.copyWith((message) => updates(message as MemoryPreference))
          as MemoryPreference;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use MemoryPreference() / MemoryPreference.new instead')
  static MemoryPreference create() => MemoryPreference._();
  static $pb.GeneratedMessage $_createMessage() => MemoryPreference._();
  @$core.override
  MemoryPreference createEmptyInstance() => MemoryPreference._();
  @$core.pragma('dart2js:noInline')
  static MemoryPreference getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<MemoryPreference>(
          MemoryPreference.$_createMessage);
  static MemoryPreference? _defaultInstance;

  /// Small and per agent: #1, #2…; never reused.
  @$pb.TagNumber(1)
  $core.int get id => $_getIZ(0);
  @$pb.TagNumber(1)
  set id($core.int value) => $_setUnsignedInt32(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// What the person wants or doesn't, e.g. "Don't propose paid CI".
  @$pb.TagNumber(2)
  $core.String get rule => $_getSZ(1);
  @$pb.TagNumber(2)
  set rule($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasRule() => $_has(1);
  @$pb.TagNumber(2)
  void clearRule() => $_clearField(2);

  /// Why, e.g. "everything runs locally in Factory".
  @$pb.TagNumber(3)
  $core.String get why => $_getSZ(2);
  @$pb.TagNumber(3)
  set why($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasWhy() => $_has(2);
  @$pb.TagNumber(3)
  void clearWhy() => $_clearField(3);

  /// When it was added and last changed, in ms since 1970.
  @$pb.TagNumber(4)
  $fixnum.Int64 get learned => $_getI64(3);
  @$pb.TagNumber(4)
  set learned($fixnum.Int64 value) => $_setInt64(3, value);
  @$pb.TagNumber(4)
  $core.bool hasLearned() => $_has(3);
  @$pb.TagNumber(4)
  void clearLearned() => $_clearField(4);

  @$pb.TagNumber(5)
  $fixnum.Int64 get updated => $_getI64(4);
  @$pb.TagNumber(5)
  set updated($fixnum.Int64 value) => $_setInt64(4, value);
  @$pb.TagNumber(5)
  $core.bool hasUpdated() => $_has(4);
  @$pb.TagNumber(5)
  void clearUpdated() => $_clearField(5);
}

class MemoryNote extends $pb.GeneratedMessage {
  factory MemoryNote({
    $core.String? slug,
    $core.String? summary,
    $core.String? body,
    $fixnum.Int64? created,
    $fixnum.Int64? updated,
  }) {
    final result = MemoryNote._();
    if (slug != null) result.slug = slug;
    if (summary != null) result.summary = summary;
    if (body != null) result.body = body;
    if (created != null) result.created = created;
    if (updated != null) result.updated = updated;
    return result;
  }

  MemoryNote._();

  factory MemoryNote.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MemoryNote()..mergeFromBuffer(data, registry);
  factory MemoryNote.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MemoryNote()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'MemoryNote',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: MemoryNote.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'slug')
    ..aOS(2, _omitFieldNames ? '' : 'summary')
    ..aOS(3, _omitFieldNames ? '' : 'body')
    ..aInt64(4, _omitFieldNames ? '' : 'created')
    ..aInt64(5, _omitFieldNames ? '' : 'updated')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MemoryNote clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MemoryNote copyWith(void Function(MemoryNote) updates) =>
      super.copyWith((message) => updates(message as MemoryNote)) as MemoryNote;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use MemoryNote() / MemoryNote.new instead')
  static MemoryNote create() => MemoryNote._();
  static $pb.GeneratedMessage $_createMessage() => MemoryNote._();
  @$core.override
  MemoryNote createEmptyInstance() => MemoryNote._();
  @$core.pragma('dart2js:noInline')
  static MemoryNote getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<MemoryNote>(MemoryNote.$_createMessage);
  static MemoryNote? _defaultInstance;

  /// a-z, 0-9 and -, e.g. "deploy-quirks".
  @$pb.TagNumber(1)
  $core.String get slug => $_getSZ(0);
  @$pb.TagNumber(1)
  set slug($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasSlug() => $_has(0);
  @$pb.TagNumber(1)
  void clearSlug() => $_clearField(1);

  /// One line, up to 150 characters.
  @$pb.TagNumber(2)
  $core.String get summary => $_getSZ(1);
  @$pb.TagNumber(2)
  set summary($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasSummary() => $_has(1);
  @$pb.TagNumber(2)
  void clearSummary() => $_clearField(2);

  /// Markdown.
  @$pb.TagNumber(3)
  $core.String get body => $_getSZ(2);
  @$pb.TagNumber(3)
  set body($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasBody() => $_has(2);
  @$pb.TagNumber(3)
  void clearBody() => $_clearField(3);

  /// ms since 1970.
  @$pb.TagNumber(4)
  $fixnum.Int64 get created => $_getI64(3);
  @$pb.TagNumber(4)
  set created($fixnum.Int64 value) => $_setInt64(3, value);
  @$pb.TagNumber(4)
  $core.bool hasCreated() => $_has(3);
  @$pb.TagNumber(4)
  void clearCreated() => $_clearField(4);

  @$pb.TagNumber(5)
  $fixnum.Int64 get updated => $_getI64(4);
  @$pb.TagNumber(5)
  set updated($fixnum.Int64 value) => $_setInt64(4, value);
  @$pb.TagNumber(5)
  $core.bool hasUpdated() => $_has(4);
  @$pb.TagNumber(5)
  void clearUpdated() => $_clearField(5);
}

class MemoryChange extends $pb.GeneratedMessage {
  factory MemoryChange({
    $fixnum.Int64? time,
    $core.String? actor,
    MemoryChangeKind? kind,
    $core.int? preferenceId,
    $core.String? noteSlug,
    $core.String? text,
  }) {
    final result = MemoryChange._();
    if (time != null) result.time = time;
    if (actor != null) result.actor = actor;
    if (kind != null) result.kind = kind;
    if (preferenceId != null) result.preferenceId = preferenceId;
    if (noteSlug != null) result.noteSlug = noteSlug;
    if (text != null) result.text = text;
    return result;
  }

  MemoryChange._();

  factory MemoryChange.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MemoryChange()..mergeFromBuffer(data, registry);
  factory MemoryChange.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MemoryChange()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'MemoryChange',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: MemoryChange.$_createMessage)
    ..aInt64(1, _omitFieldNames ? '' : 'time')
    ..aOS(2, _omitFieldNames ? '' : 'actor')
    ..aE<MemoryChangeKind>(3, _omitFieldNames ? '' : 'kind',
        enumValues: MemoryChangeKind.values)
    ..aI(4, _omitFieldNames ? '' : 'preferenceId',
        fieldType: $pb.PbFieldType.OU3)
    ..aOS(5, _omitFieldNames ? '' : 'noteSlug')
    ..aOS(6, _omitFieldNames ? '' : 'text')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MemoryChange clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MemoryChange copyWith(void Function(MemoryChange) updates) =>
      super.copyWith((message) => updates(message as MemoryChange))
          as MemoryChange;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use MemoryChange() / MemoryChange.new instead')
  static MemoryChange create() => MemoryChange._();
  static $pb.GeneratedMessage $_createMessage() => MemoryChange._();
  @$core.override
  MemoryChange createEmptyInstance() => MemoryChange._();
  @$core.pragma('dart2js:noInline')
  static MemoryChange getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<MemoryChange>(
          MemoryChange.$_createMessage);
  static MemoryChange? _defaultInstance;

  /// ms since 1970.
  @$pb.TagNumber(1)
  $fixnum.Int64 get time => $_getI64(0);
  @$pb.TagNumber(1)
  set time($fixnum.Int64 value) => $_setInt64(0, value);
  @$pb.TagNumber(1)
  $core.bool hasTime() => $_has(0);
  @$pb.TagNumber(1)
  void clearTime() => $_clearField(1);

  /// The agent id of who made it; empty for the person.
  @$pb.TagNumber(2)
  $core.String get actor => $_getSZ(1);
  @$pb.TagNumber(2)
  set actor($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasActor() => $_has(1);
  @$pb.TagNumber(2)
  void clearActor() => $_clearField(2);

  @$pb.TagNumber(3)
  MemoryChangeKind get kind => $_getN(2);
  @$pb.TagNumber(3)
  set kind(MemoryChangeKind value) => $_setField(3, value);
  @$pb.TagNumber(3)
  $core.bool hasKind() => $_has(2);
  @$pb.TagNumber(3)
  void clearKind() => $_clearField(3);

  /// Set for preference changes.
  @$pb.TagNumber(4)
  $core.int get preferenceId => $_getIZ(3);
  @$pb.TagNumber(4)
  set preferenceId($core.int value) => $_setUnsignedInt32(3, value);
  @$pb.TagNumber(4)
  $core.bool hasPreferenceId() => $_has(3);
  @$pb.TagNumber(4)
  void clearPreferenceId() => $_clearField(4);

  /// Set for note changes.
  @$pb.TagNumber(5)
  $core.String get noteSlug => $_getSZ(4);
  @$pb.TagNumber(5)
  set noteSlug($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasNoteSlug() => $_has(4);
  @$pb.TagNumber(5)
  void clearNoteSlug() => $_clearField(5);

  /// A preference's rule (the old one when removed); a note's summary.
  @$pb.TagNumber(6)
  $core.String get text => $_getSZ(5);
  @$pb.TagNumber(6)
  set text($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasText() => $_has(5);
  @$pb.TagNumber(6)
  void clearText() => $_clearField(6);
}

/// The background tasks an agent can see (its own, or every agent's with
/// see_all or admin): running ones first, newest first, then the 20 that
/// ended most recently.
class Tasks extends $pb.GeneratedMessage {
  factory Tasks({
    $core.Iterable<BackgroundTask>? tasks,
  }) {
    final result = Tasks._();
    if (tasks != null) result.tasks.addAll(tasks);
    return result;
  }

  Tasks._();

  factory Tasks.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Tasks()..mergeFromBuffer(data, registry);
  factory Tasks.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Tasks()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Tasks',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Tasks.$_createMessage)
    ..pPM<BackgroundTask>(1, _omitFieldNames ? '' : 'tasks',
        subBuilder: BackgroundTask.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Tasks clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Tasks copyWith(void Function(Tasks) updates) =>
      super.copyWith((message) => updates(message as Tasks)) as Tasks;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Tasks() / Tasks.new instead')
  static Tasks create() => Tasks._();
  static $pb.GeneratedMessage $_createMessage() => Tasks._();
  @$core.override
  Tasks createEmptyInstance() => Tasks._();
  @$core.pragma('dart2js:noInline')
  static Tasks getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Tasks>(Tasks.$_createMessage);
  static Tasks? _defaultInstance;

  @$pb.TagNumber(1)
  $pb.PbList<BackgroundTask> get tasks => $_getList(0);
}

class BackgroundTask extends $pb.GeneratedMessage {
  factory BackgroundTask({
    $core.String? id,
    $core.String? agentId,
    $core.String? command,
    $fixnum.Int64? started,
    $fixnum.Int64? ended,
    $core.bool? running,
    $core.String? status,
    $core.String? logPath,
    $core.String? logTail,
  }) {
    final result = BackgroundTask._();
    if (id != null) result.id = id;
    if (agentId != null) result.agentId = agentId;
    if (command != null) result.command = command;
    if (started != null) result.started = started;
    if (ended != null) result.ended = ended;
    if (running != null) result.running = running;
    if (status != null) result.status = status;
    if (logPath != null) result.logPath = logPath;
    if (logTail != null) result.logTail = logTail;
    return result;
  }

  BackgroundTask._();

  factory BackgroundTask.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      BackgroundTask()..mergeFromBuffer(data, registry);
  factory BackgroundTask.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      BackgroundTask()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'BackgroundTask',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: BackgroundTask.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'agentId')
    ..aOS(3, _omitFieldNames ? '' : 'command')
    ..aInt64(4, _omitFieldNames ? '' : 'started')
    ..aInt64(5, _omitFieldNames ? '' : 'ended')
    ..aOB(6, _omitFieldNames ? '' : 'running')
    ..aOS(7, _omitFieldNames ? '' : 'status')
    ..aOS(8, _omitFieldNames ? '' : 'logPath')
    ..aOS(9, _omitFieldNames ? '' : 'logTail')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  BackgroundTask clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  BackgroundTask copyWith(void Function(BackgroundTask) updates) =>
      super.copyWith((message) => updates(message as BackgroundTask))
          as BackgroundTask;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use BackgroundTask() / BackgroundTask.new instead')
  static BackgroundTask create() => BackgroundTask._();
  static $pb.GeneratedMessage $_createMessage() => BackgroundTask._();
  @$core.override
  BackgroundTask createEmptyInstance() => BackgroundTask._();
  @$core.pragma('dart2js:noInline')
  static BackgroundTask getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<BackgroundTask>(
          BackgroundTask.$_createMessage);
  static BackgroundTask? _defaultInstance;

  /// Per agent: "task-3".
  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// The agent that started it.
  @$pb.TagNumber(2)
  $core.String get agentId => $_getSZ(1);
  @$pb.TagNumber(2)
  set agentId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasAgentId() => $_has(1);
  @$pb.TagNumber(2)
  void clearAgentId() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.String get command => $_getSZ(2);
  @$pb.TagNumber(3)
  set command($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasCommand() => $_has(2);
  @$pb.TagNumber(3)
  void clearCommand() => $_clearField(3);

  /// When it started and ended, in ms since 1970; ended is 0 while it runs.
  @$pb.TagNumber(4)
  $fixnum.Int64 get started => $_getI64(3);
  @$pb.TagNumber(4)
  set started($fixnum.Int64 value) => $_setInt64(3, value);
  @$pb.TagNumber(4)
  $core.bool hasStarted() => $_has(3);
  @$pb.TagNumber(4)
  void clearStarted() => $_clearField(4);

  @$pb.TagNumber(5)
  $fixnum.Int64 get ended => $_getI64(4);
  @$pb.TagNumber(5)
  set ended($fixnum.Int64 value) => $_setInt64(4, value);
  @$pb.TagNumber(5)
  $core.bool hasEnded() => $_has(4);
  @$pb.TagNumber(5)
  void clearEnded() => $_clearField(5);

  @$pb.TagNumber(6)
  $core.bool get running => $_getBF(5);
  @$pb.TagNumber(6)
  set running($core.bool value) => $_setBool(5, value);
  @$pb.TagNumber(6)
  $core.bool hasRunning() => $_has(5);
  @$pb.TagNumber(6)
  void clearRunning() => $_clearField(6);

  /// How it ended: "exit 0", "exit 1", "signal SIGKILL", "timed out",
  /// "stopped by Rivet", "stopped by the person"…; empty while it runs.
  @$pb.TagNumber(7)
  $core.String get status => $_getSZ(6);
  @$pb.TagNumber(7)
  set status($core.String value) => $_setString(6, value);
  @$pb.TagNumber(7)
  $core.bool hasStatus() => $_has(6);
  @$pb.TagNumber(7)
  void clearStatus() => $_clearField(7);

  /// The absolute path of its output log.
  @$pb.TagNumber(8)
  $core.String get logPath => $_getSZ(7);
  @$pb.TagNumber(8)
  set logPath($core.String value) => $_setString(7, value);
  @$pb.TagNumber(8)
  $core.bool hasLogPath() => $_has(7);
  @$pb.TagNumber(8)
  void clearLogPath() => $_clearField(8);

  /// The last lines of its output (about 20).
  @$pb.TagNumber(9)
  $core.String get logTail => $_getSZ(8);
  @$pb.TagNumber(9)
  set logTail($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasLogTail() => $_has(8);
  @$pb.TagNumber(9)
  void clearLogTail() => $_clearField(9);
}

/// An agent's monitors: what will prompt it later, and what else will.
class Monitors extends $pb.GeneratedMessage {
  factory Monitors({
    $core.Iterable<Monitor>? monitors,
    $core.Iterable<$core.String>? builtins,
  }) {
    final result = Monitors._();
    if (monitors != null) result.monitors.addAll(monitors);
    if (builtins != null) result.builtins.addAll(builtins);
    return result;
  }

  Monitors._();

  factory Monitors.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Monitors()..mergeFromBuffer(data, registry);
  factory Monitors.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Monitors()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Monitors',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Monitors.$_createMessage)
    ..pPM<Monitor>(1, _omitFieldNames ? '' : 'monitors',
        subBuilder: Monitor.$_createMessage)
    ..pPS(2, _omitFieldNames ? '' : 'builtins')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Monitors clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Monitors copyWith(void Function(Monitors) updates) =>
      super.copyWith((message) => updates(message as Monitors)) as Monitors;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Monitors() / Monitors.new instead')
  static Monitors create() => Monitors._();
  static $pb.GeneratedMessage $_createMessage() => Monitors._();
  @$core.override
  Monitors createEmptyInstance() => Monitors._();
  @$core.pragma('dart2js:noInline')
  static Monitors getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Monitors>(Monitors.$_createMessage);
  static Monitors? _defaultInstance;

  /// Its monitors, oldest first: active, paused and failed ones, and ones that
  /// ended in the last week.
  @$pb.TagNumber(1)
  $pb.PbList<Monitor> get monitors => $_getList(0);

  /// What else will wake it, one line each, e.g. "task-3 (background task) —
  /// wakes you when it exits", "checklist reminder — on: …".
  @$pb.TagNumber(2)
  $pb.PbList<$core.String> get builtins => $_getList(1);
}

class Monitor extends $pb.GeneratedMessage {
  factory Monitor({
    $core.int? id,
    $core.String? name,
    $core.String? prompt,
    $core.String? when,
    $core.String? kind,
    MonitorState? state,
    $core.String? reason,
    $fixnum.Int64? nextDue,
    $fixnum.Int64? lastFired,
    $core.int? fires,
    $fixnum.Int64? until,
    $core.int? maxFires,
    $core.bool? idleOnly,
    $core.String? createdBy,
    $fixnum.Int64? created,
  }) {
    final result = Monitor._();
    if (id != null) result.id = id;
    if (name != null) result.name = name;
    if (prompt != null) result.prompt = prompt;
    if (when != null) result.when = when;
    if (kind != null) result.kind = kind;
    if (state != null) result.state = state;
    if (reason != null) result.reason = reason;
    if (nextDue != null) result.nextDue = nextDue;
    if (lastFired != null) result.lastFired = lastFired;
    if (fires != null) result.fires = fires;
    if (until != null) result.until = until;
    if (maxFires != null) result.maxFires = maxFires;
    if (idleOnly != null) result.idleOnly = idleOnly;
    if (createdBy != null) result.createdBy = createdBy;
    if (created != null) result.created = created;
    return result;
  }

  Monitor._();

  factory Monitor.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Monitor()..mergeFromBuffer(data, registry);
  factory Monitor.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Monitor()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Monitor',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Monitor.$_createMessage)
    ..aI(1, _omitFieldNames ? '' : 'id', fieldType: $pb.PbFieldType.OU3)
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'prompt')
    ..aOS(4, _omitFieldNames ? '' : 'when')
    ..aOS(5, _omitFieldNames ? '' : 'kind')
    ..aE<MonitorState>(6, _omitFieldNames ? '' : 'state',
        enumValues: MonitorState.values)
    ..aOS(7, _omitFieldNames ? '' : 'reason')
    ..aInt64(8, _omitFieldNames ? '' : 'nextDue')
    ..aInt64(9, _omitFieldNames ? '' : 'lastFired')
    ..aI(10, _omitFieldNames ? '' : 'fires', fieldType: $pb.PbFieldType.OU3)
    ..aInt64(11, _omitFieldNames ? '' : 'until')
    ..aI(12, _omitFieldNames ? '' : 'maxFires', fieldType: $pb.PbFieldType.OU3)
    ..aOB(13, _omitFieldNames ? '' : 'idleOnly')
    ..aOS(14, _omitFieldNames ? '' : 'createdBy')
    ..aInt64(15, _omitFieldNames ? '' : 'created')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Monitor clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Monitor copyWith(void Function(Monitor) updates) =>
      super.copyWith((message) => updates(message as Monitor)) as Monitor;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Monitor() / Monitor.new instead')
  static Monitor create() => Monitor._();
  static $pb.GeneratedMessage $_createMessage() => Monitor._();
  @$core.override
  Monitor createEmptyInstance() => Monitor._();
  @$core.pragma('dart2js:noInline')
  static Monitor getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Monitor>(Monitor.$_createMessage);
  static Monitor? _defaultInstance;

  /// Small and per agent: 3 for m3; never reused.
  @$pb.TagNumber(1)
  $core.int get id => $_getIZ(0);
  @$pb.TagNumber(1)
  set id($core.int value) => $_setUnsignedInt32(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// What the agent is told when it fires.
  @$pb.TagNumber(3)
  $core.String get prompt => $_getSZ(2);
  @$pb.TagNumber(3)
  set prompt($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasPrompt() => $_has(2);
  @$pb.TagNumber(3)
  void clearPrompt() => $_clearField(3);

  /// What it waits for, in words, e.g. "every 1h", "check `make test` every
  /// 5m, fires on failure", "files src/**/*.rs".
  @$pb.TagNumber(4)
  $core.String get when => $_getSZ(3);
  @$pb.TagNumber(4)
  set when($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasWhen() => $_has(3);
  @$pb.TagNumber(4)
  void clearWhen() => $_clearField(4);

  /// "at", "every", "cron", "check", "files" or "watch".
  @$pb.TagNumber(5)
  $core.String get kind => $_getSZ(4);
  @$pb.TagNumber(5)
  set kind($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasKind() => $_has(4);
  @$pb.TagNumber(5)
  void clearKind() => $_clearField(5);

  @$pb.TagNumber(6)
  MonitorState get state => $_getN(5);
  @$pb.TagNumber(6)
  set state(MonitorState value) => $_setField(6, value);
  @$pb.TagNumber(6)
  $core.bool hasState() => $_has(5);
  @$pb.TagNumber(6)
  void clearState() => $_clearField(6);

  /// Why it's paused or ended, e.g. "by the person", "by coordinator", "its
  /// until passed"; may be empty.
  @$pb.TagNumber(7)
  $core.String get reason => $_getSZ(6);
  @$pb.TagNumber(7)
  set reason($core.String value) => $_setString(6, value);
  @$pb.TagNumber(7)
  $core.bool hasReason() => $_has(6);
  @$pb.TagNumber(7)
  void clearReason() => $_clearField(7);

  /// When it next fires or checks, in ms since 1970; 0 when it has no time
  /// (files, watch, or not active).
  @$pb.TagNumber(8)
  $fixnum.Int64 get nextDue => $_getI64(7);
  @$pb.TagNumber(8)
  set nextDue($fixnum.Int64 value) => $_setInt64(7, value);
  @$pb.TagNumber(8)
  $core.bool hasNextDue() => $_has(7);
  @$pb.TagNumber(8)
  void clearNextDue() => $_clearField(8);

  /// When it last fired, in ms since 1970; 0 if it never has.
  @$pb.TagNumber(9)
  $fixnum.Int64 get lastFired => $_getI64(8);
  @$pb.TagNumber(9)
  set lastFired($fixnum.Int64 value) => $_setInt64(8, value);
  @$pb.TagNumber(9)
  $core.bool hasLastFired() => $_has(8);
  @$pb.TagNumber(9)
  void clearLastFired() => $_clearField(9);

  @$pb.TagNumber(10)
  $core.int get fires => $_getIZ(9);
  @$pb.TagNumber(10)
  set fires($core.int value) => $_setUnsignedInt32(9, value);
  @$pb.TagNumber(10)
  $core.bool hasFires() => $_has(9);
  @$pb.TagNumber(10)
  void clearFires() => $_clearField(10);

  /// When it ends on its own, in ms since 1970; 0 for never.
  @$pb.TagNumber(11)
  $fixnum.Int64 get until => $_getI64(10);
  @$pb.TagNumber(11)
  set until($fixnum.Int64 value) => $_setInt64(10, value);
  @$pb.TagNumber(11)
  $core.bool hasUntil() => $_has(10);
  @$pb.TagNumber(11)
  void clearUntil() => $_clearField(11);

  /// It ends after this many fires; 0 for no limit.
  @$pb.TagNumber(12)
  $core.int get maxFires => $_getIZ(11);
  @$pb.TagNumber(12)
  set maxFires($core.int value) => $_setUnsignedInt32(11, value);
  @$pb.TagNumber(12)
  $core.bool hasMaxFires() => $_has(11);
  @$pb.TagNumber(12)
  void clearMaxFires() => $_clearField(12);

  /// It fires only while the agent is idle.
  @$pb.TagNumber(13)
  $core.bool get idleOnly => $_getBF(12);
  @$pb.TagNumber(13)
  set idleOnly($core.bool value) => $_setBool(12, value);
  @$pb.TagNumber(13)
  $core.bool hasIdleOnly() => $_has(12);
  @$pb.TagNumber(13)
  void clearIdleOnly() => $_clearField(13);

  /// The agent id of who set it; empty for the person.
  @$pb.TagNumber(14)
  $core.String get createdBy => $_getSZ(13);
  @$pb.TagNumber(14)
  set createdBy($core.String value) => $_setString(13, value);
  @$pb.TagNumber(14)
  $core.bool hasCreatedBy() => $_has(13);
  @$pb.TagNumber(14)
  void clearCreatedBy() => $_clearField(14);

  /// ms since 1970.
  @$pb.TagNumber(15)
  $fixnum.Int64 get created => $_getI64(14);
  @$pb.TagNumber(15)
  set created($fixnum.Int64 value) => $_setInt64(14, value);
  @$pb.TagNumber(15)
  $core.bool hasCreated() => $_has(14);
  @$pb.TagNumber(15)
  void clearCreated() => $_clearField(15);
}

class Settings extends $pb.GeneratedMessage {
  factory Settings({
    $core.String? defaultModel,
    $core.String? defaultEffort,
    $fixnum.Int64? compactAtTokens,
    $core.int? ramPercent,
    $core.int? diskFreeGb,
    $core.int? diskFreePercent,
    $core.int? cpuPercent,
    $core.double? sidebarWidth,
    $core.double? panelWidth,
    $core.String? userName,
    $core.double? buildCacheGb,
    $core.int? vaultUnlockMinutes,
    $core.bool? idleCompaction,
    $core.bool? idleCompactionShortCache,
    $core.Iterable<CacheTtl>? cacheTtls,
  }) {
    final result = Settings._();
    if (defaultModel != null) result.defaultModel = defaultModel;
    if (defaultEffort != null) result.defaultEffort = defaultEffort;
    if (compactAtTokens != null) result.compactAtTokens = compactAtTokens;
    if (ramPercent != null) result.ramPercent = ramPercent;
    if (diskFreeGb != null) result.diskFreeGb = diskFreeGb;
    if (diskFreePercent != null) result.diskFreePercent = diskFreePercent;
    if (cpuPercent != null) result.cpuPercent = cpuPercent;
    if (sidebarWidth != null) result.sidebarWidth = sidebarWidth;
    if (panelWidth != null) result.panelWidth = panelWidth;
    if (userName != null) result.userName = userName;
    if (buildCacheGb != null) result.buildCacheGb = buildCacheGb;
    if (vaultUnlockMinutes != null)
      result.vaultUnlockMinutes = vaultUnlockMinutes;
    if (idleCompaction != null) result.idleCompaction = idleCompaction;
    if (idleCompactionShortCache != null)
      result.idleCompactionShortCache = idleCompactionShortCache;
    if (cacheTtls != null) result.cacheTtls.addAll(cacheTtls);
    return result;
  }

  Settings._();

  factory Settings.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Settings()..mergeFromBuffer(data, registry);
  factory Settings.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Settings()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Settings',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Settings.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'defaultModel')
    ..aOS(2, _omitFieldNames ? '' : 'defaultEffort')
    ..a<$fixnum.Int64>(
        3, _omitFieldNames ? '' : 'compactAtTokens', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..aI(4, _omitFieldNames ? '' : 'ramPercent', fieldType: $pb.PbFieldType.OU3)
    ..aI(5, _omitFieldNames ? '' : 'diskFreeGb', fieldType: $pb.PbFieldType.OU3)
    ..aI(6, _omitFieldNames ? '' : 'diskFreePercent',
        fieldType: $pb.PbFieldType.OU3)
    ..aI(7, _omitFieldNames ? '' : 'cpuPercent', fieldType: $pb.PbFieldType.OU3)
    ..aD(8, _omitFieldNames ? '' : 'sidebarWidth')
    ..aD(9, _omitFieldNames ? '' : 'panelWidth')
    ..aOS(10, _omitFieldNames ? '' : 'userName')
    ..aD(11, _omitFieldNames ? '' : 'buildCacheGb')
    ..aI(20, _omitFieldNames ? '' : 'vaultUnlockMinutes',
        fieldType: $pb.PbFieldType.OU3)
    ..aOB(23, _omitFieldNames ? '' : 'idleCompaction')
    ..aOB(24, _omitFieldNames ? '' : 'idleCompactionShortCache')
    ..pPM<CacheTtl>(25, _omitFieldNames ? '' : 'cacheTtls',
        subBuilder: CacheTtl.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Settings clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Settings copyWith(void Function(Settings) updates) =>
      super.copyWith((message) => updates(message as Settings)) as Settings;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Settings() / Settings.new instead')
  static Settings create() => Settings._();
  static $pb.GeneratedMessage $_createMessage() => Settings._();
  @$core.override
  Settings createEmptyInstance() => Settings._();
  @$core.pragma('dart2js:noInline')
  static Settings getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Settings>(Settings.$_createMessage);
  static Settings? _defaultInstance;

  /// Model for new agents that don't choose one: "<provider>/<model id>".
  @$pb.TagNumber(1)
  $core.String get defaultModel => $_getSZ(0);
  @$pb.TagNumber(1)
  set defaultModel($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasDefaultModel() => $_has(0);
  @$pb.TagNumber(1)
  void clearDefaultModel() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get defaultEffort => $_getSZ(1);
  @$pb.TagNumber(2)
  set defaultEffort($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasDefaultEffort() => $_has(1);
  @$pb.TagNumber(2)
  void clearDefaultEffort() => $_clearField(2);

  /// Agents' contexts are compacted when they reach this many tokens.
  @$pb.TagNumber(3)
  $fixnum.Int64 get compactAtTokens => $_getI64(2);
  @$pb.TagNumber(3)
  set compactAtTokens($fixnum.Int64 value) => $_setInt64(2, value);
  @$pb.TagNumber(3)
  $core.bool hasCompactAtTokens() => $_has(2);
  @$pb.TagNumber(3)
  void clearCompactAtTokens() => $_clearField(3);

  /// When agents with the Resources extension are told the machine runs low
  /// (config.json's `resources`), as in effect: memory used above this
  /// percentage for a few minutes; a volume Factory uses with less free than
  /// disk_free_gb GB or disk_free_percent of it, whichever is smaller; the
  /// processors busier than cpu_percent for several minutes, 0 when that
  /// alert is off.
  @$pb.TagNumber(4)
  $core.int get ramPercent => $_getIZ(3);
  @$pb.TagNumber(4)
  set ramPercent($core.int value) => $_setUnsignedInt32(3, value);
  @$pb.TagNumber(4)
  $core.bool hasRamPercent() => $_has(3);
  @$pb.TagNumber(4)
  void clearRamPercent() => $_clearField(4);

  @$pb.TagNumber(5)
  $core.int get diskFreeGb => $_getIZ(4);
  @$pb.TagNumber(5)
  set diskFreeGb($core.int value) => $_setUnsignedInt32(4, value);
  @$pb.TagNumber(5)
  $core.bool hasDiskFreeGb() => $_has(4);
  @$pb.TagNumber(5)
  void clearDiskFreeGb() => $_clearField(5);

  @$pb.TagNumber(6)
  $core.int get diskFreePercent => $_getIZ(5);
  @$pb.TagNumber(6)
  set diskFreePercent($core.int value) => $_setUnsignedInt32(5, value);
  @$pb.TagNumber(6)
  $core.bool hasDiskFreePercent() => $_has(5);
  @$pb.TagNumber(6)
  void clearDiskFreePercent() => $_clearField(6);

  @$pb.TagNumber(7)
  $core.int get cpuPercent => $_getIZ(6);
  @$pb.TagNumber(7)
  set cpuPercent($core.int value) => $_setUnsignedInt32(6, value);
  @$pb.TagNumber(7)
  $core.bool hasCpuPercent() => $_has(6);
  @$pb.TagNumber(7)
  void clearCpuPercent() => $_clearField(7);

  /// How wide the person dragged the sidebar and the right panel (the
  /// extension panel), in logical pixels, the same on every device
  /// (config.json's `display`); 0 for the app's default. The app keeps them
  /// in its own range.
  @$pb.TagNumber(8)
  $core.double get sidebarWidth => $_getN(7);
  @$pb.TagNumber(8)
  set sidebarWidth($core.double value) => $_setDouble(7, value);
  @$pb.TagNumber(8)
  $core.bool hasSidebarWidth() => $_has(7);
  @$pb.TagNumber(8)
  void clearSidebarWidth() => $_clearField(8);

  @$pb.TagNumber(9)
  $core.double get panelWidth => $_getN(8);
  @$pb.TagNumber(9)
  set panelWidth($core.double value) => $_setDouble(8, value);
  @$pb.TagNumber(9)
  $core.bool hasPanelWidth() => $_has(8);
  @$pb.TagNumber(9)
  void clearPanelWidth() => $_clearField(9);

  /// The user's first name, which agents call them by (config.json's
  /// `user.name`); empty when not given, and agents say "the person".
  @$pb.TagNumber(10)
  $core.String get userName => $_getSZ(9);
  @$pb.TagNumber(10)
  set userName($core.String value) => $_setString(9, value);
  @$pb.TagNumber(10)
  $core.bool hasUserName() => $_has(9);
  @$pb.TagNumber(10)
  void clearUserName() => $_clearField(10);

  /// GB the build caches of one agent's checkout, or of one project folder,
  /// may take (config.json's `resources.build_cache_gb`, 25 unless changed):
  /// over it, Factory clears them while nothing works there. 0: never.
  @$pb.TagNumber(11)
  $core.double get buildCacheGb => $_getN(10);
  @$pb.TagNumber(11)
  set buildCacheGb($core.double value) => $_setDouble(10, value);
  @$pb.TagNumber(11)
  $core.bool hasBuildCacheGb() => $_has(10);
  @$pb.TagNumber(11)
  void clearBuildCacheGb() => $_clearField(11);

  /// Unused: requests need no vault passphrase (config.json's
  /// `vault.unlock_minutes`, 5 unless changed).
  @$pb.TagNumber(20)
  $core.int get vaultUnlockMinutes => $_getIZ(11);
  @$pb.TagNumber(20)
  set vaultUnlockMinutes($core.int value) => $_setUnsignedInt32(11, value);
  @$pb.TagNumber(20)
  $core.bool hasVaultUnlockMinutes() => $_has(11);
  @$pb.TagNumber(20)
  void clearVaultUnlockMinutes() => $_clearField(20);

  /// An idle agent compacts its conversation just before its prompt cache
  /// expires, so coming back to it later doesn't pay to cache the whole long
  /// conversation again (config.json's `compaction.idle`, on unless changed).
  /// Only for a cache Claktory knows the lifetime of (an hour: after 54
  /// minutes idle; 5 minutes: only with `idle_compaction_short_cache`), and a
  /// conversation of at least 150,000 tokens on a 200,000-token model (75% of
  /// a smaller window, at most 200,000 on a larger one).
  @$pb.TagNumber(23)
  $core.bool get idleCompaction => $_getBF(12);
  @$pb.TagNumber(23)
  set idleCompaction($core.bool value) => $_setBool(12, value);
  @$pb.TagNumber(23)
  $core.bool hasIdleCompaction() => $_has(12);
  @$pb.TagNumber(23)
  void clearIdleCompaction() => $_clearField(23);

  /// Also compact after 4 minutes idle when the cache lasts 5 minutes
  /// (config.json's `compaction.idle_short_cache`, off unless changed). Far
  /// more often than with an hour's cache.
  @$pb.TagNumber(24)
  $core.bool get idleCompactionShortCache => $_getBF(13);
  @$pb.TagNumber(24)
  set idleCompactionShortCache($core.bool value) => $_setBool(13, value);
  @$pb.TagNumber(24)
  $core.bool hasIdleCompactionShortCache() => $_has(13);
  @$pb.TagNumber(24)
  void clearIdleCompactionShortCache() => $_clearField(24);

  /// How long each provider's or model's prompt cache is asked to last, as
  /// set (config.json's `prompt_cache`); what isn't listed asks for the
  /// provider's default. See Provider.cache_ttl for what applies.
  @$pb.TagNumber(25)
  $pb.PbList<CacheTtl> get cacheTtls => $_getList(14);
}

/// How long a provider's (or one model's) prompt cache is asked to last.
class CacheTtl extends $pb.GeneratedMessage {
  factory CacheTtl({
    $core.String? key,
    $core.String? ttl,
  }) {
    final result = CacheTtl._();
    if (key != null) result.key = key;
    if (ttl != null) result.ttl = ttl;
    return result;
  }

  CacheTtl._();

  factory CacheTtl.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CacheTtl()..mergeFromBuffer(data, registry);
  factory CacheTtl.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CacheTtl()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'CacheTtl',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: CacheTtl.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'key')
    ..aOS(2, _omitFieldNames ? '' : 'ttl')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CacheTtl clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CacheTtl copyWith(void Function(CacheTtl) updates) =>
      super.copyWith((message) => updates(message as CacheTtl)) as CacheTtl;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use CacheTtl() / CacheTtl.new instead')
  static CacheTtl create() => CacheTtl._();
  static $pb.GeneratedMessage $_createMessage() => CacheTtl._();
  @$core.override
  CacheTtl createEmptyInstance() => CacheTtl._();
  @$core.pragma('dart2js:noInline')
  static CacheTtl getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<CacheTtl>(CacheTtl.$_createMessage);
  static CacheTtl? _defaultInstance;

  /// A provider id ("anthropic") or a model id ("anthropic/claude-opus-4-8"),
  /// which wins over its provider's.
  @$pb.TagNumber(1)
  $core.String get key => $_getSZ(0);
  @$pb.TagNumber(1)
  set key($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasKey() => $_has(0);
  @$pb.TagNumber(1)
  void clearKey() => $_clearField(1);

  /// "5m" or "1h". An hour costs more to write the cache (2× input instead of
  /// 1.25× on Anthropic) and keeps it through longer breaks.
  @$pb.TagNumber(2)
  $core.String get ttl => $_getSZ(1);
  @$pb.TagNumber(2)
  set ttl($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasTtl() => $_has(1);
  @$pb.TagNumber(2)
  void clearTtl() => $_clearField(2);
}

class Provider extends $pb.GeneratedMessage {
  factory Provider({
    $core.String? id,
    $core.String? name,
    $core.String? signedInWith,
    $core.bool? canSignOut,
    $core.Iterable<LoginMethod>? methods,
    ProviderUsage? usage,
    $core.String? plan,
    $core.Iterable<Account>? accounts,
    $core.String? cacheTtl,
  }) {
    final result = Provider._();
    if (id != null) result.id = id;
    if (name != null) result.name = name;
    if (signedInWith != null) result.signedInWith = signedInWith;
    if (canSignOut != null) result.canSignOut = canSignOut;
    if (methods != null) result.methods.addAll(methods);
    if (usage != null) result.usage = usage;
    if (plan != null) result.plan = plan;
    if (accounts != null) result.accounts.addAll(accounts);
    if (cacheTtl != null) result.cacheTtl = cacheTtl;
    return result;
  }

  Provider._();

  factory Provider.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Provider()..mergeFromBuffer(data, registry);
  factory Provider.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Provider()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Provider',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Provider.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'signedInWith')
    ..aOB(4, _omitFieldNames ? '' : 'canSignOut')
    ..pPM<LoginMethod>(5, _omitFieldNames ? '' : 'methods',
        subBuilder: LoginMethod.$_createMessage)
    ..aOM<ProviderUsage>(6, _omitFieldNames ? '' : 'usage',
        subBuilder: ProviderUsage.$_createMessage)
    ..aOS(7, _omitFieldNames ? '' : 'plan')
    ..pPM<Account>(8, _omitFieldNames ? '' : 'accounts',
        subBuilder: Account.$_createMessage)
    ..aOS(9, _omitFieldNames ? '' : 'cacheTtl')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Provider clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Provider copyWith(void Function(Provider) updates) =>
      super.copyWith((message) => updates(message as Provider)) as Provider;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Provider() / Provider.new instead')
  static Provider create() => Provider._();
  static $pb.GeneratedMessage $_createMessage() => Provider._();
  @$core.override
  Provider createEmptyInstance() => Provider._();
  @$core.pragma('dart2js:noInline')
  static Provider getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Provider>(Provider.$_createMessage);
  static Provider? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// How Factory is signed in, e.g. "Anthropic (Claude Pro/Max)" or
  /// "ANTHROPIC_API_KEY". Empty when it isn't.
  @$pb.TagNumber(3)
  $core.String get signedInWith => $_getSZ(2);
  @$pb.TagNumber(3)
  set signedInWith($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasSignedInWith() => $_has(2);
  @$pb.TagNumber(3)
  void clearSignedInWith() => $_clearField(3);

  /// Factory stored this credential and `SignOut` removes it. False for
  /// credentials from the environment.
  @$pb.TagNumber(4)
  $core.bool get canSignOut => $_getBF(3);
  @$pb.TagNumber(4)
  set canSignOut($core.bool value) => $_setBool(3, value);
  @$pb.TagNumber(4)
  $core.bool hasCanSignOut() => $_has(3);
  @$pb.TagNumber(4)
  void clearCanSignOut() => $_clearField(4);

  /// Ways to sign in, in the provider's order of preference.
  @$pb.TagNumber(5)
  $pb.PbList<LoginMethod> get methods => $_getList(4);

  /// How much of its usage limits the signed-in subscription has used. Unset
  /// when the provider isn't signed in or has no usage limits (API keys). Set
  /// as soon as a subscription signs in, with no windows and `updated` 0 until
  /// the first read lands.
  @$pb.TagNumber(6)
  ProviderUsage get usage => $_getN(5);
  @$pb.TagNumber(6)
  set usage(ProviderUsage value) => $_setField(6, value);
  @$pb.TagNumber(6)
  $core.bool hasUsage() => $_has(5);
  @$pb.TagNumber(6)
  void clearUsage() => $_clearField(6);
  @$pb.TagNumber(6)
  ProviderUsage ensureUsage() => $_ensure(5);

  /// The signed-in subscription's plan as the provider names it, such as
  /// "Claude Max 20x" or "ChatGPT Pro". Empty when it isn't signed in with a
  /// subscription, or the plan hasn't been read or the provider doesn't say.
  @$pb.TagNumber(7)
  $core.String get plan => $_getSZ(6);
  @$pb.TagNumber(7)
  set plan($core.String value) => $_setString(6, value);
  @$pb.TagNumber(7)
  $core.bool hasPlan() => $_has(6);
  @$pb.TagNumber(7)
  void clearPlan() => $_clearField(7);

  /// Every account signed in to it, in the order requests try them. The fields
  /// above describe the first, as before several accounts could be signed in.
  @$pb.TagNumber(8)
  $pb.PbList<Account> get accounts => $_getList(7);

  /// How long its prompt cache is asked to last now, "5m" or "1h" (models with
  /// their own setting may differ; see Settings.cache_ttls). Empty when
  /// Claktory can't choose it for this provider and doesn't know how long it
  /// lasts: its agents never compact while idle.
  @$pb.TagNumber(9)
  $core.String get cacheTtl => $_getSZ(8);
  @$pb.TagNumber(9)
  set cacheTtl($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasCacheTtl() => $_has(8);
  @$pb.TagNumber(9)
  void clearCacheTtl() => $_clearField(9);
}

/// One account signed in to a provider, such as one of two Claude
/// subscriptions. Requests go to the first account with capacity; when one
/// reaches its usage limit they carry on with the next.
class Account extends $pb.GeneratedMessage {
  factory Account({
    $core.String? id,
    $core.String? label,
    $core.String? plan,
    ProviderUsage? usage,
    $core.bool? canSignOut,
    $core.bool? inUse,
    $fixnum.Int64? exhaustedUntil,
    $core.bool? outOfCredit,
    $core.String? methodId,
  }) {
    final result = Account._();
    if (id != null) result.id = id;
    if (label != null) result.label = label;
    if (plan != null) result.plan = plan;
    if (usage != null) result.usage = usage;
    if (canSignOut != null) result.canSignOut = canSignOut;
    if (inUse != null) result.inUse = inUse;
    if (exhaustedUntil != null) result.exhaustedUntil = exhaustedUntil;
    if (outOfCredit != null) result.outOfCredit = outOfCredit;
    if (methodId != null) result.methodId = methodId;
    return result;
  }

  Account._();

  factory Account.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Account()..mergeFromBuffer(data, registry);
  factory Account.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Account()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Account',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Account.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'label')
    ..aOS(3, _omitFieldNames ? '' : 'plan')
    ..aOM<ProviderUsage>(4, _omitFieldNames ? '' : 'usage',
        subBuilder: ProviderUsage.$_createMessage)
    ..aOB(5, _omitFieldNames ? '' : 'canSignOut')
    ..aOB(6, _omitFieldNames ? '' : 'inUse')
    ..aInt64(7, _omitFieldNames ? '' : 'exhaustedUntil')
    ..aOB(8, _omitFieldNames ? '' : 'outOfCredit')
    ..aOS(9, _omitFieldNames ? '' : 'methodId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Account clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Account copyWith(void Function(Account) updates) =>
      super.copyWith((message) => updates(message as Account)) as Account;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Account() / Account.new instead')
  static Account create() => Account._();
  static $pb.GeneratedMessage $_createMessage() => Account._();
  @$core.override
  Account createEmptyInstance() => Account._();
  @$core.pragma('dart2js:noInline')
  static Account getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Account>(Account.$_createMessage);
  static Account? _defaultInstance;

  /// Made by Factory, stable for as long as the account is signed in.
  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// Who it is: a subscription's email, as the provider says (empty until
  /// known); an API key's name, given with `UpdateAccount.label`, or else
  /// its last characters ("sk-…a1b2"); a key from the environment, the
  /// variable's name.
  @$pb.TagNumber(2)
  $core.String get label => $_getSZ(1);
  @$pb.TagNumber(2)
  set label($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasLabel() => $_has(1);
  @$pb.TagNumber(2)
  void clearLabel() => $_clearField(2);

  /// Its plan, such as "Claude Max 20x"; "API key" for a key; empty when
  /// unknown.
  @$pb.TagNumber(3)
  $core.String get plan => $_getSZ(2);
  @$pb.TagNumber(3)
  set plan($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasPlan() => $_has(2);
  @$pb.TagNumber(3)
  void clearPlan() => $_clearField(3);

  /// How much of its usage limits it has used; unset when it has none to read.
  @$pb.TagNumber(4)
  ProviderUsage get usage => $_getN(3);
  @$pb.TagNumber(4)
  set usage(ProviderUsage value) => $_setField(4, value);
  @$pb.TagNumber(4)
  $core.bool hasUsage() => $_has(3);
  @$pb.TagNumber(4)
  void clearUsage() => $_clearField(4);
  @$pb.TagNumber(4)
  ProviderUsage ensureUsage() => $_ensure(3);

  /// Factory stored it and `SignOut` removes it (false for an API key from the
  /// environment).
  @$pb.TagNumber(5)
  $core.bool get canSignOut => $_getBF(4);
  @$pb.TagNumber(5)
  set canSignOut($core.bool value) => $_setBool(4, value);
  @$pb.TagNumber(5)
  $core.bool hasCanSignOut() => $_has(4);
  @$pb.TagNumber(5)
  void clearCanSignOut() => $_clearField(5);

  /// Requests go to it now.
  @$pb.TagNumber(6)
  $core.bool get inUse => $_getBF(5);
  @$pb.TagNumber(6)
  set inUse($core.bool value) => $_setBool(5, value);
  @$pb.TagNumber(6)
  $core.bool hasInUse() => $_has(5);
  @$pb.TagNumber(6)
  void clearInUse() => $_clearField(6);

  /// It reached a usage limit: requests skip it until then, in ms since 1970;
  /// 0 when it has capacity.
  @$pb.TagNumber(7)
  $fixnum.Int64 get exhaustedUntil => $_getI64(6);
  @$pb.TagNumber(7)
  set exhaustedUntil($fixnum.Int64 value) => $_setInt64(6, value);
  @$pb.TagNumber(7)
  $core.bool hasExhaustedUntil() => $_has(6);
  @$pb.TagNumber(7)
  void clearExhaustedUntil() => $_clearField(7);

  /// An API key that ran out of credit: requests skip it until it's tried
  /// again, by itself every hour or with `UpdateAccount.retry`. Its
  /// `exhausted_until` stays 0, since nothing says when it gets credit.
  @$pb.TagNumber(8)
  $core.bool get outOfCredit => $_getBF(7);
  @$pb.TagNumber(8)
  set outOfCredit($core.bool value) => $_setBool(7, value);
  @$pb.TagNumber(8)
  $core.bool hasOutOfCredit() => $_has(7);
  @$pb.TagNumber(8)
  void clearOutOfCredit() => $_clearField(8);

  /// How it's signed in: the `LoginMethod` id, "oauth" (a subscription or
  /// OAuth account) or "api_key" (a key, stored or from the environment).
  @$pb.TagNumber(9)
  $core.String get methodId => $_getSZ(8);
  @$pb.TagNumber(9)
  set methodId($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasMethodId() => $_has(8);
  @$pb.TagNumber(9)
  void clearMethodId() => $_clearField(9);
}

class ProviderUsage extends $pb.GeneratedMessage {
  factory ProviderUsage({
    $core.Iterable<UsageWindow>? windows,
    $fixnum.Int64? updated,
    $core.String? error,
  }) {
    final result = ProviderUsage._();
    if (windows != null) result.windows.addAll(windows);
    if (updated != null) result.updated = updated;
    if (error != null) result.error = error;
    return result;
  }

  ProviderUsage._();

  factory ProviderUsage.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ProviderUsage()..mergeFromBuffer(data, registry);
  factory ProviderUsage.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ProviderUsage()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ProviderUsage',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ProviderUsage.$_createMessage)
    ..pPM<UsageWindow>(1, _omitFieldNames ? '' : 'windows',
        subBuilder: UsageWindow.$_createMessage)
    ..aInt64(2, _omitFieldNames ? '' : 'updated')
    ..aOS(3, _omitFieldNames ? '' : 'error')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ProviderUsage clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ProviderUsage copyWith(void Function(ProviderUsage) updates) =>
      super.copyWith((message) => updates(message as ProviderUsage))
          as ProviderUsage;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ProviderUsage() / ProviderUsage.new instead')
  static ProviderUsage create() => ProviderUsage._();
  static $pb.GeneratedMessage $_createMessage() => ProviderUsage._();
  @$core.override
  ProviderUsage createEmptyInstance() => ProviderUsage._();
  @$core.pragma('dart2js:noInline')
  static ProviderUsage getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ProviderUsage>(
          ProviderUsage.$_createMessage);
  static ProviderUsage? _defaultInstance;

  /// The provider's limits; the one with id "weekly" (all models, per week) is
  /// the headline.
  @$pb.TagNumber(1)
  $pb.PbList<UsageWindow> get windows => $_getList(0);

  /// When usage was last read, in ms since 1970; 0 if it never was.
  @$pb.TagNumber(2)
  $fixnum.Int64 get updated => $_getI64(1);
  @$pb.TagNumber(2)
  set updated($fixnum.Int64 value) => $_setInt64(1, value);
  @$pb.TagNumber(2)
  $core.bool hasUpdated() => $_has(1);
  @$pb.TagNumber(2)
  void clearUpdated() => $_clearField(2);

  /// Why the latest read failed; empty when it worked. `windows` and `updated`
  /// then still hold the last good read, if there was one.
  @$pb.TagNumber(3)
  $core.String get error => $_getSZ(2);
  @$pb.TagNumber(3)
  set error($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasError() => $_has(2);
  @$pb.TagNumber(3)
  void clearError() => $_clearField(3);
}

/// One usage limit, such as the weekly or the 5-hour one.
class UsageWindow extends $pb.GeneratedMessage {
  factory UsageWindow({
    $core.String? id,
    $core.String? label,
    $core.double? usedPercent,
    $fixnum.Int64? resetsAt,
  }) {
    final result = UsageWindow._();
    if (id != null) result.id = id;
    if (label != null) result.label = label;
    if (usedPercent != null) result.usedPercent = usedPercent;
    if (resetsAt != null) result.resetsAt = resetsAt;
    return result;
  }

  UsageWindow._();

  factory UsageWindow.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UsageWindow()..mergeFromBuffer(data, registry);
  factory UsageWindow.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UsageWindow()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'UsageWindow',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: UsageWindow.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'label')
    ..aD(3, _omitFieldNames ? '' : 'usedPercent')
    ..aInt64(4, _omitFieldNames ? '' : 'resetsAt')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UsageWindow clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UsageWindow copyWith(void Function(UsageWindow) updates) =>
      super.copyWith((message) => updates(message as UsageWindow))
          as UsageWindow;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use UsageWindow() / UsageWindow.new instead')
  static UsageWindow create() => UsageWindow._();
  static $pb.GeneratedMessage $_createMessage() => UsageWindow._();
  @$core.override
  UsageWindow createEmptyInstance() => UsageWindow._();
  @$core.pragma('dart2js:noInline')
  static UsageWindow getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<UsageWindow>(
          UsageWindow.$_createMessage);
  static UsageWindow? _defaultInstance;

  /// "weekly", "five_hour", "weekly_opus", "weekly_sonnet", ...
  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// Shown as is, e.g. "Weekly", "5-hour", "Weekly (Opus)".
  @$pb.TagNumber(2)
  $core.String get label => $_getSZ(1);
  @$pb.TagNumber(2)
  set label($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasLabel() => $_has(1);
  @$pb.TagNumber(2)
  void clearLabel() => $_clearField(2);

  /// 0-100.
  @$pb.TagNumber(3)
  $core.double get usedPercent => $_getN(2);
  @$pb.TagNumber(3)
  set usedPercent($core.double value) => $_setDouble(2, value);
  @$pb.TagNumber(3)
  $core.bool hasUsedPercent() => $_has(2);
  @$pb.TagNumber(3)
  void clearUsedPercent() => $_clearField(3);

  /// When the window resets, in ms since 1970; 0 if unknown.
  @$pb.TagNumber(4)
  $fixnum.Int64 get resetsAt => $_getI64(3);
  @$pb.TagNumber(4)
  set resetsAt($fixnum.Int64 value) => $_setInt64(3, value);
  @$pb.TagNumber(4)
  $core.bool hasResetsAt() => $_has(3);
  @$pb.TagNumber(4)
  void clearResetsAt() => $_clearField(4);
}

class LoginMethod extends $pb.GeneratedMessage {
  factory LoginMethod({
    $core.String? id,
    $core.String? label,
  }) {
    final result = LoginMethod._();
    if (id != null) result.id = id;
    if (label != null) result.label = label;
    return result;
  }

  LoginMethod._();

  factory LoginMethod.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      LoginMethod()..mergeFromBuffer(data, registry);
  factory LoginMethod.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      LoginMethod()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'LoginMethod',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: LoginMethod.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'label')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  LoginMethod clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  LoginMethod copyWith(void Function(LoginMethod) updates) =>
      super.copyWith((message) => updates(message as LoginMethod))
          as LoginMethod;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use LoginMethod() / LoginMethod.new instead')
  static LoginMethod create() => LoginMethod._();
  static $pb.GeneratedMessage $_createMessage() => LoginMethod._();
  @$core.override
  LoginMethod createEmptyInstance() => LoginMethod._();
  @$core.pragma('dart2js:noInline')
  static LoginMethod getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<LoginMethod>(
          LoginMethod.$_createMessage);
  static LoginMethod? _defaultInstance;

  /// "oauth" or "api_key".
  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// e.g. "Anthropic (Claude Pro/Max)", "Anthropic API key".
  @$pb.TagNumber(2)
  $core.String get label => $_getSZ(1);
  @$pb.TagNumber(2)
  set label($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasLabel() => $_has(1);
  @$pb.TagNumber(2)
  void clearLabel() => $_clearField(2);
}

class LoginFlow extends $pb.GeneratedMessage {
  factory LoginFlow({
    $core.String? providerId,
    $core.String? methodId,
    $core.String? url,
    $core.String? instructions,
    $core.String? userCode,
    $core.String? verificationUrl,
    $core.String? progress,
    LoginPrompt? prompt,
    $core.String? error,
  }) {
    final result = LoginFlow._();
    if (providerId != null) result.providerId = providerId;
    if (methodId != null) result.methodId = methodId;
    if (url != null) result.url = url;
    if (instructions != null) result.instructions = instructions;
    if (userCode != null) result.userCode = userCode;
    if (verificationUrl != null) result.verificationUrl = verificationUrl;
    if (progress != null) result.progress = progress;
    if (prompt != null) result.prompt = prompt;
    if (error != null) result.error = error;
    return result;
  }

  LoginFlow._();

  factory LoginFlow.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      LoginFlow()..mergeFromBuffer(data, registry);
  factory LoginFlow.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      LoginFlow()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'LoginFlow',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: LoginFlow.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'providerId')
    ..aOS(2, _omitFieldNames ? '' : 'methodId')
    ..aOS(3, _omitFieldNames ? '' : 'url')
    ..aOS(4, _omitFieldNames ? '' : 'instructions')
    ..aOS(5, _omitFieldNames ? '' : 'userCode')
    ..aOS(6, _omitFieldNames ? '' : 'verificationUrl')
    ..aOS(7, _omitFieldNames ? '' : 'progress')
    ..aOM<LoginPrompt>(8, _omitFieldNames ? '' : 'prompt',
        subBuilder: LoginPrompt.$_createMessage)
    ..aOS(9, _omitFieldNames ? '' : 'error')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  LoginFlow clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  LoginFlow copyWith(void Function(LoginFlow) updates) =>
      super.copyWith((message) => updates(message as LoginFlow)) as LoginFlow;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use LoginFlow() / LoginFlow.new instead')
  static LoginFlow create() => LoginFlow._();
  static $pb.GeneratedMessage $_createMessage() => LoginFlow._();
  @$core.override
  LoginFlow createEmptyInstance() => LoginFlow._();
  @$core.pragma('dart2js:noInline')
  static LoginFlow getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<LoginFlow>(LoginFlow.$_createMessage);
  static LoginFlow? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get providerId => $_getSZ(0);
  @$pb.TagNumber(1)
  set providerId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProviderId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProviderId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get methodId => $_getSZ(1);
  @$pb.TagNumber(2)
  set methodId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasMethodId() => $_has(1);
  @$pb.TagNumber(2)
  void clearMethodId() => $_clearField(2);

  /// Open this to sign in.
  @$pb.TagNumber(3)
  $core.String get url => $_getSZ(2);
  @$pb.TagNumber(3)
  set url($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasUrl() => $_has(2);
  @$pb.TagNumber(3)
  void clearUrl() => $_clearField(3);

  /// What to do with `url`.
  @$pb.TagNumber(4)
  $core.String get instructions => $_getSZ(3);
  @$pb.TagNumber(4)
  set instructions($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasInstructions() => $_has(3);
  @$pb.TagNumber(4)
  void clearInstructions() => $_clearField(4);

  /// Device-code sign-in: enter `user_code` at `verification_url`.
  @$pb.TagNumber(5)
  $core.String get userCode => $_getSZ(4);
  @$pb.TagNumber(5)
  set userCode($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasUserCode() => $_has(4);
  @$pb.TagNumber(5)
  void clearUserCode() => $_clearField(5);

  @$pb.TagNumber(6)
  $core.String get verificationUrl => $_getSZ(5);
  @$pb.TagNumber(6)
  set verificationUrl($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasVerificationUrl() => $_has(5);
  @$pb.TagNumber(6)
  void clearVerificationUrl() => $_clearField(6);

  /// Latest status message from the provider.
  @$pb.TagNumber(7)
  $core.String get progress => $_getSZ(6);
  @$pb.TagNumber(7)
  set progress($core.String value) => $_setString(6, value);
  @$pb.TagNumber(7)
  $core.bool hasProgress() => $_has(6);
  @$pb.TagNumber(7)
  void clearProgress() => $_clearField(7);

  /// A question waiting for `AnswerLogin`; unset when none.
  @$pb.TagNumber(8)
  LoginPrompt get prompt => $_getN(7);
  @$pb.TagNumber(8)
  set prompt(LoginPrompt value) => $_setField(8, value);
  @$pb.TagNumber(8)
  $core.bool hasPrompt() => $_has(7);
  @$pb.TagNumber(8)
  void clearPrompt() => $_clearField(8);
  @$pb.TagNumber(8)
  LoginPrompt ensurePrompt() => $_ensure(7);

  /// Sign-in failed. The flow stays until `CancelLogin` dismisses it.
  @$pb.TagNumber(9)
  $core.String get error => $_getSZ(8);
  @$pb.TagNumber(9)
  set error($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasError() => $_has(8);
  @$pb.TagNumber(9)
  void clearError() => $_clearField(9);
}

class LoginPrompt extends $pb.GeneratedMessage {
  factory LoginPrompt({
    LoginPromptKind? kind,
    $core.String? message,
    $core.String? placeholder,
    $core.Iterable<LoginOption>? options,
  }) {
    final result = LoginPrompt._();
    if (kind != null) result.kind = kind;
    if (message != null) result.message = message;
    if (placeholder != null) result.placeholder = placeholder;
    if (options != null) result.options.addAll(options);
    return result;
  }

  LoginPrompt._();

  factory LoginPrompt.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      LoginPrompt()..mergeFromBuffer(data, registry);
  factory LoginPrompt.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      LoginPrompt()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'LoginPrompt',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: LoginPrompt.$_createMessage)
    ..aE<LoginPromptKind>(1, _omitFieldNames ? '' : 'kind',
        enumValues: LoginPromptKind.values)
    ..aOS(2, _omitFieldNames ? '' : 'message')
    ..aOS(3, _omitFieldNames ? '' : 'placeholder')
    ..pPM<LoginOption>(4, _omitFieldNames ? '' : 'options',
        subBuilder: LoginOption.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  LoginPrompt clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  LoginPrompt copyWith(void Function(LoginPrompt) updates) =>
      super.copyWith((message) => updates(message as LoginPrompt))
          as LoginPrompt;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use LoginPrompt() / LoginPrompt.new instead')
  static LoginPrompt create() => LoginPrompt._();
  static $pb.GeneratedMessage $_createMessage() => LoginPrompt._();
  @$core.override
  LoginPrompt createEmptyInstance() => LoginPrompt._();
  @$core.pragma('dart2js:noInline')
  static LoginPrompt getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<LoginPrompt>(
          LoginPrompt.$_createMessage);
  static LoginPrompt? _defaultInstance;

  @$pb.TagNumber(1)
  LoginPromptKind get kind => $_getN(0);
  @$pb.TagNumber(1)
  set kind(LoginPromptKind value) => $_setField(1, value);
  @$pb.TagNumber(1)
  $core.bool hasKind() => $_has(0);
  @$pb.TagNumber(1)
  void clearKind() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get message => $_getSZ(1);
  @$pb.TagNumber(2)
  set message($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasMessage() => $_has(1);
  @$pb.TagNumber(2)
  void clearMessage() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.String get placeholder => $_getSZ(2);
  @$pb.TagNumber(3)
  set placeholder($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasPlaceholder() => $_has(2);
  @$pb.TagNumber(3)
  void clearPlaceholder() => $_clearField(3);

  /// For `SELECT`.
  @$pb.TagNumber(4)
  $pb.PbList<LoginOption> get options => $_getList(3);
}

class LoginOption extends $pb.GeneratedMessage {
  factory LoginOption({
    $core.String? id,
    $core.String? label,
  }) {
    final result = LoginOption._();
    if (id != null) result.id = id;
    if (label != null) result.label = label;
    return result;
  }

  LoginOption._();

  factory LoginOption.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      LoginOption()..mergeFromBuffer(data, registry);
  factory LoginOption.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      LoginOption()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'LoginOption',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: LoginOption.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'label')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  LoginOption clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  LoginOption copyWith(void Function(LoginOption) updates) =>
      super.copyWith((message) => updates(message as LoginOption))
          as LoginOption;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use LoginOption() / LoginOption.new instead')
  static LoginOption create() => LoginOption._();
  static $pb.GeneratedMessage $_createMessage() => LoginOption._();
  @$core.override
  LoginOption createEmptyInstance() => LoginOption._();
  @$core.pragma('dart2js:noInline')
  static LoginOption getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<LoginOption>(
          LoginOption.$_createMessage);
  static LoginOption? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get label => $_getSZ(1);
  @$pb.TagNumber(2)
  set label($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasLabel() => $_has(1);
  @$pb.TagNumber(2)
  void clearLabel() => $_clearField(2);
}

class Project extends $pb.GeneratedMessage {
  factory Project({
    $core.String? id,
    $core.String? name,
    $core.String? path,
    $core.Iterable<ProjectFolder>? folders,
    $core.bool? archived,
  }) {
    final result = Project._();
    if (id != null) result.id = id;
    if (name != null) result.name = name;
    if (path != null) result.path = path;
    if (folders != null) result.folders.addAll(folders);
    if (archived != null) result.archived = archived;
    return result;
  }

  Project._();

  factory Project.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Project()..mergeFromBuffer(data, registry);
  factory Project.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Project()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Project',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Project.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'path')
    ..pPM<ProjectFolder>(4, _omitFieldNames ? '' : 'folders',
        subBuilder: ProjectFolder.$_createMessage)
    ..aOB(5, _omitFieldNames ? '' : 'archived')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Project clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Project copyWith(void Function(Project) updates) =>
      super.copyWith((message) => updates(message as Project)) as Project;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Project() / Project.new instead')
  static Project create() => Project._();
  static $pb.GeneratedMessage $_createMessage() => Project._();
  @$core.override
  Project createEmptyInstance() => Project._();
  @$core.pragma('dart2js:noInline')
  static Project getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Project>(Project.$_createMessage);
  static Project? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// Absolute path of the project's folder.
  @$pb.TagNumber(3)
  $core.String get path => $_getSZ(2);
  @$pb.TagNumber(3)
  set path($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasPath() => $_has(2);
  @$pb.TagNumber(3)
  void clearPath() => $_clearField(3);

  /// Folders inside the project, each where some of its agents work (see
  /// Agent.folder_id). In the person's order.
  @$pb.TagNumber(4)
  $pb.PbList<ProjectFolder> get folders => $_getList(3);

  /// Archived: put away, with nothing deleted. Its agents stay in
  /// Workspace.agents with their conversations, checklists, memory and
  /// files, but don't run: one that was working is stopped, and messages and
  /// events for them wait, queued, until the project is unarchived. Their
  /// monitors pause, other agents can't message them, and the Map leaves the
  /// project out. Unarchived, all of that resumes as it was.
  @$pb.TagNumber(5)
  $core.bool get archived => $_getBF(4);
  @$pb.TagNumber(5)
  set archived($core.bool value) => $_setBool(4, value);
  @$pb.TagNumber(5)
  $core.bool hasArchived() => $_has(4);
  @$pb.TagNumber(5)
  void clearArchived() => $_clearField(5);
}

/// A folder that some of a project's agents work in. It only scopes where they
/// work: messaging and everything else stay project-wide.
class ProjectFolder extends $pb.GeneratedMessage {
  factory ProjectFolder({
    $core.String? id,
    $core.String? name,
    $core.String? path,
    $core.bool? collapsed,
  }) {
    final result = ProjectFolder._();
    if (id != null) result.id = id;
    if (name != null) result.name = name;
    if (path != null) result.path = path;
    if (collapsed != null) result.collapsed = collapsed;
    return result;
  }

  ProjectFolder._();

  factory ProjectFolder.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ProjectFolder()..mergeFromBuffer(data, registry);
  factory ProjectFolder.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ProjectFolder()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ProjectFolder',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ProjectFolder.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'path')
    ..aOB(4, _omitFieldNames ? '' : 'collapsed')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ProjectFolder clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ProjectFolder copyWith(void Function(ProjectFolder) updates) =>
      super.copyWith((message) => updates(message as ProjectFolder))
          as ProjectFolder;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ProjectFolder() / ProjectFolder.new instead')
  static ProjectFolder create() => ProjectFolder._();
  static $pb.GeneratedMessage $_createMessage() => ProjectFolder._();
  @$core.override
  ProjectFolder createEmptyInstance() => ProjectFolder._();
  @$core.pragma('dart2js:noInline')
  static ProjectFolder getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ProjectFolder>(
          ProjectFolder.$_createMessage);
  static ProjectFolder? _defaultInstance;

  /// Unique within the project.
  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// Absolute; may be outside the project's folder.
  @$pb.TagNumber(3)
  $core.String get path => $_getSZ(2);
  @$pb.TagNumber(3)
  set path($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasPath() => $_has(2);
  @$pb.TagNumber(3)
  void clearPath() => $_clearField(3);

  /// Its agents are folded away in the sidebar.
  @$pb.TagNumber(4)
  $core.bool get collapsed => $_getBF(3);
  @$pb.TagNumber(4)
  set collapsed($core.bool value) => $_setBool(3, value);
  @$pb.TagNumber(4)
  $core.bool hasCollapsed() => $_has(3);
  @$pb.TagNumber(4)
  void clearCollapsed() => $_clearField(4);
}

class Agent extends $pb.GeneratedMessage {
  factory Agent({
    $core.String? id,
    $core.String? projectId,
    $core.String? name,
    $core.String? prompt,
    $core.String? model,
    $core.String? effort,
    AgentStatus? status,
    $fixnum.Int64? contextTokens,
    $fixnum.Int64? compactAtTokens,
    $core.int? queuedMessages,
    $core.bool? modelAvailable,
    $core.String? color,
    $core.bool? unseen,
    $core.Iterable<AgentExtension>? extensions,
    $core.String? parentId,
    $core.bool? collapsed,
    $core.Iterable<SlashCommand>? commands,
    $core.String? placeholder,
    $core.String? folder,
    $core.String? folderId,
    $core.String? openPanel,
    $core.String? avatarShape,
    $core.String? avatarFace,
    $core.int? characterSeed,
    CharacterMood? characterMood,
    $fixnum.Int64? characterMoodSinceMs,
    $core.String? avatarShapeOverride,
    $core.String? avatarFaceOverride,
    $core.String? role,
    ViewNode? layout,
    $core.String? defaultPrompt,
  }) {
    final result = Agent._();
    if (id != null) result.id = id;
    if (projectId != null) result.projectId = projectId;
    if (name != null) result.name = name;
    if (prompt != null) result.prompt = prompt;
    if (model != null) result.model = model;
    if (effort != null) result.effort = effort;
    if (status != null) result.status = status;
    if (contextTokens != null) result.contextTokens = contextTokens;
    if (compactAtTokens != null) result.compactAtTokens = compactAtTokens;
    if (queuedMessages != null) result.queuedMessages = queuedMessages;
    if (modelAvailable != null) result.modelAvailable = modelAvailable;
    if (color != null) result.color = color;
    if (unseen != null) result.unseen = unseen;
    if (extensions != null) result.extensions.addAll(extensions);
    if (parentId != null) result.parentId = parentId;
    if (collapsed != null) result.collapsed = collapsed;
    if (commands != null) result.commands.addAll(commands);
    if (placeholder != null) result.placeholder = placeholder;
    if (folder != null) result.folder = folder;
    if (folderId != null) result.folderId = folderId;
    if (openPanel != null) result.openPanel = openPanel;
    if (avatarShape != null) result.avatarShape = avatarShape;
    if (avatarFace != null) result.avatarFace = avatarFace;
    if (characterSeed != null) result.characterSeed = characterSeed;
    if (characterMood != null) result.characterMood = characterMood;
    if (characterMoodSinceMs != null)
      result.characterMoodSinceMs = characterMoodSinceMs;
    if (avatarShapeOverride != null)
      result.avatarShapeOverride = avatarShapeOverride;
    if (avatarFaceOverride != null)
      result.avatarFaceOverride = avatarFaceOverride;
    if (role != null) result.role = role;
    if (layout != null) result.layout = layout;
    if (defaultPrompt != null) result.defaultPrompt = defaultPrompt;
    return result;
  }

  Agent._();

  factory Agent.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Agent()..mergeFromBuffer(data, registry);
  factory Agent.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Agent()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Agent',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Agent.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'projectId')
    ..aOS(3, _omitFieldNames ? '' : 'name')
    ..aOS(4, _omitFieldNames ? '' : 'prompt')
    ..aOS(5, _omitFieldNames ? '' : 'model')
    ..aOS(6, _omitFieldNames ? '' : 'effort')
    ..aE<AgentStatus>(7, _omitFieldNames ? '' : 'status',
        enumValues: AgentStatus.values)
    ..a<$fixnum.Int64>(
        8, _omitFieldNames ? '' : 'contextTokens', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..a<$fixnum.Int64>(
        9, _omitFieldNames ? '' : 'compactAtTokens', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..aI(10, _omitFieldNames ? '' : 'queuedMessages',
        fieldType: $pb.PbFieldType.OU3)
    ..aOB(11, _omitFieldNames ? '' : 'modelAvailable')
    ..aOS(12, _omitFieldNames ? '' : 'color')
    ..aOB(13, _omitFieldNames ? '' : 'unseen')
    ..pPM<AgentExtension>(14, _omitFieldNames ? '' : 'extensions',
        subBuilder: AgentExtension.$_createMessage)
    ..aOS(15, _omitFieldNames ? '' : 'parentId')
    ..aOB(16, _omitFieldNames ? '' : 'collapsed')
    ..pPM<SlashCommand>(17, _omitFieldNames ? '' : 'commands',
        subBuilder: SlashCommand.$_createMessage)
    ..aOS(18, _omitFieldNames ? '' : 'placeholder')
    ..aOS(19, _omitFieldNames ? '' : 'folder')
    ..aOS(20, _omitFieldNames ? '' : 'folderId')
    ..aOS(21, _omitFieldNames ? '' : 'openPanel')
    ..aOS(22, _omitFieldNames ? '' : 'avatarShape')
    ..aOS(23, _omitFieldNames ? '' : 'avatarFace')
    ..aI(24, _omitFieldNames ? '' : 'characterSeed',
        fieldType: $pb.PbFieldType.OU3)
    ..aE<CharacterMood>(25, _omitFieldNames ? '' : 'characterMood',
        enumValues: CharacterMood.values)
    ..aInt64(26, _omitFieldNames ? '' : 'characterMoodSinceMs')
    ..aOS(27, _omitFieldNames ? '' : 'avatarShapeOverride')
    ..aOS(28, _omitFieldNames ? '' : 'avatarFaceOverride')
    ..aOS(29, _omitFieldNames ? '' : 'role')
    ..aOM<ViewNode>(30, _omitFieldNames ? '' : 'layout',
        subBuilder: ViewNode.$_createMessage)
    ..aOS(31, _omitFieldNames ? '' : 'defaultPrompt')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Agent clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Agent copyWith(void Function(Agent) updates) =>
      super.copyWith((message) => updates(message as Agent)) as Agent;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Agent() / Agent.new instead')
  static Agent create() => Agent._();
  static $pb.GeneratedMessage $_createMessage() => Agent._();
  @$core.override
  Agent createEmptyInstance() => Agent._();
  @$core.pragma('dart2js:noInline')
  static Agent getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Agent>(Agent.$_createMessage);
  static Agent? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// Empty for an agent in the workspace, outside any project.
  @$pb.TagNumber(2)
  $core.String get projectId => $_getSZ(1);
  @$pb.TagNumber(2)
  set projectId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasProjectId() => $_has(1);
  @$pb.TagNumber(2)
  void clearProjectId() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.String get name => $_getSZ(2);
  @$pb.TagNumber(3)
  set name($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasName() => $_has(2);
  @$pb.TagNumber(3)
  void clearName() => $_clearField(3);

  /// The agent's prompt: instructions added to its system prompt.
  @$pb.TagNumber(4)
  $core.String get prompt => $_getSZ(3);
  @$pb.TagNumber(4)
  set prompt($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasPrompt() => $_has(3);
  @$pb.TagNumber(4)
  void clearPrompt() => $_clearField(4);

  /// "<provider>/<model id>", e.g. "anthropic/claude-opus-5-5".
  @$pb.TagNumber(5)
  $core.String get model => $_getSZ(4);
  @$pb.TagNumber(5)
  set model($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasModel() => $_has(4);
  @$pb.TagNumber(5)
  void clearModel() => $_clearField(5);

  /// Reasoning effort: one of the model's `efforts`.
  @$pb.TagNumber(6)
  $core.String get effort => $_getSZ(5);
  @$pb.TagNumber(6)
  set effort($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasEffort() => $_has(5);
  @$pb.TagNumber(6)
  void clearEffort() => $_clearField(6);

  @$pb.TagNumber(7)
  AgentStatus get status => $_getN(6);
  @$pb.TagNumber(7)
  set status(AgentStatus value) => $_setField(7, value);
  @$pb.TagNumber(7)
  $core.bool hasStatus() => $_has(6);
  @$pb.TagNumber(7)
  void clearStatus() => $_clearField(7);

  /// Tokens the agent's context held after its last model call.
  @$pb.TagNumber(8)
  $fixnum.Int64 get contextTokens => $_getI64(7);
  @$pb.TagNumber(8)
  set contextTokens($fixnum.Int64 value) => $_setInt64(7, value);
  @$pb.TagNumber(8)
  $core.bool hasContextTokens() => $_has(7);
  @$pb.TagNumber(8)
  void clearContextTokens() => $_clearField(8);

  /// The context is compacted when it reaches this many tokens.
  @$pb.TagNumber(9)
  $fixnum.Int64 get compactAtTokens => $_getI64(8);
  @$pb.TagNumber(9)
  set compactAtTokens($fixnum.Int64 value) => $_setInt64(8, value);
  @$pb.TagNumber(9)
  $core.bool hasCompactAtTokens() => $_has(8);
  @$pb.TagNumber(9)
  void clearCompactAtTokens() => $_clearField(9);

  /// Messages waiting for the current run to finish.
  @$pb.TagNumber(10)
  $core.int get queuedMessages => $_getIZ(9);
  @$pb.TagNumber(10)
  set queuedMessages($core.int value) => $_setUnsignedInt32(9, value);
  @$pb.TagNumber(10)
  $core.bool hasQueuedMessages() => $_has(9);
  @$pb.TagNumber(10)
  void clearQueuedMessages() => $_clearField(10);

  /// `model` can run now: its provider is signed in. When false the agent has
  /// no model, and the app says so instead of showing `model`.
  @$pb.TagNumber(11)
  $core.bool get modelAvailable => $_getBF(10);
  @$pb.TagNumber(11)
  set modelAvailable($core.bool value) => $_setBool(10, value);
  @$pb.TagNumber(11)
  $core.bool hasModelAvailable() => $_has(10);
  @$pb.TagNumber(11)
  void clearModelAvailable() => $_clearField(11);

  /// The agent's color, which marks its messages and its bar: "blue",
  /// "violet", "green", "pink", "teal", "amber", "red" or "slate".
  @$pb.TagNumber(12)
  $core.String get color => $_getSZ(11);
  @$pb.TagNumber(12)
  set color($core.String value) => $_setString(11, value);
  @$pb.TagNumber(12)
  $core.bool hasColor() => $_has(11);
  @$pb.TagNumber(12)
  void clearColor() => $_clearField(12);

  /// A run of the agent finished (or failed) since the person last opened it:
  /// there's something to look at. Selecting the agent clears it; a run that
  /// ends while the agent is selected, or that was stopped, doesn't set it.
  @$pb.TagNumber(13)
  $core.bool get unseen => $_getBF(12);
  @$pb.TagNumber(13)
  set unseen($core.bool value) => $_setBool(12, value);
  @$pb.TagNumber(13)
  $core.bool hasUnseen() => $_has(12);
  @$pb.TagNumber(13)
  void clearUnseen() => $_clearField(13);

  /// One per extension in `Workspace.extensions`, in that order.
  @$pb.TagNumber(14)
  $pb.PbList<AgentExtension> get extensions => $_getList(13);

  /// The agent it's nested under in the sidebar, in the same project or
  /// workspace; empty at the top level. Purely visual. Agents come in sidebar
  /// order: within a project (or the workspace), siblings in list order.
  @$pb.TagNumber(15)
  $core.String get parentId => $_getSZ(14);
  @$pb.TagNumber(15)
  set parentId($core.String value) => $_setString(14, value);
  @$pb.TagNumber(15)
  $core.bool hasParentId() => $_has(14);
  @$pb.TagNumber(15)
  void clearParentId() => $_clearField(15);

  /// Its children are folded away in the sidebar.
  @$pb.TagNumber(16)
  $core.bool get collapsed => $_getBF(15);
  @$pb.TagNumber(16)
  set collapsed($core.bool value) => $_setBool(15, value);
  @$pb.TagNumber(16)
  $core.bool hasCollapsed() => $_has(15);
  @$pb.TagNumber(16)
  void clearCollapsed() => $_clearField(16);

  /// The slash commands the person can run for this agent, in the order to
  /// offer them.
  @$pb.TagNumber(17)
  $pb.PbList<SlashCommand> get commands => $_getList(16);

  /// What its message box says when empty, e.g. what the agent is for. Empty
  /// means the app shows "Message <name>". Display only.
  @$pb.TagNumber(18)
  $core.String get placeholder => $_getSZ(17);
  @$pb.TagNumber(18)
  set placeholder($core.String value) => $_setString(17, value);
  @$pb.TagNumber(18)
  $core.bool hasPlaceholder() => $_has(17);
  @$pb.TagNumber(18)
  void clearPlaceholder() => $_clearField(18);

  /// Deprecated, always empty: see folder_id.
  @$pb.TagNumber(19)
  $core.String get folder => $_getSZ(18);
  @$pb.TagNumber(19)
  set folder($core.String value) => $_setString(18, value);
  @$pb.TagNumber(19)
  $core.bool hasFolder() => $_has(18);
  @$pb.TagNumber(19)
  void clearFolder() => $_clearField(19);

  /// The project folder it works in (one of Project.folders); empty when it
  /// works in the project's own folder (or the workspace). Its file tools write
  /// only there; it reads the whole project.
  @$pb.TagNumber(20)
  $core.String get folderId => $_getSZ(19);
  @$pb.TagNumber(20)
  set folderId($core.String value) => $_setString(19, value);
  @$pb.TagNumber(20)
  $core.bool hasFolderId() => $_has(19);
  @$pb.TagNumber(20)
  void clearFolderId() => $_clearField(20);

  /// The extension whose panel is open beside this agent, as the person left
  /// it; empty when none is (or that extension is off for it). The selected
  /// agent's panel content arrives as `ExtensionView`, sent again whenever the
  /// selection changes.
  @$pb.TagNumber(21)
  $core.String get openPanel => $_getSZ(20);
  @$pb.TagNumber(21)
  set openPanel($core.String value) => $_setString(20, value);
  @$pb.TagNumber(21)
  $core.bool hasOpenPanel() => $_has(20);
  @$pb.TagNumber(21)
  void clearOpenPanel() => $_clearField(21);

  /// Its character, drawn in `color` with the same eyes as every agent: its
  /// shape, one of "circle", "square", "triangle", "diamond", "pentagon" or
  /// "hexagon" (catalog order; an automatic one is character_seed % 6). The
  /// one chosen in its settings, or else one picked from its id, so renaming,
  /// moving or reordering it never changes it.
  @$pb.TagNumber(22)
  $core.String get avatarShape => $_getSZ(21);
  @$pb.TagNumber(22)
  set avatarShape($core.String value) => $_setString(21, value);
  @$pb.TagNumber(22)
  $core.bool hasAvatarShape() => $_has(21);
  @$pb.TagNumber(22)
  void clearAvatarShape() => $_clearField(22);

  /// Unused: characters no longer have faces, so it's always empty.
  @$pb.TagNumber(23)
  $core.String get avatarFace => $_getSZ(22);
  @$pb.TagNumber(23)
  set avatarFace($core.String value) => $_setString(22, value);
  @$pb.TagNumber(23)
  $core.bool hasAvatarFace() => $_has(22);
  @$pb.TagNumber(23)
  void clearAvatarFace() => $_clearField(23);

  /// A number from its id (32-bit FNV-1a of the id), the same on every device
  /// and for good: offsets its animation so agents don't blink together.
  @$pb.TagNumber(24)
  $core.int get characterSeed => $_getIZ(23);
  @$pb.TagNumber(24)
  set characterSeed($core.int value) => $_setUnsignedInt32(23, value);
  @$pb.TagNumber(24)
  $core.bool hasCharacterSeed() => $_has(23);
  @$pb.TagNumber(24)
  void clearCharacterSeed() => $_clearField(24);

  /// What its character shows now. Factory decides it; the app only draws it.
  @$pb.TagNumber(25)
  CharacterMood get characterMood => $_getN(24);
  @$pb.TagNumber(25)
  set characterMood(CharacterMood value) => $_setField(25, value);
  @$pb.TagNumber(25)
  $core.bool hasCharacterMood() => $_has(24);
  @$pb.TagNumber(25)
  void clearCharacterMood() => $_clearField(25);

  /// When `character_mood` became what it is, in ms since 1970; unchanged
  /// while the mood stays. A mood that changes while the app shows the agent
  /// may play its one-time entry; one already in place when the app connects
  /// doesn't.
  @$pb.TagNumber(26)
  $fixnum.Int64 get characterMoodSinceMs => $_getI64(25);
  @$pb.TagNumber(26)
  set characterMoodSinceMs($fixnum.Int64 value) => $_setInt64(25, value);
  @$pb.TagNumber(26)
  $core.bool hasCharacterMoodSinceMs() => $_has(25);
  @$pb.TagNumber(26)
  void clearCharacterMoodSinceMs() => $_clearField(26);

  /// The shape chosen in its settings; absent when it's automatic. It may be
  /// one this Factory doesn't know (such as an organic shape of before), in
  /// which case `avatar_shape` is automatic.
  @$pb.TagNumber(27)
  $core.String get avatarShapeOverride => $_getSZ(26);
  @$pb.TagNumber(27)
  set avatarShapeOverride($core.String value) => $_setString(26, value);
  @$pb.TagNumber(27)
  $core.bool hasAvatarShapeOverride() => $_has(26);
  @$pb.TagNumber(27)
  void clearAvatarShapeOverride() => $_clearField(27);

  /// Unused: characters no longer have faces, so it's always absent.
  @$pb.TagNumber(28)
  $core.String get avatarFaceOverride => $_getSZ(27);
  @$pb.TagNumber(28)
  set avatarFaceOverride($core.String value) => $_setString(27, value);
  @$pb.TagNumber(28)
  $core.bool hasAvatarFaceOverride() => $_has(27);
  @$pb.TagNumber(28)
  void clearAvatarFaceOverride() => $_clearField(28);

  /// What the agent does, such as "Coordinator" or "Copywriter", shown
  /// fainter beside or under its name, which is its identity (often a human
  /// first name, "Mike"). Other agents see it with the name too
  /// ("Mike (Coordinator)"). Empty when it has none.
  @$pb.TagNumber(29)
  $core.String get role => $_getSZ(28);
  @$pb.TagNumber(29)
  set role($core.String value) => $_setString(28, value);
  @$pb.TagNumber(29)
  $core.bool hasRole() => $_has(28);
  @$pb.TagNumber(29)
  void clearRole() => $_clearField(29);

  /// How the agent's main area is laid out: its conversation and the viewers
  /// open beside it (SetLayout, the show tool). Unset: just its conversation.
  @$pb.TagNumber(30)
  ViewNode get layout => $_getN(29);
  @$pb.TagNumber(30)
  set layout(ViewNode value) => $_setField(30, value);
  @$pb.TagNumber(30)
  $core.bool hasLayout() => $_has(29);
  @$pb.TagNumber(30)
  void clearLayout() => $_clearField(30);
  @$pb.TagNumber(30)
  ViewNode ensureLayout() => $_ensure(29);

  /// The default prompt the agent can go back to, as this version of
  /// Claktory ships it, with its placeholders ({{user}}…); empty when it has
  /// none (only Rivet has one). While `prompt` equals it, the prompt follows
  /// the default: a new version's default replaces it. To go back to the
  /// default, set the prompt to this text (UpdateAgent).
  @$pb.TagNumber(31)
  $core.String get defaultPrompt => $_getSZ(30);
  @$pb.TagNumber(31)
  set defaultPrompt($core.String value) => $_setString(30, value);
  @$pb.TagNumber(31)
  $core.bool hasDefaultPrompt() => $_has(30);
  @$pb.TagNumber(31)
  void clearDefaultPrompt() => $_clearField(31);
}

class SlashCommand extends $pb.GeneratedMessage {
  factory SlashCommand({
    $core.String? name,
    $core.String? description,
    $core.String? argsHint,
  }) {
    final result = SlashCommand._();
    if (name != null) result.name = name;
    if (description != null) result.description = description;
    if (argsHint != null) result.argsHint = argsHint;
    return result;
  }

  SlashCommand._();

  factory SlashCommand.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SlashCommand()..mergeFromBuffer(data, registry);
  factory SlashCommand.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SlashCommand()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SlashCommand',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SlashCommand.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'name')
    ..aOS(2, _omitFieldNames ? '' : 'description')
    ..aOS(3, _omitFieldNames ? '' : 'argsHint')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SlashCommand clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SlashCommand copyWith(void Function(SlashCommand) updates) =>
      super.copyWith((message) => updates(message as SlashCommand))
          as SlashCommand;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SlashCommand() / SlashCommand.new instead')
  static SlashCommand create() => SlashCommand._();
  static $pb.GeneratedMessage $_createMessage() => SlashCommand._();
  @$core.override
  SlashCommand createEmptyInstance() => SlashCommand._();
  @$core.pragma('dart2js:noInline')
  static SlashCommand getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<SlashCommand>(
          SlashCommand.$_createMessage);
  static SlashCommand? _defaultInstance;

  /// "clear", without the slash.
  @$pb.TagNumber(1)
  $core.String get name => $_getSZ(0);
  @$pb.TagNumber(1)
  set name($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasName() => $_has(0);
  @$pb.TagNumber(1)
  void clearName() => $_clearField(1);

  /// One line, e.g. "Start a new conversation".
  @$pb.TagNumber(2)
  $core.String get description => $_getSZ(1);
  @$pb.TagNumber(2)
  set description($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasDescription() => $_has(1);
  @$pb.TagNumber(2)
  void clearDescription() => $_clearField(2);

  /// What it takes after its name, e.g. "<instructions>"; empty if nothing.
  @$pb.TagNumber(3)
  $core.String get argsHint => $_getSZ(2);
  @$pb.TagNumber(3)
  set argsHint($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasArgsHint() => $_has(2);
  @$pb.TagNumber(3)
  void clearArgsHint() => $_clearField(3);
}

class Model extends $pb.GeneratedMessage {
  factory Model({
    $core.String? id,
    $core.String? name,
    $fixnum.Int64? contextWindow,
    $core.Iterable<$core.String>? efforts,
  }) {
    final result = Model._();
    if (id != null) result.id = id;
    if (name != null) result.name = name;
    if (contextWindow != null) result.contextWindow = contextWindow;
    if (efforts != null) result.efforts.addAll(efforts);
    return result;
  }

  Model._();

  factory Model.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Model()..mergeFromBuffer(data, registry);
  factory Model.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Model()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Model',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Model.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..a<$fixnum.Int64>(
        3, _omitFieldNames ? '' : 'contextWindow', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..pPS(4, _omitFieldNames ? '' : 'efforts')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Model clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Model copyWith(void Function(Model) updates) =>
      super.copyWith((message) => updates(message as Model)) as Model;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Model() / Model.new instead')
  static Model create() => Model._();
  static $pb.GeneratedMessage $_createMessage() => Model._();
  @$core.override
  Model createEmptyInstance() => Model._();
  @$core.pragma('dart2js:noInline')
  static Model getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Model>(Model.$_createMessage);
  static Model? _defaultInstance;

  /// "<provider>/<model id>".
  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  @$pb.TagNumber(3)
  $fixnum.Int64 get contextWindow => $_getI64(2);
  @$pb.TagNumber(3)
  set contextWindow($fixnum.Int64 value) => $_setInt64(2, value);
  @$pb.TagNumber(3)
  $core.bool hasContextWindow() => $_has(2);
  @$pb.TagNumber(3)
  void clearContextWindow() => $_clearField(3);

  /// Reasoning efforts the model accepts, weakest first.
  @$pb.TagNumber(4)
  $pb.PbList<$core.String> get efforts => $_getList(3);
}

/// The selected agent's whole transcript, oldest entry first.
class Transcript extends $pb.GeneratedMessage {
  factory Transcript({
    $core.String? agentId,
    $core.Iterable<Entry>? entries,
    $core.bool? hasEarlier,
  }) {
    final result = Transcript._();
    if (agentId != null) result.agentId = agentId;
    if (entries != null) result.entries.addAll(entries);
    if (hasEarlier != null) result.hasEarlier = hasEarlier;
    return result;
  }

  Transcript._();

  factory Transcript.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Transcript()..mergeFromBuffer(data, registry);
  factory Transcript.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Transcript()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Transcript',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Transcript.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..pPM<Entry>(2, _omitFieldNames ? '' : 'entries',
        subBuilder: Entry.$_createMessage)
    ..aOB(3, _omitFieldNames ? '' : 'hasEarlier')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Transcript clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Transcript copyWith(void Function(Transcript) updates) =>
      super.copyWith((message) => updates(message as Transcript)) as Transcript;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Transcript() / Transcript.new instead')
  static Transcript create() => Transcript._();
  static $pb.GeneratedMessage $_createMessage() => Transcript._();
  @$core.override
  Transcript createEmptyInstance() => Transcript._();
  @$core.pragma('dart2js:noInline')
  static Transcript getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Transcript>(Transcript.$_createMessage);
  static Transcript? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  /// The latest entries only: at most 200, and at most about 256 KB encoded
  /// (at least one, however big), starting at a model response's first entry.
  /// Older ones come with LoadEntries.
  @$pb.TagNumber(2)
  $pb.PbList<Entry> get entries => $_getList(1);

  /// There are entries before the first of `entries`.
  @$pb.TagNumber(3)
  $core.bool get hasEarlier => $_getBF(2);
  @$pb.TagNumber(3)
  set hasEarlier($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasHasEarlier() => $_has(2);
  @$pb.TagNumber(3)
  void clearHasEarlier() => $_clearField(3);
}

/// Earlier entries of an agent's transcript, answered with EntryPage to the
/// asker only. NOT_FOUND for an unknown agent, INVALID_REQUEST when
/// `before_seq` is beyond the end.
class LoadEntries extends $pb.GeneratedMessage {
  factory LoadEntries({
    $core.String? agentId,
    $fixnum.Int64? beforeSeq,
    $core.int? limit,
    $fixnum.Int64? fromSeq,
  }) {
    final result = LoadEntries._();
    if (agentId != null) result.agentId = agentId;
    if (beforeSeq != null) result.beforeSeq = beforeSeq;
    if (limit != null) result.limit = limit;
    if (fromSeq != null) result.fromSeq = fromSeq;
    return result;
  }

  LoadEntries._();

  factory LoadEntries.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      LoadEntries()..mergeFromBuffer(data, registry);
  factory LoadEntries.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      LoadEntries()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'LoadEntries',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: LoadEntries.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..a<$fixnum.Int64>(
        2, _omitFieldNames ? '' : 'beforeSeq', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..aI(3, _omitFieldNames ? '' : 'limit', fieldType: $pb.PbFieldType.OU3)
    ..a<$fixnum.Int64>(4, _omitFieldNames ? '' : 'fromSeq', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  LoadEntries clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  LoadEntries copyWith(void Function(LoadEntries) updates) =>
      super.copyWith((message) => updates(message as LoadEntries))
          as LoadEntries;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use LoadEntries() / LoadEntries.new instead')
  static LoadEntries create() => LoadEntries._();
  static $pb.GeneratedMessage $_createMessage() => LoadEntries._();
  @$core.override
  LoadEntries createEmptyInstance() => LoadEntries._();
  @$core.pragma('dart2js:noInline')
  static LoadEntries getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<LoadEntries>(
          LoadEntries.$_createMessage);
  static LoadEntries? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  /// Entries before this seq (exclusive): the seq of the first entry the app
  /// holds.
  @$pb.TagNumber(2)
  $fixnum.Int64 get beforeSeq => $_getI64(1);
  @$pb.TagNumber(2)
  set beforeSeq($fixnum.Int64 value) => $_setInt64(1, value);
  @$pb.TagNumber(2)
  $core.bool hasBeforeSeq() => $_has(1);
  @$pb.TagNumber(2)
  void clearBeforeSeq() => $_clearField(2);

  /// Without `from_seq`: the latest entries before `before_seq`, cut as
  /// Transcript's are, at most `limit` (0 for 200; 1000 at most).
  @$pb.TagNumber(3)
  $core.int get limit => $_getIZ(2);
  @$pb.TagNumber(3)
  set limit($core.int value) => $_setUnsignedInt32(2, value);
  @$pb.TagNumber(3)
  $core.bool hasLimit() => $_has(2);
  @$pb.TagNumber(3)
  void clearLimit() => $_clearField(3);

  /// Every entry from this seq up to `before_seq`, uncut: to jump to an older
  /// entry and keep what the app holds one run with no gaps.
  @$pb.TagNumber(4)
  $fixnum.Int64 get fromSeq => $_getI64(3);
  @$pb.TagNumber(4)
  set fromSeq($fixnum.Int64 value) => $_setInt64(3, value);
  @$pb.TagNumber(4)
  $core.bool hasFromSeq() => $_has(3);
  @$pb.TagNumber(4)
  void clearFromSeq() => $_clearField(4);
}

/// The answer to LoadEntries: entries oldest first.
class EntryPage extends $pb.GeneratedMessage {
  factory EntryPage({
    $core.String? agentId,
    $core.Iterable<Entry>? entries,
    $core.bool? hasEarlier,
  }) {
    final result = EntryPage._();
    if (agentId != null) result.agentId = agentId;
    if (entries != null) result.entries.addAll(entries);
    if (hasEarlier != null) result.hasEarlier = hasEarlier;
    return result;
  }

  EntryPage._();

  factory EntryPage.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      EntryPage()..mergeFromBuffer(data, registry);
  factory EntryPage.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      EntryPage()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'EntryPage',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: EntryPage.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..pPM<Entry>(2, _omitFieldNames ? '' : 'entries',
        subBuilder: Entry.$_createMessage)
    ..aOB(3, _omitFieldNames ? '' : 'hasEarlier')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  EntryPage clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  EntryPage copyWith(void Function(EntryPage) updates) =>
      super.copyWith((message) => updates(message as EntryPage)) as EntryPage;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use EntryPage() / EntryPage.new instead')
  static EntryPage create() => EntryPage._();
  static $pb.GeneratedMessage $_createMessage() => EntryPage._();
  @$core.override
  EntryPage createEmptyInstance() => EntryPage._();
  @$core.pragma('dart2js:noInline')
  static EntryPage getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<EntryPage>(EntryPage.$_createMessage);
  static EntryPage? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  @$pb.TagNumber(2)
  $pb.PbList<Entry> get entries => $_getList(1);

  /// There are entries before the first of `entries`.
  @$pb.TagNumber(3)
  $core.bool get hasEarlier => $_getBF(2);
  @$pb.TagNumber(3)
  set hasEarlier($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasHasEarlier() => $_has(2);
  @$pb.TagNumber(3)
  void clearHasEarlier() => $_clearField(3);
}

/// What waits for the selected agent, not yet taken by it. Sent on connect,
/// when an agent is selected, and whenever its queue changes; empty when
/// nothing waits. A message the agent takes leaves the list and arrives in
/// the transcript as usual.
class QueuedMessages extends $pb.GeneratedMessage {
  factory QueuedMessages({
    $core.String? agentId,
    $core.Iterable<QueuedMessage>? messages,
  }) {
    final result = QueuedMessages._();
    if (agentId != null) result.agentId = agentId;
    if (messages != null) result.messages.addAll(messages);
    return result;
  }

  QueuedMessages._();

  factory QueuedMessages.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      QueuedMessages()..mergeFromBuffer(data, registry);
  factory QueuedMessages.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      QueuedMessages()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'QueuedMessages',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: QueuedMessages.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..pPM<QueuedMessage>(2, _omitFieldNames ? '' : 'messages',
        subBuilder: QueuedMessage.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  QueuedMessages clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  QueuedMessages copyWith(void Function(QueuedMessages) updates) =>
      super.copyWith((message) => updates(message as QueuedMessages))
          as QueuedMessages;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use QueuedMessages() / QueuedMessages.new instead')
  static QueuedMessages create() => QueuedMessages._();
  static $pb.GeneratedMessage $_createMessage() => QueuedMessages._();
  @$core.override
  QueuedMessages createEmptyInstance() => QueuedMessages._();
  @$core.pragma('dart2js:noInline')
  static QueuedMessages getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<QueuedMessages>(
          QueuedMessages.$_createMessage);
  static QueuedMessages? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  /// Oldest first.
  @$pb.TagNumber(2)
  $pb.PbList<QueuedMessage> get messages => $_getList(1);
}

class QueuedMessage extends $pb.GeneratedMessage {
  factory QueuedMessage({
    $fixnum.Int64? id,
    $core.String? source,
    $core.String? sender,
    $core.String? senderId,
    $core.String? text,
    $fixnum.Int64? created,
    $core.String? summary,
    SentFrom? sentFrom,
    $core.String? clientId,
  }) {
    final result = QueuedMessage._();
    if (id != null) result.id = id;
    if (source != null) result.source = source;
    if (sender != null) result.sender = sender;
    if (senderId != null) result.senderId = senderId;
    if (text != null) result.text = text;
    if (created != null) result.created = created;
    if (summary != null) result.summary = summary;
    if (sentFrom != null) result.sentFrom = sentFrom;
    if (clientId != null) result.clientId = clientId;
    return result;
  }

  QueuedMessage._();

  factory QueuedMessage.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      QueuedMessage()..mergeFromBuffer(data, registry);
  factory QueuedMessage.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      QueuedMessage()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'QueuedMessage',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: QueuedMessage.$_createMessage)
    ..aInt64(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'source')
    ..aOS(3, _omitFieldNames ? '' : 'sender')
    ..aOS(4, _omitFieldNames ? '' : 'senderId')
    ..aOS(5, _omitFieldNames ? '' : 'text')
    ..aInt64(6, _omitFieldNames ? '' : 'created')
    ..aOS(7, _omitFieldNames ? '' : 'summary')
    ..aOM<SentFrom>(8, _omitFieldNames ? '' : 'sentFrom',
        subBuilder: SentFrom.$_createMessage)
    ..aOS(9, _omitFieldNames ? '' : 'clientId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  QueuedMessage clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  QueuedMessage copyWith(void Function(QueuedMessage) updates) =>
      super.copyWith((message) => updates(message as QueuedMessage))
          as QueuedMessage;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use QueuedMessage() / QueuedMessage.new instead')
  static QueuedMessage create() => QueuedMessage._();
  static $pb.GeneratedMessage $_createMessage() => QueuedMessage._();
  @$core.override
  QueuedMessage createEmptyInstance() => QueuedMessage._();
  @$core.pragma('dart2js:noInline')
  static QueuedMessage getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<QueuedMessage>(
          QueuedMessage.$_createMessage);
  static QueuedMessage? _defaultInstance;

  /// Its id in factory.db's messages.
  @$pb.TagNumber(1)
  $fixnum.Int64 get id => $_getI64(0);
  @$pb.TagNumber(1)
  set id($fixnum.Int64 value) => $_setInt64(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// "person", "agent", "task" or "factory".
  @$pb.TagNumber(2)
  $core.String get source => $_getSZ(1);
  @$pb.TagNumber(2)
  set source($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasSource() => $_has(1);
  @$pb.TagNumber(2)
  void clearSource() => $_clearField(2);

  /// Who it's from, for events: the agent's name (with its project when it's
  /// in another one), a task's id, "Factory". Empty for the person.
  @$pb.TagNumber(3)
  $core.String get sender => $_getSZ(2);
  @$pb.TagNumber(3)
  set sender($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasSender() => $_has(2);
  @$pb.TagNumber(3)
  void clearSender() => $_clearField(3);

  /// The sending agent's id, or the task's. Empty for the person.
  @$pb.TagNumber(4)
  $core.String get senderId => $_getSZ(3);
  @$pb.TagNumber(4)
  set senderId($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasSenderId() => $_has(3);
  @$pb.TagNumber(4)
  void clearSenderId() => $_clearField(4);

  /// The whole text, as the agent will read it.
  @$pb.TagNumber(5)
  $core.String get text => $_getSZ(4);
  @$pb.TagNumber(5)
  set text($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasText() => $_has(4);
  @$pb.TagNumber(5)
  void clearText() => $_clearField(5);

  /// ms since 1970.
  @$pb.TagNumber(6)
  $fixnum.Int64 get created => $_getI64(5);
  @$pb.TagNumber(6)
  set created($fixnum.Int64 value) => $_setInt64(5, value);
  @$pb.TagNumber(6)
  $core.bool hasCreated() => $_has(5);
  @$pb.TagNumber(6)
  void clearCreated() => $_clearField(6);

  /// One line for people, for events that have one (a task that finished,
  /// Factory restarted); empty otherwise, then show `text`.
  @$pb.TagNumber(7)
  $core.String get summary => $_getSZ(6);
  @$pb.TagNumber(7)
  set summary($core.String value) => $_setString(6, value);
  @$pb.TagNumber(7)
  $core.bool hasSummary() => $_has(6);
  @$pb.TagNumber(7)
  void clearSummary() => $_clearField(7);

  /// The person's: the device it was sent from (see UserText.sent_from).
  @$pb.TagNumber(8)
  SentFrom get sentFrom => $_getN(7);
  @$pb.TagNumber(8)
  set sentFrom(SentFrom value) => $_setField(8, value);
  @$pb.TagNumber(8)
  $core.bool hasSentFrom() => $_has(7);
  @$pb.TagNumber(8)
  void clearSentFrom() => $_clearField(8);
  @$pb.TagNumber(8)
  SentFrom ensureSentFrom() => $_ensure(7);

  /// The person's: SendMessage.client_id, as sent; empty without one.
  @$pb.TagNumber(9)
  $core.String get clientId => $_getSZ(8);
  @$pb.TagNumber(9)
  set clientId($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasClientId() => $_has(8);
  @$pb.TagNumber(9)
  void clearClientId() => $_clearField(9);
}

/// Adds an entry or replaces the one with the same `seq`.
class EntryUpdate extends $pb.GeneratedMessage {
  factory EntryUpdate({
    $core.String? agentId,
    Entry? entry,
  }) {
    final result = EntryUpdate._();
    if (agentId != null) result.agentId = agentId;
    if (entry != null) result.entry = entry;
    return result;
  }

  EntryUpdate._();

  factory EntryUpdate.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      EntryUpdate()..mergeFromBuffer(data, registry);
  factory EntryUpdate.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      EntryUpdate()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'EntryUpdate',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: EntryUpdate.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..aOM<Entry>(2, _omitFieldNames ? '' : 'entry',
        subBuilder: Entry.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  EntryUpdate clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  EntryUpdate copyWith(void Function(EntryUpdate) updates) =>
      super.copyWith((message) => updates(message as EntryUpdate))
          as EntryUpdate;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use EntryUpdate() / EntryUpdate.new instead')
  static EntryUpdate create() => EntryUpdate._();
  static $pb.GeneratedMessage $_createMessage() => EntryUpdate._();
  @$core.override
  EntryUpdate createEmptyInstance() => EntryUpdate._();
  @$core.pragma('dart2js:noInline')
  static EntryUpdate getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<EntryUpdate>(
          EntryUpdate.$_createMessage);
  static EntryUpdate? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  @$pb.TagNumber(2)
  Entry get entry => $_getN(1);
  @$pb.TagNumber(2)
  set entry(Entry value) => $_setField(2, value);
  @$pb.TagNumber(2)
  $core.bool hasEntry() => $_has(1);
  @$pb.TagNumber(2)
  void clearEntry() => $_clearField(2);
  @$pb.TagNumber(2)
  Entry ensureEntry() => $_ensure(1);
}

/// Appends streamed text to an entry: `text` for assistant text and thinking.
class EntryDelta extends $pb.GeneratedMessage {
  factory EntryDelta({
    $core.String? agentId,
    $fixnum.Int64? seq,
    $core.String? text,
  }) {
    final result = EntryDelta._();
    if (agentId != null) result.agentId = agentId;
    if (seq != null) result.seq = seq;
    if (text != null) result.text = text;
    return result;
  }

  EntryDelta._();

  factory EntryDelta.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      EntryDelta()..mergeFromBuffer(data, registry);
  factory EntryDelta.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      EntryDelta()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'EntryDelta',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: EntryDelta.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..a<$fixnum.Int64>(2, _omitFieldNames ? '' : 'seq', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..aOS(3, _omitFieldNames ? '' : 'text')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  EntryDelta clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  EntryDelta copyWith(void Function(EntryDelta) updates) =>
      super.copyWith((message) => updates(message as EntryDelta)) as EntryDelta;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use EntryDelta() / EntryDelta.new instead')
  static EntryDelta create() => EntryDelta._();
  static $pb.GeneratedMessage $_createMessage() => EntryDelta._();
  @$core.override
  EntryDelta createEmptyInstance() => EntryDelta._();
  @$core.pragma('dart2js:noInline')
  static EntryDelta getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<EntryDelta>(EntryDelta.$_createMessage);
  static EntryDelta? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  @$pb.TagNumber(2)
  $fixnum.Int64 get seq => $_getI64(1);
  @$pb.TagNumber(2)
  set seq($fixnum.Int64 value) => $_setInt64(1, value);
  @$pb.TagNumber(2)
  $core.bool hasSeq() => $_has(1);
  @$pb.TagNumber(2)
  void clearSeq() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.String get text => $_getSZ(2);
  @$pb.TagNumber(3)
  set text($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasText() => $_has(2);
  @$pb.TagNumber(3)
  void clearText() => $_clearField(3);
}

enum Entry_Kind {
  user,
  assistant,
  thinking,
  toolCall,
  compaction,
  notice,
  event,
  proposal,
  secretRequest,
  notSet
}

class Entry extends $pb.GeneratedMessage {
  factory Entry({
    $fixnum.Int64? seq,
    $fixnum.Int64? timestampMs,
    UserText? user,
    AssistantText? assistant,
    Thinking? thinking,
    ToolCall? toolCall,
    Compaction? compaction,
    Notice? notice,
    Event? event,
    $fixnum.Int64? reply,
    Proposal? proposal,
    SecretRequest? secretRequest,
  }) {
    final result = Entry._();
    if (seq != null) result.seq = seq;
    if (timestampMs != null) result.timestampMs = timestampMs;
    if (user != null) result.user = user;
    if (assistant != null) result.assistant = assistant;
    if (thinking != null) result.thinking = thinking;
    if (toolCall != null) result.toolCall = toolCall;
    if (compaction != null) result.compaction = compaction;
    if (notice != null) result.notice = notice;
    if (event != null) result.event = event;
    if (reply != null) result.reply = reply;
    if (proposal != null) result.proposal = proposal;
    if (secretRequest != null) result.secretRequest = secretRequest;
    return result;
  }

  Entry._();

  factory Entry.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Entry()..mergeFromBuffer(data, registry);
  factory Entry.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Entry()..mergeFromJson(json, registry);

  static const $core.Map<$core.int, Entry_Kind> _Entry_KindByTag = {
    3: Entry_Kind.user,
    4: Entry_Kind.assistant,
    5: Entry_Kind.thinking,
    6: Entry_Kind.toolCall,
    7: Entry_Kind.compaction,
    8: Entry_Kind.notice,
    9: Entry_Kind.event,
    11: Entry_Kind.proposal,
    12: Entry_Kind.secretRequest,
    0: Entry_Kind.notSet
  };
  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Entry',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Entry.$_createMessage)
    ..oo(0, [3, 4, 5, 6, 7, 8, 9, 11, 12])
    ..a<$fixnum.Int64>(1, _omitFieldNames ? '' : 'seq', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..aInt64(2, _omitFieldNames ? '' : 'timestampMs')
    ..aOM<UserText>(3, _omitFieldNames ? '' : 'user',
        subBuilder: UserText.$_createMessage)
    ..aOM<AssistantText>(4, _omitFieldNames ? '' : 'assistant',
        subBuilder: AssistantText.$_createMessage)
    ..aOM<Thinking>(5, _omitFieldNames ? '' : 'thinking',
        subBuilder: Thinking.$_createMessage)
    ..aOM<ToolCall>(6, _omitFieldNames ? '' : 'toolCall',
        subBuilder: ToolCall.$_createMessage)
    ..aOM<Compaction>(7, _omitFieldNames ? '' : 'compaction',
        subBuilder: Compaction.$_createMessage)
    ..aOM<Notice>(8, _omitFieldNames ? '' : 'notice',
        subBuilder: Notice.$_createMessage)
    ..aOM<Event>(9, _omitFieldNames ? '' : 'event',
        subBuilder: Event.$_createMessage)
    ..a<$fixnum.Int64>(10, _omitFieldNames ? '' : 'reply', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..aOM<Proposal>(11, _omitFieldNames ? '' : 'proposal',
        subBuilder: Proposal.$_createMessage)
    ..aOM<SecretRequest>(12, _omitFieldNames ? '' : 'secretRequest',
        subBuilder: SecretRequest.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Entry clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Entry copyWith(void Function(Entry) updates) =>
      super.copyWith((message) => updates(message as Entry)) as Entry;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Entry() / Entry.new instead')
  static Entry create() => Entry._();
  static $pb.GeneratedMessage $_createMessage() => Entry._();
  @$core.override
  Entry createEmptyInstance() => Entry._();
  @$core.pragma('dart2js:noInline')
  static Entry getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Entry>(Entry.$_createMessage);
  static Entry? _defaultInstance;

  @$pb.TagNumber(3)
  @$pb.TagNumber(4)
  @$pb.TagNumber(5)
  @$pb.TagNumber(6)
  @$pb.TagNumber(7)
  @$pb.TagNumber(8)
  @$pb.TagNumber(9)
  @$pb.TagNumber(11)
  @$pb.TagNumber(12)
  Entry_Kind whichKind() => _Entry_KindByTag[$_whichOneof(0)]!;
  @$pb.TagNumber(3)
  @$pb.TagNumber(4)
  @$pb.TagNumber(5)
  @$pb.TagNumber(6)
  @$pb.TagNumber(7)
  @$pb.TagNumber(8)
  @$pb.TagNumber(9)
  @$pb.TagNumber(11)
  @$pb.TagNumber(12)
  void clearKind() => $_clearField($_whichOneof(0));

  /// Position in the transcript; unique per agent and never reused.
  @$pb.TagNumber(1)
  $fixnum.Int64 get seq => $_getI64(0);
  @$pb.TagNumber(1)
  set seq($fixnum.Int64 value) => $_setInt64(0, value);
  @$pb.TagNumber(1)
  $core.bool hasSeq() => $_has(0);
  @$pb.TagNumber(1)
  void clearSeq() => $_clearField(1);

  @$pb.TagNumber(2)
  $fixnum.Int64 get timestampMs => $_getI64(1);
  @$pb.TagNumber(2)
  set timestampMs($fixnum.Int64 value) => $_setInt64(1, value);
  @$pb.TagNumber(2)
  $core.bool hasTimestampMs() => $_has(1);
  @$pb.TagNumber(2)
  void clearTimestampMs() => $_clearField(2);

  @$pb.TagNumber(3)
  UserText get user => $_getN(2);
  @$pb.TagNumber(3)
  set user(UserText value) => $_setField(3, value);
  @$pb.TagNumber(3)
  $core.bool hasUser() => $_has(2);
  @$pb.TagNumber(3)
  void clearUser() => $_clearField(3);
  @$pb.TagNumber(3)
  UserText ensureUser() => $_ensure(2);

  @$pb.TagNumber(4)
  AssistantText get assistant => $_getN(3);
  @$pb.TagNumber(4)
  set assistant(AssistantText value) => $_setField(4, value);
  @$pb.TagNumber(4)
  $core.bool hasAssistant() => $_has(3);
  @$pb.TagNumber(4)
  void clearAssistant() => $_clearField(4);
  @$pb.TagNumber(4)
  AssistantText ensureAssistant() => $_ensure(3);

  @$pb.TagNumber(5)
  Thinking get thinking => $_getN(4);
  @$pb.TagNumber(5)
  set thinking(Thinking value) => $_setField(5, value);
  @$pb.TagNumber(5)
  $core.bool hasThinking() => $_has(4);
  @$pb.TagNumber(5)
  void clearThinking() => $_clearField(5);
  @$pb.TagNumber(5)
  Thinking ensureThinking() => $_ensure(4);

  @$pb.TagNumber(6)
  ToolCall get toolCall => $_getN(5);
  @$pb.TagNumber(6)
  set toolCall(ToolCall value) => $_setField(6, value);
  @$pb.TagNumber(6)
  $core.bool hasToolCall() => $_has(5);
  @$pb.TagNumber(6)
  void clearToolCall() => $_clearField(6);
  @$pb.TagNumber(6)
  ToolCall ensureToolCall() => $_ensure(5);

  @$pb.TagNumber(7)
  Compaction get compaction => $_getN(6);
  @$pb.TagNumber(7)
  set compaction(Compaction value) => $_setField(7, value);
  @$pb.TagNumber(7)
  $core.bool hasCompaction() => $_has(6);
  @$pb.TagNumber(7)
  void clearCompaction() => $_clearField(7);
  @$pb.TagNumber(7)
  Compaction ensureCompaction() => $_ensure(6);

  @$pb.TagNumber(8)
  Notice get notice => $_getN(7);
  @$pb.TagNumber(8)
  set notice(Notice value) => $_setField(8, value);
  @$pb.TagNumber(8)
  $core.bool hasNotice() => $_has(7);
  @$pb.TagNumber(8)
  void clearNotice() => $_clearField(8);
  @$pb.TagNumber(8)
  Notice ensureNotice() => $_ensure(7);

  @$pb.TagNumber(9)
  Event get event => $_getN(8);
  @$pb.TagNumber(9)
  set event(Event value) => $_setField(9, value);
  @$pb.TagNumber(9)
  $core.bool hasEvent() => $_has(8);
  @$pb.TagNumber(9)
  void clearEvent() => $_clearField(9);
  @$pb.TagNumber(9)
  Event ensureEvent() => $_ensure(8);

  /// The model response it came from: the same for every entry one response
  /// produced (its thinking, text and tool calls), the seq of the first of
  /// them. 0 for entries that don't come from a response (the person's
  /// messages, events, notices, compaction dividers).
  @$pb.TagNumber(10)
  $fixnum.Int64 get reply => $_getI64(9);
  @$pb.TagNumber(10)
  set reply($fixnum.Int64 value) => $_setInt64(9, value);
  @$pb.TagNumber(10)
  $core.bool hasReply() => $_has(9);
  @$pb.TagNumber(10)
  void clearReply() => $_clearField(10);

  @$pb.TagNumber(11)
  Proposal get proposal => $_getN(10);
  @$pb.TagNumber(11)
  set proposal(Proposal value) => $_setField(11, value);
  @$pb.TagNumber(11)
  $core.bool hasProposal() => $_has(10);
  @$pb.TagNumber(11)
  void clearProposal() => $_clearField(11);
  @$pb.TagNumber(11)
  Proposal ensureProposal() => $_ensure(10);

  /// An agent asks for a secret (its secrets_request tool): see SecretRequest.
  @$pb.TagNumber(12)
  SecretRequest get secretRequest => $_getN(11);
  @$pb.TagNumber(12)
  set secretRequest(SecretRequest value) => $_setField(12, value);
  @$pb.TagNumber(12)
  $core.bool hasSecretRequest() => $_has(11);
  @$pb.TagNumber(12)
  void clearSecretRequest() => $_clearField(12);
  @$pb.TagNumber(12)
  SecretRequest ensureSecretRequest() => $_ensure(11);
}

/// Something other than the person prompting the agent: Factory itself,
/// another agent, a background task that finished, or a monitor. The model
/// reads it as a message tagged with where it's from; the app shows it as an
/// event, never as the person's.
class Event extends $pb.GeneratedMessage {
  factory Event({
    EventSource? source,
    $core.String? from,
    $core.String? fromId,
    $core.String? summary,
    $core.String? text,
  }) {
    final result = Event._();
    if (source != null) result.source = source;
    if (from != null) result.from = from;
    if (fromId != null) result.fromId = fromId;
    if (summary != null) result.summary = summary;
    if (text != null) result.text = text;
    return result;
  }

  Event._();

  factory Event.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Event()..mergeFromBuffer(data, registry);
  factory Event.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Event()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Event',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Event.$_createMessage)
    ..aE<EventSource>(1, _omitFieldNames ? '' : 'source',
        enumValues: EventSource.values)
    ..aOS(2, _omitFieldNames ? '' : 'from')
    ..aOS(3, _omitFieldNames ? '' : 'fromId')
    ..aOS(4, _omitFieldNames ? '' : 'summary')
    ..aOS(5, _omitFieldNames ? '' : 'text')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Event clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Event copyWith(void Function(Event) updates) =>
      super.copyWith((message) => updates(message as Event)) as Event;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Event() / Event.new instead')
  static Event create() => Event._();
  static $pb.GeneratedMessage $_createMessage() => Event._();
  @$core.override
  Event createEmptyInstance() => Event._();
  @$core.pragma('dart2js:noInline')
  static Event getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Event>(Event.$_createMessage);
  static Event? _defaultInstance;

  @$pb.TagNumber(1)
  EventSource get source => $_getN(0);
  @$pb.TagNumber(1)
  set source(EventSource value) => $_setField(1, value);
  @$pb.TagNumber(1)
  $core.bool hasSource() => $_has(0);
  @$pb.TagNumber(1)
  void clearSource() => $_clearField(1);

  /// Who it's from: "Factory", the sending agent's name, the task's id, or the
  /// monitor's name.
  @$pb.TagNumber(2)
  $core.String get from => $_getSZ(1);
  @$pb.TagNumber(2)
  set from($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasFrom() => $_has(1);
  @$pb.TagNumber(2)
  void clearFrom() => $_clearField(2);

  /// The sending agent's id, for AGENT; the task's id, for TASK; the
  /// monitor's id ("m3"), for MONITOR.
  @$pb.TagNumber(3)
  $core.String get fromId => $_getSZ(2);
  @$pb.TagNumber(3)
  set fromId($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasFromId() => $_has(2);
  @$pb.TagNumber(3)
  void clearFromId() => $_clearField(3);

  /// One line for people; empty when `text` is meant to be shown as it is.
  @$pb.TagNumber(4)
  $core.String get summary => $_getSZ(3);
  @$pb.TagNumber(4)
  set summary($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasSummary() => $_has(3);
  @$pb.TagNumber(4)
  void clearSummary() => $_clearField(4);

  @$pb.TagNumber(5)
  $core.String get text => $_getSZ(4);
  @$pb.TagNumber(5)
  set text($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasText() => $_has(4);
  @$pb.TagNumber(5)
  void clearText() => $_clearField(5);
}

/// A message the agent proposed for Rivet, the admin agent, which only the
/// person can message: shown in the proposing agent's conversation, after the
/// propose_to_rivet call, for the person to approve (with a comment) or
/// dismiss with DecideProposal. The entry is sent again when it's decided.
class Proposal extends $pb.GeneratedMessage {
  factory Proposal({
    $core.String? id,
    $core.String? text,
    ProposalState? state,
    $core.String? comment,
    $fixnum.Int64? decidedAtMs,
  }) {
    final result = Proposal._();
    if (id != null) result.id = id;
    if (text != null) result.text = text;
    if (state != null) result.state = state;
    if (comment != null) result.comment = comment;
    if (decidedAtMs != null) result.decidedAtMs = decidedAtMs;
    return result;
  }

  Proposal._();

  factory Proposal.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Proposal()..mergeFromBuffer(data, registry);
  factory Proposal.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Proposal()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Proposal',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Proposal.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'text')
    ..aE<ProposalState>(3, _omitFieldNames ? '' : 'state',
        enumValues: ProposalState.values)
    ..aOS(4, _omitFieldNames ? '' : 'comment')
    ..aInt64(5, _omitFieldNames ? '' : 'decidedAtMs')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Proposal clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Proposal copyWith(void Function(Proposal) updates) =>
      super.copyWith((message) => updates(message as Proposal)) as Proposal;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Proposal() / Proposal.new instead')
  static Proposal create() => Proposal._();
  static $pb.GeneratedMessage $_createMessage() => Proposal._();
  @$core.override
  Proposal createEmptyInstance() => Proposal._();
  @$core.pragma('dart2js:noInline')
  static Proposal getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Proposal>(Proposal.$_createMessage);
  static Proposal? _defaultInstance;

  /// Unique in this Factory; what DecideProposal names.
  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// What the agent proposes, written to stand alone for Rivet.
  @$pb.TagNumber(2)
  $core.String get text => $_getSZ(1);
  @$pb.TagNumber(2)
  set text($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasText() => $_has(1);
  @$pb.TagNumber(2)
  void clearText() => $_clearField(2);

  @$pb.TagNumber(3)
  ProposalState get state => $_getN(2);
  @$pb.TagNumber(3)
  set state(ProposalState value) => $_setField(3, value);
  @$pb.TagNumber(3)
  $core.bool hasState() => $_has(2);
  @$pb.TagNumber(3)
  void clearState() => $_clearField(3);

  /// The person's comment on approving; empty otherwise.
  @$pb.TagNumber(4)
  $core.String get comment => $_getSZ(3);
  @$pb.TagNumber(4)
  set comment($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasComment() => $_has(3);
  @$pb.TagNumber(4)
  void clearComment() => $_clearField(4);

  /// When it was approved or dismissed, in ms since 1970; 0 while pending.
  @$pb.TagNumber(5)
  $fixnum.Int64 get decidedAtMs => $_getI64(4);
  @$pb.TagNumber(5)
  set decidedAtMs($fixnum.Int64 value) => $_setInt64(4, value);
  @$pb.TagNumber(5)
  $core.bool hasDecidedAtMs() => $_has(4);
  @$pb.TagNumber(5)
  void clearDecidedAtMs() => $_clearField(5);
}

class UserText extends $pb.GeneratedMessage {
  factory UserText({
    $core.String? text,
    SentFrom? sentFrom,
    $core.String? clientId,
  }) {
    final result = UserText._();
    if (text != null) result.text = text;
    if (sentFrom != null) result.sentFrom = sentFrom;
    if (clientId != null) result.clientId = clientId;
    return result;
  }

  UserText._();

  factory UserText.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UserText()..mergeFromBuffer(data, registry);
  factory UserText.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UserText()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'UserText',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: UserText.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'text')
    ..aOM<SentFrom>(2, _omitFieldNames ? '' : 'sentFrom',
        subBuilder: SentFrom.$_createMessage)
    ..aOS(3, _omitFieldNames ? '' : 'clientId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UserText clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UserText copyWith(void Function(UserText) updates) =>
      super.copyWith((message) => updates(message as UserText)) as UserText;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use UserText() / UserText.new instead')
  static UserText create() => UserText._();
  static $pb.GeneratedMessage $_createMessage() => UserText._();
  @$core.override
  UserText createEmptyInstance() => UserText._();
  @$core.pragma('dart2js:noInline')
  static UserText getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<UserText>(UserText.$_createMessage);
  static UserText? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get text => $_getSZ(0);
  @$pb.TagNumber(1)
  set text($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasText() => $_has(0);
  @$pb.TagNumber(1)
  void clearText() => $_clearField(1);

  /// The device it was sent from; unset for messages from before it, and for
  /// those Claktory sent for the person (an approved proposal to Rivet).
  @$pb.TagNumber(2)
  SentFrom get sentFrom => $_getN(1);
  @$pb.TagNumber(2)
  set sentFrom(SentFrom value) => $_setField(2, value);
  @$pb.TagNumber(2)
  $core.bool hasSentFrom() => $_has(1);
  @$pb.TagNumber(2)
  void clearSentFrom() => $_clearField(2);
  @$pb.TagNumber(2)
  SentFrom ensureSentFrom() => $_ensure(1);

  /// SendMessage.client_id, as sent; empty without one, and for messages from
  /// before it.
  @$pb.TagNumber(3)
  $core.String get clientId => $_getSZ(2);
  @$pb.TagNumber(3)
  set clientId($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasClientId() => $_has(2);
  @$pb.TagNumber(3)
  void clearClientId() => $_clearField(3);
}

class AssistantText extends $pb.GeneratedMessage {
  factory AssistantText({
    $core.String? text,
  }) {
    final result = AssistantText._();
    if (text != null) result.text = text;
    return result;
  }

  AssistantText._();

  factory AssistantText.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AssistantText()..mergeFromBuffer(data, registry);
  factory AssistantText.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AssistantText()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'AssistantText',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: AssistantText.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'text')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AssistantText clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AssistantText copyWith(void Function(AssistantText) updates) =>
      super.copyWith((message) => updates(message as AssistantText))
          as AssistantText;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use AssistantText() / AssistantText.new instead')
  static AssistantText create() => AssistantText._();
  static $pb.GeneratedMessage $_createMessage() => AssistantText._();
  @$core.override
  AssistantText createEmptyInstance() => AssistantText._();
  @$core.pragma('dart2js:noInline')
  static AssistantText getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<AssistantText>(
          AssistantText.$_createMessage);
  static AssistantText? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get text => $_getSZ(0);
  @$pb.TagNumber(1)
  set text($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasText() => $_has(0);
  @$pb.TagNumber(1)
  void clearText() => $_clearField(1);
}

class Thinking extends $pb.GeneratedMessage {
  factory Thinking({
    $core.String? text,
  }) {
    final result = Thinking._();
    if (text != null) result.text = text;
    return result;
  }

  Thinking._();

  factory Thinking.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Thinking()..mergeFromBuffer(data, registry);
  factory Thinking.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Thinking()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Thinking',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Thinking.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'text')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Thinking clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Thinking copyWith(void Function(Thinking) updates) =>
      super.copyWith((message) => updates(message as Thinking)) as Thinking;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Thinking() / Thinking.new instead')
  static Thinking create() => Thinking._();
  static $pb.GeneratedMessage $_createMessage() => Thinking._();
  @$core.override
  Thinking createEmptyInstance() => Thinking._();
  @$core.pragma('dart2js:noInline')
  static Thinking getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Thinking>(Thinking.$_createMessage);
  static Thinking? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get text => $_getSZ(0);
  @$pb.TagNumber(1)
  set text($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasText() => $_has(0);
  @$pb.TagNumber(1)
  void clearText() => $_clearField(1);
}

class ToolCall extends $pb.GeneratedMessage {
  factory ToolCall({
    $core.String? callId,
    $core.String? name,
    $core.String? input,
    $core.String? output,
    ToolCallState? state,
    $fixnum.Int64? started,
    $fixnum.Int64? ended,
    $fixnum.Int64? timeoutMs,
    $core.String? details,
  }) {
    final result = ToolCall._();
    if (callId != null) result.callId = callId;
    if (name != null) result.name = name;
    if (input != null) result.input = input;
    if (output != null) result.output = output;
    if (state != null) result.state = state;
    if (started != null) result.started = started;
    if (ended != null) result.ended = ended;
    if (timeoutMs != null) result.timeoutMs = timeoutMs;
    if (details != null) result.details = details;
    return result;
  }

  ToolCall._();

  factory ToolCall.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ToolCall()..mergeFromBuffer(data, registry);
  factory ToolCall.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ToolCall()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ToolCall',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ToolCall.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'callId')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'input')
    ..aOS(4, _omitFieldNames ? '' : 'output')
    ..aE<ToolCallState>(5, _omitFieldNames ? '' : 'state',
        enumValues: ToolCallState.values)
    ..aInt64(6, _omitFieldNames ? '' : 'started')
    ..aInt64(7, _omitFieldNames ? '' : 'ended')
    ..a<$fixnum.Int64>(
        8, _omitFieldNames ? '' : 'timeoutMs', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..aOS(9, _omitFieldNames ? '' : 'details')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ToolCall clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ToolCall copyWith(void Function(ToolCall) updates) =>
      super.copyWith((message) => updates(message as ToolCall)) as ToolCall;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ToolCall() / ToolCall.new instead')
  static ToolCall create() => ToolCall._();
  static $pb.GeneratedMessage $_createMessage() => ToolCall._();
  @$core.override
  ToolCall createEmptyInstance() => ToolCall._();
  @$core.pragma('dart2js:noInline')
  static ToolCall getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<ToolCall>(ToolCall.$_createMessage);
  static ToolCall? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get callId => $_getSZ(0);
  @$pb.TagNumber(1)
  set callId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasCallId() => $_has(0);
  @$pb.TagNumber(1)
  void clearCallId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// The call's arguments as pretty-printed JSON. While the model is still
  /// writing them (PENDING), the raw JSON written so far, which may be cut off
  /// anywhere; empty until it starts.
  @$pb.TagNumber(3)
  $core.String get input => $_getSZ(2);
  @$pb.TagNumber(3)
  set input($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasInput() => $_has(2);
  @$pb.TagNumber(3)
  void clearInput() => $_clearField(3);

  /// What the tool returned, or its output so far while it runs.
  @$pb.TagNumber(4)
  $core.String get output => $_getSZ(3);
  @$pb.TagNumber(4)
  set output($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasOutput() => $_has(3);
  @$pb.TagNumber(4)
  void clearOutput() => $_clearField(4);

  @$pb.TagNumber(5)
  ToolCallState get state => $_getN(4);
  @$pb.TagNumber(5)
  set state(ToolCallState value) => $_setField(5, value);
  @$pb.TagNumber(5)
  $core.bool hasState() => $_has(4);
  @$pb.TagNumber(5)
  void clearState() => $_clearField(5);

  /// When the tool began running and when it ended, in ms since 1970; 0 until
  /// it starts, and ended is 0 while it runs. Show a running call's elapsed
  /// time from `started`: no updates are sent just for the clock.
  @$pb.TagNumber(6)
  $fixnum.Int64 get started => $_getI64(5);
  @$pb.TagNumber(6)
  set started($fixnum.Int64 value) => $_setInt64(5, value);
  @$pb.TagNumber(6)
  $core.bool hasStarted() => $_has(5);
  @$pb.TagNumber(6)
  void clearStarted() => $_clearField(6);

  @$pb.TagNumber(7)
  $fixnum.Int64 get ended => $_getI64(6);
  @$pb.TagNumber(7)
  set ended($fixnum.Int64 value) => $_setInt64(6, value);
  @$pb.TagNumber(7)
  $core.bool hasEnded() => $_has(6);
  @$pb.TagNumber(7)
  void clearEnded() => $_clearField(7);

  /// bash only: when it will be stopped, in ms after `started` (its timeout).
  /// 0 for no timeout (run_in_background) and for other tools.
  @$pb.TagNumber(8)
  $fixnum.Int64 get timeoutMs => $_getI64(7);
  @$pb.TagNumber(8)
  set timeoutMs($fixnum.Int64 value) => $_setInt64(7, value);
  @$pb.TagNumber(8)
  $core.bool hasTimeoutMs() => $_has(7);
  @$pb.TagNumber(8)
  void clearTimeoutMs() => $_clearField(8);

  /// What the tool returned for the app to draw its view from, as JSON: ids,
  /// names, counts, paths, links (docs/EXTENSIONS.md, "Its view in the
  /// conversation"). Each tool has its own shape; keys are only ever added.
  /// Empty while the call runs, and for a tool that returns none ("{}" and
  /// null are sent as empty).
  @$pb.TagNumber(9)
  $core.String get details => $_getSZ(8);
  @$pb.TagNumber(9)
  set details($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasDetails() => $_has(8);
  @$pb.TagNumber(9)
  void clearDetails() => $_clearField(9);
}

/// The context was summarized to make room; everything above this entry is
/// history the model now sees only through `summary`.
class Compaction extends $pb.GeneratedMessage {
  factory Compaction({
    $core.String? summary,
    $fixnum.Int64? tokensBefore,
    $core.bool? idle,
  }) {
    final result = Compaction._();
    if (summary != null) result.summary = summary;
    if (tokensBefore != null) result.tokensBefore = tokensBefore;
    if (idle != null) result.idle = idle;
    return result;
  }

  Compaction._();

  factory Compaction.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Compaction()..mergeFromBuffer(data, registry);
  factory Compaction.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Compaction()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Compaction',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Compaction.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'summary')
    ..a<$fixnum.Int64>(
        2, _omitFieldNames ? '' : 'tokensBefore', $pb.PbFieldType.OU6,
        defaultOrMaker: $fixnum.Int64.ZERO)
    ..aOB(3, _omitFieldNames ? '' : 'idle')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Compaction clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Compaction copyWith(void Function(Compaction) updates) =>
      super.copyWith((message) => updates(message as Compaction)) as Compaction;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Compaction() / Compaction.new instead')
  static Compaction create() => Compaction._();
  static $pb.GeneratedMessage $_createMessage() => Compaction._();
  @$core.override
  Compaction createEmptyInstance() => Compaction._();
  @$core.pragma('dart2js:noInline')
  static Compaction getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Compaction>(Compaction.$_createMessage);
  static Compaction? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get summary => $_getSZ(0);
  @$pb.TagNumber(1)
  set summary($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasSummary() => $_has(0);
  @$pb.TagNumber(1)
  void clearSummary() => $_clearField(1);

  @$pb.TagNumber(2)
  $fixnum.Int64 get tokensBefore => $_getI64(1);
  @$pb.TagNumber(2)
  set tokensBefore($fixnum.Int64 value) => $_setInt64(1, value);
  @$pb.TagNumber(2)
  $core.bool hasTokensBefore() => $_has(1);
  @$pb.TagNumber(2)
  void clearTokensBefore() => $_clearField(2);

  /// Compacted while the agent was idle, just before its prompt cache
  /// expired (Settings.idle_compaction), not because the context was full or
  /// on request.
  @$pb.TagNumber(3)
  $core.bool get idle => $_getBF(2);
  @$pb.TagNumber(3)
  set idle($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasIdle() => $_has(2);
  @$pb.TagNumber(3)
  void clearIdle() => $_clearField(3);
}

/// Something the harness reports rather than the model: errors, stops.
class Notice extends $pb.GeneratedMessage {
  factory Notice({
    $core.String? text,
    NoticeLevel? level,
  }) {
    final result = Notice._();
    if (text != null) result.text = text;
    if (level != null) result.level = level;
    return result;
  }

  Notice._();

  factory Notice.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Notice()..mergeFromBuffer(data, registry);
  factory Notice.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Notice()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Notice',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Notice.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'text')
    ..aE<NoticeLevel>(2, _omitFieldNames ? '' : 'level',
        enumValues: NoticeLevel.values)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Notice clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Notice copyWith(void Function(Notice) updates) =>
      super.copyWith((message) => updates(message as Notice)) as Notice;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Notice() / Notice.new instead')
  static Notice create() => Notice._();
  static $pb.GeneratedMessage $_createMessage() => Notice._();
  @$core.override
  Notice createEmptyInstance() => Notice._();
  @$core.pragma('dart2js:noInline')
  static Notice getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Notice>(Notice.$_createMessage);
  static Notice? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get text => $_getSZ(0);
  @$pb.TagNumber(1)
  set text($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasText() => $_has(0);
  @$pb.TagNumber(1)
  void clearText() => $_clearField(1);

  @$pb.TagNumber(2)
  NoticeLevel get level => $_getN(1);
  @$pb.TagNumber(2)
  set level(NoticeLevel value) => $_setField(2, value);
  @$pb.TagNumber(2)
  $core.bool hasLevel() => $_has(1);
  @$pb.TagNumber(2)
  void clearLevel() => $_clearField(2);
}

/// The focused project's folder as a tree, flattened into the rows to show: a
/// folder's children follow it when it's expanded. Folders come before files,
/// each sorted by name. Sent after the Workspace on connect and whenever the
/// tree changes, including when agents change files.
class Explorer extends $pb.GeneratedMessage {
  factory Explorer({
    $core.String? projectId,
    $core.bool? showAll,
    $core.Iterable<FileEntry>? entries,
    $core.String? error,
    $core.Iterable<ExplorerRoot>? folders,
  }) {
    final result = Explorer._();
    if (projectId != null) result.projectId = projectId;
    if (showAll != null) result.showAll = showAll;
    if (entries != null) result.entries.addAll(entries);
    if (error != null) result.error = error;
    if (folders != null) result.folders.addAll(folders);
    return result;
  }

  Explorer._();

  factory Explorer.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Explorer()..mergeFromBuffer(data, registry);
  factory Explorer.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      Explorer()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'Explorer',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: Explorer.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'projectId')
    ..aOB(2, _omitFieldNames ? '' : 'showAll')
    ..pPM<FileEntry>(3, _omitFieldNames ? '' : 'entries',
        subBuilder: FileEntry.$_createMessage)
    ..aOS(4, _omitFieldNames ? '' : 'error')
    ..pPM<ExplorerRoot>(5, _omitFieldNames ? '' : 'folders',
        subBuilder: ExplorerRoot.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Explorer clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  Explorer copyWith(void Function(Explorer) updates) =>
      super.copyWith((message) => updates(message as Explorer)) as Explorer;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use Explorer() / Explorer.new instead')
  static Explorer create() => Explorer._();
  static $pb.GeneratedMessage $_createMessage() => Explorer._();
  @$core.override
  Explorer createEmptyInstance() => Explorer._();
  @$core.pragma('dart2js:noInline')
  static Explorer getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<Explorer>(Explorer.$_createMessage);
  static Explorer? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get projectId => $_getSZ(0);
  @$pb.TagNumber(1)
  set projectId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasProjectId() => $_has(0);
  @$pb.TagNumber(1)
  void clearProjectId() => $_clearField(1);

  /// Whether files .factoryignore hides are shown (as `hidden` rows).
  @$pb.TagNumber(2)
  $core.bool get showAll => $_getBF(1);
  @$pb.TagNumber(2)
  set showAll($core.bool value) => $_setBool(1, value);
  @$pb.TagNumber(2)
  $core.bool hasShowAll() => $_has(1);
  @$pb.TagNumber(2)
  void clearShowAll() => $_clearField(2);

  @$pb.TagNumber(3)
  $pb.PbList<FileEntry> get entries => $_getList(2);

  /// Set when the folder can't be read; `entries` is empty then.
  @$pb.TagNumber(4)
  $core.String get error => $_getSZ(3);
  @$pb.TagNumber(4)
  set error($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasError() => $_has(3);
  @$pb.TagNumber(4)
  void clearError() => $_clearField(4);

  /// One per folder of the project (Project.folders), in that order, each
  /// listed like the project's own folder above. The app shows the project's
  /// own folder as a root too when it has entries or there are no folders.
  /// Then one per agent in the project with its own checkout, in sidebar
  /// order (ExplorerRoot.agent_id set).
  @$pb.TagNumber(5)
  $pb.PbList<ExplorerRoot> get folders => $_getList(4);
}

/// One of the project's folders in the explorer, or an agent's own checkout
/// (the Own checkout extension). The app labels a folder with its name from
/// Project.folders, and a checkout as the agent's ("Backend's checkout").
class ExplorerRoot extends $pb.GeneratedMessage {
  factory ExplorerRoot({
    $core.String? folderId,
    $core.String? path,
    $core.Iterable<FileEntry>? entries,
    $core.String? error,
    $core.String? agentId,
    $core.String? branch,
  }) {
    final result = ExplorerRoot._();
    if (folderId != null) result.folderId = folderId;
    if (path != null) result.path = path;
    if (entries != null) result.entries.addAll(entries);
    if (error != null) result.error = error;
    if (agentId != null) result.agentId = agentId;
    if (branch != null) result.branch = branch;
    return result;
  }

  ExplorerRoot._();

  factory ExplorerRoot.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ExplorerRoot()..mergeFromBuffer(data, registry);
  factory ExplorerRoot.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ExplorerRoot()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ExplorerRoot',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ExplorerRoot.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'folderId')
    ..aOS(2, _omitFieldNames ? '' : 'path')
    ..pPM<FileEntry>(3, _omitFieldNames ? '' : 'entries',
        subBuilder: FileEntry.$_createMessage)
    ..aOS(4, _omitFieldNames ? '' : 'error')
    ..aOS(5, _omitFieldNames ? '' : 'agentId')
    ..aOS(6, _omitFieldNames ? '' : 'branch')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ExplorerRoot clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ExplorerRoot copyWith(void Function(ExplorerRoot) updates) =>
      super.copyWith((message) => updates(message as ExplorerRoot))
          as ExplorerRoot;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ExplorerRoot() / ExplorerRoot.new instead')
  static ExplorerRoot create() => ExplorerRoot._();
  static $pb.GeneratedMessage $_createMessage() => ExplorerRoot._();
  @$core.override
  ExplorerRoot createEmptyInstance() => ExplorerRoot._();
  @$core.pragma('dart2js:noInline')
  static ExplorerRoot getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ExplorerRoot>(
          ExplorerRoot.$_createMessage);
  static ExplorerRoot? _defaultInstance;

  /// Empty for an agent's checkout.
  @$pb.TagNumber(1)
  $core.String get folderId => $_getSZ(0);
  @$pb.TagNumber(1)
  set folderId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasFolderId() => $_has(0);
  @$pb.TagNumber(1)
  void clearFolderId() => $_clearField(1);

  /// Absolute.
  @$pb.TagNumber(2)
  $core.String get path => $_getSZ(1);
  @$pb.TagNumber(2)
  set path($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasPath() => $_has(1);
  @$pb.TagNumber(2)
  void clearPath() => $_clearField(2);

  /// Relative to `path`; listed as Explorer.entries are, with the folder's
  /// own .factoryignore.
  @$pb.TagNumber(3)
  $pb.PbList<FileEntry> get entries => $_getList(2);

  /// Set when the folder can't be read; `entries` is empty then.
  @$pb.TagNumber(4)
  $core.String get error => $_getSZ(3);
  @$pb.TagNumber(4)
  set error($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasError() => $_has(3);
  @$pb.TagNumber(4)
  void clearError() => $_clearField(4);

  /// An agent's own checkout: the agent's id, and its branch
  /// ("factory/backend"). The path is where in the checkout the agent works.
  @$pb.TagNumber(5)
  $core.String get agentId => $_getSZ(4);
  @$pb.TagNumber(5)
  set agentId($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasAgentId() => $_has(4);
  @$pb.TagNumber(5)
  void clearAgentId() => $_clearField(5);

  @$pb.TagNumber(6)
  $core.String get branch => $_getSZ(5);
  @$pb.TagNumber(6)
  set branch($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasBranch() => $_has(5);
  @$pb.TagNumber(6)
  void clearBranch() => $_clearField(6);
}

class FileEntry extends $pb.GeneratedMessage {
  factory FileEntry({
    $core.String? path,
    $core.String? name,
    $core.int? depth,
    $core.bool? isDir,
    $core.bool? expanded,
    $core.bool? hidden,
  }) {
    final result = FileEntry._();
    if (path != null) result.path = path;
    if (name != null) result.name = name;
    if (depth != null) result.depth = depth;
    if (isDir != null) result.isDir = isDir;
    if (expanded != null) result.expanded = expanded;
    if (hidden != null) result.hidden = hidden;
    return result;
  }

  FileEntry._();

  factory FileEntry.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      FileEntry()..mergeFromBuffer(data, registry);
  factory FileEntry.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      FileEntry()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'FileEntry',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: FileEntry.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'path')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aI(3, _omitFieldNames ? '' : 'depth', fieldType: $pb.PbFieldType.OU3)
    ..aOB(4, _omitFieldNames ? '' : 'isDir')
    ..aOB(5, _omitFieldNames ? '' : 'expanded')
    ..aOB(6, _omitFieldNames ? '' : 'hidden')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  FileEntry clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  FileEntry copyWith(void Function(FileEntry) updates) =>
      super.copyWith((message) => updates(message as FileEntry)) as FileEntry;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use FileEntry() / FileEntry.new instead')
  static FileEntry create() => FileEntry._();
  static $pb.GeneratedMessage $_createMessage() => FileEntry._();
  @$core.override
  FileEntry createEmptyInstance() => FileEntry._();
  @$core.pragma('dart2js:noInline')
  static FileEntry getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<FileEntry>(FileEntry.$_createMessage);
  static FileEntry? _defaultInstance;

  /// Relative to the project folder, with "/" between names.
  @$pb.TagNumber(1)
  $core.String get path => $_getSZ(0);
  @$pb.TagNumber(1)
  set path($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasPath() => $_has(0);
  @$pb.TagNumber(1)
  void clearPath() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// 0 for the project folder's own entries.
  @$pb.TagNumber(3)
  $core.int get depth => $_getIZ(2);
  @$pb.TagNumber(3)
  set depth($core.int value) => $_setUnsignedInt32(2, value);
  @$pb.TagNumber(3)
  $core.bool hasDepth() => $_has(2);
  @$pb.TagNumber(3)
  void clearDepth() => $_clearField(3);

  @$pb.TagNumber(4)
  $core.bool get isDir => $_getBF(3);
  @$pb.TagNumber(4)
  set isDir($core.bool value) => $_setBool(3, value);
  @$pb.TagNumber(4)
  $core.bool hasIsDir() => $_has(3);
  @$pb.TagNumber(4)
  void clearIsDir() => $_clearField(4);

  @$pb.TagNumber(5)
  $core.bool get expanded => $_getBF(4);
  @$pb.TagNumber(5)
  set expanded($core.bool value) => $_setBool(4, value);
  @$pb.TagNumber(5)
  $core.bool hasExpanded() => $_has(4);
  @$pb.TagNumber(5)
  void clearExpanded() => $_clearField(5);

  /// Matched by .factoryignore; only sent when `show_all` is on.
  @$pb.TagNumber(6)
  $core.bool get hidden => $_getBF(5);
  @$pb.TagNumber(6)
  set hidden($core.bool value) => $_setBool(5, value);
  @$pb.TagNumber(6)
  $core.bool hasHidden() => $_has(5);
  @$pb.TagNumber(6)
  void clearHidden() => $_clearField(6);
}

/// Part of the Workspace: the same on every device. Metadata only.
class VaultState extends $pb.GeneratedMessage {
  factory VaultState({
    $core.bool? exists,
    $core.bool? unlocked,
    $core.bool? lockedMode,
    $core.Iterable<SecretInfo>? secrets,
    $core.Iterable<SecretRequest>? requests,
  }) {
    final result = VaultState._();
    if (exists != null) result.exists = exists;
    if (unlocked != null) result.unlocked = unlocked;
    if (lockedMode != null) result.lockedMode = lockedMode;
    if (secrets != null) result.secrets.addAll(secrets);
    if (requests != null) result.requests.addAll(requests);
    return result;
  }

  VaultState._();

  factory VaultState.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      VaultState()..mergeFromBuffer(data, registry);
  factory VaultState.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      VaultState()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'VaultState',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: VaultState.$_createMessage)
    ..aOB(1, _omitFieldNames ? '' : 'exists')
    ..aOB(2, _omitFieldNames ? '' : 'unlocked')
    ..aOB(3, _omitFieldNames ? '' : 'lockedMode')
    ..pPM<SecretInfo>(4, _omitFieldNames ? '' : 'secrets',
        subBuilder: SecretInfo.$_createMessage)
    ..pPM<SecretRequest>(5, _omitFieldNames ? '' : 'requests',
        subBuilder: SecretRequest.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  VaultState clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  VaultState copyWith(void Function(VaultState) updates) =>
      super.copyWith((message) => updates(message as VaultState)) as VaultState;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use VaultState() / VaultState.new instead')
  static VaultState create() => VaultState._();
  static $pb.GeneratedMessage $_createMessage() => VaultState._();
  @$core.override
  VaultState createEmptyInstance() => VaultState._();
  @$core.pragma('dart2js:noInline')
  static VaultState getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<VaultState>(VaultState.$_createMessage);
  static VaultState? _defaultInstance;

  /// There's a vault: the backend makes one when it starts, so false only
  /// when it couldn't. Without one, everything else is empty.
  @$pb.TagNumber(1)
  $core.bool get exists => $_getBF(0);
  @$pb.TagNumber(1)
  set exists($core.bool value) => $_setBool(0, value);
  @$pb.TagNumber(1)
  $core.bool hasExists() => $_has(0);
  @$pb.TagNumber(1)
  void clearExists() => $_clearField(1);

  /// The backend holds the vault's key now. False only in locked mode, until
  /// the person unlocks it after the backend started (UnlockVault); agents
  /// can't use secrets meanwhile.
  @$pb.TagNumber(2)
  $core.bool get unlocked => $_getBF(1);
  @$pb.TagNumber(2)
  set unlocked($core.bool value) => $_setBool(1, value);
  @$pb.TagNumber(2)
  $core.bool hasUnlocked() => $_has(1);
  @$pb.TagNumber(2)
  void clearUnlocked() => $_clearField(2);

  /// No key is kept on disk: the vault opens only with the passphrase (or the
  /// recovery key) after each start of the backend. Off (always, unless the
  /// person gave it a key of their own and turned it on): it opens by itself.
  @$pb.TagNumber(3)
  $core.bool get lockedMode => $_getBF(2);
  @$pb.TagNumber(3)
  set lockedMode($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasLockedMode() => $_has(2);
  @$pb.TagNumber(3)
  void clearLockedMode() => $_clearField(3);

  /// By name.
  @$pb.TagNumber(4)
  $pb.PbList<SecretInfo> get secrets => $_getList(3);

  /// Agents' requests still waiting for the person, oldest first. Each is also
  /// an entry in its agent's conversation.
  @$pb.TagNumber(5)
  $pb.PbList<SecretRequest> get requests => $_getList(4);
}

class SecretInfo extends $pb.GeneratedMessage {
  factory SecretInfo({
    $core.String? id,
    $core.String? name,
    $core.String? description,
    $core.Iterable<SecretVariable>? variables,
    $core.Iterable<SecretGrant>? grants,
    $fixnum.Int64? createdMs,
    $fixnum.Int64? updatedMs,
    $core.int? version,
    $core.String? lastUsedBy,
    $fixnum.Int64? lastUsedMs,
    $core.Iterable<$core.String>? warnings,
    $core.String? ownerAgentId,
  }) {
    final result = SecretInfo._();
    if (id != null) result.id = id;
    if (name != null) result.name = name;
    if (description != null) result.description = description;
    if (variables != null) result.variables.addAll(variables);
    if (grants != null) result.grants.addAll(grants);
    if (createdMs != null) result.createdMs = createdMs;
    if (updatedMs != null) result.updatedMs = updatedMs;
    if (version != null) result.version = version;
    if (lastUsedBy != null) result.lastUsedBy = lastUsedBy;
    if (lastUsedMs != null) result.lastUsedMs = lastUsedMs;
    if (warnings != null) result.warnings.addAll(warnings);
    if (ownerAgentId != null) result.ownerAgentId = ownerAgentId;
    return result;
  }

  SecretInfo._();

  factory SecretInfo.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretInfo()..mergeFromBuffer(data, registry);
  factory SecretInfo.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretInfo()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SecretInfo',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SecretInfo.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'description')
    ..pPM<SecretVariable>(4, _omitFieldNames ? '' : 'variables',
        subBuilder: SecretVariable.$_createMessage)
    ..pPM<SecretGrant>(5, _omitFieldNames ? '' : 'grants',
        subBuilder: SecretGrant.$_createMessage)
    ..aInt64(6, _omitFieldNames ? '' : 'createdMs')
    ..aInt64(7, _omitFieldNames ? '' : 'updatedMs')
    ..aI(8, _omitFieldNames ? '' : 'version', fieldType: $pb.PbFieldType.OU3)
    ..aOS(9, _omitFieldNames ? '' : 'lastUsedBy')
    ..aInt64(10, _omitFieldNames ? '' : 'lastUsedMs')
    ..pPS(11, _omitFieldNames ? '' : 'warnings')
    ..aOS(12, _omitFieldNames ? '' : 'ownerAgentId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretInfo clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretInfo copyWith(void Function(SecretInfo) updates) =>
      super.copyWith((message) => updates(message as SecretInfo)) as SecretInfo;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SecretInfo() / SecretInfo.new instead')
  static SecretInfo create() => SecretInfo._();
  static $pb.GeneratedMessage $_createMessage() => SecretInfo._();
  @$core.override
  SecretInfo createEmptyInstance() => SecretInfo._();
  @$core.pragma('dart2js:noInline')
  static SecretInfo getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<SecretInfo>(SecretInfo.$_createMessage);
  static SecretInfo? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// How agents name it (bash's `secrets`), such as "AWS". Unique.
  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  /// What it is and what it's for; agents read it.
  @$pb.TagNumber(3)
  $core.String get description => $_getSZ(2);
  @$pb.TagNumber(3)
  set description($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasDescription() => $_has(2);
  @$pb.TagNumber(3)
  void clearDescription() => $_clearField(3);

  @$pb.TagNumber(4)
  $pb.PbList<SecretVariable> get variables => $_getList(3);

  /// The agents that may use it, by when they were granted it.
  @$pb.TagNumber(5)
  $pb.PbList<SecretGrant> get grants => $_getList(4);

  @$pb.TagNumber(6)
  $fixnum.Int64 get createdMs => $_getI64(5);
  @$pb.TagNumber(6)
  set createdMs($fixnum.Int64 value) => $_setInt64(5, value);
  @$pb.TagNumber(6)
  $core.bool hasCreatedMs() => $_has(5);
  @$pb.TagNumber(6)
  void clearCreatedMs() => $_clearField(6);

  @$pb.TagNumber(7)
  $fixnum.Int64 get updatedMs => $_getI64(6);
  @$pb.TagNumber(7)
  set updatedMs($fixnum.Int64 value) => $_setInt64(6, value);
  @$pb.TagNumber(7)
  $core.bool hasUpdatedMs() => $_has(6);
  @$pb.TagNumber(7)
  void clearUpdatedMs() => $_clearField(7);

  /// Counts up each time its values change.
  @$pb.TagNumber(8)
  $core.int get version => $_getIZ(7);
  @$pb.TagNumber(8)
  set version($core.int value) => $_setUnsignedInt32(7, value);
  @$pb.TagNumber(8)
  $core.bool hasVersion() => $_has(7);
  @$pb.TagNumber(8)
  void clearVersion() => $_clearField(8);

  /// The last agent that used it, and when; empty and 0 if none has.
  @$pb.TagNumber(9)
  $core.String get lastUsedBy => $_getSZ(8);
  @$pb.TagNumber(9)
  set lastUsedBy($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasLastUsedBy() => $_has(8);
  @$pb.TagNumber(9)
  void clearLastUsedBy() => $_clearField(9);

  @$pb.TagNumber(10)
  $fixnum.Int64 get lastUsedMs => $_getI64(9);
  @$pb.TagNumber(10)
  set lastUsedMs($fixnum.Int64 value) => $_setInt64(9, value);
  @$pb.TagNumber(10)
  $core.bool hasLastUsedMs() => $_has(9);
  @$pb.TagNumber(10)
  void clearLastUsedMs() => $_clearField(10);

  /// What the person should know, such as "MY_PIN is too short to hide in
  /// agents' output".
  @$pb.TagNumber(11)
  $pb.PbList<$core.String> get warnings => $_getList(10);

  /// The agent that saved it (a key it made itself); empty when the person
  /// added it.
  @$pb.TagNumber(12)
  $core.String get ownerAgentId => $_getSZ(11);
  @$pb.TagNumber(12)
  set ownerAgentId($core.String value) => $_setString(11, value);
  @$pb.TagNumber(12)
  $core.bool hasOwnerAgentId() => $_has(11);
  @$pb.TagNumber(12)
  void clearOwnerAgentId() => $_clearField(12);
}

class SecretVariable extends $pb.GeneratedMessage {
  factory SecretVariable({
    $core.String? name,
    $core.String? hint,
    $core.bool? file,
  }) {
    final result = SecretVariable._();
    if (name != null) result.name = name;
    if (hint != null) result.hint = hint;
    if (file != null) result.file = file;
    return result;
  }

  SecretVariable._();

  factory SecretVariable.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretVariable()..mergeFromBuffer(data, registry);
  factory SecretVariable.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretVariable()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SecretVariable',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SecretVariable.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'name')
    ..aOS(2, _omitFieldNames ? '' : 'hint')
    ..aOB(3, _omitFieldNames ? '' : 'file')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretVariable clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretVariable copyWith(void Function(SecretVariable) updates) =>
      super.copyWith((message) => updates(message as SecretVariable))
          as SecretVariable;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SecretVariable() / SecretVariable.new instead')
  static SecretVariable create() => SecretVariable._();
  static $pb.GeneratedMessage $_createMessage() => SecretVariable._();
  @$core.override
  SecretVariable createEmptyInstance() => SecretVariable._();
  @$core.pragma('dart2js:noInline')
  static SecretVariable getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<SecretVariable>(
          SecretVariable.$_createMessage);
  static SecretVariable? _defaultInstance;

  /// The environment variable, such as AWS_SECRET_ACCESS_KEY.
  @$pb.TagNumber(1)
  $core.String get name => $_getSZ(0);
  @$pb.TagNumber(1)
  set name($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasName() => $_has(0);
  @$pb.TagNumber(1)
  void clearName() => $_clearField(1);

  /// The value's last 4 characters, to tell values apart ("••••3f9a"); empty
  /// when the value is shorter than 12 characters.
  @$pb.TagNumber(2)
  $core.String get hint => $_getSZ(1);
  @$pb.TagNumber(2)
  set hint($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasHint() => $_has(1);
  @$pb.TagNumber(2)
  void clearHint() => $_clearField(2);

  /// The value is written to a private file, and `name` holds its path (as
  /// KUBECONFIG or GOOGLE_APPLICATION_CREDENTIALS expect).
  @$pb.TagNumber(3)
  $core.bool get file => $_getBF(2);
  @$pb.TagNumber(3)
  set file($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasFile() => $_has(2);
  @$pb.TagNumber(3)
  void clearFile() => $_clearField(3);
}

class SecretGrant extends $pb.GeneratedMessage {
  factory SecretGrant({
    $core.String? agentId,
    $fixnum.Int64? grantedMs,
    $core.int? runningTasks,
  }) {
    final result = SecretGrant._();
    if (agentId != null) result.agentId = agentId;
    if (grantedMs != null) result.grantedMs = grantedMs;
    if (runningTasks != null) result.runningTasks = runningTasks;
    return result;
  }

  SecretGrant._();

  factory SecretGrant.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretGrant()..mergeFromBuffer(data, registry);
  factory SecretGrant.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretGrant()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SecretGrant',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SecretGrant.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..aInt64(2, _omitFieldNames ? '' : 'grantedMs')
    ..aI(3, _omitFieldNames ? '' : 'runningTasks',
        fieldType: $pb.PbFieldType.OU3)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretGrant clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretGrant copyWith(void Function(SecretGrant) updates) =>
      super.copyWith((message) => updates(message as SecretGrant))
          as SecretGrant;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SecretGrant() / SecretGrant.new instead')
  static SecretGrant create() => SecretGrant._();
  static $pb.GeneratedMessage $_createMessage() => SecretGrant._();
  @$core.override
  SecretGrant createEmptyInstance() => SecretGrant._();
  @$core.pragma('dart2js:noInline')
  static SecretGrant getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<SecretGrant>(
          SecretGrant.$_createMessage);
  static SecretGrant? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  @$pb.TagNumber(2)
  $fixnum.Int64 get grantedMs => $_getI64(1);
  @$pb.TagNumber(2)
  set grantedMs($fixnum.Int64 value) => $_setInt64(1, value);
  @$pb.TagNumber(2)
  $core.bool hasGrantedMs() => $_has(1);
  @$pb.TagNumber(2)
  void clearGrantedMs() => $_clearField(2);

  /// How many of the agent's background tasks running now got this secret:
  /// what RevokeSecret's stop_running_tasks would stop.
  @$pb.TagNumber(3)
  $core.int get runningTasks => $_getIZ(2);
  @$pb.TagNumber(3)
  set runningTasks($core.int value) => $_setUnsignedInt32(2, value);
  @$pb.TagNumber(3)
  $core.bool hasRunningTasks() => $_has(2);
  @$pb.TagNumber(3)
  void clearRunningTasks() => $_clearField(3);
}

/// A variable with its value, in requests from the app and in SecretValue.
class SecretEntry extends $pb.GeneratedMessage {
  factory SecretEntry({
    $core.String? variable,
    $core.String? value,
    $core.bool? file,
  }) {
    final result = SecretEntry._();
    if (variable != null) result.variable = variable;
    if (value != null) result.value = value;
    if (file != null) result.file = file;
    return result;
  }

  SecretEntry._();

  factory SecretEntry.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretEntry()..mergeFromBuffer(data, registry);
  factory SecretEntry.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretEntry()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SecretEntry',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SecretEntry.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'variable')
    ..aOS(2, _omitFieldNames ? '' : 'value')
    ..aOB(3, _omitFieldNames ? '' : 'file')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretEntry clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretEntry copyWith(void Function(SecretEntry) updates) =>
      super.copyWith((message) => updates(message as SecretEntry))
          as SecretEntry;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SecretEntry() / SecretEntry.new instead')
  static SecretEntry create() => SecretEntry._();
  static $pb.GeneratedMessage $_createMessage() => SecretEntry._();
  @$core.override
  SecretEntry createEmptyInstance() => SecretEntry._();
  @$core.pragma('dart2js:noInline')
  static SecretEntry getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<SecretEntry>(
          SecretEntry.$_createMessage);
  static SecretEntry? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get variable => $_getSZ(0);
  @$pb.TagNumber(1)
  set variable($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasVariable() => $_has(0);
  @$pb.TagNumber(1)
  void clearVariable() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get value => $_getSZ(1);
  @$pb.TagNumber(2)
  set value($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasValue() => $_has(1);
  @$pb.TagNumber(2)
  void clearValue() => $_clearField(2);

  /// See SecretVariable.file.
  @$pb.TagNumber(3)
  $core.bool get file => $_getBF(2);
  @$pb.TagNumber(3)
  set file($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasFile() => $_has(2);
  @$pb.TagNumber(3)
  void clearFile() => $_clearField(3);
}

/// No longer needed: the person's requests come from their app, signed in to
/// the backend as one of their devices, and agents can't make them. Leave it
/// out. Only a vault in locked mode, still locked, takes its passphrase or
/// recovery key here to open it, as UnlockVault does.
class VaultProof extends $pb.GeneratedMessage {
  factory VaultProof({
    $core.String? passphrase,
  }) {
    final result = VaultProof._();
    if (passphrase != null) result.passphrase = passphrase;
    return result;
  }

  VaultProof._();

  factory VaultProof.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      VaultProof()..mergeFromBuffer(data, registry);
  factory VaultProof.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      VaultProof()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'VaultProof',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: VaultProof.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'passphrase')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  VaultProof clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  VaultProof copyWith(void Function(VaultProof) updates) =>
      super.copyWith((message) => updates(message as VaultProof)) as VaultProof;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use VaultProof() / VaultProof.new instead')
  static VaultProof create() => VaultProof._();
  static $pb.GeneratedMessage $_createMessage() => VaultProof._();
  @$core.override
  VaultProof createEmptyInstance() => VaultProof._();
  @$core.pragma('dart2js:noInline')
  static VaultProof getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<VaultProof>(VaultProof.$_createMessage);
  static VaultProof? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get passphrase => $_getSZ(0);
  @$pb.TagNumber(1)
  set passphrase($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasPassphrase() => $_has(0);
  @$pb.TagNumber(1)
  void clearPassphrase() => $_clearField(1);
}

/// Unused: the backend makes the vault by itself when it starts. Refused
/// with ERROR_CODE_INVALID_REQUEST (there's one already).
class CreateVault extends $pb.GeneratedMessage {
  factory CreateVault({
    $core.String? passphrase,
    $core.bool? lockedMode,
  }) {
    final result = CreateVault._();
    if (passphrase != null) result.passphrase = passphrase;
    if (lockedMode != null) result.lockedMode = lockedMode;
    return result;
  }

  CreateVault._();

  factory CreateVault.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CreateVault()..mergeFromBuffer(data, registry);
  factory CreateVault.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CreateVault()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'CreateVault',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: CreateVault.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'passphrase')
    ..aOB(2, _omitFieldNames ? '' : 'lockedMode')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CreateVault clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CreateVault copyWith(void Function(CreateVault) updates) =>
      super.copyWith((message) => updates(message as CreateVault))
          as CreateVault;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use CreateVault() / CreateVault.new instead')
  static CreateVault create() => CreateVault._();
  static $pb.GeneratedMessage $_createMessage() => CreateVault._();
  @$core.override
  CreateVault createEmptyInstance() => CreateVault._();
  @$core.pragma('dart2js:noInline')
  static CreateVault getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<CreateVault>(
          CreateVault.$_createMessage);
  static CreateVault? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get passphrase => $_getSZ(0);
  @$pb.TagNumber(1)
  set passphrase($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasPassphrase() => $_has(0);
  @$pb.TagNumber(1)
  void clearPassphrase() => $_clearField(1);

  /// See VaultState.locked_mode.
  @$pb.TagNumber(2)
  $core.bool get lockedMode => $_getBF(1);
  @$pb.TagNumber(2)
  set lockedMode($core.bool value) => $_setBool(1, value);
  @$pb.TagNumber(2)
  $core.bool hasLockedMode() => $_has(1);
  @$pb.TagNumber(2)
  void clearLockedMode() => $_clearField(2);
}

/// Opens a vault in locked mode after a restart. `passphrase` may be the
/// recovery key.
class UnlockVault extends $pb.GeneratedMessage {
  factory UnlockVault({
    $core.String? passphrase,
  }) {
    final result = UnlockVault._();
    if (passphrase != null) result.passphrase = passphrase;
    return result;
  }

  UnlockVault._();

  factory UnlockVault.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UnlockVault()..mergeFromBuffer(data, registry);
  factory UnlockVault.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UnlockVault()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'UnlockVault',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: UnlockVault.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'passphrase')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UnlockVault clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UnlockVault copyWith(void Function(UnlockVault) updates) =>
      super.copyWith((message) => updates(message as UnlockVault))
          as UnlockVault;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use UnlockVault() / UnlockVault.new instead')
  static UnlockVault create() => UnlockVault._();
  static $pb.GeneratedMessage $_createMessage() => UnlockVault._();
  @$core.override
  UnlockVault createEmptyInstance() => UnlockVault._();
  @$core.pragma('dart2js:noInline')
  static UnlockVault getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<UnlockVault>(
          UnlockVault.$_createMessage);
  static UnlockVault? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get passphrase => $_getSZ(0);
  @$pb.TagNumber(1)
  set passphrase($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasPassphrase() => $_has(0);
  @$pb.TagNumber(1)
  void clearPassphrase() => $_clearField(1);
}

/// Unused: there's no unlocked window any more. Answered with VaultSession 0.
class LockVaultSession extends $pb.GeneratedMessage {
  factory LockVaultSession() => LockVaultSession._();

  LockVaultSession._();

  factory LockVaultSession.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      LockVaultSession()..mergeFromBuffer(data, registry);
  factory LockVaultSession.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      LockVaultSession()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'LockVaultSession',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: LockVaultSession.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  LockVaultSession clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  LockVaultSession copyWith(void Function(LockVaultSession) updates) =>
      super.copyWith((message) => updates(message as LockVaultSession))
          as LockVaultSession;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use LockVaultSession() / LockVaultSession.new instead')
  static LockVaultSession create() => LockVaultSession._();
  static $pb.GeneratedMessage $_createMessage() => LockVaultSession._();
  @$core.override
  LockVaultSession createEmptyInstance() => LockVaultSession._();
  @$core.pragma('dart2js:noInline')
  static LockVaultSession getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<LockVaultSession>(
          LockVaultSession.$_createMessage);
  static LockVaultSession? _defaultInstance;
}

/// Adds a secret, granted to no one. No proof needed.
class CreateSecret extends $pb.GeneratedMessage {
  factory CreateSecret({
    $core.String? name,
    $core.String? description,
    $core.Iterable<SecretEntry>? entries,
  }) {
    final result = CreateSecret._();
    if (name != null) result.name = name;
    if (description != null) result.description = description;
    if (entries != null) result.entries.addAll(entries);
    return result;
  }

  CreateSecret._();

  factory CreateSecret.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CreateSecret()..mergeFromBuffer(data, registry);
  factory CreateSecret.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      CreateSecret()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'CreateSecret',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: CreateSecret.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'name')
    ..aOS(2, _omitFieldNames ? '' : 'description')
    ..pPM<SecretEntry>(3, _omitFieldNames ? '' : 'entries',
        subBuilder: SecretEntry.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CreateSecret clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  CreateSecret copyWith(void Function(CreateSecret) updates) =>
      super.copyWith((message) => updates(message as CreateSecret))
          as CreateSecret;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use CreateSecret() / CreateSecret.new instead')
  static CreateSecret create() => CreateSecret._();
  static $pb.GeneratedMessage $_createMessage() => CreateSecret._();
  @$core.override
  CreateSecret createEmptyInstance() => CreateSecret._();
  @$core.pragma('dart2js:noInline')
  static CreateSecret getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<CreateSecret>(
          CreateSecret.$_createMessage);
  static CreateSecret? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get name => $_getSZ(0);
  @$pb.TagNumber(1)
  set name($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasName() => $_has(0);
  @$pb.TagNumber(1)
  void clearName() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get description => $_getSZ(1);
  @$pb.TagNumber(2)
  set description($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasDescription() => $_has(1);
  @$pb.TagNumber(2)
  void clearDescription() => $_clearField(2);

  @$pb.TagNumber(3)
  $pb.PbList<SecretEntry> get entries => $_getList(2);
}

/// Changes a secret's name and description, and with `entries`, replaces its
/// variables and values (a new version). Empty `entries`: values unchanged.
/// No proof needed: agents granted it are told it changed.
class UpdateSecret extends $pb.GeneratedMessage {
  factory UpdateSecret({
    $core.String? id,
    $core.String? name,
    $core.String? description,
    $core.Iterable<SecretEntry>? entries,
  }) {
    final result = UpdateSecret._();
    if (id != null) result.id = id;
    if (name != null) result.name = name;
    if (description != null) result.description = description;
    if (entries != null) result.entries.addAll(entries);
    return result;
  }

  UpdateSecret._();

  factory UpdateSecret.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UpdateSecret()..mergeFromBuffer(data, registry);
  factory UpdateSecret.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      UpdateSecret()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'UpdateSecret',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: UpdateSecret.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'description')
    ..pPM<SecretEntry>(4, _omitFieldNames ? '' : 'entries',
        subBuilder: SecretEntry.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UpdateSecret clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  UpdateSecret copyWith(void Function(UpdateSecret) updates) =>
      super.copyWith((message) => updates(message as UpdateSecret))
          as UpdateSecret;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use UpdateSecret() / UpdateSecret.new instead')
  static UpdateSecret create() => UpdateSecret._();
  static $pb.GeneratedMessage $_createMessage() => UpdateSecret._();
  @$core.override
  UpdateSecret createEmptyInstance() => UpdateSecret._();
  @$core.pragma('dart2js:noInline')
  static UpdateSecret getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<UpdateSecret>(
          UpdateSecret.$_createMessage);
  static UpdateSecret? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.String get description => $_getSZ(2);
  @$pb.TagNumber(3)
  set description($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasDescription() => $_has(2);
  @$pb.TagNumber(3)
  void clearDescription() => $_clearField(3);

  @$pb.TagNumber(4)
  $pb.PbList<SecretEntry> get entries => $_getList(3);
}

class DeleteSecret extends $pb.GeneratedMessage {
  factory DeleteSecret({
    $core.String? id,
    VaultProof? proof,
  }) {
    final result = DeleteSecret._();
    if (id != null) result.id = id;
    if (proof != null) result.proof = proof;
    return result;
  }

  DeleteSecret._();

  factory DeleteSecret.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DeleteSecret()..mergeFromBuffer(data, registry);
  factory DeleteSecret.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DeleteSecret()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'DeleteSecret',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: DeleteSecret.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOM<VaultProof>(2, _omitFieldNames ? '' : 'proof',
        subBuilder: VaultProof.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DeleteSecret clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DeleteSecret copyWith(void Function(DeleteSecret) updates) =>
      super.copyWith((message) => updates(message as DeleteSecret))
          as DeleteSecret;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use DeleteSecret() / DeleteSecret.new instead')
  static DeleteSecret create() => DeleteSecret._();
  static $pb.GeneratedMessage $_createMessage() => DeleteSecret._();
  @$core.override
  DeleteSecret createEmptyInstance() => DeleteSecret._();
  @$core.pragma('dart2js:noInline')
  static DeleteSecret getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<DeleteSecret>(
          DeleteSecret.$_createMessage);
  static DeleteSecret? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  VaultProof get proof => $_getN(1);
  @$pb.TagNumber(2)
  set proof(VaultProof value) => $_setField(2, value);
  @$pb.TagNumber(2)
  $core.bool hasProof() => $_has(1);
  @$pb.TagNumber(2)
  void clearProof() => $_clearField(2);
  @$pb.TagNumber(2)
  VaultProof ensureProof() => $_ensure(1);
}

class GrantSecret extends $pb.GeneratedMessage {
  factory GrantSecret({
    $core.String? id,
    $core.String? agentId,
    VaultProof? proof,
  }) {
    final result = GrantSecret._();
    if (id != null) result.id = id;
    if (agentId != null) result.agentId = agentId;
    if (proof != null) result.proof = proof;
    return result;
  }

  GrantSecret._();

  factory GrantSecret.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      GrantSecret()..mergeFromBuffer(data, registry);
  factory GrantSecret.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      GrantSecret()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'GrantSecret',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: GrantSecret.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'agentId')
    ..aOM<VaultProof>(3, _omitFieldNames ? '' : 'proof',
        subBuilder: VaultProof.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  GrantSecret clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  GrantSecret copyWith(void Function(GrantSecret) updates) =>
      super.copyWith((message) => updates(message as GrantSecret))
          as GrantSecret;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use GrantSecret() / GrantSecret.new instead')
  static GrantSecret create() => GrantSecret._();
  static $pb.GeneratedMessage $_createMessage() => GrantSecret._();
  @$core.override
  GrantSecret createEmptyInstance() => GrantSecret._();
  @$core.pragma('dart2js:noInline')
  static GrantSecret getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<GrantSecret>(
          GrantSecret.$_createMessage);
  static GrantSecret? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get agentId => $_getSZ(1);
  @$pb.TagNumber(2)
  set agentId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasAgentId() => $_has(1);
  @$pb.TagNumber(2)
  void clearAgentId() => $_clearField(2);

  @$pb.TagNumber(3)
  VaultProof get proof => $_getN(2);
  @$pb.TagNumber(3)
  set proof(VaultProof value) => $_setField(3, value);
  @$pb.TagNumber(3)
  $core.bool hasProof() => $_has(2);
  @$pb.TagNumber(3)
  void clearProof() => $_clearField(3);
  @$pb.TagNumber(3)
  VaultProof ensureProof() => $_ensure(2);
}

/// No proof needed: it only takes access away. The agent is told.
class RevokeSecret extends $pb.GeneratedMessage {
  factory RevokeSecret({
    $core.String? id,
    $core.String? agentId,
    $core.bool? stopRunningTasks,
  }) {
    final result = RevokeSecret._();
    if (id != null) result.id = id;
    if (agentId != null) result.agentId = agentId;
    if (stopRunningTasks != null) result.stopRunningTasks = stopRunningTasks;
    return result;
  }

  RevokeSecret._();

  factory RevokeSecret.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      RevokeSecret()..mergeFromBuffer(data, registry);
  factory RevokeSecret.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      RevokeSecret()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'RevokeSecret',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: RevokeSecret.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'agentId')
    ..aOB(3, _omitFieldNames ? '' : 'stopRunningTasks')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  RevokeSecret clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  RevokeSecret copyWith(void Function(RevokeSecret) updates) =>
      super.copyWith((message) => updates(message as RevokeSecret))
          as RevokeSecret;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use RevokeSecret() / RevokeSecret.new instead')
  static RevokeSecret create() => RevokeSecret._();
  static $pb.GeneratedMessage $_createMessage() => RevokeSecret._();
  @$core.override
  RevokeSecret createEmptyInstance() => RevokeSecret._();
  @$core.pragma('dart2js:noInline')
  static RevokeSecret getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<RevokeSecret>(
          RevokeSecret.$_createMessage);
  static RevokeSecret? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get agentId => $_getSZ(1);
  @$pb.TagNumber(2)
  set agentId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasAgentId() => $_has(1);
  @$pb.TagNumber(2)
  void clearAgentId() => $_clearField(2);

  /// Also stops the agent's background tasks that got the secret.
  @$pb.TagNumber(3)
  $core.bool get stopRunningTasks => $_getBF(2);
  @$pb.TagNumber(3)
  set stopRunningTasks($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasStopRunningTasks() => $_has(2);
  @$pb.TagNumber(3)
  void clearStopRunningTasks() => $_clearField(3);
}

class RevealSecret extends $pb.GeneratedMessage {
  factory RevealSecret({
    $core.String? id,
    VaultProof? proof,
  }) {
    final result = RevealSecret._();
    if (id != null) result.id = id;
    if (proof != null) result.proof = proof;
    return result;
  }

  RevealSecret._();

  factory RevealSecret.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      RevealSecret()..mergeFromBuffer(data, registry);
  factory RevealSecret.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      RevealSecret()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'RevealSecret',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: RevealSecret.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOM<VaultProof>(2, _omitFieldNames ? '' : 'proof',
        subBuilder: VaultProof.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  RevealSecret clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  RevealSecret copyWith(void Function(RevealSecret) updates) =>
      super.copyWith((message) => updates(message as RevealSecret))
          as RevealSecret;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use RevealSecret() / RevealSecret.new instead')
  static RevealSecret create() => RevealSecret._();
  static $pb.GeneratedMessage $_createMessage() => RevealSecret._();
  @$core.override
  RevealSecret createEmptyInstance() => RevealSecret._();
  @$core.pragma('dart2js:noInline')
  static RevealSecret getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<RevealSecret>(
          RevealSecret.$_createMessage);
  static RevealSecret? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  VaultProof get proof => $_getN(1);
  @$pb.TagNumber(2)
  set proof(VaultProof value) => $_setField(2, value);
  @$pb.TagNumber(2)
  $core.bool hasProof() => $_has(1);
  @$pb.TagNumber(2)
  void clearProof() => $_clearField(2);
  @$pb.TagNumber(2)
  VaultProof ensureProof() => $_ensure(1);
}

/// Reads a pasted .env: answered with the variables found, without values.
class PreviewEnv extends $pb.GeneratedMessage {
  factory PreviewEnv({
    $core.String? text,
  }) {
    final result = PreviewEnv._();
    if (text != null) result.text = text;
    return result;
  }

  PreviewEnv._();

  factory PreviewEnv.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      PreviewEnv()..mergeFromBuffer(data, registry);
  factory PreviewEnv.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      PreviewEnv()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'PreviewEnv',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: PreviewEnv.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'text')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PreviewEnv clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  PreviewEnv copyWith(void Function(PreviewEnv) updates) =>
      super.copyWith((message) => updates(message as PreviewEnv)) as PreviewEnv;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use PreviewEnv() / PreviewEnv.new instead')
  static PreviewEnv create() => PreviewEnv._();
  static $pb.GeneratedMessage $_createMessage() => PreviewEnv._();
  @$core.override
  PreviewEnv createEmptyInstance() => PreviewEnv._();
  @$core.pragma('dart2js:noInline')
  static PreviewEnv getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<PreviewEnv>(PreviewEnv.$_createMessage);
  static PreviewEnv? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get text => $_getSZ(0);
  @$pb.TagNumber(1)
  set text($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasText() => $_has(0);
  @$pb.TagNumber(1)
  void clearText() => $_clearField(1);
}

/// Adds what a pasted .env holds: one secret named `name` with every
/// variable, or with `one_per_variable` one secret per variable, named after
/// it. No proof needed.
class ImportEnv extends $pb.GeneratedMessage {
  factory ImportEnv({
    $core.String? text,
    $core.String? name,
    $core.String? description,
    $core.bool? onePerVariable,
  }) {
    final result = ImportEnv._();
    if (text != null) result.text = text;
    if (name != null) result.name = name;
    if (description != null) result.description = description;
    if (onePerVariable != null) result.onePerVariable = onePerVariable;
    return result;
  }

  ImportEnv._();

  factory ImportEnv.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ImportEnv()..mergeFromBuffer(data, registry);
  factory ImportEnv.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ImportEnv()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ImportEnv',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ImportEnv.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'text')
    ..aOS(2, _omitFieldNames ? '' : 'name')
    ..aOS(3, _omitFieldNames ? '' : 'description')
    ..aOB(4, _omitFieldNames ? '' : 'onePerVariable')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ImportEnv clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ImportEnv copyWith(void Function(ImportEnv) updates) =>
      super.copyWith((message) => updates(message as ImportEnv)) as ImportEnv;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ImportEnv() / ImportEnv.new instead')
  static ImportEnv create() => ImportEnv._();
  static $pb.GeneratedMessage $_createMessage() => ImportEnv._();
  @$core.override
  ImportEnv createEmptyInstance() => ImportEnv._();
  @$core.pragma('dart2js:noInline')
  static ImportEnv getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<ImportEnv>(ImportEnv.$_createMessage);
  static ImportEnv? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get text => $_getSZ(0);
  @$pb.TagNumber(1)
  set text($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasText() => $_has(0);
  @$pb.TagNumber(1)
  void clearText() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get name => $_getSZ(1);
  @$pb.TagNumber(2)
  set name($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasName() => $_has(1);
  @$pb.TagNumber(2)
  void clearName() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.String get description => $_getSZ(2);
  @$pb.TagNumber(3)
  set description($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasDescription() => $_has(2);
  @$pb.TagNumber(3)
  void clearDescription() => $_clearField(3);

  @$pb.TagNumber(4)
  $core.bool get onePerVariable => $_getBF(3);
  @$pb.TagNumber(4)
  set onePerVariable($core.bool value) => $_setBool(3, value);
  @$pb.TagNumber(4)
  $core.bool hasOnePerVariable() => $_has(3);
  @$pb.TagNumber(4)
  void clearOnePerVariable() => $_clearField(4);
}

/// Grants (approve) or dismisses an agent's secret request. To give one of
/// the vault's secrets, `secret_id` names it. To add a new one, `entries`
/// hold its variables and values, and `name` and `description` its name and
/// description (empty: the request's), and it's made and given. A request
/// from before agents described what they need, naming a secret, is given
/// that secret when neither is set. The agent is told only what it got.
class DecideSecretRequest extends $pb.GeneratedMessage {
  factory DecideSecretRequest({
    $core.String? agentId,
    $core.String? requestId,
    $core.bool? approve,
    VaultProof? proof,
    $core.Iterable<SecretEntry>? entries,
    $core.String? comment,
    $core.String? secretId,
    $core.String? name,
    $core.String? description,
  }) {
    final result = DecideSecretRequest._();
    if (agentId != null) result.agentId = agentId;
    if (requestId != null) result.requestId = requestId;
    if (approve != null) result.approve = approve;
    if (proof != null) result.proof = proof;
    if (entries != null) result.entries.addAll(entries);
    if (comment != null) result.comment = comment;
    if (secretId != null) result.secretId = secretId;
    if (name != null) result.name = name;
    if (description != null) result.description = description;
    return result;
  }

  DecideSecretRequest._();

  factory DecideSecretRequest.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DecideSecretRequest()..mergeFromBuffer(data, registry);
  factory DecideSecretRequest.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      DecideSecretRequest()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'DecideSecretRequest',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: DecideSecretRequest.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'agentId')
    ..aOS(2, _omitFieldNames ? '' : 'requestId')
    ..aOB(3, _omitFieldNames ? '' : 'approve')
    ..aOM<VaultProof>(4, _omitFieldNames ? '' : 'proof',
        subBuilder: VaultProof.$_createMessage)
    ..pPM<SecretEntry>(5, _omitFieldNames ? '' : 'entries',
        subBuilder: SecretEntry.$_createMessage)
    ..aOS(6, _omitFieldNames ? '' : 'comment')
    ..aOS(7, _omitFieldNames ? '' : 'secretId')
    ..aOS(8, _omitFieldNames ? '' : 'name')
    ..aOS(9, _omitFieldNames ? '' : 'description')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DecideSecretRequest clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  DecideSecretRequest copyWith(void Function(DecideSecretRequest) updates) =>
      super.copyWith((message) => updates(message as DecideSecretRequest))
          as DecideSecretRequest;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core
      .Deprecated('Use DecideSecretRequest() / DecideSecretRequest.new instead')
  static DecideSecretRequest create() => DecideSecretRequest._();
  static $pb.GeneratedMessage $_createMessage() => DecideSecretRequest._();
  @$core.override
  DecideSecretRequest createEmptyInstance() => DecideSecretRequest._();
  @$core.pragma('dart2js:noInline')
  static DecideSecretRequest getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<DecideSecretRequest>(
          DecideSecretRequest.$_createMessage);
  static DecideSecretRequest? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get agentId => $_getSZ(0);
  @$pb.TagNumber(1)
  set agentId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasAgentId() => $_has(0);
  @$pb.TagNumber(1)
  void clearAgentId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get requestId => $_getSZ(1);
  @$pb.TagNumber(2)
  set requestId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasRequestId() => $_has(1);
  @$pb.TagNumber(2)
  void clearRequestId() => $_clearField(2);

  @$pb.TagNumber(3)
  $core.bool get approve => $_getBF(2);
  @$pb.TagNumber(3)
  set approve($core.bool value) => $_setBool(2, value);
  @$pb.TagNumber(3)
  $core.bool hasApprove() => $_has(2);
  @$pb.TagNumber(3)
  void clearApprove() => $_clearField(3);

  @$pb.TagNumber(4)
  VaultProof get proof => $_getN(3);
  @$pb.TagNumber(4)
  set proof(VaultProof value) => $_setField(4, value);
  @$pb.TagNumber(4)
  $core.bool hasProof() => $_has(3);
  @$pb.TagNumber(4)
  void clearProof() => $_clearField(4);
  @$pb.TagNumber(4)
  VaultProof ensureProof() => $_ensure(3);

  @$pb.TagNumber(5)
  $pb.PbList<SecretEntry> get entries => $_getList(4);

  /// Told to the agent; may be empty.
  @$pb.TagNumber(6)
  $core.String get comment => $_getSZ(5);
  @$pb.TagNumber(6)
  set comment($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasComment() => $_has(5);
  @$pb.TagNumber(6)
  void clearComment() => $_clearField(6);

  /// An existing secret to give (SecretInfo.id).
  @$pb.TagNumber(7)
  $core.String get secretId => $_getSZ(6);
  @$pb.TagNumber(7)
  set secretId($core.String value) => $_setString(6, value);
  @$pb.TagNumber(7)
  $core.bool hasSecretId() => $_has(6);
  @$pb.TagNumber(7)
  void clearSecretId() => $_clearField(7);

  /// A new secret's name and description.
  @$pb.TagNumber(8)
  $core.String get name => $_getSZ(7);
  @$pb.TagNumber(8)
  set name($core.String value) => $_setString(7, value);
  @$pb.TagNumber(8)
  $core.bool hasName() => $_has(7);
  @$pb.TagNumber(8)
  void clearName() => $_clearField(8);

  @$pb.TagNumber(9)
  $core.String get description => $_getSZ(8);
  @$pb.TagNumber(9)
  set description($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasDescription() => $_has(8);
  @$pb.TagNumber(9)
  void clearDescription() => $_clearField(9);
}

/// Who used a secret and when, newest first; `id` empty for every secret.
class ListSecretUses extends $pb.GeneratedMessage {
  factory ListSecretUses({
    $core.String? id,
    $core.int? limit,
  }) {
    final result = ListSecretUses._();
    if (id != null) result.id = id;
    if (limit != null) result.limit = limit;
    return result;
  }

  ListSecretUses._();

  factory ListSecretUses.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ListSecretUses()..mergeFromBuffer(data, registry);
  factory ListSecretUses.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ListSecretUses()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ListSecretUses',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ListSecretUses.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aI(2, _omitFieldNames ? '' : 'limit', fieldType: $pb.PbFieldType.OU3)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ListSecretUses clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ListSecretUses copyWith(void Function(ListSecretUses) updates) =>
      super.copyWith((message) => updates(message as ListSecretUses))
          as ListSecretUses;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ListSecretUses() / ListSecretUses.new instead')
  static ListSecretUses create() => ListSecretUses._();
  static $pb.GeneratedMessage $_createMessage() => ListSecretUses._();
  @$core.override
  ListSecretUses createEmptyInstance() => ListSecretUses._();
  @$core.pragma('dart2js:noInline')
  static ListSecretUses getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ListSecretUses>(
          ListSecretUses.$_createMessage);
  static ListSecretUses? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// 0 for 100.
  @$pb.TagNumber(2)
  $core.int get limit => $_getIZ(1);
  @$pb.TagNumber(2)
  set limit($core.int value) => $_setUnsignedInt32(1, value);
  @$pb.TagNumber(2)
  $core.bool hasLimit() => $_has(1);
  @$pb.TagNumber(2)
  void clearLimit() => $_clearField(2);
}

/// For a vault with a key of the person's own: `old_passphrase` is the
/// passphrase or the recovery key. The new one has 8 characters at least.
/// Answered with VaultSession.
class ChangePassphrase extends $pb.GeneratedMessage {
  factory ChangePassphrase({
    $core.String? oldPassphrase,
    $core.String? newPassphrase,
  }) {
    final result = ChangePassphrase._();
    if (oldPassphrase != null) result.oldPassphrase = oldPassphrase;
    if (newPassphrase != null) result.newPassphrase = newPassphrase;
    return result;
  }

  ChangePassphrase._();

  factory ChangePassphrase.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ChangePassphrase()..mergeFromBuffer(data, registry);
  factory ChangePassphrase.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ChangePassphrase()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ChangePassphrase',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ChangePassphrase.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'oldPassphrase')
    ..aOS(2, _omitFieldNames ? '' : 'newPassphrase')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChangePassphrase clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ChangePassphrase copyWith(void Function(ChangePassphrase) updates) =>
      super.copyWith((message) => updates(message as ChangePassphrase))
          as ChangePassphrase;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ChangePassphrase() / ChangePassphrase.new instead')
  static ChangePassphrase create() => ChangePassphrase._();
  static $pb.GeneratedMessage $_createMessage() => ChangePassphrase._();
  @$core.override
  ChangePassphrase createEmptyInstance() => ChangePassphrase._();
  @$core.pragma('dart2js:noInline')
  static ChangePassphrase getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ChangePassphrase>(
          ChangePassphrase.$_createMessage);
  static ChangePassphrase? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get oldPassphrase => $_getSZ(0);
  @$pb.TagNumber(1)
  set oldPassphrase($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasOldPassphrase() => $_has(0);
  @$pb.TagNumber(1)
  void clearOldPassphrase() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get newPassphrase => $_getSZ(1);
  @$pb.TagNumber(2)
  set newPassphrase($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasNewPassphrase() => $_has(1);
  @$pb.TagNumber(2)
  void clearNewPassphrase() => $_clearField(2);
}

/// A recovery key, a key of the person's own that opens the vault too; it
/// replaces the last one, which stops working. Answered with RecoveryKey.
class NewRecoveryKey extends $pb.GeneratedMessage {
  factory NewRecoveryKey({
    VaultProof? proof,
  }) {
    final result = NewRecoveryKey._();
    if (proof != null) result.proof = proof;
    return result;
  }

  NewRecoveryKey._();

  factory NewRecoveryKey.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      NewRecoveryKey()..mergeFromBuffer(data, registry);
  factory NewRecoveryKey.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      NewRecoveryKey()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'NewRecoveryKey',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: NewRecoveryKey.$_createMessage)
    ..aOM<VaultProof>(1, _omitFieldNames ? '' : 'proof',
        subBuilder: VaultProof.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  NewRecoveryKey clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  NewRecoveryKey copyWith(void Function(NewRecoveryKey) updates) =>
      super.copyWith((message) => updates(message as NewRecoveryKey))
          as NewRecoveryKey;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use NewRecoveryKey() / NewRecoveryKey.new instead')
  static NewRecoveryKey create() => NewRecoveryKey._();
  static $pb.GeneratedMessage $_createMessage() => NewRecoveryKey._();
  @$core.override
  NewRecoveryKey createEmptyInstance() => NewRecoveryKey._();
  @$core.pragma('dart2js:noInline')
  static NewRecoveryKey getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<NewRecoveryKey>(
          NewRecoveryKey.$_createMessage);
  static NewRecoveryKey? _defaultInstance;

  @$pb.TagNumber(1)
  VaultProof get proof => $_getN(0);
  @$pb.TagNumber(1)
  set proof(VaultProof value) => $_setField(1, value);
  @$pb.TagNumber(1)
  $core.bool hasProof() => $_has(0);
  @$pb.TagNumber(1)
  void clearProof() => $_clearField(1);
  @$pb.TagNumber(1)
  VaultProof ensureProof() => $_ensure(0);
}

/// Turns locked mode on (the key kept on disk is deleted) or off (it's kept
/// again). On needs a key of the person's own (a passphrase or recovery
/// key) first. Answered with VaultSession.
class SetVaultLockedMode extends $pb.GeneratedMessage {
  factory SetVaultLockedMode({
    $core.bool? lockedMode,
    VaultProof? proof,
  }) {
    final result = SetVaultLockedMode._();
    if (lockedMode != null) result.lockedMode = lockedMode;
    if (proof != null) result.proof = proof;
    return result;
  }

  SetVaultLockedMode._();

  factory SetVaultLockedMode.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetVaultLockedMode()..mergeFromBuffer(data, registry);
  factory SetVaultLockedMode.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetVaultLockedMode()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SetVaultLockedMode',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SetVaultLockedMode.$_createMessage)
    ..aOB(1, _omitFieldNames ? '' : 'lockedMode')
    ..aOM<VaultProof>(2, _omitFieldNames ? '' : 'proof',
        subBuilder: VaultProof.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetVaultLockedMode clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetVaultLockedMode copyWith(void Function(SetVaultLockedMode) updates) =>
      super.copyWith((message) => updates(message as SetVaultLockedMode))
          as SetVaultLockedMode;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SetVaultLockedMode() / SetVaultLockedMode.new instead')
  static SetVaultLockedMode create() => SetVaultLockedMode._();
  static $pb.GeneratedMessage $_createMessage() => SetVaultLockedMode._();
  @$core.override
  SetVaultLockedMode createEmptyInstance() => SetVaultLockedMode._();
  @$core.pragma('dart2js:noInline')
  static SetVaultLockedMode getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<SetVaultLockedMode>(
          SetVaultLockedMode.$_createMessage);
  static SetVaultLockedMode? _defaultInstance;

  @$pb.TagNumber(1)
  $core.bool get lockedMode => $_getBF(0);
  @$pb.TagNumber(1)
  set lockedMode($core.bool value) => $_setBool(0, value);
  @$pb.TagNumber(1)
  $core.bool hasLockedMode() => $_has(0);
  @$pb.TagNumber(1)
  void clearLockedMode() => $_clearField(1);

  @$pb.TagNumber(2)
  VaultProof get proof => $_getN(1);
  @$pb.TagNumber(2)
  set proof(VaultProof value) => $_setField(2, value);
  @$pb.TagNumber(2)
  $core.bool hasProof() => $_has(1);
  @$pb.TagNumber(2)
  void clearProof() => $_clearField(2);
  @$pb.TagNumber(2)
  VaultProof ensureProof() => $_ensure(1);
}

/// Unused: always 0, and never sent unasked any more. Requests need no
/// VaultProof, so there's no unlocked window to show.
class VaultSession extends $pb.GeneratedMessage {
  factory VaultSession({
    $fixnum.Int64? unlockedUntilMs,
  }) {
    final result = VaultSession._();
    if (unlockedUntilMs != null) result.unlockedUntilMs = unlockedUntilMs;
    return result;
  }

  VaultSession._();

  factory VaultSession.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      VaultSession()..mergeFromBuffer(data, registry);
  factory VaultSession.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      VaultSession()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'VaultSession',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: VaultSession.$_createMessage)
    ..aInt64(1, _omitFieldNames ? '' : 'unlockedUntilMs')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  VaultSession clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  VaultSession copyWith(void Function(VaultSession) updates) =>
      super.copyWith((message) => updates(message as VaultSession))
          as VaultSession;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use VaultSession() / VaultSession.new instead')
  static VaultSession create() => VaultSession._();
  static $pb.GeneratedMessage $_createMessage() => VaultSession._();
  @$core.override
  VaultSession createEmptyInstance() => VaultSession._();
  @$core.pragma('dart2js:noInline')
  static VaultSession getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<VaultSession>(
          VaultSession.$_createMessage);
  static VaultSession? _defaultInstance;

  @$pb.TagNumber(1)
  $fixnum.Int64 get unlockedUntilMs => $_getI64(0);
  @$pb.TagNumber(1)
  set unlockedUntilMs($fixnum.Int64 value) => $_setInt64(0, value);
  @$pb.TagNumber(1)
  $core.bool hasUnlockedUntilMs() => $_has(0);
  @$pb.TagNumber(1)
  void clearUnlockedUntilMs() => $_clearField(1);
}

class SecretValue extends $pb.GeneratedMessage {
  factory SecretValue({
    $core.String? id,
    $core.Iterable<SecretEntry>? entries,
    $fixnum.Int64? unlockedUntilMs,
  }) {
    final result = SecretValue._();
    if (id != null) result.id = id;
    if (entries != null) result.entries.addAll(entries);
    if (unlockedUntilMs != null) result.unlockedUntilMs = unlockedUntilMs;
    return result;
  }

  SecretValue._();

  factory SecretValue.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretValue()..mergeFromBuffer(data, registry);
  factory SecretValue.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretValue()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SecretValue',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SecretValue.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..pPM<SecretEntry>(2, _omitFieldNames ? '' : 'entries',
        subBuilder: SecretEntry.$_createMessage)
    ..aInt64(3, _omitFieldNames ? '' : 'unlockedUntilMs')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretValue clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretValue copyWith(void Function(SecretValue) updates) =>
      super.copyWith((message) => updates(message as SecretValue))
          as SecretValue;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SecretValue() / SecretValue.new instead')
  static SecretValue create() => SecretValue._();
  static $pb.GeneratedMessage $_createMessage() => SecretValue._();
  @$core.override
  SecretValue createEmptyInstance() => SecretValue._();
  @$core.pragma('dart2js:noInline')
  static SecretValue getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<SecretValue>(
          SecretValue.$_createMessage);
  static SecretValue? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  $pb.PbList<SecretEntry> get entries => $_getList(1);

  /// Unused: always 0.
  @$pb.TagNumber(3)
  $fixnum.Int64 get unlockedUntilMs => $_getI64(2);
  @$pb.TagNumber(3)
  set unlockedUntilMs($fixnum.Int64 value) => $_setInt64(2, value);
  @$pb.TagNumber(3)
  $core.bool hasUnlockedUntilMs() => $_has(2);
  @$pb.TagNumber(3)
  void clearUnlockedUntilMs() => $_clearField(3);
}

class EnvPreview extends $pb.GeneratedMessage {
  factory EnvPreview({
    $core.Iterable<SecretVariable>? variables,
    $core.Iterable<$core.String>? warnings,
  }) {
    final result = EnvPreview._();
    if (variables != null) result.variables.addAll(variables);
    if (warnings != null) result.warnings.addAll(warnings);
    return result;
  }

  EnvPreview._();

  factory EnvPreview.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      EnvPreview()..mergeFromBuffer(data, registry);
  factory EnvPreview.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      EnvPreview()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'EnvPreview',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: EnvPreview.$_createMessage)
    ..pPM<SecretVariable>(1, _omitFieldNames ? '' : 'variables',
        subBuilder: SecretVariable.$_createMessage)
    ..pPS(2, _omitFieldNames ? '' : 'warnings')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  EnvPreview clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  EnvPreview copyWith(void Function(EnvPreview) updates) =>
      super.copyWith((message) => updates(message as EnvPreview)) as EnvPreview;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use EnvPreview() / EnvPreview.new instead')
  static EnvPreview create() => EnvPreview._();
  static $pb.GeneratedMessage $_createMessage() => EnvPreview._();
  @$core.override
  EnvPreview createEmptyInstance() => EnvPreview._();
  @$core.pragma('dart2js:noInline')
  static EnvPreview getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<EnvPreview>(EnvPreview.$_createMessage);
  static EnvPreview? _defaultInstance;

  @$pb.TagNumber(1)
  $pb.PbList<SecretVariable> get variables => $_getList(0);

  /// Duplicates, empty values, values too short to hide in agents' output,
  /// lines that couldn't be read (by line number, never with their text).
  @$pb.TagNumber(2)
  $pb.PbList<$core.String> get warnings => $_getList(1);
}

/// Shown once: the person keeps it, to open the vault when the passphrase is
/// forgotten or the key on disk is lost.
class RecoveryKey extends $pb.GeneratedMessage {
  factory RecoveryKey({
    $core.String? key,
    $fixnum.Int64? unlockedUntilMs,
  }) {
    final result = RecoveryKey._();
    if (key != null) result.key = key;
    if (unlockedUntilMs != null) result.unlockedUntilMs = unlockedUntilMs;
    return result;
  }

  RecoveryKey._();

  factory RecoveryKey.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      RecoveryKey()..mergeFromBuffer(data, registry);
  factory RecoveryKey.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      RecoveryKey()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'RecoveryKey',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: RecoveryKey.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'key')
    ..aInt64(2, _omitFieldNames ? '' : 'unlockedUntilMs')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  RecoveryKey clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  RecoveryKey copyWith(void Function(RecoveryKey) updates) =>
      super.copyWith((message) => updates(message as RecoveryKey))
          as RecoveryKey;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use RecoveryKey() / RecoveryKey.new instead')
  static RecoveryKey create() => RecoveryKey._();
  static $pb.GeneratedMessage $_createMessage() => RecoveryKey._();
  @$core.override
  RecoveryKey createEmptyInstance() => RecoveryKey._();
  @$core.pragma('dart2js:noInline')
  static RecoveryKey getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<RecoveryKey>(
          RecoveryKey.$_createMessage);
  static RecoveryKey? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get key => $_getSZ(0);
  @$pb.TagNumber(1)
  set key($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasKey() => $_has(0);
  @$pb.TagNumber(1)
  void clearKey() => $_clearField(1);

  /// Unused: always 0.
  @$pb.TagNumber(2)
  $fixnum.Int64 get unlockedUntilMs => $_getI64(1);
  @$pb.TagNumber(2)
  set unlockedUntilMs($fixnum.Int64 value) => $_setInt64(1, value);
  @$pb.TagNumber(2)
  $core.bool hasUnlockedUntilMs() => $_has(1);
  @$pb.TagNumber(2)
  void clearUnlockedUntilMs() => $_clearField(2);
}

class SecretUses extends $pb.GeneratedMessage {
  factory SecretUses({
    $core.Iterable<SecretUse>? uses,
  }) {
    final result = SecretUses._();
    if (uses != null) result.uses.addAll(uses);
    return result;
  }

  SecretUses._();

  factory SecretUses.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretUses()..mergeFromBuffer(data, registry);
  factory SecretUses.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretUses()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SecretUses',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SecretUses.$_createMessage)
    ..pPM<SecretUse>(1, _omitFieldNames ? '' : 'uses',
        subBuilder: SecretUse.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretUses clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretUses copyWith(void Function(SecretUses) updates) =>
      super.copyWith((message) => updates(message as SecretUses)) as SecretUses;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SecretUses() / SecretUses.new instead')
  static SecretUses create() => SecretUses._();
  static $pb.GeneratedMessage $_createMessage() => SecretUses._();
  @$core.override
  SecretUses createEmptyInstance() => SecretUses._();
  @$core.pragma('dart2js:noInline')
  static SecretUses getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<SecretUses>(SecretUses.$_createMessage);
  static SecretUses? _defaultInstance;

  @$pb.TagNumber(1)
  $pb.PbList<SecretUse> get uses => $_getList(0);
}

class SecretUse extends $pb.GeneratedMessage {
  factory SecretUse({
    $core.String? secretId,
    $core.String? secretName,
    $fixnum.Int64? timeMs,
    $core.String? kind,
    $core.String? agentId,
    $core.String? taskId,
    $core.String? detail,
  }) {
    final result = SecretUse._();
    if (secretId != null) result.secretId = secretId;
    if (secretName != null) result.secretName = secretName;
    if (timeMs != null) result.timeMs = timeMs;
    if (kind != null) result.kind = kind;
    if (agentId != null) result.agentId = agentId;
    if (taskId != null) result.taskId = taskId;
    if (detail != null) result.detail = detail;
    return result;
  }

  SecretUse._();

  factory SecretUse.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretUse()..mergeFromBuffer(data, registry);
  factory SecretUse.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretUse()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SecretUse',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SecretUse.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'secretId')
    ..aOS(2, _omitFieldNames ? '' : 'secretName')
    ..aInt64(3, _omitFieldNames ? '' : 'timeMs')
    ..aOS(4, _omitFieldNames ? '' : 'kind')
    ..aOS(5, _omitFieldNames ? '' : 'agentId')
    ..aOS(6, _omitFieldNames ? '' : 'taskId')
    ..aOS(7, _omitFieldNames ? '' : 'detail')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretUse clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretUse copyWith(void Function(SecretUse) updates) =>
      super.copyWith((message) => updates(message as SecretUse)) as SecretUse;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SecretUse() / SecretUse.new instead')
  static SecretUse create() => SecretUse._();
  static $pb.GeneratedMessage $_createMessage() => SecretUse._();
  @$core.override
  SecretUse createEmptyInstance() => SecretUse._();
  @$core.pragma('dart2js:noInline')
  static SecretUse getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<SecretUse>(SecretUse.$_createMessage);
  static SecretUse? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get secretId => $_getSZ(0);
  @$pb.TagNumber(1)
  set secretId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasSecretId() => $_has(0);
  @$pb.TagNumber(1)
  void clearSecretId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get secretName => $_getSZ(1);
  @$pb.TagNumber(2)
  set secretName($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasSecretName() => $_has(1);
  @$pb.TagNumber(2)
  void clearSecretName() => $_clearField(2);

  @$pb.TagNumber(3)
  $fixnum.Int64 get timeMs => $_getI64(2);
  @$pb.TagNumber(3)
  set timeMs($fixnum.Int64 value) => $_setInt64(2, value);
  @$pb.TagNumber(3)
  $core.bool hasTimeMs() => $_has(2);
  @$pb.TagNumber(3)
  void clearTimeMs() => $_clearField(3);

  /// "use" (an agent's command got it), "reveal" (the person saw it in the
  /// app), "grant", "revoke".
  @$pb.TagNumber(4)
  $core.String get kind => $_getSZ(3);
  @$pb.TagNumber(4)
  set kind($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasKind() => $_has(3);
  @$pb.TagNumber(4)
  void clearKind() => $_clearField(4);

  /// The agent; for reveal, grant and revoke, the one concerned, if any.
  @$pb.TagNumber(5)
  $core.String get agentId => $_getSZ(4);
  @$pb.TagNumber(5)
  set agentId($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasAgentId() => $_has(4);
  @$pb.TagNumber(5)
  void clearAgentId() => $_clearField(5);

  /// The background task that got it; empty for a command run in the
  /// foreground.
  @$pb.TagNumber(6)
  $core.String get taskId => $_getSZ(5);
  @$pb.TagNumber(6)
  set taskId($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasTaskId() => $_has(5);
  @$pb.TagNumber(6)
  void clearTaskId() => $_clearField(6);

  /// The command's first line (uses), the device (reveals), or for grants
  /// and revokes the admin agent's id that made them (empty: the person).
  @$pb.TagNumber(7)
  $core.String get detail => $_getSZ(6);
  @$pb.TagNumber(7)
  set detail($core.String value) => $_setString(6, value);
  @$pb.TagNumber(7)
  $core.bool hasDetail() => $_has(6);
  @$pb.TagNumber(7)
  void clearDetail() => $_clearField(7);
}

/// An agent's request (secrets_request), shown in its conversation after the
/// call, for the person to grant or dismiss with DecideSecretRequest. The
/// entry is sent again when it's decided. Agents don't know which secrets
/// the vault holds: they describe what they need in `new_secret` (a name,
/// what it's for and the variables they expect), and the person gives an
/// existing secret or adds one. Requests from before that name an existing
/// secret in `secret_id`.
class SecretRequest extends $pb.GeneratedMessage {
  factory SecretRequest({
    $core.String? id,
    $core.String? agentId,
    $core.String? secretId,
    $core.String? secretName,
    NewSecret? newSecret,
    $core.String? reason,
    SecretRequestState? state,
    $fixnum.Int64? decidedMs,
    $core.String? comment,
    $core.String? givenSecretId,
    $core.String? givenSecretName,
  }) {
    final result = SecretRequest._();
    if (id != null) result.id = id;
    if (agentId != null) result.agentId = agentId;
    if (secretId != null) result.secretId = secretId;
    if (secretName != null) result.secretName = secretName;
    if (newSecret != null) result.newSecret = newSecret;
    if (reason != null) result.reason = reason;
    if (state != null) result.state = state;
    if (decidedMs != null) result.decidedMs = decidedMs;
    if (comment != null) result.comment = comment;
    if (givenSecretId != null) result.givenSecretId = givenSecretId;
    if (givenSecretName != null) result.givenSecretName = givenSecretName;
    return result;
  }

  SecretRequest._();

  factory SecretRequest.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretRequest()..mergeFromBuffer(data, registry);
  factory SecretRequest.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SecretRequest()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SecretRequest',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SecretRequest.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aOS(2, _omitFieldNames ? '' : 'agentId')
    ..aOS(3, _omitFieldNames ? '' : 'secretId')
    ..aOS(4, _omitFieldNames ? '' : 'secretName')
    ..aOM<NewSecret>(5, _omitFieldNames ? '' : 'newSecret',
        subBuilder: NewSecret.$_createMessage)
    ..aOS(6, _omitFieldNames ? '' : 'reason')
    ..aE<SecretRequestState>(7, _omitFieldNames ? '' : 'state',
        enumValues: SecretRequestState.values)
    ..aInt64(8, _omitFieldNames ? '' : 'decidedMs')
    ..aOS(9, _omitFieldNames ? '' : 'comment')
    ..aOS(10, _omitFieldNames ? '' : 'givenSecretId')
    ..aOS(11, _omitFieldNames ? '' : 'givenSecretName')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretRequest clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SecretRequest copyWith(void Function(SecretRequest) updates) =>
      super.copyWith((message) => updates(message as SecretRequest))
          as SecretRequest;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SecretRequest() / SecretRequest.new instead')
  static SecretRequest create() => SecretRequest._();
  static $pb.GeneratedMessage $_createMessage() => SecretRequest._();
  @$core.override
  SecretRequest createEmptyInstance() => SecretRequest._();
  @$core.pragma('dart2js:noInline')
  static SecretRequest getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<SecretRequest>(
          SecretRequest.$_createMessage);
  static SecretRequest? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  /// The agent that asked.
  @$pb.TagNumber(2)
  $core.String get agentId => $_getSZ(1);
  @$pb.TagNumber(2)
  set agentId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasAgentId() => $_has(1);
  @$pb.TagNumber(2)
  void clearAgentId() => $_clearField(2);

  /// Only in requests from before agents described what they need: the
  /// existing secret it asked for. Empty otherwise.
  @$pb.TagNumber(3)
  $core.String get secretId => $_getSZ(2);
  @$pb.TagNumber(3)
  set secretId($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasSecretId() => $_has(2);
  @$pb.TagNumber(3)
  void clearSecretId() => $_clearField(3);

  /// What it asks for, in a word: the name it gave (new_secret.name), or the
  /// existing secret's.
  @$pb.TagNumber(4)
  $core.String get secretName => $_getSZ(3);
  @$pb.TagNumber(4)
  set secretName($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasSecretName() => $_has(3);
  @$pb.TagNumber(4)
  void clearSecretName() => $_clearField(4);

  /// What it needs: a name, what it's for (which service, what access) and
  /// the variables it expects (no values). Prefills a new secret, if the
  /// person adds one. Unset only in requests that name an existing secret.
  @$pb.TagNumber(5)
  NewSecret get newSecret => $_getN(4);
  @$pb.TagNumber(5)
  set newSecret(NewSecret value) => $_setField(5, value);
  @$pb.TagNumber(5)
  $core.bool hasNewSecret() => $_has(4);
  @$pb.TagNumber(5)
  void clearNewSecret() => $_clearField(5);
  @$pb.TagNumber(5)
  NewSecret ensureNewSecret() => $_ensure(4);

  /// Why it needs it, in the agent's words.
  @$pb.TagNumber(6)
  $core.String get reason => $_getSZ(5);
  @$pb.TagNumber(6)
  set reason($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasReason() => $_has(5);
  @$pb.TagNumber(6)
  void clearReason() => $_clearField(6);

  @$pb.TagNumber(7)
  SecretRequestState get state => $_getN(6);
  @$pb.TagNumber(7)
  set state(SecretRequestState value) => $_setField(7, value);
  @$pb.TagNumber(7)
  $core.bool hasState() => $_has(6);
  @$pb.TagNumber(7)
  void clearState() => $_clearField(7);

  /// When it was decided, in ms since 1970; 0 while pending.
  @$pb.TagNumber(8)
  $fixnum.Int64 get decidedMs => $_getI64(7);
  @$pb.TagNumber(8)
  set decidedMs($fixnum.Int64 value) => $_setInt64(7, value);
  @$pb.TagNumber(8)
  $core.bool hasDecidedMs() => $_has(7);
  @$pb.TagNumber(8)
  void clearDecidedMs() => $_clearField(8);

  /// The person's comment; may be empty.
  @$pb.TagNumber(9)
  $core.String get comment => $_getSZ(8);
  @$pb.TagNumber(9)
  set comment($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasComment() => $_has(8);
  @$pb.TagNumber(9)
  void clearComment() => $_clearField(9);

  /// GRANTED: the secret given, which may be another than the one described.
  @$pb.TagNumber(10)
  $core.String get givenSecretId => $_getSZ(9);
  @$pb.TagNumber(10)
  set givenSecretId($core.String value) => $_setString(9, value);
  @$pb.TagNumber(10)
  $core.bool hasGivenSecretId() => $_has(9);
  @$pb.TagNumber(10)
  void clearGivenSecretId() => $_clearField(10);

  @$pb.TagNumber(11)
  $core.String get givenSecretName => $_getSZ(10);
  @$pb.TagNumber(11)
  set givenSecretName($core.String value) => $_setString(10, value);
  @$pb.TagNumber(11)
  $core.bool hasGivenSecretName() => $_has(10);
  @$pb.TagNumber(11)
  void clearGivenSecretName() => $_clearField(11);
}

class NewSecret extends $pb.GeneratedMessage {
  factory NewSecret({
    $core.String? name,
    $core.String? description,
    $core.Iterable<SecretVariable>? variables,
  }) {
    final result = NewSecret._();
    if (name != null) result.name = name;
    if (description != null) result.description = description;
    if (variables != null) result.variables.addAll(variables);
    return result;
  }

  NewSecret._();

  factory NewSecret.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      NewSecret()..mergeFromBuffer(data, registry);
  factory NewSecret.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      NewSecret()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'NewSecret',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: NewSecret.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'name')
    ..aOS(2, _omitFieldNames ? '' : 'description')
    ..pPM<SecretVariable>(3, _omitFieldNames ? '' : 'variables',
        subBuilder: SecretVariable.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  NewSecret clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  NewSecret copyWith(void Function(NewSecret) updates) =>
      super.copyWith((message) => updates(message as NewSecret)) as NewSecret;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use NewSecret() / NewSecret.new instead')
  static NewSecret create() => NewSecret._();
  static $pb.GeneratedMessage $_createMessage() => NewSecret._();
  @$core.override
  NewSecret createEmptyInstance() => NewSecret._();
  @$core.pragma('dart2js:noInline')
  static NewSecret getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<NewSecret>(NewSecret.$_createMessage);
  static NewSecret? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get name => $_getSZ(0);
  @$pb.TagNumber(1)
  set name($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasName() => $_has(0);
  @$pb.TagNumber(1)
  void clearName() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get description => $_getSZ(1);
  @$pb.TagNumber(2)
  set description($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasDescription() => $_has(1);
  @$pb.TagNumber(2)
  void clearDescription() => $_clearField(2);

  @$pb.TagNumber(3)
  $pb.PbList<SecretVariable> get variables => $_getList(2);
}

/// This Claktory's account at claktory.com, or at the service in
/// config.json's `account.service`. Signing in is the backend's, not a
/// device's: one sign-in, and every device that connects shows it.
class ClaktoryAccount extends $pb.GeneratedMessage {
  factory ClaktoryAccount({
    ClaktoryAccountState? state,
    $core.String? email,
    $core.String? service,
    $core.String? name,
    $fixnum.Int64? lastContactMs,
    $core.String? problem,
    $fixnum.Int64? codeExpiresMs,
    $core.String? lastSignInJson,
    $core.String? lastCheckInJson,
    $fixnum.Int64? lastSignInMs,
    $fixnum.Int64? lastCheckInMs,
  }) {
    final result = ClaktoryAccount._();
    if (state != null) result.state = state;
    if (email != null) result.email = email;
    if (service != null) result.service = service;
    if (name != null) result.name = name;
    if (lastContactMs != null) result.lastContactMs = lastContactMs;
    if (problem != null) result.problem = problem;
    if (codeExpiresMs != null) result.codeExpiresMs = codeExpiresMs;
    if (lastSignInJson != null) result.lastSignInJson = lastSignInJson;
    if (lastCheckInJson != null) result.lastCheckInJson = lastCheckInJson;
    if (lastSignInMs != null) result.lastSignInMs = lastSignInMs;
    if (lastCheckInMs != null) result.lastCheckInMs = lastCheckInMs;
    return result;
  }

  ClaktoryAccount._();

  factory ClaktoryAccount.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ClaktoryAccount()..mergeFromBuffer(data, registry);
  factory ClaktoryAccount.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ClaktoryAccount()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ClaktoryAccount',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ClaktoryAccount.$_createMessage)
    ..aE<ClaktoryAccountState>(1, _omitFieldNames ? '' : 'state',
        enumValues: ClaktoryAccountState.values)
    ..aOS(2, _omitFieldNames ? '' : 'email')
    ..aOS(3, _omitFieldNames ? '' : 'service')
    ..aOS(4, _omitFieldNames ? '' : 'name')
    ..aInt64(5, _omitFieldNames ? '' : 'lastContactMs')
    ..aOS(6, _omitFieldNames ? '' : 'problem')
    ..aInt64(7, _omitFieldNames ? '' : 'codeExpiresMs')
    ..aOS(8, _omitFieldNames ? '' : 'lastSignInJson')
    ..aOS(9, _omitFieldNames ? '' : 'lastCheckInJson')
    ..aInt64(10, _omitFieldNames ? '' : 'lastSignInMs')
    ..aInt64(11, _omitFieldNames ? '' : 'lastCheckInMs')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ClaktoryAccount clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ClaktoryAccount copyWith(void Function(ClaktoryAccount) updates) =>
      super.copyWith((message) => updates(message as ClaktoryAccount))
          as ClaktoryAccount;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ClaktoryAccount() / ClaktoryAccount.new instead')
  static ClaktoryAccount create() => ClaktoryAccount._();
  static $pb.GeneratedMessage $_createMessage() => ClaktoryAccount._();
  @$core.override
  ClaktoryAccount createEmptyInstance() => ClaktoryAccount._();
  @$core.pragma('dart2js:noInline')
  static ClaktoryAccount getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ClaktoryAccount>(
          ClaktoryAccount.$_createMessage);
  static ClaktoryAccount? _defaultInstance;

  @$pb.TagNumber(1)
  ClaktoryAccountState get state => $_getN(0);
  @$pb.TagNumber(1)
  set state(ClaktoryAccountState value) => $_setField(1, value);
  @$pb.TagNumber(1)
  $core.bool hasState() => $_has(0);
  @$pb.TagNumber(1)
  void clearState() => $_clearField(1);

  /// The email it's signed in with, or that the code went to.
  @$pb.TagNumber(2)
  $core.String get email => $_getSZ(1);
  @$pb.TagNumber(2)
  set email($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasEmail() => $_has(1);
  @$pb.TagNumber(2)
  void clearEmail() => $_clearField(2);

  /// The service's address, such as "https://account.claktory.com".
  @$pb.TagNumber(3)
  $core.String get service => $_getSZ(2);
  @$pb.TagNumber(3)
  set service($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasService() => $_has(2);
  @$pb.TagNumber(3)
  void clearService() => $_clearField(3);

  /// This Claktory's name there, as sent at sign-in (the computer's name).
  @$pb.TagNumber(4)
  $core.String get name => $_getSZ(3);
  @$pb.TagNumber(4)
  set name($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasName() => $_has(3);
  @$pb.TagNumber(4)
  void clearName() => $_clearField(4);

  /// When the service last answered, in ms since 1970; 0 if it never has.
  @$pb.TagNumber(5)
  $fixnum.Int64 get lastContactMs => $_getI64(4);
  @$pb.TagNumber(5)
  set lastContactMs($fixnum.Int64 value) => $_setInt64(4, value);
  @$pb.TagNumber(5)
  $core.bool hasLastContactMs() => $_has(4);
  @$pb.TagNumber(5)
  void clearLastContactMs() => $_clearField(5);

  /// What went wrong with the last call to the service, in words for the
  /// person ("claktory.com can't be reached; Claktory tries again by
  /// itself"); empty when it worked.
  @$pb.TagNumber(6)
  $core.String get problem => $_getSZ(5);
  @$pb.TagNumber(6)
  set problem($core.String value) => $_setString(5, value);
  @$pb.TagNumber(6)
  $core.bool hasProblem() => $_has(5);
  @$pb.TagNumber(6)
  void clearProblem() => $_clearField(6);

  /// AWAITING_CODE: when the code stops working, in ms since 1970.
  @$pb.TagNumber(7)
  $fixnum.Int64 get codeExpiresMs => $_getI64(6);
  @$pb.TagNumber(7)
  set codeExpiresMs($fixnum.Int64 value) => $_setInt64(6, value);
  @$pb.TagNumber(7)
  $core.bool hasCodeExpiresMs() => $_has(6);
  @$pb.TagNumber(7)
  void clearCodeExpiresMs() => $_clearField(7);

  /// Exactly what this Claktory last sent the service, as JSON, for "What
  /// claktory.com receives": its sign-in (the request that emails the code)
  /// and its check-in. The machine token, signatures and codes travel beside
  /// them and aren't shown; empty before the first.
  @$pb.TagNumber(8)
  $core.String get lastSignInJson => $_getSZ(7);
  @$pb.TagNumber(8)
  set lastSignInJson($core.String value) => $_setString(7, value);
  @$pb.TagNumber(8)
  $core.bool hasLastSignInJson() => $_has(7);
  @$pb.TagNumber(8)
  void clearLastSignInJson() => $_clearField(8);

  @$pb.TagNumber(9)
  $core.String get lastCheckInJson => $_getSZ(8);
  @$pb.TagNumber(9)
  set lastCheckInJson($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasLastCheckInJson() => $_has(8);
  @$pb.TagNumber(9)
  void clearLastCheckInJson() => $_clearField(9);

  /// When each was sent, in ms since 1970; 0 before the first.
  @$pb.TagNumber(10)
  $fixnum.Int64 get lastSignInMs => $_getI64(9);
  @$pb.TagNumber(10)
  set lastSignInMs($fixnum.Int64 value) => $_setInt64(9, value);
  @$pb.TagNumber(10)
  $core.bool hasLastSignInMs() => $_has(9);
  @$pb.TagNumber(10)
  void clearLastSignInMs() => $_clearField(10);

  @$pb.TagNumber(11)
  $fixnum.Int64 get lastCheckInMs => $_getI64(10);
  @$pb.TagNumber(11)
  set lastCheckInMs($fixnum.Int64 value) => $_setInt64(10, value);
  @$pb.TagNumber(11)
  $core.bool hasLastCheckInMs() => $_has(10);
  @$pb.TagNumber(11)
  void clearLastCheckInMs() => $_clearField(11);
}

/// Emails a code to sign this Claktory in. Answered with Ack once it's sent;
/// ERROR_CODE_RATE_LIMITED or ERROR_CODE_UNREACHABLE when it can't be.
class AccountSignIn extends $pb.GeneratedMessage {
  factory AccountSignIn({
    $core.String? email,
  }) {
    final result = AccountSignIn._();
    if (email != null) result.email = email;
    return result;
  }

  AccountSignIn._();

  factory AccountSignIn.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AccountSignIn()..mergeFromBuffer(data, registry);
  factory AccountSignIn.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AccountSignIn()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'AccountSignIn',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: AccountSignIn.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'email')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AccountSignIn clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AccountSignIn copyWith(void Function(AccountSignIn) updates) =>
      super.copyWith((message) => updates(message as AccountSignIn))
          as AccountSignIn;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use AccountSignIn() / AccountSignIn.new instead')
  static AccountSignIn create() => AccountSignIn._();
  static $pb.GeneratedMessage $_createMessage() => AccountSignIn._();
  @$core.override
  AccountSignIn createEmptyInstance() => AccountSignIn._();
  @$core.pragma('dart2js:noInline')
  static AccountSignIn getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<AccountSignIn>(
          AccountSignIn.$_createMessage);
  static AccountSignIn? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get email => $_getSZ(0);
  @$pb.TagNumber(1)
  set email($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasEmail() => $_has(0);
  @$pb.TagNumber(1)
  void clearEmail() => $_clearField(1);
}

/// The emailed code. Answered with Ack once signed in, when the vault is
/// protected by the account (and its recovery key made). A wrong code is
/// ERROR_CODE_WRONG_PROOF, its message saying how many tries are left; an
/// expired one ERROR_CODE_INVALID_REQUEST.
class AccountVerify extends $pb.GeneratedMessage {
  factory AccountVerify({
    $core.String? code,
  }) {
    final result = AccountVerify._();
    if (code != null) result.code = code;
    return result;
  }

  AccountVerify._();

  factory AccountVerify.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AccountVerify()..mergeFromBuffer(data, registry);
  factory AccountVerify.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AccountVerify()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'AccountVerify',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: AccountVerify.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'code')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AccountVerify clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AccountVerify copyWith(void Function(AccountVerify) updates) =>
      super.copyWith((message) => updates(message as AccountVerify))
          as AccountVerify;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use AccountVerify() / AccountVerify.new instead')
  static AccountVerify create() => AccountVerify._();
  static $pb.GeneratedMessage $_createMessage() => AccountVerify._();
  @$core.override
  AccountVerify createEmptyInstance() => AccountVerify._();
  @$core.pragma('dart2js:noInline')
  static AccountVerify getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<AccountVerify>(
          AccountVerify.$_createMessage);
  static AccountVerify? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get code => $_getSZ(0);
  @$pb.TagNumber(1)
  set code($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasCode() => $_has(0);
  @$pb.TagNumber(1)
  void clearCode() => $_clearField(1);
}

/// Emails a new code to the same address. Answered with Ack.
class AccountResend extends $pb.GeneratedMessage {
  factory AccountResend() => AccountResend._();

  AccountResend._();

  factory AccountResend.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AccountResend()..mergeFromBuffer(data, registry);
  factory AccountResend.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AccountResend()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'AccountResend',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: AccountResend.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AccountResend clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AccountResend copyWith(void Function(AccountResend) updates) =>
      super.copyWith((message) => updates(message as AccountResend))
          as AccountResend;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use AccountResend() / AccountResend.new instead')
  static AccountResend create() => AccountResend._();
  static $pb.GeneratedMessage $_createMessage() => AccountResend._();
  @$core.override
  AccountResend createEmptyInstance() => AccountResend._();
  @$core.pragma('dart2js:noInline')
  static AccountResend getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<AccountResend>(
          AccountResend.$_createMessage);
  static AccountResend? _defaultInstance;
}

/// Signs out: no more check-ins, and back to SIGNED_OUT. The vault stays as
/// it is, unlocked while Claktory runs. Answered with Ack.
class AccountSignOut extends $pb.GeneratedMessage {
  factory AccountSignOut() => AccountSignOut._();

  AccountSignOut._();

  factory AccountSignOut.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AccountSignOut()..mergeFromBuffer(data, registry);
  factory AccountSignOut.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      AccountSignOut()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'AccountSignOut',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: AccountSignOut.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AccountSignOut clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  AccountSignOut copyWith(void Function(AccountSignOut) updates) =>
      super.copyWith((message) => updates(message as AccountSignOut))
          as AccountSignOut;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use AccountSignOut() / AccountSignOut.new instead')
  static AccountSignOut create() => AccountSignOut._();
  static $pb.GeneratedMessage $_createMessage() => AccountSignOut._();
  @$core.override
  AccountSignOut createEmptyInstance() => AccountSignOut._();
  @$core.pragma('dart2js:noInline')
  static AccountSignOut getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<AccountSignOut>(
          AccountSignOut.$_createMessage);
  static AccountSignOut? _defaultInstance;
}

/// The account service to use; empty for claktory.com's. Only while signed
/// out. Answered with Ack.
class SetAccountService extends $pb.GeneratedMessage {
  factory SetAccountService({
    $core.String? url,
  }) {
    final result = SetAccountService._();
    if (url != null) result.url = url;
    return result;
  }

  SetAccountService._();

  factory SetAccountService.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetAccountService()..mergeFromBuffer(data, registry);
  factory SetAccountService.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      SetAccountService()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'SetAccountService',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: SetAccountService.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'url')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetAccountService clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  SetAccountService copyWith(void Function(SetAccountService) updates) =>
      super.copyWith((message) => updates(message as SetAccountService))
          as SetAccountService;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use SetAccountService() / SetAccountService.new instead')
  static SetAccountService create() => SetAccountService._();
  static $pb.GeneratedMessage $_createMessage() => SetAccountService._();
  @$core.override
  SetAccountService createEmptyInstance() => SetAccountService._();
  @$core.pragma('dart2js:noInline')
  static SetAccountService getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<SetAccountService>(
          SetAccountService.$_createMessage);
  static SetAccountService? _defaultInstance;

  @$pb.TagNumber(1)
  $core.String get url => $_getSZ(0);
  @$pb.TagNumber(1)
  set url($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasUrl() => $_has(0);
  @$pb.TagNumber(1)
  void clearUrl() => $_clearField(1);
}

/// The vault's recovery key, to note down. Answered with RecoveryKey, to the
/// asker only; logged in the vault's uses.
class ShowRecoveryKey extends $pb.GeneratedMessage {
  factory ShowRecoveryKey() => ShowRecoveryKey._();

  ShowRecoveryKey._();

  factory ShowRecoveryKey.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ShowRecoveryKey()..mergeFromBuffer(data, registry);
  factory ShowRecoveryKey.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ShowRecoveryKey()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ShowRecoveryKey',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ShowRecoveryKey.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ShowRecoveryKey clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ShowRecoveryKey copyWith(void Function(ShowRecoveryKey) updates) =>
      super.copyWith((message) => updates(message as ShowRecoveryKey))
          as ShowRecoveryKey;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ShowRecoveryKey() / ShowRecoveryKey.new instead')
  static ShowRecoveryKey create() => ShowRecoveryKey._();
  static $pb.GeneratedMessage $_createMessage() => ShowRecoveryKey._();
  @$core.override
  ShowRecoveryKey createEmptyInstance() => ShowRecoveryKey._();
  @$core.pragma('dart2js:noInline')
  static ShowRecoveryKey getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ShowRecoveryKey>(
          ShowRecoveryKey.$_createMessage);
  static ShowRecoveryKey? _defaultInstance;
}

/// How this Claktory is arranged: where agents are, whom they can message,
/// which secrets they have, what data they read and write, and their
/// extensions that grant reach or power. Computed by the backend from the
/// same rules agents follow, so the app only lays it out. Nodes are sorted by
/// id and edges by from_id, to_id and kind, so the same Claktory gives the
/// same map.
class ClaktoryMap extends $pb.GeneratedMessage {
  factory ClaktoryMap({
    $core.Iterable<MapNode>? nodes,
    $core.Iterable<MapEdge>? edges,
  }) {
    final result = ClaktoryMap._();
    if (nodes != null) result.nodes.addAll(nodes);
    if (edges != null) result.edges.addAll(edges);
    return result;
  }

  ClaktoryMap._();

  factory ClaktoryMap.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ClaktoryMap()..mergeFromBuffer(data, registry);
  factory ClaktoryMap.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      ClaktoryMap()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'ClaktoryMap',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: ClaktoryMap.$_createMessage)
    ..pPM<MapNode>(1, _omitFieldNames ? '' : 'nodes',
        subBuilder: MapNode.$_createMessage)
    ..pPM<MapEdge>(2, _omitFieldNames ? '' : 'edges',
        subBuilder: MapEdge.$_createMessage)
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ClaktoryMap clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  ClaktoryMap copyWith(void Function(ClaktoryMap) updates) =>
      super.copyWith((message) => updates(message as ClaktoryMap))
          as ClaktoryMap;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use ClaktoryMap() / ClaktoryMap.new instead')
  static ClaktoryMap create() => ClaktoryMap._();
  static $pb.GeneratedMessage $_createMessage() => ClaktoryMap._();
  @$core.override
  ClaktoryMap createEmptyInstance() => ClaktoryMap._();
  @$core.pragma('dart2js:noInline')
  static ClaktoryMap getDefault() =>
      _defaultInstance ??= $pb.GeneratedMessage.$_defaultFor<ClaktoryMap>(
          ClaktoryMap.$_createMessage);
  static ClaktoryMap? _defaultInstance;

  @$pb.TagNumber(1)
  $pb.PbList<MapNode> get nodes => $_getList(0);

  @$pb.TagNumber(2)
  $pb.PbList<MapEdge> get edges => $_getList(1);
}

class MapNode extends $pb.GeneratedMessage {
  factory MapNode({
    $core.String? id,
    MapNodeKind? kind,
    $core.String? label,
    $core.String? detail,
    $core.String? parentId,
    $core.bool? admin,
    $core.String? color,
    $core.Iterable<MapBadge>? badges,
    $core.String? refId,
    $core.String? projectId,
  }) {
    final result = MapNode._();
    if (id != null) result.id = id;
    if (kind != null) result.kind = kind;
    if (label != null) result.label = label;
    if (detail != null) result.detail = detail;
    if (parentId != null) result.parentId = parentId;
    if (admin != null) result.admin = admin;
    if (color != null) result.color = color;
    if (badges != null) result.badges.addAll(badges);
    if (refId != null) result.refId = refId;
    if (projectId != null) result.projectId = projectId;
    return result;
  }

  MapNode._();

  factory MapNode.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MapNode()..mergeFromBuffer(data, registry);
  factory MapNode.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MapNode()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'MapNode',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: MapNode.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'id')
    ..aE<MapNodeKind>(2, _omitFieldNames ? '' : 'kind',
        enumValues: MapNodeKind.values)
    ..aOS(3, _omitFieldNames ? '' : 'label')
    ..aOS(4, _omitFieldNames ? '' : 'detail')
    ..aOS(5, _omitFieldNames ? '' : 'parentId')
    ..aOB(6, _omitFieldNames ? '' : 'admin')
    ..aOS(7, _omitFieldNames ? '' : 'color')
    ..pPM<MapBadge>(8, _omitFieldNames ? '' : 'badges',
        subBuilder: MapBadge.$_createMessage)
    ..aOS(9, _omitFieldNames ? '' : 'refId')
    ..aOS(10, _omitFieldNames ? '' : 'projectId')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MapNode clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MapNode copyWith(void Function(MapNode) updates) =>
      super.copyWith((message) => updates(message as MapNode)) as MapNode;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use MapNode() / MapNode.new instead')
  static MapNode create() => MapNode._();
  static $pb.GeneratedMessage $_createMessage() => MapNode._();
  @$core.override
  MapNode createEmptyInstance() => MapNode._();
  @$core.pragma('dart2js:noInline')
  static MapNode getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<MapNode>(MapNode.$_createMessage);
  static MapNode? _defaultInstance;

  /// Unique in the map and the same every time: "workspace",
  /// "project:<project id>", "folder:<project id>/<folder id>",
  /// "agent:<agent id>", "secret:<secret id>", "data:records" (Claktory's
  /// records) and "data:agent:<agent id>" (an agent's own database).
  @$pb.TagNumber(1)
  $core.String get id => $_getSZ(0);
  @$pb.TagNumber(1)
  set id($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasId() => $_has(0);
  @$pb.TagNumber(1)
  void clearId() => $_clearField(1);

  @$pb.TagNumber(2)
  MapNodeKind get kind => $_getN(1);
  @$pb.TagNumber(2)
  set kind(MapNodeKind value) => $_setField(2, value);
  @$pb.TagNumber(2)
  $core.bool hasKind() => $_has(1);
  @$pb.TagNumber(2)
  void clearKind() => $_clearField(2);

  /// A name to show: the agent's, project's, folder's or secret's;
  /// "Workspace", "Claktory's records", or "<agent name>'s database".
  @$pb.TagNumber(3)
  $core.String get label => $_getSZ(2);
  @$pb.TagNumber(3)
  set label($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasLabel() => $_has(2);
  @$pb.TagNumber(3)
  void clearLabel() => $_clearField(3);

  /// A line about it: an agent's role, a project's, folder's or database's
  /// path, a secret's description; may be empty.
  @$pb.TagNumber(4)
  $core.String get detail => $_getSZ(3);
  @$pb.TagNumber(4)
  set detail($core.String value) => $_setString(3, value);
  @$pb.TagNumber(4)
  $core.bool hasDetail() => $_has(3);
  @$pb.TagNumber(4)
  void clearDetail() => $_clearField(4);

  /// The node it sits under, as in the sidebar: a folder under its project,
  /// an agent under the agent it's nested under, else its folder, else its
  /// project, else the workspace; an agent's database under the agent.
  /// Empty at the top: the workspace, projects, admin agents (wherever they
  /// are), secrets and Claktory's records.
  @$pb.TagNumber(5)
  $core.String get parentId => $_getSZ(4);
  @$pb.TagNumber(5)
  set parentId($core.String value) => $_setString(4, value);
  @$pb.TagNumber(5)
  $core.bool hasParentId() => $_has(4);
  @$pb.TagNumber(5)
  void clearParentId() => $_clearField(5);

  /// An admin agent: sees and messages every agent.
  @$pb.TagNumber(6)
  $core.bool get admin => $_getBF(5);
  @$pb.TagNumber(6)
  set admin($core.bool value) => $_setBool(5, value);
  @$pb.TagNumber(6)
  $core.bool hasAdmin() => $_has(5);
  @$pb.TagNumber(6)
  void clearAdmin() => $_clearField(6);

  /// An agent's color (Agent.color); empty for other nodes.
  @$pb.TagNumber(7)
  $core.String get color => $_getSZ(6);
  @$pb.TagNumber(7)
  set color($core.String value) => $_setString(6, value);
  @$pb.TagNumber(7)
  $core.bool hasColor() => $_has(6);
  @$pb.TagNumber(7)
  void clearColor() => $_clearField(7);

  /// An agent's extensions that grant reach or power, in a fixed order.
  @$pb.TagNumber(8)
  $pb.PbList<MapBadge> get badges => $_getList(7);

  /// The id of what it stands for, to open it: the agent's, project's,
  /// folder's (within project_id) or secret's id; for an agent's database,
  /// the agent's. Empty for the workspace and Claktory's records.
  @$pb.TagNumber(9)
  $core.String get refId => $_getSZ(8);
  @$pb.TagNumber(9)
  set refId($core.String value) => $_setString(8, value);
  @$pb.TagNumber(9)
  $core.bool hasRefId() => $_has(8);
  @$pb.TagNumber(9)
  void clearRefId() => $_clearField(9);

  /// The project a folder or an agent (or its database) is in; empty
  /// otherwise.
  @$pb.TagNumber(10)
  $core.String get projectId => $_getSZ(9);
  @$pb.TagNumber(10)
  set projectId($core.String value) => $_setString(9, value);
  @$pb.TagNumber(10)
  $core.bool hasProjectId() => $_has(9);
  @$pb.TagNumber(10)
  void clearProjectId() => $_clearField(10);
}

/// An extension an agent has on that grants reach or power.
class MapBadge extends $pb.GeneratedMessage {
  factory MapBadge({
    $core.String? key,
    $core.String? label,
    $core.String? icon,
  }) {
    final result = MapBadge._();
    if (key != null) result.key = key;
    if (label != null) result.label = label;
    if (icon != null) result.icon = icon;
    return result;
  }

  MapBadge._();

  factory MapBadge.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MapBadge()..mergeFromBuffer(data, registry);
  factory MapBadge.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MapBadge()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'MapBadge',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: MapBadge.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'key')
    ..aOS(2, _omitFieldNames ? '' : 'label')
    ..aOS(3, _omitFieldNames ? '' : 'icon')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MapBadge clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MapBadge copyWith(void Function(MapBadge) updates) =>
      super.copyWith((message) => updates(message as MapBadge)) as MapBadge;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use MapBadge() / MapBadge.new instead')
  static MapBadge create() => MapBadge._();
  static $pb.GeneratedMessage $_createMessage() => MapBadge._();
  @$core.override
  MapBadge createEmptyInstance() => MapBadge._();
  @$core.pragma('dart2js:noInline')
  static MapBadge getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<MapBadge>(MapBadge.$_createMessage);
  static MapBadge? _defaultInstance;

  /// Stable: "agents:admin" (sees and messages every agent), "secrets:give"
  /// (gives secrets to agents), "tasks:see_all" (sees and stops every
  /// agent's tasks), "web", "tree" (own checkout: lands on the main branch),
  /// "database", "database:factory" (Claktory's records), "memory:factory"
  /// (the Claktory manual: can change Claktory's settings).
  @$pb.TagNumber(1)
  $core.String get key => $_getSZ(0);
  @$pb.TagNumber(1)
  set key($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasKey() => $_has(0);
  @$pb.TagNumber(1)
  void clearKey() => $_clearField(1);

  /// To show, e.g. "Web".
  @$pb.TagNumber(2)
  $core.String get label => $_getSZ(1);
  @$pb.TagNumber(2)
  set label($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasLabel() => $_has(1);
  @$pb.TagNumber(2)
  void clearLabel() => $_clearField(2);

  /// An icon name, as Extension.icon.
  @$pb.TagNumber(3)
  $core.String get icon => $_getSZ(2);
  @$pb.TagNumber(3)
  set icon($core.String value) => $_setString(2, value);
  @$pb.TagNumber(3)
  $core.bool hasIcon() => $_has(2);
  @$pb.TagNumber(3)
  void clearIcon() => $_clearField(3);
}

class MapEdge extends $pb.GeneratedMessage {
  factory MapEdge({
    $core.String? fromId,
    $core.String? toId,
    MapEdgeKind? kind,
    $core.bool? bothWays,
  }) {
    final result = MapEdge._();
    if (fromId != null) result.fromId = fromId;
    if (toId != null) result.toId = toId;
    if (kind != null) result.kind = kind;
    if (bothWays != null) result.bothWays = bothWays;
    return result;
  }

  MapEdge._();

  factory MapEdge.fromBuffer($core.List<$core.int> data,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MapEdge()..mergeFromBuffer(data, registry);
  factory MapEdge.fromJson($core.String json,
          [$pb.ExtensionRegistry registry = $pb.ExtensionRegistry.EMPTY]) =>
      MapEdge()..mergeFromJson(json, registry);

  static final $pb.BuilderInfo _i = $pb.BuilderInfo(
      _omitMessageNames ? '' : 'MapEdge',
      package: const $pb.PackageName(_omitMessageNames ? '' : 'factory.v1'),
      createEmptyInstance: MapEdge.$_createMessage)
    ..aOS(1, _omitFieldNames ? '' : 'fromId')
    ..aOS(2, _omitFieldNames ? '' : 'toId')
    ..aE<MapEdgeKind>(3, _omitFieldNames ? '' : 'kind',
        enumValues: MapEdgeKind.values)
    ..aOB(4, _omitFieldNames ? '' : 'bothWays')
    ..hasRequiredFields = false;

  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MapEdge clone() => deepCopy();
  @$core.Deprecated('See https://github.com/google/protobuf.dart/issues/998.')
  MapEdge copyWith(void Function(MapEdge) updates) =>
      super.copyWith((message) => updates(message as MapEdge)) as MapEdge;

  @$core.override
  $pb.BuilderInfo get info_ => _i;

  @$core.pragma('dart2js:noInline')
  @$core.Deprecated('Use MapEdge() / MapEdge.new instead')
  static MapEdge create() => MapEdge._();
  static $pb.GeneratedMessage $_createMessage() => MapEdge._();
  @$core.override
  MapEdge createEmptyInstance() => MapEdge._();
  @$core.pragma('dart2js:noInline')
  static MapEdge getDefault() => _defaultInstance ??=
      $pb.GeneratedMessage.$_defaultFor<MapEdge>(MapEdge.$_createMessage);
  static MapEdge? _defaultInstance;

  /// Node ids.
  @$pb.TagNumber(1)
  $core.String get fromId => $_getSZ(0);
  @$pb.TagNumber(1)
  set fromId($core.String value) => $_setString(0, value);
  @$pb.TagNumber(1)
  $core.bool hasFromId() => $_has(0);
  @$pb.TagNumber(1)
  void clearFromId() => $_clearField(1);

  @$pb.TagNumber(2)
  $core.String get toId => $_getSZ(1);
  @$pb.TagNumber(2)
  set toId($core.String value) => $_setString(1, value);
  @$pb.TagNumber(2)
  $core.bool hasToId() => $_has(1);
  @$pb.TagNumber(2)
  void clearToId() => $_clearField(2);

  @$pb.TagNumber(3)
  MapEdgeKind get kind => $_getN(2);
  @$pb.TagNumber(3)
  set kind(MapEdgeKind value) => $_setField(3, value);
  @$pb.TagNumber(3)
  $core.bool hasKind() => $_has(2);
  @$pb.TagNumber(3)
  void clearKind() => $_clearField(3);

  /// MESSAGES only: each can message the other. Such a pair is one edge,
  /// from the lower id.
  @$pb.TagNumber(4)
  $core.bool get bothWays => $_getBF(3);
  @$pb.TagNumber(4)
  set bothWays($core.bool value) => $_setBool(3, value);
  @$pb.TagNumber(4)
  $core.bool hasBothWays() => $_has(3);
  @$pb.TagNumber(4)
  void clearBothWays() => $_clearField(4);
}

const $core.bool _omitFieldNames =
    $core.bool.fromEnvironment('protobuf.omit_field_names');
const $core.bool _omitMessageNames =
    $core.bool.fromEnvironment('protobuf.omit_message_names');
