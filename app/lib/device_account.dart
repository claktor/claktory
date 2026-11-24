import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart' show sha256;

import 'tunnel/api/account.dart' as rust;

import 'package:flutter/foundation.dart';

import 'tunnel_client.dart' show RustTunnels, newDeviceKey;

/// This device signed in to the person's Claktory account (claktory.com),
/// on its own: it finds their Claktories there and gets the assertion a
/// Claktory takes in place of a pairing code. The account service's
/// "Devices" calls (crates/claktory-account/API.md).

/// claktory.com's account service.
const defaultAccountService = 'https://account.claktory.com';

/// Ed25519 with this device's account key: made on the device and kept
/// with its connections, its public half the device's name with the
/// account. In Rust (app/rust), loaded on first use.
abstract interface class AccountKeys {
  /// The public half of [key] (32 bytes).
  Future<Uint8List> publicKey(String key);

  /// [message] signed with [key] (64 bytes).
  Future<Uint8List> sign(String key, List<int> message);
}

class RustAccountKeys implements AccountKeys {
  const RustAccountKeys();

  @override
  Future<Uint8List> publicKey(String key) async {
    await RustTunnels.load();
    return rust.accountPublicKey(key: key);
  }

  @override
  Future<Uint8List> sign(String key, List<int> message) async {
    await RustTunnels.load();
    return rust.accountSign(key: key, message: message);
  }
}

/// Base64url without padding, as the account service writes binary values.
String b64(List<int> bytes) => base64Url.encode(bytes).replaceAll('=', '');

/// What a device's key signs for a request: the same text a Claktory's
/// backend key signs (API.md, "Keys and signatures").
List<int> requestMessage(
  String method,
  String pathAndQuery,
  int time,
  List<int> body,
) => utf8.encode(
  'claktory-account request v1\n$method\n$pathAndQuery\n$time\n'
  '${sha256.convert(body)}',
);

/// The account service said no: [code] is what to act on ("wrong_code"),
/// [message] is for people, [details] the rest of its answer.
class AccountServiceError implements Exception {
  const AccountServiceError(
    this.status,
    this.code,
    this.message, [
    this.details = const {},
  ]);

  final int status;
  final String code;
  final String message;
  final Map<String, Object?> details;

  /// Tries left after a wrong code, if it said.
  int? get attemptsLeft => switch (details['attempts_left']) {
    final int left => left,
    _ => null,
  };

  @override
  String toString() => 'AccountServiceError($status $code: $message)';
}

/// The account service couldn't be reached, or answered with something
/// that isn't its API.
class AccountServiceUnreachable implements Exception {
  const AccountServiceUnreachable(this.reason);

  final String reason;

  @override
  String toString() => 'AccountServiceUnreachable($reason)';
}

/// A sign-in under way: the code was emailed.
typedef DeviceSignInStarted = ({
  String requestId,
  Duration expiresIn,
  String codeFrom,
});

/// Whose account this device is signed in to.
typedef DeviceAccountIdentity = ({String accountId, String email, String name});

/// One of the account's Claktories, as the service last heard from it.
class AccountClaktory {
  const AccountClaktory({
    required this.id,
    required this.name,
    this.color = '',
    this.machine = '',
    this.platform = '',
    this.version = '',
    this.endpointId = '',
    this.relays = const [],
    this.lastContact,
  });

  factory AccountClaktory.fromJson(Map<String, Object?> json) {
    String text(String key) => switch (json[key]) {
      final String value => value,
      _ => '',
    };
    final lastContact = json['last_contact_ms'];
    return AccountClaktory(
      id: text('claktory_id'),
      name: text('name'),
      color: text('color'),
      machine: text('machine'),
      platform: text('platform'),
      version: text('version'),
      endpointId: text('endpoint_id').toLowerCase(),
      relays: [
        for (final relay in json['relays'] as List? ?? const [])
          if (relay is String) relay,
      ],
      lastContact: lastContact is int && lastContact > 0
          ? DateTime.fromMillisecondsSinceEpoch(lastContact)
          : null,
    );
  }

  /// Made by the Claktory, the same on every device.
  final String id;
  final String name;

  /// An agent color's name, or empty.
  final String color;

  /// The machine it runs on ("Mac mini").
  final String machine;
  final String platform;
  final String version;

  /// Its iroh endpoint id, 64 hex characters; empty when it can't be
  /// reached through the tunnel (it hasn't said where it is).
  final String endpointId;
  final List<String> relays;

  /// When it last checked in with the service, if ever.
  final DateTime? lastContact;

  /// How long since its last check-in it still counts as online: it
  /// checks in about once an hour.
  static const onlineWithin = Duration(minutes: 70);

