// An agent's name with its role beside it, quietly, on one line that says
// who asks in Needs you (needs-you-cards.md §3.10, Matt via Bruno 46581):
// "Urgent · June · Project Manager · 14:32". The role is cut first, with an
// ellipsis, and goes entirely before the name is cut; what comes before the
// name (Urgent) and after it (the time, "3 alerts") stays whole.
import 'package:flutter/widgets.dart';

/// Below this much room the role isn't shown at all: " · Pr…" says nothing.
const _leastRole = 4;

/// [before], [name], " · [role]" in [roleStyle] and [after] on one line in
/// [style]. [wrap] (the largest text sizes) lets the whole line wrap rather
/// than cutting anything. VoiceOver reads it "June, Project Manager".
class NameRoleLine extends StatelessWidget {
  const NameRoleLine({
    super.key,
    required this.name,
    required this.role,
    required this.style,
    required this.roleStyle,
    this.before = const [],
    this.after = const [],
    this.wrap = false,
    this.mark,
  });

  final String name;

  /// Empty: the name alone, with no dangling dot.
  final String role;
  final TextStyle style;
  final TextStyle roleStyle;
  final List<InlineSpan> before;
  final List<InlineSpan> after;
  final bool wrap;

  /// Marks what find matched in the name's span (FindHighlight.mark).
  final TextSpan Function(TextSpan span)? mark;

  TextSpan get _name {
    final span = TextSpan(text: name);
    return mark?.call(span) ?? span;
  }

  TextSpan get _role =>
      TextSpan(text: ' · $role', semanticsLabel: ', $role', style: roleStyle);

  @override
  Widget build(BuildContext context) {
    if (wrap || role.isEmpty) {
      return Text.rich(
        TextSpan(
          children: [...before, _name, if (role.isNotEmpty) _role, ...after],
        ),
        style: style,
        maxLines: wrap ? null : 1,
        overflow: wrap ? null : TextOverflow.ellipsis,
      );
    }
    return LayoutBuilder(
      builder: (context, box) {
        final scaler = MediaQuery.textScalerOf(context);
        final direction = Directionality.of(context);
        double width(List<InlineSpan> spans) {
          final painter = TextPainter(
            text: TextSpan(style: style, children: spans),
            textDirection: direction,
            textScaler: scaler,
            maxLines: 1,
          )..layout();
          final width = painter.width;
          painter.dispose();
          return width;
        }

        final rest = width([...before, _name, ...after]);
        final least = width([
          TextSpan(
            text: ' · ${role.characters.take(_leastRole)}…',
            style: roleStyle,
          ),
        ]);
        // No room for the role: the name (cut last of all) and what's
        // around it.
        if (!box.hasBoundedWidth || box.maxWidth - rest < least) {
          if (box.hasBoundedWidth && rest > box.maxWidth) {
            return Row(
              children: [
                Flexible(
                  child: Text.rich(
                    TextSpan(children: [...before, _name]),
                    style: style,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                if (after.isNotEmpty)
                  Text.rich(
                    TextSpan(children: after),
                    style: style,
                    maxLines: 1,
                  ),
              ],
            );
          }
          return Text.rich(
            TextSpan(children: [...before, _name, ...after]),
            style: style,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          );
        }
        return Row(
          children: [
            Text.rich(
              TextSpan(children: [...before, _name]),
              style: style,
              maxLines: 1,
            ),
            Flexible(
              child: Text.rich(
                _role,
                style: style,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
            if (after.isNotEmpty)
              Text.rich(TextSpan(children: after), style: style, maxLines: 1),
          ],
        );
      },
    );
  }
}
