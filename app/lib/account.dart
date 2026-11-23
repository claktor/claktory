import 'dart:convert';
import 'dart:math' as math;

import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import 'backend.dart';
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'markdown.dart';
import 'recovery_key.dart' show RecoveryKeyBox;
import 'mobile/drawer.dart' show largeText;
import 'mobile/input.dart' show InputKind;
import 'mobile/metrics.dart' show Metrics, Touch, TypeScale;
import 'name_step.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';
import 'settings_view.dart';
import 'usage.dart';

/// The Claktory account: signing this Claktory in to claktory.com with an
/// emailed code (the first launch's first step), and Settings → Account.
/// The account is the backend's (`Workspace.account`), so every device
/// shows the same and a device pairing with a Claktory that's signed in has
/// nothing to do.

/// Whether "Continue for now" put off signing in to [backend]'s Claktory
/// when claktory.com couldn't be reached. It lasts until the app quits, so
/// the step comes back at the next launch.
ValueNotifier<bool> signInPutOff(Backend backend) => _choice(backend);

/// The person asked to sign in (Settings → Account's Sign in…) after
/// Continue for now: the step shows again.
void askSignIn(Backend backend) => _choice(backend).value = false;

ValueNotifier<bool> _choice(Backend backend) =>
    _putOff[backend] ??= ValueNotifier(false);

final _putOff = Expando<ValueNotifier<bool>>('sign-in put off');

/// Whether to ask the person to sign in: the backend is connected, has
/// said what it knows, and isn't signed in (or is waiting for the code),
/// unless claktory.com couldn't be reached and they went on for now.
/// Signing in is required: nothing else in the app works until it's done.
/// A backend that knows no account (`Workspace.account` unset) isn't
/// asked about; one an older version let skip (SKIPPED) is.
bool asksSignIn(BuildContext context) {
  final backend = AppScope.of(context);
  final store = backend.workspace;
  if (!backend.connected.value || !store.received) return false;
  final workspace = store.workspace;
  if (!workspace.hasAccount()) return false;
  final state = workspace.account.state;
  if (state != ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_OUT &&
      state != ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_AWAITING_CODE &&
      state != ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SKIPPED) {
    return false;
  }
  return !signInPutOff(backend).value;
}

/// Whether [text] looks like an email address: something, @, a domain with
/// a dot. The service checks it properly.
bool looksLikeEmail(String text) =>
    RegExp(r'^[^@\s]+@[^@\s]+\.[^@\s.]+$').hasMatch(text.trim());

/// Opens the account's page on the web; tests replace it.
@visibleForTesting
Future<bool> Function(Uri url) openAccountPage = launchUrl;

/// What claktory.com keeps and why, linked from the sign-in step.
final privacyPage = Uri.parse('https://claktory.com/privacy');

/// "How Claktory uses your email", a quiet link to [privacyPage]: where
/// the person gives an email (signing a Claktory or a device in), so the
/// privacy policy is a tap away before anything is sent. A finger's height
/// on a phone.
class PrivacyLink extends StatelessWidget {
  const PrivacyLink({super.key});

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    return Align(
      alignment: Alignment.centerLeft,
      child: Semantics(
        link: true,
        child: Pressable(
          onTap: () => openAccountPage(privacyPage),
          builder: (context, hovered) {
            final link = Text(
              'How Claktory uses your email',
              style: TypeScale.of(context).caption.copyWith(
                color: hovered ? palette.textMuted : palette.textFaint,
                decoration: TextDecoration.underline,
                decorationColor: hovered
                    ? palette.textMuted
                    : palette.textFaint,
              ),
            );
            return phone
                ? ConstrainedBox(
                    constraints: const BoxConstraints(minHeight: Touch.target),
                    child: Align(
                      alignment: Alignment.centerLeft,
                      widthFactor: 1,
                      child: link,
                    ),
                  )
                : link;
          },
        ),
      ),
    );
  }
}

/// How long after a code is sent before another can be asked for.
const resendAfter = Duration(seconds: 30);

/// How long a code works (the service's 15 minutes).
const codeLife = Duration(minutes: 15);

