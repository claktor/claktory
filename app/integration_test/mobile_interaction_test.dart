// The iPhone's chat and viewer, end to end in the Simulator (research mobile.md §6,
// "Chat/viewer suite", and the §7 matrix items a Simulator can show): the message
// box and the real keyboard, a conversation streaming while you read earlier
// messages, touch and hold versus selecting text, comments from selection to
// resolved, and, on every screen it passes, 44pt targets with labels, text contrast
// (4.5:1, or 3:1 for large text; a state's deliberate fade, such as a queued
// message, is listed as exempted) and no overflow, at whatever text size and
// appearance the Simulator is set to. Haptics, recording, VoiceOver on hardware and
// the paste permission stay with a real iPhone.
//
// It runs against the dev Claktory the Simulator is already paired with
// (pair it first with phone_test.dart), never the person's. Set the
// Simulator up for each pass, then run:
//
//   # The software keyboard must show, as on a phone, not minimized for
//   # the Mac's keyboard:
//   xcrun simctl spawn <udid> defaults write com.apple.keyboard.preferences \
//     AutomaticMinimizationEnabled -bool false
//   xcrun simctl ui <udid> appearance light|dark
//   xcrun simctl ui <udid> content_size large|accessibility-extra-extra-extra-large
//   cd app && flutter test integration_test/mobile_interaction_test.dart \
//     -d <udid> --dart-define=FACTORY_DEV=true --dart-define=AGENT=Rivet \
//     --dart-define=VIEW_FILE=/abs/path/on/the/dev/machine/docs/IDENTITY.md \
//     [--dart-define=EXPECT=dark,large] [--dart-define=SEND=true] \
//     [--dart-define=SHOTS_DIR=/abs/path]
//
// EXPECT makes sure the pass ran as set up (dark or light, large: past
// 1.5× text). VIEW_FILE is a Markdown file on the dev Claktory's machine;
// without it the comments are skipped. The suite writes one comment and
// one reply on it, which go to AGENT (one that can't run, waiting for
// usage, say, keeps them queued), then deletes them; it puts the file
// beside AGENT's conversation for the run and restores the layout after.
// With SEND, it also sends AGENT a message, to see the keyboard stay up.
// The conversation's streaming is played into the app's own transcript, as
// the backend's updates arrive, so it needs no model: nothing of it
// reaches the backend.
//
// With SHOTS_DIR, each step's screen is saved there (the app's layers:
// no keyboard), and the system's own (keyboard, edit menu) are asked for
// by writing the name in SHOTS_DIR/request: a loop on the Mac answers
// with `xcrun simctl io <udid> screenshot SHOTS_DIR/<name>.png` and
// deletes the request. Unanswered, it goes on without.
//
// Every problem is collected, and the test fails at its end listing them
// all (also in SHOTS_DIR/notes.txt).
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoAdaptiveTextSelectionToolbar;
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/mobile/composer.dart';
import 'package:factory_app/mobile/presentation.dart' show PhoneSheet;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/transcript.dart';
import 'package:factory_app/viewers/text.dart' show TextViewer;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _agentName = String.fromEnvironment('AGENT', defaultValue: 'Rivet');
const _file = String.fromEnvironment('VIEW_FILE');
const _expect = String.fromEnvironment('EXPECT');
const _send = bool.fromEnvironment('SEND');
const _shots = String.fromEnvironment('SHOTS_DIR');

