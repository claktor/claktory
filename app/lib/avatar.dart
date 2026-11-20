import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';

/// "#RRGGBB" (or "RRGGBB") as a color; null when it isn't one.
Color? hexColor(String hex) {
  final digits = hex.startsWith('#') ? hex.substring(1) : hex;
  if (digits.length != 6) return null;
  final value = int.tryParse(digits, radix: 16);
  return value == null ? null : Color(0xFF000000 | value);
}

/// The person's avatar when the backend hasn't said (an older backend):
/// slate, the last of the twelve pairs.
const _fallbackFrom = Color(0xFF475569);
const _fallbackTo = Color(0xFF334155);

/// The person this Claktory belongs to, as the backend computes them: their
/// initials on a diagonal gradient, in a circle (never a Claktory's rounded
/// square, nor an agent's character). With no name yet, a person glyph.
class PersonAvatar extends StatelessWidget {
  const PersonAvatar({super.key, this.person, this.size = 20});

  /// Whom to draw; the workspace's person when null.
  final Person? person;
  final double size;

  @override
  Widget build(BuildContext context) {
    final person = this.person;
    if (person != null) return _draw(person);
    // Outside a Claktory's views (some tests), the default circle.
    final store = context
        .getInheritedWidgetOfExactType<AppScope>()
        ?.backend
        .workspace;
    if (store == null) return _draw(Person());
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => _draw(store.workspace.person),
    );
  }

  Widget _draw(Person person) {
    final from = hexColor(person.avatarFrom) ?? _fallbackFrom;
    final to = hexColor(person.avatarTo) ?? _fallbackTo;
    final initials = person.initials.trim();
    const white = Color(0xFFFFFFFF);
    // A picture, not text to select and copy.
    return Unselectable(
      child: Semantics(
        label: person.name.isEmpty ? 'You' : person.name,
        excludeSemantics: true,
        child: Container(
          width: size,
          height: size,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [from, to],
            ),
          ),
          child: initials.isEmpty
              ? ExtensionGlyph(
                  icon: 'user',
                  name: 'You',
                  color: white,
                  size: size * 0.6,
                )
              : Text(
                  initials,
                  maxLines: 1,
                  softWrap: false,
                  overflow: TextOverflow.clip,
                  textScaler: TextScaler.noScaling,
                  style: TextStyles.label.copyWith(
                    color: white,
                    // Two letters fit a circle at 40% of its size.
                    fontSize:
                        size * (initials.characters.length > 1 ? 0.4 : 0.46),
                    fontWeight: FontWeight.w600,
                    height: 1,
                    letterSpacing: 0,
                  ),
                ),
        ),
      ),
    );
  }
}
