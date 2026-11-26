// #529, S2 (Squalo's multi-claktory-stories.md, Fugo's conditions): a
// Claktory is marked only once the account's list was read and lacks it.
// Signed out, or the list unreadable, nothing is known: nothing is marked
// at a glance, and only its details (its form) say what's known.
import 'package:factory_app/account_match.dart';
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/drafts.dart';
import 'package:factory_app/identity.dart' show Palette;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// An account check that says [state] of every Claktory.
class _Says extends AccountMatch {
  _Says(this.state)
    : super(
        remotes: () => const [],
        changes: ChangeNotifier(),
        account: ChangeNotifier(),
        theirs: () => null,
      );

  final AccountCheck state;

  @override
  AccountCheck? check(String? endpointId) => state;
}

void main() {
  final target = parsePairingCode(
    'factory://?t=${'a' * 64}&e=${'b' * 64}&name=nixos',
  )!;

  Claktory says(AccountCheck state) {
    final match = _Says(state);
    addTearDown(match.dispose);
    final backend = RecordingBackend(localBackend: false);
    backend.workspace.apply(Workspace(settings: Settings(name: 'nixos')));
    return Claktory(
      target: target,
      backend: backend,
      drafts: Drafts(MemoryDraftStorage()),
      match: () => match,
    );
  }

  Future<void> show(WidgetTester tester, Widget mark) async {
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Align(alignment: Alignment.topLeft, child: mark),
      ),
    );
  }

  for (final state in AccountCheck.values) {
    testWidgets('$state: marked at a glance only when known not on it', (
      tester,
    ) async {
      final claktory = says(state);
      final known = state == AccountCheck.notTheirs;
      expect(claktory.offAccount, known);
      expect(
        claktoryLabel('nixos', claktory),
        known ? 'nixos, not on your Claktory account' : 'nixos',
      );
      await show(tester, OffAccountMark(claktory: claktory, explained: true));
      expect(
        find.textContaining('Not on your Claktory account', findRichText: true),
        known ? findsOneWidget : findsNothing,
      );
      expect(find.textContaining("Couldn't check"), findsNothing);
      expect(find.textContaining('Sign in'), findsNothing);
      if (known) {
        // In `danger`: only here.
        final label = tester.renderObject<RenderParagraph>(
          find.textContaining(
            'Not on your Claktory account',
            findRichText: true,
          ),
        );
        expect(
          label.text.style?.color,
          Palette.of(tester.element(find.byType(OffAccountMark))).danger,
        );
      }
    });
  }

  testWidgets('its details say what is known: not on it, couldn\'t check, '
      'or signed out with Sign in…; theirs says nothing', (tester) async {
    await show(
      tester,
      OffAccountMark(claktory: says(AccountCheck.unreadable), details: true),
    );
    expect(
      find.text(
        "Couldn't check whether nixos is on your Claktory account. Claktory "
        'tries again by itself.',
      ),
      findsOneWidget,
    );
    await show(
      tester,
      OffAccountMark(claktory: says(AccountCheck.signedOut), details: true),
    );
    expect(
      find.text(
        'Sign in to your Claktory account in this app to check which '
        'Claktories are on it.',
      ),
      findsOneWidget,
    );
    expect(find.text('Sign in…'), findsOneWidget);
    await show(
      tester,
      OffAccountMark(claktory: says(AccountCheck.notTheirs), details: true),
    );
    expect(
      find.text(
        "It may belong to someone else. If it's yours, sign it in to your "
        'Claktory account.',
      ),
      findsOneWidget,
    );
    await show(
      tester,
      OffAccountMark(claktory: says(AccountCheck.theirs), details: true),
    );
    expect(find.byType(Text), findsNothing);
  });
}
