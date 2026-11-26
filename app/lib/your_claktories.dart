import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'account.dart'
    show
        CodeBoxes,
        PrivacyLink,
        accountSiteName,
        codeLength,
        looksLikeEmail,
        openDeleteAccountPage;
import 'backend.dart' show Backend;
import 'claktory_identity.dart' show claktoryName;
import 'connections.dart';
import 'controls.dart';
import 'device_account.dart';
import 'device_kind.dart' show thisDeviceName;
import 'device_vouch.dart' show SignInThroughThisMac;
import 'devices.dart' show thisDeviceKind;
import 'forms.dart' show FormFrame;
import 'identity.dart';
import 'markdown.dart' show Markdown;
import 'mobile/drawer.dart' show largeText;
import 'mobile/input.dart' show InputKind;
import 'mobile/lines.dart' show ControlsLine;
import 'mobile/metrics.dart' show Touch, TypeScale;
import 'name_step.dart' show StepButtons, stepFieldScrollPadding;
import 'pairing.dart'
    show DeviceNumberField, decideDevice, deviceCallingItself, isDeviceNumber;
import 'people/e2ee.dart' show DmSignOut, E2ee, e2eeFor, e2eeOf, startedE2ee;
import 'people/people_view.dart' show peopleAvailable;
import 'protocol/factory/v1/factory.pb.dart';
import 'rail.dart'
    show Claktories, ClaktoriesScope, claktoryInitials, confirmRemoveClaktory;
import 'scope.dart';
import 'usage.dart' show formatAge;

/// Where to get Claktory for a Mac, a Linux server or Windows.
const downloadUrl = 'https://claktory.com/download';

/// Signing this device in to the person's Claktory account, and "Your
/// Claktories": the ones on their account, each connected to with one
/// confirmation, no code needed (onboarding slice 6). The account is this
/// device's own ([Connections.account]), not a Claktory's.

/// The account service a device signs in with: one a Claktory it shows
/// already uses (the dev Claktory's, say), else claktory.com's.
String accountServiceFor(Claktories? claktories) {
  for (final claktory in claktories?.all ?? const []) {
    final service = claktory.backend.workspace.workspace.account.service;
    if (service.isNotEmpty) return service;
  }
  return defaultAccountService;
}

/// "claktory.com", or a service's host.
String accountServiceName(String service) {
  if (service == defaultAccountService) return 'claktory.com';
  return Uri.tryParse(service)?.host ?? service;
}

/// This device's platform as the account service shows it: ios, macos,
/// android, linux or windows (an iPad says ios too).
String get thisDevicePlatform => switch (defaultTargetPlatform) {
  TargetPlatform.iOS => 'ios',
  TargetPlatform.macOS => 'macos',
  TargetPlatform.android => 'android',
  TargetPlatform.linux => 'linux',
  TargetPlatform.windows => 'windows',
  TargetPlatform.fuchsia => '',
};

/// What went wrong with a call to the account service, in the app's words.
String accountProblem(Object error, String service) {
  final name = accountServiceName(service);
  if (error is AccountServiceUnreachable) {
    return "Can't reach $name. Check your internet connection and try again.";
  }
  if (error is! AccountServiceError) return 'Something went wrong: $error';
  switch (error.code) {
    case 'invalid_email':
      return 'Enter an email address, like you@example.com.';
    case 'wrong_code':
      return switch (error.attemptsLeft ?? 0) {
        0 => 'Too many tries. Get a new code.',
        1 => "That code isn't right. 1 try left.",
        final left => "That code isn't right. $left tries left.",
      };
    case 'code_expired':
      return 'That code has expired. Get a new code.';
    case 'rate_limited':
      final wait = error.details['retry_after'];
      final minutes = wait is int ? (wait / 60).ceil() : 1;
      return 'Too many tries. Try again in '
          '${minutes <= 1 ? '1 minute' : '$minutes minutes'}.';
    case 'email_unavailable':
      return "$name can't send email right now. Try again later.";
    case 'too_many_devices':
      return 'Your account has too many devices signed in. Remove one at '
          '$name/account, then try again.';
    case 'internal':
      return '$name has a problem right now. Try again later.';
  }
  return error.message.isNotEmpty ? error.message : 'Something went wrong.';
}

/// Signs this device out of the Claktory account. Where it has messages,
/// or Claktories it reached through the account, it asks first, saying
/// what happens to them (row 671, Fugo; #529 MC5; Trish's words): the
/// messages deleted here when the person's other devices have them, kept
/// here, locked, when it's the only one, unless the person deletes them;
/// those Claktories stop connecting until it signs in again.
Future<void> signOutOfAccount(
  BuildContext context,
  DeviceAccount account,
) async {
  final stopping = _throughAccount(context);
  final e2ee = _messagesOf(context, account);
  final plan = e2ee == null ? DmSignOut.none : await e2ee.signOutPlan();
  final messages = plan == DmSignOut.none ? null : e2ee;
  if (messages == null && stopping.isEmpty) return account.signOut();
  if (!context.mounted) return;
  await showModal(
    context,
    (context, close) => _SignOutDevice(
      account: account,
      e2ee: messages,
      alone: plan == DmSignOut.alone,
      stopping: stopping,
      close: close,
    ),
  );
}

/// The names of the Claktories this device reached through the account
/// (no code of their own), as the rail shows them: each lets it in only
/// while it's signed in.
List<String> _throughAccount(BuildContext context) {
  final connections = AppScope.scope(context).connections;
  final claktories = ClaktoriesScope.of(context);
  return [
    for (final target in connections.saved)
      if (target.fromAccount)
        switch (claktories?.all.where((each) => each.target == target)) {
          final found? when found.isNotEmpty => claktoryName(
            found.first.backend,
          ),
          _ => target.label,
        },
  ];
}

