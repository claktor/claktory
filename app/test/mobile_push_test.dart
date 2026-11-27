// An iPhone's registration for pushes (#273 B3, mobile/push.dart): once the
// person allows notifications, each Claktory it connects to gets this
// iPhone's handle at the push relay and its key, on every connect; the relay
// is asked again only when something it was asked with changed. The person
// is asked right after pairing a Claktory, or the first time they send
// something, never at launch.
import 'dart:async';
import 'dart:typed_data';

import 'package:factory_app/backend.dart' show Backend;
import 'package:factory_app/connections.dart';
import 'package:factory_app/diagnostics_record.dart' show PushStep;
import 'package:factory_app/drafts.dart';
import 'package:factory_app/mobile/permission.dart' show registerPermissionHost;
import 'package:factory_app/mobile/push.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:flutter/foundation.dart'
    show TargetPlatform, debugDefaultTargetPlatformOverride;
import 'package:flutter/services.dart' show PlatformException;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _studio = 'factory://?t=${'a' * 64}&name=studio&e=${'c' * 64}';
final _home = 'factory://?t=${'b' * 64}&name=home&e=${'d' * 64}';

class _Backend extends RecordingBackend {
  _Backend() : super(localBackend: false);

  final registered = <RegisterNotifications>[];

  /// What each SetActive said of this device's notifications.
  final said = <NotificationPermission>[];

  @override
  Future<void> registerNotifications(
    RegisterNotifications registration,
  ) async => registered.add(registration);

  @override
  Future<void> sendActive(bool active) async =>
      said.add(Backend.notificationPermission());
}

class _Device implements PushDevice {
  String permission = 'not_determined';
  String token = '';
  String answer = 'allowed';
  var asked = 0;
  final saved = <String, PushRegistration>{};
  final tokenChanges = StreamController<PushState>.broadcast(sync: true);
  var keys = 0;

  /// What the keychain says instead of keeping a key, as v0.1.16's did.
  PlatformException? refuse;

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
    if (refuse case final refused?) throw refused;
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

  testWidgets('on a screen, why first: the permission sheet, then the '
      "system's prompt; a yes registers (area 5a, P3)", (tester) async {
    final registrar = await start(tester);
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    late BuildContext screen;
    await Harness(Workspace()).show(
      tester,
      size: const Size(390, 844),
      home: Builder(
        builder: (context) {
          screen = context;
          return const SizedBox.expand();
        },
      ),
    );
    addTearDown(registerPermissionHost(screen));
    await connect(tester, studio());

    unawaited(registrar.askInContext());
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Know when an agent needs you'), findsOneWidget);
    expect(device.asked, 0, reason: 'not before Continue');
    await tester.tap(find.text('Continue'));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(device.asked, 1);
    expect(find.text('Know when an agent needs you'), findsNothing);
    expect(studio().registered, hasLength(1));
  });

  testWidgets('lost with its window before an answer (an iPad crossing '
      'between its narrow and wide layouts builds a new one), it asks again '
      "in the new window (Sale's row 198, Lucy 49620)", (tester) async {
    final registrar = await start(tester);
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    late BuildContext screen;
    Future<void> window() => Harness(Workspace()).show(
      tester,
      size: const Size(390, 844),
      home: Builder(
        builder: (context) {
          screen = context;
          return const SizedBox.expand();
        },
      ),
    );
    await window();
    addTearDown(registerPermissionHost(screen));
    await connect(tester, studio());
    unawaited(registrar.askInContext());
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Know when an agent needs you'), findsOneWidget);

    // The window goes, its sheet with it, before Continue.
    await tester.pumpWidget(const SizedBox());
    await tester.pump();
    expect(find.text('Know when an agent needs you'), findsNothing);
    await window();
    addTearDown(registerPermissionHost(screen));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Know when an agent needs you'), findsOneWidget);
    await tester.tap(find.text('Continue'));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(device.asked, 1);
    expect(studio().registered, hasLength(1));

    // Answered: a later window or coming back asks nothing.
    await tester.pumpWidget(const SizedBox());
    await window();
    addTearDown(registerPermissionHost(screen));
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Know when an agent needs you'), findsNothing);
  });

