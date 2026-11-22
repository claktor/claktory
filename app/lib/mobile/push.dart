// Notifications pushed to this iPhone while the app isn't in front (#273 B3,
// notifications.md §3 in Research).
//
// Each Claktory this iPhone connects to gets its own registration: the push
// relay gives this iPhone a handle for its push token, this iPhone makes a
// 32-byte key, and both go to that Claktory (RegisterNotifications). The
// Claktory seals what it says with the key; Apple only ever carries the
// ciphertext, and the notification extension opens it here
// (app/ios/NotificationService, Runner/Push.swift).
//
// The handle and key are kept on the device, with its keychain (Runner/
// Notifications.swift), and sent again on every connect: the Claktory may
// have lost them (the device removed and paired again), and the same ones
// again change nothing there. The relay is asked for a new handle only when
// the token, the relay or the platform changed, or after 30 days.
//
// What notifies, and when, is the backend's: this only gets the iPhone
// registered. The person is asked whether Claktory may notify them in
// context, the first time they send something from the iPhone
// ([PushRegistrar.askInContext]), never at launch.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../backend.dart' show Backend;
import '../diagnostics_record.dart'
    show NotificationsKey, NotificationsKind, PushStep;
import '../notifications.dart' show claktoryId;
import '../protocol/factory/v1/factory.pb.dart';
import '../rail.dart' show Claktories, Claktory;

/// Claktory's own push relay, when a Claktory's settings name none.
const defaultPushRelay = 'https://push.claktory.com';

/// The relay a dev build registers with (`--dart-define=FACTORY_PUSH_RELAY=`
/// a local or fake relay); never read by a release build.
const _devPushRelay = String.fromEnvironment('FACTORY_PUSH_RELAY');
const _dev = bool.fromEnvironment('FACTORY_DEV');

/// A dev build may reach a relay on this computer over plain http (a local
/// claktory-push for tests); every other relay is https, since the token
/// goes to it.
bool loopbackDevRelay(Uri relay, {bool dev = _dev}) =>
    dev &&
    relay.scheme == 'http' &&
    (relay.host == '127.0.0.1' || relay.host == 'localhost');

/// The relay this device registers with for a Claktory whose settings name
/// [configured] (empty: Claktory's own); empty when it registers with none.
/// A dev build never uses Claktory's own: it allows so many registrations
/// an hour from one address, and the person's iPhone shares this Mac's, so
/// a Simulator's tests could keep it from registering. A dev build uses
/// FACTORY_PUSH_RELAY, else a relay the Claktory names other than Claktory's
/// own, else none.
String pushRelayFor(
  String configured, {
  bool dev = _dev,
  String devRelay = _devPushRelay,
}) {
  if (!dev) return configured.isEmpty ? defaultPushRelay : configured;
  if (devRelay.isNotEmpty) return devRelay;
  final named = Uri.tryParse(configured);
  final production = Uri.parse(defaultPushRelay);
  if (configured.isEmpty || named?.host == production.host) return '';
  return configured;
}

/// How long a handle from the relay is used before it's asked for afresh.
const pushRefresh = Duration(days: 30);

/// Whether the person lets Claktory notify them, and this iPhone's push
/// token (empty until Apple gives one).
@immutable
class PushState {
  const PushState({
    required this.permission,
    required this.token,
    required this.platform,
  });

  /// "not_determined", "denied" or "allowed".
  final String permission;
  final String token;

  /// How pushes reach this device, as the relay and the Claktory name it:
  /// "apns", "apns_sandbox" (a build for development) or "fcm".
  final String platform;

  bool get allowed => permission == 'allowed';

  /// As the Claktory names it (RegisterNotifications.platform).
  PushPlatform get pushPlatform => switch (platform) {
    'apns' => PushPlatform.PUSH_PLATFORM_APNS,
    'apns_sandbox' => PushPlatform.PUSH_PLATFORM_APNS_SANDBOX,
    'fcm' => PushPlatform.PUSH_PLATFORM_FCM,
    _ => PushPlatform.PUSH_PLATFORM_UNSPECIFIED,
  };

  static PushState fromMap(Object? map) {
    final values = map is Map ? map : const {};
    return PushState(
      permission: values['permission'] as String? ?? 'not_determined',
      token: values['token'] as String? ?? '',
      platform:
          values['platform'] as String? ??
          (values['sandbox'] == true ? 'apns_sandbox' : 'apns'),
    );
  }
}

/// What this iPhone registered for one Claktory.
@immutable
class PushRegistration {
  const PushRegistration({
    required this.handle,
    required this.token,
    required this.relay,
    required this.platform,
    required this.registered,
    required this.key,
  });

  final String handle;
  final String token;
  final String relay;
  final String platform;
  final DateTime registered;
  final Uint8List key;

