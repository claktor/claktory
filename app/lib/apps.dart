import 'dart:io' show File, Platform;

import 'package:flutter/widgets.dart';

import 'attachments.dart' show pickAttachments, pickFiles;
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'markdown.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';
import 'usage.dart' show formatAge;

/// The grantee a secret given to an app has in the vault: "app:telegram".
const appGrantee = 'app:';

/// The app a vault grant to [agentId] ("app:telegram") is, if it's one.
AppInfo? appOfGrantee(Workspace workspace, String agentId) {
  if (!agentId.startsWith(appGrantee)) return null;
  final id = agentId.substring(appGrantee.length);
  for (final app in workspace.apps) {
    if (app.id == id) return app;
  }
  return null;
}

/// Asks for a .clapp file and answers its bytes; null when cancelled. Tests
/// set [appFilePicker] instead.
Future<List<int>?> pickAppFile() async {
  final pick = appFilePicker;
  if (pick != null) return pick();
  final paths = Platform.isIOS ? await pickAttachments() : await pickFiles();
  if (paths.isEmpty) return null;
  return File(paths.first).readAsBytes();
}

/// Stands in for the system's file picker in tests.
Future<List<int>?> Function()? appFilePicker;

/// Settings → Apps: the apps installed, each with whether it runs, its
/// setup, the agents it's given to and the events they hear; and
/// installing one from a file.
class AppsSettings extends StatelessWidget {
  const AppsSettings({super.key});

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final palette = Palette.of(context);
        final apps = backend.workspace.workspace.apps;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (apps.isEmpty)
              Text(
                'No apps yet.',
                style: TextStyles.body.copyWith(color: palette.textFaint),
              )
            else
              for (final app in apps) AppRow(key: ValueKey(app.id), app: app),
            const SizedBox(height: Space.s),
            Align(
              alignment: Alignment.centerLeft,
              child: Button(
                label: 'Install from a file…',
                onPressed: () => installAppFromFile(context),
              ),
            ),
            const SizedBox(height: Space.l),
            Text(
              'Each app runs walled off from everything else: it reaches '
              'only the sites you approve and keeps its secrets to itself. '
              'An agent can use an app only once you give it one.',
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          ],
        );
      },
    );
  }
}

/// Asks for a .clapp file, then shows what the app can reach, to install.
Future<void> installAppFromFile(BuildContext context) async {
  final content = await pickAppFile();
  if (content == null || !context.mounted) return;
  await showModal(
    context,
    (context, close) => AppInstallForm(content: content, close: close),
  );
}

/// What an app the person picked can reach, from the backend's review of
/// the file, and Install. Installing repeats the review's checksum, so what
/// installs is the file reviewed.
class AppInstallForm extends StatefulWidget {
  const AppInstallForm({super.key, required this.content, required this.close});

  final List<int> content;
  final VoidCallback close;

  @override
  State<AppInstallForm> createState() => _AppInstallFormState();
}

class _AppInstallFormState extends State<AppInstallForm> {
  AppPermissions? _permissions;
  String? _error;
  var _busy = false;

  @override
  void initState() {
    super.initState();
    _review();
  }

  Future<void> _review() async {
    try {
      final permissions = await AppScope.of(context)
          .installLocalApp(widget.content);
      if (mounted) setState(() => _permissions = permissions);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    }
  }

