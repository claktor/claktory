import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:crypto/crypto.dart' show sha256;
import 'package:factory_app/device_account.dart';
import 'package:flutter_test/flutter_test.dart';

/// Keys that "sign" with SHA-256, so a test can check what was signed.
class FakeKeys implements AccountKeys {
  @override
  Future<Uint8List> publicKey(String key) async =>
      Uint8List.fromList(List.filled(32, 7));

  @override
  Future<Uint8List> sign(String key, List<int> message) async =>
      Uint8List.fromList([
        ...sha256.convert(message).bytes,
        ...List.filled(32, 0),
      ]);
}

/// An account service on this machine: [answer] answers each request,
/// which [requests] records with its body.
class FakeService {
  FakeService._(this.server);

  static Future<FakeService> start() async =>
      FakeService._(await HttpServer.bind(InternetAddress.loopbackIPv4, 0));

  final HttpServer server;
  final requests = <({HttpRequest request, String body})>[];
  late (int, Object?) Function(HttpRequest request, String body) answer;

  String get url => 'http://127.0.0.1:${server.port}';

  void listen() => server.listen((request) async {
    final body = await utf8.decodeStream(request);
    requests.add((request: request, body: body));
    final (status, json) = answer(request, body);
    request.response
      ..statusCode = status
      ..headers.contentType = ContentType.json
      ..write(jsonEncode(json));
    await request.response.close();
  });
}

