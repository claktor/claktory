import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:crypto/crypto.dart' show sha256;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart' show ValueNotifier;

import 'package:factory_app/device_account.dart';
import 'package:factory_app/people/dm_keys.dart';
import 'package:factory_app/people/dm_home.dart';
import 'package:factory_app/people/e2ee.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart'
    show DmLeafOffer;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' as pb;
import 'package:factory_app/tunnel/api/dm.dart';
import 'package:factory_app/tunnel/api/dm_transfer.dart';
import 'package:factory_app/user_apps/session.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart'
    show AnyhowException;

// Fakes for end-to-end messages in widget tests, where the app's Rust
// isn't loaded: [FakeWorker] keeps the worker's E2 routes (README "End to
// end") over plain JSON "envelopes" it can read the way the real one reads
// MLS framing (kind, group, epoch), and [FakeDmDevice] stands in for the
// Rust DmDevice: groups at epochs, members, history, the retry cache and
// the self group's views. Nothing here is cryptography; it checks the
// app's side of the contract.

/// A fake envelope: what it says, readable by both fakes.
String fakeEnvelope(Map<String, Object?> body) =>
    jsonEncode({'format': 2, 'bytes': dmB64(utf8.encode(jsonEncode(body)))});

Map<String, Object?> readFake(String envelope) {
  final json = jsonDecode(envelope) as Map<String, Object?>;
  return jsonDecode(utf8.decode(dmUnb64(json['bytes']! as String)))
      as Map<String, Object?>;
}

class _Group {
  _Group(
    this.kind, {
    this.conversationId = '',
    this.generation = 0,
    this.claktoryId = '',
  });

  final String kind;
  final String conversationId;
  final int generation;
  final String claktoryId;
  List<String> accounts = const [];
  int epoch = 0;
  final mailbox = <Map<String, Object?>>[];
  String info = '';
  final sent = <String, String>{};
}

/// The worker's end-to-end routes, for any number of [as] callers.
class FakeWorker {
  final keys = <String, Map<String, Object?>>{};

  /// When the worker takes what comes next, if set: else its seq.
  int? acceptAt;
  final packages = <String, List<String>>{};
  final lastResort = <String, String>{};

  /// When each leaf's last resort's lifetime ends (seconds), as GET
  /// key-packages says; none (0) unless a test sets it.
  final lastResortEnds = <String, int>{};
  final _groups = <String, _Group>{};
  final welcomes = <String, List<Map<String, Object?>>>{};
  final handoffs = <String, List<Map<String, Object?>>>{};

  /// The account's live grants, as GET /v1/apps/grants lists them.
  final grants = <Map<String, Object?>>[];
  final log = <String>[];

  /// The devices signed out of each account by another of its devices
  /// (POST /v1/devices/remove): the service answers them 401 since.
  final signedOut = <String, Set<String>>{};

  /// The listed devices of each account that signed themselves out of it
  /// (row 671): `GET keys/<own account>` marks them.
  final accountSignOuts = <String, Set<String>>{};

  /// Each account's security keys before the one it has now: the worker
  /// takes none of them again (POST me/keys/rotate).
  final retired = <String, Set<String>>{};

  /// Makes the next message post to a group answer 409 epoch, once.
  var refuseNextAsStale = false;

  /// Answers [route] ("POST conversations/v1/messages") with what it
  /// returns instead, when that's an exception: the service unreachable,
  /// or refusing.
  Object? Function(String route)? refuse;

  /// Devices ("account/leaf") whose sign-in is too old for a new key.
  final staleSignIn = <String>{};
  var _welcomeId = 0;

  int epochOf(String group) => _groups[group]!.epoch;

  /// Moves [group]'s epoch on, as a commit not in its feed yet would.
  void advance(String group) => _groups[group]!.epoch++;

  /// Lists [accounts] as [group]'s, as a service that lies would.
  void listAccounts(String group, List<String> accounts) =>
      _groups[group]!.accounts = accounts;
  List<Map<String, Object?>> mailboxOf(String group) => _groups[group]!.mailbox;

  /// Adds [envelope] to [group]'s mailbox as another member would; a
  /// commit moves the group's epoch unless not [advance].
  int deliver(String group, String envelope, {bool advance = true}) {
    final body = readFake(envelope);
    final known = _groups[group]!;
    final seq = known.mailbox.length + 1;
    known.mailbox.add({
      'seq': seq,
      'kind': switch (body['t']) {
        'commit' => 'commit',
        'proposal' => 'proposal',
        _ => 'application',
      },
      'envelope': jsonDecode(envelope),
      'accepted_ms': acceptAt ?? seq,
      'epoch': body['epoch'],
    });
    if (body['t'] == 'commit' && advance) known.epoch++;
    return seq;
  }

  /// The outbox's changes, as `/sync` follows them: a cursor is how many
  /// came before it.
  final syncEvents = <Map<String, Object?>>[];
  Completer<void>? _syncMoved;

  /// Adds [event] ({'kind': 'keys', 'account_id': 'ana'}, say) to the
  /// outbox: a held `/sync` answers with it.
  void post(Map<String, Object?> event) {
    syncEvents.add(event);
    _syncMoved?.complete();
    _syncMoved = null;
  }

  SignedCall as(String account, String leaf) => (method, path, [body]) async {
    final uri = Uri.parse(path);
    final route = '$method ${uri.path.replaceFirst('/v1/apps/dms/', '')}';
    log.add(route);
    if (refuse?.call(route) case final Exception error) throw error;
    if (signedOut[account]?.contains(leaf) ?? false) {
      _refuse(401, 'unauthorized');
    }
    if (route == 'GET sync') {
      final since = uri.queryParameters['since'];
      if (since == null) return {'cursor': '${syncEvents.length}'};
      final from = int.parse(since);
      if (from >= syncEvents.length &&
          uri.queryParameters.containsKey('wait')) {
        await (_syncMoved ??= Completer()).future;
      }
      return {
        'events': syncEvents.sublist(from),
        'next_cursor': '${syncEvents.length}',
        'has_more': false,
      };
    }
    return _answer(account, leaf, route, uri, body ?? const {});
  };

  Never _refuse(int status, String code, [Map<String, Object?>? details]) =>
      throw AccountServiceError(status, code, code, details ?? const {});

