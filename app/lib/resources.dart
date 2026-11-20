import 'dart:math' as math;

import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';

import 'characters/character.dart';
import 'backend.dart';
import 'controls.dart';
import 'explorer.dart' show shortPath;
import 'extensions.dart' show logTime;
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart';
import 'tell_rivet.dart';

/// How the machine and Factory use its resources, read-only: the alerts in
/// effect, the machine's gauges (processors, memory, swap, each volume's free
/// space), the last hour of memory and processors, Factory's processes and
/// the machine's biggest others, and what Factory's data and the projects
/// take on disk.
class ResourcesView extends StatelessWidget {
  const ResourcesView({super.key, required this.resources});

  final Resources resources;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final store = AppScope.of(context).workspace;
    final caption = TextStyles.caption.copyWith(
      color: palette.textFaint,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        final settings = store.workspace.settings;
        final machine = resources.machine;
        final measured = resources.measured.toInt();
        return ListView(
          padding: const EdgeInsets.fromLTRB(
            Space.s,
            Space.s,
            Space.s,
            Space.l,
          ),
          children: [
            for (final alert in resources.alerts) _Alert(alert),
            if (resources.sampled > 0)
              Padding(
                padding: const EdgeInsets.fromLTRB(
                  Space.s,
                  Space.xs,
                  Space.s,
                  0,
                ),
                child: Text(
                  'Updated at ${_at(resources.sampled)}, every minute',
                  style: caption,
                ),
              ),
            const _Heading('Machine'),
            _Gauge(
              label: 'Processors',
              fraction: machine.cpuPercent / 100,
              value:
                  '${_percent(machine.cpuPercent)} of '
                  '${machine.cores == 1 ? '1 core' : '${machine.cores} cores'}',
              high:
                  settings.cpuPercent > 0 &&
                  machine.cpuPercent > settings.cpuPercent,
            ),
            _Gauge(
              label: 'Memory',
              fraction: _ratio(machine.memoryUsed, machine.memoryTotal),
              value:
                  '${formatBytes(machine.memoryUsed)} of '
                  '${formatBytes(machine.memoryTotal)}',
              high:
                  settings.ramPercent > 0 &&
                  _ratio(machine.memoryUsed, machine.memoryTotal) * 100 >
                      settings.ramPercent,
            ),
            _Gauge(
              label: 'Swap',
              fraction: _ratio(machine.swapUsed, machine.swapTotal),
              value: machine.swapTotal == Int64.ZERO
                  ? 'None'
                  : '${formatBytes(machine.swapUsed)} of '
                        '${formatBytes(machine.swapTotal)}',
            ),
            for (final volume in machine.volumes)
              _Gauge(
                label: volume.mount,
                fraction: 1 - _ratio(volume.available, volume.total),
                value:
                    '${formatBytes(volume.available)} free of '
                    '${formatBytes(volume.total)}',
                note: volume.uses.join(', '),
                high: _lowOnSpace(volume, settings),
              ),
            if (resources.history.length > 1) ...[
              const _Heading('Last hour'),
              _History(
                label: 'Memory',
                samples: [
                  for (final sample in resources.history)
                    _ratio(sample.memoryUsed, machine.memoryTotal),
                ],
                now: formatBytes(resources.history.last.memoryUsed),
              ),
              _History(
                label: 'Processors',
                samples: [
                  for (final sample in resources.history)
                    sample.cpuPercent / 100,
                ],
                now: _percent(resources.history.last.cpuPercent),
              ),
            ],
            const _Heading('Claktory'),
            if (resources.factory.isEmpty)
              const _Empty('Not sampled yet.')
            else
              for (final process in resources.factory)
                _ProcessRow(process: process, store: store),
            if (resources.others.isNotEmpty) ...[
              const _Heading('Other processes'),
              for (final process in resources.others)
                _ProcessRow(process: process, store: store),
            ],
            _Heading(
              'Disk',
              trailing: measured > 0
                  ? 'measured at ${_at(Int64(measured))}'
                  : null,
            ),
            if (measured == 0)
              const _Empty(
                'Measuring… This takes a minute after Claktory starts.',
              )
            else
              for (final usage in resources.disk)
                _DiskRow(
                  key: ValueKey('${usage.kind}/${usage.id}/${usage.path}'),
                  usage: usage,
                  store: store,
                ),
          ],
        );
      },
    );
  }
}

