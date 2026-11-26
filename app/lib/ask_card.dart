import 'dart:io' show FileSystemException;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'avatar.dart';
import 'backend.dart';
import 'card_answer.dart';
import 'card_files.dart';
import 'characters/character.dart';
import 'command_card.dart';
import 'controls.dart';
import 'decision.dart';
import 'dictation.dart';
import 'explorer.dart' show shortPath;
import 'extensions.dart' show logTime;
import 'identity.dart';
import 'ipad/workbench.dart' show WorkbenchScope;
import 'layout.dart' show layoutOf, open;
import 'markdown.dart';
import 'message_time.dart' show clockTime;
import 'mobile/lines.dart';
import 'mobile/metrics.dart';
import 'outbox.dart' show connectionLost, decisionWaiting;
import 'pipes.dart'
    show
        PipeApprovalAsk,
        PipeApprovalAskView,
        PipeTrustAsk,
        PipeTrustAskView,
        pipePeerClaktory,
        pipesLimiting;
import 'mobile/navigation.dart' show PhoneRoutes;
import 'protocol/factory/v1/factory.pb.dart';
import 'review_answer.dart' show rememberReviewReturn, rememberReviewTalk;
import 'scope.dart';
import 'selection.dart' show SelectableArea;
import 'taint_label.dart';
import 'visible_codes.dart' show proseCodes;

/// Why an answer waits, under its card.
const askWaiting = decisionWaiting;

/// Approved, but the change the card asked for couldn't be made; its
/// reason follows (AskPerson.action_error).
const approvedNotDone = "Approved, but it couldn't be done";

/// How the Claktory refuses an answer to a card its agent changed since
/// it was shown (#428).
const cardChanged = 'This card changed.';

/// An agent's question for the person, one it can't go on without: what it
/// needs (its title) and why (the body, Markdown), and the person's answer
/// (written, when it asks for one) or approval, or Dismiss, which leaves it
/// to the agent's judgment. A sibling of the proposal card, in the agent's
/// color. Its words select and copy like the conversation's, wherever it
/// shows, and a command in the body (a fenced code block) has Copy beside
/// it. The backend sends the entry back answered; the card keeps nothing
/// but what's being typed.
class AskPersonCard extends StatefulWidget {
  const AskPersonCard(
    this.ask, {
    super.key,
    required this.agentId,
    required this.seq,
    this.from = AskAnsweredFrom.ASK_ANSWERED_FROM_NEEDS_YOU,
  });

  final AskPerson ask;
  final String agentId;

  /// Where it shows, so its agent hears where the person answered: Needs
  /// you, unless its conversation says otherwise.
  final AskAnsweredFrom from;

  /// The entry's seq in the agent's conversation.
  final int seq;

  @override
  State<AskPersonCard> createState() => _AskPersonCardState();
}

class _AskPersonCardState extends State<AskPersonCard> with PublishesAnswer {
  /// The card's key: its draft's, its files' and its recordings'.
  String get _key => cardKey(widget.agentId, widget.ask.id);

  /// What's typed on the card: its draft, the same field wherever it shows
  /// (its review bar, focus mode's dock) while the app runs (Q11, #514).
  TextEditingController get _text =>
      dockDraft(_key) ?? CardDrafts.of(_backend!).field(_key);

  /// The draft listened to, for a dock's Submit as words come and go.
  TextEditingController? _listened;
  var _typed = false;

  void _listen() {
    final text = _text;
    if (identical(text, _listened)) return;
    _listened?.removeListener(_typing);
    _listened = text..addListener(_typing);
    _typed = text.text.trim().isNotEmpty;
  }

  void _typing() {
    final typed = _listened?.text.trim().isNotEmpty ?? false;
    if (typed == _typed) return;
    _typed = typed;
    if (inDock && mounted) setState(() {});
  }

  /// The written answer's focus, or a step's what's in the way: never
  /// both on the card at once.
  final _focus = FocusNode(debugLabel: 'Ask answer');
  var _busy = false;
  String? _error;

  /// A procedure's Can't do this… was chosen: its comment field and the
  /// button that sends it show instead of Done.
  var _cant = false;

