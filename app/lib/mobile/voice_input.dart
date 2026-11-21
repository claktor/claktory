import 'dart:async';

import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../recorder.dart' show RecordingEnd;
import '../voice.dart';
import 'composer.dart' show SendButton;
import 'haptics.dart';
import 'metrics.dart';
import 'navigation.dart';

/// Where a voice message stands in a phone's message box (#197, research
/// voice.md and mobile.md "Voice memo placement").
enum PhoneVoiceMode {
  /// Not recording: the mic waits in the box's primary slot.
  idle,

  /// Recording while a finger holds the mic; letting go sends.
  holding,

  /// Held and slid toward the start: letting go throws it away.
  cancelling,

  /// Recording from a tap (or VoiceOver), or after a hold was cut short:
  /// Send and Cancel are buttons.
  locked,

  /// A call or Siri stopped it: what was said waits for Send or Cancel.
  paused,
}

/// A phone's voice message, from hold to release: hold the mic to record,
/// let go to send, slide toward the start to cancel. A tap records
/// without holding, with Send and Cancel to finish, so nothing needs a
/// hold (VoiceOver and Switch Control too). What recording is and how it's
/// sent stay the message box's ([start], [send], [cancel]): this is only
/// how a finger drives them.
class PhoneVoice extends ChangeNotifier {
  PhoneVoice({required this.start, required this.send, required this.cancel});

  /// Starts recording (asking first, as the message box does); false if it
  /// couldn't.
  final Future<bool> Function() start;

  /// Sends what was recorded; throws it away.
  final Future<void> Function() send;
  final Future<void> Function() cancel;

  /// How far toward the start a held finger goes to cancel, in points.
  static const cancelDistance = 96.0;

  var _mode = PhoneVoiceMode.idle;
  PhoneVoiceMode get mode => _mode;

  /// Recording, or about to: the box shows the recording's row.
  bool get active => _mode != PhoneVoiceMode.idle;

  /// The finger left before recording began (a first use's questions came
  /// up, say).
  var _letGo = false;

  void _set(PhoneVoiceMode mode) {
    if (mode == _mode) return;
    _mode = mode;
    notifyListeners();
  }

  /// A tap on the mic: recording with Send and Cancel.
  Future<void> tap() async {
    if (active) return;
    _set(PhoneVoiceMode.locked);
    unawaited(Haptics.impact());
    if (!await start()) _set(PhoneVoiceMode.idle);
  }

  /// A finger held on the mic.
  Future<void> holdStarted() async {
    if (active) return;
    _letGo = false;
    _set(PhoneVoiceMode.holding);
    unawaited(Haptics.impact());
    final started = await start();
    if (!started) {
      _set(PhoneVoiceMode.idle);
    } else if (_letGo) {
      // Let go while it started: nothing meant to be said yet.
      _set(PhoneVoiceMode.idle);
      await cancel();
    }
  }

  /// The held finger moved [dx] points along the box from where it began.
  void holdMoved(double dx) {
    if (_mode != PhoneVoiceMode.holding && _mode != PhoneVoiceMode.cancelling) {
      return;
    }
    final next = dx <= -cancelDistance
        ? PhoneVoiceMode.cancelling
        : PhoneVoiceMode.holding;
    if (next != _mode) unawaited(Haptics.selection());
    _set(next);
  }

  /// The held finger let go: sent, or thrown away if slid to cancel.
  Future<void> holdEnded({required bool recording}) async {
    _letGo = true;
    if (!recording) return;
    switch (_mode) {
      case PhoneVoiceMode.holding:
        _set(PhoneVoiceMode.idle);
        unawaited(Haptics.light());
        await send();
      case PhoneVoiceMode.cancelling:
        _set(PhoneVoiceMode.idle);
        await cancel();
      default:
    }
  }

  /// The system took the finger (a call, a notification pulled down): not
  /// a choice to send, so it waits with Send and Cancel.
  void holdLost() {
    _letGo = true;
    if (_mode == PhoneVoiceMode.holding || _mode == PhoneVoiceMode.cancelling) {
      _set(PhoneVoiceMode.locked);
    }
  }

  /// It stopped by itself: at the time limit it's sent, as on the Mac; a
  /// call or Siri pauses it for the person to decide.
  Future<void> ended(RecordingEnd end) async {
    switch (end) {
      case RecordingEnd.limit:
        _set(PhoneVoiceMode.idle);
        await send();
      case RecordingEnd.interrupted:
        _set(PhoneVoiceMode.paused);
      case RecordingEnd.error:
        _set(PhoneVoiceMode.idle);
    }
  }

  /// Send, after a tap or a pause.
  Future<void> sendNow() async {
    _set(PhoneVoiceMode.idle);
    unawaited(Haptics.light());
    await send();
  }

  /// Cancel, after a tap or a pause.
  Future<void> cancelNow() async {
    _set(PhoneVoiceMode.idle);
    await cancel();
  }
}

/// The mic in a phone's message box, in Send's place while there's nothing
/// to send: hold to record, let go to send, slide toward the start to
/// cancel; a tap records with Send and Cancel. While held it grows red
/// under the finger, and the drawer leaves the slide alone.
class PhoneMicButton extends StatelessWidget {
  const PhoneMicButton({
    super.key,
    required this.voice,
    required this.recording,
    this.unavailable,
  });

  final PhoneVoice voice;

  /// The recording itself, to tell a release that sends from one that
  /// came before recording began.
  final VoiceRecording recording;

