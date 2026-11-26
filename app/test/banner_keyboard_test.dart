// Sale's row 22: a phone on its side, typing, with "Sign in to your
// Claktory account in this app by …" showing. The banner steps aside while the keyboard leaves
// little room, so the message box stays in sight, and comes back when it's
// down. Upright on a big phone, with room, it stays.
import 'package:factory_app/message_box.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _banner = find.textContaining('in this app by ');

Future<Harness> _phone(WidgetTester tester, Size size) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(
    Workspace(
      agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
      selectedAgentId: 'rivet',
    ),
  );
  await harness.show(tester, size: size);
  harness.backend.confirmBy.value = DateTime.now().add(
    const Duration(days: 30),
  );
  await pumpFrames(tester);
  expect(_banner, findsOneWidget);
  return harness;
}

void _keyboard(WidgetTester tester, double height) {
  tester.view.viewInsets = FakeViewPadding(bottom: height);
  addTearDown(tester.view.resetViewInsets);
}

void main() {
  testWidgets('on its side, typing: the banner steps aside, the message box '
      'shows, and the banner is back once the keyboard is down', (
    tester,
  ) async {
    await _phone(tester, const Size(852, 393));
    _keyboard(tester, 200);
    await pumpFrames(tester);
    expect(_banner.hitTestable(), findsNothing);
    expect(tester.takeException(), isNull);
    final box = tester.getRect(find.byType(MessageBox).first);
    expect(box.height, greaterThan(0));
    expect(box.bottom, lessThanOrEqualTo(393 - 200 + 0.5));
    tester.view.resetViewInsets();
    await pumpFrames(tester);
    expect(_banner.hitTestable(), findsOneWidget);
  });

  testWidgets('upright on a big phone, with room, it stays', (tester) async {
    await _phone(tester, const Size(393, 852));
    _keyboard(tester, 336);
    await pumpFrames(tester);
    expect(_banner.hitTestable(), findsOneWidget);
  });
}
