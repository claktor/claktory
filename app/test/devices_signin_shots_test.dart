// Area 2 drawn as it looks, for her to
// check against her mock-ups 01 and 02: Add a Claktory's states on a Mac,
// the iPad's code state, and the problem pages on a Mac and an iPhone.
// With SHOTS set, each is drawn to a PNG there:
//   SHOTS=/tmp/area2 flutter test test/devices_signin_shots_test.dart
// Without it, each still lays out without an overflow or an exception.
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/connections.dart'
    show RemoteFactory, parsePairingCode;
import 'package:factory_app/controls.dart' show showModal;
import 'package:factory_app/device_account.dart';
import 'package:factory_app/devices.dart' show FactoryChooser;
import 'package:factory_app/mobile/local_network.dart';
import 'package:factory_app/pairing.dart' show PairingWait;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart' show CameraAccess;
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:factory_app/tunnel/api/tunnel.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'device_confirm_test.dart' show FakeAccountApi, home, signIn;
import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;
import 'tunnel_test.dart' show tunnelStatus;

Future<void> _shot(WidgetTester tester, String name) async {
  expect(tester.takeException(), isNull, reason: name);
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

final _nixos = 'factory://?t=${'b' * 64}&name=nixos&e=${'d' * 64}';

const _server = AccountClaktory(
  id: 'c2',
  name: 'Hetzner box',
  color: 'purple',
  endpointId: 'e2e2e2e2e2e2e2e2',
  relays: ['https://relay.example.com'],
);

class _Network implements LocalNetwork {
  @override
  Future<LocalNetworkAccess> check() async => LocalNetworkAccess.granted;

  @override
  Future<void> openSettings() async {}
}

void main() {
  setUpAll(loadGeist);

  testWidgets('Add a Claktory on a Mac', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final api = FakeAccountApi()
      ..claktoryList = [
        AccountClaktory(
          id: home.id,
          name: 'MacBook',
          color: 'green',
          endpointId: home.endpointId,
          relays: home.relays,
          lastContact: DateTime.now(),
        ),
        _server,
      ];
    final account = DeviceAccount(key: 'k' * 64, open: (_, _) => api);
    await signIn(account);
    final harness = Harness(Workspace(), account: account);
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester);
    unawaited(
      showModal(
        tester.element(find.byType(Sidebar).first),
        (context, close) => FactoryChooser(close: close),
      ),
    );
    await pumpFrames(tester);
    await _shot(tester, 'mac-01-signed-in');
    await tester.tap(find.text('MacBook'));
    await pumpFrames(tester);
    await _shot(tester, 'mac-02-row-asks');
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    await tester.tap(find.text('Use a code instead'));
    await pumpFrames(tester);
    await _shot(tester, 'mac-03-code');
    await tester.enterText(find.byType(EditableText).last, _nixos);
    await tester.pump();
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    await _shot(tester, 'mac-04-check');
  });

  testWidgets('Add a Claktory with nothing left, on a Mac', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final api = FakeAccountApi()..claktoryList = [home];
    final account = DeviceAccount(key: 'k' * 64, open: (_, _) => api);
    await signIn(account);
    final harness = Harness(Workspace(), account: account);
    harness.connections.use(RemoteFactory.fromAccount(home));
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester);
    unawaited(
      showModal(
        tester.element(find.byType(Sidebar).first),
        (context, close) => FactoryChooser(close: close),
      ),
    );
    await pumpFrames(tester);
    await _shot(tester, 'mac-05-nothing-left');
  });

  testWidgets('Add a Claktory with a code on an iPad', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace());
    harness.camera.state = CameraAccess.granted;
    await harness.show(tester, size: const Size(1180, 820));
    await pumpFrames(tester);
    unawaited(
      showModal(
        tester.element(find.byType(Sidebar).first),
        (context, close) => FactoryChooser(close: close, code: true),
      ),
    );
    await pumpFrames(tester);
    await _shot(tester, 'ipad-03-code');
    await tester.tap(find.text('Paste code'));
    await pumpFrames(tester);
    await _shot(tester, 'ipad-03b-paste');
  });

  for (final (where, size) in [
    ('mac', const Size(1200, 800)),
    ('iphone', const Size(393, 852)),
  ]) {
    testWidgets('the problem pages on the $where', (tester) async {
      final phone = where == 'iphone';
      debugDefaultTargetPlatformOverride = phone
          ? TargetPlatform.iOS
          : TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      if (phone) {
        localNetwork = _Network();
        addTearDown(() => localNetwork = null);
      }
      final harness = Harness(Workspace(), localBackend: false);
      harness.connections.use(parsePairingCode(_nixos));
      await harness.show(tester, size: size);
      await pumpFrames(tester);
      final backend = harness.backend;

      // Waiting to be allowed.
      backend.pairing.value = PairingWait(
        PairingPending(deviceId: 'd9', name: 'Mac', machine: 'nixos'),
        '4721',
      );
      await pumpFrames(tester);
      await _shot(tester, '$where-10-waiting');
      backend.pairing.value = null;

      // Can't connect.
      backend
        ..connected.value = false
        ..reconnecting.value = false
        ..lastConnected.value = DateTime.now().subtract(
          const Duration(minutes: 12),
        )
        ..tunnel.value = tunnelStatus(
          phase: TunnelPhase.failed,
          path: TunnelPath.none,
          failure: TunnelFailure.unreachable,
          error: 'no route',
        );
      await pumpFrames(tester);
      await _shot(tester, '$where-11-cant-connect');

      // Turned away, never seen let in since the app opened.
      backend.refused.value = 'unknown device';
      await pumpFrames(tester);
      await _shot(tester, '$where-12-not-allowed');

      // Turned away after it was seen let in, with no other device
      // there: factoryd pair, plainly.
      backend.refused.value = null;
      backend.workspace.apply(Workspace());
      backend.refused.value = 'unknown device';
      await pumpFrames(tester);
      await _shot(tester, '$where-12b-not-allowed-pair');

      // Signed out of the account.
      backend.refused.value =
          'This Claktory was signed out of your Claktory account, so it '
          "can't let this device in.";
      await pumpFrames(tester);
      await _shot(tester, '$where-13-signed-out');
    });
  }
}
