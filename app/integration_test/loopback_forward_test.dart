// Localhost forwarding against a real Claktory on another machine, from an
// iPhone (the Simulator, which shares this Mac's network). The Claktory
// must not be on this Mac, since both ends would want the same ports: a
// Linux one in a container does (see below).
//
// On the Claktory's machine, something answers on 127.0.0.1:8976 with
// "forwarded" (busybox httpd -p 127.0.0.1:8976 -h DIR, DIR/hello.txt), and
// nothing listens on 8977. The test checks that:
// - a page coming back to localhost:8976 reaches it, over 127.0.0.1 and ::1;
// - nothing on 8977 there says so (the Claktory's sentence);
// - a port this device uses already is refused as taken;
// - a sign-in's own address (Anthropic's, port 53692) brings the browser
//   back to the Claktory: a made-up code there ends the flow with an error
//   from the Claktory, not from this device;
// - stopping closes the ports.
//
//   cd app && flutter test integration_test/loopback_forward_test.dart \
//     -d <simulator> --dart-define=PAIRING_CODE='factory://…'
//
// A container for it, with a Linux build (scripts/build-linux.sh aarch64 OUT):
//   podman run -d --name claktory-fwd -v OUTDIR:/fwd alpine sh -c \
//     'mkdir -p /www && echo forwarded > /www/hello.txt &&
//      busybox httpd -p 127.0.0.1:8976 -h /www && exec /fwd/factoryd'
//   podman exec claktory-fwd /fwd/factoryd pair Test   (the code; leave it
//   waiting, or allow the device later with factoryd allow)
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/loopback_forward.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/tunnel_client.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _code = String.fromEnvironment('PAIRING_CODE');

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('pages come back to the Claktory through this device', (
    tester,
  ) async {
    final target = parsePairingCode(_code);
    expect(target, isNotNull, reason: 'pass --dart-define=PAIRING_CODE=…');
    final key = newDeviceKey();
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
    addTearDown(backend.close);
    final forwards = backend.forwards;

    Future<void> until(
      String what,
      bool Function() done, {
      Duration within = const Duration(seconds: 30),
    }) async {
      final end = DateTime.now().add(within);
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 200)),
        );
        await tester.pump();
      }
    }

    Future<String> get(String url) async {
      final client = HttpClient()
        ..connectionTimeout = const Duration(seconds: 10);
      try {
        final request = await client.getUrl(Uri.parse(url));
        final response = await request.close().timeout(
          const Duration(seconds: 20),
        );
        return '${response.statusCode} '
            '${(await utf8.decodeStream(response)).trim()}';
      } finally {
        client.close(force: true);
      }
    }

    // Allowed on the Claktory's machine meanwhile (factoryd allow).
    await until(
      'connected',
      () => backend.connected.value,
      within: const Duration(minutes: 3),
    );
    final machine = backend.workspace.workspace.remote.machine;

    // A page that comes back to 8976, where the Claktory's machine answers.
    final flow = await tester.runAsync(
      () => forwards.start(
        Uri.parse(
          'https://dash.test/oauth?redirect_uri='
          '${Uri.encodeComponent('http://localhost:8976/cb')}',
        ),
      ),
    );
    expect(flow, isNotNull);
    expect(flow!.problems, isEmpty);
    expect(
      await tester.runAsync(() => get('http://127.0.0.1:8976/hello.txt')),
      '200 forwarded',
    );
    expect(
      await tester.runAsync(() => get('http://[::1]:8976/hello.txt')),
      '200 forwarded',
    );
    await until('counted', () => forwards.served(flow) >= 2);

    // Nothing on 8977 there.
    final nothing = (await tester.runAsync(
      () => forwards.start(
        Uri.parse('https://a.test/?callback=http://127.0.0.1:8977/'),
      ),
    ))!;
    expect(nothing.problems, isEmpty);
    await tester.runAsync(() async {
      try {
        await get('http://127.0.0.1:8977/');
      } on Object catch (_) {
        // Closed: nothing answered there.
      }
    });
    await until(
      "the Claktory's sentence",
      () => forwards.lastError(nothing).isNotEmpty,
    );
    // ignore: avoid_print
    print('nothing on 8977: ${forwards.lastError(nothing)}');
    expect(forwards.lastError(nothing), contains('8977'));

    // A port this device uses already.
    final mine = (await tester.runAsync(
      () => ServerSocket.bind(InternetAddress.loopbackIPv4, 0),
    ))!;
    final taken = (await tester.runAsync(
      () => forwards.start(
        Uri.parse('https://a.test/?redirect=http://localhost:${mine.port}/'),
      ),
    ))!;
    expect(taken.problems, {mine.port: ForwardProblem.taken});
    await tester.runAsync(mine.close);

    // A sign-in: Anthropic's page comes back to the Claktory's port 53692.
    await tester.runAsync(
      () => backend.startLogin(
        StartLogin(providerId: 'anthropic', methodId: 'oauth'),
      ),
    );
    await until(
      'the sign-in page',
      () =>
          backend.workspace.workspace.hasLogin() &&
          backend.workspace.workspace.login.url.isNotEmpty,
    );
    final page = Uri.parse(backend.workspace.workspace.login.url);
    expect(loopbackPorts(page), [53692]);
    final signIn = (await tester.runAsync(() => forwards.start(page)))!;
    expect(signIn.problems, isEmpty);
    final state = page.queryParameters['state'] ?? '';
    final back = await tester.runAsync(
      () => get('http://localhost:53692/callback?code=made-up&state=$state'),
    );
    // ignore: avoid_print
    print('the Claktory answered the sign-in: $back');
    await until(
      'the made-up code refused',
      () =>
          !backend.workspace.workspace.hasLogin() ||
          backend.workspace.workspace.login.error.isNotEmpty,
    );
    // ignore: avoid_print
    print('then: ${backend.workspace.workspace.login.error}');
    expect(forwards.served(signIn), greaterThanOrEqualTo(1));
    if (backend.workspace.workspace.hasLogin()) {
      await tester.runAsync(backend.cancelLogin);
    }

    // Stopped: the ports close.
    await tester.runAsync(forwards.stopAll);
    expect(forwards.active, isEmpty);
    final refused = await tester.runAsync(() async {
      try {
        final socket = await Socket.connect(
          InternetAddress.loopbackIPv4,
          8976,
          timeout: const Duration(seconds: 2),
        );
        socket.destroy();
        return false;
      } on SocketException {
        return true;
      }
    });
    expect(refused, isTrue, reason: '8976 closed on $machine\'s side here');
  });
}
