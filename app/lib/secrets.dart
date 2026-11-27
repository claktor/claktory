import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'account.dart' show signInPutOff;
import 'apps.dart' show appOfGrantee;
import 'avatar.dart';
import 'backend.dart';
import 'card_answer.dart';
import 'characters/character.dart';
import 'claktory_identity.dart' show claktoryNameInText;
import 'connectors.dart';
import 'controls.dart';
import 'decision.dart';
import 'errands/browser_agent.dart' show browserRefusal;
import 'extensions.dart' show logTime;
import 'forms.dart';
import 'identity.dart';
import 'mobile/drawer.dart' show largeText;
import 'mobile/metrics.dart' show TypeScale;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'mobile/input.dart' show InputKind;
import 'loopback_forward.dart' show openLink;
import 'markdown.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';
import 'resources.dart' show formatBytes;
import 'usage.dart' show formatAge;
import 'usage_wait.dart' show atWork, usageWaitLine;
import 'visible_codes.dart' show hiddenRune, proseCodes, runeCode;
import 'recovery_key.dart' show AccountRecoveryKey;
import 'settings_shell.dart' show SettingAnchor;

/// How long a value shown stays on screen.
const secretShownFor = Duration(seconds: 30);

/// How long a copied value stays on the clipboard.
const secretCopiedFor = Duration(minutes: 1);

VaultState _vault(Backend backend) => backend.workspace.workspace.vault;

/// A secret's description asks what it grants: agents, the Map and the
/// Secrets panel show it, and a vague one ("a Cloudflare secret") reads as
/// more than it is.
const secretGrantsLabel = 'What it can do';
const secretGrantsNote =
    'Say exactly what it grants: its permissions and what they reach. Agents '
    'and the Map show it.';
const secretGrantsExample = 'Cloudflare: DNS Edit on claktory.com';

/// An agent's name, or an app's for a secret given to one ("app:telegram"),
/// or the id when the app doesn't know it.
String _agentName(Backend backend, String id) =>
    backend.workspace.agent(id)?.name ??
    appOfGrantee(backend.workspace.workspace, id)?.name ??
    id;

/// Settings → Secrets: the secrets with what's given to whom, and adding
/// them (agents' requests show only in their conversations). The backend keeps
/// them encrypted from the start; there's nothing to set up or unlock.
class SecretsSettings extends StatelessWidget {
  const SecretsSettings({super.key});

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final palette = Palette.of(context);
        final vault = _vault(backend);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (vault.exists && !vault.unlocked) ...[
              _Locked(
                account: backend.workspace.workspace.hasAccount()
                    ? backend.workspace.workspace.account
                    : null,
                why: vault.lockedWhy,
              ),
              const SizedBox(height: Space.l),
            ],
            // The account's recovery key (#239): covered, and a new one
            // to use here only if the person made it. Claktory's alert
            // sends them here.
            const SettingAnchor(
              id: 'recovery-key',
              child: AccountRecoveryKey(),
            ),
            // Agents' proposals to connect a service, the same way.
            for (final proposal in vault.proposals)
              Padding(
                padding: const EdgeInsets.only(bottom: Space.l),
                child: ConnectProposalCard(
                  proposal,
                  key: ValueKey(proposal.id),
                ),
              ),
            const OtherConnectFlows(),
            if (vault.secrets.isEmpty)
              Text(
                'No secrets yet.',
                style: TypeScale.of(context).body
                    .copyWith(color: palette.textFaint),
              )
            else
              for (final secret in vault.secrets)
                _SecretRow(key: ValueKey(secret.id), secret: secret),
            const SizedBox(height: Space.s),
            Wrap(
              spacing: Space.s,
              runSpacing: Space.s,
              children: [
                if (vault.connectors.isNotEmpty)
                  Button(
                    label: 'Connect a service…',
                    onPressed: () => showModal(
                      context,
                      (context, close) => ConnectServiceForm(close: close),
                    ),
                  ),
                Button(
                  label: 'Add a secret…',
                  onPressed: () => showModal(
                    context,
                    (context, close) => SecretForm(close: close),
                  ),
                ),
                Button(
                  label: 'Paste .env…',
                  onPressed: () => showModal(
                    context,
                    (context, close) => EnvForm(close: close),
                  ),
                ),
              ],
            ),
            const SizedBox(height: Space.l),
            Text(
              'Agents use secrets without seeing them, and Claktory hides '
              'them in everything agents read. An agent you give a secret to '
              'could still find a way to reveal it, so give each agent only '
              'what it needs.',
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textFaint),
            ),
          ],
        );
      },
    );
  }
}

/// Locked since a restart, and why: the backend's words
/// (VaultState.locked_why: claktory.com can't be reached, the account no
/// longer opens them, they open with the passphrase…), or that this
/// Claktory was removed from the account. The recovery key opens them
/// meanwhile.
class _Locked extends StatelessWidget {
  const _Locked({required this.account, this.why = ''});

  /// This Claktory's account, when it has one.
  final ClaktoryAccount? account;

  /// Why they're locked, in the backend's words; empty from a backend
  /// before it said.
  final String why;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final account = this.account;
    final removed =
        account?.state == ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_REVOKED;
    // Covered by the account's key (#239): that's the one that opens them
    // (Abbacchio's row 1727).
    final covered = AppScope.of(context)
        .workspace
        .workspace
        .vault
        .recoveryKeyCovered;
    final why = removed || this.why.isEmpty
        ? null
        : 'Secrets are locked. ${this.why}';
    final said =
        why ??
        (account == null
            ? "Secrets are locked. Agents can't use them until you unlock them."
            : removed
            ? 'Secrets are locked because this Claktory was removed from your '
                  'account. Sign in again to unlock them, or use '
                  '${covered ? "your account's" : "this Claktory's"} recovery '
                  'key.'
            // Signed in: the backend's own words; none while it's asking.
            : 'Secrets are locked. '
                  '${account.problem.isEmpty ? 'Opening them…' : account.problem}');
    return Container(
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        color: palette.signal.withValues(alpha: 0.06),
        border: Border.all(color: palette.signal.withValues(alpha: 0.4)),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            said,
            style: TypeScale.of(context).body.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.m),
          Wrap(
            spacing: Space.s,
            runSpacing: Space.s,
            children: [
              // Signing in again also opens them by themselves after later
              // restarts, so it comes first.
              if (removed)
                Button(
                  label: 'Sign in again…',
                  primary: true,
                  onPressed: () =>
                      signInPutOff(AppScope.of(context)).value = false,
                ),
              Button(
                label: removed
                    ? 'Use recovery key…'
                    : 'Unlock with recovery key…',
                onPressed: () => showModal(
                  context,
                  (context, close) => UnlockSecretsForm(close: close),
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

/// "Unlock with recovery key": the key, and the backend's word when it
/// isn't right.
class UnlockSecretsForm extends StatefulWidget {
  const UnlockSecretsForm({super.key, required this.close});

  final VoidCallback close;

  @override
  State<UnlockSecretsForm> createState() => _UnlockSecretsFormState();
}

class _UnlockSecretsFormState extends State<UnlockSecretsForm> {
  final _key = TextEditingController();
  var _busy = false;
  String? _error;

  @override
  void dispose() {
    _key.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    if (_busy || _key.text.trim().isEmpty) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).unlockVault(_key.text.trim());
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => FormFrame(
    title: 'Unlock with recovery key',
    // Covered by the account's key (#239), that one opens them; else each
    // Claktory has its own (Abbacchio's row 1727).
    body: AppScope.of(context).workspace.workspace.vault.recoveryKeyCovered
        ? "Enter your account's recovery key. It opens the secrets on "
              '${claktoryNameInText(AppScope.of(context))}.'
        : 'Enter the recovery key saved for '
              '${claktoryNameInText(AppScope.of(context))}. Each Claktory has '
              'its own.',
    fields: [
      Field(
        label: 'Recovery key',
        child: TextField(
          controller: _key,
          autofocus: true,
          // Typed or pasted as it is.
          autocorrect: false,
          textCapitalization: TextCapitalization.none,
          onSubmitted: (_) => _submit(),
        ),
      ),
    ],
    error: _error,
    submitLabel: _busy ? 'Unlocking…' : 'Unlock',
    onSubmit: _busy ? null : _submit,
    onCancel: widget.close,
  );
}

/// A toggle with its label (and a note under it) beside it.
class _ToggleRow extends StatelessWidget {
  const _ToggleRow({
    required this.label,
    required this.value,
    required this.onChanged,
    this.note,
    this.limits = const [],
  });

  final String label;
  final String? note;

  /// What limits its use, under the note (an errand agent's, row 1711).
  final List<String> limits;
  final bool value;
  final ValueChanged<bool>? onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ToggleTap(
      value: value,
      onChanged: onChanged,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 2),
            child: Toggle(value: value, label: label, onChanged: onChanged),
          ),
          const SizedBox(width: Space.m),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                // Muted while it can't be changed.
                Text(
                  label,
                  style: TypeScale.of(context).body.copyWith(
                    color: onChanged == null ? palette.textMuted : palette.text,
                  ),
                ),
                if (note != null)
                  Text(
                    note!,
                    style: TypeScale.of(context).caption.copyWith(
                      color: onChanged == null
                          ? palette.textFaint
                          : palette.textMuted,
                    ),
                  ),
                for (final limit in limits)
                  Text(
                    limit,
                    style: TypeScale.of(context).caption
                        .copyWith(color: palette.textMuted),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// The switch that makes every command using a secret wait for the
/// person's Allow (SecretInfo.ask_each_time), and what it does.
const askEachTimeLabel = 'Ask each time';
const askEachTimeNote =
    "Each command that uses it waits until you allow it. Apps can't use "
    'it.';

/// One secret: its name, what it's for, whom it's given to and when it was
/// last used. Opened, its values (hidden, or shown for 30 seconds), Copy,
/// giving and taking back, Edit, Delete, and its recent use.
class _SecretRow extends StatefulWidget {
  const _SecretRow({super.key, required this.secret});

  final SecretInfo secret;

  @override
  State<_SecretRow> createState() => _SecretRowState();
}

class _SecretRowState extends State<_SecretRow> {
  var _open = false;

  /// The values while they're shown, by variable; never kept beyond that.
  Map<String, String>? _shown;
  DateTime? _shownAt;
  Timer? _hide;

  /// What happened last ("Copied…"), or why it couldn't.
  String? _note;
  var _noteIsError = false;

  List<SecretUse>? _uses;

  /// Ask each time is being set.
  var _setting = false;

  @override
  void dispose() {
    _hide?.cancel();
    _shown = null;
    _site.dispose();
    super.dispose();
  }

  SecretInfo get secret => widget.secret;

  void _say(String? note, {bool error = false}) => setState(() {
    _note = note;
    _noteIsError = error;
  });

  Future<void> _toggle() async {
    setState(() => _open = !_open);
    if (_open) await _loadUses();
  }

  Future<void> _loadUses() async {
    try {
      final uses = await AppScope.of(context)
          .listSecretUses(secret.id, limit: 10);
      if (mounted) setState(() => _uses = uses);
    } on RequestFailure {
      if (mounted) setState(() => _uses = const []);
    }
  }

  /// The values, fetched as the person.
  Future<Map<String, String>?> _fetch() async {
    final backend = AppScope.of(context);
    Map<String, String>? values;
    try {
      final entries = await backend.revealSecret(secret.id);
      values = {for (final entry in entries) entry.variable: entry.value};
    } on RequestFailure catch (failure) {
      if (mounted) _say(failure.message, error: true);
    }
    return values;
  }

  Future<void> _show() async {
    final values = await _fetch();
    if (values == null || !mounted) return;
    _hide?.cancel();
    setState(() {
      _shown = values;
      _shownAt = DateTime.now();
      _note = null;
    });
    _hide = Timer(secretShownFor, _hideValues);
  }

  void _hideValues() {
    _hide?.cancel();
    if (mounted) {
      setState(() {
        _shown = null;
        _shownAt = null;
      });
    }
  }

  Future<void> _copy(String variable) async {
    final value = (_shown ?? await _fetch())?[variable];
    if (value == null || !mounted) return;
    await copySecret(value);
    if (mounted) _say('Copied. The clipboard clears in a minute.');
  }

  Future<void> _askEachTime(bool on) async {
    setState(() {
      _setting = true;
      _note = null;
    });
    try {
      await AppScope.of(context).setSecretAskEachTime(secret.id, on);
    } on RequestFailure catch (failure) {
      if (mounted) _say(failure.message, error: true);
    } finally {
      if (mounted) setState(() => _setting = false);
    }
  }

  /// Gives it to an agent: in full, or with [fillOnly] only for Claktory's
  /// browser to fill in on its sites (#435). Either replaces the other, so
  /// an agent that has it the other way can be picked.
  Future<void> _give({bool fillOnly = false}) async {
    final backend = AppScope.of(context);
    final given = {
      for (final grant in secret.grants)
        if (grant.fillOnly == fillOnly) grant.agentId,
    };
    final agents = [
      for (final agent in backend.workspace.workspace.agents)
        if (!given.contains(agent.id)) agent,
    ];
    if (agents.isEmpty) {
      _say('Every agent has it already.');
      return;
    }
    Agent? agent;
    await showModal(
      context,
      (context, close) => _AgentPicker(
        title: fillOnly
            ? 'Whose browser may fill in ${secret.name}?'
            : 'Give ${secret.name} to',
        agents: agents,
        refusal: fillOnly ? browserRefusal : null,
        pick: (picked) {
          agent = picked;
          close();
        },
        close: close,
      ),
    );
    final picked = agent;
    if (picked == null || !mounted) return;
    try {
      await giveSecret(context, secret, picked, fillOnly: fillOnly);
    } on RequestFailure catch (failure) {
      if (mounted) _say(failure.message, error: true);
    }
  }

  /// The site being typed in to add, and whether the list is being set.
  final _site = TextEditingController();
  var _settingSites = false;

  /// Replaces where Claktory's browser may fill it in: refused, the
  /// backend's reason shows as it is.
  Future<bool> _setSites(List<String> sites) async {
    setState(() {
      _settingSites = true;
      _note = null;
    });
    try {
      await AppScope.of(context).setSecretSites(secret.id, sites);
      return true;
    } on RequestFailure catch (failure) {
      if (mounted) _say(failure.message, error: true);
      return false;
    } finally {
      if (mounted) setState(() => _settingSites = false);
    }
  }

  Future<void> _addSite() async {
    final site = _site.text.trim();
    if (site.isEmpty || _settingSites) return;
    if (await _setSites([...secret.sites, site]) && mounted) _site.clear();
  }

  /// Its sites (#435): where Claktory's browser may fill it in for an
  /// agent's errand, each with Remove, and a field to add one.
  List<Widget> _sites(BuildContext context, Palette palette) {
    final type = TypeScale.of(context);
    final sites = secret.sites;
    return [
      Text(
        'Sites',
        style: type.caption.copyWith(
          color: palette.textMuted,
          fontWeight: FontWeight.w500,
        ),
      ),
      Text(
        sites.isEmpty
            ? "Claktory's browser doesn't fill it in anywhere yet. Add a "
                  'site, such as https://shop.example, where it may.'
            : "Claktory's browser fills it in only on these sites.",
        style: type.caption.copyWith(color: palette.textFaint),
      ),
      // Asks each time: no errand can wait for an Allow, so it's filled
      // nowhere (Ghiaccio).
      if (sites.isNotEmpty && secret.askEachTime)
        Text(
          "It asks each time, so it isn't filled in anywhere. Turn off Ask "
          "each time to let Claktory's browser fill it in.",
          style: type.caption.copyWith(color: palette.signal),
        ),
      const SizedBox(height: Space.xs),
      for (final site in sites)
        Padding(
          padding: const EdgeInsets.only(bottom: Space.xs),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  site,
                  style: type.body.copyWith(
                    fontFamily: TextStyles.mono,
                    color: palette.text,
                  ),
                ),
              ),
              const SizedBox(width: Space.s),
              Button(
                label: 'Remove',
                onPressed: _settingSites
                    ? null
                    : () => _setSites([
                        for (final each in sites)
                          if (each != site) each,
                      ]),
              ),
            ],
          ),
        ),
      Row(
        children: [
          Expanded(
            child: CallbackShortcuts(
              bindings: {
                const SingleActivator(LogicalKeyboardKey.enter): _addSite,
              },
              child: TextField(
                controller: _site,
                placeholder: 'https://shop.example',
                kind: InputKind.url,
                semanticsLabel: 'A site to fill it in on',
              ),
            ),
          ),
          const SizedBox(width: Space.s),
          ListenableBuilder(
            listenable: _site,
            builder: (context, _) => Button(
              label: 'Add site',
              onPressed: _settingSites || _site.text.trim().isEmpty
                  ? null
                  : _addSite,
            ),
          ),
        ],
      ),
    ];
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final given = [
      for (final grant in secret.grants)
        grant.fillOnly
            ? '${_agentName(backend, grant.agentId)} (fill in only)'
            : _agentName(backend, grant.agentId),
    ];
    final now = DateTime.now();
    final summary = [
      if (secret.hasConnection())
        connectionSummary(backend, secret)
      else
        given.isEmpty
            ? 'Not given to any agent'
            : 'Given to ${given.join(', ')}',
      if (secret.lastUsedMs > 0)
        'Last used ${formatAge(secret.lastUsedMs.toInt(), now)}'
            '${secret.lastUsedBy.isEmpty ? '' : ' by ${_agentName(backend, secret.lastUsedBy)}'}',
      if (secret.ownerAgentId.isNotEmpty)
        'Saved by ${_agentName(backend, secret.ownerAgentId)}',
      if (secret.askEachTime) 'Asks each time',
    ].join(' · ');
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.xs),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Unselectable(
            child: Semantics(
              button: true,
              label: secret.name,
              child: Pressable(
                onTap: _toggle,
                builder: (context, hovered) => AnimatedContainer(
                  duration: Motion.fast,
                  padding: const EdgeInsets.symmetric(
                    horizontal: Space.s,
                    vertical: Space.s,
                  ),
                  decoration: BoxDecoration(
                    color: hovered ? palette.hover : null,
                    borderRadius: BorderRadius.circular(Radii.control),
                  ),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      SizedBox(
                        width: Space.m + 2,
                        height: 20,
                        child: Align(
                          alignment: Alignment.centerLeft,
                          child: AnimatedRotation(
                            turns: _open ? 0.25 : 0,
                            duration: Motion.fast,
                            curve: Curves.easeOut,
                            child: Chevron(
                              color: hovered
                                  ? palette.textMuted
                                  : palette.textFaint,
                              down: false,
                            ),
                          ),
                        ),
                      ),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              secret.name,
                              style: TypeScale.of(context).label
                                  .copyWith(color: palette.text),
                            ),
                            if (secret.description.isNotEmpty)
                              Text(
                                secret.description,
                                maxLines: _open ? null : 2,
                                overflow: _open ? null : TextOverflow.ellipsis,
                                style: TypeScale.of(context).body
                                    .copyWith(color: palette.textMuted),
                              ),
                            if (secret.hasConnection())
                              ConnectionHeadline(secret: secret)
                            else if (secret.hasExport())
                              ExportHeadline(secret: secret),
                            if (secret.variables.isNotEmpty)
                              Text(
                                [
                                  for (final variable in secret.variables)
                                    variable.name,
                                ].join(', '),
                                style: TypeScale.of(context).caption.copyWith(
                                  fontFamily: TextStyles.mono,
                                  color: palette.textFaint,
                                ),
                              ),
                            Text(
                              summary,
                              style: TypeScale.of(context).caption
                                  .copyWith(color: palette.textFaint),
                            ),
                            for (final warning in secret.warnings)
                              Text(
                                warning,
                                style: TypeScale.of(context).caption
                                    .copyWith(color: palette.danger),
                              ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
          if (_open)
            secret.hasConnection()
                ? Padding(
                    padding: const EdgeInsets.fromLTRB(
                      Space.s + Space.m + 2,
                      0,
                      Space.s,
                      Space.m,
                    ),
                    child: ConnectionDetails(secret: secret),
                  )
                : _details(context, palette, backend),
        ],
      ),
    );
  }

  Widget _details(BuildContext context, Palette palette, Backend backend) {
    final shown = _shown;
    final shownAt = _shownAt;
    final uses = _uses;
    return Padding(
      padding: const EdgeInsets.fromLTRB(
        Space.s + Space.m + 2,
        Space.xs,
        Space.s,
        Space.m,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (secret.hasExport()) ...[
            AccessLines(
              access: secret.export.access,
              resources: secret.export.resources,
              expiresMs: secret.export.expiresMs.toInt(),
            ),
            const SizedBox(height: Space.m),
          ],
          for (final variable in secret.variables)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.xs),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          variable.file
                              ? '${variable.name} (file)'
                              : variable.name,
                          style: TypeScale.of(context).caption.copyWith(
                            fontFamily: TextStyles.mono,
                            color: palette.textMuted,
                          ),
                        ),
                        Text(
                          shown?[variable.name] ??
                              (variable.hint.isEmpty
                                  ? '••••••'
                                  : '••••••${variable.hint}'),
                          style: TypeScale.of(context).body.copyWith(
                            fontFamily: TextStyles.mono,
                            color: palette.text,
                          ),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(width: Space.s),
                  Button(label: 'Copy', onPressed: () => _copy(variable.name)),
                ],
              ),
            ),
          Wrap(
            spacing: Space.s,
            runSpacing: Space.s,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Button(
                label: shown == null ? 'Show' : 'Hide',
                onPressed: shown == null ? _show : _hideValues,
              ),
              if (shownAt != null)
                Ticking(
                  active: true,
                  builder: (context, now) {
                    final left =
                        secretShownFor.inSeconds -
                        now.difference(shownAt).inSeconds;
                    return Text(
                      'Hides in ${left < 0 ? 0 : left}s',
                      style: TypeScale.of(context).caption.copyWith(
                        color: palette.textFaint,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    );
                  },
                ),
            ],
          ),
          if (_note != null)
            Padding(
              padding: const EdgeInsets.only(top: Space.xs),
              child: Text(
                _note!,
                style: TypeScale.of(context).caption.copyWith(
                  color: _noteIsError ? palette.danger : palette.textMuted,
                ),
              ),
            ),
          const SizedBox(height: Space.m),
          _ToggleRow(
            label: askEachTimeLabel,
            note: askEachTimeNote,
            value: secret.askEachTime,
            onChanged: _setting ? null : _askEachTime,
          ),
          const SizedBox(height: Space.m),
          ..._sites(context, palette),
          const SizedBox(height: Space.m),
          for (final grant in secret.grants)
            _GrantRow(secret: secret, grant: grant),
          const SizedBox(height: Space.s),
          Wrap(
            spacing: Space.s,
            runSpacing: Space.s,
            children: [
              Button(label: 'Give to…', onPressed: _give),
              // Only where it has sites to fill in on (#435).
              if (secret.sites.isNotEmpty)
                Button(
                  label: "Let Claktory's browser fill it in…",
                  onPressed: () => _give(fillOnly: true),
                ),
              Button(
                label: 'Edit…',
                onPressed: () => showModal(
                  context,
                  (context, close) => SecretForm(secret: secret, close: close),
                ),
              ),
              Button(
                label: 'Delete…',
                onPressed: () => showModal(
                  context,
                  (context, close) => _DeleteForm(secret: secret, close: close),
                ),
              ),
            ],
          ),
          if (uses != null && uses.isNotEmpty) ...[
            const SizedBox(height: Space.m),
            Text(
              'Recent use',
              style: TypeScale.of(context).caption.copyWith(
                color: palette.textFaint,
                fontWeight: FontWeight.w500,
              ),
            ),
            const SizedBox(height: Space.xs),
            for (final use in uses) _UseLine(use: use),
          ],
        ],
      ),
    );
  }
}

