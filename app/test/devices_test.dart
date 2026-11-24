import 'dart:async';

import 'package:factory_app/agent_view.dart' show AgentView;
import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/controls.dart' show Button, showModal;
import 'package:factory_app/devices.dart'
    show
        FactoryChooser,
        QrCode,
        androidAppUrl,
        iosAppUrl,
        openAppPage,
        pairingQrText;
import 'package:factory_app/links.dart' show pairingFingerprint;
import 'package:factory_app/pairing.dart' show DeviceNumberField;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart' show CameraAccess;
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:url_launcher/url_launcher.dart' show launchUrl;

import 'harness.dart';

final _token = 'a' * 64;
final _fingerprint = 'B' * 64;
final _code =
    'factory://100.64.0.2:7443?t=$_token&fp=$_fingerprint&name=mac%20mini';

Workspace workspace({bool on = true, List<PairedDevice> devices = const []}) =>
    Workspace(
      agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
      selectedAgentId: 'rivet',
      version: '2026.11.20-0900',
      remote: RemoteAccess(
        on: on,
        port: on ? 7443 : 0,
        fingerprint: on ? 'f' * 64 : '',
        machine: 'mac-mini',
        addresses: ['100.64.0.2', 'mac-mini.local', '192.168.1.20'],
        devices: devices,
      ),
    );

PairedDevice device(
  String id,
  String name, {
  bool connected = false,
  int lastSeen = 0,
  String address = '',
}) => PairedDevice(
  id: id,
  name: name,
  pairedMs: Int64(DateTime(2026, 11, 12).millisecondsSinceEpoch),
  lastSeenMs: Int64(lastSeen),
  connected: connected,
  lastAddress: address,
);

Finder labeled(String label) => find.byWidgetPredicate(
  (w) => w is Semantics && w.properties.label == label,
);

