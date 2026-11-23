import 'dart:async';
import 'dart:io' show File;

import 'package:flutter/widgets.dart';

import 'app_dock.dart' show openApp;
import 'attachments.dart' show pickAttachments, pickFiles;
import 'backend.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'mobile/metrics.dart';
import 'mobile/platform_policy.dart';
import 'markdown.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'resources.dart' show formatBytes;
import 'scope.dart';
import 'selection.dart';
import 'usage.dart' show formatAge;
import 'visible_codes.dart' show proseCodes;

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
  // A touch device's own picker (iPhone, iPad, Android), as the message
  // box chooses; a Mac's open panel.
  final paths = PhonePlatformPolicy.current.touch
      ? await pickAttachments()
      : await pickFiles();
  if (paths.isEmpty) return null;
  return File(paths.first).readAsBytes();
}

/// Stands in for the system's file picker in tests.
Future<List<int>?> Function()? appFilePicker;

/// The Marketplace's Apps tab: the apps installed, Claktory's own first,
/// each with whether it runs, its setup, the agents it's given to and the
/// events they hear; Claktory's own not installed yet; and installing one
/// from a file.
class AppsList extends StatelessWidget {
  const AppsList({super.key});

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final palette = Palette.of(context);
        // By name, Claktory's own first.
        final apps = [
          ...backend.workspace.workspace.apps,
        ]..sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
        bool own(AppInfo app) =>
            app.bundled || app.tier == AppTier.APP_TIER_CLAKTORY;
        final bundled = backend.workspace.workspace.bundledApps;
        // A newer version of an installed app updates it from its own row.
        final updates = {
          for (final offer in bundled)
            if (offer.installedVersion.isNotEmpty &&
                apps.any((app) => app.id == offer.id))
              offer.id: offer,
        };
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (apps.isEmpty && bundled.isEmpty)
              Text(
                'No apps yet.',
                style: TypeScale.of(context).body
                    .copyWith(color: palette.textFaint),
              )
            else ...[
              for (final app in apps)
                if (own(app))
                  AppRow(
                    key: ValueKey(app.id),
                    app: app,
                    update: updates[app.id],
                  ),
              // Claktory's own apps this release ships, not installed yet
              // (or newer than one whose row is gone).
              for (final offer in bundled)
                if (!updates.containsKey(offer.id))
                  BundledAppRow(
                    key: ValueKey('bundled:${offer.id}'),
                    app: offer,
                  ),
              for (final app in apps)
                if (!own(app))
                  AppRow(
                    key: ValueKey(app.id),
                    app: app,
                    update: updates[app.id],
                  ),
            ],
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
              'only the sites you approve, keeps its secrets to itself, and '
              "reads only what it's allowed to.",
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textFaint),
            ),
          ],
        );
      },
    );
  }
}

/// One of Claktory's own apps this release ships: its icon, name and what
/// it's for, "Made by Claktory", and Install (Update to v0.2.1 when an
/// older version is installed), which reviews it as a file is reviewed.
class BundledAppRow extends StatelessWidget {
  const BundledAppRow({super.key, required this.app});

