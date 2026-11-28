// Settings → Move's approval (the account service's API.md, "Moves with an
// account"): only here,
// what the service saw first, the owner authenticates and the device signs
// the approval itself, with an expiry no later than the request's end.
import 'dart:convert';

import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/device_account.dart'
    show AccountServiceError, AccountServiceUnreachable, DeviceAccount;
import 'package:factory_app/device_approval.dart';
import 'package:factory_app/move_approval.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _endpoint =
    'ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12';

/// key_fingerprint of that key, worked out apart from the app (Python:
/// sha256 of its base64url, first 16 hex).
const _code = 'c77ef7e5ebf2a13b';

Map<String, Object?> _json({
  String fingerprint = _code,
  String state = 'waiting',
  bool mayApprove = true,
  String name = 'hetzner-box',
  int expiresIn = 480,
}) => {
  'id': 'req-1',
  'kind': 'move',
  'state': state,
  'started': 1000000,
  'expires_in': expiresIn,
  'seen': {
    'ip': '203.0.113.7',
    'network': 'Hetzner Online GmbH',
    'place': 'Falkenstein, Germany',
  },
  'says': {'name': name, 'platform': 'linux-x86_64', 'version': 'v0.2.0'},
  'fingerprint': fingerprint,
  'may_approve': mayApprove,
  'endpoint': _endpoint,
};

/// This Claktory's word, as ProveAllowedDevice answers: its payload names
/// the account and the Claktory.
final _word =
    '${base64Url.encode(utf8.encode(jsonEncode({'v': 1, 'account_id': 'acct-9', 'claktory_id': 'studio-1', 'account_key': 'k', 'request_id': 'req-1', 'iat': 1000100}))).replaceAll('=', '')}.c2ln';

class _FakeApprovals implements MoveApprovals {
  _FakeApprovals(this.listed);

  List<Map<String, Object?>> listed;
  @override
  bool signedIn = true;
  @override
  final ValueNotifier<int> changes = ValueNotifier(0);
  var looked = 0;

  /// Thrown instead of listing, when set: something other than the
  /// service failing (this device's key not there).
  Object? failLooking;
  final decided = <(String, Map<String, Object?>)>[];
  Object? refuse;

  @override
  String get accountKey => 'aa' * 32;

  @override
  Future<List<WaitingMove>> moves() async {
    looked++;
    if (failLooking case final error?) throw error;
    return [for (final json in listed) WaitingMove.fromJson(json)];
  }

  @override
  Future<void> decide(String id, Map<String, Object?> decision) async {
    decided.add((id, decision));
    if (refuse case final error?) throw error;
    listed = [];
  }
}

class _FakeApprover implements DeviceApprover {
  final asked = <Map<String, Object?>>[];
  OwnerAuthFailure? refuse;

  /// What the signer throws instead of signing (a StateError, say).
  Object? fail;

  @override
  Future<void> confirmOwner({required String reason}) async {
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
  }) async {
    asked.add({
      'account': account,
      'requestId': requestId,
      'accountId': accountId,
      'sourceClaktoryId': sourceClaktoryId,
      'endpoint': endpoint,
      'fingerprint': fingerprint,
      'iat': iat,
      'exp': exp,
      'reason': reason,
    });
    if (fail case final error?) throw error;
    if (refuse case final reason?) throw OwnerNotAuthenticated(reason);
    return 'payload.signature';
  }
}

Finder _button(String label) => find.widgetWithText(Button, label);

