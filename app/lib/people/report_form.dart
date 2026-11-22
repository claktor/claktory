import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../forms.dart';
import '../identity.dart';
import '../mobile/metrics.dart';
import '../user_apps/session.dart';
import 'contacts.dart';
import 'problems.dart';

// D-S5: reporting someone in People to Claktory's team (Mista's routes,
// crates/claktory-user-apps/README.md, "Reports"): why, the person's own
// words if they like, and their name and picture only if they choose.
// Nobody else is told, and reporting neither declines nor blocks.

/// The most a report's note may say, as the service checks too.
const reportNoteLimit = 1000;

/// What each reason reads as.
String reportReasonLabel(ReportReason reason) => switch (reason) {
  ReportReason.spam => 'Spam',
  ReportReason.harassment => 'Harassment',
  ReportReason.impersonation => 'Pretending to be someone else',
  ReportReason.other => 'Something else',
};

/// Report [person], through whichever of [requestId], [contactId] or
/// [blockId] they're in People by.
class ReportForm extends StatefulWidget {
  const ReportForm({
    super.key,
    required this.api,
    required this.person,
    required this.close,
    this.requestId,
    this.contactId,
    this.blockId,
  });

  final ContactsApi api;
  final PeoplePerson person;
  final VoidCallback close;
  final String? requestId;
  final String? contactId;
  final String? blockId;

  @override
  State<ReportForm> createState() => _ReportFormState();
}

class _ReportFormState extends State<ReportForm> {
  final _note = TextEditingController();

  /// This report's id, kept for its retries, so a retry isn't a second
  /// report.
  final _actionId = newActionId();
  ReportReason? _reason;
  var _includeProfile = false;
  var _busy = false;
  String? _error;

  /// The service's words once it's sent.
  String? _sent;

  @override
  void initState() {
    super.initState();
    _note.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _note.dispose();
    super.dispose();
  }

  Future<void> _send() async {
    final reason = _reason;
    if (reason == null) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final report = await widget.api.report(
        actionId: _actionId,
        requestId: widget.requestId,
        contactId: widget.contactId,
        blockId: widget.blockId,
        reason: reason,
        note: _note.text.trim(),
        includeProfile: _includeProfile,
      );
      if (mounted) {
        setState(
          () => _sent = report.message.isNotEmpty
              ? report.message
              : "Report sent. Claktory's team will look at it. They won't "
                    'know you reported them.',
        );
      }
    } on UserAppError catch (error) {
      if (mounted) setState(() => _error = problemText(error));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final title = 'Report ${widget.person.label}';
    final sent = _sent;
    if (sent != null) {
      return FormFrame(
        title: title,
        fields: [
          Text(sent, style: type.body.copyWith(color: palette.text)),
          const SizedBox(height: Space.l),
        ],
        error: null,
        submitLabel: 'Done',
        onSubmit: widget.close,
        onCancel: widget.close,
      );
    }
    return FormFrame(
      title: title,
      body:
          "Claktory's team will look at it. "
          "${widget.person.name.isEmpty ? 'They' : widget.person.name} won't "
          "know you reported them. Reporting doesn't block them or decline "
          'their request: do that too, if you want.',
      dirty: _reason != null || _note.text.isNotEmpty,
      fields: [
        Field(
          label: 'Why',
          child: Select<ReportReason?>(
            title: 'Why',
            value: _reason,
            options: [
              const SelectOption(null, 'Choose a reason'),
              for (final reason in ReportReason.values)
                SelectOption(reason, reportReasonLabel(reason)),
            ],
            onChanged: (reason) => setState(() => _reason = reason),
          ),
        ),
        Field(
          label: 'Anything to add',
          note: 'Optional. Only Claktory’s team reads it.',
          child: TextField(
            controller: _note,
            placeholder: 'What happened, if you like',
            semanticsLabel: 'Anything to add',
            minLines: 2,
            maxLines: 6,
            inputFormatters: [
              LengthLimitingTextInputFormatter(reportNoteLimit),
            ],
          ),
        ),
        _CheckRow(
          on: _includeProfile,
          label: 'Include the name and picture they show you',
          onTap: () => setState(() => _includeProfile = !_includeProfile),
        ),
        const SizedBox(height: Space.l),
      ],
      error: _error,
      submitLabel: _busy ? 'Sending…' : 'Send report',
      onSubmit: _busy || _reason == null ? null : _send,
      onCancel: widget.close,
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
