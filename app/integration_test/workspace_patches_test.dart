// Workspace patches against a real Claktory, on an iPhone (the Simulator):
// two connections to the same Claktory, one asking for patches and one for
// whole Workspaces, must show the same Workspace after every change. The
// changes are made here: projects, agents, prompts, selection, archiving,
// settings, a layout.
//
//   cd app && flutter test integration_test/workspace_patches_test.dart \
//     -d <simulator> --dart-define=PAIRING_CODE='factory://…'
import 'dart:async';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/tunnel_client.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

const _code = String.fromEnvironment('PAIRING_CODE');

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('patched and whole Workspaces stay the same', (tester) async {
    final target = parsePairingCode(_code);
    expect(target, isNotNull, reason: 'pass --dart-define=PAIRING_CODE=…');
    final key = newDeviceKey();
    Backend connect({required bool patches}) {
      final backend =
          Backend(
              workspace: WorkspaceStore(),
              transcript: TranscriptStore(),
              explorer: ExplorerStore(),
              extensionView: ExtensionViewStore(),
              localBackend: false,
              localPairing: () async => null,
            )
            ..deviceKey = ((_) => key)
            ..askForPatches = patches
            ..connectTo(target);
      unawaited(backend.run());
      return backend;
    }

    final patched = connect(patches: true);
    final whole = connect(patches: false);
    Future<void> until(String what, bool Function() done) async {
      final end = DateTime.now().add(const Duration(seconds: 30));
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await tester.pump(const Duration(milliseconds: 100));
      }
    }

    Workspace shown(Backend backend) => backend.workspace.workspace;
    var steps = 0;
    Future<void> same(String step) async {
      steps++;
      final end = DateTime.now().add(const Duration(seconds: 10));
      while (shown(patched) != shown(whole)) {
        if (DateTime.now().isAfter(end)) {
          final a = shown(patched), b = shown(whole);
          final differ = [
            for (final field in Workspace().info_.fieldInfo.values)
              if (a.getField(field.tagNumber).toString() !=
                  b.getField(field.tagNumber).toString())
                field.name,
          ];
          fail('after $step, the patched Workspace differs in $differ');
        }
        await tester.pump(const Duration(milliseconds: 100));
      }
    }

    await until(
      'connected',
      () =>
          patched.connected.value &&
          whole.connected.value &&
          shown(patched).agents.isNotEmpty &&
          shown(whole).agents.isNotEmpty,
    );
    await same('connecting');

    final stamp = DateTime.now().millisecondsSinceEpoch % 100000;
    final name = 'Patch$stamp';
    await patched.createProject(name);
    await same('creating a project');
    final project = shown(patched).projects.firstWhere((p) => p.name == name);
    await patched.createAgent(
      CreateAgent(name: 'Ada$stamp', projectId: project.id),
    );
    await same('creating an agent');
    final agent = shown(patched).agents
        .firstWhere((a) => a.name == 'Ada$stamp');
    await patched.updateAgent(
      UpdateAgent(agentId: agent.id, prompt: 'Builds things.', role: 'Maker'),
    );
    await same('changing its prompt and role');
    await patched.updateAgent(UpdateAgent(agentId: agent.id, prompt: ''));
    await same('emptying its prompt');
    await patched.selectAgent(agent.id);
    await same('selecting it');
    await patched.focusProject(project.id);
    await same('focusing its project');
    await patched.focusProject('');
    await same('unfocusing it');
    await patched.setLayout(
      agent.id,
      ViewNode(
        id: 'root',
        split: ViewSplit(
          axis: ViewAxis.VIEW_AXIS_ROW,
          children: [
            ViewNode(id: 'chat', leaf: ViewLeaf(kind: 'chat')),
            ViewNode(id: 'p1', leaf: ViewLeaf(kind: 'settings')),
          ],
        ),
      ),
    );
    await same('a layout');
    await patched.setLayout(agent.id, null);
    await same('its layout back');
    await patched.sendMessage(agent.id, 'Hello');
    await same('a message (its run fails without a model)');
    await patched.updateProject(
      UpdateProject(projectId: project.id, archived: true),
    );
    await same('archiving the project');
    await patched.updateProject(
      UpdateProject(projectId: project.id, archived: false, name: '$name b'),
    );
    await same('unarchiving and renaming it');
    final user = shown(patched).settings.userName;
    await patched.updateSettings(UpdateSettings(userName: 'Lee$stamp'));
    await same('a setting');
    await patched.updateSettings(UpdateSettings(userName: user));
    await same('the setting back');
    await patched.deleteAgent(agent.id);
    await same('deleting the agent');
    await patched.deleteProject(project.id);
    await same('deleting the project');
    // ignore: avoid_print
    print('the same after $steps steps');
    await patched.close();
    await whole.close();
  });
}
