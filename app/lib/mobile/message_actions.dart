import 'dart:async';

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:flutter/rendering.dart' show CustomSemanticsAction;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../identity.dart';
import '../markdown.dart';
import '../selection.dart';
import '../transcript.dart' show AgentTints;
import 'haptics.dart';
import 'metrics.dart';
import 'presentation.dart';

/// What can be done with a message in a conversation on a phone (research
/// mobile.md §4, "Message actions"): touch and hold it for Reply (where
/// there's a message box to reply in), Copy and Select text (and Report…,
/// for the agent's own reply), the way the
/// iPhone's chat apps offer them. VoiceOver finds the same among the
/// message's actions, so nothing needs a hold.
///
/// Select text opens the message on its own, where the iPhone's handles
/// select any part of it; in the conversation, a double tap still
/// selects. Only what works is offered: nothing to edit, retry or delete,
/// which the backend doesn't do.
///
/// Elsewhere (the Mac) it's [child] as it is: there the pointer selects
/// and the menu is the right click's.
class MessageActions extends StatelessWidget {
  const MessageActions({
    super.key,
    required this.text,
    required this.child,
    this.onReply,
    this.onReport,
    this.markdown = true,
  });

  /// The message's words, as sent (Markdown for an agent's).
  final String text;

  /// Whether [text] is Markdown, shown rendered (an agent's reply or
  /// message); false for the person's own, shown as typed. Copy gives what
  /// shows: Markdown's words without its markers.
  final bool markdown;

  /// Quotes the message into the message box (Drafts.reply); null where
  /// there's no box to reply in.
  final VoidCallback? onReply;

  /// Reports the message to Claktory's team: the agent's own reply only;
  /// null for anything else.
  final VoidCallback? onReport;

  final Widget child;

  @override
  Widget build(BuildContext context) {
    if (!touchFirst(context) || text.trim().isEmpty) return child;
    final selecting = MessageSelection.of(context);
    Widget hold(bool selected, Widget? child) => GestureDetector(
      // While words are selected, a hold is the selection's, as on iOS.
      onLongPress: selected ? null : () => _menu(context),
      child: child,
    );
    return Semantics(
      container: true,
      customSemanticsActions: {
        const CustomSemanticsAction(label: 'Reply'): ?onReply,
        const CustomSemanticsAction(label: 'Copy'): () =>
            copyMessage(_copied(context)),
        const CustomSemanticsAction(label: 'Select text'): () =>
            showMessageText(context, text, markdown: markdown),
        const CustomSemanticsAction(label: 'Report'): ?onReport,
      },
      child: selecting == null
          ? hold(false, child)
          : ValueListenableBuilder(
              valueListenable: selecting,
              builder: (context, selected, child) => hold(selected, child),
              child: child,
            ),
    );
  }

  /// What Copy puts on the clipboard: what a selection of the whole
  /// message copies, an @mention as its agent's name.
  String _copied(BuildContext context) => markdown
      ? markdownToPlain(
          text,
          agents:
              context
                  .getInheritedWidgetOfExactType<AgentTints>()
                  ?.agents
                  .values ??
              const [],
        )
      : text;

  Future<void> _menu(BuildContext context) async {
    unawaited(Haptics.impact());
    final chosen = await showActions<_Action>(
      context,
      actions: [
        if (onReply != null) const SheetAction('Reply', _Action.reply),
        const SheetAction('Copy', _Action.copy),
        const SheetAction('Select text', _Action.select),
        if (onReport != null) const SheetAction('Report…', _Action.report),
      ],
    );
    if (!context.mounted) return;
    switch (chosen) {
      case _Action.reply:
        onReply?.call();
      case _Action.copy:
        await copyMessage(_copied(context));
      case _Action.select:
        await showMessageText(context, text, markdown: markdown);
      case _Action.report:
        onReport?.call();
      case null:
    }
  }
}

enum _Action { reply, copy, select, report }

/// Whether words are selected in the conversation around a message
/// ([selecting]): once a selection has started, a touch and hold belongs to
/// it (iOS moves or starts a selection), not to the message's actions.
class MessageSelection extends InheritedWidget {
  const MessageSelection({
    super.key,
    required this.selecting,
    required super.child,
  });

  final ValueListenable<bool> selecting;

  static ValueListenable<bool>? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<MessageSelection>()?.selecting;

  @override
  bool updateShouldNotify(MessageSelection old) => selecting != old.selecting;
}

/// Puts a message's words on the clipboard.
Future<void> copyMessage(String text) async {
  await Clipboard.setData(ClipboardData(text: text));
  unawaited(Haptics.success());
}

/// A message on its own in a tall sheet, its words selectable with the
/// iPhone's handles and menu.
Future<void> showMessageText(
  BuildContext context,
  String text, {
  bool markdown = true,
}) => showPanel(
  context,
  title: 'Select text',
  tall: true,
  builder: (context, close) => SingleChildScrollView(
    padding: EdgeInsets.only(bottom: Metrics.of(context).gutter),
    child: SelectableArea(
      child: DefaultTextStyle.merge(
        style: TypeScale.of(context).body
            .copyWith(color: Palette.of(context).text),
        child: markdown ? Markdown(text) : Text(text),
      ),
    ),
  ),
);
