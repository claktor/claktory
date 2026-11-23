import 'dart:async';
import 'dart:convert';

import 'package:factory_app/device_account.dart';
import 'package:factory_app/people/dm_keys.dart';
import 'package:factory_app/people/e2ee.dart';
import 'package:factory_app/tunnel/api/dm.dart';
import 'package:factory_app/tunnel/api/dm_transfer.dart';
import 'package:factory_app/user_apps/session.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart'
    show AnyhowException;
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
  test('messages start over only when a key this device had changes', () {
    expect(startsOver('a' * 64, 'b' * 64), isTrue);
    expect(startsOver('', 'b' * 64), isFalse, reason: 'a first key');
    expect(startsOver('a' * 64, 'a' * 64), isFalse);
  });

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
      expect(phone.keys.kept.keys, {
        DmKeyName.store,
        DmKeyName.leaf,
        DmKeyName.identity,
      });
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

  test('opened again over keys it kept: ready, its chat goes on', () async {
    // The key store hands keys over as views nothing may change, as the
    // platform channels do (Abbacchio: zeroing one threw at every launch
    // after the first, and messages never started).
    final worker = FakeWorker();
    final matt = await _ready(worker, 'matt', 'phone');
    final ana = await _ready(worker, 'ana', 'pixel');
    final group = (await matt.e2ee.conversationGroup('v1', 3, 'ana'))!;
    await ana.e2ee.sync();
    await matt.e2ee.say(group.groupId, _text('m1', 'before'));

    final again = matt.relaunch();
    await again.start();
    expect(again.setup, DmSetup.ready);
    expect(again.failure, DmFailure.none);
    await again.say(group.groupId, _text('m2', 'after'));
    expect(again.sending['m2']!.state, DmSendState.sent);
    await ana.e2ee.sync();
    expect((await ana.e2ee.history(group.groupId)).map((m) => m.content.text), [
      'before',
      'after',
    ]);
  });

  test('an unreadable key fails set-up instead of making a new one', () async {
    final endpoint = FakeEndpoint(FakeWorker(), 'matt', 'phone');
    // Its store is here: the key was this install's own (row 202).
    endpoint.storeKept = true;
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

  test(
    'a reinstall over keys the keychain kept is a new device here: new '
    'leaf and store keys, and no identity until a hand-off (row 202)',
    () async {
      final worker = FakeWorker();
      final phone = await _ready(worker, 'matt', 'phone');
      final ipad = FakeEndpoint(worker, 'matt', 'ipad');
      await ipad.e2ee.start();
      final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
      await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
      await ipad.e2ee.takeHandoff();
      expect(ipad.e2ee.setup, DmSetup.ready);
      Map<DmKeyName, List<int>> keptNow() => {
        for (final MapEntry(:key, :value) in ipad.keys.kept.entries)
          key: [...value],
      };
      final before = keptNow();
      expect(before.keys, containsAll([DmKeyName.leaf, DmKeyName.store]));
      expect(before[DmKeyName.identity], isNotNull);

      // Opened again with its store: the same keys.
      await ipad.relaunch().start();
      expect(ipad.e2ee.setup, DmSetup.ready);
      expect(keptNow(), before);

      // Deleted and installed again: the keychain kept its keys, the store
      // went. A new leaf key (so a new signature key under the same leaf
      // id), a new store key, and the identity gone.
      await ipad.reinstall().start();
      expect(ipad.e2ee.setup, DmSetup.addThisDevice);
      final after = keptNow();
      expect(after[DmKeyName.leaf], isNot(before[DmKeyName.leaf]));
      expect(after[DmKeyName.store], isNot(before[DmKeyName.store]));
      expect(after.containsKey(DmKeyName.identity), isFalse);
      expect(after.containsKey(DmKeyName.identityNext), isFalse);

      // Added again from the phone: the identity comes back by hand-off.
      final again = await ipad.e2ee.offerThisDevice(platform: 'iPad');
      await phone.e2ee.addDevice(await phone.e2ee.scanned(again!));
      await ipad.e2ee.takeHandoff();
      expect(ipad.e2ee.setup, DmSetup.ready);
      expect(ipad.keys.kept[DmKeyName.identity], isNotNull);
    },
  );

  test('a new device is added by its code and joins every group', () async {
    final worker = FakeWorker();
    final phone = await _ready(worker, 'matt', 'phone');
    await _ready(worker, 'ana', 'pixel');
    final chat = (await phone.e2ee.conversationGroup('v1', 3, 'ana'))!;

    final ipad = FakeEndpoint(worker, 'matt', 'ipad', name: 'Studio iPad');
    await ipad.e2ee.start();
    expect(ipad.e2ee.setup, DmSetup.addThisDevice);
    final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
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

  test('a new device kept out of a group by a device under the old key says '
      'so, and joins by itself once it can (row 611)', () async {
    final worker = FakeWorker();
    final phone = await _ready(worker, 'matt', 'phone');
    final ipad = FakeEndpoint(worker, 'matt', 'ipad');
    await ipad.e2ee.start();
    final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
    await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
    var refusing = true;
    ipad.device.refuseRejoin = () => refusing
        ? AnyhowException(
            "refused: matt's certificate is from a security key that was "
            'replaced',
          )
        : null;
    await ipad.e2ee.takeHandoff();
    expect(ipad.e2ee.setup, DmSetup.ready);
    expect(ipad.e2ee.waitingForOtherDevices, isTrue);
    // Another refusal says nothing of the kind.
    refusing = false;
    ipad.now += const Duration(hours: 1).inMilliseconds;
    await ipad.e2ee.sync();
    expect(ipad.e2ee.waitingForOtherDevices, isFalse);
    expect(ipad.e2ee.groups.values.map((each) => each.kind), contains('self'));
  });

  group('no other device (row 630)', () {
    test('a device with no keys makes a new security key for the account '
        'and is ready', () async {
      final worker = FakeWorker();
      final phone = await _ready(worker, 'matt', 'phone');
      final old = worker.keys['matt']!['identity_key'];
      // The phone is lost; a reinstalled Mac has nothing to be added from.
      final mac = FakeEndpoint(worker, 'matt', 'mac')..skipSecrets(100);
      await mac.e2ee.start();
      expect(mac.e2ee.setup, DmSetup.addThisDevice);
      expect(await mac.e2ee.startNewKey(), isNull, reason: mac.e2ee.problem);
      expect(mac.e2ee.setup, DmSetup.ready);
      expect(mac.e2ee.offer, isNull);
      expect(worker.keys['matt']!['identity_key'], isNot(old));
      expect(worker.keys['matt']!['identity_key'], dmB64(mac.device.identity!));
      expect(worker.retired['matt'], contains(old));
      // The old key held as retired here from the start.
      expect(dmB64(mac.device.replacedKey!), old);
      expect(mac.keys.kept[DmKeyName.identity], isNotNull);
      expect(mac.device.lists['matt']!.leaves.map((each) => each.leafId), [
        'mac',
      ]);
      expect(worker.packages['matt/mac'], hasLength(dmKeyPackages));
      // The phone's old leaf is out of the self group, in a commit of its
      // own, and listed apart; never signed out (Fugo).
      final mine = mac.e2ee.groups.values
          .singleWhere((each) => each.kind == 'self')
          .groupId;
      expect(await _leafIds(mac, mine), ['mac']);
      expect((await mac.device.replacedDevices()).map((each) => each.leafId), [
        'phone',
      ]);
      expect(mac.e2ee.waitingFor, isEmpty);
      expect(worker.signedOut['matt'], isNull);
      expect(worker.log, isNot(contains('POST /v1/devices/remove')));
      // The phone, if it's still around, learns its key was replaced
      // (the account's keys event, then a sync).
      await phone.e2ee.refreshKeys('matt');
      await phone.e2ee.sync();
      expect(phone.e2ee.failure, DmFailure.identityReplaced);
    });

    test('after more than one start-over, every key the account replaced '
        'is retired here too (row 654)', () async {
      final worker = FakeWorker();
      await _ready(worker, 'matt', 'phone');
      final served = worker.keys['matt']!['identity_key']! as String;
      final earlier = dmB64(List.filled(32, 61));
      worker.retired['matt'] = {earlier};
      final mac = FakeEndpoint(worker, 'matt', 'mac')..skipSecrets(100);
      await mac.e2ee.start();
      expect(await mac.e2ee.startNewKey(), isNull, reason: mac.e2ee.problem);
      expect(dmB64(mac.device.replacedKey!), served);
      expect(mac.device.replacedBefore.map(dmB64), [earlier]);
    });

    test('the old device is never signed out, even while its removal '
        "doesn't go through (Fugo)", () async {
      final worker = FakeWorker();
      await _ready(worker, 'matt', 'phone');
      final mac = FakeEndpoint(worker, 'matt', 'mac')..skipSecrets(100);
      await mac.e2ee.start();
      // The rejoin's commit goes through; every one after it doesn't.
      var commits = 0;
      worker.refuse = (route) =>
          route.startsWith('POST groups/') &&
              route.endsWith('/commits') &&
              ++commits > 1
          ? const AccountServiceUnreachable('x')
          : null;
      expect(await mac.e2ee.startNewKey(), isNull);
      for (var i = 0; i < 3; i++) {
        mac.now += const Duration(hours: 1).inMilliseconds;
        await mac.e2ee.sync();
      }
      final mine = mac.e2ee.groups.values
          .singleWhere((each) => each.kind == 'self')
          .groupId;
      expect(await _leafIds(mac, mine), contains('phone'));
      // Nothing is sent there meanwhile, and the phone stays signed in.
      expect(mac.e2ee.waitingFor[mine], 'matt');
      expect(worker.signedOut['matt'], isNull);
      expect(worker.log, isNot(contains('POST /v1/devices/remove')));
      // Then it goes through.
      worker.refuse = null;
      mac.now += const Duration(hours: 1).inMilliseconds;
      await mac.e2ee.sync();
      expect(await _leafIds(mac, mine), ['mac']);
      expect(mac.e2ee.waitingFor, isEmpty);
      expect(worker.signedOut['matt'], isNull);
    });

    test(
      'a sign-in too old is sent to sign in again, and nothing is kept',
      () async {
        final worker = FakeWorker();
        await _ready(worker, 'matt', 'phone');
        final old = worker.keys['matt']!['identity_key'];
        final mac = FakeEndpoint(worker, 'matt', 'mac')..skipSecrets(100);
        await mac.e2ee.start();
        worker.staleSignIn.add('matt/mac');
        expect(
          await mac.e2ee.startNewKey(),
          DmStartOverStop.signInAgain,
          reason: mac.e2ee.problem,
        );
        expect(mac.e2ee.setup, DmSetup.addThisDevice);
        expect(worker.keys['matt']!['identity_key'], old);
        expect(mac.keys.kept[DmKeyName.identity], isNull);
        // Signed in again: it goes through.
        worker.staleSignIn.clear();
        expect(await mac.e2ee.startNewKey(), isNull);
        expect(mac.e2ee.setup, DmSetup.ready);
        // What stopped the first try isn't left standing (row 640).
        expect(mac.e2ee.problem, isEmpty);
      },
    );

    test('this device is never removed as one that had the old key, and a '
        'removal of it kept by an older build clears (row 640)', () async {
      final worker = FakeWorker();
      final mac = await _ready(worker, 'matt', 'mac');
      await mac.device.beginRemoval(
        removal: const DmRemoval(
          remove: ['mac'],
          names: ['Mac'],
          keep: [],
          newKey: false,
        ),
      );
      mac.device.replaced.add(
        DmMember(
          account: 'matt',
          leafId: 'mac',
          kind: DmLeafKind.device,
          name: 'Mac',
          sinceMs: BigInt.zero,
        ),
      );
      expect(await mac.e2ee.finishRemoval(), isNull);
      expect(mac.e2ee.removal, isNull);
      expect(mac.device.replaced, isEmpty);
      expect(worker.signedOut['matt'], isNull);
      expect(worker.log, isNot(contains('POST /v1/devices/remove')));
    });
  });

  test("a hand-off that doesn't open is dropped and the offer spent", () async {
    final worker = FakeWorker();
    await _ready(worker, 'matt', 'phone');
    final ipad = FakeEndpoint(worker, 'matt', 'ipad');
    await ipad.e2ee.start();
    await ipad.e2ee.offerThisDevice(platform: 'iPad');
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
    final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
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
    // Out of every group by a commit, not only failing the checks once
    // off the list (Abbacchio's R5): no leaf of it is left.
    for (final group in phone.e2ee.groups.values) {
      expect(await _leafIds(phone, group.groupId), isNot(contains('ipad')));
    }
  });

  group('signing out of the Claktory account (row 671)', () {
    Future<(FakeEndpoint, FakeEndpoint, DmServerGroup)> twoOfMatts(
      FakeWorker worker,
    ) async {
      final phone = await _ready(worker, 'matt', 'phone');
      await _ready(worker, 'ana', 'pixel');
      final chat = (await phone.e2ee.conversationGroup('v1', 3, 'ana'))!;
      final mac = FakeEndpoint(worker, 'matt', 'mac');
      await mac.e2ee.start();
      final code = await mac.e2ee.offerThisDevice(platform: 'Mac');
      await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
      await mac.e2ee.takeHandoff();
      await phone.e2ee.sync();
      expect(mac.e2ee.setup, DmSetup.ready);
      return (phone, mac, chat);
    }

    test('with other devices: it leaves the list, its key packages go, the '
        'others take it out of every group, and everything here is '
        'deleted', () async {
      final worker = FakeWorker();
      final (phone, mac, chat) = await twoOfMatts(worker);
      expect(worker.packages['matt/mac'], isNotEmpty);
      expect(await mac.e2ee.signOutPlan(), DmSignOut.withOthers);
      expect(await phone.e2ee.signOutPlan(), DmSignOut.withOthers);

      await mac.e2ee.leave();
      expect(worker.keys['matt']!['version'], 3);
      expect(worker.packages.containsKey('matt/mac'), isFalse);
      expect(worker.lastResort.containsKey('matt/mac'), isFalse);
      expect(mac.keys.kept, isEmpty, reason: 'security key included');
      expect(mac.storeKept, isFalse);
      // It doesn't leave the groups itself: the others' sweep does.
      expect(await _leafIds(phone, chat.groupId), contains('mac'));
      await phone.e2ee.refreshKeys('matt');
      await phone.e2ee.sync();
      for (final group in phone.e2ee.groups.values) {
        expect(await _leafIds(phone, group.groupId), isNot(contains('mac')));
      }
      expect((await phone.e2ee.devices()).map((each) => each.leafId), [
        'phone',
      ]);
      expect(await phone.e2ee.signOutPlan(), DmSignOut.alone);
    });

    test("with other devices but the worker out of reach: deleted here all "
        'the same, the list as it was', () async {
      final worker = FakeWorker();
      final (_, mac, _) = await twoOfMatts(worker);
      worker.refuse = (route) => route == 'PUT me/keys'
          ? const AccountServiceUnreachable('offline')
          : null;
      await mac.e2ee.leave();
      expect(worker.keys['matt']!['version'], 2);
      expect(mac.keys.kept, isEmpty);
      expect(mac.storeKept, isFalse);
    });

    test('the only device: its messages stay, locked, and go on when it '
        'signs in to the same account again', () async {
      final worker = FakeWorker();
      final phone = await _ready(worker, 'matt', 'phone');
      final ana = await _ready(worker, 'ana', 'pixel');
      final chat = (await phone.e2ee.conversationGroup('v1', 3, 'ana'))!;
      await ana.e2ee.sync();
      await phone.e2ee.say(chat.groupId, _text('m1', 'before'));
      expect(await phone.e2ee.signOutPlan(), DmSignOut.alone);
      final kept = {...phone.keys.kept.keys};

      // Signed out (nothing here changes) and in again: the same store.
      await phone.relaunch().start();
      expect(phone.e2ee.setup, DmSetup.ready);
      expect(phone.keys.kept.keys, kept);
      await ana.e2ee.say(chat.groupId, _text('a1', 'after'));
      await phone.e2ee.sync();
      expect(
        (await phone.e2ee.history(chat.groupId)).map((m) => m.content.text),
        ['before', 'after'],
      );
    });

    test('Delete messages: everything here, the security key too; signed '
        'in again it has none, and the person starts over', () async {
      final worker = FakeWorker();
      final phone = await _ready(worker, 'matt', 'phone');
      await phone.e2ee.deleteHere();
      expect(phone.keys.kept, isEmpty);
      expect(phone.storeKept, isFalse);
      await phone.relaunch().start();
      expect(phone.e2ee.setup, DmSetup.addThisDevice);
      expect(phone.keys.kept.containsKey(DmKeyName.identity), isFalse);
    });

    test('nothing set up here: it only signs out', () async {
      final worker = FakeWorker();
      final mac = FakeEndpoint(worker, 'matt', 'mac');
      await mac.e2ee.start();
      expect(await mac.e2ee.signOutPlan(), DmSignOut.none);
    });

    test('a device that signed out shows as such to the others, and to '
        "nobody else's", () async {
      final worker = FakeWorker();
      final (phone, _, _) = await twoOfMatts(worker);
      final ana = FakeEndpoint(worker, 'ana', 'pixel');
      worker.accountSignOuts['matt'] = {'mac'};
      await phone.e2ee.readSignedOut();
      expect(phone.e2ee.signedOut, {'mac'});
      expect(
        (await ana.e2ee.routes.keys('matt'))!.signedOut,
        isEmpty,
        reason: 'only to its own devices',
      );
      worker.accountSignOuts['matt'] = {};
      await phone.e2ee.readSignedOut();
      expect(phone.e2ee.signedOut, isEmpty);
    });
  });

  group("a friend's first message (row 676)", () {
    /// Ana's Mac [a] and her second device [c] (added from [a]), and
    /// Matt's [b]; [route] wraps [a]'s worker calls.
    Future<(FakeEndpoint, FakeEndpoint, FakeEndpoint)> people(
      FakeWorker worker, {
      SignedCall Function(SignedCall worker)? route,
    }) async {
      final a = FakeEndpoint(worker, 'ana', 'a', route: route);
      await a.e2ee.start();
      await a.e2ee.setUpFirst();
      final c = FakeEndpoint(worker, 'ana', 'c');
      await c.e2ee.start();
      final code = await c.e2ee.offerThisDevice(platform: 'Mac');
      await a.e2ee.addDevice(await a.e2ee.scanned(code!));
      await c.e2ee.takeHandoff();
      await a.e2ee.sync();
      await c.e2ee.sync();
      final b = await _ready(worker, 'matt', 'b');
      return (a, c, b);
    }

    Future<List<String?>> texts(FakeEndpoint at, String group) async => [
      for (final message in await at.e2ee.history(group)) message.content.text,
    ];

    test('another of her devices joining first: the add is made again, '
        'and the first message reaches Matt', () async {
      final worker = FakeWorker();
      FakeEndpoint? other;
      var race = false;
      final (a, c, b) = await people(
        worker,
        route: (call) => (method, path, [body]) async {
          if (race && method == 'POST' && path.endsWith('/commits')) {
            // C sees the new group listed and joins it first (Abbacchio's
            // row 676): the add comes back 409 epoch.
            race = false;
            await other!.e2ee.sync();
          }
          return call(method, path, body);
        },
      );
      other = c;
      race = true;
      final chat = (await a.e2ee.conversationGroup('v1', 1, 'matt'))!;
      expect(race, isFalse, reason: 'the race was run');
      expect(await _leaves(a, chat.groupId), containsAll(['a', 'c', 'b']));
      expect(a.e2ee.joining, isEmpty);

      await b.e2ee.sync();
      await a.e2ee.say(chat.groupId, _text('m1', 'ana 1'));
      expect(a.e2ee.sending['m1']!.state, DmSendState.sent);
      await b.e2ee.sync();
      expect(await texts(b, chat.groupId), ['ana 1']);
    });

    test('with no key package of his: it waits, says so, sends nothing (the '
        'worker would have taken it), and goes once he is in', () async {
      final worker = FakeWorker();
      final (a, c, b) = await people(worker);
      worker.packages.remove('matt/b');
      worker.lastResort.remove('matt/b');
      final chat = (await a.e2ee.conversationGroup('v1', 1, 'matt'))!;
      expect(a.e2ee.joining, {chat.groupId: 'matt'});
      final posted = worker.log.length;
      await a.e2ee.say(chat.groupId, _text('m1', 'ana 1'));
      expect(a.e2ee.sending['m1']!.state, DmSendState.refused);
      expect(a.e2ee.sending['m1']!.problem, 'waitingForFriend');
      expect(
        worker.log.skip(posted).where((route) => route.contains('messages')),
        isEmpty,
        reason: 'nothing on the wire',
      );
      expect(await a.device.unsent(), isEmpty, reason: 'nothing encrypted');

      // Her other device leaves the add to the first of hers there.
      await c.e2ee.sync();
      final claims = worker.log.where((r) => r.contains('claim')).length;
      await c.e2ee.sync();
      expect(worker.log.where((r) => r.contains('claim')).length, claims);

      // His device puts packages up: the next sync of hers adds him.
      await b.e2ee.sync();
      a.now += FakeEndpoint.retryAfter.inMilliseconds * 2;
      await a.e2ee.sync();
      expect(a.e2ee.joining, isEmpty);
      await a.e2ee.say(chat.groupId, _text('m2', 'ana 1'));
      expect(a.e2ee.sending['m2']!.state, DmSendState.sent);
      await b.e2ee.sync();
      expect(await texts(b, chat.groupId), ['ana 1']);
    });

    test("a claimed package for a device not on his list is refused: "
        'nothing is added or sent, and the chat says it waits', () async {
      final worker = FakeWorker();
      final (a, _, _) = await people(
        worker,
        route: (call) => (method, path, [body]) async {
          if (path.endsWith('/key-packages/claim') &&
              body?['account_id'] == 'matt') {
            return {
              'key_packages': [
                {
                  'leaf_id': 'b',
                  'key_package': jsonDecode(
                    fakeEnvelope({'t': 'kp', 'member': 'matt/rogue', 'n': 1}),
                  ),
                },
              ],
            };
          }
          return call(method, path, body);
        },
      );
      final chat = (await a.e2ee.conversationGroup('v1', 1, 'matt'))!;
      expect(await _leaves(a, chat.groupId), isNot(contains('rogue')));
      expect(a.e2ee.joining, {chat.groupId: 'matt'});
      expect(a.e2ee.problem, contains("isn't on its account's list"));
      await a.e2ee.say(chat.groupId, _text('m1', 'ana 1'));
      expect(a.e2ee.sending['m1']!.problem, 'waitingForFriend');
    });

    test('the worker refusing it (409 waiting_for_friend): never sent, said '
        'as waiting', () async {
      final worker = FakeWorker();
      final (a, _, b) = await people(worker);
      final chat = (await a.e2ee.conversationGroup('v1', 1, 'matt'))!;
      await b.e2ee.sync();
      worker.refuse = (route) =>
          route.startsWith('POST') && route.endsWith('/messages')
          ? const AccountServiceError(409, 'waiting_for_friend', '', {})
          : null;
      await a.e2ee.say(chat.groupId, _text('m1', 'ana 1'));
      expect(a.e2ee.sending['m1']!.state, DmSendState.refused);
      expect(a.e2ee.sending['m1']!.problem, 'waitingForFriend');
      expect(await a.device.unsent(), isEmpty);
    });
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

    test('signed in to another account: its channel is refused, said, and '
        'not asked for again at every sync', () async {
      final worker = FakeWorker();
      var asked = 0;
      final phone = FakeEndpoint(
        worker,
        'matt',
        'phone',
        route: (call) => (method, path, [body]) {
          if (method == 'POST' &&
              path.endsWith('/dms/groups') &&
              body?['kind'] == 'channel') {
            asked++;
            throw AccountServiceError(404, 'not_found', 'not_found', const {});
          }
          return call(method, path, body);
        },
      );
      await phone.e2ee.start();
      await phone.e2ee.setUpFirst();
      phone.e2ee.claktory = FakeClaktoryLink(worker, 'matt');
      await phone.e2ee.sync();
      expect(asked, 1);
      expect(phone.e2ee.problem, contains('signed in to another account'));
      await phone.e2ee.sync();
      await phone.e2ee.sync();
      expect(asked, 1);
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

    test('one taken off the list and ungranted goes, though it no longer '
        'passes the checks', () async {
      final worker = FakeWorker();
      final phone = await _ready(worker, 'matt', 'phone');
      await _ready(worker, 'ana', 'pixel');
      phone.e2ee.claktory = FakeClaktoryLink(worker, 'matt');
      final chat = (await phone.e2ee.conversationGroup('v1', 3, 'ana'))!;
      worker.grants.add({
        'app': 'dms',
        'claktory_id': 'mini',
        'resources': [
          {'kind': 'conversation', 'id': 'v1'},
        ],
      });
      await phone.e2ee.grantsChanged();
      expect(await _leaves(phone, chat.groupId), contains('mini#1'));

      // Removed from the person's Claktories: off the list, so it fails
      // the checks here, and its grant gone.
      phone.e2ee.claktory = null;
      final listed = phone.device.deviceList(account: 'matt')!.leaves;
      // Signed and taken (pinned once the worker has it).
      await phone.device.acceptList(
        account: 'matt',
        deviceList: await phone.device.signDeviceList(
          version: BigInt.from(9),
          createdMs: BigInt.one,
          leaves: [
            for (final each in listed)
              if (each.leafId != 'mini#1') each,
          ],
        ),
      );
      expect(await _leaves(phone, chat.groupId), isNot(contains('mini#1')));
      expect(await _leafIds(phone, chat.groupId), contains('mini#1'));
      worker.grants.clear();
      await phone.e2ee.grantsChanged();
      expect(await _leafIds(phone, chat.groupId), ['phone', 'pixel']);
    });

    test(
      "a friend's Claktory in the chat is theirs: never taken out",
      () async {
        final worker = FakeWorker();
        final phone = await _ready(worker, 'matt', 'phone');
        final ana = await _ready(worker, 'ana', 'pixel');
        phone.e2ee.claktory = FakeClaktoryLink(worker, 'matt');
        ana.e2ee.claktory = FakeClaktoryLink(worker, 'ana', leafId: 'studio#1');
        final chat = (await phone.e2ee.conversationGroup('v1', 3, 'ana'))!;
        await ana.e2ee.sync();
        // Ana gives her Claktory the chat.
        worker.grants.add({
          'app': 'dms',
          'claktory_id': 'studio',
          'resources': [
            {'kind': 'conversation', 'id': 'v1'},
          ],
        });
        await ana.e2ee.grantsChanged();
        await ana.e2ee.sync();
        expect(await _leaves(ana, chat.groupId), contains('studio#1'));
        // Matt's device, with Ana's list naming it (so it passes there),
        // leaves it be.
        await phone.e2ee.refreshKeys('ana');
        await phone.e2ee.sync();
        expect(await _leaves(phone, chat.groupId), contains('studio#1'));
        await phone.e2ee.grantsChanged();
        expect(await _leafIds(phone, chat.groupId), contains('studio#1'));
      },
    );

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

  group('history from the device that added this one (E8)', () {
    // The phone adds the iPad; the iPad's code named its own endpoint.
    Future<(FakeEndpoint, FakeEndpoint)> added() async {
      final worker = FakeWorker();
      final phone = await _ready(worker, 'matt', 'phone');
      final ipad = FakeEndpoint(worker, 'matt', 'ipad');
      await ipad.e2ee.start();
      final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
      expect(ipad.transfers, hasLength(1));
      await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
      await pumpEventQueue();
      await ipad.e2ee.takeHandoff();
      return (phone, ipad);
    }

    DmTransferProgress at(int done, int total, [DmTransferEnd? end]) =>
        DmTransferProgress(chatsDone: done, chatsTotal: total, end: end);

    test('comes over, and the endpoint closes once it has', () async {
      final (phone, ipad) = await added();
      // The phone sent its history straight to the iPad, over an endpoint
      // of its own that it closed after.
      expect(phone.transfers.single.sent, ['ipad']);
      expect(phone.transfers.single.closed, isTrue);

      final receiver = ipad.transfers.single;
      expect(receiver.receiving, 1);
      expect(ipad.e2ee.transfer, at(0, 0));
      expect(ipad.e2ee.historyFromMs, 1000);
      receiver.progress.add(at(3, 12));
      await pumpEventQueue();
      expect(ipad.e2ee.transfer, at(3, 12));
      expect(receiver.closed, isFalse);
      receiver.progress.add(at(12, 12, DmTransferEnd.complete));
      await pumpEventQueue();
      expect(ipad.e2ee.transfer, isNull);
      expect(receiver.closed, isTrue);
    });

    test('Skip stops it and closes the endpoint', () async {
      final (_, ipad) = await added();
      final receiver = ipad.transfers.single;
      receiver.progress.add(at(3, 12));
      await pumpEventQueue();
      await ipad.e2ee.skipTransfer();
      expect(receiver.cancelled, isTrue);
      expect(receiver.closed, isTrue);
      expect(ipad.e2ee.transfer, at(3, 12, DmTransferEnd.skipped));
      expect(ipad.e2ee.canRetryTransfer, isFalse);
      await ipad.e2ee.dismissTransfer();
      expect(ipad.e2ee.transfer, isNull);
    });

    test('cut short, Try again asks the phone for it again', () async {
      final (phone, ipad) = await added();
      final receiver = ipad.transfers.single;
      receiver.progress.add(at(5, 12, DmTransferEnd.cutShort));
      await pumpEventQueue();
      expect(ipad.e2ee.transfer, at(5, 12, DmTransferEnd.cutShort));
      expect(receiver.closed, isFalse);
      expect(ipad.e2ee.canRetryTransfer, isTrue);

      await ipad.e2ee.retryTransfer();
      expect(receiver.receiving, 2);
      expect(ipad.e2ee.transfer, at(0, 0));
      // The phone reads the ask in the self group and sends again.
      await phone.e2ee.sync();
      await pumpEventQueue();
      expect(phone.transfers, hasLength(2));
      expect(phone.transfers.last.sent, ['ipad']);
    });

    test("a device that didn't add this one doesn't answer", () async {
      final (phone, ipad) = await added();
      phone.device.historyFor = {};
      ipad.transfers.single.progress.add(at(0, 0, DmTransferEnd.cutShort));
      await pumpEventQueue();
      await ipad.e2ee.retryTransfer();
      await phone.e2ee.sync();
      await pumpEventQueue();
      expect(phone.transfers, hasLength(1));
    });

    test('the endpoint is closed 30 minutes on', () async {
      final (_, ipad) = await added();
      final receiver = ipad.transfers.single;
      receiver.progress.add(at(5, 12, DmTransferEnd.cutShort));
      await pumpEventQueue();
      ipad.now += const Duration(minutes: 31).inMilliseconds;
      expect(ipad.e2ee.canRetryTransfer, isFalse);
      await ipad.e2ee.sync();
      expect(receiver.closed, isTrue);
      expect(ipad.e2ee.transfer, at(5, 12, DmTransferEnd.cutShort));
    });

    test('a new code closes the endpoint of the old one', () async {
      final worker = FakeWorker();
      await _ready(worker, 'matt', 'phone');
      final ipad = FakeEndpoint(worker, 'matt', 'ipad');
      await ipad.e2ee.start();
      await ipad.e2ee.offerThisDevice(platform: 'iPad');
      await ipad.e2ee.offerThisDevice(platform: 'iPad');
      expect(ipad.transfers.map((each) => each.closed), [true, false]);
    });

    test("one the relay won't admit: added all the same, without it", () async {
      final worker = FakeWorker();
      final phone = await _ready(worker, 'matt', 'phone');
      final ipad = FakeEndpoint(worker, 'matt', 'ipad')..admit = false;
      await ipad.e2ee.start();
      final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
      expect(ipad.transfers.single.closed, isTrue);
      await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
      await ipad.e2ee.takeHandoff();
      expect(ipad.e2ee.setup, DmSetup.ready);
      expect(ipad.e2ee.transfer, at(0, 0, DmTransferEnd.skipped));
      expect(ipad.e2ee.canRetryTransfer, isFalse);
    });
  });

  group('Lost or stolen: a new security key (R4)', () {
    // Matt's phone, iPad and Mac, and Ana; the Mac is lost.
    Future<
      ({
        FakeWorker worker,
        FakeEndpoint phone,
        FakeEndpoint ipad,
        FakeEndpoint mac,
        FakeEndpoint ana,
        String chat,
        String? bobChat,
      })
    >
    world({
      SignedCall Function(SignedCall worker)? route,
      bool bob = false,
      bool renewing = false,
    }) async {
      final worker = FakeWorker();
      final phone = FakeEndpoint(worker, 'matt', 'phone', route: route);
      await phone.e2ee.start();
      await phone.e2ee.setUpFirst();
      final ana = await _ready(worker, 'ana', 'pixel');
      final chat = (await phone.e2ee.conversationGroup('v1', 3, 'ana'))!;
      // A second chat, for a third group to commit in.
      String? bobChat;
      if (bob) {
        await _ready(worker, 'bob', 'bobphone');
        bobChat = (await phone.e2ee.conversationGroup('v2', 1, 'bob'))!.groupId;
      }
      Future<FakeEndpoint> add(String leaf) async {
        final other = FakeEndpoint(worker, 'matt', leaf);
        await other.e2ee.start();
        final code = await other.e2ee.offerThisDevice(platform: leaf);
        await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
        if (!renewing) {
          await other.e2ee.takeHandoff();
          return other;
        }
        // The setup screen asks for a new code while the hand-off is
        // taken: as it starts, and once it's under way (Abbacchio's R5).
        final take = other.e2ee.takeHandoff();
        final first = other.e2ee.offerThisDevice(platform: leaf);
        for (var i = 0; i < 3; i++) {
          await Future<void>.delayed(Duration.zero);
        }
        final later = other.e2ee.offerThisDevice(platform: leaf);
        await take;
        expect(await first, isNull);
        expect(await later, isNull);
        // Set up, with no code left to take hand-offs as its own.
        expect(other.e2ee.setup, DmSetup.ready);
        expect(other.e2ee.offer, isNull);
        return other;
      }

      final ipad = await add('ipad');
      final mac = await add('mac');
      for (final each in [phone, ipad, mac, ana]) {
        await each.e2ee.sync();
      }
      expect(await _leaves(phone, chat.groupId), [
        'phone',
        'pixel',
        'ipad',
        'mac',
      ]);
      return (
        worker: worker,
        phone: phone,
        ipad: ipad,
        mac: mac,
        ana: ana,
        chat: chat.groupId,
        bobChat: bobChat,
      );
    }

    // A sync of [endpoint] started while each commit waits on the worker,
    // as the outbox starts one (Abbacchio's R5b): it reads the groups again
    // meanwhile.
    SignedCall Function(SignedCall) racing(FakeEndpoint? Function() endpoint) =>
        (call) => (method, path, [body]) async {
          if (path.endsWith('/commits') && endpoint() != null) {
            // Not awaited: it may commit too, through this same route.
            final sync = endpoint()!.e2ee.sync();
            for (var i = 0; i < 5; i++) {
              await Future<void>.delayed(Duration.zero);
            }
            unawaited(sync);
            return call(method, path, body);
          }
          return call(method, path, body);
        };

    String self(FakeEndpoint endpoint) => endpoint.e2ee.groups.values
        .singleWhere((each) => each.kind == 'self')
        .groupId;

    test(
      'a code asked for while the kept device was added leaves none '
      'behind: it takes the new key, the bundle there when it looks',
      () async {
        final w = await world(renewing: true);
        expect(
          await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
          isNull,
        );
        expect(w.worker.handoffs['ipad'], hasLength(1));
        // A hand-off came: a set-up device leaves it to the new key's taking.
        await w.ipad.e2ee.takeHandoff();
        expect(w.worker.handoffs['ipad'], hasLength(1));
        await w.ipad.e2ee.sync();
        expect(w.ipad.e2ee.problem, isEmpty);
        expect(w.ipad.e2ee.setup, DmSetup.ready);
        expect(w.ipad.device.identity, w.phone.device.identity);
        expect(w.ipad.e2ee.newKeyFrom, 'phone');
        expect(w.worker.handoffs['ipad'], isEmpty);
      },
    );

    // Abbacchio's R5 rows 570–575: a kept device that missed the rotation
    // had its move refused as behind at every pass of a sync, about 50
    // requests a second, until the service said Too many tries.
    test('a kept device whose move is refused as behind waits longer each '
        'time, joins again from the group, and finishes once it can', () async {
      final w = await world();
      expect(
        await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
        isNull,
      );
      final commits = 'POST groups/${w.chat}/commits';
      var refusing = true;
      w.worker.refuse = (route) => refusing && route == commits
          ? AccountServiceError(409, 'epoch', 'epoch', {
              'epoch': w.worker.epochOf(w.chat),
            })
          : null;
      final before = w.worker.log.length;
      int tried() =>
          w.worker.log.skip(before).where((route) => route == commits).length;

      for (var i = 0; i < 5; i++) {
        await w.ipad.e2ee.sync();
      }
      expect(tried(), 1, reason: 'refused once, then it waits');
      expect(w.ipad.e2ee.finishingKey, isTrue);

      // After the wait, once more, and behind again: it joins again from
      // the group as the worker has it (refused too, here).
      w.ipad.now += const Duration(seconds: 5).inMilliseconds;
      for (var i = 0; i < 5; i++) {
        await w.ipad.e2ee.sync();
      }
      expect(tried(), 3);
      expect(w.worker.log, contains('GET groups/${w.chat}/group-info'));
      // Its join refused, it's out of the group here until the next try.
      expect(await _leaves(w.ipad, w.chat), isEmpty);
      for (var i = 0; i < 5; i++) {
        await w.ipad.e2ee.sync();
      }
      expect(tried(), 3, reason: 'nothing more until the wait is over');

      // The worker takes it again: after the waits it's in, moved.
      refusing = false;
      w.ipad.now += const Duration(minutes: 10).inMilliseconds;
      await w.ipad.e2ee.sync();
      await w.ipad.e2ee.sync();
      expect(w.ipad.e2ee.finishingKey, isFalse);
      expect((await w.ipad.device.groups()).every((g) => g.moved), isTrue);
      expect(await _leaves(w.ipad, w.chat), contains('ipad'));
    });

    // Abbacchio's row 590: after waiting out a refusal, the kept device
    // never tried again by itself in a quiet account. Following the
    // account, it syncs once a wait is over, with nothing new.
    test('once a wait after a refusal is over, following syncs anyway, '
        'and the kept device finishes', () async {
      final w = await world();
      expect(
        await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
        isNull,
      );
      final commits = 'POST groups/${w.chat}/commits';
      var refusing = true;
      w.worker.refuse = (route) => refusing && route == commits
          ? AccountServiceError(409, 'epoch', 'epoch', {
              'epoch': w.worker.epochOf(w.chat),
            })
          : null;
      await w.ipad.e2ee.sync();
      expect(w.ipad.e2ee.finishingKey, isTrue);
      expect(w.ipad.e2ee.waitOver, isFalse, reason: 'still waiting');

      refusing = false;
      w.ipad.now += const Duration(seconds: 6).inMilliseconds;
      expect(w.ipad.e2ee.waitOver, isTrue);
      // What following does at its next answer, empty as it is.
      await w.ipad.e2ee.sync();
      expect(w.ipad.e2ee.finishingKey, isFalse);
      expect((await w.ipad.device.groups()).every((g) => g.moved), isTrue);
      expect(w.ipad.e2ee.waitOver, isFalse, reason: 'once, not every answer');
    });

    // Abbacchio's row 601: a sync that stopped on the worker's brief 503
    // wasn't tried again until something else happened in the account.
    test('a sync stopped by an error is tried again once its wait is over, '
        'and the kept device takes the new key', () async {
      final w = await world();
      expect(
        await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
        isNull,
      );
      final messages = 'GET groups/${self(w.ipad)}/messages';
      var failing = true;
      w.worker.refuse = (route) => failing && route == messages
          ? AccountServiceError(503, 'unavailable', 'unavailable', const {})
          : null;
      // Long after anything its setup waited for.
      w.ipad.now += const Duration(hours: 1).inMilliseconds;
      await w.ipad.e2ee.sync();
      expect(w.ipad.e2ee.problem, contains('unavailable'));
      expect(w.ipad.e2ee.newKeyFrom, isNull);
      expect(w.ipad.e2ee.waitOver, isFalse, reason: 'still waiting');

      failing = false;
      w.ipad.now += const Duration(seconds: 6).inMilliseconds;
      expect(w.ipad.e2ee.waitOver, isTrue);
      // What following does at its next answer, empty as it is.
      await w.ipad.e2ee.sync();
      expect(w.ipad.e2ee.newKeyFrom, 'phone');
      expect(w.ipad.device.identity, w.phone.device.identity);
      expect(w.ipad.e2ee.finishingKey, isFalse);
      expect(w.ipad.e2ee.waitOver, isFalse);
    });

    test('syncs that fail during the wait after a failed sync leave the wait '
        'as it is; one after it doubles it', () async {
      final w = await world();
      expect(
        await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
        isNull,
      );
      final messages = 'GET groups/${self(w.ipad)}/messages';
      w.worker.refuse = (route) => route == messages
          ? AccountServiceError(503, 'unavailable', 'unavailable', const {})
          : null;
      w.ipad.now += const Duration(hours: 1).inMilliseconds;
      await w.ipad.e2ee.sync();
      expect(w.ipad.e2ee.problem, contains('unavailable'));
      // Five more quick failures, as group events would start them: the
      // wait still ends retryAfter after the first.
      for (var i = 0; i < 5; i++) {
        w.ipad.now += 500;
        await w.ipad.e2ee.sync();
      }
      w.ipad.now += const Duration(seconds: 3).inMilliseconds;
      expect(w.ipad.e2ee.waitOver, isTrue);
      // The try once it's over fails too: the next wait is twice as long.
      await w.ipad.e2ee.sync();
      w.ipad.now += const Duration(seconds: 9).inMilliseconds;
      expect(w.ipad.e2ee.waitOver, isFalse);
      w.ipad.now += const Duration(seconds: 2).inMilliseconds;
      expect(w.ipad.e2ee.waitOver, isTrue);
    });

    test('the lost device is signed out before the new key; the kept device '
        'takes it from the self group, and Ana holds the moves until she has '
        'it', () async {
      final w = await world();
      final old = w.phone.device.identity!;

      expect(
        await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
        isNull,
      );
      // Fugo: the lost device is signed out of the account before the
      // worker is given the new key.
      final signOut = w.worker.log.indexOf('POST /v1/devices/remove');
      final rotate = w.worker.log.indexOf('POST me/keys/rotate');
      expect(signOut, greaterThanOrEqualTo(0));
      expect(rotate, greaterThan(signOut));
      expect(w.worker.signedOut['matt'], {'mac'});
      // The worker serves the new key, which this device has, kept in the
      // key store as the identity; nothing left as the next.
      final now = w.phone.device.identity!;
      expect(now, isNot(old));
      expect(w.worker.keys['matt']!['identity_key'], dmB64(now));
      expect(w.phone.keys.kept[DmKeyName.identityNext], isNull);
      expect(
        w.phone.keys.kept[DmKeyName.identity]!.reversed,
        orderedEquals(now),
      );
      expect(w.phone.device.lists['matt']!.leaves.map((each) => each.leafId), [
        'phone',
        'ipad',
      ]);
      // Moved everywhere, and new key packages up.
      expect((await w.phone.device.groups()).every((g) => g.moved), isTrue);
      expect(w.phone.e2ee.finishingKey, isFalse);
      expect(w.worker.packages['matt/phone'], hasLength(dmKeyPackages));

      // The iPad takes it from the self group, with the bundle the phone
      // left it, and says where it came from.
      await w.ipad.e2ee.sync();
      expect(w.ipad.e2ee.setup, DmSetup.ready);
      expect(w.ipad.device.identity, now);
      expect(w.ipad.e2ee.newKeyFrom, 'phone');
      expect(w.worker.handoffs['ipad'], isEmpty);
      expect(w.ipad.keys.kept[DmKeyName.identityNext], isNull);
      expect((await w.ipad.device.groups()).every((g) => g.moved), isTrue);
      w.ipad.e2ee.dismissNewKey();
      expect(w.ipad.e2ee.newKeyFrom, isNull);

      // Ana held the phone's move until she fetched Matt's new key: now a
      // key change she sees, and the chat goes on.
      await w.ana.e2ee.sync();
      expect(w.ana.e2ee.held, isEmpty);
      expect(w.ana.device.pins['matt']!.identityPublic, now);
      expect(w.ana.device.pins['matt']!.changedUnseen, isTrue);
      expect(
        w.ana.device.processed.where((p) => p.outcome == DmOutcome.held),
        isNotEmpty,
      );
      expect(await _leaves(w.ana, w.chat), ['phone', 'pixel', 'ipad']);
      await w.ana.e2ee.seen('matt');
      await w.ana.e2ee.say(w.chat, _text('a1', 'hi'));
      await w.phone.e2ee.sync();
      expect(
        (await w.phone.device.history(
          groupId: dmUnb64(w.chat),
          limit: 9,
        )).map((m) => m.content.text),
        contains('hi'),
      );
      // In Ana's history, the line that Matt's key changed comes before
      // what's said under it, once for all his devices' moves.
      final anas = await w.ana.device.history(
        groupId: dmUnb64(w.chat),
        limit: 20,
      );
      expect(
        anas.where((m) => m.members?.keyChanged.contains('matt') ?? false),
        hasLength(1),
      );

      // The Mac is signed out: the worker answers it no more.
      await w.mac.e2ee.sync();
      expect(w.mac.device.identity, old);
    });

    test('a crash after the message: carried on with the same new key, which '
        "the iPad waits for until the worker serves it", () async {
      var lose = true;
      final w = await world(
        route: (call) => (method, path, [body]) async {
          if (path.endsWith('/me/keys/rotate') && lose) {
            throw const AccountServiceUnreachable('offline');
          }
          return call(method, path, body);
        },
      );
      final old = w.phone.device.identity!;
      expect(
        await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
        DmKeyChange.failed,
      );
      final rotation = (await w.phone.device.rotation())!;
      expect(rotation.said, isTrue);
      expect(w.phone.keys.kept[DmKeyName.identityNext], isNotNull);
      expect(w.phone.device.identity, old);

      // (c) The iPad has the message, but the worker serves the old key:
      // nothing is taken, and it goes on as it was.
      await w.ipad.e2ee.sync();
      expect(w.ipad.e2ee.setup, DmSetup.ready);
      expect(w.ipad.device.identity, old);
      expect(await w.ipad.device.pendingRotation(), isNotNull);

      // The phone starts again: the same key, not a second one, and the
      // message isn't said twice.
      lose = false;
      final said = w.worker.mailboxOf(self(w.phone)).length;
      await w.phone.e2ee.start();
      expect(w.phone.e2ee.setup, DmSetup.ready);
      expect(w.phone.device.identity, rotation.identityPublic);
      expect(
        w.worker.keys['matt']!['identity_key'],
        dmB64(rotation.identityPublic),
      );
      expect(await w.phone.device.rotation(), isNull);
      final rotations = [
        for (final item in w.worker.mailboxOf(self(w.phone)).skip(said))
          if (readFake(jsonEncode(item['envelope']))['t'] == 'app') item,
      ];
      expect(rotations, isEmpty);

      await w.ipad.e2ee.sync();
      expect(w.ipad.device.identity, rotation.identityPublic);
    });

    test(
      'a rotation that went through with its answer lost counts as done',
      () async {
        // It goes through, its answer is lost, and so is the check after.
        var lose = 0;
        final w = await world(
          route: (call) => (method, path, [body]) async {
            if (lose == 1 && path.contains('/dms/keys/matt')) {
              lose = 2;
              throw const AccountServiceUnreachable('offline');
            }
            final answer = await call(method, path, body);
            if (path.endsWith('/me/keys/rotate') && lose == 0) {
              lose = 1;
              throw const AccountServiceUnreachable('offline');
            }
            return answer;
          },
        );
        expect(
          await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
          DmKeyChange.failed,
        );
        // Again: the worker answers 400 (it has the key already), which is
        // done.
        await w.phone.e2ee.start();
        final rotation = w.worker.keys['matt']!['identity_key'];
        expect(dmB64(w.phone.device.identity!), rotation);
        expect(await w.phone.device.rotation(), isNull);
        expect(w.phone.e2ee.failure, DmFailure.none);
      },
    );

    test('the Claktory this app shows is listed and enrolled again under '
        'the new key, its leaf kept in its channel', () async {
      final w = await world();
      final mini = FakeClaktoryLink(w.worker, 'matt');
      w.phone.e2ee.claktory = mini;
      await w.phone.e2ee.sync();
      expect(mini.enrollments, hasLength(1));
      final channel = w.phone.e2ee.groups.values
          .singleWhere((each) => each.kind == 'channel')
          .groupId;
      expect(await _leaves(w.phone, channel), contains('mini#1'));
      expect(w.phone.e2ee.claktoriesToFinish, isEmpty);

      expect(
        await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
        isNull,
      );
      expect(mini.enrollments, hasLength(2));
      expect(w.phone.device.lists['matt']!.leaves.map((each) => each.leafId), [
        'phone',
        'ipad',
        'mini#1',
      ]);
      expect(w.worker.keys['matt']!['version'], 2);
      expect(w.phone.e2ee.claktoriesToFinish, isEmpty);
      // Still in its channel, once: not added again.
      expect(
        (await _leaves(w.phone, channel)).where((leaf) => leaf == 'mini#1'),
        hasLength(1),
      );
    });

    test("the lost device can't be signed out: no new key; Try again "
        'carries on to it', () async {
      var offline = true;
      final w = await world(
        route: (call) => (method, path, [body]) async {
          if (path == '/v1/devices/remove' && offline) {
            throw const AccountServiceUnreachable('offline');
          }
          return call(method, path, body);
        },
      );
      final old = w.phone.device.identity!;
      expect(
        await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
        DmKeyChange.signOutFailed,
      );
      expect(w.worker.log, isNot(contains('POST me/keys/rotate')));
      expect(await w.phone.device.rotation(), isNull);
      expect(w.phone.device.identity, old);
      // It no longer gets new messages: out of every group already.
      for (final group in w.phone.e2ee.groups.values) {
        expect(await _leafIds(w.phone, group.groupId), isNot(contains('mac')));
      }

      // Try again: signed out, then the hand-off, the message in the self
      // group and the new key.
      offline = false;
      expect(
        await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
        isNull,
      );
      expect(w.worker.signedOut['matt'], {'mac'});
      expect(w.worker.log, contains('POST me/keys/rotate'));
      expect(w.phone.device.identity, isNot(old));
      await w.ipad.e2ee.sync();
      expect(w.ipad.device.identity, w.phone.device.identity);
    });

    test('a removal that stopped half way (off the list, still in its '
        'groups) is finished by Lock out again', () async {
      final w = await world();
      // As a build before the fix left it: the list without the Mac, its
      // leaf still in every group.
      final listed = w.phone.device.deviceList(account: 'matt')!.leaves;
      // Signed and taken (pinned once the worker has it).
      await w.phone.device.acceptList(
        account: 'matt',
        deviceList: await w.phone.device.signDeviceList(
          version: BigInt.from(9),
          createdMs: BigInt.one,
          leaves: [
            for (final each in listed)
              if (each.leafId != 'mac') each,
          ],
        ),
      );
      expect(await _leafIds(w.phone, self(w.phone)), contains('mac'));
      expect(await _leaves(w.phone, self(w.phone)), isNot(contains('mac')));
      expect(
        await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
        isNull,
      );
      for (final group in w.phone.e2ee.groups.values) {
        expect(await _leafIds(w.phone, group.groupId), isNot(contains('mac')));
      }
      expect(w.worker.log, contains('POST me/keys/rotate'));
    });

    test('Remove with three devices takes the one removed out of the self '
        'group and the chat, and the others carry on', () async {
      final w = await world();
      await w.phone.e2ee.removeDevice('mac');
      for (final group in [self(w.phone), w.chat]) {
        expect(await _leafIds(w.phone, group), isNot(contains('mac')));
      }
      for (final each in [w.ipad, w.ana]) {
        await each.e2ee.sync();
      }
      expect(await _leafIds(w.ana, w.chat), ['phone', 'pixel', 'ipad']);
      expect(await _leafIds(w.ipad, self(w.ipad)), ['phone', 'ipad']);
      await w.phone.e2ee.say(w.chat, _text('p1', 'still here'));
      await w.ana.e2ee.sync();
      expect(
        (await w.ana.e2ee.history(w.chat)).map((m) => m.content.text),
        contains('still here'),
      );
    });

    test('Remove while syncs run takes the device out of all three groups '
        'with no error, and it reads nothing after', () async {
      FakeEndpoint? racer;
      final w = await world(route: racing(() => racer), bob: true);
      final phone = racer = w.phone;
      final groups = [self(phone), w.chat, w.bobChat!];
      for (final group in groups) {
        expect(await _leafIds(phone, group), contains('mac'));
      }
      expect(await phone.e2ee.remove('mac', 'MacBook'), isNull);
      for (final group in groups) {
        expect(await _leafIds(phone, group), isNot(contains('mac')));
      }
      expect(w.worker.signedOut['matt'], {'mac'});
      expect(phone.device.removalRow, isNull);
      expect(phone.e2ee.removal, isNull);
      // What's said after is for the phone, the iPad and Ana, not the Mac.
      await phone.e2ee.say(w.chat, _text('p9', 'after'));
      await w.mac.e2ee.sync();
      expect(
        (await w.mac.e2ee.history(w.chat)).map((m) => m.content.text),
        isNot(contains('after')),
      );
    });

    test('Lock out while syncs run commits the device out of every group, '
        'signs it out and makes the new key', () async {
      FakeEndpoint? racer;
      final w = await world(route: racing(() => racer), bob: true);
      final phone = racer = w.phone;
      final old = phone.device.identity!;
      expect(
        await phone.e2ee.changeKey(
          remove: ['mac'],
          keep: ['ipad'],
          names: ['MacBook'],
        ),
        isNull,
      );
      for (final group in [self(phone), w.chat, w.bobChat!]) {
        expect(await _leafIds(phone, group), isNot(contains('mac')));
      }
      expect(w.worker.signedOut['matt'], {'mac'});
      expect(phone.device.identity, isNot(old));
      expect(phone.device.removalRow, isNull);
    });

    test('a device off the list but still in the self group and a chat (a '
        'removal from before the fix) is taken out at the next start, and '
        'reads nothing after', () async {
      final w = await world();
      // The list without the Mac, at the worker too; its leaf still in
      // every group, and nothing kept to carry on (Abbacchio's rig).
      w.worker.refuse = (route) => route.endsWith('/commits')
          ? const AccountServiceUnreachable('x')
          : null;
      expect(await w.phone.e2ee.remove('mac', 'MacBook'), DmRemoveStop.failed);
      w.worker.refuse = null;
      w.phone.device.removalRow = null;
      w.phone.e2ee.removal = null;
      expect(
        w.phone.device.lists['matt']!.leaves.map((each) => each.leafId),
        isNot(contains('mac')),
      );
      expect(await _leafIds(w.phone, self(w.phone)), contains('mac'));
      expect(await _leafIds(w.phone, w.chat), contains('mac'));
      // Shown as not fully removed until then.
      expect(await w.phone.e2ee.notFullyRemoved(), {'mac': 'mac'});

      final phone = w.phone.relaunch();
      await phone.start();
      expect(phone.setup, DmSetup.ready);
      for (final group in [self(w.phone), w.chat]) {
        expect(await _leafIds(w.phone, group), isNot(contains('mac')));
      }
      expect(await phone.notFullyRemoved(), isEmpty);
      // And signed out of the account on the same start (Fugo): it can do
      // nothing there either, such as lock the person's own Mac out.
      expect(w.worker.signedOut['matt'], {'mac'});
      expect(w.phone.device.removalRow, isNull);
      expect(phone.removal, isNull);
      await phone.say(w.chat, _text('p8', 'after'));
      await w.mac.e2ee.sync();
      expect(
        (await w.mac.e2ee.history(w.chat)).map((m) => m.content.text),
        isNot(contains('after')),
      );
    });

    test("a swept device whose sign-out doesn't go through is kept as under "
        'way, and signed out at the next launch', () async {
      var offline = false;
      final w = await world(
        route: (call) => (method, path, [body]) async {
          if (path == '/v1/devices/remove' && offline) {
            throw const AccountServiceUnreachable('offline');
          }
          return call(method, path, body);
        },
      );
      w.worker.refuse = (route) => route.endsWith('/commits')
          ? const AccountServiceUnreachable('x')
          : null;
      expect(await w.phone.e2ee.remove('mac', 'MacBook'), DmRemoveStop.failed);
      w.worker.refuse = null;
      w.phone.device.removalRow = null;
      w.phone.e2ee.removal = null;
      offline = true;
      var phone = w.phone.relaunch();
      await phone.start();
      for (final group in [self(w.phone), w.chat]) {
        expect(await _leafIds(w.phone, group), isNot(contains('mac')));
      }
      expect(w.worker.signedOut['matt'], isNull);
      // Your devices says so, with Try again.
      expect(phone.removal?.remove, ['mac']);
      expect(phone.removal?.newKey, isFalse);
      offline = false;
      phone = w.phone.relaunch();
      await phone.start();
      expect(w.worker.signedOut['matt'], {'mac'});
      expect(phone.removal, isNull);
    });

    test('a device signed out by an earlier sweep but still in a group '
        'ends its row once the next sweep takes it out', () async {
      final w = await world();
      w.worker.refuse = (route) => route.endsWith('/commits')
          ? const AccountServiceUnreachable('x')
          : null;
      expect(await w.phone.e2ee.remove('mac', 'MacBook'), DmRemoveStop.failed);
      w.phone.device.removalRow = null;
      w.phone.e2ee.removal = null;
      // The first sweep takes it out of the self group and signs it out;
      // the chat's commit doesn't go through.
      final chat = 'POST groups/${w.chat}/commits';
      w.worker.refuse = (route) =>
          route == chat ? const AccountServiceUnreachable('x') : null;
      await w.phone.e2ee.sync();
      expect(w.worker.signedOut['matt'], {'mac'});
      expect(await _leafIds(w.phone, self(w.phone)), isNot(contains('mac')));
      expect(await _leafIds(w.phone, w.chat), contains('mac'));
      // The next takes it out of the chat: nothing left under way
      // (Abbacchio's row 613: "isn't fully removed yet" stayed).
      w.worker.refuse = null;
      w.phone.now += const Duration(hours: 1).inMilliseconds;
      await w.phone.e2ee.sync();
      expect(await _leafIds(w.phone, w.chat), isNot(contains('mac')));
      expect(w.phone.e2ee.removal, isNull);
      expect(w.phone.device.removalRow, isNull);
      expect(await w.phone.e2ee.notFullyRemoved(), isEmpty);
    });

    test('a list the worker refused is never pinned, so the next one '
        'another device puts is taken and the next change follows it '
        '(row 615)', () async {
      final w = await world();
      final before = w.phone.device.lists['matt']!.version;
      final laptop = FakeEndpoint(w.worker, 'matt', 'laptop');
      await laptop.e2ee.start();
      final code = await laptop.e2ee.offerThisDevice(platform: 'Mac');
      final found = await w.phone.e2ee.scanned(code!);
      w.worker.refuse = (route) => route == 'PUT me/keys'
          ? AccountServiceError(503, 'unavailable', 'unavailable', const {})
          : null;
      await expectLater(w.phone.e2ee.addDevice(found), throwsA(anything));
      w.worker.refuse = null;
      expect(w.phone.device.lists['matt']!.version, before);
      // The iPad takes the Mac off meanwhile, at that same version.
      await w.ipad.e2ee.removeDevice('mac');
      final theirs = w.worker.keys['matt']!['version']! as int;
      expect(theirs, before.toInt() + 1);
      await w.phone.e2ee.refreshKeys('matt');
      expect(w.phone.device.lists['matt']!.version.toInt(), theirs);
      // Added again: on top of the iPad's list.
      await w.phone.e2ee.addDevice(found);
      expect(w.worker.keys['matt']!['version'], theirs + 1);
      expect(
        w.phone.device.lists['matt']!.leaves.map((each) => each.leafId),
        allOf(contains('laptop'), isNot(contains('mac'))),
      );
    });

    test('a Lock out with a sign-in too old asks to sign in again, then '
        'carries on where it stopped (row 630)', () async {
      final w = await world();
      final old = w.worker.keys['matt']!['identity_key'];
      w.worker.staleSignIn.add('matt/phone');
      expect(
        await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
        DmKeyChange.signInAgain,
      );
      expect(w.worker.keys['matt']!['identity_key'], old);
      // Kept as under way, so nothing is lost.
      expect(w.phone.e2ee.removal?.newKey, isTrue);
      w.worker.staleSignIn.clear();
      expect(
        await w.phone.e2ee.changeKey(remove: ['mac'], keep: ['ipad']),
        isNull,
      );
      expect(w.worker.keys['matt']!['identity_key'], isNot(old));
    });

    test('an add code works once, and never again once its device is '
        'removed (row 612)', () async {
      final w = await world();
      final laptop = FakeEndpoint(w.worker, 'matt', 'laptop');
      await laptop.e2ee.start();
      final code = await laptop.e2ee.offerThisDevice(platform: 'Mac');
      // Shown and not added: it still works.
      await w.phone.e2ee.scanned(code!);
      await w.phone.e2ee.addDevice(await w.phone.e2ee.scanned(code));
      await laptop.e2ee.takeHandoff();
      await expectLater(
        w.phone.e2ee.scanned(code),
        throwsA(isA<DmCodeRefused>()),
      );
      // Locked out, with the iPad and the Mac kept: the same code can't
      // bring it back, here or on the iPad, which took the list without it.
      for (final device in [w.phone, w.ipad, w.mac, w.phone]) {
        await device.e2ee.sync();
      }
      expect(
        await w.phone.e2ee.changeKey(remove: ['laptop'], keep: ['ipad', 'mac']),
        isNull,
        reason: w.phone.e2ee.problem,
      );
      final version = w.worker.keys['matt']!['version'];
      await w.ipad.e2ee.sync();
      for (final device in [w.phone, w.ipad]) {
        await expectLater(
          device.e2ee.scanned(code),
          throwsA(isA<DmCodeRefused>()),
        );
      }
      expect(w.worker.keys['matt']!['version'], version);
      expect(
        w.phone.device.lists['matt']!.leaves.map((each) => each.leafId),
        isNot(contains('laptop')),
      );
    });

    test('a new device another device just added is never taken for one '
        'removed', () async {
      final w = await world();
      // The iPad adds a laptop; the phone hasn't read the new list yet,
      // and the laptop is in the self group already.
      final laptop = FakeEndpoint(w.worker, 'matt', 'laptop');
      await laptop.e2ee.start();
      final code = await laptop.e2ee.offerThisDevice(platform: 'Mac');
      await w.ipad.e2ee.addDevice(await w.ipad.e2ee.scanned(code!));
      await laptop.e2ee.takeHandoff();
      await laptop.e2ee.sync();
      await w.ipad.e2ee.sync();
      expect(await _leafIds(w.ipad, self(w.ipad)), contains('laptop'));
      // The phone reads the new list before it takes anything out.
      await w.phone.e2ee.sync();
      expect(await _leafIds(w.phone, self(w.phone)), contains('laptop'));
    });

    test('a Lock out stopped part-way (signed out failed, dialog closed) '
        'carries on at the next launch, through to the new key', () async {
      var offline = true;
      final w = await world(
        route: (call) => (method, path, [body]) async {
          if (path == '/v1/devices/remove' && offline) {
            throw const AccountServiceUnreachable('offline');
          }
          return call(method, path, body);
        },
      );
      final old = w.phone.device.identity!;
      expect(
        await w.phone.e2ee.changeKey(
          remove: ['mac'],
          keep: ['ipad'],
          names: ['MacBook'],
        ),
        DmKeyChange.signOutFailed,
      );
      // Off the list now: only what's kept names it.
      expect(w.phone.e2ee.removal?.names, ['MacBook']);
      expect(
        (await w.phone.e2ee.devices()).map((each) => each.leafId),
        isNot(contains('mac')),
      );
      offline = false;
      final phone = w.phone.relaunch();
      await phone.start();
      expect(w.worker.signedOut['matt'], {'mac'});
      expect(w.worker.log, contains('POST me/keys/rotate'));
      expect(w.phone.device.identity, isNot(old));
      expect(w.phone.device.removalRow, isNull);
      expect(phone.removal, isNull);
    });

    test('a Remove stopped at its sign-out carries on at the next launch, '
        'and Try again in Your devices does too', () async {
      var offline = true;
      final w = await world(
        route: (call) => (method, path, [body]) async {
          if (path == '/v1/devices/remove' && offline) {
            throw const AccountServiceUnreachable('offline');
          }
          return call(method, path, body);
        },
      );
      expect(
        await w.phone.e2ee.remove('mac', 'MacBook'),
        DmRemoveStop.signOutUnreachable,
      );
      expect(w.phone.e2ee.removal?.names, ['MacBook']);
      // Still offline at the next launch: kept.
      var phone = w.phone.relaunch();
      await phone.start();
      expect(phone.removal?.remove, ['mac']);
      expect(await phone.finishRemoval(), DmRemoveStop.signOutUnreachable);
      offline = false;
      expect(await phone.finishRemoval(), isNull);
      expect(w.worker.signedOut['matt'], {'mac'});
      expect(phone.removal, isNull);
      expect(w.phone.device.removalRow, isNull);
      // And the key is the same: Remove makes no new one.
      expect(w.worker.log, isNot(contains('POST me/keys/rotate')));
      phone = w.phone.relaunch();
      await phone.start();
      expect(phone.removal, isNull);
    });

    test('another key served with no message naming it: added again from '
        'there', () async {
      final w = await world();
      w.worker.keys['matt'] = {
        ...w.worker.keys['matt']!,
        'identity_key': dmB64(List.filled(32, 99)),
      };
      await w.ipad.e2ee.start();
      expect(w.ipad.e2ee.failure, DmFailure.identityReplaced);
    });

    test('a commit under a key the worker never serves is held, then Join '
        'again a day on', () async {
      final w = await world();
      final group = w.chat;
      w.worker.deliver(
        group,
        fakeEnvelope({
          't': 'commit',
          'g': group,
          'epoch': w.worker.epochOf(group),
          'from': 'matt/phone',
          'members': ['matt/phone', 'ana/pixel'],
          'id': dmB64(List.filled(32, 98)),
        }),
      );
      await w.ana.e2ee.sync();
      expect(w.ana.e2ee.held.keys, [group]);
      expect(w.ana.e2ee.heldLong(group), isFalse);
      // Tried again, still held: nothing past it is read.
      await w.ana.e2ee.sync();
      expect(w.ana.e2ee.held.keys, [group]);
      w.ana.now += E2ee.heldFor.inMilliseconds;
      expect(w.ana.e2ee.heldLong(group), isTrue);
      await w.ana.e2ee.rejoinHeld(group);
      expect(w.ana.e2ee.held, isEmpty);
    });

    /// Ana's device held at a commit nobody can take; [said] messages
    /// come after it before she joins again.
    Future<({FakeEndpoint ana, String group, int held})> heldThenSaid(
      int said,
    ) async {
      final w = await world();
      final group = w.chat;
      w.worker.deliver(
        group,
        fakeEnvelope({
          't': 'commit',
          'g': group,
          'epoch': w.worker.epochOf(group),
          'from': 'matt/phone',
          'members': ['matt/phone', 'ana/pixel'],
          'id': dmB64(List.filled(32, 98)),
        }),
        // The worker's GroupInfo stays the one to join again from.
        advance: false,
      );
      final held = w.worker.mailboxOf(group).length;
      await w.ana.e2ee.sync();
      for (var i = 0; i < said; i++) {
        w.worker.deliver(
          group,
          FakeDmDevice.claktorySays(
            group,
            'matt/phone',
            DmContent(kind: 'text', clientMessageId: 'm$i', extraJson: ''),
            epoch: w.worker.epochOf(group),
          ),
        );
      }
      // Still held; the worker's list says how far the group is.
      await w.ana.e2ee.sync();
      w.ana.now += E2ee.heldFor.inMilliseconds;
      await w.ana.e2ee.rejoinHeld(group);
      expect(w.ana.e2ee.held, isEmpty);
      return (ana: w.ana, group: group, held: held);
    }

    List<DmMembers> skippedLines(List<DmMessage> history) => [
      for (final message in history)
        if (message.members case final members? when members.skipped != null)
          members,
    ];

    test('joining again past messages it never read says so, where they '
        'were (row 654, never silent)', () async {
      final (:ana, :group, :held) = await heldThenSaid(2);
      final history = await ana.e2ee.history(group);
      final lines = skippedLines(history);
      expect(lines, hasLength(1));
      expect(lines.single.skipped, '');
      // Where the unread part began: after everything read before it.
      final line = history.singleWhere((each) => each.members == lines.single);
      expect(line.seq, held);
      expect(history.last, line);
      expect(
        history.where((each) => each.content.clientMessageId == 'm0'),
        isEmpty,
      );
    });

    test('joining again past only commits says nothing', () async {
      final (:ana, :group, held: _) = await heldThenSaid(0);
      expect(skippedLines(await ana.e2ee.history(group)), isEmpty);
    });
  });
}

/// Every leaf in [group] on [endpoint], checked or not.
Future<List<String>> _leafIds(FakeEndpoint endpoint, String group) =>
    endpoint.device.leafIds(groupId: dmUnb64(group));

Future<List<String>> _leaves(FakeEndpoint endpoint, String group) async => [
  for (final member in await endpoint.device.members(groupId: dmUnb64(group)))
    member.leafId,
];