  static PushRegistration? fromMap(Object? map) {
    if (map is! Map) return null;
    final key = map['key'];
    final handle = map['handle'];
    if (key is! Uint8List || handle is! String || handle.isEmpty) return null;
    final registered = map['registered'];
    return PushRegistration(
      handle: handle,
      token: map['token'] as String? ?? '',
      relay: map['relay'] as String? ?? '',
      platform: map['platform'] as String? ?? '',
      registered: DateTime.fromMillisecondsSinceEpoch(
        registered is int ? registered : 0,
      ),
      key: key,
    );
  }
}

/// This iPhone's side of pushes: its permission, token and what it keeps
/// for each Claktory (Notifications.swift's factory/push); a fake in tests.
abstract interface class PushDevice {
  Future<PushState> state();

  /// Asks the person, the first time; allowed, the token follows on
  /// [tokens].
  Future<String> ask();

  /// The Claktories it keeps a key for, by id.
  Future<List<String>> kept();

  /// This iPhone's push token, as Apple gives it.
  Stream<PushState> get tokens;

  Future<PushRegistration?> registration(String claktory);

  /// Keeps [handle] for [claktory], with its key (made when there's none),
  /// and answers the key.
  Future<Uint8List> register(
    String claktory, {
    required String handle,
    required String token,
    required String relay,
    required String platform,
  });

  Future<void> forget(String claktory);
}

/// The push relay: a handle for a push token, and none any more.
abstract interface class PushRelay {
  Future<String> register(String relay, String platform, String token);

  /// [handle] no longer pushes: the relay forgets it, and the Claktory that
  /// pushes to it hears it's gone.
  Future<void> unregister(String relay, String handle);
}

/// Registers this iPhone with each Claktory it's connected to, once the
/// person allows notifications.
class PushRegistrar {
  /// The app's, for what asks the person in context
  /// ([askInContext]); none until the app makes it (an iPhone's).
  static PushRegistrar? current;

  PushRegistrar({
    required this.claktories,
    required this.device,
    required this.relay,
    DateTime Function()? now,
  }) : _now = now ?? DateTime.now {
    claktories.addListener(_follow);
    _tokens = device.tokens.listen((state) {
      _state = state;
      _syncAll();
    });
    _follow();
    unawaited(_start());
  }

  final Claktories claktories;
  final PushDevice device;

  /// Where setting up notifications last failed, by the connection to each
  /// Claktory it failed for, until it works: Settings → Notifications says
  /// so, with Try again ([retry]).
  final failures = ValueNotifier<Map<Backend, PushStep>>(const {});

  /// Sets notifications up with [backend]'s Claktory again, now.
  void retry(Backend backend) {
    for (final claktory in claktories.all) {
      if (identical(claktory.backend, backend)) _sync(claktory);
    }
  }

  void _failed(Backend backend, PushStep? step) {
    final now = {...failures.value};
    if (step == null) {
      if (now.remove(backend) == null) return;
    } else {
      if (now[backend] == step) return;
      now[backend] = step;
    }
    failures.value = now;
  }

  final PushRelay relay;
  final DateTime Function() _now;

  late final StreamSubscription<PushState> _tokens;
  PushState? _state;
  final _followed = <Claktory, VoidCallback>{};
  final _syncing = <Claktory, Future<void>>{};
  bool _asked = false;

  Future<void> _start() async {
    _state = await _safely(device.state);
    await _prune();
    _syncAll();
  }

  /// Keys of Claktories removed while the app wasn't running go, once the
  /// device's list of them was read.
  Future<void> _prune() async {
    final kept = await _safely(device.kept) ?? const <String>[];
    if (claktories.connections.readFailure != null) return;
    final ids = {for (final claktory in claktories.all) claktoryId(claktory)};
    for (final id in kept) {
      if (!ids.contains(id)) await _forget(id);
    }
  }

  /// Follows each Claktory's connection: registered on every connect.
  void _follow() {
    final open = claktories.all.toSet();
    for (final gone in [
      for (final claktory in _followed.keys)
        if (!open.contains(claktory)) claktory,
    ]) {
      gone.backend.connectedSince.removeListener(_followed.remove(gone)!);
      _forgetIfRemoved(gone);
    }
    for (final claktory in open) {
      if (_followed.containsKey(claktory)) continue;
      void connected() {
        if (claktory.backend.connectedSince.value != null) _sync(claktory);
      }

      _followed[claktory] = connected;
      claktory.backend.connectedSince.addListener(connected);
      connected();
    }
  }