/// What the suite writes, so what it leaves behind can be told apart.
const _mark = 'mobile_interaction_test';

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('the chat and the viewer on an iPhone', (tester) async {
    final problems = <String>[];
    final log = <String>[];
    File? notes;
    if (_shots.isNotEmpty) {
      notes = File('$_shots/notes.txt');
      await notes.create(recursive: true);
    }
    void note(String text) {
      log.add(text);
      debugPrint('[$_mark] $text');
      notes?.writeAsStringSync('$text\n', mode: FileMode.append);
    }

    var where = 'start';
    void problem(String text) {
      problems.add('$where: $text');
      note('PROBLEM $where: $text');
    }

    void check(bool ok, String text) {
      if (!ok) problem(text);
    }

    // Overflows and other errors while laying out are problems of the
    // step they happen in, not the end of the run.
    final reportError = FlutterError.onError;
    FlutterError.onError = (details) {
      FlutterError.dumpErrorToConsole(details, forceReport: true);
      final what = details.exceptionAsString().split('\n').first;
      problem('${details.library ?? 'error'}: $what');
    };
    final semantics = tester.ensureSemantics();

    Future<void> settle([int ms = 600]) async {
      final end = DateTime.now().add(Duration(milliseconds: ms));
      while (DateTime.now().isBefore(end)) {
        await tester.pump(const Duration(milliseconds: 50));
        await Future<void>.delayed(const Duration(milliseconds: 30));
      }
    }

    Future<bool> until(bool Function() done, {int seconds = 10}) async {
      final end = DateTime.now().add(Duration(seconds: seconds));
      while (!done()) {
        if (DateTime.now().isAfter(end)) return false;
        await settle(100);
      }
      return true;
    }

    Future<void> shot(String name) async {
      if (_shots.isEmpty) return;
      await settle();
      final view = binding.renderViews.first;
      final image = await (view.debugLayer! as OffsetLayer).toImage(
        Offset.zero & view.size * view.flutterView.devicePixelRatio,
      );
      final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
      await File('$_shots/$name.png').writeAsBytes(bytes!.buffer.asUint8List());
    }

    Future<void> simShot(String name) async {
      if (_shots.isEmpty) return;
      await settle();
      final request = File('$_shots/request');
      await request.writeAsString(name);
      final answered = await until(() => !request.existsSync(), seconds: 8);
      if (!answered) {
        if (request.existsSync()) request.deleteSync();
        note('no Simulator screenshot for $name');
      }
    }

    // The agent the suite works with: the selected agent is the backend's,
    // shared by every device, so another one can change it meanwhile.
    Future<void> Function()? reselect;
    String? Function()? switchedAway;

    Future<void> step(String name, Future<void> Function() body) async {
      where = name;
      note('— $name');
      await reselect?.call();
      try {
        await body();
      } catch (error, stack) {
        problem('stopped: ${'$error'.split('\n').first}');
        debugPrint('$stack'.split('\n').take(8).join('\n'));
      }
      final other = switchedAway?.call();
      if (other != null) {
        problem(
          'another device selected $other meanwhile: run this suite with '
          'the dev Claktory to itself',
        );
      }
    }

    double inset() =>
        tester.view.viewInsets.bottom / tester.view.devicePixelRatio;
    Size screen() => tester.view.physicalSize / tester.view.devicePixelRatio;

    /// Where the keyboard starts, from the top (the screen's bottom without).
    double keyboardTop() => screen().height - inset();

    /// Waits for the keyboard to finish coming up; whether it did.
    Future<bool> keyboardUp() async {
      var last = -1.0;
      return until(() {
        final now = inset();
        final still = now > 100 && now == last;
        last = now;
        return still;
      }, seconds: 8);
    }

    /// Text under 4.5:1 because a state fades it on purpose (a queued
    /// message), which WCAG exempts (IDENTITY.md, Phone): listed at the
    /// end, not failed.
    final exempted = <String>{};

    /// The contrast of the text in view, as painted: each passage's
    /// background (its most common color) against its ink (the color in it
    /// farthest from that). Under 4.5:1 (3:1 for 18pt, or 14pt bold) fails,
    /// faint text too, since a phone's is read; faded on purpose, it's
    /// [exempted]. Flutter's own textContrastGuideline
    /// reads the wrong pixels in a live app at 3×, so this one reads them
    /// at the screen's.
    Future<void> contrast(String screen) async {
      final view = binding.renderViews.first;
      final ratio = view.flutterView.devicePixelRatio;
      final image = await (view.debugLayer! as OffsetLayer).toImage(
        Offset.zero & view.size * ratio,
      );
      final bytes = (await image.toByteData())!;
      int pixel(int x, int y) => bytes.getUint32((y * image.width + x) * 4);
      double luminance(int rgba) => Color.fromARGB(
        255,
        rgba >> 24 & 0xff,
        rgba >> 16 & 0xff,
        rgba >> 8 & 0xff,
      ).computeLuminance();
      final seen = <String>{};
      for (final element in find.byType(RichText).hitTestable().evaluate()) {
        final box = element.renderObject! as RenderBox;
        if (!box.attached || !box.hasSize) continue;
        final rect = box.localToGlobal(Offset.zero) & box.size;
        final words = (element.widget as RichText).text.toPlainText().trim();
        if (words.isEmpty || !seen.add(words)) continue;
        if (rect.width < 4 ||
            rect.height < 4 ||
            rect.left < 0 ||
            rect.top < 0 ||
            rect.right > view.size.width ||
            rect.bottom > keyboardTop()) {
          continue;
        }
        final counts = <int, int>{};
        var samples = 0;
        for (
          var y = (rect.top * ratio).ceil();
          y < rect.bottom * ratio && y < image.height;
          y++
        ) {
          for (
            var x = (rect.left * ratio).ceil();
            x < rect.right * ratio && x < image.width;
            x += 2
          ) {
            final color = pixel(x, y) | 0xff;
            counts[color] = (counts[color] ?? 0) + 1;
            samples++;
          }
        }
        if (samples == 0) continue;
        final background = counts.entries
            .reduce((a, b) => a.value >= b.value ? a : b)
            .key;
        final back = luminance(background);
        var best = 1.0;
        for (final entry in counts.entries) {
          if (entry.value < samples * 0.004) continue;
          final ink = luminance(entry.key);
          final light = ink > back ? ink : back;
          final dark = ink > back ? back : ink;
          final found = (light + 0.05) / (dark + 0.05);
          if (found > best) best = found;
        }
        final style = (element.widget as RichText).text.style;
        final size = (element.widget as RichText).textScaler.scale(
          style?.fontSize ?? 14,
        );
        final bold = (style?.fontWeight?.value ?? 400) >= 600;
        final needed = size >= 18 || (bold && size >= 14) ? 3.0 : 4.5;
        if (best >= needed) continue;
        var faded = false;
        element.visitAncestorElements((ancestor) {
          final widget = ancestor.widget;
          if ((widget is Opacity && widget.opacity < 1) ||
              (widget is AnimatedOpacity && widget.opacity < 1)) {
            faded = true;
          }
          return !faded;
        });
        final short = words.length > 40 ? '${words.substring(0, 40)}…' : words;
        final what =
            '"$short" (${size.round()}pt) at '
            '${best.toStringAsFixed(2)}:1';
        if (faded) {
          exempted.add(what);
        } else {
          problem('$screen, contrast: $what, under $needed:1');
        }
      }
    }

    /// The accessibility checks, on what's on screen now.
    Future<void> guidelines(String screen) async {
      for (final (name, guideline) in [
        ('44pt targets', iOSTapTargetGuideline),
        ('labels', labeledTapTargetGuideline),
      ]) {
        final result = await guideline.evaluate(tester);
        if (!result.passed) {
          problem('$screen, $name: ${result.reason?.trim()}');
        }
      }
      await contrast(screen);
    }

    /// What the Simulator says about the step's surroundings.
    void environment() {
      final context = tester.element(find.byType(AgentView).first);
      final media = MediaQuery.of(context);
      final dark = media.platformBrightness == Brightness.dark;
      final scale = media.textScaler.scale(17) / 17;
      note(
        'screen ${screen().width.round()}×${screen().height.round()}pt, '
        'text ${scale.toStringAsFixed(2)}×, ${dark ? 'dark' : 'light'}, '
        'safe area ${media.padding.top.round()}/${media.padding.bottom.round()}',
      );
      final expected = _expect.split(',').map((e) => e.trim()).toSet();
      if (expected.contains('dark')) check(dark, 'not dark as expected');
      if (expected.contains('light')) check(!dark, 'not light as expected');
      if (expected.contains('large')) {
        check(scale > 1.5, 'text at $scale×, not large as expected');
      }
    }

    /// Sends what a key of the software keyboard does to the field with
    /// focus (Return's action), as iOS does.
    Future<void> keyboardAction(TextInputAction action) async {
      await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
        SystemChannels.textInput.name,
        SystemChannels.textInput.codec.encodeMethodCall(
          MethodCall('TextInputClient.performAction', <dynamic>[
            -1, // Any field (a debug build's own exception).
            action.toString(),
          ]),
        ),
        (_) {},
      );
      await settle();
    }

    await app.main();
    final started = await until(
      () => find.byType(AgentView).evaluate().isNotEmpty,
      seconds: 30,
    );
    if (!started) {
      FlutterError.onError = reportError;
      semantics.dispose();
      fail(
        'no conversation came up: is the Simulator paired with the dev '
        'Claktory, and is it running?',
      );
    }
    await settle(2000);
    final scope = AppScope.scope(tester.element(find.byType(AgentView).first));
    final backend = scope.backend;
    final agents = backend.workspace.workspace.agents;
    final agent = agents.where((a) => a.name == _agentName).firstOrNull;
    if (agent == null) {
      FlutterError.onError = reportError;
      semantics.dispose();
      fail('no agent named $_agentName in the dev Claktory');
    }
    String? away;
    void watch() {
      final selected = backend.workspace.selectedAgent;
      if (selected != null && selected.id != agent.id) away ??= selected.name;
    }

    reselect = () async {
      backend.workspace.removeListener(watch);
      if (backend.workspace.selectedAgent?.id != agent.id) {
        await backend.selectAgent(agent.id);
        await until(
          () =>
              backend.workspace.selectedAgent?.id == agent.id &&
              backend.transcript.agentId == agent.id,
        );
        await settle(1500);
      }
      away = null;
      backend.workspace.addListener(watch);
    };
    switchedAway = () => away;
    addTearDown(() => backend.workspace.removeListener(watch));
    await reselect();
    final transcript = backend.transcript;
    final layout = agent.hasLayout() ? agent.layout : null;

    Finder composerField() => find.descendant(
      of: find.byType(PhoneComposerRow),
      matching: find.byType(EditableText),
    );
    Finder inTranscript(Finder finder) =>
        find.descendant(of: find.byType(TranscriptView), matching: finder);
    Finder transcriptScroll() => inTranscript(find.byType(Scrollable)).first;

    Rect globalCaret(EditableTextState state) {
      final box = state.renderEditable;
      final caret = box.getLocalRectForCaret(box.selection!.extent);
      return MatrixUtils.transformRect(box.getTransformTo(null), caret);
    }

    // Entries played into the conversation take numbers far past the
    // backend's, so a real one meanwhile can't take their place.
    var seq =
        (transcript.length == 0 ? 0 : transcript[transcript.length - 1].seq) +
        100000;
    Entry say(String text) => Entry(
      seq: Int64(++seq),
      timestampMs: Int64(DateTime.now().millisecondsSinceEpoch),
      assistant: AssistantText(text: text),
    );
    void play(Entry entry) =>
        transcript.update(EntryUpdate(agentId: agent.id, entry: entry));
    void stream(Entry entry, String more) => transcript.delta(
      EntryDelta(agentId: agent.id, seq: entry.seq, text: more),
    );

    try {
      // 0. The conversation as it opens.
      await step('the conversation', () async {
        environment();
        // Enough to read back through, whatever the agent has said.
        for (var n = 1; n <= 24; n++) {
          play(
            say(
              'Filler $n. An earlier message to read back through while '
              'the conversation goes on below it, long enough to wrap.',
            ),
          );
        }
        await settle(1500);
        await shot('a1-conversation');
        await guidelines('the conversation');
      });

      // 1. The message box and the keyboard.
      await step('the message box and the keyboard', () async {
        final field = composerField();
        check(field.evaluate().isNotEmpty, 'no message box');
        final editable = tester.widget<EditableText>(field);
        check(
          !editable.focusNode.hasFocus,
          'the message box took focus on its own',
        );
        check(
          editable.textInputAction == TextInputAction.newline,
          "Return doesn't start a new line (${editable.textInputAction})",
        );
        check(
          editable.keyboardType == TextInputType.multiline,
          'not a multiline keyboard (${editable.keyboardType})',
        );
        // The draft this phone keeps for the agent, put back after.
        final draft = editable.controller.text;
        if (draft.isEmpty) {
          final empty = tester.getSize(find.byType(PhoneComposerRow)).height;
          // One line of 17pt at the text size, its padding, a little more.
          final line = MediaQuery.textScalerOf(
            tester.element(find.byType(PhoneComposerRow)),
          ).scale(17);
          check(
            empty < math.max(52, line * 1.4 + 24),
            'the empty message box is ${empty.round()}pt tall, not one line',
          );
        }
        await tester.showKeyboard(field);
        final up = await keyboardUp();
        check(
          up,
          "the keyboard never came up (minimized for the Mac's? see the header)",
        );
        note('keyboard: ${inset().round()}pt');
        await tester.enterText(field, 'First line\nSecond line\nThird line');
        await settle();
        final send = find.bySemanticsLabel('Send');
        check(send.evaluate().isNotEmpty, 'no Send');
        final sendRect = tester.getRect(send.last);
        check(
          sendRect.bottom <= keyboardTop() + 0.5,
          'Send ($sendRect) under the keyboard (top ${keyboardTop()})',
        );
        check(
          sendRect.width >= 44 && sendRect.height >= 44,
          'Send is ${sendRect.size}, under 44pt',
        );
        final state = tester.state<EditableTextState>(field);
        final caret = globalCaret(state);
        check(
          caret.bottom <= keyboardTop() + 0.5,
          'the caret ($caret) under the keyboard',
        );
        await shot('b1-draft');
        await simShot('b1-draft-keyboard');
        await guidelines('the message box, keyboard up');

        // A long draft: the box stops growing and scrolls, the caret and
        // Send stay above the keyboard, some of the conversation shows.
        await tester.enterText(
          field,
          [for (var n = 1; n <= 40; n++) 'Line $n of a long draft.'].join('\n'),
        );
        await settle();
        final row = tester.widget<PhoneComposerRow>(
          find.byType(PhoneComposerRow),
        );
        final box = tester.getRect(field);
        final cap = row.maxFieldHeight ?? double.infinity;
        check(
          box.height <= cap + 20,
          'the box grew to ${box.height.round()}pt, past its cap '
          '${cap.round()}pt',
        );
        final room =
            keyboardTop() -
            MediaQuery.paddingOf(tester.element(find.byType(AgentView).first))
                .top;
        check(
          cap <= room * 0.3 + 1 || cap <= 44,
          'the cap (${cap.round()}pt) is more than 30% of the '
          '${room.round()}pt above the keyboard',
        );
        final inner = find.descendant(
          of: field,
          matching: find.byType(Scrollable),
        );
        final scrolls =
            inner.evaluate().isNotEmpty &&
            tester
                    .state<ScrollableState>(inner.first)
                    .position
                    .maxScrollExtent >
                0;
        check(scrolls, "a long draft doesn't scroll in the box");
        final end = globalCaret(tester.state<EditableTextState>(field));
        check(
          end.bottom <= keyboardTop() + 0.5 && end.bottom <= box.bottom + 1,
          'the caret at the end ($end) not in view (box $box)',
        );
        check(
          tester.getRect(send.last).bottom <= keyboardTop() + 0.5,
          'Send under the keyboard with a long draft',
        );
        final chat = tester.getRect(find.byType(TranscriptView));
        final shown = chat.height;
        check(
          shown >= 88,
          'only ${shown.round()}pt of the conversation shows with a long '
          'draft',
        );
        await shot('b2-long-draft');
        await simShot('b2-long-draft-keyboard');

        // Paste, select and copy, through the iPhone's own pasteboard.
        await tester.enterText(field, '');
        await settle();
        const pasted = 'Pasted from the clipboard ($_mark).';
        await Clipboard.setData(const ClipboardData(text: pasted));
        await tester.longPress(field);
        await settle(900);
        final menu =
            find.byType(SystemContextMenu).evaluate().isNotEmpty ||
            find
                .byType(CupertinoAdaptiveTextSelectionToolbar)
                .evaluate()
                .isNotEmpty;
        check(menu, 'touch and hold in the box shows no edit menu');
        await simShot('b3-edit-menu');
        final editing = tester.state<EditableTextState>(field);
        editing.pasteText(SelectionChangedCause.toolbar);
        await settle();
        check(
          editing.textEditingValue.text == pasted,
          'Paste put "${editing.textEditingValue.text}"',
        );
        editing.selectAll(SelectionChangedCause.toolbar);
        await settle();
        final all = editing.textEditingValue.selection;
        check(
          all.start == 0 && all.end == pasted.length,
          'Select All selected $all',
        );
        await Clipboard.setData(const ClipboardData(text: ''));
        editing.copySelection(SelectionChangedCause.toolbar);
        await settle();
        final copied = await Clipboard.getData(Clipboard.kTextPlain);
        check(copied?.text == pasted, 'Copy put "${copied?.text}"');
        editing.hideToolbar();

        if (_send) {
          await tester.enterText(field, 'Only say OK. ($_mark)');
          await settle();
          await tester.tap(find.bySemanticsLabel('Send').last);
          await settle(1500);
          final after = tester.widget<EditableText>(field);
          check(after.controller.text.isEmpty, 'the box kept what was sent');
          check(after.focusNode.hasFocus, 'sending took the focus away');
          check(inset() > 100, 'sending put the keyboard away');
        }

        // Streamed words don't take the focus or the keyboard away.
        await tester.enterText(field, 'Still writing');
        await settle();
        final reply = say('A reply streaming while you write');
        play(reply);
        for (var n = 0; n < 12; n++) {
          stream(reply, ', and more of it');
          await settle(120);
        }
        final still = tester.widget<EditableText>(field);
        check(still.focusNode.hasFocus, 'streaming took the focus');
        check(inset() > 100, 'streaming put the keyboard away');
        check(
          still.controller.text == 'Still writing',
          'streaming changed the draft to "${still.controller.text}"',
        );
        check(
          globalCaret(tester.state<EditableTextState>(field)).bottom <=
              keyboardTop() + 0.5,
          'the caret went under the keyboard while streaming',
        );

        await tester.enterText(field, draft);

        // The keyboard tucked away: a tap on the conversation, or
        // scrolling it, as Messages does.
        await tester.tapAt(tester.getRect(transcriptScroll()).center);
        check(
          await until(() => inset() == 0, seconds: 5),
          "a tap on the conversation didn't put the keyboard away",
        );
        await tester.showKeyboard(field);
        await keyboardUp();
        await tester.drag(transcriptScroll(), const Offset(0, 120));
        check(
          await until(() => inset() == 0, seconds: 5),
          "scrolling the conversation didn't put the keyboard away",
        );
        FocusManager.instance.primaryFocus?.unfocus();
        await until(() => inset() == 0, seconds: 5);
        await settle();
      });

      // 2. Streaming while reading earlier messages.
      await step('streaming while you read', () async {
        final scroll = transcriptScroll();
        final position = tester.state<ScrollableState>(scroll).position;
        note(
          'conversation: ${transcript.length} entries, '
          '${position.maxScrollExtent.round()}pt to scroll',
        );
        // Back through what's been said, as a finger does.
        bool earlierInView() {
          final view = tester.getRect(scroll);
          return inTranscript(find.textContaining('Filler', findRichText: true))
              .evaluate()
              .any((element) {
                final box = element.renderObject! as RenderBox;
                if (!box.attached || !box.hasSize) return false;
                final rect = box.localToGlobal(Offset.zero) & box.size;
                return rect.overlaps(view);
              });
        }

        for (var n = 0; n < 20 && (n < 3 || !earlierInView()); n++) {
          await tester.drag(scroll, const Offset(0, 300));
          await settle(400);
        }
        await settle(800);
        final view = tester.getRect(scroll);
        // What's read: the passage nearest the middle of the view.
        String? anchorText;
        double? anchorTop;
        var best = double.infinity;
        for (final element in inTranscript(find.byType(RichText)).evaluate()) {
          final box = element.renderObject! as RenderBox;
          if (!box.attached || !box.hasSize) continue;
          final rect = box.localToGlobal(Offset.zero) & box.size;
          // In view, if only in part (at the largest text, one message
          // can be taller than the view).
          if (rect.bottom < view.top || rect.top > view.bottom) continue;
          final text = (element.widget as RichText).text.toPlainText();
          if (!text.startsWith('Filler')) continue;
          final distance = (rect.center.dy - view.center.dy).abs();
          if (distance < best) {
            best = distance;
            anchorText = text;
            anchorTop = rect.top;
          }
        }
        check(anchorText != null, 'nothing earlier to read in view');
        if (anchorText == null) return;
        note(
          'reading "${anchorText.substring(0, 9)}" at ${anchorTop!.round()}',
        );
        final latest = find
            .bySemanticsLabel('Go to the latest message')
            .hitTestable();
        check(latest.evaluate().isNotEmpty, 'no Latest while reading back');

        // A reply starts and streams in, a word at a time.
        final reply = say('Streaming check: ');
        play(reply);
        for (var n = 0; n < 30; n++) {
          stream(reply, n % 6 == 5 ? 'paragraph.\n\n' : 'word $n ');
          await settle(80);
        }
        await settle(500);
        double top() {
          final found = inTranscript(
            find.byWidgetPredicate(
              (w) => w is RichText && w.text.toPlainText() == anchorText,
            ),
          );
          return found.evaluate().isEmpty
              ? double.nan
              : tester.getRect(found.first).top;
        }

        final moved = (top() - anchorTop).abs();
        check(
          moved <= 1,
          'what you were reading moved ${moved.toStringAsFixed(1)}pt',
        );
        final count = find
            .bySemanticsLabel(RegExp(r'^\d+ new messages?, go to the latest$'))
            .hitTestable();
        check(count.evaluate().isNotEmpty, 'Latest shows no count of new');
        if (count.evaluate().isNotEmpty) {
          final size = tester.getSize(count.first);
          check(size.height >= 44, 'Latest is ${size.height}pt tall');
        }
        await shot('c1-reading-while-streaming');
        await guidelines('reading back, Latest showing');

        await tester.tap(count.evaluate().isNotEmpty ? count.first : latest);
        await settle(1500);
        // The end: the reply, then anything still queued for the agent.
        final end = tester.state<ScrollableState>(transcriptScroll()).position;
        check(
          (end.maxScrollExtent - end.pixels).abs() < 2,
          "Latest didn't go to the end "
          '(${end.pixels.round()} of ${end.maxScrollExtent.round()})',
        );
        check(
          find
              .bySemanticsLabel(RegExp(r'(latest message|go to the latest)$'))
              .hitTestable()
              .evaluate()
              .isEmpty,
          'Latest still shows at the end',
        );

        // At the end, the view follows what streams in.
        for (var n = 0; n < 20; n++) {
          stream(reply, 'more $n ');
          if (n % 5 == 4) stream(reply, '\n\n');
          await settle(80);
        }
        await settle(800);
        final following = tester
            .state<ScrollableState>(transcriptScroll())
            .position;
        check(
          (following.maxScrollExtent - following.pixels).abs() < 2,
          "at the end, the view didn't follow the stream "
          '(${following.pixels.round()} of '
          '${following.maxScrollExtent.round()})',
        );
        await shot('c2-following');
      });

      // 3. Touch and hold versus selecting.
      await step('touch and hold, and selecting', () async {
        const held = 'A message to touch and hold, from $_mark.';
        play(say(held));
        await settle(1200);
        // What's queued for the agent comes after it, and can push it up
        // out of view: back up to it, then all of it in the middle.
        final built = inTranscript(
          find.textContaining(held, findRichText: true),
        );
        for (var n = 0; n < 30 && built.evaluate().isEmpty; n++) {
          await tester.drag(transcriptScroll(), const Offset(0, 200));
          await settle(400);
        }
        if (built.evaluate().isNotEmpty) {
          await Scrollable.ensureVisible(
            tester.element(built.first),
            alignment: 0.5,
          );
          await settle(800);
        }
        final message = built;
        final shown =
            message.evaluate().isNotEmpty &&
            tester
                .getRect(message.first)
                .overlaps(tester.getRect(transcriptScroll()));
        check(shown, 'no message in view to hold');
        if (!shown) return;
        // A word in the part of it in view, wherever the view has gone.
        Offset inView() =>
            tester
                .getRect(built.first)
                .intersect(tester.getRect(transcriptScroll()))
                .topLeft +
            const Offset(30, 20);
        final words = (tester.widget(message.first) as RichText).text
            .toPlainText();

        // Its actions without holding, for VoiceOver.
        final node = tester.getSemantics(message.first);
        final labels = <String>{};
        for (SemanticsNode? n = node; n != null; n = n.parent) {
          for (final id
              in n.getSemanticsData().customSemanticsActionIds ??
                  const <int>[]) {
            final action = CustomSemanticsAction.getAction(id);
            if (action?.label != null) labels.add(action!.label!);
          }
        }
        check(
          labels.containsAll({'Copy', 'Select text'}),
          "a message's actions for VoiceOver: $labels",
        );

        await tester.longPressAt(inView());
        await settle(800);
        check(
          find.text('Select text').evaluate().isNotEmpty &&
              find.text('Copy').evaluate().isNotEmpty,
          'touch and hold shows no Copy and Select text',
        );
        check(
          find.byType(CupertinoAdaptiveTextSelectionToolbar).evaluate().isEmpty,
          'touch and hold also started selecting',
        );
        await shot('d1-hold');
        await guidelines("a message's actions");
        await Clipboard.setData(const ClipboardData(text: ''));
        await tester.tap(find.text('Copy').last);
        await settle(800);
        final copied = await Clipboard.getData(Clipboard.kTextPlain);
        check(
          copied?.text?.trim() == words.trim(),
          'Copy put "${copied?.text}"',
        );
        check(find.text('Select text').evaluate().isEmpty, 'the sheet stayed');

        // A double tap selects a word, with the iPhone's handles and menu.
        final at = inView();
        check(
          tester.getRect(transcriptScroll()).contains(at),
          'the message to select is out of view',
        );
        await tester.tapAt(at);
        await tester.pump(const Duration(milliseconds: 80));
        await tester.tapAt(at);
        await settle(900);
        final selecting = find
            .byType(CupertinoAdaptiveTextSelectionToolbar)
            .evaluate()
            .isNotEmpty;
        check(selecting, 'a double tap showed no edit menu');
        await shot('d2-selected');
        // Once selecting, a hold is the selection's, not the message's.
        await tester.longPressAt(at);
        await settle(900);
        check(
          find.text('Select text').evaluate().isEmpty,
          "touch and hold on a selection opened the message's actions",
        );
        await shot('d3-hold-on-selection');
        // Done selecting: a tap elsewhere.
        await tester.tapAt(
          tester
                  .getRect(built.first)
                  .intersect(tester.getRect(transcriptScroll()))
                  .bottomRight -
              const Offset(12, 12),
        );
        await settle(600);
        final regions = inTranscript(find.byType(SelectableRegion));
        if (find
                .byType(CupertinoAdaptiveTextSelectionToolbar)
                .evaluate()
                .isNotEmpty &&
            regions.evaluate().isNotEmpty) {
          note('a tap left the selection; cleared it');
          tester.state<SelectableRegionState>(regions.first).clearSelection();
          await settle(600);
        }
        if (find.text('Select text').evaluate().isNotEmpty) {
          await tester.tap(find.text('Cancel').last);
          await settle();
        }

        // Select text: the message on its own, any part of it selectable.
        await Scrollable.ensureVisible(
          tester.element(built.first),
          alignment: 0.5,
        );
        await settle(800);
        await tester.longPressAt(inView());
        await settle(800);
        await tester.tap(find.text('Select text').last);
        await settle(900);
        final sheet = find.byType(PhoneSheet);
        check(sheet.evaluate().isNotEmpty, 'Select text opened no sheet');
        if (sheet.evaluate().isNotEmpty) {
          final inside = find.descendant(
            of: sheet.last,
            matching: find.textContaining(held, findRichText: true),
          );
          await tester.longPress(inside.first);
          await settle(900);
          check(
            find
                .byType(CupertinoAdaptiveTextSelectionToolbar)
                .evaluate()
                .isNotEmpty,
            'touch and hold in Select text showed no edit menu',
          );
          await shot('d4-select-text');
          await tester.tap(find.bySemanticsLabel('Close').last);
          await settle(800);
          check(
            find.byType(PhoneSheet).evaluate().isEmpty,
            "Select text's sheet didn't close",
          );
        }
      });

      // 4. Comments, from selection to resolved.
      if (_file.isEmpty) {
        note('no VIEW_FILE: comments skipped');
      } else {
        await step('comments end to end', () async {
          final name = _file.split('/').last;
          await backend.setLayout(
            agent.id,
            ViewNode(
              id: _mark,
              split: ViewSplit(
                axis: ViewAxis.VIEW_AXIS_ROW,
                children: [
                  ViewNode(
                    id: 'chat',
                    leaf: ViewLeaf(kind: 'chat'),
                  ),
                  ViewNode(
                    id: '$_mark-file',
                    leaf: ViewLeaf(kind: 'text', path: _file, title: name),
                  ),
                ],
              ),
            ),
          );
          await settle(1200);
          await tester.tap(
            find.bySemanticsLabel(RegExp('^More options for ')).first,
          );
          await settle(900);
          await tester.tap(find.text(name).last);
          await settle(2500);
          await guidelines('the file');
          await shot('e1-file');
          List<Thread> threads() =>
              backend.threads.of(_file)?.threads ?? const [];
          final before = {for (final t in threads()) t.id};

          // A word of a paragraph, selected as a finger does.
          // One whose start is in view: at the largest text, one
          // paragraph can be taller than the screen.
          final starts = [
            for (final element
                in find
                    .descendant(
                      of: find.byType(TextViewer),
                      matching: find.byWidgetPredicate(
                        (w) =>
                            w is RichText && w.text.toPlainText().length > 80,
                      ),
                    )
                    .evaluate())
              if (element.renderObject case final RenderBox box
                  when box.attached && box.hasSize)
                box.localToGlobal(const Offset(60, 20)),
          ].where((at) => at.dy > 120 && at.dy < keyboardTop() - 60);
          check(starts.isNotEmpty, 'no paragraph in view');
          await tester.longPressAt(starts.first);
          await settle(900);
          final menu = find.byType(CupertinoAdaptiveTextSelectionToolbar);
          check(menu.evaluate().isNotEmpty, 'selecting showed no edit menu');
          final items = menu.evaluate().isEmpty
              ? const <String>[]
              : [
                  for (final item
                      in tester
                              .widget<CupertinoAdaptiveTextSelectionToolbar>(
                                menu.first,
                              )
                              .buttonItems ??
                          const <ContextMenuButtonItem>[])
                    item.label ?? item.type.name,
                ];
          check(
            items.contains('Comment') && items.contains('Suggest'),
            'the edit menu has $items',
          );
          await shot('e2-selected');
          final comment = find.descendant(
            of: menu.first,
            matching: find.text('Comment'),
          );
          if (comment.evaluate().isNotEmpty) {
            await tester.tap(comment);
          } else {
            // At the largest text it's on the menu's next page (›).
            note("Comment is on the edit menu's next page");
            tester
                .widget<CupertinoAdaptiveTextSelectionToolbar>(menu.first)
                .buttonItems!
                .firstWhere((item) => item.label == 'Comment')
                .onPressed!();
          }
          await settle(1200);

          // The editor: above the keyboard, Return sends.
          final sheet = find.byType(PhoneSheet);
          check(sheet.evaluate().isNotEmpty, 'Comment opened no sheet');
          final field = find.descendant(
            of: sheet.last,
            matching: find.byType(EditableText),
          );
          final up = await keyboardUp();
          check(up, "the comment's keyboard never came up");
          check(
            tester.widget<EditableText>(field.first).focusNode.hasFocus,
            'the comment field has no focus',
          );
          await tester.enterText(field.first, 'A comment from $_mark.');
          await settle();
          check(
            tester.getRect(field.first).bottom <= keyboardTop() + 0.5,
            'the comment field under the keyboard',
          );
          await shot('e3-editor');
          await simShot('e3-editor-keyboard');
          await guidelines('writing a comment');
          await keyboardAction(TextInputAction.send);
          final made = await until(
            () => threads().any((t) => !before.contains(t.id)),
          );
          check(made, 'Return sent no comment');
          if (!made) return;
          final thread = threads().firstWhere((t) => !before.contains(t.id));
          await until(() => find.byType(PhoneSheet).evaluate().isEmpty);
          check(
            find.byType(PhoneSheet).evaluate().isEmpty,
            "the editor didn't close",
          );

          try {
            // Its marker, a finger's size, opens the thread in a sheet.
            final marker = find.bySemanticsLabel('Comment for ${agent.name}');
            await until(() => marker.evaluate().isNotEmpty);
            check(marker.evaluate().isNotEmpty, 'no marker for the comment');
            if (marker.evaluate().isEmpty) return;
            final size = tester.getSize(marker.last);
            check(size.width >= 44 && size.height >= 44, 'the marker is $size');
            await tester.tap(marker.last);
            await settle(900);
            check(
              find
                  .textContaining('A comment from $_mark')
                  .evaluate()
                  .isNotEmpty,
              "the thread's sheet doesn't show the comment",
            );
            await shot('e4-thread');
            await guidelines("a comment's thread");

            // A reply, in its own sheet; Return sends it.
            await tester.ensureVisible(find.text('Reply').last);
            await settle(500);
            await tester.tap(find.text('Reply').last);
            await settle(1200);
            final reply = find.descendant(
              of: find.byType(PhoneSheet).last,
              matching: find.byType(EditableText),
            );
            await keyboardUp();
            await tester.enterText(reply.first, 'A reply from $_mark.');
            await settle();
            check(
              tester.getRect(reply.first).bottom <= keyboardTop() + 0.5,
              'the reply field under the keyboard',
            );
            await keyboardAction(TextInputAction.send);
            final replied = await until(
              () => threads().any(
                (t) =>
                    t.id == thread.id &&
                    t.comments.any((c) => c.body.contains('A reply from')),
              ),
            );
            check(replied, 'Return sent no reply');
            await settle(900);

            // Resolved from the sheet.
            final resolve = find.text('Resolve');
            if (resolve.evaluate().isEmpty) {
              await tester.tap(marker.last);
              await settle(900);
            }
            check(resolve.evaluate().isNotEmpty, 'no Resolve');
            if (resolve.evaluate().isNotEmpty) {
              await tester.ensureVisible(resolve.last);
              await settle(500);
              await tester.tap(resolve.last);
              final resolved = await until(
                () => threads().any(
                  (t) =>
                      t.id == thread.id &&
                      t.status == ThreadStatus.THREAD_STATUS_RESOLVED,
                ),
              );
              check(resolved, 'Resolve resolved nothing');
            }
            await shot('e5-resolved');
            while (find.byType(PhoneSheet).evaluate().isNotEmpty) {
              final close = find.bySemanticsLabel('Close');
              if (close.evaluate().isEmpty) break;
              await tester.tap(close.last);
              await settle(700);
            }
          } finally {
            // Gone again, whatever happened.
            final held = threads().where((t) => t.id == thread.id);
            for (final comment
                in held.isEmpty ? thread.comments : held.first.comments) {
              await backend.deleteComment(comment.id);
            }
            await until(() => threads().every((t) => t.id != thread.id));
            if (threads().any((t) => t.id == thread.id)) {
              note("the suite's comment is still on $name");
            }
          }

          // Back: the file goes from this phone, the layout stays.
          final kept = backend.workspace.workspace.agents
              .firstWhere((a) => a.id == agent.id)
              .layout;
          await tester.tap(find.bySemanticsLabel('Back').last);
          await settle(1200);
          final after = backend.workspace.workspace.agents
              .firstWhere((a) => a.id == agent.id)
              .layout;
          check(after == kept, 'Back changed the layout');
          check(
            find.byType(AgentView).hitTestable().evaluate().isNotEmpty,
            "Back didn't come back to the conversation",
          );
        });
      }
    } finally {
      where = 'cleaning up';
      if (_file.isNotEmpty) {
        await backend.setLayout(agent.id, layout);
      }
      await settle(800);
      FlutterError.onError = reportError;
      semantics.dispose();
    }

    if (exempted.isNotEmpty) {
      note('Exempted: faded on purpose for a state, under 4.5:1:');
      for (final line in exempted) {
        note('  $line');
      }
    }
    note(
      problems.isEmpty
          ? 'passed'
          : '${problems.length} problem${problems.length == 1 ? '' : 's'}',
    );
    if (problems.isNotEmpty) fail(problems.join('\n'));
  });
}
