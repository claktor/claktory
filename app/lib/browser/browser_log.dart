import 'dart:async';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../characters/character.dart';
import '../controls.dart';
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../mobile/metrics.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import '../settings_view.dart' show SettingRequests;
import '../tell_rivet.dart' show ErrorLine;
import '../usage.dart' show formatAge;
import 'session.dart' show browserEndReason, browserHost;

// B3b's log in Settings → Browser: what agents did with browser control in
// the last 7 days, newest first (who, which site, on which device, what it
// did and how that ended), never what was typed or read; and Clear log….
// Only where the backend has browser control (its preview lists the
// extension). Asked for each time it shows, and again as sessions change;
// nothing is kept on the device.

/// How many entries a page asks for.
const browserLogPage = 100;

/// Whether the backend has browser control: a backend without it (its
/// preview off) has no such extension.
bool browserControlAvailable(Workspace workspace) =>
    workspace.extensions.any((extension) => extension.id == 'browser');

/// What an entry says happened. [agent] is the agent's name, [device] the
/// device's, '' when not known.
String browserLogWhat(
  BrowserLogEntry entry, {
  String agent = 'The agent',
  String device = '',
}) => switch (entry.op) {
  'request' when entry.outcome == 'assist' =>
    'Asked to read and use ${browserHost(entry.origin)}',
  'request' => 'Asked to read ${browserHost(entry.origin)}',
  'grant' => device.isEmpty ? 'You allowed it' : 'You allowed it on $device',
  'end' => 'Ended: ${browserEndReason(entry.outcome, agent: agent)}',
  'read' => 'Read the page',
  'scroll' => 'Scrolled',
  'click' => 'Clicked',
  'fill' => 'Filled in a field',
  'navigate' => 'Went to another page',
  _ => 'Did something else',
};

/// How a command came out, when it isn't simply done; "" for done, and for
/// requests, grants and ends, which [browserLogWhat] says in full.
String browserLogOutcome(BrowserLogEntry entry) {
  if (const {'request', 'grant', 'end'}.contains(entry.op)) return '';
  return switch (entry.outcome) {
    '' || 'done' => '',
    'denied' => 'You didn’t allow it',
    'expired' => 'Already ended',
    'approval_pending' => 'Not allowed yet',
    'paused' => 'Paused',
    'stale_target' || 'stale_document' => 'The page had changed',
    'origin_changed' => 'The tab was on another site',
    'unsupported_page' || 'unsupported_target' => 'Not possible on that page',
    'sensitive_action' => 'Blocked: too sensitive',
    'limit_exceeded' => 'Over its limit',
    'interrupted' => 'Interrupted',
    'outcome_unknown' => 'Not sure it happened',
    _ => 'Didn’t work',
  };
}

/// Settings → Browser: the log, an agent filter, Show earlier and Clear
/// log….
class BrowserLogSettings extends StatefulWidget {
  const BrowserLogSettings({super.key, required this.workspace});

  final Workspace workspace;

  @override
  State<BrowserLogSettings> createState() => _BrowserLogSettingsState();
}

