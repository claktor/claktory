// The phone drawer's search: what it finds says what its tree row would.
import 'package:factory_app/pipes.dart' show pipeOverLimitLine;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart' show SidebarSearch;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

void main() {
  testWidgets('a pipe found whose lead is past its daily limit says so, as '
      'its row in the tree does', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(
      Workspace(
        projects: [Project(id: 'p1', name: 'Claktory', path: '/w')],
        agents: [
          Agent(
            id: 'mike',
            name: 'Mike',
            projectId: 'p1',
            modelAvailable: true,
          ),
          Agent(
            id: 'kai',
            name: 'Kai',
            projectId: 'p1',
            pipeId: 'pipe1',
            modelAvailable: true,
            spend: ContainedSpend(
              spentUsd: 10.2,
              capUsd: 10,
              capTokens: Int64(500000),
              over: true,
            ),
          ),
        ],
        pipes: [
          Pipe(
            id: 'pipe1',
            name: 'Acme launch',
            projectId: 'p1',
            leadId: 'kai',
            state: PipeState.PIPE_STATE_OPEN,
            online: true,
            peer: PipePeer(claktoryName: "Ana's Claktory"),
          ),
        ],
        selectedAgentId: 'mike',
      ),
    );
    await harness.show(tester, size: const Size(390, 844));
    await settle(tester);
    await tester.tap(find.bySemanticsLabel('Menu'));
    await pumpFrames(tester);
    await tester.enterText(
      find.byWidgetPredicate(
        (widget) =>
            widget is EditableText && widget.focusNode.debugLabel == 'Search',
      ),
      'acme',
    );
    await pumpFrames(tester);
    expect(find.byType(SidebarSearch), findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(SidebarSearch),
        matching: find.textContaining(pipeOverLimitLine),
      ),
      findsOneWidget,
    );
    expect(find.text('Online'), findsNothing);
  });
}
