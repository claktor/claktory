import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

// Design tokens from docs/IDENTITY.md. Keep the two in sync.

/// Colors for the current system appearance.
class Palette {
  const Palette._({
    required this.canvas,
    required this.sidebar,
    required this.hairline,
    required this.text,
    required this.textMuted,
    required this.textFaint,
    required this.signal,
    required this.danger,
    required this.link,
  });

  final Color canvas;
  final Color sidebar;
  final Color hairline;
  final Color text;
  final Color textMuted;
  final Color textFaint;
  final Color signal;
  final Color danger;

  /// Web links in what agents write and in tool output, so they stand out
  /// from the text around them.
  final Color link;

  /// Hover and selection tints derive from `text` so they work on any surface.
  Color get hover => text.withValues(alpha: 0.05);
  Color get selected => text.withValues(alpha: 0.09);

  static const _dark = Palette._(
    canvas: Color(0xFF0E0E10),
    sidebar: Color(0xFF151517),
    hairline: Color(0xFF232327),
    text: Color(0xFFEDEDEF),
    textMuted: Color(0xFFA1A1AA),
    textFaint: Color(0xFF71717A),
    signal: Color(0xFFFF5B1F),
    danger: Color(0xFFFF5C7A),
    link: Color(0xFF6AA8FF),
  );

  static const _light = Palette._(
    canvas: Color(0xFFFFFFFF),
    sidebar: Color(0xFFF5F5F6),
    hairline: Color(0xFFE4E4E7),
    text: Color(0xFF18181B),
    textMuted: Color(0xFF52525B),
    textFaint: Color(0xFF8B8B94),
    signal: Color(0xFFE5480F),
    danger: Color(0xFFC8193C),
    link: Color(0xFF1F5FD1),
  );

  static Palette of(BuildContext context) =>
      MediaQuery.platformBrightnessOf(context) == Brightness.dark
      ? _dark
      : _light;
}

/// Agent colors, by the names the backend gives them (`Agent.color`). An
/// agent's color marks its bar, its dot and the bubbles of messages to and
/// from it. White text on each is at least 4.5:1.
const agentColors = <String, Color>{
  'blue': Color(0xFF2F6FEB),
  'violet': Color(0xFF7B52E0),
  'green': Color(0xFF1F8549),
  'pink': Color(0xFFC7358A),
  'teal': Color(0xFF11808A),
  'amber': Color(0xFFA86009),
  'red': Color(0xFFD03C3C),
  'slate': Color(0xFF5A6377),
};

/// The color named `name`, or slate for a name this app doesn't know.
Color agentColor(String name) => agentColors[name] ?? const Color(0xFF5A6377);

/// The faint wash of an agent's color behind its conversation: a few
/// percent, so text keeps its contrast in either theme.
const conversationTintAlpha = 0.04;

/// Each provider's own brand color, by provider id (`Provider.id`): its
/// usage circle in the sidebar. OpenAI's is black or white, so it follows
/// `text`; a provider this app doesn't know is `textMuted`.
Color providerColor(String id, Palette palette) => switch (id) {
  'anthropic' => const Color(0xFFD97757),
  'openai-codex' => const Color(0xFF10A37F),
  'openrouter' => const Color(0xFF6467F2),
  'openai' => palette.text,
  _ => palette.textMuted,
};

/// The single accent color (dark-theme value), for the OS task switcher.
const signal = Color(0xFFFF5B1F);

/// Type scale. Colors are applied where the styles are used.
abstract final class TextStyles {
  static const sans = 'Geist';
  static const mono = 'GeistMono';

  static const caption = TextStyle(
    fontFamily: sans,
    fontSize: 12,
    height: 1.45,
  );
  static const body = TextStyle(fontFamily: sans, fontSize: 13, height: 1.45);
  static const label = TextStyle(
    fontFamily: sans,
    fontSize: 13,
    height: 1.45,
    fontWeight: FontWeight.w500,
  );
  static const emphasis = TextStyle(
    fontFamily: sans,
    fontSize: 15,
    height: 1.2,
    fontWeight: FontWeight.w600,
  );
  static const title = TextStyle(
    fontFamily: sans,
    fontSize: 20,
    height: 1.2,
    fontWeight: FontWeight.w600,
  );
  static const code = TextStyle(fontFamily: mono, fontSize: 12, height: 1.5);
}

/// 4px grid.
abstract final class Space {
  static const xs = 4.0;
  static const s = 8.0;
  static const m = 12.0;
  static const l = 16.0;
  static const xl = 24.0;
  static const xxl = 32.0;
  static const xxxl = 48.0;
}

abstract final class Radii {
  static const control = 6.0;
  static const panel = 10.0;
  static const floating = 14.0;
}

abstract final class Motion {
  static const fast = Duration(milliseconds: 120);
  static const base = Duration(milliseconds: 200);
}

/// The sidebar's width by default; the person can drag it between
/// [sidebarMinWidth] and [sidebarMaxWidth] (`Settings.sidebar_width`).
const sidebarWidth = 248.0;
const sidebarMinWidth = 200.0;
const sidebarMaxWidth = 300.0;

/// The band at the top of the window. Over the sidebar it holds only the
/// window controls (macOS traffic lights, centered in it); over the content
/// the view's header bar fills it. All of it drags the window.
const titleBarHeight = 44.0;

/// A phone's screen is narrower than this on its shorter side, and it
/// takes a phone's layout (docs/IDENTITY.md, "Phone"): the sidebar as its
/// first screen, an agent's conversation pushed over it, panels as sheets.
/// A Mac window never gets this narrow.
const phoneLayoutWidth = 600.0;

/// Whether [context] is laid out as on a phone: a phone's screen, upright
/// or on its side (a tablet's isn't), or a window narrower than a phone.
bool isPhoneLayout(BuildContext context) {
  final size = MediaQuery.sizeOf(context);
  return switch (defaultTargetPlatform) {
    TargetPlatform.iOS ||
    TargetPlatform.android => size.shortestSide < phoneLayoutWidth,
    _ => size.width < phoneLayoutWidth,
  };
}

/// How wide the transcript and composer grow as the window widens.
const contentMaxWidth = 1520.0;

/// The soft shadow for floating layers (menus, dialogs).
const floatingShadow = [
  BoxShadow(color: Color(0x40000000), blurRadius: 24, offset: Offset(0, 8)),
];
