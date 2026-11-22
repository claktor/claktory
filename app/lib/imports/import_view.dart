import 'dart:io' show Platform;
import 'dart:math' as math;

import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import '../attachments.dart' show pickAttachments, pickFiles;
import '../controls.dart';
import '../forms.dart';
import '../identity.dart';
import '../message_time.dart' show dayLine;
import '../mobile/metrics.dart';
import 'claude_export.dart';
import 'import_worker.dart';

// Bringing in conversations from a Claude export (Track B). B0a, this:
// how to get the export, choosing it, reading it on this device and a
// preview to choose from. Nothing is sent anywhere yet; B0b sends what's
// chosen. Hidden: nothing in the app opens it yet.

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
  final paths = Platform.isIOS ? await pickAttachments() : await pickFiles();
  return paths.isEmpty ? null : paths.first;
}

/// Stands in for the system's file picker in tests.
Future<String?> Function()? exportFilePicker;

/// Reads the chosen file; tests set their own.
ExportJob Function(String path) exportReader = readExportFile;

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

  static const _title = 'Import your Claude conversations';

  @override
  void dispose() {
    _job?.cancel();
    super.dispose();
  }

  Future<void> _choose() async {
    final path = await pickExportFile();
    if (path == null || !mounted) return;
    final job = exportReader(path);
    setState(() {
      _job = job;
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

  @override
  Widget build(BuildContext context) {
    if (_job != null) return _reading(context);
    final preview = _preview;
    if (preview != null) return _choosing(context, preview);
    return _instructions(context);
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
      // B0b sends the chosen conversations; until then nothing goes.
      submitLabel: _chosen.isEmpty
          ? 'Import'
          : 'Import ${_chosen.length} conversation'
                '${_chosen.length == 1 ? '' : 's'}',
      onSubmit: null,
      onCancel: widget.close,
    );
  }
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
