import 'dart:developer' as developer;
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'tunnel_client.dart' show newDeviceKey;

/// A Factory on another machine, as its pairing code describes it:
/// `factory://<host>:<port>?t=<token>&fp=<certificate SHA-256>&name=<machine>`,
/// and since the private tunnel `&e=<its iroh endpoint id>`, with
/// `&r=<relay URL>` for each relay it can be reached through.
@immutable
class RemoteFactory {
  const RemoteFactory({
    required this.code,
    required this.host,
    required this.port,
    required this.token,
    required this.fingerprint,
    required this.name,
    this.endpointId = '',
    this.relays = const [],
  });

  /// The pairing code, as pasted: what's kept on the device.
  final String code;
  final String host;
  final int port;

  /// This device's key for it.
  final String token;

  /// The SHA-256 of its certificate, lowercase hex: the only certificate
  /// this app accepts from it.
  final String fingerprint;

  /// Its machine's name, e.g. "mac-mini".
  final String name;

  /// Its iroh endpoint id: the app reaches it only through the private
  /// tunnel, dialled by this key. Empty in a code from before the tunnel,
  /// which connects to [endpoint] directly.
  final String endpointId;

  /// Relays it can be reached through; none: iroh's defaults.
  final List<String> relays;

  /// Whether the app reaches it through the private tunnel.
  bool get tunnelled => endpointId.isNotEmpty;

  /// Where the app connects: TLS on its remote listener.
  Uri get endpoint => Uri(scheme: 'wss', host: host, port: port);

  /// "mac-mini", or its address when the code has no name.
  String get label => name.isNotEmpty
      ? name
      : host.isNotEmpty
      ? host
      : 'its machine';

  /// An [HttpClient] that trusts only this Factory's certificate. It's
  /// self-signed, so it never passes ordinary checks; the pin is what
  /// matters.
  HttpClient client() =>
      HttpClient()
        ..badCertificateCallback = (certificate, host, port) =>
            sha256.convert(certificate.der).toString() == fingerprint;

  @override
  bool operator ==(Object other) =>
      other is RemoteFactory && other.code == code;

  @override
  int get hashCode => code.hashCode;
}

final _hex64 = RegExp(r'^[0-9a-fA-F]{64}$');

/// An iroh endpoint id: 32 bytes as hex, or as base32 (iroh reads both).
final _endpointId = RegExp(r'^([0-9a-f]{64}|[a-z2-7]{52})$');

/// The Factory a pairing code points at; null if [text] isn't one.
RemoteFactory? parsePairingCode(String text) {
  final code = text.trim();
  final Uri uri;
  try {
    uri = Uri.parse(code);
  } on FormatException {
    return null;
  }
  final token = uri.queryParameters['t'] ?? '';
  final fingerprint = uri.queryParameters['fp'] ?? '';
  final endpointId = (uri.queryParameters['e'] ?? '').toLowerCase();
  final relays = [
    for (final relay in uri.queryParametersAll['r'] ?? const <String>[])
      if (Uri.tryParse(relay)?.scheme case 'https' || 'http') relay,
  ];
  final direct =
      uri.host.isNotEmpty &&
      uri.hasPort &&
      uri.port > 0 &&
      _hex64.hasMatch(fingerprint);
  if (uri.scheme != 'factory' ||
      !_hex64.hasMatch(token) ||
      (endpointId.isNotEmpty && !_endpointId.hasMatch(endpointId)) ||
      // Through the tunnel it needs only its endpoint id; directly, its
      // address and certificate.
      (endpointId.isEmpty && !direct)) {
    return null;
  }
  return RemoteFactory(
    code: code,
    host: uri.host,
    port: uri.hasPort ? uri.port : 0,
    token: token,
    fingerprint: fingerprint.toLowerCase(),
    name: uri.queryParameters['name'] ?? '',
    endpointId: endpointId,
    relays: relays,
  );
}

/// Which Factory this app shows, and the other ones it has connected to.
/// Kept on this device, the third thing besides the window's frame and the
/// drafts (the person's decision): a device can't ask a backend where its
/// backend is.
class Connections extends ChangeNotifier {
  Connections(
    this._storage, [
    this._current,
    List<RemoteFactory>? saved,
    Map<String, String>? keys,
  ]) : _saved = saved ?? [],
       _keys = keys ?? {};

  /// Reads what this device keeps. Anything unreadable is left out.
  static Future<Connections> load(ConnectionStorage storage) async {
    var kept = const StoredConnections(current: '', saved: [], keys: {});
    try {
      kept = await storage.read();
    } catch (error) {
      developer.log('could not read connections: $error', name: 'factory');
    }
    final saved = [for (final code in kept.saved) ?parsePairingCode(code)];
    final current = parsePairingCode(kept.current);
    if (current != null && !saved.contains(current)) saved.add(current);
    return Connections(storage, current, saved, {
      for (final MapEntry(:key, :value) in kept.keys.entries)
        if (saved.any((target) => target.code == key)) key: value,
    });
  }

