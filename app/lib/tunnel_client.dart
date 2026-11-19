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
  /// A tunnel to the backend [endpointId] (through [relays], from its
  /// pairing code), as the device [deviceKey].
  Future<Tunnel> open({
    required String deviceKey,
    required String endpointId,
    required List<String> relays,
  });
}

abstract interface class Tunnel {
  /// Where the app connects: `ws://127.0.0.1:<port>`.
  int get port;

  TunnelStatus get status;

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
  }) async {
    await _load();
    final handle = await rust.openTunnel(
      deviceKey: deviceKey,
      endpointId: endpointId,
      relays: relays,
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
  Future<void> close() async {
    await _handle.close();
    _handle.dispose();
  }
}

/// What the person should do about [status], for a backend on [machine];
/// empty when there's nothing to do.
String tunnelAdvice(TunnelStatus status, String machine) =>
    switch (status.failure) {
      TunnelFailure.unreachable =>
        'Check that $machine is on and awake, has a network connection, '
            'and runs Claktory with remote access on. This Mac keeps trying.',
      TunnelFailure.offline =>
        "Check this Mac's network connection. It keeps trying.",
      TunnelFailure.refused =>
        "$machine doesn't accept this device. Pair it again: on $machine, "
            'open Settings → Devices → Pair a device…, then paste the code '
            'here under "Connect to another Claktory".',
      TunnelFailure.other => 'This Mac keeps trying.',
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

/// How a connection travels, for the person: "direct", "through a relay",
/// or empty while there's none.
String tunnelPathLabel(TunnelPath path) => switch (path) {
  TunnelPath.direct => 'direct',
  TunnelPath.relayed => 'through a relay',
  TunnelPath.none => '',
};

/// A new device key: an iroh secret key is any 32 bytes, so 32 random ones
/// as lowercase hex, made on this device and kept with its pairing.
String newDeviceKey([Random? random]) {
  final source = random ?? Random.secure();
  return [
    for (var i = 0; i < 32; i++)
      source.nextInt(256).toRadixString(16).padLeft(2, '0'),
  ].join();
}
