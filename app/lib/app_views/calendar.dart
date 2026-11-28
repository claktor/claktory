// The catalog's Calendar (Lucy's ux/calendar.md): what an app (Claktory's
// Calendar) puts on a calendar, as a week or a day of hours, or an agenda
// by day, in the device's own time. Each entry opens its details beside it
// (a sheet on a phone), with one button to its place in Claktory.
import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart' show Button, ExtensionGlyph, Pressable;
import '../identity.dart';
import '../mobile/metrics.dart';
import '../mobile/popover.dart';
import '../mobile/presentation.dart' show presentSheet;
import '../open_place.dart';
import '../scope.dart';
import '../visible_codes.dart';
import 'charts.dart' show AppTable, statColor;

/// The most entries a calendar draws.
const maxCalendarEntries = 2000;

/// Under this width a calendar has no week: an agenda or a day, as a
/// phone's calendar.
const calendarWideFrom = 600.0;

/// One entry on a calendar, from a row of the app's data.
@immutable
class CalendarEntry {
  const CalendarEntry({
    required this.id,
    required this.start,
    this.end,
    required this.title,
    this.agent = '',
    this.color = '',
    this.allDay = false,
    this.kind = 'event',
    this.repeat = '',
    this.detail = '',
    this.open,
    this.live = false,
  });

  final String id;

  /// When it starts; for an all-day one, the local midnight of its first
  /// day.
  final DateTime start;

  /// When it ends; for an all-day one, the local midnight after its last
  /// day. Null for a point in time (a monitor's fire), or a [live] one.
  final DateTime? end;
  final String title;

  /// Its agent's id; empty for none.
  final String agent;

  /// A chart palette name or `agent:<id>`; its agent's color when empty.
  final String color;
  final bool allDay;

  /// monitor, fire, item, task, run, event or deadline.
  final String kind;

  /// How it repeats, in words ("every 6 hours"); empty for once.
  final String repeat;

  /// Where it comes from, in a sentence.
  final String detail;

  /// Its place in Claktory; null for none.
  final OpenTarget? open;

  /// Running now: it lasts until now.
  final bool live;

  /// A point in time: a pill at its time, not a block.
  bool get point => !allDay && !live && end == null;

  /// Where it ends at [now]: its end, now for a live one, its start for a
  /// point.
  DateTime until(DateTime now) => end ?? (live ? now : start);
}

/// An app's row's place in Claktory: `agent:<id>`, `item:<agent>:<n>`,
/// `monitor:<agent>:<m3 or 3>`, `task:<agent>:<task>`; null for anything
/// else.
OpenTarget? parseOpenTarget(Object? text) {
  if (text is! String) return null;
  final parts = text.split(':');
  bool id(String s) => RegExp(r'^[A-Za-z0-9_-]{1,64}$').hasMatch(s);
  return switch (parts) {
    ['agent', final agent] when id(agent) => OpenAgent(agent),
    ['item', final agent, final n] when id(agent) => switch (int.tryParse(n)) {
      final item? when item >= 0 => OpenItem(agent, item),
      _ => null,
    },
    ['monitor', final agent, final m] when id(agent) => switch (int.tryParse(
      m.startsWith('m') ? m.substring(1) : m,
    )) {
      final monitor? when monitor >= 0 => OpenMonitor(agent, monitor),
      _ => null,
    },
    ['task', final agent, final task] when id(agent) && id(task) => OpenTask(
      agent,
      task,
    ),
    _ => null,
  };
}

/// The entries in [table]'s rows, by its columns' names; rows without an
/// id, a title or a start are left out.
List<CalendarEntry> calendarEntries(AppTable? table) {
  if (table == null) return const [];
  int at(String name) => table.columns.indexOf(name);
  final columns = {
    for (final name in [
      'id',
      'start',
      'end',
      'title',
      'agent',
      'color',
      'allDay',
      'kind',
      'repeat',
      'detail',
      'open',
      'live',
    ])
      name: at(name),
  };
  Object? cell(List<Object?> row, String name) {
    final i = columns[name]!;
    return i < 0 || i >= row.length ? null : row[i];
  }

  // Agents' words, some maybe copied from outside: what would hide or
  // reorder them shown by its code, and one line but for the details.
  String text(List<Object?> row, String name, {bool lines = false}) =>
      switch (cell(row, name)) {
        final String s => visibleCodes(lines ? s : calendarLine(s)),
        final num n => '$n',
        _ => '',
      };
  // An all-day one's dates are UTC midnights: the same days everywhere.
  DateTime? when(Object? ms, {required bool day}) {
    if (ms is! num || !ms.isFinite || ms < 0 || ms > 8.64e15) return null;
    final time = DateTime.fromMillisecondsSinceEpoch(ms.toInt(), isUtc: day);
    return day ? DateTime(time.year, time.month, time.day) : time.toLocal();
  }

  const kinds = {'monitor', 'fire', 'item', 'task', 'run', 'event', 'deadline'};
  final out = <CalendarEntry>[];
  for (final row in table.rows) {
    if (out.length >= maxCalendarEntries) break;
    final allDay = cell(row, 'allDay') == true;
    final start = when(cell(row, 'start'), day: allDay);
    final title = text(row, 'title');
    if (start == null || title.isEmpty) continue;
    var end = when(cell(row, 'end'), day: allDay);
    if (allDay && (end == null || !end.isAfter(start))) {
      end = DateTime(start.year, start.month, start.day + 1);
    }
    if (end != null && end.isBefore(start)) end = null;
    final kind = text(row, 'kind');
    final live = cell(row, 'live') == true && !allDay;
    out.add(
      CalendarEntry(
        id: text(row, 'id'),
        start: start,
        end: live ? null : end,
        title: title,
        agent: text(row, 'agent'),
        color: text(row, 'color'),
        allDay: allDay,
        kind: kinds.contains(kind) ? kind : 'event',
        repeat: text(row, 'repeat'),
        detail: text(row, 'detail', lines: true),
        open: parseOpenTarget(cell(row, 'open')),
        live: live,
      ),
    );
  }
  out.sort((a, b) => a.start.compareTo(b.start));
  return out;
}

/// [text] on one line: its breaks and tabs as spaces.
String calendarLine(String text) =>
    text.replaceAll(RegExp('[\r\n\t\u0085\u2028\u2029]+'), ' ');

/// The Mac's UTC offset, in minutes, at either end of what's shown, and
/// its place's name when known.
typedef MacZone = ({int start, int end, String name});

/// A calendar: [view] (week, day or agenda; under [calendarWideFrom] wide,
/// agenda for a week) around [anchor], with its bar: ‹ Today ›, the range,
/// the app's [tools] and the views. Every move (a button, a swipe, a key, a
/// day of the phone's strip) goes to [onMove], which the app answers with
/// the entries for it.
class AppCalendar extends StatefulWidget {
  const AppCalendar({
    super.key,
    required this.entries,
    this.view = 'week',
    this.anchor,
    this.weekStart,
    this.macZone,
    this.tools = const [],
    this.emptyTitle = '',
    this.agendaEmptyTitle = '',
    this.emptyBody = '',
    this.height,
    this.onMove,
  });

  final List<CalendarEntry> entries;
  final String view;

  /// The day or week shown holds it; now when null.
  final DateTime? anchor;

  /// [DateTime.monday], [DateTime.sunday] or [DateTime.saturday]; the
  /// device's own when null.
  final int? weekStart;
  final MacZone? macZone;

  /// The app's own controls on the bar (Agent, Show, ⋯).
  final List<Widget> tools;
  final String emptyTitle;

  /// The title over an empty Agenda, which a narrow screen draws whatever
  /// [view] says; [emptyTitle] when empty.
  final String agendaEmptyTitle;
  final String emptyBody;

  /// Its whole height; what's left of the screen when null.
  final double? height;
  final void Function(DateTime anchor, String view)? onMove;

  /// Now, as the calendar reads it: tests set another.
  @visibleForTesting
  static DateTime Function() clock = DateTime.now;

  @override
  State<AppCalendar> createState() => _AppCalendarState();
}

DateTime _day(DateTime t) => DateTime(t.year, t.month, t.day);

DateTime _addDays(DateTime day, int days) =>
    DateTime(day.year, day.month, day.day + days);