  /// Can't do this…: its field opens with the keys, so what's typed lands
  /// in it at once, as Add a comment's does (Abbacchio).
  void _openCant() {
    setState(() => _cant = true);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _cant) _focus.requestFocus();
    });
  }

  /// A procedure's finished steps are listed, not only counted.
  var _showSteps = false;

  /// An Execute card with warnings: Execute was tapped once, and asks
  /// again (#494).
  var _confirming = false;

  /// Where the files the person adds to what they write here wait (#511).
  String get _filesKey => _key;

  /// Files wait to go with what's written here.
  bool get _holdsFiles =>
      _backend?.attachments.of(_filesKey).isNotEmpty ?? false;

  /// The files waiting, sent up to the agent (each once): what the answer
  /// names.
  /// None on a pipe's cards, which offer none: a trust card's comment goes
  /// to the pipe's lead (Fugo).
  Future<List<Attachment>> _files(Backend backend) async =>
      widget.ask.hasPipeTrust() || widget.ask.hasPipeApproval()
      ? const []
      : CardUploads.of(backend).send(
          backend,
          widget.agentId,
          _filesKey,
          backend.attachments.of(_filesKey),
        );

  /// What didn't go in as files were added, said in the card's line.
  void _filesNote(String? note) {
    if (!mounted) return;
    setState(() {
      _error = note;
      _waiting = false;
    });
  }

  /// The paperclip beside a field of the card's.
  Widget _attach() =>
      CardAttachButton(card: _filesKey, enabled: !_busy, onNote: _filesNote);

  /// The files waiting, under the field, with where else they may go.
  Widget _chips() =>
      CardFileChips(card: _filesKey, busy: _busy, note: _passOn());

  /// On the card of an agent that talks to a pipe's other side (its
  /// liaison, or one limited to it), whom it may pass the files to; with
  /// the card's own label of what it read (Fugo's ask on #511).
  Widget? _passOn() {
    final backend = _backend;
    if (backend == null) return null;
    final workspace = backend.workspace.workspace;
    final agent = backend.workspace.agent(widget.agentId);
    final name = agent?.name ?? 'The agent';
    final pipes = {
      for (final pipe in pipesLimiting(workspace, widget.agentId))
        pipe.id: pipe,
      for (final pipe in workspace.pipes)
        if (agent != null && pipe.id == agent.pipeId) pipe.id: pipe,
    }.values.toList();
    final read = taintLine(widget.ask.taint, agent: name);
    if (pipes.isEmpty && read == null) return null;
    final palette = Palette.of(context);
    final peers = [for (final pipe in pipes) pipePeerClaktory(pipe)];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (peers.isNotEmpty)
          Text(
            '$name can pass these files on to '
            '${peers.length < 2 ? peers.single : '${peers.sublist(0, peers.length - 1).join(', ')} or ${peers.last}'}.',
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textMuted),
          ),
        TaintLabel(taint: widget.ask.taint, agent: name),
      ],
    );
  }

  /// While [field] has the keys on a Mac, files dropped or pasted on the
  /// window come here.
  Widget _dropping(String name, Widget field) => CardFileDrop(
    card: _filesKey,
    label: 'your answer to $name',
    enabled: !_busy,
    onNote: _filesNote,
    child: field,
  );

  void _filesChanged() {
    if (mounted) setState(() {});
  }

  @override
  void didUpdateWidget(AskPersonCard old) {
    super.didUpdateWidget(old);
    // The next step: a clean card for it. Not when its agent changed the
    // card (#428), which can move the step back: what the person typed
    // stays, under what changed.
    if (old.ask.revision == widget.ask.revision &&
        currentStep(old.ask) != currentStep(widget.ask)) {
      _text.clear();
      _cant = false;
      _error = null;
      _waiting = false;
      _whenBack = null;
    }
  }

  @override
  void dispose() {
    _backend?.connected.removeListener(_connectedChanged);
    _backend?.attachments.removeListener(_filesChanged);
    // The draft stays with the card's key, for wherever it shows next.
    _listened?.removeListener(_typing);
    _focus.dispose();
    super.dispose();
  }

  /// What the person answered last, refused because the card had changed
  /// meanwhile (#473): shown, so it reads as set aside, not lost, until
  /// they answer again.
  String? _setAside;

  /// [revision]: the card's version the person answered (#428), and
  /// [talkSeen], how much of its talk it showed them (#492), kept for a
  /// send again once the connection is back, so a card changed or replied
  /// on meanwhile is never answered unseen.
  Future<void> _answer({
    required bool dismiss,
    int? step,
    bool again = false,
    int? revision,
    int? talkSeen,
    bool confirmed = false,
    String? reason,
  }) async {
    // A no's reason from focus mode's dock goes with what's typed.
    final text = dismiss ? noComment(reason, _text.text) : _text.text.trim();
    if (_busy ||
        (step == null && !dismiss && widget.ask.wantsText && text.isEmpty)) {
      if (_dock) throw const AnswerNotSent();
      return;
    }
    final seen = revision ?? widget.ask.revision;
    final talk = talkSeen ?? widget.ask.thread.length;
    await _send(
      (backend) async => backend.answerAsk(
        widget.agentId,
        widget.seq,
        // Dismissed, what's typed goes too, as the person's comment.
        text: text,
        dismiss: dismiss,
        step: step,
        revision: seen,
        talkSeen: talk,
        from: widget.from,
        confirmed: confirmed,
        // Its files (#511); none with Execute, which has no field.
        attachments: confirmed ? const [] : await _files(backend),
      ),
      () => _answer(
        dismiss: dismiss,
        step: step,
        again: true,
        revision: seen,
        talkSeen: talk,
        confirmed: confirmed,
        reason: reason,
      ),
      again: again,
      said: text.isNotEmpty
          ? text
          : step != null
          ? (dismiss ? "Can't do this" : 'Done')
          : dismiss
          ? 'Dismiss'
          : confirmed
          ? 'Execute'
          : 'Approve',
    );
  }

  /// Sends what's typed as the person's reply (#370): the agent is told
  /// their words, with no yes or no, and the question counts as answered.
  Future<void> _reply({
    bool again = false,
    int? revision,
    int? talkSeen,
  }) async {
    final text = _text.text.trim();
    // Words, or files alone (#511).
    if (_busy || (text.isEmpty && !_holdsFiles)) {
      if (_dock) throw const AnswerNotSent();
      return;
    }
    final seen = revision ?? widget.ask.revision;
    final talk = talkSeen ?? widget.ask.thread.length;
    await _send(
      (backend) async => backend.answerAsk(
        widget.agentId,
        widget.seq,
        text: text,
        attachments: await _files(backend),
        reply: true,
        revision: seen,
        talkSeen: talk,
        from: widget.from,
      ),
      () => _reply(again: true, revision: seen, talkSeen: talk),
      again: again,
      said: text.isEmpty ? 'Reply' : text,
    );
  }

  /// Ends a card with a thread, deciding nothing: what's typed goes as
  /// the person's last words on it.
  Future<void> _resolve({
    bool again = false,
    int? revision,
    int? talkSeen,
  }) async {
    if (_busy) return;
    final text = _text.text.trim();
    final seen = revision ?? widget.ask.revision;
    final talk = talkSeen ?? widget.ask.thread.length;
    await _send(
      (backend) async => backend.answerAsk(
        widget.agentId,
        widget.seq,
        text: text,
        attachments: await _files(backend),
        resolve: true,
        revision: seen,
        talkSeen: talk,
        from: widget.from,
      ),
      () => _resolve(again: true, revision: seen, talkSeen: talk),
      again: again,
      said: text.isNotEmpty ? text : 'Resolve',
    );
  }

  /// What was being sent when the connection dropped, to send again (as
  /// it's typed then) once it's back; null when nothing waits.
  VoidCallback? _whenBack;

  /// [_error] says it waits for the connection, rather than what went
  /// wrong.
  var _waiting = false;

  Backend? _backend;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (identical(backend, _backend)) return;
    _backend?.connected.removeListener(_connectedChanged);
    _backend?.attachments.removeListener(_filesChanged);
    _backend = backend
      ..connected.addListener(_connectedChanged)
      ..attachments.addListener(_filesChanged);
    _listen();
  }

  /// The connection is back: the answer that waited for it goes.
  void _connectedChanged() {
    final whenBack = _whenBack;
    if (whenBack == null || _busy || _backend?.connected.value != true) return;
    _whenBack = null;
    whenBack();
  }

  /// Sends an answer. Cut off by the connection, it stays in the card,
  /// "Not sent yet", and goes again by itself once the connection is back
  /// (#401); sent [again], an answer the Claktory already has (it got the
  /// first before the connection dropped) counts as delivered.
  Future<void> _send(
    Future<void> Function(Backend backend) request,
    VoidCallback retry, {
    required bool again,
    required String said,
  }) async {
    final backend = AppScope.of(context);
    setState(() {
      _busy = true;
      _error = null;
      _waiting = false;
    });
    _whenBack = null;
    var now = false;
    try {
      await request(backend);
      // Sent, its files with it (#511).
      backend.attachments.take(_filesKey);
      CardUploads.of(backend).sent(_filesKey);
      // Sent: should its agent reply and reopen the card, the field starts
      // empty under the thread, which now holds these words.
      if (mounted) {
        _text.clear();
        setState(() => _setAside = null);
      }
    } on FileSystemException catch (error) {
      // A file to go with it couldn't be read (moved or deleted).
      if (mounted) setState(() => _error = cardFilesFailure(error));
      if (_dock) throw const AnswerNotSent();
    } on RequestFailure catch (failure) {
      if (!mounted) return;
      if (again && failure.message.startsWith('This question was already')) {
        return;
      }
      if (connectionLost(failure)) {
        // Back already (it dropped and came back meanwhile): once more now.
        if (backend.connected.value && !again) {
          now = true;
        } else {
          _whenBack = retry;
          setState(() {
            _error = askWaiting;
            _waiting = true;
          });
        }
        // It goes by itself once the connection's back; the card says so.
        if (_dock) throw const AnswerNotSent();
        return;
      }
      // Its agent changed the card meanwhile: the answer is set aside,
      // shown with the change, and what's typed stays to send again.
      if (failure.message.startsWith(cardChanged)) {
        setState(() => _setAside = said);
        if (_dock) throw const AnswerNotSent();
        return;
      }
      setState(() => _error = failure.message);
      if (_dock) rethrow;
    } finally {
      if (mounted) setState(() => _busy = false);
      if (now && mounted) retry();
    }
  }

  /// The color of the line under the card: what went wrong, or that it
  /// waits.
  Color _statusColor(Palette palette) => _error == null
      ? palette.textFaint
      : _waiting
      ? palette.textMuted
      : palette.danger;

  /// A question answered in a dock (focus mode's): its field, yes and no
  /// drawn there, not on the card. Not Execute, a procedure or a pipe's
  /// card, whose controls stay their own.
  bool get _docked {
    final ask = widget.ask;
    return inDock &&
        !ask.hasCommand() &&
        ask.steps.isEmpty &&
        !ask.hasPipeTrust() &&
        !ask.hasPipeApproval();
  }

  /// For focus mode's keys and dock (#514): as the card's own buttons.
  @override
  CardAnswer? answerNow() {
    final ask = widget.ask;
    final pending =
        ask.state == AskPersonState.ASK_PERSON_STATE_PENDING ||
        ask.state == AskPersonState.ASK_PERSON_STATE_UNSPECIFIED;
    if (!pending || ask.withdrawn) return null;
    final key = 'agent:${widget.agentId}/${widget.seq}';
    final busy = _busy;
    final Future<void> Function(String? reason)? dismiss = busy
        ? null
        : (reason) => _fromDock(() => _answer(dismiss: true, reason: reason));
    if (ask.steps.isNotEmpty) {
      final at = currentStep(ask);
      if (at == null) return null;
      return CardAnswer(
        key: key,
        docked: false,
        yes: 'Done',
        no: "Can't do this",
        busy: busy,
        error: _waiting ? null : _error,
        status: _waiting ? _error : null,
        onYes: busy
            ? null
            : () => _fromDock(() => _answer(dismiss: false, step: at)),
        onNo: busy
            ? null
            : (reason) => _fromDock(
                () => _answer(dismiss: true, step: at, reason: reason),
              ),
      );
    }
    if (ask.hasCommand()) {
      return CardAnswer(
        key: key,
        docked: false,
        yes: _confirming ? 'Run anyway' : 'Execute',
        no: 'Dismiss',
        pressTwice: true,
        busy: busy,
        error: _waiting ? null : _error,
        status: _waiting ? _error : null,
        onYes: busy ? null : () => _fromDock(_executeNow),
        onNo: dismiss,
      );
    }
    final typed = _typed;
    if (ask.hasPipeTrust() || ask.hasPipeApproval()) {
      // Trust and Approve stay the card's own buttons, after its digits
      // or its warning (FS14, Fugo's rule C): never a key.
      return CardAnswer(
        key: key,
        docked: false,
        yes: ask.hasPipeTrust() ? 'Trust' : 'Approve',
        no: 'Dismiss',
        typed: typed,
        busy: busy,
        error: _waiting ? null : _error,
        status: _waiting ? _error : null,
        onSubmit: busy ? null : () => _fromDock(_reply),
        onNo: dismiss,
      );
    }
    final wantsText = ask.wantsText;
    final name = _backend?.workspace.agent(widget.agentId)?.name ?? 'the agent';
    return CardAnswer(
      key: key,
      docked: true,
      field: _text,
      placeholder: wantsText
          ? 'Your answer'
          : 'A comment for $name, if you like',
      wantsText: wantsText,
      yes: wantsText ? 'Answer' : 'Approve',
      no: 'Dismiss',
      // Words on an approval are a reply (#370); on a written answer,
      // they're the answer.
      typed: typed && !wantsText,
      busy: busy,
      error: _waiting ? null : _error,
      status: _waiting ? _error : null,
      filesKey: _filesKey,
      onYes: busy || (wantsText && !typed)
          ? null
          : () => _fromDock(() => _answer(dismiss: false)),
      onSubmit: busy ? null : () => _fromDock(_reply),
      onNo: dismiss,
    );
  }

  /// An answer from focus mode's keys on its way (#514): refused, it
  /// throws; held for the connection, or set aside as the card changed,
  /// it throws [AnswerNotSent], the card saying why.
  var _dock = false;

  Future<void> _fromDock(Future<void> Function() run) async {
    _dock = true;
    try {
      await run();
    } finally {
      _dock = false;
    }
  }

  /// Execute, as its button does: with warnings, the first asks again.
  Future<void> _executeNow() async {
    if (widget.ask.command.warnings.isNotEmpty && !_confirming) {
      setState(() => _confirming = true);
      if (_dock) throw const AnswerNotSent();
      return;
    }
    await _answer(dismiss: false, confirmed: true);
  }

  @override
  Widget build(BuildContext context) {
    _listen();
    publishAnswer();
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final ask = widget.ask;
    final agent = AppScope.of(context).workspace.agent(widget.agentId);
    final color = agentColor(agent?.color ?? 'slate');
    final name = agent?.name ?? 'The agent';
    final pending =
        ask.state == AskPersonState.ASK_PERSON_STATE_PENDING ||
        ask.state == AskPersonState.ASK_PERSON_STATE_UNSPECIFIED;
    final dismissed = ask.state == AskPersonState.ASK_PERSON_STATE_DISMISSED;
    final decided = ask.decidedMs.toInt();
    final card = Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.06),
        border: Border.all(
          color: color.withValues(alpha: pending ? 0.55 : 0.25),
        ),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              if (agent != null) ...[
                AgentCharacter(agent: agent, size: 20),
                const SizedBox(width: Space.xs + 2),
              ],
              Flexible(
                child: Text(
                  // Answered, dismissed or taken back (#407), it needs
                  // nothing.
                  ask.withdrawn || !pending
                      ? '$name asked'
                      : ask.hasCommand()
                      ? '$name asks to run a command'
                      : ask.steps.isEmpty
                      ? '$name needs your answer'
                      : '$name needs you to do something',
                  style: type.caption.copyWith(
                    color: ask.withdrawn ? palette.textMuted : color,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          // Where the question may come from, when its agent read a pipe.
          TaintLabel(taint: ask.taint, agent: name),
          const SizedBox(height: Space.s),
          // A command to run (#494): Claktory's heading and the command,
          // the agent's title and body under them as its reason.
          if (ask.hasCommand())
            CommandFacts(
              ask: ask,
              name: name,
              muted: dismissed || ask.withdrawn,
            )
          else
            // What the agent wrote, to decide on: what reorders text shows
            // by its code (proseCodes, Fugo).
            Text(
              proseCodes(ask.title),
              style: type.emphasis.copyWith(
                color: dismissed ? palette.textMuted : palette.text,
              ),
            ),
          // An admin agent's action card (#423): what Approve does, in
          // Claktory's own words, then the agent's reason apart and named
          // as its, so its words never read as part of the action (Fugo).
          if (ask.actionDescription.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            Text(
              proseCodes(ask.actionDescription),
              style: type.body.copyWith(color: palette.text),
            ),
            // Waiting to trust a pipe's other side again (#471 G7), the
            // reason goes under Claktory's warning instead (Fugo).
            if (ask.actionReason.isNotEmpty &&
                !(ask.hasPipeTrust() && pending && !ask.withdrawn)) ...[
              const SizedBox(height: Space.s),
              _reason(palette, type, name),
            ],
          ] else if (ask.body.isNotEmpty && !ask.hasCommand()) ...[
            const SizedBox(height: Space.xs),
            for (final (i, part) in askBodyParts(ask.body).indexed) ...[
              if (i > 0) const SizedBox(height: Space.s),
              part.command
                  ? CommandBlock(part.text)
                  : Markdown(proseCodes(part.text), color: palette.textMuted),
            ],
          ],
          // A document it asks the person to review, with Review.
          if (ask.reviewPath.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            _ReviewFile(
              path: ask.reviewPath,
              // Nothing to review once its agent took it back (#407).
              onReview: ask.withdrawn
                  ? null
                  : () => openReview(
                      context,
                      widget.agentId,
                      ask.reviewPath,
                      // The talk as the card shows it now (#492).
                      card: (seq: widget.seq, talkSeen: ask.thread.length),
                    ),
            ),
          ],
          // Changed by its agent while it waited (#428): when, and why in
          // its words, above the answer (what's typed stays).
          // Or an answer it set aside (#473), even before the new version
          // comes.
          if (ask.updatedMs > 0 || _setAside != null) ...[
            const SizedBox(height: Space.s),
            UpdatedLine(
              updatedMs: ask.updatedMs.toInt(),
              // Said once: the card's own reason already says it (an
              // action card updated with the same words, Zucchero's row
              // 535).
              reason:
                  ask.actionDescription.isNotEmpty &&
                      ask.updateReason.trim() == ask.actionReason.trim()
                  ? ''
                  : ask.updateReason,
              name: name,
              // Claktory's own words when it marked the card itself: its
              // file changed while it waited (Ghiaccio 46899).
              note: ask.updateNote,
              setAside: _setAside,
            ),
          ],
          // The talk on it so far, oldest first: the person's words and
          // its agent's replies, above the card's buttons.
          if (ask.thread.isNotEmpty) ...[
            const SizedBox(height: Space.m),
            AskThread(thread: ask.thread, agent: agent, name: name),
          ],
          const SizedBox(height: Space.m),
          // Taken back by its agent (#407): nothing to answer, and never
          // the person's dismissal.
          if (ask.withdrawn)
            WithdrawnLine(
              agent: agent,
              name: name,
              reason: ask.withdrawReason,
              decidedMs: decided,
            )
          else if (ask.steps.isNotEmpty)
            ..._procedure(palette, type, color, name, pending, decided)
          else if (pending && ask.hasCommand())
            ..._execute(palette, type, name)
          else if (pending)
            ..._pending(palette, type, name)
          else ...[
            ..._decided(palette, type, color, name, dismissed, decided),
            // What it did, on the card only (never in History).
            if (ask.hasCommand() && ask.command.hasRun()) ...[
              const SizedBox(height: Space.s),
              CommandRunView(command: ask.command),
            ],
          ],
        ],
      ),
    );
    // In a conversation its words are the conversation's to select; in
    // Needs you, where nothing around it is, its own (a selection needs an
    // overlay for its menu, which the app always has).
    if (SelectionContainer.maybeOf(context) != null ||
        Overlay.maybeOf(context) == null) {
      return card;
    }
    return SelectableArea(child: card);
  }

  /// What a recording for this card is kept as until its words come
  /// (#401): it comes back here after a relaunch.
  DictationFor _keep(String name) => DictationFor(
    _key,
    widget.ask.title.isEmpty
        ? 'Your answer to $name'
        : 'Your answer to $name: ${widget.ask.title}',
    // The bar's name goes back to the conversation the card is in.
    select: (backend) => backend.selectAgent(widget.agentId),
  );

  /// An action card's reason, in its agent's words, named as its so they
  /// never read as part of the action (Fugo).
  Widget _reason(Palette palette, TypeScale type, String name) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Text(
        "$name's reason",
        style: type.caption.copyWith(
          color: palette.textFaint,
          fontWeight: FontWeight.w500,
        ),
      ),
      Markdown(proseCodes(widget.ask.actionReason), color: palette.textMuted),
    ],
  );

  /// An Execute card waiting (#494): Dismiss or Execute, which runs
  /// exactly the command shown, once; with Claktory's warnings, a second
  /// tap to confirm. No reply: the backend refuses one on it.
  List<Widget> _execute(Palette palette, TypeScale type, String name) {
    void execute() => _executeNow();

    return [
      ActionLine(
        lead: Text(
          _error ??
              (_confirming
                  ? 'Run it anyway? Read the warnings above first.'
                  : '$name gets what it prints once it has run.'),
          style: type.caption.copyWith(
            color: _confirming && _error == null
                ? palette.danger
                : _statusColor(palette),
          ),
        ),
        action: ControlsLine(
          children: [
            if (_confirming)
              Button(
                label: 'Cancel',
                onPressed: _busy
                    ? null
                    : () => setState(() => _confirming = false),
              )
            else
              Button(
                label: 'Dismiss',
                onPressed: _busy ? null : () => _answer(dismiss: true),
              ),
            Button(
              label: _confirming ? 'Run anyway' : 'Execute',
              primary: true,
              onPressed: _busy ? null : execute,
            ),
          ],
        ),
      ),
    ];
  }

  List<Widget> _pending(Palette palette, TypeScale type, String name) {
    final wantsText = widget.ask.wantsText;
    // An agent asking the person to approve a pipe (#471 G1): the pipe's
    // approval card, from what it proposes, approved through this card;
    // its Dismiss and reply are this card's.
    // An agent asking the person to accept a change at a pipe's other end
    // (#471 G7): Claktory's warning from the pipe as it is, the agent's
    // reason under it, then Trust (or OK) through this card.
    if (widget.ask.hasPipeTrust()) {
      return [
        PipeTrustAskView(
          pipeId: widget.ask.pipeTrust.pipeId,
          ask: PipeTrustAsk(
            cardId: widget.ask.id,
            agent: name,
            change: widget.ask.pipeTrust.identityChange,
            comment: _text,
            onDismiss: () => _answer(dismiss: true),
            onReply: _reply,
            reason: widget.ask.actionReason.isEmpty
                ? null
                : _reason(palette, type, name),
            busy: _busy,
            error: _error,
          ),
        ),
        ..._afterTalk(palette, type, name),
        if (widget.ask.thread.isNotEmpty) ..._talk(type, reply: false),
      ];
    }
    if (widget.ask.hasPipeApproval()) {
      return [
        PipeApprovalAskView(
          pipeId: widget.ask.pipeApproval.pipeId,
          ask: PipeApprovalAsk(
            cardId: widget.ask.id,
            agent: name,
            proposal: widget.ask.pipeApproval,
            updated: widget.ask.updatedMs > 0,
            comment: _text,
            onDismiss: () => _answer(dismiss: true),
            onReply: _reply,
            busy: _busy,
            error: _error,
          ),
        ),
        ..._afterTalk(palette, type, name),
        if (widget.ask.thread.isNotEmpty) ..._talk(type, reply: false),
      ];
    }
    // In focus mode's dock (#514): the field, yes and no are the dock's;
    // the card keeps what Approve means after a talk, how it's going, and
    // the files waiting.
    if (_docked) {
      return [
        ..._afterTalk(palette, type, name, below: false),
        if (_error case final error?)
          Text(
            error,
            style: type.caption.copyWith(color: _statusColor(palette)),
          ),
        _chips(),
        if (widget.ask.thread.isNotEmpty) ..._talk(type, reply: wantsText),
      ];
    }
    return [
      // A written answer has its box; an approval, a comment under its
      // buttons for either.
      if (wantsText) ...[
        // Or said: the mic puts what's said in, to check before sending.
        // What's typed stays when the card scrolls away and back.
        _dropping(
          name,
          KeepWhileTyped(
            controller: _text,
            child: Dictation(
              controller: _text,
              focusNode: _focus,
              enabled: !_busy,
              keep: _keep(name),
              // Files with it (#511).
              trailing: _attach(),
              child: CallbackShortcuts(
                bindings: {
                  const SingleActivator(LogicalKeyboardKey.enter): () =>
                      _answer(dismiss: false),
                },
                child: TextField(
                  controller: _text,
                  focusNode: _focus,
                  placeholder: 'Your answer',
                  minLines: 1,
                  maxLines: 8,
                ),
              ),
            ),
          ),
        ),
        _chips(),
        const SizedBox(height: Space.s),
      ],
      // What Approve means after a talk on the card (#492), above it.
      ..._afterTalk(palette, type, name, below: false),
      // On a phone at large text the buttons go under the words. Once a
      // comment is typed on an approval, one Submit in place of both: what's
      // typed is a reply, never a yes or a no (#370).
      CommentTyped(
        controller: _text,
        builder: (context, typed) {
          final submit = typed && !wantsText;
          return ActionLine(
            lead: Text(
              _error ??
                  (submit
                      ? '$name gets your comment and carries on from it.'
                      : wantsText
                      ? '$name gets your answer and carries on.'
                      : widget.ask.reviewPath.isNotEmpty
                      ? 'Your comments on it go to $name.'
                      : '$name carries on once you approve.'),
              style: type.caption.copyWith(color: _statusColor(palette)),
            ),
            action: submit
                ? SubmitButton(onPressed: _busy ? null : _reply)
                : ControlsLine(
                    children: [
                      Button(
                        label: 'Dismiss',
                        onPressed: _busy ? null : () => _answer(dismiss: true),
                      ),
                      Button(
                        label: wantsText ? 'Answer' : 'Approve',
                        primary: true,
                        onPressed: _busy || (wantsText && !typed)
                            ? null
                            : () => _answer(dismiss: false),
                      ),
                    ],
                  ),
          );
        },
      ),
      if (!wantsText)
        // Return in it is Submit, once something is typed. Files with it
        // (#511) go with Approve, Dismiss or Submit.
        _dropping(
          name,
          DecisionComment(
            controller: _text,
            agent: name,
            enabled: !_busy,
            onSubmit: _reply,
            keep: _keep(name),
            attach: _attach(),
            files: _chips(),
            holdsFiles: _holdsFiles,
          ),
        ),
      if (widget.ask.thread.isNotEmpty) ..._talk(type, reply: wantsText),
    ];
  }

  /// What answering means after a talk on the card (#492): agreeing with
  /// where it ended, when the agent's reply came after the card's words
  /// last changed (the backend says, AskPerson.approve_follows_talk); on an
  /// action card, which can't be talked on, only what it shows, should a
  /// talk ever be there. [below]: under the card's own view (the pipe
  /// cards'), rather than above its buttons.
  List<Widget> _afterTalk(
    Palette palette,
    TypeScale type,
    String name, {
    bool below = true,
  }) {
    final ask = widget.ask;
    final String line;
    if (ask.approveFollowsTalk) {
      line = ask.wantsText
          ? "Your answer follows $name's last reply."
          : "Approving agrees with $name's last reply.";
    } else if (ask.thread.isNotEmpty &&
        (ask.actionDescription.isNotEmpty ||
            ask.hasPipeApproval() ||
            ask.hasPipeTrust())) {
      line =
          'Approving does only what the card says, not what was said in '
          'the talk.';
    } else {
      return const [];
    }
    final text = Text(
      line,
      style: type.caption.copyWith(
        color: palette.textMuted,
        fontWeight: FontWeight.w500,
      ),
    );
    return below
        ? [const SizedBox(height: Space.s), text]
        : [text, const SizedBox(height: Space.xs)];
  }

  /// Under a card with a thread, beneath its own buttons: Reply, for a
  /// card whose typed words are otherwise its answer (an approval's are a
  /// reply already, Submit), and Resolve, which ends it deciding nothing.
  List<Widget> _talk(TypeScale type, {required bool reply}) => [
    const SizedBox(height: Space.s),
    CommentTyped(
      controller: _text,
      builder: (context, typed) => ActionLine(
        lead: Text(
          reply
              ? 'Reply to keep talking, or resolve it with no decision.'
              : 'Resolve ends it with no decision.',
          style: type.caption.copyWith(color: Palette.of(context).textFaint),
        ),
        action: ControlsLine(
          children: [
            if (reply)
              Button(
                label: 'Reply',
                // Words, or files alone (#511).
                onPressed: _busy || !(typed || _holdsFiles) ? null : _reply,
              ),
            Button(label: 'Resolve', onPressed: _busy ? null : _resolve),
          ],
        ),
      ),
    ),
  ];

  /// A procedure (ask_person's steps): one step at a time, "Step 2 of 6",
  /// with Done and Can't do this…, the steps finished counted above it
  /// and listed on request; decided, how it ended, with every step.
  List<Widget> _procedure(
    Palette palette,
    TypeScale type,
    Color color,
    String name,
    bool pending,
    int decided,
  ) {
    final ask = widget.ask;
    final steps = ask.steps;
    final at = currentStep(ask);
    final finished = [
      for (final (i, step) in steps.indexed)
        if (!_open(step)) i,
    ];
    if (pending && at != null) {
      final step = steps[at];
      return [
        if (finished.isNotEmpty) ...[
          _StepsToggle(
            label: finished.length == 1
                ? '1 step done'
                : '${finished.length} steps done',
            open: _showSteps,
            onTap: () => setState(() => _showSteps = !_showSteps),
          ),
          if (_showSteps)
            for (final i in finished) _StepLine(index: i, step: steps[i]),
          const SizedBox(height: Space.s),
        ],
        Text(
          'Step ${at + 1} of ${steps.length}',
          style: type.caption.copyWith(
            color: color,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: Space.xs),
        for (final (i, part) in askBodyParts(step.text).indexed) ...[
          if (i > 0) const SizedBox(height: Space.s),
          part.command
              ? CommandBlock(part.text)
              : Markdown(proseCodes(part.text), color: palette.text),
        ],
        const SizedBox(height: Space.m),
        if (_cant) ...[
          // What's in the way, for the agent to decide what's next; typed
          // or said. What's typed stays when the card scrolls away and back.
          _dropping(
            name,
            KeepWhileTyped(
              controller: _text,
              child: Dictation(
                controller: _text,
                focusNode: _focus,
                enabled: !_busy,
                keep: _keep(name),
                // Files with it (#511): a screenshot of what's in the way.
                trailing: _attach(),
                child: TextField(
                  controller: _text,
                  focusNode: _focus,
                  placeholder: "What's in the way? (optional)",
                  minLines: 1,
                  maxLines: 6,
                ),
              ),
            ),
          ),
          _chips(),
          const SizedBox(height: Space.s),
          ActionLine(
            lead: Text(
              _error ?? '$name hears why and decides what to do next.',
              style: type.caption.copyWith(color: _statusColor(palette)),
            ),
            action: ControlsLine(
              children: [
                Button(
                  label: 'Back',
                  onPressed: _busy ? null : () => setState(() => _cant = false),
                ),
                Button(
                  label: "Can't do this",
                  primary: true,
                  onPressed: _busy
                      ? null
                      : () => _answer(dismiss: true, step: at),
                ),
              ],
            ),
          ),
        ] else ...[
          ActionLine(
            lead: Text(
              _error ??
                  (at + 1 < steps.length
                      ? 'The next step shows once this one is done.'
                      : '$name carries on once this is done.'),
              style: type.caption.copyWith(color: _statusColor(palette)),
            ),
            action: ControlsLine(
              children: [
                Button(
                  label: "Can't do this…",
                  onPressed: _busy ? null : _openCant,
                ),
                Button(
                  label: 'Done',
                  primary: true,
                  onPressed: _busy
                      ? null
                      : () => _answer(dismiss: false, step: at),
                ),
              ],
            ),
          ),
          _dropping(
            name,
            DecisionComment(
              controller: _text,
              agent: name,
              enabled: !_busy,
              onSubmitted: () => _answer(dismiss: false, step: at),
              keep: _keep(name),
              attach: _attach(),
              files: _chips(),
              holdsFiles: _holdsFiles,
            ),
          ),
          if (ask.thread.isNotEmpty) ..._talk(type, reply: true),
        ],
      ];
    }
    // Decided: every step done, stopped at one, or left to the agent.
    final stopped = steps.indexed
        .where((each) => each.$2.state == AskStepState.ASK_STEP_STATE_CANT)
        .firstOrNull;
    // Talked through: a reply (the steps left as they were until its agent
    // answers) or Resolve, never every step done.
    final talked = ask.replied || ask.resolved;
    final allDone =
        ask.state == AskPersonState.ASK_PERSON_STATE_ANSWERED &&
        stopped == null &&
        !talked;
    // Closed by its agent as settled in the talk (#496).
    final agentClosed = agentResolved(ask);
    final agent = AppScope.of(context).workspace.agent(widget.agentId);
    final outcome = agentClosed
        ? '$name closed it'
        : ask.resolved
        ? 'Resolved'
        : ask.replied
        ? 'Replied'
        : allDone
        ? steps.length == 2
              ? 'Both steps done'
              : 'All ${steps.length} steps done'
        : stopped != null
        ? "Couldn't do step ${stopped.$1 + 1} of ${steps.length}"
        : 'Left to $name';
    return [
      Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (agentClosed && agent != null)
            AgentCharacter(agent: agent, size: 16)
          else
            const PersonAvatar(size: 16),
          const SizedBox(width: Space.xs + 2),
          Flexible(
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: outcome,
                    style: TextStyle(
                      color: allDone || talked ? color : palette.textMuted,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  if (decided > 0)
                    TextSpan(
                      text: ' · ${cardTime(context, decided)}',
                      style: TextStyle(color: palette.textFaint),
                    ),
                ],
              ),
              style: type.caption,
            ),
          ),
        ],
      ),
      // The person's words: their reply, or their last word on resolving.
      if (talked && ask.answer.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(ask.answer, style: type.body.copyWith(color: palette.text)),
      ],
      // And their files with it (#511).
      if (talked) CardSentFiles(ask.answerAttachments),
      // What was in the way.
      if (stopped != null && stopped.$2.comment.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(
          stopped.$2.comment,
          style: type.body.copyWith(color: palette.textMuted),
        ),
      ],
      if (stopped != null) CardSentFiles(stopped.$2.commentAttachments),
      const SizedBox(height: Space.xs),
      _StepsToggle(
        label: 'Steps',
        open: _showSteps,
        onTap: () => setState(() => _showSteps = !_showSteps),
      ),
      if (_showSteps)
        for (final (i, step) in steps.indexed)
          _StepLine(index: i, step: step, reached: !_open(step)),
    ];
  }

  List<Widget> _decided(
    Palette palette,
    TypeScale type,
    Color color,
    String name,
    bool dismissed,
    int decided,
  ) {
    final ask = widget.ask;
    // Approved, but the change it asked for couldn't be made (#423's
    // action cards, a clear): Claktory says why (AskPerson.action_error).
    final failed =
        !ask.resolved &&
        !ask.replied &&
        !dismissed &&
        ask.actionError.isNotEmpty;
    // Closed by its agent once the talk settled it (ask_reply's resolve,
    // #496): its words end the talk, said as it closed the card.
    final agentClosed = agentResolved(ask);
    final agent = AppScope.of(context).workspace.agent(widget.agentId);
    return [
      Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (agentClosed && agent != null)
            AgentCharacter(agent: agent, size: 16)
          else
            const PersonAvatar(size: 16),
          const SizedBox(width: Space.xs + 2),
          Flexible(
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    // A reply (#370): the person's words, neither a yes
                    // nor a no.
                    // Ended with no decision, after talking it through.
                    text: agentClosed
                        ? '$name closed it'
                        : ask.resolved
                        ? 'Resolved'
                        : ask.replied
                        ? 'Replied'
                        : dismissed
                        ? 'Left to $name'
                        : failed
                        ? approvedNotDone
                        : ask.wantsText
                        ? 'Answered'
                        : ask.hasCommand()
                        ? 'Executed'
                        : 'Approved',
                    style: TextStyle(
                      color: failed
                          ? palette.danger
                          : dismissed && !ask.replied
                          ? palette.textMuted
                          : color,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  if (decided > 0)
                    TextSpan(
                      text: ' · ${cardTime(context, decided)}',
                      style: TextStyle(color: palette.textFaint),
                    ),
                ],
              ),
              style: type.caption,
            ),
          ),
        ],
      ),
      if (failed) ...[
        const SizedBox(height: Space.xs),
        Text(
          ask.actionError,
          style: type.body.copyWith(color: palette.textMuted),
        ),
      ],
      // The answer, or the comment it was dismissed with.
      if (ask.answer.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        Text(
          ask.answer,
          style: type.body.copyWith(
            color: dismissed && !ask.replied ? palette.textMuted : palette.text,
          ),
        ),
      ],
      // And the files that went with it (#511; a review's drawings).
      CardSentFiles(ask.answerAttachments),
    ];
  }
}

