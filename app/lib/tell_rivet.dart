import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'scope.dart';

/// What Rivet is asked when the person hands it an error: the error in
/// full, after [about] (where it showed) when there's one.
String errorRequest(String error, {String about = ''}) {
  final where = about.isEmpty ? '' : '$about\n';
  return 'Claktory shows this error. Please fix it:\n\n$where$error';
}

/// Hands an error Factory shows to Rivet, as the sidebar's "+" hands it a
/// request: Rivet's conversation opens with the error typed in its message
/// box, ready to send. Nothing shows when there's no Rivet, or it has no
/// model to run. A quiet button, or as [link] a caption link for an error
/// that's itself a line of caption.
class TellRivet extends StatelessWidget {
  const TellRivet({
    super.key,
    required this.error,
    this.about = '',
    this.onTold,
    this.link = false,
  });

  final String error;

  /// Where the error showed, said before it: "The settings file has an
  /// error.", "Builder's last run failed."
  final String about;

  /// After it's handed over, such as closing the dialog it showed in.
  final VoidCallback? onTold;
  final bool link;

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        final rivet = rivetIn(store.workspace);
        if (rivet == null || !rivet.modelAvailable || error.isEmpty) {
          return const SizedBox.shrink();
        }
        final label = 'Ask ${rivet.name} to fix';
        void tell() {
          if (askRivet(
            context,
            errorRequest(error, about: about),
            hint: 'Send it to ${rivet.name} to fix',
          )) {
            onTold?.call();
          }
        }

        if (!link) return Button(label: label, onPressed: tell);
        final palette = Palette.of(context);
        return Semantics(
          button: true,
          label: label,
          excludeSemantics: true,
          child: Pressable(
            onTap: tell,
            builder: (context, hovered) => Text(
              label,
              style: TextStyles.caption.copyWith(
                color: hovered ? palette.text : palette.textMuted,
                fontWeight: FontWeight.w500,
                decoration: hovered ? TextDecoration.underline : null,
                decorationColor: palette.text,
              ),
            ),
          ),
        );
      },
    );
  }
}

/// An error line with [TellRivet] after it, wrapping when it's long.
class ErrorLine extends StatelessWidget {
  const ErrorLine(this.text, {super.key, this.about = '', this.style});

  final String text;
  final String about;

  /// The error's style; caption `danger` unless given.
  final TextStyle? style;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Wrap(
      spacing: Space.s,
      crossAxisAlignment: WrapCrossAlignment.end,
      children: [
        Text(
          text,
          style: style ?? TextStyles.caption.copyWith(color: palette.danger),
        ),
        TellRivet(error: text, about: about, link: true),
      ],
    );
  }
}
