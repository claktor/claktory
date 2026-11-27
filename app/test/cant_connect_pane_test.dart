import 'package:factory_app/connection.dart' show Connection, ConnectionDialog;
import 'package:factory_app/mobile/local_network.dart';
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/tunnel/api/tunnel.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'tunnel_test.dart' show tunnelStatus;

// Zucchero's row 480 (#37): while the drawer and the connection sheet said
// "Can't connect", the main pane behind them said "Connecting…". It says
// the same now, as a problem page (devices-signin.md §4): what to check,
// Details and Try again.

void main() {
  for (final (platform, size) in [
    (TargetPlatform.iOS, const Size(320, 568)),
    (TargetPlatform.macOS, const Size(1200, 800)),
  ]) {
    testWidgets("can't connect, the main pane says so with Try again, never "
        'Connecting… (${platform.name})', (tester) async {
      debugDefaultTargetPlatformOverride = platform;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      if (platform == TargetPlatform.iOS) {
        // The smallest phone, at the largest text.
        tester.platformDispatcher.textScaleFactorTestValue = 53 / 17;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      }
      final harness = Harness(Workspace(), localBackend: false);
      harness.connections.use(
        parsePairingCode(
          'factory://?t=${'b' * 64}&name=mac-mini&e=${'d' * 64}',
        ),
      );
      await harness.show(tester, size: size);
      await pumpFrames(tester);
      final backend = harness.backend
        ..connected.value = false
        ..reconnecting.value = false;
      await pumpFrames(tester);
      expect(find.text('Connecting…'), findsWidgets);
      expect(find.text('Try again'), findsNothing);

      backend.lastConnected.value = DateTime.now().subtract(
        const Duration(minutes: 12),
      );
      backend.tunnel.value = tunnelStatus(
        phase: TunnelPhase.failed,
        path: TunnelPath.none,
        failure: TunnelFailure.unreachable,
        error: 'no route',
      );
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      expect(find.text('Connecting…'), findsNothing);
      expect(find.text("Can't connect to mac-mini"), findsWidgets);
      expect(
        find.text(
          'Make sure mac-mini is on, awake and online. Last seen 12 min ago. '
          'Trying again…',
        ),
        findsOneWidget,
      );
      expect(find.text('Details'), findsOneWidget);
      // No Local Network to ask about here.
      expect(find.text('Is Local Network on?'), findsNothing);
      final reconnect = find.text('Try again');
      expect(reconnect, findsOneWidget);
      await tester.ensureVisible(reconnect);
      await pumpFrames(tester);
      final at = tester.getRect(reconnect);
      expect(at.top, greaterThanOrEqualTo(0));
      expect(at.bottom, lessThanOrEqualTo(size.height));

      // Tapped, it tries again at once.
      await tester.tap(reconnect);
      await pumpFrames(tester);
      expect(backend.reconnectsAsked, 1);

      // Connected again: the usual pane.
      backend.connected.value = true;
      backend.workspace.apply(Workspace());
      await pumpFrames(tester);
      expect(find.text('Try again'), findsNothing);
      expect(find.textContaining('Trying again'), findsNothing);
    });
  }

  // Trish (#37): the Claktory by its own name wherever the app knows it,
  // never the machine's from an old pairing code; and the tunnel's "can't
  // reach the relay or the machine" in the sheet's words.
  testWidgets("can't connect, it names the Claktory as it calls itself, and "
      "says what this iPhone can't reach", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=mac-mini&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(390, 844));
    await pumpFrames(tester);
    final backend = harness.backend
      ..connected.value = true
      ..workspace.apply(Workspace(settings: Settings(name: 'Studio')));
    await pumpFrames(tester);
    backend
      ..connected.value = false
      ..reconnecting.value = false
      ..tunnel.value = tunnelStatus(
        phase: TunnelPhase.failed,
        path: TunnelPath.none,
        failure: TunnelFailure.offline,
        error:
            "This device can't reach the relay or the machine: check its "
            'network.',
      );
    await pumpFrames(tester);
    expect(find.text("Can't connect to Studio"), findsWidgets);
    expect(find.textContaining('mac-mini'), findsNothing);
    final connection = Connection(backend);
    expect(
      connection.error,
      "This iPhone can't reach Claktory's servers or Studio: check its "
      'network.',
    );
    // Any other failure keeps the tunnel's own words, for whoever looks.
    backend.tunnel.value = tunnelStatus(
      phase: TunnelPhase.failed,
      path: TunnelPath.none,
      failure: TunnelFailure.unreachable,
      error: 'no route',
    );
    expect(Connection(backend).error, 'no route');
  });

  // An iPhone or iPad (devices-signin.md §7): whether Local Network is on,
  // asked, never checked unasked; Details is the connection in full.
  testWidgets("on an iPhone, can't connect asks whether Local Network is on, "
      'and Check Local Network opens its sheet', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final network = _Network();
    localNetwork = network;
    addTearDown(() => localNetwork = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=mac-mini&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(390, 844));
    await pumpFrames(tester);
    harness.backend
      ..connected.value = false
      ..reconnecting.value = false
      ..tunnel.value = tunnelStatus(
        phase: TunnelPhase.failed,
        path: TunnelPath.none,
        failure: TunnelFailure.unreachable,
        error: 'no route',
      );
    await pumpFrames(tester);
    expect(find.text('Is Local Network on?'), findsOneWidget);
    expect(network.checks, 0, reason: 'never checked unasked');
    await tester.tap(find.text('Check Local Network'));
    await pumpFrames(tester);
    expect(find.text('Find your Claktory on this network'), findsOneWidget);
  });

  testWidgets("can't connect: Details shows the connection in full", (
    tester,
  ) async {
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=mac-mini&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester);
    harness.backend
      ..connected.value = false
      ..reconnecting.value = false
      ..tunnel.value = tunnelStatus(
        phase: TunnelPhase.failed,
        path: TunnelPath.none,
        failure: TunnelFailure.unreachable,
        error: 'no route',
      );
    await pumpFrames(tester);
    await tester.tap(find.text('Details'));
    await pumpFrames(tester);
    expect(find.byType(ConnectionDialog), findsOneWidget);
    expect(find.text('no route'), findsWidgets);
  });
}

class _Network implements LocalNetwork {
  var checks = 0;

  @override
  Future<LocalNetworkAccess> check() async {
    checks++;
    return LocalNetworkAccess.granted;
  }

  @override
  Future<void> openSettings() async {}
}
