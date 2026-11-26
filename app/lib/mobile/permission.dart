// The permission sheet (area 5a, ux/permissions.md): one way to ask for
// every permission the system guards, on every device. Before the system's
// prompt, a short sheet says why, in Claktory's words, with one way on,
// Continue, which brings the prompt (Apple's rule for the screen before a
// system prompt). Allowed, what the person was doing carries on. Declined,
// it says what stops, what still works and where to turn it on, with Open
// Settings and a quiet way out that's never a dead end; back from Settings
// with it on, it closes by itself. Restricted (Screen Time, a work profile,
// an administrator), Settings can't help, so it says so and offers only
// the way out.
//
// A [Permission] says what the system lets the app read and do for one
// permission, and its words; a [PermissionFlow] walks its stages; a
// [PermissionContent] shows them, in a phone's sheet or an iPad's or Mac's
// dialog ([askPermission]), a popover ([askMicrophone], mic.dart), or a
// screen of its own (the scanner's first time).
import 'dart:async';
import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../devices.dart' show thisDeviceKind;
import '../forms.dart' show FittedHeading;
import '../identity.dart';
import 'device.dart' as device;
import 'haptics.dart';
import 'metrics.dart' show Touch, TypeScale;
import 'presentation.dart' show presentSheet;
import 'scroll_fade.dart';

/// What the system says of a permission, where it lets the app read it.
enum PermissionAccess {
  granted,

  /// Not asked yet on this device: the Before stage. Android also says so
  /// after a first no, when it can still ask once more.
  undetermined,

  /// Refused: only Settings can change it.
  denied,

  /// Off, and Settings can't change it here (Screen Time, a work profile,
  /// an administrator).
  restricted,

  /// Nothing to ask for: no camera, no microphone, no native side.
  unavailable,

  /// The system can't say without asking (Local Network): the Before stage
  /// until a check answers. After asking, nothing to say for sure, so it's
  /// taken as on, and the feature finds out.
  unknown,
}

/// The words of one permission's sheet. [body] ends with what happens next
/// ("Your iPhone asks next."), and every one names the device as itself
/// ([thisDeviceKind]).
@immutable
class PermissionWords {
  const PermissionWords({
    required this.glyph,
    required this.title,
    required this.body,
    required this.offTitle,
    required this.offBody,
    this.offCaption,
    this.restrictedBody,
    this.stillWorks,
    this.openSettings = 'Open Settings',
    this.drawGlyph,
  });

  /// Its glyph's name ([ExtensionGlyph]): 'network', 'bell', 'mic'.
  final String glyph;

  /// Its glyph, drawn here, for one the app's set doesn't have (the
  /// camera's); null: [glyph].
  final Widget Function(Color color, double size)? drawGlyph;

  /// Before: what it lets Claktory do.
  final String title;
  final String body;

  /// Declined: "… is off", what stops and what still works.
  final String offTitle;
  final String offBody;

  /// Declined: where to turn it on ("To turn it on: Settings → …"), in a
  /// caption under [offBody].
  final String? offCaption;

  /// Restricted: that Settings can't change it on this device, after
  /// [stillWorks]. Null: [restrictedFallback].
  final String? restrictedBody;

  /// Restricted: what still works without it, first, so a restricted
  /// sheet is never only a no ("You'll see what needs you only when you
  /// open Claktory.").
  final String? stillWorks;

  /// The way to Settings: "Open System Settings" on a Mac.
  final String openSettings;
}

/// Restricted: why Settings can't help (Trish, 43277).
String restrictedFallback() =>
    'Screen Time or whoever manages this $thisDeviceKind turned it off, so '
    "it can't be changed here.";

/// One permission the system guards: what the sheet needs of it.
abstract class Permission {
  const Permission();

  /// Its words, on this device.
  PermissionWords words(BuildContext context);

  /// What the system says now, without asking: read each time it's needed
  /// (the person may have changed it in Settings). [PermissionAccess.unknown]
  /// where it can't be read without asking.
  Future<PermissionAccess> access();

