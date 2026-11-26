// A review an agent asks of a PDF, on an iPad (ipad/document_review.dart):
// the document pane's foot says who asks and what, and how many pages are
// drawn on, with the answer field always open; Return, Approve or Send,
// and Dismiss send each drawn page, whole with its ink, as an upload to
// the agent, named in the answer, with what's typed. Answered, the pane
// closes.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/attachments.dart' show LocalFile;
import 'package:factory_app/card_answer.dart' show drawingGoesLine, insteadLine;
import 'package:factory_app/card_files.dart' show cardFilesKey;
import 'package:factory_app/controls.dart' show Pressable;
import 'package:factory_app/dictation.dart';
import 'package:factory_app/ipad/document_review.dart';
import 'package:factory_app/ipad/ink_surface.dart';
import 'package:factory_app/outbox.dart' show decisionWaiting;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart'
    show
        AskPersonState,
        AskThreadMessage,
        SetLayout,
        SpeechModel,
        SpeechModelState;
import 'package:factory_app/recorder.dart';
import 'package:factory_app/voice.dart' show voiceMessagesOn;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:protobuf/protobuf.dart' show GeneratedMessageGenericExtensions;

import '../harness.dart' show Harness;
import '../voice_test.dart' show FakeRecorder, io;
import 'as_ipad.dart';
import 'document_native.dart';

/// The review's answer field.
final _field = find.descendant(
  of: find.byKey(const ValueKey('review-answer')),
  matching: find.byType(EditableText),
);

