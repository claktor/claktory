// Choosing a model: the field names the provider, and
// the list is grouped by provider with a search, so "Claude Opus 5" from
// Anthropic and from OpenRouter can be told apart.
import 'package:factory_app/controls.dart' show SelectOption;
import 'package:factory_app/mobile/metrics.dart' show Touch;
import 'package:factory_app/model_select.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  providers: [
    Provider(id: 'anthropic', name: 'Anthropic'),
    Provider(id: 'openai-codex', name: 'ChatGPT'),
    Provider(id: 'openrouter', name: 'OpenRouter'),
  ],
  models: [
    Model(id: 'anthropic/claude-opus-5', name: 'Claude Opus 5'),
    Model(id: 'anthropic/claude-sonnet-5', name: 'Claude Sonnet 5'),
    Model(id: 'openrouter/anthropic/claude-opus-5', name: 'Claude Opus 5'),
    Model(id: 'openrouter/openai/gpt-5.5', name: 'OpenAI: GPT-5.5'),
    Model(id: 'openai-codex/gpt-5.5', name: 'GPT-5.5', imageOutput: true),
  ],
);

Future<List<String>> _show(
  WidgetTester tester, {
  String value = 'openrouter/anthropic/claude-opus-5',
  List<SelectOption<String>> extra = const [],
  Size size = const Size(1200, 900),
}) async {
  final chosen = <String>[];
  final workspace = _workspace();
  final harness = Harness(workspace);
  await harness.show(
    tester,
    size: size,
    home: Align(
      alignment: Alignment.topLeft,
      child: SizedBox(
        width: 320,
        child: ModelSelect(
          workspace: workspace,
          value: value,
          extra: extra,
          onChanged: chosen.add,
        ),
      ),
    ),
  );
  await pumpFrames(tester);
  return chosen;
}

Future<void> _open(WidgetTester tester) async {
  await tester.tap(find.byType(ModelSelect));
  await pumpFrames(tester);
}

void main() {
  _typedIds();

  testWidgets('the field names the provider beside the model', (tester) async {
    await _show(tester);
    expect(
      find.text('Claude Opus 5  OpenRouter', findRichText: true),
      findsOneWidget,
    );
  });

  testWidgets('a model that can make images says so in the list (#403), '
      'and only that one', (tester) async {
    await _show(tester);
    await _open(tester);
    expect(
      find.text('GPT-5.5  $modelMakesImages', findRichText: true),
      findsOneWidget,
    );
    expect(find.textContaining(modelMakesImages, findRichText: true), findsOne);
  });

  testWidgets("the list is grouped by provider in Settings' order", (
    tester,
  ) async {
    await _show(tester);
    await _open(tester);
    double top(Finder finder) => tester.getTopLeft(finder).dy;
    final anthropic = find.text('Anthropic');
    final chatgpt = find.text('ChatGPT');
    final openrouter = find.text('OpenRouter');
    expect(top(anthropic), lessThan(top(chatgpt)));
    expect(top(chatgpt), lessThan(top(openrouter)));
    final opus = find.text('Claude Opus 5');
    expect(opus, findsNWidgets(2));
    // One under Anthropic, one under OpenRouter.
    expect(top(opus.first), inExclusiveRange(top(anthropic), top(chatgpt)));
    expect(top(opus.last), greaterThan(top(openrouter)));
  });

  testWidgets('search narrows by name and provider; a click chooses', (
    tester,
  ) async {
    final chosen = await _show(tester, value: 'anthropic/claude-opus-5');
    await _open(tester);
    await tester.enterText(find.byType(EditableText), 'openrouter opus');
    await pumpFrames(tester);
    expect(find.text('Claude Opus 5'), findsOneWidget);
    expect(find.text('Claude Sonnet 5'), findsNothing);
    expect(find.text('Anthropic'), findsNothing);
    await tester.tap(find.text('Claude Opus 5'));
    await pumpFrames(tester);
    expect(chosen, ['openrouter/anthropic/claude-opus-5']);
    expect(find.text('Search models or providers'), findsNothing);
  });

  testWidgets('Enter in the search takes the first match', (tester) async {
    final chosen = await _show(tester);
    await _open(tester);
    await tester.enterText(find.byType(EditableText), 'gpt');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await pumpFrames(tester);
    expect(chosen, ['openai-codex/gpt-5.5']);
  });

  testWidgets('nothing matches, says so', (tester) async {
    await _show(tester);
    await _open(tester);
    await tester.enterText(find.byType(EditableText), 'gemini');
    await pumpFrames(tester);
    expect(find.text('No model matches “gemini”.'), findsOneWidget);
  });

  testWidgets('extra choices (the default) come first, and not in a search', (
    tester,
  ) async {
    final chosen = await _show(
      tester,
      value: '',
      extra: const [SelectOption('', 'Default (Claude Opus 5)')],
    );
    expect(
      find.text('Default (Claude Opus 5)', findRichText: true),
      findsOneWidget,
    );
    await _open(tester);
    expect(
      tester.getTopLeft(find.text('Default (Claude Opus 5)').last).dy,
      lessThan(tester.getTopLeft(find.text('Anthropic')).dy),
    );
    await tester.enterText(find.byType(EditableText), 'sonnet');
    await pumpFrames(tester);
    expect(find.text('Default (Claude Opus 5)'), findsOneWidget);
    await tester.tap(find.text('Claude Sonnet 5'));
    await pumpFrames(tester);
    expect(chosen, ['anthropic/claude-sonnet-5']);
  });

  testWidgets("a model that can't run reads No model available", (
    tester,
  ) async {
    await _show(tester, value: 'anthropic/claude-gone');
    expect(find.text('No model available', findRichText: true), findsOneWidget);
  });

  testWidgets('on a phone, each row is a finger tall', (tester) async {
    await _show(tester, size: const Size(390, 844));
    await _open(tester);
    expect(
      tester.getSize(find.text('Claude Sonnet 5')).height,
      lessThan(Touch.target),
    );
    final row = find
        .ancestor(
          of: find.text('Claude Sonnet 5'),
          matching: find.byType(Container),
        )
        .first;
    expect(tester.getSize(row).height, greaterThanOrEqualTo(Touch.target));
  });
}

