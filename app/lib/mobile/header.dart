import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../selection.dart' show Unselectable;
import 'metrics.dart';
import 'platform_policy.dart';

/// How large a [PhoneHeader]'s title grows with the person's text size.
const phoneTitleTextScale = 1.6;

/// A phone screen's header (docs/IDENTITY.md, "Phone"): at most one action
/// at its start (Menu or Back), its title, and up to two at its end (More,
/// Close), each a 44pt target. At least 44pt tall, and taller as the
/// system's text grows (its title up to [phoneTitleTextScale]), rather
/// than cutting its title off.
class PhoneHeader extends StatelessWidget {
  const PhoneHeader({
    super.key,
    this.leading,
    required this.title,
    this.trailing = const [],
    this.onTitleTap,
    this.titleLabel,
    this.titleHint,
    this.background,
    this.bottom,
    this.hairline = true,
  }) : assert(trailing.length <= 2, 'at most two actions at the end');

  /// Menu or Back ([HeaderButton.menu], [HeaderButton.back]).
  final Widget? leading;

  /// What the screen is: a name, or a name over a line (an agent's role),
  /// as many lines as the text needs.
  final Widget title;

  /// More, Close ([HeaderButton.more], [HeaderButton.close]).
  final List<Widget> trailing;

  /// A tap on the title: an agent's details, say.
  final VoidCallback? onTitleTap;

  /// What a tap on the title does, for VoiceOver ("Mike's details").
  final String? titleLabel;

  /// What tapping the title does, for VoiceOver ("Opens agent settings").
  final String? titleHint;

  /// Its fill: the sidebar's color unless given (an agent's tint).
  final Color? background;

  /// Under the row, over the hairline: an agent's colored line.
  final Widget? bottom;

  /// A hairline along its bottom.
  final bool hairline;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    // A bar's title grows only so far, as iOS's navigation bars' do (the
    // page's own words grow in full): past that, a word as long as
    // "Notifications" no longer fits a small iPhone's bar and would break
    // in the middle.
    final heading = MediaQuery.withClampedTextScaling(
      maxScaleFactor: phoneTitleTextScale,
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: Space.xs),
        child: Semantics(header: true, child: this.title),
      ),
    );
    Widget title = heading;
    if (onTitleTap case final onTap?) {
      title = Semantics(
        // One element: the title's words, or the label in their place.
        container: true,
        button: true,
        label: titleLabel,
        hint: titleHint,
        excludeSemantics: titleLabel != null,
        onTap: titleLabel != null ? onTap : null,
        child: Pressable(
          onTap: onTap,
          // The header's whole height takes the tap.
          builder: (context, pressed) => ConstrainedBox(
            constraints: BoxConstraints(
              minHeight: PhonePlatformPolicy.of(context).touchTarget,
            ),
            child: Align(
              alignment: AlignmentDirectional.centerStart,
              widthFactor: 1,
              child: AnimatedOpacity(
                duration: Motion.fast,
                opacity: pressed ? 0.6 : 1,
                child: heading,
              ),
            ),
          ),
        ),
      );
    }
    return DecoratedBox(
      decoration: BoxDecoration(
        color: background ?? palette.sidebar,
        border: hairline
            ? Border(bottom: BorderSide(color: palette.hairline))
            : null,
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ConstrainedBox(
            constraints: BoxConstraints(
              minHeight: PhonePlatformPolicy.of(context).touchTarget,
            ),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: Space.xs),
              child: Row(
                children: [
                  leading ?? const SizedBox(width: Touch.gutter - Space.xs),
                  Expanded(
                    child: Align(
                      alignment: AlignmentDirectional.centerStart,
                      child: title,
                    ),
                  ),
                  ...trailing,
                  if (trailing.isEmpty)
                    const SizedBox(width: Touch.gutter - Space.xs),
                ],
              ),
            ),
          ),
          ?bottom,
        ],
      ),
    );
  }
}

/// A header's action: a glyph in a 44×44 target, for VoiceOver [label].
/// At the end of a [DenseList]'s row, as tall as the row ([Touch.denseLine])
/// and 44 wide, with [inRow].
class HeaderButton extends StatelessWidget {
  const HeaderButton({
    super.key,
    required this.label,
    required this.glyph,
    required this.onTap,
    this.active = false,
    this.inRow = false,
  });

  /// Back, to the page under this one.
  factory HeaderButton.back({Key? key, required VoidCallback onTap}) =>
      HeaderButton(
        key: key,
        label: 'Back',
        onTap: onTap,
        glyph: (color) => ExtensionGlyph(
          icon: 'chevron-left',
          name: 'Back',
          color: color,
          size: 22,
        ),
      );

  /// The drawer of Claktories, projects and agents.
  factory HeaderButton.menu({Key? key, required VoidCallback onTap}) =>
      HeaderButton(
        key: key,
        label: 'Menu',
        onTap: onTap,
        glyph: (color) =>
            ExtensionGlyph(icon: 'menu', name: 'Menu', color: color, size: 20),
      );

  /// More of what can be done here, in a sheet. [label] names it when
  /// there's more than one More around ("Mike's extensions").
  factory HeaderButton.more({
    Key? key,
    String label = 'More',
    required VoidCallback onTap,
    bool inRow = false,
  }) => HeaderButton(
    key: key,
    label: label,
    onTap: onTap,
    inRow: inRow,
    glyph: (color) =>
        ExtensionGlyph(icon: 'ellipsis', name: 'More', color: color, size: 20),
  );

  /// Close, for a page or sheet that rose from below.
  factory HeaderButton.close({Key? key, required VoidCallback onTap}) =>
      HeaderButton(
        key: key,
        label: 'Close',
        onTap: onTap,
        glyph: (color) =>
            ExtensionGlyph(icon: 'x', name: 'Close', color: color, size: 20),
      );

  final String label;
  final Widget Function(Color color) glyph;
  final VoidCallback onTap;

  /// Tinted while what it opens is open.
  final bool active;

  /// At the end of a list's row rather than in a header.
  final bool inRow;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final target = PhonePlatformPolicy.of(context).touchTarget;
    return Unselectable(
      child: Semantics(
        // Its own element for VoiceOver, even inside a row that is one.
        container: true,
        button: true,
        label: label,
        onTap: onTap,
        excludeSemantics: true,
        child: Pressable(
          onTap: onTap,
          builder: (context, pressed) => SizedBox(
            width: target,
            // In a dense row, the row's height: it spans the row's width
            // (iOS). A platform that asks for more keeps its own.
            height: inRow && DenseList.of(context) && target <= Touch.target
                ? Touch.denseLine
                : target,
            child: Center(
              child: AnimatedContainer(
                duration: Motion.fast,
                width: 36,
                height: inRow && DenseList.of(context) ? 32 : 36,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: active
                      ? palette.selected
                      : pressed
                      ? palette.hover
                      : const Color(0x00000000),
                  borderRadius: BorderRadius.circular(Radii.panel),
                ),
                child: glyph(palette.text),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
