// The first run on a phone (#198, phase 2A): signing in, the name and the
// other steps are pages of their own, not a card over the window; the email
// brings the email keyboard and the code one numeric field that takes the
// code Mail or Messages offers; the action is the page's width, first under
// what it acts on; and nothing overflows at the largest text.
import 'package:factory_app/account.dart';
import 'package:factory_app/controls.dart' show Button, Pressable;
import 'package:factory_app/device_account.dart';
import 'package:factory_app/name_step.dart';
import 'package:factory_app/phone.dart' show PairScreen;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/your_claktories.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'account_test.dart'
    show awaiting, become, field, openSettings, refuseWhen, signedOut;
import 'account_test.dart' as account_test show withAccount;
import 'device_confirm_test.dart' show FakeAccountApi, home;
import 'harness.dart';
import 'pair_start.dart';

const _phone = Size(393, 852);
const _small = Size(320, 568);

/// The errors Flutter reports from here on (overflows, say), where each is.
List<String> _errors() {
  final errors = <String>[];
  final old = FlutterError.onError;
  FlutterError.onError = (details) {
    errors.add(
      details
          .toString()
          .split('\n')
          .where((line) => line.contains('.dart:'))
          .take(2)
          .join(' '),
    );
    old?.call(details);
  };
  addTearDown(() => FlutterError.onError = old);
  return errors;
}

void _largestText(WidgetTester tester) {
  tester.platformDispatcher.textScaleFactorTestValue = 3.1;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
}

void _iPhone() {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
}

Future<Harness> _show(
  WidgetTester tester,
  ClaktoryAccountState state, {
  bool named = true,
  Size size = _phone,
}) async {
  final harness = Harness(account_test.withAccount(state), named: named);
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  return harness;
}

EditableText _editable(WidgetTester tester, Finder of) =>
    tester.widget<EditableText>(
      find.descendant(of: of, matching: find.byType(EditableText)),
    );

/// The button whose words are [label].
Finder _button(String label) =>
    find.ancestor(of: find.text(label), matching: find.byType(Button)).first;

