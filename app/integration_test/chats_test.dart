// Chats from the Mac app against a real Claktory (the dev one): the
// sidebar's New chat opens an empty chat, a question sent in it gets a
// reply and a title, Rename names it, and Delete (asked first) removes it.
// The dev Claktory needs a model that can run. Screenshots go to SHOTS_DIR
// (/tmp/chats unless set).
//
//   cd app && FLUTTER_XCODE_PRODUCT_BUNDLE_IDENTIFIER=dev.factory.app.itest \
//     flutter test integration_test/chats_test.dart -d macos \
//     --dart-define=FACTORY_DEV=true
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/chats.dart';
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _shots = String.fromEnvironment('SHOTS_DIR', defaultValue: '/tmp/chats');

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  // Frames come from the test, not the screen, so a covered window works.
  binding.framePolicy = LiveTestWidgetsFlutterBindingFramePolicy.benchmark;

  testWidgets('a chat: new, asked, answered, renamed and deleted', (
    tester,
  ) async {
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
      Duration timeout = const Duration(seconds: 120),
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
        Offset.zero & view.size * view.flutterView.devicePixelRatio,
      );
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      final file = File('$_shots/$name.png');
      await file.create(recursive: true);
      await file.writeAsBytes(bytes!.buffer.asUint8List());
      // ignore: avoid_print
      print('screenshot: ${file.path}');
    }

    await app.main();
    await until(
      'the sidebar',
      () => find.byType(Sidebar).evaluate().isNotEmpty,
    );
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    await until('the workspace', () => backend.workspace.received);
    Workspace ws() => backend.workspace.workspace;
    final before = {for (final chat in ws().chats) chat.id};

    // The Chats section's "+".
    final plus = find.bySemanticsLabel('New chat');
    await until('New chat', () => plus.evaluate().isNotEmpty);
    await tester.tap(plus.first);
    await until('the new chat', () => selectedChat(ws()) != null);
    final id = selectedChat(ws())!.id;
    expect(before, isNot(contains(id)));
    await until('its view', () => find.byType(ChatView).evaluate().isNotEmpty);
    await shot('1-new');

    // Typed straight into the box: enterText waits for a frame from the
    // screen, which a window in the background never draws.
    final box = tester.widget<EditableText>(
      find.descendant(
        of: find.byType(ChatView),
        matching: find.byType(EditableText),
      ),
    );
    box.focusNode.requestFocus();
    box.controller.text = 'What is the capital of France? Answer in one word.';
    await step();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    Chat chat() => ws().chats.firstWhere((chat) => chat.id == id);
    bool answered() {
      final store = backend.chatTranscript;
      for (var i = 0; i < store.length; i++) {
        final entry = store[i].entry;
        if (entry.hasAssistant() && entry.assistant.text.isNotEmpty) {
          return chat().status != AgentStatus.AGENT_STATUS_WORKING;
        }
      }
      return false;
    }

    await until('the answer', answered);
    await shot('2-answered');
    // ignore: avoid_print
    print('title: "${chat().title}" model: ${chat().model}');
    expect(chat().title, isNotEmpty);

    await backend.renameChat(id, 'Capitals');
    await until('the new name', () => chat().title == 'Capitals');
    await until(
      'the new name shown',
      () => find
          .textContaining('Capitals', findRichText: true)
          .evaluate()
          .isNotEmpty,
    );
    await shot('3-renamed');

    await tester.tap(
      find.descendant(
        of: find.byType(ChatView),
        matching: find.bySemanticsLabel('More'),
      ),
    );
    await until('Delete…', () => find.text('Delete…').evaluate().isNotEmpty);
    await tester.tap(find.text('Delete…'));
    await until(
      'the question',
      () => find.text('Delete Capitals?').evaluate().isNotEmpty,
    );
    await shot('4-delete');
    await tester.tap(find.text('Delete'));
    await until('it gone', () => ws().chats.every((chat) => chat.id != id));
    await shot('5-deleted');
  });
}
