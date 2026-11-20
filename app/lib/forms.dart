import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_name.dart';
import 'characters/character.dart';
import 'backend.dart';
import 'controls.dart';
import 'folder_browser.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'secrets.dart';
import 'settings_view.dart';
import 'state.dart';

/// Title, fields, an error line and the buttons. Cmd+Enter (or Ctrl+Enter)
/// submits from anywhere in the form; Esc cancels.
class FormFrame extends StatelessWidget {
  const FormFrame({
    super.key,
    required this.title,
    required this.fields,
    required this.error,
    required this.submitLabel,
    required this.onSubmit,
    required this.onCancel,
    this.body,
    this.secondary,
    this.cancelLabel = 'Cancel',
  });

  final String title;

  /// A sentence under the title.
  final String? body;
  final List<Widget> fields;
  final String? error;
  final String submitLabel;

  /// Null disables submitting.
  final VoidCallback? onSubmit;
  final VoidCallback onCancel;

  /// The cancel button's words, such as "Not now".
  final String cancelLabel;

  /// A less common action, on the left of the buttons (e.g. delete).
  final Widget? secondary;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    void submit() => onSubmit?.call();
    final phone = isPhoneLayout(context);
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.enter, meta: true): submit,
        const SingleActivator(LogicalKeyboardKey.enter, control: true): submit,
      },
      child: FocusTraversalGroup(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              title,
              style: TextStyles.emphasis.copyWith(color: palette.text),
            ),
            if (body != null) ...[
              const SizedBox(height: Space.s),
              Text(
                body!,
                style: TextStyles.body.copyWith(color: palette.textMuted),
              ),
            ],
            const SizedBox(height: Space.xl),
            // A long form scrolls between its title and its buttons.
            Flexible(
              child: SingleChildScrollView(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: fields,
                ),
              ),
            ),
            if (error != null)
              Padding(
                padding: const EdgeInsets.only(bottom: Space.m),
                child: Text(
                  error!,
                  style: TextStyles.caption.copyWith(color: palette.danger),
                ),
              ),
            Row(
              children: [
                if (!phone) ?secondary,
                const Spacer(),
                Button(label: cancelLabel, onPressed: onCancel),
                const SizedBox(width: Space.s),
                Button(label: submitLabel, onPressed: onSubmit, primary: true),
              ],
            ),
            // A phone is too narrow for three buttons in a row: the less
            // common action goes on a line of its own, under them.
            if (phone && secondary != null) ...[
              const SizedBox(height: Space.m),
              Align(alignment: Alignment.centerLeft, child: secondary),
            ],
          ],
        ),
      ),
    );
  }
}

/// Runs a request for a form: tracks that it's in flight, closes the form
/// when the backend accepts, and shows the backend's reason when it refuses.
mixin _Submitting<T extends StatefulWidget> on State<T> {
  var submitting = false;
  String? error;

  Future<void> submitRequest(Future<void> request, VoidCallback close) async {
    setState(() {
      submitting = true;
      error = null;
    });
    try {
      await request;
      close();
    } on RequestFailure catch (failure) {
      if (!mounted) return;
      setState(() {
        submitting = false;
        error = failure.message;
      });
    }
  }
}

class CreateProjectForm extends StatefulWidget {
  const CreateProjectForm({super.key, required this.close});

  final VoidCallback close;

  @override
  State<CreateProjectForm> createState() => _CreateProjectFormState();
}

class _CreateProjectFormState extends State<CreateProjectForm>
    with _Submitting {
  final _name = TextEditingController();

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  void _submit() {
    final name = _name.text.trim();
    if (name.isEmpty || submitting) return;
    final backend = AppScope.of(context);
    final history = AppScope.scope(context).history;
    final before = {
      for (final project in backend.workspace.workspace.projects) project.id,
    };
    // A new project opens focused, ready for its first agent.
    Future<void> createAndFocus() async {
      await backend.createProject(name);
      for (final project in backend.workspace.workspace.projects) {
        if (!before.contains(project.id)) {
          history.clear();
          return backend.focusProject(project.id);
        }
      }
    }

    submitRequest(createAndFocus(), widget.close);
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: _name,
      builder: (context, _) => FormFrame(
        title: 'New project',
        error: error,
        submitLabel: 'Create',
        onSubmit: _name.text.trim().isEmpty || submitting ? null : _submit,
        onCancel: widget.close,
        fields: [
          Field(
            label: 'Name',
            child: TextField(
              controller: _name,
              autofocus: true,
              onSubmitted: (_) => _submit(),
            ),
          ),
        ],
      ),
    );
  }
}

/// Renames or deletes a project, and keeps its folders: renames them,
/// points them elsewhere, removes them, or adds one (through Rivet, or a
/// short form when Rivet can't run).
class ProjectSettingsForm extends StatefulWidget {
  const ProjectSettingsForm({
    super.key,
    required this.project,
    required this.close,
  });

  final Project project;
  final VoidCallback close;

  @override
  State<ProjectSettingsForm> createState() => _ProjectSettingsFormState();
}

/// One of the project's folders as the form edits it.
class _FolderEdit {
  _FolderEdit(this.folder)
    : name = TextEditingController(text: folder.name),
      path = TextEditingController(text: folder.path);

  final ProjectFolder folder;
  final TextEditingController name;
  final TextEditingController path;
  var removed = false;

  bool get renamed => name.text.trim() != folder.name;
  bool get moved => path.text.trim() != folder.path;
  bool get changed => removed || renamed || moved;

  /// What's typed can be saved: a folder needs a name and a path.
  bool get valid =>
      removed || (name.text.trim().isNotEmpty && path.text.trim().isNotEmpty);

  void dispose() {
    name.dispose();
    path.dispose();
  }
}

