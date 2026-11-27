// An iPad with no Claktory yet connects the phone's way at every width:
// PairScreen (scan, sign in, paste), beside the rail when the
// window is wide, never the Mac's ConnectStep, which only pastes.
import 'package:factory_app/connect_step.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/ipad/pair_step.dart';
import 'package:factory_app/main.dart';
import 'package:factory_app/phone.dart' show PairScreen;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart' show DeviceDrafts;
import 'package:factory_app/setup.dart' show InstallResult;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../setup_test.dart' show FakeSetupChannel;
import 'as_ipad.dart';

Future<void> _show(WidgetTester tester, Size size) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  tester.view.display.size = size;
  tester.view.display.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  addTearDown(tester.view.display.reset);
  await tester.pumpWidget(
    FactoryApp(
      drafts: DeviceDrafts(MemoryDraftStorage()),
      connections: Connections(MemoryConnectionStorage()),
      open: (target) {
        // As the app's (main.dart): no Claktory of its own here.
        final backend = RecordingBackend(localBackend: false)
          ..connectTo(target);
        backend.workspace.apply(
          Workspace(settings: Settings(userName: 'Robin')),
        );
        return backend;
      },
      localBackend: false,
      // A Mac with no Claktory installed, as connect_step_test's.
      setupChannel: FakeSetupChannel(
        results: {
          'install --check': const InstallResult(result: 'not_installed'),
        },
      ),
    ),
  );
  await pumpFrames(tester);
}

void main() {
  for (final (name, size) in [
    ('full screen, landscape', const Size(1376, 1032)),
    ('full screen, portrait', const Size(1032, 1376)),
    ('half of the screen', const Size(688, 1032)),
    ('a third: the phone layout', const Size(507, 1032)),
  ]) {
    testWidgets('$name: the phone\'s way to connect, not the Mac\'s', (
      tester,
    ) async {
      asIPad();
      await _show(tester, size);
      expect(find.byType(PairScreen), findsOneWidget);
      expect(find.byType(ConnectStep), findsNothing);
      expect(
        find.byType(IPadPairStep),
        size.width >= 600 ? findsOneWidget : findsNothing,
      );
      // It fills the window but the rail, from the top.
      final screen = tester.getRect(find.byType(PairScreen));
      expect(screen.right, size.width);
      expect(screen.width, greaterThan(size.width - 80));
    });
  }

  testWidgets('a Mac keeps its own step, which pastes a code', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    await _show(tester, const Size(1200, 900));
    expect(find.byType(ConnectStep), findsOneWidget);
    expect(find.byType(PairScreen), findsNothing);
  });
}