/// One agent a secret is given to: its character and name, how many of
/// its tasks hold it now, and Take back.
class _GrantRow extends StatelessWidget {
  const _GrantRow({required this.secret, required this.grant});

  final SecretInfo secret;
  final SecretGrant grant;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final agent = backend.workspace.agent(grant.agentId);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.xs),
      child: Row(
        children: [
          if (agent != null) ...[
            AgentCharacter(agent: agent, size: 20),
            const SizedBox(width: Space.s - 2),
          ],
          Expanded(
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: _agentName(backend, grant.agentId),
                    style: TextStyle(color: palette.text),
                  ),
                  // The names it gets it under too, as its request asked.
                  if (grant.aliases.isNotEmpty)
                    TextSpan(
                      text: '  also as ${grant.aliases.keys.join(', ')}',
                      style: TypeScale.of(context).caption.copyWith(
                        color: palette.textMuted,
                        fontFamily: TextStyles.mono,
                      ),
                    ),
                  // For Claktory's browser to fill in on its sites, never
                  // for its commands or apps (#435).
                  if (grant.fillOnly)
                    TextSpan(
                      text: '  can fill it in on its sites only',
                      style: TypeScale.of(context).caption
                          .copyWith(color: palette.textMuted),
                    ),
                  if (grant.runningTasks > 0)
                    TextSpan(
                      text:
                          '  ${grant.runningTasks} running '
                          '${grant.runningTasks == 1 ? 'task' : 'tasks'} '
                          'using it',
                      style: TypeScale.of(context).caption
                          .copyWith(color: palette.textFaint),
                    ),
                ],
              ),
              style: TypeScale.of(context).body,
            ),
          ),
          Button(
            label: 'Take back',
            onPressed: () => showModal(
              context,
              (context, close) =>
                  TakeBackForm(secret: secret, grant: grant, close: close),
            ),
          ),
        ],
      ),
    );
  }
}

/// One use of a secret: when, by whom, and what.
class _UseLine extends StatelessWidget {
  const _UseLine({required this.use});

  final SecretUse use;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final agent = backend.workspace.agent(use.agentId);
    final who = agent?.name ?? (use.agentId.isEmpty ? '' : use.agentId);
    final what = switch (use.kind) {
      'reveal' => use.detail.isEmpty ? 'Shown' : 'Shown on ${use.detail}',
      'grant' => 'Given to $who',
      'revoke' => 'Taken back from $who',
      // From a file on this computer (vault import): who asked, and what
      // was stored, its path's hidden characters shown as codes.
      'import' =>
        '${who.isEmpty ? 'Imported' : 'Imported, asked by $who'}'
            '${use.detail.isEmpty ? '' : ': ${importPath(use.detail)}'}',
      // An errand filled it in (#435): "filled into a password field on
      // https://shop.example".
      'fill' => switch (use.detail.lastIndexOf(' on ')) {
        -1 => 'Filled in by $who',
        final at => 'Filled in by $who on ${use.detail.substring(at + 4)}',
      },
      _ => use.detail.isEmpty ? '$who used it' : '$who: ${use.detail}',
    };
    return Text.rich(
      TextSpan(
        children: [
          TextSpan(
            text:
                '${logTime(DateTime.fromMillisecondsSinceEpoch(use.timeMs.toInt()))}  ',
            style: TextStyle(color: palette.textFaint),
          ),
          TextSpan(
            text: what,
            style: TextStyle(
              color: palette.textMuted,
              fontFamily: use.kind == 'use' && use.detail.isNotEmpty
                  ? TextStyles.mono
                  : null,
            ),
          ),
        ],
      ),
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: TypeScale.of(context).caption,
    );
  }
}

/// Puts [value] on the clipboard, and clears it after a minute if it still
/// holds it.
Future<void> copySecret(String value) async {
  await Clipboard.setData(ClipboardData(text: value));
  Timer(secretCopiedFor, () async {
    try {
      final now = await Clipboard.getData(Clipboard.kTextPlain);
      if (now?.text == value) {
        await Clipboard.setData(const ClipboardData(text: ''));
      }
    } catch (_) {
      // No clipboard to clear.
    }
  });
}

/// Gives [secret] to [agent]; with [fillOnly], only for Claktory's browser
/// to fill in on its sites (#435).
Future<void> giveSecret(
  BuildContext context,
  SecretInfo secret,
  Agent agent, {
  bool fillOnly = false,
}) => AppScope.of(context).grantSecret(secret.id, agent.id, fillOnly: fillOnly);

/// "Take back AWS from June?", with stopping its tasks that use it.
class TakeBackForm extends StatefulWidget {
  const TakeBackForm({
    super.key,
    required this.secret,
    required this.grant,
    required this.close,
  });

  final SecretInfo secret;
  final SecretGrant grant;
  final VoidCallback close;

  @override
  State<TakeBackForm> createState() => _TakeBackFormState();
}

class _TakeBackFormState extends State<TakeBackForm> {
  var _stop = true;
  var _busy = false;
  String? _error;

  Future<void> _submit() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).revokeSecret(
        widget.secret.id,
        widget.grant.agentId,
        stopRunningTasks: widget.grant.runningTasks > 0 && _stop,
      );
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final name = _agentName(AppScope.of(context), widget.grant.agentId);
    final tasks = widget.grant.runningTasks;
    return FormFrame(
      title: 'Take back ${widget.secret.name} from $name?',
      // A fill-only grant never showed it a value.
      body: widget.grant.fillOnly
          ? "$name's browser can't fill it in from now on."
          : "$name can't use it from now on. It may still know a value it "
                'has used: to be sure, make a new key where it came from and '
                'update it here.',
      fields: [
        if (tasks > 0) ...[
          _ToggleRow(
            label:
                "Also stop $name's ${tasks == 1 ? 'task' : '$tasks tasks'} "
                'that ${tasks == 1 ? 'is' : 'are'} using it',
            value: _stop,
            onChanged: (value) => setState(() => _stop = value),
          ),
          const SizedBox(height: Space.l),
        ],
      ],
      error: _error,
      submitLabel: 'Take back',
      onSubmit: _busy ? null : _submit,
      onCancel: widget.close,
    );
  }
}

class _DeleteForm extends StatefulWidget {
  const _DeleteForm({required this.secret, required this.close});

  final SecretInfo secret;
  final VoidCallback close;

  @override
  State<_DeleteForm> createState() => _DeleteFormState();
}

class _DeleteFormState extends State<_DeleteForm> {
  var _busy = false;
  String? _error;

  Future<void> _submit() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).deleteSecret(widget.secret.id);
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => FormFrame(
    title: 'Delete ${widget.secret.name}?',
    body: "Agents you gave it to can't use it anymore. This can't be undone.",
    fields: const [],
    error: _error,
    submitLabel: 'Delete',
    onSubmit: _busy ? null : _submit,
    onCancel: widget.close,
  );
}

/// Picks one of [agents].
class _AgentPicker extends StatelessWidget {
  const _AgentPicker({
    required this.title,
    required this.agents,
    required this.pick,
    required this.close,
    this.refusal,
  });

  final String title;
  final List<Agent> agents;