class _ProjectSettingsFormState extends State<ProjectSettingsForm>
    with _Submitting {
  late final _name = TextEditingController(text: widget.project.name);
  late final _folders = [
    for (final folder in widget.project.folders) _FolderEdit(folder),
  ];
  var _confirmingDelete = false;

  /// Adding a folder with the form, as Rivet can't run.
  var _addingFolder = false;
  final _newName = TextEditingController();
  final _newPath = TextEditingController();

  @override
  void dispose() {
    _name.dispose();
    for (final folder in _folders) {
      folder.dispose();
    }
    _newName.dispose();
    _newPath.dispose();
    super.dispose();
  }

  bool get _changed =>
      _name.text.trim() != widget.project.name ||
      _folders.any((folder) => folder.changed);

  bool get _valid =>
      _name.text.trim().isNotEmpty && _folders.every((folder) => folder.valid);

  void _submit() {
    if (!_changed || !_valid || submitting) return;
    final backend = AppScope.of(context);
    final project = widget.project;
    final name = _name.text.trim();
    Future<void> save() async {
      if (name != project.name) {
        await backend.updateProject(
          UpdateProject(projectId: project.id, name: name),
        );
      }
      for (final edit in _folders) {
        if (edit.removed) {
          await backend.deleteFolder(project.id, edit.folder.id);
        } else if (edit.renamed || edit.moved) {
          final update = UpdateFolder(
            projectId: project.id,
            folderId: edit.folder.id,
          );
          if (edit.renamed) update.name = edit.name.text.trim();
          if (edit.moved) update.path = edit.path.text.trim();
          await backend.updateFolder(update);
        }
      }
    }

    submitRequest(save(), widget.close);
  }

  /// Rivet adds the folder, as the person describes it; without a Rivet
  /// that can run, the form does.
  void _addFolder() {
    if (askRivet(context, 'Add a folder to ${widget.project.name} for ')) {
      widget.close();
    } else {
      setState(() {
        _addingFolder = true;
        error = null;
      });
    }
  }

  void _createFolder() {
    final name = _newName.text.trim();
    if (name.isEmpty || submitting) return;
    submitRequest(
      AppScope.of(context).createFolder(
        CreateFolder(
          projectId: widget.project.id,
          name: name,
          path: _newPath.text.trim(),
        ),
      ),
      widget.close,
    );
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final project = widget.project;
    if (_confirmingDelete) {
      final agents = backend.workspace.agentsIn(project.id).length;
      return FormFrame(
        title: 'Delete ${project.name}?',
        body: [
          if (agents == 1) 'Its agent and its conversation are deleted.',
          if (agents > 1)
            'Its $agents agents and their conversations are deleted.',
          "The project's folder and its files are kept.",
        ].join(' '),
        error: error,
        submitLabel: 'Delete project',
        onSubmit: submitting
            ? null
            : () => submitRequest(
                backend.deleteProject(project.id),
                widget.close,
              ),
        onCancel: () => setState(() {
          _confirmingDelete = false;
          error = null;
        }),
        fields: const [],
      );
    }
    if (_addingFolder) {
      return ListenableBuilder(
        listenable: Listenable.merge([_newName, _newPath]),
        builder: (context, _) => FormFrame(
          title: 'New folder in ${project.name}',
          error: error,
          submitLabel: 'Add folder',
          onSubmit: _newName.text.trim().isEmpty || submitting
              ? null
              : _createFolder,
          onCancel: widget.close,
          fields: [
            Field(
              label: 'Name',
              child: TextField(
                controller: _newName,
                autofocus: true,
                placeholder: 'Research',
                onSubmitted: (_) => _createFolder(),
              ),
            ),
            Field(
              label: 'Path',
              note:
                  'Leave empty to make one inside the project, named after '
                  'it.',
              child: _PathField(
                controller: _newPath,
                placeholder: '${project.path}/…',
                from: project.path,
                onSubmit: _createFolder,
              ),
            ),
          ],
        ),
      );
    }
    return ListenableBuilder(
      listenable: Listenable.merge([
        _name,
        for (final folder in _folders) ...[folder.name, folder.path],
      ]),
      builder: (context, _) {
        return FormFrame(
          title: 'Project settings',
          error: error,
          submitLabel: 'Save',
          onSubmit: !_changed || !_valid || submitting ? null : _submit,
          onCancel: widget.close,
          secondary: Button(
            label: 'Delete project…',
            onPressed: () => setState(() {
              _confirmingDelete = true;
              error = null;
            }),
          ),
          fields: [
            Field(
              label: 'Name',
              child: TextField(
                controller: _name,
                autofocus: true,
                onSubmitted: (_) => _submit(),
              ),
            ),
            Field(
              label: 'Folders',
              note:
                  "Agents work in the project's folder or in one of these. "
                  'Removing a folder keeps its files and moves its agents to '
                  "the project's folder.",
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  for (final edit in _folders)
                    if (!edit.removed)
                      Padding(
                        padding: const EdgeInsets.only(bottom: Space.m),
                        child: _FolderRow(
                          edit: edit,
                          onRemove: () => setState(() => edit.removed = true),
                          onSubmit: _submit,
                        ),
                      ),
                  Align(
                    alignment: Alignment.centerLeft,
                    child: Button(
                      label: 'Add a folder…',
                      onPressed: _addFolder,
                    ),
                  ),
                ],
              ),
            ),
          ],
        );
      },
    );
  }
}

/// A folder in the project's settings: its name and a × that removes it,
/// over its path.
class _FolderRow extends StatelessWidget {
  const _FolderRow({
    required this.edit,
    required this.onRemove,
    required this.onSubmit,
  });

  final _FolderEdit edit;
  final VoidCallback onRemove;
  final VoidCallback onSubmit;

  @override
  Widget build(BuildContext context) {
    // The name, with the ×, over the path: the form is too narrow for one
    // line.
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: edit.name,
                placeholder: 'Name',
                onSubmitted: (_) => onSubmit(),
              ),
            ),
            const SizedBox(width: Space.xs),
            GlyphButton(
              label: 'Remove ${edit.folder.name}',
              active: false,
              onTap: onRemove,
              glyph: (color) => CloseGlyph(color: color),
            ),
          ],
        ),
        const SizedBox(height: Space.xs),
        Padding(
          // Lined up with the name, clear of the ×.
          padding: const EdgeInsets.only(right: 28 + Space.xs),
          child: _PathField(
            controller: edit.path,
            placeholder: 'Path',
            from: edit.folder.path,
            onSubmit: onSubmit,
          ),
        ),
      ],
    );
  }
}

/// A folder's absolute path: typed, or chosen. On this Mac, Choose… opens
/// the system's folder sheet; on a Factory on another machine the path is
/// that machine's, so it opens a browser of its folders instead.
class _PathField extends StatelessWidget {
  const _PathField({
    required this.controller,
    required this.placeholder,
    required this.from,
    required this.onSubmit,
  });

  final TextEditingController controller;
  final String placeholder;