  Future<void> _install() async {
    final permissions = _permissions;
    if (permissions == null) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).installLocalApp(
        widget.content,
        approve: true,
        sha256: permissions.sha256,
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
    final palette = Palette.of(context);
    final permissions = _permissions;
    if (permissions == null) {
      return FormFrame(
        title: 'Install an app',
        fields: [
          if (_error == null)
            Text(
              'Reading the file…',
              style: TextStyles.body.copyWith(color: palette.textMuted),
            ),
          const SizedBox(height: Space.l),
        ],
        error: _error,
        submitLabel: 'Install',
        onSubmit: null,
        onCancel: widget.close,
      );
    }
    final update = permissions.installedVersion.isNotEmpty;
    final caption = TextStyles.caption.copyWith(color: palette.textFaint);
    return FormFrame(
      title: update
          ? 'Update ${permissions.name} to ${permissions.version}?'
          : 'Install ${permissions.name}?',
      body: permissions.description.isEmpty ? null : permissions.description,
      fields: [
        Text(
          [
            if (!update) 'Version ${permissions.version}',
            if (permissions.publisher.isNotEmpty) 'By ${permissions.publisher}',
            tierLine(permissions.tier, permissions.publisherKey),
          ].join(' · '),
          style: TextStyles.caption.copyWith(color: palette.textMuted),
        ),
        if (_unreviewed(permissions.tier)) ...[
          const SizedBox(height: Space.s),
          _Warning(
            "Claktory hasn't reviewed this app. Install it only if you "
            'trust who made it.',
          ),
        ],
        const SizedBox(height: Space.l),
        if (update && permissions.added.isNotEmpty)
          Field(
            label: 'New since version ${permissions.installedVersion}',
            child: _Lines(permissions.added),
          ),
        Field(
          label: 'It can reach',
          child: permissions.hosts.isEmpty
              ? Text(
                  'No sites on the internet.',
                  style: TextStyles.body.copyWith(color: palette.textMuted),
                )
              : _Lines(permissions.hosts, mono: true),
        ),
        if (permissions.scopes.isNotEmpty)
          Field(
            label: 'Agents you give it to can',
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (final scope in permissions.scopes)
                  _ScopeLine(scope: scope),
              ],
            ),
          ),
        if (permissions.events.isNotEmpty)
          Field(
            label: 'Agents you choose hear',
            child: _Lines([
              for (final event in permissions.events) event.label,
            ]),
          ),
        if (permissions.secrets.isNotEmpty)
          Field(
            label: 'Setting it up asks for',
            child: _Lines([
              for (final secret in permissions.secrets) secret.label,
            ]),
          ),
        Text(
          [
            if (permissions.memoryMb > 0)
              'Up to ${permissions.memoryMb} MB of memory',
            if (permissions.storageMb > 0)
              '${permissions.memoryMb > 0 ? 'and' : 'Up to'} '
                  '${permissions.storageMb} MB of storage',
          ].join(' '),
          style: caption,
        ),
        Text('SHA-256 ${permissions.sha256}', style: caption),
        const SizedBox(height: Space.l),
      ],
      error: _error,
      submitLabel: _busy
          ? (update ? 'Updating…' : 'Installing…')
          : (update ? 'Update' : 'Install'),
      onSubmit: _busy ? null : _install,
      onCancel: widget.close,
    );
  }
}

bool _unreviewed(AppTier tier) =>
    tier == AppTier.APP_TIER_COMMUNITY || tier == AppTier.APP_TIER_LOCAL;

/// Who vouches for an app: "Made by Claktory", "From a file, signed by its
/// developer (ed25519:…)".
String tierLine(AppTier tier, String key) => switch (tier) {
  AppTier.APP_TIER_CLAKTORY => 'Made by Claktory',
  AppTier.APP_TIER_VERIFIED => 'Verified publisher',
  AppTier.APP_TIER_COMMUNITY => 'Community app, only checked automatically',
  AppTier.APP_TIER_LOCAL =>
    key.isEmpty
        ? 'From a file, signed by its developer'
        : 'From a file, signed by its developer ($key)',
  _ => 'Unknown publisher',
};

/// How an app is doing, in a word or two.
String appStateLine(AppState state) => switch (state) {
  AppState.APP_STATE_RUNNING => 'Running',
  AppState.APP_STATE_STARTING => 'Starting…',
  AppState.APP_STATE_STOPPED => 'Off',
  AppState.APP_STATE_NEEDS_SETUP => 'Needs setup',
  AppState.APP_STATE_PAUSED_CRASHED => 'Paused because it kept quitting',
  AppState.APP_STATE_RESTARTING => 'Starting again…',
  AppState.APP_STATE_UNAVAILABLE => "Can't run on this computer",
  _ => 'Unknown',
};