  /// Why an agent can't be picked, said under it, greyed; null when it can.
  final String? Function(Agent agent)? refusal;
  final ValueChanged<Agent> pick;
  final VoidCallback close;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return FormFrame(
      title: title,
      fields: [
        for (final agent in agents)
          if (refusal?.call(agent) case final why?)
            Semantics(
              label: '${agent.name}, $why',
              child: ExcludeSemantics(
                child: Padding(
                  padding: const EdgeInsets.symmetric(
                    horizontal: Space.s,
                    vertical: Space.xs,
                  ),
                  child: Row(
                    children: [
                      Opacity(
                        opacity: 0.5,
                        child: AgentCharacter(agent: agent, size: 20),
                      ),
                      const SizedBox(width: Space.s),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              agent.name,
                              style: TypeScale.of(context).body
                                  .copyWith(color: palette.textFaint),
                            ),
                            Text(
                              why,
                              style: TypeScale.of(context).caption
                                  .copyWith(color: palette.textFaint),
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            )
          else
            Unselectable(
              child: Semantics(
                button: true,
                label: agent.name,
                child: Pressable(
                  onTap: () => pick(agent),
                  builder: (context, hovered) => Container(
                    height: 32,
                    padding: const EdgeInsets.symmetric(horizontal: Space.s),
                    decoration: BoxDecoration(
                      color: hovered ? palette.hover : null,
                      borderRadius: BorderRadius.circular(Radii.control),
                    ),
                    child: Row(
                      children: [
                        AgentCharacter(agent: agent, size: 20),
                        const SizedBox(width: Space.s),
                        Text(
                          agent.name,
                          style: TypeScale.of(context).body
                              .copyWith(color: palette.text),
                        ),
                        if (agent.role.isNotEmpty) ...[
                          const SizedBox(width: Space.s),
                          Text(
                            agent.role,
                            style: TypeScale.of(context).caption.copyWith(
                              fontSize: 11,
                              color: palette.textFaint,
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                ),
              ),
            ),
        const SizedBox(height: Space.l),
      ],
      error: null,
      submitLabel: 'Choose',
      onSubmit: null,
      onCancel: close,
    );
  }
}

/// A variable being typed into a form: its name, its value, and whether
/// it's kept as a file.
class _VariableEdit {
  _VariableEdit({String name = '', this.file = false})
    : name = TextEditingController(text: name);

  final TextEditingController name;
  final value = TextEditingController();
  bool file;

  void dispose() {
    name.dispose();
    // Typed values go as soon as the form does.
    value.clear();
    value.dispose();
  }
}

/// Adding a secret, or editing one ([secret]): its name, description and
/// variables with their values. Editing keeps the values unless they're
/// all entered again.
class SecretForm extends StatefulWidget {
  const SecretForm({super.key, this.secret, required this.close});

  final SecretInfo? secret;
  final VoidCallback close;

  @override
  State<SecretForm> createState() => _SecretFormState();
}

class _SecretFormState extends State<SecretForm> {
  late final _name = TextEditingController(text: widget.secret?.name ?? '');
  late final _description = TextEditingController(
    text: widget.secret?.description ?? '',
  );
  late final List<_VariableEdit> _variables = [
    for (final variable in widget.secret?.variables ?? <SecretVariable>[])
      _VariableEdit(name: variable.name, file: variable.file),
    if (widget.secret == null) _VariableEdit(),
  ];
  var _askEachTime = false;
  var _busy = false;
  String? _error;

  /// Where Claktory's browser may fill it in (#435), one per line; on its
  /// row once it's made.
  final _sites = TextEditingController();

  @override
  void dispose() {
    _sites.dispose();
    _name.dispose();
    _description.dispose();
    for (final variable in _variables) {
      variable.dispose();
    }
    super.dispose();
  }

  /// A connection or a secret made from one: its values are Claktory's to
  /// keep, so only its name and description change here.
  bool get _named =>
      widget.secret?.hasConnection() == true ||
      widget.secret?.hasExport() == true;

  Future<void> _submit() async {
    final editing = widget.secret;
    final name = _name.text.trim();
    if (name.isEmpty) {
      setState(() => _error = 'Give it a name.');
      return;
    }
    final typed = _variables.where((v) => v.value.text.isNotEmpty).length;
    final replace = !_named && (editing == null || typed > 0);
    if (replace) {
      if (_variables.isEmpty ||
          _variables.any((v) => v.name.text.trim().isEmpty)) {
        setState(() => _error = 'Give each value a variable name.');
        return;
      }
      if (typed < _variables.length) {
        setState(
          () => _error = editing == null
              ? 'Enter a value for each variable.'
              : 'To change the values, enter them all.',
        );
        return;
      }
    }
    final entries = replace
        ? [
            for (final variable in _variables)
              SecretEntry(
                variable: variable.name.text.trim(),
                value: variable.value.text,
                file: variable.file,
              ),
          ]
        : <SecretEntry>[];
    setState(() {
      _busy = true;
      _error = null;
    });
    final backend = AppScope.of(context);
    try {
      if (editing == null) {
        await backend.createSecret(
          name,
          _description.text.trim(),
          entries,
          askEachTime: _askEachTime,
          sites: [
            for (final site in _sites.text.split(RegExp(r'[\s,]+')))
              if (site.isNotEmpty) site,
          ],
        );
      } else {
        await backend.updateSecret(
          editing.id,
          name,
          _description.text.trim(),
          entries,
        );
      }
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final editing = widget.secret;
    return FormFrame(
      title: editing == null ? 'Add a secret' : 'Edit ${editing.name}',
      fields: [
        Field(
          label: 'Name',
          note: 'Agents ask for it by this name.',
          child: TextField(
            controller: _name,
            placeholder: 'Cloudflare',
            kind: InputKind.text,
            autofocus: editing == null,
          ),
        ),
        Field(
          label: secretGrantsLabel,
          note: secretGrantsNote,
          child: TextField(
            controller: _description,
            placeholder: secretGrantsExample,
            kind: InputKind.multiline,
            maxLines: 3,
            minLines: 1,
          ),
        ),
        if (!_named) ..._values(palette, editing),
        // Once it's made, its row has the switch.
        if (editing == null) ...[
          const SizedBox(height: Space.m),
          _ToggleRow(
            label: askEachTimeLabel,
            note: askEachTimeNote,
            value: _askEachTime,
            onChanged: (on) => setState(() => _askEachTime = on),
          ),
          const SizedBox(height: Space.m),
          Field(
            label: 'Sites (optional)',
            note:
                "Where Claktory's browser may fill it in, one per line. "
                'Each is an exact https:// address.',
            child: TextField(
              controller: _sites,
              placeholder: 'https://shop.example',
              kind: InputKind.multiline,
              maxLines: 4,
              minLines: 1,
            ),
          ),
        ],
        const SizedBox(height: Space.l),
      ],
      error: _error,
      submitLabel: editing == null ? 'Add' : 'Save',
      onSubmit: _busy ? null : _submit,
      onCancel: widget.close,
    );
  }

  List<Widget> _values(Palette palette, SecretInfo? editing) => [
    Text(
      'Values',
      style: TypeScale.of(context).caption
          .copyWith(color: palette.textMuted, fontWeight: FontWeight.w500),
    ),
    if (editing != null)
      Text(
        'Leave the values empty to keep them. To change one, enter them '
        'all.',
        style: TypeScale.of(context).caption.copyWith(color: palette.textFaint),
      ),
    const SizedBox(height: Space.xs + 2),
    for (final (index, variable) in _variables.indexed)
      _VariableFields(
        key: ObjectKey(variable),
        variable: variable,
        onChanged: () => setState(() {}),
        onRemove: _variables.length > 1
            ? () => setState(() => _variables.removeAt(index).dispose())
            : null,
      ),
    Align(
      alignment: Alignment.centerLeft,
      child: Button(
        label: 'Add another value',
        onPressed: () => setState(() => _variables.add(_VariableEdit())),
      ),
    ),
  ];
}

class _VariableFields extends StatelessWidget {
  const _VariableFields({
    super.key,
    required this.variable,
    required this.onChanged,
    this.onRemove,
  });

  final _VariableEdit variable;
  final VoidCallback onChanged;
  final VoidCallback? onRemove;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.m),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: TextField(
                  controller: variable.name,
                  placeholder: 'CLOUDFLARE_API_TOKEN',
                  autocorrect: false,
                  textCapitalization: TextCapitalization.characters,
                ),
              ),
              if (onRemove != null) ...[
                const SizedBox(width: Space.xs),
                GlyphButton(
                  label: 'Remove ${variable.name.text}',
                  active: false,
                  onTap: onRemove!,
                  glyph: (color) => CloseGlyph(color: color),
                ),
              ],
            ],
          ),
          const SizedBox(height: Space.xs),
          variable.file
              ? TextField(
                  controller: variable.value,
                  placeholder: 'Paste the file’s contents',
                  kind: InputKind.multiline,
                  autocorrect: false,
                  textCapitalization: TextCapitalization.none,
                  maxLines: 6,
                  minLines: 3,
                )
              : TextField(
                  controller: variable.value,
                  placeholder: 'Paste the value',
                  kind: InputKind.secret,
                ),
          const SizedBox(height: Space.xs),
          _ToggleRow(
            label: 'Save as a file',
            note:
                'For tools that need a file, such as a key file. The variable '
                'holds the file’s path.',
            value: variable.file,
            onChanged: (value) {
              variable.file = value;
              onChanged();
            },
          ),
          Container(
            margin: const EdgeInsets.only(top: Space.s),
            height: 1,
            color: palette.hairline,
          ),
        ],
      ),
    );
  }
}

/// Pasting a .env file: Preview lists its variables (never the values)
/// and what's wrong, then they become one secret or one each.
class EnvForm extends StatefulWidget {
  const EnvForm({super.key, required this.close});

  final VoidCallback close;

  @override
  State<EnvForm> createState() => _EnvFormState();
}

class _EnvFormState extends State<EnvForm> {
  final _text = TextEditingController();
  final _name = TextEditingController();
  final _description = TextEditingController();

  /// What was pasted, held only until it's added or the form closes.
  String? _pasted;
  EnvPreview? _preview;
  var _each = false;
  var _busy = false;
  String? _error;

  @override
  void dispose() {
    _pasted = null;
    _text.clear();
    _text.dispose();
    _name.dispose();
    _description.dispose();
    super.dispose();
  }

  Future<void> _previewIt() async {
    final text = _text.text;
    if (text.trim().isEmpty) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final preview = await AppScope.of(context).previewEnv(text);
      if (!mounted) return;
      setState(() {
        _busy = false;
        _pasted = text;
        _preview = preview;
        _text.clear();
        if (_name.text.isEmpty) {
          _name.text = suggestSecretName([
            for (final variable in preview.variables) variable.name,
          ]);
        }
      });
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  Future<void> _add() async {
    final pasted = _pasted;
    if (pasted == null) return;
    if (!_each && _name.text.trim().isEmpty) {
      setState(() => _error = 'Give it a name.');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).importEnv(
        pasted,
        _name.text.trim(),
        _description.text.trim(),
        onePerVariable: _each,
      );
      _pasted = null;
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final preview = _preview;
    if (preview == null) {
      return FormFrame(
        title: 'Paste a .env file',
        body:
            "Claktory reads the names and values. The values aren't shown "
            'back.',
        fields: [
          TextField(
            controller: _text,
            placeholder: 'Paste here',
            kind: InputKind.multiline,
            autocorrect: false,
            textCapitalization: TextCapitalization.none,
            maxLines: 8,
            minLines: 8,
            autofocus: true,
          ),
          const SizedBox(height: Space.l),
        ],
        error: _error,
        submitLabel: 'Preview',
        onSubmit: _busy ? null : _previewIt,
        onCancel: widget.close,
      );
    }
    final names = [for (final variable in preview.variables) variable.name];
    return FormFrame(
      title: 'Paste a .env file',
      body: names.isEmpty
          ? 'Found no values.'
          : 'Found ${names.length} ${names.length == 1 ? 'value' : 'values'}.',
      fields: [
        for (final name in names)
          Text(
            name,
            style: TypeScale.of(context).body
                .copyWith(fontFamily: TextStyles.mono, color: palette.text),
          ),
        for (final warning in preview.warnings)
          Text(
            warning,
            style: TypeScale.of(context).caption
                .copyWith(color: palette.danger),
          ),
        const SizedBox(height: Space.l),
        if (names.length > 1)
          Field(
            label: 'Keep them',
            child: Select<bool>(
              value: _each,
              options: const [
                SelectOption(false, 'Together, as one secret'),
                SelectOption(true, 'As a secret for each'),
              ],
              onChanged: (value) => setState(() => _each = value),
            ),
          ),
        if (!_each || names.length == 1) ...[
          Field(
            label: 'Name',
            note: 'Agents ask for it by this name.',
            child: TextField(
              controller: _name,
              placeholder: 'Cloudflare',
              kind: InputKind.text,
            ),
          ),
          Field(
            label: secretGrantsLabel,
            note: secretGrantsNote,
            child: TextField(
              controller: _description,
              placeholder: secretGrantsExample,
              kind: InputKind.multiline,
              maxLines: 3,
              minLines: 1,
            ),
          ),
        ],
      ],
      error: _error,
      submitLabel: 'Add',
      onSubmit: _busy || names.isEmpty ? null : _add,
      onCancel: widget.close,
    );
  }
}

/// A name for the secret holding [variables]: their common first word
/// ("AWS_ACCESS_KEY_ID", "AWS_REGION" → "AWS"; "CLOUDFLARE_API_TOKEN" →
/// "Cloudflare"), or '' when they share none.
String suggestSecretName(List<String> variables) {
  if (variables.isEmpty) return '';
  final first = {for (final variable in variables) variable.split('_').first};
  if (first.length != 1) return '';
  final word = first.single;
  if (word.isEmpty) return '';
  // Short ones are acronyms, kept as they are.
  if (word.length <= 3) return word.toUpperCase();
  return word[0].toUpperCase() + word.substring(1).toLowerCase();
}

/// In an agent's settings, under the Secrets extension: the secrets it has,
/// so it's clear there too (they're given and taken back under Secrets).
class AgentSecretsSummary extends StatelessWidget {
  const AgentSecretsSummary({super.key, required this.agentId});

  final String agentId;

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final palette = Palette.of(context);
        final names = [
          for (final secret in _vault(backend).secrets)
            if (secret.grants.any((grant) => grant.agentId == agentId))
              secret.name,
        ];
        return Text(
          agentSecretsSummary(names),
          style: TypeScale.of(context).caption
              .copyWith(color: palette.textMuted),
        );
      },
    );
  }
}

/// "Has AWS and Cloudflare.", "Has no secrets yet."
String agentSecretsSummary(List<String> names) {
  if (names.isEmpty) return 'Has no secrets yet.';
  final shown = names.length <= 3
      ? names
      : [...names.take(2), '${names.length - 2} more'];
  final listed = shown.length == 1
      ? shown.single
      : '${shown.take(shown.length - 1).join(', ')} and ${shown.last}';
  return 'Has $listed.';
}

/// In an agent's settings: a toggle for each secret, on while it's given
/// to the agent. Taking back confirms.
/// While the agent is limited to a pipe (Agent.quarantined) its secrets
/// don't work: the toggles are greyed and take no taps (its settings say
/// why, above them). While it runs errands (Agent.contained) each one it
/// has says under it what it can still do with it.
class AgentSecretsField extends StatefulWidget {
  const AgentSecretsField({super.key, required this.agent});

  final Agent agent;

  @override
  State<AgentSecretsField> createState() => _AgentSecretsFieldState();
}

class _AgentSecretsFieldState extends State<AgentSecretsField> {
  String? _error;