  bool get reachable => endpointId.isNotEmpty;

  bool online(DateTime now) {
    final last = lastContact;
    return last != null && now.difference(last) <= onlineWithin;
  }
}

/// This device's calls to the account service, signed with its account
/// key [accountKey] (lowercase hex, 32 bytes).
class DeviceAccountService {
  DeviceAccountService({
    required this.accountKey,
    String service = defaultAccountService,
    this.keys = const RustAccountKeys(),
    HttpClient Function()? client,
    DateTime Function()? now,
  }) : service = Uri.parse(service),
       _client = client ?? HttpClient.new,
       _now = now ?? DateTime.now;

  final Uri service;
  final String accountKey;
  final AccountKeys keys;
  final HttpClient Function() _client;
  final DateTime Function() _now;

  /// The service's clock less this device's, learned from a clock_skew
  /// refusal, so a device with its clock off still signs in.
  Duration _skew = Duration.zero;

  /// Each signed request's last time, by what else it signs, for the last
  /// 5 minutes (see [_stamp]).
  final _stamped = <String, int>{};

  /// The time to sign a request with: [now], or a second past the last
  /// time this device signed the same request. Signatures are the same for
  /// the same request in the same second, so several callers asking at once
  /// (the account's Claktories, when the service is back after a restart)
  /// would be refused as replays. Never more than a minute ahead.
  int _stamp(int now, String method, String pathAndQuery, List<int> bytes) {
    _stamped.removeWhere((_, time) => time <= now - 300);
    final key = '$method\n$pathAndQuery\n${base64.encode(bytes)}';
    final last = _stamped[key];
    final time = last != null && last >= now && last < now + 60
        ? last + 1
        : now;
    _stamped[key] = time;
    return time;
  }

  static const _timeout = Duration(seconds: 20);

  /// Emails [email] a code that signs this device in to its account, as
  /// [name] ("Matt's iPhone") on [platform] ("ios"), app [version].
  Future<DeviceSignInStarted> start({
    required String email,
    required String name,
    required String platform,
    required String version,
  }) async {
    // Signed by the key it names, like verify (row 671): only then does
    // the service say the key is another account's, so that only its
    // holder learns it.
    final answer = await _call('POST', '/v1/devices/sign-in/start', {
      'email': email,
      'account_key': b64(await keys.publicKey(accountKey)),
      'name': name,
      'platform': platform,
      'version': version,
    });
    return (
      requestId: answer['request_id'] as String? ?? '',
      expiresIn: Duration(seconds: answer['expires_in'] as int? ?? 900),
      codeFrom: answer['code_from'] as String? ?? '',
    );
  }

  /// Finishes the sign-in [requestId] with the emailed [code].
  Future<({String accountId, String email})> verify({
    required String requestId,
    required String code,
  }) async {
    final answer = await _call('POST', '/v1/devices/sign-in/verify', {
      'request_id': requestId,
      'code': code,
    });
    return (
      accountId: answer['account_id'] as String? ?? '',
      email: answer['email'] as String? ?? '',
    );
  }

  /// Whose account this device is signed in to; null when it isn't (or
  /// was signed out from elsewhere).
  Future<DeviceAccountIdentity?> me() async {
    try {
      final answer = await _call('GET', '/v1/devices/me', null);
      return (
        accountId: answer['account_id'] as String? ?? '',
        email: answer['email'] as String? ?? '',
        name: answer['name'] as String? ?? '',
      );
    } on AccountServiceError catch (error) {
      if (error.status == 401 && error.code == 'unauthorized') return null;
      rethrow;
    }
  }

  Future<void> signOut() => _call('POST', '/v1/devices/sign-out', {});

  /// The account's Claktories.
  Future<List<AccountClaktory>> claktories() async {
    final answer = await _call('GET', '/v1/claktories', null);
    return [
      for (final claktory in answer['claktories'] as List? ?? const [])
        if (claktory is Map<String, Object?>)
          AccountClaktory.fromJson(claktory),
    ];
  }

  /// What this device shows a Claktory, in its Hello, to say it's signed
  /// in to the same account: for the tunnel's device key [deviceKey] (its
  /// endpoint id, 64 hex characters). It lasts [expiresIn].
  Future<({String assertion, Duration expiresIn})> assertion(
    String deviceKey,
  ) async {
    final answer = await _call('POST', '/v1/devices/assertion', {
      'device_key': deviceKey,
    });
    return (
      assertion: answer['assertion'] as String? ?? '',
      expiresIn: Duration(seconds: answer['expires_in'] as int? ?? 3600),
    );
  }

