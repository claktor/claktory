import 'dart:ui';

import 'package:factory_app/android/device_identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart'
    show DeviceKind;
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    'Android phone/tablet names and Hello kinds share the 600dp boundary',
    () {
      for (final side in [1.0, 400.0, 599.9]) {
        final identity = AndroidDeviceIdentity.forDisplayShortestSide(side);
        expect(identity.kind, DeviceKind.DEVICE_KIND_PHONE);
        expect(identity.name, 'Android phone');
        expect(identity.noun, 'phone');
      }
      for (final side in [600.0, 800.0, 1200.0]) {
        final identity = AndroidDeviceIdentity.forDisplayShortestSide(side);
        expect(identity.kind, DeviceKind.DEVICE_KIND_TABLET);
        expect(identity.name, 'Android tablet');
        expect(identity.noun, 'tablet');
      }
    },
  );

  test('missing display/view metrics are a device, not a guessed phone', () {
    for (final side in [0.0, -1.0, double.nan, double.infinity]) {
      final identity = AndroidDeviceIdentity.forDisplayShortestSide(side);
      expect(identity.kind, DeviceKind.DEVICE_KIND_UNSPECIFIED);
      expect(identity.name, 'Android device');
      expect(identity.noun, 'device');
    }
  });

  test(
    'display ratio is applied exactly once, independently of window size',
    () {
      for (final window in [const Size(360, 700), const Size(840, 1200)]) {
        expect(
          androidLogicalShortestSide(
            displaySize: const Size(2400, 3200),
            displayPixelRatio: 2,
            viewSize: window,
            viewPixelRatio: 1,
          ),
          1200,
        );
      }
    },
  );

  test('invalid display metrics fall back to view then unknown device', () {
    for (final ratio in [0.0, -1.0, double.nan, double.infinity]) {
      expect(
        androidLogicalShortestSide(
          displaySize: const Size(2400, 3200),
          displayPixelRatio: ratio,
          viewSize: const Size(1200, 2000),
          viewPixelRatio: 3,
        ),
        400,
      );
    }
    expect(
      androidLogicalShortestSide(
        displaySize: Size.zero,
        displayPixelRatio: 1,
        viewSize: Size.zero,
        viewPixelRatio: 0,
      ),
      0,
    );
  });

  testWidgets('resizing a tablet window does not turn it into a phone', (
    tester,
  ) async {
    tester.view.display.size = const Size(1600, 2560);
    tester.view.display.devicePixelRatio = 2;
    addTearDown(tester.view.display.reset);
    addTearDown(tester.view.reset);
    for (final width in [360.0, 800.0, 1100.0]) {
      tester.view.physicalSize = Size(width * 2, 1000);
      final identity = AndroidDeviceIdentity.forDisplayShortestSide(
        androidDisplayShortestSide(),
      );
      expect(identity, same(AndroidDeviceIdentity.tablet));
    }
  });
}
