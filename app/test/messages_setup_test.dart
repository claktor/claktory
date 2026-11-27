import 'package:factory_app/controls.dart' show Pressable;
import 'package:factory_app/device_account.dart';
import 'package:factory_app/people/dm_keys.dart';
import 'package:factory_app/people/e2ee.dart';
import 'package:factory_app/people/messages_setup.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
import 'package:factory_app/tunnel/api/dm_transfer.dart';
import 'package:factory_app/people/secret_clipboard.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart'
    show AnyhowException;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'device_confirm_test.dart' show FakeAccountApi, signIn;
import 'dm_fakes.dart';
import 'harness.dart';

// People's messages on this device: setting up the
// first device, a new one's code, adding it by scanning, and removing one.

Future<Harness> _show(
  WidgetTester tester,
  E2ee e2ee, {
  DeviceAccount? account,
}) async {
  final harness = Harness(Workspace(), account: account);
  await harness.show(
    tester,
    home: Center(
      child: SizedBox(
        width: 480,
        child: SingleChildScrollView(
          child: Column(
            children: [
              MessagesSetup(e2ee: e2ee),
              MessagesSetup(e2ee: e2ee, devices: true),
            ],
          ),
        ),
      ),
    ),
  );
  await _settle(tester);
  return harness;
}

/// A clipboard with the platform's half: what's on it, and whether the
/// last copy is still what's there.
class _Clipboard implements SecretClipboard {
  String? text;
  var _writes = 0;
  int? _ours;

  @override
  MethodChannel get channel => throw UnimplementedError();

  @override
  Future<bool> get available async => true;

  @override
  Future<int?> copy(String text, {int lasts = SecretClipboard.seconds}) async {
    this.text = text;
    return _ours = ++_writes;
  }

  /// The person copies something else.
  void personCopies(String text) {
    this.text = text;
    _writes++;
    _ours = null;
  }

  @override
  Future<void> clear(int token) async {
    if (token == _ours && token == _writes) text = null;
  }

  @override
  Future<String?> paste() async => text;
}

/// The engine's calls take several turns, each after a frame.
Future<void> _settle(WidgetTester tester) async {
  for (var i = 0; i < 4; i++) {
    await settle(tester);
  }
}

