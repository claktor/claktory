import 'dart:convert';

import 'package:factory_app/device_account.dart';
import 'package:factory_app/people/my_friend_id.dart';
import 'package:factory_app/people/my_identity.dart';
import 'package:factory_app/people/problems.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/user_apps/session.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

// U3a and D3a: the account-level apps' session, and My friend ID over it.
// The service's answers follow D1a's contract (claktory-user-apps
// README, "Your own DM identity").

Map<String, Object?> _me({
  String name = '',
  int profile = 1,
  bool requests = true,
  int privacy = 1,
  int generation = 1,
}) => {
  'profile': {
    'name': name,
    'avatar_seed': '0123456789abcdef0123456789abcdef',
    'revision': profile,
  },
  'privacy': {'requests_on': requests, 'revision': privacy},
  'friend_id': {'generation': generation, 'created_ms': 1800000000000},
};

const _code = 'cf1_ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';

/// A signed-in device whose calls [answer] answers, recorded in [calls].
class _Service {
  final calls = <(String, String, Map<String, Object?>?)>[];
  late Map<String, Object?> Function(
    String method,
    String path,
    Map<String, Object?>? body,
  )
  answer;

  /// Each call as "METHOD path body", the body as JSON.
  List<String> get log => [
    for (final (method, path, body) in calls)
      '$method $path ${jsonEncode(body)}',
  ];

  UserAppsSession session({bool signedIn = true}) => UserAppsSession(
    DeviceAccount(key: 'ab' * 32, email: signedIn ? 'matt@example.com' : ''),
    nonce: () => 'n${calls.length}',
    call: (method, path, [body]) async {
      calls.add((method, path, body));
      return answer(method, path, body);
    },
  );
}

