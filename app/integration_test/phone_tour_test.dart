// A tour of the iPhone app for screenshots (#198), against the dev
// Claktory, in the Simulator: pairing, the list of Claktories, the agents,
// a conversation with a three-line draft, a message's menu, the ⋯ sheet,
// a file and its comment popover, Settings, an agent's settings, and the
// Map. Each screen is saved as a PNG in SHOTS_DIR (a folder on this Mac:
// the Simulator's apps can write there). Whoever runs it allows the
// device on the dev Claktory meanwhile (`factoryd --dev allow <id>`).
// A step that can't be done is noted in SHOTS_DIR/notes.txt and skipped.
//
//   cd app && flutter test integration_test/phone_tour_test.dart \
//     -d <simulator> --dart-define=FACTORY_DEV=true \
//     --dart-define=PAIRING_CODE='factory://…' \
//     --dart-define=EXTRA_CODE='factory://…' --dart-define=AGENT=Rivet \
//     --dart-define=TOUR_FILE=/path/on/the/dev/machine.md \
//     --dart-define=SHOTS_DIR=/tmp/phone [--dart-define=SHORT=true]
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/forms.dart' show AgentSettingsForm;
import 'package:factory_app/controls.dart' show showModal;
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _code = String.fromEnvironment('PAIRING_CODE');
const _extra = String.fromEnvironment('EXTRA_CODE');
const _agent = String.fromEnvironment('AGENT', defaultValue: 'Rivet');
const _file = String.fromEnvironment('TOUR_FILE');
const _shots = String.fromEnvironment('SHOTS_DIR', defaultValue: '/tmp/phone');

