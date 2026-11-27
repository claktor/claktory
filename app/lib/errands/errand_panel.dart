import 'dart:async';

import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../characters/character.dart';
import '../controls.dart';
import '../extensions.dart' show logTime;
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../mobile/metrics.dart' show TypeScale;
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import '../tell_rivet.dart' show ErrorLine;
import 'browser_session.dart' show errandOpen;
import 'origin.dart';

// An errand in Claktory's browser (#435 P5): what it's for, whose it is,
// the sites it may go to, how it's going, and its log: what was done,
// where, by whom, never what was typed or read. Nothing is kept on the
// device: the log is asked for each time it shows.

/// How [errand] is going, in a few words.
String errandStateLabel(Errand errand) => switch (errand.state) {
  ErrandState.ERRAND_STATE_RUNNING => 'Running',
  ErrandState.ERRAND_STATE_PERSONS_TURN => 'Waiting for you',
  ErrandState.ERRAND_STATE_TAKEN_OVER => 'You took over',
  ErrandState.ERRAND_STATE_DONE => 'Done',
  ErrandState.ERRAND_STATE_STOPPED => 'Stopped',
  ErrandState.ERRAND_STATE_CANCELLED => 'Cancelled',
  ErrandState.ERRAND_STATE_FAILED => 'Ended',
  _ => '',
};

/// [errand] in a line: its agent, what it's for (the opener's words) and
/// how it's going, with Log.
class ErrandLine extends StatelessWidget {
  const ErrandLine({super.key, required this.errand, required this.agent});

  final Errand errand;

  /// Its agent, when this Claktory still has it.
  final Agent? agent;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final agent = this.agent;
    final name = agent?.name ?? 'An agent';
    return Row(
      children: [
        if (agent != null) ...[
          AgentCharacter(agent: agent, size: 18),
          const SizedBox(width: Space.xs + 2),
        ],
        Expanded(
          child: Text.rich(
            TextSpan(
              children: [
                TextSpan(
                  text: "$name's errand",
                  style: type.caption.copyWith(
                    color: palette.text,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                if (errand.purpose.isNotEmpty)
                  TextSpan(
                    text: ': ${errand.purpose}',
                    style: type.caption.copyWith(color: palette.textMuted),
                  ),
                TextSpan(
                  text: ' · ${errandStateLabel(errand)}',
                  style: type.caption.copyWith(color: palette.textFaint),
                ),
              ],
            ),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
        ),
        const SizedBox(width: Space.s),
        Button(label: 'Log', onPressed: () => showErrand(context, errand)),
      ],
    );
  }
}

/// Opens [errand]: what it's for, its sites, and its log.
Future<void> showErrand(BuildContext context, Errand errand) => showModal(
  context,
  (context, close) => _ErrandPanel(errandId: errand.id, close: close),
);

class _ErrandPanel extends StatefulWidget {
  const _ErrandPanel({required this.errandId, required this.close});

  final String errandId;
  final VoidCallback close;

  @override
  State<_ErrandPanel> createState() => _ErrandPanelState();
}

class _ErrandPanelState extends State<_ErrandPanel> {
  ErrandLog? _log;
  String? _error;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_log == null && _error == null) unawaited(_load());
  }

  Future<void> _load() async {
    try {
      final log = await AppScope.of(context).listErrandLog(widget.errandId);
      if (mounted) setState(() => _log = log);
    } on RequestFailure catch (error) {
      if (mounted) setState(() => _error = error.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final palette = Palette.of(context);
        final type = TypeScale.of(context);
        final workspace = backend.workspace.workspace;
        final errand = workspace.errands
            .where((errand) => errand.id == widget.errandId)
            .firstOrNull;
        final agent = errand == null
            ? null
            : backend.workspace.agent(errand.agentId);
        final opener = errand == null || errand.openedBy.isEmpty
            ? 'you'
            : backend.workspace.agent(errand.openedBy)?.name ?? 'an agent';
        return FormFrame(
          title: errand == null
              ? 'Errand'
              : "${agent?.name ?? 'An agent'}'s errand",
          body: errand == null || errand.purpose.isEmpty
              ? null
              : errand.purpose,
          error: null,
          submitLabel: 'Done',
          onSubmit: widget.close,
          onCancel: widget.close,
          fields: [
            if (errand != null) ...[
              Text(
                [
                  'Opened by $opener ${logTime(DateTime.fromMillisecondsSinceEpoch(errand.createdMs.toInt()))}',
                  errandStateLabel(errand),
                  if (!errandOpen(errand.state) && errand.endReason.isNotEmpty)
                    errand.endReason,
                ].join(' · '),
                style: type.caption.copyWith(color: palette.textMuted),
              ),
              const SizedBox(height: Space.s),
              Text(
                errand.sites.isEmpty
                    ? 'No sites'
                    : 'Only on ${errand.sites.map(originHost).join(', ')}',
                style: type.caption.copyWith(color: palette.textMuted),
              ),
              const SizedBox(height: Space.m),
            ],
            if (_error case final error?)
              ErrorLine(
                error,
                about: "Claktory couldn't show the errand's log.",
              )
            else if (_log case final log?)
              if (log.entries.isEmpty)
                Text(
                  'Nothing done yet.',
                  style: type.body.copyWith(color: palette.textMuted),
                )
              else
                for (final entry in log.entries.reversed)
                  _LogRow(entry: entry, agent: agent),
          ],
        );
      },
    );
  }
}

/// One thing done: when, who, what, where; the element it was done to as
/// the page's words, in quotes.
class _LogRow extends StatelessWidget {
  const _LogRow({required this.entry, required this.agent});

  final ErrandLogEntry entry;
  final Agent? agent;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final who = entry.agentId.isEmpty ? 'You' : (agent?.name ?? 'The agent');
    final where = [
      if (entry.origin.isNotEmpty) originHost(entry.origin),
      entry.path,
    ].join();
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.s),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text.rich(
            TextSpan(
              children: [
                TextSpan(
                  text: '$who: ${entry.action}',
                  style: type.body.copyWith(color: palette.text),
                ),
                if (entry.target.isNotEmpty)
                  TextSpan(
                    text: ' “${entry.target}”',
                    style: type.body.copyWith(color: palette.textMuted),
                  ),
              ],
            ),
          ),
          Text(
            [
              logTime(DateTime.fromMillisecondsSinceEpoch(entry.atMs.toInt())),
              if (where.isNotEmpty) where,
              if (entry.outcome.isNotEmpty) entry.outcome,
            ].join(' · '),
            style: type.caption.copyWith(
              color: entry.outcome.isEmpty ? palette.textFaint : palette.danger,
            ),
          ),
        ],
      ),
    );
  }
}
