import 'dart:async';
import 'dart:developer' as developer;
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'device_account.dart';
import 'ipad/ink_bridge.dart' show InkJournal;
import 'tunnel_client.dart' show newDeviceKey;

/// A Factory on another machine, as its pairing code describes it. Since
/// the private tunnel: `factory://?t=<token>&name=<name>&e=<endpoint>`
/// (its iroh endpoint id), then `&r=<relay URL>` for each relay it can be
/// reached through and `&a=<ip:port>` for each direct address, and its
/// `&color=`. Before it:
/// `factory://<host>:<port>?t=<token>&fp=<SHA-256>&name=<machine>`, with
/// its certificate's SHA-256.
///
/// One found through the person's account has no token: this device shows
/// a Claktory its account instead (Hello.assertion). Its code is made here,
/// `factory://?account=<claktory id>&e=…&name=…`, and kept like any other.
@immutable
class RemoteFactory {
  const RemoteFactory({
    required this.code,
    required this.host,
    required this.port,
    required this.token,
    required this.fingerprint,
    required this.name,
    this.color = '',
    this.endpointId = '',
    this.relays = const [],
    this.addresses = const [],
    this.accountId = '',
  });

  /// One of the person's Claktories, found through their account.
  factory RemoteFactory.fromAccount(AccountClaktory claktory) {
    final code = Uri(
      scheme: 'factory',
      host: '',
      queryParameters: {
        'account': claktory.id,
        'e': claktory.endpointId,
        if (claktory.relays.isNotEmpty) 'r': claktory.relays,
        'name': claktory.name,
        if (claktory.color.isNotEmpty) 'color': claktory.color,
      },
    ).toString();
    return parsePairingCode(code, fromAccount: true)!;
  }

  /// The pairing code, as pasted: what's kept on the device.
  final String code;
  final String host;
  final int port;

  /// This device's key for it.
  final String token;

  /// The SHA-256 of its certificate, lowercase hex: the only certificate
  /// this app accepts from it.
  final String fingerprint;

  /// Its name when the code was made (the Claktory's, or before names
  /// its machine's, e.g. "mac-mini"). Once connected, its Settings say.
  final String name;

  /// Its color when the code was made (an agent color's name), or empty.
  final String color;

  /// Its iroh endpoint id: the app reaches it only through the private
  /// tunnel, dialled by this key. Empty in a code from before the tunnel,
  /// which connects to [endpoint] directly.
  final String endpointId;

  /// Relays it can be reached through; none: iroh's defaults.
  final List<String> relays;

  /// Direct UDP addresses it listens on (`ip:port`), tried as well as the
  /// relays: this Mac's own code has only these.
  final List<String> addresses;

  /// The Claktory's id with the person's account (`claktory_id`) when it
  /// was found there, connected to without a code; empty for a code's.
  final String accountId;

  /// Found through the account: it lets this device in on its account.
  bool get fromAccount => accountId.isNotEmpty;

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

/// The Factory a pairing code points at; null if [text] isn't one. A
/// code this app made for a Claktory found through the account is one
/// only when [fromAccount] (as kept on this device), never as pasted.
RemoteFactory? parsePairingCode(String text, {bool fromAccount = false}) {
  final code = text.trim();
  final Uri uri;
  final Map<String, String> query;
  final Map<String, List<String>> queryAll;
  try {
    uri = Uri.parse(code);
    // Decoding the query throws on a broken escape (%E0%A4, say), which a
    // garbled paste has: that's not a code, rather than an error.
    query = uri.queryParameters;
    queryAll = uri.queryParametersAll;
  } on FormatException {
    return null;
  }
  final token = query['t'] ?? '';
  final fingerprint = query['fp'] ?? '';
  final endpointId = (query['e'] ?? '').toLowerCase();
  final relays = [
    for (final relay in queryAll['r'] ?? const <String>[])
      if (Uri.tryParse(relay)?.scheme case 'https' || 'http') relay,
  ];
  final addresses = [
    for (final address in queryAll['a'] ?? const <String>[])
      if (_socketAddress.hasMatch(address)) address,
  ];
  final accountId = fromAccount ? query['account'] ?? '' : '';
  if (accountId.isNotEmpty) {
    if (uri.scheme != 'factory' || !_endpointId.hasMatch(endpointId)) {
      return null;
    }
    return RemoteFactory(
      code: code,
      host: '',
      port: 0,
      token: '',
      fingerprint: '',
      name: query['name'] ?? '',
      color: query['color'] ?? '',
      endpointId: endpointId,
      relays: relays,
      accountId: accountId,
    );
  }
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
    name: query['name'] ?? '',
    color: query['color'] ?? '',
    endpointId: endpointId,
    relays: relays,
    addresses: addresses,
  );
}

