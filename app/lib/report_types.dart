// Kinds of report as data (pipes-automation.md §11, T4, on Cioccolata's
// contract): Settings → Pipes lists every kind a pipe's agent can forward,
// built-in ones marked, each opening to exactly what crosses (which fields
// hold the other side's words and so mark whoever reads them, which are
// only a choice), and an editor for Matt: a name, a table of fields (a
// kind, required, its cap clamped to Claktory's ceiling, a list's
// choices), and what its reader may answer. The backend checks every rule
// and keeps every version; the app only shows them and sends the person's
// edits. On screen it's never a schema, record, hand-off or taint.

import 'dart:async';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart' show TextInputFormatter;
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'forms.dart' show FormFrame;
import 'identity.dart';
import 'mobile/input.dart' show InputKind;
import 'mobile/metrics.dart';
import 'pipes.dart' show PortalGlyph, pipeColor;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'usage.dart' show formatAge;

/// What a reader may answer, in the catalogue's order (Claktory's words;
/// no kind or reader writes its own).
const reportOutcomes = [
  PipeReportState.PIPE_REPORT_STATE_ACCEPTED,
  PipeReportState.PIPE_REPORT_STATE_DONE,
  PipeReportState.PIPE_REPORT_STATE_DECLINED,
  PipeReportState.PIPE_REPORT_STATE_KNOWN,
  PipeReportState.PIPE_REPORT_STATE_FIXED,
  PipeReportState.PIPE_REPORT_STATE_WONT_FIX,
  PipeReportState.PIPE_REPORT_STATE_NEEDS_INFO,
];

/// An answer as the editor offers it.
String outcomeLabel(PipeReportState state) => switch (state) {
  PipeReportState.PIPE_REPORT_STATE_ACCEPTED => 'Accepted',
  PipeReportState.PIPE_REPORT_STATE_DONE => 'Done',
  PipeReportState.PIPE_REPORT_STATE_DECLINED => 'Declined',
  PipeReportState.PIPE_REPORT_STATE_KNOWN => 'Already known',
  PipeReportState.PIPE_REPORT_STATE_FIXED => 'Fixed in a release',
  PipeReportState.PIPE_REPORT_STATE_WONT_FIX => 'Won’t fix',
  PipeReportState.PIPE_REPORT_STATE_NEEDS_INFO => 'Needs more',
  _ => '',
};

/// The kinds of field, in the editor's order.
const fieldKinds = [
  ReportFieldType.REPORT_FIELD_TYPE_TEXT,
  ReportFieldType.REPORT_FIELD_TYPE_TEXT_LIST,
  ReportFieldType.REPORT_FIELD_TYPE_ENUM,
  ReportFieldType.REPORT_FIELD_TYPE_BOOL,
  ReportFieldType.REPORT_FIELD_TYPE_NUMBER,
  ReportFieldType.REPORT_FIELD_TYPE_DATE,
  ReportFieldType.REPORT_FIELD_TYPE_VERSION,
  ReportFieldType.REPORT_FIELD_TYPE_ATTACHMENTS,
];

String fieldKindLabel(ReportFieldType kind) => switch (kind) {
  ReportFieldType.REPORT_FIELD_TYPE_TEXT => 'Text',
  ReportFieldType.REPORT_FIELD_TYPE_TEXT_LIST => 'List',
  ReportFieldType.REPORT_FIELD_TYPE_ENUM => 'One of a list',
  ReportFieldType.REPORT_FIELD_TYPE_BOOL => 'Yes or no',
  ReportFieldType.REPORT_FIELD_TYPE_NUMBER => 'Number',
  ReportFieldType.REPORT_FIELD_TYPE_DATE => 'Date',
  ReportFieldType.REPORT_FIELD_TYPE_VERSION => 'Claktory version',
  ReportFieldType.REPORT_FIELD_TYPE_ATTACHMENTS => 'Files',
  _ => 'Text',
};

/// A list's choice as the person reads it: "data_loss" as "Data loss",
/// "iphone" as "iPhone".
String choiceLabel(String id) {
  const names = {'iphone': 'iPhone', 'ipad': 'iPad', 'macos': 'macOS'};
  if (names[id] case final name?) return name;
  final words = id.replaceAll('_', ' ').trim();
  if (words.isEmpty) return id;
  return words[0].toUpperCase() + words.substring(1);
}

/// Whether this Claktory keeps kinds of report as data: its rules are
/// there (an older one, or one still starting them, has none).
bool reportTypesAvailable(Workspace workspace) =>
    workspace.hasReportTypeRules() && workspace.reportTypeRules.fields > 0;

/// Whether a field of [kind] holds the other side's own words, so reading
/// it marks the reader: as Claktory's rules say (Workspace's
/// report_type_rules), or, from a Claktory without them, as [fact] says.
bool holdsTheirWords(
  Workspace workspace,
  ReportFieldType kind, {
  bool fact = false,
}) {
  final kinds = workspace.reportTypeRules.textKinds;
  if (kinds.isEmpty) return !fact;
  return kinds.contains(kind);
}