void main() {
  group('the session', () {
    test('every request is made unique, GETs in their query', () async {
      final service = _Service()..answer = (_, _, _) => {};
      final session = service.session();
      await session.get('/v1/apps/dms/me');
      await session.post('/v1/apps/dms/me/friend-id/reveal', {});
      await session.put('/v1/apps/dms/me/privacy', {'requests_on': false});
      expect(service.log, [
        'GET /v1/apps/dms/me?request_nonce=n0 null',
        'POST /v1/apps/dms/me/friend-id/reveal {"request_nonce":"n1"}',
        'PUT /v1/apps/dms/me/privacy {"requests_on":false,"request_nonce":"n2"}',
      ]);
    });

    test('refusals say what to do', () async {
      final service = _Service();
      Future<UserAppProblem> problem(Object error) async {
        service.answer = (_, _, _) => throw error;
        try {
          await service.session().get('/v1/apps/dms/me');
        } on UserAppError catch (error) {
          return error.problem;
        }
        throw StateError('no error');
      }

      expect(
        await problem(const AccountServiceError(409, 'stale', '')),
        UserAppProblem.stale,
      );
      expect(
        await problem(const AccountServiceError(403, 'device_only', '')),
        UserAppProblem.deviceOnly,
      );
      expect(
        await problem(const AccountServiceError(429, 'rate_limited', '')),
        UserAppProblem.rateLimited,
      );
      expect(
        await problem(const AccountServiceError(503, 'app_unavailable', '')),
        UserAppProblem.unavailable,
      );
      expect(
        await problem(const AccountServiceError(400, 'invalid_request', '')),
        UserAppProblem.invalid,
      );
      expect(
        await problem(const AccountServiceUnreachable('timed out')),
        UserAppProblem.unreachable,
      );
      // Not signed in on this device: nothing is sent.
      service.calls.clear();
      await expectLater(
        service.session(signedIn: false).get('/v1/apps/dms/me'),
        throwsA(
          isA<UserAppError>().having(
            (e) => e.problem,
            'problem',
            UserAppProblem.notSignedIn,
          ),
        ),
      );
      expect(service.calls, isEmpty);
    });
  });

  group('my identity', () {
    test('read from the service’s answer, the code never in it', () {
      final me = MyIdentity.fromJson(
        _me(name: 'Matt', profile: 3, requests: false, privacy: 2),
      );
      expect(me.name, 'Matt');
      expect((me.profileRevision, me.privacyRevision), (3, 2));
      expect(me.requestsOn, isFalse);
      expect(me.generation, 1);
    });

    test('its avatar picks a gradient as the backend does', () {
      MyIdentity seeded(String seed, [String name = '']) => MyIdentity(
        name: name,
        avatarSeed: seed,
        profileRevision: 1,
        requestsOn: true,
        privacyRevision: 1,
        generation: 1,
        createdMs: 0,
      );
      // Worked out with person.rs's `pair`: SHA-256's first 8 bytes,
      // modulo 12.
      expect(
        peopleAvatar(seeded('0123456789abcdef0123456789abcdef')).avatarFrom,
        '#4D7C0F',
      );
      expect(peopleAvatar(seeded('f' * 32)).avatarFrom, '#475569');
      expect(peopleAvatar(seeded('a' * 32)).avatarFrom, '#C2410C');
      expect(peopleAvatar(seeded('a' * 32, 'Matt Smith')).initials, 'MS');
      expect(initialsOf('matt'), 'M');
      expect(initialsOf('ana-maria de la cruz'), 'AC');
      expect(initialsOf(''), '');
    });

    test('a code reads in groups of four; its link fills in Add friend', () {
      expect(groupedCode(_code), 'cf1_ABCD EFGH IJKL MNOP QRST UVWX YZ23 4567');
      expect(friendLink(_code), 'claktory://add-friend#$_code');
    });

    test('changes name the revision they were made from', () async {
      final service = _Service()..answer = (_, _, _) => _me();
      final api = MyIdentityApi(service.session());
      final me = MyIdentity.fromJson(_me(profile: 4, privacy: 6));
      await api.setProfile(me, name: 'Matt');
      await api.setProfile(me, newAvatar: true);
      await api.setRequests(me, false);
      service.answer = (_, _, _) => {'code': _code, 'generation': 2};
      final reset = await api.reset(1);
      expect(reset, (code: _code, generation: 2));
      expect(service.log, [
        'PUT /v1/apps/dms/me/profile '
            '{"name":"Matt","revision":4,"request_nonce":"n0"}',
        'PUT /v1/apps/dms/me/profile '
            '{"new_avatar":true,"revision":4,"request_nonce":"n1"}',
        'PUT /v1/apps/dms/me/privacy '
            '{"requests_on":false,"revision":6,"request_nonce":"n2"}',
        'POST /v1/apps/dms/me/friend-id/reset '
            '{"generation":1,"request_nonce":"n3"}',
      ]);
    });
  });

  group('My friend ID', () {
    late _Service service;
    late List<String> copied;

    setUp(() {
      service = _Service();
      copied = [];
    });
    tearDown(() => myIdentityApi = null);

    Future<void> show(WidgetTester tester, {bool signedIn = true}) async {
      myIdentityApi = (_) => MyIdentityApi(service.session(signedIn: signedIn));
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData') {
            copied.add((call.arguments as Map)['text'] as String);
          }
          return null;
        },
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        ),
      );
      final harness = Harness(Workspace());
      await harness.show(
        tester,
        home: Center(
          child: SizedBox(
            width: 432,
            height: 800,
            child: MyFriendIdView(close: () {}),
          ),
        ),
      );
      await settle(tester);
    }

    testWidgets('shows the friend ID only when asked; copies and hides it', (
      tester,
    ) async {
      service.answer = (method, path, body) => switch (path) {
        '/v1/apps/dms/me/friend-id/reveal' => {'code': _code, 'generation': 1},
        _ => _me(),
      };
      await show(tester);
      expect(find.text('No name yet'), findsOneWidget);
      expect(find.textContaining('ABCD'), findsNothing);
      expect(
        find.text('Anyone with your friend ID can send you one.'),
        findsOneWidget,
      );
      expect(
        find.text('People uses your Claktory account, matt@example.com.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Show'));
      await settle(tester);
      expect(
        find.text('cf1_ABCD EFGH IJKL MNOP QRST UVWX YZ23 4567'),
        findsOneWidget,
      );
      await tester.tap(find.text('Copy'));
      await settle(tester);
      expect(copied, [_code]);
      await tester.tap(find.text('Hide'));
      await settle(tester);
      expect(find.textContaining('ABCD'), findsNothing);
      expect(find.text('Show'), findsOneWidget);
    });

    testWidgets('reset asks first; another device’s reset reads it again', (
      tester,
    ) async {
      var generation = 1;
      service.answer = (method, path, body) => switch (path) {
        '/v1/apps/dms/me/friend-id/reset' => throw const AccountServiceError(
          409,
          'stale',
          '',
        ),
        _ => _me(generation: generation),
      };
      await show(tester);
      generation = 2;
      await tester.tap(find.text('Reset friend ID…'));
      await settle(tester);
      expect(find.text('Reset your friend ID?'), findsOneWidget);
      await tester.tap(find.text('Reset'));
      await settle(tester);
      final reset = service.calls.firstWhere((c) => c.$2.endsWith('/reset'));
      expect(reset.$3!['generation'], 1);
      expect(
        find.text(
          'This changed on another device. What you see is up to date now.',
        ),
        findsOneWidget,
      );
      expect(service.calls.last.$2, startsWith('/v1/apps/dms/me?'));
    });

    testWidgets('friend requests turn off; the name changes', (tester) async {
      service.answer = (method, path, body) => switch (path) {
        '/v1/apps/dms/me/privacy' => _me(requests: false, privacy: 2),
        '/v1/apps/dms/me/profile' => _me(name: 'Matt', profile: 2),
        _ => _me(),
      };
      await show(tester);
      await tester.tap(find.text('Friend requests'));
      await settle(tester);
      expect(
        find.text('Nobody can send you one, even with your friend ID.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Add a name…'));
      await settle(tester);
      await tester.enterText(find.byType(EditableText), '  Matt ');
      await tester.tap(find.text('Save'));
      await settle(tester);
      expect(find.text('Matt'), findsOneWidget);
      final profile = service.calls.lastWhere((c) => c.$2.endsWith('/profile'));
      expect(profile.$3!['name'], 'Matt');
      expect(profile.$3!['revision'], 1);
    });

    testWidgets('on a phone, a sheet', (tester) async {
      service.answer = (method, path, body) => switch (path) {
        '/v1/apps/dms/me/friend-id/reveal' => {'code': _code, 'generation': 1},
        _ => _me(name: 'Matt Smith'),
      };
      myIdentityApi = (_) => MyIdentityApi(service.session());
      final harness = Harness(Workspace());
      await harness.show(
        tester,
        size: const Size(375, 667),
        home: Builder(
          builder: (context) => Center(
            child: GestureDetector(
              onTap: () => showMyFriendId(context),
              child: const Text('Open'),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Open'));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.text('My friend ID'), findsOneWidget);
      expect(find.text('Matt Smith'), findsOneWidget);
      await tester.tap(find.text('Show'));
      await pumpFrames(tester);
      expect(
        find.text('cf1_ABCD EFGH IJKL MNOP QRST UVWX YZ23 4567'),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
    });

    testWidgets('not signed in on this device, it says so', (tester) async {
      await show(tester, signedIn: false);
      expect(
        find.text(
          'Sign in to your Claktory account on this device to use People.',
        ),
        findsOneWidget,
      );
      expect(service.calls, isEmpty);
    });
  });

  test('the words for each problem', () {
    for (final problem in UserAppProblem.values) {
      expect(problemText(UserAppError(problem)), isNotEmpty);
    }
    expect(
      problemText(
        const UserAppError(
          UserAppProblem.rateLimited,
          'Try again in 12 minutes.',
        ),
      ),
      'Try again in 12 minutes.',
    );
    expect(jsonEncode(_me()), contains('avatar_seed'));
  });
}
