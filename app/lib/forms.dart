import 'dart:async';
import 'dart:io' show File;
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'agent_name.dart';
import 'characters/character.dart';
import 'backend.dart';
import 'explorer.dart' show shortPath;
import 'approve_elsewhere.dart' show approvesElsewhere, browserElsewhereText;
import 'controls.dart';
import 'decision.dart' show QuietLink;
import 'extension_about.dart' show hasExtensionView;
import 'folder_browser.dart';
import 'errands/browser_agent.dart'
    show
        browserDescription,
        browserExtension,
        browserRefusal,
        confirmBrowserOff;
import 'identity.dart';
import 'mobile/metrics.dart';
import 'model_select.dart' show ModelSelect;
import 'mobile/presentation.dart' show confirmDiscard, guardSheet;
import 'mobile/drawer.dart' show largeText;
import 'mobile/input.dart' show InputKind;
import 'mobile/platform_policy.dart';
import 'notification_settings.dart' show notifyModeOptions;
import 'pipe_reports.dart'
    show ForwardReportsField, StopReadingReportsField, containedKeeps;
import 'protected_places.dart'
    show protectedPathCaption, protectedRootOf, saveIntoProtectedPlaces;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'secrets.dart';
import 'pinned_agents.dart' show unpinnedOnArchive;
import 'pipes.dart'
    show
        ConnectPipeForm,
        PortalGlyph,
        openPipe,
        pipeColor,
        pipeLead,
        pipeLimitText,
        pipePeerName,
        pipeStatus,
        pipesIn,
        pipesLimiting;
import 'selection.dart' show SelectableArea, Unselectable;
import 'settings_view.dart';
import 'share_files.dart' show showInFinder;
import 'visible_codes.dart' show nameInText;
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
    this.points = const [],
    this.secondary,
    this.cancelLabel = 'Cancel',
    this.dirty = false,
  });

  /// It has changes that closing it would drop: on a phone, Cancel or
  /// dragging the sheet away asks first ([confirmDiscard]).
  final bool dirty;

  final String title;

  /// A sentence under the title.
  final String? body;

  /// A short list under the title (and [body]), one point each: what a
  /// confirm's action will do.
  final List<String> points;
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
    // Too narrow for three buttons in a row on a phone, and at the largest
    // text anywhere (an iPad's dialog).
    final apart = phone || largeText(context);
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.enter, meta: true): submit,
        const SingleActivator(LogicalKeyboardKey.enter, control: true): submit,
      },
      child: FocusTraversalGroup(
        child: phone && fields.isNotEmpty
            ? Builder(
                builder: (context) {
                  guardSheet(
                    context,
                    dirty ? () => confirmDiscard(context) : null,
                  );
                  return _phoneForm(context, palette);
                },
              )
            : phone
            ? _phoneDialog(context, palette)
            : Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(
                    title,
                    style: TypeScale.of(context).emphasis
                        .copyWith(color: palette.text),
                  ),
                  if (FormNote.of(context) case final note?) ...[
                    const SizedBox(height: Space.s),
                    note,
                  ],
                  if (body != null) ...[
                    const SizedBox(height: Space.s),
                    Text(
                      body!,
                      style: TypeScale.of(context).body
                          .copyWith(color: palette.textMuted),
                    ),
                  ],
                  if (points.isNotEmpty) ...[
                    const SizedBox(height: Space.s),
                    _Points(points),
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
                        style: TypeScale.of(context).caption
                            .copyWith(color: palette.danger),
                      ),
                    ),
                  // The less common action at the start, the buttons at the
                  // end; where their words don't fit beside it (longer
                  // words, larger text), it goes under them.
                  OverflowBar(
                    alignment: MainAxisAlignment.spaceBetween,
                    overflowAlignment: OverflowBarAlignment.end,
                    overflowDirection: VerticalDirection.up,
                    overflowSpacing: Space.m,
                    children: [
                      if (!apart && secondary != null)
                        secondary!
                      else
                        const SizedBox.shrink(),
                      Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Button(label: cancelLabel, onPressed: onCancel),
                          const SizedBox(width: Space.s),
                          Button(
                            label: submitLabel,
                            onPressed: onSubmit,
                            primary: true,
                          ),
                        ],
                      ),
                    ],
                  ),
                  // Where three buttons don't fit in a row, the less common
                  // action goes on a line of its own, under them.
                  if (apart && secondary != null) ...[
                    const SizedBox(height: Space.m),
                    Align(alignment: Alignment.centerLeft, child: secondary),
                  ],
                ],
              ),
      ),
    );
  }

  /// A dialog with nothing to fill in, on a phone (Reconnect, Remove this
  /// device?): its words scroll over its choices at the foot, stacked as a
  /// phone's sheet stacks them: the action full width, nearest the thumb,
  /// then [secondary] (such as Archive instead) and Cancel as links under
  /// it, never side by side, so no label is ever cut. At the largest text
  /// sizes they scroll with the words.
  Widget _phoneDialog(BuildContext context, Palette palette) {
    final type = TypeScale.of(context);
    final body = this.body;
    final error = this.error;
    final secondary = this.secondary;
    final large = largeText(context);
    final submit = Button(
      label: submitLabel,
      onPressed: onSubmit,
      primary: true,
    );
    final words = [
      Semantics(
        header: true,
        child: FittedHeading(
          title,
          style: type.emphasis.copyWith(color: palette.text),
        ),
      ),
      if (body != null) ...[
        const SizedBox(height: Space.s),
        Text(body, style: type.body.copyWith(color: palette.textMuted)),
      ],
      if (points.isNotEmpty) ...[
        const SizedBox(height: Space.s),
        _Points(points),
      ],
      if (error != null) ...[
        const SizedBox(height: Space.m),
        Text(error, style: type.secondary.copyWith(color: palette.danger)),
      ],
    ];
    final buttons = [
      const SizedBox(height: Space.xl),
      submit,
      const SizedBox(height: Space.s),
      // Centred under it, as the permission sheets' Not now.
      if (secondary != null) Center(child: secondary),
      Center(
        child: QuietLink(label: cancelLabel, onTap: onCancel),
      ),
    ];
    // At the largest text sizes even the buttons may not fit: all of it
    // scrolls together then.
    if (large) {
      return SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [...words, ...buttons],
        ),
      );
    }
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Flexible(
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: words,
            ),
          ),
        ),
        ...buttons,
      ],
    );
  }

  /// A form on a phone, as iOS lays one out in a sheet: Cancel at the
  /// start of its top row, the title in the middle and the action at the
  /// end, then the fields, scrolling under them. Why it was refused shows
  /// under the title, where the eye is after tapping the action.
  Widget _phoneForm(BuildContext context, Palette palette) {
    final type = TypeScale.of(context);
    final body = this.body;
    final error = this.error;
    final secondary = this.secondary;
    // In the bar, two lines at most; under it (the largest text sizes),
    // whole, however many lines it takes (Zucchero's row 507: "Can't
    // connect to …" lost the name).
    Widget heading(bool stacked) => Semantics(
      header: true,
      child: stacked
          ? FittedHeading(
              title,
              textAlign: TextAlign.center,
              style: type.emphasis.copyWith(color: palette.text),
            )
          : Text(
              title,
              textAlign: TextAlign.center,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: type.emphasis.copyWith(color: palette.text),
            ),
    );
    final cancel = _BarButton(
      label: cancelLabel,
      onTap: !dirty
          ? onCancel
          : () async {
              if (await confirmDiscard(context)) onCancel();
            },
    );
    final action = _BarButton(
      label: submitLabel,
      onTap: onSubmit,
      primary: true,
    );
    return LayoutBuilder(
      builder: (context, constraints) {
        // When Cancel or the action wouldn't fit in its quarter of the bar
        // ("I've saved it" on a small iPhone), or at the largest text sizes
        // the title wouldn't fit whole in two lines between them, the title
        // goes under them and each has half the bar, so no word is cut
        // short. Like iOS's, the title grows with the text only as far as
        // the bar does, in it or under it (Lucy: at AX5 it took a quarter
        // of the screen).
        final stacked =
            !_fitsBar(context, cancelLabel, constraints.maxWidth / 4) ||
            !_fitsBar(context, submitLabel, constraints.maxWidth / 4) ||
            (largeText(context) &&
                !_titleFitsBar(context, type, constraints.maxWidth / 2));
        return _phoneFormBody(
          context,
          palette,
          type,
          body: body,
          error: error,
          secondary: secondary,
          heading: heading(stacked),
          cancel: cancel,
          action: action,
          stacked: stacked,
        );
      },
    );
  }

  /// Whether the title fits whole in two lines [width] wide, at the bar's
  /// text size (clamped at ×1.5, as the bar is), never breaking a word.
  bool _titleFitsBar(BuildContext context, TypeScale type, double width) {
    final scaler = MediaQuery.textScalerOf(context).clamp(maxScaleFactor: 1.5);
    TextPainter laid(String text, {int? maxLines, double? maxWidth}) =>
        TextPainter(
          text: TextSpan(text: text, style: type.emphasis),
          textScaler: scaler,
          textDirection: TextDirection.ltr,
          maxLines: maxLines,
        )..layout(maxWidth: maxWidth ?? double.infinity);
    final whole = laid(title, maxLines: 2, maxWidth: width);
    var fits = !whole.didExceedMaxLines;
    whole.dispose();
    for (final word in title.split(RegExp(r'\s+'))) {
      if (!fits) break;
      final painter = laid(word);
      fits = painter.width <= width;
      painter.dispose();
    }
    return fits;
  }

  /// Whether [label] fits a bar button [width] wide, at the bar's text
  /// size (clamped at ×1.5, as the bar is).
  static bool _fitsBar(BuildContext context, String label, double width) {
    final scaler = MediaQuery.textScalerOf(context).clamp(maxScaleFactor: 1.5);
    final painter = TextPainter(
      text: TextSpan(
        text: label,
        style: PhoneType.label.copyWith(fontWeight: FontWeight.w600),
      ),
      textScaler: scaler,
      textDirection: TextDirection.ltr,
      maxLines: 1,
    )..layout();
    final fits = painter.width + 2 * Space.s <= width;
    painter.dispose();
    return fits;
  }

  Widget _phoneFormBody(
    BuildContext context,
    Palette palette,
    TypeScale type, {
    required String? body,
    required String? error,
    required Widget? secondary,
    required Widget heading,
    required Widget cancel,
    required Widget action,
    required bool stacked,
  }) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Like iOS's bar, its buttons grow with the text only so far.
        MediaQuery.withClampedTextScaling(
          maxScaleFactor: 1.5,
          // Stacked, each has half the bar.
          child: Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              Flexible(child: cancel),
              if (!stacked) Expanded(flex: 2, child: heading),
              Flexible(child: action),
            ],
          ),
        ),
        if (stacked)
          MediaQuery.withClampedTextScaling(
            maxScaleFactor: 1.5,
            child: heading,
          ),
        const SizedBox(height: Space.s),
        Flexible(
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                ?FormNote.of(context),
                if (body != null) ...[
                  Text(
                    body,
                    style: type.body.copyWith(color: palette.textMuted),
                  ),
                  const SizedBox(height: Space.s),
                ],
                if (error != null) ...[
                  // Over the fields: scrolled into sight as it comes, as
                  // the person may have scrolled down to the end.
                  _ScrolledTo(
                    key: ValueKey(error),
                    child: Text(
                      error,
                      style: type.secondary.copyWith(color: palette.danger),
                    ),
                  ),
                  const SizedBox(height: Space.s),
                ],
                const SizedBox(height: Space.s),
                ...fields,
                if (secondary != null)
                  Align(alignment: Alignment.centerLeft, child: secondary),
              ],
            ),
          ),
        ),
      ],
    );
  }
}

