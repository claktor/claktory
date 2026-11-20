// claktory://add?code=… through macOS, on a Mac: the debug build's own
// scheme (claktory-debug) is opened with `open`, as an agent would open
// the real one, and the app asks "Connect to …?". Cancel leaves it be. It
// saves a screenshot in SHOTS_DIR (/tmp/links unless set).
//
//   cd app && FLUTTER_XCODE_PRODUCT_BUNDLE_IDENTIFIER=dev.factory.app.itest \
//     flutter test integration_test/links_test.dart -d macos \
//     --dart-define=FACTORY_DEV=true
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/main.dart' as app;
import 'package:factory_app/sidebar.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _shots = String.fromEnvironment('SHOTS_DIR', defaultValue: '/tmp/links');

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  // Frames come from the test, not the screen, so a covered window works.
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.benchmark;

  testWidgets('a claktory link asks before connecting', (tester) async {
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

    await app.main();
    await until('the window', () => find.byType(Sidebar).evaluate().isNotEmpty);
    final code = 'factory://?t=${'a' * 64}&name=box&e=${'c' * 64}';
    final opened = await Process.run('/usr/bin/open', [
      // The app this test runs, by its path: the scheme may have other
      // debug builds behind it.
      '-a',
      Platform.resolvedExecutable.replaceFirst(
        RegExp(r'/Contents/MacOS/.*$'),
        '',
      ),
      'claktory-debug://add?code=${Uri.encodeComponent(code)}',
    ]);
    expect(opened.exitCode, 0, reason: '${opened.stderr}');
    await until(
      'the question',
      () => find.text('Connect to box?').evaluate().isNotEmpty,
    );
    await shot('1-connect-to-box');
    await tester.tap(find.text('Cancel'));
    await until(
      'the question gone',
      () => find.text('Connect to box?').evaluate().isEmpty,
    );
  });
}
