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
import '../tunnel_client.dart' show RustTunnels;
import '../user_apps/session.dart';
import 'dm_keys.dart';
import '../user_apps/grants.dart';
import 'review.dart' show hiddenCharacter;

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
  account.addListener(() {
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
    this.retryAfter = const Duration(seconds: 5),
  }) : routes = DmRoutes(session),
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
      if (mine != null &&
          !listEquals(mine.identityKey, status.identityPublic)) {
        return _fail(DmFailure.identityReplaced);
      }
      if (mine != null) {
        await device.acceptList(
          account: accountId,
          deviceList: mine.deviceList,
        );
      }
      _set(DmSetup.ready);
      await sync();
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
    }
  }

  /// The store with this device's keys; made, with its keys, the first
  /// time. A store made with another key can't be read by anything, so
  /// it's replaced.
  Future<DmDevice?> _openStore() async {
    final storeKey = await _keyOrNew(DmKeyName.store);
    final leafKey = await _keyOrNew(DmKeyName.leaf);
    final identity = await keys.get(DmKeyName.identity);
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

  Future<Uint8List> _keyOrNew(DmKeyName name) async {
    final kept = await keys.get(name);
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
  /// the other device shows ("iPadOS"), [endpointId] where history comes
  /// from (E8).
  Future<String?> offerThisDevice({
    required String platform,
    required List<int> endpointId,
  }) async {
    final device = _device;
    if (device == null || setup != DmSetup.addThisDevice) return null;
    _offer?.dispose();
    _offerMadeMs = _nowMs();
    _offer = await device.startHandoff(
      leafId: leafId,
      name: _oneLine(deviceName, 64),
      platform: _oneLine(platform, 32),
      endpointId: endpointId,
    );
    _notify();
    // The worker's outbox says when a hand-off comes.
    unawaited(_resume());
    return _offer!.qr();
  }

  /// What came for this device's offer: the person's identity and pins,
  /// opened only with the offer's own keys. One that doesn't open is
  /// dropped, and the offer is spent: a new one is shown.
  Future<void> takeHandoff() async {
    final device = _device;
    final offer = _offer;
    if (device == null || offer == null) return;
    final waiting = await routes.handoffs();
    for (final item in waiting) {
      try {
        final done = await device.finishHandoff(
          handoff: offer,
          sealed: item.sealed,
          nowMs: BigInt.from(_nowMs()),
        );
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
        // In every group of the person's by external commit.
        await sync();
        return;
      } catch (error) {
        problem = '$error';
        await routes.deleteHandoff(item.id);
        _offer = null;
        offer.dispose();
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
    _notify();
  }

  /// Removes the person's device [leaf]: a new list without it, and out of
  /// every group it's in.
  Future<void> removeDevice(String leaf) async {
    final device = _device;
    if (device == null || leaf == leafId) return;
    await _changeList(
      device,
      (leaves) => [
        for (final each in leaves)
          if (each.leafId != leaf) each,
      ],
    );
    for (final group in groups.values) {
      final members = await device.members(groupId: group.id);
      if (!members.any((member) => member.leafId == leaf)) continue;
      final commit = await device.removeMembers(
        groupId: group.id,
        leafIds: [leaf],
      );
      try {
        await routes.commit(group.groupId, commit);
        await _catchUpOne(device, group.groupId);
      } on UserAppError catch (error) {
        await device.commitRefused(groupId: group.id);
        problem = error.toString();
      }
    }
    _notify();
  }

  /// The person's devices, from their list, with the names their
  /// certificates give in the self group.
  Future<List<({String leafId, String name, bool here})>> devices() async {
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
          (
            leafId: leaf.leafId,
            name: leaf.leafId == leafId
                ? deviceName
                : members
                          .where((member) => member.leafId == leaf.leafId)
                          .firstOrNull
                          ?.name ??
                      '',
            here: leaf.leafId == leafId,
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
    try {
      for (final welcome in await routes.welcomes()) {
        await _join(device, welcome);
      }
      groups
        ..clear()
        ..addEntries([
          for (final group in await routes.groups())
            MapEntry(group.groupId, group),
        ]);
      final here = {
        for (final group in await device.groups()) dmB64(group.groupId): group,
      };
      for (final group in groups.values) {
        final mine = here[group.groupId];
        if (mine == null) {
          await _rejoin(device, group);
        } else if (group.lastSeq > mine.seq) {
          await _catchUp(device, group, mine);
        }
      }
      // Claktories whose access ended come out before anything is sent
      // (Fugo L6).
      await _reconcileGrantsSafely(device, adding: false);
      await _sendUnsent(device);
      await _topUpKeyPackages();
      await _linkClaktory(device);
      await _reconcileGrantsSafely(device, adding: true);
    } on UserAppError catch (error) {
      problem = error.toString();
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
      if (!offer.enrolled) {
        final certificate = await device.issueCertificate(
          kind: DmLeafKind.claktory,
          leafId: offer.leafId,
          signatureKey: offer.leafSignatureKey,
          name: _oneLine(offer.name, 64),
          createdMs: BigInt.from(_nowMs()),
        );
        await link.enroll(certificate, await device.exportPins());
      }
      final channel = await _channel(device, offer.claktoryId);
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
  ) async =>
      (await device.members(groupId: group.id))
          .any((member) => member.leafId == leaf);

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
    for (final group in groups.values) {
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
        final out = [
          for (final member in members)
            if (member.kind == DmLeafKind.claktory &&
                !want.contains(member.leafId))
              member.leafId,
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
        final processed = await device.process(
          groupId: group.id,
          seq: item.seq,
          envelope: item.envelope,
          arrivedMs: item.acceptedMs,
        );
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
          case DmOutcome.message || DmOutcome.proposal || DmOutcome.already:
            break;
        }
        if (processed.message case final message?) {
          _confirmed(message);
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
      if (account == accountId) return _fail(DmFailure.identityReplaced);
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
    await _sharePins();
    _notify();
  }

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
      if (dmB64(unsent.groupId) == group) unsent,
  ];

  /// What the person's pin of [account] says now.
  DmPin? pin(String account) => _device?.pin(account: account);

  void _set(DmSetup to) {
    setup = to;
    _notify();
    unawaited(_resume());
  }

  void _fail(DmFailure why) {
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
