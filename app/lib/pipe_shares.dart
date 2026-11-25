// Websites shared over pipes (pipe-port-sharing.md, its revised plan): a
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
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import 'backend.dart';
import 'controls.dart';
import 'forms.dart' show FormFrame;
import 'identity.dart';
import 'mobile/input.dart' show InputKind;
import 'mobile/lines.dart' show ActionLine;
import 'mobile/metrics.dart' show TypeScale;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'pipe_groups.dart' show pipeGroupsIn;
import 'pipes.dart' show pipePeerName, pipesIn;
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
/// pipes get, naming the port, since another program can take it later.
String shareWarning(String port) =>
    'People on these pipes can use everything this computer serves on '
    'port $port.';

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
}) => showModal(
  context,
  (context, close) => ShareWebsiteForm(pipe: pipe, group: group, close: close),
);

/// Stops [share] once the person says so.
Future<void> stopShare(BuildContext context, PipeShare share) async {
  final backend = AppScope.of(context);
  final sure = await showActions<bool>(
    context,
    title: 'Stop sharing ${visibleCodes(share.name)}?',
    message:
        'People on its pipes can no longer open it. Shared again, it gets '
        'a new address.',
    actions: const [SheetAction('Stop sharing', true, destructive: true)],
  );
  if (sure != true || !context.mounted) return;
  try {
    await backend.stopPipeShare(share.id);
  } on RequestFailure catch (failure) {
    if (!context.mounted) return;
    await showActions<void>(
      context,
      title: "Couldn't stop sharing ${visibleCodes(share.name)}",
      message: failure.message,
      actions: const [],
      cancelLabel: 'OK',
    );
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
    final groups = pipeGroupsIn(workspace, _projectId);
    final open = _open(workspace);
    final check = _check;
    final port = _portNumber;
    final caption = TextStyles.caption;
    return FormFrame(
      title: 'Share a website',
      body:
          'A website on this computer, such as a dashboard, that people on '
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
                    'Nothing on this computer answers on port ${check.port} '
                    'yet. You can share it now and start it later.',
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
        Container(
          padding: const EdgeInsets.all(Space.s),
          decoration: BoxDecoration(
            color: palette.signal.withValues(alpha: 0.06),
            border: Border.all(color: palette.signal.withValues(alpha: 0.4)),
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Text(
            shareWarning(port == null ? '…' : '$port'),
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
/// workspace changes (Abbacchio's row 1111).
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
    final open = pipe.state == PipeState.PIPE_STATE_OPEN;
    if (shares.isEmpty && !open) return const SizedBox.shrink();
    return Field(
      label: 'Shared websites',
      note: shares.isEmpty
          ? 'Share a website on this computer, such as a dashboard, with '
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
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final name = visibleCodes(share.name);
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
      note = share.live ? reach : '$reach · Waits while its pipes are paused';
    } else {
      title = '$name, from ${pipePeerName(pipe)}';
      note = share.hidden
          ? 'Hidden from the top of the pipe'
          : 'At the top of the pipe';
    }
    return Row(
      children: [
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(title, style: TextStyles.body.copyWith(color: palette.text)),
              Text(
                note,
                style: TextStyles.caption.copyWith(color: palette.textMuted),
              ),
            ],
          ),
        ),
        const SizedBox(width: Space.m),
        if (share.mine)
          Button(label: 'Stop', onPressed: () => stopShare(context, share))
        else
          Button(
            label: share.hidden ? 'Show' : 'Hide',
            onPressed: () => fireAndLog(
              AppScope.of(context).hidePipeShare(share.id, !share.hidden),
            ),
          ),
      ],
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
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final share in shares)
          ReceivedShare(key: ValueKey(share.id), pipe: pipe, share: share),
      ],
    );
  }
}

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

/// A website [pipe]'s other side shares: who shares what, where it opens
/// (or why it can't now), and Open, Copy address and Hide.
class ReceivedShare extends StatefulWidget {
  const ReceivedShare({super.key, required this.pipe, required this.share});

  final Pipe pipe;
  final PipeShare share;

  @override
  State<ReceivedShare> createState() => _ReceivedShareState();
}

class _ReceivedShareState extends State<ReceivedShare> {
  /// Whether the browser is Safari, once known: then it opens at
  /// 127.0.0.1.
  var _safari = false;
  var _busy = false;
  var _copied = false;
  Timer? _copiedTimer;
  String? _error;

  /// Asks the browser again when the app comes back to the front: the
  /// person may have chosen another in System Settings meanwhile.
  late final _lifecycle = AppLifecycleListener(
    onResume: () => unawaited(_askBrowser()),
  );

  @override
  void initState() {
    super.initState();
    _lifecycle;
    unawaited(_askBrowser());
  }

  @override
  void dispose() {
    _lifecycle.dispose();
    _copiedTimer?.cancel();
    super.dispose();
  }

  /// Whether the browser is Safari now, kept for the address shown.
  Future<bool> _askBrowser() async {
    final safari = await defaultBrowserIsSafari();
    if (mounted && safari != _safari) setState(() => _safari = safari);
    return safari;
  }

  /// The address this Mac's browser opens it at.
  String get _address {
    final share = widget.share;
    return _safari && share.fallbackAddress.isNotEmpty
        ? share.fallbackAddress
        : share.address;
  }

  Future<void> _open() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final backend = AppScope.of(context);
      final safari = await _askBrowser();
      final opened = await backend.openPipeShare(widget.share.id);
      final url = safari && opened.fallbackUrl.isNotEmpty
          ? opened.fallbackUrl
          : opened.url;
      if (!await openSharePage(Uri.parse(url)) && mounted) {
        setState(() => _error = "Your browser didn't open. Try again.");
      }
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _copy() async {
    await Clipboard.setData(ClipboardData(text: _address));
    if (!mounted) return;
    setState(() => _copied = true);
    _copiedTimer?.cancel();
    _copiedTimer = Timer(const Duration(seconds: 2), () {
      if (mounted) setState(() => _copied = false);
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final share = widget.share;
    // Opened only on the computer the Claktory runs on, for now: its
    // address is that computer's.
    final here = !backend.forwards.remote();
    final problem = share.listenerProblem;
    final (String line, Color color) = !share.live
        ? ('Waits while the pipe is paused.', palette.textMuted)
        : !here
        ? ('Open it on the computer your Claktory runs on.', palette.textMuted)
        : problem.isNotEmpty
        ? (problem, palette.danger)
        : (
            _safari
                ? '${visibleCodes(_address)} (Safari can’t open the usual '
                      'address)'
                : visibleCodes(_address),
            palette.textMuted,
          );
    final usable = share.live && here;
    return Container(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.xl,
        vertical: Space.m,
      ),
      decoration: BoxDecoration(
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
            if (_error case final error?)
              Text(error, style: type.caption.copyWith(color: palette.danger)),
          ],
        ),
        action: Wrap(
          spacing: Space.xs,
          runSpacing: Space.xs,
          alignment: WrapAlignment.end,
          children: [
            if (here) ...[
              Button(
                label: 'Open',
                primary: true,
                onPressed: usable && !_busy ? _open : null,
              ),
              Button(
                label: _copied ? 'Copied' : 'Copy address',
                onPressed: usable ? _copy : null,
              ),
            ],
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
