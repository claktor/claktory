// An agent's settings in their order, drawn as they look,
// to check by eye: on a Mac window and on an iPhone, at the default text
// and the largest on the iPhone; the top, its folder, the extensions folded
// and unfolded, the foot, and opened at its extensions (⋯ → Extensions…). With
// SHOTS set, each is drawn to a PNG there:
//   SHOTS=/tmp/settings flutter test test/agent_settings_shots_test.dart
// Without it, each still lays out without an overflow or an exception.
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/controls.dart' show showModal;
import 'package:factory_app/forms.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show gpt, opus;
import 'extensions_test.dart'
    show agentsExtension, agentsOn, checklistExtension;
import 'harness.dart';
import 'ipad/window_fixtures.dart' show TextFactor;
import 'phone_sizes_test.dart' show loadGeist;

Workspace _workspace() => Workspace(
  models: [opus, gpt],
  providers: [
    anthropic(signedInWith: 'Anthropic (Claude Pro/Max)', canSignOut: true),
  ],
  settings: Settings(
    defaultModel: 'anthropic/claude-opus-5-5',
    defaultEffort: 'high',
  ),
  projects: [
    Project(
      id: 'p1',
      name: 'Claktory',
      path: '/Users/me/factory',
      folders: [
        ProjectFolder(id: 'app', name: 'App', path: '/Users/me/factory/app'),
      ],
    ),
  ],
  agents: [
    Agent(
      id: 'rivet',
      name: 'Rivet',
      role: 'Admin',
      modelAvailable: true,
      extensions: [agentsOn()],
    ),
    Agent(
      id: 'june',
      name: 'June',
      role: 'Backend',
      projectId: 'p1',
      model: 'anthropic/claude-opus-5-5',
      effort: 'high',
      modelAvailable: true,
      prompt: [
        'You build the backend: crates/ and the protocol.',
        'Read the code around what you change before you change it.',
        'Keep the manual true when what Claktory does changes.',
        'Run the checks before you report.',
        'Ask Bruno before touching another agent\'s files.',
        'Commit on your own branch; land only when told.',
        'Report to Bruno with what changed and how you checked it.',
        'Never print a secret.',
      ].join('\n'),
      extensions: [
        agentsOn(),
        AgentExtension(id: 'checklist', enabled: true),
      ],
    ),
  ],
  extensions: [agentsExtension, checklistExtension],
  selectedAgentId: 'june',
);

Future<void> _shot(WidgetTester tester, String name) async {
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

void main() {
  setUpAll(loadGeist);

  // The largest text on the iPhone, where it's set (a Mac's sidebar isn't
  // drawn for it).
  for (final (phone, large) in [(false, false), (true, false), (true, true)]) {
    {
      final where = '${phone ? 'iphone' : 'mac'}${large ? '-ax5' : ''}';
      testWidgets('agent settings on the $where', (tester) async {
        if (phone) debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        if (large) {
          tester.platformDispatcher.textScaleFactorTestValue = TextFactor.ax5;
          addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        }
        final workspace = _workspace();
        final harness = Harness(workspace);
        await harness.show(
          tester,
          size: phone ? const Size(393, 852) : const Size(1100, 900),
        );
        await pumpFrames(tester);
        showModal(
          tester.element(find.text('June').first),
          (context, close) =>
              AgentSettingsForm(agent: workspace.agents[1], close: close),
        );
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
        expect(find.text('Or ask Rivet to change June'), findsOneWidget);
        await _shot(tester, 'settings-$where-top');
        // The project has folders: Folder beside (or under) Project.
        await tester.ensureVisible(find.text('Folder'));
        await pumpFrames(tester);
        expect(find.text('Project'), findsOneWidget);
        await _shot(tester, 'settings-$where-folder');
        await tester.ensureVisible(find.text('Change…'));
        await pumpFrames(tester);
        expect(find.text('2 on ·'), findsOneWidget);
        await _shot(tester, 'settings-$where-folded');
        await unfoldExtensions(tester);
        expect(tester.takeException(), isNull);
        await _shot(tester, 'settings-$where-unfolded');
        await tester.ensureVisible(find.text('Delete agent…'));
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
        await _shot(tester, 'settings-$where-foot');
      });

      // From ⋯ → Extensions… open, there.
      testWidgets('agent settings at its extensions on the $where', (
        tester,
      ) async {
        if (phone) debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        if (large) {
          tester.platformDispatcher.textScaleFactorTestValue = TextFactor.ax5;
          addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        }
        final workspace = _workspace();
        final harness = Harness(workspace);
        await harness.show(
          tester,
          size: phone ? const Size(393, 852) : const Size(1100, 900),
        );
        await pumpFrames(tester);
        showModal(
          tester.element(find.text('June').first),
          (context, close) => AgentSettingsForm(
            agent: workspace.agents[1],
            close: close,
            showExtensions: true,
          ),
        );
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
        expect(find.text('2 on ·'), findsNothing);
        await _shot(tester, 'settings-$where-from-menu');
      });
    }
  }
}
