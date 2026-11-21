import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'mobile/input.dart' show InputKind;
import 'mobile/metrics.dart';
import 'protocol/factory/v1/factory.pb.dart' as pb;
import 'resources.dart' show formatBytes;

/// Moving this Claktory to another computer (#217, docs/MOVE.md, "The
/// handshake"): the target runs `factoryd receive` and shows a code; the
/// person pastes it here, compares six digits, reads the plan, and the
/// two copy the Claktory between them while its agents keep working, then
/// switch over after a short freeze.

/// Where a move stands (Workspace.move's phase).
enum MovePhase {
  /// No move.
  none,

  /// Dialling the target.
  connecting,

  /// Both sides show the digits; the person confirms they match.
  confirm,

  /// The target checks the plan: the plan shows, problems and all.
  checking,

  /// Copying while the agents work.
  copying,

  /// Agents stopped; the last of it is copied.
  frozen,

  /// The target imports what it got.
  importing,

  /// The switch: past the point of no return.
  committing,

  /// Moved: this Claktory runs on the target now.
  done,

  /// It stopped; the Claktory carries on here as it was.
  failed,
}

/// A folder's place on this computer and where it goes on the other.
typedef MoveMapping = ({String from, String to});

/// A move as the backend says it is: all the app shows of it.
@immutable
class MoveStatus {
  const MoveStatus({
    required this.phase,
    this.targetMachine = '',
    this.targetOs = '',
    this.targetArch = '',
    this.targetVersion = '',
    this.digits = '',
    this.targetConfirmed = false,
    this.planBytes = 0,
    this.canCancel = false,
    this.problems = const [],
    this.missingTools = const [],
    this.map = const [],
    this.bytesHave = 0,
    this.bytesTotal = 0,
    this.round = 0,
    this.stepText = '',
    this.error = '',
  });

  static const none = MoveStatus(phase: MovePhase.none);

  final MovePhase phase;

  /// The other computer: its name, system and Claktory, from the code.
  final String targetMachine;
  final String targetOs;
  final String targetArch;
  final String targetVersion;

  /// The six digits both computers show, "123 456".
  final String digits;

  /// The person confirmed the digits on the other computer too.
  final bool targetConfirmed;

  /// How much there is to move.
  final int planBytes;

  /// It can still be called off, and the Claktory carry on here: until
  /// the switch begins.
  final bool canCancel;

  /// Why it can't go yet, each a sentence; none: it can.
  final List<String> problems;

  /// Commands the agents used that the other computer doesn't have.
  final List<String> missingTools;

  /// Where folders go.
  final List<MoveMapping> map;

  /// How much the other computer has of it, of all there is.
  final int bytesHave;
  final int bytesTotal;

  /// Which copying round this is, from 1.
  final int round;

  /// What the other computer does now, while it imports.
  final String stepText;

  /// Why it failed, a sentence.
  final String error;

  bool get active => phase != MovePhase.none;

  /// The agents are stopped for it: every device shows it alone.
  bool get frozen => switch (phase) {
    MovePhase.frozen ||
    MovePhase.importing ||
    MovePhase.committing ||
    MovePhase.done => true,
    _ => false,
  };

  /// "Linux x86_64".
  String get targetSystem => systemName(targetOs, targetArch);

  /// "cloud-box", or "the other computer" before it's said.
  String get target =>
      targetMachine.isEmpty ? 'the other computer' : targetMachine;
}

/// What a move can be asked to do (backend.dart's requests).
abstract interface class MoveActions {
  Future<void> start(String code);
  Future<void> confirm();
  Future<void> cancel();

  /// Stops the agents and copies the last of it now, rather than when
  /// copying has caught up.
  Future<void> freeze();
}

/// What a receive code says of the computer it's from, read here before
/// anything is dialled.
@immutable
class ReceiveCode {
  const ReceiveCode({
    required this.code,
    required this.machine,
    required this.os,
    required this.arch,
    required this.version,
  });

  final String code;
  final String machine;
  final String os;
  final String arch;
  final String version;

  /// "Linux x86_64".
  String get system => systemName(os, arch);
}

/// A computer's system as people say it: "Linux x86_64", "macOS arm64".
String systemName(String os, String arch) => [
  switch (os.toLowerCase()) {
    'linux' => 'Linux',
    'macos' || 'darwin' => 'macOS',
    'windows' => 'Windows',
    _ => os,
  },
  arch,
].where((part) => part.isNotEmpty).join(' ');

