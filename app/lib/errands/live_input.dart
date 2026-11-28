import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';

// What the person does in a tab of an agent's browser handed to them to
// solve a check, as the runner takes it: pointer and wheel events where
// they land on the frame, keys by their W3C names, and text. Only in that
// tab, for that request; the backend enforces it, and a view that only
// watches sends nothing. Nothing goes to or from the device's clipboard.

/// Which button a pointer event is for.
enum LiveButton { left, middle, right }

/// A pointer going down, moving or coming up.
enum LivePointerKind { down, move, up }

/// One thing the person did in the view.
sealed class LiveEvent {
  const LiveEvent();
}

/// A pointer at [x], [y]: fractions (0 to 1) of the frame, so a frame drawn
/// smaller than the screen, or a screen just resized, can't misplace it.
final class LivePointer extends LiveEvent {
  const LivePointer(this.kind, this.x, this.y, {this.button = LiveButton.left});

  final LivePointerKind kind;
  final double x;
  final double y;
  final LiveButton button;

  @override
  bool operator ==(Object other) =>
      other is LivePointer &&
      other.kind == kind &&
      other.x == x &&
      other.y == y &&
      other.button == button;

  @override
  int get hashCode => Object.hash(kind, x, y, button);

  @override
  String toString() => 'LivePointer($kind, $x, $y, $button)';
}

/// A scroll of [dx], [dy] screen pixels at [x], [y] (fractions of the
/// frame).
final class LiveWheel extends LiveEvent {
  const LiveWheel(this.dx, this.dy, this.x, this.y);

  final double dx;
  final double dy;
  final double x;
  final double y;

  @override
  bool operator ==(Object other) =>
      other is LiveWheel &&
      other.dx == dx &&
      other.dy == dy &&
      other.x == x &&
      other.y == y;

  @override
  int get hashCode => Object.hash(dx, dy, x, y);

  @override
  String toString() => 'LiveWheel($dx, $dy at $x, $y)';
}

/// A key going [down] or up: [key] is its W3C `key` name ("Enter",
/// "ArrowLeft", "a"), with the modifiers held.
final class LiveKey extends LiveEvent {
  const LiveKey(
    this.key, {
    required this.down,
    this.control = false,
    this.shift = false,
    this.alt = false,
    this.meta = false,
  });

  final String key;
  final bool down;
  final bool control;
  final bool shift;
  final bool alt;
  final bool meta;

  @override
  bool operator ==(Object other) =>
      other is LiveKey &&
      other.key == key &&
      other.down == down &&
      other.control == control &&
      other.shift == shift &&
      other.alt == alt &&
      other.meta == meta;

  @override
  int get hashCode => Object.hash(key, down, control, shift, alt, meta);

  @override
  String toString() =>
      'LiveKey($key ${down ? 'down' : 'up'}'
      '${control ? ' ctrl' : ''}${shift ? ' shift' : ''}'
      '${alt ? ' alt' : ''}${meta ? ' meta' : ''})';
}

/// Text typed: what a key with a character, or a phone's keyboard, gives.
final class LiveTyped extends LiveEvent {
  const LiveTyped(this.text);

  final String text;

  @override
  bool operator ==(Object other) => other is LiveTyped && other.text == text;

  @override
  int get hashCode => text.hashCode;

  @override
  String toString() => 'LiveTyped($text)';
}

/// The most text one message carries (the backend's cap, R5): longer
/// text goes in pieces.
const liveTextMost = 4096;

/// [text] in pieces of at most [liveTextMost] bytes of UTF-8, never
/// splitting a character.
List<String> liveTextPieces(String text) {
  final pieces = <String>[];
  var piece = StringBuffer();
  var bytes = 0;
  for (final rune in text.runes) {
    final char = String.fromCharCode(rune);
    final size = rune < 0x80
        ? 1
        : rune < 0x800
        ? 2
        : rune < 0x10000
        ? 3
        : 4;
    if (bytes + size > liveTextMost) {
      pieces.add(piece.toString());
      piece = StringBuffer();
      bytes = 0;
    }
    piece.write(char);
    bytes += size;
  }
  if (piece.isNotEmpty) pieces.add(piece.toString());
  return pieces;
}

/// Where a frame of [frame]'s size is drawn in a box of [box]'s size: as
/// large as it fits, placed by [alignment], never stretched, and never
/// larger than [maxScale] times its own size.
Rect liveFrameRect(
  Size box,
  Size frame, {
  double maxScale = double.infinity,
  Alignment alignment = Alignment.center,
}) {
  if (frame.isEmpty || box.isEmpty) return Rect.zero;
  var fitted = applyBoxFit(BoxFit.contain, frame, box).destination;
  if (fitted.width > frame.width * maxScale) fitted = frame * maxScale;
  return alignment.inscribe(fitted, Offset.zero & box);
}

