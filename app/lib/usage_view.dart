import 'dart:async';
import 'dart:math' as math;

import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'identity.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'pane_chrome.dart' show InPane;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';
import 'mobile/haptics.dart';
import 'mobile/lines.dart' show ActionLine;
import 'usage.dart'
    show
        UsageCard,
        UsageMeter,
        UsageRing,
        accountState,
        formatAge,
        formatPercent,
        formatReset,
        headlineWindow,
        usageMeters;
import 'usage_resets.dart'
    show resetCount, resetCue, resetsLeft, useResetDialog;

/// How often the numbers are asked for again while they show.
const usageRefresh = Duration(seconds: 30);

/// The ranges the Usage page shows.
enum UsageRange {
  today('Today'),
  week('7 days'),
  month('30 days'),
  all('All time');

  const UsageRange(this.label);

  final String label;
}

/// What to ask the backend for [range], as of [now] on this device: from
/// local midnight today, or 7 or 30 local days back, or since the first
/// record; hour steps for today, day steps otherwise.
LoadUsage usageAsk(UsageRange range, DateTime now) {
  final midnight = DateTime(now.year, now.month, now.day);
  final since = switch (range) {
    UsageRange.today => midnight,
    UsageRange.week => midnight.subtract(const Duration(days: 6)),
    UsageRange.month => midnight.subtract(const Duration(days: 29)),
    UsageRange.all => null,
  };
  return LoadUsage(
    sinceMs: Int64(since?.millisecondsSinceEpoch ?? 0),
    untilMs: Int64(0),
    bucketMs: Int64(
      range == UsageRange.today
          ? const Duration(hours: 1).inMilliseconds
          : const Duration(days: 1).inMilliseconds,
    ),
    utcOffsetMinutes: now.timeZoneOffset.inMinutes,
  );
}

/// Every token a call used: what was sent, what came back, and what was
/// read from and written to the provider's cache.
int totalTokens(UsageTokens tokens) =>
    tokens.input.toInt() +
    tokens.output.toInt() +
    tokens.cacheRead.toInt() +
    tokens.cacheWrite.toInt();

UsageTokens _sum(Iterable<UsageTokens> all) {
  var input = 0, output = 0, read = 0, write = 0;
  for (final tokens in all) {
    input += tokens.input.toInt();
    output += tokens.output.toInt();
    read += tokens.cacheRead.toInt();
    write += tokens.cacheWrite.toInt();
  }
  return UsageTokens(
    input: Int64(input),
    output: Int64(output),
    cacheRead: Int64(read),
    cacheWrite: Int64(write),
  );
}

/// A count of tokens, short: "980", "12K", "1.2M", "3.4B".
String formatTokens(int count) {
  String short(double value, String unit) {
    if (value >= 100) return '${value.round()}$unit';
    final tenths = (value * 10).round() / 10;
    return tenths == tenths.roundToDouble()
        ? '${tenths.round()}$unit'
        : '$tenths$unit';
  }

  if (count < 1000) return '$count';
  if (count < 999500) return short(count / 1000, 'K');
  if (count < 999500000) return short(count / 1000000, 'M');
  return short(count / 1000000000, 'B');
}

/// [count] with thousands separated: "12,345".
String _grouped(int count) {
  final digits = '$count';
  final out = StringBuffer();
  for (var i = 0; i < digits.length; i++) {
    if (i > 0 && (digits.length - i) % 3 == 0) out.write(',');
    out.write(digits[i]);
  }
  return out.toString();
}

/// One line of a breakdown: who or what, its tokens, and what's under it.
class UsageLine {
  UsageLine({
    required this.key,
    required this.label,
    required this.tokens,
    this.requests = 0,
    this.detail = '',
  });

  final String key;
  final String label;
  final UsageTokens tokens;
  final int requests;

  /// What's under it, such as an agent's models.
  final String detail;

  int get total => totalTokens(tokens);
}

/// The report's rows summed by [keyOf], largest first; [labelOf] names a
/// group from its first row, [detailOf] says what's in it.
List<UsageLine> groupUsage(
  List<UsageRow> rows,
  String Function(UsageRow row) keyOf,
  String Function(UsageRow row) labelOf, {
  String Function(List<UsageRow> rows)? detailOf,
}) {
  final groups = <String, List<UsageRow>>{};
  for (final row in rows) {
    groups.putIfAbsent(keyOf(row), () => []).add(row);
  }
  final lines = [
    for (final MapEntry(:key, :value) in groups.entries)
      UsageLine(
        key: key,
        label: labelOf(value.first),
        tokens: _sum(value.map((row) => row.tokens)),
        requests: value.fold(0, (sum, row) => sum + row.requests.toInt()),
        detail: detailOf?.call(value) ?? '',
      ),
  ];
  lines.sort((a, b) => b.total.compareTo(a.total));
  return lines;
}