  Future<void> _set(SecretInfo secret, bool on) async {
    setState(() => _error = null);
    final grant = [
      for (final grant in secret.grants)
        if (grant.agentId == widget.agent.id) grant,
    ];
    try {
      if (on) {
        await giveSecret(context, secret, widget.agent);
      } else if (grant.isNotEmpty) {
        await showModal(
          context,
          (context, close) =>
              TakeBackForm(secret: secret, grant: grant.first, close: close),
        );
      }
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final palette = Palette.of(context);
        final vault = _vault(backend);
        return Field(
          label: 'Secrets',
          note: vault.secrets.isEmpty
              ? null
              : 'Secrets this agent can use. It never sees their values.',
          child: vault.secrets.isEmpty
              ? Text(
                  'None yet. When this agent needs one, it sends you a '
                  'request.',
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textFaint),
                )
              : Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    for (final secret in vault.secrets)
                      Padding(
                        padding: const EdgeInsets.only(bottom: Space.s),
                        child: _ToggleRow(
                          label: secret.name,
                          note: secret.description.isEmpty
                              ? null
                              : secret.description,
                          value: secret.grants.any(
                            (grant) => grant.agentId == widget.agent.id,
                          ),
                          // The backend's warnings that name it, such as
                          // what it can still do with it while it runs
                          // errands (Ghiaccio 50734, 51137): the grant
                          // stays, and comes back whole once it stops. Not
                          // while it's limited to a pipe, which its
                          // settings say once, above.
                          limits: widget.agent.quarantined
                              ? const []
                              : [
                                  for (final warning in secret.warnings)
                                    if (_names(warning, widget.agent.name))
                                      warning,
                                ],
                          // Off while it's limited to a pipe, which its
                          // settings say above: nothing to change here.
                          onChanged:
                              widget.agent.quarantined ||
                                  widget.agent.readsReports
                              ? null
                              : (value) => _set(secret, value),
                        ),
                      ),
                    if (_error != null)
                      Text(
                        _error!,
                        style: TypeScale.of(context).caption
                            .copyWith(color: palette.danger),
                      ),
                  ],
                ),
        );
      },
    );
  }
}

/// Whether [warning] names [name] as a whole word ("While June runs
/// errands, …", "A grant to June was changed …").
bool _names(String warning, String name) =>
    name.isNotEmpty &&
    RegExp('(^|\\W)${RegExp.escape(name)}(\\W|\$)').hasMatch(warning);

/// The Secrets extension's panel beside an agent: the secrets it has, from
/// the vault the backend sends with the workspace (the extension has no
/// view of its own), each with what it's for, its variables and when it
/// was given, to take back; and giving it another.
class AgentSecretsPanel extends StatelessWidget {
  const AgentSecretsPanel({super.key, required this.agentId, this.now});

  final String agentId;

  /// What "now" is for ages; the clock's time unless given.
  final DateTime? now;

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final palette = Palette.of(context);
        final vault = _vault(backend);
        final agent = backend.workspace.agent(agentId);
        final given = [
          for (final secret in vault.secrets)
            for (final grant in secret.grants)
              if (grant.agentId == agentId) (secret: secret, grant: grant),
        ];
        final others = [
          for (final secret in vault.secrets)
            if (!secret.hasConnection() &&
                !secret.grants.any((grant) => grant.agentId == agentId))
              secret,
        ];
        return ListView(
          padding: const EdgeInsets.fromLTRB(
            Space.l,
            Space.m,
            Space.l,
            Space.l,
          ),
          children: [
            // Unlocked here, as in Settings, rather than sent there (Lucy's
            // Settings spec §5).
            if (vault.exists && !vault.unlocked) ...[
              _Locked(
                account: backend.workspace.workspace.hasAccount()
                    ? backend.workspace.workspace.account
                    : null,
                why: vault.lockedWhy,
              ),
              const SizedBox(height: Space.m),
            ],
            if (given.isEmpty)
              Text(
                '${agent?.name ?? agentId} has no secrets yet.',
                style: TypeScale.of(context).body
                    .copyWith(color: palette.textFaint),
              )
            else
              for (final (:secret, :grant) in given)
                _GivenSecret(
                  key: ValueKey(secret.id),
                  secret: secret,
                  grant: grant,
                  now: now ?? DateTime.now(),
                ),
            if (agent != null && others.isNotEmpty) ...[
              const SizedBox(height: Space.s),
              Align(
                alignment: Alignment.centerLeft,
                child: Button(
                  label: 'Give a secret…',
                  onPressed: () => showModal(
                    context,
                    (context, close) => _SecretPicker(
                      title: 'Give a secret to ${agent.name}',
                      secrets: others,
                      pick: (secret) {
                        close();
                        fireAndLog(backend.grantSecret(secret.id, agent.id));
                      },
                      close: close,
                    ),
                  ),
                ),
              ),
            ],
            const SizedBox(height: Space.l),
            Text(
              '${agent?.name ?? agentId} uses them without ever seeing their '
              'values.',
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textFaint),
            ),
          ],
        );
      },
    );
  }
}

/// One secret an agent has, in its Secrets panel: its name, what it's for,
/// its variables, when it was given and last used by the agent, and Take
/// back.
class _GivenSecret extends StatelessWidget {
  const _GivenSecret({
    super.key,
    required this.secret,
    required this.grant,
    required this.now,
  });

  final SecretInfo secret;
  final SecretGrant grant;
  final DateTime now;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final tasks = grant.runningTasks;
    final when = [
      if (grant.grantedMs > 0)
        'Given ${formatAge(grant.grantedMs.toInt(), now)}',
      if (secret.lastUsedBy == grant.agentId && secret.lastUsedMs > 0)
        'Last used ${formatAge(secret.lastUsedMs.toInt(), now)}',
      if (tasks > 0) 'Used by $tasks running ${tasks == 1 ? 'task' : 'tasks'}',
      // Only for Claktory's browser, on its sites (#435).
      if (grant.fillOnly)
        secret.sites.isEmpty
            ? 'Fill in only, on no site yet'
            : 'Fill in only, on ${secret.sites.join(', ')}',
    ].join(' · ');
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.m),
      child: Builder(
        builder: (context) {
          final about = Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                secret.name,
                style: TypeScale.of(context).label
                    .copyWith(color: palette.text),
              ),
              if (secret.description.isNotEmpty)
                Text(
                  secret.description,
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textMuted),
                ),
              if (secret.variables.isNotEmpty)
                Text(
                  [
                    for (final variable in secret.variables) variable.name,
                    // The names it gets it under too, as it asked.
                    for (final alias in grant.aliases.keys) 'also as $alias',
                  ].join(', '),
                  style: TypeScale.of(context).code.copyWith(
                    fontSize: isPhoneLayout(context) ? null : 11,
                    color: palette.textMuted,
                  ),
                ),
              if (when.isNotEmpty)
                Text(
                  when,
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textFaint),
                ),
            ],
          );
          final takeBack = Button(
            label: 'Take back',
            onPressed: () => showModal(
              context,
              (context, close) =>
                  TakeBackForm(secret: secret, grant: grant, close: close),
            ),
          );
          // At the largest text on a phone, Take back goes under it.
          if (isPhoneLayout(context) && largeText(context)) {
            return Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                about,
                const SizedBox(height: Space.s),
                takeBack,
              ],
            );
          }
          return Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(child: about),
              const SizedBox(width: Space.s),
              takeBack,
            ],
          );
        },
      ),
    );
  }
}

/// Picks one of [secrets].
class _SecretPicker extends StatelessWidget {
  const _SecretPicker({
    required this.title,
    required this.secrets,
    required this.pick,
    required this.close,
  });

