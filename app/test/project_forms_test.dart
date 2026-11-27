// Area 5b's forms (Lucy's projects-sidebar.md §5.2, §6, §7; Trish's words
// in §12): where a new project's files will go, as the backend says;
// Project settings starting with the project's folder, with Show in
// Finder on the Mac that runs the agents; what Delete does, with Archive
// instead; and what moving an agent to another project changes.
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/forms.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/share_files.dart' as share;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _bakery = Project(
  id: 'p1',
  name: 'Bakery',
  path: '/Users/me/.factory/workspace/bakery',
  folders: [
    ProjectFolder(id: 'site', name: 'Site', path: '/Users/me/code/site'),
  ],
);

Workspace _workspace({bool pipes = false}) => Workspace(
  projects: [
    _bakery,
    Project(id: 'p2', name: 'Research', path: '/Users/me/research'),
  ],
  agents: [
    for (final (id, name) in [('a1', 'June'), ('a2', 'Omar'), ('a3', 'Bo')])
      Agent(id: id, name: name, projectId: 'p1', modelAvailable: true),
  ],
  selectedAgentId: 'a1',
  pipes: [
    if (pipes)
      for (final (id, owner) in [('x1', 'Fred'), ('x2', 'Ana')])
        Pipe(
          id: id,
          name: owner,
          projectId: 'p1',
          peer: PipePeer(ownerName: owner),
        ),
  ],
);

Finder _button(String label) => find.widgetWithText(Button, label);