  /// Where choosing starts when nothing is typed.
  final String from;
  final VoidCallback onSubmit;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final remote = backend.target;
    final machine = backend.workspace.workspace.remote.machine.isNotEmpty
        ? backend.workspace.workspace.remote.machine
        : remote?.label ?? '';
    final row = Row(
      children: [
        Expanded(
          child: TextField(
            controller: controller,
            placeholder: placeholder,
            onSubmitted: (_) => onSubmit(),
          ),
        ),
        const SizedBox(width: Space.xs),
        Button(
          label: 'Choose…',
          onPressed: () async {
            final typed = controller.text.trim();
            final start = typed.isEmpty ? from : typed;
            final chosen = remote == null
                ? await pickFolder(from: start)
                : await chooseRemoteFolder(
                    context,
                    backend: backend,
                    from: start,
                    machine: machine,
                  );
            if (chosen != null) controller.text = chosen;
          },
        ),
      ],
    );
    if (remote == null) return row;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        row,
        const SizedBox(height: Space.xs),
        Text(
          'A path on $machine, where this Claktory runs.',
          style: TextStyles.caption.copyWith(color: palette.textFaint),
        ),
      ],
    );
  }
}

/// Model choices: models that can run now, plus [current] if it can't, shown
/// as what it is (no model). [defaultLabel] names the Factory default when it
/// may be chosen.
List<SelectOption<String>> _modelOptions(
  Workspace workspace,
  String current, {
  String? defaultLabel,
}) {
  final options = [if (defaultLabel != null) SelectOption('', defaultLabel)];
  var found = defaultLabel != null && current.isEmpty;
  for (final model in workspace.models) {
    options.add(SelectOption(model.id, modelLabel(model)));
    if (model.id == current) found = true;
  }
  if (!found) options.add(SelectOption(current, 'No model available'));
  return options;
}

List<SelectOption<String>> _effortOptions(
  Workspace workspace,
  String model,
  String current, {
  String? defaultLabel,
}) {
  final options = [if (defaultLabel != null) SelectOption('', defaultLabel)];
  var found = defaultLabel != null && current.isEmpty;
  for (final candidate in workspace.models) {
    if (candidate.id != model) continue;
    for (final effort in candidate.efforts) {
      options.add(SelectOption(effort, effortLabel(effort)));
      if (effort == current) found = true;
    }
  }
  if (!found) {
    options.add(
      SelectOption(current, current.isEmpty ? 'Not set' : effortLabel(current)),
    );
  }
  return options;
}

/// The effort to keep when the model changes: the same one if the new model
/// accepts it. Otherwise the default when one may be chosen, or else the
/// Factory default effort or the model's strongest, so it's always valid.
String _effortFor(
  Workspace workspace,
  String model,
  String effort, {
  required bool offerDefault,
}) {
  for (final candidate in workspace.models) {
    if (candidate.id != model) continue;
    if (candidate.efforts.contains(effort)) return effort;
    if (offerDefault || candidate.efforts.isEmpty) return '';
    final fallback = workspace.settings.defaultEffort;
    return candidate.efforts.contains(fallback)
        ? fallback
        : candidate.efforts.last;
  }
  return '';
}

/// "Default (Opus 5.5)": what the Factory default actually is right now.
String _defaultModelLabel(Workspace workspace) {
  for (final model in workspace.models) {
    if (model.id == workspace.settings.defaultModel) {
      return 'Default (${modelLabel(model)})';
    }
  }
  return 'Default (no model available)';
}

String _defaultEffortLabel(Workspace workspace) {
  final effort = workspace.settings.defaultEffort;
  return effort.isEmpty ? 'Default' : 'Default (${effortLabel(effort)})';
}

/// A reasoning effort as the person reads it: "xhigh" is "Extra high".
String effortLabel(String effort) => switch (effort) {
  'xhigh' => 'Extra high',
  '' => '',
  _ => effort[0].toUpperCase() + effort.substring(1),
};

/// Shared fields of the create and settings forms.
class _AgentFields extends StatelessWidget {
  const _AgentFields({
    required this.workspace,
    required this.name,
    required this.role,
    required this.prompt,
    required this.project,
    required this.folder,
    required this.model,
    required this.effort,
    required this.color,
    required this.onProject,
    required this.onFolder,
    required this.onModel,
    required this.onEffort,
    required this.onColor,
    required this.onSubmit,
    required this.offerDefault,
    required this.offerWorkspace,
    this.outsideLabel = 'Workspace',
    required this.close,
  });

  final Workspace workspace;
  final TextEditingController name;

  /// What it does, shown under its name; may be left empty.
  final TextEditingController role;
  final TextEditingController prompt;
  final String project;

  /// Which of the project's folders it works in; empty for the project's
  /// own folder.
  final String folder;
  final String model;
  final String effort;
  final String color;
  final ValueChanged<String> onProject;
  final ValueChanged<String> onFolder;
  final ValueChanged<String> onModel;
  final ValueChanged<String> onEffort;
  final ValueChanged<String> onColor;
  final VoidCallback onSubmit;

  /// Offer "Default" (the Factory default) for model and effort.
  final bool offerDefault;

  /// Offer the workspace, outside any project, as its place. New agents go
  /// in projects; an existing one can still move beside Rivet.
  final bool offerWorkspace;

  /// What to call the place outside any project: "Admin" for an admin
  /// agent, else "Workspace".
  final String outsideLabel;

  /// Closes the form, for leaving it to sign in.
  final VoidCallback close;

