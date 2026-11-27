import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'characters/character.dart';
import 'checklist_reply.dart' show ChecklistReply;
import 'backend.dart';
import 'controls.dart';
import 'find.dart';
import 'database.dart';
import 'forms.dart' show FormFrame;
import 'extension_about.dart';
import 'highlights.dart';
import 'identity.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'memory.dart';
import 'messaging.dart';
import 'mobile/input.dart' show InputKind;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'monitors.dart';
import 'resources.dart';
import 'secrets.dart' show AgentSecretsPanel;
import 'tasks.dart';
import 'pipe_reports.dart' show ReportsPanel, containedKeeps;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart';
import 'selection.dart';

/// Width of an extension's panel on the right by default; the person can
/// drag it between [panelMinWidth] and [panelMaxWidth]
/// (`Settings.panel_width`), as far as the window has room.
const panelWidth = 320.0;
const panelMinWidth = 280.0;
const panelMaxWidth = 800.0;

/// [agent]'s settings for extension [id], or null when it has none.
AgentExtension? agentExtension(Agent agent, String id) {
  for (final extension in agent.extensions) {
    if (extension.id == id) return extension;
  }
  return null;
}

/// The extensions turned on for [agent], in the workspace's order; for one
/// limited to a pipe or reading reports, only those it can still use.
List<Extension> enabledExtensions(Workspace workspace, Agent agent) => [
  for (final extension in workspace.extensions)
    if ((agentExtension(agent, extension.id)?.enabled ?? false) &&
        containedKeeps(agent, extension.id))
      extension,
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

/// Over the conversation (a window with no room beside it): Esc closes
/// [agent]'s panel. It takes the focus as it opens, so Esc reaches it;
/// a click back in the conversation leaves it open until Close.
class EscClosesPanel extends StatelessWidget {
  const EscClosesPanel({super.key, required this.agent, required this.child});

  final Agent agent;
  final Widget child;

  @override
  Widget build(BuildContext context) => CallbackShortcuts(
    bindings: {
      const SingleActivator(LogicalKeyboardKey.escape): () =>
          fireAndLog(AppScope.of(context).openPanel(agent.id, '')),
    },
    child: FocusScope(autofocus: true, child: child),
  );
}

/// An extension's panel on the right of the window, beside [agent]: its
/// title and a close button, over what the backend sends for it.
class ExtensionPanel extends StatelessWidget {
  const ExtensionPanel({
    super.key,
    required this.extension,
    required this.agent,
    this.width = panelWidth,
  });

  final Extension extension;
  final Agent agent;

  /// Its sheet's width on a phone.
  final double width;

  String _title(ExtensionView? view) =>
      view == null || view.title.isEmpty ? extension.name : view.title;

  void _close(Backend backend) => fireAndLog(backend.openPanel(agent.id, ''));

  /// On a phone, a sheet's content: its name and Close in a row a finger's
  /// height, as a sheet's title is, over the panel.
  Widget _phone(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.extensionView,
      builder: (context, _) {
        final view = backend.extensionView.of(extension.id, agent.id);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsetsDirectional.only(
                start: Touch.gutter,
                end: Space.xs,
              ),
              child: ConstrainedBox(
                constraints: const BoxConstraints(minHeight: Touch.target),
                child: Row(
                  children: [
                    Expanded(
                      child: Semantics(
                        header: true,
                        child: Text(
                          _title(view),
                          style: PhoneType.emphasis.copyWith(
                            color: palette.text,
                          ),
                        ),
                      ),
                    ),
                    HeaderButton(
                      label: 'Close ${extension.name}',
                      onTap: () => _close(backend),
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
    );
  }

  @override
  Widget build(BuildContext context) {
    if (isPhoneLayout(context)) return _phone(context);
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border(left: BorderSide(color: palette.hairline)),
      ),
      child: SizedBox(
        width: width,
        child: ListenableBuilder(
          listenable: backend.extensionView,
          builder: (context, _) {
            final view = backend.extensionView.of(extension.id, agent.id);
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                WindowDrag(
                  child: Container(
                    // The title bar's height, taller when large text needs
                    // it (an iPad at accessibility sizes), never cut.
                    constraints: const BoxConstraints(
                      minHeight: titleBarHeight,
                    ),
                    padding: const EdgeInsets.fromLTRB(
                      Space.l,
                      Space.xs,
                      Space.s,
                      Space.xs,
                    ),
                    decoration: BoxDecoration(
                      border: Border(
                        bottom: BorderSide(color: palette.hairline),
                      ),
                    ),
                    child: Row(
                      children: [
                        Expanded(
                          child: Text(
                            _title(view),
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TypeScale.of(context).label
                                .copyWith(color: palette.text),
                          ),
                        ),
                        GlyphButton(
                          label: 'Close ${extension.name}',
                          active: false,
                          onTap: () => _close(backend),
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
    // Secrets has no view: the vault the workspace carries says what the
    // agent has.
    if (extension.id == 'secrets') return AgentSecretsPanel(agentId: agent.id);
    // Reports: the reports themselves, a reader's or its pipe's.
    if (extension.id == 'handoff') {
      return ReportsPanel(extension: extension, agent: agent);
    }
    // Others with no view of their own say what they do.
    if (!hasExtensionView(extension)) {
      return ExtensionAbout(extension: extension, agentId: agent.id);
    }
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
            style: TypeScale.of(context).body
                .copyWith(color: palette.textFaint),
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

  /// Each item's place, to scroll to it when it's pointed at.
  final _itemKeys = <int, GlobalKey>{};

  /// The item last pointed at, and when: it opens, to its history and a
  /// reply, when the view gets to it.
  ({int id, DateTime at})? _opened;

  /// What points at items (Needs you's Show in checklist, an agent's
  /// highlight), and the last pointing this view went to.
  ValueNotifier<({List<String> keys, DateTime at})?>? _pointedAt;
  DateTime? _wentTo;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final pointedAt = AppScope.of(context).highlights.pointedAt;
    if (pointedAt != _pointedAt) {
      _pointedAt?.removeListener(_pointed);
      _pointedAt = pointedAt..addListener(_pointed);
    }
  }

  void _pointed() {
    if (mounted) setState(() {});
  }

  /// Find in the checklist (⌘F): its items' words and notes, in the order
  /// they're listed.
  late final _find = LocalFind(
    placeholder: 'Search this checklist',
    items: () => [for (final item in _order) (item.id, _findText(item))],
    go: (id, _) {
      if (id is! int) return;
      _pinned = false;
      final index = _order.indexWhere((item) => item.id == id);
      _goTo(
        id,
        _order.length < 2 ? 0 : index / (_order.length - 1),
        glow: false,
      );
    },
  );

  /// The items as the last build listed them.
  var _order = const <ChecklistItem>[];

  /// What find searches in [item]: its words, and its note where it shows.
  static String _findText(ChecklistItem item) =>
      _showsNote(item) ? '${item.text}\n${item.note}' : item.text;

  @override
  void dispose() {
    _pointedAt?.removeListener(_pointed);
    _find.dispose();
    _scroll.dispose();
    super.dispose();
  }

  /// The item of this checklist pointed at in the last few seconds and not
  /// gone to yet, if any: the view stops following its end for it.
  int? _target(Set<int> ids) {
    final pointed = _pointedAt?.value;
    if (pointed == null || pointed.at == _wentTo) return null;
    if (DateTime.now().difference(pointed.at) > const Duration(seconds: 10)) {
      return null;
    }
    final prefix = 'checklist_item:${widget.agentId}#';
    for (final key in pointed.keys) {
      if (!key.startsWith(prefix)) continue;
      final id = int.tryParse(key.substring(prefix.length));
      if (id != null && ids.contains(id)) {
        _wentTo = pointed.at;
        _opened = (id: id, at: pointed.at);
        return id;
      }
    }
    return null;
  }

  /// Scrolls item [id] into view once this frame is laid out. One not
  /// built yet (far down a long list) is near [fraction] of the way: the
  /// view goes there first and looks again.
  void _goTo(int id, double fraction, {int tries = 4, bool glow = true}) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_scroll.hasClients) return;
      final place = _itemKeys[id]?.currentContext;
      if (place != null) {
        Scrollable.ensureVisible(place, alignment: 0.2);
        // Found (⌘F): it's marked, not glowed.
        if (!glow) return;
        // Glowing as it comes into view: what pointed at it glowed it
        // before the checklist was there (a phone's sheet still sliding
        // up), when no one could see it.
        final agent = AppScope.of(context).workspace.agent(widget.agentId);
        AppScope.of(context).highlights.glow(
          'checklist_item:${widget.agentId}#$id',
          [agentColor(agent?.color ?? 'slate')],
        );
        return;
      }
      if (tries == 0) return;
      final position = _scroll.position;
      position.jumpTo(position.maxScrollExtent * fraction.clamp(0.0, 1.0));
      _goTo(id, fraction, tries: tries - 1, glow: glow);
    });
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
    final now = widget.now ?? DateTime.now();
    final store = AppScope.of(context).workspace;
    // The person's goals, above the items.
    final goals = _Goals(
      checklist: checklist,
      agentId: widget.agentId,
      store: store,
    );
    if (checklist.items.isEmpty) {
      _order = const [];
      return FindScope(
        target: _find,
        child: ListView(
          padding: const EdgeInsets.fromLTRB(Space.s, 0, Space.s, Space.l),
          children: [
            ScrolledSelection(child: goals),
            Padding(
              padding: const EdgeInsets.all(Space.s),
              child: Text(
                'Nothing on the checklist yet.',
                style: TypeScale.of(context).body
                    .copyWith(color: palette.textFaint),
              ),
            ),
          ],
        ),
      );
    }
    final goalText = {for (final goal in checklist.goals) goal.id: goal.text};
    DateTime dayOf(ChecklistItem item) {
      final added = DateTime.fromMillisecondsSinceEpoch(item.created.toInt());
      return DateTime(added.year, added.month, added.day);
    }

    bool finished(ChecklistItem item) =>
        item.status == ChecklistStatus.CHECKLIST_STATUS_DONE ||
        item.status == ChecklistStatus.CHECKLIST_STATUS_DROPPED;
    // Items added in one call share their millisecond: then by number, as
    // they were added (List.sort isn't stable past 32: Abbacchio's row
    // 661).
    int byAdded(ChecklistItem a, ChecklistItem b) {
      final added = a.created.compareTo(b.created);
      return added != 0 ? added : a.id.compareTo(b.id);
    }

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
    // Outstanding: in progress first, then blocked, then open, then
    // parked, each oldest first.
    int rank(ChecklistItem item) => switch (item.status) {
      ChecklistStatus.CHECKLIST_STATUS_IN_PROGRESS => 0,
      ChecklistStatus.CHECKLIST_STATUS_BLOCKED => 1,
      ChecklistStatus.CHECKLIST_STATUS_PARKED => 3,
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
    // Keyed by item and section: the list keeps an item's state (an open
    // log) as others come and go, and an item that changes section starts
    // fresh in its new place, easing in.
    final highlights = AppScope.of(context).highlights;
    Widget arrival(ChecklistItem item, {String? added}) => Arrival(
      key: ValueKey((item.id, added == null)),
      animate: !reduced && moved.contains(item.id),
      child: Glow(
        store: highlights,
        matches: (key) => key == 'checklist_item:${widget.agentId}#${item.id}',
        side: AxisDirection.left,
        child: LocalFindMarks(
          key: _itemKeys.putIfAbsent(item.id, GlobalKey.new),
          find: _find,
          itemKey: item.id,
          child: _Item(
            item: item,
            store: store,
            added: added,
            agentId: widget.agentId,
            now: now,
            goal: goalText[item.goal],
            openedAt: _opened?.id == item.id ? _opened?.at : null,
          ),
        ),
      ),
    );
    // Selected where the pointer is at any zoom, however far it's
    // scrolled (ScrolledSelection), each item and heading on its own.
    // Copied, each item's lines (its words, what it waits on, its note)
    // on lines of their own, and the next item on a line after it.
    Widget item(ChecklistItem item, {String? added}) => ScrolledSelection(
      key: ValueKey((item.id, added == null)),
      child: CopyShape(
        lines: true,
        trail: '\n',
        child: arrival(item, added: added),
      ),
    );
    // An item pointed at (Show in checklist) is scrolled to, and the view
    // stops following its end, so it stays in view.
    final order = [
      for (final (_, finishedThen) in history) ...finishedThen,
      ...outstanding,
    ];
    _itemKeys.removeWhere((id, _) => !order.any((item) => item.id == id));
    _order = order;
    // What's found follows the items as they change.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _find.refresh();
    });
    if (_target({for (final item in order) item.id}) case final id?) {
      _pinned = false;
      final index = order.indexWhere((item) => item.id == id);
      _goTo(id, order.length < 2 ? 0 : index / (order.length - 1));
    }
    if (_pinned) _followSoon();
    final caption = TypeScale.of(context).caption
        .copyWith(color: palette.textFaint, fontWeight: FontWeight.w500);
    Widget heading(String left, String right) => Padding(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.m, Space.s, Space.xs),
      child: Row(
        children: [
          Expanded(child: Text(left, style: caption)),
          // At the largest text it wraps rather than pushing past the edge.
          Flexible(
            child: Text(right, style: caption, textAlign: TextAlign.end),
          ),
        ],
      ),
    );
    // On a phone the progress scrolls with the list: held above it, it'd
    // leave no room for the list at the largest text.
    final phone = isPhoneLayout(context);
    return FindScope(
      target: _find,
      child: LayoutBuilder(
        builder: (context, constraints) => Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (!phone) ...[
              _Progress(items: checklist.items),
              // Held above the items, scrolling on its own past two fifths of
              // the panel.
              ConstrainedBox(
                constraints: BoxConstraints(
                  maxHeight: constraints.maxHeight.isFinite
                      ? constraints.maxHeight * 0.4
                      : double.infinity,
                ),
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    border: Border(bottom: BorderSide(color: palette.hairline)),
                  ),
                  // With no goals there's only Add goal…: nothing to scroll.
                  child: checklist.goals.isEmpty
                      ? Padding(
                          padding: const EdgeInsets.symmetric(
                            horizontal: Space.s,
                          ),
                          child: goals,
                        )
                      : SingleChildScrollView(
                          padding: const EdgeInsets.symmetric(
                            horizontal: Space.s,
                          ),
                          child: ScrolledSelection(child: goals),
                        ),
                ),
              ),
            ],
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
                          if (phone) ...[
                            ScrolledSelection(
                              child: _Progress(items: checklist.items),
                            ),
                            ScrolledSelection(child: goals),
                          ],
                          for (final (day, finishedThen) in history) ...[
                            ScrolledSelection(
                              child: heading(
                                dayLabel(day, now),
                                doneLabel(days[day]!),
                              ),
                            ),
                            for (final done in finishedThen) item(done),
                          ],
                          if (outstanding.isNotEmpty) ...[
                            ScrolledSelection(
                              child: heading(
                                'Outstanding',
                                '${outstanding.length}',
                              ),
                            ),
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
        ),
      ),
    );
  }
}

/// Whether [item]'s note shows under it: while it's blocked or parked.
bool _showsNote(ChecklistItem item) =>
    (item.status == ChecklistStatus.CHECKLIST_STATUS_BLOCKED ||
        item.status == ChecklistStatus.CHECKLIST_STATUS_PARKED) &&
    item.note.isNotEmpty;

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
/// many items are done, in progress, blocked, open and parked. Dropped
/// items don't count.
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
    final parked = count(ChecklistStatus.CHECKLIST_STATUS_PARKED);
    final dropped = count(ChecklistStatus.CHECKLIST_STATUS_DROPPED);
    final counted = items.length - dropped;
    final open = counted - done - working - blocked - parked;
    final style = TypeScale.of(context).caption
        .copyWith(color: palette.textMuted);
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
                if (parked > 0) '$parked parked',
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

/// The goal an item serves, on its line: "Goal: Get the Android app to
/// a…", its words cut at a word to fit beside the rest.
String goalLine(String goal) {
  const most = 28;
  final words = goal.trim();
  if (words.length <= most) return 'Goal: $words';
  // Cut at the last whole word that fits.
  final cut = words.substring(0, most);
  final space = cut.lastIndexOf(' ');
  return 'Goal: ${(space > 0 ? cut.substring(0, space) : cut).trimRight()}…';
}

/// How far [goal] has come: the items that serve it, done over all but
/// dropped ones ("2 of 5 items done"), or "No items yet".
String goalProgress(Goal goal, List<ChecklistItem> items) {
  final serving = [
    for (final item in items)
      if (item.goal == goal.id &&
          item.status != ChecklistStatus.CHECKLIST_STATUS_DROPPED)
        item,
  ];
  if (serving.isEmpty) return 'No items yet';
  final done = serving
      .where((item) => item.status == ChecklistStatus.CHECKLIST_STATUS_DONE)
      .length;
  return '$done of ${serving.length} ${serving.length == 1 ? 'item' : 'items'} done';
}

bool _finishedGoal(Goal goal) =>
    goal.status == ChecklistStatus.CHECKLIST_STATUS_DONE ||
    goal.status == ChecklistStatus.CHECKLIST_STATUS_DROPPED;

/// The person's goals on a checklist, above its items: the open ones, then
/// "2 finished" folding the done and dropped ones, and Add goal…. Each
/// says how far its items have come; its ⋯ edits, finishes, drops or
/// removes it. With none, only Add goal….
class _Goals extends StatefulWidget {
  const _Goals({
    required this.checklist,
    required this.agentId,
    required this.store,
  });

  final Checklist checklist;
  final String agentId;
  final WorkspaceStore store;

  @override
  State<_Goals> createState() => _GoalsState();
}

class _GoalsState extends State<_Goals> {
  var _showFinished = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final goals = widget.checklist.goals;
    final open = [
      for (final goal in goals)
        if (!_finishedGoal(goal)) goal,
    ];
    final finished = [
      for (final goal in goals)
        if (_finishedGoal(goal)) goal,
    ];
    final caption = TypeScale.of(context).caption
        .copyWith(color: palette.textFaint, fontWeight: FontWeight.w500);
    Widget row(Goal goal) => _GoalRow(
      key: ValueKey(goal.id),
      goal: goal,
      items: widget.checklist.items,
      agentId: widget.agentId,
      store: widget.store,
    );
    // As tall as what it holds: with no goals, Add goal… alone, not the
    // two fifths of the panel it may scroll in.
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (goals.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(
              Space.s,
              Space.m,
              Space.s,
              Space.xs,
            ),
            child: Row(
              children: [
                Expanded(child: Text('Goals', style: caption)),
                Text('${open.length}', style: caption),
              ],
            ),
          ),
        for (final goal in open) row(goal),
        if (finished.isNotEmpty) ...[
          Semantics(
            button: true,
            expanded: _showFinished,
            child: Pressable(
              onTap: () => setState(() => _showFinished = !_showFinished),
              builder: (context, hovered) => Container(
                constraints: BoxConstraints(
                  minHeight: isPhoneLayout(context) ? Touch.target : 0,
                ),
                padding: const EdgeInsets.symmetric(
                  horizontal: Space.s,
                  vertical: Space.xs,
                ),
                alignment: Alignment.centerLeft,
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Flexible(
                      child: Text(
                        '${finished.length} finished',
                        style: TypeScale.of(context).caption.copyWith(
                          color: hovered
                              ? palette.textMuted
                              : palette.textFaint,
                        ),
                      ),
                    ),
                    const SizedBox(width: Space.xs),
                    Chevron(color: palette.textFaint, down: _showFinished),
                  ],
                ),
              ),
            ),
          ),
          if (_showFinished)
            for (final goal in finished) row(goal),
        ],
        Padding(
          padding: const EdgeInsets.fromLTRB(
            Space.xs,
            Space.xs,
            Space.xs,
            Space.s,
          ),
          child: Align(
            alignment: Alignment.centerLeft,
            child: Button(
              label: 'Add goal…',
              onPressed: () => showModal(
                context,
                (context, close) =>
                    _GoalForm(agentId: widget.agentId, close: close),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// One goal: its mark, its words, how far its items have come, and its ⋯;
/// a click opens its log.
class _GoalRow extends StatefulWidget {
  const _GoalRow({
    super.key,
    required this.goal,
    required this.items,
    required this.agentId,
    required this.store,
  });

  final Goal goal;
  final List<ChecklistItem> items;
  final String agentId;
  final WorkspaceStore store;

  @override
  State<_GoalRow> createState() => _GoalRowState();
}

class _GoalRowState extends State<_GoalRow> {
  var _open = false;

  Future<void> _actions() async {
    final goal = widget.goal;
    final finished = _finishedGoal(goal);
    final action = await showActions<String>(
      context,
      title: goal.text,
      actions: [
        const SheetAction('Edit…', 'edit'),
        if (finished)
          const SheetAction('Reopen', 'reopen')
        else ...[
          const SheetAction('Mark done', 'done'),
          const SheetAction('Drop', 'drop'),
        ],
        const SheetAction('Remove…', 'remove', destructive: true),
      ],
    );
    if (!mounted || action == null) return;
    final backend = AppScope.of(context);
    final agentId = widget.agentId;
    switch (action) {
      case 'edit':
        await showModal(
          context,
          (context, close) =>
              _GoalForm(agentId: agentId, goal: goal, close: close),
        );
      case 'reopen':
        fireAndLog(
          backend.updateGoal(
            agentId,
            goal.id,
            status: ChecklistStatus.CHECKLIST_STATUS_OPEN,
          ),
        );
      case 'done':
        fireAndLog(
          backend.updateGoal(
            agentId,
            goal.id,
            status: ChecklistStatus.CHECKLIST_STATUS_DONE,
          ),
        );
      case 'drop':
        fireAndLog(
          backend.updateGoal(
            agentId,
            goal.id,
            status: ChecklistStatus.CHECKLIST_STATUS_DROPPED,
          ),
        );
      case 'remove':
        final sure = await showActions<bool>(
          context,
          title: 'Remove this goal?',
          message: 'Its items stay on the checklist.',
          actions: const [SheetAction('Remove', true, destructive: true)],
        );
        if (sure == true) {
          fireAndLog(backend.updateGoal(agentId, goal.id, remove: true));
        }
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final goal = widget.goal;
    final finished = _finishedGoal(goal);
    final phone = isPhoneLayout(context);
    return Semantics(
      container: true,
      expanded: goal.log.isEmpty ? null : _open,
      child: Pressable(
        onTap: goal.log.isEmpty ? null : () => setState(() => _open = !_open),
        builder: (context, hovered) => Container(
          constraints: BoxConstraints(minHeight: phone ? Touch.target : 0),
          padding: const EdgeInsets.fromLTRB(
            Space.s,
            Space.s - 2,
            Space.xs,
            Space.s - 2,
          ),
          decoration: BoxDecoration(
            color: hovered && goal.log.isNotEmpty ? palette.hover : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Padding(
                padding: const EdgeInsets.only(top: 2.5),
                child: StatusMark(status: goal.status),
              ),
              const SizedBox(width: Space.s),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      goal.text,
                      style: TypeScale.of(context).body.copyWith(
                        color: finished ? palette.textMuted : palette.text,
                        decoration:
                            goal.status ==
                                ChecklistStatus.CHECKLIST_STATUS_DROPPED
                            ? TextDecoration.lineThrough
                            : null,
                        decorationColor: palette.textMuted,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Row(
                      children: [
                        Flexible(
                          child: Text(
                            goalProgress(goal, widget.items),
                            style: TypeScale.of(context).caption
                                .copyWith(color: palette.textMuted),
                          ),
                        ),
                        if (goal.log.isNotEmpty) ...[
                          const SizedBox(width: Space.s),
                          Chevron(color: palette.textFaint, down: _open),
                        ],
                      ],
                    ),
                    if (_open) ...[
                      const SizedBox(height: Space.s),
                      for (final update in goal.log)
                        _LogLine(update: update, store: widget.store),
                    ],
                  ],
                ),
              ),
              const SizedBox(width: Space.xs),
              GlyphButton(
                label: 'More for ${goal.text}',
                active: false,
                onTap: _actions,
                glyph: (color) => MoreGlyph(color: color),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Adding a goal, in the person's words, or changing one's words.
class _GoalForm extends StatefulWidget {
  const _GoalForm({required this.agentId, required this.close, this.goal});

  final String agentId;
  final VoidCallback close;

  /// The goal to change; null adds one.
  final Goal? goal;

  @override
  State<_GoalForm> createState() => _GoalFormState();
}

class _GoalFormState extends State<_GoalForm> {
  late final _text = TextEditingController(text: widget.goal?.text ?? '');
  var _saving = false;
  String? _error;

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final text = _text.text.trim();
    if (_saving || text.isEmpty) return;
    setState(() {
      _saving = true;
      _error = null;
    });
    final backend = AppScope.of(context);
    final goal = widget.goal;
    try {
      if (goal == null) {
        await backend.addGoal(widget.agentId, text);
      } else if (text != goal.text) {
        await backend.updateGoal(widget.agentId, goal.id, text: text);
      }
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _saving = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => FormFrame(
    title: widget.goal == null ? 'Add a goal' : 'Edit goal',
    error: _error,
    submitLabel: widget.goal == null ? 'Add' : 'Save',
    onSubmit: _saving ? null : _save,
    onCancel: widget.close,
    fields: [
      Field(
        label: 'Goal',
        note:
            'In your words, such as: Get the Android app to a link I can '
            'share.',
        child: TextField(
          controller: _text,
          autofocus: true,
          placeholder: 'What do you want done?',
          semanticsLabel: 'Goal',
          kind: InputKind.text,
          onSubmitted: (_) => _save(),
        ),
      ),
    ],
  );
}

/// How long something has gone on, in the app's units (formatAge's,
/// without "ago"): "under a minute", "40 min", "3 h", "1 day", "2 days".
String blockedFor(Duration duration) {
  if (duration.inMinutes < 1) return 'under a minute';
  if (duration.inHours < 1) return '${duration.inMinutes} min';
  if (duration.inDays < 1) return '${duration.inHours} h';
  return duration.inDays == 1 ? '1 day' : '${duration.inDays} days';
}

/// "at 14:00", or "24 Nov at 14:00" on another day than [now]'s.
String atTime(int ms, DateTime now) {
  final time = DateTime.fromMillisecondsSinceEpoch(ms);
  final clock = logTime(time, now: time);
  final sameDay =
      time.year == now.year && time.month == now.month && time.day == now.day;
  return sameDay
      ? 'at $clock'
      : '${time.day} ${_months[time.month - 1]} at $clock';
}

/// When a parked item goes back to open on its own (its retry_at_ms), in
/// words: "Resumes at 14:00"; '' when it has no time, or the time passed.
String resumesWords(ChecklistItem item, DateTime now) {
  final ms = item.retryAtMs.toInt();
  if (item.status != ChecklistStatus.CHECKLIST_STATUS_PARKED ||
      ms <= now.millisecondsSinceEpoch) {
    return '';
  }
  return 'Resumes ${atTime(ms, now)}';
}

/// What a blocked item waits on, in words: [text], and whether "See
/// question" follows (a card for it waits).
typedef WaitingWords = ({String text, bool seeQuestion});

/// The words for an item blocked on [kind] ([blockedOn] for one from
/// before kinds, "person" or the blocker's words). [ref] names the agent
/// or "agent#item" it waits on; [agentId] is the agent whose item it is
/// (its owner, or its checklist's agent).
WaitingWords waitingWords({
  required BlockedKind kind,
  required String blockedOn,
  required String ref,
  required int sinceMs,
  required int retryAtMs,
  required int reaskAtMs,
  required int dismissedMs,
  required bool hasPendingAsk,
  required String agentId,
  required String checklistAgentId,
  required Workspace workspace,
  required DateTime now,
}) {
  String nameOf(String id) =>
      workspace.agents.where((agent) => agent.id == id).firstOrNull?.name ?? id;
  String at(int ms) => atTime(ms, now);

  final person =
      kind == BlockedKind.BLOCKED_KIND_PERSON_DECISION ||
      kind == BlockedKind.BLOCKED_KIND_PERSON_ACTION ||
      (kind == BlockedKind.BLOCKED_KIND_UNSPECIFIED && blockedOn == 'person');
  if (!person) {
    final text = switch (kind) {
      // By the item's words (blocked_on), never its number.
      BlockedKind.BLOCKED_KIND_ITEM => () {
        final hash = ref.indexOf('#');
        final agent = hash < 0 ? ref : ref.substring(0, hash);
        final what = shortened(blockedOn, 32);
        return agent.isEmpty || agent == checklistAgentId
            ? 'Waiting for: $what'
            : 'Waiting for ${nameOf(agent)}: $what';
      }(),
      BlockedKind.BLOCKED_KIND_AGENT => 'Waiting for ${nameOf(ref)}',
      BlockedKind.BLOCKED_KIND_EXTERNAL =>
        'Waiting for ${blockedOn.isEmpty ? 'something outside' : blockedOn}'
            '${retryAtMs > now.millisecondsSinceEpoch ? ' · checks again ${at(retryAtMs)}' : ''}',
      _ => 'Blocked on $blockedOn',
    };
    return (text: text, seeQuestion: false);
  }
  if (hasPendingAsk) {
    // A card asks: Needs you shows it, and "See question" goes to it.
    final text = switch (kind) {
      BlockedKind.BLOCKED_KIND_PERSON_DECISION => 'Waiting for your decision',
      BlockedKind.BLOCKED_KIND_PERSON_ACTION => 'Waiting for you to act',
      _ => 'Blocked on you',
    };
    return (text: text, seeQuestion: true);
  }
  // No card asks: the agent's state, never a task for the person (Matt's
  // ruling). Needs you doesn't show it; the card will when one asks.
  final owner = nameOf(agentId);
  if (dismissedMs > 0) {
    // Dismissed: the agent decides meanwhile, and asks again no sooner
    // than a day later, when it still needs the person.
    return (
      text: reaskAtMs > 0
          ? 'You left it to $owner · it may ask again ${at(reaskAtMs)}'
          : 'You left it to $owner',
      seeQuestion: false,
    );
  }
  return (
    text: reaskAtMs > 0
        ? 'Waiting for $owner to ask you ${at(reaskAtMs)}'
        : 'Waiting for $owner to ask you',
    seeQuestion: false,
  );
}

/// [text] cut at a word to at most [max] characters, with "…" when cut.
String shortened(String text, int max) {
  if (text.length <= max) return text;
  final cut = text.lastIndexOf(' ', max);
  return '${text.substring(0, cut > 0 ? cut : max).trimRight()}…';
}

/// What a blocked item waits on ([waitingWords]), in `textMuted`; with a
/// question waiting, "· See question", which opens the conversation at the
/// card asking about it (WaitingItem.card_seq), or at [agentId]'s first
/// card waiting when the backend names none.
class _BlockedOn extends StatelessWidget {
  const _BlockedOn({
    required this.item,
    required this.agentId,
    required this.checklistAgentId,
    required this.now,
  });

  final ChecklistItem item;
  final String agentId;
  final String checklistAgentId;
  final DateTime now;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final caption = TypeScale.of(context).caption;
    final backend = AppScope.of(context);
    final words = waitingWords(
      kind: item.blockedKind,
      blockedOn: item.blockedOn,
      ref: item.blockedRef,
      sinceMs: item.blockedSinceMs.toInt(),
      retryAtMs: item.retryAtMs.toInt(),
      reaskAtMs: item.reaskAtMs.toInt(),
      dismissedMs: item.dismissedMs.toInt(),
      hasPendingAsk: item.hasPendingAsk,
      agentId: agentId,
      checklistAgentId: checklistAgentId,
      workspace: backend.workspace.workspace,
      now: now,
    );
    final text = Text(
      words.text,
      style: caption.copyWith(color: palette.textMuted),
    );
    if (!words.seeQuestion) return text;
    final card = backend.workspace
        .agent(checklistAgentId)
        ?.waitingItems
        .where((waiting) => waiting.item == item.id && waiting.cardSeq != 0)
        .firstOrNull;
    final cardAgentId = card == null
        ? agentId
        : card.cardAgentId.isEmpty
        ? checklistAgentId
        : card.cardAgentId;
    final waiting =
        card?.cardSeq ??
        backend.workspace.agent(agentId)?.waitingSeqs.firstOrNull;
    return Wrap(
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        text,
        Text(' · ', style: caption.copyWith(color: palette.textMuted)),
        Semantics(
          link: true,
          child: Pressable(
            onTap: waiting == null
                ? null
                : () => fireAndLog(() async {
                    await backend.selectAgent(cardAgentId);
                    backend.reveal.reachSeq(cardAgentId, waiting.toInt());
                  }()),
            builder: (context, hovered) => Text(
              'See question',
              style: caption.copyWith(
                color: hovered ? palette.text : palette.textMuted,
                decoration: TextDecoration.underline,
                decorationColor: hovered ? palette.text : palette.textMuted,
              ),
            ),
          ),
        ),
      ],
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
          width: isPhoneLayout(context) ? Touch.target : 28,
          height: isPhoneLayout(context) ? Touch.target : 28,
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
  const _Item({
    required this.item,
    required this.store,
    required this.now,
    this.added,
    this.agentId = '',
    this.goal,
    this.openedAt,
  });

  final ChecklistItem item;
  final WorkspaceStore store;

  /// When it was last pointed at (Show in checklist): it opens then.
  final DateTime? openedAt;

  /// The words of the goal it serves, if any.
  final String? goal;

  /// Whose checklist it is: an item's owning agent when it has no owner.
  final String agentId;

  /// What "now" is, for how long it has been blocked.
  final DateTime now;

  /// The day it was added, shown when it isn't under that day's heading.
  final String? added;

  @override
  State<_Item> createState() => _ItemState();
}

class _ItemState extends State<_Item> {
  late var _open = widget.openedAt != null;

  @override
  void didUpdateWidget(_Item old) {
    super.didUpdateWidget(old);
    final at = widget.openedAt;
    if (at != null && at != old.openedAt) _open = true;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final item = widget.item;
    final finished =
        item.status == ChecklistStatus.CHECKLIST_STATUS_DONE ||
        item.status == ChecklistStatus.CHECKLIST_STATUS_DROPPED;
    final owner = item.owner.isEmpty ? null : widget.store.agent(item.owner);
    final phone = isPhoneLayout(context);
    final row = Semantics(
      container: true,
      expanded: _open,
      // Open, its history and a field to reply on it.
      child: Pressable(
        onTap: () => setState(() => _open = !_open),
        builder: (context, hovered) => Container(
          constraints: BoxConstraints(minHeight: phone ? Touch.target : 0),
          padding: const EdgeInsets.symmetric(
            horizontal: Space.s,
            vertical: Space.s - 2,
          ),
          decoration: BoxDecoration(
            color: hovered ? palette.hover : null,
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
                    Text.rich(
                      // Its words' matches marked when found (⌘F).
                      FindHighlight.markText(
                        context,
                        item.text,
                        TypeScale.of(context).body.copyWith(
                          color: finished ? palette.textMuted : palette.text,
                          decoration:
                              item.status ==
                                  ChecklistStatus.CHECKLIST_STATUS_DROPPED
                              ? TextDecoration.lineThrough
                              : null,
                          decorationColor: palette.textMuted,
                        ),
                      ),
                    ),
                    // What a blocked item waits on: on the person, for how
                    // long and whether they were asked; else the blocker.
                    if (item.status ==
                            ChecklistStatus.CHECKLIST_STATUS_BLOCKED &&
                        (item.blockedOn.isNotEmpty ||
                            item.blockedKind !=
                                BlockedKind.BLOCKED_KIND_UNSPECIFIED))
                      Padding(
                        padding: const EdgeInsets.only(top: 2),
                        child: _BlockedOn(
                          item: item,
                          agentId: item.owner.isEmpty
                              ? widget.agentId
                              : item.owner,
                          checklistAgentId: widget.agentId,
                          now: widget.now,
                        ),
                      ),
                    // A parked item's time to go back to open, if it has one.
                    if (resumesWords(item, widget.now).isNotEmpty)
                      Padding(
                        padding: const EdgeInsets.only(top: 2),
                        child: Text(
                          resumesWords(item, widget.now),
                          style: TypeScale.of(context).caption
                              .copyWith(color: palette.textMuted),
                        ),
                      ),
                    if (_showsNote(item))
                      Padding(
                        padding: const EdgeInsets.only(top: 2),
                        child: Text.rich(
                          FindHighlight.markText(
                            context,
                            item.note,
                            TypeScale.of(context).caption
                                .copyWith(color: palette.textFaint),
                          ),
                        ),
                      ),
                    const SizedBox(height: 2),
                    // Its number, status, owner and day are the panel's
                    // labels, not the item's words: a copy leaves them out.
                    if (phone)
                      // At the largest text, onto as many lines as it needs.
                      Unselectable(
                        child: Wrap(
                          spacing: Space.s,
                          runSpacing: 2,
                          crossAxisAlignment: WrapCrossAlignment.center,
                          children: [
                            Text(
                              '#${item.id}',
                              style: TypeScale.of(context).caption.copyWith(
                                color: palette.textFaint,
                                fontFeatures: const [
                                  FontFeature.tabularFigures(),
                                ],
                              ),
                            ),
                            Text(
                              statusLabel(item.status),
                              style: TypeScale.of(context).caption
                                  .copyWith(color: palette.textMuted),
                            ),
                            if (item.owner.isEmpty)
                              Text(
                                'Unassigned',
                                style: TypeScale.of(context).caption
                                    .copyWith(color: palette.textFaint),
                              )
                            else
                              Row(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  owner == null
                                      ? const UnknownCharacter(size: 20)
                                      : AgentCharacter(agent: owner, size: 20),
                                  const SizedBox(width: Space.xs),
                                  Flexible(
                                    child: Text(
                                      owner?.name ?? item.owner,
                                      style: TypeScale.of(context).caption
                                          .copyWith(color: palette.textMuted),
                                    ),
                                  ),
                                ],
                              ),
                            if (widget.added != null)
                              Text(
                                widget.added!,
                                style: TypeScale.of(context).caption
                                    .copyWith(color: palette.textFaint),
                              ),
                            if (widget.goal case final goal?)
                              Text(
                                goalLine(goal),
                                style: TypeScale.of(context).caption
                                    .copyWith(color: palette.textFaint),
                              ),
                            Chevron(color: palette.textFaint, down: _open),
                          ],
                        ),
                      )
                    else
                      Unselectable(
                        child: Row(
                          children: [
                            Text(
                              '#${item.id}',
                              style: TypeScale.of(context).caption.copyWith(
                                color: palette.textFaint,
                                fontFeatures: const [
                                  FontFeature.tabularFigures(),
                                ],
                              ),
                            ),
                            const SizedBox(width: Space.s),
                            Text(
                              statusLabel(item.status),
                              style: TypeScale.of(context).caption
                                  .copyWith(color: palette.textMuted),
                            ),
                            const SizedBox(width: Space.s),
                            if (item.owner.isEmpty)
                              Flexible(
                                child: Text(
                                  'Unassigned',
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TypeScale.of(context).caption
                                      .copyWith(color: palette.textFaint),
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
                                  style: TypeScale.of(context).caption
                                      .copyWith(color: palette.textMuted),
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
                                  style: TypeScale.of(context).caption
                                      .copyWith(color: palette.textFaint),
                                ),
                              ),
                            ],
                            if (widget.goal case final goal?) ...[
                              const SizedBox(width: Space.s),
                              Flexible(
                                child: Text(
                                  goalLine(goal),
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TypeScale.of(context).caption
                                      .copyWith(color: palette.textFaint),
                                ),
                              ),
                            ],
                            const Spacer(),
                            Chevron(color: palette.textFaint, down: _open),
                          ],
                        ),
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
      ),
    );
    if (!_open) return row;
    // Outside the row's press, so a click beside the field doesn't fold it;
    // under the item's words.
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        row,
        Padding(
          padding: const EdgeInsets.fromLTRB(
            Space.s + 14 + Space.s,
            Space.xs,
            Space.s,
            Space.s,
          ),
          child: ChecklistReply(
            key: ValueKey('reply:${widget.agentId}#${item.id}'),
            checklistAgentId: widget.agentId,
            itemId: item.id,
            owner: item.owner,
          ),
        ),
      ],
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
        style: TypeScale.of(context).caption,
      ),
    );
  }
}

/// A 14px mark for an item's status: an empty ring when open, a half-filled
/// signal ring while in progress, a paused ring while blocked, a faint ring
/// with a dot while parked, a check when done, a struck ring when dropped.
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
            ChecklistStatus.CHECKLIST_STATUS_DROPPED ||
            ChecklistStatus.CHECKLIST_STATUS_PARKED => palette.textFaint,
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
      case ChecklistStatus.CHECKLIST_STATUS_PARKED:
        // Set aside: a dot at its centre.
        canvas.drawCircle(center, 1.75, Paint()..color = color);
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
  ChecklistStatus.CHECKLIST_STATUS_PARKED => 'Parked',
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
