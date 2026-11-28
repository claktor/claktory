// An update this Mac's Claktory couldn't start went back to a version with
// no socket for this app: once the socket has been gone a while and
// update-failed.json names such a version, the app offers that version's
// own app, and nothing else.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/older_app.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'network_test.dart' show until;
import 'tunnel_test.dart' show FakeTunnel, FakeTunnels;

String _failed(String version, String backTo) => jsonEncode({
  'version': version,
  'back_to': backTo,
  'reason': 'It exited (exit status: 1).',
});

void main() {
  group('the offer', () {
    OlderApp? offer(String json, {String app = 'v0.1.28', bool dev = false}) =>
        olderAppOffer(json, appVersion: app, dev: dev);

    test('an update to this version that went back to an older one', () {
      final made = offer(_failed('v0.1.28', 'v0.1.27'))!;
      expect('${made.failed}', 'v0.1.28');
      expect('${made.backTo}', 'v0.1.27');
      // This app's release came without a new backend: the update was to
      // the one before.
      expect(offer(_failed('v0.1.28', 'v0.1.27'), app: 'v0.1.29'), isNotNull);
      expect(offer(_failed('v0.1.28', 'v0.1.9')), isNotNull);
    });

    test('only versions exactly: nothing that could name another folder', () {
      for (final backTo in [
        'v0.1.27/../../Downloads',
        '../v0.1.27',
        'v0.1.27/',
        ' v0.1.27',
        '0.1.27',
        'v0.1',
        '',
        '2026.11.19-1958',
        'v0.1.27-dev.20261127-2244.09',
      ]) {
        expect(offer(_failed('v0.1.28', backTo)), isNull, reason: backTo);
      }
    });

    test('never back to this version or a newer one', () {
      expect(offer(_failed('v0.1.28', 'v0.1.28')), isNull);
      expect(offer(_failed('v0.1.28', 'v0.1.29')), isNull);
      expect(offer(_failed('v0.1.30', 'v0.1.27')), isNull, reason: 'newer');
    });

    test('a file from an update to a version without the socket is stale', () {
      expect(offer(_failed('v0.1.27', 'v0.1.26')), isNull);
      // It names no update at all: the backend kept stopping.
      expect(offer(_failed('', '')), isNull);
    });

    test('nothing for a file it can\'t read, or an app without a version', () {
      expect(offer('not json'), isNull);
      expect(offer('[]'), isNull);
      expect(offer(jsonEncode({'version': 'v0.1.28'})), isNull);
      expect(offer(jsonEncode({'version': 2, 'back_to': 'v0.1.27'})), isNull);
      expect(offer(_failed('v0.1.28', 'v0.1.27'), app: 'dev'), isNull);
    });

    test("Claktory Dev's versions only in Claktory Dev", () {
      final failed = _failed(
        'v0.1.28-dev.20261128-0900.00',
        'v0.1.28-dev.20261127-2244.09',
      );
      const app = 'v0.1.28-dev.20261128-0901.30';
      expect(offer(failed, app: app), isNull);
      final made = offer(failed, app: app, dev: true)!;
      expect('${made.backTo}', 'v0.1.28-dev.20261127-2244.09');
      expect(
        offer(failed, app: 'v0.1.28-dev.20261127-2244.09', dev: true),
        isNull,
        reason: 'not back to this very version',
      );
    });
  });

  group('the watch', () {
    late Directory home;
    late String socket;
    setUp(() async {
      home = await Directory.systemTemp.createTemp('older-app');
      socket = '${home.path}/app.sock';
      await File('${home.path}/update-failed.json')
          .writeAsString(_failed('v0.1.28', 'v0.1.27'));
    });
    tearDown(() => home.delete(recursive: true));

    OlderAppWatch watch() => OlderAppWatch(
      appVersion: 'v0.1.28',
      dev: false,
      home: () async => home.path,
    );

    test('a socket missing for 30 seconds, not a backend restarting', () async {
      final watching = watch();
      final start = DateTime(2026, 11, 28, 9);
      expect(await watching.failed(socket, now: start), isNull);
      expect(
        await watching.failed(socket, now: start.add(olderAppAfter * 0.9)),
        isNull,
      );
      final made = await watching.failed(socket, now: start.add(olderAppAfter));
      expect('${made?.backTo}', 'v0.1.27');

      // Back for a moment, then gone again: counted afresh.
      File(socket).createSync();
      expect(await watching.failed(socket, now: start), isNull);
      File(socket).deleteSync();
      final later = start.add(const Duration(minutes: 5));
      expect(await watching.failed(socket, now: later), isNull);
      expect(
        await watching.failed(socket, now: later.add(olderAppAfter)),
        isNotNull,
      );
      watching.connected();
      expect(
        await watching.failed(socket, now: later.add(olderAppAfter)),
        isNull,
        reason: 'connected since',
      );
    });

    test('a socket that is there is never gone', () async {
      File(socket).createSync();
      final watching = watch();
      final start = DateTime(2026, 11, 28, 9);
      await watching.failed(socket, now: start);
      expect(
        await watching.failed(socket, now: start.add(olderAppAfter * 2)),
        isNull,
      );
    });

    test('no file, no offer', () async {
      File('${home.path}/update-failed.json').deleteSync();
      final watching = watch();
      final start = DateTime(2026, 11, 28, 9);
      await watching.failed(socket, now: start);
      expect(
        await watching.failed(socket, now: start.add(olderAppAfter)),
        isNull,
      );
    });
  });

  test("the backend offers it while its socket is gone, and doesn't once "
      'it connects', () async {
    final home = await Directory.systemTemp.createTemp('older-app');
    addTearDown(() => home.delete(recursive: true));
    final path = '${home.path}/app.sock';
    await File('${home.path}/update-failed.json')
        .writeAsString(_failed('v0.1.28', 'v0.1.27'));
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      tunnels: FakeTunnels(FakeTunnel(0)),
      localSocket: path,
      localPeerCheck: (_) async => null,
      olderApps: OlderAppWatch(
        appVersion: 'v0.1.28',
        dev: false,
        after: Duration.zero,
        home: () async => home.path,
      ),
    );
    unawaited(backend.run());
    await until(() => backend.olderApp.value != null, seconds: 10);
    expect('${backend.olderApp.value?.backTo}', 'v0.1.27');

    // A backend with the socket after all: connected, nothing offered.
    final server = await HttpServer.bind(
      InternetAddress(path, type: InternetAddressType.unix),
      0,
    );
    addTearDown(() => server.close(force: true));
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (message.hasHello()) {
          socket.add(
            ServerMessage(
              replyTo: message.id,
              welcome: Welcome(serverVersion: 'test'),
            ).writeToBuffer(),
          );
        }
      });
    });
    backend.reconnectNow();
    await until(() => backend.connected.value, seconds: 30);
    expect(backend.olderApp.value, isNull);
    await backend.close();
  });

  group('the page', () {
    final calls = <MethodCall>[];
    PlatformException? failure;
    setUp(() {
      calls.clear();
      failure = null;
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(const MethodChannel('factory/device'), (
            call,
          ) async {
            calls.add(call);
            if (failure case final failure?) throw failure;
            return null;
          });
    });
    tearDown(() {
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('factory/device'),
            null,
          );
    });

    Future<Harness> shown(WidgetTester tester) async {
      final harness = Harness(Workspace());
      harness.backend.connected.value = false;
      harness.backend.olderApp.value = olderAppOffer(
        _failed('v0.1.28', 'v0.1.27'),
        appVersion: 'v0.1.28',
        dev: false,
      );
      await harness.show(tester);
      await tester.pump();
      return harness;
    }

    testWidgets("says what happened, and only its button swaps the app", (
      tester,
    ) async {
      await shown(tester);
      expect(find.text("Claktory v0.1.28 didn't start"), findsOneWidget);
      expect(
        find.textContaining('went back to Claktory v0.1.27'),
        findsOneWidget,
      );
      expect(calls.where((call) => call.method == 'useOlderApp'), isEmpty);

      await tester.tap(find.text('Open Claktory v0.1.27'));
      await tester.pump();
      final used = calls.singleWhere((call) => call.method == 'useOlderApp');
      final arguments = used.arguments as Map<Object?, Object?>;
      expect(arguments['version'], 'v0.1.27');
      expect(arguments['home'], isA<String>());
    });

    testWidgets('a refusal says why, and it can be tried again', (
      tester,
    ) async {
      failure = PlatformException(
        code: 'failed',
        message: "it isn't Claktory's app, signed by its developer (-67050)",
      );
      await shown(tester);
      await tester.tap(find.text('Open Claktory v0.1.27'));
      await tester.pump();
      await tester.pump();
      expect(
        find.text(
          "Couldn't open Claktory v0.1.27: it isn't Claktory's app, signed by "
          'its developer (-67050). Install Claktory again from claktory.com.',
        ),
        findsOneWidget,
      );
      expect(find.text('Open Claktory v0.1.27'), findsOneWidget);
    });

    testWidgets('connected, the page goes', (tester) async {
      final harness = await shown(tester);
      harness.backend.connected.value = true;
      await tester.pump();
      expect(find.text("Claktory v0.1.28 didn't start"), findsNothing);
    });
  });
}