  testWidgets('a Claktory paired since the app started asks once, as it '
      'first connects; those it started with never do', (tester) async {
    await start(tester);
    await connect(tester, studio());
    expect(device.asked, 0, reason: 'paired before: never at launch');

    connections.use(parsePairingCode(_home));
    await tester.pump();
    final home = backends[_home]!;
    expect(device.asked, 0, reason: 'not before it lets this iPhone in');
    await connect(tester, home);
    expect(device.asked, 1);
    // A yes registers with both.
    expect(home.registered, hasLength(1));
    expect(studio().registered, hasLength(1));

    // Reconnected later: not again.
    home.connectedSince.value = null;
    await tester.pump();
    await connect(tester, home);
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

  testWidgets('Turn on in Settings asks, once, and a yes registers at once; '
      'asking in context after it asks nothing more', (tester) async {
    final registrar = await start(tester);
    await connect(tester, studio());
    expect(registrar.state?.permission, 'not_determined');
    final state = await registrar.ask();
    await tester.pump();
    expect(state?.permission, 'allowed');
    expect(device.asked, 1);
    expect(studio().registered.single.handle, 'handle1');
    await registrar.askInContext();
    await registrar.ask();
    expect(device.asked, 1, reason: 'the system asks only once');
  });

  testWidgets('turned off in the system: nothing asks or registers; turned '
      'on there, back in the app it registers', (tester) async {
    final registrar = await start(tester);
    device.permission = 'denied';
    await connect(tester, studio());
    expect((await registrar.ask())?.permission, 'denied');
    expect(device.asked, 0);
    expect(studio().registered, isEmpty);

    // The person turns them on in Settings, and comes back.
    device
      ..permission = 'allowed'
      ..token = 'feed';
    expect((await registrar.refresh())?.permission, 'allowed');
    await tester.pump();
    expect(studio().registered.single.handle, 'handle1');
    // Already allowed: reading again registers nothing more.
    await registrar.refresh();
    await tester.pump();
    expect(studio().registered, hasLength(1));
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

  testWidgets("a keychain that won't keep the key (v0.1.16 on Matt's "
      'iPhone): the Claktory never hears, so it says so; Try again, and the '
      'next connect, register once it can', (tester) async {
    final registrar = await start(tester);
    device
      ..permission = 'allowed'
      ..token = 'feed'
      ..refuse = PlatformException(
        code: 'keychain',
        details: {'step': 'key', 'status': -34018},
      );
    device.tokenChanges.add(
      const PushState(permission: 'allowed', token: 'feed', platform: 'apns'),
    );
    await connect(tester, studio());
    expect(relay.asked, hasLength(1));
    expect(studio().registered, isEmpty);
    expect(registrar.failures.value, {studio(): PushStep.key});

    // Asked again on the next try: no key was kept.
    device.refuse = null;
    registrar.retry(studio());
    await tester.pump();
    await tester.pump();
    expect(relay.asked, hasLength(2));
    expect(studio().registered.single.key, hasLength(32));
    expect(registrar.failures.value, isEmpty);
  });

  testWidgets('a keychain that keeps the key but not the registration says '
      'which', (tester) async {
    final registrar = await start(tester);
    device
      ..permission = 'allowed'
      ..token = 'feed'
      ..refuse = PlatformException(
        code: 'keychain',
        details: {'step': 'registration', 'status': -25300},
      );
    device.tokenChanges.add(
      const PushState(permission: 'allowed', token: 'feed', platform: 'apns'),
    );
    await connect(tester, studio());
    expect(registrar.failures.value, {studio(): PushStep.registration});
  });

  group('each Claktory hears whether this iPhone shows notifications '
      '(item 13)', () {
    const unsaid = NotificationPermission.NOTIFICATION_PERMISSION_UNSPECIFIED;
    const notAsked = NotificationPermission.NOTIFICATION_PERMISSION_NOT_ASKED;
    const allowed = NotificationPermission.NOTIFICATION_PERMISSION_ALLOWED;
    const denied = NotificationPermission.NOTIFICATION_PERMISSION_DENIED;
    // And unsaid once the registrar goes (each test's own tear-down).
    tearDown(() => expect(Backend.notificationPermission(), unsaid));

    testWidgets('read at start, said again as the person answers; the '
        'registration carries it', (tester) async {
      final registrar = await start(tester);
      expect(Backend.notificationPermission(), notAsked);
      expect(studio().said, [notAsked]);

      await connect(tester, studio());
      await registrar.askInContext();
      await tester.pump();
      expect(Backend.notificationPermission(), allowed);
      expect(studio().said.last, allowed);
      expect(studio().registered.single.permission, allowed);

      // Read again, unchanged: nothing more to say.
      final before = studio().said.length;
      await registrar.refresh();
      expect(studio().said, hasLength(before));
    });

    testWidgets('turned off in the system, then back in the app: said at '
        'once', (tester) async {
      await start(tester);
      device
        ..permission = 'allowed'
        ..token = 'feed';
      await connect(tester, studio());
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pump();
      expect(studio().said.last, allowed);

      device.permission = 'denied';
      // Away in Settings, and back.
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pump();
      await tester.pump();
      expect(Backend.notificationPermission(), denied);
      expect(studio().said.last, denied);
    });
  });

  test("a dev build never registers with Claktory's own relay: only with "
      'FACTORY_PUSH_RELAY, or a relay the Claktory names', () {
    // A release build: the Claktory's, else Claktory's own.
    expect(pushRelayFor('', dev: false), defaultPushRelay);
    expect(
      pushRelayFor('https://push.example.org', dev: false),
      'https://push.example.org',
    );
    expect(
      pushRelayFor('', dev: false, devRelay: 'http://127.0.0.1:9'),
      defaultPushRelay,
    );
    // A dev build.
    expect(pushRelayFor('', dev: true), '');
    expect(pushRelayFor(defaultPushRelay, dev: true), '');
    expect(pushRelayFor('https://push.claktory.com/', dev: true), '');
    expect(
      pushRelayFor('https://push.example.org', dev: true),
      'https://push.example.org',
    );
    expect(
      pushRelayFor(defaultPushRelay, dev: true, devRelay: 'http://127.0.0.1:9'),
      'http://127.0.0.1:9',
    );
  });

  test('plain http only for a dev build on this computer', () {
    expect(
      loopbackDevRelay(Uri.parse('http://127.0.0.1:8090'), dev: true),
      isTrue,
    );
    expect(
      loopbackDevRelay(Uri.parse('http://localhost:8090'), dev: true),
      isTrue,
    );
    expect(
      loopbackDevRelay(Uri.parse('http://127.0.0.1:8090'), dev: false),
      isFalse,
    );
    expect(
      loopbackDevRelay(Uri.parse('http://push.example.org'), dev: true),
      isFalse,
    );
  });
}
