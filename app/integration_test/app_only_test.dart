// A Mac marked app only (ClaktoryAppOnly, as `install.sh --app-only`
// writes it): the app opens on "Connect to your Claktory" instead of this
// Mac's own, and "Set up Claktory on this Mac" brings it back, clearing
// the mark. It saves screenshots in SHOTS_DIR (/tmp/app-only unless set).
//
//   cd app && FLUTTER_XCODE_PRODUCT_BUNDLE_IDENTIFIER=dev.factory.app.itest \
//     flutter test integration_test/app_only_test.dart -d macos \
//     --dart-define=FACTORY_DEV=true
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/connect_step.dart';
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/setup.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _shots = String.fromEnvironment(
  'SHOTS_DIR',
  defaultValue: '/tmp/app-only',
);

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  // Frames come from the test, not the screen, so a covered window works.
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.benchmark;

  testWidgets('a Mac marked app only asks for a code first', (tester) async {
    var clock = Duration.zero;
    Future<void> step() async {
      const wait = Duration(milliseconds: 100);
      await Future<void>.delayed(wait);
      clock += wait;
      await tester.pumpBenchmark(clock);
    }

    Future<void> until(
      String what,
      bool Function() done, {
      Duration timeout = const Duration(seconds: 30),
    }) async {
      final end = DateTime.now().add(timeout);
      await step();
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await step();
      }
    }

    Future<void> shot(String name) async {
      for (var i = 0; i < 5; i++) {
        await step();
      }
      final view = binding.renderViews.first;
      final image = await (view.debugLayer! as OffsetLayer).toImage(
        Offset.zero & view.size,
        pixelRatio: view.flutterView.devicePixelRatio,
      );
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      final file = File('$_shots/$name.png');
      await file.create(recursive: true);
      await file.writeAsBytes(bytes!.buffer.asUint8List());
      // ignore: avoid_print
      print('screenshot: ${file.path}');
    }

    const channel = SetupChannel();
    await channel.setAppOnly(true);
    addTearDown(() => channel.setAppOnly(false));
    await app.main();
    await until(
      'the question',
      () => find.byType(ConnectStep).evaluate().isNotEmpty,
    );
    await shot('1-connect-to-your-claktory');
    await tester.tap(find.text('Set up Claktory on this Mac'));
    await until(
      "this Mac's own",
      () =>
          find.byType(ConnectStep).evaluate().isEmpty &&
          find.byType(Sidebar).evaluate().isNotEmpty,
    );
    expect(await channel.appOnly(), isFalse);
    await shot('2-this-mac');
  });
}
