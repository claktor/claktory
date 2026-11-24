// Pipe groups (#359): a project's pipes gathered under named rows in the
// sidebar, such as every friend's feedback pipe under one. Arrangement
// only: a pipe in a group works as it did outside one. The groups, their
// names, which pipes they hold and whether they're folded away live in the
// Claktory (Workspace.pipe_groups, Pipe.group_id), so every device shows
// the same. Here: the group's glyph, a pipe's "Move to group…", a group's
// Rename… and Delete group…, and the forms they open. The rows themselves
// are the sidebar's (sidebar.dart).
import 'dart:async';

import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'forms.dart';
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'pipes.dart' show pipeColor, pipesIn;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// [projectId]'s pipe groups, in their order (PipeGroup.position).
List<PipeGroup> pipeGroupsIn(Workspace workspace, String projectId) => [
  for (final group in workspace.pipeGroups)
    if (group.projectId == projectId) group,
]..sort((a, b) => a.position.compareTo(b.position));

/// The group [pipe] is in, if it's one of its project's.
PipeGroup? groupOf(Workspace workspace, Pipe pipe) {
  if (pipe.groupId.isEmpty) return null;
  for (final group in workspace.pipeGroups) {
    if (group.id == pipe.groupId && group.projectId == pipe.projectId) {
      return group;
    }
  }
  return null;
}

/// A project's pipe rows: each group with its pipes (in the order they
/// were made), then the pipes in none. A pipe whose group is gone, or is
/// another project's, counts as in none, so every pipe shows.
({List<(PipeGroup, List<Pipe>)> groups, List<Pipe> loose}) pipeRowsIn(
  Workspace workspace,
  String projectId,
) {
  final groups = pipeGroupsIn(workspace, projectId);
  final ids = {for (final group in groups) group.id};
  final pipes = pipesIn(workspace, projectId);
  return (
    groups: [
      for (final group in groups)
        (
          group,
          [
            for (final pipe in pipes)
              if (pipe.groupId == group.id) pipe,
          ],
        ),
    ],
    loose: [
      for (final pipe in pipes)
        if (!ids.contains(pipe.groupId)) pipe,
    ],
  );
}

/// What VoiceOver reads for a group's row: "Friend feedback, group of 4
/// pipes, collapsed", and that something's new.
String pipeGroupLabel(PipeGroup group, int pipes, {required bool unseen}) => [
  group.name,
  'group of $pipes ${pipes == 1 ? 'pipe' : 'pipes'}',
  group.collapsed ? 'collapsed' : 'expanded',
  if (unseen) 'new messages',
].join(', ');

/// A pipe group's glyph: three pipes coming in from the left and meeting
/// in one portal: many pipes feeding one place. On the pipe portal's
/// 16-unit grid, in its 1.5 stroke.
class PipeGroupGlyph extends StatelessWidget {
  const PipeGroupGlyph({super.key, this.color = pipeColor, this.size = 16});

  final Color color;
  final double size;

  @override
  Widget build(BuildContext context) =>
      CustomPaint(size: Size.square(size), painter: _GroupPainter(color));
}

class _GroupPainter extends CustomPainter {
  const _GroupPainter(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final unit = size.shortestSide / 16;
    canvas.scale(unit);
    final stroke = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5
      ..strokeCap = StrokeCap.round;
    // The place they feed: a portal's rim and heart, at the right.
    const portal = Offset(10.75, 8);
    canvas
      // The pipes, from the left edge into the rim.
      ..drawLine(const Offset(1.25, 2.75), const Offset(6.4, 5.9), stroke)
      ..drawLine(const Offset(1.25, 8), const Offset(5.75, 8), stroke)
      ..drawLine(const Offset(1.25, 13.25), const Offset(6.4, 10.1), stroke)
      ..drawCircle(portal, 4.5, stroke)
      ..drawCircle(portal, 1.7, Paint()..color = color);
  }

  @override
  bool shouldRepaint(_GroupPainter old) => old.color != color;
}

/// A pipe's actions about its group, from its row's right click or touch
/// and hold, or its header's "…": Move to group… (the project's other
/// groups, then New group…), and Remove from group while it's in one.
Future<void> pipeGroupActions(BuildContext context, Pipe pipe) async {
  final origin = _Origin(context);
  final workspace = AppScope.of(context).workspace.workspace;
  final group = groupOf(workspace, pipe);
  final action = await showActions<String>(
    context,
    title: pipe.name,
    actions: [
      const SheetAction('Move to group…', 'move'),
      if (group != null) const SheetAction('Remove from group', 'remove'),
    ],
  );
  final here = origin.context;
  if (here == null || !here.mounted) return;
  switch (action) {
    case 'move':
      await _moveToGroup(here, origin, pipe);
    case 'remove':
      _setGroup(origin, pipe, '');
  }
}

/// Moves [pipe] to one of its project's groups, chosen from a list of
/// them (the one it's in left out), or to a new one (New group…).
Future<void> moveToGroup(BuildContext context, Pipe pipe) =>
    _moveToGroup(context, _Origin(context), pipe);

Future<void> _moveToGroup(
  BuildContext context,
  _Origin origin,
  Pipe pipe,
) async {
  final workspace = AppScope.of(context).workspace.workspace;
  final groups = [
    for (final group in pipeGroupsIn(workspace, pipe.projectId))
      if (group.id != pipe.groupId) group,
  ];
  final chosen = await showActions<String>(
    context,
    title: 'Move to group',
    actions: [
      for (final group in groups)
        SheetAction(
          group.name,
          group.id,
          glyph: (color) => PipeGroupGlyph(color: color),
        ),
      const SheetAction('New group…', ''),
    ],
  );
  if (chosen == null) return;
  if (chosen.isEmpty) {
    final here = origin.context;
    if (here == null || !here.mounted) return;
    await newPipeGroup(here, projectId: pipe.projectId, pipe: pipe);
  } else {
    _setGroup(origin, pipe, chosen);
  }
}

