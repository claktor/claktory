import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'account.dart';
import 'apps.dart' show AppsSettings;
import 'avatar.dart';
import 'backend.dart';
import 'claktory_identity.dart' show ClaktoryIdentity, claktoryName;
import 'controls.dart';
import 'devices.dart';
import 'identity.dart';
import 'mobile/drawer.dart' show largeText;
import 'mobile/header.dart';
import 'mobile/input.dart' show InputKind;
import 'mobile/platform_policy.dart' show PhonePlatformPolicy;
import 'mobile/metrics.dart';
import 'mobile/navigation.dart';
import 'move.dart' show BackendMove, MoveSettingRow;
import 'pane_chrome.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'provider_picker.dart' show ProviderSettings;
import 'remove_mac.dart';
import 'scope.dart';
import 'screens.dart' show SettingsScroller, SettingsSectionAnchor;
import 'secrets.dart';
import 'transcript.dart';
import 'tell_rivet.dart';
import 'selection.dart';
import 'voice.dart' show VoiceSettings, voiceKnown;
import 'zoom.dart' show ZoomSetting;

/// Factory-wide settings: provider sign-in and the defaults for new agents.
/// On a Mac, a pane beside the conversation ([InPane]); on a phone, or with
/// no agent to show it beside, it replaces the agent view until closed with
/// Done or Esc.
class SettingsView extends StatelessWidget {
  const SettingsView({super.key, this.section});