  /// A signed call to another of the service's device routes, such as an
  /// account-level app's (user_apps/session.dart): [body] as JSON, none
  /// for a GET. Answers the answer's JSON; throws [AccountServiceError] or
  /// [AccountServiceUnreachable].
  Future<Map<String, Object?>> signed(
    String method,
    String path, [
    Map<String, Object?>? body,
  ]) => _call(method, path, body);

  /// One call, [body] as JSON (none for a GET). A signed call refused for
  /// the clock or as a replay signs again once.
  Future<Map<String, Object?>> _call(
    String method,
    String path,
    Map<String, Object?>? body, {
    bool signed = true,
  }) async {
    try {
      return await _send(method, path, body, signed: signed);
    } on AccountServiceError catch (error) {
      if (!signed || error.status != 401) rethrow;
      switch (error.code) {
        case 'clock_skew':
          final serverTime = error.details['server_time'];
          if (serverTime is! int) rethrow;
          _skew = Duration(
            milliseconds: serverTime * 1000 - _now().millisecondsSinceEpoch,
          );
        case 'replayed':
          // Signatures are the same within a second: the next one's.
          await Future<void>.delayed(const Duration(milliseconds: 1100));
        default:
          rethrow;
      }
      return _send(method, path, body, signed: signed);
    }
  }

  Future<Map<String, Object?>> _send(
    String method,
    String path,
    Map<String, Object?>? body, {
    required bool signed,
  }) async {
    final uri = service.resolve(path);
    final bytes = body == null ? <int>[] : utf8.encode(jsonEncode(body));
    final client = _client();
    try {
      final request = await client.openUrl(method, uri).timeout(_timeout);
      request.headers.set(HttpHeaders.acceptHeader, 'application/json');
      if (body != null) {
        request.headers.contentType = ContentType.json;
        request.contentLength = bytes.length;
      }
      if (signed) {
        final pathAndQuery = uri.hasQuery
            ? '${uri.path}?${uri.query}'
            : uri.path;
        final time = _stamp(
          (_now().add(_skew).millisecondsSinceEpoch / 1000).floor(),
          method,
          pathAndQuery,
          bytes,
        );
        request.headers
          ..set(
            HttpHeaders.authorizationHeader,
            'Device ${b64(await keys.publicKey(accountKey))}',
          )
          ..set('Claktory-Time', '$time')
          ..set(
            'Claktory-Signature',
            b64(
              await keys.sign(
                accountKey,
                requestMessage(method, pathAndQuery, time, bytes),
              ),
            ),
          );
      }
      request.add(bytes);
      final response = await request.close().timeout(_timeout);
      final text = await utf8.decodeStream(response).timeout(_timeout);
      final Object? json;
      try {
        json = text.isEmpty ? <String, Object?>{} : jsonDecode(text);
      } on FormatException {
        throw AccountServiceUnreachable(
          'HTTP ${response.statusCode}, not JSON',
        );
      }
      if (json is! Map<String, Object?>) {
        throw AccountServiceUnreachable('HTTP ${response.statusCode}');
      }
      if (response.statusCode >= 200 && response.statusCode < 300) {
        return json;
      }
      final code = json['error'];
      if (code is! String) {
        throw AccountServiceUnreachable('HTTP ${response.statusCode}');
      }
      throw AccountServiceError(
        response.statusCode,
        code,
        json['message'] as String? ?? '',
        json,
      );
    } on SocketException catch (error) {
      throw AccountServiceUnreachable(error.message);
    } on HttpException catch (error) {
      throw AccountServiceUnreachable(error.message);
    } on HandshakeException catch (error) {
      throw AccountServiceUnreachable(error.message);
    } on TimeoutException {
      throw const AccountServiceUnreachable('timed out');
    } finally {
      client.close(force: true);
    }
  }
}

/// This device signed in to the person's Claktory account: to find their
/// Claktories, connect to one without a code, and show each it connects to
/// that it's theirs (an assertion in its Hello). Kept with the device's
/// connections ([Connections.account]): its account key, the service and
/// the email it's signed in with. A sign-in under way is kept only while
/// the app runs.
class DeviceAccount extends ChangeNotifier {
  DeviceAccount({
    this._key = '',
    this._service = '',
    this._email = '',
    DeviceAccountService Function(String key, String service)? open,
    String Function()? makeKey,
    DateTime Function()? now,
  }) : _openService = open ?? _defaultService,
       _makeKey = makeKey ?? newAccountKey,
       _now = now ?? DateTime.now;

