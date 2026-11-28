// An app's view open while the window crosses between the phone's layout
// and the wide one (an iPad rotated in Split View, an Android tablet
// resized) stays open, with the window built as main.dart builds it: a new
// Shell each time it crosses.
import 'package:factory_app/app_dock.dart';
import 'package:factory_app/identity.dart' show isPhoneLayout;
import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/layout.dart' show chatKind;
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/panes.dart' show AppPane, ExpandedPane;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart' show PhoneClaktories;
import 'package:factory_app/shell.dart' show Shell;
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';

final _charts = AppInfo(
  id: 'charts',
  name: 'Charts',
  icon: 'chart-column',
  state: AppState.APP_STATE_RUNNING,
  views: [AppViewSpec(id: 'main', title: 'Charts')],
);

Workspace _workspace() => Workspace(
  agents: [
    Agent(
      id: 'rivet',
      name: 'Rivet',
      modelAvailable: true,
      layout: ViewNode(
        id: 'chat',
        leaf: ViewLeaf(kind: chatKind),
      ),
    ),
  ],
  selectedAgentId: 'rivet',
  settings: Settings(userName: 'Matt'),
  apps: [_charts],
  appsAvailable: true,
  pinnedApps: ['charts'],
);

/// The window as main.dart's _buildWindow lays it out.
class _Window extends StatelessWidget {
  const _Window();

  @override
  Widget build(BuildContext context) => isPhoneLayout(context)
      ? const PhoneClaktories(shell: Shell(key: ValueKey('shown')))
      : const Shell(key: ValueKey('shown'), rail: SizedBox(width: 56));
}

Future<Harness> _show(WidgetTester tester, Size size) async {
  final harness = Harness(_workspace());
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(focus: focus, child: const _Window()),
  );
  await pumpFrames(tester);
  return harness;
}

Future<void> _resize(WidgetTester tester, Size size) async {
  tester.view.physicalSize = size;
  tester.view.display.size = size;
  await pumpFrames(tester, const Duration(seconds: 1));
}

/// Opens Charts as the person does: from the drawer's strip on the phone's
/// layout, the rail's dock wide.
Future<void> _openCharts(WidgetTester tester) async {
  if (isPhoneLayout(tester.element(find.byType(AppHandOver)))) {
    await tester.tap(find.bySemanticsLabel(RegExp(r'^(Menu|Back)$')).first);
    await pumpFrames(tester, const Duration(seconds: 1));
    await tester.ensureVisible(find.text('Charts').last);
    await tester.tap(find.text('Charts').last);
  } else {
    openApp(tester.element(find.byType(AppHandOver)), _charts);
  }
  await pumpFrames(tester, const Duration(seconds: 1));
}

void main() {
  for (final (name, phone, wide, setUp) in [
    (
      'an iPad in Split View, rotated',
      const Size(504, 1024),
      const Size(809, 1024),
      asIPad,
    ),
    (
      'an Android tablet, resized',
      const Size(411, 914),
      const Size(1280, 800),
      () {
        PhonePlatformPolicy.debugAndroidPolicy = true;
        addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
      },
    ),
  ]) {
    testWidgets('$name: Charts opened narrow stays open wide and narrow '
        'again', (tester) async {
      setUp();
      await _show(tester, phone);
      await _openCharts(tester);
      expect(find.byType(AppPane), findsOneWidget);
      expect(find.byType(ExpandedPane), findsNothing);

      await _resize(tester, wide);
      expect(find.byType(ExpandedPane), findsOneWidget);
      expect(find.byType(AppPane), findsOneWidget);

      await _resize(tester, phone);
      expect(find.byType(ExpandedPane), findsNothing);
      expect(find.byType(AppPane), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets('$name: Charts opened wide stays open narrow', (tester) async {
      setUp();
      await _show(tester, wide);
      await _openCharts(tester);
      expect(find.byType(ExpandedPane), findsOneWidget);

      await _resize(tester, phone);
      expect(find.byType(AppPane), findsOneWidget);
      expect(find.byType(ExpandedPane), findsNothing);
      expect(tester.takeException(), isNull);
    });

    testWidgets('$name: Charts closed before crossing stays closed', (
      tester,
    ) async {
      setUp();
      await _show(tester, phone);
      await _openCharts(tester);
      await tester.tap(find.bySemanticsLabel('Back'));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.byType(AppPane), findsNothing);

      await _resize(tester, wide);
      expect(find.byType(AppPane), findsNothing);
      await _openCharts(tester);
      await tester.tap(find.bySemanticsLabel('Back to Rivet'));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.byType(AppPane), findsNothing);

      await _resize(tester, phone);
      expect(find.byType(AppPane), findsNothing);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('a hand-over is the one Claktory\'s, for one frame', (
    tester,
  ) async {
    final mine = RecordingBackend();
    final other = RecordingBackend();
    final leaf = ViewLeaf(appId: 'charts', view: 'main');
    handAppOver(mine, leaf);
    expect(takeHandedOverApp(other), isNull);
    expect(takeHandedOverApp(mine), leaf);
    expect(takeHandedOverApp(mine), isNull);

    // Not taken in its frame: gone after it.
    handAppOver(mine, leaf);
    expect(SchedulerBinding.instance.hasScheduledFrame, isTrue);
    await tester.pump();
    expect(takeHandedOverApp(mine), isNull);
  });
}
