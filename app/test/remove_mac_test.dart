import 'package:factory_app/controls.dart' show Toggle;
import 'package:factory_app/remove_mac.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart' show SettingsView;
import 'package:factory_app/setup.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'setup_test.dart' show FakeSetupChannel;

void main() {
  late FakeSetupChannel channel;

  setUp(() {
    channel = FakeSetupChannel();
    removalChannel = channel;
    removable = true;
  });
  tearDown(() {
    removalChannel = const SetupChannel();
    removable = null;
  });

  Future<Harness> openRemove(WidgetTester tester) async {
    final harness = Harness(workspace());
    await harness.show(tester, size: const Size(1200, 1100));
    harness.page.value = AppPage.settings;
    await settle(tester);
    await tester.scrollUntilVisible(
      find.text('Remove Claktory from this Mac…'),
      300,
      scrollable: find
          .descendant(
            of: find.byType(SettingsView),
            matching: find.byType(Scrollable),
          )
          .first,
    );
    await settle(tester);
    expect(find.text('This Mac'), findsOneWidget);
    await tester.tap(find.text('Remove Claktory from this Mac…'));
    await settle(tester);
    expect(find.text('Remove Claktory from this Mac?'), findsOneWidget);
    return harness;
  }

  testWidgets('removing keeps the data unless asked, then the app goes', (
    tester,
  ) async {
    channel.results['uninstall'] = const InstallResult(result: 'removed');
    await openRemove(tester);
    await tester.tap(find.text('Remove Claktory'));
    await settle(tester);
    expect(channel.runs, ['uninstall']);
    expect(channel.removed, [false], reason: 'it keeps what it knows');
  });

  testWidgets('deleting the data too asks again, naming what goes', (
    tester,
  ) async {
    channel.results['uninstall --dry-run --delete-data'] = InstallResult.parse(
      5,
      '{"result":"dry_run","items":['
          '{"what":"conversations","bytes":2000000000,"complete":true},'
          '{"what":"checkouts","bytes":400000000,"complete":false},'
          '{"what":"backend","bytes":150000000,"complete":true}]}',
      '',
    );
    channel.results['uninstall --delete-data'] = const InstallResult(
      result: 'removed',
    );
    await openRemove(tester);
    await tester.tap(
      find.descendant(
        of: find.byType(RemoveForm),
        matching: find.byType(Toggle),
      ),
    );
    await settle(tester);
    await tester.tap(find.text('Continue…'));
    await settle(tester);
    expect(find.text('Delete all your Claktory data?'), findsOneWidget);
    expect(find.textContaining('This deletes 1 agent with'), findsOneWidget);
    expect(find.textContaining('(at least 2.4 GB)'), findsOneWidget);
    expect(channel.runs, isNot(contains('uninstall --delete-data')));

    await tester.tap(find.text('Delete and remove'));
    await settle(tester);
    expect(channel.runs.last, 'uninstall --delete-data');
    expect(channel.removed, [true], reason: "and forgets what it kept");
  });

  testWidgets('a failure says why and stays; an old backend says to update', (
    tester,
  ) async {
    channel.results['uninstall'] = const InstallResult(
      result: 'partly_removed',
      reason: "Claktory stopped, but its folder couldn't be removed.",
      detail: '~/.factory/bin: permission denied',
    );
    await openRemove(tester);
    await tester.tap(find.text('Remove Claktory'));
    await settle(tester);
    expect(
      find.text("Claktory stopped, but its folder couldn't be removed."),
      findsOneWidget,
    );
    expect(channel.removed, isEmpty);
    await tester.tap(find.text('Show details'));
    await settle(tester);
    expect(find.text('~/.factory/bin: permission denied'), findsOneWidget);

    channel.results['uninstall'] = InstallResult.parse(2, '', 'unknown');
    await tester.tap(find.text('Try again'));
    await settle(tester);
    expect(
      find.text(
        "This version of Claktory can't remove itself. Update Claktory, "
        'then try again.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('the switch turns over from its label too, once a click', (
    tester,
  ) async {
    await openRemove(tester);
    Toggle toggle() => tester.widget<Toggle>(
      find.descendant(
        of: find.byType(RemoveForm),
        matching: find.byType(Toggle),
      ),
    );
    await tester.tap(find.text('Also delete your data'));
    await settle(tester);
    expect(toggle().value, isTrue);
    expect(find.text('Continue…'), findsOneWidget);
    // On the switch itself: once, not twice.
    await tester.tap(
      find.descendant(
        of: find.byType(RemoveForm),
        matching: find.byType(Toggle),
      ),
    );
    await settle(tester);
    expect(toggle().value, isFalse);
    await tester.tap(find.textContaining('Unless you delete it'));
    await settle(tester);
    expect(toggle().value, isTrue);
  });

  testWidgets('only for this Mac, not elsewhere', (tester) async {
    removable = false;
    final harness = Harness(workspace());
    await harness.show(tester, size: const Size(1200, 1100));
    harness.page.value = AppPage.settings;
    await settle(tester);
    expect(find.text('This Mac', skipOffstage: false), findsNothing);
  });
}
