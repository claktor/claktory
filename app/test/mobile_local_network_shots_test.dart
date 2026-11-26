// Local Network on an iPhone or iPad (mobile/local_network.dart), drawn:
// with SHOTS set, PNGs of why Claktory asks and of what declining means,
// over the pairing screen, on an iPhone SE, a Pro Max and an iPad, at the
// default text size and a large one:
//   SHOTS=/tmp/local-network flutter test test/mobile_local_network_shots_test.dart
// test-bundle: alone (loads the app's fonts)
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/mobile/local_network.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'phone_sizes_test.dart' show loadGeist;

Future<void> _shot(WidgetTester tester, String name) async {
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

class _Answers implements LocalNetwork {
  @override
  Future<LocalNetworkAccess> check() async => LocalNetworkAccess.denied;

  @override
  Future<void> openSettings() async {}
}

void main() {
  setUpAll(loadGeist);

  for (final (device, size, padding) in const [
    ('se', Size(375, 667), FakeViewPadding(top: 20)),
    ('promax', Size(440, 956), FakeViewPadding(top: 62, bottom: 34)),
    ('ipad', Size(1032, 1376), FakeViewPadding(top: 24, bottom: 20)),
  ]) {
    for (final scale in const [1.0, 2.35]) {
      testWidgets('$device at ${scale}x: nothing cut off', (tester) async {
        if (device == 'ipad') {
          asIPad();
        } else {
          debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
          addTearDown(() => debugDefaultTargetPlatformOverride = null);
        }
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        tester.view.padding = padding;
        addTearDown(tester.view.resetPadding);
        final harness = Harness(Workspace(), localBackend: false);
        await harness.show(tester, size: size);
        await pumpFrames(tester);
        // The iPad's own pairing step comes from the app (main.dart); the
        // sheet over whatever the window shows is what's drawn here.
        final context = tester.element(
          find.byType(PairScreen).evaluate().isNotEmpty
              ? find.byType(PairScreen)
              : find.byType(Text).last,
        );
        unawaited(showLocalNetwork(context, _Answers()));
        await pumpFrames(tester);
        await tester.pump(const Duration(seconds: 1));
        expect(tester.takeException(), isNull);
        await _shot(tester, '$device-$scale-explain');

        await tester.ensureVisible(find.text('Continue'));
        await pumpFrames(tester);
        await tester.tap(find.text('Continue'));
        await pumpFrames(tester);
        await tester.pump(const Duration(seconds: 1));
        expect(tester.takeException(), isNull);
        await _shot(tester, '$device-$scale-off');
      });
    }
  }
}