  final BundledApp app;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final update = app.installedVersion.isNotEmpty;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: Space.s),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 2),
            child: ExtensionGlyph(
              icon: app.icon,
              name: app.name,
              color: palette.textMuted,
              size: 16,
            ),
          ),
          const SizedBox(width: Space.s),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  app.name,
                  style: type.body.copyWith(
                    color: palette.text,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                if (app.description.isNotEmpty)
                  Text(
                    app.description,
                    style: type.caption.copyWith(color: palette.textMuted),
                  ),
                Text(
                  update
                      ? 'Made by Claktory · Version ${app.installedVersion} '
                            'installed'
                      : 'Made by Claktory · Version ${app.version}',
                  style: type.caption.copyWith(color: palette.textFaint),
                ),
              ],
            ),
          ),
          const SizedBox(width: Space.s),
          Button(
            label: update ? 'Update to v${app.version}' : 'Install',
            primary: !update,
            onPressed: () => showModal(
              context,
              (context, close) => AppInstallForm(bundled: app.id, close: close),
            ),
          ),
        ],
      ),
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
  const AppInstallForm({
    super.key,
    this.content = const [],
    this.bundled = '',
    required this.close,
  });

  /// The .clapp picked; empty for one of Claktory's own apps.
  final List<int> content;

  /// The id of one of Claktory's own apps this release ships
  /// (Workspace.bundled_apps), checked as any file is.
  final String bundled;
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
          .installLocalApp(widget.content, bundled: widget.bundled);
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
        bundled: widget.bundled,
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
              widget.bundled.isEmpty ? 'Reading the file…' : 'Checking it…',
              style: TypeScale.of(context).body
                  .copyWith(color: palette.textMuted),
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
    final caption = TypeScale.of(context).caption
        .copyWith(color: palette.textFaint);
    return FormFrame(
      title: update
          ? 'Update ${permissions.name} to v${permissions.version}?'
          : 'Install ${permissions.name}?',
      body: permissions.description.isEmpty ? null : permissions.description,
      fields: [
        Text(
          [
            if (!update) 'v${permissions.version}',
            // Claktory's own says so once, in the tier's line.
            if (permissions.publisher.isNotEmpty &&
                permissions.tier != AppTier.APP_TIER_CLAKTORY)
              publisherLine(permissions.tier, permissions.publisher),
            tierLine(permissions.tier, permissions.publisherKey),
          ].join(' · '),
          style: TypeScale.of(context).caption
              .copyWith(color: palette.textMuted),
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
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.textMuted),
                )
              : _Lines(permissions.hosts, mono: true),
        ),
        if (permissions.sources.isNotEmpty)
          Field(
            label: 'It asks to read',
            note:
                "It reads none of this until it's set up. Ask Rivet to set "
                'it up: it asks you before the app reads anything private.',
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (final source in permissions.sources)
                  _SourceLine(
                    source: source,
                    reach: sourceReach(
                      permissions.name,
                      source,
                      AppScope.of(context).workspace.workspace,
                    ),
                  ),
                if (permissions.hosts.isNotEmpty) ...[
                  const SizedBox(height: Space.xs),
                  _Warning(sendsTo(permissions.name, permissions.hosts)),
                ],
              ],
            ),
          ),
        if (permissions.scopes.isNotEmpty)
          Field(
            label: 'Agents using it can',
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

/// Who made an app, by its own say: only Claktory and verified publishers
/// are checked, so for any other app the name is only what it claims (an
/// app can call its publisher "claktory").
String publisherLine(AppTier tier, String publisher) => switch (tier) {
  AppTier.APP_TIER_CLAKTORY || AppTier.APP_TIER_VERIFIED => 'By $publisher',
  _ => "Says it's by $publisher",
};

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
  const AppRow({super.key, required this.app, this.update});

  final AppInfo app;

  /// A newer version of it that Claktory ships, to update to; null for
  /// none.
  final BundledApp? update;

  @override
  State<AppRow> createState() => _AppRowState();
}

class _AppRowState extends State<AppRow> {
  var _open = false;
  String? _error;

  /// What the last Undo left as it was, because it changed since.
  String? _note;

  late final _wanted = AppScope.of(context).marketplaceApp;

