import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'extensions.dart' show logTime;
import 'identity.dart';
import 'mobile/share.dart' show canShare, globalRectOf, shareText;
import 'pipes.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart' show SelectableArea;
import 'state.dart' show WorkspaceStore;

/// What an agent's card suggesting a pipe asks, in a line (Needs you).
String pipeStartSummary(PipeStart start) =>
    start.kind == PipeStartKind.PIPE_START_KIND_ACCEPT
    ? 'Join a pipe'
    : 'Invite someone to a pipe';

/// An agent (Rivet, say) suggesting a pipe, in its conversation (PipeStart):
/// for an invite, the project, name and purpose it suggests, for the person
/// to check before Make invite, then the code, shown only here and only
/// once; for a code someone sent, a field to paste it in, then who the
/// code says it's from, kept apart from the name the person gives the pipe
/// (nothing in a code is verified), and Connect. Either way both people
/// then approve the pipe in its view. Dismiss closes it unused.
class PipeStartCard extends StatefulWidget {
  const PipeStartCard(this.start, {super.key});

  final PipeStart start;

  @override
  State<PipeStartCard> createState() => _PipeStartCardState();
}

class _PipeStartCardState extends State<PipeStartCard> {
  PipeStart get start => widget.start;

  bool get _accept => start.kind == PipeStartKind.PIPE_START_KIND_ACCEPT;

  bool get _waiting => start.state == PipeStartState.PIPE_START_STATE_WAITING;

  /// The project chosen; the suggested one until the person picks another.
  late String _project = start.projectId;
  late final _name = TextEditingController(text: start.name);
  late final _purpose = TextEditingController(text: start.purpose);
  final _code = TextEditingController();

  /// What the pasted code says, once read.
  PipeInvitePreview? _preview;

  /// The invite made from this card: its code shows only while the card
  /// does, never kept anywhere.
  PipeInvite? _invite;
  var _copied = false;
  var _busy = false;
  String? _error;

  @override
  void dispose() {
    _name.dispose();
    _purpose.dispose();
    _code.dispose();
    super.dispose();
  }

  Future<void> _run(Future<void> Function(Backend backend) request) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await request(AppScope.of(context));
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _makeInvite() => _run((backend) async {
    final invite = await backend.createPipeInvite(
      _project,
      _name.text.trim(),
      _purpose.text.trim(),
      startId: start.id,
    );
    if (mounted) setState(() => _invite = invite);
  });

  void _read() => _run((backend) async {
    final preview = await backend.readPipeInvite(_code.text.trim());
    if (!mounted) return;
    setState(() => _preview = preview);
  });

  void _connect() => _run((backend) async {
    final preview = _preview;
    if (preview == null) return;
    // The backend opens the new pipe itself, where both approve.
    await backend.acceptPipeInvite(
      preview.code,
      _project,
      name: _name.text.trim(),
      purpose: _purpose.text.trim(),
      startId: start.id,
    );
  });

  void _dismiss() =>
      _run((backend) => backend.dismissPipeStart(start.agentId, start.id));

