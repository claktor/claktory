import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/connections.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/pairing.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'pairing_test.dart' show until;
import 'tunnel_test.dart' show FakeTunnel, FakeTunnels, backendWith;

/// The account service as a device signed in to it sees it: assertions
/// for any tunnel key, recorded.
class FakeAccountApi extends DeviceAccountService {
  FakeAccountApi() : super(accountKey: 'k' * 64);

  final asked = <String>[];
  var claktoryList = <AccountClaktory>[];
  Object? failure;

  @override
  Future<({String assertion, Duration expiresIn})> assertion(
    String deviceKey,
  ) async {
    asked.add(deviceKey);
    if (failure case final Object failure) throw failure;
    return (
      assertion: 'assertion-for-$deviceKey',
      expiresIn: const Duration(hours: 1),
    );
  }

  @override
  Future<List<AccountClaktory>> claktories() async => claktoryList;

  @override
  Future<void> signOut() async {}

  @override
  Future<DeviceSignInStarted> start({
    required String email,
    required String name,
    required String platform,
    required String version,
  }) async => (
    requestId: 'r1',
    expiresIn: const Duration(minutes: 15),
    codeFrom: 'code@auth.claktory.com',
  );

  @override
  Future<({String accountId, String email})> verify({
    required String requestId,
    required String code,
  }) async {
    if (code != '123456') {
      throw const AccountServiceError(400, 'wrong_code', '', {
        'attempts_left': 4,
      });
    }
    return (accountId: 'acc', email: 'matt@example.com');
  }
}

/// Signs [account] in with the right code.
Future<void> signIn(DeviceAccount account) async {
  await account.start(
    email: 'matt@example.com',
    name: 'iPhone',
    platform: 'ios-arm64',
    version: 'v0.1.13',
  );
  await account.verify('123456');
}

/// A device signed in as matt@example.com, through [api].
DeviceAccount signedIn(FakeAccountApi api) => DeviceAccount(
  key: 'k' * 64,
  email: 'matt@example.com',
  open: (_, _) => api,
);

/// A backend's end of the tunnel that answers each message with [answer]
/// (nothing if null), and gives the first Hello.
Future<(HttpServer, Future<ClientMessage>)> answering(
  ServerMessage? Function(ClientMessage message)? answer,
) async {
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  final hello = Completer<ClientMessage>();
  server.listen((request) async {
    final socket = await WebSocketTransformer.upgrade(request);
    socket.listen((frame) {
      final message = ClientMessage.fromBuffer(frame as List<int>);
      if (message.hasHello() && !hello.isCompleted) hello.complete(message);
      final reply = answer?.call(message);
      if (reply == null) return;
      socket.add((reply..replyTo = message.id).writeToBuffer());
    });
  });
  return (server, hello.future);
}

final home = AccountClaktory(
  id: 'c1',
  name: 'Home',
  color: 'blue',
  endpointId: 'a' * 64,
  relays: const ['https://relay.example.com'],
);

