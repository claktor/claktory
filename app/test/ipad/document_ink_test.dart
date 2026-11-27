// Keeping what's drawn on a PDF on an iPad (ipad/document_host.dart with
// InkLayerSync): every change saved as its page's ink layer, what's
// kept loaded when the PDF opens, and the line under the page saying where
// the drawing is, with "Keep both" when another device
// saved the page first.
import 'dart:async';

import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/claktory_identity.dart';
import 'package:factory_app/ipad/workbench.dart' show WorkbenchScope;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart'
    hide InkComposite;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart' show Harness;
import '../panes_test.dart' show leaf, snapshot;
import 'as_ipad.dart';
import 'document_native.dart';

List<Map<String, Object?>> _sent(DocumentNative native, String method) => [
  for (final call in native.calls)
    if (call.method == method) (call.arguments as Map).cast<String, Object?>(),
];

String _status(WidgetTester tester) {
  final line = find.descendant(
    of: find.byKey(const ValueKey('document-status')),
    matching: find.byType(Text),
  );
  return tester.widget<Text>(line.first).data!;
}

Future<(DocumentNative, Harness)> _open(
  WidgetTester tester, {
  bool asking = false,
  void Function(DocumentNative native)? before,
}) async {
  asIPad();
  final native = DocumentNative();
  before?.call(native);
  final harness = await showDocument(tester, asking: asking);
  await native.ready();
  await settle(tester);
  return (native, harness);
}

