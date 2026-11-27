import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

const _raw =
    '401 {"type":"error","error":{"type":"authentication_error",'
    '"message":"invalid x-api-key"}}';

/// Builder's conversation, its last run refused as [refused] says, with
/// Anthropic's [accounts] signed in.
Future<Harness> _show(
  WidgetTester tester,
  CredentialsRefused? refused, {
  List<Account> accounts = const [],
}) async {
  final snapshot = workspace(agents: [builder(working: false)]);
  snapshot.providers.first.accounts.addAll(accounts);
  final harness = Harness(snapshot);
  harness.backend.transcript.load(
    Transcript(
      agentId: 'a1',
      entries: [
        Entry(
          seq: Int64(1),
          notice: Notice(
            text: _raw,
            level: NoticeLevel.NOTICE_LEVEL_ERROR,
            refused: refused,
          ),
        ),
      ],
    ),
  );
  harness.backend.appliesLayouts = true;
  await harness.show(tester);
  await pumpFrames(tester);
  return harness;
}

void main() {
  testWidgets('a refused key says whose it was, and Change key… replaces it '
      'in Settings → Providers', (tester) async {
    final harness = await _show(
      tester,
      CredentialsRefused(
        providerId: 'anthropic',
        accountId: 'k1',
        methodId: 'api_key',
      ),
      accounts: [Account(id: 'k1', label: 'sk-…a1b2', plan: 'API key')],
    );
    expect(find.text("Anthropic refused Builder's key."), findsOneWidget);
    expect(find.text(_raw), findsNothing, reason: 'not the raw answer');
    expect(find.textContaining('to fix'), findsNothing);
    expect(find.text('Providers'), findsNothing);
    await tester.tap(find.text('Change key…'));
    await pumpFrames(tester);
    final start = harness.lastStartLogin;
    expect(start.providerId, 'anthropic');
    expect(start.methodId, 'api_key');
    expect(start.addAccount, isFalse, reason: 'in place of the refused key');
    expect(find.text('Providers'), findsWidgets);
  });

  testWidgets('a refused sign-in offers Sign in again…', (tester) async {
    final harness = await _show(
      tester,
      CredentialsRefused(
        providerId: 'anthropic',
        accountId: 'c1',
        methodId: 'oauth',
      ),
      accounts: [Account(id: 'c1', label: 'matt@example.com')],
    );
    expect(find.text("Anthropic refused Builder's sign-in."), findsOneWidget);
    await tester.tap(find.text('Sign in again…'));
    await pumpFrames(tester);
    expect(harness.lastStartLogin.methodId, 'oauth');
    expect(harness.lastStartLogin.addAccount, isFalse);
  });

  testWidgets("a refused key from the environment can't be replaced: Add a "
      'key… adds one before it', (tester) async {
    final harness = await _show(
      tester,
      CredentialsRefused(
        providerId: 'anthropic',
        accountId: 'env',
        methodId: 'api_key',
      ),
      accounts: [Account(id: 'env', label: 'ANTHROPIC_API_KEY')],
    );
    expect(
      find.text('Anthropic refused the key in ANTHROPIC_API_KEY.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Add a key…'));
    await pumpFrames(tester);
    expect(harness.lastStartLogin.methodId, 'api_key');
    expect(harness.lastStartLogin.addAccount, isTrue);
  });

  testWidgets("another account than the first: Settings → Providers, to "
      'choose', (tester) async {
    final harness = await _show(
      tester,
      CredentialsRefused(
        providerId: 'anthropic',
        accountId: 'k2',
        methodId: 'api_key',
      ),
      accounts: [
        Account(id: 'k1', label: 'sk-…a1b2'),
        Account(id: 'k2', label: 'sk-…c3d4'),
      ],
    );
    await tester.tap(find.text('Change key…'));
    await pumpFrames(tester);
    expect(harness.backend.sent.where((m) => m.hasStartLogin()), isEmpty);
    expect(find.text('Providers'), findsWidgets);
  });

  testWidgets('any other failure shows as before, with Ask Rivet to fix', (
    tester,
  ) async {
    await _show(tester, null);
    expect(find.text(_raw), findsOneWidget);
  });
}