/// What this Mac's own Factory is called until it says its name.
const thisMacName = 'This Mac';

/// Every Factory this app connects to (this Mac's, and the ones on other
/// machines it's paired with, all live at once), and which one the window
/// shows. Kept on this device, the third
/// thing besides the window's frame and the drafts (the person's decision):
/// a device can't ask a backend where its backend is.
class Connections extends ChangeNotifier {
  Connections(
    this._storage, [
    this._current,
    List<RemoteFactory>? saved,
    Map<String, String>? keys,
    DeviceAccount? account,
  ]) : _saved = saved ?? [],
       _keys = keys ?? {},
       account = account ?? DeviceAccount() {
    this.account.onKept = _changed;
  }

  /// Reads what this device keeps; a code that isn't one is left out.
  /// When the store itself can't be read, the Connections are empty and
  /// [readFailure] says why: nothing is written over the store until
  /// it's read after all or the person starts over ([startOver]).
  static Future<Connections> load(ConnectionStorage storage) async {
    final StoredConnections kept;
    try {
      kept = await storage.read();
    } on ConnectionStorageFailure catch (failure) {
      developer.log('could not read connections: $failure', name: 'factory');
      return Connections(storage).._readFailure = failure;
    } catch (error) {
      developer.log('could not read connections: $error', name: 'factory');
      return Connections(storage)
        .._readFailure = ConnectionStorageFailure('', '$error');
    }
    RemoteFactory? parse(String code) =>
        parsePairingCode(code, fromAccount: true);
    final saved = [for (final code in kept.saved) ?parse(code)];
    final current = parse(kept.current);
    if (current != null && !saved.contains(current)) saved.add(current);
    bool known(String code) => saved.any((target) => target.code == code);
    return Connections(storage, current, saved, {
      for (final MapEntry(:key, :value) in kept.keys.entries)
        if (known(key)) key: value,
    }, DeviceAccount.kept(kept.account));
  }

  final ConnectionStorage _storage;

  /// Why what this device keeps couldn't be read; null when it was. While
  /// it's set nothing is written, so a store that may still hold the
  /// device's pairings and account is never replaced by what's in memory.
  ConnectionStorageFailure? get readFailure => _readFailure;
  ConnectionStorageFailure? _readFailure;

  /// Why the last change couldn't be kept; null once one is. What shows is
  /// right until the app starts again, which reads the last kept.
  final writeFailure = ValueNotifier<ConnectionStorageFailure?>(null);

  /// Forgets what this device kept, when it couldn't be read and the
  /// person chooses to start over: the store is empty, then written to
  /// again. Throws [ConnectionStorageFailure] when it can't.
  Future<void> startOver() async {
    await _storage.reset();
    // Drawings an iPad kept unsaved go with what it forgets.
    unawaited(const InkJournal().clear());
    _readFailure = null;
    _current = null;
    _saved.clear();
    _keys.clear();
    _changed();
  }

  /// Writes what's in memory now, as after a change: again after a failed
  /// write.
  void keep() => _changed();

  RemoteFactory? _current;
  final List<RemoteFactory> _saved;

  /// This device's iroh key for each Factory it reaches through the
  /// tunnel, by pairing code: made here, never sent anywhere. One per
  /// pairing, so two Factories can't tell it's the same device.
  final Map<String, String> _keys;

  /// This device signed in to the person's Claktory account, to find
  /// their Claktories and show them it's theirs. Kept with the rest.
  final DeviceAccount account;

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

  /// Every other Factory this device has connected to, oldest first. All
  /// of them are connected at once.
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

  /// Shows [target] from now on (null for this Mac's), and remembers it:
  /// a new one joins the end of [saved], one already there keeps its place.
  void use(RemoteFactory? target) {
    if (target != null && !_saved.contains(target)) _saved.add(target);
    _current = target == null
        ? null
        : _saved.firstWhere((other) => other == target);
    _changed();
  }

  /// Forgets [target]; if it's the one showing, this Mac's shows instead.
  void forget(RemoteFactory target) {
    _saved.removeWhere((other) => other == target);
    _keys.remove(target.code);
    // Its drawings an iPad kept unsaved (a Claktory without an endpoint
    // shares the journal's 'local' key, so it's left alone).
    if (target.tunnelled) {
      unawaited(
        const InkJournal().clear(
          claktory: InkJournal.claktoryKey(target.endpointId),
        ),
      );
    }
    if (_current == target) _current = null;
    _changed();
  }

  void _changed() {
    notifyListeners();
    if (_readFailure != null) {
      developer.log(
        "not keeping connections: what's kept couldn't be read",
        name: 'factory',
      );
      return;
    }
    unawaited(_write());
  }