bool _sameDay(DateTime a, DateTime b) =>
    a.year == b.year && a.month == b.month && a.day == b.day;

const _weekdays = [
  'Monday',
  'Tuesday',
  'Wednesday',
  'Thursday',
  'Friday',
  'Saturday',
  'Sunday',
];
const _months = [
  'January',
  'February',
  'March',
  'April',
  'May',
  'June',
  'July',
  'August',
  'September',
  'October',
  'November',
  'December',
];

String _short(String word) => word.substring(0, 3);

/// "18:00".
String calendarClock(DateTime t) =>
    '${t.hour.toString().padLeft(2, '0')}:${t.minute.toString().padLeft(2, '0')}';

/// "Thursday 3 December", with the year when it isn't [now]'s.
String calendarDate(DateTime day, DateTime now) =>
    '${_weekdays[day.weekday - 1]} ${day.day} ${_months[day.month - 1]}'
    '${day.year == now.year ? '' : ' ${day.year}'}';

/// "30 Nov – 6 Dec 2026", "1 – 7 Dec 2026".
String calendarWeek(DateTime first, DateTime last) {
  String month(DateTime d) => _short(_months[d.month - 1]);
  if (first.year != last.year) {
    return '${first.day} ${month(first)} ${first.year} – '
        '${last.day} ${month(last)} ${last.year}';
  }
  if (first.month != last.month) {
    return '${first.day} ${month(first)} – ${last.day} ${month(last)} '
        '${last.year}';
  }
  return '${first.day} – ${last.day} ${month(last)} ${last.year}';
}

/// "1 h 10 min", "25 min", "3 h".
String calendarLength(Duration length) {
  final minutes = math.max(0, length.inMinutes);
  final h = minutes ~/ 60;
  final m = minutes % 60;
  if (minutes == 0) return 'under a minute';
  if (h == 0) return '$m min';
  return m == 0 ? '$h h' : '$h h $m min';
}

/// What kind of entry it is, in words ("checklist item"); empty for a
/// placed event.
String _kindWords(String kind) => switch (kind) {
  'monitor' || 'fire' => 'monitor',
  'item' => 'checklist item',
  'task' => 'task',
  'deadline' => 'deadline',
  _ => '',
};

/// The button to an entry's place.
String _openWords(OpenTarget target) => switch (target) {
  OpenMonitor() => 'Open monitor',
  OpenItem() => 'Open item',
  OpenTask() => 'Open task',
  _ => 'Open conversation',
};

/// The agent's name, or its id when it's gone.
({String name, String role, Color? color}) _agent(
  BuildContext context,
  String id,
) {
  final agent = context
      .getInheritedWidgetOfExactType<AppScope>()
      ?.backend
      .workspace
      .agent(id);
  return (
    name: agent?.name ?? id,
    role: agent?.role ?? '',
    color: statColor(context, 'agent:$id'),
  );
}

/// An entry's color: its own, else its agent's, else muted.
Color _color(BuildContext context, CalendarEntry entry) =>
    (entry.color.isEmpty ? null : statColor(context, entry.color)) ??
    (entry.agent.isEmpty ? null : statColor(context, 'agent:${entry.agent}')) ??
    Palette.of(context).textMuted;

/// When an entry is, in words: "Today, 18:00", "Today, since 13:10",
/// "Friday 4 December, 15:00 – 16:30", "All-day, 3 – 5 December"; nothing
/// for a band, whose days are only those asked for and whose cadence says
/// when it runs.
String _when(CalendarEntry entry, DateTime now) {
  if (_band(entry)) return '';
  final today = _day(now);
  String day(DateTime d) => _sameDay(d, today)
      ? 'Today'
      : _sameDay(d, _addDays(today, 1))
      ? 'Tomorrow'
      : _sameDay(d, _addDays(today, -1))
      ? 'Yesterday'
      : calendarDate(d, now);
  if (entry.allDay) {
    final last = _addDays(entry.end ?? entry.start, -1);
    if (!last.isAfter(entry.start)) return '${day(entry.start)}, all-day';
    return 'All-day, ${day(entry.start)} – ${day(last)}';
  }
  if (entry.live) {
    return '${day(_day(entry.start))}, since ${calendarClock(entry.start)}';
  }
  final end = entry.end;
  if (end == null) {
    return '${day(_day(entry.start))}, ${calendarClock(entry.start)}';
  }
  final ends = _sameDay(entry.start, end)
      ? calendarClock(end)
      : '${day(_day(end))} ${calendarClock(end)}';
  return '${day(_day(entry.start))}, ${calendarClock(entry.start)} – $ends';
}

/// How it repeats or how long it's run: "every 6 hours", "running for
/// 1 h 10 min".
String _extra(CalendarEntry entry, DateTime now) => entry.live
    ? '${entry.kind == 'run' ? 'working' : 'running'} for '
          '${calendarLength(now.difference(entry.start))}'
    : entry.repeat;

/// Which entry it is across the app's updates: its id, or its start and
/// title.
String _which(CalendarEntry entry) => entry.id.isNotEmpty
    ? entry.id
    : '${entry.start.millisecondsSinceEpoch} ${entry.title}';

/// A collapsed repeat (an all-day monitor that repeats): drawn on the band
/// as "Fugo · Logs · every 5 min".
bool _band(CalendarEntry entry) =>
    entry.allDay &&
    (entry.kind == 'monitor' || entry.kind == 'fire') &&
    entry.repeat.isNotEmpty;

/// What the band says for [entry].
String _bandTitle(BuildContext context, CalendarEntry entry) => _band(entry)
    ? [
        if (entry.agent.isNotEmpty) _agent(context, entry.agent).name,
        entry.title,
        entry.repeat,
      ].join(' · ')
    : entry.title;

/// What a screen reader says for an entry: "Production logs, Pericolo's
/// monitor, today at 18:00, every 6 hours".
String _spoken(BuildContext context, CalendarEntry entry, DateTime now) {
  final who = entry.agent.isEmpty ? '' : _agent(context, entry.agent).name;
  final what = _kindWords(entry.kind);
  final whose = who.isEmpty
      ? what
      : entry.kind == 'run'
      ? who
      : '$who\'s ${what.isEmpty ? 'event' : what}';
  // "today at 18:00", "today since 13:10", "today, all-day".
  var when = _when(entry, now);
  for (final (shown, said) in [
    ('Today, ', 'today'),
    ('Tomorrow, ', 'tomorrow'),
  ]) {
    if (!when.startsWith(shown)) continue;
    final rest = when.substring(shown.length);
    when = rest.startsWith(RegExp('[0-9]'))
        ? '$said at $rest'
        : rest.startsWith('since ')
        ? '$said $rest'
        : '$said, $rest';
  }
  return [
    entry.title,
    if (whose.isNotEmpty) whose,
    if (when.isNotEmpty) when,
    if (_extra(entry, now).isNotEmpty) _extra(entry, now),
  ].join(', ');
}

class _AppCalendarState extends State<AppCalendar> {
  Timer? _tick;

  /// Today's entries already over, shown in the agenda.
  var _earlierOpen = false;

  /// The entry whose details are open, by [_which].
  String? _selected;

  /// The last press was on the calendar: its keys are on until a press
  /// elsewhere.
  var _active = false;

  /// What was shown at the last build, for the keys.
  var _shown = (view: 'week', wide: true);

  @override
  void initState() {
    super.initState();
    HardwareKeyboard.instance.addHandler(_onKey);
    // The now line and what's running move on each half minute.
    _tick = Timer.periodic(
      const Duration(seconds: 30),
      (_) => mounted ? setState(() {}) : null,
    );
  }

  @override
  void dispose() {
    HardwareKeyboard.instance.removeHandler(_onKey);
    _tick?.cancel();
    super.dispose();
  }

  DateTime get _now => AppCalendar.clock();

  DateTime get _anchor => _day(widget.anchor ?? _now);

  int get _weekStart =>
      widget.weekStart ??
      (switch (Localizations.maybeLocaleOf(context)?.countryCode) {
        'US' || 'CA' || 'JP' || 'BR' || 'MX' || 'IL' => DateTime.sunday,
        _ => DateTime.monday,
      });

  DateTime _weekOf(DateTime day) =>
      _addDays(day, -((day.weekday - _weekStart) % 7));

