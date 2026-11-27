import 'package:factory_app/connections.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'rivet_test.dart' show admin;

Workspace snapshot({bool rivet = true}) => Workspace(
  projects: [
    Project(
      id: 'p1',
      name: 'Factory',
      path: '/Users/me/factory',
      folders: [
        ProjectFolder(id: 'app', name: 'App', path: '/Users/me/factory/app'),
        ProjectFolder(
          id: 'research',
          name: 'Research',
          path: '/Users/me/research',
        ),
      ],
    ),
    Project(id: 'p2', name: 'Site', path: '/Users/me/site'),
  ],
  agents: [
    if (rivet) admin(),
    Agent(
      id: 'a1',
      projectId: 'p1',
      name: 'Builder',
      folderId: 'app',
      modelAvailable: true,
    ),
  ],
  selectedAgentId: 'a1',
  focusedProjectId: 'p1',
);

Future<Harness> showWith(WidgetTester tester, Workspace start) async {
  final harness = Harness(start);
  await harness.show(tester);
  await settle(tester);
  return harness;
}

/// Opens the focused project's settings from its heading.
/// The focused project's heading opens its menu (§4); Project settings…
/// in it, its settings.
Future<void> openProjectSettings(WidgetTester tester) async {
  await tester.tap(find.text('Factory').first);
  await pumpFrames(tester);
  await tester.tap(find.text('Project settings…'));
  await pumpFrames(tester);
}

Future<void> reach(WidgetTester tester, Finder target) async {
  await tester.ensureVisible(target);
  await pumpFrames(tester);
}

