import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../characters/character.dart';
import '../claktory_identity.dart' show claktoryName;
import '../controls.dart';
import '../devices.dart' show QuietLink;
import '../extensions.dart' show logTime;
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../markdown.dart' show CodeBlock;
import '../mobile/lines.dart';
import '../mobile/metrics.dart' show TypeScale;
import '../mobile/presentation.dart' show SheetAction, showActions;
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import '../tell_rivet.dart' show ErrorLine;
import 'browser_agent.dart';
import 'browser_session.dart';
import 'origin.dart';

/// Why the browser can't run on this computer, and what sets it up: the
/// command, whole and in monospace with Copy, or the NixOS lines.
class AgentBrowserSetupView extends StatelessWidget {
  const AgentBrowserSetupView({
    super.key,
    required this.setup,
    this.problem = true,
  });

  final AgentBrowserSetup setup;

  final bool problem;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (problem)
          Text(
            setup.problem.isEmpty
                ? "Claktory's browser can't run on this computer."
                : setup.problem,
            style: type.body.copyWith(color: palette.text),
          ),
        if (!setup.unsupported && setup.command.isNotEmpty) ...[
          if (problem) const SizedBox(height: Space.s),
          Text(
            // Linux's runs through sudo; a Mac's (brew, container) doesn't.
            setup.command.contains('sudo ')
                ? 'Run this once in a terminal on this computer. It asks for '
                      "your administrator's password."
                : 'Run this once in a terminal on this computer.',
            style: type.caption.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.xs),
          _Copyable(setup.command),
        ],
        if (!setup.unsupported && setup.nixosLines.isNotEmpty) ...[
          const SizedBox(height: Space.s),
          Text(
            'On NixOS, add these lines to your configuration, then rebuild.',
            style: type.caption.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.xs),
          _Copyable(setup.nixosLines),
        ],
      ],
    );
  }
}

class _Copyable extends StatefulWidget {
  const _Copyable(this.text);

  final String text;

  @override
  State<_Copyable> createState() => _CopyableState();
}

class _CopyableState extends State<_Copyable> {
  var _copied = false;

  @override
  Widget build(BuildContext context) => ActionLine(
    lead: CodeBlock(widget.text),
    action: Button(
      label: _copied ? 'Copied' : 'Copy',
      onPressed: () {
        unawaited(Clipboard.setData(ClipboardData(text: widget.text)));
        setState(() => _copied = true);
      },
    ),
  );
}

class AgentBrowserSettings extends StatelessWidget {
  const AgentBrowserSettings({super.key, required this.workspace});

  final Workspace workspace;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final browser = workspace.agentBrowser;
    final agents = [
      for (final agent in workspace.agents)
        if (hasBrowser(agent)) agent,
    ];
    Widget heading(String text) => Padding(
      padding: const EdgeInsets.only(top: Space.l, bottom: Space.xs),
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
        _Status(browser: browser),
        heading("Agents' browsers"),
        if (agents.isEmpty)
          Text(
            'No agent has the browser yet. Ask Rivet to give it to one, or '
            "turn on Browser in an agent's settings.",
            style: type.body.copyWith(color: palette.textMuted),
          )
        else
          for (final agent in agents)
            AgentBrowserRow(key: ValueKey(agent.id), agent: agent),
        heading('Time zone and languages'),
        _Locale(browser: browser),
        heading('Browser log'),
        ActionLine(
          lead: Text(
            'What agents did in their browsers',
            style: type.body.copyWith(color: palette.text),
          ),
          action: Button(
            label: 'Open',
            onPressed: () => showModal(
              context,
              (context, close) => BrowserLog(close: close),
            ),
          ),
        ),
      ],
    );
  }
}

class _Status extends StatefulWidget {
  const _Status({required this.browser});

  final AgentBrowser browser;

  @override
  State<_Status> createState() => _StatusState();
}

