import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'account.dart';
import 'backend.dart';
import 'controls.dart';
import 'devices.dart';
import 'identity.dart';
import 'login.dart';
import 'pane_chrome.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'secrets.dart';
import 'transcript.dart';
import 'tell_rivet.dart';
import 'selection.dart';

/// Factory-wide settings: provider sign-in and the defaults for new agents.
/// On a Mac, a pane beside the conversation ([InPane]); on a phone, or with
/// no agent to show it beside, it replaces the agent view until closed with
/// Done or Esc.
class SettingsView extends StatelessWidget {
  const SettingsView({super.key});

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    void close() => scope.page.value = AppPage.agent;
    // Beside a conversation, the pane's header names and closes it.
    final inPane = InPane.of(context);
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
                          style: TextStyles.label.copyWith(
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
                    child: ListView(
                      padding: const EdgeInsets.symmetric(vertical: Space.xl),
                      children: [
                        _Section(
                          title: 'General',
                          children: [_YourName(workspace: workspace)],
                        ),
                        // Only while the backend has the account on.
                        if (workspace.hasAccount() &&
                            workspace.account.state !=
                                ClaktoryAccountState
                                    .CLAKTORY_ACCOUNT_STATE_UNSPECIFIED) ...[
                          const SizedBox(height: Space.xxl),
                          _Section(
                            title: 'Account',
                            children: [
                              AccountSettings(account: workspace.account),
                            ],
                          ),
                        ],
                        const SizedBox(height: Space.xxl),
                        _Section(
                          title: 'Providers',
                          children: [
                            for (final provider in workspace.providers)
                              ProviderCard(
                                provider: provider,
                                flow:
                                    workspace.hasLogin() &&
                                        workspace.login.providerId ==
                                            provider.id
                                    ? workspace.login
                                    : null,
                              ),
                          ],
                        ),
                        const SizedBox(height: Space.xxl),
                        const _Section(
                          title: 'Secrets',
                          children: [SecretsSettings()],
                        ),
                        const SizedBox(height: Space.xxl),
                        _Section(
                          title: 'Defaults',
                          children: [_Defaults(workspace: workspace)],
                        ),
                        const SizedBox(height: Space.xxl),
                        _Section(
                          title: 'Devices',
                          children: [DevicesSettings(workspace: workspace)],
                        ),
                        const SizedBox(height: Space.xxl),
                        _Section(
                          title: 'Resources',
                          children: [_ResourceAlerts(workspace: workspace)],
                        ),
                        const SizedBox(height: Space.xxl),
                        _Section(
                          title: 'Display',
                          children: [_Highlights(workspace: workspace)],
                        ),
                      ],
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

class _Section extends StatelessWidget {
  const _Section({required this.title, required this.children});

  final String title;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Align(
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
                style: TextStyles.emphasis.copyWith(color: palette.text),
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
                  style: TextStyles.body.copyWith(color: palette.textMuted),
                )
              : SizedBox(
                  width: 280,
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
              width: 280,
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
        SettingRow(
          label: 'Summarize conversations at',
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  SizedBox(
                    width: 96,
                    child: TextField(
                      controller: _compactAt,
                      onSubmitted: (_) => _saveCompactAt(),
                    ),
                  ),
                  const SizedBox(width: Space.s),
                  Text(
                    'tokens',
                    style: TextStyles.body.copyWith(color: palette.textMuted),
                  ),
                  const SizedBox(width: Space.m),
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
                style: TextStyles.caption.copyWith(color: palette.textFaint),
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
                    'When an agent with a long conversation is left idle, '
                    'Claktory summarizes it shortly before picking it up '
                    'again would cost more, so your next message costs less.',
                value: settings.idleCompaction,
                onChanged: (on) => _update(UpdateSettings(idleCompaction: on)),
              ),
              const SizedBox(height: Space.s),
              Padding(
                padding: const EdgeInsets.only(left: Space.xl),
                child: _Switch(
                  label: 'Also after 5 minutes idle',
                  note:
                      'For conversations kept ready only 5 minutes. This '
                      'summarizes far more often.',
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
            style: TextStyles.caption.copyWith(color: palette.textMuted),
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
    final words = TextStyles.body.copyWith(color: palette.textMuted);
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
                    style: TextStyles.caption.copyWith(
                      color: palette.textFaint,
                    ),
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
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SettingRow(
          label: 'Your name',
          child: Row(
            children: [
              SizedBox(
                width: 200,
                child: TextField(
                  controller: _name,
                  placeholder: 'First name',
                  onSubmitted: (_) => _save(),
                ),
              ),
              const SizedBox(width: Space.m),
              Expanded(
                child: ListenableBuilder(
                  listenable: _name,
                  builder: (context, _) =>
                      _name.text.trim() == widget.workspace.settings.userName
                      ? Text(
                          'Your agents call you by it.',
                          style: TextStyles.caption.copyWith(
                            color: palette.textFaint,
                          ),
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
                  style: TextStyles.caption.copyWith(color: palette.textMuted),
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
                  color: enabled ? palette.text : palette.textFaint,
                ),
              ),
              const SizedBox(height: 2),
              Text(
                note,
                style: TextStyles.caption.copyWith(color: palette.textMuted),
              ),
            ],
          ),
        ),
      ],
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
              style: TextStyles.body.copyWith(color: palette.textMuted),
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
              style: TextStyles.body.copyWith(color: palette.textMuted),
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