  Map<String, Object?> _answer(
    String account,
    String leaf,
    String route,
    Uri uri,
    Map<String, Object?> body,
  ) {
    final parts = route.split(' ')[1].split('/');
    switch ((route.split(' ')[0], parts)) {
      case ('GET', ['keys', final who]):
        final now = keys[who] ?? _refuse(404, 'not_found');
        return {
          ...now,
          // Oldest first (row 654).
          if (retired[who] case final old? when old.isNotEmpty)
            'replaced_identity_keys': [...old],
          // Only to one of the account's own devices (row 671).
          if (who == account && (accountSignOuts[who]?.isNotEmpty ?? false))
            'signed_out': [...accountSignOuts[who]!],
        };
      case ('PUT', ['me', 'keys']):
        final now = keys[account];
        if ((now?['version'] ?? 0) != body['previous_version']) {
          _refuse(409, 'stale', {'keys': now});
        }
        final identity = body['identity_key'] ?? now?['identity_key'];
        if (now != null &&
            body['identity_key'] != null &&
            body['identity_key'] != now['identity_key']) {
          _refuse(409, 'identity_changed');
        }
        // A leaf off the list gets no more key packages: its own go
        // (the worker's keep_listed).
        final listed = {
          for (final leaf
              in (jsonDecode(
                    utf8.decode(dmUnb64(body['device_list']! as String)),
                  ) as Map)['leaves']
                  as List)
            '$account/${(leaf as Map)['leaf']}',
        };
        for (final kept in [packages, lastResort]) {
          kept.removeWhere(
            (key, _) => key.startsWith('$account/') && !listed.contains(key),
          );
        }
        return keys[account] = {
          'account_id': account,
          'identity_key': identity,
          'device_list': body['device_list'],
          'version': body['version'],
        };
      case ('POST', ['me', 'keys', 'rotate']):
        // A new key from a device signed in long ago (row 630, Fugo).
        if (staleSignIn.contains('$account/$leaf')) {
          _refuse(403, 'sign_in_again');
        }
        final now = keys[account];
        final identity = body['identity_key']! as String;
        if (now == null) _refuse(409, 'not_set_up');
        if (identity == now['identity_key'] ||
            (retired[account]?.contains(identity) ?? false)) {
          _refuse(400, 'invalid_request');
        }
        (retired[account] ??= {}).add(now['identity_key']! as String);
        // The old key's packages go with it.
        packages.removeWhere((key, _) => key.startsWith('$account/'));
        lastResort.removeWhere((key, _) => key.startsWith('$account/'));
        keys[account] = {
          'account_id': account,
          'identity_key': identity,
          'device_list': body['device_list'],
          'version': body['version'],
        };
        return {};
      case ('POST', ['key-packages']):
        if (keys[account] == null) _refuse(409, 'not_set_up');
        final mine = packages.putIfAbsent('$account/$leaf', () => []);
        for (final package in body['key_packages']! as List) {
          mine.add(jsonEncode(package));
        }
        if (body['last_resort'] case final last?) {
          lastResort['$account/$leaf'] = jsonEncode(last);
          lastResortEnds.remove('$account/$leaf');
        }
        return {'remaining': mine.length, 'last_resort': true};
      case ('GET', ['key-packages']):
        return {
          'remaining': packages['$account/$leaf']?.length ?? 0,
          'last_resort': lastResort.containsKey('$account/$leaf'),
          'last_resort_ends': lastResortEnds['$account/$leaf'] ?? 0,
        };
      case ('POST', ['key-packages', 'claim']):
        final whose = body['account_id']! as String;
        final only = body['leaf_ids'] as List?;
        return {
          'key_packages': [
            for (final entry in packages.entries)
              if (entry.key.startsWith('$whose/') &&
                  (only == null ||
                      only.contains(entry.key.substring(whose.length + 1))) &&
                  (entry.value.isNotEmpty || lastResort.containsKey(entry.key)))
                {
                  'leaf_id': entry.key.substring(whose.length + 1),
                  'key_package': jsonDecode(
                    entry.value.isNotEmpty
                        ? entry.value.removeAt(0)
                        : lastResort[entry.key]!,
                  ),
                },
          ],
        };
      case ('POST', ['groups']):
        final kind = body['kind']! as String;
        final subject = _groups.values.where(
          (each) =>
              each.kind == kind &&
              (kind != 'conversation' ||
                  (each.conversationId == body['conversation_id'] &&
                      each.generation == body['contact_generation'])) &&
              (kind != 'channel' || each.claktoryId == body['claktory_id']) &&
              (kind == 'conversation' || each.accounts.contains(account)),
        );
        if (subject.isNotEmpty) _refuse(409, 'exists');
        final info = readFake(jsonEncode(body['group_info']));
        final id = info['g']! as String;
        final group = _groups[id] = _Group(
          kind,
          conversationId: body['conversation_id'] as String? ?? '',
          generation: body['contact_generation'] as int? ?? 0,
          claktoryId: body['claktory_id'] as String? ?? '',
        );
        group
          ..accounts = kind == 'conversation'
              ? [for (final a in info['accounts']! as List) a as String]
              : [account]
          ..info = jsonEncode(body['group_info']);
        return {'group_id': id, 'epoch': 0, 'last_seq': 0};
      case ('GET', ['groups']):
        return {
          'groups': [
            for (final MapEntry(:key, :value) in _groups.entries)
              if (value.accounts.contains(account))
                {
                  'group_id': key,
                  'kind': value.kind,
                  if (value.kind == 'conversation') ...{
                    'conversation_id': value.conversationId,
                    'contact_generation': value.generation,
                    'accounts': value.accounts,
                  },
                  if (value.kind == 'channel') 'claktory_id': value.claktoryId,
                  'epoch': value.epoch,
                  'last_seq': value.mailbox.length,
                },
          ],
        };
      case ('GET', ['groups', final id, 'messages']):
        final after = int.parse(uri.queryParameters['after'] ?? '0');
        return {
          'messages': [
            for (final item in _groups[id]!.mailbox)
              if ((item['seq']! as int) > after) item,
          ],
          'has_more': false,
        };
      case ('GET', ['groups', final id, 'group-info']):
        final group = _groups[id]!;
        return {'epoch': group.epoch, 'group_info': jsonDecode(group.info)};
      case ('POST', ['groups', final id, 'commits']):
        final group = _groups[id]!;
        final commit = readFake(jsonEncode(body['commit']));
        if (commit['epoch'] != group.epoch) {
          _refuse(409, 'epoch', {'epoch': group.epoch});
        }
        final seq = deliver(id, jsonEncode(body['commit']));
        group.info = jsonEncode(body['group_info']);
        if (body['welcome'] case final welcome?) {
          for (final to in readFake(jsonEncode(welcome))['to']! as List) {
            welcomes.putIfAbsent(to as String, () => []).add({
              'id': 'w${++_welcomeId}',
              'group_id': id,
              'kind': group.kind,
              if (group.kind == 'conversation') ...{
                'conversation_id': group.conversationId,
                'contact_generation': group.generation,
                'accounts': group.accounts,
              },
              'welcome': welcome,
              'seq': seq,
            });
          }
        }
        return {'seq': seq, 'epoch': group.epoch};
      case ('GET', ['welcomes']):
        return {'welcomes': welcomes[leaf] ?? const []};
      case ('DELETE', ['welcomes', final id]):
        welcomes[leaf]?.removeWhere((each) => each['id'] == id);
        return {};
      case ('POST', ['groups', final id, 'messages']):
        return _say(id, body);
      case ('POST', ['conversations', final conversation, 'messages']):
        final id = _groups.entries
            .firstWhere(
              (each) =>
                  each.value.conversationId == conversation &&
                  each.value.generation == body['contact_generation'],
            )
            .key;
        return _say(id, body);
      case ('POST', ['', 'v1', 'devices', 'remove']):
        final key = body['account_key']! as String;
        if (key == leaf) _refuse(400, 'invalid_request');
        return {'removed': (signedOut[account] ??= {}).add(key)};
      case ('GET', ['', 'v1', 'apps', 'grants']):
        return {'grants': grants};
      case ('POST', ['me', 'handoffs']):
        handoffs.putIfAbsent(body['to']! as String, () => []).add({
          'id': 'h${handoffs.length + 1}',
          'from': leaf,
          'sealed': body['sealed'],
        });
        return {'id': 'h${handoffs.length}'};
      case ('GET', ['me', 'handoffs']):
        return {'handoffs': handoffs[leaf] ?? const []};
      case ('DELETE', ['me', 'handoffs', final id]):
        handoffs[leaf]?.removeWhere((each) => each['id'] == id);
        return {};
    }
    _refuse(404, 'not_found');
  }

  Map<String, Object?> _say(String id, Map<String, Object?> body) {
    final group = _groups[id]!;
    final envelope = jsonEncode(body['envelope']);
    final clientId = body['client_message_id']! as String;
    if (group.sent[clientId] case final before?) {
      if (before != envelope) _refuse(409, 'conflict');
      return {'seq': 0, 'accepted_ms': 0};
    }
    if (refuseNextAsStale || readFake(envelope)['epoch'] != group.epoch) {
      refuseNextAsStale = false;
      _refuse(409, 'epoch', {'epoch': group.epoch});
    }
    group.sent[clientId] = envelope;
    final seq = deliver(id, envelope);
    return {'message_id': 'm$seq', 'seq': seq, 'accepted_ms': seq};
  }
}

/// The platform's key store, in memory.
class FakeDmKeys implements DmKeys {
  FakeDmKeys({this.isAvailable = true});

  final bool isAvailable;
  final kept = <DmKeyName, Uint8List>{};
  final unreadable = <DmKeyName>{};

  @override
  Future<bool> available() async => isAvailable;

  @override
  Future<void> put(DmKeyName name, Uint8List bytes) async =>
      kept[name] = Uint8List.fromList(bytes);

  @override
  Future<Uint8List?> get(DmKeyName name) async {
    if (unreadable.contains(name)) throw DmKeyUnreadable(name, 'damaged');
    // As the platform channel hands them over: a view nothing may change
    // (Abbacchio: zeroing one threw after a restart).
    return switch (kept[name]) {
      final bytes? => Uint8List.fromList(bytes).asUnmodifiableView(),
      null => null,
    };
  }

  @override
  Future<void> delete(DmKeyName name) async {
    // A damaged item goes too, as the keychain deletes it.
    kept.remove(name);
    unreadable.remove(name);
  }

  @override
  Future<String> folder() async => '/fake';

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _Here {
  _Here(this.kind, this.epoch, this.members);

  final DmGroupKind kind;
  int epoch;
  List<String> members;
  int seq = 0;
  String? pending;
  int pendingEpoch = 0;
  List<String> pendingMembers = const [];

  /// This device's leaf was committed out of the group: it reads nothing
  /// said there after.
  bool removed = false;

  /// This device's leaf is under the person's current key here.
  bool moved = true;
  bool pendingMove = false;

  /// Once this device reads the home (#451): where it is in the worker's
  /// feed, null for where it was before, and the proposals taken there
  /// (their 'p'), which a commit naming them in 'takes' needs.
  int? proposalSeq;
  final proposals = <String>{};
}

/// The app's Rust DmDevice, in memory.
class FakeDmDevice implements DmDevice {
  FakeDmDevice(this.leafKey);

  final List<int> leafKey;
  var setUp = false;
  var account = '';
  var leaf = '';
  var name = '';
  Uint8List? identity;
  final pins = <String, DmPin>{};
  final _groups = <String, _Here>{};
  final _history = <String, List<DmMessage>>{};
  final _outgoing =
      <String, ({String group, String envelope, DmContent content, int at})>{};
  final _views = <String, DmView>{};
  var _packages = 0;
  var _offers = 0;

  /// Every process call's outcome, for tests.
  final processed = <DmProcessed>[];

  /// The store deleted ([E2ee] forgetting it): as a new one opens empty.
  void forget() {
    setUp = false;
    account = '';
    leaf = '';
    name = '';
    identity = null;
    pins.clear();
    lists.clear();
    _groups.clear();
    _history.clear();
    _outgoing.clear();
    _views.clear();
    removedLeaves.clear();
    usedCodes.clear();
    removalRow = null;
  }

  String _member() => '$account/$leaf';

  @override
  DmStatus status() => DmStatus(
    setUp: setUp,
    accountId: account,
    leafId: leaf,
    name: name,
    leafPublic: Uint8List.fromList(leafKey),
    identityPublic: identity,
  );

  @override
  Future<DmSetUp> setUpFirst({
    required List<int> identityKey,
    required String accountId,
    required String leafId,
    required String name,
    required BigInt nowMs,
  }) async {
    setUp = true;
    account = accountId;
    leaf = leafId;
    this.name = name;
    identity = Uint8List.fromList([...identityKey.reversed]);
    pins[accountId] = _pin(identity!);
    lists[accountId] = DmDeviceList(
      version: BigInt.one,
      createdMs: nowMs,
      leaves: [_listed(leafId, leafKey)],
    );
    return DmSetUp(
      identityPublic: identity!,
      certificate: Uint8List(0),
      deviceList: _listBytes(lists[accountId]!),
    );
  }

  /// The key [setUpNewKey] replaced, held as retired here.
  Uint8List? replacedKey;

  @override
  Future<DmSetUp> setUpNewKey({
    required List<int> identityKey,
    required List<int> oldIdentityPublic,
    required List<Uint8List> replacedIdentityKeys,
    required String accountId,
    required String leafId,
    required String name,
    required BigInt nowMs,
  }) async {
    if (listEqualsInts(_public(identityKey), oldIdentityPublic)) {
      throw AnyhowException("that's the key the account has");
    }
    if (replacedIdentityKeys.any(
      (key) => listEqualsInts(key, _public(identityKey)),
    )) {
      throw AnyhowException("that's the key the account has");
    }
    replacedKey = Uint8List.fromList(oldIdentityPublic);
    replacedBefore = [...replacedIdentityKeys];
    return setUpFirst(
      identityKey: identityKey,
      accountId: accountId,
      leafId: leafId,
      name: name,
      nowMs: nowMs,
    );
  }

  /// The keys a start-over here retired before the one it replaced.
  List<Uint8List> replacedBefore = const [];

  /// Keys retired here because a held commit proved them (row 654), by
  /// account, base64url.
  final retiredHere = <String, Set<String>>{};