  final String title;
  final List<SecretInfo> secrets;
  final ValueChanged<SecretInfo> pick;
  final VoidCallback close;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return FormFrame(
      title: title,
      fields: [
        for (final secret in secrets)
          Unselectable(
            child: Semantics(
              button: true,
              label: secret.name,
              child: Pressable(
                onTap: () => pick(secret),
                builder: (context, hovered) => Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: Space.s,
                    vertical: Space.xs + 2,
                  ),
                  decoration: BoxDecoration(
                    color: hovered ? palette.hover : null,
                    borderRadius: BorderRadius.circular(Radii.control),
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Text(
                        secret.name,
                        style: TypeScale.of(context).body
                            .copyWith(color: palette.text),
                      ),
                      if (secret.description.isNotEmpty)
                        Text(
                          secret.description,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TypeScale.of(context).caption
                              .copyWith(color: palette.textFaint),
                        ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        const SizedBox(height: Space.l),
      ],
      error: null,
      submitLabel: 'Choose',
      onSubmit: null,
      onCancel: close,
    );
  }
}

/// Words too common in secrets' names to say two are the same.
const _commonWords = {
  'api',
  'key',
  'keys',
  'token',
  'tokens',
  'secret',
  'secrets',
  'access',
  'id',
  'the',
  'for',
  'and',
  'account',
  'credentials',
  'password',
};

/// The person's secret that looks like the one [wanted] describes, if any:
/// one with a name word in common (Cloudflare DNS for "Cloudflare token"),
/// or with every variable it asks for. The card picks it to start with.
SecretInfo? closestSecret(NewSecret wanted, List<SecretInfo> secrets) {
  Set<String> words(String name) => {
    for (final word in name.toLowerCase().split(RegExp('[^a-z0-9]+')))
      if (word.length > 1 && !_commonWords.contains(word)) word,
  };
  final asked = words(wanted.name);
  final variables = {for (final variable in wanted.variables) variable.name};
  for (final secret in secrets) {
    if (secret.name.toLowerCase() == wanted.name.toLowerCase()) return secret;
  }
  for (final secret in secrets) {
    if (words(secret.name).intersection(asked).isNotEmpty) return secret;
  }
  for (final secret in secrets) {
    final has = {for (final variable in secret.variables) variable.name};
    if (variables.isNotEmpty && has.containsAll(variables)) return secret;
  }
  return null;
}

/// Why the secret chosen for [agent]'s request may not be the one it
/// needs: it sets [sets], and of the names [asked] for, not all. With
/// none matching, a check that it's the right one.
String mismatchLine({
  required String agent,
  required List<String> asked,
  required List<String> sets,
}) {
  final missing = [
    for (final name in asked)
      if (!sets.contains(name)) name,
  ];
  final check = "Check it's the one $agent needs.";
  // Some match: only what it leaves out.
  if (missing.length < asked.length) {
    return "This secret doesn't set ${_or(missing)}.";
  }
  if (missing.length == 1) {
    return 'This secret sets ${_and(sets)}, not ${missing.single}. $check';
  }
  return "This secret doesn't set ${_or(missing)}. $check";
}

String _or(List<String> names) => _joined(names, 'or');
String _and(List<String> names) => _joined(names, 'and');
String _joined(List<String> names, String last) => names.length < 2
    ? names.join()
    : '${names.sublist(0, names.length - 1).join(', ')} $last ${names.last}';

/// What a waiting request asks of the person, in a line (Needs you):
/// [shownIn] is whose conversation shows it, the admin agent's for the
/// secret it collects.
String secretRequestSummary(
  SecretRequest request,
  Workspace workspace, {
  String? shownIn,
}) {
  final name = request.newSecret.name.isNotEmpty
      ? request.newSecret.name
      : request.secretName;
  if (request.login) {
    return request.fittingSecretId.isNotEmpty
        ? 'Asks to use your ${request.fittingSecretName} on '
              '${loginSite(request)}'
        : 'Asks to sign in to ${loginSite(request)}';
  }
  if (request.collecting &&
      request.adminAgentId.isNotEmpty &&
      shownIn == request.adminAgentId &&
      shownIn != request.agentId) {
    final asker =
        [
          for (final agent in workspace.agents)
            if (agent.id == request.agentId) agent.name,
        ].firstOrNull ??
        request.agentId;
    return 'Asks you to add a secret for $asker: $name';
  }
  if (request.generate.isNotEmpty) return 'Asks for ${keyKind(request)}';
  if (request.imports.isNotEmpty) {
    return 'Asks to import ${importKind(request)}';
  }
  // Approved with an admin agent finding it; handed back to the person
  // (its admin cleared or deleted), it asks them again, as its card does.
  if (request.state == SecretRequestState.SECRET_REQUEST_STATE_APPROVED &&
      request.adminAgentId.isNotEmpty) {
    final admin = [
      for (final agent in workspace.agents)
        if (agent.id == request.adminAgentId) agent.name,
    ].firstOrNull;
    return '${admin ?? 'An admin agent'} is finding it: $name';
  }
  if (request.adminAgentId.isNotEmpty && request.secretId.isEmpty) {
    return 'Needs a secret: $name';
  }
  return request.secretName.isNotEmpty
      ? 'Asks for ${request.secretName}'
      : 'Asks for a secret: $name';
}

/// Approved first, the admin agent may have given it.
bool _byAdmin(SecretRequest request) =>
    request.generate.isEmpty &&
    request.approvedMs > 0 &&
    request.state == SecretRequestState.SECRET_REQUEST_STATE_GRANTED;

/// How a decided request ended, in the words its card, its Needs you row
/// and History share (Lucy's §8): "Gave Hetzner", "Added and gave
/// Throwaway Google account", "Generated build-box deploy", "Imported,
/// given to Risotto", "Dismissed"… [agent] is the asking agent's name;
/// [collected]: shown in the admin agent's conversation, the secret it
/// collected ("Gave it to June").
String secretOutcome(
  SecretRequest request, {
  required String agent,
  bool collected = false,
}) {
  final given = request.givenSecretName.isNotEmpty
      ? request.givenSecretName
      : request.secretName.isNotEmpty
      ? request.secretName
      : request.newSecret.name;
  final imports = request.imports.isNotEmpty;
  return switch (request.state) {
    SecretRequestState.SECRET_REQUEST_STATE_WITHDRAWN => 'Withdrawn',
    SecretRequestState.SECRET_REQUEST_STATE_EXPIRED => 'Expired',
    // Answered with words alone (#370), on older cards.
    SecretRequestState.SECRET_REQUEST_STATE_DISMISSED when request.replied =>
      'Replied',
    SecretRequestState.SECRET_REQUEST_STATE_DISMISSED => 'Dismissed',
    _ when request.login => _loginOutcome(request, given),
    // Made, to no one as it asked.
    SecretRequestState.SECRET_REQUEST_STATE_GENERATED =>
      imports
          ? 'Imported, given to no one'
          : 'Generated $given, given to no one',
    _ when imports => 'Imported, given to $agent',
    _ when request.generate.isNotEmpty => 'Generated $given',
    _ when collected => 'Gave it to $agent',
    _ when _byAdmin(request) => '$agent got $given',
    _ when request.added => 'Added and gave $given',
    _ => 'Gave $given',
  };
}

String loginSite(SecretRequest request) {
  if (request.loginSite.isNotEmpty) return request.loginSite;
  final host = request.sites.isEmpty
      ? ''
      : Uri.tryParse(request.sites.first)?.host ?? '';
  return host.isNotEmpty ? host : 'the site';
}

/// One of the person's own logins (Use it) isn't "saved".
String _loginOutcome(SecretRequest request, String given) {
  final site = loginSite(request);
  final saved = request.added && given.isNotEmpty;
  return switch (request.loginOutcome) {
    SecretLoginOutcome.SECRET_LOGIN_OUTCOME_SIGNING_IN =>
      'Signing in to $site…',
    SecretLoginOutcome.SECRET_LOGIN_OUTCOME_SIGNED_IN =>
      saved ? 'Signed in to $site · Saved as $given' : 'Signed in to $site',
    SecretLoginOutcome.SECRET_LOGIN_OUTCOME_CODE_NEXT =>
      saved
          ? 'Saved as $given. $site asks for a code next.'
          : '$site asks for a code next.',
    _ => saved ? 'Saved as $given' : 'Gave $given',
  };
}

/// Whether the agent's link (SecretRequest.link) can be offered: https,
/// and going where its button says, the backend's ASCII host (Fugo).
bool opensLink(SecretRequest request) {
  final uri = Uri.tryParse(request.link);
  return uri != null &&
      uri.scheme == 'https' &&
      request.linkHost.isNotEmpty &&
      uri.host == request.linkHost;
}

/// What the admin agent's taking on a request waits for (#360 K5): its
/// usage limits ("Rivet is waiting for your Claude usage to reset at
/// 18:00."), what it's doing now, or nothing (it's finding it).
String adminWaitLine(SecretRequest request, Agent? admin, String adminName) {
  if (request.collecting) {
    return "Waiting for you to add it in $adminName's conversation.";
  }
  if (admin != null && admin.usageWait.isNotEmpty) {
    final line = usageWaitLine(admin);
    final said = line.startsWith('Waiting ')
        ? '$adminName is waiting ${line.substring('Waiting '.length)}'
        : '$adminName: $line';
    return said.endsWith('.') ? said : '$said.';
  }
  if (admin != null && atWork(admin)) {
    return "$adminName will do this when it finishes what it's doing.";
  }
  return 'Approved. $adminName is finding it.';
}

/// What an import request (SecretRequest.imports) brings into the vault:
/// "a file", "2 files".
String importKind(SecretRequest request) =>
    request.imports.length == 1 ? 'a file' : '${request.imports.length} files';

/// A path as it is, with what could make it read as another shown as
/// visible codes (Fugo): control, bidi and zero-width characters as
/// \u{202E}. Never shortened.
String importPath(String path) => [
  for (final rune in path.runes)
    hiddenRune(rune) ? runeCode(rune) : String.fromCharCode(rune),
].join();

/// Where an imported file is, when that's somewhere that holds keys or
/// credentials (SecretImport.sensitive), in a line; null otherwise.
String? importSensitive(String kind) => switch (kind) {
  '' => null,
  'ssh' => 'This is in your SSH keys folder.',
  'gnupg' => 'This is in your GnuPG keys folder.',
  'aws' => 'This is in your AWS credentials folder.',
  'gcloud' => 'This is in your Google Cloud credentials folder.',
  'kube' => 'This is in your Kubernetes credentials folder.',
  'docker' => 'This is in your Docker credentials folder.',
  'keychains' => "This is in your Mac's keychains folder.",
  'browser' => "This is in a web browser's profile folder.",
  'netrc' => 'This is your .netrc file, which holds passwords for other sites.',
  'claktory' => "This is in Claktory's own folder.",
  _ => 'This is in a folder that holds credentials.',
};

/// What a generate request (SecretRequest.generate) asks for: "a new SSH
/// key".
String keyKind(SecretRequest request) => request.generate.startsWith('ssh-')
    ? 'a new SSH key'
    : request.generate == 'password'
    ? 'a new password'
    : 'a new key';

/// What a password Claktory makes is drawn from, from its spec: "letters,
/// digits and symbols".
String passwordClasses(PasswordSpec spec) {
  final classes = [
    if (spec.lowercase && spec.uppercase)
      'letters'
    else if (spec.lowercase)
      'lowercase letters'
    else if (spec.uppercase)
      'uppercase letters',
    if (spec.digits) 'digits',
    if (spec.symbols) 'symbols',
  ];
  if (classes.length < 2) return classes.firstOrNull ?? 'characters';
  return '${classes.sublist(0, classes.length - 1).join(', ')} and '
      '${classes.last}';
}

/// What a password of [length] characters is, by [spec]: "A 32-character
/// password: letters, digits and symbols".
String passwordSummary(PasswordSpec spec, int length) =>
    'A $length-character password: ${passwordClasses(spec)}';

/// Its strength at [length], as Claktory works it out: whole bits.
int passwordBits(PasswordSpec spec, int length) =>
    (length * spec.bitsPerCharacter).floor();

/// Its strength in a word (Trish: bits are a number few can judge). The
/// backend never makes one under 72 bits, so "weak" is only what's below.
String passwordStrength(int bits) => bits >= 128
    ? 'very strong'
    : bits >= 96
    ? 'strong'
    : bits >= 72
    ? 'fair'
    : 'weak';

/// An agent's request for a secret, in its conversation and in Needs you:
/// what it needs and why, in its color. With an admin agent to pass it to
/// (SecretRequest.admin_agent_id), the person approves the need, or
/// dismisses it, and the admin agent finds the secret or asks for it to be
/// added; Choose a secret… does it directly instead. Without one, the
/// person gives one of their secrets (Your secrets) or adds a new one
/// filled in from the request (New secret), or dismisses it with a word
/// why. In the admin agent's conversation, while it collects the secret
/// (SecretRequest.collecting), it's the new secret to add, filled in.
/// Decided, what was given and when. The backend sends the entry back
/// decided; the card keeps nothing.
class SecretRequestCard extends StatefulWidget {
  const SecretRequestCard(
    this.request, {
    super.key,
    this.shownIn,
    this.direct = false,
    this.onDecided,
  });

  final SecretRequest request;

  /// The agent whose conversation (or Needs you item) shows it: the admin
  /// agent's shows the secret it collects.
  final String? shownIn;

  /// Giving or adding directly, without the card around it: Choose a
  /// secret…'s dialog.
  final bool direct;

  /// Decided from here (the dialog closes).
  final VoidCallback? onDecided;

  @override
  State<SecretRequestCard> createState() => _SecretRequestCardState();
}

class _SecretRequestCardState extends State<SecretRequestCard>
    with AutomaticKeepAliveClientMixin, PublishesAnswer {
  /// While it waits, what the person chose or typed here (a length, a
  /// name, a value) stays when the card scrolls out of view and back
  /// (Zucchero's row 189); it goes with the conversation, as before.
  @override
  bool get wantKeepAlive => _waiting || _signingIn;

  late final _name = TextEditingController(text: widget.request.newSecret.name);

  /// Kept only on this screen: the backend never holds a vault value in
  /// the Workspace, so after a refusal the username is still here to fix.
  final _loginUser = TextEditingController();
  final _loginPassword = TextEditingController();

  bool get _signingIn =>
      request.login &&
      request.loginOutcome ==
          SecretLoginOutcome.SECRET_LOGIN_OUTCOME_SIGNING_IN;
  late final _description = TextEditingController(
    text: widget.request.newSecret.description,
  );
  late final List<_VariableEdit> _values = [
    for (final variable in widget.request.newSecret.variables)
      _VariableEdit(name: variable.name, file: variable.file),
  ];
  final _comment = TextEditingController();
  final _commentFocus = FocusNode();

  /// Adding a new secret rather than choosing one; set when first built,
  /// from the person's secrets.
  bool? _adding;

  /// The secret chosen to give; empty for none yet.
  var _chosen = '';
  var _deciding = false;
  String? _error;

  /// When the chosen secret doesn't set every name asked for: give it under
  /// those names too (the default), each the variable picked for it here
  /// (asked name → the secret's variable; empty leaves it out), or as it is.
  var _alias = true;
  final _aliases = <String, String>{};

  /// What the agent's change to its request added, said on the card (N14).
  String? _changed;

  /// Its agent changed it while the person typed in its field (N4b).
  var _changedWhileTyping = false;

  /// The asking agent's name, as last shown.
  var _agentName = '';

  @override
  void didUpdateWidget(SecretRequestCard old) {
    super.didUpdateWidget(old);
    // Decided, it scrolls away like any entry.
    updateKeepAlive();
    if (widget.request.loginOutcome ==
            SecretLoginOutcome.SECRET_LOGIN_OUTCOME_REFUSED &&
        old.request.loginOutcome !=
            SecretLoginOutcome.SECRET_LOGIN_OUTCOME_REFUSED) {
      _loginPassword.clear();
    }
    // The agent changed what it asks for: the form starts from that.
    if (old.request.updatedMs != widget.request.updatedMs) {
      // Changed while words were typed (N4b): they stay, and the field
      // says what happened.
      if (commentTyped(_comment)) _changedWhileTyping = true;
      final wanted = widget.request.newSecret;
      _name.text = wanted.name;
      _description.text = wanted.description;
      // What was pasted stays for what it still asks for (N14); what it
      // asks for now and didn't before is said.
      final kept = {
        for (final value in _values)
          if (value.value.text.isNotEmpty) value.name.text: value.value.text,
      };
      final before = {for (final value in _values) value.name.text};
      for (final value in _values) {
        value.dispose();
      }
      _values
        ..clear()
        ..addAll([
          for (final variable in wanted.variables)
            _VariableEdit(name: variable.name, file: variable.file)
              ..value.text = kept[variable.name] ?? '',
        ]);
      final added = [
        for (final variable in wanted.variables)
          if (!before.contains(variable.name)) variable.name,
      ];
      _changed = added.isEmpty ? null : 'Now also asks for ${_and(added)}.';
      _aliases.clear();
      // A choice made for what it asked before isn't one for this.
      _deleteOriginals = false;
    }
  }

  @override
  void dispose() {
    _name.dispose();
    _description.dispose();
    for (final value in _values) {
      value.dispose();
    }
    _comment.dispose();
    _commentFocus.dispose();
    _loginUser.dispose();
    _loginPassword
      ..clear()
      ..dispose();
    super.dispose();
  }

  SecretRequest get request => widget.request;

  String? get _replaces =>
      request.loginOutcome == SecretLoginOutcome.SECRET_LOGIN_OUTCOME_REFUSED &&
          request.givenSecretName.isNotEmpty
      ? request.givenSecretName
      : null;

  bool get _loginReady =>
      _loginUser.text.trim().isNotEmpty && _loginPassword.text.isNotEmpty;

  Future<void> _signIn() async {
    if (!_loginReady) {
      _notSent();
      return;
    }
    final name = _replaces ?? _name.text.trim();
    if (name.isEmpty) {
      setState(() {
        _error = 'Enter a name for the login.';
        _editing = true;
      });
      _notSent();
      return;
    }
    await _decide(() async {
      await AppScope.of(context).decideSecretRequest(
        request.agentId,
        request.id,
        approve: true,
        name: name,
        description: request.newSecret.description,
        comment: _comment.text.trim(),
        entries: [
          SecretEntry(variable: 'USERNAME', value: _loginUser.text.trim()),
          SecretEntry(variable: 'PASSWORD', value: _loginPassword.text),
        ],
      );
      _loginPassword.clear();
    });
  }

  Future<void> _useLogin() => _decide(
    () => AppScope.of(context).decideSecretRequest(
      request.agentId,
      request.id,
      approve: true,
      secretId: request.fittingSecretId,
      comment: _comment.text.trim(),
    ),
  );

  /// From before agents described what they need: it names a secret.
  bool get _names => request.secretId.isNotEmpty;

  bool get _waiting =>
      request.state == SecretRequestState.SECRET_REQUEST_STATE_PENDING ||
      request.state == SecretRequestState.SECRET_REQUEST_STATE_APPROVED;

  /// A key for Claktory to make (SecretRequest.generate): Generate, never
  /// a values form or the admin agent.
  bool get _generates => request.generate.isNotEmpty;

  /// Files for the vault to import (SecretRequest.imports): Import, never
  /// a values form or the admin agent.
  bool get _imports => request.imports.isNotEmpty;

  /// An import deletes nothing unless the person says so.
  var _deleteOriginals = false;

  /// A generated key doesn't ask each time it's used, unless the person
  /// turns it on (Matt's call).
  var _askEach = false;

  /// A password to make (generate "password"): its spec, and the length
  /// the person chose, between the spec's shortest and longest (null: as
  /// asked).
  bool get _password => request.generate == 'password';
  int? _length;
  int get _passwordLength => _length ?? request.password.length;

  /// The admin agent took it on, and the person does it here instead (Do
  /// it here, #360 K5).
  var _here = false;

  /// The request answered on its own card (#360 K2, K4): give the secret
  /// that fits, or add the one asked for, or let the admin agent choose.
  /// Not the admin agent's add-secret card, a request naming a secret, a
  /// key to make or files to import.
  /// One of the person's secrets this card may give: not a connected
  /// service (given to no one), nor a website login (given only in Settings
  /// → Secrets, never for commands).
  static bool _givable(SecretInfo secret) =>
      !secret.hasConnection() && secret.sites.isEmpty;

  bool get _answers =>
      !widget.direct && !_collect && !_names && !_generates && !_imports;

  /// The admin agent's add-secret card for the request it collects.
  bool get _collect =>
      request.collecting &&
      request.adminAgentId.isNotEmpty &&
      widget.shownIn == request.adminAgentId &&
      widget.shownIn != request.agentId;

  /// The person approves the need, and the admin agent does the rest.
  bool get _approves =>
      !widget.direct &&
      !_collect &&
      !_names &&
      !_generates &&
      !_imports &&
      request.adminAgentId.isNotEmpty;

  Future<void> _give(List<SecretInfo> secrets) async {
    if (_deciding) return;
    final adding = !_names && _adding == true;
    final chosen = secrets.any((secret) => secret.id == _chosen);
    if (!_names && !adding && !chosen) {
      setState(() => _error = 'Choose a secret, or add a new one.');
      _notSent();
      return;
    }
    if (adding && _name.text.trim().isEmpty) {
      setState(() {
        _error = 'Enter a name for the secret.';
        _editing = true;
      });
      _notSent();
      return;
    }
    if (adding && _values.any((v) => v.value.text.isEmpty)) {
      setState(() => _error = 'Enter a value for each variable.');
      _notSent();
      return;
    }
    final secret = secrets.where((secret) => secret.id == _chosen).firstOrNull;
    final missing = !_names && !adding && secret != null
        ? _missing(secret)
        : const <String>[];
    final aliases = {
      if (_alias)
        for (final name in missing)
          if (_aliasFor(name, secret!) case final variable
              when variable.isNotEmpty)
            name: variable,
    };
    await _decide(
      () => AppScope.of(context).decideSecretRequest(
        request.agentId,
        request.id,
        approve: true,
        secretId: _names || adding ? '' : _chosen,
        name: adding ? _name.text.trim() : '',
        description: adding ? _description.text.trim() : '',
        aliases: aliases,
        asIs: missing.isNotEmpty && aliases.isEmpty,
        comment: _comment.text.trim(),
        entries: [
          if (adding)
            for (final value in _values)
              SecretEntry(
                variable: value.name.text,
                value: value.value.text,
                file: value.file,
              ),
        ],
      ),
    );
  }

  Future<void> _generate() async {
    final name = _name.text.trim();
    if (name.isEmpty) {
      setState(
        () => _error = _password
            ? 'Enter a name for the password.'
            : 'Enter a name for the key.',
      );
      _notSent();
      return;
    }
    await _decide(
      () => AppScope.of(context).decideSecretRequest(
        request.agentId,
        request.id,
        approve: true,
        name: name,
        askEachTime: _askEach,
        comment: _comment.text.trim(),
        passwordLength: _password ? _length ?? 0 : 0,
      ),
    );
  }

  /// Imports the files; [give] gives them to the agent that asked (Import
  /// and give to…), a choice of its own, never a switch left on (#360
  /// K7, Fugo).
  Future<void> _import({required bool give}) async {
    final name = _name.text.trim();
    if (name.isEmpty) {
      setState(() => _error = 'Enter a name for the secret.');
      _notSent();
      return;
    }
    await _decide(
      () => AppScope.of(context).decideSecretRequest(
        request.agentId,
        request.id,
        approve: true,
        name: name,
        askEachTime: _askEach,
        comment: _comment.text.trim(),
        grantToAsker: give && !request.grantToNobody,
        deleteOriginals: _deleteOriginals,
      ),
    );
  }

  /// The names asked for that [secret] doesn't set.
  List<String> _missing(SecretInfo secret) {
    final has = {for (final variable in secret.variables) variable.name};
    return [
      for (final variable in request.newSecret.variables)
        if (!has.contains(variable.name)) variable.name,
    ];
  }

  /// The variable of [secret] that [name] is given as: the one picked, or
  /// to start with, when one name is missing and one variable unused, that
  /// one; or the secret's only variable.
  String _aliasFor(String name, SecretInfo secret) {
    final picked = _aliases[name];
    if (picked != null) return picked;
    final asked = {for (final v in request.newSecret.variables) v.name};
    final unused = [
      for (final variable in secret.variables)
        if (!asked.contains(variable.name)) variable.name,
    ];
    if (unused.length == 1 && _missing(secret).length == 1) {
      return unused.single;
    }
    if (secret.variables.length == 1) return secret.variables.single.name;
    return '';
  }

  /// Dismiss: what's typed goes as the reason (N11), and the agent is
  /// told to read it, so a question or a "do it this way" makes it ask
  /// again (Lucy's §7). [reason]: the no's, from focus mode's dock, with
  /// the comment.
  Future<void> _dismiss({String? reason}) => _decide(
    () => AppScope.of(context).decideSecretRequest(
      request.agentId,
      request.id,
      approve: false,
      comment: noComment(reason, _comment.text),
    ),
  );

  /// A decision from focus mode's keys (#514) on its way: refused, it
  /// throws, and one the card stopped (a name missing) throws
  /// [AnswerNotSent], its error on the card.
  var _fromDock = false;

  Future<void> _docked(Future<void> Function() run) async {
    _fromDock = true;
    try {
      await run();
    } finally {
      _fromDock = false;
    }
  }

  void _notSent() {
    if (_fromDock) throw const AnswerNotSent();
  }

  Future<void> _decide(Future<void> Function() send) async {
    if (_deciding) {
      _notSent();
      return;
    }
    setState(() {
      _deciding = true;
      _error = null;
    });
    try {
      await send();
      widget.onDecided?.call();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
      if (_fromDock) rethrow;
    } finally {
      if (mounted) setState(() => _deciding = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    super.build(context);
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      // What's typed changes the card's line; the values pasted, whether
      // its yes can go.
      listenable: Listenable.merge([
        store,
        _comment,
        for (final value in _values) value.value,
        _loginUser,
        _loginPassword,
      ]),
      builder: (context, _) {
        // Emptied, what the agent changed is no longer news over the field.
        if (!commentTyped(_comment)) _changedWhileTyping = false;
        final secrets = _secrets = store.workspace.vault.secrets;
        final card = _card(
          context,
          store.agent(request.agentId),
          request.adminAgentId.isEmpty
              ? null
              : store.agent(request.adminAgentId),
          secrets,
        );
        publishAnswer();
        return card;
      },
    );
  }

  /// The person's secrets, as last shown.
  var _secrets = const <SecretInfo>[];

  /// For focus mode's keys (#514): the card's main action (Give, Add and
  /// give, Approve, Generate, Import…), Submit once a comment's typed, and
  /// Dismiss; the secrets to choose from as its choices.
  @override
  CardAnswer? answerNow() {
    if (!_waiting || widget.direct) return null;
    if (request.login) {
      final fits = _fits;
      final Future<void> Function() yes = fits ? _useLogin : _signIn;
      return CardAnswer(
        key: 'agent:${request.agentId}/secret/${request.id}',
        docked: false,
        yes: fits ? 'Use it' : 'Sign in',
        no: 'Dismiss',
        busy: _deciding,
        error: _error,
        onYes: _deciding || (!fits && !_loginReady) ? null : () => _docked(yes),
        onNo: _deciding
            ? null
            : (reason) => _docked(() => _dismiss(reason: reason)),
      );
    }
    final secrets = _secrets;
    String yes = '';
    Future<void> Function()? onYes;
    var choices = const <CardChoice>[];
    CardChoice choice(SecretInfo secret) => CardChoice(
      secret.name,
      chosen: _adding == false && _chosen == secret.id,
      choose: () => setState(() {
        _adding = false;
        _chosen = secret.id;
        _error = null;
        _aliases.clear();
      }),
    );
    if (_imports) {
      final blocked = request.imports.any((file) => file.problem.isNotEmpty);
      final give = !request.grantToNobody;
      yes = give ? 'Import and give' : 'Import only';
      onYes = blocked ? null : () => _import(give: give);
    } else if (_generates) {
      yes = 'Generate';
      onYes = _generate;
    } else if (_answers) {
      final approved =
          request.state == SecretRequestState.SECRET_REQUEST_STATE_APPROVED;
      if (!approved || _here) {
        final chosen = secrets.where((s) => s.id == _chosen).firstOrNull;
        final adding = _adding != false || chosen == null;
        final derives =
            !adding &&
            chosen.hasConnection() &&
            request.adminAgentId.isNotEmpty;
        final admin =
            AppScope.of(context).workspace.agent(request.adminAgentId)?.name ??
            request.adminAgentId;
        yes = derives
            ? 'Let $admin make it'
            : adding
            ? 'Add and give to $_agentName'
            : 'Give ${chosen.name} to $_agentName';
        // Off until every value is in.
        onYes = derives
            ? _approve
            : adding && _missingValues.isNotEmpty
            ? null
            : () => _give(secrets);
        choices = [
          for (final secret in secrets)
            if (_givable(secret)) choice(secret),
        ];
      }
    } else {
      final adding = !_names && _adding == true;
      yes = adding
          ? 'Add and give to $_agentName'
          : _names
          ? 'Give ${request.secretName} to $_agentName'
          : 'Give to $_agentName';
      onYes =
          (!_names && !adding && secrets.isEmpty) ||
              (adding && _missingValues.isNotEmpty)
          ? null
          : () => _give(secrets);
      if (!_names && !_collect) {
        choices = [for (final secret in secrets) choice(secret)];
      }
    }
    final yesNow = onYes;
    // Words typed go with the yes, never in its place (#533): no Submit.
    return CardAnswer(
      key: 'agent:${request.agentId}/secret/${request.id}',
      docked: false,
      yes: yes,
      no: 'Dismiss',
      busy: _deciding,
      error: _error,
      choices: choices.take(9).toList(),
      onYes: _deciding || yesNow == null ? null : () => _docked(yesNow),
      onNo: _deciding
          ? null
          : (reason) => _docked(() => _dismiss(reason: reason)),
    );
  }

  Widget _card(
    BuildContext context,
    Agent? agent,
    Agent? admin,
    List<SecretInfo> secrets,
  ) {
    final palette = Palette.of(context);
    final name = _agentName = agent?.name ?? request.agentId;
    final adminName = admin?.name ?? request.adminAgentId;
    // The admin agent's card is in its color, with its character.
    final shown = _collect ? admin : agent;
    final color = agentColor(shown?.color ?? 'slate');
    final pending = _waiting;
    if (_collect) _adding = true;
    // A new secret's description shows once, inside Edit (N13); a named
    // one's, what it is.
    final description = _names
        ? [
                for (final secret in secrets)
                  if (secret.id == request.secretId) secret.description,
              ].firstOrNull ??
              ''
        : '';
    if (request.login && !widget.direct) {
      return _loginCard(context, palette, agent, color);
    }
    if (widget.direct && widget.onDecided != null) {
      // In Choose a secret…'s dialog, which says whose it is.
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: _pending(context, palette, name, secrets),
      );
    }
    String title;
    if (_imports) {
      title = '$name asks to import ${importKind(request)}';
    } else if (_generates) {
      title = '$name asks for ${keyKind(request)}';
    } else if (_collect) {
      title =
          '$adminName asks you to add a secret for $name: '
          '${request.newSecret.name}';
    } else if (_names) {
      title = '$name asks for ${request.secretName}';
    } else if (_approves) {
      title = '$name needs a secret: ${request.newSecret.name}';
    } else {
      title = '$name asks for a secret: ${request.newSecret.name}';
    }
    // The names in it are the agent's words: what reorders text shows by
    // its code (proseCodes, Fugo).
    title = proseCodes(title);
    if (pending &&
        !_names &&
        !_generates &&
        !_imports &&
        _adding == null &&
        !_answers) {
      // Chooses the secret that looks like the one asked for, if any;
      // otherwise adds the one it describes.
      final closest = closestSecret(request.newSecret, secrets);
      _adding = closest == null;
      _chosen = closest?.id ?? '';
    }
    return Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.06),
        border: Border.all(
          color: color.withValues(alpha: pending ? 0.55 : 0.25),
        ),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              if (shown != null) ...[
                AgentCharacter(agent: shown, size: 20),
                const SizedBox(width: Space.xs + 2),
              ],
              Flexible(
                child: Text(
                  title,
                  style: TypeScale.of(context).caption
                      .copyWith(color: color, fontWeight: FontWeight.w600),
                ),
              ),
              // The agent changed what it asks for since it asked.
              if (pending && request.updatedMs > 0 && !_collect) ...[
                const SizedBox(width: Space.s),
                Text(
                  'Updated '
                  '${logTime(DateTime.fromMillisecondsSinceEpoch(request.updatedMs.toInt()))}',
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textFaint),
                ),
              ],
            ],
          ),
          // What the agent wrote, to decide on: what reorders text shows by
          // its code (proseCodes, Fugo).
          if (description.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            Markdown(
              proseCodes(description),
              color: palette.textMuted,
              shortLinks: true,
            ),
          ],
          if (pending && _changed != null) ...[
            const SizedBox(height: Space.xs),
            Text(
              _changed!,
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.signal),
            ),
          ],
          if (request.reason.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            Markdown(
              proseCodes(request.reason),
              color: pending ? null : palette.textMuted,
              shortLinks: true,
            ),
          ],
          const SizedBox(height: Space.m),
          if (pending && _imports)
            ..._importing(context, palette, name)
          else if (pending && _generates)
            ..._generating(context, palette, name)
          else if (pending && _answers)
            ..._answer(context, palette, name, admin, adminName, secrets)
          else if (pending)
            ..._pending(context, palette, name, secrets)
          else
            ..._decided(palette, color, name),
        ],
      ),
    );
  }

  Widget _loginCard(
    BuildContext context,
    Palette palette,
    Agent? agent,
    Color color,
  ) {
    final type = TypeScale.of(context);
    final name = _agentName;
    final pending = _waiting;
    final site = loginSite(request);
    final fits = _fits;
    const bold = TextStyle(fontWeight: FontWeight.w600);
    return Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.06),
        border: Border.all(
          color: color.withValues(alpha: pending ? 0.55 : 0.25),
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
              // The site is Claktory's reading of the page, never the
              // agent's words.
              Flexible(
                child: Text.rich(
                  TextSpan(
                    children: fits
                        ? [
                            TextSpan(
                              text: proseCodes(
                                'Use your ${request.fittingSecretName} on ',
                              ),
                            ),
                            TextSpan(text: site, style: bold),
                            const TextSpan(text: '?'),
                          ]
                        : [
                            TextSpan(text: '$name asks to sign in to '),
                            TextSpan(text: site, style: bold),
                          ],
                  ),
                  style: type.emphasis.copyWith(
                    color: pending ? palette.text : palette.textMuted,
                  ),
                ),
              ),
            ],
          ),
          if (request.reason.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            Markdown(
              proseCodes(request.reason),
              color: palette.textMuted,
              shortLinks: true,
            ),
          ],
          const SizedBox(height: Space.m),
          if (pending)
            ..._login(context, palette, name, site, fits)
          else
            ..._decided(palette, color, name),
        ],
      ),
    );
  }

  List<Widget> _login(
    BuildContext context,
    Palette palette,
    String agent,
    String site,
    bool fits,
  ) {
    final type = TypeScale.of(context);
    if (fits) {
      return [
        Text(
          proseCodes('${request.fittingSecretName} · yours'),
          style: type.caption.copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.s),
        CardAnswerBlock(
          focus: _commentFocus,
          field: (_) => _commentField(agent),
          row: _loginRow(
            agent,
            site,
            base: '$agent never sees it.',
            primary: Button(
              label: 'Use it',
              primary: true,
              onPressed: _deciding ? null : _useLogin,
            ),
          ),
        ),
        const SizedBox(height: Space.s),
        QuietLink(
          label: 'Use a different login…',
          onTap: _deciding ? null : _useOtherLogin,
        ),
      ];
    }
    final lookalike =
        request.lookalikeSecretName.isNotEmpty &&
        request.lookalikeSite.isNotEmpty;
    return [
      if (lookalike) ...[
        Text(
          proseCodes(
            'This page is $site, not ${request.lookalikeSite}. Your '
            '${request.lookalikeSecretName} is for ${request.lookalikeSite}.',
          ),
          style: type.body.copyWith(color: palette.danger),
        ),
        const SizedBox(height: Space.m),
      ],
      // Cancel: the device never offers to save what goes to the vault.
      AutofillGroup(
        onDisposeAction: AutofillContextAction.cancel,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Field(
              label: 'Email or username',
              child: TextField(
                controller: _loginUser,
                kind: InputKind.email,
                keyboardType: TextInputType.emailAddress,
                autofillHints: const [AutofillHints.username],
                textInputAction: TextInputAction.next,
                semanticsLabel: 'Email or username for $site',
              ),
            ),
            Field(
              label: 'Password',
              child: TextField(
                controller: _loginPassword,
                kind: InputKind.secret,
                autofillHints: const [AutofillHints.password],
                textInputAction: TextInputAction.done,
                onSubmitted: (_) {
                  if (_loginReady && !_deciding) unawaited(_signIn());
                },
                semanticsLabel: 'Password for $site',
              ),
            ),
          ],
        ),
      ),
      if (_replaces case final given?)
        Text(
          proseCodes(
            'Signing in updates your $given for every agent that uses it.',
          ),
          style: type.caption.copyWith(color: palette.textMuted),
        )
      else if (_editing)
        Field(
          label: 'Name',
          child: TextField(
            controller: _name,
            kind: InputKind.text,
            semanticsLabel: 'Name',
          ),
        )
      else
        _summary(context, palette, const []),
      const SizedBox(height: Space.s),
      CardAnswerBlock(
        focus: _commentFocus,
        field: (_) => _commentField(agent),
        row: _loginRow(
          agent,
          site,
          base: 'Claktory types it on $site only. $agent never sees it.',
          refused:
              request.loginOutcome ==
              SecretLoginOutcome.SECRET_LOGIN_OUTCOME_REFUSED,
          primary: Button(
            label: 'Sign in',
            primary: true,
            onPressed: _deciding || !_loginReady ? null : _signIn,
          ),
        ),
      ),
      if (_otherLogin) ...[
        const SizedBox(height: Space.s),
        QuietLink(
          label: proseCodes('Use your ${request.fittingSecretName} instead'),
          onTap: _deciding ? null : () => setState(() => _otherLogin = false),
        ),
      ],
    ];
  }

  Widget _loginRow(
    String agent,
    String site, {
    required String base,
    bool refused = false,
    required Widget primary,
  }) => ListenableBuilder(
    listenable: _comment,
    builder: (context, _) {
      final palette = Palette.of(context);
      final typed = commentTyped(_comment);
      var color = palette.textFaint;
      final String line;
      if (_deciding) {
        line = 'Sending…';
      } else if (_error case final error?) {
        line = error;
        color = palette.danger;
      } else if (refused) {
        line = _replaces == null
            ? "$site didn't accept it. Check it and try again."
            : "$site didn't accept your $_replaces. Check it and try again.";
        color = palette.danger;
      } else {
        line = typed ? '$base $agent gets your words either way.' : base;
      }
      return CardRow(
        quietLine: !typed && !refused && _error == null && !_deciding,
        line: CardLine(line, color: color),
        quiet: [
          Button(label: 'Dismiss', onPressed: _deciding ? null : _dismiss),
        ],
        primary: primary,
      );
    },
  );

  /// A request naming one of the person's secrets (from before agents
  /// described what they need), or the admin agent's card to add the one
  /// it collects (in its color, no Other ways): the one secret layout
  /// (Lucy's Needs you cards §3.5), Give or Add and give.
  List<Widget> _pending(
    BuildContext context,
    Palette palette,
    String agent,
    List<SecretInfo> secrets,
  ) {
    final adding = !_names && _adding == true;
    return [
      if (adding) ..._newSecret(context, palette),
      CardAnswerBlock(
        focus: _commentFocus,
        field: (_) => _commentField(agent),
        row: _row(
          agent,
          base: '$agent never sees the ${_valueWord(adding)}.',
          neverSees: true,
          values: adding,
          primary: Button(
            // The same words on every device, stacking on a phone when
            // they don't fit (Trish 43715).
            label: _names
                ? 'Give ${request.secretName} to $agent'
                : 'Add and give to $agent',
            primary: true,
            onPressed: _deciding || (adding && _missingValues.isNotEmpty)
                ? null
                : () => _give(secrets),
          ),
        ),
      ),
    ];
  }

  /// "value", or "values" when it asks for several.
  String _valueWord(bool adding) =>
      adding && _values.length > 1 ? 'values' : 'value';

  /// The variables still without a value, on a new secret.
  List<String> get _missingValues => [
    for (final value in _values)
      if (value.value.text.isEmpty) value.name.text,
  ];

  /// A new secret, as only the person can supply it: a value to paste for
  /// each variable, then its settings folded into "Saved as … · Edit".
  List<Widget> _newSecret(BuildContext context, Palette palette) => [
    ..._valueFields(context, palette, paste: 'Paste it here'),
    if (_editing) ..._nameFields() else _summary(context, palette, const []),
    const SizedBox(height: Space.s),
  ];

  /// The new secret's name and what it grants, opened by Edit: the
  /// agent's description shows only here (N13).
  List<Widget> _nameFields() => [
    // Each field carries its own name to a screen reader: the
    // label above it may have scrolled away.
    Field(
      label: 'Name',
      child: TextField(
        controller: _name,
        kind: InputKind.text,
        semanticsLabel: 'Name',
      ),
    ),
    Field(
      label: secretGrantsLabel,
      // The agent's words, never Claktory's (Fugo).
      note: request.newSecret.description.isEmpty
          ? secretGrantsNote
          : 'As $_agentName described it. $secretGrantsNote',
      child: TextField(
        controller: _description,
        placeholder: secretGrantsExample,
        kind: InputKind.multiline,
        maxLines: 3,
        semanticsLabel: secretGrantsLabel,
      ),
    ),
  ];

  /// A value to paste for each variable (never one the agent filled in,
  /// Fugo).
  List<Widget> _valueFields(
    BuildContext context,
    Palette palette, {
    String paste = 'Paste the value',
  }) => [
    for (final value in _values)
      Padding(
        padding: const EdgeInsets.only(bottom: Space.s),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              value.file ? '${value.name.text} (file)' : value.name.text,
              style: TypeScale.of(context).caption.copyWith(
                fontFamily: TextStyles.mono,
                color: palette.textMuted,
              ),
            ),
            const SizedBox(height: Space.xs),
            value.file
                ? TextField(
                    controller: value.value,
                    placeholder: 'Paste the file’s contents',
                    kind: InputKind.multiline,
                    autocorrect: false,
                    textCapitalization: TextCapitalization.none,
                    maxLines: 6,
                    minLines: 3,
                    semanticsLabel: '${value.name.text} (file)',
                  )
                : TextField(
                    controller: value.value,
                    placeholder: paste,
                    kind: InputKind.secret,
                    semanticsLabel: value.name.text,
                  ),
          ],
        ),
      ),
  ];

  /// A key or password for Claktory to make: its settings folded into one
  /// line ("Saved as build-box deploy · For Mista · Doesn't ask each time
  /// · Edit"), Edit opening its name (also the key's comment), a
  /// password's length and Ask each time in place; then the field and
  /// Dismiss · Generate.
  List<Widget> _generating(
    BuildContext context,
    Palette palette,
    String agent,
  ) => [
    if (_editing) ...[
      Field(
        label: 'Name',
        note: _password ? null : "Also the key's comment",
        child: TextField(
          controller: _name,
          kind: InputKind.text,
          semanticsLabel: 'Name',
        ),
      ),
      if (_password) ...[
        _PasswordLength(
          spec: request.password,
          length: _passwordLength,
          onChanged: _deciding
              ? null
              : (length) => setState(() => _length = length),
        ),
        const SizedBox(height: Space.m),
      ],
      _ToggleRow(
        label: askEachTimeLabel,
        note: askEachTimeNote,
        value: _askEach,
        onChanged: (on) => setState(() => _askEach = on),
      ),
      const SizedBox(height: Space.m),
    ] else ...[
      _summary(context, palette, [
        request.grantToNobody ? 'For no one' : 'For $agent',
        _askEach ? 'Asks each time' : "Doesn't ask each time",
        if (_password) '$_passwordLength characters',
      ]),
      const SizedBox(height: Space.s),
    ],
    CardAnswerBlock(
      focus: _commentFocus,
      field: (_) => _commentField(agent),
      row: _row(
        agent,
        base: _password
            ? 'No one ever sees it, $agent included.'
            : 'No one ever sees its private half, $agent included.',
        primary: Button(
          label: 'Generate',
          primary: true,
          onPressed: _deciding ? null : _generate,
        ),
      ),
    ),
  ];

  /// Files for the vault: whether whom it asks for may have been led to
  /// (it read another's words), each file's box whole as Claktory found
  /// it (its full path, size, when it changed, a fingerprint, where it is
  /// if that holds keys, and why it can't be imported now; Fugo), its
  /// settings folded into one line (Edit: the name, Ask each time,
  /// deleting the originals), the field, then Dismiss · Import only ·
  /// Import and give to… (#360 K7: two deliberate choices).
  List<Widget> _importing(BuildContext context, Palette palette, String agent) {
    final blocked = request.imports.any((file) => file.problem.isNotEmpty);
    final one = request.imports.length == 1;
    return [
      if (request.askerReadPipes) ...[
        _ReadOutside(agent: agent),
        const SizedBox(height: Space.m),
      ],
      for (final file in request.imports) ...[
        _ImportFile(file),
        const SizedBox(height: Space.s),
      ],
      Text(
        'Files are stored exactly as they are, binary files too.',
        style: TypeScale.of(context).caption.copyWith(color: palette.textFaint),
      ),
      const SizedBox(height: Space.s),
      if (_editing) ...[
        const SizedBox(height: Space.xs),
        Field(
          label: 'Name',
          child: TextField(
            controller: _name,
            kind: InputKind.text,
            semanticsLabel: 'Name',
          ),
        ),
        _ToggleRow(
          label: askEachTimeLabel,
          note: askEachTimeNote,
          value: _askEach,
          onChanged: _deciding ? null : (on) => setState(() => _askEach = on),
        ),
        const SizedBox(height: Space.m),
        _ToggleRow(
          label: one
              ? 'Delete the original after it’s imported'
              : 'Delete the originals after they’re imported',
          note: 'Deleted like any file, not securely erased.',
          value: _deleteOriginals,
          onChanged: _deciding
              ? null
              : (on) => setState(() => _deleteOriginals = on),
        ),
        const SizedBox(height: Space.m),
      ] else ...[
        _summary(context, palette, [
          _askEach ? 'Asks each time' : "Doesn't ask each time",
          _deleteOriginals
              ? (one ? 'Deletes the original' : 'Deletes the originals')
              : (one ? 'Keeps the original' : 'Keeps the originals'),
        ]),
        const SizedBox(height: Space.s),
      ],
      CardAnswerBlock(
        focus: _commentFocus,
        field: (_) => _commentField(agent),
        row: _row(
          agent,
          base: blocked
              ? "It can't be imported as it is."
              : request.grantToNobody
              ? "No one sees what's in it. No agent gets it until you give it "
                    'to one.'
              : "No one sees what's in it, $agent included.",
          baseColor: blocked ? palette.danger : null,
          second: request.grantToNobody
              ? null
              : Button(
                  label: 'Import only',
                  onPressed: _deciding || blocked
                      ? null
                      : () => _import(give: false),
                ),
          primary: request.grantToNobody
              ? Button(
                  label: 'Import only',
                  primary: true,
                  onPressed: _deciding || blocked
                      ? null
                      : () => _import(give: false),
                )
              : Button(
                  label: 'Import and give to $agent',
                  primary: true,
                  onPressed: _deciding || blocked
                      ? null
                      : () => _import(give: true),
                ),
        ),
      ),
    ];
  }

  /// The request, answered here (#360): one main action, chosen by what
  /// the vault holds (fitting_secret, the backend's choice). A secret that
  /// fits: Give it. A connected service that can make it: Let the admin
  /// agent make it. None: a field per value to paste, under Get it on
  /// {host} where the agent gave a link, and Add and give. Other ways…
  /// gives one of the person's other secrets, a new one, or the admin
  /// agent's choice. Once the admin agent has it, its wait, and Add it
  /// here instead.
  List<Widget> _answer(
    BuildContext context,
    Palette palette,
    String agent,
    Agent? admin,
    String adminName,
    List<SecretInfo> secrets,
  ) {
    final type = TypeScale.of(context);
    final hasAdmin = request.adminAgentId.isNotEmpty;
    final approved =
        request.state == SecretRequestState.SECRET_REQUEST_STATE_APPROVED;
    if (approved && !_here) {
      // Nothing waits on the person: no primary.
      return [
        _row(
          agent,
          base: adminWaitLine(request, admin, adminName),
          baseColor: palette.textMuted,
          second: Button(
            label: 'Add it here instead',
            onPressed: _deciding
                ? null
                : () => setState(() {
                    _here = true;
                    _error = null;
                  }),
          ),
        ),
      ];
    }
    if (_adding == null) {
      final fitting = secrets
          .where((secret) => secret.id == request.fittingSecretId)
          .firstOrNull;
      _adding = fitting == null;
      _chosen = fitting?.id ?? '';
    }
    final chosen = secrets.where((secret) => secret.id == _chosen).firstOrNull;
    final adding = _adding! || chosen == null;
    // A connected service makes one: the admin agent does it.
    final derives = !adding && chosen.hasConnection() && hasAdmin;
    // What can be given instead: the person's own secrets ([_givable]), a
    // new one, or the admin agent's choice.
    final others = [
      for (final secret in secrets)
        if (_givable(secret) && (adding || secret.id != _chosen)) secret,
    ];
    final variables = [
      for (final variable in request.newSecret.variables) variable.name,
    ];
    final link = request.link;
    final showsLink = adding && opensLink(request);
    final Widget primary;
    final String base;
    if (derives) {
      primary = Button(
        label: 'Let $adminName make it',
        primary: true,
        onPressed: _deciding ? null : _approve,
      );
      base = '$adminName makes one from your ${chosen.name} connection.';
    } else {
      primary = Button(
        // The same words on every device, stacking on a phone when they
        // don't fit (Trish 43715).
        label: adding
            ? 'Add and give to $agent'
            : 'Give ${chosen.name} to $agent',
        primary: true,
        onPressed: _deciding || (adding && _missingValues.isNotEmpty)
            ? null
            : () => _give(secrets),
      );
      base = '$agent never sees the ${_valueWord(adding)}.';
    }
    return [
      if (showsLink) ...[
        Align(
          alignment: AlignmentDirectional.centerStart,
          child: Button(
            label: 'Get it on ${request.linkHost}',
            onPressed: () => unawaited(openLink(context, Uri.parse(link))),
          ),
        ),
        // The agent's words, quoted as its own, never as Claktory's
        // (Fugo).
        if (request.steps.isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          Text(
            proseCodes('$agent: “${request.steps}”'),
            style: type.caption.copyWith(
              color: palette.textMuted,
              fontStyle: FontStyle.italic,
            ),
          ),
        ],
        const SizedBox(height: Space.m),
      ],
      if (!adding && variables.isNotEmpty) ...[
        Text(
          variables.length == 1
              ? 'Sets ${variables.single}'
              : 'Sets ${_and(variables)}',
          style: type.caption.copyWith(
            fontFamily: TextStyles.mono,
            color: palette.textFaint,
          ),
        ),
        const SizedBox(height: Space.m),
      ],
      if (!adding && !derives && _missing(chosen).isNotEmpty)
        _mismatch(palette, agent, chosen),
      if (adding) ..._newSecret(context, palette),
      // In focus mode, the secrets to give instead are a list in place,
      // numbered for its keys (FS5, #514); elsewhere Other ways…, in the
      // card above its field, never in the thumb row (Lucy's §5.2).
      if (inDock && secrets.any(_givable)) ...[
        _SecretChoices(
          choices: answerNow()?.choices ?? const [],
          adding: adding,
          onAdd: _deciding
              ? null
              : () => setState(() {
                  _adding = true;
                  _error = null;
                  _aliases.clear();
                }),
          adminName: request.adminAgentId.isNotEmpty && !derives
              ? adminName
              : null,
          onAdmin: _deciding ? null : () => unawaited(_approve()),
        ),
        const SizedBox(height: Space.m),
      ] else if (others.isNotEmpty ||
          !adding ||
          (request.adminAgentId.isNotEmpty && !derives)) ...[
        // Give one of yours, a new one, or the admin agent's choice: one
        // quiet link, not buttons of their own (rule 3).
        Builder(
          builder: (context) => QuietLink(
            label: 'Other ways…',
            onTap: _deciding
                ? null
                : () => _another(context, others, adding, derives, adminName),
          ),
        ),
        const SizedBox(height: Space.s),
      ],
      CardAnswerBlock(
        focus: _commentFocus,
        field: (_) => _commentField(agent),
        row: _row(
          agent,
          base: base,
          neverSees: !derives,
          values: adding,
          primary: primary,
        ),
      ),
    ];
  }

  /// The field, open from the start (Lucy's rule 4): its words go with
  /// whichever the person presses (D4), as the reason with Dismiss.
  /// Return in it sends nothing (rule 6).
  Widget _commentField(String agent) => DecisionComment(
    controller: _comment,
    focusNode: _commentFocus,
    agent: agent,
    enabled: !_deciding,
    above: _changedWhileTyping && commentTyped(_comment)
        ? '$_agentName updated this request while you were typing. Your '
              'words are kept.'
        : null,
  );

  /// The card's row (Lucy's rule 2): what pressing does, then Dismiss,
  /// any [second] action and the [primary]. Its buttons never change as
  /// the person types (rule 7); the line does: with [values] (a new
  /// secret), what's still to paste, or that Dismiss discards what was
  /// pasted; with words typed, that the agent gets them either way. Else
  /// [base] ([neverSees]: "Kai never sees the value", which words join).
  /// Sending, it says so; refused, why, in `danger`.
  Widget _row(
    String agent, {
    required String base,
    Color? baseColor,
    bool neverSees = false,
    bool values = false,
    Widget? second,
    Widget? primary,
  }) => ListenableBuilder(
    listenable: Listenable.merge([
      _comment,
      for (final value in _values) value.value,
    ]),
    builder: (context, _) {
      final palette = Palette.of(context);
      final typed = commentTyped(_comment);
      final pasted =
          values && _values.any((value) => value.value.text.isNotEmpty);
      final missing = values ? _missingValues : const <String>[];
      final either = '$agent gets your words either way.';
      final String line;
      var color = baseColor ?? palette.textFaint;
      // Only "June never sees the values." as it is: a hint (Lucy 45566).
      var hint = false;
      if (_deciding) {
        line = 'Sending…';
        color = palette.textFaint;
      } else if (_error case final error?) {
        line = error;
        color = palette.danger;
      } else if (pasted && missing.isNotEmpty) {
        line = 'Enter ${_and(missing)} first.';
      } else if (pasted) {
        line = typed
            ? 'Dismiss discards what you pasted. $either'
            : 'Dismiss discards what you pasted.';
      } else if (typed && neverSees) {
        // "Kai never sees the value." → "Kai gets your words either way,
        // and never sees the value."
        line =
            '$agent gets your words either way, and '
            '${base.substring(agent.length + 1)}';
      } else {
        line = typed ? '$base $either' : base;
        hint = !typed && neverSees;
      }
      return CardRow(
        quietLine: hint,
        line: CardLine(line, color: color),
        quiet: [
          // Its words go as the reason (N11): the agent reads them.
          Button(label: 'Dismiss', onPressed: _deciding ? null : _dismiss),
          ?second,
        ],
        primary: primary,
      );
    },
  );

  /// Its settings are open to change (Edit), on a new secret, a key to
  /// make or files to import.
  var _editing = false;

  /// On a login request one of the person's logins fits: they chose to sign
  /// in with another instead.
  var _otherLogin = false;

  /// One of the person's logins fits, and they haven't chosen another.
  bool get _fits => request.fittingSecretId.isNotEmpty && !_otherLogin;

  /// Signs in with another login than the one that fits, saved under a
  /// name of its own ("X login 2"), since that one's is taken.
  void _useOtherLogin() {
    final taken = {
      for (final secret in AppScope.of(
        context,
      ).workspace.workspace.vault.secrets)
        secret.name,
    };
    final base = _name.text.trim();
    if (taken.contains(base)) {
      var n = 2;
      while (taken.contains('$base $n')) {
        n++;
      }
      _name.text = '$base $n';
    }
    setState(() => _otherLogin = true);
  }

  /// "Saved as Resend · For Mista · Asks each time · Edit": the secret's
  /// name and its settings, folded into one line (N15, Lucy's §3.5).
  Widget _summary(BuildContext context, Palette palette, List<String> more) =>
      ListenableBuilder(
        listenable: _name,
        builder: (context, _) => Wrap(
          crossAxisAlignment: WrapCrossAlignment.center,
          spacing: Space.s,
          children: [
            Text(
              [
                _name.text.trim().isEmpty
                    ? 'Not named yet'
                    : 'Saved as ${_name.text.trim()}',
                ...more,
              ].join(' · '),
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textMuted),
            ),
            QuietLink(
              label: 'Edit',
              onTap: () => setState(() => _editing = true),
            ),
          ],
        ),
      );

  /// Give another…: one of the person's other secrets, a new one, or the
  /// admin agent's choice (approve_only).
  Future<void> _another(
    BuildContext context,
    List<SecretInfo> others,
    bool adding,
    bool derives,
    String adminName,
  ) async {
    const add = '\u0000add';
    const admin = '\u0000admin';
    final chosen = await showActions<String>(
      context,
      actions: [
        for (final secret in others) SheetAction(secret.name, secret.id),
        if (!adding) const SheetAction('Add a new one', add),
        if (request.adminAgentId.isNotEmpty && !derives)
          SheetAction('Let $adminName choose', admin),
      ],
    );
    if (!mounted || chosen == null) return;
    if (chosen == admin) {
      await _approve();
      return;
    }
    setState(() {
      _error = null;
      _aliases.clear();
      if (chosen == add) {
        _adding = true;
      } else {
        _adding = false;
        _chosen = chosen;
      }
    });
  }

  Future<void> _approve() => _decide(
    () => AppScope.of(context).decideSecretRequest(
      request.agentId,
      request.id,
      approve: true,
      approveOnly: true,
      comment: _comment.text.trim(),
    ),
  );

  /// The chosen secret doesn't set every name the agent asked for: which
  /// it doesn't, and whether to give it under those names too, each as
  /// which of its variables, or as it is.
  Widget _mismatch(Palette palette, String agent, SecretInfo secret) {
    final missing = _missing(secret);
    final own = [for (final variable in secret.variables) variable.name];
    // Several rows, one per name asked for, to say which variable each is.
    final rows = missing.length > 1 || own.length > 1;
    final caption = TypeScale.of(context).caption
        .copyWith(color: palette.textMuted);
    final mono = TypeScale.of(context).caption
        .copyWith(fontFamily: TextStyles.mono, color: palette.text);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            mismatchLine(
              agent: agent,
              asked: [
                for (final variable in request.newSecret.variables)
                  variable.name,
              ],
              sets: own,
            ),
            style: caption,
          ),
          const SizedBox(height: Space.s),
          _ToggleRow(
            label: missing.length == 1
                ? 'Also give it as ${missing.single}'
                : 'Also give it under the names $agent asked for',
            note: _alias
                ? rows
                      ? null
                      : '$agent gets its value as ${own.single} and '
                            '${missing.single}.'
                : own.length == 1
                ? '$agent gets it only as ${own.single}.'
                : '$agent gets it under its own names only.',
            value: _alias,
            onChanged: (on) => setState(() => _alias = on),
          ),
          if (_alias && rows)
            for (final name in missing)
              Padding(
                padding: const EdgeInsets.only(top: Space.s),
                child: _aliasRow(
                  name: Text(name, style: mono),
                  choice: Select<String>(
                    value: _aliasFor(name, secret),
                    options: [
                      for (final variable in secret.variables)
                        SelectOption(variable.name, 'Same as ${variable.name}'),
                      const SelectOption('', 'Leave it out'),
                    ],
                    onChanged: (variable) =>
                        setState(() => _aliases[name] = variable),
                  ),
                ),
              ),
        ],
      ),
    );
  }

  /// A name asked for and which of the secret's variables it is: side by
  /// side, or on a phone the choice under the name, the column's width.
  Widget _aliasRow({required Widget name, required Widget choice}) {
    if (isPhoneLayout(context)) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          name,
          const SizedBox(height: Space.xs),
          choice,
        ],
      );
    }
    return Row(
      children: [
        Expanded(child: name),
        const SizedBox(width: Space.s),
        SizedBox(width: 280, child: choice),
      ],
    );
  }

  List<Widget> _decided(Palette palette, Color color, String agent) {
    final dismissed =
        request.state == SecretRequestState.SECRET_REQUEST_STATE_DISMISSED;
    // Left unanswered too long: the agent was told and can ask again.
    final expired =
        request.state == SecretRequestState.SECRET_REQUEST_STATE_EXPIRED;
    // The agent took it back (its reason is the comment).
    final withdrawn =
        request.state == SecretRequestState.SECRET_REQUEST_STATE_WITHDRAWN;
    final decided = request.decidedMs.toInt();
    final byAdmin = _byAdmin(request);
    return [
      // Who decided: the person (not when it expired, the agent took it
      // back, or it may have been the admin agent).
      Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (!expired && !withdrawn && !byAdmin) ...[
            const PersonAvatar(size: 16),
            const SizedBox(width: Space.xs + 2),
          ],
          Flexible(
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    // The card's own outcome, as Needs you and History
                    // say it (Lucy's §8).
                    text: secretOutcome(
                      request,
                      agent: agent,
                      collected:
                          widget.shownIn != null &&
                          widget.shownIn == request.adminAgentId &&
                          widget.shownIn != request.agentId,
                    ),
                    style: TextStyle(
                      color: dismissed || expired || withdrawn
                          ? palette.textMuted
                          : color,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  if (decided > 0)
                    TextSpan(
                      text:
                          ' · ${logTime(DateTime.fromMillisecondsSinceEpoch(decided))}',
                      style: TextStyle(color: palette.textFaint),
                    ),
                ],
              ),
              style: TypeScale.of(context).caption,
            ),
          ),
        ],
      ),
      if (request.comment.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(
          '“${request.comment}”',
          style: TypeScale.of(context).body.copyWith(color: palette.textMuted),
        ),
      ],
      // What was imported (or would have been): each file, as found.
      if (_imports)
        for (final file in request.imports) ...[
          const SizedBox(height: Space.xs),
          Text(
            '${importPath(file.path)} · ${formatBytes(file.size)} · '
            'fingerprint ${file.fingerprint}',
            style: TypeScale.of(context).caption.copyWith(
              fontFamily: TextStyles.mono,
              color: palette.textMuted,
            ),
          ),
        ],
      if (request.publicKey.isNotEmpty) ...[
        const SizedBox(height: Space.m),
        // Its public half, to put where the agent will use it (#360 K6):
        // Copy key and Open {host}, staying on the card (Lucy's §3.5).
        _PublicKey(request),
        if (request.doneMs > 0) ...[
          const SizedBox(height: Space.s),
          Text(
            'Done. $agent knows the key is in place.',
            style: TypeScale.of(context).caption.copyWith(color: color),
          ),
        ],
      ],
    ];
  }
}

