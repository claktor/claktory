import 'package:factory_app/people/dm_keys.dart';
import 'package:factory_app/people/e2ee.dart';
import 'package:factory_app/people/messages_setup.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
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
    final code = await ipad.e2ee.offerThisDevice(
      platform: 'iPad',
      endpointId: [0],
    );
    await phone.e2ee.addDevice(await phone.e2ee.scanned(code!));
    await ipad.e2ee.takeHandoff();
    await phone.e2ee.sync();

    await _show(tester, phone.e2ee);
    await tester.tap(find.text('Remove…'));
    await _settle(tester);
    expect(find.text('Remove ipad from your messages?'), findsOneWidget);
    await tester.tap(find.text('Remove'));
    await _settle(tester);
    expect(worker.keys['matt']!['version'], 3);
    expect(find.text('Remove…'), findsNothing);
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
}

Future<FakeEndpoint> _first(FakeWorker worker) async {
  final mac = FakeEndpoint(worker, 'matt', 'mac', name: 'Mac');
  await mac.e2ee.start();
  await mac.e2ee.setUpFirst();
  return mac;
}
