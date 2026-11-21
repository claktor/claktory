import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'extensions.dart' show logTime;
import 'identity.dart';
import 'mobile/lines.dart';
import 'mobile/presentation.dart';
import 'mobile/metrics.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart';
import 'tell_rivet.dart';

/// An agent's monitors: what will prompt it later. Active ones first, then
/// the paused and failed ones (which can be resumed), then those that ended
/// in the last week. Each says what it waits for, when it next fires
/// (ticking), when it last did and how often; a click shows the prompt it
/// sends. Underneath, quietly, what else will wake the agent.
class MonitorsView extends StatelessWidget {
  const MonitorsView({
    super.key,
    required this.monitors,
    required this.agentId,
  });

  final Monitors monitors;

  /// Whose monitors they are.
  final String agentId;

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    List<Monitor> inState(Set<MonitorState> states) => [
      for (final monitor in monitors.monitors)
        if (states.contains(monitor.state)) monitor,
    ];
    final active = inState({MonitorState.MONITOR_STATE_ACTIVE});
    final paused = inState({
      MonitorState.MONITOR_STATE_PAUSED,
      MonitorState.MONITOR_STATE_ERROR,
    });
    final ended = inState({
      MonitorState.MONITOR_STATE_DONE,
      MonitorState.MONITOR_STATE_CANCELLED,
    });
    Widget item(Monitor monitor) => MonitorRow(
      key: ValueKey(monitor.id),
      monitor: monitor,
      agentId: agentId,
      store: store,
    );
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => ListView(
        padding: const EdgeInsets.fromLTRB(Space.s, Space.s, Space.s, Space.l),
        children: [
          if (monitors.monitors.isEmpty)
            const _Line(
              'No monitors yet. The agent adds one when it needs to '
              'come back to something later.',
            ),
          if (active.isNotEmpty) ...[
            const _Heading('Active'),
            for (final monitor in active) item(monitor),
          ],
          if (paused.isNotEmpty) ...[
            const _Heading('Paused'),
            for (final monitor in paused) item(monitor),
          ],
          if (ended.isNotEmpty) ...[
            const _Heading('Ended'),
            for (final monitor in ended) item(monitor),
          ],
          if (monitors.builtins.isNotEmpty) ...[
            const _Heading('Also wakes it'),
            for (final line in monitors.builtins) _Line(line, caption: true),
          ],
        ],
      ),
    );
  }
}

class _Heading extends StatelessWidget {
  const _Heading(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.m, Space.s, Space.xs),
      child: Text(
        text,
        style: TypeScale.of(context).caption
            .copyWith(color: palette.textFaint, fontWeight: FontWeight.w500),
      ),
    );
  }
}

/// A quiet line: the empty list, or one of the other things that wake it.
class _Line extends StatelessWidget {
  const _Line(this.text, {this.caption = false});

  final String text;
  final bool caption;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: Space.xs,
      ),
      child: Text(
        text,
        style: caption
            ? TypeScale.of(context).caption.copyWith(color: palette.textMuted)
            : TypeScale.of(context).body.copyWith(color: palette.textFaint),
      ),
    );
  }
}

/// What a monitor's state is called, with why when there's a reason:
/// "Paused by the person", "Failed: its command kept failing".
String monitorStateLabel(Monitor monitor) {
  final reason = monitor.reason;
  return switch (monitor.state) {
    MonitorState.MONITOR_STATE_ACTIVE => 'Active',
    MonitorState.MONITOR_STATE_PAUSED =>
      reason.isEmpty ? 'Paused' : 'Paused $reason',
    MonitorState.MONITOR_STATE_DONE =>
      reason.isEmpty ? 'Done' : 'Done: $reason',
    MonitorState.MONITOR_STATE_CANCELLED =>
      reason.isEmpty ? 'Cancelled' : 'Cancelled $reason',
    MonitorState.MONITOR_STATE_ERROR =>
      reason.isEmpty ? 'Failed' : 'Failed: $reason',
    _ => '',
  };
}

/// When an active monitor next fires: "next in 4m 12s", "due now"; empty
/// when it has no time.
String nextDueLabel(Monitor monitor, DateTime now) {
  if (monitor.state != MonitorState.MONITOR_STATE_ACTIVE ||
      monitor.nextDue <= 0) {
    return '';
  }
  final left = monitor.nextDue.toInt() - now.millisecondsSinceEpoch;
  if (left <= 0) return 'due now';
  // Rounded up, so it says 0s only as it comes due.
  final seconds = (left / 1000).ceil();
  return 'next in ${formatDuration(Duration(seconds: seconds), whole: true)}';
}

/// One monitor: its dot and name, what it waits for, and under it its
/// number, state, next and last fire and how many; Pause or Resume and
/// Cancel while it hasn't ended.
class MonitorRow extends StatefulWidget {
  const MonitorRow({
    super.key,
    required this.monitor,
    required this.agentId,
    required this.store,
  });

  final Monitor monitor;
  final String agentId;
  final WorkspaceStore store;

  @override
  State<MonitorRow> createState() => _MonitorRowState();
}

class _MonitorRowState extends State<MonitorRow> {
  var _open = false;
  var _busy = false;

  /// Why the last change was refused, until the monitor changes.
  String? _error;

  @override
  void didUpdateWidget(MonitorRow old) {
    super.didUpdateWidget(old);
    if (old.monitor != widget.monitor) {
      _error = null;
      _busy = false;
    }
  }