void main() {
  test("a Claktory found through the account is kept by its code, and a "
      "pasted code without a token isn't one", () {
    final target = RemoteFactory.fromAccount(home);
    expect(target.fromAccount, isTrue);
    expect(target.accountId, 'c1');
    expect(target.token, isEmpty);
    expect(target.endpointId, 'a' * 64);
    expect(target.relays, ['https://relay.example.com']);
    expect(target.label, 'Home');
    expect(target.color, 'blue');
    expect(parsePairingCode(target.code, fromAccount: true), target);
    expect(parsePairingCode(target.code), isNull);
  });

  test('signed in, every Hello shows an assertion for its tunnel key and '
      'says it can confirm', () async {
    final (server, hello) = await answering(null);
    final api = FakeAccountApi();
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..account = signedIn(api);
    backend.connectTo(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac-mini'),
    );
    unawaited(backend.run());
    final sent = (await hello).hello;
    // The tunnel's endpoint id for this device: 64 "b"s.
    expect(api.asked, ['b' * 64]);
    expect(sent.assertion, 'assertion-for-${'b' * 64}');
    expect(sent.canConfirm, isTrue);
    expect(sent.token, 'c' * 64);
    await backend.close();
    await server.close(force: true);
  });

  test("not signed in, a paired device's Hello still says it can confirm, "
      'with no assertion', () async {
    final (server, hello) = await answering(null);
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..account = DeviceAccount();
    backend.connectTo(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac-mini'),
    );
    unawaited(backend.run());
    final sent = (await hello).hello;
    expect(sent.assertion, isEmpty);
    expect(sent.canConfirm, isTrue);
    await backend.close();
    await server.close(force: true);
  });

  test("found through the account, it can't connect signed out: it says "
      'to sign in, without trying', () async {
    var hellos = 0;
    final (server, _) = await answering((_) {
      hellos++;
      return null;
    });
    final account = DeviceAccount();
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..account = account;
    backend.connectTo(RemoteFactory.fromAccount(home));
    unawaited(backend.run());
    await until(() => backend.signInRequired.value != null);
    expect(backend.signInRequired.value, isNotNull);
    await Future<void>.delayed(const Duration(milliseconds: 100));
    expect(hellos, 0);
    await backend.close();
    await server.close(force: true);
  });

  test('found through the account, it waits for Allow with the number '
      'worked out from its assertion', () async {
    final (server, hello) = await answering(
      (_) => ServerMessage(
        pairingPending: PairingPending(
          deviceId: 'd9',
          machine: 'Mac mini',
          discovered: true,
          expiresMs: Int64(DateTime.now().millisecondsSinceEpoch + 600000),
        ),
      ),
    );
    final api = FakeAccountApi();
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..account = signedIn(api);
    backend.connectTo(RemoteFactory.fromAccount(home));
    unawaited(backend.run());
    final sent = (await hello).hello;
    expect(sent.token, isEmpty);
    expect(sent.assertion, 'assertion-for-${'b' * 64}');
    await until(() => backend.pairing.value != null);
    expect(
      backend.pairing.value!.number,
      pairingNumber(
        backendId: 'a' * 64,
        deviceId: 'b' * 64,
        token: 'assertion-for-${'b' * 64}',
      ),
    );
    await backend.close();
    await server.close(force: true);
  });

  test('a Hello refused until it signs in stops trying, and says so', () async {
    final (server, _) = await answering(
      (_) => ServerMessage(
        error: Error(
          code: ErrorCode.ERROR_CODE_SIGN_IN_REQUIRED,
          message: 'Confirm this device is yours: sign in.',
        ),
      ),
    );
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..account = DeviceAccount();
    backend.connectTo(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac-mini'),
    );
    unawaited(backend.run());
    await until(() => backend.signInRequired.value != null);
    expect(
      backend.signInRequired.value,
      'Confirm this device is yours: sign in.',
    );
    expect(backend.refused.value, isNull);
    // Nothing said of its account: nothing known from it.
    expect(backend.signInSaid, isNull);
    backend.retry();
    expect(backend.signInRequired.value, isNull);
    await backend.close();
    await server.close(force: true);
  });

  test('a Claktory that knows this device says where to sign in as it '
      "turns it away (Sale's row 189 b)", () async {
    final (server, _) = await answering(
      (_) => ServerMessage(
        error: Error(
          code: ErrorCode.ERROR_CODE_SIGN_IN_REQUIRED,
          message: 'Confirm this device is yours: sign in.',
          accountService: 'https://account.example.test',
          accountEmail: 'm…@example.com',
        ),
      ),
    );
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..account = DeviceAccount();
    backend.connectTo(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac-mini'),
    );
    unawaited(backend.run());
    await until(() => backend.signInRequired.value != null);
    expect(backend.signInSaid?.service, 'https://account.example.test');
    expect(backend.signInSaid?.email, 'm…@example.com');
    await backend.close();
    await server.close(force: true);
  });

  test('turned away until it signs in, it asks again quietly, its screen '
      'staying up, and connects once let in (row 189)', () async {
    var letIn = false;
    var hellos = 0;
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (!message.hasHello()) return;
        hellos++;
        if (letIn) {
          socket.add(
            (ServerMessage(
              welcome: Welcome(serverVersion: 'test'),
            )..replyTo = message.id).writeToBuffer(),
          );
          return;
        }
        socket.add(
          (ServerMessage(
            error: Error(
              code: ErrorCode.ERROR_CODE_SIGN_IN_REQUIRED,
              message: 'Confirm this device is yours: sign in.',
            ),
          )..replyTo = message.id).writeToBuffer(),
        );
        unawaited(socket.close());
      });
    });
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..account = DeviceAccount();
    backend.connectTo(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac-mini'),
    );
    unawaited(backend.run());
    await until(() => backend.signInRequired.value != null);
    await Future<void>.delayed(const Duration(milliseconds: 200));
    expect(hellos, 1);

    // Asked again while still turned away: the screen never leaves.
    var cleared = false;
    void watch() {
      if (backend.signInRequired.value == null) cleared = true;
    }

    backend.signInRequired.addListener(watch);
    backend.probeSignIn();
    await until(() => hellos == 2);
    await Future<void>.delayed(const Duration(milliseconds: 200));
    expect(hellos, 2);
    expect(cleared, isFalse);
    expect(backend.signInRequired.value, isNotNull);
    expect(backend.connected.value, isFalse);

    // Let in meanwhile: the next ask connects.
    letIn = true;
    backend.probeSignIn();
    await until(() => backend.connected.value);
    expect(backend.connected.value, isTrue);
    expect(backend.signInRequired.value, isNull);
    backend.signInRequired.removeListener(watch);
    await backend.close();
    await server.close(force: true);
  });

  test('a Hello refused as another account\'s stops trying, says what it '
      'said, and forgets nothing (#529, S4)', () async {
    const said =
        "So it can't let this iPhone in. To work with someone else's "
        'agents, open a pipe with them.';
    final (server, _) = await answering(
      (_) => ServerMessage(
        error: Error(code: ErrorCode.ERROR_CODE_OTHER_ACCOUNT, message: said),
      ),
    );
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..account = DeviceAccount();
    backend.connectTo(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=fred'),
    );
    unawaited(backend.run());
    await until(() => backend.otherAccount.value != null);
    expect(backend.otherAccount.value, said);
    // Nothing tried until Try again, as when refused.
    expect(backend.refused.value, said);
    expect(backend.signInRequired.value, isNull);
    backend.retry();
    expect(backend.otherAccount.value, isNull);
    expect(backend.refused.value, isNull);
    await backend.close();
    await server.close(force: true);
  });

  test("a Welcome says by when this device has to confirm it's the "
      "owner's", () async {
    final by = DateTime.now().add(const Duration(days: 7));
    final (server, _) = await answering(
      (_) => ServerMessage(
        welcome: Welcome(
          deviceId: 'd9',
          confirmByMs: Int64(by.millisecondsSinceEpoch),
        ),
      ),
    );
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64);
    backend.connectTo(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac-mini'),
    );
    unawaited(backend.run());
    await until(() => backend.connected.value);
    expect(
      backend.confirmBy.value?.millisecondsSinceEpoch,
      by.millisecondsSinceEpoch,
    );
    await backend.close();
    await server.close(force: true);
  });

  test('an assertion lasts until shortly before it expires; signed out at '
      'claktory.com, the device is signed out here', () async {
    final api = FakeAccountApi();
    var now = DateTime(2026, 11, 20, 12);
    final account = DeviceAccount(
      key: 'k' * 64,
      email: 'matt@example.com',
      open: (_, _) => api,
      now: () => now,
    );
    var kept = 0;
    account.onKept = () => kept++;
    await account.assertionFor('b' * 64);
    await account.assertionFor('b' * 64);
    expect(api.asked, hasLength(1));
    now = now.add(const Duration(minutes: 56));
    await account.assertionFor('b' * 64);
    expect(api.asked, hasLength(2));

    api.failure = const AccountServiceError(401, 'unauthorized', '');
    await expectLater(
      account.assertionFor('c' * 64),
      throwsA(isA<AccountServiceError>()),
    );
    expect(account.signedIn, isFalse);
    expect(account.toKept(), {'key': 'k' * 64});
    expect(kept, 1);
  });

  test('signed in while it has to confirm, it confirms at once', () async {
    final by = DateTime.now().add(const Duration(days: 7));
    final confirmed = Completer<ClientMessage>();
    final (server, _) = await answering((message) {
      if (message.hasHello()) {
        return ServerMessage(
          welcome: Welcome(
            deviceId: 'd9',
            confirmByMs: Int64(by.millisecondsSinceEpoch),
          ),
        );
      }
      if (message.hasConfirmDevice()) {
        confirmed.complete(message);
        return ServerMessage(ack: Ack());
      }
      return null;
    });
    final api = FakeAccountApi();
    final account = DeviceAccount(key: 'k' * 64, open: (_, _) => api);
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..account = account;
    backend.connectTo(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac-mini'),
    );
    unawaited(backend.run());
    await until(() => backend.connected.value);
    expect(backend.confirmBy.value, isNotNull);

    await signIn(account);
    final message = await confirmed.future;
    expect(message.confirmDevice.assertion, 'assertion-for-${'b' * 64}');
    expect(message.confirmDevice.deviceId, isEmpty);
    await until(() => backend.confirmBy.value == null);
    expect(backend.confirmBy.value, isNull);
    await backend.close();
    await server.close(force: true);
  });

  // A device paired with a code before it showed the account
  // is told it can't approve (Welcome.cannot_approve); signing in on it
  // connects again, so the next Hello's assertion lets it.
  test("told it can't approve, signing in connects again, and the field "
      'clears', () async {
    final hellos = <Hello>[];
    final (server, _) = await answering((message) {
      if (!message.hasHello()) return null;
      hellos.add(message.hello);
      return ServerMessage(
        welcome: Welcome(
          deviceId: 'd9',
          cannotApprove: message.hello.assertion.isEmpty
              ? "This device can't approve here yet."
              : '',
        ),
      );
    });
    final api = FakeAccountApi();
    final account = DeviceAccount(key: 'k' * 64, open: (_, _) => api);
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..account = account;
    backend.connectTo(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac-mini'),
    );
    unawaited(backend.run());
    await until(() => backend.connected.value);
    expect(backend.cannotApprove.value, "This device can't approve here yet.");
    expect(hellos, hasLength(1));

    await signIn(account);
    await until(() => backend.cannotApprove.value.isEmpty);
    expect(hellos, hasLength(2));
    expect(hellos.last.assertion, 'assertion-for-${'b' * 64}');
    await backend.close();
    await server.close(force: true);
  });

  test('still told it can\'t approve once signed in: it connects again once, '
      'never at every change of the account after', () async {
    final hellos = <Hello>[];
    final (server, _) = await answering((message) {
      if (!message.hasHello()) return null;
      hellos.add(message.hello);
      return ServerMessage(
        welcome: Welcome(
          deviceId: 'd9',
          cannotApprove: "This device can't approve here.",
        ),
      );
    });
    final api = FakeAccountApi();
    final account = DeviceAccount(key: 'k' * 64, open: (_, _) => api);
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64)
      ..account = account;
    backend.connectTo(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac-mini'),
    );
    unawaited(backend.run());
    await until(() => backend.connected.value);
    await signIn(account);
    await until(() => hellos.length == 2 && backend.connected.value);

    // Signed in already: signing in again changes the account twice.
    await signIn(account);
    await Future<void>.delayed(const Duration(milliseconds: 300));
    expect(hellos, hasLength(2));
    expect(backend.cannotApprove.value, "This device can't approve here.");
    await backend.close();
    await server.close(force: true);
  });

  // #365 R5: signing the Claktory in from this device confirms this device
  // there (the backend's), so it isn't asked to sign in a second time.
  test('signing the Claktory in from here confirms this device: no second '
      'sign-in', () async {
    final by = DateTime.now().add(const Duration(days: 7));
    final (server, _) = await answering((message) {
      if (message.hasHello()) {
        return ServerMessage(
          welcome: Welcome(
            deviceId: 'd9',
            confirmByMs: Int64(by.millisecondsSinceEpoch),
          ),
        );
      }
      if (message.hasAccountVerify()) {
        return message.accountVerify.code == '123456'
            ? ServerMessage(ack: Ack())
            : ServerMessage(
                error: Error(
                  code: ErrorCode.ERROR_CODE_INVALID_REQUEST,
                  message: "That code isn't right.",
                ),
              );
      }
      return null;
    });
    final backend = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = ((_) => 'k' * 64);
    backend.connectTo(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac-mini'),
    );
    unawaited(backend.run());
    await until(() => backend.connected.value);
    expect(backend.confirmBy.value, isNotNull);

    // A wrong code confirms nothing.
    await expectLater(
      backend.accountVerify('000000'),
      throwsA(isA<RequestFailure>()),
    );
    expect(backend.confirmBy.value, isNotNull);
    await backend.accountVerify('123456');
    expect(backend.confirmBy.value, isNull);
    await backend.close();
    await server.close(force: true);
  });
}