/// What signing out does to [names], the Claktories reached through the
/// account, on this [device] (Trish's words); empty for none.
String stopConnecting(List<String> names, String device) {
  final until = 'on this $device until you sign in again.';
  return switch (names) {
    [] => '',
    [final one] => '$one stops connecting $until',
    [final a, final b] => '$a and $b stop connecting $until',
    [final a, final b, ...final rest] =>
      '$a, $b and ${rest.length} more stop connecting $until',
  };
}

/// This device's messages for [account]: its engine if it runs, or one
/// started where the Claktory shown offers People; null otherwise.
E2ee? _messagesOf(BuildContext context, DeviceAccount account) {
  if (e2eeFor case final hook?) return hook(context);
  if (startedE2ee(account) case final known?) return known;
  final workspace = AppScope.scope(context).backend.workspace.workspace;
  return peopleAvailable(workspace) ? e2eeOf(context) : null;
}

class _SignOutDevice extends StatefulWidget {
  const _SignOutDevice({
    required this.account,
    required this.e2ee,
    required this.alone,
    required this.stopping,
    required this.close,
  });

  final DeviceAccount account;

  /// Its messages; null when it has none.
  final E2ee? e2ee;

  /// The Claktories reached through the account, by name.
  final List<String> stopping;

  /// The only device with the person's messages: they stay, unless
  /// deleted.
  final bool alone;
  final VoidCallback close;

  @override
  State<_SignOutDevice> createState() => _SignOutDeviceState();
}

class _SignOutDeviceState extends State<_SignOutDevice> {
  var _delete = false;
  var _busy = false;

  Future<void> _signOut() async {
    setState(() => _busy = true);
    final e2ee = widget.e2ee;
    try {
      if (e2ee == null) {
        // No messages here: nothing to leave or delete.
      } else if (!widget.alone) {
        await e2ee.leave();
      } else if (_delete) {
        await e2ee.deleteHere();
      }
    } finally {
      // Signed out in any case: what's here went, or stays locked. Gone
      // from the person's messages, its key goes too: their other devices
      // take it out of the account, so it comes back as a new device.
      await widget.account.signOut(newKey: e2ee != null && !widget.alone);
      widget.close();
    }
  }

  @override
  Widget build(BuildContext context) {
    final kind = thisDeviceKind;
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    void choose(bool on) => setState(() => _delete = on);
    final label = 'Delete messages from this $kind';
    final messages = widget.e2ee == null
        ? null
        : widget.alone
        ? 'This is the only device with your messages. They stay on this '
              '$kind, locked, until you sign in again with this account.'
        : 'Your messages will be deleted from this $kind. Your other '
              'devices keep them, and you can add this $kind again from '
              'one of them.';
    return FormFrame(
      title: 'Sign out of your Claktory account?',
      body: [
        ?messages,
        if (widget.stopping.isNotEmpty) stopConnecting(widget.stopping, kind),
      ].join(' '),
      fields: [
        if (widget.e2ee != null && widget.alone)
          ToggleTap(
            value: _delete,
            onChanged: _busy ? null : choose,
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Padding(
                  padding: const EdgeInsets.only(top: 2),
                  child: Toggle(
                    value: _delete,
                    label: label,
                    onChanged: _busy ? null : choose,
                  ),
                ),
                const SizedBox(width: Space.m),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        label,
                        style: type.body.copyWith(color: palette.text),
                      ),
                      if (_delete)
                        Text(
                          "Your security key is deleted too. Next time you'll "
                          'start over, and your friends will see your '
                          'security key change.',
                          style: type.caption.copyWith(
                            color: palette.textMuted,
                          ),
                        ),
                    ],
                  ),
                ),
              ],
            ),
          ),
      ],
      error: null,
      submitLabel: _delete ? 'Sign out and delete' : 'Sign out',
      onSubmit: _busy ? null : () => unawaited(_signOut()),
      onCancel: widget.close,
    );
  }
}

/// Signs this device in with an emailed code: its email, then the code.
/// [intro] says why, above the email field.
class DeviceSignInForm extends StatefulWidget {
  const DeviceSignInForm({super.key, required this.intro, this.email});

  final String intro;

  /// Signs in again only with this address (the account this device is
  /// signed in to), shown rather than typed, so a mistyped one can't sign
  /// it in to another account (Abbacchio's row 644). Null: any address.
  final String? email;

  @override
  State<DeviceSignInForm> createState() => _DeviceSignInFormState();
}

class _DeviceSignInFormState extends State<DeviceSignInForm> {
  final _email = TextEditingController();
  final _code = TextEditingController();
  var _busy = false;
  String? _error;

  /// The error is about the code, which a new one fixes: offer one.
  var _offerNewCode = false;

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

  DeviceAccount get _account => AppScope.scope(context).connections.account;

