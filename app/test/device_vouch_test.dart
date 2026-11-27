import 'dart:typed_data';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/device_vouch.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/services.dart' show MethodChannel;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// Public halves that name their key: its first two characters, repeated.
class _Keys implements AccountKeys {
  @override
  Future<Uint8List> publicKey(String key) async =>
      Uint8List.fromList(List.filled(32, key.codeUnitAt(0)));

  @override
  Future<Uint8List> sign(String key, List<int> message) async => Uint8List(64);
}

/// The account service's sign-in/verify, as [verify] says.
class _Service extends DeviceAccountService {
  _Service(this.calls, String key, String service)
    : super(accountKey: key, service: service, keys: _Keys());

  final List<({String key, String service, String requestId, String code})>
  calls;

  /// What verify does: answers, or throws.
  static Object? Function(String key) verifies = (_) => null;

  @override
  Future<({String accountId, String email})> verify({
    required String requestId,
    required String code,
  }) async {
    calls.add((
      key: accountKey,
      service: service.toString(),
      requestId: requestId,
      code: code,
    ));
    if (verifies(accountKey) case final Object error) throw error;
    return (accountId: 'acct', email: 'matt@example.com');
  }

  @override
  Future<void> signOut() async {}
}

class _Vault implements AccountKeyVault {
  bool works = true;
  String? kept;
  var writes = 0;

  @override
  MethodChannel get channel => throw UnimplementedError();

  @override
  Future<bool> available() async => works;

  @override
  Future<String?> read() async => kept;

  @override
  Future<void> write(String key) async {
    writes++;
    kept = key;
  }
}

/// This Mac's Claktory: [answers] say what each VouchDevice gets.
class _Local extends RecordingBackend {
  _Local() : super(localBackend: true);

  final asked = <VouchDevice>[];
  final answers = <Object>[];

  @override
  Future<DeviceVouched> vouchDevice({
    required String accountKey,
    required String name,
    required String platform,
    required String version,
  }) async {
    asked.add(
      VouchDevice(
        accountKey: accountKey,
        name: name,
        platform: platform,
        version: version,
      ),
    );
    final answer = answers.isEmpty ? _vouched() : answers.removeAt(0);
    if (answer is DeviceVouched) return answer;
    throw answer;
  }

  void signedIn(bool yes) => workspace.apply(
    Workspace(
      account: ClaktoryAccount(
        state: yes
            ? ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN
            : ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_OUT,
        email: 'matt@example.com',
      ),
    ),
  );
}

DeviceVouched _vouched() => DeviceVouched(
  requestId: 'r1',
  code: '123456',
  service: 'https://account.example.test',
  email: 'matt@example.com',
);

class _Changes extends ChangeNotifier {
  void changed() => notifyListeners();
}

