import 'package:factory_app/controls.dart' show Button, TextField;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

const _default = 'You are Rivet. You help {{user}} set up Claktory.\n';

/// Opens the settings of the one agent, whose prompt is [prompt] and whose
/// default is [defaultPrompt].
Future<Harness> _open(
  WidgetTester tester, {
  required String prompt,
  String defaultPrompt = _default,
}) async {
  final agent = builder(working: false)
    ..prompt = prompt
    ..defaultPrompt = defaultPrompt;
  final harness = Harness(workspace(agents: [agent]));
  await harness.show(tester);
  await settle(tester);
  await openAgentSettings(tester);
  return harness;
}

final _reset = find.widgetWithText(Button, 'Reset to default');
const _isDefault =
    "Claktory's default. Each update brings the newest version until you "
    'change it.';

Finder get _promptField => find
    .descendant(
      of: find.byWidgetPredicate(
        (w) =>
            w is TextField && w.placeholder == 'What this agent does and how',
      ),
      matching: find.byType(EditableText),
    )
    .first;

void main() {
  testWidgets('a prompt that is the default says so, with nothing to reset', (
    tester,
  ) async {
    await _open(tester, prompt: _default);
    expect(find.text(_isDefault), findsOneWidget);
    expect(_reset, findsNothing);
    // Its placeholders show as written.
    expect(find.textContaining('{{user}}'), findsWidgets);
  });

  testWidgets('a changed prompt resets to the default, applied with Save', (
    tester,
  ) async {
    final harness = await _open(tester, prompt: 'Only build websites.');
    expect(find.text('Changed from the default.'), findsOneWidget);
    await tester.ensureVisible(_reset);
    await pumpFrames(tester);
    await tester.tap(_reset);
    await settle(tester);
    expect(find.text(_isDefault), findsOneWidget);
    expect(_reset, findsNothing);
    expect(tester.widget<EditableText>(_promptField).controller.text, _default);
    final sent = harness.backend.sent.length;
    await tester.ensureVisible(find.text('Save'));
    await pumpFrames(tester);
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(harness.backend.sent.length, greaterThan(sent));
    final update = harness.last.updateAgent;
    expect(update.prompt, _default.trim());

    // Typing it back by hand counts as the default too.
    await _open(tester, prompt: 'Only build websites.');
    await tester.enterText(_promptField, '$_default  ');
    await settle(tester);
    expect(find.text(_isDefault), findsOneWidget);
  });

  testWidgets('an agent with no default shows neither', (tester) async {
    await _open(tester, prompt: 'Build things.', defaultPrompt: '');
    expect(find.textContaining('default.'), findsNothing);
    expect(_reset, findsNothing);
  });

  testWidgets("opening Rivet's settings with its default changes nothing", (
    tester,
  ) async {
    // The default as shipped ends with a line break; the form trims.
    final harness = await _open(tester, prompt: _default);
    await tester.ensureVisible(find.text('Save'));
    await pumpFrames(tester);
    final sent = harness.backend.sent.length;
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(
      harness.backend.sent.skip(sent).where((r) => r.hasUpdateAgent()),
      isEmpty,
    );
  });
}
