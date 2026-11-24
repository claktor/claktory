// The fallback model (#225): what a step a model declines is tried again
// on, an agent's own in its settings and the Claktory-wide default in
// Settings → Defaults.
import 'package:factory_app/controls.dart' show Field, showModal;
import 'package:factory_app/forms.dart';
import 'package:factory_app/model_select.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({String defaultFallback = '', String own = ''}) =>
    Workspace(
      projects: [Project(id: 'p1', name: 'Factory')],
      agents: [
        Agent(
          id: 'sam',
          name: 'Sam',
          projectId: 'p1',
          modelAvailable: true,
          model: 'anthropic/claude-opus-5-5',
          fallbackModel: own,
        ),
      ],
      selectedAgentId: 'sam',
      models: [
        Model(
          id: 'anthropic/claude-opus-5-5',
          name: 'Claude Opus 5.5',
          efforts: ['low', 'high'],
        ),
        Model(id: 'openai-codex/gpt-5.5', name: 'GPT-5.5', efforts: ['high']),
      ],
      settings: Settings(
        userName: 'Matt',
        defaultModel: 'anthropic/claude-opus-5-5',
        defaultFallbackModel: defaultFallback,
      ),
    );

/// The model field in the field labeled [label].
Finder _select(String label) => find.descendant(
  of: find.ancestor(of: find.text(label), matching: find.byType(Field)),
  matching: find.byType(ModelSelect),
);

Future<Harness> _agentSettings(
  WidgetTester tester,
  Workspace workspace, {
  Size size = const Size(1200, 1100),
}) async {
  final harness = Harness(workspace);
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  showModal(
    tester.element(find.text('Sam').first),
    (context, close) =>
        AgentSettingsForm(agent: workspace.agents.first, close: close),
  );
  await pumpFrames(tester);
  return harness;
}

void main() {
  testWidgets("an agent's fallback follows the default until one is picked, "
      'and picking it saves it', (tester) async {
    final harness = await _agentSettings(
      tester,
      _workspace(defaultFallback: 'openai-codex/gpt-5.5'),
    );
    final select = tester.widget<ModelSelect>(_select('Fallback model'));
    expect(select.value, '');
    expect(
      [for (final option in select.extra) option.label],
      ['Default (GPT-5.5)'],
    );
    expect(find.text('Default (GPT-5.5)', findRichText: true), findsOneWidget);
    expect(
      find.text(
        'If its model declines a step for safety reasons, Claktory tries it '
        'again on this one.',
      ),
      findsOneWidget,
    );
    select.onChanged('anthropic/claude-opus-5-5');
    await pumpFrames(tester);
    await tester.tap(find.text('Save'));
    await pumpFrames(tester);
    final update = harness.backend.sent.lastWhere((m) => m.hasUpdateAgent());
    expect(update.updateAgent.fallbackModel, 'anthropic/claude-opus-5-5');
    expect(update.updateAgent.hasModel(), isFalse);
  });

  testWidgets('Follow the default clears its own; with no default it says '
      'none', (tester) async {
    final harness = await _agentSettings(
      tester,
      _workspace(own: 'openai-codex/gpt-5.5'),
    );
    final select = tester.widget<ModelSelect>(_select('Fallback model'));
    expect(select.value, 'openai-codex/gpt-5.5');
    expect(select.extra.first.label, 'Default (none)');
    select.onChanged('');
    await pumpFrames(tester);
    await tester.tap(find.text('Save'));
    await pumpFrames(tester);
    final update = harness.backend.sent.lastWhere((m) => m.hasUpdateAgent());
    expect(update.updateAgent.hasFallbackModel(), isTrue);
    expect(update.updateAgent.fallbackModel, '');
  });

  testWidgets('one that is no longer available says so', (tester) async {
    await _agentSettings(tester, _workspace(own: 'gone/model'));
    final select = tester.widget<ModelSelect>(_select('Fallback model'));
    expect(select.value, 'gone/model');
    expect(
      find.descendant(
        of: _select('Fallback model'),
        matching: find.text('No model available', findRichText: true),
      ),
      findsOneWidget,
    );
  });

  testWidgets('Settings → Defaults sets the default fallback, or none', (
    tester,
  ) async {
    final harness = Harness(
      _workspace(defaultFallback: 'openai-codex/gpt-5.5'),
    );
    harness.page.value = AppPage.settings;
    await harness.show(tester, size: const Size(1400, 1000));
    await pumpFrames(tester);
    await tester.scrollUntilVisible(
      find.text('Default fallback model'),
      200,
      scrollable: find
          .descendant(
            of: find.byType(SettingsView),
            matching: find.byType(Scrollable),
          )
          .first,
    );
    await pumpFrames(tester);
    final row = find.ancestor(
      of: find.text('Default fallback model'),
      matching: find.byType(SettingRow),
    );
    final select = tester.widget<ModelSelect>(
      find.descendant(of: row, matching: find.byType(ModelSelect)),
    );
    expect(select.value, 'openai-codex/gpt-5.5');
    expect([for (final option in select.extra) option.label], ['None']);
    select.onChanged('');
    await pumpFrames(tester);
    final update = harness.backend.sent.lastWhere((m) => m.hasUpdateSettings());
    expect(update.updateSettings.hasDefaultFallbackModel(), isTrue);
    expect(update.updateSettings.defaultFallbackModel, '');
  });

  for (final width in [320.0, 440.0]) {
    testWidgets('on a phone at the largest text both fit $width points', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final errors = <String>[];
      final old = FlutterError.onError;
      FlutterError.onError = (details) {
        errors.add(
          details
              .toString()
              .split('\n')
              .where((line) => line.contains('.dart:'))
              .take(2)
              .join(' '),
        );
        old?.call(details);
      };
      addTearDown(() => FlutterError.onError = old);
      final workspace = _workspace(defaultFallback: 'openai-codex/gpt-5.5');
      final harness = await _agentSettings(
        tester,
        workspace,
        size: Size(width, 700),
      );
      expect(_select('Fallback model'), findsOneWidget);
      for (var i = 0; i < 10; i++) {
        await tester.drag(
          find.byType(Scrollable).last,
          const Offset(0, -300),
          warnIfMissed: false,
        );
        await pumpFrames(tester);
      }
      tester.takeException();
      expect(errors, isEmpty);

      harness.page.value = AppPage.settings;
      harness.backend.settingsSection.value = 'defaults';
      await pumpFrames(tester);
      expect(find.text('Default fallback model'), findsWidgets);
      tester.takeException();
      expect(errors, isEmpty);
    });
  }
}