void main() {
  late _FakeApprover approver;
  setUp(() {
    approver = _FakeApprover();
    deviceApprover = approver;
  });
  tearDown(() => deviceApprover = approver);

  Future<Harness> show(WidgetTester tester, _FakeApprovals approvals) async {
    final harness = Harness(Workspace(settings: Settings(name: 'Studio')))
      ..backend.allowedWord = _word;
    await harness.show(
      tester,
      home: SingleChildScrollView(
        child: SizedBox(
          width: 600,
          child: AccountMoveApproval(approvals: approvals),
        ),
      ),
    );
    await settle(tester);
    return harness;
  }

  testWidgets('the card leads with what the service saw, then what the '
      'computer calls itself, the fingerprint small; nothing is sent until '
      'Approve', (tester) async {
    final approvals = _FakeApprovals([_json()]);
    final harness = await show(tester, approvals);
    expect(find.text('A computer asked to receive Studio'), findsOneWidget);
    expect(find.text('203.0.113.7'), findsOneWidget);
    expect(find.text('Hetzner Online GmbH'), findsOneWidget);
    expect(find.text('Falkenstein, Germany'), findsOneWidget);
    expect(find.text('It calls itself'), findsOneWidget);
    expect(find.text('hetzner-box · linux-x86_64 · v0.2.0'), findsOneWidget);
    // Made here from the key, in groups to compare by eye.
    expect(
      find.text(
        "This computer's code: C77E F7E5 EBF2 A13B. It must match the code "
        "on the computer you're moving to.",
      ),
      findsOneWidget,
    );
    expect(
      tester.getTopLeft(find.text('203.0.113.7')).dy,
      lessThan(
        tester.getTopLeft(find.text('hetzner-box · linux-x86_64 · v0.2.0')).dy,
      ),
    );
    expect(harness.backend.sent, isEmpty);
    expect(approver.asked, isEmpty);
    expect(approvals.decided, isEmpty);
  });

  testWidgets('Approve: this Claktory vouches, the owner authenticates and '
      'the device signs, ending no later than the request, then the '
      'decision goes with both', (tester) async {
    final approvals = _FakeApprovals([_json()]);
    final harness = await show(tester, approvals);
    await tester.tap(_button('Approve move…'));
    await settle(tester);
    expect(harness.last.proveAllowedDevice.requestId, 'req-1');
    final asked = approver.asked.single;
    // This device's own account, whose key the approver reads itself.
    expect(asked['account'], same(harness.connections.account));
    expect(asked['requestId'], 'req-1');
    expect(asked['accountId'], 'acct-9');
    expect(asked['sourceClaktoryId'], 'studio-1');
    expect(asked['endpoint'], _endpoint);
    expect(asked['fingerprint'], _code);
    // The request's end, on the service's clock: started + 10 minutes.
    expect(asked['exp'], 1000600);
    // Its clock now: the end less what was left, give or take a second.
    expect(asked['iat'] as int, inInclusiveRange(1000120, 1000122));
    // The prompt names the code to compare, not the address.
    expect(asked['reason'], contains('C77E F7E5 EBF2 A13B'));
    final (id, decision) = approvals.decided.single;
    expect(id, 'req-1');
    expect(decision, {
      'approve': true,
      'fingerprint': _code,
      'allowed': _word,
      'move_approval': 'payload.signature',
    });
    expect(
      find.text('Approved. Studio starts moving to that computer in a moment.'),
      findsOneWidget,
    );
    expect(find.text('A computer asked to receive Studio'), findsNothing);
  });

  testWidgets("a fingerprint that isn't the listed key's: no Approve, it "
      'says why, and nothing is signed (Fugo 51031)', (tester) async {
    final approvals = _FakeApprovals([_json(fingerprint: '0123456789abcdef')]);
    final harness = await show(tester, approvals);
    expect(_button('Approve move…'), findsNothing);
    expect(_button('Decline'), findsOneWidget);
    expect(find.textContaining("This computer's code"), findsNothing);
    expect(
      find.textContaining("doesn't match the computer it names"),
      findsOneWidget,
    );
    expect(harness.backend.sent, isEmpty);
    expect(approver.asked, isEmpty);
  });

  test('the code is made from the key, as the service makes it', () {
    expect(endpointFingerprint(_endpoint), _code);
    expect(endpointFingerprint('not hex'), isNull);
    expect(fingerprintGroups(_code), 'C77E F7E5 EBF2 A13B');
  });

  testWidgets('the owner cancels: nothing is decided and nothing said', (
    tester,
  ) async {
    final approvals = _FakeApprovals([_json()]);
    approver.refuse = OwnerAuthFailure.cancelled;
    await show(tester, approvals);
    await tester.tap(_button('Approve move…'));
    await settle(tester);
    expect(approvals.decided, isEmpty);
    expect(find.textContaining('nothing was approved'), findsNothing);
    expect(_button('Approve move…'), findsOneWidget);
  });

  testWidgets("the device can't confirm it's them, or has nothing to ask: "
      'it says so, nothing decided', (tester) async {
    final approvals = _FakeApprovals([_json()]);
    approver.refuse = OwnerAuthFailure.failed;
    await show(tester, approvals);
    await tester.tap(_button('Approve move…'));
    await settle(tester);
    expect(
      find.text(
        "This device couldn't confirm it's you, so nothing was approved.",
      ),
      findsOneWidget,
    );
    approver.refuse = OwnerAuthFailure.unavailable;
    await tester.tap(_button('Approve move…'));
    await settle(tester);
    expect(find.textContaining('set up a passcode'), findsOneWidget);
    expect(approvals.decided, isEmpty);
  });

  testWidgets('signed out meanwhile, or a request the signer refuses: it '
      'says so, nothing decided', (tester) async {
    final approvals = _FakeApprovals([_json()]);
    approver.fail = StateError('not signed in');
    await show(tester, approvals);
    await tester.tap(_button('Approve move…'));
    await settle(tester);
    expect(
      find.text(
        'Sign in to your Claktory account on this device to approve moves.',
      ),
      findsOneWidget,
    );
    approver.fail = ArgumentError('exp too late');
    await tester.tap(_button('Approve move…'));
    await settle(tester);
    expect(
      find.text(
        "This request can't be approved as it is, so nothing was signed. "
        'Start the move again on the other computer.',
      ),
      findsOneWidget,
    );
    expect(approvals.decided, isEmpty);
  });

  testWidgets('Decline sends only the fingerprint, and nothing is signed', (
    tester,
  ) async {
    final approvals = _FakeApprovals([_json()]);
    final harness = await show(tester, approvals);
    await tester.tap(_button('Decline'));
    await settle(tester);
    expect(approvals.decided.single.$2, {
      'approve': false,
      'fingerprint': _code,
    });
    expect(approver.asked, isEmpty);
    expect(harness.backend.sent, isEmpty);
    expect(find.text('Declined. Studio stays where it is.'), findsOneWidget);
  });

  testWidgets("a device signed in through a Claktory can't approve, only "
      'decline', (tester) async {
    await show(tester, _FakeApprovals([_json(mayApprove: false)]));
    expect(_button('Approve move…'), findsNothing);
    expect(_button('Decline'), findsOneWidget);
    expect(
      find.textContaining("This device can't approve moves"),
      findsOneWidget,
    );
  });

  testWidgets("this Claktory says this device can't approve "
      "(Welcome.cannot_approve): its words in place of Approve, Decline "
      'stays; once it clears, Approve is back', (tester) async {
    final harness = await show(tester, _FakeApprovals([_json()]));
    harness.backend.cannotApprove.value =
        'This device was paired with a code. Sign in to approve here.';
    await tester.pump();
    expect(_button('Approve move…'), findsNothing);
    expect(_button('Decline'), findsOneWidget);
    expect(
      find.text('This device was paired with a code. Sign in to approve here.'),
      findsOneWidget,
    );
    harness.backend.cannotApprove.value = '';
    await tester.pump();
    expect(_button('Approve move…'), findsOneWidget);
    expect(find.byKey(const ValueKey('move-why-not')), findsNothing);
  });

  testWidgets('two computers at once: says so, nothing to answer', (
    tester,
  ) async {
    await show(tester, _FakeApprovals([_json(state: 'two_at_once')]));
    expect(
      find.text('Two computers asked to receive Studio at once'),
      findsOneWidget,
    );
    expect(_button('Approve move…'), findsNothing);
    expect(_button('Decline'), findsNothing);
  });

  testWidgets("the service's refusals in the app's words", (tester) async {
    final approvals = _FakeApprovals([_json()])
      ..refuse = const AccountServiceError(409, 'fingerprint_changed', '');
    await show(tester, approvals);
    await tester.tap(_button('Approve move…'));
    await settle(tester);
    expect(
      find.textContaining('The computer asking changed since this was shown'),
      findsOneWidget,
    );
    approvals.refuse = const AccountServiceError(503, 'unavailable', '');
    await tester.tap(_button('Approve move…'));
    await settle(tester);
    expect(find.textContaining("couldn't check your devices"), findsOneWidget);
    approvals.refuse = const AccountServiceUnreachable('down');
    await tester.tap(_button('Decline'));
    await settle(tester);
    expect(
      find.text("Can't reach your account right now, so nothing was decided."),
      findsOneWidget,
    );
  });

  testWidgets('what the computer calls itself shows hidden characters by '
      'code, so it reads as nothing else', (tester) async {
    await show(tester, _FakeApprovals([_json(name: 'box\u202Eevil')]));
    expect(find.textContaining('box\\u{202E}evil'), findsOneWidget);
  });

  testWidgets('not signed in to the account: nothing, and never asks', (
    tester,
  ) async {
    final approvals = _FakeApprovals([_json()])..signedIn = false;
    await show(tester, approvals);
    expect(find.byType(Button), findsNothing);
    expect(approvals.looked, 0);
  });

  test("this Claktory's word names the account and the Claktory", () {
    expect(allowedWordSays(_word), (
      accountId: 'acct-9',
      claktoryId: 'studio-1',
    ));
    expect(allowedWordSays('not-a-word'), isNull);
  });

  testWidgets('signed out while it shows: it goes at once (Abbacchio 51780)', (
    tester,
  ) async {
    final approvals = _FakeApprovals([_json()]);
    await show(tester, approvals);
    expect(find.text('A computer asked to receive Studio'), findsOneWidget);
    approvals
      ..signedIn = false
      ..changes.value += 1;
    await tester.pump();
    expect(find.text('A computer asked to receive Studio'), findsNothing);
    expect(find.byType(Button), findsNothing);
  });

  testWidgets('how the last one ended goes once another request shows '
      '(Abbacchio 51780)', (tester) async {
    final approvals = _FakeApprovals([_json()]);
    await show(tester, approvals);
    await tester.tap(_button('Decline'));
    await settle(tester);
    expect(find.text('Declined. Studio stays where it is.'), findsOneWidget);
    approvals.listed = [
      {..._json(), 'id': 'req-2'},
    ];
    await tester.pump(const Duration(seconds: 5));
    await settle(tester);
    expect(find.text('A computer asked to receive Studio'), findsOneWidget);
    expect(find.text('Declined. Studio stays where it is.'), findsNothing);
  });

  testWidgets("a changed computer, on a Decline, says nothing was decided "
      '(Abbacchio 51780)', (tester) async {
    final approvals = _FakeApprovals([_json()])
      ..refuse = const AccountServiceError(409, 'fingerprint_changed', '');
    await show(tester, approvals);
    await tester.tap(_button('Decline'));
    await settle(tester);
    expect(find.textContaining('so nothing was decided'), findsOneWidget);
    expect(find.textContaining('so nothing was approved'), findsNothing);
  });

  testWidgets("looking fails for another reason: nothing thrown, it shows "
      'nothing', (tester) async {
    final approvals = _FakeApprovals([_json()])
      ..failLooking = ArgumentError('no key library');
    await show(tester, approvals);
    expect(tester.takeException(), isNull);
    expect(find.byType(Button), findsNothing);
  });
}