  @override
  Future<bool> retireReplaced({
    required List<int> groupId,
    required String envelope,
    required String account,
    required List<int> served,
    required List<Uint8List> listed,
  }) async {
    final body = readFake(envelope);
    final key = body['id'];
    final whose = (body['from'] as String? ?? '').split('/').first;
    final pinned = pins[account]?.identityPublic;
    if (listed.any(
      (each) =>
          listEqualsInts(each, served) ||
          (pinned != null && listEqualsInts(each, pinned)),
    )) {
      throw AnyhowException(
        'refused: the service lists a current security key as replaced',
      );
    }
    if (key is! String ||
        whose != account ||
        key == dmB64(served) ||
        !listed.any((each) => dmB64(each) == key)) {
      return false;
    }
    return (retiredHere[account] ??= {}).add(key);
  }

  /// Each account's device list as this device last took it.
  final lists = <String, DmDeviceList>{};

  static DmListed _listed(String leaf, List<int> key) => DmListed(
    kind: claktoryOf(leaf) == null ? DmLeafKind.device : DmLeafKind.claktory,
    leafId: leaf,
    signatureKey: Uint8List.fromList(key),
  );

  static Uint8List _listBytes(DmDeviceList list) => Uint8List.fromList(
    utf8.encode(
      jsonEncode({
        'version': list.version.toInt(),
        'created': list.createdMs.toInt(),
        'leaves': [
          for (final leaf in list.leaves)
            {'leaf': leaf.leafId, 'key': dmB64(leaf.signatureKey)},
        ],
        if (list.home case final home?) 'home': dmB64(home),
      }),
    ),
  );

  @override
  DmDeviceList? deviceList({required String account}) => lists[account];

  /// The person's devices removed here or off a list this device took,
  /// and the codes handed off to, as the Rust keeps them (row 612).
  final removedLeaves = <String>{};
  final usedCodes = <String>{};

  /// [list] as [account]'s, keeping the person's leaves it drops as
  /// removed.
  DmDeviceList _takeList(String account, DmDeviceList list) {
    final before = lists[account];
    if (account == this.account && before != null) {
      final now = {for (final leaf in list.leaves) leaf.leafId};
      removedLeaves.addAll([
        for (final leaf in before.leaves)
          if (!now.contains(leaf.leafId)) leaf.leafId,
      ]);
    }
    return lists[account] = list;
  }

  @override
  Future<void> codeUsed({required String comparison}) async =>
      usedCodes.add(comparison);

  @override
  Future<Uint8List> signDeviceList({
    required BigInt version,
    required BigInt createdMs,
    required List<DmListed> leaves,
  }) async {
    // Signed only: pinned once the worker takes it ([acceptList]), as the
    // Rust (row 615).
    return _listBytes(
      DmDeviceList(version: version, createdMs: createdMs, leaves: leaves),
    );
  }

  @override
  Future<Uint8List> signDeviceListWithHome({
    required BigInt version,
    required BigInt createdMs,
    required List<DmListed> leaves,
    Uint8List? home,
  }) async => _listBytes(
    DmDeviceList(
      version: version,
      createdMs: createdMs,
      leaves: leaves,
      home: home,
    ),
  );

  /// Whether this device went over to its home (#451): a new one has
  /// nothing at the worker to read first.
  var drained = false;

  @override
  bool homeDrained() => drained;

  @override
  Future<void> setHomeDrained() async => drained = true;

  @override
  Future<DmHandoff> startHandoff({
    required String leafId,
    required String name,
    required String platform,
    required List<int> endpointId,
  }) async => FakeHandoff(
    jsonEncode({
      'leaf': leafId,
      'name': name,
      'platform': platform,
      'n': ++_offers,
    }),
  );

  @override
  Future<DmHandoffSealed> sealHandoff({required String qr}) async {
    final offer = jsonDecode(qr) as Map<String, Object?>;
    final leaf = offer['leaf']! as String;
    if (removedLeaves.contains(leaf) ||
        usedCodes.contains(fakeDigits(jsonEncode(offer)))) {
      throw AnyhowException(
        'This code was already used, or is for a device you removed.',
      );
    }
    return DmHandoffSealed(
      sealed: Uint8List.fromList(
        utf8.encode(
          jsonEncode({
            'account': account,
            'identity': dmB64(identity!),
            'for': leaf,
            // Sealed to that code's own keys: no other code opens it.
            'code': offer['n'],
          }),
        ),
      ),
      leafSignatureKey: Uint8List.fromList(utf8.encode(leaf)),
      leafId: leaf,
      name: offer['name']! as String,
      platform: offer['platform']! as String,
      endpointId: Uint8List(32),
      comparison: fakeDigits(jsonEncode(offer)),
    );
  }

  @override
  Future<DmHandoffDone> finishHandoffFor({
    required DmHandoff handoff,
    required List<int> sealed,
    required BigInt nowMs,
    required String accountId,
    required List<int> identityPublic,
  }) async {
    final offer = jsonDecode(handoff.qr()) as Map<String, Object?>;
    final body = jsonDecode(utf8.decode(sealed)) as Map<String, Object?>;
    if (body['for'] != offer['leaf'] || body['code'] != offer['n']) {
      throw StateError("doesn't open");
    }
    // As the crate's: only the identity the account publishes (Melone F3).
    if (body['account'] != accountId ||
        !listEqualsInts(dmUnb64(body['identity']! as String), identityPublic)) {
      throw StateError("isn't the account's published identity");
    }
    setUp = true;
    account = body['account']! as String;
    leaf = offer['leaf']! as String;
    name = offer['name']! as String;
    identity = dmUnb64(body['identity']! as String);
    pins[account] = _pin(identity!);
    return DmHandoffDone(
      accountId: account,
      identityKey: Uint8List.fromList([...identity!.reversed]),
      identityPublic: identity!,
      certificate: Uint8List(0),
    );
  }

  @override
  Future<List<DmMember>> members({required List<int> groupId}) async => [
    for (final member in _groups[dmB64(groupId)]?.members ?? const <String>[])
      // As the device's roster does: a leaf its account's list (as this
      // device took it) doesn't name fails the checks, so it isn't a
      // member, though it's still in the group ([leafIds]).
      if (_listedHere(member))
        DmMember(
          account: member.split('/').first,
          leafId: member.split('/').last,
          kind: claktoryOf(member.split('/').last) == null
              ? DmLeafKind.device
              : DmLeafKind.claktory,
          name: member.split('/').last,
          sinceMs: BigInt.from(added[member] ?? 0),
        ),
  ];

  /// Every leaf in the group with what it claims, checked or not (as the
  /// device's leaves_unchecked).
  @override
  Future<List<DmMember>> leaves({required List<int> groupId}) async => [
    for (final member in _groups[dmB64(groupId)]?.members ?? const <String>[])
      DmMember(
        account: member.split('/').first,
        leafId: member.split('/').last,
        kind: claktoryOf(member.split('/').last) == null
            ? DmLeafKind.device
            : DmLeafKind.claktory,
        name: member.split('/').last,
        sinceMs: BigInt.from(added[member] ?? 0),
      ),
  ];

  /// Whether `account/leaf` [member] is on its account's list here, or
  /// its account has none here.
  bool _listedHere(String member) {
    final list = lists[member.split('/').first];
    final leafId = member.split('/').last;
    return list == null || list.leaves.any((each) => each.leafId == leafId);
  }

  /// When each member's certificate was made, as `account/leaf`.
  final added = <String, int>{};

  /// The certificates this device issued, by leaf.
  final issued = <String>[];

  @override
  Future<Uint8List> issueCertificate({
    required DmLeafKind kind,
    required String leafId,
    required List<int> signatureKey,
    required String name,
    required BigInt createdMs,
  }) async {
    issued.add(leafId);
    return Uint8List.fromList(utf8.encode('certificate:$account:$leafId'));
  }

  @override
  Future<DmCommit> removeMembers({
    required List<int> groupId,
    required List<String> leafIds,
  }) async {
    final id = dmB64(groupId);
    final here = _groups[id]!;
    final members = [
      for (final member in here.members)
        if (!leafIds.contains(member.split('/').last)) member,
    ];
    final commit = fakeEnvelope({
      't': 'commit',
      'g': id,
      'epoch': here.epoch,
      'from': _member(),
      'members': members,
    });
    here
      ..pending = commit
      ..pendingEpoch = here.epoch + 1
      ..pendingMembers = members;
    return DmCommit(
      commit: commit,
      groupInfo: _info(id, here.epoch + 1, members),
    );
  }

  DmPin _pin(
    Uint8List key, {
    bool changed = false,
    bool verified = false,
    Uint8List? changedFrom,
    bool mismatched = false,
  }) => DmPin(
    identityPublic: key,
    verified: verified,
    changedUnseen: changed,
    canSend: !changed,
    deviceListVersion: BigInt.one,
    changedFrom: changedFrom,
    mismatched: mismatched,
  );

  /// Each account's keys moved off by a change, as they were (base64 →
  /// pin), and the ones the person said didn't match: as the crate's
  /// `previous` (E11-3).
  final _previous = <String, Map<String, DmPin>>{};
  final _mismatched = <String, Set<String>>{};

  @override
  Future<void> undoKeyChange({
    required String account,
    required List<int> backTo,
  }) async {
    final back = _previous[account]?.remove(dmB64(backTo));
    final now = pins[account];
    if (back == null || now == null) {
      throw StateError("that isn't an earlier security key to go back to");
    }
    (_mismatched[account] ??= {}).add(dmB64(now.identityPublic));
    _previous[account]![dmB64(now.identityPublic)] = now;
    pins[account] = _pin(back.identityPublic, verified: back.verified);
  }

  @override
  DmPin? pin({required String account}) => pins[account];

  String _side(String who, List<int> key) => '$who=${dmB64(key)}';

  @override
  DmSafety safetyNumber({required String account}) {
    final theirs = pins[account];
    if (theirs == null || identity == null) throw StateError('not pinned');
    final ours = _side(this.account, identity!);
    final them = _side(account, theirs.identityPublic);
    final both = ([ours, them]..sort()).join();
    final digits = [
      for (var i = 0; i < 12; i++)
        '${(both.hashCode.abs() + i * 7919) % 100000}'.padLeft(5, '0'),
    ].join(' ');
    return DmSafety(
      digits: digits,
      qr: Uint8List.fromList(utf8.encode('$ours|$them')),
    );
  }

