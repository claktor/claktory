import 'package:factory_app/device_account.dart';
import 'package:factory_app/people/contacts.dart';
import 'package:factory_app/people/people_view.dart' show contactsApi;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/user_apps/grants.dart';
import 'package:factory_app/user_apps/grants_view.dart';
import 'package:factory_app/user_apps/session.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

// U3b: Agents' access, the account's grants straight from the service
// (claktory-user-apps README, "Grants"): who, on which Claktory, in which
// app, what and where (friends by name and tag, never ids), and Remove.

final _day = DateTime(2026, 11, 17, 10).millisecondsSinceEpoch;

Map<String, Object?> _grant({
  String id = 'g1',
  String claktory = 'k1',
  String agent = 'mike',
  List<String> scopes = const ['dm.read', 'dm.send'],
  List<String> conversations = const ['v1', 'v9'],
  int expires = 0,
}) => {
  'id': id,
  'app': 'dms',
  'claktory_id': claktory,
  'agent': {'id': agent, 'incarnation': 'inc-$agent'},
  'scopes': scopes,
  'resources': [
    for (final c in conversations) {'kind': 'conversation', 'id': c},
  ],
  'granted_ms': _day,
  'expires_ms': expires,
};

/// The account service, as Agents' access reads it.
class _Service {
  final log = <String>[];
  var grants = <Map<String, Object?>>[];
  var contacts = <Map<String, Object?>>[
    {
      'id': 'c1',
      'name': 'Ana',
      'avatar_seed': 'a' * 32,
      'tag': 'K7QF2M',
      'conversation_id': 'v1',
      'since_ms': _day,
    },
  ];
  var contactsFail = false;
  var revokeAnswer = 204;

  UserAppsSession session() => UserAppsSession(
    DeviceAccount(key: 'ab' * 32, email: 'matt@example.com'),
    nonce: () => 'n',
    call: (method, path, [body]) async {
      final bare = path.split('?').first;
      if (method == 'GET') {
        return switch (bare) {
          '/v1/apps/grants' => {'grants': grants},
          '/v1/apps/dms/contacts' when contactsFail =>
            throw const AccountServiceError(503, 'app_unavailable', ''),
          '/v1/apps/dms/contacts' => {'contacts': contacts},
          _ => {'incoming': [], 'outgoing': [], 'blocks': []},
        };
      }
      log.add('$method $bare');
      if (revokeAnswer == 404) {
        throw const AccountServiceError(404, 'not_found', '');
      }
      grants.removeWhere((g) => '/v1/apps/grants/${g['id']}' == bare);
      return {};
    },
  );
}

const _claktories = [
  AccountClaktory(id: 'k1', name: "Matt's Mac", endpointId: 'e1'),
  AccountClaktory(id: 'k2', name: 'Studio', endpointId: 'e2'),
];

Workspace _workspace() => Workspace(
  agents: [Agent(id: 'mike', name: 'Mike', color: 'green')],
  remote: RemoteAccess(endpointId: 'E1'),
);

