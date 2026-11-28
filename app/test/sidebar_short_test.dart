// A short window (an Android tablet on its side, a Mac window pulled
// short): what stays above the sections (Needs you and Rivet), the
// footer and Chats leave the project list room for a few rows at least,
// or the sections scroll as one list, every project a scroll away. The
// list never shows open with no room for its rows. And an Android
// tablet's wide layout keeps clear of the status bar.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// Doppio's tablet: Rivet alone at the top with three things waiting, a
/// project of a lead and three under it, Chats open with none yet.
Workspace _workspace() => Workspace(
  projects: [Project(id: 'launch', name: 'Claktory Launch', path: '/w/l')],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
    Agent(
      id: 'elena',
      name: 'Elena',
      role: 'Launch lead',
      projectId: 'launch',
      modelAvailable: true,
      waitingSeqs: [Int64(3), Int64(4), Int64(5)],
    ),
    for (final name in ['Jun', 'Sofia', 'Amara'])
      Agent(
        id: name.toLowerCase(),
        name: name,
        role: 'Writer',
        projectId: 'launch',
        parentId: 'elena',
        modelAvailable: true,
      ),
  ],
);

void _as(TargetPlatform platform) {
  debugDefaultTargetPlatformOverride = platform;
  PhonePlatformPolicy.debugAndroidPolicy = platform == TargetPlatform.android;
  addTearDown(() {
    debugDefaultTargetPlatformOverride = null;
    PhonePlatformPolicy.debugAndroidPolicy = false;
  });
}

/// The list the projects scroll in: the sections' one list, or the
/// projects' own.
Finder get _sidebarList {
  final one = find.byKey(const ValueKey('sidebar-one-list'));
  return find.descendant(
    of: one.evaluate().isNotEmpty ? one : find.byType(Sidebar),
    matching: find.ancestor(
      of: find.text('Claktory Launch', skipOffstage: false),
      matching: find.byType(Scrollable),
    ),
  );
}

/// Unscrolled, the sections show the project and its first agent, or
/// they are one list, which scrolls as a whole.
void _roomFor(WidgetTester tester) {
  if (find.byKey(const ValueKey('sidebar-one-list')).evaluate().isNotEmpty) {
    return;
  }
  final list = tester.getRect(_sidebarList.first);
  for (final name in ['Claktory Launch', 'Elena']) {
    final row = tester.getRect(
      find.descendant(
        of: find.byType(Sidebar),
        matching: find.text(name, skipOffstage: false),
      ),
    );
    expect(row.top, greaterThanOrEqualTo(list.top), reason: name);
    expect(row.bottom, lessThanOrEqualTo(list.bottom), reason: name);
  }
}

/// The project's agents can each be scrolled to and tapped.
Future<void> _everyAgentReachable(WidgetTester tester) async {
  for (final name in ['Elena', 'Jun', 'Sofia', 'Amara']) {
    final row = find.descendant(
      of: find.byType(Sidebar),
      matching: find.text(name),
    );
    await tester.scrollUntilVisible(row, 40, scrollable: _sidebarList.first);
    await tester.pump();
    expect(row.hitTestable(), findsOneWidget, reason: name);
  }
}

Future<void> _show(WidgetTester tester, Size size) async {
  // Fresh, unscrolled.
  await tester.pumpWidget(const SizedBox());
  await Harness(_workspace()..selectedAgentId = 'rivet')
      .show(tester, size: size);
  await pumpFrames(tester);
  expect(tester.takeException(), isNull);
}

void main() {
  testWidgets('an Android tablet on its side (960×600): the project list '
      'has room, its agents in reach', (tester) async {
    _as(TargetPlatform.android);
    tester.view.padding = const FakeViewPadding(top: 24, bottom: 24);
    await _show(tester, const Size(960, 600));
    _roomFor(tester);
    await _everyAgentReachable(tester);
    expect(tester.takeException(), isNull);
  });

  testWidgets('a Mac window pulled short: the project list has room, its '
      'agents in reach', (tester) async {
    _as(TargetPlatform.macOS);
    for (var height = 300.0; height <= 600; height += 20) {
      await _show(tester, Size(1000, height));
      _roomFor(tester);
      await _everyAgentReachable(tester);
    }
    expect(tester.takeException(), isNull);
  });

  testWidgets('a tall window keeps its sections', (tester) async {
    for (final platform in [TargetPlatform.android, TargetPlatform.macOS]) {
      _as(platform);
      await _show(tester, const Size(1000, 900));
      expect(find.byKey(const ValueKey('sidebar-one-list')), findsNothing);
      await _everyAgentReachable(tester);
    }
  });

  testWidgets("an Android tablet's wide layout starts under the status bar", (
    tester,
  ) async {
    _as(TargetPlatform.android);
    tester.view.padding = const FakeViewPadding(top: 24);
    await _show(tester, const Size(1280, 800));
    // The sidebar and the conversation's header start under it.
    expect(tester.getTopLeft(find.byType(Sidebar)).dy, 24);
    expect(tester.getTopLeft(find.byType(AgentView)).dy, 24);
    expect(tester.takeException(), isNull);
  });
}
