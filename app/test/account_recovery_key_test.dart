// The account's recovery key in Settings → Secrets (main's half):
// covered, made on which Claktory and when; a new key on the account to
// use here only if the person made it (UseRecoveryKey); Account's Show
// recovery key… gone once no Claktory keeps the key, and Make a new
// recovery key… there instead.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recovery_key.dart';
import 'package:factory_app/recovery_key_make.dart' show MakeRecoveryKeyForm;
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'account_test.dart' show signedIn, withAccount;
import 'harness.dart';

final _now = Int64(DateTime.now().millisecondsSinceEpoch);

Workspace _workspace({
  bool covered = true,
  bool showable = false,
  bool used = false,
  String madeOn = 'Mac Studio',
  bool change = false,
}) {
  final snapshot = withAccount(signedIn);
  snapshot.vault = VaultState(
    exists: true,
    unlocked: true,
    recoveryKeyCovered: covered,
    recoveryKeyMadeOn: madeOn,
    recoveryKeyMadeMs: _now,
    recoveryKeyShowable: showable,
    recoveryKeyUsed: used,
    recoveryKeyChangeWaiting: change,
    recoveryKeyChangeMadeOn: change ? 'Work Laptop' : '',
    recoveryKeyChangeMadeMs: change ? _now : Int64.ZERO,
  );
  return snapshot;
}

Future<Harness> _open(
  WidgetTester tester,
  Workspace snapshot, {
  String section = 'secrets',
}) async {
  final harness = Harness(snapshot);
  await harness.show(tester, size: const Size(1200, 900));
  await settle(tester);
  harness.page.value = AppPage.settings;
  await settle(tester);
  harness.backend.settingsSection.value = section;
  await settle(tester);
  return harness;
}

void main() {
  test('who made it: untrusted words on one line, a removed Claktory said '
      'so', () {
    expect(recoveryKeyMaker(''), 'a Claktory no longer on your account');
    expect(recoveryKeyMaker('  '), 'a Claktory no longer on your account');
    final odd = recoveryKeyMaker('Mac\nStudio\u202e');
    expect(odd, isNot(contains('\n')));
    expect(odd, isNot(contains('\u202e')));
    expect(odd, startsWith('Mac Studio'));
    // However long claktory.com lets it be.
    final long = recoveryKeyMaker('M' * 500);
    expect(long.runes.length, 60);
    expect(long, endsWith('…'));
  });

  testWidgets('covered: Secrets says by the account, made where and when; '
      'Account has no Show recovery key…, but Make a new recovery key…', (
    tester,
  ) async {
    final harness = await _open(tester, _workspace(used: true));
    expect(find.text("Covered by your account's recovery key"), findsOneWidget);
    expect(
      find.text(
        'Made on Mac Studio, today. You used it here to open your secrets.',
      ),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('recovery-key-change')), findsNothing);
    expect(find.textContaining('Make a new recovery key'), findsNothing);

    harness.backend.settingsSection.value = 'account';
    await settle(tester);
    expect(find.text('Manage account at claktory.com'), findsOneWidget);
    expect(find.text('Show recovery key…'), findsNothing);
    await tester.tap(find.text('Make a new recovery key…'));
    await settle(tester);
    expect(find.byType(MakeRecoveryKeyForm), findsOneWidget);
  });

  testWidgets('covered but still kept here: Show recovery key… stays', (
    tester,
  ) async {
    await _open(tester, _workspace(showable: true), section: 'account');
    expect(find.text('Show recovery key…'), findsOneWidget);
  });

  testWidgets('a vault with its own key: nothing new in Secrets', (
    tester,
  ) async {
    await _open(tester, _workspace(covered: false));
    expect(find.text("Covered by your account's recovery key"), findsNothing);
    expect(find.byType(AccountRecoveryKey), findsOneWidget);
    expect(find.text('Recovery key'), findsNothing);
  });

  testWidgets('made on a Claktory since removed says so', (tester) async {
    await _open(tester, _workspace(madeOn: ''));
    expect(
      find.text('Made on a Claktory no longer on your account, today.'),
      findsOneWidget,
    );
  });

  testWidgets('a new key: the warning, Use the key here sends it, and a '
      'refusal says why', (tester) async {
    final harness = await _open(tester, _workspace(change: true));
    expect(find.byKey(const ValueKey('recovery-key-change')), findsOneWidget);
    expect(find.text('Your account has a new recovery key'), findsOneWidget);
    expect(
      find.textContaining('Made on Work Laptop, today. Use it here only if'),
      findsOneWidget,
    );
    expect(find.textContaining('Make a new recovery key'), findsNothing);

    harness.backend.refuse =
        'To use the new key, sign in to your Claktory account on this device '
        'first.';
    await tester.tap(find.text('Use the key here'));
    await settle(tester);
    expect(harness.last.hasUseRecoveryKey(), isTrue);
    expect(find.text(harness.backend.refuse!), findsOneWidget);

    harness.backend.refuse = null;
    await tester.tap(find.text('Use the key here'));
    await settle(tester);
    expect(
      harness.backend.sent.where((m) => m.hasUseRecoveryKey()),
      hasLength(2),
    );
    expect(
      find.textContaining('sign in to your Claktory account on this device'),
      findsNothing,
    );

    // The backend used it: the warning goes.
    await harness.apply(tester, _workspace(used: true));
    expect(find.byKey(const ValueKey('recovery-key-change')), findsNothing);
    expect(find.text("Covered by your account's recovery key"), findsOneWidget);
  });

  testWidgets('on an iPhone the warning fits and its button shows', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace(change: true));
    await harness.show(tester, size: const Size(375, 667));
    await settle(tester);
    harness.page.value = AppPage.settings;
    await settle(tester);
    harness.backend.settingsSection.value = 'secrets';
    await pumpFrames(tester, const Duration(seconds: 2));
    await revealInSettings(tester, find.text('Use the key here'));
    expect(tester.takeException(), isNull);
    final button = tester.getRect(find.text('Use the key here'));
    expect(button.right, lessThanOrEqualTo(375));
  });
}
