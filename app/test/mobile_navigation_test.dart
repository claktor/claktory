// A phone's navigation (#198, phase 1A): the conversation in front, the
// drawer from the Menu or the leading edge, pages pushed over the
// conversation with Back, and what the drawer finds.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/controls.dart' show GlyphButton, TextField;
import 'package:factory_app/mobile/drawer.dart';
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/pipes.dart' show PipeView;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/sidebar.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:factory_app/inbox.dart' show InboxView;
import 'package:factory_app/mobile/needs_you_phone.dart' show PhoneNeedsYou;

import 'harness.dart';
import 'phone_test.dart' show menuButton, phone, workspace;

Future<Harness> _show(WidgetTester tester, Workspace snapshot) async {
  final harness = Harness(snapshot);
  await harness.show(tester, size: phone);
  await pumpFrames(tester);
  return harness;
}

Finder _inDrawer(String text) =>
    find.descendant(of: find.byType(Sidebar), matching: find.text(text));

PhoneRoutes _routes(WidgetTester tester) =>
    PhoneRoutes.of(tester.element(find.byType(AgentView)))!;

/// A swipe from the screen's leading edge.
Future<void> _edgeSwipe(WidgetTester tester, {double distance = 300}) async {
  await tester.dragFrom(const Offset(6, 420), Offset(distance, 0));
  await pumpFrames(tester);
}

Pipe _pipe({List<int> waiting = const []}) => Pipe(
  id: 'pipe1',
  name: 'Acme launch',
  projectId: 'p1',
  state: PipeState.PIPE_STATE_OPEN,
  createdMs: Int64(1000),
  peer: PipePeer(claktoryName: "Ana's Claktory", ownerName: 'Ana'),
  online: true,
  waitingSeqs: [for (final seq in waiting) Int64(seq)],
);

