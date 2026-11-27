import 'dart:async';

import 'package:factory_app/account.dart' show openAccountPage, privacyPage;
import 'package:factory_app/controls.dart' show Button, showModal;
import 'package:factory_app/device_account.dart';
import 'package:factory_app/loopback_forward.dart' show linkLauncher;
import 'package:factory_app/connections.dart'
    show RemoteFactory, parsePairingCode;
import 'package:factory_app/devices.dart' show FactoryChooser, thisDeviceKind;
import 'package:factory_app/drafts.dart' show MemoryDraftStorage;
import 'package:factory_app/people/e2ee.dart' show e2eeFor;
import 'package:factory_app/pairing.dart' show DeviceNumberField;
import 'package:factory_app/phone.dart' show PairScreen;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart'
    show Claktories, ClaktoriesScope, DeviceDrafts;
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:factory_app/visible_codes.dart' show nameInText;
import 'package:factory_app/your_claktories.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'device_confirm_test.dart' show FakeAccountApi, home, signIn;
import 'dm_fakes.dart';
import 'harness.dart';
import 'pair_start.dart';

Future<bool> launchUrlForTests(Uri url) async => true;

/// A name that reorders the words around it and isn't all ASCII.
const _oddName = 'Ana’s \u202EMac\u202C שלום';

