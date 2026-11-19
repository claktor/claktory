import 'package:flutter/widgets.dart';

import 'identity.dart';

/// An agent's role: smaller than its name (11px), in [color], which the
/// caller picks a step fainter than the name's.
TextStyle roleStyle(Color color) =>
    TextStyles.caption.copyWith(fontSize: 11, height: 1.25, color: color);

/// An agent's name with its role under it, smaller and fainter ("Mike" over
/// "Coordinator"). With no role it's the name alone, as it always was.
class NameOverRole extends StatelessWidget {
  const NameOverRole({
    super.key,
    required this.name,
    required this.role,
    required this.style,
    required this.roleColor,
    this.trailing,
  });

  final String name;
  final String role;
  final TextStyle style;
  final Color roleColor;

  /// After the name on its line, such as the agent's project.
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final text = Text(
      name,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      // Stacked, the lines sit closer, to fit the row.
      style: role.isEmpty ? style : style.copyWith(height: 1.25),
    );
    final line = trailing == null
        ? text
        : Row(
            children: [
              Flexible(child: text),
              trailing!,
            ],
          );
    if (role.isEmpty) return line;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        line,
        Text(
          role,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: roleStyle(roleColor),
        ),
      ],
    );
  }
}

/// An agent's name with its role after it on the same line, smaller and
/// fainter ("Mike  Coordinator"), where a line has no room for two: pickers
/// and the names over messages.
class NameThenRole extends StatelessWidget {
  const NameThenRole({
    super.key,
    required this.name,
    required this.role,
    required this.style,
    required this.roleColor,
  });

  final String name;
  final String role;
  final TextStyle style;
  final Color roleColor;

  @override
  Widget build(BuildContext context) => Text.rich(
    TextSpan(
      text: name,
      children: [
        if (role.isNotEmpty)
          TextSpan(
            text: '  $role',
            // Not underlined with the name, when that's a link.
            style: roleStyle(roleColor)
                .copyWith(decoration: TextDecoration.none),
          ),
      ],
    ),
    style: style,
    maxLines: 1,
    overflow: TextOverflow.ellipsis,
  );
}
