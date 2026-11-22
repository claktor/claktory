import 'dart:convert';
import 'dart:typed_data';

import 'package:crypto/crypto.dart';
import 'package:flutter/widgets.dart' show StringCharacters;

import '../protocol/factory/v1/factory.pb.dart' show Person;
import '../user_apps/session.dart';

// D3a: the person's own identity in People (DMs), as the account service
// keeps it (D1a, crates/claktory-user-apps/README.md, "Your own DM
// identity"): a name and a generated avatar, whether anyone may send them
// a friend request, and their secret friend ID. Only this device's own
// signed requests reach it; nothing is kept on the device, and the friend
// ID itself never leaves the view that shows it.

/// The person's own identity, without the friend ID's code.
class MyIdentity {
  const MyIdentity({
    required this.name,
    required this.avatarSeed,
    required this.profileRevision,
    required this.requestsOn,
    required this.privacyRevision,
    required this.generation,
    required this.createdMs,
  });

  factory MyIdentity.fromJson(Map<String, Object?> json) {
    Map<String, Object?> part(String key) => switch (json[key]) {
      final Map<String, Object?> map => map,
      _ => const {},
    };
    int number(Map<String, Object?> map, String key) => switch (map[key]) {
      final int n => n,
      _ => 0,
    };
    final profile = part('profile');
    final privacy = part('privacy');
    final friend = part('friend_id');
    return MyIdentity(
      name: profile['name'] as String? ?? '',
      avatarSeed: profile['avatar_seed'] as String? ?? '',
      profileRevision: number(profile, 'revision'),
      requestsOn: privacy['requests_on'] as bool? ?? true,
      privacyRevision: number(privacy, 'revision'),
      generation: number(friend, 'generation'),
      createdMs: number(friend, 'created_ms'),
    );
  }

  /// The name people see; empty until the person chooses one.
  final String name;

  /// 32 hex characters the avatar is drawn from.
  final String avatarSeed;
  final int profileRevision;

  /// Whether anyone with the friend ID may send a friend request.
  final bool requestsOn;
  final int privacyRevision;

  /// Which friend ID is the current one (counts up with each reset), and
  /// when it was made.
  final int generation;
  final int createdMs;
}

/// The avatars' gradients, as the backend has them (person.rs): one of
/// twelve, from top left to bottom right.
const _pairs = [
  ('#4F46E5', '#7C3AED'),
  ('#7C3AED', '#C026D3'),
  ('#BE185D', '#7C3AED'),
  ('#E11D48', '#9F1239'),
  ('#C2410C', '#BE123C'),
  ('#B45309', '#C2410C'),
  ('#4D7C0F', '#047857'),
  ('#047857', '#0F766E'),
  ('#0F766E', '#0369A1'),
  ('#0369A1', '#4338CA'),
  ('#1D4ED8', '#0E7490'),
  ('#475569', '#334155'),
];

/// [identity] drawn as the person's avatar is ([PersonAvatar]): a circle,
/// the initials of their People name in white on the gradient its avatar
/// seed gives, the way the backend picks one (person.rs's `pair`).
Person peopleAvatar(MyIdentity identity) =>
    peopleAvatarOf(identity.name, identity.avatarSeed);

/// Someone in People (a friend, a request) drawn the same way: [name]'s
/// initials on the gradient [seed] gives.
Person peopleAvatarOf(String name, String seed) {
  final digest = sha256.convert(utf8.encode('claktory avatar\u0000$seed'));
  // The first 8 bytes as an unsigned 64-bit number, modulo 12 (Dart's
  // ints are signed): its high and low halves, 2^32 being 4 modulo 12.
  final bytes = ByteData.sublistView(Uint8List.fromList(digest.bytes));
  final high = bytes.getUint32(0);
  final low = bytes.getUint32(4);
  final (from, to) = _pairs[(high % 12 * 4 + low % 12) % 12];
  return Person(
    name: name,
    initials: initialsOf(name),
    avatarFrom: from,
    avatarTo: to,
  );
}

/// The initials of [name] as the backend makes them: the first letters of
/// its first and last words ("Matt Smith": "MS"), or of its one word.
String initialsOf(String name) {
  final words = name
      .split(RegExp(r'[\s\-_.]+'))
      .where((word) => word.isNotEmpty)
      .toList();
  String first(String word) => word.characters.first;
  return switch (words) {
    [] => '',
    [final word] => first(word),
    [final a, ..., final b] => '${first(a)}${first(b)}',
  }.toUpperCase();
}

/// A friend ID's code as the service shows it once asked: "cf1_" and 32
/// letters and digits. Kept only by the view showing it.
typedef FriendId = ({String code, int generation});

/// The link that shares [code]: it only fills in Add friend for whoever
/// opens it (dms.md §3).
String friendLink(String code) => 'claktory://add-friend#$code';

/// [code] in groups of four after its prefix, easier to read aloud or
/// type: "cf1_ABCD EFGH …". Typed back, spaces don't matter.
String groupedCode(String code) {
  final at = code.indexOf('_');
  final prefix = at < 0 ? '' : code.substring(0, at + 1);
  final body = code.substring(at + 1);
  final groups = [
    for (var i = 0; i < body.length; i += 4)
      body.substring(i, i + 4 > body.length ? body.length : i + 4),
  ];
  return '$prefix${groups.join(' ')}';
}

/// The identity routes, through [session].
class MyIdentityApi {
  const MyIdentityApi(this.session);

  final UserAppsSession session;

  static const _me = '/v1/apps/dms/me';

  /// The identity; made by the service the first time any device asks.
  Future<MyIdentity> me() async => MyIdentity.fromJson(await session.get(_me));

  /// The current friend ID's code, to show.
  Future<FriendId> reveal() async =>
      _friendId(await session.post('$_me/friend-id/reveal', const {}));

  /// A new friend ID in place of [generation] (the one shown): the old one
  /// stops working for good. [UserAppProblem.stale] when another device
  /// reset it meanwhile.
  Future<FriendId> reset(int generation) async => _friendId(
    await session.post('$_me/friend-id/reset', {'generation': generation}),
  );

  /// Changes the name, or draws a new avatar, from [from]'s profile.
  Future<MyIdentity> setProfile(
    MyIdentity from, {
    String? name,
    bool newAvatar = false,
  }) async => MyIdentity.fromJson(
    await session.put('$_me/profile', {
      'name': ?name,
      if (newAvatar) 'new_avatar': true,
      'revision': from.profileRevision,
    }),
  );

  /// Lets anyone with the friend ID send a friend request, or no one.
  Future<MyIdentity> setRequests(MyIdentity from, bool on) async =>
      MyIdentity.fromJson(
        await session.put('$_me/privacy', {
          'requests_on': on,
          'revision': from.privacyRevision,
        }),
      );

  static FriendId _friendId(Map<String, Object?> json) => (
    code: json['code'] as String? ?? '',
    generation: json['generation'] as int? ?? 0,
  );
}