  /// On a phone, the section this page shows (its id, "devices"); null
  /// lists them all.
  final String? section;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    void close() => scope.page.value = AppPage.agent;
    // Beside a conversation, the pane's header names and closes it.
    final inPane = InPane.of(context);
    // A phone lists the sections, each a page of its own.
    if (!inPane && isPhoneLayout(context)) {
      final section = this.section;
      return section == null
          ? const _PhoneSettings()
          : _PhoneSettingsPage(id: section);
    }
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: FocusScope(
        autofocus: !inPane,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (!inPane)
              WindowDrag(
                child: Container(
                  height: titleBarHeight,
                  padding: const EdgeInsets.only(
                    left: Space.xl,
                    right: Space.m,
                  ),
                  decoration: BoxDecoration(
                    border: Border(bottom: BorderSide(color: palette.hairline)),
                  ),
                  child: Row(
                    children: [
                      Expanded(
                        child: Text(
                          'Settings',
                          style: TypeScale.of(context).label.copyWith(
                            color: palette.text,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                      Button(label: 'Done', onPressed: close),
                    ],
                  ),
                ),
              ),
            Expanded(
              child: ListenableBuilder(
                listenable: scope.backend.workspace,
                builder: (context, _) {
                  final workspace = scope.backend.workspace.workspace;
                  return SelectableArea(
                    // Scrolls to the section an agent opens (screens.dart).
                    child: SettingsScroller(
                      builder: (context, controller) => ListView(
                        controller: controller,
                        padding: const EdgeInsets.symmetric(vertical: Space.xl),
                        children: [
                          for (final (i, section) in settingsSectionsOf(
                            context,
                            workspace,
                          ).indexed) ...[
                            if (i > 0) const SizedBox(height: Space.xxl),
                            _Section(
                              title: section.title,
                              children: section.children,
                            ),
                          ],
                        ],
                      ),
                    ),
                  );
                },
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A section of Settings: its title (its id in lower case, as show_screen
/// names it), what's in it, and on a phone, where each is a page of its
/// own, its glyph and a word on what it's set to.
class SettingsSection {
  const SettingsSection({
    required this.title,
    required this.children,
    required this.glyph,
    this.summary = '',
    this.group = 0,
  });

  final String title;
  final List<Widget> children;
  final Widget Function(Color color) glyph;

  /// What it's set to, at the end of its row on a phone ("Matt").
  final String summary;

  /// Rows of one group sit together on a phone, apart from the next.
  final int group;

  String get id => title.toLowerCase();
}

/// The models a step a model declines can be tried again on: [emptyLabel]
/// for none chosen ('', no fallback or the default's), then the models
/// there are, and [current] as "No model available" if it isn't one of
/// them.
List<SelectOption<String>> fallbackModelOptions(
  Workspace workspace,
  String current, {
  required String emptyLabel,
}) => [
  SelectOption('', emptyLabel),
  for (final model in workspace.models)
    SelectOption(model.id, modelLabel(model)),
  if (current.isNotEmpty &&
      !workspace.models.any((model) => model.id == current))
    SelectOption(current, 'No model available'),
];

/// The sections Settings shows now, in order.
List<SettingsSection> settingsSectionsOf(
  BuildContext context,
  Workspace workspace,
) {
  Widget Function(Color) lucide(String icon) =>
      (color) => ExtensionGlyph(icon: icon, name: '', color: color);
  final settings = workspace.settings;
  Model? model;
  for (final candidate in workspace.models) {
    if (candidate.id == settings.defaultModel) model = candidate;
  }
  return [
    SettingsSection(
      title: 'Profile',
      glyph: lucide('user'),
      summary: settings.userName,
      children: [
        _YourName(workspace: workspace),
        _Avatar(person: workspace.person),
      ],
    ),
    SettingsSection(
      title: 'General',
      glyph: (color) => CogGlyph(color: color),
      summary: claktoryName(AppScope.of(context)),
      children: [
        ClaktoryIdentity(workspace: workspace),
        // Moving this Claktory to another computer, from any device.
        MoveSettingRow(source: BackendMove(AppScope.of(context))),
      ],
    ),
    // Only while the backend has the account on.
    if (workspace.hasAccount() &&
        workspace.account.state !=
            ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_UNSPECIFIED)
      SettingsSection(
        title: 'Account',
        glyph: lucide('globe'),
        summary: workspace.account.email,
        children: [AccountSettings(account: workspace.account)],
      ),
    SettingsSection(
      title: 'Providers',
      group: 1,
      glyph: lucide('sparkles'),
      summary: [
        for (final provider in workspace.providers)
          if (provider.accounts.isNotEmpty || provider.signedInWith.isNotEmpty)
            provider.name,
      ].join(', '),
      children: [ProviderSettings(workspace: workspace)],
    ),
    SettingsSection(
      title: 'Secrets',
      group: 2,
      glyph: lucide('key-round'),
      children: const [SecretsSettings()],
    ),
    // Apps shows only where the backend runs them (the dev Claktory, until
    // Apps ships).
    if (workspace.appsAvailable)
      SettingsSection(
        title: 'Apps',
        group: 2,
        glyph: lucide('panels-top-left'),
        children: const [AppsSettings()],
      ),
    SettingsSection(
      title: 'Defaults',
      group: 1,
      glyph: lucide('brain'),
      summary: model == null ? '' : modelLabel(model),
      children: [_Defaults(workspace: workspace)],
    ),
    // Only where the Claktory knows voice messages.
    if (voiceKnown(workspace))
      SettingsSection(
        title: 'Voice',
        group: 1,
        glyph: lucide('mic'),
        children: [VoiceSettings(model: workspace.speechModel)],
      ),
    SettingsSection(
      title: 'Devices',
      group: 2,
      glyph: lucide('monitor-smartphone'),
      children: [DevicesSettings(workspace: workspace)],
    ),
    SettingsSection(
      title: 'Resources',
      group: 3,
      glyph: lucide('gauge'),
      children: [_ResourceAlerts(workspace: workspace)],
    ),
    SettingsSection(
      title: 'Display',
      group: 3,
      glyph: lucide('monitor'),
      children: [
        const ZoomSetting(),
        _Highlights(workspace: workspace),
      ],
    ),
    // Removing Claktory, from the Mac it runs on.
    if (canRemoveThisMac(context))
      SettingsSection(
        title: 'This Mac',
        group: 4,
        glyph: lucide('laptop-minimal'),
        children: const [RemoveThisMac()],
      ),
  ];
}

/// Settings on a phone: a page listing the sections, each opening as a
/// page of its own, with Back. A section an agent opens (show_screen)
/// opens over the list.
class _PhoneSettings extends StatefulWidget {
  const _PhoneSettings();

  @override
  State<_PhoneSettings> createState() => _PhoneSettingsState();
}

class _PhoneSettingsState extends State<_PhoneSettings> {
  ValueNotifier<String?>? _section;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final section = AppScope.of(context).settingsSection;
    if (section == _section) return;
    _section?.removeListener(_wanted);
    _section = section..addListener(_wanted);
    _wanted();
  }

  @override
  void dispose() {
    _section?.removeListener(_wanted);
    super.dispose();
  }

  void _wanted() {
    if (_section?.value == null) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final section = _section;
      final id = section?.value;
      if (!mounted || section == null || id == null) return;
      section.value = null;
      final workspace = AppScope.of(context).workspace.workspace;
      // Not in Settings now (Account while signed out, say): the list.
      if (settingsSectionsOf(context, workspace).any((s) => s.id == id)) {
        _open(id);
      }
    });
    // Asked for between frames: one to run it in.
    WidgetsBinding.instance.ensureVisualUpdate();
  }

  void _open(String id) => PhoneRoutes.of(context)?.push<void>(
    PhoneRoute(
      id: 'settings:$id',
      builder: (context) => SettingsView(section: id),
    ),
  );

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    void close() => scope.page.value = AppPage.agent;
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          PhoneHeader(
            leading: HeaderButton.back(onTap: close),
            title: Text(
              'Settings',
              style: PhoneType.emphasis.copyWith(color: palette.text),
            ),
          ),
          Expanded(
            child: ListenableBuilder(
              listenable: scope.backend.workspace,
              builder: (context, _) {
                // In their groups: who you are and this Claktory; models;
                // what agents can reach; this computer and the screen.
                final all = settingsSectionsOf(
                  context,
                  scope.backend.workspace.workspace,
                );
                final sections = [
                  for (var group = 0; group <= 4; group++)
                    ...all.where((section) => section.group == group),
                ];
                return ListView(
                  padding: const EdgeInsets.symmetric(vertical: Space.s),
                  children: [
                    for (final (i, section) in sections.indexed) ...[
                      if (i > 0 && sections[i - 1].group != section.group)
                        Padding(
                          padding: const EdgeInsets.symmetric(
                            horizontal: Touch.gutter,
                            vertical: Space.s,
                          ),
                          child: Container(height: 1, color: palette.hairline),
                        ),
                      _SectionRow(
                        section: section,
                        onTap: () => _open(section.id),
                      ),
                    ],
                  ],
                );
              },
            ),
          ),
        ],
      ),
    );
  }
}

/// A section's row in a phone's Settings: its glyph, its title, what it's
/// set to, and a chevron to its page. At large text sizes what it's set to
/// goes under the title.
class _SectionRow extends StatelessWidget {
  const _SectionRow({required this.section, required this.onTap});

