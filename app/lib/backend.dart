import 'dart:async';
import 'dart:developer' as developer;
import 'dart:io';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:protobuf/protobuf.dart';

import 'android/background_connection.dart';
import 'annotations.dart' show ThreadStore;
import 'app_views/transport.dart';
import 'app_update.dart';
import 'attachments.dart';
import 'background_grace.dart';
import 'connections.dart';
import 'loopback_forward.dart';
import 'mobile/share_browser.dart' show inAppShareBrowser;
import 'diagnostics_record.dart';
import 'device_account.dart'
    show AccountServiceError, AccountServiceUnreachable, DeviceAccount;
import 'device_kind.dart'
    show thisDeviceKindForHello, thisDeviceNameForHello, thisPlatformForHello;
import 'protocol/factory/v1/factory.pb.dart';
import 'highlights.dart';
import 'network.dart';
import 'outbox.dart';
import 'viewers/file_cache.dart' show FileCache;
import 'voice_keep.dart'
    show KeptKind, KeptVoices, forgetKeptVoices, keptClaktoryKey;
import 'paced.dart';
import 'pairing.dart';
import 'state.dart';
import 'tunnel_client.dart';
import 'user_input.dart';
import 'waiting.dart' show EntryReveal;
import 'windows_local.dart';
import 'workspace_patch.dart';

/// Must match `PROTOCOL_VERSION` in the backend's factory-protocol crate.
const _protocolVersion = 1;

/// This app's release version, told to the backend in Hello ("v0.1.16";
/// "dev" for a build the release scripts didn't stamp), as
/// [AppBuild.current] has it.
const _clientVersion = String.fromEnvironment(
  'FACTORY_VERSION',
  defaultValue: 'dev',
);

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
  // Windows reaches its own with its app-token instead ([WindowsLocal]): no
  // code is ever read or shown there (windows.md, authority B).
  if (Platform.isWindows) return null;
  final home = Platform.environment['HOME'] ?? '';
  const folder = bool.fromEnvironment('FACTORY_DEV')
      ? '.factory-dev'
      : '.factory';
  try {
    final code = await File('$home/$folder/remote/local-pairing')
        .readAsString();
    return parsePairingCode(code);
  } on IOException {
    // Moved to another computer: the copy left here keeps its code with
    // the rest of what it fenced off, and takes its token, on the open
    // port only (its tunnel's key went with the move), to delete the copy.
    try {
      final code = await File('$home/$folder/moved/remote/local-pairing')
          .readAsString();
      final moved = parsePairingCode(code);
      if (moved == null) return null;
      return RemoteFactory(
        code: moved.code,
        host: moved.host,
        port: moved.port,
        token: moved.token,
        fingerprint: moved.fingerprint,
        name: moved.name,
      );
    } on IOException {
      return null;
    }
  }
}

const _minRetryDelay = Duration(milliseconds: 250);
const _maxRetryDelay = Duration(seconds: 5);

/// The longest a Claktory that isn't on screen waits between tries while
/// it can't connect ([Backend.onScreen], #454): each try at one that isn't
/// there costs a burst of packets, ~80 KB a minute at [_maxRetryDelay].
const _awayRetryMost = Duration(minutes: 15);

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

/// What the backend says to an answer for a request its agent has changed
/// since the person saw it (#428): the request shows its new version, and
/// this, for them to look at it again.
const cardChangedRefusal = 'This request changed. Look at it again.';

/// The same refusal in the words from before the Requests rename (#588),
/// from a Claktory that hasn't updated yet. Matched too, so the app can
/// ship a release before the backend changes its words.
const _cardChangedBefore = 'This card changed. Look at it again.';

/// Whether [failure] is that refusal: shown as it is, never as "Couldn't
/// send".
bool cardChanged(RequestFailure failure) =>
    failure.message == cardChangedRefusal ||
    failure.message == _cardChangedBefore;

/// A try given up on, to try again after the backoff: it said why.
class _NotNow implements Exception {
  const _NotNow();
}

/// A try abandoned for a connection made from scratch
/// ([Backend.reconnectFromScratch]): the next starts at once.
class _Abandoned implements Exception {
  const _Abandoned();
}

/// Keeps one connection to a backend open, reconnecting with backoff when it
/// drops, and routes what the backend sends into the stores. The app keeps
/// one for each Claktory it's connected to, all live at once (rail.dart). Speaks the
/// protocol in proto/factory/v1/factory.proto.
/// The websites pipes' peers share that a device showing a Claktory on
/// another machine forwards, each id with its port there ([Backend]'s
/// `_keepShares`): those opened there (listening), live and not hidden; on
/// a phone or tablet ([touch]) only those it forwards already ([forwarded])
/// or whose page is open over the app now ([shown]: on an iPhone or iPad,
/// forwarded again once its tunnel was made afresh, #505).
@visibleForTesting
Map<String, int> sharesToForward(
  Workspace workspace, {
  required bool touch,
  required bool Function(String id) forwarded,
  required bool Function(String id) shown,
}) {
  final wanted = <String, int>{};
  for (final share in workspace.pipeShares) {
    if (share.mine || !share.live || share.hidden || !share.listening) {
      continue;
    }
    if (touch && !forwarded(share.id) && !shown(share.id)) continue;
    final port = Uri.tryParse(share.address)?.port ?? 0;
    if (port > 0) wanted[share.id] = port;
  }
  return wanted;
}

class Backend implements AppViewTransport {
  Backend({
    required this.workspace,
    required this.transcript,
    required this.explorer,
    required this.extensionView,
    QueuedStore? queued,
    Tunnels? tunnels,
    TunnelRuntime? tunnelRuntime,
    DiagnosticsRecorder? diagnostics,
    this.localBackend = true,
    Future<RemoteFactory?> Function()? localPairing,
    @visibleForTesting this.localEndpoint = _localEndpoint,
    @visibleForTesting WindowsLocal? windowsLocal,
  }) : queued = queued ?? QueuedStore(),
       _windowsLocal =
           windowsLocal ?? (Platform.isWindows ? const WindowsLocal() : null),
       _tunnels = tunnels ?? RustTunnels(),
       _runtime =
           tunnelRuntime ??
           (tunnels == null
               ? const RustTunnelRuntime()
               : const AnsweringRuntime()),
       _localPairing = localPairing ?? readLocalPairing,
       diagnostics =
           diagnostics ??
           (tunnels == null ? DiagnosticsRecorder.onDevice() : null),
       // Made now, so it sees every snapshot from the first.
       highlights = HighlightStore(
         workspace: workspace,
         extensionView: extensionView,
       ) {
    // Before the app first leaves the screen, so its grace is heard.
    BackgroundGrace.watch();
    tunnel.addListener(_keepFailure);
    reconnecting.addListener(_keepFailure);
  }

  /// The failure it last said it can't connect for, kept through the tries
  /// after it until one connects (Zucchero's row 475, #37): each try's
  /// tunnel starts its status afresh, and a reconnect made from scratch is
  /// quiet, so what it knows would show for a moment every half minute and
  /// "Reconnecting…" the rest of the time. Null while connected, before
  /// it first said so, and once a reason to try afresh makes it stale (back
  /// from the background, the network moved, the person asked).
  final lastFailure = ValueNotifier<TunnelStatus?>(null);

  /// [lastFailure] as it was when a reason to try afresh set it aside:
  /// once the quiet tries after it fail too and the tunnel has no failure
  /// of its own yet, it shows again at once, rather than "Connecting…"
  /// until the tunnel's own dial gives up, half a minute and more (Bruno's
  /// row 480). Gone once connected, or connecting elsewhere.
  TunnelStatus? _setAside;

  /// Sets [lastFailure] aside: a reason to try afresh.
  void _freshFailure() {
    _setAside = lastFailure.value ?? _setAside;
    lastFailure.value = null;
  }

  void _keepFailure() {
    if (_target == null || connected.value || reconnecting.value) return;
    final status = tunnel.value;
    if (status == null || status.failure == TunnelFailure.none) {
      final before = _setAside;
      if (before == null || lastFailure.value != null) return;
      lastFailure.value = before;
      _setAside = null;
      return;
    }
    _setAside = null;
    lastFailure.value = status;
  }

  final WorkspaceStore workspace;
  final TranscriptStore transcript;

  /// What happens to the connection, recorded on this device while the
  /// Claktory it shows has Advanced diagnostics on, and sent to it once
  /// connected (#214); null where the device can't keep them.
  final DiagnosticsRecorder? diagnostics;
  final ExplorerStore explorer;
  final ExtensionViewStore extensionView;
  final QueuedStore queued;

  /// Files waiting in the agents' message boxes, on this device.
  final attachments = AttachmentDrafts();

  /// The person's messages on their way, shown in the conversation at once.
  late final outbox = Outbox(
    BackendOutbox(this),
    connected: connected,
    claktory: () => keptClaktoryKey(target),
    kind: KeptKind.agent,
  );

  /// What glows as state changes, and what's said beside it.
  final HighlightStore highlights;

  /// Comments on files, by path, as the backend last sent them
  /// (annotations.dart).
  final threads = ThreadStore();

  /// Where the next conversation shown opens, when it's not at its end:
  /// at what waits for the person (waiting.dart).
  final reveal = EntryReveal();

  /// The selected pipe's messages (Workspace.selected_pipe_id).
  final pipeChat = PipeChat();

  /// The selected chat's conversation (Workspace.selected_chat_id), as an
  /// agent's is in [transcript]; what waits for it, and the person's
  /// messages on their way to it. Chat ids never meet agents' ids.
  final chatTranscript = ChatTranscriptStore();
  final chatQueued = QueuedStore();
  late final chatOutbox = Outbox(
    _ChatOutbox(this),
    connected: connected,
    claktory: () => keptClaktoryKey(target),
    kind: KeptKind.chat,
  );

  /// The conversation focus mode's Context follows without selecting it
  /// ([watchTranscript], #514), apart from the selected one: an agent's or
  /// a chat's entries and what waits for it, or a pipe's messages.
  final watchedTranscript = TranscriptStore();
  final watchedQueued = QueuedStore();
  final watchedPipe = PipeChat();
  WatchTranscript? _watch;

  /// Opens a screen an agent asked to show (its show_screen tool) in the
  /// window showing this Claktory, answering whether it did
  /// (screens.dart). Null while no window shows it: the answer is no.
  bool Function(OpenScreen request)? showScreen;

  /// The Settings section to bring into view next, by its id ("devices"),
  /// once: Settings scrolls to it and clears it.
  final settingsSection = ValueNotifier<String?>(null);

  /// The Marketplace tab to show next, by its id ("apps"), once: the
  /// Marketplace switches to it and clears it.
  final marketplaceTab = ValueNotifier<String?>(null);

  /// Needs you opens on History next, once: a History row was opened, so
  /// coming back finds the list it was opened from (row 684).
  var needsYouHistoryNext = false;

  /// The card Needs you shows next, by its NeedsYouItem key, once (a
  /// notification tapped): focus mode, or a phone's Needs you, goes to it
  /// when it's there and clears it.
  final needsYouCard = ValueNotifier<String?>(null);

  /// The app to open in the Marketplace's Apps tab next, by its id, once:
  /// its row opens, comes into view and clears it.
  final marketplaceApp = ValueNotifier<String?>(null);

  /// Counts the backend's word that some agent's monitors changed
  /// (MonitorsChanged): the Monitors page asks again on each.
  final monitorsChanged = ValueNotifier<int>(0);

  /// The last file in any agent's panes the backend said changed.
  final fileChanged = ValueNotifier<FileChanged?>(null);

  /// The latest list of a file's ink layers, whichever file: asked for
  /// (listInkLayers), or sent whenever a layer on its path is saved or
  /// deleted, from any device. A surface follows its own path.
  final inkLayerLists = ValueNotifier<InkLayerList?>(null);

  /// True from the backend's `welcome` until the connection drops.
  final connected = ValueNotifier<bool>(false);

  /// Whether to ask for Workspace patches; off, the backend sends whole
  /// Workspaces, as to apps before them (to compare the two in tests).
  bool askForPatches = true;

  /// Whether this connection has had a whole Workspace, which its patches
  /// change: the first state after connecting always is one, from a
  /// backend before revisions.
  var _wholeWorkspace = false;

  /// The revision of the Workspace [workspace] holds, exactly as this
  /// Claktory sent it (ServerMessage.workspace_revision); '' when none.
  /// Hello sends it, so a connection that comes back gets only what changed
  /// while it was away (#378), as a patch to it. Kept in memory only.
  var _workspaceRevision = '';

  /// When the backend last welcomed this connection; null while it's down.
  final connectedSince = ValueNotifier<DateTime?>(null);

  /// When this app was last connected to it, since it started: the moment
  /// its last connection dropped; null while connected, or never this
  /// run. Kept in memory only (friend bug #37).
  final lastConnected = ValueNotifier<DateTime?>(null);

  /// When it last checked in with the person's Claktory account (its
  /// service's last_contact_ms), as the account's list last said; null
  /// when it isn't on the account, or the list isn't read (friend bug
  /// #37). Set by whoever reads the list ([Claktories]).
  final checkedIn = ValueNotifier<DateTime?>(null);

  /// Since when this device's tunnel can't reach its relay, as first seen
  /// so, across the tunnels opened meanwhile; null while it can (friend
  /// bug #37).
  final relayDownSince = ValueNotifier<DateTime?>(null);

  /// True while it reconnects after the app came back from the background,
  /// the network moved or a connection that was up dropped: what shows
  /// stays, and nothing says it can't connect. It lasts at least
  /// [quietReconnect], and until the first try begun since has failed, so
  /// "can't connect" never shows while an honest try is still under way.
  final reconnecting = ValueNotifier<bool>(false);

  /// How long a reconnect stays quiet at least.
  Duration quietReconnect = const Duration(seconds: 8);

  /// How long a new connection has to open and be welcomed, the first
  /// time: after tries that ran out of it, the next gets longer
  /// ([_connectWindow]).
  Duration connectWithin = const Duration(seconds: 10);

  /// Tries in a row that ran out of time to connect or be welcomed: on a
  /// weak link, one that's getting there is let finish rather than cut at
  /// the same point every time (#413). Back to 0 once welcomed.
  var _slowTries = 0;

  /// How long this try has to open and be welcomed: [connectWithin], twice
  /// that after a try ran out of it, three times after two or more; and
  /// through a tunnel, never less than its dial has (which grows the same
  /// way, tunnel.rs DIAL_TIMEOUTS) and a little more for the handshake.
  Duration _connectWindow([Tunnel? through]) {
    var window =
        connectWithin * (_slowTries == 0 ? 1 : (_slowTries == 1 ? 2 : 3));
    final dialMs = through?.status.dialTimeoutMs ?? 0;
    final dial = Duration(milliseconds: dialMs) + const Duration(seconds: 2);
    if (dialMs > 0 && dial > window) window = dial;
    return window;
  }

  Timer? _quiet;

  /// The try under way when the quiet reconnect began (each try counts up
  /// [_tries]); the quiet ends with a later one.
  var _quietFrom = 0;
  var _tries = 0;
  var _quietTimeUp = false;
  var _quietTried = false;

  void _reconnectQuietly() {
    reconnecting.value = true;
    _quietFrom = _tries;
    _quietTimeUp = false;
    _quietTried = false;
    _quiet?.cancel();
    _quiet = Timer(quietReconnect, () {
      _quietTimeUp = true;
      _endQuiet();
    });
  }

  /// Try [tryNumber] ended without a welcome.
  void _triedQuietly(int tryNumber) {
    if (tryNumber <= _quietFrom) return;
    _quietTried = true;
    _endQuiet();
  }

  void _endQuiet() {
    // What it knew before shows again once the quiet time is up, without
    // waiting for a try that may take half a minute to fail: a glance at
    // Control Center made it say "Reconnecting…" for up to a minute
    // (Zucchero's row 486).
    if (_quietTimeUp && (_quietTried || _setAside != null) && !_moving) {
      reconnecting.value = false;
    }
  }

  /// This Claktory's move to another computer, as last seen (Workspace
  /// .move): kept while the connection is down, since the backend stops
  /// serving other devices once the move freezes, and after it's done, it
  /// answers no one. Null with no move, or before this Claktory's
  /// Workspace arrives.
  final move = ValueNotifier<Move?>(null);

  /// The relays its Claktory uses now, as its Workspace says (#450:
  /// Claktory's own while it's signed in, n0's while it isn't): the tunnel
  /// homes on them from its next endpoint on, and dials through them. Its
  /// pairing code's stay as they were kept; this is for this run only.
  void _followRelays(Workspace workspace) {
    if (!workspace.hasRemote()) return;
    final relays = workspace.remote.relays;
    if (relays.isEmpty) return;
    // Kept for the tunnels opened after this one, while it's away.
    _relaysSaid = List.unmodifiable(relays);
    final tunnel = _tunnel;
    if (tunnel == null || listEquals(relays, _relaysTold)) return;
    _relaysTold = List.unmodifiable(relays);
    tunnel.setRelays(relays);
  }

  void _sawMove(Workspace workspace) {
    final next = workspace.hasMove() ? workspace.move : null;
    if (next == move.value) return;
    move.value = next;
  }

  /// This Claktory moved to another computer (ERROR_CODE_MOVED): where and
  /// when, as the copy it left here says; null otherwise. Its screen takes
  /// the window's place (move.dart). The connection that was answered so
  /// stays open a while, for [deleteMovedCopy]; once it closes, the next
  /// Hello is answered the same way.
  final movedAway = ValueNotifier<MovedAway?>(null);

  /// The connection that was answered with [movedAway].
  WebSocket? _movedSocket;

  /// Deletes, for good, the copy a Claktory that moved away left on this
  /// Mac, the person having typed [typedWord] ("delete") to confirm. Only
  /// this Mac's own Claktory takes it, on the connection it answered with
  /// [movedAway]; then a new Hello says when it was deleted.
  Future<void> deleteMovedCopy(String typedWord) async {
    final socket = _socket;
    if (socket == null || !identical(socket, _movedSocket)) {
      // Between connections: the next one comes at once.
      _retryDelay = _minRetryDelay;
      _wakeUp();
      throw const RequestFailure(
        "Couldn't reach Claktory on this Mac just now. Try again in a moment.",
      );
    }
    final id = _nextId++;
    final answer = Completer<ServerMessage>();
    _pending[id] = answer;
    _send(
      ClientMessage(
        id: Int64(id),
        deleteMovedCopy: DeleteMovedCopy(typedWord: typedWord),
      ),
    );
    await answer.future;
    // Asked again, it says when the copy was deleted.
    _retryDelay = _minRetryDelay;
    unawaited(socket.close());
    _wakeUp();
  }

  // App views (app_views/): this connection's surfaces, as the backend
  // sends them, and what's asked of them.
  final _viewEvents = StreamController<AppViewEvent>.broadcast(sync: true);

  @override
  Stream<AppViewEvent> get viewEvents => _viewEvents.stream;

  @override
  ValueListenable<bool> get viewsConnected => connected;

  @override
  Future<AppSurfaceData> openView({
    required String appId,
    required String view,
    required String params,
    required List<String> catalogs,
  }) async {
    final reply = await _call(
      ClientMessage(
        openAppView: OpenAppView(
          appId: appId,
          view: view,
          params: params,
          catalogs: catalogs,
        ),
      ),
    );
    return appSurfaceData(reply.appSurface);
  }

  @override
  Future<void> actInView(String surfaceId, String message) => _request(
    ClientMessage(
      appViewAction: AppViewAction(surfaceId: surfaceId, message: message),
    ),
  );

  @override
  Future<void> closeView(String surfaceId) =>
      _request(ClientMessage(closeAppView: CloseAppView(surfaceId: surfaceId)));

  /// What the backend says the person should hear of (Notify), as it
  /// comes: it has applied every rule already (notifications.dart shows it
  /// as the system's notification unless this window is in use).
  Stream<Notify> get notified => _notified.stream;
  final _notified = StreamController<Notify>.broadcast(sync: true);

  /// Browser control's messages for this Mac's connection only
  /// (BrowserCommand, BrowserEnded), for its broker (browser/session.dart).
  Stream<ServerMessage> get browserMessages => _browser.stream;
  final _browser = StreamController<ServerMessage>.broadcast(sync: true);

  /// What this Mac's browser broker tells the backend (BrowserCapability,
  /// Grant, Result, Heartbeat, Stop): one of those, nothing else.
  Future<void> sendBrowser(ClientMessage message) {
    assert(
      const {
        ClientMessage_Body.browserCapability,
        ClientMessage_Body.browserGrant,
        ClientMessage_Body.browserResult,
        ClientMessage_Body.browserHeartbeat,
        ClientMessage_Body.browserStop,
      }.contains(message.whichBody()),
    );
    return _request(message);
  }

  /// Browser control's log, newest first (ListBrowserLog): [ask]'s agent
  /// ('' for all), entries before its `beforeMs` (0 for the newest), at
  /// most its `limit` (the backend's 100 when 0).
  Future<BrowserLog> listBrowserLog(ListBrowserLog ask) async {
    final reply = await _call(ClientMessage(listBrowserLog: ask));
    if (!reply.hasBrowserLog()) {
      throw const RequestFailure('Claktory sent no browser log.');
    }
    return reply.browserLog;
  }

  /// Empties browser control's log (ClearBrowserLog).
  Future<void> clearBrowserLog() =>
      _request(ClientMessage(clearBrowserLog: ClearBrowserLog()));

  /// What the person does with Claktory's browser (#435): stop it, delete
  /// its data, set its time zone and languages, sign it out of an identity
  /// provider. One of those, nothing else: the person never drives it
  /// (Matt, card 222).
  Future<void> sendAgentBrowser(ClientMessage message) {
    assert(
      const {
        ClientMessage_Body.stopAgentBrowser,
        ClientMessage_Body.deleteAgentBrowserData,
        ClientMessage_Body.setAgentBrowserLocale,
        ClientMessage_Body.signOutAgentBrowserAccount,
      }.contains(message.whichBody()),
    );
    return _request(message);
  }

  /// Every time zone Claktory's browser can use, by IANA name, sorted
  /// (ListTimeZones).
  Future<List<String>> listTimeZones() async {
    final reply = await _call(ClientMessage(listTimeZones: ListTimeZones()));
    if (!reply.hasTimeZones()) {
      throw const RequestFailure('Claktory sent no time zones.');
    }
    return reply.timeZones.names;
  }