/// Whether the person has it on: it runs, or will once it can.
bool appOn(AppState state) => switch (state) {
  AppState.APP_STATE_RUNNING ||
  AppState.APP_STATE_STARTING ||
  AppState.APP_STATE_NEEDS_SETUP ||
  AppState.APP_STATE_RESTARTING => true,
  _ => false,
};

/// One installed app: its icon, name and state with the switch that runs
/// it, its own line and what its setup still needs. Opened, what it's for,
/// its setup, the agents it's given to, what it reaches, and Uninstall.
class AppRow extends StatefulWidget {
  const AppRow({super.key, required this.app});

  final AppInfo app;

  @override
  State<AppRow> createState() => _AppRowState();
}

class _AppRowState extends State<AppRow> {
  var _open = false;
  String? _error;

  AppInfo get app => widget.app;

  Future<void> _run(Future<void> Function() request) async {
    setState(() => _error = null);
    try {
      await request();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final troubled =
        app.state == AppState.APP_STATE_PAUSED_CRASHED ||
        app.state == AppState.APP_STATE_UNAVAILABLE;
    final missing = [
      for (final item in app.setup)
        if (!item.done) item,
    ];
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.xs),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Unselectable(
                  child: Semantics(
                    button: true,
                    label: app.name,
                    child: Pressable(
                      onTap: () => setState(() => _open = !_open),
                      builder: (context, hovered) => AnimatedContainer(
                        duration: Motion.fast,
                        padding: const EdgeInsets.all(Space.s),
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
                            Padding(
                              padding: const EdgeInsets.only(
                                top: 2,
                                right: Space.s,
                              ),
                              child: ExtensionGlyph(
                                icon: app.icon,
                                name: app.name,
                                color: palette.text,
                              ),
                            ),
                            Expanded(
                              child: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  Text(
                                    app.name,
                                    style: TextStyles.label.copyWith(
                                      color: palette.text,
                                    ),
                                  ),
                                  Text(
                                    [
                                      appStateLine(app.state),
                                      _grantsLine(backend, app),
                                    ].join(' · '),
                                    style: TextStyles.caption.copyWith(
                                      color: troubled
                                          ? palette.danger
                                          : palette.textFaint,
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
              ),
              Padding(
                padding: const EdgeInsets.only(top: Space.s),
                child: Toggle(
                  label: 'Run ${app.name}',
                  value: appOn(app.state),
                  onChanged: app.state == AppState.APP_STATE_UNAVAILABLE
                      ? null
                      : (on) => _run(() => backend.setAppRunning(app.id, on)),
                ),
              ),
            ],
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(
              Space.s + Space.m + 2,
              0,
              Space.s,
              0,
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (app.stateDetail.isNotEmpty)
                  Text(
                    app.stateDetail,
                    style: TextStyles.caption.copyWith(
                      color: troubled ? palette.danger : palette.textMuted,
                    ),
                  ),
                if (app.status.isNotEmpty) ...[
                  const SizedBox(height: Space.xs),
                  Markdown(app.status, shortLinks: true),
                ],
                // What its setup still needs shows closed too: it waits on
                // the person.
                if (!_open)
                  for (final item in missing) _SetupLine(app: app, item: item),
                if (_open) _details(context, palette, backend),
                if (_error != null)
                  Text(
                    _error!,
                    style: TextStyles.caption.copyWith(color: palette.danger),
                  ),
                const SizedBox(height: Space.s),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _details(BuildContext context, Palette palette, Backend backend) {
    final workspace = backend.workspace.workspace;
    final caption = TextStyles.caption.copyWith(color: palette.textFaint);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (app.description.isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          Markdown(app.description, color: palette.textMuted, shortLinks: true),
        ],
        if (app.setup.isNotEmpty) ...[
          const SizedBox(height: Space.m),
          Text('Setup', style: _heading(palette)),
          for (final item in app.setup) _SetupLine(app: app, item: item),
        ],
        const SizedBox(height: Space.m),
        Text('Agents', style: _heading(palette)),
        const SizedBox(height: Space.xs),
        if (app.grants.isEmpty)
          Text(
            'No agent can use it yet.',
            style: TextStyles.body.copyWith(color: palette.textMuted),
          )
        else
          for (final grant in app.grants)
            _GrantLine(app: app, grant: grant, workspace: workspace),
        if (app.scopes.isNotEmpty && _ungranted(workspace, app).isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          Align(
            alignment: Alignment.centerLeft,
            child: Button(
              label: 'Give to an agent…',
              onPressed: () => showModal(
                context,
                (context, close) => AppAccessForm(app: app, close: close),
              ),
            ),
          ),
        ],
        if (app.hosts.isNotEmpty) ...[
          const SizedBox(height: Space.m),
          Text('It can reach', style: _heading(palette)),
          _Lines(app.hosts, mono: true),
        ],
        const SizedBox(height: Space.m),
        Text(
          [
            'Version ${app.version}',
            if (app.publisher.isNotEmpty) 'By ${app.publisher}',
            tierLine(app.tier, app.publisherKey),
            if (app.installedMs > 0)
              'Installed ${formatAge(app.installedMs.toInt(), DateTime.now())}',
          ].join(' · '),
          style: caption,
        ),
        const SizedBox(height: Space.s),
        Align(
          alignment: Alignment.centerLeft,
          child: Button(
            label: 'Uninstall…',
            onPressed: () => showModal(
              context,
              (context, close) => UninstallAppForm(app: app, close: close),
            ),
          ),
        ),
      ],
    );
  }
}

TextStyle _heading(Palette palette) => TextStyles.caption.copyWith(
  color: palette.textFaint,
  fontWeight: FontWeight.w500,
);

/// "Given to June and Mike", "Given to no agent".
String _grantsLine(Backend backend, AppInfo app) {
  if (app.grants.isEmpty) return 'Given to no agent';
  final names = [
    for (final grant in app.grants)
      backend.workspace.agent(grant.agentId)?.name ?? grant.agentId,
  ];
  final listed = names.length == 1
      ? names.single
      : '${names.take(names.length - 1).join(', ')} and ${names.last}';
  return 'Given to $listed';
}

/// The agents [app] isn't given to yet.
List<Agent> _ungranted(Workspace workspace, AppInfo app) {
  final given = {for (final grant in app.grants) grant.agentId};
  return [
    for (final agent in workspace.agents)
      if (!given.contains(agent.id)) agent,
  ];
}

/// The event kinds [agentId] hears from [app].
Set<String> _heard(AppInfo app, String agentId) => {
  for (final subscription in app.subscriptions)
    if (subscription.agentIds.contains(agentId)) subscription.eventKind,
};

/// One thing an app's setup asks for: its label, how to get it, whether
/// it's set, and Set… or Replace….
class _SetupLine extends StatelessWidget {
  const _SetupLine({required this.app, required this.item});

  final AppInfo app;
  final AppSetupItem item;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: Space.s),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  item.label,
                  style: TextStyles.body.copyWith(color: palette.text),
                ),
                Text(
                  item.done ? 'Saved' : 'Not set yet',
                  style: TextStyles.caption.copyWith(
                    color: item.done ? palette.textFaint : palette.signal,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: Space.s),
          if (item.kind == AppSetupKind.APP_SETUP_KIND_SECRET)
            Button(
              label: item.done ? 'Replace…' : 'Set…',
              primary: !item.done,
              onPressed: () => showModal(
                context,
                (context, close) =>
                    AppSecretForm(app: app, item: item, close: close),
              ),
            )
          else
            Text(
              'Needs a newer Claktory',
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
        ],
      ),
    );
  }
}

/// One agent an app is given to: its character and name, what it may do,
/// the events it hears, and Change….
class _GrantLine extends StatelessWidget {
  const _GrantLine({
    required this.app,
    required this.grant,
    required this.workspace,
  });

