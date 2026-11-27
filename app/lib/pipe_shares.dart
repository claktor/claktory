// Websites shared over pipes (its revised plan): a
// website on this computer (a dashboard on port 3000, say) shared with one
// pipe, several, or a pipe group's, which the people at their other ends
// open in their own browser. The shares live in the Claktory
// (Workspace.pipe_shares, both kinds), so every device shows the same;
// their chat lines are the pipes' own Claktory lines. Here:
//
// - Share a website… (from a pipe's settings or a group's menu): a port, a
//   name, the pipes or the group, and Claktory's one warning sentence.
// - Shared websites, in a pipe's settings: the shares on it, this side's
//   with Stop, the other side's with Hide or Show.
// - A share another Claktory shares, under the pipe's header: Open, Copy
//   address, Hide.
// - What moving a pipe into or out of a group changes in what it gets.
// - How long a share lasts: chosen when it's shared, 24
//   hours unless changed, shown on it as the time left, and changed from
//   this side's share in a pipe's settings.
import 'dart:async';
import 'dart:math' as math;

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import 'android/share_browser_lifecycle.dart';
import 'approve_elsewhere.dart';
import 'backend.dart';
import 'claktory_identity.dart' show claktoryName;
import 'controls.dart';
import 'devices.dart' show thisDeviceKind;
import 'forms.dart' show FormFrame;
import 'identity.dart';
import 'loopback_forward.dart';
import 'mobile/input.dart' show InputKind;
import 'mobile/lines.dart' show ActionLine;
import 'mobile/metrics.dart' show TypeScale;
import 'mobile/share_browser.dart' show inAppShareBrowser;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'pipe_groups.dart' show pipeGroupsIn;
import 'pipes.dart' show pipeColor, pipePeerName, pipesIn;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'visible_codes.dart' show visibleCodes;

/// The shares on [pipe]: this side's that reach it, and the other side's
/// that came over it.
List<PipeShare> sharesOn(Workspace workspace, Pipe pipe) => [
  for (final share in workspace.pipeShares)
    if (share.pipeIds.contains(pipe.id)) share,
];

/// This side's shares that follow group [groupId].
List<PipeShare> groupShares(Workspace workspace, String groupId) => [
  if (groupId.isNotEmpty)
    for (final share in workspace.pipeShares)
      if (share.mine && share.groupId == groupId) share,
];

/// Claktory's one warning, under what's shared: what the people on the
/// pipes get, naming the port, since another program can take it later,
/// and the [computer] it's on ([shareComputer]).
String shareWarning(String port, {String computer = 'this computer'}) =>
    'People on these pipes can use everything $computer serves on '
    'port $port.';

/// The computer whose ports a website is shared from, as the sentences
/// about sharing name it: "this computer" when the app runs
/// there; else its Claktory's name as the app shows it ("Studio"), so the
/// person on a MacBook connected to it never reads it as the MacBook.
String shareComputer(Backend backend) =>
    backend.forwards.remote() ? claktoryName(backend) : 'this computer';

/// What moving [pipe] into [groupId] ("" for out of any) changes in the
/// shares it gets, a sentence for each way; null when nothing.
String? groupMoveNote(Workspace workspace, Pipe pipe, String groupId) {
  if (groupId == pipe.groupId) return null;
  String names(Iterable<PipeShare> shares) =>
      shares.map((share) => visibleCodes(share.name)).join(', ');
  final gains = [
    for (final share in groupShares(workspace, groupId))
      if (!share.pipeIds.contains(pipe.id)) share,
  ];
  final losses = [
    for (final share in groupShares(workspace, pipe.groupId))
      if (share.pipeIds.contains(pipe.id)) share,
  ];
  if (gains.isEmpty && losses.isEmpty) return null;
  return [
    if (gains.isNotEmpty) 'This pipe will also get: ${names(gains)}.',
    if (losses.isNotEmpty) 'This pipe will stop getting: ${names(losses)}.',
  ].join(' ');
}

/// Asks before a move of [pipe] into [groupId] ("" for out of any) that
/// changes the websites it gets; true to go on (at once when it changes
/// none).
Future<bool> confirmGroupMove(
  BuildContext context,
  Pipe pipe,
  String groupId,
) async {
  final workspace = AppScope.of(context).workspace.workspace;
  final note = groupMoveNote(workspace, pipe, groupId);
  if (note == null) return true;
  String? group;
  for (final each in workspace.pipeGroups) {
    if (each.id == groupId) group = each.name;
  }
  final sure = await showActions<bool>(
    context,
    title: groupId.isEmpty
        ? 'Remove ${pipe.name} from its group?'
        : 'Move ${pipe.name} to ${group ?? 'the group'}?',
    message: note,
    actions: [SheetAction(groupId.isEmpty ? 'Remove' : 'Move', true)],
  );
  return sure == true;
}

/// What deleting [group] says of its shares: they stay with the pipes they
/// reach now; null when it has none.
String? groupDeleteNote(Workspace workspace, PipeGroup group) {
  final shares = groupShares(workspace, group.id);
  if (shares.isEmpty) return null;
  final names = shares.map((share) => visibleCodes(share.name)).join(', ');
  return shares.length == 1
      ? '$names stays shared with its pipes.'
      : '$names stay shared with their pipes.';
}

/// Opens Share a website… for [pipe], or for [group]'s pipes.
Future<void> shareWebsite(
  BuildContext context, {
  Pipe? pipe,
  PipeGroup? group,
}) async {
  // Sharing a website through pipes is the person's.
  if (approvesElsewhere) return;
  await showModal(
    context,
    (context, close) =>
        ShareWebsiteForm(pipe: pipe, group: group, close: close),
  );
}

