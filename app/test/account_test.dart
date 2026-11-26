import 'package:factory_app/account.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/name_step.dart' show nameFromEmail;
import 'package:factory_app/controls.dart' show Field;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';

/// The sign-in step's field.
final field = find.descendant(
  of: find.byType(SignInStep),
  matching: find.byType(EditableText),
);

const signedOut = ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_OUT;
const awaiting = ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_AWAITING_CODE;
const signedIn = ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN;
const revoked = ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_REVOKED;

Int64 ago(Duration duration) =>
    Int64(DateTime.now().subtract(duration).millisecondsSinceEpoch);

/// A workspace whose Claktory account is in [state]; none when null, as
/// while the backend has the account off.
Workspace withAccount(ClaktoryAccountState? state) {
  final snapshot = workspace();
  if (state == null) return snapshot;
  return snapshot
    ..account = ClaktoryAccount(
      state: state,
      email: state == signedOut ? '' : 'matt@example.com',
      service: 'https://account.claktory.com',
      name: 'Mac mini',
      lastContactMs: Int64(
        DateTime.now()
            .subtract(const Duration(hours: 2))
            .millisecondsSinceEpoch,
      ),
      codeExpiresMs: state == awaiting
          ? Int64(
              DateTime.now()
                  .add(const Duration(minutes: 15))
                  .millisecondsSinceEpoch,
            )
          : null,
      lastSignInJson: state == signedIn
          ? '{"email":"matt@example.com","version":"0.1.2"}'
          : null,
      lastCheckInJson: state == signedIn
          ? '{"version":"0.1.2","platform":"macos-arm64"}'
          : null,
      lastSignInMs: state == signedIn ? ago(const Duration(days: 2)) : null,
      lastCheckInMs: state == signedIn ? ago(const Duration(minutes: 5)) : null,
    );
}

/// Moves the backend's account to [state], as a new snapshot would.
Future<void> become(
  WidgetTester tester,
  Harness harness,
  ClaktoryAccountState state,
) async {
  final snapshot = harness.backend.workspace.workspace.deepCopy();
  snapshot.account.state = state;
  if (state == awaiting) {
    snapshot.account.email = 'matt@example.com';
    snapshot.account.codeExpiresMs = Int64(
      DateTime.now().add(const Duration(minutes: 15)).millisecondsSinceEpoch,
    );
  }
  await harness.apply(tester, snapshot);
  await pumpFrames(tester);
}

/// Refuses the request [matches] picks with [code] and [message].
void refuseWhen(
  Harness harness,
  bool Function(ClientMessage message) matches,
  ErrorCode code,
  String message,
) {
  harness.backend.onSend = (sent) {
    if (matches(sent)) throw RequestFailure(message, code: code);
  };
}

Future<Harness> showAccount(
  WidgetTester tester,
  ClaktoryAccountState? state, {
  bool named = true,
  Size size = const Size(1200, 900),
}) async {
  final harness = Harness(withAccount(state), named: named);
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  return harness;
}

Future<void> openSettings(WidgetTester tester, Harness harness) async {
  harness.page.value = AppPage.settings;
  await settle(tester);
}

Future<void> reveal(WidgetTester tester, Finder target) async {
  await tester.scrollUntilVisible(
    target,
    200,
    scrollable: find
        .descendant(
          of: find.byType(SettingsView),
          matching: find.byType(Scrollable),
        )
        .first,
  );
  await pumpFrames(tester);
}