/// When [entry] began waiting, for Needs you's order and its age: a card
/// its agent replied on (and so reopened) by its thread's last message,
/// anything else by when it came; 0 for none.
int waitingSince(Entry? entry) {
  if (entry == null) return 0;
  var since = entry.timestampMs.toInt();
  // Changed by its agent since (#428, #473): from then, so it rises.
  final updated = updatedSince(entry);
  if (updated > since) since = updated;
  if (entry.hasAskPerson() && entry.askPerson.thread.isNotEmpty) {
    final last = entry.askPerson.thread.last.atMs.toInt();
    if (last > since) since = last;
  }
  return since;
}

/// When [entry]'s card was last changed by its agent (ask_update,
/// proposal_update), in ms since 1970; 0 if it never was.
int updatedSince(Entry? entry) {
  if (entry == null) return 0;
  if (entry.hasAskPerson()) return entry.askPerson.updatedMs.toInt();
  if (entry.hasProposal()) return entry.proposal.updatedMs.toInt();
  return 0;
}

/// The talk on a card (AskPerson.thread), oldest first and compact: each
/// message after its writer's 16px avatar (the person's, or the agent's
/// character), "You · 14:02" or "June · 14:05" in caption, and its words
/// as Markdown in body `text`, what reorders text shown by its code. A
/// hairline sets it off from the card's own words above.
class AskThread extends StatelessWidget {
  const AskThread({
    super.key,
    required this.thread,
    required this.agent,
    required this.name,
  });

