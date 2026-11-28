import 'dart:async';
import 'dart:io' show FileSystemException;

import 'package:flutter/foundation.dart' show defaultTargetPlatform;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart' show LaunchMode, launchUrl;

import 'avatar.dart';
import 'approve_elsewhere.dart';
import 'backend.dart';
import 'card_answer.dart';
import 'card_files.dart';
import 'characters/character.dart';
import 'command_card.dart';
import 'controls.dart';
import 'errands/tab_screens.dart' show showCheckView;
import 'decision.dart';
import 'devices.dart' show thisDeviceKind;
import 'dictation.dart';
import 'explorer.dart' show shortPath;
import 'extensions.dart' show logTime;
import 'identity.dart';
import 'ipad/workbench.dart' show WorkbenchScope;
import 'layout.dart' show layoutOf, open;
import 'markdown.dart';
import 'message_time.dart' show clockTime;
import 'mobile/haptics.dart' show Haptics;
import 'mobile/lines.dart';
import 'mobile/metrics.dart';
import 'outbox.dart' show connectionLost, decisionWaiting;
import 'people/messages_setup.dart' show secretClipboard;
import 'people/secret_clipboard.dart' show clearWhenDue;
import 'pipes.dart'
    show
        PipeApprovalAsk,
        PipeApprovalAskView,
        PipeTrustAsk,
        PipeTrustAskView,
        pipePeerClaktory,
        pipesLimiting;
import 'mobile/navigation.dart' show PhoneRoutes;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'protocol/factory/v1/factory.pb.dart';
import 'review_answer.dart' show rememberReviewReturn, rememberReviewTalk;
import 'scope.dart';
import 'selection.dart' show SelectableArea;
import 'taint_label.dart';
import 'visible_codes.dart' show proseCodes;
import 'needs_you_focus.dart' show focusModeOf, setFocusMode;

/// Why an answer waits, under its card.
const askWaiting = decisionWaiting;

/// Approved, but the change the card asked for couldn't be made; its
/// reason follows (AskPerson.action_error).
const approvedNotDone = "Approved, but it couldn't be done";