  final AppInfo app;
  final AppGrant grant;
  final Workspace workspace;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    Agent? agent;
    for (final candidate in workspace.agents) {
      if (candidate.id == grant.agentId) agent = candidate;
    }
    final scopes = [
      for (final scope in app.scopes)
        if (grant.scopes.contains(scope.id)) scope.label,
    ];
    final heard = _heard(app, grant.agentId);
    final events = [
      for (final event in app.events)
        if (heard.contains(event.kind)) event.label,
    ];
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.s),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (agent != null) ...[
            AgentCharacter(agent: agent, size: 20),
            const SizedBox(width: Space.s),
          ],
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  agent?.name ?? grant.agentId,
                  style: TextStyles.body.copyWith(color: palette.text),
                ),
                if (scopes.isNotEmpty)
                  Text(
                    scopes.join(' · '),
                    style: TextStyles.caption.copyWith(
                      color: palette.textMuted,
                    ),
                  ),
                Text(
                  events.isEmpty
                      ? 'Hears none of its events'
                      : 'Hears: ${labelInSentence(events.join(' · '))}',
                  style: TextStyles.caption.copyWith(color: palette.textFaint),
                ),
              ],
            ),
          ),
          if (agent != null)
            Button(
              label: 'Change…',
              onPressed: () => showModal(
                context,
                (context, close) =>
                    AppAccessForm(app: app, agent: agent, close: close),
              ),
            ),
        ],
      ),
    );
  }
}