  final List<AskThreadMessage> thread;

  /// The card's agent, when the app knows it, and its name.
  final Agent? agent;
  final String name;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final agent = this.agent;
    return Container(
      padding: const EdgeInsets.only(top: Space.s),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: palette.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final (i, message) in thread.indexed)
            Padding(
              padding: EdgeInsets.only(top: i == 0 ? 0 : Space.s),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Padding(
                    padding: const EdgeInsets.only(top: 1),
                    child: message.fromAgent && agent != null
                        ? AgentCharacter(agent: agent, size: 16)
                        : message.fromAgent
                        ? const SizedBox.square(dimension: 16)
                        : const PersonAvatar(size: 16),
                  ),
                  const SizedBox(width: Space.xs + 2),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Text.rich(
                          TextSpan(
                            children: [
                              TextSpan(
                                text: message.fromAgent ? name : 'You',
                                style: TextStyle(
                                  color: palette.textMuted,
                                  fontWeight: FontWeight.w500,
                                ),
                              ),
                              if (message.atMs > 0)
                                TextSpan(
                                  text:
                                      ' · ${cardTime(context, message.atMs.toInt())}',
                                  style: TextStyle(color: palette.textFaint),
                                ),
                            ],
                          ),
                          style: type.caption,
                        ),
                        if (message.text.isNotEmpty)
                          Markdown(
                            proseCodes(message.text),
                            color: palette.text,
                          ),
                        // The person's files on that turn (#511).
                        CardSentFiles(message.attachments),
                      ],
                    ),
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// A card its agent took back once it no longer applied (#407): "Withdrawn
/// by Mike" with when, by the agent's character, and its reason under it
/// in the agent's own words. In place of the answer, with nothing to press.
class WithdrawnLine extends StatelessWidget {
  const WithdrawnLine({
    super.key,
    required this.agent,
    required this.name,
    required this.reason,
    required this.decidedMs,
  });

