// Connecting this Mac from a shell (#245, setup_link.dart): a request
// connects a Mac with no Claktory saved, says how it goes (connecting,
// waiting with the digits and this device's key, connected), and tells the
// person once; a Mac already set up never takes one.
import 'package:factory_app/connections.dart';
import 'package:factory_app/pairing.dart' show PairingWait;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/setup_link.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _code = 'factory://?t=${'a' * 64}&name=box&e=${'c' * 64}';

void main() {
  final said = <Map<Object?, Object?>>[];

  setUp(() {
    said.clear();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(setupLinks.channel, (call) async {
          if (call.method == 'status') {
            said.add(Map.of(call.arguments as Map));
          }
          return call.method == 'listen' ? <String>[] : null;
        });
  });

  tearDown(() {
    while (setupLinks.take() != null) {}
    //X
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(setupLinks.channel, null);
  });

  Future<Harness> mac(WidgetTester tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace());
    await harness.show(tester);
    return harness;
  }

  // The note: its words (the title's are elsewhere on the screen too).
  final note = find.textContaining('by a setup run on it');

  List<Object?> states() => [for (final status in said) status['state']];

  testWidgets('a fresh Mac connects, says the digits and its key, and once '
      'connected tells the person', (tester) async {
    final harness = await mac(tester);
    setupLinks.receive(_code);
    await settle(tester);
    expect(harness.connections.current?.code, _code);
    expect(said.last, {'state': 'connecting', 'name': 'box'});
    expect(note, findsNothing);

    // The Claktory has it wait to be allowed.
    harness.backend.pairing.value = PairingWait(
      PairingPending(deviceId: 'd7', name: 'Mac'),
      '4821',
    );
    await settle(tester);
    expect(said.last['state'], 'waiting');
    expect(said.last['device_id'], 'd7');
    expect(said.last['number'], '4821');
    expect(said.last['endpoint_id'], isA<String>());

    // Allowed: welcomed.
    harness.backend.pairing.value = null;
    harness.backend.connectedSince.value = DateTime.now();
    await settle(tester);
    expect(said.last, {'state': 'connected', 'name': 'box'});
    expect(find.text('Connected to box'), findsWidgets);
    expect(
      find.text(
        "This Mac was just connected to box by a setup run on it. If you "
        "didn't start one, remove this Mac in box's Settings → Devices.",
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('OK'));
    await settle(tester);
    expect(note, findsNothing);

    // Said once: nothing more comes of it.
    final count = said.length;
    harness.backend.connectedSince.value = DateTime.now();
    await settle(tester);
    expect(said.length, count);
    expect(note, findsNothing);
  });

  // As the app shows them (main.dart): each Claktory in a window of its
  // own, with its own backend, so connecting opens another window.
  testWidgets('it follows the Claktory connecting into its own window', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace());
    final target = parsePairingCode(_code)!;
    final box = RecordingBackend(localBackend: false)..connectTo(target);
    final shown = ValueNotifier<RecordingBackend>(harness.backend);
    harness.connections.addListener(() {
      if (harness.connections.current == target) shown.value = box;
    });
    await harness.show(
      tester,
      home: ValueListenableBuilder(
        valueListenable: shown,
        builder: (context, backend, _) {
          final scope = AppScope.scope(context);
          return AppScope(
            backend: backend,
            login: scope.login,
            page: scope.page,
            appBuild: scope.appBuild,
            drafts: scope.drafts,
            history: scope.history,
            connections: scope.connections,
            child: SetupLinkOpener(
              key: ObjectKey(backend),
              deviceEndpoint: (key, target) async => 'e1',
              child: const SizedBox.expand(),
            ),
          );
        },
      ),
    );
    setupLinks.receive(_code);
    await settle(tester);
    expect(shown.value, box);
    expect(said.last, {'state': 'connecting', 'name': 'box'});

    box.pairing.value = PairingWait(
      PairingPending(deviceId: 'd7', name: 'Mac'),
      '4821',
    );
    await settle(tester);
    expect(said.last, {
      'state': 'waiting',
      'name': 'box',
      'device_id': 'd7',
      'number': '4821',
      'endpoint_id': 'e1',
    });

    // The code ran out before it was allowed: the window says so, and so
    // does the status.
    box.pairing.value = null;
    box.refused.value = 'The code expired before this Mac used it.';
    await settle(tester);
    expect(said.last, {
      'state': 'failed',
      'reason': 'refused',
      'message': 'The code expired before this Mac used it.',
    });
    expect(states(), ['connecting', 'waiting', 'failed']);
    //Y
  });

  testWidgets('a claktory:// code is taken as a factory:// one', (
    tester,
  ) async {
    final harness = await mac(tester);
    final code = _code.replaceFirst('factory://', 'claktory://');
    setupLinks.receive(code);
    await settle(tester);
    expect(harness.connections.current?.code, code);
    expect(said.last, {'state': 'connecting', 'name': 'box'});
    harness.backend.connectedSince.value = DateTime.now();
    await settle(tester);
    expect(said.last, {'state': 'connected', 'name': 'box'});
    await tester.tap(find.text('OK'));
    await settle(tester);
  });

  testWidgets('a Mac set up already takes none', (tester) async {
    final harness = await mac(tester);
    final other = parsePairingCode(
      'factory://?t=${'b' * 64}&name=home&e=${'d' * 64}',
    );
    harness.connections.use(other);
    setupLinks.receive(_code);
    await settle(tester);
    expect(said.single, {'state': 'failed', 'reason': 'already_connected'});
    expect(harness.connections.current, other);
    expect(harness.connections.saved, [other]);
  });

  testWidgets("a code that isn't one is refused", (tester) async {
    final harness = await mac(tester);
    setupLinks.receive('nonsense');
    await settle(tester);
    expect(said.single, {'state': 'failed', 'reason': 'bad_code'});
    expect(harness.connections.saved, isEmpty);
  });

  testWidgets('turned away, it says why', (tester) async {
    final harness = await mac(tester);
    setupLinks.receive(_code);
    await settle(tester);
    harness.backend.refused.value = 'This device was removed.';
    await settle(tester);
    expect(said.last, {
      'state': 'failed',
      'reason': 'refused',
      'message': 'This device was removed.',
    });
    expect(states(), ['connecting', 'failed']);
    expect(note, findsNothing);
  });

  testWidgets('the person picking another meanwhile calls it off', (
    tester,
  ) async {
    final harness = await mac(tester);
    setupLinks.receive(_code);
    await settle(tester);
    harness.connections.use(null);
    await settle(tester);
    expect(said.last, {'state': 'failed', 'reason': 'cancelled'});
  });

  testWidgets("a welcome from before it started isn't its", (tester) async {
    final harness = await mac(tester);
    harness.backend.connectedSince.value = DateTime(2020);
    setupLinks.receive(_code);
    await settle(tester);
    expect(states(), ['connecting']);
    expect(note, findsNothing);
  });
}
