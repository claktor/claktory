// The voice message or spoken answer being recorded now, app-wide (#409,
// Matt's: "you should be able to go literally anywhere else in the app …
// and then just click send"). A recording started in a message box or a
// field outlives it: away from it, a floating bar (VoiceBar) says what
// it's for, how long it's run and how loud, with Cancel and Send (Done, for
// an answer); back at it, the box or field shows it again in place. There's
// one microphone, so one recording at a time.
import 'dart:async';

import 'package:flutter/semantics.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'mobile/drawer.dart' show PhoneDrawerHost;
import 'mobile/metrics.dart';
import 'mobile/voice_input.dart' show PhoneVoice, PhoneVoiceMode;
import 'rail.dart' show ClaktoriesScope, PhoneClaktories;
import 'scope.dart';
import 'voice.dart';

/// A recording going on, wherever its box or field is now: [send] and
/// [cancel] work with it gone. Made by the box or field that records; it
/// goes live ([activeVoice]) as its recording starts, and stops being once
/// it stops.
abstract class VoiceSession extends ChangeNotifier {
  VoiceSession({required this.key, required this.recording}) {
    recording.addListener(_changed);
  }

  /// Whose it is: the message box's key ([MessageTarget.key]) or the
  /// field's (`dictation:` and its [DictationFor.key]); a box or field with
  /// the same key, built again, takes it back.
  final String key;

  final VoiceRecording recording;

  /// Who it's for: the agent's or chat's name, or what the answer is for
  /// ("Your answer to June: Ship it?").
  String get title;

  /// A voice message (Send sends it), or an answer said into a field (Done
  /// puts the words in).
  bool get message;

  /// On a phone, the finger's way with it (the box's); null for a field's.
  PhoneVoice? get phone => null;

  /// Goes back to where it was started (its conversation); null where
  /// there's nowhere to go.
  VoidCallback? get open => null;

  /// Sends it (or, for a field, puts its words in); true if it went.
  Future<bool> send();

  /// Throws it away.
  Future<void> cancel();

  /// It's the recording going on now.
  bool get live => activeVoice.value == this;

  /// It's no longer live: one that no box or field has now is done with
  /// (disposed).
  @protected
  void ended() {}

  void _changed() {
    if (_disposed) return;
    if (recording.recording) {
      if (!live) beginVoice(this);
      notifyListeners();
      return;
    }
    if (recording.starting || !live) return;
    activeVoice.value = null;
    notifyListeners();
    // Not while the recording tells its listeners: [ended] may dispose it.
    scheduleMicrotask(() {
      if (!_disposed && !live) ended();
    });
  }

  final _shownBy = <Object>{};

  /// A box or field shows it in place now: the floating bar steps aside.
  bool get shownInPlace => _shownBy.isNotEmpty;

  /// [by] shows it in place, or no longer ([shown] false).
  void shownIn(Object by, {required bool shown}) {
    final changed = shown ? _shownBy.add(by) : _shownBy.remove(by);
    if (changed) _later();
  }

  var _telling = false;

  /// Tells listeners once what's building now is done: boxes and fields
  /// show and stop showing it as they're built and disposed.
  void _later() {
    if (_telling) return;
    _telling = true;
    scheduleMicrotask(() {
      _telling = false;
      if (!_disposed) notifyListeners();
    });
  }

  /// It was recorded with a finger held down, still down: that kind ends
  /// with the finger, in its box.
  bool get held =>
      phone?.mode == PhoneVoiceMode.holding ||
      phone?.mode == PhoneVoiceMode.cancelling;

  /// Says [note] where the person sees it: in the floating bar's place
  /// when the box or field isn't showing it (else null: the box or field
  /// says it).
  @protected
  bool sayAway(String note) {
    if (shownInPlace) return false;
    voiceNote.value = note;
    return true;
  }

  var _disposed = false;

  /// Whether it's been disposed.
  bool get disposed => _disposed;