/// An emailed sign-in code's digits.
const codeLength = 6;

/// The field for an emailed sign-in code, drawn as [codeLength] digit
/// boxes. Underneath it's one text field, invisible over the boxes, so it
/// types, deletes, pastes and autofills as any field does: typing fills the
/// boxes from the left, Delete empties them from the right, pasting the
/// whole code fills all six, and the system offers the code from Mail or
/// Messages. The boxes only show its digits, and while it's focused the
/// box the next digit goes in (the last when it's full) has the signal
/// border. To a screen reader it's one field, [semanticsLabel].
class CodeBoxes extends StatefulWidget {
  const CodeBoxes({
    super.key,
    required this.controller,
    this.autofocus = false,
    this.onSubmitted,
    this.semanticsLabel = 'Code from your email',
    this.scrollPadding = const EdgeInsets.all(20),
  });

  final TextEditingController controller;
  final bool autofocus;
  final ValueChanged<String>? onSubmitted;
  final String semanticsLabel;
  final EdgeInsets scrollPadding;

  @override
  State<CodeBoxes> createState() => _CodeBoxesState();
}

class _CodeBoxesState extends State<CodeBoxes> {
  final _focus = FocusNode();

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final metrics = Metrics.of(context);
    final style = TypeScale.of(context).title.copyWith(
      fontFamily: TextStyles.mono,
      fontWeight: FontWeight.w500,
      color: palette.text,
    );
    // The widest digit at the system's text size.
    final glyph = TextPainter(
      text: TextSpan(text: '0', style: style),
      textDirection: TextDirection.ltr,
      textScaler: MediaQuery.textScalerOf(context),
      maxLines: 1,
    )..layout();
    final glyphSize = glyph.size;
    glyph.dispose();
    final inset = metrics.phone ? Space.m : Space.s;
    return LayoutBuilder(
      builder: (context, constraints) {
        // As wide as a digit and its margins, at least a finger on a
        // phone; where six don't fit, the gaps shrink first, then the
        // boxes, and the digits with them.
        var width = math.max(
          metrics.phone ? metrics.target : 36.0,
          glyphSize.width + 2 * inset,
        );
        var gap = Space.s;
        final room = constraints.maxWidth;
        if (codeLength * width + (codeLength - 1) * gap > room) {
          gap = Space.xs;
        }
        if (codeLength * width + (codeLength - 1) * gap > room) {
          width = (room - (codeLength - 1) * gap) / codeLength;
        }
        final scale = math.min(1.0, (width - 2 * Space.xs) / glyphSize.width);
        final height = math.max(
          metrics.phone ? metrics.target : 40.0,
          glyphSize.height * scale + 2 * inset,
        );
        final boxes = ListenableBuilder(
          listenable: Listenable.merge([widget.controller, _focus]),
          builder: (context, _) {
            final value = widget.controller.value;
            final text = value.text;
            final selection = value.selection;
            final focused = _focus.hasFocus;
            final next = !focused || !selection.isValid
                ? null
                : selection.isCollapsed
                ? math.min(selection.extentOffset, codeLength - 1)
                : null;
            bool selected(int i) =>
                focused &&
                selection.isValid &&
                !selection.isCollapsed &&
                i >= selection.start &&
                i < selection.end;
            return Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (var i = 0; i < codeLength; i++) ...[
                  if (i > 0) SizedBox(width: gap),
                  Container(
                    key: ValueKey('code-box-$i'),
                    width: width,
                    height: height,
                    alignment: Alignment.center,
                    padding: const EdgeInsets.symmetric(horizontal: Space.xs),
                    decoration: BoxDecoration(
                      color: selected(i)
                          ? palette.signal.withValues(alpha: 0.28)
                          : palette.canvas,
                      borderRadius: BorderRadius.circular(Radii.control),
                      border: Border.all(
                        color: i == next ? palette.signal : palette.hairline,
                      ),
                    ),
                    child: i < text.length
                        ? FittedBox(
                            fit: BoxFit.scaleDown,
                            child: Text(text[i], style: style, maxLines: 1),
                          )
                        : null,
                  ),
                ],
              ],
            );
          },
        );
        return Align(
          alignment: AlignmentDirectional.centerStart,
          heightFactor: 1,
          child: Stack(
            children: [
              ExcludeSemantics(child: boxes),
              // The field itself, over the boxes, unseen: a tap or a
              // click anywhere on them focuses it, a long press or a
              // right click offers Paste, and a screen reader finds it.
              Positioned.fill(
                child: Opacity(
                  opacity: 0,
                  alwaysIncludeSemantics: true,
                  child: TextField(
                    controller: widget.controller,
                    focusNode: _focus,
                    autofocus: widget.autofocus,
                    kind: InputKind.oneTimeCode,
                    semanticsLabel: widget.semanticsLabel,
                    scrollPadding: widget.scrollPadding,
                    onSubmitted: widget.onSubmitted,
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

/// The first launch's first step, over the window until it's done: an
/// email address, then the code sent to it. Then the name step.
class SignInStep extends StatefulWidget {
  const SignInStep({super.key});

  @override
  State<SignInStep> createState() => _SignInStepState();
}

class _SignInStepState extends State<SignInStep> {
  final _email = TextEditingController();
  final _code = TextEditingController();
  var _busy = false;
  String? _error;

  /// The error is about the code, which a new one fixes: offer one.
  var _offerNewCode = false;

  /// claktory.com couldn't be reached; [_retry] tries again.
  Future<void> Function()? _retry;

  /// Typing another address while a code waits.
  var _changingEmail = false;

  /// A new code was sent from here.
  var _resent = false;

  @override
  void initState() {
    super.initState();
    _code.addListener(_codeChanged);
  }

  @override
  void dispose() {
    _email.dispose();
    _code.dispose();
    super.dispose();
  }

  Backend get _backend => AppScope.of(context);

  ClaktoryAccount get _account => _backend.workspace.workspace.account;

  bool get _awaitingCode =>
      _account.state ==
          ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_AWAITING_CODE &&
      !_changingEmail;

  /// Keeps the code to its digits, and sends it once all six are in.
  void _codeChanged() {
    final text = _code.text;
    var digits = text.replaceAll(RegExp(r'\D'), '');
    if (digits.length > 6) digits = digits.substring(0, 6);
    if (digits != text) {
      _code.value = TextEditingValue(
        text: digits,
        selection: TextSelection.collapsed(offset: digits.length),
      );
      return;
    }
    if (digits.length == 6 && !_busy) _verify();
  }

  /// Runs [request]; a refusal shows why, and an unreachable claktory.com
  /// switches to the step that offers to go on without it.
  Future<bool> _run(
    Future<void> Function() request, {
    bool verifying = false,
  }) async {
    if (_busy) return false;
    setState(() {
      _busy = true;
      _error = null;
      _offerNewCode = false;
      _retry = null;
    });
    try {
      await request();
      return true;
    } on RequestFailure catch (failure) {
      if (!mounted) return false;
      setState(() {
        if (failure.code == ErrorCode.ERROR_CODE_UNREACHABLE) {
          _retry = request;
        } else {
          _error = failure.message;
          _offerNewCode =
              verifying &&
              (failure.code == ErrorCode.ERROR_CODE_INVALID_REQUEST ||
                  failure.message.startsWith('Too many tries'));
        }
      });
      return false;
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _sendCode() async {
    final email = _email.text.trim();
    if (!looksLikeEmail(email)) {
      setState(() => _error = 'Enter an email address, like you@example.com.');
      return;
    }
    final backend = _backend;
    if (await _run(() => backend.accountSignIn(email)) && mounted) {
      setState(() {
        _changingEmail = false;
        _resent = false;
        _code.clear();
      });
    }
  }

  Future<void> _verify() async {
    final code = _code.text;
    final backend = _backend;
    if (!await _run(() => backend.accountVerify(code), verifying: true) &&
        mounted &&
        _retry == null) {
      // Wrong: ready for the code typed again.
      _code.clear();
    }
  }

  Future<void> _resend() async {
    final backend = _backend;
    if (await _run(backend.accountResend) && mounted) {
      setState(() {
        _resent = true;
        _code.clear();
      });
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: _backend.workspace,
    builder: (context, _) => StepCard(
      child: _retry != null
          ? _unreachable(context)
          : _awaitingCode
          ? _codeStep(context)
          : _emailStep(context),
    ),
  );

  Widget _title(BuildContext context, String title, String body) {
    final palette = Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Semantics(
          header: true,
          child: Text(
            title,
            style: stepTitleStyle(context).copyWith(color: palette.text),
          ),
        ),
        const SizedBox(height: Space.s),
        Text(
          body,
          style: TypeScale.of(context).body.copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.xl),
      ],
    );
  }

  Widget _errorLine(BuildContext context) => _error == null
      ? const SizedBox.shrink()
      : Padding(
          padding: const EdgeInsets.only(top: Space.s),
          child: Text(
            _error!,
            style: TypeScale.of(context).caption
                .copyWith(color: Palette.of(context).danger),
          ),
        );

  Widget _emailStep(BuildContext context) {
    final phone = isPhoneLayout(context);
    return ListenableBuilder(
      listenable: _email,
      builder: (context, _) => Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _title(
            context,
            'Sign in to Claktory',
            "Enter your email address. You'll get a code to sign in.",
          ),
          TextField(
            key: const ValueKey('email'),
            controller: _email,
            placeholder: 'Email address',
            autofocus: true,
            kind: InputKind.email,
            scrollPadding: stepFieldScrollPadding(context),
            onSubmitted: (_) => _sendCode(),
          ),
          _errorLine(context),
          const SizedBox(height: Space.xl),
          _buttons(context, [
            Button(
              label: 'Send code',
              primary: true,
              onPressed: _email.text.trim().isEmpty || _busy ? null : _sendCode,
            ),
            // Back to the code already sent, from typing another address.
            if (_changingEmail &&
                _account.state ==
                    ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_AWAITING_CODE)
              Button(
                label: 'Back',
                onPressed: () => setState(() {
                  _changingEmail = false;
                  _error = null;
                }),
              ),
          ]),
          SizedBox(height: phone ? Space.s : Space.l),
          const PrivacyLink(),
        ],
      ),
    );
  }

  /// The step's buttons, its action first: along the end of the card, the
  /// action last, or on a phone one under another, the action on top.
  Widget _buttons(BuildContext context, List<Widget> buttons) =>
      isPhoneLayout(context)
      ? StepButtons(children: buttons)
      : Wrap(
          alignment: WrapAlignment.end,
          spacing: Space.s,
          runSpacing: Space.s,
          children: buttons.reversed.toList(),
        );

  Widget _codeStep(BuildContext context) {
    final palette = Palette.of(context);
    final account = _account;
    // A code is sent [codeLife] before it expires.
    final sentAt = account.codeExpiresMs.toInt() == 0
        ? null
        : DateTime.fromMillisecondsSinceEpoch(account.codeExpiresMs.toInt())
              .subtract(codeLife);
    final resend = Ticking(
      active:
          sentAt != null && DateTime.now().isBefore(sentAt.add(resendAfter)),
      builder: (context, now) {
        final wait = sentAt == null
            ? Duration.zero
            : sentAt.add(resendAfter).difference(now);
        if (_resent && wait > Duration.zero) {
          return _note(context, 'New code sent.');
        }
        if (wait > Duration.zero) {
          return _note(
            context,
            'Resend in ${(wait.inMilliseconds / 1000).ceil()}s',
          );
        }
        return Button(label: 'Resend code', onPressed: _busy ? null : _resend);
      },
    );
    final different = Button(
      label: 'Use a different email',
      onPressed: () => setState(() {
        _changingEmail = true;
        _error = null;
        _offerNewCode = false;
        if (_email.text.isEmpty) _email.text = account.email;
      }),
    );
    return ListenableBuilder(
      listenable: _code,
      builder: (context, _) => Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _title(
            context,
            'Check your email',
            'Enter the 6-digit code sent to ${account.email}. It works for '
                '15 minutes.',
          ),
          CodeBoxes(
            key: const ValueKey('code'),
            controller: _code,
            autofocus: true,
            scrollPadding: stepFieldScrollPadding(context),
            onSubmitted: (_) =>
                _code.text.length == codeLength ? _verify() : null,
          ),
          _errorLine(context),
          const SizedBox(height: Space.s),
          Text(
            [
              if (account.codeFrom.isNotEmpty) "It's from ${account.codeFrom}.",
              "Can't find it? Check your spam folder.",
            ].join(' '),
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textFaint),
          ),
          if (_offerNewCode) ...[
            const SizedBox(height: Space.s),
            Align(
              alignment: Alignment.centerLeft,
              child: Button(
                label: 'Send new code',
                onPressed: _busy ? null : _resend,
              ),
            ),
          ],
          const SizedBox(height: Space.xl),
          if (isPhoneLayout(context))
            StepButtons(children: [_continueButton(), resend, different])
          else
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  child: Wrap(
                    spacing: Space.xs,
                    runSpacing: Space.xs,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: [resend, different],
                  ),
                ),
                const SizedBox(width: Space.s),
                _continueButton(),
              ],
            ),
        ],
      ),
    );
  }

