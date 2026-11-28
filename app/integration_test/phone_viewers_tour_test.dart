// Each file viewer on an iPhone, turned: the files in FILES (paths on the
// machine the dev Claktory runs on, comma-separated) opened one at a time
// over AGENT's conversation, then for each a Simulator screenshot in
// portrait, after a tap on its middle, turned to landscape, after a tap
// there, read further down, and turned back. Pairs with PAIRING_CODE when
// the app isn't paired yet (whoever runs it allows the device meanwhile).
//
// The Simulator's screenshots include the status bar and what the
// system draws (a PDF's pages): the tour asks for each by writing its
// name to SHOTS_DIR/request, and whoever runs it answers with
// `xcrun simctl io <udid> screenshot SHOTS_DIR/<name>.png`, then deletes
// the request. SHOTS_DIR/notes.txt says what each step saw, and "done"
// at the end.
//
//   scripts/ios.sh --dev --run --device <udid> \
//     --target integration_test/phone_viewers_tour_test.dart \
//     --define FILES=/tmp/tour/a.pdf,/tmp/tour/b.md \
//     --define SHOTS_DIR=/tmp/viewers [--define AGENT=Rivet]
//
// With --define OPEN_SHOT_MS=300, each file is shot that long after it's
// opened, then again that long after it's opened a second time, from the
// copy kept on the device (to see what a far link shows meanwhile: build
// with FACTORY_QA_ROUND_TRIP_MS and FACTORY_QA_DOWNLINK_KBPS).
// With --define SEAL=true, each file is a sealed one instead: a shot of
// it, then of its sealed version (Open sealed version) when it has one,
// then of it again after Back.
import 'dart:io';

import 'package:factory_app/main.dart' as app;
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/pairing.dart' show PairingDigits;
import 'package:factory_app/scope.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _code = String.fromEnvironment('PAIRING_CODE');
const _agent = String.fromEnvironment('AGENT', defaultValue: 'Rivet');
const _files = String.fromEnvironment('FILES');
const _openShotMs = int.fromEnvironment('OPEN_SHOT_MS');
const _seal = bool.fromEnvironment('SEAL');
const _shots = String.fromEnvironment(
  'SHOTS_DIR',
  defaultValue: '/tmp/viewers',
);

/// The phone's screens, once a Claktory shows.
final _shell = find.byType(PhoneRoutesScope);

/// Something inside them, whose context reaches the phone's routes.
final _inShell = find.descendant(
  of: _shell,
  matching: find.byWidgetPredicate((_) => true),
);

