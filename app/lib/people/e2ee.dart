import 'dart:async';
import 'dart:convert';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart' show Backend;
import '../protocol/factory/v1/factory.pb.dart' show DmLeafOffer;
import '../scope.dart';
import '../tunnel/api/dm.dart';
import '../tunnel/api/dm_transfer.dart';
import '../tunnel_client.dart' show RustTunnels;
import '../user_apps/session.dart';
import 'dm_keys.dart';
import '../user_apps/grants.dart';
import 'review.dart' show DmAgentDraft, DmProposal, hiddenCharacter;

// E5: direct messages end to end (dms-e2ee.md; the worker's contract is
// crates/claktory-user-apps/README.md, "End to end"). The service relays
// MLS messages it can't read; this device keeps its own history, sealed,
// in a local store (app/rust's DmDevice over dm/store.rs), with its keys
// in the platform's key store (dm_keys.dart).
//
// [E2ee] is the one place that talks to the worker for end to end: it
// sets the device up (the person's first device, or another one added
// from one that's set up), joins the groups it's in (Welcomes, or by
// external commit when it has none or fell behind), takes every group's
// feed in order into the store, and sends what the person says, each
// message encrypted once and kept until it comes back. Chats, drafts,
// read positions and reviews read from it and go through it.
//
// Fugo's call-site rules hold here: pins come from the service only as a
// first contact or a key change the person sees (never imported from it);
// pins from another device only by the self group or the hand-off, both in
// Rust; a handshake that fails makes the device join again, with backoff;
// the identity's secret is in Dart only between being made and kept, and
// is never logged.

/// Stands in for this device's engine in tests.
E2ee Function(BuildContext context)? e2eeFor;

final _engines = Expando<E2ee>();

/// This device's engine, for the account it's signed in to: started and
/// following the worker from its first use, for as long as the app runs
/// (dms-e2ee §5.3: every group is taken in while the app runs, not only
/// while a chat is open). Signing in or out starts it again.
E2ee e2eeOf(BuildContext context) {
  if (e2eeFor case final hook?) return hook(context);
  final scope = AppScope.scope(context);
  final account = scope.connections.account;
  final known = _engines[account];
  if (known != null) return known;
  final e2ee = _engines[account] = E2ee(session: UserAppsSession(account))
    ..claktory = _BackendLink(scope.backend);
  var signedIn = account.signedIn;
  var key = account.key;
  account.addListener(() {
    // Removed by another of the person's devices: a new account key, so
    // this device starts over as a new one, its keys and store gone (Fugo).
    final before = key;
    key = account.key;
    if (startsOver(before, key)) {
      signedIn = account.signedIn;
      unawaited(e2ee.startOver());
      return;
    }
    if (account.signedIn == signedIn) return;
    signedIn = account.signedIn;
    unawaited(e2ee.start());
  });
  // Back in front: grants that ended meanwhile come out first.
  AppLifecycleListener(onResume: () => unawaited(e2ee.grantsChanged()));
  unawaited(e2ee.start());
  unawaited(e2ee.follow());
  return e2ee;
}

/// Whether this device's messages start over as a new device: its account
/// key changed from one it had (removed by another device). A first key,
/// made at a fresh device's first sign-in, never does (Fugo).
bool startsOver(String before, String now) =>
    before.isNotEmpty && now != before;

/// The Claktory the app shows, through its backend connection.
class _BackendLink implements DmClaktoryLink {
  _BackendLink(this.backend);

  final Backend backend;

  @override
  Future<DmLeafOffer> offer() => backend.dmLeafOffer();

  @override
  Future<void> enroll(List<int> certificate, List<int> pins) =>
      backend.dmEnrollLeaf(certificate, pins);
}

/// What a security code's QR text starts with; its bytes follow, in
/// base64url.
const dmSafetyPrefix = 'claktory-safety:1:';

/// Base64url without padding, as the worker writes ids and keys.
String dmB64(List<int> bytes) => base64Url.encode(bytes).replaceAll('=', '');

/// What [dmB64] wrote.
Uint8List dmUnb64(String text) =>
    base64Url.decode(text.padRight((text.length + 3) ~/ 4 * 4, '='));

/// An envelope as the worker's JSON has it, as the string Rust takes.
String _envelope(Object? json) => jsonEncode(json);

/// An envelope Rust made, as the JSON the worker takes.
Object? _wire(String envelope) => jsonDecode(envelope);

/// An account's keys at the worker: its identity's public key and its
/// signed device list, at its version.
class DmAccountKeys {
  const DmAccountKeys({
    required this.accountId,
    required this.identityKey,
    required this.deviceList,
    required this.version,
  });

  factory DmAccountKeys.fromJson(Map<String, Object?> json) => DmAccountKeys(
    accountId: _text(json, 'account_id'),
    identityKey: dmUnb64(_text(json, 'identity_key')),
    deviceList: dmUnb64(_text(json, 'device_list')),
    version: _int(json, 'version'),
  );

  final String accountId;
  final Uint8List identityKey;
  final Uint8List deviceList;
  final int version;
}

/// A group as the worker lists it.
class DmServerGroup {
  const DmServerGroup({
    required this.groupId,
    required this.kind,
    this.conversationId = '',
    this.generation = 0,
    this.accounts = const [],
    this.claktoryId = '',
    this.epoch = 0,
    this.lastSeq = 0,
  });

  factory DmServerGroup.fromJson(Map<String, Object?> json) => DmServerGroup(
    groupId: _text(json, 'group_id'),
    kind: _text(json, 'kind'),
    conversationId: _text(json, 'conversation_id'),
    generation: _int(json, 'contact_generation'),
    accounts: [
      for (final account in json['accounts'] as List? ?? const [])
        if (account is String) account,
    ],
    claktoryId: _text(json, 'claktory_id'),
    epoch: _int(json, 'epoch'),
    lastSeq: _int(json, 'last_seq'),
  );

  /// Base64url, as in paths.
  final String groupId;

  /// `conversation`, `self` or `channel`.
  final String kind;
  final String conversationId;
  final int generation;

  /// A conversation's two people.
  final List<String> accounts;

  /// A channel's Claktory.
  final String claktoryId;
  final int epoch;
  final int lastSeq;

  Uint8List get id => dmUnb64(groupId);

  /// What this device's store calls its kind, from what the worker says;
  /// null for a kind this doesn't know. [me] is the person's account.
  DmGroupKind? kindFor(String me) => switch (kind) {
    'conversation' => DmGroupKind(
      kind: DmGroupType.conversation,
      accounts: accounts,
      claktoryId: '',
    ),
    'self' => DmGroupKind(
      kind: DmGroupType.selfGroup,
      accounts: [me],
      claktoryId: '',
    ),
    'channel' => DmGroupKind(
      kind: DmGroupType.channel,
      accounts: [me],
      claktoryId: claktoryId,
    ),
    _ => null,
  };
}

/// One entry of a group's feed.
class DmFeedItem {
  const DmFeedItem({
    required this.seq,
    required this.kind,
    required this.envelope,
    this.acceptedMs = 0,
    this.epoch = 0,
  });

  factory DmFeedItem.fromJson(Map<String, Object?> json) => DmFeedItem(
    seq: _int(json, 'seq'),
    kind: _text(json, 'kind'),
    envelope: _envelope(json['envelope']),
    acceptedMs: _int(json, 'accepted_ms'),
    epoch: _int(json, 'epoch'),
  );

  final int seq;

  /// `application`, `commit` or `proposal`.
  final String kind;

  /// The envelope's JSON.
  final String envelope;
  final int acceptedMs;
  final int epoch;
}

/// A Welcome waiting for this device.
class DmWelcomeItem {
  const DmWelcomeItem({
    required this.id,
    required this.group,
    required this.welcome,
    this.seq = 0,
  });

  factory DmWelcomeItem.fromJson(Map<String, Object?> json) => DmWelcomeItem(
    id: _text(json, 'id'),
    group: DmServerGroup.fromJson(json),
    welcome: _envelope(json['welcome']),
    seq: _int(json, 'seq'),
  );

  final String id;

  /// The group it's for (its id and kind; no epoch or seq).
  final DmServerGroup group;
  final String welcome;

  /// The seq of the commit that added this device: its feed goes on after.
  final int seq;
}

/// A sealed hand-off waiting for this device.
class DmHandoffItem {
  const DmHandoffItem({
    required this.id,
    required this.from,
    required this.sealed,
  });

  factory DmHandoffItem.fromJson(Map<String, Object?> json) => DmHandoffItem(
    id: _text(json, 'id'),
    from: _text(json, 'from'),
    sealed: dmUnb64(_text(json, 'sealed')),
  );

  final String id;
  final String from;
  final Uint8List sealed;
}

/// The worker's end-to-end routes, through [session].
class DmRoutes {
  const DmRoutes(this.session);

  final UserAppsSession session;

  static const _dms = '/v1/apps/dms';

  /// [account]'s keys; null before it set any up.
  Future<DmAccountKeys?> keys(String account) async {
    try {
      final answer = await session.get('$_dms/keys/$account');
      if (_text(answer, 'identity_key').isEmpty) return null;
      return DmAccountKeys.fromJson(answer);
    } on UserAppError catch (error) {
      if (error.problem == UserAppProblem.notFound ||
          error.problem == UserAppProblem.notSetUp) {
        return null;
      }
      rethrow;
    }
  }

  /// The person's keys: [identityKey] only the first time; [version]
  /// replaces [previousVersion]. [UserAppProblem.stale] carries the keys
  /// as they are (`details['keys']`).
  Future<DmAccountKeys> putKeys({
    Uint8List? identityKey,
    required Uint8List deviceList,
    required int version,
    required int previousVersion,
  }) async => DmAccountKeys.fromJson(
    await session.put('$_dms/me/keys', {
      'identity_key': ?(identityKey == null ? null : dmB64(identityKey)),
      'device_list': dmB64(deviceList),
      'version': version,
      'previous_version': previousVersion,
    }),
  );

  /// The person's new security key (Lost or stolen, dms-e2ee §2.1): its
  /// public half and a version-1 device list signed by it.
  Future<void> rotateKeys({
    required Uint8List identityKey,
    required Uint8List deviceList,
  }) => session.post('$_dms/me/keys/rotate', {
    'identity_key': dmB64(identityKey),
    'device_list': dmB64(deviceList),
    'version': 1,
  });

  /// One-time key packages for this device's leaf, and its last-resort one.
  Future<int> uploadKeyPackages(
    List<String> packages, {
    String? lastResort,
  }) async => _int(
    await session.post('$_dms/key-packages', {
      'key_packages': [for (final package in packages) _wire(package)],
      'last_resort': ?(lastResort == null ? null : _wire(lastResort)),
    }),
    'remaining',
  );

  /// How many one-time key packages this device's leaf has left.
  Future<int> keyPackagesLeft() async =>
      _int(await session.get('$_dms/key-packages'), 'remaining');

  /// A key package for each of [account]'s leaves (or of [leafIds]).
  Future<List<({String leafId, String keyPackage})>> claim(
    String account, {
    List<String>? leafIds,
  }) async => [
    for (final item in _list(
      await session.post('$_dms/key-packages/claim', {
        'account_id': account,
        'leaf_ids': ?leafIds,
      }),
      'key_packages',
    ))
      (
        leafId: _text(item, 'leaf_id'),
        keyPackage: _envelope(item['key_package']),
      ),
  ];

  /// Registers a group this device made, with its GroupInfo at epoch 0
  /// (the worker reads the group's id from it).
  Future<void> createGroup(DmServerGroup group, String groupInfo) =>
      session.post('$_dms/groups', {
        'kind': group.kind,
        if (group.conversationId.isNotEmpty) ...{
          'conversation_id': group.conversationId,
          'contact_generation': group.generation,
        },
        if (group.claktoryId.isNotEmpty) 'claktory_id': group.claktoryId,
        'group_info': _wire(groupInfo),
      });

  Future<List<DmServerGroup>> groups() async => [
    for (final item in _list(await session.get('$_dms/groups'), 'groups'))
      DmServerGroup.fromJson(item),
  ];

  /// [group]'s feed after [after], oldest first.
  Future<({List<DmFeedItem> items, bool hasMore})> feed(
    String group,
    int after,
  ) async {
    final answer = await session.get(
      '$_dms/groups/$group/messages?after=$after&limit=100',
    );
    return (
      items: [
        for (final item in _list(answer, 'messages')) DmFeedItem.fromJson(item),
      ],
      hasMore: answer['has_more'] == true,
    );
  }