  /// Done with: its box or field went with it not live, or it ended with
  /// none. Once only.
  @override
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    recording.removeListener(_changed);
    if (live) activeVoice.value = null;
    recording.dispose();
    super.dispose();
  }
}

/// The recording going on now, if any; one at a time, app-wide.
final activeVoice = ValueNotifier<VoiceSession?>(null);

/// Makes [session] the recording going on now.
void beginVoice(VoiceSession session) {
  activeVoice.value = session;
}

/// Why a recording for [key] can't start now: another one is going on
/// (one microphone). Null when it can.
String? voiceBusy(String key) {
  final live = activeVoice.value;
  if (live == null || live.key == key) return null;
  return live.message
      ? "You're recording a voice message to ${live.title}. Send or cancel "
            'it first.'
      : "You're recording an answer. Finish or cancel it first.";
}

/// What the floating bar says for a moment, after what it did ("Sent to
/// Lea") or what happened to a recording away from its box; null when
/// nothing.
final voiceNote = ValueNotifier<String?>(null);

/// How long [voiceNote] shows.
const voiceNoteFor = Duration(seconds: 3);

/// A recording at least this long asks before Cancel throws it away.
const cancelAsksOver = Duration(seconds: 30);

/// What [select] goes back to from anywhere: the Claktory [context] shows,
/// its conversations rather than a page, and on a phone the conversation
/// in front of the drawer and any page over it.
VoidCallback voiceOpener(
  BuildContext context,
  Future<void> Function(Backend backend) select,
) {
  final roots = context
      .getInheritedWidgetOfExactType<ClaktoriesScope>()
      ?.notifier;
  final claktory = roots?.selected;
  final scope = AppScope.scope(context);
  return () {
    if (roots != null && claktory != null && roots.all.contains(claktory)) {
      roots.select(claktory);
    }
    scope.history.clear();
    scope.page.value = AppPage.agent;
    fireAndLog(select(scope.backend));
    PhoneClaktories.showConversation();
  };
}

/// [child] shows [session] in place (a box's recording row, a field's
/// recording line) while it's in view: built, its ticker on (not under a
/// page), and on a phone not behind the drawer.
class VoiceInPlace extends StatefulWidget {
  const VoiceInPlace({super.key, required this.session, required this.child});

  final VoiceSession session;
  final Widget child;

  @override
  State<VoiceInPlace> createState() => _VoiceInPlaceState();
}

class _VoiceInPlaceState extends State<VoiceInPlace> {
  /// On a phone, whether the conversation is in front: it changes as the
  /// drawer opens and closes (and pages come and go).
  ValueNotifier<bool>? _front;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final front = PhoneNavigation.of(context);
    if (identical(front, _front)) return;
    _front?.removeListener(_changed);
    _front = front?..addListener(_changed);
  }

  @override
  void didUpdateWidget(VoiceInPlace old) {
    super.didUpdateWidget(old);
    if (old.session != widget.session) old.session.shownIn(this, shown: false);
  }

  @override
  void dispose() {
    _front?.removeListener(_changed);
    widget.session.shownIn(this, shown: false);
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  @override
  Widget build(BuildContext context) {
    // A page over it turns its ticker off; the drawer covers what's under
    // it, pages too.
    final drawer = PhoneDrawerHost.of(context)?.drawerOpen ?? false;
    final shown = TickerMode.valuesOf(context).enabled && !drawer;
    widget.session.shownIn(this, shown: shown);
    return widget.child;
  }
}

/// The window ([child]) with the recording bar: while a recording goes on
/// away from its box or field, what it's for (a tap goes back there), the
/// time and the sound as it's heard, Cancel and Send (or Done, for an
/// answer); for a moment after, what happened ("Sent to Lea"). On a phone
/// it takes a strip at the top, the window moved down under it, as a call
/// in progress does, so it hides none of the header's buttons; elsewhere
/// it floats at the top, centered, over the window's middle.
class VoiceBarHost extends StatefulWidget {
  const VoiceBarHost({super.key, required this.child});