  /// [gone] is no longer among this device's Claktories: the person
  /// removed it. Its key goes, so a push from it no longer opens, nor can
  /// a tap reach it. (Only when the device's list was read: one that
  /// couldn't be read isn't a removal.)
  void _forgetIfRemoved(Claktory gone) {
    final connections = claktories.connections;
    if (connections.readFailure != null) return;
    final id = claktoryId(gone);
    if (id == 'local') return;
    if (claktories.all.any((each) => claktoryId(each) == id)) return;
    unawaited(_forget(id));
  }

  /// [id]'s handle is unregistered at the relay (as best it can: offline,
  /// the key still goes), so its Claktory stops pushing to this device, then
  /// its key goes.
  Future<void> _forget(String id) async {
    final kept = await _safely(() => device.registration(id));
    if (kept != null && kept.relay.isNotEmpty) {
      await _safely(() => relay.unregister(kept.relay, kept.handle));
    }
    await _safely(() => device.forget(id));
  }

  void _syncAll() {
    for (final claktory in claktories.all) {
      if (claktory.backend.connectedSince.value != null) _sync(claktory);
    }
  }

  /// One at a time for each Claktory: a connect while one runs waits for
  /// it, then goes again.
  void _sync(Claktory claktory) {
    final before = _syncing[claktory] ?? Future<void>.value();
    late final Future<void> next;
    next = before.then((_) => _register(claktory)).whenComplete(() {
      if (identical(_syncing[claktory], next)) _syncing.remove(claktory);
    });
    _syncing[claktory] = next;
  }

  Future<void> _register(Claktory claktory) async {
    final state = _state;
    if (state == null || !state.allowed || state.token.isEmpty) return;
    final backend = claktory.backend;
    if (backend.connectedSince.value == null) return;
    final id = claktoryId(claktory);
    // This device's own Claktory: an iPhone has none.
    if (id == 'local') return;
    final settings = backend.workspace.workspace.settings.notifications;
    final relayUrl = pushRelayFor(settings.relay);
    if (relayUrl.isEmpty) {
      debugPrint(
        'a dev build registers for notifications only with '
        'FACTORY_PUSH_RELAY, never Claktory\'s own relay',
      );
      return;
    }
    var step = PushStep.other;
    var fresh = false;
    try {
      var kept = await device.registration(id);
      if (kept == null ||
          kept.token != state.token ||
          kept.relay != relayUrl ||
          kept.platform != state.platform ||
          _now().difference(kept.registered) > pushRefresh) {
        fresh = true;
        step = PushStep.relay;
        final handle = await relay.register(
          relayUrl,
          state.platform,
          state.token,
        );
        step = PushStep.key;
        final key = await device.register(
          id,
          handle: handle,
          token: state.token,
          relay: relayUrl,
          platform: state.platform,
        );
        kept = PushRegistration(
          handle: handle,
          token: state.token,
          relay: relayUrl,
          platform: state.platform,
          registered: _now(),
          key: key,
        );
      }
      step = PushStep.claktory;
      await backend.registerNotifications(
        RegisterNotifications(
          handle: kept.handle,
          key: kept.key,
          platform: state.pushPlatform,
        ),
      );
      _failed(backend, null);
      backend.diagnostics?.add(NotificationsKind.registered, {
        NotificationsKey.fresh: fresh,
      });
    } catch (error) {
      // The relay out of reach or refusing, the keychain or the Claktory:
      // shown in Settings, and tried again on the next connect.
      // Only what kind of failure: what it says could hold the token or
      // the handle, which never go in a log.
      var status = 0;
      if (error case PlatformException(code: 'keychain', :final details)) {
        if (details case {'step': 'registration'}) {
          step = PushStep.registration;
        }
        if (details case {'status': final int code}) status = code.abs();
      }
      debugPrint(
        "couldn't register for notifications at ${step.name} "
        '(${error.runtimeType}${status == 0 ? '' : ' $status'})',
      );
      _failed(backend, step);
      backend.diagnostics?.add(NotificationsKind.failed, {
        NotificationsKey.step: step,
        NotificationsKey.fresh: fresh,
        if (status != 0) NotificationsKey.status: status,
      });
    }
  }

  /// Whether the person lets Claktory notify them, and this device's push
  /// token, as last read; null before it's read.
  PushState? get state => _state;

  /// The person asked for notifications in Settings: the system asks
  /// (unless it has already), and a yes registers with every Claktory at
  /// once. Answers the state as it is now, read afresh (after the person
  /// changed it in the system's Settings, say).
  Future<PushState?> ask() async {
    _asked = true;
    // Afresh: the person may have changed it in the system's Settings.
    final state = await _safely(device.state) ?? _state;
    if (state != null && state.permission == 'not_determined') {
      final answer = await _safely(device.ask);
      if (answer == 'allowed') {
        _state = await _safely(device.state) ?? state;
        _syncAll();
      }
    }
    return refresh();
  }

