// The agent's header and menu (agent-view.md §3, §4): the name opens its
// settings, the model is a control with Undo, Checklist and Tasks are
// buttons with their counts, the meter explains itself, ⋯ holds the rest,
// and things give way in order when it's narrow. With no room beside the
// conversation, the panel comes over it.
import 'package:factory_app/agent_view.dart' show AgentView;
import 'package:factory_app/controls.dart' show HoverCard;
import 'package:factory_app/forms.dart' show AgentSettingsForm;
import 'package:factory_app/highlights.dart' show Glow;
import 'package:factory_app/ipad/effort_segments.dart';
import 'package:factory_app/model_select.dart' show ModelChooser;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';

const opus = 'anthropic/claude-opus-4-8';
const haiku = 'anthropic/claude-haiku-4-5';

Workspace header({
  String openPanel = '',
  int checklistOpen = 0,
  int tasksRunning = 0,
  int branches = 0,
  String role = 'Researcher',
}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  models: [
    Model(
      id: opus,
      name: 'Claude Opus 4.8',
      efforts: ['low', 'medium', 'high'],
    ),
    Model(id: haiku, name: 'Claude Haiku 4.5'),
  ],
  agents: [
    Agent(
      id: 'a1',
      name: 'June',
      role: role,
      projectId: 'p1',
      model: opus,
      effort: 'high',
      modelAvailable: true,
      openPanel: openPanel,
      checklistOpen: checklistOpen,
      tasksRunning: tasksRunning,
      branches: branches,
      contextTokens: Int64(212000),
      compactAtTokens: Int64(400000),
      extensions: [
        for (final id in ['agents', 'checklist', 'tasks', 'web'])
          AgentExtension(id: id, enabled: true),
      ],
    ),
  ],
  extensions: [
    Extension(id: 'agents', name: 'Messaging', icon: 'users'),
    Extension(id: 'checklist', name: 'Checklist', icon: 'list-checks'),
    Extension(id: 'tasks', name: 'Tasks', icon: 'terminal'),
    Extension(id: 'web', name: 'Web', icon: 'globe'),
  ],
  selectedAgentId: 'a1',
  openPanel: openPanel,
);

/// [text] in the agent's view, not its sidebar row.
Finder inView(String text) =>
    find.descendant(of: find.byType(AgentView), matching: find.text(text));

Finder said(String label) => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == label,
);

Future<Harness> showHeader(
  WidgetTester tester,
  Workspace snapshot, {
  Size size = const Size(2200, 900),
}) async {
  final harness = Harness(snapshot);
  await harness.show(tester, size: size);
  await settle(tester);
  return harness;
}

