import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import 'characters/character.dart';
import 'backend.dart';
import 'controls.dart';
import 'database.dart';
import 'highlights.dart';
import 'identity.dart';
import 'memory.dart';
import 'messaging.dart';
import 'monitors.dart';
import 'resources.dart';
import 'tasks.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart';
import 'selection.dart';

/// Width of the extension bar between the sidebar and the content.
const extensionBarWidth = 40.0;

/// Width of an extension's panel on the right.
const panelWidth = 320.0;

/// [agent]'s settings for extension [id], or null when it has none.
AgentExtension? agentExtension(Agent agent, String id) {
  for (final extension in agent.extensions) {
    if (extension.id == id) return extension;
  }
  return null;
}

/// The extensions turned on for [agent], in the workspace's order.
List<Extension> enabledExtensions(Workspace workspace, Agent agent) => [
  for (final extension in workspace.extensions)
    if (agentExtension(agent, extension.id)?.enabled ?? false) extension,
];

/// The extension whose panel shows beside [agent]: the one it has open
/// (each agent remembers its own), if [agent] has that extension on.
Extension? openPanelOf(Workspace workspace, Agent? agent) {
  if (agent == null || agent.openPanel.isEmpty) return null;
  for (final extension in enabledExtensions(workspace, agent)) {
    if (extension.id == agent.openPanel) return extension;
  }
  return null;
}

/// A thin column right of the sidebar with an icon for each extension the
/// selected agent has on. Clicking one opens its panel; clicking the open
/// one closes it.
class ExtensionBar extends StatelessWidget {
  const ExtensionBar({super.key, required this.agent});

  /// The selected agent; null shows an empty bar, so nothing jumps.
  final Agent? agent;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final workspace = backend.workspace.workspace;
    final agent = this.agent;
    final open = openPanelOf(workspace, agent);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border(right: BorderSide(color: palette.hairline)),
      ),
      child: SizedBox(
        width: extensionBarWidth,
        child: Column(
          children: [
            const TitleSpace(),
            if (agent != null)
              for (final extension in enabledExtensions(workspace, agent))
                Padding(
                  padding: const EdgeInsets.only(bottom: Space.xs),
                  // Glows when it's pointed out, and for its panel's items
                  // while the panel is closed.
                  child: Glow(
                    store: backend.highlights,
                    matches: (key) =>
                        key == 'extension:${agent.id}/${extension.id}' ||
                        (open?.id != extension.id &&
                            extension.id == 'checklist' &&
                            key.startsWith('checklist_item:${agent.id}#')),
                    child: HoverCard(
                      beside: true,
                      card: (context) => Text(extension.name),
                      child: GlyphButton(
                        label: extension.name,
                        active: open?.id == extension.id,
                        onTap: () => fireAndLog(
                          backend.openPanel(
                            agent.id,
                            open?.id == extension.id ? '' : extension.id,
                          ),
                        ),
                        glyph: (color) => ExtensionGlyph(
                          icon: extension.icon,
                          name: extension.name,
                          color: color,
                        ),
                      ),
                    ),
                  ),
                ),
          ],
        ),
      ),
    );
  }
}

/// An extension's panel on the right of the window, beside [agent]: its
/// title and a close button, over what the backend sends for it.
class ExtensionPanel extends StatelessWidget {
  const ExtensionPanel({
    super.key,
    required this.extension,
    required this.agent,
  });