  @override
  bool safetyMatches({required String account, required List<int> scanned}) {
    final mine = utf8.decode(safetyNumber(account: account).qr).split('|');
    return utf8.decode(scanned) == '${mine[1]}|${mine[0]}';
  }

  @override
  Future<void> firstContact({
    required String account,
    required List<int> identityPublic,
  }) async => pins[account] = _pin(Uint8List.fromList(identityPublic));

  @override
  Future<void> keyChanged({
    required String account,
    required List<int> identityPublic,
  }) async {
    // One this device won't take, as the crate refuses a key replaced for
    // good (E11-3).
    if (refusedKeys.contains(dmB64(identityPublic))) {
      throw StateError('this key was replaced');
    }
    keyChangedFor.add(account);
    final before = pins[account];
    final key = Uint8List.fromList(identityPublic);
    if (before != null) {
      (_previous[account] ??= {})[dmB64(before.identityPublic)] = before;
    }
    _previous[account]?.remove(dmB64(key));
    pins[account] = _pin(
      key,
      changed: true,
      changedFrom: before?.identityPublic,
      mismatched: _mismatched[account]?.contains(dmB64(key)) ?? false,
    );
  }

  /// Keys (base64) [keyChanged] refuses.
  final refusedKeys = <String>{};

  /// While set, [send] fails with it, as something unforeseen would.
  Object? sendFails;

  /// Accounts whose key changed here: their leaves the new list doesn't
  /// name are dead (row 630), as are this device's own account's after it
  /// set up under a new key ([replacedKey]).
  final keyChangedFor = <String>{};

  bool _dead(String member) {
    final who = member.split('/').first;
    final leafId = member.split('/').last;
    if (claktoryOf(leafId) != null || member == _member()) return false;
    final changed =
        keyChangedFor.contains(who) || (who == account && replacedKey != null);
    return changed && !_listedHere(member);
  }

  /// The person's devices [removeDeadLeaves] took out, as the Rust keeps
  /// them.
  final replaced = <DmMember>[];

  @override
  Future<DmCommit?> removeDeadLeaves({required List<int> groupId}) async {
    final here = _groups[dmB64(groupId)]!;
    final dead = [
      for (final member in here.members)
        if (_dead(member) && member.split('/').first == account) member,
    ];
    if (dead.isEmpty) return null;
    for (final member in dead) {
      final leafId = member.split('/').last;
      if (replaced.any((each) => each.leafId == leafId)) continue;
      replaced.add(
        DmMember(
          account: account,
          leafId: leafId,
          kind: DmLeafKind.device,
          name: leafId,
          sinceMs: BigInt.zero,
        ),
      );
    }
    return removeMembers(
      groupId: groupId,
      leafIds: [for (final member in dead) member.split('/').last],
    );
  }

  @override
  Future<List<DmMember>> replacedDevices() async => [...replaced];

  @override
  Future<void> forgetReplaced({required String leafId}) async =>
      replaced.removeWhere((each) => each.leafId == leafId);

  @override
  Future<List<String>> replacedLeaves({required List<int> groupId}) async => [
    if (replacedKey != null)
      for (final member in _groups[dmB64(groupId)]?.members ?? const <String>[])
        if (member.split('/').first == account &&
            member != _member() &&
            !_listedHere(member))
          member.split('/').last,
  ];

  /// Groups a reinstalled device's former install holds here (row 202), by
  /// group: whose, and the device's name.
  final reinstalledIn = <String, ({String account, String device})>{};

  @override
  Future<String?> reinstalling({required List<int> groupId}) async =>
      reinstalledIn[dmB64(groupId)]?.device;

  /// In a conversation, the other person when none of their devices is
  /// a member that passes ([members]) (row 676).
  String? _notJoined(_Here here) {
    if (here.kind.kind != DmGroupType.conversation) return null;
    // A wait for a new security key says so first ([waitingFor]).
    if (here.members.any(_dead)) return null;
    for (final account in here.kind.accounts) {
      if (account == this.account) continue;
      final present = here.members.any(
        (member) =>
            member.split('/').first == account &&
            claktoryOf(member.split('/').last) == null &&
            _listedHere(member) &&
            !_dead(member),
      );
      if (!present) return account;
    }
    return null;
  }

  @override
  Future<DmJoining?> notJoined({required List<int> groupId}) async {
    final here = _groups[dmB64(groupId)];
    if (here == null) return null;
    final account = _notJoined(here);
    if (account == null) return null;
    final first = here.members
        .where((member) => member.split('/').first == this.account)
        .firstOrNull;
    return DmJoining(account: account, addsFirst: first == _member());
  }

  @override
  Future<String?> waitingFor({required List<int> groupId}) async {
    if (reinstalledIn[dmB64(groupId)] case final held?) return held.account;
    for (final member in _groups[dmB64(groupId)]?.members ?? const <String>[]) {
      if (_dead(member)) return member.split('/').first;
    }
    return null;
  }

  @override
  Future<void> seen({required String account}) async {
    final now = pins[account]!;
    pins[account] = _pin(
      now.identityPublic,
      verified: now.verified,
      changedFrom: now.changedFrom,
      mismatched: now.mismatched,
    );
  }

  @override
  Future<void> verify({required String account}) async {
    final now = pins[account]!;
    _mismatched[account]?.remove(dmB64(now.identityPublic));
    pins[account] = _pin(
      now.identityPublic,
      verified: true,
      changedFrom: now.changedFrom,
    );
  }

  @override
  Future<void> acceptList({
    required String account,
    required List<int> deviceList,
  }) async {
    final json = jsonDecode(utf8.decode(deviceList)) as Map<String, Object?>;
    _takeList(
      account,
      DmDeviceList(
        version: BigInt.from(json['version']! as int),
        createdMs: BigInt.from(json['created'] as int? ?? 0),
        leaves: [
          for (final leaf in json['leaves']! as List)
            _listed(
              (leaf as Map)['leaf']! as String,
              dmUnb64(leaf['key']! as String),
            ),
        ],
        home: switch (json['home']) {
          final String home => dmUnb64(home),
          _ => null,
        },
      ),
    );
  }

  @override
  Future<Uint8List> exportPins() async => Uint8List.fromList(
    utf8.encode(jsonEncode({for (final key in pins.keys) key: true})),
  );

  @override
  Future<List<String>> keyPackages({required int count}) async => [
    for (var i = 0; i < count; i++)
      fakeEnvelope({'t': 'kp', 'member': _member(), 'n': ++_packages}),
  ];

  @override
  Future<String> lastResortKeyPackage() async =>
      fakeEnvelope({'t': 'kp', 'member': _member(), 'n': 0});

  /// When [sweepKeyPackages] ran, in seconds.
  final swept = <int>[];

  @override
  bool lastResortDue({required int lastResortEnds, required int nowSecs}) =>
      lastResortEnds != 0 && lastResortEnds <= nowSecs + 7 * 24 * 60 * 60;

  @override
  Future<int> sweepKeyPackages({required int nowSecs}) async {
    swept.add(nowSecs);
    return 0;
  }

  String _info(String group, int epoch, List<String> members) => fakeEnvelope({
    't': 'info',
    'g': group,
    'epoch': epoch,
    'members': members,
    'accounts': _groups[group]?.kind.accounts ?? const [],
  });

  @override
  Future<String> createGroup({
    required List<int> groupId,
    required DmGroupKind kind,
  }) async {
    final id = dmB64(groupId);
    _groups[id] = _Here(kind, 0, [_member()]);
    return _info(id, 0, [_member()]);
  }

  @override
  Future<void> forgetGroup({required List<int> groupId}) async {
    final id = dmB64(groupId);
    _groups.remove(id);
    _history.remove(id);
  }

  /// Every takeProposal call's outcome, for tests.
  final takenProposals = <DmProcessed>[];

  @override
  int proposalSeq({required List<int> groupId}) {
    final here = _groups[dmB64(groupId)];
    if (here == null) return 0;
    return here.proposalSeq ?? (here.seq <= dmHomeSeqBase ? here.seq : 0);
  }

  @override
  Future<DmProcessed> takeProposal({
    required List<int> groupId,
    required int seq,
    required String envelope,
  }) async {
    final here = _groups[dmB64(groupId)]!;
    DmProcessed out(DmOutcome outcome) {
      final processed = DmProcessed(
        outcome: outcome,
        epoch: BigInt.from(here.epoch),
        why: '',
        handshake: true,
        replaced: false,
        behind: false,
        unlisted: false,
      );
      takenProposals.add(processed);
      return processed;
    }

    if (seq <= proposalSeq(groupId: groupId)) return out(DmOutcome.already);
    here.proposalSeq = seq;
    final body = readFake(envelope);
    if (body['t'] != 'proposal') return out(DmOutcome.already);
    if (body['epoch'] != here.epoch) return out(DmOutcome.dropped);
    here.proposals.add(body['p']! as String);
    return out(DmOutcome.proposal);
  }

  /// Every takeWorkerCommit call's outcome, for tests.
  final takenWorkerCommits = <DmProcessed>[];

  @override
  Future<DmProcessed> takeWorkerCommit({
    required List<int> groupId,
    required int seq,
    required String envelope,
    required int arrivedMs,
  }) async {
    final here = _groups[dmB64(groupId)]!;
    DmProcessed out(DmOutcome outcome) {
      final processed = DmProcessed(
        outcome: outcome,
        epoch: BigInt.from(here.epoch),
        why: '',
        handshake: true,
        replaced: false,
        behind: false,
        unlisted: false,
      );
      takenWorkerCommits.add(processed);
      return processed;
    }

    if (seq <= proposalSeq(groupId: groupId)) return out(DmOutcome.already);
    here.proposalSeq = seq;
    final body = readFake(envelope);
    if (body['t'] != 'commit') return out(DmOutcome.already);
    if (envelope == here.pending) {
      here
        ..epoch = here.pendingEpoch
        ..members = here.pendingMembers
        ..pending = null;
      return out(DmOutcome.committed);
    }
    if (body['epoch'] != here.epoch) return out(DmOutcome.already);
    // One the checks refuse: dropped, passed.
    if (body['bad'] == true) return out(DmOutcome.dropped);
    here
      ..pending = null
      ..epoch += 1
      ..members = [for (final m in body['members']! as List) m as String];
    return out(DmOutcome.committed);
  }

