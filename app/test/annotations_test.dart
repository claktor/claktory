import 'dart:convert';

import 'package:factory_app/annotations.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, showPanes;
import 'viewers_test.dart' show png;

const _essay = '''# On rivers

The first paragraph sets the scene.

The second paragraph makes the claim, and it is far too long.

The third one ends it.
''';

FileContent _file(String path, String body, {String sha = 'v1'}) => FileContent(
  path: path,
  mime: path.endsWith('.md') ? 'text/markdown' : 'text/plain',
  size: Int64(utf8.encode(body).length),
  data: utf8.encode(body),
  sha256: sha,
);

/// A thread on "makes the claim" of [_essay], rendered and in the source.
Thread _thread({
  String id = 't1',
  bool draft = true,
  String body = 'Lead with this.',
  AnchorState anchor = AnchorState.ANCHOR_STATE_ANCHORED,
  ThreadStatus status = ThreadStatus.THREAD_STATUS_OPEN,
  Suggestion? suggestion,
}) {
  const exact = 'makes the claim';
  final rendered =
      'On rivers\nThe first paragraph sets the scene.\n'
      'The second paragraph ';
  final source = _essay.indexOf(exact);
  return Thread(
    id: id,
    agentId: 'a1',
    path: '/w/essay.md',
    kind: 'markdown',
    versionSha256: 'v1',
    status: status,
    anchor: anchor,
    rendered: anchor == AnchorState.ANCHOR_STATE_DETACHED
        ? null
        : TextAnchor(
            start: rendered.length,
            end: rendered.length + exact.length,
            exact: exact,
          ),
    source: anchor == AnchorState.ANCHOR_STATE_DETACHED
        ? null
        : TextAnchor(
            start: source,
            end: source + exact.length,
            exact: exact,
            firstLine: 5,
            lastLine: 5,
          ),
    originalExact: exact,
    comments: [ThreadComment(id: 'c1', body: body, draft: draft)],
    suggestion: suggestion,
  );
}

Future<Harness> _showEssay(
  WidgetTester tester, {
  List<Thread> threads = const [],
}) async {
  final harness = await showPanes(
    tester,
    beside('text', '/w/essay.md'),
    files: {'/w/essay.md': _file('/w/essay.md', _essay)},
  );
  // Wide enough for the margin beside the conversation.
  tester.view.physicalSize = const Size(1600, 900);
  harness.backend.threads.apply(
    ThreadList(path: '/w/essay.md', versionSha256: 'v1', threads: threads),
  );
  await settle(tester);
  // The rendered text is known after the frame that built it.
  await tester.pump();
  await tester.pump();
  return harness;
}

/// The background colors painted behind [text] in the viewer.
List<Color> _marksOn(WidgetTester tester, String text) {
  final colors = <Color>[];
  for (final widget in tester.widgetList<RichText>(find.byType(RichText))) {
    widget.text.visitChildren((span) {
      if (span is TextSpan &&
          span.text == text &&
          span.style?.backgroundColor != null) {
        colors.add(span.style!.backgroundColor!);
      }
      return true;
    });
  }
  return colors;
}