/// Only the list, a conversation and Settings (for other sizes).
const _short = bool.fromEnvironment('SHORT');

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('a tour of the iPhone app', (tester) async {
    final notes = File('$_shots/notes.txt');
    await notes.create(recursive: true);
    void note(String text) {
      notes.writeAsStringSync('$text\n', mode: FileMode.append);
    }

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

    Future<void> shot(String name) async {
      await settle();
      final view = binding.renderViews.first;
      // The view's layer is already scaled to physical pixels.
      final image = await (view.debugLayer! as OffsetLayer).toImage(
        Offset.zero & view.size * view.flutterView.devicePixelRatio,
      );
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      await File('$_shots/$name.png').writeAsBytes(bytes!.buffer.asUint8List());
      note(
        'shot $name: ${view.size.width.round()}×${view.size.height.round()}pt '
        '@${view.flutterView.devicePixelRatio}x',
      );
    }

    // The system's own layers (the keyboard) aren't in the app's: whoever
    // runs the tour takes these with `simctl io screenshot` when asked, by
    // a file named `request` holding the name, and saves <name>.png.
    Future<void> simShot(String name) async {
      await settle();
      final request = File('$_shots/request');
      final done = File('$_shots/$name.png');
      await request.writeAsString(name);
      final end = DateTime.now().add(const Duration(seconds: 20));
      while (!done.existsSync() || request.existsSync()) {
        if (DateTime.now().isAfter(end)) {
          note('no simulator screenshot $name');
          if (request.existsSync()) request.deleteSync();
          return;
        }
        await settle(200);
      }
      note('shot $name: the Simulator\'s own screenshot');
    }

    Future<void> step(String name, Future<void> Function() body) async {
      try {
        await body();
      } catch (error) {
        note('SKIPPED $name: $error'.split('\n').first);
        final shown = [
          for (final text in find.byType(Text).evaluate())
            (text.widget as Text).data ?? '',
        ].where((text) => text.isNotEmpty);
        final all = shown.join(' | ');
        note('  on screen: ${all.length > 300 ? '${all.substring(0, 300)}…' : all}');
      }
    }

    await app.main();
    await settle(1500);

    // 9. Pairing: what a phone with nothing paired opens to.
    await shot('09a-pair-first');
    await step('paste', () async {
      for (var i = 0; i < 25; i++) {
        if (find.byType(EditableText).evaluate().isNotEmpty) break;
        if (find.text('Paste code instead').evaluate().isNotEmpty) {
          await tester.tap(find.text('Paste code instead'));
        }
        await settle(200);
      }
      if (find.byType(EditableText).evaluate().isEmpty) {
        throw StateError('no box to paste the code in');
      }
      await shot('09b-pair-paste');
    });
    await tester.enterText(find.byType(EditableText).first, _code);
    await settle(300);
    await tester.tap(find.text('Connect').last);
    await settle(1500);
    await shot('09c-waiting-for-allow');
    await until(find.byType(Sidebar), seconds: 120);
    if (find.text("What's your name?").evaluate().isNotEmpty) {
      await shot('10-name-step');
      await tester.enterText(find.byType(EditableText).last, 'Matt');
      await tester.tap(find.text('Continue'));
    }
    await settle(2500);

    // 2. The agents and projects, with what waits for the person.
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    final waiting = [
      for (final agent in backend.workspace.workspace.agents)
        if (agent.waitingSeqs.isNotEmpty)
          '${agent.name} (${agent.waitingSeqs.length})',
    ];
    note('agents waiting for an answer: ${waiting.isEmpty ? 'none' : waiting}');
    await shot('02-agents');

    // 1. The list of Claktories, with a second one added from a code.
    await step('claktories', () async {
      final first = scope.connections.current!;
      if (_extra.isNotEmpty) {
        scope.connections.use(parsePairingCode(_extra));
        await settle();
        scope.connections.use(first);
        await settle(2000);
      }
      await tester.tap(find.text('Claktories').first);
      await settle();
      await shot('01-claktories');
      await tester.tap(find.text(first.label).first);
      await settle(2000);
    });

    // 3. A conversation, with a three-line draft and the keyboard up.
    final agent = backend.workspace.workspace.agents.firstWhere(
      (each) => each.name == _agent,
    );
    await tester.tap(find.text(_agent).first);
    await settle(2000);
    await shot('03a-conversation');
    await step('draft', () async {
      final box = find.byType(EditableText).last;
      await tester.showKeyboard(box);
      await tester.enterText(
        box,
        'A first line of a draft\nand a second one\nand a third.',
      );
      await settle(1500);
      await shot('03b-draft-keyboard');
      note(
        'keyboard: the view is inset by '
        '${binding.renderViews.first.flutterView.viewInsets.bottom / binding.renderViews.first.flutterView.devicePixelRatio}pt at the bottom',
      );
      await simShot('03b-draft-keyboard-sim');
      // A touch and hold in the draft: what iOS offers (selection, Paste).
      await tester.longPress(box);
      await settle();
      await shot('03c-draft-long-press');
      await simShot('03c-draft-long-press-sim');
      note(
        'draft long press: toolbar '
        '${find.text('Paste').evaluate().isNotEmpty ? 'shows Paste' : 'shows no Paste'}, '
        'selection ${(tester.state<EditableTextState>(box).textEditingValue.selection)}',
      );
      FocusManager.instance.primaryFocus?.unfocus();
      await settle();
    });
    if (_short) {
      scope.page.value = AppPage.settings;
      await shot('07a-settings');
      scope.page.value = AppPage.agent;
      return;
    }

    // 4. A message's menu, from a touch and hold.
    await step('long press', () async {
      final texts = find.descendant(
        of: find.byType(Scrollable).first,
        matching: find.byType(RichText),
      );
      await tester.longPress(texts.at(texts.evaluate().length ~/ 2));
      await settle();
      await shot('04-long-press');
      await tester.tapAt(const Offset(20, 120));
      await settle();
    });

    // 5. The ⋯ sheet, with a file open beside the conversation.
    if (_file.isNotEmpty) {
      await backend.setLayout(
        agent.id,
        ViewNode(
          id: 'tour',
          split: ViewSplit(
            axis: ViewAxis.VIEW_AXIS_ROW,
            children: [
              ViewNode(
                id: 'chat',
                leaf: ViewLeaf(kind: 'chat'),
              ),
              ViewNode(
                id: 'file',
                leaf: ViewLeaf(
                  kind: 'text',
                  path: _file,
                  title: _file.split('/').last,
                ),
              ),
            ],
          ),
        ),
      );
      await settle();
    }
    await step('sheet', () async {
      await tester.tap(find.bySemanticsLabel('Extensions').first);
      await settle();
      await shot('05-more-sheet');
    });

    // 6. The file, and the comment popover on a selection.
    await step('viewer', () async {
      await tester.tap(find.text(_file.split('/').last).last);
      await settle(2500);
      await shot('06a-text-viewer');
      final lines = find.byType(RichText);
      await tester.longPress(lines.at(lines.evaluate().length ~/ 3));
      await settle();
      await shot('06b-selection');
      if (find.text('Comment').evaluate().isNotEmpty) {
        await tester.tap(find.text('Comment').first);
        await settle();
        await shot('06c-comment-popover');
      }
      for (var i = 0; i < 3; i++) {
        if (find.bySemanticsLabel('Back').evaluate().isEmpty) break;
        await tester.tap(find.bySemanticsLabel('Back').last);
        await settle();
      }
    });
    await backend.setLayout(agent.id, null);

    // 7. Settings, and an agent's settings.
    await step('settings', () async {
      scope.page.value = AppPage.settings;
      await shot('07a-settings');
      await tester.drag(find.byType(Scrollable).first, const Offset(0, -900));
      await shot('07b-settings-further');
      scope.page.value = AppPage.agent;
      await settle();
      final context = tester.element(find.byType(Sidebar).first);
      showModal(
        context,
        (context, close) => AgentSettingsForm(agent: agent, close: close),
      );
      await shot('07c-agent-settings');
      await tester.drag(find.byType(Scrollable).last, const Offset(0, -600));
      await shot('07d-agent-settings-further');
      await tester.tap(find.text('Cancel').last);
      await settle();
    });

    // 8. The Map, and a card chosen on it.
    await step('map', () async {
      scope.page.value = AppPage.map;
      await settle(2500);
      await shot('08a-map');
      await tester.tap(find.text(_agent).last);
      await settle();
      await shot('08b-map-selected');
      scope.page.value = AppPage.agent;
      await settle();
    });
  });
}