  void _move(DateTime anchor, String view) =>
      widget.onMove?.call(_day(anchor), view);

  /// A step back (-1) or on (1): a week, or a day in Day.
  void _step(String view, int by) =>
      _move(_addDays(_anchor, view == 'day' ? by : 7 * by), view);

  /// Whether the keys are the calendar's now: the last press was on it,
  /// and what has focus is around it (its page's selection), not a field
  /// or a popover elsewhere.
  bool get _ours {
    if (!_active || !mounted) return false;
    final focused = FocusManager.instance.primaryFocus?.context;
    if (focused == null) return true;
    if (focused == context) return true;
    var around = false;
    context.visitAncestorElements((element) {
      if (element == focused) around = true;
      return !around;
    });
    return around;
  }

  bool _onKey(KeyEvent event) {
    if (!_ours) return false;
    if (HardwareKeyboard.instance.isMetaPressed ||
        HardwareKeyboard.instance.isControlPressed ||
        HardwareKeyboard.instance.isAltPressed) {
      return false;
    }
    return _key(_shown.view, _shown.wide, event) == KeyEventResult.handled;
  }

  KeyEventResult _key(String view, bool wide, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.arrowLeft) {
      _step(view, -1);
    } else if (key == LogicalKeyboardKey.arrowRight) {
      _step(view, 1);
    } else if (event is KeyRepeatEvent) {
      return KeyEventResult.ignored;
    } else if (key == LogicalKeyboardKey.keyT) {
      _move(_now, view);
    } else if (key == LogicalKeyboardKey.keyD) {
      _move(_anchor, 'day');
    } else if (key == LogicalKeyboardKey.keyW && wide) {
      _move(_anchor, 'week');
    } else if (key == LogicalKeyboardKey.keyA) {
      _move(_anchor, 'agenda');
    } else {
      return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth.isFinite
            ? constraints.maxWidth
            : 800.0;
        final wide = width >= calendarWideFrom;
        final view = switch (widget.view) {
          'week' when !wide => 'agenda',
          'day' || 'agenda' || 'week' => widget.view,
          _ => wide ? 'week' : 'agenda',
        };
        final height =
            widget.height ??
            math.max(420.0, MediaQuery.sizeOf(context).height - 180);
        final body = switch (view) {
          'agenda' => _agenda(context),
          _ => _grid(context, view, wide),
        };
        _shown = (view: view, wide: wide);
        // A press on it turns its keys on (← → T D W A), one elsewhere off.
        return TapRegion(
          onTapOutside: (_) => _active = false,
          child: Listener(
            onPointerDown: (_) => _active = true,
            child: SizedBox(
              height: height,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _bar(context, view, wide),
                  if (_caption(context) case final caption?)
                    Padding(
                      padding: const EdgeInsets.only(bottom: Space.s),
                      child: caption,
                    ),
                  Expanded(child: body),
                ],
              ),
            ),
          ),
        );
      },
    );
  }

  /// The range shown, as the bar's heading.
  String _heading(String view) {
    final now = _now;
    if (view == 'week') {
      final first = _weekOf(_anchor);
      return calendarWeek(first, _addDays(first, 6));
    }
    return calendarDate(_anchor, now);
  }

  /// "Your time · Studio is 6 hours behind" where the device's zone isn't
  /// the Mac's anywhere in what's shown.
  Widget? _caption(BuildContext context) {
    final zone = widget.macZone;
    if (zone == null) return null;
    final first = _weekOf(_anchor);
    final last = _addDays(first, 7);
    final mine = [
      first.timeZoneOffset.inMinutes,
      last.timeZoneOffset.inMinutes,
    ];
    final theirs = [zone.start, zone.end];
    var off = 0;
    for (var i = 0; i < 2; i++) {
      if (mine[i] != theirs[i]) off = theirs[i] - mine[i];
    }
    if (off == 0) return null;
    final name =
        context
            .getInheritedWidgetOfExactType<AppScope>()
            ?.backend
            .target
            ?.label ??
        'Claktory';
    final String words;
    if (zone.name.isNotEmpty) {
      words =
          'Your time · $name is on ${visibleCodes(calendarLine(zone.name))} time';
    } else {
      final hours = off.abs() ~/ 60;
      final half = off.abs() % 60 >= 30;
      final count = half ? '$hours½' : '$hours';
      final unit = hours == 1 && !half ? 'hour' : 'hours';
      words =
          'Your time · $name is $count $unit ${off < 0 ? 'behind' : 'ahead'}';
    }
    final type = TypeScale.of(context);
    return Text(
      words,
      style: type.caption.copyWith(color: Palette.of(context).textMuted),
    );
  }

  Widget _bar(BuildContext context, String view, bool wide) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final views = wide
        ? const ['day', 'week', 'agenda']
        : const ['agenda', 'day'];
    final switcher = _Segments(
      options: views,
      chosen: view,
      choose: (next) => _move(_anchor, next),
    );
    if (!wide) {
      return Padding(
        padding: const EdgeInsets.only(bottom: Space.s),
        child: Row(
          children: [
            switcher,
            const Spacer(),
            // The first of the app's tools (Agent) and its last (⋯).
            for (final (i, tool) in widget.tools.indexed)
              if (i == 0 || i == widget.tools.length - 1)
                Padding(
                  padding: const EdgeInsets.only(left: Space.xs),
                  child: tool,
                ),
          ],
        ),
      );
    }
    Widget arrow(String icon, String label, int by) => Semantics(
      button: true,
      label: label,
      excludeSemantics: true,
      onTap: () => _step(view, by),
      child: Pressable(
        onTap: () => _step(view, by),
        builder: (context, hovered) => Container(
          width: 28,
          height: 28,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: hovered ? palette.hover : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: ExtensionGlyph(icon: icon, name: '', color: palette.textMuted),
        ),
      ),
    );
    // Where, then the app's tools and the views: on one line where they
    // fit, else the tools under where, the heading never cut.
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.s),
      child: LayoutBuilder(
        builder: (context, constraints) => Wrap(
          alignment: WrapAlignment.spaceBetween,
          crossAxisAlignment: WrapCrossAlignment.center,
          spacing: Space.l,
          runSpacing: Space.xs,
          children: [
            Row(
              mainAxisSize: MainAxisSize.min,
              spacing: Space.xs,
              children: [
                arrow(
                  'chevron-left',
                  view == 'day' ? 'Previous day' : 'Previous week',
                  -1,
                ),
                Button(label: 'Today', onPressed: () => _move(_now, view)),
                arrow(
                  'chevron-right',
                  view == 'day' ? 'Next day' : 'Next week',
                  1,
                ),
                const SizedBox(width: Space.s),
                ConstrainedBox(
                  constraints: BoxConstraints(
                    maxWidth: math.max(0, constraints.maxWidth - 200),
                  ),
                  child: Semantics(
                    header: true,
                    child: Text(
                      _heading(view),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      // A screen's title on a Mac; on a touch screen, where
                      // the bar's controls are a finger's size, a heading's.
                      style: (touchFirst(context) ? type.emphasis : type.title)
                          .copyWith(color: palette.text),
                    ),
                  ),
                ),
              ],
            ),
            // The app's tools (Agent, Show), the views, then its last (⋯).
            Row(
              mainAxisSize: MainAxisSize.min,
              spacing: Space.xs,
              children: [
                ...widget.tools.take(math.max(0, widget.tools.length - 1)),
                const SizedBox(width: Space.xs),
                switcher,
                ?widget.tools.lastOrNull,
              ],
            ),
          ],
        ),
      ),
    );
  }

  /// An entry's details: beside it on a Mac or iPad, a sheet on a phone.
  void _details(BuildContext from, CalendarEntry entry) {
    final place = OpenPlace.maybeOf(from);
    Widget card(BuildContext context, void Function([Object?]) close) =>
        _Details(
          entry: entry,
          now: _now,
          place: place,
          openFrom: from,
          close: close,
        );
    // Ringed while its details are open.
    setState(() => _selected = _which(entry));
    void closed(Object? _) {
      if (mounted && _selected == _which(entry)) {
        setState(() => _selected = null);
      }
    }

    if (isPhoneLayout(from)) {
      unawaited(presentSheet<Object>(from, builder: card).then(closed));
      return;
    }
    unawaited(
      presentPopover<Object>(
        from,
        beside: true,
        builder: (context, close) =>
            SizedBox(width: popoverWidth, child: card(context, close)),
      ).then(closed),
    );
  }

  /// A horizontal swipe moves by [step] (a week or a day), as iPhone and
  /// iPad Calendar do.
  Widget _swipe(Widget child, void Function(int by) step) => GestureDetector(
    behavior: HitTestBehavior.translucent,
    onHorizontalDragEnd: (details) {
      final speed = details.primaryVelocity ?? 0;
      if (speed.abs() < 200) return;
      step(speed < 0 ? 1 : -1);
    },
    child: child,
  );

  // ---- Week and day ----

  Widget _grid(BuildContext context, String view, bool wide) {
    final palette = Palette.of(context);
    final days = view == 'week'
        ? [for (var i = 0; i < 7; i++) _addDays(_weekOf(_anchor), i)]
        : [_anchor];
    final grid = _TimeGrid(
      days: days,
      entries: widget.entries,
      now: _now,
      header: view == 'week' || wide,
      onTap: _details,
      selected: _selected,
      spoken: (entry) => _spoken(context, entry, _now),
    );
    final empty = _emptyOver(days.first, _addDays(days.last, 1));
    final swiped = _swipe(
      Stack(
        children: [
          Positioned.fill(child: grid),
          ?empty,
        ],
      ),
      (by) => _step(view, by),
    );
    if (wide || view != 'day') return swiped;
    // A phone's day: the week's days over it, today in signal; a tap moves
    // to one, a swipe across them a week.
    final type = TypeScale.of(context);
    final first = _weekOf(_anchor);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _swipe(
          Row(
            children: [
              for (var i = 0; i < 7; i++)
                Expanded(
                  child: _StripDay(
                    day: _addDays(first, i),
                    today: _sameDay(_addDays(first, i), _now),
                    chosen: _sameDay(_addDays(first, i), _anchor),
                    onTap: () => _move(_addDays(first, i), 'day'),
                  ),
                ),
            ],
          ),
          (by) => _step('week', by),
        ),
        Container(height: 1, color: palette.hairline),
        Padding(
          padding: const EdgeInsets.symmetric(vertical: Space.s),
          child: Semantics(
            header: true,
            child: Text(
              calendarDate(_anchor, _now),
              style: type.emphasis.copyWith(color: palette.text),
            ),
          ),
        ),
        Expanded(child: swiped),
      ],
    );
  }

  /// The app's words for nothing between [from] and [to], over the empty
  /// grid; null when something's there.
  Widget? _emptyOver(DateTime from, DateTime to) {
    if (widget.emptyTitle.isEmpty && widget.emptyBody.isEmpty) return null;
    final now = _now;
    final any = widget.entries.any(
      (e) => e.start.isBefore(to) && !e.until(now).isBefore(from),
    );
    if (any) return null;
    return Positioned.fill(
      child: IgnorePointer(
        child: Center(
          child: _Empty(title: widget.emptyTitle, body: widget.emptyBody),
        ),
      ),
    );
  }

  // ---- Agenda ----

  Widget _agenda(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final now = _now;
    final today = _day(now);
    final first = _anchor;
    // Each entry on its first day shown: an all-day one that began before
    // on the first day.
    final byDay = <DateTime, List<CalendarEntry>>{};
    for (final entry in widget.entries) {
      final last = entry.allDay
          ? _addDays(entry.end ?? entry.start, -1)
          : _day(entry.until(now));
      if (last.isBefore(first)) continue;
      final day = entry.start.isBefore(first) ? first : _day(entry.start);
      (byDay[day] ??= []).add(entry);
    }
    final days = byDay.keys.toList()..sort();
    if (days.isEmpty) {
      return _swipe(
        Center(
          child: _Empty(
            title: widget.agendaEmptyTitle.isEmpty
                ? widget.emptyTitle
                : widget.agendaEmptyTitle,
            body: widget.emptyBody,
          ),
        ),
        (by) => _step('agenda', by),
      );
    }
    final rows = <Widget>[];
    for (final day in days) {
      final entries = byDay[day]!
        ..sort((a, b) {
          if (a.allDay != b.allDay) return a.allDay ? -1 : 1;
          return a.start.compareTo(b.start);
        });
      final name = _sameDay(day, today)
          ? 'Today'
          : _sameDay(day, _addDays(today, 1))
          ? 'Tomorrow'
          : null;
      rows.add(
        Padding(
          padding: EdgeInsets.only(
            top: rows.isEmpty ? 0 : Space.l,
            bottom: Space.xs,
          ),
          child: Semantics(
            header: true,
            child: Text.rich(
              TextSpan(
                children: [
                  if (name != null)
                    TextSpan(
                      text: '$name  ',
                      style: type.label.copyWith(color: palette.text),
                    ),
                  TextSpan(
                    text: calendarDate(day, now),
                    style: name == null
                        ? type.label.copyWith(color: palette.text)
                        : type.secondary.copyWith(color: palette.textFaint),
                  ),
                ],
              ),
            ),
          ),
        ),
      );
      rows.add(Container(height: 1, color: palette.hairline));
      if (!_sameDay(day, today)) {
        rows.addAll([for (final e in entries) _row(context, e, now)]);
        continue;
      }
      // Today: what's over folds away, then what's still going, the now
      // line, and what's coming.
      final allDay = [
        for (final e in entries)
          if (e.allDay) e,
      ];
      final over = [
        for (final e in entries)
          if (!e.allDay && !e.live && e.until(now).isBefore(now)) e,
      ];
      final going = [
        for (final e in entries)
          if (!e.allDay && !over.contains(e) && !e.start.isAfter(now)) e,
      ];
      final coming = [
        for (final e in entries)
          if (!e.allDay && e.start.isAfter(now)) e,
      ];
      rows.addAll([for (final e in allDay) _row(context, e, now)]);
      if (over.isNotEmpty) {
        rows.add(
          _EarlierToday(
            count: over.length,
            open: _earlierOpen,
            toggle: () => setState(() => _earlierOpen = !_earlierOpen),
          ),
        );
        if (_earlierOpen) {
          rows.addAll([for (final e in over) _row(context, e, now)]);
        }
      }
      rows.addAll([for (final e in going) _row(context, e, now)]);
      rows.add(_AgendaNow(now: now));
      rows.addAll([for (final e in coming) _row(context, e, now)]);
    }
    return _swipe(
      ListView(
        physics: const ClampingScrollPhysics(),
        padding: EdgeInsets.zero,
        children: rows,
      ),
      (by) => _step('agenda', by),
    );
  }

  Widget _row(BuildContext context, CalendarEntry entry, DateTime now) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final color = _color(context, entry);
    final end = entry.end;
    final String time;
    if (entry.allDay) {
      time = 'all-day';
    } else if (entry.live) {
      time = '${calendarClock(entry.start)} – now';
    } else if (end == null) {
      time = calendarClock(entry.start);
    } else if (_sameDay(entry.start, end)) {
      time = '${calendarClock(entry.start)} – ${calendarClock(end)}';
    } else {
      time =
          '${calendarClock(entry.start)} – '
          '${_short(_weekdays[end.weekday - 1])} ${calendarClock(end)}';
    }
    final who = entry.agent.isEmpty ? '' : _agent(context, entry.agent).name;
    // A repeat's cadence, then how long an all-day one lasts; a band's
    // cadence line says whether it ends, its days only what's asked for.
    final extra = _extra(entry, now);
    var until = '';
    if (entry.allDay && !_band(entry)) {
      final last = _addDays(entry.end ?? entry.start, -1);
      if (last.isAfter(entry.start)) {
        until = last.difference(_day(now)).inDays < 7
            ? 'until ${_weekdays[last.weekday - 1]}'
            : 'until ${last.day} ${_short(_months[last.month - 1])}';
      }
    }
    final under = [
      if (who.isNotEmpty) who,
      if (_kindWords(entry.kind).isNotEmpty) _kindWords(entry.kind),
      if (extra.isNotEmpty) extra,
      if (until.isNotEmpty) until,
    ].join(' · ');
    final past = entry.kind == 'fire';
    return Builder(
      builder: (context) => Semantics(
        button: true,
        label: _spoken(context, entry, now),
        excludeSemantics: true,
        onTap: () => _details(context, entry),
        child: Pressable(
          onTap: () => _details(context, entry),
          builder: (context, hovered) => Container(
            constraints: const BoxConstraints(minHeight: 44),
            padding: const EdgeInsets.symmetric(vertical: Space.s),
            decoration: BoxDecoration(
              color: _selected == _which(entry)
                  ? palette.selected
                  : hovered
                  ? palette.hover
                  : null,
              border: Border(bottom: BorderSide(color: palette.hairline)),
            ),
            child: IntrinsicHeight(
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  SizedBox(
                    width: MediaQuery.textScalerOf(context).scale(76),
                    child: Text(
                      time,
                      style: type.secondary.copyWith(
                        color: palette.textMuted,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
                  ),
                  Container(
                    width: 3,
                    margin: const EdgeInsets.only(right: Space.s),
                    decoration: BoxDecoration(
                      color: past ? color.withValues(alpha: 0.4) : color,
                      borderRadius: BorderRadius.circular(1.5),
                    ),
                  ),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          spacing: Space.xs,
                          children: [
                            _Glyph(kind: entry.kind, color: color, size: 14),
                            Flexible(
                              child: Text(
                                entry.title,
                                style: type.label.copyWith(
                                  color: past
                                      ? palette.textMuted
                                      : palette.text,
                                ),
                              ),
                            ),
                          ],
                        ),
                        if (under.isNotEmpty)
                          Text(
                            under,
                            style: type.caption.copyWith(
                              color: palette.textMuted,
                            ),
                          ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Day | Week | Agenda.
class _Segments extends StatelessWidget {
  const _Segments({
    required this.options,
    required this.chosen,
    required this.choose,
  });

  final List<String> options;
  final String chosen;
  final void Function(String view) choose;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    String words(String view) => '${view[0].toUpperCase()}${view.substring(1)}';
    final tall = touchFirst(context) ? 36.0 : 28.0;
    return Container(
      padding: const EdgeInsets.all(2),
      decoration: BoxDecoration(
        color: palette.hover,
        borderRadius: BorderRadius.circular(Radii.control + 2),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final view in options)
            Semantics(
              button: true,
              selected: view == chosen,
              inMutuallyExclusiveGroup: true,
              label: words(view),
              excludeSemantics: true,
              onTap: () => choose(view),
              child: Pressable(
                onTap: () => choose(view),
                builder: (context, hovered) => Container(
                  height: tall,
                  padding: const EdgeInsets.symmetric(horizontal: Space.m),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: view == chosen
                        ? palette.selected
                        : hovered
                        ? palette.hover
                        : null,
                    borderRadius: BorderRadius.circular(Radii.control),
                  ),
                  child: Text(
                    words(view),
                    style: type.label.copyWith(
                      color: view == chosen ? palette.text : palette.textMuted,
                    ),
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// A day of the phone's week strip: its letter over its number, today's in
/// a signal disc, the one shown in a ring.
class _StripDay extends StatelessWidget {
  const _StripDay({
    required this.day,
    required this.today,
    required this.chosen,
    required this.onTap,
  });

  final DateTime day;
  final bool today;
  final bool chosen;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final scaler = MediaQuery.textScalerOf(context);
    // At the largest text the numbers shrink rather than wrap.
    final disc = math.min(scaler.scale(32), 44.0);
    return Semantics(
      button: true,
      selected: chosen,
      label: calendarDate(day, AppCalendar.clock()),
      excludeSemantics: true,
      onTap: onTap,
      child: Pressable(
        onTap: onTap,
        builder: (context, _) => ConstrainedBox(
          constraints: const BoxConstraints(minHeight: 44),
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: Space.xs),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  _weekdays[day.weekday - 1][0],
                  textScaler: TextScaler.linear(math.min(scaler.scale(1), 1.6)),
                  style: type.caption.copyWith(color: palette.textFaint),
                ),
                Container(
                  width: disc,
                  height: disc,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: today ? palette.signal : null,
                    border: chosen && !today
                        ? Border.all(color: palette.text)
                        : null,
                  ),
                  child: FittedBox(
                    child: Text(
                      '${day.day}',
                      style: type.body.copyWith(
                        color: today ? const Color(0xFFFFFFFF) : palette.text,
                        fontWeight: today || chosen ? FontWeight.w600 : null,
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// "Earlier today · 3", folding away what's over.
class _EarlierToday extends StatelessWidget {
  const _EarlierToday({
    required this.count,
    required this.open,
    required this.toggle,
  });

  final int count;
  final bool open;
  final VoidCallback toggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Semantics(
      button: true,
      expanded: open,
      label: 'Earlier today, $count',
      excludeSemantics: true,
      onTap: toggle,
      child: Pressable(
        onTap: toggle,
        builder: (context, hovered) => Container(
          constraints: const BoxConstraints(minHeight: 44),
          decoration: BoxDecoration(
            color: hovered ? palette.hover : null,
            border: Border(bottom: BorderSide(color: palette.hairline)),
          ),
          child: Row(
            spacing: Space.xs,
            children: [
              ExtensionGlyph(
                icon: open ? 'chevron-down' : 'chevron-right',
                name: '',
                color: palette.textMuted,
                size: 14,
              ),
              Text(
                'Earlier today · $count',
                style: type.secondary.copyWith(color: palette.textMuted),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The agenda's now: the time in signal and a line to the edge.
class _AgendaNow extends StatelessWidget {
  const _AgendaNow({required this.now});

  final DateTime now;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Semantics(
      label: 'Now, ${calendarClock(now)}',
      excludeSemantics: true,
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: Space.xs),
        child: Row(
          children: [
            SizedBox(
              width: MediaQuery.textScalerOf(context).scale(76),
              child: Text(
                calendarClock(now),
                style: type.caption.copyWith(
                  color: palette.signal,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            Expanded(child: Container(height: 1.5, color: palette.signal)),
          ],
        ),
      ),
    );
  }
}

/// The app's words for an empty range.
class _Empty extends StatelessWidget {
  const _Empty({required this.title, required this.body});

  final String title;
  final String body;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 360),
      child: Padding(
        padding: const EdgeInsets.all(Space.l),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          spacing: Space.xs,
          children: [
            if (title.isNotEmpty)
              Text(
                title,
                textAlign: TextAlign.center,
                style: type.label.copyWith(color: palette.text),
              ),
            if (body.isNotEmpty)
              Text(
                body,
                textAlign: TextAlign.center,
                style: type.secondary.copyWith(color: palette.textMuted),
              ),
          ],
        ),
      ),
    );
  }
}

/// An entry's details (Lucy §5): what, when, who, where it comes from, and
/// the one button to its place, or why it has none.
class _Details extends StatefulWidget {
  const _Details({
    required this.entry,
    required this.now,
    required this.place,
    required this.openFrom,
    required this.close,
  });

  final CalendarEntry entry;
  final DateTime now;
  final OpenPlace? place;

  /// Where it was opened from: the calendar, under it.
  final BuildContext openFrom;
  final void Function([Object?]) close;

  @override
  State<_Details> createState() => _DetailsState();
}

class _DetailsState extends State<_Details> {
  /// Why its place has gone; null until asked, or when it hasn't.
  Future<String?>? _gone;

  @override
  void initState() {
    super.initState();
    final target = widget.entry.open;
    final place = widget.place;
    // An answer slower than a second gets the button anyway: it never
    // flips to a sentence once shown, and the tap finds what has gone.
    if (target != null && place != null) {
      _gone = place
          .gone(target)
          .then<String?>((why) => why)
          .timeout(const Duration(seconds: 1), onTimeout: () => null);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final entry = widget.entry;
    final color = _color(context, entry);
    final agent = entry.agent.isEmpty ? null : _agent(context, entry.agent);
    final extra = _extra(entry, widget.now);
    final target = entry.open;
    final place = widget.place;
    final phone = isPhoneLayout(context);
    return Padding(
      padding: phone ? EdgeInsets.zero : const EdgeInsets.all(Space.l),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        spacing: Space.s,
        children: [
          Row(
            spacing: Space.s,
            children: [
              _Glyph(kind: entry.kind, color: color, size: 16),
              Expanded(
                child: Semantics(
                  header: true,
                  child: Text(
                    entry.title,
                    style: type.emphasis.copyWith(color: palette.text),
                  ),
                ),
              ),
            ],
          ),
          Text(
            [
              if (_when(entry, widget.now) case final w when w.isNotEmpty) w,
              if (extra.isNotEmpty) extra,
            ].join(' · '),
            style: type.secondary.copyWith(color: palette.textMuted),
          ),
          if (agent != null)
            Row(
              spacing: Space.s,
              children: [
                _Initial(name: agent.name, color: agent.color ?? color),
                Flexible(
                  child: Text.rich(
                    TextSpan(
                      children: [
                        TextSpan(
                          text: agent.name,
                          style: type.label.copyWith(color: palette.text),
                        ),
                        if (agent.role.isNotEmpty)
                          TextSpan(
                            text: ' · ${agent.role}',
                            style: type.secondary.copyWith(
                              color: palette.textMuted,
                            ),
                          ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
          if (entry.detail.isNotEmpty)
            Text(entry.detail, style: type.body.copyWith(color: palette.text)),
          if (target != null && place != null)
            FutureBuilder<String?>(
              future: _gone,
              builder: (context, answer) {
                final waiting = answer.connectionState != ConnectionState.done;
                final why = waiting || answer.hasError ? null : answer.data;
                if (why != null) {
                  return Text(
                    why,
                    style: type.secondary.copyWith(color: palette.textMuted),
                  );
                }
                final button = Button(
                  label: _openWords(target),
                  primary: true,
                  onPressed: () {
                    widget.close();
                    place.open(widget.openFrom, target);
                  },
                );
                // Its row kept while the answer comes, so nothing moves;
                // a button of its own, apart from the words over it.
                return Semantics(
                  container: true,
                  child: Visibility(
                    visible: !waiting,
                    maintainSize: true,
                    maintainAnimation: true,
                    maintainState: true,
                    child: Padding(
                      padding: const EdgeInsets.only(top: Space.xs),
                      child: phone
                          ? SizedBox(width: double.infinity, child: button)
                          : Align(
                              alignment: Alignment.centerRight,
                              child: button,
                            ),
                    ),
                  ),
                );
              },
            ),
        ],
      ),
    );
  }
}

/// An agent's initial in a disc of its color.
class _Initial extends StatelessWidget {
  const _Initial({required this.name, required this.color});

  final String name;
  final Color color;

  @override
  Widget build(BuildContext context) => Container(
    width: 20,
    height: 20,
    alignment: Alignment.center,
    decoration: BoxDecoration(color: color, shape: BoxShape.circle),
    child: Text(
      name.isEmpty ? '?' : name.characters.first.toUpperCase(),
      textScaler: TextScaler.noScaling,
      style: TextStyles.caption.copyWith(
        color: color.computeLuminance() > 0.5
            ? const Color(0xFF111217)
            : const Color(0xFFFFFFFF),
        fontWeight: FontWeight.w600,
        height: 1,
      ),
    ),
  );
}

/// The small mark of an entry's kind: a monitor's alarm clock, a checklist,
/// a task's prompt, a run's pulse, an event's calendar, a deadline's flag,
/// a past fire's tick.
class _Glyph extends StatelessWidget {
  const _Glyph({required this.kind, required this.color, this.size = 12});

  final String kind;
  final Color color;
  final double size;

  @override
  Widget build(BuildContext context) {
    final icon = switch (kind) {
      'monitor' => 'alarm-clock',
      'fire' => 'check',
      'item' => 'list-checks',
      'task' => 'terminal',
      _ => null,
    };
    return ExcludeSemantics(
      child: icon != null
          ? ExtensionGlyph(icon: icon, name: '', color: color, size: size)
          : CustomPaint(
              size: Size.square(size),
              painter: _OwnGlyph(kind, color),
            ),
    );
  }
}

/// The marks the app's glyph set has none of, drawn on a 24-unit grid
/// with a 2-unit round stroke, as the others are.
class _OwnGlyph extends CustomPainter {
  _OwnGlyph(this.kind, this.color);

  final String kind;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.scale(size.width / 24);
    final pen = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round;
    switch (kind) {
      case 'run':
        canvas.drawPath(
          Path()
            ..moveTo(3, 12)
            ..lineTo(7, 12)
            ..lineTo(10, 5)
            ..lineTo(14, 19)
            ..lineTo(17, 12)
            ..lineTo(21, 12),
          pen,
        );
      case 'deadline':
        canvas.drawPath(
          Path()
            ..moveTo(5, 21)
            ..lineTo(5, 4)
            ..cubicTo(9, 2, 12, 6, 19, 4)
            ..lineTo(19, 13)
            ..cubicTo(12, 15, 9, 11, 5, 13),
          pen,
        );
      default:
        canvas.drawRRect(
          RRect.fromRectAndRadius(
            const Rect.fromLTRB(4, 5, 20, 20),
            const Radius.circular(2),
          ),
          pen,
        );
        canvas.drawLine(const Offset(4, 10), const Offset(20, 10), pen);
        canvas.drawLine(const Offset(8, 3), const Offset(8, 7), pen);
        canvas.drawLine(const Offset(16, 3), const Offset(16, 7), pen);
    }
  }

  @override
  bool shouldRepaint(_OwnGlyph old) => old.kind != kind || old.color != color;
}

// ---- The time grid ----

/// The height of an hour, and of a point's pill.
const _hour = 48.0;
const _pill = 20.0;

/// The gutter of hours at the left.
const _gutter = 52.0;

/// Days of hours side by side (a week, or a day): their names over them,
/// the all-day band, then the hours, entries placed by their times,
/// overlapping ones side by side, and the now line across today.
class _TimeGrid extends StatefulWidget {
  const _TimeGrid({
    required this.days,
    required this.entries,
    required this.now,
    required this.header,
    required this.onTap,
    required this.spoken,
    required this.selected,
  });

  /// The entry whose details are open, by [_which]; null for none.
  final String? selected;

  final List<DateTime> days;
  final List<CalendarEntry> entries;
  final DateTime now;
  final bool header;
  final void Function(BuildContext context, CalendarEntry entry) onTap;
  final String Function(CalendarEntry entry) spoken;

  @override
  State<_TimeGrid> createState() => _TimeGridState();
}

/// An entry's piece in one day: where it starts and ends there, and its
/// column of [columns] among those it overlaps.
class _Piece {
  _Piece(this.entry, this.from, this.to);

  final CalendarEntry entry;
  final double from;
  final double to;
  var column = 0;
  var columns = 1;
}

class _TimeGridState extends State<_TimeGrid> {
  late final ScrollController _scroll;

  @override
  void initState() {
    super.initState();
    // The working hours around now when today's shown; else from 8:00.
    final now = widget.now;
    final today = widget.days.any((d) => _sameDay(d, now));
    final hour = today ? now.hour + now.minute / 60 - 1.5 : 7.75;
    _scroll = ScrollController(initialScrollOffset: math.max(0, hour) * _hour);
    // Once it's laid out where it opens, the blocks' titles follow it.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _scroll.dispose();
    super.dispose();
  }

  /// The pieces of the timed entries in [day], laid out side by side where
  /// they overlap (a point taking its pill's room).
  List<_Piece> _pieces(DateTime day) {
    final next = _addDays(day, 1);
    final now = widget.now;
    final pieces = <_Piece>[];
    double hours(DateTime t) => t.isBefore(day)
        ? 0
        : !t.isBefore(next)
        ? 24
        : t.difference(day).inSeconds / 3600;
    for (final entry in widget.entries) {
      if (entry.allDay) continue;
      final end = entry.until(now);
      if (!entry.start.isBefore(next) || end.isBefore(day)) continue;
      if (entry.point && !_sameDay(entry.start, day)) continue;
      final from = hours(entry.start);
      var to = entry.point ? from + _pill / _hour : hours(end);
      if (!entry.point && to - from < _pill / _hour) to = from + _pill / _hour;
      pieces.add(_Piece(entry, from, math.min(24, to)));
    }
    pieces.sort((a, b) => a.from.compareTo(b.from));
    // Clusters of overlapping pieces share the day's width.
    var cluster = <_Piece>[];
    var clusterEnd = -1.0;
    void close() {
      final ends = <double>[];
      for (final piece in cluster) {
        var column = ends.indexWhere((end) => end <= piece.from + 1e-9);
        if (column < 0) {
          ends.add(piece.to);
          column = ends.length - 1;
        } else {
          ends[column] = piece.to;
        }
        piece.column = column;
      }
      for (final piece in cluster) {
        piece.columns = ends.length;
      }
    }

    for (final piece in pieces) {
      if (piece.from >= clusterEnd - 1e-9 && cluster.isNotEmpty) {
        close();
        cluster = [];
      }
      cluster.add(piece);
      clusterEnd = math.max(clusterEnd, piece.to);
    }
    if (cluster.isNotEmpty) close();
    return pieces;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final now = widget.now;
    final days = widget.days;
    final band = _band();
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth.isFinite
            ? constraints.maxWidth
            : 800.0;
        final column = math.max(1.0, (width - _gutter) / days.length);
        final todayAt = days.indexWhere((d) => _sameDay(d, now));
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (widget.header)
              Padding(
                padding: const EdgeInsets.only(left: _gutter, bottom: Space.xs),
                child: Row(
                  children: [
                    for (final (i, day) in days.indexed)
                      SizedBox(
                        width: column,
                        child: _DayName(day: day, today: i == todayAt),
                      ),
                  ],
                ),
              ),
            if (band.lanes > 0)
              _bandView(context, band, column)
            else
              Container(height: 1, color: palette.hairline),
            Expanded(
              child: SingleChildScrollView(
                controller: _scroll,
                physics: const ClampingScrollPhysics(),
                child: SizedBox(
                  height: _hour * 24 + Space.s,
                  child: Stack(
                    clipBehavior: Clip.none,
                    children: [
                      // The hours' lines and the days' edges.
                      Positioned.fill(
                        child: CustomPaint(
                          painter: _GridLines(
                            columns: days.length,
                            column: column,
                            line: palette.hairline,
                          ),
                        ),
                      ),
                      for (var h = 1; h < 24; h++)
                        Positioned(
                          left: 0,
                          width: _gutter - Space.s,
                          top: h * _hour - 8,
                          child: ExcludeSemantics(
                            child: Text(
                              '${h.toString().padLeft(2, '0')}:00',
                              textAlign: TextAlign.right,
                              textScaler: TextScaler.noScaling,
                              style: TextStyles.caption.copyWith(
                                color: palette.textFaint,
                                fontSize: 11,
                                height: 16 / 11,
                              ),
                            ),
                          ),
                        ),
                      for (final (i, day) in days.indexed)
                        for (final piece in _pieces(day))
                          Positioned(
                            left:
                                _gutter +
                                i * column +
                                piece.column * (column / piece.columns) +
                                1,
                            width: math.max(1, column / piece.columns - 3),
                            top: piece.from * _hour + 1,
                            height: math.max(
                              _pill - 2,
                              (piece.to - piece.from) * _hour - 2,
                            ),
                            child: _EntryView(
                              entry: piece.entry,
                              now: now,
                              selected: widget.selected == _which(piece.entry),
                              scroll: _scroll,
                              top: piece.from * _hour + 1,
                              cut:
                                  piece.to >= 24 &&
                                  piece.entry
                                      .until(now)
                                      .isAfter(_addDays(day, 1)),
                              spoken: widget.spoken(piece.entry),
                              onTap: widget.onTap,
                            ),
                          ),
                      if (todayAt >= 0) ...[
                        Positioned(
                          left: 0,
                          width: _gutter - Space.s,
                          top: (now.hour + now.minute / 60) * _hour - 8,
                          child: Semantics(
                            label: 'Now, ${calendarClock(now)}',
                            excludeSemantics: true,
                            child: Text(
                              calendarClock(now),
                              textAlign: TextAlign.right,
                              textScaler: TextScaler.noScaling,
                              style: TextStyles.caption.copyWith(
                                color: palette.signal,
                                fontWeight: FontWeight.w600,
                                fontSize: 11,
                                height: 16 / 11,
                                backgroundColor: palette.canvas,
                              ),
                            ),
                          ),
                        ),
                        Positioned(
                          left: _gutter + todayAt * column - 4,
                          width: column + 4,
                          top: (now.hour + now.minute / 60) * _hour - 4,
                          height: 8,
                          child: IgnorePointer(
                            child: CustomPaint(
                              painter: _NowLine(palette.signal),
                            ),
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
              ),
            ),
          ],
        );
      },
    );
  }

  /// The all-day entries in lanes: each spans the days it covers, as few
  /// lanes as they fit in.
  ({int lanes, List<(CalendarEntry, int, int, int)> placed}) _band() {
    final days = widget.days;
    final first = days.first;
    final after = _addDays(days.last, 1);
    final lanes = <List<(int, int)>>[];
    final placed = <(CalendarEntry, int, int, int)>[];
    final all =
        [
          for (final e in widget.entries)
            if (e.allDay &&
                e.start.isBefore(after) &&
                (e.end ?? _addDays(e.start, 1)).isAfter(first))
              e,
        ]..sort((a, b) {
          final by = a.start.compareTo(b.start);
          if (by != 0) return by;
          return (b.end ?? b.start).compareTo(a.end ?? a.start);
        });
    for (final e in all) {
      final from = math.max(0, e.start.difference(first).inHours ~/ 24);
      final end = e.end ?? _addDays(e.start, 1);
      final to = math.min(
        days.length - 1,
        (end.difference(first).inHours / 24).ceil() - 1,
      );
      var lane = lanes.indexWhere(
        (taken) => taken.every((span) => span.$2 < from || span.$1 > to),
      );
      if (lane < 0) {
        lanes.add([]);
        lane = lanes.length - 1;
      }
      lanes[lane].add((from, to));
      placed.add((e, from, to, lane));
    }
    return (lanes: lanes.length, placed: placed);
  }

  Widget _bandView(
    BuildContext context,
    ({int lanes, List<(CalendarEntry, int, int, int)> placed}) band,
    double column,
  ) {
    final palette = Palette.of(context);
    final scaler = MediaQuery.textScalerOf(context);
    final lane = math.max(22.0, scaler.scale(16) + 6);
    return Container(
      decoration: BoxDecoration(
        border: Border(
          top: BorderSide(color: palette.hairline),
          bottom: BorderSide(color: palette.hairline),
        ),
      ),
      height: band.lanes * (lane + 2) + 4,
      child: Stack(
        children: [
          Positioned(
            left: 0,
            width: _gutter - Space.s,
            top: 4,
            child: Text(
              'all-day',
              textAlign: TextAlign.right,
              textScaler: TextScaler.noScaling,
              style: TextStyles.caption.copyWith(
                color: palette.textFaint,
                fontSize: 11,
              ),
            ),
          ),
          for (final (entry, from, to, at) in band.placed)
            Positioned(
              left: _gutter + from * column + 1,
              width: (to - from + 1) * column - 3,
              top: 2 + at * (lane + 2),
              height: lane,
              child: _EntryView(
                entry: entry,
                now: widget.now,
                title: _bandTitle(context, entry),
                selected: widget.selected == _which(entry),
                cut: false,
                spoken: widget.spoken(entry),
                onTap: widget.onTap,
              ),
            ),
        ],
      ),
    );
  }
}

/// A day's name over its column: "Thu 3", today's number in a signal disc.
class _DayName extends StatelessWidget {
  const _DayName({required this.day, required this.today});

  final DateTime day;
  final bool today;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final name = _short(_weekdays[day.weekday - 1]);
    return Semantics(
      label: calendarDate(day, AppCalendar.clock()),
      excludeSemantics: true,
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        spacing: Space.xs,
        children: [
          Text(
            today ? name : '$name ${day.day}',
            style: type.secondary.copyWith(
              color: today ? palette.text : palette.textMuted,
              fontWeight: today ? FontWeight.w600 : null,
            ),
          ),
          if (today)
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
              decoration: BoxDecoration(
                color: palette.signal,
                borderRadius: BorderRadius.circular(10),
              ),
              child: Text(
                '${day.day}',
                style: type.caption.copyWith(
                  color: const Color(0xFFFFFFFF),
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// An entry on the grid or the band: a block from its start to its end
/// with its time under its title, or a pill at its time; a past fire faint
/// and dashed with a tick; one running with a dashed foot.
class _EntryView extends StatelessWidget {
  const _EntryView({
    required this.entry,
    required this.now,
    required this.cut,
    required this.spoken,
    required this.onTap,
    this.title,
    this.selected = false,
    this.scroll,
    this.top = 0,
  });

  final CalendarEntry entry;
  final DateTime now;

  /// What it says; its own title when null.
  final String? title;

  /// Its details are open: a ring in the text's color around it.
  final bool selected;

  /// The grid's scroll and where the block starts in it: a block whose top
  /// is scrolled away keeps its title in sight at the top of what shows.
  final ScrollController? scroll;
  final double top;

  /// It goes on past the day's end.
  final bool cut;
  final String spoken;
  final void Function(BuildContext context, CalendarEntry entry) onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final color = _color(context, entry);
    final past = entry.kind == 'fire';
    final small = TextStyles.caption.copyWith(
      fontSize: 11,
      height: 14 / 11,
      color: past ? palette.textMuted : palette.text,
    );
    final bold = small.copyWith(fontWeight: FontWeight.w600);
    final words = title ?? entry.title;
    final pill = entry.point || entry.allDay;
    return Builder(
      builder: (context) => Semantics(
        button: true,
        label: spoken,
        excludeSemantics: true,
        onTap: () => onTap(context, entry),
        child: Pressable(
          onTap: () => onTap(context, entry),
          builder: (context, hovered) => DecoratedBox(
            position: DecorationPosition.foreground,
            decoration: BoxDecoration(
              border: selected
                  ? Border.all(color: palette.text, width: 2)
                  : null,
              borderRadius: BorderRadius.circular(pill ? 6 : 4),
            ),
            child: CustomPaint(
              foregroundPainter: past || entry.live
                  ? _Dashes(
                      color: past ? palette.textFaint : color,
                      whole: past,
                      radius: pill ? 6 : 4,
                    )
                  : null,
              child: Container(
                clipBehavior: Clip.hardEdge,
                decoration: BoxDecoration(
                  color: past
                      ? null
                      : color.withValues(alpha: hovered ? 0.32 : 0.22),
                  borderRadius: BorderRadius.circular(pill ? 6 : 4),
                  border: pill || past
                      ? null
                      : Border(left: BorderSide(color: color, width: 3)),
                ),
                padding: EdgeInsets.fromLTRB(pill ? 6 : 5, pill ? 0 : 3, 4, 0),
                alignment: pill ? Alignment.centerLeft : Alignment.topLeft,
                child: pill
                    ? Row(
                        spacing: 4,
                        children: [
                          _Glyph(
                            kind: entry.kind,
                            color: past ? palette.textFaint : color,
                            size: 11,
                          ),
                          Flexible(
                            child: Text.rich(
                              TextSpan(
                                children: [
                                  if (!entry.allDay)
                                    TextSpan(
                                      text: '${calendarClock(entry.start)} ',
                                      style: small,
                                    ),
                                  TextSpan(text: words, style: bold),
                                ],
                              ),
                              maxLines: 1,
                              softWrap: false,
                              overflow: TextOverflow.ellipsis,
                              textScaler: TextScaler.noScaling,
                            ),
                          ),
                        ],
                      )
                    : _Sticky(
                        scroll: scroll,
                        top: top,
                        child: CustomMultiChildLayout(
                          delegate: _WholeLines(),
                          children: [
                            LayoutId(
                              id: _Line.title,
                              child: Row(
                                spacing: 4,
                                children: [
                                  _Glyph(
                                    kind: entry.kind,
                                    color: color,
                                    size: 11,
                                  ),
                                  Flexible(
                                    child: Text(
                                      words,
                                      maxLines: 1,
                                      softWrap: false,
                                      overflow: TextOverflow.ellipsis,
                                      textScaler: TextScaler.noScaling,
                                      style: bold,
                                    ),
                                  ),
                                ],
                              ),
                            ),
                            LayoutId(
                              id: _Line.when,
                              child: Text(
                                [
                                  '${calendarClock(entry.start)}–'
                                      '${entry.live ? 'now' : calendarClock(entry.until(now))}',
                                  if (entry.agent.isNotEmpty)
                                    _agent(context, entry.agent).name,
                                ].join(' · '),
                                maxLines: 1,
                                softWrap: false,
                                overflow: TextOverflow.ellipsis,
                                textScaler: TextScaler.noScaling,
                                style: small.copyWith(color: palette.textMuted),
                              ),
                            ),
                          ],
                        ),
                      ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

enum _Line { title, when }

/// A block's title at its top, and when it is under it only if that line
/// fits whole: a short block never shows half a line. The title shows what
/// fits of it, cut at the block's foot.
class _WholeLines extends MultiChildLayoutDelegate {
  @override
  void performLayout(Size size) {
    final loose = BoxConstraints(maxWidth: size.width);
    final title = layoutChild(_Line.title, loose);
    positionChild(_Line.title, Offset.zero);
    final when = layoutChild(_Line.when, loose);
    final fits = title.height + when.height <= size.height;
    positionChild(_Line.when, Offset(0, fits ? title.height : size.height));
  }

  @override
  bool shouldRelayout(_WholeLines oldDelegate) => false;
}

/// A block's words, pushed down to stay at the top of what shows of it
/// while the grid's scroll hides its top.
class _Sticky extends StatelessWidget {
  const _Sticky({required this.scroll, required this.top, required this.child});

  final ScrollController? scroll;
  final double top;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final scroll = this.scroll;
    if (scroll == null) return child;
    return LayoutBuilder(
      builder: (context, constraints) => AnimatedBuilder(
        animation: scroll,
        builder: (context, child) {
          final hidden = scroll.hasClients ? scroll.offset - top : 0.0;
          final room = constraints.maxHeight.isFinite
              ? constraints.maxHeight - 16
              : 0.0;
          final push = hidden.clamp(0.0, math.max(0.0, room)).toDouble();
          return Transform.translate(offset: Offset(0, push), child: child);
        },
        child: child,
      ),
    );
  }
}

/// A past fire's dashed outline, or a running entry's dashed foot.
class _Dashes extends CustomPainter {
  _Dashes({required this.color, required this.whole, required this.radius});

  final Color color;
  final bool whole;
  final double radius;

  @override
  void paint(Canvas canvas, Size size) {
    final pen = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1;
    final path = whole
        ? (Path()..addRRect(
            RRect.fromRectAndRadius(
              (Offset.zero & size).deflate(0.5),
              Radius.circular(radius),
            ),
          ))
        : (Path()
            ..moveTo(0, size.height - 1)
            ..lineTo(size.width, size.height - 1));
    for (final metric in path.computeMetrics()) {
      for (var at = 0.0; at < metric.length; at += 7) {
        canvas.drawPath(
          metric.extractPath(at, math.min(at + 4, metric.length)),
          pen,
        );
      }
    }
  }

  @override
  bool shouldRepaint(_Dashes old) =>
      old.color != color || old.whole != whole || old.radius != radius;
}

/// The hours' hairlines across, and a hairline between days.
class _GridLines extends CustomPainter {
  _GridLines({required this.columns, required this.column, required this.line});

  final int columns;
  final double column;
  final Color line;

  @override
  void paint(Canvas canvas, Size size) {
    final pen = Paint()
      ..color = line
      ..strokeWidth = 1;
    for (var h = 1; h < 24; h++) {
      final y = h * _hour + 0.5;
      canvas.drawLine(Offset(_gutter, y), Offset(size.width, y), pen);
    }
    for (var i = 0; i <= columns; i++) {
      final x = _gutter + i * column - 0.5;
      canvas.drawLine(Offset(x, 0), Offset(x, 24 * _hour), pen);
    }
  }

  @override
  bool shouldRepaint(_GridLines old) =>
      old.columns != columns || old.column != column || old.line != line;
}

/// Now, across today: a dot at the day's edge and a line in signal.
class _NowLine extends CustomPainter {
  _NowLine(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final pen = Paint()..color = color;
    canvas.drawCircle(Offset(4, size.height / 2), 4, pen);
    canvas.drawRect(
      Rect.fromLTWH(4, size.height / 2 - 0.75, size.width - 4, 1.5),
      pen,
    );
  }

  @override
  bool shouldRepaint(_NowLine old) => old.color != color;
}