final _hex64 = RegExp(r'^[0-9a-f]{64}$');

/// [text] as a receive code (`claktory-move://?e=…&s=…&m=…`), or null.
ReceiveCode? parseReceiveCode(String text) {
  final code = text.trim();
  final uri = Uri.tryParse(code);
  if (uri == null || uri.scheme != 'claktory-move') return null;
  final query = uri.queryParameters;
  if ((query['e'] ?? '').isEmpty || !_hex64.hasMatch(query['s'] ?? '')) {
    return null;
  }
  return ReceiveCode(
    code: code,
    machine: query['m'] ?? '',
    os: query['o'] ?? '',
    arch: query['h'] ?? '',
    version: query['v'] ?? '',
  );
}

/// How far copying has got, 0 to 1.
double moveProgress(MoveStatus move) => move.bytesTotal <= 0
    ? 0
    : (move.bytesHave / move.bytesTotal).clamp(0.0, 1.0);

/// "1.2 of 4.8 GB".
String moveBytes(MoveStatus move) =>
    '${formatBytes(Int64(move.bytesHave))} of ${formatBytes(Int64(move.bytesTotal))}';

/// A 4px meter, filled in `signal` as far as [value] (0 to 1).
class MoveMeter extends StatelessWidget {
  const MoveMeter({super.key, required this.value});

  final double value;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ClipRRect(
      borderRadius: BorderRadius.circular(2),
      child: SizedBox(
        height: 4,
        child: Stack(
          fit: StackFit.expand,
          children: [
            ColoredBox(color: palette.text.withValues(alpha: 0.14)),
            FractionallySizedBox(
              alignment: Alignment.centerLeft,
              widthFactor: value,
              child: ColoredBox(color: palette.signal),
            ),
          ],
        ),
      ),
    );
  }
}

/// The plan, as the other computer checked it: problems first (each in
/// `danger`), the tools it lacks, then where folders go.
class MovePlan extends StatelessWidget {
  const MovePlan({super.key, required this.move});

  final MoveStatus move;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    Widget heading(String text) => Padding(
      padding: const EdgeInsets.only(top: Space.m, bottom: Space.xs),
      child: Text(
        text,
        style: type.caption.copyWith(
          color: palette.textMuted,
          fontWeight: FontWeight.w600,
        ),
      ),
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (move.planBytes > 0)
          Text(
            '${formatBytes(Int64(move.planBytes))} to copy.',
            style: type.body.copyWith(color: palette.text),
          ),
        if (move.problems.isNotEmpty) ...[
          heading("What's in the way"),
          for (final problem in move.problems)
            Text(problem, style: type.body.copyWith(color: palette.danger)),
        ],
        if (move.missingTools.isNotEmpty) ...[
          heading('Not on ${move.target}'),
          Text(
            '${move.missingTools.join(', ')}. Your agents use these. '
            'Install them there so their work can carry on.',
            style: type.body.copyWith(color: palette.textMuted),
          ),
        ],
        if (move.map.isNotEmpty) ...[
          heading('Folders'),
          for (final (:from, :to) in move.map)
            Text(
              from == to ? from : '$from → $to',
              style: type.caption.copyWith(
                color: palette.textMuted,
                fontFamily: TextStyles.mono,
              ),
            ),
        ],
      ],
    );
  }
}

/// The move, from pasting the code to done, as a form: what it waits for,
/// what can be done, Cancel while it can be. The backend drives it; this
/// form keeps only the code being typed.
class MoveForm extends StatefulWidget {
  const MoveForm({
    super.key,
    required this.move,
    required this.actions,
    required this.close,
  });

  final MoveStatus move;
  final MoveActions actions;
  final VoidCallback close;

  @override
  State<MoveForm> createState() => _MoveFormState();
}

class _MoveFormState extends State<MoveForm> {
  final _code = TextEditingController();
  var _busy = false;
  String? _error;

  @override
  void dispose() {
    _code.dispose();
    super.dispose();
  }