void main() {
  group('passages', () {
    test('a quote is found where it was, else nearest it', () {
      const text = 'one two three. one two three.';
      expect(
        locatePassage(text, TextAnchor(start: 4, end: 7, exact: 'two')),
        const TextRange(start: 4, end: 7),
      );
      // Moved: the nearer of the two.
      expect(
        locatePassage(text, TextAnchor(start: 17, end: 20, exact: 'two')),
        const TextRange(start: 19, end: 22),
      );
      // Blocks joined differently: any whitespace matches.
      expect(
        locatePassage(
          'end.\n\nNext one',
          TextAnchor(start: 0, end: 9, exact: 'end. Next'),
        ),
        const TextRange(start: 0, end: 10),
      );
      expect(
        locatePassage(text, TextAnchor(start: 0, end: 4, exact: 'four')),
        isNull,
      );
    });

    test('a passage quotes 32 characters each side, as W3C selectors', () {
      final text = '${'a' * 40}PICK${'b' * 40}';
      final passage = Passage('markdown', text, 44, 40);
      expect(passage.exact, 'PICK');
      expect(passage.prefix, 'a' * 32);
      expect(passage.suffix, 'b' * 32);
      expect(
        [for (final selector in passage.selectors) jsonDecode(selector.json)],
        [
          {
            'type': 'TextQuoteSelector',
            'exact': 'PICK',
            'prefix': 'a' * 32,
            'suffix': 'b' * 32,
          },
          {'type': 'TextPositionSelector', 'start': 40, 'end': 44},
        ],
      );
    });

    test('marks paint behind a span and leave its text as it was', () {
      const red = Color(0xFFFF0000);
      final span = TextSpan(
        text: 'Hello ',
        children: [
          TextSpan(
            text: 'bold world',
            style: TextStyle(fontWeight: FontWeight.w600),
          ),
        ],
      );
      final marked = markSpan(span, [(14, 20, red)], base: 10);
      expect(marked.toPlainText(), span.toPlainText());
      final painted = <String>[];
      marked.visitChildren((piece) {
        if (piece is TextSpan && piece.style?.backgroundColor == red) {
          painted.add(piece.text!);
        }
        return true;
      });
      expect(painted, ['o ', 'bold']);
      expect(markSpan(span, const []), same(span));
    });

    test('a region is a media fragment, and reads back', () {
      const region = Region(Rect.fromLTWH(0.125, 0.08, 0.3, 0.2));
      final json = jsonDecode(region.selectors.single.json) as Map;
      expect(json['type'], 'FragmentSelector');
      expect(json['value'], 'xywh=percent:12.5,8,30,20');
      final back = regionOf(Thread(selectors: region.selectors))!;
      expect(back.left, closeTo(0.125, 1e-9));
      expect(back.top, closeTo(0.08, 1e-9));
      expect(back.width, closeTo(0.3, 1e-9));
      expect(back.height, closeTo(0.2, 1e-9));
      expect(const Region(Rect.fromLTWH(0.5, 0.5, 0.001, 0.2)).isEmpty, isTrue);
      expect(regionOf(Thread(selectors: [Selector(json: 'not json')])), isNull);
    });

    test('a marked link stays a link in every piece', () {
      final tap = TapGestureRecognizer();
      addTearDown(tap.dispose);
      final span = TextSpan(
        text: 'see ',
        children: [TextSpan(text: 'the docs', recognizer: tap)],
      );
      final marked = markSpan(span, [(6, 9, const Color(0x33FF0000))]);
      final links = <String>[];
      marked.visitChildren((piece) {
        if (piece is TextSpan && piece.text != null) {
          if (identical(piece.recognizer, tap)) links.add(piece.text!);
        }
        return true;
      });
      expect(links.join(), 'the docs');
      expect(links, hasLength(greaterThan(1)));
    });
  });

  group('the text viewer', () {
    testWidgets('asks for the comments and shows them beside the passage', (
      tester,
    ) async {
      final harness = await _showEssay(tester, threads: [_thread()]);
      expect(
        harness.backend.sent
            .where((m) => m.hasListThreads())
            .map((m) => m.listThreads.path),
        contains('/w/essay.md'),
      );
      expect(find.text('Lead with this.'), findsOneWidget);
      expect(find.text('Draft'), findsOneWidget);
      expect(_marksOn(tester, 'makes the claim'), hasLength(1));
      // The card is level with its passage.
      final card = tester.getTopLeft(find.text('Lead with this.')).dy;
      final passage = tester.getTopLeft(
        find.textContaining('The second paragraph', findRichText: true).first,
      );
      expect((card - passage.dy).abs(), lessThan(80));
    });

    testWidgets('sends the drafts to the agent in one go', (tester) async {
      final harness = await _showEssay(tester, threads: [_thread()]);
      await tester.tap(find.text('Send 1 comment to Builder'));
      await settle(tester);
      final sent = harness.backend.sent.lastWhere((m) => m.hasSendComments());
      expect(sent.sendComments.agentId, 'a1');
      expect(sent.sendComments.path, '/w/essay.md');
    });

    testWidgets('a sent comment has no Send button, and shows when', (
      tester,
    ) async {
      await _showEssay(tester, threads: [_thread(draft: false)]);
      expect(find.textContaining('Send '), findsNothing);
      expect(find.text('Draft'), findsNothing);
    });

    testWidgets('selecting a passage and commenting on it makes a thread', (
      tester,
    ) async {
      final harness = await _showEssay(tester);
      final paragraph = find
          .textContaining('The second paragraph', findRichText: true)
          .first;
      final box = tester.getRect(paragraph);
      final mouse = await tester.startGesture(
        box.topLeft + const Offset(1, 8),
        kind: PointerDeviceKind.mouse,
      );
      await tester.pump();
      await mouse.moveTo(box.topLeft + const Offset(120, 8));
      await tester.pump();
      await mouse.up();
      await settle(tester);
      await tester.tap(find.text('Comment'));
      await settle(tester);
      await tester.enterText(_field, 'Too long.');
      await tester.pump();
      // The composer's Comment, which keeps it as a draft.
      await tester.tap(find.text('Comment').last);
      await settle(tester);
      final made = harness.backend.sent.lastWhere((m) => m.hasCreateThread());
      final create = made.createThread;
      expect(create.agentId, 'a1');
      expect(create.path, '/w/essay.md');
      expect(create.kind, 'markdown');
      expect(create.versionSha256, 'v1');
      expect(create.body, 'Too long.');
      expect(create.sendNow, isFalse);
      final quote = jsonDecode(create.selectors.first.json) as Map;
      expect(quote['type'], 'TextQuoteSelector');
      expect('The second paragraph makes', startsWith(quote['exact']));
      expect(quote['prefix'], endsWith('sets the scene.\n'));
    });

    testWidgets('selecting again while the highlights change', (tester) async {
      final harness = await _showEssay(tester);
      Future<void> drag(String text, double from, double to) async {
        final box = tester.getRect(
          find.textContaining(text, findRichText: true).first,
        );
        final mouse = await tester.startGesture(
          box.topLeft + Offset(from, 8),
          kind: PointerDeviceKind.mouse,
        );
        await tester.pump();
        await mouse.moveTo(box.topLeft + Offset(to, 8));
        await tester.pump();
        await mouse.up();
        await settle(tester);
      }

      await drag('The second paragraph', 1, 120);
      expect(
        (_controller(tester).selection.value as Passage?)?.exact,
        isNotEmpty,
      );
      // A comment lands on the paragraph under the selection: it's drawn
      // again, highlighted, while the selection is in it.
      harness.backend.threads.apply(
        ThreadList(
          path: '/w/essay.md',
          versionSha256: 'v1',
          threads: [_thread()],
        ),
      );
      await settle(tester);
      await tester.pump();
      await drag('The third one', 1, 60);
      expect(tester.takeException(), isNull);
      expect(
        'The third one ends it.',
        startsWith((_controller(tester).selection.value! as Passage).exact),
      );
    });

    testWidgets('⌘Enter sends a new comment at once', (tester) async {
      final harness = await _showEssay(tester);
      final controller = _controller(tester);
      controller.selection.value = Passage(
        'markdown',
        'On rivers\nThe first paragraph sets the scene.',
        10,
        19,
      );
      controller.comment();
      await settle(tester);
      await tester.enterText(_field, 'Cut this.');
      await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
      await settle(tester);
      final made = harness.backend.sent.lastWhere((m) => m.hasCreateThread());
      expect(made.createThread.sendNow, isTrue);
      expect(made.createThread.body, 'Cut this.');
    });

    testWidgets('a picked thread offers Reply and Resolve', (tester) async {
      final harness = await _showEssay(tester, threads: [_thread()]);
      await tester.tap(find.text('Lead with this.'));
      await settle(tester);
      // Picked, its passage stands out more.
      expect(_marksOn(tester, 'makes the claim').single.a, greaterThan(0.3));
      await tester.tap(find.text('Resolve'));
      await settle(tester);
      final status = harness.backend.sent.lastWhere(
        (m) => m.hasSetThreadStatus(),
      );
      expect(status.setThreadStatus.threadId, 't1');
      expect(status.setThreadStatus.resolved, isTrue);

      await tester.tap(find.text('Reply'));
      await settle(tester);
      await tester.enterText(_field, 'Done?');
      await settle(tester);
      await tester.tap(find.text('Comment').last);
      await settle(tester);
      final reply = harness.backend.sent.lastWhere((m) => m.hasReplyThread());
      expect(reply.replyThread.threadId, 't1');
      expect(reply.replyThread.body, 'Done?');
    });

    testWidgets('a thread whose passage is gone says so', (tester) async {
      await _showEssay(
        tester,
        threads: [_thread(anchor: AnchorState.ANCHOR_STATE_DETACHED)],
      );
      expect(find.text('The text it was on is gone.'), findsOneWidget);
      expect(find.textContaining('makes the claim'), findsWidgets);
      expect(_marksOn(tester, 'makes the claim'), isEmpty);
    });

    testWidgets('resolved threads hide until asked for', (tester) async {
      await _showEssay(
        tester,
        threads: [_thread(status: ThreadStatus.THREAD_STATUS_RESOLVED)],
      );
      expect(find.text('Lead with this.'), findsNothing);
      await tester.tap(find.text('Show 1 resolved'));
      await settle(tester);
      expect(find.text('Lead with this.'), findsOneWidget);
      expect(find.text('Resolved'), findsOneWidget);
    });

    testWidgets('the plain view highlights by line and says which', (
      tester,
    ) async {
      await _showEssay(tester, threads: [_thread()]);
      await tester.tap(find.text('Plain'));
      await settle(tester);
      await tester.pump();
      expect(find.textContaining('Line 5'), findsOneWidget);
      expect(_marksOn(tester, 'makes the claim'), hasLength(1));
    });

    testWidgets('it reads the file again when it changes, and says who', (
      tester,
    ) async {
      final harness = await _showEssay(tester);
      // Something's selected in the paragraph that changes.
      final box = tester.getRect(
        find.textContaining('The second paragraph', findRichText: true).first,
      );
      final mouse = await tester.startGesture(
        box.topLeft + const Offset(1, 8),
        kind: PointerDeviceKind.mouse,
      );
      await tester.pump();
      await mouse.moveTo(box.topLeft + const Offset(200, 8));
      await tester.pump();
      await mouse.up();
      await settle(tester);
      final before = harness.backend.sent.where((m) => m.hasReadFile()).length;
      harness.backend.files['/w/essay.md'] = _file(
        '/w/essay.md',
        _essay.replaceFirst('far too long', 'short'),
        sha: 'v2',
      );
      harness.backend.fileChanged.value = FileChanged(
        path: '/w/essay.md',
        sha256: 'v2',
        byAgentId: 'a1',
      );
      await settle(tester);
      expect(
        harness.backend.sent.where((m) => m.hasReadFile()).length,
        before + 1,
      );
      expect(find.textContaining('it is short'), findsOneWidget);
      expect(find.text('Builder just updated this file'), findsOneWidget);
      // Another file changing doesn't matter here.
      harness.backend.fileChanged.value = FileChanged(path: '/w/other.md');
      await settle(tester);
      expect(
        harness.backend.sent.where((m) => m.hasReadFile()).length,
        before + 1,
      );
      await tester.pump(const Duration(seconds: 7));
    });

    testWidgets('a narrow pane has a Comments button and a sheet', (
      tester,
    ) async {
      final harness = await showPanes(
        tester,
        beside('text', '/w/essay.md'),
        files: {'/w/essay.md': _file('/w/essay.md', _essay)},
      );
      tester.view.physicalSize = const Size(1000, 800);
      harness.backend.threads.apply(
        ThreadList(
          path: '/w/essay.md',
          versionSha256: 'v1',
          threads: [_thread()],
        ),
      );
      await settle(tester);
      await tester.pump();
      final button = find.byWidgetPredicate(
        (w) => w is GlyphButton && w.label == '1 comments',
      );
      expect(button, findsOneWidget);
      expect(find.text('Lead with this.'), findsNothing);
      await tester.tap(button);
      await settle(tester);
      expect(find.text('Lead with this.'), findsOneWidget);
      expect(find.text('Send 1 comment to Builder'), findsOneWidget);
    });
  });

  group('suggested changes', () {
    Finder button(String label) =>
        find.descendant(of: find.byType(Commented), matching: find.text(label));

    bool enabled(WidgetTester tester, String label) =>
        tester
            .widget<Button>(
              find.ancestor(of: button(label), matching: find.byType(Button)),
            )
            .onPressed !=
        null;

    testWidgets("an agent's shows the change, to accept or reject", (
      tester,
    ) async {
      final harness = await _showEssay(
        tester,
        threads: [
          _thread(
            draft: false,
            body: 'Shorter.',
            suggestion: Suggestion(replacement: 'argues', authorAgentId: 'a1'),
          ),
        ],
      );
      expect(find.text('argues'), findsOneWidget);
      expect(find.text('Shorter.'), findsOneWidget);
      expect(enabled(tester, 'Accept'), isTrue);
      await tester.tap(button('Accept'));
      await settle(tester);
      final decided = harness.backend.sent.lastWhere(
        (m) => m.hasDecideSuggestion(),
      );
      expect(decided.decideSuggestion.threadId, 't1');
      expect(decided.decideSuggestion.accept, isTrue);
      await tester.tap(button('Reject'));
      await settle(tester);
      expect(harness.backend.sent.last.decideSuggestion.accept, isFalse);
    });

    testWidgets('once its text changed it can only be rejected', (
      tester,
    ) async {
      await _showEssay(
        tester,
        threads: [
          _thread(
            anchor: AnchorState.ANCHOR_STATE_MOVED,
            suggestion: Suggestion(replacement: 'argues', authorAgentId: 'a1'),
          ),
        ],
      );
      expect(
        find.text('The text changed after this was suggested.'),
        findsOneWidget,
      );
      expect(enabled(tester, 'Accept'), isFalse);
      expect(enabled(tester, 'Reject'), isTrue);
    });

    testWidgets("the person's own: Apply, or Discard while unsent", (
      tester,
    ) async {
      final harness = await _showEssay(
        tester,
        threads: [
          _thread(
            body: '',
            suggestion: Suggestion(replacement: ''),
          ),
        ],
      );
      // Its comment is empty: only the change shows, a deletion here.
      expect(find.text('Delete this text'), findsOneWidget);
      expect(find.text('Draft'), findsNothing);
      expect(find.text('Send 1 comment to Builder'), findsOneWidget);
      await tester.tap(button('Discard'));
      await settle(tester);
      expect(harness.backend.sent.last.deleteComment.commentId, 'c1');
      await tester.tap(button('Apply'));
      await settle(tester);
      expect(harness.backend.sent.last.decideSuggestion.accept, isTrue);
    });

    testWidgets('decided, it says which', (tester) async {
      await _showEssay(
        tester,
        threads: [
          _thread(
            status: ThreadStatus.THREAD_STATUS_RESOLVED,
            suggestion: Suggestion(
              replacement: 'argues',
              authorAgentId: 'a1',
              decision: SuggestionDecision.SUGGESTION_DECISION_ACCEPTED,
            ),
          ),
        ],
      );
      _controller(tester).toggleResolved();
      await settle(tester);
      expect(find.text('Accepted'), findsOneWidget);
      expect(button('Accept'), findsNothing);
    });

    testWidgets('suggesting a change at a selection', (tester) async {
      final harness = await _showEssay(tester);
      final controller = _controller(tester);
      controller.selection.value = Passage(
        'markdown',
        'On rivers\nThe first paragraph sets the scene.',
        20,
        29,
      );
      await settle(tester);
      await tester.tap(button('Suggest'));
      await settle(tester);
      final fields = find.descendant(
        of: find.byType(Commented),
        matching: find.byType(EditableText),
      );
      expect(fields, findsNWidgets(2));
      expect(
        tester.widget<EditableText>(fields.first).controller.text,
        'paragraph',
      );
      // Nothing changed yet: nothing to suggest.
      expect(enabled(tester, 'Suggest'), isFalse);
      await tester.enterText(fields.first, 'part');
      await tester.pump();
      await tester.tap(button('Suggest').last);
      await settle(tester);
      final made = harness.backend.sent.lastWhere((m) => m.hasCreateThread());
      expect(made.createThread.suggestion, 'part');
      expect(made.createThread.body, '');
      expect(made.createThread.sendNow, isFalse);
    });
  });

  _imageTests();
}

