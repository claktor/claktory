import 'dart:convert';
import 'dart:io';

import 'package:fixnum/fixnum.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/claktory_identity.dart';
import 'package:factory_app/account_match.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/links.dart' show appLinks;
import 'package:factory_app/main.dart';
import 'package:factory_app/name_step.dart';
import 'package:factory_app/mobile/drawer.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/device_kind.dart' show thisDeviceKindForTests;
import 'package:factory_app/devices.dart' show thisDeviceKind;
import 'package:factory_app/rail.dart';
import 'package:factory_app/waiting.dart' show NeedsYouMark;
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/tunnel_client.dart';
import 'package:factory_app/voice_keep.dart';
import 'package:factory_app/mobile/local_network.dart' show localNetwork;
import 'package:factory_app/visible_codes.dart' show nameInText;
import 'package:flutter/foundation.dart'
    show debugDefaultTargetPlatformOverride;
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'device_confirm_test.dart' show FakeAccountApi, signedIn;
import 'harness.dart';

const _nixosKey =
    'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc';
const _miniKey =
    'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd';
final _nixos = 'factory://?t=${'a' * 64}&name=nixos&e=$_nixosKey';
final _mini = 'factory://?t=${'b' * 64}&name=mac-mini&e=$_miniKey';

/// A Claktory not on the default account.
final _pi = 'factory://?t=${'e' * 64}&name=pi&e=${'f' * 64}';

Workspace _workspace(
  String agent, {
  bool working = false,
  bool unseen = false,
  List<int> waiting = const [],
  String usageWait = '',
}) => Workspace(
  agents: [
    Agent(
      id: 'rivet',
      name: agent,
      modelAvailable: true,
      status: working
          ? AgentStatus.AGENT_STATUS_WORKING
          : AgentStatus.AGENT_STATUS_IDLE,
      unseen: unseen,
      waitingSeqs: [for (final seq in waiting) Int64(seq)],
      usageWait: usageWait,
    ),
  ],
  selectedAgentId: 'rivet',
  settings: Settings(userName: 'Matt'),
);

Finder _labeled(String label) => find.byWidgetPredicate(
  (w) => w is Semantics && w.properties.label == label,
);

/// The app with a recording backend for each Claktory, by its pairing code
/// ('' for this Mac's), each showing [snapshots] for it.
class _App {
  _App({
    this.snapshots = const {},
    this.localBackend = true,
    this.account,
    this.ownCode,
  });

  /// This Mac's own Claktory's code, as its file holds it.
  final String? ownCode;

  /// A Mac, which runs a Claktory of its own; a phone doesn't.
  final bool localBackend;

  final Map<String, Workspace> snapshots;
  final storage = MemoryConnectionStorage();

  /// The person's Claktory account on this device: by default signed in,
  /// with nixos and mac-mini on it.
  final DeviceAccount? account;
  final accountApi = FakeAccountApi()
    ..claktoryList = [
      const AccountClaktory(id: 'n', name: 'nixos', endpointId: _nixosKey),
      const AccountClaktory(id: 'm', name: 'mac-mini', endpointId: _miniKey),
    ];
  late final connections = Connections(
    storage,
    null,
    null,
    null,
    account ?? signedIn(accountApi),
  );
  final draftStorage = MemoryDraftStorage();
  late final drafts = DeviceDrafts(draftStorage);
  final backends = <String, RecordingBackend>{};

  Backend open(RemoteFactory? target) {
    final key = target?.code ?? '';
    final backend = RecordingBackend(localBackend: localBackend)
      ..connectTo(target);
    backend.workspace.apply(snapshots[key] ?? _workspace('Rivet'));
    return backends[key] = backend;
  }

  Future<void> show(
    WidgetTester tester, {
    Size size = const Size(1200, 900),
  }) async {
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      FactoryApp(
        drafts: drafts,
        connections: connections,
        open: open,
        localBackend: localBackend,
        ownCode: ownCode == null
            ? null
            : () async => parsePairingCode(ownCode!),
      ),
    );
    await pumpFrames(tester);
  }
}