class _StatusState extends State<_Status> {
  var _unfolded = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final browser = widget.browser;
    final setup = browser.setup;
    if (browser.state == AgentBrowserState.AGENT_BROWSER_STATE_UNAVAILABLE) {
      if (setup.unsupported) {
        final name = claktoryName(AppScope.of(context));
        return Text(
          setup.problem.isEmpty
              ? "$name can't run it. It needs a Mac with Apple silicon and "
                    'macOS 26 or later.'
              : "$name can't run it. ${setup.problem}",
          style: type.body.copyWith(color: palette.text),
        );
      }
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Not set up yet. An agent sets it up the first time it needs it.',
            style: type.body.copyWith(color: palette.text),
          ),
          if (setup.command.isNotEmpty || setup.nixosLines.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            Align(
              alignment: AlignmentDirectional.centerStart,
              child: QuietLink(
                label: _unfolded ? 'Hide' : 'Set it up yourself',
                onTap: () => setState(() => _unfolded = !_unfolded),
              ),
            ),
            if (_unfolded) ...[
              const SizedBox(height: Space.s),
              AgentBrowserSetupView(setup: setup, problem: false),
            ],
          ],
        ],
      );
    }
    // Some Claktories send Chrome's product with it ("Chrome/158…").
    final version = browser.chromeVersion
        .replaceFirst(RegExp(r'^[A-Za-z]+/'), '')
        .split('.')
        .first;
    return Row(
      children: [
        Dot(
          color: browser.state == AgentBrowserState.AGENT_BROWSER_STATE_OFF
              ? palette.textFaint
              : palette.signal,
        ),
        const SizedBox(width: Space.s),
        Expanded(
          child: Text.rich(
            TextSpan(
              children: [
                TextSpan(
                  text: 'Ready',
                  style: type.body.copyWith(
                    color: palette.text,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                if (version.isNotEmpty)
                  TextSpan(
                    text: ' · Chrome $version',
                    style: type.body.copyWith(color: palette.textMuted),
                  ),
              ],
            ),
          ),
        ),
      ],
    );
  }
}

class AgentBrowserRow extends StatefulWidget {
  const AgentBrowserRow({super.key, required this.agent});

  final Agent agent;

  @override
  State<AgentBrowserRow> createState() => _AgentBrowserRowState();
}

class _AgentBrowserRowState extends State<AgentBrowserRow> {
  String? _error;
  var _busy = false;

  Future<void> _run(Future<void> Function() request) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await request();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    }
    if (mounted) setState(() => _busy = false);
  }

  Future<void> _actions(BuildContext context) async {
    final backend = AppScope.of(context);
    final agent = widget.agent;
    final workspace = backend.workspace.workspace;
    final session = AgentBrowserSession.of(backend);
    // "www.x.com" and "x.com" read as one site, so signing out clears both.
    final sites = <String, List<String>>{};
    for (final origin
        in browserOf(workspace, agent.id)?.signedInSites ?? const <String>[]) {
      (sites[siteName(origin)] ??= []).add(origin);
    }
    final action = await showActions<String>(
      context,
      title: agent.name,
      actions: [
        for (final host in sites.keys)
          SheetAction('Sign out of $host', 'site:$host'),
        const SheetAction('Delete browser data…', 'delete'),
        const SheetAction('Turn off browser…', 'off', destructive: true),
      ],
    );
    if (!context.mounted || action == null) return;
    if (action.startsWith('site:')) {
      final origins = sites[action.substring(5)] ?? const [];
      await _run(() async {
        for (final origin in origins) {
          await session.signOut(agentId: agent.id, site: origin);
        }
      });
    } else if (action == 'delete') {
      final sure = await showActions<bool>(
        context,
        title: "Delete ${agent.name}'s browser data?",
        message:
            '${agent.name} is signed out of every site, and the browser’s '
            'data is deleted. Your logins stay in Secrets.',
        actions: const [SheetAction('Delete', true, destructive: true)],
      );
      if (sure != true || !mounted) return;
      await _run(() => session.deleteData(agentId: agent.id));
    } else if (action == 'off') {
      await _run(() => turnOffBrowser(context, backend, agent));
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final agent = widget.agent;
    final hosts = signedInHosts(backend.workspace.workspace, agent.id);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.s),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              ExcludeSemantics(child: AgentCharacter(agent: agent, size: 28)),
              const SizedBox(width: Space.s),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      agent.name,
                      style: type.body.copyWith(
                        color: palette.text,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    Text(
                      signedInText(hosts),
                      style: type.caption.copyWith(color: palette.textMuted),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: Space.s),
              Builder(
                builder: (context) => GlyphButton(
                  label: "${agent.name}'s browser",
                  active: false,
                  onTap: () {
                    if (!_busy) unawaited(_actions(context));
                  },
                  glyph: (color) => ExtensionGlyph(
                    icon: 'ellipsis',
                    name: 'More',
                    color: color,
                  ),
                ),
              ),
            ],
          ),
          if (_error case final error?)
            ErrorLine(error, about: "Couldn't change ${agent.name}'s browser"),
        ],
      ),
    );
  }
}

