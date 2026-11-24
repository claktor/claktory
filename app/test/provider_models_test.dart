// Friend bug #14: each provider signed in to says where its models come
// from and when they were last read from the provider, with Refresh models.
import 'package:factory_app/login.dart' show ProviderCard;
import 'package:factory_app/provider_models.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Provider _provider({
  bool fetchable = true,
  int fetchedMs = 0,
  bool fetching = false,
  String error = '',
}) => Provider(
  id: 'anthropic',
  name: 'Anthropic',
  signedInWith: 'an API key',
  modelsFetchable: fetchable,
  modelsFetchedMs: Int64(fetchedMs),
  modelsFetching: fetching,
  modelsFetchError: error,
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
