// This device's messages engine starts as soon as the Claktory the
// app shows offers People (its `dm` extension), not only once People opens,
// and never where it doesn't.

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/people/e2ee.dart';
import 'package:factory_app/people/start_messages.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'dm_fakes.dart';
import 'harness.dart';

Workspace _workspace({required bool dms}) => Workspace(
  extensions: [
    Extension(id: 'memory', name: 'Memory'),
    if (dms) Extension(id: 'dm', name: 'Direct messages'),
  ],
);

void main() {
  tearDown(() => e2eeFor = null);

  testWidgets('started once the Claktory shown offers People, never '
      'before', (tester) async {
    final endpoint = FakeEndpoint(FakeWorker(), 'matt', 'phone');
    var asked = 0;
    e2eeFor = (_) {
      asked++;
      return endpoint.e2ee;
    };
    final harness = Harness(_workspace(dms: false));
    await harness.show(tester, home: const StartMessages());
    await tester.pump();
    expect(asked, 0);
    harness.backend.workspace.apply(_workspace(dms: true));
    await tester.pump();
    expect(asked, greaterThan(0));
  });

  // Which Claktory the engine checks against the account is
  // the rail's entry whose connection it links, never another's; one
  // paired with a direct code (no tunnel) can't be confirmed.
  testWidgets("each connection is its own rail entry's", (tester) async {
    await tester.pumpWidget(const SizedBox());
    final connections = Connections(MemoryConnectionStorage());
    final backends = <String, Backend>{};
    final claktories = Claktories(
      connections: connections,
      localBackend: true,
      drafts: DeviceDrafts(MemoryDraftStorage()),
      open: (target) =>
          backends[target?.code ?? ''] = RecordingBackend()..connectTo(target),
    );
    addTearDown(claktories.dispose);
    final endpoint = 'c' * 64;
    final tunnelled = 'factory://?t=${'a' * 64}&name=nixos&e=$endpoint';
    final direct =
        'factory://192.168.1.20:7420?t=${'b' * 64}&fp=${'d' * 64}&name=old';
    connections
      ..use(parsePairingCode(tunnelled))
      ..use(parsePairingCode(direct));

    late BuildContext context;
    await tester.pumpWidget(
      ClaktoriesScope(
        claktories: claktories,
        child: Builder(
          builder: (each) {
            context = each;
            return const SizedBox();
          },
        ),
      ),
    );
    expect(dmShownOf(context, backends['']!), const DmShown.thisMac());
    expect(
      dmShownOf(context, backends[tunnelled]!),
      DmShown.remote(code: tunnelled, endpoint: endpoint),
    );
    final old = dmShownOf(context, backends[direct]!);
    expect(old, DmShown.remote(code: direct, endpoint: ''));
    expect(old.confirmable, isFalse);
    // A connection no entry has, and a window with no rail: never this
    // Mac's own.
    expect(dmShownOf(context, RecordingBackend()), const DmShown.unknown());
    await tester.pumpWidget(
      Builder(
        builder: (each) {
          context = each;
          return const SizedBox();
        },
      ),
    );
    expect(dmShownOf(context, backends['']!), const DmShown.unknown());
  });
}