/// Whether [volume] has less free than the settings allow: under
/// disk_free_gb GB or disk_free_percent of it, whichever is smaller.
bool _lowOnSpace(Volume volume, Settings settings) {
  if (volume.total == Int64.ZERO) return false;
  final total = volume.total.toDouble();
  final byGb = settings.diskFreeGb * 1e9;
  final byPercent = total * settings.diskFreePercent / 100;
  final floor = math.min(byGb, byPercent);
  return floor > 0 && volume.available.toDouble() < floor;
}

double _ratio(Int64 part, Int64 whole) =>
    whole <= Int64.ZERO ? 0 : (part.toDouble() / whole.toDouble()).clamp(0, 1);

String _percent(double value) => '${value.round()}%';

String _at(Int64 ms) =>
    logTime(DateTime.fromMillisecondsSinceEpoch(ms.toInt()));

/// Bytes as people read them, in powers of 1000 like the Finder, with a
/// decimal under 100: "830 KB", "12.4 GB", "212 GB", "1.2 TB".
String formatBytes(Int64 bytes) {
  const units = ['bytes', 'KB', 'MB', 'GB', 'TB', 'PB'];
  var value = bytes.toDouble();
  var unit = 0;
  // 999.6 MB reads as 1.0 GB, not 1000 MB.
  while (value >= 999.5 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  if (unit == 0) {
    return bytes == Int64.ONE ? '1 byte' : '${bytes.toInt()} bytes';
  }
  final digits = value < 100 ? 1 : 0;
  return '${value.toStringAsFixed(digits)} ${units[unit]}';
}

class _Heading extends StatelessWidget {
  const _Heading(this.text, {this.trailing});

  final String text;

  /// Said at the right, faint.
  final String? trailing;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final style = TextStyles.caption.copyWith(
      color: palette.textFaint,
      fontWeight: FontWeight.w500,
    );
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.m, Space.s, Space.xs),
      child: Row(
        children: [
          Expanded(child: Text(text, style: style)),
          if (trailing case final trailing?)
            Text(
              trailing,
              style: TextStyles.caption.copyWith(
                color: palette.textFaint,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
        ],
      ),
    );
  }
}

class _Empty extends StatelessWidget {
  const _Empty(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(
      horizontal: Space.s,
      vertical: Space.xs,
    ),
    child: Text(
      text,
      style: TextStyles.body.copyWith(color: Palette.of(context).textFaint),
    ),
  );
}

/// An alert in effect: a danger dot and what the agents were told.
class _Alert extends StatelessWidget {
  const _Alert(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: Space.s, vertical: 2),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: Space.l,
            height: 18,
            child: Align(
              alignment: Alignment.centerLeft,
              child: Dot(color: palette.danger),
            ),
          ),
          Expanded(
            child: Text(
              text,
              style: TextStyles.body.copyWith(color: palette.danger),
            ),
          ),
        ],
      ),
    );
  }
}

/// A 4px meter, like the context meter: [fraction] filled in `textMuted`,
/// or `danger` when [high].
class _Meter extends StatelessWidget {
  const _Meter({required this.fraction, this.high = false});

  final double fraction;
  final bool high;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      height: 4,
      alignment: Alignment.centerLeft,
      decoration: BoxDecoration(
        color: palette.selected,
        borderRadius: BorderRadius.circular(2),
      ),
      child: FractionallySizedBox(
        widthFactor: fraction.clamp(0.0, 1.0),
        heightFactor: 1,
        child: DecoratedBox(
          decoration: BoxDecoration(
            color: high ? palette.danger : palette.textMuted,
            borderRadius: BorderRadius.circular(2),
          ),
        ),
      ),
    );
  }
}

/// One of the machine's gauges: its name and how much is used, over a
/// meter; [note] under it, faint.
class _Gauge extends StatelessWidget {
  const _Gauge({
    required this.label,
    required this.fraction,
    required this.value,
    this.note = '',
    this.high = false,
  });

  final String label;
  final double fraction;
  final String value;
  final String note;

