import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import '../attachments.dart' show pickAttachments, pickFiles;
import '../backend.dart';
import '../claktory_identity.dart' show claktoryName;
import '../controls.dart';
import '../forms.dart';
import '../identity.dart';
import '../message_time.dart' show dayLine;
import '../mobile/metrics.dart';
import '../mobile/navigation.dart' show PhoneRoutes;
import '../mobile/platform_policy.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import '../settings_view.dart' show modelLabel;
import 'handoff.dart';
import 'claude_export.dart';
import 'import_worker.dart';

// Bringing in conversations from a Claude export (Track B): how to get
// the export, choosing it, reading it on this device and a preview to
// choose from (B0a); then the agent to give the chosen ones to, what goes
// where, and handing them over (B0b, handoff.dart). Only the chosen
// conversations leave the device. Hidden: nothing in the app opens it
// yet.

/// Opens the import: how to get a Claude export, then choosing one, then
/// its preview.
Future<void> showImportExport(BuildContext context) =>
    showModal(context, (context, close) => ImportExportForm(close: close));

/// Claude's privacy settings, where its export is asked for.
final claudePrivacySettings = Uri.parse(
  'https://claude.ai/settings/data-privacy-controls',
);

/// Opens a page in the browser; tests set their own.
Future<bool> Function(Uri url) openPrivacyPage = launchUrl;

/// Asks for the export file and answers its path; null when cancelled.
Future<String?> pickExportFile() async {
  final pick = exportFilePicker;
  if (pick != null) return pick();
  // A touch device's own picker (iPhone, iPad, Android), as the message
  // box chooses; a Mac's open panel.
  final paths = PhonePlatformPolicy.current.touch
      ? await pickAttachments()
      : await pickFiles();
  return paths.isEmpty ? null : paths.first;
}

/// Stands in for the system's file picker in tests.
Future<String?> Function()? exportFilePicker;

/// Reads the chosen file; tests set their own.
ExportJob Function(String path) exportReader = readExportFile;

/// Makes the files for the chosen conversations; tests set their own.
ImportJob<List<ImportArtifact>> Function(
  String path,
  ExportPreview preview,
  List<String> ids,
)
importPreparer = (path, preview, ids) => prepareImport(
  path,
  exportSha256: preview.sha256,
  ids: ids,
  nowMs: DateTime.now().millisecondsSinceEpoch,
  leftOut: (
    account: preview.account,
    voice: preview.voice,
    other: preview.other,
  ),
);

/// What the person reads for each [ExportProblem].
String exportProblemText(ExportProblem problem) => switch (problem) {
  ExportProblem.tooBig =>
    'This export is bigger than Claktory can read (100 MB at most).',
  ExportProblem.unsupported =>
    "Claktory doesn't recognize this file as a Claude export.",
  ExportProblem.unsafe =>
    "This file can't be read safely, so Claktory didn't open it.",
  ExportProblem.severalPeople =>
    "This export seems to hold several people's conversations, such as an "
        "organization's. Claktory imports only your own.",
};

/// What a preview left out, as its line says it: "Left out: your account
/// details, 3 voice recordings, 2 other files."; null when nothing was.
String? leftOutLine(ExportPreview preview) {
  String count(int n, String one) => '$n $one${n == 1 ? '' : 's'}';
  final parts = [
    if (preview.account) 'your account details',
    if (preview.voice > 0) count(preview.voice, 'voice recording'),
    if (preview.other > 0) count(preview.other, 'other file'),
  ];
  if (parts.isEmpty) return null;
  return 'Left out: ${parts.join(', ')}.';
}

class ImportExportForm extends StatefulWidget {
  const ImportExportForm({super.key, required this.close});

  final VoidCallback close;

  @override
  State<ImportExportForm> createState() => _ImportExportFormState();
}

class _ImportExportFormState extends State<ImportExportForm> {
  ExportJob? _job;
  ExportPreview? _preview;
  String? _error;
  final _chosen = <int>{};
  var _own = false;

  /// The file chosen, read again for the conversations chosen.
  String? _path;

  /// Choosing where they go: none chosen at first.
  var _destination = false;
  String _agentId = '';

  /// Making their files, then handing them over.
  ImportJob<List<ImportArtifact>>? _preparing;
  ImportHandoff? _handoff;

  static const _title = 'Import your Claude conversations';

  @override
  void dispose() {
    _job?.cancel();
    _preparing?.cancel();
    final handoff = _handoff;
    if (handoff != null && handoff.state != HandoffState.done) {
      handoff.cancel();
    }
    handoff?.dispose();
    super.dispose();
  }

