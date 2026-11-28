import 'dart:async';
import 'dart:convert';
import 'dart:io' show File;
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart' show Backend;
import '../device_account.dart' show AccountClaktory, DeviceAccount;
import '../protocol/factory/v1/factory.pb.dart'
    show
        DmAck,
        DmCursor,
        DmDelivery,
        DmDeliveryState,
        DmFeed,
        DmFollow,
        DmHome,
        DmLeafOffer,
        DmReceipt,
        DmRoute,
        DmSend,
        DmSendResult;
import '../protocol/factory/v1/factory.pb.dart' as pb show DmRoutes;
import '../rail.dart' show ClaktoriesScope;
import '../scope.dart';
import '../tunnel/api/dm.dart';
import '../tunnel/api/dm_transfer.dart';
import '../tunnel_client.dart' show RustTunnels;
import '../user_apps/session.dart';
import 'dm_home.dart';
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

/// The Claktory each engine talks to, as the app showed it last.
final _engineBackends = Expando<Backend>();

/// This device's engine, for the account it's signed in to: started and
/// following the worker from its first use, for as long as the app runs
/// (dms-e2ee §5.3: every group is taken in while the app runs, not only
/// while a chat is open). Signing in or out starts it again.
E2ee e2eeOf(BuildContext context) {
  if (e2eeFor case final hook?) return hook(context);
  final scope = AppScope.scope(context);
  final account = scope.connections.account;
  final known = _engines[account];
  final shown = dmShownOf(context, scope.backend);
  if (known != null) {
    _show(known, scope.backend, shown);
    return known;
  }
  final link = _BackendLink(scope.backend);
  final e2ee = _engines[account] = E2ee(session: UserAppsSession(account))
    // Which of the person's Claktories are theirs, as claktory.com lists
    // them (Fugo 39914).
    ..accountClaktories = account.claktories
    ..claktory = link
    // The person's home: the Claktory the app shows (#451, v1).
    ..home = link
    ..show(shown);
  _engineBackends[e2ee] = scope.backend;
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

/// Points [e2ee] at the Claktory the app shows ([backend], which [shown]
/// says): another of the person's Claktories shown now (the rail) is the
/// one it talks to from here on, never the one it started with (row 569),
/// once their account lists it ([E2ee.show]).
void _show(E2ee e2ee, Backend backend, DmShown shown) {
  if (!identical(_engineBackends[e2ee], backend)) {
    _engineBackends[e2ee] = backend;
    final link = _BackendLink(backend);
    e2ee
      ..claktory = link
      ..home = link;
  }
  e2ee.show(shown);
}

/// Which Claktory [backend] is, from the rail's entry whose connection it
/// is (Fugo 40182): never from another entry than the one linked. A
/// window with no rail, or a backend no entry has, can't say: never
/// confirmed.
@visibleForTesting
DmShown dmShownOf(BuildContext context, Backend backend) {
  final claktories = context
      .getInheritedWidgetOfExactType<ClaktoriesScope>()
      ?.notifier;
  if (claktories == null) return const DmShown.unknown();
  var entry = claktories.selected;
  if (!identical(entry.backend, backend)) {
    final others = claktories.all.where(
      (each) => identical(each.backend, backend),
    );
    if (others.isEmpty) return const DmShown.unknown();
    entry = others.first;
  }
  final target = entry.target;
  if (target == null) {
    // This Mac's own; on a phone with nothing paired, nothing.
    return claktories.localBackend
        ? const DmShown.thisMac()
        : const DmShown.unknown();
  }
  return DmShown.remote(code: target.code, endpoint: target.endpointId);
}

/// This device's engine for [account] if something started it already;
/// null otherwise.
E2ee? startedE2ee(DeviceAccount account) => _engines[account];

/// Whether this device's messages start over as a new device: its account
/// key changed from one it had (removed by another device). A first key,
/// made at a fresh device's first sign-in, never does (Fugo).
bool startsOver(String before, String now) =>
    before.isNotEmpty && now != before;

/// The Claktory the app shows, through its backend connection: its leaf
/// to enroll, and the person's home when it's that (#451).
class _BackendLink implements DmClaktoryLink, DmHomeLink {
  _BackendLink(this.backend);

  final Backend backend;

  @override
  Future<DmLeafOffer> offer() => backend.dmLeafOffer();

  @override
  Future<void> enroll(List<int> certificate, List<int> pins) =>
      backend.dmEnrollLeaf(certificate, pins);

  @override
  ValueListenable<bool> get connected => backend.connected;

  @override
  Stream<DmFeed> get feeds => backend.dmFeeds;

  @override
  Stream<DmHome> get homes => backend.dmHomes;

  @override
  Future<DmReceipt> send(DmSend send) => backend.dmSend(send);

  @override
  Future<void> follow(DmFollow follow) => backend.dmFollow(follow);

  @override
  Future<void> ack(DmAck ack) => backend.dmAck(ack);

  @override
  Future<DmHome> routes(pb.DmRoutes routes) => backend.dmRoutes(routes);
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
    this.replacedIdentityKeys = const [],
    this.signedOut = const [],
  });

  factory DmAccountKeys.fromJson(Map<String, Object?> json) => DmAccountKeys(
    accountId: _text(json, 'account_id'),
    identityKey: dmUnb64(_text(json, 'identity_key')),
    deviceList: dmUnb64(_text(json, 'device_list')),
    version: _int(json, 'version'),
    replacedIdentityKeys: [
      for (final key in json['replaced_identity_keys'] as List? ?? const [])
        if (key is String && key.isNotEmpty) dmUnb64(key),
    ],
    signedOut: [
      for (final leaf in json['signed_out'] as List? ?? const [])
        if (leaf is String && leaf.isNotEmpty) leaf,
    ],
  );

  final String accountId;
  final Uint8List identityKey;
  final Uint8List deviceList;
  final int version;

  /// The keys the account replaced, oldest first, as the worker lists them
  /// (row 654): only ever retired here as a held commit proves, or by this
  /// device's own start-over (Fugo).
  final List<Uint8List> replacedIdentityKeys;

  /// The account's listed devices that signed out of it (row 671), by
  /// leaf id: only in the answer to one of its own devices, and empty from
  /// a service that doesn't say.
  final List<String> signedOut;
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

  /// The person's friends' accounts, as their friends list has it.
  Future<List<String>> friends() async => [
    for (final item in _list(await session.get('$_dms/contacts'), 'contacts'))
      _text(item, 'account_id'),
  ];

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

  /// This device's leaf's supply at the worker: the one-time key packages
  /// it has left, whether it keeps a last resort, and when that one's
  /// lifetime ends (seconds since the epoch; 0 when it doesn't say).
  Future<({int remaining, bool lastResort, int lastResortEnds})>
  keyPackageSupply() async {
    final answer = await session.get('$_dms/key-packages');
    return (
      remaining: _int(answer, 'remaining'),
      lastResort: answer['last_resort'] == true,
      lastResortEnds: (answer['last_resort_ends'] as num?)?.toInt() ?? 0,
    );
  }

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

/// What signing out of the Claktory account does to this device's
/// messages ([E2ee.signOutPlan], row 671).
enum DmSignOut {
  /// There are none here: it only signs out.
  none,

  /// This is the person's only device with them: they stay here, locked,
  /// until it signs in to the same account again, unless deleted.
  alone,

  /// The person's other devices have them: this one leaves, and they're
  /// deleted here.
  withOthers,
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
/// An add code [E2ee.scanned] refuses: handed off to already, or for a
/// device the person removed or locked out (Abbacchio's row 612, Fugo).
class DmCodeRefused implements Exception {
  const DmCodeRefused();

  /// What the Rust says, and the person reads.
  static const message =
      'This code was already used, or is for a device you removed.';

  @override
  String toString() => message;
}

/// Why [E2ee.startNewKey] didn't make a new security key.
enum DmStartOverStop {
  /// The account service wants a recent sign-in first (403
  /// sign_in_again, Fugo): sign in again, then start over.
  signInAgain,

  /// Too many new keys today, or the worker refused; [E2ee.problem] has
  /// the detail.
  failed,
}

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

  /// The new key's message waits for one of the person's own Claktories:
  /// this device is connected to another account's (row 1453). It goes
  /// once it connects to one of theirs.
  waitsForOwnClaktory,

  /// The worker refused the new key for today: an account's key changes
  /// 3 times a day at most. The Lock out stays under way and carries on
  /// by itself at [E2ee.keyChangeAtMs] (Fugo 43752).
  keyWaitsForLimit,

  /// The worker or this device refused; [E2ee.problem] has the detail.
  /// Tried again from where it got to.
  failed,

  /// The account service wants a recent sign-in for a new key (403
  /// sign_in_again, row 630): sign in again, then carry on.
  signInAgain,
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

/// Whether a store is at [path]; tests give their own.
Future<bool> _storeExistsHere(String path) => File(path).exists();

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

/// A friend's home turns the person's away ([DmPeer.refused], row 567).
const dmTurnedAway = 'turned_away';

/// The most items a window takes (row 561): claktory-dm-crypto's
/// WINDOW_MOST, the opening commit included.
const dmWindowMost = 64;
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
    Future<bool> Function(String path)? storeExists,
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
       _storeExists = storeExists ?? _storeExistsHere,
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

  /// The person's home Claktory (#451): every message goes to it, and
  /// this device follows its groups there once it went over ([_drained]).
  /// Null only in tests of the worker as it was.
  DmHomeLink? get home => _home;
  DmHomeLink? _home;

  /// Whether the Claktory the app shows is on the person's account
  /// ([ownership]). Until it's confirmed it's given nothing (Fugo 39894):
  /// no leaf, pins, list change, routes, messages or home; this device
  /// stays on the worker, and what it sends waits.
  bool get homeIsOurs => _homeIsOurs;
  var _homeIsOurs = false;
  void _setOurs(bool ours) {
    if (ours == _homeIsOurs) return;
    _homeIsOurs = ours;
    homeState = null;
    _routesSent = '';
    _followed = false;
    _linkedMs = -1;
    if (ours && (_home?.connected.value ?? false)) {
      scheduleMicrotask(_connectionMoved);
    } else {
      scheduleMicrotask(_notify);
    }
  }

  /// The person's account's Claktories, as claktory.com lists them (their
  /// endpoints and ids): how the Claktory shown is known to be theirs.
  /// With none, no Claktory is (Fugo 40182).
  Future<List<AccountClaktory>> Function()? accountClaktories;

  /// What the account says of the Claktory the app shows.
  DmOwnership ownership = DmOwnership.checking;

  /// Which Claktory the app shows ([show]); null until it says.
  DmShown? _shown;

  /// Tests of the engine alone: the Claktory shown counts as the person's
  /// own without the account's word ([ungatedForTests]).
  var _ungated = false;

  /// Tests of the engine alone, with no account to list Claktories: the
  /// one shown counts as the person's own from now on (false: checked
  /// again, as in the app). Never in the app, which always asks the
  /// account (Fugo 40182).
  @visibleForTesting
  set ungatedForTests(bool ungated) {
    _ungated = ungated;
    _shown = null;
    _ownCheckedMs = -1;
    _ownClaktoryId = null;
    ownership = ungated ? DmOwnership.ours : DmOwnership.checking;
    _setOurs(ungated);
  }

  /// Its id on the person's account, once that's confirmed: the only
  /// Claktory whose leaf this device enrolls.
  String? _ownClaktoryId;

  /// When ownership was last confirmed, by [_nowMs]; -1 for never, or
  /// for another Claktory.
  var _ownCheckedMs = -1;

  /// The app shows [shown]: nothing goes to it until the account lists it
  /// (Fugo 39914). Another Claktory is checked afresh, even one dialled
  /// the same way (Fugo 40182).
  void show(DmShown shown) {
    if (shown == _shown) return;
    _shown = shown;
    _ownCheckedMs = -1;
    _ownClaktoryId = null;
    if (_ungated) return;
    ownership = DmOwnership.checking;
    _setOurs(false);
    scheduleMicrotask(_notify);
    scheduleMicrotask(() => unawaited(sync()));
  }

  /// Confirms the Claktory shown is on the person's account, when it was
  /// last more than [_linkEvery] ago (a Claktory taken off it stops then):
  /// a Claktory reached through the tunnel by the endpoint it authenticated
  /// being on claktory.com's list, never by what it says of itself; this
  /// Mac's own by the id its leaf offer gives, on the list. One on another
  /// machine paired with a direct code (no tunnel, no endpoint) never is:
  /// nothing authenticates which it is (Fugo 40182). Fails closed: the
  /// list unread, the device signed out, no list at all, or the Claktory
  /// shown not known, it's not confirmed.
  Future<void> _checkOwn() async {
    if (_ungated) return;
    if (_ownCheckedMs >= 0 &&
        _nowMs() - _ownCheckedMs < _linkEvery.inMilliseconds) {
      return;
    }
    final shown = _shown;
    final read = accountClaktories;
    String? id;
    var now = DmOwnership.notOurs;
    try {
      if (shown == null || !shown.confirmable) {
        now = DmOwnership.unconfirmable;
      } else if (read == null) {
        now = DmOwnership.notOurs;
      } else if (shown.endpoint case final endpoint when endpoint.isNotEmpty) {
        final listed = await read();
        id = listed
            .where(
              (each) =>
                  each.endpointId.isNotEmpty &&
                  each.endpointId.toLowerCase() == endpoint.toLowerCase(),
            )
            .firstOrNull
            ?.id;
      } else {
        // This Mac's own ([DmShown.thisMac]): the only one asked who it
        // is.
        final listed = await read();
        final offered = (await claktory?.offer())?.claktoryId ?? '';
        if (offered.isNotEmpty && listed.any((each) => each.id == offered)) {
          id = offered;
        }
      }
      if (id != null && id.isNotEmpty) now = DmOwnership.ours;
    } catch (error) {
      problem = '$error';
      now = DmOwnership.unchecked;
    }
    // Another Claktory shown meanwhile: its own check stands.
    if (shown != _shown) return;
    _ownClaktoryId = now == DmOwnership.ours ? id : null;
    _ownCheckedMs = now == DmOwnership.ours ? _nowMs() : -1;
    if (ownership != now) {
      ownership = now;
      scheduleMicrotask(_notify);
    }
    _setOurs(now == DmOwnership.ours);
  }

  /// The home, once the Claktory shown is the person's own.
  DmHomeLink? get _ours => _homeIsOurs ? _home : null;
  set home(DmHomeLink? link) {
    _homeSubscriptions();
    _home = link;
    _followed = false;
    // What another Claktory said of itself never stands for this one.
    homeState = null;
    if (link == null) return;
    _feedsHeard = link.feeds.listen(_heard);
    _homesHeard = link.homes.listen(_homeSaid);
    _homeConnected = link.connected;
    link.connected.addListener(_connectionMoved);
    // Connected already (the app switched to it): as if it just came,
    // after whatever is building now.
    if (link.connected.value) scheduleMicrotask(_connectionMoved);
  }

  StreamSubscription<DmFeed>? _feedsHeard;
  StreamSubscription<DmHome>? _homesHeard;
  ValueListenable<bool>? _homeConnected;

  void _homeSubscriptions() {
    unawaited(_feedsHeard?.cancel());
    unawaited(_homesHeard?.cancel());
    _homeConnected?.removeListener(_connectionMoved);
    _feedsHeard = _homesHeard = null;
    _homeConnected = null;
  }

  /// What this device has from its home while it follows.
  final homeFeed = DmHomeFeed();

  /// The home's state as it last said (DmHome); null before it said.
  DmHome? homeState;