class _Locale extends StatefulWidget {
  const _Locale({required this.browser});

  final AgentBrowser browser;

  @override
  State<_Locale> createState() => _LocaleState();
}

class _LocaleState extends State<_Locale> {
  String? _deviceZone;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    unawaited(_ask());
  }

  Future<void> _ask() async {
    final zone = await AgentBrowserSession.of(AppScope.of(context)).timeZone();
    if (mounted && zone != _deviceZone) setState(() => _deviceZone = zone);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final browser = widget.browser;
    final session = AgentBrowserSession.of(AppScope.of(context));
    final languages = browser.languages.isEmpty
        ? const ['en-US']
        : browser.languages.toList();
    final device =
        browser.timeZone.isNotEmpty &&
        browser.timeZone == _deviceZone &&
        _sameList(languages, session.languages());
    final words = [
      browser.timeZone.isEmpty ? "This computer's time zone" : browser.timeZone,
      languages.join(', '),
    ].join(' · ');
    return ActionLine(
      lead: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            device ? "This device's: $words" : words,
            style: type.body.copyWith(color: palette.text),
          ),
          Text(
            device
                ? 'Sites see these. They match this device, so the browser '
                      'looks like yours.'
                : 'Sites see these.',
            style: type.caption.copyWith(color: palette.textMuted),
          ),
        ],
      ),
      action: Button(
        label: 'Change…',
        onPressed: () => showModal(
          context,
          (context, close) =>
              AgentBrowserLocaleForm(browser: browser, close: close),
        ),
      ),
    );
  }
}

bool _sameList(List<String> a, List<String> b) =>
    a.length == b.length &&
    [for (var i = 0; i < a.length; i++) a[i] == b[i]].every((same) => same);

class BrowserLog extends StatefulWidget {
  const BrowserLog({super.key, required this.close});

  final VoidCallback close;

  @override
  State<BrowserLog> createState() => _BrowserLogState();
}

