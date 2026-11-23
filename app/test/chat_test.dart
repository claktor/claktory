import 'dart:async';
import 'dart:convert';

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/controls.dart' show Button, showModal;
import 'package:factory_app/device_account.dart';
import 'package:factory_app/inbox.dart';
import 'package:factory_app/people/chat.dart';
import 'package:factory_app/people/chat_view.dart';
import 'package:factory_app/people/contacts.dart';
import 'package:factory_app/people/e2ee.dart';
import 'package:factory_app/people/people_view.dart';
import 'package:factory_app/people/report_form.dart';
import 'package:factory_app/people/review.dart';
import 'package:factory_app/people/review_view.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
import 'package:factory_app/tunnel/api/dm.dart'
    show DmByAgent, DmContent, DmLeafKind, DmMember, DmMembers;
import 'package:factory_app/user_apps/session.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'dm_fakes.dart';
import 'harness.dart';
import 'phone_test.dart' show workspace;

// D3c, end to end since E5: a chat with a friend against E2's contract
// (claktory-user-apps README, "End to end"), with the worker and the app's
// Rust stood in for by dm_fakes.dart: history from this device's own store
// as the feed comes, Sending… until a message comes back, an unknown
// outcome retried with the same bytes, an ended conversation read-only,
// the friend's key change holding sending; and the service's other routes
// (conversations, proposals, agents' drafts) from [_Service].

final _now = DateTime.now().millisecondsSinceEpoch;

/// The service's routes that aren't end to end: the conversation, what
/// agents proposed and drafted, and its outbox; [push] answers the
/// request held for changes; [log] has each call but the held ones.
class _Service {
  final log = <String>[];
  bool active = true;

  /// What agents proposed and drafted (D3d), and how approving answers
  /// (null: it's sent).
  final proposals = <Map<String, Object?>>[];
  final agentDrafts = <Map<String, Object?>>[];
  Object? Function(Map<String, Object?> body)? onApprove;
  var _cursor = 1;
  Completer<List<Map<String, Object?>>>? _held;

  /// Answers the held request for changes with [events].
  void push(List<Map<String, Object?>> events) {
    _held?.complete(events);
    _held = null;
  }

  Map<String, Object?> conversation() => {
    'id': 'v1',
    'contact_id': 'c1',
    'contact_generation': 3,
    'with': {
      'name': 'Ana',
      'avatar_seed': 'aaaa',
      'tag': 'K7QF2M',
      'account_id': 'ana',
    },
    'active': active,
    'since_ms': _now,
    'ended_ms': 0,
    'last_seq': 0,
  };

  Future<Map<String, Object?>> call(
    String method,
    String path, [
    Map<String, Object?>? body,
  ]) async {
    final uri = Uri.parse(path);
    final query = {...uri.queryParameters}..remove('request_nonce');
    final rest = {...?body}..remove('request_nonce');
    if (method == 'GET' && uri.path.endsWith('/sync')) {
      if (query.containsKey('since')) {
        final events = await (_held = Completer()).future;
        return {
          'events': events,
          'next_cursor': 'c${++_cursor}',
          'has_more': false,
        };
      }
      log.add('GET sync');
      return {
        'conversations': [conversation()],
        'proposals': proposals,
        'agent_drafts': [
          for (final draft in agentDrafts)
            {...draft, 'conversation_id': 'v1'}..remove('envelope'),
        ],
        'cursor': 'c$_cursor',
      };
    }
    if (method == 'GET' && uri.path.endsWith('/proposals')) {
      log.add('GET proposals');
      return {'proposals': proposals};
    }
    if (method == 'GET' && uri.path.endsWith('/agent-drafts')) {
      log.add('GET agent-drafts');
      return {'drafts': agentDrafts};
    }
    if (method == 'DELETE') {
      final id = uri.path.split('/').last;
      log.add('DELETE agent-draft $id');
      agentDrafts.removeWhere((draft) => draft['id'] == id);
      return {};
    }
    if (method == 'GET' && uri.path.endsWith('/conversations')) {
      log.add('GET conversations');
      return {
        'conversations': [conversation()],
      };
    }
    final route = uri.path.split('/').last;
    log.add('$method $route ${jsonEncode(rest)}');
    switch (route) {
      case 'approve':
        final answer = onApprove?.call(rest);
        if (answer is Exception) throw answer;
        final id = uri.pathSegments[uri.pathSegments.length - 2];
        proposals.removeWhere((each) => each['id'] == id);
        return {'proposal_id': id, 'state': 'sent'};
      case 'decline':
        final id = uri.pathSegments[uri.pathSegments.length - 2];
        proposals.removeWhere((each) => each['id'] == id);
        return {'proposal_id': id, 'state': 'declined'};
      case 'delete':
        return {'deleted_seq': rest['through_seq']};
      case 'reports':
        return {
          'id': 'r1',
          'about': {'name': 'Ana', 'avatar_seed': 'aaaa', 'tag': 'K7QF2M'},
          'subject': 'contact',
          'reason': rest['reason'],
          'state': 'received',
          'created_ms': _now,
          'message': '',
        };
    }
    throw StateError('no route for $method $path');
  }
}

/// Whether [path] is one of the worker's end-to-end routes.
bool _endToEnd(String method, String path) {
  final route = Uri.parse(path).path.replaceFirst('/v1/apps/dms/', '');
  return route.startsWith('keys/') ||
      route.startsWith('me/keys') ||
      route.startsWith('me/handoffs') ||
      route.startsWith('key-packages') ||
      route.startsWith('groups') ||
      route.startsWith('welcomes') ||
      route == '/v1/apps/grants' ||
      (method == 'POST' &&
          RegExp(r'^conversations/[^/]+/messages$').hasMatch(route));
}

/// Matt (the person, on his phone) and Ana, each set up, with the service.
class _World {
  _World() {
    matt = FakeEndpoint(
      worker,
      'matt',
      'phone',
      route: (e2) =>
          (method, path, [body]) => _endToEnd(method, path)
          ? e2(method, path, body)
          : service.call(method, path, body),
    );
    ana = FakeEndpoint(worker, 'ana', 'pixel', name: 'Pixel');
  }

  final worker = FakeWorker();
  final service = _Service();
  late final FakeEndpoint matt;
  late final FakeEndpoint ana;

  Future<void> setUp() async {
    for (final endpoint in [matt, ana]) {
      await endpoint.e2ee.start();
      await endpoint.e2ee.setUpFirst();
    }
    // Matt's Claktory, enrolled, with its channel.
    matt.e2ee.claktory = FakeClaktoryLink(worker, 'matt', leafId: 'mac-mini#1');
    await matt.e2ee.sync();
    _channel = matt.e2ee.groups.values
        .singleWhere((each) => each.kind == 'channel')
        .groupId;
  }

  ChatApi api() => ChatApi(matt.e2ee.session);

  ChatThread thread({bool active = true, String Function()? newId}) =>
      ChatThread(
        api(),
        matt.e2ee,
        _conversation(active: active),
        newId: newId,
        readEvery: Duration.zero,
      );

  /// The conversation's group, as Matt's device made it.
  String get group => matt.e2ee.groups.values
      .firstWhere((each) => each.kind == 'conversation')
      .groupId;