  AppInfo get app => widget.app;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _wanted.addListener(_reveal);
      _reveal();
    });
  }

  @override
  void dispose() {
    _wanted.removeListener(_reveal);
    super.dispose();
  }

  /// Asked for (History's own app change): opened and brought into view.
  void _reveal() {
    if (!mounted || _wanted.value != app.id) return;
    _wanted.value = null;
    setState(() => _open = true);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) Scrollable.ensureVisible(context, duration: Motion.fast);
    });
  }

  Future<void> _run(Future<void> Function() request) async {
    setState(() {
      _error = null;
      _note = null;
    });
    try {
      await request();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    }
  }

  /// Undo beside who gave something: back as it was before that change.
  void _undo(String changeId) => _run(() async {
    final note = await undoAppAccess(AppScope.of(context), changeId);
    if (mounted) setState(() => _note = note);
  });

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
                                    style: TypeScale.of(context).label
                                        .copyWith(color: palette.text),
                                  ),
                                  Text(
                                    [
                                      appStateLine(app.state),
                                      _grantsLine(backend, app),
                                    ].join(' · '),
                                    style: TypeScale.of(context).caption
                                        .copyWith(
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
                    style: TypeScale.of(context).caption.copyWith(
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
                if (app.views.isNotEmpty || widget.update != null)
                  Padding(
                    padding: const EdgeInsets.only(top: Space.xs),
                    // Open, for an app with views; which version it's on,
                    // beside the one to update to.
                    child: Wrap(
                      spacing: Space.s,
                      runSpacing: Space.xs,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        if (app.views.isNotEmpty)
                          Button(
                            label: 'Open',
                            onPressed: () => openApp(context, app),
                          ),
                        if (widget.update case final update?) ...[
                          Button(
                            label: 'Update to v${update.version}',
                            primary: true,
                            onPressed: () => showModal(
                              context,
                              (context, close) => AppInstallForm(
                                bundled: update.id,
                                close: close,
                              ),
                            ),
                          ),
                          Text(
                            'v${update.installedVersion} installed',
                            style: TypeScale.of(context).caption
                                .copyWith(color: palette.textFaint),
                          ),
                        ],
                      ],
                    ),
                  ),
                if (_open) _details(context, palette, backend),
                if (_note case final note?)
                  Text(
                    note,
                    style: TypeScale.of(context).caption
                        .copyWith(color: palette.textMuted),
                  ),
                if (_error != null)
                  Text(
                    _error!,
                    style: TypeScale.of(context).caption
                        .copyWith(color: palette.danger),
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
    final caption = TypeScale.of(context).caption
        .copyWith(color: palette.textFaint);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (app.description.isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          Markdown(app.description, color: palette.textMuted, shortLinks: true),
        ],
        if (app.setup.isNotEmpty) ...[
          const SizedBox(height: Space.m),
          Text('Setup', style: _heading(context, palette)),
          for (final item in app.setup) _SetupLine(app: app, item: item),
        ],
        // Nothing given yet, as Claktory's own apps start.
        if (app.grants.isEmpty &&
            (app.sources.isNotEmpty || app.readsFiles) &&
            !app.sources.any((source) => source.granted)) ...[
          const SizedBox(height: Space.s),
          Text(
            "${app.name} isn't set up yet. Tell Rivet what you'd like from "
            'it, and it sets ${app.name} up for you.',
            style: TypeScale.of(context).body
                .copyWith(color: palette.textMuted),
          ),
        ],
        if (app.sources.isNotEmpty || app.readsFiles) ...[
          const SizedBox(height: Space.m),
          Text('It can read', style: _heading(context, palette)),
          const SizedBox(height: Space.xs),
          if (!app.sources.any((source) => source.granted))
            Padding(
              padding: const EdgeInsets.only(bottom: Space.s),
              child: Text(
                "${app.name} can read only what's turned on here.",
                style: caption,
              ),
            ),
          // Each source on its own, none on until the person turns it on.
          for (final source in app.sources)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.s),
              child: _SwitchLine(
                label: source.label,
                private: source.private,
                about: sourceReach(app.name, source, workspace),
                note: source.reason,
                value: source.granted,
                onChanged: (on) => _run(
                  () => backend.grantAppSource(app.id, source.id, granted: on),
                ),
                given: source.granted && source.hasGiven()
                    ? AppAccessGivenLine(
                        given: source.given,
                        source: true,
                        onUndo: _undo,
                      )
                    : null,
              ),
            ),
          // Where what it reads can go.
          // A CSV or TSV file the person picks, for an app that reads files.
          if (app.readsFiles)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.s),
              child: Align(
                alignment: Alignment.centerLeft,
                child: Button(
                  label: 'Add a file…',
                  onPressed: () => showModal(
                    context,
                    (context, close) => AppFileForm(app: app, close: close),
                  ),
                ),
              ),
            ),
          if (app.hosts.isNotEmpty) _Warning(sendsTo(app.name, app.hosts)),
        ],
        const SizedBox(height: Space.m),
        Text('Agents', style: _heading(context, palette)),
        const SizedBox(height: Space.xs),
        if (app.grants.isEmpty)
          Text(
            'No agent can use it yet.',
            style: TypeScale.of(context).body
                .copyWith(color: palette.textMuted),
          )
        else
          for (final grant in app.grants)
            _GrantLine(
              app: app,
              grant: grant,
              workspace: workspace,
              onUndo: _undo,
            ),
        if (app.scopes.isNotEmpty && _ungranted(workspace, app).isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          Align(
            alignment: Alignment.centerLeft,
            child: Button(
              label: 'Let an agent use it…',
              onPressed: () => showModal(
                context,
                (context, close) => AppAccessForm(app: app, close: close),
              ),
            ),
          ),
        ],
        if (app.hosts.isNotEmpty) ...[
          const SizedBox(height: Space.m),
          Text('It can reach', style: _heading(context, palette)),
          _Lines(app.hosts, mono: true),
        ],
        const SizedBox(height: Space.m),
        Text(
          [
            'Version ${app.version}',
            if (app.publisher.isNotEmpty &&
                app.tier != AppTier.APP_TIER_CLAKTORY)
              publisherLine(app.tier, app.publisher),
            tierLine(app.tier, app.publisherKey),
            if (app.bundled) 'Comes with Claktory',
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

/// Who gave something in an app's access as it is now, and when:
/// "Given by Rivet", "You approved it", "You gave it" (for a [source],
/// turned on). Null-free: an agent since deleted is "an agent".
String appAccessGivenText(
  Workspace workspace,
  AppAccessGiven given, {
  bool source = false,
}) {
  final by = given.byAgentId;
  if (by.isNotEmpty) {
    final name =
        workspace.agents.where((agent) => agent.id == by).firstOrNull?.name ??
        'an agent';
    return source ? 'Turned on by $name' : 'Given by $name';
  }
  if (given.byCard) return 'You approved it';
  return source ? 'You turned it on' : 'You gave it';
}

/// Undoes the app access change [changeId]; what to tell the person
/// after: what was left as it was because it changed since, or null when
/// it all went back.
Future<String?> undoAppAccess(Backend backend, String changeId) async {
  final undone = await backend.undoAppAccessChange(changeId);
  if (undone.kept.isNotEmpty) return undone.kept.join(' ');
  if (undone.changeId.isEmpty) {
    return "Nothing to undo: it's all changed since.";
  }
  return null;
}

/// "Given by Rivet · 2h ago · Undo": who set this part of an app's access
/// as it is, when, and Undo, which puts back what that change replaced.
class AppAccessGivenLine extends StatelessWidget {
  const AppAccessGivenLine({
    super.key,
    required this.given,
    required this.onUndo,
    this.source = false,
  });

  final AppAccessGiven given;
  final ValueChanged<String> onUndo;

  /// A source turned on, rather than an agent's access.
  final bool source;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final style = TypeScale.of(context).caption
        .copyWith(color: palette.textFaint);
    final at = given.atMs.toInt();
    return Wrap(
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        Text(
          [
            appAccessGivenText(workspace, given, source: source),
            if (at > 0) formatAge(at, DateTime.now()),
          ].join(' · '),
          style: style,
        ),
        if (given.changeId.isNotEmpty) ...[
          Text(' · ', style: style),
          Semantics(
            button: true,
            child: Pressable(
              onTap: () => onUndo(given.changeId),
              builder: (context, hovered) => Text(
                'Undo',
                style: style.copyWith(
                  color: hovered ? palette.text : palette.signal,
                ),
              ),
            ),
          ),
        ],
      ],
    );
  }
}

