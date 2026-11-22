import 'package:factory_app/device_account.dart';
import 'package:factory_app/people/my_identity.dart';
import 'package:factory_app/user_apps/session.dart';
import 'package:flutter_test/flutter_test.dart';

import 'device_account_test.dart' show FakeKeys, FakeService;

// U3a over real HTTP (a file of its own: a widget test's binding answers
// every HTTP request with 400).

void main() {
  test('signed as this device, through its account sign-in', () async {
    final server = await FakeService.start();
    addTearDown(() => server.server.close(force: true));
    server.listen();
    server.answer = (request, body) => (
      200,
      {
        'profile': {'name': 'Matt', 'avatar_seed': '', 'revision': 1},
        'privacy': {'requests_on': true, 'revision': 1},
        'friend_id': {'generation': 1, 'created_ms': 0},
      },
    );
    final account = DeviceAccount(
      key: 'ab' * 32,
      service: server.url,
      email: 'matt@example.com',
      open: (key, service) => DeviceAccountService(
        accountKey: key,
        service: service,
        keys: FakeKeys(),
      ),
    );
    final me = await MyIdentityApi(UserAppsSession(account, nonce: () => 'x'))
        .me();
    expect(me.name, 'Matt');
    final request = server.requests.single.request;
    expect(request.uri.toString(), '/v1/apps/dms/me?request_nonce=x');
    expect(request.headers.value('authorization'), startsWith('Device '));
    expect(request.headers.value('claktory-signature'), isNotNull);
  });
}
