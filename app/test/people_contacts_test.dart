import 'dart:convert';
import 'dart:io' show HttpOverrides;

import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/device_account.dart';
import 'package:factory_app/people/contacts.dart';
import 'package:factory_app/people/people_view.dart';
import 'package:factory_app/people/report_form.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/user_apps/session.dart';
import 'package:factory_app/scanner.dart' show CameraAccess;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'device_account_test.dart' show FakeKeys, FakeService;
import 'harness.dart';

// D3b: friend requests, friends and blocks, against D1b's contract
// (claktory-user-apps README, "Requests, friends and blocks").

final _ana = {'name': 'Ana', 'avatar_seed': 'a' * 32, 'tag': 'K7QF2M'};
final _ana2 = {'name': 'Ana', 'avatar_seed': 'b' * 32, 'tag': 'P2W9XR'};

/// The account service, as far as People reads it: [requests], [contacts]
/// and [blocks] answer the reads, [act] every change; [log] has them all.
class _Service {
  final log = <String>[];
  Map<String, Object?> requests = {'incoming': [], 'outgoing': []};
  Map<String, Object?> contacts = {'contacts': []};
  Map<String, Object?> blocks = {'blocks': []};
  Map<String, Object?> declines = {'declines': []};
  Map<String, Object?> reports = {'reports': []};
  Map<String, Object?> Function(String method, String path) act = (_, _) => {};

  /// This device signed in to the account; and a read that fails.
  bool signedIn = true;
  String failing = '';

  ContactsApi api() => ContactsApi(
    UserAppsSession(
      signedIn
          ? DeviceAccount(key: 'ab' * 32, email: 'matt@example.com')
          : DeviceAccount(),
      nonce: () => 'n',
      call: (method, path, [body]) async {
        final bare = path.split('?').first;
        if (method == 'GET') {
          if (bare == failing) {
            throw const AccountServiceUnreachable('offline');
          }
          return switch (bare) {
            '/v1/apps/dms/requests' => requests,
            '/v1/apps/dms/contacts' => contacts,
            '/v1/apps/dms/blocks' => blocks,
            '/v1/apps/dms/declines' => declines,
            '/v1/apps/dms/reports' => reports,
            _ => {
              'profile': {'name': 'Matt', 'avatar_seed': '', 'revision': 1},
              'privacy': {'requests_on': true, 'revision': 1},
              'friend_id': {'generation': 1, 'created_ms': 0},
            },
          };
        }
        final rest = {...?body}..remove('request_nonce');
        log.add('$method $bare${rest.isEmpty ? '' : ' ${jsonEncode(rest)}'}');
        return act(method, bare);
      },
    ),
  );
}

final _day = DateTime(2026, 11, 20, 10).millisecondsSinceEpoch;