  Future<void> _run(Future<void> Function() request) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await request();
    } on Exception catch (error) {
      if (mounted) setState(() => _error = _said(error));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  static String _said(Exception error) {
    try {
      return (error as dynamic).message as String;
    } on Object {
      return '$error';
    }
  }

  @override
  Widget build(BuildContext context) {
    final move = widget.move;
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final cancel = Button(
      label: 'Cancel',
      onPressed: _busy || !move.canCancel
          ? null
          : () => _run(widget.actions.cancel),
    );
    final (
      String title,
      Widget body,
      List<Widget> buttons,
    ) = switch (move.phase) {
      MovePhase.none => _paste(context),
      MovePhase.connecting => (
        'Connecting to ${move.target}…',
        const SizedBox.shrink(),
        [cancel],
      ),
      MovePhase.confirm => (
        'Do the digits match?',
        Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _line(
              context,
              move.targetConfirmed
                  ? '${move.target} confirmed them. Check that it shows the '
                        'same six digits.'
                  : 'Check that ${move.target} shows the same six digits, '
                        'and confirm them there too.',
            ),
            const SizedBox(height: Space.m),
            Text(
              move.digits,
              textAlign: TextAlign.center,
              style: type.emphasis.copyWith(
                color: palette.text,
                fontSize: 28,
                fontFeatures: const [FontFeature.tabularFigures()],
                letterSpacing: 2,
              ),
            ),
          ],
        ),
        [
          cancel,
          Button(
            label: 'They match',
            primary: true,
            onPressed: _busy ? null : () => _run(widget.actions.confirm),
          ),
        ],
      ),
      MovePhase.checking => (
        'Checking with ${move.target}…',
        _line(
          context,
          'Making sure it has room and everything your agents need.',
        ),
        [cancel],
      ),
      MovePhase.copying => (
        'Copying to ${move.target}',
        Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _line(
              context,
              'Your agents keep working while it copies. Then they pause for '
              'a moment while the last changes copy, and carry on there.',
            ),
            const SizedBox(height: Space.m),
            MoveMeter(value: moveProgress(move)),
            const SizedBox(height: Space.xs),
            Text(
              [
                if (move.bytesTotal > 0) moveBytes(move),
                if (move.round > 1) 'copying what changed',
              ].join(' · '),
              style: type.caption.copyWith(
                color: palette.textMuted,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
            if (move.planBytes > 0 ||
                move.missingTools.isNotEmpty ||
                move.map.isNotEmpty) ...[
              const SizedBox(height: Space.s),
              MovePlan(
                move: MoveStatus(
                  phase: move.phase,
                  targetMachine: move.targetMachine,
                  planBytes: move.planBytes,
                  missingTools: move.missingTools,
                  map: move.map,
                ),
              ),
            ],
          ],
        ),
        [
          cancel,
          Button(
            label: 'Move now',
            primary: true,
            onPressed: _busy ? null : () => _run(widget.actions.freeze),
          ),
        ],
      ),
      MovePhase.frozen || MovePhase.importing || MovePhase.committing => (
        'Moving to ${move.target}…',
        MovingProgress(move: move),
        [cancel],
      ),
      MovePhase.done => (
        'Moved to ${move.target}',
        _line(
          context,
          'Claktory runs on ${move.target} now. Your devices follow it by '
          'themselves.',
        ),
        [Button(label: 'Done', primary: true, onPressed: widget.close)],
      ),
      MovePhase.failed => (
        "Couldn't move to ${move.target}",
        Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (move.error.isNotEmpty)
              Text(
                move.error,
                style: type.body.copyWith(color: palette.danger),
              ),
            if (move.problems.isNotEmpty) MovePlan(move: move),
            const SizedBox(height: Space.s),
            _line(context, 'Claktory keeps running here as before.'),
          ],
        ),
        [
          Button(
            label: 'Close',
            primary: true,
            onPressed: _busy
                ? null
                : () async {
                    await _run(widget.actions.cancel);
                    widget.close();
                  },
          ),
        ],
      ),
    };
    // On a phone the whole form scrolls, title and buttons too, so the
    // field and buttons stay in reach over the keyboard and at the largest
    // text; on a Mac only its middle does, between title and buttons.
    final phone = isPhoneLayout(context);
    final form = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // A heading; on a phone, where the form is a sheet of its own, it
        // names the sheet too.
        Semantics(
          container: true,
          header: true,
          namesRoute: phone,
          child: Text(
            title,
            style: type.emphasis.copyWith(color: palette.text),
          ),
        ),
        const SizedBox(height: Space.s),
        phone ? body : Flexible(child: SingleChildScrollView(child: body)),
        if (_error case final error?)
          Padding(
            padding: const EdgeInsets.only(top: Space.m),
            child: Text(
              error,
              style: type.caption.copyWith(color: palette.danger),
            ),
          ),
        const SizedBox(height: Space.xl),
        Wrap(
          alignment: WrapAlignment.end,
          spacing: Space.s,
          runSpacing: Space.s,
          children: buttons,
        ),
      ],
    );
    return ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 480),
      child: phone ? SingleChildScrollView(child: form) : form,
    );
  }

  Widget _line(BuildContext context, String text) => Text(
    text,
    style: TypeScale.of(context).body
        .copyWith(color: Palette.of(context).textMuted),
  );

  /// Before a move: the code from the other computer, and what it says of
  /// it once it's one.
  (String, Widget, List<Widget>) _paste(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return (
      'Move to another computer',
      ListenableBuilder(
        listenable: _code,
        builder: (context, _) {
          final code = parseReceiveCode(_code.text);
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _line(
                context,
                'Everything moves, and your agents keep working while it '
                'copies. On the other computer, paste this in a terminal: it '
                'installs Claktory there and shows a code. Paste that code '
                'here.',
              ),
              const SizedBox(height: Space.s),
              const _Command(receiveCommand),
              const SizedBox(height: Space.m),
              TextField(
                controller: _code,
                autofocus: true,
                placeholder: 'Paste code',
                // Still named once it's filled in. A link, never corrected
                // or suggested, and kept out of autofill: it holds a secret.
                semanticsLabel: 'Paste code',
                kind: InputKind.url,
                autofillHints: const [],
                onSubmitted: (_) => _connect(),
              ),
              const SizedBox(height: Space.xs),
              if (code != null)
                Text(
                  [
                    if (code.machine.isNotEmpty) code.machine,
                    if (code.system.isNotEmpty) code.system,
                    if (code.version.isNotEmpty) 'Claktory ${code.version}',
                  ].join(' · '),
                  style: type.caption.copyWith(color: palette.textMuted),
                )
              else if (_code.text.trim().isNotEmpty)
                Text(
                  "This isn't the code the other computer shows.",
                  style: type.caption.copyWith(color: palette.danger),
                ),
            ],
          );
        },
      ),
      [
        Button(label: 'Cancel', onPressed: widget.close),
        ListenableBuilder(
          listenable: _code,
          builder: (context, _) => Button(
            label: 'Connect',
            primary: true,
            onPressed: _busy || parseReceiveCode(_code.text) == null
                ? null
                : _connect,
          ),
        ),
      ],
    );
  }

  void _connect() {
    final code = parseReceiveCode(_code.text);
    if (code == null || _busy) return;
    _run(() => widget.actions.start(code.code));
  }
}

