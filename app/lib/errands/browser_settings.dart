import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../controls.dart';
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../markdown.dart' show CodeBlock;
import '../mobile/lines.dart';
import '../mobile/metrics.dart' show TypeScale;
import '../mobile/presentation.dart' show SheetAction, showActions;
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import '../tell_rivet.dart' show ErrorLine;
import 'browser_session.dart';
import 'errand_panel.dart';
import 'origin.dart';

// Settings → Claktory's browser (#435 P5, agent-browser.md R8): whether
// it can run here (and the one command, or NixOS lines, that sets it up),
// how it is, Close browser while it runs, the time zone and languages its pages see (#435 change 3), the
// identity providers it's signed in to, each with Sign out, and the sites
// (their addresses, never their cookies), its
// errands of the last 7 days with their logs, and Delete
// browser data. Only where the backend has it.

/// What a state says, in a few words.
String agentBrowserStateLabel(AgentBrowser browser) => switch (browser.state) {
  AgentBrowserState.AGENT_BROWSER_STATE_OFF => 'Closed',
  AgentBrowserState.AGENT_BROWSER_STATE_STARTING => 'Starting',
  AgentBrowserState.AGENT_BROWSER_STATE_WARM => 'Open, not in use',
  AgentBrowserState.AGENT_BROWSER_STATE_DRIVEN => 'An agent is using it',
  AgentBrowserState.AGENT_BROWSER_STATE_PERSONS_TURN => 'Waiting for you',
  AgentBrowserState.AGENT_BROWSER_STATE_TAKEN_OVER => 'You took over',
  AgentBrowserState.AGENT_BROWSER_STATE_BROWSING => 'Yours to use',
  AgentBrowserState.AGENT_BROWSER_STATE_STOPPING => 'Closing',
  AgentBrowserState.AGENT_BROWSER_STATE_UNAVAILABLE => 'Not set up',
  _ => '',
};

/// Why the browser can't run on this computer, and what sets it up: the
/// command, whole and in monospace with Copy, or the NixOS lines.
class AgentBrowserSetupView extends StatelessWidget {
  const AgentBrowserSetupView({super.key, required this.setup});

  final AgentBrowserSetup setup;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          setup.problem.isEmpty
              ? "Claktory's browser can't run on this computer."
              : setup.problem,
          style: type.body.copyWith(color: palette.text),
        ),
        if (!setup.unsupported && setup.command.isNotEmpty) ...[
          const SizedBox(height: Space.s),
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

/// Settings → Claktory's browser.
class AgentBrowserSettings extends StatefulWidget {
  const AgentBrowserSettings({super.key, required this.workspace});

  final Workspace workspace;

  @override
  State<AgentBrowserSettings> createState() => _AgentBrowserSettingsState();
}

class _AgentBrowserSettingsState extends State<AgentBrowserSettings> {
  String? _error;
  var _deleting = false;

  Future<void> _delete() async {
    final sure = await showActions<bool>(
      context,
      title: "Delete Claktory's browser data?",
      message:
          'This signs it out of every site and deletes its history, and '
          'stops any errand using it. Agents will need you to sign in '
          'again.',
      actions: const [SheetAction('Delete', true, destructive: true)],
    );
    if (sure != true || !mounted) return;
    setState(() {
      _deleting = true;
      _error = null;
    });
    try {
      await AgentBrowserSession.of(AppScope.of(context)).deleteData();
    } on RequestFailure catch (error) {
      if (mounted) setState(() => _error = error.message);
    }
    if (mounted) setState(() => _deleting = false);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final workspace = widget.workspace;
    final browser = workspace.agentBrowser;
    final unavailable =
        browser.state == AgentBrowserState.AGENT_BROWSER_STATE_UNAVAILABLE;
    final errands = workspace.errands.toList()
      ..sort((a, b) => b.createdMs.compareTo(a.createdMs));
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
        Text(
          'A browser of its own, on this Claktory, that agents use for '
          "errands on the web. You don't use it yourself: when an errand "
          'needs something only you have, like a code, it asks you in '
          'Needs you.',
          style: type.body.copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.m),
        if (unavailable)
          AgentBrowserSetupView(setup: browser.setup)
        else
          ActionLine(
            lead: Text(
              [
                agentBrowserStateLabel(browser),
                if (browser.chromeVersion.isNotEmpty)
                  // The number alone, as the protocol says, even from a
                  // Claktory that sends Chrome's product ("Chrome/158…":
                  // Abbacchio's nit, "Chrome Chrome/158").
                  'Chrome ${browser.chromeVersion.replaceFirst(RegExp(r'^[A-Za-z]+/'), '')}',
              ].join(' · '),
              style: type.body.copyWith(color: palette.text),
            ),
            action: agentBrowserRunning(browser.state)
                ? Button(
                    label: 'Close browser',
                    onPressed: AgentBrowserSession.of(backend).stop,
                  )
                : const SizedBox.shrink(),
          ),
        heading('Time zone and languages'),
        ActionLine(
          lead: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                browser.timeZone.isEmpty
                    ? "Time zone: this computer's"
                    : 'Time zone: ${browser.timeZone}',
                style: type.body.copyWith(color: palette.text),
              ),
              Text(
                browser.languages.isEmpty
                    ? 'Languages: en-US'
                    : 'Languages: ${browser.languages.join(', ')}',
                style: type.body.copyWith(color: palette.text),
              ),
              Text(
                'What sites see in it. A change applies the next time it '
                'starts.',
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
        ),
        heading('Signed in to'),
        for (final account in browser.accounts)
          Padding(
            padding: const EdgeInsets.only(bottom: Space.xs),
            child: AgentBrowserAccountLine(
              key: ValueKey(account.provider),
              account: account,
            ),
          ),
        if (browser.signedInSites.isEmpty && browser.accounts.isEmpty)
          Text(
            'No sites yet.',
            style: type.body.copyWith(color: palette.textMuted),
          )
        else
          for (final site in browser.signedInSites)
            Text(
              originHost(site),
              style: type.body.copyWith(color: palette.text),
            ),
        if (errands.isNotEmpty) ...[
          heading('Errands in the last 7 days'),
          for (final errand in errands)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.xs),
              child: ErrandLine(
                errand: errand,
                agent: backend.workspace.agent(errand.agentId),
              ),
            ),
        ],
        const SizedBox(height: Space.l),
        if (_error case final error?) ...[
          ErrorLine(
            error,
            about: "Claktory couldn't delete the browser's data.",
          ),
          const SizedBox(height: Space.s),
        ],
        ActionLine(
          lead: Text(
            'Signs it out of every site and deletes its history.',
            style: type.caption.copyWith(color: palette.textMuted),
          ),
          action: Button(
            label: 'Delete browser data…',
            onPressed: _deleting ? null : _delete,
          ),
        ),
      ],
    );
  }
}

