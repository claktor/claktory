import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'attachments.dart';
import 'backend.dart';
import 'controls.dart';
import 'drafts.dart';
import 'forms.dart';
import 'identity.dart';
import 'mobile/page_find.dart' show PhonePageFind;
import 'mobile/composer.dart';
import 'mobile/device.dart' show hardwareKeyboard;
import 'mobile/device_permissions.dart' show MicReady, askMicrophone;
import 'mobile/input.dart';
import 'mobile/lines.dart';
import 'mobile/metrics.dart';
import 'mobile/platform_policy.dart';
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'mobile/voice_input.dart';
import 'model_step.dart' show showConnectModel;
import 'protocol/factory/v1/factory.pb.dart';
import 'recorder.dart' show MicAccess, Recording, RecordingEnd, deviceRecorder;
import 'scope.dart';
import 'tell_rivet.dart' show ErrorLine;
import 'voice.dart';
import 'voice_keep.dart' show KeptKind, KeptVoice, KeptVoices, VoiceNotes;
import 'voice_session.dart';

/// What a [MessageBox] writes to: an agent ([AgentMessageTarget]), or
/// another kind of conversation (a chat) that implements it. The box owns
/// the typing, the draft, the files, the voice and the slash-command menu;
/// the target says where it all goes.
abstract class MessageTarget {
  /// Tells targets apart, the same for as long as it's the same one: the
  /// key of its draft ([Drafts]) and of the files waiting in its box
  /// ([Backend.attachments]). A new key is a new box: its draft, cleared
  /// errors, a recording cancelled. Agents use their id; anything else
  /// keys its own so it never meets an agent's id.
  String get key;

  /// The box's placeholder, such as "Message June".
  String get placeholder;

  /// What VoiceOver and TalkBack call the field, empty or not: the
  /// placeholder unless that's an agent's own (several lines that take
  /// turns, say).
  String get label => placeholder;

  /// The slash commands the box offers when "/" starts the text.
  List<SlashCommand> get commands => const [];

  /// Whether the box offers files (the paperclip, a phone's +, files
  /// dropped on the window) and voice messages.
  bool get takesFiles => true;
  bool get takesVoice => true;

  /// Why nothing can be sent now, and what can be done; null when it can.
  MessageBlocked? blocked(BuildContext context) => null;

  /// Whom a voice message goes to, by name ("June"), for the floating
  /// recording bar while the box is out of sight.
  String get name => label;

  /// Goes back to this conversation from anywhere in the app, as [context]
  /// shows it now (the bar's name, tapped); null where there's no way.
  VoidCallback? reopen(BuildContext context) => null;

  /// What its voice messages are, kept on the device until sent.
  KeptKind get voiceKind => KeptKind.agent;

  /// The color Send is filled with while the box writes to this target,
  /// in place of signal, with white on it: a pipe's, so what goes out to
  /// another Claktory looks it. Null for signal.
  Color? get accent => null;

  /// Sends [text] with [files] (from the box, taken out of it); with
  /// [voice] (its length) it's a voice message, files[0] its recording,
  /// [text] and any other files going with it as one message. It should show at once and say if it fails, as an agent's
  /// outbox does: the box has already been cleared.
  void send(String text, {List<LocalFile> files = const [], Duration? voice});

  /// Runs [command] with [args]; a [RequestFailure] gives the text back to
  /// the box, with the failure under it.
  Future<void> runCommand(SlashCommand command, String args) async {}
}

/// Why a [MessageTarget] can't be written to now, said in the box, with
/// what to do about it. A phone shows only the [MessageAction.primary]
/// ones.
class MessageBlocked {
  const MessageBlocked(this.why, {this.actions = const []});

  final String why;
  final List<MessageAction> actions;
}

class MessageAction {
  const MessageAction(this.label, this.onPressed, {this.primary = false});

  final String label;
  final VoidCallback onPressed;
  final bool primary;
}

/// An agent's message box: its draft and files by its id, its outbox, its
/// slash commands; without a model, why it can't run and how to fix it.
class AgentMessageTarget extends MessageTarget {
  AgentMessageTarget(this.agent, this.backend);

  final Agent agent;
  final Backend backend;

  @override
  String get key => agent.id;

  @override
  String get placeholder =>
      agent.placeholder.isEmpty ? 'Message ${agent.name}' : agent.placeholder;

  @override
  String get label => 'Message ${agent.name}';

  @override
  String get name => agent.name;

  @override
  VoidCallback? reopen(BuildContext context) =>
      voiceOpener(context, (backend) => backend.selectAgent(agent.id));

  @override
  List<SlashCommand> get commands => agent.commands;

  @override
  MessageBlocked? blocked(BuildContext context) {
    if (agent.modelAvailable) return null;
    final hasModels = backend.workspace.workspace.models.isNotEmpty;
    if (!hasModels) {
      return MessageBlocked(
        'Connect a model so ${agent.name} can answer.',
        actions: [
          MessageAction(
            'Connect a model…',
            () => showConnectModel(context),
            primary: true,
          ),
        ],
      );
    }
    return MessageBlocked(
      "No model available, so ${agent.name} can't run.",
      actions: [
        MessageAction(
          'Change model…',
          () => showModal(
            context,
            (context, close) => AgentSettingsForm(agent: agent, close: close),
          ),
        ),
        MessageAction(
          'Sign in…',
          () => AppScope.openProviders(context),
          primary: true,
        ),
      ],
    );
  }

  @override
  void send(String text, {List<LocalFile> files = const [], Duration? voice}) =>
      backend.outbox.send(agent.id, text, files: files, voice: voice);

  @override
  Future<void> runCommand(SlashCommand command, String args) =>
      backend.runCommand(agent.id, command.name, args);
}

/// The message box: what's written to [target], an agent or a chat.
/// Multi-line; Enter sends, Shift+Enter adds a line (on a phone Return adds
/// a line and Send sends). Always enabled: messages sent while an agent
/// works wait for its run. Files, photos and voice messages go with it,
/// slash commands run, and the draft is kept, all by [target]'s key.
class MessageBox extends StatefulWidget {
  const MessageBox({
    super.key,
    required this.target,
    required this.onSent,
    this.maxFieldHeight,
    this.autofocus = true,
  });

  final MessageTarget target;

  /// Takes the keys as it comes (not on a touch screen). Focus mode's
  /// Context box doesn't: the keys stay with the card.
  final bool autofocus;

  /// Just before a message goes: the conversation follows it.
  final VoidCallback onSent;

  /// On a phone, how tall the field may grow before it scrolls.
  final double? maxFieldHeight;

  @override
  State<MessageBox> createState() => _MessageBoxState();
}

class _MessageBoxState extends State<MessageBox> {
  late final Drafts _drafts;

  /// Starts from the target's draft, which it keeps up to date.
  late final TextEditingController _text;

