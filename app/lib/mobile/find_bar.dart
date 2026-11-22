// A phone's find bar, at a page's foot, as Safari's Find on Page has it:
// the field, how many matches and which one, the previous and next match,
// and Done. It sits above the keyboard (pushed pages end there), in the
// thumb's reach; at the largest text the field takes its own line and the
// rest goes under it.

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../selection.dart' show Unselectable;
import 'header.dart' show HeaderButton;
import 'input.dart';
import 'metrics.dart';
import 'platform_policy.dart';

class PhoneFindBar extends StatelessWidget {
  const PhoneFindBar({
    super.key,
    required this.controller,
    required this.focusNode,
    required this.placeholder,
    required this.count,
    required this.onStep,
    this.canStep = true,
    this.failed = false,
    required this.onDone,
    this.nextIsUp = false,
  });

  final TextEditingController controller;
  final FocusNode focusNode;

  /// The field's words when empty, and its name for VoiceOver.
  final String placeholder;

  /// "3 of 12", "No matches", or nothing before a search.
  final String count;

  /// The next match (1) or the one before (-1).
  final void Function(int by) onStep;

  /// Whether there are matches to step through: the arrows are dimmed and
  /// do nothing while there are none.
  final bool canStep;

  /// Whether [count] says why it couldn't search, shown as an error.
  final bool failed;
  final VoidCallback onDone;

  /// Whether the next match is further up the screen (a conversation, read
  /// from its newest end): the up arrow is then Next.
  final bool nextIsUp;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final field = DecoratedBox(
      decoration: BoxDecoration(
        color: palette.hover,
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: TextFieldBox(
        focusNode: focusNode,
        child: ConstrainedBox(
          constraints: BoxConstraints(
            minHeight: PhonePlatformPolicy.least(context, Touch.denseLine),
          ),
          child: Padding(
            padding: const EdgeInsetsDirectional.only(start: Space.m),
            child: Row(
              children: [
                ExcludeSemantics(
                  child: ExtensionGlyph(
                    icon: 'search',
                    name: '',
                    color: palette.textFaint,
                  ),
                ),
                const SizedBox(width: Space.xs),
                Expanded(
                  child: TextField(
                    key: const ValueKey('phone-find'),
                    controller: controller,
                    focusNode: focusNode,
                    placeholder: placeholder,
                    semanticsLabel: placeholder,
                    bordered: false,
                    kind: InputKind.text,
                    textInputAction: TextInputAction.search,
                    autocorrect: false,
                    onSubmitted: (_) => onStep(1),
                    // The keyboard stays up for the next Search.
                    onEditingComplete: () {},
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
    final countText = Semantics(
      liveRegion: true,
      child: Text(
        count,
        key: const ValueKey('phone-find-count'),
        style: type.secondary.copyWith(
          color: failed ? palette.danger : palette.textMuted,
          fontFeatures: const [FontFeature.tabularFigures()],
        ),
      ),
    );
    final arrows = _Dimmed(
      enabled: canStep,
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          // Up the screen first, then down, whichever way the matches go.
          HeaderButton(
            label: nextIsUp ? 'Next match' : 'Previous match',
            onTap: () => onStep(nextIsUp ? 1 : -1),
            glyph: (color) => _Chevron(color: color, up: true),
          ),
          HeaderButton(
            label: nextIsUp ? 'Previous match' : 'Next match',
            onTap: () => onStep(nextIsUp ? -1 : 1),
            glyph: (color) => _Chevron(color: color, up: false),
          ),
        ],
      ),
    );

    final done = Button(label: 'Done', onPressed: onDone);
    // A bar's words grow only so far, as iOS's toolbars' do; the field's
    // line of its own takes over past [_ownLine].
    // A viewer's page ends at the home indicator, not the keyboard (a
    // sheet's keyboard mustn't resize what's under it), and the home
    // indicator's inset goes while the keyboard is up: the bar rises by
    // the keyboard itself. On a page that ends at the keyboard this is 0.
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    return Padding(
      padding: EdgeInsets.only(bottom: keyboard),
      child: MediaQuery.withClampedTextScaling(
        maxScaleFactor: _most,
        child: Unselectable(
          child: Container(
            key: const ValueKey('phone-find-bar'),
            padding: const EdgeInsets.fromLTRB(
              Touch.gutter,
              Space.xs,
              Space.s,
              Space.xs,
            ),
            decoration: BoxDecoration(
              color: palette.sidebar,
              border: Border(top: BorderSide(color: palette.hairline)),
            ),
            child: LayoutBuilder(
              builder: (context, constraints) {
                // The field keeps at least half the bar; past that (the
                // largest text), it has a line of its own.
                final scale = MediaQuery.textScalerOf(context).scale(1);
                if (scale <= _ownLine) {
                  return Row(
                    children: [
                      // The field keeps most of the line; the count (or why
                      // it couldn't search) wraps rather than squeeze it.
                      Expanded(child: field),
                      const SizedBox(width: Space.s),
                      ConstrainedBox(
                        constraints: BoxConstraints(
                          maxWidth: constraints.maxWidth / 4,
                        ),
                        child: countText,
                      ),
                      arrows,
                      done,
                    ],
                  );
                }
                return Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Padding(
                      padding: const EdgeInsetsDirectional.only(end: Space.s),
                      child: field,
                    ),
                    Row(
                      children: [
                        Expanded(child: countText),
                        arrows,
                        done,
                      ],
                    ),
                  ],
                );
              },
            ),
          ),
        ),
      ),
    );
  }
}

/// How large the bar's words grow.
const _most = 2.0;

/// From which text size the field has a line of its own.
const _ownLine = 1.6;

/// Up or down: Back's chevron, turned.
class _Chevron extends StatelessWidget {
  const _Chevron({required this.color, required this.up});

  final Color color;
  final bool up;

  @override
  Widget build(BuildContext context) => RotatedBox(
    quarterTurns: up ? 1 : 3,
    child: ExtensionGlyph(
      icon: 'chevron-left',
      name: '',
      color: color,
      size: 22,
    ),
  );
}

/// [child], dimmed and out of reach unless [enabled].
class _Dimmed extends StatelessWidget {
  const _Dimmed({required this.enabled, required this.child});

  final bool enabled;
  final Widget child;

  @override
  Widget build(BuildContext context) => AnimatedOpacity(
    duration: Motion.fast,
    opacity: enabled ? 1 : 0.35,
    // VoiceOver still names them, dimmed: they can't be activated.
    child: Semantics(
      enabled: enabled,
      child: IgnorePointer(ignoring: !enabled, child: child),
    ),
  );
}