  /// An errand's log, oldest first ([errandId] empty: every errand's), the
  /// last 7 days (ListErrandLog).
  Future<ErrandLog> listErrandLog(String errandId) async {
    final reply = await _call(
      ClientMessage(listErrandLog: ListErrandLog(errandId: errandId)),
    );
    if (!reply.hasErrandLog()) {
      throw const RequestFailure('Claktory sent no errand log.');
    }
    return reply.errandLog;
  }

  /// This Claktory's direct-message leaf, for one of the person's devices
  /// to certify (DmLeafOfferRequest; dms-e2ee §2, Claktories).
  Future<DmLeafOffer> dmLeafOffer() async {
    final reply = await _call(ClientMessage(dmLeafOffer: DmLeafOfferRequest()));
    if (!reply.hasDmLeafOffer()) {
      throw const RequestFailure('Claktory sent no leaf.');
    }
    return reply.dmLeafOffer;
  }

  /// Enrolls this Claktory's leaf: the [certificate] a device signed for
  /// the offered key, and its [pins] (JSON) (DmEnrollLeaf).
  Future<void> dmEnrollLeaf(List<int> certificate, List<int> pins) => _request(
    ClientMessage(
      dmEnrollLeaf: DmEnrollLeaf(certificate: certificate, pins: pins),
    ),
  );

  /// What the home sends this connection about the person's direct
  /// messages (#451): DmFollow's pages, then each new entry and delivery as
  /// it comes, and DmSend's answers too.
  final _dmFeeds = StreamController<DmFeed>.broadcast();
  Stream<DmFeed> get dmFeeds => _dmFeeds.stream;

  /// The home's state as it last said (DmHome): the answer to DmRoutes,
  /// and sent again to a following connection when it changes.
  final _dmHomes = StreamController<DmHome>.broadcast();
  Stream<DmHome> get dmHomes => _dmHomes.stream;

  /// Hands an MLS message to the home (DmSend): its receipt.
  Future<DmReceipt> dmSend(DmSend send) async {
    final reply = await _call(ClientMessage(dmSend: send));
    if (!reply.hasDmFeed() || !reply.dmFeed.hasSent()) {
      throw const RequestFailure('Claktory sent no receipt.');
    }
    return reply.dmFeed.sent;
  }

  /// Follows this device's groups from [DmFollow.cursors] (DmFollow): the
  /// pages, and what comes after, arrive on [dmFeeds].
  Future<void> dmFollow(DmFollow follow) =>
      _call(ClientMessage(dmFollow: follow));

  /// Says this device kept its groups' entries through these seqs (DmAck).
  Future<void> dmAck(DmAck ack) => _request(ClientMessage(dmAck: ack));

  /// Gives the home its routing table (DmRoutes): the home's state.
  Future<DmHome> dmRoutes(DmRoutes routes) async {
    final reply = await _call(ClientMessage(dmRoutes: routes));
    if (!reply.hasDmHome()) {
      throw const RequestFailure('Claktory sent no state.');
    }
    return reply.dmHome;
  }

  /// Turns advanced diagnostics on or off, or deletes what they kept
  /// (SetDiagnostics).
  Future<void> setDiagnostics(SetDiagnostics ask) =>
      _request(ClientMessage(setDiagnostics: ask));

  /// Where a pipe's reports go, or a project's default for new pipes
  /// (SetPipeForward); with make_reader, makes the recipient a reader too.
  Future<void> setPipeForward(SetPipeForward ask) =>
      _request(ClientMessage(setPipeForward: ask));

  /// Makes a kind of report, or a new version of one (SaveReportType).
  Future<void> saveReportType(SaveReportType ask) =>
      _request(ClientMessage(saveReportType: ask));

  /// Deletes a kind of report; its versions stay for the reports that
  /// use them (DeleteReportType).
  Future<void> deleteReportType(String id) =>
      _request(ClientMessage(deleteReportType: DeleteReportType(id: id)));

  /// A pipe agent's daily spending caps (SetSpendCap); 0 for a default.
  Future<void> setSpendCap(SetSpendCap ask) =>
      _request(ClientMessage(setSpendCap: ask));

  /// Contains an agent for errands, or lets it out (SetAgentContained,
  /// the person's alone).
  Future<void> setAgentContained(String agentId, {required bool contained}) =>
      _request(
        ClientMessage(
          setAgentContained: SetAgentContained(
            agentId: agentId,
            contained: contained,
          ),
        ),
      );

  /// Makes an agent read reports, or lets it go back (SetReportReader).
  Future<void> setReportReader(SetReportReader ask) =>
      _request(ClientMessage(setReportReader: ask));

  /// Sends a pipe's waiting reports to where they go now, or discards
  /// them (DecidePipeReports).
  Future<void> decidePipeReports(String pipeId, {required bool send}) =>
      _request(
        ClientMessage(
          decidePipeReports: DecidePipeReports(pipeId: pipeId, send: send),
        ),
      );

  /// A page of a pipe's reports, or a reader's across its pipes
  /// (ListPipeReports), newest first.
  Future<PipeReportPage> listPipeReports(ListPipeReports ask) async {
    final reply = await _call(ClientMessage(listPipeReports: ask));
    if (!reply.hasPipeReportPage()) {
      throw const RequestFailure('Claktory sent no reports.');
    }
    return reply.pipeReportPage;
  }

  /// Whether the person is using this Claktory here now (SetActive): said
  /// to the backend as it changes and on every new connection, since a
  /// connection counts as inactive until it says otherwise. While any of
  /// its devices is active, the backend notifies none.
  bool get active => _active;
  var _active = false;

  void setActive(bool active) {
    if (active == _active) return;
    _active = active;
    if (connected.value) fireAndLog(sendActive(active));
  }

  /// What this device's system says of Claktory's notifications, sent with
  /// every SetActive (item 13): an iPhone's or iPad's push registrar sets it
  /// (mobile/push.dart); UNSPECIFIED where nothing does, judged by its
  /// platform as before.
  static NotificationPermission Function() notificationPermission =
      _unspecified;

  static NotificationPermission _unspecified() =>
      NotificationPermission.NOTIFICATION_PERMISSION_UNSPECIFIED;

  /// Says whether the person is using this Claktory here (SetActive), and
  /// whether this device shows notifications.
  Future<void> sendActive(bool active) => _request(
    ClientMessage(
      setActive: SetActive(
        active: active,
        notifications: notificationPermission(),
      ),
    ),
  );

  /// Says SetActive again: this device's notifications changed (turned on
  /// or off in its Settings).
  void resendActive() {
    if (connected.value) fireAndLog(sendActive(_active));
  }

  /// Hands this device's push registration to the backend
  /// (RegisterNotifications): the relay's handle and the key it seals with.
  /// An empty handle stops the pushes.
  Future<void> registerNotifications(RegisterNotifications registration) =>
      _request(ClientMessage(registerNotifications: registration));

  /// Sends a test notification to every device (SendTestNotification).
  Future<void> sendTestNotification() =>
      _request(ClientMessage(sendTestNotification: SendTestNotification()));

  /// The move is copying or past it: a dropped connection is the move's
  /// doing (the backend serves only this computer once it freezes, and
  /// nothing once it's done), so the reconnect stays quiet, however long
  /// it takes, rather than saying it can't connect.
  bool get _moving => switch (move.value?.phase) {
    MovePhase.MOVE_PHASE_COPYING ||
    MovePhase.MOVE_PHASE_FROZEN ||
    MovePhase.MOVE_PHASE_IMPORTING ||
    MovePhase.MOVE_PHASE_COMMITTING ||
    MovePhase.MOVE_PHASE_DONE => true,
    _ => false,
  };

  /// The device's network moved (Wi-Fi to cellular, or back after none):
  /// a Claktory on another machine is reached afresh at once, quietly,
  /// rather than once pings go unanswered. This Mac's own isn't on the
  /// network.
  void networkMoved() {
    if (_closed ||
        _suspended ||
        _target == null ||
        refused.value != null ||
        signInRequired.value != null) {
      return;
    }
    _log('the network moved: connecting afresh');
    _prefetchAssertion();
    // Not on screen, it tries at once too, whatever its backoff.
    _tryNow = true;
    _restart(ConnectionReason.network);
  }

  /// Whether this is the Claktory on screen, the one the window shows. On
  /// a phone or an iPad only it tries again every few seconds while it
  /// can't connect; the others, once their first quick tries fail, wait
  /// twice as long each time, up to [awayRetryMost] (#454), and try again
  /// at once when the network moves or the person opens them. True unless
  /// told otherwise: a Mac keeps every Claktory at the quick pace.
  bool get onScreen => _onScreen;
  var _onScreen = true;
  set onScreen(bool value) {
    if (value == _onScreen) return;
    _onScreen = value;
    if (!value || _closed) return;
    // Opened: tried again at once, as quickly as before.
    final away = _awayDelay;
    _awayDelay = Duration.zero;
    if (!connected.value && away > Duration.zero) {
      _log('on screen: trying again now');
      check();
    }
  }

  /// How long a Claktory not on screen waits between tries now; zero
  /// while it's connected, on screen, or in its first quick tries.
  var _awayDelay = Duration.zero;

  /// The longest a Claktory not on screen waits between tries.
  Duration awayRetryMost = _awayRetryMost;

  /// The longest a Claktory on screen waits between tries.
  @visibleForTesting
  Duration retryMost = _maxRetryDelay;

  /// The next try goes at once even off screen: the network moved, or the
  /// person asked.
  var _tryNow = false;

  /// Whether this one is backing off ([onScreen]): not on screen, and its
  /// quick tries have failed.
  bool get _backingOff => !_onScreen && _awayDelay > Duration.zero;

  /// Not on screen, the wait after a failed try ([since]) goes on to its
  /// backoff, through wakes that don't call for a try (back in front, a
  /// check after sleep), and through the background, where timers don't
  /// run: back, a try already due goes at once.
  Future<void> _awayWait(DateTime since) async {
    while (!_closed && _backingOff && !_tryNow) {
      final left = since.add(_awayDelay).difference(DateTime.now());
      if (left <= Duration.zero) break;
      if (_parked || _suspended) {
        await _wake.future;
        continue;
      }
      final due = Completer<void>();
      final timer = Timer(left, due.complete);
      await Future.any([due.future, _wake.future]);
      timer.cancel();
    }
    _tryNow = false;
  }

  /// Whether the connection is closed while the app is in the background
  /// ([suspend]), until it's back.
  var _suspended = false;

  /// Whether it keeps trying while the app is in the background, on its
  /// usual schedule (the quick one on screen, its backoff off screen),
  /// rather than waiting until the app is back (W6, Android: the Claktory
  /// a share came from, while its page is open in a browser tab over the
  /// app, so its network coming back after the screen was off brings it
  /// back). The app stays in the background as far as everything else
  /// goes: nothing says it resumed. Set true, it undoes [suspend].
  bool get keepTryingWhileHidden => _keepTrying;
  var _keepTrying = false;
  set keepTryingWhileHidden(bool value) {
    if (value == _keepTrying || _closed) return;
    _keepTrying = value;
    if (value && _hiddenAt != null) _keptWhileHidden = true;
    if (!value) return;
    if (_suspended) {
      _suspended = false;
      _log('kept trying in the background: its connection opens again');
    }
    _wakeUp();
  }

  /// In the background and not told to keep trying there: nothing dials
  /// until the app is back.
  bool get _parked => _hiddenAt != null && !_keepTrying;

  /// Closes the connection while the app is in the background, tunnel and
  /// all, so nothing goes out until it's back ([resumed] makes it afresh):
  /// for Android, which keeps the app running there (#454). Only while
  /// hidden; a dial that ends after this is closed at once.
  void suspend() {
    if (_closed || _suspended || _hiddenAt == null) return;
    _suspended = true;
    _log('in the background: closing the connection until the app is back');
    _tunnelBack?.cancel();
    _tunnelBack = null;
    _backWatch?.cancel();
    _backWatch = null;
    final abandon = _abandon;
    _abandon = Completer<void>();
    abandon.complete();
    _redialed = _socket;
    unawaited(_socket?.close());
    _freshTunnel();
    _wakeUp();
  }

  /// Work the person waits on with this Claktory (an upload, a recording's
  /// words), while any is under way ([holding]): an app in the background
  /// keeps its connection for it.
  ValueListenable<bool> get holds => _holds;
  final _holds = ValueNotifier<bool>(false);
  var _holding = 0;

  /// Runs [work], counted in [holds] until it ends, however it ends.
  Future<T> holding<T>(Future<T> Function() work) async {
    _holding++;
    _holds.value = true;
    try {
      return await work();
    } finally {
      if (--_holding == 0) _holds.value = false;
    }
  }

  StreamSubscription<NetworkPath>? _network;

  /// When the app went to the background, while it's there.
  DateTime? _hiddenAt;

  /// Told to keep trying at some point during this time in the background
  /// (a shared website open in a browser tab over the app, #505): its
  /// connection was kept live, so coming back needn't make it afresh
  /// (Risotto, Sale row 175). Kept as a latch, since the tab may say it
  /// closed before the app says it's back.
  var _keptWhileHidden = false;

  AndroidBackgroundConnection? _android;

  /// The app went to the background: iOS suspends it soon, and the
  /// connection dies without saying so.
  void hidden() {
    if (_hiddenAt != null) return;
    _hiddenAt = DateTime.now();
    _keptWhileHidden = _keepTrying;
    _log('to the background');
    _note(ConnectionKind.lifecycle, {ConnectionKey.app: AppLife.background});
    _android?.hidden();
  }

  /// How long the tunnel has, once told to reconnect (back from the
  /// background, the network moved), to bring the connection back: until
  /// then nothing asks it to reconnect again (it makes its endpoint afresh
  /// when that's needed, and another ask would start that over), and if
  /// it hasn't by then, the tunnel is closed and opened afresh, once.
  Duration tunnelBackWithin = const Duration(seconds: 10);

  /// The app is back. After more than a moment away, the connection is
  /// made afresh at once (the old one is most likely dead), quietly, and
  /// the backend's snapshot brings the conversation up to date. Back
  /// within the grace the device kept it running ([BackgroundGrace], #413),
  /// it's only checked: no new endpoint, nor the whole sync, unless the
  /// check goes unanswered.
  void resumed({DateTime? now}) {
    _android?.resumed();
    // Back: input, which ends idleness ([_idle]).
    UserInput.instance.touched();
    final hiddenAt = _hiddenAt;
    _hiddenAt = null;
    final kept = _keptWhileHidden;
    _keptWhileHidden = false;
    _leftInBackground = false;
    final suspended = _suspended;
    _suspended = false;
    // Its assertion may have lapsed meanwhile: on its way before it's
    // needed, whether or not the connection is made afresh.
    _prefetchAssertion();
    final away = hiddenAt == null
        ? Duration.zero
        : (now ?? DateTime.now()).difference(hiddenAt);
    if (hiddenAt != null) {
      _log('back after ${away.inSeconds}s');
      _note(ConnectionKind.lifecycle, {
        ConnectionKey.app: AppLife.back,
        ConnectionKey.awayS: away.inSeconds,
      });
    }
    if (_qaStallResume && away >= const Duration(seconds: 3)) {
      _qaStallNext = true;
    }
    // Turned away until this device signs in: back, it asks once more,
    // quietly; the Claktory may have let it in meanwhile (row 189).
    if (hiddenAt != null) probeSignIn();
    // One that can't connect and isn't on screen keeps to its backoff,
    // counting the time away: a try already due goes now (#454).
    if (_backingOff && !connected.value) {
      _wakeUp();
      return;
    }
    if (suspended) {
      _restart(ConnectionReason.resume);
      return;
    }
    if (away < const Duration(seconds: 3)) {
      // Not connected, whatever it last said is stale now: it tries again.
      if (!connected.value) check();
      return;
    }
    if (BackgroundGrace.heldThrough(away)) {
      _log('back within the grace: checking the connection');
      check();
      return;
    }
    // Kept connected while away (a browser tab over the app held it up):
    // most likely still alive, so only checked; a check unanswered makes
    // it afresh as usual.
    if (kept && connected.value) {
      _log('kept connected while away: checking the connection');
      check();
      return;
    }
    _restart(ConnectionReason.resume);
  }

  /// Makes the connection afresh at once, quietly: the old one is most
  /// likely dead (the tunnel's too), and the backend's snapshot brings what
  /// shows up to date.
  void _restart(ConnectionReason reason) {
    // Back, or on another network: what failed before says nothing now.
    if (reason == ConnectionReason.resume ||
        reason == ConnectionReason.network) {
      _freshFailure();
    }
    _note(ConnectionKind.reconnect, {ConnectionKey.reason: reason});
    _reconnectQuietly();
    _reconnectTunnel(again: true);
    _retryDelay = _minRetryDelay;
    _redialed = _socket;
    unawaited(_socket?.close());
    _wakeUp();
    _watchBack();
  }

  /// How long a reconnect has to bring the connection back before it's
  /// made from scratch ([reconnectFromScratch]), then twice as long each
  /// time after, up to [_backWithinMost], until it's back (#214).
  Duration backWithin = const Duration(seconds: 20);
  static const _backWithinMost = Duration(minutes: 5);

  /// The watch [_watchBack] keeps, and how long its next wait is.
  Timer? _backWatch;
  Duration? _backNext;

  /// Watches that the connection comes back: a reconnect (back from the
  /// background, the network moved, an unanswered check, a drop) that
  /// hasn't brought it back within [backWithin] starts it from scratch,
  /// as relaunching the app would, so nothing it waits on can keep it
  /// down for good. Not while the app is in the background, nor while
  /// this device is turned away or has to sign in.
  void _watchBack() {
    if (_closed || target == null) return;
    _backWatch?.cancel();
    final wait = _backNext ?? backWithin;
    void check() {
      _backWatch = null;
      if (_closed ||
          connected.value ||
          _parked ||
          _backingOff ||
          refused.value != null ||
          signInRequired.value != null) {
        return;
      }
      // A dial still under way is let finish (it has its own limit, which
      // grows on a weak link: #413): starting from scratch would only
      // throw it away and dial from zero again.
      if (_tunnel?.status.phase == TunnelPhase.dialing) {
        _backWatch = Timer(const Duration(seconds: 1), check);
        return;
      }
      final doubled = wait * 2;
      _backNext = doubled > _backWithinMost ? _backWithinMost : doubled;
      reconnectFromScratch(
        'not back within ${wait.inSeconds}s',
        reason: ConnectionReason.watch,
      );
    }

    _backWatch = Timer(wait, check);
  }

  /// Gives up on the try under way, whatever it waits on: it throws
  /// [_Abandoned] at once ([_orAbandoned]).
  var _abandon = Completer<void>();

  /// [work], or [_Abandoned] as soon as the try is given up on; what
  /// [work] brings after that goes to [late] (a socket to close, say).
  Future<T> _orAbandoned<T>(Future<T> work, {void Function(T)? late}) {
    final abandon = _abandon.future;
    var given = false;
    return Future.any<T>([
      work.then((value) {
        if (given) late?.call(value);
        return value;
      }),
      abandon.then<T>((_) {
        given = true;
        throw const _Abandoned();
      }),
    ]);
  }

  /// Closes [socket], waiting a moment at most: one on a dead connection
  /// may never finish closing.
  Future<void> _let(WebSocket socket) =>
      socket.close().timeout(const Duration(seconds: 2), onTimeout: () {});

  /// Whether the tunnels' runtime is asked if it still answers before the
  /// next tunnel opens: after a from-scratch start, or a tunnel that
  /// didn't open or close in time.
  var _checkRuntime = false;

  /// How long the tunnels' runtime has to answer before it's made afresh.
  Duration runtimeAnswerWithin = const Duration(seconds: 2);

  /// Starts the connection from scratch, as relaunching the app does: the
  /// try under way given up on whatever it waits for, the connection and
  /// the tunnel dropped (its endpoint with it), the tunnels' runtime made
  /// afresh if it no longer answers, and a new connection made at once.
  /// When the person taps Reconnect, and when a reconnect doesn't bring
  /// the connection back in time ([_watchBack]).
  void reconnectFromScratch(
    String why, {
    ConnectionReason reason = ConnectionReason.asked,
  }) {
    if (_closed) return;
    _log('starting the connection from scratch: $why');
    _tryNow = true;
    _note(ConnectionKind.reconnect, {ConnectionKey.reason: reason});
    _reconnectQuietly();
    final abandon = _abandon;
    _abandon = Completer<void>();
    abandon.complete();
    _checkRuntime = true;
    _freshTunnel();
    _retryDelay = _minRetryDelay;
    _redialed = _socket;
    unawaited(_socket?.close());
    _wakeUp();
    _watchBack();
  }

  /// Tells the tunnel to reconnect: the tunnel decides whether that takes
  /// a new endpoint. [again] is a new reason to (back from the
  /// background, the network moved, the person asked): it's told at once,
  /// and given [tunnelBackWithin] to bring the connection back before it's
  /// opened afresh. Otherwise (a try that failed, a connection that
  /// dropped) it isn't told while it's still within that time.
  void _reconnectTunnel({bool again = false}) {
    final open = _tunnel;
    if (open == null) return;
    // In the background (Android keeps the app running there, its network
    // cut in Doze), the tunnel is left as it is: coming back tells it, so
    // a rebuild meanwhile would only be thrown away.
    if (_parked) {
      if (!_leftInBackground) _log('in the background: the tunnel waits');
      _leftInBackground = true;
      return;
    }
    if (!again && _tunnelBack != null) return;
    unawaited(open.reconnect());
    if (!again) return;
    _tunnelBack?.cancel();
    _tunnelBack = Timer(tunnelBackWithin, () {
      _tunnelBack = null;
      if (_closed ||
          connected.value ||
          _parked ||
          _backingOff ||
          !identical(_tunnel, open)) {
        return;
      }
      _log(
        'not back within ${tunnelBackWithin.inSeconds}s: '
        'opening the tunnel afresh',
      );
      _note(ConnectionKind.reconnect, {
        ConnectionKey.reason: ConnectionReason.tunnel,
      });
      _freshTunnel();
      _retryDelay = _minRetryDelay;
      unawaited(_socket?.close());
      _wakeUp();
    });
  }

