import 'package:factory_app/explorer.dart';
import 'package:factory_app/layout.dart' show chatOnly, leaves, open;
import 'package:factory_app/mobile/drawer.dart';
import 'package:factory_app/panes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const row = ViewAxis.VIEW_AXIS_ROW;

ViewNode chat() => ViewNode(
  id: 'chat',
  leaf: ViewLeaf(kind: 'chat'),
);

/// The conversation with [projectId]'s Files beside it.
ViewNode withFilesOf(String projectId) => ViewNode(
  id: 's1',
  split: ViewSplit(
    axis: row,
    children: [
      chat(),
      ViewNode(
        id: 'f1',
        leaf: ViewLeaf(kind: 'files', path: projectId),
      ),
    ],
  ),
);

Workspace snapshot({ViewNode? layout, String focused = ''}) => Workspace(
  projects: [
    Project(id: 'p1', name: 'Factory', path: '/Users/me/factory'),
    Project(id: 'p2', name: 'Site', path: '/Users/me/site'),
  ],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', modelAvailable: true),
    Agent(
      id: 'a1',
      projectId: 'p1',
      name: 'Builder',
      modelAvailable: true,
      layout: layout,
    ),
    Agent(id: 'a2', projectId: 'p2', name: 'Writer', modelAvailable: true),
  ],
  selectedAgentId: 'a1',
  focusedProjectId: focused,
  highlights: true,
);

Explorer tree({
  String projectId = 'p1',
  bool showAll = false,
  List<FileEntry>? entries,
  String error = '',
}) => Explorer(
  projectId: projectId,
  showAll: showAll,
  error: error,
  entries:
      entries ??
      [
        FileEntry(path: 'app', name: 'app', isDir: true, expanded: true),
        FileEntry(path: 'app/lib', name: 'lib', depth: 1, isDir: true),
        FileEntry(path: 'app/pubspec.yaml', name: 'pubspec.yaml', depth: 1),
        FileEntry(path: 'crates', name: 'crates', isDir: true),
        FileEntry(path: 'README.md', name: 'README.md'),
      ],
);

Future<Harness> showFiles(
  WidgetTester tester, {
  ViewNode? layout,
  String focused = '',
  List<Explorer> explorers = const [],
  Workspace? workspace,
}) async {
  final harness = Harness(
    workspace ?? snapshot(layout: layout, focused: focused),
  );
  for (final explorer in explorers) {
    harness.backend.explorer.apply(explorer);
  }
  await harness.show(tester);
  await settle(tester);
  return harness;
}

Finder inFiles(String text) =>
    find.descendant(of: find.byType(FilesPane), matching: find.text(text));

Finder filesButtons() => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == 'Show files',
);

/// Hovers the sidebar row reading [text], so its buttons show.
Future<TestGesture> hoverRow(WidgetTester tester, String text) async {
  FocusManager.instance.highlightStrategy =
      FocusHighlightStrategy.alwaysTraditional;
  addTearDown(
    () => FocusManager.instance.highlightStrategy =
        FocusHighlightStrategy.automatic,
  );
  final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
  await mouse.addPointer(location: tester.getCenter(find.text(text).first));
  addTearDown(mouse.removePointer);
  await tester.pump();
  return mouse;
}

List<ClientMessage> layoutsSent(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasSetLayout()) message,
];

