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
import 'mobile/input.dart';
import 'mobile/lines.dart';
import 'mobile/metrics.dart';
import 'mobile/platform_policy.dart';
import 'mobile/voice_input.dart';
import 'model_step.dart' show showConnectModel;
import 'protocol/factory/v1/factory.pb.dart';
import 'recorder.dart' show MicAccess, RecordingEnd, deviceRecorder;
import 'scope.dart';
import 'tell_rivet.dart' show ErrorLine;
import 'voice.dart';

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

  /// The color Send is filled with while the box writes to this target,
  /// in place of signal, with white on it: a pipe's, so what goes out to
  /// another Claktory looks it. Null for signal.
  Color? get accent => null;

  /// Sends [text] with [files] (from the box, taken out of it), or a
  /// voice message ([voice], its length; [text] empty, the recording in
  /// [files]). It should show at once and say if it fails, as an agent's
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
  });

  final MessageTarget target;

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
    _drops = FileDrop.dropped.listen(_attach);
    FileDrop.claim(this, takesFiles: widget.target.takesFiles);
    _voice
      ..onEnded = _recordingEnded
      ..addListener(_voiceChanged);
  }

  /// A voice message being recorded, from the mic button.
  final _voice = VoiceRecording();

  /// On a phone, how a finger drives [_voice]: hold, slide, let go.
  late final _phoneVoice = PhoneVoice(
    start: _startPhoneVoice,
    send: _sendPhoneVoice,
    cancel: _cancelVoice,
  );

  /// Sends a phone's recording; one too short to hold words (a tap that
  /// lingered) is dropped, saying how it's done.
  Future<void> _sendPhoneVoice() async {
    if (_voice.elapsed < const Duration(milliseconds: 600)) {
      await _voice.cancel();
      if (mounted) setState(() => _error = 'Hold to record, let go to send.');
      return;
    }
    await _sendVoice();
  }

  /// The phone's microphone is turned off for Claktory: its note offers
  /// Settings.
  var _micDenied = false;

  /// What the mic says on a phone that can't record voice messages.
  static const noVoiceHere =
      "You can't record voice messages on this phone yet.";

  /// Starts recording from a phone's mic, as [_record] does, saying how to
  /// give the microphone back when it's off.
  Future<bool> _startPhoneVoice() async {
    // A phone with no recorder yet (Android, for now) says so at once,
    // before anything asks to download the speech model.
    if (await deviceRecorder.access() == MicAccess.unavailable) {
      if (mounted) setState(() => _error = noVoiceHere);
      return false;
    }
    if (!mounted) return false;
    final model = AppScope.of(context).workspace.workspace.speechModel;
    if (voiceUnavailable(model) case final why?) {
      setState(() => _error = why);
      return false;
    }
    if (!await readyForVoice(context) || !mounted) return false;
    setState(() {
      _error = null;
      _micDenied = false;
    });
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
    return false;
  }

  /// Has the keys while recording: Esc cancels, Return sends.
  final _recordingFocus = FocusNode();
  var _wasRecording = false;

  /// Rebuilds when recording starts or stops; the strip follows the
  /// sound itself.
  void _voiceChanged() {
    final now = _voice.recording || _voice.starting;
    if (now == _wasRecording) return;
    _wasRecording = now;
    if (mounted) setState(() {});
  }

  /// The mic: asks once before the Claktory downloads its speech model
  /// (starting it, so the first message waits only for what's left), then
  /// records.
  Future<void> _record() async {
    final model = AppScope.of(context).workspace.workspace.speechModel;
    if (voiceUnavailable(model) case final why?) {
      setState(() => _error = why);
      return;
    }
    if (!await readyForVoice(context) || !mounted) return;
    setState(() => _error = null);
    if (!await _voice.start()) {
      if (mounted) setState(() => _error = _voice.error);
      return;
    }
    _recordingFocus.requestFocus();
  }

  /// Sends what was recorded as a voice message, for the Claktory to
  /// transcribe; it shows in the conversation at once, as any message.
  Future<void> _sendVoice() async {
    final target = widget.target;
    final recording = await _voice.finish();
    if (recording == null || !mounted) return;
    final file = File(recording.path);
    final size = file.existsSync() ? file.lengthSync() : 0;
    if (size == 0) {
      setState(() => _error = 'Nothing was recorded.');
      return;
    }
    widget.onSent();
    target.send(
      '',
      files: [
        LocalFile(
          path: recording.path,
          name: recording.path.split('/').last,
          size: size,
        ),
      ],
      voice: recording.duration,
    );
    // A phone's keyboard stays down: it wasn't up for a voice message.
    if (!_phoneVoice.active && !touchFirst(context)) _focus.requestFocus();
  }

  Future<void> _cancelVoice() async {
    await _voice.cancel();
    if (mounted && !touchFirst(context)) _focus.requestFocus();
  }

  /// The recording stopped by itself: at its time limit (or a call, on an
  /// iPhone) it's sent as it is; if the microphone failed, it's gone.
  void _recordingEnded(RecordingEnd end) {
    final phone = _phoneVoice.active;
    if (phone) unawaited(_phoneVoice.ended(end));
    if (end == RecordingEnd.error) {
      unawaited(_voice.cancel());
      setState(
        () => _error = 'The recording stopped because the microphone failed.',
      );
      return;
    }
    // A phone's own decides: a call pauses it rather than sending.
    if (phone) return;
    unawaited(_sendVoice());
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

  /// The paperclip, or a phone's +: the system's picker for files (and
  /// photos, on a phone). Every phone asks its own through the same
  /// contract (`factory/device`'s pickAttachments), never a desktop's; a
  /// picker that can't open says so rather than doing nothing.
  Future<void> _pick() async {
    // A Mac's window.
    if (!PhonePlatformPolicy.current.touch) return _attach(await pickFiles());
    // An iPhone, an iPad or Android.
    final List<String> paths;
    try {
      paths = await _pickOnPhone();
    } on MissingPluginException {
      // This app has no picker of its own yet (Android, for now).
      if (mounted) setState(() => _error = pickerMissing);
      return;
    } on Exception {
      if (mounted) setState(() => _error = pickerFailed);
      return;
    }
    await _attach(paths);
  }

  /// What the box says when a phone's picker failed to open.
  static const pickerFailed = "Couldn't open your photos or files. Try again.";

  /// What the box says when this phone's app has no picker at all.
  static const pickerMissing = "You can't attach files from this phone yet.";

  /// The phone's picker (attachments.dart's [pickAttachments], with its
  /// failures kept): the chosen files' paths, empty when cancelled.
  static Future<List<String>> _pickOnPhone() async {
    final paths = await const MethodChannel('factory/device')
        .invokeListMethod<Object?>('pickAttachments');
    return [
      for (final path in paths ?? const <Object?>[])
        if (path is String) path,
    ];
  }

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
  String? _error;

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
    if (old.target.key != widget.target.key ||
        old.target.takesFiles != widget.target.takesFiles) {
      FileDrop.claim(this, takesFiles: widget.target.takesFiles);
    }
    if (old.target.key != widget.target.key) {
      // A recording is for the target it was started for.
      unawaited(_voice.cancel());
      _text.value = _draftValue(widget.target.key);
      _error = null;
      // A touch screen's keyboard stays down: it would cover the
      // conversation just chosen.
      if (!touchFirst(context)) _focus.requestFocus();
    }
  }

  @override
  void dispose() {
    unawaited(_drops?.cancel());
    FileDrop.release(this);
    _drafts.removeListener(_draftChanged);
    _text.dispose();
    _focus.dispose();
    _voice.dispose();
    _recordingFocus.dispose();
    _phoneVoice.dispose();
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
    // or Ctrl with it sends from a hardware keyboard.
    if (isPhoneLayout(context) &&
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
    final text = _text.text.trim();
    final key = widget.target.key;
    final backend = AppScope.of(context);
    // Files can go on their own.
    if (text.isEmpty && backend.attachments.of(key).isEmpty) return;
    _text.clear();
    setState(() => _error = null);
    widget.onSent();
    // It shows at once, its files uploading; one that isn't sent stays
    // there, with Retry.
    widget.target.send(text, files: backend.attachments.take(key));
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
        listenable: Listenable.merge([_text, backend.attachments, _phoneVoice]),
        builder: (context, _) => PhoneComposerRow(
          accent: target.accent,
          onAttach: target.takesFiles ? _pick : null,
          canSend:
              !_phoneVoice.active &&
              (_text.text.trim().isNotEmpty ||
                  backend.attachments.of(target.key).isNotEmpty),
          onSend: _send,
          maxFieldHeight: widget.maxFieldHeight,
          // The mic while there's nothing to send; Send for a recording
          // that isn't held.
          primary: !voice
              ? null
              : switch (_phoneVoice.mode) {
                  PhoneVoiceMode.locked || PhoneVoiceMode.paused =>
                    PhoneVoiceSend(voice: _phoneVoice, accent: target.accent),
                  _ => PhoneMicButton(
                    key: const ValueKey('mic'),
                    voice: _phoneVoice,
                    recording: _voice,
                    unavailable: voiceUnavailable(workspace.speechModel),
                  ),
                },
          recording: _phoneVoice.active
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
        ),
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
                        Expanded(child: _Note(_error!, color: palette.danger)),
                        const SizedBox(width: Space.s),
                        Button(
                          label: 'Open Settings',
                          onPressed: () => deviceRecorder.openSettings(),
                        ),
                      ],
                    )
                  : _Note(_error!, color: palette.danger),
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
  Widget build(BuildContext context) {
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
              if (_error != null) _Note(_error!, color: palette.danger),
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
                                            autofocus: !touchFirst(context),
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
                                      if (voice) ...[
                                        const SizedBox(width: Space.xs),
                                        _voice.recording
                                            ? Button(
                                                label: 'Send',
                                                primary: true,
                                                onPressed: _sendVoice,
                                              )
                                            : _MicButton(
                                                unavailable: voiceUnavailable(
                                                  workspace.speechModel,
                                                ),
                                                starting: _voice.starting,
                                                onTap: _record,
                                              ),
                                      ],
                                      // A phone has no Enter key; an
                                      // iPad has one only with a hardware
                                      // keyboard (the on-screen Return
                                      // adds a line), so Send is there
                                      // while it's not.
                                      if (touch && blocked == null)
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
                                                    onPressed:
                                                        _text.text
                                                                .trim()
                                                                .isEmpty &&
                                                            AppScope.of(context)
                                                                .attachments
                                                                .of(target.key)
                                                                .isEmpty
                                                        ? null
                                                        : _send,
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

/// The files in a message box, as chips over its text, each with × to
/// take it out.
class _Attachments extends StatelessWidget {
  const _Attachments({required this.boxKey});

  /// The box's [MessageTarget.key].
  final String boxKey;

  @override
  Widget build(BuildContext context) {
    final attachments = AppScope.of(context).attachments;
    return ListenableBuilder(
      listenable: attachments,
      builder: (context, _) {
        final files = attachments.of(boxKey);
        if (files.isEmpty) return const SizedBox.shrink();
        return Padding(
          padding: const EdgeInsets.only(top: 2, right: Space.xs, bottom: 2),
          child: Wrap(
            spacing: Space.xs,
            runSpacing: Space.xs,
            children: [
              for (final file in files)
                AttachmentChip(
                  name: file.name,
                  size: file.size,
                  onRemove: () => attachments.remove(boxKey, file),
                ),
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
  const _Note(this.text, {required this.color});

  final String text;
  final Color color;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.s, left: Space.xs),
      child: ErrorLine(
        text,
        style: TypeScale.of(context).caption.copyWith(color: color),
      ),
    );
  }
}