  String get _service => _account.signedIn
      ? _account.service
      : accountServiceFor(ClaktoriesScope.of(context));

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
    if (digits.length == 6 && !_busy) unawaited(_verify());
  }

  Future<bool> _run(Future<void> Function() request) async {
    if (_busy) return false;
    final service = _service;
    setState(() {
      _busy = true;
      _error = null;
      _offerNewCode = false;
    });
    try {
      await request();
      return true;
    } catch (error) {
      if (mounted) {
        setState(() {
          _error = accountProblem(error, service);
          _offerNewCode =
              error is AccountServiceError &&
              (error.code == 'code_expired' ||
                  (error.code == 'wrong_code' && error.attemptsLeft == 0));
        });
      }
      return false;
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _sendCode() async {
    final email = widget.email ?? _email.text.trim();
    if (!looksLikeEmail(email)) {
      setState(() => _error = 'Enter an email address, like you@example.com.');
      return;
    }
    final account = _account;
    final service = _service;
    final version = AppScope.scope(context).appBuild.version;
    final sent = await _run(
      () async => account.start(
        email: email,
        service: service,
        name: await thisDeviceName(),
        platform: thisDevicePlatform,
        version: version,
      ),
    );
    if (sent && mounted) _code.clear();
  }

  Future<void> _verify() async {
    final account = _account;
    final code = _code.text;
    if (!await _run(() => account.verify(code)) && mounted) _code.clear();
  }

  @override
  Widget build(BuildContext context) {
    final account = _account;
    return ListenableBuilder(
      listenable: account,
      builder: (context, _) {
        final waiting = account.awaitingCode;
        return waiting == null
            ? _emailStep(context)
            : _codeStep(context, waiting);
      },
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
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return ListenableBuilder(
      listenable: _email,
      builder: (context, _) {
        final send = Button(
          label: 'Send code',
          primary: true,
          onPressed:
              (widget.email == null && _email.text.trim().isEmpty) || _busy
              ? null
              : _sendCode,
        );
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // A Mac whose own Claktory is signed in: one click (#529).
            const SignInThroughThisMac(),
            Text(
              widget.intro,
              style: type.body.copyWith(color: palette.textMuted),
            ),
            const SizedBox(height: Space.l),
            if (widget.email case final email?)
              Text(
                email,
                key: const ValueKey('device-email-fixed'),
                style: type.body.copyWith(color: palette.text),
              )
            else
              TextField(
                key: const ValueKey('device-email'),
                controller: _email,
                placeholder: 'Email address',
                kind: InputKind.email,
                scrollPadding: stepFieldScrollPadding(context),
                onSubmitted: (_) => _sendCode(),
              ),
            _errorLine(context),
            const SizedBox(height: Space.l),
            if (isPhoneLayout(context))
              StepButtons(children: [send])
            else
              Align(alignment: Alignment.centerRight, child: send),
            SizedBox(height: isPhoneLayout(context) ? Space.s : Space.l),
            const PrivacyLink(),
          ],
        );
      },
    );
  }

  Widget _codeStep(BuildContext context, DeviceCodeWait waiting) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return ListenableBuilder(
      listenable: _code,
      builder: (context, _) {
        final newCode = _offerNewCode
            ? Button(
                label: 'Send new code',
                onPressed: _busy
                    ? null
                    : () {
                        _email.text = waiting.email;
                        unawaited(_sendCode());
                      },
              )
            : null;
        final different = widget.email != null
            ? null
            : Button(
                label: 'Use a different email',
                onPressed: () {
                  _email.text = waiting.email;
                  setState(() {
                    _error = null;
                    _offerNewCode = false;
                  });
                  _account.cancelSignIn();
                },
              );
        final continueButton = Button(
          label: 'Continue',
          primary: true,
          onPressed: _code.text.length == 6 && !_busy ? _verify : null,
        );
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              'Enter the 6-digit code sent to ${waiting.email}. It works for '
              '15 minutes.',
              style: type.body.copyWith(color: palette.textMuted),
            ),
            const SizedBox(height: Space.l),
            CodeBoxes(
              key: const ValueKey('device-code'),
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
                if (waiting.codeFrom.isNotEmpty)
                  "It's from ${waiting.codeFrom}.",
                "Can't find it? Check your spam folder.",
              ].join(' '),
              style: type.caption.copyWith(color: palette.textFaint),
            ),
            const SizedBox(height: Space.l),
            if (isPhoneLayout(context))
              StepButtons(children: [continueButton, ?newCode, ?different])
            else
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(
                    child: Wrap(
                      spacing: Space.xs,
                      runSpacing: Space.xs,
                      children: [?newCode, ?different],
                    ),
                  ),
                  const SizedBox(width: Space.s),
                  continueButton,
                ],
              ),
          ],
        );
      },
    );
  }
}

/// "Your Claktories": signed in, the ones on this account, each a row to
/// connect to after one confirmation; signed out, the sign-in. [onConnect]
/// connects to the one picked (by default it shows in the window).
class YourClaktories extends StatefulWidget {
  const YourClaktories({
    super.key,
    this.onConnect,
    this.hideConnected = false,
    this.offerDelete = false,
  });

  final void Function(RemoteFactory target)? onConnect;

  /// Leaves out the ones this app shows already (listed beside it).
  final bool hideConnected;

  /// Offers Delete account… under Sign out: where this is the only place
  /// the account shows (a phone with no Claktory yet), so an account made
  /// here can be deleted here too (App Store 5.1.1(v)).
  final bool offerDelete;

  @override
  State<YourClaktories> createState() => _YourClaktoriesState();
}

class _YourClaktoriesState extends State<YourClaktories> {
  /// The account's Claktories, once listed; null while they're looked for.
  List<AccountClaktory>? _listed;
  String? _error;

  /// The one picked, asking to confirm.
  AccountClaktory? _confirming;

  /// Who the list is for: listed again for another email.
  String _listedFor = '';