void main() {
  testWidgets('focusing a project opens no files by itself', (tester) async {
    await showFiles(tester, focused: 'p1', explorers: [tree()]);
    expect(find.byType(FilesPane), findsNothing);
    expect(find.text('README.md'), findsNothing);
    expect(find.text('Show hidden files'), findsNothing);
  });

  testWidgets("a project row's Files button opens its Files beside the "
      'conversation, and glows it when open', (tester) async {
    final harness = await showFiles(tester);
    expect(filesButtons(), findsNothing, reason: 'shown on hover');
    await hoverRow(tester, 'Site');
    await tester.tap(filesButtons());
    await settle(tester);
    final sent = harness.last.setLayout;
    expect(sent.agentId, 'a1', reason: "in the selected agent's layout");
    final split = sent.root.split;
    expect(split.children.first.leaf.kind, 'chat');
    expect(
      split.children.last.leaf,
      ViewLeaf(kind: 'files', path: 'p2', title: 'Site'),
    );
    expect(split.weights, [closeTo(3 / 4, 1e-9), closeTo(1 / 4, 1e-9)]);
    expect(
      harness.backend.sent.where((message) => message.hasFocusProject()),
      isEmpty,
      reason: 'the button only opens the Files',
    );

    // Open already: its pane glows instead, and the button stays tinted.
    await harness.apply(tester, snapshot(layout: withFilesOf('p2')));
    await settle(tester);
    expect(filesButtons(), findsOneWidget, reason: 'shown while open');
    await tester.tap(filesButtons());
    await settle(tester);
    expect(layoutsSent(harness), hasLength(1));
    expect(
      harness.backend.highlights.glows.map((glow) => glow.key),
      contains('pane:a1:f1'),
    );
    await tester.pump(const Duration(seconds: 2));
  });

  testWidgets('opened Files and a file beside them fit a small window', (
    tester,
  ) async {
    final project = Project(id: 'p1', name: 'Factory', path: '/Users/me/f');
    var root = withFiles(chatOnly(), project);
    final files = leaves(root).last.id;
    root = open(root, '/Users/me/f/README.md', beside: files).root;
    final harness = Harness(snapshot(layout: root));
    harness.backend.explorer.apply(tree());
    await harness.show(tester, size: const Size(1000, 700));
    await settle(tester);
    expect(tester.takeException(), isNull, reason: 'nothing overflows');
    expect(find.byType(FilesPane), findsOneWidget);
    expect(
      find.bySemanticsLabel('Show hidden files'),
      findsOneWidget,
      reason: 'a narrow pane keeps just the check',
    );
  });

  testWidgets("the focused project's heading has the Files button too", (
    tester,
  ) async {
    final harness = await showFiles(tester, focused: 'p1');
    await hoverRow(tester, 'Factory');
    await tester.tap(filesButtons());
    await settle(tester);
    expect(harness.last.setLayout.root.split.children.last.leaf.path, 'p1');
  });

  testWidgets('a Files pane shows its project, unfocused, and opens folders '
      'like a tree', (tester) async {
    final harness = await showFiles(
      tester,
      layout: withFilesOf('p1'),
      explorers: [
        tree(),
        tree(projectId: 'p2', entries: []),
      ],
    );
    expect(find.text('Files'), findsOneWidget, reason: "the pane's title");
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('pane-f1')),
        matching: find.text('Factory'),
      ),
      findsOneWidget,
      reason: "the project's name in the header",
    );
    final lib = tester.getTopLeft(find.text('lib'));
    final app = tester.getTopLeft(find.text('app'));
    expect(lib.dx, greaterThan(app.dx), reason: 'children are indented');

    await tester.tap(find.text('crates'));
    await settle(tester);
    expect(
      harness.last.expandFolder,
      ExpandFolder(projectId: 'p1', path: 'crates', expanded: true),
    );
    await tester.tap(find.text('app'));
    await settle(tester);
    expect(
      harness.last.expandFolder,
      ExpandFolder(projectId: 'p1', path: 'app', expanded: false),
    );

    await tester.tap(find.text('Show hidden files'));
    await settle(tester);
    expect(
      harness.last.showAllFiles,
      ShowAllFiles(projectId: 'p1', showAll: true),
    );

    // Hidden rows are dimmed.
    harness.backend.explorer.apply(
      tree(
        showAll: true,
        entries: [
          FileEntry(path: 'target', name: 'target', isDir: true, hidden: true),
        ],
      ),
    );
    await settle(tester);
    final opacity = tester.widget<Opacity>(
      find
          .ancestor(of: find.text('target'), matching: find.byType(Opacity))
          .first,
    );
    expect(opacity.opacity, lessThan(1));
  });

  testWidgets('a file opens right of the Files pane, and the next below it', (
    tester,
  ) async {
    final harness = await showFiles(
      tester,
      layout: withFilesOf('p1'),
      explorers: [tree()],
    );
    await tester.tap(find.text('README.md'));
    await settle(tester);
    var root = harness.last.setLayout.root;
    expect(leaves(root).map((leaf) => leaf.leaf.kind), [
      'chat',
      'files',
      'text',
    ]);
    expect(
      leaves(root).last.leaf,
      ViewLeaf(
        kind: 'text',
        path: '/Users/me/factory/README.md',
        title: 'README.md',
      ),
    );
    // Right of it, taking half the conversation's room.
    expect(root.split.children.map((child) => child.id).take(2), [
      'chat',
      'f1',
    ]);
    expect(root.split.weights, [0.5 / 2, 0.5, 0.5 / 2]);

    await harness.apply(tester, snapshot(layout: root));
    await settle(tester);
    await tester.tap(find.text('pubspec.yaml'));
    await settle(tester);
    root = harness.last.setLayout.root;
    expect(leaves(root).map((leaf) => leaf.leaf.title), [
      '',
      '',
      'README.md',
      'pubspec.yaml',
    ]);
    final viewers = root.split.children.last;
    expect(
      viewers.split.axis,
      ViewAxis.VIEW_AXIS_COLUMN,
      reason: 'stacked below the first',
    );
  });

  testWidgets("a Files pane shows each of the project's folders", (
    tester,
  ) async {
    final workspace = snapshot(layout: withFilesOf('p1'));
    workspace.projects[0].folders.addAll([
      ProjectFolder(id: 'research', name: 'Research', path: '/Users/me/notes'),
      ProjectFolder(id: 'gone', name: 'Archive', path: '/Volumes/gone'),
    ]);
    final explorer = tree()
      ..folders.addAll([
        ExplorerRoot(
          folderId: 'research',
          path: '/Users/me/notes',
          entries: [
            FileEntry(path: 'guide', name: 'guide', isDir: true),
            FileEntry(path: 'index.md', name: 'index.md'),
          ],
        ),
        ExplorerRoot(
          folderId: 'gone',
          path: '/Volumes/gone',
          error: "Can't read /Volumes/gone: no such folder",
        ),
      ]);
    final harness = await showFiles(
      tester,
      workspace: workspace,
      explorers: [explorer],
    );
    double top(Finder finder) => tester.getTopLeft(finder).dy;

    // Each under its name and path, after the project's own folder.
    expect(inFiles('Research'), findsOneWidget);
    expect(find.text('~/notes'), findsOneWidget);
    expect(top(find.text('README.md')), lessThan(top(inFiles('Research'))));
    expect(top(inFiles('Research')), lessThan(top(find.text('guide'))));
    expect(top(find.text('index.md')), lessThan(top(inFiles('Archive'))));
    expect(
      find.text("Can't read /Volumes/gone: no such folder"),
      findsOneWidget,
    );

    // Its folders open in it.
    await tester.tap(find.text('guide'));
    await settle(tester);
    expect(
      harness.last.expandFolder,
      ExpandFolder(
        projectId: 'p1',
        path: 'guide',
        expanded: true,
        folderId: 'research',
      ),
    );

    // An empty project folder steps aside for its folders.
    harness.backend.explorer.apply(explorer..entries.clear());
    await settle(tester);
    expect(find.text('Nothing here, apart from hidden files.'), findsNothing);
    expect(inFiles('Research'), findsOneWidget);
  });

  testWidgets("an agent's own checkout has a section of its own", (
    tester,
  ) async {
    final explorer = tree()
      ..folders.add(
        ExplorerRoot(
          agentId: 'a1',
          branch: 'factory/a1',
          path: '/Users/me/.factory/trees/a1/factory',
          entries: [FileEntry(path: 'src', name: 'src', isDir: true)],
        ),
      );
    final harness = await showFiles(
      tester,
      layout: withFilesOf('p1'),
      explorers: [explorer],
    );
    expect(find.text("Builder's checkout"), findsOneWidget);
    expect(
      find.text('factory/a1 · ~/.factory/trees/a1/factory'),
      findsOneWidget,
    );
    expect(
      tester.getTopLeft(find.text('README.md')).dy,
      lessThan(tester.getTopLeft(find.text("Builder's checkout")).dy),
    );

    // Its folders open in the checkout.
    await tester.tap(find.text('src'));
    await settle(tester);
    expect(
      harness.last.expandFolder,
      ExpandFolder(projectId: 'p1', path: 'src', expanded: true, agentId: 'a1'),
    );
  });

  testWidgets('a Files pane says what is true when there are no rows', (
    tester,
  ) async {
    final harness = await showFiles(
      tester,
      layout: withFilesOf('p1'),
      explorers: [tree(projectId: 'p2')],
    );
    expect(
      find.text('Reading the folder…'),
      findsOneWidget,
      reason: "another project's tree isn't this one's",
    );

    harness.backend.explorer.apply(tree(entries: []));
    await settle(tester);
    expect(find.text('Nothing here, apart from hidden files.'), findsOneWidget);
    harness.backend.explorer.apply(tree(entries: [], showAll: true));
    await settle(tester);
    expect(find.text('This folder is empty.'), findsOneWidget);
    harness.backend.explorer.apply(
      tree(entries: [], error: "Can't read /Users/me/factory: gone"),
    );
    await settle(tester);
    expect(find.text("Can't read /Users/me/factory: gone"), findsOneWidget);

    await harness.apply(
      tester,
      snapshot(layout: withFilesOf('p1'))..projects.removeAt(0),
    );
    await settle(tester);
    expect(find.text('This project was deleted.'), findsOneWidget);
  });

  testWidgets("a phone offers a project's Files in its menu, as a page", (
    tester,
  ) async {
    final harness = Harness(snapshot());
    harness.backend.files['/Users/me/site/index.md'] = FileContent(
      path: '/Users/me/site/index.md',
      mime: 'text/plain',
      size: Int64(5),
      data: 'Hello'.codeUnits,
    );
    await harness.show(tester, size: const Size(390, 844));
    await pumpFrames(tester);
    // A touch and hold on the project, in the drawer.
    tester
        .state<PhoneDrawerHostState>(find.byType(PhoneDrawerHost))
        .openDrawer();
    await pumpFrames(tester);
    await tester.longPress(find.text('Site'));
    await pumpFrames(tester);
    expect(find.text('Project settings'), findsOneWidget);
    await tester.tap(find.text('Files'));
    await pumpFrames(tester);
    expect(harness.last.watchFiles, WatchFiles(projectId: 'p2'));
    expect(find.text('Reading the folder…'), findsOneWidget);
    expect(layoutsSent(harness), isEmpty, reason: 'a page, not a pane');

    harness.backend.explorer.apply(
      tree(
        projectId: 'p2',
        entries: [FileEntry(path: 'index.md', name: 'index.md')],
      ),
    );
    await pumpFrames(tester);
    await tester.tap(find.text('index.md'));
    await pumpFrames(tester);
    expect(find.text('Hello'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(find.text('Hello'), findsNothing);
    expect(find.text('index.md'), findsOneWidget, reason: 'back to the Files');

    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(harness.last.watchFiles.projectId, '');
    expect(find.text('index.md'), findsNothing);
  });

  testWidgets("a phone leaves a Mac's Files pane out of the agent's list", (
    tester,
  ) async {
    final harness = Harness(snapshot(layout: withFilesOf('p1')));
    await harness.show(tester, size: const Size(390, 844));
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel('Extensions'));
    await pumpFrames(tester);
    expect(find.text('Files'), findsNothing);
  });

  group('layout', () {
    test(
      'withFiles puts the Files right of the conversation, a quarter of it',
      () {
        final project = Project(id: 'p1', name: 'Factory');
        final root = withFiles(chatOnly(), project);
        expect(root.split.children.map((child) => child.leaf.kind), [
          'chat',
          'files',
        ]);
        expect(filesPane(root, 'p1'), isNotNull);
        expect(filesPane(root, 'p2'), isNull);
      },
    );

    test(
      'a file opened beside Files with only a page open goes right of it',
      () {
        final root = withPage(withFilesOf('p1'), AppPage.settings);
        final opened = open(root, '/w/a.md', beside: 'f1');
        final kinds = leaves(opened.root)
            .map((leaf) => leaf.leaf.kind)
            .toList();
        expect(kinds.indexOf('text'), kinds.indexOf('files') + 1);
      },
    );

    test("a file opened elsewhere isn't confused with a Files pane", () {
      final root = withFilesOf('/w/a.md');
      final opened = open(root, '/w/a.md');
      expect(opened.paneId, isNot('f1'));
    });
  });
}
