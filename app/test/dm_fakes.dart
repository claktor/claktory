import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:fixnum/fixnum.dart';

import 'package:factory_app/device_account.dart';
import 'package:factory_app/people/dm_keys.dart';
import 'package:factory_app/people/e2ee.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart'
    show DmLeafOffer;
import 'package:factory_app/tunnel/api/dm.dart';
import 'package:factory_app/tunnel/api/dm_transfer.dart';
import 'package:factory_app/user_apps/session.dart';

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
  final packages = <String, List<String>>{};
  final lastResort = <String, String>{};
  final _groups = <String, _Group>{};
  final welcomes = <String, List<Map<String, Object?>>>{};
  final handoffs = <String, List<Map<String, Object?>>>{};

  /// The account's live grants, as GET /v1/apps/grants lists them.
  final grants = <Map<String, Object?>>[];
  final log = <String>[];

  /// The devices signed out of each account by another of its devices
  /// (POST /v1/devices/remove): the service answers them 401 since.
  final signedOut = <String, Set<String>>{};

  /// Each account's security keys before the one it has now: the worker
  /// takes none of them again (POST me/keys/rotate).
  final retired = <String, Set<String>>{};

  /// Makes the next message post to a group answer 409 epoch, once.
  var refuseNextAsStale = false;

  /// Answers [route] ("POST conversations/v1/messages") with what it
  /// returns instead, when that's an exception: the service unreachable,
  /// or refusing.
  Object? Function(String route)? refuse;
  var _welcomeId = 0;

  int epochOf(String group) => _groups[group]!.epoch;
  List<Map<String, Object?>> mailboxOf(String group) => _groups[group]!.mailbox;

  /// Adds [envelope] to [group]'s mailbox as another member would; a
  /// commit moves the group's epoch unless not [advance].
  int deliver(String group, String envelope, {bool advance = true}) {
    final body = readFake(envelope);
    final known = _groups[group]!;
    final seq = known.mailbox.length + 1;
    known.mailbox.add({
      'seq': seq,
      'kind': body['t'] == 'commit' ? 'commit' : 'application',
      'envelope': jsonDecode(envelope),
      'accepted_ms': seq,
      'epoch': body['epoch'],
    });
    if (body['t'] == 'commit' && advance) known.epoch++;
    return seq;
  }

  SignedCall as(String account, String leaf) => (method, path, [body]) async {
    final uri = Uri.parse(path);
    final route = '$method ${uri.path.replaceFirst('/v1/apps/dms/', '')}';
    log.add(route);
    if (refuse?.call(route) case final Exception error) throw error;
    if (signedOut[account]?.contains(leaf) ?? false) {
      _refuse(401, 'unauthorized');
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
        return keys[who] ?? _refuse(404, 'not_found');
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
        return keys[account] = {
          'account_id': account,
          'identity_key': identity,
          'device_list': body['device_list'],
          'version': body['version'],
        };
      case ('POST', ['me', 'keys', 'rotate']):
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
        }
        return {'remaining': mine.length, 'last_resort': true};
      case ('GET', ['key-packages']):
        return {'remaining': packages['$account/$leaf']?.length ?? 0};
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
  Future<void> delete(DmKeyName name) async => kept.remove(name);

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
      leaves: [_listed(leafId, leafKey)],
    );
    return DmSetUp(
      identityPublic: identity!,
      certificate: Uint8List(0),
      deviceList: _listBytes(lists[accountId]!),
    );
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
        'leaves': [
          for (final leaf in list.leaves)
            {'leaf': leaf.leafId, 'key': dmB64(leaf.signatureKey)},
        ],
      }),
    ),
  );

  @override
  DmDeviceList? deviceList({required String account}) => lists[account];

  @override
  Future<Uint8List> signDeviceList({
    required BigInt version,
    required BigInt createdMs,
    required List<DmListed> leaves,
  }) async {
    final list = lists[account] = DmDeviceList(
      version: version,
      leaves: leaves,
    );
    return _listBytes(list);
  }

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
    return DmHandoffSealed(
      sealed: Uint8List.fromList(
        utf8.encode(
          jsonEncode({
            'account': account,
            'identity': dmB64(identity!),
            'for': leaf,
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
  Future<DmHandoffDone> finishHandoff({
    required DmHandoff handoff,
    required List<int> sealed,
    required BigInt nowMs,
  }) async {
    final offer = jsonDecode(handoff.qr()) as Map<String, Object?>;
    final body = jsonDecode(utf8.decode(sealed)) as Map<String, Object?>;
    if (body['for'] != offer['leaf']) throw StateError("doesn't open");
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

  DmPin _pin(Uint8List key, {bool changed = false, bool verified = false}) =>
      DmPin(
        identityPublic: key,
        verified: verified,
        changedUnseen: changed,
        canSend: !changed,
        deviceListVersion: BigInt.one,
      );

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
  }) async =>
      pins[account] = _pin(Uint8List.fromList(identityPublic), changed: true);

  @override
  Future<void> seen({required String account}) async =>
      pins[account] = _pin(pins[account]!.identityPublic);

  @override
  Future<void> verify({required String account}) async =>
      pins[account] = _pin(pins[account]!.identityPublic, verified: true);

  @override
  Future<void> acceptList({
    required String account,
    required List<int> deviceList,
  }) async {
    final json = jsonDecode(utf8.decode(deviceList)) as Map<String, Object?>;
    lists[account] = DmDeviceList(
      version: BigInt.from(json['version']! as int),
      leaves: [
        for (final leaf in json['leaves']! as List)
          _listed(
            (leaf as Map)['leaf']! as String,
            dmUnb64(leaf['key']! as String),
          ),
      ],
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

  @override
  Future<DmCommit> rejoinGroup({
    required List<int> groupId,
    required DmGroupKind kind,
    required String groupInfo,
    required int seq,
  }) async {
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
  Future<DmCommit> addMembers({
    required List<int> groupId,
    required List<String> keyPackages,
  }) async {
    final id = dmB64(groupId);
    final here = _groups[id]!;
    final added = [
      for (final package in keyPackages) readFake(package)['member']! as String,
    ];
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
    if (here.kind.kind == DmGroupType.conversation) {
      for (final account in here.kind.accounts) {
        if (pins[account]?.canSend == false) throw StateError('key changed');
      }
    }
    if (_outgoing[content.clientMessageId] case final kept?) {
      return kept.envelope;
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
    }) {
      final processed = DmProcessed(
        outcome: outcome,
        message: message,
        epoch: BigInt.from(here.epoch),
        why: '',
        handshake: handshake,
      );
      this.processed.add(processed);
      return processed;
    }

    if (seq <= here.seq) return out(DmOutcome.already);
    final before = here.seq;
    here.seq = seq;
    final body = readFake(envelope);
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
    } else {
      _history.putIfAbsent(id, () => []).add(message);
    }
    return out(DmOutcome.message, message: message);
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
    lists[account] = DmDeviceList(
      version: BigInt.from(json['version']! as int),
      leaves: [
        for (final each in json['leaves']! as List)
          _listed(
            (each as Map)['leaf']! as String,
            dmUnb64(each['key']! as String),
          ),
      ],
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
  Future<void> beginRemoval({required DmRemoval removal}) async =>
      removalRow = removal;

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

  @override
  Future<List<String>> leafIds({required List<int> groupId}) async => [
    for (final m in _groups[dmB64(groupId)]?.members ?? const <String>[])
      m.split('/').last,
  ];

  final _keyLines = <String>{};

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
    _make = () => E2ee(
      session: UserAppsSession(
        DeviceAccount(key: 'k', email: '$account@example.com'),
        nonce: () => 'n',
        call: route?.call(call) ?? call,
      ),
      keys: keys,
      open: ({
        required String path,
        required List<int> storeKey,
        required List<int> leafKey,
        Uint8List? identityKey,
      }) async => device,
      newSecret: () async => Uint8List.fromList(List.filled(32, ++_secrets)),
      removeStore: (_) async {},
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
    );
    e2ee = _make();
  }

  /// The app quit and opened again: a new engine over the same key store
  /// and store, as a launch with keys already kept finds them.
  E2ee relaunch() {
    e2ee.dispose();
    return e2ee = _make();
  }

  late final E2ee Function() _make;
  var _secrets = 0;

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