  final SettingsSection section;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final large = largeText(context);
    final title = Text(
      section.title,
      style: PhoneType.body.copyWith(color: palette.text),
    );
    final summary = section.summary.isEmpty
        ? null
        : Text(
            section.summary,
            maxLines: large ? 2 : 1,
            overflow: TextOverflow.ellipsis,
            textAlign: large ? TextAlign.start : TextAlign.end,
            style: (large ? PhoneType.secondary : PhoneType.body).copyWith(
              color: palette.textMuted,
            ),
          );
    return Semantics(
      container: true,
      button: true,
      onTap: onTap,
      child: Pressable(
        onTap: onTap,
        builder: (context, pressed) => Container(
          constraints: const BoxConstraints(minHeight: Touch.row),
          color: pressed ? palette.hover : const Color(0x00000000),
          padding: const EdgeInsets.symmetric(
            horizontal: Touch.gutter,
            vertical: Space.s,
          ),
          child: Row(
            children: [
              SizedBox(
                width: 36,
                child: Align(
                  alignment: AlignmentDirectional.centerStart,
                  child: section.glyph(palette.textMuted),
                ),
              ),
              if (large)
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [title, ?summary],
                  ),
                )
              else ...[
                title,
                const SizedBox(width: Space.m),
                Expanded(child: summary ?? const SizedBox.shrink()),
              ],
              const SizedBox(width: Space.s),
              Chevron(color: palette.textFaint, down: false),
            ],
          ),
        ),
      ),
    );
  }
}

/// A section of Settings as a page of its own on a phone, with Back to the
/// list.
class _PhoneSettingsPage extends StatelessWidget {
  const _PhoneSettingsPage({required this.id});

  final String id;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    void back() => PhoneRoutes.of(context)?.pop<void>();
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): back},
      child: ListenableBuilder(
        listenable: scope.backend.workspace,
        builder: (context, _) {
          SettingsSection? section;
          for (final candidate in settingsSectionsOf(
            context,
            scope.backend.workspace.workspace,
          )) {
            if (candidate.id == id) section = candidate;
          }
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              PhoneHeader(
                leading: HeaderButton.back(onTap: back),
                title: Text(
                  section?.title ?? '',
                  style: PhoneType.emphasis.copyWith(color: palette.text),
                ),
              ),
              Expanded(
                child: SelectableArea(
                  child: ListView(
                    padding: const EdgeInsets.fromLTRB(
                      Touch.gutter,
                      Space.l,
                      Touch.gutter,
                      Space.xl,
                    ),
                    children: [
                      for (final child in section?.children ?? <Widget>[]) ...[
                        child,
                        const SizedBox(height: Space.s),
                      ],
                    ],
                  ),
                ),
              ),
            ],
          );
        },
      ),
    );
  }
}

class _Section extends StatelessWidget {
  const _Section({required this.title, required this.children});

