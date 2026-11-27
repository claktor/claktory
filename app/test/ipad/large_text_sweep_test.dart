// From Sorbet's S5 iPad sweep at larger text (AX1, AX5): a form's three
// buttons don't fit a dialog's row, so its less common action (Delete…)
// goes on a line of its own under Cancel and Save, as on a phone; and the
// sidebar's foot shows the count of agents at work whole, never cut to
// half its height; and in Slide Over, an agent's extensions put their names
// under the switch, whole, never "Messa|ging".
import 'dart:async' show unawaited;

import 'package:factory_app/controls.dart' show showModal;
import 'package:factory_app/forms.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/team_activity.dart';
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../messaging_test.dart' as messaging show workspace;
import '../phone_sizes_test.dart' show loadGeist;
import 'as_ipad.dart';
import 'window_fixtures.dart' show TextFactor;

/// A Pro 13-inch on its side, less its safe area.
const _pro13Landscape = Size(1376, 980);

Future<Harness> _show(WidgetTester tester, Size window, double text) async {
  asIPad();
  tester.platformDispatcher.textScaleFactorTestValue = text;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  final harness = Harness(messaging.workspace());
  await harness.show(tester, size: window);
  await pumpFrames(tester);
  return harness;
}

void main() {
  for (final (name, text) in [
    ('AX1', TextFactor.ax1),
    ('AX5', TextFactor.ax5),
  ]) {
    for (final window in const [Size(744, 1133), _pro13Landscape]) {
      for (final (form, delete) in [
        ('Agent settings', 'Delete agent…'),
        ('Project settings', 'Delete project…'),
      ]) {
        testWidgets('$form at $name in a $window window: Delete… on its own '
            'line under Cancel and Save, nothing overflowing', (tester) async {
          final harness = await _show(tester, window, text);
          final workspace = harness.backend.workspace;
          unawaited(
            showModal(
              tester.element(find.byType(Sidebar)),
              (context, close) => form == 'Agent settings'
                  ? AgentSettingsForm(
                      agent: workspace.agent('a1')!,
                      close: close,
                    )
                  : ProjectSettingsForm(
                      project: workspace.workspace.projects.first,
                      close: close,
                    ),
            ),
          );
          await pumpFrames(tester);
          await tester.ensureVisible(find.text(delete));
          await pumpFrames(tester);
          expect(tester.takeException(), isNull);
          final save = tester.getRect(find.text('Save'));
          final cancel = tester.getRect(find.text('Cancel'));
          final below = tester.getRect(find.text(delete));
          expect(below.top, greaterThan(save.bottom));
          expect(below.top, greaterThan(cancel.bottom));
          expect(save.right, lessThanOrEqualTo(window.width));
        });
      }
    }

    testWidgets("the sidebar's foot at $name on a Pro 13-inch on its side: "
        'the count of agents at work whole', (tester) async {
      await _show(tester, _pro13Landscape, text);
      final count = find.descendant(
        of: find.byType(TeamActivity),
        matching: find.byType(RichText),
      );
      final paragraph = tester.renderObject<RenderParagraph>(count);
      final whole = paragraph.getDryLayout(
        BoxConstraints(maxWidth: paragraph.size.width),
      );
      expect(paragraph.size.height, greaterThanOrEqualTo(whole.height - 0.5));
      // Its pill holds it.
      final pill = tester.getRect(find.byType(TeamActivity));
      expect(pill.height, greaterThanOrEqualTo(whole.height));
    });
  }

  testWidgets('at the default text on a Mac, Delete… stays at the left of '
      'the row, and the count sits in its 24-point cell', (tester) async {
    final harness = Harness(messaging.workspace());
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester);
    final workspace = harness.backend.workspace;
    unawaited(
      showModal(
        tester.element(find.byType(Sidebar)),
        (context, close) =>
            AgentSettingsForm(agent: workspace.agent('a1')!, close: close),
      ),
    );
    await pumpFrames(tester);
    await tester.ensureVisible(find.text('Delete agent…'));
    await pumpFrames(tester);
    final save = tester.getRect(find.text('Save'));
    final delete = tester.getRect(find.text('Delete agent…'));
    expect(delete.center.dy, closeTo(save.center.dy, 1));
    expect(delete.right, lessThan(save.left));
    expect(tester.getSize(find.byType(TeamActivity)).height, 24);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets("Agent settings' extensions in Slide Over (320) at AX5: each "
      'name whole, never broken mid-word', (tester) async {
    // As wide as on the iPad: the test font's square letters break any word.
    await tester.runAsync(loadGeist);
    asIPad();
    tester.platformDispatcher.textScaleFactorTestValue = TextFactor.ax5;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    const names = {
      'agents': 'Messaging',
      'checklist': 'Checklist',
      'comments': 'Comments',
      'database': 'Database',
      'devices': 'Devices',
      'reports': 'Reports',
      'highlights': 'Highlights',
      'memory': 'Memory',
      'monitors': 'Monitors',
      'resources': 'Resources',
      'secrets': 'Secrets',
      'tree': 'Own checkout',
    };
    final snapshot = messaging.workspace();
    snapshot.extensions
      ..clear()
      ..addAll([
        for (final MapEntry(:key, :value) in names.entries)
          Extension(id: key, name: value, icon: 'users'),
      ]);
    final agent = snapshot.agents.firstWhere((agent) => agent.id == 'a1');
    agent.extensions
      ..clear()
      ..addAll([
        for (final id in names.keys) AgentExtension(id: id, enabled: true),
      ]);
    final harness = Harness(snapshot);
    await harness.show(tester, size: const Size(320, 834));
    await pumpFrames(tester);
    final workspace = harness.backend.workspace;
    unawaited(
      showModal(
        tester.element(
          find
              .descendant(of: find.byType(Overlay), matching: find.byType(Text))
              .first,
        ),
        (context, close) =>
            AgentSettingsForm(
              agent: workspace.agent('a1')!,
              close: close,
              showExtensions: true,
            ),
      ),
    );
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    for (final name in names.values) {
      final label = find
          .byWidgetPredicate(
            // After its glyph, when it fits beside the switch.
            (widget) =>
                widget is RichText &&
                widget.text.toPlainText().replaceFirst('\uFFFC ', '') == name,
            skipOffstage: false,
          )
          .last;
      await tester.ensureVisible(label);
      await pumpFrames(tester);
      final paragraph = tester.renderObject<RenderParagraph>(label);
      final start = paragraph.text.toPlainText().indexOf(name);
      // Every line after the first starts after a space.
      double top(int at) => paragraph
          .getBoxesForSelection(
            TextSelection(baseOffset: start + at, extentOffset: start + at + 1),
          )
          .first
          .top;
      for (var at = 1; at < name.length; at++) {
        if (name[at] != ' ' && top(at) > top(at - 1) + 1) {
          expect(name[at - 1], ' ', reason: '"$name" breaks at $at');
        }
      }
    }
  });
}
