// Alerts kept in Needs you (Bruno 44236, from Matt; Secco's contract
// 44270; Lucy's needs-you-cards.md §3.9; Trish's words 44382): an agent's
// notify_person, or Claktory's own, waits in Needs you until the person
// reads it, so none is lost with its notification. Nothing waits on a
// decision, so an alert is a light row of its own, never a card: who sent
// it and when, Urgent when it is, its title and words whole, and Mark as
// read. Urgent ones stand alone above the cards; the rest go under them,
// under Alerts, three or more from one agent folded into one row.
//
// What's unread is the backend's (Workspace.alerts); reading one sends
// ReadAlerts, and it leaves for History. The app keeps only what this
// page showed and read here, to mark it Read until the person leaves.
import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import 'ask_card.dart' show cardTime;
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'identity.dart';
import 'inbox.dart' show NeedsYouItem, NeedsYouKind;
import 'mobile/drawer.dart' show largeText;
import 'mobile/metrics.dart' show Metrics;
import 'name_role_line.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// From how many alerts one agent's (none urgent) fold into one row.
const alertFoldAt = 3;

/// How many alerts wait, each counted once (the badge's): all of them,
/// though Workspace.alerts holds only the newest 100.
int unreadAlerts(Workspace workspace) =>
    math.max(workspace.alertsUnread, workspace.alerts.length);

/// The alerts as Needs you items, newest first: each urgent one alone,
/// then the rest, one item each, or one per sender that has
/// [alertFoldAt] or more.
List<NeedsYouItem> alertItems(Workspace workspace) {
  final newest = workspace.alerts.reversed.toList();
  final counts = <String, int>{};
  for (final alert in newest) {
    if (!alert.urgent) counts[alert.agentId] = (counts[alert.agentId] ?? 0) + 1;
  }
  final folded = <String>{};
  final rest = <NeedsYouItem>[];
  for (final alert in newest) {
    if (alert.urgent) continue;
    if ((counts[alert.agentId] ?? 0) < alertFoldAt) {
      rest.add(NeedsYouItem.alert(alert.agentId, alert.id));
    } else if (folded.add(alert.agentId)) {
      rest.add(NeedsYouItem.alertGroup(alert.agentId));
    }
  }
  return [
    for (final alert in newest)
      if (alert.urgent) NeedsYouItem.alert(alert.agentId, alert.id),
    ...rest,
  ];
}

/// The alerts [item] stands for among [known] (unread ones, and those read
/// here), newest first.
List<Alert> alertsOf(Iterable<Alert> known, NeedsYouItem item) {
  final alerts = [
    for (final alert in known)
      if (item.kind == NeedsYouKind.alert
          ? alert.id == item.sessionId
          : item.kind == NeedsYouKind.alertGroup &&
                !alert.urgent &&
                alert.agentId == item.agentId)
        alert,
  ];
  alerts.sort((a, b) => b.sentMs.compareTo(a.sentMs));
  return alerts;
}

/// Whether [item] is an urgent alert: it goes above the cards.
bool isUrgentAlert(Iterable<Alert> known, NeedsYouItem item) =>
    item.kind == NeedsYouKind.alert &&
    alertsOf(known, item).any((alert) => alert.urgent);

/// Whether this device can mark [alert] read: Claktory's alert about this
/// device itself only the person's other devices can.
bool readableHere(Backend backend, Alert alert) =>
    alert.deviceId.isEmpty || alert.deviceId != backend.deviceId;

/// What an alert about this device says in Mark as read's place.
const aboutThisDevice =
    'This alert is about this device. Mark it read on another of your '
    'devices.';

/// Who sent [alert], as it was sent: its agent's name then, or this
/// Claktory's for its own.
String alertFrom(Alert alert) => alert.fromName.isNotEmpty
    ? alert.fromName
    : alert.agentId.isEmpty
    ? 'Claktory'
    : 'An agent';

/// What [alert] says in a line, for find and focus mode's queue: its
/// title, or that it's in its agent's conversation when withheld.
String alertSummary(Alert alert) => alert.withheld
    ? withheldTitle(alertFrom(alert))
    : alert.title.isEmpty
    ? alert.text
    : alert.title;

/// A withheld alert's words: who sent it, never what it says (its agent
/// had read a web page or another Claktory's words).
String withheldTitle(String from) => '$from sent you an alert';

/// Where a withheld alert's words are.
String withheldLine(String from) => "Read it in $from's conversation.";

