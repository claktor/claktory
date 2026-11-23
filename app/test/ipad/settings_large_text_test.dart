import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/rail.dart' show ClaktoryRail;
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/shell.dart';
import 'package:flutter/rendering.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../accounts_test.dart' show claude;
import '../app_test.dart' show workspace;
import '../harness.dart';
import '../phone_sizes_test.dart' show loadGeist;
import 'as_ipad.dart';
import 'window_fixtures.dart' show TextFactor;

final _letter = RegExp(r'[A-Za-z]');

/// Every place in Settings' laid-out text where a line ends inside a word:
/// "Clakto|ry name".
/// As Sorbet's iPad sweep counts them: with the app's font, leaving out a
/// paragraph as wide as most of the window (nothing could hold its word)
/// and a word longer than "conversations" (a long name).
Set<String> _midWord(WidgetTester tester) {
  final found = <String>{};
  final window = tester.view.physicalSize.width / tester.view.devicePixelRatio;
  final settings = tester.renderObject(find.byType(SettingsView));
  void visit(RenderObject object) {
    // A paragraph cut short with … ends where it's cut, not at a break.
    if (object is RenderParagraph &&
        object.hasSize &&
        !object.didExceedMaxLines &&
        object.size.width < window * 0.6) {
      final text = object.text.toPlainText(includeSemanticsLabels: false);
      double top(int offset) =>
          object.getOffsetForCaret(TextPosition(offset: offset), Rect.zero).dy;
      for (var end = 1; end < text.length; end++) {
        if (!_letter.hasMatch(text[end - 1]) || !_letter.hasMatch(text[end])) {
          continue;
        }
        // The caret before the letter is on a lower line than the one
        // before it: the line ends inside the word.
        var from = end, to = end;
        while (from > 0 && _letter.hasMatch(text[from - 1])) {
          from--;
        }
        while (to < text.length && _letter.hasMatch(text[to])) {
          to++;
        }
        if (to - from > 13) continue;
        if (top(end) > top(end - 1) + 1) {
          found.add(
            '${text.substring(0, end)}|${text.substring(end)}'.replaceAll(
              '\n',
              ' ',
            ),
          );
        }
      }
    }
    object.visitChildren(visit);
  }

  visit(settings);
  return found;
}

void main() {
  setUpAll(loadGeist);

  // Sorbet's row 103: at the largest text, Settings' label column and the
  // lines beside it kept their standard widths, so words broke in the middle
  // ("Clakto|ry name", "Versio|n"), and a line of controls overflowed.
  for (final window in const [
    Size(600, 800),
    Size(678, 1024),
    Size(990, 1024),
    Size(1376, 1032),
  ]) {
    testWidgets('on an iPad at AX5 no word breaks in Settings in a $window '
        'window, and nothing overflows', (tester) async {
      asIPad();
      tester.platformDispatcher.textScaleFactorTestValue = TextFactor.ax5;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final errors = <String>[];
      final old = FlutterError.onError;
      FlutterError.onError = (details) => errors.add(details.toString());
      addTearDown(() => FlutterError.onError = old);
      final now = DateTime.now().millisecondsSinceEpoch;
      final snapshot = workspace()
        ..providers.clear()
        ..providers.add(claude())
        // Zucchero's row 133: two devices, one not confirmed yet; durations
        // and numbers that aren't the shortest.
        ..remote = RemoteAccess(
          on: true,
          port: 7443,
          fingerprint: 'f' * 64,
          machine: 'mac-mini',
          addresses: ['100.64.0.2'],
          devices: [
            PairedDevice(
              id: 'd1',
              name: 'iPhone SE',
              pairedMs: Int64(now - const Duration(days: 1).inMilliseconds),
              lastSeenMs: Int64(
                now - const Duration(minutes: 8).inMilliseconds,
              ),
              confirmByMs: Int64(now + const Duration(days: 2).inMilliseconds),
            ),
            PairedDevice(
              id: 'd2',
              name: 'iPad Pro',
              pairedMs: Int64(now - const Duration(days: 1).inMilliseconds),
              connected: true,
              confirmed: true,
            ),
          ],
        );
      snapshot.settings
        ..checklist = ChecklistSettings(
          agentBlockedMinutes: 240,
          personBlockedMinutes: 1440,
          externalRetryMinutes: 120,
          reaskMinutes: 1440,
          overdueMinutes: 90,
        )
        ..ramPercent = 90
        ..diskFreeGb = 10
        ..diskFreePercent = 10
        ..buildCacheGb = 25
        ..compactAtTokens = Int64(400000);
      final harness = Harness(snapshot);
      final focus = WorkbenchFocus();
      addTearDown(focus.dispose);
      // The window as main.dart builds an iPad's, the workbench's.
      await harness.show(
        tester,
        size: window,
        home: WorkbenchScope(
          focus: focus,
          child: const Shell(rail: ClaktoryRail()),
        ),
      );
      harness.page.value = AppPage.settings;
      await settle(tester);
      final scrollable = find
          .descendant(
            of: find.byType(SettingsView),
            matching: find.byType(Scrollable),
          )
          .first;
      final broken = <String>{};
      final clipped = <String>{};
      for (var i = 0; i < 400; i++) {
        broken.addAll(_midWord(tester));
        // A field's value whole in its box (Zucchero's row 133: "9" for
        // 90, "40" for 400k).
        for (final field
            in tester.allRenderObjects.whereType<RenderEditable>()) {
          final text = field.text?.toPlainText() ?? '';
          if (text.isEmpty || !field.hasSize) continue;
          if (field.getMaxIntrinsicWidth(double.infinity) >
              field.size.width + 1) {
            clipped.add(text);
          }
        }
        final position = tester.state<ScrollableState>(scrollable).position;
        if (position.pixels >= position.maxScrollExtent) break;
        position.jumpTo(position.pixels + 400);
        await pumpFrames(tester);
      }
      FlutterError.onError = old;
      expect(broken, isEmpty);
      expect(clipped, isEmpty);
      expect(errors, isEmpty);
    });
  }
}