  final String title;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return SettingsSectionAnchor(
      id: title.toLowerCase(),
      child: Align(
        alignment: Alignment.topCenter,
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 640),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: Space.xl),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  title,
                  style: TypeScale.of(context).emphasis
                      .copyWith(color: palette.text),
                ),
                const SizedBox(height: Space.m),
                for (final child in children) ...[
                  child,
                  const SizedBox(height: Space.s),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Runs requests for a settings control and shows why one was refused.
mixin SettingRequests<T extends StatefulWidget> on State<T> {
  String? failure;

  Future<void> run(Future<void> Function() request) async {
    setState(() => failure = null);
    try {
      await request();
    } on RequestFailure catch (error) {
      if (mounted) setState(() => failure = error.message);
    }
  }

  Widget failureLine(Palette palette) => failure == null
      ? const SizedBox.shrink()
      : Padding(
          padding: const EdgeInsets.only(top: Space.s),
          child: ErrorLine(failure!, about: 'In Settings:'),
        );
}

/// The model, effort and compaction threshold new agents start with. Only
/// models that can run now are offered.
class _Defaults extends StatefulWidget {
  const _Defaults({required this.workspace});

  final Workspace workspace;

  @override
  State<_Defaults> createState() => _DefaultsState();
}

class _DefaultsState extends State<_Defaults> with SettingRequests {
  final _compactAt = TextEditingController();
  var _shownCompactAt = Int64.ZERO;

  @override
  void initState() {
    super.initState();
    _syncCompactAt();
  }

  @override
  void didUpdateWidget(_Defaults old) {
    super.didUpdateWidget(old);
    _syncCompactAt();
  }

  /// Shows the backend's threshold unless the user is editing it.
  void _syncCompactAt() {
    final value = widget.workspace.settings.compactAtTokens;
    if (value == _shownCompactAt) return;
    final editing = _compactAt.text != formatTokens(_shownCompactAt);
    _shownCompactAt = value;
    if (!editing || _compactAt.text.isEmpty) {
      _compactAt.text = formatTokens(value);
    }
  }

  @override
  void dispose() {
    _compactAt.dispose();
    super.dispose();
  }

  void _update(UpdateSettings update) =>
      run(() => AppScope.of(context).updateSettings(update));

  void _saveCompactAt() {
    final tokens = parseTokens(_compactAt.text);
    if (tokens == null || tokens <= 0) {
      setState(() => failure = 'Enter a number of tokens, like 400k.');
      return;
    }
    _update(UpdateSettings(compactAtTokens: Int64(tokens)));
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final workspace = widget.workspace;
    final settings = workspace.settings;
    Model? model;
    for (final candidate in workspace.models) {
      if (candidate.id == settings.defaultModel) model = candidate;
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SettingRow(
          label: 'Model',
          child: workspace.models.isEmpty
              ? Text(
                  'No model available. Sign in to a provider above.',
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.textMuted),
                )
              : SizedBox(
                  width: controlWidth(context, 280),
                  child: Select(
                    value: model?.id ?? '',
                    options: [
                      if (model == null)
                        const SelectOption('', 'No model available'),
                      for (final candidate in workspace.models)
                        SelectOption(candidate.id, modelLabel(candidate)),
                    ],
                    onChanged: (id) {
                      if (id.isEmpty) return;
                      final update = UpdateSettings(defaultModel: id);
                      // Keep the effort only if the new model accepts it.
                      for (final candidate in workspace.models) {
                        if (candidate.id == id &&
                            settings.defaultEffort.isNotEmpty &&
                            !candidate.efforts.contains(
                              settings.defaultEffort,
                            )) {
                          update.defaultEffort = '';
                        }
                      }
                      _update(update);
                    },
                  ),
                ),
        ),
        if (model != null && model.efforts.isNotEmpty)
          SettingRow(
            label: 'Reasoning effort',
            child: SizedBox(
              width: controlWidth(context, 280),
              child: Select(
                value: model.efforts.contains(settings.defaultEffort)
                    ? settings.defaultEffort
                    : '',
                options: [
                  if (!model.efforts.contains(settings.defaultEffort))
                    const SelectOption('', 'Not set'),
                  for (final effort in model.efforts)
                    SelectOption(effort, effort),
                ],
                onChanged: (effort) {
                  if (effort.isNotEmpty) {
                    _update(UpdateSettings(defaultEffort: effort));
                  }
                },
              ),
            ),
          ),
        if (workspace.models.isNotEmpty)
          SettingRow(
            label: 'Default fallback model',
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                SizedBox(
                  width: controlWidth(context, 280),
                  child: Select(
                    value: settings.defaultFallbackModel,
                    options: fallbackModelOptions(
                      workspace,
                      settings.defaultFallbackModel,
                      emptyLabel: 'None',
                    ),
                    onChanged: (id) =>
                        _update(UpdateSettings(defaultFallbackModel: id)),
                  ),
                ),
                const SizedBox(height: Space.xs),
                Text(
                  'When a model declines a step for safety reasons, Claktory '
                  "tries it again on this one. An agent's settings can choose "
                  'another.',
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textFaint),
                ),
              ],
            ),
          ),
        SettingRow(
          label: 'Summarize conversations at',
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _Line(
                children: [
                  SizedBox(
                    width: isPhoneLayout(context) ? 120 : 96,
                    child: TextField(
                      controller: _compactAt,
                      // "400k": digits first, the letters a tap away.
                      keyboardType: const TextInputType.numberWithOptions(
                        signed: true,
                      ),
                      autocorrect: false,
                      onSubmitted: (_) => _saveCompactAt(),
                    ),
                  ),
                  Text(
                    'tokens',
                    style: TypeScale.of(context).body
                        .copyWith(color: palette.textMuted),
                  ),
                  ListenableBuilder(
                    listenable: _compactAt,
                    builder: (context, _) =>
                        _compactAt.text.trim() ==
                            formatTokens(settings.compactAtTokens)
                        ? const SizedBox.shrink()
                        : Button(label: 'Save', onPressed: _saveCompactAt),
                  ),
                ],
              ),
              const SizedBox(height: Space.xs),
              Text(
                'When a conversation gets this long, older messages are '
                'summarized so the agent can keep going.',
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textFaint),
              ),
            ],
          ),
        ),
        SettingRow(
          label: 'Idle conversations',
          top: true,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _Switch(
                label: 'Summarize idle conversations',
                note:
                    'When a long conversation sits idle, Claktory summarizes '
                    'it just before it stops being kept ready, so your next '
                    'message costs less.',
                value: settings.idleCompaction,
                onChanged: (on) => _update(UpdateSettings(idleCompaction: on)),
              ),
              const SizedBox(height: Space.s),
              Padding(
                padding: const EdgeInsets.only(left: Space.xl),
                child: _Switch(
                  label: 'Also after 5 minutes idle',
                  note:
                      'For conversations kept ready only 5 minutes. '
                      'Summarizes far more often.',
                  value: settings.idleCompactionShortCache,
                  onChanged: settings.idleCompaction
                      ? (on) => _update(
                          UpdateSettings(idleCompactionShortCache: on),
                        )
                      : null,
                ),
              ),
            ],
          ),
        ),
        failureLine(palette),
      ],
    );
  }
}