/// How long a share can last, as Share a website… and Change end… offer
/// them: seconds from now, 0 for until it's stopped.
const shareLengths = [
  SelectOption(3600, 'After 1 hour'),
  SelectOption(86400, 'After 24 hours'),
  SelectOption(604800, 'After 7 days'),
  SelectOption(0, 'Never'),
];

/// What Share a website… chooses to begin with: 24 hours.
const defaultShareLength = 86400;

/// The time a share has left, at [now]: "Ends in 3 hours"; null when it
/// lasts until it's stopped.
String? shareEnds(PipeShare share, DateTime now) {
  if (share.expiresMs <= 0) return null;
  final left = share.expiresMs.toInt() - now.millisecondsSinceEpoch;
  if (left <= 0) return 'Ending now';
  String of(int n, String one, String many) => n == 1 ? '1 $one' : '$n $many';
  final minutes = (left / Duration.millisecondsPerMinute).ceil();
  if (minutes < 60) return 'Ends in ${of(minutes, 'minute', 'minutes')}';
  final hours = (left / Duration.millisecondsPerHour).ceil();
  if (hours <= 48) return 'Ends in ${of(hours, 'hour', 'hours')}';
  final days = (left / Duration.millisecondsPerDay).ceil();
  return 'Ends in ${of(days, 'day', 'days')}';
}

/// The clock the time a share has left counts from; tests replace it.
DateTime Function() shareClock = DateTime.now;

/// Rebuilds [builder] with the time now, every half minute while [share]
/// has an end, so the time left it shows keeps counting down.
class ShareCountdown extends StatefulWidget {
  const ShareCountdown({super.key, required this.share, required this.builder});

  final PipeShare share;
  final Widget Function(BuildContext context, DateTime now) builder;

  @override
  State<ShareCountdown> createState() => _ShareCountdownState();
}

class _ShareCountdownState extends State<ShareCountdown> {
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    _schedule();
  }

  @override
  void didUpdateWidget(ShareCountdown old) {
    super.didUpdateWidget(old);
    if ((old.share.expiresMs > 0) != (widget.share.expiresMs > 0)) _schedule();
  }

  void _schedule() {
    _tick?.cancel();
    _tick = widget.share.expiresMs > 0
        ? Timer.periodic(const Duration(seconds: 30), (_) {
            if (mounted) setState(() {});
          })
        : null;
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.builder(context, shareClock());
}

/// Asks how long [share], this side's, should last from now, and sets it.
/// The choice is sent even when the share's row is gone meanwhile (the
/// share ended while the sheet was open), so the backend's refusal is
/// said rather than nothing happening.
Future<void> changeShareEnd(BuildContext context, PipeShare share) async {
  if (approvesElsewhere) return;
  final backend = AppScope.of(context);
  final origin = _Origin(context);
  final name = visibleCodes(share.name);
  final lasts = await showActions<int>(
    context,
    title: 'When should $name stop being shared?',
    message: 'Counted from now. People on its pipes can open it until then.',
    actions: [
      for (final length in shareLengths)
        SheetAction(length.label, length.value),
    ],
  );
  if (lasts == null) return;
  try {
    await backend.setPipeShareExpiry(share.id, lasts);
  } on RequestFailure catch (failure) {
    final here = origin.context;
    if (here == null || !here.mounted) return;
    await showActions<void>(
      here,
      title: "Couldn't change when $name ends",
      message: failure.message,
      actions: const [],
      cancelLabel: 'OK',
    );
  }
}

/// Stops [share] once the person says so.
Future<void> stopShare(BuildContext context, PipeShare share) async {
  if (approvesElsewhere) return;
  final backend = AppScope.of(context);
  final origin = _Origin(context);
  final sure = await showActions<bool>(
    context,
    title: 'Stop sharing ${visibleCodes(share.name)}?',
    message:
        'People on its pipes can no longer open it. Shared again, it gets '
        'a new address.',
    actions: const [SheetAction('Stop sharing', true, destructive: true)],
  );
  if (sure != true) return;
  try {
    await backend.stopPipeShare(share.id);
  } on RequestFailure catch (failure) {
    final here = origin.context;
    if (here == null || !here.mounted) return;
    await showActions<void>(
      here,
      title: "Couldn't stop sharing ${visibleCodes(share.name)}",
      message: failure.message,
      actions: const [],
      cancelLabel: 'OK',
    );
  }
}

/// Where a share's row asked from: the row while it's there, else the list
/// it was in, else the app's own navigator, so an answer that comes after
/// the share left the screen still has somewhere to be said.
class _Origin {
  _Origin(this.row)
    : list = row.findAncestorStateOfType<ScrollableState>()?.context,
      root = Navigator.maybeOf(row, rootNavigator: true)?.context;
  final BuildContext row;
  final BuildContext? list;
  final BuildContext? root;

  BuildContext? get context {
    for (final here in [row, list, root]) {
      if (here != null && here.mounted) return here;
    }
    return null;
  }
}

/// Share a website…: a port on this computer, what it's called, and the
/// open pipes to share it with, or a group's; then Claktory's warning
/// naming the port, and Share.
class ShareWebsiteForm extends StatefulWidget {
  const ShareWebsiteForm({
    super.key,
    required this.close,
    this.pipe,
    this.group,
  });

  /// The pipe it's opened from, chosen to begin with.
  final Pipe? pipe;

  /// The group it's opened from, chosen to begin with.
  final PipeGroup? group;
  final VoidCallback close;