void main() {
  test('lists: sent requests that wait; accepted ones are friends', () async {
    final service = _Service()
      ..requests = {
        'incoming': [
          {'id': 'r1', 'from': _ana, 'created_ms': _day, 'expires_ms': 0},
        ],
        'outgoing': [
          {'id': 's1', 'state': 'pending', 'created_ms': _day},
          {'id': 's2', 'state': 'accepted', 'contact_id': 'c9'},
        ],
      }
      ..contacts = {
        'contacts': [
          {..._ana2, 'id': 'c1', 'conversation_id': 'v1', 'since_ms': _day},
        ],
      };
    final lists = await service.api().lists();
    expect(lists.incoming.single.from.label, 'Ana · K7QF2M');
    expect([for (final sent in lists.sent) sent.id], ['s1']);
    expect(lists.friends.single.person.label, 'Ana · P2W9XR');
    expect(lists.friends.single.conversationId, 'v1');
    expect(
      const PeoplePerson(name: '', avatarSeed: '', tag: 'Q4').label,
      'No name · Q4',
    );
  });

  test('a friend ID from a link; an action id for each request', () {
    expect(friendIdFrom('  cf1_ABCD  '), 'cf1_ABCD');
    expect(friendIdFrom('claktory://add-friend#cf1_ABCD'), 'cf1_ABCD');
    expect(
      friendIdFrom('https://claktory.com/add-friend#cf1_ABCD'),
      'cf1_ABCD',
    );
    final id = newActionId();
    expect(id.length, greaterThanOrEqualTo(22));
    expect(RegExp(r'^[A-Za-z0-9_-]+$').hasMatch(id), isTrue);
    expect(newActionId(), isNot(id));
  });

  group('People', () {
    late _Service service;
    setUp(() => service = _Service());
    tearDown(() => contactsApi = null);

    Future<Harness> show(
      WidgetTester tester, {
      Size? size,
      CameraAccess camera = CameraAccess.unavailable,
      DeviceAccount? account,
    }) async {
      contactsApi = (_) => service.api();
      final harness = Harness(Workspace(), account: account)
        ..camera.state = camera;
      await harness.show(
        tester,
        size: size ?? const Size(1200, 900),
        home: size != null
            ? Builder(
                builder: (context) => Center(
                  child: GestureDetector(
                    onTap: () => showPeople(context),
                    child: const Text('Open'),
                  ),
                ),
              )
            : Center(
                child: SizedBox(
                  width: 432,
                  height: 860,
                  child: PeopleView(close: () {}),
                ),
              ),
      );
      if (size != null) {
        await tester.tap(find.text('Open'));
        await pumpFrames(tester, const Duration(seconds: 1));
      }
      await settle(tester);
      return harness;
    }

    testWidgets('not signed in on this device: it says so, and never waits '
        'for ever', (tester) async {
      service.signedIn = false;
      await show(tester);
      expect(find.text('Getting your friends…'), findsNothing);
      expect(
        find.text(
          'Sign in to your Claktory account on this device to use People.',
        ),
        findsOneWidget,
      );
      // And a way to: the app's own device sign-in, right there.
      expect(find.text('My friend ID…'), findsNothing);
      await tester.tap(find.text('Sign in…'));
      await settle(tester);
      expect(find.text('Sign in to use People'), findsOneWidget);
      expect(find.text('Email address'), findsWidgets);
    });

    testWidgets('removed by another of the person\'s devices: it says so, '
        'as soon as the service does (row 199)', (tester) async {
      service.signedIn = false;
      // Real HTTP to the fake service: widget tests answer 400 otherwise.
      final removed = await tester.runAsync(
        () => HttpOverrides.runWithHttpOverrides(() async {
          final fake = await FakeService.start();
          fake.listen();
          fake.answer = (request, body) =>
              (403, {'error': 'device_removed', 'message': ''});
          final account = DeviceAccount(
            key: 'ab' * 32,
            service: fake.url,
            email: 'matt@example.com',
            open: (key, url) => DeviceAccountService(
              accountKey: key,
              service: url,
              keys: FakeKeys(),
            ),
          );
          try {
            await account.signed('GET', '/v1/apps/dms/feed');
          } on AccountServiceError {
            // Refused, as removed.
          }
          await fake.server.close(force: true);
          return account;
        }, _RealHttp()),
      );
      expect(removed!.removed, isTrue);
      await show(tester, account: removed);
      expect(
        find.text(
          'This device was removed from your account. Sign in again to '
          'start over as a new device.',
        ),
        findsOneWidget,
      );
      expect(
        find.text(
          'Sign in to your Claktory account on this device to use People.',
        ),
        findsNothing,
      );
      expect(find.text('Sign in…'), findsOneWidget);
    });

    testWidgets('signed out on a phone, at the largest text: Sign in… fits', (
      tester,
    ) async {
      service.signedIn = false;
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await show(tester, size: const Size(320, 568));
      expect(tester.takeException(), isNull);
      await tester.ensureVisible(find.text('Sign in…'));
      await tester.tap(find.text('Sign in…'));
      await settle(tester);
      expect(find.text('Sign in to use People'), findsOneWidget);
      // Cancel, under the title now, is still a finger's size.
      expect(
        tester.getSize(find.widgetWithText(Button, 'Cancel').last).height,
        greaterThanOrEqualTo(44),
      );
      // The keyboard up: the email field scrolls clear of it.
      tester.view.viewInsets = const FakeViewPadding(bottom: 336 * 3);
      addTearDown(tester.view.resetViewInsets);
      final email = find.byType(EditableText).last;
      await tester.showKeyboard(email);
      await pumpFrames(tester, const Duration(seconds: 1));
      await tester.ensureVisible(email);
      await pumpFrames(tester, const Duration(milliseconds: 500));
      expect(tester.takeException(), isNull);
      expect(tester.getRect(email).bottom, lessThanOrEqualTo(568 - 336));
    });

    testWidgets('one list unreadable: why, not waiting', (tester) async {
      service.failing = '/v1/apps/dms/blocks';
      await show(tester);
      expect(find.text('Getting your friends…'), findsNothing);
      expect(
        find.text("Can't reach claktory.com right now. Check your connection."),
        findsOneWidget,
      );
    });

    testWidgets('a request: accept, decline or block, each with its tag', (
      tester,
    ) async {
      service.requests = {
        'incoming': [
          {'id': 'r1', 'from': _ana, 'created_ms': _day},
          {'id': 'r2', 'from': _ana2, 'created_ms': _day},
        ],
        'outgoing': [],
      };
      await show(tester);
      expect(find.text('Ana · K7QF2M', findRichText: true), findsOneWidget);
      expect(find.text('Ana · P2W9XR', findRichText: true), findsOneWidget);
      expect(
        find.text(
          'No friends yet. Give someone your friend ID, or add theirs.',
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Accept').first);
      await settle(tester);
      await tester.tap(find.text('Decline').last);
      await settle(tester);
      await tester.tap(find.bySemanticsLabel('More for Ana · K7QF2M'));
      await settle(tester);
      await tester.tap(find.text('Block…'));
      await settle(tester);
      expect(find.text('Block Ana · K7QF2M?'), findsOneWidget);
      await tester.tap(find.text('Block'));
      await settle(tester);
      expect(service.log, [
        'POST /v1/apps/dms/requests/r1/accept',
        'POST /v1/apps/dms/requests/r2/decline',
        'POST /v1/apps/dms/blocks {"request_id":"r1"}',
      ]);
    });

    testWidgets('a friend removed or blocked; one unblocked', (tester) async {
      service
        ..contacts = {
          'contacts': [
            {..._ana, 'id': 'c1', 'conversation_id': 'v1', 'since_ms': _day},
          ],
        }
        ..blocks = {
          'blocks': [
            {..._ana2, 'id': 'b1', 'created_ms': _day},
          ],
        };
      await show(tester);
      expect(find.textContaining('Friends since'), findsOneWidget);
      await tester.tap(find.bySemanticsLabel('More for Ana · K7QF2M'));
      await settle(tester);
      await tester.tap(find.text('Remove friend…'));
      await settle(tester);
      expect(find.text('Remove Ana · K7QF2M?'), findsOneWidget);
      await tester.tap(find.text('Remove'));
      await settle(tester);
      await tester.tap(find.text('Unblock'));
      await settle(tester);
      expect(service.log, [
        'DELETE /v1/apps/dms/contacts/c1',
        'DELETE /v1/apps/dms/blocks/b1',
      ]);
    });

    testWidgets('a request sent says nothing of whom; accepted first, '
        'it says so', (tester) async {
      service
        ..requests = {
          'incoming': [],
          'outgoing': [
            {'id': 's1', 'state': 'pending', 'created_ms': _day},
          ],
        }
        ..act = (method, path) =>
            throw const AccountServiceError(409, 'accepted', '');
      await show(tester);
      expect(find.textContaining('Request sent'), findsOneWidget);
      await tester.tap(find.text('Cancel'));
      await settle(tester);
      expect(service.log, ['POST /v1/apps/dms/requests/s1/cancel']);
      expect(
        find.text("They accepted before you canceled. You're friends now."),
        findsOneWidget,
      );
    });

    testWidgets('People and Add a friend say who can read messages', (
      tester,
    ) async {
      await show(tester);
      const words =
          'End-to-end encrypted: only you and your friends can read these '
          'messages, on your devices and any Claktory either of you gives a chat '
          'to. '
          "Claktory's servers see who you talk to and when, but not what you "
          'say.';
      expect(find.text(words), findsOneWidget, reason: "Friends' note");
      await tester.tap(find.text('Add a friend…'));
      await settle(tester);
      expect(
        find.text(
          'Paste or type their friend ID. They see your name and picture '
          'with your request, nothing else. $words',
        ),
        findsOneWidget,
      );
      for (final word in ['private', 'secure', 'Private', 'Secure']) {
        expect(find.textContaining(word), findsNothing, reason: word);
      }
    });

    testWidgets('adding a friend sends their ID once, retries alike', (
      tester,
    ) async {
      var refuse = true;
      service.act = (method, path) {
        if (refuse) {
          refuse = false;
          throw const AccountServiceUnreachable('timed out');
        }
        return {
          'receipt': {'id': 'x', 'state': 'pending'},
          'message': 'Request sent. They can accept if they receive it.',
        };
      };
      await show(tester);
      await tester.tap(find.text('Add a friend…'));
      await settle(tester);
      // Its preview reads the person's own profile.
      await settle(tester);
      expect(find.text('Matt'), findsOneWidget, reason: 'what they see');
      await tester.enterText(
        find.byType(EditableText),
        'claktory://add-friend#cf1_ABCD',
      );
      await tester.pump();
      await tester.tap(find.text('Send request'));
      await settle(tester);
      expect(
        find.text("Can't reach claktory.com right now. Check your connection."),
        findsOneWidget,
      );
      await tester.tap(find.text('Send request'));
      await settle(tester);
      expect(
        find.text('Request sent. They can accept if they receive it.'),
        findsOneWidget,
      );
      final sends = [
        for (final line in service.log)
          jsonDecode(line.substring(line.indexOf('{'))) as Map,
      ];
      expect(sends, hasLength(2));
      expect(sends.first['code'], 'cf1_ABCD');
      expect(sends.last['action_id'], sends.first['action_id']);
    });

    testWidgets('Add a friend scans their friend ID where there is a camera; '
        'Send request is still the person\'s (friend bug #2)', (tester) async {
      final harness = await show(tester, camera: CameraAccess.granted);
      await tester.tap(find.text('Add a friend…'));
      await settle(tester);
      await settle(tester);
      expect(
        find.textContaining('Paste, type or scan their friend ID.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Scan code'));
      await pumpFrames(tester);
      expect(find.text('Scan their friend ID'), findsOneWidget);
      harness.camera.scan('claktory-pipe://?w=a');
      await pumpFrames(tester);
      expect(find.text("This isn't a friend ID."), findsOneWidget);
      harness.camera.scan('claktory://add-friend#cf1_ABCD');
      await pumpFrames(tester);
      await settle(tester);
      expect(
        tester.widget<EditableText>(find.byType(EditableText)).controller.text,
        'cf1_ABCD',
      );
      expect(service.log.where((l) => l.startsWith('POST')), isEmpty);
      expect(find.text('Send request'), findsOneWidget);
    });

    testWidgets('on a phone, a request’s actions sit under its words', (
      tester,
    ) async {
      service.requests = {
        'incoming': [
          {
            'id': 'r1',
            'from': {..._ana, 'name': 'Alexandra Konstantinopoulou'},
            'created_ms': _day,
          },
        ],
        'outgoing': [],
      };
      await show(tester, size: const Size(375, 667));
      expect(find.text('People'), findsOneWidget);
      expect(find.text('Accept'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets('someone declined: reopened at once', (tester) async {
      service
        ..declines = {
          'declines': [
            {'id': 'd1', 'from': _ana, 'until_ms': _day},
          ],
        }
        ..act = (method, path) => {'request': null};
      await show(tester);
      expect(find.text('Declined'), findsOneWidget);
      expect(find.text('Ana · K7QF2M', findRichText: true), findsOneWidget);
      expect(
        find.textContaining('Can send requests again from'),
        findsOneWidget,
      );
      await tester.tap(find.text('Reopen'));
      await settle(tester);
      expect(service.log, ['DELETE /v1/apps/dms/declines/d1']);
    });

    testWidgets('a friend reported: why, a note, their profile if chosen; '
        'a retry is the same report', (tester) async {
      var refuse = true;
      service
        ..contacts = {
          'contacts': [
            {..._ana, 'id': 'c1', 'conversation_id': 'v1', 'since_ms': _day},
          ],
        }
        ..act = (method, path) {
          if (refuse) {
            refuse = false;
            throw const AccountServiceUnreachable('timed out');
          }
          return {
            'id': 'p1',
            'about': _ana,
            'subject': 'contact',
            'reason': 'harassment',
            'state': 'received',
            'created_ms': _day,
            'message': 'Report sent. Claktory’s team will look at it.',
          };
        };
      await show(tester);
      await tester.tap(find.bySemanticsLabel('More for Ana · K7QF2M'));
      await settle(tester);
      await tester.tap(find.text('Report…'));
      await settle(tester);
      expect(find.text('Report Ana · K7QF2M'), findsOneWidget);
      // Nothing to send until there's a reason.
      await tester.tap(find.text('Send report'));
      await settle(tester);
      expect(service.log, isEmpty);
      await tester.tap(find.text('Choose a reason'));
      await settle(tester);
      await tester.tap(find.text('Harassment').last);
      await settle(tester);
      await tester.enterText(
        find.descendant(
          of: find.byType(ReportForm),
          matching: find.byType(EditableText),
        ),
        '  Kept messaging after I said no.  ',
      );
      await tester.tap(
        find.bySemanticsLabel('Include the name and picture they show you'),
      );
      await tester.pump();
      await tester.tap(find.text('Send report'));
      await settle(tester);
      expect(
        find.text("Can't reach claktory.com right now. Check your connection."),
        findsOneWidget,
      );
      await tester.tap(find.text('Send report'));
      await settle(tester);
      expect(
        find.text('Report sent. Claktory’s team will look at it.'),
        findsOneWidget,
      );
      final sent = [
        for (final line in service.log)
          jsonDecode(line.substring(line.indexOf('{'))) as Map,
      ];
      expect(sent, hasLength(2));
      expect(sent.first, {
        'action_id': sent.first['action_id'],
        'contact_id': 'c1',
        'reason': 'harassment',
        'note': 'Kept messaging after I said no.',
        'include_profile': true,
      });
      expect(sent.last['action_id'], sent.first['action_id']);
    });

    testWidgets('a request and a block can be reported too', (tester) async {
      service
        ..requests = {
          'incoming': [
            {'id': 'r1', 'from': _ana, 'created_ms': _day},
          ],
          'outgoing': [],
        }
        ..blocks = {
          'blocks': [
            {..._ana2, 'id': 'b1', 'created_ms': _day},
          ],
        };
      await show(tester);
      for (final (label, field, id) in [
        ('Ana · K7QF2M', 'request_id', 'r1'),
        ('Ana · P2W9XR', 'block_id', 'b1'),
      ]) {
        service.log.clear();
        await tester.tap(find.bySemanticsLabel('More for $label'));
        await settle(tester);
        await tester.tap(find.text('Report…'));
        await settle(tester);
        await tester.tap(find.text('Choose a reason'));
        await settle(tester);
        await tester.tap(find.text('Spam').last);
        await settle(tester);
        await tester.tap(find.text('Send report'));
        await settle(tester);
        final body = jsonDecode(
          service.log.single.substring(service.log.single.indexOf('{')),
        ) as Map;
        expect(body[field], id);
        expect(body['reason'], 'spam');
        expect(body.containsKey('note'), isFalse);
        expect(body.containsKey('include_profile'), isFalse);
        await tester.tap(find.text('Done').last);
        await settle(tester);
      }
    });

    testWidgets('the reports made, and whether they were looked at', (
      tester,
    ) async {
      service.reports = {
        'reports': [
          {
            'id': 'p1',
            'about': _ana,
            'subject': 'request',
            'reason': 'impersonation',
            'state': 'reviewed',
            'created_ms': _day,
          },
        ],
      };
      await show(tester);
      expect(find.text('Your reports'), findsOneWidget);
      expect(
        find.textContaining('Pretending to be someone else · sent '),
        findsOneWidget,
      );
      expect(find.textContaining('· Reviewed'), findsOneWidget);
    });
  });
}

/// The platform's own HTTP, past flutter_test's stand-in.
class _RealHttp extends HttpOverrides {}
