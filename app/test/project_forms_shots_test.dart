// Area 5b's forms drawn as they look, dark, for Lucy to check (her
// projects-sidebar.md §5.2, §6, §7; Trish's words §12): New project's
// folder line here and on Mac Studio; Project settings' Folder row with
// Show in Finder and a long path cut in the middle; the Delete sheet with
// 1, 3 and 5 pipes; Archive's note with pipes and pins; Agent settings'
// Project note in its three kinds; and Delete and Project settings on an
// iPhone. With SHOTS set, each is drawn to a PNG there:
//   SHOTS=/tmp/5b flutter test test/project_forms_shots_test.dart
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

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;
import 'rivet_test.dart' show admin;

const _owners = ['Fred', 'Ana', 'Kenji', 'Lena', 'Omar'];

final _bakery = Project(
  id: 'p1',
  name: 'Bakery',
  path:
      '/Users/me/Documents/Clients/Rosalind Bakery and Cafe/Website redesign '
      '2026/project files',
  folders: [
    ProjectFolder(id: 'site', name: 'Site', path: '/Users/me/code/site'),
  ],
);

Workspace _workspace({int pipes = 0, bool pins = false}) => Workspace(
  projects: [
    _bakery,
    Project(id: 'p2', name: 'Research', path: '/Users/me/research'),
  ],
  agents: [
    admin()..projectId = 'p1',
    for (final (id, name) in [('a1', 'June'), ('a2', 'Bo'), ('a3', 'Mia')])
      Agent(id: id, name: name, projectId: 'p1', modelAvailable: true),
  ],
  selectedAgentId: 'a1',
  settings: Settings(pinnedAgents: pins ? ['a1', 'a2'] : const []),
  pipes: [
    for (final owner in _owners.take(pipes))
      Pipe(
        id: owner,
        name: owner,
        projectId: 'p1',
        peer: PipePeer(ownerName: owner),
      ),
  ],
);

Future<void> _shot(WidgetTester tester, String name) async {
  expect(tester.takeException(), isNull);
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

const _mac = Size(1100, 900);
const _iphone = Size(393, 852);

void main() {
  setUpAll(loadGeist);
  setUp(() {
    final dispatcher = TestWidgetsFlutterBinding.instance.platformDispatcher;
    dispatcher.platformBrightnessTestValue = Brightness.dark;
    addTearDown(dispatcher.clearPlatformBrightnessTestValue);
    WidgetsApp.debugAllowBannerOverride = false;
    addTearDown(() => WidgetsApp.debugAllowBannerOverride = true);
  });

  /// The app at [size], with [form] open over it as a sheet.
  Future<Harness> open(
    WidgetTester tester,
    Workspace workspace,
    Widget Function(VoidCallback close) form, {
    Size size = _mac,
    bool local = true,
  }) async {
    debugDefaultTargetPlatformOverride = size == _iphone
        ? TargetPlatform.iOS
        : TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace, localBackend: local);
    await harness.show(tester, size: size);
    await pumpFrames(tester);
    showModal(
      tester.element(find.text('June').first),
      (context, close) => form(close),
    );
    await pumpFrames(tester);
    return harness;
  }

  for (final (local, where) in [(true, 'here'), (false, 'mac-studio')]) {
    testWidgets('New project names its folder, $where', (tester) async {
      await open(
        tester,
        _workspace()..remote = RemoteAccess(machine: 'Mac Studio'),
        (close) => CreateProjectForm(close: close),
        local: local,
      );
      await tester.enterText(
        find.descendant(
          of: find.byType(CreateProjectForm),
          matching: find.byType(EditableText),
        ),
        'Bakery website',
      );
      await pumpFrames(tester);
      expect(
        find.text(
          local
              ? 'Folder: ~/.factory/workspace/bakery-website'
              : 'Folder on Mac Studio: ~/.factory/workspace/bakery-website',
        ),
        findsOneWidget,
      );
      await _shot(tester, 'new-project-$where');
    });
  }

  for (final (size, where) in [(_mac, 'mac'), (_iphone, 'iphone')]) {
    testWidgets('Project settings starts with its folder, on the $where', (
      tester,
    ) async {
      await open(
        tester,
        _workspace(),
        (close) => ProjectSettingsForm(project: _bakery, close: close),
        size: size,
      );
      expect(find.textContaining('…'), findsWidgets);
      await _shot(tester, 'project-settings-$where');
    });
  }

  testWidgets("Archive's note with pipes and pins", (tester) async {
    await open(
      tester,
      _workspace(pipes: 2, pins: true),
      (close) => ProjectSettingsForm(project: _bakery, close: close),
    );
    await tester.ensureVisible(find.text('Archive project'));
    await pumpFrames(tester);
    expect(
      find.textContaining('until you unarchive it. June and Bo are unpinned.'),
      findsOneWidget,
    );
    await _shot(tester, 'archive-note');
  });

  for (final (pipes, size, where) in [
    (1, _mac, 'mac'),
    (3, _mac, 'mac'),
    (5, _mac, 'mac'),
    (3, _iphone, 'iphone'),
  ]) {
    testWidgets('Delete with $pipes pipes, on the $where', (tester) async {
      await open(
        tester,
        _workspace(pipes: pipes),
        (close) => ProjectSettingsForm(project: _bakery, close: close),
        size: size,
      );
      await tester.ensureVisible(find.text('Delete project…'));
      await pumpFrames(tester);
      await tester.tap(find.text('Delete project…'));
      await pumpFrames(tester);
      expect(find.text('Delete Bakery?'), findsOneWidget);
      await _shot(tester, 'delete-$pipes-pipes-$where');
    });
  }

  for (final (who, to, note, name) in [
    (
      'June',
      'Research',
      "June will work in Research's folder and can message only agents in "
          'Research.',
      'project',
    ),
    (
      'June',
      'Workspace (no project)',
      'June will work in the workspace folder and can message only Rivet '
          'and other agents outside projects.',
      'workspace',
    ),
    ('Rivet', 'Research', "Rivet will work in Research's folder.", 'admin'),
  ]) {
    testWidgets("Agent settings' Project note: $name", (tester) async {
      final workspace = _workspace();
      await open(
        tester,
        workspace,
        (close) => AgentSettingsForm(
          agent: workspace.agents.firstWhere((agent) => agent.name == who),
          close: close,
        ),
      );
      await tester.ensureVisible(find.text('Bakery').last);
      await pumpFrames(tester);
      await tester.tap(find.text('Bakery').last);
      await pumpFrames(tester);
      await tester.tap(find.text(to).last);
      await pumpFrames(tester);
      expect(find.text(note), findsOneWidget);
      await _shot(tester, 'agent-project-note-$name');
    });
  }
}