/// When admin agents with the Resources extension are told the machine runs
/// low: memory used, a volume's free space, the processors' load.
class _ResourceAlerts extends StatefulWidget {
  const _ResourceAlerts({required this.workspace});

  final Workspace workspace;

  @override
  State<_ResourceAlerts> createState() => _ResourceAlertsState();
}

class _ResourceAlertsState extends State<_ResourceAlerts> with SettingRequests {
  void _save(UpdateSettings update) =>
      run(() => AppScope.of(context).updateSettings(update));

  void _invalid() => setState(() => failure = 'Enter a whole number.');

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final settings = widget.workspace.settings;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.only(bottom: Space.m),
          child: Text(
            'Rivet tells you when this computer runs low. Alert when:',
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textMuted),
          ),
        ),
        _Threshold(
          label: 'Memory',
          parts: [(settings.ramPercent, '% used')],
          lead: 'Above',
          onInvalid: _invalid,
          onSave: (values) => _save(UpdateSettings(ramPercent: values[0])),
        ),
        _Threshold(
          label: 'Disk',
          parts: [
            (settings.diskFreeGb, 'GB or'),
            (settings.diskFreePercent, '% free'),
          ],
          lead: 'Below',
          note: settings.diskFreeGb == 0 ? 'Off' : '0 GB turns it off.',
          onInvalid: _invalid,
          onSave: (values) => _save(
            UpdateSettings(diskFreeGb: values[0], diskFreePercent: values[1]),
          ),
        ),
        _Threshold(
          label: 'Processors',
          parts: [(settings.cpuPercent, '% busy')],
          lead: 'Above',
          note: settings.cpuPercent == 0 ? 'Off' : '0 turns it off.',
          onInvalid: _invalid,
          onSave: (values) => _save(UpdateSettings(cpuPercent: values[0])),
        ),
        _Threshold(
          label: 'Build caches',
          parts: [(settings.buildCacheGb.round(), 'GB each')],
          lead: 'Up to',
          note:
              'When one grows past this, Claktory clears it while nothing\'s '
              'using it. 0 means never.',
          onInvalid: _invalid,
          onSave: (values) =>
              _save(UpdateSettings(buildCacheGb: values[0].toDouble())),
        ),
        failureLine(palette),
      ],
    );
  }
}

