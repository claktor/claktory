// A computer joining the account with no code (the account service's
// API.md, "Adding a computer with the person's approval"): the Claktory
// only rings the bell, Needs you lists a quiet item, and its card fetches
// the request itself, leads with what the service saw, sets the computer's
// own words apart, and approves only after the owner authenticates, then
// this Claktory vouches; "Connect this Mac to it" adds its tile without
// switching to it.
import 'dart:async';

import 'package:factory_app/account_join.dart';
import 'package:factory_app/card_answer.dart' show AnswerDock, CardAnswer;
import 'package:factory_app/controls.dart' show Button, Toggle, hasLucideIcon;
import 'package:factory_app/device_account.dart'
    show
        AccountClaktory,
        AccountServiceError,
        AccountServiceUnreachable,
        DeviceAccount;
import 'package:factory_app/device_approval.dart';
import 'package:factory_app/inbox.dart'
    show NeedsYouKind, needsYouCount, needsYouItems;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Map<String, Object?> _json({
  String id = 'req-1',
  String state = 'waiting',
  String answeredBy = '',
  bool mayApprove = true,
  String name = 'nixos',
  int expiresIn = 530,
  String kind = 'claktory',
}) => {
  'id': id,
  'kind': kind,
  'state': state,
  'started': 1000000,
  'expires_in': expiresIn,
  'seen': {
    'ip': '203.0.113.4',
    'network': 'Hetzner Online',
    'place': 'Falkenstein, Germany',
  },
  'says': {'name': name, 'platform': 'linux-x86_64', 'version': 'v0.2.0'},
  'fingerprint': '7f3a9c01d2e4b5a6',
  'may_approve': mayApprove,
  if (answeredBy.isNotEmpty) 'answered_by': answeredBy,
};

const _joined = AccountClaktory(
  id: 'new-1',
  name: 'nixos',
  fingerprint: '7f3a9c01d2e4b5a6',
  endpointId:
      'cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34',
);

class _FakeJoins implements JoinApprovals {
  _FakeJoins(this.listed);

  List<Map<String, Object?>> listed;
  @override
  bool signedIn = true;
  @override
  final ValueNotifier<int> changes = ValueNotifier(0);
  @override
  String email = 'matt@example.com';
  var looked = 0;
  final decided = <(String, Map<String, Object?>)>[];
  Object? refuse;

  /// The account's Claktories: before the approval, then after.
  var claktoriesBefore = <AccountClaktory>[];
  var claktoriesAfter = <AccountClaktory>[_joined];
  var listedClaktories = 0;

  /// Held until completed, when set: the service answering late.
  Completer<void>? gate;

  @override
  Future<List<JoinRequest>> requests() async {
    looked++;
    await gate?.future;
    return [for (final json in listed) JoinRequest.fromJson(json)];
  }

  @override
  Future<void> decide(String id, Map<String, Object?> decision) async {
    decided.add((id, decision));
    if (refuse case final error?) throw error;
  }

  @override
  Future<List<AccountClaktory>> claktories() async {
    listedClaktories++;
    return decided.isEmpty ? claktoriesBefore : claktoriesAfter;
  }
}

class _FakeApprover implements DeviceApprover {
  final reasons = <String>[];
  OwnerAuthFailure? refuse;

  /// What the backend had been sent when the owner was asked.
  List<ClientMessage>? sentWhenAsked;
  Harness? harness;

  @override
  Future<void> confirmOwner({required String reason}) async {
    reasons.add(reason);
    sentWhenAsked = [...?harness?.backend.sent];
    if (refuse case final reason?) throw OwnerNotAuthenticated(reason);
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
  }) => throw UnimplementedError('Joins sign nothing.');
}

Workspace _workspace({List<String> waiting = const ['req-1']}) => Workspace(
  settings: Settings(name: 'Studio'),
  account: ClaktoryAccount(
    state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN,
    email: 'matt@example.com',
    authorizationsWaiting: waiting,
  ),
);

Finder _button(String label) => find.widgetWithText(Button, label);

