import 'dart:math';

import 'loopback_forward.dart';
import 'network.dart' show deviceNetwork;
import 'tunnel/api/tunnel.dart' as rust;
import 'tunnel/api/tunnel.dart'
    show
        TunnelFailure,
        TunnelPath,
        TunnelPhase,
        TunnelReach,
        TunnelRelayTrouble,
        TunnelStatus;

import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart'
    show ExternalLibrary;

import 'tunnel/frb_generated.dart';

export 'tunnel/api/tunnel.dart'
    show
        TunnelFailure,
        TunnelPath,
        TunnelPhase,
        TunnelReach,
        TunnelRelayTrouble,
        TunnelStatus;

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

  /// The endpoint ids, as lowercase hex, a pairing number is worked out
  /// from: this device's, from its [deviceKey], and the backend's
  /// [endpointId] (a pairing code's `e=`, hex or base32).
  Future<({String device, String backend})> pairingIds({
    required String deviceKey,
    required String endpointId,
  });
}

abstract interface class Tunnel {
  /// Where the app connects: `ws://127.0.0.1:<port><requestPath>`.
  int get port;

  /// What the app asks for at [port]: the tunnel's own secret path. Any
  /// program on this device can reach the port, so the tunnel forwards
  /// only a socket asking for this. Never shown, saved or logged.
  String get requestPath;

  TunnelStatus get status;

  /// After the app comes back from the background, or the network changed:
  /// the next socket dials a new connection, and nothing counts as failed.
  Future<void> reconnect();

  /// The relays its Claktory uses now, as its Workspace says (#450): the
  /// device homes on them from its next endpoint on, and dials through them.
  void setRelays(List<String> relays);

  /// Listens on this device's 127.0.0.1 and ::1 at [port] and forwards
  /// each connection to that port on the backend's machine, or to [target]
  /// there when given (a website shared over a pipe, #503) (ALPN
  /// claktory-forward/1). Throws [ForwardFailure].
  Future<PortForward> forwardPort(int port, {int? target});

  Future<void> close();
}

/// What the tunnels run on (#214): one runtime for all of them, which can
/// be asked whether it still runs anything, and made afresh when it
/// doesn't, as relaunching the app would.
abstract interface class TunnelRuntime {
  /// Whether it runs a task within [within].
  Future<bool> answers(Duration within);

  /// Ends it, and every tunnel on it; the next tunnel opens on a new one.
  void fresh();
}

/// The Rust library's runtime.
class RustTunnelRuntime implements TunnelRuntime {
  const RustTunnelRuntime();

  @override
  Future<bool> answers(Duration within) async {
    await RustTunnels.load();
    return rust.runtimeAnswers().timeout(within, onTimeout: () => false);
  }

  @override
  void fresh() => rust.freshRuntime();
}

/// A runtime that always answers: tunnels that aren't Rust's (tests).
class AnsweringRuntime implements TunnelRuntime {
  const AnsweringRuntime();

  @override
  Future<bool> answers(Duration within) async => true;

  @override
  void fresh() {}
}

/// The tunnel, in Rust, loaded on first use.
class RustTunnels implements Tunnels {
  RustTunnels();

  static Future<void>? _loaded;

  /// Loads the app's Rust library (the tunnel's, and the account key's
  /// too), once; from [library] when given (a test's build of it).
  static Future<void> load({ExternalLibrary? library}) =>
      _loaded ??= RustLib.init(externalLibrary: library);

  @override
  Future<Tunnel> open({
    required String deviceKey,
    required String endpointId,
    required List<String> relays,
    List<String> addresses = const [],
  }) async {
    await load();
    final handle = await rust.openTunnel(
      deviceKey: deviceKey,
      endpointId: endpointId,
      relays: relays,
      addresses: addresses,
    );
    return _RustTunnel(handle);
  }

  @override
  Future<({String device, String backend})> pairingIds({
    required String deviceKey,
    required String endpointId,
  }) async {
    await load();
    final ids = rust.pairingIds(deviceKey: deviceKey, endpointId: endpointId);
    return (device: ids.device, backend: ids.backend);
  }
}

class _RustTunnel implements Tunnel {
  _RustTunnel(this._handle)
    : port = _handle.port(),
      requestPath = _handle.path();

  final rust.TunnelHandle _handle;

  @override
  final int port;

  @override
  final String requestPath;

  @override
  TunnelStatus get status => _handle.status();

  @override
  Future<void> reconnect() => _handle.reconnect();

  @override
  void setRelays(List<String> relays) => _handle.setRelays(relays: relays);

  @override
  Future<PortForward> forwardPort(int port, {int? target}) async {
    try {
      return _RustForward(
        target == null
            ? await _handle.forwardPort(port: port)
            : await _handle.forwardPortTo(port: port, target: target),
      );
    } on rust.ForwardError catch (error) {
      throw ForwardFailure(switch (error.kind) {
        rust.ForwardErrorKind.portTaken => ForwardProblem.taken,
        rust.ForwardErrorKind.unsupported => ForwardProblem.unsupported,
        rust.ForwardErrorKind.failed => ForwardProblem.failed,
      }, error.message);
    }
  }

  @override
  Future<void> close() async {
    await _handle.close();
    _handle.dispose();
  }
}

/// A port of this device's loopback carried to the same port on the
/// backend's machine.
class _RustForward implements PortForward {
  _RustForward(this._handle) : port = _handle.port();

  final rust.ForwardHandle _handle;

  @override
  final int port;

  @override
  int get connections => _handle.status().connections;

  @override
  String get lastError => _handle.status().lastError;

  @override
  Future<void> stop() async {
    await _handle.stop();
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
  TunnelFailure.unreachable => unreachableAdvice(machine),
  TunnelFailure.offline when !offlineNow(status) =>
    "This $device is online, but can't reach Claktory's servers. "
        '${unreachableAdvice(machine)}',
  TunnelFailure.offline =>
    'This $device is offline. Check your internet connection.',
  TunnelFailure.refused =>
    "$machine doesn't allow this device. Add it on $machine in "
        'Settings → Devices.',
  TunnelFailure.other => 'Trying again…',
  TunnelFailure.none => '',
};

/// What to do about a Claktory on [machine] that doesn't answer.
String unreachableAdvice(String machine) =>
    'Make sure $machine is on, awake and online, with Claktory open. '
    'Trying again…';

/// Whether this device is offline, as far as it knows: the tunnel says so
/// when it can reach neither its relay nor the machine, but a relay out of
/// reach doesn't make the device offline, so its own network has the last
/// word. Before the system has said (or where it can't), the tunnel's word
/// stands.
bool offlineNow(TunnelStatus status) =>
    status.failure == TunnelFailure.offline &&
    deviceNetwork.value?.satisfied != true;

/// A tunnel's status while nothing's known yet.
const idleTunnel = TunnelStatus(
  phase: TunnelPhase.idle,
  path: TunnelPath.none,
  error: '',
  failure: TunnelFailure.none,
  relayConnected: false,
  relayUpMs: 0,
  relayTrouble: TunnelRelayTrouble.none,
  received: 0,
  linkReceived: 0,
  reach: TunnelReach.none,
  dialTimeoutMs: 0,
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
