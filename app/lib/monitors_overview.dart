import 'dart:async';
import 'dart:math' as math;

import 'package:clock/clock.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'extensions.dart' show logTime;
import 'highlights.dart' show Glow;
import 'identity.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'monitors.dart';
import 'pane_chrome.dart' show InPane;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';
import 'state.dart' show WorkspaceStore;
import 'tell_rivet.dart' show ErrorLine;

/// Which monitors the page shows, by state.
enum MonitorsShown {
  all('All states', []),
  active('Active', [MonitorState.MONITOR_STATE_ACTIVE]),
  paused('Paused or failed', [
    MonitorState.MONITOR_STATE_PAUSED,
    MonitorState.MONITOR_STATE_ERROR,
  ]),
  ended('Ended', [
    MonitorState.MONITOR_STATE_DONE,
    MonitorState.MONITOR_STATE_CANCELLED,
  ]);

  const MonitorsShown(this.label, this.states);

  final String label;
  final List<MonitorState> states;
}

/// Every agent's monitors in one place: scheduled jobs,
/// reminders and watches, by agent, filtered by agent, project and state.
/// Each agent's name opens its conversation; a monitor opened shows its
/// prompt and what happened to it, a page at a time. Asked of the backend
/// (ListMonitors) when it opens, when a filter changes and whenever it
/// says some monitors changed.
class MonitorsOverview extends StatefulWidget {
  const MonitorsOverview({super.key});

  @override
  State<MonitorsOverview> createState() => _MonitorsOverviewState();
}

class _MonitorsOverviewState extends State<MonitorsOverview> {
  var _agent = '';
  var _project = '';
  var _shown = MonitorsShown.all;
  MonitorList? _list;
  String? _error;
  var _asked = 0;
  late final Backend _backend;

  /// The monitor something opened this page at ([monitorKey]), until the
  /// list has it; then the one open because of it.
  String? _goingTo;
  String? _opened;

  /// The pointing this page last went to.
  DateTime? _wentTo;

  /// Each monitor's place by [monitorKey], to scroll to it.
  final _rows = <String, GlobalKey>{};

  @override
  void initState() {
    super.initState();
    _backend = AppScope.of(context);
    _backend.monitorsChanged.addListener(_load);
    _backend.connected.addListener(_reconnected);
    _backend.highlights.pointedAt.addListener(_pointed);
    _goingTo = _pointedMonitor();
    unawaited(_load());
  }

  @override
  void dispose() {
    _backend.monitorsChanged.removeListener(_load);
    _backend.connected.removeListener(_reconnected);
    _backend.highlights.pointedAt.removeListener(_pointed);
    super.dispose();
  }

  /// The monitor pointed at in the last few seconds and not gone to yet,
  /// if any.
  String? _pointedMonitor() {
    final pointed = _backend.highlights.pointedAt.value;
    if (pointed == null || pointed.at == _wentTo) return null;
    if (clock.now().difference(pointed.at) > const Duration(seconds: 10)) {
      return null;
    }
    final key = pointed.keys.where((key) => key.startsWith('monitor:'));
    if (key.isEmpty) return null;
    _wentTo = pointed.at;
    return key.first;
  }

  void _pointed() {
    final key = _pointedMonitor();
    if (key == null || !mounted) return;
    _goingTo = key;
    // A filter that would hide it is cleared.
    if (_agent.isNotEmpty ||
        _project.isNotEmpty ||
        _shown != MonitorsShown.all) {
      _filter(() {
        _agent = '';
        _project = '';
        _shown = MonitorsShown.all;
      });
    } else {
      setState(() {});
    }
  }

