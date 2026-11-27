import 'package:flutter/widgets.dart';

import '../identity.dart';
import 'platform_policy.dart';

/// A finger's sizes on a phone (docs/IDENTITY.md, "Phone"): Apple's 44pt
/// for anything that can be tapped, rows a little taller for a name over a
/// line under it, and 16pt from the screen's edges.
abstract final class Touch {
  /// The least a tappable thing measures each way, whatever it shows.
  static const target = 44.0;

  /// The least a list row measures: a name with a line under it.
  static const row = 52.0;

  /// From the screen's edges to the content.
  static const gutter = 16.0;

  /// In a [DenseList], the least a row with a line under its name
  /// measures: as tall as a finger needs, no taller.
  static const denseRow = 44.0;

  /// In a [DenseList], the least a row of one line measures. The rows
  /// span the list's width, so a finger finds them at this height.
  static const denseLine = 36.0;
}

/// A phone's lists that hold a lot and are read at a glance, the drawer and
/// the list of Claktories: a notch smaller than [PhoneType] and [Touch]'s
/// rows ([DenseType], [Touch.denseRow], [Touch.denseLine]), nearer a Mac's
/// sidebar, so more of them is in view. Text in them still grows with the
/// system's text size.
class DenseList extends InheritedWidget {
  const DenseList({super.key, required super.child});

  /// Whether [context] is in one.
  static bool of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<DenseList>() != null;

  @override
  bool updateShouldNotify(DenseList oldWidget) => false;
}

/// The type a phone reads at: iOS's 17pt body, as Apple's own apps use,
/// with the app's Geist and Geist Mono. Text grows with the system's text
/// size from here; nothing clamps it.
abstract final class PhoneType {
  static const body = TextStyle(
    fontFamily: TextStyles.sans,
    fontSize: 17,
    height: 1.4,
  );
  static const secondary = TextStyle(
    fontFamily: TextStyles.sans,
    fontSize: 15,
    height: 1.4,
  );
  static const caption = TextStyle(
    fontFamily: TextStyles.sans,
    fontSize: 13,
    height: 1.4,
  );
  static const label = TextStyle(
    fontFamily: TextStyles.sans,
    fontSize: 17,
    height: 1.3,
    fontWeight: FontWeight.w500,
  );
  static const emphasis = TextStyle(
    fontFamily: TextStyles.sans,
    fontSize: 17,
    height: 1.3,
    fontWeight: FontWeight.w600,
  );
  static const title = TextStyle(
    fontFamily: TextStyles.sans,
    fontSize: 22,
    height: 1.25,
    fontWeight: FontWeight.w600,
  );
  static const code = TextStyle(
    fontFamily: TextStyles.mono,
    fontSize: 15,
    height: 1.45,
  );

  /// Code within a line of [body] text.
  static const codeInline = TextStyle(
    fontFamily: TextStyles.mono,
    fontSize: 16,
    height: 1.4,
  );
}

/// [PhoneType] a notch smaller, for a [DenseList]: 15pt names, 12pt lines
/// under them.
abstract final class DenseType {
  static const body = TextStyle(
    fontFamily: TextStyles.sans,
    fontSize: 15,
    height: 1.35,
  );
  static const secondary = TextStyle(
    fontFamily: TextStyles.sans,
    fontSize: 13,
    height: 1.35,
  );
  static const caption = TextStyle(
    fontFamily: TextStyles.sans,
    fontSize: 12,
    height: 1.35,
  );
  static const label = TextStyle(
    fontFamily: TextStyles.sans,
    fontSize: 15,
    height: 1.3,
    fontWeight: FontWeight.w500,
  );
  static const emphasis = TextStyle(
    fontFamily: TextStyles.sans,
    fontSize: 15,
    height: 1.3,
    fontWeight: FontWeight.w600,
  );
  static const title = TextStyle(
    fontFamily: TextStyles.sans,
    fontSize: 20,
    height: 1.25,
    fontWeight: FontWeight.w600,
  );
  static const code = TextStyle(
    fontFamily: TextStyles.mono,
    fontSize: 13,
    height: 1.45,
  );
  static const codeInline = TextStyle(
    fontFamily: TextStyles.mono,
    fontSize: 14,
    height: 1.4,
  );
}

/// The styles to use where this widget is: [PhoneType]'s on a phone or
/// any touch device, a wide iPad window too ([touchFirst]; [DenseType]'s
/// in a [DenseList]),
/// [TextStyles]' elsewhere, so a widget shared by both asks once.
class TypeScale {
  const TypeScale._(this.phone, {this.dense = false});

