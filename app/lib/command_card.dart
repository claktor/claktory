// An Execute card (#494, AskPerson.command): an agent asks the person to
// run a command on this Claktory's computer. What the card shows of it, in
// Claktory's words above the agent's (Fugo 35295, 35326; Secco's contract,
// 36076):
// - Claktory's heading, "Run this command on <machine>?", never the
//   agent's title;
// - the command whole, as code with every hidden character by its code,
//   never Markdown, so nothing in it can pass for something else; with
//   Copy;
// - where it runs and when it's stopped;
// - Claktory's warnings, in `danger`;
// - then the agent's reason, named as its.
// Once run, its run in place: how it's going or went, what it waits for,
// and its output, on the card only (never in History).
import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'identity.dart';
import 'markdown.dart';
import 'mobile/lines.dart';
import 'mobile/metrics.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'visible_codes.dart' show proseCodes, visibleCodes;

/// Claktory's heading on an Execute card.
String commandHeading(AskCommand command) => command.machine.isEmpty
    ? 'Run this command?'
    : 'Run this command on ${visibleCodes(command.machine)}?';

/// "2 min", "30 s", "1 h 30 min": how long a command may run.
String commandLimit(int seconds) {
  if (seconds < 60) return '$seconds s';
  if (seconds < 3600) {
    final minutes = seconds ~/ 60;
    final rest = seconds % 60;
    return rest == 0 ? '$minutes min' : '$minutes min $rest s';
  }
  final hours = seconds ~/ 3600;
  final minutes = (seconds % 3600) ~/ 60;
  return minutes == 0 ? '$hours h' : '$hours h $minutes min';
}

/// A command as code, with Copy beside it: every hidden character shown by
/// its code (Fugo); Copy copies it as it is.
class CommandBlock extends StatefulWidget {
  const CommandBlock(this.command, {super.key});

  final String command;

  @override
  State<CommandBlock> createState() => _CommandBlockState();
}

class _CommandBlockState extends State<CommandBlock> {
  var _copied = false;

  @override
  Widget build(BuildContext context) => ActionLine(
    // Wrapped, never scrolled sideways: all of it in sight, a long line's
    // end too (what runs is what's shown). Each of its lines numbered, so a
    // line wrapped on a narrow screen never reads as two.
    lead: MonoBlock(visibleCodes(widget.command), numbered: true),
    action: Button(
      label: _copied ? 'Copied' : 'Copy',
      onPressed: () {
        Clipboard.setData(ClipboardData(text: widget.command));
        setState(() => _copied = true);
      },
    ),
  );
}

/// What an Execute card asks to run, and why: Claktory's heading, the
/// command, where and for how long, Claktory's warnings, then [name]'s
/// reason (the card's title and body).
class CommandFacts extends StatelessWidget {
  const CommandFacts({
    super.key,
    required this.ask,
    required this.name,
    required this.muted,
  });

  final AskPerson ask;
  final String name;

  /// Dismissed or taken back: the heading no longer asks.
  final bool muted;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final command = ask.command;
    final limit = command.timeoutSeconds;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          commandHeading(command),
          style: type.emphasis.copyWith(
            color: muted ? palette.textMuted : palette.text,
          ),
        ),
        const SizedBox(height: Space.s),
        CommandBlock(command.command),
        const SizedBox(height: Space.xs),
        Text(
          [
            // The agent chose it: hidden characters by their codes (Fugo).
            if (command.cwd.isNotEmpty) 'In ${visibleCodes(command.cwd)}',
            if (limit > 0) 'Stops after ${commandLimit(limit)}',
          ].join(' · '),
          style: type.caption.copyWith(color: palette.textMuted),
        ),
        for (final warning in command.warnings) ...[
          const SizedBox(height: Space.xs),
          _Warning(warning),
        ],
        if (ask.title.isNotEmpty || ask.body.isNotEmpty) ...[
          const SizedBox(height: Space.m),
          Text(
            "$name's reason",
            style: type.caption.copyWith(
              color: palette.textFaint,
              fontWeight: FontWeight.w500,
            ),
          ),
          if (ask.title.isNotEmpty)
            Text(
              proseCodes(ask.title),
              style: type.body.copyWith(color: palette.text),
            ),
          if (ask.body.isNotEmpty)
            Markdown(proseCodes(ask.body), color: palette.textMuted),
        ],
      ],
    );
  }
}