void _imageTests() {
  group('image regions', () {
    const path = '/w/chart.png';
    const area = Rect.fromLTWH(0.25, 0.25, 0.5, 0.5);

    Thread regionThread({
      AnchorState anchor = AnchorState.ANCHOR_STATE_ANCHORED,
    }) => Thread(
      id: 'r1',
      agentId: 'a1',
      path: path,
      kind: 'image',
      versionSha256: 'v1',
      status: ThreadStatus.THREAD_STATUS_OPEN,
      anchor: anchor,
      selectors: const Region(area).selectors,
      comments: [ThreadComment(id: 'c1', body: 'Label this axis.')],
    );

    Future<Harness> showChart(
      WidgetTester tester, {
      List<Thread> threads = const [],
    }) async {
      final data = await png(tester, 400, 200);
      final harness = await showPanes(
        tester,
        beside('image', path),
        files: {
          path: FileContent(
            path: path,
            mime: 'image/png',
            size: Int64(data.length),
            data: data,
            sha256: 'v1',
          ),
        },
      );
      tester.view.physicalSize = const Size(1600, 900);
      harness.backend.threads.apply(
        ThreadList(path: path, versionSha256: 'v1', threads: threads),
      );
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 300)),
      );
      await settle(tester);
      await tester.pump();
      return harness;
    }

    /// The image's box on screen.
    Rect picture(WidgetTester tester) => tester.getRect(find.byType(RawImage));

    testWidgets("a comment's region shows, with its card beside it", (
      tester,
    ) async {
      final harness = await showChart(tester, threads: [regionThread()]);
      expect(
        harness.backend.sent.any((m) => m.listThreads.path == path),
        isTrue,
      );
      expect(find.text('Label this axis.'), findsOneWidget);
      expect(find.byType(RegionThumbnail), findsOneWidget);
      // Level with the region's top.
      final box = picture(tester);
      final card = tester.getRect(
        find.ancestor(
          of: find.text('Label this axis.'),
          matching: find.byType(AnimatedContainer),
        ).first,
      );
      expect(card.top, closeTo(box.top + box.height * 0.25, 2));
      expect(
        find.text('Made on an earlier version of this image.'),
        findsNothing,
      );
    });

    testWidgets('a click on a region picks its comment', (tester) async {
      await showChart(tester, threads: [regionThread()]);
      expect(find.text('Reply'), findsNothing);
      await tester.tapAt(picture(tester).center);
      await tester.pump(const Duration(milliseconds: 400));
      await settle(tester);
      expect(find.text('Reply'), findsOneWidget);
    });

    testWidgets('Comment, then a drag, makes a comment on that region', (
      tester,
    ) async {
      final harness = await showChart(tester);
      await tester.tap(find.text('Comment'));
      await settle(tester);
      final box = picture(tester);
      await tester.dragFrom(
        box.topLeft + Offset(box.width * 0.1, box.height * 0.2),
        Offset(box.width * 0.4, box.height * 0.5),
      );
      await settle(tester);
      // The margin offers Comment (and no Suggest: an image can't be).
      expect(find.text('Suggest'), findsNothing);
      await tester.tap(find.text('Comment').last);
      await settle(tester);
      expect(find.byType(RegionThumbnail), findsOneWidget);
      await tester.enterText(_field, 'Brighter here.');
      await tester.pump();
      await tester.tap(find.text('Comment').last);
      await settle(tester);
      final made = harness.backend.sent.lastWhere((m) => m.hasCreateThread());
      expect(made.createThread.kind, 'image');
      expect(made.createThread.versionSha256, 'v1');
      final region = regionOf(Thread(selectors: made.createThread.selectors))!;
      expect(region.left, closeTo(0.1, 0.02));
      expect(region.top, closeTo(0.2, 0.02));
      expect(region.width, closeTo(0.4, 0.02));
      expect(region.height, closeTo(0.5, 0.02));
    });

    testWidgets('a comment made on an earlier version says so', (tester) async {
      await showChart(
        tester,
        threads: [regionThread(anchor: AnchorState.ANCHOR_STATE_OUTDATED)],
      );
      expect(
        find.text('Made on an earlier version of this image.'),
        findsOneWidget,
      );
      // Still on its region: never put aside.
      expect(find.byType(RegionThumbnail), findsOneWidget);
    });

    testWidgets('a new version reads the image again, and says who', (
      tester,
    ) async {
      final harness = await showChart(tester);
      final before = harness.backend.sent.where((m) => m.hasReadFile()).length;
      harness.backend.fileChanged.value = FileChanged(
        path: path,
        sha256: 'v2',
        byAgentId: 'a1',
      );
      await settle(tester);
      expect(
        harness.backend.sent.where((m) => m.hasReadFile()).length,
        before + 1,
      );
      expect(find.text('Builder just updated this file'), findsOneWidget);
      await tester.pump(const Duration(seconds: 7));
    });
  });
}

/// The comment being written's field (not the conversation's).
Finder get _field => find.descendant(
  of: find.byType(Commented),
  matching: find.byType(EditableText),
);

CommentsController _controller(WidgetTester tester) {
  final button = tester.widgetList<CommentsButton>(find.byType(CommentsButton));
  return button.first.controller;
}