void main() {
  testWidgets('the name opens its settings; the bar itself is no button', (
    tester,
  ) async {
    await showHeader(tester, header());
    // An empty stretch of the bar: nothing opens.
    final menu = tester.getTopLeft(said('More options for June'));
    await tester.tapAt(menu - const Offset(120, 0));
    await settle(tester);
    expect(find.text('Agent settings'), findsNothing);
    await tester.tap(said('June, Researcher'));
    await settle(tester);
    expect(find.text('Agent settings'), findsOneWidget);
  });

  testWidgets('the model control changes the effort in place, with Undo for '
      'six seconds', (tester) async {
    final harness = await showHeader(tester, header());
    expect(find.text('Claude Opus 4.8 · High'), findsOneWidget);
    await tester.tap(said('Model: Claude Opus 4.8 · High'));
    await settle(tester);
    expect(find.text('Reasoning effort'), findsOneWidget);
    expect(find.text('More settings…'), findsOneWidget);
    // The effort's list, then Low.
    await tester.tap(find.text('High').last);
    await settle(tester);
    await tester.tap(find.text('Low').last);
    await settle(tester);
    expect(harness.last.updateAgent.agentId, 'a1');
    expect(harness.last.updateAgent.effort, 'low');
    expect(harness.last.updateAgent.hasModel(), isFalse);
    // The backend applies it; the popover stays open, the control says so.
    final applied = header()..agents.first.effort = 'low';
    await harness.apply(tester, applied);
    expect(find.text('Changed to Low · '), findsOneWidget);
    // Esc closes the popover.
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(find.text('More settings…'), findsNothing);
    expect(find.text('Changed to Low · '), findsOneWidget);
    await tester.tap(find.text('Undo'));
    await settle(tester);
    expect(harness.last.updateAgent.effort, 'high');
    expect(find.text('Changed to Low · '), findsNothing);

    // Another change: gone by itself after six seconds.
    await tester.tap(said('Model: Claude Opus 4.8 · Low'));
    await settle(tester);
    await tester.tap(find.text('Low').last);
    await settle(tester);
    await tester.tap(find.text('Medium').last);
    await settle(tester);
    await harness.apply(tester, header()..agents.first.effort = 'medium');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(find.text('Changed to Medium · '), findsOneWidget);
    await tester.pump(const Duration(seconds: 6));
    await settle(tester);
    expect(find.text('Changed to Medium · '), findsNothing);
    expect(find.text('Claude Opus 4.8 · Medium'), findsOneWidget);
  });

  testWidgets('a model without the effort snaps it to one it takes', (
    tester,
  ) async {
    final harness = await showHeader(tester, header());
    await tester.tap(said('Model: Claude Opus 4.8 · High'));
    await settle(tester);
    // The model list opens over the popover.
    await tester.tap(said('Model: Claude Opus 4.8, anthropic'));
    await settle(tester);
    await tester.tap(find.text('Claude Haiku 4.5').last);
    await settle(tester);
    expect(harness.last.updateAgent.model, haiku);
    expect(harness.last.updateAgent.effort, '', reason: 'Haiku has none');
    expect(harness.last.updateAgent.hasEffort(), isTrue);
    // The popover is still open, under the list that closed.
    expect(find.text('More settings…'), findsOneWidget);
  });

  testWidgets('Checklist shows its open items; Tasks only while tasks run', (
    tester,
  ) async {
    final harness = await showHeader(tester, header(checklistOpen: 4));
    expect(said('Checklist, 4 open'), findsOneWidget);
    expect(said('Tasks, 0 running'), findsNothing);
    await harness.apply(tester, header(checklistOpen: 4, tasksRunning: 2));
    expect(find.text('Tasks · 2 running'), findsOneWidget);
    await tester.tap(said('Tasks, 2 running'));
    await settle(tester);
    expect(harness.last.openPanel.extensionId, 'tasks');
    // Open, it's tinted, and a click closes it.
    await harness.apply(
      tester,
      header(checklistOpen: 4, tasksRunning: 2, openPanel: 'tasks'),
    );
    await tester.tap(said('Tasks, 2 running'));
    await settle(tester);
    expect(harness.last.openPanel.extensionId, '');
  });

  testWidgets('the meter explains itself and opens nothing', (tester) async {
    await showHeader(tester, header());
    expect(find.text('212k / 400k'), findsOneWidget);
    final card = tester.widget<HoverCard>(
      find.ancestor(
        of: find.text('212k / 400k'),
        matching: find.byType(HoverCard),
      ),
    );
    expect(
      (card.card(
        tester.element(find.text('212k / 400k')),
      ) as ConstrainedBox).child,
      isA<Text>().having(
        (text) => text.data,
        'words',
        'This conversation: 212k of 400k. At 400k, June summarizes it and '
            'carries on.',
      ),
    );
    await tester.tap(find.text('212k / 400k'));
    await settle(tester);
    expect(find.text('Agent settings'), findsNothing);
  });

  testWidgets('⋯ holds Find, Other versions, the panels with something to '
      'show and the settings, in order', (tester) async {
    final harness = await showHeader(
      tester,
      header(openPanel: 'agents', branches: 3, checklistOpen: 2),
    );
    await tester.tap(said('More options for June'));
    await settle(tester);
    final rows = [
      'Find…',
      'Other versions (3)',
      'Messaging',
      'Checklist',
      'Tasks',
      'Pin',
      'Agent settings…',
      'Extensions…',
    ];
    for (final (index, row) in rows.indexed.skip(1)) {
      expect(
        tester.getTopLeft(find.text(row).last).dy,
        greaterThan(tester.getTopLeft(find.text(rows[index - 1]).last).dy),
        reason: '$row after ${rows[index - 1]}',
      );
    }
    // Web only says what it does: it's in Agent settings, not here.
    expect(find.text('Web'), findsNothing);
    // The open panel is checked; choosing it closes it.
    expect(
      tester
          .widget<Semantics>(
            find
                .descendant(
                  of: find.byKey(const ValueKey('agent-menu:panel:agents')),
                  matching: find.byType(Semantics),
                )
                .first,
          )
          .properties
          .selected,
      isTrue,
    );
    await tester.tap(find.text('Messaging').last);
    await settle(tester);
    expect(harness.last.openPanel.extensionId, '');
    expect(find.text('Agent settings…'), findsNothing);
    // Esc closes it too.
    await tester.tap(said('More options for June'));
    await settle(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(find.text('Agent settings…'), findsNothing);
    // Down and Enter choose.
    await tester.tap(said('More options for June'));
    await settle(tester);
    for (var i = 0; i < 7; i++) {
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    }
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(find.text('Agent settings'), findsOneWidget);
    expect(
      tester
          .widget<AgentSettingsForm>(find.byType(AgentSettingsForm))
          .showExtensions,
      isFalse,
    );
  });

  // Abbacchio's row 1661: VoiceOver on a Mac can't reach the sidebar row's
  // custom actions, so ⋯ has Pin, Unpin and a pin's moves too.
  testWidgets('⋯ pins, unpins and moves a pin', (tester) async {
    final pinned = header()
      ..agents.add(Agent(id: 'a2', name: 'Fred', projectId: 'p1'))
      ..settings = Settings(pinnedAgents: ['a2', 'a1']);
    final harness = await showHeader(tester, pinned);
    await tester.tap(said('More options for June'));
    await settle(tester);
    // The last pin: Unpin and Move up, no Move down.
    expect(find.text('Unpin'), findsOneWidget);
    expect(find.text('Move down'), findsNothing);
    await tester.tap(find.text('Move up').last);
    await settle(tester);
    expect(harness.last.pinAgent.agentId, 'a1');
    expect(harness.last.pinAgent.beforeId, 'a2');
    await tester.tap(said('More options for June'));
    await settle(tester);
    await tester.tap(find.text('Unpin').last);
    await settle(tester);
    expect(harness.last.unpinAgent.agentId, 'a1');
    // Not pinned: Pin, and no moves.
    await harness.apply(tester, header());
    await tester.tap(said('More options for June'));
    await settle(tester);
    expect(find.text('Move up'), findsNothing);
    await tester.tap(find.text('Pin').last);
    await settle(tester);
    expect(harness.last.pinAgent.agentId, 'a1');
    expect(harness.last.pinAgent.beforeId, '');
  });

  // Abbacchio's row 1716: VoiceOver's VO-Space is the item's semantics
  // tap, which did nothing.
  testWidgets("⋯'s items do what they say from VoiceOver's tap", (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    final pinned = header()
      ..agents.add(Agent(id: 'a2', name: 'Fred', projectId: 'p1'))
      ..settings = Settings(pinnedAgents: ['a2', 'a1']);
    final harness = await showHeader(tester, pinned);
    await tester.tap(said('More options for June'));
    await settle(tester);
    final node = tester.getSemantics(find.bySemanticsLabel('Move up').last);
    expect(node.getSemanticsData().hasAction(SemanticsAction.tap), isTrue);
    tester.binding.renderViews.first.owner!.semanticsOwner!.performAction(
      node.id,
      SemanticsAction.tap,
    );
    await settle(tester);
    expect(harness.last.pinAgent.agentId, 'a1');
    expect(harness.last.pinAgent.beforeId, 'a2');
    expect(find.text('Move up'), findsNothing, reason: 'closed');
    semantics.dispose();
  });

  // Abbacchio's row 1647: Extensions… opened the settings at the top with
  // the extensions folded, as Agent settings… does.
  testWidgets('Extensions… opens the settings at its extensions, unfolded', (
    tester,
  ) async {
    await showHeader(tester, header());
    await tester.tap(said('More options for June'));
    await settle(tester);
    await tester.tap(find.text('Extensions…').last);
    await settle(tester);
    expect(
      tester
          .widget<AgentSettingsForm>(find.byType(AgentSettingsForm))
          .showExtensions,
      isTrue,
    );
  });

  testWidgets('what an agent points at glows on its button, else on ⋯', (
    tester,
  ) async {
    await showHeader(tester, header(tasksRunning: 1));
    bool glows(Finder target, String key) => tester
        .widget<Glow>(
          find.ancestor(of: target, matching: find.byType(Glow)).first,
        )
        .matches(key);
    final menu = said('More options for June');
    expect(glows(menu, 'extension:a1/agents'), isTrue);
    expect(glows(menu, 'extension:a1/checklist'), isFalse);
    expect(glows(menu, 'extension:a1/tasks'), isFalse, reason: 'its button');
    expect(glows(menu, 'extension:a2/agents'), isFalse, reason: 'another');
    final checklist = said('Checklist');
    expect(glows(checklist, 'extension:a1/checklist'), isTrue);
    expect(glows(checklist, 'checklist_item:a1#3'), isTrue);
  });

  testWidgets('narrow, things give way in order; the model and ⋯ stay', (
    tester,
  ) async {
    final snapshot = header(checklistOpen: 4, tasksRunning: 2);
    await showHeader(tester, snapshot);
    expect(inView('Tasks · 2 running'), findsOneWidget);
    expect(inView('212k / 400k'), findsOneWidget);
    expect(inView('Claude Opus 4.8 · High'), findsOneWidget);
    expect(inView('Researcher'), findsOneWidget);

    Future<void> width(double width) async {
      tester.view.physicalSize = Size(width, 900);
      tester.view.display.size = Size(width, 900);
      await settle(tester);
    }

    // Narrower: Tasks to its glyph and count, then Checklist, the meter,
    // the effort, the role, each before the next.
    final seen = <String>[];
    for (var w = 2200.0; w >= 800; w -= 20) {
      await width(w);
      expect(tester.takeException(), isNull, reason: 'at $w');
      for (final (gone, what) in [
        (inView('Tasks · 2 running'), 'tasks'),
        (inView('Checklist'), 'checklist'),
        (inView('212k / 400k'), 'meter'),
        (inView('Claude Opus 4.8 · High'), 'effort'),
        (inView('Researcher'), 'role'),
      ]) {
        if (gone.evaluate().isEmpty && !seen.contains(what)) seen.add(what);
      }
      expect(said('More options for June'), findsOneWidget, reason: 'at $w');
      expect(
        find.descendant(
          of: find.byType(AgentView),
          matching: find.textContaining('Claude Opus 4.8'),
        ),
        findsOneWidget,
      );
    }
    expect(seen, ['tasks', 'checklist', 'meter', 'effort', 'role']);
  });

  testWidgets('with no room beside the conversation the panel comes over it, '
      'and Esc closes it', (tester) async {
    final harness = await showHeader(
      tester,
      header(openPanel: 'checklist'),
      size: const Size(900, 800),
    );
    expect(find.byKey(const ValueKey('panel-over')), findsOneWidget);
    expect(said('Close Checklist'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(harness.last.openPanel.extensionId, '');
    // With room, beside it as before.
    await harness.show(tester, size: const Size(2200, 900));
    await settle(tester);
    expect(find.byKey(const ValueKey('panel-over')), findsNothing);
    expect(said('Close Checklist'), findsOneWidget);
  });

  testWidgets("on an iPad: the popover holds the models' list and the "
      "effort's segments, the menu a finger's rows", (tester) async {
    asIPad();
    final harness = await showHeader(
      tester,
      header(),
      size: const Size(1366, 1024),
    );
    await tester.tap(said('Model: Claude Opus 4.8 · High'));
    await settle(tester);
    expect(find.byType(ModelChooser), findsOneWidget);
    expect(find.byType(EffortSegments), findsOneWidget);
    // Two models: no search, and nothing raises the keyboard.
    expect(find.text('Search models or providers'), findsNothing);
    final haiku = find.text('Claude Haiku 4.5').last;
    expect(
      tester
          .getSize(
            find.ancestor(of: haiku, matching: find.byType(Container)).first,
          )
          .height,
      greaterThanOrEqualTo(44),
    );
    await tester.tap(haiku);
    await settle(tester);
    expect(harness.last.updateAgent.model, 'anthropic/claude-haiku-4-5');
    // It stays open for the effort.
    expect(find.byType(ModelChooser), findsOneWidget);
    await tester.tapAt(const Offset(1300, 1000));
    await settle(tester);
    expect(find.byType(ModelChooser), findsNothing);

    await tester.tap(said('More options for June'));
    await settle(tester);
    final row = find.byKey(const ValueKey('agent-menu:settings'));
    expect(tester.getSize(row).height, greaterThanOrEqualTo(44));
  });
}