/// How the Claktory refuses an answer to a request its agent changed
/// since it was shown (#428), and the words a Claktory from before the
/// Requests rename (#588) still says.
const cardChanged = 'This request changed.';
const _cardChangedBefore = 'This card changed.';

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
    // Emptied: what the agent changed is no longer news over the field.
    final news = !typed && _changedWhileTyping;
    if (news) _changedWhileTyping = false;
    if ((inDock || news) && mounted) setState(() {});
  }

  /// The written answer's focus, or a step's what's in the way: never
  /// both on the card at once.
  final _focus = FocusNode(debugLabel: 'Ask answer');
  var _busy = false;
  String? _error;

  /// A step's Can't do this was pressed with nothing typed (Lucy's Needs
  /// you cards §3.3): the cursor went to the field, which now asks what's
  /// in the way, and a second press sends it bare.
  var _cantAsked = false;

  /// Can't do this on step [at]: with words (or files) it goes at once,
  /// with them as the reason; with none, the first press puts the cursor
  /// in the field and the second sends it bare.
  void _cant(int at) {
    if (_typed || _holdsFiles || _cantAsked) {
      _answer(dismiss: true, step: at);
      return;
    }
    setState(() => _cantAsked = true);
    _focus.requestFocus();
  }

  /// Its agent changed the card while the person was typing in it (N4b):
  /// said over the field until it's empty.
  var _changedWhileTyping = false;

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
    // Recovery codes, only while the card waits on screen (#435 W8b).
    if (!_pendingAsk(widget.ask)) _codes = null;
    // A code card's end, felt (Prosciutto): filled in, or the page changed.
    if (widget.ask.code && _pendingAsk(old.ask) && !_pendingAsk(widget.ask)) {
      if (widget.ask.actionError.isNotEmpty) {
        unawaited(Haptics.warning());
      } else if (widget.ask.state == AskPersonState.ASK_PERSON_STATE_ANSWERED) {
        unawaited(Haptics.success());
      }
    }
    // Changed by its agent while words were typed (N4b): they stay, and
    // the field says what happened.
    if (old.ask.revision != widget.ask.revision &&
        _text.text.trim().isNotEmpty) {
      _changedWhileTyping = true;
    }
    // The next step: a clean card for it. Not when its agent changed the
    // card (#428), which can move the step back: what the person typed
    // stays, under what changed.
    if (old.ask.revision == widget.ask.revision &&
        currentStep(old.ask) != currentStep(widget.ask)) {
      _text.clear();
      _cantAsked = false;
      _changedWhileTyping = false;
      _error = null;
      _waiting = false;
      _whenBack = null;
    }
  }

  @override
  void dispose() {
    _copied?.cancel();
    _backend?.connected.removeListener(_connectedChanged);
    _backend?.attachments.removeListener(_filesChanged);
    // The draft stays with the card's key, for wherever it shows next.
    _listened?.removeListener(_typing);
    _focus.dispose();
    _code.dispose();
    _codeFocus.dispose();
    super.dispose();
  }

  bool _pendingAsk(AskPerson ask) =>
      !ask.withdrawn &&
      (ask.state == AskPersonState.ASK_PERSON_STATE_PENDING ||
          ask.state == AskPersonState.ASK_PERSON_STATE_UNSPECIFIED);

  /// A code card's code (#435 W8): never a draft, a recording or a file,
  /// and gone once sent.
  final _code = TextEditingController();
  final _codeFocus = FocusNode();

  /// Can't do this… → In my own words: the card asks what's in the way
  /// in place of the code or the payment's buttons.
  var _ownWords = false;

  var _openedHere = false;

  /// Claktory is told first, so every device shows I've paid, and a
  /// Claktory that can't take payments refuses before anything opens.
  Future<void> _openPayment(Uri page, {bool again = false}) async {
    if (_busy) return;
    if (!again) {
      setState(() {
        _busy = true;
        _error = null;
        _waiting = false;
      });
      try {
        await AppScope.of(context).paymentOpened(widget.agentId, widget.seq);
      } on RequestFailure catch (failure) {
        if (mounted) setState(() => _error = failure.message);
        if (_dock) rethrow;
        return;
      } finally {
        if (mounted) setState(() => _busy = false);
      }
    }
    var opened = false;
    try {
      opened = await openPaymentPage(page);
    } on PlatformException {
      opened = false;
    }
    if (!mounted) return;
    setState(() {
      _openedHere = opened || _openedHere;
      _error = opened ? null : "The payment page didn't open here.";
    });
  }

  Future<void> _cantPay(BuildContext context) async {
    const own = '\u0000own';
    final reason = await showActions<String>(
      context,
      actions: const [
        SheetAction('Not now', 'Not now'),
        SheetAction("I don't want to pay this", "I don't want to pay this"),
        SheetAction('In my own words…', own),
      ],
    );
    if (!mounted || reason == null) return;
    if (reason == own) {
      setState(() => _ownWords = true);
      _focus.requestFocus();
      return;
    }
    await _answer(dismiss: true, reason: reason);
  }

  /// Fills the code in (AnswerAsk with it as the text): Claktory types it
  /// into the page, and the agent never sees it.
  Future<void> _fill({bool again = false}) async {
    final code = _code.text.trim();
    if (_busy || code.isEmpty) return;
    await _send(
      (backend) => backend.answerAsk(
        widget.agentId,
        widget.seq,
        text: code,
        revision: widget.ask.revision,
        talkSeen: widget.ask.thread.length,
        from: widget.from,
      ),
      () => _fill(again: true),
      again: again,
      // Never the code itself, even set aside.
      said: 'Fill in',
    );
    if (mounted && _error == null) _code.clear();
  }

  /// Can't do this… on a code card: the reasons a code doesn't come, as
  /// focus mode's no offers them, or the person's own words.
  Future<void> _cantFill(BuildContext context) async {
    const own = '\u0000own';
    final reason = await showActions<String>(
      context,
      actions: const [
        SheetAction('No code arrived', 'No code arrived'),
        SheetAction('Not now', 'Not now'),
        SheetAction('In my own words…', own),
      ],
    );
    if (!mounted || reason == null) return;
    if (reason == own) {
      setState(() => _ownWords = true);
      _focus.requestFocus();
      return;
    }
    await _answer(dismiss: true, reason: reason);
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
    // A no's reason from focus mode's dock goes with what's typed. Execute
    // never takes words: it runs exactly what it shows (Fugo, #533 D2).
    // Nor does an errand step's Done: words go only with Can't do this,
    // as its line says, so none are kept that its agent never hears
    // (Abbacchio's row 1640).
    final done =
        !dismiss &&
        (isGenericHandoff(widget.ask) ||
            isPayment(widget.ask) ||
            isCheck(widget.ask));
    final text = dismiss
        ? noComment(reason, _text.text)
        : confirmed || done
        ? ''
        : _text.text.trim();
    if (_busy ||
        (step == null &&
            !dismiss &&
            !done &&
            widget.ask.wantsText &&
            text.isEmpty)) {
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
        attachments: confirmed || done ? const [] : await _files(backend),
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
          : done && isPayment(widget.ask)
          ? "I've paid"
          : done
          ? 'Done'
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
      if (failure.message.startsWith(cardChanged) ||
          failure.message.startsWith(_cardChangedBefore)) {
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
  Color _statusColor(Palette palette) => _error == null || _busy
      ? palette.textFaint
      : _waiting
      ? palette.textMuted
      : palette.danger;

  /// What the card's line says in place of what pressing does (Lucy's
  /// rule 9): Sending…, that it sends once the connection's back, or why
  /// it was refused; null when none.
  String? get _status => _busy ? 'Sending…' : _error;

  /// Over the field, once its agent changed the card while the person
  /// typed in it (N4b).
  /// Trish's words (42223).
  String? _changedLine(String name) {
    if (!_changedWhileTyping) return null;
    final at = currentStep(widget.ask);
    return at == null
        ? '$name updated this request while you were typing. Your words are '
              'kept.'
        : '$name changed step ${at + 1} while you were typing. Your words '
              'are kept.';
  }

  /// A question answered in a dock (focus mode's): its field, yes and no
  /// drawn there, not on the card. Not Execute, a procedure or a pipe's
  /// card, whose controls stay their own.
  bool get _docked {
    final ask = widget.ask;
    return inDock &&
        !ask.hasCommand() &&
        ask.steps.isEmpty &&
        !ask.hasPipeTrust() &&
        !ask.hasPipeApproval() &&
        // An errand's cards keep their own buttons (#435).
        !ask.hasHandoff();
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
    // A recovery-codes card has only its own two answers, no no (Fugo
    // 40694).
    if (isRecoveryCodes(ask)) return null;
    if (isPayment(ask)) {
      final opened = ask.handoff.openedMs > 0;
      final page = paymentPage(ask);
      return CardAnswer(
        key: key,
        docked: false,
        yes: opened ? "I've paid" : 'Open on this $thisDeviceKind',
        no: "Can't do this…",
        busy: busy,
        error: _waiting ? null : _error,
        status: _waiting ? _error : null,
        onYes: busy || (!opened && page == null)
            ? null
            : opened
            ? () => _fromDock(() => _answer(dismiss: false))
            : () => _fromDock(() => _openPayment(page!)),
        onNo: dismiss,
      );
    }
    if (isAgreement(ask) ||
        isTwoStep(ask) ||
        isGenericHandoff(ask) ||
        isCheck(ask)) {
      final name =
          _backend?.workspace.agent(widget.agentId)?.name ?? 'the agent';
      final errand = _errandButtons(name)!;
      return CardAnswer(
        key: key,
        docked: false,
        yes: errand.yes,
        no: errand.no,
        typed: errand.reply && _typed,
        busy: busy,
        error: _waiting ? null : _error,
        status: _waiting ? _error : null,
        onYes: busy
            ? null
            : isCheck(ask)
            ? () async => _solve()
            : () => _fromDock(() => _answer(dismiss: false)),
        onSubmit: busy || !errand.reply ? null : () => _fromDock(_reply),
        onNo: dismiss,
      );
    }
    if (ask.code) {
      // Its field and buttons are its own, never the dock's (Pesci,
      // Prosciutto: decision (a)).
      return CardAnswer(
        key: key,
        docked: false,
        yes: 'Fill in',
        no: "Can't do this…",
        busy: busy,
        error: _waiting ? null : _error,
        status: _waiting ? _error : null,
        onYes: busy || _code.text.trim().isEmpty
            ? null
            : () => _fromDock(_fill),
        onNo: dismiss,
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
      placeholder: wantsText ? 'Your answer' : replyPlaceholder(name),
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
              // The site is Claktory's reading of the page, never the
              // agent's words.
              if (isPayment(ask))
                Flexible(
                  child: Text.rich(
                    TextSpan(
                      children: [
                        TextSpan(text: '$name asks you to pay on '),
                        TextSpan(
                          text: paymentSite(ask),
                          style: const TextStyle(fontWeight: FontWeight.w600),
                        ),
                      ],
                    ),
                    style: type.emphasis.copyWith(
                      color: dismissed || ask.withdrawn
                          ? palette.textMuted
                          : palette.text,
                    ),
                  ),
                )
              else
                Flexible(
                  child: Text(
                    // Answered, dismissed or taken back, it needs
                    // nothing.
                    ask.code
                        ? (ask.withdrawn || !pending
                              ? '$name needed a code'
                              : '$name needs a code')
                        // Whether or not two-step was asked for: the site
                        // showed them.
                        : isRecoveryCodes(ask)
                        ? "$name's browser saved recovery codes"
                        // Not an OK: the person does it themselves.
                        : isCheck(ask)
                        ? (ask.withdrawn || !pending
                              ? '$name needed your help'
                              : '$name needs your help')
                        : ask.hasHandoff()
                        ? (ask.withdrawn || !pending
                              ? '$name needed your OK'
                              : '$name needs your OK')
                        : ask.withdrawn || !pending
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
          else if (ask.code) ...[
            // Claktory's heading, the site as Chrome has it, nothing from
            // the page or the agent (Fugo 41442).
            Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: handoffHost(ask),
                    style: const TextStyle(fontWeight: FontWeight.w600),
                  ),
                  const TextSpan(text: ' asks for a code'),
                ],
              ),
              style: type.emphasis.copyWith(
                color: dismissed ? palette.textMuted : palette.text,
              ),
            ),
            if (pending && !ask.withdrawn) ...[
              const SizedBox(height: Space.xs),
              Text(
                'Sent to your phone or email. Claktory types it into that '
                'page once, and $name never sees it.',
                style: type.body.copyWith(color: palette.textMuted),
              ),
            ],
          ] else if (isPayment(ask)) ...[
            if (ask.handoff.what.isNotEmpty)
              Text(
                proseCodes('$name: “${ask.handoff.what}”'),
                style: type.body.copyWith(color: palette.textMuted),
              ),
          ] else if (isCheck(ask)) ...[
            // Claktory's heading, the site as Claktory read it; the agent's
            // words under it as its own.
            Text.rich(
              TextSpan(
                children: [
                  const TextSpan(text: 'Solve a check on '),
                  TextSpan(
                    text: checkSite(ask),
                    style: const TextStyle(fontWeight: FontWeight.w600),
                  ),
                ],
              ),
              style: type.emphasis.copyWith(
                color: dismissed ? palette.textMuted : palette.text,
              ),
            ),
            if (ask.handoff.what.isNotEmpty) ...[
              const SizedBox(height: Space.xs),
              Text(
                proseCodes('$name: “${ask.handoff.what}”'),
                style: type.body.copyWith(color: palette.textMuted),
              ),
            ],
          ] else if (ask.hasHandoff()) ...[
            // Claktory's heading for an errand's card ("play.google.com
            // asks you to agree"), the site in bold.
            _HostTitle(
              title: ask.title,
              host: handoffHost(ask),
              style: type.emphasis.copyWith(
                color: dismissed ? palette.textMuted : palette.text,
              ),
            ),
            // Where any other errand step is, as Chrome has the page's
            // site: Claktory's words, never the page's (#435 W10).
            if (isGenericHandoff(ask) && handoffHost(ask).isNotEmpty) ...[
              const SizedBox(height: Space.xs),
              Text.rich(
                TextSpan(
                  children: [
                    const TextSpan(text: 'On '),
                    TextSpan(
                      text: handoffHost(ask),
                      style: const TextStyle(fontWeight: FontWeight.w600),
                    ),
                  ],
                ),
                style: type.body.copyWith(color: palette.textMuted),
              ),
            ],
            // The box's words on the page, fenced as the site's.
            if (isAgreement(ask) && ask.handoff.agreement.isNotEmpty) ...[
              const SizedBox(height: Space.s),
              SiteWords(ask.handoff.agreement),
            ],
          ] else
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
          ] else if (isRecoveryCodes(ask) && pending && !ask.withdrawn) ...[
            // Claktory's state first (Ghiaccio 43944): a locked vault keeps
            // them only until it restarts, said as a warning; then the
            // agent's words, as its own. The buttons say the rest.
            if (bodyParagraphs(ask.body).firstOrNull case final state?) ...[
              const SizedBox(height: Space.xs),
              Text(
                state,
                style: type.body.copyWith(
                  color: recoveryCodesAtRisk(ask)
                      ? palette.danger
                      : palette.textMuted,
                ),
              ),
            ],
            if (ask.handoff.what.isNotEmpty) ...[
              const SizedBox(height: Space.xs),
              Text(
                proseCodes('$name: “${ask.handoff.what}”'),
                style: type.caption.copyWith(
                  color: palette.textMuted,
                  fontStyle: FontStyle.italic,
                ),
              ),
            ],
          ] else if (ask.body.isNotEmpty &&
              !ask.hasCommand() &&
              !ask.code &&
              !isPayment(ask) &&
              !isCheck(ask) &&
              !isRecoveryCodes(ask)) ...[
            const SizedBox(height: Space.xs),
            // Past 8 lines, its middle folds (Lucy's Needs you cards).
            FoldedBody(
              body: ask.body,
              build: (text) => Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                mainAxisSize: MainAxisSize.min,
                children: [
                  for (final (i, part) in askBodyParts(text).indexed) ...[
                    if (i > 0) const SizedBox(height: Space.s),
                    part.command
                        ? CommandBlock(part.text)
                        : Markdown(
                            proseCodes(part.text),
                            color: palette.textMuted,
                          ),
                  ],
                ],
              ),
            ),
          ],
          // A site's recovery codes, asked for only while the card is on
          // screen and waiting (#435 W8b).
          if (isRecoveryCodes(ask) && pending && !ask.withdrawn) ...[
            const SizedBox(height: Space.s),
            // Shown and kept only by the person (ShowRecoveryCodes and
            // KeepRecoveryCodes; Fugo 52432): never on Windows.
            if (approvesElsewhere)
              const ApproveElsewhere(text: recoveryCodesElsewhereText)
            else
              ..._recoveryCodes(palette, type),
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
          // its agent's replies, above the card's buttons; a procedure's
          // waiting step has it just above the step (#533).
          if (ask.thread.isNotEmpty &&
              !(ask.steps.isNotEmpty &&
                  !ask.withdrawn &&
                  currentStep(ask) != null)) ...[
            const SizedBox(height: Space.m),
            AskThread(
              thread: ask.thread,
              agent: agent,
              name: name,
              // A procedure's: each turn about a step says which (#533).
              step: ask.steps.isEmpty ? null : 0,
            ),
          ],
          const SizedBox(height: Space.m),
          // Taken back by its agent (#407): nothing to answer, and never
          // the person's dismissal.
          if (ask.withdrawn) ...[
            WithdrawnLine(
              agent: agent,
              name: name,
              reason: ask.withdrawReason,
              decidedMs: decided,
            ),
            // What the person was typing as it was taken back stays, to
            // send to its agent or copy (#533 N6).
            ..._unsent(palette, type, name),
          ] else if (ask.steps.isNotEmpty)
            ..._procedure(palette, type, color, name, pending, decided)
          else if (pending && ask.hasCommand())
            ..._execute(palette, type, name)
          else if (pending && ask.code)
            ..._codeCard(palette, type, name)
          else if (pending && isPayment(ask))
            ..._paymentCard(palette, name)
          else if (pending && isRecoveryCodes(ask))
            // Nothing to save or keep on Windows, where they never show.
            ...(approvesElsewhere ? const <Widget>[] : _recoveryRow(palette))
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
      return CardDropArea(child: card);
    }
    return CardDropArea(child: SelectableArea(child: card));
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

  /// Opens the check's tab, live, for the person to solve.
  void _solve() {
    final name =
        AppScope.of(context).workspace.agent(widget.agentId)?.name ??
        'the agent';
    showCheckView(
      context,
      (
        agentId: widget.agentId,
        seq: widget.seq,
        site: checkSite(widget.ask),
        what: widget.ask.handoff.what,
      ),
      back: widget.from == AskAnsweredFrom.ASK_ANSWERED_FROM_NEEDS_YOU
          ? 'Back to Needs you'
          : 'Back to $name',
    );
  }

  /// An agreement or two-step card's buttons and line (#435 W8b, Lucy's
  /// §3.6): null for every other card.
  ({String yes, String no, String line, String verbing, bool reply})?
  _errandButtons(String name) {
    final ask = widget.ask;
    if (isAgreement(ask)) {
      return (
        yes: 'Accept',
        no: "Can't do this",
        line: 'Claktory checks only this box on ${handoffHost(ask)}.',
        verbing: 'accepting',
        reply: true,
      );
    }
    if (isTwoStep(ask)) {
      return (
        yes: 'Approve',
        no: "Can't do this",
        line:
            '$name may turn it on, with codes by text message or email '
            'only.',
        verbing: 'approving',
        reply: false,
      );
    }
    if (isCheck(ask)) {
      return (
        yes: 'Solve check',
        no: "Can't do this",
        line: '$name waits until you solve it.',
        verbing: 'solving',
        reply: false,
      );
    }
    // Any other step only the person can take for an errand (#435 W10: a
    // payment, a sign-in…): Done carries on from the page as it is, sending
    // no words; what's typed goes only with Can't do this, as its reason.
    if (isGenericHandoff(ask)) {
      return (
        yes: 'Done',
        no: "Can't do this",
        line: '$name waits until you choose Done.',
        verbing: 'finishing',
        reply: false,
      );
    }
    return null;
  }

  /// A recovery-codes card's codes (#435 W8b): asked for as it shows, kept
  /// only while it's on screen, never in a draft or the Workspace.
  List<String>? _codes;
  String? _codesError;
  var _codesAsked = false;

  Future<void> _showCodes() async {
    setState(() {
      _codesAsked = true;
      _codesError = null;
    });
    try {
      final codes = await AppScope.of(context)
          .showRecoveryCodes(widget.agentId, widget.seq);
      if (mounted) setState(() => _codes = codes);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _codesError = failure.message);
    }
  }

  /// The codes, two to a row in the code face, with Copy: the clipboard
  /// that clears itself where the device has one (Fugo 41442). Gone, why,
  /// and Show to ask again.
  List<Widget> _recoveryCodes(Palette palette, TypeScale type) {
    if (!_codesAsked) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && !_codesAsked) unawaited(_showCodes());
      });
    }
    final codes = _codes;
    if (codes == null) {
      final error = _codesError;
      return [
        Text(
          error ?? 'Getting the codes…',
          style: type.caption.copyWith(
            color: error == null ? palette.textFaint : palette.danger,
          ),
        ),
        if (error != null)
          QuietLink(label: 'Show', onTap: () => unawaited(_showCodes())),
      ];
    }
    final face = type.body.copyWith(
      fontFamily: TextStyles.mono,
      color: palette.text,
    );
    return [
      Container(
        padding: const EdgeInsets.all(Space.m),
        decoration: BoxDecoration(
          border: Border.all(color: palette.hairline),
          borderRadius: BorderRadius.circular(Radii.control),
        ),
        child: LayoutBuilder(
          builder: (context, constraints) {
            final two = constraints.maxWidth >= 240;
            final half = (codes.length / 2).ceil();
            Widget column(Iterable<String> part) => Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [for (final code in part) Text(code, style: face)],
            );
            return two
                ? Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Expanded(child: column(codes.take(half))),
                      Expanded(child: column(codes.skip(half))),
                    ],
                  )
                : column(codes);
          },
        ),
      ),
      QuietLink(
        label: _copied != null ? 'Copied' : 'Copy',
        onTap: () => unawaited(_copyCodes(codes)),
      ),
    ];
  }

  Future<void> _copyCodes(List<String> codes) async {
    final text = codes.join('\n');
    final token = await secretClipboard.copy(text);
    if (token == null) {
      await Clipboard.setData(ClipboardData(text: text));
    } else {
      clearWhenDue(secretClipboard, token, const Duration(seconds: 30 * 60));
    }
    if (!mounted) return;
    _copied?.cancel();
    setState(
      () => _copied = Timer(const Duration(seconds: 2), () {
        if (mounted) setState(() => _copied = null);
      }),
    );
  }

  /// A recovery-codes card's two answers, and no others (Fugo 40694): I
  /// saved them (Claktory deletes its copy) and Keep in Secrets (a secret
  /// given to no agent). No field: nothing goes to the agent.
  List<Widget> _recoveryRow(Palette palette) => [
    CardRow(
      line: CardLine(
        _status ??
            "They're deleted when this card closes. Save them somewhere "
                'safe, or keep them in Secrets.',
        color: _statusColor(palette),
      ),
      quiet: [
        Button(
          label: 'I saved them',
          onPressed: _busy ? null : () => _answer(dismiss: false),
        ),
      ],
      primary: Button(
        label: 'Keep in Secrets',
        primary: true,
        onPressed: _busy ? null : () => unawaited(_keepCodes()),
      ),
    ),
  ];

  Future<void> _keepCodes({bool again = false}) => _send(
    (backend) => backend.keepRecoveryCodes(widget.agentId, widget.seq),
    () => _keepCodes(again: true),
    again: again,
    said: 'Keep in Secrets',
  );

  /// A code card waiting (#435 W8, Lucy's §3.6): one field, the code
  /// only (no comment, mic, files or draft, so nothing typed here reaches
  /// the agent), then Claktory's line, Can't do this… and Fill in. Return
  /// fills in with a keyboard; a phone's keys send nothing (rule 6). On a
  /// touch screen it takes the cursor as it opens, for the code the
  /// keyboard offers from Messages (Prosciutto), in a conversation only
  /// when it's the card waiting there now.
  List<Widget> _codeCard(Palette palette, TypeScale type, String name) {
    final host = handoffHost(widget.ask);
    if (_ownWords) return _ownWordsBlock(palette, name);
    final agent = AppScope.of(context).workspace.agent(widget.agentId);
    final waitingHere =
        _dock ||
        agent == null ||
        agent.waitingSeqs.isEmpty ||
        agent.waitingSeqs.last.toInt() == widget.seq;
    void fill() => unawaited(_fill());
    return [
      KeepInSight(
        focus: _codeFocus,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            CallbackShortcuts(
              bindings: {
                const SingleActivator(LogicalKeyboardKey.enter): fill,
                const SingleActivator(LogicalKeyboardKey.numpadEnter): fill,
              },
              child: TextField(
                controller: _code,
                focusNode: _codeFocus,
                placeholder: 'Code',
                semanticsLabel: 'The code $host sent',
                autofocus: touchFirst(context) && waitingHere,
                // Digits only when the page's field is (Chrome says): the
                // number pad; otherwise letters too, nothing guessed.
                keyboardType: widget.ask.handoff.numeric
                    ? TextInputType.number
                    : TextInputType.visiblePassword,
                textInputAction: TextInputAction.done,
                // The on-screen keyboard's Done fills it in, as Return does
                // (Prosciutto 44920); nothing while empty or sending.
                onSubmitted: (_) => fill(),
                autofillHints: const [AutofillHints.oneTimeCode],
                textCapitalization: TextCapitalization.characters,
                autocorrect: false,
                enableSuggestions: false,
                smartPunctuation: false,
                inputFormatters: codeFormatters,
              ),
            ),
            const SizedBox(height: Space.s),
            ListenableBuilder(
              listenable: _code,
              builder: (context, _) => CardRow(
                line: CardLine(
                  _status ?? 'Claktory fills it in on $host.',
                  color: _statusColor(palette),
                ),
                quiet: [
                  Builder(
                    builder: (context) => Button(
                      label: "Can't do this…",
                      onPressed: _busy
                          ? null
                          : () => unawaited(_cantFill(context)),
                    ),
                  ),
                ],
                primary: Button(
                  label: 'Fill in',
                  primary: true,
                  onPressed: _busy || _code.text.trim().isEmpty ? null : fill,
                ),
              ),
            ),
          ],
        ),
      ),
    ];
  }

  List<Widget> _ownWordsBlock(Palette palette, String name) => [
    CardAnswerBlock(
      focus: _focus,
      field: (_) => DecisionComment(
        controller: _text,
        focusNode: _focus,
        agent: name,
        enabled: !_busy,
        placeholder: "What's in the way? $name sees this.",
        onSubmit: () => _answer(dismiss: true),
      ),
      row: CommentTyped(
        controller: _text,
        builder: (context, typed) => CardRow(
          line: switch (_status ??
              (isPayment(widget.ask)
                  ? null
                  : 'Nothing is filled in. $name gets your words.')) {
            final line? => CardLine(line, color: _statusColor(palette)),
            null => null,
          },
          quiet: [
            Button(
              label: 'Back',
              onPressed: _busy ? null : () => setState(() => _ownWords = false),
            ),
          ],
          primary: Button(
            label: "Can't do this",
            primary: true,
            onPressed: _busy || !typed ? null : () => _answer(dismiss: true),
          ),
        ),
      ),
    ),
  ];

  /// No field: nothing typed on a payment card reaches the agent except
  /// with Can't do this.
  List<Widget> _paymentCard(Palette palette, String name) {
    if (_ownWords) return _ownWordsBlock(palette, name);
    final opened = widget.ask.handoff.openedMs > 0;
    final page = paymentPage(widget.ask);
    final safari = _openedHere && defaultTargetPlatform == TargetPlatform.iOS;
    return [
      CardRow(
        line: CardLine(
          _status ??
              (opened
                  ? safari
                        ? "Opened in Safari. Come back here once you've paid."
                        : "Opened in your browser. Come back here once "
                              "you've paid."
                  : '$name waits until you pay.'),
          color: _status == null ? palette.textMuted : _statusColor(palette),
        ),
        quiet: [
          Builder(
            builder: (context) => Button(
              label: "Can't do this…",
              onPressed: _busy ? null : () => unawaited(_cantPay(context)),
            ),
          ),
          if (opened && page != null)
            Button(
              label: 'Open again',
              onPressed: _busy
                  ? null
                  : () => unawaited(_openPayment(page, again: true)),
            ),
        ],
        primary: opened
            ? Button(
                label: "I've paid",
                primary: true,
                onPressed: _busy ? null : () => _answer(dismiss: false),
              )
            : page == null
            ? null
            : Button(
                label: 'Open on this $thisDeviceKind',
                primary: true,
                onPressed: _busy ? null : () => unawaited(_openPayment(page)),
              ),
      ),
    ];
  }

  /// An Execute card waiting (#494): Dismiss or Execute, which runs
  /// exactly the command shown, once; with Claktory's warnings, a second
  /// tap to confirm. No reply: the backend refuses one on it.
  List<Widget> _execute(Palette palette, TypeScale type, String name) {
    void execute() => _executeNow();

    return [
      // Why not, for its agent (#533 N5), said first: it goes only with
      // Dismiss, never with Execute, which runs exactly what it shows.
      CardAnswerBlock(
        focus: _focus,
        field: (_) => DecisionComment(
          controller: _text,
          agent: name,
          enabled: !_busy,
          focusNode: _focus,
          placeholder: 'Why not? $name sees this only if you dismiss.',
          above: _changedLine(name),
          keep: _keep(name),
        ),
        row: CommentTyped(
          controller: _text,
          builder: (context, typed) => CardRow(
            // "June sees what it prints." only: a hint (Lucy 45566).
            quietLine: _status == null && !_confirming && !typed,
            line: CardLine(
              _status ??
                  (_confirming
                      ? "Read the warnings above, then run it only if you're "
                            'sure.'
                      : typed
                      ? 'Your words go only with Dismiss. Execute runs the '
                            'command exactly as shown.'
                      : '$name sees what it prints.'),
              color: _confirming && _status == null
                  ? palette.danger
                  : _statusColor(palette),
            ),
            quiet: [
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
            ],
            primary: Button(
              label: _confirming ? 'Run anyway' : 'Execute',
              primary: true,
              onPressed: _busy ? null : execute,
            ),
          ),
        ),
      ),
    ];
  }

  /// Sent to its agent from a card it took back: "Sent to June".
  var _unsentGone = false;
  Timer? _copied;

  /// Words typed on a card its agent took back meanwhile (#533 N6): not
  /// sent, with Send to its agent (a message in its conversation, its
  /// files with it) and Copy.
  List<Widget> _unsent(Palette palette, TypeScale type, String name) {
    final words = _text.text.trim();
    if (words.isEmpty && !_holdsFiles) {
      if (!_unsentGone) return const [];
      return [
        const SizedBox(height: Space.s),
        Text(
          'Sent to $name.',
          style: type.caption.copyWith(color: palette.textFaint),
        ),
      ];
    }
    return [
      const SizedBox(height: Space.s),
      if (words.isNotEmpty)
        Text.rich(
          TextSpan(
            children: [
              TextSpan(
                text: 'Not sent: ',
                style: TextStyle(
                  color: palette.textMuted,
                  fontWeight: FontWeight.w500,
                ),
              ),
              TextSpan(text: words),
            ],
          ),
          style: type.body.copyWith(color: palette.text),
        ),
      _chips(),
      const SizedBox(height: Space.xs),
      CardRow(
        line: CardLine(
          words.isEmpty
              ? 'Not sent: your files.'
              : '$name took the request back before you sent this.',
        ),
        quiet: [
          if (words.isNotEmpty)
            Button(
              label: _copied == null ? 'Copy' : 'Copied',
              onPressed: () => _copyUnsent(words),
            ),
        ],
        primary: Button(
          label: 'Send to $name',
          primary: true,
          onPressed: () => _sendUnsent(words),
        ),
      ),
    ];
  }

  Future<void> _copyUnsent(String words) async {
    await Clipboard.setData(ClipboardData(text: words));
    if (!mounted) return;
    _copied?.cancel();
    setState(
      () => _copied = Timer(const Duration(milliseconds: 1500), () {
        if (mounted) setState(() => _copied = null);
      }),
    );
  }

  /// As a message in its agent's conversation, through its outbox: it
  /// shows there at once, and goes once the connection's there.
  void _sendUnsent(String words) {
    final backend = _backend;
    if (backend == null) return;
    backend.outbox.send(
      widget.agentId,
      words,
      files: backend.attachments.take(_filesKey),
    );
    _text.clear();
    setState(() => _unsentGone = true);
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
    // An errand's agreement or two-step card (#435 W8b): its own words on
    // the buttons and the line; a two-step one takes no reply, its words
    // going only with Can't do this.
    final errand = _errandButtons(name);
    final reply = errand?.reply ?? true;
    return [
      // The field, open from the start (Lucy's rule 4): a written answer's
      // answer, or words that turn an approval's buttons into Submit
      // (#370). Return sends them, never Approve. Files with it (#511) go
      // with whichever is pressed.
      CardAnswerBlock(
        focus: _focus,
        field: (_) => _dropping(
          name,
          DecisionComment(
            controller: _text,
            agent: name,
            enabled: !_busy,
            focusNode: _focus,
            placeholder: wantsText
                ? 'Your answer'
                : reply
                ? null
                : "Why not? $name sees this only with Can't do this.",
            onSubmit: wantsText
                ? () => _answer(dismiss: false)
                : reply
                ? _reply
                : null,
            above: _changedLine(name),
            keep: _keep(name),
            attach: _attach(),
            files: _chips(),
            holdsFiles: _holdsFiles,
          ),
        ),
        row: CommentTyped(
          controller: _text,
          builder: (context, typed) {
            final submit = typed && !wantsText && reply;
            final line =
                _status ??
                (submit
                    ? insteadLine(errand?.verbing ?? 'approving')
                    : typed && !reply
                    ? "Your words go only with Can't do this."
                    : _afterTalkLine(name) ??
                          errand?.line ??
                          (widget.ask.reviewPath.isNotEmpty && !wantsText
                              ? 'Your comments on it go to $name.'
                              : null));
            return CardRow(
              line: line == null
                  ? null
                  : CardLine(
                      line,
                      color: _status == null && !submit
                          ? palette.textMuted
                          : _statusColor(palette),
                    ),
              quiet: [
                if (!submit)
                  Button(
                    label: errand?.no ?? 'Dismiss',
                    onPressed: _busy ? null : () => _answer(dismiss: true),
                  ),
              ],
              primary: submit
                  ? SubmitButton(onPressed: _busy ? null : _reply)
                  : Button(
                      label: errand?.yes ?? (wantsText ? 'Answer' : 'Approve'),
                      primary: true,
                      onPressed: _busy || (wantsText && !typed)
                          ? null
                          : isCheck(widget.ask)
                          ? _solve
                          : () => _answer(dismiss: false),
                    ),
            );
          },
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
    final line = _afterTalkLine(name);
    if (line == null) return const [];
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

  /// [_afterTalk]'s words, for a card's line; null when there's no talk
  /// to agree with.
  String? _afterTalkLine(String name) {
    final ask = widget.ask;
    if (ask.approveFollowsTalk) {
      return ask.wantsText
          ? "Your answer follows $name's last reply."
          : "Approve means you agree with $name's last reply.";
    }
    if (ask.thread.isNotEmpty &&
        (ask.actionDescription.isNotEmpty ||
            ask.hasPipeApproval() ||
            ask.hasPipeTrust())) {
      return 'Approving does only what the request says, not what was said in '
          'the talk.';
    }
    return null;
  }

  /// Under a card with a thread, beneath its row, quiet links (Lucy's
  /// rule 3): Reply, for a card whose typed words are otherwise its answer
  /// (an approval's are a reply already, Submit; a step's, Send), and
  /// Resolve, which ends it deciding nothing.
  List<Widget> _talk(TypeScale type, {required bool reply}) => [
    const SizedBox(height: Space.xs),
    CommentTyped(
      controller: _text,
      builder: (context, typed) => Wrap(
        spacing: Space.m,
        children: [
          if (reply)
            QuietLink(
              label: 'Reply',
              // Words, or files alone (#511).
              onTap: _busy || !(typed || _holdsFiles) ? null : _reply,
            ),
          QuietLink(label: 'Resolve', onTap: _busy ? null : _resolve),
        ],
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
        // The talk about it, above the step it's about (#533).
        if (ask.thread.isNotEmpty) ...[
          AskThread(
            thread: ask.thread,
            agent: AppScope.of(context).workspace.agent(widget.agentId),
            name: name,
            // Turns about an earlier step say which (#533).
            step: at + 1,
          ),
          const SizedBox(height: Space.m),
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
        // The field, open from the start (Lucy's rule 4). Return in it is
        // Send, once something is typed: never Done (#533 N1).
        CardAnswerBlock(
          focus: _focus,
          field: (_) => _dropping(
            name,
            DecisionComment(
              controller: _text,
              agent: name,
              enabled: !_busy,
              focusNode: _focus,
              onSubmit: _reply,
              // Can't do this pressed with nothing typed: what it asks.
              placeholder: _cantAsked ? "What's in the way?" : null,
              above: _changedLine(name),
              keep: _keep(name),
              attach: _attach(),
              files: _chips(),
              holdsFiles: _holdsFiles,
            ),
          ),
          row: CommentTyped(
            controller: _text,
            builder: (context, typed) {
              final send = typed || _holdsFiles;
              final done = Button(
                label: 'Done',
                primary: !send,
                onPressed: _busy
                    ? null
                    : () => _answer(dismiss: false, step: at),
              );
              return CardRow(
                // "The next step shows once…" only: a hint (Lucy 45566).
                quietLine: _status == null && !send,
                line: CardLine(
                  _status ??
                      (send
                          ? 'Send gives $name your '
                                '${typed ? 'words' : 'files'}. Step ${at + 1} '
                                "waits for $name's answer."
                          : at + 1 < steps.length
                          ? 'The next step shows once this one is done.'
                          : '$name carries on once this is done.'),
                  color: _statusColor(palette),
                ),
                quiet: [
                  Button(
                    label: "Can't do this",
                    onPressed: _busy ? null : () => _cant(at),
                  ),
                  if (send) done,
                ],
                primary: send
                    ? Button(
                        label: 'Send',
                        primary: true,
                        onPressed: _busy ? null : _reply,
                      )
                    : done,
              );
            },
          ),
        ),
        // Send replies, so only Resolve, once there's a talk.
        if (ask.thread.isNotEmpty) ..._talk(type, reply: false),
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
    final outcome = askOutcome(ask, agent: name);
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
                      color: (allDone || talked) && stopped == null
                          ? color
                          : palette.textMuted,
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
                    text: askOutcome(ask, agent: name),
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
      if (failed && !ask.code && !isAgreement(ask)) ...[
        const SizedBox(height: Space.xs),
        Text(
          ask.actionError,
          style: type.body.copyWith(color: palette.textMuted),
        ),
      ],
      // The answer, or the comment it was dismissed with (a recovery-codes
      // card's, which secret holds them).
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
    this.step,
  });

  final List<AskThreadMessage> thread;

  /// On a procedure (#533), the step it waits at (1-based), or 0 once it
  /// doesn't: a turn about another step says which, "On step 1". Null on
  /// an ordinary card.
  final int? step;

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
                              if (step != null &&
                                  message.step > 0 &&
                                  message.step != step)
                                TextSpan(
                                  text: ' · On step ${message.step}',
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
  _ when isPayment(ask) => 'Asks you to pay on ${paymentSite(ask)}',
  _ when isCheck(ask) => 'Solve a check on ${checkSite(ask)}',
  final at? => '${ask.title} · Step ${at + 1} of ${ask.steps.length}',
  null => ask.title,
};

/// How a decided [ask] ended, in the words its card, its Needs you row
/// and History share (Lucy's §8); [agent] is its agent's name. [short]: the
/// Needs you row's, a procedure's without its count or wait ("Couldn't do
/// step 2").
/// An errand's card asking the person to accept an agreement on a site
/// (#435 W8b).
bool isAgreement(AskPerson ask) =>
    ask.hasHandoff() &&
    ask.handoff.kind == ErrandHandoffKind.ERRAND_HANDOFF_KIND_AGREEMENT;

/// An errand's card with a site's recovery codes (#435 W8b).
bool isRecoveryCodes(AskPerson ask) =>
    ask.hasHandoff() &&
    ask.handoff.kind == ErrandHandoffKind.ERRAND_HANDOFF_KIND_RECOVERY_CODES;

/// A recovery-codes card whose codes a restart would lose: Claktory's
/// vault was locked, so it holds them only in memory (Ghiaccio 43832).
bool recoveryCodesAtRisk(AskPerson ask) =>
    ask.body.startsWith("Claktory's vault is locked");

/// An errand's card asking to turn two-step sign-in on at a site (#435
/// W8b).
bool isTwoStep(AskPerson ask) =>
    ask.hasHandoff() &&
    ask.handoff.kind == ErrandHandoffKind.ERRAND_HANDOFF_KIND_TWO_STEP;

bool isPayment(AskPerson ask) =>
    ask.hasHandoff() &&
    ask.handoff.kind == ErrandHandoffKind.ERRAND_HANDOFF_KIND_PAYMENT;

/// A check that the person is a person, which the agent couldn't solve:
/// its tab opens live for them to solve it ([showCheckView]).
bool isCheck(AskPerson ask) =>
    ask.hasHandoff() &&
    ask.handoff.kind == ErrandHandoffKind.ERRAND_HANDOFF_KIND_CAPTCHA;

/// A check's site, in Claktory's words.
String checkSite(AskPerson ask) =>
    ask.handoff.domain.isNotEmpty ? ask.handoff.domain : handoffHost(ask);

String paymentSite(AskPerson ask) {
  if (ask.handoff.domain.isNotEmpty) return ask.handoff.domain;
  final host = Uri.tryParse(ask.handoff.url)?.host ?? '';
  return host.isNotEmpty ? host : handoffHost(ask);
}

Uri? paymentPage(AskPerson ask) {
  final uri = Uri.tryParse(ask.handoff.url);
  return uri != null && uri.scheme == 'https' && uri.host.isNotEmpty
      ? uri
      : null;
}

/// In the app on an iPhone or iPad (SFSafariViewController, so Apple Pay
/// works) and Android (a custom tab); the default browser elsewhere.
Future<bool> Function(Uri url) openPaymentPage = (url) => launchUrl(
  url,
  mode: switch (defaultTargetPlatform) {
    TargetPlatform.iOS || TargetPlatform.android => LaunchMode.inAppBrowserView,
    _ => LaunchMode.externalApplication,
  },
);

/// An errand's card for any other step only the person can take (#435
/// W10: a sign-in, a check that they're a person…): Done and Can't do
/// this, with the cards' own field and row. Claktory no longer asks for
/// these; older ones still show this way.
bool isGenericHandoff(AskPerson ask) =>
    ask.hasHandoff() &&
    !ask.code &&
    !isAgreement(ask) &&
    !isTwoStep(ask) &&
    !isRecoveryCodes(ask) &&
    !isPayment(ask) &&
    !isCheck(ask) &&
    ask.handoff.kind != ErrandHandoffKind.ERRAND_HANDOFF_KIND_CODE;

/// A code card's site (#435 W8): the host of the page's origin as Chrome
/// gave it (punycode, never the page's words).
String handoffHost(AskPerson ask) {
  final origin = ask.handoff.origin;
  final host = Uri.tryParse(origin)?.host ?? '';
  return host.isEmpty ? origin : host;
}

/// What a code card's field takes (#435 W8): spaces and dashes pasted with
/// a code left out, at most 16 characters (the backend refuses more).
final codeFormatters = <TextInputFormatter>[
  FilteringTextInputFormatter.deny(RegExp(r'[\s\-\u2010-\u2015\u2212]')),
  LengthLimitingTextInputFormatter(16),
];

/// The line saying a code card's page changed before Claktory could fill
/// the code in (#435 W8): the card ended, nothing filled in.
const codePageChanged = "The page changed, so it wasn't filled in.";

String askOutcome(AskPerson ask, {required String agent, bool short = false}) {
  if (ask.withdrawn) return 'Withdrawn';
  // A code card (#435 W8): never the code.
  if (ask.code) {
    if (ask.state == AskPersonState.ASK_PERSON_STATE_DISMISSED) {
      return "Couldn't fill it in";
    }
    if (ask.actionError.isNotEmpty) return codePageChanged;
    return short ? 'Filled in' : 'Filled in on ${handoffHost(ask)}';
  }
  // A site's recovery codes (#435 W8b): never the codes.
  if (isRecoveryCodes(ask)) {
    return ask.answer.isNotEmpty ? 'Kept in Secrets' : 'You saved them';
  }
  if (isAgreement(ask) &&
      ask.state != AskPersonState.ASK_PERSON_STATE_DISMISSED &&
      !ask.replied &&
      !ask.resolved) {
    return ask.actionError.isNotEmpty
        ? 'The page changed, so nothing was ticked.'
        : 'Accepted';
  }
  if (isPayment(ask) && !ask.replied && !ask.resolved) {
    return ask.state == AskPersonState.ASK_PERSON_STATE_DISMISSED
        ? "Couldn't pay"
        : short
        ? 'Paid'
        : 'Paid on ${paymentSite(ask)}';
  }
  if (isCheck(ask) && !ask.replied && !ask.resolved) {
    return ask.state == AskPersonState.ASK_PERSON_STATE_DISMISSED
        ? "Couldn't do this"
        : 'Solved';
  }
  // Any other errand step (#435 W10): done, or not.
  if (isGenericHandoff(ask) && !ask.replied && !ask.resolved) {
    return ask.state == AskPersonState.ASK_PERSON_STATE_DISMISSED
        ? "Couldn't do this"
        : 'Done';
  }
  final steps = ask.steps;
  if (steps.isNotEmpty) {
    final stopped = steps.indexWhere(
      (step) => step.state == AskStepState.ASK_STEP_STATE_CANT,
    );
    // Can't do this waits for its agent to rewrite the step or answer; it
    // gave up once it resolved the card (Cioccolata's N3). Never
    // "Dismissed" (N3).
    if (stopped >= 0) {
      final of = short ? '' : ' of ${steps.length}';
      return ask.resolved
          ? 'Stopped at step ${stopped + 1}$of'
          : short
          ? "Couldn't do step ${stopped + 1}"
          : "Couldn't do step ${stopped + 1}$of · waiting for $agent";
    }
  }
  if (agentResolved(ask)) return '$agent closed it';
  if (ask.resolved) return 'Resolved';
  if (ask.replied) {
    // Sent on a step (#533): it waits there for its agent.
    final at = steps.indexWhere(_open);
    return at < 0 || steps.isEmpty || short
        ? 'Replied'
        : 'Replied on step ${at + 1} · waiting for $agent';
  }
  final dismissed = ask.state == AskPersonState.ASK_PERSON_STATE_DISMISSED;
  if (steps.isNotEmpty) {
    if (dismissed) return 'Dismissed';
    return short
        ? 'All steps done'
        : steps.length == 2
        ? 'Both steps done'
        : 'All ${steps.length} steps done';
  }
  if (dismissed) return 'Dismissed';
  if (ask.actionError.isNotEmpty) return approvedNotDone;
  if (ask.wantsText) return 'Answered';
  if (ask.hasCommand()) return 'Ran';
  return 'Approved';
}

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
  // From focus mode's stage: the conversation, where the file opens.
  final claktory = AppScope.of(context);
  if (focusModeOf(claktory).value) setFocusMode(claktory, false);
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

/// An errand card's heading, Claktory's ("play.google.com asks you to
/// agree"), the site's host in bold where it leads.
class _HostTitle extends StatelessWidget {
  const _HostTitle({
    required this.title,
    required this.host,
    required this.style,
  });

  final String title;
  final String host;
  final TextStyle style;

  @override
  Widget build(BuildContext context) {
    final leads = host.isNotEmpty && title.startsWith(host);
    return Text.rich(
      TextSpan(
        children: [
          if (leads)
            TextSpan(
              text: host,
              style: const TextStyle(fontWeight: FontWeight.w600),
            ),
          TextSpan(
            text: proseCodes(leads ? title.substring(host.length) : title),
          ),
        ],
      ),
      style: style,
    );
  }
}

/// Words a site showed, fenced as the site's and never Claktory's (#435
/// W8b's agreement, Fugo): "The site's words" over them, in a hairline
/// box.
class SiteWords extends StatelessWidget {
  const SiteWords(this.words, {super.key});

  final String words;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Container(
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            "The site's words",
            style: type.caption.copyWith(
              color: palette.textFaint,
              fontWeight: FontWeight.w500,
            ),
          ),
          const SizedBox(height: Space.xs),
          Text(
            proseCodes(words),
            style: type.body.copyWith(color: palette.text),
          ),
        ],
      ),
    );
  }
}
