import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import 'platform_policy.dart';
import 'push.dart' show PushRegistrar;

/// The message box on a phone (research mobile.md §4, "Conversation, input
/// and keyboard"), laid out as iOS's own Messages: + at its start for files
/// and photos, the field in a rounded box that grows from one line to five
/// and then scrolls, and one primary action at its end, Send while there's
/// something to send. Every action is a finger's size (44pt).
///
/// What it does is the message box's own (agent_view.dart): this is only
/// how it sits on a phone.
class PhoneComposerRow extends StatelessWidget {
  const PhoneComposerRow({
    super.key,
    required this.field,
    required this.canSend,
    required this.onSend,
    this.onAttach,
    this.primary,
    this.maxFieldHeight,
    this.recording,
    this.accent,
  });

  /// Send's color, the target's ([SendButton.accent]).
  final Color? accent;

  /// While a voice message records: what shows in place of + and the field
  /// (the primary slot stays, under the finger that holds it).
  final Widget? recording;

  /// The text field, borderless: the row draws its box.
  final Widget field;

  /// Whether there's text or a file to send; Send is offered only then.
  final bool canSend;
  final VoidCallback onSend;

  /// The + for files and photos; none where nothing can be attached.
  final VoidCallback? onAttach;

  /// What takes Send's place while there's nothing to send (the mic); with
  /// none, Send shows, unavailable.
  final Widget? primary;

  /// How tall the field's box may grow before its text scrolls, so a long
  /// draft never takes the whole screen above the keyboard.
  final double? maxFieldHeight;

  /// Sends; the first time, the system then asks whether Claktory may
  /// notify the person (an answer is what they'll want to hear of).
  void _send() {
    onSend();
    unawaited(PushRegistrar.current?.askInContext());
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final target = PhonePlatformPolicy.of(context).touchTarget;
    final box = Container(
      constraints: BoxConstraints(
        minHeight: target - Space.xs,
        maxHeight: math.max(target, maxFieldHeight ?? double.infinity),
      ),
      padding: const EdgeInsets.symmetric(horizontal: Space.m, vertical: 7),
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(20),
        border: Border.all(color: palette.hairline),
      ),
      // As tall as what's written, from one line: a Container's own
      // alignment would fill the cap, five lines' worth of empty box.
      child: Align(
        alignment: AlignmentDirectional.centerStart,
        heightFactor: 1,
        child: field,
      ),
    );
    final recording = this.recording;
    if (recording != null) {
      return Row(
        children: [
          Expanded(
            child: SizedBox(height: target, child: recording),
          ),
          const SizedBox(width: Space.xs),
          primary ?? SendButton(onSend: canSend ? _send : null, accent: accent),
        ],
      );
    }
    return Row(
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        if (onAttach != null)
          Semantics(
            // Its own element, or the row's others merge into it.
            container: true,
            child: GlyphButton(
              label: 'Attach files',
              active: false,
              onTap: onAttach!,
              glyph: (color) => ExtensionGlyph(
                icon: 'plus',
                name: 'Attach files',
                color: color,
                size: 22,
              ),
            ),
          )
        else
          const SizedBox(width: Space.xs),
        const SizedBox(width: Space.xs),
        Expanded(
          // Its own element for VoiceOver and TalkBack, not merged into the
          // page's, and a finger's size like the buttons beside it: the
          // field's label, value and actions are the whole box's.
          // Anywhere in the box, a finger's tap or VoiceOver's, brings the
          // keyboard up, as in Messages, not only on its line of text.
          child: Builder(
            builder: (context) => Semantics(
              container: true,
              onTap: () => _requestKeyboard(context),
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                excludeFromSemantics: true,
                onTap: () => _requestKeyboard(context),
                child: Padding(
                  // Centered on the buttons beside it while it's one line.
                  padding: const EdgeInsets.symmetric(vertical: 2),
                  child: box,
                ),
              ),
            ),
          ),
        ),
        const SizedBox(width: Space.xs),
        if (canSend || primary == null)
          SendButton(onSend: canSend ? _send : null, accent: accent)
        else
          primary!,
      ],
    );
  }
}

/// Send, at the end of a phone's message box: an arrow up in a signal
/// disc, 32pt in a 44pt target, or in the target's [accent] with the
/// arrow white. Faint while there's nothing to send.
/// Brings up the keyboard for the field under [context], focusing it.
void _requestKeyboard(BuildContext context) {
  EditableTextState? editable;
  void visit(Element element) {
    if (editable != null) return;
    if (element is StatefulElement && element.state is EditableTextState) {
      editable = element.state as EditableTextState;
      return;
    }
    element.visitChildren(visit);
  }

  context.visitChildElements(visit);
  editable?.requestKeyboard();
}

class SendButton extends StatelessWidget {
  const SendButton({super.key, required this.onSend, this.accent});

  final VoidCallback? onSend;

  /// The disc's color in place of signal, such as a pipe's; the arrow on
  /// it is white.
  final Color? accent;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final enabled = onSend != null;
    final fill = accent ?? palette.signal;
    return Semantics(
      container: true,
      button: true,
      enabled: enabled,
      label: 'Send',
      onTap: onSend,
      excludeSemantics: true,
      child: Pressable(
        onTap: onSend,
        builder: (context, pressed) => SizedBox.square(
          dimension: PhonePlatformPolicy.of(context).touchTarget,
          child: Center(
            child: AnimatedContainer(
              duration: Motion.fast,
              width: 32,
              height: 32,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: enabled
                    ? (pressed
                          ? Color.alphaBlend(const Color(0x33000000), fill)
                          : fill)
                    : palette.selected,
              ),
              alignment: Alignment.center,
              // Lucide's arrow-down, turned up.
              child: Transform.rotate(
                angle: math.pi,
                child: ExtensionGlyph(
                  icon: 'arrow-down',
                  name: 'Send',
                  // Text on signal is black (docs/IDENTITY.md, Contrast);
                  // on an accent, white, as on agents' bubbles.
                  color: !enabled
                      ? palette.textFaint
                      : accent != null
                      ? const Color(0xFFFFFFFF)
                      : const Color(0xFF000000),
                  size: 18,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