  /// The agent that withdrew it, when the app knows it, and its name.
  final Agent? agent;
  final String name;
  final String reason;
  final int decidedMs;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final agent = this.agent;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (agent != null) ...[
              AgentCharacter(agent: agent, size: 16),
              const SizedBox(width: Space.xs + 2),
            ],
            Flexible(
              child: Text.rich(
                TextSpan(
                  children: [
                    TextSpan(
                      text: 'Withdrawn by $name',
                      style: TextStyle(
                        color: palette.textMuted,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                    if (decidedMs > 0)
                      TextSpan(
                        text: ' · ${cardTime(context, decidedMs)}',
                        style: TextStyle(color: palette.textFaint),
                      ),
                  ],
                ),
                style: type.caption,
              ),
            ),
          ],
        ),
        if (reason.isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          // As the card's body reads: `code` as code.
          Markdown(proseCodes(reason), color: palette.textMuted),
        ],
      ],
    );
  }
}

/// When a card was decided, withdrawn or changed: "14:32" today ("2:32
/// PM" on a 12-hour device, as its message times read), "3 Nov 14:32"
/// another day.
String cardTime(BuildContext context, int ms) {
  final clock = clockTime(
    ms,
    h24: MediaQuery.maybeAlwaysUse24HourFormatOf(context) ?? true,
  );
  final logged = logTime(DateTime.fromMillisecondsSinceEpoch(ms));
  // logTime's date, when it has one, before this device's clock.
  final space = logged.lastIndexOf(' ');
  return space < 0 ? clock : '${logged.substring(0, space)} $clock';
}

