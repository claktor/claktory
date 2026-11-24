// Replying on a checklist item (Matt's ask): a field under the item, in
// Needs you and in the Checklist panel, whose words reach the agent as the
// person's own message about that item; and the reference such a message
// carries in the conversation, which opens the item.
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'dictation.dart';
import 'identity.dart';
import 'mobile/lines.dart';
import 'mobile/metrics.dart' show Touch, TypeScale;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart' show Unselectable;

/// Shows item [id] of [agentId]'s checklist: that agent's conversation with
/// its Checklist beside it, scrolled to the item, glowing.
Future<void> showChecklistItem(Backend backend, String agentId, int id) async {
  final agent = backend.workspace.agent(agentId);
  await backend.selectAgent(agentId);
  await backend.openPanel(agentId, 'checklist');
  final key = 'checklist_item:$agentId#$id';
  // Pointed at, so the checklist scrolls to it, glow or not.
  backend.highlights.pointedAt.value = (keys: [key], at: DateTime.now());
  backend.highlights.glow(key, [agentColor(agent?.color ?? 'slate')]);
}

/// Who gets a reply on [item] of [checklistAgentId]'s checklist: the agent
/// it's assigned to, else the checklist's own.
String checklistReplyTo(String checklistAgentId, String owner) =>
    owner.isEmpty ? checklistAgentId : owner;

/// A field to reply on item [itemId] of [checklistAgentId]'s checklist,
/// with Send. What's typed goes to [owner] (the checklist's agent when
/// empty) as the person's message, tied to the item; it changes nothing on
/// the item itself but its history. Enter sends; the field empties once the
/// backend has it.
class ChecklistReply extends StatefulWidget {
  const ChecklistReply({
    super.key,
    required this.checklistAgentId,
    required this.itemId,
    this.owner = '',
  });

  final String checklistAgentId;
  final int itemId;
  final String owner;

  @override
  State<ChecklistReply> createState() => _ChecklistReplyState();
}

class _ChecklistReplyState extends State<ChecklistReply> {
  final _text = TextEditingController();
  final _focus = FocusNode(debugLabel: 'Checklist reply');
  var _busy = false;
  String? _error;

  /// Who it went to, once sent, until something more is typed.
  String? _sentTo;

  @override
  void initState() {
    super.initState();
    _text.addListener(_typed);
  }

  @override
  void dispose() {
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _typed() {
    if (_sentTo != null && _text.text.isNotEmpty) {
      setState(() => _sentTo = null);
    }
  }

  String get _to => checklistReplyTo(widget.checklistAgentId, widget.owner);

  Future<void> _send() async {
    final text = _text.text.trim();
    if (_busy || text.isEmpty) return;
    final backend = AppScope.of(context);
    final name = backend.workspace.agent(_to)?.name ?? 'the agent';
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await backend.sendMessage(
        _to,
        text,
        checklistItem: widget.itemId,
        checklistAgentId: widget.checklistAgentId,
      );
      if (!mounted) return;
      _text.clear();
      setState(() => _sentTo = name);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final name = AppScope.of(context).workspace.agent(_to)?.name ?? 'The agent';
    final sentTo = _sentTo;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Or said: the mic puts what's said in, to check before sending.
        Dictation(
          controller: _text,
          focusNode: _focus,
          enabled: !_busy,
          // A recording not sent yet comes back here after a relaunch.
          keep: DictationFor(
            'checklist/${widget.checklistAgentId}/${widget.itemId}',
            'Your reply to $name on item ${widget.itemId}',
          ),
          child: CallbackShortcuts(
            bindings: {const SingleActivator(LogicalKeyboardKey.enter): _send},
            // Its placeholder isn't copied with the words around it.
            child: Unselectable(
              child: TextField(
                controller: _text,
                focusNode: _focus,
                placeholder: 'Reply about this item',
                semanticsLabel: 'Reply to $name about this item',
                minLines: 1,
                maxLines: 8,
                // Brought into view over a phone's keyboard with Send and
                // the line beside it under it, not only the field.
                scrollPadding: EdgeInsets.fromLTRB(
                  20,
                  20,
                  20,
                  20 +
                      Space.s +
                      Touch.target +
                      MediaQuery.textScalerOf(context).scale(48),
                ),
              ),
            ),
          ),
        ),
        const SizedBox(height: Space.s),
        ActionLine(
          lead: Text(
            _error ??
                (sentTo != null
                    ? 'Sent to $sentTo.'
                    : '$name gets it as your message about this item.'),
            style: type.caption.copyWith(
              color: _error == null ? palette.textFaint : palette.danger,
            ),
          ),
          action: ListenableBuilder(
            listenable: _text,
            builder: (context, _) => Button(
              label: 'Send',
              primary: true,
              onPressed: _busy || _text.text.trim().isEmpty ? null : _send,
            ),
          ),
        ),
      ],
    );
  }
}

/// Over one of the person's messages, the checklist item it's about: its
/// words as they were when sent. A click shows the item on
/// its agent's checklist (the app knows a checklist's items only while it's
/// open, so one since gone shows the checklist without it); with its agent
/// gone, it's only words.
class ChecklistRef extends StatelessWidget {
  const ChecklistRef(this.ref, {super.key, this.color});

  final ChecklistItemRef ref;

  /// The words' color; the palette's faint text by default.
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final agent = backend.workspace.agent(ref.agentId);
    final words = 'About: ${ref.text}';
    Widget line(Color color) => Text(
      words,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: TypeScale.of(context).caption.copyWith(color: color),
    );
    if (agent == null) return line(color ?? palette.textFaint);
    return Semantics(
      link: true,
      label: '$words. Show in checklist',
      excludeSemantics: true,
      child: Pressable(
        onTap: () =>
            fireAndLog(showChecklistItem(backend, ref.agentId, ref.id)),
        builder: (context, hovered) =>
            line(hovered ? palette.textMuted : color ?? palette.textFaint),
      ),
    );
  }
}