/// A title that wraps between words, never inside one: a word wider than
/// the line ("Reconnecting…" at the largest text size on a small iPhone)
/// is drawn just small enough to fit, as iOS shrinks a title, rather than
/// broken in two (Zucchero's row 507).
class FittedHeading extends StatelessWidget {
  const FittedHeading(
    this.text, {
    super.key,
    required this.style,
    this.textAlign = TextAlign.start,
  });

  final String text;
  final TextStyle style;
  final TextAlign textAlign;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final scaler = MediaQuery.textScalerOf(context);
      final room = constraints.maxWidth;
      var fitted = scaler;
      if (room.isFinite && room > 0) {
        final widest = _widestWord(context, scaler);
        if (widest > room) {
          final size = style.fontSize ?? 14;
          fitted = TextScaler.linear(scaler.scale(size) * room / widest / size);
        }
      }
      return Text(text, textAlign: textAlign, textScaler: fitted, style: style);
    },
  );

  double _widestWord(BuildContext context, TextScaler scaler) {
    var widest = 0.0;
    for (final word in text.split(RegExp(r'\s+'))) {
      if (word.isEmpty) continue;
      final painter = TextPainter(
        text: TextSpan(
          text: word,
          style: DefaultTextStyle.of(context).style.merge(style),
        ),
        textScaler: scaler,
        textDirection: Directionality.of(context),
        maxLines: 1,
      )..layout();
      // A hair over, so rounding never puts the last letter on a line of
      // its own.
      widest = math.max(widest, painter.width + 1);
      painter.dispose();
    }
    return widest;
  }
}

/// [child], scrolled into sight once it shows (a new one, by its key).
class _ScrolledTo extends StatefulWidget {
  const _ScrolledTo({super.key, required this.child, this.scroll = true});

  final Widget child;

  /// False: shown where it is, not scrolled to.
  final bool scroll;

  @override
  State<_ScrolledTo> createState() => _ScrolledToState();
}

class _ScrolledToState extends State<_ScrolledTo> {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !widget.scroll) return;
      Scrollable.ensureVisible(
        context,
        duration: MediaQuery.disableAnimationsOf(context)
            ? Duration.zero
            : Motion.base,
        curve: Curves.easeOut,
      );
    });
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

/// A form's Cancel or its action in a phone's form sheet: words in
/// `signal`, the action's in weight 600, in a 44pt target; faint while it
/// can't be done.
class _BarButton extends StatelessWidget {
  const _BarButton({
    required this.label,
    required this.onTap,
    this.primary = false,
  });