  @override
  State<ShareWebsiteForm> createState() => _ShareWebsiteFormState();
}

class _ShareWebsiteFormState extends State<ShareWebsiteForm> {
  final _port = TextEditingController();
  final _name = TextEditingController();

  /// "" for chosen pipes ([_pipes]), else the group it follows.
  late var _target = widget.group?.id ?? '';
  late final _pipes = {if (widget.pipe case final pipe?) pipe.id};

  /// How long it lasts, in seconds; 0 until it's stopped.
  var _lasts = defaultShareLength;

  /// What the Claktory said of the port typed, once it has.
  PipeSharePort? _check;
  Timer? _checking;
  var _busy = false;
  String? _error;

  String get _projectId =>
      widget.pipe?.projectId ?? widget.group?.projectId ?? '';

  int? get _portNumber {
    final port = int.tryParse(_port.text.trim());
    return port != null && port >= 1 && port <= 65535 ? port : null;
  }

  @override
  void initState() {
    super.initState();
    _port.addListener(_portChanged);
    _name.addListener(_nameChanged);
  }

  /// What each field held when last looked at: their listeners also hear
  /// the cursor move.
  var _lastPort = '';
  var _lastName = '';

  /// What's typed answers what the form said of it: the message goes.
  void _nameChanged() {
    if (_name.text == _lastName) return;
    _lastName = _name.text;
    setState(() => _error = null);
  }

  @override
  void dispose() {
    _checking?.cancel();
    _port.dispose();
    _name.dispose();
    super.dispose();
  }

  /// Asks the Claktory about the port once typing pauses.
  void _portChanged() {
    if (_port.text == _lastPort) return;
    _lastPort = _port.text;
    _checking?.cancel();
    setState(() {
      _check = null;
      _error = null;
    });
    final port = _portNumber;
    if (port == null) return;
    _checking = Timer(const Duration(milliseconds: 400), () async {
      try {
        final check = await AppScope.of(context).checkPipeSharePort(port);
        if (mounted && _portNumber == check.port) {
          setState(() => _check = check);
        }
      } on RequestFailure {
        // Share says why, if it's refused.
      }
    });
  }

  List<Pipe> _open(Workspace workspace) => [
    for (final pipe in pipesIn(workspace, _projectId))
      if (pipe.state == PipeState.PIPE_STATE_OPEN) pipe,
  ];

  Future<void> _share() async {
    if (_busy) return;
    final port = _portNumber;
    final name = _name.text.trim();
    final String? problem = port == null
        ? 'Enter a port from 1 to 65535, such as 3000.'
        : name.isEmpty
        ? 'Name it, so people on the pipes know what it is.'
        : _target.isEmpty && _pipes.isEmpty
        ? 'Choose at least one pipe to share it with.'
        : null;
    if (problem != null) {
      setState(() => _error = problem);
      return;
    }
    final workspace = AppScope.of(context).workspace.workspace;
    final open = {for (final pipe in _open(workspace)) pipe.id};
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).sharePipeWebsite(
        SharePipeWebsite(
          port: port,
          name: name,
          pipeIds: _target.isEmpty ? _pipes.intersection(open) : const [],
          groupId: _target,
          lastsS: Int64(_lasts),
        ),
      );
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final computer = shareComputer(AppScope.of(context));
    final groups = pipeGroupsIn(workspace, _projectId);
    final open = _open(workspace);
    final check = _check;
    final port = _portNumber;
    final caption = TextStyles.caption;
    return FormFrame(
      title: 'Share a website',
      body:
          'A website on $computer, such as a dashboard, that people on '
          'your pipes open in their own browser.',
      error: _error,
      submitLabel: 'Share',
      onSubmit: _busy || (check != null && check.problem.isNotEmpty)
          ? null
          : _share,
      onCancel: widget.close,
      dirty: _port.text.isNotEmpty || _name.text.isNotEmpty,
      fields: [
        Field(
          label: 'Port',
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SizedBox(
                width: 120,
                child: TextField(
                  controller: _port,
                  autofocus: !touchFirst(context),
                  placeholder: '3000',
                  kind: InputKind.number,
                  semanticsLabel: 'Port',
                ),
              ),
              if (check != null && check.problem.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(top: Space.xs + 2),
                  child: Text(
                    check.problem,
                    style: caption.copyWith(color: palette.danger),
                  ),
                )
              else if (check != null && !check.listening)
                Padding(
                  padding: const EdgeInsets.only(top: Space.xs + 2),
                  child: Text(
                    'Nothing on $computer answers on port ${check.port} '
                    'yet. You can share it now and start it later.',
                    style: caption.copyWith(color: palette.textMuted),
                  ),
                )
              // What answers there, so the person sees it's the program
              // they mean, on the computer named.
              else if (check != null && check.program.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(top: Space.xs + 2),
                  child: Text(
                    '${visibleCodes(check.program)}'
                    '${check.pid > 0 ? ' (process ${check.pid})' : ''} '
                    'answers on port ${check.port} on $computer.',
                    style: caption.copyWith(color: palette.textMuted),
                  ),
                ),
            ],
          ),
        ),
        Field(
          label: 'Name',
          note: 'What people on the pipes see it called.',
          child: TextField(controller: _name, placeholder: 'Dashboard'),
        ),
        Field(
          label: 'Share with',
          note: _target.isNotEmpty
              ? _groupNote(workspace, _target)
              : open.isEmpty
              ? 'No pipe in this project is open now.'
              : null,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (groups.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(bottom: Space.xs),
                  child: Select<String>(
                    value: _target,
                    title: 'Share with',
                    options: [
                      const SelectOption('', 'Pipes you choose'),
                      for (final group in groups)
                        SelectOption(group.id, 'The group “${group.name}”'),
                    ],
                    onChanged: (target) => setState(() {
                      _target = target;
                      _error = null;
                    }),
                  ),
                ),
              if (_target.isEmpty)
                for (final pipe in open)
                  Padding(
                    padding: const EdgeInsets.only(bottom: Space.xs),
                    child: _PipeLine(
                      pipe: pipe,
                      value: _pipes.contains(pipe.id),
                      onChanged: (on) => setState(() {
                        _error = null;
                        if (on) {
                          _pipes.add(pipe.id);
                        } else {
                          _pipes.remove(pipe.id);
                        }
                      }),
                    ),
                  ),
            ],
          ),
        ),
        Field(
          label: 'Ends',
          note: _lasts == 0
              ? 'It stays shared until you stop it.'
              : 'It stops being shared then. You can change this later in '
                    'the pipe’s settings.',
          child: Select<int>(
            value: _lasts,
            title: 'Ends',
            options: shareLengths,
            onChanged: (lasts) => setState(() => _lasts = lasts),
          ),
        ),
        Container(
          padding: const EdgeInsets.all(Space.s),
          decoration: BoxDecoration(
            color: palette.signal.withValues(alpha: 0.06),
            border: Border.all(color: palette.signal.withValues(alpha: 0.4)),
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Text(
            shareWarning(
              port == null ? '…' : '$port',
              computer: shareComputer(AppScope.of(context)),
            ),
            style: type.body.copyWith(color: palette.text),
          ),
        ),
      ],
    );
  }

  /// Under a group chosen: the pipes it reaches now, and that it follows
  /// the group.
  String _groupNote(Workspace workspace, String groupId) {
    final pipes = [
      for (final pipe in pipesIn(workspace, _projectId))
        if (pipe.groupId == groupId) pipe.name,
    ];
    final now = pipes.isEmpty
        ? 'No pipe is in it yet.'
        : 'Its pipes now: ${pipes.join(', ')}.';
    return '$now A pipe you move into the group gets it too, and one you '
        'move out stops getting it.';
  }
}

