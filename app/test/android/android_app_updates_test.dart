// Manifest/channel/policy and framework-screen evidence; no APK installation,
// release download bytes, native PackageManager or physical-device acceptance.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:factory_app/android/android_app_updates.dart';
import 'package:factory_app/compatibility.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'support/channel_fakes.dart';

Map<String, Object?> _manifest() => {
  'version': 'v0.1.19',
  'version_code': 101900,
  'min_server': 'v0.1.15',
  'page': 'https://claktory.com/android',
  'apk': 'https://claktory.com/media/example/claktory-0.1.19-101900.apk',
  'size': 89755471,
  'sha256': 'a' * 64,
  'package': 'com.claktory.app',
  'cert_sha256': 'b' * 64,
};

AndroidAppUpdate _release({int code = 101900}) =>
    parseAndroidAppUpdate(jsonEncode(_manifest()..['version_code'] = code))!;

class _Headers implements HttpHeaders {
  _Headers({this.contentType});
  @override
  ContentType? contentType;
  final values = <String, Object>{};
  @override
  void set(String name, Object value, {bool preserveHeaderCase = false}) {
    values[name] = value;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _Response extends Stream<List<int>> implements HttpClientResponse {
  _Response(
    this.chunks, {
    this.statusCode = 200,
    int? length,
    String type = 'application/json',
  }) : contentLength =
           length ?? chunks.fold<int>(0, (n, chunk) => n + chunk.length),
       headers = _Headers(contentType: ContentType.parse(type));
  final List<List<int>> chunks;
  @override
  final int statusCode;
  @override
  final int contentLength;
  @override
  final HttpHeaders headers;
  var listened = false;
  @override
  StreamSubscription<List<int>> listen(
    void Function(List<int>)? onData, {
    Function? onError,
    void Function()? onDone,
    bool? cancelOnError,
  }) {
    listened = true;
    return Stream.fromIterable(chunks).listen(
      onData,
      onError: onError,
      onDone: onDone,
      cancelOnError: cancelOnError,
    );
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _Request implements HttpClientRequest {
  _Request(this.response);
  final _Response response;
  @override
  bool followRedirects = true;
  @override
  final _Headers headers = _Headers();
  @override
  Future<HttpClientResponse> close() async => response;
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _Client implements HttpClient {
  _Client(_Response response, {this.error}) : request = _Request(response);
  final _Request request;
  final Exception? error;
  Uri? requested;
  bool? forcedClose;
  @override
  Duration? connectionTimeout;
  @override
  Future<HttpClientRequest> getUrl(Uri url) async {
    requested = url;
    if (error case final error?) throw error;
    return request;
  }

  @override
  void close({bool force = false}) {
    forcedClose = force;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('public release manifest', () {
    test('reads all receipt fields and ignores additive fields', () {
      final update = parseAndroidAppUpdate(
        jsonEncode(_manifest()..['future_field'] = {'enabled': true}),
      )!;
      expect(update.version, 'v0.1.19');
      expect(update.versionCode, 101900);
      expect(update.minServer, 'v0.1.15');
      expect(update.page, androidAppInstallPage);
      expect(update.apk.path, '/media/example/claktory-0.1.19-101900.apk');
      expect(update.size, 89755471);
      expect(update.sha256, 'a' * 64);
      expect(update.certSha256, 'b' * 64);
      expect(
        androidAppUpdateManifest.toString(),
        'https://claktory.com/android/latest.json',
      );
    });

    for (final field in _manifest().keys) {
      test('missing $field is unknown', () {
        expect(
          parseAndroidAppUpdate(jsonEncode(_manifest()..remove(field))),
          isNull,
        );
      });
    }

    for (final body in <String?>[
      null,
      '',
      '{',
      'null',
      '[]',
      '42',
      '"v0.1.19"',
    ]) {
      test('invalid manifest body ${jsonEncode(body)} is unknown', () {
        expect(parseAndroidAppUpdate(body), isNull);
      });
    }

    final invalid = <(String, Object?)>[
      ('version', 'dev'),
      ('version', 'v0.1.19-preview'),
      ('version', 'v0.01.19'),
      ('version', 101900),
      ('min_server', 'yesterday'),
      ('min_server', null),
      ('version_code', '101900'),
      ('version_code', 101900.5),
      ('version_code', 0),
      ('version_code', -1),
      ('version_code', 2100000001),
      ('size', 0),
      ('size', -1),
      ('size', '89755471'),
      ('sha256', 'not-a-digest'),
      ('cert_sha256', 'b' * 63),
      ('package', 'com.claktory.app.dev'),
      ('page', 'http://claktory.com/android'),
      ('page', 'https://elsewhere.invalid/android'),
      ('page', 'https://claktory.com.evil.invalid/android'),
      ('page', 'https://user@claktory.com/android'),
      ('page', 'https://claktory.com:444/android'),
      ('page', 'https://claktory.com/account/delete'),
      ('page', 'https://claktory.com/android?token=fixture'),
      ('page', 'https://claktory.com/android#fragment'),
      ('page', 'javascript:alert(1)'),
      ('apk', 'http://claktory.com/media/app.apk'),
      ('apk', 'https://elsewhere.invalid/app.apk'),
      ('apk', 'https://claktory.com/media/not-an-apk'),
      ('apk', 'file:///fixture/app.apk'),
    ];
    for (var i = 0; i < invalid.length; i++) {
      final (field, value) = invalid[i];
      test('rejects malformed field $field #$i', () {
        expect(
          parseAndroidAppUpdate(jsonEncode(_manifest()..[field] = value)),
          isNull,
        );
      });
    }
  });

  group('HTTP manifest contract', () {
    test(
      'one bounded JSON request to fixed endpoint, no redirect, closes client',
      () async {
        final body = jsonEncode(_manifest());
        final client = _Client(_Response([utf8.encode(body)]));
        expect(await loadAndroidAppUpdateManifest(client: () => client), body);
        expect(client.requested, androidAppUpdateManifest);
        expect(client.connectionTimeout, const Duration(seconds: 10));
        expect(client.request.followRedirects, isFalse);
        expect(
          client.request.headers.values[HttpHeaders.acceptHeader],
          'application/json',
        );
        expect(client.forcedClose, isTrue);
      },
    );

    for (final status in [302, 403, 404, 500]) {
      test(
        'HTTP $status is unknown, not a cached offer or redirected download',
        () async {
          final response = _Response([
            utf8.encode(jsonEncode(_manifest())),
          ], statusCode: status);
          final client = _Client(response);
          expect(
            await loadAndroidAppUpdateManifest(client: () => client),
            isNull,
          );
          expect(response.listened, isFalse);
          expect(client.forcedClose, isTrue);
        },
      );
    }

    test('wrong MIME type, declared oversize, chunked oversize, invalid UTF8 are unknown', () async {
      for (final response in [
        _Response([utf8.encode(jsonEncode(_manifest()))], type: 'text/html'),
        _Response([], length: 65537),
        _Response([List.filled(40000, 32), List.filled(40000, 32)], length: -1),
        _Response([
          [0xff],
        ]),
      ]) {
        final client = _Client(response);
        expect(
          await loadAndroidAppUpdateManifest(client: () => client),
          isNull,
        );
        expect(client.forcedClose, isTrue);
      }
    });

    test('socket/timeout failures are unknown and close the client', () async {
      for (final error in <Exception>[
        const SocketException('fixture'),
        TimeoutException('fixture'),
      ]) {
        final client = _Client(_Response([]), error: error);
        expect(
          await loadAndroidAppUpdateManifest(client: () => client),
          isNull,
        );
        expect(client.forcedClose, isTrue);
      }
    });
  });

  group('availability policy', () {
    test('newer installed code, not merely a different display version', () {
      final latest = _release();
      expect(
        androidAppUpdate(
          latest: latest,
          installedVersionCode: 101800,
          backendVersion: 'v0.1.15',
        ),
        same(latest),
      );
      for (final code in <int?>[null, 0, -1, 101900, 101901, 2100000001]) {
        expect(
          androidAppUpdate(
            latest: latest,
            installedVersionCode: code,
            backendVersion: 'v0.1.19',
          ),
          isNull,
        );
      }
      // A rebuild has the same display version but a newer actual code.
      expect(
        androidAppUpdate(
          latest: _release(code: 101901),
          installedVersionCode: 101900,
          backendVersion: 'v0.1.19',
        )?.versionCode,
        101901,
      );
    });

    test('unknown release never becomes an offer', () {
      expect(
        androidAppUpdate(
          latest: null,
          installedVersionCode: 101800,
          backendVersion: 'v0.1.19',
        ),
        isNull,
      );
    });

    test('floor and compatibility line both apply', () {
      final latest = _release();
      for (final backend in [
        'v0.1.14',
        'v0.0.99',
        'v0.2.0',
        'v1.1.19',
        'dev',
        '',
      ]) {
        expect(latest.supportsBackend(backend), isFalse, reason: backend);
        expect(
          androidAppUpdate(
            latest: latest,
            installedVersionCode: 101800,
            backendVersion: backend,
          ),
          isNull,
          reason: backend,
        );
      }
      expect(latest.supportsBackend('v0.1.15'), isTrue);
      expect(latest.supportsBackend('v0.1.100'), isTrue);
    });
  });

  group('installed native code', () {
    test(
      'factory/device.appVersionCode has no args; rereads every time',
      () async {
        final script = ChannelScript('factory/device', const [
          ChannelReply('appVersionCode', result: 101900),
          ChannelReply('appVersionCode', result: 101901),
        ]);
        addTearDown(script.dispose);
        expect(await installedAndroidAppVersionCode(), 101900);
        expect(await installedAndroidAppVersionCode(), 101901);
        script.assertDone();
      },
    );

    for (final answer in <Object?>[null, '101900', true, 0, -1, 2100000001]) {
      test('bad native answer $answer is unknown', () async {
        final script = ChannelScript('factory/device', [
          ChannelReply('appVersionCode', result: answer),
        ]);
        addTearDown(script.dispose);
        expect(await installedAndroidAppVersionCode(), isNull);
        script.assertDone();
      });
    }

    test('PackageManager failure is unknown', () async {
      final script = ChannelScript('factory/device', [
        ChannelReply(
          'appVersionCode',
          error: PlatformException(code: 'unavailable'),
        ),
      ]);
      addTearDown(script.dispose);
      expect(await installedAndroidAppVersionCode(), isNull);
      script.assertDone();
    });

    test('older native host is unknown', () async {
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('factory/device'),
            null,
          );
      expect(await installedAndroidAppVersionCode(), isNull);
    });
  });

  group('transient refresh', () {
    test('dev/debug default does no network or native work', () async {
      var calls = 0;
      final updates = AndroidAppUpdates(
        loadManifest: () async {
          calls++;
          return jsonEncode(_manifest());
        },
        readVersionCode: () async {
          calls++;
          return 101800;
        },
      );
      addTearDown(updates.dispose);
      expect(updates.enabled, isFalse);
      await updates.refresh();
      expect(calls, 0);
      expect(updates.latest, isNull);
      expect(updates.installedVersionCode, isNull);
    });

    test(
      'failed/404/bad refresh removes an old offer, not guessed data',
      () async {
        String? body = jsonEncode(_manifest());
        final updates = AndroidAppUpdates(
          enabled: true,
          loadManifest: () async => body,
          readVersionCode: () async => 101800,
        );
        addTearDown(updates.dispose);
        await updates.refresh();
        expect(updates.latest?.versionCode, 101900);
        // The HTTP adapter maps 404/non-200 to null.
        body = null;
        await updates.refresh();
        expect(updates.latest, isNull);
        body = '{';
        await updates.refresh();
        expect(updates.latest, isNull);
      },
    );

    test('network and native exceptions stay unknown', () async {
      final updates = AndroidAppUpdates(
        enabled: true,
        loadManifest: () async => throw TimeoutException('fixture'),
        readVersionCode: () async =>
            throw PlatformException(code: 'unavailable'),
      );
      addTearDown(updates.dispose);
      await updates.refresh();
      expect(updates.latest, isNull);
      expect(updates.installedVersionCode, isNull);
    });

    test(
      'latest refresh wins a race; dispose ignores pending answers',
      () async {
        final first = Completer<String?>();
        final second = Completer<String?>();
        final third = Completer<String?>();
        final replies = [first, second, third];
        var index = 0;
        final updates = AndroidAppUpdates(
          enabled: true,
          loadManifest: () => replies[index++].future,
          readVersionCode: () async => 101800,
        );
        var notifications = 0;
        updates.addListener(() => notifications++);
        final a = updates.refresh();
        final b = updates.refresh();
        second.complete(jsonEncode(_manifest()..['version_code'] = 101902));
        await b;
        first.complete(jsonEncode(_manifest()));
        await a;
        expect(updates.latest?.versionCode, 101902);
        expect(notifications, 1);
        final c = updates.refresh();
        updates.dispose();
        third.complete(jsonEncode(_manifest()));
        await c;
        expect(notifications, 1);
      },
    );
  });

  group('external install page', () {
    test('lockout fallback works without metadata, never opens APK', () async {
      final opened = <Uri>[];
      expect(
        await openAndroidAppUpdatePage(
          open: (page) async {
            opened.add(page);
            return true;
          },
        ),
        isTrue,
      );
      expect(opened, [androidAppInstallPage]);
      expect(
        await openAndroidAppUpdatePage(
          page: _release().apk,
          open: (page) async {
            opened.add(page);
            return true;
          },
        ),
        isFalse,
      );
      expect(opened, hasLength(1));
    });

    test(
      'unsafe target never launches; unavailable browser returns false',
      () async {
        var calls = 0;
        expect(
          await openAndroidAppUpdatePage(
            page: Uri.parse('https://elsewhere.invalid/android'),
            open: (_) async {
              calls++;
              return true;
            },
          ),
          isFalse,
        );
        expect(calls, 0);
        expect(
          await openAndroidAppUpdatePage(open: (_) async => false),
          isFalse,
        );
        expect(
          await openAndroidAppUpdatePage(
            open: (_) async => throw PlatformException(code: 'unavailable'),
          ),
          isFalse,
        );
      },
    );
  });
  _compatibilityTests();
}

Harness _mismatched() => Harness(
  Workspace(
    settings: Settings(userName: 'QA', name: 'QA Claktory'),
    version: 'v0.2.0',
  ),
)..appVersion = 'v0.1.19';

void _compatibilityTests() {
  group('Android too-old app download page', () {
    setUp(() {
      PhonePlatformPolicy.debugAndroidPolicy = true;
      addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
    });

    for (final scale in [1.0, 3.12]) {
      testWidgets(
        '320x568 at ${scale}x reaches Download without a release list',
        (tester) async {
          tester.platformDispatcher.textScaleFactorTestValue = scale;
          addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
          final opened = <Uri>[];
          final harness = _mismatched();
          await harness.show(
            tester,
            size: const Size(320, 568),
            home: VersionStep(
              which: Mismatch.appOlder,
              openAndroidPage: (page) async {
                opened.add(page);
                return true;
              },
            ),
          );
          await settle(tester);
          expect(
            find.text('This app is too old for this Claktory'),
            findsOneWidget,
          );
          expect(
            find.text('Update this app, then open it again.'),
            findsOneWidget,
          );
          expect(harness.backend.workspace.workspace.latestRelease, isEmpty);
          expect(
            MediaQuery.textScalerOf(
              tester.element(
                find.text('This app is too old for this Claktory'),
              ),
            ).scale(16),
            closeTo(16 * scale, 0.01),
          );
          await tester.ensureVisible(find.text('Download'));
          await settle(tester);
          final button = find.widgetWithText(Button, 'Download');
          expect(tester.getSize(button).height, greaterThanOrEqualTo(48));
          expect(find.text('Download').hitTestable(), findsOneWidget);
          expect(tester.takeException(), isNull);
          await tester.tap(find.text('Download'));
          await settle(tester);
          expect(opened, [androidAppInstallPage]);
          expect(
            harness.backend.sent.where((m) => m.hasInstallUpdate()),
            isEmpty,
          );
          expect(harness.quits, 0);
          expect(tester.takeException(), isNull);
        },
      );
    }

    testWidgets('real mismatched app surface contains the Android action', (
      tester,
    ) async {
      final harness = _mismatched();
      await harness.show(tester, size: const Size(320, 568));
      await settle(tester);
      expect(find.byType(VersionStep), findsOneWidget);
      expect(
        find.text('This app is too old for this Claktory'),
        findsOneWidget,
      );
      await tester.ensureVisible(find.text('Download'));
      await settle(tester);
      expect(find.text('Download').hitTestable(), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets(
      'browser failure is honest and Download retries the same page',
      (tester) async {
        final opened = <Uri>[];
        await _mismatched().show(
          tester,
          size: const Size(320, 568),
          home: VersionStep(
            which: Mismatch.appOlder,
            openAndroidPage: (page) async {
              opened.add(page);
              return opened.length > 1;
            },
          ),
        );
        await settle(tester);
        await tester.ensureVisible(find.text('Download'));
        await tester.tap(find.text('Download'));
        await settle(tester);
        expect(
          find.text("Couldn't open the download page. Try again."),
          findsOneWidget,
        );
        expect(find.text('Download'), findsOneWidget);
        await tester.ensureVisible(find.text('Download'));
        await tester.tap(find.text('Download'));
        await settle(tester);
        expect(opened, [androidAppInstallPage, androidAppInstallPage]);
        expect(
          find.text("Couldn't open the download page. Try again."),
          findsNothing,
        );
        expect(tester.takeException(), isNull);
      },
    );

    testWidgets(
      'opening disables duplicate actions and disposal ignores completion',
      (tester) async {
        final answer = Completer<bool>();
        var calls = 0;
        await _mismatched().show(
          tester,
          size: const Size(320, 568),
          home: VersionStep(
            which: Mismatch.appOlder,
            openAndroidPage: (_) {
              calls++;
              return answer.future;
            },
          ),
        );
        await settle(tester);
        await tester.ensureVisible(find.text('Download'));
        await tester.tap(find.text('Download'));
        await tester.pump();
        expect(find.text('Opening…'), findsOneWidget);
        expect(
          tester
              .widget<Button>(find.widgetWithText(Button, 'Opening…'))
              .onPressed,
          isNull,
        );
        expect(calls, 1);
        await tester.pumpWidget(const SizedBox());
        answer.complete(false);
        await tester.pump();
        expect(tester.takeException(), isNull);
      },
    );

    testWidgets('backend-too-old never offers an app download as its fix', (
      tester,
    ) async {
      await _mismatched().show(
        tester,
        size: const Size(320, 568),
        home: const VersionStep(which: Mismatch.backendOlder),
      );
      await settle(tester);
      expect(find.text('Update Claktory to use this app'), findsOneWidget);
      expect(find.text('Download'), findsNothing);
    });

    testWidgets(
      'other platforms keep their existing recovery instructions',
      (tester) async {
        await _mismatched().show(
          tester,
          home: const VersionStep(which: Mismatch.appOlder),
        );
        await settle(tester);
        expect(find.text('Download'), findsNothing);
        if (defaultTargetPlatform == TargetPlatform.iOS) {
          expect(
            find.text(
              'Update Claktory from the App Store or TestFlight, then open it again.',
            ),
            findsOneWidget,
          );
        }
        expect(tester.takeException(), isNull);
      },
      variant: TargetPlatformVariant({
        TargetPlatform.iOS,
        TargetPlatform.macOS,
      }),
    );
  });
}