  /// Ana started over [times] times while Matt's phone was away (row
  /// 654): it never saw her new keys, and the first commit it meets is
  /// under one she has replaced since; [said] went after it. The worker
  /// serves her newest key and lists the ones before it.
  void anaStartedOver(int times, {List<String> said = const []}) {
    String key(int n) => dmB64(List.filled(32, 40 + n));
    final first = worker.keys['ana']!['identity_key']! as String;
    worker.deliver(
      group,
      fakeEnvelope({
        't': 'commit',
        'g': group,
        'epoch': worker.epochOf(group),
        'from': 'ana/pixel',
        'members': ['matt/phone', 'ana/pixel'],
        'id': key(1),
      }),
      // The worker's GroupInfo stays the one to join again from.
      advance: false,
    );
    for (final text in said) {
      worker.deliver(
        group,
        FakeDmDevice.claktorySays(
          group,
          'ana/pixel',
          DmContent(kind: 'text', clientMessageId: 'a-$text', extraJson: ''),
          epoch: worker.epochOf(group),
        ),
      );
    }
    worker.retired['ana'] = {first, for (var n = 1; n < times; n++) key(n)};
    worker.keys['ana']!['identity_key'] = key(times);
  }

  /// Ana says [texts] in the conversation, and Matt's device takes them in.
  Future<void> fromAna(List<String> texts) async {
    await ana.e2ee.sync();
    for (final text in texts) {
      await ana.e2ee.say(
        group,
        DmContent(
          kind: 'text',
          clientMessageId: 'a-$text',
          text: text,
          extraJson: '',
        ),
      );
    }
    await matt.e2ee.sync();
  }
}

Map<String, Object?> _proposal({
  String id = 'p1',
  int revision = 1,
  String text = 'See you at 6',
  String agentName = 'June',
  String conversation = 'v1',
  String? sealedFor,
  String from = _mini,
}) => {
  'id': id,
  'revision': revision,
  'state': 'pending',
  'conversation_id': conversation,
  'contact_generation': 3,
  'from': {
    'claktory_id': 'mac-mini',
    'agent_id': 'june',
    'agent_incarnation': 'inc-1',
    'agent_name': agentName,
  },
  // Sealed by the Claktory's leaf in Matt's channel with it.
  'envelope': jsonDecode(
    FakeDmDevice.claktorySays(
      _channel,
      from,
      DmContent(
        kind: 'proposal',
        clientMessageId: 'cm-$id-$revision',
        text: text,
        extraJson: jsonEncode({
          'conversation_id': sealedFor ?? conversation,
          'contact_generation': 3,
          'agent_id': 'june',
          'incarnation': 'inc-1',
          'agent_name': agentName,
          'sequence': revision,
        }),
      ),
    ),
  ),
  'epoch': 1,
  // The service's word, which approving never takes.
  'proposal_digest': 'f' * 64,
  'created_ms': _now,
  'expires_ms': _now + 3 * 86400000,
};

/// Matt's channel with his Claktory mac-mini, and its leaf there.
var _channel = '';
const _mini = 'matt/mac-mini#1';

/// An agent's draft in [conversation], as the service lists it.
Map<String, Object?> _agentDraft({
  String id = 'd1',
  String name = 'June',
  String text = 'Maybe Friday?',
  String conversation = 'v1',
}) => {
  'id': id,
  'agent': {'name': name, 'claktory_id': 'mac-mini'},
  'revision': 1,
  'envelope': jsonDecode(
    FakeDmDevice.claktorySays(
      _channel,
      _mini,
      DmContent(
        kind: 'agent_draft',
        clientMessageId: id,
        text: text,
        byAgent: DmByAgent(
          claktoryId: 'mac-mini',
          agentId: name.toLowerCase(),
          incarnation: 'inc-1',
          name: name,
        ),
        extraJson: jsonEncode({
          'draft_id': id,
          'conversation_id': conversation,
          'contact_generation': 3,
        }),
      ),
    ),
  ),
};

/// What Matt's device sent approving: its body, and the message sealed in
/// it as the conversation's group has it.
({Map<String, Object?> body, Map<String, Object?> sealed}) _approval(
  String line,
) {
  final body = jsonDecode(
    line.substring('POST approve '.length),
  ) as Map<String, Object?>;
  return (body: body, sealed: readFake(jsonEncode(body['envelope'])));
}

const _ana = PeoplePerson(
  name: 'Ana',
  avatarSeed: 'aaaa',
  tag: 'K7QF2M',
  accountId: 'ana',
);

DmConversation _conversation({bool active = true}) => DmConversation(
  id: 'v1',
  contactId: 'c1',
  generation: 3,
  person: _ana,
  active: active,
  sinceMs: _now,
  endedMs: active ? 0 : _now,
  lastSeq: 0,
);

