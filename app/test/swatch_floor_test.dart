// Color swatches (an agent's color, the avatar's) meet the platform's touch
// floor: 26 on a Mac, iOS's 44pt on an iPhone, at least Android's 48dp; and
// the Claktory name field reads as itself.
import 'package:factory_app/forms.dart' show ColorPicker;
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Future<Size> _swatch(WidgetTester tester, Size size) async {
  final harness = Harness(Workspace());
  await harness.show(tester, size: size);
  await tester.pumpWidget(
    WidgetsApp(
      color: const Color(0xFF000000),
      builder: (context, _) => Center(
        child: ColorPicker(value: 'blue', onChanged: (_) {}),
      ),
    ),
  );
  await pumpFrames(tester);
  return tester.getSize(find.byKey(const ValueKey('color-blue')));
}

void main() {
  testWidgets('a color swatch is 26 on a Mac, 44 on an iPhone, at least 48 '
      'on Android, phone or tablet', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    expect(await _swatch(tester, const Size(1200, 900)), const Size(26, 26));

    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    expect(await _swatch(tester, const Size(393, 852)), const Size(44, 44));

    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    PhonePlatformPolicy.debugAndroidPolicy = true;
    addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
    for (final size in [const Size(393, 852), const Size(1200, 900)]) {
      final swatch = await _swatch(tester, size);
      expect(swatch.width, greaterThanOrEqualTo(48), reason: '$size');
      expect(swatch.height, greaterThanOrEqualTo(48), reason: '$size');
    }
  });

  testWidgets("the Claktory's name field reads as Claktory name, typed in "
      'or not', (tester) async {
    final harness = Harness(
      Workspace(
        agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
        selectedAgentId: 'rivet',
        remote: RemoteAccess(machine: 'Mac mini'),
      ),
    );
    harness.page.value = AppPage.settings;
    harness.backend.settingsSection.value = 'general';
    await harness.show(tester, size: const Size(1400, 1000));
    await pumpFrames(tester);
    expect(find.byType(SettingsView), findsOneWidget);
    expect(find.bySemanticsLabel('Claktory name'), findsOneWidget);
    await tester.enterText(
      find.descendant(
        of: find.bySemanticsLabel('Claktory name'),
        matching: find.byType(EditableText),
      ),
      'Home',
    );
    await pumpFrames(tester);
    expect(find.bySemanticsLabel('Claktory name'), findsOneWidget);
  });
}
