import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'accounts_test.dart' show claude, showSettings;
import 'harness.dart';

Finder toggle(String label) => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == label,
);

Future<void> reveal(WidgetTester tester, Finder target) async {
  await revealInSettings(tester, target);
  await pumpFrames(tester);
}

void main() {
  testWidgets('idle conversations are summarized unless turned off, and '
      'after 5 minutes only when asked', (tester) async {
    final harness = await showSettings(tester, claude()..cacheTtl = '5m');
    final settings = harness.backend.workspace.workspace.settings;
    settings.idleCompaction = true;
    await harness.apply(tester, harness.backend.workspace.workspace);
    await reveal(tester, find.text('Also after 5 minutes idle'));

    await tester.tap(toggle('Also after 5 minutes idle'));
    await tester.pump();
    expect(
      harness.last.updateSettings,
      UpdateSettings(idleCompactionShortCache: true),
    );

    await tester.ensureVisible(toggle('Summarize idle conversations'));
    await pumpFrames(tester);
    await tester.tap(toggle('Summarize idle conversations'));
    await tester.pump();
    expect(harness.last.updateSettings, UpdateSettings(idleCompaction: false));

    // Off, the 5-minute switch can't be changed.
    final off = harness.backend.workspace.workspace.deepCopy();
    off.settings.idleCompaction = false;
    await harness.apply(tester, off);
    final sent = harness.backend.sent.length;
    await tester.tap(toggle('Also after 5 minutes idle'));
    await tester.pump();
    expect(harness.backend.sent, hasLength(sent));
  });

  testWidgets('Anthropic keeps conversations ready 5 minutes or an hour', (
    tester,
  ) async {
    final harness = await showSettings(tester, claude()..cacheTtl = '5m');
    await reveal(tester, find.text('Keep conversations ready'));
    expect(find.text('5 minutes'), findsOneWidget);
    await tester.tap(find.text('5 minutes'));
    await settle(tester);
    await tester.tap(find.text('1 hour').last);
    await settle(tester);
    expect(
      harness.last.updateSettings,
      UpdateSettings(
        cacheTtl: CacheTtl(key: 'anthropic', ttl: '1h'),
      ),
    );
  });

  testWidgets("a provider whose cache Claktory can't choose has no choice", (
    tester,
  ) async {
    await showSettings(tester, claude());
    expect(find.text('Keep conversations ready'), findsNothing);
  });
}
