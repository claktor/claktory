import 'package:factory_app/controls.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/extension_about.dart';
import 'package:factory_app/extensions.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final agentsExtension = Extension(
  id: 'agents',
  name: 'Agents',
  description: 'Messaging between agents.',
  defaultEnabled: true,
  icon: 'users',
  options: [
    ExtensionOption(
      id: 'reachable',
      label: 'Other agents can message it',
      kind: ExtensionOptionKind.EXTENSION_OPTION_KIND_SWITCH,
      defaultValue: true,
    ),
    ExtensionOption(
      id: 'admin',
      label: 'Admin: can see and message every agent',
      kind: ExtensionOptionKind.EXTENSION_OPTION_KIND_SWITCH,
    ),
    ExtensionOption(
      id: 'also',
      label: 'Can also see and message',
      kind: ExtensionOptionKind.EXTENSION_OPTION_KIND_AGENTS,
      includesProject: true,
    ),
  ],
);

final checklistExtension = Extension(
  id: 'checklist',
  name: 'Checklist',
  description: 'A to-do list of everything the person asked for.',
  icon: 'list-checks',
);

AgentExtension agentsOn({List<String> also = const []}) => AgentExtension(
  id: 'agents',
  enabled: true,
  options: {
    'reachable': OptionValue(on: true),
    'admin': OptionValue(on: false),
    'also': OptionValue(agentIds: also),
  }.entries,
);

Agent agent(String id, String name, String project, {bool checklist = true}) =>
    Agent(
      id: id,
      name: name,
      projectId: project,
      modelAvailable: true,
      extensions: [
        agentsOn(),
        AgentExtension(id: 'checklist', enabled: checklist),
      ],
    );

Workspace workspace({
  String openPanel = '',
  String focused = '',
  bool checklist = true,
}) => Workspace(
  projects: [
    Project(id: 'p1', name: 'Factory', path: '/Users/me/factory'),
    Project(id: 'p2', name: 'Site', path: '/Users/me/site'),
  ],
  agents: [
    agent('rivet', 'Rivet', ''),
    // The selected agent, with the panel it has open.
    agent('a1', 'Builder', 'p1', checklist: checklist)..openPanel = openPanel,
    agent('a2', 'Tester', 'p1'),
    agent('a3', 'Writer', 'p2'),
  ],
  extensions: [agentsExtension, checklistExtension],
  selectedAgentId: 'a1',
  focusedProjectId: focused,
  openPanel: openPanel,
);

final now = DateTime.now();
Int64 at(DateTime time) => Int64(time.millisecondsSinceEpoch);

ExtensionView checklistView({String agentId = 'a1'}) {
  final today = DateTime(now.year, now.month, now.day, 0, 30);
  final yesterday = today.subtract(const Duration(days: 1));
  return ExtensionView(
    extensionId: 'checklist',
    agentId: agentId,
    title: 'Checklist',
    checklist: Checklist(
      items: [
        ChecklistItem(
          id: 1,
          text: 'Fix the header gap',
          status: ChecklistStatus.CHECKLIST_STATUS_DONE,
          owner: 'a1',
          created: at(yesterday),
          log: [
            ChecklistUpdate(time: at(yesterday), text: 'Added'),
            ChecklistUpdate(
              time: at(yesterday),
              text: 'Done: shipped',
              author: 'a1',
            ),
          ],
        ),
        ChecklistItem(
          id: 2,
          text: 'Usage circles',
          status: ChecklistStatus.CHECKLIST_STATUS_IN_PROGRESS,
          owner: 'a2',
          created: at(today),
        ),
        ChecklistItem(id: 3, text: 'Someday', created: at(today)),
      ],
    ),
  );
}

Finder glyphButton(String label) => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == label,
);

Future<Harness> showWith(WidgetTester tester, Workspace snapshot) async {
  final harness = Harness(snapshot);
  await harness.show(tester);
  await settle(tester);
  return harness;
}

