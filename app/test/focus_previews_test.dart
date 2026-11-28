// What opens from focus mode, on a Mac and an iPad: a file, a picture, a
// PDF, a video or a made image, from Context's conversation (the show
// line, an attachment, a picture sent) leaves focus mode first and opens
// beside the request's conversation, in sight, never in the window focus
// mode hides; a link opens the browser and leaves focus mode as it is. A
// file the agent expands meanwhile waits behind focus mode, and Esc or
// Done brings the window back with it, in sight and taking clicks.
import 'dart:convert' show utf8;
import 'dart:io';

import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/layout.dart' show layoutOf, open;
import 'package:factory_app/loopback_forward.dart' show linkLauncher;
import 'package:factory_app/needs_you_focus.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:factory_app/viewers/markdown_media.dart' show MarkdownPicture;
import 'package:factory_app/tool_views.dart' show MadeImage;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'tool_views_test.dart' show call;
import 'viewers_test.dart' show image, png;

final _now = DateTime.now();

/// June and Mista each wait on a question; Mista's, the newer, is on the
/// stage. June's conversation is the window's.
Workspace _workspace() => Workspace(
  agents: [
    Agent(id: 'june', name: 'June', color: 'violet', waitingSeqs: [Int64(1)]),
    Agent(id: 'mista', name: 'Mista', color: 'green', waitingSeqs: [Int64(7)]),
  ],
  selectedAgentId: 'june',
);

Entry _ask(int seq, int minutesAgo) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(
    _now.subtract(Duration(minutes: minutesAgo)).millisecondsSinceEpoch,
  ),
  askPerson: AskPerson(id: 'q$seq', title: 'Question $seq'),
);

const _picture = '/u/mista/shot.png';
const _made = '/Users/me/.factory/agents/mista/images/c3.png';

Finder _inContext(Finder finder) =>
    find.descendant(of: find.byType(FocusContext), matching: finder);

/// Focus mode, Mista's question on the stage, and Context showing
/// [entries] of Mista's conversation before it.
Future<Harness> _focus(
  WidgetTester tester,
  List<Entry> entries, {
  bool iPad = false,
  bool review = false,
}) async {
  if (iPad) {
    asIPad();
  } else {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
  }
  final harness = Harness(_workspace());
  final pixels = await png(tester, 64, 48);
  harness.backend
    ..appliesLayouts = true
    ..previews[_picture] = image(_picture, pixels)
    ..files[_picture] = image(_picture, pixels)
    ..previews[_made] = image(_made, pixels)
    ..files[_made] = image(_made, pixels)
    ..entriesById['june/1'] = _ask(1, 30)
    ..entriesById['mista/7'] = _ask(7, 5);
  if (review) {
    const document = '/w/plan.md';
    final text = utf8.encode('# Plan\n\n![The figure](fig.png)\n');
    harness.backend
      ..files[document] = FileContent(
        path: document,
        mime: 'text/markdown',
        size: Int64(text.length),
        data: text,
        sha256: 'v1',
      )
      ..files['/w/fig.png'] = image('/w/fig.png', pixels)
      ..previews['/w/fig.png'] = image('/w/fig.png', pixels)
      ..entriesById['mista/7'] = (_ask(7, 5)..askPerson.reviewPath = document);
  }
  if (iPad) {
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    await harness.show(
      tester,
      size: const Size(1376, 1032),
      home: WorkbenchScope(
        focus: focus,
        child: const Shell(rail: SizedBox(width: 56)),
      ),
    );
  } else {
    await harness.show(tester, size: const Size(1440, 900));
  }
  await settle(tester);
  setFocusMode(harness.backend, true);
  await settle(tester);
  harness.backend.watchedTranscript.load(
    Transcript(agentId: 'mista', entries: [...entries, _ask(7, 5)]),
  );
  await settle(tester);
  expect(find.byType(FocusContext), findsOneWidget);
  return harness;
}

/// Geist, as the app ships it, so words are as wide as on the device.
Future<void> _loadFonts() async {
  final loader = FontLoader('Geist');
  for (final file in ['Regular', 'Medium', 'SemiBold']) {
    loader.addFont(
      Future.value(
        ByteData.sublistView(File('fonts/Geist-$file.ttf').readAsBytesSync()),
      ),
    );
  }
  await loader.load();
}