  /// A commit, with its Welcome and the new epoch's GroupInfo.
  Future<({int seq, int epoch})> commit(String group, DmCommit commit) async {
    final answer = await session.post('$_dms/groups/$group/commits', {
      'commit': _wire(commit.commit),
      'welcome': ?(commit.welcome == null ? null : _wire(commit.welcome!)),
      'group_info': ?(commit.groupInfo == null
          ? null
          : _wire(commit.groupInfo!)),
    });
    return (seq: _int(answer, 'seq'), epoch: _int(answer, 'epoch'));
  }

  /// The GroupInfo to join [group] by external commit.
  Future<String> groupInfo(String group) async => _envelope(
    (await session.get('$_dms/groups/$group/group-info'))['group_info'],
  );

  Future<List<DmWelcomeItem>> welcomes() async => [
    for (final item in _list(await session.get('$_dms/welcomes'), 'welcomes'))
      DmWelcomeItem.fromJson(item),
  ];

  Future<void> deleteWelcome(String id) => session.delete('$_dms/welcomes/$id');

  /// A message in a self group or channel.
  Future<void> sayInGroup(String group, String clientId, String envelope) =>
      session.post('$_dms/groups/$group/messages', {
        'client_message_id': clientId,
        'envelope': _wire(envelope),
      });

  /// A message in a conversation.
  Future<void> sayInConversation(
    String conversation,
    int generation,
    String clientId,
    String envelope,
  ) => session.post('$_dms/conversations/$conversation/messages', {
    'client_message_id': clientId,
    'contact_generation': generation,
    'envelope': _wire(envelope),
  });

  /// The identity and pins, sealed for the new device [to].
  Future<void> handOff(String to, Uint8List sealed) =>
      session.post('$_dms/me/handoffs', {'to': to, 'sealed': dmB64(sealed)});

  Future<List<DmHandoffItem>> handoffs() async => [
    for (final item in _list(
      await session.get('$_dms/me/handoffs'),
      'handoffs',
    ))
      DmHandoffItem.fromJson(item),
  ];

  Future<void> deleteHandoff(String id) =>
      session.delete('$_dms/me/handoffs/$id');
}

/// Where this device stands with end-to-end messages.
enum DmSetup {
  /// Opening the store and asking the worker.
  starting,

  /// Not signed in to a Claktory account on this device.
  signedOut,

  /// This device can't keep the keys (a Mac build without its signing,
  /// say): messages are off here.
  unavailable,

  /// No device of the person's is set up yet: this one can be the first.
  firstDevice,

  /// The person set up messages on another device: this one is added from
  /// there, by its QR code.
  addThisDevice,

  /// Set up: it talks.
  ready,

  /// Something kept on this device can't be read, or the worker refused
  /// it: [E2ee.failure] says what.
  failed,
}

/// Why [DmSetup.failed].
enum DmFailure {
  none,

  /// A key in the platform's key store can't be read.
  unreadableKey,

  /// The store here belongs to another account than the one signed in.
  otherAccount,

  /// A new security key was made for the account on another device: this
  /// one is added again from there (Fugo M2).
  identityReplaced,

  /// The worker or the store refused; [E2ee.problem] has the detail.
  refused,
}

/// What stopped a change of security key ([E2ee.changeKey]).
/// One of the person's devices, as [E2ee.devices] lists them.
typedef DmOwnDevice = ({String leafId, String name, bool here, int sinceMs});

/// What stopped a Remove… part-way, to try again.
enum DmRemoveStop {
  /// Not yet out of every group (or off the list); [E2ee.problem] has the
  /// detail.
  failed,

  /// Out of every group, but the account service couldn't be reached to
  /// sign it out: it may have been.
  signOutUnreachable,

  /// Out of every group, but the account service refused to sign it out.
  signOutRefused,
}

enum DmKeyChange {
  /// A removed device couldn't be signed out of the account: the new key
  /// isn't made until it is (Fugo).
  signOutFailed,

  /// The worker or this device refused; [E2ee.problem] has the detail.
  /// Tried again from where it got to.
  failed,
}

/// Opens this device's store; tests give their own.
typedef DmOpen = Future<DmDevice?> Function({
  required String path,
  required List<int> storeKey,
  required List<int> leafKey,
  Uint8List? identityKey,
});

Future<DmDevice?> _openRust({
  required String path,
  required List<int> storeKey,
  required List<int> leafKey,
  Uint8List? identityKey,
}) async {
  await RustTunnels.load();
  return DmDevice.open(
    path: path,
    storeKey: storeKey,
    leafKey: leafKey,
    identityKey: identityKey,
  );
}

Future<Uint8List> _newSecretRust() async {
  await RustTunnels.load();
  return dmNewSecret();
}

Future<void> _removeStoreRust(String path) async {
  await RustTunnels.load();
  await dmRemoveStore(path: path);
}

/// Who this device is at the account service; tests give their own.
typedef DmWhoAmI =
    Future<({String accountId, String name, String deviceKey})?> Function();

/// How many one-time key packages a device keeps at the worker, and when
/// it adds more.
const dmKeyPackages = 20;
const _keyPackagesLow = 5;

/// How long the worker may hold a request for changes: under the account
/// calls' 20 s timeout.
const _followSeconds = 15;

/// This device's end-to-end messages: set up, in step with the worker, and
/// sending. A [ChangeNotifier]: anything shown from it reads again on a
/// change ([changes] says which groups moved).
/// The Claktory this app shows, for enrolling its leaf (Ghiaccio's
/// DmLeafOffer and DmEnrollLeaf): its offer, then the certificate and pins.
abstract class DmClaktoryLink {
  Future<DmLeafOffer> offer();
  Future<void> enroll(List<int> certificate, List<int> pins);
}

/// The Claktory id in a Claktory leaf's id (`<claktory_id>#<epoch>`),
/// as claktory-dm-crypto's claktory_of reads it; null for any other.
String? claktoryOf(String leafId) {
  final at = leafId.lastIndexOf('#');
  if (at <= 0 || at == leafId.length - 1) return null;
  final epoch = leafId.substring(at + 1);
  if (!epoch.codeUnits.every((unit) => unit >= 0x30 && unit <= 0x39)) {
    return null;
  }
  return leafId.substring(0, at);
}

/// A Claktory leaf's registration epoch, for the newest of a Claktory's.
int _epochOf(String leafId) =>
    int.tryParse(leafId.substring(leafId.lastIndexOf('#') + 1)) ?? 0;

class E2ee extends ChangeNotifier {
  E2ee({
    required this.session,
    this.keys = const DmKeys(),
    DmOpen? open,
    Future<Uint8List> Function()? newSecret,
    Future<void> Function(String path)? removeStore,
    DmWhoAmI? whoAmI,
    int Function()? nowMs,
    String Function()? newId,
    Future<DmTransferEndpoint> Function()? bindTransfer,
    Future<bool> Function(String idHex)? admitTransfer,
    this.retryAfter = const Duration(seconds: 5),
  }) : routes = DmRoutes(session),
       _bindTransfer = bindTransfer ?? DmTransferEndpoint.bind,
       _admitTransfer = admitTransfer ?? _assertionFor(session),
       grantsApi = GrantsApi(session),
       _open = open ?? _openRust,
       _newSecret = newSecret ?? _newSecretRust,
       _removeStore = removeStore ?? _removeStoreRust,
       _whoAmI = whoAmI ?? session.account.whoAmI,
       _nowMs = nowMs ?? (() => DateTime.now().millisecondsSinceEpoch),
       _newId = newId ?? _randomId;

  final UserAppsSession session;
  final DmRoutes routes;
  final GrantsApi grantsApi;
  final DmKeys keys;

  /// The Claktory this app shows, whose leaf this device enrolls and gives
  /// a channel; null for none.
  DmClaktoryLink? claktory;
  final DmOpen _open;
  final Future<DmTransferEndpoint> Function() _bindTransfer;
  final Future<bool> Function(String idHex) _admitTransfer;
  final Future<Uint8List> Function() _newSecret;
  final Future<void> Function(String path) _removeStore;
  final DmWhoAmI _whoAmI;
  final int Function() _nowMs;
  final String Function() _newId;

  /// How long to wait before trying the worker again.
  final Duration retryAfter;

  var setup = DmSetup.starting;
  var failure = DmFailure.none;

  /// What went wrong last, for the log and a failure's detail.
  String problem = '';

  DmDevice? _device;

  /// The store, once open.
  DmDevice? get device => _device;

  /// The person's account, and this device's leaf id and name.
  String accountId = '';
  String leafId = '';
  String deviceName = '';

  /// The groups the worker lists for this device, by id.
  final groups = <String, DmServerGroup>{};

  final _groupChanges = StreamController<String>.broadcast();

  /// Each group whose history, view or waiting messages moved, as it does.
  Stream<String> get groupChanges => _groupChanges.stream;

  void _moved(String group) {
    if (!_closed) _groupChanges.add(group);
  }

  /// Where each message this device sent stands, by its retry id, while
  /// the app runs: sending, or why it didn't go.
  final sending = <String, DmSending>{};

  /// Groups this device fell out of step with, and when it may next try
  /// to join them again.
  final _rejoinAfter = <String, DateTime>{};
  final _rejoinTries = <String, int>{};

  Future<void>? _syncing;
  var _syncAgain = false;
  var _closed = false;

  final _events = StreamController<Map<String, Object?>>.broadcast();

  /// The worker's other changes (conversations, proposals, agents'
  /// drafts), as its outbox has them, for what shows them; and
  /// `{"kind": "resync"}` when they should all be read again.
  Stream<Map<String, Object?>> get events => _events.stream;

  Timer? _pause;
  Completer<void>? _paused;

  /// Keeps up with the worker until [dispose], one request held at a time:
  /// its outbox says what changed, and the groups it names are taken in.
  Future<void> follow() async {
    var cursor = '';
    var wait = true;
    while (!_closed) {
      // Set up, or waiting for a hand-off to this device.
      if (setup != DmSetup.ready &&
          !(setup == DmSetup.addThisDevice && _offer != null)) {
        // Until that changes; then from the start.
        await _wait(timed: false);
        cursor = '';
        continue;
      }
      try {
        if (cursor.isEmpty) {
          cursor = _text(await session.get('/v1/apps/dms/sync'), 'cursor');
          _events.add(const {'kind': 'resync'});
          await takeHandoff();
          await sync();
          continue;
        }
        final answer = await session.get(
          '/v1/apps/dms/sync?since=${Uri.encodeQueryComponent(cursor)}'
          '${wait ? '&wait=$_followSeconds' : ''}',
        );
        if (_closed) return;
        cursor = _text(answer, 'next_cursor');
        wait = answer['has_more'] != true;
        var groupsMoved = false;
        for (final event in _list(answer, 'events')) {
          switch (event['kind']) {
            case 'group_message' || 'welcome' || 'groups':
              groupsMoved = true;
            case 'keys':
              await refreshKeys(_text(event, 'account_id'));
            case 'handoff':
              await takeHandoff();
            default:
              _events.add(event);
          }
        }
        if (groupsMoved) await sync();
      } on UserAppError catch (error) {
        if (_closed) return;
        if (error.problem == UserAppProblem.gone) {
          // Too far behind: everything again.
          cursor = '';
          continue;
        }
        problem = error.toString();
        await _wait();
      }
    }
  }

  /// Until [retryAfter] has passed, or, not [timed], until where this
  /// device stands changes.
  Future<void> _wait({bool timed = true}) {
    final paused = _paused = Completer<void>();
    if (timed) {
      _pause = Timer(retryAfter, () {
        if (!paused.isCompleted) paused.complete();
      });
    }
    return paused.future;
  }

