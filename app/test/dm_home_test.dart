import 'dart:async';

import 'dart:convert';
import 'dart:typed_data';

import 'package:factory_app/device_account.dart' show AccountClaktory;
import 'package:factory_app/people/dm_home.dart';
import 'package:factory_app/people/e2ee.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart'
    show DmDeliveryState, DmPeer, DmSendResult;
import 'package:factory_app/tunnel/api/dm.dart';
import 'package:flutter_test/flutter_test.dart';

import 'dm_fakes.dart';

// #451 H7: messages go through the person's home Claktory, not
// claktory.com (dms-no-server-storage.md; Mista's H3 contract). The worker
// still carries keys, Welcomes and commits; what's said comes and goes
// through the home, once this device went over to it.

DmContent _text(String id, String text) =>
    DmContent(kind: 'text', clientMessageId: id, text: text, extraJson: '');

Future<FakeEndpoint> _ready(
  FakeWorker worker,
  String account,
  String leaf,
) async {
  final endpoint = FakeEndpoint(worker, account, leaf);
  await endpoint.e2ee.start();
  await endpoint.e2ee.setUpFirst();
  expect(endpoint.e2ee.setup, DmSetup.ready);
  return endpoint;
}

/// What the engine does with a feed pushed to it, done.
Future<void> _settle() async {
  for (var i = 0; i < 30; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

/// Matt (his phone, over his home) and Ana (on the worker), in a
/// conversation.
class _World {
  final worker = FakeWorker();
  late final FakeEndpoint matt;
  late final FakeEndpoint ana;
  late final FakeHome home;
  late final String group;

  Future<void> setUp({bool drain = true}) async {
    matt = await _ready(worker, 'matt', 'phone');
    ana = await _ready(worker, 'ana', 'pixel');
    group = (await matt.e2ee.conversationGroup('v1', 3, 'ana'))!.groupId;
    await ana.e2ee.sync();
    home = FakeHome(worker);
    matt.e2ee.home = home;
    if (drain) {
      await matt.e2ee.sync();
      await _settle();
    }
  }

  /// The texts in [group]'s history on [endpoint], oldest first.
  Future<List<String?>> said(FakeEndpoint endpoint) async => [
    for (final message in await endpoint.e2ee.history(group))
      if (message.members == null) message.content.text,
  ];

  int feedReads() => worker.log
      .where((line) => line.startsWith('GET groups/$group/messages'))
      .length;
}

void main() {
  late _World world;
  setUp(() => world = _World());

  test('the worker feed is read to its end once, then the device follows '
      'its home, from the join for a group the worker numbered', () async {
    await world.setUp(drain: false);
    expect(world.matt.device.drained, isFalse);
    await world.matt.e2ee.sync();
    await _settle();
    expect(world.matt.device.drained, isTrue);
    final follow = world.home.follows.single;
    expect(follow.leafId, 'phone');
    // Its positions are the worker's: the home starts them at the join.
    expect(follow.cursors, isEmpty);

    // From now on, never the worker's feed.
    final before = world.feedReads();
    await world.ana.e2ee.say(world.group, _text('a1', 'hi Matt'));
    await world.matt.e2ee.sync();
    expect(world.feedReads(), before);
    expect(await world.said(world.matt), isEmpty);
    world.home.pull(world.group);
    await _settle();
    expect(await world.said(world.matt), ['hi Matt']);

    // Opened again, it follows from where it got to at the home.
    world.matt.relaunch();
    world.matt.e2ee.home = world.home;
    await world.matt.e2ee.start();
    await world.matt.e2ee.sync();
    await _settle();
    final again = world.home.follows.last;
    expect(again.cursors.single.seq, world.home.logOf(world.group).last.seq);
    expect(again.cursors.single.seq.toInt(), greaterThan(dmHomeSeqBase));
  });

  test("a Claktory's leave stays at the worker: taken from there before "
      'the home\'s commit that takes it in, and only then', () async {
    await world.setUp();
    world.home.pull(world.group);
    await _settle();
    final worker = world.worker;
    final epoch = worker.epochOf(world.group);
    final logged = world.home.logOf(world.group).length;
    final members = [
      for (final member in await world.matt.device.members(
        groupId: dmUnb64(world.group),
      ))
        '${member.account}/${member.leafId}',
    ];
    final reads = world.feedReads();
    // Ana's Claktory asks to leave, at the worker; the home never has it.
    worker.deliver(
      world.group,
      fakeEnvelope({
        't': 'proposal',
        'g': world.group,
        'epoch': epoch,
        'from': 'ana/mini#1',
        'p': 'leave-mini',
      }),
      advance: false,
    );
    world.home.pull(world.group);
    await _settle();
    expect(world.home.logOf(world.group), hasLength(logged));
    expect(world.matt.device.takenProposals, isEmpty);
    expect(world.feedReads(), reads, reason: 'nothing to take it in yet');

    // Ana's phone commits it by reference; the home passes the commit on.
    worker.deliver(
      world.group,
      fakeEnvelope({
        't': 'commit',
        'g': world.group,
        'epoch': epoch,
        'from': 'ana/pixel',
        'members': members,
        'takes': ['leave-mini'],
      }),
    );
    world.home.pull(world.group);
    await _settle();
    expect(world.feedReads(), reads + 1);
    expect(world.matt.device.takenProposals.map((each) => each.outcome), [
      DmOutcome.proposal,
    ]);
    final commit = world.matt.device.processed.last;
    expect(commit.outcome, DmOutcome.committed);
    expect(
      world.matt.device.proposalSeq(groupId: dmUnb64(world.group)),
      worker.mailboxOf(world.group).last['seq'],
    );

    // The next commit takes nothing more from the worker.
    worker.deliver(
      world.group,
      fakeEnvelope({
        't': 'commit',
        'g': world.group,
        'epoch': epoch + 1,
        'from': 'ana/pixel',
        'members': members,
      }),
    );
    world.home.pull(world.group);
    await _settle();
    expect(world.matt.device.takenProposals, hasLength(1));
    expect(world.matt.device.processed.last.outcome, DmOutcome.committed);
  });

  test('sent through the home, with this device\'s leaf and the raw MLS '
      'bytes; taken back from its feed, and acked', () async {
    await world.setUp();
    await world.matt.e2ee.say(world.group, _text('m1', 'hi Ana'));
    final send = world.home.sends.single;
    expect(send.leafId, 'phone');
    expect(send.clientMessageId, 'm1');
    // Raw bytes, not the worker's JSON envelope.
    expect(readFake(dmEnvelopeOf(send.envelope))['c'], isA<Map>());
    expect(world.matt.e2ee.sending['m1']!.state, DmSendState.sent);
    await _settle();
    final mine = await world.matt.e2ee.history(world.group);
    expect(mine.last.mine, isTrue);
    expect(mine.last.seq, dmHomeSeqBase + 1);
    expect(await world.matt.e2ee.unsent(world.group), isEmpty);
    // Passed on to Ana.
    await world.ana.e2ee.sync();
    expect(await world.said(world.ana), ['hi Ana']);
    // Acked a moment later, once for the group.
    await Future<void>.delayed(const Duration(milliseconds: 1100));
    expect(world.home.acks.single.through.single.seq.toInt(), mine.last.seq);
  });

  test('the home out of reach: it waits here (Waiting for your Claktory) '
      'and goes by itself when it\'s back', () async {
    await world.setUp();
    world.home.connected.value = false;
    await world.matt.e2ee.say(world.group, _text('m1', 'later'));
    expect(world.matt.e2ee.sending['m1']!.state, DmSendState.waiting);
    expect(world.matt.e2ee.sending['m1']!.problem, 'unreachable');
    expect(world.home.sends, isEmpty);
    world.home.connected.value = true;
    await _settle();
    expect(world.home.sends.single.clientMessageId, 'm1');
    expect(await world.said(world.matt), ['later']);
  });

  test("another Claktory than the home: it waits too, said as such", () async {
    await world.setUp();
    world.home.isHome = false;
    await world.matt.e2ee.say(world.group, _text('m1', 'held'));
    expect(world.matt.e2ee.sending['m1']!.state, DmSendState.waiting);
    expect(world.matt.e2ee.sending['m1']!.problem, 'notHome');
    expect(await world.matt.e2ee.unsent(world.group), hasLength(1));
  });

  test('too old for the home: sealed again under the same id, no new '
      'message', () async {
    await world.setUp();
    world.home.answer = DmSendResult.DM_SEND_RESULT_TOO_OLD;
    await world.matt.e2ee.say(world.group, _text('m1', 'again'));
    expect(world.home.sends.map((s) => s.clientMessageId), ['m1', 'm1']);
    expect(world.matt.e2ee.sending['m1']!.state, DmSendState.sent);
    await _settle();
    expect(await world.said(world.matt), ['again']);
  });

  test('a full home: waits, and tries again after the time it says', () async {
    await world.setUp();
    world.home
      ..answer = DmSendResult.DM_SEND_RESULT_HOME_FULL
      ..retryAfterMs = 50;
    await world.matt.e2ee.say(world.group, _text('m1', 'room?'));
    expect(world.matt.e2ee.sending['m1']!.problem, 'homeFull');
    await Future<void>.delayed(const Duration(milliseconds: 80));
    await _settle();
    expect(world.home.sends, hasLength(2));
    expect(await world.said(world.matt), ['room?']);
  });

  test("deliveries say how it's getting on past the home", () async {
    await world.setUp();
    await world.matt.e2ee.say(world.group, _text('m1', 'hi'));
    expect(world.matt.e2ee.delivery('m1'), isNull);
    // A friend's Claktory out of reach is the home's state, not a
    // delivery (Mista 32614).
    world.home
      ..peers.add(DmPeer(accountId: 'ana', reachable: false))
      ..said();
    await _settle();
    expect(world.matt.e2ee.homeState!.peers.single.reachable, isFalse);
    world.home.deliver(
      world.group,
      'm1',
      DmDeliveryState.DM_DELIVERY_STATE_DELIVERED,
    );
    await _settle();
    expect(
      world.matt.e2ee.delivery('m1')!.state,
      DmDeliveryState.DM_DELIVERY_STATE_DELIVERED,
    );
  });

  test("bounced, the conversation only lost members since: sent again by "
      'itself, naming what it resends', () async {
    await world.setUp();
    await world.matt.e2ee.say(world.group, _text('m1', 'stale'));
    await _settle();
    world.home.deliver(
      world.group,
      'm1',
      DmDeliveryState.DM_DELIVERY_STATE_BOUNCED,
    );
    await _settle();
    expect(world.matt.e2ee.askResend, isEmpty);
    final again = world.home.sends.last;
    expect(again.clientMessageId, isNot('m1'));
    final content =
        readFake(dmEnvelopeOf(again.envelope))['c']! as Map<String, Object?>;
    expect(content['text'], 'stale');
    expect(
      jsonDecode(content['extra']! as String),
      containsPair('resends', 'm1'),
    );
    // Bounced again (another device's copy, say): never sent a third time.
    world.home.deliver(
      world.group,
      'm1',
      DmDeliveryState.DM_DELIVERY_STATE_BOUNCED,
    );
    await _settle();
    expect(world.home.sends, hasLength(2));
  });

  test('bounced after someone joined: the person is asked, and only Resend '
      'sends it again', () async {
    await world.setUp();
    await world.matt.e2ee.say(world.group, _text('m1', 'stale'));
    await _settle();
    world.matt.device.keep(
      world.group,
      DmMessage(
        departed: false,
        seq: dmHomeSeqBase + 9,
        mine: false,
        fromAccount: '',
        fromLeaf: '',
        fromKindDevice: false,
        content: const DmContent(kind: '', clientMessageId: '', extraJson: ''),
        arrivedMs: 9,
        members: DmMembers(
          joined: const [],
          setUpAgain: const [],
          added: [
            DmMember(
              account: 'ana',
              leafId: 'ipad',
              kind: DmLeafKind.device,
              name: 'iPad',
              sinceMs: BigInt.one,
            ),
          ],
          removed: const [],
          keyChanged: const [],
        ),
      ),
    );
    world.home.deliver(
      world.group,
      'm1',
      DmDeliveryState.DM_DELIVERY_STATE_BOUNCED,
    );
    await _settle();
    expect(world.matt.e2ee.askResend, {'m1'});
    expect(world.home.sends, hasLength(1));
    await world.matt.e2ee.resend(world.group, 'm1');
    expect(world.matt.e2ee.askResend, isEmpty);
    expect(world.home.sends, hasLength(2));
  });

  test("bounced from another of the person's devices: this one leaves it "
      'to that one', () async {
    await world.setUp();
    await world.ana.e2ee.say(world.group, _text('a1', 'from Ana'));
    world.home.pull(world.group);
    await _settle();
    // A delivery for what this device didn't send (the home tells each
    // leaf its own; here, a message that isn't this device's).
    world.home.deliver(
      world.group,
      'a1',
      DmDeliveryState.DM_DELIVERY_STATE_BOUNCED,
    );
    await _settle();
    expect(world.matt.e2ee.askResend, isEmpty);
    expect(world.home.sends, isEmpty);
  });

  test(
    "routes: each friend's home from their signed list, with the "
    'conversations it may carry; the person\'s own list names this home',
    () async {
      await world.setUp(drain: false);
      // Ana's list names her home.
      final anaHome = Uint8List.fromList(List.filled(32, 8));
      final list = world.matt.device.lists['ana']!;
      world.matt.device.lists['ana'] = DmDeviceList(
        version: list.version,
        createdMs: list.createdMs,
        leaves: list.leaves,
        home: anaHome,
      );
      await world.matt.e2ee.sync();
      await _settle();
      final route = world.home.routesSaid.last.single;
      expect(route.accountId, 'ana');
      expect(route.homeEndpoint, anaHome);
      expect(route.groupIds.map(dmB64), [world.group]);
      // Matt's list named no home: the one the app shows, from now on.
      expect(world.matt.e2ee.homeEndpoint, world.home.endpointId);
      // A service that names a stranger in the conversation gets no route
      // to her home: who's in it is the MLS group's word (Fugo).
      world.worker.listAccounts(world.group, ['ana', 'eve', 'matt']);
      world.matt.device.lists['eve'] = DmDeviceList(
        version: BigInt.one,
        createdMs: BigInt.zero,
        leaves: const [],
        home: Uint8List.fromList(List.filled(32, 9)),
      );
      await world.matt.e2ee.sync();
      expect(
        world.matt.e2ee.groups[world.group]!.accounts,
        contains('eve'),
        reason: 'the service listed her',
      );
      expect(world.home.routesSaid.last.map((route) => route.accountId), [
        'ana',
      ]);
      // Said once while nothing changes.
      final said = world.home.routesSaid.length;
      await world.matt.e2ee.sync();
      expect(world.home.routesSaid, hasLength(said));
    },
  );

  // Zucchero's row 567: Ana keeps her messages on another Claktory. The
  // worker tells Matt's device only while it follows: changed while the
  // app was closed, it's read as following starts; changed while it
  // follows, the routes follow at once.
  group('routes follow a friend\'s home as it moves', () {
    Future<List<int>> anaMovesHome(int to) async {
      final anaHome = FakeHome(world.worker)
        ..endpointId = List<int>.filled(32, to);
      world.ana.e2ee.home = anaHome;
      await world.ana.e2ee.sync();
      await _settle();
      expect(world.ana.e2ee.homeEndpoint, anaHome.endpointId);
      world.worker.post({'kind': 'keys', 'account_id': 'ana'});
      return anaHome.endpointId;
    }

    List<int>? routedTo() => world.home.routesSaid.lastOrNull
        ?.where((route) => route.accountId == 'ana')
        .firstOrNull
        ?.homeEndpoint;

    test('moved while the app was closed: read as following starts', () async {
      await world.setUp();
      final moved = await anaMovesHome(8);
      expect(routedTo(), isNull);
      unawaited(world.matt.e2ee.follow());
      await _settle();
      expect(routedTo(), moved);
      world.matt.e2ee.dispose();
    });

    // Fugo: what the service lists alone never pins anyone.
    test('a stranger the service names in a conversation is never '
        'fetched or pinned as following starts', () async {
      await world.setUp();
      world.worker.listAccounts(world.group, ['ana', 'eve', 'matt']);
      world.worker.keys['eve'] = {...world.worker.keys['ana']!};
      await world.matt.e2ee.sync();
      unawaited(world.matt.e2ee.follow());
      await _settle();
      expect(world.worker.log, isNot(contains('GET keys/eve')));
      expect(world.matt.device.pin(account: 'eve'), isNull);
      expect(world.worker.log, contains('GET keys/ana'));
      world.matt.e2ee.dispose();
    });

    test('moved while it follows: routed there at once', () async {
      await world.setUp();
      unawaited(world.matt.e2ee.follow());
      await _settle();
      final moved = await anaMovesHome(8);
      await _settle();
      expect(routedTo(), moved);
      world.matt.e2ee.dispose();
    });
  });

  // Row 567 (Mista): Ana's home turns Matt's away. His device reads her
  // keys again and gives his home its routes again; again only after a
  // wait while it goes on, and from the start once it's over.
  test("a friend's home that turns the person's away: their keys read "
      'again, the routes given again, with a wait between', () async {
    await world.setUp();
    int reads() =>
        world.worker.log.where((line) => line == 'GET keys/ana').length;
    final routes = world.home.routesSaid.length;
    final before = reads();
    final peer = DmPeer(
      accountId: 'ana',
      homeEndpoint: List<int>.filled(32, 8),
      reachable: false,
      refused: dmTurnedAway,
    );
    world.home
      ..peers.add(peer)
      ..said();
    await _settle();
    expect(reads(), before + 1);
    expect(world.home.routesSaid, hasLength(routes + 1));
    // Said again at once: nothing more until the wait is over.
    world.home.said();
    await _settle();
    expect(reads(), before + 1);
    // Over: the next time it's said, at once.
    world.home
      ..peers.clear()
      ..said();
    await _settle();
    world.home
      ..peers.add(peer)
      ..said();
    await _settle();
    expect(reads(), before + 2);
    // Only the home's word: another Claktory's isn't.
    world.home
      ..peers.clear()
      ..said();
    await _settle();
    world.home
      ..isHome = false
      ..endpointId = List<int>.filled(32, 6)
      ..peers.add(peer)
      ..said();
    await _settle();
    expect(reads(), before + 2);
    world.matt.e2ee.dispose();
  });

  // Mista 39824: a device whose first Claktory isn't the person's home
  // stays on the worker until it's connected to the home, never reading a
  // home that isn't one.
  test('connected first to another Claktory than the home: it goes over '
      'only once that one is the home', () async {
    await world.setUp(drain: false);
    final list = world.matt.device.lists['matt']!;
    world.matt.device.lists['matt'] = DmDeviceList(
      version: list.version,
      createdMs: list.createdMs,
      leaves: list.leaves,
      home: Uint8List.fromList(List.filled(32, 7)),
    );
    world.home.isHome = false;
    await world.matt.e2ee.sync();
    await _settle();
    expect(world.matt.device.drained, isFalse);
    expect(world.home.follows, isEmpty);
    expect(world.matt.e2ee.homeIsHere, isFalse);
    // The person keeps them here: this Claktory says it's the home now.
    world.home
      ..isHome = true
      ..said();
    await _settle();
    await world.matt.e2ee.sync();
    await _settle();
    expect(world.matt.device.drained, isTrue);
    expect(world.home.follows, isNotEmpty);
  });

  // Row 569: the rail shows another of Matt's Claktories, each with its
  // own connection: the engine talks to the one shown, and back again.
  test('the app shows another Claktory: the engine talks to it, and to '
      'the home again once it is shown', () async {
    await world.setUp();
    final other = FakeHome(world.worker)
      ..isHome = false
      ..endpointId = List<int>.filled(32, 6);
    world.matt.e2ee.home = other;
    expect(world.matt.e2ee.homeState, isNull);
    await _settle();
    expect(other.routesSaid, isNotEmpty);
    expect(world.matt.e2ee.homeState!.endpointId, other.endpointId);
    expect(world.matt.e2ee.homeIsHere, isFalse);
    final follows = world.home.follows.length;
    world.matt.e2ee.home = world.home;
    await _settle();
    expect(world.matt.e2ee.homeIsHere, isTrue);
    expect(world.home.follows.length, follows + 1);
  });

  // Row 582 (Zucchero 40993): the home said it let go of a group past this
  // device's cursor; once joined again, what it said then no longer
  // stands: followed afresh from the new place, it joins again only if
  // the home says so again, never at every sync.
  test('joined again after the home let go: once, not at every sync', () async {
    await world.setUp();
    int rejoins() => [
      for (final send in world.home.sends)
        if (send.clientMessageId.isEmpty) send,
    ].length;
    final before = rejoins();
    world.home.letGo.add(world.group);
    world.home.sayState(world.group);
    await _settle();
    await world.matt.e2ee.sync();
    await _settle();
    final joined = rejoins();
    expect(joined, greaterThan(before));
    expect(world.home.letGo, isEmpty);
    for (var i = 0; i < 3; i++) {
      world.matt.now += const Duration(minutes: 10).inMilliseconds;
      await world.matt.e2ee.sync();
      await _settle();
    }
    expect(rejoins(), joined);
  });

  // Row 578 (Mista 40770): a message the home took, which hadn't come back
  // when the person's home moved, goes to the new home once this device is
  // connected to it, not only at the next launch.
  test('one the old home took goes to the new one', () async {
    await world.setUp();
    world.home.answer = DmSendResult.DM_SEND_RESULT_STORED;
    await world.matt.e2ee.say(world.group, _text('m1', 'before the move'));
    await _settle();
    expect(world.matt.e2ee.sending['m1']!.state, DmSendState.sent);
    final moved = FakeHome(world.worker)..endpointId = List<int>.filled(32, 6);
    world.matt.e2ee.home = moved;
    await _settle();
    await world.matt.e2ee.sync();
    await _settle();
    expect([for (final send in moved.sends) send.clientMessageId], ['m1']);
    // Once: back from the new home, it isn't sent again.
    await world.matt.e2ee.sync();
    await _settle();
    expect([for (final send in moved.sends) send.clientMessageId], ['m1']);
  });

  // Zucchero's row 572: the home's answer to routes came after the app
  // showed another Claktory; what the home said of itself never stands for
  // the one shown now.
  test('a late answer from the Claktory shown before never stands', () async {
    await world.setUp();
    final hold = world.home.holdRoutes = Completer<void>();
    world.matt.e2ee.home = world.home;
    await _settle();
    expect(world.home.routesSaid, isNotEmpty);
    final other = FakeHome(world.worker)
      ..isHome = false
      ..endpointId = List<int>.filled(32, 6);
    world.matt.e2ee.home = other;
    await _settle();
    expect(world.matt.e2ee.homeState!.endpointId, other.endpointId);
    hold.complete();
    await _settle();
    expect(world.matt.e2ee.homeState!.endpointId, other.endpointId);
    expect(world.matt.e2ee.homeIsHere, isFalse);
  });

  // Fugo 39894/39914: a Claktory the person's account doesn't list by the
  // endpoint the app dials it by (someone else's, paired with) is given
  // nothing: no leaf, pins or list change, no routes, no home, nothing
  // sent; once the account lists it, it all goes on.
  group("the person's own Claktory, as their account lists it", () {
    const dialled = 'aa11';
    late FakeClaktoryLink link;
    late List<AccountClaktory> listed;
    Object? unreadable;
    late Object version;

    Future<void> shown({Object? failing}) async {
      await world.setUp(drain: false);
      link = FakeClaktoryLink(world.worker, 'matt', leafId: 'mini#1');
      listed = [];
      unreadable = failing;
      version = world.worker.keys['matt']!['version']!;
      world.matt.e2ee
        ..ungatedForTests = false
        ..accountClaktories = () async {
          if (unreadable case final error?) throw error;
          return listed;
        }
        ..claktory = link
        ..show(const DmShown.remote(code: 'code-a', endpoint: dialled));
      await world.matt.e2ee.sync();
      await world.matt.e2ee.say(world.group, _text('m1', 'held'));
      await _settle();
    }

    void givenNothing() {
      expect(link.enrollments, isEmpty);
      expect(world.worker.keys['matt']!['version'], version);
      expect(world.home.routesSaid, isEmpty);
      expect(world.home.follows, isEmpty);
      expect(world.home.sends, isEmpty);
      expect(world.matt.device.drained, isFalse);
      expect(world.matt.e2ee.homeEndpoint, isNull);
      expect(world.matt.e2ee.sending['m1']!.state, DmSendState.waiting);
    }

    List<String> sent() => [
      for (final send in world.home.sends)
        if (send.clientMessageId.isNotEmpty) send.clientMessageId,
    ];

    test('another account\'s: given nothing, and what it says of itself '
        "doesn't count; listed after all, everything goes on", () async {
      await shown();
      givenNothing();
      expect(world.matt.e2ee.ownership, DmOwnership.notOurs);
      expect(world.matt.e2ee.sending['m1']!.problem, 'otherAccount');
      world.home.said();
      await _settle();
      expect(world.matt.e2ee.homeState, isNull);

      listed = [
        const AccountClaktory(id: 'mini', name: 'Mac mini', endpointId: 'AA11'),
      ];
      await world.matt.e2ee.sync();
      await _settle();
      await world.matt.e2ee.sync();
      await _settle();
      expect(world.matt.e2ee.ownership, DmOwnership.ours);
      expect(world.matt.device.drained, isTrue);
      expect(world.home.routesSaid, isNotEmpty);
      expect(sent(), ['m1']);
      expect(link.enrollments, isNotEmpty);
    });

    test("this Mac's own Claktory: by the id its leaf offer gives, on the "
        'list', () async {
      await world.setUp(drain: false);
      final link = FakeClaktoryLink(world.worker, 'matt', leafId: 'mini#1');
      var listed = <AccountClaktory>[];
      world.matt.e2ee
        ..ungatedForTests = false
        ..accountClaktories = (() async => listed)
        ..claktory = link
        ..show(const DmShown.thisMac());
      await world.matt.e2ee.sync();
      expect(world.matt.e2ee.ownership, DmOwnership.notOurs);
      listed = [const AccountClaktory(id: 'mini', name: 'Mac mini')];
      await world.matt.e2ee.sync();
      await _settle();
      expect(world.matt.e2ee.ownership, DmOwnership.ours);
    });

    // Fugo 40182: one on another machine paired with a direct code from
    // before the tunnel has no endpoint, and nothing authenticates which
    // Claktory it is: never confirmed, whatever it offers.
    test("this Mac's own, then one paired with a direct code: given "
        'nothing from then on', () async {
      await world.setUp(drain: false);
      link = FakeClaktoryLink(world.worker, 'matt', leafId: 'mini#1');
      listed = [const AccountClaktory(id: 'mini', name: 'Mac mini')];
      world.matt.e2ee
        ..ungatedForTests = false
        ..accountClaktories = (() async => listed)
        ..claktory = link
        ..show(const DmShown.thisMac());
      await world.matt.e2ee.sync();
      await _settle();
      expect(world.matt.e2ee.ownership, DmOwnership.ours);
      expect(world.matt.e2ee.homeIsOurs, isTrue);
      final before = sent();
      final enrolled = link.enrollments.length;
      final routes = world.home.routesSaid.length;

      world.matt.e2ee.show(
        const DmShown.remote(code: 'factory://old:7420', endpoint: ''),
      );
      await world.matt.e2ee.sync();
      await world.matt.e2ee.say(world.group, _text('m2', 'held'));
      await _settle();
      expect(world.matt.e2ee.ownership, DmOwnership.unconfirmable);
      expect(world.matt.e2ee.homeIsOurs, isFalse);
      expect(world.matt.e2ee.sending['m2']!.state, DmSendState.waiting);
      expect(world.matt.e2ee.sending['m2']!.problem, 'unconfirmable');
      expect(sent(), before);
      expect(link.enrollments, hasLength(enrolled));
      expect(world.home.routesSaid, hasLength(routes));
    });

    test('one paired with a direct code, offering a listed id: never '
        'confirmed', () async {
      await world.setUp(drain: false);
      link = FakeClaktoryLink(world.worker, 'matt', leafId: 'mini#1');
      version = world.worker.keys['matt']!['version']!;
      world.matt.e2ee
        ..ungatedForTests = false
        ..accountClaktories = (() async => [
          const AccountClaktory(id: 'mini', name: 'Mac mini'),
        ])
        ..claktory = link
        ..show(const DmShown.remote(code: 'factory://old:7420', endpoint: ''));
      await world.matt.e2ee.sync();
      await world.matt.e2ee.say(world.group, _text('m1', 'held'));
      await _settle();
      await world.matt.e2ee.sync();
      await _settle();
      givenNothing();
      expect(world.matt.e2ee.ownership, DmOwnership.unconfirmable);
      expect(world.matt.e2ee.sending['m1']!.problem, 'unconfirmable');
    });

    test('with no account list at all: given nothing', () async {
      await world.setUp(drain: false);
      link = FakeClaktoryLink(world.worker, 'matt', leafId: 'mini#1');
      version = world.worker.keys['matt']!['version']!;
      world.matt.e2ee
        ..ungatedForTests = false
        ..claktory = link
        ..show(const DmShown.thisMac());
      await world.matt.e2ee.sync();
      await world.matt.e2ee.say(world.group, _text('m1', 'held'));
      await _settle();
      givenNothing();
      expect(world.matt.e2ee.ownership, DmOwnership.notOurs);
    });

    test("the account's list can't be read: given nothing", () async {
      await shown(failing: StateError('offline'));
      givenNothing();
      expect(world.matt.e2ee.ownership, DmOwnership.unchecked);
    });

    test('listed by its endpoint, but offering another Claktory\'s leaf: '
        'never enrolled, the rest as for its own', () async {
      await shown();
      listed = [
        const AccountClaktory(id: 'other', name: 'Other', endpointId: dialled),
      ];
      await world.matt.e2ee.sync();
      await _settle();
      await world.matt.e2ee.sync();
      await _settle();
      expect(world.matt.e2ee.ownership, DmOwnership.ours);
      expect(link.enrollments, isEmpty);
      expect(sent(), ['m1']);
    });
  });

  // Zucchero's row 567: the app switched to another of Matt's Claktories,
  // no longer his home; what the last said of itself never stands for it.
  test('another Claktory connected: its own word on being the home', () async {
    await world.setUp();
    expect(world.matt.e2ee.homeIsHere, isTrue);
    world.home.connected.value = false;
    world.home
      ..isHome = false
      ..endpointId = List<int>.filled(32, 6);
    world.home.connected.value = true;
    await _settle();
    expect(world.matt.e2ee.homeState!.endpointId, world.home.endpointId);
    expect(world.matt.e2ee.homeIsHere, isFalse);
  });

  // Zucchero's row 549: a new friend not in the conversation yet joins by
  // a commit that reaches this device only through the homes, so the
  // route to their home is there before they're a member: the friend this
  // device's record of the conversation waits for, never anyone the
  // service lists.
  test("routes: a friend not joined yet has a route to their home, so "
      'their join can come', () async {
    final matt = await _ready(world.worker, 'matt', 'phone');
    final ana = await _ready(world.worker, 'ana', 'pixel');
    world.worker.packages.remove('ana/pixel');
    world.worker.lastResort.remove('ana/pixel');
    final group = (await matt.e2ee.conversationGroup('v1', 3, 'ana'))!.groupId;
    final anaHome = Uint8List.fromList(List.filled(32, 8));
    final list = matt.device.lists['ana']!;
    matt.device.lists['ana'] = DmDeviceList(
      version: list.version,
      createdMs: list.createdMs,
      leaves: list.leaves,
      home: anaHome,
    );
    final home = FakeHome(world.worker);
    matt.e2ee.home = home;
    await matt.e2ee.sync();
    await matt.e2ee.sync();
    await _settle();
    expect(matt.e2ee.joining, {group: 'ana'});
    final route = home.routesSaid.last.single;
    expect(route.accountId, 'ana');
    expect(route.homeEndpoint, anaHome);
    expect(route.groupIds.map(dmB64), [group]);
    expect(ana.e2ee.setup, DmSetup.ready);
  });

  // Row 551: Matt's own commit was taken by the worker just as his app
  // stopped, before it merged it or handed it to his home. What Ana says
  // after it reaches him through his home: held, never acked unread,
  // until the commit comes from the worker; then read.
  test('a commit the home never carried comes from the worker, and what '
      'waited for it is read, never passed over', () async {
    await world.setUp();
    final commit = await world.matt.device.removeMembers(
      groupId: dmUnb64(world.group),
      leafIds: const [],
    );
    world.worker.deliver(world.group, commit.commit);
    world.home.passOver(world.group);
    await world.ana.e2ee.sync();
    await world.ana.e2ee.say(world.group, _text('a1', 'after the commit'));
    world.home.pull(world.group);
    await _settle();
    await world.matt.e2ee.sync();
    await _settle();
    expect(await world.said(world.matt), ['after the commit']);
    expect(
      world.matt.device.takenWorkerCommits.map((p) => p.outcome),
      contains(DmOutcome.committed),
    );
    expect(world.matt.e2ee.behind, isEmpty);
  });

  // Zucchero's row 568: a commit made from a device connected to another
  // of the person's Claktories reaches only the worker, and nothing is said
  // after it: read from there at the next sync all the same.
  test('a commit only the worker has, with nothing after it, is read from '
      'there at the next sync', () async {
    await world.setUp();
    world.home.pull(world.group);
    await _settle();
    final epoch = world.worker.epochOf(world.group);
    world.worker.deliver(
      world.group,
      fakeEnvelope({
        't': 'commit',
        'g': world.group,
        'epoch': epoch,
        'from': 'ana/pixel',
        'members': ['matt/phone', 'ana/pixel'],
      }),
    );
    world.home.passOver(world.group);
    await world.matt.e2ee.sync();
    await _settle();
    final mine = (await world.matt.device.groups()).firstWhere(
      (g) => dmB64(g.groupId) == world.group,
    );
    expect(mine.epoch.toInt(), epoch + 1);
    // Read once for that epoch: the next sync doesn't read it again.
    final reads = world.feedReads();
    await world.matt.e2ee.sync();
    expect(world.feedReads(), reads);
  });

  // Row 553: the same commit, in a group where nothing else comes: settled
  // against the worker at the next launch, never left unmerged.
  test('a pending commit the worker took is merged at the next launch, '
      'with nothing else to bring it', () async {
    await world.setUp();
    final commit = await world.matt.device.removeMembers(
      groupId: dmUnb64(world.group),
      leafIds: const [],
    );
    world.worker.deliver(world.group, commit.commit);
    world.home.passOver(world.group);
    final before = world.worker.epochOf(world.group) - 1;
    expect(
      (await world.matt.device.groups())
          .firstWhere((g) => dmB64(g.groupId) == world.group)
          .pending,
      isTrue,
    );
    world.matt.relaunch();
    world.matt.e2ee.home = world.home;
    await world.matt.e2ee.start();
    await world.matt.e2ee.sync();
    await _settle();
    final mine = (await world.matt.device.groups()).firstWhere(
      (g) => dmB64(g.groupId) == world.group,
    );
    expect(mine.pending, isFalse);
    expect(mine.epoch.toInt(), before + 1);
  });

  // Mista's gap: the worker is past this device, but the commit there is
  // one it refuses, so nothing brings it there: it joins again, never
  // waits for good.
  test(
    'a worker past this device with nothing it can take: joins again',
    () async {
      await world.setUp();
      final epoch = world.worker.epochOf(world.group);
      world.worker.deliver(
        world.group,
        fakeEnvelope({
          't': 'commit',
          'g': world.group,
          'epoch': epoch,
          'from': 'ana/pixel',
          'members': ['matt/phone', 'ana/pixel'],
          'bad': true,
        }),
      );
      world.home.passOver(world.group);
      world.worker.deliver(
        world.group,
        fakeEnvelope({
          't': 'app',
          'g': world.group,
          'epoch': epoch + 1,
          'from': 'ana/pixel',
          'c': {'kind': 'text', 'id': 'x', 'text': 'past the bad one'},
        }),
      );
      world.home.pull(world.group);
      await _settle();
      await world.matt.e2ee.sync();
      await _settle();
      expect(
        world.matt.device.takenWorkerCommits.map((p) => p.outcome),
        contains(DmOutcome.dropped),
      );
      expect(
        world.worker.log,
        contains('GET groups/${world.group}/group-info'),
      );
      expect(world.matt.e2ee.behind, isEmpty);
    },
  );

  test("something from an epoch no one has is passed over and said, never "
      'left waiting', () async {
    await world.setUp();
    final group = world.matt.e2ee.groups[world.group]!;
    world.worker.deliver(
      world.group,
      fakeEnvelope({
        't': 'app',
        'g': world.group,
        'epoch': 9,
        'from': 'ana/pixel',
        'c': {'kind': 'text', 'id': 'x', 'text': 'from nowhere'},
      }),
    );
    world.home.pull(world.group);
    await _settle();
    await world.matt.e2ee.sync();
    await _settle();
    expect(world.matt.e2ee.behind, isEmpty);
    expect(await world.said(world.matt), isEmpty);
    // Said where it was: something here couldn't be read.
    final history = await world.matt.e2ee.history(world.group);
    expect(history.where((m) => m.members != null), hasLength(1));
    expect(group.groupId, world.group);
  });

  test('an agent\'s approved message is sealed only once approved, and an '
      'agent\'s message left from before the home never goes', () async {
    await world.setUp(drain: false);
    // Sealed before an approval, the way it was before the home.
    await world.matt.device.send(
      groupId: dmUnb64(world.group),
      content: const DmContent(
        kind: 'text',
        clientMessageId: 'old',
        text: 'never approved',
        byAgent: DmByAgent(
          claktoryId: 'mac-mini',
          agentId: 'june',
          incarnation: 'inc-1',
          name: 'June',
        ),
        extraJson: '',
      ),
      nowMs: 1,
    );
    await world.matt.e2ee.sync();
    await _settle();
    expect(world.home.sends, isEmpty);
    expect(await world.matt.e2ee.unsent(world.group), isEmpty);
  });
}
