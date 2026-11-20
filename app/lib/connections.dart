import 'dart:developer' as developer;
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'tunnel_client.dart' show newDeviceKey;

/// A Factory on another machine, as its pairing code describes it. Since
/// the private tunnel: `factory://?t=<token>&name=<machine>&e=<endpoint>`
/// (its iroh endpoint id), then `&r=<relay URL>` for each relay it can be
/// reached through and `&a=<ip:port>` for each direct address. Before it:
/// `factory://<host>:<port>?t=<token>&fp=<SHA-256>&name=<machine>`, with
/// its certificate's SHA-256.
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
    this.addresses = const [],
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

  /// Direct UDP addresses it listens on (`ip:port`), tried as well as the
  /// relays: this Mac's own code has only these.
  final List<String> addresses;

  /// Whether the app reaches it through the private tunnel.
  bool get tunnelled => endpointId.isNotEmpty;

  /// Where the app connects: TLS on its remote listener.
  Uri get endpoint => Uri(scheme: 'wss', host: host, port: port);

  /// The same Factory under another [name], as the person renamed it on
  /// this device.
  RemoteFactory named(String name) => RemoteFactory(
    code: code,
    host: host,
    port: port,
    token: token,
    fingerprint: fingerprint,
    name: name,
    endpointId: endpointId,
    relays: relays,
    addresses: addresses,
  );

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

/// Whether this device runs a Factory of its own, shown when no other is
/// chosen: a Mac does. A phone doesn't: it shows one on another machine,
/// paired with it, or nothing until it is.
bool get hasLocalBackend => switch (defaultTargetPlatform) {
  TargetPlatform.iOS || TargetPlatform.android => false,
  _ => true,
};

final _hex64 = RegExp(r'^[0-9a-fA-F]{64}$');

/// An iroh endpoint id: 32 bytes as hex, or as base32 (iroh reads both).
final _endpointId = RegExp(r'^([0-9a-f]{64}|[a-z2-7]{52})$');

/// `ip:port`, IPv4 or `[IPv6]`.
final _socketAddress = RegExp(
  r'^(\d{1,3}(\.\d{1,3}){3}|\[[0-9a-fA-F:.]+\]):\d{1,5}$',
);

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
  final addresses = [
    for (final address in uri.queryParametersAll['a'] ?? const <String>[])
      if (_socketAddress.hasMatch(address)) address,
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
    addresses: addresses,
  );
}

/// What this Mac's own Factory is called until the person renames it.
const thisMacName = 'This Mac';

/// Every Factory this app connects to (this Mac's, and the ones on other
/// machines it's paired with, all live at once), which one the window
/// shows, and what the person calls each. Kept on this device, the third
/// thing besides the window's frame and the drafts (the person's decision):
/// a device can't ask a backend where its backend is.
class Connections extends ChangeNotifier {
  Connections(
    this._storage, [
    this._current,
    List<RemoteFactory>? saved,
    Map<String, String>? keys,
    Map<String, String>? names,
  ]) : _saved = saved ?? [],
       _keys = keys ?? {},
       _names = names ?? {};

  /// Reads what this device keeps. Anything unreadable is left out.
  static Future<Connections> load(ConnectionStorage storage) async {
    var kept = const StoredConnections(current: '', saved: []);
    try {
      kept = await storage.read();
    } catch (error) {
      developer.log('could not read connections: $error', name: 'factory');
    }
    final saved = [for (final code in kept.saved) ?parsePairingCode(code)];
    final current = parsePairingCode(kept.current);
    if (current != null && !saved.contains(current)) saved.add(current);
    bool known(String code) => saved.any((target) => target.code == code);
    return Connections(
      storage,
      current,
      saved,
      {
        for (final MapEntry(:key, :value) in kept.keys.entries)
          if (known(key)) key: value,
      },
      {
        for (final MapEntry(:key, :value) in kept.names.entries)
          if ((key.isEmpty || known(key)) && value.trim().isNotEmpty)
            key: value.trim(),
      },
    );
  }

  final ConnectionStorage _storage;
  RemoteFactory? _current;
  final List<RemoteFactory> _saved;

  /// This device's iroh key for each Factory it reaches through the
  /// tunnel, by pairing code: made here, never sent anywhere. One per
  /// pairing, so two Factories can't tell it's the same device.
  final Map<String, String> _keys;

  /// What the person calls each Factory on this device, by pairing code
  /// ('' for this Mac's), where they've renamed it.
  final Map<String, String> _names;

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
  /// Under the name the person gave it, if they renamed it.
  RemoteFactory? get current => _named(_current);

  /// Every other Factory this device has connected to, oldest first, under
  /// the names the person gave them. All of them are connected at once.
  List<RemoteFactory> get saved =>
      List.unmodifiable([for (final target in _saved) _named(target)!]);

  RemoteFactory? _named(RemoteFactory? target) {
    if (target == null) return null;
    final name = _names[target.code];
    return name == null ? target : target.named(name);
  }

  /// What [target] is called here (null for this Mac's): the name the
  /// person gave it, else its machine's.
  String nameOf(RemoteFactory? target) =>
      _names[target?.code ?? ''] ??
      (target == null ? thisMacName : target.label);

  /// Calls [target] (null for this Mac's) [name] on this device; an empty
  /// name goes back to its machine's.
  void rename(RemoteFactory? target, String name) {
    final key = target?.code ?? '';
    final trimmed = name.trim();
    final original = target == null ? thisMacName : target.label;
    if (trimmed.isEmpty || trimmed == original) {
      if (_names.remove(key) == null) return;
    } else {
      if (_names[key] == trimmed) return;
      _names[key] = trimmed;
    }
    _changed();
  }

  /// [text] as a QR code, made on this device; null where it can't be.
  Future<QrModules?> qrCode(String text) async {
    try {
      return await _storage.qrCode(text);
    } catch (error) {
      developer.log('could not make a QR code: $error', name: 'factory');
      return null;
    }
  }

  /// Shows [target] from now on (null for this Mac's), and remembers it:
  /// a new one joins the end of [saved], one already there keeps its place.
  void use(RemoteFactory? target) {
    if (target != null && !_saved.contains(target)) _saved.add(target);
    // The one kept, not [target]: it may come under the person's name.
    _current = target == null
        ? null
        : _saved.firstWhere((other) => other == target);
    _changed();
  }

  /// Forgets [target]; if it's the one showing, this Mac's shows instead.
  void forget(RemoteFactory target) {
    _saved.removeWhere((other) => other == target);
    _keys.remove(target.code);
    _names.remove(target.code);
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
            names: Map.of(_names),
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
    this.names = const {},
  });

  /// The code of the Factory showing; empty for this Mac's.
  final String current;
  final List<String> saved;

  /// This device's key by pairing code.
  final Map<String, String> keys;

  /// What the person calls each Factory here, by pairing code ('' for this
  /// Mac's).
  final Map<String, String> names;
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
      names: {
        for (final MapEntry(:key, :value)
            in (kept?['names'] as Map<Object?, Object?>? ?? const {}).entries)
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
        'names': connections.names,
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
  StoredConnections kept = const StoredConnections(current: '', saved: []);

  @override
  Future<StoredConnections> read() async => kept;

  @override
  Future<void> write(StoredConnections connections) async => kept = connections;

  @override
  Future<QrModules?> qrCode(String text) async => null;
}