void main() {
  late List<({String key, String service, String requestId, String code})>
  calls;
  late _Local local;
  late _Vault vault;
  late DeviceAccount account;
  late RecordingBackend other;
  late _Changes changes;
  late Map<String, String> kept;
  var made = 0;

  DeviceAccount newAccount([Map<String, String> from = const {}]) {
    final made = DeviceAccount.kept(
      from,
      vaulted: vault.kept,
      open: (key, service) => _Service(calls, key, service),
    );
    return made;
  }

  LocalVouch start() => LocalVouch(
    account: account,
    local: () => local,
    changes: changes,
    others: () => [other],
    vault: vault,
    keys: _Keys(),
    name: () async => "Matt's MacBook",
    version: 'v0.1.20',
  );

  Future<void> settle() async {
    for (var i = 0; i < 20; i++) {
      await Future<void>.delayed(Duration.zero);
    }
  }

  setUp(() {
    calls = [];
    local = _Local();
    vault = _Vault();
    other = RecordingBackend()..signInRequired.value = 'Sign in to connect.';
    changes = _Changes();
    made = 0;
    _Service.verifies = (_) => null;
    account = DeviceAccount(
      open: (key, service) => _Service(calls, key, service),
      makeKey: () => '${++made}' * 64,
    );
    kept = {};
    account.onKept = () => kept = account.toKept();
  });

  test('signs in through this Mac\'s Claktory once it is signed in, keeping '
      'the key in the keychain', () async {
    final vouch = start();
    await settle();
    expect(local.asked, isEmpty, reason: 'not while it is signed out');

    local.signedIn(true);
    await settle();
    expect(local.asked, hasLength(1));
    final asked = local.asked.single;
    expect(asked.accountKey, b64(List.filled(32, '1'.codeUnitAt(0))));
    expect(asked.name, "Matt's MacBook");
    expect(asked.platform, isNotEmpty);
    expect(asked.version, 'v0.1.20');
    expect(vault.kept, '1' * 64, reason: 'kept before it signs in');
    expect(calls.single.key, '1' * 64, reason: 'verify is signed by it');
    expect(calls.single.service, 'https://account.example.test');
    expect(calls.single.requestId, 'r1');
    expect(calls.single.code, '123456');
    expect(account.signedIn, isTrue);
    expect(account.email, 'matt@example.com');
    expect(account.service, 'https://account.example.test');
    expect(kept['key_in'], 'keychain');
    expect(kept.containsKey('key'), isFalse, reason: 'never with the rest');
    expect(other.signInRequired.value, isNull, reason: 'it tries again');
    expect(vouch.offered, isFalse);

    // After a restart it reads the key from the keychain.
    final again = newAccount(kept);
    expect(again.signedIn, isTrue);
    expect(again.key, '1' * 64);
    vault.kept = null;
    expect(newAccount(kept).signedIn, isFalse, reason: 'the key is gone');
    vouch.dispose();
  });

  test('an account service that can\'t vouch leaves it as it was, '
      'shown nothing', () async {
    local.answers.add(DeviceVouched(refused: 'unavailable', message: 'No.'));
    local.signedIn(true);
    final vouch = start();
    await settle();
    expect(local.asked, hasLength(1));
    expect(account.signedIn, isFalse);
    expect(vouch.offered, isFalse);
    changes.changed();
    local.signedIn(true);
    await settle();
    expect(local.asked, hasLength(1), reason: 'not until the app restarts');
    vouch.dispose();
  });

  test('without the keychain it never asks', () async {
    vault.works = false;
    local.signedIn(true);
    final vouch = start();
    await settle();
    expect(local.asked, isEmpty);
    expect(vouch.offered, isFalse);
    vouch.dispose();
  });

  test('removed by another device, it doesn\'t sign in by itself again but '
      'offers it, with a new key', () async {
    _Service.verifies = (key) => key == '1' * 64
        ? const AccountServiceError(403, 'device_removed', 'Removed.')
        : null;
    local.signedIn(true);
    final vouch = start();
    await settle();
    expect(account.signedIn, isFalse);
    expect(account.autoVouch, isFalse);
    expect(kept['auto'], 'off');
    expect(vouch.offered, isTrue);

    // A restart keeps it off.
    expect(newAccount(kept).autoVouch, isFalse);
    changes.changed();
    await settle();
    expect(local.asked, hasLength(1), reason: 'never by itself again');

    await vouch.vouchNow();
    expect(local.asked, hasLength(2));
    expect(account.signedIn, isTrue);
    expect(account.key, '2' * 64, reason: 'the removed key never again');
    expect(account.autoVouch, isTrue);
    expect(kept.containsKey('auto'), isFalse);
    expect(vouch.offered, isFalse);
    vouch.dispose();
  });

  test('refused as removed by the Claktory, it stops; the person\'s click '
      'tries a new key', () async {
    local.answers.add(
      DeviceVouched(refused: 'device_removed', message: 'Removed.'),
    );
    local.signedIn(true);
    final vouch = start();
    await settle();
    expect(account.autoVouch, isFalse);
    expect(vouch.offered, isTrue);
    local.answers.add(
      DeviceVouched(refused: 'device_removed', message: 'Removed.'),
    );
    await vouch.vouchNow();
    expect(local.asked, hasLength(3), reason: 'once more with a new key');
    expect(local.asked[2].accountKey, isNot(local.asked[1].accountKey));
    expect(account.signedIn, isTrue);
    vouch.dispose();
  });

  test('signed out by the person, it waits for their click', () async {
    local.signedIn(true);
    final vouch = start();
    await settle();
    expect(account.signedIn, isTrue);
    await account.signOut();
    await settle();
    expect(account.signedIn, isFalse);
    expect(local.asked, hasLength(1));
    expect(vouch.offered, isTrue);
    vouch.dispose();
  });

  test('a key that was another account\'s is made again once', () async {
    local.answers.add(
      DeviceVouched(refused: 'key_of_another_account', message: 'No.'),
    );
    local.signedIn(true);
    final vouch = start();
    await settle();
    expect(local.asked, hasLength(2));
    expect(account.signedIn, isTrue);
    expect(account.key, '2' * 64);
    expect(vault.kept, '2' * 64);
    vouch.dispose();
  });

  test('a refusal from this Mac\'s Claktory tries again later, '
      'not at once', () async {
    local.answers.add(
      const RequestFailure(
        'claktory.com can\'t be reached.',
        code: ErrorCode.ERROR_CODE_UNREACHABLE,
      ),
    );
    local.signedIn(true);
    final vouch = start();
    await settle();
    changes.changed();
    await settle();
    expect(local.asked, hasLength(1));
    expect(account.signedIn, isFalse);
    expect(vouch.offered, isFalse, reason: 'it carries on by itself');
    vouch.dispose();
  });

  test('a remote Claktory\'s app asks nothing', () async {
    local.signedIn(true);
    final vouch = LocalVouch(
      account: account,
      local: () => null,
      changes: changes,
      others: () => [other],
      vault: vault,
      keys: _Keys(),
      name: () async => 'Mac',
      version: '1',
    );
    await settle();
    expect(local.asked, isEmpty);
    vouch.dispose();
  });

  testWidgets('the one-click sign-in shows only when offered', (tester) async {
    local.answers.add(
      DeviceVouched(refused: 'device_removed', message: 'Removed.'),
    );
    final vouch = start();
    await tester.runAsync(() async {
      local.signedIn(true);
      await settle();
    });
    await Harness(Workspace())
        .show(tester, home: SignInThroughThisMac(vouch: vouch));
    expect(find.text('Use this account'), findsOneWidget);
    expect(find.textContaining('matt@example.com'), findsOneWidget);
    await tester.runAsync(() async {
      await tester.tap(find.text('Use this account'));
      await settle();
    });
    await tester.pump();
    expect(account.signedIn, isTrue);
    expect(find.text('Use this account'), findsNothing);
    vouch.dispose();
  });

  testWidgets("not offered when it's the account the Claktory asking turns "
      'away (Abbacchio row 1541)', (tester) async {
    local.answers.add(
      DeviceVouched(refused: 'device_removed', message: 'Removed.'),
    );
    final vouch = start();
    await tester.runAsync(() async {
      local.signedIn(true);
      await settle();
    });
    await Harness(Workspace()).show(
      tester,
      home: SignInThroughThisMac(vouch: vouch, refused: 'Matt@Example.com'),
    );
    expect(find.text('Use this account'), findsNothing);
    vouch.dispose();
  });

  test('Connections read a key kept in the keychain', () async {
    final storage = MemoryConnectionStorage()
      ..kept = const StoredConnections(
        current: '',
        saved: [],
        account: {
          'key_in': 'keychain',
          'service': 'https://account.example.test',
          'email': 'matt@example.com',
        },
      );
    vault.kept = 'ab' * 32;
    final connections = await Connections.load(storage, vault: vault);
    expect(connections.account.signedIn, isTrue);
    expect(connections.account.key, 'ab' * 32);
    expect(connections.account.toKept()['key_in'], 'keychain');
  });
}
