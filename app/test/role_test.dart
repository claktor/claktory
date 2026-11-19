import 'package:factory_app/agent_name.dart';
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/forms.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';
import 'messaging_test.dart' as messaging;

Agent mike({String role = 'Coordinator'}) => Agent(
  id: 'mike',
  projectId: 'p1',
  name: 'Mike',
  role: role,
  modelAvailable: true,
  color: 'green',
);

/// The height of [name]'s sidebar row.
double rowHeight(WidgetTester tester, String name) => tester
    .getSize(
      find
          .ancestor(
            of: find.descendant(
              of: find.byType(Sidebar),
              matching: find.text(name),
            ),
            matching: find.byType(AnimatedContainer),
          )
          .first,
    )
    .height;

void main() {
  testWidgets('the sidebar and the header show the role under the name', (
    tester,
  ) async {
    final harness = Harness(
      workspace(agents: [builder(working: false), mike()], selected: 'mike'),
    );
    await harness.show(tester);

    final inSidebar = find.descendant(
      of: find.byType(Sidebar),
      matching: find.text('Coordinator'),
    );
    expect(inSidebar, findsOneWidget);
    // Under the name, smaller.
    final name = find.descendant(
      of: find.byType(Sidebar),
      matching: find.text('Mike'),
    );
    expect(
      tester.getTopLeft(inSidebar).dy,
      greaterThan(tester.getBottomLeft(name).dy - 1),
    );
    expect(tester.widget<Text>(inSidebar).style!.fontSize, 11);
    // A row with a role is taller; one without stays as it was.
    expect(rowHeight(tester, 'Mike'), 40);
    expect(rowHeight(tester, 'Builder'), 32);

    // The header too.
    expect(find.text('Coordinator'), findsNWidgets(2));

    // No role: the name alone, and the row as before.
    await harness.apply(
      tester,
      workspace(
        agents: [
          builder(working: false),
          mike(role: ''),
        ],
        selected: 'mike',
      ),
    );
    expect(find.text('Coordinator'), findsNothing);
    await settle(tester);
    expect(rowHeight(tester, 'Mike'), 32);
  });

  testWidgets("a message's sender shows its role after its name", (
    tester,
  ) async {
    final harness = Harness(
      workspace(agents: [builder(working: false), mike()]),
    );
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            event: Event(
              source: EventSource.EVENT_SOURCE_AGENT,
              from: 'Mike',
              fromId: 'mike',
              text: 'Take the next task.',
            ),
          ),
        ],
      ),
    );
    await harness.show(tester);
    final sender = find.byType(NameThenRole);
    expect(sender, findsOneWidget);
    expect(tester.widget<NameThenRole>(sender).role, 'Coordinator');
    expect(find.text('Mike  Coordinator'), findsOneWidget);
  });

  testWidgets('the new agent form sends the role', (tester) async {
    final harness = Harness(workspace(agents: [builder(working: false)]));
    await harness.show(tester);
    await tapProjectPlus(tester, 'Factory');
    expect(find.text('Role'), findsOneWidget);
    final fields = find.descendant(
      of: find.byType(FormFrame),
      matching: find.byType(EditableText),
    );
    await tester.enterText(fields.at(0), 'Mike');
    await tester.enterText(fields.at(1), ' Coordinator ');
    await tester.pump();
    await tester.tap(find.text('Create'));
    await settle(tester);
    expect(harness.last.createAgent.name, 'Mike');
    expect(harness.last.createAgent.role, 'Coordinator');
  });

  testWidgets('agent settings change and clear the role', (tester) async {
    final harness = Harness(
      workspace(agents: [builder(working: false), mike()], selected: 'mike'),
    );
    await harness.show(tester);

    Future<void> openSettings() async {
      await tester.tap(
        find.descendant(
          of: find.byType(AgentView),
          matching: find.text('Mike'),
        ),
      );
      await settle(tester);
    }

    await openSettings();
    // Name, then Role.
    final role = find
        .descendant(
          of: find.byType(FormFrame),
          matching: find.byType(EditableText),
        )
        .at(1);
    expect(tester.widget<EditableText>(role).controller.text, 'Coordinator');
    await tester.enterText(role, 'Lead');
    await tester.pump();
    await tester.tap(find.text('Save'));
    await settle(tester);
    var update = harness.last.updateAgent;
    expect(update.role, 'Lead');
    expect(update.hasName(), isFalse);

    await openSettings();
    await tester.enterText(role, '');
    await tester.pump();
    await tester.tap(find.text('Save'));
    await settle(tester);
    update = harness.last.updateAgent;
    expect(update.hasRole(), isTrue);
    expect(update.role, '');
  });
  testWidgets('the Messaging panel shows roles under names', (tester) async {
    final snapshot = messaging.workspace();
    snapshot.agents[2].role = 'Tests the app';
    final harness = Harness(snapshot);
    harness.backend.extensionView.apply(
      messaging.view(['a2', 'a3'], ['rivet']),
    );
    await harness.show(tester);
    await settle(tester);
    final panel = find.byWidgetPredicate(
      (w) => w.runtimeType.toString() == 'MessagingView',
    );
    expect(
      find.descendant(of: panel, matching: find.text('Tests the app')),
      findsOneWidget,
    );
    expect(
      tester
          .getSize(
            find
                .ancestor(
                  of: find.descendant(of: panel, matching: find.text('Tester')),
                  matching: find.byType(Container),
                )
                .first,
          )
          .height,
      40,
    );
  });
}