/// The kind [id] at [version] (0 for the current one): from [older] (a
/// page's definitions of versions no longer current), else the current
/// one, which stands in for a version the app wasn't given.
ReportSchema? reportTypeOf(
  Workspace workspace,
  String id, {
  int version = 0,
  Iterable<ReportSchema> older = const [],
}) {
  final wanted = id.isEmpty ? 'bug_report' : id;
  if (version > 0) {
    for (final type in older) {
      if (type.id == wanted && type.version == version) return type;
    }
  }
  for (final type in workspace.reportSchemas) {
    if (type.id == wanted) return type;
  }
  for (final type in older) {
    if (type.id == wanted) return type;
  }
  return null;
}

/// The kinds [forward] lets its pipe's agent forward, by id: its list, or
/// from an older Claktory its one kind.
List<String> forwardTypeIds(PipeForward forward) => forward.typeIds.isNotEmpty
    ? forward.typeIds
    : [forward.schemaId.isEmpty ? 'bug_report' : forward.schemaId];

/// How many pipes forward [id], or would once they forward to an agent.
int reportTypeUses(Workspace workspace, String id) {
  var uses = 0;
  for (final pipe in workspace.pipes) {
    if (pipe.hasForward() && forwardTypeIds(pipe.forward).contains(id)) {
      uses++;
    }
  }
  return uses;
}

/// Where [id] is still on, as Delete's refusal names them: open pipes
/// that forward it (by name) and projects whose new pipes would ("Garden's
/// new pipes").
List<String> reportTypePlaces(Workspace workspace, String id) => [
  for (final pipe in workspace.pipes)
    if (pipe.state != PipeState.PIPE_STATE_CLOSED &&
        pipe.hasForward() &&
        forwardTypeIds(pipe.forward).contains(id))
      pipe.name.isEmpty ? pipe.id : pipe.name,
  for (final project in workspace.projects)
    if (project.hasReportForward() &&
        forwardTypeIds(project.reportForward).contains(id))
      "${project.name}'s new pipes",
];

/// "a", "a and b", "a, b and c".
String _joined(List<String> words) => words.length < 2
    ? words.join()
    : '${words.take(words.length - 1).join(', ')} and ${words.last}';

/// "4 fields · v2", with "on 2 pipes" while pipes forward it.
String reportTypeSummary(Workspace workspace, ReportSchema type) {
  final fields = type.fields.length;
  final uses = reportTypeUses(workspace, type.id);
  return [
    '$fields ${fields == 1 ? 'field' : 'fields'}',
    if (type.version > 0) 'v${type.version}',
    if (uses > 0) 'used by $uses ${uses == 1 ? 'pipe' : 'pipes'}',
  ].join(' · ');
}

/// "Accepted, Done and Needs more".
String outcomesText(Iterable<PipeReportState> outcomes) {
  final words = [
    for (final state in reportOutcomes)
      if (outcomes.contains(state)) outcomeLabel(state),
  ];
  return words.length < 2
      ? words.join()
      : '${words.sublist(0, words.length - 1).join(', ')} and ${words.last}';
}

/// What crosses with a kind of report: each field's label and rule, marked
/// "Their words" (with the portal: reading it marks the reader) or "A
/// choice" (it marks no one), [widened] ones flagged, then what its reader
/// may answer. [before], when given, tells a new field from a widened one.
class ReportTypeFields extends StatelessWidget {
  const ReportTypeFields({
    super.key,
    required this.type,
    this.widened = const {},
    this.before,
    this.outcomes = true,
  });

  final ReportSchema type;
  final Set<String> widened;
  final ReportSchema? before;
  final bool outcomes;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final textStyle = TypeScale.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final before = this.before;
    final gone = before == null
        ? const <ReportField>[]
        : [
            for (final field in before.fields)
              if (!type.fields.any((each) => each.name == field.name)) field,
          ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final field in type.fields)
          Padding(
            padding: const EdgeInsets.only(bottom: Space.xs + 2),
            child: _FieldLine(
              field: field,
              theirWords: holdsTheirWords(
                workspace,
                field.type,
                fact: field.fact,
              ),
              flag: !widened.contains(field.name)
                  ? null
                  : before != null &&
                        !before.fields.any((each) => each.name == field.name)
                  ? 'New'
                  : 'Wider',
            ),
          ),
        for (final field in gone)
          Padding(
            padding: const EdgeInsets.only(bottom: Space.xs + 2),
            child: Text(
              '${field.label.isEmpty ? field.name : field.label}  Removed',
              style: textStyle.caption.copyWith(
                color: palette.textFaint,
                decoration: TextDecoration.lineThrough,
              ),
            ),
          ),
        if (outcomes && type.outcomes.isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          Text(
            'Its reader can answer: ${outcomesText(type.outcomes)}.',
            style: textStyle.caption.copyWith(color: palette.textMuted),
          ),
        ],
      ],
    );
  }
}

/// One field: its label and rule, then whose words it holds.
class _FieldLine extends StatelessWidget {
  const _FieldLine({required this.field, required this.theirWords, this.flag});

  final ReportField field;
  final bool theirWords;

  /// "New" or "Wider", for a field an edit widened.
  final String? flag;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final flag = this.flag;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text.rich(
          TextSpan(
            children: [
              TextSpan(
                text: field.label.isEmpty ? field.name : field.label,
                style: TextStyle(color: palette.text),
              ),
              if (field.required)
                TextSpan(
                  text: '  Required',
                  style: TextStyle(color: palette.textMuted),
                ),
              if (field.rule.isNotEmpty)
                TextSpan(
                  text: '  ${field.rule}',
                  style: TextStyle(color: palette.textFaint),
                ),
              if (flag != null)
                TextSpan(
                  text: '  $flag',
                  style: TextStyle(
                    color: palette.signal,
                    fontWeight: FontWeight.w600,
                  ),
                ),
            ],
          ),
          style: type.caption,
        ),
        WordsMark(theirWords: theirWords),
      ],
    );
  }
}

