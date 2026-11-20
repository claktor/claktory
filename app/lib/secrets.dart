import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'account.dart' show signInPutOff;
import 'backend.dart';
import 'characters/character.dart';
import 'connectors.dart';
import 'controls.dart';
import 'extensions.dart' show logTime;
import 'forms.dart';
import 'identity.dart';
import 'markdown.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';
import 'usage.dart' show formatAge;

/// How long a value shown stays on screen.
const secretShownFor = Duration(seconds: 30);

/// How long a copied value stays on the clipboard.
const secretCopiedFor = Duration(minutes: 1);

VaultState _vault(Backend backend) => backend.workspace.workspace.vault;

/// An agent's name, or its id when the app doesn't know it.
String _agentName(Backend backend, String id) =>
    backend.workspace.agent(id)?.name ?? id;

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
              ),
              const SizedBox(height: Space.l),
            ],
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
                style: TextStyles.body.copyWith(color: palette.textFaint),
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
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          ],
        );
      },
    );
  }
}

/// Locked since a restart, and why: with a Claktory account, what the
/// backend says (claktory.com can't be reached, or the account no longer
/// opens them), or that this Claktory was removed from the account. The
/// recovery key opens them meanwhile.
class _Locked extends StatelessWidget {
  const _Locked({required this.account});