  /// Whether these are a phone's.
  final bool phone;

  /// Whether these are a [DenseList]'s, on a phone.
  final bool dense;

  static const _phone = TypeScale._(true);
  static const _dense = TypeScale._(true, dense: true);
  static const _desktop = TypeScale._(false);

  static TypeScale of(BuildContext context) => !touchFirst(context)
      ? _desktop
      : DenseList.of(context)
      ? _dense
      : _phone;

  /// Reading text: messages, descriptions, field contents.
  TextStyle get body => dense
      ? DenseType.body
      : phone
      ? PhoneType.body
      : TextStyles.body;

  /// Text under or beside the main text: a role, a status, a note.
  TextStyle get secondary => dense
      ? DenseType.secondary
      : phone
      ? PhoneType.secondary
      : TextStyles.body;

  /// Small print nobody needs to act on.
  TextStyle get caption => dense
      ? DenseType.caption
      : phone
      ? PhoneType.caption
      : TextStyles.caption;

  /// A control's words: buttons, rows.
  TextStyle get label => dense
      ? DenseType.label
      : phone
      ? PhoneType.label
      : TextStyles.label;

  /// A heading within a screen, or a sheet's title.
  TextStyle get emphasis => dense
      ? DenseType.emphasis
      : phone
      ? PhoneType.emphasis
      : TextStyles.emphasis;

  /// A screen's title.
  TextStyle get title => dense
      ? DenseType.title
      : phone
      ? PhoneType.title
      : TextStyles.title;

  /// Code and output, in a block of its own.
  TextStyle get code => dense
      ? DenseType.code
      : phone
      ? PhoneType.code
      : TextStyles.code;

  /// Code within a line of [body] text.
  TextStyle get codeInline => dense
      ? DenseType.codeInline
      : phone
      ? PhoneType.codeInline
      : TextStyles.code;
}

/// The sizes to use where this widget is: a finger's on a phone or any
/// touch device, a wide iPad window too ([touchFirst]), a
/// pointer's elsewhere. All of them are minimums: a row or a control grows
/// with the system's text size rather than cutting its words off.
class Metrics {
  const Metrics._(this.phone, {this.touch = Touch.target});

  final bool phone;

  /// A finger's target on this platform ([PhonePlatformPolicy.touchTarget]).
  final double touch;

  static const _phone = Metrics._(true);
  static const _desktop = Metrics._(false);

  static Metrics of(BuildContext context) {
    if (!touchFirst(context)) return _desktop;
    final touch = PhonePlatformPolicy.of(context).touchTarget;
    return touch == Touch.target ? _phone : Metrics._(true, touch: touch);
  }

  /// The least a tappable thing measures each way.
  double get target => phone ? touch : 28;

  /// A button's or a field's height.
  double get control => phone ? touch : 28;

  /// A list row's height.
  double get row => phone ? Touch.row : 32;

  /// From the edges to the content.
  double get gutter => phone ? Touch.gutter : Space.xl;
}

/// [child] with the safe areas, and with [keyboard] the keyboard, taken out
/// of its [MediaQuery]: for what keeps its content clear of them itself (a
/// page, a sheet, the drawer), so nothing inside adds them again. One
/// [MediaQuery], from [context]'s: two nested ones each built from
/// [context] would put back what the outer one took out.
Widget withoutInsets(
  BuildContext context, {
  required Widget child,
  bool keyboard = true,
}) => MediaQuery(
  data: MediaQuery.of(context)
      .removePadding(
        removeTop: true,
        removeBottom: true,
        removeLeft: true,
        removeRight: true,
      )
      .removeViewInsets(removeBottom: keyboard),
  child: child,
);

/// [glyph], drawn at [size] points, grown with the person's text size as
/// SF Symbols grow, up to twice its size, in a box that size: beside a
/// label at an accessibility size it keeps its weight next to the words.
Widget glyphWithText(BuildContext context, Widget glyph, {double size = 16}) {
  final grow = glyphGrowth(context, size: size);
  if (grow == 1.0) return glyph;
  return SizedBox.square(
    dimension: size * grow,
    child: FittedBox(child: glyph),
  );
}

/// How much [glyphWithText] grows a glyph of [size] points: with the text,
/// from 1 up to 2.
double glyphGrowth(BuildContext context, {double size = 16}) =>
    (MediaQuery.textScalerOf(context).scale(size) / size).clamp(1.0, 2.0);