  /// While the tunnel has [tunnelBackWithin] to bring the connection back.
  Timer? _tunnelBack;

  /// Said once each time the app is in the background.
  var _leftInBackground = false;

  /// Closes the tunnel, so the next try opens a new one: a new endpoint
  /// and a new port. That one opens only once this one has closed, so the
  /// relay never sees this device's key twice.
  void _freshTunnel() {
    if (_tunnel == null) return;
    _tunnelBack?.cancel();
    _tunnelBack = null;
    _tunnelClosing = _closeTunnel();
  }

  /// The tunnel [_freshTunnel] is closing, until the next opens.
  Future<void>? _tunnelClosing;

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
  /// again at once rather than after its backoff, quietly: a failure from
  /// before (the Mac asleep, the app away) says nothing about now, so
  /// "can't connect" waits for a try made since to fail.
  void check() {
    if (_closed ||
        _suspended ||
        refused.value != null ||
        signInRequired.value != null) {
      return;
    }
    final socket = _socket;
    // Backing off, it keeps to its backoff (#454).
    if (_backingOff && !connected.value) return;
    if (socket == null || !connected.value) {
      // A failure from before (asleep, away) says nothing about now.
      _freshFailure();
      _reconnectQuietly();
      _reconnectTunnel();
      _retryDelay = _minRetryDelay;
      _wakeUp();
      return;
    }
    if (_checking) return;
    final id = _nextId++;
    try {
      socket.add(ClientMessage(id: Int64(id)).writeToBuffer());
    } on StateError {
      // Closed already, a reconnect on its way (back from the background,
      // say): nothing left to check.
      return;
    }
    _checking = true;
    final answer = Completer<ServerMessage>();
    _pending[id] = answer;
    answer.future
        .timeout(answerWithin)
        .then<void>(
          (_) {},
          onError: (Object error) {
            if (error is TimeoutException && identical(_socket, socket)) {
              _log('no answer within ${answerWithin.inSeconds}s: reconnecting');
              _restart(ConnectionReason.check);
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
  static const _tick = Duration(seconds: 1);

  void _watchSleep() {
    _lastTick = DateTime.now();
    _sleepWatch ??= Timer.periodic(_tick, (_) {
      final now = DateTime.now();
      final slept = now.difference(_lastTick) > _tick * 4;
      _lastTick = now;
      // Not while the app is in the background: coming back checks.
      if (slept && _hiddenAt == null) {
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

  /// transcribeAudio's requests waiting for what was heard, by request id.
  final _dictations = <String, Completer<AudioTranscribed>>{};

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

  /// This Claktory needs this device to show it's the owner's before it
  /// lets it in (ERROR_CODE_SIGN_IN_REQUIRED): its grace to confirm is
  /// over, or it was found through the account and this device isn't
  /// signed in to it (or no assertion could be had). What it said; null
  /// otherwise. Like [refused], nothing's tried until [retry].
  final signInRequired = ValueNotifier<String?>(null);

  /// With [signInRequired], what the Claktory said of its own account
  /// (#527, Sale's row 189 b): its account service (empty for
  /// claktory.com) and its email, masked ("m…@example.com"; empty when
  /// it isn't signed in). A Claktory says it only to a device it knows;
  /// null when it said neither, so nothing's known from it.
  ({String service, String email})? signInSaid;

  /// This Claktory is signed in to another Claktory account than this
  /// device (ERROR_CODE_OTHER_ACCOUNT, #529 S4): what it said, which says
  /// which case; null otherwise. [refused] is set with it, so nothing's
  /// tried until [retry].
  final otherAccount = ValueNotifier<String?>(null);

  /// With [otherAccount]: this device was let in here before and is now
  /// signed in to a different Claktory account, so signing in with the
  /// right one lets it back in (Error.other_account_before); false for a
  /// device it never let in, which only a pipe can work with.
  var otherAccountBefore = false;

  /// While this device isn't confirmed as the owner's on this Claktory,
  /// when its grace to confirm ends (`Welcome.confirm_by_ms`); null when it
  /// needn't confirm.
  final confirmBy = ValueNotifier<DateTime?>(null);

  /// This device's account, which gets the assertion a Hello shows a
  /// Claktory on another machine; null where there's none (tests).
  DeviceAccount? get account => _account;
  DeviceAccount? _account;
  set account(DeviceAccount? account) {
    _account?.removeListener(_accountChanged);
    _account = account?..addListener(_accountChanged);
  }

  /// Signed in on this device while this Claktory waits for it to
  /// confirm: it confirms now, without connecting again.
  void _accountChanged() {
    final target = _target;
    final key = _helloKey;
    if (confirmBy.value == null ||
        !connected.value ||
        target == null ||
        key == null ||
        !(account?.signedIn ?? false)) {
      return;
    }
    unawaited(() async {
      final assertion = await _assertionFor(target, key);
      if (assertion == null || assertion.isEmpty || target != _target) {
        return;
      }
      try {
        await confirmThisDevice(assertion);
        if (target == _target) confirmBy.value = null;
      } on RequestFailure catch (failure) {
        _log("couldn't confirm this device: ${failure.message}");
      }
    }());
  }

  /// On a Factory on another machine, the paired device this connection is
  /// (`Welcome.device_id`); empty on this Mac's.
  var deviceId = '';

  /// The Claktory's name and color as it said on connecting
  /// (`Welcome.name`, `Welcome.color`), until its Settings carry them.
  var welcomeName = '';
  var welcomeColor = '';

  final Tunnels _tunnels;

  /// What the tunnels run on: made afresh when it stops answering (#214).
  final TunnelRuntime _runtime;

  /// Reads this Mac's own pairing code, each time it connects: a backend
  /// that's been updated may have a new one.
  final Future<RemoteFactory?> Function() _localPairing;

  /// This app's key for this Mac's Factory: any key on this machine gets
  /// in with its code, so one made for each run.
  late final _localKey = newDeviceKey();

  /// This Mac's Factory's open port, as a WebSocket address.
  final String localEndpoint;

  /// How this PC's own Claktory is reached on Windows: its app-token, sent
  /// only once the server end of the connection is this same account's.
  /// Null elsewhere.
  final WindowsLocal? _windowsLocal;

  /// The app-token this try's Hello carries to this PC's Claktory, read
  /// afresh each try (the backend writes a new one at every start).
  String? _localAppToken;

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

  /// The relays [_tunnel] was last told its Claktory uses.
  List<String> _relaysTold = const [];

  /// The relays its Claktory last said it uses ([_followRelays]): a
  /// tunnel opened afresh (made from scratch while it can't connect, say)
  /// homes on them rather than the pairing code's, which may be gone.
  List<String> _relaysSaid = const [];

  /// Whether [_tunnel]'s port on this device has let a connection in: only
  /// then does its going quiet mean the system took it back (opened afresh
  /// at once), rather than a tunnel that can't reach its Claktory (shown).
  var _tunnelServed = false;

  /// [error] as it can be logged: without the tunnel's path, which a
  /// failed WebSocket's message would carry in its URL.
  String _withoutTunnelPath(Object error) {
    final path = _tunnel?.requestPath ?? '';
    final said = '$error';
    return path.length > 1 ? said.replaceAll(path, '/…') : said;
  }

  RemoteFactory? _tunnelTarget;

  /// The ports this device forwards to the backend's machine, for browser
  /// flows that come back to localhost there (loopback_forward.dart).
  late final forwards = LoopbackForwards(
    remote: () => target != null,
    forward: forwardPort,
    forwardTo: (port, target) => forwardPort(port, target: target),
  );

  /// Listens on [port] of this device's loopback and carries each
  /// connection through the tunnel to the same port on the backend's
  /// machine, or to [target] there (throws [ForwardFailure]).
  Future<PortForward> forwardPort(int port, {int? target}) async {
    final open = _tunnel;
    if (open == null) {
      throw const ForwardFailure(
        ForwardProblem.failed,
        'Not connected to it right now.',
      );
    }
    return open.forwardPort(port, target: target);
  }

  /// The websites pipes' peers share that this device forwards (#503): on
  /// a computer showing a Claktory on another machine, each one opened
  /// there (listening), live and not hidden, for as long as that lasts; on
  /// a phone or tablet only those opened on it (W6), so nothing listens in
  /// the background for the others, and on an iPhone or iPad the one whose
  /// page is open over the app again once its tunnel was made afresh (back
  /// from the Home Screen, #505), so reloading it works.
  void _keepShares(Workspace workspace) {
    unawaited(
      forwards.keepShares(
        target == null
            ? const {}
            : sharesToForward(
                workspace,
                touch: sharesOnTouchDevice,
                forwarded: (id) => forwards.sharePort(id) != null,
                shown: (id) => inAppShareBrowser.shows(this, id),
              ),
      ),
    );
  }

  Timer? _tunnelWatch;

  /// The tunnel's [TunnelStatus.received] when it was last read, and
  /// when it last moved.
  int _tunnelReceived = 0;
  DateTime? _tunnelMovedAt;

  /// The tunnel's [TunnelStatus.linkReceived] when it was last read, and
  /// when it last moved.
  int _tunnelLinkReceived = 0;
  DateTime? _tunnelLinkMovedAt;

  /// How long after the tunnel last brought data the link's own traffic
  /// still counts as the connection alive ([_showTunnel]).
  static const _transferPause = Duration(seconds: 60);

  Future<void> run() async {
    if (Platform.isAndroid) _android = AndroidBackgroundConnection(this);
    _watchSleep();
    // Recordings kept on this device from before, sent once connected.
    KeptVoices.of(this);
    _note(ConnectionKind.lifecycle, {ConnectionKey.app: AppLife.started});
    deviceNetwork.addListener(_networkKnown);
    deviceNetwork.addListener(_watchBlocked);
    _network ??= deviceNetworkMoves.listen((path) {
      _note(ConnectionKind.network, {ConnectionKey.net: netKind(path)});
      networkMoved();
    });
    while (!_closed) {
      if (refused.value != null || signInRequired.value != null) {
        // Asked once more, quietly (probeSignIn): one try, its screen
        // staying up meanwhile; turned away again, it waits here again.
        if (_probing && refused.value == null) {
          _probing = false;
        } else {
          _probing = false;
          reconnecting.value = false;
          await _wake.future;
          continue;
        }
      }
      // In the background with its connection closed: nothing until the
      // app is back.
      if (_suspended) {
        await _wake.future;
        continue;
      }
      final target = _target;
      if (target == null && !localBackend) {
        await _wake.future;
        continue;
      }
      // Found through the account, it's let in only on an assertion.
      if (target != null &&
          target.fromAccount &&
          !(account?.signedIn ?? false)) {
        signInRequired.value = _signInToConnect;
        continue;
      }
      // This Mac's Factory through the tunnel too, with its own code; the
      // open port is for a backend from before it, and with the code's
      // token when the tunnel to it fails (every other try then).
      final local = target == null ? await _localPairing() : null;
      final via = target ?? local;
      // Without a tunnel (a moved copy's code), the open port only.
      final loopback =
          via == null || (local != null && (_loopbackNext || !local.tunnelled));
      if (local != null) _loopbackNext = false;
      _welcomed = false;
      final tryNumber = ++_tries;
      _tryRoute = loopback
          ? ConnectionRoute.local
          : via.tunnelled
          ? ConnectionRoute.tunnel
          : ConnectionRoute.plain;
      _tryStarted = DateTime.now();
      _readyDue = false;
      _tryNet = switch (deviceNetwork.value) {
        final path? => netKind(path),
        null => null,
      };
      if (_tryNet != null) _netTold = true;
      _note(ConnectionKind.connect, {
        ConnectionKey.attempt: tryNumber,
        ConnectionKey.route: _tryRoute,
        ConnectionKey.net: ?_tryNet,
      });
      ConnectionFailure why;
      int? closeCode;
      // A Claktory on another machine is shown this device's assertion:
      // asked for now, while the tunnel dials, rather than after.
      final assertion = target != null && target.tunnelled
          ? _assertionAhead(target, deviceKey(target))
          : null;
      var abandoned = false;
      try {
        final WebSocket socket;
        _localAppToken = null;
        if (loopback && _windowsLocal != null) {
          socket = await _orAbandoned(
            _windowsLoopback(_windowsLocal),
            late: _let,
          );
        } else if (loopback) {
          socket = await _orAbandoned(
            _connectWithin(WebSocket.connect(localEndpoint)),
            late: _let,
          );
        } else if (via.tunnelled) {
          final tunnel = await _orAbandoned(
            _tunnelTo(via, key: target == null ? _localKey : null),
          );
          // Plain ws: iroh already encrypts it end to end, to the key the
          // pairing code names.
          // A dead connection inside the tunnel takes the handshake with
          // it: given up on, and the tunnel dials afresh.
          // At the tunnel's own path: nothing else on this device can
          // pass for this app there.
          try {
            socket = await _orAbandoned(
              _connectWithin(
                WebSocket.connect(
                  'ws://127.0.0.1:${tunnel.port}${tunnel.requestPath}',
                ),
                through: tunnel,
              ),
              late: _let,
            );
            if (identical(tunnel, _tunnel)) _tunnelServed = true;
          } on SocketException {
            // Its own port on this device didn't answer after it had let
            // connections in: the tunnel no longer listens there (the
            // system took it back while the app was away). The next try,
            // at once, opens it afresh, unless the machine turned this
            // device away, which says why below. One that never answered
            // is kept, so why it can't connect shows.
            // Not while the app is in the background: once it's back,
            // the port is tried again, and opened afresh then.
            if (_tunnelServed &&
                identical(tunnel, _tunnel) &&
                tunnel.status.failure != TunnelFailure.refused &&
                !_parked) {
              _log("the tunnel's port didn't answer: opening it afresh");
              _freshTunnel();
              _retryDelay = _minRetryDelay;
            }
            rethrow;
          }
        } else {
          socket = await _orAbandoned(
            _connectWithin(
              WebSocket.connect(
                via.endpoint.toString(),
                customClient: via.client(),
              ),
            ),
            late: _let,
          );
        }
        if (_closed || target != _target) {
          await _let(socket);
          continue;
        }
        // No ping yet: the welcome's window below says whether the link
        // answers, and the first frame (Welcome, or "waiting to be
        // allowed") starts the pings ([_receive]). On a weak link a ping
        // sent before then could only cut the try at the same moment, its
        // pong queued behind everything else still on its way (Sale's row
        // 81, #413).
        _socket = socket;
        _watchTunnel();
        // What the tunnel had received when this connection opened, for
        // the log when it closes.
        final receivedBefore = _tunnelReceived;
        _helloVia = via;
        _helloKey = via == null || !via.tunnelled || loopback
            ? null
            : target == null
            ? _localKey
            : deviceKey(via);
        // Not had in time (claktory.com slow or out of reach): found
        // through the account, it tries again (what's on its way is kept
        // for then); paired with a code, it goes without.
        final asserted = assertion == null
            ? ''
            : await _orAbandoned(
                assertion.timeout(assertionWithin, onTimeout: () => null),
              );
        _helloAssertion = asserted ?? '';
        if (_closed || target != _target) {
          await _let(socket);
          continue;
        }
        if (target != null && target.fromAccount && asserted == null) {
          _log('no device assertion in time: trying again');
          await _let(socket);
          throw const _NotNow();
        }
        // Found through the account, it's let in only on an assertion.
        if (target != null && target.fromAccount && _helloAssertion.isEmpty) {
          await _let(socket);
          signInRequired.value = _signInToConnect;
          continue;
        }
        final kind = await _orAbandoned(thisDeviceKindForHello());
        final name = await _orAbandoned(thisDeviceNameForHello());
        final platform = await _orAbandoned(thisPlatformForHello());
        _helloId = _nextId++;
        _send(
          ClientMessage(
            id: Int64(_helloId),
            hello: Hello(
              protocolVersion: _protocolVersion,
              clientVersion: _clientVersion,
              token: _helloVia?.token ?? _localAppToken,
              assertion: _helloAssertion,
              // It can sign in and confirm it's the owner's.
              canConfirm: true,
              deviceKind: kind,
              deviceName: name,
              platform: platform,
              // Changes come as patches to the last Workspace, not whole.
              workspacePatches: askForPatches,
              // Back, it carries on from the Workspace it holds.
              workspaceRevision: askForPatches ? _workspaceRevision : '',
              // Back, the conversation shown carries on from what it holds
              // (#413): only what changed since comes.
              transcriptAgentId: transcript.heldRevision.isEmpty
                  ? ''
                  : transcript.agentId,
              transcriptRevision: transcript.heldRevision,
            ),
          ),
        );
        // Welcomed within [_connectWindow] of asking, or it starts again,
        // with longer; a device waiting to be allowed waits as long as its
        // code lasts. From the Hello, not the socket: the wait for this
        // device's assertion (up to [assertionWithin]) isn't the
        // Claktory's to answer for.
        final welcomeWithin = _connectWindow();
        final asked = DateTime.now();
        final throughTunnel = via != null && via.tunnelled && !loopback;
        Timer? welcome;
        void welcomeDue() {
          if (!identical(_socket, socket) || connected.value) return;
          // The tunnel's link still answering: the Welcome may be on its
          // way behind everything else on a weak link, so a few seconds
          // more, up to [_transferPause] from the Hello (Sale's 63 kbps
          // run: cut 73 ms after the link last moved).
          final link = _tunnelLinkMovedAt;
          final now = DateTime.now();
          if (throughTunnel &&
              link != null &&
              now.difference(link) < const Duration(seconds: 2) &&
              now.difference(asked) < _transferPause) {
            welcome = _welcomeTimer = Timer(
              const Duration(seconds: 5),
              welcomeDue,
            );
            return;
          }
          _log('no welcome within ${now.difference(asked).inSeconds}s');
          _slowTries++;
          _reconnectTunnel();
          unawaited(socket.close());
        }

        welcome = _welcomeTimer = Timer(welcomeWithin, welcomeDue);
        // Read until it closes, or the try is given up on (then closed).
        final done = Completer<void>();
        final reading = socket.listen(
          (frame) {
            if (frame is List<int>) _receive(frame);
          },
          onError: (Object error, StackTrace stack) {
            if (!done.isCompleted) done.completeError(error, stack);
          },
          onDone: () {
            if (!done.isCompleted) done.complete();
          },
          cancelOnError: true,
        );
        try {
          await _orAbandoned(done.future);
        } finally {
          welcome?.cancel();
          if (!done.isCompleted) {
            unawaited(reading.cancel());
            unawaited(_let(socket));
          }
        }
        // Through a tunnel, what it brought meanwhile and when it last did:
        // a connection closed by its ping while bytes were still arriving
        // would say so (#413).
        String ago(DateTime? at) => at == null
            ? 'never'
            : '${DateTime.now().difference(at).inMilliseconds} ms ago';
        _log(
          'connection closed (code ${socket.closeCode})'
          '${via != null && via.tunnelled && !loopback ? '; the tunnel received ${(_tunnelReceived - receivedBefore) & 0xffffffff} bytes meanwhile, the last ${ago(_tunnelMovedAt)}; the link last ${ago(_tunnelLinkMovedAt)}' : ''}',
        );
        closeCode = socket.closeCode;
        why = ConnectionFailure.closed;
        // A live connection that dropped (unanswered pings, say): the
        // tunnel's connection under it is most likely dead too.
        if (connected.value &&
            !_closed &&
            via != null &&
            via.tunnelled &&
            !loopback &&
            !identical(_redialed, socket)) {
          _note(ConnectionKind.reconnect, {
            ConnectionKey.reason: ConnectionReason.dropped,
          });
          _reconnectTunnel();
          _watchBack();
        }
      } on HandshakeException catch (error) {
        _log("its certificate isn't the pinned one: $error");
        why = ConnectionFailure.handshake;
      } on IOException catch (error) {
        _log('connection failed: ${_withoutTunnelPath(error)}');
        why = ConnectionFailure.unreachable;
      } on TunnelOpenFailure catch (failure) {
        _log("couldn't open the tunnel: ${failure.message}");
        why = ConnectionFailure.tunnel;
      } on TimeoutException catch (timeout) {
        _log(
          'no connection within ${(timeout.duration ?? connectWithin).inSeconds}s',
        );
        why = ConnectionFailure.timeout;
        _slowTries++;
        _reconnectTunnel();
      } on _NotNow {
        // Said why already.
        why = ConnectionFailure.other;
      } on _Abandoned {
        // For a connection made from scratch, which said so: at once.
        abandoned = true;
        why = ConnectionFailure.abandoned;
      }
      final began = _welcomedAt ?? _tryStarted;
      _note(ConnectionKind.disconnected, {
        ConnectionKey.attempt: tryNumber,
        ConnectionKey.why: refused.value != null
            ? ConnectionFailure.refused
            : signInRequired.value != null
            ? ConnectionFailure.signin
            : why,
        ConnectionKey.welcomed: _welcomed,
        ConnectionKey.code: ?closeCode,
      }, began == null ? Duration.zero : DateTime.now().difference(began));
      _welcomedAt = null;
      // This Mac's own through the tunnel never got welcomed: the open
      // port next, with its code's token.
      if (local != null && !loopback && !_welcomed) _loopbackNext = true;
      _disconnected();
      if (!_welcomed) _triedQuietly(tryNumber);
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
      if (target != _target || abandoned) continue;
      // In the background, nothing tries again until the app is back
      // (resumed wakes it): no new connection, nor endpoint, meanwhile.
      // Not on screen, its quick tries done, it backs off (#454).
      if (!_onScreen &&
          !_welcomed &&
          (_awayDelay > Duration.zero || _retryDelay >= retryMost)) {
        final next = _awayDelay * 2;
        _awayDelay = next < retryMost * 2
            ? retryMost * 2
            : next > awayRetryMost
            ? awayRetryMost
            : next;
        _log('not on screen: next try in ${_awayDelay.inSeconds}s at most');
      }
      final failedAt = DateTime.now();
      if (_parked) {
        _log('in the background: trying again once the app is back');
        await _wake.future;
      } else {
        final due = Completer<void>();
        final timer = Timer(_retryDelay, due.complete);
        await Future.any([due.future, _wake.future]);
        timer.cancel();
      }
      await _awayWait(failedAt);
      final doubled = _retryDelay * 2;
      _retryDelay = doubled > retryMost ? retryMost : doubled;
    }
  }

  /// The next try at this Mac's own Factory goes to its open port, with
  /// its pairing code's token: the tunnel to it just failed.
  var _loopbackNext = false;

  /// Whether the current connection was welcomed.
  var _welcomed = false;

  /// How the current try goes, when it began and when it was welcomed:
  /// for [diagnostics].
  var _tryRoute = ConnectionRoute.local;
  DateTime? _tryStarted;
  DateTime? _welcomedAt;

  /// The connection's Welcome timeout, cancelled while it waits to be
  /// allowed.
  Timer? _welcomeTimer;

  /// What the current connection's Hello was sent to, and with which
  /// device key: the pairing number is worked out from them.
  RemoteFactory? _helloVia;
  String? _helloKey;

  static const _signInToConnect =
      'Sign in to your Claktory account on this device to connect.';

  /// The endpoint id of the Claktory this connects to, through the tunnel
  /// (this Mac's own's too, once it's read its code); empty otherwise.
  String get endpointId => (_target ?? _helloVia)?.endpointId ?? '';

  /// The current connection's Hello: its refusal is the connection's.
  var _helloId = 0;

  /// The device assertion the current connection's Hello showed; empty if
  /// none.
  String _helloAssertion = '';

  /// This PC's own Claktory on Windows: its app-token read, the
  /// connection opened, and the token kept for Hello only if the server end
  /// of that connection runs as this same Windows account. Otherwise the
  /// connection closes with nothing sent, and the next try starts again.
  Future<WebSocket> _windowsLoopback(WindowsLocal local) async {
    final token = await local.readToken(dev: localPort != '7420');
    if (token == null) {
      throw const SocketException("This PC's Claktory has no app-token yet");
    }
    final connecting = connectLoopback(localEndpoint);
    final (socket, ownPort) = await connecting.timeout(
      _connectWindow(null),
      onTimeout: () {
        unawaited(
          connecting.then((late) => late.$1.close(), onError: (Object _) {}),
        );
        throw TimeoutException('no connection', _connectWindow(null));
      },
    );
    final serverPort = Uri.parse(localEndpoint).port;
    bool ours;
    try {
      ours = await local.sameUser(serverPort, ownPort);
    } on Object {
      ours = false;
    }
    if (!ours) {
      _log("this PC's Claktory port is held by another account: nothing sent");
      await _let(socket);
      throw const SocketException(
        "This PC's Claktory port belongs to another account",
      );
    }
    _localAppToken = token;
    return socket;
  }

  /// [connecting], or a [TimeoutException] after [_connectWindow] (with
  /// the tunnel it goes [through]); one that connects after all is closed,
  /// so no connection given up on lingers at the other end beside the
  /// next.
  Future<WebSocket> _connectWithin(
    Future<WebSocket> connecting, {
    Tunnel? through,
  }) {
    final within = _connectWindow(through);
    return connecting.timeout(
      within,
      onTimeout: () {
        unawaited(
          connecting.then((late) => late.close(), onError: (Object _) {}),
        );
        throw TimeoutException('no connection', within);
      },
    );
  }

  /// How long a Hello waits for this device's assertion, once its
  /// connection is open; the request itself carries on, for the next try.
  Duration assertionWithin = const Duration(seconds: 5);

  /// Assertions on their way, by device key and endpoint: one request each,
  /// however many wait for it.
  final _assertionsAhead = <String, Future<String?>>{};

  /// [target]'s assertion as [key], asked for now or joining the request
  /// already on its way ([_assertionFor]).
  Future<String?> _assertionAhead(RemoteFactory target, String key) {
    final id = '$key/${target.endpointId}';
    final ahead = _assertionsAhead[id];
    if (ahead != null) return ahead;
    final asked = _assertionFor(target, key);
    _assertionsAhead[id] = asked;
    unawaited(asked.whenComplete(() => _assertionsAhead.remove(id)));
    return asked;
  }

  /// Gets the assertion for the Claktory this connects to on its way, so
  /// a connection made afresh needn't wait for claktory.com: the app back
  /// in front, the network moved. The account keeps it until shortly
  /// before it expires.
  void _prefetchAssertion() {
    final target = _target;
    if (_closed || target == null || !target.tunnelled) return;
    if (!(account?.signedIn ?? false)) return;
    unawaited(_assertionAhead(target, deviceKey(target)));
  }

  /// A device assertion for [target], reached as the device key [key]:
  /// for the tunnel key that key makes. Empty when this device isn't
  /// signed in to its account or the service refuses one; null when it
  /// can't be had now (claktory.com out of reach, or failing), to try
  /// again.
  Future<String?> _assertionFor(RemoteFactory target, String key) async {
    final account = this.account;
    if (account == null || !account.signedIn) return '';
    try {
      final ids = await _tunnels.pairingIds(
        deviceKey: key,
        endpointId: target.endpointId,
      );
      return await account.assertionFor(ids.device);
    } on AccountServiceError catch (error) {
      _log("couldn't get a device assertion: $error");
      return error.status >= 500 ? null : '';
    } on AccountServiceUnreachable catch (error) {
      _log("couldn't get a device assertion: $error");
      return null;
    } catch (error) {
      _log("couldn't get a device assertion: $error");
      return null;
    }
  }

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
          // Found through the account, the assertion stands in for a
          // code's token.
          token: pending.discovered ? _helloAssertion : via.token,
        );
      } catch (error) {
        _log("can't work out the pairing number: $error");
      }
    }
    if (_closed || !identical(via, _helloVia)) return;
    pairing.value = PairingWait(pending, number);
  }

  /// Cancel on the waiting page (#603): takes back this device's request
  /// on that Claktory, on the connection that made it (the backend knows it
  /// by the key it came with), so it leaves the list there at once. That
  /// Claktory then closes the connection, which the caller is leaving
  /// anyway. Best effort: unsent, the request runs out there as before.
  void withdrawPairing() {
    if (pairing.value == null || refused.value != null) return;
    try {
      _send(
        ClientMessage(id: Int64(_nextId++), withdrawPairing: WithdrawPairing()),
      );
    } on StateError {
      // Closed already: it runs out there.
    }
  }

  /// Connects to [target] instead (null for this Mac's Factory). What the
  /// old one showed goes at once, so nothing of it shows as the new one's.
  void connectTo(RemoteFactory? target) {
    _target = target;
    _relaysSaid = const [];
    lastFailure.value = null;
    _setAside = null;
    // Decisions made on the last one aren't this one's.
    _whenBack.clear();
    _selecting = null;
    diagnostics?.select(
      target == null
          ? 'local'
          : target.endpointId.isNotEmpty
          ? target.endpointId
          : '${target.host}_${target.port}',
    );
    refused.value = null;
    signInRequired.value = null;
    otherAccount.value = null;
    confirmBy.value = null;
    pairing.value = null;
    movedAway.value = null;
    _retryDelay = _minRetryDelay;
    // Unknown until this one's Workspace arrives, not empty.
    workspace.clear();
    _workspaceRevision = '';
    move.value = null;
    highlights.forgetKnown();
    transcript.load(Transcript());
    pipeChat.load(PipeTranscript());
    chatTranscript.load(Transcript());
    chatQueued.apply(QueuedMessages());
    extensionView.apply(ExtensionView());
    queued.apply(QueuedMessages());
    unawaited(_socket?.close());
    if (target != _tunnelTarget) unawaited(_closeTunnel());
    _wakeUp();
  }

  /// [_qaStallResume]'s next try, after coming back.
  var _qaStallNext = false;

  /// How long a tunnel has to open: it only listens on this device and
  /// makes its endpoint, so one that takes longer is stuck (#214).
  Duration tunnelOpenWithin = const Duration(seconds: 15);

  /// The tunnel to [target], opened the first time, as [key] (else this
  /// device's key for it).
  Future<Tunnel> _tunnelTo(RemoteFactory target, {String? key}) async {
    if (_qaStallNext) {
      _qaStallNext = false;
      _log('QA: this try hangs for good');
      return Completer<Tunnel>().future;
    }
    final closing = _tunnelClosing;
    if (closing != null) {
      _tunnelClosing = null;
      try {
        await closing;
      } catch (error) {
        _log("the old tunnel didn't close cleanly: $error");
      }
    }
    final open = _tunnel;
    if (open != null && _tunnelTarget == target) return open;
    await _closeTunnel();
    // Something under the tunnels may be stuck (a runtime whose tasks no
    // longer run, as after the app came back from suspension): asked
    // first, and made afresh if it doesn't answer, as relaunching would.
    if (_checkRuntime) {
      _checkRuntime = false;
      if (!await _runtime.answers(runtimeAnswerWithin)) {
        _log("the tunnels' runtime didn't answer: starting it afresh");
        _note(ConnectionKind.reconnect, {
          ConnectionKey.reason: ConnectionReason.runtime,
        });
        _runtime.fresh();
      }
    }
    final relays = _relaysSaid.isNotEmpty ? _relaysSaid : target.relays;
    final Tunnel opened;
    try {
      final opening = _tunnels.open(
        deviceKey: key ?? deviceKey(target),
        endpointId: target.endpointId,
        relays: relays,
        addresses: target.addresses,
      );
      opened = await opening.timeout(
        tunnelOpenWithin,
        onTimeout: () {
          // One that opens after all is closed, so it never stays on the
          // relay as this device beside the next.
          unawaited(
            opening.then((late) => late.close(), onError: (Object _) {}),
          );
          _checkRuntime = true;
          throw TimeoutException(
            'not open within ${tunnelOpenWithin.inSeconds}s',
          );
        },
      );
    } catch (error) {
      tunnel.value = TunnelStatus(
        phase: TunnelPhase.failed,
        path: TunnelPath.none,
        error: "This Mac couldn't start the private tunnel: $error",
        failure: TunnelFailure.other,
        relayConnected: false,
        relayUpMs: 0,
        relayTrouble: TunnelRelayTrouble.none,
        received: 0,
        linkReceived: 0,
        dialTimeoutMs: 0,
        reach: TunnelReach.none,
      );
      throw TunnelOpenFailure('$error');
    }
    // Closed while it opened: nothing would ever close it, and it would
    // stay on the relay as this device beside the one that replaced it.
    // Quietly: this connection is going, so there's nothing to show.
    if (_closed) {
      await opened.close();
      throw const _NotNow();
    }
    _tunnel = opened;
    _relaysTold = relays;
    _tunnelTarget = target;
    _tunnelServed = false;
    return opened;
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
    // Bytes arriving through the tunnel: the link is delivering, if
    // slowly, so the next ping waits from now, as it does for a whole
    // frame ([_receive]). A pong stuck behind a Workspace still on its way
    // no longer closes the connection bringing it (#413: on one or two
    // bars, every 12 s, for ever); a link where nothing arrives is still
    // closed within [_pingEvery] twice and a second.
    final now = DateTime.now();
    if (status.received != _tunnelReceived) {
      _tunnelReceived = status.received;
      _tunnelMovedAt = now;
      _socket?.pingInterval = _ping;
    }
    // On a weak link the connection can hold its data back for longer than
    // the ping allows (QUIC backing off behind a deep queue), while its
    // own traffic still arrives: alive too, as long as data came lately.
    // A link where nothing arrives still closes within the ping's time;
    // one whose data stopped for good, after [_transferPause] (Sale's 63
    // kbps runs, #413).
    if (status.linkReceived != _tunnelLinkReceived) {
      _tunnelLinkReceived = status.linkReceived;
      _tunnelLinkMovedAt = now;
      final data = _tunnelMovedAt;
      if (data != null && now.difference(data) < _transferPause) {
        _socket?.pingInterval = _ping;
      }
    }
    final shown = tunnel.value;
    if (shown != null &&
        shown.phase == status.phase &&
        shown.path == status.path &&
        shown.reach == status.reach &&
        shown.error == status.error &&
        shown.failure == status.failure &&
        shown.relayConnected == status.relayConnected &&
        shown.relayTrouble == status.relayTrouble) {
      return;
    }
    // Up: how long it took (since the tunnel's endpoint was made, or the
    // relay was lost). Down: why, roughly, and again when that changes
    // (#388); a tunnel just opened whose relay isn't up yet, with nothing
    // wrong, says nothing until it's up (Zucchero: every launch began
    // with a bare up=0).
    final trouble = _relayTrouble(status.relayTrouble);
    if (((shown != null || status.relayConnected) &&
            shown?.relayConnected != status.relayConnected) ||
        (!status.relayConnected &&
            trouble != null &&
            shown?.relayTrouble != status.relayTrouble)) {
      _note(
        ConnectionKind.relay,
        {
          ConnectionKey.up: status.relayConnected,
          if (!status.relayConnected && trouble != null)
            ConnectionKey.err: trouble,
        },
        status.relayConnected
            ? Duration(milliseconds: status.relayUpMs)
            : Duration.zero,
      );
    }
    if ((shown?.path != status.path || shown?.reach != status.reach) &&
        status.path != TunnelPath.none) {
      _note(ConnectionKind.path, {
        ConnectionKey.route: _pathRoute(status.path),
        ConnectionKey.reach: ?_reach(status.reach),
      });
    }
    if (status.relayConnected) {
      relayDownSince.value = null;
    } else if (status.relayTrouble != TunnelRelayTrouble.none) {
      relayDownSince.value ??= DateTime.now();
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
    // relayDownSince stays: the next tunnel's relay, the same one, is
    // likely still out of reach, and since when is what it says. The
    // first status with the relay up clears it.
    // Its forwards go with it.
    if (open != null) {
      await forwards.stopAll().timeout(
        const Duration(seconds: 3),
        onTimeout: () => _log("the tunnel's forwards didn't stop within 3s"),
      );
    }
    // Waited for, so no new one opens while it's still there: the relay
    // would see this device's key twice. The tunnel bounds its own close
    // (its sockets may be dead, taken back while the app was away); this
    // only keeps the app from waiting forever on one that doesn't, and
    // has the runtime under it asked before the next opens.
    await open?.close().timeout(
      const Duration(seconds: 10),
      onTimeout: () {
        _log("the old tunnel didn't close within 10s");
        _checkRuntime = true;
      },
    );
  }

  /// Reconnects at once, when the person asks (Reconnect, or a tap on
  /// Reconnecting…): from scratch, as relaunching the app would (#214).
  void reconnectNow() {
    if (_closed) return;
    if (refused.value != null || signInRequired.value != null) {
      return retry();
    }
    _backNext = null;
    _freshFailure();
    reconnectFromScratch('asked to');
  }

  /// A quiet try is due past [signInRequired] (probeSignIn).
  var _probing = false;

  /// Turned away until this device signs in ([signInRequired]), asks the
  /// Claktory once more without leaving the sign-in screen: it may have
  /// let this device in meanwhile (added again in its Settings → Devices,
  /// or signed in to its account again), and then it connects by itself
  /// (Sale's row 189). Turned away again, the screen stays as it was.
  /// Nothing to ask when this device must sign in first (one found
  /// through the account), nor off screen.
  void probeSignIn() {
    final target = _target;
    if (_closed ||
        _suspended ||
        !_onScreen ||
        _probing ||
        signInRequired.value == null ||
        refused.value != null ||
        (target != null &&
            target.fromAccount &&
            !(account?.signedIn ?? false))) {
      return;
    }
    _log('turned away until signed in: asking again');
    _probing = true;
    _wakeUp();
  }

  /// After a refusal, tries the same Factory again: after signing in, say.
  void retry() {
    refused.value = null;
    signInRequired.value = null;
    otherAccount.value = null;
    _retryDelay = _minRetryDelay;
    _wakeUp();
  }

  void _wakeUp() {
    if (!_wake.isCompleted) _wake.complete();
    _wake = Completer<void>();
  }

  Future<void> close() async {
    _android?.dispose();
    _closed = true;
    _idleTimer?.cancel();
    if (_followingInput) UserInput.instance.removeListener(_touched);
    _diagnosticsTimer?.cancel();
    _tunnelBack?.cancel();
    _backWatch?.cancel();
    if (!_abandon.isCompleted) _abandon.complete();
    account = null;
    unawaited(_notified.close());
    unawaited(_browser.close());
    unawaited(_historyPushes.close());
    _quiet?.cancel();
    _sleepWatch?.cancel();
    unawaited(_network?.cancel());
    deviceNetwork.removeListener(_networkKnown);
    deviceNetwork.removeListener(_watchBlocked);
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

  /// Shows [agentId] at once, from what this app has (its conversation
  /// fills in as it arrives), and asks the backend to select it. Cut off by
  /// the connection, it's asked again once the connection is back, the
  /// latest choice only, and shown meanwhile: a click while a connection
  /// died under it was lost, and the view stayed on the agent before
  /// (Matt's MacBook, Bruno 36927).
  Future<void> selectAgent(String agentId) async {
    final shown = workspace.workspace.selectedAgentId;
    _selecting = agentId;
    if (shown != agentId && workspace.agent(agentId) != null) {
      workspace.apply(_withSelection(workspace.workspace.deepCopy()));
    }
    try {
      await _request(ClientMessage(selectAgent: SelectAgent(agentId: agentId)));
    } on RequestFailure catch (failure) {
      // It goes again once the connection is back.
      if (connectionLost(failure) && _selecting == agentId) return;
      if (_selecting == agentId) {
        _selecting = null;
        // Refused: what the backend has selected shows again.
        if (workspace.workspace.selectedAgentId == agentId) {
          workspace.apply(
            workspace.workspace.deepCopy()..selectedAgentId = shown,
          );
        }
      }
      rethrow;
    }
    if (_selecting == agentId) _selecting = null;
  }

  /// The agent [selectAgent] asked for, until the backend has selected it:
  /// what's shown, whatever a Workspace from before says.
  String? _selecting;

  /// [workspace] with the agent being selected shown as selected, if
  /// it's there.
  Workspace _withSelection(Workspace workspace) {
    final selecting = _selecting;
    if (selecting == null || workspace.selectedAgentId == selecting) {
      return workspace;
    }
    if (!workspace.agents.any((agent) => agent.id == selecting)) {
      // Gone meanwhile: nothing to select.
      _selecting = null;
      return workspace;
    }
    return workspace..selectedAgentId = selecting;
  }

  /// The selection cut off by the connection, asked for again.
  void _selectAgain() {
    final selecting = _selecting;
    if (selecting == null) return;
    unawaited(
      selectAgent(selecting).catchError(
        (Object error) => _log('selection sent again: $error'),
        test: (error) => error is RequestFailure,
      ),
    );
  }

  /// The person opened [paneId] of [agentId]'s layout: it's the newest now,
  /// on every device (its opened time).
  Future<void> paneOpened(String agentId, String paneId) => _request(
    ClientMessage(
      paneOpened: PaneOpened(agentId: agentId, paneId: paneId),
    ),
  );

  /// Sends the person's message; [clientId] comes back on the queued
  /// message and the entry it becomes (see [outbox]). A reply on a
  /// checklist item names it: [checklistItem], on [checklistAgentId]'s
  /// checklist (the receiver's own when empty).
  Future<void> sendMessage(
    String agentId,
    String text, {
    String clientId = '',
    List<Attachment> attachments = const [],
    bool voice = false,
    int checklistItem = 0,
    String checklistAgentId = '',
  }) => _request(
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

  /// Finds [query] in agent [agentId]'s whole conversation (or chat
  /// [chatId]'s, or pipe [pipeId]'s messages): the entries it's in, newest
  /// first, [limit] at a time, older than [beforeSeq] when given.
  Future<ConversationMatches> searchConversation({
    String agentId = '',
    String chatId = '',
    String pipeId = '',
    required String query,
    int? beforeSeq,
    int limit = 100,
  }) async {
    final reply = await _call(
      ClientMessage(
        searchConversation: SearchConversation(
          agentId: agentId,
          chatId: chatId,
          pipeId: pipeId,
          query: query,
          beforeSeq: beforeSeq == null ? null : Int64(beforeSeq),
          limit: limit,
        ),
      ),
    );
    if (!reply.hasConversationMatches()) {
      throw const RequestFailure('Claktory sent no matches.');
    }
    return reply.conversationMatches;
  }

  /// Reports reply [seq] of agent [agentId]'s conversation (or chat
  /// [chatId]'s) to Claktory's team. Without [send] nothing leaves this
  /// computer: it answers what would be sent, for the person to see; with
  /// it, it answers once claktory.com has it.
  Future<ReplyReport> reportReply({
    String agentId = '',
    String chatId = '',
    required int seq,
    String note = '',
    bool send = false,
  }) async {
    final reply = await _call(
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
    if (!reply.hasReplyReport()) {
      throw const RequestFailure('Claktory sent no report.');
    }
    return reply.replyReport;
  }

  /// Starts (or picks up) downloading the speech model voice messages
  /// need; its progress comes in `Workspace.speech_model`.
  Future<void> installSpeechModel() =>
      _request(ClientMessage(installSpeechModel: InstallSpeechModel()));

  /// Deletes the speech model, or stops its download.
  Future<void> removeSpeechModel() =>
      _request(ClientMessage(removeSpeechModel: RemoveSpeechModel()));

  /// Transcribes a voice message that failed again; [messageId] is its
  /// `QueuedMessage.id`.
  Future<void> retryVoiceMessage(String agentId, Int64 messageId) => _request(
    ClientMessage(
      retryVoiceMessage: RetryVoiceMessage(
        agentId: agentId,
        messageId: messageId,
      ),
    ),
  );

  /// Drops a voice message waiting to be transcribed, or that failed,
  /// undelivered.
  Future<void> discardVoiceMessage(String agentId, Int64 messageId) => _request(
    ClientMessage(
      discardVoiceMessage: DiscardVoiceMessage(
        agentId: agentId,
        messageId: messageId,
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
  /// the backend has. It goes in pieces sized to the link ([pacedUpload],
  /// #375), so the connection's pings aren't held up behind it.
  Future<int> uploadChunk(String uploadId, int offset, List<int> data) =>
      pacedUpload(
        send: (offset, piece) => _uploadPiece(uploadId, offset, piece),
        pace: _upPace,
        offset: offset,
        data: data,
      );

  Future<int> _uploadPiece(String uploadId, int offset, List<int> data) async {
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
  /// with the message. With [sha256] (hex), the backend checks the file
  /// against it and refuses it if it differs.
  Future<Attachment> finishUpload(String uploadId, {String sha256 = ''}) async {
    final reply = await _call(
      ClientMessage(
        finishUpload: FinishUpload(uploadId: uploadId, sha256: sha256),
      ),
    );
    if (!reply.hasUploadedFile()) {
      throw const RequestFailure('Claktory sent nothing back for the file.');
    }
    return reply.uploadedFile.attachment;
  }

  /// Starts uploading an ink blob as [uploadId] (UPLOAD_PURPOSE_INK: no
  /// agent; kept privately, by its SHA-256), or picks it up where it
  /// stopped: answers how many of its bytes the backend has. Its chunks go
  /// as any upload's; finishInkUpload ends it.
  Future<int> startInkUpload(
    String uploadId, {
    required String name,
    required int size,
    required String mime,
  }) async {
    final reply = await _call(
      ClientMessage(
        startUpload: StartUpload(
          uploadId: uploadId,
          name: name,
          size: Int64(size),
          mime: mime,
          purpose: UploadPurpose.UPLOAD_PURPOSE_INK,
        ),
      ),
    );
    return reply.uploadProgress.received.toInt();
  }

  /// An ink blob's upload is all there: answers the SHA-256 that names it
  /// from now on, once it's stored (the backend checks the bytes against
  /// [sha256] and refuses it if they differ).
  Future<String> finishInkUpload(
    String uploadId, {
    required String sha256,
  }) async {
    final reply = await _call(
      ClientMessage(
        finishUpload: FinishUpload(uploadId: uploadId, sha256: sha256),
      ),
    );
    final blob = reply.hasUploadedFile() ? reply.uploadedFile.blobSha256 : '';
    if (blob.isEmpty) {
      throw const RequestFailure('Claktory sent nothing back for the drawing.');
    }
    return blob;
  }

  /// Part of an ink blob a drawing or review item refers to, from
  /// [offset] (at most 1 MiB); `error` "unknown" when nothing does.
  Future<InkBlobData> readInkBlob(String sha256, {int offset = 0}) async {
    final reply = await _call(
      ClientMessage(
        readInkBlob: ReadInkBlob(sha256: sha256, offset: Int64(offset)),
      ),
    );
    if (!reply.hasInkBlobData()) {
      throw const RequestFailure('Claktory sent no drawing.');
    }
    return reply.inkBlobData;
  }

  /// The ink layers on [path], every version's at its latest revision
  /// (also in [inkLayerLists]).
  Future<InkLayerList> listInkLayers(String path) async {
    final reply = await _call(
      ClientMessage(listInkLayers: ListInkLayers(path: path)),
    );
    if (!reply.hasInkLayerList()) {
      throw const RequestFailure('Claktory sent no drawings.');
    }
    return reply.inkLayerList;
  }

  /// Saves a drawing as its ink layer's next revision (or a new layer):
  /// answers the revision saved, or the layer as another device saved it
  /// first (`conflict`), nothing overwritten.
  Future<InkLayerSaved> saveInkLayer(SaveInkLayer save) async {
    final reply = await _call(ClientMessage(saveInkLayer: save));
    if (!reply.hasInkLayerSaved()) {
      throw const RequestFailure('Claktory sent nothing back for the drawing.');
    }
    return reply.inkLayerSaved;
  }

  /// Deletes an ink layer at its latest revision, [baseRevision].
  Future<void> deleteInkLayer(String inkLayerId, int baseRevision) => _request(
    ClientMessage(
      deleteInkLayer: DeleteInkLayer(
        inkLayerId: inkLayerId,
        baseRevision: baseRevision,
      ),
    ),
  );

  /// Drops what the backend has of [uploadId].
  Future<void> cancelUpload(String uploadId) =>
      _request(ClientMessage(cancelUpload: CancelUpload(uploadId: uploadId)));

  /// Starts uploading a dictation as [uploadId] (UPLOAD_PURPOSE_DICTATION: a
  /// recording for no agent, AAC in .m4a, at most 10 minutes and 16 MiB),
  /// or picks it up where it stopped: answers how many of its bytes the
  /// backend has. Its chunks go as any upload's (uploadChunk);
  /// finishDictationUpload ends it, and transcribeAudio hears it.
  Future<int> startDictationUpload(
    String uploadId, {
    required int size,
    String name = 'dictation.m4a',
    String mime = 'audio/mp4',
  }) async {
    final reply = await _call(
      ClientMessage(
        startUpload: StartUpload(
          uploadId: uploadId,
          name: name,
          size: Int64(size),
          mime: mime,
          purpose: UploadPurpose.UPLOAD_PURPOSE_DICTATION,
        ),
      ),
    );
    return reply.uploadProgress.received.toInt();
  }

  /// A dictation's upload is all there (checked against [sha256], hex, when
  /// given): the backend keeps it, privately, for transcribeAudio.
  Future<void> finishDictationUpload(
    String uploadId, {
    String sha256 = '',
  }) async {
    final reply = await _call(
      ClientMessage(
        finishUpload: FinishUpload(uploadId: uploadId, sha256: sha256),
      ),
    );
    if (!reply.hasUploadedFile()) {
      throw const RequestFailure(
        'Claktory sent nothing back for the recording.',
      );
    }
  }

  /// Transcribes the dictation uploaded as [uploadId], for the person to
  /// edit: completes with what was heard (AudioTranscribed: `text`, or
  /// `error` no_model, unsupported, too_long or failed). Nothing goes to an
  /// agent, and the backend deletes the recording either way. Throws
  /// [RequestFailure] when the request is refused, or the connection drops
  /// before the answer: asked again with the same ids, the backend attaches
  /// to the job or answers what it heard (for about an hour), or says
  /// no_recording when it has neither (upload it again, with new ids).
  Future<AudioTranscribed> transcribeAudio({
    required String requestId,
    required String uploadId,
  }) async {
    final heard = Completer<AudioTranscribed>();
    _dictations[requestId] = heard;
    try {
      await _request(
        ClientMessage(
          transcribeAudio: TranscribeAudio(
            requestId: requestId,
            uploadId: uploadId,
          ),
        ),
      );
    } catch (_) {
      _dictations.remove(requestId);
      rethrow;
    }
    return heard.future;
  }

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

  /// [agentId]'s entry [seq], as it is now, whichever agent shows: what
  /// Needs you shows of a card waiting in a conversation that isn't open.
  /// Null when the backend has no such entry.
  Future<Entry?> fetchEntry(String agentId, int seq) async {
    final reply = await _call(
      ClientMessage(
        loadEntries: LoadEntries(
          agentId: agentId,
          beforeSeq: Int64(seq + 1),
          fromSeq: Int64(seq),
        ),
      ),
    );
    for (final entry in reply.entryPage.entries) {
      if (entry.seq.toInt() == seq) return entry;
    }
    return null;
  }

  /// The person's answer to [agentId]'s question at entry [seq]: [text]
  /// (needed when it asks for one; else an optional comment with the
  /// approval), or [dismiss] it, leaving it to the agent's judgment.
  /// [attachments], uploaded for the agent, go with either: the pages the
  /// person drew on, answering a review of a PDF.
  ///
  /// With [reply] (#370), [text] is the person's reply, neither a yes nor a
  /// no: sent as a dismissal too, so a Claktory from before Submit tells
  /// the agent their comment, never "yes, go ahead" (Ghiaccio's rule).
  Future<void> answerAsk(
    String agentId,
    int seq, {
    String text = '',
    bool dismiss = false,
    bool reply = false,
    bool resolve = false,
    AskAnsweredFrom from = AskAnsweredFrom.ASK_ANSWERED_FROM_UNSPECIFIED,
    List<Attachment> attachments = const [],
    int? step,
    int? revision,
    int? talkSeen,
    bool confirmed = false,
  }) => _request(
    ClientMessage(
      answerAsk: AnswerAsk(
        agentId: agentId,
        seq: Int64(seq),
        text: text,
        dismiss: dismiss || reply,
        reply: reply,
        // Ends a card with a thread, deciding nothing.
        resolve: resolve,
        // Where the person answered it: their conversation, or Needs you.
        from: from,
        attachments: attachments,
        // A procedure's step (0-based): Done, or with [dismiss], Can't do
        // this.
        step: step,
        revision: _revision(revision),
        // How much of the card's talk the person saw (#492): an approval
        // after a reply they didn't see is refused.
        talkSeen: talkSeen,
        // A command card's Execute (#494): the person saw the command and
        // tapped Execute, and its confirm when it warns. Never sent from
        // anywhere that doesn't show the command.
        confirmed: confirmed ? true : null,
      ),
    ),
  );

  Future<void> stopAgent(String agentId) =>
      _request(ClientMessage(stopAgent: StopAgent(agentId: agentId)));

  /// Has [agentId] (an agent or a chat) waiting out its usage limits ask
  /// again now: its provider reads its accounts' usage again, and every
  /// agent waiting on it asks. Refused when it isn't waiting anymore.
  Future<void> tryAgainNow(String agentId) =>
      _request(ClientMessage(tryAgainNow: TryAgainNow(agentId: agentId)));

  /// Moves [agentId] (an agent or a chat) waiting on its provider's usage
  /// to its alternative (Agent.usage_wait_alternative), and with
  /// [allWaiting] every agent and chat waiting on that provider, each to
  /// its own: only while as many others wait as the person was shown
  /// ([expectedOthers]), else refused and none moves. Refused, with a
  /// sentence to show, when there's none.
  Future<void> continueOnAlternative(
    String agentId, {
    bool allWaiting = false,
    int expectedOthers = 0,
  }) => _request(
    ClientMessage(
      continueOnAlternative: ContinueOnAlternative(
        agentId: agentId,
        allWaiting: allWaiting,
        expectedOthers: expectedOthers,
      ),
    ),
  );

  /// Puts [agentId] (an agent or a chat) back on the model it had before
  /// it continued elsewhere (Agent.usage_switch), and with [all] every one
  /// switched from that provider that's ready: only while as many others
  /// are as the person was shown ([expectedOthers]).
  Future<void> switchBack(
    String agentId, {
    bool all = false,
    int expectedOthers = 0,
  }) => _request(
    ClientMessage(
      switchBack: SwitchBack(
        agentId: agentId,
        all: all,
        expectedOthers: expectedOthers,
      ),
    ),
  );

  /// Puts [chatId] on [model] (an id from Workspace.models), from its next
  /// model call.
  Future<void> setChatModel(String chatId, String model) => _request(
    ClientMessage(
      setChatModel: SetChatModel(chatId: chatId, model: model),
    ),
  );

  /// Turns Continue automatically on or off (Workspace.usage_auto_continue,
  /// friend bug #8 A2).
  Future<void> setUsageAutoContinue(bool on) => _request(
    ClientMessage(setUsageAutoContinue: SetUsageAutoContinue(on: on)),
  );

  /// Approves or dismisses [agentId]'s proposal to Rivet. Approving sends
  /// Rivet [comment] and the proposal as the person's message; the entry
  /// comes back decided. Refused when it's already decided, or there's no
  /// Rivet that can run.
  /// A chat's proposal names the chat ([chatId]) instead of an agent.
  ///
  /// With [reply] (#370), [comment] is the person's answer for Rivet to act
  /// on, never an approval: sent as not approved, for a Claktory from
  /// before Submit.
  Future<void> decideProposal(
    String agentId,
    String proposalId, {
    required bool approve,
    String comment = '',
    String chatId = '',
    bool reply = false,
    int? revision,
  }) => _request(
    ClientMessage(
      decideProposal: DecideProposal(
        agentId: chatId.isEmpty ? agentId : '',
        chatId: chatId,
        proposalId: proposalId,
        approve: approve && !reply,
        comment: comment,
        reply: reply,
        revision: _revision(revision),
      ),
    ),
  );

  /// The [revision] of a card (#428) as the person saw it, to answer that
  /// version only: refused, "This card changed. Look at it again.", when
  /// its agent has changed it since. None from a Claktory before #428
  /// (its cards are 0): then the answer is for the card as it is.
  static int? _revision(int? revision) =>
      revision == null || revision <= 0 ? null : revision;

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

  /// Every agent's monitors (`ListMonitors`), only [agentId]'s, those in
  /// [projectId] or in [states] when given; by agent, then number.
  Future<MonitorList> listMonitors({
    String agentId = '',
    String projectId = '',
    List<MonitorState> states = const [],
  }) async {
    final reply = await _call(
      ClientMessage(
        listMonitors: ListMonitors(
          agentId: agentId,
          projectId: projectId,
          states: states,
        ),
      ),
    );
    if (!reply.hasMonitorList()) {
      throw const RequestFailure("Couldn't list the monitors. Try again.");
    }
    return reply.monitorList;
  }

  /// What happened to [agentId]'s monitor [monitorId], newest first: a
  /// page of the backend's 50, older than [beforeSeq] when set.
  Future<MonitorHistory> monitorHistory(
    String agentId,
    int monitorId, {
    Int64 beforeSeq = Int64.ZERO,
  }) async {
    final reply = await _call(
      ClientMessage(
        listMonitorHistory: ListMonitorHistory(
          agentId: agentId,
          monitorId: monitorId,
          beforeSeq: beforeSeq,
        ),
      ),
    );
    if (!reply.hasMonitorHistory()) {
      throw const RequestFailure("Couldn't read its history. Try again.");
    }
    return reply.monitorHistory;
  }

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

  /// Reads [providerId]'s model list from the provider now (empty: every
  /// provider that can be read), answered once Workspace has the new list
  /// (friend bug #14). Its failure is a sentence to show.
  Future<void> refreshModels([String providerId = '']) => _request(
    ClientMessage(refreshModels: RefreshModels(providerId: providerId)),
  );

  /// Settings → Providers opened: Claktory reads again each model list and
  /// key's credit that's gone stale, and Workspace shows them as they come
  /// (ProvidersShown). Asked each time it opens; it costs nothing.
  Future<void> providersShown() =>
      _request(ClientMessage(providersShown: ProvidersShown()));

  /// Adds [model] (`provider/model id`) to Workspace.models once its
  /// provider says this sign-in has it and an agent can run on it; done
  /// already when it's there. A refusal is a sentence to show.
  Future<void> addModel(String model) =>
      _request(ClientMessage(addModel: AddModel(model: model)));

  /// Puts Claktory's background service back in charge of a backend
  /// started by hand (friend bug #24, BackendService.can_restore): acked,
  /// then this backend stops and the service starts it again. A refusal
  /// is a sentence to show.
  Future<void> restoreBackendService() =>
      _request(ClientMessage(restoreBackendService: RestoreBackendService()));

  /// Stops Claktory on this Mac (friend bug #27, BackendService.can_stop):
  /// [quitting], as the app quits, only when Settings.when_app_quits says
  /// so; else the person's switch, and it stays off until they start it.
  /// Acked, then the connection closes. A refusal is a sentence to show.
  Future<void> stopEngine({required bool quitting}) =>
      _request(ClientMessage(stopEngine: StopEngine(quitting: quitting)));

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

  /// With [askEachTime], every command that uses it waits for the
  /// person's Allow.
  Future<void> createSecret(
    String name,
    String description,
    List<SecretEntry> entries, {
    bool askEachTime = false,
    List<String> sites = const [],
  }) => _request(
    ClientMessage(
      createSecret: CreateSecret(
        name: name,
        description: description,
        entries: entries,
        askEachTime: askEachTime,
        sites: sites,
      ),
    ),
  );

  /// A recovery-codes card's codes (#435 W8b), to this device only, while
  /// the card at [seq] waits. Kept only by the card on screen, never in
  /// the Workspace. Refused (gone), the reason is in words to show.
  Future<List<String>> showRecoveryCodes(String agentId, int seq) async {
    final reply = await _call(
      ClientMessage(
        showRecoveryCodes: ShowRecoveryCodes(agentId: agentId, seq: Int64(seq)),
      ),
    );
    if (!reply.hasRecoveryCodes()) {
      throw const RequestFailure('Claktory sent no codes.');
    }
    return reply.recoveryCodes.codes.toList();
  }

  /// Keep in Secrets on a recovery-codes card (#435 W8b): a new secret
  /// given to no agent, and the card ends. Refused (the vault locked), the
  /// reason is in words; the card still waits.
  Future<void> keepRecoveryCodes(String agentId, int seq) => _request(
    ClientMessage(
      keepRecoveryCodes: KeepRecoveryCodes(agentId: agentId, seq: Int64(seq)),
    ),
  );

  /// The person read these alerts (Workspace.alerts): they opened one, or
  /// chose Mark as read. Each leaves Needs you for History.
  Future<void> readAlerts(List<String> ids) =>
      _request(ClientMessage(readAlerts: ReadAlerts(ids: ids)));

  /// Where Claktory's browser may fill a secret in for an errand (#435):
  /// exact https origins, replacing the list; empty, it's filled nowhere.
  /// Refused, the reason is in words to show as it is.
  Future<void> setSecretSites(String id, List<String> sites) => _request(
    ClientMessage(
      setSecretSites: SetSecretSites(id: id, sites: sites),
    ),
  );

  /// Adds a goal of the person's, in their words, to [agentId]'s
  /// checklist.
  Future<void> addGoal(String agentId, String text) => _request(
    ClientMessage(
      addGoal: AddGoal(agentId: agentId, text: text),
    ),
  );

  /// Changes goal [goalId] on [agentId]'s checklist: its words, its status
  /// (done, dropped, open again), or with [remove] takes it off (its items
  /// stay, serving no goal).
  Future<void> updateGoal(
    String agentId,
    int goalId, {
    String? text,
    ChecklistStatus? status,
    bool remove = false,
  }) => _request(
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

  /// Whether every command that uses secret [id] waits for the person's
  /// Allow. Turning it off needs the vault open, as giving a secret does.
  Future<void> setSecretAskEachTime(String id, bool on) => _request(
    ClientMessage(
      setSecretAskEachTime: SetSecretAskEachTime(id: id, askEachTime: on),
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

  /// Gives a secret to an agent; with [fillOnly], only to fill in on its
  /// sites in Claktory's browser (#435), replacing a full grant, as a full
  /// one replaces it.
  Future<void> grantSecret(
    String id,
    String agentId, {
    bool fillOnly = false,
  }) => _request(
    ClientMessage(
      grantSecret: GrantSecret(id: id, agentId: agentId, fillOnly: fillOnly),
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
  /// approving it needs neither. With [approveOnly], approves the need
  /// instead, and the request's admin agent finds or collects the secret.
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
    bool reply = false,
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
        approveOnly: approveOnly,
        askEachTime: askEachTime,
        passwordLength: passwordLength,
        deleteOriginals: deleteOriginals,
        grantToAsker: grantToAsker,
        reply: reply,
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
    bool reply = false,
  }) => _request(
    ClientMessage(
      dismissConnectProposal: DismissConnectProposal(
        proposalId: proposalId,
        comment: comment,
        reply: reply,
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

  /// What the person already answered, dismissed, approved or denied in
  /// the last 30 days (Needs you's History), newest first, to this device
  /// only.
  Future<NeedsYouHistory> listNeedsYouHistory(ListNeedsYouHistory ask) async {
    final reply = await _call(ClientMessage(listNeedsYouHistory: ask));
    if (!reply.hasNeedsYouHistory()) {
      throw const RequestFailure('Claktory sent no history.');
    }
    return reply.needsYouHistory;
  }

  /// Undoes a recorded change to an app's access (NeedsYouRecord's
  /// app_change_id, AppAccessGiven's change_id): what's still as it left
  /// it goes back; the answer says what changed since and was left.
  Future<AppAccessUndone> undoAppAccessChange(String changeId) async {
    final reply = await _call(
      ClientMessage(
        undoAppAccessChange: UndoAppAccessChange(changeId: changeId),
      ),
    );
    if (!reply.hasAppAccessUndone()) {
      throw const RequestFailure(
        "Couldn't tell whether it was undone. Check Marketplace → Apps.",
      );
    }
    return reply.appAccessUndone;
  }

  /// The person added a generated key where it goes (#360 K6): the agent
  /// is told it's in place.
  Future<void> secretRequestDone(String agentId, String requestId) => _request(
    ClientMessage(
      secretRequestDone: SecretRequestDone(
        agentId: agentId,
        requestId: requestId,
      ),
    ),
  );

  /// Takes [agentId]'s conversation back to just before entry [seq] (on the
  /// agent's reply, to the end of the run it belongs to); what came after
  /// is kept as another version (friend bug #9). [timestampMs] is the
  /// entry's, so a conversation that changed meanwhile is refused rather
  /// than cut in the wrong place.
  Future<void> forkConversation(String agentId, Int64 seq, Int64 timestampMs) =>
      _request(
        ClientMessage(
          forkConversation: ForkConversation(
            agentId: agentId,
            seq: seq,
            timestampMs: timestampMs,
          ),
        ),
      );

  /// [agentId]'s other versions of its conversation.
  Future<Branches> listBranches(String agentId) async {
    final reply = await _call(
      ClientMessage(listBranches: ListBranches(agentId: agentId)),
    );
    if (!reply.hasBranches()) {
      throw const RequestFailure("Couldn't list the versions. Try again.");
    }
    return reply.branches;
  }

  /// The whole history of this Claktory's shape (#458): every node and
  /// edge that ever was, with when, the messages per hour and the
  /// chapters. Asked again when [historyChanged] moves.
  Future<HistoryOverview> historyOverview({String project = ''}) async {
    final reply = await _call(
      ClientMessage(getHistoryOverview: GetHistoryOverview(projectId: project)),
    );
    if (!reply.hasHistoryOverview()) {
      throw const RequestFailure("Couldn't load the history. Try again.");
    }
    return reply.historyOverview;
  }

  /// A page of the history's events after [cursor] (from the start when
  /// empty), up to [untilMs] when given; only [project]'s when given, a
  /// page then holding fewer, even none, with more to ask for.
  Future<HistoryEvents> historyEvents({
    List<int> cursor = const [],
    Int64? untilMs,
    int limit = 0,
    bool runs = true,
    String project = '',
  }) async {
    final reply = await _call(
      ClientMessage(
        getHistoryEvents: GetHistoryEvents(
          afterCursor: cursor,
          untilMs: untilMs,
          limit: limit,
          includeRuns: runs,
          projectId: project,
        ),
      ),
    );
    if (!reply.hasHistoryEvents()) {
      throw const RequestFailure("Couldn't load the history. Try again.");
    }
    return reply.historyEvents;
  }

  /// While [on], new events come on [historyPushes] and a changed overview
  /// moves [historyChanged].
  Future<void> watchHistory(bool on, {String project = ''}) => _request(
    ClientMessage(
      watchHistory: WatchHistory(on: on, projectId: project),
    ),
  );

  /// New events while History is watched (WatchHistory).
  Stream<HistoryEvents> get historyPushes => _historyPushes.stream;
  final _historyPushes = StreamController<HistoryEvents>.broadcast(sync: true);

  /// The history's version, from the backend's word that its overview
  /// changed (HistoryChanged).
  final historyChanged = ValueNotifier<Int64>(Int64.ZERO);

  /// Makes [branchId] [agentId]'s conversation; the one it has now is kept
  /// as another version.
  Future<void> switchBranch(String agentId, String branchId) => _request(
    ClientMessage(
      switchBranch: SwitchBranch(agentId: agentId, branchId: branchId),
    ),
  );

  /// Deletes [agentId]'s other version [branchId], for good.
  Future<void> deleteBranch(String agentId, String branchId) => _request(
    ClientMessage(
      deleteBranch: DeleteBranch(agentId: agentId, branchId: branchId),
    ),
  );

  /// The CSV and TSV files [appId] could be given (Add a file…), newest
  /// first, those whose names hold every word of [query].
  Future<AppFiles> listAppFiles(String appId, {String query = ''}) async {
    final reply = await _call(
      ClientMessage(
        listAppFiles: ListAppFiles(appId: appId, query: query),
      ),
    );
    if (!reply.hasAppFiles()) {
      throw const RequestFailure("Couldn't list the files. Try again.");
    }
    return reply.appFiles;
  }

  /// Undoes Rivet's change to an agent's model or effort (NeedsYouRecord's
  /// model_change_id): the model and effort it replaced come back, or
  /// Claktory says why not (changed since, undone already).
  Future<void> undoModelChange(String changeId) => _request(
    ClientMessage(undoModelChange: UndoModelChange(changeId: changeId)),
  );

  /// The quiet copies [agentId] was sent, newest first (`ListQuietCopies`):
  /// [limit] of them (the backend's 20 when 0), older than the copy
  /// [beforeId] when set. Listing them doesn't mark them read.
  Future<QuietCopies> listQuietCopies(
    String agentId, {
    int beforeId = 0,
    int limit = 0,
  }) async {
    final reply = await _call(
      ClientMessage(
        listQuietCopies: ListQuietCopies(
          agentId: agentId,
          beforeId: Int64(beforeId),
          limit: limit,
        ),
      ),
    );
    if (!reply.hasQuietCopies()) {
      throw const RequestFailure('Claktory sent no copies.');
    }
    return reply.quietCopies;
  }

  /// Installs an app from a .clapp's [content], or Claktory's own app
  /// [bundled] with this release: without [approve], what it can reach,
  /// for the person to review; approved, with that review's [sha256],
  /// installed (and null).
  Future<AppPermissions?> installLocalApp(
    List<int> content, {
    bool approve = false,
    String sha256 = '',
    String bundled = '',
  }) async {
    final message = ClientMessage(
      installLocalApp: InstallLocalApp(
        content: content,
        approve: approve,
        sha256: sha256,
        bundled: bundled,
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

  /// Lets app [appId] read its source [sourceId] (Claktory's records, an
  /// agent's database), or with [granted] false takes it back.
  Future<void> grantAppSource(
    String appId,
    String sourceId, {
    required bool granted,
  }) => _request(
    ClientMessage(
      grantAppSource: GrantAppSource(
        appId: appId,
        sourceId: sourceId,
        granted: granted,
      ),
    ),
  );

  /// Pins [appId] to the app dock at [position] among the pinned apps (0
  /// first; -1, or past the end, last), moving it if it's there already;
  /// with [pinned] false, takes it off the dock (Workspace.pinned_apps).
  Future<void> pinApp(String appId, {bool pinned = true, int position = -1}) =>
      _request(
        ClientMessage(
          pinApp: PinApp(appId: appId, pinned: pinned, position: position),
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

  /// The emailed code: signs this Claktory in. Typed on this device, it
  /// confirms this device as the owner's too (#365 R5), so a Claktory
  /// elsewhere no longer asks it to: the next Welcome says so again if not.
  Future<void> accountVerify(String code) async {
    final target = _target;
    await _request(ClientMessage(accountVerify: AccountVerify(code: code)));
    if (target != null && target == _target) confirmBy.value = null;
  }

  /// Emails a new code to the same address.
  Future<void> accountResend() =>
      _request(ClientMessage(accountResend: AccountResend()));

  Future<void> accountSignOut() =>
      _request(ClientMessage(accountSignOut: AccountSignOut()));

  /// Drops a sign-in to another account still waiting for its code (Switch
  /// account…), leaving the one signed in as it was (#529, V6).
  Future<void> accountSignInCancel() =>
      _request(ClientMessage(accountSignInCancel: AccountSignInCancel()));

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

  /// Asks which group of the recovery key the person types back to show
  /// they saved it (R1): a fresh one each time, for this device only.
  Future<RecoveryKeyCheck> checkRecoveryKey() async {
    final reply = await _call(
      ClientMessage(checkRecoveryKey: CheckRecoveryKey()),
    );
    if (!reply.hasRecoveryKeyCheck()) {
      throw const RequestFailure('Claktory sent no group to check.');
    }
    return reply.recoveryKeyCheck;
  }

  /// The group the person typed back for [checkId]: right, and
  /// VaultState.recovery_key_unconfirmed goes on every device.
  Future<void> confirmRecoveryKey(String checkId, String typed) => _request(
    ClientMessage(
      confirmRecoveryKey: ConfirmRecoveryKey(checkId: checkId, typed: typed),
    ),
  );

  /// Seals this vault to the account's new recovery key (#239), which this
  /// Claktory couldn't check by itself (VaultState.recovery_key_change_
  /// waiting): only the person, on this computer or a device confirmed as
  /// theirs. A refusal is a sentence to show.
  Future<void> useRecoveryKey() =>
      _request(ClientMessage(useRecoveryKey: UseRecoveryKey()));

  /// Empties the build cache at [path].
  Future<void> clearBuildCache(String path) =>
      _request(ClientMessage(clearBuildCache: ClearBuildCache(path: path)));

  /// Checks for a newer release now (CheckForUpdates); what it finds
  /// comes in Workspace.update_check.
  Future<void> checkForUpdates() =>
      _request(ClientMessage(checkForUpdates: CheckForUpdates()));

  /// Installs the latest release. With [appPath], the backend replaces the app
  /// bundle there too, once this app quits.
  Future<void> installUpdate(String appPath) =>
      _request(ClientMessage(installUpdate: InstallUpdate(appPath: appPath)));

  /// A piece of release [version]'s app archive from [offset], to update
  /// this app from a Factory on another machine: up to [maxBytes] (0: as
  /// much as it sends at once, 1 MiB).
  Future<AppPiece> fetchApp(
    String version,
    int offset, {
    int maxBytes = 0,
  }) async {
    final reply = await _call(
      ClientMessage(
        fetchApp: FetchApp(
          version: version,
          offset: Int64(offset),
          maxBytes: Int64(maxBytes),
        ),
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

  /// Needs you's focus mode on or off for this device (#514): kept by the
  /// backend, in Settings.focus_mode by [focusDeviceId].
  Future<void> setFocusMode(bool on) =>
      _request(ClientMessage(setFocusMode: SetFocusMode(on: on)));

  /// This device in Settings.focus_mode: its paired id, or "local" for
  /// this computer's own app.
  String get focusDeviceId => deviceId.isEmpty ? 'local' : deviceId;

  /// Follows [target]'s conversation (an agent's, a chat's or a pipe's)
  /// on this connection without selecting it, into [watchedTranscript],
  /// [watchedQueued] and [watchedPipe]; null stops. Asked again on the
  /// next connection, as the backend forgets it.
  Future<void> watchTranscript(WatchTranscript? target) {
    _watch = target;
    return _request(
      ClientMessage(watchTranscript: target ?? WatchTranscript()),
    );
  }

  /// [message] is the watched conversation's (ServerMessage.watched):
  /// into its own stores, never the selected one's. False for anything
  /// else so tagged.
  bool _watched(ServerMessage message) {
    switch (message.whichBody()) {
      case ServerMessage_Body.transcript:
        watchedTranscript.load(message.transcript);
      case ServerMessage_Body.entry:
        watchedTranscript.update(message.entry);
      case ServerMessage_Body.delta:
        watchedTranscript.delta(message.delta);
      case ServerMessage_Body.queuedMessages:
        watchedQueued.apply(message.queuedMessages);
      case ServerMessage_Body.chatTranscript:
        final chat = message.chatTranscript;
        watchedTranscript.load(
          Transcript(
            agentId: chat.chatId,
            entries: chat.entries,
            hasEarlier: chat.hasEarlier,
          ),
        );
      case ServerMessage_Body.chatEntry:
        watchedTranscript.update(
          EntryUpdate(
            agentId: message.chatEntry.chatId,
            entry: message.chatEntry.entry,
          ),
        );
      case ServerMessage_Body.chatDelta:
        final delta = message.chatDelta;
        watchedTranscript.delta(
          EntryDelta(agentId: delta.chatId, seq: delta.seq, text: delta.text),
        );
      case ServerMessage_Body.chatQueuedMessages:
        watchedQueued.apply(
          QueuedMessages(
            agentId: message.chatQueuedMessages.chatId,
            messages: message.chatQueuedMessages.messages,
          ),
        );
      case ServerMessage_Body.pipeTranscript:
        watchedPipe.load(message.pipeTranscript);
      default:
        return false;
    }
    return true;
  }

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

  /// Keep this Mac awake while agents work, or not (Workspace.keep_awake):
  /// the person's answer, from Settings or Needs you.
  Future<void> setKeepAwake(bool on) =>
      _request(ClientMessage(setKeepAwake: SetKeepAwake(on: on)));

  /// Turns keeping agents working with the lid closed on or off (#371);
  /// turning it on works only from the Mac app on that Mac.
  Future<void> setLidClosed({required bool on}) =>
      _request(ClientMessage(setLidClosed: SetLidClosed(on: on)));

  /// An agent asked to show [request]'s screen: the window showing this
  /// Claktory opens it if it can, and the backend hears whether it did.
  void screenRequested(OpenScreen request) {
    final opened = showScreen?.call(request) ?? false;
    fireAndLog(screenOpened(request.id, opened));
  }

  /// Answers an OpenScreen: whether this device opened the screen.
  Future<void> screenOpened(String id, bool opened) => _request(
    ClientMessage(
      screenOpened: ScreenOpened(id: id, opened: opened),
    ),
  );

  /// Zooms the whole app on this device ([zoom] 1 is actual size): kept
  /// for this device only (`Settings.zoom`, see zoom.dart).
  Future<void> setZoom(double zoom) =>
      _request(ClientMessage(setZoom: SetZoom(zoom: zoom)));

  /// How much of this Claktory's files this device keeps for its viewers
  /// ([bytes]; 0 puts back the default, 1 GB): kept for this device only
  /// (`Settings.file_cache`, see viewers/file_cache.dart).
  Future<void> setFileCache(int bytes) =>
      _request(ClientMessage(setFileCache: SetFileCache(bytes: Int64(bytes))));

  /// Gives the person's avatar new colors (Workspace.person), the same on
  /// every device: [choice], one of Person.avatar_choices, or else others
  /// at random.
  Future<void> shuffleAvatar({int? choice}) =>
      _request(ClientMessage(shuffleAvatar: ShuffleAvatar(choice: choice)));

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

  /// The code of the agent's pairing [pairingId] (DevicePairing.id) while
  /// it waits, to show on its card; the same code each time it's asked.
  /// Refused with ERROR_CODE_NOT_FOUND once it no longer waits.
  Future<PairingCode> showPairingCode(String pairingId) async {
    final reply = await _call(
      ClientMessage(showPairingCode: ShowPairingCode(pairingId: pairingId)),
    );
    if (!reply.hasPairingCode()) {
      throw const RequestFailure('Claktory sent no pairing code.');
    }
    return reply.pairingCode;
  }

  /// Skips connecting a model in a new Claktory's first run
  /// (`Settings.onboarding`), on every device.
  Future<void> skipModelStep() =>
      _request(ClientMessage(setOnboarding: SetOnboarding(skipModel: true)));

  // ---- Pipes (#196) ----

  /// Makes an invite for a pipe from [projectId]: the pipe shows as
  /// INVITED, and the code comes back only here, once. [startId] is the
  /// agent's card it's made from (PipeStart), which it closes.
  Future<PipeInvite> createPipeInvite(
    String projectId,
    String name,
    String purpose, {
    String startId = '',
  }) async {
    final reply = await _call(
      ClientMessage(
        createPipeInvite: CreatePipeInvite(
          projectId: projectId,
          name: name,
          purpose: purpose,
          startId: startId,
        ),
      ),
    );
    if (!reply.hasPipeInvite()) {
      throw const RequestFailure('Claktory sent no invite.');
    }
    return reply.pipeInvite;
  }

  /// What a pipe invite [code] says, without using it.
  Future<PipeInvitePreview> readPipeInvite(String code) async {
    final reply = await _call(
      ClientMessage(readPipeInvite: ReadPipeInvite(code: code)),
    );
    if (!reply.hasPipeInvitePreview()) {
      throw const RequestFailure("Claktory couldn't read that code.");
    }
    return reply.pipeInvitePreview;
  }

  /// Uses an invite [code] for a pipe to [projectId]; [startId] is the
  /// agent's card it's used from (PipeStart), which it closes.
  Future<void> acceptPipeInvite(
    String code,
    String projectId, {
    String name = '',
    String purpose = '',
    String startId = '',
  }) => _request(
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

  /// Closes [agentId]'s card [startId] suggesting a pipe, unused.
  Future<void> dismissPipeStart(
    String agentId,
    String startId, {
    String comment = '',
  }) => _request(
    ClientMessage(
      dismissPipeStart: DismissPipeStart(
        agentId: agentId,
        startId: startId,
        comment: comment,
      ),
    ),
  );

  /// The person's decision on a pipe waiting for their approval.
  Future<void> approvePipe(ApprovePipe approve) =>
      _request(ClientMessage(approvePipe: approve));

  Future<void> updatePipe(UpdatePipe update) =>
      _request(ClientMessage(updatePipe: update));

  /// A new pipe group at the end of [projectId]'s (#359), [pipeId] moved
  /// into it when given.
  Future<void> createPipeGroup(
    String projectId,
    String name, {
    String pipeId = '',
  }) => _request(
    ClientMessage(
      createPipeGroup: CreatePipeGroup(
        projectId: projectId,
        name: name,
        pipeId: pipeId,
      ),
    ),
  );

  Future<void> renamePipeGroup(String groupId, String name) => _request(
    ClientMessage(
      renamePipeGroup: RenamePipeGroup(groupId: groupId, name: name),
    ),
  );

  /// Pins [agentId] under Rivet in the sidebar before the pin [beforeId]
  /// (empty: last), or moves its pin there (Settings.pinned_agents).
  /// Refused, the reason is in words to show as it is.
  Future<void> pinAgent(String agentId, {String beforeId = ''}) => _request(
    ClientMessage(
      pinAgent: PinAgent(agentId: agentId, beforeId: beforeId),
    ),
  );

  /// Unpins [agentId]; one not pinned is no change.
  Future<void> unpinAgent(String agentId) =>
      _request(ClientMessage(unpinAgent: UnpinAgent(agentId: agentId)));

  /// Has the backend read the places macOS protects at [roots]
  /// (ProtectedPlace.root) now, macOS asking the person for any it hasn't
  /// decided on; answered once read, the Workspace following when anything
  /// changed. The person's only.
  Future<void> checkProtectedPlaces(List<String> roots) => _request(
    ClientMessage(checkProtectedPlaces: CheckProtectedPlaces(roots: roots)),
  );

  /// The OK on a protected place's Needs you item: to History; the place
  /// stays refused.
  Future<void> dismissProtectedPlace(String root) => _request(
    ClientMessage(dismissProtectedPlace: DismissProtectedPlace(root: root)),
  );

  /// Deletes a pipe group; its pipes stay, outside any.
  Future<void> deletePipeGroup(String groupId) => _request(
    ClientMessage(deletePipeGroup: DeletePipeGroup(groupId: groupId)),
  );

  /// Puts [pipeId] in [groupId], or in none ("").
  Future<void> setPipeGroup(String pipeId, String groupId) => _request(
    ClientMessage(
      setPipeGroup: SetPipeGroup(pipeId: pipeId, groupId: groupId),
    ),
  );

  /// Shares a website on this computer with pipes or a group's
  /// (pipe-port-sharing.md); refused, why in the person's words ("Port 7420
  /// is Claktory's own.").
  Future<void> sharePipeWebsite(SharePipeWebsite share) =>
      _request(ClientMessage(sharePipeWebsite: share));

  /// Stops one of this side's shares on every pipe at once.
  Future<void> stopPipeShare(String shareId) =>
      _request(ClientMessage(stopPipeShare: StopPipeShare(shareId: shareId)));

  /// How long one of this side's shares lasts from now, in seconds; 0
  /// keeps it until it's stopped (friend bug #46).
  Future<void> setPipeShareExpiry(String shareId, int lastsS) => _request(
    ClientMessage(
      setPipeShareExpiry: SetPipeShareExpiry(
        shareId: shareId,
        lastsS: Int64(lastsS),
      ),
    ),
  );

  /// The one-time addresses that open a share another Claktory shares with
  /// this one in the browser, its server here started if it wasn't.
  Future<PipeShareOpened> openPipeShare(String shareId) async {
    final reply = await _call(
      ClientMessage(openPipeShare: OpenPipeShare(shareId: shareId)),
    );
    if (!reply.hasPipeShareOpened()) {
      throw const RequestFailure("Couldn't open it. Try again.");
    }
    return reply.pipeShareOpened;
  }

  /// Hides a share another Claktory shares with this one, or shows it.
  Future<void> hidePipeShare(String shareId, bool hidden) => _request(
    ClientMessage(
      hidePipeShare: HidePipeShare(shareId: shareId, hidden: hidden),
    ),
  );

  /// Whether [port] could be shared, and whether something answers on it.
  Future<PipeSharePort> checkPipeSharePort(int port) async {
    final reply = await _call(
      ClientMessage(checkPipeSharePort: CheckPipeSharePort(port: port)),
    );
    if (!reply.hasPipeSharePort()) {
      throw const RequestFailure("Couldn't check the port. Try again.");
    }
    return reply.pipeSharePort;
  }

  Future<void> setPipeGroupCollapsed(String groupId, bool collapsed) =>
      _request(
        ClientMessage(
          setPipeGroupCollapsed: SetPipeGroupCollapsed(
            groupId: groupId,
            collapsed: collapsed,
          ),
        ),
      );

  /// Accepts who [pipeId]'s other side is now, after a change
  /// (PipePeer.identity_change): pins it, and lets through what was held.
  /// [cardId]: accepted on an agent's card asking for it (#471 G7).
  Future<void> acceptPipeIdentity(String pipeId, {String cardId = ''}) =>
      _request(
        ClientMessage(
          acceptPipeIdentity: AcceptPipeIdentity(
            pipeId: pipeId,
            cardId: cardId,
          ),
        ),
      );

  Future<void> setPipeState(String pipeId, PipeAction action) => _request(
    ClientMessage(
      setPipeState: SetPipeState(pipeId: pipeId, action: action),
    ),
  );

  /// Shows [pipeId] in the main pane, on every device; its messages follow.
  Future<void> selectPipe(String pipeId) =>
      _request(ClientMessage(selectPipe: SelectPipe(pipeId: pipeId)));

  /// Makes a chat and shows it, on every device: with [text] as its first
  /// message, or empty (gone again once it's left with nothing said).
  Future<void> createChat({String text = '', String clientId = ''}) => _request(
    ClientMessage(
      createChat: CreateChat(text: text, clientId: clientId),
    ),
  );

  /// Shows [chatId] in the main pane, on every device; its conversation
  /// follows. The agent selected stays, to go back to.
  Future<void> selectChat(String chatId) =>
      _request(ClientMessage(selectChat: SelectChat(chatId: chatId)));

  /// The person writes in a chat; [clientId] comes back on the queued
  /// message, as for an agent.
  Future<void> sendChatMessage(
    String chatId,
    String text, {
    String clientId = '',
    List<Attachment> attachments = const [],
    bool voice = false,
  }) => _request(
    ClientMessage(
      sendChatMessage: SendChatMessage(
        chatId: chatId,
        text: text,
        clientId: clientId,
        attachments: attachments,
        voice: voice,
      ),
    ),
  );

  /// Stops a chat's reply, or its wait for usage.
  Future<void> stopChat(String chatId) =>
      _request(ClientMessage(stopChat: StopChat(chatId: chatId)));

  /// Deletes a chat and its conversation, for good.
  Future<void> deleteChat(String chatId) =>
      _request(ClientMessage(deleteChat: DeleteChat(chatId: chatId)));

  /// Starts moving this Claktory to the computer showing [code] (from
  /// `factoryd receive` there). Throws [RequestFailure] with a sentence for
  /// the person when it can't.
  Future<void> startMove(String code) =>
      _request(ClientMessage(startMove: StartMove(code: code)));

  /// The person sees the same digits on both computers.
  Future<void> confirmMove() =>
      _request(ClientMessage(confirmMove: ConfirmMove()));

  /// Calls the move off, until the switch; once it failed, dismisses it.
  Future<void> cancelMove() =>
      _request(ClientMessage(cancelMove: CancelMove()));

  /// Pauses the agents for the last of the copy now, rather than once the
  /// copy has caught up.
  Future<void> freezeMove() =>
      _request(ClientMessage(freezeMove: FreezeMove()));

  /// Names a chat; empty goes back to its automatic title.
  Future<void> renameChat(String chatId, String title) => _request(
    ClientMessage(
      renameChat: RenameChat(chatId: chatId, title: title),
    ),
  );

  /// Loads the entries before the first one [chatTranscript] holds, when
  /// it's [chatId]'s. Throws [RequestFailure] when it can't.
  Future<void> loadEarlierChat(String chatId) => _loadChatBefore(chatId);

  /// Loads every entry from [seq] up to the first one [chatTranscript]
  /// holds, when it's [chatId]'s.
  Future<void> loadChatFrom(String chatId, int seq) =>
      _loadChatBefore(chatId, from: seq);

  Future<void> _loadChatBefore(String chatId, {int? from}) async {
    final store = chatTranscript;
    if (store.chatId != chatId || store.length == 0) return;
    final loads = store.loads;
    final first = store[0].entry.seq;
    if (from != null && from >= first.toInt()) return;
    final reply = await _call(
      ClientMessage(
        loadChatEntries: LoadChatEntries(
          chatId: chatId,
          beforeSeq: first,
          fromSeq: from == null ? null : Int64(from),
        ),
      ),
    );
    if (store.loads != loads ||
        store.length == 0 ||
        store[0].entry.seq != first) {
      return;
    }
    final page = reply.chatEntryPage;
    store.prepend(page.chatId, page.entries, hasEarlier: page.hasEarlier);
  }

  /// [chatId]'s entry [seq] as it is now, whichever chat shows; null when
  /// the backend has no such entry.
  Future<Entry?> fetchChatEntry(String chatId, int seq) async {
    final reply = await _call(
      ClientMessage(
        loadChatEntries: LoadChatEntries(
          chatId: chatId,
          beforeSeq: Int64(seq + 1),
          fromSeq: Int64(seq),
        ),
      ),
    );
    for (final entry in reply.chatEntryPage.entries) {
      if (entry.seq.toInt() == seq) return entry;
    }
    return null;
  }

  /// The person writes in a pipe.
  /// Held ([holding]) until the pipe takes it: it has no outbox.
  Future<void> sendPipeMessage(String pipeId, String text) => holding(
    () => _request(
      ClientMessage(
        sendPipeMessage: SendPipeMessage(pipeId: pipeId, text: text),
      ),
    ),
  );

  /// Sends or discards a held message, or lets a held incoming one in;
  /// [reply], on one of this side's agents' (#370), sends it back to its
  /// writer with [comment] instead, unsent.
  Future<void> decidePipeMessage(
    String pipeId,
    int seq,
    bool allow, {
    String comment = '',
    bool reply = false,
  }) => _request(
    ClientMessage(
      decidePipeMessage: DecidePipeMessage(
        pipeId: pipeId,
        seq: Int64(seq),
        // A reply (#370) is never a yes, to an older Claktory too.
        allow: allow && !reply,
        comment: comment,
        reply: reply,
      ),
    ),
  );

  /// Loads the pipe's messages before the first one held.
  Future<void> loadPipeMessages(String pipeId) async {
    final chat = pipeChat;
    if (chat.pipeId != pipeId || chat.messages.isEmpty) return;
    final first = chat.messages.first.seq;
    final reply = await _call(
      ClientMessage(
        loadPipeMessages: LoadPipeMessages(pipeId: pipeId, beforeSeq: first),
      ),
    );
    if (reply.hasPipeMessagePage()) chat.prepend(reply.pipeMessagePage);
  }

  /// Loads pipe [pipeId]'s messages from [seq] up to the first one held, to
  /// go to an older one (a find's match).
  Future<void> loadPipeFrom(String pipeId, int seq) async {
    final chat = pipeChat;
    if (chat.pipeId != pipeId || chat.messages.isEmpty) return;
    final first = chat.messages.first.seq;
    if (seq >= first.toInt()) return;
    final reply = await _call(
      ClientMessage(
        loadPipeMessages: LoadPipeMessages(
          pipeId: pipeId,
          beforeSeq: first,
          fromSeq: Int64(seq),
        ),
      ),
    );
    if (reply.hasPipeMessagePage()) chat.prepend(reply.pipeMessagePage);
  }

  /// [pipeId]'s message [seq], as it is now, whichever pipe shows: what
  /// Needs you shows of a held message. Null when there's no such message.
  Future<PipeMessage?> fetchPipeMessage(String pipeId, int seq) async {
    final reply = await _call(
      ClientMessage(
        loadPipeMessages: LoadPipeMessages(
          pipeId: pipeId,
          beforeSeq: Int64(seq + 1),
          limit: 1,
        ),
      ),
    );
    for (final message in reply.pipeMessagePage.messages) {
      if (message.seq.toInt() == seq) return message;
    }
    return null;
  }

  /// Makes a liaison agent for a pipe: a member and its lead.
  Future<void> addPipeLiaison(String pipeId, String name) => _request(
    ClientMessage(
      addPipeLiaison: AddPipeLiaison(pipeId: pipeId, name: name),
    ),
  );

  /// The person's decision on an agent that read a pipe using a secret;
  /// [reply] (#370) doesn't allow it, and [comment] goes to the agent.
  Future<void> decideSecretApproval(
    String agentId,
    int seq,
    bool allow, {
    String comment = '',
    bool reply = false,
  }) => _request(
    ClientMessage(
      decideSecretApproval: DecideSecretApproval(
        agentId: agentId,
        seq: Int64(seq),
        // A reply (#370) is never a yes, to an older Claktory too.
        allow: allow && !reply,
        comment: comment,
        reply: reply,
      ),
    ),
  );

  /// The person's decision on Rivet's proposal at [seq] in [agentId]'s
  /// conversation to give an agent access to an app (AppGrantProposal):
  /// approved, the account makes the grant; declined, [comment] goes to
  /// Rivet. Refused while the account can't be reached; the card keeps
  /// waiting.
  Future<void> decideAppGrant(
    String agentId,
    int seq, {
    required bool approve,
    String comment = '',
    bool reply = false,
  }) => _request(
    ClientMessage(
      decideAppGrant: DecideAppGrant(
        agentId: agentId,
        seq: Int64(seq),
        approve: approve,
        comment: comment,
        reply: reply,
      ),
    ),
  );

  /// The person's decision on Rivet's proposal at [seq] in [agentId]'s
  /// conversation of what an app may read and which agents may use it
  /// (AppAccessProposal): approved, exactly what the card showed ([digest]);
  /// either way [comment] goes to Rivet.
  Future<void> decideAppAccess(
    String agentId,
    int seq, {
    required bool approve,
    required String digest,
    String comment = '',
    bool reply = false,
  }) => _request(
    ClientMessage(
      decideAppAccess: DecideAppAccess(
        agentId: agentId,
        seq: Int64(seq),
        approve: approve,
        comment: comment,
        digest: approve ? digest : '',
        reply: reply,
      ),
    ),
  );

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
  /// read comes back with `error` set. It comes in pieces sized to the link
  /// ([pacedRead], #375), [onProgress] told as each comes, and carries on
  /// after the connection drops and comes back; [carryOn] can stop it
  /// after a piece, [onFirst] after the first. With [preview] (this
  /// screen's size), a smaller copy made for it may come instead, saying
  /// so (`FileContent.preview`, #379); with [previewPage] too, that page
  /// of a PDF as an image.
  Future<FileContent> readFile(
    String path, {
    int maxBytes = 0,
    int offset = 0,
    String expectedSha256 = '',
    ReadProgress? onProgress,
    ReadOnward? carryOn,
    ReadFirst? onFirst,
    PreviewBox? preview,
    int previewPage = 0,
  }) => pacedRead(
    range: (offset, maxBytes, expected) => _readRange(
      path,
      maxBytes: maxBytes,
      offset: offset,
      expectedSha256: expected,
      preview: preview,
      previewPage: previewPage,
    ),
    connected: connected,
    pace: _downPace,
    maxBytes: maxBytes,
    offset: offset,
    expectedSha256: expectedSha256,
    onProgress: onProgress,
    carryOn: carryOn,
    onFirst: onFirst,
  );

  /// About how long [bytes] of a file would take to come over this link,
  /// as its reads so far have measured it; null before one has (#379).
  Duration? readTime(int bytes) => _downPace.estimate(bytes);

  /// How big a piece of a file to ask for, and to send, on this link.
  final _downPace = Pace();
  final _upPace = Pace();

  /// One ReadFile.
  Future<FileContent> _readRange(
    String path, {
    required int maxBytes,
    required int offset,
    required String expectedSha256,
    PreviewBox? preview,
    int previewPage = 0,
  }) async {
    final asked = DateTime.now();
    final reply = await _call(
      ClientMessage(
        readFile: ReadFile(
          path: path,
          maxBytes: Int64(maxBytes),
          offset: Int64(offset),
          expectedSha256: expectedSha256,
          previewWidth: preview?.width,
          previewHeight: preview?.height,
          previewPage: previewPage == 0 ? null : previewPage,
        ),
      ),
    );
    if (!reply.hasFileContent()) {
      throw const RequestFailure('Claktory sent no file.');
    }
    if (_qaDownlinkKbps > 0 || _qaRoundTripMs > 0) {
      await _qaSlowRead(asked, reply.fileContent.data.length);
    }
    return reply.fileContent;
  }

  /// [_qaDownlinkKbps]' link: a read of [bytes] asked for at [asked] is
  /// held until it would have come on that link, which reads at the same
  /// time share, one after the other, as a real one's would.
  static Future<void> _qaSlowRead(DateTime asked, int bytes) async {
    final reached = asked.add(Duration(milliseconds: _qaRoundTripMs));
    final free = _qaLinkFree.isAfter(reached) ? _qaLinkFree : reached;
    final arrives = free.add(
      Duration(
        milliseconds: _qaDownlinkKbps > 0 ? bytes * 8 ~/ _qaDownlinkKbps : 0,
      ),
    );
    _qaLinkFree = arrives;
    final wait = arrives.difference(DateTime.now());
    if (wait > Duration.zero) await Future<void>.delayed(wait);
  }

  /// When [_qaSlowRead]'s link has sent what it was given.
  static var _qaLinkFree = DateTime.fromMillisecondsSinceEpoch(0);

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
  /// it; either resolves its thread. [reply] (#370) does neither: [comment]
  /// goes into the thread, which stays open.
  Future<void> decideSuggestion(
    String threadId, {
    required bool accept,
    String comment = '',
    bool reply = false,
  }) => _request(
    ClientMessage(
      decideSuggestion: DecideSuggestion(
        threadId: threadId,
        // A reply (#370) is never an accept, to an older Claktory too.
        accept: accept && !reply,
        comment: comment,
        reply: reply,
      ),
    ),
  );

  /// Lays out [agentId]'s panes; null (or a lone chat leaf) clears it.
  /// [focus] expands that pane to fill the window ("" collapses it); left
  /// out, the one expanded stays so while it's still in [root].
  Future<void> setLayout(String agentId, ViewNode? root, {String? focus}) =>
      _request(
        ClientMessage(
          setLayout: SetLayout(agentId: agentId, root: root, focus: focus),
        ),
      );

  /// Unpairs a device: it can't connect any more.
  Future<void> unpairDevice(String deviceId) =>
      _request(ClientMessage(unpairDevice: UnpairDevice(deviceId: deviceId)));

  /// Confirms the paired device [deviceId] as the person's own ("Confirm
  /// it's yours"), from this device, which must be confirmed itself.
  Future<void> confirmDevice(String deviceId) =>
      _request(ClientMessage(confirmDevice: ConfirmDevice(deviceId: deviceId)));

  /// Confirms this device as the owner's with [assertion], its account's
  /// word that it's signed in.
  Future<void> confirmThisDevice(String assertion) => _request(
    ClientMessage(confirmDevice: ConfirmDevice(assertion: assertion)),
  );

  /// This Mac's own Claktory, signed in to the person's account, asks it
  /// for a one-time code that signs this app in as a device with the
  /// account key whose public half is [accountKey] (base64url), #529. Only
  /// this computer's own app may ask; the refusals are [RequestFailure]s
  /// with their codes.
  Future<DeviceVouched> vouchDevice({
    required String accountKey,
    required String name,
    required String platform,
    required String version,
  }) async {
    final reply = await _call(
      ClientMessage(
        vouchDevice: VouchDevice(
          accountKey: accountKey,
          name: name,
          platform: platform,
          version: version,
        ),
      ),
    );
    if (!reply.hasDeviceVouched()) {
      throw const RequestFailure('Claktory sent no answer to the sign-in.');
    }
    return reply.deviceVouched;
  }

  /// Lets in the device waiting with [deviceId]'s code
  /// (RemoteAccess.pending_devices), with the [number] the person typed
  /// off its screen: it's paired from now on. A wrong number is refused,
  /// saying how many tries are left; the third turns the device away.
  Future<void> approveDevice(String deviceId, String number) => _request(
    ClientMessage(
      approveDevice: ApproveDevice(deviceId: deviceId, number: number),
    ),
  );

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
  /// A request whose reply only says it was done. A real `Future<void>`,
  /// not the reply's passed as one: a caller's catchError handler that
  /// returns nothing is then valid (on the reply's Future, it threw
  /// "The error handler of Future.catchError must return a value of the
  /// future's type" whenever the backend refused).
  Future<void> _request(ClientMessage message) async {
    try {
      await _call(message);
    } on RequestFailure catch (failure) {
      if (!_decisions.contains(message.whichBody()) ||
          !connectionLost(failure)) {
        rethrow;
      }
      // A decision on a card cut off by the connection (#401): it goes
      // again by itself once the connection is back, and the card says so.
      _whenBack.add(message.deepCopy()..clearId());
      throw const RequestFailure(decisionWaiting);
    }
  }

  /// Decisions on a card or a review, each about one waiting thing (a
  /// question, a proposal, a secret asked for…), so sent again after the
  /// connection dropped they're done once: a repeat is refused as already
  /// decided. What's typed with them goes along.
  static const _decisions = {
    ClientMessage_Body.answerAsk,
    ClientMessage_Body.decideProposal,
    ClientMessage_Body.decideSecretRequest,
    ClientMessage_Body.decideSecretApproval,
    ClientMessage_Body.decideAppGrant,
    ClientMessage_Body.decideAppAccess,
    ClientMessage_Body.decidePipeMessage,
    ClientMessage_Body.decideSuggestion,
    ClientMessage_Body.approvePipe,
    ClientMessage_Body.dismissConnectProposal,
    ClientMessage_Body.dismissPipeStart,
  };

  /// Decisions cut off by the connection, sent again once it's back; kept
  /// only in memory, and only for the Claktory they were made on.
  final _whenBack = <ClientMessage>[];

  void _sendWhenBack() {
    final waiting = [..._whenBack];
    _whenBack.clear();
    for (final message in waiting) {
      // Refused now (decided meanwhile, here or elsewhere): the card shows
      // what the Workspace says.
      unawaited(
        _request(message).catchError(
          (Object error) => _log('decision sent again: $error'),
          test: (error) => error is RequestFailure,
        ),
      );
    }
  }

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
    try {
      _send(message);
    } on StateError {
      // The socket closed before Claktory noticed (a timer's request, say):
      // as lost as one it hadn't answered.
      _pending.remove(id);
      return Future.error(const RequestFailure('Connection lost.'));
    }
    return completer.future;
  }

  void _send(ClientMessage message) {
    _socket?.add(message.writeToBuffer());
    _traffic();
  }

  /// How often the connection is checked: a ping after this long with
  /// nothing heard, and closed when its pong isn't back this long after.
  static const _pingEvery = Duration(seconds: 5);

  /// [_pingEvery] while the app is idle ([_idle]).
  static const _pingIdle = Duration(seconds: 25);

  /// How long a phone or tablet goes with no input ([UserInput]), nothing
  /// arriving or sent and no request waiting for its answer before it's
  /// idle.
  @visibleForTesting
  static Duration idleAfter = const Duration(seconds: 60);

  /// Whether this device rests its connection when idle: phones and
  /// tablets, where keeping the radio awake is the cost.
  @visibleForTesting
  static bool idleOnThisDevice = Platform.isIOS || Platform.isAndroid;

  /// Idle (Polpo's idle-keepalive.md, Matt's call): the connection is
  /// checked every [_pingIdle], and the tunnel's paths every 25 s, instead
  /// of every few seconds, so an open phone left alone lets its radio rest;
  /// a link that died silently meanwhile is noticed within about a minute.
  /// A touch, a key or the app coming back ends it at once, and the tunnel
  /// checks the link then. What arrives meanwhile doesn't: it keeps the
  /// link checked by itself.
  var _idle = false;

  /// When something last went either way on the connection.
  var _lastTraffic = DateTime.now();

  /// Due when the app may next be idle.
  Timer? _idleTimer;

  var _followingInput = false;

  /// The connection's ping now.
  Duration get _ping => _idle ? _pingIdle : _pingEvery;

  /// The ping the connection has now: [_pingIdle] while idle.
  @visibleForTesting
  Duration? get pingInterval => _socket?.pingInterval;

  /// Something went either way: the app may be idle [idleAfter] from now.
  void _traffic() {
    _lastTraffic = DateTime.now();
    _idleLater();
  }

  /// Makes sure [_idleDue] looks again: when the last traffic or input is
  /// [idleAfter] old, or [after].
  void _idleLater({Duration? after}) {
    if (!idleOnThisDevice || _idle || _closed || _idleTimer != null) return;
    if (!_followingInput) {
      _followingInput = true;
      UserInput.instance
        ..watch()
        ..addListener(_touched);
    }
    final input = UserInput.instance.last;
    final last = input.isAfter(_lastTraffic) ? input : _lastTraffic;
    _idleTimer = Timer(
      after ?? last.add(idleAfter).difference(DateTime.now()),
      _idleDue,
    );
  }

  void _idleDue() {
    _idleTimer = null;
    if (_closed || _idle) return;
    final input = UserInput.instance.last;
    final last = input.isAfter(_lastTraffic) ? input : _lastTraffic;
    if (DateTime.now().difference(last) < idleAfter) return _idleLater();
    // An answer on its way, or not connected: not idle, so look again
    // later; neither counts as traffic.
    if (_pending.isNotEmpty || !connected.value) {
      return _idleLater(after: idleAfter);
    }
    _idle = true;
    _socket?.pingInterval = _pingIdle;
    _tunnel?.setIdle(true);
  }

  /// The person did something: idle no longer.
  void _touched() {
    if (_idle) _leaveIdle();
  }

  void _leaveIdle() {
    _idle = false;
    _socket?.pingInterval = _pingEvery;
    _tunnel?.setIdle(false);
    _idleLater();
  }

  void _receive(List<int> frame) {
    // Heard from: the link is alive, so the next ping waits from now. A
    // pong queued behind a long frame no longer closes a connection that's
    // busy bringing it (#375).
    _socket?.pingInterval = _ping;
    _traffic();
    final ServerMessage message;
    try {
      message = ServerMessage.fromBuffer(frame);
    } on InvalidProtocolBufferException catch (error) {
      _log('undecodable frame: ${error.message}');
      return;
    }
    if (message.watched && _watched(message)) return;
    switch (message.whichBody()) {
      case ServerMessage_Body.welcome:
        _welcomed = true;
        // A connection made afresh starts awake: its tunnel may be new.
        if (_idle) _leaveIdle();
        _slowTries = 0;
        // Back: the tunnel's time to bring it back is over.
        _tunnelBack?.cancel();
        _tunnelBack = null;
        _backWatch?.cancel();
        _backWatch = null;
        _backNext = null;
        _retryDelay = _minRetryDelay;
        _awayDelay = Duration.zero;
        _quiet?.cancel();
        reconnecting.value = false;
        deviceId = message.welcome.deviceId;
        welcomeName = message.welcome.name;
        welcomeColor = message.welcome.color;
        confirmBy.value = message.welcome.confirmByMs > 0
            ? DateTime.fromMillisecondsSinceEpoch(
                message.welcome.confirmByMs.toInt(),
              )
            : null;
        pairing.value = null;
        movedAway.value = null;
        connectedSince.value = DateTime.now();
        lastConnected.value = null;
        lastFailure.value = null;
        _setAside = null;
        // Let in after all (asked again: probeSignIn).
        signInRequired.value = null;
        connected.value = true;
        _log('connected to factoryd ${message.welcome.serverVersion}');
        _sendWhenBack();
        _selectAgain();
        final started = _tryStarted;
        _welcomedAt = DateTime.now();
        _note(ConnectionKind.connected, {
          ConnectionKey.attempt: _tries,
          ConnectionKey.route: _tryRoute == ConnectionRoute.tunnel
              ? _pathRoute(_tunnel?.status.path ?? TunnelPath.none)
              : _tryRoute,
          // The network, when it wasn't known yet as the try began (the
          // first try after a launch, before the system has told it).
          if (_tryNet == null && deviceNetwork.value != null)
            ConnectionKey.net: netKind(deviceNetwork.value!),
        }, started == null ? Duration.zero : _welcomedAt!.difference(started));
        if (deviceNetwork.value != null) _netTold = true;
        _sendDiagnosticsEvery();
        // The backend forgets a watch when the connection closes.
        if (_watching.isNotEmpty) fireAndLog(watchFiles(_watching));
        if (_watch case final watch?) fireAndLog(watchTranscript(watch));
        // And whether the person is using it, and (once it knows) whether
        // this device shows notifications: Notify goes to the connections
        // not in use, so an inactive one says it too.
        if (_active ||
            notificationPermission() !=
                NotificationPermission.NOTIFICATION_PERMISSION_UNSPECIFIED) {
          fireAndLog(sendActive(_active));
        }
      case ServerMessage_Body.ack:
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.pairingPending:
        unawaited(_pairingPending(message.pairingPending));
      case ServerMessage_Body.error:
        final toHello = message.replyTo.toInt() == _helloId;
        if (toHello &&
            message.error.code == ErrorCode.ERROR_CODE_UNAUTHORIZED) {
          if (_target != null) {
            // Not (or no longer) a paired device: retrying won't help.
            // Removed from its Claktory (lost, say), it keeps nothing of
            // it: the files its viewers kept and its unsent recordings go
            // first, as forgetting it here does (Fugo 33088).
            final key = keptClaktoryKey(_target);
            unawaited(FileCache.forget(key));
            unawaited(forgetKeptVoices(key));
            refused.value = message.error.message;
          } else if (!connected.value) {
            // This Mac's own, not ready for this app yet (just installed,
            // its code not written): it tries again, reading the code
            // afresh.
            _log('this Mac\'s Claktory refused: ${message.error.message}');
            unawaited(_socket?.close());
          }
        } else if (toHello &&
            message.error.code == ErrorCode.ERROR_CODE_SIGN_IN_REQUIRED) {
          // Refused until this device signs in: neither will it.
          final error = message.error;
          signInSaid =
              error.accountService.isEmpty && error.accountEmail.isEmpty
              ? null
              : (service: error.accountService, email: error.accountEmail);
          signInRequired.value = error.message;
        } else if (toHello &&
            message.error.code == ErrorCode.ERROR_CODE_OTHER_ACCOUNT) {
          // Another account's Claktory (#529, S4): nothing's tried until
          // Try again, as when refused, but nothing here is forgotten.
          // What it said shows on its own screen (OtherAccountView).
          // Set first: what shows reads it as [otherAccount] changes.
          otherAccountBefore = message.error.otherAccountBefore;
          otherAccount.value = message.error.message;
          refused.value = message.error.message;
        } else if (toHello &&
            message.error.code == ErrorCode.ERROR_CODE_MOVED) {
          // Moved away: its screen shows where to, and the connection
          // stays open for deleting the copy left here.
          _welcomeTimer?.cancel();
          _movedSocket = _socket;
          reconnecting.value = false;
          movedAway.value = message.error.moved;
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
        _workspaceRevision = message.workspaceRevision;
        workspace.apply(_withSelection(message.workspace));
        _sawMove(message.workspace);
        _followRelays(message.workspace);
        _keepShares(message.workspace);
        _shareDiagnostics(message.workspace.diagnostics.share);
      case ServerMessage_Body.workspacePatch:
        // A patch names the Workspace it changes: the one held, also the one
        // held before this connection (resumed). From a backend before
        // revisions, the last one this connection had.
        final base = message.workspaceBaseRevision;
        final fits = base.isEmpty
            ? _wholeWorkspace
            : base == _workspaceRevision;
        final next = fits
            ? patchWorkspace(workspace.workspace, message.workspacePatch)
            : null;
        if (next == null) {
          // It can't be applied: start again, from a whole one.
          _log('a workspace patch that does not fit: reconnecting');
          _wholeWorkspace = false;
          _workspaceRevision = '';
          _restart(ConnectionReason.patch);
          return;
        }
        _wholeWorkspace = true;
        _workspaceRevision = message.workspaceRevision;
        workspace.apply(_withSelection(next));
        _sawMove(next);
        _followRelays(next);
        _keepShares(next);
        _shareDiagnostics(next.diagnostics.share);
      case ServerMessage_Body.transcript:
        transcript.load(
          message.transcript,
          revision: message.transcriptRevision,
        );
      case ServerMessage_Body.entry:
        transcript.update(message.entry, revision: message.transcriptRevision);
      case ServerMessage_Body.delta:
        transcript.delta(message.delta, revision: message.transcriptRevision);
      case ServerMessage_Body.explorer:
        explorer.apply(message.explorer);
      case ServerMessage_Body.extensionView:
        extensionView.apply(message.extensionView);
      case ServerMessage_Body.queuedMessages:
        queued.apply(message.queuedMessages);
      case ServerMessage_Body.highlight:
        highlights.apply(message.highlight);
      case ServerMessage_Body.openScreen:
        screenRequested(message.openScreen);
      case ServerMessage_Body.pairingCode:
      case ServerMessage_Body.appChunk:
      case ServerMessage_Body.folderList:
      case ServerMessage_Body.fileContent:
      case ServerMessage_Body.vaultSession:
      case ServerMessage_Body.secretValue:
      case ServerMessage_Body.envPreview:
      case ServerMessage_Body.recoveryKey:
      case ServerMessage_Body.recoveryKeyCheck:
      case ServerMessage_Body.secretUses:
      case ServerMessage_Body.entryPage:
      case ServerMessage_Body.uploadProgress:
      case ServerMessage_Body.uploadedFile:
      case ServerMessage_Body.connectStarted:
      case ServerMessage_Body.connectChoices:
      case ServerMessage_Body.usageReport:
      case ServerMessage_Body.needsYouHistory:
      case ServerMessage_Body.usageResetAnswer:
      case ServerMessage_Body.appPermissions:
      case ServerMessage_Body.appAccessUndone:
      case ServerMessage_Body.pipeInvite:
      case ServerMessage_Body.pipeInvitePreview:
      case ServerMessage_Body.pipeMessagePage:
      case ServerMessage_Body.chatEntryPage:
      case ServerMessage_Body.quietCopies:
      case ServerMessage_Body.browserLog:
      case ServerMessage_Body.inkBlobData:
      case ServerMessage_Body.inkLayerSaved:
      case ServerMessage_Body.presentation:
      case ServerMessage_Body.sketchCreated:
      case ServerMessage_Body.dmLeafOffer:
      case ServerMessage_Body.pipeReportPage:
      case ServerMessage_Body.replyReport:
      case ServerMessage_Body.conversationMatches:
      case ServerMessage_Body.appFiles:
      case ServerMessage_Body.monitorList:
      case ServerMessage_Body.monitorHistory:
      case ServerMessage_Body.branches:
      case ServerMessage_Body.pipeShareOpened:
      case ServerMessage_Body.pipeSharePort:
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.historyOverview:
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.historyEvents:
        // An answer to historyEvents, or new events while watched.
        if (message.replyTo != 0) {
          _pending.remove(message.replyTo.toInt())?.complete(message);
        } else {
          _historyPushes.add(message.historyEvents);
        }
      case ServerMessage_Body.historyChanged:
        historyChanged.value = message.historyChanged.historyVersion;
      case ServerMessage_Body.agentBrowserFrame:
      case ServerMessage_Body.agentBrowserClipboard:
      case ServerMessage_Body.agentBrowserViewMoved:
      case ServerMessage_Body.errandLog:
      case ServerMessage_Body.timeZones:
      // PreviewProjectFolder's answer (New project's caption), to the asker.
      case ServerMessage_Body.projectFolderPreview:
      // A recovery-codes card's, to this connection only (#435 W8b): to
      // the card that asked, never kept here.
      case ServerMessage_Body.recoveryCodes:
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.deviceVouched:
        // VouchDevice's answer, to this app only (#529).
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.dmFeed:
        // DmSend's receipt, DmFollow's pages, or pushed as things come.
        _pending.remove(message.replyTo.toInt())?.complete(message);
        _dmFeeds.add(message.dmFeed);
      case ServerMessage_Body.dmHome:
        _pending.remove(message.replyTo.toInt())?.complete(message);
        _dmHomes.add(message.dmHome);
      case ServerMessage_Body.monitorsChanged:
        // The Monitors page asks again while it's on screen.
        monitorsChanged.value++;
      case ServerMessage_Body.audioTranscribed:
        // To this connection alone, for its transcribeAudio.
        final heard = message.audioTranscribed;
        _dictations.remove(heard.requestId)?.complete(heard);
      case ServerMessage_Body.inkLayerList:
        // Asked for, or sent to every client when a layer on its path is
        // saved or deleted (reply_to 0).
        inkLayerLists.value = message.inkLayerList;
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.threadList:
        threads.apply(message.threadList);
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.thread:
        threads.put(message.thread);
        _pending.remove(message.replyTo.toInt())?.complete(message);
      case ServerMessage_Body.fileChanged:
        fileChanged.value = message.fileChanged;
      case ServerMessage_Body.pipeTranscript:
        pipeChat.load(message.pipeTranscript);
      case ServerMessage_Body.pipeMessageUpdate:
        pipeChat.update(message.pipeMessageUpdate);
        // Every connection's: the watched pipe's too.
        watchedPipe.update(message.pipeMessageUpdate);
      case ServerMessage_Body.chatTranscript:
        final chat = message.chatTranscript;
        chatTranscript.load(
          Transcript(
            agentId: chat.chatId,
            entries: chat.entries,
            hasEarlier: chat.hasEarlier,
          ),
        );
      case ServerMessage_Body.chatEntry:
        chatTranscript.update(
          EntryUpdate(
            agentId: message.chatEntry.chatId,
            entry: message.chatEntry.entry,
          ),
        );
      case ServerMessage_Body.chatDelta:
        final delta = message.chatDelta;
        chatTranscript.delta(
          EntryDelta(agentId: delta.chatId, seq: delta.seq, text: delta.text),
        );
      case ServerMessage_Body.chatQueuedMessages:
        chatQueued.apply(
          QueuedMessages(
            agentId: message.chatQueuedMessages.chatId,
            messages: message.chatQueuedMessages.messages,
          ),
        );
      case ServerMessage_Body.appSurface:
        final surface = appSurfaceData(message.appSurface);
        final pending = _pending.remove(message.replyTo.toInt());
        if (pending != null) {
          pending.complete(message);
        } else {
          _viewEvents.add(AppSurfaceChanged(surface));
        }
      case ServerMessage_Body.appViewUpdate:
        final update = message.appViewUpdate;
        _viewEvents.add(
          AppSurfaceUpdated(
            AppViewUpdateData(
              surfaceId: update.surfaceId,
              seq: update.seq.toInt(),
              message: update.message,
            ),
          ),
        );
      case ServerMessage_Body.notify:
        _notified.add(message.notify);
      case ServerMessage_Body.browserCommand:
      case ServerMessage_Body.browserEnded:
        _browser.add(message);
      case ServerMessage_Body.notSet:
        // A message kind from a newer backend; nothing to do with it yet.
        break;
    }
    _readyAfter(message.whichBody());
  }

  /// Records [kind] for [diagnostics], while they're on; and once enough
  /// wait, sends them.
  void _note(
    ConnectionKind kind, [
    Map<ConnectionKey, Object> detail = const {},
    Duration took = Duration.zero,
  ]) {
    final record = diagnostics;
    if (record == null || !record.on) return;
    record.add(kind, detail, took);
    if (record.waitingCount >= diagnosticsSendPast) {
      unawaited(_sendDiagnostics());
    }
  }

  static RelayTrouble? _relayTrouble(TunnelRelayTrouble trouble) =>
      switch (trouble) {
        TunnelRelayTrouble.none => null,
        TunnelRelayTrouble.unchosen => RelayTrouble.unchosen,
        TunnelRelayTrouble.dns => RelayTrouble.dns,
        TunnelRelayTrouble.timeout => RelayTrouble.timeout,
        TunnelRelayTrouble.tls => RelayTrouble.tls,
        TunnelRelayTrouble.network => RelayTrouble.network,
        TunnelRelayTrouble.other => RelayTrouble.other,
      };

  static PathReach? _reach(TunnelReach reach) => switch (reach) {
    TunnelReach.none => null,
    TunnelReach.local => PathReach.local,
    TunnelReach.lan => PathReach.lan,
    TunnelReach.tailnet => PathReach.tailnet,
    TunnelReach.public => PathReach.public,
  };

  /// The device's network as the try began, when known.
  NetKind? _tryNet;

  /// Whether the device's network has been said since the app started:
  /// on a try's connect or connected, or, when the system tells it only
  /// after both (Android's first answer can take a second, Sale), as a
  /// `network` event with app=started: the network at launch, not a move.
  var _netTold = false;

  /// Whether the system last said it blocks this app's network.
  var _wasBlocked = false;

  /// The system lets this app's network through again (W6): a Claktory
  /// told to keep trying in the background tries at once, as for a network
  /// move. The others wait as before: it isn't a move.
  void _watchBlocked() {
    final blocked = deviceNetwork.value?.blocked ?? false;
    final unblocked = _wasBlocked && !blocked;
    _wasBlocked = blocked;
    if (unblocked && _keepTrying) {
      _log('its network is let through again: trying now');
      networkMoved();
    }
  }

  void _networkKnown() {
    final path = deviceNetwork.value;
    if (path == null || _netTold || _tries == 0) return;
    _netTold = true;
    _note(ConnectionKind.network, {
      ConnectionKey.net: netKind(path),
      ConnectionKey.app: AppLife.started,
    });
  }

  /// Whether the connection was welcomed and isn't [ConnectionKind.ready]
  /// yet, and whether its Workspace is in.
  var _readyDue = false;
  var _readyWorkspace = false;

  /// Notes [ConnectionKind.ready] once the welcome's sync is in: the
  /// Workspace (whole, or the patch that resumes it), then, when an agent
  /// is shown, its conversation, which the Claktory sends straight after
  /// it. How long since the try began, and what the tunnel's connection
  /// had received by then.
  void _readyAfter(ServerMessage_Body body) {
    if (body == ServerMessage_Body.welcome) {
      _readyDue = true;
      _readyWorkspace = false;
      return;
    }
    if (!_readyDue) return;
    if (!_readyWorkspace) {
      _readyWorkspace =
          body == ServerMessage_Body.workspace ||
          body == ServerMessage_Body.workspacePatch;
      // An agent shown: its conversation follows.
      if (!_readyWorkspace || workspace.workspace.selectedAgentId.isNotEmpty) {
        return;
      }
    } else if (body != ServerMessage_Body.transcript) {
      return;
    }
    _readyDue = false;
    final started = _tryStarted;
    final through = _tryRoute == ConnectionRoute.tunnel ? _tunnel : null;
    _note(ConnectionKind.ready, {
      ConnectionKey.attempt: _tries,
      if (through != null) ConnectionKey.bytes: through.status.linkReceived,
    }, started == null ? Duration.zero : DateTime.now().difference(started));
  }

  static ConnectionRoute _pathRoute(TunnelPath path) => switch (path) {
    TunnelPath.direct => ConnectionRoute.direct,
    TunnelPath.relayed => ConnectionRoute.relayed,
    TunnelPath.none => ConnectionRoute.tunnel,
  };

  /// How often what's recorded is sent while connected, and how many
  /// waiting send it at once.
  Duration diagnosticsEvery = const Duration(seconds: 30);
  int diagnosticsSendPast = 200;

  /// The most events one request carries (SendDiagnostics' own limit).
  static const _diagnosticsPerRequest = 2000;

  Timer? _diagnosticsTimer;
  var _sendingDiagnostics = false;

  /// The Claktory says whether diagnostics are on: the device follows
  /// (turned off, what it kept goes), and sends what waits.
  void _shareDiagnostics(bool share) {
    final record = diagnostics;
    if (record == null) return;
    final turnedOn = share && !record.on;
    if (record.on != share) {
      _log('advanced diagnostics ${share ? 'on: recording' : 'off'}');
    }
    record.share(share);
    // Turned on while connected: the connection as it is now, sent at once.
    // A device whose connection stays up (a Mac on its own Claktory) had
    // nothing to send until its next reconnect, so it seemed to start only
    // after a relaunch (friend bug #53).
    if (turnedOn && connected.value) {
      _note(ConnectionKind.recording, {
        ConnectionKey.route: _tryRoute == ConnectionRoute.tunnel
            ? _pathRoute(_tunnel?.status.path ?? TunnelPath.none)
            : _tryRoute,
        if (deviceNetwork.value case final net?)
          ConnectionKey.net: netKind(net),
      });
    }
    if (share) unawaited(_sendDiagnostics());
  }

  void _sendDiagnosticsEvery() {
    _diagnosticsTimer?.cancel();
    if (diagnostics == null) return;
    _diagnosticsTimer = Timer.periodic(
      diagnosticsEvery,
      (_) => unawaited(_sendDiagnostics()),
    );
  }

  /// Sends what's recorded, oldest first, a request at a time; what the
  /// Claktory kept leaves the device. A refusal (diagnostics turned off
  /// meanwhile, an older Claktory) leaves it for the next time: the
  /// Claktory's own word on whether they're on decides what's kept.
  Future<void> _sendDiagnostics() async {
    final record = diagnostics;
    if (record == null || _sendingDiagnostics) return;
    _sendingDiagnostics = true;
    try {
      while (record.on && connected.value && !_closed) {
        final batch = record.waiting.take(_diagnosticsPerRequest).toList();
        if (batch.isEmpty) return;
        await _request(
          ClientMessage(
            sendDiagnostics: SendDiagnostics(
              events: [
                for (final event in batch)
                  DiagnosticEvent(
                    atMs: Int64(event.atMs),
                    seq: Int64(event.seq),
                    area: event.area,
                    kind: event.kind,
                    detail: event.detail,
                    durationMs: Int64(event.durationMs),
                    session: Int64(event.session),
                  ),
              ],
            ),
          ),
        );
        record.sent(batch.last.seq);
        _log('sent ${batch.length} diagnostic events');
      }
    } on RequestFailure catch (error) {
      _log("couldn't send diagnostics: ${error.message}");
    } finally {
      _sendingDiagnostics = false;
    }
  }

  void _disconnected() {
    _socket = null;
    // The next connection may be over another link: its pieces start
    // small again (Sale's row 68).
    _upPace.relink();
    _downPace.relink();
    _diagnosticsTimer?.cancel();
    _diagnosticsTimer = null;
    _wholeWorkspace = false;
    // A connection that was up and dropped is most likely a network change:
    // it comes back without a word, if it can.
    if (connected.value && !_closed) _reconnectQuietly();
    if (connected.value) lastConnected.value = DateTime.now();
    connectedSince.value = null;
    connected.value = false;
    final pending = _pending.values.toList();
    _pending.clear();
    for (final completer in pending) {
      completer.completeError(const RequestFailure('Connection lost.'));
    }
    // Their answers went with the connection.
    final dictations = _dictations.values.toList();
    _dictations.clear();
    for (final completer in dictations) {
      completer.completeError(const RequestFailure('Connection lost.'));
    }
  }
}

/// Sends a request nothing waits on, logging it if the backend refuses.
void fireAndLog(Future<void> request) {
  // Through then<void>: [request] may be a Future of a reply passed as
  // one of void, whose catchError can't take a handler that returns
  // nothing.
  request
      .then<void>((_) {})
      .catchError(
        (Object error) => _log('request failed: $error'),
        test: (error) => error is RequestFailure,
      );
}

void _log(String message) {
  developer.log(message, name: 'factory.backend');
  // Built with FACTORY_LOG_STDOUT=true (to test; ignored in release builds), the
  // lines that say what the connection did (never where it goes: no
  // address, path or key) also go to the standard output, Android's
  // logcat, read beside the tunnel's own.
  if (_logStdout && _toStdout.any(message.startsWith)) {
    debugPrint('factory.backend: $message');
  }
}

// Never in a release build, whatever it was built with: only debug and
// profile builds, which nothing ships, can print them.
const _logStdout = bool.fromEnvironment('FACTORY_LOG_STDOUT') && !kReleaseMode;

/// QA only, in Claktory Dev (#214): the first try after coming back from
/// the background hangs for good, as Matt's iPhone's did, so a tap on
/// Reconnecting… and the from-scratch watch can be checked. Built in only
/// with --dart-define=FACTORY_QA_STALL_RESUME=true.
const _qaStallResume =
    bool.fromEnvironment('FACTORY_QA_STALL_RESUME') &&
    bool.fromEnvironment('FACTORY_DEV');

/// QA only, in Claktory Dev (#379): files come as if over a slow link,
/// this many kbit/s with [_qaRoundTripMs] on top of each read, so the
/// phone's previews can be timed against the original without slowing
/// the whole computer's network. Built in only with
/// --dart-define=FACTORY_QA_DOWNLINK_KBPS=1000 (and
/// FACTORY_QA_ROUND_TRIP_MS=150).
const _qaDownlinkKbps = bool.fromEnvironment('FACTORY_DEV')
    ? int.fromEnvironment('FACTORY_QA_DOWNLINK_KBPS')
    : 0;
const _qaRoundTripMs = bool.fromEnvironment('FACTORY_DEV')
    ? int.fromEnvironment('FACTORY_QA_ROUND_TRIP_MS')
    : 0;

const _toStdout = [
  'to the background',
  'back after ',
  'in the background: ',
  'not back within ',
  "the tunnel's port didn't answer",
  "the old tunnel didn't close",
  'no welcome within ',
  'no connection within ',
  'connection closed (code ',
  'the network moved',
];

/// The selected chat's conversation: an agent's transcript store, keyed by
/// the chat's id.
class ChatTranscriptStore extends TranscriptStore {
  /// The chat it belongs to; empty before one loads.
  String get chatId => agentId;
}

/// A chat's way to the backend for the outbox: text, or a voice message
/// (#367), its recording uploaded for the chat as an agent's is; the
/// backend takes no other file for a chat.
class _ChatOutbox extends BackendOutbox {
  _ChatOutbox(this._chats) : super(_chats);

  final Backend _chats;

  @override
  Future<void> send(
    String agentId,
    String text,
    String clientId,
    List<SentFile> files, {
    bool voice = false,
  }) => _chats.sendChatMessage(
    agentId,
    text,
    clientId: clientId,
    voice: voice,
    attachments: [
      for (final file in files)
        Attachment(
          path: file.path,
          name: file.name,
          size: Int64(file.size),
          mime: file.mime,
        ),
    ],
  );
}

/// [surface] as the app's views hold it.
AppSurfaceData appSurfaceData(AppSurface surface) => AppSurfaceData(
  surfaceId: surface.surfaceId,
  appId: surface.appId,
  view: surface.view,
  params: surface.params,
  title: surface.title,
  state: switch (surface.state) {
    AppSurfaceState.APP_SURFACE_STATE_OPEN => AppViewState.open,
    AppSurfaceState.APP_SURFACE_STATE_STOPPED => AppViewState.stopped,
    AppSurfaceState.APP_SURFACE_STATE_FAILED => AppViewState.failed,
    AppSurfaceState.APP_SURFACE_STATE_CLOSED => AppViewState.closed,
    AppSurfaceState.APP_SURFACE_STATE_UNSUPPORTED => AppViewState.unsupported,
    // Opening, or a state from a newer backend: not shown yet.
    _ => AppViewState.opening,
  },
  error: surface.error,
  catalog: surface.catalog,
  seq: surface.seq.toInt(),
  snapshot: surface.snapshot,
);