  /// Opens the store and finds where this device stands.
  Future<void> start() async {
    setup = DmSetup.starting;
    failure = DmFailure.none;
    _notify();
    if (!session.signedIn) return _set(DmSetup.signedOut);
    try {
      if (!await keys.available()) return _set(DmSetup.unavailable);
      final who = await _whoAmI();
      if (who == null) return _set(DmSetup.signedOut);
      accountId = who.accountId;
      leafId = who.deviceKey;
      deviceName = who.name;
      final device = _device = await _openStore();
      if (device == null) return;
      final status = device.status();
      if (status.setUp && status.accountId != accountId) {
        return _fail(DmFailure.otherAccount);
      }
      final mine = await routes.keys(accountId);
      if (!status.setUp) {
        return _set(mine == null ? DmSetup.firstDevice : DmSetup.addThisDevice);
      }
      // A change of security key this device was making or taking when it
      // stopped: carried on with the same new key.
      if (await _resumeKeyStore(device)) return await start();
      if (mine != null &&
          !listEquals(mine.identityKey, device.status().identityPublic)) {
        // Changed on another device (or by this one, which carries on):
        // settled once the self group has been read, never from the
        // worker alone.
        _ownKeyCheck = true;
      } else if (mine != null) {
        await device.acceptList(
          account: accountId,
          deviceList: mine.deviceList,
        );
      }
      _set(DmSetup.ready);
      if (await device.rotation() case final rotation?) {
        await _carryOnKeyChange(device, rotation.kept, rotation.removed);
      } else {
        await sync();
      }
      // A removal stopped part-way (a dialog closed, the app quit):
      // carried on, through its sign-out and any new key (R5b).
      if ((removal = await device.removal()) != null) await finishRemoval();
    } on DmKeyUnreadable catch (error) {
      problem = error.message;
      _fail(DmFailure.unreadableKey);
    } on UserAppError catch (error) {
      problem = error.toString();
      if (error.problem == UserAppProblem.notSignedIn) {
        _set(DmSetup.signedOut);
      } else {
        _fail(DmFailure.refused);
      }
    } on PlatformException catch (error) {
      problem = error.message ?? error.code;
      _fail(DmFailure.refused);
    } catch (error) {
      // Anything else ends in Try again, never a start that never ends.
      problem = '$error';
      _fail(DmFailure.refused);
    }
  }

  /// The store with this device's keys; made, with its keys, the first
  /// time. A store made with another key can't be read by anything, so
  /// it's replaced.
  Future<DmDevice?> _openStore() async {
    final storeKey = await _keyOrNew(DmKeyName.store);
    final leafKey = await _keyOrNew(DmKeyName.leaf);
    final identity = await _keptKey(DmKeyName.identity);
    final path = '${await keys.folder()}/store.db';
    var device = await _open(
      path: path,
      storeKey: storeKey,
      leafKey: leafKey,
      identityKey: identity,
    );
    if (device == null) {
      await _removeStore(path);
      device = await _open(
        path: path,
        storeKey: storeKey,
        leafKey: leafKey,
        identityKey: identity,
      );
    }
    identity?.fillRange(0, identity.length, 0);
    return device;
  }

  /// The key store's [name], as a copy this engine may zero once done
  /// with it: what the platform channel hands over is an unmodifiable view
  /// (Abbacchio: zeroing it threw, and messages never started after a
  /// restart). The channel's own buffer can't be zeroed from Dart (Fugo:
  /// accepted residue).
  Future<Uint8List?> _keptKey(DmKeyName name) async {
    final bytes = await keys.get(name);
    return bytes == null ? null : Uint8List.fromList(bytes);
  }

  Future<Uint8List> _keyOrNew(DmKeyName name) async {
    final kept = await _keptKey(name);
    if (kept != null) return kept;
    final made = await _newSecret();
    await keys.put(name, made);
    return made;
  }

  /// Sets up messages with this as the person's first device: a new
  /// security key (the identity), kept in the key store before anything
  /// else, this device alone on the list, its key packages and the self
  /// group.
  Future<void> setUpFirst() async {
    final device = _device;
    if (device == null || setup != DmSetup.firstDevice) return;
    try {
      final identity = await _newSecret();
      await keys.put(DmKeyName.identity, identity);
      final made = await device.setUpFirst(
        identityKey: identity,
        accountId: accountId,
        leafId: leafId,
        name: deviceName,
        nowMs: BigInt.from(_nowMs()),
      );
      identity.fillRange(0, identity.length, 0);
      await routes.putKeys(
        identityKey: made.identityPublic,
        deviceList: made.deviceList,
        version: 1,
        previousVersion: 0,
      );
      await _topUpKeyPackages(force: true);
      _set(DmSetup.ready);
      await sync();
      await _makeSelfGroup(device);
    } on UserAppError catch (error) {
      problem = error.toString();
      if (error.problem == UserAppProblem.identityChanged ||
          error.problem == UserAppProblem.stale) {
        // Another device was set up first: this one is added from it.
        await keys.delete(DmKeyName.identity);
        await _forgetStore();
        return _set(DmSetup.addThisDevice);
      }
      _fail(DmFailure.refused);
    }
  }

  /// The person's self group, made by their first device.
  Future<void> _makeSelfGroup(DmDevice device) async {
    if (groups.values.any((each) => each.kind == 'self')) return;
    final group = DmServerGroup(
      groupId: dmB64(_randomBytes(16)),
      kind: 'self',
      accounts: [accountId],
    );
    final info = await device.createGroup(
      groupId: group.id,
      kind: group.kindFor(accountId)!,
    );
    try {
      await routes.createGroup(group, info);
      groups[group.groupId] = group;
    } on UserAppError catch (error) {
      await device.forgetGroup(groupId: group.id);
      if (error.problem != UserAppProblem.exists) rethrow;
      await sync();
    }
  }

  DmHandoff? _offer;

  /// Counts [offerThisDevice]'s asks, so one overtaken (by a newer one, or
  /// the device set up meanwhile) leaves no code behind.
  var _offerCall = 0;

  /// A hand-off is being taken ([takeHandoff]).
  var _taking = false;

  /// No code any more: the device is set up, or failed.
  void _dropOffer() {
    _offerCall++;
    _offer?.dispose();
    _offer = null;
  }

  /// This device's QR code's text while it waits to be added; null when
  /// it isn't waiting.
  String? get offer => _offer?.qr();

  /// The digits beside [offer], for a code that's pasted rather than
  /// scanned: the adding device shows the same ones.
  String? get offerDigits => _offer?.comparison();

  /// How long ago [offer] was made.
  Duration get offerAge => Duration(milliseconds: _nowMs() - _offerMadeMs);
  var _offerMadeMs = 0;

  /// Starts waiting to be added from one of the person's devices that's set
  /// up: the returned text is the QR code to show it. [platform] is what
  /// the other device shows ("iPadOS"). The code names this device's own
  /// endpoint for the history (E8).
  /// The last [offerThisDevice] couldn't make a code.
  var offerFailed = false;

  Future<String?> offerThisDevice({required String platform}) async {
    final device = _device;
    // Not while a hand-off is being taken: its history comes to the
    // endpoint the offer it opened named.
    if (device == null || setup != DmSetup.addThisDevice || _taking) {
      return null;
    }
    _offer?.dispose();
    _offer = null;
    offerFailed = false;
    _offerMadeMs = _nowMs();
    final call = ++_offerCall;
    // Still waiting to be added, and still the latest ask: a code made
    // after the device was set up would take every hand-off that came for
    // it as its own (Abbacchio's R5: the new key's, after Lost or stolen).
    bool current() =>
        !_closed && call == _offerCall && setup == DmSetup.addThisDevice;
    try {
      // Where the history comes to (E8): an endpoint of its own for this
      // code, or none when the relay won't admit one now.
      await _closeReceiver();
      if (!current()) return null;
      final receiver = await _admitted();
      if (!current()) {
        await receiver?.close();
        return null;
      }
      _receiver = receiver;
      final made = await device.startHandoff(
        leafId: leafId,
        name: _oneLine(deviceName, 64),
        platform: _oneLine(platform, 32),
        endpointId: receiver?.id() ?? Uint8List(32),
      );
      if (!current()) {
        made.dispose();
        return null;
      }
      _offer = made;
    } catch (error) {
      if (!current()) return null;
      // Said, with Try again, never a code that never comes.
      problem = '$error';
      offerFailed = true;
      _notify();
      return null;
    }
    _notify();
    // The worker's outbox says when a hand-off comes.
    unawaited(_resume());
    return _offer!.qr();
  }

  /// The new device's endpoint for its history, from its offer until the
  /// history came over, was skipped, or 30 minutes passed.
  DmTransferEndpoint? _receiver;
  int _receiverMs = 0;
  StreamSubscription<DmTransferProgress>? _receiving;

  /// How the history coming to this new device stands: null for none.
  DmTransferProgress? transfer;

  /// When this device was added: older messages are on the others until
  /// the history comes over.
  int historyFromMs = 0;

  /// The person chose Skip.
  var transferSkipped = false;

  /// How long a transfer's key lives (Fugo): the endpoint goes with it.
  static const _transferLife = Duration(minutes: 30);

  /// A transfer endpoint the relay admits (its device assertion, Pericolo:
  /// 401 or 429 is "can't start"); null when it can't be had.
  Future<DmTransferEndpoint?> _admitted() async {
    DmTransferEndpoint? endpoint;
    try {
      endpoint = await _bindTransfer();
      if (await _admitTransfer(endpoint.idHex())) return endpoint;
    } catch (error) {
      problem = '$error';
    }
    await endpoint?.close();
    return null;
  }

  static Future<bool> Function(String idHex) _assertionFor(
    UserAppsSession session,
  ) => (idHex) async {
    try {
      return (await session.account.assertionFor(idHex)).isNotEmpty;
    } catch (_) {
      return false;
    }
  };

  Future<void> _closeReceiver() async {
    unawaited(_receiving?.cancel());
    _receiving = null;
    final receiver = _receiver;
    _receiver = null;
    await receiver?.close();
  }

  /// Takes the history in, as it comes, into this device's store.
  void _receiveHistory() {
    final receiver = _receiver;
    final device = _device;
    if (device == null) return;
    if (receiver == null) {
      // No endpoint the relay admitted: the history stays on the others.
      transfer = const DmTransferProgress(
        chatsDone: 0,
        chatsTotal: 0,
        end: DmTransferEnd.skipped,
      );
      return;
    }
    _receiverMs = _offerMadeMs;
    transferSkipped = false;
    transfer = const DmTransferProgress(chatsDone: 0, chatsTotal: 0);
    unawaited(_receiving?.cancel());
    _receiving = receiver
        .receive(device: device)
        .listen(
          (progress) {
            transfer = progress;
            // The chats fill in as they come.
            for (final group in groups.values) {
              _moved(group.groupId);
            }
            if (progress.end case final end?) unawaited(_transferEnded(end));
            _notify();
          },
          onError: (Object error) {
            problem = '$error';
            transfer = DmTransferProgress(
              chatsDone: transfer?.chatsDone ?? 0,
              chatsTotal: transfer?.chatsTotal ?? 0,
              end: DmTransferEnd.skipped,
            );
            unawaited(_transferEnded(DmTransferEnd.skipped));
            _notify();
          },
        );
  }

  Future<void> _transferEnded(DmTransferEnd end) async {
    _receiving = null;
    // Kept for Try again only after one that didn't all come, by itself.
    if (end == DmTransferEnd.complete || transferSkipped) {
      final receiver = _receiver;
      _receiver = null;
      await receiver?.close();
    }
    if (end == DmTransferEnd.complete) transfer = null;
    _notify();
  }

  /// Closes the endpoint kept for Try again once its 30 minutes are up
  /// (Fugo: never left bound in the background).
  Future<void> _expireReceiver() async {
    if (_receiver == null || _offer != null) return;
    if (_nowMs() - _receiverMs < _transferLife.inMilliseconds) return;
    await _closeReceiver();
    if (transfer case final now? when now.end == null) {
      transfer = DmTransferProgress(
        chatsDone: now.chatsDone,
        chatsTotal: now.chatsTotal,
        end: DmTransferEnd.cutShort,
      );
    }
    _notify();
  }

  /// Skip: the history stays on the person's other devices.
  Future<void> skipTransfer() async {
    transferSkipped = true;
    _receiver?.cancel();
    await _closeReceiver();
    transfer = DmTransferProgress(
      chatsDone: transfer?.chatsDone ?? 0,
      chatsTotal: transfer?.chatsTotal ?? 0,
      end: DmTransferEnd.skipped,
    );
    _notify();
  }

  /// OK on what didn't come over: the note goes, and so does the endpoint.
  Future<void> dismissTransfer() async {
    await _closeReceiver();
    transfer = null;
    _notify();
  }

  /// Whether Try again can ask for the history again: the endpoint is still
  /// here, within its 30 minutes.
  bool get canRetryTransfer =>
      _receiver != null &&
      !transferSkipped &&
      _nowMs() - _receiverMs < _transferLife.inMilliseconds;

  /// Try again: receiving again, then asking the device that added this one
  /// to send what's missing (a `history_again` in the self group).
  Future<void> retryTransfer() async {
    if (!canRetryTransfer) {
      await _closeReceiver();
      _notify();
      return;
    }
    _receiveHistory();
    _notify();
    final self = groups.values.where((each) => each.kind == 'self').firstOrNull;
    if (self == null) return;
    await say(
      self.groupId,
      DmContent(
        kind: 'history_again',
        clientMessageId: _newId(),
        extraJson: '',
      ),
    );
  }