/// "Their words · marks its reader" with the portal, or "A choice · marks
/// no one".
class WordsMark extends StatelessWidget {
  const WordsMark({super.key, required this.theirWords});

  final bool theirWords;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (theirWords) ...[
          const ExcludeSemantics(
            child: PortalGlyph(color: pipeColor, size: 10),
          ),
          const SizedBox(width: Space.xs),
        ],
        Flexible(
          child: Text(
            theirWords
                ? 'Their words · like reading their messages'
                : 'Safe to read · can’t carry their words',
            style: type.caption.copyWith(
              color: theirWords ? palette.textMuted : palette.textFaint,
            ),
          ),
        ),
      ],
    );
  }
}

/// Settings → Pipes: every kind of report a pipe's agent can forward,
/// built-in ones first and marked, each opening to what crosses; New
/// kind….
class ReportTypesSettings extends StatelessWidget {
  const ReportTypesSettings({super.key});

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) {
        final palette = Palette.of(context);
        final type = TypeScale.of(context);
        final workspace = backend.workspace.workspace;
        final types = workspace.reportSchemas;
        final most = workspace.reportTypeRules.types;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // What they're for, and where pipes are made (Lucy's Settings
            // spec §6, ST5); the section's title names them already.
            Text(
              'What a pipe’s agent fills in for each kind of report it passes '
              'on. Pipes themselves are set up in their project.',
              style: type.body.copyWith(color: palette.textMuted),
            ),
            const SizedBox(height: Space.m),
            for (final each in types)
              ReportTypeRow(key: ValueKey(each.id), type: each),
            const SizedBox(height: Space.s),
            Align(
              alignment: Alignment.centerLeft,
              child: Button(
                label: 'New report type…',
                onPressed: most > 0 && types.length >= most
                    ? null
                    : () => editReportType(context),
              ),
            ),
            const SizedBox(height: Space.l),
            Text(
              'A pipe’s agent fills in one of these for each report it '
              'forwards, from what the other side tells it. For the agent '
              'that reads it, a field in their words is like reading their '
              'messages. The other fields can’t carry words, so they’re safe '
              'to read. Choose which report types each pipe forwards in its '
              'settings.',
              style: type.caption.copyWith(color: palette.textFaint),
            ),
          ],
        );
      },
    );
  }
}

/// A kind of report: its name ("Built in" after a built-in one's), how
/// many fields, its version and the pipes that forward it; it opens it.
class ReportTypeRow extends StatelessWidget {
  const ReportTypeRow({super.key, required this.type});