class _BrowserLogState extends State<BrowserLog> {
  var _agentId = '';
  ErrandLog? _log;
  String? _error;
  var _asked = 0;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_asked == 0) unawaited(_load());
  }

  Future<void> _load() async {
    final asked = ++_asked;
    final agentId = _agentId;
    try {
      final log = await AppScope.of(context).listErrandLog(agentId: agentId);
      if (mounted && asked == _asked) {
        setState(() {
          _log = log;
          _error = null;
        });
      }
    } on RequestFailure catch (failure) {
      if (mounted && asked == _asked) setState(() => _error = failure.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final store = AppScope.of(context).workspace;
    final workspace = store.workspace;
    final agents = [
      for (final agent in workspace.agents)
        if (hasBrowser(agent) || agent.id == _agentId) agent,
    ];
    return FormFrame(
      title: 'Browser log',
      body: 'What agents did in their browsers in the last 7 days.',
      error: null,
      submitLabel: 'Done',
      onSubmit: widget.close,
      onCancel: widget.close,
      fields: [
        if (agents.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(bottom: Space.m),
            child: Align(
              alignment: AlignmentDirectional.centerStart,
              child: SizedBox(
                width: 220,
                child: Select<String>(
                  title: 'Agent',
                  value: _agentId,
                  options: [
                    const SelectOption('', 'All agents'),
                    for (final agent in agents)
                      SelectOption(agent.id, agent.name),
                  ],
                  onChanged: (id) {
                    setState(() {
                      _agentId = id;
                      _log = null;
                    });
                    unawaited(_load());
                  },
                ),
              ),
            ),
          ),
        if (_error case final error?)
          ErrorLine(error, about: "Claktory couldn't show the browser log.")
        else if (_log case final log?)
          if (log.entries.isEmpty)
            Text(
              'Nothing done yet.',
              style: type.body.copyWith(color: palette.textMuted),
            )
          else
            for (final entry in log.entries.reversed)
              _LogRow(entry: entry, agent: store.agent(entry.agentId)),
      ],
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
    final who = entry.agentId.isEmpty ? 'You' : (agent?.name ?? 'An agent');
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

/// Claktory's browser's time zone and languages, set together: a time zone
/// typed or picked from the backend's list, and languages most preferred
/// first. Empty is not set: the computer's zone, en-US.
class AgentBrowserLocaleForm extends StatefulWidget {
  const AgentBrowserLocaleForm({
    super.key,
    required this.browser,
    required this.close,
    this.listTimeZones,
  });

  final AgentBrowser browser;
  final VoidCallback close;

  /// In tests, where the zones come from instead of the backend.
  @visibleForTesting
  final Future<List<String>> Function()? listTimeZones;

  @override
  State<AgentBrowserLocaleForm> createState() => _AgentBrowserLocaleFormState();
}

class _AgentBrowserLocaleFormState extends State<AgentBrowserLocaleForm> {
  late final _zone = TextEditingController(text: widget.browser.timeZone);
  final _zoneFocus = FocusNode(debugLabel: 'time zone');
  late final _languages = widget.browser.languages.toList();
  final _add = TextEditingController();
  List<String> _zones = const [];
  var _saving = false;
  String? _error;

  /// How many matching zones show under the field.
  static const _shown = 5;

  @override
  void initState() {
    super.initState();
    _zone.addListener(_changed);
    _zoneFocus.addListener(_changed);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_zones.isEmpty) unawaited(_loadZones());
  }

  Future<void> _loadZones() async {
    try {
      final zones =
          await (widget.listTimeZones ?? AppScope.of(context).listTimeZones)();
      if (mounted) setState(() => _zones = zones);
    } on RequestFailure {
      // Typing a zone still works; the backend checks it on Save.
    }
  }

  void _changed() => setState(() {});

  @override
  void dispose() {
    _zone.dispose();
    _zoneFocus.dispose();
    _add.dispose();
    super.dispose();
  }

  /// The zones matching what's typed: every word in the name, spaces as
  /// underscores ("new york" finds America/New_York).
  List<String> get _matches {
    final typed = _zone.text.trim().toLowerCase();
    if (typed.isEmpty) return const [];
    final words = typed.split(RegExp(r'\s+'));
    return [
      for (final zone in _zones)
        if (zone.toLowerCase() != typed &&
            words.every(
              (word) =>
                  zone.toLowerCase().replaceAll('_', ' ').contains(word) ||
                  zone.toLowerCase().contains(word),
            ))
          zone,
    ].take(_shown).toList();
  }

  void _addLanguage() {
    final tag = _add.text.trim();
    if (tag.isEmpty) return;
    setState(() {
      if (!_languages.contains(tag)) _languages.add(tag);
      _add.clear();
    });
  }

  void _move(int from, int by) => setState(() {
    final tag = _languages.removeAt(from);
    _languages.insert(from + by, tag);
  });

  /// This device's zone and languages, in place of what's there.
  Future<void> _useDevice() async {
    final session = AgentBrowserSession.of(AppScope.of(context));
    final zone = await session.timeZone();
    if (!mounted) return;
    setState(() {
      if (zone != null) _zone.text = zone;
      _languages
        ..clear()
        ..addAll(session.languages());
    });
  }

  Future<void> _save() async {
    if (_saving) return;
    _addLanguage();
    setState(() {
      _saving = true;
      _error = null;
    });
    try {
      await AgentBrowserSession.of(AppScope.of(context))
          .setLocale(_zone.text.trim(), _languages);
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
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final matches = _zoneFocus.hasFocus ? _matches : const <String>[];
    return FormFrame(
      title: 'Time zone and languages',
      body:
          "What sites see in Claktory's browser. A change applies the next "
          'time it starts.',
      error: _error,
      submitLabel: 'Save',
      onSubmit: _saving ? null : _save,
      onCancel: widget.close,
      fields: [
        Padding(
          padding: const EdgeInsets.only(bottom: Space.l),
          child: Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(label: "Use this device's", onPressed: _useDevice),
          ),
        ),
        Field(
          label: 'Time zone',
          note: "Empty uses this computer's.",
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              TextField(
                controller: _zone,
                focusNode: _zoneFocus,
                placeholder: 'Europe/London',
                autocorrect: false,
                onSubmitted: (_) => _save(),
              ),
              // Part of the field's tap region, so a click on one doesn't
              // take the field's focus, and the list, first.
              for (final zone in matches)
                TextFieldTapRegion(
                  child: Pressable(
                    onTap: () {
                      _zone.text = zone;
                      _zoneFocus.unfocus();
                    },
                    builder: (context, hovered) => Container(
                      height: 28,
                      padding: const EdgeInsets.symmetric(horizontal: Space.s),
                      color: hovered ? palette.hover : null,
                      alignment: Alignment.centerLeft,
                      child: Text(
                        zone,
                        style: type.body.copyWith(color: palette.text),
                      ),
                    ),
                  ),
                ),
            ],
          ),
        ),
        Field(
          label: 'Languages',
          note: 'Most preferred first, at most 8. Empty uses en-US.',
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final (i, tag) in _languages.indexed)
                SizedBox(
                  height: 32,
                  child: Row(
                    children: [
                      Expanded(
                        child: Text(
                          tag,
                          style: type.body.copyWith(color: palette.text),
                        ),
                      ),
                      // Each in its own place, shown or not, so the
                      // rows' controls line up.
                      _slot(
                        i > 0,
                        _glyph(
                          'chevron-up',
                          'Move $tag up',
                          () => _move(i, -1),
                        ),
                      ),
                      _slot(
                        i < _languages.length - 1,
                        _glyph(
                          'chevron-down',
                          'Move $tag down',
                          () => _move(i, 1),
                        ),
                      ),
                      _glyph(
                        'x',
                        'Remove $tag',
                        () => setState(() => _languages.removeAt(i)),
                      ),
                    ],
                  ),
                ),
              if (_languages.length < 8) ...[
                const SizedBox(height: Space.xs),
                ActionLine(
                  lead: TextField(
                    controller: _add,
                    placeholder: 'A language, such as en-GB or fr',
                    autocorrect: false,
                    onSubmitted: (_) => _addLanguage(),
                  ),
                  action: Button(label: 'Add', onPressed: _addLanguage),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }

  Widget _slot(bool shown, Widget child) => Visibility(
    visible: shown,
    maintainSize: true,
    maintainAnimation: true,
    maintainState: true,
    child: child,
  );

  Widget _glyph(String icon, String label, VoidCallback onTap) => GlyphButton(
    label: label,
    active: false,
    onTap: onTap,
    glyph: (color) => ExtensionGlyph(icon: icon, name: label, color: color),
  );
}