  /// Scrolls to [key]'s monitor once this frame is laid out, glowing.
  void _goTo(String key, Agent? agent) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final place = _rows[key]?.currentContext;
      if (!mounted || place == null) return;
      Scrollable.ensureVisible(place, alignment: 0.2);
      _backend.highlights.glow(key, [agentColor(agent?.color ?? 'slate')]);
    });
  }

  void _reconnected() {
    if (_backend.connected.value) unawaited(_load());
  }

  Future<void> _load() async {
    final asked = ++_asked;
    try {
      final list = await _backend.listMonitors(
        agentId: _agent,
        projectId: _project,
        states: _shown.states,
      );
      if (!mounted || asked != _asked) return;
      setState(() {
        _list = list;
        _error = null;
      });
    } on RequestFailure catch (failure) {
      if (!mounted || asked != _asked) return;
      setState(() => _error = failure.message);
    }
  }

  void _filter(void Function() change) {
    setState(() {
      change();
      _list = null;
      _error = null;
    });
    unawaited(_load());
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    void close() => AppScope.closePage(context);
    final inPane = InPane.of(context);
    final phone = isPhoneLayout(context);
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: FocusScope(
        autofocus: !inPane,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (!inPane)
              phone
                  ? PhoneHeader(
                      leading: HeaderButton.back(onTap: close),
                      title: Text(
                        'Monitors',
                        style: PhoneType.emphasis.copyWith(color: palette.text),
                      ),
                    )
                  : WindowDrag(
                      child: Container(
                        height: titleBarHeight,
                        padding: const EdgeInsets.only(
                          left: Space.xl,
                          right: Space.m,
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
                                'Monitors',
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TypeScale.of(context).label.copyWith(
                                  color: palette.text,
                                  fontWeight: FontWeight.w600,
                                ),
                              ),
                            ),
                            const SizedBox(width: Space.s),
                            Button(label: 'Done', onPressed: close),
                          ],
                        ),
                      ),
                    ),
            Expanded(
              child: SelectableArea(
                child: ListenableBuilder(
                  listenable: _backend.workspace,
                  builder: (context, _) => SingleChildScrollView(
                    padding: EdgeInsets.all(phone ? Space.l : Space.xl),
                    child: Align(
                      alignment: Alignment.topCenter,
                      child: ConstrainedBox(
                        constraints: const BoxConstraints(maxWidth: 640),
                        child: _body(context, palette),
                      ),
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

  Widget _body(BuildContext context, Palette palette) {
    final store = _backend.workspace;
    final workspace = store.workspace;
    final type = TypeScale.of(context);
    final list = _list;
    // A filter's agent or project gone: shown as all.
    final agents = [
      for (final agent in workspace.agents)
        if (_project.isEmpty || agent.projectId == _project) agent,
    ];
    // Each filter as wide as its words need at the text size (its widest
    // choice, with the field's padding and chevron), up to the page's width,
    // so "Paused or failed" stays on one line on a phone at any size short
    // of that.
    final scaler = MediaQuery.textScalerOf(context);
    final phone = touchFirst(context);
    final style = type.body;
    final direction = Directionality.of(context);
    double words(Iterable<String> labels) {
      var widest = 0.0;
      for (final label in labels) {
        final painter = TextPainter(
          text: TextSpan(text: label, style: style),
          textScaler: scaler,
          textDirection: direction,
          maxLines: 1,
        )..layout();
        widest = math.max(widest, painter.width);
        painter.dispose();
      }
      // Select's padding each side, the gap and chevron, and its border.
      return widest.ceilToDouble() +
          2 * (phone ? Space.m : Space.s + 2) +
          Space.s +
          10 +
          2;
    }

    final filters = Unselectable(
      child: LayoutBuilder(
        builder: (context, constraints) {
          double wide(double base, Iterable<String> labels) => math.min(
            math.max(scaler.scale(base), words(labels)),
            constraints.maxWidth,
          );
          return Wrap(
            spacing: Space.s,
            runSpacing: Space.s,
            children: [
              SizedBox(
                width: wide(180, const ['All agents']),
                child: Select<String>(
                  title: 'Agent',
                  value: agents.any((a) => a.id == _agent) ? _agent : '',
                  options: [
                    const SelectOption('', 'All agents'),
                    for (final agent in agents)
                      SelectOption(agent.id, agent.name),
                  ],
                  onChanged: (id) => _filter(() => _agent = id),
                ),
              ),
              if (workspace.projects.isNotEmpty)
                SizedBox(
                  width: wide(180, const ['All projects']),
                  child: Select<String>(
                    title: 'Project',
                    value: workspace.projects.any((p) => p.id == _project)
                        ? _project
                        : '',
                    options: [
                      const SelectOption('', 'All projects'),
                      for (final project in workspace.projects)
                        SelectOption(project.id, project.name),
                    ],
                    onChanged: (id) => _filter(() {
                      _project = id;
                      // An agent of another project would match nothing.
                      final agent = store.agent(_agent);
                      if (agent != null &&
                          id.isNotEmpty &&
                          agent.projectId != id) {
                        _agent = '';
                      }
                    }),
                  ),
                ),
              SizedBox(
                width: wide(160, [
                  for (final shown in MonitorsShown.values) shown.label,
                ]),
                child: Select<MonitorsShown>(
                  title: 'State',
                  value: _shown,
                  options: [
                    for (final shown in MonitorsShown.values)
                      SelectOption(shown, shown.label),
                  ],
                  onChanged: (shown) => _filter(() => _shown = shown),
                ),
              ),
            ],
          );
        },
      ),
    );
    final filtered =
        _agent.isNotEmpty || _project.isNotEmpty || _shown != MonitorsShown.all;
    // By agent, in the backend's order (by agent, then number).
    final groups = <String, List<AgentMonitor>>{};
    for (final item in list?.monitors ?? const <AgentMonitor>[]) {
      (groups[item.agentId] ??= []).add(item);
    }
    if (_goingTo case final going?) {
      final found = list?.monitors.where(
        (item) => monitorKey(item.agentId, item.monitor.id) == going,
      );
      if (found != null && found.isNotEmpty) {
        _goingTo = null;
        _opened = going;
        _goTo(going, store.agent(found.first.agentId));
      }
    }
    Widget quiet(String text, {Color? color}) => Padding(
      padding: const EdgeInsets.symmetric(vertical: Space.s),
      child: Text(
        text,
        style: type.body.copyWith(color: color ?? palette.textFaint),
      ),
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'Scheduled jobs, reminders and watches your agents set up for '
          'later, and what each will be told then.',
          style: type.caption.copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.l),
        filters,
        const SizedBox(height: Space.l),
        if (_error case final error?)
          ErrorLine(error, about: 'Listing every agent’s monitors:'),
        if (list == null && _error == null)
          quiet('Loading monitors…')
        else if (list != null && groups.isEmpty)
          quiet(
            filtered
                ? 'No monitors match.'
                : 'No monitors yet. An agent adds one when it needs to do '
                      'something later, such as a scheduled job or a '
                      'reminder.',
          ),
        for (final MapEntry(key: agentId, value: items) in groups.entries)
          _AgentGroup(
            key: ValueKey(agentId),
            agentId: agentId,
            items: items,
            store: store,
            opened: _opened,
            place: (key) => _rows.putIfAbsent(key, GlobalKey.new),
          ),
      ],
    );
  }
}

/// What points at [agentId]'s monitor [id] (m3 is 3): the Monitors page
/// goes to it, and it glows there.
String monitorKey(String agentId, int id) => 'monitor:$agentId#$id';

/// One agent's monitors under its name, which opens its conversation.
class _AgentGroup extends StatelessWidget {
  const _AgentGroup({
    super.key,
    required this.agentId,
    required this.items,
    required this.store,
    required this.opened,
    required this.place,
  });

  final String agentId;
  final List<AgentMonitor> items;
  final WorkspaceStore store;

  /// The monitor something opened the page at ([monitorKey]), if any.
  final String? opened;

  /// Each monitor's place, by [monitorKey].
  final GlobalKey Function(String key) place;

  void _open(BuildContext context) {
    final backend = AppScope.of(context);
    AppScope.closePage(context);
    fireAndLog(backend.selectAgent(agentId));
    // On a phone, the conversation is pushed over the list.
    PhoneNavigation.of(context)?.value = true;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final agent = store.agent(agentId);
    final project = agent == null || agent.projectId.isEmpty
        ? null
        : store.workspace.projects
              .where((p) => p.id == agent.projectId)
              .firstOrNull;
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Semantics(
            button: agent != null,
            child: Pressable(
              onTap: agent == null ? null : () => _open(context),
              builder: (context, hovered) => Container(
                constraints: BoxConstraints(
                  minHeight: isPhoneLayout(context) ? Touch.target : 32,
                ),
                padding: const EdgeInsets.symmetric(horizontal: Space.s),
                decoration: BoxDecoration(
                  color: hovered ? palette.hover : null,
                  borderRadius: BorderRadius.circular(Radii.control),
                ),
                child: Row(
                  children: [
                    if (agent != null) ...[
                      AgentCharacter(agent: agent, size: 18),
                      const SizedBox(width: Space.s),
                    ],
                    Flexible(
                      child: Text(
                        agent?.name ?? agentId,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TypeScale.of(context).label.copyWith(
                          color: palette.text,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    if (project != null) ...[
                      const SizedBox(width: Space.s),
                      Flexible(
                        child: Text(
                          project.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TypeScale.of(context).caption
                              .copyWith(color: palette.textFaint),
                        ),
                      ),
                    ],
                    const Spacer(),
                    if (agent != null)
                      Text(
                        'Open conversation',
                        style: TypeScale.of(context).caption.copyWith(
                          color: hovered ? palette.text : palette.textMuted,
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ),
          for (final item in items)
            if (monitorKey(item.agentId, item.monitor.id) case final key)
              Glow(
                key: place(key),
                store: AppScope.of(context).highlights,
                matches: (glowing) => glowing == key,
                child: MonitorRow(
                  monitor: item.monitor,
                  agentId: item.agentId,
                  store: store,
                  held: item.held,
                  opened: opened == key,
                  more: (context) => MonitorHistoryView(
                    agentId: item.agentId,
                    monitorId: item.monitor.id,
                  ),
                ),
              ),
        ],
      ),
    );
  }
}

/// What a [MonitorEventKind] is called. A fire is the monitor waking its
/// agent, never the job done: "Fired".
String monitorEventLabel(MonitorEventKind kind) => switch (kind) {
  MonitorEventKind.MONITOR_EVENT_KIND_CREATED => 'Set',
  MonitorEventKind.MONITOR_EVENT_KIND_FIRED => 'Fired',
  MonitorEventKind.MONITOR_EVENT_KIND_PAUSED => 'Paused',
  MonitorEventKind.MONITOR_EVENT_KIND_RESUMED => 'Resumed',
  MonitorEventKind.MONITOR_EVENT_KIND_CANCELLED => 'Cancelled',
  MonitorEventKind.MONITOR_EVENT_KIND_DONE => 'Done',
  MonitorEventKind.MONITOR_EVENT_KIND_ERROR => 'Failed',
  _ => 'Changed',
};

/// One monitor's history, newest first, a page at a time with Show
/// older; asked again when monitors change (a fire, say).
class MonitorHistoryView extends StatefulWidget {
  const MonitorHistoryView({
    super.key,
    required this.agentId,
    required this.monitorId,
  });

  final String agentId;
  final int monitorId;

  @override
  State<MonitorHistoryView> createState() => _MonitorHistoryViewState();
}

class _MonitorHistoryViewState extends State<MonitorHistoryView> {
  final _events = <MonitorEvent>[];
  var _more = false;
  var _loaded = false;
  var _loading = false;
  String? _error;
  var _asked = 0;
  late final Backend _backend;

  @override
  void initState() {
    super.initState();
    _backend = AppScope.of(context);
    _backend.monitorsChanged.addListener(_reload);
    unawaited(_page(fresh: true));
  }

  @override
  void dispose() {
    _backend.monitorsChanged.removeListener(_reload);
    super.dispose();
  }

  void _reload() => unawaited(_page(fresh: true));

  /// The newest page ([fresh]), or the one older than what's shown.
  Future<void> _page({required bool fresh}) async {
    final asked = ++_asked;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final page = await _backend.monitorHistory(
        widget.agentId,
        widget.monitorId,
        beforeSeq: fresh || _events.isEmpty ? Int64.ZERO : _events.last.seq,
      );
      if (!mounted || asked != _asked) return;
      setState(() {
        if (fresh) _events.clear();
        _events.addAll(page.events);
        _more = page.more;
        _loaded = true;
        _loading = false;
      });
    } on RequestFailure catch (failure) {
      if (!mounted || asked != _asked) return;
      setState(() {
        _error = failure.message;
        _loading = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final caption = TypeScale.of(context).caption.copyWith(
      color: palette.textMuted,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final store = AppScope.of(context).workspace;
    String line(MonitorEvent event) {
      final when = logTime(
        DateTime.fromMillisecondsSinceEpoch(event.timeMs.toInt()),
      );
      final actor = event.actor.isEmpty
          ? ''
          : ' by ${store.agent(event.actor)?.name ?? event.actor}';
      // The detail is what agents read, naming an agent by its id ("fired
      // by kai"): an agent's doing says its name instead, and only what it
      // waits for stays, from when it was set.
      final detail =
          event.detail.isEmpty ||
              (event.actor.isNotEmpty &&
                  event.kind != MonitorEventKind.MONITOR_EVENT_KIND_CREATED)
          ? ''
          // The person's doing comes as its detail, read as an agent's is:
          // "Paused by Matt", as "Paused by Rivet".
          : event.actor.isEmpty && event.detail.startsWith('by ')
          ? ' ${event.detail}'
          : ': ${event.detail}';
      return '$when  ${monitorEventLabel(event.kind)}$actor$detail';
    }

    return Padding(
      padding: const EdgeInsets.only(top: Space.s),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'History',
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textFaint),
          ),
          const SizedBox(height: 2),
          if (!_loaded && _error == null)
            Text('Loading…', style: caption.copyWith(color: palette.textFaint))
          else if (_loaded && _events.isEmpty)
            Text(
              'Nothing yet.',
              style: caption.copyWith(color: palette.textFaint),
            ),
          for (final event in _events)
            Text(
              line(event),
              style: caption.copyWith(
                color: event.kind == MonitorEventKind.MONITOR_EVENT_KIND_ERROR
                    ? palette.danger
                    : null,
              ),
            ),
          if (_error case final error?)
            ErrorLine(
              error,
              about:
                  'Reading the history of monitor m${widget.monitorId} of '
                  'agent ${widget.agentId}:',
            ),
          if (_more) ...[
            const SizedBox(height: Space.xs),
            Unselectable(
              child: Button(
                label: _loading ? 'Loading…' : 'Show older',
                onPressed: _loading ? null : () => _page(fresh: false),
              ),
            ),
          ],
        ],
      ),
    );
  }
}
