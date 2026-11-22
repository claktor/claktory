import 'dart:convert';
import 'dart:typed_data';

import 'package:fixnum/fixnum.dart';

import 'package:factory_app/device_account.dart';
import 'package:factory_app/people/dm_keys.dart';
import 'package:factory_app/people/e2ee.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart'
    show DmLeafOffer;
import 'package:factory_app/tunnel/api/dm.dart';
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
    return switch (kept[name]) {
      final bytes? => Uint8List.fromList(bytes),
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
      DmMember(
        account: member.split('/').first,
        leafId: member.split('/').last,
        kind: claktoryOf(member.split('/').last) == null
            ? DmLeafKind.device
            : DmLeafKind.claktory,
        name: member.split('/').last,
      ),
  ];

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
    'extra': content.extraJson,
  };

  static DmContent _fromJson(Map<String, Object?> json) => DmContent(
    kind: json['kind']! as String,
    clientMessageId: json['id']! as String,
    text: json['text'] as String?,
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
    here.seq = seq;
    final body = readFake(envelope);
    if (body['t'] == 'commit') {
      if (envelope == here.pending) {
        here
          ..epoch = here.pendingEpoch
          ..members = here.pendingMembers
          ..pending = null;
        return out(DmOutcome.committed);
      }
      if (body['epoch'] != here.epoch) {
        return out(DmOutcome.dropped, handshake: true);
      }
      here
        ..epoch += 1
        ..members = [for (final m in body['members']! as List) m as String];
      return out(DmOutcome.committed);
    }
    final from = body['from']! as String;
    final own = _outgoing.entries
        .where((each) => each.value.envelope == envelope)
        .firstOrNull;
    if (own == null && body['epoch'] != here.epoch) {
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
    } else {
      _history.putIfAbsent(id, () => []).add(message);
    }
    return out(DmOutcome.message, message: message);
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

  @override
  void dispose() {}

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

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
    var n = 0;
    final call = worker.as(account, leaf);
    e2ee = E2ee(
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
      newSecret: () async => Uint8List.fromList(List.filled(32, ++n)),
      removeStore: (_) async {},
      whoAmI: () async =>
          (accountId: account, name: name ?? leaf, deviceKey: leaf),
      nowMs: () => 1000,
      newId: () => '$leaf-${++_ids}',
    );
  }

  final FakeWorker worker;
  final String account;
  final String leaf;
  final FakeDmKeys keys;
  late final FakeDmDevice device;
  late final E2ee e2ee;
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
