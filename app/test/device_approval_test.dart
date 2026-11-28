// The device's own move approval: the owner first, the
// card's mistakes refused before any prompt, and the signed text byte for
// byte claktory-account's sign_move_approval (the vector app/rust's
// account.rs checks against the key 03…03 too).
import 'dart:convert';

import 'package:factory_app/device_account.dart';
import 'package:factory_app/device_approval.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// sign_move_approval's answer for the key 03…03 and the fields below.
const _vector =
    'eyJ2IjoxLCJraW5kIjoibW92ZSIsInJlcXVlc3RfaWQiOiJyZXEtMSIsImFjY291bnRfaWQiOiJhY2N0LTEiLCJzb3VyY2VfY2xha3RvcnlfaWQiOiJzdHVkaW8tMSIsImVuZHBvaW50IjoiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYiIsImZpbmdlcnByaW50IjoiMGU5NDc3Y2Y0NjA1YmUzYyIsImRldmljZV9rZXkiOiI3VWtveGlqUndzYnE2UU00a0ZtVllTbFpKenBjWV9rMk5zRkdGS3lITjlFIiwiaWF0IjoxMDAwLCJleHAiOjE2MDB9.ar5HwXBga_pMT6SAnPnrxyU8kHXWceZo5HjwywCdtJ8mUinyDLbmnFqQmAj8kplWVha5PrUqige1c5rhhw4FAQ';

Uint8List _unb64(String text) =>
    base64Url.decode(text.padRight((text.length + 3) ~/ 4 * 4, '='));

/// The key 03…03's halves as the backend made them, signing only the
/// vector's message.
class _VectorKeys implements AccountKeys {
  final signed = <List<int>>[];

  @override
  Future<Uint8List> publicKey(String key) async {
    expect(key, '03' * 32);
    return _unb64('7UkoxijRwsbq6QM4kFmVYSlZJzpcY_k2NsFGFKyHN9E');
  }

  @override
  Future<Uint8List> sign(String key, List<int> message) async {
    expect(key, '03' * 32);
    signed.add(message);
    final [payload, signature] = _vector.split('.');
    expect(message, [
      ...utf8.encode('claktory move approval v1\n'),
      ..._unb64(payload),
    ]);
    return _unb64(signature);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  final endpoint = 'ab' * 32;
  DeviceAccount signedIn() =>
      DeviceAccount(key: '03' * 32, email: 'matt@example.com');

  late _VectorKeys keys;
  late List<String> asked;
  OwnerAuthFailure? refuse;
  late OwnerApprover approver;
  setUp(() {
    keys = _VectorKeys();
    asked = [];
    refuse = null;
    approver = OwnerApprover(
      keys: keys,
      authenticate: (reason) async {
        asked.add(reason);
        if (refuse case final failure?) throw OwnerNotAuthenticated(failure);
      },
    );
  });

  Future<String> approve({
    DeviceAccount? account,
    String fingerprint = '0e9477cf4605be3c',
    int iat = 1000,
    int exp = 1600,
  }) => approver.approveMove(
    account: account ?? signedIn(),
    requestId: 'req-1',
    accountId: 'acct-1',
    sourceClaktoryId: 'studio-1',
    endpoint: endpoint,
    fingerprint: fingerprint,
    iat: iat,
    exp: exp,
    reason: 'approve moving Studio to the computer with code 0E94 77CF',
  );

  test('the owner first, then the signature, byte for byte the account '
      "service's", () async {
    expect(await approve(), _vector);
    expect(asked, [
      'approve moving Studio to the computer with code 0E94 77CF',
    ]);
    // The card's grouped, upper-case code is the same code.
    expect(await approve(fingerprint: '0E9477CF4605BE3C'), _vector);
  });

  test('the owner not confirmed: nothing signed', () async {
    for (final failure in OwnerAuthFailure.values) {
      refuse = failure;
      await expectLater(
        approve(),
        throwsA(
          isA<OwnerNotAuthenticated>().having(
            (e) => e.reason,
            'reason',
            failure,
          ),
        ),
      );
    }
    expect(keys.signed, isEmpty);
  });

  test("a card's mistakes are refused before the owner is asked", () async {
    // Up to the request's 10 minutes, and ending after it starts.
    await expectLater(approve(exp: 1601), throwsArgumentError);
    await expectLater(approve(exp: 1000), throwsArgumentError);
    await expectLater(approve(iat: 1700), throwsArgumentError);
    // Only the code worked out from the endpoint, never another.
    await expectLater(
      approve(fingerprint: '0123456789abcdef'),
      throwsArgumentError,
    );
    // Signed out: no key of its own to sign with.
    await expectLater(approve(account: DeviceAccount()), throwsStateError);
    expect(asked, isEmpty);
    expect(keys.signed, isEmpty);
    // The owner's check alone signs nothing, and needs words to ask.
    await expectLater(approver.confirmOwner(reason: ''), throwsArgumentError);
    expect(asked, isEmpty);
    await approver.confirmOwner(reason: 'Approve adding hetzner-box');
    expect(asked, ['Approve adding hetzner-box']);
    expect(keys.signed, isEmpty);
  });

  test("the system's answers, on factory/device", () async {
    const channel = MethodChannel('factory/device');
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
    Object? answer;
    final calls = <MethodCall>[];
    messenger.setMockMethodCallHandler(channel, (call) async {
      calls.add(call);
      if (answer case final Exception error) throw error;
      return answer;
    });
    answer = 'ok';
    // Capitalised on an iPhone; as written on a Mac, after macOS's own
    // "Claktory is trying to".
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    await authenticateOwner('approve it');
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    await authenticateOwner('approve it');
    debugDefaultTargetPlatformOverride = null;
    expect(calls.map((call) => call.method), everyElement('authenticateOwner'));
    expect(calls.map((call) => call.arguments), [
      {'reason': 'Approve it'},
      {'reason': 'approve it'},
    ]);
    for (final (said, failure) in [
      ('cancelled', OwnerAuthFailure.cancelled),
      ('unavailable', OwnerAuthFailure.unavailable),
      ('failed', OwnerAuthFailure.failed),
      (null, OwnerAuthFailure.failed),
    ]) {
      answer = said;
      await expectLater(
        authenticateOwner('Approve it'),
        throwsA(
          isA<OwnerNotAuthenticated>().having(
            (e) => e.reason,
            'reason',
            failure,
          ),
        ),
      );
    }
    // A platform that can't ask yet: as the card's "can't approve yet".
    messenger.setMockMethodCallHandler(channel, (call) async {
      throw MissingPluginException();
    });
    await expectLater(
      authenticateOwner('Approve it'),
      throwsA(isA<UnimplementedError>()),
    );
  });
}
