// A baseline of the iPhone app on an iPad, against the dev Claktory,
// in the Simulator: what the app does there today, nothing changed for the
// shots. Pairing, the conversation, the drawer, a draft with the keyboard,
// the More sheet, a file in its viewer and a selection in it, then the same
// in landscape. Each screen is saved in SHOTS_DIR twice: the app's own
// drawing (<name>.png) and, when the runner answers a `request` file there
// with `xcrun simctl io <udid> screenshot`, the whole iPad screen
// (<name>-sim.png: the keyboard, and the frame an iPhone app gets on an
// iPad). Notes, sizes and what couldn't be done go in SHOTS_DIR/notes.txt.
//
//   cd app && flutter test integration_test/ipad_tour_test.dart \
//     -d <simulator> --dart-define=FACTORY_DEV=true \
//     --dart-define=PAIRING_CODE='factory://…' --dart-define=AGENT=Rivet \
//     --dart-define=TOUR_FILE=/path/on/the/dev/machine.md \
//     --dart-define=SHOTS_DIR=/tmp/ipad
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/explorer.dart' show openFile;
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _code = String.fromEnvironment('PAIRING_CODE');
const _agent = String.fromEnvironment('AGENT', defaultValue: 'Rivet');
const _file = String.fromEnvironment('TOUR_FILE');
const _shots = String.fromEnvironment('SHOTS_DIR', defaultValue: '/tmp/ipad');

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('the iPhone app on an iPad', (tester) async {
    final notes = File('$_shots/notes.txt');
    await notes.create(recursive: true);
    void note(String text) =>
        notes.writeAsStringSync('$text\n', mode: FileMode.append);

    Future<void> settle([int ms = 1200]) async {
      final end = DateTime.now().add(Duration(milliseconds: ms));
      while (DateTime.now().isBefore(end)) {
        await tester.pump(const Duration(milliseconds: 50));
        await Future<void>.delayed(const Duration(milliseconds: 50));
      }
    }

    Future<void> until(Finder finder, {int seconds = 60}) async {
      final end = DateTime.now().add(Duration(seconds: seconds));
      while (finder.evaluate().isEmpty) {
        if (DateTime.now().isAfter(end)) fail('never found $finder');
        await settle(200);
      }
    }

    String onScreen() {
      final all = [
        for (final text in find.byType(Text).evaluate())
          (text.widget as Text).data ?? '',
      ].where((text) => text.isNotEmpty).join(' | ');
      return all.length > 400 ? '${all.substring(0, 400)}…' : all;
    }

    // The app's own drawing, then the Simulator's screenshot of the whole
    // screen, asked for by a file the runner watches.
    Future<void> shot(String name) async {
      await settle();
      final view = binding.renderViews.first;
      final ratio = view.flutterView.devicePixelRatio;
      final image = await (view.debugLayer! as OffsetLayer).toImage(
        Offset.zero & view.size * ratio,
      );
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      await File('$_shots/$name.png').writeAsBytes(bytes!.buffer.asUint8List());
      final insets = view.flutterView.viewInsets.bottom / ratio;
      note(
        'shot $name: the app is ${view.size.width.round()}×'
        '${view.size.height.round()}pt @${ratio}x'
        '${insets > 0 ? ', keyboard ${insets.round()}pt' : ''}',
      );
      final request = File('$_shots/request');
      final done = File('$_shots/$name-sim.png');
      await request.writeAsString('$name-sim');
      final end = DateTime.now().add(const Duration(seconds: 20));
      while (!done.existsSync() || request.existsSync()) {
        if (DateTime.now().isAfter(end)) {
          note('  no Simulator screenshot for $name');
          if (request.existsSync()) request.deleteSync();
          return;
        }
        await settle(200);
      }
    }

    Future<void> step(String name, Future<void> Function() body) async {
      try {
        await body();
      } catch (error) {
        note('SKIPPED $name: $error'.split('\n').first);
        note('  on screen: ${onScreen()}');
      }
    }

    await app.main();
    await settle(1500);

    // Pairing: what an iPad with nothing paired opens to.
    await shot('01-pair-first');
    for (var i = 0; i < 25; i++) {
      if (find.byType(EditableText).evaluate().isNotEmpty) break;
      if (find.text('Paste code instead').evaluate().isNotEmpty) {
        await tester.tap(find.text('Paste code instead'));
      }
      await settle(200);
    }
    await shot('02-pair-paste');
    await tester.enterText(find.byType(EditableText).first, _code);
    await settle(300);
    await tester.tap(find.text('Connect').last);
    final menu = find.bySemanticsLabel('Menu');
    await until(menu, seconds: 120);
    if (find.text("What's your name?").evaluate().isNotEmpty) {
      await shot('03-name-step');
      await tester.enterText(find.byType(EditableText).last, 'Matt');
      await tester.tap(find.text('Continue'));
    }
    await settle(2500);
    await shot('04-first-screen');

    final scope = AppScope.scope(tester.element(menu.first));
    final routes = PhoneRoutes.of(tester.element(menu.first))!;
    final backend = scope.backend;
    for (var i = 0; i < 300; i++) {
      if (backend.workspace.workspace.agents.any((a) => a.name == _agent)) {
        break;
      }
      await settle(200);
    }
    note(
      'target: ${backend.target?.label} tunnelled ${backend.target?.tunnelled}, '
      'refused: ${backend.refused.value}, '
      'sign in: ${backend.signInRequired.value}, '
      'pairing: ${backend.pairing.value}, '
      'tunnel: ${backend.tunnel.value?.phase} ${backend.tunnel.value?.path} '
      'relay ${backend.tunnel.value?.relayConnected} '
      '"${backend.tunnel.value?.error}"',
    );
    note(
      'connected: ${backend.connected.value}, agents: '
      '${backend.workspace.workspace.agents.map((a) => a.name).toList()}',
    );
    note('  on screen: ${onScreen()}');
    final agent = backend.workspace.workspace.agents.firstWhere(
      (each) => each.name == _agent,
    );
    note(
      'agents waiting for an answer: ${[for (final each in backend.workspace.workspace.agents)
        if (each.waitingSeqs.isNotEmpty) each.name]}',
    );

    Future<void> tour(String orientation) async {
      routes.openConversation(agent.id);
      await settle(2000);
      await shot('$orientation-1-conversation');
      await step('$orientation drawer', () async {
        await tester.tap(menu.first);
        await settle();
        await shot('$orientation-2-drawer');
        routes.closeDrawer();
        await settle();
      });
      await step('$orientation keyboard', () async {
        final box = find.byType(EditableText).last;
        await tester.showKeyboard(box);
        await tester.enterText(
          box,
          'A first line of a draft\nand a second one\nand a third.',
        );
        await settle(1500);
        await shot('$orientation-3-draft-keyboard');
        await tester.enterText(box, '');
        FocusManager.instance.primaryFocus?.unfocus();
        await settle();
      });
      await step('$orientation more', () async {
        await tester.tap(find.bySemanticsLabel('More').first);
        await settle();
        await shot('$orientation-4-more-sheet');
        await tester.tapAt(const Offset(20, 20));
        await settle();
      });
      if (_file.isEmpty) return;
      await step('$orientation viewer', () async {
        openFile(tester.element(menu.first), _file);
        await settle(2500);
        await shot('$orientation-5-viewer');
        note('  viewer: ${onScreen()}');
        final lines = find.byType(RichText);
        await tester.longPress(lines.at(lines.evaluate().length ~/ 3));
        await settle();
        await shot('$orientation-6-selection');
        note(
          '  after a touch and hold in the file: Comment '
          '${find.text('Comment').evaluate().isEmpty ? 'not offered' : 'offered'}',
        );
        if (find.text('Comment').evaluate().isNotEmpty) {
          await tester.tap(find.text('Comment').first);
          await settle();
          await shot('$orientation-7-comment');
        }
        for (var i = 0; i < 3 && routes.canPop; i++) {
          routes.pop<void>();
          await settle();
        }
      });
      await backend.setLayout(agent.id, null);
      await settle();
    }

    await tour('portrait');
    await SystemChrome.setPreferredOrientations([
      DeviceOrientation.landscapeLeft,
    ]);
    await settle(2500);
    await tour('landscape');
    await SystemChrome.setPreferredOrientations([]);
    note('done');
  });
}
