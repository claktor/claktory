// Answering by voice (Matt's: wherever he answers Claktory, he can speak
// it): a mic beside a text field records what's said, the Claktory
// transcribes it (TranscribeAudio: nothing kept, nothing sent to an agent),
// and the words go into the field where the caret is, to check and send
// as ordinary text. It never sends by itself. The speech model is asked
// for, and greyed out, as in the message box (voice.dart). The recording
// is kept on the device until its words come back (#401, voice_keep.dart):
// on a weak connection it waits, "Not sent yet", and goes again once the
// connection is back, picking up what the Claktory already has; after a
// relaunch it comes back to its field.
import 'dart:async';
import 'dart:io' show File, FileSystemException;
import 'dart:math' show min;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'mobile/haptics.dart';
import 'mobile/metrics.dart' show Metrics, TypeScale;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'outbox.dart' show connectionLost;
import 'recorder.dart';
import 'scope.dart';
import 'voice.dart';
import 'voice_keep.dart';

/// Why a recording couldn't become words, in the person's terms.
class DictationFailure implements Exception {
  const DictationFailure(this.message, {this.code = ''});

  final String message;

  /// AudioTranscribed's error code, when the Claktory said why.
  final String code;

  @override
  String toString() => message;
}

/// Turns [recording] into the words said in it, on [backend]'s Claktory;
/// throws a [DictationFailure] saying why it couldn't. Tests replace it.
typedef Transcribe = Future<String> Function(
  Backend backend,
  Recording recording,
);

/// How recordings are transcribed: the Claktory's TranscribeAudio.
Transcribe transcribeRecording = transcribeOnClaktory;

/// The bytes sent at a time, as the outbox's.
const _chunk = 1024 * 1024;

/// Sends [recording] to [backend]'s Claktory (an upload for no agent) and
/// asks it what was said. A Claktory from before dictation refuses the
/// upload's purpose: then it says to update.
///
/// A [KeptVoice] goes with its own ids, so asked again after the
/// connection dropped it picks up what the Claktory has: the rest of its
/// upload, the job transcribing it, or the words it heard. When the
/// Claktory no longer has it (no_recording: a restart, or over an hour),
/// it goes up again, once, as a new upload.
Future<String> transcribeOnClaktory(
  Backend backend,
  Recording recording,
) async {
  final kept = recording is KeptVoice ? recording : null;
  for (var again = false; ; again = true) {
    final uploadId = kept?.uploadId ?? newVoiceId();
    final requestId = kept?.requestId ?? newVoiceId();
    if (kept == null || !kept.uploaded) {
      await _upload(backend, recording.path, uploadId, resumable: kept != null);
      if (kept != null) {
        kept.uploaded = true;
        await kept.save();
      }
    }
    final heard = await backend.transcribeAudio(
      requestId: requestId,
      uploadId: uploadId,
    );
    if (heard.error == 'no_recording' && kept != null && !again) {
      kept.renew();
      await kept.save();
      continue;
    }
    if (heard.error.isNotEmpty) {
      throw DictationFailure(dictationError(heard.error), code: heard.error);
    }
    return heard.text;
  }
}

/// Uploads the recording at [path] as [uploadId], from what the Claktory
/// has of it. One that isn't [resumable] is dropped there when it fails.
Future<void> _upload(
  Backend backend,
  String path,
  String uploadId, {
  required bool resumable,
}) async {
  final file = File(path);
  final size = await file.length();
  try {
    var sent = await backend.startDictationUpload(uploadId, size: size);
    final source = await file.open();
    try {
      while (sent < size) {
        await source.setPosition(sent);
        final data = await source.read(min(_chunk, size - sent));
        if (data.isEmpty) {
          throw FileSystemException('It got shorter', path);
        }
        sent = await backend.uploadChunk(uploadId, sent, data);
      }
    } finally {
      await source.close();
    }
    await backend.finishDictationUpload(uploadId);
  } on RequestFailure catch (failure) {
    final older = _older(failure);
    if (older || !resumable) {
      unawaited(backend.cancelUpload(uploadId).catchError((_) {}));
    }
    if (older) throw const DictationFailure(_update, code: 'update');
    rethrow;
  }
}

