// The iPad sweep (Prosciutto's S5 for the store): every screen of the app,
// as an iPad shows it, at every window the iPad gives it (Slide Over,
// Split View's thirds and halves, Stage Manager and windowed widths, full
// screen upright and on its side) and at the standard and the largest
// text: nothing overflows. What overflows is collected per screen and
// listed at the end, rather than stopping at the first.
import 'dart:async';
import 'dart:convert';

import 'package:factory_app/controls.dart' show showModal;
import 'package:factory_app/forms.dart';
import 'package:factory_app/ipad/columns.dart' show WorkbenchTier;
import 'package:factory_app/your_claktories.dart' show showConfirmSignIn;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart' show ClaktoryRail;
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/services.dart' show LogicalKeyboardKey;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../phone_sizes_test.dart' show loadGeist;
import 'as_ipad.dart';
import 'window_fixtures.dart';

const _notes = '/w/field-notes-from-the-spring-survey-of-the-river.md';

/// Every extension a Claktory has, with its panel where it has one.
const _extensions = <(String, String, String)>[
  ('agents', 'Messaging', 'users'),
  ('ask', 'Ask', 'message-circle-question'),
  ('checklist', 'Checklist', 'list-checks'),
  ('comments', 'Comments', 'message-square-text'),
  ('database', 'Database', 'database'),
  ('devices', 'Devices', 'monitor-smartphone'),
  ('handoff', 'Reports', 'inbox'),
  ('highlight', 'Highlights', 'sparkles'),
  ('memory', 'Memory', 'brain'),
  ('monitors', 'Monitors', 'alarm-clock'),
  ('move', 'Move', 'send'),
  ('pipes', 'Pipes', 'network'),
  ('resources', 'Resources', 'gauge'),
  ('secrets', 'Secrets', 'key-round'),
  ('swarm', 'Team health', 'chart-column'),
  ('tasks', 'Tasks', 'terminal'),
  ('tree', 'Own checkout', 'git-branch'),
  ('viewer', 'Viewer', 'panels-top-left'),
  ('web', 'Web', 'globe'),
];

List<AgentExtension> _on() => [
  for (final (id, _, _) in _extensions) AgentExtension(id: id, enabled: true),
];

Workspace _workspace() => Workspace(
  extensions: [
    for (final (id, name, icon) in _extensions)
      Extension(id: id, name: name, icon: icon),
  ],
  projects: [
    Project(id: 'p1', name: 'Field Notes from the river meadows', path: '/w'),
  ],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
    Agent(
      id: 'theo',
      name: 'Theodora Featherstonehaugh',
      role: 'Researcher of the spring survey',
      projectId: 'p1',
      extensions: _on(),
      modelAvailable: true,
      color: 'red',
      layout: ViewNode(
        id: 's1',
        split: ViewSplit(
          axis: ViewAxis.VIEW_AXIS_ROW,
          children: [
            ViewNode(
              id: 'chat',
              leaf: ViewLeaf(kind: 'chat'),
            ),
            ViewNode(
              id: 'notes',
              leaf: ViewLeaf(
                kind: 'text',
                path: _notes,
                title: _notes.split('/').last,
              ),
            ),
          ],
        ),
      ),
    ),
    Agent(
      id: 'maya',
      name: 'Maya',
      role: 'Writer',
      projectId: 'p1',
      modelAvailable: true,
    ),
  ],
  selectedAgentId: 'theo',
);

/// The windows: each of the iPad's at the largest text, where things
/// overflow first, and a few at their own text too (on its side, a mini
/// upright, a Split View half).
final _windows = [
  for (final fixture in windowFixtures)
    if (fixture.idiom.name == 'pad') ...[
      (fixture: fixture, text: TextFactor.ax5),
      if (fixture.textFactor != TextFactor.ax5 &&
          const {
            'pro13-landscape-1376',
            'mini-portrait-744',
            'split-half-678',
          }.contains(fixture.name))
        (fixture: fixture, text: fixture.textFactor),
    ],
];

final _letter = RegExp(r'\p{L}', unicode: true);

/// The longest word every column should hold whole, at any text size:
/// "conversations".
const _everydayWord = 13;

/// A column narrower than this share of the window sits beside something
/// (a label column, a grid): it could stack instead of breaking words.
const _sideColumn = 0.6;