  final Widget child;

  @override
  State<VoiceBarHost> createState() => _VoiceBarHostState();
}

class _VoiceBarHostState extends State<VoiceBarHost> {
  VoiceSession? _session;
  PhoneVoice? _phone;
  Timer? _noteTimer;

  /// Cancel was tapped on a long recording: the bar asks first.
  var _confirming = false;

  @override
  void initState() {
    super.initState();
    activeVoice.addListener(_sessionChanged);
    voiceNote.addListener(_noteChanged);
    _sessionChanged();
  }

  @override
  void dispose() {
    activeVoice.removeListener(_sessionChanged);
    voiceNote.removeListener(_noteChanged);
    _session?.removeListener(_changed);
    _phone?.removeListener(_changed);
    _noteTimer?.cancel();
    super.dispose();
  }

  void _sessionChanged() {
    final session = activeVoice.value;
    if (identical(session, _session)) return;
    _session?.removeListener(_changed);
    _phone?.removeListener(_changed);
    _session = session?..addListener(_changed);
    _phone = session?.phone?..addListener(_changed);
    _confirming = false;
    if (mounted) setState(() {});
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  void _noteChanged() {
    _noteTimer?.cancel();
    if (voiceNote.value case final note?) {
      if (mounted) {
        SemanticsService.sendAnnouncement(
          View.of(context),
          note,
          Directionality.of(context),
        );
      }
      _noteTimer = Timer(voiceNoteFor, () {
        if (voiceNote.value == note) voiceNote.value = null;
      });
    }
    if (mounted) setState(() {});
  }

  Future<void> _send(VoiceSession session) async {
    voiceNote.value = null;
    await session.send();
  }

  void _cancel(VoiceSession session) {
    if (session.recording.elapsed >= cancelAsksOver && !_confirming) {
      setState(() => _confirming = true);
      return;
    }
    _confirming = false;
    unawaited(session.cancel());
  }

  /// What the bar shows now; null for no bar.
  Widget? _content() {
    final session = _session;
    final showing =
        session != null &&
        session.live &&
        !session.shownInPlace &&
        !session.held;
    if (!showing) {
      final note = voiceNote.value;
      return note == null ? null : _Note(note);
    }
    return _confirming
        ? _Confirm(
            session: session,
            onKeep: () => setState(() => _confirming = false),
            onDelete: () => _cancel(session),
          )
        : _Recording(
            session: session,
            onSend: () => unawaited(_send(session)),
            onCancel: () => _cancel(session),
          );
  }

  @override
  Widget build(BuildContext context) {
    final content = _content();
    final phone = isPhoneLayout(context);
    final strip = phone && content != null;
    final top = MediaQuery.paddingOf(context).top;
    return Stack(
      fit: StackFit.expand,
      children: [
        // The window stays where it is in the tree whatever the bar does,
        // so nothing in it is built again; the strip is painted after it,
        // so the drawer's blocking of what's under it leaves the bar
        // reachable by VoiceOver and TalkBack.
        CustomMultiChildLayout(
          delegate: _StripLayout(),
          children: [
            LayoutId(
              id: _StripSlot.window,
              child: MediaQuery.removePadding(
                context: context,
                removeTop: strip,
                child: widget.child,
              ),
            ),
            LayoutId(
              id: _StripSlot.strip,
              child: strip
                  ? VoiceBar(
                      child: _Strip(top: top, child: content),
                    )
                  : const SizedBox.shrink(),
            ),
          ],
        ),
        if (!phone && content != null)
          Positioned(
            top: top + Space.xs,
            left: Space.s,
            right: Space.s,
            child: Align(
              alignment: Alignment.topCenter,
              child: VoiceBar(child: _Pill(child: content)),
            ),
          ),
      ],
    );
  }
}

enum _StripSlot { window, strip }

/// The strip across the top, as tall as it needs, and the window under it.
class _StripLayout extends MultiChildLayoutDelegate {
  @override
  void performLayout(Size size) {
    final strip = layoutChild(
      _StripSlot.strip,
      BoxConstraints(maxWidth: size.width, minWidth: size.width),
    ).height;
    positionChild(_StripSlot.strip, Offset.zero);
    layoutChild(
      _StripSlot.window,
      BoxConstraints.tight(Size(size.width, size.height - strip)),
    );
    positionChild(_StripSlot.window, Offset(0, strip));
  }