/// An identity provider Claktory's browser is signed in to (#435): "Google
/// as Work Gmail", the vault secret it signed in with, and Sign out; while
/// a sign-out waits for the errand using the browser, it says so instead.
class AgentBrowserAccountLine extends StatefulWidget {
  const AgentBrowserAccountLine({super.key, required this.account});

  final AgentBrowserAccount account;

  @override
  State<AgentBrowserAccountLine> createState() =>
      _AgentBrowserAccountLineState();
}

class _AgentBrowserAccountLineState extends State<AgentBrowserAccountLine> {
  var _sending = false;
  String? _error;

  String get _provider => widget.account.name.isEmpty
      ? widget.account.provider
      : widget.account.name;

  Future<void> _signOut() async {
    final provider = _provider;
    final sure = await showActions<bool>(
      context,
      title: "Sign Claktory's browser out of $provider?",
      message:
          'Agents will need your help to sign in to $provider again. If an '
          'errand is using the browser, it signs out when the errand ends.',
      actions: const [SheetAction('Sign out', true, destructive: true)],
    );
    if (sure != true || !mounted) return;
    setState(() {
      _sending = true;
      _error = null;
    });
    try {
      await AgentBrowserSession.of(AppScope.of(context))
          .signOut(widget.account.provider);
    } on RequestFailure catch (error) {
      if (mounted) setState(() => _error = error.message);
    }
    if (mounted) setState(() => _sending = false);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final account = widget.account;
    final name = account.secretName;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ActionLine(
          lead: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                name.isEmpty ? _provider : '$_provider as $name',
                style: type.body.copyWith(color: palette.text),
              ),
              if (account.signingOut)
                Text(
                  'Signing out when the errand using the browser ends.',
                  style: type.caption.copyWith(color: palette.textMuted),
                ),
            ],
          ),
          action: account.signingOut
              ? const SizedBox.shrink()
              : Button(
                  label: 'Sign out',
                  onPressed: _sending ? null : _signOut,
                ),
        ),
        if (_error case final error?)
          ErrorLine(
            error,
            about: "Claktory couldn't sign the browser out of $_provider.",
          ),
      ],
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