void main() {
  testWidgets('the pane says who asks and what; Approve sends each drawn '
      'page with the comment', (tester) async {
    asIPad();
    final native = DocumentNative();
    final harness = await showDocument(tester);
    expect(find.text('Builder asks you to review this'), findsOneWidget);
    expect(find.text('Check the figures on page 3'), findsOneWidget);
    expect(find.text('The totals look off.'), findsOneWidget);
    // Nothing drawn: nothing said about drawings.
    expect(find.textContaining('go with it'), findsNothing);
    expect(find.text('Reply to Builder'), findsOneWidget);

    await native.drew(3, 2);
    await native.drew(1, 1);
    await native.drew(5, 0); // drawn, then all erased
    await settle(tester);
    // The line beside the buttons: the drawings go with either.
    expect(find.text('Your drawings on 2 pages go with it.'), findsOneWidget);
    expect(find.text('Approve'), findsOneWidget);
    expect(find.text('Dismiss'), findsOneWidget);
    await tester.enterText(_field, 'Looks right');
    await tester.pump();
    // A comment is a reply (#370): Submit alone, saying so.
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Dismiss'), findsNothing);
    expect(
      find.text(
        'Submit sends your words instead of approving. '
        'Your drawings on 2 pages go with it.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Submit'));
    await settle(tester);

    // The pages in order, whole (no region), with their ink.
    final exports = [
      for (final call in native.calls)
        if (call.method == 'exportComposite')
          (call.arguments as Map).cast<String, Object?>(),
    ];
    expect([for (final e in exports) e['page_1based']], [1, 3]);
    expect(exports.first.containsKey('region'), isFalse);
    // The page's layer as the backend keeps it.
    final kept = native.ink.layers.values.singleWhere(
      (l) => l.space.page1based == 1,
    );
    expect(
      (exports.first['ink_layer_id'], exports.first['layer_revision']),
      (kept.id, kept.revision),
    );
    expect(exports.first['include_ink'], isTrue);

    final uploads = sentOf(harness, (m) => m.hasStartUpload());
    expect(
      [for (final u in uploads) u.startUpload.name],
      ['paper-p1.png', 'paper-p3.png'],
    );
    expect(uploads.every((u) => u.startUpload.agentId == 'a1'), isTrue);
    expect(uploads.first.startUpload.mime, 'image/png');

    final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
    expect(answer.seq, Int64(7));
    expect(answer.text, 'Looks right');
    expect(answer.reply, isTrue);
    // An older Claktory reads no yes in it.
    expect(answer.dismiss, isTrue);
    expect(
      [for (final a in answer.attachments) a.name],
      ['paper-p1.png', 'paper-p3.png'],
    );
  });

  testWidgets('Dismiss sends the drawings too', (tester) async {
    asIPad();
    final native = DocumentNative();
    final harness = await showDocument(tester);
    await native.drew(2, 4);
    await settle(tester);
    expect(find.text(drawingGoesLine), findsOneWidget);
    // Typed and cleared: Approve and Dismiss are back.
    await tester.enterText(_field, 'Not this week');
    await tester.pump();
    expect(find.text('Dismiss'), findsNothing);
    await tester.enterText(_field, '');
    await tester.pump();
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
    expect(answer.dismiss, isTrue);
    expect(answer.reply, isFalse);
    expect(answer.text, isEmpty);
    expect([for (final a in answer.attachments) a.name], ['paper-p2.png']);
  });

  testWidgets('a reply whose drawings can\'t go up keeps its words', (
    tester,
  ) async {
    asIPad();
    final native = DocumentNative();
    readComposite = (file) async => throw StateError('gone');
    final harness = await showDocument(tester);
    await native.drew(1, 1);
    await settle(tester);
    await tester.enterText(_field, 'See the circle');
    await tester.pump();
    await tester.tap(find.text('Submit'));
    await settle(tester);
    expect(sentOf(harness, (m) => m.hasAnswerAsk()), isEmpty);
    expect(
      find.text(
        "Couldn't send your drawings, so your answer wasn't sent. Try again.",
      ),
      findsOneWidget,
    );
    expect(
      tester.widget<EditableText>(_field).controller.text,
      'See the circle',
    );
    expect(find.text('Submit'), findsOneWidget);
  });

  testWidgets('the bar stays above the home indicator', (tester) async {
    asIPad();
    DocumentNative();
    tester.view.padding = const FakeViewPadding(bottom: 40);
    addTearDown(tester.view.resetPadding);
    await showDocument(tester);
    final bar = tester.getRect(find.byKey(const ValueKey('document-review')));
    expect(bar.bottom, lessThanOrEqualTo(1024 - 20));
  });

  testWidgets('a narrow window at the largest text: Submit fits on one '
      'line in the bar', (tester) async {
    asIPad();
    DocumentNative();
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    // Half of a 1032-point-wide Split View, the tallest iPad.
    await showDocument(tester, size: const Size(516, 1366));
    // The panes are tabs at this width: the paper's.
    if (find.text('paper.pdf').evaluate().isNotEmpty) {
      await tester.tap(find.text('paper.pdf').first);
      await settle(tester);
    }
    await tester.enterText(_field, 'See page 2');
    await tester.pump();
    expect(tester.takeException(), isNull);
    final submit = find.byKey(const ValueKey('review-send'));
    expect(
      find.descendant(of: submit, matching: find.text('Submit')),
      findsOneWidget,
    );
    final bar = tester.getRect(find.byKey(const ValueKey('document-review')));
    final button = tester.getRect(submit);
    expect(button.left, greaterThanOrEqualTo(bar.left));
    expect(button.right, lessThanOrEqualTo(bar.right));
    // One line.
    final label = tester.renderObject<RenderParagraph>(
      find.descendant(of: submit, matching: find.text('Submit')),
    );
    final boxes = label.getBoxesForSelection(
      const TextSelection(baseOffset: 0, extentOffset: 6),
    );
    expect({for (final box in boxes) box.top}, hasLength(1));
  });

  testWidgets('at the largest text in a short window the page keeps its '
      'share, and what is under it scrolls', (tester) async {
    asIPad();
    DocumentNative();
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await showDocument(tester, size: const Size(1133, 560));
    // The panes are tabs at this size: the paper's.
    await tester.tap(find.text('paper.pdf'));
    await settle(tester);
    expect(tester.takeException(), isNull);
    expect(tester.getSize(find.byType(InkSurface)).height, greaterThan(100));
    // Approve is a scroll away.
    final approve = find.descendant(
      of: find.byKey(const ValueKey('document-review')),
      matching: find.text('Approve'),
    );
    await tester.ensureVisible(approve);
    await settle(tester);
    expect(tester.getRect(approve).bottom, lessThanOrEqualTo(560));
  });

  for (final (reason, lost) in [
    ('resize', true),
    ('rotate', true),
    ('hidden', false),
    ('undo', false),
  ]) {
    testWidgets('a stroke cut by "$reason" ${lost ? 'says' : "doesn't say"} '
        'it was lost', (tester) async {
      asIPad();
      final native = DocumentNative();
      await showDocument(tester, asking: false);
      await native.cut(reason);
      await settle(tester);
      expect(
        find.text(
          'Your last stroke was lost when the pane changed size. '
          'Draw it again.',
        ),
        lost ? findsOneWidget : findsNothing,
      );
    });
  }

  testWidgets('no question waiting on the file: no bar', (tester) async {
    asIPad();
    DocumentNative();
    await showDocument(tester, asking: false);
    expect(find.text('Builder asks you to review this'), findsNothing);
    expect(find.byKey(const ValueKey('document-review')), findsNothing);
  });

  testWidgets('a page that can\'t be drawn out: nothing is answered, and '
      'the bar says so', (tester) async {
    asIPad();
    final native = DocumentNative();
    readComposite = (file) async => throw StateError('gone');
    final harness = await showDocument(tester);
    await native.drew(1, 1);
    await settle(tester);
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    expect(sentOf(harness, (m) => m.hasAnswerAsk()), isEmpty);
    expect(
      find.text(
        "Couldn't send your drawings, so your answer wasn't sent. Try again.",
      ),
      findsOneWidget,
    );
    // Still there to try again.
    expect(find.text('Dismiss'), findsOneWidget);
  });

  group('handwriting into the answer', () {
    Finder offer(String text) => find.descendant(
      of: find.byKey(const ValueKey('document-review')),
      matching: find.text(text),
    );
    const all = 'Turn handwriting into your answer';
    const selected = 'Turn selected handwriting into your answer';

    List<Map<String, Object?>> reads(DocumentNative native) => [
      for (final call in native.calls)
        if (call.method == 'recognizeText')
          (call.arguments as Map).cast<String, Object?>(),
    ];

    String comment(WidgetTester tester) =>
        tester.widget<EditableText>(_field).controller.text;

    testWidgets('below iPadOS 27, or from a native that doesn\'t say: '
        'nothing at all', (tester) async {
      asIPad();
      final native = DocumentNative();
      await showDocument(tester);
      await native.drew(3, 2);
      for (final handwriting in [false, null]) {
        await native.ready(handwriting: handwriting);
        await native.lasso(3, 2);
        await settle(tester);
        expect(find.textContaining('handwriting'), findsNothing);
      }
      expect(reads(native), isEmpty);
    });

    testWidgets('nothing drawn: no offer', (tester) async {
      asIPad();
      final native = DocumentNative();
      await showDocument(tester);
      await native.ready(handwriting: true);
      await settle(tester);
      expect(find.textContaining('handwriting'), findsNothing);
    });

    testWidgets('every drawn page is read into the comment, which opens to '
        'be changed and is sent only with an answer', (tester) async {
      asIPad();
      final native = DocumentNative();
      final harness = await showDocument(tester);
      await native.ready(handwriting: true);
      await native.drew(3, 2);
      await native.drew(1, 1);
      await settle(tester);
      final reading = Completer<void>();
      native.recognize = (arguments) async {
        await reading.future;
        return {
          'available': true,
          'page_1based': arguments['page_1based'],
          'text': '  why page ${arguments['page_1based']}?\n',
        };
      };
      await tester.tap(offer(all));
      await tester.pump();
      expect(offer('Reading your handwriting…'), findsOneWidget);
      expect(offer(all), findsNothing);
      reading.complete();
      await settle(tester);

      expect(reads(native), [
        {'page_1based': 1, 'scope': 'all'},
        {'page_1based': 3, 'scope': 'all'},
      ]);
      expect(comment(tester), 'why page 1?\nwhy page 3?');
      expect(find.textContaining(insteadLine('approving')), findsOneWidget);
      expect(sentOf(harness, (m) => m.hasAnswerAsk()), isEmpty);
      // Offered again, for more.
      expect(offer(all), findsOneWidget);

      await tester.enterText(_field, '${comment(tester)} and the axis');
      await tester.pump();
      await tester.tap(find.text('Submit'));
      await settle(tester);
      final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
      expect(answer.text, 'why page 1?\nwhy page 3? and the axis');
    });

    testWidgets('the answer field and the offer are each a 44 pt target, '
        'apart', (tester) async {
      asIPad();
      final native = DocumentNative();
      await showDocument(tester);
      await native.ready(handwriting: true);
      await native.drew(1, 2);
      await settle(tester);
      Rect target(String text) => tester.getRect(
        find.ancestor(of: offer(text), matching: find.byType(Pressable)).first,
      );
      final add = tester.getRect(find.byKey(const ValueKey('review-answer')));
      final read = target(all);
      expect(add.height, greaterThanOrEqualTo(44));
      expect(read.height, greaterThanOrEqualTo(44));
      expect(add.overlaps(read), isFalse);
      expect(read.top, greaterThanOrEqualTo(add.bottom));
      // A tap at the offer's top edge (clear of the divider's grab zone)
      // is the offer's, not the field's.
      await tester.tapAt(Offset(read.left + 24, read.top + 1));
      await settle(tester);
      expect(find.textContaining(insteadLine('approving')), findsOneWidget);
      expect(comment(tester), 'why page 1?');
    });

    testWidgets('a lasso selection is read alone, on a new line after what '
        'the comment has', (tester) async {
      asIPad();
      final native = DocumentNative();
      await showDocument(tester);
      await native.ready(handwriting: true);
      await native.drew(3, 5);
      await native.drew(4, 1);
      await native.lasso(3, 2);
      await settle(tester);
      expect(offer(selected), findsOneWidget);
      expect(offer(all), findsNothing);

      await tester.enterText(_field, 'Two things:');
      await tester.tap(offer(selected));
      await settle(tester);
      expect(reads(native), [
        {'page_1based': 3, 'scope': 'selection'},
      ]);
      expect(comment(tester), 'Two things:\nwhy page 3?');

      // The lasso let go: the whole drawing again.
      await native.lasso(3, 0);
      await settle(tester);
      expect(offer(all), findsOneWidget);
    });

    testWidgets('an answer the ink changed under is dropped without a word', (
      tester,
    ) async {
      asIPad();
      final native = DocumentNative();
      await showDocument(tester);
      await native.ready(handwriting: true);
      await native.drew(2, 1);
      await settle(tester);

      // Native says so.
      native.recognize = (arguments) => {
        'available': true,
        'stale': true,
        'page_1based': 2,
        'text': null,
      };
      await tester.tap(offer(all));
      await settle(tester);
      expect(comment(tester), isEmpty);
      expect(find.textContaining('read your handwriting'), findsNothing);
      expect(find.text('No handwriting to read.'), findsNothing);

      // A stroke while it read: what it read is out of date.
      final reading = Completer<void>();
      native.recognize = (arguments) async {
        await reading.future;
        return {'available': true, 'page_1based': 2, 'text': 'old words'};
      };
      await tester.tap(offer(all));
      await tester.pump();
      await native.drew(2, 2);
      reading.complete();
      await settle(tester);
      expect(comment(tester), isEmpty);
      expect(find.textContaining('old words'), findsNothing);
      expect(offer(all), findsOneWidget);
    });

    testWidgets('no text read, or the reading failed: the bar says so', (
      tester,
    ) async {
      asIPad();
      final native = DocumentNative();
      await showDocument(tester);
      await native.ready(handwriting: true);
      await native.drew(1, 3);
      await settle(tester);

      native.recognize = (arguments) => {
        'available': true,
        'page_1based': 1,
        'text': '  ',
      };
      await tester.tap(offer(all));
      await settle(tester);
      expect(offer('No handwriting to read.'), findsOneWidget);
      expect(comment(tester), isEmpty);
      // Gone once the drawing changes.
      await native.drew(1, 4);
      await settle(tester);
      expect(offer('No handwriting to read.'), findsNothing);

      native.recognize = (arguments) =>
          throw PlatformException(code: 'recognition_failed');
      await tester.tap(offer(all));
      await settle(tester);
      expect(
        offer("Couldn't read your handwriting. Try again."),
        findsOneWidget,
      );
      expect(offer(all), findsOneWidget);
    });

    testWidgets('an iPad that turns out not to read handwriting: the offer '
        'goes, nothing said', (tester) async {
      asIPad();
      final native = DocumentNative();
      await showDocument(tester);
      await native.ready(handwriting: true);
      await native.drew(1, 3);
      await settle(tester);
      native.recognize = (arguments) => {
        'available': false,
        'reason': 'os_too_old',
        'min_os': '27.0',
      };
      await tester.tap(offer(all));
      await settle(tester);
      expect(find.textContaining('handwriting'), findsNothing);
    });
  });

  testWidgets('another agent shown and back: the answer typed and the page '
      'are as they were (Prosciutto\'s S5)', (tester) async {
    asIPad();
    final native = DocumentNative();
    final harness = await showDocument(tester);
    await native.ready();
    await native.turned(3);
    await tester.enterText(_field, 'Page 3 adds up');
    await tester.pump();
    final first = native.views.last;

    // Rivet shown: Builder's pane goes from the window, still in its layout.
    final asked = harness.backend.workspace.workspace.clone();
    harness.backend.workspace.apply(asked.clone()..selectedAgentId = 'rivet');
    await settle(tester);
    expect(_field, findsNothing);

    // Builder again: a new view, which goes back to page 3, and the answer.
    harness.backend.workspace.apply(asked);
    await settle(tester);
    expect(native.views.last, isNot(first));
    await native.ready();
    await settle(tester);
    final turns = [
      for (final call in native.calls)
        if (call.method == 'goToPage') (call.arguments as Map)['page_1based'],
    ];
    expect(turns, [3]);
    expect(find.text('Page 3 adds up'), findsOneWidget);
    expect(find.text('Submit'), findsOneWidget);

    // Answered: nothing is kept for it.
    await tester.tap(find.text('Submit'));
    await settle(tester);
    expect(
      sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk.text,
      'Page 3 adds up',
    );
  });

  testWidgets('a pane shown again on page 1 isn\'t turned', (tester) async {
    asIPad();
    final native = DocumentNative();
    final harness = await showDocument(tester);
    await native.ready();
    final asked = harness.backend.workspace.workspace.clone();
    harness.backend.workspace.apply(asked.clone()..selectedAgentId = 'rivet');
    await settle(tester);
    harness.backend.workspace.apply(asked);
    await settle(tester);
    await native.ready();
    await settle(tester);
    expect(native.calls.where((c) => c.method == 'goToPage'), isEmpty);
    expect(find.text('Approve'), findsOneWidget);
  });

  group('answering in the pane', () {
    SetLayout? closed(Harness harness) =>
        sentOf(harness, (m) => m.hasSetLayout()).lastOrNull?.setLayout;

    testWidgets('typed, Return sends it as the answer, and the pane closes '
        'back to the conversation', (tester) async {
      asIPad();
      DocumentNative();
      final harness = await showDocument(tester);
      harness.backend.appliesLayouts = true;
      // Always open, named for VoiceOver, with how to send it.
      expect(_field, findsOneWidget);
      expect(find.text('Reply to Builder'), findsOneWidget);
      expect(
        find.byWidgetPredicate(
          (w) =>
              w is Semantics &&
              w.properties.hint == 'Return sends it to Builder.',
        ),
        findsOneWidget,
      );
      // Empty: Return does nothing, so a stray one never approves.
      await tester.tap(_field);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await settle(tester);
      expect(sentOf(harness, (m) => m.hasAnswerAsk()), isEmpty);
      expect(find.text('Approve'), findsOneWidget);

      await tester.enterText(_field, 'I like the second one better');
      await tester.pump();
      expect(find.text('Submit'), findsOneWidget);
      expect(find.text('Approve'), findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await settle(tester);
      final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
      expect(answer.text, 'I like the second one better');
      // A reply, never a yes (#370).
      expect(answer.reply, isTrue);
      // The paper's pane is gone: only the conversation is left.
      final layout = closed(harness)!;
      expect(layout.agentId, 'a1');
      expect(layout.hasRoot(), isFalse);
    });

    testWidgets('the on-screen keyboard\'s Send key sends too', (tester) async {
      asIPad();
      DocumentNative();
      final harness = await showDocument(tester);
      await tester.enterText(_field, 'Second');
      await tester.testTextInput.receiveAction(TextInputAction.send);
      await settle(tester);
      expect(
        sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk.text,
        'Second',
      );
    });

    testWidgets('empty, Approve approves with no words', (tester) async {
      asIPad();
      DocumentNative();
      final harness = await showDocument(tester);
      await tester.tap(find.text('Approve'));
      await settle(tester);
      final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
      expect(answer.text, isEmpty);
      expect(answer.dismiss, isFalse);
      expect(answer.reply, isFalse);
      // Opened from the file, not its card: no talk seen (#492).
      expect(answer.hasTalkSeen(), isTrue);
      expect(answer.talkSeen, 0);
    });

    testWidgets('an agent that wants words: Answer waits for them', (
      tester,
    ) async {
      asIPad();
      DocumentNative();
      final harness = await showDocument(
        tester,
        ask: (ask) => ask.wantsText = true,
      );
      expect(find.text('Approve'), findsNothing);
      expect(find.text('Your answer'), findsOneWidget);
      await tester.tap(find.text('Answer'));
      await settle(tester);
      expect(sentOf(harness, (m) => m.hasAnswerAsk()), isEmpty);
      await tester.enterText(_field, 'The second');
      await tester.pump();
      // Words are this card's answer: no "instead of" line.
      expect(find.textContaining('instead of'), findsNothing);
      await tester.tap(find.text('Answer'));
      await settle(tester);
      final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
      expect(answer.text, 'The second');
      // The words it asked for are an answer, not a reply.
      expect(answer.reply, isFalse);
    });

    testWidgets('an answer that fails keeps its words and the pane', (
      tester,
    ) async {
      asIPad();
      DocumentNative();
      final harness = await showDocument(tester);
      harness.backend.refuse = 'Not connected to the backend.';
      await tester.enterText(_field, 'The second');
      await tester.pump();
      await tester.tap(find.text('Submit'));
      await settle(tester);
      // Cut off: it waits, and goes once the connection's back.
      expect(find.text(decisionWaiting), findsOneWidget);
      expect(tester.widget<EditableText>(_field).controller.text, 'The second');
      expect(sentOf(harness, (m) => m.hasSetLayout()), isEmpty);
    });

    testWidgets(
      'an answer to a card changed meanwhile says so (#428), and keeps '
      'its words and the pane',
      (tester) async {
        asIPad();
        DocumentNative();
        final harness = await showDocument(tester);
        harness.backend.refuse = 'This card changed. Look at it again.';
        await tester.enterText(_field, 'The second');
        await tester.pump();
        await tester.tap(find.text('Submit'));
        await settle(tester);
        expect(
          find.text('This card changed. Look at it again.'),
          findsOneWidget,
        );
        expect(
          tester.widget<EditableText>(_field).controller.text,
          'The second',
        );
        expect(sentOf(harness, (m) => m.hasSetLayout()), isEmpty);
      },
    );
  });

  group('a card its agent replied on while the PDF was open (row 485)', () {
    const changed = 'This card changed. Look at it again.';

    Future<Harness> stale(WidgetTester tester) async {
      final harness = await showDocument(
        tester,
        ask: (ask) => ask.revision = 1,
      );
      // Builder replied on the card meanwhile: only its latest revision is
      // taken.
      final replied = harness.backend.entriesById['a1/7']!.deepCopy();
      replied.askPerson
        ..revision = 2
        ..thread.add(
          AskThreadMessage(fromAgent: true, text: 'Page 3 is the summary.'),
        );
      harness.backend.entriesById['a1/7'] = replied;
      harness.backend.onSend = (m) {
        if (m.hasAnswerAsk() && m.answerAsk.revision != 2) {
          throw const RequestFailure(changed);
        }
      };
      return harness;
    }

    testWidgets('Dismiss goes with its latest revision, the drawings with '
        'it, uploaded once', (tester) async {
      asIPad();
      final native = DocumentNative();
      final harness = await stale(tester);
      await native.drew(2, 4);
      await settle(tester);
      await tester.tap(find.text('Dismiss'));
      await settle(tester);
      final answers = [
        for (final m in sentOf(harness, (m) => m.hasAnswerAsk())) m.answerAsk,
      ];
      expect([for (final a in answers) a.revision], [1, 2]);
      expect(answers.last.dismiss, isTrue);
      expect(
        [for (final a in answers.last.attachments) a.name],
        ['paper-p2.png'],
      );
      expect(sentOf(harness, (m) => m.hasStartUpload()), hasLength(1));
      expect(find.text(changed), findsNothing);
    });

    testWidgets('Approve is refused, and the bar shows the reply; approved '
        'again, it goes', (tester) async {
      asIPad();
      DocumentNative();
      final harness = await stale(tester);
      await tester.tap(find.text('Approve'));
      await settle(tester);
      expect(find.text(changed), findsOneWidget);
      expect(
        find.textContaining('Page 3 is the summary.', findRichText: true),
        findsOneWidget,
      );
      await tester.tap(find.text('Approve'));
      await settle(tester);
      final last = sentOf(harness, (m) => m.hasAnswerAsk()).last.answerAsk;
      expect((last.revision, last.talkSeen, last.dismiss), (2, 1, false));
    });
  });

  testWidgets("replied on from another device, the bar keeps the card, held, "
      "rather than an older one; back with what's typed and the drawings "
      "once its agent answers (Abbacchio's row 1325)", (tester) async {
    asIPad();
    final native = DocumentNative();
    final harness = await showDocument(tester, ask: (ask) => ask.revision = 1);
    await native.drew(2, 4);
    await tester.enterText(_field, 'Half done');
    await settle(tester);
    Future<void> waiting(bool waits) async {
      final workspace = harness.backend.workspace.workspace.deepCopy();
      final agent = workspace.agents.firstWhere((agent) => agent.id == 'a1');
      agent.waitingSeqs.clear();
      if (waits) agent.waitingSeqs.add(Int64(7));
      await harness.apply(tester, workspace);
      await settle(tester);
    }

    final asked = harness.backend.entriesById['a1/7']!;
    harness.backend.entriesById['a1/7'] = asked.deepCopy()
      ..askPerson.state = AskPersonState.ASK_PERSON_STATE_ANSWERED
      ..askPerson.replied = true
      ..askPerson.answer = 'From the Mac';
    await waiting(false);
    expect(find.byKey(const ValueKey('document-review')), findsOneWidget);
    expect(
      find.text('You replied. Waiting for Builder to answer.'),
      findsOneWidget,
    );
    await tester.tap(find.byKey(const ValueKey('review-send')));
    await settle(tester);
    expect(sentOf(harness, (m) => m.hasAnswerAsk()), isEmpty);
    // Builder answers on it: it waits again, at a new revision.
    harness.backend.entriesById['a1/7'] = asked.deepCopy()
      ..askPerson.revision = 2;
    await waiting(true);
    expect(
      find.text('You replied. Waiting for Builder to answer.'),
      findsNothing,
    );
    expect(tester.widget<EditableText>(_field).controller.text, 'Half done');
    await tester.tap(find.byKey(const ValueKey('review-send')));
    await settle(tester);
    final answer = sentOf(harness, (m) => m.hasAnswerAsk()).last.answerAsk;
    expect((answer.revision, answer.text), (2, 'Half done'));
    expect([for (final a in answer.attachments) a.name], ['paper-p2.png']);
  });

  testWidgets("closed while held, the bar moves on to the next card saying "
      "so, its field empty (Zucchero's row 513)", (tester) async {
    asIPad();
    DocumentNative();
    final harness = await showDocument(tester, ask: (ask) => ask.revision = 1);
    final asked = harness.backend.entriesById['a1/7']!;
    harness.backend.entriesById['a1/8'] = asked.deepCopy()
      ..seq = Int64(8)
      ..askPerson.id = 'q8'
      ..askPerson.title = 'And the appendix';
    Future<void> waiting(List<int> seqs) async {
      final workspace = harness.backend.workspace.workspace.deepCopy();
      final agent = workspace.agents.firstWhere((agent) => agent.id == 'a1');
      agent.waitingSeqs
        ..clear()
        ..addAll([for (final seq in seqs) Int64(seq)]);
      await harness.apply(tester, workspace);
      await settle(tester);
    }

    await waiting([7, 8]);
    await tester.enterText(_field, 'Draft for the first');
    await settle(tester);
    harness.backend.entriesById['a1/7'] = asked.deepCopy()
      ..askPerson.state = AskPersonState.ASK_PERSON_STATE_ANSWERED
      ..askPerson.replied = true;
    await waiting([8]);
    expect(
      find.text('You replied. Waiting for Builder to answer.'),
      findsOneWidget,
    );
    harness.backend.entriesById['a1/7'] = asked.deepCopy()
      ..askPerson.state = AskPersonState.ASK_PERSON_STATE_ANSWERED
      ..askPerson.replied = true
      ..askPerson.resolved = true;
    await waiting([8]);
    expect(find.text('And the appendix'), findsOneWidget);
    expect(
      find.text('That card was closed. This is another one from Builder.'),
      findsOneWidget,
    );
    expect(tester.widget<EditableText>(_field).controller.text, isEmpty);
  });

  group('files with the answer (#511)', () {
    late Directory folder;
    late String shot;
    setUp(() {
      folder = Directory.systemTemp.createTempSync('ipad-review-files');
      shot = '${folder.path}/logo.png';
      File(shot).writeAsBytesSync(List.filled(500, 3));
    });
    tearDown(() => folder.deleteSync(recursive: true));

    Future<void> io(WidgetTester tester) async {
      for (var i = 0; i < 12; i++) {
        await tester.pump();
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 10)),
        );
      }
      await settle(tester);
    }

    Future<void> attach(WidgetTester tester, Harness harness) async {
      await tester.runAsync(
        () => harness.backend.attachments.add(cardFilesKey('a1', 'q1'), [shot]),
      );
      await tester.pump();
    }

    testWidgets('a file and a drawn page go in one answer; the drawings '
        'line stays beside the chips', (tester) async {
      asIPad();
      final native = DocumentNative();
      final harness = await showDocument(tester);
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('document-review')),
          matching: find.bySemanticsLabel('Attach files'),
        ),
        findsOneWidget,
      );
      await attach(tester, harness);
      await native.drew(2, 4);
      await settle(tester);
      expect(find.text('logo.png'), findsOneWidget);
      expect(find.text(drawingGoesLine), findsOneWidget);
      await tester.tap(find.text('Approve'));
      await io(tester);
      final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
      expect(
        [for (final a in answer.attachments) a.name],
        ['paper-p2.png', 'logo.png'],
      );
      expect(harness.backend.attachments.of(cardFilesKey('a1', 'q1')), isEmpty);
    });

    testWidgets("drawings that can't go up: nothing is sent, and the file "
        'stays for the retry', (tester) async {
      asIPad();
      final native = DocumentNative();
      readComposite = (file) async => throw StateError('gone');
      final harness = await showDocument(tester);
      await attach(tester, harness);
      await native.drew(1, 1);
      await settle(tester);
      await tester.tap(find.text('Dismiss'));
      await io(tester);
      expect(sentOf(harness, (m) => m.hasAnswerAsk()), isEmpty);
      expect(find.text('logo.png'), findsOneWidget);
      expect(harness.backend.attachments.of(cardFilesKey('a1', 'q1')), [
        isA<LocalFile>(),
      ]);
    });

    for (final width in [320.0, 375.0, 516.0]) {
      testWidgets('at ${width.toInt()} points wide (Slide Over, a third, '
          'a half) the paperclip is a 44 pt target beside the mic and the '
          'field', (tester) async {
        asIPad();
        DocumentNative();
        await showDocument(tester, size: Size(width, 1024));
        if (find.text('paper.pdf').evaluate().isNotEmpty) {
          await tester.tap(find.text('paper.pdf').first);
          await settle(tester);
        }
        expect(tester.takeException(), isNull);
        final clip = find.descendant(
          of: find.byKey(const ValueKey('document-review')),
          matching: find.bySemanticsLabel('Attach files'),
        );
        final field = tester.getRect(
          find.byKey(const ValueKey('review-answer')),
        );
        final rect = tester.getRect(clip);
        expect(rect.height, greaterThanOrEqualTo(44));
        expect(rect.width, greaterThanOrEqualTo(44));
        expect(rect.left, greaterThanOrEqualTo(field.right));
        expect(rect.right, lessThanOrEqualTo(width));
      });
    }
  });

  group('answering by voice', () {
    late FakeRecorder recorder;
    late VoiceRecorder was;
    late Transcribe wasTranscribe;
    setUp(() {
      voiceMessagesOn = true;
      was = deviceRecorder;
      recorder = FakeRecorder();
      deviceRecorder = recorder;
      wasTranscribe = transcribeRecording;
    });
    tearDown(() {
      deviceRecorder = was;
      transcribeRecording = wasTranscribe;
    });

    testWidgets('the mic at the answer records; Done puts the words in at '
        'the caret, nothing sent until Submit', (tester) async {
      transcribeRecording = (backend, recording) async => 'except page 3';
      asIPad();
      DocumentNative();
      final harness = await showDocument(tester);
      await harness.apply(
        tester,
        harness.backend.workspace.workspace.clone()
          ..speechModel = SpeechModel(
            state: SpeechModelState.SPEECH_MODEL_STATE_READY,
          ),
      );
      await settle(tester);
      await tester.enterText(_field, 'Fine');
      await tester.pump();
      final dictate = find.bySemanticsLabel('Dictate').last;
      expect(tester.getSize(dictate).height, greaterThanOrEqualTo(44));
      await tester.tap(dictate);
      await io(tester);
      expect(recorder.recordingInto, isNotNull);
      expect(find.text('Done'), findsOneWidget);
      await tester.tap(find.text('Done'));
      await io(tester);
      await settle(tester);
      expect(find.text('Fine except page 3'), findsOneWidget);
      expect(sentOf(harness, (m) => m.hasAnswerAsk()), isEmpty);
      await tester.tap(find.text('Submit'));
      await settle(tester);
      expect(
        sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk.text,
        'Fine except page 3',
      );
    });

    testWidgets('a Claktory that can\'t take voice: no mic', (tester) async {
      asIPad();
      DocumentNative();
      await showDocument(tester);
      expect(find.bySemanticsLabel('Dictate'), findsNothing);
    });
  });
}