  /// Whether this device went over to its home: it read what the worker
  /// still held, once, and since reads only the home (Mista 32426, §6).
  var _drained = false;

  /// How long a commit from here waits to come back from the home before
  /// the rest goes on without it.
  static const _homeWait = Duration(seconds: 5);
  final DmOpen _open;
  final Future<DmTransferEndpoint> Function() _bindTransfer;
  final Future<bool> Function(String idHex) _admitTransfer;
  final Future<Uint8List> Function() _newSecret;
  final Future<void> Function(String path) _removeStore;
  final Future<bool> Function(String path) _storeExists;
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

  /// The person's listed devices that signed out of the account (row 671),
  /// by leaf id, as the worker last said: Signed out in Your devices.
  Set<String> signedOut = const {};

  /// Reads again which of the person's devices signed out.
  Future<void> readSignedOut() async {
    if (setup != DmSetup.ready || accountId.isEmpty) return;
    try {
      final mine = await routes.keys(accountId);
      final now = {...?mine?.signedOut};
      if (setEquals(now, signedOut)) return;
      signedOut = now;
      _notify();
    } catch (_) {
      // Read again when Your devices opens next.
    }
  }

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
  /// to join them again (by [_nowMs]).
  final _rejoinAfter = <String, int>{};
  final _rejoinTries = <String, int>{};

  /// Groups whose commit from here (a move to the person's new key, a
  /// removed device taken out) the worker refused as behind, how many
  /// times in a row, and when it may next be tried (by [_nowMs]): never at
  /// once again. A kept device that missed a rotation retried at every
  /// pass of a sync, about 50 requests a second (Abbacchio's R5 rows
  /// 570–575).
  final _refusedTries = <String, int>{};
  final _refusedAfter = <String, int>{};

  /// Whether a commit to [group] waits after being refused as behind.
  bool _waitsAfterRefusal(String group) =>
      _nowMs() < (_refusedAfter[group] ?? 0);

  /// [group] refused a commit from here as behind: it waits longer after
  /// each time in a row. How many times in a row.
  int _refusedAsBehind(String group) {
    final tries = (_refusedTries[group] ?? 0) + 1;
    _refusedTries[group] = tries;
    _refusedAfter[group] =
        _nowMs() + (retryAfter * min(1 << (tries - 1), 64)).inMilliseconds;
    return tries;
  }

  /// When the last sync started (by [_nowMs]).
  var _syncedMs = -1;

  /// Whether a wait after a refusal, a refused join or a failed sign-out
  /// has ended since the last sync started: [follow] syncs then, even with
  /// nothing new in the account, where nothing else would (Abbacchio's row
  /// 590: a kept device stayed out of the channel).
  @visibleForTesting
  bool get waitOver {
    final now = _nowMs();
    return [
      ..._refusedAfter.values,
      ..._rejoinAfter.values,
      ..._signOutAfter.values,
      ?_syncFailedAfter,
      ?keyChangeAtMs,
    ].any((ms) => ms > _syncedMs && ms <= now);
  }

  /// After a sync stopped on an error (the worker briefly unavailable),
  /// when it's tried again, longer after each in a row: the events it
  /// missed are behind the cursor and won't come again (Abbacchio's row
  /// 601: a kept device never read its self group again).
  int? _syncFailedAfter;
  var _syncFailures = 0;

  void _syncFailed() {
    // A sync something else started during the wait (a group event)
    // that fails too leaves the wait as it is: only a try once it's over
    // makes the next longer, so a short outage doesn't push it far past
    // its end (Abbacchio: six quick failures in a 30 s outage made it
    // 160 s).
    final now = _nowMs();
    final waiting = _syncFailedAfter;
    if (waiting != null && now < waiting) return;
    _syncFailures++;
    _syncFailedAfter =
        now + (retryAfter * min(1 << (_syncFailures - 1), 64)).inMilliseconds;
  }

  /// A commit from here to [group] went through.
  void _commitTaken(String group) {
    _refusedTries.remove(group);
    _refusedAfter.remove(group);
  }

  /// The person's removed devices signed out of their account from here
  /// while the app runs: not signed out again at each sync while a group
  /// still holds them (the service's removal limit answered 429).
  final _signedOut = <String>{};

  /// The new key's bundles handed off from here while the app runs, as
  /// `<retry id>/<leaf>`: each once (row 1453).
  final _handedOff = <String>{};

  /// The rotation's message came back from the log while a removal ran.
  var _saidMeanwhile = false;

  /// When a new key the worker refused for today ([DmKeyChange
  /// .keyWaitsForLimit]) is tried again by itself, by [_nowMs]: the start
  /// of the worker's next day (its limit counts whole days, UTC), a minute
  /// past. Null when nothing waits for it.
  int? keyChangeAtMs;

  /// The device locked out was given the person's current key before it
  /// was (a key change under way when it was locked out, finished first):
  /// what the Lock out waiting for [keyChangeAtMs] says (Fugo 43752).
  var keyHeldByLockedOut = false;

  /// The worker's day after [nowMs] starts, a minute past.
  static int _nextWorkerDay(int nowMs) =>
      (nowMs ~/ Duration.millisecondsPerDay + 1) * Duration.millisecondsPerDay +
      Duration.millisecondsPerMinute;