void main() {
  late _FakeApprover approver;
  setUp(() {
    approver = _FakeApprover();
    deviceApprover = approver;
    forgetSettledJoins();
  });

  Future<Harness> show(
    WidgetTester tester,
    _FakeJoins joins, {
    Workspace? workspace,
  }) async {
    final harness = Harness(workspace ?? _workspace())
      ..backend.allowedWord = 'word.sig';
    approver.harness = harness;
    await harness.show(
      tester,
      home: SingleChildScrollView(
        child: SizedBox(
          width: 600,
          child: AccountJoinCard(
            id: 'req-1',
            approvals: joins,
            findEvery: const Duration(seconds: 1),
          ),
        ),
      ),
    );
    await settle(tester);
    return harness;
  }

  testWidgets('the card leads with what the service saw, then what the '
      'computer calls itself, set apart; Connect is on; the fingerprint is '
      'small; nothing is sent until Approve', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final joins = _FakeJoins([_json()]);
    final harness = await show(tester, joins);
    expect(find.text('Approve a new Claktory?'), findsOneWidget);
    expect(find.text('Hetzner Online · 203.0.113.4'), findsOneWidget);
    expect(find.text('Falkenstein, Germany'), findsOneWidget);
    expect(find.textContaining('8 min left'), findsOneWidget);
    expect(find.text('It calls itself'), findsOneWidget);
    expect(find.text('nixos · Linux'), findsOneWidget);
    expect(
      tester.getTopLeft(find.text('Hetzner Online · 203.0.113.4')).dy,
      lessThan(tester.getTopLeft(find.text('nixos · Linux')).dy),
    );
    final toggle = tester.widget<Toggle>(find.byType(Toggle));
    expect(toggle.label, 'Connect this Mac to it');
    expect(toggle.value, isTrue);
    expect(find.text('For support: 7F3A 9C01 D2E4 B5A6'), findsOneWidget);
    // Decline, then Approve at the right end (Lucy's needs-you-cards rule
    // 2), as agents' requests have them.
    expect(
      tester.getTopLeft(_button('Decline')).dx,
      lessThan(tester.getTopLeft(_button('Approve')).dx),
    );
    expect(tester.getRect(_button('Approve')).right, greaterThan(600 - 40));
    expect(harness.backend.sent, isEmpty);
    expect(approver.reasons, isEmpty);
    expect(joins.decided, isEmpty);
  });

  testWidgets('Approve: the owner authenticates first, then this Claktory '
      'vouches, and the decision goes with the fingerprint and its word; '
      'the new Claktory is added, the window stays', (tester) async {
    final joins = _FakeJoins([_json()]);
    final harness = await show(tester, joins);
    await tester.tap(_button('Approve'));
    await settle(tester);
    // What the service saw, never the computer's own name.
    expect(
      approver.reasons.single,
      'approve the computer at 203.0.113.4 joining your account',
    );
    // Nothing asked of the Claktory before the owner said yes.
    expect(approver.sentWhenAsked, isEmpty);
    expect(harness.last.proveAllowedDevice.requestId, 'req-1');
    final (id, decision) = joins.decided.single;
    expect(id, 'req-1');
    expect(decision, {
      'approve': true,
      'fingerprint': '7f3a9c01d2e4b5a6',
      'allowed': 'word.sig',
    });
    expect(find.text('nixos joined your account'), findsOneWidget);
    expect(_button('Approve'), findsNothing);
    await tester.pump(const Duration(seconds: 2));
    await settle(tester);
    expect(harness.connections.saved.single.accountId, 'new-1');
    expect(harness.connections.saved.single.endpointId, _joined.endpointId);
    expect(harness.connections.current, isNull);
    // Answered here: Needs you lists it no longer.
    expect(
      needsYouItems(_workspace()).where((i) => i.kind == NeedsYouKind.join),
      isEmpty,
    );
  });

  testWidgets('Connect off: approved, and nothing added', (tester) async {
    final joins = _FakeJoins([_json()]);
    final harness = await show(tester, joins);
    await tester.tap(find.byType(Toggle));
    await settle(tester);
    await tester.tap(_button('Approve'));
    await settle(tester);
    expect(joins.decided.single.$2['approve'], isTrue);
    await tester.pump(const Duration(seconds: 2));
    await settle(tester);
    expect(joins.listedClaktories, 0);
    expect(harness.connections.saved, isEmpty);
  });

  testWidgets("the new Claktory isn't listed in time: it says where to "
      'connect from', (tester) async {
    final joins = _FakeJoins([_json()])..claktoriesAfter = [];
    final harness = await show(tester, joins);
    await tester.tap(_button('Approve'));
    await settle(tester);
    await tester.pump(const Duration(minutes: 4));
    await settle(tester);
    expect(
      find.text(
        "nixos joined your account. When it's ready, connect to it in Add a "
        'Claktory, under On your account.',
      ),
      findsOneWidget,
    );
    expect(harness.connections.saved, isEmpty);
  });

  testWidgets('the owner cancels: nothing asked of the Claktory, nothing '
      'decided, the card as it was', (tester) async {
    final joins = _FakeJoins([_json()]);
    approver.refuse = OwnerAuthFailure.cancelled;
    final harness = await show(tester, joins);
    await tester.tap(_button('Approve'));
    await settle(tester);
    expect(harness.backend.sent, isEmpty);
    expect(joins.decided, isEmpty);
    expect(find.textContaining('nothing was approved'), findsNothing);
    expect(_button('Approve'), findsOneWidget);
  });

  testWidgets("the device can't confirm it's them, or has nothing to ask: "
      'it says so, nothing decided', (tester) async {
    final joins = _FakeJoins([_json()]);
    approver.refuse = OwnerAuthFailure.failed;
    await show(tester, joins);
    await tester.tap(_button('Approve'));
    await settle(tester);
    expect(
      find.text(
        "This device couldn't confirm it's you, so nothing was approved.",
      ),
      findsOneWidget,
    );
    approver.refuse = OwnerAuthFailure.unavailable;
    await tester.tap(_button('Approve'));
    await settle(tester);
    expect(find.textContaining('set up a passcode'), findsOneWidget);
    expect(joins.decided, isEmpty);
  });

  testWidgets('Decline sends only the fingerprint, with no owner prompt', (
    tester,
  ) async {
    final joins = _FakeJoins([_json()]);
    final harness = await show(tester, joins);
    await tester.tap(_button('Decline'));
    await settle(tester);
    expect(joins.decided.single.$2, {
      'approve': false,
      'fingerprint': '7f3a9c01d2e4b5a6',
    });
    expect(approver.reasons, isEmpty);
    expect(harness.backend.sent, isEmpty);
    expect(find.text('Declined. Nothing joined your account.'), findsOneWidget);
  });

  testWidgets("a device signed in through a Claktory can't approve: it says "
      'why, and can decline', (tester) async {
    await show(tester, _FakeJoins([_json(mayApprove: false)]));
    expect(_button('Approve'), findsNothing);
    expect(find.byType(Toggle), findsNothing);
    expect(_button('Decline'), findsOneWidget);
    expect(find.byKey(const ValueKey('join-why-not')), findsOneWidget);
  });

  testWidgets("this Claktory says this device can't approve "
      "(Welcome.cannot_approve): its words in place of Approve, Decline "
      'stays; once it clears, Approve is back', (tester) async {
    final harness = await show(tester, _FakeJoins([_json()]));
    harness.backend.cannotApprove.value =
        'This device was paired with a code. Sign in to approve here.';
    await tester.pump();
    expect(_button('Approve'), findsNothing);
    expect(find.byType(Toggle), findsNothing);
    expect(_button('Decline'), findsOneWidget);
    expect(
      find.text('This device was paired with a code. Sign in to approve here.'),
      findsOneWidget,
    );
    harness.backend.cannotApprove.value = '';
    await tester.pump();
    expect(_button('Approve'), findsOneWidget);
    expect(find.byKey(const ValueKey('join-why-not')), findsNothing);
  });

  testWidgets("told it can't approve, the answer a phone's thumb row or "
      'focus mode docks has no Approve either', (tester) async {
    final answer = ValueNotifier<CardAnswer?>(null);
    addTearDown(answer.dispose);
    final harness = Harness(_workspace())..backend.allowedWord = 'word.sig';
    harness.backend.cannotApprove.value = 'Sign in to approve here.';
    await harness.show(
      tester,
      home: AnswerDock(
        answer: answer,
        child: SingleChildScrollView(
          child: AccountJoinCard(id: 'req-1', approvals: _FakeJoins([_json()])),
        ),
      ),
    );
    await settle(tester);
    expect(answer.value?.yes, '');
    expect(answer.value?.onYes, isNull);
    expect(answer.value?.no, 'Decline');
    harness.backend.cannotApprove.value = '';
    await settle(tester);
    expect(answer.value?.yes, 'Approve');
  });

  test(
    'findJoined finds nothing for an empty fingerprint (Fugo 51905)',
    () async {
      final blank = AccountClaktory(
        id: 'blank-1',
        name: 'nixos',
        fingerprint: '',
        endpointId: 'ab' * 32,
      );
      final joins = _FakeJoins([_json()])..claktoriesBefore = [blank];
      const now = Duration(milliseconds: 1);
      expect(
        await findJoined(joins, const {}, '', every: now, giveUp: now),
        isNull,
      );
    },
  );

  testWidgets('two computers at once: says so, and OK sets it aside', (
    tester,
  ) async {
    final joins = _FakeJoins([_json(state: 'two_at_once')]);
    await show(tester, joins);
    expect(
      find.text(
        'Two computers asked at once, so neither joined. Start again on the '
        'one you want.',
      ),
      findsOneWidget,
    );
    expect(_button('Approve'), findsNothing);
    expect(_button('Decline'), findsNothing);
    await tester.tap(_button('OK'));
    await settle(tester);
    expect(_button('OK'), findsNothing);
    expect(joinsWaiting(_workspace()), isEmpty);
    expect(joins.decided, isEmpty);
  });

  testWidgets('out of time: "Expired before anyone approved it", OK only', (
    tester,
  ) async {
    await show(tester, _FakeJoins([_json(expiresIn: 0)]));
    expect(find.text('Expired before anyone approved it'), findsOneWidget);
    expect(_button('Approve'), findsNothing);
    expect(_button('Decline'), findsNothing);
    expect(_button('OK'), findsOneWidget);
  });

  testWidgets('the bell stops while it shows: answered on another device', (
    tester,
  ) async {
    final joins = _FakeJoins([_json()]);
    final harness = await show(tester, joins);
    joins.listed = [];
    await harness.apply(tester, _workspace(waiting: []));
    await settle(tester);
    expect(find.text('Answered on another device'), findsOneWidget);
    expect(_button('Approve'), findsNothing);
  });

  testWidgets("the service's refusals in the app's words", (tester) async {
    final joins = _FakeJoins([_json()])
      ..refuse = const AccountServiceError(403, 'not_allowed_to_approve', '');
    await show(tester, joins);
    await tester.tap(_button('Approve'));
    await settle(tester);
    expect(
      find.textContaining("This device can't approve it."),
      findsOneWidget,
    );
    joins.refuse = const AccountServiceError(409, 'fingerprint_changed', '');
    await tester.tap(_button('Approve'));
    await settle(tester);
    expect(
      find.textContaining(
        'The computer asking changed after this was shown, so nothing was '
        'approved',
      ),
      findsOneWidget,
    );
    joins.refuse = const AccountServiceUnreachable('down');
    await tester.tap(_button('Decline'));
    await settle(tester);
    expect(
      find.text(
        "Can't reach your account right now, so nothing was decided. Try "
        'again.',
      ),
      findsOneWidget,
    );
    joins.refuse = const AccountServiceError(410, 'expired_token', '');
    await tester.tap(_button('Approve'));
    await settle(tester);
    expect(find.text('Expired before anyone approved it'), findsOneWidget);
  });

  testWidgets('what the computer calls itself shows hidden characters by '
      'code', (tester) async {
    await show(tester, _FakeJoins([_json(name: 'box\u202Eevil')]));
    expect(find.textContaining('box\\u{202E}evil'), findsOneWidget);
  });

  testWidgets('not signed in on this device: how to, and never asks', (
    tester,
  ) async {
    final joins = _FakeJoins([_json()])..signedIn = false;
    await show(tester, joins);
    expect(find.textContaining('sign in on this device'), findsOneWidget);
    expect(find.byType(Button), findsNothing);
    expect(joins.looked, 0);
  });

  testWidgets('signed in to another account: says which, and no answers', (
    tester,
  ) async {
    final joins = _FakeJoins([_json()])..email = 'ana@example.com';
    await show(tester, joins);
    expect(
      find.textContaining('This device is signed in to ana@example.com'),
      findsOneWidget,
    );
    expect(find.byType(Button), findsNothing);
  });

  test('the bell makes a quiet item each, less those answered here', () {
    final items = needsYouItems(_workspace(waiting: ['req-1', 'req-2']));
    expect(
      [
        for (final item in items)
          if (item.kind == NeedsYouKind.join) item.key,
      ],
      ['join:req-1', 'join:req-2'],
    );
    expect(needsYouCount(_workspace()), 1);
    expect(hasLucideIcon('server'), isTrue);
  });

  test('time left, as the row says it', () {
    final at = DateTime(2026, 11, 27, 9);
    JoinRequest request(int left) =>
        JoinRequest(id: 'r', expiresIn: left, fetched: at);
    expect(joinTimeLeft(request(530), at), '8 min left');
    expect(
      joinTimeLeft(request(530), at.add(const Duration(minutes: 2))),
      '6 min left',
    );
    expect(joinTimeLeft(request(59), at), 'Less than a minute left');
    expect(joinTimeLeft(request(0), at), '');
    expect(request(530).asked, at.subtract(const Duration(seconds: 70)));
    expect(joinPlatform('linux-aarch64'), 'Linux');
    expect(joinPlatform('plan9'), 'plan9');
  });

  testWidgets('in Needs you: its row says who asks and the time left', (
    tester,
  ) async {
    final joins = _FakeJoins([_json()])..gate = Completer();
    final harness = Harness(_workspace());
    JoinWatch.of(harness.backend).watchWith(joins);
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await settle(tester);
    expect(find.text('A new Claktory asks to join your account'), findsWidgets);
    expect(find.text('Waiting for your answer'), findsWidgets);
    // The service answers: the row says its time left at once.
    joins.gate!.complete();
    await tester.pump();
    await tester.pump();
    expect(find.text('8 min left'), findsWidgets);
    expect(joins.looked, greaterThan(0));
    await tester.tap(
      find.text('A new Claktory asks to join your account').first,
    );
    await settle(tester);
    await tester.pump(const Duration(seconds: 1));
    expect(find.text('Approve a new Claktory?'), findsOneWidget);
    // Open, the line under the title says only what the card doesn't.
    expect(find.text('8 min left'), findsOneWidget);
    harness.backend.allowedWord = 'word.sig';
    final approve = find.widgetWithText(Button, 'Approve');
    await tester.ensureVisible(approve);
    await settle(tester);
    await tester.tap(approve);
    await settle(tester);
    await tester.pump(const Duration(seconds: 2));
    await settle(tester);
    // How it ended, said once.
    expect(find.text('nixos joined your account'), findsOneWidget);
  });

  testWidgets('on an iPhone: the same card, the switch a 44-point row, '
      'Decline then Approve at the right', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final joins = _FakeJoins([_json()]);
    final harness = Harness(_workspace());
    JoinWatch.of(harness.backend).watchWith(joins);
    await harness.show(tester, size: const Size(375, 667));
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.pump();
    await pumpFrames(tester);
    await tester.tap(
      find.text('A new Claktory asks to join your account').first,
    );
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(tester.takeException(), isNull);
    expect(find.text('Connect this iPhone to it'), findsOneWidget);
    final row = find.ancestor(
      of: find.text('Connect this iPhone to it'),
      matching: find.byType(Row),
    );
    expect(tester.getSize(row.first).height, greaterThanOrEqualTo(44));
    // Tapping its words turns it off.
    await tester.tap(find.text('Connect this iPhone to it'));
    await pumpFrames(tester);
    expect(tester.widget<Toggle>(find.byType(Toggle)).value, isFalse);
    final approve = _button('Approve');
    await tester.ensureVisible(approve);
    await pumpFrames(tester);
    expect(tester.getRect(approve).right, lessThanOrEqualTo(375));
    // At the right, under the thumb.
    expect(tester.getRect(approve).right, greaterThan(375 - 60));
    expect(
      tester.getTopLeft(_button('Decline')).dx,
      lessThan(tester.getTopLeft(approve).dx),
    );
  });

  testWidgets('answered on another device: the card and its row say which '
      'and how (Lucy 51352, Cioccolata 51750)', (tester) async {
    final joins = _FakeJoins([_json()]);
    final harness = await show(tester, joins);
    expect(_button('Approve'), findsOneWidget);
    // Approved on the iPhone: the bell stops, and the service says so.
    joins.listed = [_json(state: 'approved', answeredBy: "Matt's iPhone")];
    await harness.apply(tester, _workspace(waiting: []));
    await settle(tester);
    expect(find.text("Approved on Matt's iPhone"), findsOneWidget);
    expect(_button('Approve'), findsNothing);
    expect(_button('Decline'), findsNothing);
    expect(
      joinWords(harness.backend, 'req-1').what,
      "Approved on Matt's iPhone",
    );
  });

  testWidgets('declined on a device no longer on the account: "another '
      'device"', (tester) async {
    final joins = _FakeJoins([_json(state: 'declined')]);
    await show(tester, joins, workspace: _workspace(waiting: []));
    expect(find.text('Declined on another device'), findsOneWidget);
  });

  testWidgets('connects only to the Claktory with the fingerprint approved, '
      'not another that joined at once (Fugo 51762)', (tester) async {
    final other = AccountClaktory(
      id: 'other-1',
      name: 'nixos',
      fingerprint: '0000000000000000',
      endpointId: 'ab' * 32,
    );
    final joins = _FakeJoins([_json()])..claktoriesAfter = [other, _joined];
    final harness = await show(tester, joins);
    await tester.tap(_button('Approve'));
    await settle(tester);
    await tester.pump(const Duration(seconds: 2));
    await settle(tester);
    expect(harness.connections.saved.single.accountId, 'new-1');
  });

  testWidgets('signed out while it shows: how to sign in, at once', (
    tester,
  ) async {
    final joins = _FakeJoins([_json()]);
    await show(tester, joins);
    expect(_button('Approve'), findsOneWidget);
    joins
      ..signedIn = false
      ..changes.value += 1;
    await tester.pump();
    expect(_button('Approve'), findsNothing);
    expect(find.textContaining('sign in on this device'), findsOneWidget);
  });

  testWidgets('a changed computer on a Decline says nothing was decided', (
    tester,
  ) async {
    final joins = _FakeJoins([_json()])
      ..refuse = const AccountServiceError(409, 'fingerprint_changed', '');
    await show(tester, joins);
    await tester.tap(_button('Decline'));
    await settle(tester);
    expect(find.textContaining('so nothing was decided'), findsOneWidget);
  });

  test("the account's list reads each Claktory's fingerprint", () {
    final listed = AccountClaktory.fromJson({
      'claktory_id': 'new-1',
      'name': 'nixos',
      'fingerprint': '7F3A9C01D2E4B5A6',
    });
    expect(listed.fingerprint, '7f3a9c01d2e4b5a6');
  });
}
