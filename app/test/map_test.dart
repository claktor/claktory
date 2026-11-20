import 'package:factory_app/map.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';

import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'map_fixture.dart';

/// The Map draws Workspace.map, which the backend computes; the app only
/// lays it out.
void main() {
  Future<Harness> showMap(
    WidgetTester tester, {
    Workspace? workspace,
    Size size = const Size(1440, 900),
  }) async {
    final harness = Harness(workspace ?? claktoryWorkspace());
    await harness.show(tester, size: size);
    harness.page.value = AppPage.map;
    await settle(tester);
    return harness;
  }

  Finder chip(String name) =>
      find.descendant(of: find.byType(MapView), matching: find.text(name));

  double opacityOf(WidgetTester tester, String name) => tester
      .widget<AnimatedOpacity>(
        find.ancestor(of: chip(name), matching: find.byType(AnimatedOpacity)),
      )
      .opacity;

  testWidgets('every agent, project, folder and secret is on it', (
    tester,
  ) async {
    await showMap(tester);
    for (final name in [
      'Rivet',
      'Mike',
      'Sam',
      'Iris',
      'Disk Scout',
      'Claktory',
      'Disk',
      'App',
      'Research',
      'Secrets',
      'OpenAI',
      'Cloudflare',
    ]) {
      // Projects, folders and the vault are painted; agents and secrets
      // are chips.
      if (const {
        'Claktory',
        'Disk',
        'App',
        'Research',
        'Secrets',
      }.contains(name)) {
        continue;
      }
      expect(chip(name), findsOneWidget, reason: name);
    }
    expect(find.text('Done'), findsOneWidget);
  });

  testWidgets('clicking an agent opens its conversation', (tester) async {
    final harness = await showMap(tester);
    await tester.tap(chip('Sam'));
    await settle(tester);
    expect(harness.last.selectAgent.agentId, 'builder');
    expect(harness.page.value, AppPage.agent);
  });

  testWidgets('hovering an agent lights up those it reaches and dims the '
      'rest', (tester) async {
    await showMap(tester);
    expect(opacityOf(tester, 'Disk Scout'), 1);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    addTearDown(mouse.removePointer);
    await mouse.moveTo(tester.getCenter(chip('Iris')));
    await tester.pump();
    // Iris messages her project, is nested under Mike and has OpenAI;
    // Disk Scout is out of her reach.
    expect(opacityOf(tester, 'Iris'), 1);
    expect(opacityOf(tester, 'Mike'), 1);
    expect(opacityOf(tester, 'OpenAI'), 1);
    expect(opacityOf(tester, 'Cloudflare'), lessThan(1));
    expect(opacityOf(tester, 'Disk Scout'), lessThan(1));

    await mouse.moveTo(const Offset(1430, 890));
    await tester.pump();
    expect(opacityOf(tester, 'Disk Scout'), 1);
  });

  testWidgets('each kind of line has a switch', (tester) async {
    await showMap(tester);
    Finder toggle(String name) => find.byWidgetPredicate(
      (w) =>
          w is Semantics &&
          w.properties.label == name &&
          w.properties.toggled != null,
    );
    bool shown(String name) =>
        tester.widget<Semantics>(toggle(name)).properties.toggled!;
    for (final name in mapLayerNames.values) {
      expect(toggle(name), findsOneWidget, reason: name);
      expect(shown(name), isTrue);
    }
    await tester.tap(toggle('Secrets'));
    await tester.pump();
    expect(shown('Secrets'), isFalse);
    await tester.tap(toggle('Secrets'));
    await tester.pump();
    expect(shown('Secrets'), isTrue);
  });

  testWidgets('a backend too old to send the map says so', (tester) async {
    await showMap(tester, workspace: claktoryWorkspace()..clearMap());
    expect(find.text('Update Claktory to see its map.'), findsOneWidget);
  });

  testWidgets('on a phone it fills the screen, and Done goes back', (
    tester,
  ) async {
    final harness = await showMap(tester, size: const Size(393, 852));
    expect(find.byType(MapView), findsOneWidget);
    expect(chip('Mike'), findsOneWidget);
    await tester.tap(find.text('Done'));
    await settle(tester);
    expect(harness.page.value, AppPage.agent);
  });
}