  /// Sends this device's history to the new device [leaf] it just added,
  /// over a fresh endpoint the relay admits.
  Future<void> _sendHistory(String leaf) async {
    final device = _device;
    if (device == null) return;
    final sender = await _admitted();
    if (sender == null) return;
    try {
      await sender.sendHistory(device: device, toLeaf: leaf);
    } catch (error) {
      problem = '$error';
    } finally {
      await sender.close();
    }
  }

  /// What came for this device's offer: the person's identity and pins,
  /// opened only with the offer's own keys. One that doesn't open is
  /// dropped, and the offer is spent: a new one is shown.
  Future<void> takeHandoff() async {
    final device = _device;
    final offer = _offer;
    // Only a device waiting to be added: on one that's set up, what comes
    // to me/handoffs is a new key's bundle, its own to take (Fugo).
    if (device == null ||
        offer == null ||
        setup != DmSetup.addThisDevice ||
        _taking) {
      return;
    }
    _taking = true;
    try {
      await _takeHandoff(device, offer);
    } finally {
      _taking = false;
    }
  }

  Future<void> _takeHandoff(DmDevice device, DmHandoff offer) async {
    final waiting = await routes.handoffs();
    for (final item in waiting) {
      var opened = false;
      try {
        final done = await device.finishHandoff(
          handoff: offer,
          sealed: item.sealed,
          nowMs: BigInt.from(_nowMs()),
        );
        opened = true;
        await keys.put(DmKeyName.identity, done.identityKey);
        done.identityKey.fillRange(0, done.identityKey.length, 0);
        await routes.deleteHandoff(item.id);
        _offer = null;
        offer.dispose();
        final mine = await routes.keys(accountId);
        if (mine != null) {
          await device.acceptList(
            account: accountId,
            deviceList: mine.deviceList,
          );
        }
        await _topUpKeyPackages(force: true);
        _set(DmSetup.ready);
        // The history from the device that added this one, while it joins
        // every group of the person's by external commit.
        historyFromMs = _nowMs();
        _receiveHistory();
        await sync();
        return;
      } catch (error) {
        problem = '$error';
        // One that doesn't open spends the offer: one try per code. One
        // that did is taken already.
        if (!opened) {
          await routes.deleteHandoff(item.id);
          _offer = null;
          offer.dispose();
        }
        _notify();
        return;
      }
    }
  }

  /// On a device that's set up: what [qr] (scanned from a new device) says
  /// of it, with the person's identity sealed for it, to ask the person
  /// before [addDevice].
  Future<DmHandoffSealed> scanned(String qr) => _device!.sealHandoff(qr: qr);

  /// Adds the device [sealed] is for: a new device list naming it, then its
  /// hand-off through the worker. It joins the person's groups itself.
  Future<void> addDevice(DmHandoffSealed sealed) async {
    final device = _device;
    if (device == null) return;
    await _changeList(
      device,
      (leaves) => [
        for (final leaf in leaves)
          if (leaf.leafId != sealed.leafId) leaf,
        DmListed(
          kind: DmLeafKind.device,
          leafId: sealed.leafId,
          signatureKey: sealed.leafSignatureKey,
        ),
      ],
    );
    await routes.handOff(sealed.leafId, sealed.sealed);
    // Then this device's history, straight to it (E8).
    unawaited(_sendHistory(sealed.leafId));
    _notify();
  }

  /// Removes the person's device [leaf]: a new list without it, and out of
  /// every group it's in. Throws when a group couldn't be committed, after
  /// trying the rest: asked again, it carries on where it stopped.
  Future<void> removeDevice(String leaf) async {
    final device = _device;
    if (device == null || leaf == leafId) return;
    // Asked again (a sign-out or a new key to try again): off the list
    // once, out of any group it's still in.
    final listed =
        device
            .deviceList(account: accountId)
            ?.leaves
            .any((each) => each.leafId == leaf) ??
        true;
    if (listed) {
      await _changeList(
        device,
        (leaves) => [
          for (final each in leaves)
            if (each.leafId != leaf) each,
        ],
      );
    }
    Object? stopped;
    // The groups as they are now: a sync while this waits on the worker
    // reads them again (Abbacchio's R5b).
    for (final group in await _groupsHere(device)) {
      try {
        await _removeFrom(device, group, [leaf]);
      } catch (error) {
        stopped ??= error;
      }
    }
    _notify();
    if (stopped != null) {
      problem = '$stopped';
      throw stopped;
    }
  }

  /// The worker's groups this device is in, as a list of its own.
  Future<List<DmServerGroup>> _groupsHere(DmDevice device) async {
    final here = {
      for (final group in await device.groups()) dmB64(group.groupId),
    };
    return [
      for (final group in [...groups.values])
        if (here.contains(group.groupId)) group,
    ];
  }

  /// Takes those of [leaves] still in [group] out of it in one commit
  /// (every leaf, checked or not: once off the list a leaf fails the
  /// checks, so it's no member any more, but it's still in the group until
  /// this commit, Abbacchio's R5). Behind the group's epoch (a sync moved
  /// it meanwhile), once more after catching up.
  Future<void> _removeFrom(
    DmDevice device,
    DmServerGroup group,
    List<String> leaves,
  ) async {
    for (var attempt = 0; ; attempt++) {
      final inGroup = await device.leafIds(groupId: group.id);
      final out = [
        for (final leaf in leaves)
          if (inGroup.contains(leaf)) leaf,
      ];
      if (out.isEmpty) return;
      try {
        return await _commitRemove(device, group, out);
      } on UserAppError catch (error) {
        if (error.problem != UserAppProblem.epoch || attempt > 0) rethrow;
        await _catchUpOne(device, group.groupId);
      }
    }
  }

  /// The person's devices taken off their list but still in a group here
  /// (a removal that stopped part-way), by leaf, with the name each leaf
  /// claims: unchecked, so only to show and remove. None while the
  /// person's key is being settled.
  Future<Map<String, String>> _removedButIn(DmDevice device) async {
    final list = device.deviceList(account: accountId);
    if (list == null || _ownKeyCheck) return const {};
    final listed = {for (final leaf in list.leaves) leaf.leafId};
    return {
      for (final group in await _groupsHere(device))
        for (final leaf in await device.leaves(groupId: group.id))
          if (leaf.kind == DmLeafKind.device &&
              leaf.account == accountId &&
              leaf.leafId != leafId &&
              !listed.contains(leaf.leafId))
            leaf.leafId: leaf.name,
    };
  }

  /// The person's devices off their list that are still in a group here
  /// (Fugo, G5's device twin), committed out of each: a removal that
  /// stopped part-way is finished at the next sync, whatever stopped it.
  /// The list is read again from the worker first, so a device another of
  /// the person's devices just added is never taken for one removed.
  Future<void> _sweepRemovedDevices(DmDevice device) async {
    if ((await _removedButIn(device)).isEmpty) return;
    await refreshKeys(accountId);
    final strays = await _removedButIn(device);
    if (strays.isEmpty) return;
    // Kept as under way first: a sign-out that doesn't go through shows in
    // Your devices with Try again, and is carried on at the next launch.
    await _beginRemoval(
      device,
      remove: [...strays.keys],
      names: [...strays.values],
      newKey: false,
    );
    for (final group in await _groupsHere(device)) {
      if (held.containsKey(group.groupId)) continue;
      try {
        await _removeFrom(device, group, [...strays.keys]);
      } catch (error) {
        problem = '$error';
      }
    }
    // Signed out of the account too, as Remove does (Fugo): off the list
    // only by the person's own removal, so this is what they meant, and a
    // device still signed in could act on the account (Lost or stolen… on
    // the person's own Mac). Signed out whatever group is left (a held
    // one shows as Not fully removed).
    for (final leaf in strays.keys) {
      if (await signOutDevice(leaf) == null) await _removed(device, leaf);
    }
    _notify();
  }

  /// The person's devices still in a group here though off their list,
  /// by leaf with the name they claim, for Your devices to finish
  /// removing; those a removal under way names are left to it.
  Future<Map<String, String>> notFullyRemoved() async {
    final device = _device;
    if (device == null || setup != DmSetup.ready) return const {};
    final removing = removal?.remove ?? const <String>[];
    return {
      for (final MapEntry(:key, :value) in (await _removedButIn(
        device,
      )).entries)
        if (!removing.contains(key)) key: value,
    };
  }

  /// Finishes removing [leaf], which is off the list but still in a group
  /// here: out of each, then signed out. Null once done; otherwise what
  /// stopped it.
  Future<DmRemoveStop?> finishRemoving(String leaf, String name) =>
      remove(leaf, name);

  /// The removal under way on this device, kept in its store from the
  /// moment the person confirms it until it's done (R5b): what Your
  /// devices shows with Try again while it isn't. Null when there's none.
  DmRemoval? removal;

  /// A removal is running now.
  bool get removing => _removing > 0;
  var _removing = 0;

  /// Keeps [remove] (with their [names]) as under way, with any removal
  /// already under way: one row, so neither is lost.
  Future<void> _beginRemoval(
    DmDevice device, {
    required List<String> remove,
    required List<String> names,
    List<String> keep = const [],
    required bool newKey,
  }) async {
    final before = await device.removal();
    final named = <String, String>{
      if (before != null)
        for (final (i, leaf) in before.remove.indexed)
          leaf: i < before.names.length ? before.names[i] : '',
      for (final (i, leaf) in remove.indexed)
        leaf: i < names.length ? names[i] : '',
    };
    final kept = newKey ? keep : before?.keep ?? keep;
    final next = DmRemoval(
      remove: [...named.keys],
      names: [...named.values],
      keep: [
        for (final leaf in kept)
          if (!named.containsKey(leaf)) leaf,
      ],
      newKey: newKey || (before?.newKey ?? false),
    );
    await device.beginRemoval(removal: next);
    removal = next;
  }

  /// [leaf] is removed and signed out: off the removal under way, unless
  /// a new key follows (that ends with the new key).
  Future<void> _removed(DmDevice device, String leaf) async {
    final now = await device.removal();
    if (now == null || now.newKey) return;
    final i = now.remove.indexOf(leaf);
    if (i < 0) return;
    if (now.remove.length == 1) {
      await device.endRemoval();
      removal = null;
      return;
    }
    final next = DmRemoval(
      remove: [...now.remove]..removeAt(i),
      names: [
        for (final (j, name) in now.names.indexed)
          if (j != i) name,
      ],
      keep: now.keep,
      newKey: false,
    );
    await device.beginRemoval(removal: next);
    removal = next;
  }

  /// Remove…: the person's device [leaf] (named [name]) off the list, out
  /// of every group and signed out, kept as under way until it is. Null
  /// once done; otherwise what stopped it, to try again (or carried on at
  /// the next launch).
  Future<DmRemoveStop?> remove(String leaf, String name) async {
    final device = _device;
    if (device == null || setup != DmSetup.ready) return DmRemoveStop.failed;
    _removing++;
    _notify();
    try {
      await _beginRemoval(device, remove: [leaf], names: [name], newKey: false);
      await removeDevice(leaf);
      switch (await signOutDevice(leaf)) {
        case null:
          await _removed(device, leaf);
          return null;
        case UserAppProblem.unreachable:
          return DmRemoveStop.signOutUnreachable;
        case _:
          return DmRemoveStop.signOutRefused;
      }
    } catch (error) {
      problem = '$error';
      return DmRemoveStop.failed;
    } finally {
      _removing--;
      _notify();
    }
  }

  /// Carries on with the removal under way (Your devices' Try again, and
  /// each launch): Lock out through its new key, Remove through each
  /// sign-out. Null once done or when there's none.
  Future<Object?> finishRemoval() async {
    final device = _device;
    if (device == null || setup != DmSetup.ready || removing) return null;
    try {
      final now = removal = await device.removal();
      _notify();
      if (now == null) return null;
      if (now.newKey) {
        return await changeKey(remove: now.remove, keep: now.keep);
      }
      for (final (i, leaf) in [...now.remove].indexed) {
        final stopped = await remove(
          leaf,
          i < now.names.length ? now.names[i] : '',
        );
        if (stopped != null) return stopped;
      }
      return null;
    } catch (error) {
      problem = '$error';
      return DmRemoveStop.failed;
    }
  }