void main() {
  _claktoryCodes();
  _scanInChooser();

  group('pairing codes', () {
    test('read host, port, key, pin and name', () {
      final target = parsePairingCode('  $_code\n')!;
      expect(target.host, '100.64.0.2');
      expect(target.port, 7443);
      expect(target.token, _token);
      expect(target.fingerprint, 'b' * 64, reason: 'compared in lowercase');
      expect(target.name, 'mac mini');
      expect(target.endpoint.toString(), 'wss://100.64.0.2:7443');
    });

    test('an IPv6 host keeps its brackets in the address', () {
      final target = parsePairingCode(
        'factory://[fd7a:115c::1]:7443?t=$_token&fp=$_fingerprint',
      )!;
      expect(target.host, 'fd7a:115c::1');
      expect(target.endpoint.toString(), 'wss://[fd7a:115c::1]:7443');
      expect(target.label, 'fd7a:115c::1', reason: 'no name: its address');
    });

    test('anything else is not a code', () {
      for (final text in [
        '',
        'hello',
        'https://100.64.0.2:7443?t=$_token&fp=$_fingerprint',
        'factory://100.64.0.2?t=$_token&fp=$_fingerprint',
        'factory://100.64.0.2:7443?t=short&fp=$_fingerprint',
        'factory://100.64.0.2:7443?t=$_token',
      ]) {
        expect(parsePairingCode(text), isNull, reason: text);
      }
    });
  });

  test('the device keeps which Factory shows, and the ones it knows', () async {
    final storage = MemoryConnectionStorage();
    final connections = Connections(storage);
    final target = parsePairingCode(_code)!;
    connections.use(target);
    await Future<void>.delayed(Duration.zero);
    expect(storage.kept.current, _code);
    expect(storage.kept.saved, [_code]);

    final restarted = await Connections.load(storage);
    expect(restarted.current, target);
    restarted.use(null);
    expect(restarted.current, isNull);
    expect(restarted.saved, [target], reason: 'still known');
    restarted.forget(target);
    expect(restarted.saved, isEmpty);

    // Unreadable codes are left out.
    storage.kept = const StoredConnections(current: 'junk', saved: ['junk']);
    final cleaned = await Connections.load(storage);
    expect(cleaned.current, isNull);
    expect(cleaned.saved, isEmpty);
  });

  Future<Harness> showSettings(WidgetTester tester, Workspace snapshot) async {
    final harness = Harness(snapshot);
    harness.page.value = AppPage.settings;
    await harness.show(tester);
    await pumpFrames(tester);
    // Settings builds its sections as they scroll into view.
    await tester.scrollUntilVisible(
      find.text('Devices'),
      200,
      scrollable: find
          .descendant(
            of: find.byType(SettingsView),
            matching: find.byType(Scrollable),
          )
          .first,
    );
    await pumpFrames(tester);
    return harness;
  }

  testWidgets('with no devices, adding one turns access on first', (
    tester,
  ) async {
    final harness = await showSettings(tester, workspace(on: false));
    expect(find.text('No devices yet.'), findsOneWidget);
    for (final gone in ['Remote access', 'Port', 'Listen on', 'Status']) {
      expect(find.text(gone), findsNothing, reason: gone);
    }
    await tester.ensureVisible(find.text('Add a device…'));
    await tester.tap(find.text('Add a device…'));
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, 'iPhone');
    await tester.pump();
    await tester.tap(find.text('Show code'));
    await pumpFrames(tester);
    expect(
      harness.backend.sent
          .where((m) => m.hasSetRemoteAccess() || m.hasCreatePairing())
          .map((m) => m.whichBody()),
      [ClientMessage_Body.createPairing],
      reason: 'a code waiting turns remote access on by itself',
    );
    expect(harness.last.createPairing.host, '', reason: 'no address to pick');
  });

  testWidgets('devices: this one marked, removing it confirmed', (
    tester,
  ) async {
    final now = DateTime.now().millisecondsSinceEpoch;
    final harness = await showSettings(
      tester,
      workspace(
        devices: [
          device('d1', 'Laptop', connected: true, address: '100.64.0.7'),
          device('d2', 'iPad', lastSeen: now - 5 * 60 * 1000),
          device('d3', 'Old laptop'),
        ],
      ),
    );
    harness.backend.deviceId = 'd1';
    await harness.apply(
      tester,
      workspace(
        devices: [
          device('d1', 'Laptop', connected: true, address: '100.64.0.7'),
          device('d2', 'iPad', lastSeen: now - 5 * 60 * 1000),
          device('d3', 'Old laptop'),
        ],
      ),
    );
    await pumpFrames(tester);
    expect(find.textContaining('This device'), findsOneWidget);
    expect(find.text('Connected now · Added 12 Nov'), findsOneWidget);
    expect(
      find.text('Last connected 5 min ago · Added 12 Nov'),
      findsOneWidget,
    );
    expect(find.text('Never connected · Added 12 Nov'), findsOneWidget);

    // Another device goes at once.
    await tester.ensureVisible(find.text('Remove').at(1));
    await tester.tap(find.text('Remove').at(1));
    await pumpFrames(tester);
    expect(harness.last.unpairDevice.deviceId, 'd2');

    // This one asks first.
    final sent = harness.backend.sent.length;
    await tester.tap(find.text('Remove').first);
    await pumpFrames(tester);
    expect(find.text('Remove this device?'), findsOneWidget);
    expect(harness.backend.sent.length, sent);
    await tester.tap(find.text('Remove').last);
    await pumpFrames(tester);
    expect(harness.last.unpairDevice.deviceId, 'd1');
  });

  testWidgets('adding a phone shows its code once, as a QR code first, '
      'to copy too', (tester) async {
    final harness = await showSettings(tester, workspace());
    await tester.ensureVisible(find.text('Add a device…'));
    await tester.tap(find.text('Add a device…'));
    await pumpFrames(tester);
    expect(find.text('Add a device'), findsOneWidget);
    // A phone or tablet unless the person says otherwise.
    expect(find.text('Phone or tablet'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, 'iPhone');
    await tester.pump();
    await tester.tap(find.text('Show code'));
    await pumpFrames(tester);
    final request = harness.last.createPairing;
    expect(request.name, 'iPhone');
    expect(
      harness.backend.sent.where((m) => m.hasSetRemoteAccess()),
      isEmpty,
      reason: 'already on',
    );
    expect(find.text('Connect iPhone'), findsOneWidget);
    expect(
      find.text('Open Claktory on iPhone and scan this code.'),
      findsOneWidget,
    );
    expect(find.byType(QrCode), findsOneWidget);
    // Where to get the app, above the code (#352 P6).
    final opened = <Uri>[];
    openAppPage = (url) async {
      opened.add(url);
      return true;
    };
    addTearDown(() => openAppPage = launchUrl);
    expect(find.text("Don't have the app?"), findsOneWidget);
    expect(
      tester.getTopLeft(find.text("Don't have the app?")).dy,
      lessThan(tester.getTopLeft(find.byType(QrCode)).dy),
    );
    await tester.tap(find.text('iPhone and iPad'));
    await tester.tap(find.text('Android'));
    expect(opened, [iosAppUrl, androidAppUrl]);
    expect(_primaryButton(tester), isNull, reason: 'Copy code is secondary');
    expect(
      find.text(harness.backend.pairingCode.code),
      findsNothing,
      reason: 'the code is the QR code; Copy code copies it',
    );
    final copied = _clipboard(tester);
    await tester.tap(find.text('Copy code'));
    await tester.pump();
    expect(copied, [harness.backend.pairingCode.code]);
    // Closing ends the code: it lasts only while it's shown.
    await tester.tap(find.text('Cancel').last);
    await pumpFrames(tester);
    expect(find.text('Connect iPhone'), findsNothing);
    expect(harness.last.cancelPairing.deviceId, 'd9');
  });

  testWidgets("adding a computer, which can't scan, leads with Copy code; the "
      'QR code shows only when asked for', (tester) async {
    final harness = await showSettings(tester, workspace());
    await tester.ensureVisible(find.text('Add a device…'));
    await tester.tap(find.text('Add a device…'));
    await pumpFrames(tester);
    await tester.tap(find.text('Phone or tablet'));
    await pumpFrames(tester);
    await tester.tap(find.text('Computer').last);
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, 'Laptop');
    await tester.pump();
    await tester.tap(find.text('Show code'));
    await pumpFrames(tester);
    expect(harness.last.createPairing.name, 'Laptop');
    expect(find.text('Connect Laptop'), findsOneWidget);
    expect(
      find.text(
        'In Claktory on Laptop, click Add a Claktory and paste this code.',
      ),
      findsOneWidget,
    );
    expect(find.byType(QrCode), findsNothing);
    // A computer gets the Mac app (#352 P6).
    expect(find.text("Don't have the app?"), findsOneWidget);
    expect(find.text('Get it for Mac'), findsOneWidget);
    expect(find.text('Android'), findsNothing);
    expect(_primaryButton(tester), 'Copy code');
    final copied = _clipboard(tester);
    await tester.tap(find.text('Copy code'));
    await tester.pump();
    expect(copied, [harness.backend.pairingCode.code]);
    expect(find.text('Copied'), findsOneWidget);
    await tester.tap(find.text('Show QR code'));
    await pumpFrames(tester);
    expect(find.byType(QrCode), findsOneWidget);
    await tester.tap(find.text('Cancel').last);
    await pumpFrames(tester);
    expect(harness.last.cancelPairing.deviceId, 'd9');
  });

  testWidgets("a code with a link puts the link in its QR code, so a "
      "phone's camera opens Claktory or its store; Copy code copies the "
      'code (#352 P6)', (tester) async {
    final harness = await showSettings(tester, workspace());
    const payload = 't=abc&name=mac-mini&e=ff';
    harness.backend.pairingCode = PairingCode(
      code: 'factory://?$payload',
      link: 'https://claktory.com/pair#$payload',
      deviceId: 'd9',
    );
    await tester.ensureVisible(find.text('Add a device…'));
    await tester.tap(find.text('Add a device…'));
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, 'iPhone');
    await tester.pump();
    await tester.tap(find.text('Show code'));
    await pumpFrames(tester);
    expect(
      tester.widget<QrCode>(find.byType(QrCode)).text,
      'https://claktory.com/pair#$payload',
    );
    final copied = _clipboard(tester);
    await tester.tap(find.text('Copy code'));
    await tester.pump();
    expect(copied, ['factory://?$payload']);

    // A link that isn't exactly the code, or none: the code itself.
    expect(
      pairingQrText('factory://?$payload', 'https://claktory.com/pair#t=x'),
      'factory://?$payload',
    );
    expect(
      pairingQrText(
        'factory://?$payload',
        'https://evil.example/pair#$payload',
      ),
      'factory://?$payload',
    );
    expect(pairingQrText('factory://?$payload', ''), 'factory://?$payload');
  });

  Future<Harness> showCode(WidgetTester tester) async {
    final harness = await showSettings(tester, workspace());
    await tester.ensureVisible(find.text('Add a device…'));
    await tester.tap(find.text('Add a device…'));
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, 'iPhone');
    await tester.pump();
    await tester.tap(find.text('Show code'));
    await pumpFrames(tester);
    return harness;
  }

  Future<void> deviceUsesCode(WidgetTester tester, Harness harness) =>
      harness.apply(
        tester,
        harness.backend.workspace.workspace.deepCopy()
          ..remote.pendingDevices.add(
            PendingDevice(
              deviceId: 'd9',
              name: 'iPhone',
              kind: DeviceKind.DEVICE_KIND_PHONE,
              number: '0427',
            ),
          ),
      );

  testWidgets('a device that used the code waits to be allowed, with the '
      'number its screen shows', (tester) async {
    final harness = await showCode(tester);
    await deviceUsesCode(tester, harness);
    await pumpFrames(tester);
    expect(find.text('Allow iPhone to connect?'), findsOneWidget);
    // Typed off its screen, never shown here (#352 P4).
    expect(
      find.text('To allow it, type the number its screen shows.'),
      findsOneWidget,
    );
    expect(find.textContaining('0427'), findsNothing);
    final allow = find.widgetWithText(Button, 'Allow');
    expect(tester.widget<Button>(allow).onPressed, isNull);
    // A wrong number: the backend's words, and the form stays.
    harness.backend.refuse = "That's not the number iPhone shows. 1 try left.";
    await tester.enterText(_numberField, '1234');
    await pumpFrames(tester);
    await tester.tap(allow);
    await pumpFrames(tester);
    expect(
      find.text("That's not the number iPhone shows. 1 try left."),
      findsOneWidget,
    );
    expect(find.text('Allow iPhone to connect?'), findsOneWidget);
    harness.backend.refuse = null;
    await tester.enterText(_numberField, '0427');
    await pumpFrames(tester);
    await tester.tap(allow);
    await pumpFrames(tester);
    expect(harness.last.approveDevice.deviceId, 'd9');
    expect(harness.last.approveDevice.number, '0427');
    expect(find.text('Allow iPhone to connect?'), findsNothing);
    // Let in: nothing to cancel.
    expect(harness.backend.sent.where((m) => m.hasCancelPairing()), isEmpty);
  });

  testWidgets("Don't allow turns it away", (tester) async {
    final harness = await showCode(tester);
    await deviceUsesCode(tester, harness);
    await pumpFrames(tester);
    await tester.tap(find.text("Don't allow"));
    await pumpFrames(tester);
    expect(
      harness.backend.sent
          .where((m) => m.hasDenyDevice())
          .single
          .denyDevice
          .deviceId,
      'd9',
    );
    expect(find.text('Allow iPhone to connect?'), findsNothing);
  });

  testWidgets('a device that stopped waiting says so', (tester) async {
    final harness = await showCode(tester);
    await deviceUsesCode(tester, harness);
    await pumpFrames(tester);
    harness.backend.onSend = (message) {
      if (message.hasApproveDevice()) {
        throw const RequestFailure(
          'No device is waiting.',
          code: ErrorCode.ERROR_CODE_NOT_FOUND,
        );
      }
    };
    await tester.enterText(_numberField, '0427');
    await pumpFrames(tester);
    await tester.tap(find.text('Allow'));
    await pumpFrames(tester);
    expect(
      find.text('iPhone stopped waiting. To add it, make a new code.'),
      findsOneWidget,
    );
  });

  testWidgets("the first device's code comes even though turning access on "
      'drops the connection', (tester) async {
    final harness = await showSettings(tester, workspace(on: false));
    final now = DateTime.now().millisecondsSinceEpoch;
    var first = true;
    harness.backend.onSend = (message) {
      if (!message.hasCreatePairing() || !first) return;
      first = false;
      // The backend made the device, then restarted the tunnel this app
      // reaches it through: the answer is lost.
      final next = harness.backend.workspace.workspace.deepCopy();
      next.remote.on = true;
      next.remote.devices.add(
        PairedDevice(
          id: 'iphone',
          name: 'iPhone',
          pairedMs: Int64(now),
          codeExpiresMs: Int64(now + 600000),
        ),
      );
      harness.backend.reconnecting.value = true;
      harness.backend.connected.value = false;
      Future<void>.delayed(const Duration(seconds: 3), () {
        harness.backend.connected.value = true;
        harness.backend.reconnecting.value = false;
        harness.backend.workspace.apply(next);
      });
      throw const RequestFailure('Connection lost.');
    };
    await tester.ensureVisible(find.text('Add a device…'));
    await tester.tap(find.text('Add a device…'));
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, 'iPhone');
    await tester.pump();
    await tester.tap(find.text('Show code'));
    await tester.pump(const Duration(seconds: 1));
    expect(
      find.text('Preparing the code… This takes a few seconds the first time.'),
      findsOneWidget,
    );
    expect(find.text('Connection lost.'), findsNothing);
    // Back after 3 s; its state, at the latest 3 s after that.
    await tester.pump(const Duration(seconds: 3));
    await tester.pump(const Duration(seconds: 3));
    await pumpFrames(tester);
    expect(
      harness.backend.sent
          .where((m) => m.hasCreatePairing() || m.hasUnpairDevice())
          .map((m) => m.whichBody()),
      [
        ClientMessage_Body.createPairing,
        ClientMessage_Body.unpairDevice,
        ClientMessage_Body.createPairing,
      ],
      reason: 'the device the lost answer made is taken back',
    );
    expect(harness.last.createPairing.name, 'iPhone');
    expect(find.text('Connect iPhone'), findsOneWidget);
  });

  testWidgets("a refusal while connected shows as it is", (tester) async {
    final harness = await showSettings(tester, workspace());
    harness.backend.refuse = 'The private tunnel isn\'t running.';
    await tester.ensureVisible(find.text('Add a device…'));
    await tester.tap(find.text('Add a device…'));
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, 'iPhone');
    await tester.pump();
    await tester.tap(find.text('Show code'));
    await pumpFrames(tester);
    expect(find.text("The private tunnel isn't running."), findsOneWidget);
    expect(
      harness.backend.sent.where((m) => m.hasCreatePairing()),
      hasLength(1),
    );
  });

  testWidgets('the chooser connects to another Factory from its code', (
    tester,
  ) async {
    final harness = Harness(workspace());
    await harness.show(tester);
    await pumpFrames(tester);
    // The version line isn't a way in (the rail's + is, rail_test).
    expect(labeled('Claktories'), findsNothing);
    unawaited(
      showModal(
        tester.element(find.byType(Sidebar).first),
        (context, close) => FactoryChooser(close: close),
      ),
    );
    await pumpFrames(tester);
    expect(find.text('This Mac'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, 'nonsense');
    await tester.pump();
    expect(find.text("That isn't a Claktory code."), findsOneWidget);
    // Another of Claktory's codes says what it is (friend bug #2).
    await tester.enterText(
      find.byType(EditableText).last,
      'claktory-pipe://?i=abc',
    );
    await tester.pump();
    expect(
      find.textContaining("That's a pipe invite, not a code to connect this "),
      findsOneWidget,
    );
    await tester.enterText(find.byType(EditableText).last, _code);
    await tester.pump();
    // Asked first, as every way a code comes in is (Fugo): its
    // fingerprint, and Cancel leaves everything as it was.
    final target = parsePairingCode(_code)!;
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(
      find.textContaining(pairingFingerprint(target), findRichText: true),
      findsOneWidget,
    );
    await tester.tap(find.text('Cancel').last);
    await pumpFrames(tester);
    expect(find.text('Connect to this Claktory?'), findsNothing);
    expect(harness.connections.current, isNull);
    expect(harness.backend.target, isNot(target));
    // Return in the field asks too; then Connect connects.
    await tester.showKeyboard(find.byType(EditableText).last);
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await pumpFrames(tester);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    await tester.tap(find.text('Connect').last);
    await pumpFrames(tester);

    expect(harness.connections.current, target);
    expect(harness.connectionStorage.kept.current, _code);
    expect(harness.backend.target, target);
    // The old Factory's state is gone until the new one's arrives.
    expect(harness.backend.workspace.workspace.agents, isEmpty);
    // The footer names it.
    expect(find.textContaining('to mac mini'), findsOneWidget);

    // Refused: it says so, and stops trying.
    harness.backend.refused.value = 'unknown device';
    await pumpFrames(tester);
    expect(find.text("mac mini doesn't allow this device"), findsOneWidget);

    // Back to this Mac's.
    await tester.tap(find.text("mac mini doesn't allow this device"));
    await pumpFrames(tester);
    await tester.tap(find.text('This Mac'));
    await pumpFrames(tester);
    expect(harness.connections.current, isNull);
    expect(harness.backend.target, isNull);
    expect(harness.backend.refused.value, isNull);
    expect(harness.connections.saved, [target], reason: 'still known');
  });
}