class _BrowserLogSettingsState extends State<BrowserLogSettings>
    with SettingRequests, AutomaticKeepAliveClientMixin {
  /// Settings builds its sections as they scroll into view: this one keeps
  /// its agent and pages while Settings is open.
  @override
  bool get wantKeepAlive => true;

  late final Backend _backend = AppScope.of(context);

  /// Only this agent's; '' for every agent's.
  var _agent = '';

  var _entries = <BrowserLogEntry>[];
  var _more = false;
  var _loading = true;
  String? _failed;

  /// Which ask is the latest: an answer to an older one is dropped.
  var _asked = 0;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _load();
    });
  }

  @override
  void didUpdateWidget(BrowserLogSettings old) {
    super.didUpdateWidget(old);
    // Sessions moved on: what's shown, again, with what's new.
    if (_sessions(old.workspace) != _sessions(widget.workspace)) {
      _load(keep: true);
    }
  }

  /// What the log follows of the sessions: each one's state and commands.
  static String _sessions(Workspace workspace) => [
    for (final session in workspace.browserSessions)
      '${session.id}/${session.state}/${session.commandsUsed}/${session.action}',
  ].join(',');

  /// The newest page; [earlier], the page before what's shown; [keep], as
  /// many as are shown.
  Future<void> _load({bool earlier = false, bool keep = false}) async {
    final asked = ++_asked;
    setState(() {
      _loading = true;
      _failed = null;
    });
    try {
      final reply = await _backend.listBrowserLog(
        ListBrowserLog(
          agentId: _agent,
          beforeMs: earlier && _entries.isNotEmpty
              ? _entries.last.atMs
              : Int64.ZERO,
          beforeId: earlier && _entries.isNotEmpty
              ? _entries.last.id
              : Int64.ZERO,
          limit: keep && _entries.length > browserLogPage
              ? _entries.length.clamp(browserLogPage, 500)
              : browserLogPage,
        ),
      );
      if (!mounted || asked != _asked) return;
      setState(() {
        _entries = earlier ? [..._entries, ...reply.entries] : reply.entries;
        _more = reply.more;
        _loading = false;
      });
    } on RequestFailure catch (error) {
      if (!mounted || asked != _asked) return;
      setState(() {
        _failed = error.message;
        _loading = false;
      });
    }
  }

  void _choose(String agent) {
    if (agent == _agent) return;
    setState(() {
      _agent = agent;
      _entries = [];
      _more = false;
    });
    _load();
  }

  void _clear() => showModal(
    context,
    (context, close) => FormFrame(
      title: 'Clear the browser log?',
      body:
          'This deletes the record of what every agent did in Chrome, on '
          'every device.',
      fields: const [],
      error: null,
      submitLabel: 'Clear',
      onSubmit: () {
        close();
        run(() async {
          await _backend.clearBrowserLog();
          if (mounted) await _load();
        });
      },
      onCancel: close,
    ),
  );

  @override
  Widget build(BuildContext context) {
    super.build(context);
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = widget.workspace;
    final agents = [...workspace.agents]
      ..sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
    final faint = type.body.copyWith(color: palette.textFaint);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'What agents did in Chrome over the last 7 days. What was typed '
          'and what pages said are never kept.',
          style: type.body.copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.m),
        Wrap(
          spacing: Space.s,
          runSpacing: Space.s,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 240),
              child: Select<String>(
                title: 'Show',
                value: _agent,
                options: [
                  const SelectOption('', 'All agents'),
                  for (final agent in agents)
                    SelectOption(agent.id, agent.name),
                  if (_agent.isNotEmpty &&
                      !agents.any((agent) => agent.id == _agent))
                    SelectOption(_agent, _agent),
                ],
                onChanged: _choose,
              ),
            ),
            Button(
              label: 'Clear log…',
              onPressed: _entries.isEmpty ? null : _clear,
            ),
          ],
        ),
        const SizedBox(height: Space.m),
        if (_failed case final why? when _entries.isEmpty)
          Wrap(
            spacing: Space.s,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              ErrorLine(why, about: 'Settings → Browser:'),
              Button(label: 'Try again', onPressed: _load),
            ],
          )
        else if (_entries.isEmpty)
          Text(
            _loading ? 'Loading…' : 'Nothing in the last 7 days.',
            style: faint,
          ),
        for (final entry in _entries)
          KeyedSubtree(
            key: ValueKey(entry.id),
            child: BrowserLogRow(entry: entry, workspace: workspace),
          ),
        if (_entries.isNotEmpty && _more)
          Padding(
            padding: const EdgeInsets.only(top: Space.s),
            child: Align(
              alignment: Alignment.centerLeft,
              child: _loading
                  ? Text('Loading…', style: faint)
                  : Button(
                      label: 'Show earlier',
                      onPressed: () => _load(earlier: true),
                    ),
            ),
          ),
        if (_entries.isNotEmpty && _failed != null)
          ErrorLine(_failed!, about: 'Settings → Browser:'),
        failureLine(palette),
      ],
    );
  }
}

/// One entry: the agent (its character and name) and when, what it did,
/// how that came out when it wasn't simply done, and the site and device.
class BrowserLogRow extends StatelessWidget {
  const BrowserLogRow({
    super.key,
    required this.entry,
    required this.workspace,
  });

  final BrowserLogEntry entry;
  final Workspace workspace;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final agent = workspace.agents
        .where((agent) => agent.id == entry.agentId)
        .firstOrNull;
    final device = workspace.remote.devices
        .where((device) => device.id == entry.deviceId)
        .firstOrNull
        ?.name;
    final outcome = browserLogOutcome(entry);
    final at = entry.atMs.toInt();
    final where = [
      if (entry.op != 'request') browserHost(entry.origin),
      if (entry.op != 'grant' && device != null) device,
    ].join(' · ');
    return Container(
      constraints: BoxConstraints(
        minHeight: isPhoneLayout(context) ? Touch.target : 44,
      ),
      padding: const EdgeInsets.symmetric(vertical: Space.s),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox.square(
            dimension: 24,
            child: Center(
              child: agent != null
                  ? AgentCharacter(agent: agent, size: 24)
                  : ExtensionGlyph(
                      icon: 'globe',
                      name: '',
                      color: palette.textFaint,
                    ),
            ),
          ),
          const SizedBox(width: Space.s + 2),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Ticking(
                  active: at > 0,
                  builder: (context, now) => Text.rich(
                    TextSpan(
                      text: agent?.name ?? 'An agent that’s gone',
                      style: type.body.copyWith(
                        color: palette.text,
                        fontWeight: FontWeight.w500,
                      ),
                      children: [
                        if (at > 0)
                          TextSpan(
                            text: ' · ${formatAge(at, now)}',
                            style: type.caption.copyWith(
                              color: palette.textFaint,
                              fontWeight: FontWeight.w400,
                            ),
                          ),
                      ],
                    ),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                Text.rich(
                  TextSpan(
                    text: browserLogWhat(
                      entry,
                      agent: agent?.name ?? 'The agent',
                      device: device ?? '',
                    ),
                    children: [
                      if (outcome.isNotEmpty)
                        TextSpan(
                          text: ' · $outcome',
                          style: TextStyle(
                            color: entry.outcome == 'outcome_unknown'
                                ? palette.danger
                                : palette.textMuted,
                          ),
                        ),
                    ],
                  ),
                  style: type.body.copyWith(color: palette.text),
                ),
                if (where.isNotEmpty)
                  Text(
                    where,
                    style: type.caption.copyWith(color: palette.textFaint),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
