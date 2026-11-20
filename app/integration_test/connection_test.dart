// The connection's status on an iPhone (the Simulator) already connected
// to a real Claktory: "Connected" at the top of the agent list, how it
// connects on a tap, and Reconnect connecting afresh.
//
//   cd app && flutter test integration_test/connection_test.dart \
//     -d <simulator> --dart-define=FACTORY_DEV=true
import 'package:factory_app/connection.dart';
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'phone_test.dart' show hold, waitFor;

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('connected, how, and Reconnect', (tester) async {
    await app.main();
    await waitFor(
      tester,
      find.text('Connected'),
      timeout: const Duration(minutes: 1),
    );
    final backend = AppScope.scope(
      tester.element(find.byType(Sidebar).first),
    ).backend;
    final since = backend.connectedSince.value;
    expect(since, isNotNull);
    // ignore: avoid_print
    print('path: ${backend.tunnel.value?.path}');
    await hold(tester, seconds: 2);
    await tester.tap(find.text('Connected').first);
    await waitFor(tester, find.byType(ConnectionDialog));
    final details = Connection(backend).details;
    // ignore: avoid_print
    print('details: $details');
    expect(find.text(details), findsOneWidget);
    await hold(tester, seconds: 3);
    await tester.tap(find.text('Reconnect'));
    await waitFor(tester, find.byType(ConnectionDialog), gone: true);
    // Connected afresh: a new welcome.
    await waitFor(
      tester,
      find.text('Connected'),
      timeout: const Duration(seconds: 30),
    );
    final end = DateTime.now().add(const Duration(seconds: 30));
    while (backend.connectedSince.value == since ||
        backend.connectedSince.value == null) {
      if (DateTime.now().isAfter(end)) fail('never connected afresh');
      await tester.pump(const Duration(milliseconds: 200));
    }
    expect(backend.connected.value, isTrue);
    await hold(tester, seconds: 2);
  });
}