  final String label;
  final VoidCallback? onTap;
  final bool primary;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final enabled = onTap != null;
    return Semantics(
      container: true,
      button: true,
      enabled: enabled,
      label: label,
      onTap: onTap,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, pressed) => ConstrainedBox(
          // A finger's size: iOS's 44pt, Android's 48dp.
          constraints: BoxConstraints(
            minHeight: PhonePlatformPolicy.least(context, Touch.target),
            minWidth: PhonePlatformPolicy.least(context, Touch.target),
          ),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            child: Center(
              widthFactor: 1,
              // Wider than its half of the bar (Make video at the largest
              // text on a small iPhone, Sale's row 149): drawn just small
              // enough to fit, as iOS shrinks a bar button's title, never
              // cut short.
              child: FittedBox(
                fit: BoxFit.scaleDown,
                child: Text(
                  label,
                  maxLines: 1,
                  style: PhoneType.label.copyWith(
                    color: !enabled
                        ? palette.textFaint
                        : pressed
                        ? palette.signal.withValues(alpha: 0.6)
                        : palette.signal,
                    fontWeight: primary ? FontWeight.w600 : FontWeight.w400,
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Runs a request for a form: tracks that it's in flight, closes the form
/// when the backend accepts, and shows the backend's reason when it refuses.
/// A save the person set aside at a sheet that comes first.
class _SetAside implements Exception {
  const _SetAside();
}

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
    } on _SetAside {
      // The person set aside a sheet before anything was saved: the form
      // stays as it was.
      if (mounted) setState(() => submitting = false);
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

  /// The folder the name typed would get, as the backend answers
  /// (PreviewProjectFolder: its rule, never copied here); "" before any.
  var _folder = '';

  /// What was last asked, so an answer to an earlier name is dropped.
  var _asked = '';
  Timer? _typing;

  @override
  void initState() {
    super.initState();
    _name.addListener(_named);
  }

  @override
  void dispose() {
    _typing?.cancel();
    _name.dispose();
    super.dispose();
  }

  /// Asks where the name would go once typing pauses.
  void _named() {
    final name = _name.text.trim();
    if (name == _asked) return;
    _asked = name;
    _typing?.cancel();
    // The folder shown is only ever this name's.
    setState(() => _folder = '');
    if (name.isEmpty) return;
    _typing = Timer(const Duration(milliseconds: 250), () async {
      try {
        final preview = await AppScope.of(context).previewProjectFolder(name);
        if (mounted && preview.name == _asked) {
          setState(() => _folder = preview.path);
        }
      } on RequestFailure {
        // An older Claktory, or offline: no caption, never a guess.
        if (mounted && name == _asked) setState(() => _folder = '');
      }
    });
  }

  void _submit() {
    final name = _name.text.trim();
    if (name.isEmpty || submitting) return;
    // Created: where it would go no longer needs asking.
    _typing?.cancel();
    final backend = AppScope.of(context);
    final history = AppScope.scope(context).history;
    final before = {
      for (final project in backend.workspace.workspace.projects) project.id,
    };
    final focus = !phoneShowsAllProjects(context);
    // A new project opens focused, ready for its first agent; a phone
    // focuses none.
    Future<void> createAndFocus() async {
      await backend.createProject(name);
      if (!focus) return;
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
            // Where its files go (Lucy's projects-sidebar §6), on the
            // machine that runs the agents.
            note: _folder.isEmpty || _asked.isEmpty
                ? null
                : _folderCaption(context, _folder),
            child: TextField(
              controller: _name,
              autofocus: true,
              kind: InputKind.text,
              onSubmitted: (_) => _submit(),
            ),
          ),
        ],
      ),
    );
  }
}

/// "Folder: ~/.factory/workspace/bakery", or on a Claktory on another
/// machine "Folder on Mac Studio: ~/…" (Trish's words, §12).
String _folderCaption(BuildContext context, String path) {
  final backend = AppScope.of(context);
  final machine = _machineOf(backend);
  final where = machine == null ? 'Folder' : 'Folder on $machine';
  return '$where: ${shortPath(path)}';
}

/// The machine that runs the agents, when it isn't this one.
String? _machineOf(Backend backend) {
  if (backend.localBackend && backend.target == null) return null;
  final named = backend.workspace.workspace.remote.machine;
  if (named.isNotEmpty) return nameInText(named);
  final label = backend.target?.label ?? '';
  return label.isEmpty ? 'its computer' : nameInText(label);
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
  late final _brief = TextEditingController(text: widget.project.brief);
  late final _folders = [
    for (final folder in widget.project.folders) _FolderEdit(folder),
  ];
  var _confirmingDelete = false;

  /// Adding a folder with the form, as Rivet can't run.
  var _addingFolder = false;

  /// Connecting a pipe to another Claktory, in this modal's place.
  var _connectingPipe = false;
  final _newName = TextEditingController();
  final _newPath = TextEditingController();

  @override
  void dispose() {
    _name.dispose();
    _brief.dispose();
    for (final folder in _folders) {
      folder.dispose();
    }
    _newName.dispose();
    _newPath.dispose();
    super.dispose();
  }

  bool get _changed =>
      _name.text.trim() != widget.project.name ||
      _brief.text.trim() != widget.project.brief ||
      _folders.any((folder) => folder.changed);

  bool get _valid =>
      _name.text.trim().isNotEmpty && _folders.every((folder) => folder.valid);

  void _submit() {
    if (!_changed || !_valid || submitting) return;
    final backend = AppScope.of(context);
    final project = widget.project;
    final name = _name.text.trim();
    final brief = _brief.text.trim();
    // Folders moved into a place macOS protects get its sheet first.
    final moved = {
      for (final edit in _folders)
        if (!edit.removed && edit.moved)
          edit.path.text.trim(): edit.name.text.trim(),
    };
    Future<void> save() async {
      if (name != project.name || brief != project.brief) {
        final update = UpdateProject(projectId: project.id);
        if (name != project.name) update.name = name;
        if (brief != project.brief) update.brief = brief;
        await backend.updateProject(update);
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

    submitRequest(_saving(moved, save), widget.close);
  }

  /// [save], with the sheet first for [folders] (path to name) going into
  /// a place macOS protects.
  Future<void> _saving(
    Map<String, String> folders,
    Future<void> Function() save,
  ) async {
    if (!await saveIntoProtectedPlaces(context, folders: folders, save: save)) {
      throw const _SetAside();
    }
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
    final backend = AppScope.of(context);
    final path = _newPath.text.trim();
    submitRequest(
      _saving(
        {path: name},
        () => backend.createFolder(
          CreateFolder(projectId: widget.project.id, name: name, path: path),
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
      void archive() {
        if (submitting) return;
        submitRequest(
          backend.updateProject(
            UpdateProject(projectId: project.id, archived: true),
          ),
          widget.close,
        );
      }

      return FormFrame(
        title: 'Delete ${project.name}?',
        // Where the files stay, and which pipes close (Lucy's
        // projects-sidebar §7, Trish's words).
        body: deleteProjectBody(backend.workspace.workspace, project),
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
        // Nothing lost, and it can be unarchived: on a phone, the first
        // link under Delete project, as the safer way.
        secondary: isPhoneLayout(context)
            ? QuietLink(label: 'Archive instead', onTap: archive)
            : Button(
                label: 'Archive instead',
                onPressed: submitting ? null : archive,
              ),
        fields: const [],
      );
    }
    if (_connectingPipe) {
      return ConnectPipeForm(project: project, close: widget.close);
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
                kind: InputKind.text,
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
        _brief,
        for (final folder in _folders) ...[folder.name, folder.path],
      ]),
      builder: (context, _) {
        return FormFrame(
          title: 'Project settings',
          error: error,
          dirty: _changed,
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
            // Where its files are, first (Lucy's projects-sidebar §6).
            Field(
              label: 'Folder',
              child: ProjectFolderLine(path: project.path),
            ),
            Field(
              label: 'Name',
              child: TextField(
                controller: _name,
                // Not over a phone's settings: the keyboard would hide them.
                autofocus: !isPhoneLayout(context),
                kind: InputKind.text,
                onSubmitted: (_) => _submit(),
              ),
            ),
            Field(
              label: 'Brief',
              note:
                  'Optional. Every agent in this project has it in its '
                  'instructions.',
              child: TextField(
                controller: _brief,
                placeholder: 'The outcome, the team and the rules',
                semanticsLabel: 'Brief',
                kind: InputKind.multiline,
                minLines: 3,
                maxLines: 12,
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
            _pipesField(backend),
            // Where its new pipes' reports go, sent as it's chosen.
            ForwardReportsField(projectId: widget.project.id),
            _archiveField(backend),
          ],
        );
      },
    );
  }

  /// The project's pipes to other Claktories, each opening at a click, and
  /// Connect a pipe… to make a new one or use another's invite.
  Widget _pipesField(Backend backend) {
    final palette = Palette.of(context);
    final pipes = pipesIn(backend.workspace.workspace, widget.project.id);
    return Field(
      label: 'Pipes',
      note:
          "A pipe connects this project to someone else's Claktory, so your "
          "agents and theirs can talk. They see only the pipe.",
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final pipe in pipes)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.s),
              child: Pressable(
                onTap: () {
                  openPipe(context, pipe);
                  widget.close();
                },
                builder: (context, hovered) => Row(
                  children: [
                    const PortalGlyph(),
                    const SizedBox(width: Space.s),
                    Flexible(
                      child: Text(
                        pipe.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TypeScale.of(context).body.copyWith(
                          color: hovered ? palette.link : palette.text,
                        ),
                      ),
                    ),
                    const SizedBox(width: Space.s),
                    Expanded(
                      child: Text(
                        // A liaison waiting on its limit says so (#357 L1).
                        pipeStatus(
                          pipe,
                          lead: pipeLead(backend.workspace.workspace, pipe),
                        ),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TypeScale.of(context).caption
                            .copyWith(color: palette.textFaint),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          Align(
            alignment: Alignment.centerLeft,
            child: Button(
              label: 'Connect a pipe…',
              onPressed: () => setState(() {
                _connectingPipe = true;
                error = null;
              }),
            ),
          ),
        ],
      ),
    );
  }

  /// Archives the project at once (nothing is lost, so nothing to
  /// confirm), or unarchives it.
  Widget _archiveField(Backend backend) {
    final archived =
        [
          for (final project in backend.workspace.workspace.projects)
            if (project.id == widget.project.id) project.archived,
        ].firstOrNull ??
        widget.project.archived;
    return Field(
      label: 'Archive',
      note: archived
          ? "Archived: it's hidden from the sidebar, and its agents don't "
                'work until you unarchive it.'
          : [
              'Hides the project and its agents from the sidebar, and its '
                  'agents stop working. Nothing is deleted, and you can '
                  'unarchive it anytime.',
              // Its liaison doesn't run: the other side hears nothing.
              if (pipesIn(
                backend.workspace.workspace,
                widget.project.id,
              ).isNotEmpty)
                'Its pipes stay open, but nothing answers on them until you '
                    'unarchive it.',
              // Its agents' pins go with it, and don't come back.
              unpinnedOnArchive(backend.workspace.workspace, widget.project.id),
            ].where((line) => line.isNotEmpty).join(' '),
      child: Align(
        alignment: Alignment.centerLeft,
        child: Button(
          label: archived ? 'Unarchive project' : 'Archive project',
          onPressed: submitting
              ? null
              : () => submitRequest(
                  backend.updateProject(
                    UpdateProject(
                      projectId: widget.project.id,
                      archived: !archived,
                    ),
                  ),
                  widget.close,
                ),
        ),
      ),
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
                kind: InputKind.text,
                onSubmitted: (_) => onSubmit(),
              ),
            ),
            // Where it is now, as saved.
            if (showsInFinder(AppScope.of(context)) &&
                edit.folder.path.isNotEmpty) ...[
              const SizedBox(width: Space.xs),
              Button(
                label: 'Show in Finder',
                onPressed: () =>
                    unawaited(showInFinder([File(edit.folder.path)])),
              ),
            ],
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

/// A line at the top of the form under it: why it's open instead of
/// something else (Rivet paused, say).
class FormNote extends InheritedWidget {
  const FormNote({super.key, required this.note, required super.child});

  final Widget note;

  static Widget? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<FormNote>()?.note;

  @override
  bool updateShouldNotify(FormNote old) => old.note != note;
}

/// A phone shows every project, each folding in place: no project is
/// focused there, even while one is on a Mac or an iPad.
bool phoneShowsAllProjects(BuildContext context) =>
    isPhoneLayout(context) &&
    switch (defaultTargetPlatform) {
      TargetPlatform.iOS => deviceIdiom(context) != Idiom.pad,
      TargetPlatform.android => true,
      _ => false,
    };

/// A dialog's points: each a bullet, its lines hanging after it.
class _Points extends StatelessWidget {
  const _Points(this.points);

  final List<String> points;

  @override
  Widget build(BuildContext context) {
    final style = TypeScale.of(context).body
        .copyWith(color: Palette.of(context).textMuted);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final point in points)
          Padding(
            padding: const EdgeInsets.only(top: Space.xs),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                ExcludeSemantics(child: Text('•  ', style: style)),
                Expanded(child: Text(point, style: style)),
              ],
            ),
          ),
      ],
    );
  }
}

/// What moving [agent] to the project [to] ("" for none) changes, for
/// Agent settings' Project (Lucy's projects-sidebar §5.2, Trish's words):
/// "June will work in Bakery's folder and can message only agents in
/// Bakery." Null while it stays where it is. An admin agent messages every
/// agent wherever it is, so only its folder is said.
String? projectMoveNote(
  Workspace workspace, {
  required Agent agent,
  required String to,
  required bool admin,
}) {
  if (to == agent.projectId) return null;
  final name = nameInText(agent.name);
  if (to.isEmpty) {
    return admin
        ? '$name will work in the workspace folder.'
        : '$name will work in the workspace folder and can message only '
              'Rivet and other agents outside projects.';
  }
  final project = workspace.projects.where((p) => p.id == to).firstOrNull;
  if (project == null) return null;
  final there = nameInText(project.name);
  return admin
      ? "$name will work in $there's folder."
      : "$name will work in $there's folder and can message only agents in "
            '$there.';
}

/// What deleting [project] does, said before it's done (Lucy's
/// projects-sidebar §7): its agents and their conversations go, its folder
/// and files stay, and its pipes close, their messages here deleted for
/// good (the other side keeps its own).
String deleteProjectBody(Workspace workspace, Project project) {
  final agents = workspace.agents
      .where((agent) => agent.projectId == project.id)
      .length;
  final pipes = [
    for (final pipe in pipesIn(workspace, project.id)) pipePeerName(pipe),
  ];
  final names = pipes.length <= 3
      ? _andNames(pipes)
      : '${pipes.take(3).join(', ')} and ${pipes.length - 3} more';
  return [
    if (agents == 1) 'This deletes its agent and its conversation.',
    if (agents > 1) 'This deletes its $agents agents and their conversations.',
    project.path.isEmpty
        ? 'Its folder and files stay.'
        : 'Its folder and files stay in ${shortPath(project.path)}.',
    if (pipes.length == 1)
      'Its pipe with $names closes, and its messages here are deleted for '
          'good.',
    if (pipes.length > 1)
      'Its pipes with $names close, and their messages here are deleted for '
          'good.',
  ].join(' ');
}

/// "A", "A and B", "A, B and C".
String _andNames(List<String> names) => names.length < 2
    ? names.join()
    : '${names.sublist(0, names.length - 1).join(', ')} and ${names.last}';

/// Whether Show in Finder can show a folder of [backend]'s: on the Mac
/// that runs the agents, where its folders are this Mac's.
bool showsInFinder(Backend backend) =>
    defaultTargetPlatform == TargetPlatform.macOS &&
    backend.localBackend &&
    backend.target == null;

/// A project's folder in Project settings (Lucy's projects-sidebar §6):
/// its path in a caption, selectable, cut in the middle when it's long;
/// on the Mac that runs the agents, Show in Finder after it.
class ProjectFolderLine extends StatelessWidget {
  const ProjectFolderLine({super.key, required this.path});

  final String path;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final machine = _machineOf(backend);
    final style = TypeScale.of(context).caption
        .copyWith(color: palette.textMuted);
    final shown = Semantics(
      label: machine == null ? path : '$path, on $machine',
      excludeSemantics: true,
      child: SelectableArea(child: MiddleCut(shortPath(path), style: style)),
    );
    if (!showsInFinder(backend)) return shown;
    return Row(
      children: [
        Expanded(child: shown),
        const SizedBox(width: Space.s),
        Unselectable(
          child: Button(
            label: 'Show in Finder',
            onPressed: () => unawaited(showInFinder([File(path)])),
          ),
        ),
      ],
    );
  }
}

/// [text] on one line, cut in the middle with "…" when it doesn't fit, so
/// a path keeps its start and, longer, its end.
class MiddleCut extends StatelessWidget {
  const MiddleCut(this.text, {super.key, required this.style});

