import 'dart:ui' show FlutterView, PlatformDispatcher;

import 'package:flutter/widgets.dart' show Size, WidgetsBinding;

import '../protocol/factory/v1/factory.pb.dart' show DeviceKind;

/// Non-identifying Android names, without model/serial or another permission.
/// This describes the display, not the current split-screen window width.
final class AndroidDeviceIdentity {
  const AndroidDeviceIdentity._(this.kind, this.name, this.noun);

  static const unknown = AndroidDeviceIdentity._(
    DeviceKind.DEVICE_KIND_UNSPECIFIED,
    'Android device',
    'device',
  );
  static const phone = AndroidDeviceIdentity._(
    DeviceKind.DEVICE_KIND_PHONE,
    'Android phone',
    'phone',
  );
  static const tablet = AndroidDeviceIdentity._(
    DeviceKind.DEVICE_KIND_TABLET,
    'Android tablet',
    'tablet',
  );

  factory AndroidDeviceIdentity.forDisplayShortestSide(double side) =>
      !side.isFinite || side <= 0
      ? unknown
      : side >= 600
      ? tablet
      : phone;

  final DeviceKind kind;
  final String name;
  final String noun;
}

/// A physical display's logical size; a window resize must not rename a device.
/// A missing/invalid display falls back to the view, then an unknown device.
/// Nothing is fetched or persisted at import time (Apple builds import this too).
double androidDisplayShortestSide({FlutterView? view}) {
  // Hello can ask before runApp, including from a plain test with no binding.
  // Naming is best-effort: it must never stop authentication or a connection.
  try {
    final selected = view ?? _identityView();
    if (selected == null) return 0;
    var displaySize = Size.zero;
    var displayRatio = 0.0;
    try {
      displaySize = selected.display.size;
      displayRatio = selected.display.devicePixelRatio;
    } on Object {
      // A host without display metrics may still have a useful view size.
    }
    return androidLogicalShortestSide(
      displaySize: displaySize,
      displayPixelRatio: displayRatio,
      viewSize: selected.physicalSize,
      viewPixelRatio: selected.devicePixelRatio,
    );
  } on Object {
    return 0; // Unknown, not a failed Hello or a guessed phone.
  }
}

FlutterView? _identityView() {
  try {
    // The binding supplies mutable TestFlutterViews when one exists.
    return WidgetsBinding.instance.platformDispatcher.views.firstOrNull;
  } on Object {
    // No binding (or no usable binding): don't initialize one just for a name.
    return PlatformDispatcher.instance.views.firstOrNull;
  }
}

/// Unit conversion with an explicit fallback for hosts with no display metrics.
double androidLogicalShortestSide({
  required Size displaySize,
  required double displayPixelRatio,
  required Size viewSize,
  required double viewPixelRatio,
}) {
  double? logical(Size size, double ratio) {
    if (!ratio.isFinite ||
        ratio <= 0 ||
        !size.width.isFinite ||
        !size.height.isFinite ||
        size.width <= 0 ||
        size.height <= 0) {
      return null;
    }
    return size.shortestSide / ratio;
  }

  return logical(displaySize, displayPixelRatio) ??
      logical(viewSize, viewPixelRatio) ??
      0;
}
