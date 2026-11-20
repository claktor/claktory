import 'dart:convert';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import 'backend.dart';
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'markdown.dart';
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
ValueNotifier<bool> signInPutOff(Backend backend) =>
    _putOff[backend] ??= ValueNotifier(false);

final _putOff = Expando<ValueNotifier<bool>>('sign-in put off');

/// Whether to ask the person to sign in: the backend is connected, has
/// said what it knows, and isn't signed in (or is waiting for the code),
/// unless they've put it off for now. A backend that knows no account
/// (`Workspace.account` unset) isn't asked about.
bool asksSignIn(BuildContext context) {
  final backend = AppScope.of(context);
  final store = backend.workspace;
  if (!backend.connected.value || !store.received) return false;
  final workspace = store.workspace;
  if (!workspace.hasAccount()) return false;
  final state = workspace.account.state;
  if (state != ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_OUT &&
      state != ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_AWAITING_CODE) {
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

/// How long after a code is sent before another can be asked for.
const resendAfter = Duration(seconds: 30);

/// How long a code works (the service's 15 minutes).
const codeLife = Duration(minutes: 15);

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

  Widget _title(Palette palette, String title, String body) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      Text(title, style: TextStyles.emphasis.copyWith(color: palette.text)),
      const SizedBox(height: Space.s),
      Text(body, style: TextStyles.body.copyWith(color: palette.textMuted)),
      const SizedBox(height: Space.xl),
    ],
  );

  Widget _errorLine(Palette palette) => _error == null
      ? const SizedBox.shrink()
      : Padding(
          padding: const EdgeInsets.only(top: Space.s),
          child: Text(
            _error!,
            style: TextStyles.caption.copyWith(color: palette.danger),
          ),
        );

  Widget _emailStep(BuildContext context) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: _email,
      builder: (context, _) => Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _title(
            palette,
            'Sign in to Claktory',
            "Enter your email address. You'll get a code to sign in.",
          ),
          TextField(
            key: const ValueKey('email'),
            controller: _email,
            placeholder: 'Email address',
            autofocus: true,
            onSubmitted: (_) => _sendCode(),
          ),
          _errorLine(palette),
          const SizedBox(height: Space.xl),
          Align(
            alignment: Alignment.centerRight,
            child: Button(
              label: 'Send code',
              primary: true,
              onPressed: _email.text.trim().isEmpty || _busy ? null : _sendCode,
            ),
          ),
          const SizedBox(height: Space.l),
          Text(
            'Claktory shares your email, roughly where it runs, and how much '
            "it's used (agents, hours, tokens). Never your conversations, "
            'files or secrets.',
            style: TextStyles.caption.copyWith(color: palette.textFaint),
          ),
        ],
      ),
    );
  }

  Widget _codeStep(BuildContext context) {
    final palette = Palette.of(context);
    final account = _account;
    // A code is sent [codeLife] before it expires.
    final sentAt = account.codeExpiresMs.toInt() == 0
        ? null
        : DateTime.fromMillisecondsSinceEpoch(account.codeExpiresMs.toInt())
              .subtract(codeLife);
    return ListenableBuilder(
      listenable: _code,
      builder: (context, _) => Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _title(
            palette,
            'Check your email',
            'Enter the 6-digit code sent to ${account.email}. It works for '
                '15 minutes.',
          ),
          TextField(
            key: const ValueKey('code'),
            controller: _code,
            autofocus: true,
            onSubmitted: (_) => _code.text.length == 6 ? _verify() : null,
          ),
          _errorLine(palette),
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
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Wrap(
                  spacing: Space.xs,
                  runSpacing: Space.xs,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    Ticking(
                      active:
                          sentAt != null &&
                          DateTime.now().isBefore(sentAt.add(resendAfter)),
                      builder: (context, now) {
                        final wait = sentAt == null
                            ? Duration.zero
                            : sentAt.add(resendAfter).difference(now);
                        if (_resent && wait > Duration.zero) {
                          return _note(palette, 'New code sent.');
                        }
                        if (wait > Duration.zero) {
                          return _note(
                            palette,
                            'Resend in ${(wait.inMilliseconds / 1000).ceil()}s',
                          );
                        }
                        return Button(
                          label: 'Resend code',
                          onPressed: _busy ? null : _resend,
                        );
                      },
                    ),
                    Button(
                      label: 'Use a different email',
                      onPressed: () => setState(() {
                        _changingEmail = true;
                        _error = null;
                        _offerNewCode = false;
                        if (_email.text.isEmpty) _email.text = account.email;
                      }),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: Space.s),
              Button(
                label: 'Continue',
                primary: true,
                onPressed: _code.text.length == 6 && !_busy ? _verify : null,
              ),
            ],
          ),
        ],
      ),
    );
  }

  /// A quiet line where a button sits, as tall as one.
  Widget _note(Palette palette, String text) => SizedBox(
    height: 28,
    child: Padding(
      padding: const EdgeInsets.symmetric(horizontal: Space.m),
      child: Center(
        widthFactor: 1,
        child: Text(
          text,
          style: TextStyles.label.copyWith(color: palette.textFaint),
        ),
      ),
    ),
  );

  Widget _unreachable(BuildContext context) {
    final palette = Palette.of(context);
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _title(
          palette,
          "Can't reach claktory.com right now.",
          "You can start now and sign in once it's back. Secrets aren't "
              'available until then.',
        ),
        Wrap(
          alignment: WrapAlignment.end,
          spacing: Space.s,
          runSpacing: Space.s,
          children: [
            Button(
              label: 'Continue for now',
              onPressed: () => signInPutOff(_backend).value = true,
            ),
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
          ],
        ),
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
  void _signIn() => signInPutOff(AppScope.of(context)).value = false;

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
        state == ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_UNSPECIFIED;
    final revoked =
        state == ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_REVOKED;
    final muted = TextStyles.body.copyWith(color: palette.textMuted);
    final caption = TextStyles.caption.copyWith(color: palette.textMuted);

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
              ? Row(
                  children: [
                    Text('Not signed in', style: muted),
                    const SizedBox(width: Space.m),
                    Button(label: 'Sign in…', onPressed: _signIn),
                  ],
                )
              : Text(
                  account.email,
                  style: TextStyles.body.copyWith(color: palette.text),
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
                  style: TextStyles.body.copyWith(color: palette.danger),
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
              style: TextStyles.body.copyWith(color: palette.text),
            ),
          ),
        SettingRow(
          label: 'Service',
          child: Row(
            children: [
              Flexible(
                child: Text(
                  serviceHost(account.service),
                  overflow: TextOverflow.ellipsis,
                  style: TextStyles.body.copyWith(color: palette.text),
                ),
              ),
              const SizedBox(width: Space.m),
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
            style: TextStyles.body.copyWith(color: palette.text),
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
                            style: TextStyles.label.copyWith(
                              color: hovered ? palette.text : palette.textMuted,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
                if (_showSent)
                  for (final (label, json) in sent) ...[
                    const SizedBox(height: Space.s),
                    Text(
                      label,
                      style: TextStyles.caption.copyWith(
                        color: palette.textFaint,
                      ),
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
        failureLine(palette),
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
    final key = _key;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'Your recovery key',
          style: TextStyles.emphasis.copyWith(color: palette.text),
        ),
        const SizedBox(height: Space.s),
        Text(
          "It unlocks your secrets if claktory.com can't be reached or your "
          'account is gone. Keep it somewhere safe, such as your password '
          'manager.',
          style: TextStyles.body.copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.xl),
        if (key != null)
          CodeBlock(key)
        else if (_error case final error?)
          Text(error, style: TextStyles.caption.copyWith(color: palette.danger))
        else
          // Asked for; it comes in a moment.
          const SizedBox(height: 40),
        const SizedBox(height: Space.xl),
        Row(
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            if (key != null)
              Button(
                label: _copied ? 'Copied' : 'Copy',
                onPressed: () {
                  Clipboard.setData(ClipboardData(text: key));
                  setState(() => _copied = true);
                },
              ),
            const SizedBox(width: Space.s),
            Button(label: 'Done', primary: true, onPressed: widget.close),
          ],
        ),
      ],
    );
  }
}