  Future<void> _choose() async {
    final path = await pickExportFile();
    if (path == null || !mounted) return;
    final job = exportReader(path);
    setState(() {
      _job = job;
      _path = path;
      _error = null;
    });
    try {
      final preview = await job.result;
      if (!mounted || _job != job) return;
      setState(() {
        _job = null;
        _preview = preview;
        _chosen.clear();
        _own = false;
      });
    } on ExportCancelled {
      // Back to choosing, as it was.
    } on ExportFailure catch (failure) {
      if (!mounted || _job != job) return;
      setState(() {
        _job = null;
        _error = exportProblemText(failure.problem);
      });
    }
  }

  void _stop() {
    _job?.cancel();
    setState(() => _job = null);
  }

  /// Makes the chosen conversations' files, then hands them to the agent.
  Future<void> _give(ExportPreview preview) async {
    final path = _path;
    if (path == null || _agentId.isEmpty) return;
    final ids = [
      for (final i in _chosen.toList()..sort()) preview.conversations[i].id,
    ];
    final job = importPreparer(path, preview, ids);
    setState(() {
      _preparing = job;
      _error = null;
    });
    final List<ImportArtifact> files;
    try {
      files = await job.result;
    } on ExportCancelled {
      return;
    } on ExportFailure catch (failure) {
      if (mounted && _preparing == job) {
        setState(() {
          _preparing = null;
          _error = exportProblemText(failure.problem);
        });
      }
      return;
    }
    if (!mounted || _preparing != job) return;
    final handoff = ImportHandoff(AppScope.of(context), _agentId, files)
      ..addListener(() {
        if (mounted) setState(() {});
      });
    setState(() {
      _preparing = null;
      _handoff = handoff;
    });
    await handoff.run();
  }

  /// Back from handing over, cancelled or failed, to choosing where.
  void _stopHandoff() {
    final handoff = _handoff;
    if (handoff != null && handoff.state != HandoffState.done) {
      handoff.cancel();
    }
    handoff?.dispose();
    setState(() => _handoff = null);
  }

  @override
  Widget build(BuildContext context) {
    if (_job != null) return _reading(context);
    final preview = _preview;
    if (preview == null) return _instructions(context);
    final handoff = _handoff;
    if (handoff != null) return _handing(context, handoff);
    if (_destination) return _where(context, preview);
    return _choosing(context, preview);
  }