  final String text;
  final TextStyle style;

  @override
  Widget build(BuildContext context) {
    final scaler = MediaQuery.textScalerOf(context);
    final direction = Directionality.of(context);
    return LayoutBuilder(
      builder: (context, constraints) {
        double width(String words) {
          final painter = TextPainter(
            text: TextSpan(text: words, style: style),
            textDirection: direction,
            textScaler: scaler,
            maxLines: 1,
          )..layout();
          final measured = painter.width;
          painter.dispose();
          return measured;
        }

        String cut(int keep) {
          final head = keep ~/ 3;
          return '${text.substring(0, head)}…'
              '${text.substring(text.length - (keep - head))}';
        }

        var shown = text;
        if (constraints.hasBoundedWidth && width(text) > constraints.maxWidth) {
          // The most characters that fit, a third of them from the start.
          var low = 0;
          var high = text.length - 1;
          while (low < high) {
            final mid = (low + high + 1) ~/ 2;
            if (width(cut(mid)) <= constraints.maxWidth) {
              low = mid;
            } else {
              high = mid - 1;
            }
          }
          shown = cut(low);
        }
        return Text(shown, style: style, maxLines: 1, softWrap: false);
      },
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
            // A path: as typed, no capitals or corrections.
            kind: InputKind.text,
            textCapitalization: TextCapitalization.none,
            autocorrect: false,
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
    // In a place macOS protects (Lucy's P2): whether it asks first, or
    // refused, as the backend finds.
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final workspace = backend.workspace.workspace;
        final place = protectedRootOf(workspace, controller.text);
        final protected = place == null
            ? null
            : protectedPathCaption(workspace, place);
        return _captioned(
          context,
          row,
          protected,
          remote == null ? null : machine,
        );
      },
    );
  }

  /// [row] over the captions it has: [protected]'s, and where the path is
  /// when the Claktory is on [machine], another one.
  Widget _captioned(
    BuildContext context,
    Widget row,
    String? protected,
    String? machine,
  ) {
    final palette = Palette.of(context);
    if (machine == null && protected == null) return row;
    final caption = TypeScale.of(context).caption;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        row,
        if (protected != null) ...[
          const SizedBox(height: Space.xs),
          Text(protected, style: caption.copyWith(color: palette.textMuted)),
        ],
        if (machine != null) ...[
          const SizedBox(height: Space.xs),
          Text(
            'A path on $machine, where this Claktory runs.',
            style: caption.copyWith(color: palette.textFaint),
          ),
        ],
      ],
    );
  }
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

