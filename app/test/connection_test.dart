import 'package:factory_app/connection.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/controls.dart' show LiveDot;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/tunnel/api/tunnel.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'tunnel_test.dart' show tunnelStatus;

final _code =
    'factory://?t=${'a' * 64}&name=mac-mini&e=${'c' * 64}'
    '&r=https://relay-eu.example.com';

void main() {
  test("this Mac's own Claktory, and one on another machine", () {
    final harness = Harness(Workspace());
    final backend = harness.backend;
    backend.connectedSince.value = DateTime(2026, 11, 20, 14, 2);
    var connection = Connection(backend);
    expect(connection.link, Link.live);
    expect(connection.line, 'Running on this Mac');
    expect(connection.short, 'Connected');
    expect(connection.details, startsWith('Claktory is running on this Mac.'));

    backend.connectTo(parsePairingCode(_code));
    backend.connected.value = true;
    backend.connectedSince.value = DateTime.now();
    backend.tunnel.value = tunnelStatus(path: TunnelPath.direct);
    connection = Connection(backend);
    expect(connection.line, 'Connected to mac-mini');
    expect(connection.details, startsWith('Connected directly to mac-mini'));
    expect(connection.details, contains('Private and encrypted.'));

    backend.tunnel.value = tunnelStatus(path: TunnelPath.relayed);
    expect(
      Connection(backend).details,
      contains('by an indirect route'),
      reason: 'relayed: it may be slower',
    );
    // The name it has on that machine, when it's known.
    expect(
      Connection(backend, machine: 'Mac Studio').line,
      'Connected to Mac Studio',
    );
  });

  test('connecting, reconnecting, failing and turned away', () {
    final harness = Harness(Workspace());
    final backend = harness.backend
      ..connectTo(parsePairingCode(_code))
      ..connected.value = false;
    backend.tunnel.value = tunnelStatus(phase: TunnelPhase.dialing);
    expect(Connection(backend).link, Link.connecting);
    expect(Connection(backend).line, 'Connecting to mac-mini…');
    expect(Connection(backend).details, 'Trying to reach mac-mini…');

    backend.reconnecting.value = true;
    expect(Connection(backend).line, 'Reconnecting…');
    expect(Connection(backend).short, 'Reconnecting…');

    // A failure while it reconnects quietly says nothing yet.
    backend.tunnel.value = tunnelStatus(
      phase: TunnelPhase.failed,
      path: TunnelPath.none,
      failure: TunnelFailure.unreachable,
    );
    expect(Connection(backend).link, Link.connecting);
    backend.reconnecting.value = false;
    final failing = Connection(backend);
    expect(failing.link, Link.failing);
    expect(failing.line, "Can't connect to mac-mini");
    expect(failing.short, "Can't connect");
    expect(failing.details, contains('Make sure mac-mini is on'));
    expect(failing.problem, isTrue);

    backend.refused.value = 'unknown device';
    expect(Connection(backend).line, "mac-mini doesn't allow this device");
    expect(Connection(backend).short, 'Not allowed');
  });

  testWidgets('the footer always says how it connects; a click shows more, '
      'with Reconnect', (tester) async {
    final harness = Harness(Workspace());
    await harness.show(tester);
    await settle(tester);
    final line = find.descendant(
      of: find.byType(Sidebar),
      matching: find.text('Running on this Mac'),
    );
    expect(line, findsOneWidget);
    await tester.tap(line);
    await settle(tester);
    expect(find.byType(ConnectionDialog), findsOneWidget);
    expect(find.text('Switch Claktory…'), findsOneWidget);
    await tester.tap(find.text('Reconnect'));
    await settle(tester);
    expect(find.byType(ConnectionDialog), findsNothing);

    // Dropped: it says so at once, with the pulsing mark.
    harness.backend.connected.value = false;
    harness.backend.reconnecting.value = true;
    await pumpFrames(tester);
    expect(find.text('Reconnecting…'), findsWidgets);
    expect(
      find.descendant(
        of: find.byType(ConnectionLine),
        matching: find.byType(LiveDot),
      ),
      findsOneWidget,
    );
    // Reconnect's quiet spell runs out.
    await tester.pump(const Duration(seconds: 9));
  });
}
