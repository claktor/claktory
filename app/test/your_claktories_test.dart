import 'package:factory_app/account.dart' show openAccountPage, privacyPage;
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/device_account.dart';
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/phone.dart' show PairScreen;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart' show SettingsView;
import 'package:factory_app/your_claktories.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'device_confirm_test.dart' show FakeAccountApi, home, signIn;
import 'harness.dart';

Future<bool> launchUrlForTests(Uri url) async => true;

void main() {
  Future<(Harness, FakeAccountApi)> phone(
    WidgetTester tester, {
    bool signedIn = false,
  }) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
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
    if (signedIn) await signIn(account);
    final harness = Harness(Workspace(), localBackend: false, account: account);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    return (harness, api);
  }

  testWidgets('a phone signs in with its email, sees its Claktories and '
      'connects to one after confirming', (tester) async {
    final (harness, _) = await phone(tester);
    expect(find.byType(PairScreen), findsOneWidget);
    await tester.tap(find.text('Sign in with your email instead'));
    await pumpFrames(tester);
    await tester.enterText(
      find.byKey(const ValueKey('device-email')),
      'matt@example.com',
    );
    await pumpFrames(tester);
    await tester.tap(find.text('Send code'));
    await pumpFrames(tester);
    expect(
      find.text(
        'Enter the 6-digit code sent to matt@example.com. It works for 15 '
        'minutes.',
      ),
      findsOneWidget,
    );
    expect(
      find.text(
        "It's from code@auth.claktory.com. Can't find it? Check your spam "
        'folder.',
      ),
      findsOneWidget,
    );
    // A wrong code says how many tries are left.
    await tester.enterText(find.byKey(const ValueKey('device-code')), '000000');
    await pumpFrames(tester);
    expect(find.text("That code isn't right. 4 tries left."), findsOneWidget);
    await tester.enterText(find.byKey(const ValueKey('device-code')), '123456');
    await pumpFrames(tester);

    expect(find.text('Home'), findsOneWidget);
    expect(find.text('Online'), findsOneWidget);
    // Not registered with the tunnel yet: it can't be picked.
    expect(find.text("nas · Can't be reached yet"), findsOneWidget);
    expect(find.text('Signed in as matt@example.com'), findsOneWidget);
    await tester.tap(find.text('Server'));
    await pumpFrames(tester);
    expect(find.text('Connect to Server?'), findsNothing);

    await tester.tap(find.text('Home'));
    await pumpFrames(tester);
    expect(find.text('Connect to Home?'), findsOneWidget);
    expect(harness.connections.current, isNull);
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    final current = harness.connections.current!;
    expect(current.fromAccount, isTrue);
    expect(current.accountId, 'c1');
    expect(current.endpointId, 'a' * 64);
    // The account is kept with the connections.
    expect(harness.connectionStorage.kept.account, {
      'key': 'k' * 64,
      'email': 'matt@example.com',
    });
  });

  testWidgets('a phone already signed in starts at its Claktories, and '
      'signs out', (tester) async {
    await phone(tester, signedIn: true);
    expect(find.text('Home'), findsOneWidget);
    expect(find.text('Use a code instead'), findsOneWidget);
    await tester.tap(find.text('Sign out'));
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('device-email')), findsOneWidget);
  });

  testWidgets('App Store: a phone signed in with no Claktory yet can delete '
      'its account, and the email step links the privacy policy', (
    tester,
  ) async {
    final opened = <Uri>[];
    openAccountPage = (url) async {
      opened.add(url);
      return true;
    };
    addTearDown(() => openAccountPage = launchUrlForTests);
    await phone(tester, signedIn: true);
    // Last and apart, under Sign out, a finger's height.
    final delete = find.text('Delete account…');
    await tester.ensureVisible(delete);
    expect(
      tester
          .getSize(find.ancestor(of: delete, matching: find.byType(Button)))
          .height,
      greaterThanOrEqualTo(44),
    );
    expect(
      tester.getTopLeft(delete).dy,
      greaterThan(tester.getTopLeft(find.text('Sign out')).dy),
    );
    expect(
      find.text(
        'Deletes your email and everything linked to it at claktory.com, '
        'after asking.',
      ),
      findsOneWidget,
    );
    await tester.tap(delete);
    await pumpFrames(tester);
    expect(opened, [Uri.parse('https://account.claktory.com/account/delete')]);

    await tester.tap(find.text('Sign out'));
    await pumpFrames(tester);
    expect(find.text('Delete account…'), findsNothing);
    await tester.tap(find.text('How Claktory uses your email'));
    await pumpFrames(tester);
    expect(opened.last, privacyPage);
  });

  testWidgets('a Claktory that wants this device signed in says so, and '
      'connects again once it is', (tester) async {
    final (harness, _) = await phone(tester);
    harness.connections.use(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=Home'),
    );
    harness.backend.signInRequired.value =
        'Confirm this device is yours: sign in.';
    await pumpFrames(tester);
    expect(find.text('Sign in to connect to Home'), findsOneWidget);
    expect(find.text('Confirm this device is yours: sign in.'), findsOneWidget);
    // A paired one isn't given up on from here.
    expect(find.text('Cancel'), findsNothing);
    await tester.enterText(
      find.byKey(const ValueKey('device-email')),
      'matt@example.com',
    );
    await pumpFrames(tester);
    await tester.tap(find.text('Send code'));
    await pumpFrames(tester);
    await tester.enterText(find.byKey(const ValueKey('device-code')), '123456');
    await pumpFrames(tester);
    expect(harness.backend.signInRequired.value, isNull);
  });

  testWidgets('with a week to confirm, a banner asks this device to sign in', (
    tester,
  ) async {
    final (harness, _) = await phone(tester);
    harness.connections.use(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=Home'),
    );
    harness.backend.confirmBy.value = DateTime.now().add(
      const Duration(days: 30),
    );
    await pumpFrames(tester);
    expect(
      find.textContaining('Confirm this iPhone is yours by '),
      findsOneWidget,
    );
    await tester.tap(
      find.descendant(
        of: find.byType(ConfirmDeviceBanner),
        matching: find.text('Sign in…'),
      ),
    );
    await pumpFrames(tester);
    expect(find.text('Confirm this iPhone is yours'), findsOneWidget);
  });

  testWidgets('on a phone a device waiting to be allowed shows along the top, '
      'under the status bar, its buttons under the words', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.view.padding = const FakeViewPadding(top: 47, bottom: 34);
    final harness = Harness(
      Workspace(
        agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
        selectedAgentId: 'rivet',
        remote: RemoteAccess(
          on: true,
          pendingDevices: [
            PendingDevice(
              deviceId: 'd4',
              name: "Matt's iPad",
              number: '0427',
              signedIn: true,
              discovered: true,
            ),
          ],
        ),
      ),
    );
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    final words = find.text("Matt's iPad, signed in as you, wants to connect.");
    expect(words, findsOneWidget);
    expect(tester.getTopLeft(words).dy, greaterThanOrEqualTo(47));
    expect(
      tester.getTopLeft(find.text('Allow')).dy,
      greaterThan(tester.getBottomLeft(words).dy),
    );
    // The conversation starts under the banner, not under the status bar.
    final rivet = find.text('Rivet').first;
    expect(
      tester.getTopLeft(rivet).dy,
      greaterThan(tester.getBottomLeft(find.text('Allow')).dy),
    );

    await tester.tap(find.text('Allow'));
    await pumpFrames(tester);
    expect(harness.last.approveDevice.deviceId, 'd4');
    harness.backend.workspace.apply(
      harness.backend.workspace.workspace.deepCopy()
        ..remote.pendingDevices.clear(),
    );
    await pumpFrames(tester);
    expect(words, findsNothing);
    expect(tester.getTopLeft(find.text('Rivet').first).dy, greaterThan(47));
  });

  group('Settings → Devices', () {
    final now = DateTime.now();
    Workspace devices() => Workspace(
      agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
      selectedAgentId: 'rivet',
      remote: RemoteAccess(
        on: true,
        devices: [
          PairedDevice(
            id: 'd1',
            name: "Matt's iPhone",
            pairedMs: Int64(now.millisecondsSinceEpoch),
            confirmByMs: Int64(
              now.add(const Duration(days: 6)).millisecondsSinceEpoch,
            ),
          ),
          PairedDevice(
            id: 'd2',
            name: 'Old iPad',
            pairedMs: Int64(now.millisecondsSinceEpoch),
            confirmByMs: Int64(
              now.subtract(const Duration(days: 1)).millisecondsSinceEpoch,
            ),
          ),
          PairedDevice(
            id: 'd3',
            name: 'MacBook',
            pairedMs: Int64(now.millisecondsSinceEpoch),
            confirmed: true,
          ),
        ],
        pendingDevices: [
          PendingDevice(
            deviceId: 'd4',
            name: "Matt's iPad",
            number: '0427',
            signedIn: true,
            discovered: true,
          ),
        ],
      ),
    );

    testWidgets("a device not confirmed yet says by when, and is confirmed "
        "from here", (tester) async {
      final harness = Harness(devices());
      harness.page.value = AppPage.settings;
      await harness.show(tester, size: const Size(1400, 1000));
      await pumpFrames(tester);
      await tester.scrollUntilVisible(
        find.text('Add a device…'),
        200,
        scrollable: find
            .descendant(
              of: find.byType(SettingsView),
              matching: find.byType(Scrollable),
            )
            .first,
      );
      await pumpFrames(tester);
      expect(
        find.textContaining('Not confirmed yet · confirm by '),
        findsOneWidget,
      );
      expect(
        find.text("Not confirmed: it can't connect until it's confirmed"),
        findsOneWidget,
      );
      expect(find.text("Confirm it's yours"), findsNWidgets(2));
      await tester.ensureVisible(find.text("Confirm it's yours").first);
      await pumpFrames(tester);
      await tester.tap(find.text("Confirm it's yours").first);
      await pumpFrames(tester);
      expect(harness.last.confirmDevice.deviceId, 'd1');
    });

    testWidgets('a device that found this Claktory through the account asks '
        'to be allowed, with its number', (tester) async {
      final harness = Harness(devices());
      await harness.show(tester, size: const Size(1200, 900));
      await pumpFrames(tester);
      expect(
        find.text("Matt's iPad, signed in as you, wants to connect."),
        findsOneWidget,
      );
      expect(
        find.text('Allow it only if its screen shows 0427.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Allow'));
      await pumpFrames(tester);
      expect(harness.last.approveDevice.deviceId, 'd4');
    });
  });

  test('a deadline reads as a day', () {
    final now = DateTime(2026, 11, 20, 18);
    expect(deadlineDay(DateTime(2026, 11, 20, 23), now), 'today');
    expect(deadlineDay(DateTime(2026, 11, 21, 9), now), 'tomorrow');
    expect(deadlineDay(DateTime(2026, 11, 27, 9), now), '27 Nov');
    expect(deadlineDay(DateTime(2027, 1, 3), now), '3 Jan 2027');
  });

  test("the service's refusals in the app's words", () {
    expect(
      accountProblem(
        const AccountServiceError(429, 'rate_limited', '', {
          'retry_after': 600,
        }),
        defaultAccountService,
      ),
      'Too many tries. Try again in 10 minutes.',
    );
    expect(
      accountProblem(
        const AccountServiceUnreachable('timed out'),
        'http://127.0.0.1:8182',
      ),
      "Can't reach 127.0.0.1. Check your internet connection and try again.",
    );
  });
}
