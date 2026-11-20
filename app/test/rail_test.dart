import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/main.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/tunnel_client.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _nixos = 'factory://?t=${'a' * 64}&name=nixos&e=${'c' * 64}';
final _mini = 'factory://?t=${'b' * 64}&name=mac-mini&e=${'d' * 64}';

Workspace _workspace(
  String agent, {
  bool working = false,
  bool unseen = false,
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
  _App({this.snapshots = const {}, this.localBackend = true});

  /// A Mac, which runs a Claktory of its own; a phone doesn't.
  final bool localBackend;

  final Map<String, Workspace> snapshots;
  final storage = MemoryConnectionStorage();
  late final connections = Connections(storage);
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

      app.connections.rename(nixos, 'Home server');
      expect(app.connections.nameOf(nixos), 'Home server');
      expect(app.backends[_nixos]!.target?.label, 'Home server');
      expect(app.storage.kept.names, {_nixos: 'Home server'});
      app.connections.rename(null, 'Studio');
      expect(app.connections.nameOf(null), 'Studio');
      app.connections.rename(null, '');
      expect(app.connections.nameOf(null), 'This Mac');

      app.connections.forget(nixos);
      expect(claktories.all.map((c) => c.key), ['', _mini]);
      expect(claktories.selected.key, '', reason: "this Mac's instead");
      expect(app.storage.kept.names, isEmpty);
    });

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

    test('names and the ones showing are kept on the device', () async {
      final storage = MemoryConnectionStorage();
      final connections = Connections(storage)
        ..use(parsePairingCode(_nixos))
        ..rename(parsePairingCode(_nixos), 'Home');
      expect(connections.current?.label, 'Home');
      final again = await Connections.load(storage);
      expect(again.current?.label, 'Home');
      expect(again.saved.single.label, 'Home');
      expect(again.nameOf(null), 'This Mac');
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
      _labeled("This device isn't paired with this Claktory any more"),
      findsOneWidget,
    );
  });

  testWidgets('a right click renames or removes one', (tester) async {
    final app = _App();
    app.connections.use(parsePairingCode(_nixos));
    await app.show(tester);

    await tester.tap(_labeled('nixos'), buttons: kSecondaryButton);
    await pumpFrames(tester);
    expect(find.text('Name'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, 'Home server');
    await tester.tap(find.text('Save'));
    await pumpFrames(tester);
    expect(_labeled('Home server'), findsOneWidget);
    expect(find.text('HS'), findsOneWidget);

    await tester.tap(_labeled('Home server'), buttons: kSecondaryButton);
    await pumpFrames(tester);
    await tester.tap(find.text('Remove…'));
    await pumpFrames(tester);
    expect(find.text('Remove Home server?'), findsOneWidget);
    app.backends[_nixos]!.deviceId = 'phone-1';
    await tester.tap(find.text('Remove'));
    await pumpFrames(tester);
    expect(_labeled('Home server'), findsNothing);
    expect(app.connections.saved, isEmpty);
    expect(app.connections.current, isNull, reason: "this Mac's shows");
    expect(
      app.backends[_nixos]!.sent.last.unpairDevice.deviceId,
      'phone-1',
      reason: 'it leaves its devices',
    );
  });

  testWidgets('"+" adds one by its code', (tester) async {
    final app = _App();
    await app.show(tester);
    expect(_labeled('This Mac'), findsOneWidget);
    await tester.tap(_labeled('Add a Claktory'));
    await pumpFrames(tester);
    expect(find.text('Your Claktories'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, _mini);
    await tester.pump();
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    expect(_labeled('mac-mini'), findsOneWidget);
    expect(app.connections.current?.code, _mini);
    expect(app.backends[_mini]!.target?.code, _mini);
  });

  testWidgets('on a phone, the Claktories are a list at the top', (
    tester,
  ) async {
    final app = _App(
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
    await app.show(tester, size: const Size(390, 844));
    expect(find.byType(ClaktoryRail), findsNothing);
    // The chosen one shows first.
    expect(
      find.descendant(of: find.byType(Sidebar), matching: find.text('Nix')),
      findsWidgets,
    );

    // Back to the list (the sidebar's "‹ Claktories" row).
    final back = tester.widget(
      find.byWidgetPredicate((w) => w.runtimeType.toString() == '_BackToList'),
    );
    (back as dynamic).back();
    await pumpFrames(tester);
    expect(find.byType(ClaktoryList), findsOneWidget);
    expect(_labeled('nixos'), findsOneWidget);
    expect(_labeled('mac-mini has something new'), findsOneWidget);

    await tester.tap(_labeled('mac-mini'));
    await pumpFrames(tester);
    expect(find.byType(ClaktoryList), findsNothing);
    expect(app.connections.current?.code, _mini);
    expect(
      find.descendant(of: find.byType(Sidebar), matching: find.text('Mini')),
      findsWidgets,
    );

    // Adding one pairs it, then shows it.
    (tester.widget(
      find.byWidgetPredicate((w) => w.runtimeType.toString() == '_BackToList'),
    ) as dynamic).back();
    await pumpFrames(tester);
    await tester.tap(_labeled('Add a Claktory'));
    await pumpFrames(tester);
    expect(find.byType(PairScreen), findsOneWidget);
    final third = 'factory://?t=${'e' * 64}&name=pi&e=${'f' * 64}';
    await tester.enterText(find.byType(EditableText), third);
    await tester.pump();
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    expect(find.byType(PairScreen), findsNothing);
    expect(app.connections.current?.code, third);
    expect(app.connections.saved, hasLength(3));
  });

  for (final size in const [Size(320, 568), Size(568, 320), Size(390, 844)]) {
    testWidgets('the phone list fits a ${size.width.toInt()}×'
        '${size.height.toInt()} screen, long names and problems too', (
      tester,
    ) async {
      final long =
          'factory://?t=${'e' * 64}&name=a-very-long-machine-name-for-a-'
          'small-screen&e=${'f' * 64}';
      final app = _App(
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
      );
      app.backends[long]!.connected.value = false;
      app.backends[long]!.refused.value = 'unknown device';
      (tester.widget(
        find.byWidgetPredicate(
          (w) => w.runtimeType.toString() == '_BackToList',
        ),
      ) as dynamic).back();
      await pumpFrames(tester);
      expect(find.byType(ClaktoryList), findsOneWidget);
      expect(find.text("Can't connect to mac-mini"), findsOneWidget);
      expect(
        find.textContaining("This device isn't paired with"),
        findsOneWidget,
      );
      expect(find.text('Working'), findsOneWidget);
      // The last rows are reachable however short the screen.
      await tester.scrollUntilVisible(
        find.text('Add a Claktory'),
        100,
        scrollable: find.byType(Scrollable).first,
      );
      await tester.tap(find.text('Add a Claktory'));
      await pumpFrames(tester);
      expect(find.byType(PairScreen), findsOneWidget);
      // Overflows would have failed the test by now.
    });
  }

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
}