/// The everyday words laid out on screen broken across two lines between
/// two letters, as "Clakto|ry": text squeezed into a column narrower than
/// a word at its size. Each line after a paragraph's first starts where
/// [RenderParagraph.getPositionForOffset] finds it at its left edge; a
/// single-line paragraph (cut with an ellipsis) is left out, and so are a
/// word longer than [_everydayWord] letters (a long name) and a paragraph
/// as wide as most of the window ([_sideColumn]).
List<String> midWordBreaks(WidgetTester tester) {
  final found = <String>[];
  final window = tester.view.physicalSize.width / tester.view.devicePixelRatio;
  for (final element in find.byType(RichText).evaluate()) {
    final paragraph = element.renderObject;
    if (paragraph is! RenderParagraph ||
        !paragraph.hasSize ||
        !paragraph.attached ||
        paragraph.maxLines == 1 ||
        // Most of the window's width and still too narrow for a word at
        // this size (a title in Slide Over): nothing could hold it whole.
        paragraph.size.width >= window * _sideColumn) {
      continue;
    }
    final text = paragraph.text.toPlainText(includeSemanticsLabels: false);
    if (text.length < 2) continue;
    final boxes = paragraph.getBoxesForSelection(
      TextSelection(baseOffset: 0, extentOffset: text.length),
    );
    // The lines, top to bottom: boxes that overlap down the page are one
    // line (a bold word, an emoji), its left edge the leftmost.
    final sorted = [...boxes]..sort((a, b) => a.top.compareTo(b.top));
    final lines = <({double top, double bottom, double left})>[];
    for (final box in sorted) {
      final last = lines.isEmpty ? null : lines.last;
      if (last != null && box.top < last.bottom - 1) {
        lines.last = (
          top: last.top,
          bottom: box.bottom > last.bottom ? box.bottom : last.bottom,
          left: box.left < last.left ? box.left : last.left,
        );
      } else {
        lines.add((top: box.top, bottom: box.bottom, left: box.left));
      }
    }
    for (final line in lines.skip(1)) {
      final at = paragraph
          .getPositionForOffset(
            Offset(line.left + 0.5, (line.top + line.bottom) / 2),
          )
          .offset;
      if (at <= 0 || at >= text.length) continue;
      if (!_letter.hasMatch(text[at - 1]) || !_letter.hasMatch(text[at])) {
        continue;
      }
      // A word longer than any line could hold at this size (a long name)
      // breaks wherever the text goes; an everyday word must not.
      var start = at, end = at;
      while (start > 0 && _letter.hasMatch(text[start - 1])) {
        start--;
      }
      while (end < text.length && _letter.hasMatch(text[end])) {
        end++;
      }
      if (end - start <= _everydayWord) {
        final from = (at - 12).clamp(0, text.length);
        final to = (at + 12).clamp(0, text.length);
        found.add(
          '${text.substring(from, at)}|${text.substring(at, to)}'.replaceAll(
            '\n',
            ' ',
          ),
        );
      }
    }
  }
  return found;
}

