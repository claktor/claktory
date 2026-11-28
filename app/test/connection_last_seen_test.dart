// Friend bug #37: a Claktory that can't be reached said only that. Now it
// says what's known: when it was last seen (this app's last connection to
// it, and its last check-in with the person's Claktory account), and why
// as far as this device can tell (offline, unable to reach Claktory's
// servers and since when, or online while it doesn't answer).
// Never a guessed cause: whether the other machine reaches its relay
// can't be told from here (Pesci).
import 'dart:async';
import 'dart:io';

import 'package:factory_app/account_match.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/connection.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/mobile/device.dart'
    show DeviceCapabilities, Idiom, debugDeviceCapabilities;
import 'package:factory_app/network.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/tunnel/api/tunnel.dart';
import 'package:factory_app/tunnel_client.dart' show tunnelAdvice;
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'reconnect_from_scratch_test.dart' show until;
import 'tunnel_test.dart' show FakeTunnel, FakeTunnels, tunnelStatus;

final _code =
    'factory://?t=${'a' * 64}&name=mac-mini&e=${'c' * 64}'
    '&r=https://relay-eu.example.com';

final _now = DateTime(2026, 12, 3, 15, 0);

/// A Claktory on another machine whose last try failed: it couldn't be
/// reached ([failure]), with this device's relay up or not.
Backend _failing({
  TunnelFailure failure = TunnelFailure.unreachable,
  bool relay = true,
  TunnelRelayTrouble trouble = TunnelRelayTrouble.none,
}) {
  final backend = Harness(Workspace()).backend
    ..connectTo(parsePairingCode(_code))
    ..connected.value = false
    ..reconnecting.value = false;
  backend.tunnel.value = tunnelStatus(
    phase: TunnelPhase.failed,
    path: TunnelPath.none,
    failure: failure,
    relay: relay,
    trouble: trouble,
  );
  return backend;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    debugDeviceCapabilities = const DeviceCapabilities(idiom: Idiom.phone);
  });
  tearDown(() {
    debugDefaultTargetPlatformOverride = null;
    debugDeviceCapabilities = null;
  });

  test('nothing known: it says only what it did', () {
    final connection = Connection(_failing(), now: _now);
    expect(connection.link, Link.failing);
    expect(connection.short, "Can't connect");
    expect(
      connection.details,
      "This iPhone is online, but mac-mini doesn't answer. Make sure "
      'mac-mini is on, awake and online, with Claktory open. Trying again…',
    );
  });

  test('checked in lately and connected earlier: it was running, and '
      'when this iPhone last had it', () {
    final backend = _failing()
      ..checkedIn.value = _now.subtract(const Duration(minutes: 12))
      ..lastConnected.value = DateTime(2026, 12, 3, 13, 40);
    final connection = Connection(backend, now: _now);
    expect(connection.short, "Can't connect · last seen 12 min ago");
    expect(
      connection.details,
      "This iPhone is online, but mac-mini doesn't answer. mac-mini last "
      'checked in with your Claktory account 12 min ago, so it was running '
      'then. This iPhone was last connected to it at 13:40. Make sure '
      'mac-mini is on, awake and online, with Claktory open. Trying again…',
    );
  });

  test('not checked in for hours: when, and no more', () {
    final backend = _failing()
      ..checkedIn.value = _now.subtract(const Duration(hours: 3));
    final connection = Connection(backend, now: _now);
    expect(connection.short, "Can't connect · last seen 3 h ago");
    expect(
      connection.details,
      contains('mac-mini last checked in with your Claktory account 3 h ago. '),
    );
    expect(connection.details, isNot(contains('running')));
    // Connected since that check-in: the later is when it was last seen.
    backend.lastConnected.value = _now.subtract(const Duration(minutes: 5));
    expect(
      Connection(backend, now: _now).short,
      "Can't connect · last seen 5 min ago",
    );
  });

  test("this iPhone can't reach Claktory's servers: since when, and why", () {
    final backend = _failing(relay: false, trouble: TunnelRelayTrouble.timeout)
      ..relayDownSince.value = DateTime(2026, 12, 3, 14, 2);
    expect(
      Connection(backend, now: _now).details,
      startsWith(
        "This iPhone is online, but hasn't reached Claktory's servers since "
        '14:02: nothing answered in time.',
      ),
    );
  });

  test('once it says it can\'t connect, it keeps saying so (and what it '
      'knows) through the tries after, until a reason to try afresh '
      "(Zucchero's row 475)", () async {
    final backend = _failing()
      ..lastConnected.value = DateTime(2026, 12, 3, 14, 40);
    backend.tunnel.value = tunnelStatus(
      phase: TunnelPhase.failed,
      path: TunnelPath.none,
      failure: TunnelFailure.unreachable,
      error: 'Nothing answered within 30 seconds.',
    );
    expect(backend.lastFailure.value, isNotNull);
    final said = Connection(backend, now: _now).details;
    expect(
      said,
      startsWith("This iPhone is online, but mac-mini doesn't answer."),
    );

    // A reconnect from scratch: quiet, its new tunnel's status afresh.
    backend
      ..reconnecting.value = true
      ..tunnel.value = tunnelStatus(
        phase: TunnelPhase.dialing,
        path: TunnelPath.none,
      );
    final during = Connection(backend, now: _now);
    expect(during.link, Link.failing);
    expect(during.short, startsWith("Can't connect"));
    expect(during.details, said);
    // Its cause line too, so the sheet doesn't jump (row 480).
    expect(during.error, 'Nothing answered within 30 seconds.');

    // The network moved: what failed before says nothing now.
    backend.networkMoved();
    expect(backend.lastFailure.value, isNull);
    expect(Connection(backend, now: _now).link, Link.connecting);
    await backend.close();
  });

  test('offline: only that, and when it was last seen', () {
    final backend = _failing(
      failure: TunnelFailure.offline,
      relay: false,
      trouble: TunnelRelayTrouble.dns,
    )..relayDownSince.value = DateTime(2026, 12, 3, 14, 2);
    backend.lastConnected.value = DateTime(2026, 12, 3, 14, 1);
    expect(
      Connection(backend, now: _now).details,
      'This iPhone was last connected to it at 14:01. This iPhone is '
      'offline. Check your internet connection.',
    );
  });

  test("its relay out of reach while this iPhone's own network is up: "
      'not offline, and what it knows instead (Zucchero, #37)', () {
    addTearDown(() => deviceNetwork.value = null);
    final backend = _failing(
      failure: TunnelFailure.offline,
      relay: false,
      trouble: TunnelRelayTrouble.timeout,
    )..relayDownSince.value = DateTime(2026, 12, 3, 14, 2);
    deviceNetwork.value = const NetworkPath(
      satisfied: true,
      interfaces: ['wifi'],
    );
    final online = Connection(backend, now: _now).details;
    expect(online, isNot(contains('offline')));
    expect(
      online,
      "This iPhone is online, but hasn't reached Claktory's servers since "
      '14:02: nothing answered in time. Make sure mac-mini is on, awake and '
      'online, with Claktory open. Trying again…',
    );
    // Nothing known of when: no since.
    backend.relayDownSince.value = null;
    expect(
      Connection(backend, now: _now).details,
      startsWith(
        "This iPhone is online, but can't reach Claktory's servers: nothing "
        'answered in time. Make sure',
      ),
    );
    // Elsewhere (the pairing form, the problem's dialog) too.
    expect(
      tunnelAdvice(backend.tunnel.value!, 'mac-mini', device: 'iPhone'),
      "This iPhone is online, but can't reach Claktory's servers. Make sure "
      'mac-mini is on, awake and online, with Claktory open. Trying again…',
    );

    // Its own network down: offline, as before.
    deviceNetwork.value = const NetworkPath(satisfied: false);
    expect(
      Connection(backend, now: _now).details,
      'This iPhone is offline. Check your internet connection.',
    );
  });

  test(
    "the account's list gives each Claktory on it its last check-in",
    () async {
      final key = 'C' * 64;
      var signedOut = false;
      final account = ChangeNotifier();
      final match = AccountMatch(
        remotes: () => [key],
        changes: ChangeNotifier(),
        account: account,
        theirs: () => signedOut
            ? null
            : Future.value([
                AccountClaktory(
                  id: 'm',
                  name: 'mac-mini',
                  endpointId: key,
                  lastContact: DateTime(2026, 12, 3, 14, 48),
                ),
                AccountClaktory(id: 'n', name: 'nixos', endpointId: 'd' * 64),
              ]),
      );
      addTearDown(match.dispose);
      await pumpEventQueue();
      expect(match.checkedIn(key.toLowerCase()), DateTime(2026, 12, 3, 14, 48));
      expect(match.checkedIn('d' * 64), isNull, reason: 'never checked in');
      expect(match.checkedIn('e' * 64), isNull, reason: 'not on the account');
      expect(match.checkedIn(null), isNull, reason: 'this Mac’s own');

      signedOut = true;
      account.notifyListeners();
      await pumpEventQueue();
      expect(match.checkedIn(key), isNull);
    },
  );

  test('a connection that drops is remembered as last connected; the '
      "relay's loss, as since when", () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final sockets = <WebSocket>[];
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      sockets.add(socket);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (!message.hasHello()) return;
        socket.add(
          ServerMessage(
            replyTo: message.id,
            welcome: Welcome(serverVersion: 'test'),
          ).writeToBuffer(),
        );
      });
    });
    addTearDown(() => server.close(force: true));
    final tunnel = FakeTunnel(server.port);
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      tunnels: FakeTunnels(tunnel),
    )..deviceKey = ((_) => 'k' * 64);
    backend.connectTo(parsePairingCode(_code));
    unawaited(backend.run());
    await until(() => backend.connected.value);
    expect(backend.lastConnected.value, isNull);

    final before = DateTime.now();
    tunnel.status = tunnelStatus(
      relay: false,
      trouble: TunnelRelayTrouble.timeout,
    );
    for (final socket in sockets) {
      await socket.close();
    }
    await until(() => !backend.connected.value);
    expect(backend.lastConnected.value, isNotNull);
    expect(
      backend.lastConnected.value!.isBefore(before),
      isFalse,
      reason: 'when it dropped',
    );
    await until(() => backend.relayDownSince.value != null);
    final down = backend.relayDownSince.value;
    // Made from scratch, its tunnel closed and opened afresh: since when
    // stays (Abbacchio's D3).
    backend.reconnectNow();
    await until(() => tunnel.closed);
    expect(backend.relayDownSince.value, down);
    tunnel.closed = false;

    tunnel.status = tunnelStatus();
    await until(() => backend.relayDownSince.value == null);
    await until(() => backend.connected.value, seconds: 15);
    expect(backend.lastConnected.value, isNull, reason: 'connected again');
    await backend.close();
  });
}
