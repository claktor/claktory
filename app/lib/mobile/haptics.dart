import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// The iPhone's taps you feel, with Apple's meanings (Human Interface
/// Guidelines, "Playing haptics"): short, for something the person did
/// deliberately and can see happen. Never for streamed text or an ordinary
/// tap. Nothing elsewhere.
abstract final class Haptics {
  static bool get _here => defaultTargetPlatform == TargetPlatform.iOS;

  /// A deliberate choice changed: another Claktory or agent, a context
  /// menu opening.
  static Future<void> selection() async {
    if (_here) await HapticFeedback.selectionClick();
  }

  /// Something small took hold: a sheet snapping, a slide passing Cancel.
  static Future<void> light() async {
    if (_here) await HapticFeedback.lightImpact();
  }

  /// Something began that the person holds: a recording starting.
  static Future<void> impact() async {
    if (_here) await HapticFeedback.mediumImpact();
  }

  /// Something the person waited for went through: a message sent.
  static Future<void> success() async {
    if (_here) await HapticFeedback.successNotification();
  }

  /// Something is about to be lost, or was refused.
  static Future<void> warning() async {
    if (_here) await HapticFeedback.warningNotification();
  }
}