/// "Default (GPT-5.5)": what an agent with no fallback model
/// of its own uses, or "(none)" when Settings → Defaults has none.
String _followDefaultLabel(Workspace workspace) {
  final id = workspace.settings.defaultFallbackModel;
  if (id.isEmpty) return 'Default (none)';
  for (final model in workspace.models) {
    if (model.id == id) return 'Default (${modelLabel(model)})';
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
    this.projectNote,
    this.defaultPrompt = '',
    this.lead,
    this.fallback,
    this.appearance,
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

  /// Under Project: what moving it there changes, once it's changed.
  final String? projectNote;

  /// The prompt Claktory ships for this agent, to go back to; empty when it
  /// has none (only Rivet has one).
  final String defaultPrompt;

  /// The Lead field, under where it works; only an existing agent has it.
  final Widget? lead;

  /// The Fallback model field, under the model; only an existing agent
  /// has it.
  final Widget? fallback;

  /// How it looks (its character, shape and color), in place of the
  /// color alone; only an existing agent has it.
  final Widget? appearance;

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
    final phone = isPhoneLayout(context);
    // Two fields side by side where there's room; on a phone, stacked.
    Widget pair(Widget first, Widget second, {int a = 1, int b = 1}) => Flex(
      direction: phone ? Axis.vertical : Axis.horizontal,
      crossAxisAlignment: phone
          ? CrossAxisAlignment.stretch
          : CrossAxisAlignment.start,
      children: [
        _Share(flex: a, child: first),
        if (!phone) const SizedBox(width: Space.m),
        _Share(flex: b, child: second),
      ],
    );
    // Lucy's agent view §6: the model first (the change people make
    // themselves when a provider runs out), then the name and role, how
    // it looks, its prompt, and where it works.
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (workspace.models.isEmpty)
          Field(
            label: 'Model',
            child: Builder(
              builder: (context) {
                final words = Text(
                  'No model available',
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.textMuted),
                );
                final signIn = Button(
                  label: 'Sign in…',
                  onPressed: () {
                    close();
                    AppScope.openProviders(context);
                  },
                );
                // On a phone, Sign in goes under the words.
                if (isPhoneLayout(context)) {
                  return Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      words,
                      const SizedBox(height: Space.s),
                      signIn,
                    ],
                  );
                }
                return Row(
                  children: [
                    Expanded(child: words),
                    signIn,
                  ],
                );
              },
            ),
          )
        else
          pair(
            a: 3,
            b: 2,
            Field(
              label: 'Model',
              child: ModelSelect(
                workspace: workspace,
                value: model,
                extra: [
                  if (offerDefault)
                    SelectOption('', _defaultModelLabel(workspace)),
                ],
                onChanged: onModel,
              ),
            ),
            Field(
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
        if (workspace.models.isNotEmpty) ?fallback,
        pair(
          Field(
            label: 'Name',
            child: TextField(
              controller: name,
              // A phone brings the keyboard up only for a new agent's
              // name, not over its settings.
              autofocus: !phone || name.text.isEmpty,
              kind: InputKind.text,
              onSubmitted: (_) => onSubmit(),
            ),
          ),
          Field(
            label: 'Role',
            child: TextField(
              controller: role,
              placeholder: 'Such as Coordinator or Writer',
              semanticsLabel: 'Role, shown under its name. Optional.',
              kind: InputKind.text,
              onSubmitted: (_) => onSubmit(),
            ),
          ),
        ),
        appearance ??
            Field(
              label: 'Color',
              child: ColorPicker(value: color, onChanged: onColor),
            ),
        Field(
          label: 'Prompt',
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _FoldedPrompt(controller: prompt),
              if (defaultPrompt.isNotEmpty)
                _DefaultPrompt(prompt: prompt, defaultPrompt: defaultPrompt),
            ],
          ),
        ),
        _where(context, pair),
        ?lead,
      ],
    );
  }

  /// Project and Folder, side by side where there's room.
  Widget _where(
    BuildContext context,
    Widget Function(Widget, Widget, {int a, int b}) pair,
  ) {
    final project = workspace.projects.isEmpty
        ? null
        : Field(
            label: 'Project',
            note: projectNote,
            child: Select(
              value: this.project,
              options: [
                if (offerWorkspace)
                  SelectOption('', '$outsideLabel (no project)'),
                for (final candidate in workspace.projects)
                  SelectOption(candidate.id, candidate.name),
              ],
              onChanged: onProject,
            ),
          );
    final folder = _folders.isEmpty
        ? null
        : Field(
            label: 'Folder',
            note: "Where it works: one of the project's folders.",
            child: Select(
              value: _folders.any((candidate) => candidate.id == this.folder)
                  ? this.folder
                  : '',
              options: [
                const SelectOption('', "The project's folder"),
                for (final candidate in _folders)
                  SelectOption(candidate.id, candidate.name),
              ],
              onChanged: onFolder,
            ),
          );
    if (project != null && folder != null) return pair(project, folder);
    return project ?? folder ?? const SizedBox.shrink();
  }
}

/// An agent's prompt, folded to its first 6 lines with Show all while it
/// runs longer (Lucy's agent view §6); it unfolds when it takes the keys,
/// so typing never happens in a fold.
class _FoldedPrompt extends StatefulWidget {
  const _FoldedPrompt({required this.controller});

  final TextEditingController controller;

  @override
  State<_FoldedPrompt> createState() => _FoldedPromptState();
}

class _FoldedPromptState extends State<_FoldedPrompt> {
  static const _foldLines = 6;
  final _focus = FocusNode();
  var _open = false;

  @override
  void initState() {
    super.initState();
    _focus.addListener(() {
      if (_focus.hasFocus && !_open) setState(() => _open = true);
    });
  }

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final field = TextField(
        controller: widget.controller,
        focusNode: _focus,
        placeholder: 'What this agent does and how',
        semanticsLabel: 'Prompt',
        kind: InputKind.multiline,
        minLines: 4,
        maxLines: _open ? null : _foldLines,
      );
      // Longer than the fold: Show all under it. The field stays first in
      // the column either way, so unfolding on focus keeps the keys in it.
      var folded = false;
      if (!_open) {
        final painter = TextPainter(
          text: TextSpan(
            text: widget.controller.text,
            style: TypeScale.of(context).body,
          ),
          textDirection: Directionality.of(context),
          textScaler: MediaQuery.textScalerOf(context),
        )..layout(maxWidth: math.max(0, constraints.maxWidth - 2 * Space.m));
        folded = painter.computeLineMetrics().length > _foldLines;
        painter.dispose();
      }
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          field,
          if (folded)
            QuietLink(
              label: 'Show all',
              onTap: () => setState(() => _open = true),
            ),
        ],
      );
    },
  );
}

/// Its [flex] share of a Mac's row; on a phone, where the row is a column,
/// its own height.
class _Share extends StatelessWidget {
  const _Share({required this.flex, required this.child});

  final int flex;
  final Widget child;

  @override
  Widget build(BuildContext context) =>
      isPhoneLayout(context) ? child : Expanded(flex: flex, child: child);
}

/// The agent colors as swatches; the chosen one is ringed.
class ColorPicker extends StatelessWidget {
  const ColorPicker({super.key, required this.value, required this.onChanged});

