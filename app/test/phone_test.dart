import 'package:factory_app/agent_view.dart';
import 'package:factory_app/extensions.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart' show NoAgentView;
import 'package:factory_app/sidebar.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'pair_start.dart';

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
    await pastPairStart(tester, paste: true);

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
    // A pasted code asks first, as a link does: nothing yet.
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(harness.connections.current, isNull);
    await tester.tap(find.text('Connect').last);
    await pumpFrames(tester);
    expect(harness.connections.current?.endpointId, 'c' * 64);
    expect(harness.backend.target?.endpointId, 'c' * 64);
    expect(find.byType(PairScreen), findsNothing);
    expect(find.byType(NoAgentView), findsOneWidget);
  });

  testWidgets('Scan code asks for the camera, then connects to the code it '
      'scans', (tester) async {
    // The iPhone's words (tests run as Android unless told).
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera.state = CameraAccess.undetermined;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    expect(harness.camera.asked, 0, reason: 'not before the person asks');
    await tester.tap(find.text('Scan code instead'));
    await pumpFrames(tester);
    // Why first (area 5a, P7); Continue asks.
    expect(harness.camera.asked, 0);
    await tester.tap(find.text('Continue'));
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
    // A scanned code asks first, as a link does, and asks once, however
    // often the camera reads it meanwhile.
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    harness.camera.scan(_code);
    await pumpFrames(tester);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(harness.connections.current, isNull);
    // Cancel: nothing connects, and the same code in view doesn't ask
    // again.
    await tester.tap(find.text('Cancel').last);
    await pumpFrames(tester);
    expect(harness.connections.current, isNull);
    harness.camera.scan(_code);
    await pumpFrames(tester);
    expect(find.text('Connect to this Claktory?'), findsNothing);
    // Another code asks; then the first again, and Connect connects.
    harness.camera.scan('factory://?t=${'e' * 64}&name=pi&e=${'f' * 64}');
    await pumpFrames(tester);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    await tester.tap(find.text('Cancel').last);
    await pumpFrames(tester);
    harness.camera.scan(_code);
    await pumpFrames(tester);
    await tester.tap(find.text('Connect').last);
    await pumpFrames(tester);
    expect(harness.connections.current?.endpointId, 'c' * 64);
    expect(find.byType(PairScreen), findsNothing);
  });

  testWidgets('a claktory:// code, pasted or scanned, asks and connects as '
      'a factory:// one does', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final code = 'claktory://?t=${'a' * 64}&name=mac-mini&e=${'c' * 64}';
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera.state = CameraAccess.granted;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    await pastPairStart(tester);
    expect(find.byKey(const ValueKey('camera')), findsOneWidget);
    harness.camera.scan(code);
    await pumpFrames(tester);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    await tester.tap(find.text('Cancel').last);
    await pumpFrames(tester);
    expect(harness.connections.current, isNull);
    // Pasted instead.
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText), code);
    await tester.pump();
    await tester.tap(find.text('Connect'));
    await pumpFrames(tester);
    expect(find.text('Connect to this Claktory?'), findsOneWidget);
    expect(harness.connections.current, isNull);
    await tester.tap(find.text('Connect').last);
    await pumpFrames(tester);
    expect(harness.connections.current?.code, code);
    expect(harness.backend.target?.endpointId, 'c' * 64);
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

  testWidgets('the camera the first time: why, then Continue asks, and '
      'allowed, it scans', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera.state = CameraAccess.undetermined;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    await tester.tap(find.text('Scan code instead'));
    await pumpFrames(tester);
    expect(find.text('Scan the code'), findsOneWidget);
    expect(
      find.text(
        'Claktory uses the camera to read the code your Claktory '
        'shows. Your iPhone asks next.',
      ),
      findsOneWidget,
    );
    expect(find.text('Connect to your Claktory'), findsNothing);
    expect(find.byKey(const ValueKey('camera')), findsNothing);
    expect(harness.camera.asked, 0);
    await tester.tap(find.text('Continue'));
    await pumpFrames(tester);
    expect(harness.camera.asked, 1);
    expect(find.byKey(const ValueKey('camera')), findsOneWidget);
  });

  testWidgets('a refused camera says how to allow it, and scans once allowed', (
    tester,
  ) async {
    // An iPhone's words (Android's: mobile_pair_android_test.dart).
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera
      ..state = CameraAccess.undetermined
      ..granting = false;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    await tester.tap(find.text('Scan code instead'));
    await pumpFrames(tester);
    // Why first (area 5a, P7): nothing asked until Continue.
    expect(find.text('Scan the code'), findsOneWidget);
    expect(harness.camera.asked, 0);
    await tester.tap(find.text('Continue'));
    await pumpFrames(tester);
    expect(find.text('The camera is off for Claktory'), findsOneWidget);
    expect(find.text('Paste code instead'), findsOneWidget);
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

  testWidgets('friend bug #2: a refused camera, then Paste: Scan code '
      'instead leads back to Open Settings, and scans once allowed', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera.state = CameraAccess.denied;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester);
    expect(find.text('Open Settings'), findsNothing);
    await tester.tap(find.text('Scan code instead'));
    await pumpFrames(tester);
    expect(find.text('Open Settings'), findsOneWidget);
    await tester.tap(find.text('Open Settings'));
    await tester.pump();
    expect(harness.camera.settingsOpened, 1);
    // Allowed in Settings, then back in Claktory: the camera.
    harness.camera.state = CameraAccess.granted;
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await pumpFrames(tester);
    expect(find.byKey(const ValueKey('camera')), findsOneWidget);
  });

  testWidgets('a restricted camera, then Paste: no Scan code instead', (
    tester,
  ) async {
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera.state = CameraAccess.restricted;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester);
    expect(find.text('Scan code instead'), findsNothing);
  });

  testWidgets('a restricted camera offers only pasting, with no Settings', (
    tester,
  ) async {
    // The iPhone's words (tests run as Android unless told).
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera.state = CameraAccess.restricted;
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    // The first screen offers no scanning, only pasting.
    expect(find.text('Scan code instead'), findsNothing);
    expect(find.text('Open Settings'), findsNothing);
    // From signing in, Use a code instead: it says why there's no
    // scanning.
    await pastPairStart(tester, email: true);
    await tester.tap(find.text('Use a code instead'));
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
