// A project's Files against a real Claktory, on an iPhone (the Simulator):
// a "files" pane in an agent's layout brings that project's Explorer though
// it isn't focused, the phone's Files page (WatchFiles) brings another's,
// and a folder opened in one comes back expanded. The projects are made
// here, in temporary folders, and deleted at the end.
//
//   cd app && flutter test integration_test/files_test.dart \
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

  testWidgets("Files panes and the phone's page bring their projects' trees", (
    tester,
  ) async {
    final target = parsePairingCode(_code);
    expect(target, isNotNull, reason: 'pass --dart-define=PAIRING_CODE=…');
    final key = newDeviceKey();
    final backend =
        Backend(
            workspace: WorkspaceStore(),
            transcript: TranscriptStore(),
            explorer: ExplorerStore(),
            extensionView: ExtensionViewStore(),
            localBackend: false,
          )
          ..deviceKey = ((_) => key)
          ..connectTo(target);
    unawaited(backend.run());
    Future<void> until(String what, bool Function() done) async {
      final end = DateTime.now().add(const Duration(seconds: 30));
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await tester.pump(const Duration(milliseconds: 100));
      }
    }

    Workspace shown() => backend.workspace.workspace;
    await until(
      'connected',
      () => backend.connected.value && shown().agents.isNotEmpty,
    );

    final stamp = DateTime.now().millisecondsSinceEpoch % 100000;
    Future<Project> project(String name) async {
      await backend.createProject(name);
      await until(
        'project $name',
        () => shown().projects.any((p) => p.name == name),
      );
      return shown().projects.firstWhere((p) => p.name == name);
    }

    final paned = await project('Paned$stamp');
    final watched = await project('Watched$stamp');
    await backend.focusProject('');
    await backend.createAgent(
      CreateAgent(name: 'Ada$stamp', projectId: paned.id),
    );
    await until(
      'the agent',
      () => shown().agents.any((a) => a.name == 'Ada$stamp'),
    );
    final agent = shown().agents.firstWhere((a) => a.name == 'Ada$stamp');

    expect(backend.explorer.of(paned.id), isNull, reason: 'not open yet');
    await backend.setLayout(
      agent.id,
      ViewNode(
        id: 'root',
        split: ViewSplit(
          axis: ViewAxis.VIEW_AXIS_ROW,
          children: [
            ViewNode(
              id: 'chat',
              leaf: ViewLeaf(kind: 'chat'),
            ),
            ViewNode(
              id: 'files',
              leaf: ViewLeaf(kind: 'files', path: paned.id),
            ),
          ],
        ),
      ),
    );
    await until(
      "the pane's project's tree",
      () => backend.explorer.of(paned.id) != null,
    );
    // A new project's folder holds its .factoryignore, hidden: no rows.
    expect(backend.explorer.of(paned.id)!.error, '');

    await backend.watchFiles(watched.id);
    await until(
      "the watched project's tree",
      () => backend.explorer.of(watched.id) != null,
    );
    await backend.showAllFiles(watched.id, true);
    await until(
      'Show hidden files',
      () => backend.explorer.of(watched.id)!.showAll,
    );
    expect(
      backend.explorer.of(watched.id)!.entries.map((entry) => entry.name),
      contains('.factoryignore'),
    );

    await backend.watchFiles('');
    await backend.deleteAgent(agent.id);
    await backend.deleteProject(paned.id);
    await backend.deleteProject(watched.id);
    // ignore: avoid_print
    print('Files trees came for a pane and a watch');
    await backend.close();
  });
}
