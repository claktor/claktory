// Settings' section layout:
// the heading names the Claktory, never "This Mac"; a section's title and
// its labels share one left edge; help sits under its control, the label
// on the control's first line; This device's row a hairline apart; and a
// phone's or an iPad's own version with the store it came from.
import 'package:factory_app/app_source.dart';
import 'package:factory_app/connections.dart' show RemoteFactory;
import 'package:factory_app/controls.dart' show Button, TextField;
import 'package:factory_app/device_account.dart' show AccountClaktory;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';

Workspace _workspace() => workspace()
  ..settings.name = ''
  ..remote = RemoteAccess(machine: 'Mac Studio');

Future<Harness> _open(
  WidgetTester tester,
  String section, {
  Harness? harness,
  Size size = const Size(1200, 900),
  TargetPlatform platform = TargetPlatform.macOS,
}) async {
  debugDefaultTargetPlatformOverride = platform;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  harness ??= Harness(_workspace());
  await harness.show(tester, size: size);
  harness.page.value = AppPage.settings;
  await pumpFrames(tester);
  harness.backend.settingsSection.value = section;
  await pumpFrames(tester);
  return harness;
}

Finder _inSection(String id, Finder finder) => find.descendant(
  of: find.byKey(ValueKey('settings-section:$id')),
  matching: finder,
);

void main() {
  testWidgets("the index's heading names the Claktory by its computer while "
      "it has no name, never This Mac, which is the device's row", (
    tester,
  ) async {
    await _open(tester, 'general');
    expect(find.text('Mac Studio'), findsWidgets);
    expect(find.text('This Mac'), findsOneWidget, reason: 'its own row only');
  });

  testWidgets("a section's title starts where its labels do, not centred", (
    tester,
  ) async {
    await _open(tester, 'general');
    final title = tester.getTopLeft(
      _inSection('general', find.text('General')),
    );
    final label = tester.getTopLeft(
      _inSection('general', find.text('Claktory name')),
    );
    expect(title.dx, label.dx);
  });

  testWidgets("help sits under its control, and the label on the control's "
      'first line', (tester) async {
    await _open(tester, 'general');
    final field = tester.getRect(
      _inSection('general', find.byType(TextField)).first,
    );
    final help = tester.getRect(
      find.text('Shown on every device connected to this Claktory.'),
    );
    expect(help.top, greaterThanOrEqualTo(field.bottom));
    expect(help.left, field.left);
    // On the field's line: its text and the label's start together.
    final label = tester.getRect(find.text('Claktory name'));
    final text = tester.getRect(
      _inSection('general', find.byType(EditableText)).first,
    );
    expect((label.top - text.top).abs(), lessThan(4));
  });

  testWidgets("Profile's name: its help under the field", (tester) async {
    await _open(tester, 'profile');
    final name = tester.getRect(
      _inSection('profile', find.byType(EditableText)).first,
    );
    expect(
      tester.getRect(find.text('Your agents call you by it.')).top,
      greaterThanOrEqualTo(name.bottom),
    );
  });

  testWidgets("This device's row is a hairline apart from the Claktory's", (
    tester,
  ) async {
    await _open(tester, 'general');
    final display = tester.getRect(
      find.byKey(const ValueKey('settings-row:display')),
    );
    final device = tester.getRect(
      find.byKey(const ValueKey('settings-row:device')),
    );
    final hairlines = find.byWidgetPredicate((widget) {
      if (widget is! Container || widget.constraints?.maxHeight != 1) {
        return false;
      }
      return true;
    });
    final between = hairlines.evaluate().where((element) {
      final box = element.renderObject! as RenderBox;
      final at = box.localToGlobal(Offset.zero);
      return at.dx < display.right &&
          at.dy > display.bottom &&
          at.dy < device.top;
    });
    expect(between, hasLength(1));
  });

  group("a phone's own version, with the store it came from", () {
    Future<Harness> device(WidgetTester tester) async {
      final harness = Harness(_workspace(), localBackend: false)
        ..appVersion = '0.1.23';
      return _open(
        tester,
        'device',
        harness: harness,
        platform: TargetPlatform.iOS,
      );
    }

    testWidgets('from TestFlight: TestFlight, opening its invitation', (
      tester,
    ) async {
      debugAppSource = AppSource.testFlight;
      addTearDown(() => debugAppSource = null);
      final harness = await device(tester);
      expect(find.text('v0.1.23'), findsWidgets);
      final button = find.byWidgetPredicate(
        (widget) => widget is Button && widget.label == 'TestFlight',
      );
      await tester.tap(button);
      await pumpFrames(tester);
      expect(harness.opened, [testFlightUrl]);
    });

    testWidgets('from the App Store: App Store', (tester) async {
      debugAppSource = AppSource.appStore;
      addTearDown(() => debugAppSource = null);
      await device(tester);
      expect(
        find.byWidgetPredicate(
          (widget) => widget is Button && widget.label == 'App Store',
        ),
        findsOneWidget,
      );
    });

    testWidgets("an iPad's wide Settings: This iPad starts with the device's "
        'own Claktory account, then its version (Lucy 50427)', (tester) async {
      final harness = Harness(_workspace(), localBackend: false)
        ..appVersion = '0.1.23';
      await harness.show(tester, size: const Size(1194, 834));
      harness.connections.use(
        RemoteFactory.fromAccount(
          AccountClaktory(id: 's', name: 'Mac Studio', endpointId: 'e' * 64),
        ),
      );
      harness.backend.workspace.apply(_workspace());
      await pumpFrames(tester);
      await _open(
        tester,
        'device',
        harness: harness,
        platform: TargetPlatform.iOS,
        size: const Size(1194, 834),
      );
      final section = find.byKey(const ValueKey('settings-section:device'));
      expect(section, findsOneWidget, reason: 'the wide layout');
      final account = tester.getTopLeft(
        find.descendant(of: section, matching: find.text('Claktory account')),
      );
      final version = tester.getTopLeft(
        find.descendant(of: section, matching: find.text('Version')),
      );
      expect(account.dy, lessThan(version.dy));
    });

    testWidgets('a dev build, from no store: the version alone', (
      tester,
    ) async {
      await device(tester);
      expect(find.text('v0.1.23'), findsWidgets);
      for (final label in ['App Store', 'TestFlight', 'Google Play']) {
        expect(
          find.byWidgetPredicate(
            (widget) => widget is Button && widget.label == label,
          ),
          findsNothing,
        );
      }
    });
  });
}