  /// The chosen project's folders.
  List<ProjectFolder> get _folders => [
    for (final candidate in workspace.projects)
      if (candidate.id == project) ...candidate.folders,
  ];

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final usesDefault = offerDefault && model.isEmpty;
    final effortModel = usesDefault ? workspace.settings.defaultModel : model;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Field(
          label: 'Name',
          child: TextField(
            controller: name,
            autofocus: true,
            onSubmitted: (_) => onSubmit(),
          ),
        ),
        Field(
          label: 'Role',
          note: 'What it does, shown under its name. Optional.',
          child: TextField(
            controller: role,
            placeholder: 'Such as Coordinator or Writer',
            onSubmitted: (_) => onSubmit(),
          ),
        ),
        Field(
          label: 'Color',
          child: _ColorPicker(value: color, onChanged: onColor),
        ),
        Field(
          label: 'Prompt',
          child: TextField(
            controller: prompt,
            placeholder: 'What this agent does and how',
            minLines: 4,
            maxLines: 10,
          ),
        ),
        if (workspace.projects.isNotEmpty)
          Field(
            label: 'Project',
            child: Select(
              value: project,
              options: [
                if (offerWorkspace)
                  SelectOption('', '$outsideLabel (no project)'),
                for (final candidate in workspace.projects)
                  SelectOption(candidate.id, candidate.name),
              ],
              onChanged: onProject,
            ),
          ),
        if (_folders.isNotEmpty)
          Field(
            label: 'Folder',
            note: 'Where it works: one of the project\'s folders.',
            child: Select(
              value: _folders.any((candidate) => candidate.id == folder)
                  ? folder
                  : '',
              options: [
                const SelectOption('', "The project's folder"),
                for (final candidate in _folders)
                  SelectOption(candidate.id, candidate.name),
              ],
              onChanged: onFolder,
            ),
          ),
        if (workspace.models.isEmpty)
          Field(
            label: 'Model',
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    'No model available',
                    style: TextStyles.body.copyWith(color: palette.textMuted),
                  ),
                ),
                Button(
                  label: 'Sign in…',
                  onPressed: () {
                    close();
                    AppScope.openSettings(context);
                  },
                ),
              ],
            ),
          )
        else
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                flex: 3,
                child: Field(
                  label: 'Model',
                  child: Select(
                    value: model,
                    options: _modelOptions(
                      workspace,
                      model,
                      defaultLabel: offerDefault
                          ? _defaultModelLabel(workspace)
                          : null,
                    ),
                    onChanged: onModel,
                  ),
                ),
              ),
              const SizedBox(width: Space.m),
              Expanded(
                flex: 2,
                child: Field(
                  label: 'Reasoning effort',
                  child: Select(
                    value: effort,
                    options: _effortOptions(
                      workspace,
                      effortModel,
                      effort,
                      defaultLabel: offerDefault
                          ? _defaultEffortLabel(workspace)
                          : null,
                    ),
                    onChanged: onEffort,
                  ),
                ),
              ),
            ],
          ),
      ],
    );
  }
}

/// The agent colors as swatches; the chosen one is ringed.
class _ColorPicker extends StatelessWidget {
  const _ColorPicker({required this.value, required this.onChanged});

  final String value;
  final ValueChanged<String> onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Wrap(
      spacing: Space.s,
      runSpacing: Space.s,
      children: [
        for (final MapEntry(key: name, value: color) in agentColors.entries)
          Semantics(
            key: ValueKey('color-$name'),
            button: true,
            selected: name == value,
            label: name,
            child: Pressable(
              onTap: () => onChanged(name),
              builder: (context, hovered) => AnimatedContainer(
                duration: Motion.fast,
                width: 26,
                height: 26,
                padding: const EdgeInsets.all(3),
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  border: Border.all(
                    width: 2,
                    color: name == value
                        ? palette.text
                        : hovered
                        ? palette.textFaint
                        : const Color(0x00000000),
                  ),
                ),
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    color: color,
                    shape: BoxShape.circle,
                  ),
                ),
              ),
            ),
          ),
      ],
    );
  }
}

/// The color a new agent gets unless the person picks another; Factory
/// picks the same way.
/// The color Factory gives a new agent: the one least like the others'.
String nextAgentColor(Workspace workspace) => workspace.nextAgentColor.isEmpty
    ? agentColors.keys.first
    : workspace.nextAgentColor;

class CreateAgentForm extends StatefulWidget {
  const CreateAgentForm({
    super.key,
    required this.projectId,
    required this.close,
  });

  /// The project it's created in. New agents always go in a project.
  final String projectId;
  final VoidCallback close;

  @override
  State<CreateAgentForm> createState() => _CreateAgentFormState();
}

class _CreateAgentFormState extends State<CreateAgentForm> with _Submitting {
  final _name = TextEditingController();
  final _role = TextEditingController();
  final _prompt = TextEditingController();
  late var _project = widget.projectId;
  var _folder = '';
  var _model = '';
  var _effort = '';
  String? _color;

  @override
  void dispose() {
    _name.dispose();
    _role.dispose();
    _prompt.dispose();
    super.dispose();
  }

  void _submit() {
    final name = _name.text.trim();
    if (name.isEmpty || submitting) return;
    final backend = AppScope.of(context);
    submitRequest(
      backend.createAgent(
        CreateAgent(
          projectId: _project,
          folderId: _folder,
          name: name,
          role: _role.text.trim(),
          prompt: _prompt.text.trim(),
          model: _model,
          effort: _effort,
          color: _color ?? nextAgentColor(backend.workspace.workspace),
        ),
      ),
      widget.close,
    );
  }

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: Listenable.merge([store, _name]),
      builder: (context, _) {
        final workspace = store.workspace;
        return FormFrame(
          title: 'New agent',
          error: error,
          submitLabel: 'Create',
          onSubmit: _name.text.trim().isEmpty || submitting ? null : _submit,
          onCancel: widget.close,
          fields: [
            _AgentFields(
              workspace: workspace,
              name: _name,
              role: _role,
              prompt: _prompt,
              project: _project,
              folder: _folder,
              model: _model,
              effort: _effort,
              color: _color ?? nextAgentColor(workspace),
              onProject: (id) => setState(() {
                _project = id;
                _folder = '';
              }),
              onFolder: (id) => setState(() => _folder = id),
              onModel: (model) => setState(() {
                _model = model;
                _effort = _effortFor(
                  workspace,
                  model,
                  _effort,
                  offerDefault: true,
                );
              }),
              onEffort: (effort) => setState(() => _effort = effort),
              onColor: (color) => setState(() => _color = color),
              onSubmit: _submit,
              offerDefault: true,
              offerWorkspace: false,
              close: widget.close,
            ),
          ],
        );
      },
    );
  }
}

/// Edits an agent's name, color, prompt, project, model and effort, sending only
/// what changed, or deletes the agent.
class AgentSettingsForm extends StatefulWidget {
  const AgentSettingsForm({
    super.key,
    required this.agent,
    required this.close,
  });

  final Agent agent;
  final VoidCallback close;

  @override
  State<AgentSettingsForm> createState() => _AgentSettingsFormState();
}