  final Extension extension;
  final Agent agent;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border(left: BorderSide(color: palette.hairline)),
      ),
      child: SizedBox(
        width: panelWidth,
        child: ListenableBuilder(
          listenable: backend.extensionView,
          builder: (context, _) {
            final view = backend.extensionView.of(extension.id, agent.id);
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                WindowDrag(
                  child: Container(
                    height: titleBarHeight,
                    padding: const EdgeInsets.fromLTRB(Space.l, 0, Space.s, 0),
                    decoration: BoxDecoration(
                      border: Border(
                        bottom: BorderSide(color: palette.hairline),
                      ),
                    ),
                    child: Row(
                      children: [
                        Expanded(
                          child: Text(
                            view == null || view.title.isEmpty
                                ? extension.name
                                : view.title,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyles.label.copyWith(
                              color: palette.text,
                            ),
                          ),
                        ),
                        GlyphButton(
                          label: 'Close ${extension.name}',
                          active: false,
                          onTap: () =>
                              fireAndLog(backend.openPanel(agent.id, '')),
                          glyph: (color) => CloseGlyph(color: color),
                        ),
                      ],
                    ),
                  ),
                ),
                Expanded(child: SelectableArea(child: _body(context, view))),
              ],
            );
          },
        ),
      ),
    );
  }

  Widget _body(BuildContext context, ExtensionView? view) {
    final palette = Palette.of(context);
    // Until the backend sends the view, there's nothing true to show.
    if (view == null) return const SizedBox.shrink();
    switch (view.whichBody()) {
      case ExtensionView_Body.checklist:
        return ChecklistView(checklist: view.checklist, agentId: view.agentId);
      case ExtensionView_Body.memory:
        return MemoryView(memory: view.memory);
      case ExtensionView_Body.messaging:
        return MessagingView(messaging: view.messaging, agentId: view.agentId);
      case ExtensionView_Body.tasks:
        return TasksView(tasks: view.tasks, agentId: view.agentId);
      case ExtensionView_Body.monitors:
        return MonitorsView(monitors: view.monitors, agentId: view.agentId);
      case ExtensionView_Body.database:
        return DatabaseView(database: view.database);
      case ExtensionView_Body.resources:
        return ResourcesView(resources: view.resources);
      case ExtensionView_Body.notSet:
        return Padding(
          padding: const EdgeInsets.all(Space.l),
          child: Text(
            'This panel needs a newer Claktory.',
            style: TextStyles.body.copyWith(color: palette.textFaint),
          ),
        );
    }
  }
}

/// An agent's checklist, read-only, like a chat: items grouped by the day
/// they were added, oldest at the top and today at the bottom, where it
/// opens and stays while items come and change. Scrolled up, it holds still
/// and offers a button back to the latest. Above the list, how much is
/// done; each day says how much of it is.
class ChecklistView extends StatefulWidget {
  const ChecklistView({
    super.key,
    required this.checklist,
    this.agentId = '',
    this.now,
  });

  final Checklist checklist;

  /// Whose checklist it is.
  final String agentId;

  /// What "today" is; the clock's time unless given.
  final DateTime? now;

  @override
  State<ChecklistView> createState() => _ChecklistViewState();
}

class _ChecklistViewState extends State<ChecklistView> {
  final _scroll = ScrollController();

  /// Following the bottom; false once the person scrolls up.
  var _pinned = true;

  /// Whether each item was finished at the last build, by id, to spot one
  /// that moved.
  final _finished = <int, bool>{};
  var _jumpScheduled = false;

  @override
  void dispose() {
    _scroll.dispose();
    super.dispose();
  }