// A model id the list doesn't have can be typed; its
// provider is asked before it's chosen, and a refusal shows.
Future<(Harness, List<String>)> _showSignedIn(WidgetTester tester) async {
  final chosen = <String>[];
  final workspace = _workspace();
  for (final provider in workspace.providers) {
    if (provider.id != 'openai-codex') provider.signedInWith = 'an API key';
  }
  final harness = Harness(workspace);
  await harness.show(
    tester,
    home: Align(
      alignment: Alignment.topLeft,
      child: SizedBox(
        width: 320,
        child: ModelSelect(
          workspace: workspace,
          value: 'anthropic/claude-opus-5',
          onChanged: chosen.add,
        ),
      ),
    ),
  );
  await pumpFrames(tester);
  await _open(tester);
  return (harness, chosen);
}

void _typedIds() {
  testWidgets('a whole model id not in the list: Use it, checked with its '
      'provider first', (tester) async {
    final (harness, chosen) = await _showSignedIn(tester);
    await tester.enterText(
      find.byType(EditableText),
      'openrouter/google/gemini-4-neon',
    );
    await pumpFrames(tester);
    expect(find.text('Not in the list'), findsOneWidget);
    expect(
      find.text('Use “google/gemini-4-neon” from OpenRouter'),
      findsOneWidget,
    );
    expect(find.textContaining('No model matches'), findsNothing);
    // Return uses it, since nothing in the list matches.
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await pumpFrames(tester);
    expect(harness.last.addModel.model, 'openrouter/google/gemini-4-neon');
    expect(chosen, ['openrouter/google/gemini-4-neon']);
  });

  testWidgets("the provider's refusal shows, and nothing is chosen", (
    tester,
  ) async {
    final (harness, chosen) = await _showSignedIn(tester);
    harness.backend.refuse =
        'Anthropic has no model “claude-x” for this sign-in.';
    await tester.enterText(find.byType(EditableText), 'anthropic/claude-x');
    await pumpFrames(tester);
    await tester.tap(find.text('Use “claude-x” from Anthropic'));
    await pumpFrames(tester);
    expect(harness.last.addModel.model, 'anthropic/claude-x');
    expect(
      find.text('Anthropic has no model “claude-x” for this sign-in.'),
      findsOneWidget,
    );
    expect(chosen, isEmpty);
    // Typing again clears it.
    await tester.enterText(find.byType(EditableText), 'anthropic/claude-y');
    await pumpFrames(tester);
    expect(find.textContaining('has no model'), findsNothing);
  });

  testWidgets('an id without its provider is offered from each provider '
      'signed in to', (tester) async {
    await _showSignedIn(tester);
    await tester.enterText(find.byType(EditableText), 'gemini-4-neon');
    await pumpFrames(tester);
    expect(find.text('Use “gemini-4-neon” from Anthropic'), findsOneWidget);
    expect(find.text('Use “gemini-4-neon” from OpenRouter'), findsOneWidget);
    // ChatGPT isn't signed in to.
    expect(find.text('Use “gemini-4-neon” from ChatGPT'), findsNothing);
  });

  testWidgets("while the list has a match, Return takes it and a word isn't "
      'offered as an id', (tester) async {
    final (harness, chosen) = await _showSignedIn(tester);
    await tester.enterText(find.byType(EditableText), 'sonnet');
    await pumpFrames(tester);
    expect(find.text('Not in the list'), findsNothing);
    await tester.enterText(find.byType(EditableText), 'openrouter/openai');
    await pumpFrames(tester);
    // An id, so it's offered too, after the list.
    expect(find.text('Use “openai” from OpenRouter'), findsOneWidget);
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await pumpFrames(tester);
    expect(chosen, ['openrouter/openai/gpt-5.5']);
    expect(harness.backend.sent.where((m) => m.hasAddModel()), isEmpty);
  });
}