/// One of Claktory's warnings about a command, with its kind's glyph.
class _Warning extends StatelessWidget {
  const _Warning(this.warning);

  final CommandWarning warning;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final icon = switch (warning.kind) {
      CommandWarningKind.COMMAND_WARNING_KIND_REMOTE => 'network',
      CommandWarningKind.COMMAND_WARNING_KIND_SENSITIVE => 'key-round',
      CommandWarningKind.COMMAND_WARNING_KIND_PIPE_TO_SHELL => 'globe',
      _ => 'shield-alert',
    };
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(top: 2),
          child: ExtensionGlyph(
            icon: icon,
            name: '',
            color: palette.danger,
            size: 14,
          ),
        ),
        const SizedBox(width: Space.xs + 2),
        Expanded(
          child: Text(
            warning.text,
            style: type.body.copyWith(color: palette.danger),
          ),
        ),
      ],
    );
  }
}

/// [output] without a terminal's escape sequences: CSI (colors, cursor
/// moves) and OSC (titles, links), which would show as codes otherwise.
String withoutTerminalCodes(String output) => output.replaceAll(
  RegExp(
    r'\x1B\[[0-?]*[ -/]*[@-~]'
    r'|\x1B\][^\x07\x1B]*(?:\x07|\x1B\\)'
    r'|\x1B[@-Z\\-_]',
  ),
  '',
);

/// A waiting Execute card in a line or two (Needs you, the agent's list):
/// Claktory's heading, then the command's first line, so cards for one
/// machine can be told apart (Zucchero's row 487), hidden characters by
/// their codes.
String commandSummary(AskCommand command) {
  final first = command.command
      .split('\n')
      .map((line) => line.trim())
      .firstWhere((line) => line.isNotEmpty, orElse: () => '');
  return first.isEmpty
      ? commandHeading(command)
      : '${commandHeading(command)}\n${visibleCodes(first)}';
}

/// How long [run] has gone so far, while it runs: counted here from when it
/// started (the backend sends its own count only to the conversation
/// showing, Secco's row 487), never less than what it last said.
Duration commandElapsed(CommandRun run, {DateTime? now}) {
  final since =
      (now ?? DateTime.now()).millisecondsSinceEpoch - run.startedMs.toInt();
  final said = run.durationMs.toInt();
  return Duration(
    milliseconds: run.startedMs > 0 && since > said ? since : said,
  );
}

/// How an Execute card's run is going or went, in a line: "Running · 12s",
/// "Finished · 1.2s", "Exited with code 1 · 3.4s", "Stopped after 2 min".
/// While it runs, whole seconds as they pass ([commandElapsed]).
String commandRunLine(AskCommand command, {DateTime? now}) {
  final run = command.run;
  final took = formatDuration(Duration(milliseconds: run.durationMs.toInt()));
  return switch (run.state) {
    CommandRunState.COMMAND_RUN_STATE_RUNNING =>
      'Running · ${formatDuration(commandElapsed(run, now: now), whole: true)}',
    CommandRunState.COMMAND_RUN_STATE_EXITED when run.exitCode == 0 =>
      'Finished · $took',
    CommandRunState.COMMAND_RUN_STATE_EXITED =>
      'Exited with code ${run.exitCode} · $took',
    CommandRunState.COMMAND_RUN_STATE_TIMED_OUT =>
      command.timeoutSeconds > 0
          ? 'Stopped after ${commandLimit(command.timeoutSeconds)}, '
                'as it ran too long'
          : 'Stopped, as it ran too long',
    CommandRunState.COMMAND_RUN_STATE_FAILED_TO_START => "Couldn't start",
    CommandRunState.COMMAND_RUN_STATE_INTERRUPTED => 'Stopped',
    _ => '',
  };
}

/// An Execute card's run, under the card once it's run: how it's going or
/// went, what it waits for, where it was run from, and what it printed
/// (the end of it), on the card only.
class CommandRunView extends StatelessWidget {
  const CommandRunView({super.key, required this.command});

  final AskCommand command;