  final ReportSchema type;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final textStyle = TypeScale.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    return Semantics(
      button: true,
      child: Pressable(
        onTap: () => showReportType(context, type),
        builder: (context, hovered) => Container(
          constraints: BoxConstraints(
            minHeight: isPhoneLayout(context) ? Touch.target : 40,
          ),
          padding: const EdgeInsets.symmetric(
            horizontal: Space.s,
            vertical: Space.xs + 2,
          ),
          decoration: BoxDecoration(
            color: hovered ? palette.hover : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text.rich(
                TextSpan(
                  children: [
                    TextSpan(
                      text: type.name,
                      style: TextStyle(color: palette.text),
                    ),
                    if (type.builtin)
                      TextSpan(
                        text: '  Built in',
                        style: TextStyle(color: palette.textFaint),
                      ),
                  ],
                ),
                style: textStyle.body,
              ),
              Text(
                reportTypeSummary(workspace, type),
                style: textStyle.caption.copyWith(color: palette.textMuted),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A kind of report on its own: what crosses, who last changed it, and
/// Edit… or Delete….
Future<void> showReportType(BuildContext context, ReportSchema type) =>
    showModal(
      context,
      (context, close) => ReportTypeSheet(type: type, close: close),
    );

class ReportTypeSheet extends StatefulWidget {
  const ReportTypeSheet({super.key, required this.type, required this.close});

  final ReportSchema type;
  final VoidCallback close;

  @override
  State<ReportTypeSheet> createState() => _ReportTypeSheetState();
}

class _ReportTypeSheetState extends State<ReportTypeSheet> {
  var _deleting = false;
  var _busy = false;
  String? _error;

  Future<void> _delete() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).deleteReportType(widget.type.id);
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => _sheet(context, store.workspace),
    );
  }

  Widget _sheet(BuildContext context, Workspace workspace) {
    final palette = Palette.of(context);
    final textStyle = TypeScale.of(context);
    // The version as it is now, once an edit is saved.
    final type =
        workspace.reportSchemas
            .where((each) => each.id == widget.type.id)
            .firstOrNull ??
        widget.type;
    // Delete is refused while it's on anywhere: the confirm says where,
    // rather than promising pipes stop forwarding it (Zucchero's T6 nit).
    final places = reportTypePlaces(workspace, type.id);
    return FormFrame(
      title: type.name,
      body: reportTypeChangedText(workspace, type),
      fields: [
        ReportTypeFields(type: type, widened: {...type.widenedFields}),
        if (_deleting) ...[
          const SizedBox(height: Space.m),
          Container(
            padding: const EdgeInsets.all(Space.m),
            decoration: BoxDecoration(
              border: Border.all(color: palette.hairline),
              borderRadius: BorderRadius.circular(Radii.panel),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  'Delete “${type.name}”?',
                  style: textStyle.emphasis.copyWith(color: palette.text),
                ),
                const SizedBox(height: Space.xs),
                Text(
                  places.isEmpty
                      ? 'No pipe forwards it. Reports already forwarded keep '
                            'it.'
                      : '“${type.name}” is still forwarded by '
                            '${_joined(places)}. Turn it off there first.',
                  style: textStyle.body.copyWith(color: palette.textMuted),
                ),
                const SizedBox(height: Space.m),
                Wrap(
                  alignment: WrapAlignment.end,
                  spacing: Space.s,
                  runSpacing: Space.xs,
                  children: [
                    Button(
                      label: 'Cancel',
                      onPressed: _busy
                          ? null
                          : () => setState(() => _deleting = false),
                    ),
                    Button(
                      label: 'Delete',
                      primary: true,
                      onPressed: _busy || places.isNotEmpty ? null : _delete,
                    ),
                  ],
                ),
              ],
            ),
          ),
        ],
      ],
      error: _error,
      secondary: Wrap(
        spacing: Space.s,
        runSpacing: Space.xs,
        children: [
          Button(
            label: 'Edit…',
            onPressed: () {
              widget.close();
              unawaited(editReportType(context, type: type));
            },
          ),
          if (!type.builtin)
            Button(
              label: 'Delete…',
              onPressed: _deleting || _busy
                  ? null
                  : () => setState(() {
                      _deleting = true;
                      _error = null;
                    }),
            ),
        ],
      ),
      submitLabel: 'Done',
      onSubmit: widget.close,
      onCancel: widget.close,
    );
  }
}

/// "Version 2 · built in · changed by Rivet 3h ago".
String reportTypeChangedText(Workspace workspace, ReportSchema type) {
  String? by;
  if (type.byId.isNotEmpty) {
    by =
        workspace.agents
            .where((agent) => agent.id == type.byId)
            .firstOrNull
            ?.name ??
        'an agent that’s gone';
  }
  final at = type.atMs.toInt();
  final changed = type.version > 1 && at > 0
      ? 'changed ${by == null ? '' : 'by $by '}'
            '${formatAge(at, DateTime.now())}'
      : type.version <= 1 && by != null
      ? 'made by $by'
      : '';
  return [
    if (type.version > 0) 'Version ${type.version}',
    if (type.builtin) 'built in',
    if (changed.isNotEmpty) changed,
  ].join(' · ');
}

/// The editor: a new kind, or (with [type]) a new version of one.
Future<void> editReportType(BuildContext context, {ReportSchema? type}) =>
    showModal(
      context,
      (context, close) => ReportTypeEditor(type: type, close: close),
    );

/// One field as it's being edited.
class _FieldDraft {
  _FieldDraft([ReportField? field])
    : name = field?.name ?? '',
      kind =
          field == null ||
              field.type == ReportFieldType.REPORT_FIELD_TYPE_UNSPECIFIED
          ? ReportFieldType.REPORT_FIELD_TYPE_TEXT
          : field.type,
      required = field?.required ?? false,
      choices = [...?field?.choices],
      label = TextEditingController(text: field?.label ?? ''),
      hint = TextEditingController(
        text: field != null && field.ruleWritten ? field.rule : '',
      ),
      bytes = TextEditingController(
        text: field == null || field.maxBytes == 0 ? '' : '${field.maxBytes}',
      ),
      items = TextEditingController(
        text: field == null || field.maxItems == 0 ? '' : '${field.maxItems}',
      ),
      min = TextEditingController(
        text: field != null && field.hasMin() ? '${field.min}' : '',
      ),
      max = TextEditingController(
        text: field != null && field.hasMax() ? '${field.max}' : '',
      );

  /// Its id, kept across versions; '' for a new one (Claktory makes it
  /// from the label).
  final String name;
  ReportFieldType kind;
  bool required;
  final List<String> choices;
  final TextEditingController label;
  final TextEditingController hint;
  final TextEditingController bytes;
  final TextEditingController items;
  final TextEditingController min;
  final TextEditingController max;
  final choice = TextEditingController();

  /// A stable key while fields are added and removed.
  final key = UniqueKey();

  List<TextEditingController> get controllers => [
    label,
    hint,
    bytes,
    items,
    min,
    max,
    choice,
  ];

  void dispose() {
    for (final controller in controllers) {
      controller.dispose();
    }
  }