  /// Brings the system's prompt (when it can still ask) and answers what
  /// it said. The flow reads [access] again after any answer but granted,
  /// so a request may answer [PermissionAccess.denied] for "no" however it
  /// came (Android's instant no after two refusals).
  Future<PermissionAccess> request();

  /// The app's page in the system's Settings (a Mac's exact pane where it
  /// has one), where the person turns it on.
  Future<void> openSettings();
}

/// Where a permission's sheet is.
enum PermissionStage {
  /// Why Claktory asks, with Continue: held, nothing sets it aside on a
  /// phone.
  before,

  /// The system's prompt is up: Continue is inactive.
  asking,

  /// Refused: Open Settings, and the quiet way out.
  declined,

  /// Off for good here: the way out only.
  restricted,

  /// Allowed, or set aside: the sheet goes.
  done,
}

/// A permission's stages, from where its [Permission.access] puts it to
/// done. [result] is what it ended on: granted, or what it was when set
/// aside (undetermined before Continue).
class PermissionFlow extends ChangeNotifier {
  PermissionFlow(
    this.permission, {
    PermissionAccess from = PermissionAccess.undetermined,
  }) : _stage = stageFor(from) ?? PermissionStage.done,
       _result = from;

  final Permission permission;

  /// The stage [access] starts at; null where there's nothing to show
  /// (allowed already, or nothing to ask for).
  static PermissionStage? stageFor(PermissionAccess access) => switch (access) {
    PermissionAccess.undetermined ||
    PermissionAccess.unknown => PermissionStage.before,
    PermissionAccess.denied => PermissionStage.declined,
    PermissionAccess.restricted => PermissionStage.restricted,
    PermissionAccess.granted || PermissionAccess.unavailable => null,
  };

  PermissionStage _stage;
  PermissionStage get stage => _stage;

  PermissionAccess _result;
  PermissionAccess get result => _result;

  final _done = Completer<PermissionAccess>();

  /// Completes with [result] once it's done.
  Future<PermissionAccess> get done => _done.future;

  /// Why it asks, or the system asking: nothing sets it aside on a phone.
  bool get held =>
      _stage == PermissionStage.before || _stage == PermissionStage.asking;

  var _disposed = false;

  void _go(PermissionStage stage, PermissionAccess result) {
    if (_disposed) return;
    _result = result;
    if (_stage == stage) return;
    _stage = stage;
    if (stage == PermissionStage.done && !_done.isCompleted) {
      _done.complete(result);
    }
    notifyListeners();
  }

  /// A check is running: one at a time (iOS answers each once).
  var _checking = false;

  /// Continue: the system's prompt, then where its answer puts it.
  Future<void> proceed() async {
    if (_checking || _stage != PermissionStage.before) return;
    _checking = true;
    _go(PermissionStage.asking, _result);
    var answer = await permission.request();
    if (answer != PermissionAccess.granted) {
      final now = await permission.access();
      if (now != PermissionAccess.unknown) answer = now;
    }
    _checking = false;
    switch (answer) {
      case PermissionAccess.granted || PermissionAccess.unknown:
        unawaited(Haptics.success());
        _go(PermissionStage.done, PermissionAccess.granted);
      case PermissionAccess.undetermined:
        // Android after a first no: it can ask once more.
        _go(PermissionStage.before, answer);
      case PermissionAccess.denied:
        unawaited(Haptics.warning());
        _go(PermissionStage.declined, answer);
      case PermissionAccess.restricted:
        unawaited(Haptics.warning());
        _go(PermissionStage.restricted, answer);
      case PermissionAccess.unavailable:
        _go(PermissionStage.done, answer);
    }
  }

  /// Back from Settings (the app came to the front): turned on, it's done.
  /// Where it can't be read without asking, a check, which asks nothing
  /// once it's been answered.
  Future<void> recheck() async {
    if (_checking) return;
    if (_stage != PermissionStage.declined &&
        _stage != PermissionStage.restricted) {
      return;
    }
    _checking = true;
    var now = await permission.access();
    if (now == PermissionAccess.unknown) now = await permission.request();
    _checking = false;
    if (now == PermissionAccess.granted) {
      unawaited(Haptics.success());
      _go(PermissionStage.done, now);
    }
  }