void main() {
  // Claktories follow the app's lifecycle, which needs the binding.
  TestWidgetsFlutterBinding.ensureInitialized();

  group('Claktories', () {
    test('one live backend for each, kept in step with the connections', () {
      final app = _App();
      final claktories = Claktories(
        connections: app.connections,
        localBackend: true,
        open: app.open,
        drafts: app.drafts,
      );
      addTearDown(claktories.dispose);
      expect(claktories.all.map((c) => c.key), ['']);
      expect(claktories.selected.target, isNull);

      final nixos = parsePairingCode(_nixos)!;
      app.connections.use(nixos);
      expect(claktories.all.map((c) => c.key), ['', _nixos]);
      expect(claktories.selected.key, _nixos);
      expect(app.backends[_nixos]!.target, nixos);
      final local = app.backends['']!;

      app.connections.use(parsePairingCode(_mini));
      // Choosing one never reorders the rail.
      claktories.select(claktories.all[1]);
      expect(claktories.all.map((c) => c.key), ['', _nixos, _mini]);
      expect(claktories.selected.key, _nixos);
      expect(app.backends['']!, same(local), reason: 'still the same one');

      app.connections.forget(nixos);
      expect(claktories.all.map((c) => c.key), ['', _mini]);
      expect(claktories.selected.key, '', reason: "this Mac's instead");
    });

    test('opening one that makes its device key opens it once, not twice', () {
      // A backend's first connection makes this device's key for its
      // Claktory and keeps it, which changes the connections while the
      // Claktory is being opened: that must not open a second, live
      // connection to it as the same device.
      final app = _App(localBackend: false);
      final opened = <String>[];
      final claktories = Claktories(
        connections: app.connections,
        localBackend: false,
        open: (target) {
          opened.add(target?.code ?? '');
          if (target != null) app.connections.deviceKey(target);
          return app.open(target);
        },
        drafts: app.drafts,
      );
      addTearDown(claktories.dispose);
      final nixos = parsePairingCode(_nixos)!;
      app.connections.use(nixos);
      expect(opened, ['', _nixos]);
      expect(claktories.selected.target, nixos);
      expect(claktories.selected.backend, same(app.backends[_nixos]));
      expect(app.connections.deviceKey(nixos), isNotEmpty);
    });

    // Kept, it would knock on itself as a newcomer to allow.
    test("an entry from the account's list for this Mac's own Claktory goes "
        'at launch; a code\'s and a phone\'s stay', () async {
      final app = _App();
      final own = RemoteFactory.fromAccount(
        const AccountClaktory(id: 'n', name: 'nixos', endpointId: _nixosKey),
      );
      final mini = parsePairingCode(_mini)!;
      app.connections
        ..use(mini)
        ..use(own);
      // A new install writes its code once it runs.
      String? file;
      final claktories = Claktories(
        connections: app.connections,
        localBackend: true,
        open: app.open,
        drafts: app.drafts,
        ownCode: () async => file == null ? null : parsePairingCode(file),
      );
      addTearDown(claktories.dispose);
      await pumpEventQueue();
      expect(claktories.ownEndpoint, isEmpty, reason: 'none yet');
      expect(app.connections.saved, [mini, own]);
      file = _nixos;
      // Its connection comes back (the test's backend starts connected).
      app.backends['']!.connected
        ..value = false
        ..value = true;
      await pumpEventQueue();
      expect(claktories.ownEndpoint, _nixosKey);
      expect(app.connections.saved, [mini]);
      expect(claktories.all.map((c) => c.key), ['', _mini]);
      expect(claktories.selected.target, isNull, reason: "this Mac's instead");

      // Given up here, nothing of it is known; chosen again, it's read.
      claktories.setLocalBackend(false);
      expect(claktories.ownEndpoint, isEmpty, reason: 'none here now');
      app.connections.use(own);
      await pumpEventQueue();
      expect(app.connections.saved, [mini, own]);
      claktories.setLocalBackend(true);
      await pumpEventQueue();
      expect(app.connections.saved, [mini]);

      // A code for it is a pairing of its own, as before.
      final paired = parsePairingCode(_nixos)!;
      app.connections.use(paired);
      app.backends['']!.connected
        ..value = false
        ..value = true;
      await pumpEventQueue();
      expect(app.connections.saved, [mini, paired]);

      // A phone runs none: nothing is read, nothing goes.
      final phone = _App(localBackend: false);
      phone.connections.use(own);
      var read = 0;
      final its = Claktories(
        connections: phone.connections,
        localBackend: false,
        open: phone.open,
        drafts: phone.drafts,
        ownCode: () async {
          read++;
          return parsePairingCode(_nixos);
        },
      );
      addTearDown(its.dispose);
      await pumpEventQueue();
      expect(read, 0);
      expect(phone.connections.saved, [own]);
    });

    test(
      "with no code to read, this Mac's own Claktory is known by what it "
      "says over its connection: an account list's entry for it goes",
      () async {
        final app = _App();
        final own = RemoteFactory.fromAccount(
          const AccountClaktory(id: 'n', name: 'nixos', endpointId: _nixosKey),
        );
        final mini = parsePairingCode(_mini)!;
        app.connections
          ..use(mini)
          ..use(own);
        final claktories = Claktories(
          connections: app.connections,
          localBackend: true,
          open: app.open,
          drafts: app.drafts,
        );
        addTearDown(claktories.dispose);
        await pumpEventQueue();
        expect(claktories.ownEndpoint, isEmpty, reason: 'not said yet');
        expect(app.connections.saved, [mini, own]);

        app.backends['']!.workspace.apply(
          _workspace('Rivet')
            ..remote = RemoteAccess(endpointId: _nixosKey.toUpperCase()),
        );
        await pumpEventQueue();
        expect(claktories.ownEndpoint, _nixosKey);
        expect(app.connections.saved, [mini]);
        expect(claktories.all.map((c) => c.key), ['', _mini]);

        // Added from the account's list again: it goes again.
        app.connections.use(own);
        app.backends['']!.workspace.apply(
          _workspace('Rivet')..remote = RemoteAccess(endpointId: _nixosKey),
        );
        await pumpEventQueue();
        expect(app.connections.saved, [mini]);

        // What another Claktory says of itself is never taken as this Mac's.
        final other = _App(
          snapshots: {
            _mini: _workspace('Rivet')
              ..remote = RemoteAccess(endpointId: _nixosKey),
          },
        );
        other.connections
          ..use(mini)
          ..use(own);
        final its = Claktories(
          connections: other.connections,
          localBackend: true,
          open: other.open,
          drafts: other.drafts,
        );
        addTearDown(its.dispose);
        await pumpEventQueue();
        expect(its.ownEndpoint, isEmpty);
        expect(other.connections.saved, [mini, own]);
      },
    );

    test('a phone shows another paired one when the one showing goes', () {
      final app = _App();
      final claktories = Claktories(
        connections: app.connections,
        localBackend: false,
        open: app.open,
        drafts: app.drafts,
      );
      addTearDown(claktories.dispose);
      expect(claktories.all, isEmpty);
      app.connections.use(parsePairingCode(_nixos));
      app.connections.use(parsePairingCode(_mini));
      app.connections.forget(parsePairingCode(_mini)!);
      expect(claktories.selected.key, _nixos);
    });

    test('the ones connected and showing are kept on the device', () async {
      final storage = MemoryConnectionStorage();
      Connections(storage).use(parsePairingCode(_nixos));
      final again = await Connections.load(storage);
      expect(again.current?.code, _nixos);
      expect(again.saved.single.label, 'nixos');
    });

    test("a code's name and color show until the Claktory says its own", () {
      final target = parsePairingCode('$_nixos&color=teal')!;
      expect(target.color, 'teal');
      final backend = RecordingBackend(localBackend: false)..connectTo(target);
      expect(claktoryName(backend), 'nixos');
      expect(claktoryColorName(backend), 'teal');
      backend.welcomeName = 'Home';
      expect(claktoryName(backend), 'Home');
      backend.workspace.apply(
        Workspace(
          settings: Settings(name: 'Studio', color: 'green'),
        ),
      );
      expect(claktoryName(backend), 'Studio');
      expect(claktoryColorName(backend), 'green');
      expect(
        claktoryName(RecordingBackend(localBackend: true)),
        'This Mac',
        reason: "this Mac's, before it says",
      );
    });

    test(
      "each one's drafts are its own, kept together on the device",
      () async {
        final device = MemoryDraftStorage()..saved = {'rivet': 'local draft'};
        final drafts = await DeviceDrafts.load(device);
        final local = drafts.of('');
        final remote = drafts.of(_nixos);
        expect(local.of('rivet'), 'local draft');
        expect(remote.of('rivet'), '', reason: 'same id, another Claktory');
        remote.set('rivet', 'remote draft');
        await Future<void>.delayed(Duration.zero);
        expect(device.saved['rivet'], 'local draft');
        expect(device.saved.length, 2);
        local.set('rivet', '');
        await Future<void>.delayed(Duration.zero);
        final reloaded = await DeviceDrafts.load(device);
        expect(reloaded.of('').of('rivet'), '');
        expect(reloaded.of(_nixos).of('rivet'), 'remote draft');
      },
    );

    test('initials', () {
      expect(claktoryInitials('mac-mini'), 'MM');
      expect(claktoryInitials('nixos'), 'N');
      expect(claktoryInitials('home server box'), 'HS');
      expect(claktoryInitials(''), '?');
      // A name as nameInText shows it, in an isolate with a zero-width
      // space kept: its letters only.
      expect(claktoryInitials('\u2068Bad\\u{202E}Name\u200B two\u2069'), 'BT');
      expect(claktoryInitials('\u2068\u2069'), '?');
      // Its hidden characters shown by their codes are no letters either.
      expect(claktoryInitials(nameInText('Ana’s \u202EMac\u202C שלום')), 'AM');
    });
  });

  testWidgets('the rail shows every Claktory and switches the whole window', (
    tester,
  ) async {
    final app = _App(
      snapshots: {
        '': _workspace('Rivet'),
        _nixos: _workspace('Nix', working: true, unseen: true),
      },
    );
    app.connections.use(parsePairingCode(_nixos));
    app.connections.use(null);
    await app.show(tester);

    expect(find.byType(ClaktoryRail), findsOneWidget);
    expect(_labeled('This Mac'), findsOneWidget);
    expect(_labeled('nixos'), findsOneWidget);
    expect(find.text('N'), findsOneWidget, reason: 'its initials');
    // At a glance: nixos has news, and an agent at work.
    expect(_labeled('nixos has something new'), findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(ClaktoryRail),
        matching: find.byType(WorkingWave),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(of: find.byType(Sidebar), matching: find.text('Rivet')),
      findsWidgets,
    );

    await tester.tap(_labeled('nixos'));
    await pumpFrames(tester);
    expect(app.connections.current?.code, _nixos);
    expect(
      find.descendant(of: find.byType(Sidebar), matching: find.text('Nix')),
      findsWidgets,
    );
    expect(
      find.descendant(of: find.byType(Sidebar), matching: find.text('Rivet')),
      findsNothing,
    );
    // Both stay connected.
    expect(app.backends['']!.connected.value, isTrue);
    expect(app.backends[_nixos]!.connected.value, isTrue);

    // Losing the connection shows at a glance too.
    app.backends['']!.connected.value = false;
    app.backends['']!.refused.value = 'unknown device';
    await pumpFrames(tester);
    expect(
      _labeled('this Claktory no longer allows this device'),
      findsOneWidget,
    );
  });

  testWidgets("a right click renames and recolors one for every device, or "
      'removes it', (tester) async {
    final app = _App();
    app.connections.use(parsePairingCode(_nixos));
    await app.show(tester);
    final nixos = app.backends[_nixos]!;
    nixos.onSend = (message) {
      if (!message.hasUpdateSettings()) return;
      final update = message.updateSettings;
      nixos.workspace.apply(
        nixos.workspace.workspace.deepCopy()
          ..settings.name = update.name
          ..settings.color = update.color,
      );
    };

    await tester.tap(_labeled('nixos'), buttons: kSecondaryButton);
    await pumpFrames(tester);
    expect(find.text('Name'), findsOneWidget);
    expect(find.text('Shown on every device connected to it.'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, 'Home server');
    await tester.tap(find.bySemanticsLabel('green'));
    await pumpFrames(tester);
    await tester.tap(find.text('Save'));
    await pumpFrames(tester);
    expect(
      nixos.sent.last.updateSettings,
      UpdateSettings(name: 'Home server', color: 'green'),
    );
    expect(_labeled('Home server'), findsOneWidget);
    expect(find.text('HS'), findsOneWidget);
    expect(
      tester.widget<Text>(find.text('HS')).style?.color,
      agentColor('green'),
      reason: 'its initials in its color',
    );
    expect(app.storage.kept.saved, [_nixos], reason: 'no name kept here');

    // Not connected, it can only be removed.
    nixos.connected.value = false;
    await tester.tap(_labeled('Home server'), buttons: kSecondaryButton);
    await pumpFrames(tester);
    expect(
      find.text('Connect to Home server to change its name or color.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Remove…'));
    await pumpFrames(tester);
    expect(find.text('Remove Home server?'), findsOneWidget);
    nixos.connected.value = true;
    nixos.deviceId = 'phone-1';
    await tester.tap(find.text('Remove'));
    await pumpFrames(tester);
    expect(_labeled('Home server'), findsNothing);
    expect(app.connections.saved, isEmpty);
    expect(app.connections.current, isNull, reason: "this Mac's shows");
    expect(
      nixos.sent.last.unpairDevice.deviceId,
      'phone-1',
      reason: 'it leaves its devices',
    );
  });

  testWidgets('removing one with recordings not sent yet says they go '
      "too (Sale's row 76)", (tester) async {
    final kept = Directory.systemTemp.createTempSync('kept');
    final was = keptVoiceFolder;
    keptVoiceFolder = () async => kept.path;
    addTearDown(() {
      keptVoiceFolder = was;
      kept.deleteSync(recursive: true);
    });
    for (final (id, claktory) in [
      ('1', keptClaktoryKey(parsePairingCode(_nixos))),
      ('2', keptClaktoryKey(parsePairingCode(_nixos))),
      ('3', keptClaktoryKey(null)),
    ]) {
      final folder = Directory('${kept.path}/$id')..createSync();
      final voice = KeptVoice(
        path: '${folder.path}/Voice.m4a',
        duration: const Duration(seconds: 5),
        claktory: claktory,
        kind: KeptKind.agent,
        target: 'rivet',
        label: '',
        made: DateTime(2026, 11, 24),
        clientId: 'c$id',
        uploadId: 'u$id',
        requestId: 'r$id',
      );
      File(voice.path).writeAsBytesSync([1, 2, 3]);
      File('${folder.path}/${KeptVoice.recordName}')
          .writeAsStringSync(jsonEncode(voice.toJson()));
    }
    final app = _App();
    app.connections.use(parsePairingCode(_nixos));
    await app.show(tester);
    await tester.tap(_labeled('nixos'), buttons: kSecondaryButton);
    await pumpFrames(tester);
    await tester.tap(find.text('Remove…'));
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 50)),
    );
    await pumpFrames(tester);
    expect(
      find.textContaining(
        " 2 voice recordings you haven't sent yet are deleted too.",
      ),
      findsOneWidget,
    );
  });

  group('Remove, when this is the only device connected there (#529, '
      'MC9)', () {
    Future<void> ask(
      WidgetTester tester, {
      required List<PairedDevice> devices,
      DeviceAccount? account,
      String? code,
    }) async {
      code ??= _pi;
      final app = _App(
        account: account,
        snapshots: {
          code: _workspace('Pi')..remote = RemoteAccess(devices: devices),
        },
      );
      app.connections
        ..use(parsePairingCode(code))
        ..use(null);
      await app.show(tester);
      app.backends[code]!.deviceId = 'this';
      final name = parsePairingCode(code)!.name;
      await tester.tap(
        find.bySemanticsLabel(RegExp('^$name')),
        buttons: kSecondaryButton,
      );
      await pumpFrames(tester);
      await tester.tap(find.text('Remove…'));
      await pumpFrames(tester);
    }

    final me = PairedDevice(id: 'this', name: 'Mac');

    testWidgets('the only one: the terminal is the way back', (tester) async {
      await ask(
        tester,
        devices: [
          me,
          // A code made but never used is no way in.
          PairedDevice(id: 'new', name: 'iPad', codeExpiresMs: Int64(1)),
        ],
      );
      expect(
        find.text(
          'This $thisDeviceKind is the only device connected to pi. Its '
          'agents and work '
          'stay there. To connect again, run factoryd pair in a terminal on '
          'pi.',
        ),
        findsOneWidget,
      );
    });

    testWidgets('the only one, on the account: Add a Claktory is', (
      tester,
    ) async {
      await ask(tester, devices: [me], code: _nixos);
      expect(
        find.text(
          'This $thisDeviceKind is the only device connected to nixos. Its '
          'agents and '
          'work stay there. To connect again, choose it under On your '
          'account in Add a Claktory.',
        ),
        findsOneWidget,
      );
    });

    testWidgets('another device there: add this one again from it', (
      tester,
    ) async {
      await ask(
        tester,
        devices: [
          me,
          PairedDevice(id: 'phone', name: 'iPhone'),
        ],
      );
      expect(
        find.text(
          'This $thisDeviceKind disconnects from it. Its agents and work stay '
          'on pi. To connect again, add this $thisDeviceKind on pi in '
          'Settings → Devices.',
        ),
        findsOneWidget,
      );
    });
  });

  testWidgets('Add a Claktory lists none of the Claktories already here: '
      "they're the rail's, to switch to and remove (devices-signin.md §3)", (
    tester,
  ) async {
    final app = _App();
    app.connections
      ..use(parsePairingCode(_nixos))
      ..use(null);
    await app.show(tester);
    await tester.tap(_labeled('Add a Claktory'));
    await pumpFrames(tester);
    expect(find.text('Add a Claktory'), findsWidgets);
    for (final gone in [
      'Your Claktories',
      'Running here',
      'Running elsewhere',
    ]) {
      expect(find.text(gone), findsNothing, reason: gone);
    }
    expect(find.text('Remove…'), findsNothing);
    expect(find.text('Use a code instead'), findsOneWidget);
  });

  testWidgets("Add a Claktory never offers this Mac's own, known by its code "
      'before it answers', (tester) async {
    final app = _App(ownCode: _nixos);
    await app.show(tester);
    await tester.runAsync(pumpEventQueue);
    await pumpFrames(tester);
    await tester.tap(_labeled('Add a Claktory'));
    await pumpFrames(tester);
    expect(find.text('mac-mini'), findsOneWidget);
    expect(find.text('nixos'), findsNothing);
  });

  testWidgets('a touch-only iPad at full width: touch and hold a tile opens '
      'its form (Zucchero row 642)', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    // The app sets iOS's Local Network up as it starts on an iPad: not for
    // the tests after this one.
    addTearDown(() => localNetwork = null);
    final app = _App();
    app.connections
      ..use(parsePairingCode(_nixos))
      ..use(null);
    await app.show(tester, size: const Size(1180, 820));
    await tester.longPress(_labeled('nixos'));
    await pumpFrames(tester);
    expect(find.text('Shown on every device connected to it.'), findsOneWidget);
    await tester.tap(find.text('Cancel').last);
    await pumpFrames(tester);
  });

  test('with none kept, the question says nothing of recordings', () {
    expect(unsentGoesToo(0), '');
    expect(
      unsentGoesToo(1),
      " A voice recording you haven't sent yet is deleted too.",
    );
  });

  testWidgets("this Mac's Claktory shows its own name", (tester) async {
    final app = _App(
      snapshots: {'': _workspace('Rivet')..settings.name = 'Studio'},
    );
    await app.show(tester);
    expect(_labeled('Studio'), findsOneWidget);
    expect(_labeled('This Mac'), findsNothing);
  });

  testWidgets("this Mac's tile is a monitor on a desktop Mac, a laptop on a "
      'laptop (area 2, MC13)', (tester) async {
    String glyph() => tester
        .widgetList<CustomPaint>(find.byType(CustomPaint))
        .map((paint) => paint.painter.runtimeType.toString())
        .firstWhere(
          (name) => name == '_DesktopPainter' || name == '_LaptopPainter',
        );
    addTearDown(() => thisDeviceKindForTests = null);
    thisDeviceKindForTests = DeviceKind.DEVICE_KIND_DESKTOP;
    final app = _App(snapshots: {_nixos: _workspace('Nix')});
    app.connections.use(parsePairingCode(_nixos));
    await app.show(tester);
    expect(glyph(), '_DesktopPainter');

    // A laptop: its own tree, as on another Mac.
    await tester.pumpWidget(const SizedBox());
    thisDeviceKindForTests = DeviceKind.DEVICE_KIND_LAPTOP;
    final laptop = _App(snapshots: {_nixos: _workspace('Nix')});
    laptop.connections.use(parsePairingCode(_nixos));
    await laptop.show(tester);
    await pumpFrames(tester);
    expect(glyph(), '_LaptopPainter');
  });

  testWidgets('"+" adds one by its code', (tester) async {
    final app = _App();
    await app.show(tester);
    expect(_labeled('This Mac'), findsOneWidget);
    await tester.tap(_labeled('Add a Claktory'));
    await pumpFrames(tester);
    await tester.tap(find.text('Use a code instead'));
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, _mini);
    await tester.pump();
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    // A pasted code connects only once confirmed, in the same dialog.
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(app.connections.current, isNull);
    await tester.tap(find.text('Connect').last);
    await pumpFrames(tester);
    expect(_labeled('mac-mini'), findsOneWidget);
    expect(app.connections.current?.code, _mini);
    expect(app.backends[_mini]!.target?.code, _mini);
  });

  testWidgets("a Claktory's tile says when something there waits for you, "
      'before its news', (tester) async {
    final app = _App(
      snapshots: {
        '': _workspace('Rivet'),
        _nixos: _workspace('Nix', unseen: true, waiting: [4, 9]),
      },
    );
    app.connections.use(parsePairingCode(_nixos));
    app.connections.use(null);
    await app.show(tester);

    // Counted, not a dot.
    expect(_labeled('nixos, 2 in Needs you'), findsOneWidget);
    expect(
      find.descendant(of: find.byType(NeedsYouMark), matching: find.text('2')),
      findsOneWidget,
    );
    expect(_labeled('nixos has something new'), findsNothing);
  });

  testWidgets('on a phone, "‹ Claktories" counts what waits in the others '
      '(#529, MC10)', (tester) async {
    final app = _App(
      localBackend: false,
      snapshots: {
        // Over its name step, the row back to the Claktories.
        _nixos: _workspace('Nix', waiting: [4, 9])..settings.userName = '',
        _mini: _workspace('Mini', waiting: [2]),
      },
    );
    app.connections
      ..use(parsePairingCode(_mini))
      ..use(parsePairingCode(_nixos));
    await app.show(tester, size: const Size(390, 844));
    expect(find.text("What's your name?"), findsOneWidget);
    // Only the others: nixos shows, mini has 1.
    expect(_labeled('Back to Claktories, 1 in Needs you'), findsOneWidget);
  });

  for (final turned in ['sign-in', 'other account']) {
    testWidgets('on a phone, a Claktory that turns this one away ($turned) '
        'while its name step is up shows why at once (Sale row 173)', (
      tester,
    ) async {
      final app = _App(
        localBackend: false,
        snapshots: {
          _nixos: _workspace('Nix')..settings.userName = '',
          _mini: _workspace('Mini'),
        },
      );
      app.connections
        ..use(parsePairingCode(_mini))
        ..use(parsePairingCode(_nixos));
      await app.show(tester, size: const Size(390, 844));
      expect(find.text("What's your name?"), findsOneWidget);
      final backend = app.backends[_nixos]!;
      backend.connected.value = false;
      if (turned == 'sign-in') {
        backend.signInRequired.value =
            'This Claktory lets this device in only once it shows it is yours.';
      } else {
        backend.otherAccount.value = 'Open a pipe with them.';
      }
      await pumpFrames(tester);
      expect(find.text("What's your name?"), findsNothing);
      expect(find.byType(ClaktoriesBackRow), findsOneWidget);
      // Nor does it say it's connecting: it won't try again.
      expect(find.textContaining('Connecting'), findsNothing);
    });

    testWidgets('on a phone, a Claktory that turns this one away ($turned) '
        'keeps "‹ Claktories" to the others (Zucchero row 647)', (
      tester,
    ) async {
      final app = _App(
        localBackend: false,
        snapshots: {_nixos: _workspace('Nix'), _mini: _workspace('Mini')},
      );
      app.connections
        ..use(parsePairingCode(_mini))
        ..use(parsePairingCode(_nixos));
      await app.show(tester, size: const Size(390, 844));
      final backend = app.backends[_nixos]!;
      backend.connected.value = false;
      if (turned == 'sign-in') {
        backend.signInRequired.value =
            'This Claktory lets this device in only once it shows it is yours.';
      } else {
        backend.otherAccount.value = 'Open a pipe with them.';
      }
      await pumpFrames(tester);
      expect(find.byType(ClaktoriesBackRow), findsOneWidget);
    });
  }

  // Remove… on a Claktory from the account that wants
  // this device signed in forgot it at once, with no question.
  testWidgets('a Claktory from the account that wants a sign-in: Remove… '
      'asks first, as the rail does', (tester) async {
    final target = RemoteFactory.fromAccount(
      const AccountClaktory(id: 'n', name: 'nixos', endpointId: _nixosKey),
    );
    final app = _App(snapshots: {target.code: _workspace('Nix')});
    app.connections.use(target);
    await app.show(tester);
    final backend = app.backends[target.code]!;
    backend.connected.value = false;
    backend.signInRequired.value =
        'This Claktory lets this device in only once it shows it is yours.';
    await pumpFrames(tester);
    await tester.tap(find.text('Remove…').last);
    await pumpFrames(tester);
    expect(find.text('Remove nixos?'), findsOneWidget);
    expect(app.connections.saved, contains(target), reason: 'not yet');
  });

  testWidgets("a tile counts what waits in a Claktory's chats and pipes too, "
      'as Needs you does', (tester) async {
    final app = _App(
      snapshots: {
        '': _workspace('Rivet'),
        _nixos: _workspace('Nix', waiting: [4])
          ..chats.add(Chat(id: 'c1', waitingSeqs: [Int64(2)]))
          ..pipes.add(Pipe(id: 'p1', state: PipeState.PIPE_STATE_APPROVAL)),
      },
    );
    app.connections.use(parsePairingCode(_nixos));
    app.connections.use(null);
    await app.show(tester);

    expect(_labeled('nixos, 3 in Needs you'), findsOneWidget);
  });

  testWidgets("a tile doesn't wave for an agent only waiting out its usage "
      'limits', (tester) async {
    final app = _App(
      snapshots: {
        '': _workspace('Rivet'),
        _nixos: _workspace(
          'Nix',
          working: true,
          usageWait: 'Waiting for your Claude usage to reset at Fri 18:57',
        ),
      },
    );
    app.connections.use(parsePairingCode(_nixos));
    app.connections.use(null);
    await app.show(tester);

    expect(
      find.descendant(
        of: find.byType(ClaktoryRail),
        matching: find.byType(WorkingWave),
      ),
      findsNothing,
    );
  });

  testWidgets("a step over one Claktory leaves the rail free to switch", (
    tester,
  ) async {
    final app = _App(
      snapshots: {
        '': _workspace('Rivet'),
        // Asks the name first: a step over the window.
        _nixos: _workspace('Nix')..settings.userName = '',
      },
    );
    app.connections.use(parsePairingCode(_nixos));
    final semantics = tester.ensureSemantics();
    await app.show(tester);
    expect(find.text("What's your name?"), findsOneWidget);
    // VoiceOver reaches the rail too, and nothing else behind the step.
    expect(find.semantics.byLabel(RegExp('This Mac')), findsWidgets);
    // Nix, the agent behind it, isn't.
    expect(find.semantics.byLabel(RegExp('Nix')), findsNothing);
    semantics.dispose();

    await tester.tap(_labeled('This Mac'));
    await pumpFrames(tester);
    expect(app.connections.current, isNull);
    expect(find.text("What's your name?"), findsNothing);
  });

  testWidgets('on a phone, a step leaves the way back to the Claktories', (
    tester,
  ) async {
    final app = _App(
      localBackend: false,
      snapshots: {
        _nixos: _workspace('Nix')..settings.userName = '',
        _mini: _workspace('Mini'),
      },
    );
    app.connections
      ..use(parsePairingCode(_mini))
      ..use(parsePairingCode(_nixos));
    await app.show(tester, size: const Size(390, 844));
    expect(find.text("What's your name?"), findsOneWidget);

    await tester.tap(_labeled('Back to Claktories').hitTestable());
    await pumpFrames(tester);
    expect(find.byType(ClaktoryList), findsOneWidget);
  });

  testWidgets("on a phone, a link opened over the list of Claktories asks "
      "there, and one already added shows (Zucchero's row 685)", (
    tester,
  ) async {
    final app = _App(
      localBackend: false,
      snapshots: {_nixos: _workspace('Nix'), _mini: _workspace('Mini')},
    );
    app.connections
      ..use(parsePairingCode(_mini))
      ..use(parsePairingCode(_nixos));
    addTearDown(() {
      while (appLinks.take() != null) {}
    });
    await app.show(tester, size: const Size(390, 844));
    final back = tester.widget(
      find.byWidgetPredicate((w) => w.runtimeType.toString() == '_BackToList'),
    );
    (back as dynamic).back();
    await pumpFrames(tester);
    expect(find.byType(ClaktoryList), findsOneWidget);

    // A new one's code, as the Camera opens it.
    appLinks.receive('factory://?t=${'d' * 64}&name=studio&e=${'e' * 64}');
    await pumpFrames(tester);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(
      find.text('Connect to this Claktory?').hitTestable(),
      findsOneWidget,
    );
    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(find.byType(ClaktoryList), findsOneWidget);
    expect(app.connections.saved, hasLength(2));

    // One added already: it shows, its window in place of the list.
    appLinks.receive(_mini);
    await pumpFrames(tester);
    expect(app.connections.current?.code, _mini);
    expect(find.byType(ClaktoryList), findsNothing);
  });

  testWidgets('on a phone, the Claktories are a rail in the drawer; the '
      'list of them stays in reach', (tester) async {
    // Signed out, so Add a Claktory asks for the email first, with the
    // code a link away: and none can be checked, which each one's label
    // says.
    final app = _App(
      account: DeviceAccount(),
      localBackend: false,
      snapshots: {
        _nixos: _workspace('Nix'),
        _mini: _workspace('Mini', unseen: true),
      },
    );
    app.connections
      ..use(parsePairingCode(_nixos))
      ..use(parsePairingCode(_mini))
      ..use(parsePairingCode(_nixos));
    // The phone's camera, which FactoryApp asks the native side about:
    // none here, so Add a Claktory opens on pasting.
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      const MethodChannel('factory/device'),
      (call) async => call.method == 'cameraAccess' ? 'unavailable' : null,
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        const MethodChannel('factory/device'),
        null,
      ),
    );
    await app.show(tester, size: const Size(390, 844));
    expect(find.byType(ClaktoryRail), findsNothing);
    // The chosen one's conversation fills the screen; the drawer waits.
    expect(find.byType(Sidebar), findsNothing);
    tester
        .state<PhoneDrawerHostState>(find.byType(PhoneDrawerHost))
        .openDrawer();
    await pumpFrames(tester);
    expect(find.byType(PhoneClaktoryRail), findsOneWidget);
    expect(
      find.descendant(of: find.byType(Sidebar), matching: find.text('Nix')),
      findsWidgets,
    );
    expect(_labeled('mac-mini has something new'), findsOneWidget);

    // Another: its agents, in the drawer still open.
    await tester.tap(_labeled('mac-mini'));
    await pumpFrames(tester);
    expect(app.connections.current?.code, _mini);
    expect(
      find.descendant(of: find.byType(Sidebar), matching: find.text('Mini')),
      findsWidgets,
    );

    // Over a step that covers the window, the way to the list of them.
    final back = tester.widget(
      find.byWidgetPredicate((w) => w.runtimeType.toString() == '_BackToList'),
    );
    (back as dynamic).back();
    await pumpFrames(tester);
    expect(find.byType(ClaktoryList), findsOneWidget);
    expect(_labeled('nixos'), findsOneWidget);
    await tester.tap(_labeled('nixos'));
    await pumpFrames(tester);
    expect(find.byType(ClaktoryList), findsNothing);
    expect(app.connections.current?.code, _nixos);

    // Adding one pairs it, then shows it.
    tester
        .state<PhoneDrawerHostState>(find.byType(PhoneDrawerHost))
        .openDrawer();
    await pumpFrames(tester);
    await tester.tap(_labeled('Add a Claktory'));
    await pumpFrames(tester);
    expect(find.byType(PairScreen), findsOneWidget);
    // Signed out: the email sign-in first (area 2, 8.2), the code a link
    // away; no camera here, so it's pasted.
    expect(find.byKey(const ValueKey('device-email')), findsOneWidget);
    await tester.tap(find.text('Use a code instead'));
    await pumpFrames(tester);
    final third = 'factory://?t=${'e' * 64}&name=pi&e=${'f' * 64}';
    await tester.enterText(find.byType(EditableText), third);
    await tester.pump();
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    // A code asks first (confirmPairing), as a link does.
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    await tester.tap(find.text('Connect').last);
    await pumpFrames(tester);
    expect(find.byType(PairScreen), findsNothing);
    expect(app.connections.current?.code, third);
    expect(app.connections.saved, hasLength(3));
  });

  testWidgets('a Claktory found through the account and signed out of it: '
      "Remove… asks the rail's question first, never removes at once "
      "(Abbacchio's 1675)", (tester) async {
    final app = _App();
    final found = RemoteFactory.fromAccount(
      const AccountClaktory(
        id: 'n',
        name: 'nixos',
        endpointId: _nixosKey,
        relays: ['https://relay.example.com'],
      ),
    );
    app.connections.use(found);
    await app.show(tester);
    app.backends[found.code]!.signInRequired.value =
        'This Claktory was signed out of your Claktory account, so it '
        "can't let this device in.";
    await pumpFrames(tester);
    expect(find.text('nixos was signed out'), findsOneWidget);
    await tester.tap(find.text('Remove…'));
    await pumpFrames(tester);
    expect(find.text('Remove nixos?'), findsOneWidget);
    expect(app.connections.saved, contains(found));
  });

  testWidgets("the phone's list says what stops a Claktory waiting on a "
      "sign-in, never Connecting… (Zucchero's row 720)", (tester) async {
    final app = _App(
      account: DeviceAccount(),
      localBackend: false,
      snapshots: {_nixos: _workspace('Nix'), _mini: _workspace('Mini')},
    );
    app.connections
      ..use(parsePairingCode(_mini))
      ..use(parsePairingCode(_nixos));
    await app.show(tester, size: const Size(390, 844));
    app.backends[_mini]!
      ..connected.value = false
      ..signInRequired.value = 'Confirm this device is yours: sign in.';
    await pumpFrames(tester);
    final back = tester.widget(
      find.byWidgetPredicate((w) => w.runtimeType.toString() == '_BackToList'),
    );
    (back as dynamic).back();
    await pumpFrames(tester);
    expect(find.byType(ClaktoryList), findsOneWidget);
    expect(find.text('Sign in to connect'), findsOneWidget);
    expect(find.text('Connecting…'), findsNothing);
    // Signed out itself, it says so instead.
    app.backends[_mini]!.signInRequired.value =
        'This Claktory was signed out of your Claktory account, so it '
        "can't let this device in.";
    await pumpFrames(tester);
    expect(find.text('Signed out'), findsOneWidget);
  });

  testWidgets("a pushed page's back row shows its whole label where it "
      'fits, else the chevron alone, grown with the text; never cut or '
      'wrapped (Lucy 50050)', (tester) async {
    final long = 'factory://?t=${'e' * 64}&name=atticstudio&e=${'f' * 64}';
    for (final scale in const [1.0, 3.12]) {
      await tester.pumpWidget(const SizedBox());
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final app = _App(account: DeviceAccount(), localBackend: false);
      app.connections.use(parsePairingCode(long));
      await app.show(tester, size: const Size(320, 568));
      PhoneClaktories.showAdding(code: true);
      await pumpFrames(tester);
      // Where it goes, said either way.
      final back = _labeled('Back to atticstudio');
      expect(back, findsOneWidget, reason: '$scale');
      final label = find.text('atticstudio');
      final chevron = tester
          .widgetList<ExtensionGlyph>(find.byType(ExtensionGlyph))
          .firstWhere((glyph) => glyph.icon == 'chevron-left');
      if (scale == 1) {
        expect(label, findsOneWidget);
        expect(chevron.size, 16);
      } else {
        expect(label, findsNothing);
        expect(chevron.size, greaterThan(16));
      }
      expect(tester.takeException(), isNull);
    }
  });

  testWidgets("on a phone, a Claktory's Use a new code pushes Add a Claktory "
      'at its code, with Back to that Claktory (area 2, 8.4; Lucy 49058)', (
    tester,
  ) async {
    final app = _App(
      account: DeviceAccount(),
      localBackend: false,
      snapshots: {_nixos: _workspace('Nix')},
    );
    app.connections.use(parsePairingCode(_nixos));
    await app.show(tester, size: const Size(390, 844));
    app.backends[_nixos]!
      ..connected.value = false
      ..refused.value = 'unknown device';
    await pumpFrames(tester);
    expect(find.text('nixos no longer allows this $thisDeviceKind'), findsOne);
    await tester.tap(find.text('Use a new code'));
    await pumpFrames(tester);
    // The phone's own page, not the Mac's chooser in a sheet; at the code,
    // though this phone is signed out (which starts others at the email).
    expect(find.byType(PairScreen), findsOneWidget);
    expect(find.byType(ClaktoryList), findsNothing);
    expect(find.byKey(const ValueKey('device-email')), findsNothing);
    expect(find.text('Sign in with your email instead'), findsOneWidget);
    // Back, named for where it goes: the Claktory's page again.
    await tester.tap(find.text('nixos').first);
    await pumpFrames(tester);
    expect(find.byType(PairScreen), findsNothing);
    expect(find.text('nixos no longer allows this $thisDeviceKind'), findsOne);
  });

  for (final (scale, size) in [
    for (final scale in const [1.0, 3.1])
      for (final size in const [Size(320, 568), Size(568, 320), Size(390, 844)])
        (scale, size),
  ]) {
    testWidgets(
      'the phone list fits a ${size.width.toInt()}×'
      '${size.height.toInt()} screen${scale > 1 ? ' at the largest text' : ''}, long names and problems too',
      (tester) async {
        final long =
            'factory://?t=${'e' * 64}&name=a-very-long-machine-name-for-a-'
            'small-screen&e=${'f' * 64}';
        // Signed out: each row is marked too, which must fit.
        final app = _App(
          account: DeviceAccount(),
          localBackend: false,
          snapshots: {
            _nixos: _workspace('Nix', working: true),
            _mini: _workspace('Mini', unseen: true),
            long: _workspace('Long'),
          },
        );
        app.connections
          ..use(parsePairingCode(_nixos))
          ..use(parsePairingCode(_mini))
          ..use(parsePairingCode(long))
          ..use(parsePairingCode(_nixos));
        await app.show(tester, size: size);
        // One that can't be reached, and one that turned this device away.
        app.backends[_mini]!.connected.value = false;
        app.backends[_mini]!.tunnel.value = const TunnelStatus(
          phase: TunnelPhase.failed,
          path: TunnelPath.none,
          error: 'timed out',
          failure: TunnelFailure.unreachable,
          relayConnected: false,
          relayUpMs: 0,
          relayTrouble: TunnelRelayTrouble.none,
          received: 0,
          linkReceived: 0,
          dialTimeoutMs: 0,
          dialMs: 0,
          reach: TunnelReach.none,
        );
        app.backends[long]!.connected.value = false;
        app.backends[long]!.refused.value = 'unknown device';
        (tester.widget(
          find.byWidgetPredicate(
            (w) => w.runtimeType.toString() == '_BackToList',
          ),
        ) as dynamic).back();
        await pumpFrames(tester);
        // The list, at this text size: each row grows with it.
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        await pumpFrames(tester);
        expect(find.byType(ClaktoryList), findsOneWidget);
        for (final line in ['Working', "Can't connect", 'Not allowed']) {
          await tester.scrollUntilVisible(
            find.text(line),
            100,
            scrollable: find.byType(Scrollable).first,
          );
        }
        // The last rows are reachable however short the screen: scrolled
        // until it can be tapped, not only until it's built.
        await tester.scrollUntilVisible(
          find.text('Add a Claktory').hitTestable(),
          100,
          scrollable: find.byType(Scrollable).first,
        );
        await tester.tap(find.text('Add a Claktory'));
        await pumpFrames(tester);
        expect(find.byType(PairScreen), findsOneWidget);
        // Overflows would have failed the test by now.
      },
    );
  }

  // At the largest text the initials grew past their
  // 28 pt tile; they fill it at most.
  testWidgets('at the largest text the initials stay inside their tile', (
    tester,
  ) async {
    tester.platformDispatcher.textScaleFactorTestValue = 53 / 17;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final app = _App(
      snapshots: {'': _workspace('Rivet'), _nixos: _workspace('Nix')},
    );
    app.connections
      ..use(parsePairingCode(_nixos))
      ..use(parsePairingCode(_mini))
      ..use(null);
    await app.show(tester);
    await pumpFrames(tester);
    for (final initials in ['N', 'MM']) {
      final text = find.descendant(
        of: find.byType(ClaktoryRail),
        matching: find.text(initials),
      );
      expect(text, findsOneWidget);
      final tile = tester.getRect(
        find.ancestor(of: text, matching: find.byType(Stack)).first,
      );
      expect(tile.size, const Size.square(28));
      // Laid out whole, on one line (not squeezed into a box narrower than
      // the letters and painted past it)...
      final paragraph = tester.renderObject<RenderParagraph>(text);
      expect(
        paragraph.getMaxIntrinsicWidth(double.infinity),
        lessThanOrEqualTo(paragraph.size.width + 0.01),
        reason: initials,
      );
      // ...and inside the tile as drawn.
      final drawn = tester.getRect(text);
      expect(drawn.left, greaterThanOrEqualTo(tile.left), reason: initials);
      expect(drawn.right, lessThanOrEqualTo(tile.right), reason: initials);
      expect(drawn.top, greaterThanOrEqualTo(tile.top), reason: initials);
      expect(drawn.bottom, lessThanOrEqualTo(tile.bottom), reason: initials);
      // Still larger than at the standard size, room allowing.
      if (initials == 'N') expect(drawn.height, greaterThan(11));
    }
  });

  testWidgets('removing one from the phone list stays on the list', (
    tester,
  ) async {
    final app = _App(localBackend: false);
    app.connections
      ..use(parsePairingCode(_nixos))
      ..use(parsePairingCode(_mini));
    await app.show(tester, size: const Size(390, 844));
    (tester.widget(
      find.byWidgetPredicate((w) => w.runtimeType.toString() == '_BackToList'),
    ) as dynamic).back();
    await pumpFrames(tester);
    await tester.longPress(_labeled('mac-mini'));
    await pumpFrames(tester);
    await tester.tap(find.text('Remove…'));
    await pumpFrames(tester);
    await tester.tap(find.text('Remove'));
    await pumpFrames(tester);
    expect(app.connections.saved.map((t) => t.code), [_nixos]);
    expect(find.byType(ClaktoryList), findsOneWidget);
    expect(_labeled('mac-mini'), findsNothing);
  });

  testWidgets('on its side, the phone list keeps clear of the notch', (
    tester,
  ) async {
    tester.view.padding = const FakeViewPadding(left: 47, right: 47);
    addTearDown(tester.view.resetPadding);
    final app = _App(localBackend: false);
    app.connections.use(parsePairingCode(_nixos));
    await app.show(tester, size: const Size(568, 320));
    (tester.widget(
      find.byWidgetPredicate((w) => w.runtimeType.toString() == '_BackToList'),
    ) as dynamic).back();
    await pumpFrames(tester);
    expect(tester.getTopLeft(find.text('Claktories')).dx, greaterThan(47));
  });

  group('Not on the account', () {
    final pi = 'factory://?t=${'e' * 64}&name=pi&e=${'F' * 64}';

    test("the account's list decides, by the endpoint dialled", () async {
      final api = FakeAccountApi()
        ..claktoryList = [
          const AccountClaktory(id: 'n', name: 'nixos', endpointId: _nixosKey),
        ];
      var signedOut = false;
      var who = 'a@example.com';
      Object? failure;
      var now = DateTime(2026, 11, 23, 21, 39);
      final changes = ChangeNotifier();
      final account = ChangeNotifier();
      final remotes = [_nixosKey];
      final match = AccountMatch(
        remotes: () => remotes,
        changes: changes,
        account: account,
        theirs: () => signedOut
            ? null
            : failure == null
            ? api.claktories()
            : Future.error(failure),
        whose: () => who,
        now: () => now,
      );
      addTearDown(match.dispose);
      expect(match.check(_nixosKey), isNull, reason: 'not read yet');
      expect(match.check(null), AccountCheck.theirs, reason: 'its own');
      await pumpEventQueue();
      expect(match.check(_nixosKey), AccountCheck.theirs);
      expect(match.check(_nixosKey.toUpperCase()), AccountCheck.theirs);
      expect(match.check(_miniKey), AccountCheck.notTheirs);
      expect(match.check(''), AccountCheck.notTheirs, reason: 'a direct code');

      // Paired since: read again.
      api.claktoryList = [
        ...api.claktoryList,
        const AccountClaktory(id: 'm', name: 'mac-mini', endpointId: _miniKey),
      ];
      remotes.add(_miniKey);
      changes.notifyListeners();
      await pumpEventQueue();
      expect(match.check(_miniKey), AccountCheck.theirs);

      // Unreadable now: the list read stands.
      failure = const AccountServiceUnreachable('offline');
      account.notifyListeners();
      await pumpEventQueue();
      expect(match.check(_miniKey), AccountCheck.theirs);

      signedOut = true;
      account.notifyListeners();
      await pumpEventQueue();
      expect(match.check(_nixosKey), AccountCheck.signedOut);
      expect(match.check(null), AccountCheck.theirs);

      // Signed in again, but never read since: nothing can be checked,
      // once reads have failed for [AccountMatch.unsureFor].
      signedOut = false;
      account.notifyListeners();
      await pumpEventQueue();
      expect(match.check(_nixosKey), isNull, reason: 'not for long yet');
      now = now.add(AccountMatch.unsureFor);
      account.notifyListeners();
      await pumpEventQueue();
      expect(match.check(_nixosKey), AccountCheck.unreadable);
      failure = null;
      account.notifyListeners();
      await pumpEventQueue();
      expect(match.check(_nixosKey), AccountCheck.theirs);
      expect(AccountCheck.theirs.marked, isFalse);
      expect(AccountCheck.signedOut.marked, isTrue);
      expect(AccountCheck.signedOut.alarming, isFalse);
      expect(AccountCheck.unreadable.alarming, isTrue);

      // Signed in as someone else, whose list can't be read: the first
      // one's never stands in for it.
      who = 'b@example.com';
      failure = const AccountServiceUnreachable('offline');
      account.notifyListeners();
      await pumpEventQueue();
      expect(match.check(_nixosKey), isNull);
      now = now.add(AccountMatch.unsureFor);
      account.notifyListeners();
      await pumpEventQueue();
      expect(match.check(_nixosKey), AccountCheck.unreadable);
      failure = null;
      api.claktoryList = [];
      account.notifyListeners();
      await pumpEventQueue();
      expect(match.check(_nixosKey), AccountCheck.notTheirs);
    });

    testWidgets("just launched on a slow network, nothing's marked until "
        'reads have failed for 30 s, tried again 2, 4, 8 and 16 s apart '
        '(#388)', (tester) async {
      var now = DateTime(2026, 11, 23, 21, 39, 11);
      Object? failure = const AccountServiceUnreachable('offline');
      var reads = 0;
      final api = FakeAccountApi()
        ..claktoryList = [
          const AccountClaktory(id: 'n', name: 'nixos', endpointId: _nixosKey),
        ];
      final match = AccountMatch(
        remotes: () => [_nixosKey],
        changes: ChangeNotifier(),
        account: ChangeNotifier(),
        theirs: () {
          reads++;
          return failure == null ? api.claktories() : Future.error(failure);
        },
        now: () => now,
      );
      addTearDown(match.dispose);
      var told = 0;
      match.addListener(() => told++);
      Future<void> after(int seconds) async {
        now = now.add(Duration(seconds: seconds));
        await tester.pump(Duration(seconds: seconds));
      }

      await tester.pump();
      expect(reads, 1);
      expect(match.check(_nixosKey), isNull);
      for (final (wait, read) in [(2, 2), (4, 3), (8, 4)]) {
        await after(wait - 1);
        expect(reads, read - 1, reason: 'not before ${wait}s');
        await after(1);
        expect(reads, read);
        expect(match.check(_nixosKey), isNull, reason: 'still unsure');
      }
      expect(told, 0, reason: 'nothing marked meanwhile');
      // 30 s after the first failure: unchecked, and said so.
      await after(16);
      expect(reads, 5);
      expect(match.check(_nixosKey), AccountCheck.unreadable);
      expect(told, 1);
      // Then once a minute, as before; read, it's theirs.
      failure = null;
      await after(59);
      expect(reads, 5);
      await after(1);
      expect(reads, 6);
      expect(match.check(_nixosKey), AccountCheck.theirs);
    });

    testWidgets('a read that works within the 30 s marks nothing at all', (
      tester,
    ) async {
      var now = DateTime(2026, 11, 23, 21, 39, 51);
      Object? failure = const AccountServiceUnreachable('offline');
      final api = FakeAccountApi()
        ..claktoryList = [
          const AccountClaktory(id: 'n', name: 'nixos', endpointId: _nixosKey),
        ];
      final seen = <AccountCheck?>[];
      late final AccountMatch match;
      match = AccountMatch(
        remotes: () => [_nixosKey],
        changes: ChangeNotifier(),
        account: ChangeNotifier(),
        theirs: () =>
            failure == null ? api.claktories() : Future.error(failure),
        now: () => now,
      )..addListener(() => seen.add(match.check(_nixosKey)));
      addTearDown(match.dispose);
      await tester.pump();
      now = now.add(const Duration(seconds: 2));
      await tester.pump(const Duration(seconds: 2));
      failure = null;
      now = now.add(const Duration(seconds: 4));
      await tester.pump(const Duration(seconds: 4));
      expect(match.check(_nixosKey), AccountCheck.theirs);
      expect(seen, [AccountCheck.theirs]);
      expect(match.check(_miniKey), AccountCheck.notTheirs);
    });

    testWidgets('one not on it is marked: its tile, its card and its form; '
        'never this Mac or one that is', (tester) async {
      final app = _App();
      app.connections
        ..use(parsePairingCode(_nixos))
        ..use(parsePairingCode(pi))
        ..use(null);
      await app.show(tester);
      expect(_labeled('This Mac'), findsOneWidget);
      expect(_labeled('nixos'), findsOneWidget);
      expect(_labeled('pi, not on your Claktory account'), findsOneWidget);
      Color? ring(String initials) {
        final tile = find.ancestor(
          of: find.text(initials),
          matching: find.byWidgetPredicate(
            (w) => w.runtimeType.toString() == '_Tile',
          ),
        );
        final box = tester.widget<AnimatedContainer>(
          find.descendant(of: tile, matching: find.byType(AnimatedContainer)),
        );
        return ((box.decoration as BoxDecoration?)?.border as Border?)
            ?.top
            .color;
      }

      final palette = Palette.of(tester.element(find.byType(ClaktoryRail)));
      expect(ring('P'), palette.danger);
      expect(ring('N'), isNot(palette.danger));

      // Its card, on hover.
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      addTearDown(mouse.removePointer);
      await mouse.addPointer(location: Offset.zero);
      await mouse.moveTo(tester.getCenter(find.text('P')));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(
        find.textContaining('Not on your Claktory account', findRichText: true),
        findsOneWidget,
      );
      expect(
        find.text(
          "It may belong to someone else. If it's yours, sign it in to your "
          'Claktory account.',
        ),
        findsOneWidget,
      );
      await mouse.moveTo(Offset.zero);
      await pumpFrames(tester, const Duration(seconds: 1));

      // Its form, on a right click.
      await tester.tap(
        _labeled('pi, not on your Claktory account'),
        buttons: kSecondaryButton,
      );
      await pumpFrames(tester);
      expect(
        find.textContaining('Not on your Claktory account', findRichText: true),
        findsOneWidget,
      );
      await tester.tap(find.text('Cancel'));
      await pumpFrames(tester);
      await tester.tap(_labeled('nixos'), buttons: kSecondaryButton);
      await pumpFrames(tester);
      expect(
        find.textContaining('Not on your Claktory account', findRichText: true),
        findsNothing,
      );
    });

    testWidgets('signed out, nothing is known, so nothing is marked: no '
        "label, no ring, nothing on its card; its form says so, with Sign "
        'in… (#529, S2)', (tester) async {
      final app = _App(account: DeviceAccount());
      app.connections
        ..use(parsePairingCode(_nixos))
        ..use(null);
      await app.show(tester);
      expect(_labeled('This Mac'), findsOneWidget);
      expect(_labeled('nixos'), findsOneWidget);
      final palette = Palette.of(tester.element(find.byType(ClaktoryRail)));
      final tile = find.ancestor(
        of: find.text('N'),
        matching: find.byWidgetPredicate(
          (w) => w.runtimeType.toString() == '_Tile',
        ),
      );
      final box = tester.widget<AnimatedContainer>(
        find.descendant(of: tile, matching: find.byType(AnimatedContainer)),
      );
      expect(
        ((box.decoration as BoxDecoration?)?.border as Border?)?.top.color,
        isNot(palette.danger),
      );
      // Its card, on hover: its name and connection only.
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      addTearDown(mouse.removePointer);
      await mouse.addPointer(location: Offset.zero);
      await mouse.moveTo(tester.getCenter(find.text('N')));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.textContaining('Claktory account'), findsNothing);
      expect(find.text('Sign in…'), findsNothing);
      await mouse.moveTo(Offset.zero);
      await pumpFrames(tester, const Duration(seconds: 1));
      // Its details: what's known, and the way to sign in.
      await tester.tap(_labeled('nixos'), buttons: kSecondaryButton);
      await pumpFrames(tester);
      expect(
        find.text(
          'Sign in to your Claktory account in this app to check which '
          'Claktories are on it.',
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Sign in…'));
      await pumpFrames(tester);
      expect(find.text('Sign in to your Claktory account'), findsOneWidget);
      expect(
        find.text('So this app can recognize the Claktories on your account.'),
        findsOneWidget,
      );
    });

    testWidgets('on a phone, signed out, neither the list, its rows nor the '
        "drawer's header mark anything (#529, S2)", (tester) async {
      final app = _App(localBackend: false, account: DeviceAccount());
      app.connections
        ..use(parsePairingCode(_mini))
        ..use(parsePairingCode(_nixos));
      await app.show(tester, size: const Size(390, 844));
      tester
          .state<PhoneDrawerHostState>(find.byType(PhoneDrawerHost))
          .openDrawer();
      await pumpFrames(tester);
      expect(find.textContaining('Claktory account'), findsNothing);
      expect(find.text('Sign in…'), findsNothing);
      expect(_labeled('nixos'), findsOneWidget);
      final back = tester.widget(
        find.byWidgetPredicate(
          (w) => w.runtimeType.toString() == '_BackToList',
        ),
      );
      (back as dynamic).back();
      await pumpFrames(tester);
      expect(find.byType(ClaktoryList), findsOneWidget);
      expect(find.textContaining('Claktory account'), findsNothing);
      expect(find.text('Sign in…'), findsNothing);
    });

    testWidgets("on a phone, the drawer header's connection word follows the "
        'connection by itself (#388)', (tester) async {
      final app = _App(localBackend: false, account: DeviceAccount());
      app.connections.use(parsePairingCode(_nixos));
      await app.show(tester, size: const Size(390, 844));
      tester
          .state<PhoneDrawerHostState>(find.byType(PhoneDrawerHost))
          .openDrawer();
      await pumpFrames(tester);
      final header = find.byWidgetPredicate(
        (w) => w.runtimeType.toString() == '_DrawerHeader',
      );
      Finder word(String text) =>
          find.descendant(of: header, matching: find.text(text));
      final backend = app.backends[_nixos]!;
      expect(backend.connected.value, isTrue);
      expect(word('Connected'), findsOneWidget);
      // Down, then welcomed again, with no new Workspace: the word follows.
      backend.connected.value = false;
      await pumpFrames(tester);
      expect(word('Connecting…'), findsOneWidget);
      backend.connected.value = true;
      await pumpFrames(tester);
      expect(word('Connected'), findsOneWidget);
    });

    testWidgets("on a phone, the drawer's header and the list say so too", (
      tester,
    ) async {
      final app = _App(localBackend: false);
      app.connections
        ..use(parsePairingCode(_nixos))
        ..use(parsePairingCode(pi));
      await app.show(tester, size: const Size(390, 844));
      tester
          .state<PhoneDrawerHostState>(find.byType(PhoneDrawerHost))
          .openDrawer();
      await pumpFrames(tester);
      expect(_labeled('pi, not on your Claktory account'), findsOneWidget);
      expect(
        find.textContaining('Not on your Claktory account', findRichText: true),
        findsOneWidget,
      );
      await tester.tap(_labeled('nixos'));
      await pumpFrames(tester);
      expect(
        find.textContaining('Not on your Claktory account', findRichText: true),
        findsNothing,
      );

      (tester.widget(
        find.byWidgetPredicate(
          (w) => w.runtimeType.toString() == '_BackToList',
        ),
      ) as dynamic).back();
      await pumpFrames(tester);
      expect(find.byType(ClaktoryList), findsOneWidget);
      expect(
        find.textContaining('Not on your Claktory account', findRichText: true),
        findsOneWidget,
      );
      expect(_labeled('pi, not on your Claktory account'), findsOneWidget);
    });
  });

  testWidgets('another Claktory asks the name with the one this Mac knows', (
    tester,
  ) async {
    final app = _App(
      snapshots: {
        _nixos: _workspace('Rivet')..settings = Settings(userName: ''),
      },
    );
    await app.show(tester);
    expect(find.text("What's your name?"), findsNothing);
    app.connections.use(parsePairingCode(_nixos));
    await pumpFrames(tester);
    expect(find.text("What's your name?"), findsOneWidget);
    final field = tester.widget<EditableText>(
      find.descendant(
        of: find.byType(NameStep),
        matching: find.byType(EditableText),
      ),
    );
    expect(field.controller.text, 'Matt');
    expect(field.controller.selection.end, 4);
  });
}