  /// Why voice messages can't be sent here: a tap says so.
  final String? unavailable;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: voice,
      builder: (context, _) {
        final held =
            voice.mode == PhoneVoiceMode.holding ||
            voice.mode == PhoneVoiceMode.cancelling;
        final cancelling = voice.mode == PhoneVoiceMode.cancelling;
        return Semantics(
          container: true,
          button: true,
          label: 'Record a voice message',
          onTap: voice.tap,
          excludeSemantics: true,
          child: NoDrawerGesture(
            child: Listener(
              onPointerCancel: (_) => voice.holdLost(),
              child: RawGestureDetector(
                behavior: HitTestBehavior.opaque,
                gestures: {
                  TapGestureRecognizer:
                      GestureRecognizerFactoryWithHandlers<
                        TapGestureRecognizer
                      >(
                        TapGestureRecognizer.new,
                        (tap) => tap.onTap = voice.tap,
                      ),
                  LongPressGestureRecognizer:
                      GestureRecognizerFactoryWithHandlers<
                        LongPressGestureRecognizer
                      >(
                        // Quicker than a menu's hold: it's how it's used.
                        () => LongPressGestureRecognizer(
                          duration: const Duration(milliseconds: 200),
                        ),
                        (hold) {
                          hold
                            ..onLongPressStart = (_) {
                              unawaited(voice.holdStarted());
                            }
                            ..onLongPressMoveUpdate = (details) {
                              voice.holdMoved(details.offsetFromOrigin.dx);
                            }
                            ..onLongPressEnd = (_) {
                              unawaited(
                                voice.holdEnded(recording: recording.recording),
                              );
                            };
                        },
                      ),
                },
                child: SizedBox.square(
                  dimension: Touch.target,
                  child: Center(
                    child: AnimatedContainer(
                      duration: Motion.fast,
                      width: held ? 40 : 32,
                      height: held ? 40 : 32,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: held
                            ? (cancelling ? palette.selected : palette.danger)
                            : null,
                      ),
                      alignment: Alignment.center,
                      child: ExtensionGlyph(
                        icon: 'mic',
                        name: 'Record a voice message',
                        color: held
                            ? const Color(0xFFFFFFFF)
                            : (unavailable == null
                                  ? palette.textMuted
                                  : palette.textFaint),
                        size: 22,
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

/// What a phone's message box shows while recording, in place of + and
/// the field: a live dot, the time and the sound as it's heard, and how
/// to finish. Held: "Slide to cancel", then "Let go to cancel" past the
/// point. Otherwise Cancel at its start (Send is the box's own button, at
/// its end).
class PhoneRecordingRow extends StatelessWidget {
  const PhoneRecordingRow({
    super.key,
    required this.voice,
    required this.recording,
  });

  final PhoneVoice voice;
  final VoiceRecording recording;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return ListenableBuilder(
      listenable: Listenable.merge([voice, recording]),
      builder: (context, _) {
        final mode = voice.mode;
        final held =
            mode == PhoneVoiceMode.holding || mode == PhoneVoiceMode.cancelling;
        final paused = mode == PhoneVoiceMode.paused;
        final time = voiceDuration(recording.elapsed);
        return Row(
          children: [
            if (held)
              const SizedBox(width: Space.m)
            else
              Semantics(
                container: true,
                button: true,
                label: 'Cancel',
                onTap: voice.cancelNow,
                excludeSemantics: true,
                child: Pressable(
                  onTap: voice.cancelNow,
                  builder: (context, pressed) => SizedBox(
                    height: Touch.target,
                    child: Padding(
                      padding: const EdgeInsets.symmetric(horizontal: Space.s),
                      child: Center(
                        child: Text(
                          'Cancel',
                          style: type.label.copyWith(
                            color: pressed ? palette.textMuted : palette.danger,
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            Semantics(
              container: true,
              liveRegion: paused,
              label: paused ? 'Paused, $time' : 'Recording, $time',
              excludeSemantics: true,
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  if (paused)
                    SizedBox.square(
                      dimension: 8,
                      child: DecoratedBox(
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: palette.textFaint,
                        ),
                      ),
                    )
                  else
                    LiveDot(color: palette.danger),
                  const SizedBox(width: Space.s),
                  Text(
                    time,
                    style: type.body.copyWith(
                      color: palette.text,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: Space.s),
            Expanded(
              child: held
                  ? _SlideHint(cancelling: mode == PhoneVoiceMode.cancelling)
                  : paused
                  ? Text(
                      'Paused',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: type.secondary.copyWith(color: palette.textMuted),
                    )
                  : ExcludeSemantics(
                      child: SizedBox(
                        height: 24,
                        child: CustomPaint(
                          painter: Waveform(
                            recording.levels,
                            color: palette.textMuted,
                          ),
                        ),
                      ),
                    ),
            ),
          ],
        );
      },
    );
  }
}

/// "‹ Slide to cancel" while held, "Let go to cancel" past the point.
class _SlideHint extends StatelessWidget {
  const _SlideHint({required this.cancelling});

  final bool cancelling;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Align(
      alignment: AlignmentDirectional.centerEnd,
      child: Text(
        cancelling ? 'Let go to cancel' : '‹ Slide to cancel',
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: type.secondary.copyWith(
          color: cancelling ? palette.danger : palette.textMuted,
          fontWeight: cancelling ? FontWeight.w600 : null,
        ),
      ),
    );
  }
}

/// Send, when recording from a tap or paused: the box's own Send.
class PhoneVoiceSend extends StatelessWidget {
  const PhoneVoiceSend({super.key, required this.voice});

  final PhoneVoice voice;

  @override
  Widget build(BuildContext context) => SendButton(onSend: voice.sendNow);
}