  /// Not now, or the feature's own way out: on without it.
  void setAside() => _go(PermissionStage.done, _result);

  Future<void> openSettings() => permission.openSettings();

  @override
  void dispose() {
    _disposed = true;
    if (!_done.isCompleted) _done.complete(_result);
    super.dispose();
  }
}

/// The quiet way out of a permission's sheet, declined or restricted: "Not
/// now" unless the feature has its own ("Paste code instead").
@immutable
class PermissionWayOut {
  const PermissionWayOut(this.label, this.onTap);

  final String label;

  /// After the flow is set aside.
  final VoidCallback onTap;
}

/// The screens showing now, newest last ([registerPermissionHost]).
final _hosts = <BuildContext>[];

/// Where a permission asked for away from any screen (notifications after
/// a device's first pairing) shows: the shell showing now, or null.
BuildContext? get permissionHost {
  for (final context in _hosts.reversed) {
    if (context.mounted) return context;
  }
  return null;
}

/// Makes [context] (a shell's, under the window's overlay) where
/// permissions asked for away from any screen show; answers how to stop.
VoidCallback registerPermissionHost(BuildContext context) {
  _hosts.add(context);
  return () => _hosts.remove(context);
}

/// Asks for [permission] where it's needed now: nothing when it's allowed
/// already or there's nothing to ask for; otherwise its sheet (a phone's)
/// or dialog (an iPad's, a Mac's), at the stage its access puts it, until
/// it's allowed or set aside. Answers what it ended on.
Future<PermissionAccess> askPermission(
  BuildContext context,
  Permission permission, {
  PermissionWayOut? wayOut,
}) async {
  final access = await permission.access();
  if (PermissionFlow.stageFor(access) == null || !context.mounted) {
    return access;
  }
  final flow = PermissionFlow(permission, from: access);
  await presentPermission(context, flow, wayOut: wayOut);
  // Not disposed: the sheet still listens as it slides away.
  return flow.result;
}

/// Shows [flow] until it's done or set aside: a sheet on a phone (held
/// while it explains or the system asks: no grabber, no swipe, no Close),
/// a dialog elsewhere. Completes as it closes.
///
/// [untilAnswered]: it closes once the system has answered, whatever it
/// said, for a feature that says a no in its own place (the mic's line).
Future<void> presentPermission(
  BuildContext context,
  PermissionFlow flow, {
  PermissionWayOut? wayOut,
  bool untilAnswered = false,
}) {
  Widget content(VoidCallback close) => PermissionContent(
    flow: flow,
    wayOut: wayOut,
    untilAnswered: untilAnswered,
    onDone: close,
  );

  if (isPhoneLayout(context)) {
    return presentSheet<void>(
      context,
      closeButton: false,
      shouldClose: () async => !flow.held,
      held: _Held(flow),
      builder: (context, close) => content(close),
    );
  }
  return showModal(context, (context, close) => content(close));
}

/// The sheet is held while the flow is.
class _Held implements ValueListenable<bool> {
  _Held(this.flow);

  final PermissionFlow flow;

  @override
  bool get value => flow.held;

  @override
  void addListener(VoidCallback listener) => flow.addListener(listener);

  @override
  void removeListener(VoidCallback listener) => flow.removeListener(listener);
}

/// "Settings → Apps → [app] → [item]": under Apps since iOS 18, at
/// Settings' top level before.
String iosSettingsPath(String item) {
  final major = int.tryParse(
    RegExp(r'(\d+)\.').firstMatch(Platform.operatingSystemVersion)?[1] ?? '',
  );
  final apps = major == null || major >= 18 ? 'Apps → ' : '';
  return 'Settings → $apps$appSettingsName → $item';
}

/// The app's name in Settings: Claktory Dev for any build but the App
/// Store's (#501).
String get appSettingsName => kReleaseMode ? 'Claktory' : 'Claktory Dev';