  DeviceAccount? _account;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final account = AppScope.scope(context).connections.account;
    if (account == _account) return;
    _account?.removeListener(_changed);
    _account = account..addListener(_changed);
    _changed();
  }

  @override
  void dispose() {
    _account?.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    final account = _account!;
    if (account.signedIn && account.email != _listedFor) {
      _listedFor = account.email;
      unawaited(_list());
    } else if (!account.signedIn && _listedFor.isNotEmpty) {
      _listedFor = '';
      setState(() {
        _listed = null;
        _confirming = null;
      });
    }
  }

  Future<void> _list() async {
    final account = _account!;
    setState(() => _error = null);
    try {
      final listed = await account.claktories();
      if (mounted) setState(() => _listed = listed);
    } catch (error) {
      if (mounted) {
        setState(() => _error = accountProblem(error, account.service));
      }
    }
  }

  void _connect(AccountClaktory claktory) {
    final target = RemoteFactory.fromAccount(claktory);
    final connect = widget.onConnect;
    if (connect != null) {
      connect(target);
    } else {
      AppScope.switchFactory(context, target);
    }
  }

  @override
  Widget build(BuildContext context) {
    final account = _account!;
    return ListenableBuilder(
      listenable: account,
      builder: (context, _) {
        if (!account.signedIn) {
          return const DeviceSignInForm(
            intro:
                'Sign in with the email you use for Claktory to see your '
                'Claktories and connect without a code.',
          );
        }
        final confirming = _confirming;
        if (confirming != null) return _confirm(context, confirming);
        return _listView(context, account);
      },
    );
  }

  Widget _confirm(BuildContext context, AccountClaktory claktory) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final cancel = Button(
      label: 'Cancel',
      onPressed: () => setState(() => _confirming = null),
    );
    final connect = Button(
      label: 'Connect',
      primary: true,
      onPressed: () => _connect(claktory),
    );
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            AccountClaktoryTile(claktory: claktory),
            const SizedBox(width: Space.m),
            Expanded(
              child: Text(
                'Connect to ${claktory.name}?',
                style: type.emphasis.copyWith(color: palette.text),
              ),
            ),
          ],
        ),
        const SizedBox(height: Space.s),
        Text(
          'If you already use ${claktory.name} on another device, allow this '
          '$thisDeviceKind there.',
          style: type.body.copyWith(color: palette.textMuted),
        ),
        const SizedBox(height: Space.l),
        if (isPhoneLayout(context))
          StepButtons(children: [connect, cancel])
        else
          Wrap(
            alignment: WrapAlignment.end,
            spacing: Space.s,
            children: [cancel, connect],
          ),
      ],
    );
  }

  Widget _listView(BuildContext context, DeviceAccount account) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final claktories = ClaktoriesScope.of(context);
    final listed = _listed;
    final error = _error;
    // Those this app already shows, by their endpoint id.
    final shown = <String, String>{
      for (final (backend, target)
          in claktories?.all.map((each) => (each.backend, each.target)) ??
              [(AppScope.of(context), AppScope.of(context).target)])
        if (backend.endpointId.isNotEmpty)
          backend.endpointId: target == null
              ? 'This $thisDeviceKind'
              : 'Connected',
    };
    final now = DateTime.now();
    final signOut = Button(
      label: 'Sign out',
      onPressed: () => unawaited(signOutOfAccount(context, account)),
    );
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (error != null) ...[
          Text(error, style: type.caption.copyWith(color: palette.danger)),
          const SizedBox(height: Space.s),
          Align(
            alignment: Alignment.centerLeft,
            child: Button(label: 'Try again', onPressed: _list),
          ),
        ] else if (listed == null)
          Text(
            'Looking for your Claktories…',
            style: type.body.copyWith(color: palette.textMuted),
          )
        else if (listed.isEmpty)
          Markdown(
            'No Claktories on this account yet. Install Claktory on a Mac or a '
            'Linux server (or Windows, in preview) from '
            '[claktory.com/download]($downloadUrl) and sign in with this '
            'email, and it shows up here.',
            color: palette.textMuted,
          )
        else
          for (final claktory in listed)
            if (!widget.hideConnected || shown[claktory.endpointId] == null)
              _AccountClaktoryRow(
                claktory: claktory,
                status: shown[claktory.endpointId],
                now: now,
                onTap: () => setState(() => _confirming = claktory),
              ),
        const SizedBox(height: Space.l),
        // On a phone at the largest text, Sign out under the email.
        if (isPhoneLayout(context) && largeText(context))
          Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'Signed in as ${account.email}',
                style: type.caption.copyWith(color: palette.textFaint),
              ),
              const SizedBox(height: Space.s),
              signOut,
            ],
          )
        else
          Row(
            children: [
              Expanded(
                child: Text(
                  'Signed in as ${account.email}',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: type.caption.copyWith(color: palette.textFaint),
                ),
              ),
              signOut,
            ],
          ),
        if (widget.offerDelete) ...[
          const SizedBox(height: Space.xl),
          _deleteAccount(context, account),
        ],
      ],
    );
  }

  /// Last and apart, since it can't be undone: the account site asks once
  /// more before it deletes anything.
  Widget _deleteAccount(BuildContext context, DeviceAccount account) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final button = Button(
      label: 'Delete account…',
      onPressed: () => openDeleteAccountPage(account.service),
    );
    final words = Text(
      'Deletes your email and everything linked to it at '
      '${accountSiteName(account.service)}, after asking.',
      style: type.caption.copyWith(color: palette.textFaint),
    );
    if (isPhoneLayout(context)) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          button,
          const SizedBox(height: Space.s),
          words,
        ],
      );
    }
    return Row(
      children: [
        button,
        const SizedBox(width: Space.m),
        Expanded(child: words),
      ],
    );
  }
}

/// One of the account's Claktories: its tile, name, and whether it's
/// online. [status] says this app shows it already ("Connected"); a row
/// that can't be reached or is shown already isn't pressable.
class _AccountClaktoryRow extends StatelessWidget {
  const _AccountClaktoryRow({
    required this.claktory,
    required this.status,
    required this.now,
    required this.onTap,
  });