  /// Sends the code typed, once it's whole.
  Widget _continueButton() => Button(
    label: 'Continue',
    primary: true,
    onPressed: _code.text.length == 6 && !_busy ? _verify : null,
  );

  /// A quiet line where a button sits, as tall as one.
  Widget _note(BuildContext context, String text) {
    final phone = isPhoneLayout(context);
    return ConstrainedBox(
      constraints: BoxConstraints(minHeight: phone ? Touch.target : 28),
      child: SizedBox(
        height: phone ? null : 28,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: Space.m),
          child: Center(
            widthFactor: 1,
            child: Text(
              text,
              textAlign: TextAlign.center,
              style: TypeScale.of(context).label
                  .copyWith(color: Palette.of(context).textFaint),
            ),
          ),
        ),
      ),
    );
  }

  Widget _unreachable(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _title(
          context,
          "Can't reach claktory.com right now.",
          "You can start now and sign in once it's back. Secrets aren't "
              'available until then.',
        ),
        _buttons(context, [
          Button(
            label: 'Try again',
            primary: true,
            onPressed: _busy
                ? null
                : () {
                    final retry = _retry!;
                    _run(retry);
                  },
          ),
          Button(
            label: 'Continue for now',
            onPressed: () => signInPutOff(_backend).value = true,
          ),
        ]),
      ],
    );
  }
}

