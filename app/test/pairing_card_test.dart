import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/devices.dart' show QrCode;
import 'package:factory_app/pairing_card.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

final _now = DateTime(2026, 11, 19, 9, 30);

const _code = 'factory://?t=0123456789abcdef&name=mac-mini&e=abc';

DevicePairing _pairing(
  DevicePairingState state, {
  int expiresIn = 9 * 60 + 42,
  int endedMs = 0,
  DeviceKind kind = DeviceKind.DEVICE_KIND_UNSPECIFIED,
}) => DevicePairing(
  id: 'p1',
  agentId: 'a1',
  deviceId: 'd9',
  name: 'Laptop',
  state: state,
  expiresMs: Int64(_now.millisecondsSinceEpoch + expiresIn * 1000),
  endedMs: Int64(endedMs),
  kind: kind,
);

/// Shows agent a1's conversation with the pairing card in it, the backend
/// answering ShowPairingCode with [_code].
Future<Harness> _show(WidgetTester tester, DevicePairing pairing) async {
  final harness = Harness(workspace(agents: [builder(working: false)]));
  harness.backend.shownCodes['p1'] = PairingCode(code: _code, deviceId: 'd9');
  harness.backend.transcript.load(
    Transcript(
      agentId: 'a1',
      entries: [Entry(seq: Int64(1), devicePairing: pairing)],
    ),
  );
  await harness.show(tester);
  await pumpFrames(tester);
  return harness;
}

Future<void> _becomes(
  WidgetTester tester,
  Harness harness,
  DevicePairing pairing,
) async {
  harness.backend.transcript.update(
    EntryUpdate(
      agentId: 'a1',
      entry: Entry(seq: Int64(1), devicePairing: pairing),
    ),
  );
  await pumpFrames(tester);
}

/// The card's primary button's label.
String? _primary(WidgetTester tester) => tester
    .widgetList<Button>(find.byType(Button))
    .where((button) => button.primary)
    .map((button) => button.label)
    .firstOrNull;

Iterable<ClientMessage> _asks(Harness harness) =>
    harness.backend.sent.where((m) => m.hasShowPairingCode());