/// What one agent may do with an app (its scopes) and which of its events
/// it hears; with no [agent], an agent to give it to is picked first.
/// Saving gives the scopes (none takes the app away) and sets the events.
class AppAccessForm extends StatefulWidget {
  const AppAccessForm({
    super.key,
    required this.app,
    required this.close,
    this.agent,
  });

  final AppInfo app;
  final Agent? agent;
  final VoidCallback close;

  @override
  State<AppAccessForm> createState() => _AppAccessFormState();
}

class _AppAccessFormState extends State<AppAccessForm> {
  late String? _agentId = widget.agent?.id;
  late Set<String> _scopes = _grantedScopes(_agentId);
  late Set<String> _events = _agentId == null
      ? {}
      : _heard(widget.app, _agentId!);
  String? _error;
  var _busy = false;

  Set<String> _grantedScopes(String? agentId) {
    for (final grant in widget.app.grants) {
      if (grant.agentId == agentId) return grant.scopes.toSet();
    }
    // A new grant starts with all of it: the person takes away what it
    // shouldn't have.
    return {for (final scope in widget.app.scopes) scope.id};
  }

  bool get _granted =>
      widget.app.grants.any((grant) => grant.agentId == _agentId);

  Future<void> _save({bool takeAway = false}) async {
    final agentId = _agentId;
    if (agentId == null) return;
    final app = widget.app;
    final backend = AppScope.of(context);
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final scopes = takeAway
          ? <String>[]
          : [
              for (final scope in app.scopes)
                if (_scopes.contains(scope.id)) scope.id,
            ];
      await backend.grantApp(app.id, agentId, scopes);
      // Taking the app away drops what it hears too.
      if (scopes.isNotEmpty) {
        final had = _heard(app, agentId);
        for (final event in app.events) {
          final hears =
              _events.contains(event.kind) && _scopes.contains(event.scope);
          if (hears == had.contains(event.kind)) continue;
          final now = [
            for (final subscription in app.subscriptions)
              if (subscription.eventKind == event.kind)
                for (final id in subscription.agentIds)
                  if (id != agentId) id,
            if (hears) agentId,
          ];
          await backend.subscribeApp(app.id, event.kind, now);
        }
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
    final backend = AppScope.of(context);
    final app = widget.app;
    final agent = widget.agent;
    final choices = agent == null
        ? _ungranted(backend.workspace.workspace, app)
        : const <Agent>[];
    _agentId ??= choices.isEmpty ? null : choices.first.id;
    final name =
        agent?.name ??
        [
          for (final choice in choices)
            if (choice.id == _agentId) choice.name,
        ].firstOrNull ??
        '';
    return FormFrame(
      title: agent == null
          ? 'Give ${app.name} to an agent'
          : '${app.name} for ${agent.name}',
      fields: [
        if (agent == null)
          Field(
            label: 'Agent',
            child: choices.isEmpty
                ? Text(
                    'Every agent has it already.',
                    style: TextStyles.body.copyWith(color: palette.textMuted),
                  )
                : Select<String>(
                    value: _agentId ?? '',
                    options: [
                      for (final choice in choices)
                        SelectOption(choice.id, choice.name),
                    ],
                    onChanged: (id) => setState(() => _agentId = id),
                  ),
          ),
        Field(
          label: name.isEmpty ? 'It can' : '$name can',
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final scope in app.scopes)
                Padding(
                  padding: const EdgeInsets.only(bottom: Space.s),
                  child: _SwitchLine(
                    label: scope.label,
                    note: scope.tools.join(', '),
                    mono: true,
                    value: _scopes.contains(scope.id),
                    onChanged: (on) => setState(() {
                      _scopes = {..._scopes};
                      on ? _scopes.add(scope.id) : _scopes.remove(scope.id);
                    }),
                  ),
                ),
            ],
          ),
        ),
        if (app.events.isNotEmpty)
          Field(
            label: name.isEmpty ? 'It hears' : '$name hears',
            note: 'Each one wakes it, like a message.',
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (final event in app.events)
                  Padding(
                    padding: const EdgeInsets.only(bottom: Space.s),
                    child: _SwitchLine(
                      label: event.label,
                      note: _scopes.contains(event.scope)
                          ? null
                          : 'Turn on “${_scopeLabel(app, event.scope)}” first.',
                      value:
                          _events.contains(event.kind) &&
                          _scopes.contains(event.scope),
                      onChanged: _scopes.contains(event.scope)
                          ? (on) => setState(() {
                              _events = {..._events};
                              on
                                  ? _events.add(event.kind)
                                  : _events.remove(event.kind);
                            })
                          : null,
                    ),
                  ),
              ],
            ),
          ),
      ],
      error: _error,
      submitLabel: _busy ? 'Saving…' : (_granted ? 'Save' : 'Give'),
      onSubmit: _busy || _agentId == null || (!_granted && _scopes.isEmpty)
          ? null
          : () => _save(),
      onCancel: widget.close,
      secondary: _granted
          ? Button(
              label: 'Take back',
              onPressed: _busy ? null : () => _save(takeAway: true),
            )
          : null,
    );
  }
}

