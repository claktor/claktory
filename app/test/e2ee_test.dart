import 'dart:convert';

import 'package:factory_app/people/dm_keys.dart';
import 'package:factory_app/people/e2ee.dart';
import 'package:factory_app/tunnel/api/dm.dart';
import 'package:flutter_test/flutter_test.dart';

import 'dm_fakes.dart';

// E5's engine against E2's contract (claktory-user-apps README, "End to
// end"): setting up, Welcomes and external joins, feeds in order, each
// message encrypted once and sent again after 409 epoch, and the self
// group's drafts and read positions.

DmContent _text(String id, String text) =>
    DmContent(kind: 'text', clientMessageId: id, text: text, extraJson: '');

Future<FakeEndpoint> _ready(
  FakeWorker worker,
  String account,
  String leaf,
) async {
  final endpoint = FakeEndpoint(worker, account, leaf);
  await endpoint.e2ee.start();
  expect(endpoint.e2ee.setup, DmSetup.firstDevice);
  await endpoint.e2ee.setUpFirst();
  expect(endpoint.e2ee.setup, DmSetup.ready);
  return endpoint;
}

void main() {
  test("a device that can't keep keys has messages off", () async {
    final endpoint = FakeEndpoint(FakeWorker(), 'matt', 'phone');
    endpoint.keys.kept.clear();
    final e2ee = E2ee(
      session: endpoint.e2ee.session,
      keys: FakeDmKeys(isAvailable: false),
      whoAmI: () async => (accountId: 'matt', name: 'iPhone', deviceKey: 'p'),
    );
    await e2ee.start();
    expect(e2ee.setup, DmSetup.unavailable);
  });

  test(
    'the first device makes its keys, its list and the self group',
    () async {
      final worker = FakeWorker();
      final phone = await _ready(worker, 'matt', 'phone');
      // The identity was kept before anything went to the worker.
      expect(phone.keys.kept.keys, containsAll(DmKeyName.values));
      expect(worker.keys['matt']!['version'], 1);
      expect(worker.packages['matt/phone'], hasLength(dmKeyPackages));
      expect(worker.lastResort['matt/phone'], isNotNull);
      expect(
        phone.e2ee.groups.values.map((each) => each.kind),
        contains('self'),
      );

      // Another device of Matt's starts: it's added from the phone.
      final ipad = FakeEndpoint(worker, 'matt', 'ipad');
      await ipad.e2ee.start();
      expect(ipad.e2ee.setup, DmSetup.addThisDevice);
    },
  );

  test('an unreadable key fails set-up instead of making a new one', () async {
    final endpoint = FakeEndpoint(FakeWorker(), 'matt', 'phone');
    endpoint.keys.unreadable.add(DmKeyName.leaf);
    await endpoint.e2ee.start();
    expect(endpoint.e2ee.setup, DmSetup.failed);
    expect(endpoint.e2ee.failure, DmFailure.unreadableKey);
    expect(endpoint.keys.kept.containsKey(DmKeyName.leaf), isFalse);
  });

  test(
    'two people talk: a Welcome, then each message once, in order',
    () async {
      final worker = FakeWorker();
      final matt = await _ready(worker, 'matt', 'phone');
      final ana = await _ready(worker, 'ana', 'pixel');

      final group = (await matt.e2ee.conversationGroup('v1', 3, 'ana'))!;
      expect(matt.device.pin(account: 'ana'), isNotNull);
      // Ana's device comes in by its Welcome, after the Add's seq.
      await ana.e2ee.sync();
      expect(ana.e2ee.groups[group.groupId]!.conversationId, 'v1');
      expect(worker.welcomes['pixel'], isEmpty);
      expect(ana.device.pin(account: 'matt'), isNotNull);

      await matt.e2ee.say(group.groupId, _text('m1', 'hi Ana'));
      expect(matt.e2ee.sending['m1']!.state, DmSendState.sent);
      await ana.e2ee.sync();
      final heard = await ana.e2ee.history(group.groupId);
      expect(heard.single.content.text, 'hi Ana');
      expect(heard.single.mine, isFalse);

      // It comes back to Matt as his own, and stops waiting.
      await matt.e2ee.sync();
      final mine = await matt.e2ee.history(group.groupId);
      expect(mine.single.mine, isTrue);
      expect(matt.e2ee.sending, isNot(contains('m1')));
      expect(await matt.e2ee.unsent(group.groupId), isEmpty);

      // Synced again, nothing is taken twice.
      await ana.e2ee.sync();
      expect(await ana.e2ee.history(group.groupId), hasLength(1));
    },
  );

  test('after 409 epoch a message goes again as a new one', () async {
    final worker = FakeWorker();
    final matt = await _ready(worker, 'matt', 'phone');
    await _ready(worker, 'ana', 'pixel');
    final group = (await matt.e2ee.conversationGroup('v1', 3, 'ana'))!;

    worker.refuseNextAsStale = true;
    await matt.e2ee.say(group.groupId, _text('m1', 'again'));
    final now = matt.e2ee.current('m1');
    expect(now, isNot('m1'));
    expect(matt.e2ee.sending[now]!.state, DmSendState.sent);
    expect(
      worker.mailboxOf(group.groupId).where((m) => m['kind'] == 'application'),
      hasLength(1),
    );
  });

  test('an unknown outcome is tried again with the same bytes', () async {
    final worker = FakeWorker();
    final matt = await _ready(worker, 'matt', 'phone');
    await _ready(worker, 'ana', 'pixel');
    final group = (await matt.e2ee.conversationGroup('v1', 3, 'ana'))!;
    await matt.device.send(
      groupId: dmUnb64(group.groupId),
      content: _text('m1', 'kept'),
      nowMs: 1,
    );
    // As after a restart: what was encrypted and not sent goes now.
    await matt.e2ee.sync();
    expect(matt.e2ee.sending['m1']!.state, DmSendState.sent);
    await matt.e2ee.retry('m1');
    expect(
      worker.mailboxOf(group.groupId).where((m) => m['kind'] == 'application'),
      hasLength(1),
    );
  });

  test('a key change holds sending until the person has seen it', () async {
    final worker = FakeWorker();
    final matt = await _ready(worker, 'matt', 'phone');
    await _ready(worker, 'ana', 'pixel');
    final group = (await matt.e2ee.conversationGroup('v1', 3, 'ana'))!;
    worker.keys['ana']!['identity_key'] = dmB64(List.filled(32, 9));
    await matt.e2ee.refreshKeys('ana');
    expect(matt.e2ee.pin('ana')!.changedUnseen, isTrue);
    await matt.e2ee.say(group.groupId, _text('m1', 'held'));
    expect(matt.e2ee.sending['m1']!.state, DmSendState.refused);
    await matt.e2ee.seen('ana');
    await matt.e2ee.say(group.groupId, _text('m2', 'now'));
    expect(matt.e2ee.sending['m2']!.state, DmSendState.sent);
  });

  test('a device out of step joins again by external commit', () async {
    final worker = FakeWorker();
    final matt = await _ready(worker, 'matt', 'phone');
    final ana = await _ready(worker, 'ana', 'pixel');
    final group = (await matt.e2ee.conversationGroup('v1', 3, 'ana'))!;
    await ana.e2ee.sync();
    // A commit Ana's device can't take, at its own epoch: it's out of
    // step.
    worker.deliver(
      group.groupId,
      fakeEnvelope({
        't': 'commit',
        'g': group.groupId,
        'epoch': 5,
        'from': 'matt/phone',
        'members': ['matt/phone'],
      }),
      advance: false,
    );
    await ana.e2ee.sync();
    expect(ana.device.processed.where((p) => p.handshake), hasLength(1));
    // Its own external commit came back and was merged.
    expect(ana.device.processed.last.outcome, DmOutcome.committed);
    expect(worker.log, contains('GET groups/${group.groupId}/group-info'));
    expect(
      worker.mailboxOf(group.groupId).last['kind'],
      'commit',
      reason: 'its external commit went in',
    );
  });

  test("the person's drafts and reads reach their other devices", () async {
    final worker = FakeWorker();
    final matt = await _ready(worker, 'matt', 'phone');
    await _ready(worker, 'ana', 'pixel');
    final group = (await matt.e2ee.conversationGroup('v1', 3, 'ana'))!;
    await matt.e2ee.tellMyDevices('draft', group.groupId, text: 'half');
    await matt.e2ee.tellMyDevices('read', group.groupId, seq: 4);
    final view = await matt.e2ee.view(group.groupId);
    expect((view.draft, view.readSeq), ('half', 4));
    final self = matt.e2ee.groups.values.firstWhere((g) => g.kind == 'self');
    expect(
      worker.mailboxOf(self.groupId).where((m) => m['kind'] == 'application'),
      hasLength(2),
    );
    // Back in the feed, they count once and aren't history.
    await matt.e2ee.sync();
    expect(await matt.e2ee.history(self.groupId), isEmpty);
    expect(await matt.e2ee.unsent(self.groupId), isEmpty);
  });

  test('a new device is added by its code and joins every group', () async {
    final worker = FakeWorker();
    final phone = await _ready(worker, 'matt', 'phone');
    await _ready(worker, 'ana', 'pixel');
    final chat = (await phone.e2ee.conversationGroup('v1', 3, 'ana'))!;

    final ipad = FakeEndpoint(worker, 'matt', 'ipad', name: 'Studio iPad');
    await ipad.e2ee.start();
    expect(ipad.e2ee.setup, DmSetup.addThisDevice);
    final code = await ipad.e2ee.offerThisDevice(
      platform: 'iPad',
      endpointId: List.filled(32, 0),
    );
    expect(code, ipad.e2ee.offer);

    // The phone scans it, the person sees whose it is, and adds it.
    final found = await phone.e2ee.scanned(code!);
    expect((found.name, found.platform), ('Studio iPad', 'iPad'));
    await phone.e2ee.addDevice(found);
    expect(worker.keys['matt']!['version'], 2);
    expect(worker.handoffs['ipad'], hasLength(1));

    // The iPad takes it: the identity, then every group by external
    // commit, with its own key packages up.
    await ipad.e2ee.takeHandoff();
    expect(ipad.e2ee.setup, DmSetup.ready);
    expect(ipad.e2ee.offer, isNull);
    expect(ipad.keys.kept[DmKeyName.identity], isNotNull);
    expect(worker.handoffs['ipad'], isEmpty);
    expect(worker.packages['matt/ipad'], hasLength(dmKeyPackages));
    expect(ipad.e2ee.groups.keys, contains(chat.groupId));
    expect(ipad.e2ee.groups.values.map((each) => each.kind), contains('self'));

    // Both list both, this one first.
    await phone.e2ee.refreshKeys('matt');
    final devices = await phone.e2ee.devices();
    expect(devices.map((each) => (each.leafId, each.here)), [
      ('phone', true),
      ('ipad', false),
    ]);
  });

  test("a hand-off that doesn't open is dropped and the offer spent", () async {
    final worker = FakeWorker();
    await _ready(worker, 'matt', 'phone');
    final ipad = FakeEndpoint(worker, 'matt', 'ipad');
    await ipad.e2ee.start();
    await ipad.e2ee.offerThisDevice(platform: 'iPad', endpointId: [0]);
    worker.handoffs['ipad'] = [
      {
        'id': 'h1',
        'from': 'phone',
        'sealed': dmB64(utf8.encode('{"for":"x"}')),
      },
    ];
    await ipad.e2ee.takeHandoff();
    expect(ipad.e2ee.setup, DmSetup.addThisDevice);
    expect(ipad.e2ee.offer, isNull);
    expect(worker.handoffs['ipad'], isEmpty);
  });

  test('a removed device leaves the list and its groups', () async {
    final worker = FakeWorker();
    final phone = await _ready(worker, 'matt', 'phone');
    await _ready(worker, 'ana', 'pixel');
    final chat = (await phone.e2ee.conversationGroup('v1', 3, 'ana'))!;
    final ipad = FakeEndpoint(worker, 'matt', 'ipad');
    await ipad.e2ee.start();
    final code = await ipad.e2ee.offerThisDevice(
      platform: 'iPad',
      endpointId: [0],
    );
    await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
    await ipad.e2ee.takeHandoff();
    await phone.e2ee.sync();
    expect(
      (await phone.device.members(groupId: dmUnb64(chat.groupId)))
          .map((each) => each.leafId),
      contains('ipad'),
    );

    await phone.e2ee.removeDevice('ipad');
    expect(worker.keys['matt']!['version'], 3);
    expect((await phone.e2ee.devices()).map((each) => each.leafId), ['phone']);
    expect(
      (await phone.device.members(groupId: dmUnb64(chat.groupId)))
          .map((each) => each.leafId),
      isNot(contains('ipad')),
    );
  });

  group("the person's Claktory", () {
    test('is enrolled, listed and added to its channel', () async {
      final worker = FakeWorker();
      final phone = await _ready(worker, 'matt', 'phone');
      final mini = FakeClaktoryLink(worker, 'matt')..stock = false;
      phone.e2ee.claktory = mini;
      await phone.e2ee.sync();
      // Certified, listed, enrolled with the pins.
      expect(phone.device.issued, ['mini#1']);
      expect(mini.enrollments, hasLength(1));
      expect(phone.device.lists['matt']!.leaves.map((each) => each.leafId), [
        'phone',
        'mini#1',
      ]);
      expect(worker.keys['matt']!['version'], 2);
      final channel = phone.e2ee.groups.values.singleWhere(
        (each) => each.kind == 'channel',
      );
      expect(channel.claktoryId, 'mini');
      // Its key packages aren't up yet: added once they are.
      expect(await _leaves(phone, channel.groupId), ['phone']);
      mini.stockUp();
      await phone.e2ee.sync();
      expect(await _leaves(phone, channel.groupId), ['phone', 'mini#1']);
      // Done: not enrolled or listed again.
      await phone.e2ee.sync();
      expect(mini.enrollments, hasLength(1));
      expect(worker.keys['matt']!['version'], 2);
    });

    test('joins a conversation with a grant, and leaves without', () async {
      final worker = FakeWorker();
      final phone = await _ready(worker, 'matt', 'phone');
      await _ready(worker, 'ana', 'pixel');
      phone.e2ee.claktory = FakeClaktoryLink(worker, 'matt');
      final chat = (await phone.e2ee.conversationGroup('v1', 3, 'ana'))!;
      await phone.e2ee.sync();
      // No grant: only the two people's devices.
      expect(await _leaves(phone, chat.groupId), ['phone', 'pixel']);

      worker.grants.add({
        'id': 'g1',
        'app': 'dms',
        'claktory_id': 'mini',
        'agent': {'id': 'june', 'incarnation': 'i1'},
        'scopes': ['dm.read'],
        'resources': [
          {'kind': 'conversation', 'id': 'v1'},
        ],
      });
      await phone.e2ee.grantsChanged();
      expect(await _leaves(phone, chat.groupId), ['phone', 'pixel', 'mini#1']);
      // Its pins went to its channel first.
      final channel = phone.e2ee.groups.values.singleWhere(
        (each) => each.kind == 'channel',
      );
      expect(
        worker
            .mailboxOf(channel.groupId)
            .where((m) => m['kind'] == 'application'),
        isNotEmpty,
      );

      // Taken back: out at the next sync, before anything else.
      worker.grants.clear();
      await phone.e2ee.grantsChanged();
      expect(await _leaves(phone, chat.groupId), ['phone', 'pixel']);
    });

    test("isn't added while a friend's key change is unseen", () async {
      final worker = FakeWorker();
      final phone = await _ready(worker, 'matt', 'phone');
      await _ready(worker, 'ana', 'pixel');
      phone.e2ee.claktory = FakeClaktoryLink(worker, 'matt');
      final chat = (await phone.e2ee.conversationGroup('v1', 3, 'ana'))!;
      worker.keys['ana']!['identity_key'] = dmB64(List.filled(32, 9));
      await phone.e2ee.refreshKeys('ana');
      worker.grants.add({
        'app': 'dms',
        'claktory_id': 'mini',
        'resources': [
          {'kind': 'conversation', 'id': 'v1'},
        ],
      });
      await phone.e2ee.grantsChanged();
      expect(await _leaves(phone, chat.groupId), ['phone', 'pixel']);
      await phone.e2ee.seen('ana');
      await phone.e2ee.grantsChanged();
      expect(await _leaves(phone, chat.groupId), ['phone', 'pixel', 'mini#1']);
    });
  });
}

Future<List<String>> _leaves(FakeEndpoint endpoint, String group) async => [
  for (final member in await endpoint.device.members(groupId: dmUnb64(group)))
    member.leafId,
];