TextStyle _heading(BuildContext context, Palette palette) =>
    TypeScale.of(context).caption
        .copyWith(color: palette.textFaint, fontWeight: FontWeight.w500);

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
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.text),
                ),
                Text(
                  item.done ? 'Saved' : 'Not set yet',
                  style: TypeScale.of(context).caption.copyWith(
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
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textFaint),
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
    required this.onUndo,
  });

  final AppInfo app;
  final AppGrant grant;
  final Workspace workspace;
  final ValueChanged<String> onUndo;

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
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.text),
                ),
                if (scopes.isNotEmpty)
                  Text(
                    scopes.join(' · '),
                    style: TypeScale.of(context).caption
                        .copyWith(color: palette.textMuted),
                  ),
                Text(
                  events.isEmpty
                      ? 'Hears none of its events'
                      : 'Hears: ${labelInSentence(events.join(' · '))}',
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textFaint),
                ),
                if (grant.hasGiven())
                  AppAccessGivenLine(given: grant.given, onUndo: onUndo),
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
          ? 'Let an agent use ${app.name}'
          : '${app.name} for ${agent.name}',
      fields: [
        // Through the app, an agent reads what the app reads: with a scope
        // that reads it, where the app says which do (AppScopeSpec.reads_data).
        if (_read(app).isNotEmpty &&
            (!app.scopes.any((scope) => scope.readsData) ||
                app.scopes.any(
                  (scope) => scope.readsData && _scopes.contains(scope.id),
                ))) ...[
          _Warning(
            'Through ${app.name}, ${name.isEmpty ? 'the agent' : name} can '
            'also read ${_andList(_read(app))}.',
          ),
          const SizedBox(height: Space.l),
        ],
        if (agent == null)
          Field(
            label: 'Agent',
            child: choices.isEmpty
                ? Text(
                    'Every agent has it already.',
                    style: TypeScale.of(context).body
                        .copyWith(color: palette.textMuted),
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
    this.about,
    this.mono = false,
    this.private = false,
    this.given,
  });

  final String label;

  /// Who turned it on and when, with Undo, under the rest.
  final Widget? given;

  /// It holds people's words (AppDataSource.private): marked so.
  final bool private;

  /// What turning it on means, plainly, under the label, before [note].
  final String? about;
  final String? note;
  final bool mono;
  final bool value;
  final ValueChanged<bool>? onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final note = this.note;
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
                Text.rich(
                  TextSpan(
                    children: [
                      TextSpan(text: label),
                      if (private) _privateTag(context),
                    ],
                  ),
                  style: TypeScale.of(context).body.copyWith(
                    color: onChanged == null ? palette.textMuted : palette.text,
                  ),
                ),
                if (about case final about? when about.isNotEmpty)
                  Text(
                    about,
                    style: TypeScale.of(context).caption
                        .copyWith(color: palette.textMuted),
                  ),
                if (note != null && note.isNotEmpty)
                  Text(
                    note,
                    style: TypeScale.of(context).caption.copyWith(
                      color: palette.textFaint,
                      fontFamily: mono ? TextStyles.mono : null,
                    ),
                  ),
                ?given,
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// Add a file…: the CSV and TSV files in the workspace and the projects
/// that [app] could read (ListAppFiles), newest first, with a search; one
/// that can't be given says why and can't be chosen. Add gives the chosen
/// one (GrantAppSource), or says why Claktory refused it.
class AppFileForm extends StatefulWidget {
  const AppFileForm({super.key, required this.app, required this.close});

  final AppInfo app;
  final VoidCallback close;

  @override
  State<AppFileForm> createState() => _AppFileFormState();
}

class _AppFileFormState extends State<AppFileForm> {
  late final Backend _backend = AppScope.of(context);
  final _search = TextEditingController();
  Timer? _typing;

  var _files = <AppFileChoice>[];
  var _truncated = false;
  var _loading = true;
  String? _failed;

  /// Which ask is the latest: an answer to an older one is dropped.
  var _asked = 0;

  AppFileChoice? _chosen;
  var _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _search.addListener(_searchChanged);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _load();
    });
  }

  @override
  void dispose() {
    _typing?.cancel();
    _search.dispose();
    super.dispose();
  }

  void _searchChanged() {
    _typing?.cancel();
    _typing = Timer(const Duration(milliseconds: 300), () {
      if (mounted) _load();
    });
  }

  Future<void> _load() async {
    final asked = ++_asked;
    setState(() {
      _loading = true;
      _failed = null;
    });
    try {
      final found = await _backend.listAppFiles(
        widget.app.id,
        query: _search.text.trim(),
      );
      if (!mounted || asked != _asked) return;
      setState(() {
        _files = found.files;
        _truncated = found.truncated;
        _loading = false;
        if (!_files.any((file) => file.id == _chosen?.id)) _chosen = null;
      });
    } on RequestFailure catch (failure) {
      if (!mounted || asked != _asked) return;
      setState(() {
        _failed = failure.message;
        _loading = false;
      });
    }
  }

  Future<void> _add() async {
    final chosen = _chosen;
    if (chosen == null || _busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await _backend.grantAppSource(widget.app.id, chosen.id, granted: true);
      if (mounted) widget.close();
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
    final type = TypeScale.of(context);
    final caption = type.caption.copyWith(color: palette.textFaint);
    final query = _search.text.trim();
    final given = {
      for (final source in widget.app.sources)
        if (source.granted) source.id,
    };
    return FormFrame(
      title: 'Add a file to ${widget.app.name}',
      body:
          '${widget.app.name} reads the file as it is, each time it needs '
          'it. CSV and TSV files in your projects and workspace are listed '
          'here.',
      dirty: _chosen != null,
      fields: [
        TextField(
          controller: _search,
          placeholder: 'Search by name or project',
          semanticsLabel: 'Search files',
        ),
        const SizedBox(height: Space.s),
        if (_failed case final why?)
          Text(why, style: type.caption.copyWith(color: palette.danger))
        else if (_files.isEmpty)
          Text(
            _loading
                ? 'Loading…'
                : query.isEmpty
                ? 'No CSV or TSV files in your workspace or projects.'
                : 'No file matches “$query”.',
            style: type.body.copyWith(color: palette.textMuted),
          )
        else
          for (final file in _files)
            _FileChoiceRow(
              file: file,
              given: given.contains(file.id),
              chosen: _chosen?.id == file.id,
              onTap: () => setState(() => _chosen = file),
            ),
        if (_truncated) ...[
          const SizedBox(height: Space.xs),
          Text(
            'Only the newest files are listed. Search to find others.',
            style: caption,
          ),
        ],
      ],
      error: _error,
      submitLabel: _busy ? 'Adding…' : 'Add',
      onSubmit: _chosen == null || _busy ? null : _add,
      onCancel: widget.close,
    );
  }
}

