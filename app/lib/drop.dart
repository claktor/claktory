import 'package:flutter/widgets.dart';

import 'attachments.dart' show FileDrop;
import 'chats.dart' show selectedChat;
import 'controls.dart';
import 'identity.dart';
import 'mobile/metrics.dart';
import 'scope.dart';

/// What dropping files on the window would do now, for the overlay.
enum DropWhere {
  /// Onto a card whose field has the keys (#511), to [DropPlace.name]:
  /// "your answer to June".
  card,

  /// Into the message box showing, to [DropPlace.name].
  box,

  /// The conversation showing takes text only (a chat).
  textOnly,

  /// The agent showing can't run, so nothing can be sent to it.
  blocked,

  /// No message box shows (Settings, a page, no agent chosen).
  none,
}

/// Where files dragged over the window would go, and to whom.
typedef DropPlace = ({DropWhere where, String name, Color? color});

/// Over the whole Mac window while files from the Finder are dragged over
/// it: the window dims under a 30% scrim, edged in the color of whoever
/// they'd go to, and a card in its middle says where they'll go and how
/// many ("Drop to add to your message to June", "3 files"), or why they
/// can't go anywhere here (a chat takes text only; no conversation is
/// open). It fades in and out in `fast`, never takes the pointer, and goes
/// once they're dropped or leave; the window's drag is native, so crossing
/// what's under it doesn't flicker it. Only a Mac's window takes drops
/// ([FileDrop.windowDrops]); a phone, an iPad or Android shows nothing.
class WindowDropOverlay extends StatefulWidget {
  const WindowDropOverlay({super.key});

  @override
  State<WindowDropOverlay> createState() => _WindowDropOverlayState();
}

class _WindowDropOverlayState extends State<WindowDropOverlay> {
  /// How many files it last said, kept while it fades out.
  var _count = 0;

  /// In the tree: shown, or fading out.
  var _present = false;

  DropPlace? _place;

  DropPlace _placeNow(BuildContext context) {
    final takes = FileDrop.takesFiles.value;
    final workspace = AppScope.of(context).workspace;
    if (FileDrop.cardOver.value ?? FileDrop.divertedTo.value case final card?) {
      return (where: DropWhere.card, name: card, color: null);
    }
    if (takes == null) return (where: DropWhere.none, name: '', color: null);
    if (!takes || selectedChat(workspace.workspace) != null) {
      return (where: DropWhere.textOnly, name: '', color: null);
    }
    // The agent of the box that takes them, else the one showing.
    final agent =
        workspace.agent(FileDrop.claimedKey ?? '') ?? workspace.selectedAgent;
    return (
      where: agent != null && !agent.modelAvailable
          ? DropWhere.blocked
          : DropWhere.box,
      name: agent?.name ?? '',
      color: agent == null ? null : agentColor(agent.color),
    );
  }

  @override
  Widget build(BuildContext context) {
    if (!FileDrop.windowDrops || isPhoneLayout(context)) {
      return const SizedBox.shrink();
    }
    return IgnorePointer(
      // Rebuilt as files come and go; what takes them is read then (boxes
      // claim the window while they're built, so it isn't listened to).
      child: ListenableBuilder(
        // Where they are too: over a card, they'd go to it (#511).
        listenable: Listenable.merge([FileDrop.count, FileDrop.cardOver]),
        builder: (context, _) {
          final count = FileDrop.count.value;
          final shown = count > 0;
          if (shown) {
            _count = count;
            _present = true;
            _place = _placeNow(context);
          }
          if (!_present || _place == null) return const SizedBox.shrink();
          return TweenAnimationBuilder<double>(
            tween: Tween(begin: 0, end: shown ? 1 : 0),
            duration: Motion.fast,
            curve: Curves.easeOut,
            onEnd: () {
              if (!shown && mounted) setState(() => _present = false);
            },
            builder: (context, t, child) => Opacity(opacity: t, child: child),
            child: _scrim(context, _place!, _count),
          );
        },
      ),
    );
  }

  Widget _scrim(BuildContext context, DropPlace place, int count) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final takes = place.where == DropWhere.box || place.where == DropWhere.card;
    final color = takes ? place.color ?? palette.signal : palette.textMuted;
    final (String title, String? note) = switch (place.where) {
      DropWhere.card => (
        'Drop to add to ${place.name}',
        count > 1 ? '$count files' : null,
      ),
      DropWhere.box => (
        place.name.isEmpty
            ? 'Drop to add to your message'
            : 'Drop to add to your message to ${place.name}',
        count > 1 ? '$count files' : null,
      ),
      DropWhere.textOnly => (
        "Files can't be added to a chat",
        'Chats take text only. Open an agent to send it files.',
      ),
      DropWhere.blocked => (
        "Files can't be added now",
        "No model available, so ${place.name} can't run.",
      ),
      DropWhere.none => (
        'Open a conversation to add files',
        'Files dropped here go to the message box of the agent showing.',
      ),
    };
    return Container(
      decoration: BoxDecoration(
        color: const Color(0x4D000000),
        border: takes ? Border.all(color: color, width: 2) : null,
      ),
      alignment: Alignment.center,
      child: Container(
        constraints: const BoxConstraints(maxWidth: 420),
        padding: const EdgeInsets.symmetric(
          horizontal: Space.xl,
          vertical: Space.l,
        ),
        decoration: BoxDecoration(
          color: palette.sidebar,
          border: Border.all(color: palette.hairline),
          borderRadius: BorderRadius.circular(Radii.floating),
          boxShadow: floatingShadow,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 56,
              height: 56,
              decoration: BoxDecoration(
                color: color.withValues(alpha: 0.14),
                shape: BoxShape.circle,
              ),
              alignment: Alignment.center,
              child: ExtensionGlyph(
                icon: 'upload',
                name: 'Upload',
                color: color,
                size: 28,
              ),
            ),
            const SizedBox(height: Space.m),
            Text(
              title,
              textAlign: TextAlign.center,
              style: type.emphasis.copyWith(color: palette.text),
            ),
            if (note != null)
              Text(
                note,
                textAlign: TextAlign.center,
                style: type.caption.copyWith(color: palette.textMuted),
              ),
          ],
        ),
      ),
    );
  }
}