/// Opens [alert]'s agent's conversation at it (its pipe's, for a liaison),
/// leaving Needs you; Claktory's own has none.
void openAlert(BuildContext context, Alert alert) {
  if (alert.agentId.isEmpty) return;
  final backend = AppScope.of(context);
  final agent = backend.workspace.workspace.agents
      .where((agent) => agent.id == alert.agentId)
      .firstOrNull;
  if (agent == null) return;
  AppScope.closePage(context);
  fireAndLog(() async {
    if (agent.pipeId.isNotEmpty) {
      await backend.selectPipe(agent.pipeId);
    } else {
      await backend.selectAgent(agent.id);
    }
    if (alert.seq > 0) backend.reveal.reachSeq(agent.id, alert.seq.toInt());
  }());
}

/// An alert in Needs you, or several from one agent folded: a light row,
/// no frame, no field and no primary button (§3.9).
class NeedsYouAlert extends StatelessWidget {
  const NeedsYouAlert({
    super.key,
    required this.item,
    required this.alerts,
    required this.unread,
    required this.open,
    required this.onToggle,
    required this.onRead,
    this.highlighted = false,
  });

  final NeedsYouItem item;

  /// What it stands for, newest first ([alertsOf]).
  final List<Alert> alerts;

  /// The ids of those still unread.
  final Set<String> unread;

  /// A fold, opened in place.
  final bool open;
  final VoidCallback onToggle;

  /// Reads these (Mark as read, opening one).
  final ValueChanged<List<Alert>> onRead;

  /// Just opened from its notification: it stands out a moment.
  final bool highlighted;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final child = item.kind == NeedsYouKind.alertGroup
        ? _group(context)
        : alerts.isEmpty
        ? const SizedBox.shrink()
        : _AlertLine(
            alert: alerts.first,
            read: !unread.contains(alerts.first.id),
            onRead: () => onRead([alerts.first]),
          );
    return AnimatedContainer(
      duration: Motion.base,
      curve: Curves.easeOut,
      decoration: BoxDecoration(
        color: highlighted
            ? palette.signal.withValues(alpha: 0.12)
            : const Color(0x00000000),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: child,
    );
  }

  Widget _group(BuildContext context) {
    final palette = Palette.of(context);
    final latest = alerts.firstOrNull;
    final left = [
      for (final alert in alerts)
        if (unread.contains(alert.id)) alert,
    ];
    final backend = AppScope.of(context);
    final readable = [
      for (final alert in left)
        if (readableHere(backend, alert)) alert,
    ];
    final from = latest == null ? 'Claktory' : alertFrom(latest);
    final header = Semantics(
      button: true,
      expanded: open,
      child: Pressable(
        onTap: onToggle,
        builder: (context, hovered) => Container(
          constraints: const BoxConstraints(minHeight: 48),
          padding: const EdgeInsets.symmetric(
            horizontal: Space.m,
            vertical: Space.s,
          ),
          decoration: BoxDecoration(
            color: hovered ? palette.hover : null,
            borderRadius: BorderRadius.circular(Radii.panel),
          ),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _Sender(agentId: item.agentId, read: left.isEmpty),
              const SizedBox(width: Space.s + 2),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    NameRoleLine(
                      name: from,
                      role: alertRole(context, item.agentId),
                      after: [
                        TextSpan(
                          text:
                              ' · ${alerts.length} '
                              '${alerts.length == 1 ? 'alert' : 'alerts'}',
                        ),
                      ],
                      wrap: largeText(context),
                      style: TextStyles.label.copyWith(
                        color: left.isEmpty ? palette.textMuted : palette.text,
                        fontWeight: FontWeight.w600,
                      ),
                      roleStyle: TextStyles.label.copyWith(
                        color: palette.textMuted,
                        fontWeight: FontWeight.w400,
                      ),
                    ),
                    if (latest != null)
                      Text(
                        alertSummary(latest),
                        maxLines: largeText(context) ? null : 2,
                        overflow: largeText(context)
                            ? null
                            : TextOverflow.ellipsis,
                        style: TextStyles.body.copyWith(
                          color: palette.textMuted,
                        ),
                      ),
                  ],
                ),
              ),
              const SizedBox(width: Space.s),
              if (left.isEmpty)
                Padding(
                  padding: const EdgeInsets.only(right: Space.s, top: 2),
                  child: _ReadMark(),
                ),
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: AnimatedRotation(
                  turns: open ? 0.25 : 0,
                  duration: Motion.fast,
                  curve: Curves.easeOut,
                  child: Chevron(
                    color: hovered ? palette.textMuted : palette.textFaint,
                    down: false,
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
    return AnimatedSize(
      duration: Motion.base,
      curve: Curves.easeOut,
      alignment: Alignment.topCenter,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          header,
          if (open) ...[
            for (final alert in alerts)
              Padding(
                // Under the fold's own sender, its alerts without one; at
                // the largest text, the whole width for their words.
                padding: EdgeInsets.only(
                  left: largeText(context) ? 0 : 24 + Space.s + 2,
                ),
                child: _AlertLine(
                  alert: alert,
                  read: !unread.contains(alert.id),
                  onRead: () => onRead([alert]),
                  sender: false,
                ),
              ),
            if (readable.length > 1)
              Padding(
                // Its words at the rows' text edge.
                padding: EdgeInsets.fromLTRB(
                  (largeText(context) ? 0 : 24 + Space.s + 2) + Space.m,
                  0,
                  Space.m,
                  Space.s,
                ),
                child: Align(
                  alignment: AlignmentDirectional.centerStart,
                  child: _QuietButton(
                    label: 'Mark all ${readable.length} as read',
                    onPressed: () => onRead(readable),
                  ),
                ),
              ),
          ],
        ],
      ),
    );
  }
}