/// A password Claktory makes: what it is, its strength, and its length to
/// change between the spec's shortest and longest, with − and +.
class _PasswordLength extends StatelessWidget {
  const _PasswordLength({
    required this.spec,
    required this.length,
    required this.onChanged,
  });

  final PasswordSpec spec;
  final int length;
  final ValueChanged<int>? onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final shortest = spec.shortest;
    final longest = spec.longest;
    Widget step(String label, String icon, int to) {
      final enabled = onChanged != null && to >= shortest && to <= longest;
      return Opacity(
        opacity: enabled ? 1 : 0.4,
        child: GlyphButton(
          label: label,
          active: false,
          onTap: enabled ? () => onChanged!(to) : () {},
          glyph: (color) =>
              ExtensionGlyph(icon: icon, name: label, color: color),
        ),
      );
    }

    final notes = [
      if (spec.allowed.isNotEmpty) 'Only these characters: ${spec.allowed}',
      if (spec.avoidSimilar) 'No characters that look alike, such as 0 and O.',
      if (spec.maxLength > 0 && longest == spec.maxLength)
        'The service accepts at most ${spec.maxLength} characters.',
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          passwordSummary(spec, length),
          style: type.body.copyWith(color: palette.text),
        ),
        const SizedBox(height: Space.xs),
        Wrap(
          crossAxisAlignment: WrapCrossAlignment.center,
          spacing: Space.s,
          runSpacing: Space.xs,
          children: [
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                step('Shorter', 'minus', length - 1),
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: Space.xs),
                  child: Text(
                    '$length',
                    style: type.body.copyWith(
                      color: palette.text,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                ),
                step('Longer', 'plus', length + 1),
              ],
            ),
            Text(
              'Strength: ${passwordStrength(passwordBits(spec, length))}',
              style: type.caption.copyWith(color: palette.textMuted),
            ),
          ],
        ),
        for (final note in notes) ...[
          const SizedBox(height: Space.xs),
          Text(note, style: type.caption.copyWith(color: palette.textFaint)),
        ],
      ],
    );
  }
}

