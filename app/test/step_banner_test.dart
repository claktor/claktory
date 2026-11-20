import 'package:factory_app/changelog.dart' show ChangelogView;
import 'package:factory_app/name_step.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// A first launch (no name yet) with an update waiting, which brings news.
Workspace _snapshot({String name = ''}) => Workspace(
  agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
  selectedAgentId: 'rivet',
  settings: Settings(userName: name),
  version: 'v0.1.10',
  latestRelease: 'v0.1.11',
  changelog: [
    ChangelogVersion(
      version: 'v0.1.11',
      items: [
        ChangelogItem(
          kind: ChangelogKind.CHANGELOG_KIND_ADDED,
          text: 'Something new.',
        ),
      ],
    ),
  ],
);

void main() {
  testWidgets('over the first-launch question, the update banner still '
      'works', (tester) async {
    final harness = Harness(_snapshot(), named: false);
    harness.appVersion = 'v0.1.10';
    await harness.show(tester);
    await pumpFrames(tester);
    expect(find.byType(NameStep), findsOneWidget);
    // One banner to click, above the step; its place keeps a hidden one.
    expect(find.text('Update').hitTestable(), findsOneWidget);

    await tester.tap(find.text('Update').hitTestable());
    await pumpFrames(tester);
    expect(harness.last.hasInstallUpdate(), isTrue);
  });

  testWidgets("What's new shows instead of the question until it's closed", (
    tester,
  ) async {
    final harness = Harness(_snapshot(), named: false);
    harness.appVersion = 'v0.1.10';
    harness.backend.appliesLayouts = true;
    await harness.show(tester);
    await pumpFrames(tester);
    await tester.tap(find.text("What's new").hitTestable());
    await pumpFrames(tester);
    expect(find.byType(ChangelogView), findsOneWidget);
    expect(find.byType(NameStep), findsNothing);

    // Closed, by its pane's ×: the question again.
    await tester.tap(
      find.byWidgetPredicate((w) => w is GlyphButton && w.label == 'Close'),
    );
    await pumpFrames(tester);
    expect(find.byType(NameStep), findsOneWidget);
  });
}