  @override
  Future<void> skipProposals({
    required List<int> groupId,
    required int seq,
  }) async {
    final here = _groups[dmB64(groupId)]!;
    if (seq > proposalSeq(groupId: groupId)) here.proposalSeq = seq;
  }

  @override
  Future<void> joinGroup({
    required List<int> groupId,
    required DmGroupKind kind,
    required String welcome,
    required int seq,
  }) async {
    final body = readFake(welcome);
    final id = dmB64(groupId);
    if (!(body['to']! as List).contains(leaf)) throw StateError('not mine');
    _groups[id] = _Here(kind, body['epoch']! as int, [
      for (final member in body['members']! as List) member as String,
    ])..seq = seq;
  }

  /// When set, what a join by external commit throws (a member's checks
  /// failing, say).
  Object? Function()? refuseRejoin;

  @override
  Future<void> noteSkipped({
    required List<int> groupId,
    required int seq,
    required int arrivedMs,
    required String because,
  }) async {
    _history.putIfAbsent(dmB64(groupId), () => [])
      ..add(
        DmMessage(
          departed: false,
          seq: seq,
          mine: false,
          fromAccount: '',
          fromLeaf: '',
          fromKindDevice: false,
          content: const DmContent(
            kind: '',
            clientMessageId: '',
            extraJson: '',
          ),
          arrivedMs: arrivedMs,
          members: DmMembers(
            joined: const [],
            setUpAgain: const [],
            added: const [],
            removed: const [],
            keyChanged: const [],
            skipped: because,
          ),
        ),
      )
      ..sort((a, b) => a.seq.compareTo(b.seq));
  }

  @override
  Future<DmCommit> rejoinGroup({
    required List<int> groupId,
    required DmGroupKind kind,
    required String groupInfo,
    required int seq,
  }) async {
    if (refuseRejoin?.call() case final refusal?) throw refusal;
    final info = readFake(groupInfo);
    final id = dmB64(groupId);
    final epoch = info['epoch']! as int;
    final members = [
      for (final member in info['members']! as List)
        if (member != _member()) member as String,
      _member(),
    ];
    final here = _groups[id] = _Here(kind, epoch, members)..seq = seq;
    final commit = fakeEnvelope({
      't': 'commit',
      'g': id,
      'epoch': epoch,
      'from': _member(),
      'members': members,
    });
    here
      ..pending = commit
      ..pendingEpoch = epoch + 1
      ..pendingMembers = members;
    return DmCommit(commit: commit, groupInfo: _info(id, epoch + 1, members));
  }

  @override
  Future<DmCommit?> addMembers({
    required List<int> groupId,
    required List<String> keyPackages,
  }) async {
    final id = dmB64(groupId);
    final here = _groups[id]!;
    // As Group::add: one in the group already, or repeated, is left out;
    // none left is nothing to add.
    final added = <String>[];
    for (final package in keyPackages) {
      final member = readFake(package)['member']! as String;
      if (!here.members.contains(member) && !added.contains(member)) {
        added.add(member);
      }
    }
    if (added.isEmpty) return null;
    // As Group::add: each against its account's list as this device took
    // it (Fugo, row 676).
    for (final member in added) {
      if (!_listedHere(member)) {
        throw StateError("a device that isn't on its account's list");
      }
    }
    final members = [...here.members, ...added];
    final commit = fakeEnvelope({
      't': 'commit',
      'g': id,
      'epoch': here.epoch,
      'from': _member(),
      'members': members,
    });
    here
      ..pending = commit
      ..pendingEpoch = here.epoch + 1
      ..pendingMembers = members;
    return DmCommit(
      commit: commit,
      welcome: fakeEnvelope({
        't': 'welcome',
        'g': id,
        'epoch': here.epoch + 1,
        'members': members,
        'to': [for (final member in added) member.split('/').last],
      }),
      groupInfo: _info(id, here.epoch + 1, members),
    );
  }

  @override
  Future<void> commitRefused({required List<int> groupId}) async =>
      _groups[dmB64(groupId)]?.pending = null;

  @override
  Future<List<DmGroup>> groups() async => [
    for (final MapEntry(:key, :value) in _groups.entries)
      DmGroup(
        groupId: dmUnb64(key),
        kind: value.kind,
        seq: value.seq,
        epoch: BigInt.from(value.epoch),
        moved: value.moved,
        pending: value.pending != null,
      ),
  ];

  @override
  Future<String> send({
    required List<int> groupId,
    required DmContent content,
    required int nowMs,
  }) async {
    final id = dmB64(groupId);
    final here = _groups[id]!;
    if (sendFails case final error?) throw error;
    if (here.kind.kind == DmGroupType.conversation) {
      for (final account in here.kind.accounts) {
        if (pins[account]?.canSend == false) throw StateError('key changed');
      }
    }
    if (_notJoined(here) case final account?) {
      throw StateError('waiting for $account to join');
    }
    if (_outgoing[content.clientMessageId] case final kept?) {
      return kept.envelope;
    }
    // The sender's clock inside, as the Rust puts it (row 556).
    final extra = content.extraJson.trim().isEmpty
        ? <String, Object?>{}
        : jsonDecode(content.extraJson) as Map<String, Object?>;
    if (nowMs > 0 && !extra.containsKey('sent_ms')) {
      extra['sent_ms'] = nowMs;
      content = DmContent(
        kind: content.kind,
        clientMessageId: content.clientMessageId,
        text: content.text,
        byAgent: content.byAgent,
        extraJson: jsonEncode(extra),
      );
    }
    final envelope = fakeEnvelope({
      't': 'app',
      'g': id,
      'epoch': here.epoch,
      'from': _member(),
      'c': _content(content),
    });
    _outgoing[content.clientMessageId] = (
      group: id,
      envelope: envelope,
      content: content,
      at: nowMs,
    );
    if (here.kind.kind == DmGroupType.selfGroup) _view(content);
    return envelope;
  }

  static Map<String, Object?> _content(DmContent content) => {
    'kind': content.kind,
    'id': content.clientMessageId,
    'text': content.text,
    if (content.byAgent case final by?)
      'by': {
        'claktory_id': by.claktoryId,
        'agent_id': by.agentId,
        'incarnation': by.incarnation,
        'name': by.name,
      },
    'extra': content.extraJson,
  };

  /// What [from] (a Claktory's leaf, `account/leaf`) says in [group], as
  /// its envelope: an agent's proposal or draft, sealed in its channel.
  static String claktorySays(
    String group,
    String from,
    DmContent content, {
    int epoch = 1,
  }) => fakeEnvelope({
    't': 'app',
    'g': group,
    'epoch': epoch,
    'from': from,
    'c': _content(content),
  });

  static DmContent _fromJson(Map<String, Object?> json) => DmContent(
    kind: json['kind']! as String,
    clientMessageId: json['id']! as String,
    text: json['text'] as String?,
    byAgent: switch (json['by']) {
      final Map<String, Object?> by => DmByAgent(
        claktoryId: by['claktory_id']! as String,
        agentId: by['agent_id']! as String,
        incarnation: by['incarnation']! as String,
        name: by['name']! as String,
      ),
      _ => null,
    },
    extraJson: json['extra'] as String? ?? '',
  );

  void _view(DmContent content) {
    final extra = content.extraJson.isEmpty
        ? const <String, Object?>{}
        : jsonDecode(content.extraJson) as Map<String, Object?>;
    final group = extra['group'] as String? ?? '';
    final view =
        _views[group] ??
        const DmView(draft: '', draftMs: 0, readSeq: 0, deletedSeq: 0);
    _views[group] = switch (content.kind) {
      'draft' => DmView(
        draft: content.text ?? '',
        draftMs: extra['at_ms'] as int? ?? 0,
        readSeq: view.readSeq,
        deletedSeq: view.deletedSeq,
      ),
      'read' => DmView(
        draft: view.draft,
        draftMs: view.draftMs,
        readSeq: extra['seq'] as int? ?? 0,
        deletedSeq: view.deletedSeq,
      ),
      'delete' => DmView(
        draft: view.draft,
        draftMs: view.draftMs,
        readSeq: view.readSeq,
        deletedSeq: extra['seq'] as int? ?? 0,
      ),
      _ => view,
    };
    if (content.kind == 'delete') {
      final through = extra['seq'] as int? ?? 0;
      _history[group]?.removeWhere((message) => message.seq <= through);
    }
  }

  @override
  Future<DmView> view({required List<int> groupId}) async =>
      _views[dmB64(groupId)] ??
      const DmView(draft: '', draftMs: 0, readSeq: 0, deletedSeq: 0);

  @override
  Future<List<DmUnsent>> unsent() async => [
    for (final MapEntry(:key, :value) in _outgoing.entries)
      DmUnsent(
        groupId: dmUnb64(value.group),
        clientMessageId: key,
        envelope: value.envelope,
        createdMs: value.at,
        content: value.content,
      ),
  ];

  @override
  Future<void> dropUnsent({
    required List<int> groupId,
    required String clientMessageId,
  }) async => _outgoing.remove(clientMessageId);

