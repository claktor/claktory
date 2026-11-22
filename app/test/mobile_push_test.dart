// An iPhone's registration for pushes (#273 B3, mobile/push.dart): once the
// person allows notifications, each Claktory it connects to gets this
// iPhone's handle at the push relay and its key, on every connect; the relay
// is asked again only when something it was asked with changed. The person
// is asked the first time they send something, never at launch.
import 'dart:async';
import 'dart:typed_data';

import 'package:factory_app/connections.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/mobile/push.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _studio = 'factory://?t=${'a' * 64}&name=studio&e=${'c' * 64}';
final _home = 'factory://?t=${'b' * 64}&name=home&e=${'d' * 64}';

class _Backend extends RecordingBackend {
  _Backend() : super(localBackend: false);

  final registered = <RegisterNotifications>[];

  @override
  Future<void> registerNotifications(
    RegisterNotifications registration,
  ) async => registered.add(registration);
}

class _Device implements PushDevice {
  String permission = 'not_determined';
  String token = '';
  String answer = 'allowed';
  var asked = 0;
  final saved = <String, PushRegistration>{};
  final tokenChanges = StreamController<PushState>.broadcast(sync: true);
  var keys = 0;

  PushState get _state =>
      PushState(permission: permission, token: token, platform: 'apns');

  @override
  Future<PushState> state() async => _state;

  @override
  Future<String> ask() async {
    asked++;
    permission = answer;
    if (answer == 'allowed') token = 'feed';
    return answer;
  }

  @override
  Stream<PushState> get tokens => tokenChanges.stream;

  @override
  Future<PushRegistration?> registration(String claktory) async =>
      saved[claktory];

  @override
  Future<Uint8List> register(
    String claktory, {
    required String handle,
    required String token,
    required String relay,
    required String platform,
  }) async {
    final key =
        saved[claktory]?.key ?? Uint8List.fromList(List.filled(32, ++keys));
    saved[claktory] = PushRegistration(
      handle: handle,
      token: token,
      relay: relay,
      platform: platform,
      registered: DateTime(2026, 11, 22),
      key: key,
    );
    return key;
  }

  @override
  Future<void> forget(String claktory) async => saved.remove(claktory);

  @override
  Future<List<String>> kept() async => saved.keys.toList();
}

class _Relay implements PushRelay {
  final asked = <String>[];
  final unregistered = <String>[];

  @override
  Future<void> unregister(String relay, String handle) async =>
      unregistered.add('$relay $handle');

  @override
  Future<String> register(String relay, String platform, String token) async {
    asked.add('$relay $platform $token');
    return 'handle${asked.length}';
  }
}