/// A card its agent changed while it waited (#428, #473): "Updated" in
/// signal and "· 14:32" faint, in caption; an answer the change set aside,
/// as the person gave it; and its reason in the agent's own words, muted,
/// as a card's body reads.
class UpdatedLine extends StatelessWidget {
  const UpdatedLine({
    super.key,
    required this.updatedMs,
    required this.reason,
    this.name = '',
    this.note = '',
    this.setAside,
  });

  final int updatedMs;
  final String reason;

  /// Claktory's words for a change it made itself (a review card's file
  /// changed while it waited): shown as Claktory's, never as the agent's
  /// reason.
  final String note;

  /// The agent's name, for its reason's label; none without it.
  final String name;

  /// What the person answered, refused because the card changed meanwhile
  /// (#473): shown so they see it wasn't lost.
  final String? setAside;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final said = setAside;
    final who = name.isEmpty ? 'The agent' : name;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        // Quiet but seen (Matt): the mark in signal, as what waits on the
        // person is, the time faint.
        if (updatedMs > 0)
          Text.rich(
            TextSpan(
              children: [
                TextSpan(
                  text: updatedMark,
                  style: TextStyle(
                    color: palette.signal,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                TextSpan(
                  text: ' · ${cardTime(context, updatedMs)}',
                  style: TextStyle(color: palette.textFaint),
                ),
              ],
            ),
            style: type.caption,
          ),
        if (said != null) ...[
          if (updatedMs > 0) const SizedBox(height: Space.xs),
          Text(
            note.isNotEmpty ? fileChangedAfterAnswer : changedAfterAnswer(who),
            style: type.body.copyWith(color: palette.text),
          ),
          Text(
            youSaid(said),
            style: type.body.copyWith(color: palette.textMuted),
          ),
        ],
        if (note.isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          Text(note, style: type.body.copyWith(color: palette.textMuted)),
        ],
        if (reason.isNotEmpty) ...[
          const SizedBox(height: Space.xs),
          if (name.isNotEmpty)
            Text(
              "$name's reason",
              style: type.caption.copyWith(
                color: palette.textFaint,
                fontWeight: FontWeight.w500,
              ),
            ),
          Markdown(proseCodes(reason), color: palette.textMuted),
        ],
      ],
    );
  }
}