  final ConnectionStorage _storage;
  RemoteFactory? _current;
  final List<RemoteFactory> _saved;

  /// This device's iroh key for each Factory it reaches through the
  /// tunnel, by pairing code: made here, never sent anywhere. One per
  /// pairing, so two Factories can't tell it's the same device.
  final Map<String, String> _keys;

  /// This device's key for [target], made and kept the first time.
  String deviceKey(RemoteFactory target, {String Function()? make}) {
    final kept = _keys[target.code];
    if (kept != null) return kept;
    final made = (make ?? newDeviceKey)();
    _keys[target.code] = made;
    _changed();
    return made;
  }

  /// The Factory on another machine this app shows; null for this Mac's.
  RemoteFactory? get current => _current;

  /// Every other Factory this device has connected to, oldest first.
  List<RemoteFactory> get saved => List.unmodifiable(_saved);

  /// [text] as a QR code, made on this device; null where it can't be.
  Future<QrModules?> qrCode(String text) async {
    try {
      return await _storage.qrCode(text);
    } catch (error) {
      developer.log('could not make a QR code: $error', name: 'factory');
      return null;
    }
  }

  /// Shows [target] from now on (null for this Mac's), and remembers it.
  void use(RemoteFactory? target) {
    _current = target;
    if (target != null) {
      _saved.removeWhere((other) => other == target);
      _saved.add(target);
    }
    _changed();
  }

  /// Forgets [target]; if it's the one showing, this Mac's shows instead.
  void forget(RemoteFactory target) {
    _saved.removeWhere((other) => other == target);
    _keys.remove(target.code);
    if (_current == target) _current = null;
    _changed();
  }

  void _changed() {
    notifyListeners();
    _storage
        .write(
          StoredConnections(
            current: _current?.code ?? '',
            saved: [for (final target in _saved) target.code],
            keys: Map.of(_keys),
          ),
        )
        .catchError(
          (Object error) => developer.log(
            'could not keep connections: $error',
            name: 'factory',
          ),
        );
  }
}

/// What [ConnectionStorage] keeps: pairing codes, and this device's iroh
/// key for each.
@immutable
class StoredConnections {
  const StoredConnections({
    required this.current,
    required this.saved,
    this.keys = const {},
  });

  /// The code of the Factory showing; empty for this Mac's.
  final String current;
  final List<String> saved;

  /// This device's key by pairing code.
  final Map<String, String> keys;
}

/// Where [Connections] are kept, and how a pairing code becomes a QR code.
abstract interface class ConnectionStorage {
  Future<StoredConnections> read();
  Future<void> write(StoredConnections connections);

  /// [text] as a QR code: its modules row by row, dark ones true; null if
  /// it can't be made here.
  Future<QrModules?> qrCode(String text);
}

/// A QR code's modules: [size] by [size], dark ones true.
@immutable
class QrModules {
  const QrModules(this.size, this.dark);

  final int size;
  final List<bool> dark;

  bool at(int x, int y) => dark[y * size + x];
}

/// This app's user defaults on this device, and Core Image's QR codes,
/// through the native side (`MainFlutterWindow.swift`).
class DeviceConnectionStorage implements ConnectionStorage {
  static const _channel = MethodChannel('factory/device');

  @override
  Future<StoredConnections> read() async {
    final kept = await _channel.invokeMapMethod<String, Object?>('connections');
    return StoredConnections(
      current: kept?['current'] as String? ?? '',
      saved: [
        for (final code in kept?['saved'] as List<Object?>? ?? const [])
          if (code is String) code,
      ],
      keys: {
        for (final MapEntry(:key, :value)
            in (kept?['keys'] as Map<Object?, Object?>? ?? const {}).entries)
          if (key is String && value is String) key: value,
      },
    );
  }

  @override
  Future<void> write(StoredConnections connections) =>
      _channel.invokeMethod<void>('setConnections', {
        'current': connections.current,
        'saved': connections.saved,
        'keys': connections.keys,
      });

  @override
  Future<QrModules?> qrCode(String text) async {
    final made = await _channel.invokeMapMethod<String, Object?>(
      'qrCode',
      text,
    );
    final size = made?['size'];
    final modules = made?['modules'];
    if (size is! int ||
        modules is! Uint8List ||
        modules.length != size * size) {
      return null;
    }
    return QrModules(size, [for (final module in modules) module != 0]);
  }
}

/// Connections kept in memory, for tests: [kept] is what a restart reads.
class MemoryConnectionStorage implements ConnectionStorage {
  StoredConnections kept = const StoredConnections(
    current: '',
    saved: [],
    keys: {},
  );

  @override
  Future<StoredConnections> read() async => kept;

  @override
  Future<void> write(StoredConnections connections) async => kept = connections;

  @override
  Future<QrModules?> qrCode(String text) async => null;
}
