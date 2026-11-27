import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';

/// Makes a trackpad's or mouse's pointer on an iPad show hover highlights,
/// as on a Mac, and a finger's touch hide them again.
///
/// Flutter's focus manager switches to touch highlights on a touch but
/// never back on a pointer: on iOS a mouse or trackpad leaves the mode as
/// it was, so after the first touch every row's hover tint stays off
/// (FocusableActionDetector shows it only in traditional mode). This
/// follows the last pointer instead: a mouse or trackpad event holds the
/// traditional mode, a finger or Apple Pencil gives it back to Flutter.
class PointerHighlights {
  PointerHighlights._();

  static var _users = 0;

  /// Starts following the pointer while a window needs it; [release] when
  /// it's gone, the last giving the strategy back to Flutter.
  static void hold() {
    if (_users++ > 0) return;
    GestureBinding.instance.pointerRouter.addGlobalRoute(_saw);
  }

  static void release() {
    if (_users == 0 || --_users > 0) return;
    GestureBinding.instance.pointerRouter.removeGlobalRoute(_saw);
    FocusManager.instance.highlightStrategy = FocusHighlightStrategy.automatic;
  }

  static void _saw(PointerEvent event) {
    final strategy = switch (event.kind) {
      PointerDeviceKind.mouse ||
      PointerDeviceKind.trackpad => FocusHighlightStrategy.alwaysTraditional,
      PointerDeviceKind.touch ||
      PointerDeviceKind.stylus ||
      PointerDeviceKind.invertedStylus => FocusHighlightStrategy.automatic,
      PointerDeviceKind.unknown => null,
    };
    if (strategy == null) return;
    final manager = FocusManager.instance;
    if (manager.highlightStrategy != strategy) {
      manager.highlightStrategy = strategy;
    }
  }
}