void main() {
  testWidgets('with the account off, nothing of it shows', (tester) async {
    final harness = await showAccount(tester, null);
    expect(find.text('Sign in to Claktory'), findsNothing);
    await openSettings(tester, harness);
    expect(find.text('Account'), findsNothing);
  });

  testWidgets('the first launch signs in with an emailed code, then asks '
      'the name', (tester) async {
    final harness = await showAccount(tester, signedOut, named: false);
    expect(find.text('Sign in to Claktory'), findsOneWidget);
    expect(find.text("What's your name?"), findsNothing);

    await tester.enterText(field, 'matt@');
    await tester.pump();
    await tester.tap(find.text('Send code'));
    await tester.pump();
    expect(
      find.text('Enter an email address, like you@example.com.'),
      findsOneWidget,
    );
    expect(harness.backend.sent, isEmpty);

    await tester.enterText(field, ' matt@example.com ');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pump();
    expect(harness.last.accountSignIn.email, 'matt@example.com');
    await become(tester, harness, awaiting);

    expect(find.text('Check your email'), findsOneWidget);
    expect(
      find.text(
        'Enter the 6-digit code sent to matt@example.com. It works for 15 '
        'minutes.',
      ),
      findsOneWidget,
    );
    expect(find.textContaining('Resend in'), findsOneWidget);
    expect(find.text("Can't find it? Check your spam folder."), findsOneWidget);
    // Only digits count; the sixth sends it.
    await tester.enterText(field, '123 45');
    await tester.pump();
    expect(harness.backend.sent, hasLength(1));
    await tester.enterText(field, '123 456');
    await tester.pump();
    expect(harness.last.accountVerify.code, '123456');

    await become(tester, harness, signedIn);
    expect(find.text('Check your email'), findsNothing);
    expect(find.text("What's your name?"), findsOneWidget);
    // Empty, with a guess from the email offered under it.
    final name = tester.widget<EditableText>(find.byType(EditableText).last);
    expect(name.controller.text, isEmpty);
    await tester.tap(find.text('Matt?'));
    await tester.pump();
    expect(name.controller.text, 'Matt');
    expect(find.text('Matt?'), findsNothing);
    await tester.tap(find.text('Continue'));
    await tester.pump();
    expect(harness.last.updateSettings.userName, 'Matt');
  });

  test('a name is guessed from the email, unless it looks like no first '
      'name', () {
    expect(nameFromEmail('matt.k@gmail.com'), 'Matt');
    expect(nameFromEmail('matt_k42@example.com'), 'Matt');
    expect(nameFromEmail('MATT@example.com'), 'Matt');
    expect(nameFromEmail('kenji+claktory@example.com'), 'Kenji');
    expect(nameFromEmail('mcKay@example.com'), 'McKay');
    expect(nameFromEmail('2zoe@example.com'), 'Zoe');
    expect(nameFromEmail('info@example.com'), isNull);
    expect(nameFromEmail('claktory-test-7@gmail.com'), isNull);
    expect(nameFromEmail('test.user@example.com'), isNull);
    expect(nameFromEmail('demo@example.com'), isNull);
    expect(nameFromEmail('acme@acme.com'), isNull);
    expect(nameFromEmail('matt@mail.acme.co.uk'), 'Matt');
    expect(nameFromEmail('j@example.com'), isNull);
    expect(nameFromEmail('42@example.com'), isNull);
    expect(nameFromEmail(''), isNull);
  });

  testWidgets('a wrong code says how many tries are left, and none left '
      'offers a new code', (tester) async {
    final harness = await showAccount(tester, awaiting);
    refuseWhen(
      harness,
      (sent) => sent.hasAccountVerify(),
      ErrorCode.ERROR_CODE_WRONG_PROOF,
      "That code isn't right. 2 tries left.",
    );
    await tester.enterText(field, '111111');
    await tester.pump();
    expect(find.text("That code isn't right. 2 tries left."), findsOneWidget);
    expect(find.text('Send new code'), findsNothing);
    expect(
      tester.widget<EditableText>(field).controller.text,
      isEmpty,
      reason: 'ready for the code again',
    );

    refuseWhen(
      harness,
      (sent) => sent.hasAccountVerify(),
      ErrorCode.ERROR_CODE_WRONG_PROOF,
      'Too many tries. Get a new code.',
    );
    await tester.enterText(field, '222222');
    await tester.pump();
    expect(find.text('Too many tries. Get a new code.'), findsOneWidget);
    harness.backend.onSend = null;
    await tester.tap(find.text('Send new code'));
    await tester.pump();
    expect(harness.last.hasAccountResend(), isTrue);
    expect(find.text('New code sent.'), findsOneWidget);
  });

  testWidgets('another email starts over with it', (tester) async {
    final harness = await showAccount(tester, awaiting);
    await tester.tap(find.text('Use a different email'));
    await tester.pump();
    expect(find.text('Sign in to Claktory'), findsOneWidget);
    await tester.enterText(field, 'matt@home.test');
    await tester.pump();
    await tester.tap(find.text('Send code'));
    await tester.pump();
    expect(harness.last.accountSignIn.email, 'matt@home.test');
    expect(find.text('Check your email'), findsOneWidget);
  });

  testWidgets('Back returns to the code already sent', (tester) async {
    final harness = await showAccount(tester, awaiting);
    await tester.tap(find.text('Use a different email'));
    await tester.pump();
    expect(
      tester.widget<EditableText>(field).controller.text,
      'matt@example.com',
    );
    await tester.tap(find.text('Back'));
    await tester.pump();
    expect(find.text('Check your email'), findsOneWidget);
    expect(harness.backend.sent, isEmpty);
  });

  testWidgets('signing in is required: no Not now, nor what the email is '
      'used for, only a link to it; the app behind is out of reach', (
    tester,
  ) async {
    final opened = <Uri>[];
    openAccountPage = (url) async {
      opened.add(url);
      return true;
    };
    addTearDown(() => openAccountPage = (url) async => true);
    final harness = await showAccount(tester, signedOut);
    expect(find.text('Sign in to Claktory'), findsOneWidget);
    expect(find.text('Not now'), findsNothing);
    expect(find.textContaining('Claktory shares your email'), findsNothing);
    await tester.tap(find.text('How Claktory uses your email'));
    await tester.pump();
    expect(opened, [Uri.parse('https://claktory.com/privacy')]);
    // The agent's conversation behind the step can't be clicked.
    await tester.tap(find.text('Builder').first, warnIfMissed: false);
    await tester.pump();
    expect(harness.backend.sent.where((m) => m.hasSelectAgent()), isEmpty);
  });

  testWidgets('on a phone, the app behind the sign-in is inert', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = await showAccount(
      tester,
      signedOut,
      size: const Size(393, 852),
    );
    expect(find.text('Sign in to Claktory'), findsOneWidget);
    expect(find.text('Not now'), findsNothing);
    final semantics = tester.ensureSemantics();
    await tester.pump();
    expect(find.semantics.byLabel(RegExp('Builder')), findsNothing);
    expect(find.semantics.byLabel('Sign in to Claktory'), findsOneWidget);
    semantics.dispose();
    await tester.tap(find.text('Builder').first, warnIfMissed: false);
    await tester.pump();
    expect(harness.backend.sent.where((m) => m.hasSelectAgent()), isEmpty);
  });

  testWidgets('Delete account… opens the deletion step on the account site', (
    tester,
  ) async {
    final opened = <Uri>[];
    openAccountPage = (url) async {
      opened.add(url);
      return true;
    };
    addTearDown(() => openAccountPage = (url) async => true);
    final harness = await showAccount(tester, signedIn);
    await openSettings(tester, harness);
    await reveal(tester, find.text('Delete account…'));
    expect(
      find.text(
        'Deletes your email and everything linked to it at claktory.com, '
        'after asking.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Delete account…'));
    await tester.pump();
    expect(opened, [Uri.parse('https://account.claktory.com/account/delete')]);
  });

  testWidgets('a Claktory an older version let skip is asked again', (
    tester,
  ) async {
    await showAccount(
      tester,
      ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SKIPPED,
    );
    expect(find.text('Sign in to Claktory'), findsOneWidget);
    expect(find.text('Not now'), findsNothing);
  });

  testWidgets('the code step names the sender, with no way around it', (
    tester,
  ) async {
    final snapshot = withAccount(awaiting)
      ..account.codeFrom = 'code@auth.claktory.com';
    final harness = Harness(snapshot);
    await harness.show(tester, size: const Size(1200, 900));
    await pumpFrames(tester);
    expect(
      find.text(
        "It's from code@auth.claktory.com. Can't find it? Check your spam "
        'folder.',
      ),
      findsOneWidget,
    );
    expect(find.text('Not now'), findsNothing);
  });

  testWidgets("when claktory.com can't be reached, Claktory starts without "
      'it until the next launch', (tester) async {
    final harness = await showAccount(tester, signedOut);
    refuseWhen(
      harness,
      (sent) => sent.hasAccountSignIn(),
      ErrorCode.ERROR_CODE_UNREACHABLE,
      "claktory.com can't be reached",
    );
    await tester.enterText(field, 'matt@example.com');
    await tester.pump();
    await tester.tap(find.text('Send code'));
    await tester.pump();
    expect(find.text("Can't reach claktory.com right now."), findsOneWidget);

    await tester.tap(find.text('Try again'));
    await tester.pump();
    expect(harness.backend.sent, hasLength(2));
    expect(harness.last.accountSignIn.email, 'matt@example.com');

    await tester.tap(find.text('Continue for now'));
    await pumpFrames(tester);
    expect(find.text("Can't reach claktory.com right now."), findsNothing);

    await openSettings(tester, harness);
    await reveal(tester, find.text('Not signed in'));
    await tester.tap(find.text('Sign in…'));
    await pumpFrames(tester);
    expect(find.text('Sign in to Claktory'), findsOneWidget);
  });

  testWidgets('a phone pairing with a Claktory signed in sees no step', (
    tester,
  ) async {
    await showAccount(tester, signedIn, size: const Size(390, 844));
    expect(find.text('Sign in to Claktory'), findsNothing);
    expect(find.text('Check your email'), findsNothing);
  });

  testWidgets('Settings → Account shows the account and what was sent', (
    tester,
  ) async {
    Uri? opened;
    openAccountPage = (url) async {
      opened = url;
      return true;
    };
    addTearDown(() => openAccountPage = (url) async => true);
    final harness = await showAccount(tester, signedIn);
    await openSettings(tester, harness);
    await reveal(tester, find.text('matt@example.com'));
    expect(find.text('Mac mini'), findsOneWidget);
    expect(find.text('account.claktory.com'), findsOneWidget);
    expect(find.text('2 h ago'), findsOneWidget);

    await tester.tap(find.text('Show exactly what was sent'));
    await pumpFrames(tester);
    expect(find.text('At sign-in · 2 days ago'), findsOneWidget);
    expect(find.text('Most recent · 5 min ago'), findsOneWidget);
    expect(
      find.text('{\n  "version": "0.1.2",\n  "platform": "macos-arm64"\n}'),
      findsOneWidget,
    );

    await reveal(tester, find.text('Manage account at claktory.com'));
    await tester.tap(find.text('Manage account at claktory.com'));
    await tester.pump();
    expect(opened, Uri.parse('https://account.claktory.com/account'));

    await tester.tap(find.text('Show recovery key…'));
    await pumpFrames(tester);
    expect(harness.last.hasShowRecoveryKey(), isTrue);
    expect(find.text('Recovery key for this Mac'), findsOneWidget);
    expect(find.text('ABCD-EFGH-IJKL-MNOP'), findsOneWidget);
    await tester.tap(find.text('Done').last);
    await pumpFrames(tester);

    await tester.tap(find.text('Sign out…'));
    await pumpFrames(tester);
    expect(find.text('Sign out of Claktory?'), findsOneWidget);
    await tester.tap(find.text('Sign out').last);
    await pumpFrames(tester);
    expect(harness.last.hasAccountSignOut(), isTrue);
  });

  testWidgets('another account service signs out first', (tester) async {
    final harness = await showAccount(tester, signedIn);
    await openSettings(tester, harness);
    await reveal(tester, find.text('Change…'));
    await tester.tap(find.text('Change…'));
    await pumpFrames(tester);
    await tester.enterText(
      find.descendant(
        of: find.byType(Field),
        matching: find.byType(EditableText),
      ),
      'http://127.0.0.1:8080',
    );
    await tester.pump();
    await tester.tap(find.text('Save'));
    await pumpFrames(tester);
    final sent = harness.backend.sent;
    expect(sent[sent.length - 2].hasAccountSignOut(), isTrue);
    expect(sent.last.setAccountService.url, 'http://127.0.0.1:8080');
  });

  testWidgets('a sign-out claktory.com will hear later is said on the '
      'sign-in step that covers Settings', (tester) async {
    final snapshot = withAccount(signedOut);
    snapshot.account.problem =
        'Signed out. Claktory tells claktory.com as soon as it can reach it.';
    final harness = Harness(snapshot);
    await harness.show(tester, size: const Size(1200, 900));
    await pumpFrames(tester);
    expect(find.text('Sign in to Claktory'), findsOneWidget);
    expect(
      find.text(
        'Signed out. Claktory tells claktory.com as soon as it can reach it.',
      ),
      findsOneWidget,
    );
    // Told: the line goes with the problem.
    final told = harness.backend.workspace.workspace.deepCopy()
      ..account.problem = '';
    await harness.apply(tester, told);
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('sign-in-notice')), findsNothing);
  });

  testWidgets('a Claktory removed at claktory.com says so and signs in '
      'again', (tester) async {
    final harness = await showAccount(tester, revoked);
    expect(find.text('Sign in to Claktory'), findsNothing);
    await openSettings(tester, harness);
    final line = find.textContaining('was removed from your account');
    await reveal(tester, line);
    await tester.tap(find.text('Sign in again'));
    await tester.pump();
    expect(harness.last.accountSignIn.email, 'matt@example.com');
  });

  testWidgets('the steps fit a 375-point phone', (tester) async {
    final harness = await showAccount(
      tester,
      signedOut,
      size: const Size(375, 667),
    );
    expect(find.text('Sign in to Claktory'), findsOneWidget);
    await become(tester, harness, awaiting);
    expect(find.text('Check your email'), findsOneWidget);
    refuseWhen(
      harness,
      (sent) => sent.hasAccountVerify(),
      ErrorCode.ERROR_CODE_UNREACHABLE,
      "claktory.com can't be reached",
    );
    await tester.enterText(field, '123456');
    await tester.pump();
    expect(find.text("Can't reach claktory.com right now."), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Settings → Account fits a 375-point phone, what was sent '
      'open', (tester) async {
    final harness = await showAccount(
      tester,
      signedIn,
      size: const Size(375, 667),
    );
    await openSettings(tester, harness);
    // A phone lists the sections: Account opens as a page of its own.
    harness.backend.settingsSection.value = 'account';
    await pumpFrames(tester);
    final more = find.text('Show exactly what was sent');
    await reveal(tester, more);
    await tester.ensureVisible(more);
    await pumpFrames(tester);
    await tester.tap(more);
    await pumpFrames(tester);
    await reveal(tester, find.textContaining('Most recent · '));
    await reveal(tester, find.text('Sign out…'));
    expect(tester.takeException(), isNull);
  });

  // Zucchero's row 126: at AX XXXL on an iPhone SE the sheet overflowed by
  // 2487 px and couldn't scroll, so the key couldn't be seen.
  testWidgets('on an iPhone SE at the largest text, the recovery key sheet '
      'scrolls to the key and Done', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 53 / 17;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final harness = await showAccount(
      tester,
      signedIn,
      size: const Size(320, 568),
    );
    await openSettings(tester, harness);
    await reveal(tester, find.text('Account'));
    await tester.tap(find.text('Account'));
    await pumpFrames(tester);
    await reveal(tester, find.text('Show recovery key…'));
    await tester.tap(find.text('Show recovery key…'));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    expect(find.text('Recovery key for this Mac'), findsOneWidget);

    final sheet = find
        .ancestor(
          of: find.text('Recovery key for this Mac'),
          matching: find.byType(Scrollable),
        )
        .first;
    for (final target in [
      find.text('ABCD-EFGH-IJKL-MNOP'),
      find.text('Done'),
    ]) {
      await tester.scrollUntilVisible(target, 200, scrollable: sheet);
      await pumpFrames(tester);
      final rect = tester.getRect(target);
      expect(rect.top, greaterThanOrEqualTo(0));
      expect(rect.bottom, lessThanOrEqualTo(568));
    }
    // The key wraps at its blocks, all of it in sight (Zucchero's row 131),
    // rather than scrolling sideways.
    final key = find.text('ABCD-EFGH-IJKL-MNOP');
    await tester.scrollUntilVisible(key, 200, scrollable: sheet);
    await pumpFrames(tester);
    expect(tester.getRect(key).right, lessThanOrEqualTo(320));
    expect(tester.getRect(key).height, greaterThan(80), reason: 'wrapped');
    await tester.scrollUntilVisible(find.text('Done'), 200, scrollable: sheet);
    await pumpFrames(tester);
    await tester.tap(find.text('Done'));
    await pumpFrames(tester);
    expect(find.text('Recovery key for this Mac'), findsNothing);
    expect(tester.takeException(), isNull);
  });
}
