import 'package:factory_app/connections.dart';
import 'package:factory_app/devices.dart';
import 'package:factory_app/links.dart';
import 'package:factory_app/identity.dart' show Palette;
import 'package:factory_app/mobile/drawer.dart'
    show PhoneDrawerHost, PhoneDrawerHostState;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _code = 'factory://?t=${'a' * 64}&name=box&e=${'c' * 64}';

/// claktory://add?code=… adds the Claktory the code is for, once the
/// person says so.
void main() {
  // The person's account lists this Claktory unless a test says not.
  setUp(() => onAccountTest = (_, _) async => true);
  tearDown(() {
    onAccountTest = null;
    while (appLinks.take() != null) {}
  });

  String link(String code) =>
      'claktory://add?code=${Uri.encodeComponent(code)}';

  test('a link to add a Claktory holds its code, encoded or not', () {
    expect(parseLink(Uri.parse(link(_code)))!.target!.name, 'box');
    // Pasted in as it is, the code runs on into the link's own query.
    expect(
      parseLink(Uri.parse('claktory://add?code=$_code'))!.target!.name,
      'box',
    );
    expect(parseLink(Uri.parse('claktory-dev://add?code=$_code')), isNotNull);
    expect(
      parseLink(Uri.parse('claktory://add?code=nonsense'))!.target,
      isNull,
    );
    // Not a link of this app's.
    expect(parseLink(Uri.parse('claktory://open?code=$_code')), isNull);
    expect(
      parseLink(Uri.parse('https://claktory.com/add?code=$_code')),
      isNull,
    );
  });

  testWidgets('a link asks before connecting, and Connect connects', (
    tester,
  ) async {
    // A Mac's words ("this Mac"); tests run as Android otherwise.
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace());
    await harness.show(tester);
    appLinks.receive(link(_code));
    await settle(tester);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    // The code's fingerprint first; its name only quoted, its own words.
    expect(
      find.text(
        'Check that it matches what it showed there: cccc cccc cccc cccc',
        findRichText: true,
      ),
      findsOneWidget,
    );
    // On this device's account, it lets this Mac in with no Allow
    // (Abbacchio's row 1667).
    expect(find.text('It calls itself “box”.'), findsOneWidget);
    expect(find.textContaining('still need to allow'), findsNothing);
    expect(find.textContaining("isn't on your Claktory account"), findsNothing);
    // Nothing yet.
    expect(harness.connections.current, isNull);
    await tester.tap(find.text('Connect'));
    await settle(tester);
    expect(harness.connections.current?.code, _code);
    expect(find.text('Connect to this Claktory?'), findsNothing);
  });

  testWidgets('a pairing code itself, as the Camera opens it from a QR '
      'code, asks the same; off the account it warns', (tester) async {
    onAccountTest = (_, _) async => false;
    final harness = Harness(Workspace());
    await harness.show(tester);
    appLinks.receive(_code);
    await settle(tester);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(
      find.text(
        "This Claktory isn't on your Claktory account. Connect only if "
        "you're setting it up yourself, right now.",
      ),
      findsOneWidget,
    );
    // Off the account, it still has to be allowed there.
    expect(find.textContaining('still need to allow'), findsOneWidget);
    expect(harness.connections.current, isNull);
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(harness.connections.saved, isEmpty);
  });

  testWidgets('signed out, or the list unreadable, it claims nothing of '
      'whose it is: only the caution, not in red (#529, MC3)', (tester) async {
    onAccountTest = (_, _) async => null;
    final harness = Harness(Workspace());
    await harness.show(tester);
    appLinks.receive(_code);
    await settle(tester);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(find.textContaining("isn't on your Claktory account"), findsNothing);
    final caution = find.text(
      "Connect only if you're setting it up yourself, right now.",
    );
    expect(caution, findsOneWidget);
    expect(
      tester.widget<Text>(caution).style?.color,
      Palette.of(tester.element(caution)).textMuted,
    );
    await tester.tap(find.text('Cancel'));
    await settle(tester);
  });

  group('claktory:// pairing codes, read like factory:// ones', () {
    final token = 'a' * 64;
    final endpoint = 'c' * 64;
    final shapes = {
      'tunnel': '://?t=$token&name=box&e=$endpoint',
      'direct': '://100.64.0.2:7443?t=$token&fp=${'b' * 64}&name=box',
    };

    test('pasted or scanned (one parser): both schemes, every shape', () {
      for (final MapEntry(key: shape, value: rest) in shapes.entries) {
        for (final scheme in ['factory', 'claktory']) {
          final target = parsePairingCode('$scheme$rest');
          expect(target, isNotNull, reason: '$scheme $shape');
          expect(target!.name, 'box');
          expect(target.code, '$scheme$rest', reason: 'kept as it was');
        }
      }
      final direct = parsePairingCode('claktory${shapes['direct']}')!;
      expect((direct.host, direct.port), ('100.64.0.2', 7443));
      expect(
        parsePairingCode('claktory${shapes['tunnel']}')!.endpointId,
        endpoint,
      );
      // A Claktory found through the account, as kept on this device.
      for (final scheme in ['factory', 'claktory']) {
        final account = '$scheme://?account=mini&e=$endpoint&name=mini';
        expect(parsePairingCode(account, fromAccount: true), isNotNull);
        expect(parsePairingCode(account), isNull, reason: 'never pasted');
      }
      // Other schemes never are.
      expect(parsePairingCode('claktory-dev${shapes['tunnel']}'), isNull);
      expect(parsePairingCode('https${shapes['tunnel']}'), isNull);
    });

    test('a link: the code itself, or inside claktory://add', () {
      for (final scheme in ['factory', 'claktory']) {
        for (final rest in shapes.values) {
          final code = '$scheme$rest';
          expect(parseLink(Uri.parse(code))!.target!.name, 'box', reason: code);
          expect(
            parseLink(Uri.parse(link(code)))!.target!.name,
            'box',
            reason: 'add $code',
          );
        }
      }
      // claktory://add is still the add link, never read as a code.
      expect(parseLink(Uri.parse('claktory://add?code=x'))!.target, isNull);
    });

    testWidgets('opened from the Camera, it asks the same', (tester) async {
      final harness = Harness(Workspace());
      await harness.show(tester);
      appLinks.receive('claktory${shapes['tunnel']}');
      await settle(tester);
      expect(find.text('Connect to this Claktory?'), findsOneWidget);
      await tester.tap(find.text('Cancel'));
      await settle(tester);
      expect(harness.connections.saved, isEmpty);
    });
  });

  // #352 P6: a QR code may hold the pairing page's link, the code's query
  // after its `#`, so a phone's camera opens Claktory (Fugo's rules).
  group('the pairing page link', () {
    final query = 't=${'a' * 64}&name=box&e=${'c' * 64}&r=https%3A%2F%2Frelay';
    final page = 'https://claktory.com/pair#$query';

    test('read as the factory:// code its fragment holds, and only so', () {
      final target = parsePairingCode(page)!;
      expect(target.code, 'factory://?$query', reason: 'kept as the code');
      expect(target.name, 'box');
      expect(target.endpointId, 'c' * 64);
      expect(target.relays, ['https://relay']);
      expect(parseLink(Uri.parse(page))!.target!.code, 'factory://?$query');
      // Pasted from a messenger, among words.
      expect(parsePairingCode('Scan or tap: $page.')!.name, 'box');
      // Validated as the code is: a bad token isn't one.
      expect(parsePairingCode(page.replaceFirst('a' * 64, 'a' * 63)), isNull);
      // Never the account's, even asked as kept codes are.
      expect(
        parsePairingCode(
          'https://claktory.com/pair#account=mini&e=${'c' * 64}',
          fromAccount: true,
        ),
        isNull,
      );
      // Exactly that page: no other host, path, scheme, port, user or
      // query, and the code after the `#`, never in the query.
      for (final other in [
        'http://claktory.com/pair#$query',
        'https://claktory.com.evil.test/pair#$query',
        'https://evil.test/pair#$query',
        'https://www.claktory.com/pair#$query',
        'https://claktory.com/pair/#$query',
        'https://claktory.com/pairing#$query',
        'https://claktory.com:8443/pair#$query',
        'https://x@claktory.com/pair#$query',
        'https://claktory.com/pair?$query',
        'https://claktory.com/pair?x=1#$query',
        'https://claktory.com/pair#',
        'https://claktory.com/pair',
      ]) {
        expect(parsePairingCode(other), isNull, reason: other);
        expect(parseLink(Uri.parse(other))?.target, isNull, reason: other);
      }
    });

    testWidgets('opened as a Universal Link, it asks the same', (tester) async {
      final harness = Harness(Workspace());
      await harness.show(tester);
      appLinks.receive(page);
      await settle(tester);
      expect(find.text('Connect to this Claktory?'), findsOneWidget);
      await tester.tap(find.text('Connect'));
      await settle(tester);
      expect(harness.connections.current?.code, 'factory://?$query');
    });
  });

  testWidgets('a name never stands as the title', (tester) async {
    final harness = Harness(Workspace());
    await harness.show(tester);
    appLinks.receive(link(_code.replaceFirst('name=box', 'name=Your%20Mac')));
    await settle(tester);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(find.textContaining('Connect to Your Mac'), findsNothing);
    expect(find.textContaining('“Your Mac”'), findsOneWidget);
  });

  testWidgets('the QR code shows the same code to compare', (tester) async {
    await Harness(Workspace()).show(
      tester,
      home: SizedBox(
        width: 400,
        child: PairingInstructions(
          code: _code,
          device: 'iPhone',
          way: PairingWay.scan,
          qrSize: 120,
        ),
      ),
    );
    await settle(tester);
    expect(
      find.text('Its code: cccc cccc cccc cccc', findRichText: true),
      findsOneWidget,
    );
  });

  test('a link too long is refused before it is read', () {
    appLinks.receive('factory://?t=${'a' * 64}&e=${'c' * 64}&x=${'y' * 5000}');
    expect(appLinks.take(), isNull);
    appLinks.receive(_code);
    expect(appLinks.take(), isNotNull);
  });

  for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
    test(
      '${platform.name} listens once: what waited, then what comes',
      () async {
        debugDefaultTargetPlatformOverride = platform;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        TestWidgetsFlutterBinding.ensureInitialized();
        const channel = MethodChannel('factory/links');
        final messenger =
            TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
        var listens = 0;
        messenger.setMockMethodCallHandler(channel, (call) async {
          if (call.method == 'listen') {
            listens++;
            return [_code];
          }
          return null;
        });
        addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
        final links = AppLinks();
        await links.listen();
        expect(listens, 1);
        await messenger.handlePlatformMessage(
          'factory/links',
          const StandardMethodCodec().encodeMethodCall(
            MethodCall('opened', 'claktory://add?code=x'),
          ),
          (_) {},
        );
        await messenger.handlePlatformMessage(
          'factory/links',
          const StandardMethodCodec().encodeMethodCall(
            MethodCall('opened', 42),
          ),
          (_) {},
        );
        expect(links.take().toString(), _code);
        expect(links.take().toString(), 'claktory://add?code=x');
        expect(links.take(), isNull);
      },
    );
  }

  testWidgets('Cancel leaves everything as it was', (tester) async {
    final harness = Harness(Workspace());
    await harness.show(tester);
    appLinks.receive(link(_code));
    await settle(tester);
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(harness.connections.current, isNull);
    expect(harness.connections.saved, isEmpty);
  });

  testWidgets('a Claktory added already shows at once', (tester) async {
    final harness = Harness(Workspace());
    harness.connections.use(parsePairingCode(_code));
    harness.connections.use(null);
    await harness.show(tester);
    appLinks.receive(link(_code));
    await settle(tester);
    expect(find.text('Connect to this Claktory?'), findsNothing);
    expect(harness.connections.current?.code, _code);
  });

  testWidgets("a link without a code says it can't be used", (tester) async {
    final harness = Harness(Workspace());
    await harness.show(tester);
    appLinks.receive('claktory://add?code=nonsense');
    await settle(tester);
    expect(find.text("Can't use this link"), findsOneWidget);
    // Where a Mac pastes a code.
    expect(
      find.text(
        'This link has no Claktory code in it. Paste the code in Connect '
        'to another Claktory instead.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('OK'));
    await settle(tester);
    expect(find.text("Can't use this link"), findsNothing);
    expect(harness.connections.saved, isEmpty);
  });

  group('on a phone', () {
    Future<void> phone(WidgetTester tester, Harness harness) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
      addTearDown(tester.view.resetPadding);
      await harness.show(tester, size: const Size(393, 852));
      await pumpFrames(tester);
    }

    testWidgets("a link without a code names the phone's screen", (
      tester,
    ) async {
      await phone(tester, Harness(Workspace()));
      appLinks.receive('claktory://add?code=nonsense');
      await settle(tester);
      expect(
        find.text(
          'This link has no Claktory code in it. Paste the code in Connect '
          'to your Claktory instead.',
        ),
        findsOneWidget,
      );
    });

    testWidgets('a link closes the menu, so its question is in front', (
      tester,
    ) async {
      await phone(tester, Harness(Workspace()));
      await tester.tap(find.bySemanticsLabel('Menu').last);
      await pumpFrames(tester, const Duration(seconds: 1));
      final host = tester.state<PhoneDrawerHostState>(
        find.byType(PhoneDrawerHost),
      );
      expect(host.drawerOpen, isTrue);
      appLinks.receive(link(_code));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(host.drawerOpen, isFalse);
      final ask = find.text('Connect to this Claktory?');
      expect(ask, findsOneWidget);
      // In front: a tap on it reaches it, not the page under it.
      expect(ask.hitTestable(), findsOneWidget);
    });
  });
}
