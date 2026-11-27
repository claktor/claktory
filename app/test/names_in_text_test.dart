// Names in Claktory's own sentences (Abbacchio's row 1730, Fugo 50896): a
// name with a right-to-left override can't turn the words after it
// backwards; what reorders shows by its code, and a name that isn't plain
// ASCII sits in a first-strong isolate.
import 'package:factory_app/claktory_identity.dart';
import 'package:factory_app/connection.dart' show Connection;
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/tunnel/api/tunnel.dart';
import 'package:factory_app/visible_codes.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'tunnel_test.dart' show tunnelStatus;

const _evil = 'Evil\u202EMac\u200B of someone';

void main() {
  test('a plain name stays as it is; anything else is isolated, what '
      'reorders by its code', () {
    expect(nameInText('Studio'), 'Studio');
    expect(nameInText('nixos-site 2'), 'nixos-site 2');
    final evil = nameInText(_evil);
    expect(evil, startsWith('\u2068'));
    expect(evil, endsWith('\u2069'));
    expect(evil, isNot(contains('\u202E')));
    expect(evil, contains(r'\u{202E}'));
    // Persian needs its joiners: they stay.
    expect(nameInText('می\u200Cخواهم'), contains('\u200C'));
    // An isolate in the name itself shows as its code, so it can't close
    // anything.
    expect(nameInText('a\u2069b'), r'a\u{2069}b');
    expect(nameInText('two\nlines'), 'two lines');
  });

  testWidgets("the rail's sentence about the connection reads forwards with "
      'such a name', (tester) async {
    final harness = Harness(Workspace(settings: Settings(name: _evil)));
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=mac-mini&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(390, 844));
    await pumpFrames(tester);
    final backend = harness.backend
      ..workspace.apply(Workspace(settings: Settings(name: _evil)))
      ..connected.value = true
      ..connectedSince.value = DateTime(2026, 11, 27, 6, 10)
      ..tunnel.value = tunnelStatus(path: TunnelPath.direct);
    await pumpFrames(tester);
    expect(claktoryName(backend), isNot(contains('\u202E')));
    final details = Connection(backend).details;
    expect(details, startsWith('Connected directly to \u2068Evil'));
    expect(details, endsWith('\u2069 since 06:10. Private and encrypted.'));
    expect(details, isNot(contains('\u202E')));
  });
}
