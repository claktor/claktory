// The site's phone beat (Kakyoin's demo): a real Needs you card on a
// Claktory, answered on the phone at a pace for a recording. Pairs with
// PAIRING_CODE when the app isn't paired yet (whoever runs it allows the
// device on the Claktory while it waits), opens Needs you from the menu,
// opens the card titled TITLE and holds on it, types ANSWER and sends it,
// then opens the agent's conversation and holds there. Whoever runs it
// records the Simulator meanwhile. It answers a real card: run it once.
//
// MARKS_DIR gets "start" (the app's first frame), "answered" and "end",
// each the time in milliseconds since the epoch, to trim the recording.
// Where a value isn't given as a define, it's read from MARKS_DIR's file
// of the same name in lower case (code, title, answer, agent: the agent
// asking, in case its row is gone once answered), so a build made
// once serves any card.
//
//   scripts/ios.sh --dev --run --device <udid> \
//     --target integration_test/phone_needs_you_demo_test.dart \
//     --define MARKS_DIR=/tmp/needs-you-demo \
//     --define "ANSWER=B, the clean and modern one."
import 'dart:io';

import 'package:factory_app/ask_card.dart' show AskPersonCard;
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/message_box.dart' show MessageBox;
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _marks = String.fromEnvironment(
  'MARKS_DIR',
  defaultValue: '/tmp/needs-you-demo',
);

/// A define, or MARKS_DIR's file of that name, or [fallback].
String _value(String define, String file, String fallback) {
  if (define.isNotEmpty) return define;
  final read = File('$_marks/$file');
  if (read.existsSync()) {
    final text = read.readAsStringSync().trim();
    if (text.isNotEmpty) return text;
  }
  return fallback;
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  final code = _value(const String.fromEnvironment('PAIRING_CODE'), 'code', '');
  final title = _value(
    const String.fromEnvironment('TITLE'),
    'title',
    "Which design for Rosa's?",
  );
  final answer = _value(
    const String.fromEnvironment('ANSWER'),
    'answer',
    'B, the clean and modern one.',
  );
  final agent = _value(const String.fromEnvironment('AGENT'), 'agent', 'Lucia');

  testWidgets('the phone answers a Needs you card', (tester) async {
    void mark(String name) => File('$_marks/$name')
      ..createSync(recursive: true)
      ..writeAsStringSync('${DateTime.now().millisecondsSinceEpoch}');

    /// Lets [ms] of real time pass, drawing every frame.
    Future<void> hold([int ms = 1200]) async {
      final end = DateTime.now().add(Duration(milliseconds: ms));
      while (DateTime.now().isBefore(end)) {
        await tester.pump(const Duration(milliseconds: 16));
        await Future<void>.delayed(const Duration(milliseconds: 16));
      }
    }

    Future<void> until(
      bool Function() done,
      String what, {
      int seconds = 90,
    }) async {
      final end = DateTime.now().add(Duration(seconds: seconds));
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await hold(100);
      }
    }

    bool shows(Finder finder) => finder.evaluate().isNotEmpty;

    /// Types [text] into [field] a few letters at a time, as a person would.
    Future<void> type(Finder field, String text) async {
      for (var at = 0; at <= text.length; at += 3) {
        await tester.enterText(field, text.substring(0, at));
        await hold(55);
      }
      await tester.enterText(field, text);
      await hold(500);
    }

    final drawerNeedsYou = find.byKey(const ValueKey('needs-you'));
    final menu = find.bySemanticsLabel('Menu');

    await app.main();
    await hold(300);
    mark('start');
    await hold(2000);

    // Not paired yet: its code pasted in, then the four digits while it's
    // allowed there.
    if (!shows(find.byType(Sidebar)) && !shows(menu)) {
      if (code.isEmpty) fail('not paired, and no PAIRING_CODE or $_marks/code');
      for (var i = 0; i < 25; i++) {
        if (shows(find.byType(EditableText))) break;
        if (shows(find.text('Paste code instead'))) {
          await tester.tap(find.text('Paste code instead'));
        }
        await hold(300);
      }
      await hold(800);
      await tester.enterText(find.byType(EditableText).first, code);
      await hold(1200);
      await tester.tap(find.text('Connect').last);
      mark('waiting');
    }
    // Connected: the menu, or a conversation with its Menu button.
    await until(
      () =>
          !shows(find.textContaining('Waiting for', findRichText: true)) &&
          (shows(drawerNeedsYou) || shows(menu)),
      'connected',
      seconds: 180,
    );
    await hold(1800);

    // Needs you, from the top of the menu.
    if (!shows(drawerNeedsYou)) {
      await tester.tap(menu.first);
      await hold(1400);
    }
    await until(() => shows(drawerNeedsYou), 'saw Needs you in the menu');
    await hold(900);
    await tester.tap(drawerNeedsYou.first);
    await hold(1600);

    // The card: its row opened, unless it shows open already.
    final card = find.byWidgetPredicate(
      (widget) => widget is AskPersonCard && widget.ask.title == title,
    );
    await until(
      () => shows(card) || shows(find.textContaining(title)),
      'found "$title" in Needs you',
    );
    if (!shows(card)) {
      await tester.tap(find.textContaining(title).first);
      await hold(1200);
    }
    await until(() => shows(card), 'opened "$title"');
    await tester.ensureVisible(card.first);
    await hold(4000);

    // The answer, typed and sent.
    final field = find.descendant(
      of: card.first,
      matching: find.byType(EditableText),
    );
    await tester.ensureVisible(field.first);
    await hold(400);
    await tester.tap(field.first);
    await hold(700);
    await type(field.first, answer);
    FocusManager.instance.primaryFocus?.unfocus();
    await hold(700);
    final send = find.descendant(of: card.first, matching: find.text('Answer'));
    await tester.ensureVisible(send.first);
    await hold(300);
    await tester.tap(send.first);
    mark('answered');
    await hold(2500);

    // The agent's conversation, where the answer went.
    final open = find.text('Open conversation');
    if (shows(open)) {
      await tester.ensureVisible(open.first);
      await hold(400);
      await tester.tap(open.first);
    } else {
      // Its row gone: back out, and the agent from the menu.
      final back = find.bySemanticsLabel('Back');
      if (shows(back)) await tester.tap(back.last);
      await hold(1200);
      if (!shows(find.byType(Sidebar))) await tester.tap(menu.first);
      await hold(1400);
      await tester.tap(find.text(agent).last);
    }
    await until(
      () => shows(find.byType(MessageBox)),
      "opened the agent's conversation",
      seconds: 20,
    );
    await hold(10000);
    mark('end');
  });
}