/// One alert's threshold: "[lead] [n] unit [n] unit", each number a 56px
/// field showing the setting in effect unless it's being edited, and Save
/// while any differs.
class _Threshold extends StatefulWidget {
  const _Threshold({
    required this.label,
    required this.parts,
    required this.lead,
    required this.onSave,
    required this.onInvalid,
    this.note = '',
  });

  final String label;

  /// Each number in effect and the words after it.
  final List<(int, String)> parts;
  final String lead;
  final String note;
  final void Function(List<int> values) onSave;

  /// A field isn't a whole number.
  final VoidCallback onInvalid;

  @override
  State<_Threshold> createState() => _ThresholdState();
}

class _ThresholdState extends State<_Threshold> {
  late final _fields = [
    for (final (value, _) in widget.parts)
      TextEditingController(text: '$value'),
  ];
  late var _shown = [for (final (value, _) in widget.parts) value];

  @override
  void didUpdateWidget(_Threshold old) {
    super.didUpdateWidget(old);
    // Show the backend's value unless the person is editing that field.
    for (var i = 0; i < _fields.length; i++) {
      final value = widget.parts[i].$1;
      if (value == _shown[i]) continue;
      final editing = _fields[i].text.trim() != '${_shown[i]}';
      if (!editing || _fields[i].text.isEmpty) _fields[i].text = '$value';
    }
    _shown = [for (final (value, _) in widget.parts) value];
  }

  @override
  void dispose() {
    for (final field in _fields) {
      field.dispose();
    }
    super.dispose();
  }

  bool get _changed => [
    for (var i = 0; i < _fields.length; i++)
      _fields[i].text.trim() != '${widget.parts[i].$1}',
  ].any((changed) => changed);

  void _save() {
    final values = [
      for (final field in _fields) int.tryParse(field.text.trim()),
    ];
    // The backend says what's out of range; this only needs numbers.
    if (values.any((value) => value == null || value < 0)) {
      widget.onInvalid();
      return;
    }
    widget.onSave([for (final value in values) value!]);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final words = TypeScale.of(context).body.copyWith(color: palette.textMuted);
    return SettingRow(
      label: widget.label,
      child: Wrap(
        spacing: Space.s,
        runSpacing: Space.xs,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Text(widget.lead, style: words),
          for (var i = 0; i < _fields.length; i++) ...[
            SizedBox(
              width: 56,
              child: TextField(
                controller: _fields[i],
                // Digits, with Return to save (a number pad has none).
                keyboardType: const TextInputType.numberWithOptions(
                  signed: true,
                ),
                autocorrect: false,
                onSubmitted: (_) => _save(),
              ),
            ),
            Text(widget.parts[i].$2, style: words),
          ],
          ListenableBuilder(
            listenable: Listenable.merge(_fields),
            builder: (context, _) => _changed
                ? Button(label: 'Save', onPressed: _save)
                : widget.note.isEmpty
                ? const SizedBox.shrink()
                : Text(
                    widget.note,
                    style: TypeScale.of(context).caption
                        .copyWith(color: palette.textFaint),
                  ),
          ),
        ],
      ),
    );
  }
}

/// The person's first name, which agents call them by
/// (`Settings.user_name`); asked on the first launch, changed here.
class _YourName extends StatefulWidget {
  const _YourName({required this.workspace});

  final Workspace workspace;

  @override
  State<_YourName> createState() => _YourNameState();
}

class _YourNameState extends State<_YourName> with SettingRequests {
  late final _name = TextEditingController(
    text: widget.workspace.settings.userName,
  );
  late var _shown = widget.workspace.settings.userName;

  @override
  void didUpdateWidget(_YourName old) {
    super.didUpdateWidget(old);
    // The backend's name, unless the person is editing it.
    final name = widget.workspace.settings.userName;
    if (name == _shown) return;
    if (_name.text.trim() == _shown.trim()) _name.text = name;
    _shown = name;
  }

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  void _save() {
    final name = _name.text.trim();
    if (name.isEmpty) {
      setState(() => failure = 'Enter your first name.');
      return;
    }
    run(
      () => AppScope.of(context).updateSettings(UpdateSettings(userName: name)),
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final field = TextField(
      controller: _name,
      placeholder: 'First name',
      kind: InputKind.name,
      onSubmitted: (_) => _save(),
    );
    if (isPhoneLayout(context)) {
      // The field as wide as the screen, Save or the note under it.
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SettingRow(
            label: 'Your name',
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                field,
                const SizedBox(height: Space.s),
                ListenableBuilder(
                  listenable: _name,
                  builder: (context, _) =>
                      _name.text.trim() == widget.workspace.settings.userName
                      ? Text(
                          'Your agents call you by it.',
                          style: PhoneType.caption.copyWith(
                            color: palette.textFaint,
                          ),
                        )
                      : Align(
                          alignment: AlignmentDirectional.centerStart,
                          child: Button(label: 'Save', onPressed: _save),
                        ),
                ),
              ],
            ),
          ),
          failureLine(palette),
        ],
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SettingRow(
          label: 'Your name',
          child: Row(
            children: [
              SizedBox(width: 200, child: field),
              const SizedBox(width: Space.m),
              Expanded(
                child: ListenableBuilder(
                  listenable: _name,
                  builder: (context, _) =>
                      _name.text.trim() == widget.workspace.settings.userName
                      ? Text(
                          'Your agents call you by it.',
                          style: TypeScale.of(context).caption
                              .copyWith(color: palette.textFaint),
                        )
                      : Align(
                          alignment: Alignment.centerLeft,
                          child: Button(label: 'Save', onPressed: _save),
                        ),
                ),
              ),
            ],
          ),
        ),
        failureLine(palette),
      ],
    );
  }
}