  Future<void> _change(MonitorAction action) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context)
          .changeMonitor(widget.agentId, widget.monitor.id, action);
      // The view that follows shows the change; until then, nothing more.
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  /// On a phone Cancel asks first: it can't be undone, and a finger can
  /// land on it by mistake.
  Future<void> _cancel() async {
    if (isPhoneLayout(context)) {
      final sure = await showActions<bool>(
        context,
        title: widget.monitor.name,
        message: "It won't fire again.",
        actions: const [SheetAction('Cancel monitor', true, destructive: true)],
        cancelLabel: 'Keep monitor',
      );
      if (sure != true || !mounted) return;
    }
    await _change(MonitorAction.MONITOR_ACTION_CANCEL);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    final monitor = widget.monitor;
    final state = monitor.state;
    final active = state == MonitorState.MONITOR_STATE_ACTIVE;
    final failed = state == MonitorState.MONITOR_STATE_ERROR;
    final ended =
        state == MonitorState.MONITOR_STATE_DONE ||
        state == MonitorState.MONITOR_STATE_CANCELLED;
    final caption = TypeScale.of(context).caption.copyWith(
      color: palette.textFaint,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final muted = caption.copyWith(color: palette.textMuted);
    final setter = monitor.createdBy.isEmpty
        ? null
        : widget.store.agent(monitor.createdBy);
    final fires = monitor.maxFires > 0
        ? '${monitor.fires} of ${monitor.maxFires} fires'
        : monitor.fires == 1
        ? '1 fire'
        : '${monitor.fires} fires';
    DateTime time(int ms) => DateTime.fromMillisecondsSinceEpoch(ms);
    final actions = ended
        ? const <Widget>[]
        : [
            Button(
              label: active ? 'Pause' : 'Resume',
              onPressed: _busy
                  ? null
                  : () => _change(
                      active
                          ? MonitorAction.MONITOR_ACTION_PAUSE
                          : MonitorAction.MONITOR_ACTION_RESUME,
                    ),
            ),
            Button(label: 'Cancel', onPressed: _busy ? null : _cancel),
          ];
    return Semantics(
      container: true,
      expanded: _open,
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
              SizedBox(
                width: Space.l + 2,
                height: 18,
                child: Align(
                  alignment: Alignment.centerLeft,
                  child: Dot(
                    color: active
                        ? palette.signal
                        : failed
                        ? palette.danger
                        : ended
                        ? palette.textFaint
                        : palette.textMuted,
                  ),
                ),
              ),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      monitor.name,
                      maxLines: _open ? null : 1,
                      overflow: _open ? null : TextOverflow.ellipsis,
                      style: TypeScale.of(context).label.copyWith(
                        color: ended ? palette.textMuted : palette.text,
                      ),
                    ),
                    Text(
                      monitor.when,
                      maxLines: _open ? null : 2,
                      overflow: _open ? null : TextOverflow.ellipsis,
                      style: TypeScale.of(context).body
                          .copyWith(color: palette.textMuted),
                    ),
                    const SizedBox(height: 2),
                    // What's known about it, wrapping in a narrow panel.
                    Wrap(
                      spacing: Space.s,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        Text('m${monitor.id}', style: caption),
                        if (!active)
                          Text(
                            monitorStateLabel(monitor),
                            style: muted.copyWith(
                              color: failed ? palette.danger : null,
                            ),
                          ),
                        if (failed)
                          TellRivet(
                            error: monitorStateLabel(monitor),
                            about:
                                'Monitor m${monitor.id} ("${monitor.name}") of '
                                'agent ${widget.agentId} failed.',
                            link: true,
                          ),
                        if (active && monitor.nextDue > 0)
                          Ticking(
                            active: true,
                            builder: (context, now) =>
                                Text(nextDueLabel(monitor, now), style: muted),
                          ),
                        Text(
                          monitor.lastFired > 0
                              ? 'last fired ${logTime(time(monitor.lastFired.toInt()))}'
                              : 'never fired',
                          style: caption,
                        ),
                        Text(fires, style: caption),
                        if (monitor.until > 0 && !ended)
                          Text(
                            'until ${logTime(time(monitor.until.toInt()))}',
                            style: caption,
                          ),
                        if (monitor.idleOnly)
                          Text('while idle', style: caption),
                      ],
                    ),
                    if (_error != null)
                      Padding(
                        padding: const EdgeInsets.only(top: 2),
                        child: ErrorLine(
                          _error!,
                          about:
                              'Changing monitor m${monitor.id} of agent '
                              '${widget.agentId} was refused.',
                        ),
                      ),
                    if (_open) ...[
                      const SizedBox(height: Space.s),
                      Text(
                        'Tells it',
                        style: TypeScale.of(context).caption
                            .copyWith(color: palette.textFaint),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        monitor.prompt,
                        style: TypeScale.of(context).body
                            .copyWith(color: palette.text),
                      ),
                      const SizedBox(height: Space.xs),
                      Text.rich(
                        TextSpan(
                          children: [
                            TextSpan(
                              text:
                                  'Set ${logTime(time(monitor.created.toInt()))} by ',
                            ),
                            TextSpan(
                              text: monitor.createdBy.isEmpty
                                  ? 'you'
                                  : setter?.name ?? monitor.createdBy,
                              style: TextStyle(
                                color: setter == null
                                    ? palette.textMuted
                                    : agentColor(setter.color),
                              ),
                            ),
                          ],
                        ),
                        style: caption,
                      ),
                    ],
                    // On a phone its actions are under it once it's open:
                    // beside it they'd leave its name no room.
                    if (phone && _open && actions.isNotEmpty) ...[
                      const SizedBox(height: Space.s),
                      ControlsLine(children: actions),
                    ],
                  ],
                ),
              ),
              if (!phone && !ended) ...[
                const SizedBox(width: Space.s),
                ...actions,
              ],
              const SizedBox(width: Space.xs),
              SizedBox(
                height: ended || phone ? 18 : 28,
                child: Center(
                  child: Chevron(
                    color: hovered ? palette.textMuted : palette.textFaint,
                    down: _open,
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