  @override
  void initState() {
    super.initState();
    _drafts = AppScope.scope(context).drafts;
    _text = TextEditingController.fromValue(_draftValue(widget.target.key))
      ..addListener(_typed);
    // A message started elsewhere (the sidebar's "+") lands here; the box
    // has the focus already, as it autofocuses.
    _drafts.takeFocus(widget.target.key);
    _drafts.addListener(_draftChanged);
    // Files dropped on the window come here, for this target.
    _drops = FileDrop.dropped.listen((paths) {
      if (FileDrop.owns(this)) unawaited(_attach(paths));
    });
    _stagedDrops = FileDrop.staged.listen(_attachStaged);
    _textDrops = FileDrop.texts.listen(_dropText);
    _takeVoice();
  }

  /// Whether nothing is pushed over this box (a phone layout's Settings,
  /// say): only then do files dropped on the window come here.
  bool? _onTop;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final onTop = ModalRoute.isCurrentOf(context) ?? true;
    if (onTop == _onTop) return;
    _onTop = onTop;
    if (onTop) {
      _claimDrops();
    } else {
      FileDrop.release(this);
    }
  }

  void _claimDrops() => FileDrop.claim(
    this,
    takesFiles: widget.target.takesFiles,
    key: widget.target.key,
  );

  /// The voice message recorded from this box, or one recorded for its
  /// target from a box that went meanwhile: it goes on when the box
  /// goes, the floating bar sending it.
  late _BoxVoice _session;

  /// A voice message being recorded, from the mic button.
  VoiceRecording get _voice => _session.recording;

  /// On a phone, how a finger drives [_voice]: hold, slide, let go.
  PhoneVoice get _phoneVoice => _session.phone;

  /// This box's voice: the one going on for its target, if there is one,
  /// else its own.
  void _takeVoice() {
    final live = activeVoice.value;
    _session = live is _BoxVoice && live.key == widget.target.key
        ? live
        : _BoxVoice(widget.target);
    _session.box = this;
    _voice.addListener(_voiceChanged);
    _wasVoice = (recording: _voice.recording, starting: _voice.starting);
  }

  /// The box goes, or writes to another target: a recording going on goes
  /// on without it (a finger held on the mic can't be followed, so it waits
  /// with Send and Cancel); one that isn't is done with.
  void _letVoiceGo() {
    final session = _session;
    _voice.removeListener(_voiceChanged);
    if (session.box == this) session.box = null;
    if (session.live) {
      _phoneVoice.holdLost();
    } else if (session.box == null) {
      session.dispose();
    }
  }

  /// The box's part when its voice message went: the conversation follows
  /// it, and on a Mac the field has the keys again.
  void _voiceSent() {
    widget.onSent();
    // A phone's keyboard stays down: it wasn't up for a voice message.
    if (!_phoneVoice.active && !touchFirst(context)) _focus.requestFocus();
  }

  /// Says [error] under the box.
  void _voiceError(String error, {bool plain = false}) =>
      plain ? _hint(error) : setState(() => _error = error);

  /// What was recorded waits in the box as its voice note, to go
  /// with what's written there: on a Mac the field has the keys again, to
  /// carry on writing or send it all with Return.
  void _voiceAttached() {
    if (!mounted) return;
    setState(() => _error = null);
    if (!touchFirst(context)) _focus.requestFocus();
  }

  /// The voice note waiting in this box, if any.
  KeptVoice? get _note =>
      VoiceNotes.of(AppScope.of(context)).note(widget.target.key);

  /// Something written in the box, or files: a recording made now waits
  /// in it as its voice note, to go with them ([_BoxVoice._finish]).
  bool get _hasDraft =>
      _text.text.trim().isNotEmpty ||
      (widget.target.takesFiles &&
          AppScope.of(context).attachments.of(widget.target.key).isNotEmpty);

  /// Anything to send: what's written, files, or a voice note; not while
  /// files chosen are still on their way into the box ("Adding photo…"),
  /// so a message never goes without them.
  bool get _canSend =>
      (_hasDraft || _note != null) &&
      AppScope.of(context).attachments.adding(widget.target.key).isEmpty;

  /// The phone's mic, the same one wherever it sits (Send's place, or
  /// beside Send while there's text), so a finger holding it carries on
  /// as the row changes under it.
  final _micKey = GlobalKey(debugLabel: 'mic');

  /// The phone's microphone is turned off for Claktory: its note offers
  /// Settings.
  var _micDenied = false;

  /// A phone's mic was pressed and what comes before recording (the
  /// microphone's popover, the speech model's question) is still asking:
  /// the box shows no recording yet.
  var _micAsking = false;

  /// What the mic says on a phone that can't record voice messages.
  static const noVoiceHere =
      "You can't record voice messages on this phone yet.";

  /// Starts recording from a phone's mic, as [_record] does, saying how to
  /// give the microphone back when it's off.
  Future<bool> _startPhoneVoice() async {
    if (voiceBusy(widget.target.key) case final why?) {
      _hint(why);
      return false;
    }
    // A phone with no recorder yet (Android, for now) says so at once,
    // before anything asks to download the speech model.
    if (await deviceRecorder.access() == MicAccess.unavailable) {
      if (mounted) _hint(noVoiceHere);
      return false;
    }
    if (!mounted) return false;
    final model = AppScope.of(context).workspace.workspace.speechModel;
    if (voiceUnavailable(model) case final why?) {
      setState(() => _error = why);
      return false;
    }
    // While the popover asks (and the speech model's question), the box
    // stays as it was: the recording's row only once it records.
    setState(() => _micAsking = true);
    final ready = await _micAsked() && mounted && await readyForVoice(context);
    if (!mounted) return false;
    setState(() => _micAsking = false);
    if (!ready) return false;
    setState(() {
      _error = null;
      _micDenied = false;
    });
    _session.from(context);
    if (await _voice.start()) return true;
    final access = await deviceRecorder.access();
    if (!mounted) return false;
    setState(() {
      // Open Settings only when Settings can change it: not while it can
      // still be asked (Android after a first no), nor when restrictions
      // decide.
      _micDenied = access == MicAccess.denied;
      _error = _voice.error;
    });
    if (_micDenied) _micBack ??= AppLifecycleListener(onResume: _micResumed);
    return false;
  }

  /// Watches for the app coming back while the microphone is off: from
  /// Settings with it on, the line and its Open Settings go by themselves
  /// (permissions spec, rule 3).
  AppLifecycleListener? _micBack;

  Future<void> _micResumed() async {
    if (!_micDenied) return;
    final access = await deviceRecorder.access();
    if (!mounted || !_micDenied || access != MicAccess.granted) return;
    setState(() {
      _micDenied = false;
      _error = null;
    });
    _micBack?.dispose();
    _micBack = null;
  }

  /// Has the keys while recording: Esc cancels, Return sends.
  final _recordingFocus = FocusNode();
  var _wasVoice = (recording: false, starting: false);

  /// Rebuilds when the microphone is asked for, and when recording starts
  /// or stops: from starting to recording too, which puts Send in the strip;
  /// the strip follows the sound itself.
  void _voiceChanged() {
    final now = (recording: _voice.recording, starting: _voice.starting);
    if (now == _wasVoice) return;
    _wasVoice = now;
    if (mounted) setState(() {});
  }

  /// The Mac's and iPad's mic, where the microphone's popover points.
  final _deskMicKey = GlobalKey(debugLabel: 'desk mic');

  /// The first time, before the system asks for the microphone, its
  /// popover over the mic says why: whether
  /// to go on now. Not when it was set aside, nor right after it was
  /// allowed in it: the next tap or hold records, as the finger that asked
  /// has long lifted. Refused or restricted, recording goes on to fail and
  /// say so in its own line, with Open Settings where that can help.
  Future<bool> _micAsked() async {
    final anchor =
        (isPhoneLayout(context) ? _micKey : _deskMicKey).currentContext ??
        context;
    return switch (await askMicrophone(anchor)) {
      MicReady.setAside || MicReady.allowedNow => false,
      _ => true,
    };
  }

  /// The mic: asks once before the Claktory downloads its speech model
  /// (starting it, so the first message waits only for what's left), then
  /// records.
  Future<void> _record() async {
    if (voiceBusy(widget.target.key) case final why?) {
      _hint(why);
      return;
    }
    final model = AppScope.of(context).workspace.workspace.speechModel;
    if (voiceUnavailable(model) case final why?) {
      setState(() => _error = why);
      return;
    }
    if (!await _micAsked() || !mounted) return;
    if (!await readyForVoice(context) || !mounted) return;
    setState(() => _error = null);
    _session.from(context);
    if (!await _voice.start()) {
      if (mounted) setState(() => _error = _voice.error);
      return;
    }
    _recordingFocus.requestFocus();
  }

  /// Sends what was recorded as a voice message ([_BoxVoice.send]).
  Future<void> _sendVoice() => _session.send();

  Future<void> _cancelVoice() => _session.cancel();

  /// Cancelled: on a Mac the field has the keys again.
  void _voiceCancelled() {
    if (!touchFirst(context)) _focus.requestFocus();
  }

  KeyEventResult _onRecordingKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) return KeyEventResult.ignored;
    if (event.logicalKey == LogicalKeyboardKey.escape) {
      unawaited(_cancelVoice());
      return KeyEventResult.handled;
    }
    if (event.logicalKey == LogicalKeyboardKey.enter) {
      unawaited(_sendVoice());
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  StreamSubscription<List<String>>? _drops;
  StreamSubscription<StagedDrop>? _stagedDrops;
  StreamSubscription<String>? _textDrops;

  /// Text dropped on an iPad's window: typed in where the cursor
  /// is, or at the end, as a paste would be.
  void _dropText(String text) {
    if (!FileDrop.owns(this)) return;
    final value = _text.value;
    final selection = value.selection.isValid
        ? value.selection
        : TextSelection.collapsed(offset: value.text.length);
    _text.value = value
        .replaced(selection, text)
        .copyWith(
          selection: TextSelection.collapsed(
            offset: selection.start + text.length,
          ),
        );
    _focus.requestFocus();
  }

  /// An iPad's drop: "Adding photo…" at once, as the picker's
  /// photos do, then the files once they're copied for the app.
  Future<void> _attachStaged(StagedDrop drop) async {
    if (!FileDrop.owns(this) ||
        !widget.target.takesFiles ||
        widget.target.blocked(context) != null) {
      return;
    }
    final drafts = AppScope.of(context).attachments;
    final key = widget.target.key;
    drafts.startAdding(key, drop.kinds);
    final List<String> paths;
    try {
      paths = await drop.paths;
      if (mounted) {
        await _attach(paths);
      } else {
        await drafts.add(key, paths);
      }
    } finally {
      drafts.doneAdding(key, drop.kinds);
    }
    final missing = notAdded(drop.kinds, drop.kinds.length - paths.length);
    if (missing != null && mounted) setState(() => _error = missing);
  }

  /// Puts the files at [paths] in this box, saying why any couldn't go.
  Future<void> _attach(List<String> paths) async {
    if (paths.isEmpty ||
        !widget.target.takesFiles ||
        widget.target.blocked(context) != null) {
      return;
    }
    final left = await AppScope.of(context).attachments
        .add(widget.target.key, paths);
    if (!mounted) return;
    setState(
      () => _error = left == 0
          ? null
          : left == paths.length && left == 1
          ? "Can't attach this: it's a folder or can't be read."
          : "Some items weren't attached: folders, and files that can't "
                'be read.',
    );
    _focus.requestFocus();
  }

  /// The paperclip, where an iPad's picker menu points.
  final _attachButton = GlobalKey(debugLabel: 'Attach files');

  /// The paperclip, or a phone's +: the system's picker for files (and
  /// photos, on a phone). Every phone asks its own through the same
  /// contract (`factory/device`'s pickAttachments), never a desktop's; a
  /// picker that can't open says so rather than doing nothing.
  Future<void> _pick() async {
    // A Mac's window.
    if (!PhonePlatformPolicy.current.touch) return _attach(await pickFiles());
    // An iPhone, an iPad or Android. What was chosen shows in the box as
    // soon as the picker closes, "Adding photo…" while it's copied for the
    // app (a photo took seconds to show).
    final drafts = AppScope.of(context).attachments;
    final key = widget.target.key;
    var chosen = const <String>[];
    final List<String> paths;
    try {
      // Take Photo first in its menu, where there's a camera.
      paths = await pickOnDevice(
        context,
        from: pickerAnchor(_attachButton.currentContext),
        camera: true,
        onChosen: widget.target.takesFiles
            ? (kinds) => drafts.startAdding(key, chosen = kinds)
            : null,
      );
    } on MissingPluginException {
      drafts.doneAdding(key, chosen);
      // This app has no picker of its own yet (Android, for now).
      if (mounted) setState(() => _error = pickerMissing);
      return;
    } on Exception {
      drafts.doneAdding(key, chosen);
      if (mounted) setState(() => _error = pickerFailed);
      return;
    }
    try {
      if (mounted) {
        await _attach(paths);
      } else {
        // The box went meanwhile: they wait in its agent's box all the same.
        await drafts.add(key, paths);
      }
    } finally {
      drafts.doneAdding(key, chosen);
    }
    final missing = notAdded(chosen, chosen.length - paths.length);
    if (missing != null && mounted) setState(() => _error = missing);
  }

  /// What the box says when a phone's picker failed to open.
  static const pickerFailed = "Couldn't open your photos or files. Try again.";

  /// What the box says when this phone's app has no picker at all.
  static const pickerMissing = "You can't attach files from this phone yet.";

  /// The draft changed from outside the box: a message started for this
  /// target elsewhere.
  void _draftChanged() {
    final key = widget.target.key;
    if (_drafts.of(key) != _text.text) _text.value = _draftValue(key);
    if (_drafts.takeFocus(key)) {
      _text.selection = TextSelection.collapsed(offset: _text.text.length);
      _focus.requestFocus();
    }
  }

  late final _focus = FocusNode(onKeyEvent: _onKey);

  /// What's said under the box: why something failed, with Ask Rivet to
  /// fix, or what to do ([_hint]), without.
  String? _errorText;
  var _errorPlain = false;
  String? get _error => _errorText;
  set _error(String? text) {
    _errorText = text;
    _errorPlain = false;
  }

  /// Says [text] under the box: something for the person to do (send or
  /// cancel the recording going on, say), nothing gone wrong, so no Ask
  /// Rivet to fix.
  void _hint(String text) => setState(() {
    _errorText = text;
    _errorPlain = true;
  });

  /// The slash-command menu, over the top of the box.
  final _menu = OverlayPortalController();
  final _box = LayerLink();

  /// The menu's highlighted command, among [_matches].
  var _highlighted = 0;

  /// Esc closed the menu; it stays closed until the text changes.
  var _dismissed = false;
  var _lastText = '';

  /// The commands matching what's typed: text starting with "/" and no
  /// space yet, each command whose name starts with what follows it.
  List<SlashCommand> get _matches {
    final text = _text.text;
    if (_dismissed || !text.startsWith('/') || text.contains(RegExp(r'\s'))) {
      return const [];
    }
    final typed = text.substring(1).toLowerCase();
    return [
      for (final command in widget.target.commands)
        if (command.name.toLowerCase().startsWith(typed)) command,
    ];
  }

  /// The target's draft, with the cursor at its end.
  TextEditingValue _draftValue(String key) {
    final draft = _drafts.of(key);
    return TextEditingValue(
      text: draft,
      selection: TextSelection.collapsed(offset: draft.length),
    );
  }

  void _typed() {
    _drafts.set(widget.target.key, _text.text);
    if (_text.text == _lastText) return;
    _lastText = _text.text;
    _dismissed = false;
    _highlighted = 0;
    _syncMenu();
  }

  void _syncMenu() {
    final show = _matches.isNotEmpty;
    if (show && !_menu.isShowing) _menu.show();
    if (!show && _menu.isShowing) _menu.hide();
    if (mounted) setState(() {});
  }

  /// The command typed out in full, "/name" or "/name args", or null.
  (SlashCommand, String)? _typedCommand() {
    final match = RegExp(r'^/(\S+)(?:\s+([\s\S]*))?$')
        .firstMatch(_text.text.trim());
    if (match == null) return null;
    for (final command in widget.target.commands) {
      if (command.name == match.group(1)) {
        return (command, (match.group(2) ?? '').trim());
      }
    }
    return null;
  }

  Future<void> _run(SlashCommand command, String args) async {
    final key = widget.target.key;
    final typed = _text.text;
    _text.clear();
    setState(() => _error = null);
    try {
      await widget.target.runCommand(command, args);
    } on RequestFailure catch (failure) {
      if (!mounted) return _giveBack(key, typed);
      // Give it back, so it can be fixed and tried again.
      if (_text.text.isEmpty && widget.target.key == key) _text.text = typed;
      setState(() => _error = '/${command.name}: ${failure.message}');
    }
  }

  @override
  void didUpdateWidget(MessageBox old) {
    super.didUpdateWidget(old);
    if ((old.target.key != widget.target.key ||
            old.target.takesFiles != widget.target.takesFiles) &&
        (_onTop ?? true)) {
      _claimDrops();
    }
    if (old.target.key == widget.target.key) _session.target = widget.target;
    if (old.target.key != widget.target.key) {
      // A recording is for the target it was started for: it goes on,
      // the floating bar sending it there.
      _letVoiceGo();
      _takeVoice();
      _text.value = _draftValue(widget.target.key);
      _error = null;
      // A touch screen's keyboard stays down: it would cover the
      // conversation just chosen.
      if (!touchFirst(context)) _focus.requestFocus();
    }
  }

  @override
  void dispose() {
    _micBack?.dispose();
    unawaited(_drops?.cancel());
    unawaited(_stagedDrops?.cancel());
    unawaited(_textDrops?.cancel());
    FileDrop.release(this);
    _drafts.removeListener(_draftChanged);
    _text.dispose();
    _focus.dispose();
    _letVoiceGo();
    _recordingFocus.dispose();
    super.dispose();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    // Enter while an input method is composing confirms the composition.
    final composing = _text.value.composing;
    if (composing.isValid && !composing.isCollapsed) {
      return KeyEventResult.ignored;
    }
    final matches = _matches;
    if (matches.isNotEmpty) {
      final count = matches.length;
      if (key == LogicalKeyboardKey.arrowDown) {
        setState(() => _highlighted = (_highlighted + 1) % count);
        return KeyEventResult.handled;
      }
      if (key == LogicalKeyboardKey.arrowUp) {
        setState(() => _highlighted = (_highlighted - 1 + count) % count);
        return KeyEventResult.handled;
      }
      if (key == LogicalKeyboardKey.escape) {
        _dismissed = true;
        _syncMenu();
        return KeyEventResult.handled;
      }
      if (key == LogicalKeyboardKey.tab) {
        // Complete the name, to type what follows it.
        final name = matches[_highlighted.clamp(0, count - 1)].name;
        _text.value = TextEditingValue(
          text: '/$name ',
          selection: TextSelection.collapsed(offset: name.length + 2),
        );
        return KeyEventResult.handled;
      }
    }
    if (key != LogicalKeyboardKey.enter ||
        HardwareKeyboard.instance.isShiftPressed) {
      return KeyEventResult.ignored;
    }
    // On a phone Return starts a new line, as the keyboard's own does; ⌘
    // or Ctrl with it sends from a hardware keyboard. An iPad with a
    // hardware keyboard sends with Return at every width, a narrow Split
    // View too, as Messages does.
    if (isPhoneLayout(context) &&
        !hardwareKeyboard.value &&
        !HardwareKeyboard.instance.isMetaPressed &&
        !HardwareKeyboard.instance.isControlPressed &&
        _matches.isEmpty) {
      return KeyEventResult.ignored;
    }
    if (matches.isNotEmpty) {
      _run(matches[_highlighted.clamp(0, matches.length - 1)], '');
    } else if (_typedCommand() case (final command, final args)) {
      _run(command, args);
    } else {
      _send();
    }
    return KeyEventResult.handled;
  }

  /// The menu of matching commands, just above the box and as wide.
  Widget _buildMenu(BuildContext context, double width) {
    final palette = Palette.of(context);
    final matches = _matches;
    return CompositedTransformFollower(
      link: _box,
      targetAnchor: Alignment.topLeft,
      followerAnchor: Alignment.bottomLeft,
      offset: const Offset(0, -Space.xs),
      child: Align(
        alignment: Alignment.bottomLeft,
        child: Container(
          width: width,
          constraints: const BoxConstraints(maxHeight: 280),
          padding: const EdgeInsets.all(Space.xs),
          decoration: BoxDecoration(
            color: palette.sidebar,
            borderRadius: BorderRadius.circular(Radii.panel),
            border: Border.all(color: palette.hairline),
            boxShadow: floatingShadow,
          ),
          child: ListView(
            shrinkWrap: true,
            children: [
              for (var i = 0; i < matches.length; i++)
                MouseRegion(
                  cursor: SystemMouseCursors.click,
                  onEnter: (_) => setState(() => _highlighted = i),
                  child: GestureDetector(
                    onTap: () => _run(matches[i], ''),
                    child: Container(
                      padding: const EdgeInsets.symmetric(
                        horizontal: Space.s,
                        vertical: Space.xs + 2,
                      ),
                      decoration: BoxDecoration(
                        color: i == _highlighted
                            ? palette.selected
                            : const Color(0x00000000),
                        borderRadius: BorderRadius.circular(Radii.control),
                      ),
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            '/${matches[i].name}',
                            style: TypeScale.of(context).label
                                .copyWith(color: palette.text),
                          ),
                          if (matches[i].argsHint.isNotEmpty) ...[
                            const SizedBox(width: Space.xs),
                            Text(
                              matches[i].argsHint,
                              style: TypeScale.of(context).body
                                  .copyWith(color: palette.textFaint),
                            ),
                          ],
                          const SizedBox(width: Space.m),
                          Expanded(
                            child: Text(
                              matches[i].description,
                              style: TypeScale.of(context).body
                                  .copyWith(color: palette.textMuted),
                            ),
                          ),
                        ],
                      ),
                    ),
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }

  /// A message or command that failed after its box was gone: back to its
  /// draft, unless something new was typed there since.
  void _giveBack(String key, String text) {
    if (_drafts.of(key).isEmpty) _drafts.set(key, text);
  }

  Future<void> _send() async {
    // A recording going on is sent from its own Send; files still on
    // their way into the box hold it until they're in.
    if (_voice.recording || _voice.starting || !_canSend) return;
    final text = _text.text.trim();
    final key = widget.target.key;
    final backend = AppScope.of(context);
    final notes = VoiceNotes.of(backend);
    // Files, or a voice note, can go on their own.
    if (text.isEmpty &&
        backend.attachments.of(key).isEmpty &&
        notes.note(key) == null) {
      return;
    }
    _text.clear();
    setState(() => _error = null);
    widget.onSent();
    final files = backend.attachments.take(key);
    // It shows at once, its files uploading; one that isn't sent stays
    // there, with Retry. A voice note goes with the rest as one message,
    // its recording first.
    final note = notes.take(key);
    if (note == null) {
      widget.target.send(text, files: files);
      return;
    }
    final recording = File(note.path);
    widget.target.send(
      text,
      files: [
        LocalFile(
          path: note.path,
          name: note.path.split('/').last,
          size: recording.existsSync() ? recording.lengthSync() : 0,
        ),
        ...files,
      ],
      voice: note.duration,
    );
  }

  /// The message box on a phone: + for files and photos, the field
  /// growing from one line to five and then scrolling, Send at its end
  /// (mobile/composer.dart). Return starts a new line. When nothing can be
  /// sent (an agent without a model), why and what to do take the field's
  /// place: the target's main action.
  Widget _buildPhone(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final target = widget.target;
    final backend = AppScope.of(context);
    final blocked = target.blocked(context);
    final Widget row;
    if (blocked != null) {
      final lead = Text(
        blocked.why,
        style: type.secondary.copyWith(color: palette.textMuted),
      );
      final actions = [
        for (final action in blocked.actions)
          if (action.primary)
            Button(
              label: action.label,
              primary: true,
              onPressed: action.onPressed,
            ),
      ];
      row = actions.isEmpty
          ? lead
          : ActionLine(
              lead: lead,
              action: Wrap(
                spacing: Space.s,
                runSpacing: Space.s,
                children: actions,
              ),
            );
    } else {
      final workspace = backend.workspace.workspace;
      final voice = target.takesVoice && voiceKnown(workspace);
      row = ListenableBuilder(
        listenable: Listenable.merge([
          _text,
          backend.attachments,
          _phoneVoice,
          VoiceNotes.of(backend),
        ]),
        builder: (context, _) {
          // With something written, what's recorded is added to it as its
          // voice note, not sent on its own.
          final adding = _hasDraft;
          // Pressed, but not recording until what asks first is answered.
          final active = _phoneVoice.active && !_micAsking;
          final mode = _micAsking ? PhoneVoiceMode.idle : _phoneVoice.mode;
          final mic = KeyedSubtree(
            key: _micKey,
            child: PhoneMicButton(
              key: const ValueKey('mic'),
              voice: _phoneVoice,
              recording: _voice,
              unavailable: voiceUnavailable(workspace.speechModel),
              label: adding ? 'Add a voice note' : 'Record a voice message',
            ),
          );
          return PhoneComposerRow(
            accent: target.accent,
            onAttach: target.takesFiles ? _pick : null,
            canSend: !active && _canSend,
            onSend: _send,
            maxFieldHeight: widget.maxFieldHeight,
            // The mic while there's nothing to send; Send (or Add, to the
            // text) for a recording that isn't held.
            primary: !voice
                ? null
                : switch (mode) {
                    PhoneVoiceMode.locked || PhoneVoiceMode.paused
                        when adding =>
                      _AddVoiceNote(voice: _phoneVoice, accent: target.accent),
                    PhoneVoiceMode.locked || PhoneVoiceMode.paused =>
                      PhoneVoiceSend(voice: _phoneVoice, accent: target.accent),
                    // Held from beside Send: it stays under the finger.
                    PhoneVoiceMode.holding ||
                    PhoneVoiceMode.cancelling when adding => Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        mic,
                        const SizedBox(width: Space.xs),
                        SizedBox.square(
                          dimension: PhonePlatformPolicy.of(context)
                              .touchTarget,
                        ),
                      ],
                    ),
                    _ => mic,
                  },
            // Beside Send while there's something written and no voice note
            // yet: one to add to it.
            beside: voice && adding && _note == null && !active ? mic : null,
            recording: active
                ? PhoneRecordingRow(voice: _phoneVoice, recording: _voice)
                : null,
            field: TextField(
              controller: _text,
              focusNode: _focus,
              kind: InputKind.message,
              placeholder: target.placeholder,
              semanticsLabel: target.label,
              // The keyboard would cover the conversation at once.
              autofocus: false,
              minLines: 1,
              maxLines: 5,
              bordered: false,
            ),
          );
        },
      );
    }
    return Padding(
      padding: const EdgeInsets.fromLTRB(
        Space.xs,
        Space.xs,
        Space.xs,
        Space.xs,
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (_error != null)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: Space.s),
              child: _micDenied
                  ? Row(
                      children: [
                        Expanded(
                          // Something to do: no Ask Rivet to fix.
                          child: _Note(
                            _error!,
                            color: palette.danger,
                            plain: true,
                          ),
                        ),
                        const SizedBox(width: Space.s),
                        Button(
                          label: 'Open Settings',
                          onPressed: () => deviceRecorder.openSettings(),
                        ),
                      ],
                    )
                  : _Note(_error!, color: palette.danger, plain: _errorPlain),
            ),
          Padding(
            padding: EdgeInsets.only(
              left: PhonePlatformPolicy.of(context).touchTarget + Space.xs,
            ),
            child: _Attachments(boxKey: target.key),
          ),
          LayoutBuilder(
            builder: (context, constraints) => CompositedTransformTarget(
              link: _box,
              child: OverlayPortal(
                controller: _menu,
                overlayChildBuilder: (context) =>
                    _buildMenu(context, constraints.maxWidth),
                child: row,
              ),
            ),
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) => VoiceInPlace(
    // While it's in view, the box shows its recording itself: the floating
    // bar steps aside.
    session: _session,
    // A voice note put in or taken out: the mic and Send follow.
    child: ListenableBuilder(
      listenable: VoiceNotes.of(AppScope.of(context)),
      builder: (context, _) => _build(context),
    ),
  );

  Widget _build(BuildContext context) {
    if (isPhoneLayout(context)) {
      // A phone's find bar takes the box's place at the foot while it's
      // open (mobile/page_find.dart): the box steps aside, its draft kept.
      final finding = PhonePageFind.finding(context);
      return Offstage(
        offstage: finding,
        child: TickerMode(
          enabled: !finding,
          child: ExcludeFocus(excluding: finding, child: _buildPhone(context)),
        ),
      );
    }
    final palette = Palette.of(context);
    final target = widget.target;
    final blocked = target.blocked(context);
    // A phone has no Enter key to send with, nor room to spare.
    final phone = isPhoneLayout(context);
    final workspace = AppScope.of(context).workspace.workspace;
    // Voice messages, on a Mac, where the Claktory knows them (the
    // iPhone's are held to record, Mobile's).
    final voice =
        !phone && blocked == null && target.takesVoice && voiceKnown(workspace);
    final recording = voice && (_voice.recording || _voice.starting);
    // The row's height: a Mac's 28; on a touch screen (an iPad) its
    // buttons are a finger's size, so it's at least that, and grows with
    // their text rather than cutting it.
    final touch = touchFirst(context);
    final row = touch ? Metrics.of(context).target : _composerRow;
    return Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: contentMaxWidth),
        child: Padding(
          padding: EdgeInsets.fromLTRB(
            phone ? Space.s : Space.xl,
            Space.s,
            phone ? Space.s : Space.xl,
            // On an iPad the conversation runs under the home indicator
            // (ipad/workbench.dart): the box stays above it. 0 on a Mac.
            (phone ? Space.s : Space.l) + MediaQuery.paddingOf(context).bottom,
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (_error != null)
                _Note(_error!, color: palette.danger, plain: _errorPlain),
              LayoutBuilder(
                builder: (context, constraints) => CompositedTransformTarget(
                  link: _box,
                  child: OverlayPortal(
                    controller: _menu,
                    overlayChildBuilder: (context) =>
                        _buildMenu(context, constraints.maxWidth),
                    child: ValueListenableBuilder(
                      valueListenable: FileDrop.over,
                      builder: (context, filesOver, child) => AnimatedContainer(
                        duration: Motion.fast,
                        // Hugs its content: one row, as tall as one line
                        // (or a button) when empty, growing with the text.
                        padding: const EdgeInsets.fromLTRB(
                          Space.m,
                          Space.xs,
                          Space.xs,
                          Space.xs,
                        ),
                        decoration: BoxDecoration(
                          color: palette.sidebar,
                          borderRadius: BorderRadius.circular(Radii.panel),
                          // Files dragged over the window land here.
                          border: Border.all(
                            color: filesOver && blocked == null
                                ? palette.signal
                                : palette.hairline,
                          ),
                        ),
                        child: child,
                      ),
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          _Attachments(boxKey: target.key),
                          ConstrainedBox(
                            constraints: BoxConstraints(minHeight: row),
                            child: Row(
                              crossAxisAlignment: CrossAxisAlignment.end,
                              children: [
                                Expanded(
                                  child: recording
                                      ? Container(
                                          constraints: BoxConstraints(
                                            minHeight: row,
                                          ),
                                          alignment: Alignment.centerLeft,
                                          child: Focus(
                                            focusNode: _recordingFocus,
                                            onKeyEvent: _onRecordingKey,
                                            child: RecordingStrip(
                                              voice: _voice,
                                              onCancel: _cancelVoice,
                                              // Send in the strip, so the
                                              // waveform centers on the
                                              // whole row.
                                              // The box's padding is
                                              // more at the start.
                                              shift: -(Space.m - Space.xs) / 2,
                                              trailing: _voice.recording
                                                  ? Button(
                                                      // With something
                                                      // written, it's added
                                                      // to that.
                                                      label: _hasDraft
                                                          ? 'Add'
                                                          : 'Send',
                                                      primary: true,
                                                      onPressed: _sendVoice,
                                                    )
                                                  : null,
                                            ),
                                          ),
                                        )
                                      : blocked == null
                                      ? Container(
                                          constraints: BoxConstraints(
                                            minHeight: row,
                                          ),
                                          alignment: Alignment.centerLeft,
                                          padding: const EdgeInsets.symmetric(
                                            vertical: 4.5,
                                          ),
                                          child: TextField(
                                            controller: _text,
                                            focusNode: _focus,
                                            placeholder: target.placeholder,
                                            semanticsLabel: target.label,
                                            // On a touch screen (a phone, an
                                            // iPad) the keyboard would cover
                                            // the conversation at once.
                                            autofocus:
                                                widget.autofocus &&
                                                !touchFirst(context),
                                            minLines: 1,
                                            maxLines: 12,
                                            bordered: false,
                                          ),
                                        )
                                      // Sending couldn't work, so don't offer to.
                                      : Container(
                                          constraints: BoxConstraints(
                                            minHeight: row,
                                          ),
                                          alignment: Alignment.centerLeft,
                                          child: Text(
                                            blocked.why,
                                            maxLines: 1,
                                            overflow: TextOverflow.ellipsis,
                                            style: TypeScale.of(context).body
                                                .copyWith(
                                                  color: palette.textMuted,
                                                ),
                                          ),
                                        ),
                                ),
                                // At the end of the row, level with the last
                                // line: what it's doing, and what can be done.
                                ConstrainedBox(
                                  constraints: touch
                                      ? BoxConstraints(minHeight: row)
                                      : const BoxConstraints.tightFor(
                                          height: _composerRow,
                                        ),
                                  child: Row(
                                    mainAxisSize: MainAxisSize.min,
                                    children: [
                                      if (blocked == null &&
                                          target.takesFiles &&
                                          !recording) ...[
                                        const SizedBox(width: Space.xs),
                                        GlyphButton(
                                          key: _attachButton,
                                          label: 'Attach files',
                                          active: false,
                                          onTap: _pick,
                                          glyph: (color) => ExtensionGlyph(
                                            icon: 'paperclip',
                                            name: 'Attach files',
                                            color: color,
                                          ),
                                        ),
                                      ],
                                      // One voice note to a message: no mic
                                      // while the box has one.
                                      // Recording, Send is in the strip.
                                      if (voice &&
                                          !_voice.recording &&
                                          (recording || _note == null)) ...[
                                        const SizedBox(width: Space.xs),
                                        KeyedSubtree(
                                          key: _deskMicKey,
                                          child: _MicButton(
                                            unavailable: voiceUnavailable(
                                              workspace.speechModel,
                                            ),
                                            starting: _voice.starting,
                                            onTap: _record,
                                          ),
                                        ),
                                      ],
                                      // A phone has no Enter key; an
                                      // iPad has one only with a hardware
                                      // keyboard (the on-screen Return
                                      // adds a line), so Send is there
                                      // while it's not.
                                      if (touch &&
                                          blocked == null &&
                                          !recording)
                                        ListenableBuilder(
                                          listenable: Listenable.merge([
                                            _text,
                                            AppScope.of(context).attachments,
                                            hardwareKeyboard,
                                          ]),
                                          builder: (context, _) =>
                                              !phone && hardwareKeyboard.value
                                              ? const SizedBox.shrink()
                                              : Padding(
                                                  padding:
                                                      const EdgeInsets.only(
                                                        left: Space.s,
                                                      ),
                                                  child: Button(
                                                    label: 'Send',
                                                    primary: true,
                                                    onPressed: _canSend
                                                        ? _send
                                                        : null,
                                                  ),
                                                ),
                                        ),
                                      if (blocked != null)
                                        for (final action
                                            in blocked.actions) ...[
                                          SizedBox(
                                            width: action.primary
                                                ? Space.xs
                                                : Space.s,
                                          ),
                                          Button(
                                            label: action.label,
                                            primary: action.primary,
                                            onPressed: action.onPressed,
                                          ),
                                        ],
                                    ],
                                  ),
                                ),
                              ],
                            ),
                          ),
                        ],
                      ),
                    ),
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

/// A voice message recorded from a [MessageBox] for [target]: it
/// goes on when the box goes (another conversation, Settings, a page over
/// it), the floating bar ([VoiceBar]) saying so, and [send] and [cancel]
/// work without it. A box for the same target, built again, shows it.
class _BoxVoice extends VoiceSession {
  _BoxVoice(this.target) : super(key: target.key, recording: VoiceRecording()) {
    recording.onEnded = _recordingEnded;
  }

  /// Where it goes: the latest box's, while one shows it.
  MessageTarget target;

  /// The box that has it now, if any.
  _MessageBoxState? box;

  VoidCallback? _open;

  @override
  String get title => target.name;

  @override
  bool get message => true;

  @override
  VoidCallback? get open => _open;

  @override
  late final PhoneVoice phone = PhoneVoice(
    start: _start,
    send: _sendPhone,
    cancel: _drop,
  );

  /// The box's text and files ([Drafts], [Backend.attachments]) and its
  /// Claktory, as they are wherever the box is.
  Drafts? _drafts;
  Backend? _backend;

  /// It's being started from [context]'s box: the bar's name goes back
  /// there.
  void from(BuildContext context) {
    _open = target.reopen(context);
    _drafts = AppScope.scope(context).drafts;
    _backend = AppScope.of(context);
  }

  /// The box has something written (or files) to go with it: in place, it
  /// becomes the box's voice note rather than going alone.
  bool get _boxHasDraft {
    final drafts = _drafts;
    final backend = _backend;
    return (drafts?.of(key).trim().isNotEmpty ?? false) ||
        (target.takesFiles &&
            (backend?.attachments.of(key).isNotEmpty ?? false));
  }

  _MessageBoxState? get _here {
    final box = this.box;
    return box != null && box.mounted ? box : null;
  }

  Future<bool> _start() async => await _here?._startPhoneVoice() ?? false;

  /// Says [error] in the box, or in the bar's place when the box isn't
  /// showing it.
  void _say(String error, {bool plain = false}) {
    if (!sayAway(error)) _here?._voiceError(error, plain: plain);
  }

  /// A phone's recording, sent; one too short to hold words (a tap that
  /// lingered) is dropped, saying how it's done.
  Future<void> _sendPhone() async {
    if (recording.elapsed < const Duration(milliseconds: 600)) {
      await recording.cancel();
      _say('Hold to record, let go to send.', plain: true);
      return;
    }
    await _finish();
  }

  var _went = false;

  @override
  Future<bool> send() async {
    _went = false;
    if (phone.active) {
      await phone.sendNow();
    } else {
      await _finish();
    }
    return _went;
  }

  @override
  Future<void> cancel() async {
    if (phone.active) return phone.cancelNow();
    return _drop();
  }

  Future<void> _drop() async {
    await recording.cancel();
    _here?._voiceCancelled();
  }

  /// The recording is done: in its box with something written there, it
  /// waits in the box as its voice note, to go with the rest on Send;
  /// otherwise it goes now.
  Future<void> _finish() =>
      shownInPlace && _boxHasDraft ? _attach() : _deliver();

  /// Stops recording; what was recorded, or null (saying so) if nothing
  /// was.
  Future<({Recording made, int size})?> _stop() async {
    final made = await recording.finish();
    if (made == null) return null;
    final file = File(made.path);
    final size = file.existsSync() ? file.lengthSync() : 0;
    if (size == 0) {
      _say('Nothing was recorded.', plain: true);
      return null;
    }
    return (made: made, size: size);
  }

  /// Puts what was recorded in the box as its voice note, kept on
  /// the device meanwhile.
  Future<void> _attach() async {
    final stopped = await _stop();
    final backend = _backend;
    if (stopped == null || backend == null) return;
    await VoiceNotes.of(backend).put(
      key,
      KeptVoice(
        path: stopped.made.path,
        duration: stopped.made.duration,
        claktory: KeptVoices.of(backend).claktory,
        kind: target.voiceKind,
        target: key,
        kept: recording.kept,
      ),
    );
    _went = true;
    _here?._voiceAttached();
  }

  /// Sends what was recorded as a voice message, for the Claktory to
  /// transcribe, through [target]'s outbox, which keeps it until the
  /// Claktory has it: it shows in the conversation at once, as any
  /// message. What's written in its box (and its files) goes with it, as
  /// one message: from the bar, away from the box, that's how it
  /// takes the text along. Sent from the bar, the bar says so.
  Future<void> _deliver() {
    final backend = _backend;
    return backend == null
        ? _deliverRecording()
        : backend.holding(_deliverRecording);
  }

  Future<void> _deliverRecording() async {
    final stopped = await _stop();
    if (stopped == null) return;
    final made = stopped.made;
    final drafts = _drafts;
    final backend = _backend;
    final text = drafts?.of(key).trim() ?? '';
    final files = target.takesFiles && backend != null
        ? backend.attachments.take(key)
        : const <LocalFile>[];
    if (text.isNotEmpty) drafts?.set(key, '');
    _here?._voiceSent();
    target.send(
      text,
      files: [
        LocalFile(
          path: made.path,
          name: made.path.split('/').last,
          size: stopped.size,
        ),
        ...files,
      ],
      voice: made.duration,
    );
    _went = true;
    sayAway(
      text.isNotEmpty || files.isNotEmpty
          ? 'Sent to $title with your message'
          : 'Sent to $title',
    );
  }

  /// The recording stopped by itself: at its time limit (or a call, on a
  /// Mac) it's sent as it is; on a phone a call pauses it, for Send or
  /// Cancel; if the microphone failed, it's gone.
  void _recordingEnded(RecordingEnd end) {
    final onPhone = phone.active;
    if (onPhone) unawaited(phone.ended(end));
    if (end == RecordingEnd.error) {
      unawaited(recording.cancel());
      _say('The recording stopped because the microphone failed.');
      return;
    }
    if (onPhone) return;
    unawaited(_finish());
  }

  @override
  void ended() {
    if (box == null) dispose();
  }

  @override
  void dispose() {
    if (disposed) return;
    phone.dispose();
    super.dispose();
  }
}

/// On a phone, in Send's place while recording from a tap (or paused) with
/// something written in the box: the recording is added to it as its
/// voice note, to go with it on Send. Send's disc, with a check.
class _AddVoiceNote extends StatelessWidget {
  const _AddVoiceNote({required this.voice, this.accent});

  final PhoneVoice voice;
  final Color? accent;

  static const label = 'Add voice note';

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final fill = accent ?? palette.signal;
    return Semantics(
      container: true,
      button: true,
      label: label,
      onTap: voice.sendNow,
      excludeSemantics: true,
      child: Pressable(
        onTap: voice.sendNow,
        builder: (context, pressed) => SizedBox.square(
          key: const ValueKey('add-voice-note'),
          dimension: PhonePlatformPolicy.of(context).touchTarget,
          child: Center(
            child: AnimatedContainer(
              duration: Motion.fast,
              width: 32,
              height: 32,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: pressed
                    ? Color.alphaBlend(const Color(0x33000000), fill)
                    : fill,
              ),
              alignment: Alignment.center,
              child: ExtensionGlyph(
                icon: 'check',
                name: label,
                // As on Send: black on signal, white on an accent.
                color: accent != null
                    ? const Color(0xFFFFFFFF)
                    : const Color(0xFF000000),
                size: 18,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The mic at the end of the message box: click to record a voice message.
/// Greyed out where the Claktory's computer can't transcribe one, saying
/// why when clicked.
class _MicButton extends StatelessWidget {
  const _MicButton({
    required this.unavailable,
    required this.starting,
    required this.onTap,
  });

  final String? unavailable;
  final bool starting;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final button = GlyphButton(
      label: 'Record a voice message',
      active: starting,
      onTap: onTap,
      glyph: (color) => ExtensionGlyph(
        icon: 'mic',
        name: 'Record a voice message',
        color: unavailable == null ? color : palette.textFaint,
      ),
    );
    final why = unavailable;
    if (why == null) return button;
    return HoverCard(end: true, card: (context) => Text(why), child: button);
  }
}

/// The files in a message box, and its voice note, as chips over
/// its text, each with × to take it out.
class _Attachments extends StatelessWidget {
  const _Attachments({required this.boxKey});

  /// The box's [MessageTarget.key].
  final String boxKey;

  /// Takes the voice note out of the box, asking first when it's long, as
  /// Cancel does while recording: it can't be made again.
  Future<void> _removeNote(BuildContext context, KeptVoice note) async {
    final notes = VoiceNotes.of(AppScope.of(context));
    if (note.duration >= cancelAsksOver) {
      final sure = await showActions<bool>(
        context,
        title: 'Delete this voice note?',
        message: "It hasn't been sent. This can't be undone.",
        actions: const [SheetAction('Delete', true, destructive: true)],
      );
      if (sure != true) return;
    }
    await notes.discard(boxKey);
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final attachments = backend.attachments;
    final notes = VoiceNotes.of(backend);
    return ListenableBuilder(
      listenable: Listenable.merge([attachments, notes]),
      builder: (context, _) {
        final files = attachments.of(boxKey);
        final adding = attachments.adding(boxKey);
        final note = notes.note(boxKey);
        if (files.isEmpty && adding.isEmpty && note == null) {
          return const SizedBox.shrink();
        }
        // Decoded while they wait, so they're in the bubble from the tap.
        for (final file in files) {
          if (file.isImage) warmSentPicture(context, file.path);
        }
        return Padding(
          padding: const EdgeInsets.only(top: 2, right: Space.xs, bottom: 2),
          child: Wrap(
            spacing: Space.xs,
            runSpacing: Space.xs,
            children: [
              if (note != null)
                VoiceNoteChip(
                  key: const ValueKey('voice-note'),
                  duration: note.duration,
                  onRemove: () => unawaited(_removeNote(context, note)),
                ),
              for (final file in files)
                AttachmentChip(
                  name: file.name,
                  size: file.size,
                  onRemove: () => attachments.remove(boxKey, file),
                ),
              for (final kind in adding) AddingChip(kind: kind),
            ],
          ),
        );
      },
    );
  }
}

/// The height of the message box's row: a button's, which also fits one
/// line of text with room around it.
const _composerRow = 28.0;

class _Note extends StatelessWidget {
  const _Note(this.text, {required this.color, this.plain = false});

  final String text;
  final Color color;

  /// Something to do rather than a failure: no Ask Rivet to fix.
  final bool plain;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.s, left: Space.xs),
      child: plain
          ? Text(
              text,
              style: TypeScale.of(context).caption.copyWith(color: color),
            )
          : ErrorLine(
              text,
              style: TypeScale.of(context).caption.copyWith(color: color),
            ),
    );
  }
}
