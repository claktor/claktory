import 'package:factory_app/changelog.dart' show ChangelogView;
import 'package:factory_app/name_step.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/update.dart' show UpdateBanner;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' show EditableText, Size;
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

  testWidgets('on a Mac, VoiceOver behind the step reaches only the update '
      'banner', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_snapshot(), named: false);
    harness.appVersion = 'v0.1.10';
    final semantics = tester.ensureSemantics();
    await harness.show(tester, size: const Size(1200, 900));
    await pumpFrames(tester);
    for (final n in find.semantics.byPredicate((n) => true).evaluate()) {
      debugPrint('YYY ${n.label} ${n.rect} hidden=${n.flagsCollection}');
    }
    expect(find.byType(NameStep), findsOneWidget);
    expect(find.semantics.byLabel("What's your name?"), findsOneWidget);
    expect(find.semantics.byLabel(RegExp('Update')), findsWidgets);
    // The sidebar's agents and the conversation behind are silent.
    expect(find.semantics.byLabel(RegExp('Rivet')), findsNothing);
    for (final n
        in find.semantics.byPredicate((n) => n.label.isNotEmpty).evaluate()) {
      debugPrint('XXX ${n.label}');
    }
    semantics.dispose();
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

  testWidgets('on an iPad, over the first-launch question, the banner stays a '
      'strip at the top and the question stays usable (Zucchero row 635)', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_snapshot(), named: false);
    harness.appVersion = 'v0.1.10';
    await harness.show(tester, size: const Size(1180, 820));
    await pumpFrames(tester);
    expect(find.byType(NameStep), findsOneWidget);
    final update = find.text('Update').hitTestable();
    expect(update, findsOneWidget);
    final banner = find.ancestor(
      of: update,
      matching: find.byType(UpdateBanner),
    );
    expect(tester.getSize(banner).height, lessThan(120));
    expect(tester.getTopLeft(banner).dy, 0);
    // The question's field is reachable under it.
    expect(find.byType(EditableText).hitTestable(), findsOneWidget);
  });
}
