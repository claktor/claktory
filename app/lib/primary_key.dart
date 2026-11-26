import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

/// Whether this platform's shortcuts are held with ⌘ (Apple's) rather than
/// Ctrl (Windows, Linux, Android with a keyboard), as docs/IDENTITY.md has
/// them: the same shortcuts, with each system's own modifier.
bool get primaryIsMeta =>
    defaultTargetPlatform == TargetPlatform.macOS ||
    defaultTargetPlatform == TargetPlatform.iOS;

/// [key] with this platform's primary modifier (⌘ or Ctrl), and Shift if
/// [shift].
SingleActivator primaryKey(LogicalKeyboardKey key, {bool shift = false}) =>
    SingleActivator(
      key,
      meta: primaryIsMeta,
      control: !primaryIsMeta,
      shift: shift,
    );

/// Whether the primary modifier (⌘ or Ctrl) is held now: ⌘-scroll on a Mac,
/// Ctrl-scroll on Windows, zooms.
bool get primaryPressed => primaryIsMeta
    ? HardwareKeyboard.instance.isMetaPressed
    : HardwareKeyboard.instance.isControlPressed;
