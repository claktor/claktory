// A window frozen for a move (#217) is the move's screen and nothing else:
// MoveGate takes the window's overlay with it, so no sheet, dialog or
// popover shown before the pause outlives it. Each ends as if dismissed
// (what waits on it hears null, it's counted out of the sheets showing,
// its place in Back released), and the window that comes back has none.
import 'package:factory_app/agent_view.dart' show AgentView;
import 'package:factory_app/controls.dart' show showModal;
import 'package:factory_app/mobile/popover.dart' show popoverHere;
import 'package:factory_app/mobile/presentation.dart';
import 'package:factory_app/move.dart' show MovingScreen;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart' as pb;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'phone_test.dart' show phone, workspace;

const _actions = [
  SheetAction('Rename…', 'rename'),
  SheetAction('Delete…', 'delete', destructive: true),
];

Future<void> _freeze(Harness harness, WidgetTester tester) async {
  harness.backend.move.value = pb.Move(phase: pb.MovePhase.MOVE_PHASE_FROZEN);
  await pumpFrames(tester);
}

Future<void> _thaw(Harness harness, WidgetTester tester) async {
  harness.backend.move.value = null;
  await pumpFrames(tester);
}

void main() {
  testWidgets('on a phone, a sheet and an action sheet go with the window, '
      'and the window that comes back takes new ones', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace());
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    final context = tester.element(find.byType(AgentView));

    String? sheetSaid = 'not yet';
    String? actionSaid = 'not yet';
    presentSheet<String>(
      context,
      title: 'A form',
      builder: (context, close) => const Text('In the form'),
    ).then((value) => sheetSaid = value);
    await pumpFrames(tester);
    showActions<String>(
      context,
      actions: _actions,
    ).then((value) => actionSaid = value);
    await pumpFrames(tester);
    expect(find.byType(PhoneSheet), findsNWidgets(2));
    expect(sheetsPresented.value, 2);

    await _freeze(harness, tester);
    expect(find.byType(MovingScreen), findsOneWidget);
    expect(find.byType(PhoneSheet), findsNothing);
    expect(find.text('In the form'), findsNothing);
    expect(sheetsPresented.value, 0);
    expect(sheetSaid, isNull);
    expect(actionSaid, isNull);
    expect(tester.takeException(), isNull);

    await _thaw(harness, tester);
    expect(find.byType(MovingScreen), findsNothing);
    expect(find.byType(PhoneSheet), findsNothing);
    final again = presentSheet<void>(
      tester.element(find.byType(AgentView)),
      builder: (context, close) => const Text('Afterwards'),
    );
    await pumpFrames(tester);
    expect(find.text('Afterwards'), findsOneWidget);
    expect(sheetsPresented.value, 1);
    await tester.tapAt(const Offset(200, 20));
    await pumpFrames(tester);
    await again;
    expect(sheetsPresented.value, 0);
  });

  testWidgets("on a Mac, a dialog (the move's own form, say) goes with the "
      'window', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace());
    await harness.show(tester);
    await pumpFrames(tester);
    showModal(
      tester.element(find.byType(AgentView)),
      (context, close) => const Text('Move this Claktory'),
    );
    await pumpFrames(tester);
    expect(find.text('Move this Claktory'), findsOneWidget);

    await _freeze(harness, tester);
    expect(find.byType(MovingScreen), findsOneWidget);
    expect(find.text('Move this Claktory'), findsNothing);
    expect(tester.takeException(), isNull);

    await _thaw(harness, tester);
    expect(find.text('Move this Claktory'), findsNothing);
  });

  testWidgets("on an iPad's wide window, a popover goes with the window", (
    tester,
  ) async {
    asIPad();
    final harness = Harness(workspace());
    await harness.show(tester, size: const Size(1366, 1024));
    await pumpFrames(tester);
    final context = tester.element(find.byType(AgentView));
    expect(popoverHere(context), isTrue);
    String? said = 'not yet';
    showActions<String>(
      context,
      actions: _actions,
    ).then((value) => said = value);
    await pumpFrames(tester);
    expect(find.text('Delete…'), findsOneWidget);
    expect(sheetsPresented.value, 1);

    await _freeze(harness, tester);
    expect(find.byType(MovingScreen), findsOneWidget);
    expect(find.text('Delete…'), findsNothing);
    expect(sheetsPresented.value, 0);
    expect(said, isNull);
    expect(tester.takeException(), isNull);
  });
}