void main() {
  late _Service service;
  setUp(() => service = _Service());
  tearDown(() {
    grantsApi = null;
    contactsApi = null;
    accountClaktories = null;
  });

  Future<void> show(WidgetTester tester) async {
    grantsApi = (_) => GrantsApi(service.session());
    contactsApi = (_) => ContactsApi(service.session());
    accountClaktories = (_) async => _claktories;
    await Harness(_workspace()).show(
      tester,
      home: Center(
        child: SizedBox(
          width: 432,
          height: 860,
          child: AgentsAccessView(close: () {}),
        ),
      ),
    );
    await settle(tester);
  }

  test('a grant read from the service', () {
    final grant = UserAppGrant.fromJson(_grant(expires: 5));
    expect(grant.agentId, 'mike');
    expect(grant.incarnation, 'inc-mike');
    expect(grant.claktoryId, 'k1');
    expect(grant.scopes, ['dm.read', 'dm.send']);
    expect(grant.resources.first, (kind: 'conversation', id: 'v1'));
    expect(grant.expiresMs, 5);
    expect(userAppName('dms'), 'Direct messages');
    expect(userAppScopeLabel('dm.send'), 'Send messages as you in them');
  });

  // Fugo's rule: every conversation a grant reaches is counted, named or
  // not; an unnamed one still changes the count.
  test('where it reaches: friends by name, the rest counted', () {
    final two = [
      (kind: 'conversation', id: 'v1'),
      (kind: 'conversation', id: 'v9'),
    ];
    expect(
      grantReach(two, {'v1': 'Ana · K7QF2M'}),
      'In conversations with Ana · K7QF2M and 1 other',
    );
    expect(grantReach(two, {}), 'In 2 conversations');
    expect(
      grantReach(two, {'v1': 'Ana · K7QF2M', 'v9': 'Bob · P2W9XR'}),
      'In conversations with Ana · K7QF2M and Bob · P2W9XR',
    );
    expect(grantReach(two, null), 'In 2 conversations');
    expect(grantReach(const [], {}), '');
  });

  testWidgets('each grant says who, where, what; Remove takes it back', (
    tester,
  ) async {
    service.grants = [
      _grant(),
      _grant(
        id: 'g2',
        claktory: 'k2',
        agent: 'lea',
        scopes: ['dm.read'],
        conversations: ['v9'],
      ),
    ];
    await show(tester);
    // On the shown Claktory: the agent by name, its id as a tag.
    expect(find.text('Mike · mike', findRichText: true), findsOneWidget);
    expect(
      find.textContaining("On Matt's Mac · Direct messages · since "),
      findsOneWidget,
    );
    expect(find.text('•  Send messages as you in them'), findsOneWidget);
    expect(
      find.text('In conversations with Ana · K7QF2M and 1 other'),
      findsOneWidget,
    );
    expect(find.text('Until you take it back.'), findsNWidgets(2));
    // Another Claktory: its agent by id.
    expect(find.text('lea', findRichText: true), findsOneWidget);
    expect(find.textContaining('On Studio'), findsOneWidget);

    await tester.tap(find.text('Remove…').first);
    await settle(tester);
    expect(
      find.text("Remove Mike's access to Direct messages?"),
      findsOneWidget,
    );
    await tester.tap(find.text('Remove access'));
    await settle(tester);
    expect(service.log, ['DELETE /v1/apps/grants/g1']);
    expect(find.text('Mike · mike', findRichText: true), findsNothing);
    expect(find.text('lea', findRichText: true), findsOneWidget);
  });

  testWidgets('without the friends, conversations are counted', (tester) async {
    service
      ..grants = [_grant()]
      ..contactsFail = true;
    await show(tester);
    expect(find.text('In 2 conversations'), findsOneWidget);
  });

  testWidgets('none', (tester) async {
    await show(tester);
    expect(find.text('No agent can use your apps.'), findsOneWidget);
  });

  testWidgets('one already taken back says so and reads again', (tester) async {
    service
      ..grants = [_grant()]
      ..revokeAnswer = 404;
    await show(tester);
    await tester.tap(find.text('Remove…'));
    await settle(tester);
    await tester.tap(find.text('Remove access'));
    await settle(tester);
    expect(
      find.text(
        'That access was already taken back. What you see is up to date now.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('on a phone, as a sheet', (tester) async {
    service.grants = [_grant()];
    grantsApi = (_) => GrantsApi(service.session());
    contactsApi = (_) => ContactsApi(service.session());
    accountClaktories = (_) async => _claktories;
    await Harness(_workspace()).show(
      tester,
      size: const Size(393, 852),
      home: Builder(
        builder: (context) => Center(
          child: GestureDetector(
            onTap: () => showAgentsAccess(context),
            child: const Text('Open'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open'));
    await pumpFrames(tester, const Duration(seconds: 1));
    await settle(tester);
    expect(find.text("Agents' access"), findsOneWidget);
    expect(find.text('Mike · mike', findRichText: true), findsOneWidget);
    expect(find.text('Remove…'), findsOneWidget);
  });
}
