import 'dart:math';

import 'tunnel/api/tunnel.dart' as rust;
import 'tunnel/api/tunnel.dart'
    show TunnelFailure, TunnelPath, TunnelPhase, TunnelStatus;
import 'tunnel/frb_generated.dart';

export 'tunnel/api/tunnel.dart'
    show TunnelFailure, TunnelPath, TunnelPhase, TunnelStatus;

/// The private tunnel (rust/, docs: private-tunnel.md): the app reaches its
/// backend only through iroh, dialled by the backend's public key, direct or
/// through a relay, end-to-end encrypted. A [Tunnel] is a loopback port the
/// app's WebSocket connects to exactly as it would to the backend itself.
abstract interface class Tunnels {
  /// A tunnel to the backend [endpointId] (through [relays], or straight
  /// to its direct [addresses], from its pairing code), as the device
  /// [deviceKey].
  Future<Tunnel> open({
    required String deviceKey,
    required String endpointId,
    required List<String> relays,
    List<String> addresses = const [],
  });
}

abstract interface class Tunnel {
  /// Where the app connects: `ws://127.0.0.1:<port>`.
  int get port;

  TunnelStatus get status;

  /// After the app comes back from the background, or the network changed:
  /// the next socket dials a new connection, and nothing counts as failed.
  Future<void> reconnect();

  Future<void> close();
}

/// The tunnel, in Rust, loaded on first use.
class RustTunnels implements Tunnels {
  RustTunnels();

  static Future<void>? _loaded;

  static Future<void> _load() => _loaded ??= RustLib.init();

  @override
  Future<Tunnel> open({
    required String deviceKey,
    required String endpointId,
    required List<String> relays,
    List<String> addresses = const [],
  }) async {
    await _load();
    final handle = await rust.openTunnel(
      deviceKey: deviceKey,
      endpointId: endpointId,
      relays: relays,
      addresses: addresses,
    );
    return _RustTunnel(handle);
  }
}

class _RustTunnel implements Tunnel {
  _RustTunnel(this._handle) : port = _handle.port();

  final rust.TunnelHandle _handle;

  @override
  final int port;

  @override
  TunnelStatus get status => _handle.status();

  @override
  Future<void> reconnect() => _handle.reconnect();

  @override
  Future<void> close() async {
    await _handle.close();
    _handle.dispose();
  }
}

/// What the person should do about [status], for a Claktory on
/// [machine], from this [device] ("Mac", "iPhone"); empty when there's
/// nothing to do.
String tunnelAdvice(
  TunnelStatus status,
  String machine, {
  String device = 'Mac',
}) => switch (status.failure) {
  TunnelFailure.unreachable =>
    'Make sure $machine is on, awake and online, with Claktory open. '
        'Trying again…',
  TunnelFailure.offline =>
    'This $device is offline. Check your internet connection.',
  TunnelFailure.refused =>
    '$machine no longer allows this device. Add it again on $machine in '
        'Settings → Devices.',
  TunnelFailure.other => 'Trying again…',
  TunnelFailure.none => '',
};

/// A tunnel's status while nothing's known yet.
const idleTunnel = TunnelStatus(
  phase: TunnelPhase.idle,
  path: TunnelPath.none,
  error: '',
  failure: TunnelFailure.none,
  relayConnected: false,
);

/// A new device key: an iroh secret key is any 32 bytes, so 32 random ones
/// as lowercase hex, made on this device and kept with its pairing.
String newDeviceKey([Random? random]) {
  final source = random ?? Random.secure();
  return [
    for (var i = 0; i < 32; i++)
      source.nextInt(256).toRadixString(16).padLeft(2, '0'),
  ].join();
}
