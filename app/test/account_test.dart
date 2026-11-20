import 'package:factory_app/account.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/controls.dart' show Field;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:fixnum/fixnum.dart';
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
    expect(find.text('At sign-in'), findsOneWidget);
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
    expect(find.text('Your recovery key'), findsOneWidget);
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
}