const _update =
    "This Claktory can't turn speech into text yet. Update it to answer "
    'by voice.';

/// Whether [failure] is a Claktory from before dictation refusing it: it
/// reads the upload as one for an agent, and finds none with no id.
bool _older(RequestFailure failure) =>
    failure.message.contains('no agent with id ""');

/// The words for AudioTranscribed's error codes.
String dictationError(String code) => switch (code) {
  'no_model' => "The speech model isn't ready yet. Try again in a moment.",
  'unsupported' => "The computer this Claktory runs on can't recognize speech.",
  'too_long' => 'That was too long. Try a shorter answer.',
  'no_speech' => "Didn't catch any words. Try again.",
  _ => "Couldn't turn that into text. Try again.",
};

/// The Claktory's answers that end a recording: it can't ever become
/// words, so it isn't kept. Any other failure keeps it, to try again (an
/// older Claktory's, 'update', once it's updated).
const _final = {'no_speech', 'too_long', 'unsupported'};

/// Why a dictation waits, said under its field.
const dictationWaiting =
    "Not sent yet. It's kept on this device and goes when the connection "
    'is back.';

/// A recording shorter than this with no words in it was most likely a tap
/// by mistake: it goes without a word (Matt's). A longer one says so.
const quietlyUnder = Duration(seconds: 5);

/// The least room the waveform is drawn in, beside the dot and the time.
const _leastWaveform = 120.0;

/// The least width the time, Cancel and Done share a line in.
const _leastLine = 220.0;

/// Where a field's dictation stands: [unsent], its recording kept, waiting
/// for the connection or for Retry.
enum DictationState { idle, recording, transcribing, unsent }

/// What a field's dictation is for, so a recording kept on the device
/// comes back to that field after a relaunch (#401).
class DictationFor {
  const DictationFor(this.key, this.label);

  /// The same for as long as it's the same field, across launches (the
  /// kind of card and its id, say), and never another's.
  final String key;

  /// What it was for, in the person's words ("Your answer to June"), for
  /// when the field has gone: Settings → Voice lists it then.
  final String label;
}

/// [child] (a text field, editing [controller]) with a mic at its end: tap
/// to record (the waveform and the time, Cancel and Done in the field's
/// place), then the words said go in at the caret. Only the mic is added:
/// where the Claktory can't take voice (an older one, voice off) it's the
/// field alone.
class Dictation extends StatefulWidget {
  const Dictation({
    super.key,
    required this.controller,
    required this.child,
    this.focusNode,
    this.enabled = true,
    this.trailing,
    this.keep,
  });

  final TextEditingController controller;
  final Widget child;

  /// What it's for: a recording not sent yet comes back to the field with
  /// this key after a relaunch. Without it, it's listed in Settings →
  /// Voice instead.
  final DictationFor? keep;

  /// What follows the mic on the field's line (a bar's Send): it steps
  /// aside, with the field, while recording.
  final Widget? trailing;

  /// The field's focus: back to it once the words are in.
  final FocusNode? focusNode;

  /// False while what the field is for is being sent.
  final bool enabled;

  @override
  State<Dictation> createState() => DictationFieldState();
}

@visibleForTesting
class DictationFieldState extends State<Dictation> {
  final _voice = VoiceRecording();
  var _state = DictationState.idle;
  String? _error;

  /// The recording on its way, kept until its words come.
  KeptVoice? _kept;

  /// It was under [quietlyUnder]: no words in it is a tap by mistake.
  var _short = false;

  /// [_kept] waits for the connection, not for Retry.
  var _waiting = false;

  Backend? _backend;
  KeptVoices? _voices;

  /// Has the keys while recording: Esc cancels, Return is Done.
  final _recordingFocus = FocusNode(debugLabel: 'Dictation');

