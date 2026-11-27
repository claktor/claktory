import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'approve_elsewhere.dart';
import 'backend.dart';
import 'card_answer.dart' show CardAnswer, PublishesAnswer, noComment;
import 'characters/character.dart';
import 'controls.dart';
import 'decision.dart';
import 'devices.dart' show QrCode;
import 'extensions.dart' show logTime;
import 'identity.dart';
import 'mobile/share.dart' show canShare, globalRectOf, shareText;
import 'people/messages_setup.dart' show secretClipboard;
import 'people/secret_clipboard.dart' show clearWhenDue;
import 'pipes.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scan_code.dart';
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

class _PipeStartCardState extends State<PipeStartCard> with PublishesAnswer {
  PipeStart get start => widget.start;

  bool get _accept => start.kind == PipeStartKind.PIPE_START_KIND_ACCEPT;

  bool get _waiting => start.state == PipeStartState.PIPE_START_STATE_WAITING;

  /// The project chosen; the suggested one until the person picks another.
  late String _project = start.projectId;
  late final _name = TextEditingController(text: start.name);
  late final _purpose = TextEditingController(text: start.purpose);
  final _code = TextEditingController();

  /// A comment for the agent, sent with Dismiss.
  final _comment = TextEditingController();

  /// What the pasted code says, once read.
  PipeInvitePreview? _preview;

  /// The invite made from this card: its code shows only while the card
  /// does, never kept anywhere.
  PipeInvite? _invite;
  var _copied = false;

  /// The invite's code shows as a QR too, to scan in person.
  var _qr = false;

