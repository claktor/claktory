// Keep this file plain-test-only: registering testWidgets initializes the
// binding before any test executes and would conceal the Hello regression.
import 'package:factory_app/android/device_identity.dart';
import 'package:flutter/widgets.dart' show WidgetsBinding;
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('identity lookup never requires or initializes a Flutter binding', () {
    expect(() => WidgetsBinding.instance, throwsA(anything));
    final side = androidDisplayShortestSide();
    expect(side.isFinite, isTrue);
    expect(side, greaterThanOrEqualTo(0));
    final identity = AndroidDeviceIdentity.forDisplayShortestSide(side);
    expect(identity.name, isNotEmpty);
    expect(identity.noun, isNotEmpty);
    expect(
      () => WidgetsBinding.instance,
      throwsA(anything),
      reason: 'a device name must not create a binding as a side effect',
    );
  });
}
