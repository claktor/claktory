import 'package:factory_app/connection.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/controls.dart' show LiveDot, Pressable;
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/setup.dart' show LocalEngine, localEngine;
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/tunnel/api/tunnel.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
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

  test("this Mac's own, turned off or blocked by macOS, says so while it "
      "doesn't answer; another machine's never does", () {
    addTearDown(() => localEngine.value = LocalEngine.running);
    final harness = Harness(Workspace());
    final backend = harness.backend..connected.value = false;
    localEngine.value = LocalEngine.off;
    var connection = Connection(backend);
    expect(connection.link, Link.off);
    expect(connection.line, 'Turned off on this Mac');
    expect(connection.short, 'Turned off');
    expect(connection.problem, isFalse);
    expect(connection.details, contains("agents aren't running"));
    expect(reconnectFirst(connection.link), isFalse);

    localEngine.value = LocalEngine.blocked;
    connection = Connection(backend);
    expect(connection.link, Link.blocked);
    expect(connection.line, 'Blocked by macOS');
    expect(connection.problem, isTrue);
    expect(connection.details, contains('Allow in the Background'));

    // Answering after all: live, whatever was last found.
    backend.connected.value = true;
    expect(Connection(backend).link, Link.live);

    backend
      ..connectTo(parsePairingCode(_code))
      ..connected.value = false;
    expect(Connection(backend).link, Link.connecting);
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
    // Claktories are switched in the rail, never here (devices-signin.md
    // §3).
    expect(find.text('Switch Claktory…'), findsNothing);
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

  testWidgets("the line is a whole touch target on Android, and the Mac's "
      "and iPhone's as they were", (tester) async {
    Future<Size> lineOn(TargetPlatform platform, {bool android = false}) async {
      debugDefaultTargetPlatformOverride = platform;
      PhonePlatformPolicy.debugAndroidPolicy = android;
      try {
        final harness = Harness(Workspace());
        await harness.show(
          tester,
          // As in the sidebar's foot: a column's line.
          home: Align(
            alignment: Alignment.topLeft,
            child: SizedBox(
              width: 240,
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [ConnectionLine(backend: harness.backend)],
              ),
            ),
          ),
        );
        await settle(tester);
        final pressable = find.descendant(
          of: find.byType(ConnectionLine),
          matching: find.byType(Pressable),
        );
        // What a screen reader touches is the same size.
        expect(
          tester
              .getSemantics(find.bySemanticsLabel('Running on this Mac'))
              .rect
              .size,
          tester.getSize(pressable),
        );
        return tester.getSize(pressable);
      } finally {
        debugDefaultTargetPlatformOverride = null;
        PhonePlatformPolicy.debugAndroidPolicy = false;
      }
    }

    // A line of caption text, as before.
    expect(await lineOn(TargetPlatform.macOS), const Size(240, 17));
    expect(await lineOn(TargetPlatform.iOS), const Size(240, 17));
    expect(
      await lineOn(TargetPlatform.android, android: true),
      const Size(240, 48),
    );
  });
}