/// Settings → Account: who this Claktory is signed in as, where, what it
/// sends there, its recovery key, and signing out.
class AccountSettings extends StatefulWidget {
  const AccountSettings({super.key, required this.account});

  final ClaktoryAccount account;

  @override
  State<AccountSettings> createState() => _AccountSettingsState();
}

class _AccountSettingsState extends State<AccountSettings>
    with SettingRequests {
  var _showSent = false;

  /// Back to the sign-in step, in front of everything.
  void _signIn() => askSignIn(AppScope.of(context));

  Future<void> _signInAgain() async {
    final backend = AppScope.of(context);
    final email = widget.account.email;
    await run(() async {
      if (email.isEmpty) return;
      await backend.accountSignIn(email);
    });
    if (mounted) _signIn();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final account = widget.account;
    final state = account.state;
    final signedOut =
        state == ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_OUT ||
        state == ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SKIPPED ||
        state == ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_UNSPECIFIED;
    final revoked =
        state == ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_REVOKED;
    final type = TypeScale.of(context);
    final muted = type.body.copyWith(color: palette.textMuted);
    final caption = type.caption.copyWith(color: palette.textMuted);
    // On a phone at the largest text, an action goes under its words.
    final stack = isPhoneLayout(context) && largeText(context);

    /// "At sign-in · 2 h ago": what was sent, and when.
    String sentLabel(String label, Int64 ms) =>
        ms == 0 ? label : '$label · ${formatAge(ms.toInt(), DateTime.now())}';
    final sent = [
      if (account.lastSignInJson.isNotEmpty)
        (sentLabel('At sign-in', account.lastSignInMs), account.lastSignInJson),
      if (account.lastCheckInJson.isNotEmpty)
        (
          sentLabel('Most recent', account.lastCheckInMs),
          account.lastCheckInJson,
        ),
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SettingRow(
          label: 'Email',
          child: signedOut
              ? _line(
                  stack: stack,
                  children: [
                    Text('Not signed in', style: muted),
                    Button(label: 'Sign in…', onPressed: _signIn),
                  ],
                )
              : Text(
                  account.email,
                  style: type.body.copyWith(color: palette.text),
                ),
        ),
        if (revoked)
          Padding(
            padding: const EdgeInsets.only(bottom: Space.m),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'This Claktory was removed from your account at '
                  'claktory.com. Sign in again so your secrets unlock on '
                  'their own after a restart.',
                  style: type.body.copyWith(color: palette.danger),
                ),
                const SizedBox(height: Space.s),
                Button(label: 'Sign in again', onPressed: _signInAgain),
              ],
            ),
          ),
        if (account.name.isNotEmpty)
          SettingRow(
            label: 'This Claktory',
            child: Text(
              account.name,
              style: type.body.copyWith(color: palette.text),
            ),
          ),
        SettingRow(
          label: 'Service',
          child: _line(
            stack: stack,
            children: [
              Flexible(
                child: Text(
                  serviceHost(account.service),
                  overflow: TextOverflow.ellipsis,
                  style: type.body.copyWith(color: palette.text),
                ),
              ),
              Button(
                label: 'Change…',
                onPressed: () => showModal(
                  context,
                  (context, close) =>
                      _ServiceForm(account: account, close: close),
                ),
              ),
            ],
          ),
        ),
        SettingRow(
          label: 'Last contact',
          child: Text(
            account.lastContactMs.toInt() == 0
                ? 'Never'
                : formatAge(account.lastContactMs.toInt(), DateTime.now()),
            style: type.body.copyWith(color: palette.text),
          ),
        ),
        if (account.problem.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(bottom: Space.m),
            child: Text(account.problem, style: caption),
          ),
        SettingRow(
          label: 'Shared with claktory.com',
          top: true,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'When it starts, and every hour while it runs, this Claktory '
                'tells claktory.com its version, the kind of computer it runs '
                "on, and how much it's used: how many agents, projects and "
                'devices, the hours agents worked, and tokens per model. '
                'claktory.com also works out roughly where it is (city, '
                'region and country) from its internet address. When it '
                'restarts, its internet address is kept for 30 days, so you '
                'can see where your secrets were unlocked. Never your '
                'conversations, prompts, files or secrets.',
                style: caption,
              ),
              if (sent.isNotEmpty) ...[
                const SizedBox(height: Space.xs),
                Pressable(
                  onTap: () => setState(() => _showSent = !_showSent),
                  builder: (context, hovered) => Unselectable(
                    child: Container(
                      // A finger's height on a phone.
                      constraints: isPhoneLayout(context)
                          ? const BoxConstraints(minHeight: Touch.target)
                          : null,
                      color: const Color(0x00000000),
                      child: Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Chevron(
                            color: hovered ? palette.text : palette.textMuted,
                            down: _showSent,
                          ),
                          const SizedBox(width: Space.xs),
                          Flexible(
                            child: Text(
                              'Show exactly what was sent',
                              style: type.label.copyWith(
                                color: hovered
                                    ? palette.text
                                    : palette.textMuted,
                              ),
                            ),
                          ),
                        ],
                      ),
                    ),
                  ),
                ),
                if (_showSent)
                  for (final (label, json) in sent) ...[
                    const SizedBox(height: Space.s),
                    Text(
                      label,
                      style: type.caption.copyWith(color: palette.textFaint),
                    ),
                    const SizedBox(height: Space.xs),
                    CodeBlock(prettyJson(json)),
                  ],
              ],
            ],
          ),
        ),
        Wrap(
          spacing: Space.s,
          runSpacing: Space.s,
          children: [
            Button(
              label: 'Manage account at claktory.com',
              onPressed: () => openAccountPage(accountPage(account.service)),
            ),
            if (!signedOut)
              Button(
                label: 'Show recovery key…',
                onPressed: () => showModal(
                  context,
                  (context, close) => _RecoveryKey(close: close),
                ),
              ),
            if (!signedOut)
              Button(
                label: 'Sign out…',
                onPressed: () => showModal(
                  context,
                  (context, close) => _SignOutForm(close: close),
                ),
              ),
          ],
        ),
        // Last and apart: it can't be undone. The account site asks once
        // more before it deletes anything.
        const SizedBox(height: Space.xl),
        _line(
          stack: stack,
          children: [
            Button(
              label: 'Delete account…',
              onPressed: () => openDeleteAccountPage(account.service),
            ),
            Expanded(
              child: Text(
                'Deletes your email and everything linked to it at '
                '${accountSiteName(account.service)}, after asking.',
                style: type.caption.copyWith(color: palette.textFaint),
              ),
            ),
          ],
        ),
        failureLine(palette),
      ],
    );
  }

  /// Words and a button side by side, [Space.m] apart; [stack]ed, one
  /// under the other at the start, each as wide as it needs.
  Widget _line({required bool stack, required List<Widget> children}) {
    if (!stack) {
      return Row(
        children: [
          for (final (i, child) in children.indexed) ...[
            if (i > 0) const SizedBox(width: Space.m),
            child,
          ],
        ],
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final (i, child) in children.indexed) ...[
          if (i > 0) const SizedBox(height: Space.s),
          // Room to wrap, not to fill.
          if (child is Flexible) child.child else child,
        ],
      ],
    );
  }
}