/// "Also from Fugo and Pericolo", "Also from Fugo, Pericolo and June": the
/// agents whose alerts about the same thing folded into [alert], in the
/// order they came.
String alsoFromLine(Alert alert) => switch (alert.alsoFrom) {
  [] => '',
  [final one] => 'Also from $one',
  [...final rest, final last] => 'Also from ${rest.join(', ')} and $last',
};

/// The role of the agent [agentId] that sent an alert, to show beside its
/// name (§3.10); empty for Claktory's own, and an agent with none.
String alertRole(BuildContext context, String agentId) {
  if (agentId.isEmpty) return '';
  return AppScope.of(context).workspace.agent(agentId)?.role.trim() ?? '';
}

/// One alert: who and when (Urgent in danger), its title and words whole,
/// and Mark as read, or Read once it is. Clicked, it opens its agent's
/// conversation at it, which reads it.
class _AlertLine extends StatelessWidget {
  const _AlertLine({
    required this.alert,
    required this.read,
    required this.onRead,
    this.sender = true,
  });

  final Alert alert;
  final bool read;
  final VoidCallback onRead;

  /// With who sent it at its start; not in a fold, whose row has it.
  final bool sender;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final from = alertFrom(alert);
    final sent = alert.sentMs.toInt();
    final opens = alert.agentId.isNotEmpty;
    final here = readableHere(AppScope.of(context), alert);
    final lineStyle = TextStyles.label.copyWith(
      color: read ? palette.textMuted : palette.text,
      fontWeight: FontWeight.w600,
    );
    // Urgent first, so it never starts a wrapped line with its separator
    // (Lucy); then who, with its role beside it (§3.10), and when.
    final urgent = <InlineSpan>[
      if (alert.urgent) ...[
        TextSpan(
          text: 'Urgent',
          style: TextStyles.caption.copyWith(
            color: palette.danger,
            fontWeight: FontWeight.w600,
          ),
        ),
        if (sender || sent > 0)
          TextSpan(
            text: ' · ',
            style: TextStyles.caption.copyWith(
              color: palette.textFaint,
              fontWeight: FontWeight.w400,
            ),
          ),
      ],
    ];
    final when = <InlineSpan>[
      if (sent > 0)
        TextSpan(
          text: '${sender ? ' · ' : ''}${cardTime(context, sent)}',
          style: TextStyles.caption.copyWith(
            color: palette.textFaint,
            fontWeight: FontWeight.w400,
          ),
        ),
    ];
    final words = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (sender)
          NameRoleLine(
            before: urgent,
            name: from,
            role: alertRole(context, alert.agentId),
            after: when,
            wrap: largeText(context),
            style: lineStyle,
            roleStyle: TextStyles.label.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w400,
            ),
          )
        else
          Text.rich(
            TextSpan(
              children: [
                if (alert.urgent) ...[
                  TextSpan(
                    text: 'Urgent',
                    style: TextStyles.caption.copyWith(
                      color: palette.danger,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  if (sender || sent > 0)
                    TextSpan(
                      text: ' · ',
                      style: TextStyles.caption.copyWith(
                        color: palette.textFaint,
                        fontWeight: FontWeight.w400,
                      ),
                    ),
                ],
                if (sender) TextSpan(text: from),
                if (sent > 0)
                  TextSpan(
                    text: '${sender ? ' · ' : ''}${cardTime(context, sent)}',
                    style: TextStyles.caption.copyWith(
                      color: palette.textFaint,
                      fontWeight: FontWeight.w400,
                    ),
                  ),
              ],
            ),
            style: TextStyles.label.copyWith(
              color: read ? palette.textMuted : palette.text,
              fontWeight: FontWeight.w600,
            ),
          ),
        if (alert.withheld) ...[
          Text(
            withheldTitle(from),
            style: TextStyles.body.copyWith(
              color: read ? palette.textMuted : palette.text,
              fontWeight: FontWeight.w500,
            ),
          ),
          Text(
            withheldLine(from),
            style: TextStyles.body.copyWith(color: palette.textMuted),
          ),
          if (opens)
            Padding(
              padding: const EdgeInsets.only(top: Space.xs),
              child: Align(
                alignment: AlignmentDirectional.centerStart,
                child: _QuietButton(
                  label: 'Open conversation',
                  onPressed: () => _open(context),
                ),
              ),
            ),
        ] else ...[
          if (alert.title.isNotEmpty)
            Text(
              alert.title,
              style: TextStyles.body.copyWith(
                color: read ? palette.textMuted : palette.text,
                fontWeight: FontWeight.w500,
              ),
            ),
          if (alert.text.isNotEmpty)
            Text(
              alert.text,
              style: TextStyles.body.copyWith(color: palette.textMuted),
            ),
          // Others' alerts about the same thing, folded into this one
          // Their words aren't kept, only who.
          if (alert.alsoFrom.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(
                alsoFromLine(alert),
                style: TextStyles.caption.copyWith(color: palette.textFaint),
              ),
            ),
        ],
        if (!read && !here)
          Padding(
            padding: const EdgeInsets.only(top: 2),
            child: Text(
              aboutThisDevice,
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          ),
      ],
    );
    final trailing = read
        ? Padding(padding: const EdgeInsets.only(top: 2), child: _ReadMark())
        : here
        ? _QuietButton(label: 'Mark as read', onPressed: onRead)
        : null;
    final stacked = largeText(context);
    final row = Container(
      constraints: const BoxConstraints(minHeight: 48),
      padding: const EdgeInsets.symmetric(
        horizontal: Space.m,
        vertical: Space.s,
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (sender) ...[
            _Sender(agentId: alert.agentId, read: read),
            const SizedBox(width: Space.s + 2),
          ],
          Expanded(
            child: stacked
                // At the largest text, Mark as read under the words: beside
                // them there's no room.
                ? Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    spacing: Space.xs,
                    children: [words, ?trailing],
                  )
                : words,
          ),
          if (!stacked && trailing != null) ...[
            const SizedBox(width: Space.m),
            trailing,
          ],
        ],
      ),
    );
    if (!opens) return row;
    return Semantics(
      button: true,
      hint: 'Open conversation',
      child: Pressable(
        onTap: () => _open(context),
        builder: (context, hovered) => DecoratedBox(
          decoration: BoxDecoration(
            color: hovered ? palette.hover : null,
            borderRadius: BorderRadius.circular(Radii.panel),
          ),
          child: row,
        ),
      ),
    );
  }

  /// Opening it reads it, where it can be read.
  void _open(BuildContext context) {
    if (!read && readableHere(AppScope.of(context), alert)) onRead();
    openAlert(context, alert);
  }
}