/// A label inside a sentence: "Bot token" → "bot token"; one that starts
/// with an acronym ("API key") stays as it is.
String labelInSentence(String label) {
  final first = label.split(' ').first;
  if (label.isEmpty || (first.length > 1 && first == first.toUpperCase())) {
    return label;
  }
  return label[0].toLowerCase() + label.substring(1);
}

String _scopeLabel(AppInfo app, String id) {
  for (final scope in app.scopes) {
    if (scope.id == id) return scope.label;
  }
  return id;
}

/// A switch with its label and, under it, a note.
class _SwitchLine extends StatelessWidget {
  const _SwitchLine({
    required this.label,
    required this.value,
    required this.onChanged,
    this.note,
    this.mono = false,
  });

  final String label;
  final String? note;
  final bool mono;
  final bool value;
  final ValueChanged<bool>? onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final note = this.note;
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
              Text(
                label,
                style: TextStyles.body.copyWith(
                  color: onChanged == null ? palette.textMuted : palette.text,
                ),
              ),
              if (note != null && note.isNotEmpty)
                Text(
                  note,
                  style: TextStyles.caption.copyWith(
                    color: palette.textFaint,
                    fontFamily: mono ? TextStyles.mono : null,
                  ),
                ),
            ],
          ),
        ),
      ],
    );
  }
}