  /// Signs the person's removed device [leaf] out of their account (L0,
  /// Fugo), after [removeDevice]: the service and the worker answer it no
  /// more, whatever keys it holds. Null once it's signed out, or already
  /// was; otherwise why not (unreachable: it may have been), so the person
  /// can try again.
  Future<UserAppProblem?> signOutDevice(String leaf) async {
    try {
      await session.post('/v1/devices/remove', {'account_key': leaf});
      return null;
    } on UserAppError catch (error) {
      problem = error.toString();
      return error.problem;
    }
  }

  // Lost or stolen: a new security key (dms-e2ee §2.1, R4).

  /// Set when the worker serves another key for the person than this
  /// device has: settled by [_followOwnKey] once the self group is read.
  var _ownKeyCheck = false;

  /// Groups this device hasn't moved to the person's new key in yet:
  /// nothing is sent there until it has.
  final _unmoved = <String>{};

  /// Groups whose feed waits at a commit under a key not pinned yet, and
  /// since when (by [_nowMs]).
  final held = <String, int>{};

  /// How long a group waits for a key before Join again is offered (Fugo).
  static const heldFor = Duration(days: 1);

  /// A group held a day or more: [rejoinHeld] is the way out.
  bool heldLong(String group) =>
      _nowMs() - (held[group] ?? _nowMs()) >= heldFor.inMilliseconds;

  /// Joins [group] again (by external commit), past what it waits at.
  Future<void> rejoinHeld(String group) async {
    final device = _device;
    final known = groups[group];
    if (device == null || known == null) return;
    held.remove(group);
    _rejoinAfter.remove(group);
    await _rejoin(device, known);
    _notify();
  }

  /// The name of the device that made the person's new security key,
  /// when this device took it from there: shown once (Fugo M2).
  String? newKeyFrom;

  void dismissNewKey() {
    newKeyFrom = null;
    _notify();
  }

  /// The person's new key is still being finished on this device: a
  /// rotation under way, or groups it hasn't moved in yet.
  bool get finishingKey => _changingKey || _unmoved.isNotEmpty;
  var _changingKey = false;

  /// The person's Claktories whose channel is here but that aren't on the
  /// device list: after a new key, each until the app connects to it.
  List<String> get claktoriesToFinish {
    final listed = {
      for (final leaf
          in _device?.deviceList(account: accountId)?.leaves ??
              const <DmListed>[])
        ?claktoryOf(leaf.leafId),
    };
    return [
      for (final group in groups.values)
        if (group.kind == 'channel' && !listed.contains(group.claktoryId))
          group.claktoryId,
    ];
  }

  /// Lost or stolen (dms-e2ee §2.1): removes [remove] (the lost device,
  /// and any the person didn't keep) and signs each out of their account
  /// first (Fugo: before the new key, so a stolen device can't make one),
  /// then makes a new security key for [keep] and this device. The same
  /// lists again carry on where it stopped. Null once done; otherwise what
  /// stopped it.
  Future<DmKeyChange?> changeKey({
    required List<String> remove,
    required List<String> keep,
    List<String> names = const [],
  }) async {
    final device = _device;
    if (device == null || setup != DmSetup.ready) return DmKeyChange.failed;
    _changingKey = true;
    _removing++;
    _notify();
    try {
      // Under way already: carried on as it was confirmed.
      if (await device.rotation() case final rotation?) {
        return await _carryOnKeyChange(device, rotation.kept, rotation.removed);
      }
      // Kept from now until the new key is taken (R5b): a dialog closed
      // or the app quit part-way carries on at the next launch, even once
      // the device is off the list.
      await _beginRemoval(
        device,
        remove: [
          for (final leaf in remove)
            if (leaf != leafId) leaf,
        ],
        names: [
          for (final (i, leaf) in remove.indexed)
            if (leaf != leafId) i < names.length ? names[i] : '',
        ],
        keep: keep,
        newKey: true,
      );
      final lockOut = removal!;
      for (final leaf in lockOut.remove) {
        await removeDevice(leaf);
        if (await signOutDevice(leaf) != null) {
          return DmKeyChange.signOutFailed;
        }
      }
      return await _carryOnKeyChange(device, [
        for (final leaf in lockOut.keep)
          if (leaf != leafId) leaf,
      ], lockOut.remove);
    } catch (error) {
      problem = '$error';
      return DmKeyChange.failed;
    } finally {
      _changingKey = false;
      _removing--;
      _notify();
    }
  }

  /// The new key from where it got to: made (or the one kept), handed to
  /// [keep] over the self group while the old key is still the person's,
  /// given to the worker, taken here, then each group moved.
  Future<DmKeyChange?> _carryOnKeyChange(
    DmDevice device,
    List<String> keep,
    List<String> removed,
  ) async {
    _changingKey = true;
    try {
      var rotation = await device.rotation();
      var next = await _keptKey(DmKeyName.identityNext);
      if (next == null) {
        if (rotation != null) await device.dropRotation();
        rotation = null;
        next = await _newSecret();
        await keys.put(DmKeyName.identityNext, next);
      }
      rotation ??= await device.prepareRotation(
        identityKey: next,
        kept: keep,
        removed: removed,
        nowMs: BigInt.from(_nowMs()),
      );
      if (!rotation.said) {
        final self = groups.values
            .where((each) => each.kind == 'self')
            .firstOrNull;
        if (self == null) return DmKeyChange.failed;
        for (final leaf in rotation.kept) {
          await routes.handOff(leaf, rotation.sealed);
        }
        // Its retry id is the new key's, so a crash sends the same one.
        final id = 'rotation-${dmB64(rotation.identityPublic)}';
        await say(
          self.groupId,
          DmContent(
            kind: 'rotation',
            clientMessageId: id,
            extraJson: jsonEncode({
              'key': dmB64(rotation.key),
              'identity': dmB64(rotation.identityPublic),
              'removed': rotation.removed,
            }),
          ),
        );
        if (sending[id]?.state != DmSendState.sent) return DmKeyChange.failed;
        await device.rotationSaid();
      }
      try {
        await routes.rotateKeys(
          identityKey: rotation.identityPublic,
          deviceList: rotation.deviceList,
        );
      } on UserAppError {
        // It may have gone through already (a crash, a lost answer).
        final served = await routes.keys(accountId);
        if (!listEquals(served?.identityKey, rotation.identityPublic)) {
          rethrow;
        }
      }
      await device.finishRotation(identityKey: next);
      // The Lock out it finishes ended with it, in the store.
      removal = null;
      await keys.put(DmKeyName.identity, next);
      await keys.delete(DmKeyName.identityNext);
      next.fillRange(0, next.length, 0);
      _ownKeyCheck = false;
      await _topUpKeyPackages(force: true);
      // Its Claktory enrolls again under the new key.
      _linkedMs = -1;
      await sync();
      return null;
    } on UserAppError catch (error) {
      problem = error.toString();
      return DmKeyChange.failed;
    } catch (error) {
      problem = '$error';
      return DmKeyChange.failed;
    } finally {
      _changingKey = false;
      _notify();
    }
  }

  /// A new key kept from a change this device didn't finish: true when
  /// this device's store has it already and only the key store was left
  /// behind (then the store opens again with it).
  Future<bool> _resumeKeyStore(DmDevice device) async {
    final next = await _keptKey(DmKeyName.identityNext);
    if (next == null) return false;
    if (await device.rotation() != null ||
        await device.pendingRotation() != null) {
      return false;
    }
    final own = device.pin(account: accountId);
    if (own != null &&
        !listEquals(own.identityPublic, device.status().identityPublic)) {
      await keys.put(DmKeyName.identity, next);
      await keys.delete(DmKeyName.identityNext);
      _device = null;
      device.dispose();
      return true;
    }
    await keys.delete(DmKeyName.identityNext);
    return false;
  }

  /// The person's own key as the worker has it, against this device's:
  /// taken from the self group's rotation message once the worker serves
  /// the key it names (Fugo: both, never either alone); waited for while
  /// the message has come and the worker hasn't; replaced elsewhere when
  /// the worker has another and no message names it.
  Future<void> _followOwnKey(DmDevice device) async {
    final pending = await device.pendingRotation();
    if (!_ownKeyCheck && pending == null) return;
    final served = await routes.keys(accountId);
    if (served == null) return;
    if (listEquals(served.identityKey, device.status().identityPublic)) {
      _ownKeyCheck = false;
      return;
    }
    if (pending != null &&
        listEquals(served.identityKey, pending.identityPublic)) {
      return _takeNewKey(device, pending);
    }
    if (pending != null) return;
    return _fail(DmFailure.identityReplaced);
  }

  /// Takes the new key [pending] named: its bundle from `me/handoffs`,
  /// from the device that sent the message, opened with the message's
  /// key; kept as `identity_next` before the store takes it.
  Future<void> _takeNewKey(DmDevice device, DmPendingRotation pending) async {
    final item = (await routes.handoffs())
        .where((each) => each.from == pending.fromLeaf)
        .lastOrNull;
    if (item == null) return;
    try {
      final next = await device.openRotation(sealed: item.sealed);
      await keys.put(DmKeyName.identityNext, next);
      await device.takeRotation(identityKey: next, sealed: item.sealed);
      await keys.put(DmKeyName.identity, next);
      await keys.delete(DmKeyName.identityNext);
      next.fillRange(0, next.length, 0);
    } catch (error) {
      // It doesn't open, or isn't the key named: never taken.
      problem = '$error';
      await device.dropPendingRotation();
      await routes.deleteHandoff(item.id);
      return _fail(DmFailure.identityReplaced);
    }
    await routes.deleteHandoff(item.id);
    _ownKeyCheck = false;
    newKeyFrom = pending.fromName;
    await _topUpKeyPackages(force: true);
    // Its Claktory enrolls again under the new key.
    _linkedMs = -1;
    _syncAgain = true;
    _notify();
  }

  /// Moves this device's leaf to the person's new key in each group it
  /// hasn't yet: catch up, move, and on 409 epoch the next sync again.
  Future<void> _moveGroups(DmDevice device) async {
    _unmoved.clear();
    for (final mine in await device.groups()) {
      if (!mine.moved) _unmoved.add(dmB64(mine.groupId));
    }
    if (_ownKeyCheck) return;
    for (final group in [..._unmoved]) {
      if (!groups.containsKey(group) || held.containsKey(group)) continue;
      final id = dmUnb64(group);
      try {
        final commit = await device.moveGroup(groupId: id);
        await routes.commit(group, commit);
        await _catchUpOne(device, group);
        _unmoved.remove(group);
      } on UserAppError catch (error) {
        await device.commitRefused(groupId: id);
        problem = error.toString();
        if (error.problem == UserAppProblem.epoch) {
          await _catchUpOne(device, group);
          _syncAgain = true;
        }
      }
    }
  }

  /// The person's devices, from their list, with the names their
  /// certificates give in the self group and since when (0 when it isn't
  /// in the self group).
  Future<List<DmOwnDevice>> devices() async {
    final device = _device;
    if (device == null) return const [];
    final list = device.deviceList(account: accountId);
    final self = groups.values.where((each) => each.kind == 'self').firstOrNull;
    final members = self == null
        ? const <DmMember>[]
        : await device.members(groupId: self.id);
    return [
      for (final leaf in list?.leaves ?? const <DmListed>[])
        if (leaf.kind == DmLeafKind.device)
          if (members.where((m) => m.leafId == leaf.leafId).firstOrNull
              case final member)
            (
              leafId: leaf.leafId,
              name: leaf.leafId == leafId ? deviceName : member?.name ?? '',
              here: leaf.leafId == leafId,
              sinceMs: member?.sinceMs.toInt() ?? 0,
            ),
    ];
  }

  /// Signs and puts the person's next device list, [change] of the last
  /// one this device took; once more from the worker's when another device
  /// changed it first.
  Future<void> _changeList(
    DmDevice device,
    List<DmListed> Function(List<DmListed> leaves) change,
  ) async {
    for (var attempt = 0; ; attempt++) {
      final now = device.deviceList(account: accountId);
      final version = now?.version.toInt() ?? 0;
      final signed = await device.signDeviceList(
        version: BigInt.from(version + 1),
        createdMs: BigInt.from(_nowMs()),
        leaves: change(now?.leaves ?? const []),
      );
      try {
        await routes.putKeys(
          deviceList: signed,
          version: version + 1,
          previousVersion: version,
        );
        return;
      } on UserAppError catch (error) {
        if (error.problem != UserAppProblem.stale || attempt > 0) rethrow;
        await refreshKeys(accountId);
      }
    }
  }

  Future<void> _resume() async {
    _pause?.cancel();
    if (_paused case final paused? when !paused.isCompleted) paused.complete();
  }

