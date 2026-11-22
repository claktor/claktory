// Reporting an agent's reply to Claktory's team (Google Play's policy on
// AI-generated content; Bruno's decision): a sheet that shows exactly what
// goes, as the backend prepared it (the reply's text, the model that wrote
// it, this Claktory's version), takes an optional note, and sends nothing
// until Send.
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'forms.dart' show FormFrame;
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'visible_codes.dart';

/// The longest note the person can add.
const maxReportNote = 1000;

/// Which reply: entry [seq] of agent [agentId]'s conversation, or of chat
/// [chatId]'s.
typedef ReportedReply = ({String agentId, String chatId, int seq});

/// Opens the sheet to report [reply] (a dialog on a Mac, a sheet on a
/// phone).
Future<void> reportReply(BuildContext context, ReportedReply reply) =>
    showModal(
      context,
      (context, close) => ReportReplyForm(reply: reply, close: close),
    );

/// What a refused report says, by why.
String reportFailure(RequestFailure failure) => switch (failure.code) {
  // Claktory's own words say how long to wait, or which side is out of
  // reach; these are for when it gives none.
  ErrorCode.ERROR_CODE_RATE_LIMITED when failure.message.isEmpty =>
    'Too many reports for now. Try again later.',
  ErrorCode.ERROR_CODE_UNREACHABLE when failure.message.isEmpty =>
    "Couldn't send the report: claktory.com can't be reached. Try again in "
        'a moment.',
  ErrorCode.ERROR_CODE_NOT_FOUND =>
    "This reply is no longer in the conversation, so it can't be reported.",
  _ => failure.message,
};

class ReportReplyForm extends StatefulWidget {
  const ReportReplyForm({super.key, required this.reply, required this.close});

  final ReportedReply reply;
  final VoidCallback close;

  @override
  State<ReportReplyForm> createState() => _ReportReplyFormState();
}

class _ReportReplyFormState extends State<ReportReplyForm> {
  final _note = TextEditingController();

  /// What goes, as the backend will send it; null until it says.
  ReplyReport? _preview;
  String? _error;

  /// Whether trying again could help with [_error].
  var _retry = false;
  var _busy = true;
  var _sent = false;

  @override
  void initState() {
    super.initState();
    // Once it's in place: the backend is found from here.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _prepare();
    });
  }

  @override
  void dispose() {
    _note.dispose();
    super.dispose();
  }

  Future<void> _prepare() async {
    try {
      final preview = await AppScope.of(context).reportReply(
        agentId: widget.reply.agentId,
        chatId: widget.reply.chatId,
        seq: widget.reply.seq,
      );
      if (mounted) setState(() => _preview = preview);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = reportFailure(failure));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _send() async {
    if (_busy || _preview == null) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final sent = await AppScope.of(context).reportReply(
        agentId: widget.reply.agentId,
        chatId: widget.reply.chatId,
        seq: widget.reply.seq,
        note: _note.text.trim(),
        send: true,
      );
      // What went, as the send itself answered, not the earlier preview.
      if (mounted) {
        setState(() {
          _preview = sent;
          _sent = true;
        });
      }
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _error = reportFailure(failure);
          _retry = failure.code == ErrorCode.ERROR_CODE_UNREACHABLE;
        });
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final caption = TypeScale.of(
      context,
    ).caption.copyWith(color: Palette.of(context).textMuted);
    if (_sent) {
      return FormFrame(
        title: 'Report this reply',
        body: "Report sent. Thanks, Claktory's team will look at it.",
        fields: [
          if (_preview case final sent?) ...[
            Text('Model: ${proseCodes(sent.model)}', style: caption),
            Text('Version: ${sent.version}', style: caption),
          ],
        ],
        error: null,
        submitLabel: 'Done',
        onSubmit: widget.close,
        onCancel: widget.close,
      );
    }
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final preview = _preview;
    return FormFrame(
      title: 'Report this reply',
      body:
          "Claktory's team gets this reply, the model that wrote it and your "
          "Claktory's version, plus your note if you add one. Nothing else "
          'from your conversation is sent. Reports are kept for 30 days.',
      error: _error,
      // Until the backend has said what goes, nothing can be sent; once a
      // send failed in a way that may pass, Send tries again.
      submitLabel: _retry ? 'Try again' : 'Send',
      onSubmit: _busy || preview == null || (_error != null && !_retry)
          ? null
          : _send,
      onCancel: widget.close,
      dirty: _note.text.trim().isNotEmpty,
      fields: [
        if (preview == null && _error == null)
          Text('Loading…', style: caption)
        else if (preview != null) ...[
          // The reply as it goes: read only, the person's words go in the
          // note.
          Container(
            constraints: const BoxConstraints(maxHeight: 220),
            padding: const EdgeInsets.all(Space.m),
            decoration: BoxDecoration(
              color: palette.sidebar,
              borderRadius: BorderRadius.circular(Radii.control),
              border: Border.all(color: palette.hairline),
            ),
            child: SingleChildScrollView(
              child: Text(
                proseCodes(preview.text),
                style: type.body.copyWith(color: palette.text),
              ),
            ),
          ),
          if (preview.truncated)
            Text(
              'This reply is long, so only its beginning is sent.',
              style: caption,
            ),
          Text('Model: ${proseCodes(preview.model)}', style: caption),
          Text('Version: ${preview.version}', style: caption),
          Field(
            label: 'Note',
            child: TextField(
              controller: _note,
              placeholder: "What's wrong with this reply? (optional)",
              minLines: 2,
              maxLines: 6,
              inputFormatters: [
                LengthLimitingTextInputFormatter(maxReportNote),
              ],
            ),
          ),
        ],
      ],
    );
  }
}