/// Who sent it: its agent's character, or Claktory's cog for its own.
class _Sender extends StatelessWidget {
  const _Sender({required this.agentId, required this.read});

  final String agentId;
  final bool read;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final agent = agentId.isEmpty
        ? null
        : AppScope.of(context).workspace.workspace.agents
              .where((agent) => agent.id == agentId)
              .firstOrNull;
    return SizedBox.square(
      dimension: 24,
      child: Center(
        child: agent != null
            ? AgentCharacter(agent: agent, size: 24)
            : agentId.isEmpty
            ? CogGlyph(color: read ? palette.textFaint : palette.textMuted)
            : ExtensionGlyph(icon: 'user', name: '', color: palette.textFaint),
      ),
    );
  }
}

/// "Read", once it is.
class _ReadMark extends StatelessWidget {
  @override
  Widget build(BuildContext context) => Text(
    'Read',
    style: TextStyles.caption.copyWith(color: Palette.of(context).textFaint),
  );
}

/// Mark as read: quiet words, no fill, on every device (Lucy: nothing here
/// is decided, so it shouldn't look like a button to press), a finger's
/// target where touch comes first.
class _QuietButton extends StatelessWidget {
  const _QuietButton({required this.label, required this.onPressed});

  final String label;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final touch = touchFirst(context);
    return Semantics(
      button: true,
      label: label,
      onTap: onPressed,
      excludeSemantics: true,
      child: Pressable(
        onTap: onPressed,
        builder: (context, hovered) => ConstrainedBox(
          constraints: BoxConstraints(
            minHeight: touch ? Metrics.of(context).target : 0,
          ),
          child: Align(
            alignment: AlignmentDirectional.topStart,
            widthFactor: 1,
            heightFactor: 1,
            child: Text(
              label,
              style: TextStyles.label.copyWith(
                color: hovered ? palette.text : palette.textMuted,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
        ),
      ),
    );
  }
}