/// A card its agent changed while it waited (#428, #473).
const updatedMark = 'Updated';

/// The person answered a version of a card its agent changed meanwhile:
/// their answer was set aside, not lost (#473).
String changedAfterAnswer(String name) =>
    '$name changed this after your answer.';

/// The person answered a review card whose file changed meanwhile: their
/// answer was set aside, not lost.
const fileChangedAfterAnswer = 'The file changed after your answer.';

/// What they answered, as they answered it.
String youSaid(String said) => 'You said: “$said”';

/// A procedure's step not answered yet.
bool _open(AskStep step) =>
    step.state == AskStepState.ASK_STEP_STATE_PENDING ||
    step.state == AskStepState.ASK_STEP_STATE_UNSPECIFIED;

/// The step [ask] shows now, 0-based: its first not answered while it
/// waits; null when it has no steps or isn't waiting.
int? currentStep(AskPerson ask) {
  if (ask.state != AskPersonState.ASK_PERSON_STATE_PENDING &&
      ask.state != AskPersonState.ASK_PERSON_STATE_UNSPECIFIED) {
    return null;
  }
  for (final (i, step) in ask.steps.indexed) {
    if (_open(step)) return i;
  }
  return null;
}

/// What a waiting ask says in a line (Needs you, the agent's list): its
/// title, and for a procedure the step it's at.
String askSummary(AskPerson ask) => switch (currentStep(ask)) {
  // A command to run (#494): Claktory's heading, never the agent's title.
  // And its first line, so cards for one machine can be told apart.
  _ when ask.hasCommand() => commandSummary(ask.command),
  final at? => '${ask.title} · Step ${at + 1} of ${ask.steps.length}',
  null => ask.title,
};

/// Whether [ask] was closed by its agent as settled in the talk (#496),
/// rather than resolved by the person: its talk ends with the agent's
/// words, said as the card closed (the backend stamps both with the same
/// moment), with no comment of the person's.
bool agentResolved(AskPerson ask) =>
    ask.resolved &&
    ask.answer.isEmpty &&
    ask.thread.isNotEmpty &&
    ask.thread.last.fromAgent &&
    ask.thread.last.atMs == ask.decidedMs;