  /// What [toKept] kept.
  factory DeviceAccount.kept(Map<String, String> kept) => DeviceAccount(
    key: kept['key'] ?? '',
    service: kept['service'] ?? '',
    email: kept['email'] ?? '',
  );

  static DeviceAccountService _defaultService(String key, String service) =>
      DeviceAccountService(accountKey: key, service: service);

  /// What this device keeps: [StoredConnections.account].
  Map<String, String> toKept() => {
    if (_key.isNotEmpty) 'key': _key,
    if (_service.isNotEmpty) 'service': _service,
    if (_email.isNotEmpty) 'email': _email,
  };

  /// Called when what's kept changed, to keep it.
  VoidCallback? onKept;

  String _key;
  String _service;
  String _email;
  final DeviceAccountService Function(String key, String service) _openService;
  final String Function() _makeKey;
  final DateTime Function() _now;

  DeviceAccountService? _client;

  /// The email this device is signed in with; empty when it isn't.
  String get email => _email;
  bool get signedIn => _email.isNotEmpty;

  /// The account service it signs in with.
  String get service => _service.isEmpty ? defaultAccountService : _service;

  /// The code was emailed and this device waits for it; null otherwise.
  DeviceCodeWait? get awaitingCode => _awaiting;
  DeviceCodeWait? _awaiting;

  /// Assertions by tunnel key, until shortly before they expire.
  final _assertions = <String, ({String assertion, DateTime until})>{};

  DeviceAccountService get _api {
    final client = _client;
    if (client != null &&
        client.accountKey == _key &&
        client.service.toString() == Uri.parse(service).toString()) {
      return client;
    }
    return _client = _openService(_key, service);
  }

  /// Emails [email] a code that signs this device in, with the account
  /// service at [service] (empty: claktory.com's), as [name] on
  /// [platform], app [version].
  Future<void> start({
    required String email,
    String service = '',
    required String name,
    required String platform,
    required String version,
  }) async {
    if (_key.isEmpty) _key = _makeKey();
    if (!signedIn) _service = service == defaultAccountService ? '' : service;
    _kept();
    Future<DeviceSignInStarted> send() => _unlessRemoved(
      () => _api.start(
        email: email.trim(),
        name: name,
        platform: platform,
        version: version,
      ),
    );
    DeviceSignInStarted started;
    try {
      started = await send();
    } on AccountServiceError catch (error) {
      // Its key is another account's: with the new key [_unlessRemoved]
      // made, once more, as a new device (row 671, Fugo).
      if (!_anotherAccounts(error)) rethrow;
      started = await send();
    }
    _awaiting = DeviceCodeWait(
      email: email.trim(),
      requestId: started.requestId,
      codeFrom: started.codeFrom,
      expires: _now().add(started.expiresIn),
    );
    notifyListeners();
  }

  /// Signs in with the emailed [code]. A wrong one throws
  /// [AccountServiceError] (`wrong_code`, with the tries left).
  Future<void> verify(String code) async {
    final awaiting = _awaiting;
    if (awaiting == null) {
      throw const AccountServiceError(410, 'code_expired', '');
    }
    final signedIn = await _unlessRemoved(
      () => _api.verify(requestId: awaiting.requestId, code: code.trim()),
    );
    _awaiting = null;
    _email = signedIn.email.isNotEmpty ? signedIn.email : awaiting.email;
    _removed = false;
    _assertions.clear();
    _kept();
  }

  /// Gives up on the code it waits for (another email, say).
  void cancelSignIn() {
    _awaiting = null;
    notifyListeners();
  }

  /// Signs this device out: at the service if it can be reached, and here
  /// in any case. [newKey]: it left the person's messages (row 671), so
  /// their other devices take its key out of the account; the next
  /// sign-in is a new device, with a new key.
  Future<void> signOut({bool newKey = false}) async {
    try {
      if (_key.isNotEmpty) await _api.signOut();
    } on AccountServiceError {
      // Signed out there already.
    } on AccountServiceUnreachable {
      // Its key stays bound there until removed at claktory.com/account.
    } finally {
      if (newKey) {
        _key = _makeKey();
        _client = null;
      }
      _signedOut();
    }
  }

  /// The account's Claktories.
  Future<List<AccountClaktory>> claktories() =>
      _whileSignedIn(() => _api.claktories());

  /// An assertion for the tunnel key [tunnelKey] (its endpoint id, hex),
  /// to show a Claktory in a Hello; the one got earlier while it lasts.
  Future<String> assertionFor(String tunnelKey) async {
    final kept = _assertions[tunnelKey];
    if (kept != null && _now().isBefore(kept.until)) return kept.assertion;
    final made = await _whileSignedIn(() => _api.assertion(tunnelKey));
    // Five minutes to spare: a connection takes a moment to arrive.
    _assertions[tunnelKey] = (
      assertion: made.assertion,
      until: _now().add(made.expiresIn - const Duration(minutes: 5)),
    );
    return made.assertion;
  }