  final AccountClaktory claktory;
  final String? status;
  final DateTime now;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final last = claktory.lastContact;
    final where = [
      if (claktory.machine.isNotEmpty && claktory.machine != claktory.name)
        claktory.machine,
      if (!claktory.reachable)
        "Can't be reached yet"
      else if (claktory.online(now))
        'Online'
      else if (last != null)
        'Last online ${formatAge(last.millisecondsSinceEpoch, now)}'
      else
        'Offline',
    ].join(' · ');
    final enabled = status == null && claktory.reachable;
    final phone = isPhoneLayout(context);
    return Semantics(
      button: enabled,
      label: claktory.name,
      child: Pressable(
        onTap: enabled ? onTap : null,
        builder: (context, hovered) => Container(
          // On a phone it grows with its text.
          height: phone ? null : 52,
          constraints: phone
              ? const BoxConstraints(minHeight: Touch.row)
              : null,
          padding: EdgeInsets.symmetric(
            horizontal: Space.s,
            vertical: phone ? Space.xs : 0,
          ),
          decoration: BoxDecoration(
            color: hovered && enabled ? palette.hover : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Row(
            children: [
              AccountClaktoryTile(claktory: claktory),
              const SizedBox(width: Space.m),
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      claktory.name,
                      maxLines: phone ? null : 1,
                      overflow: phone ? null : TextOverflow.ellipsis,
                      style: type.body.copyWith(
                        color: palette.text,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                    Text(
                      where,
                      maxLines: phone ? null : 1,
                      overflow: phone ? null : TextOverflow.ellipsis,
                      style: type.caption.copyWith(color: palette.textFaint),
                    ),
                  ],
                ),
              ),
              if (status != null)
                Text(
                  status!,
                  style: type.caption.copyWith(color: palette.textFaint),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A Claktory's tile, as the rail draws it: its initials on a wash of its
/// color.
class AccountClaktoryTile extends StatelessWidget {
  const AccountClaktoryTile({super.key, required this.claktory});

  final AccountClaktory claktory;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final own = agentColors[claktory.color];
    return Container(
      width: 28,
      height: 28,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: own?.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(Radii.control),
        border: own == null ? Border.all(color: palette.hairline) : null,
      ),
      child: Text(
        claktoryInitials(claktory.name),
        style: TextStyles.caption.copyWith(
          color: own ?? palette.textMuted,
          fontWeight: FontWeight.w600,
          height: 1,
        ),
      ),
    );
  }
}

/// [day] as the person reads a deadline: "today", "tomorrow", "27 Nov".
String deadlineDay(DateTime day, DateTime now) {
  final today = DateTime(now.year, now.month, now.day);
  final date = DateTime(day.year, day.month, day.day);
  if (!date.isAfter(today)) return 'today';
  if (date == DateTime(today.year, today.month, today.day + 1)) {
    return 'tomorrow';
  }
  const months = [
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
  ];
  final label = '${date.day} ${months[date.month - 1]}';
  return date.year == now.year ? label : '$label ${date.year}';
}

/// Signing this device in, in a dialog of its own, with [title] and
/// [intro]; it closes once signed in.
class DeviceSignInDialog extends StatefulWidget {
  const DeviceSignInDialog({
    super.key,
    required this.title,
    required this.intro,
    required this.close,
  });

  final String title;
  final String intro;
  final VoidCallback close;

  @override
  State<DeviceSignInDialog> createState() => _DeviceSignInDialogState();
}

class _DeviceSignInDialogState extends State<DeviceSignInDialog> {
  DeviceAccount? _account;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final account = AppScope.scope(context).connections.account;
    if (account == _account) return;
    _account?.removeListener(_changed);
    _account = account..addListener(_changed);
  }

  @override
  void dispose() {
    _account?.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (_account?.signedIn ?? false) widget.close();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    // Scrolls, and Cancel goes under the title, where the text is large
    // and the screen small.
    return SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Wrap(
            alignment: WrapAlignment.spaceBetween,
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: Space.s,
            runSpacing: Space.s,
            children: [
              Text(
                widget.title,
                style: type.emphasis.copyWith(color: palette.text),
              ),
              Button(label: 'Cancel', onPressed: widget.close),
            ],
          ),
          const SizedBox(height: Space.l),
          DeviceSignInForm(intro: widget.intro),
        ],
      ),
    );
  }
}

/// Opens [DeviceSignInDialog] to confirm this device is the owner's on
/// [name]: once signed in, it confirms by itself.
Future<void> showConfirmSignIn(BuildContext context, String name) => showModal(
  context,
  (context, close) => DeviceSignInDialog(
    title: 'Sign in to keep using $name',
    intro:
        'Use the email $name is signed in with. Your '
        "$thisDeviceKind stays connected, and you won't be asked again.",
    close: close,
  ),
);