void main() {
  testWidgets("New project says where its folder will be, as it's named", (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await harness.show(tester, home: CreateProjectForm(close: () {}));
    await tester.enterText(find.byType(EditableText), 'Bakery website');
    await tester.pump();
    // Asked once typing pauses.
    expect(find.textContaining('Folder:'), findsNothing);
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pump();
    expect(harness.last.previewProjectFolder.name, 'Bakery website');
    expect(
      find.text('Folder: ~/.factory/workspace/bakery-website'),
      findsOneWidget,
    );
    // Renamed, the old folder goes at once: it waits for this name's.
    await tester.enterText(find.byType(EditableText), 'Bakery website two');
    await tester.pump();
    expect(find.textContaining('Folder:'), findsNothing);
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pump();
    expect(
      find.text('Folder: ~/.factory/workspace/bakery-website-two'),
      findsOneWidget,
    );
    // An answer to a name since changed is dropped; a blank name has none.
    await tester.enterText(find.byType(EditableText), '');
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.textContaining('Folder:'), findsNothing);
  });

  testWidgets('Project settings starts with its folder; on the Mac that runs '
      'it, Show in Finder shows it and each folder', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final shown = <String>[];
    final was = share.showInFinder;
    share.showInFinder = (files) async =>
        shown.addAll([for (final file in files) file.path]);
    addTearDown(() => share.showInFinder = was);
    final harness = Harness(_workspace());
    await harness.show(
      tester,
      size: const Size(1000, 1200),
      home: ProjectSettingsForm(project: _bakery, close: () {}),
    );
    await settle(tester);
    expect(find.text('~/.factory/workspace/bakery'), findsOneWidget);
    expect(
      tester.getTopLeft(find.text('Folder')).dy,
      lessThan(tester.getTopLeft(find.text('Name')).dy),
    );
    expect(_button('Show in Finder'), findsNWidgets(2));
    await tester.tap(_button('Show in Finder').first);
    await tester.tap(_button('Show in Finder').last);
    expect(shown, [
      '/Users/me/.factory/workspace/bakery',
      '/Users/me/code/site',
    ]);
  });

  testWidgets('anywhere but that Mac, the folder is its path alone', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await harness.show(
      tester,
      size: const Size(1000, 1200),
      home: ProjectSettingsForm(project: _bakery, close: () {}),
    );
    await settle(tester);
    expect(find.text('~/.factory/workspace/bakery'), findsOneWidget);
    expect(_button('Show in Finder'), findsNothing);
  });

  testWidgets('a long path is cut in the middle, keeping its start and end', (
    tester,
  ) async {
    const path = '~/code/clients/bakery/website/the-long-folder/src/app';
    await tester.pumpWidget(
      const Directionality(
        textDirection: TextDirection.ltr,
        child: Center(
          child: SizedBox(
            width: 200,
            child: MiddleCut(path, style: TextStyle(fontSize: 13)),
          ),
        ),
      ),
    );
    final shown = tester.widget<Text>(find.byType(Text)).data!;
    expect(shown, contains('…'));
    expect(shown, startsWith('~/'));
    expect(shown, endsWith('src/app'));
    expect(tester.getSize(find.byType(Text)).width, lessThanOrEqualTo(200));
  });

  test("Delete's words: agents, where the files stay, the pipes", () {
    expect(
      deleteProjectBody(_workspace(pipes: true), _bakery),
      'This deletes its 3 agents and their conversations. Its folder and '
      'files stay in ~/.factory/workspace/bakery. Its pipes with Fred and '
      'Ana close, and their messages here are deleted for good.',
    );
  });

  testWidgets("Delete says where the files stay and which pipes close; "
      'Archive instead archives it', (tester) async {
    final harness = Harness(_workspace(pipes: true));
    await harness.show(
      tester,
      size: const Size(1000, 1200),
      home: ProjectSettingsForm(project: _bakery, close: () {}),
    );
    await settle(tester);
    // Archive's note says the pipes go quiet.
    expect(
      find.textContaining('Its pipes stay open, but nothing answers'),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Delete project…'));
    await tester.tap(find.text('Delete project…'));
    await settle(tester);
    expect(find.text('Delete Bakery?'), findsOneWidget);
    expect(
      find.textContaining('Its pipes with Fred and Ana close'),
      findsOneWidget,
    );
    await tester.tap(_button('Archive instead'));
    await settle(tester);
    expect(
      harness.last.updateProject,
      UpdateProject(projectId: 'p1', archived: true),
    );
    expect(
      harness.backend.sent.where((message) => message.hasDeleteProject()),
      isEmpty,
    );
  });

  testWidgets("on a phone, Delete's sheet stacks: Delete project full "
      'width, then Archive instead and Cancel as links', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace(pipes: true));
    await harness.show(
      tester,
      size: const Size(393, 852),
      home: ProjectSettingsForm(project: _bakery, close: () {}),
    );
    await settle(tester);
    await tester.ensureVisible(find.text('Delete project…'));
    await settle(tester);
    await tester.tap(find.text('Delete project…'));
    await settle(tester);
    final delete = tester.getRect(_button('Delete project'));
    final archive = tester.getRect(find.text('Archive instead'));
    final cancel = tester.getRect(find.text('Cancel'));
    expect(delete.width, greaterThan(300));
    expect(archive.top, greaterThan(delete.bottom));
    expect(cancel.top, greaterThan(archive.bottom));
    // Centred under it.
    expect(archive.center.dx, moreOrLessEquals(delete.center.dx, epsilon: 1));
    expect(cancel.center.dx, moreOrLessEquals(delete.center.dx, epsilon: 1));
    expect(_button('Archive instead'), findsNothing);
    expect(_button('Cancel'), findsNothing);
    await tester.tap(find.text('Archive instead'));
    await settle(tester);
    expect(
      harness.last.updateProject,
      UpdateProject(projectId: 'p1', archived: true),
    );
  });

  test("moving an agent says what changes, only once it's moved", () {
    final workspace = _workspace();
    final june = workspace.agents.first;
    expect(
      projectMoveNote(workspace, agent: june, to: 'p1', admin: false),
      isNull,
    );
    expect(
      projectMoveNote(workspace, agent: june, to: 'p2', admin: false),
      "June will work in Research's folder and can message only agents in "
      'Research.',
    );
    expect(
      projectMoveNote(workspace, agent: june, to: '', admin: false),
      'June will work in the workspace folder and can message only Rivet '
      'and other agents outside projects.',
    );
    expect(
      projectMoveNote(workspace, agent: june, to: 'p2', admin: true),
      "June will work in Research's folder.",
    );
  });

  testWidgets("Agent settings' Project says it under the field", (
    tester,
  ) async {
    final workspace = _workspace();
    final harness = Harness(workspace);
    await harness.show(
      tester,
      size: const Size(1000, 1600),
      home: AgentSettingsForm(agent: workspace.agents.first, close: () {}),
    );
    await settle(tester);
    expect(find.textContaining('will work in'), findsNothing);
    await tester.ensureVisible(find.text('Bakery').last);
    await tester.tap(find.text('Bakery').last);
    await settle(tester);
    await tester.tap(find.text('Research').last);
    await settle(tester);
    expect(
      find.text(
        "June will work in Research's folder and can message only agents "
        'in Research.',
      ),
      findsOneWidget,
    );
  });
}
