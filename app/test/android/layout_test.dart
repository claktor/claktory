import 'package:factory_app/agent_view.dart';
import 'package:factory_app/android/layout.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/message_box.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../phone_sizes_test.dart' show loadGeist;

void main() {
  setUpAll(loadGeist);
  void asAndroid() {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    PhonePlatformPolicy.debugAndroidPolicy = true;
    addTearDown(() {
      debugDefaultTargetPlatformOverride = null;
      PhonePlatformPolicy.debugAndroidPolicy = false;
    });
  }

  for (final (name, size, compact) in const [
    ('upright phone', Size(393, 852), true),
    ('landscape phone', Size(852, 393), true),
    ('real tablet Chrome split', Size(635, 800), true),
    ('just below wide threshold', Size(799, 1000), true),
    ('portrait tablet', Size(800, 1280), false),
    ('landscape tablet', Size(1280, 800), false),
  ]) {
    testWidgets('$name selects ${compact ? 'phone' : 'wide'} composition', (
      tester,
    ) async {
      asAndroid();
      expect(androidPhoneLayout(size, phoneWidth: phoneLayoutWidth), compact);
      late bool actual;
      await tester.pumpWidget(
        MediaQuery(
          data: MediaQueryData(size: size),
          child: Builder(
            builder: (context) {
              actual = isPhoneLayout(context);
              return const SizedBox();
            },
          ),
        ),
      );
      expect(actual, compact, reason: 'the shared shell uses the adapter');
    });
  }

  testWidgets('keyboard does not turn a wide tablet into a phone', (
    tester,
  ) async {
    asAndroid();
    late bool compact;
    await tester.pumpWidget(
      MediaQuery(
        data: const MediaQueryData(
          size: Size(1280, 800),
          viewInsets: EdgeInsets.only(bottom: 370),
        ),
        child: Builder(
          builder: (context) {
            compact = isPhoneLayout(context);
            return const SizedBox();
          },
        ),
      ),
    );
    expect(compact, isFalse);
  });

  testWidgets(
    '635dp split reaches Settings as a page; composer fits on return',
    (tester) async {
      asAndroid();
      final harness = Harness(
        Workspace(
          agents: [Agent(id: 'kai', name: 'Kai', modelAvailable: true)],
          selectedAgentId: 'kai',
        ),
      );
      await harness.show(tester, size: const Size(635, 800));
      await pumpFrames(tester);
      expect(find.byType(PhoneShell), findsOneWidget);
      expect(find.byType(AgentView), findsOneWidget);
      final before = tester.getRect(find.byType(MessageBox));
      expect(before.left, greaterThanOrEqualTo(0));
      expect(before.right, lessThanOrEqualTo(635));

      harness.page.value = AppPage.settings;
      await pumpFrames(tester);
      expect(find.byType(SettingsView), findsOneWidget);
      // A phone page covers the conversation, rather than squeezing it beside
      // Settings as the old 635dp wide shell did.
      expect(find.byType(AgentView), findsNothing);
      expect(tester.takeException(), isNull);

      harness.page.value = AppPage.agent;
      await pumpFrames(tester);
      expect(find.byType(AgentView), findsOneWidget);
      final composer = tester.getRect(find.byType(MessageBox));
      expect(composer.width, before.width);
      expect(composer.right, lessThanOrEqualTo(635));
      expect(tester.takeException(), isNull);
    },
  );
}
