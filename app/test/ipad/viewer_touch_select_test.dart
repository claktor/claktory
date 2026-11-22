// Text in a file's pane is selected with a finger on an iPad, as on an
// iPhone: touch and hold selects a word, and the system's menu offers
// Comment and Suggest, with no second Comment and Suggest floating over it
// (Prosciutto's S6). A trackpad's selection has no menu, so its Comment
// and Suggest still float at the selection.
import 'dart:convert';

import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoAdaptiveTextSelectionToolbar;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show beside, snapshot;
import 'as_ipad.dart';

const _essay = '''# On rivers

The first paragraph says where the river starts, high in the hills.

The second paragraph says where it goes, slowly, past the meadows and the town.
''';

FileContent _file(String path, String body) => FileContent(
  path: path,
  mime: 'text/markdown',
  size: Int64(utf8.encode(body).length),
  data: utf8.encode(body),
  sha256: 'v1',
);

/// The essay beside the conversation in the iPad's workbench, at [size].
Future<Finder> _essayShown(WidgetTester tester, Size size) async {
  asIPad();
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  final harness = Harness(snapshot(layout: beside('text', '/w/essay.md')))
    ..backend.appliesLayouts = true;
  harness.backend.files['/w/essay.md'] = _file('/w/essay.md', _essay);
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(
      focus: focus,
      child: const Shell(rail: SizedBox(width: 56)),
    ),
  );
  await pumpFrames(tester);
  final paragraph = find.textContaining(
    'The second paragraph',
    findRichText: true,
  );
  if (paragraph.evaluate().isEmpty) {
    // One pane at a time: the file's first.
    await tester.tap(find.text('essay.md').first);
    await pumpFrames(tester);
  }
  harness.backend.threads.apply(
    ThreadList(path: '/w/essay.md', versionSha256: 'v1'),
  );
  await pumpFrames(tester);
  expect(paragraph, findsWidgets);
  return paragraph.first;
}

void main() {
  for (final size in [const Size(1366, 1024), const Size(744, 1133)]) {
    testWidgets('${size.width.round()}×${size.height.round()}: touch and '
        'hold selects a word; the menu has Comment and Suggest, and only '
        'it', (tester) async {
      final paragraph = await _essayShown(tester, size);
      await tester.longPressAt(
        tester.getRect(paragraph).topLeft + const Offset(140, 10),
      );
      await pumpFrames(tester);
      final menu = find.byType(CupertinoAdaptiveTextSelectionToolbar);
      expect(menu, findsOneWidget);
      final items = tester
          .widget<CupertinoAdaptiveTextSelectionToolbar>(menu)
          .buttonItems!;
      expect(
        items.map((item) => item.type),
        contains(ContextMenuButtonType.copy),
        reason: 'a word is selected',
      );
      expect(
        [for (final item in items) item.label],
        containsAll(['Comment', 'Suggest']),
      );
      // The menu's own, and no second pair floating at the selection.
      expect(find.text('Comment'), findsOneWidget);
      expect(find.text('Suggest'), findsOneWidget);
    });
  }

  testWidgets("a trackpad's selection has no menu: Comment and Suggest float "
      'at it', (tester) async {
    final paragraph = await _essayShown(tester, const Size(1366, 1024));
    final from = tester.getRect(paragraph).topLeft + const Offset(40, 10);
    final drag = await tester.startGesture(
      from,
      kind: PointerDeviceKind.mouse,
      buttons: kPrimaryButton,
    );
    for (var x = 20.0; x <= 200; x += 20) {
      await drag.moveTo(from + Offset(x, 0));
      await tester.pump(const Duration(milliseconds: 16));
    }
    await drag.up();
    await pumpFrames(tester);
    expect(find.byType(CupertinoAdaptiveTextSelectionToolbar), findsNothing);
    expect(find.text('Comment'), findsOneWidget);
    expect(find.text('Suggest'), findsOneWidget);
  });
}
