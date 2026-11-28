// The account's recovery key, made on this device: the key is shown, a
// block typed back, the statement checked before the owner is asked, and
// signed and published only once they're confirmed.
import 'dart:convert';
import 'dart:math';

import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/device_account.dart' show DeviceAccount;
import 'package:factory_app/device_approval.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recovery_key.dart' show RecoveryKeyBox;
import 'package:factory_app/recovery_key_make.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

class _Approver implements DeviceApprover {
  final reasons = <String>[];
  OwnerAuthFailure? refuse;

  @override
  Future<void> confirmOwner({required String reason}) async {
    reasons.add(reason);
    if (refuse case final failure?) throw OwnerNotAuthenticated(failure);
  }

  @override
  Future<String> approveMove({
    required DeviceAccount account,
    required String requestId,
    required String accountId,
    required String sourceClaktoryId,
    required String endpoint,
    required String fingerprint,
    required int iat,
    required int exp,
    required String reason,
  }) => throw UnimplementedError();
}

class _Device implements RecoveryKeyDevice {
  final signed = <List<int>>[];
  final made = <List<int>>[];
  String? askedEndpoint;

  @override
  Future<String> deviceKey() async => 'device-key';

  @override
  Future<String> accountId() async => 'acct-1';

  @override
  Future<Set<String>> claktoryIds(String endpoint) async {
    askedEndpoint = endpoint;
    return {'studio-1'};
  }

  @override
  Future<List<int>> sign(List<int> message) async {
    signed.add(message);
    return List.filled(64, 7);
  }

  @override
  Future<String> recoveryPublicKey(List<int> bytes) async {
    made.add([...bytes]);
    return 'public-of-${bytes.length}';
  }
}

String _now() => '${DateTime.now().millisecondsSinceEpoch ~/ 1000}';

/// The statement as factoryd writes it, for [asked], with [change]d fields.
String _statement(
  NewRecoveryKey asked, [
  Map<String, Object?> change = const {},
]) => base64Url.encode(
  utf8.encode(
    jsonEncode({
      'v': 1,
      'account_id': 'acct-1',
      'claktory_id': 'studio-1',
      'public_key': asked.publicKey,
      'replaces': '',
      'device_key': asked.deviceKey,
      'iat': int.parse(_now()),
      ...change,
    }),
  ),
);

