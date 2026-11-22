// Answering by voice (Matt's: wherever he answers Claktory, he can speak
// it): a mic beside a text field records what's said, the Claktory
// transcribes it (TranscribeAudio: nothing kept, nothing sent to an agent),
// and the words go into the field where the caret is, to check and send
// as ordinary text. It never sends by itself. The speech model is asked
// for, and greyed out, as in the message box (voice.dart).
import 'dart:async';
import 'dart:io' show File, FileSystemException;
import 'dart:math' show Random, min;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'mobile/haptics.dart';
import 'mobile/metrics.dart' show Metrics, TypeScale;
import 'recorder.dart';
import 'scope.dart';
import 'voice.dart';

/// Why a recording couldn't become words, in the person's terms.
class DictationFailure implements Exception {
  const DictationFailure(this.message);

  final String message;

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

final _random = Random();

String _newId() =>
    List.generate(16, (_) => _random.nextInt(16).toRadixString(16)).join();

/// Sends [recording] to [backend]'s Claktory (an upload for no agent) and
/// asks it what was said. A Claktory from before dictation refuses the
/// upload's purpose: then it says to update.
Future<String> transcribeOnClaktory(
  Backend backend,
  Recording recording,
) async {
  final file = File(recording.path);
  final size = await file.length();
  final uploadId = _newId();
  try {
    var sent = await backend.startDictationUpload(uploadId, size: size);
    final source = await file.open();
    try {
      while (sent < size) {
        await source.setPosition(sent);
        final data = await source.read(min(_chunk, size - sent));
        if (data.isEmpty) {
          throw FileSystemException('It got shorter', recording.path);
        }
        sent = await backend.uploadChunk(uploadId, sent, data);
      }
    } finally {
      await source.close();
    }
    await backend.finishDictationUpload(uploadId);
  } on RequestFailure catch (failure) {
    unawaited(backend.cancelUpload(uploadId).catchError((_) {}));
    if (_older(failure)) throw const DictationFailure(_update);
    rethrow;
  }
  final heard = await backend.transcribeAudio(
    requestId: _newId(),
    uploadId: uploadId,
  );
  if (heard.error.isNotEmpty) {
    throw DictationFailure(dictationError(heard.error));
  }
  return heard.text;
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

/// The least room the waveform is drawn in, beside the dot and the time.
const _leastWaveform = 120.0;

/// The least width the time, Cancel and Done share a line in.
const _leastLine = 220.0;

/// Where a field's dictation stands.
enum DictationState { idle, recording, transcribing }

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
  });

  final TextEditingController controller;
  final Widget child;

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

  /// Has the keys while recording: Esc cancels, Return is Done.
  final _recordingFocus = FocusNode(debugLabel: 'Dictation');

  DictationState get state => _state;

  @override
  void initState() {
    super.initState();
    _voice.onEnded = _ended;
  }

  @override
  void dispose() {
    _voice.dispose();
    _recordingFocus.dispose();
    super.dispose();
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

  /// Done: what was said is transcribed and goes in at the caret.
  Future<void> _done() async {
    if (_state != DictationState.recording) return;
    final backend = AppScope.of(context);
    final recording = await _voice.finish();
    if (recording == null) return _set(DictationState.idle);
    final file = File(recording.path);
    if (!file.existsSync() || file.lengthSync() == 0) {
      return _set(DictationState.idle, error: 'Nothing was recorded.');
    }
    unawaited(Haptics.light());
    _set(DictationState.transcribing);
    try {
      final words = (await transcribeRecording(backend, recording)).trim();
      if (!mounted) return;
      if (words.isEmpty) {
        return _set(
          DictationState.idle,
          error: "Didn't catch any words. Try again.",
        );
      }
      _insert(words);
      _set(DictationState.idle);
    } on DictationFailure catch (failure) {
      unawaited(Haptics.warning());
      _set(DictationState.idle, error: failure.message);
    } on RequestFailure catch (failure) {
      unawaited(Haptics.warning());
      _set(DictationState.idle, error: failure.message);
    } on FileSystemException {
      unawaited(Haptics.warning());
      _set(DictationState.idle, error: dictationError('failed'));
    } finally {
      // The recording was only for this: it's not kept.
      try {
        if (file.existsSync()) file.deleteSync();
      } on FileSystemException {
        // Gone already.
      }
    }
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
        if (!voiceKnown(workspace.workspace)) {
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
                // says why), and while it transcribes.
                opacity: unavailable || _state == DictationState.transcribing
                    ? 0.4
                    : 1,
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
            if (_error case final error?) ...[
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
