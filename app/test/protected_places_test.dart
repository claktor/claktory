// Places macOS protects (Lucy's permissions.md P2, Cioccolata's item 12): on
// the Mac that runs the Claktory, projects already working in one never
// checked get the sheet once ("Let your agents work in Documents"), and its
// Continue has the backend read them, so macOS asks then; refused, it says
// what that means and opens System Settings at Files and Folders.
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/forms.dart' show FormFrame;
import 'package:factory_app/protected_places.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _documents = '/Users/ana/Documents';

Workspace _workspace({
  ProtectedPlaceState documents =
      ProtectedPlaceState.PROTECTED_PLACE_STATE_UNSPECIFIED,
}) => Workspace(
  projects: [
    Project(id: 'p1', name: 'nixos-site', path: '$_documents/nixos-site'),
  ],
  protectedPlaces: [
    ProtectedPlace(
      root: _documents,
      name: 'Documents',
      state: documents,
      usedBy: ['nixos-site'],
      checkedMs:
          documents == ProtectedPlaceState.PROTECTED_PLACE_STATE_UNSPECIFIED
          ? Int64.ZERO
          : Int64(1000),
    ),
    // Refused an agent once, used by nothing now: not asked about.
    ProtectedPlace(root: '/Volumes/Backup', name: 'Backup'),
  ],
);

Finder _button(String label) => find.widgetWithText(Button, label);

/// Every place macOS protects on Ana's Mac, none used yet, and a project
/// with a folder to move, in a Claktory without Rivet, so its settings add
/// folders themselves.
Workspace _roots({
  ProtectedPlaceState documents =
      ProtectedPlaceState.PROTECTED_PLACE_STATE_UNSPECIFIED,
}) => Workspace(
  projects: [
    Project(
      id: 'p1',
      name: 'Site',
      path: '/Users/ana/site',
      folders: [
        ProjectFolder(id: 'notes', name: 'Notes', path: '/Users/ana/notes'),
      ],
    ),
  ],
  agents: [
    Agent(id: 'a1', projectId: 'p1', name: 'Builder', modelAvailable: true),
  ],
  selectedAgentId: 'a1',
  focusedProjectId: 'p1',
  protectedRoots: [
    ProtectedRoot(root: _documents, name: 'Documents'),
    ProtectedRoot(root: '/Users/ana/Desktop', name: 'Desktop'),
    ProtectedRoot(root: '/Users/ana/Downloads', name: 'Downloads'),
    ProtectedRoot(
      root: '/Users/ana/Library/Mobile Documents',
      name: 'iCloud Drive',
    ),
    ProtectedRoot(root: '/Volumes/Backup', name: 'Backup'),
  ],
  protectedPlaces: [
    if (documents != ProtectedPlaceState.PROTECTED_PLACE_STATE_UNSPECIFIED)
      ProtectedPlace(
        root: _documents,
        name: 'Documents',
        state: documents,
        usedBy: ['Docs'],
        checkedMs: Int64(1000),
      ),
  ],
);

/// Opens Site's settings, then its form for a new folder, named Docs.
Future<void> _addFolder(WidgetTester tester) async {
  await tester.tap(find.text('Site').first);
  await pumpFrames(tester);
  await tester.tap(find.text('Project settings…'));
  await pumpFrames(tester);
  await tester.ensureVisible(find.text('Add a folder…'));
  await pumpFrames(tester);
  await tester.tap(find.text('Add a folder…'));
  await pumpFrames(tester);
  expect(find.text('New folder in Site'), findsOneWidget);
  await tester.enterText(
    find
        .descendant(
          of: find.byType(FormFrame),
          matching: find.byType(EditableText),
        )
        .first,
    'Docs',
  );
  await tester.pump();
}

/// Types [path] into the new folder's path.
Future<void> _typePath(WidgetTester tester, String path) async {
  await tester.enterText(find.byType(EditableText).last, path);
  await tester.pump();
}

const _asks =
    'macOS asks before Claktory Agents can use your Documents folder.';

