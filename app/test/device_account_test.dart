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

  test('sign-in sends the public key unsigned, then verifies signed', () async {
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
    expect(start.request.headers.value('authorization'), isNull);
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