/// While the agents are stopped for the move: what's happening, the meter
/// while the last of it copies, then the other computer's step.
class MovingProgress extends StatelessWidget {
  const MovingProgress({super.key, required this.move});

  final MoveStatus move;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final line = move.stepText.isNotEmpty
        ? move.stepText
        : switch (move.phase) {
            MovePhase.frozen =>
              'Copying what changed since your agents paused.',
            MovePhase.importing => 'Setting up on ${move.target}.',
            MovePhase.committing => 'Switching over.',
            _ => '',
          };
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(line, style: type.body.copyWith(color: palette.textMuted)),
        if (move.phase == MovePhase.frozen && move.bytesTotal > 0) ...[
          const SizedBox(height: Space.m),
          MoveMeter(value: moveProgress(move)),
          const SizedBox(height: Space.xs),
          Text(
            moveBytes(move),
            style: type.caption.copyWith(
              color: palette.textMuted,
              fontFeatures: const [FontFeature.tabularFigures()],
            ),
          ),
        ],
      ],
    );
  }
}

/// The whole window while the agents are stopped for the move (the backend
/// then serves nothing else): "Moving to cloud-box…", how far, Cancel while
/// it can be, and once moved, where Claktory runs now.
class MovingScreen extends StatelessWidget {
  const MovingScreen({super.key, required this.move, required this.actions});