/// Not showing a Claktory yet: no phone screens, or the first one's
/// pairing.
bool _unpaired() =>
    _shell.evaluate().isEmpty ||
    find.text('Connect to your Claktory').evaluate().isNotEmpty ||
    find.byType(PairingDigits).evaluate().isNotEmpty;

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('each viewer, turned', (tester) async {
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

    Future<void> shot(String name, {int wait = 1200}) async {
      if (wait > 0) await settle(wait);
      final request = File('$_shots/request');
      final done = File('$_shots/$name.png');
      if (done.existsSync()) done.deleteSync();
      await request.writeAsString(name);
      final end = DateTime.now().add(const Duration(seconds: 30));
      while (!done.existsSync() || request.existsSync()) {
        if (DateTime.now().isAfter(end)) {
          note('no screenshot $name');
          if (request.existsSync()) request.deleteSync();
          return;
        }
        await settle(200);
      }
      final size = tester.view.physicalSize / tester.view.devicePixelRatio;
      note('shot $name: ${size.width.round()}×${size.height.round()}pt');
    }

    Future<void> turn(List<DeviceOrientation> to) async {
      await SystemChrome.setPreferredOrientations(to);
      await settle(2000);
    }

    // Off the middle, clear of a video's Play and a picture in the text.
    Offset spot() {
      final size = tester.view.physicalSize / tester.view.devicePixelRatio;
      return Offset(size.width * 0.2, size.height * 0.4);
    }

    Offset middle() {
      final size = tester.view.physicalSize / tester.view.devicePixelRatio;
      return size.center(Offset.zero);
    }

    await turn([DeviceOrientation.portraitUp]);
    await app.main();
    await settle(2500);
    // Paired already: its Claktory, once it's connected.
    for (var i = 0; i < 30 && _unpaired(); i++) {
      await settle(500);
    }
    // Not paired: the code from the define, or from SHOTS_DIR/code once
    // whoever runs it writes one there (codes last minutes, builds longer).
    var code = _code;
    final codeFile = File('$_shots/code');
    if (_unpaired() && code.isEmpty) {
      note('needs code');
      final until = DateTime.now().add(const Duration(minutes: 5));
      while (!codeFile.existsSync() && DateTime.now().isBefore(until)) {
        await settle(500);
      }
      if (codeFile.existsSync()) code = codeFile.readAsStringSync().trim();
    }
    if (_unpaired() && code.isNotEmpty) {
      for (var i = 0; i < 25; i++) {
        if (!_unpaired()) break;
        if (find.byType(EditableText).evaluate().isNotEmpty) break;
        if (find.text('Paste code instead').evaluate().isNotEmpty) {
          await tester.tap(find.text('Paste code instead'));
        }
        await settle(200);
      }
      if (_unpaired()) {
        await tester.enterText(find.byType(EditableText).first, code);
        await settle(300);
        await tester.tap(find.text('Connect').last);
        note('waiting to be allowed');
      }
    }
    final end = DateTime.now().add(const Duration(minutes: 3));
    while (_unpaired()) {
      if (DateTime.now().isAfter(end)) fail('never paired');
      await settle(500);
      // The question about a Claktory this phone hasn't seen, then the
      // one about the local network (the Simulator asks nothing after).
      if (find.text('Connect to this Claktory?').evaluate().isNotEmpty) {
        await tester.tap(find.text('Connect').last);
        await settle(500);
      }
      if (find
          .text('Find your Claktory on this network')
          .evaluate()
          .isNotEmpty) {
        await tester.tap(find.text('Continue').last);
        await settle(500);
      }
      final digits = find.byType(PairingDigits).evaluate();
      if (digits.isNotEmpty) {
        note('digits ${(digits.first.widget as PairingDigits).number}');
      } else if (DateTime.now().second % 15 == 0) {
        final shown = [
          for (final text in find.byType(Text).evaluate())
            (text.widget as Text).data ?? '',
        ].where((text) => text.isNotEmpty).join(' | ');
        note('on screen: $shown');
      }
    }
    await settle(2500);
    final scope = AppScope.scope(tester.element(_inShell.first));
    // Its agents, once the Claktory has sent them.
    for (var i = 0; i < 120; i++) {
      final agents = scope.backend.workspace.workspace.agents;
      if (agents.any((each) => each.name == _agent)) break;
      if (i % 20 == 19) {
        final shown = [
          for (final text in find.byType(Text).evaluate())
            (text.widget as Text).data ?? '',
        ].where((text) => text.isNotEmpty).join(' | ');
        note('no $_agent yet: ${agents.map((each) => each.name)}; $shown');
      }
      await settle(500);
    }
    final agent = scope.backend.workspace.workspace.agents.firstWhere(
      (each) => each.name == _agent,
    );
    await settle(2000);
    // A first pairing's question about notifications, put off by pulling
    // its sheet away.
    final notifications = find.text('Know when an agent needs you');
    if (notifications.evaluate().isNotEmpty) {
      await tester.drag(notifications.last, const Offset(0, 600));
      await settle(1000);
    }

    for (final path in _files.split(',').where((each) => each.isNotEmpty)) {
      final name = path.split('/').last.replaceAll('.', '-');
      final routes = PhoneRoutes.of(tester.element(_inShell.first));
      if (routes == null || !routes.showFile(agent.id, path)) {
        note('SKIPPED $path: no viewer opened');
        continue;
      }
      if (_openShotMs > 0) {
        await settle(_openShotMs);
        await shot('$name-0-opening', wait: 0);
        await settle(3000);
        await shot('$name-1-opened', wait: 0);
        await tester.tap(find.bySemanticsLabel('Back').last);
        await settle(1500);
        routes.showFile(agent.id, path);
        await settle(_openShotMs);
        await shot('$name-2-reopening', wait: 0);
        await settle(1500);
        await tester.tap(find.bySemanticsLabel('Back').last);
        await settle(1500);
        continue;
      }
      await settle(4000);
      if (_seal) {
        await shot('$name-1-current');
        final open = find.text('Open sealed version');
        if (open.evaluate().isNotEmpty) {
          await tester.tap(open.last);
          await settle(3000);
          await shot('$name-2-sealed');
          await tester.tap(find.bySemanticsLabel('Back').last);
          await settle(1500);
          await shot('$name-3-back');
        } else {
          note('$path: no Open sealed version');
        }
        await tester.tap(find.bySemanticsLabel('Back').last);
        await settle(1500);
        continue;
      }
      await shot('$name-1-portrait');
      await tester.tapAt(spot());
      await shot('$name-2-portrait-tapped');
      await tester.tapAt(spot());
      await turn([DeviceOrientation.landscapeLeft]);
      await shot('$name-3-landscape');
      await tester.tapAt(spot());
      await shot('$name-4-landscape-tapped');
      await tester.tapAt(spot());
      await settle(600);
      await tester.dragFrom(middle(), const Offset(0, -300));
      await shot('$name-5-landscape-read');
      await turn([DeviceOrientation.portraitUp]);
      await shot('$name-6-portrait-again');
      final back = find.bySemanticsLabel('Back');
      if (back.evaluate().isNotEmpty) {
        await tester.tap(back.last);
      } else {
        note('$path: no Back');
      }
      await settle(1500);
    }
    await SystemChrome.setPreferredOrientations(const []);
    note('done');
  });
}