  DictationState get state => _state;

  @override
  void initState() {
    super.initState();
    _voice.onEnded = _ended;
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (identical(backend, _backend)) return;
    if (_backend != null) {
      // Another Claktory now: what was on its way stays kept for the last.
      _unlisten();
      _waiting = false;
      if (_state != DictationState.recording) _state = DictationState.idle;
    }
    _backend = backend;
    final voices = _voices = KeptVoices.of(backend);
    backend.connected.addListener(_connectedChanged);
    voices.addListener(_keptChanged);
    _takeKept();
  }

  @override
  void dispose() {
    _unlisten();
    _voice.dispose();
    _recordingFocus.dispose();
    super.dispose();
  }

  void _unlisten() {
    _backend?.connected.removeListener(_connectedChanged);
    _voices?.removeListener(_keptChanged);
    // Not sent: it stays on the device, for this field when it's back, and
    // listed in Settings → Voice meanwhile.
    if (_kept case final kept?) _voices?.release(kept);
    _kept = null;
  }

  /// What's kept changed (read from the device once connected): one for
  /// this field is taken.
  void _keptChanged() {
    if (_takeKept() && mounted) setState(() {});
  }

  /// A recording kept for this field from before (the app ended before
  /// its words came, or the field was closed): back here, and on its way
  /// again. True if one was taken.
  bool _takeKept() {
    if (_state != DictationState.idle || _kept != null) return false;
    final key = widget.keep?.key;
    if (key == null) return false;
    final kept = _voices?.take(key);
    if (kept == null) return false;
    _kept = kept;
    _short = kept.duration < quietlyUnder;
    if (_backend?.connected.value ?? false) {
      // After this frame: it may be taken while building.
      scheduleMicrotask(() {
        if (mounted && _kept == kept) unawaited(_transcribe());
      });
    } else {
      _waiting = true;
      _state = DictationState.unsent;
    }
    return true;
  }

  /// The connection is back: a recording that waited for it goes.
  void _connectedChanged() {
    if (_backend?.connected.value != true) return;
    if (_state == DictationState.unsent && _waiting) unawaited(_transcribe());
  }

  void _set(DictationState state, {String? error}) {
    if (!mounted) return;
    setState(() {
      _state = state;
      _error = error;
    });
  }

  /// The mic: asks once before the Claktory downloads its speech model,
  /// then records.
  Future<void> _record() async {
    if (_state != DictationState.idle) return;
    final backend = AppScope.of(context);
    if (await deviceRecorder.access() == MicAccess.unavailable) {
      return _set(
        DictationState.idle,
        error: "You can't record on this device yet.",
      );
    }
    if (!mounted) return;
    final model = backend.workspace.workspace.speechModel;
    if (voiceUnavailable(model) case final why?) {
      return _set(DictationState.idle, error: why);
    }
    if (!await readyForVoice(context) || !mounted) return;
    _set(DictationState.recording);
    if (!await _voice.start()) {
      return _set(DictationState.idle, error: _voice.error);
    }
    if (!mounted) return;
    unawaited(Haptics.light());
    _recordingFocus.requestFocus();
  }

  Future<void> _cancel() async {
    await _voice.cancel();
    _set(DictationState.idle);
    widget.focusNode?.requestFocus();
  }

  /// Done: what was said is kept on the device, transcribed, and goes in
  /// at the caret.
  Future<void> _done() async {
    if (_state != DictationState.recording) return;
    final voices = _voices!;
    final recording = await _voice.finish();
    if (recording == null) return _set(DictationState.idle);
    final file = File(recording.path);
    final kept = KeptVoice(
      path: recording.path,
      duration: recording.duration,
      claktory: voices.claktory,
      kind: KeptKind.dictation,
      target: widget.keep?.key ?? '',
      label: widget.keep?.label ?? '',
      kept: _voice.kept,
    );
    if (!file.existsSync() || file.lengthSync() == 0) {
      unawaited(kept.delete());
      return _set(DictationState.idle, error: 'Nothing was recorded.');
    }
    unawaited(Haptics.light());
    _kept = kept;
    _short = recording.duration < quietlyUnder;
    _set(DictationState.transcribing);
    await voices.add(kept);
    await _transcribe();
  }

