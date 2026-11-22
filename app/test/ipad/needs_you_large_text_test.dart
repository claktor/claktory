// Sorbet's iPad sweep (G5, S5): Needs you's title bar and its Waiting and
// History tabs fit a window 640 wide at the largest text (AX5), and an
// agent's extension bar scrolls on an iPad on its side when its icons
// are taller than the window, so the last can still be reached.
import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/extensions.dart' show ExtensionBar;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../extensions_test.dart' show workspace;
import '../harness.dart';
import 'as_ipad.dart';
import 'window_fixtures.dart' show TextFactor;

void main() {
  for (final width in const [600.0, 640.0, 680.0]) {
    testWidgets('at the largest text in a window $width wide, Needs you '
        'and its tabs fit', (tester) async {
      asIPad();
      tester.platformDispatcher.textScaleFactorTestValue = TextFactor.ax5;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = Harness(workspace());
      await harness.show(tester, size: Size(width, 1000));
      await settle(tester);
      harness.page.value = AppPage.inbox;
      await pumpFrames(tester);
      await settle(tester);
      expect(tester.takeException(), isNull);
      expect(find.text('Done').hitTestable(), findsOneWidget);
      expect(find.text('History').hitTestable(), findsOneWidget);

      await tester.tap(find.text('History'));
      await settle(tester);
      expect(tester.takeException(), isNull);
      expect(find.text('Waiting').hitTestable(), findsOneWidget);
    });
  }

  testWidgets('an iPad on its side: an agent with many extensions scrolls '
      'its bar to the last', (tester) async {
    asIPad();
    final snapshot = workspace();
    snapshot.extensions
      ..clear()
      ..addAll([
        for (var i = 1; i <= 19; i++)
          Extension(id: 'ext$i', name: 'Extension $i'),
      ]);
    final builder = snapshot.agents.firstWhere((agent) => agent.id == 'a1');
    builder.extensions
      ..clear()
      ..addAll([
        for (var i = 1; i <= 19; i++)
          AgentExtension(id: 'ext$i', enabled: true),
      ]);
    final harness = Harness(snapshot);
    await harness.show(
      tester,
      size: const Size(1194, 834),
      home: Align(
        alignment: Alignment.topLeft,
        child: SizedBox(
          height: 834,
          child: ExtensionBar(agent: harness.backend.workspace.agent('a1')),
        ),
      ),
    );
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    final last = find.byWidgetPredicate(
      (widget) => widget is GlyphButton && widget.label == 'Extension 19',
      skipOffstage: false,
    );
    expect(last.hitTestable(), findsNothing);
    await tester.dragUntilVisible(
      last,
      find.byType(SingleChildScrollView),
      const Offset(0, -200),
    );
    await pumpFrames(tester);
    expect(last.hitTestable(), findsOneWidget);
    await tester.tap(last);
    await settle(tester);
    expect(harness.last.openPanel.extensionId, 'ext19');
  });
}