  Future<void> _copy(String code) async {
    await Clipboard.setData(ClipboardData(text: code));
    if (mounted) setState(() => _copied = true);
  }

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => _card(context, store),
    );
  }

  Widget _card(BuildContext context, WorkspaceStore store) {
    final palette = Palette.of(context);
    final agent = store.agent(start.agentId);
    final name = agent?.name ?? start.agentId;
    final open = _waiting || _invite != null;
    return Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        color: pipeColor.withValues(alpha: 0.06),
        border: Border.all(
          color: pipeColor.withValues(alpha: open ? 0.55 : 0.25),
        ),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              if (agent != null) ...[
                AgentCharacter(agent: agent, size: 20),
                const SizedBox(width: Space.xs + 2),
              ],
              const PortalGlyph(size: 14),
              const SizedBox(width: Space.xs),
              Flexible(
                child: Text(
                  _accept
                      ? '$name suggests joining a pipe with a code you were '
                            'sent'
                      : '$name suggests inviting someone to a pipe',
                  style: TextStyles.caption.copyWith(
                    color: pipeColor,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.m),
          ..._body(context, store, palette),
        ],
      ),
    );
  }

  List<Widget> _body(
    BuildContext context,
    WorkspaceStore store,
    Palette palette,
  ) {
    if (_invite case final invite?) return _made(context, store, invite);
    if (!_waiting) return [_ended(context, store, palette)];
    if (!_accept) {
      return [
        ..._fields(store, theySee: true),
        _actions(palette, [
          _dismissButton(),
          ListenableBuilder(
            listenable: _name,
            builder: (context, _) => Button(
              label: 'Make invite',
              primary: true,
              onPressed: _busy || _name.text.trim().isEmpty || _project.isEmpty
                  ? null
                  : _makeInvite,
            ),
          ),
        ]),
      ];
    }
    final preview = _preview;
    if (preview == null) {
      return [
        Field(
          label: 'Code',
          note: 'Paste the code you were sent.',
          child: TextField(
            controller: _code,
            placeholder: 'Paste code',
            autocorrect: false,
            textCapitalization: TextCapitalization.none,
            onSubmitted: (_) => _code.text.trim().isEmpty ? null : _read(),
          ),
        ),
        _actions(palette, [
          _dismissButton(),
          ListenableBuilder(
            listenable: _code,
            builder: (context, _) => Button(
              label: 'Continue',
              primary: true,
              onPressed: _busy || _code.text.trim().isEmpty ? null : _read,
            ),
          ),
        ]),
      ];
    }
    return [
      _TheySay(preview: preview),
      const SizedBox(height: Space.m),
      ..._fields(store, theySee: false, peer: preview.peer),
      _actions(palette, [
        Button(
          label: 'Back',
          onPressed: _busy
              ? null
              : () => setState(() {
                  _preview = null;
                  _error = null;
                }),
        ),
        ListenableBuilder(
          listenable: _name,
          builder: (context, _) => Button(
            label: 'Connect',
            primary: true,
            onPressed: _busy || _project.isEmpty ? null : _connect,
          ),
        ),
      ]),
    ];
  }

  /// The project, the person's name for the pipe and what it's for.
  List<Widget> _fields(
    WorkspaceStore store, {
    required bool theySee,
    PipePeer? peer,
  }) {
    final projects = [
      for (final project in store.workspace.projects)
        if (!project.archived) project,
    ];
    if (!projects.any((project) => project.id == _project)) {
      _project = projects.isEmpty ? '' : projects.first.id;
    }
    final them = peer == null || peer.ownerName.isEmpty
        ? 'The other person sees'
        : '${peer.ownerName} sees';
    return [
      Field(
        label: 'Project',
        child: Select<String>(
          value: _project,
          options: [
            for (final project in projects)
              SelectOption(project.id, project.name),
          ],
          onChanged: (project) => setState(() => _project = project),
        ),
      ),
      Field(
        label: 'Your name for the pipe',
        note: theySee
            ? 'The other person sees it before they accept.'
            : 'How it shows in your sidebar. You can change it.',
        child: TextField(controller: _name),
      ),
      Field(
        label: 'What it’s for',
        note: theySee
            ? 'One line. The other person sees it before they accept.'
            : '$them this.',
        child: TextField(controller: _purpose),
      ),
    ];
  }

  Widget _dismissButton() =>
      Button(label: 'Dismiss', onPressed: _busy ? null : _dismiss);

  /// The error, if any, then the buttons at the end; they wrap under each
  /// other where the card is narrow.
  Widget _actions(Palette palette, List<Widget> buttons) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      if (_error case final error?)
        Padding(
          padding: const EdgeInsets.only(bottom: Space.s),
          child: Text(
            error,
            style: TextStyles.caption.copyWith(color: palette.danger),
          ),
        ),
      Wrap(
        alignment: WrapAlignment.end,
        spacing: Space.s,
        runSpacing: Space.s,
        children: buttons,
      ),
    ],
  );

  /// The invite just made: its code, to copy and send privately, and the
  /// pipe it waits in.
  List<Widget> _made(
    BuildContext context,
    WorkspaceStore store,
    PipeInvite invite,
  ) {
    final palette = Palette.of(context);
    final pipe = store.workspace.pipes
        .where((pipe) => pipe.id == invite.pipeId)
        .firstOrNull;
    final expires = pipe?.inviteExpiresMs.toInt() ?? 0;
    final share = canShare(context);
    return [
      Text(
        'Send this code privately to the person you’re inviting. They paste '
        'it into Claktory.',
        style: TextStyles.body.copyWith(color: palette.textMuted),
      ),
      const SizedBox(height: Space.s),
      Container(
        padding: const EdgeInsets.all(Space.m),
        decoration: BoxDecoration(
          color: palette.canvas,
          border: Border.all(color: palette.hairline),
          borderRadius: BorderRadius.circular(Radii.control),
        ),
        child: SelectableArea(
          child: Text(
            invite.code,
            style: TextStyles.caption.copyWith(
              fontFamily: TextStyles.mono,
              color: palette.text,
            ),
          ),
        ),
      ),
      const SizedBox(height: Space.xs),
      Text(
        expires > 0
            ? 'It works once, until '
                  '${logTime(DateTime.fromMillisecondsSinceEpoch(expires))}. '
                  'It won’t show again, so copy it now.'
            : 'It works once. It won’t show again, so copy it now.',
        style: TextStyles.caption.copyWith(color: palette.textFaint),
      ),
      const SizedBox(height: Space.m),
      _actions(palette, [
        if (pipe != null)
          Button(label: 'Open pipe', onPressed: () => openPipe(context, pipe)),
        Button(
          label: _copied ? 'Copied' : 'Copy code',
          primary: !share,
          onPressed: () => _copy(invite.code),
        ),
        // Where there's a share sheet, it's the way to send the code, last.
        if (share)
          Builder(
            builder: (button) => Button(
              label: 'Share',
              primary: true,
              onPressed: () =>
                  shareText(context, invite.code, from: globalRectOf(button)),
            ),
          ),
      ]),
    ];
  }

  /// How it ended, and when; the pipe it made, to open.
  Widget _ended(BuildContext context, WorkspaceStore store, Palette palette) {
    final done = start.state == PipeStartState.PIPE_START_STATE_DONE;
    final pipe = done
        ? store.workspace.pipes
              .where((pipe) => pipe.id == start.pipeId)
              .firstOrNull
        : null;
    final text = done ? (_accept ? 'Connected' : 'Invite made') : 'Dismissed';
    final ended = start.endedMs.toInt();
    return Row(
      children: [
        Expanded(
          child: Text.rich(
            TextSpan(
              children: [
                TextSpan(
                  text: pipe == null ? text : '$text: ${pipe.name}',
                  style: TextStyle(
                    color: done ? pipeColor : palette.textMuted,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                if (ended > 0)
                  TextSpan(
                    text:
                        ' · ${logTime(DateTime.fromMillisecondsSinceEpoch(ended))}',
                    style: TextStyle(color: palette.textFaint),
                  ),
              ],
            ),
            style: TextStyles.caption,
          ),
        ),
        if (pipe != null) ...[
          const SizedBox(width: Space.s),
          Button(label: 'Open pipe', onPressed: () => openPipe(context, pipe)),
        ],
      ],
    );
  }
}

/// What a pasted code says about whoever sent it, in a box of its own,
/// marked "Not verified": nothing in a code can be checked, so none of it
/// mixes with the name the person gives the pipe.
class _TheySay extends StatelessWidget {
  const _TheySay({required this.preview});

  final PipeInvitePreview preview;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final peer = preview.peer;
    final who = [
      if (peer.ownerName.isNotEmpty) peer.ownerName,
      if (peer.ownerEmail.isNotEmpty) peer.ownerEmail,
      if (peer.claktoryName.isNotEmpty) peer.claktoryName,
    ].join(' · ');
    return Container(
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        color: palette.canvas,
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  'The code says',
                  style: TextStyles.caption.copyWith(color: palette.textFaint),
                ),
              ),
              Container(
                padding: const EdgeInsets.symmetric(horizontal: Space.xs + 2),
                decoration: BoxDecoration(
                  border: Border.all(color: palette.hairline),
                  borderRadius: BorderRadius.circular(Radii.control),
                ),
                child: Text(
                  'Not verified',
                  style: TextStyles.caption.copyWith(color: palette.textMuted),
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.xs),
          Text(
            who.isEmpty ? 'From someone who didn’t say' : 'From $who',
            style: TextStyles.body.copyWith(color: palette.text),
          ),
          if (peer.projectName.isNotEmpty || preview.name.isNotEmpty)
            Text(
              [
                if (peer.projectName.isNotEmpty) 'Project ${peer.projectName}',
                if (preview.name.isNotEmpty) 'pipe “${preview.name}”',
              ].join(', '),
              style: TextStyles.caption.copyWith(color: palette.textMuted),
            ),
          if (peer.purpose.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            Text(
              '“${peer.purpose}”',
              style: TextStyles.body.copyWith(color: palette.textMuted),
            ),
          ],
        ],
      ),
    );
  }
}