  final String value;
  final ValueChanged<String> onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    // A finger's target on a phone.
    final phone = isPhoneLayout(context);
    // At least the platform's floor where it's used with a finger
    // (Android's 48); a Mac's and an iPad's stay as they were.
    final size = PhonePlatformPolicy.least(
      context,
      phone ? Touch.target : 26.0,
    );
    final spacing = phone ? Space.xs : Space.s;
    final swatches = Wrap(
      spacing: spacing,
      runSpacing: spacing,
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
                width: size,
                height: size,
                padding: EdgeInsets.all(phone ? 5 : 3),
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
    // In one row when they fit; else split evenly (4 + 4), so no swatch
    // sits alone on a line (Lucy's agent view §6).
    return LayoutBuilder(
      builder: (context, constraints) {
        final count = agentColors.length;
        final room = constraints.maxWidth;
        if (!room.isFinite) return swatches;
        final fit = math.max(1, ((room + spacing) / (size + spacing)).floor());
        if (fit >= count) return swatches;
        final rows = (count / fit).ceil();
        final perRow = (count / rows).ceil();
        return Align(
          alignment: AlignmentDirectional.centerStart,
          child: SizedBox(
            width: perRow * (size + spacing) - spacing + 0.5,
            child: swatches,
          ),
        );
      },
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

/// Under an agent's prompt, when Claktory ships a default for it: says the
/// prompt is the default, or offers to put the default back (applied with
/// Save).
class _DefaultPrompt extends StatelessWidget {
  const _DefaultPrompt({required this.prompt, required this.defaultPrompt});

  final TextEditingController prompt;
  final String defaultPrompt;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: prompt,
      builder: (context, _) {
        // As the backend compares them: trailing whitespace aside.
        final isDefault = prompt.text.trim() == defaultPrompt.trim();
        return Padding(
          padding: const EdgeInsets.only(top: Space.xs + 2),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  isDefault
                      ? "Claktory's default. Each update brings the newest "
                            'version until you change it.'
                      : 'Changed from the default.',
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textFaint),
                ),
              ),
              if (!isDefault) ...[
                const SizedBox(width: Space.s),
                Button(
                  label: 'Reset to default',
                  onPressed: () => prompt.value = TextEditingValue(
                    text: defaultPrompt,
                    selection: TextSelection.collapsed(
                      offset: defaultPrompt.length,
                    ),
                  ),
                ),
              ],
            ],
          ),
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
    this.showExtensions = false,
  });

  final Agent agent;
  final VoidCallback close;

  /// Opens at its Extensions, unfolded and scrolled into sight (the
  /// agent's menu's Extensions…, Lucy's agent view §4).
  final bool showExtensions;

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

  /// The model a step its model declines is tried again on; empty to
  /// follow the default.
  late var _fallback = widget.agent.fallbackModel;

  /// The agent it reports to, as chosen here; empty for none.
  late var _lead = widget.agent.leadId;

  /// Whether it notifies the person, as chosen here.
  late var _notify = widget.agent.notify;

  /// The shape chosen here; empty for automatic.
  late var _shape = _shapeChoice(widget.agent);
  var _confirmingDelete = false;

  /// Its extensions' list is unfolded ("11 on · Change…").
  late var _extensionsOpen = widget.showExtensions;

  /// Asking whether to save before going to Rivet (Lucy's agent view §6).
  var _confirmingRivet = false;

  /// "Or ask Rivet to change June": Rivet's conversation with "Change
  /// June: " in its message box, nothing sent. With changes here, it asks
  /// first whether to save them.
  void _toRivet({bool save = false}) {
    final name = widget.agent.name;
    void go() {
      // While the form's context still stands, then the form goes.
      final rivet = rivetIn(AppScope.of(context).workspace.workspace);
      askRivet(
        context,
        'Change $name: ',
        hint: 'Tell ${rivet?.name ?? 'Rivet'} what to change',
      );
      widget.close();
    }

    if (save) {
      final backend = AppScope.of(context);
      final update = _changes();
      final changes = _extensionChanges();
      final id = widget.agent.id;
      submitRequest(() async {
        if (update != null) await backend.updateAgent(update);
        for (final change in changes) {
          await backend.updateAgent(
            UpdateAgent(agentId: id, extension_8: change),
          );
        }
      }(), go);
      return;
    }
    go();
  }

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
    if (prompt != agent.prompt.trim()) update.prompt = prompt;
    if (_project != agent.projectId) update.projectId = _project;
    // Moving to another project leaves its folder; the backend clears it.
    if (_folder != agent.folderId &&
        (_folder.isNotEmpty || _project == agent.projectId)) {
      update.folderId = _folder;
    }
    if (_model != agent.model) update.model = _model;
    if (_effort != agent.effort) update.effort = _effort;
    if (_fallback != agent.fallbackModel) update.fallbackModel = _fallback;
    if (_color != agent.color) update.color = _color;
    final lead = _leadIn(AppScope.of(context).workspace.workspace);
    if (lead != agent.leadId) update.leadId = lead;
    if (_shape != _shapeChoice(agent)) update.avatarShape = _shape;
    if (_notify != agent.notify) update.notify = _notify;
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
        update.hasFallbackModel() ||
        update.hasColor() ||
        update.hasAvatarShape() ||
        update.hasPlaceholder() ||
        update.hasFolderId() ||
        update.hasLeadId() ||
        update.hasNotify();
    return changed ? update : null;
  }

  /// The lead chosen here, if it can still be one where the agent is going;
  /// else none, as the backend would make it.
  String _leadIn(Workspace workspace) =>
      leadCandidates(
        workspace,
        widget.agent.id,
        _project,
      ).any((candidate) => candidate.id == _lead)
      ? _lead
      : '';

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
    // Turning the browser off deletes its sign-ins and data, so Save asks
    // first.
    final browserOff = changes.any(
      (change) =>
          change.id == browserExtension &&
          change.hasEnabled() &&
          !change.enabled,
    );
    // One extension change per request, as the protocol has it.
    Future<void> save() async {
      if (browserOff && !await confirmBrowserOff(context, backend, _current)) {
        throw const _SetAside();
      }
      if (update != null) await backend.updateAgent(update);
      for (final change in changes) {
        await backend.updateAgent(
          UpdateAgent(agentId: id, extension_8: change),
        );
      }
    }

    submitRequest(save(), widget.close);
  }

  /// The workspace's extensions, those with something to show first (a
  /// panel, or the secrets it was given), each group in its order.
  List<Extension> _showingFirst(Workspace workspace) {
    bool shows(Extension extension) =>
        hasExtensionView(extension) || extension.id == 'secrets';
    return [
      ...workspace.extensions.where(shows),
      ...workspace.extensions.where((extension) => !shows(extension)),
    ];
  }

  /// How it looks, in one row (Lucy's §6): its character as it will be
  /// saved, its shape and its color; Shape's help under them. On a phone
  /// the swatches take a row of their own.
  Widget _appearance(Agent agent) => Builder(
    builder: (context) {
      final previewed = _previewed(agent);
      final character = AgentCharacter(agent: previewed, size: 28);
      final shape = SizedBox(
        width: 140,
        child: Select(
          title: 'Shape',
          value: _shape,
          options: [
            const SelectOption('', 'Automatic'),
            for (final shape in CharacterShape.values)
              SelectOption(shape.name, _capitalized(shape.name)),
          ],
          onChanged: (shape) => setState(() => _shape = shape),
        ),
      );
      final colors = ColorPicker(
        value: _color,
        onChanged: (color) => setState(() => _color = color),
      );
      // On a phone, and at the largest text anywhere, the swatches go
      // under the character and its shape.
      final phone = isPhoneLayout(context) || largeText(context);
      return Field(
        label: 'Appearance',
        note:
            'Automatic keeps the same shape when you rename or move the '
            'agent.',
        child: phone
            ? Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      character,
                      const SizedBox(width: Space.m),
                      Expanded(child: shape),
                    ],
                  ),
                  const SizedBox(height: Space.s),
                  colors,
                ],
              )
            : Wrap(
                spacing: Space.m,
                runSpacing: Space.s,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [character, shape, colors],
              ),
      );
    },
  );

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
    if (_confirmingRivet) {
      return FormFrame(
        title: 'Save your changes to ${agent.name}?',
        error: error,
        submitLabel: 'Save',
        cancelLabel: 'Discard',
        onSubmit: submitting ? null : () => _toRivet(save: true),
        onCancel: () => _toRivet(),
        fields: const [],
      );
    }
    final store = backend.workspace;
    final rivet = rivetIn(store.workspace);
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
          dirty: _changed,
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
            // How Matt changes agents, said where he'd change one (Lucy's
            // §6); not in Rivet's own, nor with no Rivet to ask.
            if (rivet != null && rivet.id != agent.id && rivet.modelAvailable)
              Padding(
                padding: const EdgeInsets.only(bottom: Space.l),
                child: QuietLink(
                  label: 'Or ask ${rivet.name} to change ${agent.name}',
                  onTap: () => _changed
                      ? setState(() => _confirmingRivet = true)
                      : _toRivet(),
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
              appearance: _appearance(agent),
              // As the backend has it now, with what's changed here: the
              // admin option turned on or off makes it an admin agent.
              outsideLabel: outsideProjectsLabel(_edited),
              // What Save would change by moving it (Lucy's
              // projects-sidebar §5.2); no confirm, Save is deliberate.
              projectNote: projectMoveNote(
                workspace,
                agent: agent,
                to: _project,
                admin: isAdminAgent(_edited),
              ),
              defaultPrompt:
                  [
                    for (final candidate in workspace.agents)
                      if (candidate.id == agent.id) candidate.defaultPrompt,
                  ].firstOrNull ??
                  agent.defaultPrompt,
              lead: Field(
                label: 'Lead',
                note:
                    "Who this agent reports to. Its lead can see how it's "
                    'set up.',
                child: Select(
                  value: _leadIn(workspace),
                  options: [
                    const SelectOption('', 'None'),
                    for (final candidate in leadCandidates(
                      workspace,
                      agent.id,
                      _project,
                    ))
                      SelectOption(
                        candidate.id,
                        candidate.projectId == _project
                            ? candidate.name
                            // Elsewhere, so an admin: its role says so.
                            : '${candidate.name} '
                                  '(${candidate.role.isEmpty ? 'Admin' : candidate.role})',
                      ),
                  ],
                  onChanged: (id) => setState(() => _lead = id),
                ),
              ),
              fallback: Field(
                label: 'Fallback model',
                note:
                    'If its model declines a step for safety reasons, '
                    'Claktory tries it again on this one.',
                child: ModelSelect(
                  workspace: workspace,
                  value: _fallback,
                  title: 'Fallback model',
                  extra: [SelectOption('', _followDefaultLabel(workspace))],
                  onChanged: (id) => setState(() => _fallback = id),
                ),
              ),
              close: widget.close,
            ),
            Field(
              label: 'Message box hint',
              note:
                  'Shown in the empty message box. Put several on separate '
                  'lines to rotate them.',
              child: TextField(
                controller: _placeholder,
                kind: InputKind.multiline,
                minLines: 1,
                maxLines: 8,
                // The default, shown while it's left empty.
                placeholder:
                    'Message ${_name.text.trim().isEmpty ? agent.name : _name.text.trim()}',
              ),
            ),
            Field(
              label: 'Notify me',
              note:
                  'Always: every kind, even ones turned off in Settings → '
                  'Notifications, except during quiet hours.',
              child: Select(
                title: 'Notify me',
                value: _notify,
                options: notifyModeOptions,
                onChanged: (mode) => setState(() => _notify = mode),
              ),
            ),
            // Limited to a pipe, its secrets and extensions are off: why,
            // above them.
            if (_current.quarantined)
              _PipeLimitNote(agent: _current, workspace: workspace),
            // Reading reports keeps it from the same: why, and the way out.
            if (_current.readsReports) StopReadingReportsField(agent: _current),
            // Above the extensions, so it's in sight: which secrets it has.
            AgentSecretsField(agent: _current),
            if (workspace.extensions.isNotEmpty && !_extensionsOpen)
              Field(
                label: 'Extensions',
                child: _ExtensionsFolded(
                  on: _edited.extensions.where((e) => e.enabled).length,
                  onChange: () => setState(() => _extensionsOpen = true),
                ),
              ),
            if (workspace.extensions.isNotEmpty && _extensionsOpen)
              _ScrolledTo(
                key: const ValueKey('extensions'),
                scroll: widget.showExtensions,
                child: Field(
                  label: 'Extensions',
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      // Those with something to show first (Lucy's §6).
                      for (final extension in _showingFirst(workspace))
                        if (_saved(extension.id) case final saved?)
                          _ExtensionSettings(
                            extension: extension,
                            workspace: workspace,
                            agentId: agent.id,
                            projectId: _project,
                            description: extension.id == browserExtension
                                ? browserRefusal(_current) ??
                                      (approvesElsewhere && !saved.enabled
                                          ? browserElsewhereText(_current.name)
                                          : browserDescription(_current.name))
                                : null,
                            // Ask is all a pipe's limit leaves it; reading
                            // reports, its database and Reports too.
                            limited:
                                !containedKeeps(_current, extension.id) ||
                                (extension.id == browserExtension &&
                                    (browserRefusal(_current) != null ||
                                        approvesElsewhere && !saved.enabled)),
                            enabled: _enabled[extension.id] ?? saved.enabled,
                            value: (option) =>
                                _options[extension.id]?[option] ??
                                saved.options[option] ??
                                OptionValue(),
                            onEnabled: (on) =>
                                setState(() => _enabled[extension.id] = on),
                            onOption: (option, value) => setState(
                              () => (_options[extension.id] ??= {})[option] =
                                  value,
                            ),
                          ),
                    ],
                  ),
                ),
              ),
          ],
        );
      },
    );
  }
}