  @override
  bool shouldRelayout(_StripLayout oldDelegate) => false;
}

/// The recording bar, wherever it is: on a phone a strip at the top, else
/// floating.
class VoiceBar extends StatelessWidget {
  const VoiceBar({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) => child;
}

/// The bar on a phone: across the top, under the status bar, a hairline
/// under it.
class _Strip extends StatelessWidget {
  const _Strip({required this.top, required this.child});

  /// The status bar's height, which it fills.
  final double top;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      child: Padding(
        padding: EdgeInsets.fromLTRB(
          Space.l,
          top + Space.xs,
          Space.l,
          Space.xs,
        ),
        child: child,
      ),
    );
  }
}

/// The bar's shape: floating over the window, rounded, a hairline round it.
class _Pill extends StatelessWidget {
  const _Pill({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 480),
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: palette.sidebar,
          borderRadius: BorderRadius.circular(Radii.floating),
          border: Border.all(color: palette.hairline),
          boxShadow: floatingShadow,
        ),
        child: Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: Space.m,
            vertical: Space.xs,
          ),
          child: child,
        ),
      ),
    );
  }
}

/// The least the bar is drawn in on one line: the title, the time and the
/// buttons; narrower, the buttons go under them.
const _leastLine = 330.0;

/// The least room the waveform is drawn in, beside the name and the time.
const _leastWaveform = 180.0;

double _line(BuildContext context) =>
    touchFirst(context) ? Metrics.of(context).target : 28;

/// "Sent to Lea", for a moment.
class _Note extends StatelessWidget {
  const _Note(this.note);

  final String note;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ConstrainedBox(
      constraints: BoxConstraints(minHeight: _line(context)),
      child: Align(
        widthFactor: 1,
        child: Text(
          note,
          style: TypeScale.of(context).secondary
              .copyWith(color: palette.textMuted),
        ),
      ),
    );
  }
}

/// A recording: the live dot, who it's for (a tap goes back there), the
/// time, the sound, Cancel and Send.
class _Recording extends StatelessWidget {
  const _Recording({
    required this.session,
    required this.onSend,
    required this.onCancel,
  });