  /// A code can be scanned here, with the camera (friend bug #2).
  var _canScan = false;
  var _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    if (_accept) {
      canOfferScan(context).then((can) {
        if (mounted && can) setState(() => _canScan = true);
      });
    }
  }

  /// Scans the code someone sent, then reads it as a pasted one is.
  Future<void> _scan() async {
    final code = await scanCode(
      context,
      title: 'Scan a pipe invite',
      check: pipeInviteCheck,
      onPasteInstead: pasteCodeInto(_code, then: _read),
    );
    if (code == null || !mounted) return;
    _code.text = code;
    _read();
  }

  @override
  void dispose() {
    _name.dispose();
    _purpose.dispose();
    _code.dispose();
    _comment.dispose();
    super.dispose();
  }

  /// [rethrowing] for focus mode's keys (#514): refused, it throws too.
  Future<void> _run(
    Future<void> Function(Backend backend) request, {
    bool rethrowing = false,
  }) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await request(AppScope.of(context));
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
      if (rethrowing) rethrow;
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// Make invite is on its way: the dock stays on the card for its code.
  var _making = false;

  Future<void> _makeInvite({bool rethrowing = false}) async {
    setState(() => _making = true);
    try {
      await _createInvite(rethrowing: rethrowing);
    } finally {
      if (mounted) setState(() => _making = false);
    }
  }

  Future<void> _createInvite({required bool rethrowing}) =>
      _run(rethrowing: rethrowing, (backend) async {
        final invite = await backend.createPipeInvite(
          _project,
          _name.text.trim(),
          _purpose.text.trim(),
          startId: start.id,
        );
        if (mounted) setState(() => _invite = invite);
      });

  Future<void> _read({bool rethrowing = false}) =>
      _run(rethrowing: rethrowing, (backend) async {
        final preview = await backend.readPipeInvite(_code.text.trim());
        if (!mounted) return;
        setState(() => _preview = preview);
      });

  Future<void> _connect({bool rethrowing = false}) =>
      _run(rethrowing: rethrowing, (backend) async {
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

  Future<void> _dismiss({String? reason, bool rethrowing = false}) => _run(
    rethrowing: rethrowing,
    (backend) => backend.dismissPipeStart(
      start.agentId,
      start.id,
      comment: noComment(reason, _comment.text),
    ),
  );

  /// The step showing's own button for the dock's yes (#514): Make invite,
  /// Continue with a code pasted, then Connect. An invite keeps the dock on
  /// the card from Make invite on, since its code shows there once, after
  /// it's made; a dismissal moves on.
  @override
  CardAnswer? answerNow() {
    if (approvesElsewhere) return null;
    if (_invite != null) {
      return CardAnswer(
        key: 'pipeStart/${start.id}',
        docked: false,
        yes: '',
        no: '',
        keep: true,
      );
    }
    if (!_waiting) return null;
    final (String yes, Future<void> Function()? onYes) = !_accept
        ? (
            'Make invite',
            _name.text.trim().isEmpty || _project.isEmpty
                ? null
                : () => _makeInvite(rethrowing: true),
          )
        : _preview == null
        ? (
            'Continue',
            _code.text.trim().isEmpty ? null : () => _read(rethrowing: true),
          )
        : (
            'Connect',
            _project.isEmpty ? null : () => _connect(rethrowing: true),
          );
    return CardAnswer(
      key: 'pipeStart/${start.id}',
      docked: false,
      yes: yes,
      // The preview's Back is no no: it only goes back a step.
      no: _preview == null ? 'Dismiss' : '',
      typed: false,
      busy: _busy,
      error: _error,
      keep: _making,
      onYes: _busy ? null : onYes,
      onNo: _busy || _preview != null
          ? null
          : (reason) => _dismiss(reason: reason, rethrowing: true),
    );
  }

  Future<void> _copy(String code, int expiresMs) async {
    await copyInviteCode(code, expiresMs: expiresMs);
    if (mounted) setState(() => _copied = true);
  }

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      // What's typed turns the dock's yes on and off.
      listenable: Listenable.merge([store, _name, _code]),
      builder: (context, _) {
        final card = _card(context, store);
        publishAnswer();
        return card;
      },
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
    // No invite code made, shown or taken on Windows (approve_elsewhere.dart).
    if (approvesElsewhere) {
      return [const ApproveElsewhere(text: pipeElsewhereText)];
    }
    if (!_accept) {
      return [
        ..._fields(store, theySee: true),
        _withComment(
          store,
          _actions(
            palette,
            [_dismissButton()],
            ListenableBuilder(
              listenable: _name,
              builder: (context, _) => Button(
                label: 'Make invite',
                primary: true,
                onPressed:
                    _busy || _name.text.trim().isEmpty || _project.isEmpty
                    ? null
                    : _makeInvite,
              ),
            ),
          ),
        ),
      ];
    }
    final preview = _preview;
    if (preview == null) {
      return [
        Field(
          label: 'Code',
          note: _canScan
              ? 'Paste the code you were sent, or scan it.'
              : 'Paste the code you were sent.',
          child: TextField(
            controller: _code,
            placeholder: 'Paste code',
            autocorrect: false,
            textCapitalization: TextCapitalization.none,
            onSubmitted: (_) => _code.text.trim().isEmpty ? null : _read(),
          ),
        ),
        _withComment(
          store,
          _actions(
            palette,
            [
              _dismissButton(),
              if (_canScan)
                Button(label: 'Scan code', onPressed: _busy ? null : _scan),
            ],
            ListenableBuilder(
              listenable: _code,
              builder: (context, _) => Button(
                label: 'Continue',
                primary: true,
                onPressed: _busy || _code.text.trim().isEmpty ? null : _read,
              ),
            ),
          ),
        ),
      ];
    }
    return [
      _TheySay(preview: preview),
      const SizedBox(height: Space.m),
      ..._fields(store, theySee: false, peer: preview.peer),
      _actions(
        palette,
        [
          Button(
            label: 'Back',
            onPressed: _busy
                ? null
                : () => setState(() {
                    _preview = null;
                    _error = null;
                  }),
          ),
        ],
        ListenableBuilder(
          listenable: _name,
          builder: (context, _) => Button(
            label: 'Connect',
            primary: true,
            onPressed: _busy || _project.isEmpty ? null : _connect,
          ),
        ),
      ),
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

  /// The field for the card's agent, sent only with Dismiss, which says
  /// so before anything's typed (Lucy's §3.1), above the card's [row].
  Widget _withComment(WorkspaceStore store, Widget row) {
    final name = store.agent(start.agentId)?.name ?? start.agentId;
    return CardAnswerBlock(
      field: (focus) => DecisionComment(
        controller: _comment,
        focusNode: focus,
        agent: name,
        placeholder: 'Why not? $name sees this only if you dismiss.',
        enabled: !_busy,
      ),
      row: row,
    );
  }

  Widget _dismissButton() =>
      Button(label: 'Dismiss', onPressed: _busy ? null : _dismiss);

  /// The card's row (Lucy's rule 2): the error, if any, in its line, then
  /// the [quiet] buttons and the [primary].
  Widget _actions(Palette palette, List<Widget> quiet, Widget primary) =>
      CardRow(
        line: _error == null ? null : CardLine(_error!, color: palette.danger),
        quiet: quiet,
        primary: primary,
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
      if (_qr) ...[
        const SizedBox(height: Space.m),
        PipeInviteQr(code: invite.code),
      ],
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
      _actions(
        palette,
        [
          if (pipe != null)
            Button(
              label: 'Open pipe',
              onPressed: () => openPipe(context, pipe),
            ),
          if (!_qr)
            Button(
              label: 'Show QR code',
              onPressed: () => setState(() => _qr = true),
            ),
          if (share)
            Button(
              label: _copied ? 'Copied' : 'Copy code',
              onPressed: () => _copy(invite.code, expires),
            ),
        ],
        // Where there's a share sheet, it's the way to send the code, last.
        share
            ? Builder(
                builder: (button) => Button(
                  label: 'Share',
                  primary: true,
                  onPressed: () => shareText(
                    context,
                    invite.code,
                    from: globalRectOf(button),
                  ),
                ),
              )
            : Button(
                label: _copied ? 'Copied' : 'Copy code',
                primary: true,
                onPressed: () => _copy(invite.code, expires),
              ),
      ),
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

/// A pipe invite's code as a QR code, for someone with the person to scan
/// in Claktory on their phone or tablet, instead of being sent the code.
class PipeInviteQr extends StatelessWidget {
  const PipeInviteQr({super.key, required this.code});

  final String code;

  @override
  Widget build(BuildContext context) {
    if (approvesElsewhere) {
      return const ApproveElsewhere(text: pipeElsewhereText);
    }
    final palette = Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Center(
          child: QrCode(key: ValueKey(code), text: code, size: 180),
        ),
        const SizedBox(height: Space.s),
        Text(
          'Someone with you can scan it in Claktory on their phone or '
          'tablet: Connect a pipe…, I have a code, then Scan code.',
          textAlign: TextAlign.center,
          style: TextStyles.caption.copyWith(color: palette.textFaint),
        ),
      ],
    );
  }
}

/// Why [text] isn't a pipe invite, for a scan to keep reading; null when
/// it is one (`claktory-pipe://…`), to read as a pasted code is.
String? pipeInviteCheck(String text) =>
    text.trim().startsWith('claktory-pipe://')
    ? null
    : "This isn't a pipe invite.";

/// A pipe invite's code works for 10 minutes, from when it was made.
const inviteLifetime = Duration(minutes: 10);

/// Copies a pipe invite's code (row 166, Fugo): marked sensitive, so the
/// clipboard's preview, keyboards and clipboard history leave it out, for
/// as long as it works ([expiresMs], else [inviteLifetime]) and cleared
/// then, if it's still there: a dead code, never one the person is about
/// to paste into a messenger. Where this device has no such clipboard
/// (Linux, Windows), the plain one.
Future<void> copyInviteCode(String code, {required int expiresMs}) async {
  final now = DateTime.now().millisecondsSinceEpoch;
  final lasts = expiresMs > now
      ? ((expiresMs - now) / 1000).ceil()
      : inviteLifetime.inSeconds;
  final token = await secretClipboard.copy(code, lasts: lasts);
  if (token == null) {
    await Clipboard.setData(ClipboardData(text: code));
    return;
  }
  // Android keeps no expiry of its own; iOS's is set, and this is the same.
  clearWhenDue(secretClipboard, token, Duration(seconds: lasts));
}
