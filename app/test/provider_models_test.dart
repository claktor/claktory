// Each provider signed in to says where its models come
// from and when they were last read from the provider, with Refresh models.
import 'package:factory_app/login.dart' show ProviderCard;
import 'package:factory_app/provider_models.dart';
import 'package:factory_app/tell_rivet.dart' show TellRivet;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Provider _provider({
  bool fetchable = true,
  int fetchedMs = 0,
  bool fetching = false,
  String error = '',
  bool signedOut = false,
  List<Account> accounts = const [],
}) => Provider(
  id: 'anthropic',
  name: 'Anthropic',
  signedInWith: 'an API key',
  modelsFetchable: fetchable,
  modelsFetchedMs: Int64(fetchedMs),
  modelsFetching: fetching,
  modelsFetchError: error,
  modelsFetchSignedOut: signedOut,
  accounts: accounts,
);

Future<Harness> _show(WidgetTester tester, Provider provider) async {
  final harness = Harness(Workspace(providers: [provider]));
  await harness.show(
    tester,
    home: Align(
      alignment: Alignment.topLeft,
      child: SizedBox(width: 480, child: ProviderModels(provider: provider)),
    ),
  );
  await settle(tester);
  return harness;
}

void main() {
  testWidgets('says when the models were read, and Refresh models reads '
      'them now', (tester) async {
    final hours = DateTime.now()
        .subtract(const Duration(hours: 3, minutes: 5))
        .millisecondsSinceEpoch;
    final harness = await _show(tester, _provider(fetchedMs: hours));
    expect(find.text('Models updated 3 h ago'), findsOneWidget);
    await tester.tap(find.text('Refresh models'));
    await settle(tester);
    expect(harness.last.refreshModels.providerId, 'anthropic');
  });

  testWidgets('never read: the list Claktory came with', (tester) async {
    await _show(tester, _provider());
    expect(find.text("Claktory's built-in list"), findsOneWidget);
    expect(find.text('Refresh models'), findsOneWidget);
  });

  testWidgets("a list that can't be read now has no Refresh models", (
    tester,
  ) async {
    await _show(tester, _provider(fetchable: false));
    expect(find.text("Claktory's built-in list"), findsOneWidget);
    expect(find.text('Refresh models'), findsNothing);
  });

  testWidgets('while a read is under way it says so, and the button waits', (
    tester,
  ) async {
    final harness = await _show(tester, _provider(fetching: true));
    expect(find.text('Refreshing models…'), findsOneWidget);
    await tester.tap(find.text('Refreshing…'));
    await settle(tester);
    expect(harness.backend.sent.where((m) => m.hasRefreshModels()), isEmpty);
  });

  testWidgets("the last read's failure shows, the list kept", (tester) async {
    await _show(
      tester,
      _provider(
        fetchedMs: DateTime.now().millisecondsSinceEpoch,
        error: 'Anthropic couldn’t be reached: timed out.',
      ),
    );
    expect(find.text('Models updated just now'), findsOneWidget);
    expect(
      find.text('Anthropic couldn’t be reached: timed out.'),
      findsOneWidget,
    );
  });

  testWidgets("another failure keeps Tell Rivet beside it", (tester) async {
    await _show(tester, _provider(error: 'Anthropic couldn’t be reached.'));
    expect(find.byType(TellRivet), findsOneWidget);
    expect(find.text('Sign in again…'), findsNothing);
  });

  testWidgets('a read the provider refused for its sign-in says Sign-in '
      'expired with Sign in again…, adding as the account would, and no '
      'Tell Rivet (settings.md ST2)', (tester) async {
    final harness = await _show(
      tester,
      _provider(
        error: 'Anthropic refused the sign-in.',
        signedOut: true,
        accounts: [Account(id: 'a1', methodId: 'api_key')],
      ),
    );
    expect(find.text('Sign-in expired'), findsOneWidget);
    expect(find.text('Anthropic refused the sign-in.'), findsNothing);
    expect(find.byType(TellRivet), findsNothing);
    await tester.tap(find.text('Sign in again…'));
    await settle(tester);
    final start = harness.lastStartLogin;
    expect(start.providerId, 'anthropic');
    expect(start.methodId, 'api_key');
    expect(start.addAccount, isTrue);
  });

  testWidgets("a refused refresh shows the backend's sentence", (tester) async {
    final harness = await _show(tester, _provider());
    harness.backend.refuse = 'Sign in to Anthropic first.';
    await tester.tap(find.text('Refresh models'));
    await settle(tester);
    expect(find.text('Sign in to Anthropic first.'), findsOneWidget);
    expect(find.text('Refresh models'), findsOneWidget);
  });

  testWidgets("it's in the provider's card in Settings → Providers once "
      'signed in', (tester) async {
    final harness = Harness(Workspace(providers: [_provider()]));
    await harness.show(
      tester,
      home: SingleChildScrollView(
        child: ProviderCard(provider: _provider(), flow: null),
      ),
    );
    await settle(tester);
    expect(find.byType(ProviderModels), findsOneWidget);
  });

  testWidgets("a refresh's failure stays on its provider's card when "
      "another sorts in above it (Abbacchio's row 1707)", (tester) async {
    final openRouter = Provider(
      id: 'openrouter',
      name: 'OpenRouter',
      signedInWith: 'an API key',
      modelsFetchable: true,
    );
    final openAi = Provider(
      id: 'openai',
      name: 'OpenAI',
      signedInWith: 'an API key',
    );
    final harness = Harness(Workspace(providers: [openRouter]));
    await harness.show(tester, size: const Size(1200, 1200));
    await settle(tester);
    harness.page.value = AppPage.settings;
    await settle(tester);
    harness.backend.settingsSection.value = 'providers';
    await settle(tester);
    harness.backend.refuse = 'OpenRouter answered 401 Unauthorized';
    await tester.tap(find.text('Refresh models'));
    await settle(tester);
    expect(find.text('OpenRouter answered 401 Unauthorized'), findsOneWidget);

    await harness.apply(tester, Workspace(providers: [openAi, openRouter]));
    await settle(tester);
    Finder inCard(String id, Finder finder) => find.descendant(
      of: find.byWidgetPredicate(
        (widget) => widget is ProviderCard && widget.provider.id == id,
      ),
      matching: finder,
    );
    expect(
      inCard('openrouter', find.text('OpenRouter answered 401 Unauthorized')),
      findsOneWidget,
    );
    expect(
      inCard('openai', find.text('OpenRouter answered 401 Unauthorized')),
      findsNothing,
    );
  });

  testWidgets('not signed in, the card has no models line', (tester) async {
    final harness = Harness(Workspace());
    await harness.show(
      tester,
      home: SingleChildScrollView(
        child: ProviderCard(
          provider: Provider(id: 'anthropic', name: 'Anthropic'),
          flow: null,
        ),
      ),
    );
    await settle(tester);
    expect(find.byType(ProviderModels), findsNothing);
  });
}
