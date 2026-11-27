import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';

/// When the person last touched, clicked or scrolled in the app, pressed a
/// key or brought the app back to the foreground: on a phone or tablet left
/// alone, each Claktory's connection rests, checked every 25 s instead of
/// every few, so the radio can idle between checks, and the next touch
/// checks it at once ([Backend]'s idle).
///
/// Typing on the screen's keyboard isn't seen here (it reaches the app as
/// text, not touches); whatever is sent from it counts, as everything that
/// goes either way does.
class UserInput extends ChangeNotifier {
  UserInput._();

  static final instance = UserInput._();

  /// The last input.
  DateTime last = DateTime.now();

  var _watching = false;

  /// Follows the app's pointers and keys from now on; once is enough.
  void watch() {
    if (_watching) return;
    _watching = true;
    GestureBinding.instance.pointerRouter.addGlobalRoute(_pointer);
    HardwareKeyboard.instance.addHandler(_key);
  }

  void _pointer(PointerEvent event) {
    if (event is PointerDownEvent ||
        event is PointerSignalEvent ||
        event is PointerPanZoomStartEvent) {
      touched();
    }
  }

  bool _key(KeyEvent event) {
    if (event is KeyDownEvent) touched();
    return false;
  }

  /// The person did something.
  void touched() {
    last = DateTime.now();
    notifyListeners();
  }
}