class _AgentSettingsFormState extends State<AgentSettingsForm>
    with _Submitting {
  late final _name = TextEditingController(text: widget.agent.name);
  late final _role = TextEditingController(text: widget.agent.role);
  late final _prompt = TextEditingController(text: widget.agent.prompt);
  late final _placeholder = TextEditingController(
    text: widget.agent.placeholder,
  );
  late var _project = widget.agent.projectId;
  late var _folder = widget.agent.folderId;
  late var _model = widget.agent.model;
  late var _effort = widget.agent.effort;
  late var _color = widget.agent.color;

  /// The shape chosen here; empty for automatic.
  late var _shape = _shapeChoice(widget.agent);
  var _confirmingDelete = false;

  /// Extensions turned on or off here, by id, until saved.
  final _enabled = <String, bool>{};

  /// Extension options changed here, by extension id then option id.
  final _options = <String, Map<String, OptionValue>>{};

  @override
  void dispose() {
    _name.dispose();
    _role.dispose();
    _prompt.dispose();
    _placeholder.dispose();
    super.dispose();
  }

  UpdateAgent? _changes() {
    final agent = widget.agent;
    final update = UpdateAgent(agentId: agent.id);
    final name = _name.text.trim();
    final prompt = _prompt.text.trim();
    if (name != agent.name) update.name = name;
    final role = _role.text.trim();
    if (role != agent.role) update.role = role;
    if (prompt != agent.prompt) update.prompt = prompt;
    if (_project != agent.projectId) update.projectId = _project;
    // Moving to another project leaves its folder; the backend clears it.
    if (_folder != agent.folderId &&
        (_folder.isNotEmpty || _project == agent.projectId)) {
      update.folderId = _folder;
    }
    if (_model != agent.model) update.model = _model;
    if (_effort != agent.effort) update.effort = _effort;
    if (_color != agent.color) update.color = _color;
    if (_shape != _shapeChoice(agent)) update.avatarShape = _shape;
    // One hint, or examples one per line, without blank lines.
    final placeholder = placeholderLines(_placeholder.text).join('\n');
    if (placeholder != agent.placeholder) update.placeholder = placeholder;
    final changed =
        update.hasName() ||
        update.hasRole() ||
        update.hasPrompt() ||
        update.hasProjectId() ||
        update.hasModel() ||
        update.hasEffort() ||
        update.hasColor() ||
        update.hasAvatarShape() ||
        update.hasPlaceholder() ||
        update.hasFolderId();
    return changed ? update : null;
  }

  /// [agent] as it would look saved: its color and shape as picked here.
  /// Automatic keeps the shape it has now.
  Agent _previewed(Agent agent) {
    final previewed = agent.deepCopy()..color = _color;
    if (shapeNamed(_shape) != null) previewed.avatarShape = _shape;
    return previewed;
  }

  /// The agent as the backend has it now; the form compares extension
  /// changes against it, so a partly saved form doesn't resend.
  Agent get _current =>
      AppScope.of(context).workspace.agent(widget.agent.id) ?? widget.agent;

  /// The agent as it would be saved: the backend's, with the extensions
  /// turned on or off and the options changed here.
  Agent get _edited {
    final edited = _current.deepCopy();
    for (final extension in edited.extensions) {
      final enabled = _enabled[extension.id];
      if (enabled != null) extension.enabled = enabled;
      extension.options.addAll(_options[extension.id] ?? const {});
    }
    return edited;
  }

  /// One change per extension turned on or off, and per option changed.
  List<ExtensionChange> _extensionChanges() {
    final changes = <ExtensionChange>[];
    for (final saved in _current.extensions) {
      final enabled = _enabled[saved.id];
      if (enabled != null && enabled != saved.enabled) {
        changes.add(ExtensionChange(id: saved.id, enabled: enabled));
      }
      final options = _options[saved.id] ?? const {};
      for (final MapEntry(key: optionId, :value) in options.entries) {
        if (!_sameValue(value, saved.options[optionId])) {
          changes.add(
            ExtensionChange(id: saved.id, optionId: optionId, value: value),
          );
        }
      }
    }
    return changes;
  }

  AgentExtension? _saved(String id) {
    for (final extension in _current.extensions) {
      if (extension.id == id) return extension;
    }
    return null;
  }

  bool get _changed => _changes() != null || _extensionChanges().isNotEmpty;

  void _submit() {
    if (!_changed || _name.text.trim().isEmpty || submitting) return;
    final backend = AppScope.of(context);
    final update = _changes();
    final changes = _extensionChanges();
    final id = widget.agent.id;
    // One extension change per request, as the protocol has it.
    Future<void> save() async {
      if (update != null) await backend.updateAgent(update);
      for (final change in changes) {
        await backend.updateAgent(
          UpdateAgent(agentId: id, extension_8: change),
        );
      }
    }

    submitRequest(save(), widget.close);
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final agent = widget.agent;
    if (_confirmingDelete) {
      return FormFrame(
        title: 'Delete ${agent.name}?',
        body: 'Its conversation is deleted too.',
        error: error,
        submitLabel: 'Delete agent',
        onSubmit: submitting
            ? null
            : () => submitRequest(backend.deleteAgent(agent.id), widget.close),
        onCancel: () => setState(() {
          _confirmingDelete = false;
          error = null;
        }),
        fields: const [],
      );
    }
    final store = backend.workspace;
    return ListenableBuilder(
      listenable: Listenable.merge([
        store,
        _name,
        _role,
        _prompt,
        _placeholder,
      ]),
      builder: (context, _) {
        final workspace = store.workspace;
        final canSave = !submitting && _name.text.trim().isNotEmpty && _changed;
        return FormFrame(
          title: 'Agent settings',
          error: error,
          submitLabel: 'Save',
          onSubmit: canSave ? _submit : null,
          onCancel: widget.close,
          secondary: Button(
            label: 'Delete agent…',
            onPressed: () => setState(() {
              _confirmingDelete = true;
              error = null;
            }),
          ),
          fields: [
            Field(
              label: 'Character',
              child: CharacterPreview(
                // As it will look: its color and shape as picked, not
                // yet saved. Automatic shows the one it has until saved:
                // only Factory knows which it picks.
                agent: _previewed(agent),
              ),
            ),
            Field(
              label: 'Shape',
              note:
                  'Automatic keeps the same shape when you rename or move '
                  'the agent.',
              child: _AvatarPicker(
                value: _shape,
                options: [
                  for (final shape in CharacterShape.values)
                    (
                      shape.name,
                      _capitalized(shape.name),
                      specOf(_previewed(agent)..avatarShape = shape.name),
                    ),
                ],
                onChanged: (shape) => setState(() => _shape = shape),
              ),
            ),
            _AgentFields(
              workspace: workspace,
              name: _name,
              role: _role,
              prompt: _prompt,
              project: _project,
              folder: _folder,
              model: _model,
              effort: _effort,
              color: _color,
              onProject: (id) => setState(() {
                _project = id;
                _folder = '';
              }),
              onFolder: (id) => setState(() => _folder = id),
              onModel: (model) => setState(() {
                _model = model;
                _effort = _effortFor(
                  workspace,
                  model,
                  _effort,
                  offerDefault: false,
                );
              }),
              onEffort: (effort) => setState(() => _effort = effort),
              onColor: (color) => setState(() => _color = color),
              onSubmit: _submit,
              offerDefault: false,
              offerWorkspace: true,
              // As the backend has it now, with what's changed here: the
              // admin option turned on or off makes it an admin agent.
              outsideLabel: outsideProjectsLabel(_edited),
              close: widget.close,
            ),
            Field(
              label: 'Message box hint',
              note:
                  'Shown in the empty message box. Put several on separate '
                  'lines to rotate them.',
              child: TextField(
                controller: _placeholder,
                minLines: 1,
                maxLines: 8,
                // The default, shown while it's left empty.
                placeholder:
                    'Message ${_name.text.trim().isEmpty ? agent.name : _name.text.trim()}',
              ),
            ),
            if (workspace.extensions.isNotEmpty)
              Field(
                label: 'Extensions',
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    for (final extension in workspace.extensions)
                      if (_saved(extension.id) case final saved?)
                        _ExtensionSettings(
                          extension: extension,
                          workspace: workspace,
                          agentId: agent.id,
                          projectId: _project,
                          enabled: _enabled[extension.id] ?? saved.enabled,
                          value: (option) =>
                              _options[extension.id]?[option] ??
                              saved.options[option] ??
                              OptionValue(),
                          onEnabled: (on) =>
                              setState(() => _enabled[extension.id] = on),
                          onOption: (option, value) => setState(
                            () =>
                                (_options[extension.id] ??= {})[option] = value,
                          ),
                        ),
                  ],
                ),
              ),
            AgentSecretsField(agent: agent),
          ],
        );
      },
    );
  }
}

