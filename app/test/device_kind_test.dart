import 'package:factory_app/device_kind.dart';
import 'package:factory_app/devices.dart' as devices;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart' show Size;
import 'package:flutter_test/flutter_test.dart';

/// A Mac whose `pmset -g batt` and `sysctl -n hw.model` answer these.
CommandRunner mac({String? battery, String? model}) =>
    (command, args) async => switch (command) {
      '/usr/bin/pmset' => battery,
      '/usr/sbin/sysctl' => model,
      _ => null,
    };

const _laptopBattery =
    "Now drawing from 'Battery Power'\n"
    ' -InternalBattery-0 (id=4653155)\t87%; discharging; 6:12 remaining '
    'present: true\n';
const _desktopBattery = "Now drawing from 'AC Power'\n";

void main() {
  test('a Mac with a built-in battery is a laptop', () async {
    expect(
      await deviceKindOf(
        TargetPlatform.macOS,
        run: mac(battery: _laptopBattery, model: 'Mac15,6'),
      ),
      DeviceKind.DEVICE_KIND_LAPTOP,
    );
  });

  test('a MacBook is a laptop even when the battery says nothing', () async {
    expect(
      await deviceKindOf(
        TargetPlatform.macOS,
        run: mac(model: 'MacBookPro18,3\n'),
      ),
      DeviceKind.DEVICE_KIND_LAPTOP,
    );
  });

  test('a Mac Studio, mini or iMac is a desktop', () async {
    for (final model in ['Mac14,14', 'Macmini9,1', 'iMac21,1']) {
      expect(
        await deviceKindOf(
          TargetPlatform.macOS,
          run: mac(battery: _desktopBattery, model: model),
        ),
        DeviceKind.DEVICE_KIND_DESKTOP,
        reason: model,
      );
    }
  });

  test("a Mac that can't tell is a desktop", () async {
    expect(
      await deviceKindOf(TargetPlatform.macOS, run: mac()),
      DeviceKind.DEVICE_KIND_DESKTOP,
    );
  });

  test('an iPhone is a phone and an iPad a tablet', () async {
    for (final (side, kind) in [
      (375.0, DeviceKind.DEVICE_KIND_PHONE),
      (440.0, DeviceKind.DEVICE_KIND_PHONE),
      (744.0, DeviceKind.DEVICE_KIND_TABLET),
      (1024.0, DeviceKind.DEVICE_KIND_TABLET),
    ]) {
      expect(
        await deviceKindOf(TargetPlatform.iOS, shortestSide: side),
        kind,
        reason: '$side',
      );
    }
  });

  test(
    'Android names and Hello kinds agree at the display-size boundary',
    () async {
      for (final (side, kind, name) in [
        (400.0, DeviceKind.DEVICE_KIND_PHONE, 'Android phone'),
        (800.0, DeviceKind.DEVICE_KIND_TABLET, 'Android tablet'),
      ]) {
        expect(
          await deviceKindOf(TargetPlatform.android, shortestSide: side),
          kind,
        );
        expect(
          await deviceNameOf(TargetPlatform.android, shortestSide: side),
          name,
        );
      }
    },
  );

  testWidgets("Android's word for the device follows its display, not a "
      'resized window', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    addTearDown(tester.view.display.reset);
    addTearDown(tester.view.reset);
    tester.view.devicePixelRatio = 2;
    tester.view.display.devicePixelRatio = 2;
    // A phone's display, in a window as wide as a tablet's.
    tester.view.display.size = const Size(800, 1600);
    tester.view.physicalSize = const Size(2000, 2000);
    expect(devices.thisDeviceKind, 'phone');
    // A tablet's display, split down to a phone-sized window.
    tester.view.display.size = const Size(1600, 2400);
    tester.view.physicalSize = const Size(700, 2400);
    expect(devices.thisDeviceKind, 'tablet');
  });

  test("a Mac's name is its Computer Name; an iPhone's, iPhone", () async {
    expect(
      await deviceNameOf(
        TargetPlatform.macOS,
        run: (command, args) async =>
            command == '/usr/sbin/scutil' && args.last == 'ComputerName'
            ? "Matt's MacBook Pro\n"
            : null,
      ),
      "Matt's MacBook Pro",
    );
    expect(await deviceNameOf(TargetPlatform.iOS, shortestSide: 393), 'iPhone');
    expect(await deviceNameOf(TargetPlatform.iOS, shortestSide: 820), 'iPad');
  });
}