/// A part of an ask's body: Markdown, or a command (a fenced code block's
/// contents) to show with Copy.
typedef AskBodyPart = ({String text, bool command});

/// [body] split at its fenced code blocks (``` or ~~~), in order; the
/// Markdown between them as it is.
List<AskBodyPart> askBodyParts(String body) {
  final fence = RegExp(
    r'^ {0,3}(`{3,}|~{3,})[^\n]*\n([\s\S]*?)^ {0,3}\1[ \t]*$',
    multiLine: true,
  );
  final parts = <AskBodyPart>[];
  var at = 0;
  for (final match in fence.allMatches(body)) {
    final before = body.substring(at, match.start).trim();
    if (before.isNotEmpty) parts.add((text: before, command: false));
    final code = match.group(2)!.replaceFirst(RegExp(r'\n$'), '');
    if (code.trim().isNotEmpty) parts.add((text: code, command: true));
    at = match.end;
  }
  final rest = body.substring(at).trim();
  if (rest.isNotEmpty) parts.add((text: rest, command: false));
  return parts;
}

/// A command from an ask's body in a code block, with Copy at its end (on
/// a phone at large text, under it): "Copied" once it's on the clipboard.
/// Opens the file [agentId] asked the person to review ([path]) in that
/// agent's conversation, expanded to fill the window, so every comment
/// they leave on it goes to that agent. On an iPad's workbench, its pane
/// is the one shown.
///
/// [card], when it's opened from the card: its entry and how many messages
/// of its talk it showed, so the review's answer says so (talk_seen).
void openReview(
  BuildContext context,
  String agentId,
  String path, {
  ({int seq, int talkSeen})? card,
}) {
  if (card != null) rememberReviewTalk(agentId, card.seq, card.talkSeen);
  // On a phone, a page over what shows (Needs you stays under it, for
  // Back), with the review's bar at its foot.
  if (PhoneRoutes.of(context)?.showFile(agentId, path) ?? false) return;
  // Answered there, back here.
  rememberReviewReturn(context, agentId, path);
  final scope = AppScope.scope(context);
  final backend = scope.backend;
  if (backend.workspace.agent(agentId) == null) return;
  final workbench = context
      .getInheritedWidgetOfExactType<WorkbenchScope>()
      ?.notifier;
  // From Needs you or Settings, back to the conversation.
  if (scope.page.value != AppPage.agent) AppScope.closePage(context);
  fireAndLog(() async {
    if (backend.workspace.workspace.selectedAgentId != agentId) {
      await backend.selectAgent(agentId);
    }
    final agent = backend.workspace.agent(agentId);
    if (agent == null) return;
    final opened = open(layoutOf(agent), path);
    workbench?.focus(agentId, opened.paneId);
    // Open already and expanded: nothing to change.
    if (agent.layoutFocus == opened.paneId) return;
    await backend.setLayout(agentId, opened.root, focus: opened.paneId);
  }());
}

/// The file a review ask is about: its glyph, name and folder, and Review,
/// which opens it beside the asking agent's conversation, expanded.
class _ReviewFile extends StatelessWidget {
  const _ReviewFile({required this.path, required this.onReview});

  final String path;

  /// Null where there's nothing to review: the file only, no Review.
  final VoidCallback? onReview;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final slash = path.lastIndexOf('/');
    final name = slash < 0 ? path : path.substring(slash + 1);
    final folder = slash <= 0 ? '' : shortPath(path.substring(0, slash));
    final onReview = this.onReview;
    final lead = Row(
      children: [
        FileGlyph(color: palette.textMuted),
        const SizedBox(width: Space.s),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: type.label.copyWith(color: palette.text),
              ),
              if (folder.isNotEmpty)
                Text(
                  folder,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: type.caption.copyWith(color: palette.textFaint),
                ),
            ],
          ),
        ),
      ],
    );
    return Container(
      padding: const EdgeInsets.fromLTRB(Space.m, Space.s, Space.s, Space.s),
      decoration: BoxDecoration(
        color: palette.canvas,
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
      child: onReview == null
          ? lead
          : ActionLine(
              lead: lead,
              action: Button(label: 'Review', onPressed: onReview),
            ),
    );
  }
}

/// Shows or hides a procedure's steps: [label], and whether they show.
class _StepsToggle extends StatelessWidget {
  const _StepsToggle({
    required this.label,
    required this.open,
    required this.onTap,
  });

  final String label;
  final bool open;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Align(
      alignment: AlignmentDirectional.centerStart,
      child: Semantics(
        button: true,
        expanded: open,
        child: Pressable(
          onTap: onTap,
          builder: (context, hovered) => Container(
            // A finger's height on a phone or iPad, the words centred in
            // it (Prosciutto: not a gap under them).
            constraints: BoxConstraints(
              minHeight: touchFirst(context) ? Metrics.of(context).target : 0,
            ),
            alignment: AlignmentDirectional.centerStart,
            // One text, so it wraps at the largest sizes.
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: label,
                    style: TextStyle(
                      color: hovered ? palette.text : palette.textMuted,
                      fontWeight: FontWeight.w500,
                      // A link at rest too, not only on hover (Illuso).
                      decoration: TextDecoration.underline,
                      decorationColor: hovered
                          ? palette.text
                          : palette.textMuted,
                    ),
                  ),
                  TextSpan(
                    text: open ? '  Hide' : '  Show',
                    style: TextStyle(color: palette.textFaint),
                  ),
                ],
              ),
              style: type.caption,
            ),
          ),
        ),
      ),
    );
  }
}

/// One of a procedure's steps, small: its number and words, how it went
/// (done, couldn't do, not reached), and the person's comment on it.
class _StepLine extends StatelessWidget {
  const _StepLine({required this.index, required this.step, this.reached});

  final int index;
  final AskStep step;

  /// Whether it was answered; null where only answered ones are listed.
  final bool? reached;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final outcome = switch (step.state) {
      AskStepState.ASK_STEP_STATE_DONE => 'Done',
      AskStepState.ASK_STEP_STATE_CANT => "Couldn't do",
      _ => reached == false ? 'Not reached' : '',
    };
    // The whole step on one line, its command too ("Open Terminal and run:
    // ykman info"), without a code block's fences.
    final first = proseCodes(
      [
        for (final line in step.text.split('\n'))
          if (line.trim().isNotEmpty && !line.trim().startsWith('```'))
            line.trim(),
      ].join(' '),
    );
    return Padding(
      padding: const EdgeInsets.only(top: Space.xs),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text.rich(
            TextSpan(
              children: [
                TextSpan(text: '${index + 1}. $first'),
                if (outcome.isNotEmpty)
                  TextSpan(
                    text: ' · $outcome',
                    style: TextStyle(
                      color: step.state == AskStepState.ASK_STEP_STATE_CANT
                          ? palette.danger
                          : palette.textFaint,
                    ),
                  ),
              ],
            ),
            style: type.caption.copyWith(color: palette.textMuted),
          ),
          if (step.comment.isNotEmpty)
            Padding(
              padding: const EdgeInsetsDirectional.only(start: Space.m),
              child: Text(
                step.comment,
                style: type.caption.copyWith(color: palette.textFaint),
              ),
            ),
          // The files with its comment (#511).
          if (step.commentAttachments.isNotEmpty)
            Padding(
              padding: const EdgeInsetsDirectional.only(start: Space.m),
              child: CardSentFiles(step.commentAttachments),
            ),
        ],
      ),
    );
  }
}