void main() {
  _addClaktoryTests();
  Future<(Harness, FakeAccountApi)> phone(
    WidgetTester tester, {
    bool signedIn = false,
    bool none = false,
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
    if (none) api.claktoryList = [];
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
    await pastPairStart(tester, email: true);
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

  testWidgets('signing in again with the address fixed: only that account, '
      'never another (row 644)', (tester) async {
    final api = FakeAccountApi();
    final account = DeviceAccount(key: 'k' * 64, open: (_, _) => api);
    await signIn(account);
    final harness = Harness(Workspace(), account: account);
    await harness.show(
      tester,
      home: const Center(
        child: SizedBox(
          width: 480,
          child: DeviceSignInForm(
            intro: 'For your security, sign in again to start over.',
            email: 'matt@example.com',
          ),
        ),
      ),
    );
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('device-email')), findsNothing);
    expect(find.text('matt@example.com'), findsOneWidget);
    await tester.tap(find.text('Send code'));
    await pumpFrames(tester);
    expect(
      find.text(
        'Enter the 6-digit code sent to matt@example.com. It works for 15 '
        'minutes.',
      ),
      findsOneWidget,
    );
    expect(find.text('Use a different email'), findsNothing);
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

  test("the sign-in names the Claktory's email, masked, when it's known "
      '(#529, MC13)', () {
    expect(maskedEmail('matt@gmail.com'), 'm…@gmail.com');
    expect(maskedEmail('not an email'), '');
    expect(maskedEmail('@gmail.com'), '');
    expect(
      signInWithIntro(
        'Home',
        ClaktoryAccount(
          state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN,
          email: 'matt@gmail.com',
        ),
      ),
      'Sign in with m…@gmail.com, the email Home is signed in with.',
    );
    // Not signed in, or nothing said: the words without it.
    for (final account in [
      ClaktoryAccount(),
      ClaktoryAccount(
        state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_AWAITING_CODE,
        email: 'matt@gmail.com',
      ),
    ]) {
      expect(
        signInWithIntro('Home', account),
        'Sign in with the email Home is signed in with.',
      );
    }
  });

  test('signing out names the Claktories that stop connecting (#529, MC5)', () {
    expect(stopConnecting([], 'iPhone'), '');
    expect(
      stopConnecting(['nixos'], 'iPhone'),
      'nixos stops connecting on this iPhone until you sign in again.',
    );
    expect(
      stopConnecting(['Mac Studio', 'nixos'], 'iPhone'),
      'Mac Studio and nixos stop connecting on this iPhone until you sign in '
      'again.',
    );
    expect(
      stopConnecting(['A', 'B', 'C', 'D'], 'Mac'),
      'A, B and 2 more stop connecting on this Mac until you sign in again.',
    );
  });

  testWidgets('with no messages, signing out still asks when Claktories '
      'found through the account would stop connecting (#529, MC5)', (
    tester,
  ) async {
    final api = FakeAccountApi();
    final account = DeviceAccount(key: 'k' * 64, open: (_, _) => api);
    await signIn(account);
    final harness = Harness(Workspace(), localBackend: false, account: account);
    harness.connections
      ..use(
        RemoteFactory.fromAccount(
          // Its name as the account keeps it, whoever chose it.
          AccountClaktory(id: 'n', name: 'nix\u202Eos', endpointId: 'c' * 64),
        ),
      )
      // One paired with a code lets this device in signed out too.
      ..use(parsePairingCode('factory://?t=${'a' * 64}&e=${'d' * 64}&name=pi'));
    await harness.show(
      tester,
      home: Builder(
        builder: (context) => Button(
          label: 'Go',
          onPressed: () => unawaited(signOutOfAccount(context, account)),
        ),
      ),
    );
    await tester.tap(find.text('Go'));
    await pumpFrames(tester);
    expect(find.text('Sign out of your Claktory account?'), findsOneWidget);
    expect(
      find.text(
        '${nameInText('nix\u202Eos')} stops connecting on this '
        '$thisDeviceKind until you sign in again.',
      ),
      findsOneWidget,
    );
    expect(account.signedIn, isTrue, reason: 'only once asked');
    await tester.tap(find.text('Sign out').last);
    await pumpFrames(tester);
    expect(account.signedIn, isFalse);
  });

  testWidgets('a Claktory in the rail named with hidden characters reads as '
      'one name in what signing out says, its codes shown once', (
    tester,
  ) async {
    final api = FakeAccountApi();
    final account = DeviceAccount(key: 'k' * 64, open: (_, _) => api);
    await signIn(account);
    final harness = Harness(Workspace(), localBackend: false, account: account);
    harness.connections.use(
      RemoteFactory.fromAccount(
        AccountClaktory(id: 'n', name: _oddName, endpointId: 'c' * 64),
      ),
    );
    final claktories = Claktories(
      connections: harness.connections,
      localBackend: false,
      open: (target) => RecordingBackend()..connectTo(target),
      drafts: DeviceDrafts(MemoryDraftStorage()),
    );
    addTearDown(claktories.dispose);
    await harness.show(
      tester,
      home: ClaktoriesScope(
        claktories: claktories,
        child: Builder(
          builder: (context) => Button(
            label: 'Go',
            onPressed: () => unawaited(signOutOfAccount(context, account)),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Go'));
    await pumpFrames(tester);
    expect(
      find.text(
        '${nameInText(_oddName)} stops connecting on this '
        '$thisDeviceKind until you sign in again.',
      ),
      findsOneWidget,
    );
  });

  group('signing out with messages here (row 671)', () {
    tearDown(() => e2eeFor = null);

    Future<(Harness, DeviceAccount)> phone_(WidgetTester tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final api = FakeAccountApi();
      var made = 0;
      final account = DeviceAccount(
        key: 'k' * 64,
        open: (_, _) => api,
        makeKey: () => '${++made}' * 64,
      );
      await signIn(account);
      final harness = Harness(
        Workspace(),
        localBackend: false,
        account: account,
      );
      await harness.show(tester, size: const Size(393, 852));
      await pumpFrames(tester);
      return (harness, account);
    }

    Future<FakeEndpoint> ready(FakeWorker worker, String leaf) async {
      final endpoint = FakeEndpoint(worker, 'matt', leaf);
      await endpoint.e2ee.start();
      await endpoint.e2ee.setUpFirst();
      return endpoint;
    }

    testWidgets('with other devices: says they keep them, then leaves and '
        'deletes them here, as a new device next time', (tester) async {
      final worker = FakeWorker();
      final phone = FakeEndpoint(worker, 'matt', 'phone');
      final mac = await ready(worker, 'mac');
      await phone.e2ee.start();
      final code = await phone.e2ee.offerThisDevice(platform: 'iPhone');
      await mac.e2ee.addDevice(await mac.e2ee.scanned(code!));
      await phone.e2ee.takeHandoff();
      e2eeFor = (_) => phone.e2ee;
      final (_, account) = await phone_(tester);
      final key = account.key;

      await tester.tap(find.text('Sign out'));
      await pumpFrames(tester);
      expect(find.text('Sign out of your Claktory account?'), findsOneWidget);
      expect(
        find.text(
          'Your messages will be deleted from this iPhone. Your other '
          'devices keep them, and you can add this iPhone again from one '
          'of them.',
        ),
        findsOneWidget,
      );
      expect(find.textContaining('Delete messages'), findsNothing);
      await tester.tap(find.text('Sign out').last);
      await pumpFrames(tester);
      expect(account.signedIn, isFalse);
      expect(account.key, isNot(key), reason: 'a new device next time');
      expect(phone.keys.kept, isEmpty);
      expect(phone.storeKept, isFalse);
      expect(worker.keys['matt']!['version'], 3);
      expect(find.text('Sign out of your Claktory account?'), findsNothing);
    });

    testWidgets('the only device: they stay, locked, unless Delete messages '
        'is on, which says what it costs', (tester) async {
      final worker = FakeWorker();
      final phone = await ready(worker, 'phone');
      e2eeFor = (_) => phone.e2ee;
      final (_, account) = await phone_(tester);
      final key = account.key;

      await tester.tap(find.text('Sign out'));
      await pumpFrames(tester);
      expect(
        find.text(
          'This is the only device with your messages. They stay on this '
          'iPhone, locked, until you sign in again with this account.',
        ),
        findsOneWidget,
      );
      const note =
          "Your security key is deleted too. Next time you'll start over, "
          'and your friends will see your security key change.';
      expect(find.text(note), findsNothing);
      await tester.tap(find.text('Delete messages from this iPhone'));
      await pumpFrames(tester);
      expect(find.text(note), findsOneWidget);
      expect(find.text('Sign out and delete'), findsOneWidget);
      await tester.tap(find.text('Sign out and delete'));
      await pumpFrames(tester);
      expect(account.signedIn, isFalse);
      expect(account.key, key);
      expect(phone.keys.kept, isEmpty);
      expect(phone.storeKept, isFalse);
    });

    testWidgets('the only device, Delete messages off: signed out, nothing '
        'here deleted', (tester) async {
      final worker = FakeWorker();
      final phone = await ready(worker, 'phone');
      e2eeFor = (_) => phone.e2ee;
      final (_, account) = await phone_(tester);
      final kept = {...phone.keys.kept.keys};
      await tester.tap(find.text('Sign out'));
      await pumpFrames(tester);
      await tester.tap(find.text('Sign out').last);
      await pumpFrames(tester);
      expect(account.signedIn, isFalse);
      expect(phone.keys.kept.keys, kept);
      expect(phone.storeKept, isTrue);
    });

    testWidgets('Cancel signs nothing out', (tester) async {
      final worker = FakeWorker();
      final phone = await ready(worker, 'phone');
      e2eeFor = (_) => phone.e2ee;
      final (_, account) = await phone_(tester);
      await tester.tap(find.text('Sign out'));
      await pumpFrames(tester);
      await tester.tap(find.text('Cancel'));
      await pumpFrames(tester);
      expect(account.signedIn, isTrue);
      expect(phone.storeKept, isTrue);
    });
  });

  testWidgets('no Claktory on the account yet: where to get one, the '
      'download page a link', (tester) async {
    final pages = <Uri>[];
    linkLauncher = (url) async {
      pages.add(url);
      return true;
    };
    addTearDown(() => linkLauncher = null);
    await phone(tester, signedIn: true, none: true);
    expect(
      find.text(
        'No Claktories on this account yet. Install Claktory on a Mac or a '
        'Linux server (or Windows, in preview) from claktory.com/download '
        'and sign in with this email, and it shows up here.',
        findRichText: true,
      ),
      findsOneWidget,
    );
    await tester.tapOnText(find.textRange.ofSubstring('claktory.com/download'));
    await pumpFrames(tester);
    expect(pages, [Uri.parse(downloadUrl)]);
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

  testWidgets('a Claktory that was signed out itself says so, and that '
      'it is the one to sign in (friend bug #38)', (tester) async {
    final (harness, _) = await phone(tester);
    harness.connections.use(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=Home'),
    );
    const said =
        'This Claktory was signed out of your Claktory account, so it '
        "can't let this device in. Sign it in again in its Settings → "
        'Account, then try again here.';
    harness.backend.signInRequired.value = said;
    await pumpFrames(tester);
    expect(find.text('Home was signed out'), findsOneWidget);
    expect(find.text('Sign in to connect to Home'), findsNothing);
    // Where to sign it in, in the app: on its screen, or with no screen,
    // connected again by factoryd pair's code (MC4; the account commands
    // are the person's alone).
    expect(
      find.text(
        'Sign Home in to your Claktory account again, in Settings → Account '
        'on Home. Then try again here.',
      ),
      findsOneWidget,
    );
    expect(
      find.text(
        'No screen on Home? Run this in a terminal there and connect this '
        'iPhone with the code it shows. Then sign Home in from here, in its '
        'Settings → Account.',
      ),
      findsOneWidget,
    );
    expect(find.bySemanticsLabel('factoryd pair'), findsOneWidget);
    expect(find.textContaining('factoryd account'), findsNothing);
    // Not this device's sign-in: the Claktory's.
    expect(find.byKey(const ValueKey('device-email')), findsNothing);
    expect(find.text('Sign out'), findsNothing);
    await tester.tap(find.text('Try again'));
    await pumpFrames(tester);
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
    // Still being added by its code: Cancel, nothing kept.
    expect(find.text('Cancel'), findsOneWidget);
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

  testWidgets("one still being added by its code is cancelled at once, "
      'without a question (§4; Lucy 50050)', (tester) async {
    final (harness, _) = await phone(tester);
    final home = parsePairingCode(
      'factory://?t=${'c' * 64}&e=${'a' * 64}&name=Home',
    )!;
    harness.connections.use(home);
    harness.backend.signInRequired.value =
        'Confirm this device is yours: sign in.';
    await pumpFrames(tester);
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(harness.connections.saved, isNot(contains(home)));
    expect(find.textContaining('Remove'), findsNothing);
  });

  testWidgets('signed in with another email than the Claktory, it says '
      'whose, and Switch to… signs out, then asks for that email (Trish '
      '50061)', (tester) async {
    final (harness, _) = await phone(tester, signedIn: true);
    harness.connections.use(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=Home'),
    );
    harness.backend
      ..signInSaid = (service: '', email: 'b…@example.com')
      ..signInRequired.value = 'Confirm this device is yours: sign in.';
    await pumpFrames(tester);
    expect(find.text('Home uses another account'), findsOneWidget);
    expect(
      find.text(
        'Home is signed in as b…@example.com, and this iPhone as '
        'matt@example.com. To connect, sign in here with b…@example.com.',
      ),
      findsOneWidget,
    );
    // Said once: neither the backend's sentence nor a line of its own.
    expect(find.text('Confirm this device is yours: sign in.'), findsNothing);
    expect(find.text('Try again'), findsOneWidget);
    // Signed out (asking first only when something stops, as Sign out
    // does), the email step, the address to type as its placeholder.
    await tester.tap(find.text('Switch to b…@example.com…'));
    await pumpFrames(tester);
    expect(harness.connections.account.signedIn, isFalse);
    expect(find.byKey(const ValueKey('device-email')), findsOneWidget);
    expect(find.text('b…@example.com'), findsOneWidget);
  });

  testWidgets("the sign-in sheet's Cancel stays at its title's end on an "
      "SE, the title wrapping beside it (Zucchero's row 720)", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    await harness.show(
      tester,
      size: const Size(320, 568),
      home: Builder(
        builder: (context) => DeviceSignInDialog(
          title: 'Sign in with the account qa-zdb uses',
          intro: 'Use the email qa-zdb is signed in with.',
          close: () {},
        ),
      ),
    );
    await pumpFrames(tester);
    final title = tester.getRect(
      find.text('Sign in with the account qa-zdb uses'),
    );
    final cancel = tester.getRect(find.text('Cancel'));
    expect(cancel.top, lessThan(title.bottom));
    expect(cancel.left, greaterThan(title.left));
  });

  testWidgets('turned away with no Workspace yet, the sign-in names the '
      "email the Claktory said and uses its account service (Sale's row "
      '189 b)', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final api = FakeAccountApi();
    final opened = <String>[];
    final account = DeviceAccount(
      key: 'k' * 64,
      open: (_, service) {
        opened.add(service);
        return api;
      },
    );
    final harness = Harness(Workspace(), localBackend: false, account: account);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    harness.connections.use(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=Home'),
    );
    harness.backend
      ..signInSaid = (
        service: 'https://account.example.test',
        email: 'm…@example.com',
      )
      ..signInRequired.value = 'Confirm this device is yours: sign in.';
    await pumpFrames(tester);
    expect(
      find.text(
        'Sign in with m…@example.com, the email Home is signed in with.',
      ),
      findsOneWidget,
    );
    await tester.enterText(
      find.byKey(const ValueKey('device-email')),
      'matt@example.com',
    );
    await pumpFrames(tester);
    await tester.tap(find.text('Send code'));
    await pumpFrames(tester);
    expect(opened.last, 'https://account.example.test');
  });

  testWidgets("turned away by a Claktory on claktory.com's service, it "
      "signs in there, whatever another Claktory's is (row 189 b)", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final api = FakeAccountApi();
    final opened = <String>[];
    final account = DeviceAccount(
      key: 'k' * 64,
      open: (_, service) {
        opened.add(service);
        return api;
      },
    );
    final harness = Harness(Workspace(), localBackend: false, account: account);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    harness.connections.use(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=Home'),
    );
    // Its email said, its service empty: claktory.com's.
    harness.backend
      ..signInSaid = (service: '', email: 'm…@example.com')
      ..signInRequired.value = 'Confirm this device is yours: sign in.';
    await pumpFrames(tester);
    await tester.enterText(
      find.byKey(const ValueKey('device-email')),
      'matt@example.com',
    );
    await pumpFrames(tester);
    await tester.tap(find.text('Send code'));
    await pumpFrames(tester);
    expect(opened.last, defaultAccountService);
    expect(find.byKey(const ValueKey('device-sign-in-at')), findsNothing);
  });

  testWidgets("a store build's own sign-in goes to claktory.com, whatever "
      'service a Claktory in the rail names (Fugo 53274)', (tester) async {
    final api = FakeAccountApi();
    final opened = <String>[];
    final account = DeviceAccount(
      key: 'k' * 64,
      open: (_, service) {
        opened.add(service);
        return api;
      },
    );
    // Someone else's Claktory, first in the rail, naming its own server.
    final harness = Harness(
      Workspace(account: ClaktoryAccount(service: 'https://evil.example')),
      account: account,
    );
    final claktories = Claktories(
      connections: harness.connections,
      localBackend: true,
      open: (_) => harness.backend,
      drafts: DeviceDrafts(MemoryDraftStorage()),
    );
    addTearDown(claktories.dispose);
    // Claktory Dev and debug builds take it, for QA on the dev service.
    expect(accountServiceFor(claktories), 'https://evil.example');
    debugServiceFromClaktories = false;
    addTearDown(() => debugServiceFromClaktories = null);
    expect(accountServiceFor(claktories), defaultAccountService);
    await harness.show(
      tester,
      home: ClaktoriesScope(
        claktories: claktories,
        child: const DeviceSignInForm(intro: 'Sign in.'),
      ),
    );
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('device-sign-in-at')), findsNothing);
    await tester.enterText(
      find.byKey(const ValueKey('device-email')),
      'matt@example.com',
    );
    await pumpFrames(tester);
    await tester.tap(find.text('Send code'));
    await pumpFrames(tester);
    expect(opened, [defaultAccountService]);
  });

  testWidgets('turned away by a Claktory on a server of its own, a store '
      'build signs in there and says so (Fugo 53274)', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    debugServiceFromClaktories = false;
    addTearDown(() => debugServiceFromClaktories = null);
    final api = FakeAccountApi();
    final opened = <String>[];
    final account = DeviceAccount(
      key: 'k' * 64,
      open: (_, service) {
        opened.add(service);
        return api;
      },
    );
    final harness = Harness(Workspace(), localBackend: false, account: account);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    harness.connections.use(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=Home'),
    );
    harness.backend.workspace.apply(
      Workspace(account: ClaktoryAccount(service: 'https://evil.example')),
    );
    harness.backend.signInRequired.value =
        'Confirm this device is yours: sign in.';
    await pumpFrames(tester);
    expect(find.text('Sign in to connect to Home'), findsOneWidget);
    expect(
      find.text("You'll sign in at evil.example, not claktory.com."),
      findsOneWidget,
    );
    await tester.enterText(
      find.byKey(const ValueKey('device-email')),
      'matt@example.com',
    );
    await pumpFrames(tester);
    await tester.tap(find.text('Send code'));
    await pumpFrames(tester);
    expect(opened.last, 'https://evil.example');
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
      find.textContaining('Sign in to your Claktory account in this app by '),
      findsOneWidget,
    );
    await tester.tap(
      find.descendant(
        of: find.byType(ConfirmDeviceBanner),
        matching: find.text('Sign in…'),
      ),
    );
    await pumpFrames(tester);
    expect(find.text('Sign in to keep using Home'), findsOneWidget);
    expect(find.byKey(const ValueKey('device-sign-in-at')), findsNothing);
  });

  testWidgets('a Claktory on a server of its own asks to confirm: a store '
      'build signs in there and says so (Fugo 53274)', (tester) async {
    debugServiceFromClaktories = false;
    addTearDown(() => debugServiceFromClaktories = null);
    final (harness, _) = await phone(tester);
    harness.connections.use(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=Home'),
    );
    harness.backend.workspace.apply(
      Workspace(account: ClaktoryAccount(service: 'https://own.example')),
    );
    harness.backend.confirmBy.value = DateTime.now().add(
      const Duration(days: 30),
    );
    await pumpFrames(tester);
    await tester.tap(
      find.descendant(
        of: find.byType(ConfirmDeviceBanner),
        matching: find.text('Sign in…'),
      ),
    );
    await pumpFrames(tester);
    expect(
      find.text("You'll sign in at own.example, not claktory.com."),
      findsOneWidget,
    );
  });

  testWidgets("at the largest text the banner's words grow only to 2×", (
    tester,
  ) async {
    final (harness, _) = await phone(tester);
    harness.connections.use(
      parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=Home'),
    );
    harness.backend.confirmBy.value = DateTime.now().add(
      const Duration(days: 30),
    );
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await pumpFrames(tester);
    final words = find.textContaining(
      'Sign in to your Claktory account in this app by ',
    );
    expect(MediaQuery.textScalerOf(tester.element(words)).scale(10), 20);
    expect(tester.takeException(), isNull);
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
    final words = find.text(
      "A device that calls itself “Matt's iPad”, signed in as you, wants to "
      'connect.',
    );
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

    await tester.enterText(_numberField, '0427');
    await pumpFrames(tester);
    await tester.tap(find.text('Allow'));
    await pumpFrames(tester);
    expect(harness.last.approveDevice.deviceId, 'd4');
    expect(harness.last.approveDevice.number, '0427');
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
      await revealInSettings(tester, find.text('Add a device…'));
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
        find.text(
          "A device that calls itself “Matt's iPad”, signed in as you, wants "
          'to connect.',
        ),
        findsOneWidget,
      );
      // Typed off its screen, never shown here, with what to
      // do if it isn't the person's.
      expect(find.textContaining('0427'), findsNothing);
      expect(
        find.textContaining("If you didn't just set up a new device"),
        findsOneWidget,
      );
      final allow = find.widgetWithText(Button, 'Allow');
      expect(tester.widget<Button>(allow).onPressed, isNull);
      // A wrong number: the backend's words.
      harness.backend.refuse =
          "That's not the number Matt's iPad shows. 2 tries left.";
      await tester.enterText(_numberField, '1111');
      await pumpFrames(tester);
      await tester.tap(allow);
      await pumpFrames(tester);
      expect(harness.last.approveDevice.number, '1111');
      expect(
        find.text("That's not the number Matt's iPad shows. 2 tries left."),
        findsOneWidget,
      );
      harness.backend.refuse = null;
      await tester.enterText(_numberField, '0427');
      await pumpFrames(tester);
      await tester.tap(allow);
      await pumpFrames(tester);
      expect(harness.last.approveDevice.deviceId, 'd4');
      expect(harness.last.approveDevice.number, '0427');
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

/// Add a Claktory on a Mac with its own Claktory,
/// signed in: [listed] on the account, [shown] already here.
Future<(Harness, FakeAccountApi)> _addClaktory(
  WidgetTester tester, {
  required List<AccountClaktory> listed,
  RemoteFactory? shown,
  bool localBackend = true,
}) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final api = FakeAccountApi()..claktoryList = listed;
  final account = DeviceAccount(key: 'k' * 64, open: (_, _) => api);
  await signIn(account);
  final harness = Harness(
    Workspace(),
    localBackend: localBackend,
    account: account,
  );
  if (shown != null) harness.connections.use(shown);
  await harness.show(tester, size: const Size(1200, 800));
  await pumpFrames(tester);
  unawaited(
    showModal(
      tester.element(find.byType(Sidebar).first),
      (context, close) => FactoryChooser(close: close),
    ),
  );
  await pumpFrames(tester);
  return (harness, api);
}

void _addClaktoryTests() {
  final server = const AccountClaktory(
    id: 'c2',
    name: 'Server',
    machine: 'nas',
    endpointId: 'e2e2e2e2e2e2e2e2',
    relays: ['https://relay.example.com'],
  );

  testWidgets('Add a Claktory: the account first, a row asks in place with '
      'one Cancel on screen, and no sign-in line on a Mac with its own', (
    tester,
  ) async {
    await _addClaktory(tester, listed: [home, server]);
    expect(find.text('Add a Claktory'), findsOneWidget);
    expect(find.text('On your account · matt@example.com'), findsOneWidget);
    expect(find.text('Home'), findsOneWidget);
    expect(find.text('Server'), findsOneWidget);
    // Signed in through this Mac's Claktory: Settings → Account says so.
    expect(find.textContaining('Signed in as'), findsNothing);
    expect(find.text('Use a code instead'), findsOneWidget);
    expect(find.text('Cancel'), findsOneWidget);
    // Nothing to submit until a row is chosen.
    expect(find.text('Connect'), findsNothing);

    await tester.tap(find.text('Server'));
    await pumpFrames(tester);
    expect(find.text('Connect to Server?'), findsOneWidget);
    // The row's own Cancel only (YourClaktories.onAsking).
    expect(find.text('Cancel'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(find.text('Connect to Server?'), findsNothing);
    expect(find.text('Server'), findsOneWidget);
    expect(find.text('Cancel'), findsOneWidget, reason: "the dialog's again");
  });

  testWidgets("a name from the account service reads as a name: what "
      "reorders it shows by its code, set apart (Abbacchio's row 1784)", (
    tester,
  ) async {
    const evil = 'Evil\u202EMac\u200B of someone';
    final shown = nameInText(evil);
    expect(shown, contains('Evil\\u{202E}Mac'));
    final odd = const AccountClaktory(
      id: 'c3',
      name: evil,
      machine: 'nas\u202Eexe',
      endpointId: 'e3e3e3e3e3e3e3e3',
      relays: ['https://relay.example.com'],
    );
    await _addClaktory(tester, listed: [odd]);
    expect(find.text(shown), findsOneWidget);
    expect(find.textContaining('\u202E'), findsNothing);
    expect(find.textContaining('nas\\u{202E}exe'), findsOneWidget);
    await tester.tap(find.text(shown));
    await pumpFrames(tester);
    expect(find.text('Connect to $shown?'), findsOneWidget);
    expect(find.textContaining('If you already use $shown on'), findsOneWidget);
    expect(find.textContaining('\u202E'), findsNothing);
  });

  testWidgets('Add a Claktory with every one already here says so, with Use '
      'a code (YourClaktories.onNothingLeft)', (tester) async {
    await _addClaktory(
      tester,
      listed: [home],
      shown: RemoteFactory.fromAccount(home),
    );
    expect(
      find.text(
        'All your Claktories are already here. To add one that isn\'t on '
        'your account yet, use its code.',
      ),
      findsOneWidget,
    );
    expect(find.text('Use a code instead'), findsNothing);
    await tester.tap(find.text('Use a code'));
    await pumpFrames(tester);
    expect(find.text('Add a Claktory with a code'), findsOneWidget);
  });

  testWidgets('Add a Claktory on a device without its own Claktory ends with '
      'its sign-in line (§6)', (tester) async {
    await _addClaktory(tester, listed: [server], localBackend: false);
    expect(find.text('Signed in as matt@example.com'), findsOneWidget);
    expect(find.text('Server'), findsOneWidget);
  });
}

/// The field a waiting device's number is typed in, in the banner.
Finder get _numberField => find.descendant(
  of: find.byType(DeviceNumberField),
  matching: find.byType(EditableText),
);
