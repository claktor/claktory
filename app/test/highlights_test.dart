import 'package:factory_app/highlights.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'rivet_test.dart' show admin;

Agent member(
  String id,
  String name, {
  String color = 'green',
  String parent = '',
}) => Agent(
  id: id,
  name: name,
  projectId: 'p1',
  color: color,
  parentId: parent,
  modelAvailable: true,
  extensions: [AgentExtension(id: 'checklist', enabled: true)],
);

Workspace snapshot({
  bool highlights = true,
  List<Agent>? more,
  String selected = '',
  bool collapsed = false,
  String openPanel = '',
}) =>
    Workspace(
        highlights: highlights,
        projects: [
          Project(id: 'p1', name: 'Factory', path: '/Users/me/factory'),
        ],
        agents: [
          admin(),
          member('lead', 'Lead', color: 'red')..collapsed = collapsed,
          member('back', 'Back', color: 'teal', parent: 'lead'),
          ...?more,
        ],
        extensions: [
          Extension(id: 'checklist', name: 'Checklist', icon: 'list-checks'),
        ],
        selectedAgentId: selected,
        openPanel: openPanel,
      )
      ..agents
          .where((agent) => agent.id == selected)
          .forEach((agent) => agent.openPanel = openPanel);

/// How strongly the glow over [text]'s nearest [Glow] shows now, 0 to 1.
double strength(WidgetTester tester, String text) {
  final glow = find
      .ancestor(of: find.text(text).first, matching: find.byType(Glow))
      .first;
  final lights = find.descendant(
    of: glow,
    matching: find.byWidgetPredicate(
      (widget) => widget is Opacity && widget.child is DecoratedBox,
    ),
  );
  if (lights.evaluate().isEmpty) return 0;
  return tester.widget<Opacity>(lights.first).opacity;
}

/// The colors over [text]'s glow, at its middle.
List<Color> colors(WidgetTester tester, String text) {
  final glow = find
      .ancestor(of: find.text(text).first, matching: find.byType(Glow))
      .first;
  final box = tester.widget<DecoratedBox>(
    find
        .descendant(
          of: find.descendant(
            of: glow,
            matching: find.byWidgetPredicate(
              (widget) => widget is Opacity && widget.child is DecoratedBox,
            ),
          ),
          matching: find.byType(DecoratedBox),
        )
        .first,
  );
  final gradient =
      (box.decoration as BoxDecoration).gradient! as LinearGradient;
  return [for (final color in gradient.colors) color.withValues(alpha: 1)];
}

/// To the middle of a glow that just started: a frame to start it, then
/// on into it.
Future<void> midway(WidgetTester tester) async {
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 400));
}

/// Lets every glow and callout run out.
Future<void> outlast(WidgetTester tester) =>
    tester.pump(HighlightStore.calloutDuration + const Duration(seconds: 1));

