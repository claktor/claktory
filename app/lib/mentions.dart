import 'package:flutter/widgets.dart';
import 'package:markdown/markdown.dart' as md;

import 'agent_name.dart' show roleStyle;
import 'characters/character.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart' show Agent;
import 'selection.dart' show Unselectable;

/// The agent an @mention names, among [agents]: the one whose id it is,
/// else the one agent with that name, ignoring case. Two agents with the
/// name (in different projects), or none, name no one.
Agent? resolveMention(String word, Iterable<Agent> agents) {
  final wanted = word.toLowerCase();
  for (final agent in agents) {
    if (agent.id.toLowerCase() == wanted) return agent;
  }
  Agent? named;
  for (final agent in agents) {
    if (agent.name.toLowerCase() != wanted) continue;
    if (named != null) return null;
    named = agent;
  }
  return named;
}

/// The Markdown element an @mention parses to; its text is the word after
/// the @.
const mentionTag = 'mention';

// A letter or digit, accented ones too ("Tomás"); inside, also "-" and "_"
// ("backend-2"), never at the end, so "@Kenji-" or "@Kenji." mean Kenji.
const _char = r'A-Za-z0-9\u00C0-\u024F';

/// "@Kenji" or "@backend-2", not inside a word ("ana@example.com") or code
/// (code spans are parsed before it gets there).
class MentionSyntax extends md.InlineSyntax {
  MentionSyntax()
    : super('@([$_char](?:[${_char}_-]*[$_char])?)', startCharacter: 0x40);

  static final _word = RegExp('[${_char}_]');

  @override
  bool tryMatch(md.InlineParser parser, [int? startMatchPos]) {
    final at = startMatchPos ?? parser.pos;
    if (at > 0 && _word.hasMatch(parser.source[at - 1])) return false;
    return super.tryMatch(parser, startMatchPos);
  }

  @override
  bool onMatch(md.InlineParser parser, Match match) {
    parser.addNode(md.Element.text(mentionTag, match[1]!));
    return true;
  }
}

/// An agent mentioned in text: a tag the height of the line with its
/// character, its name in its color and its role after it, like the name
/// over a message from it. A click opens its conversation ([open]). Copied,
/// it's the agent's name.
class MentionChip extends StatefulWidget {
  const MentionChip({
    super.key,
    required this.agent,
    required this.style,
    this.onFill,
    this.open,
  });

  final Agent agent;

  /// The text around it: the tag takes its size.
  final TextStyle style;

  /// The fill's text color, when the text is on one (another agent's
  /// bubble): the name takes it, on a light wash, so it reads there.
  final Color? onFill;

  final ValueChanged<String>? open;

  @override
  State<MentionChip> createState() => _MentionChipState();
}

class _MentionChipState extends State<MentionChip> {
  var _hovered = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final agent = widget.agent;
    final color = agentColor(agent.color);
    final size = widget.style.fontSize ?? 13;
    final fill = widget.onFill;
    final ink = fill ?? color;
    final open = widget.open;
    Widget chip = Container(
      padding: const EdgeInsets.only(left: 2, right: 5),
      decoration: BoxDecoration(
        color: (fill ?? color).withValues(alpha: fill == null ? 0.14 : 0.2),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Unselectable(
            child: AgentCharacter(agent: agent, size: size + 3),
          ),
          const SizedBox(width: 3),
          Text(
            agent.name,
            style: widget.style.copyWith(
              color: ink,
              fontWeight: FontWeight.w600,
              decoration: open != null && _hovered
                  ? TextDecoration.underline
                  : TextDecoration.none,
              decorationColor: ink,
            ),
          ),
          if (agent.role.isNotEmpty)
            Unselectable(
              child: Text(
                '  ${agent.role}',
                style: roleStyle(
                  fill?.withValues(alpha: 0.75) ?? palette.textFaint,
                ).copyWith(fontSize: size - 2),
              ),
            ),
        ],
      ),
    );
    if (open == null) return chip;
    // The pointer shows over the name too, which would otherwise show the
    // text cursor: a region laid over it that lets the pointer through.
    return Semantics(
      button: true,
      label: 'Open ${agent.name}',
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => open(agent.id),
        child: Stack(
          children: [
            chip,
            Positioned.fill(
              child: MouseRegion(
                opaque: false,
                cursor: SystemMouseCursors.click,
                onEnter: (_) => setState(() => _hovered = true),
                onExit: (_) => setState(() => _hovered = false),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