/// A Claktory this device connects to wants it to confirm it's the
/// owner's ([Backend.confirmBy]) and the device isn't signed in to do it
/// by itself: by when, and Sign in…. Nothing otherwise.
class ConfirmDeviceBanner extends StatelessWidget {
  const ConfirmDeviceBanner({super.key});

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final account = AppScope.scope(context).connections.account;
    return ListenableBuilder(
      listenable: Listenable.merge([backend.confirmBy, account]),
      builder: (context, _) {
        final by = backend.confirmBy.value;
        if (by == null || account.signedIn) return const SizedBox.shrink();
        final palette = Palette.of(context);
        final type = TypeScale.of(context);
        final name = claktoryName(backend);
        // A bar over the window: its words grow only to 2×, so at the
        // largest text it doesn't take the window (its sign-in, a dialog
        // of its own, grows in full).
        return MediaQuery.withClampedTextScaling(
          maxScaleFactor: 2,
          child: Container(
            padding: const EdgeInsets.symmetric(
              horizontal: Space.l,
              vertical: Space.s,
            ),
            decoration: BoxDecoration(
              border: Border(bottom: BorderSide(color: palette.hairline)),
            ),
            child: _BannerLine(
              words: Text(
                'Sign in to your Claktory account in this app by '
                '${deadlineDay(by, DateTime.now())} to keep using $name.',
                style: type.body.copyWith(color: palette.text),
              ),
              actions: [
                Button(
                  label: 'Sign in…',
                  primary: true,
                  onPressed: () => showConfirmSignIn(context, name),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

/// This Claktory is signed in to another Claktory account than this device
/// (ERROR_CODE_OTHER_ACCOUNT, #529 S4): refused at connect, it says to open
/// a pipe instead; a device it let in before, now signed in to another
/// account, to sign in with the account it uses. Its own words (Trish's,
/// from the backend), Try again, and Remove… to take it off this device
/// (the rail's own question). Never a way around it (Fugo): no allow
/// anyway, no sign out to continue.
class OtherAccountView extends StatefulWidget {
  const OtherAccountView({super.key});

  @override
  State<OtherAccountView> createState() => _OtherAccountViewState();
}

class _OtherAccountViewState extends State<OtherAccountView> {
  DeviceAccount? _account;
  var _wasSignedIn = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final account = AppScope.scope(context).connections.account;
    if (account == _account) return;
    _account?.removeListener(_changed);
    _account = account..addListener(_changed);
    _wasSignedIn = account.signedIn;
  }

  @override
  void dispose() {
    _account?.removeListener(_changed);
    super.dispose();
  }

  /// Signed in again (with the account the Claktory uses, by the sheet
  /// below): it tries again.
  void _changed() {
    final signedIn = _account?.signedIn ?? false;
    if (signedIn && !_wasSignedIn && mounted) AppScope.of(context).retry();
    _wasSignedIn = signedIn;
  }

  /// Signing in with the account [name] uses: this device signs out of
  /// its own first, through the sign-out sheet that says what that means
  /// (MC5), then signs in (Squalo's V2).
  Future<void> _signInWithTheirs(String name) async {
    final account = _account!;
    if (account.signedIn) {
      await signOutOfAccount(context, account);
      if (account.signedIn || !mounted) return;
    }
    await showModal(
      context,
      (context, close) => DeviceSignInDialog(
        title: 'Sign in with the account $name uses',
        intro:
            'Use the email $name is signed in with, and $name lets this '
            '$thisDeviceKind in again.',
        close: close,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final account = _account!;
    return ListenableBuilder(
      listenable: Listenable.merge([backend.otherAccount, account]),
      builder: (context, _) {
        final said = backend.otherAccount.value ?? '';
        final name = claktoryName(backend);
        // A device it let in before says it's "now signed in to a
        // different" account; one refused at connect, to open a pipe.
        final before = said.contains('is now signed in to a different');
        final masked = account.signedIn ? maskedEmail(account.email) : '';
        final claktory = ClaktoriesScope.of(context)?.all
            .where((claktory) => claktory.backend == backend)
            .firstOrNull;
        final buttons = [
          if (claktory != null)
            Button(
              label: 'Remove…',
              onPressed: () =>
                  unawaited(confirmRemoveClaktory(context, claktory)),
            ),
          if (account.signedIn || before)
            Button(
              label: 'Sign in…',
              onPressed: () => unawaited(_signInWithTheirs(name)),
            ),
          Button(label: 'Try again', primary: true, onPressed: backend.retry),
        ];
        final muted = type.body.copyWith(color: palette.textMuted);
        return Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(Space.xl),
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 420),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(
                    before
                        ? '$name no longer lets this $thisDeviceKind in'
                        : '$name is on another Claktory account',
                    style: type.title.copyWith(color: palette.text),
                  ),
                  if (said.isNotEmpty) ...[
                    const SizedBox(height: Space.s),
                    Text(said, style: muted),
                  ],
                  // Which account this device is on, so "the account it
                  // uses" means something (Squalo's V2).
                  if (masked.isNotEmpty) ...[
                    const SizedBox(height: Space.s),
                    Text(
                      'This $thisDeviceKind is signed in as $masked.',
                      style: muted,
                    ),
                  ],
                  // Refused at connect: it may be the person's own, signed
                  // in with their other email (Squalo's V3): moved to this
                  // device's account in the app, never by commands, which
                  // only the person may run (Fugo 46047, #546 A).
                  if (!before) ...[
                    const SizedBox(height: Space.m),
                    Text(
                      'If $name is yours, sign it in to this '
                      "$thisDeviceKind's account instead, in Settings → "
                      'Account on $name. Then try again here.',
                      style: muted,
                    ),
                    const SizedBox(height: Space.s),
                    Text(
                      'No screen on $name? Choose Sign in… to use '
                      "$name's account on this $thisDeviceKind for now, then "
                      'sign $name in to yours from here, in its Settings → '
                      'Account.',
                      style: muted,
                    ),
                  ],
                  const SizedBox(height: Space.xl),
                  // On a phone, one a row, Try again at the bottom, nearest
                  // the thumb; elsewhere together at the end.
                  if (isPhoneLayout(context))
                    for (final (i, button) in buttons.indexed) ...[
                      if (i > 0) const SizedBox(height: Space.s),
                      button,
                    ]
                  else
                    Align(
                      alignment: AlignmentDirectional.centerEnd,
                      child: ControlsLine(children: buttons),
                    ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

/// This Claktory lets this device in only once it shows it's the owner's
/// (ERROR_CODE_SIGN_IN_REQUIRED): its words, and the sign-in, after which
/// it connects again. [onCancel], when given, gives up on it (one found
/// through the account).
class SignInRequiredView extends StatefulWidget {
  const SignInRequiredView({super.key, this.onCancel});

  final VoidCallback? onCancel;

  @override
  State<SignInRequiredView> createState() => _SignInRequiredViewState();
}

class _SignInRequiredViewState extends State<SignInRequiredView> {
  DeviceAccount? _account;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final account = AppScope.scope(context).connections.account;
    if (account == _account) return;
    _account?.removeListener(_changed);
    _account = account..addListener(_changed);
  }

  @override
  void dispose() {
    _account?.removeListener(_changed);
    super.dispose();
  }

  var _wasSignedIn = false;

  /// Signed in: it tries again.
  void _changed() {
    final signedIn = _account?.signedIn ?? false;
    if (signedIn && !_wasSignedIn && mounted) AppScope.of(context).retry();
    _wasSignedIn = signedIn;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final account = _account!;
    _wasSignedIn = account.signedIn;
    return ListenableBuilder(
      listenable: Listenable.merge([backend.signInRequired, account]),
      builder: (context, _) {
        final name = claktoryName(backend);
        final said = backend.signInRequired.value ?? '';
        // The Claktory itself was signed out of the account (friend bug
        // #38): it's the one to sign in again, in its own Settings, as
        // the backend's sentence says, not this device (Trish 35985).
        final claktorySignedOut = said.contains(
          'was signed out of your Claktory account',
        );
        final signOut = Button(
          label: 'Sign out',
          onPressed: () => unawaited(signOutOfAccount(context, account)),
        );
        final retry = Button(
          label: 'Try again',
          primary: true,
          onPressed: backend.retry,
        );
        // Gives up on it: the rail's own Remove question, never at once.
        final cancel = Button(label: 'Remove…', onPressed: widget.onCancel);
        return Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(Space.xl),
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 420),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(
                    claktorySignedOut
                        ? '$name was signed out'
                        : 'Sign in to connect to $name',
                    style: type.title.copyWith(color: palette.text),
                  ),
                  if (claktorySignedOut) ...[
                    // Its Settings → Account is on the Claktory this device
                    // can't open now. With no screen there, factoryd pair in
                    // its terminal (offline, the digits typed there) lets
                    // this device in again by code, to sign it in from here
                    // (#529 MC4; Fugo 46047, Giorno 46058: the account
                    // commands are the person's alone, never a terminal's).
                    const SizedBox(height: Space.s),
                    Text(
                      'Sign $name in to your Claktory account again, in '
                      'Settings → Account on $name. Then try again here.',
                      style: type.body.copyWith(color: palette.textMuted),
                    ),
                    const SizedBox(height: Space.s),
                    Text(
                      'No screen on $name? Run factoryd pair in a terminal '
                      'there and connect this $thisDeviceKind with the code it '
                      'shows. Then sign $name in from here, in its Settings → '
                      'Account.',
                      style: type.body.copyWith(color: palette.textMuted),
                    ),
                  ] else if (said.isNotEmpty) ...[
                    const SizedBox(height: Space.s),
                    Text(
                      said,
                      style: type.body.copyWith(color: palette.textMuted),
                    ),
                  ],
                  const SizedBox(height: Space.xl),
                  if (claktorySignedOut)
                    Align(alignment: Alignment.centerRight, child: retry)
                  else if (account.signedIn) ...[
                    Text(
                      'Signed in as ${account.email}.',
                      style: type.body.copyWith(color: palette.text),
                    ),
                    const SizedBox(height: Space.l),
                    if (isPhoneLayout(context))
                      StepButtons(children: [retry, signOut])
                    else
                      Wrap(
                        alignment: WrapAlignment.end,
                        spacing: Space.s,
                        children: [signOut, retry],
                      ),
                  ] else
                    DeviceSignInForm(
                      intro: signInWithIntro(
                        name,
                        backend.workspace.workspace.account,
                      ),
                    ),
                  if (widget.onCancel != null) ...[
                    const SizedBox(height: Space.l),
                    // On a phone, as wide as the buttons above it.
                    if (isPhoneLayout(context))
                      StepButtons(children: [cancel])
                    else
                      Align(alignment: Alignment.centerLeft, child: cancel),
                  ],
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

/// Which email to sign in with, for [name]: its own, masked
/// ("m…@gmail.com"), when it said it's signed in with one; else the words
/// without it (#529, MC13, Trish's words).
String signInWithIntro(String name, ClaktoryAccount account) {
  final email =
      account.state == ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN
      ? maskedEmail(account.email)
      : '';
  return email.isEmpty
      ? 'Sign in with the email $name is signed in with.'
      : 'Sign in with $email, the email $name is signed in with.';
}

/// [email] with its name hidden but its first letter: "m…@gmail.com";
/// empty when it isn't one.
String maskedEmail(String email) {
  final at = email.lastIndexOf('@');
  if (at < 1 || at == email.length - 1) return '';
  return '${email.characters.first}…${email.substring(at)}';
}

/// A device that found this Claktory through the person's account and
/// waits to be let in (PendingDevice.discovered): its name, whether it's
/// signed in as them, the number to compare, and Allow or Don't allow.
/// Nothing while none waits.
class DiscoveredDeviceBanner extends StatelessWidget {
  const DiscoveredDeviceBanner({super.key});

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        final device = store.workspace.remote.pendingDevices
            .where((device) => device.discovered)
            .firstOrNull;
        if (device == null) return const SizedBox.shrink();
        // Each request afresh, even from the same device asking again:
        // nothing typed for the last one carries over (Zucchero's row 264).
        return _DiscoveredDevice(
          key: ValueKey(
            '${device.deviceId}/${device.number}/${device.requestedMs}',
          ),
          device: device,
        );
      },
    );
  }
}

/// One request of [DiscoveredDeviceBanner]'s.
class _DiscoveredDevice extends StatefulWidget {
  const _DiscoveredDevice({super.key, required this.device});

  final PendingDevice device;

  @override
  State<_DiscoveredDevice> createState() => _DiscoveredDeviceState();
}

class _DiscoveredDeviceState extends State<_DiscoveredDevice> {
  /// The number typed off its screen (#352 P4).
  final _number = TextEditingController();
  var _deciding = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _number.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _number.dispose();
    super.dispose();
  }

  Future<void> _decide(PendingDevice device, {required bool allow}) async {
    setState(() {
      _deciding = true;
      _error = null;
    });
    final error = await decideDevice(
      AppScope.of(context),
      device,
      allow: allow,
      number: _number.text,
    );
    if (!mounted) return;
    setState(() {
      _deciding = false;
      _error = error;
      _number.clear();
    });
  }

  @override
  Widget build(BuildContext context) {
    final device = widget.device;
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final ready = !_deciding && isDeviceNumber(_number.text);
    return Container(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.l,
        vertical: Space.s,
      ),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      child: _BannerLine(
        words: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              device.signedIn
                  ? '${deviceCallingItself(device)}, signed in as you, '
                        'wants to connect.'
                  : '${deviceCallingItself(device)} wants to connect.',
              style: type.body.copyWith(color: palette.text),
            ),
            Text(
              'To allow it, type the number its screen shows. If you '
              "didn't just set up a new device, don't allow it. Someone "
              'may be using your email: secure it.',
              style: type.caption.copyWith(color: palette.textMuted),
            ),
            if (_error != null)
              Text(
                _error!,
                style: type.caption.copyWith(color: palette.danger),
              ),
          ],
        ),
        actions: [
          SizedBox(
            width: DeviceNumberField.widthFor(context),
            child: DeviceNumberField(
              controller: _number,
              onSubmitted: (_) {
                if (ready) _decide(device, allow: true);
              },
            ),
          ),
          Button(
            label: "Don't allow",
            onPressed: _deciding ? null : () => _decide(device, allow: false),
          ),
          Button(
            label: 'Allow',
            primary: true,
            onPressed: ready ? () => _decide(device, allow: true) : null,
          ),
        ],
      ),
    );
  }
}

/// A banner's words with its buttons at the end of the line; on a phone,
/// where they don't fit beside the words, under them at the end.
class _BannerLine extends StatelessWidget {
  const _BannerLine({required this.words, required this.actions});

  final Widget words;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    if (isPhoneLayout(context)) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          words,
          const SizedBox(height: Space.s),
          Wrap(
            alignment: WrapAlignment.end,
            spacing: Space.s,
            runSpacing: Space.s,
            children: actions,
          ),
        ],
      );
    }
    return Row(
      children: [
        Expanded(child: words),
        for (final (i, action) in actions.indexed) ...[
          SizedBox(width: i == 0 ? Space.m : Space.s),
          action,
        ],
      ],
    );
  }
}

/// Whether a device banner shows now: a device found through the account
/// waits, or this one has to confirm and isn't signed in.
bool _bannersShow(Backend backend, DeviceAccount account) =>
    (backend.confirmBy.value != null && !account.signedIn) ||
    backend.workspace.workspace.remote.pendingDevices.any(
      (device) => device.discovered,
    );

/// While the keyboard is up, the device banners step aside when it leaves
/// less than this above it: room for a header, some of the conversation
/// and the message box.
const bannersNeedAbove = 480.0;

/// [child] under what this device's account needs from the person now:
/// a device to let in ([DiscoveredDeviceBanner]) and this device to confirm
/// ([ConfirmDeviceBanner]). The banners take the screen's top inset (a
/// phone's status bar) while they show, and [child] then starts under them.
class DeviceBanners extends StatelessWidget {
  const DeviceBanners({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final account = AppScope.scope(context).connections.account;
    return ListenableBuilder(
      listenable: Listenable.merge([
        backend.confirmBy,
        account,
        backend.workspace,
      ]),
      // The same tree whether they show or not, so what's under them
      // keeps its state as one comes and goes.
      builder: (context, _) {
        // Typing where the keyboard leaves little room (a phone on its
        // side, a small one upright): they step aside until it's down, so
        // the message box and what's above it stay in sight (Sale, row 22).
        final size = MediaQuery.sizeOf(context);
        final keyboard = MediaQuery.viewInsetsOf(context).bottom;
        final aside =
            keyboard > 0 &&
            size.height - keyboard < bannersNeedAbove &&
            isPhoneLayout(context);
        final shown = _bannersShow(backend, account) && !aside;
        final safe = MediaQuery.paddingOf(context);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Container(
              color: shown && isPhoneLayout(context)
                  ? Palette.of(context).canvas
                  : null,
              padding: shown
                  ? EdgeInsets.only(
                      top: safe.top,
                      left: safe.left,
                      right: safe.right,
                    )
                  : EdgeInsets.zero,
              child: Visibility(
                visible: !aside,
                maintainState: true,
                child: const Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [DiscoveredDeviceBanner(), ConfirmDeviceBanner()],
                ),
              ),
            ),
            Expanded(
              child: MediaQuery.removePadding(
                context: context,
                removeTop: shown,
                child: child,
              ),
            ),
          ],
        );
      },
    );
  }
}