/// "account.claktory.com" for "https://account.claktory.com".
String serviceHost(String service) {
  final uri = Uri.tryParse(service);
  if (uri == null || uri.host.isEmpty) return service;
  return uri.hasPort ? '${uri.host}:${uri.port}' : uri.host;
}

/// The account's page on its service: claktory.com/account for
/// claktory.com's.
Uri accountPage(String service) {
  final uri = Uri.tryParse(service);
  if (uri == null || uri.host.isEmpty) {
    return Uri.parse('https://claktory.com/account');
  }
  return uri.replace(path: '/account');
}

/// Where the account is deleted on its service: the account site's
/// confirm step, signing in first if need be.
Uri deleteAccountPage(String service) =>
    accountPage(service).replace(path: '/account/delete');

/// Opens [deleteAccountPage] for [service]: Delete account… wherever the
/// account shows (Settings → Account, a phone's sign-in).
Future<bool> openDeleteAccountPage(String service) =>
    openAccountPage(deleteAccountPage(service));

/// "claktory.com" for claktory.com's service, else its host.
String accountSiteName(String service) {
  final host = Uri.tryParse(service)?.host ?? '';
  return host.isEmpty || host == 'account.claktory.com' ? 'claktory.com' : host;
}

/// [json] indented, or as it is when it isn't JSON.
String prettyJson(String json) {
  try {
    return const JsonEncoder.withIndent('  ').convert(jsonDecode(json));
  } on FormatException {
    return json;
  }
}