  @override
  Future<DmProcessed> process({
    required List<int> groupId,
    required int seq,
    required String envelope,
    required int arrivedMs,
  }) async {
    final id = dmB64(groupId);
    final here = _groups[id]!;
    DmProcessed out(
      DmOutcome outcome, {
      DmMessage? message,
      bool handshake = false,
      bool behind = false,
    }) {
      final processed = DmProcessed(
        outcome: outcome,
        message: message,
        epoch: BigInt.from(here.epoch),
        why: '',
        handshake: handshake,
        replaced: false,
        behind: behind,
        unlisted: false,
      );
      this.processed.add(processed);
      return processed;
    }

    if (seq <= here.seq) return out(DmOutcome.already);
    final before = here.seq;
    here.seq = seq;
    final body = readFake(envelope);
    // From an epoch past this one, as the Rust holds it (row 551).
    if ((body['epoch'] as int? ?? 0) > here.epoch &&
        envelope != here.pending &&
        !here.removed &&
        !_outgoing.values.any((each) => each.envelope == envelope)) {
      here.seq = before;
      return out(DmOutcome.held, handshake: body['t'] != 'app', behind: true);
    }
    if (body['t'] == 'commit') {
      if (envelope == here.pending) {
        here
          ..epoch = here.pendingEpoch
          ..members = here.pendingMembers
          ..moved = here.moved || here.pendingMove
          ..pendingMove = false
          ..pending = null;
        return out(DmOutcome.committed);
      }
      if (body['epoch'] != here.epoch) {
        return out(DmOutcome.dropped, handshake: true);
      }
      // One that takes in proposals by reference: only with them here.
      final takes = [for (final p in body['takes'] as List? ?? const []) p];
      if (!takes.every(here.proposals.contains)) {
        return out(DmOutcome.dropped, handshake: true);
      }
      // A leaf its account's newest list here doesn't name (row 561):
      // held, the position kept.
      if (_unlistedIn(here, body) case final leaf?) {
        here.seq = before;
        final held = DmProcessed(
          outcome: DmOutcome.held,
          epoch: BigInt.from(here.epoch),
          why: leaf.split('/').first,
          handshake: true,
          replaced: false,
          behind: false,
          unlisted: true,
        );
        processed.add(held);
        return held;
      }
      here.proposals.clear();
      // A move: its leaf under a key this device hasn't pinned is held,
      // the position kept.
      if (body['id'] case final String key) {
        final whose = (body['from']! as String).split('/').first;
        if (pins[whose] == null || dmB64(pins[whose]!.identityPublic) != key) {
          here.seq = before;
          final held = DmProcessed(
            outcome: DmOutcome.held,
            epoch: BigInt.from(here.epoch),
            why: whose,
            handshake: true,
            replaced: retiredHere[whose]?.contains(key) ?? false,
            behind: false,
            unlisted: false,
          );
          processed.add(held);
          return held;
        }
        // The first of their leaves to move here: the key-changed line.
        if (whose != account && _keyLines.add('$id/$key')) {
          here
            ..epoch += 1
            ..members = [for (final m in body['members']! as List) m as String];
          final line = DmMessage(
            departed: false,
            seq: seq,
            mine: false,
            fromAccount: '',
            fromLeaf: '',
            fromKindDevice: false,
            content: const DmContent(
              kind: '',
              clientMessageId: '',
              extraJson: '',
            ),
            arrivedMs: arrivedMs,
            members: DmMembers(
              joined: const [],
              setUpAgain: const [],
              added: const [],
              removed: const [],
              keyChanged: [whose],
            ),
          );
          _history.putIfAbsent(id, () => []).add(line);
          return out(DmOutcome.committed, message: line);
        }
      }
      here
        ..epoch += 1
        ..members = [for (final m in body['members']! as List) m as String];
      if (!here.members.contains(_member())) here.removed = true;
      return out(DmOutcome.committed);
    }
    final from = body['from']! as String;
    final own = _outgoing.entries
        .where((each) => each.value.envelope == envelope)
        .firstOrNull;
    if (own == null && (here.removed || body['epoch'] != here.epoch)) {
      return out(DmOutcome.dropped);
    }
    final content = _fromJson(body['c']! as Map<String, Object?>);
    if (own != null) _outgoing.remove(own.key);
    final message = DmMessage(
      departed: false,
      seq: seq,
      mine: own != null,
      fromAccount: from.split('/').first,
      fromLeaf: from.split('/').last,
      fromKindDevice: true,
      content: content,
      arrivedMs: arrivedMs,
    );
    if (here.kind.kind == DmGroupType.selfGroup) {
      _view(content);
      if (content.kind == 'rotation' && own == null) {
        _keepRotation(here, from, content);
      }
    } else if (here.kind.kind == DmGroupType.channel &&
        (content.kind == 'proposal' || content.kind == 'agent_draft')) {
      // Kept by its digest, as the Rust keeps it (#451).
      final leafId = from.split('/').last;
      _sideKept['$id/${sha256.convert(dmBytesOf(envelope))}'] = DmMessage(
        departed: false,
        seq: seq,
        mine: false,
        fromAccount: from.split('/').first,
        fromLeaf: leafId,
        fromKindDevice: claktoryOf(leafId) == null,
        content: content,
        arrivedMs: arrivedMs,
      );
    } else {
      _history.putIfAbsent(id, () => []).add(message);
    }
    return out(DmOutcome.message, message: message);
  }

  /// Leaves ("account/leaf") their account's newest list here doesn't
  /// name (row 561): a commit bringing one in is held as unlisted.
  final unlisted = <String>{};

  String? _unlistedIn(_Here here, Map<String, Object?> commit) => [
    for (final member in commit['members'] as List? ?? const [])
      if (!here.members.contains(member) && unlisted.contains(member))
        member as String,
  ].firstOrNull;

  /// Every processWindow call's items, for tests.
  final windows = <List<DmWindowItem>>[];

  /// The window as the crate keeps it (row 561): [items] opens with a
  /// commit held as unlisted; a later commit, by a member there before
  /// it, that takes its leaf out again closes it, and everything up to
  /// that is taken, the departed leaf's messages marked.
  @override
  Future<DmWindowOutcome> processWindow({
    required List<int> groupId,
    required List<DmWindowItem> items,
  }) async {
    windows.add(items);
    final id = dmB64(groupId);
    final here = _groups[id]!;
    DmWindowOutcome end(DmWindowEnd end, [String why = '']) =>
        DmWindowOutcome(end: end, why: why, processed: const []);
    if (here.kind.kind != DmGroupType.conversation) {
      return end(DmWindowEnd.none, 'not a conversation');
    }
    final leaf = _unlistedIn(here, readFake(items.first.envelope));
    if (leaf == null) return end(DmWindowEnd.none, 'not unlisted');
    var close = -1;
    for (var i = 1; i < items.length && close < 0; i++) {
      final body = readFake(items[i].envelope);
      if (body['t'] != 'commit') continue;
      if ((body['members']! as List).contains(leaf)) continue;
      final from = body['from'] as String?;
      if (from == null || from == leaf || !here.members.contains(from)) {
        return end(DmWindowEnd.aborted, 'not by a member there all along');
      }
      close = i;
    }
    if (close < 0) return end(DmWindowEnd.open);
    unlisted.remove(leaf);
    final taken = <DmProcessed>[];
    try {
      for (final item in items.take(close + 1)) {
        final each = await process(
          groupId: groupId,
          seq: item.seq,
          envelope: item.envelope,
          arrivedMs: item.arrivedMs,
        );
        final message = each.message;
        if (message == null ||
            '${message.fromAccount}/${message.fromLeaf}' != leaf) {
          taken.add(each);
          continue;
        }
        final departed = DmMessage(
          departed: true,
          seq: message.seq,
          mine: message.mine,
          fromAccount: message.fromAccount,
          fromLeaf: message.fromLeaf,
          fromKindDevice: message.fromKindDevice,
          content: message.content,
          arrivedMs: message.arrivedMs,
          members: message.members,
        );
        final history = _history[id]!;
        history[history.lastIndexOf(message)] = departed;
        taken.add(
          DmProcessed(
            outcome: each.outcome,
            message: departed,
            epoch: each.epoch,
            why: each.why,
            handshake: each.handshake,
            replaced: each.replaced,
            behind: each.behind,
            unlisted: each.unlisted,
          ),
        );
      }
    } finally {
      unlisted.add(leaf);
    }
    return DmWindowOutcome(end: DmWindowEnd.closed, why: '', processed: taken);
  }

  // Lost or stolen (R4): the rotation, as the Rust keeps it.

  DmRotation? _rotation;
  DmPendingRotation? _pendingRotation;
  Uint8List? _pendingKey;

  static Uint8List _public(List<int> secret) =>
      Uint8List.fromList([...secret.reversed]);

  @override
  Future<DmRotation> prepareRotation({
    required List<int> identityKey,
    required List<String> kept,
    required List<String> removed,
    required BigInt nowMs,
  }) async {
    final public = _public(identityKey);
    if (_rotation case final before?) {
      if (!listEqualsInts(before.identityPublic, public)) {
        throw StateError('another rotation is under way');
      }
      return before;
    }
    final self = _groups.values
        .where((each) => each.kind.kind == DmGroupType.selfGroup)
        .first;
    final inSelf = {for (final m in self.members) m.split('/').last};
    if (!setEqualsStrings(inSelf, {...kept, leaf})) {
      throw StateError('the self group holds other devices: $inSelf');
    }
    final list = DmDeviceList(
      version: BigInt.one,
      createdMs: nowMs,
      leaves: [
        for (final each in lists[account]!.leaves)
          if (each.leafId == leaf || kept.contains(each.leafId)) each,
      ],
    );
    final key = Uint8List.fromList(List.filled(32, public.first));
    return _rotation = DmRotation(
      identityPublic: public,
      deviceList: _listBytes(list),
      sealed: Uint8List.fromList(
        utf8.encode(
          jsonEncode({
            'rotation': dmB64(key),
            'secret': dmB64(identityKey),
            'list': utf8.decode(_listBytes(list)),
          }),
        ),
      ),
      key: key,
      kept: kept,
      removed: removed,
      said: false,
    );
  }

  @override
  Future<DmRotation?> rotation() async => _rotation;

  @override
  Future<void> rotationSaid() async {
    final r = _rotation!;
    _rotation = DmRotation(
      identityPublic: r.identityPublic,
      deviceList: r.deviceList,
      sealed: r.sealed,
      key: r.key,
      kept: r.kept,
      removed: r.removed,
      said: true,
    );
  }

  @override
  Future<void> dropRotation() async => _rotation = null;

  void _newKey(Uint8List public, String list) {
    identity = public;
    pins[account] = _pin(public);
    final json = jsonDecode(list) as Map<String, Object?>;
    _takeList(
      account,
      DmDeviceList(
        version: BigInt.from(json['version']! as int),
        createdMs: BigInt.from(json['created'] as int? ?? 0),
        leaves: [
          for (final each in json['leaves']! as List)
            _listed(
              (each as Map)['leaf']! as String,
              dmUnb64(each['key']! as String),
            ),
        ],
      ),
    );
    for (final here in _groups.values) {
      here.moved = false;
    }
  }

  @override
  Future<Uint8List> finishRotation({required List<int> identityKey}) async {
    final r = _rotation!;
    if (!listEqualsInts(r.identityPublic, _public(identityKey))) {
      throw StateError("not the rotation's key");
    }
    _newKey(r.identityPublic, utf8.decode(r.deviceList));
    _rotation = null;
    // The Lock out it finishes, with it, as the Rust's one transaction.
    removalRow = null;
    return Uint8List(0);
  }