  /// Writes what's in memory; the calls reach the store in order.
  Future<void> _write() async {
    try {
      await _storage.write(
        StoredConnections(
          current: _current?.code ?? '',
          saved: [for (final target in _saved) target.code],
          keys: Map.of(_keys),
          account: account.toKept(),
        ),
      );
      writeFailure.value = null;
    } on ConnectionStorageFailure catch (failure) {
      developer.log('could not keep connections: $failure', name: 'factory');
      writeFailure.value = failure;
    } catch (error) {
      developer.log('could not keep connections: $error', name: 'factory');
      writeFailure.value = ConnectionStorageFailure('', '$error');
    }
  }

  @override
  void dispose() {
    writeFailure.dispose();
    super.dispose();
  }
}

/// What went wrong reading, writing or clearing what this device keeps, as
/// the native store says it (`factory/device`'s error codes, the same on
/// every platform).
@immutable
class ConnectionStorageFailure implements Exception {
  const ConnectionStorageFailure(this.code, this.message);

  /// `credentials_unavailable` (for now: the device locked, the store
  /// busy), `credentials_unreadable` (there, but it can't be made sense
  /// of), `credentials_key_missing` or `credentials_key_invalidated` (the
  /// key that protects it is gone), `credentials_newer_version` (a newer
  /// app wrote it), `credentials_read_failed` (a write refused after a
  /// failed read), `credentials_write_failed`, `credentials_reset_failed`;
  /// empty for anything else.
  final String code;

  /// What the device said.
  final String message;

  /// It may well work in a moment (the device unlocked), so it's tried
  /// again by itself.
  bool get transient => code == 'credentials_unavailable';

  @override
  String toString() =>
      'ConnectionStorageFailure(${code.isEmpty ? 'unknown' : code}: $message)';
}

/// What [ConnectionStorage] keeps: pairing codes, and this device's iroh
/// key for each.
@immutable
class StoredConnections {
  const StoredConnections({
    required this.current,
    required this.saved,
    this.keys = const {},
    this.account = const {},
  });

  /// The code of the Factory showing; empty for this Mac's.
  final String current;
  final List<String> saved;

  /// This device's key by pairing code.
  final Map<String, String> keys;

  /// This device's account key, and the account it's signed in to
  /// ([DeviceAccount.toKept]).
  final Map<String, String> account;
}

/// Where [Connections] are kept, and how a pairing code becomes a QR code.
/// Each throws [ConnectionStorageFailure] when it fails.
abstract interface class ConnectionStorage {
  Future<StoredConnections> read();
  Future<void> write(StoredConnections connections);

  /// Clears what's kept, the person having chosen to start over after it
  /// couldn't be read: reads then find nothing, and writes work again.
  Future<void> reset();

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

  /// [call], its PlatformException as the store's failure.
  static Future<T> _store<T>(Future<T> Function() call) async {
    try {
      return await call();
    } on PlatformException catch (error) {
      throw ConnectionStorageFailure(
        error.code,
        error.message ?? error.details?.toString() ?? '',
      );
    } on MissingPluginException catch (error) {
      throw ConnectionStorageFailure('', error.message ?? 'no store here');
    }
  }

  @override
  Future<StoredConnections> read() async {
    final kept = await _store(
      () => _channel.invokeMapMethod<String, Object?>('connections'),
    );
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
      account: {
        for (final MapEntry(:key, :value)
            in (kept?['account'] as Map<Object?, Object?>? ?? const {}).entries)
          if (key is String && value is String) key: value,
      },
    );
  }

  @override
  Future<void> write(StoredConnections connections) => _store(
    () => _channel.invokeMethod<void>('setConnections', {
      'current': connections.current,
      'saved': connections.saved,
      'keys': connections.keys,
      'account': connections.account,
    }),
  );

  @override
  Future<void> reset() =>
      _store(() => _channel.invokeMethod<void>('resetConnections'));

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
/// [readFails], [writeFails] and [resetFails] make each fail so.
class MemoryConnectionStorage implements ConnectionStorage {
  StoredConnections kept = const StoredConnections(current: '', saved: []);
  ConnectionStorageFailure? readFails;
  ConnectionStorageFailure? writeFails;
  ConnectionStorageFailure? resetFails;
  var writes = 0;
  var resets = 0;

  @override
  Future<StoredConnections> read() async {
    if (readFails case final failure?) throw failure;
    return kept;
  }

  @override
  Future<void> write(StoredConnections connections) async {
    if (writeFails case final failure?) throw failure;
    writes++;
    kept = connections;
  }

  @override
  Future<void> reset() async {
    if (resetFails case final failure?) throw failure;
    resets++;
    readFails = null;
    kept = const StoredConnections(current: '', saved: []);
  }

  @override
  Future<QrModules?> qrCode(String text) async => null;
}