void main() {
  late FakeService service;
  late DeviceAccountService account;
  final now = DateTime.fromMillisecondsSinceEpoch(1_800_000_000_000);

  setUp(() async {
    service = await FakeService.start();
    service.listen();
    account = DeviceAccountService(
      accountKey: 'ab' * 32,
      service: service.url,
      keys: FakeKeys(),
      now: () => now,
    );
  });
  tearDown(() => service.server.close(force: true));

  test('a request is signed over the text the service checks', () {
    expect(
      utf8.decode(requestMessage('POST', '/v1/devices/assertion', 17, [])),
      'claktory-account request v1\nPOST\n/v1/devices/assertion\n17\n'
      'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
    );
    expect(b64([0xfb, 0xff]), '-_8');
  });

  test('sign-in starts and verifies signed (row 671)', () async {
    service.answer = (request, body) => switch (request.uri.path) {
      '/v1/devices/sign-in/start' => (
        200,
        {
          'request_id': 'r1',
          'expires_in': 900,
          'code_from': 'code@auth.claktory.com',
        },
      ),
      _ => (200, {'account_id': 'acc', 'email': 'matt@example.com'}),
    };
    final started = await account.start(
      email: 'matt@example.com',
      name: "Matt's iPhone",
      platform: 'ios',
      version: 'v0.1.13',
    );
    expect(started.requestId, 'r1');
    expect(started.codeFrom, 'code@auth.claktory.com');
    final start = service.requests.single;
    expect(start.request.method, 'POST');
    expect(
      start.request.headers.value('authorization'),
      'Device ${b64(List.filled(32, 7))}',
    );
    expect(
      start.request.headers.value('claktory-signature'),
      b64([
        ...sha256
            .convert(
              requestMessage(
                'POST',
                '/v1/devices/sign-in/start',
                1800000000,
                utf8.encode(start.body),
              ),
            )
            .bytes,
        ...List.filled(32, 0),
      ]),
    );
    expect(jsonDecode(start.body), {
      'email': 'matt@example.com',
      'account_key': b64(List.filled(32, 7)),
      'name': "Matt's iPhone",
      'platform': 'ios',
      'version': 'v0.1.13',
    });

    final verified = await account.verify(requestId: 'r1', code: '123456');
    expect(verified.email, 'matt@example.com');
    final verify = service.requests.last;
    expect(
      verify.request.headers.value('authorization'),
      'Device ${b64(List.filled(32, 7))}',
    );
    expect(verify.request.headers.value('claktory-time'), '1800000000');
    final signed = requestMessage(
      'POST',
      '/v1/devices/sign-in/verify',
      1800000000,
      utf8.encode(verify.body),
    );
    expect(
      verify.request.headers.value('claktory-signature'),
      b64([...sha256.convert(signed).bytes, ...List.filled(32, 0)]),
    );
  });

  test('a wrong code says how many tries are left', () async {
    service.answer = (request, body) => (
      400,
      {'error': 'wrong_code', 'message': 'Wrong code.', 'attempts_left': 3},
    );
    await expectLater(
      account.verify(requestId: 'r1', code: '000000'),
      throwsA(
        isA<AccountServiceError>()
            .having((error) => error.code, 'code', 'wrong_code')
            .having((error) => error.attemptsLeft, 'attemptsLeft', 3),
      ),
    );
  });

  test('a clock off from the service signs again with its time', () async {
    service.answer = (request, body) =>
        request.headers.value('claktory-time') == '1800000600'
        ? (200, {'claktories': []})
        : (401, {'error': 'clock_skew', 'server_time': 1800000600});
    expect(await account.claktories(), isEmpty);
    expect(service.requests, hasLength(2));
  });

  test('several asking at once, as when the service is back after a '
      'restart, are never refused as replays', () async {
    // Like the service: a signature seen before is a replay.
    final seen = <String>{};
    service.answer = (request, body) =>
        seen.add(request.headers.value('claktory-signature')!)
        ? (200, {'claktories': []})
        : (
            401,
            {'error': 'replayed', 'message': 'This request was sent already.'},
          );
    final listed = await Future.wait([
      for (var i = 0; i < 5; i++) account.claktories(),
    ]);
    expect(listed, everyElement(isEmpty));
    expect(service.requests, hasLength(5));
    expect(
      service.requests.map(
        (sent) => sent.request.headers.value('claktory-time'),
      ),
      unorderedEquals([
        '1800000000',
        '1800000001',
        '1800000002',
        '1800000003',
        '1800000004',
      ]),
    );
  });

  test('a device removed by another of the person\'s: a new key, signed '
      'out, so signing in again starts over as a new device', () async {
    service.answer = (request, body) => (
      403,
      {
        'error': 'device_removed',
        'message': 'This device was removed from your account.',
      },
    );
    var made = 0;
    final device = DeviceAccount(
      key: 'ab' * 32,
      service: service.url,
      email: 'matt@example.com',
      open: (key, url) => DeviceAccountService(
        accountKey: key,
        service: url,
        keys: FakeKeys(),
        now: () => now,
      ),
      makeKey: () => '${++made}' * 64,
    );
    var kept = 0;
    device.onKept = () => kept++;
    await expectLater(
      device.start(
        email: 'matt@example.com',
        name: 'Mac',
        platform: 'macos',
        version: '1',
      ),
      throwsA(
        isA<AccountServiceError>().having(
          (error) => error.code,
          'code',
          'device_removed',
        ),
      ),
    );
    expect(device.key, '1' * 64, reason: 'never the removed key again');
    expect(device.signedIn, isFalse);
    expect(kept, greaterThan(0), reason: 'the new key is kept');
    // A code asked for before the removal: refused the same way.
    service.answer = (request, body) => request.uri.path.endsWith('/start')
        ? (200, {'request_id': 'r1', 'expires_in': 900})
        : (403, {'error': 'device_removed', 'message': ''});
    await device.start(
      email: 'matt@example.com',
      service: service.url,
      name: 'Mac',
      platform: 'macos',
      version: '1',
    );
    expect(device.awaitingCode, isNotNull);
    await expectLater(
      device.verify('123456'),
      throwsA(
        isA<AccountServiceError>().having(
          (error) => error.code,
          'code',
          'device_removed',
        ),
      ),
    );
    expect(device.key, '2' * 64);
    expect(device.signedIn, isFalse);
    expect(device.awaitingCode, isNull);
  });

  group('a key last bound to another account (row 671)', () {
    DeviceAccount fresh({List<String>? made}) {
      var n = 0;
      return DeviceAccount(
        key: 'ab' * 32,
        service: service.url,
        open: (key, url) => DeviceAccountService(
          accountKey: key,
          service: url,
          keys: _KeyedKeys(),
          now: () => now,
        ),
        makeKey: () {
          final key = '${++n}' * 64;
          made?.add(key);
          return key;
        },
      );
    }

    test('refused at start: a new key, and the same start again by '
        'itself, as a new device', () async {
      final keys = <String>[];
      service.answer = (request, body) =>
          request.headers.value('authorization') ==
              'Device ${b64(_KeyedKeys.of('ab' * 32))}'
          ? (
              403,
              {
                'error': 'key_of_another_account',
                'message': "This device's key belongs to another account.",
              },
            )
          : (200, {'request_id': 'r2', 'expires_in': 900});
      final device = fresh(made: keys);
      await device.start(
        email: 'ana@example.com',
        service: service.url,
        name: 'Mac',
        platform: 'macos',
        version: '1',
      );
      expect(keys, ['1' * 64]);
      expect(device.key, '1' * 64);
      expect(device.awaitingCode?.requestId, 'r2');
      expect(service.requests, hasLength(2));
      expect(
        jsonDecode(service.requests.last.body)['account_key'],
        b64(_KeyedKeys.of('1' * 64)),
      );
    });

    test(
      'refused again with the new key: says so, never a third try',
      () async {
        service.answer = (request, body) => (
          403,
          {'error': 'key_of_another_account', 'message': 'another account'},
        );
        final device = fresh();
        await expectLater(
          device.start(
            email: 'ana@example.com',
            service: service.url,
            name: 'Mac',
            platform: 'macos',
            version: '1',
          ),
          throwsA(
            isA<AccountServiceError>().having(
              (error) => error.code,
              'code',
              'key_of_another_account',
            ),
          ),
        );
        expect(service.requests, hasLength(2));
        expect(device.signedIn, isFalse);
      },
    );

    test('refused at verify: a new key, and the next start is a new '
        'device', () async {
      service.answer = (request, body) => request.uri.path.endsWith('/start')
          ? (200, {'request_id': 'r1', 'expires_in': 900})
          : (403, {'error': 'key_of_another_account', 'message': ''});
      final device = fresh();
      await device.start(
        email: 'ana@example.com',
        service: service.url,
        name: 'Mac',
        platform: 'macos',
        version: '1',
      );
      await expectLater(
        device.verify('123456'),
        throwsA(
          isA<AccountServiceError>().having(
            (error) => error.code,
            'code',
            'key_of_another_account',
          ),
        ),
      );
      expect(device.key, '1' * 64);
      expect(device.signedIn, isFalse);
      expect(device.awaitingCode, isNull);
      expect(device.removed, isFalse);
    });
  });

  group('removed while signed in (row 199)', () {
    DeviceAccount signedIn() => DeviceAccount(
      key: 'ab' * 32,
      service: service.url,
      email: 'matt@example.com',
      open: (key, url) => DeviceAccountService(
        accountKey: key,
        service: url,
        keys: FakeKeys(),
        now: () => now,
      ),
      makeKey: () => 'cd' * 32,
    );

    Matcher refused(int status) => throwsA(
      isA<AccountServiceError>().having((e) => e.status, 'status', status),
    );

    test('a signed call refused as removed signs out here, saying so, and '
        'keeps the key for the next sign-in to refuse', () async {
      service.answer = (request, body) =>
          (403, {'error': 'device_removed', 'message': 'removed'});
      final device = signedIn();
      await expectLater(
        device.signed('GET', '/v1/apps/dms/feed'),
        refused(403),
      );
      expect(device.signedIn, isFalse);
      expect(device.removed, isTrue);
      expect(device.key, 'ab' * 32);
    });

    test("a 401 from an app behind the service asks the service why: "
        'removed, it says so', () async {
      service.answer = (request, body) => request.uri.path == '/v1/devices/me'
          ? (403, {'error': 'device_removed', 'message': 'removed'})
          : (401, {'error': 'unauthorized'});
      final device = signedIn();
      await expectLater(
        device.signed('GET', '/v1/apps/dms/feed'),
        refused(401),
      );
      expect(device.signedIn, isFalse);
      expect(device.removed, isTrue);
    });

    test('signed out from elsewhere is only signed out', () async {
      service.answer = (request, body) => (401, {'error': 'unauthorized'});
      final device = signedIn();
      await expectLater(
        device.signed('GET', '/v1/apps/dms/feed'),
        refused(401),
      );
      expect(device.signedIn, isFalse);
      expect(device.removed, isFalse);
    });

    test('signing in again ends it', () async {
      service.answer = (request, body) =>
          (403, {'error': 'device_removed', 'message': 'removed'});
      final device = signedIn();
      await expectLater(device.signed('GET', '/v1/claktories'), refused(403));
      expect(device.removed, isTrue);
      // The next sign-in is refused too, and starts over with a new key.
      await expectLater(
        device.start(
          email: 'matt@example.com',
          service: service.url,
          name: 'Mac',
          platform: 'macos',
          version: '1',
        ),
        refused(403),
      );
      expect(device.key, 'cd' * 32);
      service.answer = (request, body) => request.uri.path.endsWith('/start')
          ? (200, {'request_id': 'r1', 'expires_in': 900})
          : (200, {'email': 'matt@example.com'});
      await device.start(
        email: 'matt@example.com',
        service: service.url,
        name: 'Mac',
        platform: 'macos',
        version: '1',
      );
      await device.verify('123456');
      expect(device.signedIn, isTrue);
      expect(device.removed, isFalse);
    });
  });

  test("a device that isn't signed in has no account", () async {
    service.answer = (request, body) => (401, {'error': 'unauthorized'});
    expect(await account.me(), isNull);
  });

  test("the account's Claktories, online or not, reachable or not", () async {
    service.answer = (request, body) => (
      200,
      {
        'claktories': [
          {
            'claktory_id': 'c1',
            'name': 'Home',
            'color': 'blue',
            'machine': 'Mac mini',
            'platform': 'macos-arm64',
            'version': 'v0.1.13',
            'endpoint_id': 'AB' * 32,
            'relays': ['https://relay.example.com'],
            'last_contact_ms': now
                .subtract(const Duration(minutes: 5))
                .millisecondsSinceEpoch,
          },
          {
            'claktory_id': 'c2',
            'name': 'Server',
            'endpoint_id': '',
            'last_contact_ms': now
                .subtract(const Duration(hours: 3))
                .millisecondsSinceEpoch,
          },
        ],
      },
    );
    final [home, server] = await account.claktories();
    expect(home.name, 'Home');
    expect(home.endpointId, 'ab' * 32);
    expect(home.relays, ['https://relay.example.com']);
    expect(home.online(now), isTrue);
    expect(home.reachable, isTrue);
    expect(server.online(now), isFalse);
    expect(server.reachable, isFalse);
    expect(service.requests.single.request.method, 'GET');
  });

  test('an assertion for a tunnel key', () async {
    service.answer = (request, body) =>
        (200, {'assertion': 'signed.text', 'expires_in': 3600});
    final assertion = await account.assertion('cd' * 32);
    expect(assertion.assertion, 'signed.text');
    expect(assertion.expiresIn, const Duration(hours: 1));
    expect(jsonDecode(service.requests.single.body), {'device_key': 'cd' * 32});
  });

  test("a service that can't be reached says so", () async {
    await service.server.close(force: true);
    await expectLater(account.me(), throwsA(isA<AccountServiceUnreachable>()));
  });
}

/// Keys whose public half tells them apart: [of] the key's.
class _KeyedKeys implements AccountKeys {
  static Uint8List of(String key) =>
      Uint8List.fromList(sha256.convert(utf8.encode(key)).bytes);

  @override
  Future<Uint8List> publicKey(String key) async => of(key);

  @override
  Future<Uint8List> sign(String key, List<int> message) async =>
      Uint8List.fromList(List.filled(64, 0));
}