  /// As sent: caps clamped to [rules]' ceilings.
  ReportField field(ReportTypeRules rules) {
    int capped(TextEditingController controller, int ceiling) {
      final value = int.tryParse(controller.text.trim()) ?? 0;
      if (value <= 0) return 0;
      return ceiling > 0 && value > ceiling ? ceiling : value;
    }

    Int64? bound(TextEditingController controller) {
      final value = int.tryParse(controller.text.trim());
      if (value == null) return null;
      final most = rules.numberAbs.toInt();
      if (most > 0 && value.abs() > most) {
        return Int64(value < 0 ? -most : most);
      }
      return Int64(value);
    }

    final field = ReportField(
      name: name,
      label: label.text.trim(),
      type: kind,
      required: required,
      rule: hint.text.trim(),
    );
    switch (kind) {
      case ReportFieldType.REPORT_FIELD_TYPE_TEXT:
        field.maxBytes = capped(bytes, rules.textBytes);
      case ReportFieldType.REPORT_FIELD_TYPE_TEXT_LIST:
        field.maxBytes = capped(bytes, rules.itemBytes);
        field.maxItems = capped(items, rules.listItems);
      case ReportFieldType.REPORT_FIELD_TYPE_ATTACHMENTS:
        field.maxItems = capped(items, rules.attachments);
      case ReportFieldType.REPORT_FIELD_TYPE_ENUM:
        field.choices.addAll(choices);
      case ReportFieldType.REPORT_FIELD_TYPE_NUMBER:
        if (bound(min) case final low?) field.min = low;
        if (bound(max) case final high?) field.max = high;
      default:
    }
    return field;
  }
}

/// [text] as an id: lowercase letters, digits and _, as Claktory makes
/// ids from labels and keeps a list's choices.
String reportId(String text) => text
    .trim()
    .toLowerCase()
    .replaceAll(RegExp('[^a-z0-9]+'), '_')
    .replaceAll(RegExp('^_+|_+\$'), '');

/// Whether a field's [id] reads as where a report goes, as Claktory
/// refuses it: a reserved id, a routing word (or its plural) between its
/// _s ("cc_list", "web_urls"), or ending in _to.
bool readsAsRouting(String id, ReportTypeRules rules) {
  if (id.isEmpty) return false;
  if (rules.reservedIds.contains(id)) return true;
  // A routing word, or its plural ("urls", "recipients").
  bool routing(String word) =>
      rules.routingTokens.contains(word) ||
      (word.endsWith('s') &&
          rules.routingTokens.contains(word.substring(0, word.length - 1)));
  if (id.split('_').any(routing)) return true;
  return id == 'to' || id.endsWith('_to');
}

/// Keeps what's typed to whole numbers no larger than [most] (0 for no
/// limit), and with [signed] a leading minus.
class _Whole extends TextInputFormatter {
  const _Whole({this.most = 0, this.signed = false});

  final int most;
  final bool signed;

  @override
  TextEditingValue formatEditUpdate(
    TextEditingValue oldValue,
    TextEditingValue newValue,
  ) {
    final text = newValue.text;
    if (text.isEmpty || (signed && text == '-')) return newValue;
    if (!RegExp(signed ? r'^-?[0-9]+$' : r'^[0-9]+$').hasMatch(text)) {
      return oldValue;
    }
    final value = int.tryParse(text);
    if (value == null || (most > 0 && value.abs() > most)) {
      // Past the ceiling: the ceiling.
      final capped = '${value != null && value < 0 ? '-' : ''}$most';
      return TextEditingValue(
        text: capped,
        selection: TextSelection.collapsed(offset: capped.length),
      );
    }
    return newValue;
  }
}

class ReportTypeEditor extends StatefulWidget {
  const ReportTypeEditor({super.key, this.type, required this.close});

  /// The kind to edit; null for a new one.
  final ReportSchema? type;
  final VoidCallback close;

  @override
  State<ReportTypeEditor> createState() => _ReportTypeEditorState();
}

class _ReportTypeEditorState extends State<ReportTypeEditor> {
  late final _name = TextEditingController(text: widget.type?.name ?? '');
  late final List<_FieldDraft> _fields = [
    for (final field in widget.type?.fields ?? const <ReportField>[])
      _FieldDraft(field),
  ];
  late final Set<PipeReportState> _outcomes = widget.type == null
      ? {
          PipeReportState.PIPE_REPORT_STATE_ACCEPTED,
          PipeReportState.PIPE_REPORT_STATE_DONE,
          PipeReportState.PIPE_REPORT_STATE_NEEDS_INFO,
        }
      : {...widget.type!.outcomes};
  var _dirty = false;
  var _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _name.addListener(_changed);
    for (final field in _fields) {
      _listen(field);
    }
  }

  void _listen(_FieldDraft field) {
    for (final controller in field.controllers) {
      controller.addListener(_changed);
    }
  }

  /// Marks it changed, and redraws what crosses.
  void _changed() {
    if (mounted) setState(() => _dirty = true);
  }

  @override
  void dispose() {
    _name.dispose();
    for (final field in _fields) {
      field.dispose();
    }
    super.dispose();
  }

  ReportSchema _draft(ReportTypeRules rules) => ReportSchema(
    id: widget.type?.id ?? '',
    name: _name.text.trim(),
    fields: [for (final field in _fields) field.field(rules)],
    outcomes: [
      for (final state in reportOutcomes)
        if (_outcomes.contains(state)) state,
    ],
  );