  /// Puts the view at the bottom once this frame is laid out.
  void _followSoon() {
    if (_jumpScheduled) return;
    _jumpScheduled = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _jumpScheduled = false;
      if (!mounted || !_pinned || !_scroll.hasClients) return;
      final position = _scroll.position;
      if (position.pixels != position.maxScrollExtent) {
        position.jumpTo(position.maxScrollExtent);
      }
    });
  }

  bool _onScroll(ScrollNotification notification) {
    final metrics = notification.metrics;
    if (notification is ScrollUpdateNotification &&
        notification.dragDetails != null) {
      _setPinned(metrics.pixels >= metrics.maxScrollExtent - 8);
    } else if (notification is ScrollEndNotification) {
      _setPinned(metrics.pixels >= metrics.maxScrollExtent - 8);
    }
    return false;
  }

  void _setPinned(bool pinned) {
    if (pinned != _pinned) setState(() => _pinned = pinned);
  }

  void _toLatest() {
    setState(() => _pinned = true);
    if (!_scroll.hasClients) return;
    _scroll.animateTo(
      _scroll.position.maxScrollExtent,
      duration: Motion.base,
      curve: Curves.easeOut,
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final checklist = widget.checklist;
    if (checklist.items.isEmpty) {
      return Padding(
        padding: const EdgeInsets.all(Space.l),
        child: Text(
          'Nothing on the checklist yet.',
          style: TextStyles.body.copyWith(color: palette.textFaint),
        ),
      );
    }
    final now = widget.now ?? DateTime.now();
    DateTime dayOf(ChecklistItem item) {
      final added = DateTime.fromMillisecondsSinceEpoch(item.created.toInt());
      return DateTime(added.year, added.month, added.day);
    }

    bool finished(ChecklistItem item) =>
        item.status == ChecklistStatus.CHECKLIST_STATUS_DONE ||
        item.status == ChecklistStatus.CHECKLIST_STATUS_DROPPED;
    int byAdded(ChecklistItem a, ChecklistItem b) =>
        a.created.compareTo(b.created);
    // Everything added each day, for its count; only finished items are
    // listed under it.
    final days = <DateTime, List<ChecklistItem>>{};
    for (final item in checklist.items) {
      days.putIfAbsent(dayOf(item), () => []).add(item);
    }
    final history = [
      for (final day in days.keys.toList()..sort())
        if (days[day]!.any(finished))
          (day, days[day]!.where(finished).toList()..sort(byAdded)),
    ];
    // Outstanding: in progress first, then blocked, then open, each oldest
    // first.
    int rank(ChecklistItem item) => switch (item.status) {
      ChecklistStatus.CHECKLIST_STATUS_IN_PROGRESS => 0,
      ChecklistStatus.CHECKLIST_STATUS_BLOCKED => 1,
      _ => 2,
    };
    final outstanding =
        checklist.items.where((item) => !finished(item)).toList()..sort((a, b) {
          final first = rank(a).compareTo(rank(b));
          return first != 0 ? first : byAdded(a, b);
        });
    // An item that moved between the history and the outstanding ones
    // since the last build eases into its new place.
    final reduced = MediaQuery.disableAnimationsOf(context);
    final moved = <int>{
      for (final item in checklist.items)
        if (_finished[item.id] case final was?)
          if (was != finished(item)) item.id,
    };
    _finished
      ..clear()
      ..addAll({for (final item in checklist.items) item.id: finished(item)});
    final store = AppScope.of(context).workspace;
    // Keyed by item and section: the list keeps an item's state (an open
    // log) as others come and go, and an item that changes section starts
    // fresh in its new place, easing in.
    final highlights = AppScope.of(context).highlights;
    Widget item(ChecklistItem item, {String? added}) => Arrival(
      key: ValueKey((item.id, added == null)),
      animate: !reduced && moved.contains(item.id),
      child: Glow(
        store: highlights,
        matches: (key) => key == 'checklist_item:${widget.agentId}#${item.id}',
        side: AxisDirection.left,
        child: _Item(item: item, store: store, added: added),
      ),
    );
    if (_pinned) _followSoon();
    final caption = TextStyles.caption.copyWith(
      color: palette.textFaint,
      fontWeight: FontWeight.w500,
    );
    Widget heading(String left, String right) => Padding(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.m, Space.s, Space.xs),
      child: Row(
        children: [
          Expanded(child: Text(left, style: caption)),
          Text(right, style: caption),
        ],
      ),
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _Progress(items: checklist.items),
        Expanded(
          child: Stack(
            children: [
              NotificationListener<ScrollNotification>(
                onNotification: _onScroll,
                child: NotificationListener<ScrollMetricsNotification>(
                  // Growth (new items, an opened log) while following.
                  onNotification: (_) {
                    if (_pinned) _followSoon();
                    return false;
                  },
                  child: ListView(
                    controller: _scroll,
                    padding: const EdgeInsets.fromLTRB(
                      Space.s,
                      0,
                      Space.s,
                      Space.l,
                    ),
                    children: [
                      for (final (day, finishedThen) in history) ...[
                        heading(dayLabel(day, now), doneLabel(days[day]!)),
                        for (final done in finishedThen) item(done),
                      ],
                      if (outstanding.isNotEmpty) ...[
                        heading('Outstanding', '${outstanding.length}'),
                        for (final open in outstanding)
                          item(open, added: dayLabel(dayOf(open), now)),
                      ],
                    ],
                  ),
                ),
              ),
              Positioned(
                right: Space.m,
                bottom: Space.m,
                child: IgnorePointer(
                  ignoring: _pinned,
                  child: AnimatedOpacity(
                    opacity: _pinned ? 0 : 1,
                    duration: Motion.fast,
                    curve: Curves.easeOut,
                    child: _LatestButton(onTap: _toLatest),
                  ),
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// How much of [items] is done, for a day's heading: "2 of 3 done", "All 4
/// done", "All done", or "" when the day only has dropped items.
String doneLabel(List<ChecklistItem> items) {
  final counted = items
      .where((i) => i.status != ChecklistStatus.CHECKLIST_STATUS_DROPPED)
      .length;
  final done = items
      .where((i) => i.status == ChecklistStatus.CHECKLIST_STATUS_DONE)
      .length;
  if (counted == 0) return '';
  if (done == counted) return counted == 1 ? 'All done' : 'All $counted done';
  return '$done of $counted done';
}

/// The whole checklist's progress, above the list: a thin meter and how
/// many items are done, in progress, blocked and open. Dropped items don't
/// count.
class _Progress extends StatelessWidget {
  const _Progress({required this.items});

  final List<ChecklistItem> items;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    int count(ChecklistStatus status) =>
        items.where((item) => item.status == status).length;
    final done = count(ChecklistStatus.CHECKLIST_STATUS_DONE);
    final working = count(ChecklistStatus.CHECKLIST_STATUS_IN_PROGRESS);
    final blocked = count(ChecklistStatus.CHECKLIST_STATUS_BLOCKED);
    final dropped = count(ChecklistStatus.CHECKLIST_STATUS_DROPPED);
    final counted = items.length - dropped;
    final open = counted - done - working - blocked;
    final style = TextStyles.caption.copyWith(color: palette.textMuted);
    return Container(
      padding: const EdgeInsets.fromLTRB(Space.l, Space.s, Space.l, Space.s),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(
              [
                '$done of $counted done',
                if (working > 0) '$working in progress',
                if (blocked > 0) '$blocked blocked',
                if (open > 0) '$open open',
              ].join(' · '),
              style: style,
            ),
          ),
          Container(
            width: 48,
            height: 4,
            alignment: Alignment.centerLeft,
            decoration: BoxDecoration(
              color: palette.selected,
              borderRadius: BorderRadius.circular(2),
            ),
            child: FractionallySizedBox(
              widthFactor: counted == 0 ? 0 : done / counted,
              heightFactor: 1,
              child: DecoratedBox(
                decoration: BoxDecoration(
                  color: palette.textMuted,
                  borderRadius: BorderRadius.circular(2),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// A small round button with a down arrow, back to the latest.
class _LatestButton extends StatelessWidget {
  const _LatestButton({required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      label: 'Jump to the latest',
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          width: 28,
          height: 28,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: hovered ? palette.sidebar : palette.canvas,
            shape: BoxShape.circle,
            border: Border.all(color: palette.hairline),
            boxShadow: floatingShadow,
          ),
          child: ExtensionGlyph(
            icon: 'arrow-down',
            name: '',
            color: hovered ? palette.text : palette.textMuted,
          ),
        ),
      ),
    );
  }
}

class _Item extends StatefulWidget {
  const _Item({required this.item, required this.store, this.added});

  final ChecklistItem item;
  final WorkspaceStore store;

  /// The day it was added, shown when it isn't under that day's heading.
  final String? added;

  @override
  State<_Item> createState() => _ItemState();
}

class _ItemState extends State<_Item> {
  var _open = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final item = widget.item;
    final finished =
        item.status == ChecklistStatus.CHECKLIST_STATUS_DONE ||
        item.status == ChecklistStatus.CHECKLIST_STATUS_DROPPED;
    final owner = item.owner.isEmpty ? null : widget.store.agent(item.owner);
    return Pressable(
      onTap: item.log.isEmpty ? null : () => setState(() => _open = !_open),
      builder: (context, hovered) => Container(
        padding: const EdgeInsets.symmetric(
          horizontal: Space.s,
          vertical: Space.s - 2,
        ),
        decoration: BoxDecoration(
          color: hovered && item.log.isNotEmpty ? palette.hover : null,
          borderRadius: BorderRadius.circular(Radii.control),
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              // Centers the 14px mark on the first line of text.
              padding: const EdgeInsets.only(top: 2.5),
              child: StatusMark(status: item.status),
            ),
            const SizedBox(width: Space.s),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    item.text,
                    style: TextStyles.body.copyWith(
                      color: finished ? palette.textMuted : palette.text,
                      decoration:
                          item.status ==
                              ChecklistStatus.CHECKLIST_STATUS_DROPPED
                          ? TextDecoration.lineThrough
                          : null,
                      decorationColor: palette.textMuted,
                    ),
                  ),
                  // What a blocked item waits on.
                  if (item.status == ChecklistStatus.CHECKLIST_STATUS_BLOCKED &&
                      item.note.isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(top: 2),
                      child: Text(
                        item.note,
                        style: TextStyles.caption.copyWith(
                          color: palette.textFaint,
                        ),
                      ),
                    ),
                  const SizedBox(height: 2),
                  Row(
                    children: [
                      Text(
                        '#${item.id}',
                        style: TextStyles.caption.copyWith(
                          color: palette.textFaint,
                          fontFeatures: const [FontFeature.tabularFigures()],
                        ),
                      ),
                      const SizedBox(width: Space.s),
                      Text(
                        statusLabel(item.status),
                        style: TextStyles.caption.copyWith(
                          color: palette.textMuted,
                        ),
                      ),
                      const SizedBox(width: Space.s),
                      if (item.owner.isEmpty)
                        Flexible(
                          child: Text(
                            'Unassigned',
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyles.caption.copyWith(
                              color: palette.textFaint,
                            ),
                          ),
                        )
                      else ...[
                        owner == null
                            ? const UnknownCharacter(size: 20)
                            : AgentCharacter(agent: owner, size: 20),
                        const SizedBox(width: Space.xs),
                        Flexible(
                          child: Text(
                            owner?.name ?? item.owner,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyles.caption.copyWith(
                              color: palette.textMuted,
                            ),
                          ),
                        ),
                      ],
                      if (widget.added != null) ...[
                        const SizedBox(width: Space.s),
                        Flexible(
                          child: Text(
                            widget.added!,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyles.caption.copyWith(
                              color: palette.textFaint,
                            ),
                          ),
                        ),
                      ],
                      if (item.log.isNotEmpty) ...[
                        const Spacer(),
                        Chevron(color: palette.textFaint, down: _open),
                      ],
                    ],
                  ),
                  if (_open) ...[
                    const SizedBox(height: Space.s),
                    for (final update in item.log)
                      _LogLine(update: update, store: widget.store),
                  ],
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// One update in an item's log: when, who, what.
class _LogLine extends StatelessWidget {
  const _LogLine({required this.update, required this.store});

  final ChecklistUpdate update;
  final WorkspaceStore store;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final author = update.author.isEmpty ? null : store.agent(update.author);
    final who = update.author.isEmpty ? 'You' : author?.name ?? update.author;
    final time = DateTime.fromMillisecondsSinceEpoch(update.time.toInt());
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.xs),
      child: Text.rich(
        TextSpan(
          children: [
            TextSpan(
              text: '${logTime(time)}  ',
              style: TextStyle(
                color: palette.textFaint,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
            TextSpan(
              text: '$who  ',
              style: TextStyle(
                color: author == null
                    ? palette.textMuted
                    : agentColor(author.color),
                fontWeight: FontWeight.w500,
              ),
            ),
            TextSpan(
              text: update.text,
              style: TextStyle(color: palette.textMuted),
            ),
          ],
        ),
        style: TextStyles.caption,
      ),
    );
  }
}

/// A 14px mark for an item's status: an empty ring when open, a half-filled
/// signal ring while in progress, a paused ring while blocked, a check when
/// done, a struck ring when dropped.
class StatusMark extends StatelessWidget {
  const StatusMark({super.key, required this.status});

  final ChecklistStatus status;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      label: statusLabel(status),
      child: CustomPaint(
        size: const Size.square(14),
        painter: _StatusPainter(
          status: status,
          color: switch (status) {
            ChecklistStatus.CHECKLIST_STATUS_IN_PROGRESS => palette.signal,
            ChecklistStatus.CHECKLIST_STATUS_DROPPED => palette.textFaint,
            _ => palette.textMuted,
          },
        ),
      ),
    );
  }
}

class _StatusPainter extends CustomPainter {
  const _StatusPainter({required this.status, required this.color});

  final ChecklistStatus status;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final center = size.center(Offset.zero);
    final radius = size.shortestSide / 2 - 0.75;
    final stroke = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round;
    canvas.drawCircle(center, radius, stroke);
    switch (status) {
      case ChecklistStatus.CHECKLIST_STATUS_IN_PROGRESS:
        canvas.drawArc(
          Rect.fromCircle(center: center, radius: radius - 2),
          -math.pi / 2,
          math.pi,
          true,
          Paint()..color = color,
        );
      case ChecklistStatus.CHECKLIST_STATUS_DONE:
        canvas.drawPath(
          Path()
            ..moveTo(size.width * 0.3, size.height * 0.52)
            ..lineTo(size.width * 0.45, size.height * 0.66)
            ..lineTo(size.width * 0.7, size.height * 0.38),
          stroke,
        );
      case ChecklistStatus.CHECKLIST_STATUS_BLOCKED:
        for (final x in [0.4, 0.6]) {
          canvas.drawLine(
            Offset(size.width * x, size.height * 0.36),
            Offset(size.width * x, size.height * 0.64),
            stroke,
          );
        }
      case ChecklistStatus.CHECKLIST_STATUS_DROPPED:
        canvas.drawLine(
          Offset(size.width * 0.3, size.height / 2),
          Offset(size.width * 0.7, size.height / 2),
          stroke,
        );
      default:
        break;
    }
  }

  @override
  bool shouldRepaint(_StatusPainter old) =>
      old.status != status || old.color != color;
}

String statusLabel(ChecklistStatus status) => switch (status) {
  ChecklistStatus.CHECKLIST_STATUS_IN_PROGRESS => 'In progress',
  ChecklistStatus.CHECKLIST_STATUS_BLOCKED => 'Blocked',
  ChecklistStatus.CHECKLIST_STATUS_DONE => 'Done',
  ChecklistStatus.CHECKLIST_STATUS_DROPPED => 'Dropped',
  _ => 'Open',
};

const _weekdays = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
const _months = [
  'Jan',
  'Feb',
  'Mar',
  'Apr',
  'May',
  'Jun',
  'Jul',
  'Aug',
  'Sep',
  'Oct',
  'Nov',
  'Dec',
];

/// A day heading: "Today", "Yesterday", "Mon 16 Nov", with the year when
/// it isn't this one.
String dayLabel(DateTime day, DateTime now) {
  final today = DateTime(now.year, now.month, now.day);
  final date = DateTime(day.year, day.month, day.day);
  if (date == today) return 'Today';
  if (date == DateTime(today.year, today.month, today.day - 1)) {
    return 'Yesterday';
  }
  final label =
      '${_weekdays[date.weekday - 1]} ${date.day} ${_months[date.month - 1]}';
  return date.year == now.year ? label : '$label ${date.year}';
}

/// When a log update was made: "14:02" today, "16 Nov 14:02" before.
String logTime(DateTime time, {DateTime? now}) {
  now ??= DateTime.now();
  final clock =
      '${time.hour.toString().padLeft(2, '0')}:'
      '${time.minute.toString().padLeft(2, '0')}';
  final sameDay =
      time.year == now.year && time.month == now.month && time.day == now.day;
  return sameDay ? clock : '${time.day} ${_months[time.month - 1]} $clock';
}