  /// Starts again on this device: its keys and store go (what's on the
  /// person's other devices stays there).
  Future<void> startOver() async {
    for (final name in DmKeyName.values) {
      await keys.delete(name);
    }
    await _forgetStore();
    await start();
  }

  Future<void> _forgetStore() async {
    _device?.dispose();
    _device = null;
    await _removeStore('${await keys.folder()}/store.db');
  }

  /// Keeps the worker's supply of this device's key packages up.
  Future<void> _topUpKeyPackages({bool force = false}) async {
    final device = _device;
    if (device == null) return;
    final left = force ? 0 : await routes.keyPackagesLeft();
    if (left >= _keyPackagesLow) return;
    await routes.uploadKeyPackages(
      await device.keyPackages(count: dmKeyPackages - left),
      lastResort: force ? await device.lastResortKeyPackage() : null,
    );
  }

  /// In step with the worker once: Welcomes, the groups it lists (joining
  /// those this device isn't in), each group's feed, and what's waiting to
  /// go. Calls made while one runs make it run once more.
  Future<void> sync() {
    if (_syncing != null) {
      _syncAgain = true;
      return _syncing!;
    }
    return _syncing = _syncLoop().whenComplete(() => _syncing = null);
  }

  Future<void> _syncLoop() async {
    do {
      _syncAgain = false;
      await _syncOnce();
    } while (_syncAgain && !_closed);
  }

  Future<void> _syncOnce() async {
    final device = _device;
    if (device == null || setup != DmSetup.ready) return;
    await _expireReceiver();
    try {
      for (final welcome in await routes.welcomes()) {
        await _join(device, welcome);
      }
      // Read first, then swapped in at once: never empty meanwhile for
      // what reads it.
      final served = await routes.groups();
      groups
        ..clear()
        ..addEntries([
          for (final group in served) MapEntry(group.groupId, group),
        ]);
      final here = {
        for (final group in await device.groups()) dmB64(group.groupId): group,
      };
      // The self group first: a change of the person's security key comes
      // there, before the moves it explains.
      final ordered = [
        ...groups.values.where((group) => group.kind == 'self'),
        ...groups.values.where((group) => group.kind != 'self'),
      ];
      for (final group in ordered) {
        final mine = here[group.groupId];
        if (mine == null) {
          await _rejoin(device, group);
        } else if (group.lastSeq > mine.seq) {
          await _catchUp(device, group, mine);
        }
        if (group.kind == 'self') await _followOwnKey(device);
        if (setup != DmSetup.ready) return;
      }
      await _moveGroups(device);
      // Devices off the person's list come out of any group they're still
      // in, before anything is sent (Fugo, R5b).
      await _sweepRemovedDevices(device);
      // Claktories whose access ended come out before anything is sent
      // (Fugo L6).
      await _reconcileGrantsSafely(device, adding: false);
      await _sendUnsent(device);
      await _topUpKeyPackages();
      await _linkClaktory(device);
      await _reconcileGrantsSafely(device, adding: true);
    } on UserAppError catch (error) {
      problem = error.toString();
    } catch (error) {
      // Whatever else stops a sync is said, never left to stop the next.
      problem = '$error';
    }
    _notify();
  }

  /// When the Claktory's leaf was last found enrolled, listed and in its
  /// channel, by [_nowMs]; it's checked again after a while (it may have
  /// signed in again: a new leaf).
  int _linkedMs = -1;
  static const _linkEvery = Duration(minutes: 10);

  /// Enrolls the Claktory this app shows, the first time: certifies its
  /// leaf with the person's identity, puts it on their device list, gives
  /// it the pins, then makes its channel with the person's devices and
  /// adds its leaf once its key packages are up.
  Future<void> _linkClaktory(DmDevice device) async {
    final link = claktory;
    if (link == null) return;
    if (_linkedMs >= 0 && _nowMs() - _linkedMs < _linkEvery.inMilliseconds) {
      return;
    }
    final DmLeafOffer offer;
    try {
      offer = await link.offer();
    } catch (error) {
      // Not connected, or this Claktory doesn't do messages yet.
      problem = '$error';
      return;
    }
    if (offer.claktoryId.isEmpty || offer.leafId.isEmpty) return;
    final listed =
        device
            .deviceList(account: accountId)
            ?.leaves
            .any((leaf) => leaf.leafId == offer.leafId) ??
        false;
    try {
      if (!listed) {
        // On the list in place of the same Claktory's older leaf.
        await _changeList(
          device,
          (leaves) => [
            for (final leaf in leaves)
              if (claktoryOf(leaf.leafId) != offer.claktoryId) leaf,
            DmListed(
              kind: DmLeafKind.claktory,
              leafId: offer.leafId,
              signatureKey: Uint8List.fromList(offer.leafSignatureKey),
            ),
          ],
        );
      }
      // Enrolled again after a new security key: off the list till now.
      if (!offer.enrolled || !listed) {
        final certificate = await device.issueCertificate(
          kind: DmLeafKind.claktory,
          leafId: offer.leafId,
          signatureKey: offer.leafSignatureKey,
          name: _oneLine(offer.name, 64),
          createdMs: BigInt.from(_nowMs()),
        );
        await link.enroll(certificate, await device.exportPins());
      }
      final DmServerGroup? channel;
      try {
        channel = await _channel(device, offer.claktoryId);
      } on UserAppError catch (error) {
        if (error.problem != UserAppProblem.notFound) rethrow;
        // The service makes a channel only with a Claktory on the person's
        // own account: this one is signed in to another. Said, and asked
        // again only after a while, not at every sync (Abbacchio, Fugo).
        problem =
            "This Claktory is signed in to another account, so it can't "
            'join your messages.';
        _linkedMs = _nowMs();
        return;
      }
      if (channel == null) return;
      if (await _hasLeaf(device, channel, offer.leafId) ||
          await _addClaktory(device, channel, offer.leafId)) {
        _linkedMs = _nowMs();
      }
    } on UserAppError catch (error) {
      problem = error.toString();
    } catch (error) {
      // Refused by the Claktory (its pins disagree with this device's, say):
      // tried again later.
      problem = '$error';
    }
  }

  /// The person's channel with [claktoryId], made now when there's none.
  Future<DmServerGroup?> _channel(DmDevice device, String claktoryId) async {
    DmServerGroup? find() => groups.values
        .where(
          (each) => each.kind == 'channel' && each.claktoryId == claktoryId,
        )
        .firstOrNull;
    if (find() case final found?) return found;
    final group = DmServerGroup(
      groupId: dmB64(_randomBytes(16)),
      kind: 'channel',
      accounts: [accountId],
      claktoryId: claktoryId,
    );
    final info = await device.createGroup(
      groupId: group.id,
      kind: group.kindFor(accountId)!,
    );
    try {
      await routes.createGroup(group, info);
    } on UserAppError catch (error) {
      await device.forgetGroup(groupId: group.id);
      if (error.problem == UserAppProblem.exists) {
        // Another device made it first: this one joins it on the next sync.
        _syncAgain = true;
        return null;
      }
      rethrow;
    }
    groups[group.groupId] = group;
    return group;
  }

  Future<bool> _hasLeaf(
    DmDevice device,
    DmServerGroup group,
    String leaf,
  ) async => (await device.leafIds(groupId: group.id)).contains(leaf);

  /// Adds the Claktory leaf [leaf] to [group] from one of its key packages,
  /// taking out the same Claktory's older leaves; false while it has none
  /// up yet.
  Future<bool> _addClaktory(
    DmDevice device,
    DmServerGroup group,
    String leaf,
  ) async {
    final claimed = await routes.claim(accountId, leafIds: [leaf]);
    if (claimed.isEmpty) return false;
    final claktoryId = claktoryOf(leaf);
    final older = [
      for (final member in await device.members(groupId: group.id))
        if (member.kind == DmLeafKind.claktory &&
            member.leafId != leaf &&
            claktoryOf(member.leafId) == claktoryId)
          member.leafId,
    ];
    if (older.isNotEmpty) await _commitRemove(device, group, older);
    final commit = await device.addMembers(
      groupId: group.id,
      keyPackages: [for (final each in claimed) each.keyPackage],
    );
    try {
      await routes.commit(group.groupId, commit);
    } on UserAppError {
      await device.commitRefused(groupId: group.id);
      rethrow;
    }
    await _catchUpOne(device, group.groupId);
    return true;
  }

  /// Takes [leaves] out of [group] in one commit.
  Future<void> _commitRemove(
    DmDevice device,
    DmServerGroup group,
    List<String> leaves,
  ) async {
    final commit = await device.removeMembers(
      groupId: group.id,
      leafIds: leaves,
    );
    try {
      await routes.commit(group.groupId, commit);
    } on UserAppError {
      await device.commitRefused(groupId: group.id);
      rethrow;
    }
    await _catchUpOne(device, group.groupId);
  }

  /// [_reconcileGrants], whatever goes wrong in it kept to it: the rest
  /// of a sync goes on.
  Future<void> _reconcileGrantsSafely(
    DmDevice device, {
    required bool adding,
  }) async {
    try {
      await _reconcileGrants(device, adding: adding);
    } catch (error) {
      problem = '$error';
      _grantsMs = -1;
    }
  }

  /// When the grants were last read, by [_nowMs]; -1 to read them at the
  /// next sync.
  int _grantsMs = -1;
  static const _grantsEvery = Duration(minutes: 1);

  /// The account's live DM grants, as last read: conversation id → the
  /// Claktories granted on it. Null until read.
  Map<String, Set<String>>? _wanted;

  /// The grants changed here (taken back in Agents' access), or the app
  /// came back to the front: they're read again, and the conversations
  /// follow at once.
  Future<void> grantsChanged() {
    _grantsMs = -1;
    return sync();
  }

  /// Each conversation's group holds the Claktories with a live DM grant
  /// on it, and no others (Fugo L6): a Claktory granted is added, its
  /// pins given first; one whose grant was taken back or ended is taken
  /// out, by whichever of the person's devices sees it first.
  ///
  /// Without [adding], only what comes out, at every sync, from the grants
  /// as last read (read again at most once a minute, and at once at
  /// launch and on coming back); with it, what goes in too.
  Future<void> _reconcileGrants(DmDevice device, {required bool adding}) async {
    if (_wanted == null ||
        _grantsMs < 0 ||
        _nowMs() - _grantsMs >= _grantsEvery.inMilliseconds) {
      final wanted = <String, Set<String>>{};
      for (final grant in await grantsApi.list()) {
        if (grant.app != 'dms') continue;
        for (final resource in grant.resources) {
          if (resource.kind != 'conversation') continue;
          wanted.putIfAbsent(resource.id, () => {}).add(grant.claktoryId);
        }
      }
      _wanted = wanted;
      _grantsMs = _nowMs();
    }
    final wanted = _wanted!;
    // Each Claktory's newest leaf on the person's list.
    final leafOf = <String, String>{};
    for (final leaf
        in device.deviceList(account: accountId)?.leaves ??
            const <DmListed>[]) {
      final claktoryId = claktoryOf(leaf.leafId);
      if (leaf.kind != DmLeafKind.claktory || claktoryId == null) continue;
      final known = leafOf[claktoryId];
      if (known == null || _epochOf(leaf.leafId) > _epochOf(known)) {
        leafOf[claktoryId] = leaf.leafId;
      }
    }
    // The current group of each conversation.
    final current = <String, DmServerGroup>{};
    for (final group in groups.values) {
      if (group.kind != 'conversation') continue;
      final known = current[group.conversationId];
      if (known == null || group.generation > known.generation) {
        current[group.conversationId] = group;
      }
    }
    final here = {
      for (final group in await device.groups()) dmB64(group.groupId),
    };
    for (final group in [...groups.values]) {
      if (group.kind != 'conversation' || !here.contains(group.groupId)) {
        continue;
      }
      final members = await device.members(groupId: group.id);
      final want = identical(current[group.conversationId], group)
          ? {
              for (final claktoryId
                  in wanted[group.conversationId] ?? const <String>{})
                ?leafOf[claktoryId],
            }
          : const <String>{};
      try {
        // This person's Claktories not wanted here, checked or not: one
        // taken off the list or under a replaced key fails the checks, yet
        // must still go (Fugo, G5). Only this person's: a friend's are
        // theirs to give the chat and take back.
        final out = [
          for (final leaf in await device.leaves(groupId: group.id))
            if (leaf.kind == DmLeafKind.claktory &&
                leaf.account == accountId &&
                !want.contains(leaf.leafId))
              leaf.leafId,
        ];
        if (out.isNotEmpty) await _commitRemove(device, group, out);
        if (!adding) continue;
        // Not while the friend's key change waits for the person.
        final friend = group.accounts.where((each) => each != accountId);
        if (friend.any((each) => _unseenChange(device, each))) continue;
        for (final leaf in want) {
          if (members.any((member) => member.leafId == leaf)) continue;
          final channel = groups.values
              .where(
                (each) =>
                    each.kind == 'channel' &&
                    each.claktoryId == claktoryOf(leaf),
              )
              .firstOrNull;
          // Its pins first, so it can check the friend's leaves.
          if (channel != null && here.contains(channel.groupId)) {
            await _sayPins(device, channel);
          }
          await _addClaktory(device, group, leaf);
        }
      } on UserAppError catch (error) {
        problem = error.toString();
        _grantsMs = -1;
      } catch (error) {
        problem = '$error';
      }
    }
  }