/// A file an agent asks the vault to import, as Claktory found it: the
/// variable it goes in, its full path (hidden characters shown as codes,
/// never shortened), its size, when it changed and its fingerprint; where
/// it is when that holds keys; and why it can't be imported now.
class _ImportFile extends StatelessWidget {
  const _ImportFile(this.file);

  final SecretImport file;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final mono = type.caption.copyWith(fontFamily: TextStyles.mono);
    final sensitive = importSensitive(file.sensitive);
    final changed = DateTime.fromMillisecondsSinceEpoch(
      file.modifiedMs.toInt(),
    );
    return Container(
      padding: const EdgeInsets.all(Space.s),
      decoration: BoxDecoration(
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            '${file.variable} (file)',
            style: mono.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.xs),
          Text(
            importPath(file.path),
            style: type.body.copyWith(
              fontFamily: TextStyles.mono,
              color: palette.text,
            ),
          ),
          const SizedBox(height: Space.xs),
          Text(
            '${formatBytes(file.size)} · changed ${logTime(changed)} · '
            'fingerprint ${file.fingerprint}',
            style: type.caption.copyWith(color: palette.textFaint),
          ),
          if (sensitive != null) ...[
            const SizedBox(height: Space.xs),
            Text(
              sensitive,
              style: type.caption.copyWith(
                color: palette.danger,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
          if (file.problem.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            Text(
              file.problem,
              style: type.caption.copyWith(color: palette.danger),
            ),
          ],
        ],
      ),
    );
  }
}