  /// The removal under way (R5b), as the Rust keeps it in the store.
  DmRemoval? removalRow;

  @override
  Future<void> beginRemoval({required DmRemoval removal}) async {
    removedLeaves.addAll(removal.remove);
    removalRow = removal;
  }

  @override
  Future<DmRemoval?> removal() async => removalRow;

  @override
  Future<void> endRemoval() async => removalRow = null;

  void _keepRotation(_Here self, String from, DmContent content) {
    final extra = jsonDecode(content.extraJson) as Map<String, Object?>;
    final [whose, fromLeaf] = from.split('/');
    final removed = [for (final r in extra['removed']! as List) r as String];
    final key = extra['identity']! as String;
    if (whose != account || fromLeaf == leaf || removed.contains(fromLeaf)) {
      return;
    }
    if (self.members.any((m) => removed.contains(m.split('/').last))) return;
    if (key == dmB64(pins[account]!.identityPublic) || _retired.contains(key)) {
      return;
    }
    _pendingRotation = DmPendingRotation(
      fromLeaf: fromLeaf,
      fromName: fromLeaf,
      identityPublic: dmUnb64(key),
    );
    _pendingKey = dmUnb64(extra['key']! as String);
  }

  final _retired = <String>{};

  @override
  Future<DmPendingRotation?> pendingRotation() async => _pendingRotation;

  @override
  Future<Uint8List> openRotation({required List<int> sealed}) async {
    final body = jsonDecode(utf8.decode(sealed)) as Map<String, Object?>;
    if (_pendingKey == null || body['rotation'] != dmB64(_pendingKey!)) {
      throw StateError("doesn't open");
    }
    final secret = dmUnb64(body['secret']! as String);
    if (!listEqualsInts(_public(secret), _pendingRotation!.identityPublic)) {
      throw StateError('another key than the message named');
    }
    return secret;
  }

  @override
  Future<Uint8List> takeRotation({
    required List<int> identityKey,
    required List<int> sealed,
  }) async {
    final body = jsonDecode(utf8.decode(sealed)) as Map<String, Object?>;
    _retired.add(dmB64(identity!));
    _newKey(_public(identityKey), body['list']! as String);
    _pendingRotation = null;
    _pendingKey = null;
    return Uint8List(0);
  }

  @override
  Future<void> dropPendingRotation() async {
    _pendingRotation = null;
    _pendingKey = null;
  }

  @override
  Future<DmCommit> moveGroup({required List<int> groupId}) async {
    final id = dmB64(groupId);
    final here = _groups[id]!;
    final commit = fakeEnvelope({
      't': 'commit',
      'g': id,
      'epoch': here.epoch,
      'from': _member(),
      'members': here.members,
      'id': dmB64(identity!),
    });
    here
      ..pending = commit
      ..pendingEpoch = here.epoch + 1
      ..pendingMembers = here.members
      ..pendingMove = true;
    return DmCommit(
      commit: commit,
      groupInfo: _info(id, here.epoch + 1, here.members),
    );
  }

  @override
  Future<void> skip({required List<int> groupId, required int seq}) async =>
      _groups[dmB64(groupId)]!.seq = seq;

  /// Puts [member] (`account/leaf`) in [group] here, as a group that still
  /// holds a device would.
  void holdLeaf(String group, String member) =>
      _groups[group]!.members = [..._groups[group]!.members, member];

  @override
  Future<List<String>> leafIds({required List<int> groupId}) async => [
    for (final m in _groups[dmB64(groupId)]?.members ?? const <String>[])
      m.split('/').last,
  ];

  final _keyLines = <String>{};

  /// Puts [message] in [group]'s history, as if it had come (a line
  /// saying who joined, say).
  void keep(String group, DmMessage message) =>
      _history.putIfAbsent(group, () => []).add(message);

  /// Proposals and drafts taken from a channel's feed, by group and digest.
  final _sideKept = <String, DmMessage>{};

  @override
  Future<DmMessage?> sideMessage({
    required List<int> groupId,
    required List<int> digest,
  }) async => _sideKept['${dmB64(groupId)}/${_hex(digest)}'];

  static String _hex(List<int> bytes) =>
      bytes.map((each) => each.toRadixString(16).padLeft(2, '0')).join();

  /// Envelopes opened outside the feed (proposals, agent drafts).
  final openedSide = <String>[];

  @override
  Future<DmMessage> openSide({
    required List<int> groupId,
    required String envelope,
    required int arrivedMs,
  }) async {
    if (!_groups.containsKey(dmB64(groupId))) throw StateError('not a member');
    final body = readFake(envelope);
    if (body['t'] != 'app' || body['g'] != dmB64(groupId)) {
      throw StateError('not something said here');
    }
    openedSide.add(envelope);
    final from = body['from']! as String;
    return DmMessage(
      departed: false,
      seq: 0,
      mine: false,
      fromAccount: from.split('/').first,
      fromLeaf: from.split('/').last,
      fromKindDevice: claktoryOf(from.split('/').last) == null,
      content: _fromJson(body['c']! as Map<String, Object?>),
      arrivedMs: arrivedMs,
    );
  }

  @override
  Future<List<DmMessage>> history({
    required List<int> groupId,
    int? beforeSeq,
    required int limit,
  }) async {
    final before = [
      for (final message in _history[dmB64(groupId)] ?? const <DmMessage>[])
        if (beforeSeq == null || message.seq < beforeSeq) message,
    ];
    return before.sublist(before.length > limit ? before.length - limit : 0);
  }

  /// The leaves it has history for (one it added, E8); every one unless
  /// set.
  Set<String>? historyFor;

  @override
  bool hasHistoryFor({required String leafId}) =>
      historyFor?.contains(leafId) ?? true;

  @override
  void dispose() {}

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

/// Mista's transfer endpoint (E8), driven by the test: [progress] is what
/// the new device receives; what the old one sent is in [sent].
class FakeTransfer implements DmTransferEndpoint {
  FakeTransfer(this.n);

  final int n;
  final progress = StreamController<DmTransferProgress>.broadcast();
  final sent = <String>[];
  var receiving = 0;
  var cancelled = false;
  var closed = false;
  var sendEnd = DmTransferEnd.complete;

  @override
  Uint8List id() => Uint8List.fromList(List.filled(32, n));

  @override
  String idHex() => n.toRadixString(16).padLeft(2, '0') * 32;

  @override
  Stream<DmTransferProgress> receive({required DmDevice device}) {
    receiving++;
    return progress.stream;
  }

  @override
  Future<DmTransferEnd> sendHistory({
    required DmDevice device,
    required String toLeaf,
  }) async {
    sent.add(toLeaf);
    return sendEnd;
  }

  @override
  void cancel() => cancelled = true;

  @override
  Future<void> close() async => closed = true;

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

bool listEqualsInts(List<int> a, List<int> b) =>
    a.length == b.length &&
    [for (var i = 0; i < a.length; i++) a[i] == b[i]].every((same) => same);

bool setEqualsStrings(Set<String> a, Set<String> b) =>
    a.length == b.length && a.containsAll(b);

/// The digits two devices compare for [code].
String fakeDigits(String code) {
  final digits = '${code.hashCode.abs()}'.padLeft(20, '7');
  return [for (var i = 0; i < 20; i += 5) digits.substring(i, i + 5)].join(' ');
}

/// A new device's offer: its QR code's text is what it names.
class FakeHandoff implements DmHandoff {
  FakeHandoff(this.code);

  final String code;
  var disposed = false;

  @override
  String qr() => code;

  @override
  String comparison() => fakeDigits(jsonEncode(jsonDecode(code)));

  @override
  void dispose() => disposed = true;

  @override
  bool get isDisposed => disposed;
}

/// A person's device in a [FakeWorker]'s world: its keys, its store, and
/// the engine over both.
class FakeEndpoint {
  /// How long each engine made from now waits before trying again (the
  /// engine's own default unless a test shortens it).
  static var retryAfter = const Duration(seconds: 5);

  /// [route] wraps the worker's calls, to send others elsewhere.
  FakeEndpoint(
    this.worker,
    this.account,
    this.leaf, {
    String? name,
    SignedCall Function(SignedCall worker)? route,
  }) : keys = FakeDmKeys() {
    device = FakeDmDevice(utf8.encode(leaf));
    final call = worker.as(account, leaf);
    _make = () =>
        E2ee(
            session: UserAppsSession(
              DeviceAccount(key: 'k', email: '$account@example.com'),
              nonce: () => 'n',
              call: route?.call(call) ?? call,
            ),
            keys: keys,
            open:
                ({
                  required String path,
                  required List<int> storeKey,
                  required List<int> leafKey,
                  Uint8List? identityKey,
                }) async {
                  storeKept = true;
                  return device;
                },
            newSecret: () async =>
                Uint8List.fromList(List.filled(32, ++_secrets)),
            removeStore: (_) async {
              storeKept = false;
              device.forget();
            },
            storeExists: (_) async => storeKept,
            whoAmI: () async =>
                (accountId: account, name: name ?? leaf, deviceKey: leaf),
            nowMs: () => now,
            newId: () => '$leaf-${++_ids}',
            bindTransfer: () async {
              final transfer = FakeTransfer(transfers.length + 1);
              transfers.add(transfer);
              return transfer;
            },
            admitTransfer: (_) async => admit,
            retryAfter: retryAfter,
          )
          // The Claktory shown is the person's own: the account's word on it
          // is dm_home_test's (Fugo 40182).
          ..ungatedForTests = true;
    e2ee = _make();
  }

  /// The app quit and opened again: a new engine over the same key store
  /// and store, as a launch with keys already kept finds them.
  E2ee relaunch() {
    e2ee.dispose();
    return e2ee = _make();
  }

  /// Whether this endpoint's store is there: made at its first open, gone
  /// once removed or at a [reinstall].
  var storeKept = false;

  /// The app deleted and installed again (row 202): its store gone, its
  /// key store kept, as an iPhone's keychain keeps it.
  E2ee reinstall() {
    e2ee.dispose();
    storeKept = false;
    device.forget();
    return e2ee = _make();
  }