  Future<void> _join(DmDevice device, DmWelcomeItem welcome) async {
    final kind = welcome.group.kindFor(accountId);
    if (kind == null) return;
    try {
      if (kind.kind == DmGroupType.conversation) {
        for (final account in kind.accounts) {
          if (account != accountId) await refreshKeys(account);
        }
      }
      await device.joinGroup(
        groupId: welcome.group.id,
        kind: kind,
        welcome: welcome.welcome,
        seq: welcome.seq,
      );
      _moved(welcome.group.groupId);
    } catch (error) {
      // Not one this device can take (not for it, or its checks failed):
      // it joins by external commit instead, if it should be there.
      problem = '$error';
    }
    await routes.deleteWelcome(welcome.id);
  }

  /// Joins [group] by external commit: a group of the person's this device
  /// isn't in yet (a new device), or one it fell out of step with. Waits
  /// longer after each try that didn't take.
  Future<void> _rejoin(DmDevice device, DmServerGroup group) async {
    final kind = group.kindFor(accountId);
    if (kind == null) return;
    final after = _rejoinAfter[group.groupId];
    if (after != null && DateTime.now().isBefore(after)) return;
    final tries = (_rejoinTries[group.groupId] ?? 0) + 1;
    _rejoinTries[group.groupId] = tries;
    _rejoinAfter[group.groupId] = DateTime.now().add(
      retryAfter * min(1 << (tries - 1), 64),
    );
    try {
      if (kind.kind == DmGroupType.conversation) {
        for (final account in kind.accounts) {
          if (account != accountId) await refreshKeys(account);
        }
      }
      final info = await routes.groupInfo(group.groupId);
      final commit = await device.rejoinGroup(
        groupId: group.id,
        kind: kind,
        groupInfo: info,
        seq: group.lastSeq,
      );
      await routes.commit(group.groupId, commit);
      _moved(group.groupId);
      // Its commit comes back in the feed, which merges it.
      final mine = (await device.groups())
          .where((each) => dmB64(each.groupId) == group.groupId)
          .firstOrNull;
      if (mine != null) await _catchUp(device, group, mine);
    } on UserAppError catch (error) {
      problem = error.toString();
      if (error.problem == UserAppProblem.epoch) {
        // Another commit came first: a sync after the wait tries again.
        await device.forgetGroup(groupId: group.id);
      }
    } catch (error) {
      problem = '$error';
    }
  }

  /// Takes [group]'s feed after where this device is, in order.
  Future<void> _catchUp(
    DmDevice device,
    DmServerGroup group,
    DmGroup mine,
  ) async {
    var after = mine.seq;
    var epoch = mine.epoch.toInt();
    while (!_closed) {
      final ({List<DmFeedItem> items, bool hasMore}) page;
      try {
        page = await routes.feed(group.groupId, after);
      } on UserAppError catch (error) {
        if (error.problem == UserAppProblem.tooFarBehind) {
          _rejoinAfter.remove(group.groupId);
          return _rejoin(device, group);
        }
        rethrow;
      }
      for (final item in page.items) {
        Future<DmProcessed> take() => device.process(
          groupId: group.id,
          seq: item.seq,
          envelope: item.envelope,
          arrivedMs: item.acceptedMs,
        );
        var processed = await take();
        // A commit under a key not pinned yet (someone changed their
        // security key): a friend's keys are fetched and it's taken
        // again; the person's own wait for their self group. Until then
        // it's held, unread, and nothing after it is read (Ghiaccio, Fugo:
        // never dropped, so the group can go on once the key is in).
        if (processed.outcome == DmOutcome.held && processed.why != accountId) {
          await refreshKeys(processed.why);
          processed = await take();
        }
        if (processed.outcome == DmOutcome.held) {
          if (processed.why == accountId) _ownKeyCheck = true;
          held.putIfAbsent(group.groupId, _nowMs);
          return _notify();
        }
        held.remove(group.groupId);
        after = item.seq;
        switch (processed.outcome) {
          case DmOutcome.committed:
            epoch = processed.epoch.toInt();
            _rejoinTries.remove(group.groupId);
          case DmOutcome.dropped:
            // A handshake at this device's epoch or later that it can't
            // take: it's out of step, and joins again. One from before
            // it joined is only old.
            if (processed.handshake && item.epoch >= epoch) {
              return _rejoin(device, group);
            }
          case DmOutcome.message ||
              DmOutcome.proposal ||
              DmOutcome.already ||
              DmOutcome.held:
            break;
        }
        if (processed.message case final message?) {
          _confirmed(message);
          // A device this one added asks for its history again.
          if (group.kind == 'self' &&
              message.content.kind == 'history_again' &&
              !message.mine &&
              message.fromKindDevice &&
              message.fromAccount == accountId &&
              device.hasHistoryFor(leafId: message.fromLeaf)) {
            unawaited(_sendHistory(message.fromLeaf));
          }
        }
        _moved(group.groupId);
      }
      if (!page.hasMore || page.items.isEmpty) return;
    }
  }

  void _confirmed(DmMessage message) {
    if (message.mine) sending.remove(message.content.clientMessageId);
  }

  /// Sends what was encrypted here and hasn't come back: after a restart,
  /// or an answer that never came.
  Future<void> _sendUnsent(DmDevice device) async {
    for (final unsent in await device.unsent()) {
      // An agent's message goes only with the person's approval.
      if (unsent.content.byAgent != null) continue;
      final state = sending[unsent.clientMessageId];
      if (state != null && state.state != DmSendState.unknown) continue;
      await _post(device, unsent);
    }
  }

  /// Says [content] in [group]: encrypted once, kept until it comes back,
  /// and sent now. Where it stands is in [sending] by its retry id.
  Future<void> say(String group, DmContent content) async {
    final device = _device;
    if (device == null) return;
    sending[content.clientMessageId] = DmSending(
      DmSendState.sending,
      group: group,
    );
    _moved(group);
    _notify();
    try {
      await device.send(
        groupId: dmUnb64(group),
        content: content,
        nowMs: _nowMs(),
      );
    } catch (error) {
      problem = '$error';
      sending[content.clientMessageId] = DmSending(
        DmSendState.refused,
        group: group,
        text: content.text ?? '',
        problem: 'unencryptable',
      );
      return _notify();
    }
    final unsent = (await device.unsent())
        .where((each) => each.clientMessageId == content.clientMessageId)
        .firstOrNull;
    if (unsent != null) await _post(device, unsent);
    _notify();
  }

  /// Tries [clientId] again, as the same message.
  Future<void> retry(String clientId) async {
    final device = _device;
    if (device == null) return;
    final unsent = (await device.unsent())
        .where((each) => each.clientMessageId == clientId)
        .firstOrNull;
    if (unsent == null) return;
    sending[clientId] = DmSending(
      DmSendState.sending,
      group: dmB64(unsent.groupId),
    );
    _notify();
    await _post(device, unsent);
    _notify();
  }

  /// Gives up on [clientId]: it's never sent from here.
  Future<void> discard(String group, String clientId) async {
    await _device?.dropUnsent(
      groupId: dmUnb64(group),
      clientMessageId: clientId,
    );
    sending.remove(clientId);
    _moved(group);
    _notify();
  }

  Future<void> _post(DmDevice device, DmUnsent unsent) async {
    final group = dmB64(unsent.groupId);
    final id = unsent.clientMessageId;
    // Nothing goes from under a replaced key: it waits for this device's
    // move there (dms-e2ee §2.1).
    if (_unmoved.contains(group)) return;
    final known = groups[group];
    try {
      if (known != null && known.kind == 'conversation') {
        await routes.sayInConversation(
          known.conversationId,
          known.generation,
          id,
          unsent.envelope,
        );
      } else {
        await routes.sayInGroup(group, id, unsent.envelope);
      }
      // Sent: it shows as sent once it comes back in the feed.
      sending[id] = DmSending(DmSendState.sent, group: group);
    } on UserAppError catch (error) {
      switch (error.problem) {
        case UserAppProblem.unreachable || UserAppProblem.unavailable:
          sending[id] = DmSending(DmSendState.unknown, group: group);
        case UserAppProblem.epoch:
          // The group moved on first: catch up, and send it again as a
          // new message (the same id with other bytes is refused).
          await device.dropUnsent(groupId: unsent.groupId, clientMessageId: id);
          sending.remove(id);
          final fresh = _newId();
          await _catchUpOne(device, group);
          await say(
            group,
            DmContent(
              kind: unsent.content.kind,
              clientMessageId: fresh,
              text: unsent.content.text,
              byAgent: unsent.content.byAgent,
              extraJson: unsent.content.extraJson,
            ),
          );
          _replaced[id] = fresh;
        default:
          await device.dropUnsent(groupId: unsent.groupId, clientMessageId: id);
          sending[id] = DmSending(
            DmSendState.refused,
            group: group,
            problem: error.problem == UserAppProblem.gone
                ? 'closed'
                : error.problem.name,
            text: unsent.content.text ?? '',
          );
      }
    }
    _moved(group);
  }

  /// Messages sent again under a new retry id (after 409 epoch): old → new.
  final _replaced = <String, String>{};

  /// The retry id [clientId] goes by now.
  String current(String clientId) {
    var id = clientId;
    for (var next = _replaced[id]; next != null; next = _replaced[id]) {
      id = next;
    }
    return id;
  }

  /// Says something about a conversation to the person's own devices (a
  /// draft, a read position, a deletion), through the self group.
  Future<void> tellMyDevices(
    String kind,
    String conversationGroup, {
    String? text,
    int seq = 0,
  }) async {
    final self = groups.values.where((each) => each.kind == 'self').firstOrNull;
    if (self == null) return;
    await say(
      self.groupId,
      DmContent(
        kind: kind,
        clientMessageId: _newId(),
        text: text,
        extraJson: jsonEncode({
          'group': conversationGroup,
          if (kind == 'draft') 'at_ms': _nowMs() else 'seq': seq,
        }),
      ),
    );
    _moved(conversationGroup);
  }

  /// [account]'s keys, as the worker has them now, against what this
  /// device pinned: pinned at first contact, a changed key marked so the
  /// person sees it before anything is sent, and the device list taken.
  Future<void> refreshKeys(String account) async {
    final device = _device;
    if (device == null) return;
    final now = await routes.keys(account);
    if (now == null) return;
    final pin = device.pin(account: account);
    if (pin == null) {
      await device.firstContact(
        account: account,
        identityPublic: now.identityKey,
      );
    } else if (!listEquals(pin.identityPublic, now.identityKey)) {
      // The person's own key changes only through their self group
      // ([_followOwnKey]), never from the worker alone.
      if (account == accountId) {
        _ownKeyCheck = true;
        return;
      }
      if (pin.verified) wasVerified.add(account);
      await device.keyChanged(
        account: account,
        identityPublic: now.identityKey,
      );
    }
    try {
      await device.acceptList(account: account, deviceList: now.deviceList);
    } catch (error) {
      // Older than one already seen, or not signed by the pinned key:
      // refused, and nothing changes.
      problem = '$error';
    }
    for (final group in groups.values) {
      if (group.accounts.contains(account)) _moved(group.groupId);
    }
    _notify();
  }

  /// The person saw that [account]'s security key changed: sending to
  /// them goes on.
  Future<void> seen(String account) async {
    await _device?.seen(account: account);
    wasVerified.remove(account);
    await _sharePins();
    _notify();
  }

  /// The security code with [account] for the key pinned now; null
  /// before anything is pinned.
  DmSafety? safety(String account) {
    try {
      return _device?.safetyNumber(account: account);
    } catch (_) {
      return null;
    }
  }