/// An agent's extensions, folded (Lucy's agent view §6): "11 on ·
/// Change…", which unfolds the list in place.
class _ExtensionsFolded extends StatelessWidget {
  const _ExtensionsFolded({required this.on, required this.onChange});

  final int on;
  final VoidCallback onChange;

  @override
  Widget build(BuildContext context) => Wrap(
    spacing: Space.s,
    crossAxisAlignment: WrapCrossAlignment.center,
    children: [
      Text(
        '$on on ·',
        style: TypeScale.of(context).body
            .copyWith(color: Palette.of(context).text),
      ),
      QuietLink(label: 'Change…', onTap: onChange),
    ],
  );
}

/// Whom agent [agentId] can report to from [project]: the other agents
/// there, then the admin agents elsewhere. Those that report to it,
/// directly or through others, are left out: that would make a loop.
List<Agent> leadCandidates(
  Workspace workspace,
  String agentId,
  String project,
) {
  final byId = {for (final agent in workspace.agents) agent.id: agent};
  bool reportsToIt(Agent candidate) {
    final seen = <String>{};
    for (
      var lead = candidate.leadId;
      lead.isNotEmpty && seen.add(lead);
      lead = byId[lead]?.leadId ?? ''
    ) {
      if (lead == agentId) return true;
    }
    return false;
  }

  bool fits(Agent candidate) =>
      candidate.id != agentId && !reportsToIt(candidate);
  return [
    for (final candidate in workspace.agents)
      if (candidate.projectId == project && fits(candidate)) candidate,
    for (final candidate in workspace.agents)
      if (candidate.projectId != project &&
          isAdminAgent(candidate) &&
          fits(candidate))
        candidate,
  ];
}

/// Whether two option values are the same: the switch, and the same agents
/// in any order.
bool _sameValue(OptionValue a, OptionValue? b) {
  b ??= OptionValue();
  return a.on == b.on &&
      a.choice == b.choice &&
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
    this.limited = false,
    this.description,
  });

  final Extension extension;
  final Workspace workspace;
  final String agentId;

  /// The agent's project as the form has it.
  final String projectId;
  final bool enabled;

  /// Off while the agent is limited to a pipe: greyed, its options hidden,
  /// nothing to change.
  final bool limited;
  final OptionValue Function(String optionId) value;
  final ValueChanged<bool> onEnabled;
  final void Function(String optionId, OptionValue value) onOption;

  final String? description;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return LayoutBuilder(
      builder: (context, constraints) => Container(
        margin: const EdgeInsets.only(bottom: Space.s),
        // Stacked, the name takes what the card's sides would: a long one
        // ("Messaging") fits Slide Over's width whole at the largest text.
        padding: _stacks(context, constraints.maxWidth)
            ? const EdgeInsets.symmetric(
                horizontal: Space.xs,
                vertical: Space.m,
              )
            : const EdgeInsets.all(Space.m),
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
                color: enabled && !limited ? palette.text : palette.textMuted,
              ),
              label: extension.name,
              description: description ?? extension.description,
              strong: true,
              value: enabled,
              onChanged: limited ? null : onEnabled,
            ),
            // Given secrets work whether or not it's on: say which it has.
            if (extension.id == 'secrets')
              Padding(
                padding: const EdgeInsets.only(
                  top: Space.s,
                  left: Space.l + Space.s,
                ),
                child: AgentSecretsSummary(agentId: agentId),
              ),
            if (enabled && !limited)
              for (final option in extension.options)
                Padding(
                  padding: const EdgeInsets.only(
                    top: Space.m,
                    left: Space.l + Space.s,
                  ),
                  child: switch (option.kind) {
                    ExtensionOptionKind.EXTENSION_OPTION_KIND_AGENTS =>
                      AgentListField(
                        label: option.label,
                        description: option.description,
                        workspace: workspace,
                        hidden: agentId,
                        alwaysIn: option.includesProject ? projectId : null,
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
                    ExtensionOptionKind.EXTENSION_OPTION_KIND_CHOICE =>
                      _ChoiceRow(
                        option: option,
                        value: value(option.id).choice,
                        onChanged: (choice) =>
                            onOption(option.id, OptionValue(choice: choice)),
                      ),
                    // A kind from a newer backend: say so rather than guess.
                    _ => Text(
                      '${option.label}: needs a newer Claktory',
                      style: TypeScale.of(context).caption
                          .copyWith(color: palette.textFaint),
                    ),
                  },
                ),
          ],
        ),
      ),
    );
  }
}

/// Above an agent's secrets while it's limited to a pipe (Agent.quarantined):
/// what it can still do, and that its secrets and extensions are off until
/// the person lets it run commands in the pipe's settings.
class _PipeLimitNote extends StatelessWidget {
  const _PipeLimitNote({required this.agent, required this.workspace});

  final Agent agent;
  final Workspace workspace;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(bottom: Space.l),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Padding(
          padding: EdgeInsets.only(top: 2),
          child: ExcludeSemantics(
            child: PortalGlyph(color: pipeColor, size: 12),
          ),
        ),
        const SizedBox(width: Space.xs + 2),
        Expanded(
          child: Text(
            pipeLimitNote(pipesLimiting(workspace, agent.id)),
            style: TypeScale.of(context).caption
                .copyWith(color: Palette.of(context).textMuted),
          ),
        ),
      ],
    ),
  );
}