  /// Asks the Claktory for the words in [_kept]: they go in at the caret
  /// and the recording is deleted. Failing for want of the connection, it
  /// waits for it; failing otherwise, it's kept for Retry, unless it can
  /// never become words. [again]: tried again at once, the connection
  /// being back by the time it failed.
  Future<void> _transcribe({bool again = false}) async {
    final kept = _kept;
    final backend = _backend;
    if (kept == null || backend == null) return;
    _waiting = false;
    _set(DictationState.transcribing);
    // Its field closed meanwhile, or the person deleted it: what came is
    // for no one here (the recording stays kept until a field has it).
    bool gone() => !mounted || _kept != kept;
    try {
      final words = (await transcribeRecording(backend, kept)).trim();
      if (gone()) return;
      _drop(kept);
      if (words.isEmpty) {
        if (_short) return _nothingSaid();
        return _set(
          DictationState.idle,
          error: "Didn't catch any words. Try again.",
        );
      }
      _insert(words);
      _set(DictationState.idle);
    } on DictationFailure catch (failure) {
      if (gone()) return;
      if (failure.code.isEmpty || _final.contains(failure.code)) {
        _drop(kept);
        if (failure.code == 'no_speech' && _short) return _nothingSaid();
        unawaited(Haptics.warning());
        return _set(DictationState.idle, error: failure.message);
      }
      // Answered, so the Claktory let go of the recording: it goes up
      // again on Retry.
      kept.renew();
      unawaited(kept.save());
      unawaited(Haptics.warning());
      _set(DictationState.unsent, error: failure.message);
    } on RequestFailure catch (failure) {
      if (gone()) return;
      if (connectionLost(failure)) {
        // It may be back already (it dropped and came back meanwhile):
        // then it goes again now, once.
        if (backend.connected.value && !again) {
          return _transcribe(again: true);
        }
        _waiting = true;
        _set(DictationState.unsent);
        return;
      }
      unawaited(Haptics.warning());
      _set(DictationState.unsent, error: failure.message);
    } on FileSystemException {
      if (gone()) return;
      _drop(kept);
      unawaited(Haptics.warning());
      _set(DictationState.idle, error: dictationError('failed'));
    }
  }

  /// [kept] is done with: off the device.
  void _drop(KeptVoice kept) {
    if (_kept == kept) _kept = null;
    _waiting = false;
    unawaited(_voices?.forget(kept));
  }

  /// Delete, on a recording not sent yet, once the person says so: it's
  /// gone from the device, and what the Claktory has of it too.
  Future<void> _delete() async {
    final kept = _kept;
    if (kept == null) return;
    final sure = await showActions<bool>(
      context,
      title: 'Delete this recording?',
      message: "It hasn't been sent. This can't be undone.",
      actions: const [SheetAction('Delete', true, destructive: true)],
    );
    if (sure != true || !mounted || _kept != kept) return;
    _drop(kept);
    final backend = _backend;
    if (backend != null && backend.connected.value) {
      fireAndLog(backend.cancelUpload(kept.uploadId));
    }
    _set(DictationState.idle);
    widget.focusNode?.requestFocus();
  }

  /// A short recording with nothing said in it: the line closes and the
  /// field is as it was, as if cancelled.
  void _nothingSaid() {
    _set(DictationState.idle);
    widget.focusNode?.requestFocus();
  }