/// One open pipe to share with or not: its name and who's at its other
/// end, and a switch; a click anywhere on it turns it over.
class _PipeLine extends StatelessWidget {
  const _PipeLine({
    required this.pipe,
    required this.value,
    required this.onChanged,
  });

  final Pipe pipe;
  final bool value;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ToggleTap(
      value: value,
      onChanged: onChanged,
      child: Row(
        children: [
          Expanded(
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: pipe.name,
                    style: TextStyles.body.copyWith(color: palette.text),
                  ),
                  TextSpan(
                    text: ' · ${pipePeerName(pipe)}',
                    style: TextStyles.body.copyWith(color: palette.textMuted),
                  ),
                ],
              ),
            ),
          ),
          const SizedBox(width: Space.m),
          Toggle(value: value, onChanged: onChanged, label: pipe.name),
        ],
      ),
    );
  }
}

/// Shared websites, in [pipe]'s settings: each share on it, this side's
/// with Stop, the other side's with Hide or Show; and Share a website…
/// while it's open. Acts at once, apart from the form's Save, so it follows
/// the Claktory itself: the dialog it's in isn't rebuilt when the
/// workspace changes.
class PipeSharesField extends StatelessWidget {
  const PipeSharesField({super.key, required this.pipe});

  final Pipe pipe;

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => _build(context, store.workspace),
    );
  }

  Widget _build(BuildContext context, Workspace workspace) {
    var pipe = this.pipe;
    for (final now in workspace.pipes) {
      if (now.id == pipe.id) pipe = now;
    }
    final shares = sharesOn(workspace, pipe);
    // On Windows nothing is shared from here.
    final open = pipe.state == PipeState.PIPE_STATE_OPEN && !approvesElsewhere;
    if (shares.isEmpty && !open) return const SizedBox.shrink();
    return Field(
      label: 'Shared websites',
      note: shares.isEmpty
          ? 'Share a website on ${shareComputer(AppScope.of(context))}, '
                'such as a dashboard, with '
                '${pipePeerName(pipe)}.'
          : null,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final share in shares)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.s),
              child: _ShareLine(pipe: pipe, share: share),
            ),
          if (open)
            Align(
              alignment: Alignment.centerLeft,
              child: Button(
                label: 'Share a website…',
                onPressed: () => shareWebsite(context, pipe: pipe),
              ),
            ),
        ],
      ),
    );
  }
}

/// One share in a pipe's settings.
class _ShareLine extends StatelessWidget {
  const _ShareLine({required this.pipe, required this.share});

  final Pipe pipe;
  final PipeShare share;

  @override
  Widget build(BuildContext context) => ShareCountdown(
    share: share,
    builder: (context, now) => _build(context, now),
  );