/// Takes [pipe] out of its group: it's among the pipes in none.
void removeFromGroup(BuildContext context, Pipe pipe) =>
    _setGroup(_Origin(context), pipe, '');

/// Where what follows a choice shows: the row it came from while it's
/// there, else the list around it (the sidebar's). A sheet left open can
/// outlive its row: a group above it deleted from another device shifts
/// the rows under it, and a move that fails then must still say so.
class _Origin {
  _Origin(this.row)
    : list = row.findAncestorStateOfType<ScrollableState>()?.context;

  final BuildContext row;
  final BuildContext? list;

  BuildContext? get context => row.mounted
      ? row
      : list != null && list!.mounted
      ? list
      : null;
}

/// Puts [pipe] in [groupId] ("" for none), saying why if the Claktory
/// won't.
void _setGroup(_Origin origin, Pipe pipe, String groupId) {
  final here = origin.context;
  if (here == null || !here.mounted) return;
  final backend = AppScope.of(here);
  backend.setPipeGroup(pipe.id, groupId).catchError((Object error) {
    final there = origin.context;
    if (there == null || !there.mounted || error is! RequestFailure) return;
    unawaited(
      showActions<void>(
        there,
        title: groupId.isEmpty
            ? "Couldn't remove ${pipe.name} from its group"
            : "Couldn't move ${pipe.name}",
        message: error.message,
        actions: const [],
        cancelLabel: 'OK',
      ),
    );
  }, test: (error) => error is RequestFailure);
}

/// Drops [pipe] on [group]'s row: it moves in, as Move to group… does.
void dropPipeOnGroup(BuildContext context, Pipe pipe, PipeGroup group) {
  if (pipe.groupId == group.id || pipe.projectId != group.projectId) return;
  _setGroup(_Origin(context), pipe, group.id);
}

/// A group's actions, from its row's right click or touch and hold:
/// Rename… and Delete group….
Future<void> pipeGroupMenu(BuildContext context, PipeGroup group) async {
  final origin = _Origin(context);
  final action = await showActions<String>(
    context,
    title: group.name,
    actions: const [
      SheetAction('Rename…', 'rename'),
      SheetAction('Delete group…', 'delete', destructive: true),
    ],
  );
  final here = origin.context;
  if (here == null || !here.mounted) return;
  switch (action) {
    case 'rename':
      await showModal(
        here,
        (context, close) => _GroupNameForm(group: group, close: close),
      );
    case 'delete':
      await deletePipeGroup(here, group);
  }
}

/// A new group in [projectId], [pipe] moved into it when it's made from a
/// pipe's menu.
Future<void> newPipeGroup(
  BuildContext context, {
  required String projectId,
  Pipe? pipe,
}) => showModal(
  context,
  (context, close) =>
      _GroupNameForm(projectId: projectId, pipe: pipe, close: close),
);

/// Deletes [group] once the person says so; its pipes stay, outside any
/// group.
Future<void> deletePipeGroup(BuildContext context, PipeGroup group) async {
  final backend = AppScope.of(context);
  final sure = await showActions<bool>(
    context,
    title: 'Delete the group “${group.name}”?',
    message:
        'Its pipes stay, just outside any group. Nothing about them '
        'changes.',
    actions: const [SheetAction('Delete group', true, destructive: true)],
  );
  if (sure != true) return;
  fireAndLog(backend.deletePipeGroup(group.id));
}

/// New group (from a project and maybe a pipe) or Rename group (from a
/// group): its name, refused names said as the Claktory says them.
class _GroupNameForm extends StatefulWidget {
  const _GroupNameForm({
    required this.close,
    this.group,
    this.projectId = '',
    this.pipe,
  });

  /// The group to rename; null for a new one.
  final PipeGroup? group;
  final String projectId;

  /// The pipe that moves into the new group.
  final Pipe? pipe;
  final VoidCallback close;

  @override
  State<_GroupNameForm> createState() => _GroupNameFormState();
}

class _GroupNameFormState extends State<_GroupNameForm> {
  late final _name = TextEditingController(text: widget.group?.name ?? '');
  var _saving = false;
  String? _error;

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    if (_saving) return;
    setState(() {
      _saving = true;
      _error = null;
    });
    final backend = AppScope.of(context);
    final name = _name.text.trim();
    try {
      if (widget.group case final group?) {
        if (name != group.name) await backend.renamePipeGroup(group.id, name);
      } else {
        await backend.createPipeGroup(
          widget.projectId,
          name,
          pipeId: widget.pipe?.id ?? '',
        );
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
  Widget build(BuildContext context) {
    final renaming = widget.group != null;
    return FormFrame(
      title: renaming ? 'Rename group' : 'New group',
      error: _error,
      submitLabel: renaming ? 'Save' : 'Create',
      onSubmit: _saving ? null : _save,
      onCancel: widget.close,
      fields: [
        Field(
          label: 'Name',
          child: TextField(
            controller: _name,
            autofocus: true,
            placeholder: 'Friend feedback',
            onSubmitted: (_) => _save(),
          ),
        ),
      ],
    );
  }
}
