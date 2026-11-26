// A project's Files pane across the seam (#540, Sale's row 158 find): an
// iPad window narrowed to the drawer layout with Files in front opens on the
// phone's Files page for that project, not on a viewer that can't show it;
// Back goes to the conversation and leaves the pane in the agent's layout.
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/pane_chrome.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show leaf, layoutsSent, row, snapshot, split;
import 'as_ipad.dart';

Widget _window(WorkbenchFocus focus) => WorkbenchScope(
  focus: focus,
  child: const Shell(rail: SizedBox(width: 56)),
);

final _layout = split('s1', row, [
  leaf('chat', 'chat'),
  leaf('f1', 'files', 'p1'),
]);

const _wide = Size(1194, 834);

/// The phone's viewer page in front, if any.
final _viewers = find.byWidgetPredicate(
  (widget) => widget is PaneChrome && widget.phone != null,
);

const _cantShow = "Claktory can't show this kind of file here yet.";

void main() {
  for (final narrow in const [Size(504, 834), Size(320, 834)]) {
    testWidgets('narrowed to $narrow with Files focused, the window opens on '
        "the project's Files page; Back goes to the conversation and the "
        'pane stays', (tester) async {
      asIPad();
      final focus = WorkbenchFocus();
      addTearDown(focus.dispose);
      final harness = Harness(snapshot(layout: _layout));
      await harness.show(tester, size: _wide, home: _window(focus));
      await pumpFrames(tester);
      focus.focus('a1', 'f1');
      await pumpFrames(tester);
      expect(find.byType(Workbench), findsOneWidget);
      harness.backend.sent.clear();

      await harness.show(tester, size: narrow, home: _window(focus));
      await pumpFrames(tester);
      expect(find.byType(PhoneShell), findsOneWidget);
      expect(find.textContaining(_cantShow), findsNothing);
      expect(_viewers, findsNothing);
      // The phone's Files page, reading the project's folder.
      expect(find.text('Reading the folder…'), findsOneWidget);
      expect(
        harness.backend.sent.where((m) => m.hasWatchFiles()).last.watchFiles,
        WatchFiles(projectId: 'p1'),
      );

      await tester.tap(find.bySemanticsLabel('Back').last);
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.text('Reading the folder…'), findsNothing);
      expect(layoutsSent(harness), isEmpty, reason: 'the pane stays');
    });
  }
}