/// Friend bug #2: on a phone with a camera, Connect to another Claktory
/// offers Scan code; a code read there asks first, as a pasted one does.
void _scanInChooser() {
  testWidgets('on a phone the chooser scans a code, asking first', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final code = 'factory://?t=${'b' * 64}&name=mac-mini&e=${'d' * 64}';
    final harness = Harness(workspace());
    harness.camera.state = CameraAccess.granted;
    await harness.show(tester, size: const Size(390, 844));
    await pumpFrames(tester);
    unawaited(
      showModal(
        tester.element(find.byType(AgentView).first),
        (context, close) => FactoryChooser(close: close),
      ),
    );
    await pumpFrames(tester);
    expect(
      find.textContaining('Scan or paste the code', findRichText: true),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Scan code'));
    await pumpFrames(tester);
    await tester.tap(find.text('Scan code'));
    await pumpFrames(tester);
    // Over the chooser's sheet, the camera.
    expect(find.byKey(const ValueKey('camera')), findsOneWidget);
    expect(
      tester.getRect(find.byKey(const ValueKey('camera'))).top,
      lessThan(844),
    );
    harness.camera.scan('WIFI:S:home;;');
    await pumpFrames(tester);
    expect(find.textContaining("That's not a Claktory code"), findsOneWidget);
    harness.camera.scan(code);
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('camera')), findsNothing);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(harness.connections.current, isNull);
    await tester.tap(find.text('Connect').last);
    await pumpFrames(tester);
    expect(harness.connections.current, parsePairingCode(code));
  });

  testWidgets('without a camera the chooser offers only Paste', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace());
    harness.camera.state = CameraAccess.restricted;
    await harness.show(tester, size: const Size(390, 844));
    await pumpFrames(tester);
    unawaited(
      showModal(
        tester.element(find.byType(AgentView).first),
        (context, close) => FactoryChooser(close: close),
      ),
    );
    await pumpFrames(tester);
    expect(find.text('Scan code'), findsNothing);
    expect(
      find.textContaining('Paste the code from', findRichText: true),
      findsOneWidget,
    );
  });
}