/// Usage: how many tokens went to which provider, model, account and
/// agent over a range, with a chart over time and each subscription's
/// limits. A page of its own on a phone, a pane beside the conversation on
/// a Mac. Asked for when it opens, when the range changes and every 30
/// seconds while it shows; nothing is kept.
class UsageView extends StatefulWidget {
  const UsageView({super.key});

  @override
  State<UsageView> createState() => _UsageViewState();
}

class _UsageViewState extends State<UsageView> {
  var _range = UsageRange.week;
  UsageReport? _report;
  String? _error;
  var _loading = false;
  Timer? _timer;
  var _asked = 0;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
    _timer = Timer.periodic(usageRefresh, (_) => unawaited(_load()));
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<void> _load() async {
    final asked = ++_asked;
    final range = _range;
    if (_report == null) _loading = true;
    try {
      final report = await AppScope.of(context)
          .loadUsage(usageAsk(range, DateTime.now()));
      if (!mounted || asked != _asked) return;
      setState(() {
        _report = report;
        _error = null;
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

  void _choose(UsageRange range) {
    if (range == _range) return;
    setState(() {
      _range = range;
      _report = null;
    });
    unawaited(_load());
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    void close() => AppScope.closePage(context);
    final inPane = InPane.of(context);
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: FocusScope(
        autofocus: !inPane,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (!inPane)
              isPhoneLayout(context)
                  ? PhoneHeader(
                      leading: HeaderButton.back(onTap: close),
                      title: Text(
                        'Usage',
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
                            Text(
                              'Usage',
                              style: TypeScale.of(context).label.copyWith(
                                color: palette.text,
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                            const Spacer(),
                            Button(label: 'Done', onPressed: close),
                          ],
                        ),
                      ),
                    ),
            Expanded(
              child: SelectableArea(
                child: ListenableBuilder(
                  listenable: AppScope.of(context).workspace,
                  builder: (context, _) => SingleChildScrollView(
                    padding: EdgeInsets.all(
                      isPhoneLayout(context) ? Space.l : Space.xl,
                    ),
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
    final backend = AppScope.of(context);
    final workspace = backend.workspace.workspace;
    final report = _report;
    final meters = [
      for (final meter in usageMeters(workspace.providers))
        if (meter.usage != null) meter,
    ];
    final reportPart = [
      if (report == null)
        Text(
          _error ?? (_loading ? 'Loading usage…' : ''),
          style: TypeScale.of(context).body.copyWith(
            color: _error == null ? palette.textFaint : palette.danger,
          ),
        )
      else
        ..._report_(context, palette, workspace, report),
      if (_error != null && report != null)
        Padding(
          padding: const EdgeInsets.only(top: Space.s),
          child: Text(
            _error!,
            style: TypeScale.of(context).caption
                .copyWith(color: palette.danger),
          ),
        ),
    ];
    // On a phone, what's left of each subscription first (what Usage is
    // opened for), its details a tap away; then the range, as a segmented
    // control, and the report.
    if (isPhoneLayout(context)) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (meters.isNotEmpty) ...[
            _PhoneLimits(meters: meters),
            const SizedBox(height: Space.xl),
          ],
          Unselectable(
            child: _RangeSegments(selected: _range, choose: _choose),
          ),
          const SizedBox(height: Space.l),
          ...reportPart,
        ],
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Unselectable(
          child: Wrap(
            spacing: Space.m,
            children: [
              for (final range in UsageRange.values)
                _RangeTab(
                  label: range.label,
                  selected: range == _range,
                  onTap: () => _choose(range),
                ),
            ],
          ),
        ),
        const SizedBox(height: Space.l),
        ...reportPart,
        if (meters.isNotEmpty) ...[
          const SizedBox(height: Space.xl),
          const _Heading('Subscription limits'),
          Wrap(
            spacing: Space.m,
            runSpacing: Space.m,
            children: [
              for (final meter in meters)
                Container(
                  constraints: const BoxConstraints(minWidth: 200),
                  padding: const EdgeInsets.all(Space.m),
                  decoration: BoxDecoration(
                    color: palette.sidebar,
                    border: Border.all(color: palette.hairline),
                    borderRadius: BorderRadius.circular(Radii.panel),
                  ),
                  child: UsageCard(meter: meter, now: DateTime.now()),
                ),
            ],
          ),
        ],
      ],
    );
  }

  List<Widget> _report_(
    BuildContext context,
    Palette palette,
    Workspace workspace,
    UsageReport report,
  ) {
    final rows = report.rows;
    final total = _sum(rows.map((row) => row.tokens));
    final requests = rows.fold(0, (sum, row) => sum + row.requests.toInt());
    if (rows.isEmpty) {
      return [
        Text(
          _range == UsageRange.today
              ? 'No usage yet today.'
              : 'No usage in this period.',
          style: TypeScale.of(context).body.copyWith(color: palette.textFaint),
        ),
      ];
    }
    final providers = {
      for (final provider in workspace.providers) provider.id: provider.name,
    };
    final models = {for (final model in workspace.models) model.id: model.name};
    final agents = {for (final agent in workspace.agents) agent.id: agent};
    String providerName(String id) => providers[id] ?? id;
    String modelName(UsageRow row) =>
        models['${row.providerId}/${row.model}'] ?? row.model;
    String agentName(UsageRow row) => row.agentId.isEmpty
        ? 'Earlier'
        : agents[row.agentId]?.name ??
              (row.agentName.isEmpty ? 'Deleted agent' : row.agentName);
    String models_(List<UsageRow> rows) => groupUsage(
      rows,
      (row) => '${row.providerId}/${row.model}',
      modelName,
    ).map((line) => '${line.label} ${formatTokens(line.total)}').join(' · ');

    final byProvider = groupUsage(
      rows,
      (row) => row.providerId,
      (row) => providerName(row.providerId),
    );
    final byModel = groupUsage(
      rows,
      (row) => '${row.providerId}/${row.model}',
      modelName,
      detailOf: (rows) => providerName(rows.first.providerId),
    );
    final byAgent = groupUsage(
      rows,
      (row) => row.agentId,
      agentName,
      detailOf: models_,
    );
    final accounts = rows.any((row) => row.accountId.isNotEmpty);
    final byAccount = !accounts
        ? const <UsageLine>[]
        : groupUsage(
            rows,
            (row) => '${row.providerId}/${row.accountId}',
            (row) => row.accountId.isEmpty
                ? '${providerName(row.providerId)}, unknown account'
                : row.accountLabel.isEmpty
                ? '${providerName(row.providerId)}, removed account'
                : row.accountLabel,
            detailOf: (rows) => providerName(rows.first.providerId),
          );
    final all = totalTokens(total);
    final agentsSince = report.agentsSinceMs.toInt();
    return [
      Text.rich(
        TextSpan(
          children: [
            TextSpan(
              text: formatTokens(all),
              style: TypeScale.of(context).title.copyWith(
                color: palette.text,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
            TextSpan(
              text: '  tokens',
              style: TypeScale.of(context).body
                  .copyWith(color: palette.textMuted),
            ),
          ],
        ),
      ),
      const SizedBox(height: Space.xs),
      Text(
        [
          'Input ${formatTokens(total.input.toInt())}',
          'Output ${formatTokens(total.output.toInt())}',
          'Reused ${formatTokens(total.cacheRead.toInt())}',
          'Kept ready ${formatTokens(total.cacheWrite.toInt())}',
          '${_grouped(requests)} ${requests == 1 ? 'request' : 'requests'}',
        ].join(' · '),
        style: TypeScale.of(context).caption.copyWith(
          color: palette.textMuted,
          fontFeatures: const [FontFeature.tabularFigures()],
        ),
      ),
      if (report.series.isNotEmpty) ...[
        const SizedBox(height: Space.l),
        UsageChart(series: report.series, hourly: _range == UsageRange.today),
      ],
      const SizedBox(height: Space.xl),
      const _Heading('By agent'),
      for (final line in byAgent)
        _LineRow(
          line: line,
          of: all,
          color: line.key.isEmpty
              ? palette.textFaint
              : agentColor(agents[line.key]?.color ?? 'slate'),
          leading: agents[line.key] == null
              ? null
              : AgentCharacter(agent: agents[line.key]!, size: 16),
        ),
      const SizedBox(height: Space.xl),
      const _Heading('By provider'),
      for (final line in byProvider)
        _LineRow(line: line, of: all, color: providerColor(line.key, palette)),
      const SizedBox(height: Space.xl),
      const _Heading('By model'),
      for (final line in byModel)
        _LineRow(
          line: line,
          of: all,
          color: providerColor(line.key.split('/').first, palette),
        ),
      if (byAccount.isNotEmpty) ...[
        const SizedBox(height: Space.xl),
        const _Heading('By account'),
        for (final line in byAccount)
          _LineRow(
            line: line,
            of: all,
            color: providerColor(line.key.split('/').first, palette),
          ),
      ],
      if (rows.any((row) => row.agentId.isEmpty) && agentsSince > 0) ...[
        const SizedBox(height: Space.m),
        Text(
          'Claktory has counted usage by agent since '
          '${_day(DateTime.fromMillisecondsSinceEpoch(agentsSince))}. '
          'Anything before that shows as Earlier.',
          style: TypeScale.of(context).caption
              .copyWith(color: palette.textFaint),
        ),
      ],
    ];
  }
}

const _months = [
  'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
  'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
];
const _weekdays = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

String _day(DateTime time) => '${_months[time.month - 1]} ${time.day}';

String _hour(DateTime time) => '${time.hour.toString().padLeft(2, '0')}:00';

/// A section's heading, in caption weight 500, faint.
class _Heading extends StatelessWidget {
  const _Heading(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: Space.s),
    child: Text(
      text,
      style: TypeScale.of(context).caption.copyWith(
        color: Palette.of(context).textFaint,
        fontWeight: FontWeight.w500,
      ),
    ),
  );
}

/// A range to show: its name in body, `text` and underlined in `signal`
/// when chosen, as the Marketplace's tabs.
class _RangeTab extends StatelessWidget {
  const _RangeTab({
    required this.label,
    required this.selected,
    required this.onTap,
  });

  final String label;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      selected: selected,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          // A finger's height on a phone, growing with the text there.
          height: isPhoneLayout(context) ? null : 28,
          constraints: isPhoneLayout(context)
              ? const BoxConstraints(minHeight: Touch.target)
              : null,
          padding: const EdgeInsets.symmetric(horizontal: Space.xs),
          decoration: BoxDecoration(
            border: Border(
              bottom: BorderSide(
                width: 2,
                color: selected ? palette.signal : const Color(0x00000000),
              ),
            ),
          ),
          // As wide as its word: the tabs sit in a row (a Container
          // aligning its child would take the whole line).
          child: Align(
            widthFactor: 1,
            child: Text(
              label,
              style: TypeScale.of(context).body.copyWith(
                color: selected || hovered ? palette.text : palette.textMuted,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// One line of a breakdown: a dot or the agent's character, the name, its
/// share as a thin bar in its color, and its tokens with the share.
class _LineRow extends StatelessWidget {
  const _LineRow({
    required this.line,
    required this.of,
    required this.color,
    this.leading,
  });

  final UsageLine line;
  final int of;
  final Color color;
  final Widget? leading;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final share = of == 0 ? 0.0 : line.total / of;
    final percent = share * 100;
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.s),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              SizedBox(
                width: 16,
                height: 16,
                child: Center(child: leading ?? Dot(color: color)),
              ),
              const SizedBox(width: Space.s),
              Expanded(
                child: Text(
                  line.label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.text),
                ),
              ),
              const SizedBox(width: Space.s),
              Text(
                formatTokens(line.total),
                style: TypeScale.of(context).body.copyWith(
                  color: palette.text,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
              SizedBox(
                width: 48,
                child: Text(
                  percent >= 1 || percent == 0 ? '${percent.round()}%' : '<1%',
                  textAlign: TextAlign.right,
                  style: TypeScale.of(context).caption.copyWith(
                    color: palette.textFaint,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.xs),
          Padding(
            padding: const EdgeInsets.only(left: 16 + Space.s),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(1),
              child: SizedBox(
                height: 2,
                child: Stack(
                  fit: StackFit.expand,
                  children: [
                    ColoredBox(color: palette.selected),
                    FractionallySizedBox(
                      alignment: Alignment.centerLeft,
                      widthFactor: share.clamp(0, 1),
                      child: ColoredBox(color: color),
                    ),
                  ],
                ),
              ),
            ),
          ),
          if (line.detail.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(left: 16 + Space.s, top: 2),
              child: Text(
                line.detail,
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textMuted),
              ),
            ),
        ],
      ),
    );
  }
}

/// Tokens over time: a bar per hour or day, in `signal`, 72px tall at
/// most, with the hovered (or touched) one's time and tokens above it.
class UsageChart extends StatefulWidget {
  const UsageChart({super.key, required this.series, required this.hourly});

  final List<UsageBucket> series;

  /// Each bar is an hour (today) rather than a day.
  final bool hourly;

  @override
  State<UsageChart> createState() => _UsageChartState();
}

class _UsageChartState extends State<UsageChart> {
  int? _hovered;

  void _point(Offset at, double width) {
    final count = widget.series.length;
    final index = (at.dx / width * count).floor().clamp(0, count - 1);
    if (index != _hovered) setState(() => _hovered = index);
  }

  String _when(UsageBucket bucket) {
    final time = DateTime.fromMillisecondsSinceEpoch(bucket.startMs.toInt());
    return widget.hourly
        ? _hour(time)
        : '${_weekdays[time.weekday - 1]} ${_day(time)}';
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final series = widget.series;
    final hovered = _hovered;
    final caption = hovered == null
        ? '${_when(series.first)} – ${_when(series.last)}'
        : '${_when(series[hovered])} · '
              '${formatTokens(totalTokens(series[hovered].tokens))} tokens';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          caption,
          style: TypeScale.of(context).caption.copyWith(
            color: hovered == null ? palette.textFaint : palette.textMuted,
            fontFeatures: const [FontFeature.tabularFigures()],
          ),
        ),
        const SizedBox(height: Space.xs),
        LayoutBuilder(
          builder: (context, constraints) => MouseRegion(
            onHover: (event) =>
                _point(event.localPosition, constraints.maxWidth),
            onExit: (_) => setState(() => _hovered = null),
            child: GestureDetector(
              onPanDown: (details) =>
                  _point(details.localPosition, constraints.maxWidth),
              onPanUpdate: (details) =>
                  _point(details.localPosition, constraints.maxWidth),
              onPanEnd: (_) => setState(() => _hovered = null),
              child: CustomPaint(
                size: Size(constraints.maxWidth, 72),
                painter: _ChartPainter(
                  values: [for (final b in series) totalTokens(b.tokens)],
                  hovered: hovered,
                  bar: palette.signal,
                  track: palette.hairline,
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

class _ChartPainter extends CustomPainter {
  _ChartPainter({
    required this.values,
    required this.hovered,
    required this.bar,
    required this.track,
  });

  final List<int> values;
  final int? hovered;
  final Color bar;
  final Color track;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawLine(
      Offset(0, size.height - 0.5),
      Offset(size.width, size.height - 0.5),
      Paint()..color = track,
    );
    final most = values.fold(0, math.max);
    if (most == 0 || values.isEmpty) return;
    final step = size.width / values.length;
    final gap = step > 6 ? 2.0 : (step > 3 ? 1.0 : 0.0);
    for (var i = 0; i < values.length; i++) {
      if (values[i] == 0) continue;
      final height = math.max(2.0, values[i] / most * (size.height - 1));
      final paint = Paint()
        ..color = hovered == null || hovered == i
            ? bar
            : bar.withValues(alpha: 0.45);
      canvas.drawRRect(
        RRect.fromRectAndCorners(
          Rect.fromLTWH(
            i * step + gap / 2,
            size.height - 1 - height,
            step - gap,
            height,
          ),
          topLeft: const Radius.circular(1.5),
          topRight: const Radius.circular(1.5),
        ),
        paint,
      );
    }
  }

  @override
  bool shouldRepaint(_ChartPainter old) =>
      old.hovered != hovered ||
      old.bar != bar ||
      old.values.length != values.length ||
      !_same(old.values, values);

  static bool _same(List<int> a, List<int> b) {
    for (var i = 0; i < a.length; i++) {
      if (a[i] != b[i]) return false;
    }
    return true;
  }
}

/// The ranges on a phone, as iOS's segmented control: a 44pt track in
/// `hover` with the four in equal parts, their words in 15pt weight 500,
/// the chosen one raised on the canvas's color with the floating shadow,
/// in weight 600. Where the words won't fit four across (large text
/// sizes), two rows of two.
class _RangeSegments extends StatelessWidget {
  const _RangeSegments({required this.selected, required this.choose});

  final UsageRange selected;
  final void Function(UsageRange range) choose;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    // Words a notch smaller than a button's, as iOS's segmented control
    // has them, so the four fit across an iPhone's column.
    final rest = type.secondary.copyWith(fontWeight: FontWeight.w500);
    final style = type.secondary.copyWith(fontWeight: FontWeight.w600);
    final scaler = MediaQuery.textScalerOf(context);
    return LayoutBuilder(
      builder: (context, constraints) {
        // The widest word, with room either side, against a quarter.
        var widest = 0.0;
        for (final range in UsageRange.values) {
          final painter = TextPainter(
            text: TextSpan(text: range.label, style: style),
            textDirection: TextDirection.ltr,
            textScaler: scaler,
            maxLines: 1,
          )..layout();
          widest = math.max(widest, painter.width);
          painter.dispose();
        }
        const inset = 2.0;
        final across =
            constraints.maxWidth / UsageRange.values.length >=
                widest + Space.s * 2 + inset * 2
            ? UsageRange.values.length
            : 2;
        Widget segment(UsageRange range) {
          final chosen = range == selected;
          return Expanded(
            child: Semantics(
              button: true,
              selected: chosen,
              inMutuallyExclusiveGroup: true,
              label: range.label,
              onTap: () => _tap(range),
              excludeSemantics: true,
              child: Pressable(
                onTap: () => _tap(range),
                // The track's inset is inside each segment's target, so
                // every one is a finger's 44pt tall.
                builder: (context, pressed) => Padding(
                  padding: const EdgeInsets.all(inset),
                  child: AnimatedContainer(
                    duration: Motion.fast,
                    curve: Curves.easeOut,
                    constraints: const BoxConstraints(
                      minHeight: Touch.target - inset * 2,
                    ),
                    padding: const EdgeInsets.symmetric(
                      horizontal: Space.xs,
                      vertical: Space.xs,
                    ),
                    decoration: BoxDecoration(
                      color: chosen ? palette.canvas : const Color(0x00000000),
                      borderRadius: BorderRadius.circular(Radii.panel - inset),
                      boxShadow: chosen ? floatingShadow : null,
                    ),
                    alignment: Alignment.center,
                    child: Text(
                      range.label,
                      textAlign: TextAlign.center,
                      style: (chosen ? style : rest).copyWith(
                        color: chosen || pressed
                            ? palette.text
                            : palette.textMuted,
                      ),
                    ),
                  ),
                ),
              ),
            ),
          );
        }

        final rows = [
          for (var i = 0; i < UsageRange.values.length; i += across)
            UsageRange.values.sublist(i, i + across),
        ];
        return Container(
          decoration: BoxDecoration(
            color: palette.hover,
            borderRadius: BorderRadius.circular(Radii.panel),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final row in rows)
                IntrinsicHeight(
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [for (final range in row) segment(range)],
                  ),
                ),
            ],
          ),
        );
      },
    );
  }

  void _tap(UsageRange range) {
    if (range == selected) return;
    unawaited(Haptics.selection());
    choose(range);
  }
}

/// A phone's subscription limits: a group per provider under its name,
/// each account a row with its ring, who it is and its limits at a glance;
/// a tap opens the rest under it (every limit with when it resets, its
/// state, its usage resets with Use…) and another folds it away. Which
/// rows are open lasts while the page shows.
class _PhoneLimits extends StatefulWidget {
  const _PhoneLimits({required this.meters});

  final List<UsageMeter> meters;

  @override
  State<_PhoneLimits> createState() => _PhoneLimitsState();
}

class _PhoneLimitsState extends State<_PhoneLimits> {
  final _open = <String>{};

  static String _key(UsageMeter meter) =>
      '${meter.provider.id}/${meter.account?.id ?? ''}';

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final providers = <String, List<UsageMeter>>{};
    for (final meter in widget.meters) {
      (providers[meter.provider.id] ??= []).add(meter);
    }
    final now = DateTime.now();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final (index, meters) in providers.values.indexed) ...[
          if (index > 0) const SizedBox(height: Space.l),
          _Heading(meters.first.provider.name),
          Container(
            clipBehavior: Clip.antiAlias,
            decoration: BoxDecoration(
              color: palette.sidebar,
              border: Border.all(color: palette.hairline),
              borderRadius: BorderRadius.circular(Radii.panel),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (final (row, meter) in meters.indexed) ...[
                  if (row > 0)
                    Container(
                      height: 1,
                      margin: const EdgeInsetsDirectional.only(
                        start: Space.m + _ringCell + Space.m,
                      ),
                      color: palette.hairline,
                    ),
                  _LimitRow(
                    meter: meter,
                    now: now,
                    open: _open.contains(_key(meter)),
                    toggle: () => setState(() {
                      if (!_open.remove(_key(meter))) _open.add(_key(meter));
                    }),
                  ),
                ],
              ],
            ),
          ),
        ],
      ],
    );
  }
}

/// A limit row's ring, and the cell it sits in.
const _ringSize = 28.0;
const _ringCell = 32.0;

/// [usage]'s limits, the weekly one first.
List<UsageWindow> _windowsOf(ProviderUsage? usage) => usage == null
    ? const []
    : [
        ...usage.windows.where((w) => w.id == headlineWindow),
        ...usage.windows.where((w) => w.id != headlineWindow),
      ];

class _LimitRow extends StatelessWidget {
  const _LimitRow({
    required this.meter,
    required this.now,
    required this.open,
    required this.toggle,
  });

  final UsageMeter meter;
  final DateTime now;
  final bool open;
  final VoidCallback toggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final usage = meter.usage;
    final account = meter.account;
    final windows = _windowsOf(usage);
    final state = account == null
        ? null
        : accountState(meter.provider, account, now);
    // At a glance: the first two limits' shares used, or why there are
    // none; a state to act on (a limit reached, out of credit) instead.
    final String glance;
    var problem = false;
    final limited =
        account != null &&
        account.exhaustedUntil.toInt() > now.millisecondsSinceEpoch;
    if (state != null && (state.problem || limited)) {
      glance = state.text;
      problem = state.problem;
    } else if (usage != null && usage.error.isNotEmpty && windows.isEmpty) {
      glance = "Couldn't check usage";
      problem = true;
    } else if (windows.isEmpty) {
      glance = usage != null && usage.updated > 0
          ? 'No limits reported.'
          : 'Usage not checked yet.';
    } else {
      // "Used:" settles what the numbers are; the labels follow it in
      // lowercase ("Used: weekly 92% · 5-hour 34%").
      String lower(String label) =>
          label.isEmpty ? label : label[0].toLowerCase() + label.substring(1);
      final used = [
        for (final window in windows.take(2))
          '${lower(window.label)} ${formatPercent(window.usedPercent)}',
      ];
      glance = 'Used: ${used.join(' · ')}';
    }
    final resets = resetsLeft(meter).fold(0, (sum, r) => sum + r.left);
    final title = meter.who.isNotEmpty
        ? meter.who
        : meter.plan.isNotEmpty
        ? meter.plan
        : meter.provider.name;
    final row = Semantics(
      button: true,
      expanded: open,
      onTap: toggle,
      child: Pressable(
        onTap: toggle,
        builder: (context, pressed) => Container(
          constraints: const BoxConstraints(minHeight: Touch.row),
          color: pressed ? palette.hover : null,
          padding: const EdgeInsets.symmetric(
            horizontal: Space.m,
            vertical: Space.s,
          ),
          child: Row(
            children: [
              ExcludeSemantics(
                child: UsageRing(
                  meter: meter,
                  size: _ringSize,
                  cell: _ringCell,
                ),
              ),
              const SizedBox(width: Space.m),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(title, style: type.body.copyWith(color: palette.text)),
                    Text(
                      glance,
                      style: type.secondary.copyWith(
                        color: problem ? palette.danger : palette.textMuted,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
                    if (resets > 0)
                      Text(
                        resets == 1
                            ? '1 usage reset left'
                            : '$resets usage resets left',
                        style: type.secondary.copyWith(
                          color: resetCue(meter)
                              ? palette.signal
                              : palette.textMuted,
                        ),
                      ),
                  ],
                ),
              ),
              const SizedBox(width: Space.s),
              ExcludeSemantics(
                child: AnimatedRotation(
                  turns: open ? 0.5 : 0,
                  duration: Motion.fast,
                  child: Chevron(color: palette.textFaint, down: true),
                ),
              ),
            ],
          ),
        ),
      ),
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        row,
        AnimatedSize(
          duration: Motion.fast,
          curve: Curves.easeOut,
          alignment: Alignment.topCenter,
          child: open
              ? _LimitDetail(meter: meter, now: now, windows: windows)
              : const SizedBox(width: double.infinity),
        ),
      ],
    );
  }
}