/// The agent that asks had read a web page when it asked (asker_read_pipes:
/// since #346 only a page read through browser control marks an agent):
/// what it asks for may come from whoever wrote it. After a globe, as cards
/// say where a request may come from.
class _ReadOutside extends StatelessWidget {
  const _ReadOutside({required this.agent});

  final String agent;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(top: 2),
          child: ExcludeSemantics(
            child: ExtensionGlyph(
              icon: 'globe',
              name: '',
              color: palette.textMuted,
              size: 12,
            ),
          ),
        ),
        const SizedBox(width: Space.xs + 2),
        Expanded(
          child: Text(
            '$agent had read a web page when it asked. Whoever wrote it '
            'could have led it to ask for this. Import only if you expected '
            'it.',
            style: type.body.copyWith(
              color: palette.text,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
      ],
    );
  }
}

/// A key Claktory made (SecretRequest.public_key): its public half, then
/// Open {host} where the agent gave one, and Copy key.
class _PublicKey extends StatefulWidget {
  const _PublicKey(this.request);

  final SecretRequest request;

  @override
  State<_PublicKey> createState() => _PublicKeyState();
}

class _PublicKeyState extends State<_PublicKey> {
  var _copied = false;

  @override
  Widget build(BuildContext context) {
    final request = widget.request;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        CodeBlock(request.publicKey),
        const SizedBox(height: Space.s),
        CardRow(
          line: const CardLine(
            'Public key: safe to share, such as with GitHub or a server.',
          ),
          quiet: [
            if (opensLink(request))
              Button(
                label: 'Open ${request.linkHost}',
                onPressed: () =>
                    unawaited(openLink(context, Uri.parse(request.link))),
              ),
          ],
          primary: Button(
            label: _copied ? 'Copied' : 'Copy key',
            primary: true,
            onPressed: () async {
              await Clipboard.setData(ClipboardData(text: request.publicKey));
              if (mounted) setState(() => _copied = true);
            },
          ),
        ),
      ],
    );
  }
}

/// A secret request's choices in focus mode (FS5, #514): the person's
/// secrets in place, numbered for 1 to 9, the one chosen marked; then a
/// new one, and the admin agent's choice.
class _SecretChoices extends StatelessWidget {
  const _SecretChoices({
    required this.choices,
    required this.adding,
    required this.onAdd,
    required this.adminName,
    required this.onAdmin,
  });

  final List<CardChoice> choices;
  final bool adding;
  final VoidCallback? onAdd;
  final String? adminName;
  final VoidCallback? onAdmin;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    Widget row(String? number, String label, bool chosen, VoidCallback? tap) =>
        Semantics(
          button: true,
          selected: chosen,
          child: Pressable(
            onTap: tap,
            builder: (context, hovered) => Container(
              constraints: const BoxConstraints(minHeight: 28),
              padding: const EdgeInsets.symmetric(
                horizontal: Space.s,
                vertical: Space.xs,
              ),
              decoration: BoxDecoration(
                border: Border(
                  left: BorderSide(
                    width: 2,
                    color: chosen ? palette.signal : const Color(0x00000000),
                  ),
                ),
              ),
              child: Row(
                children: [
                  SizedBox(
                    width: 20,
                    child: Text(
                      number ?? '',
                      style: type.caption.copyWith(color: palette.textFaint),
                    ),
                  ),
                  Expanded(
                    child: Text(
                      label,
                      style: type.body.copyWith(
                        color: chosen || hovered
                            ? palette.text
                            : palette.textMuted,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'Or give one of yours',
          style: type.caption.copyWith(color: palette.textFaint),
        ),
        const SizedBox(height: Space.xs),
        for (final (index, choice) in choices.indexed)
          row(
            '${index + 1}',
            choice.label,
            choice.chosen,
            onAdd == null ? null : choice.choose,
          ),
        row(null, 'Add a new one', adding, onAdd),
        if (adminName case final admin?)
          row(null, 'Let $admin choose', false, onAdmin),
      ],
    );
  }
}
