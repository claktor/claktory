import 'package:factory_app/connections.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

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
    await tester.ensureVisible(find.text('Devices'));
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

  testWidgets('adding a device shows its code once, to copy', (tester) async {
    final harness = await showSettings(tester, workspace());
    await tester.ensureVisible(find.text('Add a device…'));
    await tester.tap(find.text('Add a device…'));
    await pumpFrames(tester);
    expect(find.text('Add a device'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, 'Laptop');
    await tester.pump();
    await tester.tap(find.text('Show code'));
    await pumpFrames(tester);
    final request = harness.last.createPairing;
    expect(request.name, 'Laptop');
    expect(
      harness.backend.sent.where((m) => m.hasSetRemoteAccess()),
      isEmpty,
      reason: 'already on',
    );
    expect(find.text('Scan with your iPhone'), findsOneWidget);
    expect(
      find.text(harness.backend.pairingCode.code),
      findsNothing,
      reason: 'the code is the QR code; Copy code copies it',
    );
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
    await tester.tap(find.text('Copy code'));
    await tester.pump();
    expect(copied, [harness.backend.pairingCode.code]);
    await tester.tap(find.text('Done').last);
    await pumpFrames(tester);
    expect(find.text('Scan with your iPhone'), findsNothing);
  });

  testWidgets('the footer connects to another Factory from its code', (
    tester,
  ) async {
    final harness = Harness(workspace());
    await harness.show(tester);
    await pumpFrames(tester);
    await tester.tap(labeled('Claktories'));
    await pumpFrames(tester);
    expect(find.text('This Mac'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, 'nonsense');
    await tester.pump();
    expect(find.text("That isn't a Claktory code."), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, _code);
    await tester.pump();
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);

    final target = parsePairingCode(_code)!;
    expect(harness.connections.current, target);
    expect(harness.connectionStorage.kept.current, _code);
    expect(harness.backend.target, target);
    // The old Factory's state is gone until the new one's arrives.
    expect(harness.backend.workspace.workspace.agents, isEmpty);
    expect(find.textContaining('on mac mini'), findsOneWidget);

    // Refused: it says so, and stops trying.
    harness.backend.refused.value = 'unknown device';
    await pumpFrames(tester);
    expect(find.text('mac mini no longer allows this device'), findsOneWidget);

    // Back to this Mac's.
    await tester.tap(find.text('mac mini no longer allows this device'));
    await pumpFrames(tester);
    await tester.tap(find.text('This Mac'));
    await pumpFrames(tester);
    expect(harness.connections.current, isNull);
    expect(harness.backend.target, isNull);
    expect(harness.backend.refused.value, isNull);
    expect(harness.connections.saved, [target], reason: 'still known');
  });
}