void main() {
  setUp(() => pairingNow = () => _now);
  tearDown(() => pairingNow = DateTime.now);

  test('countdown', () {
    expect(countdown(const Duration(minutes: 9, seconds: 42)), '9:42');
    expect(countdown(const Duration(milliseconds: 61500)), '1:02');
    expect(countdown(const Duration(seconds: 5)), '0:05');
    expect(countdown(const Duration(seconds: -3)), '0:00');
  });

  testWidgets('a waiting card shows the code it asks for, counts down, and '
      'copies it', (tester) async {
    final harness = await _show(
      tester,
      _pairing(DevicePairingState.DEVICE_PAIRING_STATE_WAITING),
    );
    expect(find.text('Builder adds a device: Laptop'), findsOneWidget);
    expect(_asks(harness).single.showPairingCode.pairingId, 'p1');
    // The agent didn't say what it is: copying first, the QR code under it.
    expect(
      find.text(
        'In Claktory on Laptop, click Add a Claktory and paste this code.',
      ),
      findsOneWidget,
    );
    expect(find.byType(QrCode), findsOneWidget);
    expect(
      find.text('On a phone or tablet, open Claktory and scan it instead.'),
      findsOneWidget,
    );
    expect(_primary(tester), 'Copy code');
    expect(
      find.text('This code works only for Laptop. It expires in 9:42.'),
      findsOneWidget,
    );
    expect(find.text(_code), findsNothing, reason: 'the code is the QR');

    pairingNow = () => _now.add(const Duration(seconds: 1));
    addTearDown(() => pairingNow = () => _now);
    await tester.pump(const Duration(seconds: 1));
    expect(
      find.text('This code works only for Laptop. It expires in 9:41.'),
      findsOneWidget,
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
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    await tester.tap(find.text('Copy code'));
    await pumpFrames(tester);
    expect(copied, [_code]);
    expect(find.text('Copied'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('a phone or tablet leads with the QR code', (tester) async {
    for (final kind in [
      DeviceKind.DEVICE_KIND_PHONE,
      DeviceKind.DEVICE_KIND_TABLET,
    ]) {
      await _show(
        tester,
        _pairing(DevicePairingState.DEVICE_PAIRING_STATE_WAITING, kind: kind),
      );
      expect(
        find.text('Open Claktory on Laptop and scan this code.'),
        findsOneWidget,
      );
      expect(find.byType(QrCode), findsOneWidget);
      expect(
        find.text('Or copy the code and paste it on Laptop.'),
        findsOneWidget,
      );
      expect(_primary(tester), isNull, reason: 'Copy code is secondary');
      await tester.pumpWidget(const SizedBox());
    }
  });

  testWidgets("a computer can't scan: Copy code leads, and the QR code shows "
      'only when asked for', (tester) async {
    for (final kind in [
      DeviceKind.DEVICE_KIND_LAPTOP,
      DeviceKind.DEVICE_KIND_DESKTOP,
    ]) {
      await _show(
        tester,
        _pairing(DevicePairingState.DEVICE_PAIRING_STATE_WAITING, kind: kind),
      );
      expect(
        find.text(
          'In Claktory on Laptop, click Add a Claktory and paste this code.',
        ),
        findsOneWidget,
      );
      expect(find.byType(QrCode), findsNothing);
      expect(find.textContaining('scan'), findsNothing);
      expect(_primary(tester), 'Copy code');
      await tester.tap(find.text('Show QR code'));
      await pumpFrames(tester);
      expect(find.byType(QrCode), findsOneWidget);
      expect(find.text('Show QR code'), findsNothing);
      await tester.pumpWidget(const SizedBox());
    }
  });

  testWidgets('Cancel cancels its device; the card turns Cancelled when the '
      'backend says', (tester) async {
    final harness = await _show(
      tester,
      _pairing(DevicePairingState.DEVICE_PAIRING_STATE_WAITING),
    );
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(harness.last.cancelPairing.deviceId, 'd9');
    await _becomes(
      tester,
      harness,
      _pairing(
        DevicePairingState.DEVICE_PAIRING_STATE_CANCELLED,
        endedMs: DateTime(2026, 11, 19, 9, 41).millisecondsSinceEpoch,
      ),
    );
    expect(find.text('Copy code'), findsNothing);
    expect(find.textContaining('Cancelled.'), findsOneWidget);
    expect(find.textContaining('09:41'), findsOneWidget);
  });

  testWidgets('a device that used the code: the number to compare, and Allow', (
    tester,
  ) async {
    final harness = await _show(
      tester,
      _pairing(DevicePairingState.DEVICE_PAIRING_STATE_WAITING),
    );
    final asking = harness.backend.workspace.workspace.deepCopy()
      ..remote = RemoteAccess(
        pendingDevices: [
          PendingDevice(
            deviceId: 'd9',
            name: 'Laptop',
            kind: DeviceKind.DEVICE_KIND_LAPTOP,
            number: '0427',
          ),
        ],
      );
    await harness.apply(tester, asking);
    await _becomes(
      tester,
      harness,
      _pairing(DevicePairingState.DEVICE_PAIRING_STATE_ASKING),
    );
    expect(find.text('Copy code'), findsNothing);
    expect(
      find.text(
        'Laptop used the code. Allow it only if its screen shows the '
        'same number.',
      ),
      findsOneWidget,
    );
    expect(find.text('0427'), findsOneWidget);
    await tester.tap(find.text('Allow'));
    await pumpFrames(tester);
    expect(harness.last.approveDevice.deviceId, 'd9');

    await _becomes(
      tester,
      harness,
      _pairing(
        DevicePairingState.DEVICE_PAIRING_STATE_ADDED,
        endedMs: DateTime(2026, 11, 19, 9, 43).millisecondsSinceEpoch,
      ),
    );
    expect(find.textContaining('Laptop was added.'), findsOneWidget);
    expect(find.text('Allow'), findsNothing);
  });

  testWidgets("Don't allow turns the device away", (tester) async {
    final harness = await _show(
      tester,
      _pairing(DevicePairingState.DEVICE_PAIRING_STATE_ASKING),
    );
    expect(_asks(harness), isEmpty, reason: 'no code once used');
    await harness.apply(
      tester,
      harness.backend.workspace.workspace.deepCopy()
        ..remote = RemoteAccess(
          pendingDevices: [
            PendingDevice(deviceId: 'd9', name: 'Laptop', number: '0427'),
          ],
        ),
    );
    await tester.tap(find.text("Don't allow"));
    await pumpFrames(tester);
    expect(harness.last.denyDevice.deviceId, 'd9');
    await _becomes(
      tester,
      harness,
      _pairing(DevicePairingState.DEVICE_PAIRING_STATE_DENIED),
    );
    expect(find.text("You didn't allow Laptop."), findsOneWidget);
  });

  testWidgets('an expired code says so', (tester) async {
    final harness = await _show(
      tester,
      _pairing(DevicePairingState.DEVICE_PAIRING_STATE_EXPIRED, expiresIn: -60),
    );
    expect(_asks(harness), isEmpty);
    expect(
      find.text('The code expired before Laptop used it.'),
      findsOneWidget,
    );
  });

  testWidgets('a code no longer waiting shows nothing wrong; another failure '
      'shows why', (tester) async {
    final harness = Harness(workspace(agents: [builder(working: false)]));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            devicePairing: _pairing(
              DevicePairingState.DEVICE_PAIRING_STATE_WAITING,
            ),
          ),
        ],
      ),
    );
    await harness.show(tester);
    await pumpFrames(tester);
    expect(_asks(harness), hasLength(1));
    expect(
      find.text('This code works only for Laptop. It expires in 9:42.'),
      findsOneWidget,
      reason: 'not found: its new state is on its way',
    );

    // A backend that restarted has a new code: asked again on reconnecting.
    harness.backend.shownCodes['p1'] = PairingCode(code: _code);
    harness.backend.connected.value = false;
    harness.backend.connected.value = true;
    await pumpFrames(tester);
    expect(_asks(harness), hasLength(2));
    expect(find.text('Copied'), findsNothing);

    harness.backend.refuse = 'Not connected to the backend.';
    harness.backend.connected.value = false;
    harness.backend.connected.value = true;
    await pumpFrames(tester);
    expect(find.text('Not connected to the backend.'), findsOneWidget);
    expect(
      tester
          .widget<DevicePairingCard>(find.byType(DevicePairingCard))
          .pairing
          .id,
      'p1',
    );
    await tester.pumpWidget(const SizedBox());
  });
}
