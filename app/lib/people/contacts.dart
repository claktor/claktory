import 'dart:convert';
import 'dart:math';

import '../user_apps/session.dart';
import '../visible_codes.dart' show nameInText, proseCodes;

// D3b: friend requests, friends and blocks, as the account service keeps
// them (D1b, crates/claktory-user-apps/README.md, "Requests, friends and
// blocks"). Only this device's own signed requests reach them; nothing is
// kept on the device, so each view reads them again when it opens and
// after every change (other devices aren't told yet).
//
// What a request sent says is the same whoever the friend ID belongs to,
// or if it belongs to no one: nothing here ever shows anything about whom
// a request went to.

/// Someone the person can see in People: their own name and avatar, and
/// the tag that tells them apart from anyone of the same name (the same
/// through their renames and friend ID resets).
class PeoplePerson {
  const PeoplePerson({
    required this.name,
    required this.avatarSeed,
    required this.tag,
    this.accountId = '',
  });

  // The name they chose, which only ever shows: what reorders text shows by
  // its code (proseCodes, Fugo).
  factory PeoplePerson.fromJson(Map<String, Object?> json) => PeoplePerson(
    name: proseCodes(_text(json, 'name')),
    avatarSeed: _text(json, 'avatar_seed'),
    tag: _text(json, 'tag'),
    accountId: _text(json, 'account_id'),
  );

  final String name;
  final String avatarSeed;
  final String tag;

  /// Their account, where the service says it (a friend's, for their
  /// security key and devices); never shown.
  final String accountId;

  /// How People names them, always with the tag: "Ana · K7QF2M"; just the
  /// tag when they have no name.
  String get label {
    final shown = name.isEmpty ? 'No name' : name;
    return tag.isEmpty ? shown : '$shown · $tag';
  }

  /// [label] inside a sentence ("Block Ana · K7QF2M?"): the name, which
  /// they chose, shown so it can't reorder the words around it
  /// (nameInText; Fugo 51059), the tag as it is.
  String get labelInText {
    final shown = name.isEmpty ? 'No name' : nameInText(name);
    return tag.isEmpty ? shown : '$shown · $tag';
  }
}

/// A friend request that reached the person.
class IncomingRequest {
  const IncomingRequest({
    required this.id,
    required this.from,
    required this.createdMs,
    required this.expiresMs,
  });

  factory IncomingRequest.fromJson(Map<String, Object?> json) =>
      IncomingRequest(
        id: _text(json, 'id'),
        from: PeoplePerson.fromJson(_map(json, 'from')),
        createdMs: _int(json, 'created_ms'),
        expiresMs: _int(json, 'expires_ms'),
      );

  final String id;
  final PeoplePerson from;
  final int createdMs;
  final int expiresMs;
}

/// A request the person sent: never whom to.
class SentRequest {
  const SentRequest({
    required this.id,
    required this.accepted,
    required this.createdMs,
    required this.expiresMs,
  });

  factory SentRequest.fromJson(Map<String, Object?> json) => SentRequest(
    id: _text(json, 'id'),
    accepted: json['state'] == 'accepted',
    createdMs: _int(json, 'created_ms'),
    expiresMs: _int(json, 'expires_ms'),
  );

  final String id;

  /// Accepted (they're among the friends now); else it waits, as far as
  /// the person can know.
  final bool accepted;
  final int createdMs;
  final int expiresMs;
}

class Friend {
  const Friend({
    required this.id,
    required this.person,
    required this.conversationId,
    this.generation = 0,
    required this.sinceMs,
  });

  factory Friend.fromJson(Map<String, Object?> json) => Friend(
    id: _text(json, 'id'),
    person: PeoplePerson.fromJson(json),
    conversationId: _text(json, 'conversation_id'),
    generation: _int(json, 'generation'),
    sinceMs: _int(json, 'since_ms'),
  );

  final String id;
  final PeoplePerson person;
  final String conversationId;

  /// The friendship's generation, which each message names.
  final int generation;
  final int sinceMs;
}

class Blocked {
  const Blocked({
    required this.id,
    required this.person,
    required this.createdMs,
  });

