import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'devices.dart';
import 'identity.dart';
import 'login.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'transcript.dart';
import 'tell_rivet.dart';
import 'selection.dart';

/// Factory-wide settings: provider sign-in and the defaults for new agents.
/// Replaces the agent view until closed with Done or Esc.
class SettingsView extends StatelessWidget {
  const SettingsView({super.key});

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    void close() => scope.page.value = AppPage.agent;
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: FocusScope(
        autofocus: true,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            WindowDrag(
              child: Container(
                height: titleBarHeight,
                padding: const EdgeInsets.only(left: Space.xl, right: Space.m),
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
                          title: 'Providers',
                          children: [
                            for (final provider in workspace.providers)
                              _ProviderCard(
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

class _ProviderCard extends StatefulWidget {
  const _ProviderCard({required this.provider, required this.flow});

  final Provider provider;

  /// The sign-in in progress for this provider, if any.
  final LoginFlow? flow;

  @override
  State<_ProviderCard> createState() => _ProviderCardState();
}

class _ProviderCardState extends State<_ProviderCard> with SettingRequests {
  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scope = AppScope.scope(context);
    final provider = widget.provider;
    final flow = widget.flow;
    final signedIn = provider.signedInWith.isNotEmpty;
    var methodLabel = flow?.methodId ?? '';
    for (final method in provider.methods) {
      if (method.id == flow?.methodId) methodLabel = method.label;
    }
    return Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      provider.name,
                      style: TextStyles.label.copyWith(color: palette.text),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      signedIn
                          ? 'Signed in with ${provider.signedInWith}'
                          : 'Not signed in',
                      style: TextStyles.caption.copyWith(
                        color: signedIn ? palette.textMuted : palette.textFaint,
                      ),
                    ),
                  ],
                ),
              ),
              if (provider.canSignOut)
                Button(
                  label: 'Sign out',
                  onPressed: () =>
                      run(() => scope.backend.signOut(provider.id)),
                ),
            ],
          ),
          if (flow != null) ...[
            const SizedBox(height: Space.l),
            Container(height: 1, color: palette.hairline),
            const SizedBox(height: Space.l),
            LoginPanel(flow: flow, method: methodLabel),
          ] else if (provider.methods.isNotEmpty) ...[
            const SizedBox(height: Space.m),
            Wrap(
              spacing: Space.xs,
              runSpacing: Space.xs,
              children: [
                for (var i = 0; i < provider.methods.length; i++)
                  Button(
                    label: 'Sign in with ${provider.methods[i].label}',
                    primary: !signedIn && i == 0,
                    onPressed: () => run(
                      () => scope.login.start(
                        provider.id,
                        provider.methods[i].id,
                      ),
                    ),
                  ),
              ],
            ),
          ],
          failureLine(palette),
        ],
      ),
    );
  }
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
          label: 'Compact context at',
          child: Row(
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
            'Admin agents with the Resources extension, such as Rivet, are '
            'told when the machine runs low. Each agent\'s Resources panel '
            'shows how it stands.',
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

class SettingRow extends StatelessWidget {
  const SettingRow({super.key, required this.label, required this.child});

  final String label;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.m),
      child: Row(
        children: [
          SizedBox(
            width: 160,
            child: Text(
              label,
              style: TextStyles.body.copyWith(color: palette.textMuted),
            ),
          ),
          Expanded(
            child: Align(alignment: Alignment.centerLeft, child: child),
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