void main() {
  testWidgets('on a phone, signing in is a page: the email keyboard, then '
      'one numeric code field, the action the page\'s width', (tester) async {
    _iPhone();
    final harness = await _show(tester, signedOut);
    expect(find.text('Sign in to Claktory'), findsOneWidget);
    // A page: the step fills the screen, no card on a scrim.
    expect(tester.getSize(find.byType(StepCard)), _phone);
    expect(
      tester.widget<Text>(find.text('Sign in to Claktory')).style?.fontSize,
      22,
    );
    final email = _editable(tester, find.byType(SignInStep));
    expect(email.keyboardType, TextInputType.emailAddress);
    expect(email.autofillHints, contains(AutofillHints.email));
    expect(email.autocorrect, isFalse);
    expect(email.textCapitalization, TextCapitalization.none);
    // Send code, the page's width between its 16-point gutters.
    expect(tester.getSize(_button('Send code')).width, _phone.width - 32);
    expect(
      tester.getSize(_button('Send code')).height,
      greaterThanOrEqualTo(44),
    );
    expect(
      tester.getSize(find.text('How Claktory uses your email')).height,
      lessThan(44),
    );
    expect(
      tester
          .getSize(
            find
                .ancestor(
                  of: find.text('How Claktory uses your email'),
                  matching: find.byType(ConstrainedBox),
                )
                .first,
          )
          .height,
      greaterThanOrEqualTo(44),
    );

    await become(tester, harness, awaiting);
    expect(find.text('Check your email'), findsOneWidget);
    // One field for the whole code.
    expect(
      find.descendant(
        of: find.byType(SignInStep),
        matching: find.byType(EditableText),
      ),
      findsOneWidget,
    );
    final code = _editable(tester, find.byType(SignInStep));
    expect(code.keyboardType, TextInputType.number);
    expect(code.autofillHints, contains(AutofillHints.oneTimeCode));
    // Continue first, then the ways around it, each the page's width.
    final buttons = [
      'Continue',
      'Resend in',
      'Use a different email',
    ].map((label) => find.textContaining(label).first).toList();
    for (var i = 1; i < buttons.length; i++) {
      expect(
        tester.getTopLeft(buttons[i]).dy,
        greaterThan(tester.getTopLeft(buttons[i - 1]).dy),
      );
    }
    expect(tester.getSize(_button('Continue')).width, _phone.width - 32);
    expect(
      tester.getSize(_button('Use a different email')).width,
      _phone.width - 32,
    );

    // A pasted code with a space in it goes in as its digits, and sends.
    await tester.enterText(field, '123 456');
    await pumpFrames(tester);
    expect(harness.last.accountVerify.code, '123456');
  });

  testWidgets('the keyboard never covers the email or Send code', (
    tester,
  ) async {
    _iPhone();
    await _show(tester, signedOut, size: _small);
    tester.view.viewInsets = const FakeViewPadding(bottom: 260);
    addTearDown(tester.view.resetViewInsets);
    await pumpFrames(tester);
    await tester.showKeyboard(field);
    await pumpFrames(tester);
    final above = _small.height - 260;
    expect(tester.getBottomLeft(field).dy, lessThanOrEqualTo(above));
    expect(
      tester.getBottomLeft(_button('Send code')).dy,
      lessThanOrEqualTo(above),
    );
  });

  testWidgets('the name step is a page too: a name keyboard, the guess a '
      'finger tall, Continue the page\'s width', (tester) async {
    _iPhone();
    // Signed in as matt@example.com, with no name yet.
    await _show(
      tester,
      ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN,
      named: false,
    );
    expect(find.text("What's your name?"), findsOneWidget);
    expect(tester.getSize(find.byType(StepCard)), _phone);
    final name = _editable(tester, find.byType(NameStep));
    expect(name.keyboardType, TextInputType.name);
    expect(name.textCapitalization, TextCapitalization.words);
    expect(name.autofillHints, contains(AutofillHints.givenName));
    expect(tester.getSize(_button('Continue')).width, _phone.width - 32);
    final guess = find
        .ancestor(of: find.text('Matt?'), matching: find.byType(Pressable))
        .first;
    expect(tester.getSize(guess).height, greaterThanOrEqualTo(44));
    await tester.tap(guess);
    await pumpFrames(tester);
    expect(name.controller.text, 'Matt');
  });

  for (final size in [_small, _phone]) {
    testWidgets('at the largest text every step fits '
        '${size.width.toInt()} points', (tester) async {
      _iPhone();
      _largestText(tester);
      final errors = _errors();
      final harness = await _show(tester, signedOut, size: size);
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
      await pumpFrames(tester);
      expect(find.text("Can't reach claktory.com right now."), findsOneWidget);
      tester.takeException();
      expect(errors, isEmpty);
    });

    testWidgets('at the largest text the name step fits '
        '${size.width.toInt()} points', (tester) async {
      _iPhone();
      _largestText(tester);
      final errors = _errors();
      final named = await _show(
        tester,
        ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN,
        named: false,
        size: size,
      );
      expect(find.text("What's your name?"), findsOneWidget);
      named.backend.refuse = 'The settings file has an error.';
      await tester.enterText(
        find.descendant(
          of: find.byType(NameStep),
          matching: find.byType(EditableText),
        ),
        'Matt',
      );
      await pumpFrames(tester);
      await tester.ensureVisible(find.text('Continue'));
      await pumpFrames(tester);
      await tester.tap(find.text('Continue'));
      await pumpFrames(tester);
      expect(find.text('The settings file has an error.'), findsOneWidget);
      tester.takeException();
      expect(errors, isEmpty);
    });
  }

  testWidgets('Settings → Account fits 320 points at the largest text, '
      'signed out', (tester) async {
    _iPhone();
    _largestText(tester);
    final errors = _errors();
    final harness = await _show(tester, signedOut, size: _small);
    signInPutOff(harness.backend).value = true;
    await openSettings(tester, harness);
    harness.backend.settingsSection.value = 'account';
    await pumpFrames(tester);
    final list = find.byType(Scrollable).last;
    for (var i = 0; i < 8; i++) {
      await tester.drag(list, const Offset(0, -400), warnIfMissed: false);
      await pumpFrames(tester);
    }
    tester.takeException();
    expect(errors, isEmpty);
    // Sign in… under the words, at their start.
    expect(
      tester.getTopLeft(find.text('Sign in…')).dy,
      greaterThan(tester.getBottomLeft(find.text('Not signed in')).dy),
    );
  });

  group('this phone signing in to its account', () {
    Future<Harness> pairing(WidgetTester tester, {Size size = _phone}) async {
      _iPhone();
      final api = FakeAccountApi()
        ..claktoryList = [
          AccountClaktory(
            id: home.id,
            name: home.name,
            color: home.color,
            endpointId: home.endpointId,
            relays: home.relays,
            lastContact: DateTime.now(),
          ),
          const AccountClaktory(id: 'c2', name: 'Server', machine: 'nas'),
        ];
      final account = DeviceAccount(key: 'k' * 64, open: (_, _) => api);
      final harness = Harness(
        Workspace(),
        localBackend: false,
        account: account,
      );
      await harness.show(tester, size: size);
      await pumpFrames(tester);
      expect(find.byType(PairScreen), findsOneWidget);
      // The first screen's own way in (#363).
      await pastPairStart(tester, email: true);
      await pumpFrames(tester);
      return harness;
    }

    testWidgets('the email keyboard, one numeric code field, the actions '
        'the screen\'s width', (tester) async {
      await pairing(tester);
      final email = _editable(tester, find.byType(DeviceSignInForm));
      expect(email.keyboardType, TextInputType.emailAddress);
      expect(email.autofillHints, contains(AutofillHints.email));
      final width = tester.getSize(find.byType(DeviceSignInForm)).width;
      expect(tester.getSize(_button('Send code')).width, width);
      await tester.enterText(
        find.byKey(const ValueKey('device-email')),
        'matt@example.com',
      );
      await pumpFrames(tester);
      await tester.tap(find.text('Send code'));
      await pumpFrames(tester);
      final code = _editable(tester, find.byType(DeviceSignInForm));
      expect(code.keyboardType, TextInputType.number);
      expect(code.autofillHints, contains(AutofillHints.oneTimeCode));
      expect(tester.getSize(_button('Continue')).width, width);
      expect(
        tester.getTopLeft(find.text('Use a different email')).dy,
        greaterThan(tester.getTopLeft(find.text('Continue')).dy),
      );
    });

    for (final size in [_small, _phone]) {
      testWidgets('it fits ${size.width.toInt()} points at the largest '
          'text, through to connecting', (tester) async {
        _largestText(tester);
        final errors = _errors();
        await pairing(tester, size: size);
        await tester.enterText(
          find.byKey(const ValueKey('device-email')),
          'matt@example.com',
        );
        await pumpFrames(tester);
        await tester.ensureVisible(find.text('Send code'));
        await pumpFrames(tester);
        await tester.tap(find.text('Send code'));
        await pumpFrames(tester);
        await tester.enterText(
          find.byKey(const ValueKey('device-code')),
          '123456',
        );
        await pumpFrames(tester);
        expect(find.text('Home'), findsOneWidget);
        await tester.ensureVisible(find.text('Home'));
        await pumpFrames(tester);
        await tester.tap(find.text('Home'));
        await pumpFrames(tester);
        expect(find.text('Connect to Home?'), findsOneWidget);
        tester.takeException();
        expect(errors, isEmpty);
      });
    }
  });
}
