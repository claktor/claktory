import 'package:factory_app/backend.dart';
import 'package:factory_app/connect_step.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/main.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:factory_app/setup.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'setup_test.dart' show FakeSetupChannel;

final _field = find.descendant(
  of: find.byType(ConnectStep),
  matching: find.byType(EditableText),
);

final _box = 'factory://?t=${'a' * 64}&name=box&e=${'c' * 64}';

Workspace _workspace() => Workspace(
  agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
  selectedAgentId: 'rivet',
  settings: Settings(userName: 'Matt'),
);

/// A Mac that uses a Claktory on another computer: none of its own, a
/// code pasted to connect, or Claktory set up here after all.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('Claktories', () {
    test("this Mac's own Claktory comes and goes, and the choice is kept", () {
      final kept = <bool>[];
      final opened = <String>[];
      final connections = Connections(MemoryConnectionStorage());
      final claktories = Claktories(
        connections: connections,
        localBackend: false,
        open: (target) {
          opened.add(target?.code ?? '');
          return RecordingBackend()..connectTo(target);
        },
        drafts: DeviceDrafts(MemoryDraftStorage()),
        keepLocal: (local) async => kept.add(local),
      );
      addTearDown(claktories.dispose);
      expect(claktories.all, isEmpty);
      expect(claktories.canChooseLocal, isTrue);

      connections.use(parsePairingCode(_box));
      expect(claktories.all.map((c) => c.key), [_box]);

      claktories.setLocalBackend(true);
      expect(kept, [true]);
      expect(claktories.all.map((c) => c.key), ['', _box]);
      // Its own shows, chosen just now; made afresh.
      expect(claktories.selected.key, '');
      expect(opened.where((key) => key == ''), hasLength(2));

      claktories.setLocalBackend(false);
      expect(kept, [true, false]);
      expect(claktories.all.map((c) => c.key), [_box]);
      expect(claktories.selected.key, _box);
    });
  });

  Future<(FakeSetupChannel, Connections)> showApp(
    WidgetTester tester, {
    required bool local,
    Map<String, InstallResult> results = const {},
  }) async {
    final channel = FakeSetupChannel(results: Map.of(results));
    final connections = Connections(MemoryConnectionStorage());
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      FactoryApp(
        drafts: DeviceDrafts(MemoryDraftStorage()),
        connections: connections,
        open: (target) {
          final Backend backend = RecordingBackend()..connectTo(target);
          backend.workspace.apply(_workspace());
          return backend;
        },
        localBackend: local,
        setupChannel: channel,
      ),
    );
    await pumpFrames(tester);
    return (channel, connections);
  }

  testWidgets('with no Claktory of its own, a Mac asks for a code, and '
      'connects with it', (tester) async {
    final (_, connections) = await showApp(
      tester,
      local: false,
      results: {
        'install --check': const InstallResult(result: 'not_installed'),
      },
    );
    expect(find.byType(ConnectStep), findsOneWidget);
    expect(find.text('Connect to your Claktory'), findsOneWidget);

    await tester.enterText(_field, 'nonsense');
    await pumpFrames(tester);
    expect(find.text("That isn't a Claktory code."), findsOneWidget);

    await tester.enterText(_field, _box);
    await pumpFrames(tester);
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    // Asked first, as every way a code comes in is (Fugo).
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(connections.current, isNull);
    await tester.tap(find.text('Connect').last);
    await pumpFrames(tester);
    expect(connections.current?.code, _box);
    expect(find.byType(ConnectStep), findsNothing);
  });

  testWidgets('a claktory:// code is taken as a factory:// one, asking '
      'first', (tester) async {
    final (_, connections) = await showApp(
      tester,
      local: false,
      results: {
        'install --check': const InstallResult(result: 'not_installed'),
      },
    );
    final code = _box.replaceFirst('factory://', 'claktory://');
    await tester.enterText(_field, code);
    await pumpFrames(tester);
    expect(find.text("That isn't a Claktory code."), findsNothing);
    // A quick double click asks once (Abbacchio).
    await tester.tap(find.text('Connect'));
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(connections.current, isNull);
    await tester.tap(find.text('Connect').last);
    await pumpFrames(tester);
    expect(connections.current?.code, code);
  });

  testWidgets('Set up Claktory on this Mac sets it up, without asking '
      'again', (tester) async {
    final (channel, _) = await showApp(
      tester,
      local: false,
      results: {
        'install --check': const InstallResult(result: 'not_installed'),
        'install': const InstallResult(result: 'installed'),
      },
    );
    channel.appOnlyMark = true;
    await tester.tap(find.text('Set up Claktory on this Mac'));
    await pumpFrames(tester);
    expect(channel.appOnlyMark, isFalse);
    expect(channel.runs, ['install --check', 'install']);
    expect(find.byType(ConnectStep), findsNothing);
  });

  testWidgets('the first launch asks where Claktory runs; another '
      'computer installs nothing and asks for its code', (tester) async {
    final (channel, _) = await showApp(
      tester,
      local: true,
      results: {
        'install --check': const InstallResult(result: 'not_installed'),
      },
    );
    expect(find.text('Set up Claktory'), findsOneWidget);
    await tester.tap(find.text('Use a Claktory on another computer'));
    await pumpFrames(tester);
    expect(channel.appOnlyMark, isTrue);
    expect(channel.runs, ['install --check']);
    expect(find.byType(ConnectStep), findsOneWidget);
  });
}
