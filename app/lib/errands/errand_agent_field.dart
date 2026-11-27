// "Use for errands" in Agent settings (#435; Bruno 49824, Lucy 49837): the
// person contains an agent for errands in Claktory's browser, or lets it
// out, at once with SetAgentContained, never through the form's Save. As
// Stop reading reports does: a note on what it means, and a question in
// place, in a bordered panel, before anything changes.
import 'dart:async';

import 'package:flutter/widgets.dart';

import '../backend.dart' show RequestFailure;
import '../controls.dart';
import '../devices.dart' show QuietLink;
import '../identity.dart';
import '../mobile/metrics.dart' show TypeScale;
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import '../state.dart' show isAdminAgent;
import 'browser_session.dart' show agentBrowserAvailable;

/// The errands extension's id.
const errandsExtension = 'errands';

/// Whether Agent settings offers errands for [agent]: where there's a
/// browser, and never for an admin agent or one under another limit (a
/// pipe's, or reading reports), which has its own note.
bool offersErrands(Workspace workspace, Agent agent) =>
    agentBrowserAvailable(workspace) &&
    !isAdminAgent(agent) &&
    !agent.quarantined &&
    !agent.readsReports;

class ErrandAgentField extends StatefulWidget {
  const ErrandAgentField({super.key, required this.agent, this.onContained});

  final Agent agent;

  /// It was just contained: the form unfolds its extensions at Errands,
  /// the next step.
  final VoidCallback? onContained;

  @override
  State<ErrandAgentField> createState() => _ErrandAgentFieldState();
}

class _ErrandAgentFieldState extends State<ErrandAgentField> {
  var _asking = false;
  var _busy = false;
  String? _error;

  Future<void> _set(bool contained) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context)
          .setAgentContained(widget.agent.id, contained: contained);
      if (!mounted) return;
      setState(() => _asking = false);
      if (contained) widget.onContained?.call();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    final workspace = store.workspace;
    final agent = widget.agent;
    // Contained already, it shows even where it couldn't be offered now,
    // so it can be let out.
    if (!agent.contained && !offersErrands(workspace, agent)) {
      return const SizedBox.shrink();
    }
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final caption = type.caption.copyWith(color: palette.textMuted);
    final name = agent.name;
    final on = agent.contained;
    // Whom it can still message: its lead, by name (Trish 49860); where
    // it works and with what, in the question (Fugo 49863).
    final lead = agent.leadId.isEmpty ? null : store.agent(agent.leadId)?.name;
    final others = lead == null ? 'other agents' : 'anyone but $lead';
    final icon = workspace.extensions
        .where((extension) => extension.id == errandsExtension)
        .firstOrNull
        ?.icon;
    final glyph = ExcludeSemantics(
      child: ExtensionGlyph(
        icon: icon ?? 'globe',
        name: 'Errands',
        color: palette.textMuted,
        size: 12,
      ),
    );
    final panel = _asking
        ? _Panel(
            title: on
                ? 'Stop using $name for errands?'
                : 'Use $name for errands?',
            body: on
                // What it read stays with it until its conversation is
                // cleared (Fugo 49848, Lucy 49850): no fresh start here.
                ? 'Its errand ends, Errands turns off in its extensions, and '
                      '$name gets back commands, the web, secrets, messaging, '
                      'apps and its other extensions. Its '
                      'conversation still holds words from pages it read, so '
                      'each use of a secret waits for your OK until you clear '
                      'its conversation.'
                : "$name will do things on websites for you in Claktory's "
                      "browser. While it runs errands, it can't run commands, "
                      'search the web, use secrets or apps, or message '
                      '$others. It works only in a folder of its own, with its '
                      'other extensions off, and the browser fills in secrets '
                      'only on their own sites.',
            action: on ? 'Stop errands' : 'Use for errands',
            busy: _busy,
            // Trish's words, as Agent settings' other refusals.
            error: _error == null
                ? null
                : "Couldn't change errands for $name. $_error",
            onCancel: () => setState(() {
              _asking = false;
              _error = null;
            }),
            onAction: () => unawaited(_set(!on)),
          )
        : null;
    final ask = Align(
      alignment: Alignment.centerLeft,
      child: Button(
        label: on ? 'Stop errands…' : 'Use for errands…',
        onPressed: () => setState(() => _asking = true),
      ),
    );
    if (on) {
      // The limit's note, as a pipe's and reading reports' have, above
      // the secrets and extensions it turns off.
      return Padding(
        padding: const EdgeInsets.only(bottom: Space.l),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Padding(padding: const EdgeInsets.only(top: 2), child: glyph),
                const SizedBox(width: Space.xs + 2),
                Expanded(
                  child: Text(
                    "$name runs errands, so it can't run commands, search the "
                    'web, use secrets or message $others.',
                    style: caption,
                  ),
                ),
              ],
            ),
            const SizedBox(height: Space.s),
            panel ?? ask,
          ],
        ),
      );
    }
    return Field(
      label: 'Errands',
      note: panel == null
          ? "$name can do things on websites for you in Claktory's browser."
          : null,
      child: panel ?? ask,
    );
  }
}

/// The question, in place: what changes, Cancel and the action; stacked on
/// a phone, the action the width of the panel and Cancel a link under it.
class _Panel extends StatelessWidget {
  const _Panel({
    required this.title,
    required this.body,
    required this.action,
    required this.busy,
    required this.error,
    required this.onCancel,
    required this.onAction,
  });

  final String title;
  final String body;
  final String action;
  final bool busy;
  final String? error;
  final VoidCallback onCancel;
  final VoidCallback onAction;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final error = this.error;
    final primary = Button(
      label: action,
      primary: true,
      onPressed: busy ? null : onAction,
    );
    return Container(
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Semantics(
            header: true,
            child: Text(
              title,
              style: type.emphasis.copyWith(color: palette.text),
            ),
          ),
          const SizedBox(height: Space.xs),
          Text(body, style: type.body.copyWith(color: palette.textMuted)),
          if (error != null) ...[
            const SizedBox(height: Space.s),
            Text(error, style: type.caption.copyWith(color: palette.danger)),
          ],
          const SizedBox(height: Space.m),
          if (isPhoneLayout(context)) ...[
            primary,
            Center(
              child: QuietLink(label: 'Cancel', onTap: busy ? () {} : onCancel),
            ),
          ] else
            Wrap(
              alignment: WrapAlignment.end,
              spacing: Space.s,
              runSpacing: Space.xs,
              children: [
                Button(label: 'Cancel', onPressed: busy ? null : onCancel),
                primary,
              ],
            ),
        ],
      ),
    );
  }
}