/// Whether two option values are the same: the switch, and the same agents
/// in any order.
bool _sameValue(OptionValue a, OptionValue? b) {
  b ??= OptionValue();
  return a.on == b.on &&
      a.agentIds.toSet().containsAll(b.agentIds) &&
      b.agentIds.toSet().containsAll(a.agentIds);
}

/// One extension in an agent's settings: its switch, and while it's on, its
/// options, each drawn by its kind.
class _ExtensionSettings extends StatelessWidget {
  const _ExtensionSettings({
    required this.extension,
    required this.workspace,
    required this.agentId,
    required this.projectId,
    required this.enabled,
    required this.value,
    required this.onEnabled,
    required this.onOption,
  });

  final Extension extension;
  final Workspace workspace;
  final String agentId;

  /// The agent's project as the form has it.
  final String projectId;
  final bool enabled;
  final OptionValue Function(String optionId) value;
  final ValueChanged<bool> onEnabled;
  final void Function(String optionId, OptionValue value) onOption;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      margin: const EdgeInsets.only(bottom: Space.s),
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _SwitchRow(
            leading: ExtensionGlyph(
              icon: extension.icon,
              name: extension.name,
              color: enabled ? palette.text : palette.textMuted,
            ),
            label: extension.name,
            description: extension.description,
            strong: true,
            value: enabled,
            onChanged: onEnabled,
          ),
          if (enabled)
            for (final option in extension.options)
              Padding(
                padding: const EdgeInsets.only(
                  top: Space.m,
                  left: Space.l + Space.s,
                ),
                child: switch (option.kind) {
                  ExtensionOptionKind.EXTENSION_OPTION_KIND_AGENTS =>
                    _AgentListOption(
                      option: option,
                      workspace: workspace,
                      agentId: agentId,
                      projectId: projectId,
                      chosen: value(option.id).agentIds,
                      onChanged: (ids) =>
                          onOption(option.id, OptionValue(agentIds: ids)),
                    ),
                  ExtensionOptionKind.EXTENSION_OPTION_KIND_SWITCH =>
                    _SwitchRow(
                      label: option.label,
                      description: option.description,
                      value: value(option.id).on,
                      onChanged: (on) =>
                          onOption(option.id, OptionValue(on: on)),
                    ),
                  // A kind from a newer backend: say so rather than guess.
                  _ => Text(
                    '${option.label}: needs a newer Claktory',
                    style: TextStyles.caption.copyWith(
                      color: palette.textFaint,
                    ),
                  ),
                },
              ),
        ],
      ),
    );
  }
}

/// A label and its description beside a switch.
class _SwitchRow extends StatelessWidget {
  const _SwitchRow({
    required this.label,
    required this.description,
    required this.value,
    required this.onChanged,
    this.leading,
    this.strong = false,
  });

  final String label;
  final String description;
  final bool value;
  final ValueChanged<bool> onChanged;
  final Widget? leading;

  /// The extension itself rather than one of its options.
  final bool strong;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (leading != null) ...[
          Padding(padding: const EdgeInsets.only(top: 2), child: leading),
          const SizedBox(width: Space.s),
        ],
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                label,
                style: (strong ? TextStyles.label : TextStyles.body).copyWith(
                  color: palette.text,
                ),
              ),
              if (description.isNotEmpty)
                Text(
                  description,
                  style: TextStyles.caption.copyWith(color: palette.textMuted),
                ),
            ],
          ),
        ),
        const SizedBox(width: Space.m),
        Padding(
          padding: const EdgeInsets.only(top: 2),
          child: Toggle(value: value, onChanged: onChanged, label: label),
        ),
      ],
    );
  }
}

/// An option that's a list of agents: the chosen ones as chips, and a menu
/// of every other agent, grouped by where they live, to choose from. With
/// `includes_project`, the agents in its own project are always in, so
/// they show checked and greyed.
class _AgentListOption extends StatefulWidget {
  const _AgentListOption({
    required this.option,
    required this.workspace,
    required this.agentId,
    required this.projectId,
    required this.chosen,
    required this.onChanged,
  });

  final ExtensionOption option;
  final Workspace workspace;
  final String agentId;
  final String projectId;
  final List<String> chosen;
  final ValueChanged<List<String>> onChanged;

