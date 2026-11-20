// A Claktory's name and color against a real Claktory, on an iPhone (the
// Simulator): it says them on connecting, a change shows on every device
// connected to it (two connections here), and empty goes back to the
// defaults. What it had is put back at the end.
//
//   cd app && flutter test integration_test/claktory_identity_test.dart \
//     -d <simulator> --dart-define=PAIRING_CODE='factory://…'
import 'dart:async';

import 'package:factory_app/backend.dart';
import 'package:factory_app/claktory_identity.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/tunnel_client.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _code = String.fromEnvironment('PAIRING_CODE');

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('a name and color for every device', (tester) async {
    final target = parsePairingCode(_code);
    expect(target, isNotNull, reason: 'pass --dart-define=PAIRING_CODE=…');
    final key = newDeviceKey();
    Backend connect() {
      final backend =
          Backend(
              workspace: WorkspaceStore(),
              transcript: TranscriptStore(),
              explorer: ExplorerStore(),
              extensionView: ExtensionViewStore(),
              localBackend: false,
              localPairing: () async => null,
            )
            ..deviceKey = ((_) => key)
            ..connectTo(target);
      unawaited(backend.run());
      return backend;
    }

    Future<void> until(String what, bool Function() done) async {
      final end = DateTime.now().add(const Duration(seconds: 30));
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await tester.pump(const Duration(milliseconds: 100));
      }
    }

    final one = connect();
    final other = connect();
    await until(
      'connected',
      () =>
          one.connected.value &&
          other.connected.value &&
          one.workspace.workspace.agents.isNotEmpty,
    );
    expect(one.welcomeName, isNotEmpty, reason: 'said on connecting');
    expect(agentColorsHave(one.welcomeColor), isTrue);
    final settings = one.workspace.workspace.settings;
    final machine = one.workspace.workspace.remote.machine;
    expect(settings.name, isNotEmpty);
    expect(agentColorsHave(settings.color), isTrue);

    await one.updateSettings(UpdateSettings(name: 'Studio', color: 'amber'));
    await until(
      'the other device sees it',
      () =>
          claktoryName(other) == 'Studio' &&
          claktoryColorName(other) == 'amber',
    );
    Object? refused;
    try {
      await one.updateSettings(UpdateSettings(color: 'mauve'));
    } on RequestFailure catch (error) {
      refused = error;
    }
    expect(refused, isNotNull, reason: 'a color off the palette');

    await one.updateSettings(UpdateSettings(name: '', color: ''));
    await until('the defaults back', () => claktoryName(other) != 'Studio');
    await tester.pump(const Duration(milliseconds: 500));
    if (machine.isNotEmpty) expect(claktoryName(other), machine);

    // Back as it was: set only where it wasn't the default.
    final name = claktoryName(other), color = claktoryColorName(other);
    await one.updateSettings(
      UpdateSettings(
        name: settings.name == name ? '' : settings.name,
        color: settings.color == color ? '' : settings.color,
      ),
    );
    // ignore: avoid_print
    print('name and color went to both devices and back');
    await one.close();
    await other.close();
  });
}

bool agentColorsHave(String name) => const {
  'blue',
  'violet',
  'green',
  'pink',
  'teal',
  'amber',
  'red',
  'slate',
}.contains(name);
