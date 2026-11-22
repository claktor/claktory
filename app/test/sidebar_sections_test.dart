// Projects and Chats are the sidebar's two sections, on the same level
// (#249): each heading always shows however many projects or chats there
// are, a tap on it folds the section or opens it (the backend keeps which,
// the same on every device), and folded it says how many it holds and
// whether any waits for the person. On a phone, one list whose section
// headings stay at the top while their section is in view.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';

import 'dart:ui' show Tristate;

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({
  bool projectsCollapsed = false,
  bool chatsCollapsed = false,
  int projects = 30,
  int chats = 30,
  bool waiting = false,
}) => Workspace(
  projectsCollapsed: projectsCollapsed,
  chatsCollapsed: chatsCollapsed,
  projects: [
    for (var i = 0; i < projects; i++)
      Project(id: 'p$i', name: 'Project $i', path: '/w/$i'),
  ],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', modelAvailable: true),
    for (var i = 0; i < projects; i++)
      Agent(
        id: 'a$i',
        name: 'Agent $i',
        projectId: 'p$i',
        modelAvailable: true,
        waitingSeqs: [if (waiting && i == 0) Int64(3)],
      ),
  ],
  chats: [
    for (var i = 0; i < chats; i++)
      Chat(id: 'c$i', title: 'Chat $i', updated: Int64(1000 - i)),
  ],
);

Future<Harness> _show(
  WidgetTester tester,
  Workspace snapshot, {
  Size size = const Size(1200, 800),
}) async {
  final harness = Harness(snapshot);
  await harness.show(tester, size: size);
  await settle(tester);
  return harness;
}

/// Whether [finder]'s widget is on screen, within [size].
bool _onScreen(WidgetTester tester, Finder finder, Size size) {
  final rect = tester.getRect(finder);
  return rect.top >= 0 && rect.bottom <= size.height;
}

void main() {
  testWidgets('with many projects and chats, both headings show', (
    tester,
  ) async {
    await _show(tester, _workspace());
    const size = Size(1200, 800);
    expect(_onScreen(tester, find.text('Projects'), size), isTrue);
    expect(_onScreen(tester, find.text('Chats'), size), isTrue);
    // Chats sits under Projects, its rows under it, above the footer.
    expect(
      tester.getTopLeft(find.text('Chats')).dy,
      greaterThan(tester.getTopLeft(find.text('Projects')).dy),
    );
    expect(find.text('Chat 0'), findsOneWidget);
    expect(_onScreen(tester, find.text('Chat 0'), size), isTrue);
  });

  testWidgets('a few projects: Chats follows them, with no gap', (
    tester,
  ) async {
    await _show(tester, _workspace(projects: 2, chats: 3));
    final lastProject = tester.getBottomLeft(find.text('Agent 1')).dy;
    final chats = tester.getTopLeft(find.text('Chats')).dy;
    expect(chats - lastProject, lessThan(60));
  });

  testWidgets('a tap on a heading folds it, through the backend', (
    tester,
  ) async {
    final harness = await _show(tester, _workspace());
    await tester.tap(find.text('Projects'));
    await settle(tester);
    expect(harness.last.updateSettings.projectsCollapsed, isTrue);
    expect(harness.last.updateSettings.hasChatsCollapsed(), isFalse);

    await tester.tap(find.text('Chats'));
    await settle(tester);
    expect(harness.last.updateSettings.chatsCollapsed, isTrue);
    expect(harness.last.updateSettings.hasProjectsCollapsed(), isFalse);
  });

  testWidgets('Projects folded: its count, and Chats right under it', (
    tester,
  ) async {
    await _show(tester, _workspace(projectsCollapsed: true));
    expect(find.text('Project 0'), findsNothing);
    expect(find.text('30'), findsOneWidget);
    expect(
      tester.getTopLeft(find.text('Chats')).dy -
          tester.getTopLeft(find.text('Projects')).dy,
      lessThan(48),
    );
    expect(find.text('Chat 0'), findsOneWidget);
    // Rivet, the workspace's, stays above both.
    expect(find.text('Rivet'), findsOneWidget);
  });

  testWidgets('Chats folded: its rows go, its heading stays', (tester) async {
    await _show(tester, _workspace(chatsCollapsed: true));
    expect(find.text('Chat 0'), findsNothing);
    expect(find.text('Chats'), findsOneWidget);
    expect(find.text('30'), findsOneWidget);
  });

  testWidgets('folded, it marks what waits inside', (tester) async {
    final semantics = tester.ensureSemantics();
    await _show(tester, _workspace(projectsCollapsed: true, waiting: true));
    expect(
      find.bySemanticsLabel('Projects has something waiting for your answer'),
      findsOneWidget,
    );
    semantics.dispose();
  });

  testWidgets('VoiceOver hears a heading that opens and folds', (tester) async {
    final semantics = tester.ensureSemantics();
    await _show(tester, _workspace(chatsCollapsed: true));
    final projects = tester
        .getSemantics(find.bySemanticsLabel('Projects').first)
        .getSemanticsData();
    expect(projects.flagsCollection.isHeader, isTrue);
    expect(projects.flagsCollection.isExpanded, Tristate.isTrue);
    final chats = tester
        .getSemantics(find.bySemanticsLabel('Chats').first)
        .getSemanticsData();
    expect(chats.flagsCollection.isExpanded, Tristate.isFalse);
    expect(chats.value, '30');
    semantics.dispose();
  });

  testWidgets("the phone's drawer: a section's heading stays at the top", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    const size = Size(390, 844);
    await _show(tester, _workspace()..selectedAgentId = 'a0', size: size);
    await tester.tap(find.bySemanticsLabel('Menu'));
    await pumpFrames(tester);
    final scroll = find.byType(CustomScrollView).last;
    final top = tester.getTopLeft(scroll).dy;
    // Into the projects: Projects' heading holds the top.
    await tester.drag(scroll, const Offset(0, -600));
    await pumpFrames(tester);
    expect(
      tester.getTopLeft(find.text('Projects').first).dy,
      lessThan(top + 30),
    );
    // Into the chats: Chats' heading holds it now.
    for (
      var i = 0;
      i < 12 && find.text('Chat 15').hitTestable().evaluate().isEmpty;
      i++
    ) {
      await tester.drag(scroll, const Offset(0, -300));
      await pumpFrames(tester);
    }
    await tester.drag(scroll, const Offset(0, -300));
    await pumpFrames(tester);
    final chats = tester.getTopLeft(find.text('Chats').first).dy;
    expect(chats, lessThan(top + 30));
    // Over the rows that went by under it.
    expect(tester.getTopLeft(find.text('Chat 3')).dy, lessThan(chats));
  });
}