  Widget _build(BuildContext context, DateTime now) {
    final palette = Palette.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final name = visibleCodes(share.name);
    final ends = shareEnds(share, now);
    final String title;
    final String note;
    if (share.mine) {
      title = '$name · port ${share.port}';
      String? group;
      for (final each in workspace.pipeGroups) {
        if (each.id == share.groupId) group = each.name;
      }
      final others = share.pipeIds.length - 1;
      final reach = group != null
          ? 'With the group “$group”'
          : others <= 0
          ? 'With this pipe'
          : 'With this pipe and $others other${others == 1 ? '' : 's'}';
      note = [
        reach,
        if (!share.live) 'Waits while its pipes are paused',
        ends ?? 'Until you stop it',
      ].join(' · ');
    } else {
      title = '$name, from ${pipePeerName(pipe)}';
      note = [
        share.hidden
            ? 'Hidden from the top of the pipe'
            : 'At the top of the pipe',
        ?ends,
      ].join(' · ');
    }
    return ActionLine(
      lead: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(title, style: TextStyles.body.copyWith(color: palette.text)),
          Text(
            note,
            style: TextStyles.caption.copyWith(color: palette.textMuted),
          ),
        ],
      ),
      gap: Space.m,
      action: share.mine && approvesElsewhere
          ? const SizedBox.shrink()
          : share.mine
          ? Row(
              mainAxisSize: MainAxisSize.min,
              spacing: Space.xs,
              children: [
                Button(
                  label: 'Change end…',
                  onPressed: () => changeShareEnd(context, share),
                ),
                Button(
                  label: 'Stop',
                  onPressed: () => stopShare(context, share),
                ),
              ],
            )
          : Button(
              label: share.hidden ? 'Show' : 'Hide',
              onPressed: () => fireAndLog(
                AppScope.of(context).hidePipeShare(share.id, !share.hidden),
              ),
            ),
    );
  }
}

/// Under [pipe]'s header: each website its other side shares with this
/// one and the person hasn't hidden, to open.
class PipeSharesStrip extends StatelessWidget {
  const PipeSharesStrip({super.key, required this.pipe});

  final Pipe pipe;

  @override
  Widget build(BuildContext context) {
    final workspace = AppScope.of(context).workspace.workspace;
    final shares = [
      for (final share in sharesOn(workspace, pipe))
        if (!share.mine && !share.hidden) share,
    ];
    if (shares.isEmpty) return const SizedBox.shrink();
    // At most [sharesStripShare] of the window's height, then it scrolls,
    // so however many it shares, the chat and its box stay in reach:
    // the row cut at its foot says there's more.
    return ConstrainedBox(
      constraints: BoxConstraints(
        maxHeight: math.max(
          sharesStripLeast,
          MediaQuery.sizeOf(context).height * sharesStripShare,
        ),
      ),
      child: SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (final share in shares)
              ReceivedShare(key: ValueKey(share.id), pipe: pipe, share: share),
          ],
        ),
      ),
    );
  }
}

/// The most of the window's height the received shares under a pipe's
/// header take before they scroll.
const sharesStripShare = 0.3;

/// ...and never less than this, in a short window.
const sharesStripLeast = 96.0;

/// Whether this Mac's browser is Safari, which can't open *.localhost
/// addresses; tests replace it.
Future<bool> Function() defaultBrowserIsSafari = _askSafari;

Future<bool> _askSafari() async {
  if (defaultTargetPlatform != TargetPlatform.macOS) return false;
  try {
    final id = await const MethodChannel('factory/device')
        .invokeMethod<String>('defaultBrowser');
    return id == 'com.apple.Safari' ||
        id == 'com.apple.SafariTechnologyPreview';
  } on PlatformException {
    return false;
  } on MissingPluginException {
    return false;
  }
}

/// Opens a share's page in the browser; tests replace it.
Future<bool> Function(Uri url) openSharePage = launchUrl;

/// Opens a share's page on Android:
/// in a Custom Tab that keeps Claktory running while it's on screen
/// ("custom_tab"), or in a browser that can't ("browser"); throws
/// PlatformException `no_browser` or `bad_url`. Tests replace it.
Future<String?> Function(Uri url) openShareTab = (url) =>
    const MethodChannel('factory/share-browser')
        .invokeMethod<String>('open', {'url': '$url'});

/// Opens a share's page on an iPhone or iPad
/// (ios/Runner/ShareBrowser.swift): in Safari's view over the app, at
/// the share's own address, or at [fallback] (127.0.0.1, the same port)
/// when the system can't find it on this device; "in_app", or throws
/// PlatformException `no_localhost`, `no_browser` or `bad_url`. Tests
/// replace it.
Future<String?> Function(Uri url, Uri? fallback) openShareOver =
    (url, fallback) => const MethodChannel('factory/share-browser')
        .invokeMethod<String>('open', {
          'url': '$url',
          if (fallback != null) 'fallback': '$fallback',
        });

/// A website [pipe]'s other side shares: who shares what, where it opens
/// (or why it can't now), and Open, Copy address and Hide.
class ReceivedShare extends StatefulWidget {
  const ReceivedShare({super.key, required this.pipe, required this.share});

  final Pipe pipe;
  final PipeShare share;

  @override
  State<ReceivedShare> createState() => _ReceivedShareState();
}