/// A permission's stage, as words and buttons: [compact] in a popover (no
/// glyph, read from the start), otherwise as #543's sheet. Keys: Return
/// presses the way on; Esc does nothing while it explains, and is the way
/// out after.
class PermissionContent extends StatefulWidget {
  const PermissionContent({
    super.key,
    required this.flow,
    this.wayOut,
    this.onDone,
    this.compact = false,
    this.untilAnswered = false,
    this.fill = false,
  });

  final PermissionFlow flow;
  final PermissionWayOut? wayOut;

  /// The flow reached done here (its sheet closes), or with
  /// [untilAnswered], the system answered.
  final VoidCallback? onDone;
  final bool compact;

  /// Done once the system has answered, whatever it said.
  final bool untilAnswered;

  /// Fills a screen's height (the scanner's first time): the words in its
  /// middle, scrolling when they must, the way on full width at its foot,
  /// under the thumb. No padding of its own.
  final bool fill;

  @override
  State<PermissionContent> createState() => _PermissionContentState();
}

class _PermissionContentState extends State<PermissionContent>
    with WidgetsBindingObserver {
  PermissionFlow get _flow => widget.flow;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _flow.addListener(_changed);
  }

  @override
  void didUpdateWidget(PermissionContent old) {
    super.didUpdateWidget(old);
    if (old.flow != widget.flow) {
      old.flow.removeListener(_changed);
      widget.flow.addListener(_changed);
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _flow.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (!mounted) return;
    setState(() {});
    if (_flow.stage == PermissionStage.done ||
        (widget.untilAnswered && !_flow.held)) {
      widget.onDone?.call();
    }
  }

  // Back from Settings: turned on, it goes on by itself.
  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) unawaited(_flow.recheck());
  }

  void _wayOut() {
    _flow.setAside();
    widget.wayOut?.onTap();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final words = _flow.permission.words(context);
    final stage = _flow.stage;
    final declined = stage == PermissionStage.declined;
    final restricted = stage == PermissionStage.restricted;
    final off = declined || restricted;
    final phone = isPhoneLayout(context);
    final compact = widget.compact;
    final scale = MediaQuery.textScalerOf(context).scale(1);
    // Centred while it's a few lines; larger, it reads from the start, and
    // at the accessibility sizes the glyph gives the words its room.
    final start = compact || scale >= 1.3;
    final align = start ? TextAlign.start : TextAlign.center;
    final title = off ? words.offTitle : words.title;
    final restriction = words.restrictedBody ?? restrictedFallback();
    // Restricted: what still works, then why it can't be changed.
    final body = restricted
        ? words.stillWorks ?? restriction
        : declined
        ? words.offBody
        : words.body;
    final caption = declined
        ? words.offCaption
        : restricted && words.stillWorks != null
        ? restriction
        : null;
    final text = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: start
          ? CrossAxisAlignment.start
          : CrossAxisAlignment.center,
      children: [
        if (!compact && scale < 2) ...[
          words.drawGlyph?.call(palette.textMuted, 44) ??
              ExtensionGlyph(
                icon: words.glyph,
                name: '',
                color: palette.textMuted,
                size: 44,
              ),
          const SizedBox(height: Space.l),
        ],
        // Wraps between words, never inside one ("Notifications" at the
        // largest text on a small iPhone).
        Semantics(
          header: true,
          child: FittedHeading(
            title,
            textAlign: align,
            style: (compact ? type.body : type.title).copyWith(
              color: palette.text,
              fontWeight: compact ? FontWeight.w600 : null,
            ),
          ),
        ),
        SizedBox(height: compact ? Space.s : Space.m),
        Text(
          body,
          textAlign: align,
          style: type.body.copyWith(color: palette.textMuted),
        ),
        if (caption != null) ...[
          const SizedBox(height: Space.m),
          Text(
            caption,
            textAlign: align,
            style: type.caption.copyWith(color: palette.textMuted),
          ),
        ],
      ],
    );
    final wayOutLabel = widget.wayOut?.label ?? 'Not now';
    final openSettings = Button(
      label: words.openSettings,
      primary: true,
      onPressed: () => unawaited(_flow.openSettings()),
    );
    final wayOut = _QuietLink(wayOutLabel, onTap: _wayOut);
    // Its own node, so dimmed it's read as itself, not over the sheet.
    final proceed = Semantics(
      container: true,
      child: Button(
        label: 'Continue',
        primary: true,
        onPressed: stage == PermissionStage.asking
            ? null
            : () => unawaited(_flow.proceed()),
      ),
    );
    // Restricted: the same way out, with no Open Settings to offer.
    final List<Widget> actions = restricted
        ? [
            if (phone || compact || widget.fill)
              Center(child: wayOut)
            else
              wayOut,
          ]
        : declined
        ? (phone || compact || widget.fill
              ? [openSettings, Center(child: wayOut)]
              : [wayOut, openSettings])
        : [proceed];
    // The way on at the foot, under the thumb on a phone; a dialog's
    // buttons at its end on an iPad or a Mac, the quiet one first.
    final Widget buttons = phone || compact || widget.fill
        ? Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: actions,
          )
        : Wrap(
            alignment: WrapAlignment.end,
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: Space.m,
            runSpacing: Space.s,
            children: actions,
          );
    void primary() {
      switch (stage) {
        case PermissionStage.before:
          unawaited(_flow.proceed());
        case PermissionStage.declined:
          unawaited(_flow.openSettings());
        case PermissionStage.restricted:
          _wayOut();
        case PermissionStage.asking || PermissionStage.done:
          break;
      }
    }

    // A keyboard's Return presses the way on, as an alert's default button
    // does; Esc, which closes a dialog, does nothing while it explains (the
    // way on is Continue) and is the way out once it's off.
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.enter): primary,
        const SingleActivator(LogicalKeyboardKey.numpadEnter): primary,
        const SingleActivator(LogicalKeyboardKey.escape): () {
          if (off) _wayOut();
        },
      },
      child: Focus(
        // A popover leaves the keys where they are while the keyboard on
        // screen is up (taking them would drop it from under the mic).
        autofocus: !widget.compact || device.hardwareKeyboard.value,
        child: widget.fill
            ? Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Expanded(
                    child: Center(
                      child: ScrollsWithFade(
                        // A new stage's words start at the top (row 619).
                        key: ValueKey(off),
                        child: Align(
                          alignment: start
                              ? Alignment.centerLeft
                              : Alignment.center,
                          child: text,
                        ),
                      ),
                    ),
                  ),
                  const SizedBox(height: Space.l),
                  buttons,
                ],
              )
            : _layout(phone && !compact, compact, start, off, text, buttons),
      ),
    );
  }

  Widget _layout(
    bool sheet,
    bool compact,
    bool start,
    bool off,
    Widget words,
    Widget buttons,
  ) {
    final pad = sheet ? Space.l : 0.0;
    return Padding(
      padding: EdgeInsets.fromLTRB(pad, pad, pad, sheet ? Space.m : 0),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // The words scroll at the largest sizes; the buttons stay.
          Flexible(
            child: ScrollsWithFade(
              // The words that say it's off start at the top, not where
              // Before's were scrolled to (Zucchero row 619).
              key: ValueKey(off),
              child: Align(
                alignment: start ? Alignment.centerLeft : Alignment.center,
                child: words,
              ),
            ),
          ),
          SizedBox(
            height: compact
                ? Space.m
                : off
                ? Space.l
                : Space.xl,
          ),
          buttons,
        ],
      ),
    );
  }
}

/// The quiet way out: a finger's height, its label a caption.
class _QuietLink extends StatelessWidget {
  const _QuietLink(this.label, {required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      label: label,
      onTap: onTap,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => ConstrainedBox(
          constraints: const BoxConstraints(minHeight: Touch.target),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            child: Center(
              widthFactor: 1,
              child: Text(
                label,
                textAlign: TextAlign.center,
                style: TypeScale.of(context).body.copyWith(
                  color: hovered ? palette.text : palette.textMuted,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