void main() {
  testWidgets("an agent's folder is chosen in its settings", (tester) async {
    final harness = await showWith(tester, snapshot());
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    await reach(tester, find.text('Folder'));
    expect(find.text('App'), findsWidgets, reason: 'where it works now');
    await tester.tap(find.text('App').last);
    await settle(tester);
    expect(find.text("The project's folder"), findsWidgets);
    await tester.tap(find.text('Research').last);
    await settle(tester);
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(harness.last.updateAgent.folderId, 'research');
    expect(harness.last.updateAgent.hasName(), isFalse);
  });

  testWidgets('a project without folders offers no folder choice', (
    tester,
  ) async {
    final start = snapshot()..projects[0].folders.clear();
    start.agents[1].folderId = '';
    await showWith(tester, start);
    await tester.tap(find.text('Builder').last);
    await pumpFrames(tester);
    expect(find.text('Folder'), findsNothing);
  });

  testWidgets("the project's settings rename, re-point and remove folders", (
    tester,
  ) async {
    final harness = await showWith(tester, snapshot());
    await openProjectSettings(tester);
    expect(find.text('Folders'), findsOneWidget);
    Finder field(String text) => find.byWidgetPredicate(
      (widget) => widget is EditableText && widget.controller.text == text,
    );
    await tester.enterText(field('App'), 'Repo');
    await tester.enterText(field('/Users/me/research'), '/Users/me/notes');
    await tester.pump();
    await tester.tap(
      find.byWidgetPredicate(
        (widget) =>
            widget is Semantics && widget.properties.label == 'Remove Research',
      ),
    );
    await tester.pump();
    // Re-pointing a removed folder doesn't count; removing does.
    await tester.tap(find.text('Save'));
    await settle(tester);
    final sent = harness.backend.sent;
    expect(sent, hasLength(2));
    expect(
      sent[0].updateFolder,
      UpdateFolder(projectId: 'p1', folderId: 'app', name: 'Repo'),
    );
    expect(
      sent[1].deleteFolder,
      DeleteFolder(projectId: 'p1', folderId: 'research'),
    );
  });

  testWidgets('a folder is added by Rivet', (tester) async {
    final harness = await showWith(tester, snapshot());
    harness.backend.onSend = (message) {
      if (message.hasSelectAgent()) {
        harness.backend.workspace.apply(
          harness.backend.workspace.workspace.deepCopy()
            ..selectedAgentId = message.selectAgent.agentId,
        );
      }
    };
    await openProjectSettings(tester);
    await reach(tester, find.text('Add a folder…'));
    await tester.tap(find.text('Add a folder…'));
    await settle(tester);
    expect(find.text('Project settings'), findsNothing, reason: 'closed');
    expect(harness.last.selectAgent.agentId, 'rivet');
    expect(
      tester.widget<EditableText>(find.byType(EditableText)).controller.text,
      'Add a folder to Factory for ',
    );
  });

  testWidgets("without Rivet, a folder is added with the form", (tester) async {
    final harness = await showWith(tester, snapshot(rivet: false));
    const channel = MethodChannel('factory/window');
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      channel,
      (call) async =>
          call.method == 'pickFolder' ? '/Users/me/factory/docs' : null,
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        null,
      ),
    );
    await openProjectSettings(tester);
    await reach(tester, find.text('Add a folder…'));
    await tester.tap(find.text('Add a folder…'));
    await pumpFrames(tester);
    expect(find.text('New folder in Factory'), findsOneWidget);
    // The name field has the focus.
    await tester.enterText(
      find.byWidgetPredicate(
        (widget) => widget is EditableText && widget.focusNode.hasFocus,
      ),
      'Docs',
    );
    await tester.pump();
    await tester.tap(find.text('Choose…'));
    await settle(tester);
    await tester.tap(find.text('Add folder'));
    await settle(tester);
    expect(
      harness.last.createFolder,
      CreateFolder(
        projectId: 'p1',
        name: 'Docs',
        path: '/Users/me/factory/docs',
      ),
    );
  });

  /// Connected to a Factory on mac-mini, whose folders this Mac's sheet
  /// can't show.
  Future<Harness> showRemote(WidgetTester tester) async {
    final harness = await showWith(tester, snapshot(rivet: false));
    harness.backend.connectTo(
      parsePairingCode(
        'factory://100.64.0.2:7443?t=${'a' * 64}&fp=${'b' * 64}&name=mac-mini',
      ),
    );
    await harness.apply(
      tester,
      snapshot(rivet: false)..remote = RemoteAccess(machine: 'mac-mini'),
    );
    return harness;
  }

  Future<void> addFolder(WidgetTester tester) async {
    await openProjectSettings(tester);
    await reach(tester, find.text('Add a folder…'));
    await tester.tap(find.text('Add a folder…'));
    await pumpFrames(tester);
  }

  testWidgets("on a Factory on another machine, a folder's path can be typed", (
    tester,
  ) async {
    final harness = await showRemote(tester);
    await addFolder(tester);
    expect(
      find.text('A path on mac-mini, where this Claktory runs.'),
      findsOneWidget,
    );
    await tester.enterText(
      find.byWidgetPredicate(
        (widget) => widget is EditableText && widget.focusNode.hasFocus,
      ),
      'Docs',
    );
    await tester.enterText(find.byType(EditableText).last, '/srv/docs');
    await tester.pump();
    await tester.tap(find.text('Add folder'));
    await settle(tester);
    expect(
      harness.last.createFolder,
      CreateFolder(projectId: 'p1', name: 'Docs', path: '/srv/docs'),
    );
  });

  testWidgets(
    "on a Factory on another machine, Choose… browses that machine's folders",
    (tester) async {
      final harness = await showRemote(tester);
      harness.backend.folderLists.addAll({
        '/Users/me/factory': FolderList(
          path: '/Users/me/factory',
          parent: '/Users/me',
          home: '/Users/me',
          folders: [
            FolderEntry(name: '.git', hidden: true),
            FolderEntry(name: 'app'),
            FolderEntry(name: 'crates'),
          ],
        ),
        '/Users/me/factory/app': FolderList(
          path: '/Users/me/factory/app',
          parent: '/Users/me/factory',
          home: '/Users/me',
          folders: [FolderEntry(name: 'lib')],
        ),
        '/Users/me': FolderList(
          path: '/Users/me',
          parent: '/Users',
          home: '/Users/me',
          folders: [
            FolderEntry(name: 'code'),
            FolderEntry(name: 'factory'),
          ],
        ),
        '~': FolderList(
          path: '/Users/me',
          parent: '/Users',
          home: '/Users/me',
          folders: [
            FolderEntry(name: 'code'),
            FolderEntry(name: 'factory'),
          ],
        ),
        '/Users': FolderList(
          path: '/Users',
          parent: '/',
          home: '/Users/me',
          folders: [
            FolderEntry(name: 'me'),
            FolderEntry(name: 'Shared'),
          ],
        ),
      });
      await addFolder(tester);
      await tester.tap(find.text('Choose…'));
      await pumpFrames(tester);
      // It opens at the project's folder, on mac-mini.
      expect(harness.last.listFolders.path, '/Users/me/factory');
      expect(find.text('Choose a folder'), findsOneWidget);
      expect(
        find.text('On mac-mini, where this Claktory runs.'),
        findsOneWidget,
      );
      expect(find.text('~'), findsOneWidget);
      expect(find.text('app'), findsOneWidget);
      expect(find.text('crates'), findsOneWidget);
      expect(find.text('.git'), findsNothing, reason: 'hidden');
      await tester.tap(
        find.byWidgetPredicate((w) => w is Toggle && w.label == 'Show hidden'),
      );
      await tester.pump();
      expect(find.text('.git'), findsOneWidget);

      await tester.tap(find.text('app'));
      await tester.pump();
      expect(find.text('lib'), findsOneWidget);
      await tester.tap(find.text('Up'));
      await tester.pump();
      expect(find.text('crates'), findsOneWidget);
      await tester.tap(find.text('~'));
      await tester.pump();
      expect(find.text('code'), findsOneWidget);
      await tester.tap(find.text('Up'));
      await tester.pump();
      expect(find.text('Shared'), findsOneWidget);
      expect(
        find.text('/'),
        findsOneWidget,
        reason: 'outside home, from the root',
      );
      await tester.tap(find.text('Home'));
      await tester.pump();
      expect(harness.last.listFolders.path, '/Users/me');
      await tester.tap(find.text('factory').last);
      await tester.pump();
      await tester.tap(find.text('app'));
      await tester.pump();
      await tester.tap(find.text('Choose'));
      await pumpFrames(tester);
      expect(find.text('Choose a folder'), findsNothing);
      expect(
        tester
            .widget<EditableText>(find.byType(EditableText).last)
            .controller
            .text,
        '/Users/me/factory/app',
      );
    },
  );

  testWidgets(
    "the browser opens at home when what's typed doesn't exist yet, and says "
    "why a folder can't be opened",
    (tester) async {
      final harness = await showRemote(tester);
      harness.backend.folderLists['~'] = FolderList(
        path: '/Users/me',
        parent: '/Users',
        home: '/Users/me',
        folders: [FolderEntry(name: 'private')],
      );
      await addFolder(tester);
      await tester.enterText(find.byType(EditableText).last, '/srv/new');
      await tester.pump();
      await tester.tap(find.text('Choose…'));
      await pumpFrames(tester);
      expect(
        [
          for (final m in harness.backend.sent)
            if (m.hasListFolders()) m.listFolders.path,
        ],
        ['/srv/new', '~'],
      );
      expect(find.text('private'), findsOneWidget);
      await tester.tap(find.text('private'));
      await tester.pump();
      expect(
        find.text("Can't read /Users/me/private: it doesn't exist"),
        findsOneWidget,
      );
      expect(
        find.text('private'),
        findsOneWidget,
        reason: 'it stays where it was',
      );
      await tester.tap(find.text('Cancel').last);
      await pumpFrames(tester);
      expect(
        tester
            .widget<EditableText>(find.byType(EditableText).last)
            .controller
            .text,
        '/srv/new',
      );
    },
  );
}