/// Opening a website the other side shares, and copying its address: the
/// share under the pipe's header and the chat's line announcing it (W2)
/// do it alike.
mixin _ShareOpening<W extends StatefulWidget> on State<W> {
  /// The share it opens.
  PipeShare get _share;

  /// Whether the browser is Safari, once known: then it opens at
  /// 127.0.0.1.
  var _safari = false;
  var _busy = false;
  var _copied = false;
  Timer? _copiedTimer;
  String? _error;

  /// Whether [_error] is the connection's: it goes once the Claktory is
  /// connected again.
  var _offline = false;

  /// Said under it once it opened in a browser that doesn't keep Claktory
  /// running (Android, W6).
  String? _note;

  /// Asks the browser again when the app comes back to the front: the
  /// person may have chosen another in System Settings meanwhile.
  late final _lifecycle = AppLifecycleListener(
    onResume: () {
      unawaited(_askBrowser());
      // Back from a browser over the app (Android's tab, a Mac's browser).
      _visitEnded();
    },
  );

  /// An Open's visit to the page: the share's
  /// last failure as it was at Open, so one that happened while the page
  /// was up (the site stopped, say, which the browser can't tell from a
  /// page that loaded) is said once the person is back; null with no
  /// visit under way.
  Int64? _visitFrom;
  var _visitSeen = false;
  var _visitBack = false;
  Timer? _visitTimer;

  void _visitStarted() {
    _visitFrom = _share.lastFailureMs;
    _visitSeen = false;
    _visitBack = false;
    _visitTimer?.cancel();
  }

  /// Back from the page: what failed meanwhile is said, now or as the
  /// Claktory's word of it comes in the next moments.
  void _visitEnded() {
    if (_visitFrom == null || _visitBack) return;
    _visitBack = true;
    _visitTimer = Timer(const Duration(seconds: 5), () => _visitFrom = null);
    _visitCheck();
  }

  /// Says the share's last failure once back, if it's newer than the
  /// visit; [rebuild] false where a build follows anyway.
  void _visitCheck({bool rebuild = true}) {
    final from = _visitFrom;
    if (from == null || !_visitBack || !mounted) return;
    final at = _share.lastFailureMs;
    if (at == 0 || at == from) return;
    _visitFrom = null;
    _visitTimer?.cancel();
    final why = _share.lastFailure;
    void say() => _show(
      why.isNotEmpty ? why : "${visibleCodes(_share.name)} didn't open.",
    );
    rebuild ? setState(say) : say();
  }

  @override
  void didUpdateWidget(W old) {
    super.didUpdateWidget(old);
    _visitCheck(rebuild: false);
  }

  Backend? _backend;

  @override
  void initState() {
    super.initState();
    _lifecycle;
    unawaited(_askBrowser());
    inAppShareBrowser.addListener(_pageChanged);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (identical(backend, _backend)) return;
    _backend?.connected.removeListener(_connectedChanged);
    backend.connected.addListener(_connectedChanged);
    _backend = backend;
  }

  @override
  void dispose() {
    _backend?.connected.removeListener(_connectedChanged);
    inAppShareBrowser.removeListener(_pageChanged);
    _lifecycle.dispose();
    _copiedTimer?.cancel();
    _visitTimer?.cancel();
    super.dispose();
  }

  void _connectedChanged() {
    if (_offline && _backend!.connected.value && mounted) {
      setState(() {
        _offline = false;
        _error = null;
      });
    }
  }

  /// A refused or lost request, for the person: while the connection is
  /// down, the Claktory by its name, never "the backend"; it goes once
  /// it's back.
  void _failed(RequestFailure failure) {
    if (!mounted) return;
    final backend = AppScope.of(context);
    final offline =
        !backend.connected.value ||
        failure.message == "Can't reach your Claktory right now." ||
        failure.message == 'Connection lost.';
    setState(() {
      _offline = offline;
      _error = offline
          ? "Can't reach ${claktoryName(backend)} right now."
          : failure.message;
    });
  }

  /// [error] in place of any before, not the connection's.
  void _show(String? error) {
    _offline = false;
    _error = error;
  }

  /// Whether the browser is Safari now, kept for the address shown.
  Future<bool> _askBrowser() async {
    final safari = await defaultBrowserIsSafari();
    if (mounted && safari != _safari) setState(() => _safari = safari);
    return safari;
  }

  /// The address this device's browser opens it at: on a computer showing
  /// a Claktory on another machine, at the port this device forwards it
  /// from; null there while it isn't forwarded.
  String? _address(LoopbackForwards forwards) {
    final share = _share;
    final address = _safari && share.fallbackAddress.isNotEmpty
        ? share.fallbackAddress
        : share.address;
    if (!forwards.remote()) return address;
    final port = forwards.sharePort(share.id);
    return port == null ? null : withPort(address, port);
  }

  /// Has the Claktory serve it (its listener starts at the first Open)
  /// and, from another machine, this device forward it; the address to
  /// open, its one-time code in it.
  Future<String> _serve(Backend backend) async {
    final safari = await _askBrowser();
    final opened = await backend.openPipeShare(_share.id);
    final url = safari && opened.fallbackUrl.isNotEmpty
        ? opened.fallbackUrl
        : opened.url;
    if (!backend.forwards.remote()) return url;
    // Through the tunnel, from a port of this device's own to the share's
    // there: the share's own host, at this device's port.
    final port = await backend.forwards.forwardShare(
      _share.id,
      Uri.parse(opened.url).port,
    );
    _fallbackHere = opened.fallbackUrl.isEmpty
        ? null
        : withPort(opened.fallbackUrl, port);
    return withPort(url, port);
  }

  /// The address [_serve] opened's fallback (127.0.0.1) at this device's
  /// port: where an iPhone or iPad opens it when the system can't find the
  /// share's own address.
  String? _fallbackHere;

  Future<void> _open() async {
    setState(() {
      _busy = true;
      _show(null);
    });
    _visitStarted();
    try {
      final backend = AppScope.of(context);
      final url = await _serve(backend);
      if (defaultTargetPlatform == TargetPlatform.android &&
          backend.forwards.remote()) {
        await _openTab(backend, Uri.parse(url));
      } else if (defaultTargetPlatform == TargetPlatform.iOS &&
          backend.forwards.remote()) {
        await _openInApp(backend, Uri.parse(url));
      } else if (!await openSharePage(Uri.parse(url)) && mounted) {
        setState(() => _show("Your browser didn't open. Try again."));
      }
    } on RequestFailure catch (failure) {
      _failed(failure);
    } on ForwardFailure catch (failure) {
      if (mounted) setState(() => _show(_forwardProblem(failure)));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// Android (W6): a tab over Claktory, which keeps Claktory's connection,
  /// and so the forward, up while it's on screen; the Claktory serving it is
  /// the one that stays up.
  Future<void> _openTab(Backend backend, Uri url) async {
    final opening = shareBrowserOpening(backend);
    try {
      final how = await openShareTab(url);
      if (how != 'custom_tab') {
        shareBrowserOpenFailed(opening);
        if (mounted) setState(() => _note = shareBrowserNote);
      } else if (mounted) {
        setState(() => _note = null);
      }
    } on PlatformException catch (error) {
      shareBrowserOpenFailed(opening);
      if (mounted) {
        setState(
          () => _show(
            error.code == 'no_browser'
                ? "There's no browser on this $thisDeviceKind to open it in."
                : "${visibleCodes(_share.name)} can't open on this "
                      '$thisDeviceKind.',
          ),
        );
      }
    }
  }

  /// iPhone and iPad (ios/Runner/ShareBrowser.swift): Safari's view
  /// over the app, which stays in front, its connection and the forward
  /// with it, until Done brings the person back here.
  Future<void> _openInApp(Backend backend, Uri url) async {
    final id = _share.id;
    inAppShareBrowser.opening(backend, id);
    try {
      final fallback = _fallbackHere;
      final how = await openShareOver(
        url,
        fallback == null ? null : Uri.parse(fallback),
      );
      if (how != 'in_app') inAppShareBrowser.openFailed(backend, id);
      if (mounted) setState(() => _note = null);
    } on PlatformException catch (error) {
      inAppShareBrowser.openFailed(backend, id);
      if (!mounted) return;
      // The system can't find a shared website's own address on this
      // network (iOS before 26, WebKit bug 160504) and there was no
      // fallback to open instead: said, and nothing opens.
      setState(
        () => _show(
          error.code == 'no_localhost'
              ? "This $thisDeviceKind can't open shared websites on this "
                    'network yet.'
              : "${visibleCodes(_share.name)} can't open on this "
                    '$thisDeviceKind.',
        ),
      );
    }
  }

  /// Back from its page (Done) after the page didn't load: why, plainly,
  /// under it, as an Open that failed here would say it.
  void _pageChanged() {
    final backend = _backend;
    if (backend == null || !mounted) return;
    // Its page on screen, then gone (Done): what failed meanwhile.
    if (inAppShareBrowser.shows(backend, _share.id)) {
      _visitSeen = true;
    } else if (_visitSeen) {
      _visitEnded();
    }
    if (!inAppShareBrowser.takeFailure(backend, _share.id)) return;
    final error = backend.forwards.shareLastError(_share.id);
    if (!backend.connected.value) {
      _failed(const RequestFailure('Connection lost.'));
    } else {
      setState(
        () => _show(
          error.isNotEmpty
              ? error
              : "${visibleCodes(_share.name)} didn't open. Try again.",
        ),
      );
    }
  }

  /// Why this device can't forward it, for the person.
  String _forwardProblem(ForwardFailure failure) {
    final machine = claktoryName(AppScope.of(context));
    return switch (failure.problem) {
      ForwardProblem.taken =>
        'No port is free for it on this $thisDeviceKind. Quit an app you '
            "don't need, then try again.",
      ForwardProblem.unsupported =>
        '$machine needs an update before this $thisDeviceKind can open it.',
      ForwardProblem.failed =>
        failure.message.isEmpty
            ? "Can't reach $machine right now. Try again."
            : failure.message,
    };
  }

  Future<void> _copy() async {
    final backend = AppScope.of(context);
    var address = _address(backend.forwards);
    if (address == null) {
      // Not forwarded here yet: served and forwarded first, so the address
      // copied works on this device.
      setState(() {
        _busy = true;
        _show(null);
      });
      try {
        await _serve(backend);
        if (mounted) address = _address(backend.forwards);
      } on RequestFailure catch (failure) {
        _failed(failure);
      } on ForwardFailure catch (failure) {
        if (mounted) setState(() => _show(_forwardProblem(failure)));
      } finally {
        if (mounted) setState(() => _busy = false);
      }
      if (address == null) return;
    }
    await Clipboard.setData(ClipboardData(text: address));
    if (!mounted) return;
    setState(() => _copied = true);
    _copiedTimer?.cancel();
    _copiedTimer = Timer(const Duration(seconds: 2), () {
      if (mounted) setState(() => _copied = false);
    });
  }
}

class _ReceivedShareState extends State<ReceivedShare>
    with _ShareOpening<ReceivedShare> {
  @override
  PipeShare get _share => widget.share;

  @override
  Widget build(BuildContext context) {
    final forwards = AppScope.of(context).forwards;
    return ListenableBuilder(
      listenable: forwards,
      builder: (context, _) => ShareCountdown(
        share: widget.share,
        builder: (context, now) => _build(context, forwards, now),
      ),
    );
  }

  Widget _build(BuildContext context, LoopbackForwards forwards, DateTime now) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final share = widget.share;
    // On a computer showing a Claktory on another machine it opens through
    // this one, and on a phone or tablet over the app (W6).
    // A phone or tablet: no address to copy, which would work only while
    // Claktory runs (W6).
    final touch = forwards.remote() && sharesOnTouchDevice;
    final address = _address(forwards);
    final forwardProblem = forwards.shareProblem(share.id);
    final problem = share.listenerProblem;
    final (String line, Color color) = !share.live
        ? ('Waits while the pipe is paused.', palette.textMuted)
        : problem.isNotEmpty
        ? (problem, palette.danger)
        : forwardProblem != null
        ? (_forwardProblem(forwardProblem), palette.danger)
        : touch
        ? (_note ?? '', palette.textMuted)
        : address == null
        ? ('', palette.textMuted)
        : (
            _safari
                ? '${visibleCodes(address)} (Safari can’t open the usual '
                      'address)'
                : visibleCodes(address),
            palette.textMuted,
          );
    final usable = share.live;
    // Not opened yet from this Claktory (W2): it stands out, in the pipe
    // card's wash and border, until it is.
    final fresh = share.unopened && share.live;
    return Container(
      margin: fresh
          ? const EdgeInsets.fromLTRB(Space.l, Space.s, Space.l, Space.s)
          : EdgeInsets.zero,
      padding: EdgeInsets.symmetric(
        horizontal: fresh ? Space.l : Space.xl,
        vertical: Space.m,
      ),
      decoration: fresh
          ? BoxDecoration(
              color: pipeColor.withValues(alpha: 0.06),
              border: Border.all(color: pipeColor.withValues(alpha: 0.55)),
              borderRadius: BorderRadius.circular(Radii.panel),
            )
          : BoxDecoration(
              border: Border(bottom: BorderSide(color: palette.hairline)),
            ),
      child: ActionLine(
        lead: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          spacing: 2,
          children: [
            Text(
              '${pipePeerName(widget.pipe, start: true)} shares '
              '${visibleCodes(share.name)}',
              style: type.body.copyWith(
                color: palette.text,
                fontWeight: FontWeight.w600,
              ),
            ),
            if (line.isNotEmpty)
              Text(line, style: type.caption.copyWith(color: color)),
            if (shareEnds(share, now) case final ends?)
              Text(
                ends,
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            // Said once: Open's failure is often the forward's problem the
            // line already says.
            if (_error case final error? when error != line)
              Text(error, style: type.caption.copyWith(color: palette.danger)),
            // Opening it is the person's (OpenPipeShare).
            if (approvesElsewhere)
              const ApproveElsewhere(text: openShareElsewhereText),
          ],
        ),
        action: Wrap(
          spacing: Space.xs,
          runSpacing: Space.xs,
          alignment: WrapAlignment.end,
          children: [
            if (!approvesElsewhere)
              Button(
                label: 'Open',
                primary: true,
                onPressed: usable && !_busy ? _open : null,
              ),
            if (!touch && !approvesElsewhere)
              Button(
                label: _copied ? 'Copied' : 'Copy address',
                onPressed: usable && !_busy ? _copy : null,
              ),
            Button(
              label: 'Hide',
              onPressed: () =>
                  fireAndLog(backend.hidePipeShare(share.id, true)),
            ),
          ],
        ),
      ),
    );
  }
}

/// The pipe chat's line announcing a website the other side shares (W2,
/// PipeMessage.share_id): Claktory's [line], then Open and Copy address,
/// as under the pipe's header, while the share lasts and isn't hidden;
/// once it's gone, the line alone.
class ShareAnnouncement extends StatelessWidget {
  const ShareAnnouncement({
    super.key,
    required this.shareId,
    required this.line,
  });

  final String shareId;
  final Widget line;

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        final share = store.workspace.pipeShares
            .where((share) => share.id == shareId)
            .firstOrNull;
        if (share == null || share.mine || share.hidden || !share.live) {
          return line;
        }
        return _AnnouncedShare(
          key: ValueKey(share.id),
          share: share,
          line: line,
        );
      },
    );
  }
}