  factory Blocked.fromJson(Map<String, Object?> json) => Blocked(
    id: _text(json, 'id'),
    person: PeoplePerson.fromJson(json),
    createdMs: _int(json, 'created_ms'),
  );

  final String id;
  final PeoplePerson person;
  final int createdMs;
}

/// Someone whose request the person declined in the last 30 days: their
/// requests don't reach the person until [untilMs], unless reopened.
class Declined {
  const Declined({
    required this.id,
    required this.person,
    required this.untilMs,
  });

  factory Declined.fromJson(Map<String, Object?> json) => Declined(
    id: _text(json, 'id'),
    person: PeoplePerson.fromJson(_map(json, 'from')),
    untilMs: _int(json, 'until_ms'),
  );

  final String id;
  final PeoplePerson person;
  final int untilMs;
}

/// Why the person reports someone, as the service names it.
enum ReportReason { spam, harassment, impersonation, other }

/// A report the person made (kept 30 days): about whom, as they showed
/// when it was made, and whether Claktory's team has looked at it.
class SentReport {
  const SentReport({
    required this.id,
    required this.about,
    required this.subject,
    required this.reason,
    required this.reviewed,
    required this.createdMs,
    this.message = '',
  });

  factory SentReport.fromJson(Map<String, Object?> json) => SentReport(
    id: _text(json, 'id'),
    about: PeoplePerson.fromJson(_map(json, 'about')),
    subject: _text(json, 'subject'),
    reason: _text(json, 'reason'),
    reviewed: json['state'] == 'reviewed',
    createdMs: _int(json, 'created_ms'),
    message: _text(json, 'message'),
  );

  final String id;
  final PeoplePerson about;

  /// What it's about: "request", "contact" or "block".
  final String subject;
  final String reason;
  final bool reviewed;
  final int createdMs;

  /// The service's words for what happens now, when it was just sent.
  final String message;
}

/// Everything in People's lists, read at once.
class PeopleLists {
  const PeopleLists({
    this.incoming = const [],
    this.sent = const [],
    this.friends = const [],
    this.blocked = const [],
    this.declined = const [],
    this.reports = const [],
  });

  final List<IncomingRequest> incoming;

  /// The requests sent that still wait (accepted ones are among
  /// [friends]).
  final List<SentRequest> sent;
  final List<Friend> friends;
  final List<Blocked> blocked;
  final List<Declined> declined;
  final List<SentReport> reports;
}

/// What a friend ID typed, pasted or scanned is, to send: the part after
/// "#" in a claktory://add-friend link (or its claktory.com page), else
/// the text itself. Whether it's a friend ID is the service's to say.
String friendIdFrom(String text) {
  final trimmed = text.trim();
  final at = trimmed.indexOf('add-friend#');
  return at < 0 ? trimmed : trimmed.substring(at + 'add-friend#'.length);
}

/// A new id for one request being sent, kept for its retries so a retry
/// isn't a second request: 24 random bytes (192 bits), base64url.
String newActionId() {
  final random = Random.secure();
  return base64Url
      .encode([for (var i = 0; i < 24; i++) random.nextInt(256)])
      .replaceAll('=', '');
}

/// The requests, friends and blocks routes, through [session].
/// A message the person chose to show Claktory's team with a report: its
/// text as their device shows it (dms-e2ee §2: not verifiable).
typedef ReportedMessage = ({int seq, String text, bool mine, int sentMs});

class ContactsApi {
  const ContactsApi(this.session);

  final UserAppsSession session;

  static const _dms = '/v1/apps/dms';