  /// Puts [words] in at the caret (or over what's selected, or at the end),
  /// with a space between it and what's on either side.
  void _insert(String words) {
    final controller = widget.controller;
    final text = controller.text;
    final selection = controller.selection;
    final start = selection.isValid ? selection.start : text.length;
    final end = selection.isValid ? selection.end : text.length;
    final before = text.substring(0, start);
    final after = text.substring(end);
    final lead = before.isEmpty || RegExp(r'\s$').hasMatch(before) ? '' : ' ';
    final trail = after.isEmpty || after.startsWith(RegExp(r'\s')) ? '' : ' ';
    final inserted = '$lead$words$trail';
    controller.value = TextEditingValue(
      text: '$before$inserted$after',
      selection: TextSelection.collapsed(
        offset: before.length + inserted.length,
      ),
    );
    widget.focusNode?.requestFocus();
  }

  /// The recording stopped by itself: at its time limit (or a call) what
  /// was said is used; if the microphone failed, it's gone.
  void _ended(RecordingEnd end) {
    if (end == RecordingEnd.error) {
      unawaited(_voice.cancel());
      _set(
        DictationState.idle,
        error: 'The recording stopped because the microphone failed.',
      );
      return;
    }
    unawaited(_done());
  }

  @override
  Widget build(BuildContext context) {
    final workspace = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: workspace,
      builder: (context, _) {
        // A recording not sent yet shows even before the Claktory says
        // it takes voice (the app just started, not connected yet).
        if (!voiceKnown(workspace.workspace) &&
            _state != DictationState.unsent) {
          final trailing = widget.trailing;
          if (trailing == null) return widget.child;
          return Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              Expanded(child: widget.child),
              const SizedBox(width: Space.s),
              trailing,
            ],
          );
        }
        final palette = Palette.of(context);
        final type = TypeScale.of(context);
        final model = workspace.workspace.speechModel;
        final unavailable = voiceUnavailable(model) != null;
        final Widget line = switch (_state) {
          DictationState.recording => _RecordingLine(
            voice: _voice,
            focus: _recordingFocus,
            onCancel: () => unawaited(_cancel()),
            onDone: () => unawaited(_done()),
          ),
          _ => Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              Expanded(child: widget.child),
              const SizedBox(width: Space.xs),
              Opacity(
                // Greyed where this Claktory can't recognize speech (a tap
                // says why), and while a recording is on its way.
                opacity: unavailable || _state != DictationState.idle ? 0.4 : 1,
                child: GlyphButton(
                  key: const ValueKey('dictate'),
                  label: 'Dictate',
                  active: false,
                  onTap: widget.enabled && _state == DictationState.idle
                      ? () => unawaited(_record())
                      : () {},
                  glyph: (color) => ExtensionGlyph(
                    icon: 'mic',
                    name: 'Dictate',
                    color: color,
                  ),
                ),
              ),
              if (widget.trailing case final trailing?) ...[
                const SizedBox(width: Space.s),
                trailing,
              ],
            ],
          ),
        };
        final note = switch (_state) {
          DictationState.transcribing => transcribingText(model),
          _ => null,
        };
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            line,
            if (note != null) ...[
              const SizedBox(height: Space.xs),
              Semantics(
                liveRegion: true,
                child: Text(
                  note,
                  style: type.caption.copyWith(color: palette.textFaint),
                ),
              ),
            ],
            if (_state == DictationState.unsent) ...[
              const SizedBox(height: Space.xs),
              _Unsent(
                why: _waiting
                    ? dictationWaiting
                    : 'Not sent yet. ${_error ?? ''}'.trim(),
                failed: !_waiting,
                onRetry: () => unawaited(_transcribe()),
                onDelete: () => unawaited(_delete()),
              ),
            ] else if (_error case final error?) ...[
              const SizedBox(height: Space.xs),
              Semantics(
                liveRegion: true,
                child: Text(
                  error,
                  style: type.caption.copyWith(color: palette.danger),
                ),
              ),
            ],
          ],
        );
      },
    );
  }
}

/// Under a field whose recording isn't sent yet: why (waiting for the
/// connection, or what went wrong), Retry and Delete.
class _Unsent extends StatelessWidget {
  const _Unsent({
    required this.why,
    required this.failed,
    required this.onRetry,
    required this.onDelete,
  });