Future<void> _settle() async {
  for (var i = 0; i < 5; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void main() {
  late _World world;
  setUp(() async {
    world = _World();
    await world.setUp();
  });

  test('a conversation and whom it is with, read', () {
    final conversation = DmConversation.fromJson({
      'id': 'v1',
      'contact_id': 'c1',
      'contact_generation': 2,
      'with': {
        'name': 'Ana',
        'avatar_seed': 'aaaa',
        'tag': 'K7QF2M',
        'account_id': 'ana',
      },
      'active': false,
      'since_ms': 1,
      'ended_ms': 2,
      'last_seq': 9,
    });
    expect(conversation.person.label, 'Ana · K7QF2M');
    expect(conversation.person.accountId, 'ana');
    expect(conversation.generation, 2);
    expect(conversation.active, isFalse);
  });

  test('opening a new conversation makes its group with the friend; the '
      'history is this device\'s, newest page first, then earlier', () async {
    final thread = world.thread();
    await thread.follow();
    expect(thread.group, isNotEmpty);
    expect(thread.friend, 'ana');
    expect(thread.canSend, isTrue);
    await world.fromAna([for (var i = 1; i <= 55; i++) 'message $i']);
    await _settle();
    expect(thread.messages.first.text, 'message 6');
    expect(thread.messages.last.text, 'message 55');
    expect(thread.messages.last.mine, isFalse);
    expect(thread.hasEarlier, isTrue);
    await thread.loadEarlier();
    expect(thread.messages, hasLength(55));
    expect(thread.hasEarlier, isFalse);
    // Read through the newest, on Matt's devices only.
    await Future<void>.delayed(Duration.zero);
    await _settle();
    expect((await world.matt.e2ee.view(thread.group)).readSeq, greaterThan(0));
    thread.dispose();
  });

  test('Sending… until it comes back, then in the history', () async {
    final thread = world.thread(newId: () => 'id' * 11);
    await thread.follow();
    final sending = thread.send('  hello  ');
    expect(thread.outgoing.single.state, OutgoingState.sending);
    expect(outgoingLine(thread.outgoing.single), 'Sending…');
    await sending;
    await world.matt.e2ee.sync();
    await _settle();
    expect(thread.outgoing, isEmpty);
    expect(thread.messages.single.text, 'hello');
    expect(thread.messages.single.mine, isTrue);
    // Ana reads it.
    await world.ana.e2ee.sync();
    expect(
      (await world.ana.e2ee.history(thread.group)).single.content.text,
      'hello',
    );
    thread.dispose();
  });

  test('no answer: unknown; Try again sends the same bytes', () async {
    final thread = world.thread(newId: () => 'm1');
    await thread.follow();
    world.worker.refuse = (route) => route.startsWith('POST conversations/')
        ? const AccountServiceUnreachable('offline')
        : null;
    await thread.send('hello');
    final message = thread.outgoing.single;
    expect(message.state, OutgoingState.unknown);
    expect(outgoingLine(message), contains("won't be sent twice"));
    world.worker.refuse = null;
    await thread.retry(thread.outgoing.single);
    await world.matt.e2ee.sync();
    await _settle();
    expect(thread.outgoing, isEmpty);
    expect(thread.messages.single.text, 'hello');
    thread.dispose();
  });

  test('blocked or removed first: not sent, and nothing more goes', () async {
    final thread = world.thread();
    await thread.load();
    world.worker.refuse = (route) => route.startsWith('POST conversations/')
        ? const AccountServiceError(410, 'closed', 'closed')
        : null;
    await thread.send('hello');
    expect(thread.outgoing.single.state, OutgoingState.refused);
    expect(
      outgoingLine(thread.outgoing.single),
      "Not sent: you're no longer friends.",
    );
    expect(thread.canSend, isFalse);
    await thread.send('again');
    expect(thread.outgoing, hasLength(1));
    thread.dispose();
  });

  test('out of space: not sent, and why', () async {
    final thread = world.thread();
    await thread.load();
    world.worker.refuse = (route) => route.startsWith('POST conversations/')
        ? const AccountServiceError(413, 'storage_full', 'full')
        : null;
    await thread.send('hello');
    expect(outgoingLine(thread.outgoing.single), contains('out of space'));
    thread.dispose();
  });

  test("the friend's key changed: nothing goes until the person has seen "
      'it', () async {
    final thread = world.thread();
    await thread.load();
    world.worker.keys['ana']!['identity_key'] = dmB64(List.filled(32, 9));
    await world.matt.e2ee.refreshKeys('ana');
    expect(thread.keyChanged, isTrue);
    await thread.send('hello');
    final refused = thread.outgoing.single;
    expect(refused.state, OutgoingState.refused);
    expect(
      outgoingLine(refused, friend: 'Ana'),
      "Not sent: Ana's security key changed. Read the note above, then send "
      'it again.',
    );
    await thread.keySeen();
    expect(thread.keyChanged, isFalse);
    thread.dispose();
  });

  test('the friend started over on a new key: nothing goes until their new '
      'device has taken the old ones out (row 630, Fugo)', () async {
    final thread = world.thread();
    await thread.load();
    await world.fromAna(['before']);
    expect(thread.canSend, isTrue);
    // Ana's only device lost its keys: she starts over on a new one,
    // which hasn't rejoined the chat yet.
    final commits = 'POST groups/${world.group}/commits';
    world.worker.refuse = (route) => route == commits
        ? const AccountServiceError(503, 'unavailable', 'unavailable')
        : null;
    final tablet = FakeEndpoint(world.worker, 'ana', 'tablet', name: 'Tablet')
      ..skipSecrets(100);
    await tablet.e2ee.start();
    expect(await tablet.e2ee.startNewKey(), isNull);
    await world.matt.e2ee.refreshKeys('ana');
    await world.matt.e2ee.sync();
    expect(thread.waitingFor, 'ana');
    expect(thread.canSend, isFalse);
    await thread.send('hello');
    expect(thread.outgoing, isEmpty);
    // Her new device rejoins and takes the old one out: sending goes on.
    world.worker.refuse = null;
    await tablet.e2ee.sync();
    await world.matt.e2ee.sync();
    expect(thread.waitingFor, isNull);
    expect(thread.canSend, isTrue);
    thread.dispose();
  });

  group('a friend behind more than one start-over (row 654)', () {
    test("Ana's Mac, stuck across two it missed: joins again, and says what "
        "it couldn't read", () async {
      final thread = world.thread();
      await thread.load();
      await world.fromAna(['before']);
      world.anaStartedOver(2, said: ['missed']);
      await world.matt.e2ee.sync();
      expect(world.matt.e2ee.held, isEmpty);
      expect(world.matt.device.retiredHere['ana'], {
        dmB64(List.filled(32, 41)),
      });
      await thread.load();
      final lines = [
        for (final message in thread.messages)
          if (message.members case final members?)
            ...memberLines(members, me: 'matt', friend: 'Ana'),
      ];
      expect(
        lines,
        contains(
          "Some messages sent when Ana's security key changed can't be "
          'shown on this device.',
        ),
      );
      expect(thread.messages.map((each) => each.text), contains('before'));
      expect(
        thread.messages.map((each) => each.text),
        isNot(contains('missed')),
      );
      thread.dispose();
    });

    test('a service listing the key it serves as replaced retires nothing: '
        'held, never joined again past it', () async {
      final thread = world.thread();
      await thread.load();
      addTearDown(thread.dispose);
      world.anaStartedOver(2, said: ['missed']);
      final served = world.worker.keys['ana']!['identity_key']! as String;
      world.worker.retired['ana']!.add(served);
      final before = world.worker.mailboxOf(world.group).length;
      await world.matt.e2ee.sync();
      expect(world.matt.e2ee.held.keys, [world.group]);
      // Nothing goes while it's held: it would only fail.
      expect(thread.canSend, isFalse);
      expect(world.matt.device.retiredHere['ana'], isNull);
      expect(world.worker.mailboxOf(world.group), hasLength(before));
    });
  });

  test('the draft goes to the person\'s other devices', () async {
    final thread = world.thread();
    final drafts = <String>[];
    thread.onDraft = drafts.add;
    await thread.load();
    await thread.saveDraft('hel');
    expect(thread.draft, 'hel');
    expect((await world.matt.e2ee.view(thread.group)).draft, 'hel');
    thread.dispose();
  });

  test('deleting: through the last message, on every device', () async {
    final thread = world.thread();
    await thread.follow();
    await world.fromAna(['one', 'two']);
    await _settle();
    expect(thread.messages, hasLength(2));
    await thread.deleteHistory();
    expect(world.service.log, contains('POST delete {"through_seq":3}'));
    final self = world.matt.e2ee.groups.values.firstWhere(
      (group) => group.kind == 'self',
    );
    expect(
      [
        for (final item in world.worker.mailboxOf(self.groupId))
          readFake(jsonEncode(item['envelope']))['c'],
      ].whereType<Map<String, Object?>>().map((c) => c['kind']),
      contains('delete'),
    );
    thread.dispose();
  });

  test('who can read the chat changed: a quiet line per leaf', () {
    DmMember member(String account, String name, DmLeafKind kind) => DmMember(
      account: account,
      leafId: name,
      kind: kind,
      name: name,
      sinceMs: BigInt.zero,
    );
    expect(
      memberLines(
        DmMembers(
          setUpAgain: const [],
          added: [
            member('ana', 'Pixel', DmLeafKind.device),
            member('ana', 'mac-mini', DmLeafKind.claktory),
            member('matt', 'iPad', DmLeafKind.device),
            member('matt', 'studio', DmLeafKind.claktory),
          ],
          removed: [member('ana', 'Old phone', DmLeafKind.device)],
          keyChanged: ['ana', 'matt'],
        ),
        me: 'matt',
        friend: 'Ana',
      ),
      [
        "Ana's security key changed",
        'Ana added a device: Pixel',
        "Ana's Claktory mac-mini can now read this chat",
        'You added a device: iPad',
        'Your Claktory studio can now read this chat',
        'Ana removed a device: Old phone',
      ],
    );
  });

  test('this device removed by another of the person\'s says so, never '
      '"You removed a device" with its own name (row 199)', () {
    DmMember member(String leaf, String name) => DmMember(
      account: 'matt',
      leafId: leaf,
      kind: DmLeafKind.device,
      name: name,
      sinceMs: BigInt.zero,
    );
    expect(
      memberLines(
        DmMembers(
          setUpAgain: const [],
          added: const [],
          removed: [
            member('studio-a', "Ana's Mac Studio"),
            member('studio-b', "Ana's Mac Studio"),
          ],
          keyChanged: const [],
        ),
        me: 'matt',
        friend: 'Ana',
        here: 'studio-a',
      ),
      [
        'This device was removed from your account',
        "You removed a device: Ana's Mac Studio",
      ],
    );
  });

  test("a friend's device under this device's id is the friend's (row "
      '671)', () {
    expect(
      memberLines(
        DmMembers(
          setUpAgain: const [],
          added: const [],
          removed: [
            DmMember(
              account: 'ana',
              leafId: 'studio-a',
              kind: DmLeafKind.device,
              name: 'Pixel',
              sinceMs: BigInt.zero,
            ),
          ],
          keyChanged: const [],
        ),
        me: 'matt',
        friend: 'Ana',
        here: 'studio-a',
      ),
      ['Ana removed a device: Pixel'],
    );
  });

  test('a device installed again is one line, never a removal and an add '
      '(row 202, Trish)', () {
    DmMember member(String account, String name) => DmMember(
      account: account,
      leafId: name,
      kind: DmLeafKind.device,
      name: name,
      sinceMs: BigInt.zero,
    );
    expect(
      memberLines(
        DmMembers(
          setUpAgain: [member('ana', 'iPhone'), member('matt', 'iPad')],
          added: const [],
          removed: const [],
          keyChanged: const [],
        ),
        me: 'matt',
        friend: 'Ana',
      ),
      ['Ana set up a device again: iPhone', 'You set up a device again: iPad'],
    );
  });

  test('messages it joined again past: a quiet line saying why, if it '
      'knows (row 654)', () {
    List<String> skipped(String because) => memberLines(
      DmMembers(
        setUpAgain: const [],
        added: const [],
        removed: const [],
        keyChanged: const [],
        skipped: because,
      ),
      me: 'matt',
      friend: 'Ana',
    );
    expect(skipped('ana'), [
      "Some messages sent when Ana's security key changed can't be shown on "
          'this device.',
    ]);
    expect(skipped('matt'), [
      "Some messages sent when your security key changed can't be shown on "
          'this device.',
    ]);
    expect(skipped(''), [
      "Some messages sent while this device was away can't be shown on it.",
    ]);
  });

  group("agents' suggestions (D3d)", () {
    test('hidden characters show by their codes; the rest as it is', () {
      expect(reviewRuns('a\u202Eb\u200Bc\nd\u{E0041}'), [
        (text: 'a', hidden: false),
        (text: '‹U+202E›', hidden: true),
        (text: 'b', hidden: false),
        (text: '‹U+200B›', hidden: true),
        (text: 'c\nd', hidden: false),
        (text: '‹U+E0041›', hidden: true),
      ]);
      expect(hasHidden('Sounds good \u2014 see you'), isFalse);
      expect(hasHidden('caf\u00e9 \u{1F600}'), isFalse);
      expect(hasHidden('x\u2066y'), isTrue);
      // Variation selectors: one after a visible character is its own;
      // more, or anywhere else, can carry hidden words (Fugo).
      expect(hasHidden('\u2764\uFE0F \u{1F44D}\u{1F3FD}'), isFalse);
      expect(hasHidden('\u{1F44D}${'\uFE00' * 8}'), isTrue);
      expect(reviewRuns('\u{1F44D}\uFE0F\uFE01'), [
        (text: '\u{1F44D}\uFE0F', hidden: false),
        (text: '‹U+FE01›', hidden: true),
      ]);
      expect(hasHidden('\uFE0Fa'), isTrue, reason: 'at the start');
      expect(hasHidden('a \uFE0F'), isTrue, reason: 'after a space');
      expect(hasHidden('a\u200B\uFE0F'), isTrue, reason: 'after a hidden one');
      expect(hasHidden('a\u2800b'), isTrue, reason: 'the braille blank');
    });

    test("only this conversation's waiting proposals, opened here; "
        'approving seals exactly what was opened, by its agent, with the '
        "digest of what came, never the service's", () async {
      final service = world.service
        ..proposals.addAll([
          _proposal(),
          _proposal(id: 'p2', conversation: 'v2'),
          {..._proposal(id: 'p3'), 'state': 'sent'},
        ]);
      final thread = world.thread();
      await thread.load();
      expect(thread.proposals.map((p) => p.id), ['p1']);
      await thread.approve(thread.proposals.single);
      final approval = service.log.firstWhere(
        (l) => l.startsWith('POST approve'),
      );
      final (:body, :sealed) = _approval(approval);
      expect(body['revision'], 1);
      expect(body['proposal_digest'], DmProposal.fromJson(_proposal()).digest);
      expect(body['proposal_digest'], isNot('f' * 64));
      expect(sealed['g'], world.group);
      final said = sealed['c']! as Map<String, Object?>;
      expect((said['id'], said['text']), ('cm-p1-1', 'See you at 6'));
      expect((said['by']! as Map)['agent_id'], 'june');
      expect(thread.proposals, isEmpty);
      // Not sent any other way meanwhile.
      await world.matt.e2ee.sync();
      expect(
        world.worker
            .mailboxOf(world.group)
            .where((m) => m['kind'] == 'application'),
        isEmpty,
      );
    });

    test("what doesn't open as its Claktory's, for this conversation, "
        "isn't shown", () async {
      world.service.proposals.addAll([
        _proposal(id: 'p1', sealedFor: 'v2'),
        _proposal(id: 'p2', from: 'matt/tablet'),
        _proposal(id: 'p3', from: 'ana/mac-mini#1'),
        _proposal(id: 'p4'),
      ]);
      final thread = world.thread();
      await thread.load();
      await _settle();
      expect(thread.proposals.map((p) => p.id), ['p4']);
    });

    test('the group moved on first: sealed again, then approved', () async {
      final service = world.service..proposals.add(_proposal());
      final thread = world.thread();
      await thread.load();
      await _settle();
      var tries = 0;
      service.onApprove = (_) => ++tries == 1
          ? const AccountServiceError(409, 'epoch', '', {'epoch': 2})
          : null;
      await thread.approve(thread.proposals.single);
      expect(tries, 2);
      expect(thread.reviewProblem, isNull);
      expect(thread.proposals, isEmpty);
    });

    test(
      'changed since: read again, and said; unsendable: gone, and said',
      () async {
        final service = world.service..proposals.add(_proposal());
        final thread = world.thread();
        await thread.load();
        service.onApprove = (_) {
          service.proposals
            ..clear()
            ..add(_proposal(revision: 2, text: 'See you at 7'));
          return const AccountServiceError(409, 'changed', '', {});
        };
        await thread.approve(thread.proposals.single);
        expect(thread.reviewProblem, 'changed');
        expect(
          reviewProblemText('changed', agent: thread.reviewAgent),
          'June changed it since you read it. Check it again, then send.',
        );
        expect(
          reviewProblemText('changed', agent: 'Ju\u202Ene'),
          'It changed since you read it. Check it again, then send.',
        );
        expect(thread.proposals.single.text, 'See you at 7');
        service.onApprove = (_) {
          service.proposals.clear();
          return const AccountServiceError(410, 'unsendable', '', {});
        };
        await thread.approve(thread.proposals.single);
        expect(thread.reviewProblem, 'unsendable');
        expect(
          reviewProblemText('unsendable'),
          "This message can't be sent anymore.",
        );
        expect(thread.proposals, isEmpty);
      },
    );

    test(
      'ended while open (unfriended), what waits goes with the box',
      () async {
        // Abbacchio's D3d pass: the card stayed, Decline and Edit live.
        final service = world.service..proposals.add(_proposal());
        service.agentDrafts.add(
          _agentDraft(id: 'd1', name: 'June', text: 'Maybe Friday?'),
        );
        final thread = world.thread();
        unawaited(world.matt.e2ee.follow());
        unawaited(thread.follow());
        await _settle();
        expect(thread.proposals, hasLength(1));
        expect(thread.agentDrafts, hasLength(1));
        service.active = false;
        service.push([
          {'kind': 'conversation', 'conversation_id': 'v1'},
        ]);
        await _settle();
        expect(thread.canSend, isFalse);
        expect(thread.proposals, isEmpty);
        expect(thread.agentDrafts, isEmpty);
        thread.close();
      },
    );

    test(
      'a proposal event reads them again; an agent_draft one, the drafts',
      () async {
        final service = world.service;
        final thread = world.thread();
        unawaited(world.matt.e2ee.follow());
        unawaited(thread.follow());
        await _settle();
        service.proposals.add(_proposal());
        service.push([
          {'kind': 'proposal', 'id': 'p1', 'revision': 1},
        ]);
        await _settle();
        expect(thread.proposals.single.id, 'p1');
        service.agentDrafts.add(
          _agentDraft(id: 'd1', name: 'June', text: 'Maybe Friday?'),
        );
        service.push([
          {'kind': 'agent_draft', 'conversation_id': 'v1'},
        ]);
        await _settle();
        expect(thread.agentDrafts.single.text, 'Maybe Friday?');
        thread.close();
      },
    );
  });

  group('the screen', () {
    late _Service service;
    setUp(() => service = world.service);
    tearDown(() {
      chatApi = null;
      e2eeFor = null;
    });

    Future<Harness> show(
      WidgetTester tester,
      DmConversation conversation,
    ) async {
      chatApi = (_) => world.api();
      e2eeFor = (_) => world.matt.e2ee;
      final harness = Harness(Workspace());
      await harness.show(
        tester,
        home: Center(
          child: SizedBox(
            width: 480,
            height: 560,
            child: ChatView(conversation: conversation, close: () {}),
          ),
        ),
      );
      await settle(tester);
      return harness;
    }

    Future<void> close(WidgetTester tester) async {
      await tester.pumpWidget(const SizedBox());
      await tester.pump(chatDraftAfter);
    }

    testWidgets('who, the history, and sending', (tester) async {
      await show(tester, _conversation());
      await world.fromAna(['message 1', 'message 2']);
      await settle(tester);
      expect(find.text('Ana · K7QF2M', findRichText: true), findsOneWidget);
      expect(find.text('message 1'), findsOneWidget);
      // At the top, above the first message: who can read them.
      expect(
        find.text(
          'End-to-end encrypted: only you and your friends can read these '
          'messages, on your devices and any Claktory either of you gives a chat '
          'to. '
          "Claktory's servers see who you talk to and when, but not what you "
          'say.',
        ),
        findsOneWidget,
      );
      expect(
        tester.getTopLeft(find.text(dmDisclosure)).dy,
        lessThan(tester.getTopLeft(find.text('message 1')).dy),
      );
      await tester.enterText(find.byType(EditableText), 'hi Ana');
      await tester.pump();
      await tester.tap(find.text('Send'));
      await settle(tester);
      await world.matt.e2ee.sync();
      await settle(tester);
      expect(find.text('hi Ana'), findsOneWidget);
      expect(find.text('Sent'), findsOneWidget);
      await world.ana.e2ee.sync();
      expect(
        (await world.ana.e2ee.history(world.group)).last.content.text,
        'hi Ana',
      );
      await close(tester);
    });

    testWidgets("a friend's words and name show what reorders text by its "
        'code', (tester) async {
      expect(
        PeoplePerson.fromJson({'name': 'A\u202Ena', 'tag': 'K7QF2M'}).label,
        r'A\u{202E}na · K7QF2M',
      );
      await show(tester, _conversation());
      await world.fromAna(['open evil\u202Etxt.exe']);
      await settle(tester);
      expect(find.text(r'open evil\u{202E}txt.exe'), findsOneWidget);
      expect(find.textContaining('\u202E', findRichText: true), findsNothing);
      await close(tester);
    });

    testWidgets('the draft: what is typed goes to the other devices, and '
        "another device's shows while nothing was typed here", (tester) async {
      await show(tester, _conversation());
      await tester.enterText(find.byType(EditableText), 'started here');
      await tester.pump(chatDraftAfter);
      await settle(tester);
      expect((await world.matt.e2ee.view(world.group)).draft, 'started here');
      await tester.enterText(find.byType(EditableText), '');
      await tester.pump(chatDraftAfter);
      await settle(tester);
      // As from another of Matt's devices, later.
      await world.matt.e2ee.tellMyDevices(
        'draft',
        world.group,
        text: 'changed on my Mac',
      );
      await settle(tester);
      expect(find.text('changed on my Mac'), findsOneWidget);
      await close(tester);
    });

    testWidgets("the friend's key changed: the note, OK, and sending goes on", (
      tester,
    ) async {
      await show(tester, _conversation());
      world.worker.keys['ana']!['identity_key'] = dmB64(List.filled(32, 9));
      await world.matt.e2ee.refreshKeys('ana');
      await settle(tester);
      expect(
        find.text(
          "Ana's security key changed. This happens when Ana sets up "
          'messages again or replaces a device. Verify Ana to be sure '
          "it's still them.",
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('OK'));
      await settle(tester);
      expect(find.textContaining('security key changed'), findsNothing);
      await close(tester);
    });

    testWidgets('a change under a key not served yet: waiting, then Join '
        'again a day on', (tester) async {
      await show(tester, _conversation());
      await world.fromAna(['before']);
      await settle(tester);
      final group = world.group;
      world.worker.deliver(
        group,
        fakeEnvelope({
          't': 'commit',
          'g': group,
          'epoch': world.worker.epochOf(group),
          'from': 'ana/pixel',
          'members': ['matt/phone', 'ana/pixel'],
          'id': dmB64(List.filled(32, 77)),
        }),
      );
      await world.matt.e2ee.sync();
      await settle(tester);
      const waiting = 'Waiting for the new security key to reach this device.';
      expect(find.text(waiting), findsOneWidget);
      expect(find.text('Join again'), findsNothing);
      world.matt.now += E2ee.heldFor.inMilliseconds;
      await world.matt.e2ee.sync();
      await settle(tester);
      expect(find.text('Join again'), findsOneWidget);
      await tester.tap(find.text('Join again'));
      await settle(tester);
      expect(find.text(waiting), findsNothing);
      expect(world.matt.e2ee.held, isEmpty);
      await close(tester);
    });

    testWidgets("the friend's new security key not finished yet: said, and "
        'nothing can be sent until it is (row 630)', (tester) async {
      await show(tester, _conversation());
      await world.fromAna(['before']);
      await settle(tester);
      final commits = 'POST groups/${world.group}/commits';
      world.worker.refuse = (route) => route == commits
          ? const AccountServiceError(503, 'unavailable', 'unavailable')
          : null;
      final tablet = FakeEndpoint(world.worker, 'ana', 'tablet')
        ..skipSecrets(100);
      await tablet.e2ee.start();
      await tablet.e2ee.startNewKey();
      await world.matt.e2ee.refreshKeys('ana');
      await world.matt.e2ee.sync();
      await settle(tester);
      const waiting = "Waiting for Ana's new security key to finish.";
      expect(find.text(waiting), findsOneWidget);
      world.worker.refuse = null;
      await tablet.e2ee.sync();
      await world.matt.e2ee.sync();
      await settle(tester);
      expect(find.text(waiting), findsNothing);
      await close(tester);
    });

    testWidgets("a new friend not in the chat yet: said, nothing can be "
        'sent, and it opens once they are (row 676)', (tester) async {
      world.worker.packages.remove('ana/pixel');
      world.worker.lastResort.remove('ana/pixel');
      await show(tester, _conversation());
      await world.matt.e2ee.sync();
      await settle(tester);
      const waiting = 'Waiting for Ana to join this chat.';
      expect(find.text(waiting), findsOneWidget);
      expect(world.matt.e2ee.joining, {world.group: 'ana'});
      // Her device puts key packages up: Matt's next sync adds her.
      await world.ana.e2ee.sync();
      world.matt.now += FakeEndpoint.retryAfter.inMilliseconds * 2;
      await world.matt.e2ee.sync();
      await settle(tester);
      expect(find.text(waiting), findsNothing);
      await close(tester);
    });

    testWidgets("a friend's reinstalled device not back yet: said, with its "
        'name, and nothing can be sent until it is (row 202)', (tester) async {
      await show(tester, _conversation());
      await world.fromAna(['before']);
      await settle(tester);
      world.matt.device.reinstalledIn[world.group] = (
        account: 'ana',
        device: 'iPhone',
      );
      await world.matt.e2ee.sync();
      await settle(tester);
      const waiting = "Waiting for Ana's iPhone to finish setting up again.";
      expect(find.text(waiting), findsOneWidget);
      expect(
        find.text('You can send messages here again once it has.'),
        findsOneWidget,
      );
      expect(
        find.text("Waiting for Ana's new security key to finish."),
        findsNothing,
      );
      world.matt.device.reinstalledIn.clear();
      await world.matt.e2ee.sync();
      await settle(tester);
      expect(find.text(waiting), findsNothing);
      await close(tester);
    });

    testWidgets("away across four of Ana's start-overs: the chat goes on, and "
        "says what it couldn't read (row 654)", (tester) async {
      await show(tester, _conversation());
      await world.fromAna(['before']);
      await settle(tester);
      world.anaStartedOver(4, said: ['one', 'two']);
      await world.matt.e2ee.sync();
      await settle(tester);
      expect(
        find.text(
          "Some messages sent when Ana's security key changed can't be "
          'shown on this device.',
        ),
        findsOneWidget,
      );
      expect(
        find.text('Waiting for the new security key to reach this device.'),
        findsNothing,
      );
      expect(find.text('two'), findsNothing);
      await close(tester);
    });

    testWidgets('Report: the messages ticked go with it, as shown here; at '
        'most 50', (tester) async {
      await world.matt.e2ee.conversationGroup('v1', 3, 'ana');
      await world.fromAna([for (var i = 1; i <= 55; i++) 'spam $i']);
      e2eeFor = (_) => world.matt.e2ee;
      await Harness(Workspace()).show(
        tester,
        home: Center(
          child: SizedBox(
            width: 480,
            height: 800,
            child: ReportForm(
              api: ContactsApi(world.matt.e2ee.session),
              person: _ana,
              contactId: 'c1',
              conversationId: 'v1',
              close: () {},
            ),
          ),
        ),
      );
      await settle(tester);
      await settle(tester);
      expect(find.text('Include messages'), findsOneWidget);
      expect(find.text('Ana: spam 55'), findsOneWidget);
      expect(find.text('Ana: spam 25'), findsNothing);
      await tester.tap(find.text('Show earlier'));
      await settle(tester);
      expect(find.text('Ana: spam 25'), findsOneWidget);
      for (var i = 55; i >= 6; i--) {
        final row = find.text('Ana: spam $i');
        await tester.ensureVisible(row);
        await tester.tap(row);
      }
      await tester.pump();
      expect(find.text('50 messages included'), findsOneWidget);
      expect(find.text('You can include up to 50 messages.'), findsOneWidget);
      // The 51st stays out.
      await tester.ensureVisible(find.text('Ana: spam 5'));
      await tester.tap(find.text('Ana: spam 5'));
      await tester.pump();
      expect(find.text('50 messages included'), findsOneWidget);
      await tester.tap(find.text('Ana: spam 6'));
      await tester.pump();
      expect(find.text('49 messages included'), findsOneWidget);
      await tester.ensureVisible(find.text('Choose a reason'));
      await tester.tap(find.text('Choose a reason'));
      await settle(tester);
      await tester.tap(find.text('Spam').last);
      await settle(tester);
      await tester.tap(find.text('Send report'));
      await settle(tester);
      final sent = service.log.firstWhere((l) => l.startsWith('POST reports'));
      final body = jsonDecode(
        sent.substring('POST reports '.length),
      ) as Map<String, Object?>;
      final messages = body['messages']! as List;
      expect(messages, hasLength(49));
      expect(messages.last, containsPair('text', 'spam 55'));
      expect((messages.last as Map)['from'], 'them');
      await tester.pumpWidget(const SizedBox());
    });

    testWidgets('Report on a phone: a long message shows a few lines, and '
        'Cancel with messages ticked asks first (row 200)', (tester) async {
      await world.matt.e2ee.conversationGroup('v1', 3, 'ana');
      final long = 'word ' * 3200;
      await world.fromAna([long, 'short']);
      e2eeFor = (_) => world.matt.e2ee;
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      var closed = 0;
      await Harness(Workspace()).show(
        tester,
        size: const Size(375, 667),
        home: Builder(
          builder: (context) => Center(
            child: GestureDetector(
              onTap: () => showModal(
                context,
                (sheet, close) => ReportForm(
                  api: ContactsApi(world.matt.e2ee.session),
                  person: _ana,
                  contactId: 'c1',
                  conversationId: 'v1',
                  close: () {
                    closed++;
                    close();
                  },
                ),
              ),
              child: const Text('Open'),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Open'));
      await settle(tester);
      await settle(tester);
      final row = find.text('Ana: ${long.trim()}'.padRight(0));
      final text = tester.widget<Text>(
        find.byWidgetPredicate(
          (w) => w is Text && (w.data ?? '').startsWith('Ana: word word'),
        ),
      );
      expect(text.maxLines, 4);
      expect(row, findsNothing, reason: 'the data keeps the whole text');
      await tester.ensureVisible(find.text('Ana: short'));
      await tester.tap(find.text('Ana: short'));
      await tester.pump();
      expect(find.text('1 message included'), findsOneWidget);
      await tester.tap(find.text('Cancel'));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.text('Discard changes'), findsOneWidget);
      expect(closed, 0);
      await tester.tap(find.text('Keep editing'));
      await settle(tester);
      expect(find.text('1 message included'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
    });

    testWidgets('Verify: the security code, then marked verified', (
      tester,
    ) async {
      await show(tester, _conversation());
      expect(find.text('Verify…'), findsOneWidget);
      await tester.tap(find.text('Verify…'));
      await settle(tester);
      expect(find.text('Verify Ana'), findsOneWidget);
      final digits = world.matt.e2ee.safety('ana')!.digits.split(' ');
      expect(find.text(digits.take(4).join('  ')), findsOneWidget);
      // Ana's device shows the same numbers.
      await world.ana.e2ee.sync();
      expect(world.ana.e2ee.safety('matt')!.digits, digits.join(' '));
      await tester.tap(find.text('Mark as verified'));
      await settle(tester);
      expect(world.matt.e2ee.pin('ana')!.verified, isTrue);
      expect(find.text('Verify…'), findsNothing);
      expect(find.textContaining('Verified', findRichText: true), findsWidgets);
      await close(tester);
    });

    testWidgets("Verify by scanning: a match marks it, another code doesn't", (
      tester,
    ) async {
      final harness = await show(tester, _conversation());
      harness.camera.state = CameraAccess.granted;
      await world.ana.e2ee.sync();
      await tester.tap(find.text('Verify…'));
      await settle(tester);
      await settle(tester);
      await tester.tap(find.text("Scan Ana's code"));
      await settle(tester);
      // Someone else's code.
      harness.camera.scan(
        '$dmSafetyPrefix${dmB64(utf8.encode('ana=AAAA|matt=BBBB'))}',
      );
      await settle(tester);
      expect(
        find.text("These don't match. Don't mark Ana as verified."),
        findsOneWidget,
      );
      expect(world.matt.e2ee.pin('ana')!.verified, isFalse);
      // Ana's own.
      await tester.tap(find.text("Scan Ana's code"));
      await settle(tester);
      final theirs = world.ana.e2ee.safety('matt')!;
      harness.camera.scan('$dmSafetyPrefix${dmB64(theirs.qr)}');
      await settle(tester);
      await settle(tester);
      expect(world.matt.e2ee.pin('ana')!.verified, isTrue);
      expect(find.text('Verify Ana'), findsNothing);
      await close(tester);
    });

    testWidgets('Verify with a refused camera: how to allow it, with Open '
        'Settings, and Scan once allowed and back; a restricted one has no '
        'Scan (friend bug #2)', (tester) async {
      final harness = await show(tester, _conversation());
      harness.camera.state = CameraAccess.denied;
      await world.ana.e2ee.sync();
      await tester.tap(find.text('Verify…'));
      await settle(tester);
      await settle(tester);
      expect(find.text("Scan Ana's code"), findsNothing);
      expect(
        find.text(
          "To scan Ana's code, allow Claktory to use the camera in Settings.",
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Open Settings'));
      await settle(tester);
      expect(harness.camera.settingsOpened, 1);
      harness.camera.state = CameraAccess.granted;
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await settle(tester);
      expect(find.text("Scan Ana's code"), findsOneWidget);
      expect(find.text('Open Settings'), findsNothing);

      // Restricted: no dead Scan, and nothing to open.
      harness.camera.state = CameraAccess.restricted;
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await settle(tester);
      expect(find.text("Scan Ana's code"), findsNothing);
      expect(find.text('Open Settings'), findsNothing);
      expect(find.text('Mark as verified'), findsOneWidget);
      await close(tester);
    });

    testWidgets('verified, then the key changed: no longer verified, and '
        'Verify… on the note', (tester) async {
      await show(tester, _conversation());
      await world.matt.e2ee.verify('ana');
      world.worker.keys['ana']!['identity_key'] = dmB64(List.filled(32, 9));
      await world.matt.e2ee.refreshKeys('ana');
      await settle(tester);
      expect(
        find.text("Ana's security key changed, so Ana is no longer verified."),
        findsOneWidget,
      );
      expect(find.text('Verify…'), findsNWidgets(2));
      await close(tester);
    });

    testWidgets("an agent's proposed message: to whom, as it is, hidden "
        'characters shown; Send sends it as reviewed', (tester) async {
      service.proposals.add(
        _proposal(text: 'Sounds good \u202Eko', agentName: 'June'),
      );
      await show(tester, _conversation());
      expect(
        find.text(
          'June wants to send this to Ana · K7QF2M',
          findRichText: true,
        ),
        findsOneWidget,
      );
      expect(
        find.text('Sounds good ‹U+202E›ko', findRichText: true),
        findsOneWidget,
      );
      expect(
        find.text(
          'This message has hidden characters, shown in red as codes. '
          'Sending it sends them too.',
        ),
        findsOneWidget,
      );
      expect(find.text('Ana sees it as sent by your agent.'), findsOneWidget);
      await tester.tap(find.text('Send').first);
      await settle(tester);
      final (:body, :sealed) = _approval(
        service.log.firstWhere((l) => l.startsWith('POST approve')),
      );
      expect(body['revision'], 1);
      expect(
        (sealed['c']! as Map<String, Object?>)['text'],
        'Sounds good \u202Eko',
      );
      expect(
        find.text(
          'June wants to send this to Ana · K7QF2M',
          findRichText: true,
        ),
        findsNothing,
      );
      await close(tester);
    });

    testWidgets('Edit: the words into the box, and it goes no other '
        'way; an agent\'s draft too, or Dismiss', (tester) async {
      service.proposals.add(_proposal());
      service.agentDrafts.addAll([
        _agentDraft(id: 'd1', name: 'June', text: 'Maybe Friday?'),
        _agentDraft(id: 'd2', name: 'Mike', text: 'Not this week.'),
      ]);
      await show(tester, _conversation());
      expect(
        find.text('June suggests a reply', findRichText: true),
        findsOneWidget,
      );
      await tester.tap(find.text('Edit').first);
      await settle(tester);
      expect(
        tester.widget<EditableText>(find.byType(EditableText)).controller.text,
        'See you at 6',
      );
      expect(service.log, contains('POST decline {}'));
      await tester.tap(find.text('Edit').first);
      await settle(tester);
      expect(
        tester.widget<EditableText>(find.byType(EditableText)).controller.text,
        'See you at 6\nMaybe Friday?',
      );
      expect(service.log, contains('DELETE agent-draft d1'));
      await tester.tap(find.text('Dismiss'));
      await settle(tester);
      expect(service.log, contains('DELETE agent-draft d2'));
      expect(find.text('Edit'), findsNothing);
      await close(tester);
    });

    testWidgets('unknown: Try again', (tester) async {
      world.worker.refuse = (route) => route.startsWith('POST conversations/')
          ? const AccountServiceUnreachable('offline')
          : null;
      await show(tester, _conversation());
      expect(find.text('No messages yet.'), findsOneWidget);
      // Before the first message is sent, too.
      expect(find.text(dmDisclosure), findsOneWidget);
      await tester.enterText(find.byType(EditableText), 'hello');
      await tester.pump();
      await tester.tap(find.text('Send'));
      await settle(tester);
      expect(
        find.text(
          "Couldn't confirm it was sent. Try again: it won't be sent twice.",
        ),
        findsOneWidget,
      );
      world.worker.refuse = null;
      await tester.tap(find.text('Try again'));
      await settle(tester);
      await world.matt.e2ee.sync();
      await settle(tester);
      expect(find.text('Try again'), findsNothing);
      expect(find.text('Sent'), findsOneWidget);
      await close(tester);
    });

    testWidgets('Delete… asks first, then deletes everywhere', (tester) async {
      await show(tester, _conversation());
      await world.fromAna(['message 1', 'message 2']);
      await settle(tester);
      await tester.tap(find.text('Delete…'));
      await settle(tester);
      expect(find.text('Delete this conversation?'), findsOneWidget);
      expect(find.textContaining('Ana keeps their copy.'), findsOneWidget);
      await tester.tap(find.text('Delete'));
      await settle(tester);
      expect(service.log, contains('POST delete {"through_seq":3}'));
      expect(find.text('message 1'), findsNothing);
      await close(tester);
    });

    testWidgets('ended: read-only', (tester) async {
      await show(tester, _conversation());
      await world.fromAna(['message 1']);
      await close(tester);
      service.active = false;
      await show(tester, _conversation(active: false));
      expect(find.text('message 1'), findsOneWidget);
      expect(find.textContaining("You're no longer friends"), findsOneWidget);
      expect(find.text('Send'), findsNothing);
      await close(tester);
    });

    testWidgets('too long to send', (tester) async {
      await show(tester, _conversation());
      await tester.enterText(
        find.byType(EditableText),
        'x' * (messageByteLimit + 1),
      );
      await tester.pump();
      expect(find.textContaining('Too long to send'), findsOneWidget);
      await tester.tap(find.text('Send'));
      await settle(tester);
      expect(
        world.worker.log.where((line) => line.startsWith('POST conversations')),
        isEmpty,
      );
      await close(tester);
    });
  });

  testWidgets("People: a friend's row says when an agent's message waits", (
    tester,
  ) async {
    world.service.proposals.add(_proposal());
    chatApi = (_) => world.api();
    e2eeFor = (_) => world.matt.e2ee;
    contactsApi = (_) => ContactsApi(
      UserAppsSession(
        DeviceAccount(key: 'ab' * 32, email: 'matt@example.com'),
        nonce: () => 'n',
        call: (method, path, [body]) async =>
            path.startsWith('/v1/apps/dms/contacts')
            ? {
                'contacts': [
                  {
                    'id': 'c1',
                    'name': 'Ana',
                    'avatar_seed': 'aaaa',
                    'tag': 'K7QF2M',
                    'conversation_id': 'v1',
                    'generation': 3,
                    'since_ms': _now,
                  },
                ],
              }
            : <String, Object?>{},
      ),
    );
    addTearDown(() {
      chatApi = null;
      e2eeFor = null;
      contactsApi = null;
    });
    await Harness(Workspace()).show(
      tester,
      home: Center(
        child: SizedBox(
          width: 432,
          height: 700,
          child: PeopleView(close: () {}),
        ),
      ),
    );
    await settle(tester);
    expect(find.text('June wants to send a message'), findsOneWidget);
    expect(
      waitingCaption([
        DmProposal.fromJson(_proposal()),
        DmProposal.fromJson(_proposal(id: 'p2')),
        DmProposal.fromJson(_proposal(id: 'p3', conversation: 'v2')),
      ], 'v1'),
      '2 messages waiting',
    );
    expect(
      waitingCaption([
        DmProposal.fromJson(_proposal(agentName: 'Ju\u202Ene')),
      ], 'v1'),
      'Your agent wants to send a message',
    );
    expect(waitingCaption([], 'v1'), isNull);
    // With no message waiting, a reply an agent drafted there.
    DmAgentDraft drafted(String id, {String conversation = 'v1'}) =>
        DmAgentDraft.fromJson({
          'id': id,
          'conversation_id': conversation,
          'agent': {'name': 'Mike', 'claktory_id': 'mac-mini'},
          'revision': 1,
        });
    expect(
      waitingCaption([], 'v1', drafts: [drafted('d1')]),
      'Mike suggests a reply',
    );
    expect(
      waitingCaption(
        [],
        'v1',
        drafts: [
          drafted('d1'),
          drafted('d2'),
          drafted('d3', conversation: 'v2'),
        ],
      ),
      '2 replies suggested',
    );
    expect(
      waitingCaption(
        [DmProposal.fromJson(_proposal())],
        'v1',
        drafts: [drafted('d1')],
      ),
      'June wants to send a message',
    );
    // Sent from its chat: back in People, it's gone.
    await tester.tap(find.text('Message'));
    await settle(tester);
    await pumpFrames(tester, const Duration(seconds: 1));
    await settle(tester);
    await tester.tap(
      find.descendant(
        of: find.byType(ProposalCard),
        matching: find.text('Send'),
      ),
    );
    await settle(tester);
    await tester.tap(
      find.descendant(of: find.byType(ChatView), matching: find.text('Done')),
    );
    await settle(tester);
    expect(find.byType(ChatView), findsNothing);
    expect(find.text('June wants to send a message'), findsNothing);
    expect(find.textContaining('Friends since'), findsOneWidget);
  });

  testWidgets("Needs you: an agent's messages to a friend, never their text; "
      'Open chat opens the chat, where its card sends it', (tester) async {
    world.service.proposals.add(_proposal());
    chatApi = (_) => world.api();
    e2eeFor = (_) => world.matt.e2ee;
    addTearDown(() {
      chatApi = null;
      e2eeFor = null;
    });
    DmSendWaiting send(String id, {String agent = 'june', String to = 'v1'}) =>
        DmSendWaiting(
          proposalId: id,
          agentId: agent,
          conversationId: to,
          friendName: to == 'v1' ? 'Ana' : '',
          friendTag: to == 'v1' ? 'K7QF2M' : 'P2W9XR',
          createdMs: Int64(_now),
        );
    await Harness(
      Workspace(
        agents: [
          Agent(id: 'june', name: 'June', color: 'green'),
          Agent(id: 'mike', name: 'Mike', color: 'blue'),
        ],
        dmSends: [
          send('p1'),
          send('p2', agent: 'mike'),
          send('p3', agent: 'mike'),
          send('p4', agent: 'mike', to: 'v2'),
        ],
      ),
    ).show(tester, home: const NeedsYouList());
    await settle(tester);
    expect(find.text("Send June's message to Ana · K7QF2M?"), findsOneWidget);
    expect(
      find.text('Send 2 messages from Mike to Ana · K7QF2M?'),
      findsOneWidget,
    );
    expect(find.text("Send Mike's message to P2W9XR?"), findsOneWidget);
    await tester.tap(find.text("Send June's message to Ana · K7QF2M?"));
    await settle(tester);
    expect(
      find.text('Read it in the chat, then send it there.'),
      findsOneWidget,
    );
    // Opening the chat is the action: one button, on the card, and no
    // second link under it (#325).
    expect(
      find.descendant(
        of: find.byType(Button),
        matching: find.text('Open chat'),
      ),
      findsOneWidget,
    );
    expect(find.text('Open chat'), findsOneWidget);
    await tester.ensureVisible(find.text('Open chat'));
    await settle(tester);
    await tester.tap(find.text('Open chat'));
    await settle(tester);
    await pumpFrames(tester, const Duration(seconds: 1));
    await settle(tester);
    expect(find.byType(ChatView), findsOneWidget);
    expect(
      find.text('June wants to send this to Ana · K7QF2M', findRichText: true),
      findsOneWidget,
    );
    await tester.pumpWidget(const SizedBox());
    await tester.pump(chatDraftAfter);
  });

  group('on a phone', () {
    tearDown(() {
      chatApi = null;
      e2eeFor = null;
      contactsApi = null;
    });

    testWidgets("from People's sheet: People closes, and the chat is the "
        'page, with no sheet over it', (tester) async {
      chatApi = (_) => world.api();
      e2eeFor = (_) => world.matt.e2ee;
      await world.thread().load();
      await world.fromAna(['message 1']);
      contactsApi = (_) => ContactsApi(
        UserAppsSession(
          DeviceAccount(key: 'ab' * 32, email: 'matt@example.com'),
          nonce: () => 'n',
          call: (method, path, [body]) async =>
              path.startsWith('/v1/apps/dms/contacts')
              ? {
                  'contacts': [
                    {
                      'id': 'c1',
                      'name': 'Ana',
                      'avatar_seed': 'aaaa',
                      'tag': 'K7QF2M',
                      'conversation_id': 'v1',
                      'generation': 3,
                      'since_ms': _now,
                    },
                  ],
                }
              : <String, Object?>{},
        ),
      );
      final harness = Harness(workspace());
      await harness.show(tester, size: const Size(393, 852));
      await pumpFrames(tester);
      unawaited(showPeople(tester.element(find.byType(AgentView))));
      await pumpFrames(tester);
      expect(find.byType(PeopleView), findsOneWidget);
      await tester.tap(find.text('Message'));
      await pumpFrames(tester);
      expect(find.byType(PeopleView), findsNothing);
      expect(find.byType(ChatView), findsOneWidget);
      expect(find.text('message 1').hitTestable(), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
      await tester.pump(chatDraftAfter);
    });

    for (final width in [393.0, 320.0]) {
      testWidgets('a page of its own at ${width.toInt()} points, the largest '
          'text, the keyboard up: Back, who, the history and the composer '
          'above the keyboard', (tester) async {
        final service = world.service;
        await world.thread().load();
        await world.fromAna(['message 1']);
        // An agent's proposal and draft wait too: their cards fit.
        service.proposals.add(_proposal(text: 'See you at 6 \u202Eok'));
        service.agentDrafts.add(
          _agentDraft(id: 'd1', name: 'June', text: 'Maybe Friday?'),
        );
        chatApi = (_) => world.api();
        e2eeFor = (_) => world.matt.e2ee;
        final harness = Harness(workspace());
        await harness.show(tester, size: Size(width, 700));
        tester.platformDispatcher.textScaleFactorTestValue = 3.1;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        await pumpFrames(tester);
        unawaited(
          showChat(tester.element(find.byType(AgentView)), _conversation()),
        );
        await pumpFrames(tester);
        tester.view.viewInsets = const FakeViewPadding(bottom: 336);
        addTearDown(tester.view.resetViewInsets);
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
        expect(find.byType(ChatView), findsOneWidget);
        expect(find.text('K7QF2M'), findsOneWidget);
        expect(
          find.textContaining('wants to send this', findRichText: true),
          findsOneWidget,
        );
        expect(find.bySemanticsLabel('Back'), findsWidgets);
        // The field sits above the keyboard.
        final field = tester.getRect(find.byType(EditableText));
        expect(field.bottom, lessThanOrEqualTo(700 - 336));
        await tester.enterText(find.byType(EditableText), 'hi');
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox());
        await tester.pump(chatDraftAfter);
      });
    }
  });
}