class _SignOutForm extends StatefulWidget {
  const _SignOutForm({required this.close});

  final VoidCallback close;

  @override
  State<_SignOutForm> createState() => _SignOutFormState();
}

class _SignOutFormState extends State<_SignOutForm> {
  var _busy = false;
  String? _error;

  Future<void> _signOut() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).accountSignOut();
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => FormFrame(
    title: 'Sign out of Claktory?',
    body:
        'Your agents keep working. If Claktory restarts before you sign in '
        'again, unlock your secrets with your recovery key.',
    fields: const [],
    error: _error,
    submitLabel: 'Sign out',
    onSubmit: _busy ? null : _signOut,
    onCancel: widget.close,
  );
}

/// The account service's address, for someone running their own.
class _ServiceForm extends StatefulWidget {
  const _ServiceForm({required this.account, required this.close});

  final ClaktoryAccount account;
  final VoidCallback close;

  @override
  State<_ServiceForm> createState() => _ServiceFormState();
}

class _ServiceFormState extends State<_ServiceForm> {
  late final _address = TextEditingController(text: widget.account.service);
  var _busy = false;
  String? _error;

  @override
  void dispose() {
    _address.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    final backend = AppScope.of(context);
    try {
      // The service changes only while signed out.
      final state = widget.account.state;
      if (state != ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_OUT &&
          state != ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SKIPPED &&
          state != ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_UNSPECIFIED) {
        await backend.accountSignOut();
      }
      await backend.setAccountService(_address.text.trim());
      signInPutOff(backend).value = false;
      widget.close();
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: _address,
    builder: (context, _) => FormFrame(
      title: 'Account service',
      body:
          "Only change this if you run your own account service. You'll "
          'sign in again there.',
      fields: [
        Field(
          label: 'Address',
          child: TextField(
            controller: _address,
            placeholder: 'https://account.claktory.com',
            autofocus: true,
            kind: InputKind.url,
            onSubmitted: (_) => _save(),
          ),
        ),
      ],
      error: _error,
      submitLabel: 'Save',
      onSubmit: _busy || _address.text.trim() == widget.account.service
          ? null
          : _save,
      onCancel: widget.close,
    ),
  );
}

/// The vault's recovery key, asked for when shown and never kept.
class _RecoveryKey extends StatefulWidget {
  const _RecoveryKey({required this.close});