void main() {
  testWidgets('extension icons are drawn, not lettered', (tester) async {
    for (final icon in [
      'users',
      'list-checks',
      'brain',
      'terminal',
      'sparkles',
      'git-branch',
      'scroll-text',
      'alarm-clock',
      'book-open',
      'database',
      'globe',
      // The app's own: the Map, and the devices messages come from.
      'network',
      'monitor',
      'laptop-minimal',
      'smartphone',
      'tablet',
      'monitor-smartphone',
      'paperclip',
      'message-square',
      'play',
      'pause',
      'film',
    ]) {
      await tester.pumpWidget(
        Directionality(
          textDirection: TextDirection.ltr,
          child: ExtensionGlyph(
            icon: icon,
            name: 'Zed',
            color: const Color(0xFFFFFFFF),
          ),
        ),
      );
      expect(find.text('Z'), findsNothing, reason: icon);
    }
    // The sparkle's arcs and closing parse into the whole star.
    final star = svgPath(
      'M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962'
      'L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0'
      'L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964'
      'L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z',
    ).getBounds();
    expect(star.left, closeTo(2, 0.1));
    expect(star.right, closeTo(22, 0.1));
    expect(star.top, closeTo(2, 0.1));
    expect(star.bottom, closeTo(22, 0.1));
  });

  testWidgets("⋯ lists the agent's panels and opens one; Checklist is "
      'its own button', (tester) async {
    final harness = await showWith(tester, workspace(checklist: false));
    expect(glyphButton('Checklist'), findsNothing, reason: "it's off");
    // The panels with something to show, then its settings.
    await tester.tap(glyphButton('More options for Builder'));
    await settle(tester);
    expect(find.text('Agents'), findsOneWidget);
    expect(find.text('Checklist'), findsNothing);
    expect(find.text('Agent settings…'), findsOneWidget);
    expect(find.text('Extensions…'), findsOneWidget);
    await tester.tap(find.text('Agents'));
    await settle(tester);
    expect(harness.last.openPanel.extensionId, 'agents');
    expect(harness.last.openPanel.agentId, 'a1', reason: "Builder's panel");
    expect(find.text('Agent settings…'), findsNothing, reason: 'it closed');

    await harness.apply(tester, workspace());
    expect(glyphButton('Checklist'), findsOneWidget);
    await tester.tap(glyphButton('Checklist'));
    await settle(tester);
    expect(harness.last.openPanel.extensionId, 'checklist');

    // Open: clicking it again closes it.
    await harness.apply(tester, workspace(openPanel: 'checklist'));
    await tester.tap(glyphButton('Checklist'));
    await settle(tester);
    expect(harness.last.openPanel.extensionId, '');
  });

  testWidgets('the checklist panel groups items by day and opens logs', (
    tester,
  ) async {
    final harness = await showWith(tester, workspace(openPanel: 'checklist'));
    // Nothing made up before the backend sends the view.
    expect(find.text('Fix the header gap'), findsNothing);
    harness.backend.extensionView.apply(checklistView(agentId: 'a2'));
    await settle(tester);
    expect(
      find.text('Usage circles'),
      findsNothing,
      reason: "another agent's view",
    );

    harness.backend.extensionView.apply(checklistView());
    await settle(tester);
    // The history: finished items under the day they were added.
    expect(find.text('Yesterday'), findsOneWidget);
    // Then everything outstanding, in progress first, each with its day.
    expect(find.text('Outstanding'), findsOneWidget);
    double top(String text) => tester.getTopLeft(find.text(text)).dy;
    expect(top('Yesterday'), lessThan(top('Fix the header gap')));
    expect(top('Fix the header gap'), lessThan(top('Outstanding')));
    expect(top('Outstanding'), lessThan(top('Usage circles')));
    expect(top('Usage circles'), lessThan(top('Someday')));
    expect(find.text('Today'), findsNWidgets(2), reason: 'when each was added');
    // Progress: overall, and each day's.
    expect(find.text('1 of 3 done · 1 in progress · 1 open'), findsOneWidget);
    expect(find.text('All done'), findsOneWidget, reason: "yesterday's item");
    expect(find.text('#2'), findsOneWidget);
    expect(find.text('In progress'), findsOneWidget);
    expect(find.text('Tester'), findsWidgets);
    expect(find.text('Unassigned'), findsOneWidget);

    expect(find.textContaining('Done: shipped'), findsNothing);
    await tester.tap(find.text('Fix the header gap'));
    await settle(tester);
    expect(find.textContaining('Done: shipped'), findsOneWidget);
    expect(find.textContaining('You'), findsOneWidget);

    await tester.tap(glyphButton('Close Checklist'));
    await settle(tester);
    expect(harness.last.openPanel.extensionId, '');
  });

  testWidgets('blocked items wait after the ones in progress', (tester) async {
    final harness = await showWith(tester, workspace(openPanel: 'checklist'));
    final view = checklistView();
    view.checklist.items.add(
      ChecklistItem(
        id: 4,
        text: 'Publish the site',
        status: ChecklistStatus.CHECKLIST_STATUS_BLOCKED,
        created: at(now),
        note: 'Waiting on the domain to transfer',
      ),
    );
    harness.backend.extensionView.apply(view);
    await settle(tester);
    double top(String text) => tester.getTopLeft(find.text(text)).dy;
    expect(top('Usage circles'), lessThan(top('Publish the site')));
    expect(top('Publish the site'), lessThan(top('Someday')));
    expect(find.text('Blocked'), findsOneWidget);
    expect(glyphButton('Blocked'), findsOneWidget, reason: 'its own mark');
    // What it waits on, under its text.
    expect(
      top('Publish the site'),
      lessThan(top('Waiting on the domain to transfer')),
    );
    expect(top('Waiting on the domain to transfer'), lessThan(top('Blocked')));
    expect(
      find.text('1 of 4 done · 1 in progress · 1 blocked · 1 open'),
      findsOneWidget,
    );
  });

  testWidgets('each agent shows the panel it has open', (tester) async {
    final snapshot = workspace(focused: 'p1', openPanel: 'checklist');
    final harness = await showWith(tester, snapshot);
    await settle(tester);
    expect(glyphButton('Close Checklist'), findsOneWidget);

    // Tester has none open.
    await harness.apply(tester, snapshot.deepCopy()..selectedAgentId = 'a2');
    await settle(tester);
    expect(glyphButton('Close Checklist'), findsNothing);
    // Opening one there opens Tester's.
    await tester.tap(glyphButton('Checklist'));
    await settle(tester);
    expect(harness.last.openPanel.agentId, 'a2');

    // Back to Builder, its panel is still open.
    await harness.apply(tester, snapshot);
    await settle(tester);
    expect(glyphButton('Close Checklist'), findsOneWidget);
    await tester.tap(glyphButton('Close Checklist'));
    await settle(tester);
    expect(harness.last.openPanel.agentId, 'a1');
    expect(harness.last.openPanel.extensionId, '');
  });

  testWidgets('a panel from a newer Claktory says so', (tester) async {
    final harness = await showWith(tester, workspace(openPanel: 'checklist'));
    harness.backend.extensionView.apply(
      ExtensionView(extensionId: 'checklist', agentId: 'a1', title: 'Later'),
    );
    await settle(tester);
    expect(find.text('This panel needs a newer Claktory.'), findsOneWidget);
  });

  testWidgets('an extension with no view of its own says what it does', (
    tester,
  ) async {
    final snapshot = workspace(openPanel: 'tree')
      ..extensions.add(
        Extension(
          id: 'tree',
          name: 'Own checkout',
          description: 'A checkout of the repository of its own.',
          icon: 'git-branch',
          options: [
            ExtensionOption(
              id: 'land',
              label: 'Can land on main',
              kind: ExtensionOptionKind.EXTENSION_OPTION_KIND_SWITCH,
              defaultValue: true,
            ),
          ],
        ),
      );
    snapshot.agents[1].extensions.add(
      AgentExtension(
        id: 'tree',
        enabled: true,
        options: {'land': OptionValue(on: false)}.entries,
      ),
    );
    await showWith(tester, snapshot);
    expect(glyphButton('Close Own checkout'), findsOneWidget);
    expect(
      find.text('A checkout of the repository of its own.'),
      findsOneWidget,
    );
    expect(find.text('Can land on main'), findsOneWidget);
    expect(find.text('Off'), findsOneWidget);
    await tester.tap(find.text('Open agent settings'));
    await settle(tester);
    expect(find.text('Message box hint'), findsOneWidget);
  });

  test('instances of extensions with views have views', () {
    expect(hasExtensionView(checklistExtension), isTrue);
    expect(
      hasExtensionView(Extension(id: 'database:factory', base: 'database')),
      isTrue,
    );
    expect(hasExtensionView(Extension(id: 'web')), isFalse);
  });

  testWidgets('a panel shows only while open, for an agent that has it', (
    tester,
  ) async {
    final harness = await showWith(tester, workspace(focused: 'p1'));
    expect(glyphButton('Close Checklist'), findsNothing);

    await harness.apply(
      tester,
      workspace(focused: 'p1', openPanel: 'checklist'),
    );
    expect(glyphButton('Close Checklist'), findsOneWidget);

    // An agent without the extension shows no panel.
    await harness.apply(
      tester,
      workspace(focused: 'p1', openPanel: 'checklist', checklist: false),
    );
    expect(glyphButton('Close Checklist'), findsNothing);
  });

  testWidgets('the marketplace lists the extensions', (tester) async {
    final harness = await showWith(tester, workspace());
    harness.page.value = AppPage.marketplace;
    await settle(tester);
    await tester.tap(find.text('Extensions').first);
    await settle(tester);
    expect(find.text('Agents'), findsOneWidget);
    expect(find.text('Messaging between agents.'), findsOneWidget);
    expect(
      find.text("On for every agent. Turn it off in an agent's settings."),
      findsOneWidget,
    );
    expect(find.text("Turn it on in an agent's settings."), findsOneWidget);
    expect(find.text('No extensions yet.'), findsNothing);
  });

  testWidgets('agent settings turn extensions and their options on and off', (
    tester,
  ) async {
    final harness = await showWith(tester, workspace(checklist: false));
    await tester.tap(find.text('Builder').last);
    await settle(tester);
    // Folded at first: how many are on, and Change… to unfold them.
    expect(find.text('Extensions'), findsOneWidget);
    expect(find.text('Other agents can message it'), findsNothing);
    await pumpFrames(tester);
    await unfoldExtensions(tester);
    // Checklist is off, so its options (none) and Agents' show only for
    // Agents.
    expect(find.text('Other agents can message it'), findsOneWidget);

    // The form is taller than the window here, so it scrolls to each.
    Future<void> reach(Finder target) async {
      await tester.ensureVisible(target);
      await pumpFrames(tester);
    }

    await reach(glyphButton('Checklist'));
    await tester.tap(glyphButton('Checklist'));
    await settle(tester);
    await reach(glyphButton('Admin: can see and message every agent'));
    await tester.tap(glyphButton('Admin: can see and message every agent'));
    await settle(tester);

    // The agent list: its project's agents are fixed, others can be chosen.
    await reach(find.text('No one outside its project'));
    await tester.tap(find.text('No one outside its project'));
    await settle(tester);
    expect(find.text('Factory · in its project'), findsOneWidget);
    expect(find.text('Builder'), findsWidgets);
    await tester.tap(find.text('Tester').last);
    await settle(tester);
    expect(harness.backend.sent, isEmpty, reason: 'fixed, and not sent yet');
    await tester.tap(find.text('Writer').last);
    await settle(tester);
    // It stays open for more choices until a click outside it.
    expect(find.text('Factory · in its project'), findsOneWidget);
    await reach(find.text('Can also see and message'));
    await tester.tap(find.text('Can also see and message'));
    await settle(tester);
    expect(find.text('Factory · in its project'), findsNothing);
    expect(find.text('Writer'), findsWidgets, reason: 'as a chip');

    await tester.tap(find.text('Save'));
    await settle(tester);
    final changes = [
      for (final message in harness.backend.sent)
        message.updateAgent.extension_8,
    ];
    expect(changes, hasLength(3));
    expect(changes[0].id, 'agents');
    expect(changes[0].optionId, 'admin');
    expect(changes[0].value.on, isTrue);
    expect(changes[1].optionId, 'also');
    expect(changes[1].value.agentIds, ['a3']);
    expect(changes[2].id, 'checklist');
    expect(changes[2].hasEnabled(), isTrue);
    expect(changes[2].enabled, isTrue);
  });

  testWidgets('an admin agent is grouped as Admin, not Workspace', (
    tester,
  ) async {
    final snapshot = workspace();
    snapshot.agents[0].extensions[0].options['admin'] = OptionValue(on: true);
    await showWith(tester, snapshot);
    // In the list of agents to choose from.
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    await unfoldExtensions(tester);
    await tester.ensureVisible(find.text('No one outside its project'));
    await pumpFrames(tester);
    await tester.tap(find.text('No one outside its project'));
    await settle(tester);
    expect(find.text('Admin'), findsOneWidget);
    expect(find.text('Workspace'), findsNothing);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);

    // An admin agent's own settings call its place outside projects Admin.
    final harness = await showWith(tester, snapshot);
    snapshot.agents[1].extensions[0].options['admin'] = OptionValue(on: true);
    await harness.apply(tester, snapshot);
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    await tester.ensureVisible(find.text('Factory').last);
    await pumpFrames(tester);
    await tester.tap(find.text('Factory').last);
    await settle(tester);
    expect(find.text('Admin (no project)'), findsOneWidget);
  });

  testWidgets("the checklist's nudge is a switch under it", (tester) async {
    final snapshot = workspace();
    snapshot.extensions[1] = checklistExtension.deepCopy();
    snapshot.extensions[1].options.add(
      ExtensionOption(
        id: 'nudge',
        label: 'Remind it when it stops with work outstanding',
        kind: ExtensionOptionKind.EXTENSION_OPTION_KIND_SWITCH,
        defaultValue: true,
      ),
    );
    snapshot.agents[1].extensions[1].options['nudge'] = OptionValue(on: true);
    final harness = await showWith(tester, snapshot);
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    await unfoldExtensions(tester);
    final nudge = glyphButton('Remind it when it stops with work outstanding');
    await tester.ensureVisible(nudge);
    await pumpFrames(tester);
    await tester.tap(nudge);
    await settle(tester);
    await tester.tap(find.text('Save'));
    await settle(tester);
    final change = harness.last.updateAgent.extension_8;
    expect(change.id, 'checklist');
    expect(change.optionId, 'nudge');
    expect(change.value.on, isFalse);
  });

  testWidgets("items added in one call keep their order past 32, though "
      "they share a millisecond (Abbacchio's row 661)", (tester) async {
    final harness = await showWith(tester, workspace(openPanel: 'checklist'));
    final added = at(now);
    harness.backend.extensionView.apply(
      ExtensionView(
        extensionId: 'checklist',
        agentId: 'a1',
        checklist: Checklist(
          items: [
            for (var i = 1; i <= 40; i++)
              ChecklistItem(id: i, text: 'Step $i', created: added),
          ],
        ),
      ),
    );
    await settle(tester);
    final shown = [
      for (var i = 1; i <= 40; i++)
        if (find.text('Step $i').evaluate().isNotEmpty)
          (i, tester.getTopLeft(find.text('Step $i')).dy),
    ];
    expect(shown.length, greaterThan(5));
    for (var k = 1; k < shown.length; k++) {
      expect(
        shown[k].$2,
        greaterThan(shown[k - 1].$2),
        reason: '#${shown[k].$1} after #${shown[k - 1].$1}',
      );
    }
  });

  testWidgets('a long checklist follows its bottom like a chat', (
    tester,
  ) async {
    ExtensionView longView(int count) => ExtensionView(
      extensionId: 'checklist',
      agentId: 'a1',
      checklist: Checklist(
        items: [
          for (var i = 1; i <= count; i++)
            ChecklistItem(
              id: i,
              text: 'Item $i',
              status: i.isEven
                  ? ChecklistStatus.CHECKLIST_STATUS_DONE
                  : ChecklistStatus.CHECKLIST_STATUS_DROPPED,
              // A day apart, all the way back.
              created: at(now.subtract(Duration(days: count - i))),
            ),
        ],
      ),
    );
    final harness = await showWith(tester, workspace(openPanel: 'checklist'));
    harness.backend.extensionView.apply(longView(60));
    await settle(tester);
    await tester.pump();
    final jump = glyphButton('Jump to the latest');
    double opacity() => tester
        .widget<AnimatedOpacity>(
          find.ancestor(of: jump, matching: find.byType(AnimatedOpacity)),
        )
        .opacity;

    // Opens at the bottom: today's item shows, the first doesn't.
    expect(find.text('Item 60'), findsOneWidget);
    expect(find.text('Item 1'), findsNothing);
    expect(opacity(), 0);

    // New items are followed.
    harness.backend.extensionView.apply(longView(61));
    await settle(tester);
    await tester.pump();
    expect(find.text('Item 61'), findsOneWidget);

    // Scrolled back, it holds still as items come, with a way back.
    final list = find.descendant(
      of: find.byType(ExtensionPanel),
      matching: find.byType(Scrollable),
    );
    await tester.drag(list, const Offset(0, 600));
    await settle(tester);
    await tester.pump();
    expect(opacity(), 1);
    final held = tester.getTopLeft(find.text('Item 50'));
    harness.backend.extensionView.apply(longView(62));
    await settle(tester);
    await tester.pump();
    expect(tester.getTopLeft(find.text('Item 50')), held);

    // Every day back is there, done and dropped alike.
    await tester.fling(list, const Offset(0, 20000), 5000);
    await settle(tester);
    await tester.pump();
    await pumpFrames(tester);
    expect(find.text('Item 1'), findsOneWidget);
    expect(find.textContaining('Dropped'), findsWidgets);

    await tester.tap(jump);
    await pumpFrames(tester);
    expect(find.text('Item 62'), findsOneWidget);
    expect(opacity(), 0);
  });

  testWidgets('a finished item moves up into its day, easing in', (
    tester,
  ) async {
    final harness = await showWith(tester, workspace(openPanel: 'checklist'));
    harness.backend.extensionView.apply(checklistView());
    await settle(tester);
    double top(String text) => tester.getTopLeft(find.text(text)).dy;
    expect(top('Someday'), greaterThan(top('Outstanding')));

    final view = checklistView();
    view.checklist.items[2].status = ChecklistStatus.CHECKLIST_STATUS_DONE;
    harness.backend.extensionView.apply(view);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 60));
    double opacity() => tester
        .widget<Opacity>(
          find
              .ancestor(
                of: find.text('Someday'),
                matching: find.byType(Opacity),
              )
              .first,
        )
        .opacity;
    expect(opacity(), lessThan(1), reason: 'easing in');
    await settle(tester);
    expect(opacity(), 1);
    // Now under today's heading, above the outstanding ones.
    expect(
      top('Someday'),
      greaterThan(tester.getTopLeft(find.text('Today').first).dy),
      reason: "under today's heading",
    );
    expect(top('Someday'), lessThan(top('Outstanding')));
    expect(find.text('1 of 2 done'), findsOneWidget);
  });
}