  /// Past its alert's threshold.
  final bool high;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: Space.xs,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyles.body.copyWith(color: palette.text),
                ),
              ),
              const SizedBox(width: Space.s),
              Text(
                value,
                style: TextStyles.caption.copyWith(
                  color: high ? palette.danger : palette.textMuted,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.xs),
          _Meter(fraction: fraction, high: high),
          if (note.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(
                note,
                style: TextStyles.caption.copyWith(color: palette.textFaint),
              ),
            ),
        ],
      ),
    );
  }
}

/// The last hour of one measure as a 24px sparkline, its name at the left
/// and its latest value at the right.
class _History extends StatelessWidget {
  const _History({
    required this.label,
    required this.samples,
    required this.now,
  });

  final String label;

  /// Oldest first, each 0 to 1.
  final List<double> samples;
  final String now;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: Space.xs,
      ),
      child: Row(
        children: [
          SizedBox(
            width: 80,
            child: Text(
              label,
              style: TextStyles.caption.copyWith(color: palette.textMuted),
            ),
          ),
          Expanded(
            child: Semantics(
              label: '$label over the last hour',
              child: CustomPaint(
                size: const Size.fromHeight(24),
                painter: _SparklinePainter(
                  samples: samples,
                  line: palette.textMuted,
                  base: palette.hairline,
                ),
              ),
            ),
          ),
          const SizedBox(width: Space.s),
          SizedBox(
            width: 56,
            child: Text(
              now,
              textAlign: TextAlign.right,
              style: TextStyles.caption.copyWith(
                color: palette.textMuted,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _SparklinePainter extends CustomPainter {
  _SparklinePainter({
    required this.samples,
    required this.line,
    required this.base,
  });

  final List<double> samples;
  final Color line;
  final Color base;

  @override
  void paint(Canvas canvas, Size size) {
    final bottom = size.height - 0.5;
    canvas.drawLine(
      Offset(0, bottom),
      Offset(size.width, bottom),
      Paint()..color = base,
    );
    if (samples.length < 2) return;
    final step = size.width / (samples.length - 1);
    final path = Path();
    for (var i = 0; i < samples.length; i++) {
      final point = Offset(
        i * step,
        1 + (size.height - 2) * (1 - samples[i].clamp(0.0, 1.0)),
      );
      i == 0
          ? path.moveTo(point.dx, point.dy)
          : path.lineTo(point.dx, point.dy);
    }
    final fill = Path.from(path)
      ..lineTo(size.width, size.height)
      ..lineTo(0, size.height)
      ..close();
    canvas.drawPath(fill, Paint()..color = line.withValues(alpha: 0.12));
    canvas.drawPath(
      path,
      Paint()
        ..color = line
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.25
        ..strokeJoin = StrokeJoin.round,
    );
  }

  @override
  bool shouldRepaint(_SparklinePainter old) =>
      old.line != line ||
      old.base != base ||
      old.samples.length != samples.length ||
      !Iterable<int>.generate(samples.length)
          .every((i) => old.samples[i] == samples[i]);
}

/// A process (or a task's or monitor's process group): its agent's
/// character when it's an agent's, its label, and its processors and
/// memory in columns at the right.
class _ProcessRow extends StatelessWidget {
  const _ProcessRow({required this.process, required this.store});

  final ResourceProcess process;
  final WorkspaceStore store;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final numbers = TextStyles.caption.copyWith(
      color: palette.textMuted,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final agent = process.agentId.isEmpty ? null : store.agent(process.agentId);
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: Space.xs,
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 26,
            height: 20,
            child: Align(
              alignment: Alignment.centerLeft,
              child: process.agentId.isEmpty
                  ? null
                  : agent == null
                  ? const UnknownCharacter(size: 20)
                  : AgentCharacter(agent: agent, size: 20),
            ),
          ),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  process.label,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyles.body.copyWith(color: palette.text),
                ),
                if (agent != null || process.processes > 1)
                  Text(
                    [
                      ?agent?.name,
                      if (process.processes > 1)
                        '${process.processes} processes',
                    ].join(' · '),
                    style: TextStyles.caption.copyWith(
                      color: palette.textFaint,
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(width: Space.s),
          SizedBox(
            width: 40,
            child: Text(
              _percent(process.cpuPercent),
              textAlign: TextAlign.right,
              style: numbers,
            ),
          ),
          SizedBox(
            width: 64,
            child: Text(
              formatBytes(process.memory),
              textAlign: TextAlign.right,
              style: numbers,
            ),
          ),
        ],
      ),
    );
  }
}

/// What one part of Factory's data or a project's folder takes. An agent's
/// opens to its parts: conversations, task logs, database and checkout.
class _DiskRow extends StatefulWidget {
  const _DiskRow({super.key, required this.usage, required this.store});

  final DiskUsage usage;
  final WorkspaceStore store;

  @override
  State<_DiskRow> createState() => _DiskRowState();
}

class _DiskRowState extends State<_DiskRow> {
  var _open = false;

  /// Build caches being cleaned, by path, and why one couldn't be.
  final _cleaning = <String>{};
  final _failures = <String, String>{};

  Future<void> _clean(String path) async {
    setState(() {
      _cleaning.add(path);
      _failures.remove(path);
    });
    try {
      await AppScope.of(context).clearBuildCache(path);
    } on RequestFailure catch (error) {
      if (mounted) setState(() => _failures[path] = error.message);
    } finally {
      if (mounted) setState(() => _cleaning.remove(path));
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final usage = widget.usage;
    final agent = usage.kind == 'agent' ? widget.store.agent(usage.id) : null;
    final opens = usage.parts.isNotEmpty;
    Widget line(DiskUsage usage, {required bool part}) {
      final copyOnWrite = part && usage.kind == 'checkout';
      return Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    if (!part && agent != null) ...[
                      AgentCharacter(agent: agent, size: 20),
                      const SizedBox(width: Space.xs),
                    ],
                    Flexible(
                      child: Text(
                        usage.label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyles.body.copyWith(
                          color: part ? palette.textMuted : palette.text,
                        ),
                      ),
                    ),
                  ],
                ),
                if (copyOnWrite)
                  Text(
                    'Shares most of its space with the folder it was copied '
                    'from, so it really takes less.',
                    style: TextStyles.caption.copyWith(
                      color: palette.textFaint,
                    ),
                  )
                else if (!part && usage.path.isNotEmpty)
                  Text(
                    shortPath(usage.path),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyles.caption.copyWith(
                      color: palette.textFaint,
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(width: Space.s),
          Text(
            usage.partial
                ? 'at least ${formatBytes(usage.bytes)}'
                : formatBytes(usage.bytes),
            style: TextStyles.caption.copyWith(
              color: palette.textMuted,
              fontFeatures: const [FontFeature.tabularFigures()],
            ),
          ),
          // A build cache can be cleared now; builds fill it again.
          if (part && usage.kind == 'build_cache') ...[
            const SizedBox(width: Space.s),
            Button(
              label: _cleaning.contains(usage.path) ? 'Cleaning…' : 'Clean',
              onPressed: _cleaning.contains(usage.path)
                  ? null
                  : () => _clean(usage.path),
            ),
          ],
        ],
      );
    }

    final row = Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: Space.xs,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(
                width: 14,
                height: 18,
                child: opens
                    ? Center(
                        child: Chevron(color: palette.textFaint, down: _open),
                      )
                    : null,
              ),
              const SizedBox(width: Space.xs),
              Expanded(child: line(usage, part: false)),
            ],
          ),
          if (_open)
            for (final part in usage.parts)
              Padding(
                padding: const EdgeInsets.only(
                  left: 14 + Space.xs,
                  top: Space.xs,
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    line(part, part: true),
                    if (_failures[part.path] case final failure?)
                      ErrorLine(
                        failure,
                        about: 'Cleaning the build cache ${part.path}:',
                      ),
                  ],
                ),
              ),
        ],
      ),
    );
    if (!opens) return row;
    return Pressable(
      onTap: () => setState(() => _open = !_open),
      builder: (context, hovered) => DecoratedBox(
        decoration: BoxDecoration(
          color: hovered ? palette.hover : null,
          borderRadius: BorderRadius.circular(Radii.control),
        ),
        child: row,
      ),
    );
  }
}