  final VoiceSession session;
  final VoidCallback onSend;
  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final recording = session.recording;
    final time = voiceDuration(recording.elapsed);
    final open = session.open;
    final paused = session.phone?.mode == PhoneVoiceMode.paused;
    final what = session.message
        ? 'Voice message to ${session.title}'
        : session.title;
    final clock = Text(
      time,
      maxLines: 1,
      softWrap: false,
      style: type.caption.copyWith(
        color: palette.textMuted,
        fontFeatures: const [FontFeature.tabularFigures()],
      ),
    );
    // Narrow (a small phone, large text), the time goes down beside the
    // buttons and the waveform steps aside, so the name has the line, on
    // two if need be, rather than losing whose it is (Zucchero's row 347).
    Widget status(bool narrow) => Semantics(
      container: true,
      button: open != null,
      label: paused
          ? 'Recording paused, $what, $time'
          : 'Recording $what, $time',
      hint: open == null ? null : 'Goes back to it',
      onTap: open,
      excludeSemantics: true,
      child: Pressable(
        onTap: open,
        builder: (context, hovered) => ConstrainedBox(
          constraints: BoxConstraints(minHeight: _line(context)),
          child: LayoutBuilder(
            builder: (context, box) => Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                LiveDot(color: paused ? palette.textFaint : palette.danger),
                const SizedBox(width: Space.s),
                Flexible(
                  child: Text(
                    session.title,
                    // The largest text on a small phone takes a third.
                    maxLines: narrow ? 3 : 2,
                    overflow: TextOverflow.ellipsis,
                    style: type.label.copyWith(
                      color: palette.text,
                      decoration: hovered && open != null
                          ? TextDecoration.underline
                          : null,
                    ),
                  ),
                ),
                if (!narrow && box.maxWidth >= _leastWaveform) ...[
                  const SizedBox(width: Space.s),
                  SizedBox(
                    width: 40,
                    height: 16,
                    child: CustomPaint(
                      painter: Waveform(
                        recording.levels,
                        color: palette.textMuted,
                      ),
                    ),
                  ),
                ],
                if (!narrow) ...[const SizedBox(width: Space.s), clock],
              ],
            ),
          ),
        ),
      ),
    );
    final buttons = [
      Button(
        key: const ValueKey('voice-bar-cancel'),
        label: 'Cancel',
        onPressed: onCancel,
      ),
      Button(
        key: const ValueKey('voice-bar-send'),
        label: session.message ? 'Send' : 'Done',
        primary: true,
        onPressed: onSend,
      ),
    ];
    return _Lines(
      lead: status,
      // Read with the name already.
      under: ExcludeSemantics(child: clock),
      buttons: buttons,
    );
  }
}

/// Cancel on a long recording: Keep or Delete.
class _Confirm extends StatelessWidget {
  const _Confirm({
    required this.session,
    required this.onKeep,
    required this.onDelete,
  });

  final VoiceSession session;
  final VoidCallback onKeep;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final question =
        'Delete this ${voiceDuration(session.recording.elapsed)} recording?';
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): onKeep},
      child: _Lines(
        lead: (_) => Semantics(
          liveRegion: true,
          child: ConstrainedBox(
            constraints: BoxConstraints(minHeight: _line(context)),
            child: Align(
              alignment: AlignmentDirectional.centerStart,
              widthFactor: 1,
              child: Text(
                question,
                style: TypeScale.of(context).label
                    .copyWith(color: palette.text),
              ),
            ),
          ),
        ),
        buttons: [
          Button(
            key: const ValueKey('voice-bar-keep'),
            label: 'Keep',
            onPressed: onKeep,
          ),
          Button(
            key: const ValueKey('voice-bar-delete'),
            label: 'Delete',
            primary: true,
            onPressed: onDelete,
          ),
        ],
      ),
    );
  }
}

/// [lead] and [buttons] on a line, or [buttons] under [lead] where the
/// bar is too narrow (a small phone, large text), [under] at their start.
class _Lines extends StatelessWidget {
  const _Lines({required this.lead, required this.buttons, this.under});

  /// What it's about, told whether the buttons are under it.
  final Widget Function(bool narrow) lead;
  final List<Widget> buttons;

  /// What goes beside the buttons when they're under [lead]: the time.
  final Widget? under;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, box) {
      final scale = MediaQuery.textScalerOf(context).scale(1).clamp(1.0, 2.0);
      if (box.maxWidth < _leastLine * scale) {
        final under = this.under;
        // And under each other, when even a line can't hold both.
        final wrapped = Wrap(
          alignment: WrapAlignment.end,
          spacing: Space.s,
          runSpacing: Space.xs,
          children: buttons,
        );
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            lead(true),
            const SizedBox(height: Space.xs),
            if (under == null)
              wrapped
            else
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  ConstrainedBox(
                    constraints: BoxConstraints(minHeight: _line(context)),
                    child: Align(widthFactor: 1, child: under),
                  ),
                  const SizedBox(width: Space.s),
                  Expanded(child: wrapped),
                ],
              ),
          ],
        );
      }
      return Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Flexible(child: lead(false)),
          const SizedBox(width: Space.m),
          buttons.first,
          const SizedBox(width: Space.s),
          buttons.last,
        ],
      );
    },
  );
}
