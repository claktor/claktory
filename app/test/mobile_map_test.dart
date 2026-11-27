// The Map on a phone: Back, Search and Lines in its
// header; the board at its own type size with Fit at its foot; a tapped
// agent's card with Open conversation; everything on it as a list to
// search; a pipe's pad a finger can tap.
import 'package:factory_app/controls.dart' show Toggle;
import 'package:factory_app/map.dart';
import 'package:factory_app/map_card.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'map_fixture.dart';

const _phone = Size(393, 852);

Future<Harness> _showMap(
  WidgetTester tester, {
  Workspace? workspace,
  Size size = _phone,
}) async {
  final harness = Harness(workspace ?? claktoryWorkspace());
  await harness.show(tester, size: size);
  harness.page.value = AppPage.map;
  await pumpFrames(tester);
  return harness;
}

Finder _onMap(String name) =>
    find.descendant(of: find.byType(MapView), matching: find.text(name));

void main() {
  testWidgets('its header has Back, Search and Lines; Fit sits at its foot '
      'and the layer switches step aside', (tester) async {
    await _showMap(tester);
    expect(find.bySemanticsLabel('Back'), findsOneWidget);
    expect(find.bySemanticsLabel('Search the map'), findsOneWidget);
    expect(find.bySemanticsLabel('Lines'), findsOneWidget);
    expect(find.text('Fit'), findsOneWidget);
    expect(
      tester
          .getSize(
            find
                .ancestor(
                  of: find.text('Fit'),
                  matching: find.byType(Container),
                )
                .first,
          )
          .height,
      greaterThanOrEqualTo(44),
    );
    // Not along the top: in the Lines sheet.
    expect(find.text('Under other agents'), findsNothing);
  });

  testWidgets('Lines lists a switch for each kind of line; turning one off '
      'hides its lines and marks the button', (tester) async {
    await _showMap(tester);
    await tester.tap(find.bySemanticsLabel('Lines'));
    await pumpFrames(tester);
    expect(find.text('Messages'), findsOneWidget);
    expect(find.text('Badges: extra reach or power'), findsOneWidget);
    Toggle messages() => tester.widget<Toggle>(
      find.byWidgetPredicate((w) => w is Toggle && w.label == 'Messages'),
    );
    expect(messages().value, isTrue);
    await tester.tap(find.text('Messages'));
    await pumpFrames(tester);
    expect(messages().value, isFalse);
    await tester.tap(find.bySemanticsLabel('Close').last);
    await pumpFrames(tester);
    expect(find.text('Badges: extra reach or power'), findsNothing);
  });

  testWidgets('tapping an agent shows its card in place of Fit; Open '
      'conversation opens it and closes the Map', (tester) async {
    final harness = await _showMap(tester);
    await tester.tap(_onMap('Mike'));
    await pumpFrames(tester);
    expect(find.byType(MapAgentCard), findsOneWidget);
    expect(find.text('Fit'), findsNothing);
    await tester.tap(find.text('Open conversation'));
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.agent);
    expect(find.byType(MapView), findsNothing);
  });

  testWidgets('Search lists everything on the map by project; typing '
      'narrows it, and a tap shows that agent on the map with its card', (
    tester,
  ) async {
    await _showMap(tester);
    await tester.tap(find.bySemanticsLabel('Search the map'));
    await pumpFrames(tester);
    expect(find.text('On the map'), findsOneWidget);
    expect(find.text('Rivet'), findsWidgets);
    // An agent's ties, in a line.
    expect(find.textContaining('Messages '), findsWidgets);

    await tester.enterText(find.byType(EditableText).last, 'kenji');
    await pumpFrames(tester);
    expect(find.text('Rivet'), findsNothing);
    expect(find.text('Kenji'), findsOneWidget);

    await tester.tap(find.text('Kenji'));
    await pumpFrames(tester);
    expect(find.text('On the map'), findsNothing);
    expect(find.byType(MapAgentCard), findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(MapAgentCard),
        matching: find.text('Kenji'),
      ),
      findsOneWidget,
    );
  });

  testWidgets("a search that matches nothing says so", (tester) async {
    await _showMap(tester);
    await tester.tap(find.bySemanticsLabel('Search the map'));
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText).last, 'zzzz');
    await pumpFrames(tester);
    expect(find.text('Nothing matches'), findsOneWidget);
  });

  testWidgets("a pipe's pad is a finger's height, and a tap opens the pipe "
      'in place of the Map', (tester) async {
    final workspace = claktoryWorkspace();
    workspace.pipes.add(
      Pipe(
        id: 'pipe1',
        name: 'Acme launch',
        projectId: 'factory',
        state: PipeState.PIPE_STATE_OPEN,
        createdMs: Int64(1000),
        peer: PipePeer(claktoryName: "Ana's Claktory", ownerName: 'Ana'),
        online: true,
      ),
    );
    final harness = await _showMap(tester, workspace: workspace);
    final pad = find.bySemanticsLabel(RegExp('^Pipe Acme launch'));
    expect(pad, findsOneWidget);
    await tester.tap(pad);
    await pumpFrames(tester);
    expect(
      harness.backend.sent.any((message) => message.hasSelectPipe()),
      isTrue,
    );
    expect(harness.page.value, AppPage.agent);
  });

  for (final size in const [Size(320, 568), Size(440, 956)]) {
    testWidgets('at the largest text the Map and its list fit '
        '${size.width.toInt()} points', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await _showMap(tester, size: size);
      expect(tester.takeException(), isNull);
      await tester.tap(_onMap('Mike'));
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      await tester.tap(find.text('Close').last);
      await pumpFrames(tester);
      await tester.tap(find.bySemanticsLabel('Search the map'));
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
    });
  }
}