void main() {
  setUpAll(_loadFonts);
  tearDown(() => linkLauncher = null);

  for (final iPad in [false, true]) {
    final on = iPad ? 'an iPad' : 'a Mac';

    testWidgets('on $on, the show line in Context leaves focus mode and '
        'opens the file beside its conversation', (tester) async {
      final harness = await _focus(tester, [
        call(
          'show',
          {'path': '/w/notes.md'},
          seq: 5,
          details: {
            'path': '/w/notes.md',
            'kind': 'text',
            'pane': 'gone',
            'agent': 'mista',
          },
        ),
      ], iPad: iPad);
      await tester.tap(_inContext(find.text('notes.md')));
      await _shown(tester, harness, '/w/notes.md');
    });
  }

  for (final name in ['report.pdf', 'clip.mp4', 'notes.txt']) {
    testWidgets('a file sent in the conversation ($name), opened from '
        'Context, leaves focus mode and opens beside it', (tester) async {
      final path = '/u/mista/$name';
      final harness = await _focus(tester, [
        Entry(
          seq: Int64(5),
          user: UserText(
            text: 'Have a look',
            attachments: [Attachment(path: path, name: name, size: Int64(10))],
          ),
        ),
      ]);
      await tester.tap(_inContext(find.text(name)));
      await _shown(tester, harness, path);
    });
  }

  testWidgets('a picture sent in the conversation, opened from Context, '
      'leaves focus mode and opens beside it', (tester) async {
    final harness = await _focus(tester, [
      Entry(
        seq: Int64(5),
        user: UserText(
          text: 'Here',
          attachments: [
            Attachment(path: _picture, name: 'shot.png', size: Int64(10)),
          ],
        ),
      ),
    ]);
    await tester.tap(_inContext(find.bySemanticsLabel(RegExp('shot.png'))));
    await _shown(tester, harness, _picture);
  });

  testWidgets('a picture the agent made, opened from Context, leaves focus '
      'mode and opens beside its conversation', (tester) async {
    final harness = await _focus(tester, [
      call(
        'image_generate',
        {'prompt': 'a fox'},
        seq: 5,
        details: {
          'path': _made,
          'mime': 'image/png',
          'width': 64,
          'height': 48,
          'prompt': 'a fox',
        },
      ),
    ]);
    await tester.tap(_inContext(find.byType(MadeImage)));
    await _shown(tester, harness, _made);
  });

  testWidgets('a picture in the document on the stage for review leaves '
      'focus mode and opens beside its conversation', (tester) async {
    final harness = await _focus(tester, const [], review: true);
    for (var i = 0; i < 10; i++) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 60)),
      );
      await tester.pump();
    }
    final picture = find.descendant(
      of: find.byType(FocusStage),
      matching: find.byType(MarkdownPicture),
    );
    expect(picture, findsOneWidget);
    await tester.tap(picture);
    await _shown(tester, harness, '/w/fig.png');
  });

  testWidgets('a link in Context opens the browser, and focus mode stays as '
      'it is', (tester) async {
    final opened = <Uri>[];
    linkLauncher = (url) async {
      opened.add(url);
      return true;
    };
    final harness = await _focus(tester, [
      Entry(
        seq: Int64(5),
        assistant: AssistantText(
          text: 'Read [the docs](https://example.com/docs).',
        ),
      ),
    ]);
    final sent = harness.backend.sent.length;
    await tester.tapOnText(find.textRange.ofSubstring('the docs'));
    await settle(tester);
    expect(opened, [Uri.parse('https://example.com/docs')]);
    expect(focusModeOf(harness.backend).value, isTrue);
    expect(find.byType(FocusContext), findsOneWidget);
    expect(
      harness.backend.sent
          .skip(sent)
          .where((m) => m.hasSetLayout() || m.hasSelectAgent()),
      isEmpty,
    );
  });

  for (final how in ['Esc', 'Done']) {
    testWidgets('a file its agent expands while focus mode is on waits; '
        '$how leaves, and the file is there, in sight, taking clicks', (
      tester,
    ) async {
      final harness = await _focus(tester, const []);
      final opened = harness.backend.workspace.workspace.deepCopy()
        ..selectedAgentId = 'mista';
      final mista = opened.agents.firstWhere((agent) => agent.id == 'mista');
      final file = open(layoutOf(mista), '/w/round6.png');
      mista
        ..layout = file.root
        ..layoutFocus = file.paneId;
      harness.backend.workspace.apply(opened);
      await settle(tester);
      await tester.pump(const Duration(seconds: 1));
      if (how == 'Esc') {
        await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      } else {
        await tester.tap(find.text('Done').hitTestable().first);
      }
      await settle(tester);
      await tester.pump(const Duration(seconds: 1));
      expect(focusModeOf(harness.backend).value, isFalse);
      final back = find.text('Back to Mista');
      expect(back.hitTestable(), findsOneWidget);
      for (final opacity in tester.widgetList<Opacity>(
        find.ancestor(of: back, matching: find.byType(Opacity)),
      )) {
        expect(opacity.opacity, 1);
      }
      await tester.tap(back);
      await settle(tester);
      expect(back, findsNothing);
    });
  }
}

/// Focus mode is gone, Mista's conversation is asked for, and [path] is
/// opened in a pane of its layout; with the backend's answer, the window
/// shows it, in sight.
Future<void> _shown(WidgetTester tester, Harness harness, String path) async {
  await settle(tester);
  expect(focusModeOf(harness.backend).value, isFalse);
  expect(find.byType(FocusContext), findsNothing);
  final asked = harness.backend.sent.where((m) => m.hasSelectAgent());
  expect(asked.last.selectAgent.agentId, 'mista');
  final layout = harness.backend.sent.lastWhere((m) => m.hasSetLayout());
  expect(layout.setLayout.agentId, 'mista');
  final mista = harness.backend.workspace.agent('mista')!;
  expect(identical(open(layoutOf(mista), path).root, layoutOf(mista)), isTrue);
  await harness.apply(
    tester,
    harness.backend.workspace.workspace.deepCopy()..selectedAgentId = 'mista',
  );
  await settle(tester);
  await tester.pump(const Duration(seconds: 1));
  final name = path.split('/').last;
  expect(find.text(name).hitTestable(), findsWidgets);
}