void main() {
  testWidgets('a new agent glows as it appears, not on first load', (
    tester,
  ) async {
    final harness = Harness(snapshot());
    await harness.show(tester);
    await tester.pump(const Duration(milliseconds: 300));
    expect(strength(tester, 'Lead'), 0, reason: 'first load');

    await harness.apply(tester, snapshot(more: [member('scout', 'Scout')]));
    await midway(tester);
    expect(strength(tester, 'Scout'), greaterThan(0.5));
    expect(colors(tester, 'Scout').first, agentColor('green'));
    expect(strength(tester, 'Lead'), 0);
    // It fades out.
    await tester.pump(HighlightStore.duration);
    expect(strength(tester, 'Scout'), 0);
    await outlast(tester);
  });

  testWidgets('with highlights off nothing glows', (tester) async {
    final harness = Harness(snapshot(highlights: false));
    await harness.show(tester);
    await harness.apply(
      tester,
      snapshot(highlights: false, more: [member('scout', 'Scout')]),
    );
    await midway(tester);
    expect(strength(tester, 'Scout'), 0);
    harness.backend.highlights.apply(
      Highlight(
        kind: HighlightKind.HIGHLIGHT_KIND_AGENT,
        targets: [HighlightTarget(kind: 'agent', id: 'lead')],
        text: 'Here',
      ),
    );
    await midway(tester);
    expect(strength(tester, 'Lead'), 0);
    expect(find.text('Here'), findsNothing);
  });

  testWidgets("a message glows both agents' rows in both colors", (
    tester,
  ) async {
    final harness = Harness(snapshot(more: [member('solo', 'Solo')]));
    await harness.show(tester);
    harness.backend.highlights.apply(
      Highlight(
        kind: HighlightKind.HIGHLIGHT_KIND_MESSAGE,
        fromAgentId: 'lead',
        targets: [
          HighlightTarget(kind: 'agent', id: 'lead'),
          HighlightTarget(kind: 'agent', id: 'solo'),
        ],
      ),
    );
    await midway(tester);
    for (final name in ['Lead', 'Solo']) {
      expect(strength(tester, name), greaterThan(0.5), reason: name);
      expect(colors(tester, name), [agentColor('red'), agentColor('green')]);
    }
    await outlast(tester);
  });

  testWidgets('what an agent points out glows, with what it says', (
    tester,
  ) async {
    final harness = Harness(snapshot());
    await harness.show(tester);
    harness.backend.highlights.apply(
      Highlight(
        kind: HighlightKind.HIGHLIGHT_KIND_AGENT,
        fromAgentId: 'rivet',
        text: 'Sign in here first',
        targets: [
          HighlightTarget(kind: 'settings'),
          HighlightTarget(kind: 'project', id: 'p1'),
          HighlightTarget(kind: 'someday', id: 'x'),
        ],
      ),
    );
    await midway(tester);
    expect(strength(tester, 'Factory'), greaterThan(0.5));
    expect(find.text('Sign in here first'), findsOneWidget);
    // Clicking it puts it away.
    await tester.tap(find.text('Sign in here first'));
    await midway(tester);
    expect(find.text('Sign in here first'), findsNothing);
    await outlast(tester);
  });

  testWidgets('a callout goes away by itself after a while', (tester) async {
    final harness = Harness(snapshot());
    await harness.show(tester);
    harness.backend.highlights.apply(
      Highlight(
        kind: HighlightKind.HIGHLIGHT_KIND_AGENT,
        text: 'Lead is on it',
        targets: [HighlightTarget(kind: 'agent', id: 'lead')],
      ),
    );
    await midway(tester);
    expect(find.text('Lead is on it'), findsOneWidget);
    await outlast(tester);
    expect(find.text('Lead is on it'), findsNothing);
  });

  testWidgets('a folded-away agent glows on the one it is under', (
    tester,
  ) async {
    final harness = Harness(snapshot(collapsed: true));
    await harness.show(tester);
    expect(find.text('Back'), findsNothing);
    harness.backend.highlights.apply(
      Highlight(
        kind: HighlightKind.HIGHLIGHT_KIND_AGENT,
        targets: [HighlightTarget(kind: 'agent', id: 'back')],
      ),
    );
    await midway(tester);
    expect(strength(tester, 'Lead'), greaterThan(0.5));
    await outlast(tester);
  });

  testWidgets(
    'a checklist item that changes glows; another agent\'s does not',
    (tester) async {
      final harness = Harness(
        snapshot(selected: 'lead', openPanel: 'checklist'),
      );
      await harness.show(tester);
      ExtensionView view(String agent, ChecklistStatus status) => ExtensionView(
        extensionId: 'checklist',
        agentId: agent,
        checklist: Checklist(
          items: [
            ChecklistItem(
              id: 1,
              text: 'Ship $agent',
              status: status,
              created: Int64(DateTime.now().millisecondsSinceEpoch),
            ),
          ],
        ),
      );
      harness.backend.extensionView.apply(
        view('lead', ChecklistStatus.CHECKLIST_STATUS_OPEN),
      );
      await midway(tester);
      expect(strength(tester, 'Ship lead'), 0, reason: 'first shown');

      harness.backend.extensionView.apply(
        view('lead', ChecklistStatus.CHECKLIST_STATUS_IN_PROGRESS),
      );
      await midway(tester);
      expect(strength(tester, 'Ship lead'), greaterThan(0.5));
      await outlast(tester);

      // Switching to Back shows its checklist: nothing changed.
      await harness.apply(
        tester,
        snapshot(selected: 'back', openPanel: 'checklist'),
      );
      harness.backend.extensionView.apply(
        view('back', ChecklistStatus.CHECKLIST_STATUS_DONE),
      );
      await midway(tester);
      expect(strength(tester, 'Ship back'), 0);
      await outlast(tester);
    },
  );

  testWidgets('asking Rivet glows its row and says what to do', (tester) async {
    final harness = Harness(snapshot());
    await harness.show(tester);
    await tester.tap(askRivetButtons().first);
    await midway(tester);
    expect(strength(tester, 'Rivet'), greaterThan(0.5));
    expect(find.text('Tell Rivet what you want to add'), findsOneWidget);
    await outlast(tester);
  });

  testWidgets('with reduced motion a glow is a brief still tint', (
    tester,
  ) async {
    tester.platformDispatcher.accessibilityFeaturesTestValue =
        const FakeAccessibilityFeatures(disableAnimations: true);
    addTearDown(tester.platformDispatcher.clearAccessibilityFeaturesTestValue);
    final harness = Harness(snapshot());
    await harness.show(tester);
    harness.backend.highlights.apply(
      Highlight(
        kind: HighlightKind.HIGHLIGHT_KIND_AGENT,
        targets: [HighlightTarget(kind: 'agent', id: 'lead')],
      ),
    );
    await tester.pump(const Duration(milliseconds: 16));
    final first = strength(tester, 'Lead');
    await midway(tester);
    expect(first, greaterThan(0));
    expect(strength(tester, 'Lead'), first, reason: 'still, not fading');
    await tester.pump(const Duration(seconds: 1));
    expect(strength(tester, 'Lead'), 0);
    await outlast(tester);
  });

  testWidgets('highlights are turned off in Settings', (tester) async {
    final harness = Harness(snapshot());
    await harness.show(tester);
    harness.page.value = AppPage.settings;
    await settle(tester);
    await tester.ensureVisible(find.text('Display'));
    await settle(tester);
    await tester.tap(
      find
          .byWidgetPredicate(
            (widget) =>
                widget is Semantics && widget.properties.label == 'Highlights',
          )
          .first,
    );
    await settle(tester);
    expect(harness.last.setHighlights.on, isFalse);
  });
}
