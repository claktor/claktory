// Zucchero's row 140: at an iPad's largest text, rows of a fixed height
// cut their words to the top half: a tool's header in a conversation, and
// the expanded pane's "Back to …" chip, an extension panel's title (the
// Viewer's about panel) and the agent bar's back chip. They grow with the
// text instead.
import 'package:factory_app/extensions.dart' show ExtensionPanel;
import 'package:factory_app/panes.dart' show ExpandedPane;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/transcript.dart' show TranscriptView;
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../phone_sizes_test.dart' show loadGeist;
import '../resources_test.dart' show resourcesWorkspace;
import '../tool_views_test.dart' show call;
import 'as_ipad.dart';
import 'window_fixtures.dart' show TextFactor;

/// Each paragraph under [of] laid out at least as tall as its lines.
void _noneCut(WidgetTester tester, Finder of) {
  final paragraphs = tester
      .renderObjectList<RenderParagraph>(
        find.descendant(of: of, matching: find.byType(RichText)),
      )
      .toList();
  expect(paragraphs, isNotEmpty);
  for (final p in paragraphs) {
    final natural = p
        .getDryLayout(BoxConstraints(maxWidth: p.size.width))
        .height;
    expect(
      p.size.height,
      greaterThanOrEqualTo(natural - 0.5),
      reason:
          '"${p.text.toPlainText()}" is cut to ${p.size.height} of $natural',
    );
  }
}

void main() {
  setUpAll(loadGeist);

  void atAX5(WidgetTester tester) {
    asIPad();
    tester.platformDispatcher.textScaleFactorTestValue = TextFactor.ax5;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  }

  testWidgets('at AX5 a tool call\'s header grows with its words', (
    tester,
  ) async {
    atAX5(tester);
    final harness = Harness(
      Workspace(
        agents: [Agent(id: 'a1', name: 'Mike', modelAvailable: true)],
        selectedAgentId: 'a1',
      ),
    );
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          call('bash', {'command': 'ls'}, output: 'README.md'),
          call('secrets_collect', {'name': 'Cloud'}, seq: 2),
          // A tool with a line of its own: "Showing notes.md".
          call(
            'show',
            {'path': 'notes.md'},
            seq: 3,
            details: {
              'path': '/Users/me/notes.md',
              'kind': 'text',
              'pane': 'v1',
              'agent': 'a1',
            },
          ),
        ],
      ),
    );
    await harness.show(tester, size: const Size(1376, 1032));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    expect(find.text('secrets_collect'), findsWidgets);
    expect(find.text('Showing'), findsWidgets);
    _noneCut(tester, find.byType(TranscriptView));
  });

  testWidgets('at AX5 the expanded pane\'s Back chip grows with its words', (
    tester,
  ) async {
    atAX5(tester);
    final agent = Agent(id: 'a1', name: 'Lea', modelAvailable: true);
    final harness = Harness(Workspace(agents: [agent], selectedAgentId: 'a1'));
    await harness.show(
      tester,
      size: const Size(1032, 1376),
      home: ExpandedPane(
        agent: agent,
        opened: const [],
        onBack: () {},
        child: const SizedBox.expand(),
      ),
    );
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    expect(find.text('Back to Lea'), findsOneWidget);
    _noneCut(tester, find.byType(ExpandedPane));
  });

  testWidgets('at AX5 an extension panel\'s title grows with its words', (
    tester,
  ) async {
    atAX5(tester);
    final harness = Harness(resourcesWorkspace());
    harness.backend.extensionView.apply(
      ExtensionView(
        extensionId: 'resources',
        agentId: 'a1',
        title: 'Resources',
        resources: Resources(),
      ),
    );
    await harness.show(tester, size: const Size(1376, 1032));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    final title = find.descendant(
      of: find.byType(ExtensionPanel),
      matching: find.text('Resources'),
    );
    expect(title, findsOneWidget);
    _noneCut(tester, title);
  });

  testWidgets('at AX5 the agent bar\'s back chip grows with its words', (
    tester,
  ) async {
    atAX5(tester);
    final harness = Harness(
      Workspace(
        agents: [
          Agent(id: 'a1', name: 'Mike', modelAvailable: true),
          Agent(id: 'a2', name: 'Lea', modelAvailable: true),
        ],
        // Lea's, come to from Mike's.
        selectedAgentId: 'a2',
      ),
    );
    await harness.show(tester, size: const Size(1376, 1032));
    await pumpFrames(tester);
    harness.history.follow('a1', 'a2');
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    final chip = find.byWidgetPredicate(
      (w) => w is Semantics && w.properties.label == 'Back to Mike',
    );
    expect(chip, findsOneWidget);
    _noneCut(tester, chip);
  });
}