/// Where [local] (in a box of [box]'s size) falls on a frame of [frame]'s
/// size drawn there ([liveFrameRect]), as fractions of the frame; null
/// outside it.
Offset? liveFramePoint(
  Offset local,
  Size box,
  Size frame, {
  double maxScale = double.infinity,
  Alignment alignment = Alignment.center,
}) {
  final rect = liveFrameRect(
    box,
    frame,
    maxScale: maxScale,
    alignment: alignment,
  );
  if (rect.isEmpty || !rect.contains(local)) return null;
  return Offset(
    ((local.dx - rect.left) / rect.width).clamp(0.0, 1.0),
    ((local.dy - rect.top) / rect.height).clamp(0.0, 1.0),
  );
}

/// The W3C `key` name of [key] when it isn't a character: Enter,
/// Backspace, the arrows and the rest; null for a key that types a
/// character, which goes as text.
String? liveKeyName(LogicalKeyboardKey key) => _names[key];

final _names = <LogicalKeyboardKey, String>{
  LogicalKeyboardKey.enter: 'Enter',
  LogicalKeyboardKey.numpadEnter: 'Enter',
  LogicalKeyboardKey.backspace: 'Backspace',
  LogicalKeyboardKey.delete: 'Delete',
  LogicalKeyboardKey.tab: 'Tab',
  LogicalKeyboardKey.escape: 'Escape',
  LogicalKeyboardKey.arrowLeft: 'ArrowLeft',
  LogicalKeyboardKey.arrowRight: 'ArrowRight',
  LogicalKeyboardKey.arrowUp: 'ArrowUp',
  LogicalKeyboardKey.arrowDown: 'ArrowDown',
  LogicalKeyboardKey.home: 'Home',
  LogicalKeyboardKey.end: 'End',
  LogicalKeyboardKey.pageUp: 'PageUp',
  LogicalKeyboardKey.pageDown: 'PageDown',
  LogicalKeyboardKey.insert: 'Insert',
  LogicalKeyboardKey.space: ' ',
  LogicalKeyboardKey.f1: 'F1',
  LogicalKeyboardKey.f2: 'F2',
  LogicalKeyboardKey.f3: 'F3',
  LogicalKeyboardKey.f4: 'F4',
  LogicalKeyboardKey.f5: 'F5',
  LogicalKeyboardKey.f6: 'F6',
  LogicalKeyboardKey.f7: 'F7',
  LogicalKeyboardKey.f8: 'F8',
  LogicalKeyboardKey.f9: 'F9',
  LogicalKeyboardKey.f10: 'F10',
  LogicalKeyboardKey.f11: 'F11',
  LogicalKeyboardKey.f12: 'F12',
};

/// The modifiers themselves, which go only as part of another key.
final _modifiers = {
  LogicalKeyboardKey.shiftLeft,
  LogicalKeyboardKey.shiftRight,
  LogicalKeyboardKey.controlLeft,
  LogicalKeyboardKey.controlRight,
  LogicalKeyboardKey.altLeft,
  LogicalKeyboardKey.altRight,
  LogicalKeyboardKey.metaLeft,
  LogicalKeyboardKey.metaRight,
  LogicalKeyboardKey.capsLock,
  LogicalKeyboardKey.fn,
};

/// What a key on the device's keyboard sends to the browser, which runs
/// on Linux: a shortcut (with ⌘ on a Mac, Ctrl elsewhere) as Ctrl and the
/// key, so ⌘A selects all there as it does here; a character as text;
/// a named key as itself. Nothing for a modifier alone, a key going up that typed a character, or a key the
/// browser has no name for.
List<LiveEvent> liveKeyEvents(KeyEvent event, {required bool mac}) {
  final key = event.logicalKey;
  if (_modifiers.contains(key)) return const [];
  final keyboard = HardwareKeyboard.instance;
  final shortcut = mac ? keyboard.isMetaPressed : keyboard.isControlPressed;
  final down = event is! KeyUpEvent;
  final name = liveKeyName(key);
  if (shortcut) {
    // The key's own letter, not what the modifier made of it.
    final label = name ?? key.keyLabel.toLowerCase();
    if (label.isEmpty) return const [];
    return [
      LiveKey(
        label,
        down: down,
        control: true,
        shift: keyboard.isShiftPressed,
        alt: keyboard.isAltPressed,
      ),
    ];
  }
  if (name != null && name != ' ') {
    return [
      LiveKey(
        name,
        down: down,
        shift: keyboard.isShiftPressed,
        alt: keyboard.isAltPressed,
        // Ctrl on a Mac is Ctrl there too (Ctrl+Tab, say).
        control: mac && keyboard.isControlPressed,
      ),
    ];
  }
  final character = event.character;
  if (!down || character == null || character.isEmpty) return const [];
  return [LiveTyped(character)];
}