  /// Removed devices whose sign-out didn't go through, and when it may
  /// next be tried (by [_nowMs]).
  final _signOutAfter = <String, int>{};
  final _signOutTries = <String, int>{};

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
          // What changed while nothing followed: keys events come only
          // from here on.
          if (setup == DmSetup.ready) {
            if (_device case final device?) await _readFriends(device);
            await _refreshAllKeys();
          }
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
              // A friend's home may have moved: the routes follow it at once
              // (Mista, row 567).
              if (_device case final device? when _drained) {
                await _sendRoutes(device);
              }
            case 'handoff':
              await takeHandoff();
            case 'contacts':
              _events.add(event);
              // Befriended or unfriended: pinned now, and routed or not.
              if (_device case final device? when setup == DmSetup.ready) {
                await _meetFriends(device);
                if (_drained) await _sendRoutes(device);
              }
            default:
              _events.add(event);
          }
        }
        if (groupsMoved || waitOver) await sync();
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
      _drained = device.homeDrained();
      final status = device.status();
      if (status.setUp && status.accountId != accountId) {
        return _fail(DmFailure.otherAccount);
      }
      final mine = await routes.keys(accountId);
      signedOut = {...?mine?.signedOut};
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
      // A new key under way carries on, unless a Lock out is under way
      // with it: that carries on below, and settles the key itself (one
      // handed to a device locked out since is made again, never finished:
      // Abbacchio's row 1462).
      final rotation = await device.rotation();
      if (rotation != null && ((await device.removal())?.newKey ?? false)) {
        await finishRemoval();
        await sync();
      } else if (rotation != null) {
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
  ///
  /// A new store (none here, or one replaced) is a new device here, with
  /// new device keys (row 202, Fugo): the key store outlives the app (an
  /// iPhone's keychain keeps it across a reinstall), but a leaf key used
  /// by an install whose store is gone would come back as the same leaf,
  /// with its old key packages still handed out. So its leaf and store
  /// keys are made anew, and the person's identity key is deleted: a
  /// device with no store isn't in anything, and the key comes back the
  /// usual ways (a hand-off, or a start-over).
  ///
  /// So where store.db lives must never change without moving it there
  /// first, before this runs (Fugo): a store not found at its path is
  /// taken for a new install, and an ordinary update would wipe the
  /// person's identity on every device.
  Future<DmDevice?> _openStore() async {
    final path = '${await keys.folder()}/store.db';
    if (!await _storeExists(path)) await _forgetDeviceKeys();
    var device = await _openWith(path);
    if (device == null) {
      await _removeStore(path);
      await _forgetDeviceKeys();
      device = await _openWith(path);
    }
    return device;
  }

  Future<DmDevice?> _openWith(String path) async {
    final storeKey = await _keyOrNew(DmKeyName.store);
    final leafKey = await _keyOrNew(DmKeyName.leaf);
    final identity = await _keptKey(DmKeyName.identity);
    try {
      return await _open(
        path: path,
        storeKey: storeKey,
        leafKey: leafKey,
        identityKey: identity,
      );
    } finally {
      identity?.fillRange(0, identity.length, 0);
    }
  }

  /// Every key kept for a store that's gone: what [_openWith] needs is
  /// made anew.
  Future<void> _forgetDeviceKeys() async {
    for (final name in DmKeyName.values) {
      await keys.delete(name);
    }
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

  /// No other device: makes a new security key for the person's account
  /// on this one (dms-e2ee §2, "All devices lost"; Abbacchio's row 630).
  /// The account's keys go to the new key, with this device alone on the
  /// list; it then rejoins each of the person's groups, taking out the
  /// devices under the old key (Ghiaccio, Fugo). Its old messages aren't
  /// here; friends see the key changed. Null once done.
  Future<DmStartOverStop?> startNewKey() async {
    final device = _device;
    if (device == null || setup != DmSetup.addThisDevice) {
      return DmStartOverStop.failed;
    }
    _dropOffer();
    final identity = await _newSecret();
    await keys.put(DmKeyName.identity, identity);
    try {
      // The key it replaces, held as retired here from the start
      // (Ghiaccio): its leaves are the dead ones, and a Claktory enrolled
      // under it can enroll again.
      final old = await routes.keys(accountId);
      if (old == null) throw StateError('the account has no key to replace');
      final made = await device.setUpNewKey(
        identityKey: identity,
        oldIdentityPublic: old.identityKey,
        // Those before it too, after more than one start-over (row 654).
        replacedIdentityKeys: old.replacedIdentityKeys,
        accountId: accountId,
        leafId: leafId,
        name: deviceName,
        nowMs: BigInt.from(_nowMs()),
      );
      await routes.rotateKeys(
        identityKey: made.identityPublic,
        deviceList: made.deviceList,
      );
    } catch (error) {
      // Not made: back to waiting to be added, with nothing kept.
      problem = '$error';
      await keys.delete(DmKeyName.identity);
      await _forgetStore();
      await start();
      return error is UserAppError &&
              error.problem == UserAppProblem.signInAgain
          ? DmStartOverStop.signInAgain
          : DmStartOverStop.failed;
    } finally {
      identity.fillRange(0, identity.length, 0);
    }
    // What stopped an earlier try (a sign-in too old) no longer stands.
    problem = '';
    try {
      await _topUpKeyPackages(force: true);
      _set(DmSetup.ready);
      await sync();
      await _makeSelfGroup(device);
    } catch (error) {
      // The key is the account's now; the rest carries on at each sync.
      problem = '$error';
    }
    return null;
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

  /// A hand-off came for an earlier code that this one can't open (row
  /// 564): its app closed meanwhile, say, and its keys went with it. Said
  /// beside the new code until this device is added.
  var earlierCodeSpent = false;
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
    if (waiting.isEmpty) return;
    // Only the person's identity as their account publishes it (Melone
    // F3): someone else who learned the code's secret (a photo of it, the
    // clipboard) can't plant theirs here.
    final published = await routes.keys(accountId);
    if (published == null) return;
    for (final item in waiting) {
      var opened = false;
      try {
        final done = await device.finishHandoffFor(
          handoff: offer,
          sealed: item.sealed,
          nowMs: BigInt.from(_nowMs()),
          accountId: accountId,
          identityPublic: published.identityKey,
        );
        opened = true;
        await keys.put(DmKeyName.identity, done.identityKey);
        done.identityKey.fillRange(0, done.identityKey.length, 0);
        await routes.deleteHandoff(item.id);
        _offer = null;
        offer.dispose();
        earlierCodeSpent = false;
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
          earlierCodeSpent = true;
        }
        _notify();
        return;
      }
    }
  }

  /// On a device that's set up: what [qr] (scanned from a new device) says
  /// of it, with the person's identity sealed for it, to ask the person
  /// before [addDevice].
  /// Throws [DmCodeRefused] for a code that was handed off to already,
  /// or is for a device the person removed (Abbacchio's row 612).
  Future<DmHandoffSealed> scanned(String qr) async {
    try {
      return await _device!.sealHandoff(qr: qr);
    } catch (error) {
      if ('$error'.contains(DmCodeRefused.message)) {
        throw const DmCodeRefused();
      }
      rethrow;
    }
  }

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
    // A code works once: asked again, it's refused (Abbacchio's row 612:
    // after its device was locked out, it put it back on the list).
    await device.codeUsed(comparison: sealed.comparison);
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
        await _commitRemove(device, group, out);
        return _commitTaken(group.groupId);
      } on UserAppError catch (error) {
        if (error.problem != UserAppProblem.epoch) rethrow;
        if (attempt > 0) {
          // Behind even after catching up: tried again after a wait.
          _refusedAsBehind(group.groupId);
          rethrow;
        }
        await _catchUpOne(device, group.groupId);
      }
    }
  }

  /// The person's devices taken off their list but still in a group here
  /// (a removal that stopped part-way), by leaf, with the name each leaf
  /// claims: unchecked, so only to show and remove. None while the
  /// person's key is being settled.
  Future<Map<String, String>> _removedButIn(DmDevice device) async => {
    for (final MapEntry(:key, :value) in (await _offListButIn(device)).entries)
      if (value.kind == DmLeafKind.device) key: value.name,
  };

  /// The person's leaves off their list but still in a group here, devices
  /// and Claktories alike (Fugo: a removed Claktory's leaf kept its
  /// channel's keys until some later commit), by leaf.
  Future<Map<String, ({String name, DmLeafKind kind})>> _offListButIn(
    DmDevice device,
  ) async {
    final list = device.deviceList(account: accountId);
    if (list == null || _ownKeyCheck) return const {};
    final listed = {for (final leaf in list.leaves) leaf.leafId};
    return {
      for (final group in await _groupsHere(device))
        for (final leaf in await device.leaves(groupId: group.id))
          if (leaf.account == accountId &&
              leaf.leafId != leafId &&
              !listed.contains(leaf.leafId))
            leaf.leafId: (name: leaf.name, kind: leaf.kind),
    };
  }

  /// The person's devices off their list that are still in a group here
  /// (Fugo, G5's device twin), committed out of each: a removal that
  /// stopped part-way is finished at the next sync, whatever stopped it.
  /// The list is read again from the worker first, so a device another of
  /// the person's devices just added is never taken for one removed.
  Future<void> _sweepRemovedDevices(DmDevice device) async {
    if ((await _offListButIn(device)).isEmpty) return;
    await refreshKeys(accountId);
    final offList = await _offListButIn(device);
    // A Claktory taken off the list comes out of every group here, its
    // channel too (Fugo); it isn't a device of the account, so there's no
    // sign-out or removal to keep for it.
    final claktories = [
      for (final MapEntry(:key, :value) in offList.entries)
        if (value.kind == DmLeafKind.claktory) key,
    ];
    if (claktories.isNotEmpty) {
      for (final group in await _groupsHere(device)) {
        if (held.containsKey(group.groupId) ||
            _waitsAfterRefusal(group.groupId)) {
          continue;
        }
        try {
          await _removeFrom(device, group, claktories);
        } catch (error) {
          problem = '$error';
        }
      }
    }
    final strays = {
      for (final MapEntry(:key, :value) in offList.entries)
        if (value.kind == DmLeafKind.device) key: value.name,
    };
    if (strays.isEmpty) return _notify();
    // Never signed out: a device under a key since replaced (Fugo, row
    // 630), read before it comes out of the groups. A surviving old
    // device's notice is the person's alarm when someone else started
    // over; Your devices offers Remove… for it.
    final replaced = <String>{};
    for (final group in await _groupsHere(device)) {
      replaced.addAll(await device.replacedLeaves(groupId: group.id));
    }
    // Kept as under way first: a sign-out that doesn't go through shows in
    // Your devices with Try again, and is carried on at the next launch.
    await _beginRemoval(
      device,
      remove: [...strays.keys],
      names: [...strays.values],
      newKey: false,
    );
    for (final group in await _groupsHere(device)) {
      if (held.containsKey(group.groupId) ||
          _waitsAfterRefusal(group.groupId)) {
        continue;
      }
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
      if (replaced.contains(leaf)) {
        await _removed(device, leaf);
        continue;
      }
      // Once signed out it stays so; one that didn't go through waits
      // longer after each try (Abbacchio's R5: 429 at every sync).
      // One signed out already (an earlier sweep, a Lock out) ends its
      // row here too (Abbacchio's row 613: a second sweep began its
      // removal again and left it as Not fully removed).
      if (_signedOut.contains(leaf)) {
        await _removed(device, leaf);
        continue;
      }
      if (_nowMs() < (_signOutAfter[leaf] ?? 0)) continue;
      if (await signOutDevice(leaf) == null) {
        _signedOut.add(leaf);
        _signOutAfter.remove(leaf);
        await _removed(device, leaf);
      } else {
        final tries = (_signOutTries[leaf] ?? 0) + 1;
        _signOutTries[leaf] = tries;
        _signOutAfter[leaf] =
            _nowMs() + (retryAfter * min(1 << (tries - 1), 64)).inMilliseconds;
      }
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

  /// The person's devices a start-over took out of their groups (they had
  /// the key it replaced), by leaf with their names, until removed.
  Future<Map<String, String>> replacedDevices() async {
    final device = _device;
    if (device == null || setup != DmSetup.ready) return const {};
    // Never one listed now, which is that device under the new key:
    // Remove would take the device itself out (Abbacchio's row 1463). Nor
    // one signed out from here (a Lock out) that's in no group here any
    // more: nothing is left to remove. One still signed in (a start-over
    // takes them out of the chats, never signs them out), or signed out
    // and still in a group, stays, with its Remove (Fugo 43752).
    final listed = {
      for (final leaf
          in device.deviceList(account: accountId)?.leaves ??
              const <DmListed>[])
        leaf.leafId,
    };
    final replaced = [
      for (final each in await device.replacedDevices())
        if (!listed.contains(each.leafId)) each,
    ];
    if (!replaced.any((each) => _signedOut.contains(each.leafId))) {
      return {for (final each in replaced) each.leafId: each.name};
    }
    final inGroups = <String>{
      for (final group in await device.groups())
        ...await device.leafIds(groupId: group.groupId),
    };
    return {
      for (final each in replaced)
        if (!_signedOut.contains(each.leafId) || inGroups.contains(each.leafId))
          each.leafId: each.name,
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
    if (leaf == leafId) {
      // This device itself (its leaf from before a start-over, listed by
      // a build before row 640's fix): never removed or signed out from
      // here; whatever kept it goes.
      await _removed(device, leaf);
      await device.forgetReplaced(leafId: leaf);
      _notify();
      return null;
    }
    _removing++;
    _notify();
    try {
      await _beginRemoval(device, remove: [leaf], names: [name], newKey: false);
      await removeDevice(leaf);
      switch (await signOutDevice(leaf)) {
        case null:
          await _removed(device, leaf);
          await device.forgetReplaced(leafId: leaf);
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
    final stopped = await _finishRemoval(device);
    // What stopped it, for Your devices to say (Fugo 41048): kept until
    // it's carried on again, after a launch too.
    removalStop = removal == null ? null : stopped;
    _notify();
    return stopped;
  }

  /// What stopped the removal under way when it was last carried on: a
  /// [DmKeyChange] or [DmRemoveStop]; null while it goes, or once done.
  Object? removalStop;

  Future<Object?> _finishRemoval(DmDevice device) async {
    try {
      final now = removal = await device.removal();
      _notify();
      if (now == null) return null;
      if (now.newKey) {
        return await changeKey(
          remove: now.remove,
          keep: now.keep,
          names: now.names,
        );
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
    var stopped = await _changeKey(remove: remove, keep: keep, names: names);
    // Its message came back from the log while it stopped (E11-9): carried
    // on, as the rotation is said.
    if (stopped == DmKeyChange.failed && _saidMeanwhile) {
      _saidMeanwhile = false;
      stopped = await _changeKey(remove: remove, keep: keep, names: names);
    }
    _saidMeanwhile = false;
    // What stopped it, for Your devices to say (Fugo 41048).
    removalStop = removal == null ? null : stopped;
    _notify();
    return stopped;
  }

  Future<DmKeyChange?> _changeKey({
    required List<String> remove,
    required List<String> keep,
    required List<String> names,
  }) async {
    final device = _device;
    if (device == null || setup != DmSetup.ready) return DmKeyChange.failed;
    _changingKey = true;
    _removing++;
    removalStop = null;
    keyChangeAtMs = null;
    _notify();
    try {
      var pending = await device.rotation();
      // Kept from now until the new key is taken (R5b): a dialog closed
      // or the app quit part-way carries on at the next launch, even once
      // the device is off the list. With one under way, both together.
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
      // A new key under way that the worker serves already (its answer
      // lost, say) is the person's key: finished first, so the list
      // changes below are signed under the key the worker has, not refused
      // as stale against it (Abbacchio's row 1461). The Lock out stays
      // under way past it.
      var finishedFirst = false;
      var dropped = false;
      if (pending != null) {
        final served = await routes.keys(accountId);
        if (listEquals(served?.identityKey, pending.identityPublic)) {
          dropped = pending.kept.any(lockOut.remove.contains);
          final finished = await _carryOnKeyChange(
            device,
            pending.kept,
            pending.removed,
          );
          if (finished != null) return finished;
          _changingKey = true;
          finishedFirst = true;
          // What a new key the worker refuses for today says (Fugo 43752).
          keyHeldByLockedOut = dropped;
          pending = null;
          await _beginRemoval(
            device,
            remove: lockOut.remove,
            names: lockOut.names,
            keep: lockOut.keep,
            newKey: true,
          );
        }
      }
      // Off the list, out of every group and signed out first, whatever
      // the new key waits for: never held up by a fresh sign-in (Fugo
      // 41048), even with a new key under way already.
      for (final leaf in lockOut.remove) {
        await removeDevice(leaf);
        // Signed out once is signed out for good: not asked again at each
        // Try again, which spends the 10 an hour (Abbacchio's row 1453).
        if (_signedOut.contains(leaf)) continue;
        if (await signOutDevice(leaf) != null) {
          return DmKeyChange.signOutFailed;
        }
        _signedOut.add(leaf);
        // Signed out: no longer one to remove as on the old key.
        await device.forgetReplaced(leafId: leaf);
      }
      if (pending != null) {
        if (!pending.kept.any(lockOut.remove.contains)) {
          // Carried on as it was confirmed.
          return await _carryOnKeyChange(device, pending.kept, pending.removed);
        }
        // One it was handed to is locked out now, and the worker doesn't
        // have it yet: made again without it.
        await keys.delete(DmKeyName.identityNext);
      } else if (finishedFirst && !dropped) {
        // The key finished first was never handed to a device locked out
        // now: it stays, and the Lock out is done.
        await device.endRemoval();
        removal = null;
        return null;
      }
      // One the key finished first was handed to is locked out now: made
      // again at once without it, so the device locked out never holds the
      // key that stays the person's (Fugo 41322).
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
        // Its retry id is the new key's, so a crash sends the same one.
        final id = 'rotation-${dmB64(rotation.identityPublic)}';
        // Each kept device's bundle once a launch, not at every Try again,
        // which would spend the hand-offs it may have waiting (3 in 30
        // minutes: Abbacchio's row 1453).
        for (final leaf in rotation.kept) {
          final handed = '$id/$leaf';
          if (_handedOff.contains(handed)) continue;
          await routes.handOff(leaf, rotation.sealed);
          _handedOff.add(handed);
        }
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
        // Taken (or back from the log already, which marks it said); one
        // sent again under a new retry id goes by that one.
        final sent = sending[current(id)];
        if (sent?.state != DmSendState.sent &&
            !((await device.rotation())?.said ?? false)) {
          // Waiting for one of the person's own Claktories: said so, as
          // Try again can't do it (Abbacchio's row 1453).
          return sent?.problem == 'otherAccount' ||
                  ownership == DmOwnership.notOurs
              ? DmKeyChange.waitsForOwnClaktory
              : DmKeyChange.failed;
        }
        await device.rotationSaid();
      }
      try {
        await routes.rotateKeys(
          identityKey: rotation.identityPublic,
          deviceList: rotation.deviceList,
        );
      } on UserAppError catch (error) {
        // It may have gone through already (a crash, a lost answer).
        final served = await routes.keys(accountId);
        if (!listEquals(served?.identityKey, rotation.identityPublic)) {
          if (error.problem == UserAppProblem.rateLimited) {
            // The worker's 3 a day: kept under way, said, and carried on by
            // itself once its day is over (Fugo 43752).
            problem = error.toString();
            keyChangeAtMs = _nextWorkerDay(_nowMs());
            return DmKeyChange.keyWaitsForLimit;
          }
          rethrow;
        }
      }
      final lockedOut = await device.removal();
      await device.finishRotation(identityKey: next);
      // The Lock out it finishes ended with it, in the store. One begun
      // after this key was made (of a device it was handed to, say) isn't
      // done by it: kept as under way, never dropped by a relaunch that
      // finishes this key (Abbacchio's row 1462).
      removal = null;
      final later = [
        if (lockedOut != null)
          for (final (i, leaf) in lockedOut.remove.indexed)
            if (!rotation.removed.contains(leaf))
              (leaf, i < lockedOut.names.length ? lockedOut.names[i] : ''),
      ];
      if (later.isNotEmpty) {
        await _beginRemoval(
          device,
          remove: [for (final (leaf, _) in later) leaf],
          names: [for (final (_, name) in later) name],
          keep: lockedOut!.keep,
          newKey: true,
        );
      }
      await keys.put(DmKeyName.identity, next);
      await keys.delete(DmKeyName.identityNext);
      next.fillRange(0, next.length, 0);
      if (later.isEmpty) keyHeldByLockedOut = false;
      _ownKeyCheck = false;
      await _topUpKeyPackages(force: true);
      // Its Claktory enrolls again under the new key.
      _linkedMs = -1;
      await sync();
      return null;
    } on UserAppError catch (error) {
      problem = error.toString();
      return error.problem == UserAppProblem.signInAgain
          ? DmKeyChange.signInAgain
          : DmKeyChange.failed;
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
    // The key this device is making itself (its answer lost, say): its
    // own change, finished where it's carried on, never someone else's.
    final own = await device.rotation();
    if (own != null && listEquals(served.identityKey, own.identityPublic)) {
      return;
    }
    if (pending != null) return;
    // Over at the home, only once the person's own home has said where
    // their self group is and it's read here: a kept device connected to
    // another account's Claktory, or before the home's feed came, hasn't
    // seen the rotation message yet, and waits for it (Abbacchio's row
    // 1453, Fugo 43016's point 3).
    if (_drained && !await _selfReadAtHome(device)) return;
    return _fail(DmFailure.identityReplaced);
  }

  /// Whether this device has read its self group as far as the person's
  /// own home has it.
  Future<bool> _selfReadAtHome(DmDevice device) async {
    final self = groups.values.where((each) => each.kind == 'self').firstOrNull;
    if (self == null || !_homeIsOurs || !homeIsHere) return false;
    if (homeFeed.groups[self.groupId] == null) return false;
    final here = (await device.groups())
        .where((each) => dmB64(each.groupId) == self.groupId)
        .firstOrNull;
    return here != null && here.seq >= homeFeed.lastSeq(self.groupId);
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
      if (!groups.containsKey(group) ||
          held.containsKey(group) ||
          _waitsAfterRefusal(group)) {
        continue;
      }
      final id = dmUnb64(group);
      try {
        final commit = await device.moveGroup(groupId: id);
        await _commit(group, commit);
        _commitTaken(group);
        await _catchUpOne(device, group);
        _unmoved.remove(group);
      } on UserAppError catch (error) {
        await device.commitRefused(groupId: id);
        problem = error.toString();
        if (error.problem == UserAppProblem.epoch) {
          final tries = _refusedAsBehind(group);
          await _catchUpOne(device, group);
          if (tries == 1) {
            // Caught up: moved at the next pass, once the wait is over.
            _syncAgain = true;
          } else if (groups[group] case final known?) {
            // Caught up before and still behind: its feed doesn't bring
            // this device to where the group is (a commit it missed while
            // the rotation went on). It joins again from the group as the
            // worker has it, under the new key, which moves it too.
            await _rejoin(device, known);
          }
        }
      }
    }
  }

  /// How long a new device's code works: the worker keeps its hand-off as
  /// long.
  static const codeLife = Duration(minutes: 30);

  /// When the person's device list as this device took it was signed, by
  /// the signing device's clock; 0 when there's none.
  int get deviceListMs =>
      _device?.deviceList(account: accountId)?.createdMs.toInt() ?? 0;

  /// [device], on the person's list but never in their self group, was
  /// listed more than [codeLife] ago (row 564): its code can't bring it
  /// any more, so it never set up. A later list counts from when it was
  /// signed, so this is never early.
  bool neverSetUp(DmOwnDevice device) =>
      !device.here &&
      device.sinceMs == 0 &&
      groups.values.any((each) => each.kind == 'self') &&
      deviceListMs > 0 &&
      _nowMs() - deviceListMs >= codeLife.inMilliseconds;

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
          if (members
                  .where(
                    (m) => m.account == accountId && m.leafId == leaf.leafId,
                  )
                  .firstOrNull
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
    List<DmListed> Function(List<DmListed> leaves) change, {
    List<int>? home,
  }) async {
    for (var attempt = 0; ; attempt++) {
      final now = device.deviceList(account: accountId);
      final version = now?.version.toInt() ?? 0;
      // The person's home stays named, unless this changes it (#451).
      final signed = await device.signDeviceListWithHome(
        version: BigInt.from(version + 1),
        createdMs: BigInt.from(_nowMs()),
        leaves: change(now?.leaves ?? const []),
        home: switch (home ?? now?.home) {
          final List<int> bytes => Uint8List.fromList(bytes),
          null => null,
        },
      );
      try {
        await routes.putKeys(
          deviceList: signed,
          version: version + 1,
          previousVersion: version,
        );
      } on UserAppError catch (error) {
        // Refused (or no answer): the pins stay as they were, so the
        // next list another device puts is taken (Abbacchio's row 615:
        // one signed here and never taken wedged the device). Stale: the
        // worker's list, and the change again on top of it.
        if (error.problem != UserAppProblem.stale || attempt > 0) rethrow;
        await refreshKeys(accountId);
        continue;
      }
      // Taken: it's the person's list now, here too.
      await device.acceptList(account: accountId, deviceList: signed);
      // The home reads the new list at once when given the routes again
      // (Mista 32614, Fugo): a Claktory taken off it loses its binding in
      // seconds, not at the next round.
      _routesSent = '';
      if (_drained) unawaited(_sendRoutes(device));
      return;
    }
  }

  Future<void> _resume() async {
    _pause?.cancel();
    if (_paused case final paused? when !paused.isCompleted) paused.complete();
  }

  /// Starts again on this device: its keys and store go (what's on the
  /// person's other devices stays there).
  Future<void> startOver() async {
    await deleteHere();
    await start();
  }

  /// What signing out of the Claktory account does to the messages here
  /// (row 671, Fugo): nothing when there are none; kept, locked, when this
  /// is the only device with them; gone when the person's other devices
  /// have them ([leave]). A start under way is waited for, up to [wait].
  Future<DmSignOut> signOutPlan({
    Duration wait = const Duration(seconds: 10),
  }) async {
    if (setup == DmSetup.starting) await _settled(wait);
    final device = _device;
    if (device == null) return DmSignOut.none;
    final status = device.status();
    if (!status.setUp || status.accountId != accountId) return DmSignOut.none;
    final others =
        device
            .deviceList(account: accountId)
            ?.leaves
            .any(
              (leaf) => leaf.kind == DmLeafKind.device && leaf.leafId != leafId,
            ) ??
        false;
    return others ? DmSignOut.withOthers : DmSignOut.alone;
  }

  /// Once [setup] isn't [DmSetup.starting] any more, or [wait] passed.
  Future<void> _settled(Duration wait) {
    final settled = Completer<void>();
    void check() {
      if (setup != DmSetup.starting && !settled.isCompleted) {
        settled.complete();
      }
    }

    addListener(check);
    return settled.future
        .timeout(wait, onTimeout: () {})
        .whenComplete(() => removeListener(check));
  }

  /// Takes this device out of the person's messages before it signs out
  /// of the account, where their other devices have them (row 671, Fugo):
  /// a list without it, which it signs since it holds their security key
  /// (the worker then drops its key packages, and their other devices take
  /// it out of every group), then everything here deleted ([deleteHere]).
  /// It doesn't leave the groups itself. When the worker can't be reached,
  /// it's deleted here all the same.
  Future<void> leave() async {
    final device = _device;
    if (device != null) {
      final status = device.status();
      if (status.setUp && status.accountId == accountId) {
        try {
          await _changeList(
            device,
            (leaves) => [
              for (final each in leaves)
                if (each.leafId != leafId) each,
            ],
          );
        } catch (error) {
          problem = '$error';
        }
      }
    }
    await deleteHere();
  }

  /// Deletes the messages and keys on this device, the person's security
  /// key included: signing out where other devices have them ([leave]),
  /// or with Delete messages from this device (row 671). What's on their
  /// other devices stays there.
  Future<void> deleteHere() async {
    for (final name in DmKeyName.values) {
      await keys.delete(name);
    }
    await _forgetStore();
  }

  Future<void> _forgetStore() async {
    _device?.dispose();
    _device = null;
    await _removeStore('${await keys.folder()}/store.db');
  }

  /// Keeps the worker's supply of this device's key packages up. Each
  /// lasts thirty days (Melone's F4): the last resort is replaced when the
  /// worker has none or it's near its end ([DmDevice.lastResortDue]), and
  /// whenever it makes some, the device deletes its own past their
  /// lifetime, so none opens a Welcome after.
  Future<void> _topUpKeyPackages({bool force = false}) async {
    final device = _device;
    if (device == null) return;
    final supply = force ? null : await routes.keyPackageSupply();
    final left = supply?.remaining ?? 0;
    final nowSecs = _nowMs() ~/ 1000;
    final lastResort =
        supply == null ||
        !supply.lastResort ||
        device.lastResortDue(
          lastResortEnds: supply.lastResortEnds,
          nowSecs: nowSecs,
        );
    if (left >= _keyPackagesLow && !lastResort) return;
    try {
      await device.sweepKeyPackages(nowSecs: nowSecs);
    } catch (error) {
      problem = '$error';
    }
    await routes.uploadKeyPackages(
      left >= _keyPackagesLow
          ? const []
          : await device.keyPackages(count: dmKeyPackages - left),
      lastResort: lastResort ? await device.lastResortKeyPackage() : null,
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
    return _syncing = _syncLoop()
        .whenComplete(() => _syncing = null)
        .then((_) => _afterSync());
  }

  /// After a sync: a new key the worker refused for today is carried on
  /// once its day is over (Fugo 43752), outside the sync, which it runs
  /// itself.
  Future<void> _afterSync() async {
    final at = keyChangeAtMs;
    if (at == null || _nowMs() < at || _closed) return;
    keyChangeAtMs = null;
    await finishRemoval();
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
    _syncedMs = _nowMs();
    await _checkOwn();
    await _expireReceiver();
    try {
      // Read first, then swapped in at once: never empty meanwhile for
      // what reads it. The groups before the Welcomes (row 1150): one
      // listed here and added since has its Welcome in by then.
      final served = await routes.groups();
      for (final welcome in await routes.welcomes()) {
        await _join(device, welcome);
      }
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
        } else if (_drained && homeFeed.behind(group.groupId)) {
          // The home let go of what came past this device (away over 90
          // days): it joins again.
          _rejoinAfter.remove(group.groupId);
          await _rejoin(device, group);
        } else {
          var at = mine;
          // A commit of its own the app stopped before it heard of (row
          // 553): settled against the worker's log once a launch, merged
          // if the worker took it, given up if another took its epoch.
          if (_drained && at.pending && _settledPending.add(group.groupId)) {
            await _workerCommits(device, group);
            at =
                (await device.groups())
                    .where((each) => dmB64(each.groupId) == group.groupId)
                    .firstOrNull ??
                at;
          }
          if (_lastSeq(group) > at.seq) await _catchUp(device, group, at);
          await _commitsPastHome(device, group);
        }
        if (group.kind == 'self') await _followOwnKey(device);
        if (setup != DmSetup.ready) return;
      }
      await _moveGroups(device);
      await _followDead(device);
      await _followJoining(device);
      // Devices off the person's list come out of any group they're still
      // in, before anything is sent (Fugo, R5b).
      await _sweepRemovedDevices(device);
      // Claktories whose access ended come out before anything is sent
      // (Fugo L6).
      await _reconcileGrantsSafely(device, adding: false);
      // Over to the home only once it's the one connected (Mista 39824):
      // on another of the person's Claktories this device stays on the
      // worker, and what it sends waits for the home.
      final drainNow = !_drained && await _atHome(device);
      if (drainNow) await _dropUnapproved(device);
      await _sendUnsent(device);
      await _topUpKeyPackages();
      await _linkClaktory(device);
      await _reconcileGrantsSafely(device, adding: true);
      // The pins, to the person's Claktories: of a friend first met
      // since, and once a launch all of them, so a friend added after a
      // Claktory was enrolled is pinned there too and its home can take
      // that friend's new home (row 606). Their other devices pin their
      // friends themselves.
      if (_pinsToShare) {
        _pinsToShare = false;
        await _sharePins(channels: true);
      }
      // What the worker still held is read: from now on, the home only
      // (#451), followed from here.
      if (drainNow) {
        await device.setHomeDrained();
        _drained = true;
      }
      // Followed once a connection (and again after the app opened).
      if (_drained && !_followed) unawaited(_followHome());
      if (_drained) {
        await _sendRoutes(device);
        await _adoptHome(device);
      }
      _syncFailures = 0;
      _syncFailedAfter = null;
    } on UserAppError catch (error) {
      problem = error.toString();
      _syncFailed();
    } catch (error) {
      // Whatever else stops a sync is said, never left to stop the next.
      problem = '$error';
      _syncFailed();
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
    if (link == null || !_homeIsOurs) return;
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
    // Only the leaf of the Claktory the account listed by its endpoint:
    // never another one's it offers (Fugo 39914).
    if (!_ungated && offer.claktoryId != _ownClaktoryId) {
      return;
    }
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
    // Its key is in the group already: it's there, nothing to commit.
    if (commit == null) return true;
    try {
      await _commit(group.groupId, commit);
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
      await _commit(group.groupId, commit);
    } on UserAppError {
      await device.commitRefused(groupId: group.id);
      rethrow;
    }
    await _catchUpOne(device, group.groupId);
  }

  /// Groups here with a device under a replaced key, on no list of the
  /// new one, still in them, by group: the account it's of. Nothing is
  /// sent there until it's out (Group::send waits too; Fugo, row 630), and
  /// the conversation says so.
  final waitingFor = <String, String>{};

  /// Of those, the groups held by a reinstalled device's former install
  /// (row 202): the device's name, for the chat to say so.
  final reinstalling = <String, String>{};

  /// Each group here: the person's own dead devices taken out (a
  /// start-over whose removal didn't go through yet), and who it waits
  /// for otherwise.
  Future<void> _followDead(DmDevice device) async {
    final before = {...waitingFor};
    final reinstalledBefore = {...reinstalling};
    final here = {
      for (final group in await device.groups()) dmB64(group.groupId),
    };
    for (final group in [...groups.values]) {
      if (!here.contains(group.groupId)) {
        waitingFor.remove(group.groupId);
        reinstalling.remove(group.groupId);
        continue;
      }
      try {
        var waiting = await device.waitingFor(groupId: group.id);
        if (waiting == accountId && !_waitsAfterRefusal(group.groupId)) {
          try {
            await _removeDead(device, group);
          } catch (error) {
            // Tried again at the next sync; meanwhile it waits.
            problem = '$error';
          }
          waiting = await device.waitingFor(groupId: group.id);
        }
        if (waiting == null) {
          waitingFor.remove(group.groupId);
          reinstalling.remove(group.groupId);
        } else {
          waitingFor[group.groupId] = waiting;
          switch (await device.reinstalling(groupId: group.id)) {
            case final name?:
              reinstalling[group.groupId] = name;
            case null:
              reinstalling.remove(group.groupId);
          }
        }
      } catch (error) {
        problem = '$error';
      }
    }
    if (before.length != waitingFor.length ||
        before.entries.any((each) => waitingFor[each.key] != each.value) ||
        !mapEquals(reinstalledBefore, reinstalling)) {
      _notify();
    }
  }

  /// Takes the person's own dead devices out of [group] in one commit:
  /// after a start-over's rejoin, none holds the new epoch's secrets.
  Future<void> _removeDead(DmDevice device, DmServerGroup group) async {
    // No worker list versions yet: Polnareff's E11-8 app half (polna/e118)
    // passes GET groups' own, so a device on an older list waits.
    final commit = await device.removeDeadLeaves(
      groupId: group.id,
      listVersions: const [],
    );
    if (commit == null) return;
    try {
      await _commit(group.groupId, commit);
    } on UserAppError catch (error) {
      await device.commitRefused(groupId: group.id);
      if (error.problem == UserAppProblem.epoch) {
        _refusedAsBehind(group.groupId);
      }
      rethrow;
    }
    _commitTaken(group.groupId);
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
    final wanted = await _grants();
    final here = {
      for (final group in await device.groups()) dmB64(group.groupId),
    };
    for (final group in [...groups.values]) {
      if (group.kind != 'conversation' || !here.contains(group.groupId)) {
        continue;
      }
      final members = await device.members(groupId: group.id);
      final want = _wantIn(device, group, wanted);
      try {
        final out = await _ungranted(device, group, want);
        if (out.isNotEmpty) await _commitRemove(device, group, out);
        if (!adding) continue;
        // Not while the friend's key change waits for the person.
        final friend = group.accounts.where((each) => each != accountId);
        if (friend.any((each) => _unseenChange(device, each))) continue;
        for (final leaf in want) {
          if (members.any(
            (member) => member.account == accountId && member.leafId == leaf,
          )) {
            continue;
          }
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

  /// The account's live DM grants ([_wanted]), read again when last read
  /// a minute or more ago, or [fresh].
  Future<Map<String, Set<String>>> _grants({bool fresh = false}) async {
    if (fresh ||
        _wanted == null ||
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
    return _wanted!;
  }

  /// The person's Claktory leaves [group] should hold by [wanted]: each
  /// granted Claktory's newest leaf on their list, and none in a group
  /// that isn't its conversation's current one.
  Set<String> _wantIn(
    DmDevice device,
    DmServerGroup group,
    Map<String, Set<String>> wanted,
  ) {
    final current = groups.values
        .where(
          (each) =>
              each.kind == 'conversation' &&
              each.conversationId == group.conversationId,
        )
        .fold<DmServerGroup?>(
          null,
          (best, each) =>
              best == null || each.generation > best.generation ? each : best,
        );
    if (!identical(current, group)) return const {};
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
    return {
      for (final claktoryId in wanted[group.conversationId] ?? const <String>{})
        ?leafOf[claktoryId],
    };
  }

  /// This person's Claktories in [group] not in [want], checked or not:
  /// one taken off the list or under a replaced key fails the checks, yet
  /// must still go (Fugo, G5). Only this person's: a friend's are theirs
  /// to give the chat and take back.
  Future<List<String>> _ungranted(
    DmDevice device,
    DmServerGroup group,
    Set<String> want,
  ) async => [
    for (final leaf in await device.leaves(groupId: group.id))
      if (leaf.kind == DmLeafKind.claktory &&
          leaf.account == accountId &&
          !want.contains(leaf.leafId))
        leaf.leafId,
  ];

  /// Conversations whose Claktory leaves were checked against the grants
  /// since the app started, and since this device last joined them again
  /// (Melone F2.2, Fugo 40934): only these are sent to. A GroupInfo a
  /// rejoin took may be from before a Remove, with a Claktory whose grant
  /// was taken back still in it; kept only in memory, so a launch checks
  /// each again before its first send.
  final _grantsChecked = <String>{};

  /// Of those not checked, the ones joined again this run: checked against
  /// the grants read afresh, never as last read.
  final _rejoinedHere = <String>{};

  /// Takes out of [group] the person's Claktories its grants don't want
  /// (read afresh after a rejoin); throws when they can't be read or
  /// what's taken out isn't out yet, and it stays unchecked.
  Future<void> _checkGrants(DmDevice device, DmServerGroup group) async {
    final fresh = _rejoinedHere.contains(group.groupId);
    final want = _wantIn(device, group, await _grants(fresh: fresh));
    final out = await _ungranted(device, group, want);
    if (out.isNotEmpty) await _commitRemove(device, group, out);
    if ((await _ungranted(device, group, want)).isNotEmpty) {
      throw StateError("a Claktory no longer granted isn't out yet");
    }
    _rejoinedHere.remove(group.groupId);
    _grantsChecked.add(group.groupId);
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

  /// After joining [group] again at [until] (its own commit's seq), past
  /// what came after [read]: when that held anything said, a line in the
  /// chat says some messages can't be shown (row 654, Fugo: never silent,
  /// whatever the cause), where they were.
  Future<void> _noteSkipped(
    DmDevice device,
    DmServerGroup group,
    int read,
    int until,
    String because,
  ) async {
    var after = read;
    int? when;
    var skipped = false;
    try {
      pages:
      while (true) {
        final page = await routes.feed(group.groupId, after);
        for (final item in page.items) {
          if (item.seq >= until) break pages;
          when ??= item.acceptedMs;
          if (item.kind == 'application') {
            skipped = true;
            break pages;
          }
          after = item.seq;
        }
        if (!page.hasMore || page.items.isEmpty) break;
      }
    } on UserAppError catch (error) {
      // Gone from the worker: it can't be known, so it's said.
      if (error.problem != UserAppProblem.tooFarBehind) {
        problem = error.toString();
        return;
      }
      skipped = true;
    }
    if (!skipped) return;
    await device.noteSkipped(
      groupId: group.id,
      seq: read + 1,
      arrivedMs: when ?? _nowMs(),
      because: because,
    );
    _moved(group.groupId);
  }

  /// The person's groups this device couldn't join because a device of
  /// theirs in it still has their old security key (Abbacchio's row 611):
  /// one off their list, so it can't be named. Tried again by itself.
  final _behindOldKey = <String>{};

  /// Some of the person's chats can't come here until their other devices
  /// update (Your devices says so).
  bool get waitingForOtherDevices => _behindOldKey.isNotEmpty;

  /// Joins [group] by external commit: a group of the person's this device
  /// isn't in yet (a new device), or one it fell out of step with. Waits
  /// longer after each try that didn't take. [because]: the account whose
  /// security key changed past what this device can follow, if that's why.
  Future<void> _rejoin(
    DmDevice device,
    DmServerGroup group, {
    String because = '',
  }) async {
    final kind = group.kindFor(accountId);
    if (kind == null) return;
    final after = _rejoinAfter[group.groupId];
    if (after != null && _nowMs() < after) return;
    final tries = (_rejoinTries[group.groupId] ?? 0) + 1;
    _rejoinTries[group.groupId] = tries;
    _rejoinAfter[group.groupId] =
        _nowMs() + (retryAfter * min(1 << (tries - 1), 64)).inMilliseconds;
    try {
      if (kind.kind == DmGroupType.conversation) {
        for (final account in kind.accounts) {
          if (account != accountId) await refreshKeys(account);
        }
      }
      // Where it read to: what's after it and before its own commit is
      // skipped by joining again.
      final read = (await device.groups())
          .where((each) => dmB64(each.groupId) == group.groupId)
          .firstOrNull
          ?.seq;
      // What the home has of it so far, which this device skips by joining
      // again (#451): at the worker before.
      final last = _lastSeq(group);
      final info = await routes.groupInfo(group.groupId);
      // Not here at all: added meanwhile, maybe (Mista's row 1150). The
      // worker takes an add with its Welcome at once, so a GroupInfo read
      // before this holds this device's leaf only if its Welcome is there
      // now: it joins from that, never by an external commit that would
      // put its leaf in twice under one key. Added after, the GroupInfo
      // is from before, and the worker refuses the commit as behind.
      if (read == null) {
        final welcome = (await routes.welcomes())
            .where((each) => each.group.groupId == group.groupId)
            .firstOrNull;
        if (welcome != null) {
          await _join(device, welcome);
          _rejoinTries.remove(group.groupId);
          _rejoinAfter.remove(group.groupId);
          final mine = (await device.groups())
              .where((each) => dmB64(each.groupId) == group.groupId)
              .firstOrNull;
          if (mine != null) await _catchUp(device, group, mine);
          return;
        }
      }
      final commit = await device.rejoinGroup(
        groupId: group.id,
        kind: kind,
        groupInfo: info,
        seq: last,
      );
      final taken = await _commit(group.groupId, commit);
      _commitTaken(group.groupId);
      if (kind.kind == DmGroupType.conversation) {
        _grantsChecked.remove(group.groupId);
        _rejoinedHere.add(group.groupId);
      }
      if (held.remove(group.groupId) != null) _notify();
      if (read != null && kind.kind == DmGroupType.conversation) {
        if (_drained) {
          // The home sends nothing from before a cursor it let go of, so
          // what was skipped can't be read back: anything past what this
          // device read is said (Fugo: never silent).
          if (last > read) {
            await device.noteSkipped(
              groupId: group.id,
              seq: read + 1,
              arrivedMs: _nowMs(),
              because: because,
            );
            _moved(group.groupId);
          }
        } else {
          await _noteSkipped(device, group, read, taken.seq, because);
        }
      }
      _moved(group.groupId);
      if (_behindOldKey.remove(group.groupId)) _notify();
      if (_drained) {
        // What the home said of the group was of this device's place
        // before: followed afresh from the new one, it says again whether
        // it let go of anything past it, never joined again at every sync
        // on what it said before (row 582).
        homeFeed.groups.remove(group.groupId);
        _followed = false;
        unawaited(_followHome());
      }
      // Its commit comes back in the feed, which merges it.
      final mine = (await device.groups())
          .where((each) => dmB64(each.groupId) == group.groupId)
          .firstOrNull;
      if (mine != null) await _catchUp(device, group, mine);
      // A start-over's: the person's devices under the key it replaced
      // come out at once, in a commit of their own (row 630).
      await _removeDead(device, group);
      // Its Claktories against the grants as they are now, before
      // anything is sent there (Melone F2.2); tried again before a send.
      if (kind.kind == DmGroupType.conversation) {
        try {
          await _checkGrants(device, group);
        } catch (error) {
          problem = '$error';
        }
      }
    } on UserAppError catch (error) {
      problem = error.toString();
      if (error.problem == UserAppProblem.epoch ||
          error.problem == UserAppProblem.waitingForFriend) {
        // Another commit came first, or the friend isn't in yet (the
        // worker lets only a member's add bring them, row 676): a sync
        // after the wait tries again.
        await device.forgetGroup(groupId: group.id);
      }
    } catch (error) {
      problem = '$error';
      // A member under the person's replaced key refuses the join.
      if (problem.contains(
            "$accountId's certificate is from a security key that was "
            'replaced',
          ) &&
          _behindOldKey.add(group.groupId)) {
        _notify();
      }
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
    // At the home: the worker's proposals, read once the home sends a
    // commit, each taken before the commit of its epoch.
    List<DmFeedItem>? proposals;
    while (!_closed) {
      final ({List<DmFeedItem> items, bool hasMore}) page;
      if (_drained) {
        // At the home: what it sent this device, in order.
        if (homeFeed.behind(group.groupId)) {
          _rejoinAfter.remove(group.groupId);
          return _rejoin(device, group);
        }
        page = (
          items: [
            for (final entry in homeFeed.page(group.groupId, after))
              DmFeedItem(
                seq: entry.seq,
                kind: entry.commit ? 'commit' : 'application',
                envelope: entry.envelope,
                acceptedMs: entry.acceptedMs,
                epoch: entry.epoch,
              ),
          ],
          hasMore: false,
        );
      } else {
        try {
          page = await routes.feed(group.groupId, after);
        } on UserAppError catch (error) {
          if (error.problem == UserAppProblem.tooFarBehind) {
            _rejoinAfter.remove(group.groupId);
            return _rejoin(device, group);
          }
          rethrow;
        }
      }
      for (final item in page.items) {
        // Taken already, in a window (row 561).
        if (item.seq <= after) continue;
        if (_drained && item.kind == 'commit' && item.seq > after) {
          proposals ??= await _workerProposals(device, group);
          await _takeProposals(device, group, proposals, item.epoch);
        }
        Future<DmProcessed> take() => device.process(
          groupId: group.id,
          seq: item.seq,
          envelope: item.envelope,
          arrivedMs: item.acceptedMs,
        );
        var processed = await take();
        // From an epoch past this device's (row 551): a commit its home
        // never carried, which the worker's log has. Taken from there,
        // then this again; never passed over unread meanwhile.
        if (processed.outcome == DmOutcome.held &&
            processed.behind &&
            !_drained) {
          // The worker's own feed has every commit in its place: one past
          // this device's epoch there is unreadable here, as before, and
          // a commit past it means it's out of step: it joins again.
          await device.skip(groupId: group.id, seq: item.seq);
          if (processed.handshake) return _rejoin(device, group);
          after = item.seq;
          continue;
        }
        if (processed.outcome == DmOutcome.held && processed.behind) {
          final read = await _workerCommits(device, group);
          processed = await take();
          if (processed.outcome == DmOutcome.held && processed.behind) {
            final worker = await _workerEpoch(group);
            if (read && worker != null && worker > processed.epoch.toInt()) {
              // The worker is past this device, but nothing it has brings
              // it there (let go of, or refused here): it joins again, and
              // the chat says what it couldn't read (Mista).
              behind.remove(group.groupId);
              _rejoinAfter.remove(group.groupId);
              return _rejoin(device, group);
            }
            if (worker != null && worker <= processed.epoch.toInt()) {
              // No such epoch anywhere: never readable here. Passed over,
              // and said.
              await device.skip(groupId: group.id, seq: item.seq);
              await device.noteSkipped(
                groupId: group.id,
                seq: item.seq,
                arrivedMs: item.acceptedMs,
                because: '',
              );
              after = item.seq;
              homeFeed.taken(group.groupId, after);
              _ackLater(group.groupId, after);
              behind.remove(group.groupId);
              _moved(group.groupId);
              continue;
            }
            // The worker out of reach: the next sync tries again.
            behind.putIfAbsent(group.groupId, _nowMs);
            return _notify();
          }
          if (behind.remove(group.groupId) != null) _notify();
        }
        // A leaf their list as taken here doesn't name (row 561): a
        // device of theirs added while this one was away. Their list is
        // fetched and it's taken again; still unnamed, it's refused as
        // before: passed over, and a commit means this device is out of
        // step, so it joins again.
        if (processed.outcome == DmOutcome.held && processed.unlisted) {
          await refreshKeys(processed.why);
          processed = await take();
          if (processed.outcome == DmOutcome.held && processed.unlisted) {
            // A device they've since taken off their list, brought in while
            // this one was away (row 561): if one of their listed devices
            // that was there all along took it out again, everything up to
            // that is read as an online member read it (Fugo's rule, in
            // the crate).
            if (processed.handshake && group.kind == 'conversation') {
              final window = await _window(device, group, [
                for (final each in page.items)
                  if (each.seq >= item.seq) each,
              ], more: page.hasMore);
              if (window case (:final taken, :final last)?) {
                for (final each in taken) {
                  if (each.outcome == DmOutcome.committed) {
                    epoch = each.epoch.toInt();
                  }
                  if (each.message case final message?) {
                    await _confirmed(message);
                  }
                }
                _rejoinTries.remove(group.groupId);
                behind.remove(group.groupId);
                held.remove(group.groupId);
                after = last;
                if (_drained) {
                  homeFeed.taken(group.groupId, after);
                  _ackLater(group.groupId, after);
                }
                _moved(group.groupId);
                continue;
              }
              if (_windowWaits.contains(group.groupId)) {
                // Its removal may still come: held, and said.
                behind.putIfAbsent(group.groupId, _nowMs);
                return _notify();
              }
            }
            behind.remove(group.groupId);
            await device.skip(groupId: group.id, seq: item.seq);
            if (processed.handshake && item.epoch >= epoch) {
              return _rejoin(device, group, because: 'device:${processed.why}');
            }
            after = item.seq;
            if (_drained) {
              homeFeed.taken(group.groupId, after);
              _ackLater(group.groupId, after);
            }
            continue;
          }
        }
        // A commit under a key not pinned yet (someone changed their
        // security key): a friend's keys are fetched and it's taken
        // again; the person's own wait for their self group. Until then
        // it's held, unread, and nothing after it is read (Ghiaccio, Fugo:
        // never dropped, so the group can go on once the key is in).
        if (processed.outcome == DmOutcome.held &&
            !processed.replaced &&
            processed.why != accountId) {
          final served = await refreshKeys(processed.why);
          processed = await take();
          // Still held: under a key they replaced since, which the worker
          // lists and this commit proves (row 654, Fugo's conditions in
          // the crate). Retired, it's one this device can never take.
          if (processed.outcome == DmOutcome.held &&
              !processed.replaced &&
              served != null &&
              served.replacedIdentityKeys.isNotEmpty) {
            try {
              if (await device.retireReplaced(
                groupId: group.id,
                envelope: item.envelope,
                account: processed.why,
                served: served.identityKey,
                listed: served.replacedIdentityKeys,
              )) {
                processed = await take();
              }
            } catch (error) {
              problem = '$error';
            }
          }
        }
        // Never to be taken here: it joins the group again past it, and
        // the chat says what it couldn't read (row 654).
        if (processed.outcome == DmOutcome.held && processed.replaced) {
          held.putIfAbsent(group.groupId, _nowMs);
          _notify();
          return _rejoin(device, group, because: processed.why);
        }
        if (processed.outcome == DmOutcome.held) {
          if (processed.why == accountId) _ownKeyCheck = true;
          held.putIfAbsent(group.groupId, _nowMs);
          return _notify();
        }
        held.remove(group.groupId);
        after = item.seq;
        if (_drained) {
          homeFeed.taken(group.groupId, after);
          _ackLater(group.groupId, after);
        }
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
          await _confirmed(message);
          // A device this one added asks for its history again; never one
          // marked as a departed device's (Fugo 40397).
          if (group.kind == 'self' &&
              message.content.kind == 'history_again' &&
              !message.mine &&
              !message.departed &&
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

  /// Groups whose pending commit was settled against the worker since the
  /// app opened (row 553).
  final _settledPending = <String>{};

  /// Groups whose feed at the home waits at something from an epoch past
  /// this device's (row 551), and since when (by [_nowMs]): until the
  /// commit it missed comes from the worker. The chat says a message
  /// waits.
  final behind = <String, int>{};

  /// Takes the worker's commits [group] has past this device's position
  /// there, with the proposals before them, once it reads the home (row
  /// 551): one the home never carried, such as this device's own taken by
  /// the worker just as the app stopped. Whether it read all the worker
  /// has (or the worker let go of some): false when it stopped short, at
  /// a commit under a key not pinned yet or with the worker out of reach.
  Future<bool> _workerCommits(DmDevice device, DmServerGroup group) async {
    var after = device.proposalSeq(groupId: group.id);
    try {
      while (!_closed) {
        final page = await routes.feed(group.groupId, after);
        for (final item in page.items) {
          Future<DmProcessed> take() => item.kind == 'commit'
              ? device.takeWorkerCommit(
                  groupId: group.id,
                  seq: item.seq,
                  envelope: item.envelope,
                  arrivedMs: item.acceptedMs,
                )
              : device.takeProposal(
                  groupId: group.id,
                  seq: item.seq,
                  envelope: item.envelope,
                );
          var taken = await take();
          // A leaf their list here doesn't name yet (row 561): fetched,
          // then again; still unnamed, refused and passed over.
          if (taken.outcome == DmOutcome.held && taken.unlisted) {
            await refreshKeys(taken.why);
            taken = await take();
            if (taken.outcome == DmOutcome.held && taken.unlisted) {
              await device.skipProposals(groupId: group.id, seq: item.seq);
              after = item.seq;
              continue;
            }
          }
          // Under a key not pinned yet: what comes after waits for it.
          if (taken.outcome == DmOutcome.held) return false;
          if (taken.outcome == DmOutcome.committed) {
            _commitTaken(group.groupId);
            _moved(group.groupId);
          }
          after = item.seq;
        }
        if (!page.hasMore || page.items.isEmpty) break;
      }
      return !_closed;
    } on UserAppError catch (error) {
      if (error.problem == UserAppProblem.tooFarBehind) {
        await device.skipProposals(groupId: group.id, seq: group.lastSeq);
        return true;
      }
      problem = error.toString();
    } catch (error) {
      problem = '$error';
    }
    return false;
  }

  /// Groups whose window (row 561) is still open with more commits to
  /// come: held until they do.
  final _windowWaits = <String>{};

  /// Takes [items] (a commit held as unlisted, then what follows it) as
  /// one window ([DmDevice.processWindow], row 561): what each was and the
  /// last seq taken once it closed; null when it didn't, with [group] in
  /// [_windowWaits] while more commits are to come ([more] items in the
  /// feed, or the worker past the last epoch here). At most
  /// [dmWindowMost] items.
  Future<({List<DmProcessed> taken, int last})?> _window(
    DmDevice device,
    DmServerGroup group,
    List<DmFeedItem> items, {
    required bool more,
  }) async {
    _windowWaits.remove(group.groupId);
    final some = items.take(dmWindowMost).toList();
    // Before the home, the worker's feed comes a page at a time.
    while (more && !_drained && some.length < dmWindowMost) {
      final page = await routes.feed(group.groupId, some.last.seq);
      some.addAll(page.items.take(dmWindowMost - some.length));
      more = page.hasMore && page.items.isNotEmpty;
    }
    final outcome = await device.processWindow(
      groupId: group.id,
      items: [
        for (final item in some)
          DmWindowItem(
            seq: item.seq,
            envelope: item.envelope,
            arrivedMs: item.acceptedMs,
          ),
      ],
    );
    switch (outcome.end) {
      case DmWindowEnd.closed:
        return (
          taken: outcome.processed,
          last: some[outcome.processed.length - 1].seq,
        );
      case DmWindowEnd.open when some.length < dmWindowMost:
        final reached = some
            .where((item) => item.kind == 'commit')
            .fold(0, (most, item) => max(most, item.epoch + 1));
        final worker = await _workerEpoch(group);
        if (more || worker == null || worker > reached) {
          _windowWaits.add(group.groupId);
        }
        return null;
      case _:
        if (outcome.why.isNotEmpty) problem = outcome.why;
        return null;
    }
  }

  /// The worker's epoch of each group when its log was last read for
  /// commits the home never carried ([_commitsPastHome]).
  final _readPastHome = <String, int>{};

  /// Once the home's feed is taken: a group the worker has past this
  /// device has a commit the home never carried, from a device connected to
  /// another of the person's Claktories, say (a new one joining, Zucchero's
  /// row 568). It's read from the worker's log, once for each epoch the
  /// worker lists, rather than waiting for something said after it.
  Future<void> _commitsPastHome(DmDevice device, DmServerGroup group) async {
    if (!_drained || _readPastHome[group.groupId] == group.epoch) return;
    final here = (await device.groups())
        .where((each) => dmB64(each.groupId) == group.groupId)
        .firstOrNull;
    if (here == null || group.epoch <= here.epoch.toInt()) return;
    _readPastHome[group.groupId] = group.epoch;
    if (await _workerCommits(device, group)) _moved(group.groupId);
  }

  /// Whether the Claktory the app shows is the person's home, asked if
  /// it hasn't said yet: it says so, their list names it, or their list
  /// names none ([_adoptHome] makes it the home then). False while it
  /// isn't connected.
  Future<bool> _atHome(DmDevice device) async {
    if (_ours == null) return false;
    if (homeState == null) await _sendRoutes(device);
    if (homeState == null) return false;
    final named = homeEndpoint;
    return homeIsHere || named == null || named.isEmpty;
  }

  /// [group]'s epoch as the worker has it now; null when it can't say.
  Future<int?> _workerEpoch(DmServerGroup group) async {
    try {
      for (final each in await routes.groups()) {
        if (each.groupId == group.groupId) return each.epoch;
      }
    } catch (error) {
      problem = '$error';
    }
    return null;
  }

  Future<void> _confirmed(DmMessage message) async {
    if (!message.mine) return;
    sending.remove(message.content.clientMessageId);
    _takenBy.remove(message.content.clientMessageId);
    // The person's rotation message back from the log: said, whatever
    // answer was lost on the way, so it's never encrypted again under its
    // id (E11-9, Fugo 43016: the home refuses other bytes as a conflict).
    if (message.content.kind == 'rotation') await _rotationCameBack(message);
  }

  /// Marks the rotation under way as said when [message], this device's
  /// own from the self group's log, is its message: the new key it names
  /// is the rotation's.
  Future<void> _rotationCameBack(DmMessage message) async {
    final device = _device;
    if (device == null) return;
    try {
      final rotation = await device.rotation();
      if (rotation == null || rotation.said) return;
      final said = jsonDecode(message.content.extraJson);
      if (said is! Map || said['identity'] != dmB64(rotation.identityPublic)) {
        return;
      }
      await device.rotationSaid();
      // The Lock out it's for carries on by itself: what it waited for
      // (one of the person's own Claktories) is here. One going now
      // carries on once it stops.
      if (removing) {
        _saidMeanwhile = true;
      } else if (removal != null) {
        scheduleMicrotask(() => unawaited(finishRemoval()));
      }
    } catch (error) {
      problem = '$error';
    }
  }

  /// Which home (its endpoint, base64) took each of this device's messages
  /// that hasn't come back yet, by its retry id: one a home took that then
  /// stopped being the home goes to the new one (row 578).
  final _takenBy = <String, String>{};

  /// The endpoint of the home this device is connected to; null when the
  /// Claktory connected isn't the home, or hasn't said.
  String? get _homeHere =>
      homeIsHere && homeState != null ? dmB64(homeState!.endpointId) : null;

  /// What the worker still has for [group] after where this device is
  /// there (#451, Mista 32950): once it reads the home, only proposals
  /// (a Claktory's own Remove) and commits, which the home sends too.
  /// What the worker pruned meanwhile is gone: passed over.
  Future<List<DmFeedItem>> _workerProposals(
    DmDevice device,
    DmServerGroup group,
  ) async {
    final items = <DmFeedItem>[];
    var after = device.proposalSeq(groupId: group.id);
    while (!_closed) {
      final ({List<DmFeedItem> items, bool hasMore}) page;
      try {
        page = await routes.feed(group.groupId, after);
      } on UserAppError catch (error) {
        if (error.problem != UserAppProblem.tooFarBehind) rethrow;
        await device.skipProposals(groupId: group.id, seq: group.lastSeq);
        return items;
      }
      items.addAll(page.items);
      if (!page.hasMore || page.items.isEmpty) break;
      after = page.items.last.seq;
    }
    return items;
  }

  /// Takes [waiting]'s proposals made at [epoch] or before, in order,
  /// before the home's commit at [epoch], which may take them in by
  /// reference; anything else there is passed over. One under a key not
  /// pinned yet waits, and what comes after it.
  Future<void> _takeProposals(
    DmDevice device,
    DmServerGroup group,
    List<DmFeedItem> waiting,
    int epoch,
  ) async {
    var passed = 0;
    Future<void> pass() async {
      if (passed == 0) return;
      await device.skipProposals(groupId: group.id, seq: passed);
      passed = 0;
    }

    while (waiting.isNotEmpty) {
      final item = waiting.first;
      if (item.kind != 'proposal') {
        passed = item.seq;
        waiting.removeAt(0);
        continue;
      }
      if (item.epoch > epoch) break;
      await pass();
      Future<DmProcessed> take() => device.takeProposal(
        groupId: group.id,
        seq: item.seq,
        envelope: item.envelope,
      );
      var taken = await take();
      if (taken.outcome == DmOutcome.held && taken.why != accountId) {
        await refreshKeys(taken.why);
        taken = await take();
      }
      if (taken.outcome == DmOutcome.held) return;
      waiting.removeAt(0);
    }
    await pass();
  }

  /// Sends what was encrypted here and hasn't come back: after a restart,
  /// or an answer that never came.
  Future<void> _sendUnsent(DmDevice device) async {
    for (final unsent in await device.unsent()) {
      // An agent's message is sealed only once the person approved it
      // (#451), so it goes like theirs.
      final state = sending[unsent.clientMessageId];
      // Taken by a home that isn't the home any more, and connected to the
      // one that is: it goes there, as at a relaunch (row 578, Mista
      // 40770). The friend drops the copy, its keys used.
      final takenElsewhere =
          state?.state == DmSendState.sent &&
          switch ((_takenBy[unsent.clientMessageId], _homeHere)) {
            (final by?, final here?) => by != here,
            _ => false,
          };
      if (state != null &&
          state.state != DmSendState.unknown &&
          state.state != DmSendState.waiting &&
          !takenElsewhere) {
        continue;
      }
      await _post(device, unsent);
    }
  }

  /// Before the home (#451), an agent's message was sealed before its
  /// approval and kept until the service sent it: one left from then may
  /// never have been approved, so it goes rather than through the home.
  Future<void> _dropUnapproved(DmDevice device) async {
    for (final unsent in await device.unsent()) {
      if (unsent.content.byAgent == null ||
          _approvedHere.contains(unsent.clientMessageId)) {
        continue;
      }
      await device.dropUnsent(
        groupId: unsent.groupId,
        clientMessageId: unsent.clientMessageId,
      );
    }
  }

  /// Agents' messages the person approved here since this started.
  final _approvedHere = <String>{};

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
      // A conversation's Claktories not checked against the grants since
      // launch or its last rejoin: checked first, or nothing goes (Melone
      // F2.2, Fugo 40934); a group not known here can't be, so nothing
      // goes either.
      final known = groups[group];
      if (known == null) {
        throw StateError("this conversation's Claktories can't be checked");
      }
      if (known.kind == 'conversation' && !_grantsChecked.contains(group)) {
        await _checkGrants(device, known);
      }
      await device.send(
        groupId: dmUnb64(group),
        content: content,
        nowMs: _nowMs(),
        // As removeDeadLeaves: the worker's versions come with polna/e118.
        listVersions: const [],
      );
    } catch (error) {
      problem = '$error';
      sending[content.clientMessageId] = DmSending(
        DmSendState.refused,
        group: group,
        text: content.text ?? '',
        // The friend isn't in yet (row 676): nothing was encrypted. A
        // key change only when the chat has its note (E11-3).
        problem: problem.contains(' to join')
            ? 'waitingForFriend'
            : _keyStateIn(device, group)
            ? 'unencryptable'
            : '',
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
    if (_home case final link?) {
      // Not the person's own, or not confirmed: it waits (Fugo 39894).
      await _checkOwn();
      if (!_homeIsOurs) {
        sending[id] = DmSending(
          DmSendState.waiting,
          group: group,
          problem: switch (ownership) {
            DmOwnership.notOurs => 'otherAccount',
            DmOwnership.unconfirmable => 'unconfirmable',
            _ => '',
          },
        );
        return;
      }
      return _postHome(device, link, unsent);
    }
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

  /// Hands [unsent] to the person's home (#451), the same bytes on every
  /// try (the home answers a repeat with its first receipt). With no
  /// answer, or another Claktory than the home, it waits on this device
  /// and goes by itself once the home answers: "Waiting for your
  /// Claktory".
  Future<void> _postHome(
    DmDevice device,
    DmHomeLink link,
    DmUnsent unsent,
  ) async {
    final group = dmB64(unsent.groupId);
    final id = unsent.clientMessageId;
    DmReceipt? receipt;
    if (link.connected.value) {
      try {
        receipt = await link.send(
          DmSend(
            groupId: unsent.groupId,
            envelope: dmBytesOf(unsent.envelope),
            clientMessageId: id,
            leafId: leafId,
          ),
        );
      } catch (error) {
        problem = '$error';
      }
    }
    void refused(String why) => sending[id] = DmSending(
      DmSendState.refused,
      group: group,
      problem: why,
      text: unsent.content.text ?? '',
    );
    void waits(String why, [int afterMs = 0]) {
      sending[id] = DmSending(DmSendState.waiting, group: group, problem: why);
      if (afterMs > 0) {
        Timer(Duration(milliseconds: afterMs), () {
          if (!_closed) unawaited(sync());
        });
      }
    }

    switch (receipt?.result) {
      case null:
        waits('unreachable');
      case DmSendResult.DM_SEND_RESULT_STORED ||
          DmSendResult.DM_SEND_RESULT_DUPLICATE ||
          DmSendResult.DM_SEND_RESULT_HELD:
        // Taken: it shows as sent once it comes back from the home.
        sending[id] = DmSending(DmSendState.sent, group: group);
        if (homeState case final home?) _takenBy[id] = dmB64(home.endpointId);
      case DmSendResult.DM_SEND_RESULT_TOO_OLD when _resealed.add(id):
        // This device's epoch is too far behind the home's: caught up, it's
        // encrypted again under the same id (the home kept nothing of it;
        // an approved proposal needs no second approval, Mista 32507).
        await device.dropUnsent(groupId: unsent.groupId, clientMessageId: id);
        await _catchUpOne(device, group);
        await say(
          group,
          DmContent(
            kind: unsent.content.kind,
            clientMessageId: id,
            text: unsent.content.text,
            byAgent: unsent.content.byAgent,
            extraJson: unsent.content.extraJson,
          ),
        );
        _resealed.remove(id);
        return;
      case DmSendResult.DM_SEND_RESULT_TOO_OLD:
        // Still too old, sealed again: it tries again by itself.
        waits('behind', 5000);
      case DmSendResult.DM_SEND_RESULT_WAITING_FOR_FRIEND:
        await device.dropUnsent(groupId: unsent.groupId, clientMessageId: id);
        refused('waitingForFriend');
      case DmSendResult.DM_SEND_RESULT_HOME_FULL:
        waits('homeFull', receipt!.retryAfterMs.toInt());
      case DmSendResult.DM_SEND_RESULT_RATE_LIMITED:
        waits('rateLimited', receipt!.retryAfterMs.toInt());
      case DmSendResult.DM_SEND_RESULT_NOT_HOME:
        waits('notHome');
      default:
        // CONFLICT, or an answer this app doesn't know: it won't go.
        await device.dropUnsent(groupId: unsent.groupId, clientMessageId: id);
        refused(receipt!.result.name);
    }
    _moved(group);
  }

  /// Messages sent again under a new retry id (after 409 epoch): old → new.
  final _replaced = <String, String>{};

  /// Messages being sealed again after the home found them too old.
  final _resealed = <String>{};

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

  /// Everyone this device has a conversation with, and the person: their
  /// keys and lists as the worker has them now, and the home's routes from
  /// them. The worker says a list changed only to a device following it,
  /// so one changed while the app was closed (a friend's home moved,
  /// Zucchero's row 567) is read here, as following starts.
  ///
  /// Of a conversation's accounts, only those pinned here: never a first
  /// contact the service's list of a group's accounts alone would bring.
  /// The person's friends, whom they chose, are pinned here at friending
  /// when they aren't yet, as in [_meetFriends].
  Future<void> _refreshAllKeys() async {
    final device = _device;
    if (device == null) return;
    final accounts = {
      accountId,
      for (final group in await _groupsHere(device))
        if (group.kind == 'conversation') ...group.accounts,
      ..._friends,
    }..remove('');
    for (final account in accounts) {
      if (_closed) return;
      if (device.pin(account: account) == null && !_friends.contains(account)) {
        continue;
      }
      try {
        await refreshKeys(account);
      } catch (error) {
        problem = '$error';
      }
    }
    if (_drained) await _sendRoutes(device);
  }

  /// [account]'s keys, as the worker has them now, against what this
  /// device pinned: pinned at first contact, a changed key marked so the
  /// person sees it before anything is sent, and the device list taken.
  Future<DmAccountKeys?> refreshKeys(String account) async {
    final device = _device;
    if (device == null) return null;
    final now = await routes.keys(account);
    if (now == null) return null;
    if (account == accountId) signedOut = {...now.signedOut};
    final pin = device.pin(account: account);
    if (pin == null) {
      await device.firstContact(
        account: account,
        identityPublic: now.identityKey,
      );
      _pinsToShare = true;
    } else if (!listEquals(pin.identityPublic, now.identityKey)) {
      // The person's own key changes only through their self group
      // ([_followOwnKey]), never from the worker alone.
      if (account == accountId) {
        _ownKeyCheck = true;
        return now;
      }
      final verified = pin.verified;
      try {
        await device.keyChanged(
          account: account,
          identityPublic: now.identityKey,
        );
        if (verified) wasVerified.add(account);
        keyRefused.remove(account);
      } catch (error) {
        // A key this device won't take (one they replaced for good, say):
        // the chat says their key changed, never that it waits for them
        // to join (E11-3).
        problem = '$error';
        if (keyRefused.add(account)) _notify();
        return now;
      }
    } else {
      keyRefused.remove(account);
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
    return now;
  }

  /// The person saw that [account]'s security key changed: sending to
  /// them goes on.
  Future<void> seen(String account) async {
    await _device?.seen(account: account);
    wasVerified.remove(account);
    await _sharePins();
    _notify();
    _keyStateMoved(account);
  }

  /// The person compared safety numbers with [account] after its key
  /// changed, and they didn't match (E11-3): the key it changed from is
  /// pinned again, and the one that didn't match can come back only as
  /// another change, whose note says so. False when there's no change to
  /// undo (none, or the earlier key is gone for good).
  Future<bool> undoKeyChange(String account) async {
    final device = _device;
    final backTo = device?.pin(account: account)?.changedFrom;
    if (device == null || backTo == null || account == accountId) {
      return false;
    }
    try {
      await device.undoKeyChange(account: account, backTo: backTo);
    } catch (error) {
      problem = '$error';
      _notify();
      return false;
    }
    wasVerified.remove(account);
    await _sharePins();
    // A service still giving the key that didn't match brings its note
    // back at once, unseen and marked, so nothing goes meanwhile to a leaf
    // under it (Fugo 41541).
    try {
      await refreshKeys(account);
    } catch (error) {
      problem = '$error';
    }
    _notify();
    _keyStateMoved(account);
    return true;
  }

  /// [account]'s key state moved by the person's word: what waited on it
  /// (their devices' add to a chat, row 1440) is tried at once, not at the
  /// next sync or after its wait.
  void _keyStateMoved(String account) {
    for (final group in groups.values) {
      if (!group.accounts.contains(account)) continue;
      _joinAfter.remove(group.groupId);
      _joinTries.remove(group.groupId);
    }
    unawaited(sync());
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

  /// Friends whose key, as the service gives it now, this device refused
  /// to take ([DmDevice.keyChanged] said no): the chat says their key
  /// changed (E11-3). Gone once the service gives the key pinned here, or
  /// one this device takes.
  final keyRefused = <String>{};

  /// The person compared safety numbers with [account].
  Future<void> verify(String account) async {
    await _device?.verify(account: account);
    await _sharePins();
    _notify();
  }

  /// Whether the next sync shares the pins ([_sharePins]): once a launch,
  /// and again after a friend is first pinned here (row 606).
  var _pinsToShare = true;

  /// The pins, to the person's other devices and their Claktories.
  /// [channels]: to their Claktories only, which have no pins but the
  /// ones given them (row 606).
  Future<void> _sharePins({bool channels = false}) async {
    final device = _device;
    if (device == null) return;
    for (final group in groups.values.toList()) {
      if ((group.kind == 'self' && !channels) || group.kind == 'channel') {
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

  /// Whether a friend in [group] has a key change the chat shows a note
  /// for: one not seen yet, or one this device refused.
  bool _keyStateIn(DmDevice device, String group) =>
      (groups[group]?.accounts ?? const <String>[])
          .where((each) => each != accountId)
          .any(
            (each) => keyRefused.contains(each) || _unseenChange(device, each),
          );

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
    try {
      await _addEveryone(device, group, [friend, accountId]);
    } catch (error) {
      // Nothing is sent until the friend is in (row 676): the chat says
      // it waits, and the next sync adds them ([_followJoining]).
      problem = '$error';
    }
    await _followJoining(device);
    return group;
  }

  /// Adds every listed device of [accounts] that [group] doesn't have
  /// yet, from their key packages, in one commit with its Welcome. When
  /// another commit got there first (an own device joining, row 676), it
  /// catches up and works out again what's missing, with a new claim,
  /// up to [_addTries] times: never a claim from before (Fugo).
  Future<void> _addEveryone(
    DmDevice device,
    DmServerGroup group,
    List<String> accounts,
  ) async {
    for (var attempt = 1; ; attempt++) {
      final here = {
        for (final leaf in await device.leaves(groupId: group.id))
          (leaf.account, leaf.leafId),
      };
      final adding = {...here};
      final packages = <String>[];
      for (final account in accounts) {
        // Listed devices only: a Claktory comes in with a grant. A friend
        // not pinned yet: whichever the worker has, checked as they're
        // added.
        final listed = device.deviceList(account: account)?.leaves;
        final List<String>? only = listed == null && account != accountId
            ? null
            : [
                for (final leaf in listed ?? const <DmListed>[])
                  if (leaf.kind == DmLeafKind.device &&
                      !(account == accountId && leaf.leafId == leafId) &&
                      !here.contains((account, leaf.leafId)))
                    leaf.leafId,
              ];
        if (only != null && only.isEmpty) continue;
        for (final claimed in await routes.claim(account, leafIds: only)) {
          if (account == accountId && claimed.leafId == leafId) continue;
          // One a leaf in the group already has, or a second for one
          // leaf (Ghiaccio): never added twice.
          if (!adding.add((account, claimed.leafId))) continue;
          packages.add(claimed.keyPackage);
        }
      }
      if (packages.isEmpty) return;
      // Each is checked against its account's signed list as it's added
      // (Group::add): one that isn't on it refuses the whole add. None:
      // each package's key is in the group already (Group::add leaves
      // those out), so nothing to add (told by its kind, Fugo).
      final commit = await device.addMembers(
        groupId: group.id,
        keyPackages: packages,
      );
      if (commit == null) return;
      try {
        await _commit(group.groupId, commit);
      } on UserAppError catch (error) {
        await device.commitRefused(groupId: group.id);
        if (error.problem != UserAppProblem.epoch || attempt >= _addTries) {
          rethrow;
        }
        await _catchUpOne(device, group.groupId);
        continue;
      }
      _commitTaken(group.groupId);
      await _catchUpOne(device, group.groupId);
      return;
    }
  }

  static const _addTries = 3;

  /// Conversations here the friend isn't in yet (row 676), by group: the
  /// friend's account. Nothing is sent there until they are (Group::send
  /// refuses too), and the chat says so.
  final joining = <String, String>{};

  /// Since when each of [joining] has waited, and when its next add may
  /// be tried.
  final _joiningSince = <String, int>{};
  final _joinAfter = <String, int>{};
  final _joinTries = <String, int>{};

  /// How long the person's other devices leave the friend's add to the
  /// first of them (the creator, unless it left), so they don't each
  /// spend the friend's key packages (Fugo).
  static const joinFirstFor = Duration(minutes: 2);

  /// Each conversation here the friend isn't in yet: said in the chat,
  /// and the friend's devices added by the person's first member there,
  /// or by any of them once that's waited [joinFirstFor]. With no key
  /// package of the friend's, it waits: their device joins when it next
  /// syncs.
  Future<void> _followJoining(DmDevice device) async {
    final before = {...joining};
    final here = {
      for (final group in await device.groups()) dmB64(group.groupId),
    };
    for (final group in [...groups.values]) {
      if (group.kind != 'conversation' || !here.contains(group.groupId)) {
        joining.remove(group.groupId);
        continue;
      }
      try {
        final waits = await device.notJoined(groupId: group.id);
        if (waits == null) {
          joining.remove(group.groupId);
          _joiningSince.remove(group.groupId);
          _joinAfter.remove(group.groupId);
          _joinTries.remove(group.groupId);
          continue;
        }
        joining[group.groupId] = waits.account;
        final since = _joiningSince[group.groupId] ??= _nowMs();
        final now = _nowMs();
        if (!waits.addsFirst && now - since < joinFirstFor.inMilliseconds) {
          continue;
        }
        if (now < (_joinAfter[group.groupId] ?? 0)) continue;
        try {
          await _addEveryone(device, group, [waits.account]);
        } catch (error) {
          problem = '$error';
          final tries = (_joinTries[group.groupId] ?? 0) + 1;
          _joinTries[group.groupId] = tries;
          _joinAfter[group.groupId] =
              now + (retryAfter * min(1 << (tries - 1), 64)).inMilliseconds;
        }
        if (await device.notJoined(groupId: group.id) == null) {
          joining.remove(group.groupId);
          _joiningSince.remove(group.groupId);
        }
      } catch (error) {
        problem = '$error';
      }
    }
    if (!mapEquals(before, joining)) _notify();
  }

  /// Takes [group]'s feed now, outside a whole [sync].
  Future<void> _catchUpOne(DmDevice device, String group) async {
    final known = groups[group];
    final mine = (await device.groups())
        .where((each) => dmB64(each.groupId) == group)
        .firstOrNull;
    if (known != null && mine != null) await _catchUp(device, known, mine);
  }

  // ---- The home (#451) ----

  /// The connection to the home came or went: following starts again, and
  /// what waits for it goes.
  void _connectionMoved() {
    if (_closed || !(_homeConnected?.value ?? false)) {
      _followed = false;
      return;
    }
    // Another Claktory, maybe (the app switched): what the last one said
    // of itself never stands for this one (Zucchero's row 567). Asked at
    // once, before the rest of a sync.
    homeState = null;
    _notify();
    if (setup != DmSetup.ready) return;
    _routesSent = '';
    unawaited(_followHome());
    if (_device case final device? when _drained) {
      unawaited(_sendRoutes(device));
    }
    unawaited(sync());
  }

  /// Whether the home is followed on this connection.
  var _followed = false;

  /// Follows this device's groups at the home from where the store got to:
  /// a group whose position is still the worker's starts at its join.
  Future<void> _followHome() async {
    final link = _ours;
    final device = _device;
    if (link == null || device == null || !_drained) return;
    if (!link.connected.value || leafId.isEmpty) return;
    homeFeed.reset();
    _followed = true;
    try {
      await link.follow(
        DmFollow(
          leafId: leafId,
          cursors: [
            for (final group in await device.groups())
              if (group.seq > dmHomeSeqBase)
                DmCursor(groupId: group.groupId, seq: dmSeq(group.seq)),
          ],
        ),
      );
    } catch (error) {
      // Not the home, or not connected: followed again when it is.
      _followed = false;
      problem = '$error';
    }
  }

  /// What the home sent: its entries are taken in, group by group, in
  /// order; deliveries say how the person's messages are getting on.
  void _heard(DmFeed feed) {
    if (_closed || !_drained || !_homeIsOurs) return;
    final (:moved, :delivered) = homeFeed.take(feed);
    var again = false;
    for (final group in moved) {
      if (!groups.containsKey(group) || homeFeed.behind(group)) {
        // A group not listed yet (a Welcome's), or one the home let go
        // of past this device: a whole sync.
        again = true;
        continue;
      }
      _homeLater(() async {
        final device = _device;
        if (device == null || _closed) return;
        try {
          await _catchUpOne(device, group);
        } catch (error) {
          problem = '$error';
        }
      });
      _moved(group);
    }
    if (delivered) {
      for (final delivery in feed.deliveries) {
        if (delivery.state == DmDeliveryState.DM_DELIVERY_STATE_BOUNCED) {
          _homeLater(() => _bounced(delivery));
        }
      }
      _notify();
    }
    if (again) unawaited(sync());
  }

  /// Taking in what the home sent, one group after another.
  final _homeWork = <Future<void> Function()>[];
  var _homeBusy = false;

  void _homeLater(Future<void> Function() work) {
    _homeWork.add(work);
    if (!_homeBusy) unawaited(_doHomeWork());
  }

  Future<void> _doHomeWork() async {
    _homeBusy = true;
    try {
      while (_homeWork.isNotEmpty && !_closed) {
        await _homeWork.removeAt(0)();
      }
    } finally {
      _homeBusy = false;
    }
  }

  void _homeSaid(DmHome state) {
    // Another account's Claktory says nothing that counts here.
    if (_closed || !_homeIsOurs) return;
    homeState = state;
    _followTurnedAway();
    _notify();
  }

  /// Friends whose home turns the person's away ([dmTurnedAway]), when
  /// their keys may be read again, and how many times in a row they were.
  final _turnedAwayAfter = <String, int>{};
  final _turnedAwayTries = <String, int>{};
  final _turnedAwayTimers = <String, Timer>{};

  /// A friend's home turns the person's away (Mista, row 567): their home
  /// moved and this device missed it, maybe, or theirs doesn't route the
  /// person's yet. Their keys and list are read again and the home given
  /// its routes again, longer after each try in a row; never past the
  /// home's word, and only for a friend pinned here.
  void _followTurnedAway() {
    final state = homeState;
    if (_closed || state == null || !homeIsHere) return;
    final turned = {
      for (final peer in state.peers)
        if (peer.refused == dmTurnedAway) peer.accountId,
    };
    for (final account in [..._turnedAwayTries.keys]) {
      if (turned.contains(account)) continue;
      _turnedAwayTries.remove(account);
      _turnedAwayAfter.remove(account);
      _turnedAwayTimers.remove(account)?.cancel();
    }
    for (final account in turned) {
      final now = _nowMs();
      final after = _turnedAwayAfter[account] ?? 0;
      if (now < after) continue;
      final tries = (_turnedAwayTries[account] ?? 0) + 1;
      _turnedAwayTries[account] = tries;
      final wait = retryAfter * min(1 << (tries - 1), 64);
      _turnedAwayAfter[account] = now + wait.inMilliseconds;
      // Still turned away then: tried again, though the home says nothing
      // new meanwhile.
      _turnedAwayTimers.remove(account)?.cancel();
      _turnedAwayTimers[account] = Timer(wait, _followTurnedAway);
      unawaited(_readFriendAgain(account));
    }
  }

  Future<void> _readFriendAgain(String account) async {
    final device = _device;
    if (device == null || device.pin(account: account) == null) return;
    try {
      await refreshKeys(account);
    } catch (error) {
      problem = '$error';
    }
    if (!_drained || _closed) return;
    await _sendRoutes(device, again: true);
  }

  /// Where each group was taken in to, waiting to be acked to the home.
  final _toAck = <String, int>{};
  Timer? _ackTimer;

  /// Tells the home, a moment later and once for several, how far this
  /// device kept [group]: it may let go of what's before.
  void _ackLater(String group, int seq) {
    if (seq <= dmHomeSeqBase) return;
    _toAck[group] = seq;
    _ackTimer ??= Timer(const Duration(seconds: 1), () {
      _ackTimer = null;
      final link = _ours;
      if (link == null || _toAck.isEmpty || _closed) return;
      final through = [
        for (final MapEntry(:key, :value) in _toAck.entries)
          DmCursor(groupId: dmUnb64(key), seq: dmSeq(value)),
      ];
      _toAck.clear();
      unawaited(
        link
            .ack(DmAck(leafId: leafId, through: through))
            .catchError((Object error) => problem = '$error'),
      );
    });
  }

  /// Hands [envelope], a commit claktory.com just took, to the home (§0):
  /// the person's other devices and the friend's home learn of it now.
  /// Its seq at the home; null when the home didn't take it (it reads the
  /// server's commits itself).
  Future<int?> _commitToHome(String group, String envelope) async {
    final link = _ours;
    if (link == null) return null;
    try {
      final receipt = await link.send(
        DmSend(
          groupId: dmUnb64(group),
          envelope: dmBytesOf(envelope),
          leafId: leafId,
        ),
      );
      return switch (receipt.result) {
        DmSendResult.DM_SEND_RESULT_STORED ||
        DmSendResult.DM_SEND_RESULT_DUPLICATE => receipt.seq.toInt(),
        _ => null,
      };
    } catch (error) {
      problem = '$error';
      return null;
    }
  }

  /// Sends [commit] to claktory.com, which orders it, then to the home;
  /// once followed there, waits a moment for it to come back so it's
  /// taken in in its place. Its seq: the home's, else the worker's.
  Future<({int seq, int epoch})> _commit(String group, DmCommit commit) async {
    final taken = await routes.commit(group, commit);
    final seq = await _commitToHome(group, commit.commit);
    if (seq == null) return taken;
    if (_drained) await homeFeed.arrived(group, seq, _homeWait);
    return (seq: seq, epoch: taken.epoch);
  }

  /// The last seq of [group] where this device reads it: at the home once
  /// it went over, at the worker before.
  int _lastSeq(DmServerGroup group) =>
      _drained ? homeFeed.lastSeq(group.groupId) : group.lastSeq;

  /// What was last sent as the routing table, to send it only when it
  /// changes (and again after the connection came back).
  var _routesSent = '';

  /// The person's friends, as their friends list said last: each is
  /// pinned here at friending, and their home routed with no
  /// conversation, so the first message either of them sends is
  /// taken there while no device of the other's has joined it yet.
  var _friends = <String>{};

  /// Whether [_friends] was read since this engine started: until then the
  /// routes leave no one out for not being a friend, and the home's
  /// friends' routes aren't sent before it is.
  var _friendsKnown = false;

  /// Reads the friends list into [_friends]; null when it couldn't.
  Future<Set<String>?> _readFriends(DmDevice device) async {
    try {
      final friends = {...await routes.friends()}
        ..remove('')
        ..remove(accountId);
      _friendsKnown = true;
      return _friends = friends;
    } catch (error) {
      problem = '$error';
      return null;
    }
  }

  /// The friends list again, after it changed: each friend new since is
  /// pinned now, as a first message would pin them (a pin already here
  /// is never replaced: a changed key is marked, as always). Only the
  /// person's own friends, whom they chose: a request they accepted or
  /// one of theirs accepted.
  Future<void> _meetFriends(DmDevice device) async {
    final before = _friends;
    final now = await _readFriends(device);
    if (now == null) return;
    for (final account in now.difference(before)) {
      if (_closed) return;
      try {
        await refreshKeys(account);
      } catch (error) {
        problem = '$error';
      }
    }
  }

  /// Gives the home its routing table, from what this device verified
  /// (Fugo (a)–(c)): each friend in a conversation as its MLS group's
  /// checked members say, their home from their signed list as this
  /// device took it against its pin, with the conversations it may carry.
  /// Answered with the home's state.
  /// [again]: sent even when it didn't change.
  Future<void> _sendRoutes(DmDevice device, {bool again = false}) async {
    final link = _ours;
    if (link == null || !link.connected.value) return;
    // A first table without the friends would take their homes off it,
    // and with them what they left pending there.
    if (!_friendsKnown) await _readFriends(device);
    var unfriend = _friendsKnown;
    var reread = false;
    final byHome =
        <String, ({String account, List<int> home, Set<String> groups})>{};
    for (final group in await _groupsHere(device)) {
      if (group.kind != 'conversation') continue;
      // Who's in it as this device's own MLS state checked it (Fugo):
      // never the service's list of its accounts, which could name a
      // stranger to send the conversation's copies to.
      final accounts = {
        for (final member in await device.members(groupId: group.id))
          if (member.kind == DmLeafKind.device && member.account != accountId)
            member.account,
        // The friend it waits for (row 676), as this device's own record
        // of the conversation names them: their first join may come by
        // their home alone, which this home would refuse without a route
        // (Zucchero's row 549).
        ?joining[group.groupId],
      };
      for (final account in accounts) {
        // Unfriended: their home goes off the table, conversation and all.
        // One not on the list as last read may be a friend since: read
        // again first, and left on when it can't be read.
        if (unfriend && !_friends.contains(account) && !reread) {
          reread = true;
          unfriend = await _readFriends(device) != null;
        }
        if (unfriend && !_friends.contains(account)) continue;
        final home = device.deviceList(account: account)?.home;
        if (home == null || home.isEmpty) continue;
        final key = '$account/${dmB64(home)}';
        (byHome[key] ??= (
          account: account,
          home: home,
          groups: {},
        )).groups.add(group.groupId);
      }
    }
    // Each friend's home, with no conversation: only under a pin taken
    // here, never while their key changed unseen.
    for (final account in _friends) {
      final pin = device.pin(account: account);
      if (pin == null ||
          pin.changedUnseen ||
          pin.mismatched ||
          keyRefused.contains(account)) {
        continue;
      }
      final home = device.deviceList(account: account)?.home;
      if (home == null || home.isEmpty) continue;
      byHome.putIfAbsent(
        '$account/${dmB64(home)}',
        () => (account: account, home: home, groups: {}),
      );
    }
    final keys = byHome.keys.toList()..sort();
    final said = [
      for (final key in keys)
        '$key:${(byHome[key]!.groups.toList()..sort()).join(',')}',
    ].join(';');
    if (!again && said == _routesSent && homeState != null) return;
    try {
      final state = await link.routes(
        pb.DmRoutes(
          routes: [
            for (final key in keys)
              DmRoute(
                accountId: byHome[key]!.account,
                homeEndpoint: byHome[key]!.home,
                groupIds: [
                  for (final group in byHome[key]!.groups) dmUnb64(group),
                ],
              ),
          ],
        ),
      );
      // Another Claktory shown meanwhile (the rail), or this one no longer
      // confirmed: what the last said of itself never stands for it
      // (Zucchero's row 572).
      if (!identical(link, _ours)) return;
      _routesSent = said;
      homeState = state;
      _followTurnedAway();
      _notify();
    } catch (error) {
      problem = '$error';
    }
  }

  /// The person's home as their list names it: its endpoint id; null while
  /// it names none.
  List<int>? get homeEndpoint => _device?.deviceList(account: accountId)?.home;

  /// Whether the Claktory the app shows is the person's home: it says so,
  /// or the person's list names it. A Claktory places itself as the home
  /// from the list a moment after it changes, and its answer to the routes
  /// may come before that (Zucchero's row 548): the list is the person's
  /// choice, so it isn't shown as another Claktory meanwhile.
  bool get homeIsHere {
    final state = homeState;
    if (state == null) return false;
    if (state.isHome) return true;
    final named = homeEndpoint;
    return named != null &&
        named.isNotEmpty &&
        listEquals(named, state.endpointId);
  }

  /// A list that names no home names the Claktory the app shows (Mista,
  /// 32426): the person can change it in People.
  Future<void> _adoptHome(DmDevice device) async {
    if (!_homeIsOurs) return;
    final here = homeState?.endpointId;
    if (here == null || here.isEmpty) return;
    final list = device.deviceList(account: accountId);
    if (list == null || (list.home?.isNotEmpty ?? false)) return;
    await _changeList(device, (leaves) => leaves, home: here);
  }

  /// Makes the Claktory the app shows the person's home: the place their
  /// messages are kept from now on.
  Future<void> makeHomeHere() async {
    if (!_homeIsOurs) return;
    final device = _device;
    final here = homeState?.endpointId;
    if (device == null || here == null || here.isEmpty) return;
    await _changeList(device, (leaves) => leaves, home: here);
    _routesSent = '';
    await sync();
  }

  /// How the person's message [clientId] is getting on past their home;
  /// null before the home said.
  DmDelivery? delivery(String clientId) => homeFeed.deliveries[clientId];

  /// The person's messages a friend's home couldn't read any more, whose
  /// conversation gained someone since: sent again only when the person
  /// says (Fugo), by client message id.
  final askResend = <String>{};

  /// A message of the person's that came back as too old for the friend's
  /// home: sent again at once when the conversation only lost members
  /// since; otherwise the person is asked ([askResend]). Only by the device
  /// that sent it, once: the new one names what it resends.
  Future<void> _bounced(DmDelivery delivery) async {
    final device = _device;
    if (device == null || _closed) return;
    final group = dmB64(delivery.groupId);
    final id = delivery.clientMessageId;
    final found = await _sentAndAfter(device, group, id);
    if (found == null || found.resent) return;
    if (found.message.fromLeaf != leafId) return;
    if (found.onlyRemoved) {
      await _resend(group, found.message);
    } else if (askResend.add(id)) {
      _moved(group);
      _notify();
    }
  }

  /// The person says yes to sending [clientId] again in [group].
  Future<void> resend(String group, String clientId) async {
    final device = _device;
    if (device == null) return;
    final found = await _sentAndAfter(device, group, clientId);
    askResend.remove(clientId);
    if (found == null || found.resent) return _notify();
    await _resend(group, found.message);
  }

  Future<void> _resend(String group, DmMessage message) async {
    final extra = Map<String, Object?>.of(_extra(message.content))
      ..['resends'] = message.content.clientMessageId;
    await say(
      group,
      DmContent(
        kind: message.content.kind,
        clientMessageId: _newId(),
        text: message.content.text,
        byAgent: message.content.byAgent,
        extraJson: jsonEncode(extra),
      ),
    );
  }

  /// The person's message [clientId] in [group] as this device kept it,
  /// whether it was sent again since, and whether the conversation only
  /// lost members after it; null when it isn't here.
  Future<({DmMessage message, bool resent, bool onlyRemoved})?> _sentAndAfter(
    DmDevice device,
    String group,
    String clientId,
  ) async {
    final after = <DmMessage>[];
    int? before;
    for (var pages = 0; pages < 20; pages++) {
      final page = await device.history(
        groupId: dmUnb64(group),
        beforeSeq: before,
        limit: 100,
      );
      if (page.isEmpty) return null;
      for (final message in page.reversed) {
        if (message.mine && message.content.clientMessageId == clientId) {
          return (
            message: message,
            resent: after.any(
              (each) =>
                  each.mine && _extra(each.content)['resends'] == clientId,
            ),
            onlyRemoved: after.every(
              (each) =>
                  each.members == null ||
                  (each.members!.added.isEmpty &&
                      each.members!.keyChanged.isEmpty),
            ),
          );
        }
        after.add(message);
      }
      before = page.first.seq;
    }
    return null;
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
      // An agent's message too: it's kept only once approved (#451).
      if (dmB64(unsent.groupId) == group) unsent,
  ];

  /// [envelope] from [claktoryId]'s channel, opened once (kept opened in
  /// the store): its content when the channel's certified leaf of that
  /// Claktory, of the person's own account, said it and it's a [kind];
  /// null for anything else, or what this device can't open. With no
  /// [envelope] (#451), the one with [digest] its Claktory sent into the
  /// channel through the home, as the feed brought it; null until it has.
  Future<DmContent?> _fromClaktory(
    String claktoryId,
    String envelope,
    String kind,
    int arrivedMs, {
    String digest = '',
  }) async {
    final device = _device;
    if (device == null || (envelope.isEmpty && digest.isEmpty)) return null;
    final channel = groups.values
        .where(
          (each) => each.kind == 'channel' && each.claktoryId == claktoryId,
        )
        .firstOrNull;
    if (channel == null) return null;
    try {
      final DmMessage? message;
      if (envelope.isEmpty) {
        message = await device.sideMessage(
          groupId: channel.id,
          digest: _hexBytes(digest),
        );
        if (message == null) return null;
      } else {
        // At the channel's epoch, or near it.
        await _catchUpOne(device, channel.groupId);
        message = await device.openSide(
          groupId: channel.id,
          envelope: envelope,
          arrivedMs: arrivedMs,
        );
      }
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

  /// [hex] (lowercase, as the service names a digest) as bytes; empty
  /// for anything else.
  static Uint8List _hexBytes(String hex) {
    if (hex.length.isOdd || !RegExp(r'^[0-9a-f]*$').hasMatch(hex)) {
      return Uint8List(0);
    }
    return Uint8List.fromList([
      for (var i = 0; i < hex.length; i += 2)
        int.parse(hex.substring(i, i + 2), radix: 16),
    ]);
  }

  /// Whether the proposal or draft with [digest] from [claktoryId] came
  /// through the home yet (#451): until it has, it shows as on its way.
  Future<bool> sideArrived(String claktoryId, String digest) async {
    final device = _device;
    final channel = groups.values
        .where(
          (each) => each.kind == 'channel' && each.claktoryId == claktoryId,
        )
        .firstOrNull;
    if (device == null || channel == null || digest.isEmpty) return false;
    try {
      return await device.sideMessage(
            groupId: channel.id,
            digest: _hexBytes(digest),
          ) !=
          null;
    } catch (_) {
      return false;
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
      digest: proposal.proposalDigest,
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
      digest: draft.draftDigest,
    );
    if (content == null) return null;
    if (_extra(content)['conversation_id'] != conversation) return null;
    return draft.opened(
      claktoryId: draft.claktoryId,
      agentName: _oneLine(content.byAgent?.name ?? '', 64),
      text: content.text ?? '',
    );
  }

  /// [proposal], opened, sent in [group] as its agent's message once the
  /// person approved it (#451, Mista 32507): sealed here, under its
  /// client message id, and kept until the home took it; sealed again
  /// under the same id when the group moved on, never approved twice.
  Future<void> sendApproved(String group, DmProposal proposal) {
    // The id the service lists for it, which the home knows it by (Mista
    // 32744).
    final id = proposal.clientMessageId.isEmpty
        ? proposal.id
        : proposal.clientMessageId;
    _approvedHere.add(id);
    return say(
      group,
      DmContent(
        kind: 'text',
        clientMessageId: id,
        text: proposal.text,
        byAgent: DmByAgent(
          claktoryId: proposal.claktoryId,
          agentId: proposal.agentId,
          incarnation: proposal.incarnation,
          name: proposal.agentName,
        ),
        extraJson: '',
      ),
    );
  }

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
    _homeSubscriptions();
    _ackTimer?.cancel();
    for (final timer in _turnedAwayTimers.values) {
      timer.cancel();
    }
    _pause?.cancel();
    if (_paused case final paused? when !paused.isCompleted) paused.complete();
    unawaited(_events.close());
    unawaited(_groupChanges.close());
    _device?.dispose();
    super.dispose();
  }
}

/// What the person's Claktory account says of the Claktory the app shows
/// (Fugo 39894).
enum DmOwnership {
  /// Being checked.
  checking,

  /// On their account: messages go through it.
  ours,

  /// Not on it: signed in to another account, say.
  notOurs,

  /// The account's list couldn't be read: tried again by itself.
  unchecked,

  /// Can't be checked: on another machine, paired with a code from before
  /// the private tunnel, so nothing says which Claktory it is (Fugo 40182).
  /// A new pairing code makes it checkable.
  unconfirmable,
}

/// Which Claktory the app shows, as the messages engine checks it against
/// the person's account (Fugo 40182).
@immutable
class DmShown {
  /// This Mac's own, reached on its port with its token: known by the id
  /// its leaf offer gives, on the account's list.
  const DmShown.thisMac() : code = '', endpoint = '', local = true;

  /// One on another machine: its pairing [code], and the [endpoint] the
  /// tunnel dials and authenticates it by ('' for a direct code from
  /// before the tunnel, never confirmed).
  const DmShown.remote({required this.code, required this.endpoint})
    : local = false;

  /// One the window can't say which: never confirmed.
  const DmShown.unknown() : code = '', endpoint = '', local = false;

  final String code;
  final String endpoint;
  final bool local;

  /// Whether the account can confirm it's the person's: this Mac's own,
  /// or one reached through the tunnel.
  bool get confirmable => local || endpoint.isNotEmpty;

  @override
  bool operator ==(Object other) =>
      other is DmShown &&
      other.code == code &&
      other.endpoint == endpoint &&
      other.local == local;

  @override
  int get hashCode => Object.hash(code, endpoint, local);
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

  /// Kept here until the person's home takes it ([DmSending.problem]
  /// says why: `unreachable` or `notHome`, "Waiting for your Claktory";
  /// `homeFull` or `rateLimited`): it goes by itself (#451).
  waiting,
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