  /// This Claktory's account, when it has one.
  final ClaktoryAccount? account;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final account = this.account;
    final removed =
        account?.state == ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_REVOKED;
    final why = account == null
        ? "Secrets are locked. Agents can't use them until you unlock them."
        : removed
        ? 'Secrets are locked because this Claktory was removed from your '
              'account. Sign in again to unlock them, or use your recovery '
              'key.'
        // Signed in: the backend's own words; none while it's asking.
        : 'Secrets are locked. '
              '${account.problem.isEmpty ? 'Opening them…' : account.problem}';
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
          Text(why, style: TextStyles.body.copyWith(color: palette.text)),
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
    fields: [
      Field(
        label: 'Recovery key',
        child: TextField(
          controller: _key,
          autofocus: true,
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
  });

  final String label;
  final String? note;
  final bool value;
  final ValueChanged<bool>? onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Row(
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
              Text(label, style: TextStyles.body.copyWith(color: palette.text)),
              if (note != null)
                Text(
                  note!,
                  style: TextStyles.caption.copyWith(color: palette.textMuted),
                ),
            ],
          ),
        ),
      ],
    );
  }
}

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

  @override
  void dispose() {
    _hide?.cancel();
    _shown = null;
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

  Future<void> _give() async {
    final backend = AppScope.of(context);
    final given = {for (final grant in secret.grants) grant.agentId};
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
        title: 'Give ${secret.name} to',
        agents: agents,
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
      await giveSecret(context, secret, picked);
    } on RequestFailure catch (failure) {
      if (mounted) _say(failure.message, error: true);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final given = [
      for (final grant in secret.grants) _agentName(backend, grant.agentId),
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
                              style: TextStyles.label.copyWith(
                                color: palette.text,
                              ),
                            ),
                            if (secret.description.isNotEmpty)
                              Text(
                                secret.description,
                                maxLines: _open ? null : 2,
                                overflow: _open ? null : TextOverflow.ellipsis,
                                style: TextStyles.body.copyWith(
                                  color: palette.textMuted,
                                ),
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
                                style: TextStyles.caption.copyWith(
                                  fontFamily: TextStyles.mono,
                                  color: palette.textFaint,
                                ),
                              ),
                            Text(
                              summary,
                              style: TextStyles.caption.copyWith(
                                color: palette.textFaint,
                              ),
                            ),
                            for (final warning in secret.warnings)
                              Text(
                                warning,
                                style: TextStyles.caption.copyWith(
                                  color: palette.danger,
                                ),
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
                          style: TextStyles.caption.copyWith(
                            fontFamily: TextStyles.mono,
                            color: palette.textMuted,
                          ),
                        ),
                        Text(
                          shown?[variable.name] ??
                              (variable.hint.isEmpty
                                  ? '••••••'
                                  : '••••••${variable.hint}'),
                          style: TextStyles.body.copyWith(
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
                      style: TextStyles.caption.copyWith(
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
                style: TextStyles.caption.copyWith(
                  color: _noteIsError ? palette.danger : palette.textMuted,
                ),
              ),
            ),
          const SizedBox(height: Space.m),
          for (final grant in secret.grants)
            _GrantRow(secret: secret, grant: grant),
          const SizedBox(height: Space.s),
          Wrap(
            spacing: Space.s,
            runSpacing: Space.s,
            children: [
              Button(label: 'Give to…', onPressed: _give),
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
              style: TextStyles.caption.copyWith(
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
                    text: agent?.name ?? grant.agentId,
                    style: TextStyle(color: palette.text),
                  ),
                  if (grant.runningTasks > 0)
                    TextSpan(
                      text:
                          '  ${grant.runningTasks} running '
                          '${grant.runningTasks == 1 ? 'task' : 'tasks'} '
                          'using it',
                      style: TextStyles.caption.copyWith(
                        color: palette.textFaint,
                      ),
                    ),
                ],
              ),
              style: TextStyles.body,
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
      style: TextStyles.caption,
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

/// Gives [secret] to [agent].
Future<void> giveSecret(BuildContext context, SecretInfo secret, Agent agent) =>
    AppScope.of(context).grantSecret(secret.id, agent.id);

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
      body:
          "$name can't use it from now on. It may still know a value it has "
          'used: to be sure, make a new key where it came from and update '
          'it here.',
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
  });

  final String title;
  final List<Agent> agents;
  final ValueChanged<Agent> pick;
  final VoidCallback close;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return FormFrame(
      title: title,
      fields: [
        for (final agent in agents)
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
                        style: TextStyles.body.copyWith(color: palette.text),
                      ),
                      if (agent.role.isNotEmpty) ...[
                        const SizedBox(width: Space.s),
                        Text(
                          agent.role,
                          style: TextStyles.caption.copyWith(
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
  var _busy = false;
  String? _error;

  @override
  void dispose() {
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
        await backend.createSecret(name, _description.text.trim(), entries);
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
            autofocus: editing == null,
          ),
        ),
        Field(
          label: 'Description',
          note: 'Agents read this to know what it’s for.',
          child: TextField(
            controller: _description,
            placeholder: 'Token for the claktory.com zone',
            maxLines: 3,
            minLines: 1,
          ),
        ),
        if (!_named) ..._values(palette, editing),
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
      style: TextStyles.caption.copyWith(
        color: palette.textMuted,
        fontWeight: FontWeight.w500,
      ),
    ),
    if (editing != null)
      Text(
        'Leave the values empty to keep them. To change one, enter them '
        'all.',
        style: TextStyles.caption.copyWith(color: palette.textFaint),
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
                  maxLines: 6,
                  minLines: 3,
                )
              : TextField(
                  controller: variable.value,
                  placeholder: 'Paste the value',
                  obscureText: true,
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
            style: TextStyles.body.copyWith(
              fontFamily: TextStyles.mono,
              color: palette.text,
            ),
          ),
        for (final warning in preview.warnings)
          Text(
            warning,
            style: TextStyles.caption.copyWith(color: palette.danger),
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
            child: TextField(controller: _name, placeholder: 'Cloudflare'),
          ),
          Field(
            label: 'Description',
            note: 'Agents read this to know what it’s for.',
            child: TextField(
              controller: _description,
              placeholder: 'Token for the claktory.com zone',
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
          style: TextStyles.caption.copyWith(color: palette.textMuted),
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
                  'No secrets yet. Add them in Settings → Secrets.',
                  style: TextStyles.caption.copyWith(color: palette.textFaint),
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
                          onChanged: (value) => _set(secret, value),
                        ),
                      ),
                    if (_error != null)
                      Text(
                        _error!,
                        style: TextStyles.caption.copyWith(
                          color: palette.danger,
                        ),
                      ),
                  ],
                ),
        );
      },
    );
  }
}

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
            if (vault.exists && !vault.unlocked) ...[
              Text(
                "Secrets are locked. Agents can't use them until you unlock "
                'them in Settings → Secrets.',
                style: TextStyles.caption.copyWith(color: palette.signal),
              ),
              const SizedBox(height: Space.m),
            ],
            if (given.isEmpty)
              Text(
                '${agent?.name ?? agentId} has no secrets yet.',
                style: TextStyles.body.copyWith(color: palette.textFaint),
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
              style: TextStyles.caption.copyWith(color: palette.textFaint),
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
    ].join(' · ');
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.m),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  secret.name,
                  style: TextStyles.label.copyWith(color: palette.text),
                ),
                if (secret.description.isNotEmpty)
                  Text(
                    secret.description,
                    style: TextStyles.caption.copyWith(
                      color: palette.textMuted,
                    ),
                  ),
                if (secret.variables.isNotEmpty)
                  Text(
                    secret.variables
                        .map((variable) => variable.name)
                        .join(', '),
                    style: TextStyles.code.copyWith(
                      fontSize: 11,
                      color: palette.textMuted,
                    ),
                  ),
                if (when.isNotEmpty)
                  Text(
                    when,
                    style: TextStyles.caption.copyWith(
                      color: palette.textFaint,
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(width: Space.s),
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
                        style: TextStyles.body.copyWith(color: palette.text),
                      ),
                      if (secret.description.isNotEmpty)
                        Text(
                          secret.description,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyles.caption.copyWith(
                            color: palette.textFaint,
                          ),
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

/// An agent's request for a secret, in its conversation and at the top of
/// Settings → Secrets: what it asks for and why, in its color. Pending, the
/// person gives one of their secrets (Choose one) or adds a new one filled
/// in from the request (Add new), or dismisses it with a word why. Decided,
/// what was given and when. The backend sends the entry back decided; the
/// card keeps nothing.
class SecretRequestCard extends StatefulWidget {
  const SecretRequestCard(this.request, {super.key});

  final SecretRequest request;

  @override
  State<SecretRequestCard> createState() => _SecretRequestCardState();
}

class _SecretRequestCardState extends State<SecretRequestCard> {
  late final _name = TextEditingController(text: widget.request.newSecret.name);
  late final _description = TextEditingController(
    text: widget.request.newSecret.description,
  );
  late final List<_VariableEdit> _values = [
    for (final variable in widget.request.newSecret.variables)
      _VariableEdit(name: variable.name, file: variable.file),
  ];
  final _comment = TextEditingController();

  /// Adding a new secret rather than choosing one; set when first built,
  /// from the person's secrets.
  bool? _adding;

  /// The secret chosen to give; empty for none yet.
  var _chosen = '';
  var _deciding = false;
  String? _error;

  @override
  void dispose() {
    _name.dispose();
    _description.dispose();
    for (final value in _values) {
      value.dispose();
    }
    _comment.dispose();
    super.dispose();
  }

  SecretRequest get request => widget.request;

  /// From before agents described what they need: it names a secret.
  bool get _names => request.secretId.isNotEmpty;

  Future<void> _give(List<SecretInfo> secrets) async {
    if (_deciding) return;
    final adding = !_names && _adding == true;
    final chosen = secrets.any((secret) => secret.id == _chosen);
    if (!_names && !adding && !chosen) {
      setState(() => _error = 'Choose a secret to give, or add a new one.');
      return;
    }
    if (adding && _name.text.trim().isEmpty) {
      setState(() => _error = 'Enter a name for the secret.');
      return;
    }
    if (adding && _values.any((v) => v.value.text.isEmpty)) {
      setState(() => _error = 'Enter a value for each variable.');
      return;
    }
    await _decide(
      () => AppScope.of(context).decideSecretRequest(
        request.agentId,
        request.id,
        approve: true,
        secretId: _names || adding ? '' : _chosen,
        name: adding ? _name.text.trim() : '',
        description: adding ? _description.text.trim() : '',
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

  Future<void> _dismiss() => _decide(
    () => AppScope.of(context).decideSecretRequest(
      request.agentId,
      request.id,
      approve: false,
      comment: _comment.text.trim(),
    ),
  );

  Future<void> _decide(Future<void> Function() send) async {
    if (_deciding) return;
    setState(() {
      _deciding = true;
      _error = null;
    });
    try {
      await send();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _deciding = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => _card(
        context,
        store.agent(request.agentId),
        store.workspace.vault.secrets,
      ),
    );
  }

  Widget _card(BuildContext context, Agent? agent, List<SecretInfo> secrets) {
    final palette = Palette.of(context);
    final color = agentColor(agent?.color ?? 'slate');
    final name = agent?.name ?? request.agentId;
    final pending =
        request.state != SecretRequestState.SECRET_REQUEST_STATE_GRANTED &&
        request.state != SecretRequestState.SECRET_REQUEST_STATE_DISMISSED &&
        request.state != SecretRequestState.SECRET_REQUEST_STATE_EXPIRED;
    final description = _names
        ? [
                for (final secret in secrets)
                  if (secret.id == request.secretId) secret.description,
              ].firstOrNull ??
              ''
        : request.newSecret.description;
    if (pending && !_names && _adding == null) {
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
              if (agent != null) ...[
                AgentCharacter(agent: agent, size: 20),
                const SizedBox(width: Space.xs + 2),
              ],
              Flexible(
                child: Text(
                  _names
                      ? '$name asks for ${request.secretName}'
                      : '$name asks for a secret: ${request.newSecret.name}',
                  style: TextStyles.caption.copyWith(
                    color: color,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          if (description.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            Markdown(description, color: palette.textMuted, shortLinks: true),
          ],
          if (request.reason.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            Markdown(
              request.reason,
              color: pending ? null : palette.textMuted,
              shortLinks: true,
            ),
          ],
          const SizedBox(height: Space.m),
          if (pending)
            ..._pending(context, palette, name, secrets)
          else
            ..._decided(palette, color, name),
        ],
      ),
    );
  }

  List<Widget> _pending(
    BuildContext context,
    Palette palette,
    String agent,
    List<SecretInfo> secrets,
  ) {
    final phone = isPhoneLayout(context);
    final adding = !_names && _adding == true;
    final chosen = [
      for (final secret in secrets)
        if (secret.id == _chosen) secret.name,
    ].firstOrNull;
    final String give;
    if (_names) {
      give = phone ? 'Give' : 'Give ${request.secretName} to $agent';
    } else if (adding) {
      give = phone ? 'Add and give' : 'Add and give to $agent';
    } else {
      give = phone || chosen == null ? 'Give' : 'Give $chosen to $agent';
    }
    return [
      if (!_names) ...[
        Row(
          children: [
            _ModeTab(
              title: 'Choose one',
              selected: !adding,
              onTap: () => setState(() {
                _adding = false;
                _error = null;
              }),
            ),
            const SizedBox(width: Space.s),
            _ModeTab(
              title: 'Add new',
              selected: adding,
              onTap: () => setState(() {
                _adding = true;
                _error = null;
              }),
            ),
          ],
        ),
        const SizedBox(height: Space.m),
        if (!adding)
          secrets.isEmpty
              ? Padding(
                  padding: const EdgeInsets.only(bottom: Space.l),
                  child: Text(
                    "You don't have any secrets yet.",
                    style: TextStyles.body.copyWith(color: palette.textMuted),
                  ),
                )
              : Field(
                  label: 'Secret',
                  child: Select<String>(
                    value: _chosen,
                    options: [
                      if (chosen == null)
                        const SelectOption('', 'Choose a secret'),
                      for (final secret in secrets)
                        SelectOption(secret.id, secret.name),
                    ],
                    onChanged: (id) => setState(() {
                      _chosen = id;
                      _error = null;
                    }),
                  ),
                )
        else ...[
          Padding(
            padding: const EdgeInsets.only(bottom: Space.m),
            child: Text(
              "Filled in from $agent's request. Change anything you need.",
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          ),
          Field(
            label: 'Name',
            child: TextField(controller: _name),
          ),
          Field(
            label: 'Description',
            child: TextField(controller: _description, maxLines: 3),
          ),
          for (final value in _values)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.s),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(
                    value.file ? '${value.name.text} (file)' : value.name.text,
                    style: TextStyles.caption.copyWith(
                      fontFamily: TextStyles.mono,
                      color: palette.textMuted,
                    ),
                  ),
                  const SizedBox(height: Space.xs),
                  value.file
                      ? TextField(
                          controller: value.value,
                          placeholder: 'Paste the file’s contents',
                          maxLines: 6,
                          minLines: 3,
                        )
                      : TextField(
                          controller: value.value,
                          placeholder: 'Paste the value',
                          obscureText: true,
                        ),
                ],
              ),
            ),
        ],
      ],
      Row(
        children: [
          Expanded(
            child: Text(
              _error ?? '$agent never sees the value.',
              style: TextStyles.caption.copyWith(
                color: _error == null ? palette.textFaint : palette.danger,
              ),
            ),
          ),
          const SizedBox(width: Space.s),
          Button(
            label: give,
            primary: true,
            onPressed: _deciding || (!_names && !adding && secrets.isEmpty)
                ? null
                : () => _give(secrets),
          ),
        ],
      ),
      const SizedBox(height: Space.m),
      Row(
        children: [
          Expanded(
            child: TextField(
              controller: _comment,
              placeholder: 'Tell $agent why (optional)',
              onSubmitted: (_) => _dismiss(),
            ),
          ),
          const SizedBox(width: Space.s),
          Button(label: 'Dismiss', onPressed: _deciding ? null : _dismiss),
        ],
      ),
    ];
  }

  List<Widget> _decided(Palette palette, Color color, String agent) {
    final dismissed =
        request.state == SecretRequestState.SECRET_REQUEST_STATE_DISMISSED;
    // Left unanswered too long: the agent was told and can ask again.
    final expired =
        request.state == SecretRequestState.SECRET_REQUEST_STATE_EXPIRED;
    final decided = request.decidedMs.toInt();
    final given = request.givenSecretName.isNotEmpty
        ? request.givenSecretName
        : request.secretName;
    return [
      Text.rich(
        TextSpan(
          children: [
            TextSpan(
              text: expired
                  ? 'Expired'
                  : dismissed
                  ? 'Dismissed'
                  : 'Gave $given to $agent',
              style: TextStyle(
                color: dismissed || expired ? palette.textMuted : color,
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
        style: TextStyles.caption,
      ),
      if (request.comment.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(
          '“${request.comment}”',
          style: TextStyles.body.copyWith(color: palette.textMuted),
        ),
      ],
    ];
  }
}

/// One of the request card's two ways to give: Choose one or Add new. The
/// selected one is underlined in signal, as the Marketplace's tabs are.
class _ModeTab extends StatelessWidget {
  const _ModeTab({
    required this.title,
    required this.selected,
    required this.onTap,
  });

  final String title;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      selected: selected,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          height: 28,
          padding: const EdgeInsets.symmetric(horizontal: Space.xs),
          alignment: Alignment.center,
          decoration: BoxDecoration(
            border: Border(
              bottom: BorderSide(
                width: 2,
                color: selected ? palette.signal : const Color(0x00000000),
              ),
            ),
          ),
          child: Text(
            title,
            style: TextStyles.body.copyWith(
              color: selected || hovered ? palette.text : palette.textMuted,
            ),
          ),
        ),
      ),
    );
  }
}