  /// Whether [scanned] (the text of the QR code on [account]'s device)
  /// shows the same two keys as this side's; null for a code that isn't a
  /// security code.
  bool? safetyMatches(String account, String scanned) {
    final device = _device;
    if (device == null || !scanned.startsWith(dmSafetyPrefix)) return null;
    try {
      return device.safetyMatches(
        account: account,
        scanned: dmUnb64(scanned.substring(dmSafetyPrefix.length)),
      );
    } catch (_) {
      return null;
    }
  }

  /// Friends whose key changed while they were verified: the key-changed
  /// note says they no longer are.
  final wasVerified = <String>{};

  /// The person compared safety numbers with [account].
  Future<void> verify(String account) async {
    await _device?.verify(account: account);
    await _sharePins();
    _notify();
  }

  /// The pins, to the person's other devices and their Claktories.
  Future<void> _sharePins() async {
    final device = _device;
    if (device == null) return;
    for (final group in groups.values.toList()) {
      if (group.kind == 'self' || group.kind == 'channel') {
        await _sayPins(device, group);
      }
    }
  }

  /// Says the pins in [group]. Never to a Claktory while a friend's key
  /// change waits for the person (Fugo): what they haven't seen isn't
  /// something it may trust.
  Future<void> _sayPins(DmDevice device, DmServerGroup group) async {
    if (group.kind == 'channel' && _unseenChange(device)) return;
    await say(
      group.groupId,
      DmContent(
        kind: 'pins',
        clientMessageId: _newId(),
        extraJson: jsonEncode({
          'pins': jsonDecode(utf8.decode(await device.exportPins())),
        }),
      ),
    );
  }

  /// Whether a friend's security key changed and the person hasn't seen
  /// it yet.
  bool _unseenChange(DmDevice device, [String? account]) {
    final accounts = account != null
        ? [account]
        : {
            for (final group in groups.values)
              for (final each in group.accounts)
                if (each != accountId) each,
          };
    return accounts.any(
      (each) => device.pin(account: each)?.changedUnseen ?? false,
    );
  }

  /// The conversation group for [conversationId] at [generation], made
  /// now when there's none yet: with this device's own leaves and the
  /// friend's, from their key packages. The other side may have made it
  /// first (409 exists): then it comes as a Welcome.
  Future<DmServerGroup?> conversationGroup(
    String conversationId,
    int generation,
    String friend,
  ) async {
    final device = _device;
    if (device == null) return null;
    DmServerGroup? find() => groups.values
        .where(
          (each) =>
              each.kind == 'conversation' &&
              each.conversationId == conversationId &&
              each.generation == generation,
        )
        .firstOrNull;
    if (find() case final found?) return found;
    await refreshKeys(friend);
    final group = DmServerGroup(
      groupId: dmB64(_randomBytes(16)),
      kind: 'conversation',
      conversationId: conversationId,
      generation: generation,
      accounts: [accountId, friend]..sort(),
    );
    final info = await device.createGroup(
      groupId: group.id,
      kind: group.kindFor(accountId)!,
    );
    try {
      await routes.createGroup(group, info);
    } on UserAppError catch (error) {
      await device.forgetGroup(groupId: group.id);
      if (error.problem == UserAppProblem.exists) {
        await sync();
        return find();
      }
      rethrow;
    }
    groups[group.groupId] = group;
    await _addEveryone(device, group, [friend, accountId]);
    return group;
  }

  /// Adds every leaf of [accounts] that [group] doesn't have yet, from
  /// their key packages, in one commit with its Welcome.
  Future<void> _addEveryone(
    DmDevice device,
    DmServerGroup group,
    List<String> accounts,
  ) async {
    final packages = <String>[];
    for (final account in accounts) {
      // The person's own devices only: a Claktory comes in with a grant.
      final List<String>? only = account == accountId
          ? [
              for (final leaf
                  in device.deviceList(account: accountId)?.leaves ??
                      const <DmListed>[])
                if (leaf.kind == DmLeafKind.device && leaf.leafId != leafId)
                  leaf.leafId,
            ]
          : null;
      if (only != null && only.isEmpty) continue;
      for (final claimed in await routes.claim(account, leafIds: only)) {
        if (claimed.leafId != leafId) packages.add(claimed.keyPackage);
      }
    }
    if (packages.isEmpty) return;
    final commit = await device.addMembers(
      groupId: group.id,
      keyPackages: packages,
    );
    try {
      await routes.commit(group.groupId, commit);
    } on UserAppError {
      await device.commitRefused(groupId: group.id);
      rethrow;
    }
    await _catchUpOne(device, group.groupId);
  }

  /// Takes [group]'s feed now, outside a whole [sync].
  Future<void> _catchUpOne(DmDevice device, String group) async {
    final known = groups[group];
    final mine = (await device.groups())
        .where((each) => dmB64(each.groupId) == group)
        .firstOrNull;
    if (known != null && mine != null) await _catchUp(device, known, mine);
  }

  /// [group]'s history here, newest last.
  Future<List<DmMessage>> history(
    String group, {
    int? before,
    int limit = 50,
  }) async =>
      await _device?.history(
        groupId: dmUnb64(group),
        beforeSeq: before,
        limit: limit,
      ) ??
      const [];

  /// The person's draft, read position and deletion for [group].
  Future<DmView> view(String group) async =>
      await _device?.view(groupId: dmUnb64(group)) ??
      const DmView(draft: '', draftMs: 0, readSeq: 0, deletedSeq: 0);

  /// What waits to go to [group] from here, oldest first.
  Future<List<DmUnsent>> unsent(String group) async => [
    for (final unsent in await _device?.unsent() ?? const <DmUnsent>[])
      // Not an agent's message waiting for an approval to go with.
      if (dmB64(unsent.groupId) == group && unsent.content.byAgent == null)
        unsent,
  ];

  /// [envelope] from [claktoryId]'s channel, opened once (kept opened in
  /// the store): its content when the channel's certified leaf of that
  /// Claktory, of the person's own account, said it and it's a [kind];
  /// null for anything else, or what this device can't open.
  Future<DmContent?> _fromClaktory(
    String claktoryId,
    String envelope,
    String kind,
    int arrivedMs,
  ) async {
    final device = _device;
    if (device == null || envelope.isEmpty) return null;
    final channel = groups.values
        .where(
          (each) => each.kind == 'channel' && each.claktoryId == claktoryId,
        )
        .firstOrNull;
    if (channel == null) return null;
    try {
      // At the channel's epoch, or near it.
      await _catchUpOne(device, channel.groupId);
      final message = await device.openSide(
        groupId: channel.id,
        envelope: envelope,
        arrivedMs: arrivedMs,
      );
      if (message.fromAccount != accountId ||
          message.fromKindDevice ||
          claktoryOf(message.fromLeaf) != claktoryId ||
          message.content.kind != kind) {
        return null;
      }
      return message.content;
    } catch (error) {
      problem = '$error';
      return null;
    }
  }

  static Map<String, Object?> _extra(DmContent content) {
    if (content.extraJson.isEmpty) return const {};
    final extra = jsonDecode(content.extraJson);
    return extra is Map<String, Object?> ? extra : const {};
  }

  /// [proposal] as its agent's Claktory sealed it (dms-e2ee §6): the
  /// text, the agent and the conversation from inside, which must be the
  /// one the service listed it under; null when it doesn't open or
  /// doesn't match.
  Future<DmProposal?> openProposal(DmProposal proposal) async {
    final content = await _fromClaktory(
      proposal.claktoryId,
      proposal.envelope,
      'proposal',
      proposal.createdMs,
    );
    if (content == null) return null;
    final extra = _extra(content);
    String text(String key) => switch (extra[key]) {
      final String value => value,
      _ => '',
    };
    if (text('conversation_id') != proposal.conversationId ||
        extra['contact_generation'] != proposal.generation ||
        content.clientMessageId.isEmpty) {
      return null;
    }
    return proposal.opened(
      claktoryId: proposal.claktoryId,
      agentId: text('agent_id'),
      incarnation: text('incarnation'),
      agentName: _oneLine(text('agent_name'), 64),
      text: content.text ?? '',
      clientMessageId: content.clientMessageId,
    );
  }

  /// [draft] (an agent's suggestion in [conversation]) as its Claktory
  /// sealed it; null when it doesn't open or is another conversation's.
  Future<DmAgentDraft?> openAgentDraft(
    DmAgentDraft draft,
    String conversation,
  ) async {
    final content = await _fromClaktory(
      draft.claktoryId,
      draft.envelope,
      'agent_draft',
      draft.updatedMs,
    );
    if (content == null) return null;
    if (_extra(content)['conversation_id'] != conversation) return null;
    return draft.opened(
      claktoryId: draft.claktoryId,
      agentName: _oneLine(content.byAgent?.name ?? '', 64),
      text: content.text ?? '',
    );
  }

  /// [proposal], opened, sealed again in [group] as its agent's message
  /// for the person's approval: kept like a message from here until it
  /// comes back, but sent only with the approval ([ChatApi.approve]).
  /// Sealed afresh when [again] (the group moved on since).
  Future<String> sealApproved(
    String group,
    DmProposal proposal, {
    bool again = false,
  }) async {
    final device = _device!;
    if (again) {
      await device.dropUnsent(
        groupId: dmUnb64(group),
        clientMessageId: proposal.clientMessageId,
      );
      await _catchUpOne(device, group);
    }
    return device.send(
      groupId: dmUnb64(group),
      content: DmContent(
        kind: 'text',
        clientMessageId: proposal.clientMessageId,
        text: proposal.text,
        byAgent: DmByAgent(
          claktoryId: proposal.claktoryId,
          agentId: proposal.agentId,
          incarnation: proposal.incarnation,
          name: proposal.agentName,
        ),
        extraJson: '',
      ),
      nowMs: _nowMs(),
    );
  }

  /// An approval that can't go: what was sealed for it goes too.
  Future<void> dropApproved(String group, DmProposal proposal) async =>
      _device?.dropUnsent(
        groupId: dmUnb64(group),
        clientMessageId: proposal.clientMessageId,
      );

  /// What the person's pin of [account] says now.
  DmPin? pin(String account) => _device?.pin(account: account);

  void _set(DmSetup to) {
    if (to != DmSetup.addThisDevice) _dropOffer();
    setup = to;
    _notify();
    unawaited(_resume());
  }

  void _fail(DmFailure why) {
    _dropOffer();
    failure = why;
    setup = DmSetup.failed;
    _notify();
    unawaited(_resume());
  }

  void _notify() {
    if (!_closed) notifyListeners();
  }

  @override
  void dispose() {
    unawaited(_closeReceiver());
    _closed = true;
    _pause?.cancel();
    if (_paused case final paused? when !paused.isCompleted) paused.complete();
    unawaited(_events.close());
    unawaited(_groupChanges.close());
    _device?.dispose();
    super.dispose();
  }
}

/// Where a message from this device stands.
enum DmSendState {
  /// On its way; the worker hasn't answered.
  sending,

  /// The worker took it; it shows in the history once it comes back.
  sent,

  /// No answer came: it may or may not have been sent. Try again can't
  /// send it twice.
  unknown,

  /// Refused ([DmSending.problem]); it won't go.
  refused,
}

@immutable
class DmSending {
  const DmSending(
    this.state, {
    this.group = '',
    this.problem = '',
    this.text = '',
  });

  final DmSendState state;

  /// The group it's for.
  final String group;

  /// Why it was refused: `closed` (the conversation ended), the problem's
  /// name, or `unencryptable`.
  final String problem;

  /// A refused message's text, to keep showing it once it's dropped.
  final String text;
}

/// [text] on one line, with nothing hidden in it, at most [bytes] bytes of
/// UTF-8, as a certificate takes a name.
String _oneLine(String text, int bytes) {
  final line = text.replaceAll(RegExp(r'\s+'), ' ').trim();
  final out = StringBuffer();
  var used = 0;
  for (final rune in line.runes) {
    if (hiddenCharacter(rune)) continue;
    final size = utf8.encode(String.fromCharCode(rune)).length;
    if (used + size > bytes) break;
    used += size;
    out.writeCharCode(rune);
  }
  return out.toString();
}

final _random = Random.secure();

Uint8List _randomBytes(int count) =>
    Uint8List.fromList([for (var i = 0; i < count; i++) _random.nextInt(256)]);

String _randomId() => dmB64(_randomBytes(16));

String _text(Map<String, Object?> json, String key) => switch (json[key]) {
  final String text => text,
  _ => '',
};

int _int(Map<String, Object?> json, String key) => switch (json[key]) {
  final int number => number,
  _ => 0,
};

List<Map<String, Object?>> _list(Map<String, Object?> json, String key) => [
  for (final item in json[key] as List? ?? const [])
    if (item is Map<String, Object?>) item,
];