  final String why;

  /// Something went wrong (in the danger color), rather than waiting.
  final bool failed;
  final VoidCallback onRetry;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Wrap(
      crossAxisAlignment: WrapCrossAlignment.center,
      spacing: Space.s,
      runSpacing: Space.xs,
      children: [
        Semantics(
          liveRegion: true,
          child: Text(
            why,
            style: type.caption.copyWith(
              color: failed ? palette.danger : palette.textMuted,
            ),
          ),
        ),
        Button(label: 'Retry', onPressed: onRetry),
        Button(label: 'Delete', onPressed: onDelete),
      ],
    );
  }
}

/// In the field's place while recording: a live dot, the waveform as it's
/// heard, the time, Cancel and Done. Esc cancels, Return is Done.
class _RecordingLine extends StatelessWidget {
  const _RecordingLine({
    required this.voice,
    required this.focus,
    required this.onCancel,
    required this.onDone,
  });

  final VoiceRecording voice;
  final FocusNode focus;
  final VoidCallback onCancel;
  final VoidCallback onDone;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): onCancel,
        const SingleActivator(LogicalKeyboardKey.enter): onDone,
      },
      child: Focus(
        focusNode: focus,
        child: ConstrainedBox(
          constraints: BoxConstraints(
            minHeight: touchFirst(context) ? Metrics.of(context).target : 28,
          ),
          child: ListenableBuilder(
            listenable: voice,
            builder: (context, _) {
              final status = Semantics(
                label: 'Recording, ${voiceDuration(voice.elapsed)}',
                child: ExcludeSemantics(
                  // Narrow (a panel's 320 points, Cancel and Done
                  // beside it), the waveform steps aside for the time.
                  child: LayoutBuilder(
                    builder: (context, box) => Row(
                      children: [
                        LiveDot(color: palette.danger),
                        const SizedBox(width: Space.s),
                        if (box.maxWidth >= _leastWaveform) ...[
                          Expanded(
                            child: SizedBox(
                              height: 20,
                              child: CustomPaint(
                                painter: Waveform(
                                  voice.levels,
                                  color: palette.textMuted,
                                ),
                              ),
                            ),
                          ),
                          const SizedBox(width: Space.s),
                        ],
                        Flexible(
                          child: Text(
                            voiceDuration(voice.elapsed),
                            maxLines: 1,
                            softWrap: false,
                            overflow: TextOverflow.fade,
                            style: type.caption.copyWith(
                              color: palette.textMuted,
                              fontFeatures: const [
                                FontFeature.tabularFigures(),
                              ],
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              );
              final buttons = Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Button(label: 'Cancel', onPressed: onCancel),
                  const SizedBox(width: Space.s),
                  Button(label: 'Done', primary: true, onPressed: onDone),
                ],
              );
              return LayoutBuilder(
                builder: (context, box) {
                  // Too narrow for Cancel and Done beside the time (a
                  // narrow field at large text): they go under it.
                  final scale = MediaQuery.textScalerOf(context)
                      .scale(1)
                      .clamp(1.0, 2.0);
                  if (box.maxWidth < _leastLine * scale) {
                    return Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        status,
                        const SizedBox(height: Space.xs),
                        // And under each other, when even the line
                        // can't hold both.
                        Wrap(
                          alignment: WrapAlignment.end,
                          spacing: Space.s,
                          runSpacing: Space.xs,
                          children: [
                            Button(label: 'Cancel', onPressed: onCancel),
                            Button(
                              label: 'Done',
                              primary: true,
                              onPressed: onDone,
                            ),
                          ],
                        ),
                      ],
                    );
                  }
                  return Row(
                    children: [
                      Expanded(child: status),
                      const SizedBox(width: Space.s),
                      buttons,
                    ],
                  );
                },
              );
            },
          ),
        ),
      ),
    );
  }
}