class _AnnouncedShare extends StatefulWidget {
  const _AnnouncedShare({super.key, required this.share, required this.line});

  final PipeShare share;
  final Widget line;

  @override
  State<_AnnouncedShare> createState() => _AnnouncedShareState();
}

class _AnnouncedShareState extends State<_AnnouncedShare>
    with _ShareOpening<_AnnouncedShare> {
  @override
  PipeShare get _share => widget.share;

  @override
  Widget build(BuildContext context) {
    final forwards = AppScope.of(context).forwards;
    return ListenableBuilder(
      listenable: forwards,
      builder: (context, _) {
        final palette = Palette.of(context);
        final touch = forwards.remote() && sharesOnTouchDevice;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.center,
          children: [
            widget.line,
            const SizedBox(height: Space.xs),
            if (approvesElsewhere)
              const ApproveElsewhere(text: openShareElsewhereText)
            else
              Wrap(
                spacing: Space.xs,
                runSpacing: Space.xs,
                alignment: WrapAlignment.center,
                children: [
                  Button(
                    label: 'Open',
                    primary: true,
                    onPressed: _busy ? null : _open,
                  ),
                  if (!touch)
                    Button(
                      label: _copied ? 'Copied' : 'Copy address',
                      onPressed: _busy ? null : _copy,
                    ),
                ],
              ),
            if (_error case final error?)
              Padding(
                padding: const EdgeInsets.only(top: Space.xs),
                child: Text(
                  error,
                  textAlign: TextAlign.center,
                  style: TextStyles.caption.copyWith(color: palette.danger),
                ),
              ),
          ],
        );
      },
    );
  }
}

/// Under a share opened on Android in a browser that doesn't keep Claktory
/// running (W6).
const shareBrowserNote = 'It stops loading when you leave Claktory.';
