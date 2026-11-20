import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/provider_picker.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart' show SettingsView;
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'accounts_test.dart' show claude;
import 'app_test.dart' show builder, workspace;
import 'harness.dart';

/// Providers as the backend offers them: Anthropic signed in, the rest not.
List<Provider> providers({bool anthropicSignedIn = true}) => [
  anthropicSignedIn ? claude() : anthropic(),
  Provider(
    id: 'openai-codex',
    name: 'OpenAI Codex',
    methods: [LoginMethod(id: 'oauth', label: 'OpenAI (ChatGPT Plus/Pro)')],
  ),
  Provider(
    id: 'openai',
    name: 'OpenAI',
    methods: [LoginMethod(id: 'api_key', label: 'OpenAI API key')],
  ),
  Provider(
    id: 'openrouter',
    name: 'OpenRouter',
    methods: [
      LoginMethod(id: 'oauth', label: 'OpenRouter OAuth'),
      LoginMethod(id: 'api_key', label: 'OpenRouter API key'),
    ],
  ),
];

Future<Harness> showProviders(
  WidgetTester tester, {
  bool anthropicSignedIn = true,
}) async {
  final snapshot = workspace()
    ..providers.clear()
    ..providers.addAll(providers(anthropicSignedIn: anthropicSignedIn));
  final harness = Harness(snapshot);
  await harness.show(tester, size: const Size(1200, 1100));
  harness.page.value = AppPage.settings;
  await settle(tester);
  return harness;
}

final pickerField = find.descendant(
  of: find.byType(ProviderPicker),
  matching: find.byType(EditableText),
);

void main() {
  signInOpensProviders();
  test('typing matches every word, in a name, an id or a method', () {
    final choices = providerChoices(providers());
    List<String> matching(String query) => [
      for (final choice in choices)
        if (choiceMatches(choice, query)) choice.method.label,
    ];
    expect(matching(''), hasLength(6));
    expect(matching('openai'), [
      'OpenAI (ChatGPT Plus/Pro)',
      'OpenAI API key',
    ]);
    expect(matching('KEY router'), ['OpenRouter API key']);
    expect(matching('codex'), ['OpenAI (ChatGPT Plus/Pro)']);
    expect(matching('gemini'), isEmpty);
  });

  testWidgets('Settings lists only providers signed in to, and the picker '
      'adds another', (tester) async {
    final harness = await showProviders(tester);
    // Anthropic's card, with its accounts; no card for the others.
    expect(find.text('matt@work.test'), findsOneWidget);
    expect(find.text('OpenRouter'), findsNothing);
    // The list is closed until the field is used.
    expect(find.text('OpenRouter OAuth'), findsNothing);

    await tester.tap(pickerField);
    await settle(tester);
    expect(find.text('OpenRouter OAuth'), findsOneWidget);
    // Anthropic's say it's signed in already.
    expect(find.text('Signed in'), findsNWidgets(2));

    await tester.enterText(pickerField, 'router');
    await settle(tester);
    expect(find.text('OpenAI API key'), findsNothing);
    expect(find.text('OpenRouter API key'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(harness.last.startLogin.providerId, 'openrouter');
    expect(harness.last.startLogin.methodId, 'api_key');
    expect(harness.last.startLogin.addAccount, isFalse);
    // Picked: closed and cleared.
    expect(find.text('OpenRouter OAuth'), findsNothing);
    expect(tester.widget<EditableText>(pickerField).controller.text, isEmpty);
  });

  testWidgets('Esc closes the list, then clears what was typed', (
    tester,
  ) async {
    await showProviders(tester);
    await tester.tap(pickerField);
    await tester.enterText(pickerField, 'gem');
    await settle(tester);
    expect(find.text('No provider matches “gem”.'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(find.text('No provider matches “gem”.'), findsNothing);
    expect(tester.widget<EditableText>(pickerField).controller.text, 'gem');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(tester.widget<EditableText>(pickerField).controller.text, isEmpty);
  });

  testWidgets('a provider already signed in adds an account', (tester) async {
    final harness = await showProviders(tester);
    await tester.tap(pickerField);
    await tester.enterText(pickerField, 'claude');
    await settle(tester);
    await tester.tap(find.text('Anthropic (Claude Pro/Max)').last);
    await settle(tester);
    expect(harness.last.startLogin.providerId, 'anthropic');
    expect(harness.last.startLogin.addAccount, isTrue);
  });

  testWidgets('with none signed in, the list is the page: always open', (
    tester,
  ) async {
    final harness = await showProviders(tester, anthropicSignedIn: false);
    expect(
      find.text('Sign in to a provider so your agents can work.'),
      findsWidgets,
    );
    expect(find.text('Anthropic (Claude Pro/Max)'), findsOneWidget);
    expect(find.text('OpenRouter API key'), findsOneWidget);
    await tester.tap(find.text('OpenAI (ChatGPT Plus/Pro)'));
    await settle(tester);
    expect(harness.last.startLogin.providerId, 'openai-codex');
    // Its sign-in under way: its card shows, with it.
    final flow = LoginFlow(providerId: 'openai-codex', methodId: 'oauth');
    final snapshot = harness.backend.workspace.workspace.deepCopy()
      ..login = flow;
    await harness.apply(tester, snapshot);
    await settle(tester);
    expect(find.text('OpenAI Codex'), findsOneWidget);
    expect(find.text('Starting…'), findsOneWidget);
  });
}

void signInOpensProviders() {
  testWidgets("an agent's Sign in… opens Settings at Providers", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(
      workspace(agents: [builder(working: false, modelAvailable: false)])
        ..providers.clear()
        ..providers.addAll(providers()),
    );
    harness.backend.appliesLayouts = true;
    await harness.show(tester, size: const Size(1200, 500));
    await pumpFrames(tester);
    expect(find.text('Providers'), findsNothing);
    await tester.tap(find.text('Sign in…').first);
    await pumpFrames(tester);
    // Providers is far down; its title is now at the top of Settings,
    // the picker under it.
    final settings = tester.getRect(find.byType(SettingsView));
    final title = tester.getRect(find.text('Providers'));
    expect(title.top, greaterThanOrEqualTo(settings.top));
    expect(title.top, lessThan(settings.top + 120));
    expect(find.byType(ProviderPicker), findsOneWidget);
    expect(harness.backend.settingsSection.value, isNull);
  });
}