  /// A signed call to one of the service's device routes (an
  /// account-level app's), as this device; signs it out here when the
  /// service no longer has it signed in.
  Future<Map<String, Object?>> signed(
    String method,
    String path, [
    Map<String, Object?>? body,
  ]) => _whileSignedIn(() => _api.signed(method, path, body));

  /// Who this device is at the service: its account, its name there, and
  /// its key as the service writes it (base64url), which is also its leaf
  /// id in direct messages. Null when the service no longer has it signed
  /// in.
  Future<({String accountId, String name, String deviceKey})?> whoAmI() =>
      _whileSignedIn(() async {
        final api = _api;
        final me = await api.me();
        if (me == null) return null;
        return (
          accountId: me.accountId,
          name: me.name,
          deviceKey: b64(await api.keys.publicKey(_key)),
        );
      });

  /// Whether the service still has this device signed in (it can be
  /// removed at claktory.com/account); signs it out here if not.
  Future<void> check() async {
    if (!signedIn) return;
    final me = await _whileSignedIn(() => _api.me());
    if (me == null) {
      _signedOut();
    } else if (me.email.isNotEmpty && me.email != _email) {
      _email = me.email;
      _kept();
    }
  }

  Future<T> _whileSignedIn<T>(Future<T> Function() call) async {
    if (!signedIn) {
      throw const AccountServiceError(401, 'unauthorized', 'Not signed in.');
    }
    try {
      return await call();
    } on AccountServiceError catch (error) {
      if (error.status == 401 && error.code == 'unauthorized') {
        // Signed out from elsewhere, or removed: the apps behind the
        // service (DMs' worker) don't say which, so the service is asked.
        if (await _removedHere()) _removed = true;
        _signedOut();
      }
      // Another of the person's devices removed this one (row 199): signed
      // out here at once, saying so. The key stays until the next sign-in,
      // which the service refuses the same way, and which then starts over
      // as a new device ([_unlessRemoved]).
      if (error.status == 403 && error.code == 'device_removed') {
        _removed = true;
        _signedOut();
      }
      rethrow;
    }
  }

  /// Whether the service says this device's key was removed from the
  /// account; false when it says anything else, or can't be asked.
  Future<bool> _removedHere() async {
    try {
      await _api.me();
      return false;
    } on AccountServiceError catch (error) {
      return error.status == 403 && error.code == 'device_removed';
    } catch (_) {
      return false;
    }
  }

  /// Another of the person's devices removed this one, as the service
  /// said on a signed call (row 199): what People and chats say instead of
  /// a plain sign-in, until this device signs in again.
  bool get removed => _removed;
  var _removed = false;

  /// The account key this device signs in with (hex): a new one once
  /// another device removed this one, so it starts over as a new device.
  String get key => _key;

  /// [call] to sign in; when another of the person's devices removed this
  /// one (403 device_removed), its key can never sign in to that account
  /// again (Fugo): a new key, so the next sign-in starts over as a new
  /// device, and its messages with it (e2eeOf starts them over when [key]
  /// changes). The refusal goes on to say so. The same when its key was
  /// last another account's (403 key_of_another_account, row 671): a key,
  /// and the messages under it, belong to one account; [start] then tries
  /// once more by itself.
  Future<T> _unlessRemoved<T>(Future<T> Function() call) async {
    try {
      return await call();
    } on AccountServiceError catch (error) {
      if (error.status == 403 &&
          (error.code == 'device_removed' || _anotherAccounts(error))) {
        _key = _makeKey();
        _client = null;
        _signedOut();
      }
      rethrow;
    }
  }

  static bool _anotherAccounts(AccountServiceError error) =>
      error.status == 403 && error.code == 'key_of_another_account';

  void _signedOut() {
    _email = '';
    _awaiting = null;
    _assertions.clear();
    _kept();
  }

  void _kept() {
    onKept?.call();
    notifyListeners();
  }
}

/// A sign-in on this device waiting for the emailed code.
@immutable
class DeviceCodeWait {
  const DeviceCodeWait({
    required this.email,
    required this.requestId,
    required this.codeFrom,
    required this.expires,
  });

  final String email;
  final String requestId;

  /// The address the code comes from; empty when the service didn't say.
  final String codeFrom;
  final DateTime expires;
}

/// A new account key: 32 random bytes as lowercase hex, like a device key.
String newAccountKey() => newDeviceKey();