void main() {
  testWidgets('a swipe from the leading edge opens the drawer; a swipe '
      'back, the dimmed conversation or Close closes it', (tester) async {
    await _show(tester, workspace());
    await _edgeSwipe(tester);
    expect(find.byType(Sidebar), findsOneWidget);
    expect(_routes(tester).drawerOpen, isTrue);
    // As wide as the screen leaves room for, with the conversation in
    // view beside it.
    expect(
      tester.getSize(find.byType(Sidebar)).width,
      lessThanOrEqualTo(drawerWidth(phone.width)),
    );

    await tester.dragFrom(const Offset(300, 420), const Offset(-250, 0));
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsNothing);

    // A short swipe springs back.
    await _edgeSwipe(tester, distance: 60);
    expect(find.byType(Sidebar), findsNothing);

    await _edgeSwipe(tester);
    await tester.tapAt(Offset(phone.width - 8, 420));
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsNothing);

    await tester.tap(menuButton());
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel('Close'));
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsNothing);
  });

  testWidgets('what holds the drawer swipe keeps it closed until it lets go', (
    tester,
  ) async {
    await _show(tester, workspace());
    final hold = _routes(tester).holdGestures();
    await _edgeSwipe(tester);
    expect(find.byType(Sidebar), findsNothing);
    hold.release();
    await _edgeSwipe(tester);
    expect(find.byType(Sidebar), findsOneWidget);
  });

  testWidgets('Settings from the drawer is a page over the conversation; '
      'Back takes it away', (tester) async {
    final harness = await _show(tester, workspace());
    await tester.tap(menuButton());
    await pumpFrames(tester);
    await tester.scrollUntilVisible(
      _inDrawer('Settings'),
      200,
      scrollable: find
          .descendant(
            of: find.byType(Sidebar),
            matching: find.byType(Scrollable),
          )
          .first,
    );
    await tester.tap(_inDrawer('Settings'));
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.settings);
    expect(find.byType(SettingsView), findsOneWidget);
    expect(find.byType(Sidebar), findsNothing);
    expect(_routesOf(tester, SettingsView).canPop, isTrue);
    // No drawer from the edge here: the swipe goes back.
    await tester.dragFrom(const Offset(4, 420), const Offset(300, 0));
    await pumpFrames(tester);
    expect(find.byType(SettingsView), findsNothing);
    expect(harness.page.value, AppPage.agent);
    expect(find.byType(Sidebar), findsNothing);
    expect(find.byType(AgentView), findsOneWidget);

    // An agent opening a page pushes it the same way.
    harness.page.value = AppPage.changelog;
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsNothing);
    harness.page.value = AppPage.agent;
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
  });

  testWidgets("a project's More offers its Files as a page, with Back", (
    tester,
  ) async {
    final harness = await _show(tester, workspace());
    await tester.tap(menuButton());
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel('Factory options'));
    await pumpFrames(tester);
    expect(find.text('Project settings…'), findsOneWidget);
    expect(find.text('Add an agent…'), findsOneWidget);
    expect(find.text('Connect a pipe…'), findsOneWidget);
    await tester.tap(find.text('Files'));
    await pumpFrames(tester);
    expect(harness.last.watchFiles.projectId, 'p1');
    expect(find.byType(Sidebar), findsNothing);
    expect(find.byType(AgentView), findsNothing);

    await tester.tap(find.bySemanticsLabel('Back'));
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
    expect(harness.last.watchFiles.projectId, '');
    // The sheet's gone: the edge opens the drawer again.
    await _edgeSwipe(tester);
    expect(find.byType(Sidebar), findsOneWidget);
  });

  testWidgets("switching agents keeps each one's draft", (tester) async {
    final harness = await _show(tester, workspace()..selectedAgentId = 'mike');
    final box = find
        .descendant(
          of: find.byType(AgentView),
          matching: find.byType(EditableText),
        )
        .last;
    await tester.enterText(box, 'Half a thought');
    await tester.pump();

    await tester.tap(menuButton());
    await pumpFrames(tester);
    await tester.tap(_inDrawer('Rivet'));
    await pumpFrames(tester);
    await harness.apply(tester, workspace());
    await pumpFrames(tester);
    expect(find.text('Half a thought'), findsNothing);

    await tester.tap(menuButton());
    await pumpFrames(tester);
    await tester.tap(_inDrawer('Mike'));
    await pumpFrames(tester);
    await harness.apply(tester, workspace()..selectedAgentId = 'mike');
    await pumpFrames(tester);
    expect(harness.drafts.of('mike'), 'Half a thought');
    expect(find.text('Half a thought'), findsOneWidget);
  });

  testWidgets('the search narrows the drawer to what matches', (tester) async {
    await _show(tester, workspace());
    await tester.tap(menuButton());
    await pumpFrames(tester);
    final search = find.descendant(
      of: find.byWidgetPredicate(
        (widget) => widget is TextField && widget.placeholder == 'Search',
      ),
      matching: find.byType(EditableText),
    );
    await tester.enterText(search.first, 'coord');
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsNothing);
    expect(find.byType(SidebarSearch), findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(SidebarSearch),
        matching: find.text('Mike'),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: find.byType(SidebarSearch),
        matching: find.text('Rivet'),
      ),
      findsNothing,
    );

    await tester.enterText(search.first, 'zzz');
    await pumpFrames(tester);
    expect(find.textContaining('Nothing matches'), findsOneWidget);

    final clear = find.byWidgetPredicate(
      (widget) => widget is GlyphButton && widget.label == 'Clear search',
    );
    await tester.tap(clear);
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsOneWidget);
  });

  testWidgets('Needs you leads the drawer as on a Mac: one row with the '
      'count, which pushes the Needs you page', (tester) async {
    // An iPhone's: the list of what waits first (#599).
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final snapshot = workspace();
    snapshot.agents[1].waitingSeqs.add(Int64(7));
    final harness = await _show(tester, snapshot);
    await tester.tap(menuButton());
    await pumpFrames(tester);
    expect(_inDrawer('Needs you'), findsOneWidget);
    // Mike once, in his project: the row counts what waits, it doesn't
    // list who.
    expect(_inDrawer('Mike'), findsOneWidget);
    final row = find.ancestor(
      of: _inDrawer('Needs you'),
      matching: find.byKey(const ValueKey('needs-you')),
    );
    expect(tester.getSize(row).height, greaterThanOrEqualTo(44));
    expect(
      tester.getTopLeft(_inDrawer('Needs you')).dy,
      lessThan(tester.getTopLeft(_inDrawer('Mike')).dy),
    );
    expect(
      find.bySemanticsLabel(RegExp('Something is waiting for your answer')),
      findsOneWidget,
    );
    await tester.tap(_inDrawer('Needs you'));
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.inbox);
    expect(find.byType(InboxView), findsOneWidget);
    expect(find.byType(PhoneNeedsYou), findsNothing);
    expect(find.byType(Sidebar), findsNothing);
    await tester.tap(find.bySemanticsLabel('Back'));
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.agent);
    expect(find.byType(AgentView), findsOneWidget);
  });

  testWidgets('nothing waiting: no Needs you in the drawer', (tester) async {
    await _show(tester, workspace());
    await tester.tap(menuButton());
    await pumpFrames(tester);
    expect(_inDrawer('Needs you'), findsNothing);
  });

  testWidgets("a pipe opens in the conversation's place, with the Menu", (
    tester,
  ) async {
    final snapshot = workspace()..pipes.add(_pipe());
    final harness = await _show(tester, snapshot);
    await tester.tap(menuButton());
    await pumpFrames(tester);
    await tester.tap(_inDrawer('Acme launch'));
    await pumpFrames(tester);
    expect(harness.last.selectPipe.pipeId, 'pipe1');
    expect(find.byType(Sidebar), findsNothing);

    await harness.apply(tester, snapshot..selectedPipeId = 'pipe1');
    await pumpFrames(tester);
    expect(find.byType(PipeView), findsOneWidget);
    expect(find.byType(AgentView), findsNothing);
    await tester.tap(find.bySemanticsLabel('Menu'));
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsOneWidget);
  });

  testWidgets('with no agent chosen, the drawer opens by itself', (
    tester,
  ) async {
    await _show(tester, workspace()..selectedAgentId = '');
    expect(find.byType(Sidebar), findsOneWidget);
  });

  for (final size in const [Size(320, 568), Size(375, 667), Size(440, 956)]) {
    testWidgets('the drawer fits a ${size.width.toInt()}pt phone at the '
        'largest text size', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final snapshot = workspace()..pipes.add(_pipe(waiting: [3]));
      snapshot.agents[1]
        ..role = 'Principal infrastructure and release engineer'
        ..waitingSeqs.add(Int64(7));
      final harness = Harness(snapshot);
      await harness.show(tester, size: size);
      await pumpFrames(tester);
      _routes(tester).openDrawer();
      await pumpFrames(tester);
      expect(find.byType(Sidebar), findsOneWidget);
      final list = find
          .descendant(
            of: find.byType(Sidebar),
            matching: find.byType(Scrollable),
          )
          .first;
      for (var i = 0; i < 12; i++) {
        await tester.drag(list, const Offset(0, -300), warnIfMissed: false);
        await tester.pump(const Duration(milliseconds: 50));
      }
      await pumpFrames(tester);
      // An overflow would have failed the test by now.
      expect(_inDrawer("What's new"), findsOneWidget);
    });
  }

  testWidgets('a Mac has no drawer', (tester) async {
    final harness = Harness(workspace());
    await harness.show(tester);
    await pumpFrames(tester);
    expect(find.byType(PhoneDrawerHost), findsNothing);
    expect(find.byType(Sidebar), findsOneWidget);
  });
}

PhoneRoutes _routesOf(WidgetTester tester, Type type) =>
    PhoneRoutes.of(tester.element(find.byType(type)))!;
