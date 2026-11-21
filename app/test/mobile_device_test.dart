// Two axes (docs/ipad-workbench.md §1): isPhoneLayout is the window's
// composition (the phone's routes or the panes), touchFirst the finger's
// sizes and behavior; deviceIdiom comes from the native side, never from
// the window. On every iPhone and Mac the two agree.
import 'dart:convert';
import 'dart:io';

import 'package:factory_app/identity.dart';
import 'package:factory_app/mobile/device.dart';
import 'package:factory_app/mobile/metrics.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  /// Runs the test as [platform], undone before its checks end.
  void on(TargetPlatform platform) {
    debugDefaultTargetPlatformOverride = platform;
    addTearDown(() {
      debugDefaultTargetPlatformOverride = null;
      debugDeviceCapabilities = null;
      PhonePlatformPolicy.debugAndroidPolicy = false;
    });
  }

  /// What [isPhoneLayout], [touchFirst], [Metrics] and [TypeScale] say in a
  /// window of [size] with [safe] around it.
  Future<({bool phone, bool touch, double target, double body})> ask(
    WidgetTester tester,
    Size size, {
    EdgeInsets safe = EdgeInsets.zero,
  }) async {
    late ({bool phone, bool touch, double target, double body}) said;
    await tester.pumpWidget(
      MediaQuery(
        data: MediaQueryData(size: size, padding: safe, viewPadding: safe),
        child: Builder(
          builder: (context) {
            said = (
              phone: isPhoneLayout(context),
              touch: touchFirst(context),
              target: Metrics.of(context).target,
              body: TypeScale.of(context).body.fontSize!,
            );
            return const SizedBox();
          },
        ),
      ),
    );
    return said;
  }

  void as(TargetPlatform platform, Idiom idiom, {bool onMac = false}) {
    on(platform);
    debugDeviceCapabilities = DeviceCapabilities(
      idiom: idiom,
      iosAppOnMac: onMac,
    );
  }

  testWidgets('an iPhone: the phone, upright or on its side, and touch', (
    tester,
  ) async {
    as(TargetPlatform.iOS, Idiom.phone);
    for (final size in const [Size(390, 844), Size(932, 430)]) {
      final said = await ask(tester, size);
      expect(said.phone, isTrue, reason: '$size');
      expect(said.touch, isTrue, reason: '$size');
      expect(said.target, 44);
      expect(said.body, 17);
    }
  });

  testWidgets("a Mac's window: the panes and a pointer's sizes, the phone's "
      'only under 600 wide', (tester) async {
    on(TargetPlatform.macOS);
    var said = await ask(tester, const Size(1200, 800));
    expect((said.phone, said.touch, said.target), (false, false, 28));
    said = await ask(tester, const Size(500, 800));
    expect((said.phone, said.touch, said.target), (true, true, 44));
  });

  testWidgets("an iPad: the panes with a finger's sizes when it's wide and "
      'tall enough; the phone when narrow or short', (tester) async {
    as(TargetPlatform.iOS, Idiom.pad);
    var said = await ask(tester, const Size(1366, 1024));
    expect(said.phone, isFalse);
    expect(said.touch, isTrue);
    expect(said.target, 44);
    expect(said.body, 17);

    // Stage Manager's short window is a short Mac-like window, not a phone
    // on its side.
    said = await ask(tester, const Size(1000, 560));
    expect(said.phone, isFalse);
    // Its safe area counts: under 440 of room, the phone's routes.
    said = await ask(
      tester,
      const Size(1000, 470),
      safe: const EdgeInsets.only(top: 24, bottom: 20),
    );
    expect(said.phone, isTrue);
    // Split View's third.
    said = await ask(tester, const Size(320, 1024));
    expect(said.phone, isTrue);
  });

  testWidgets("the iPad app on a Mac has a pointer: a Mac's sizes", (
    tester,
  ) async {
    as(TargetPlatform.iOS, Idiom.pad, onMac: true);
    final said = await ask(tester, const Size(1366, 1024));
    expect((said.phone, said.touch, said.target), (false, false, 28));
  });

  testWidgets("Android, where it's asked for: a wide tablet with a finger's "
      '48', (tester) async {
    PhonePlatformPolicy.debugAndroidPolicy = true;
    as(TargetPlatform.android, Idiom.pad);
    final said = await ask(tester, const Size(1280, 800));
    expect((said.phone, said.touch, said.target), (false, true, 48));
  });

  test("the native side's answer, as the fixtures have it", () {
    DeviceCapabilities read(String name) => DeviceCapabilities.fromMap(
      jsonDecode(
        File('test/ipad/fixtures/bridge/$name.json').readAsStringSync(),
      ) as Map<String, Object?>,
    );
    final pad = read('capabilities_pad');
    expect(pad.idiom, Idiom.pad);
    expect(pad.iosAppOnMac, isFalse);
    expect(pad.hardwareKeyboard, isFalse);
    expect(pad.osVersion, '27.2');
    expect((pad.raw['pencil']! as Map)['hover'], isTrue);
    expect(read('capabilities_pad_ios15').hardwareKeyboard, isTrue);
    expect(read('capabilities_phone').idiom, Idiom.phone);
    expect(
      DeviceCapabilities.fromMap(const {'idiom': 'vision'}).idiom,
      Idiom.other,
    );
  });
}