void main() {
  setUpAll(loadGeist);

  final found = <String>[];
  tearDownAll(() {
    if (found.isEmpty) return;
    // ignore: avoid_print
    print('SWEEP found ${found.length}:\n${found.join('\n')}');
  });

  for (final (:fixture, :text) in _windows) {
    final name = '$fixture at ${text.toStringAsFixed(2)}x';
    testWidgets('$name: every screen fits', (tester) async {
      asIPad();
      tester.platformDispatcher.textScaleFactorTestValue = text;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = Harness(_workspace());
      final body = utf8.encode(
        '# The spring survey, written up for the next walk\n\n'
        '${'A line of what was counted along the transects. ' * 20}\n',
      );
      harness.backend.files[_notes] = FileContent(
        path: _notes,
        mime: 'text/markdown',
        size: Int64(body.length),
        data: body,
        sha256: 'v1',
      );
      final focus = WorkbenchFocus();
      addTearDown(focus.dispose);

      final problems = <String>[];
      var screen = 'the workbench';
      final was = FlutterError.onError;
      FlutterError.onError = (details) {
        final what = details.exceptionAsString().split('\n').first;
        final at = RegExp(r'lib/[\w/]+\.dart:\d+')
            .firstMatch(details.toString())
            ?.group(0);
        final line = '$name, $screen: $what ${at ?? ''}';
        if (!problems.contains(line)) problems.add(line);
      };
      addTearDown(() => FlutterError.onError = was);

      await harness.show(
        tester,
        size: fixture.size,
        home: WorkbenchScope(
          focus: focus,
          child: const Shell(rail: ClaktoryRail()),
        ),
      );
      await pumpFrames(tester);

      // A word broken across lines in the middle ("Clakto / ry"): text
      // squeezed narrower than a word at this size.
      void checkWords() {
        for (final broken in midWordBreaks(tester)) {
          final line = '$name, $screen: breaks mid-word "$broken"';
          if (!problems.contains(line)) problems.add(line);
        }
      }

      Future<void> scrollAll() async {
        checkWords();
        for (var i = 0; i < 12; i++) {
          final scrollables = find.byType(Scrollable);
          if (scrollables.evaluate().isEmpty) break;
          await tester.drag(
            scrollables.first,
            const Offset(0, -500),
            warnIfMissed: false,
          );
          await tester.pump(const Duration(milliseconds: 50));
          checkWords();
        }
        await pumpFrames(tester);
        checkWords();
      }

      checkWords();

      // The sidebar where it opens over the window.
      final toggle = find.byKey(const ValueKey('workbench-sidebar'));
      if (toggle.evaluate().isNotEmpty) {
        screen = 'the sidebar open';
        await tester.tap(toggle.first, warnIfMissed: false);
        await pumpFrames(tester);
        final scrim = find.byKey(const ValueKey('workbench-scrim'));
        if (scrim.evaluate().isNotEmpty) {
          await tester.tap(scrim.first, warnIfMissed: false);
          await pumpFrames(tester);
        }
      }

      for (final page in [
        AppPage.settings,
        AppPage.inbox,
        AppPage.usage,
        AppPage.changelog,
        AppPage.marketplace,
        AppPage.map,
      ]) {
        screen = page.name;
        harness.page.value = page;
        await pumpFrames(tester);
        await scrollAll();
      }
      harness.page.value = AppPage.agent;
      await pumpFrames(tester);

      // Each extension's panel, open beside the conversation.
      for (final (id, _, _) in _extensions) {
        screen = 'the $id panel';
        final open = _workspace()..openPanel = id;
        open.agents.firstWhere((agent) => agent.id == 'theo').openPanel = id;
        await harness.apply(tester, open);
        await pumpFrames(tester);
        await scrollAll();
      }

      // The forms and dialogs, then with the keyboard up (a docked one
      // takes about 40% of the window): what's typed in and the buttons
      // stay above it.
      final dialogs = <String, Future<void> Function(BuildContext)>{
        'New project': (context) => showModal(
          context,
          (context, close) => CreateProjectForm(close: close),
        ),
        'New agent': (context) => showModal(
          context,
          (context, close) => CreateAgentForm(projectId: 'p1', close: close),
        ),
        'agent settings': (context) => showModal(
          context,
          (context, close) =>
              AgentSettingsForm(agent: _workspace().agents[1], close: close),
        ),
        'project settings': (context) => showModal(
          context,
          (context, close) => ProjectSettingsForm(
            project: _workspace().projects.first,
            close: close,
          ),
        ),
        'sign-in': (context) => showConfirmSignIn(context, 'Studio'),
      };
      for (final MapEntry(key: title, value: open) in dialogs.entries) {
        for (final keyboard in [0.0, fixture.size.height * 0.4]) {
          screen = keyboard == 0 ? title : '$title, keyboard up';
          tester.view.viewInsets = FakeViewPadding(bottom: keyboard);
          final context = tester.element(find.byType(Shell));
          final barriers = find.byType(ModalBarrier).evaluate().length;
          unawaited(open(context));
          await pumpFrames(tester);
          await scrollAll();
          // Esc closes it, whatever had the keys under it.
          await tester.sendKeyEvent(LogicalKeyboardKey.escape);
          await pumpFrames(tester);
          if (find.byType(ModalBarrier).evaluate().length != barriers) {
            problems.add('${fixture.name}, $screen: Esc left it open');
          }
          tester.view.viewInsets = FakeViewPadding.zero;
          // The dialog goes with a fresh app.
          await harness.show(
            tester,
            size: fixture.size,
            home: WorkbenchScope(
              focus: focus,
              child: const Shell(rail: ClaktoryRail()),
            ),
          );
          await pumpFrames(tester);
        }
      }

      FlutterError.onError = was;
      found.addAll(problems);
      expect(problems, isEmpty, reason: problems.join('\n'));
      expect(fixture.tier, isA<WorkbenchTier>());
    });
  }
}