/// One file in [AppFileForm]: where it is, then its size and when it
/// changed, or why it can't be given (then it can't be chosen); one the app
/// reads already says so.
class _FileChoiceRow extends StatelessWidget {
  const _FileChoiceRow({
    required this.file,
    required this.given,
    required this.chosen,
    required this.onTap,
  });

  final AppFileChoice file;
  final bool given;
  final bool chosen;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final open = file.why.isEmpty && !given;
    final modified = file.modifiedMs.toInt();
    final about = file.why.isNotEmpty
        ? file.why
        : [
            if (given) 'Already added',
            formatBytes(file.sizeBytes),
            if (modified > 0) 'changed ${formatAge(modified, DateTime.now())}',
          ].join(' · ');
    return Unselectable(
      child: Semantics(
        button: open,
        selected: chosen,
        child: Pressable(
          onTap: open ? onTap : null,
          builder: (context, hovered) => Container(
            constraints: BoxConstraints(
              minHeight: isPhoneLayout(context) ? Touch.target : 40,
            ),
            padding: const EdgeInsets.symmetric(
              horizontal: Space.s,
              vertical: Space.xs,
            ),
            decoration: BoxDecoration(
              color: chosen
                  ? palette.selected
                  : hovered && open
                  ? palette.hover
                  : null,
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Text(
                  proseCodes(file.label),
                  style: type.body.copyWith(
                    color: open ? palette.text : palette.textMuted,
                  ),
                ),
                Text(
                  about,
                  style: type.caption.copyWith(color: palette.textFaint),
                ),
              ],
            ),
          ),
        ),
      ),
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
            style: TypeScale.of(context).body.copyWith(color: palette.text),
          ),
          if (scope.tools.isNotEmpty)
            Text(
              scope.tools.join(', '),
              style: TypeScale.of(context).caption.copyWith(
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
            style: TypeScale.of(context).body.copyWith(
              color: palette.text,
              fontFamily: mono ? TextStyles.mono : null,
            ),
          ),
      ],
    );
  }
}