/// The chooser connects to a claktory:// code as to a factory:// one, asking
/// first.
void _claktoryCodes() {
  testWidgets('the chooser takes a claktory:// code, asking first', (
    tester,
  ) async {
    final code = _code.replaceFirst('factory://', 'claktory://');
    final harness = Harness(workspace());
    await harness.show(tester);
    await pumpFrames(tester);
    unawaited(
      showModal(
        tester.element(find.byType(Sidebar).first),
        (context, close) => FactoryChooser(close: close),
      ),
    );
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, code);
    await tester.pump();
    expect(find.text("That isn't a Claktory code."), findsNothing);
    // A quick double click asks once (Abbacchio).
    await tester.tap(find.text('Connect'));
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    final target = parsePairingCode(code)!;
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(
      find.textContaining(pairingFingerprint(target), findRichText: true),
      findsOneWidget,
    );
    expect(harness.connections.current, isNull);
    await tester.tap(find.text('Connect').last);
    await pumpFrames(tester);
    expect(harness.connections.current, target);
    expect(harness.connectionStorage.kept.current, code);
    expect(harness.backend.target, target);
  });
}

/// The primary button's label, if one shows.
String? _primaryButton(WidgetTester tester) => tester
    .widgetList<Button>(find.byType(Button))
    .where((button) => button.primary)
    .map((button) => button.label)
    .firstOrNull;

/// What's copied from now on.
List<String> _clipboard(WidgetTester tester) {
  final copied = <String>[];
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
    SystemChannels.platform,
    (call) async {
      if (call.method == 'Clipboard.setData') {
        copied.add((call.arguments as Map)['text'] as String);
      }
      return null;
    },
  );
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      null,
    ),
  );
  return copied;
}

/// The field a waiting device's number is typed in.
Finder get _numberField => find.descendant(
  of: find.byType(DeviceNumberField),
  matching: find.byType(EditableText),
);