  Future<void> _save(ReportTypeRules rules) async {
    if (_name.text.trim().isEmpty) {
      setState(() => _error = 'Give it a name.');
      return;
    }
    if (_fields.isEmpty) {
      setState(() => _error = 'Add at least one field.');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).saveReportType(
        SaveReportType(
          type: _draft(rules),
          baseVersion: widget.type?.version ?? 0,
        ),
      );
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// How many fields are of [kind].
  int _count(ReportFieldType kind) =>
      _fields.where((field) => field.kind == kind).length;

  void _add() => setState(() {
    final field = _FieldDraft();
    _fields.add(field);
    _listen(field);
    _dirty = true;
  });

  void _remove(_FieldDraft field) => setState(() {
    _fields.remove(field);
    field.dispose();
    _dirty = true;
  });

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final textStyle = TypeScale.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final rules = workspace.reportTypeRules;
    final type = widget.type;
    final draft = _draft(rules);
    final words = [
      for (final field in draft.fields) holdsTheirWords(workspace, field.type),
    ];
    final choices = words.where((each) => !each).length;
    return FormFrame(
      title: type == null ? 'New report type' : 'Edit “${type.name}”',
      body: type == null
          ? null
          : 'Saving makes version ${type.version + 1}. Reports already '
                'forwarded keep the version they were written in.',
      dirty: _dirty,
      fields: [
        Field(
          label: 'Name',
          child: TextField(
            controller: _name,
            placeholder: 'Task request',
            kind: InputKind.text,
            inputFormatters: [if (rules.nameChars > 0) _Most(rules.nameChars)],
          ),
        ),
        Field(
          label: 'Fields',
          note:
              '${_fields.length} of ${rules.fields} fields · $choices of '
              '${rules.facts} safe to read',
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final field in _fields)
                Padding(
                  key: field.key,
                  padding: const EdgeInsets.only(bottom: Space.s),
                  child: _FieldEditor(
                    draft: field,
                    rules: rules,
                    theirWords: holdsTheirWords(workspace, field.kind),
                    full: {
                      if (rules.numbers > 0 &&
                          _count(ReportFieldType.REPORT_FIELD_TYPE_NUMBER) >=
                              rules.numbers)
                        ReportFieldType.REPORT_FIELD_TYPE_NUMBER,
                      if (rules.dates > 0 &&
                          _count(ReportFieldType.REPORT_FIELD_TYPE_DATE) >=
                              rules.dates)
                        ReportFieldType.REPORT_FIELD_TYPE_DATE,
                    },
                    onChanged: _changed,
                    onRemove: () => _remove(field),
                  ),
                ),
              Align(
                alignment: Alignment.centerLeft,
                child: Button(
                  label: 'Add a field',
                  onPressed: rules.fields > 0 && _fields.length >= rules.fields
                      ? null
                      : _add,
                ),
              ),
            ],
          ),
        ),
        Field(
          label: 'Its reader can answer',
          note: 'The other side sees only these, never the reader’s own words.',
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final state in reportOutcomes)
                _Check(
                  label: outcomeLabel(state),
                  on: _outcomes.contains(state),
                  onChanged: (on) => setState(() {
                    on ? _outcomes.add(state) : _outcomes.remove(state);
                    _dirty = true;
                  }),
                ),
            ],
          ),
        ),
        Field(
          label: 'What comes through',
          child: draft.fields.isEmpty
              ? Text(
                  'Nothing yet.',
                  style: textStyle.caption.copyWith(color: palette.textFaint),
                )
              : ReportTypeFields(type: draft),
        ),
      ],
      error: _error,
      submitLabel: _busy ? 'Saving…' : 'Save',
      onSubmit: _busy ? null : () => _save(rules),
      onCancel: widget.close,
    );
  }
}

/// Keeps what's typed to [most] characters.
class _Most extends TextInputFormatter {
  const _Most(this.most);

  final int most;

  @override
  TextEditingValue formatEditUpdate(
    TextEditingValue oldValue,
    TextEditingValue newValue,
  ) => newValue.text.characters.length > most ? oldValue : newValue;
}

/// One field in the editor, in a hairline box: its label and kind (and
/// Remove), then Required and its caps, a list's choices, a number's range,
/// an optional hint, and whose words it holds.
class _FieldEditor extends StatelessWidget {
  const _FieldEditor({
    required this.draft,
    required this.rules,
    required this.theirWords,
    required this.onChanged,
    required this.onRemove,
    this.full = const {},
  });

  /// Kinds the report type has as many of as it may (numbers, dates): not
  /// offered to a field of another kind.
  final Set<ReportFieldType> full;

