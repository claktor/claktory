// Needs you for chats, against a real Claktory (the dev one), on a Mac: a
// chat made in the app proposes to Rivet, and while the proposal waits
//
//   1. the sidebar's Needs you row and the chip over the conversation count
//      it, and its Claktory's tile in the rail shows the dot;
//   2. on the Needs you page its bubble has the chat's title and glyph;
//   3. opened, the bubble shows the proposal's card, and Open chat opens
//      the chat;
//   4. from the chip, the folded bubble's Dismiss decides it, and the count
//      goes.
//
// It saves a screenshot of the window at each step in SHOTS_DIR
// (/tmp/chat-needs-you unless set), and deletes the chat at the end. The
// chat's model runs for real.
//
// The test drives frames itself, so it runs with its window in the
// background (macOS draws nothing for a window that's covered):
//
//   cd app && FLUTTER_XCODE_PRODUCT_BUNDLE_IDENTIFIER=dev.factory.app.itest \
//     flutter test integration_test/chat_needs_you_test.dart -d macos \
//     --dart-define=FACTORY_DEV=true [--dart-define=ELSEWHERE=Builder]
//
// (Its own bundle id, so it never shares the real Claktory's Keychain.)
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/chats.dart';
import 'package:factory_app/inbox.dart';
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/transcript.dart' show ProposalCard;
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _elsewhere = String.fromEnvironment('ELSEWHERE', defaultValue: 'Builder');
const _shots = String.fromEnvironment(
  'SHOTS_DIR',
  defaultValue: '/tmp/chat-needs-you',
);

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  // Frames come from the test, not the screen.
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.benchmark;

  testWidgets("a chat's proposal waits in Needs you, opens and goes", (
    tester,
  ) async {
    var clock = Duration.zero;

    /// Lets [wait] pass, then draws a frame.
    Future<void> step([
      Duration wait = const Duration(milliseconds: 100),
    ]) async {
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

    Future<void> hold(Duration time) async {
      final end = DateTime.now().add(time);
      while (DateTime.now().isBefore(end)) {
        await step();
      }
    }

    /// The whole window as it is now, as a PNG in [_shots].
    Future<void> shot(String name) async {
      await hold(const Duration(milliseconds: 500));
      final view = binding.renderViews.first;
      final layer = view.debugLayer! as OffsetLayer;
      final image = await layer.toImage(
        Offset.zero & view.size * view.flutterView.devicePixelRatio,
      );
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      final file = File('$_shots/$name.png');
      await file.create(recursive: true);
      await file.writeAsBytes(bytes!.buffer.asUint8List());
      // ignore: avoid_print
      print('screenshot: ${file.path}');
    }

    Finder labeled(String text) => find.byWidgetPredicate(
      (widget) => widget is Semantics && widget.properties.label == text,
    );

    await app.main();
    await until(
      'the sidebar',
      () => find.byType(Sidebar).evaluate().isNotEmpty,
      timeout: const Duration(minutes: 1),
    );
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    Workspace workspace() => backend.workspace.workspace;
    await until(
      'the workspace',
      () => workspace().agents.any((agent) => agent.name == _elsewhere),
      timeout: const Duration(minutes: 1),
    );
    final before = {for (final chat in workspace().chats) chat.id};

    // A new chat, made in the app.
    await tester.tap(labeled('New chat').first);
    await until(
      'the new chat',
      () =>
          workspace().selectedChatId.isNotEmpty &&
          !before.contains(workspace().selectedChatId) &&
          find.byType(ChatView).evaluate().isNotEmpty,
    );
    final id = workspace().selectedChatId;
    Chat chat() => workspace().chats.firstWhere((chat) => chat.id == id);

    try {
      final stamp = DateTime.now().millisecondsSinceEpoch % 100000;
      // enterText waits for a frame from the screen, which a window in the
      // background never draws: type through the field's own controller.
      final box = tester.state<EditableTextState>(
        find.descendant(
          of: find.byType(ChatView),
          matching: find.byType(EditableText),
        ),
      );
      box.widget.controller.text =
          'This is a test of the app ($stamp). Call your tool that proposes '
          'to Rivet once, right now, proposing exactly: "Add a designer agent '
          '(a test of the app; dismiss it)". Then stop and say nothing else.';
      box.widget.focusNode.requestFocus();
      await step();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await until(
        'the proposal',
        () => workspace().chats.any(
          (chat) => chat.id == id && chat.waitingSeqs.isNotEmpty,
        ),
        timeout: const Duration(minutes: 3),
      );
      await until(
        'its reply to end',
        () => chat().status != AgentStatus.AGENT_STATUS_WORKING,
        timeout: const Duration(minutes: 2),
      );
      final title = chatTitle(chat());
      // ignore: avoid_print
      print('chat "$title" waits: ${chat().waitingSeqs}');

      // 1. The row, the chip and the rail count it.
      final count = needsYouCount(workspace());
      await until(
        'the row and the chip',
        () =>
            labeled(needsYouLabel(count)).evaluate().isNotEmpty &&
            find.text(needsYouChipText(count)).evaluate().isNotEmpty,
      );
      await shot('1-waiting');

      // 2. The page: the bubble with the chat's title and glyph.
      final elsewhere = workspace().agents.firstWhere(
        (agent) => agent.name == _elsewhere,
      );
      await backend.selectAgent(elsewhere.id);
      await until('the agent shown', () => workspace().selectedChatId.isEmpty);
      await tester.tap(
        find.descendant(
          of: find.byType(Sidebar),
          matching: find.text('Needs you'),
        ),
      );
      final bubble = find.byKey(
        ValueKey('chat:$id/${chat().waitingSeqs.first}'),
      );
      await until(
        'the bubble, fetched',
        () =>
            bubble.evaluate().isNotEmpty &&
            find
                .descendant(
                  of: bubble,
                  matching: find.textContaining('Proposes to'),
                )
                .evaluate()
                .isNotEmpty,
      );
      expect(
        find.descendant(
          of: bubble,
          matching: find.textContaining(title, findRichText: true),
        ),
        findsWidgets,
      );
      expect(
        find.descendant(of: bubble, matching: find.byType(ChatGlyph)),
        findsOneWidget,
      );
      await shot('2-page-folded');

      // 3. Opened: the card, and Open chat opens it.
      await tester.tap(
        find.descendant(
          of: bubble,
          matching: find.textContaining('Proposes to'),
        ),
      );
      await until(
        'the card',
        () => find
            .descendant(of: bubble, matching: find.byType(ProposalCard))
            .evaluate()
            .isNotEmpty,
      );
      await shot('3-page-open');
      await tester.tap(
        find.descendant(of: bubble, matching: find.text('Open chat')),
      );
      await until(
        'the chat opened',
        () =>
            workspace().selectedChatId == id &&
            find.byType(InboxView).evaluate().isEmpty &&
            find.byType(ChatView).evaluate().isNotEmpty,
      );
      await shot('4-chat-opened');

      // 4. From the chip, folded: Dismiss.
      await tester.tap(find.text(needsYouChipText(needsYouCount(workspace()))));
      await until(
        'the chip open',
        () => find
            .descendant(of: bubble, matching: find.text('Dismiss'))
            .evaluate()
            .isNotEmpty,
      );
      await shot('5-chip-open');
      await tester.tap(
        find.descendant(of: bubble, matching: find.text('Dismiss')),
      );
      await until('dismissed', () => chat().waitingSeqs.isEmpty);
      await shot('6-dismissed');
    } finally {
      await backend.deleteChat(id);
    }
  });
}