/// The avatar's colors as swatches, each its gradient, the chosen one
/// ringed, as the agents' colors are (ColorPicker): a finger's target on
/// a phone, wrapping onto more lines.
class _AvatarColors extends StatelessWidget {
  const _AvatarColors({
    required this.choices,
    required this.chosen,
    required this.onChanged,
  });

  final List<AvatarColors> choices;
  final int chosen;
  final ValueChanged<int> onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    // At least the platform's floor where it's used with a finger
    // (Android's 48); a Mac's and an iPad's stay as they were.
    final size = PhonePlatformPolicy.least(
      context,
      phone ? Touch.target : 26.0,
    );
    return Wrap(
      spacing: phone ? Space.xs : Space.s,
      runSpacing: phone ? Space.xs : Space.s,
      children: [
        for (final (i, choice) in choices.indexed)
          Semantics(
            key: ValueKey('avatar-color-$i'),
            button: true,
            selected: i == chosen,
            label: choice.name,
            child: Pressable(
              onTap: i == chosen ? null : () => onChanged(i),
              builder: (context, hovered) => AnimatedContainer(
                duration: Motion.fast,
                width: size,
                height: size,
                padding: EdgeInsets.all(phone ? 5 : 3),
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  border: Border.all(
                    width: 2,
                    color: i == chosen
                        ? palette.text
                        : hovered
                        ? palette.textFaint
                        : const Color(0x00000000),
                  ),
                ),
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    gradient: LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: [
                        hexColor(choice.from) ?? palette.textFaint,
                        hexColor(choice.to) ?? palette.textFaint,
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
      ],
    );
  }
}

/// The person's avatar (Workspace.person): its colors to pick from
/// (Person.avatar_choices) and Shuffle, and the account they're signed in
/// as, when they are.
class _Avatar extends StatefulWidget {
  const _Avatar({required this.person});

  final Person person;

  @override
  State<_Avatar> createState() => _AvatarState();
}

class _AvatarState extends State<_Avatar> with SettingRequests {
  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final person = widget.person;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SettingRow(
          label: 'Avatar',
          child: Builder(
            builder: (context) {
              final avatar = PersonAvatar(person: person, size: 40);
              final shuffle = Button(
                label: 'Shuffle colors',
                onPressed: () => run(AppScope.of(context).shuffleAvatar),
              );
              final caption = Text(
                'Shows beside your messages and comments, on every device.',
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textFaint),
              );
              final colors = person.avatarChoices.isEmpty
                  ? null
                  : _AvatarColors(
                      choices: person.avatarChoices,
                      chosen: person.avatarChoice,
                      onChanged: (choice) => run(
                        () =>
                            AppScope.of(context).shuffleAvatar(choice: choice),
                      ),
                    );
              // A phone's finger-sized button leaves the words no room
              // beside it: they go under.
              if (isPhoneLayout(context)) {
                return Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Wrap(
                      spacing: Space.m,
                      runSpacing: Space.s,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [avatar, shuffle],
                    ),
                    const SizedBox(height: Space.s),
                    caption,
                    if (colors != null) ...[
                      const SizedBox(height: Space.m),
                      colors,
                    ],
                  ],
                );
              }
              final line = Row(
                children: [
                  avatar,
                  const SizedBox(width: Space.m),
                  shuffle,
                  const SizedBox(width: Space.m),
                  Expanded(child: caption),
                ],
              );
              if (colors == null) return line;
              return Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  line,
                  const SizedBox(height: Space.m),
                  colors,
                ],
              );
            },
          ),
        ),
        if (person.email.isNotEmpty)
          SettingRow(
            label: 'Signed in as',
            child: Text(
              person.email,
              style: TypeScale.of(context).body.copyWith(color: palette.text),
            ),
          ),
        failureLine(palette),
      ],
    );
  }
}