  /// Where they go: an agent of this Claktory, none chosen at first, and
  /// what that means: which Claktory gets them, which model reads them.
  Widget _where(BuildContext context, ExportPreview preview) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final workspace = backend.workspace.workspace;
    final agent = workspace.agents
        .where((agent) => agent.id == _agentId)
        .firstOrNull;
    final count = _chosen.length;
    final conversations = '$count conversation${count == 1 ? '' : 's'}';
    return FormFrame(
      title: _title,
      body: 'Choose the agent to give them to.',
      fields: [
        Field(
          label: 'Agent',
          child: Select<String>(
            title: 'Agent',
            value: _agentId,
            options: [
              const SelectOption('', 'Choose an agent'),
              for (final agent in workspace.agents)
                SelectOption(
                  agent.id,
                  agent.role.isEmpty
                      ? agent.name
                      : '${agent.name} (${agent.role})',
                ),
            ],
            onChanged: (id) => setState(() => _agentId = id),
          ),
        ),
        if (agent != null)
          Text(
            destinationLine(
              agent: agent,
              claktory: claktoryName(backend),
              conversations: conversations,
              model: _modelOf(backend, agent),
              provider: _providerOf(workspace, agent),
            ),
            style: type.body.copyWith(color: palette.textMuted),
          ),
        const SizedBox(height: Space.s),
        Text(
          'Nothing else in your export leaves this device.',
          style: type.caption.copyWith(color: palette.textFaint),
        ),
        if (_preparing != null) ...[
          const SizedBox(height: Space.m),
          Text(
            'Getting them ready…',
            style: type.body.copyWith(color: palette.textMuted),
          ),
        ],
        const SizedBox(height: Space.l),
      ],
      error: _error,
      submitLabel: agent == null ? 'Give' : 'Give to ${agent.name}',
      onSubmit: agent == null || _preparing != null
          ? null
          : () => _give(preview),
      cancelLabel: 'Back',
      onCancel: () {
        _preparing?.cancel();
        setState(() {
          _preparing = null;
          _destination = false;
          _error = null;
        });
      },
    );
  }

  /// Handing them over: how far it's got, Cancel; then where they went.
  Widget _handing(BuildContext context, ImportHandoff handoff) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final name =
        backend.workspace.workspace.agents
            .where((agent) => agent.id == handoff.agentId)
            .firstOrNull
            ?.name ??
        'the agent';
    final count = _chosen.length;
    final conversations = '$count conversation${count == 1 ? '' : 's'}';
    final line = switch (handoff.state) {
      HandoffState.uploading =>
        'Uploading $conversations… ${(handoff.progress * 100).floor()}%',
      HandoffState.sending => 'Sending them to $name…',
      HandoffState.done =>
        'Sent to $name. $name will suggest what to set up from them.',
      HandoffState.failed || HandoffState.cancelled => '',
    };
    final done = handoff.state == HandoffState.done;
    final failed = handoff.state == HandoffState.failed;
    return FormFrame(
      title: _title,
      fields: [
        if (line.isNotEmpty)
          Text(line, style: type.body.copyWith(color: palette.text)),
        const SizedBox(height: Space.l),
      ],
      error: failed ? handoff.failure : null,
      submitLabel: done
          ? 'Open $name'
          : failed
          ? 'Try again'
          : 'Give to $name',
      onSubmit: done
          ? () {
              final routes = PhoneRoutes.of(context);
              final scope = AppScope.scope(context);
              widget.close();
              if (routes != null) {
                routes.openConversation(handoff.agentId);
              } else {
                fireAndLog(scope.backend.selectAgent(handoff.agentId));
                scope.history.clear();
                scope.page.value = AppPage.agent;
              }
            }
          : failed
          ? () => unawaited(handoff.run())
          : null,
      cancelLabel: done ? 'Done' : 'Cancel',
      onCancel: done ? widget.close : _stopHandoff,
    );
  }

  Widget _instructions(BuildContext context) {
    final palette = Palette.of(context);
    final style = TypeScale.of(context).body.copyWith(color: palette.text);
    const steps = [
      'In Claude, open Settings → Privacy and click Export data.',
      'Anthropic emails you a link: download the file within 24 hours.',
      'Choose the file here. Claktory reads it on this device and sends '
          'nothing until you choose conversations.',
    ];
    return FormFrame(
      title: _title,
      fields: [
        for (final (i, step) in steps.indexed)
          Padding(
            padding: const EdgeInsets.only(bottom: Space.s),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                SizedBox(
                  width: 18,
                  child: Text(
                    '${i + 1}.',
                    style: style.copyWith(color: palette.textMuted),
                  ),
                ),
                Expanded(child: Text(step, style: style)),
              ],
            ),
          ),
        const SizedBox(height: Space.xs),
        // Under the steps rather than beside the buttons, which it
        // would crowd: it's where step 1 begins.
        Align(
          alignment: Alignment.centerLeft,
          child: Button(
            label: "Open Claude's privacy settings",
            onPressed: () => openPrivacyPage(claudePrivacySettings),
          ),
        ),
        const SizedBox(height: Space.l),
      ],
      error: _error,
      submitLabel: 'Choose export…',
      onSubmit: _choose,
      onCancel: widget.close,
    );
  }

  Widget _reading(BuildContext context) {
    final palette = Palette.of(context);
    return FormFrame(
      title: _title,
      fields: [
        Text(
          'Reading your export…',
          style: TypeScale.of(context).body.copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.l),
      ],
      error: null,
      submitLabel: 'Choose export…',
      onSubmit: null,
      onCancel: _stop,
    );
  }

  Widget _choosing(BuildContext context, ExportPreview preview) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final conversations = preview.conversations;
    final count = conversations.length;
    final all = count > 0 && _chosen.length == count;
    final left = leftOutLine(preview);
    return FormFrame(
      title: _title,
      body: _chosen.isEmpty
          ? 'Choose the conversations to bring in. None are selected yet.'
          : 'Choose the conversations to bring in.',
      fields: [
        Row(
          children: [
            Expanded(
              child: Text(
                [
                  '$count conversation${count == 1 ? '' : 's'}',
                  if (_chosen.isNotEmpty) '${_chosen.length} selected',
                ].join(' · '),
                style: type.caption.copyWith(
                  color: palette.textMuted,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
            if (count > 0)
              _TextAction(
                label: all ? 'Select none' : 'Select all',
                onTap: () => setState(() {
                  if (all) {
                    _chosen.clear();
                  } else {
                    _chosen.addAll(List.generate(count, (i) => i));
                  }
                }),
              ),
          ],
        ),
        const SizedBox(height: Space.xs + 2),
        if (count > 0)
          _ConversationList(
            conversations: conversations,
            chosen: _chosen,
            onToggle: (i) => setState(
              () => _chosen.contains(i) ? _chosen.remove(i) : _chosen.add(i),
            ),
          ),
        if (left != null) ...[
          const SizedBox(height: Space.s),
          Text(left, style: type.caption.copyWith(color: palette.textFaint)),
        ],
        const SizedBox(height: Space.l),
        _CheckRow(
          on: _own,
          label:
              'These are my own conversations, and I can share them with '
              'my agents.',
          onTap: () => setState(() => _own = !_own),
        ),
        const SizedBox(height: Space.l),
      ],
      error: null,
      submitLabel: _chosen.isEmpty
          ? 'Import'
          : 'Import ${_chosen.length} conversation'
                '${_chosen.length == 1 ? '' : 's'}',
      onSubmit: _chosen.isEmpty || !_own
          ? null
          : () => setState(() => _destination = true),
      onCancel: widget.close,
    );
  }
}

String _modelOf(Backend backend, Agent agent) {
  if (!agent.modelAvailable) return '';
  final model = backend.workspace.model(agent.model);
  return model == null ? agent.model : modelLabel(model);
}

String _providerOf(Workspace workspace, Agent agent) {
  final id = agent.model.split('/').first;
  for (final provider in workspace.providers) {
    if (provider.id == id) return provider.name;
  }
  return '';
}

/// What giving the conversations to [agent] means, before it happens:
/// which Claktory gets them, and which model (and whose) reads them.
String destinationLine({
  required Agent agent,
  required String claktory,
  required String conversations,
  required String model,
  required String provider,
}) {
  final upload =
      'Claktory uploads the $conversations to $claktory, where '
      '${agent.name} can read them.';
  if (model.isEmpty) {
    return '$upload ${agent.name} has no model available right now, so '
        "nothing reads them until it does.";
  }
  final whose = provider.isEmpty ? '' : ' from $provider';
  return '$upload ${agent.name} runs on $model$whose, which sees what '
      '${agent.name} reads.';
}

/// The export's conversations, newest first, each with its box: a list
/// of its own inside the form, in a hairline, so a long export scrolls
/// there and only the rows in view are built.
class _ConversationList extends StatelessWidget {
  const _ConversationList({
    required this.conversations,
    required this.chosen,
    required this.onToggle,
  });

  final List<ConversationPreview> conversations;
  final Set<int> chosen;
  final ValueChanged<int> onToggle;

  static const _row = 48.0;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scale = MediaQuery.textScalerOf(context).scale(1);
    final row = _row * math.max(1, math.min(scale, 2));
    return Container(
      height: math.min(conversations.length * row + Space.xs * 2, row * 6.5),
      decoration: BoxDecoration(
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
      child: ListView.builder(
        padding: const EdgeInsets.all(Space.xs),
        itemExtent: row,
        itemCount: conversations.length,
        itemBuilder: (context, i) => _ConversationRow(
          conversation: conversations[i],
          on: chosen.contains(i),
          onTap: () => onToggle(i),
        ),
      ),
    );
  }
}

class _ConversationRow extends StatelessWidget {
  const _ConversationRow({
    required this.conversation,
    required this.on,
    required this.onTap,
  });

  final ConversationPreview conversation;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final c = conversation;
    final title = c.title.isNotEmpty
        ? c.title
        : c.snippet.isNotEmpty
        ? c.snippet
        : 'Untitled conversation';
    final about = [
      if (c.createdMs > 0) dayLine(c.createdMs, DateTime.now()),
      '${c.messages} message${c.messages == 1 ? '' : 's'}',
    ].join(' · ');
    return Semantics(
      checked: on,
      label: '$title, $about',
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          decoration: BoxDecoration(
            color: hovered ? palette.hover : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Row(
            children: [
              CheckMark(on: on),
              const SizedBox(width: Space.m),
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: type.body.copyWith(color: palette.text),
                    ),
                    Text(
                      about,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: type.caption.copyWith(color: palette.textFaint),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A checkbox and its words, the whole row one target.
class _CheckRow extends StatelessWidget {
  const _CheckRow({required this.on, required this.label, required this.onTap});

  final bool on;
  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      checked: on,
      label: label,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: CheckMark(on: on),
            ),
            const SizedBox(width: Space.m),
            Expanded(
              child: Text(
                label,
                style: TypeScale.of(context).body.copyWith(
                  color: hovered ? palette.text : palette.textMuted,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A small action in words, in the link's color.
class _TextAction extends StatelessWidget {
  const _TextAction({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      label: label,
      onTap: onTap,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Text(
          label,
          style: TypeScale.of(context).caption
              .copyWith(color: hovered ? palette.text : palette.link),
        ),
      ),
    );
  }
}
