import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../forms.dart';
import '../identity.dart';
import '../mobile/metrics.dart';
import '../user_apps/session.dart';
import '../tunnel/api/dm.dart' show DmMessage;
import '../visible_codes.dart' show proseCodes;
import 'contacts.dart';
import 'e2ee.dart';
import 'problems.dart';

// D-S5: reporting someone in People to Claktory's team (Mista's routes,
// crates/claktory-user-apps/README.md, "Reports"): why, the person's own
// words if they like, and their name and picture only if they choose.
// Nobody else is told, and reporting neither declines nor blocks.
// Messages are end to end (E5), so the team sees none unless the person
// ticks some here: their text goes as this device shows it.

/// The most a report's note may say, as the service checks too.
const reportNoteLimit = 1000;

/// The most messages a report may include, and their text in all.
const reportMessageLimit = 50;
const reportMessagesBytes = 64 * 1024;

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
    this.conversationId = '',
  });

  final ContactsApi api;

  /// The conversation with them, whose messages the person may include;
  /// empty for none.
  final String conversationId;
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

  /// The conversation's messages on this device, oldest first, and the
  /// ones ticked, by seq.
  E2ee? _e2ee;
  String _group = '';
  var _messages = const <DmMessage>[];
  var _hasEarlier = false;
  final _picked = <int>{};
  static const _page = 30;

  @override
  void initState() {
    super.initState();
    _note.addListener(() => setState(() {}));
    if (widget.conversationId.isNotEmpty) {
      _e2ee = e2eeOf(context);
      unawaited(_read());
    }
  }

  /// The newest page of the conversation's current group, or [_page] more
  /// before what's here.
  Future<void> _read({bool earlier = false}) async {
    final e2ee = _e2ee;
    if (e2ee == null) return;
    if (_group.isEmpty) {
      final groups = [
        for (final group in e2ee.groups.values)
          if (group.kind == 'conversation' &&
              group.conversationId == widget.conversationId)
            group,
      ]..sort((a, b) => b.generation.compareTo(a.generation));
      if (groups.isEmpty) return;
      _group = groups.first.groupId;
    }
    final page = await e2ee.history(
      _group,
      before: earlier && _messages.isNotEmpty ? _messages.first.seq : null,
      limit: _page + 1,
    );
    if (!mounted) return;
    final more = page.length > _page;
    final shown = [
      for (final message in more ? page.sublist(1) : page)
        if (message.members == null && (message.content.text ?? '').isNotEmpty)
          message,
    ];
    setState(() {
      _messages = earlier ? [...shown, ..._messages] : shown;
      _hasEarlier = more;
    });
  }

  /// A tick that would take the report past what it may hold in all.
  var _tooMuch = false;

  int get _pickedBytes => [
    for (final message in _messages)
      if (_picked.contains(message.seq))
        utf8.encode(message.content.text ?? '').length,
  ].fold(0, (all, each) => all + each);

  void _pick(DmMessage message) => setState(() {
    _tooMuch = false;
    if (_picked.remove(message.seq) || _picked.length >= reportMessageLimit) {
      return;
    }
    final bytes = utf8.encode(message.content.text ?? '').length;
    if (_pickedBytes + bytes > reportMessagesBytes) {
      _tooMuch = true;
      return;
    }
    _picked.add(message.seq);
  });

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
        messages: [
          for (final message in _messages)
            if (_picked.contains(message.seq))
              (
                seq: message.seq,
                text: message.content.text ?? '',
                mine: message.fromAccount == _e2ee?.accountId,
                sentMs: message.arrivedMs,
              ),
        ],
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
      // Ticked messages are work too: Cancel or a swipe asks first
      // (Zucchero's row 200).
      dirty:
          _reason != null ||
          _note.text.isNotEmpty ||
          _includeProfile ||
          _picked.isNotEmpty,
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
        if (_messages.isNotEmpty) _messagesField(type, palette),
      ],
      error: _error,
      submitLabel: _busy ? 'Sending…' : 'Send report',
      onSubmit: _busy || _reason == null ? null : _send,
      onCancel: widget.close,
    );
  }
}

extension on _ReportFormState {
  /// Include messages: the conversation's, newest at the bottom, each with
  /// its box; past the limit, the rest dimmed.
  Widget _messagesField(TypeScale type, Palette palette) {
    final full = _picked.length >= reportMessageLimit;
    final name = widget.person.name.isEmpty ? 'Them' : widget.person.name;
    final me = _e2ee?.accountId ?? '';
    return Field(
      label: 'Include messages',
      note:
          "Claktory's team can't read your messages, so tick any you want "
          'them to see. They get the text as it shows here.',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (_hasEarlier)
            Align(
              alignment: AlignmentDirectional.centerStart,
              child: Button(
                label: 'Show earlier',
                onPressed: () => _read(earlier: true),
              ),
            ),
          for (final message in _messages)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: Opacity(
                opacity: full && !_picked.contains(message.seq) ? 0.4 : 1,
                child: _CheckRow(
                  on: _picked.contains(message.seq),
                  // Their words: what reorders text shows by its code.
                  label: proseCodes(
                    '${message.fromAccount == me ? 'You' : name}: '
                    '${message.content.text}',
                  ),
                  // A few lines each, so a long one doesn't fill screens
                  // (row 200); the whole text goes with the report.
                  maxLines: 4,
                  onTap: () => _pick(message),
                ),
              ),
            ),
          if (_picked.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            Text(
              _picked.length == 1
                  ? '1 message included'
                  : '${_picked.length} messages included',
              style: type.caption.copyWith(color: palette.textMuted),
            ),
          ],
          if (full)
            Text(
              'You can include up to $reportMessageLimit messages.',
              style: type.caption.copyWith(color: palette.textMuted),
            ),
          if (_tooMuch)
            Text(
              "That's as much as one report can include. Untick a long "
              'message to add this one.',
              style: type.caption.copyWith(color: palette.textMuted),
            ),
        ],
      ),
    );
  }
}

/// A checkbox and its words, the whole row one target.
class _CheckRow extends StatelessWidget {
  const _CheckRow({
    required this.on,
    required this.label,
    this.maxLines,
    required this.onTap,
  });

  final bool on;
  final String label;

  /// The most lines it shows; all of it when null.
  final int? maxLines;
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
                maxLines: maxLines,
                overflow: maxLines == null ? null : TextOverflow.ellipsis,
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
