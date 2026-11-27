import 'package:factory_app/account.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart'
    show RemoteFactory, parsePairingCode;
import 'package:factory_app/device_account.dart'
    show AccountClaktory, DeviceAccount;
import 'package:factory_app/devices.dart' show thisDeviceKind;
import 'package:factory_app/name_step.dart' show nameFromEmail;
import 'package:factory_app/controls.dart' show ExtensionGlyph, Field;
import 'package:factory_app/device_kind.dart' show thisDeviceKindForTests;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_shell.dart' show SettingAnchor;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'device_confirm_test.dart' as confirm show FakeAccountApi, signedIn;
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

/// Settings at the section holding [target], scrolled to it.
Future<void> reveal(WidgetTester tester, Finder target, {String? section}) =>
    revealInSettings(tester, target, section: section);

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
    // This Mac's own: nothing disconnects.
    expect(find.textContaining('let it in through your account'), findsNothing);
    await tester.tap(find.text('Sign out').last);
    await pumpFrames(tester);
    expect(harness.last.hasAccountSignOut(), isTrue);
  });

  testWidgets('signing out a Claktory this device reached through the '
      'account says it disconnects, and the ways back (#529, V6)', (
    tester,
  ) async {
    final harness = await showAccount(tester, signedIn);
    harness.connections.use(
      RemoteFactory.fromAccount(
        AccountClaktory(id: 's', name: 'Studio', endpointId: 'e' * 64),
      ),
    );
    // Connected to it: its own Workspace, its account signed in.
    harness.backend.workspace.apply(withAccount(signedIn));
    await pumpFrames(tester);
    await openSettings(tester, harness);
    await reveal(tester, find.text('Sign out…'));
    await tester.tap(find.text('Sign out…'));
    await pumpFrames(tester);
    expect(
      find.textContaining(
        'disconnects from Studio, which let it in through your account. To '
        'connect again, add this $thisDeviceKind on Studio in Settings → '
        'Devices, or run factoryd pair in a terminal there.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('Switch account… keeps this account until the new one\'s '
      'code checks out, then closes (#529, V6)', (tester) async {
    final harness = await showAccount(tester, signedIn);
    await openSettings(tester, harness);
    await reveal(tester, find.text('Switch account…'));
    await tester.tap(find.text('Switch account…'));
    await pumpFrames(tester);
    final form = find.byType(SwitchAccountForm);
    final input = find.descendant(
      of: form,
      matching: find.byType(EditableText),
    );
    expect(find.text('Switch account'), findsOneWidget);
    expect(
      find.textContaining(
        "stays signed in as matt@example.com until you enter the new "
        "account's code.",
      ),
      findsOneWidget,
    );
    // Its own Mac: nothing to warn about.
    expect(find.textContaining('stops letting this'), findsNothing);

    await tester.enterText(input, ' new@example.com ');
    await tester.pump();
    await tester.tap(find.text('Send code'));
    await pumpFrames(tester);
    expect(harness.last.accountSignIn.email, 'new@example.com');

    // Still signed in as before, the code pending.
    final pending = harness.backend.workspace.workspace.deepCopy();
    pending.account
      ..switchingTo = 'new@example.com'
      ..codeFrom = 'hello@claktory.com'
      ..codeExpiresMs = Int64(
        DateTime.now().add(const Duration(minutes: 15)).millisecondsSinceEpoch,
      );
    await harness.apply(tester, pending);
    await pumpFrames(tester);
    expect(find.text('Check your email'), findsOneWidget);
    expect(
      find.textContaining('Enter the 6-digit code sent to new@example.com.'),
      findsOneWidget,
    );
    expect(
      find.text(
        "It's from hello@claktory.com. Can't find it? Check your spam "
        'folder.',
      ),
      findsOneWidget,
    );

    await tester.enterText(input, '123 456');
    await tester.pump();
    expect(harness.last.accountVerify.code, '123456');

    final switched = harness.backend.workspace.workspace.deepCopy();
    switched.account
      ..email = 'new@example.com'
      ..clearSwitchingTo()
      ..clearCodeFrom()
      ..clearCodeExpiresMs();
    await harness.apply(tester, switched);
    await pumpFrames(tester);
    expect(form, findsNothing);
    expect(find.text('new@example.com'), findsOneWidget);
    expect(find.text('Switch account…'), findsOneWidget);
  });

  testWidgets('the switch code boxes take the focus each time the code '
      'step shows, and again after Resend code (V6, Abbacchio)', (
    tester,
  ) async {
    final harness = await showAccount(tester, signedIn);
    await openSettings(tester, harness);
    await reveal(tester, find.text('Switch account…'));
    await tester.tap(find.text('Switch account…'));
    await pumpFrames(tester);
    final form = find.byType(SwitchAccountForm);
    final input = find.descendant(
      of: form,
      matching: find.byType(EditableText),
    );
    bool boxesFocused() => tester
        .widget<EditableText>(
          find.descendant(
            of: find.byKey(const ValueKey('switch-code')),
            matching: find.byType(EditableText),
          ),
        )
        .focusNode
        .hasFocus;
    Future<void> codeSent(String to, {bool resendable = false}) async {
      final pending = harness.backend.workspace.workspace.deepCopy();
      pending.account
        ..switchingTo = to
        ..codeExpiresMs = Int64(
          DateTime.now()
              .add(Duration(minutes: resendable ? 1 : 15))
              .millisecondsSinceEpoch,
        );
      await harness.apply(tester, pending);
      await pumpFrames(tester);
    }

    await tester.enterText(input, 'new@example.com');
    await tester.pump();
    await tester.tap(find.text('Send code'));
    await pumpFrames(tester);
    await codeSent('new@example.com');
    expect(boxesFocused(), isTrue);

    // Another address, then its code: the boxes again, focused.
    await tester.tap(find.text('Use a different email'));
    await pumpFrames(tester);
    await tester.enterText(input, 'other@example.com');
    await tester.pump();
    await tester.tap(find.text('Send code'));
    await pumpFrames(tester);
    await codeSent('other@example.com', resendable: true);
    expect(boxesFocused(), isTrue);

    // Away from the boxes, then Resend code: back in them.
    FocusManager.instance.primaryFocus?.unfocus();
    await tester.pump();
    expect(boxesFocused(), isFalse);
    await tester.tap(find.text('Resend code'));
    await pumpFrames(tester);
    expect(boxesFocused(), isTrue);
  });

  testWidgets('a switch waiting for its code says so; Cancel drops it '
      '(#529, V6)', (tester) async {
    final snapshot = withAccount(signedIn);
    snapshot.account
      ..switchingTo = 'new@example.com'
      ..codeExpiresMs = Int64(
        DateTime.now().add(const Duration(minutes: 15)).millisecondsSinceEpoch,
      );
    final harness = Harness(snapshot);
    await harness.show(tester, size: const Size(1200, 900));
    await pumpFrames(tester);
    await openSettings(tester, harness);
    await reveal(tester, find.text('Enter code…'));
    expect(
      find.text(
        'Switching to new@example.com: enter the code sent there to finish.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Enter code…'));
    await pumpFrames(tester);
    expect(find.text('Check your email'), findsOneWidget);
    // Cancel drops the code asked for; the account stays.
    await tester.tap(find.text('Cancel').last);
    await pumpFrames(tester);
    expect(harness.last.hasAccountSignInCancel(), isTrue);
    expect(find.byType(SwitchAccountForm), findsNothing);
  });

  testWidgets('switching a Claktory this device reached through the '
      'account warns it may stop letting it in (#529, V6)', (tester) async {
    final harness = await showAccount(tester, signedIn);
    harness.connections.use(
      RemoteFactory.fromAccount(
        AccountClaktory(id: 's', name: 'Studio', endpointId: 'e' * 64),
      ),
    );
    harness.backend.workspace.apply(withAccount(signedIn));
    await pumpFrames(tester);
    await openSettings(tester, harness);
    await reveal(tester, find.text('Switch account…'));
    await tester.tap(find.text('Switch account…'));
    await pumpFrames(tester);
    expect(
      find.text(
        "If the new account isn't the one this $thisDeviceKind uses, Studio "
        'stops letting this $thisDeviceKind in. To connect again, add this '
        '$thisDeviceKind on Studio in Settings → Devices, or run factoryd '
        'pair in a terminal there.',
      ),
      findsOneWidget,
    );
    // Nothing asked yet: Cancel only closes.
    await tester.tap(find.text('Cancel').last);
    await pumpFrames(tester);
    expect(
      harness.backend.sent.where((m) => m.hasAccountSignInCancel()),
      isEmpty,
    );
  });

  testWidgets("on a device without its own Claktory, Settings → Account "
      "says whether this device is signed in, with Sign in… or Sign out…, "
      "first in This Mac, never in Account (area 2, 8.5; Lucy 49738)", (
    tester,
  ) async {
    Future<Harness> on(DeviceAccount account, {bool local = false}) async {
      // A fresh tree: the app before keeps none of its state.
      await tester.pumpWidget(const SizedBox());
      final harness = Harness(
        withAccount(signedIn),
        localBackend: local,
        account: account,
      );
      harness.connections.use(
        parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=Studio'),
      );
      await harness.show(tester, size: const Size(1200, 900));
      await pumpFrames(tester);
      harness.backend.workspace.apply(withAccount(signedIn));
      await pumpFrames(tester);
      await openSettings(tester, harness);
      return harness;
    }

    // Signed in: who as, and a quiet Sign out….
    await on(confirm.signedIn(confirm.FakeAccountApi()));
    final line = find.text('Signed in as matt@example.com.');
    await reveal(tester, line);
    expect(line, findsOneWidget);
    // This device's own section, not the Claktory's Account.
    expect(
      find.ancestor(
        of: find.byType(DeviceAccountRow),
        matching: find.byWidgetPredicate(
          (widget) => widget is SettingAnchor && widget.id == 'device-account',
        ),
      ),
      findsOneWidget,
    );
    expect(
      find.ancestor(
        of: find.byType(DeviceAccountRow),
        matching: find.byType(AccountSettings),
      ),
      findsNothing,
    );
    expect(
      find.descendant(
        of: find.byType(DeviceAccountRow),
        matching: find.text('Sign out…'),
      ),
      findsOneWidget,
    );

    // Signed out: Sign in…, in a dialog of its own.
    await on(DeviceAccount());
    final out = find.text('Not signed in.');
    await reveal(tester, out);
    expect(out, findsOneWidget);
    await tester.tap(
      find.descendant(
        of: find.byType(DeviceAccountRow),
        matching: find.text('Sign in…'),
      ),
    );
    await pumpFrames(tester);
    expect(find.text('Sign in on this $thisDeviceKind'), findsOneWidget);
    expect(find.byKey(const ValueKey('device-email')), findsOneWidget);
  });

  testWidgets("This Mac's section glyph is a monitor on a desktop Mac and a "
      "laptop on a laptop, as its tile in the rail (8.8)", (tester) async {
    addTearDown(() => thisDeviceKindForTests = null);
    // A Mac's Settings: tests run as Android otherwise.
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    Future<String?> glyphOn(DeviceKind kind) async {
      await tester.pumpWidget(const SizedBox());
      thisDeviceKindForTests = kind;
      final harness = Harness(withAccount(signedIn), localBackend: true);
      // Tall enough that the section list shows This Mac at its end.
      await harness.show(tester, size: const Size(1200, 3000));
      await pumpFrames(tester);
      await openSettings(tester, harness);
      await pumpFrames(tester);
      final glyph = find.descendant(
        of: find.byWidgetPredicate(
          (widget) => widget.runtimeType.toString() == '_ThisMacGlyph',
        ),
        matching: find.byType(ExtensionGlyph),
      );
      return tester.widgetList<ExtensionGlyph>(glyph).first.icon;
    }

    expect(await glyphOn(DeviceKind.DEVICE_KIND_DESKTOP), 'monitor');
    expect(await glyphOn(DeviceKind.DEVICE_KIND_LAPTOP), 'laptop-minimal');
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets("a Mac with its own Claktory says nothing of this device's "
      'sign-in: it signs in with its Claktory (area 2, §6)', (tester) async {
    final harness = await showAccount(tester, signedIn);
    await openSettings(tester, harness);
    await reveal(tester, find.text('Sign out…'));
    expect(find.byType(DeviceAccountRow), findsNothing);
  });

  testWidgets('a self-hosted account service is named as itself, never '
      "claktory.com (Zucchero's V6 NIT)", (tester) async {
    final snapshot = withAccount(signedIn);
    snapshot.account.service = 'https://accounts.example.test';
    final harness = Harness(snapshot);
    await harness.show(tester, size: const Size(1200, 900));
    await pumpFrames(tester);
    await openSettings(tester, harness);
    await reveal(tester, find.text('Manage account at accounts.example.test'));
    expect(find.text('Shared with accounts.example.test'), findsOneWidget);
    expect(find.textContaining('claktory.com'), findsNothing);
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

  testWidgets('on an iPhone SE at the largest text, the switch code sheet '
      'fits, Resend and the other email under the code (#529, V6)', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 53 / 17;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final snapshot = withAccount(signedIn);
    snapshot.account
      ..switchingTo = 'new@example.com'
      ..codeExpiresMs = Int64(
        DateTime.now().add(const Duration(minutes: 10)).millisecondsSinceEpoch,
      );
    final harness = Harness(snapshot);
    await harness.show(tester, size: const Size(320, 568));
    await pumpFrames(tester);
    await openSettings(tester, harness);
    // Account's page, as its row in Settings' list opens it.
    harness.backend.settingsSection.value = 'account';
    await pumpFrames(tester);
    await reveal(tester, find.text('Enter code…'));
    await tester.tap(find.text('Enter code…'));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    expect(find.text('Check your email'), findsOneWidget);
    final sheet = find
        .ancestor(
          of: find.byKey(const ValueKey('switch-code')),
          matching: find.byType(Scrollable),
        )
        .first;
    for (final target in [
      find.text('Resend code'),
      find.text('Use a different email'),
    ]) {
      await tester.scrollUntilVisible(target, 200, scrollable: sheet);
      await pumpFrames(tester);
      final rect = tester.getRect(target);
      expect(rect.top, greaterThanOrEqualTo(0));
      expect(rect.bottom, lessThanOrEqualTo(568));
    }
    await tester.tap(find.text('Resend code'));
    await pumpFrames(tester);
    expect(harness.last.hasAccountResend(), isTrue);
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
    // Account's page, as its row or a link opens it.
    harness.backend.settingsSection.value = 'account';
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
