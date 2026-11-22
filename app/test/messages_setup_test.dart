import 'package:factory_app/controls.dart' show Pressable;
import 'package:factory_app/device_account.dart';
import 'package:factory_app/people/dm_keys.dart';
import 'package:factory_app/people/e2ee.dart';
import 'package:factory_app/people/messages_setup.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
import 'package:factory_app/tunnel/api/dm_transfer.dart';
import 'package:factory_app/people/secret_clipboard.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'dm_fakes.dart';
import 'harness.dart';

// People's messages on this device (E5, Trish's words): setting up the
// first device, a new one's code, adding it by scanning, and removing one.

Future<Harness> _show(WidgetTester tester, E2ee e2ee) async {
  final harness = Harness(Workspace());
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
  Future<int?> copy(String text) async {
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
        "can't scan. Copy the code on your new device and choose Paste code "
        'instead.',
      ),
      findsOneWidget,
    );
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
    await mac.device.signDeviceList(
      version: BigInt.from(9),
      createdMs: BigInt.one,
      leaves: [
        for (final each in listed)
          if (each.leafId != 'laptop') each,
      ],
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
      // Fugo: signed out before the new key went to the worker.
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

      // Kept as under way: Your devices says so too (R5b).
      expect(
        find.text("ipad and phone aren't fully locked out yet."),
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
  });

  testWidgets("a key this device can't read offers to start over", (
    tester,
  ) async {
    final phone = FakeEndpoint(FakeWorker(), 'matt', 'phone');
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
        find.textContaining("can't scan. Copy the code on your new device"),
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
}

Future<FakeEndpoint> _first(FakeWorker worker) async {
  final mac = FakeEndpoint(worker, 'matt', 'mac', name: 'Mac');
  await mac.e2ee.start();
  await mac.e2ee.setUpFirst();
  return mac;
}