void main() {
  test("the key's text is factoryd's", () {
    List<int> hex(String text) => [
      for (var i = 0; i < text.length; i += 2)
        int.parse(text.substring(i, i + 2), radix: 16),
    ];
    for (final (bytes, text) in [
      (
        '0000000000000000000000000000000000000000',
        'AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA',
      ),
      (
        'ffffffffffffffffffffffffffffffffffffffff',
        '7777-7777-7777-7777-7777-7777-7777-7777',
      ),
      (
        '000102030405060708090a0b0c0d0e0f10111213',
        'AAAQ-EAYE-AUDA-OCAJ-BIFQ-YDIO-B4IB-CEQT',
      ),
      (
        '7c3f9a12e05bd4886a1f2c3e4d5b6a7988a9bacb',
        'PQ7Z-UEXA-LPKI-Q2Q7-FQ7E-2W3K-PGEK-TOWL',
      ),
    ]) {
      expect(recoveryKeyText(hex(bytes)), text);
    }
  });

  test('a block typed back reads as Claktory reads a typed key', () {
    expect(recoveryKeyTyped(' pq7z- '), 'PQ7Z');
    expect(recoveryKeyTyped('b41b'), 'B4IB');
    expect(recoveryKeyTyped('0c8j'), 'OCBJ');
  });

  test('an endpoint reads the same as hex or base32', () {
    final hex = '00' * 31 + 'ff';
    expect(endpointHex(hex.toUpperCase()), hex);
    expect(endpointHex('${'a' * 51}7'), '${'00' * 31}01');
    expect(endpointHex('nope'), '');
  });

  group('the statement', () {
    String key(int byte) =>
        base64Url.encode(List.filled(32, byte)).replaceAll('=', '');
    final asked = NewRecoveryKey(publicKey: key(2), deviceKey: 'dev');
    List<int> check(String payload) => checkRecoveryStatement(
      payload,
      publicKey: key(2),
      deviceKey: 'dev',
      accountId: 'acct-1',
      claktoryIds: {'studio-1'},
      now: DateTime.now(),
    );

    test('as asked for, it is the bytes to sign', () {
      final payload = _statement(asked, {'replaces': key(1)});
      expect(check(payload), base64Url.decode(payload));
      expect(check(_statement(asked)), isNotEmpty);
    });

    test('anything else is refused', () {
      for (final change in <Map<String, Object?>>[
        {'v': 2},
        {'public_key': 'another'},
        {'device_key': 'another'},
        {'account_id': 'acct-2'},
        {'claktory_id': 'elsewhere'},
        {'replaces': key(2)},
        {'replaces': 'not-a-key'},
        {'iat': int.parse(_now()) - 301},
        {'iat': int.parse(_now()) + 301},
        {'iat': _now()},
        {'extra': 1},
      ]) {
        expect(
          () => check(_statement(asked, change)),
          throwsFormatException,
          reason: '$change',
        );
      }
      expect(() => check('!!'), throwsFormatException);
    });
  });

  group('the form', () {
    late _Approver approver;
    late _Device device;
    setUp(() {
      approver = _Approver();
      device = _Device();
      deviceApprover = approver;
    });
    tearDown(() => deviceApprover = const OwnerApprover());

    /// What the Claktory answered to sign.
    late List<String> answered;

    Future<(Harness, List<bool>)> show(WidgetTester tester) async {
      final closed = <bool>[];
      answered = [];
      final harness = Harness(Workspace())
        ..backend.recoveryStatement = (asked) {
          final payload = _statement(asked);
          answered.add(payload);
          return payload;
        };
      await harness.show(
        tester,
        home: SingleChildScrollView(
          child: MakeRecoveryKeyForm(
            close: () => closed.add(true),
            device: device,
            random: Random(7),
          ),
        ),
      );
      await settle(tester);
      return (harness, closed);
    }

    Future<void> typeBack(WidgetTester tester, {bool wrong = false}) async {
      final text = tester
          .widget<RecoveryKeyBox>(find.byType(RecoveryKeyBox))
          .text;
      await tester.tap(find.widgetWithText(Button, "I've saved it"));
      await settle(tester);
      final label = find.textContaining(RegExp(r'^Block \d$'));
      final group = int.parse(
        tester.widget<Text>(label.first).data!.split(' ').last,
      );
      await tester.enterText(
        find.byType(EditableText),
        wrong ? 'ZZZZ' : text.split('-')[group - 1].toLowerCase(),
      );
      await tester.tap(find.widgetWithText(Button, 'Make key'));
      await settle(tester);
    }

    testWidgets('typed back, it asks the owner, then signs the statement and '
        'publishes it', (tester) async {
      final (harness, closed) = await show(tester);
      expect(harness.backend.sent, isEmpty);
      await typeBack(tester);
      final asked = harness.backend.sent
          .firstWhere((message) => message.hasNewRecoveryKey())
          .newRecoveryKey;
      expect(asked.deviceKey, 'device-key');
      expect(asked.publicKey, 'public-of-20');
      expect(approver.reasons, [makeRecoveryKeyReason]);
      expect(device.signed.single, [
        ...utf8.encode('claktory recovery key device v1\n'),
        ...base64Url.decode(answered.single),
      ]);
      final published = harness.last.publishRecoveryKey;
      expect(published.deviceKey, 'device-key');
      expect(
        published.deviceSig,
        base64Url.encode(List.filled(64, 7)).replaceAll('=', ''),
      );
      expect(closed, [true]);
    });

    testWidgets('the owner cancelled: nothing is signed or published, and '
        'the form stays', (tester) async {
      approver.refuse = OwnerAuthFailure.cancelled;
      final (harness, closed) = await show(tester);
      await typeBack(tester);
      expect(approver.reasons, hasLength(1));
      expect(device.signed, isEmpty);
      expect(
        harness.backend.sent.where(
          (message) => message.hasPublishRecoveryKey(),
        ),
        isEmpty,
      );
      expect(closed, isEmpty);
      expect(find.widgetWithText(Button, 'Make key'), findsOneWidget);
    });

    testWidgets('a statement for another key: the owner is never asked and '
        'nothing is signed', (tester) async {
      final (harness, closed) = await show(tester);
      harness.backend.recoveryStatement = (asked) =>
          _statement(asked, {'public_key': 'someone-elses'});
      await typeBack(tester);
      expect(approver.reasons, isEmpty);
      expect(device.signed, isEmpty);
      expect(
        harness.backend.sent.where(
          (message) => message.hasPublishRecoveryKey(),
        ),
        isEmpty,
      );
      expect(
        find.textContaining('something other than your new key'),
        findsOneWidget,
      );
      expect(closed, isEmpty);
    });

    testWidgets('a wrong block sends nothing', (tester) async {
      final (harness, _) = await show(tester);
      await typeBack(tester, wrong: true);
      expect(harness.backend.sent, isEmpty);
      expect(find.textContaining("That isn't block"), findsOneWidget);
    });

    testWidgets('a device that connected with a code says why instead', (
      tester,
    ) async {
      final (harness, _) = await show(tester);
      harness.backend.cannotApprove.value = 'Sign in on this device first.';
      await tester.pumpWidget(const SizedBox());
      final closed = <bool>[];
      await harness.show(
        tester,
        home: MakeRecoveryKeyForm(
          close: () => closed.add(true),
          device: device,
        ),
      );
      await settle(tester);
      expect(find.text('Sign in on this device first.'), findsOneWidget);
      expect(find.byType(RecoveryKeyBox), findsNothing);
    });
  });
}