  final MoveStatus move;
  final MoveActions actions;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final done = move.phase == MovePhase.done;
    final failed = move.phase == MovePhase.failed;
    return ColoredBox(
      color: palette.canvas,
      child: SafeArea(
        child: Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(Space.xl),
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 420),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (!done && !failed) ...[
                    const Align(
                      alignment: Alignment.centerLeft,
                      child: LiveDot(),
                    ),
                    const SizedBox(height: Space.m),
                  ],
                  Semantics(
                    header: true,
                    child: Text(
                      done
                          ? 'Moved to ${move.target}'
                          : failed
                          ? "Couldn't move to ${move.target}"
                          : 'Moving to ${move.target}…',
                      style: type.emphasis.copyWith(color: palette.text),
                    ),
                  ),
                  const SizedBox(height: Space.s),
                  if (failed) ...[
                    if (move.error.isNotEmpty) ...[
                      Text(
                        move.error,
                        style: type.body.copyWith(color: palette.danger),
                      ),
                      const SizedBox(height: Space.s),
                    ],
                    Text(
                      'Claktory keeps running here as before, and your agents '
                      'carry on.',
                      style: type.body.copyWith(color: palette.textMuted),
                    ),
                  ] else if (done)
                    Text(
                      'Claktory runs on ${move.target} now. Your devices '
                      'follow it by themselves.',
                      style: type.body.copyWith(color: palette.textMuted),
                    )
                  else ...[
                    Text(
                      'Your agents are paused until Claktory starts on '
                      '${move.target}.',
                      style: type.body.copyWith(color: palette.textMuted),
                    ),
                    const SizedBox(height: Space.m),
                    MovingProgress(move: move),
                  ],
                  if (move.canCancel) ...[
                    const SizedBox(height: Space.xl),
                    Align(
                      alignment: Alignment.centerRight,
                      child: _CancelMove(
                        actions: actions,
                        label: failed ? 'Close' : 'Cancel move',
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _CancelMove extends StatefulWidget {
  const _CancelMove({required this.actions, required this.label});

  final MoveActions actions;
  final String label;

  @override
  State<_CancelMove> createState() => _CancelMoveState();
}

class _CancelMoveState extends State<_CancelMove> {
  var _busy = false;

  @override
  Widget build(BuildContext context) => Button(
    label: widget.label,
    primary: widget.label == 'Close',
    onPressed: _busy
        ? null
        : () async {
            setState(() => _busy = true);
            try {
              await widget.actions.cancel();
            } finally {
              if (mounted) setState(() => _busy = false);
            }
          },
  );
}

/// A move's state as it changes, and what can be asked of it: the backend
/// in the app ([BackendMove]), a fake in tests.
abstract interface class MoveSource implements MoveActions {
  ValueListenable<MoveStatus> get status;
}

/// [move] (Workspace.move) as the app shows it; none when unset.
MoveStatus moveStatusOf(pb.Move? move) {
  if (move == null) return MoveStatus.none;
  final phase = switch (move.phase) {
    pb.MovePhase.MOVE_PHASE_CONNECTING => MovePhase.connecting,
    pb.MovePhase.MOVE_PHASE_CONFIRM => MovePhase.confirm,
    pb.MovePhase.MOVE_PHASE_CHECKING => MovePhase.checking,
    pb.MovePhase.MOVE_PHASE_COPYING => MovePhase.copying,
    pb.MovePhase.MOVE_PHASE_FROZEN => MovePhase.frozen,
    pb.MovePhase.MOVE_PHASE_IMPORTING => MovePhase.importing,
    pb.MovePhase.MOVE_PHASE_COMMITTING => MovePhase.committing,
    pb.MovePhase.MOVE_PHASE_DONE => MovePhase.done,
    pb.MovePhase.MOVE_PHASE_FAILED => MovePhase.failed,
    // A phase from a newer backend: under way, whatever it is.
    _ => MovePhase.connecting,
  };
  return MoveStatus(
    phase: phase,
    targetMachine: move.targetMachine,
    targetOs: move.targetOs,
    targetArch: move.targetArch,
    targetVersion: move.targetVersion,
    digits: move.digits,
    targetConfirmed: move.targetConfirmed,
    problems: move.problems,
    missingTools: move.missingTools,
    map: [for (final path in move.map) (from: path.from, to: path.to)],
    planBytes: move.planBytes.toInt(),
    bytesHave: move.bytesHave.toInt(),
    bytesTotal: move.bytesTotal.toInt(),
    round: move.round,
    stepText: move.stepText,
    error: move.error,
    canCancel: move.canCancel,
  );
}

/// This Claktory's move, from [Backend.move] (kept while the connection is
/// down), and the backend's move requests.
class BackendMove implements MoveSource {
  const BackendMove(this.backend);

  final Backend backend;

  @override
  ValueListenable<MoveStatus> get status => _MoveStatusOf(backend.move);

  @override
  Future<void> start(String code) => backend.startMove(code);

  @override
  Future<void> confirm() => backend.confirmMove();

  @override
  Future<void> cancel() => backend.cancelMove();

  @override
  Future<void> freeze() => backend.freezeMove();

  @override
  bool operator ==(Object other) =>
      other is BackendMove && identical(other.backend, backend);

  @override
  int get hashCode => identityHashCode(backend);
}

class _MoveStatusOf implements ValueListenable<MoveStatus> {
  const _MoveStatusOf(this.move);

  final ValueListenable<pb.Move?> move;

  @override
  MoveStatus get value => moveStatusOf(move.value);

  @override
  void addListener(VoidCallback listener) => move.addListener(listener);

  @override
  void removeListener(VoidCallback listener) => move.removeListener(listener);

  @override
  bool operator ==(Object other) =>
      other is _MoveStatusOf && identical(other.move, move);

  @override
  int get hashCode => identityHashCode(move);
}

/// [child], or while the agents are paused for a move, [MovingScreen] in
/// its place: the backend serves nothing else then (its Workspace has only
/// the move). A move that fails after the pause stays on the screen, saying
/// why, until the person closes it. A Mac's [rail] stays beside it, so
/// another Claktory is always in reach.
class MoveGate extends StatefulWidget {
  const MoveGate({
    super.key,
    required this.source,
    this.rail,
    required this.child,
  });

  final MoveSource source;
  final Widget? rail;
  final Widget child;

  @override
  State<MoveGate> createState() => _MoveGateState();
}

class _MoveGateState extends State<MoveGate> {
  /// The screen showed this move paused: a failure now is told there.
  var _paused = false;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder<MoveStatus>(
    valueListenable: widget.source.status,
    builder: (context, move, child) {
      if (move.frozen) {
        _paused = true;
      } else if (move.phase != MovePhase.failed) {
        _paused = false;
      }
      if (!_paused) return child!;
      final screen = MovingScreen(move: move, actions: widget.source);
      final rail = widget.rail;
      if (rail == null) return screen;
      return Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          rail,
          Expanded(child: screen),
        ],
      );
    },
    child: widget.child,
  );
}

/// Opens the move's form over the window; it follows the move as it goes.
Future<void> showMove(BuildContext context, MoveSource source) => showModal(
  context,
  (context, close) => ValueListenableBuilder<MoveStatus>(
    valueListenable: source.status,
    builder: (context, move, _) =>
        MoveForm(move: move, actions: source, close: close),
  ),
);

/// Settings' way in: "Move to another computer…", or while one runs, how
/// it's going and a way back to it.
class MoveSettingRow extends StatelessWidget {
  const MoveSettingRow({super.key, required this.source});

  final MoveSource source;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return ValueListenableBuilder<MoveStatus>(
      valueListenable: source.status,
      builder: (context, move, _) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Button(
            label: move.active ? 'Show progress…' : 'Move to another computer…',
            onPressed: () => showMove(context, source),
          ),
          const SizedBox(height: Space.xs),
          Text(
            switch (move.phase) {
              MovePhase.none =>
                'Moves Claktory and everything in it to another computer: '
                    'agents, conversations, folders, secrets and sign-ins. '
                    'Your agents pause only for a moment.',
              MovePhase.copying when move.bytesTotal > 0 =>
                'Copying to ${move.target}: ${moveBytes(move)}.',
              MovePhase.failed => "Couldn't move to ${move.target}.",
              _ => 'Moving to ${move.target}.',
            },
            style: type.caption.copyWith(
              color: move.phase == MovePhase.failed
                  ? palette.danger
                  : palette.textFaint,
            ),
          ),
        ],
      ),
    );
  }
}

/// What the person pastes in a terminal on the other computer: the online
/// installer, in receive mode, installs Claktory there and shows the code.
const receiveCommand =
    'curl -fsSL https://github.com/claktor/claktory/releases/latest/'
    'download/install.sh | bash -s -- --receive';

/// A command to run, in mono on a hairline panel, with Copy.
class _Command extends StatefulWidget {
  const _Command(this.command);

  final String command;

  @override
  State<_Command> createState() => _CommandState();
}

class _CommandState extends State<_Command> {
  var _copied = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Container(
      padding: const EdgeInsets.fromLTRB(Space.m, Space.xs, Space.xs, Space.xs),
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(
              widget.command,
              style: type.body.copyWith(
                color: palette.text,
                fontFamily: TextStyles.mono,
              ),
            ),
          ),
          Button(
            label: _copied ? 'Copied' : 'Copy',
            onPressed: () async {
              await Clipboard.setData(ClipboardData(text: widget.command));
              if (mounted) setState(() => _copied = true);
            },
          ),
        ],
      ),
    );
  }
}
