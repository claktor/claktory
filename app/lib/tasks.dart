import 'package:flutter/widgets.dart';

import 'characters/character.dart';
import 'backend.dart';
import 'controls.dart';
import 'explorer.dart';
import 'extensions.dart';
import 'find.dart' show RenderedFindScope;
import 'identity.dart';
import 'mobile/presentation.dart';
import 'mobile/metrics.dart';
import 'markdown.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart';
import 'tell_rivet.dart';

/// Background tasks: the running ones first, then those that finished in
/// the last day, newest first, as the backend orders them. Each says what
/// it runs, when it started, how long it has run (ticking) or took, and how
/// it ended; a click shows its last lines of output, and a running one can
/// be stopped.
class TasksView extends StatelessWidget {
  const TasksView({super.key, required this.tasks, required this.agentId});

  final Tasks tasks;

  /// Whose panel it is: other agents' tasks (seen with "See all agents'
  /// tasks", or as an admin) say whose they are.
  final String agentId;

  /// Searchable with ⌘F, by the words it shows (find.dart).
  @override
  Widget build(BuildContext context) =>
      RenderedFindScope(placeholder: 'Search tasks', child: _view(context));

  Widget _view(BuildContext context) {
    final store = AppScope.of(context).workspace;
    final running = [
      for (final task in tasks.tasks)
        if (task.running) task,
    ];
    final finished = [
      for (final task in tasks.tasks)
        if (!task.running) task,
    ];
    Widget item(BackgroundTask task) => TaskRow(
      key: ValueKey('${task.agentId}/${task.id}'),
      task: task,
      store: store,
      own: task.agentId == agentId,
    );
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => ListView(
        // All of it laid out, for find.
        scrollCacheExtent: RenderedFindScope.everything,
        padding: const EdgeInsets.fromLTRB(Space.s, Space.s, Space.s, Space.l),
        children: [
          // Each selectable where the pointer is at any zoom.
          for (final child in <Widget>[
            if (tasks.tasks.isEmpty)
              const _Empty('No background tasks in the last day.'),
            if (running.isNotEmpty) ...[
              const _Heading('Running'),
              for (final task in running) item(task),
            ],
            if (finished.isNotEmpty) ...[
              const _Heading('Finished'),
              for (final task in finished) item(task),
            ],
          ])
            ScrolledSelection(
              key: child.key == null ? null : ValueKey(child.key),
              child: child,
            ),
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

class _Empty extends StatelessWidget {
  const _Empty(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(horizontal: Space.s, vertical: Space.s),
    child: Text(
      text,
      style: TypeScale.of(context).body
          .copyWith(color: Palette.of(context).textFaint),
    ),
  );
}

/// Whether a finished task's [status] says it went wrong: a failing exit,
/// a signal or a timeout, but not a stop someone asked for.
bool taskFailed(String status) =>
    (status.startsWith('exit ') && status != 'exit 0') ||
    status.startsWith('signal') ||
    status == 'timed out';

/// One task: its dot, command, and under it its id, whose it is (when it's
/// another agent's), when it started, how long it ran and how it ended.
class TaskRow extends StatefulWidget {
  const TaskRow({
    super.key,
    required this.task,
    required this.store,
    required this.own,
  });

  final BackgroundTask task;
  final WorkspaceStore store;

  /// It's the panel agent's own, so it needn't say whose it is.
  final bool own;

  @override
  State<TaskRow> createState() => _TaskRowState();
}

class _TaskRowState extends State<TaskRow> {
  var _open = false;
  var _stopping = false;

  /// Why the last Stop was refused, until the task changes.
  String? _error;

  @override
  void didUpdateWidget(TaskRow old) {
    super.didUpdateWidget(old);
    if (old.task != widget.task) {
      _error = null;
      if (!widget.task.running) _stopping = false;
    }
  }

  Future<void> _stop() async {
    setState(() {
      _stopping = true;
      _error = null;
    });
    try {
      await AppScope.of(context).stopTask(widget.task.agentId, widget.task.id);
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _stopping = false;
          _error = failure.message;
        });
      }
    }
  }

  /// On a phone Stop asks first: it ends what the task runs, and a finger
  /// can land on it by mistake.
  Future<void> _confirmStop() async {
    if (isPhoneLayout(context)) {
      final sure = await showActions<bool>(
        context,
        title: widget.task.command,
        message: 'It stops right away.',
        actions: const [SheetAction('Stop task', true, destructive: true)],
        cancelLabel: 'Keep running',
      );
      if (sure != true || !mounted) return;
    }
    await _stop();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    final task = widget.task;
    final failed = !task.running && taskFailed(task.status);
    final owner = widget.own ? null : widget.store.agent(task.agentId);
    final started = DateTime.fromMillisecondsSinceEpoch(task.started.toInt());
    final caption = TypeScale.of(context).caption.copyWith(
      color: palette.textFaint,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final stop = Button(
      label: 'Stop',
      onPressed: _stopping ? null : _confirmStop,
    );
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
                  child: task.running
                      ? const LiveDot()
                      : Dot(color: failed ? palette.danger : palette.textFaint),
                ),
              ),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      task.command,
                      maxLines: _open ? null : 2,
                      overflow: _open ? null : TextOverflow.ellipsis,
                      style: TypeScale.of(context).code.copyWith(
                        color: task.running ? palette.text : palette.textMuted,
                      ),
                    ),
                    const SizedBox(height: 2),
                    // What's known about it, wrapping in a narrow panel.
                    Wrap(
                      spacing: Space.s,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        Text(task.id, style: caption),
                        if (!widget.own)
                          Row(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              owner == null
                                  ? const UnknownCharacter(size: 20)
                                  : AgentCharacter(agent: owner, size: 20),
                              const SizedBox(width: Space.xs),
                              Flexible(
                                child: Text(
                                  owner?.name ?? task.agentId,
                                  style: TypeScale.of(context).caption
                                      .copyWith(color: palette.textMuted),
                                ),
                              ),
                            ],
                          ),
                        Text(logTime(started), style: caption),
                        Ticking(
                          active: task.running,
                          builder: (context, now) => Text(
                            formatDuration(
                              Duration(
                                milliseconds:
                                    (task.running
                                        ? now.millisecondsSinceEpoch
                                        : task.ended.toInt()) -
                                    task.started.toInt(),
                              ),
                              whole: task.running,
                            ),
                            style: caption.copyWith(color: palette.textMuted),
                          ),
                        ),
                        if (!task.running && task.status.isNotEmpty)
                          Text(
                            task.status,
                            style: TypeScale.of(context).caption.copyWith(
                              color: failed
                                  ? palette.danger
                                  : palette.textMuted,
                            ),
                          ),
                      ],
                    ),
                    if (_error != null)
                      Padding(
                        padding: const EdgeInsets.only(top: 2),
                        child: ErrorLine(
                          _error!,
                          about:
                              'Stopping ${task.id} of agent ${task.agentId} '
                              'was refused.',
                        ),
                      ),
                    // On a phone Stop is under it once it's open: beside it
                    // it would leave the command no room.
                    if (phone && _open && task.running) ...[
                      const SizedBox(height: Space.s),
                      stop,
                    ],
                    if (_open) ...[
                      const SizedBox(height: Space.s),
                      if (task.logTail.isEmpty)
                        Text(
                          task.running ? 'No output yet' : 'No output',
                          style: TypeScale.of(context).caption
                              .copyWith(color: palette.textFaint),
                        )
                      else
                        CodeBlock(task.logTail, links: true),
                      if (task.logPath.isNotEmpty) ...[
                        const SizedBox(height: Space.xs),
                        Text(
                          shortPath(task.logPath),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TypeScale.of(context).caption
                              .copyWith(color: palette.textFaint),
                        ),
                      ],
                    ],
                  ],
                ),
              ),
              if (task.running && !phone) ...[
                const SizedBox(width: Space.s),
                stop,
              ],
              const SizedBox(width: Space.s),
              SizedBox(
                height: task.running && !phone ? 28 : 18,
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