  /// The requests both ways, the friends and the blocked, in one go.
  Future<PeopleLists> lists() async {
    // The first failure as it is (a UserAppError the screen can word),
    // not wrapped with the rest as a record's `.wait` would.
    final [requests, contacts, blocks, declines, reports] = await Future.wait([
      session.get('$_dms/requests'),
      session.get('$_dms/contacts'),
      session.get('$_dms/blocks'),
      session.get('$_dms/declines'),
      session.get('$_dms/reports'),
    ], eagerError: true);
    return PeopleLists(
      incoming: [
        for (final item in _list(requests, 'incoming'))
          IncomingRequest.fromJson(item),
      ],
      sent: [
        for (final item in _list(requests, 'outgoing'))
          if (SentRequest.fromJson(item) case final sent when !sent.accepted)
            sent,
      ],
      friends: [
        for (final item in _list(contacts, 'contacts')) Friend.fromJson(item),
      ],
      blocked: [
        for (final item in _list(blocks, 'blocks')) Blocked.fromJson(item),
      ],
      declined: [
        for (final item in _list(declines, 'declines')) Declined.fromJson(item),
      ],
      reports: [
        for (final item in _list(reports, 'reports')) SentReport.fromJson(item),
      ],
    );
  }

  /// Sends a friend request with [code]; the service's own words for what
  /// happened, the same whoever it reaches. [actionId] stays the same
  /// for retries of this one request.
  Future<String> send(String code, String actionId) async {
    final answer = await session.post('$_dms/requests', {
      'code': friendIdFrom(code),
      'action_id': actionId,
    });
    return answer['message'] as String? ?? '';
  }

  Future<Friend> accept(String requestId) async => Friend.fromJson(
    await session.post('$_dms/requests/$requestId/accept', {}),
  );

  Future<void> decline(String requestId) =>
      session.post('$_dms/requests/$requestId/decline', {});

  /// Takes back a request sent; [UserAppProblem.accepted] when it was
  /// accepted first.
  Future<void> cancel(String requestId) =>
      session.post('$_dms/requests/$requestId/cancel', {});

  /// Ends a friendship, without blocking.
  Future<void> remove(String contactId) =>
      session.delete('$_dms/contacts/$contactId');

  /// Blocks whoever sent [requestId], or the friend [contactId].
  Future<void> block({String? requestId, String? contactId}) => session.post(
    '$_dms/blocks',
    {'request_id': ?requestId, 'contact_id': ?contactId},
  );

  Future<void> unblock(String blockId) =>
      session.delete('$_dms/blocks/$blockId');

  /// Lets [declineId]'s sender's requests reach the person again; the
  /// request declined, when it waits again.
  Future<IncomingRequest?> reopen(String declineId) async {
    final answer = await session.delete('$_dms/declines/$declineId');
    return switch (answer['request']) {
      final Map<String, Object?> request => IncomingRequest.fromJson(request),
      _ => null,
    };
  }

  /// Reports whoever sent [requestId], the friend [contactId] or the
  /// blocked [blockId] to Claktory's team; [actionId] stays the same for
  /// retries of this one report. Answered with the report, with the
  /// service's words for what happens now.
  Future<SentReport> report({
    required String actionId,
    String? requestId,
    String? contactId,
    String? blockId,
    required ReportReason reason,
    String note = '',
    bool includeProfile = false,
    List<ReportedMessage> messages = const [],
  }) async => SentReport.fromJson(
    await session.post('$_dms/reports', {
      'action_id': actionId,
      'request_id': ?requestId,
      'contact_id': ?contactId,
      'block_id': ?blockId,
      'reason': reason.name,
      if (note.isNotEmpty) 'note': note,
      if (includeProfile) 'include_profile': true,
      if (messages.isNotEmpty)
        'messages': [
          for (final message in messages)
            {
              'seq': message.seq,
              'text': message.text,
              'from': message.mine ? 'you' : 'them',
              'sent_ms': message.sentMs,
            },
        ],
    }),
  );
}

String _text(Map<String, Object?> json, String key) => switch (json[key]) {
  final String text => text,
  _ => '',
};

int _int(Map<String, Object?> json, String key) => switch (json[key]) {
  final int number => number,
  _ => 0,
};

Map<String, Object?> _map(Map<String, Object?> json, String key) =>
    switch (json[key]) {
      final Map<String, Object?> map => map,
      _ => const {},
    };

List<Map<String, Object?>> _list(Map<String, Object?> json, String key) => [
  for (final item in json[key] as List? ?? const [])
    if (item is Map<String, Object?>) item,
];
