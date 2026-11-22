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
import 'backend.dart' show Backend, RequestFailure;
import 'claktory_identity.dart' show claktoryName;
import 'connections.dart';
import 'controls.dart';
import 'device_account.dart';
import 'device_kind.dart' show thisDeviceName;
import 'devices.dart' show thisDeviceKind;
import 'identity.dart';
import 'mobile/drawer.dart' show largeText;
import 'mobile/input.dart' show InputKind;
import 'mobile/metrics.dart' show Touch, TypeScale;
import 'name_step.dart' show StepButtons, stepFieldScrollPadding;
import 'protocol/factory/v1/factory.pb.dart';
import 'rail.dart' show Claktories, ClaktoriesScope, claktoryInitials;
import 'scope.dart';
import 'usage.dart' show formatAge;

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

/// Signs this device in with an emailed code: its email, then the code.
/// [intro] says why, above the email field.
class DeviceSignInForm extends StatefulWidget {
  const DeviceSignInForm({super.key, required this.intro});

  final String intro;

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
    final email = _email.text.trim();
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
          onPressed: _email.text.trim().isEmpty || _busy ? null : _sendCode,
        );
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              widget.intro,
              style: type.body.copyWith(color: palette.textMuted),
            ),
            const SizedBox(height: Space.l),
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
        final different = Button(
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
              StepButtons(children: [continueButton, ?newCode, different])
            else
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(
                    child: Wrap(
                      spacing: Space.xs,
                      runSpacing: Space.xs,
                      children: [?newCode, different],
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
      onPressed: () => account.signOut(),
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
          Text(
            'No Claktories on this account yet. Sign in to it on the Mac or '
            'server that runs Claktory, in Settings → Account.',
            style: type.body.copyWith(color: palette.textMuted),
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
    title: "Confirm this $thisDeviceKind is yours",
    intro:
        'Sign in with the email $name is signed in with. Your '
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
                'Confirm this $thisDeviceKind is yours by '
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
        final signOut = Button(
          label: 'Sign out',
          onPressed: () => account.signOut(),
        );
        final retry = Button(
          label: 'Try again',
          primary: true,
          onPressed: backend.retry,
        );
        final cancel = Button(label: 'Cancel', onPressed: widget.onCancel);
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
                    'Sign in to connect to $name',
                    style: type.title.copyWith(color: palette.text),
                  ),
                  if (said.isNotEmpty) ...[
                    const SizedBox(height: Space.s),
                    Text(
                      said,
                      style: type.body.copyWith(color: palette.textMuted),
                    ),
                  ],
                  const SizedBox(height: Space.xl),
                  if (account.signedIn) ...[
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
                      intro: 'Sign in with the email $name is signed in with.',
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

/// A device that found this Claktory through the person's account and
/// waits to be let in (PendingDevice.discovered): its name, whether it's
/// signed in as them, the number to compare, and Allow or Don't allow.
/// Nothing while none waits.
class DiscoveredDeviceBanner extends StatefulWidget {
  const DiscoveredDeviceBanner({super.key});

  @override
  State<DiscoveredDeviceBanner> createState() => _DiscoveredDeviceBannerState();
}

class _DiscoveredDeviceBannerState extends State<DiscoveredDeviceBanner> {
  var _deciding = false;
  String? _error;

  Future<void> _decide(PendingDevice device, {required bool allow}) async {
    final backend = AppScope.of(context);
    setState(() {
      _deciding = true;
      _error = null;
    });
    try {
      if (allow) {
        await backend.approveDevice(device.deviceId);
      } else {
        await backend.denyDevice(device.deviceId);
      }
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(
          () => _error = failure.code == ErrorCode.ERROR_CODE_NOT_FOUND
              ? '${device.name} stopped waiting.'
              : failure.message,
        );
      }
    } finally {
      if (mounted) setState(() => _deciding = false);
    }
  }

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
        final palette = Palette.of(context);
        final type = TypeScale.of(context);
        final name = device.name.isEmpty ? 'A device' : device.name;
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
                      ? '$name, signed in as you, wants to connect.'
                      : '$name wants to connect.',
                  style: type.body.copyWith(color: palette.text),
                ),
                if (device.number.isNotEmpty)
                  Text(
                    'Allow it only if its screen shows ${device.number}.',
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
              Button(
                label: "Don't allow",
                onPressed: _deciding
                    ? null
                    : () => _decide(device, allow: false),
              ),
              Button(
                label: 'Allow',
                primary: true,
                onPressed: _deciding
                    ? null
                    : () => _decide(device, allow: true),
              ),
            ],
          ),
        );
      },
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
        final shown = _bannersShow(backend, account);
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
              child: const Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [DiscoveredDeviceBanner(), ConfirmDeviceBanner()],
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
