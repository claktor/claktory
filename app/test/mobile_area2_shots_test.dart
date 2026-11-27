// Area 2 on the iPhone (Lucy's devices-signin.md §4, §8), drawn as it
// looks for her to check, through the app itself (FactoryApp: its dark
// theme, no debug banner, the drawer and the Claktories' screens), on an
// SE and a Pro Max at the default and the largest text:
// - 8.2: adding another Claktory starts at the email sign-in;
// - 8.4: the problem pages (no longer allowed, then Use a new code; sign
//   in to connect, signed out or in; the Claktory signed out; another
//   account), primary first and the ways out as links under it;
// - 8.5: This iPhone's first row, this iPhone's own sign-in.
// With SHOTS set, each is drawn to a PNG there:
//   SHOTS=/tmp/area2-phone flutter test test/mobile_area2_shots_test.dart
// Without it, each still lays out without an overflow or an exception.
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/account.dart' show DeviceAccountRow;
import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/main.dart';
import 'package:factory_app/mobile/drawer.dart';
import 'package:factory_app/phone.dart' show PairScreen;
import 'package:factory_app/rail.dart' show DeviceDrafts, PhoneClaktories;
import 'package:factory_app/scope.dart' show AppPage, AppScope;
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'account_test.dart' show signedIn, withAccount;
import 'device_confirm_test.dart' as confirm show FakeAccountApi, signedIn;
import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

Future<void> _shot(WidgetTester tester, String name) async {
  expect(tester.takeException(), isNull, reason: name);
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

final _nixos = 'factory://?t=${'a' * 64}&name=nixos&e=${'c' * 64}';

/// The app on a phone, one Claktory (nixos) saved, this phone's account
/// [account]. [welcomed]: whether nixos let it in (its workspace here),
/// which one that turns it away at connect never did.
class _Phone {
  _Phone(DeviceAccount account, {this.welcomed = true})
    : connections = Connections(
        MemoryConnectionStorage(),
        null,
        null,
        null,
        account,
      );

  final Connections connections;
  final bool welcomed;
  final drafts = DeviceDrafts(MemoryDraftStorage());
  final backends = <String, RecordingBackend>{};

  RecordingBackend get nixos => backends[_nixos]!;

  Backend open(RemoteFactory? target) {
    final key = target?.code ?? '';
    final backend = RecordingBackend(localBackend: false)..connectTo(target);
    if (welcomed) {
      backend.workspace.apply(
        withAccount(signedIn)..ensureSettings().userName = 'Matt',
      );
    } else {
      // Turned away at connect: never connected, nothing received.
      backend.connected.value = false;
    }
    return backends[key] = backend;
  }

  Future<void> show(WidgetTester tester, Size size, double scale) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    WidgetsApp.debugAllowBannerOverride = false;
    addTearDown(() => WidgetsApp.debugAllowBannerOverride = true);
    tester.platformDispatcher.platformBrightnessTestValue = Brightness.dark;
    addTearDown(tester.platformDispatcher.clearPlatformBrightnessTestValue);
    tester.platformDispatcher.textScaleFactorTestValue = scale;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    tester.view.physicalSize = size;
    tester.view.devicePixelRatio = 1;
    tester.view.display.size = size;
    tester.view.display.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    addTearDown(tester.view.display.reset);
    connections.use(parsePairingCode(_nixos));
    // No camera here, so Add a Claktory's code is pasted.
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      const MethodChannel('factory/device'),
      (call) async => call.method == 'cameraAccess' ? 'unavailable' : null,
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        const MethodChannel('factory/device'),
        null,
      ),
    );
    await tester.pumpWidget(
      FactoryApp(
        drafts: drafts,
        connections: connections,
        open: open,
        localBackend: false,
      ),
    );
    await pumpFrames(tester);
  }
}

DeviceAccount _signedIn() => confirm.signedIn(confirm.FakeAccountApi());

void main() {
  setUpAll(loadGeist);

  for (final (phone, size) in [
    ('se', const Size(320, 568)),
    ('promax', const Size(430, 932)),
  ]) {
    for (final (text, scale) in [('1x', 1.0), ('ax5', 3.12)]) {
      final at = '$phone-$text';

      testWidgets('8.2 adding another Claktory, $at', (tester) async {
        final app = _Phone(DeviceAccount());
        await app.show(tester, size, scale);
        // As the drawer's Add a Claktory opens it.
        PhoneClaktories.showAdding();
        await pumpFrames(tester);
        expect(find.byType(PairScreen), findsOneWidget);
        await _shot(tester, '82-$at-email');
        await tester.ensureVisible(find.text('Use a code instead'));
        await pumpFrames(tester);
        await tester.tap(find.text('Use a code instead'));
        await pumpFrames(tester);
        await _shot(tester, '82-$at-code');
      });

      testWidgets('8.4 no longer allowed, then Use a new code, $at', (
        tester,
      ) async {
        final app = _Phone(DeviceAccount());
        await app.show(tester, size, scale);
        app.nixos
          ..connected.value = false
          ..refused.value = 'unknown device';
        await pumpFrames(tester);
        await _shot(tester, '84-$at-not-allowed');
        await tester.ensureVisible(find.text('Use a new code'));
        await pumpFrames(tester);
        await tester.tap(find.text('Use a new code'));
        await pumpFrames(tester);
        expect(find.byType(PairScreen), findsOneWidget);
        await _shot(tester, '84-$at-new-code');
      });

      for (final (name, account) in [
        ('signin-out', DeviceAccount.new),
        ('signin-in', _signedIn),
      ]) {
        testWidgets('8.4 sign in to connect, $name, $at', (tester) async {
          final app = _Phone(account(), welcomed: false);
          await app.show(tester, size, scale);
          app.nixos.signInRequired.value =
              'Confirm this device is yours: sign in.';
          await pumpFrames(tester);
          await _shot(tester, '84-$at-$name');
        });
      }

      testWidgets('8.4 the Claktory signed out, $at', (tester) async {
        final app = _Phone(_signedIn(), welcomed: false);
        await app.show(tester, size, scale);
        app.nixos.signInRequired.value =
            'This Claktory was signed out of your Claktory account, so it '
            "can't let this device in.";
        await pumpFrames(tester);
        await _shot(tester, '84-$at-signed-out');
      });

      testWidgets('8.4 another account, $at', (tester) async {
        final app = _Phone(_signedIn(), welcomed: false);
        await app.show(tester, size, scale);
        app.nixos.otherAccount.value =
            "So it can't let this iPhone in. To work with someone else's "
            'agents, open a pipe with them.';
        await pumpFrames(tester);
        await _shot(tester, '84-$at-other');
      });

      for (final (name, account) in [
        ('signed-in', _signedIn),
        ('signed-out', DeviceAccount.new),
      ]) {
        testWidgets('8.5 This iPhone, $name, $at', (tester) async {
          final app = _Phone(account());
          await app.show(tester, size, scale);
          AppScope.scope(tester.element(find.byType(PhoneDrawerHost)))
                  .page
                  .value =
              AppPage.settings;
          await pumpFrames(tester);
          // The sections' list builds as it scrolls.
          await tester.scrollUntilVisible(
            find.text('This iPhone'),
            200,
            scrollable: find.byType(Scrollable).first,
          );
          await pumpFrames(tester);
          await tester.tap(find.text('This iPhone').first);
          await pumpFrames(tester);
          expect(find.byType(DeviceAccountRow), findsOneWidget);
          await _shot(tester, '85-$at-$name');
        });
      }
    }
  }
}