/// An open limit row: the plan and the account's state, each limit with a
/// bar of its share and when it resets, when it was checked, then its
/// usage resets with Use…, lined up under the row's words.
class _LimitDetail extends StatelessWidget {
  const _LimitDetail({
    required this.meter,
    required this.now,
    required this.windows,
  });

  final UsageMeter meter;
  final DateTime now;
  final List<UsageWindow> windows;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final muted = type.secondary.copyWith(color: palette.textMuted);
    final faint = type.caption.copyWith(color: palette.textFaint);
    final usage = meter.usage;
    final account = meter.account;
    final state = account == null
        ? null
        : accountState(meter.provider, account, now);
    final color = providerColor(meter.provider.id, palette);
    final read = usage != null && usage.updated > 0;
    final about = [
      if (meter.who.isNotEmpty && meter.plan.isNotEmpty) meter.plan,
      if (account == null) 'Signed in with ${meter.provider.signedInWith}',
      if (state != null && state.text.isNotEmpty && !state.problem) state.text,
    ].join(' · ');
    final resets = resetsLeft(meter);
    return Padding(
      padding: const EdgeInsetsDirectional.fromSTEB(
        Space.m + _ringCell + Space.m,
        0,
        Space.m,
        Space.m,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (about.isNotEmpty) Text(about, style: muted),
          if (state != null && state.problem)
            Text(state.text, style: muted.copyWith(color: palette.danger)),
          for (final window in windows)
            Padding(
              padding: const EdgeInsets.only(top: Space.m),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.end,
                    children: [
                      Expanded(
                        child: Text(
                          window.label,
                          style: type.body.copyWith(color: palette.text),
                        ),
                      ),
                      Text(
                        formatPercent(window.usedPercent),
                        style: type.body.copyWith(
                          color: palette.text,
                          fontFeatures: const [FontFeature.tabularFigures()],
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: Space.xs),
                  _Share(
                    of: window.usedPercent.clamp(0, 100) / 100,
                    color: color,
                  ),
                  const SizedBox(height: Space.xs),
                  Text(formatReset(window.resetsAt.toInt(), now), style: muted),
                ],
              ),
            ),
          if (usage != null && usage.error.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: Space.m),
              child: Text(
                "Couldn't check usage: ${usage.error}",
                style: muted.copyWith(color: palette.danger),
              ),
            ),
          if (read)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: Text(
                '${usage.error.isEmpty ? 'Checked' : 'Last checked'} '
                '${formatAge(usage.updated.toInt(), now)}',
                style: faint,
              ),
            ),
          if (resets.isNotEmpty) ...[
            const SizedBox(height: Space.l),
            Text(
              'Usage resets',
              style: type.emphasis.copyWith(color: palette.text),
            ),
            for (final reset in resets)
              Padding(
                padding: const EdgeInsets.only(top: Space.s),
                child: ActionLine(
                  lead: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        reset.label,
                        style: type.body.copyWith(color: palette.text),
                      ),
                      Text(resetCount(reset, now), style: muted),
                      if (!reset.usable && reset.unavailable.isNotEmpty)
                        Text(reset.unavailable, style: faint),
                    ],
                  ),
                  action: Button(
                    label: 'Use…',
                    onPressed: () => unawaited(
                      useResetDialog(context, meter: meter, reset: reset),
                    ),
                  ),
                ),
              ),
          ],
        ],
      ),
    );
  }
}

/// A limit's share used: a 4pt bar in the provider's color over
/// `selected`, rounded.
class _Share extends StatelessWidget {
  const _Share({required this.of, required this.color});

  final double of;
  final Color color;

  @override
  Widget build(BuildContext context) => ClipRRect(
    borderRadius: BorderRadius.circular(2),
    child: SizedBox(
      height: 4,
      child: Stack(
        fit: StackFit.expand,
        children: [
          ColoredBox(color: Palette.of(context).selected),
          FractionallySizedBox(
            alignment: AlignmentDirectional.centerStart,
            widthFactor: of,
            child: ColoredBox(color: color),
          ),
        ],
      ),
    ),
  );
}