  /// Reads the state again (the app came back from the system's
  /// Settings): newly allowed, it registers with every Claktory.
  Future<PushState?> refresh() async {
    final before = _state;
    _state = await _safely(device.state) ?? before;
    if (_state?.allowed == true && before?.allowed != true) _syncAll();
    return _state;
  }

  /// The person just did something they'd want to hear back about (sent a
  /// message from this iPhone): the first time, the system asks whether
  /// Claktory may notify them. Once asked, never again.
  Future<void> askInContext() async {
    if (_asked) return;
    _asked = true;
    final state = _state ?? await _safely(device.state);
    if (state == null || state.permission != 'not_determined') return;
    final answer = await _safely(device.ask);
    if (answer == 'allowed') {
      _state = await _safely(device.state) ?? state;
      _syncAll();
    }
  }

  static Future<T?> _safely<T>(Future<T> Function() call) async {
    try {
      return await call();
    } on MissingPluginException {
      return null;
    } on PlatformException {
      return null;
    }
  }

  void dispose() {
    if (identical(current, this)) current = null;
    claktories.removeListener(_follow);
    for (final entry in _followed.entries) {
      entry.key.backend.connectedSince.removeListener(entry.value);
    }
    _followed.clear();
    unawaited(_tokens.cancel());
  }
}

/// The iPhone's side, through factory/push (Notifications.swift).
class IosPushDevice implements PushDevice {
  IosPushDevice({this.channel = const MethodChannel('factory/push')}) {
    channel.setMethodCallHandler((call) async {
      if (call.method == 'token') {
        _tokens.add(PushState.fromMap(_withAllowed(call.arguments)));
      }
    });
  }

  final MethodChannel channel;
  final _tokens = StreamController<PushState>.broadcast();

  /// A token comes only once notifications are allowed.
  static Object? _withAllowed(Object? arguments) => {
    if (arguments is Map) ...arguments,
    'permission': 'allowed',
  };

  @override
  Stream<PushState> get tokens => _tokens.stream;

  @override
  Future<PushState> state() async =>
      PushState.fromMap(await channel.invokeMethod<Object?>('state'));

  @override
  Future<String> ask() async =>
      await channel.invokeMethod<String>('ask') ?? 'denied';

  @override
  Future<List<String>> kept() async =>
      await channel.invokeListMethod<String>('kept') ?? const [];

  @override
  Future<PushRegistration?> registration(String claktory) async =>
      PushRegistration.fromMap(
        await channel.invokeMethod<Object?>('registration', {
          'claktory': claktory,
        }),
      );

  @override
  Future<Uint8List> register(
    String claktory, {
    required String handle,
    required String token,
    required String relay,
    required String platform,
  }) async {
    final key = await channel.invokeMethod<Uint8List>('register', {
      'claktory': claktory,
      'handle': handle,
      'token': token,
      'relay': relay,
      'platform': platform,
    });
    if (key == null || key.length != 32) {
      throw StateError('no notification key');
    }
    return key;
  }

  @override
  Future<void> forget(String claktory) =>
      channel.invokeMethod<void>('forget', {'claktory': claktory});
}

/// Claktory's push relay over HTTPS: `POST {relay}/v1/register`.
class HttpPushRelay implements PushRelay {
  HttpPushRelay({HttpClient? client}) : _client = client ?? HttpClient();

  final HttpClient _client;

  @override
  Future<String> register(String relay, String platform, String token) async {
    final (status, json) = await _post(relay, 'v1/register', {
      'platform': platform,
      'token': token,
    });
    final handle = json is Map ? json['handle'] : null;
    if (status != 200 || handle is! String || handle.isEmpty) {
      throw HttpException('the push relay answered $status');
    }
    return handle;
  }

  @override
  Future<void> unregister(String relay, String handle) async {
    final (status, _) = await _post(relay, 'v1/unregister', {'handle': handle});
    if (status != 200) {
      throw HttpException('the push relay answered $status');
    }
  }

  Future<(int, Object?)> _post(
    String relay,
    String path,
    Map<String, String> body,
  ) async {
    final base = Uri.parse(relay);
    if (base.scheme != 'https' && !loopbackDevRelay(base)) {
      throw ArgumentError('a push relay is an https address: $relay');
    }
    final url = base.resolve(
      '${base.path.endsWith('/') ? '' : '${base.path}/'}$path',
    );
    final request = await _client
        .postUrl(url)
        .timeout(const Duration(seconds: 15));
    request.headers.contentType = ContentType.json;
    request.write(jsonEncode(body));
    final response = await request.close().timeout(const Duration(seconds: 15));
    final text = await response.transform(utf8.decoder).join();
    Object? json;
    try {
      json = text.isEmpty ? null : jsonDecode(text);
    } on FormatException {
      json = null;
    }
    return (response.statusCode, json);
  }
}