void main() {
  late Claktories claktories;
  late Connections connections;
  late _Device device;
  late _Relay relay;
  late DateTime now;
  final backends = <String, _Backend>{};

  Future<PushRegistrar> start(
    WidgetTester tester, {
    Map<String, PushRegistration> saved = const {},
  }) async {
    await tester.pumpWidget(const SizedBox());
    connections = Connections(MemoryConnectionStorage());
    claktories = Claktories(
      connections: connections,
      localBackend: false,
      drafts: DeviceDrafts(MemoryDraftStorage()),
      open: (target) =>
          backends[target?.code ?? ''] = _Backend()..connectTo(target),
    );
    connections.use(parsePairingCode(_studio));
    device = _Device()..saved.addAll(saved);
    relay = _Relay();
    now = DateTime(2026, 11, 22, 12);
    final registrar = PushRegistrar(
      claktories: claktories,
      device: device,
      relay: relay,
      now: () => now,
    );
    addTearDown(() {
      registrar.dispose();
      claktories.dispose();
      backends.clear();
    });
    await tester.pump();
    return registrar;
  }

  _Backend studio() => backends[_studio]!;

  Future<void> connect(WidgetTester tester, _Backend backend) async {
    backend.connectedSince.value = DateTime.now();
    await tester.pump();
    await tester.pump();
  }

  testWidgets('nothing registers until the person allows notifications, '
      'asked the first time they send something', (tester) async {
    final registrar = await start(tester);
    await connect(tester, studio());
    expect(relay.asked, isEmpty);
    expect(device.asked, 0);

    await registrar.askInContext();
    await tester.pump();
    expect(device.asked, 1);
    expect(relay.asked, ['https://push.claktory.com apns feed']);
    final sent = studio().registered.single;
    expect(sent.handle, 'handle1');
    expect(sent.key, hasLength(32));
    expect(sent.platform, PushPlatform.PUSH_PLATFORM_APNS);

    // Asked once only.
    await registrar.askInContext();
    expect(device.asked, 1);
  });

  testWidgets("turned down, nothing registers and it isn't asked again", (
    tester,
  ) async {
    final registrar = await start(tester);
    device.answer = 'denied';
    await connect(tester, studio());
    await registrar.askInContext();
    await registrar.askInContext();
    await tester.pump();
    expect(device.asked, 1);
    expect(relay.asked, isEmpty);
    expect(studio().registered, isEmpty);
  });

  testWidgets('every connect hands the same registration over again; the '
      'relay is asked only when what it was asked with changed', (
    tester,
  ) async {
    await start(tester);
    device
      ..permission = 'allowed'
      ..token = 'feed';
    device.tokenChanges.add(
      const PushState(permission: 'allowed', token: 'feed', platform: 'apns'),
    );
    await connect(tester, studio());
    expect(relay.asked, hasLength(1));
    expect(studio().registered, hasLength(1));

    // Reconnected: the same again, from what's kept.
    studio().connectedSince.value = null;
    await tester.pump();
    await connect(tester, studio());
    expect(relay.asked, hasLength(1));
    expect(studio().registered, hasLength(2));
    expect(studio().registered.last, studio().registered.first);

    // A new token from Apple: a new handle, the same key.
    device.token = 'beef';
    device.tokenChanges.add(
      const PushState(permission: 'allowed', token: 'beef', platform: 'apns'),
    );
    await tester.pump();
    await tester.pump();
    expect(relay.asked.last, 'https://push.claktory.com apns beef');
    expect(studio().registered.last.handle, 'handle2');
    expect(studio().registered.last.key, studio().registered.first.key);

    // A month on: asked afresh.
    now = now.add(const Duration(days: 31));
    await connect(tester, studio());
    expect(relay.asked, hasLength(3));
  });

  testWidgets('each Claktory gets its own handle and key, at the relay its '
      'settings name', (tester) async {
    await start(tester);
    device
      ..permission = 'allowed'
      ..token = 'feed';
    device.tokenChanges.add(
      const PushState(permission: 'allowed', token: 'feed', platform: 'apns'),
    );
    await connect(tester, studio());
    connections.use(parsePairingCode(_home));
    await tester.pump();
    final home = backends[_home]!;
    home.workspace.apply(
      Workspace(
        settings: Settings(
          notifications: NotificationSettings(relay: 'https://push.example'),
        ),
      ),
    );
    await connect(tester, home);
    expect(relay.asked.last, 'https://push.example apns feed');
    expect(
      home.registered.single.handle,
      isNot(studio().registered.single.handle),
    );
    expect(home.registered.single.key, isNot(studio().registered.single.key));
    expect(device.saved.keys, unorderedEquals(['c' * 64, 'd' * 64]));

    // The person removes one: its handle is unregistered at the relay, so
    // its Claktory stops pushing here, and its key goes; the other's stays.
    final handle = device.saved['c' * 64]!.handle;
    connections.forget(parsePairingCode(_studio)!);
    await tester.pump();
    expect(relay.unregistered, ['$defaultPushRelay $handle']);
    expect(device.saved.keys, ['d' * 64]);
  });

  testWidgets('a key for a Claktory removed while the app was closed goes '
      'at launch', (tester) async {
    final key = Uint8List(32);
    PushRegistration kept() => PushRegistration(
      handle: 'h',
      token: 't',
      relay: defaultPushRelay,
      platform: 'apns',
      registered: DateTime(2026, 11, 22),
      key: key,
    );
    await start(tester, saved: {'c' * 64: kept(), 'e' * 64: kept()});
    await tester.pump();
    expect(device.saved.keys, ['c' * 64]);
    expect(relay.unregistered, ['$defaultPushRelay h']);
  });
}