  /// How tall its output gets before it scrolls: about 20 lines.
  static const outputHeight = 320.0;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final run = command.run;
    final running = run.state == CommandRunState.COMMAND_RUN_STATE_RUNNING;
    final failed =
        run.state == CommandRunState.COMMAND_RUN_STATE_FAILED_TO_START ||
        run.state == CommandRunState.COMMAND_RUN_STATE_INTERRUPTED ||
        run.state == CommandRunState.COMMAND_RUN_STATE_TIMED_OUT ||
        (run.state == CommandRunState.COMMAND_RUN_STATE_EXITED &&
            run.exitCode != 0);
    final backend = AppScope.of(context);
    // The app on the computer the command runs on: the sensor is right
    // there, and only its own run should have it touched (Fugo 36019).
    final here = backend.localBackend && backend.target == null;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text.rich(
          TextSpan(
            children: [
              // While it runs, counting up each second.
              WidgetSpan(
                alignment: PlaceholderAlignment.baseline,
                baseline: TextBaseline.alphabetic,
                child: _RunLine(
                  command: command,
                  running: running,
                  style: type.caption.copyWith(
                    color: failed ? palette.danger : palette.text,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
              if (run.device.isNotEmpty)
                TextSpan(
                  text: ' · Run from ${run.device}',
                  style: TextStyle(color: palette.textFaint),
                ),
            ],
          ),
          style: type.caption,
        ),
        if (run.error.isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          Text(run.error, style: type.body.copyWith(color: palette.textMuted)),
        ],
        if (running && run.waiting.isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          Text(
            here
                ? '${run.waiting} Only touch the sensor if you started this.'
                : run.waiting,
            style: type.body.copyWith(color: palette.signal),
          ),
        ],
        if (run.output.isNotEmpty) ...[
          const SizedBox(height: Space.s),
          if (run.outputTruncated)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.xs),
              child: Text(
                'Showing the end',
                style: type.caption.copyWith(color: palette.textFaint),
              ),
            ),
          // The end first, where what it printed last is.
          // As printed, less its terminal codes (colors, titles), with every
          // hidden or reordering character by its code, so nothing in it can
          // make a failure read as a success (Fugo).
          MonoBlock(
            visibleCodes(withoutTerminalCodes(run.output)),
            maxHeight: outputHeight,
          ),
        ],
      ],
    );
  }
}

/// Monospace text in a code block's well (the sidebar's fill, a hairline,
/// the panel's radius), its lines wrapped rather than scrolled sideways.
/// With [maxHeight], it scrolls inside past that, starting at its end.
class MonoBlock extends StatelessWidget {
  const MonoBlock(
    this.text, {
    super.key,
    this.maxHeight,
    this.numbered = false,
  });

  final String text;
  final double? maxHeight;

  /// Each line numbered in a faint gutter: where one ends shows, however
  /// it wraps.
  final bool numbered;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final style = TypeScale.of(context).code.copyWith(color: palette.text);
    final lines = text.split('\n');
    final gutter = '${lines.length}'.length;
    final body = Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.m,
        vertical: Space.s + 2,
      ),
      child: !numbered
          ? Text(text, style: style)
          : Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (final (i, line) in lines.indexed)
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      ExcludeSemantics(
                        child: Text(
                          '${i + 1}'.padLeft(gutter),
                          style: style.copyWith(color: palette.textFaint),
                        ),
                      ),
                      const SizedBox(width: Space.s),
                      Expanded(child: Text(line, style: style)),
                    ],
                  ),
              ],
            ),
    );
    return Container(
      width: double.infinity,
      clipBehavior: Clip.antiAlias,
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      child: maxHeight == null
          ? body
          : ConstrainedBox(
              constraints: BoxConstraints(maxHeight: maxHeight!),
              child: SingleChildScrollView(reverse: true, child: body),
            ),
    );
  }
}

/// [command]'s run line ([commandRunLine]), again each second while it
/// runs.
class _RunLine extends StatefulWidget {
  const _RunLine({
    required this.command,
    required this.running,
    required this.style,
  });

  final AskCommand command;
  final bool running;
  final TextStyle style;

  @override
  State<_RunLine> createState() => _RunLineState();
}

class _RunLineState extends State<_RunLine> {
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    _sync();
  }

  @override
  void didUpdateWidget(_RunLine old) {
    super.didUpdateWidget(old);
    _sync();
  }

  void _sync() {
    if (widget.running) {
      _tick ??= Timer.periodic(
        const Duration(seconds: 1),
        (_) => setState(() {}),
      );
    } else {
      _tick?.cancel();
      _tick = null;
    }
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) =>
      Text(commandRunLine(widget.command), style: widget.style);
}