  final _FieldDraft draft;
  final ReportTypeRules rules;
  final bool theirWords;
  final VoidCallback onChanged;
  final VoidCallback onRemove;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final phone = isPhoneLayout(context);
    final id = draft.name.isEmpty ? reportId(draft.label.text) : draft.name;
    final routing = draft.name.isEmpty && readsAsRouting(id, rules);
    final caption = type.caption.copyWith(color: palette.textMuted);
    Widget number(
      TextEditingController controller,
      String placeholder, {
      int most = 0,
      bool signed = false,
    }) => SizedBox(
      // Room for -1000000000 in a number's range.
      width: signed ? 124 : (phone ? 96 : 84),
      child: TextField(
        controller: controller,
        placeholder: placeholder,
        kind: InputKind.number,
        inputFormatters: [_Whole(most: most, signed: signed)],
      ),
    );
    final kindSelect = Select<ReportFieldType>(
      value: draft.kind,
      title: 'Kind',
      options: [
        for (final kind in fieldKinds)
          if (kind == draft.kind || !full.contains(kind))
            SelectOption(kind, fieldKindLabel(kind)),
      ],
      onChanged: (kind) {
        draft.kind = kind;
        onChanged();
      },
    );
    final label = TextField(
      controller: draft.label,
      placeholder: 'Label',
      kind: InputKind.text,
      semanticsLabel: 'Field label',
      inputFormatters: [if (rules.labelChars > 0) _Most(rules.labelChars)],
    );
    final remove = GlyphButton(
      label: 'Remove field',
      active: false,
      onTap: onRemove,
      glyph: (color) => CloseGlyph(color: color),
    );
    final most = rules.numberAbs.toInt();
    return Container(
      padding: const EdgeInsets.all(Space.s),
      decoration: BoxDecoration(
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // On a phone the kind goes under the label, the row too narrow
          // for both.
          if (phone) ...[
            Row(
              children: [
                Expanded(child: label),
                const SizedBox(width: Space.xs),
                remove,
              ],
            ),
            const SizedBox(height: Space.xs),
            kindSelect,
          ] else
            Row(
              children: [
                Expanded(flex: 3, child: label),
                const SizedBox(width: Space.s),
                Expanded(flex: 2, child: kindSelect),
                const SizedBox(width: Space.xs),
                remove,
              ],
            ),
          if (routing) ...[
            const SizedBox(height: Space.xs),
            Text(
              'A field can’t be called “${draft.label.text.trim()}”: it '
              'would read as where the report goes.',
              style: type.caption.copyWith(color: palette.danger),
            ),
          ],
          const SizedBox(height: Space.xs + 2),
          Wrap(
            spacing: Space.s,
            runSpacing: Space.xs,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              _Check(
                label: 'Required',
                on: draft.required,
                onChanged: (on) {
                  draft.required = on;
                  onChanged();
                },
              ),
              ...switch (draft.kind) {
                ReportFieldType.REPORT_FIELD_TYPE_TEXT => [
                  Text('Up to', style: caption),
                  number(
                    draft.bytes,
                    '${rules.textBytes}',
                    most: rules.textBytes,
                  ),
                  Text('bytes', style: caption),
                ],
                ReportFieldType.REPORT_FIELD_TYPE_TEXT_LIST => [
                  Text('Up to', style: caption),
                  number(
                    draft.items,
                    '${rules.listItems}',
                    most: rules.listItems,
                  ),
                  Text('items of up to', style: caption),
                  number(
                    draft.bytes,
                    '${rules.itemBytes}',
                    most: rules.itemBytes,
                  ),
                  Text('bytes', style: caption),
                ],
                ReportFieldType.REPORT_FIELD_TYPE_ATTACHMENTS => [
                  Text('Up to', style: caption),
                  number(
                    draft.items,
                    '${rules.attachments}',
                    most: rules.attachments,
                  ),
                  Text('files', style: caption),
                ],
                ReportFieldType.REPORT_FIELD_TYPE_NUMBER => [
                  Text('From', style: caption),
                  // Unset, a number goes from 0 to 1,000,000.
                  number(draft.min, '0', most: most, signed: true),
                  Text('to', style: caption),
                  number(draft.max, '1000000', most: most, signed: true),
                ],
                _ => const <Widget>[],
              },
            ],
          ),
          if (draft.kind == ReportFieldType.REPORT_FIELD_TYPE_ENUM) ...[
            const SizedBox(height: Space.xs + 2),
            _Choices(draft: draft, rules: rules, onChanged: onChanged),
          ],
          const SizedBox(height: Space.xs + 2),
          TextField(
            controller: draft.hint,
            placeholder: 'Hint for the pipe’s agent (optional)',
            kind: InputKind.text,
            inputFormatters: [if (rules.ruleChars > 0) _Most(rules.ruleChars)],
          ),
          const SizedBox(height: Space.xs),
          WordsMark(theirWords: theirWords),
        ],
      ),
    );
  }
}

/// A list's choices as chips, each with ×, and a field that adds one
/// (as an id: "on_track").
class _Choices extends StatelessWidget {
  const _Choices({
    required this.draft,
    required this.rules,
    required this.onChanged,
  });

  final _FieldDraft draft;
  final ReportTypeRules rules;
  final VoidCallback onChanged;

  void _add() {
    final id = reportId(draft.choice.text);
    final full = rules.choices > 0 && draft.choices.length >= rules.choices;
    if (id.isEmpty || id.length > 40 || full || draft.choices.contains(id)) {
      return;
    }
    draft.choices.add(id);
    draft.choice.clear();
    onChanged();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final full = rules.choices > 0 && draft.choices.length >= rules.choices;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (draft.choices.isNotEmpty) ...[
          Wrap(
            spacing: Space.xs,
            runSpacing: Space.xs,
            children: [
              for (final choice in draft.choices)
                _ChoiceChip(
                  label: choice,
                  onRemove: () {
                    draft.choices.remove(choice);
                    onChanged();
                  },
                ),
            ],
          ),
          const SizedBox(height: Space.xs),
        ],
        if (!full)
          Row(
            children: [
              Expanded(
                child: TextField(
                  controller: draft.choice,
                  placeholder: 'Add a choice',
                  kind: InputKind.text,
                  onSubmitted: (_) => _add(),
                ),
              ),
              const SizedBox(width: Space.xs),
              Button(label: 'Add', onPressed: _add),
            ],
          ),
        Text(
          '${draft.choices.length} of ${rules.choices} choices',
          style: type.caption.copyWith(color: palette.textFaint),
        ),
      ],
    );
  }
}