void main() {
  void onMac() {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
  }

  test('the words: one place, or several, and where they are used', () {
    const one = ProtectedPlacePermission(
      names: ['Documents'],
      usedBy: ['nixos-site'],
    );
    final context = _NoContext();
    final words = one.words(context);
    expect(words.title, 'Let your agents work in Documents');
    expect(
      words.body,
      'macOS keeps Documents private until you let an app in. Your agents '
      'need it to work in nixos-site. Your Mac asks next, for Claktory '
      'Agents.',
    );
    expect(words.offTitle, "Your agents can't work in Documents");
    expect(
      words.offBody,
      "macOS hasn't let Claktory Agents into Documents, so commands in "
      'nixos-site fail.',
    );
    const two = ProtectedPlacePermission(
      names: ['Documents', 'Desktop'],
      usedBy: ['nixos-site', 'blog'],
    );
    expect(
      two.words(context).title,
      'Let your agents work in Documents and Desktop',
    );
    expect(
      two.words(context).body,
      contains('need them to work in nixos-site and blog.'),
    );
  });

  testWidgets('on the Mac that runs it: the sheet once for a place never '
      'checked; Continue has the backend read it; refused, Open System '
      'Settings goes to Files and Folders', (tester) async {
    onMac();
    final opened = <Uri>[];
    openSystemSettings = (url) async {
      opened.add(url);
      return true;
    };
    addTearDown(() => openSystemSettings = (_) async => false);
    final harness = Harness(_workspace());
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Let your agents work in Documents'), findsOneWidget);
    expect(find.textContaining('Backup'), findsNothing);

    await tester.tap(_button('Continue'));
    await pumpFrames(tester);
    expect(harness.last.checkProtectedPlaces.roots, [_documents]);
    // macOS said no: the backend found it refused.
    await harness.apply(
      tester,
      _workspace(documents: ProtectedPlaceState.PROTECTED_PLACE_STATE_REFUSED),
    );
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text("Your agents can't work in Documents"), findsOneWidget);
    await tester.tap(_button('Open System Settings'));
    await pumpFrames(tester);
    expect(opened.single.toString(), contains('Privacy_FilesAndFolders'));
    await tester.tap(find.text('Not now'));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text("Your agents can't work in Documents"), findsNothing);

    // Checked now: not asked again.
    await harness.apply(tester, _workspace());
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Let your agents work in Documents'), findsNothing);
  });

  testWidgets('let in: the sheet closes by itself', (tester) async {
    onMac();
    final harness = Harness(_workspace());
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester, const Duration(seconds: 1));
    await tester.tap(_button('Continue'));
    await pumpFrames(tester);
    await harness.apply(
      tester,
      _workspace(documents: ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED),
    );
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Let your agents work in Documents'), findsNothing);
    expect(find.text("Your agents can't work in Documents"), findsNothing);
  });

  testWidgets('anywhere but a Mac, nothing asks', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Let your agents work in Documents'), findsNothing);
  });

  test('a path is in a place by whole components', () {
    final workspace = _roots();
    String? place(String path) => protectedRootOf(workspace, path)?.name;
    expect(place(_documents), 'Documents');
    expect(place('$_documents/'), 'Documents');
    expect(place('$_documents/blog/drafts'), 'Documents');
    expect(place('${_documents}2/blog'), isNull);
    expect(place('/Users/ana'), isNull);
    expect(place('/Volumes/Backup/site'), 'Backup');
    expect(place(''), isNull);
    expect(protectedRootOf(Workspace(), '$_documents/blog'), isNull);
  });

  testWidgets("a folder's path in a place macOS protects says it asks first, "
      'or how to turn it on once refused', (tester) async {
    final harness = Harness(_roots());
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester);
    await _addFolder(tester);
    await _typePath(tester, '/Users/ana/site/docs');
    expect(find.textContaining('macOS'), findsNothing);
    await _typePath(tester, '$_documents/blog');
    expect(find.text(_asks), findsOneWidget);
    await _typePath(tester, '/Users/ana/Library/Mobile Documents/blog');
    expect(
      find.text('macOS asks before Claktory Agents can use your iCloud Drive.'),
      findsOneWidget,
    );
    await _typePath(tester, '/Volumes/Backup/blog');
    expect(
      find.text('macOS asks before Claktory Agents can use the Backup disk.'),
      findsOneWidget,
    );

    await _typePath(tester, '$_documents/blog');
    await harness.apply(
      tester,
      _roots(documents: ProtectedPlaceState.PROTECTED_PLACE_STATE_REFUSED),
    );
    await pumpFrames(tester);
    expect(
      find.text(
        "macOS hasn't let Claktory Agents into your Documents folder. To "
        'turn it on: System Settings → Privacy & Security → Files and '
        'Folders → Claktory Agents.',
      ),
      findsOneWidget,
    );
    await harness.apply(
      tester,
      _roots(documents: ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED),
    );
    await pumpFrames(tester);
    expect(find.textContaining('macOS'), findsNothing);
  });

  testWidgets('on the Mac that runs it, saving a folder into a place never '
      'checked shows the sheet first; Continue saves and reads it', (
    tester,
  ) async {
    onMac();
    final harness = Harness(_roots());
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester);
    await _addFolder(tester);
    await _typePath(tester, '$_documents/blog');
    await tester.tap(find.text('Add folder'));
    await pumpFrames(tester);
    expect(find.text('Let your agents work in Documents'), findsOneWidget);
    expect(
      find.textContaining('Your agents need it to work in Docs.'),
      findsOneWidget,
    );
    expect(harness.backend.sent.where((m) => m.hasCreateFolder()), isEmpty);

    await tester.tap(_button('Continue'));
    await pumpFrames(tester);
    // Saved, then read for what macOS answered.
    final sent = harness.backend.sent;
    expect(
      sent[sent.length - 2].createFolder,
      CreateFolder(projectId: 'p1', name: 'Docs', path: '$_documents/blog'),
    );
    expect(sent.last.checkProtectedPlaces.roots, [_documents]);
    await harness.apply(
      tester,
      _roots(documents: ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED),
    );
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Let your agents work in Documents'), findsNothing);
    expect(find.text('New folder in Site'), findsNothing, reason: 'saved');
  });

  testWidgets('Not now at that sheet saves nothing and keeps the form', (
    tester,
  ) async {
    onMac();
    final harness = Harness(_roots());
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester);
    await tester.tap(find.text('Site').first);
    await pumpFrames(tester);
    await tester.tap(find.text('Project settings…'));
    await pumpFrames(tester);
    // Moving a folder into Documents asks the same way.
    await tester.enterText(
      find.byWidgetPredicate(
        (widget) =>
            widget is EditableText &&
            widget.controller.text == '/Users/ana/notes',
      ),
      '$_documents/notes',
    );
    await tester.pump();
    expect(find.text(_asks), findsOneWidget);
    await tester.tap(find.text('Save'));
    await pumpFrames(tester);
    expect(find.text('Let your agents work in Documents'), findsOneWidget);
    expect(
      find.textContaining('Your agents need it to work in Notes.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Not now'));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Let your agents work in Documents'), findsNothing);
    expect(find.text('Project settings'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasUpdateFolder()), isEmpty);

    // Saving again asks again.
    await tester.tap(find.text('Save'));
    await pumpFrames(tester);
    expect(find.text('Let your agents work in Documents'), findsOneWidget);
  });

  testWidgets('anywhere but a Mac, a folder in Documents saves at once', (
    tester,
  ) async {
    final harness = Harness(_roots());
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester);
    await _addFolder(tester);
    await _typePath(tester, '$_documents/blog');
    expect(find.text(_asks), findsOneWidget);
    await tester.tap(find.text('Add folder'));
    await pumpFrames(tester);
    expect(find.text('Let your agents work in Documents'), findsNothing);
    expect(harness.last.createFolder.path, '$_documents/blog');
  });
}

/// Words need no context of their own.
class _NoContext extends Fake implements BuildContext {}