/// One secret an app's setup asks for, pasted: kept in the vault, given to
/// the app alone.
class AppSecretForm extends StatefulWidget {
  const AppSecretForm({
    super.key,
    required this.app,
    required this.item,
    required this.close,
  });

  final AppInfo app;
  final AppSetupItem item;
  final VoidCallback close;

  @override
  State<AppSecretForm> createState() => _AppSecretFormState();
}

class _AppSecretFormState extends State<AppSecretForm> {
  final _value = TextEditingController();
  String? _error;
  var _busy = false;

  @override
  void dispose() {
    _value.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final value = _value.text.trim();
    if (value.isEmpty) {
      setState(
        () => _error = 'Paste the ${labelInSentence(widget.item.label)} first.',
      );
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context)
          .setAppSecret(widget.app.id, widget.item.name, value);
      _value.clear();
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
    final item = widget.item;
    return FormFrame(
      title: '${widget.app.name}: ${item.label}',
      fields: [
        if (item.description.isNotEmpty) ...[
          Markdown(
            item.description,
            color: palette.textMuted,
            shortLinks: true,
          ),
          const SizedBox(height: Space.m),
        ],
        Field(
          label: item.label,
          note:
              'Kept in your secrets for ${widget.app.name} alone. No agent '
              'sees it.',
          child: TextField(
            controller: _value,
            obscureText: true,
            autofocus: true,
            onSubmitted: (_) => _save(),
          ),
        ),
      ],
      error: _error,
      submitLabel: _busy ? 'Saving…' : 'Save',
      onSubmit: _busy ? null : _save,
      onCancel: widget.close,
    );
  }
}

/// "Uninstall Telegram?", keeping its data or not.
class UninstallAppForm extends StatefulWidget {
  const UninstallAppForm({super.key, required this.app, required this.close});

  final AppInfo app;
  final VoidCallback close;

  @override
  State<UninstallAppForm> createState() => _UninstallAppFormState();
}

class _UninstallAppFormState extends State<UninstallAppForm> {
  var _keep = false;
  String? _error;
  var _busy = false;

  Future<void> _uninstall() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).uninstallApp(widget.app.id, keepData: _keep);
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
    title: 'Uninstall ${widget.app.name}?',
    body:
        'It stops, and agents lose its tools and events. The secrets '
        'you gave it stay in Settings → Secrets.',
    fields: [
      _SwitchLine(
        label: 'Keep its data',
        note: 'To pick up where it left off if you install it again.',
        value: _keep,
        onChanged: (keep) => setState(() => _keep = keep),
      ),
      const SizedBox(height: Space.l),
    ],
    error: _error,
    submitLabel: _busy ? 'Uninstalling…' : 'Uninstall',
    onSubmit: _busy ? null : _uninstall,
    onCancel: widget.close,
  );
}

/// One scope in the install review: what it lets an agent do, and its
/// tools.
class _ScopeLine extends StatelessWidget {
  const _ScopeLine({required this.scope});

  final AppScopeSpec scope;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.xs),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            scope.label,
            style: TextStyles.body.copyWith(color: palette.text),
          ),
          if (scope.tools.isNotEmpty)
            Text(
              scope.tools.join(', '),
              style: TextStyles.caption.copyWith(
                color: palette.textFaint,
                fontFamily: TextStyles.mono,
              ),
            ),
        ],
      ),
    );
  }
}

/// Lines of plain text, one under another.
class _Lines extends StatelessWidget {
  const _Lines(this.lines, {this.mono = false});

  final List<String> lines;
  final bool mono;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final line in lines)
          Text(
            line,
            style: TextStyles.body.copyWith(
              color: palette.text,
              fontFamily: mono ? TextStyles.mono : null,
            ),
          ),
      ],
    );
  }
}

/// A caution in the signal color, as Settings → Secrets says it's locked.
class _Warning extends StatelessWidget {
  const _Warning(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        color: palette.signal.withValues(alpha: 0.06),
        border: Border.all(color: palette.signal.withValues(alpha: 0.4)),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Text(text, style: TextStyles.body.copyWith(color: palette.text)),
    );
  }
}
