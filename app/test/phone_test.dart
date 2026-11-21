import 'package:factory_app/agent_view.dart';
import 'package:factory_app/extensions.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart' show NoAgentView;
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const phone = Size(390, 844);

final _code =
    'factory://?t=${'a' * 64}&name=mac-mini&e=${'c' * 64}'
    '&r=https://relay.example.com';

Workspace workspace({String openPanel = ''}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
    Agent(
      id: 'mike',
      name: 'Mike',
      role: 'Coordinator',
      projectId: 'p1',
      modelAvailable: true,
      model: 'anthropic/claude-opus-5-5',
      openPanel: openPanel,
      extensions: [AgentExtension(id: 'checklist', enabled: true)],
    ),
  ],
  extensions: [
    Extension(id: 'checklist', name: 'Checklist', icon: 'list-checks'),
  ],
  selectedAgentId: 'rivet',
);

Future<Harness> showPhone(WidgetTester tester, Workspace snapshot) async {
  final harness = Harness(snapshot);
  await harness.show(tester, size: phone);
  await pumpFrames(tester);
  return harness;
}

/// The conversation's leading button, which opens the drawer.
Finder menuButton() => find.bySemanticsLabel(RegExp(r'^(Menu|Back)$'));

void main() {
  testWidgets('the conversation fills the screen; the drawer chooses '
      'another agent', (tester) async {
    final harness = await showPhone(tester, workspace());
    expect(find.byType(PhoneShell), findsOneWidget);
    expect(tester.getSize(find.byType(AgentView)).width, phone.width);
    // The drawer waits, closed.
    expect(find.byType(Sidebar), findsNothing);

    await tester.tap(menuButton());
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsOneWidget);
    await tester.tap(
      find.descendant(of: find.byType(Sidebar), matching: find.text('Mike')),
    );
    await pumpFrames(tester);
    expect(harness.last.selectAgent.agentId, 'mike');
    expect(find.byType(Sidebar), findsNothing);
    await harness.apply(tester, workspace()..selectedAgentId = 'mike');
    await pumpFrames(tester);
    // The name with its role; no model or context meter at this size.
    expect(
      find.descendant(of: find.byType(AgentView), matching: find.text('Mike')),
      findsOneWidget,
    );
  });

  testWidgets('panels open as sheets from the "…" list', (tester) async {
    final harness = await showPhone(
      tester,
      workspace()..selectedAgentId = 'mike',
    );
    await tester.tap(find.bySemanticsLabel('Extensions'));
    await pumpFrames(tester);
    expect(find.text("Mike's extensions"), findsOneWidget);
    await tester.tap(find.text('Checklist'));
    await pumpFrames(tester);
    expect(harness.last.openPanel.extensionId, 'checklist');
    expect(find.text("Mike's extensions"), findsNothing);

    // The backend has it open: a sheet as wide as the screen.
    await harness.apply(
      tester,
      workspace(openPanel: 'checklist')..selectedAgentId = 'mike',
    );
    await pumpFrames(tester);
    expect(tester.getSize(find.byType(ExtensionPanel)).width, phone.width - 2);
    // A tap above it closes it: the dimmed sliver over a tall sheet.
    await tester.tapAt(const Offset(195, 3));
    await pumpFrames(tester);
    expect(harness.last.openPanel.extensionId, '');
  });

  testWidgets('the message box sits above the keyboard', (tester) async {
    await showPhone(tester, workspace()..selectedAgentId = 'mike');
    tester.view.viewInsets = const FakeViewPadding(bottom: 336);
    await pumpFrames(tester);
    final box = find
        .descendant(
          of: find.byType(AgentView),
          matching: find.byType(EditableText),
        )
        .last;
    expect(tester.getBottomLeft(box).dy, lessThan(phone.height - 336));
  });

  testWidgets('Settings, the Marketplace and What\'s new fit a phone', (
    tester,
  ) async {
    final harness = await showPhone(tester, workspace());
    for (final page in [
      AppPage.settings,
      AppPage.marketplace,
      AppPage.changelog,
    ]) {
      harness.page.value = page;
      await pumpFrames(tester);
      // An overflow would have failed the test by now.
      expect(find.byType(Sidebar), findsNothing, reason: '$page');
    }
  });

  testWidgets('with nothing paired, a phone opens to pairing, and takes only '
      'codes for the private tunnel', (tester) async {
    final harness = Harness(Workspace(), localBackend: false);
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    expect(find.byType(PairScreen), findsOneWidget);
    expect(find.byType(Sidebar), findsNothing);

    final field = find.byType(EditableText);
    await tester.enterText(
      field,
      'factory://100.64.0.2:7443?t=${'a' * 64}&fp=${'b' * 64}',
    );
    await tester.pump();
    expect(find.textContaining('from an older Claktory'), findsOneWidget);

    await tester.enterText(field, _code);
    await tester.pump();
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    expect(harness.connections.current?.endpointId, 'c' * 64);
    expect(harness.backend.target?.endpointId, 'c' * 64);
    expect(find.byType(PairScreen), findsNothing);
    expect(find.byType(NoAgentView), findsOneWidget);
  });

  testWidgets('Scan code asks for the camera, then connects to the code it '
      'scans', (tester) async {
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera.state = CameraAccess.undetermined;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    expect(harness.camera.asked, 0, reason: 'not before the person asks');
    await tester.tap(find.text('Scan code'));
    await pumpFrames(tester);
    expect(harness.camera.asked, 1);
    expect(find.byKey(const ValueKey('camera')), findsOneWidget);
    expect(find.byType(EditableText), findsNothing, reason: 'no paste box');

    harness.camera.scan('https://example.com');
    await tester.pump();
    expect(find.textContaining("That's not a Claktory code"), findsOneWidget);
    harness.camera.scan(
      'factory://100.64.0.2:7443?t=${'a' * 64}&fp=${'b' * 64}',
    );
    await tester.pump();
    expect(find.textContaining('from an older Claktory'), findsOneWidget);

    harness.camera.scan(_code);
    await pumpFrames(tester);
    expect(harness.connections.current?.endpointId, 'c' * 64);
    expect(find.byType(PairScreen), findsNothing);
  });

  testWidgets('pasting stays behind a small link, and back', (tester) async {
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera.state = CameraAccess.granted;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    expect(harness.camera.asked, 0, reason: 'already allowed');
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('camera')), findsNothing);
    await tester.enterText(find.byType(EditableText), _code);
    await tester.pump();
    await tester.tap(find.text('Scan code instead'));
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('camera')), findsOneWidget);
  });

  testWidgets('a refused camera says how to allow it, and scans once allowed', (
    tester,
  ) async {
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera
      ..state = CameraAccess.undetermined
      ..granting = false;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    await tester.tap(find.text('Scan code'));
    await pumpFrames(tester);
    expect(
      find.text('To scan, allow camera access in Settings → Claktory.'),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('camera')), findsNothing);
    await tester.tap(find.text('Open Settings'));
    await tester.pump();
    expect(harness.camera.settingsOpened, 1);

    // Allowed in Settings, then back in Claktory.
    harness.camera.state = CameraAccess.granted;
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('camera')), findsOneWidget);
  });

  testWidgets('a restricted camera says to paste instead, with no Settings', (
    tester,
  ) async {
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera.state = CameraAccess.restricted;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    expect(
      find.text(
        "The camera isn't available on this iPhone. Paste the code "
        'instead.',
      ),
      findsOneWidget,
    );
    expect(find.text('Open Settings'), findsNothing);
  });
}