  final VoidCallback close;

  @override
  State<_RecoveryKey> createState() => _RecoveryKeyState();
}

class _RecoveryKeyState extends State<_RecoveryKey> {
  String? _key;
  String? _error;
  var _copied = false;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final key = await AppScope.of(context).showRecoveryKey();
      if (mounted) setState(() => _key = key);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final key = _key;
    final copy = key == null
        ? null
        : Button(
            label: _copied ? 'Copied' : 'Copy',
            onPressed: () {
              Clipboard.setData(ClipboardData(text: key));
              setState(() => _copied = true);
            },
          );
    final done = Button(label: 'Done', primary: true, onPressed: widget.close);
    // On a phone at the largest text, one under another, Done first, each
    // the sheet's width.
    final stacked = isPhoneLayout(context) && largeText(context);
    final words = [
      Text(
        'Your recovery key',
        style: type.emphasis.copyWith(color: palette.text),
      ),
      const SizedBox(height: Space.s),
      Text(
        'Your secrets open with two halves together: one on the computer '
        'Claktory runs on and one at claktory.com. Neither can open them '
        'alone. This key opens them by itself, if claktory.com can\'t be '
        'reached, your account is gone or that computer\'s half is lost.\n\n'
        'Keep it in a password manager, such as Passwords or 1Password, or '
        'on paper kept somewhere safe. Never share it: anyone with it can '
        'open your secrets.',
        style: type.body.copyWith(color: palette.textMuted),
      ),
      const SizedBox(height: Space.xl),
      if (key != null)
        // Wrapped at its blocks, all of it in sight to copy down by hand
        // (Zucchero's row 131).
        RecoveryKeyBox(key)
      else if (_error case final error?)
        Text(error, style: type.caption.copyWith(color: palette.danger))
      else
        // Asked for; it comes in a moment.
        const SizedBox(height: 40),
    ];
    final buttons = [
      const SizedBox(height: Space.xl),
      if (stacked)
        StepButtons(children: [done, ?copy])
      else
        Row(
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            ?copy,
            const SizedBox(width: Space.s),
            done,
          ],
        ),
    ];
    // Taller than the sheet or the window (the largest text on a small
    // phone, Zucchero's row 126), it scrolls, as a form does: the words
    // and the key over the buttons, or, stacked, all of it together, since
    // even the buttons may not fit.
    if (stacked) {
      return SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [...words, ...buttons],
        ),
      );
    }
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Flexible(
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: words,
            ),
          ),
        ),
        ...buttons,
      ],
    );
  }
}