/// The sources [app] may read now, by their labels.
List<String> _read(AppInfo app) => [
  for (final source in app.sources)
    if (source.granted) source.label,
];

/// "A", "A and B", "A, B and C".
String _andList(List<String> items) => items.length < 2
    ? items.join()
    : '${items.take(items.length - 1).join(', ')} and ${items.last}';

/// What app [app] can read from [source], said plainly: Claktory's records
/// hold every agent's conversations, the person's answers among them.
String sourceReach(String app, AppDataSource source, Workspace workspace) {
  // The backend's own line on what it holds, where it says.
  if (source.holds.isNotEmpty) return source.holds;
  if (source.kind == AppDataKind.APP_DATA_KIND_FACTORY_RECORDS ||
      source.id == 'factory-records') {
    return '$app can read all your agents\' conversations, including your '
        'answers to their questions, and their tasks, checklists, monitors '
        'and usage.';
  }
  if (source.agentId == '*') {
    return '$app can read everything each agent you choose keeps in its '
        'database.';
  }
  final agent =
      [
        for (final agent in workspace.agents)
          if (agent.id == source.agentId) agent.name,
      ].firstOrNull ??
      source.agentId;
  return '$app can read everything $agent keeps in its database.';
}

/// Where app [app] can send what it reads: its [hosts].
String sendsTo(String app, List<String> hosts) =>
    '$app can send anything it reads to ${_andList(hosts)}.';

/// A source an app asks to read, in its install review: what it is, and
/// the app's reason under it.
class _SourceLine extends StatelessWidget {
  const _SourceLine({required this.source, required this.reach});

  final AppDataSource source;

  /// What it holds, plainly ([sourceReach]).
  final String reach;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.xs),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text.rich(
            TextSpan(
              children: [
                TextSpan(text: source.label),
                if (source.private) _privateTag(context),
              ],
            ),
            style: TypeScale.of(context).body.copyWith(color: palette.text),
          ),
          Text(
            reach,
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textMuted),
          ),
          if (source.reason.isNotEmpty)
            Text(
              source.reason,
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textFaint),
            ),
        ],
      ),
    );
  }
}

/// "Private" after a source that holds people's words (conversations, an
/// agent's database), as Rivet's proposal card marks it.
TextSpan _privateTag(BuildContext context) => TextSpan(
  text: '  Private',
  style: TypeScale.of(context).caption
      .copyWith(color: Palette.of(context).signal, fontWeight: FontWeight.w600),
);

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
      child: Text(
        text,
        style: TypeScale.of(context).body.copyWith(color: palette.text),
      ),
    );
  }
}