  late final E2ee Function() _make;
  var _secrets = 0;

  /// Makes this endpoint's next secrets differ from another's that made
  /// [count] already (each counts from one).
  void skipSecrets(int count) => _secrets += count;

  /// The endpoints it bound for history (E8), and whether the relay admits
  /// the next.
  final transfers = <FakeTransfer>[];
  var admit = true;

  /// Its clock.
  var now = 1000;

  final FakeWorker worker;
  final String account;
  final String leaf;
  final FakeDmKeys keys;
  late final FakeDmDevice device;
  late E2ee e2ee;
  var _ids = 0;
}

/// The Claktory the app shows, as Ghiaccio's leaf answers: its offer, and
/// once enrolled, its key packages up at [worker].
class FakeClaktoryLink implements DmClaktoryLink {
  FakeClaktoryLink(this.worker, this.account, {this.leafId = 'mini#1'});

  final FakeWorker worker;
  final String account;
  final String leafId;
  var enrolled = false;
  final enrollments = <({List<int> certificate, List<int> pins})>[];

  /// Whether it puts its key packages up when enrolled (it does, after a
  /// moment; false to hold them).
  var stock = true;

  @override
  Future<DmLeafOffer> offer() async => DmLeafOffer(
    leafSignatureKey: List.filled(32, 7),
    claktoryId: claktoryOf(leafId),
    registrationEpoch: Int64(1),
    leafId: leafId,
    name: 'Mac mini',
    enrolled: enrolled,
  );

  @override
  Future<void> enroll(List<int> certificate, List<int> pins) async {
    enrolled = true;
    enrollments.add((certificate: certificate, pins: pins));
    if (stock) stockUp();
  }

  void stockUp() {
    worker.packages['$account/$leafId'] = [
      for (var n = 1; n <= 5; n++)
        jsonEncode(
          jsonDecode(
            fakeEnvelope({'t': 'kp', 'member': '$account/$leafId', 'n': n}),
          ),
        ),
    ];
  }
}

/// The person's home Claktory as the app's link to it (#451, Mista's H3):
/// a log per group numbered from 2^32 + 1, receipts, a feed pushed to the
/// device following it, deliveries the test says, and routes. What the
/// person says (not commits, which go to the worker themselves) reaches
/// [worker] too, as their friend's home would pass it on; what others say
/// comes in with [pull].
class FakeHome implements DmHomeLink {
  FakeHome(this.worker);

  final FakeWorker worker;

  @override
  final ValueNotifier<bool> connected = ValueNotifier(true);

  /// Whether the person's list makes it their home: NOT_HOME otherwise.
  var isHome = true;

  /// When it became the person's home (row 595); 0 for not said.
  var homeSinceMs = 0;
  var endpointId = List<int>.filled(32, 5);

  final _feeds = StreamController<pb.DmFeed>.broadcast();
  final _homes = StreamController<pb.DmHome>.broadcast();
  final sends = <pb.DmSend>[];
  final follows = <pb.DmFollow>[];
  final acks = <pb.DmAck>[];
  final routesSaid = <List<pb.DmRoute>>[];
  final peers = <pb.DmPeer>[];

  /// Answers the next send with this instead of storing it.
  pb.DmSendResult? answer;

  /// Answers each send it gives a result for with that instead of storing
  /// it (a CONFLICT for one id, say).
  pb.DmSendResult? Function(pb.DmSend send)? answerFor;

  /// Stores each send it says yes to, then loses the answer: the device
  /// hears nothing back (E11-9).
  bool Function(pb.DmSend send)? loseAnswer;
  var retryAfterMs = 0;

  final _log = <String, List<pb.DmEntry>>{};
  final _receipts = <String, pb.DmReceipt>{};
  final _sentBytes = <String, String>{};
  final _taken = <String>{};
  final _pulled = <String, int>{};
  var _following = false;

  @override
  Stream<pb.DmFeed> get feeds => _feeds.stream;

  @override
  Stream<pb.DmHome> get homes => _homes.stream;

  /// [group]'s log here.
  List<pb.DmEntry> logOf(String group) => _log[group] ?? const [];

  pb.DmEntry _keep(
    String group,
    List<int> bytes, {
    bool fromMe = false,
    String clientId = '',
  }) {
    final body = readFake(dmEnvelopeOf(bytes));
    final log = _log.putIfAbsent(group, () => []);
    final entry = pb.DmEntry(
      groupId: dmUnb64(group),
      seq: dmSeq(dmHomeSeqBase + log.length + 1),
      kind: body['t'] == 'commit'
          ? pb.DmEntryKind.DM_ENTRY_KIND_COMMIT
          : pb.DmEntryKind.DM_ENTRY_KIND_APPLICATION,
      epoch: Int64((body['epoch'] as int?) ?? 0),
      envelope: bytes,
      acceptedMs: Int64(log.length + 1),
      fromMe: fromMe,
      clientMessageId: clientId,
    );
    log.add(entry);
    _taken.add(dmB64(bytes));
    return entry;
  }

  void _push(String group, List<pb.DmEntry> entries) {
    if (!_following) return;
    _feeds.add(pb.DmFeed(entries: entries, groups: [_state(group)]));
  }

  pb.DmGroupState _state(String group) => pb.DmGroupState(
    groupId: dmUnb64(group),
    lastSeq: dmSeq(dmHomeSeqBase + logOf(group).length),
    behind: letGo.contains(group),
  );

  /// Groups this home says it let go of past the follower's cursor, until
  /// it follows from within what the home has (row 582).
  final letGo = <String>{};

  /// Says what it has of [group] now, with nothing new.
  void sayState(String group) => _push(group, const []);

  @override
  Future<pb.DmReceipt> send(pb.DmSend send) async {
    if (!connected.value) throw StateError('not connected');
    sends.add(send);
    final group = dmB64(send.groupId);
    pb.DmReceipt receipt(pb.DmSendResult result, [int seq = 0]) => pb.DmReceipt(
      groupId: send.groupId,
      clientMessageId: send.clientMessageId,
      seq: dmSeq(seq),
      result: result,
      retryAfterMs: Int64(retryAfterMs),
    );
    if (answer case final once?) {
      answer = null;
      return receipt(once);
    }
    if (answerFor?.call(send) case final result?) return receipt(result);
    if (!isHome) return receipt(pb.DmSendResult.DM_SEND_RESULT_NOT_HOME);
    final id = send.clientMessageId;
    if (_receipts[id] case final first? when id.isNotEmpty) {
      // The same id with other bytes is a conflict, never a second message
      // (the home's own rule).
      if (_sentBytes[id] != dmB64(send.envelope)) {
        return receipt(pb.DmSendResult.DM_SEND_RESULT_CONFLICT);
      }
      return first.deepCopy()
        ..result = pb.DmSendResult.DM_SEND_RESULT_DUPLICATE;
    }
    final entry = _keep(group, send.envelope, fromMe: true, clientId: id);
    // On to the friend: through their home, as the worker stands in here.
    if (entry.kind == pb.DmEntryKind.DM_ENTRY_KIND_APPLICATION) {
      worker.deliver(group, dmEnvelopeOf(send.envelope));
      _pulled[group] = worker.mailboxOf(group).length;
    }
    final stored = receipt(
      pb.DmSendResult.DM_SEND_RESULT_STORED,
      entry.seq.toInt(),
    );
    if (id.isNotEmpty) {
      _receipts[id] = stored;
      _sentBytes[id] = dmB64(send.envelope);
    }
    _push(group, [entry]);
    if (loseAnswer?.call(send) ?? false) throw StateError('answer lost');
    return stored;
  }

  /// What's at the worker for [group] so far never comes here: a commit
  /// no one handed this home (row 551).
  void passOver(String group) =>
      _pulled[group] = worker.mailboxOf(group).length;

  /// What others said in [group] at the worker comes in, as from the
  /// friend's home or claktory.com; what came from here is there already.
  void pull(String group) {
    final mailbox = worker.mailboxOf(group);
    final entries = <pb.DmEntry>[];
    for (final item in mailbox.skip(_pulled[group] ?? 0)) {
      // Proposals stay at the worker (Mista 32950).
      if (item['kind'] == 'proposal') continue;
      final bytes = dmBytesOf(jsonEncode(item['envelope']));
      if (_taken.contains(dmB64(bytes))) continue;
      entries.add(_keep(group, bytes));
    }
    _pulled[group] = mailbox.length;
    if (entries.isNotEmpty) _push(group, entries);
  }

  /// How the person's message [clientId] is getting on past here.
  void deliver(
    String group,
    String clientId,
    pb.DmDeliveryState state, {
    int atMs = 0,
    int epoch = 0,
    String account = '',
  }) => _feeds.add(
    pb.DmFeed(
      deliveries: [
        pb.DmDelivery(
          groupId: dmUnb64(group),
          clientMessageId: clientId,
          state: state,
          atMs: Int64(atMs),
          epoch: Int64(epoch),
          accountId: account,
        ),
      ],
    ),
  );

  @override
  Future<void> follow(pb.DmFollow follow) async {
    if (!connected.value) throw StateError('not connected');
    follows.add(follow);
    _following = true;
    final after = {
      for (final cursor in follow.cursors)
        dmB64(cursor.groupId): cursor.seq.toInt(),
    };
    for (final MapEntry(:key, :value) in after.entries) {
      if (value <= dmHomeSeqBase + logOf(key).length) letGo.remove(key);
    }
    for (final MapEntry(:key, :value) in _log.entries) {
      final from = after[key] ?? 0;
      _push(key, [
        for (final entry in value)
          if (entry.seq.toInt() > from) entry,
      ]);
    }
  }

  @override
  Future<void> ack(pb.DmAck ack) async => acks.add(ack);

  pb.DmHome get state => pb.DmHome(
    isHome: isHome,
    endpointId: endpointId,
    peers: peers,
    homeSinceMs: Int64(homeSinceMs),
  );

  @override
  Future<pb.DmHome> routes(pb.DmRoutes routes) async {
    if (!connected.value) throw StateError('not connected');
    routesSaid.add(routes.routes);
    final answered = state;
    await holdRoutes?.future;
    return answered;
  }

  /// While set, its answers to routes wait for it: a slow connection.
  Completer<void>? holdRoutes;

  /// The home's state changed (a friend's home went out of reach).
  void said() => _homes.add(state);
}