/// A choice as a pill: its id, then × to remove it.
class _ChoiceChip extends StatelessWidget {
  const _ChoiceChip({required this.label, required this.onRemove});

  final String label;
  final VoidCallback onRemove;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      height: 26,
      padding: const EdgeInsets.only(left: Space.s, right: 2),
      decoration: BoxDecoration(
        color: palette.selected,
        borderRadius: BorderRadius.circular(13),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            choiceLabel(label),
            style: TypeScale.of(context).caption.copyWith(color: palette.text),
          ),
          Semantics(
            button: true,
            label: 'Remove ${choiceLabel(label)}',
            onTap: onRemove,
            excludeSemantics: true,
            child: Pressable(
              onTap: onRemove,
              builder: (context, hovered) => Padding(
                padding: const EdgeInsets.all(3),
                child: SizedBox.square(
                  dimension: 16,
                  child: CloseGlyph(
                    color: hovered ? palette.text : palette.textFaint,
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// A tick and its label, as one target.
class _Check extends StatelessWidget {
  const _Check({
    required this.label,
    required this.on,
    required this.onChanged,
    this.held = false,
  });

  final String label;
  final bool on;
  final ValueChanged<bool>? onChanged;

  /// Can't be changed now, but looks as it is: the last kind a pipe
  /// forwards stays ticked.
  final bool held;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final onChanged = this.onChanged;
    return Semantics(
      checked: on,
      label: label,
      excludeSemantics: true,
      child: Pressable(
        onTap: onChanged == null ? null : () => onChanged(!on),
        builder: (context, hovered) => ConstrainedBox(
          constraints: BoxConstraints(
            minHeight: isPhoneLayout(context) ? Touch.target : 24,
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              CheckMark(on: on, dim: onChanged == null && !held),
              const SizedBox(width: Space.s),
              Flexible(
                child: Text(
                  label,
                  style: TypeScale.of(context).body.copyWith(
                    color: onChanged == null && !held
                        ? palette.textFaint
                        : palette.text,
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A tick per kind of report, for "Can forward": as many as Claktory lets
/// a pipe forward, never none.
class ReportTypeChecks extends StatelessWidget {
  const ReportTypeChecks({
    super.key,
    required this.chosen,
    required this.onChanged,
    this.busy = false,
  });

  final List<String> chosen;
  final ValueChanged<List<String>> onChanged;
  final bool busy;

  @override
  Widget build(BuildContext context) {
    final workspace = AppScope.of(context).workspace.workspace;
    final most = workspace.reportTypeRules.perPipe;
    final types = workspace.reportSchemas;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final type in types)
          _Check(
            label: type.name,
            on: chosen.contains(type.id),
            held: !busy && chosen.length == 1 && chosen.contains(type.id),
            onChanged:
                busy ||
                    // The last one can't be unticked; past the most, no
                    // more can be ticked.
                    (chosen.contains(type.id)
                        ? chosen.length <= 1
                        : most > 0 && chosen.length >= most)
                ? null
                : (on) => onChanged([
                    for (final each in types)
                      if (each.id == type.id ? on : chosen.contains(each.id))
                        each.id,
                  ]),
          ),
      ],
    );
  }
}

/// A History record about kinds of report: a kind made, changed or
/// deleted (REPORT_TYPE), with what crosses and, for a change that lets
/// more cross, what widened; or the kinds a pipe was set to forward
/// (PIPE_FORWARD), each with its fields and caps.
Future<void> showReportTypeRecord(
  BuildContext context,
  NeedsYouRecord record,
) => showModal(
  context,
  (context, close) => ReportTypeRecordView(record: record, close: close),
);

/// Whether [record] has kinds of report to show.
bool recordHasReportTypes(NeedsYouRecord record) =>
    record.hasReportType() || record.reportTypes.isNotEmpty;

class ReportTypeRecordView extends StatelessWidget {
  const ReportTypeRecordView({
    super.key,
    required this.record,
    required this.close,
  });

  final NeedsYouRecord record;
  final VoidCallback close;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final textStyle = TypeScale.of(context);
    final types = record.hasReportType()
        ? [record.reportType]
        : record.reportTypes;
    final before = record.hasReportTypeBefore()
        ? record.reportTypeBefore
        : null;
    return FormFrame(
      title: record.title.isEmpty ? 'Report types' : record.title,
      body: record.widens
          ? 'This change lets more of the other side’s words through.'
          : null,
      fields: [
        for (final (i, type) in types.indexed) ...[
          Padding(
            padding: EdgeInsets.only(top: i == 0 ? 0 : Space.m, bottom: 2),
            child: Text(
              [
                type.name,
                if (type.version > 0) 'version ${type.version}',
              ].join(' · '),
              style: textStyle.caption.copyWith(
                color: palette.text,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          ReportTypeFields(
            type: type,
            widened: {...type.widenedFields},
            before: before?.id == type.id ? before : null,
          ),
        ],
      ],
      error: null,
      submitLabel: 'Done',
      onSubmit: close,
      onCancel: close,
    );
  }
}