void main() {
  testWidgets('a drawing is saved as its page\'s layer, saying so while it '
      'saves and where once it has', (tester) async {
    final (native, harness) = await _open(tester);
    expect(find.byKey(const ValueKey('document-status')), findsNothing);

    native.ink.hold = Completer.sync();
    await native.drew(2, 3);
    await settle(tester);
    expect(_status(tester), 'Saving your drawing…');
    await native.drew(4, 1);
    await settle(tester);
    expect(_status(tester), 'Saving your drawings…');

    native.ink.release();
    await settle(tester);
    expect(_status(tester), 'Saved to ${claktoryName(harness.backend)}');
    final saves = native.ink.saves;
    expect([for (final s in saves) s.space.page1based], [2, 4]);
    expect(saves.first.path, documentPath);
    expect(saves.first.sourceSha256, documentPdf().sha256);
    expect((saves.first.inkLayerId, saves.first.baseRevision), ('', 0));
    expect(
      (saves.first.drawingSha256, saves.first.previewSha256),
      (drawingSha, previewSha),
    );
    // Native learns each page's layer.
    expect(
      [
        for (final told in _sent(native, 'layerSaved'))
          (told['page_1based'], told['revision']),
      ],
      [(2, 1), (4, 1)],
    );

    // The next change is the layer's next revision.
    await native.drew(2, 4);
    await settle(tester);
    expect((saves.last.inkLayerId, saves.last.baseRevision), ('layer-1', 1));
  });

  testWidgets('what the backend keeps of this version loads as the PDF opens, '
      'and goes with an answer', (tester) async {
    final (native, _) = await _open(
      tester,
      asking: true,
      before: (native) {
        native.ink.otherDevice(3, source: documentPdf().sha256);
        // An earlier version's: not shown.
        native.ink.otherDevice(5, source: 'an-earlier-version');
      },
    );
    final loaded = _sent(native, 'loadDrawing').single['layers'] as List;
    expect(
      [for (final l in loaded) ((l as Map)['page_1based'], l['ink_layer_id'])],
      [(3, 'theirs-3')],
    );
    expect(find.text('Your drawing goes with it.'), findsOneWidget);
    expect(
      _status(tester),
      'This PDF changed. Your drawing on page 5 is from the earlier version.',
    );
  });

  testWidgets('a page another device saved first isn\'t overwritten; Keep '
      'both puts theirs under the person\'s and saves both', (tester) async {
    final (native, harness) = await _open(tester);
    // Saved elsewhere after this PDF opened.
    native.ink.otherDevice(2, source: documentPdf().sha256);
    await native.drew(2, 3);
    await settle(tester);
    expect(
      _status(tester),
      'Page 2 was saved on another device first. Your drawing on it isn\'t '
      'saved yet.',
    );
    expect(native.ink.layers['theirs-2']!.revision, 1);

    await tester.tap(find.byKey(const ValueKey('document-keep-both')));
    await settle(tester);
    final keep = _sent(native, 'loadDrawing').last['layers'] as List;
    expect((keep.single as Map)['keep_mine'], isTrue);
    final both = native.ink.saves.last;
    expect((both.inkLayerId, both.baseRevision), ('theirs-2', 1));
    expect(both.conflictOf, isEmpty);
    expect(native.ink.layers['theirs-2']!.revision, 2);
    expect(_status(tester), 'Saved to ${claktoryName(harness.backend)}');
    expect(find.byKey(const ValueKey('document-keep-both')), findsNothing);
  });

  testWidgets('two pages saved elsewhere first are said together', (
    tester,
  ) async {
    final (native, _) = await _open(tester);
    native.ink
      ..otherDevice(3, source: documentPdf().sha256)
      ..otherDevice(5, source: documentPdf().sha256);
    await native.drew(3, 1);
    await native.drew(5, 1);
    await settle(tester);
    expect(
      _status(tester),
      'Pages 3 and 5 were saved on another device first. Your drawings on '
      'them aren\'t saved yet.',
    );
  });

  testWidgets('Keep both fits a narrow window at the largest text', (
    tester,
  ) async {
    asIPad();
    tester.platformDispatcher.textScaleFactorTestValue = 3;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final native = DocumentNative();
    await showDocument(
      tester,
      asking: false,
      size: const Size(390, 844),
      phoneRoutes: true,
    );
    await native.ready();
    await settle(tester);
    native.ink.otherDevice(2, source: documentPdf().sha256);
    await native.drew(2, 1);
    await settle(tester);
    expect(tester.takeException(), isNull);
    final keep = find.byKey(const ValueKey('document-keep-both'));
    await tester.ensureVisible(keep);
    await settle(tester);
    expect(tester.getSize(keep).height, greaterThanOrEqualTo(44));
    await tester.tap(keep);
    await settle(tester);
    final both = native.ink.saves.last;
    expect((both.inkLayerId, both.baseRevision), ('theirs-2', 1));
    expect(both.conflictOf, isEmpty);
  });

  testWidgets('a drawing not kept says why', (tester) async {
    final (native, harness) = await _open(tester);
    final name = claktoryName(harness.backend);

    native.ink.failNextSave = const RequestFailure(
      'This file changed since you opened it.',
      code: ErrorCode.ERROR_CODE_INVALID_REQUEST,
    );
    await native.drew(1, 1);
    await settle(tester);
    expect(
      _status(tester),
      'Not saved. This file changed since you opened it.',
    );

    native.ink.failNextSave = const RequestFailure('Something broke.');
    await native.drew(1, 2);
    await settle(tester);
    expect(
      _status(tester),
      'Couldn\'t save your drawing. Claktory tries again the next time you '
      'draw.',
    );

    native.ink
      ..failNextSave = const RequestFailure("Can't reach your Claktory right now.")
      ..connected.value = false;
    await native.drew(1, 3);
    await settle(tester);
    expect(
      _status(tester),
      'Not connected to $name. Your drawing is kept on this iPad and saves '
      'the next time this PDF is open and $name is back.',
    );

    // Back: saved without drawing again.
    native.ink.connected.value = true;
    await settle(tester);
    expect(_status(tester), 'Saved to $name');
  });

  testWidgets('a Claktory from before kept drawings says to update it', (
    tester,
  ) async {
    final (native, _) = await _open(
      tester,
      before: (native) => native.ink.old = true,
    );
    expect(find.byKey(const ValueKey('document-status')), findsNothing);
    await native.drew(1, 1);
    await settle(tester);
    expect(
      _status(tester),
      'Not saved. To keep drawings, update Claktory on the computer it runs '
      'on.',
    );
  });

  testWidgets('ink on a page that stayed the same is saved onto the new '
      'version', (tester) async {
    final (native, harness) = await _open(tester);
    await native.drew(2, 1);
    await settle(tester);
    native.reloadWith = (arguments) => {
      'reloaded': true,
      'source_sha256': (arguments['source'] as Map)['sha256'],
      'page_count': 5,
      'page_1based': 2,
      'kept_layers': [
        {'page_1based': 2, 'ink_layer_id': 'layer-1', 'revision': 1},
      ],
      'outdated_layers': <Object?>[],
    };
    final file = documentPdf('a paper on curlews, revised');
    harness.backend
      ..files[documentPath] = file
      ..fileChanged.value = FileChanged(
        path: documentPath,
        sha256: file.sha256,
      );
    await settle(tester);

    final carried = native.ink.saves.last;
    expect(carried.space.page1based, 2);
    expect(carried.sourceSha256, file.sha256);
    expect((carried.inkLayerId, carried.baseRevision), ('', 0));
    expect(_status(tester), 'This PDF changed. Showing the new version.');

    // A view made again (the pane hidden and shown) opens the new version.
    final scope = tester.widget<WorkbenchScope>(find.byType(WorkbenchScope));
    final surface = scope.notifier!.documents.of(
      'a1',
      leaf('paper', 'pdf', documentPath),
    );
    expect(surface.params!.source.sha256, file.sha256);
    expect(surface.params!.document.sha256, file.sha256);
  });

  testWidgets('a pane closed while its drawing saves still saves it', (
    tester,
  ) async {
    final (native, harness) = await _open(tester);
    native.ink.hold = Completer.sync();
    await native.drew(3, 2);
    await settle(tester);
    expect(native.ink.saves, isEmpty);

    // The paper leaves the layout.
    harness.backend.workspace.apply(snapshot(layout: leaf('chat', 'chat')));
    await settle(tester);
    expect(find.byKey(const ValueKey('document-status')), findsNothing);

    native.ink.release();
    await settle(tester);
    expect([for (final s in native.ink.saves) s.space.page1based], [3]);
  });

  testWidgets('a drawing made offline is kept on this iPad when the PDF '
      'closes, and saves when it opens again with the Claktory back', (
    tester,
  ) async {
    final (native, harness) = await _open(
      tester,
      before: (native) => native.journal = MemoryJournal(),
    );
    final journal = native.journal! as MemoryJournal;
    final name = claktoryName(harness.backend);
    native.ink
      ..failNextSave = const RequestFailure("Can't reach your Claktory right now.")
      ..connected.value = false;
    await native.drew(3, 2);
    await settle(tester);
    expect(
      _status(tester),
      'Not connected to $name. Your drawing is kept on this iPad and saves '
      'the next time this PDF is open and $name is back.',
    );
    expect(native.ink.saves, isEmpty);
    expect([for (final e in journal.kept.values) e.page1based], [3]);

    // The paper leaves the layout while offline, and its last save gives
    // up (as when the app quits): still kept.
    final layout = harness.backend.workspace.workspace.agents
        .firstWhere((agent) => agent.id == 'a1')
        .layout;
    harness.backend.workspace.apply(snapshot(layout: leaf('chat', 'chat')));
    await settle(tester);
    expect(find.byKey(const ValueKey('document-status')), findsNothing);
    await tester.pump(const Duration(seconds: 31));
    expect([for (final e in journal.kept.values) e.page1based], [3]);

    // Back, and the PDF opened again: the drawing is shown and saved.
    native.ink.connected.value = true;
    harness.backend.workspace.apply(snapshot(layout: layout));
    await settle(tester);
    await native.ready();
    await settle(tester);
    expect([for (final s in native.ink.saves) s.space.page1based], contains(3));
    expect(_status(tester), 'Saved to $name');
    expect(journal.kept, isEmpty);
  });
}