void main() {
  group('no other device (row 630)', () {
    testWidgets('a new security key after the warning, then Your devices', (
      tester,
    ) async {
      final worker = FakeWorker();
      await _first(worker);
      final ipad = FakeEndpoint(worker, 'matt', 'ipad')..skipSecrets(100);
      await ipad.e2ee.start();
      await _show(tester, ipad.e2ee);
      await tester.tap(find.text('No other device?'));
      await _settle(tester);
      expect(find.textContaining('starts with no messages'), findsOneWidget);
      expect(
        find.textContaining('anyone who verified you will need to again'),
        findsOneWidget,
      );
      await tester.tap(find.text('Start over'));
      await _settle(tester);
      expect(ipad.e2ee.setup, DmSetup.ready);
      expect(find.text('Your devices'), findsOneWidget);
    });

    testWidgets('the devices with the old key are listed apart, with Remove…', (
      tester,
    ) async {
      final worker = FakeWorker();
      await _first(worker);
      final ipad = FakeEndpoint(worker, 'matt', 'ipad')..skipSecrets(100);
      await ipad.e2ee.start();
      await ipad.e2ee.startNewKey();
      await ipad.e2ee.sync();
      // Taken out of the chats, never signed out on its own.
      expect(worker.signedOut['matt'] ?? const <String>{}, isEmpty);
      await _show(tester, ipad.e2ee);
      expect(find.text('Still on your old security key'), findsOneWidget);
      expect(find.text('mac'), findsOneWidget);
      await tester.tap(find.text('Remove…'));
      await _settle(tester);
      expect(find.text('Remove mac?'), findsOneWidget);
      await tester.tap(find.text('Remove'));
      await _settle(tester);
      expect(worker.signedOut['matt'], {'mac'});
      expect(find.text('Still on your old security key'), findsNothing);
      expect(find.text('Remove…'), findsNothing);
    });

    testWidgets('Cancel changes nothing', (tester) async {
      final worker = FakeWorker();
      await _first(worker);
      final old = worker.keys['matt']!['identity_key'];
      final ipad = FakeEndpoint(worker, 'matt', 'ipad')..skipSecrets(100);
      await ipad.e2ee.start();
      await _show(tester, ipad.e2ee);
      await tester.tap(find.text('No other device?'));
      await _settle(tester);
      await tester.tap(find.text('Cancel'));
      await _settle(tester);
      expect(ipad.e2ee.setup, DmSetup.addThisDevice);
      expect(worker.keys['matt']!['identity_key'], old);
    });

    testWidgets('a sign-in too old asks to sign in again first', (
      tester,
    ) async {
      final worker = FakeWorker();
      await _first(worker);
      final ipad = FakeEndpoint(worker, 'matt', 'ipad')..skipSecrets(100);
      await ipad.e2ee.start();
      worker.staleSignIn.add('matt/ipad');
      final account = DeviceAccount(
        key: 'k' * 64,
        open: (_, _) => FakeAccountApi(),
      );
      await signIn(account);
      await _show(tester, ipad.e2ee, account: account);
      await tester.tap(find.text('No other device?'));
      await _settle(tester);
      await tester.tap(find.text('Start over'));
      await _settle(tester);
      expect(ipad.e2ee.setup, DmSetup.addThisDevice);
      expect(
        find.text('For your security, sign in again to start over.'),
        findsOneWidget,
      );
      // Again with the account it's signed in to, never another.
      await tester.tap(find.text('Sign in again…'));
      await _settle(tester);
      expect(find.text('matt@example.com'), findsOneWidget);
      expect(find.byKey(const ValueKey('device-email')), findsNothing);
    });
  });

  testWidgets('the first device sets up, then lists itself', (tester) async {
    final worker = FakeWorker();
    final phone = FakeEndpoint(worker, 'matt', 'phone', name: 'iPhone');
    await phone.e2ee.start();
    await _show(tester, phone.e2ee);
    expect(find.text('Set up messages'), findsOneWidget);
    expect(find.text('Your devices'), findsNothing);

    await tester.tap(find.text('Set up'));
    await _settle(tester);
    expect(phone.e2ee.setup, DmSetup.ready);
    expect(find.text('Set up messages'), findsNothing);
    expect(find.text('Your devices'), findsOneWidget);
    await _settle(tester);
    expect(find.text('iPhone'), findsOneWidget);
    expect(find.text('This device'), findsOneWidget);
    expect(find.text('Remove…'), findsNothing);
  });

  testWidgets('where the messages are kept (#451): this Claktory, or '
      'another with Keep them on this Claktory; and who sees its address', (
    tester,
  ) async {
    final worker = FakeWorker();
    final phone = FakeEndpoint(worker, 'matt', 'phone', name: 'iPhone');
    await phone.e2ee.start();
    await phone.e2ee.setUpFirst();
    final home = FakeHome(worker);
    phone.e2ee.home = home;
    await phone.e2ee.sync();
    await phone.e2ee.sync();
    await _show(tester, phone.e2ee);
    expect(find.text('Where your messages are kept'), findsOneWidget);
    expect(
      find.text(
        "Your Claktory connects to your friends' Claktories directly when it "
        'can, so they can see its network address.',
      ),
      findsOneWidget,
    );
    // Its list named no home: the Claktory the app shows became it.
    expect(phone.e2ee.homeEndpoint, home.endpointId);
    expect(
      find.text('On the Claktory this app is connected to.'),
      findsOneWidget,
    );
    expect(find.text('Keep them on this Claktory'), findsNothing);

    // The app shows another of Matt's Claktories now.
    home
      ..isHome = false
      ..endpointId = List.filled(32, 6)
      ..said();
    await _settle(tester);
    expect(
      find.text(
        'On another of your Claktories. What you send waits until you '
        'connect to it, unless you keep them on this one.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Keep them on this Claktory'));
    await _settle(tester);
    expect(phone.e2ee.homeEndpoint, List.filled(32, 6));
    home
      ..isHome = true
      ..said();
    await _settle(tester);
    expect(
      find.text('On the Claktory this app is connected to.'),
      findsOneWidget,
    );
    await tester.pump(const Duration(seconds: 2));
  });

  // The Claktory answers the first routes before it
  // placed itself from the list that now names it; the list is the
  // person's choice, so it reads as this Claktory at once, with no Keep.
  testWidgets('set up on the Claktory the app shows: this Claktory, even '
      'before it says it placed itself', (tester) async {
    final worker = FakeWorker();
    final phone = FakeEndpoint(worker, 'matt', 'phone', name: 'iPhone');
    await phone.e2ee.start();
    await phone.e2ee.setUpFirst();
    final home = FakeHome(worker)..isHome = false;
    phone.e2ee.home = home;
    await phone.e2ee.sync();
    await phone.e2ee.sync();
    await _show(tester, phone.e2ee);
    expect(phone.e2ee.homeEndpoint, home.endpointId);
    expect(phone.e2ee.homeState!.isHome, isFalse);
    expect(
      find.text('On the Claktory this app is connected to.'),
      findsOneWidget,
    );
    expect(find.text('Keep them on this Claktory'), findsNothing);
    await tester.pump(const Duration(seconds: 2));
  });

  testWidgets('a new device shows its code; the phone scans and adds it', (
    tester,
  ) async {
    final worker = FakeWorker();
    final phone = FakeEndpoint(worker, 'matt', 'phone', name: 'iPhone');
    await phone.e2ee.start();
    await phone.e2ee.setUpFirst();
    final ipad = FakeEndpoint(worker, 'matt', 'ipad', name: 'Studio iPad');
    await ipad.e2ee.start();

    await _show(tester, ipad.e2ee);
    expect(find.textContaining('to your messages'), findsOneWidget);
    expect(find.text('Waiting for your other device…'), findsOneWidget);
    final code = ipad.e2ee.offer;
    expect(code, isNotNull);
    // Without the platform's safe clipboard, no copying.
    expect(find.text('Copy code'), findsNothing);

    await tester.pumpWidget(const SizedBox());
    final harness = await _show(tester, phone.e2ee);
    harness.camera.state = CameraAccess.granted;
    await tester.tap(find.text('Add a device…'));
    await _settle(tester);
    expect(find.text('Scan the code on your new device'), findsOneWidget);
    harness.camera.scan(code!);
    await _settle(tester);
    expect(find.text('Add this device to your messages?'), findsOneWidget);
    expect(find.textContaining('Studio iPad'), findsOneWidget);
    await tester.tap(find.text('Add'));
    await _settle(tester);
    expect(worker.handoffs['ipad'], hasLength(1));
    expect(find.text('Add this device to your messages?'), findsNothing);
  });

  testWidgets("a device that can't scan says where to add from", (
    tester,
  ) async {
    final worker = FakeWorker();
    final mac = FakeEndpoint(worker, 'matt', 'mac', name: 'Mac');
    await mac.e2ee.start();
    await mac.e2ee.setUpFirst();
    await _show(tester, mac.e2ee);
    await tester.tap(find.text('Add a device…'));
    await _settle(tester);
    expect(
      find.textContaining(
        'Copy the code on your new device and choose Paste code instead.',
      ),
      findsOneWidget,
    );
    expect(find.text('Open Settings'), findsNothing);
  });

  testWidgets('a refused camera says how to allow it, with Open Settings, '
      'and scans once allowed and back (friend bug #2)', (tester) async {
    final worker = FakeWorker();
    final phone = FakeEndpoint(worker, 'matt', 'phone', name: 'iPhone');
    await phone.e2ee.start();
    await phone.e2ee.setUpFirst();
    final harness = await _show(tester, phone.e2ee);
    harness.camera.state = CameraAccess.denied;
    await tester.tap(find.text('Add a device…'));
    await _settle(tester);
    expect(
      find.text('To scan, allow camera access in Settings → Claktory.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Open Settings'));
    await _settle(tester);
    expect(harness.camera.settingsOpened, 1);
    expect(find.byKey(const ValueKey('camera')), findsNothing);
    // Allowed in Settings, then back.
    harness.camera.state = CameraAccess.granted;
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await _settle(tester);
    expect(find.byKey(const ValueKey('camera')), findsOneWidget);
    expect(find.text('Open Settings'), findsNothing);
    // Paste stays the other way.
    expect(find.text('Paste code instead'), findsOneWidget);
  }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));

  testWidgets('a restricted camera offers no Settings, only Paste', (
    tester,
  ) async {
    final worker = FakeWorker();
    final phone = FakeEndpoint(worker, 'matt', 'phone', name: 'iPhone');
    await phone.e2ee.start();
    await phone.e2ee.setUpFirst();
    final harness = await _show(tester, phone.e2ee);
    harness.camera.state = CameraAccess.restricted;
    await tester.tap(find.text('Add a device…'));
    await _settle(tester);
    expect(find.textContaining("The camera isn't available"), findsOneWidget);
    expect(find.text('Open Settings'), findsNothing);
    expect(find.text('Paste code instead'), findsOneWidget);
  });

  // The new device's app closed before the other added it; its
  // code's keys went with it.
  testWidgets('a code its app closed on: the new code says the earlier '
      "one no longer works, and the device it listed says it never set "
      'up', (tester) async {
    final worker = FakeWorker();
    final phone = FakeEndpoint(worker, 'matt', 'phone', name: 'iPhone');
    await phone.e2ee.start();
    await phone.e2ee.setUpFirst();
    final ipad = FakeEndpoint(worker, 'matt', 'ipad');
    await ipad.e2ee.start();
    final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
    final ipadNow = ipad.relaunch();
    await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
    await ipadNow.start();
    await ipadNow.offerThisDevice(platform: 'iPad');
    await ipadNow.takeHandoff();
    expect(ipadNow.setup, DmSetup.addThisDevice);
    expect(ipadNow.earlierCodeSpent, isTrue);
    expect(worker.handoffs.values.expand((each) => each), isEmpty);

    await _show(tester, ipadNow);
    expect(
      find.textContaining('Your earlier code no longer works'),
      findsOneWidget,
    );
    await tester.pumpWidget(const SizedBox());

    // The phone lists it, waiting, until its code's 30 minutes are up.
    await phone.e2ee.sync();
    await _show(tester, phone.e2ee);
    expect(find.textContaining('Not set up'), findsNothing);
    await tester.pumpWidget(const SizedBox());
    phone.now += E2ee.codeLife.inMilliseconds;
    await _show(tester, phone.e2ee);
    expect(find.textContaining('Not set up'), findsOneWidget);
    expect(find.text('Remove…'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());

    // Added again with the new code: set up, and nothing more said.
    await phone.e2ee.addDevice(await phone.e2ee.scanned(ipadNow.offer!));
    await ipadNow.takeHandoff();
    expect(ipadNow.setup, DmSetup.ready);
    expect(ipadNow.earlierCodeSpent, isFalse);
    await phone.e2ee.sync();
    await _show(tester, phone.e2ee);
    expect(find.textContaining('Not set up'), findsNothing);
    await tester.pumpWidget(const SizedBox());
  });

  // A Claktory the person's account doesn't list says so, and
  // offers nothing to keep messages on it.
  testWidgets('a Claktory on another account says so, with no Keep', (
    tester,
  ) async {
    final worker = FakeWorker();
    final phone = FakeEndpoint(worker, 'matt', 'phone', name: 'iPhone');
    await phone.e2ee.start();
    await phone.e2ee.setUpFirst();
    phone.e2ee
      ..home = FakeHome(worker)
      ..ungatedForTests = false
      ..accountClaktories = (() async => const [])
      ..show(const DmShown.remote(code: 'code-a', endpoint: 'aa11'));
    await phone.e2ee.sync();
    await _show(tester, phone.e2ee);
    expect(
      find.textContaining('This Claktory is signed in to another account'),
      findsOneWidget,
    );
    expect(find.text('Keep them on this Claktory'), findsNothing);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('removing a device asks first', (tester) async {
    final worker = FakeWorker();
    final phone = FakeEndpoint(worker, 'matt', 'phone', name: 'iPhone');
    await phone.e2ee.start();
    await phone.e2ee.setUpFirst();
    final ipad = FakeEndpoint(worker, 'matt', 'ipad');
    await ipad.e2ee.start();
    final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
    await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
    await ipad.e2ee.takeHandoff();
    await phone.e2ee.sync();

    await _show(tester, phone.e2ee);
    await tester.tap(find.text('Remove…'));
    await _settle(tester);
    expect(find.text('Remove ipad?'), findsOneWidget);
    expect(find.textContaining('signed out of your Claktory'), findsOneWidget);
    expect(find.textContaining('Settings → Devices'), findsOneWidget);
    await tester.tap(find.text('Remove'));
    await _settle(tester);
    expect(worker.keys['matt']!['version'], 3);
    // And signed out of the account: the worker answers it no more.
    expect(worker.signedOut['matt'], {'ipad'});
    expect(find.text('Remove ipad?'), findsNothing);
    expect(find.text('Remove…'), findsNothing);
  });

  testWidgets('a device that signed out of the account says so, with '
      'Remove… (row 671)', (tester) async {
    final worker = FakeWorker();
    final phone = FakeEndpoint(worker, 'matt', 'phone', name: 'iPhone');
    await phone.e2ee.start();
    await phone.e2ee.setUpFirst();
    final ipad = FakeEndpoint(worker, 'matt', 'ipad', name: 'iPad');
    await ipad.e2ee.start();
    final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
    await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
    await ipad.e2ee.takeHandoff();
    await phone.e2ee.sync();
    worker.accountSignOuts['matt'] = {'ipad'};

    await _show(tester, phone.e2ee);
    expect(find.textContaining('Signed out'), findsOneWidget);
    expect(find.text('Remove…'), findsOneWidget);
    await tester.tap(find.text('Remove…'));
    await _settle(tester);
    await tester.tap(find.text('Remove'));
    await _settle(tester);
    expect(worker.keys['matt']!['version'], 3);
    expect(find.textContaining('Signed out'), findsNothing);
  });

  testWidgets("a sign-out that doesn't go through is said, and tried again", (
    tester,
  ) async {
    final worker = FakeWorker();
    final phone = await _first(worker);
    final ipad = FakeEndpoint(worker, 'matt', 'ipad');
    await ipad.e2ee.start();
    final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
    await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
    await ipad.e2ee.takeHandoff();
    await phone.e2ee.sync();

    worker.refuse = (route) => route == 'POST /v1/devices/remove'
        ? const AccountServiceUnreachable('offline')
        : null;
    await _show(tester, phone.e2ee);
    await tester.tap(find.text('Remove…'));
    await _settle(tester);
    await tester.tap(find.text('Remove'));
    await _settle(tester);
    expect(worker.keys['matt']!['version'], 3);
    expect(
      find.text(
        'ipad no longer gets new messages, but it may still be signed in to '
        'your Claktory account. Try again.',
      ),
      findsOneWidget,
    );
    expect(worker.signedOut['matt'], isNull);
    // Kept as under way: Your devices says so too (R5b).
    expect(find.text("ipad isn't fully removed yet."), findsOneWidget);

    // A refusal is said as one.
    worker.refuse = (route) => route == 'POST /v1/devices/remove'
        ? const AccountServiceError(429, 'rate_limited', 'Too many.')
        : null;
    // The dialog's (over Your devices').
    await tester.tap(find.text('Try again').last);
    await _settle(tester);
    expect(
      find.text(
        "ipad no longer gets new messages, but it couldn't be signed out of "
        'your Claktory account. Try again.',
      ),
      findsOneWidget,
    );

    worker.refuse = null;
    await tester.tap(find.text('Try again').last);
    await _settle(tester);
    expect(worker.signedOut['matt'], {'ipad'});
    // The list was changed once.
    expect(worker.keys['matt']!['version'], 3);
    expect(find.text('Remove ipad?'), findsNothing);
    expect(find.text("ipad isn't fully removed yet."), findsNothing);
    expect(find.text('Try again'), findsNothing);
  });

  testWidgets('a removal closed part-way is finished from Your devices: '
      'Try again on what was under way, Finish removing on a device left '
      'in a group', (tester) async {
    final worker = FakeWorker();
    final mac = await _first(worker);
    for (final leaf in ['ipad', 'laptop']) {
      final other = FakeEndpoint(worker, 'matt', leaf);
      await other.e2ee.start();
      final code = await other.e2ee.offerThisDevice(platform: 'Mac');
      await mac.e2ee.addDevice(await mac.e2ee.scanned(code!));
      await other.e2ee.takeHandoff();
      await other.e2ee.sync();
    }
    await mac.e2ee.sync();

    worker.refuse = (route) => route == 'POST /v1/devices/remove'
        ? const AccountServiceUnreachable('offline')
        : null;
    await _show(tester, mac.e2ee);
    await tester.tap(find.text('Remove…').first);
    await _settle(tester);
    await tester.tap(find.text('Remove'));
    await _settle(tester);
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
    expect(find.text("ipad isn't fully removed yet."), findsOneWidget);
    worker.refuse = null;
    await tester.tap(find.text('Try again'));
    await _settle(tester);
    expect(worker.signedOut['matt'], {'ipad'});
    expect(find.text("ipad isn't fully removed yet."), findsNothing);

    // The laptop, off the list by a build before the fix and nothing kept:
    // still in the self group, so not fully removed.
    final listed = mac.device.deviceList(account: 'matt')!.leaves;
    // Signed and taken (pinned once the worker has it).
    await mac.device.acceptList(
      account: 'matt',
      deviceList: await mac.device.signDeviceList(
        version: BigInt.from(9),
        createdMs: BigInt.one,
        leaves: [
          for (final each in listed)
            if (each.leafId != 'laptop') each,
        ],
      ),
    );
    mac.e2ee.dismissNewKey();
    await _settle(tester);
    expect(find.text('Finish removing'), findsOneWidget);
    expect(find.textContaining('laptop'), findsOneWidget);
    expect(find.textContaining('Not fully removed'), findsOneWidget);
    await tester.tap(find.text('Finish removing'));
    await _settle(tester);
    expect(find.text('Finish removing'), findsNothing);
    expect(worker.signedOut['matt'], {'ipad', 'laptop'});
  });

  group('lost or stolen: a new security key (R4)', () {
    Future<(FakeWorker, FakeEndpoint, FakeEndpoint, FakeEndpoint)>
    three() async {
      final worker = FakeWorker();
      final mac = await _first(worker);
      Future<FakeEndpoint> add(String leaf) async {
        final other = FakeEndpoint(worker, 'matt', leaf);
        await other.e2ee.start();
        final code = await other.e2ee.offerThisDevice(platform: leaf);
        await mac.e2ee.addDevice(await mac.e2ee.scanned(code!));
        await other.e2ee.takeHandoff();
        return other;
      }

      final ipad = await add('ipad');
      final phone = await add('phone');
      for (final each in [mac, ipad, phone]) {
        await each.e2ee.sync();
      }
      return (worker, mac, ipad, phone);
    }

    testWidgets('confirms the devices kept, signs the lost one out first, '
        'and the others are told where the new key came from', (tester) async {
      final (worker, mac, ipad, phone) = await three();
      final old = mac.device.identity;
      await _show(tester, mac.e2ee);
      await tester.tap(find.text('Lost or stolen…').first);
      await _settle(tester);
      expect(find.text('Lock out ipad?'), findsOneWidget);
      expect(
        find.textContaining('ipad is removed and signed out'),
        findsOneWidget,
      );
      expect(
        find.textContaining("change your email's password"),
        findsOneWidget,
      );
      expect(find.text('Devices to keep'), findsOneWidget);
      expect(
        find.descendant(
          of: find.byType(Pressable),
          matching: find.text('phone'),
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Lock out'));
      await _settle(tester);
      expect(find.text('Lock out ipad?'), findsNothing);
      // Signed out before the new key went to the worker.
      expect(
        worker.log.indexOf('POST /v1/devices/remove'),
        lessThan(worker.log.indexOf('POST me/keys/rotate')),
      );
      expect(worker.signedOut['matt'], {'ipad'});
      expect(mac.device.identity, isNot(old));
      expect(mac.device.lists['matt']!.leaves.map((each) => each.leafId), [
        'mac',
        'phone',
      ]);

      // The phone takes it, and says from where, once.
      await phone.e2ee.sync();
      await tester.pumpWidget(const SizedBox());
      await _show(tester, phone.e2ee);
      expect(find.text('Your security key changed'), findsOneWidget);
      expect(
        find.textContaining('made for your account on mac'),
        findsOneWidget,
      );
      await tester.tap(find.text('OK'));
      await _settle(tester);
      expect(find.text('Your security key changed'), findsNothing);
    });

    testWidgets("one unticked goes too; a sign-out that doesn't go through "
        'makes no key until tried again', (tester) async {
      final (worker, mac, _, _) = await three();
      final old = mac.device.identity;
      await _show(tester, mac.e2ee);
      await tester.tap(find.text('Lost or stolen…').first);
      await _settle(tester);
      await tester.tap(
        find.descendant(
          of: find.byType(Pressable),
          matching: find.text('phone'),
        ),
      );
      await _settle(tester);
      worker.refuse = (route) => route == 'POST /v1/devices/remove'
          ? const AccountServiceUnreachable('offline')
          : null;
      await tester.tap(find.text('Lock out'));
      await _settle(tester);
      expect(
        find.text(
          "ipad no longer gets new messages, but it couldn't be signed out "
          "of your Claktory account, so your security key hasn't changed "
          'yet. Try again.',
        ),
        findsOneWidget,
      );
      expect(worker.log, isNot(contains('POST me/keys/rotate')));
      expect(mac.device.identity, old);

      // Kept as under way: Your devices says so too (R5b), and what's
      // still true.
      expect(
        find.text(
          "ipad and phone can't read new messages, but Claktory couldn't "
          "sign them out of your Claktory account, so your security key "
          "hasn't changed yet. Try again.",
        ),
        findsOneWidget,
      );

      worker.refuse = null;
      // The dialog's (over Your devices').
      await tester.tap(find.text('Try again').last);
      await _settle(tester);
      expect(find.text('Lock out ipad?'), findsNothing);
      expect(find.textContaining('fully locked out'), findsNothing);
      expect(worker.signedOut['matt'], {'ipad', 'phone'});
      expect(mac.device.identity, isNot(old));
      expect(mac.device.lists['matt']!.leaves.map((each) => each.leafId), [
        'mac',
      ]);
    });

    // The worker's 3 new keys a day, met: said plainly, with
    // when it carries on by itself, and no Try again in Your devices.
    testWidgets("the worker refusing a new key for today says it isn't "
        'finished and when it carries on', (tester) async {
      final (worker, mac, _, _) = await three();
      await _show(tester, mac.e2ee);
      await tester.tap(find.text('Lost or stolen…').first);
      await _settle(tester);
      worker.refuse = (route) => route == 'POST me/keys/rotate'
          ? const AccountServiceError(429, 'rate_limited', 'Too many tries.')
          : null;
      await tester.tap(find.text('Lock out'));
      await _settle(tester);
      const words =
          "ipad can't read new messages, but your security key hasn't "
          'changed yet: it can change only 3 times a day. Claktory changes '
          'it ';
      // The dialog's, and Your devices' under it, which has no Try again:
      // it carries on by itself.
      expect(find.textContaining(words), findsNWidgets(2));
      expect(mac.e2ee.removalStop, DmKeyChange.keyWaitsForLimit);
      expect(find.text('Try again'), findsOneWidget);
    });

    // E11-6: the sign-in a new key wants,
    // cancelled, never stalls it silently: Your devices says what's still
    // true and asks to sign in again, after a relaunch too.
    testWidgets('the sign-in cancelled: said, with Sign in again…, which '
        'asks again; and the same after a relaunch', (tester) async {
      final (worker, mac, _, _) = await three();
      final old = mac.device.identity;
      worker.staleSignIn.add('matt/mac');
      await _show(tester, mac.e2ee);
      await tester.tap(find.text('Lost or stolen…').first);
      await _settle(tester);
      await tester.tap(find.text('Lock out'));
      await _settle(tester);
      await tester.tap(find.text('Cancel'));
      await _settle(tester);
      const line =
          "ipad can't read new messages, but your security key hasn't "
          'changed yet. Sign in again to finish locking it out.';
      expect(find.text(line), findsOneWidget);
      // Off the list and signed out already; only the key waits.
      expect(
        mac.device.lists['matt']!.leaves.map((each) => each.leafId),
        isNot(contains('ipad')),
      );
      expect(worker.signedOut['matt'], contains('ipad'));
      expect(mac.device.identity, old);

      await tester.tap(find.text('Sign in again…'));
      await _settle(tester);
      expect(find.text('Sign in again'), findsOneWidget);
      await tester.tap(find.text('Cancel').last);
      await _settle(tester);
      expect(find.text(line), findsOneWidget);
      await tester.pumpWidget(const SizedBox());

      final again = mac.relaunch();
      await again.start();
      await _show(tester, again);
      expect(find.text(line), findsOneWidget);
      expect(find.text('Sign in again…'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
    });
  });

  testWidgets("a key this device can't read offers to start over", (
    tester,
  ) async {
    final phone = FakeEndpoint(FakeWorker(), 'matt', 'phone');
    // Its store is here: the key was this install's own.
    phone.storeKept = true;
    phone.keys.unreadable.add(DmKeyName.leaf);
    await phone.e2ee.start();
    await _show(tester, phone.e2ee);
    expect(
      find.textContaining("This device can't read its security key"),
      findsOneWidget,
    );
    phone.keys.unreadable.clear();
    await tester.tap(find.text('Start over…'));
    await _settle(tester);
    await tester.tap(find.text('Start over'));
    await _settle(tester);
    expect(phone.e2ee.setup, DmSetup.firstDevice);
  });

  group('a pasted code', () {
    late _Clipboard clipboard;
    setUp(() => secretClipboard = clipboard = _Clipboard());
    tearDown(() => secretClipboard = const SecretClipboard());

    testWidgets('is copied, compared, and added once the numbers match', (
      tester,
    ) async {
      final worker = FakeWorker();
      final mac = FakeEndpoint(worker, 'matt', 'mac', name: 'Mac');
      await mac.e2ee.start();
      await mac.e2ee.setUpFirst();
      final studio = FakeEndpoint(worker, 'matt', 'studio', name: 'Studio');
      await studio.e2ee.start();

      // The new Mac copies its code and shows its numbers.
      await _show(tester, studio.e2ee);
      final digits = studio.e2ee.offerDigits!;
      expect(find.text(digits), findsOneWidget);
      await tester.tap(find.text('Copy code'));
      await _settle(tester);
      expect(clipboard.text, studio.e2ee.offer);
      expect(find.text('Copied'), findsOneWidget);

      final code = clipboard.text;
      // Closed, it's cleared.
      await tester.pumpWidget(const SizedBox());
      await _settle(tester);
      expect(clipboard.text, isNull);

      // The other Mac can't scan: it pastes (as Universal Clipboard brought
      // it over), and shows the same numbers.
      clipboard.text = code;
      await _show(tester, mac.e2ee);
      await tester.tap(find.text('Add a device…'));
      await _settle(tester);
      expect(
        find.textContaining('Copy the code on your new device and choose'),
        findsOneWidget,
      );
      await tester.tap(find.text('Paste code instead'));
      await _settle(tester);
      expect(find.text('Add this device to your messages?'), findsOneWidget);
      expect(find.text(digits), findsOneWidget);

      // Add waits for the person to say they match.
      await tester.tap(find.text('Add'));
      await _settle(tester);
      expect(worker.handoffs['studio'], isNull);
      await tester.tap(find.text('The numbers match'));
      await _settle(tester);
      await tester.tap(find.text('Add'));
      await _settle(tester);
      expect(worker.handoffs['studio'], hasLength(1));

      // Added, the new Mac leaves its code nowhere.
      await tester.pumpWidget(const SizedBox());
      await _show(tester, studio.e2ee);
      await tester.tap(find.text('Copy code'));
      await _settle(tester);
      expect(clipboard.text, code);
      await studio.e2ee.takeHandoff();
      await _settle(tester);
      expect(studio.e2ee.setup, DmSetup.ready);
      expect(clipboard.text, isNull);
    });

    testWidgets("what the person copied since isn't cleared", (tester) async {
      final worker = FakeWorker();
      final studio = FakeEndpoint(worker, 'matt', 'studio');
      await (await _first(worker)).e2ee.sync();
      await studio.e2ee.start();
      await _show(tester, studio.e2ee);
      await tester.tap(find.text('Copy code'));
      await _settle(tester);
      clipboard.personCopies('a note');
      await tester.pumpWidget(const SizedBox());
      await _settle(tester);
      expect(clipboard.text, 'a note');
    });

    testWidgets('after 30 minutes a new code, and the old one cleared', (
      tester,
    ) async {
      final worker = FakeWorker();
      await _first(worker);
      final studio = FakeEndpoint(worker, 'matt', 'studio');
      await studio.e2ee.start();
      await _show(tester, studio.e2ee);
      final before = studio.e2ee.offer;
      await tester.tap(find.text('Copy code'));
      await _settle(tester);
      expect(clipboard.text, before);
      await tester.pump(const Duration(minutes: 30));
      await _settle(tester);
      expect(studio.e2ee.offer, isNot(before));
      expect(clipboard.text, isNull);
      expect(find.text('Copy code'), findsOneWidget);
    });

    testWidgets("text that isn't a code is refused", (tester) async {
      final worker = FakeWorker();
      final mac = await _first(worker);
      clipboard.text = 'hello';
      await _show(tester, mac.e2ee);
      await tester.tap(find.text('Add a device…'));
      await _settle(tester);
      await tester.tap(find.text('Paste code instead'));
      await _settle(tester);
      expect(
        find.text(
          "That isn't a code from a new device. Copy it again from the new "
          "device's screen.",
        ),
        findsOneWidget,
      );
    });

    testWidgets('a code used already is refused, saying so (row 612)', (
      tester,
    ) async {
      final worker = FakeWorker();
      final mac = await _first(worker);
      final ipad = FakeEndpoint(worker, 'matt', 'ipad');
      await ipad.e2ee.start();
      final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
      await mac.e2ee.addDevice(await mac.e2ee.scanned(code!));
      clipboard.text = code;
      await _show(tester, mac.e2ee);
      await tester.tap(find.text('Add a device…'));
      await _settle(tester);
      await tester.tap(find.text('Paste code instead'));
      await _settle(tester);
      expect(
        find.text(
          'This code was already used, or is for a device you removed.',
        ),
        findsOneWidget,
      );
      expect(find.text('The numbers match'), findsNothing);
    });

    testWidgets('a device the worker says was removed is refused, saying so, '
        'not Try again (row 617)', (tester) async {
      final worker = FakeWorker();
      final mac = await _first(worker);
      final ipad = FakeEndpoint(worker, 'matt', 'ipad');
      await ipad.e2ee.start();
      final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
      // Removed from the account on another device this Mac hasn't heard.
      worker.refuse = (route) => route == 'PUT me/keys'
          ? AccountServiceError(
              409,
              'removed_device',
              "A device that was removed from your account can't come back "
                  'on your list.',
              const {},
            )
          : null;
      clipboard.text = code;
      await _show(tester, mac.e2ee);
      await tester.tap(find.text('Add a device…'));
      await _settle(tester);
      await tester.tap(find.text('Paste code instead'));
      await _settle(tester);
      await tester.tap(find.text('The numbers match'));
      await _settle(tester);
      int puts() => worker.log.where((r) => r == 'PUT me/keys').length;
      final before = puts();
      await tester.tap(find.text('Add'));
      await _settle(tester);
      expect(
        find.text(
          'This code was already used, or is for a device you removed.',
        ),
        findsOneWidget,
      );
      expect(find.text("Couldn't add it. Try again."), findsNothing);
      // Not tried again as if another device had changed the list.
      expect(puts(), before + 1);
    });

    testWidgets('a new device a chat is held back from says so under Your '
        'devices (row 611)', (tester) async {
      final worker = FakeWorker();
      final mac = await _first(worker);
      final ipad = FakeEndpoint(worker, 'matt', 'ipad');
      await ipad.e2ee.start();
      final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
      await mac.e2ee.addDevice(await mac.e2ee.scanned(code!));
      ipad.device.refuseRejoin = () => AnyhowException(
        "refused: matt's certificate is from a security key that was "
        'replaced',
      );
      await ipad.e2ee.takeHandoff();
      await _show(tester, ipad.e2ee);
      expect(
        find.text(
          "Some of your chats can't come to this device yet. Open Claktory "
          "on your other devices so they can update, and they'll come here "
          'on their own.',
        ),
        findsOneWidget,
      );
    });
  });

  group('the history coming over', () {
    // The iPad, added from the phone, today.
    Future<FakeEndpoint> added() async {
      final worker = FakeWorker();
      final phone = await _first(worker);
      final ipad = FakeEndpoint(worker, 'matt', 'ipad');
      ipad.now = DateTime.now().millisecondsSinceEpoch;
      await ipad.e2ee.start();
      final code = await ipad.e2ee.offerThisDevice(platform: 'iPad');
      await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
      await ipad.e2ee.takeHandoff();
      return ipad;
    }

    DmTransferProgress at(int done, int total, [DmTransferEnd? end]) =>
        DmTransferProgress(chatsDone: done, chatsTotal: total, end: end);

    testWidgets('shows how far it is, and Skip leaves it', (tester) async {
      final ipad = await added();
      await _show(tester, ipad.e2ee);
      expect(find.text('Bringing your messages over…'), findsOneWidget);
      ipad.transfers.single.progress.add(at(3, 12));
      await _settle(tester);
      expect(find.text('3 of 12 chats'), findsOneWidget);

      await tester.tap(find.text('Skip'));
      await _settle(tester);
      expect(ipad.transfers.single.closed, isTrue);
      expect(find.text('Bringing your messages over…'), findsNothing);
      expect(
        find.text('Messages from before today are on your other devices.'),
        findsOneWidget,
      );
      expect(find.text('Try again'), findsNothing);
      await tester.tap(find.text('OK'));
      await _settle(tester);
      expect(find.textContaining('Messages from before'), findsNothing);
    });

    testWidgets('cut short, it can be tried again', (tester) async {
      final ipad = await added();
      await _show(tester, ipad.e2ee);
      ipad.transfers.single.progress.add(at(5, 12, DmTransferEnd.cutShort));
      await _settle(tester);
      expect(find.text("Some messages didn't come over."), findsOneWidget);
      await tester.tap(find.text('Try again'));
      await _settle(tester);
      expect(ipad.transfers.single.receiving, 2);
      expect(find.text('Bringing your messages over…'), findsOneWidget);
      ipad.transfers.single.progress.add(at(12, 12, DmTransferEnd.complete));
      await _settle(tester);
      expect(find.text('Bringing your messages over…'), findsNothing);
      expect(find.text("Some messages didn't come over."), findsNothing);
    });
  });

  _yourDevicesOnAPhone();
}

void _yourDevicesOnAPhone() {
  group('Your devices on a phone (row 198)', () {
    const long = 'anas-mac-studio-in-the-office';
    const other = 'anas-mac-studio-at-home';

    /// The Mac with two others of long names, added on known days, shown
    /// on a phone at the largest text size.
    Future<FakeEndpoint> shown(WidgetTester tester) async {
      final worker = FakeWorker();
      final mac = await _first(worker);
      for (final leaf in [long, other]) {
        final device = FakeEndpoint(worker, 'matt', leaf);
        await device.e2ee.start();
        final code = await device.e2ee.offerThisDevice(platform: 'macOS');
        await mac.e2ee.addDevice(await mac.e2ee.scanned(code!));
        await device.e2ee.takeHandoff();
      }
      await mac.e2ee.sync();
      mac.device.added['matt/$long'] = DateTime(
        2026,
        11,
        16,
        14,
        2,
      ).millisecondsSinceEpoch;
      mac.device.added['matt/$other'] = DateTime(
        2026,
        11,
        21,
        9,
        30,
      ).millisecondsSinceEpoch;
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = Harness(Workspace());
      await harness.show(
        tester,
        size: const Size(375, 667),
        home: SingleChildScrollView(
          child: MessagesSetup(e2ee: mac.e2ee, devices: true),
        ),
      );
      await _settle(tester);
      return mac;
    }

    testWidgets('at the largest text each name shows whole, with since '
        'when, and its buttons fit under it', (tester) async {
      await shown(tester);
      for (final name in [long, other]) {
        final text = tester.widget<Text>(find.text(name));
        expect(text.maxLines, isNull, reason: 'wraps, never cut short');
        expect(tester.getRect(find.text(name)).width, greaterThan(0));
      }
      expect(find.textContaining('Since '), findsNWidgets(2));
      for (final label in ['Lost or stolen…', 'Remove…']) {
        for (final button in find.text(label).evaluate()) {
          final rect = tester.getRect(find.byWidget(button.widget).first);
          expect(rect.left, greaterThanOrEqualTo(0));
          expect(rect.right, lessThanOrEqualTo(375));
        }
      }
      // No overflow stripe anywhere (it would fail the test as it's laid
      // out), and Remove… opens its dialog, which says since when.
      await tester.ensureVisible(find.text('Remove…').first);
      await tester.pump();
      await tester.tap(find.text('Remove…').first);
      await _settle(tester);
      expect(find.textContaining('?'), findsWidgets);
      expect(find.textContaining('Since '), findsWidgets);
    });
  });

  testWidgets('two devices of one name read apart by the time they were '
      'added (row 198)', (tester) async {
    late BuildContext context;
    await tester.pumpWidget(
      Builder(
        builder: (built) {
          context = built;
          return const SizedBox();
        },
      ),
    );
    final day = DateTime(2026, 11, 16, 14, 2).millisecondsSinceEpoch;
    final first = (leafId: 'a', name: 'Mac Studio', here: false, sinceMs: day);
    final second = (
      leafId: 'b',
      name: 'Mac Studio',
      here: false,
      sinceMs: day + const Duration(hours: 3).inMilliseconds,
    );
    final mini = (leafId: 'c', name: 'Mac mini', here: false, sinceMs: day);
    final all = [first, second, mini];
    // The device's own clock, 24-hour or not.
    expect(
      deviceSince(context, first, all),
      allOf(startsWith('Since Mon 16 Nov'), contains(' at ')),
    );
    expect(
      deviceSince(context, second, all),
      allOf(startsWith('Since Mon 16 Nov'), contains(' at ')),
    );
    expect(
      deviceSince(context, first, all),
      isNot(deviceSince(context, second, all)),
    );
    expect(
      deviceSince(context, mini, all),
      allOf(startsWith('Since '), isNot(contains(' at '))),
    );
    // Not known: nothing.
    expect(
      deviceSince(
        context,
        (leafId: 'd', name: 'X', here: false, sinceMs: 0),
        [first],
      ),
      '',
    );
  });
}

Future<FakeEndpoint> _first(FakeWorker worker) async {
  final mac = FakeEndpoint(worker, 'matt', 'mac', name: 'Mac');
  await mac.e2ee.start();
  await mac.e2ee.setUpFirst();
  return mac;
}