  @override
  State<_AgentListOption> createState() => _AgentListOptionState();
}

class _AgentListOptionState extends State<_AgentListOption> {
  final _menu = OverlayPortalController();
  final _link = LayerLink();
  final _tapRegion = Object();
  var _hovered = false;

  /// Agents in the agent's own project, which count without being chosen.
  bool _inProject(Agent agent) =>
      widget.option.includesProject && agent.projectId == widget.projectId;

  void _toggle(Agent agent) {
    final chosen = widget.chosen.toSet();
    if (!chosen.remove(agent.id)) chosen.add(agent.id);
    // Kept in the sidebar's order.
    widget.onChanged([
      for (final other in widget.workspace.agents)
        if (chosen.contains(other.id)) other.id,
    ]);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final chosen = [
      for (final agent in widget.workspace.agents)
        if (widget.chosen.contains(agent.id) && agent.id != widget.agentId)
          agent,
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          widget.option.label,
          style: TextStyles.body.copyWith(color: palette.text),
        ),
        if (widget.option.description.isNotEmpty)
          Text(
            widget.option.description,
            style: TextStyles.caption.copyWith(color: palette.textMuted),
          ),
        const SizedBox(height: Space.xs + 2),
        TapRegion(
          groupId: _tapRegion,
          onTapOutside: (_) {
            if (_menu.isShowing) _menu.hide();
          },
          child: CompositedTransformTarget(
            link: _link,
            child: OverlayPortal(
              controller: _menu,
              overlayChildBuilder: _buildMenu,
              child: CallbackShortcuts(
                bindings: {
                  const SingleActivator(LogicalKeyboardKey.escape): () {
                    if (_menu.isShowing) _menu.hide();
                  },
                },
                child: MouseRegion(
                  cursor: SystemMouseCursors.click,
                  onEnter: (_) => setState(() => _hovered = true),
                  onExit: (_) => setState(() => _hovered = false),
                  child: GestureDetector(
                    onTap: () => _menu.isShowing ? _menu.hide() : _menu.show(),
                    child: Container(
                      constraints: const BoxConstraints(minHeight: 30),
                      padding: const EdgeInsets.symmetric(
                        horizontal: Space.s,
                        vertical: Space.xs,
                      ),
                      decoration: BoxDecoration(
                        color: _hovered ? palette.hover : palette.canvas,
                        borderRadius: BorderRadius.circular(Radii.control),
                        border: Border.all(color: palette.hairline),
                      ),
                      child: Row(
                        children: [
                          Expanded(
                            child: chosen.isEmpty
                                ? Text(
                                    widget.option.includesProject
                                        ? 'No one outside its project'
                                        : 'No one',
                                    style: TextStyles.body.copyWith(
                                      color: palette.textFaint,
                                    ),
                                  )
                                : Wrap(
                                    spacing: Space.xs,
                                    runSpacing: Space.xs,
                                    children: [
                                      for (final agent in chosen)
                                        _AgentChip(agent: agent),
                                    ],
                                  ),
                          ),
                          const SizedBox(width: Space.s),
                          Chevron(color: palette.textMuted, down: true),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildMenu(BuildContext context) {
    final palette = Palette.of(context);
    final workspace = widget.workspace;
    // Agents in no project first (admin agents, then the rest), then each
    // project, as in the sidebar.
    bool admin(Agent agent) => agent.projectId.isEmpty && isAdminAgent(agent);
    final places = <(String, String, bool Function(Agent))>[
      ('', 'Admin', admin),
      ('', 'Workspace', (agent) => agent.projectId.isEmpty && !admin(agent)),
      for (final project in workspace.projects)
        (project.id, project.name, (agent) => agent.projectId == project.id),
    ];
    final field = this.context.findRenderObject();
    final width = field is RenderBox && field.hasSize ? field.size.width : null;
    final rows = <Widget>[];
    for (final (id, name, inPlace) in places) {
      final agents = [
        for (final agent in workspace.agents)
          if (inPlace(agent) && agent.id != widget.agentId) agent,
      ];
      if (agents.isEmpty) continue;
      final own = widget.option.includesProject && id == widget.projectId;
      rows.add(
        Padding(
          padding: EdgeInsets.fromLTRB(
            Space.s,
            rows.isEmpty ? Space.xs : Space.s,
            Space.s,
            Space.xs,
          ),
          child: Text(
            own ? '$name · in its project' : name,
            style: TextStyles.caption.copyWith(
              color: palette.textFaint,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
      );
      for (final agent in agents) {
        final fixed = _inProject(agent);
        final on = fixed || widget.chosen.contains(agent.id);
        rows.add(
          Pressable(
            onTap: fixed
                ? null
                : () {
                    _toggle(agent);
                    // Rebuild the menu with the new choice.
                    setState(() {});
                  },
            builder: (context, hovered) => Container(
              height: 28,
              padding: const EdgeInsets.symmetric(horizontal: Space.s),
              decoration: BoxDecoration(
                color: hovered && !fixed ? palette.hover : null,
                borderRadius: BorderRadius.circular(Radii.control),
              ),
              child: Row(
                children: [
                  CheckMark(on: on, dim: fixed),
                  const SizedBox(width: Space.s),
                  AgentCharacter(agent: agent, size: 20),
                  const SizedBox(width: Space.s),
                  Expanded(
                    child: NameThenRole(
                      name: agent.name,
                      role: agent.role,
                      style: TextStyles.body.copyWith(
                        color: fixed ? palette.textMuted : palette.text,
                      ),
                      roleColor: palette.textFaint,
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      }
    }
    return CompositedTransformFollower(
      link: _link,
      targetAnchor: Alignment.bottomLeft,
      offset: const Offset(0, Space.xs),
      child: Align(
        alignment: Alignment.topLeft,
        child: TapRegion(
          groupId: _tapRegion,
          child: Container(
            width: width,
            constraints: const BoxConstraints(maxHeight: 280, minWidth: 200),
            padding: const EdgeInsets.all(Space.xs),
            decoration: BoxDecoration(
              color: palette.sidebar,
              borderRadius: BorderRadius.circular(Radii.panel),
              border: Border.all(color: palette.hairline),
              boxShadow: floatingShadow,
            ),
            child: rows.isEmpty
                ? Padding(
                    padding: const EdgeInsets.all(Space.s),
                    child: Text(
                      'No other agents.',
                      style: TextStyles.body.copyWith(color: palette.textFaint),
                    ),
                  )
                : ListView(shrinkWrap: true, children: rows),
          ),
        ),
      ),
    );
  }
}

/// A chosen agent: its dot and name in a small pill.
class _AgentChip extends StatelessWidget {
  const _AgentChip({required this.agent});

  final Agent agent;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      height: 26,
      padding: const EdgeInsets.only(left: Space.xs, right: Space.s),
      decoration: BoxDecoration(
        color: palette.selected,
        borderRadius: BorderRadius.circular(13),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          AgentCharacter(agent: agent, size: 20),
          const SizedBox(width: Space.xs + 2),
          Text(
            agent.name,
            style: TextStyles.caption.copyWith(color: palette.text),
          ),
        ],
      ),
    );
  }
}

/// Rivet: the workspace's agent with Messaging's admin option on, or else
/// the agent with id "rivet"; null when there's neither.
Agent? rivetIn(Workspace workspace) {
  for (final agent in workspace.agents) {
    if (agent.projectId.isEmpty && isAdminAgent(agent)) return agent;
  }
  for (final agent in workspace.agents) {
    if (agent.id == 'rivet') return agent;
  }
  return null;
}

/// Hands setting something up to Rivet: opens its conversation with
/// [starter] in its message box, after any draft it has, for the person to
/// finish and send. False, doing nothing, when there's no Rivet that can
/// run; the caller then offers its form instead.
bool askRivet(BuildContext context, String starter, {String? hint}) {
  final scope = AppScope.scope(context);
  final rivet = rivetIn(scope.backend.workspace.workspace);
  if (rivet == null || !rivet.modelAvailable) return false;
  scope.drafts.compose(rivet.id, starter);
  AppScope.closePage(context);
  fireAndLog(scope.backend.selectAgent(rivet.id));
  // Where to look next: Rivet's row, and what to do there.
  final highlights = scope.backend.highlights;
  highlights.glow('agent:${rivet.id}', [agentColor(rivet.color)]);
  highlights.say(
    'agent:${rivet.id}',
    hint ?? 'Tell ${rivet.name} what you want to add',
  );
  return true;
}

/// The shape chosen in [agent]'s settings; empty when it's automatic.
String _shapeChoice(Agent agent) =>
    agent.hasAvatarShapeOverride() ? agent.avatarShapeOverride : '';

String _capitalized(String id) => id[0].toUpperCase() + id.substring(1);

/// Automatic, or one of a character's shapes, each drawn at rest
/// in the agent's color, the chosen one ringed like a color swatch. A
/// choice this app doesn't know (from a newer Factory) rings nothing.
class _AvatarPicker extends StatelessWidget {
  const _AvatarPicker({
    required this.value,
    required this.options,
    required this.onChanged,
  });

  /// The chosen id; empty for automatic.
  final String value;

  /// Each choice: its id, its name and how it looks.
  final List<(String, String, CharacterSpec)> options;
  final ValueChanged<String> onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    Color ring(bool selected, bool hovered) => selected
        ? palette.text
        : hovered
        ? palette.textFaint
        : const Color(0x00000000);
    return Wrap(
      spacing: Space.s,
      runSpacing: Space.s,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        Semantics(
          button: true,
          selected: value.isEmpty,
          label: 'Automatic',
          excludeSemantics: true,
          child: Pressable(
            onTap: () => onChanged(''),
            builder: (context, hovered) => AnimatedContainer(
              duration: Motion.fast,
              height: 36,
              padding: const EdgeInsets.symmetric(horizontal: Space.m - 2),
              alignment: Alignment.center,
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(18),
                border: Border.all(
                  width: 2,
                  color: ring(value.isEmpty, hovered),
                ),
              ),
              child: Text(
                'Automatic',
                style: TextStyles.caption.copyWith(
                  color: value.isEmpty || hovered
                      ? palette.text
                      : palette.textMuted,
                ),
              ),
            ),
          ),
        ),
        for (final (id, name, spec) in options)
          Semantics(
            key: ValueKey('avatar-$id'),
            button: true,
            selected: id == value,
            label: name,
            excludeSemantics: true,
            child: Pressable(
              onTap: () => onChanged(id),
              builder: (context, hovered) => AnimatedContainer(
                duration: Motion.fast,
                width: 36,
                height: 36,
                padding: const EdgeInsets.all(2),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(Radii.control + 2),
                  border: Border.all(
                    width: 2,
                    color: ring(id == value, hovered),
                  ),
                ),
                child: StillCharacter(spec: spec, size: 28),
              ),
            ),
          ),
      ],
    );
  }
}

/// An agent's character at 80px, living, with its own mood now ("As now")
/// or any of the seven to see how it looks in each.
class CharacterPreview extends StatefulWidget {
  const CharacterPreview({super.key, required this.agent});

  final Agent agent;

  @override
  State<CharacterPreview> createState() => _CharacterPreviewState();
}

class _CharacterPreviewState extends State<CharacterPreview> {
  /// The sample mood shown; null for the agent's own.
  Mood? _mood;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    Widget chip(String label, Mood? mood) {
      final selected = mood == _mood;
      return Pressable(
        onTap: () => setState(() => _mood = mood),
        builder: (context, hovered) => AnimatedContainer(
          duration: Motion.fast,
          height: 22,
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: selected
                ? palette.selected
                : hovered
                ? palette.hover
                : null,
            borderRadius: BorderRadius.circular(11),
          ),
          child: Text(
            label,
            style: TextStyles.caption.copyWith(
              color: selected || hovered ? palette.text : palette.textMuted,
            ),
          ),
        ),
      );
    }

    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        AgentCharacter(agent: widget.agent, size: 80, live: true, mood: _mood),
        const SizedBox(width: Space.m),
        Expanded(
          child: Wrap(
            spacing: Space.xs,
            runSpacing: Space.xs,
            children: [
              chip('As now', null),
              for (final mood in Mood.values) chip(moodName(mood), mood),
            ],
          ),
        ),
      ],
    );
  }
}

/// A mood's name on its own, for the preview's samples.
String moodName(Mood mood) => switch (mood) {
  Mood.idle => 'Idle',
  Mood.thinking => 'Thinking',
  Mood.working => 'Tool running',
  Mood.news => 'News',
  Mood.failed => 'Failed',
  Mood.noModel => 'No model',
  Mood.asleep => 'Asleep',
};