/// Whether what changes glows (Workspace.highlights).
class _Highlights extends StatefulWidget {
  const _Highlights({required this.workspace});

  final Workspace workspace;

  @override
  State<_Highlights> createState() => _HighlightsState();
}

class _HighlightsState extends State<_Highlights> with SettingRequests {
  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final on = widget.workspace.highlights;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SettingRow(
          label: 'Highlights',
          child: ToggleTap(
            value: on,
            onChanged: (value) =>
                run(() => AppScope.of(context).setHighlights(value)),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Padding(
                  padding: const EdgeInsets.only(top: 2),
                  child: Toggle(
                    value: on,
                    label: 'Highlights',
                    onChanged: (value) =>
                        run(() => AppScope.of(context).setHighlights(value)),
                  ),
                ),
                const SizedBox(width: Space.m),
                Expanded(
                  child: Text(
                    'A soft glow on what changes: new agents, projects and '
                    'folders, checklist updates, messages between agents, '
                    'and what an agent points out to you.',
                    style: TypeScale.of(context).caption
                        .copyWith(color: palette.textMuted),
                  ),
                ),
              ],
            ),
          ),
        ),
        failureLine(palette),
      ],
    );
  }
}

/// A switch with its label and a note under it.
class _Switch extends StatelessWidget {
  const _Switch({
    required this.label,
    required this.note,
    required this.value,
    required this.onChanged,
  });

  final String label;
  final String note;
  final bool value;

  /// Null when it can't be changed now.
  final ValueChanged<bool>? onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final enabled = onChanged != null;
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
                Text(
                  label,
                  style: TypeScale.of(context).body.copyWith(
                    color: enabled ? palette.text : palette.textFaint,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  note,
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

class SettingRow extends StatelessWidget {
  const SettingRow({
    super.key,
    required this.label,
    required this.child,
    this.top = false,
  });

  final String label;
  final Widget child;

  /// The label at the top of a tall control (a paragraph, switches with
  /// notes) rather than beside its middle.
  final bool top;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return LayoutBuilder(
      builder: (context, constraints) => _build(context, palette, constraints),
    );
  }

  Widget _build(
    BuildContext context,
    Palette palette,
    BoxConstraints constraints,
  ) {
    // On a phone, or in a narrow pane beside a conversation.
    if (isPhoneLayout(context) || constraints.maxWidth < 440) {
      // No room beside it: the label over the control.
      return Padding(
        padding: const EdgeInsets.only(bottom: Space.m),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              label,
              style: TypeScale.of(context).body
                  .copyWith(color: palette.textMuted),
            ),
            const SizedBox(height: Space.xs),
            Align(alignment: Alignment.centerLeft, child: child),
          ],
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.m),
      child: Row(
        crossAxisAlignment: top
            ? CrossAxisAlignment.start
            : CrossAxisAlignment.center,
        children: [
          SizedBox(
            width: 160,
            child: Text(
              label,
              style: TypeScale.of(context).body
                  .copyWith(color: palette.textMuted),
            ),
          ),
          Expanded(
            child: Align(
              alignment: top ? Alignment.topLeft : Alignment.centerLeft,
              child: child,
            ),
          ),
        ],
      ),
    );
  }
}

/// How wide a control in Settings is: [width] on a Mac, the screen's width
/// on a phone.
double controlWidth(BuildContext context, double width) =>
    isPhoneLayout(context) ? double.infinity : width;

/// Controls in a line: a field, its unit, Save. A Mac keeps them on one
/// line, with [Space.s] then [Space.m] before the last; a phone wraps them
/// onto more lines when the text is large.
class _Line extends StatelessWidget {
  const _Line({required this.children});

  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    if (isPhoneLayout(context)) {
      return Wrap(
        spacing: Space.s,
        runSpacing: Space.s,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: children,
      );
    }
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final (i, child) in children.indexed) ...[
          if (i > 0)
            SizedBox(width: i == children.length - 1 ? Space.m : Space.s),
          child,
        ],
      ],
    );
  }
}

/// A model's display name, falling back to its id.
String modelLabel(Model model) => model.name.isEmpty ? model.id : model.name;

/// "400k", "400000", "1.2M" or "400 000" → tokens; null if it isn't a count.
int? parseTokens(String text) {
  var value = text.trim().replaceAll(RegExp(r'[\s,_]'), '').toLowerCase();
  var scale = 1;
  if (value.endsWith('k')) {
    scale = 1000;
    value = value.substring(0, value.length - 1);
  } else if (value.endsWith('m')) {
    scale = 1000000;
    value = value.substring(0, value.length - 1);
  }
  final number = double.tryParse(value);
  if (number == null || number.isNaN || number.isInfinite) return null;
  return (number * scale).round();
}