/// Why an agent limited to [pipes] has its secrets and extensions off.
String pipeLimitNote(List<Pipe> pipes) => pipes.length < 2
    ? '${pipes.isEmpty ? 'Limited to a pipe' : pipeLimitText(pipes)}: it can '
          'only read and answer it, and ask you. Its secrets and extensions '
          "are off until you let it run commands in the pipe's settings."
    : '${pipeLimitText(pipes)}: it can only read and answer them, and ask '
          'you. Its secrets and extensions are off until you let it run '
          'commands in their settings.';

/// An option that's one of a few (ExtensionOption.choices): its label and
/// description beside a select of them, the default while none is set;
/// on a phone the select goes under the words.
class _ChoiceRow extends StatelessWidget {
  const _ChoiceRow({
    required this.option,
    required this.value,
    required this.onChanged,
  });

  final ExtensionOption option;
  final String value;
  final ValueChanged<String> onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final chosen = value.isEmpty ? option.defaultChoice : value;
    final select = Select<String>(
      value: chosen,
      options: [
        // One the catalog doesn't list (a newer Claktory's), as it is.
        if (!option.choices.any((choice) => choice.value == chosen))
          SelectOption(chosen, chosen),
        for (final choice in option.choices)
          SelectOption(choice.value, choice.label),
      ],
      onChanged: onChanged,
    );
    final words = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          option.label,
          style: TypeScale.of(context).body.copyWith(color: palette.text),
        ),
        if (option.description.isNotEmpty)
          Text(
            option.description,
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textMuted),
          ),
      ],
    );
    if (isPhoneLayout(context)) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          words,
          const SizedBox(height: Space.xs),
          select,
        ],
      );
    }
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(child: words),
        const SizedBox(width: Space.m),
        SizedBox(width: 180, child: select),
      ],
    );
  }
}

/// Whether a switch's words go under it rather than beside it: narrower
/// than 400 at the accessibility text sizes (an iPad's Slide Over), so they
/// take the row's whole width and a name never breaks mid-word.
bool _stacks(BuildContext context, double width) =>
    largeText(context) && width < 400;

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

  /// Null greys it: it can't be changed now.
  final ValueChanged<bool>? onChanged;
  final Widget? leading;

  /// The extension itself rather than one of its options.
  final bool strong;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final words = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          label,
          style:
              (strong
                      ? TypeScale.of(context).label
                      : TypeScale.of(context).body)
                  .copyWith(
                    color: onChanged == null ? palette.textMuted : palette.text,
                  ),
        ),
        if (description.isNotEmpty)
          Text(
            description,
            style: TypeScale.of(context).caption.copyWith(
              color: onChanged == null ? palette.textFaint : palette.textMuted,
            ),
          ),
      ],
    );
    final toggle = Padding(
      padding: const EdgeInsets.only(top: 2),
      child: Toggle(value: value, onChanged: onChanged, label: label),
    );
    return LayoutBuilder(
      builder: (context, constraints) {
        final style =
            (strong ? TypeScale.of(context).label : TypeScale.of(context).body)
                .copyWith(
                  color: onChanged == null ? palette.textMuted : palette.text,
                );
        // A word of the name wider than the line beside the switch (a
        // finger's target at most) would break inside it: then the glyph
        // and switch go above, and the name has the whole width.
        bool wordsFit() {
          final room = constraints.maxWidth - Space.s - 48;
          for (final word in label.split(' ')) {
            final painter = TextPainter(
              text: TextSpan(text: word, style: style),
              textScaler: MediaQuery.textScalerOf(context),
              textDirection: Directionality.of(context),
              maxLines: 1,
            )..layout();
            final width = painter.width;
            painter.dispose();
            if (width > room) return false;
          }
          return true;
        }

        if (_stacks(context, constraints.maxWidth) && !wordsFit()) {
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            spacing: Space.xs,
            children: [
              Row(children: [?leading, const Spacer(), toggle]),
              words,
            ],
          );
        }
        if (_stacks(context, constraints.maxWidth)) {
          // At the largest text (Lucy): the switch at the end of the name's
          // first line, so it reads as the row's, and the description
          // under both. The glyph leads the name, which wraps between
          // words, under it if it must, never inside one.
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            spacing: Space.xs,
            children: [
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(
                    child: Text.rich(
                      TextSpan(
                        children: [
                          if (leading != null) ...[
                            WidgetSpan(
                              alignment: PlaceholderAlignment.middle,
                              child: leading!,
                            ),
                            const TextSpan(text: ' '),
                          ],
                          TextSpan(text: label),
                        ],
                      ),
                      style: style,
                    ),
                  ),
                  const SizedBox(width: Space.s),
                  toggle,
                ],
              ),
              if (description.isNotEmpty)
                Text(
                  description,
                  style: TypeScale.of(context).caption.copyWith(
                    color: onChanged == null
                        ? palette.textFaint
                        : palette.textMuted,
                  ),
                ),
            ],
          );
        }
        return Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (leading != null) ...[
              Padding(padding: const EdgeInsets.only(top: 2), child: leading),
              const SizedBox(width: Space.s),
            ],
            Expanded(child: words),
            const SizedBox(width: Space.m),
            toggle,
          ],
        );
      },
    );
  }
}

/// A list of agents to choose: the chosen ones as chips, and a menu of
/// every other agent, grouped by where they live, to choose from (an
/// extension's agent-list option, who hears Resources alerts). The agents
/// in project [alwaysIn] are always in (an option's `includes_project`), so
/// they show checked and greyed, as do the agents in [always] (admin
/// agents, who may always send urgent alerts); [hidden] (the agent whose
/// option it is) isn't offered. An empty [label] leaves the field's own
/// label out, for a row that has one.
class AgentListField extends StatefulWidget {
  const AgentListField({
    super.key,
    required this.label,
    this.description = '',
    required this.workspace,
    this.hidden = '',
    this.alwaysIn,
    this.always = const {},
    required this.chosen,
    required this.onChanged,
  });

  final String label;
  final String description;
  final Workspace workspace;
  final String hidden;
  final String? alwaysIn;
  final Set<String> always;
  final List<String> chosen;
  final ValueChanged<List<String>> onChanged;

  @override
  State<AgentListField> createState() => _AgentListFieldState();
}

class _AgentListFieldState extends State<AgentListField> {
  final _menu = OverlayPortalController();
  final _link = LayerLink();
  final _tapRegion = Object();
  var _hovered = false;

  /// Agents in the agent's own project, which count without being chosen.
  bool _inProject(Agent agent) =>
      widget.alwaysIn != null && agent.projectId == widget.alwaysIn;

  /// Agents that count without being chosen, and can't be taken out.
  bool _fixed(Agent agent) =>
      _inProject(agent) || widget.always.contains(agent.id);

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
        if (widget.chosen.contains(agent.id) && agent.id != widget.hidden)
          agent,
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (widget.label.isNotEmpty)
          Text(
            widget.label,
            style: TypeScale.of(context).body.copyWith(color: palette.text),
          ),
        if (widget.description.isNotEmpty)
          Text(
            widget.description,
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textMuted),
          ),
        if (widget.label.isNotEmpty || widget.description.isNotEmpty)
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
                                    widget.alwaysIn != null
                                        ? 'No one outside its project'
                                        : 'No one',
                                    style: TypeScale.of(context).body
                                        .copyWith(color: palette.textFaint),
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
          if (inPlace(agent) && agent.id != widget.hidden) agent,
      ];
      if (agents.isEmpty) continue;
      final own = widget.alwaysIn != null && id == widget.alwaysIn;
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
            style: TypeScale.of(context).caption.copyWith(
              color: palette.textFaint,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
      );
      for (final agent in agents) {
        final fixed = _fixed(agent);
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
                      style: TypeScale.of(context).body.copyWith(
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
                      style: TypeScale.of(context).body
                          .copyWith(color: palette.textFaint),
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
            style: TypeScale.of(context).caption.copyWith(color: palette.text),
          ),
        ],
      ),
    );
  }
}

/// The archived project [agent] is in, if it's in one. Its agents don't
/// run, and the app hides them (sidebar, rail) until it's unarchived.
Project? archivedProjectOf(Workspace workspace, Agent agent) {
  if (agent.projectId.isEmpty) return null;
  for (final project in workspace.projects) {
    if (project.id == agent.projectId) return project.archived ? project : null;
  }
  return null;
}

/// Unarchives [project]: it and its agents come back to the sidebar, and
/// its agents read what waited for them.
Future<void> unarchiveProject(Backend backend, Project project) => backend
    .updateProject(UpdateProject(projectId: project.id, archived: false));

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
