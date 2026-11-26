import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../device_account.dart' show DeviceAccount;
import '../devices.dart' show QrCode, pairedOn, thisDeviceKind;
import '../extensions.dart' show dayLabel;
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../message_time.dart' show clockTime;
import '../mobile/lines.dart' show ActionLine, ControlsLine;
import '../mobile/metrics.dart' show TypeScale;
import '../scanner.dart';
import '../scope.dart';
import '../tunnel/api/dm.dart' show DmHandoffSealed;
import '../tunnel/api/dm_transfer.dart';
import '../visible_codes.dart' show proseCodes;
import '../your_claktories.dart' show DeviceSignInForm;
import 'e2ee.dart';
import '../user_apps/session.dart' show UserAppError, UserAppProblem;
import 'secret_clipboard.dart';
import 'chat_view.dart' show shownClaktoryName;

// E5: People's part for end-to-end messages on this device (dms-e2ee §2,
// §5.3; Trish's words): set up messages on the person's first device; on
// another, its QR code for a set-up device to scan, and waiting; once set
// up, the person's devices, adding one by scanning its code and removing
// one. Messages off on a build that can't keep the keys safely, and what
// went wrong when a key can't be read or the account's key was replaced.

/// Where a new device's code is copied and pasted; tests give their own.
SecretClipboard secretClipboard = const SecretClipboard();

/// Messages on this device, in People: what the person needs to do, at
/// the top; or, with [devices], their devices once set up, further down.
class MessagesSetup extends StatelessWidget {
  const MessagesSetup({super.key, required this.e2ee, this.devices = false});

  final E2ee e2ee;

  /// Only Your devices, and only once set up.
  final bool devices;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: e2ee,
    builder: (context, _) => switch (e2ee.setup) {
      DmSetup.ready when devices => Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _Devices(e2ee: e2ee),
          _Home(e2ee: e2ee),
        ],
      ),
      _ when devices => const SizedBox.shrink(),
      // Taken from another device: said once (Fugo M2).
      DmSetup.ready when e2ee.newKeyFrom != null => _Note(
        title: 'Your security key changed',
        'A new security key was made for your account on '
        '${e2ee.newKeyFrom!.isEmpty ? 'another device' : e2ee.newKeyFrom}, '
        "after a lost or stolen device was removed. If that wasn't you, "
        'secure your email and tell your friends.',
        action: Button(label: 'OK', onPressed: e2ee.dismissNewKey),
      ),
      DmSetup.ready when e2ee.finishingKey => const _Note(
        'Finishing your new security key…',
      ),
      DmSetup.ready when e2ee.transfer != null => _Transfer(e2ee: e2ee),
      DmSetup.starting ||
      DmSetup.signedOut ||
      DmSetup.ready => const SizedBox.shrink(),
      DmSetup.unavailable => _Note(
        "Messages can't be used in this version of Claktory, because it "
        "can't keep your security key safe on this Mac. Use the Claktory "
        'app, or another of your devices.',
      ),
      DmSetup.firstDevice => _SetUp(e2ee: e2ee),
      DmSetup.addThisDevice => _AddThis(e2ee: e2ee),
      DmSetup.failed => _Failed(e2ee: e2ee),
    },
  );
}

/// Where the person's messages are kept (#451): the home Claktory their
/// list names, and Keep them on this Claktory when it's another; and that
/// friends' Claktories see its network address (Fugo).
class _Home extends StatelessWidget {
  const _Home({required this.e2ee});

  final E2ee e2ee;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final named = e2ee.homeEndpoint?.isNotEmpty ?? false;
    final here = e2ee.homeIsHere;
    final String where;
    if (e2ee.ownership == DmOwnership.notOurs) {
      // Not on the person's account (Fugo 39894): given nothing.
      where =
          "This Claktory is signed in to another account, so your messages "
          "aren't kept on it. What you send waits until you connect to one "
          'of your own.';
    } else if (e2ee.ownership == DmOwnership.unconfirmable) {
      // Paired with a code from before the private tunnel (Fugo 40182).
      where =
          "Your messages aren't kept on ${shownClaktoryName(context)}: it "
          "was added with an older code that can't confirm it's yours. Add "
          'it again with a new code to keep them there.';
    } else if (e2ee.ownership == DmOwnership.unchecked) {
      where =
          "Can't check that this Claktory is yours right now, so your "
          'messages wait. Claktory tries again by itself.';
    } else if (!e2ee.homeIsOurs) {
      where = 'Checking that this Claktory is yours…';
    } else if (here) {
      where = 'On the Claktory this app is connected to.';
    } else if (named) {
      where =
          'On another of your Claktories. What you send waits until you '
          'connect to it, unless you keep them on this one.';
    } else {
      where =
          'Not chosen yet. Connect to one of your Claktories to keep '
          'them there.';
    }
    return Field(
      label: 'Where your messages are kept',
      note:
          "Your Claktory connects to your friends' Claktories directly when "
          'it can, so they can see its network address.',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(where, style: type.body.copyWith(color: palette.textMuted)),
          // Another Claktory is the home, and the one shown could be.
          if (e2ee.homeIsOurs && !here && named && e2ee.homeState != null) ...[
            const SizedBox(height: Space.s),
            Button(
              label: 'Keep them on this Claktory',
              onPressed: () => unawaited(e2ee.makeHomeHere()),
            ),
          ],
        ],
      ),
    );
  }
}

class _Note extends StatelessWidget {
  const _Note(this.text, {this.title, this.danger = false, this.action});

  final String? title;
  final String text;
  final bool danger;
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (title case final title?) ...[
            Text(
              title,
              style: type.emphasis.copyWith(
                color: danger ? palette.danger : palette.text,
              ),
            ),
            const SizedBox(height: Space.xs),
          ],
          if (text.isNotEmpty)
            Text(text, style: type.body.copyWith(color: palette.textMuted)),
          if (action case final action?) ...[
            const SizedBox(height: Space.s),
            action,
          ],
        ],
      ),
    );
  }
}

class _SetUp extends StatefulWidget {
  const _SetUp({required this.e2ee});

  final E2ee e2ee;

  @override
  State<_SetUp> createState() => _SetUpState();
}

class _SetUpState extends State<_SetUp> {
  var _busy = false;

  Future<void> _setUp() async {
    setState(() => _busy = true);
    await widget.e2ee.setUpFirst();
    if (mounted) setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) => _Note(
    title: 'Set up messages',
    'Messages with your friends are end-to-end encrypted: only your '
    'devices and theirs can read them, plus any Claktory either of you '
    'gives a chat to, where the agents you allow can read it too. '
    "Claktory's servers can't read what you write, but they do see who you "
    'talk to, when and roughly how much, your devices, your name and '
    'picture, and any messages included in a report. Your friends see the '
    'names of your devices. Until you verify a friend, you\'re trusting '
    'Claktory that their security key is really theirs.\n\n'
    'Your messages are locked with a security key that stays on your '
    'devices. Set up messages on more than one device: if you lose them '
    'all, your messages are gone, and your friends see your security key '
    'change.',
    action: Button(
      label: 'Set up',
      primary: true,
      onPressed: _busy ? null : _setUp,
    ),
  );
}

/// This device isn't added yet: its QR code, for one of the person's
/// set-up devices to scan.
class _AddThis extends StatefulWidget {
  const _AddThis({required this.e2ee});

  final E2ee e2ee;

  @override
  State<_AddThis> createState() => _AddThisState();
}

class _AddThisState extends State<_AddThis> {
  /// Starting over with a new security key ([E2ee.startNewKey]).
  var _starting = false;
  String? _startOverError;

  /// The new key waits for a fresh sign-in (Sign in again…).
  var _signInFirst = false;

  Future<void> _noOtherDevice() async {
    var sure = false;
    await showModal(
      context,
      (context, close) => FormFrame(
        title: 'Start over on this $thisDeviceKind?',
        body:
            'This $thisDeviceKind gets a new security key and starts with no '
            'messages. Your earlier messages stay with your friends.\n\n'
            'Your friends see that your security key changed, and anyone '
            'who verified you will need to again.\n\n'
            'Any other device that still has your old key stops getting '
            'messages until you add it again.',
        fields: const [],
        error: null,
        submitLabel: 'Start over',
        onSubmit: () {
          sure = true;
          close();
        },
        onCancel: close,
      ),
    );
    if (sure) await _startOver();
  }

  /// Signed in again: then the new key, which the person already chose.
  Future<void> _signInThenStartOver() async {
    if (!await _signInAgain(
          context,
          'For your security, sign in again to start over.',
        ) ||
        !mounted) {
      return;
    }
    setState(() => _signInFirst = false);
    await _startOver();
  }

  Future<void> _startOver() async {
    setState(() {
      _starting = true;
      _startOverError = null;
    });
    final stop = await widget.e2ee.startNewKey();
    if (!mounted) return;
    setState(() => _starting = false);
    switch (stop) {
      case null:
        return;
      case DmStartOverStop.signInAgain:
        // Signed in long ago (a found device, say): a fresh sign-in first.
        setState(() => _signInFirst = true);
      case DmStartOverStop.failed:
        setState(() => _startOverError = "That didn't work. Try again.");
    }
  }

  /// Whether this device can copy its code safely; until then, and where
  /// it can't, no Copy code.
  var _canCopy = false;

  /// The code copied, and the token to clear it with.
  String? _copied;
  int? _token;

  /// A code works for 30 minutes (the service keeps a hand-off no longer):
  /// then a new one.
  Timer? _renew;

  @override
  void initState() {
    super.initState();
    _offer();
    unawaited(
      secretClipboard.available.then((can) {
        if (mounted) setState(() => _canCopy = can);
      }),
    );
  }

  @override
  void dispose() {
    _renew?.cancel();
    // Added, or gone from here: the code is no use to anyone now.
    _forget();
    super.dispose();
  }

  void _forget() {
    if (_token case final token?) unawaited(secretClipboard.clear(token));
    _token = null;
    _copied = null;
  }

  Future<void> _copy(String code) async {
    _forget();
    final token = await secretClipboard.copy(code);
    if (!mounted) {
      if (token != null) unawaited(secretClipboard.clear(token));
      return;
    }
    setState(() {
      _token = token;
      _copied = token == null ? null : code;
    });
  }

  @override
  void didUpdateWidget(_AddThis old) {
    super.didUpdateWidget(old);
    _offer();
  }

  void _offer() {
    // A spent code copied is cleared, and Copy code is back.
    if (_copied != null && _copied != widget.e2ee.offer) _forget();
    // A new one when the last was spent (a hand-off that didn't open) or
    // is too old.
    if (_renew != null && widget.e2ee.offer != null) return;
    final age = widget.e2ee.offerAge;
    const life = Duration(seconds: SecretClipboard.seconds);
    if (widget.e2ee.offer != null && age < life) {
      _renewIn(life - age);
      return;
    }
    _renewIn(life);
    unawaited(widget.e2ee.offerThisDevice(platform: thisDeviceKind));
  }

  void _renewIn(Duration after) {
    _renew?.cancel();
    _renew = Timer(after, () {
      _renew = null;
      _forget();
      _make();
    });
  }

  /// A new code, and the old one forgotten.
  void _make() {
    if (!mounted) return;
    _renewIn(const Duration(seconds: SecretClipboard.seconds));
    unawaited(widget.e2ee.offerThisDevice(platform: thisDeviceKind));
    setState(() {});
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final code = widget.e2ee.offer;
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Add this $thisDeviceKind to your messages',
            style: type.emphasis.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.xs),
          Text(
            'On a device where you already use messages, open People, '
            'choose Add a device… under Your devices, and scan this code.',
            style: type.body.copyWith(color: palette.textMuted),
          ),
          // A hand-off came for a code this one replaced (its app closed
          // before it was added, say): the other device's add didn't take
          // (row 564).
          if (widget.e2ee.earlierCodeSpent) ...[
            const SizedBox(height: Space.s),
            Text(
              "Your earlier code no longer works, so your other device "
              "couldn't add this $thisDeviceKind. Add it again with this "
              'new code.',
              style: type.body.copyWith(color: palette.danger),
            ),
          ],
          const SizedBox(height: Space.m),
          Center(
            child: code == null
                ? const SizedBox.square(dimension: 200)
                : QrCode(key: ValueKey(code), text: code, size: 200),
          ),
          if (code == null && widget.e2ee.offerFailed) ...[
            const SizedBox(height: Space.s),
            ActionLine(
              lead: Text(
                "Couldn't make a code.",
                style: type.body.copyWith(color: palette.danger),
              ),
              action: Button(label: 'Try again', onPressed: _make),
            ),
          ],
          const SizedBox(height: Space.s),
          Text(
            'Waiting for your other device…',
            style: type.caption.copyWith(color: palette.textFaint),
          ),
          // All devices lost, or a reinstall on the only one (dms-e2ee §2,
          // Abbacchio's row 630): a new security key here, never a dead end.
          const SizedBox(height: Space.s),
          Button(
            label: 'No other device?',
            onPressed: _starting ? null : _noOtherDevice,
          ),
          if (_signInFirst) ...[
            const SizedBox(height: Space.s),
            ActionLine(
              lead: Text(
                'For your security, sign in again to start over.',
                style: type.body.copyWith(color: palette.text),
              ),
              action: Button(
                label: 'Sign in again…',
                primary: true,
                onPressed: _signInThenStartOver,
              ),
            ),
          ],
          if (_startOverError case final error?) ...[
            const SizedBox(height: Space.xs),
            Text(error, style: type.caption.copyWith(color: palette.danger)),
          ],
          if (_canCopy && code != null) ...[
            const SizedBox(height: Space.m),
            Button(
              label: _copied == code ? 'Copied' : 'Copy code',
              onPressed: () => _copy(code),
            ),
            const SizedBox(height: Space.s),
            _Digits(widget.e2ee.offerDigits ?? ''),
            const SizedBox(height: Space.xs),
            Text(
              'If you paste this code instead of scanning it, check that your '
              'other device shows these same numbers. The code works once, '
              'for 30 minutes.',
              style: type.caption.copyWith(color: palette.textFaint),
            ),
          ],
        ],
      ),
    );
  }
}

/// A fresh sign-in before a new security key (403 sign_in_again, Fugo):
/// the device's own sign-in, again, saying why ([intro]). True once it
/// went through.
Future<bool> _signInAgain(BuildContext context, String intro) async {
  var done = false;
  await showModal(
    context,
    (context, close) => _SignInAgain(
      intro: intro,
      close: (signedIn) {
        done = signedIn;
        close();
      },
    ),
  );
  return done;
}

class _SignInAgain extends StatefulWidget {
  const _SignInAgain({required this.intro, required this.close});

  final String intro;
  final void Function(bool signedIn) close;

  @override
  State<_SignInAgain> createState() => _SignInAgainState();
}

class _SignInAgainState extends State<_SignInAgain> {
  DeviceAccount? _account;

  /// The emailed code was asked for: its sign-in finishing closes this.
  var _asked = false;

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
    final account = _account!;
    if (account.awaitingCode != null) {
      _asked = true;
    } else if (_asked && account.signedIn) {
      widget.close(true);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
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
                'Sign in again',
                style: type.emphasis.copyWith(color: palette.text),
              ),
              Button(label: 'Cancel', onPressed: () => widget.close(false)),
            ],
          ),
          const SizedBox(height: Space.l),
          // Only the account this device is signed in to: a mistyped
          // address would sign it in to another (Abbacchio's row 644).
          DeviceSignInForm(
            intro: widget.intro,
            email: switch (_account?.email) {
              final email? when email.isNotEmpty => email,
              _ => null,
            },
          ),
        ],
      ),
    );
  }
}

/// A device just added: its history coming over from the device that
/// added it (E8), or what didn't, with Try again while it can.
class _Transfer extends StatelessWidget {
  const _Transfer({required this.e2ee});

  final E2ee e2ee;

  @override
  Widget build(BuildContext context) {
    final transfer = e2ee.transfer!;
    if (transfer.end == null) {
      final total = transfer.chatsTotal;
      return _Note(
        title: 'Bringing your messages over…',
        total == 0
            ? ''
            : '${transfer.chatsDone} of $total ${total == 1 ? 'chat' : 'chats'}',
        action: Button(label: 'Skip', onPressed: e2ee.skipTransfer),
      );
    }
    final actions = Wrap(
      spacing: Space.s,
      runSpacing: Space.s,
      children: [
        if (e2ee.canRetryTransfer)
          Button(label: 'Try again', onPressed: e2ee.retryTransfer),
        Button(label: 'OK', onPressed: e2ee.dismissTransfer),
      ],
    );
    return switch (transfer.end!) {
      DmTransferEnd.cutShort => _Note(
        "Some messages didn't come over.",
        action: actions,
      ),
      _ => _Note(
        'Messages from before '
        '${pairedOn(e2ee.historyFromMs, DateTime.now())} are on your other '
        'devices.',
        action: actions,
      ),
    };
  }
}

class _Failed extends StatelessWidget {
  const _Failed({required this.e2ee});

  final E2ee e2ee;

  Future<void> _startOver(BuildContext context) async {
    var sure = false;
    await showModal(
      context,
      (context, close) => FormFrame(
        title: 'Start over on this device?',
        body:
            'Messages kept only on this device are lost. Your other devices '
            'keep theirs.',
        fields: const [],
        error: null,
        submitLabel: 'Start over',
        onSubmit: () {
          sure = true;
          close();
        },
        onCancel: close,
      ),
    );
    if (sure) await e2ee.startOver();
  }

  @override
  Widget build(BuildContext context) {
    final startOver = Builder(
      builder: (context) =>
          Button(label: 'Start over…', onPressed: () => _startOver(context)),
    );
    return switch (e2ee.failure) {
      DmFailure.identityReplaced => _Note(
        title: 'Your security key changed on another device',
        danger: true,
        'A new security key was made for your account on another device, so '
        'this device no longer gets new messages. If that was you, add this '
        "device again from there. If it wasn't, someone may be using your "
        'email: secure it, and tell your friends.',
        action: startOver,
      ),
      DmFailure.unreadableKey => _Note(
        "This device can't read its security key, so its messages can't be "
        'opened.',
        action: startOver,
      ),
      _ => _Note(
        "Messages couldn't be set up on this device.",
        action: Button(label: 'Try again', onPressed: e2ee.start),
      ),
    };
  }
}

/// The person's devices, this one first among them, with Add a device….
class _Devices extends StatefulWidget {
  const _Devices({required this.e2ee});

  final E2ee e2ee;

  @override
  State<_Devices> createState() => _DevicesState();
}

class _DevicesState extends State<_Devices> {
  List<DmOwnDevice> _devices = const [];

  /// Reads again once a device still setting up has had its code's time
  /// (row 564).
  Timer? _setUpBy;

  /// The person's devices off the list but still in a group here, by
  /// leaf, with the names they claim.
  Map<String, String> _strays = const {};

  /// The person's devices a start-over took out (under the key it
  /// replaced), by leaf with their names: never signed out until the
  /// person removes them (Fugo, row 630).
  Map<String, String> _replaced = const {};

  /// The account's Claktories by id, to name those still to finish a new
  /// security key; null until read (or when they can't be).
  Map<String, String>? _claktories;

  @override
  void initState() {
    super.initState();
    widget.e2ee.addListener(_read);
    _read();
    unawaited(_readClaktories());
    // Which of them signed out, as the service says now (row 671).
    unawaited(widget.e2ee.readSignedOut());
  }

  Future<void> _readClaktories() async {
    try {
      final list = await AppScope.scope(context).connections.account
          .claktories();
      if (mounted) {
        setState(
          () => _claktories = {
            for (final claktory in list) claktory.id: claktory.name,
          },
        );
      }
    } catch (_) {
      // Not signed in, or unreachable: named generically.
    }
  }

  @override
  void dispose() {
    _setUpBy?.cancel();
    widget.e2ee.removeListener(_read);
    super.dispose();
  }

  void _read() => unawaited(_readNow());

  Future<void> _readNow() async {
    try {
      final devices = await widget.e2ee.devices();
      final strays = await widget.e2ee.notFullyRemoved();
      final replaced = await widget.e2ee.replacedDevices();
      if (mounted) {
        setState(() {
          _devices = devices;
          _strays = strays;
          _replaced = replaced;
        });
        _setUpBy?.cancel();
        _setUpBy = null;
        final e2ee = widget.e2ee;
        if (devices.any(
          (each) => !each.here && each.sinceMs == 0 && !e2ee.neverSetUp(each),
        )) {
          final left =
              e2ee.deviceListMs +
              E2ee.codeLife.inMilliseconds -
              DateTime.now().millisecondsSinceEpoch;
          if (left > 0) {
            _setUpBy = Timer(Duration(milliseconds: left + 1000), _read);
          }
        }
      }
    } catch (_) {
      // Read again at the next change.
    }
  }

  /// The removal under way, while it isn't done: what it's doing, or
  /// that it stopped, with Try again.
  Widget? _underWay(Palette palette, TypeScale type) {
    final removal = widget.e2ee.removal;
    if (removal == null || removal.remove.isEmpty) return null;
    final names = [
      for (final name in removal.names)
        name.isEmpty ? 'A device' : proseCodes(name),
    ];
    final who = switch (names) {
      [] => 'A device',
      [final one] => one,
      [...final rest, final last] => '${rest.join(', ')} and $last',
    };
    final isnt = names.length > 1 ? "aren't" : "isn't";
    final running = widget.e2ee.removing;
    final stop = widget.e2ee.removalStop;
    // What's still true, as it stopped (Fugo 41048).
    final words = removal.newKey
        ? running
              ? 'Locking out $who…'
              : switch (stop) {
                  DmKeyChange.signInAgain =>
                    "$who can't read new messages, but your security key "
                        "hasn't changed yet. Sign in again to finish locking "
                        '${names.length > 1 ? 'them' : 'it'} out.',
                  DmKeyChange.signOutFailed =>
                    "$who can't read new messages, but Claktory couldn't "
                        "sign ${names.length > 1 ? 'them' : 'it'} out of your "
                        "Claktory account, so your security key hasn't "
                        'changed yet. Try again.',
                  // Connected to another account's Claktory (row 1453):
                  // Try again can't finish it from there.
                  DmKeyChange.waitsForOwnClaktory =>
                    "$who can't read new messages, but your security key "
                        "hasn't changed yet. It changes once this device "
                        'connects to one of your own Claktories.',
                  // The worker's 3 a day (Fugo 43752): it goes by itself.
                  DmKeyChange.keyWaitsForLimit => _limitWords(
                    context,
                    who: who,
                    many: names.length > 1,
                    e2ee: widget.e2ee,
                  ),
                  _ => '$who $isnt fully locked out yet.',
                }
        : running
        ? 'Removing $who…'
        : '$who $isnt fully removed yet.';
    final signIn = !running && stop == DmKeyChange.signInAgain;
    final byItself = removal.newKey && stop == DmKeyChange.keyWaitsForLimit;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: Space.xs),
      child: ActionLine(
        lead: Text(
          words,
          style: type.body.copyWith(
            color: running ? palette.textMuted : palette.danger,
          ),
        ),
        action: running || byItself
            ? const SizedBox.shrink()
            : Button(
                label: signIn ? 'Sign in again…' : 'Try again',
                onPressed: () =>
                    unawaited(signIn ? _signInThenFinish() : _finishRemoval()),
              ),
      ),
    );
  }

  /// Try again: carries on with the removal, and when a new key wants a
  /// fresh sign-in, asks for it there and then (Fugo 41048).
  Future<void> _finishRemoval() async {
    final stopped = await widget.e2ee.finishRemoval();
    if (stopped == DmKeyChange.signInAgain && mounted) {
      await _signInThenFinish();
    }
  }

  /// Signed in again, the Lock out carries on where it stopped.
  Future<void> _signInThenFinish() async {
    if (!await _signInAgain(
          context,
          'For your security, sign in again to change your security key.',
        ) ||
        !mounted) {
      return;
    }
    await widget.e2ee.finishRemoval();
  }

  Future<void> _lost(DmOwnDevice device) => showModal(
    context,
    (context, close) => _ChangeKey(
      e2ee: widget.e2ee,
      lost: device,
      devices: _devices,
      close: close,
    ),
  );

  Future<void> _remove(DmOwnDevice device) {
    final since = deviceSince(context, device, _devices);
    return showModal(
      context,
      (context, close) => _RemoveDevice(
        e2ee: widget.e2ee,
        leafId: device.leafId,
        name: device.name,
        since: since,
        close: close,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final devices = [
      ..._devices.where((device) => device.here),
      ..._devices.where((device) => !device.here),
    ];
    return Field(
      label: 'Your devices',
      note: 'These can read your messages.',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // Each named in full, with since when to tell two of the same
          // name apart; the buttons go under it when they don't fit beside
          // it (Zucchero's row 198).
          for (final device in devices)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: Space.xs),
              child: ActionLine(
                lead: _DeviceName(
                  name: device.name,
                  since: deviceSince(context, device, devices),
                  signedOut:
                      !device.here &&
                      widget.e2ee.signedOut.contains(device.leafId),
                  notSetUp: widget.e2ee.neverSetUp(device),
                ),
                action: device.here
                    ? Text(
                        'This device',
                        style: type.caption.copyWith(color: palette.textFaint),
                      )
                    : ControlsLine(
                        gap: Space.xs,
                        children: [
                          Button(
                            label: 'Lost or stolen…',
                            onPressed: () => _lost(device),
                          ),
                          Button(
                            label: 'Remove…',
                            onPressed: () => _remove(device),
                          ),
                        ],
                      ),
              ),
            ),
          // Off the list, still in a group: a removal that stopped part-way
          // (Abbacchio's R5b), finished from here.
          for (final MapEntry(key: leaf, value: name) in _strays.entries)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: Space.xs),
              child: ActionLine(
                lead: Text.rich(
                  TextSpan(
                    children: [
                      TextSpan(
                        text: name.isEmpty ? 'A device' : proseCodes(name),
                        style: TextStyle(color: palette.text),
                      ),
                      TextSpan(
                        text: '  Not fully removed',
                        style: type.caption.copyWith(color: palette.danger),
                      ),
                    ],
                  ),
                  style: type.body,
                ),
                action: Button(
                  label: 'Finish removing',
                  onPressed: widget.e2ee.removing
                      ? null
                      : () => unawaited(widget.e2ee.finishRemoving(leaf, name)),
                ),
              ),
            ),
          // A start-over took these out: they had the old key, and are
          // still signed in to the account until the person removes them.
          if (_replaced.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            Text(
              'Still on your old security key',
              style: type.caption.copyWith(
                color: palette.textMuted,
                fontWeight: FontWeight.w500,
              ),
            ),
            for (final MapEntry(key: leaf, value: name) in _replaced.entries)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: Space.xs),
                child: ActionLine(
                  lead: Text(
                    name.isEmpty ? 'A device' : proseCodes(name),
                    style: type.body.copyWith(color: palette.text),
                  ),
                  action: Button(
                    label: 'Remove…',
                    onPressed: () => _remove((
                      leafId: leaf,
                      name: name,
                      here: false,
                      sinceMs: 0,
                    )),
                  ),
                ),
              ),
          ],
          ?_underWay(palette, type),
          // After a new security key: each Claktory until the app connects
          // to it, when it takes the key (Fugo, Ghiaccio).
          for (final id in widget.e2ee.claktoriesToFinish)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: Space.xs),
              child: Text(
                'Connect to ${_claktories?[id] ?? 'your Claktory'} once to '
                'finish your new security key there.',
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            ),
          // A device of the person's under their old key holds a chat back
          // (Abbacchio's row 611); this one joins by itself once it moves.
          if (widget.e2ee.waitingForOtherDevices)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: Space.xs),
              child: Text(
                "Some of your chats can't come to this device yet. Open "
                'Claktory on your other devices so they can update, and '
                "they'll come here on their own.",
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            ),
          const SizedBox(height: Space.s),
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(
              label: 'Add a device…',
              onPressed: () => showAddDevice(context, widget.e2ee),
            ),
          ),
        ],
      ),
    );
  }
}

/// Scans a new device's code, asks the person, and adds it.
Future<void> showAddDevice(BuildContext context, E2ee e2ee) => showModal(
  context,
  (context, close) => _AddDevice(e2ee: e2ee, close: close),
);

class _AddDevice extends StatefulWidget {
  const _AddDevice({required this.e2ee, required this.close});

  final E2ee e2ee;
  final VoidCallback close;

  @override
  State<_AddDevice> createState() => _AddDeviceState();
}

class _AddDeviceState extends State<_AddDevice> with WidgetsBindingObserver {
  CameraAccess? _access;
  DmHandoffSealed? _found;
  String? _error;
  var _busy = false;

  /// The code was pasted, not scanned: Add waits until the person says
  /// the digits match the new device's ([_match]).
  var _pasted = false;
  var _match = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    unawaited(_ask());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  // Back from Settings, the camera may be allowed now (friend bug #2).
  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed && _found == null) {
      unawaited(_ask());
    }
  }

  Future<void> _ask() async {
    CameraAccess access;
    try {
      access = await AppScope.scope(context).camera.access();
    } catch (_) {
      // Paste code is the way, as with no camera.
      access = CameraAccess.unavailable;
    }
    if (mounted) setState(() => _access = access);
  }

  Future<void> _scanned(String code, {bool pasted = false}) async {
    if (_found != null || _busy) return;
    _busy = true;
    try {
      // The same reading for a pasted code as for a scanned one.
      final found = await widget.e2ee.scanned(code);
      if (mounted) {
        setState(() {
          _found = found;
          _pasted = pasted;
          _match = false;
          _error = null;
        });
      }
    } on DmCodeRefused {
      // Used already, or for a device removed (Abbacchio's row 612).
      if (mounted) setState(() => _error = DmCodeRefused.message);
    } catch (_) {
      if (mounted) {
        setState(
          () => _error = pasted
              ? "That isn't a code from a new device. Copy it again from the "
                    "new device's screen."
              : "That isn't a code for adding a device.",
        );
      }
    }
    _busy = false;
  }

  Future<void> _paste() async {
    String? text;
    try {
      text = await secretClipboard.paste();
    } catch (_) {
      // Read as nothing pasted: says so.
    }
    await _scanned(text ?? '', pasted: true);
  }

  Future<void> _add() async {
    final found = _found;
    if (found == null) return;
    setState(() => _busy = true);
    try {
      await widget.e2ee.addDevice(found);
      widget.close();
    } catch (error) {
      if (error is UserAppError &&
          error.problem == UserAppProblem.removedDevice) {
        // Removed on another device, which this one didn't know yet
        // (Abbacchio's row 617): Try again would fail the same way.
        if (mounted) {
          setState(() {
            _busy = false;
            _found = null;
            _error = DmCodeRefused.message;
          });
        }
        return;
      }
      if (mounted) {
        setState(() {
          _busy = false;
          _error = "Couldn't add it. Try again.";
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final found = _found;
    if (found != null) {
      final name = found.name.isEmpty ? 'This device' : found.name;
      final platform = found.platform.isEmpty ? '' : ' (${found.platform})';
      return FormFrame(
        title: 'Add this device to your messages?',
        body:
            '$name$platform will be able to read all your messages. Add it '
            "only if it's yours.",
        fields: [
          if (_pasted) ...[
            Text(
              'Check these numbers match the ones on your new device.',
              style: type.body.copyWith(color: palette.text),
            ),
            const SizedBox(height: Space.s),
            _Digits(found.comparison),
            const SizedBox(height: Space.s),
            Semantics(
              checked: _match,
              label: 'The numbers match',
              excludeSemantics: true,
              child: Pressable(
                onTap: () => setState(() => _match = !_match),
                builder: (context, hovered) => Row(
                  children: [
                    CheckMark(on: _match),
                    const SizedBox(width: Space.s),
                    Text(
                      'The numbers match',
                      style: type.body.copyWith(color: palette.text),
                    ),
                  ],
                ),
              ),
            ),
            const SizedBox(height: Space.xs),
            Text(
              "If they don't match, don't add it. Copy the code on your new "
              'device again.',
              style: type.caption.copyWith(color: palette.textFaint),
            ),
          ],
        ],
        error: _error,
        submitLabel: 'Add',
        onSubmit: _busy || (_pasted && !_match) ? null : _add,
        onCancel: widget.close,
      );
    }
    final camera = AppScope.scope(context).camera;
    return FormFrame(
      title: 'Scan the code on your new device',
      fields: [
        if (_access == CameraAccess.granted)
          AspectRatio(
            aspectRatio: 1,
            child: Container(
              clipBehavior: Clip.antiAlias,
              decoration: BoxDecoration(
                color: const Color(0xFF000000),
                borderRadius: BorderRadius.circular(Radii.panel),
                border: Border.all(color: palette.hairline),
              ),
              child: camera.preview((code) => unawaited(_scanned(code))),
            ),
          )
        else if (_access == CameraAccess.undetermined)
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(
              label: 'Allow camera',
              primary: true,
              onPressed: () async {
                await camera.request();
                await _ask();
              },
            ),
          )
        else if (_access == CameraAccess.denied) ...[
          // Refused so far: how to allow it, as the scanner says it.
          Text(
            defaultTargetPlatform == TargetPlatform.android
                ? 'To scan, allow Claktory to use the camera in Settings.'
                : 'To scan, allow camera access in Settings → Claktory.',
            style: type.body.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.s),
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(
              label: 'Open Settings',
              primary: true,
              onPressed: () => unawaited(camera.openSettings()),
            ),
          ),
        ] else if (_access != null)
          // None, or one the person can't allow (Screen Time).
          Text(
            "The camera isn't available on this $thisDeviceKind. Copy the "
            'code on your new device and choose Paste code instead.',
            style: type.body.copyWith(color: palette.textMuted),
          ),
        const SizedBox(height: Space.s),
        Align(
          alignment: AlignmentDirectional.centerStart,
          child: Button(label: 'Paste code instead', onPressed: _paste),
        ),
      ],
      error: _error,
      submitLabel: 'Done',
      onSubmit: null,
      onCancel: widget.close,
    );
  }
}

/// Removing one of the person's devices: out of their list and every
/// group, then signed out of their account (L0, Fugo), so a lost or stolen
/// one can do nothing with what it holds; if the sign-out doesn't go
/// through, it says so and tries again.
class _RemoveDevice extends StatefulWidget {
  const _RemoveDevice({
    required this.e2ee,
    required this.leafId,
    required this.name,
    this.since = '',
    required this.close,
  });

  final E2ee e2ee;
  final String leafId;
  final String name;

  /// Since when it's been the person's ("Since Mon 16 Nov"), to tell it
  /// from another of the same name; empty when that isn't known.
  final String since;
  final VoidCallback close;

  @override
  State<_RemoveDevice> createState() => _RemoveDeviceState();
}

class _RemoveDeviceState extends State<_RemoveDevice> {
  var _busy = false;
  String? _error;

  Future<void> _go() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    // One with no name is "that device", never "this device", the one in
    // hand (Abbacchio's row 1464).
    final name = widget.name.isEmpty ? 'That device' : widget.name;
    final nameIn = widget.name.isEmpty ? 'that device' : widget.name;
    // Kept as under way from here: closed part-way, it carries on at the
    // next launch, and Your devices offers Try again meanwhile.
    final stopped = await widget.e2ee.remove(widget.leafId, widget.name);
    if (stopped == null) {
      widget.close();
      return;
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _error = switch (stopped) {
        DmRemoveStop.failed => "Couldn't remove $nameIn. Try again.",
        // Unreachable leaves it unknown; anything else was a refusal.
        DmRemoveStop.signOutUnreachable =>
          '$name no longer gets new messages, but it may still be signed '
              'in to your Claktory account. Try again.',
        DmRemoveStop.signOutRefused =>
          "$name no longer gets new messages, but it couldn't be signed "
              'out of your Claktory account. Try again.',
      };
    });
  }

  @override
  Widget build(BuildContext context) => FormFrame(
    title:
        'Remove ${widget.name.isEmpty ? 'the unnamed device' : proseCodes(widget.name)}?',
    body:
        '${widget.since.isEmpty ? '' : '${widget.since}.\n\n'}'
        'It stops getting new messages and is signed out of your '
        'Claktory account. If you sign in on it again, it starts over as a '
        'new device.\n\n'
        "If it was lost or stolen, also change your email's password, "
        'since your email can sign in new devices. And remove it in '
        "Settings → Devices, so it can't use your Claktory.",
    fields: const [],
    error: _error,
    submitLabel: _error == null ? 'Remove' : 'Try again',
    onSubmit: _busy ? null : _go,
    onCancel: widget.close,
  );
}

/// Lost or stolen (dms-e2ee §2.1): the device is removed and signed out,
/// then the person's messages get a new security key. The person confirms
/// the devices to keep by name and since when; any they untick are
/// removed with it. The lists stay the same across Try again, which
/// carries on where it stopped.
class _ChangeKey extends StatefulWidget {
  const _ChangeKey({
    required this.e2ee,
    required this.lost,
    required this.devices,
    required this.close,
  });

  final E2ee e2ee;
  final DmOwnDevice lost;
  final List<DmOwnDevice> devices;
  final VoidCallback close;

  @override
  State<_ChangeKey> createState() => _ChangeKeyState();
}

class _ChangeKeyState extends State<_ChangeKey> {
  late final List<DmOwnDevice> _others = [
    for (final device in widget.devices)
      if (!device.here && device.leafId != widget.lost.leafId) device,
  ];
  late final Set<String> _keep = {for (final device in _others) device.leafId};

  /// What was confirmed when it started: the same on Try again.
  ({List<String> remove, List<String> keep})? _started;
  var _busy = false;
  String? _error;

  /// The lost device's name, at the start of a sentence; one with none is
  /// "That device", never "This device", which is the one in hand
  /// (Abbacchio's row 1464).
  String get _name =>
      widget.lost.name.isEmpty ? 'That device' : widget.lost.name;

  /// [_name] inside a sentence.
  String get _nameIn =>
      widget.lost.name.isEmpty ? 'that device' : widget.lost.name;

  /// Since when the lost device has been the person's, to tell it from
  /// another of the same name.
  String get _lostSince => deviceSince(context, widget.lost, widget.devices);

  /// [leaf]'s name, kept with the Lock out to say which device it is once
  /// it's off the list.
  String _nameOf(String leaf) =>
      widget.devices.where((each) => each.leafId == leaf).firstOrNull?.name ??
      '';

  Future<void> _go() async {
    final lists = _started ??= (
      remove: [
        widget.lost.leafId,
        for (final device in _others)
          if (!_keep.contains(device.leafId)) device.leafId,
      ],
      keep: [..._keep],
    );
    setState(() {
      _busy = true;
      _error = null;
    });
    final stopped = await widget.e2ee.changeKey(
      remove: lists.remove,
      keep: lists.keep,
      names: [for (final leaf in lists.remove) _nameOf(leaf)],
    );
    if (stopped == null) {
      widget.close();
      return;
    }
    if (!mounted) return;
    setState(() {
      _busy = false;
      _error = switch (stopped) {
        DmKeyChange.signOutFailed =>
          "$_name no longer gets new messages, but it couldn't be signed "
              "out of your Claktory account, so your security key hasn't "
              'changed yet. Try again.',
        DmKeyChange.failed =>
          "Your new security key isn't finished. Try again to carry on where "
              'it stopped.',
        DmKeyChange.signInAgain =>
          'For your security, sign in again to change your security key.',
        DmKeyChange.waitsForOwnClaktory =>
          "$_name no longer gets new messages, but your security key hasn't "
              'changed yet. It changes once this device connects to one of '
              'your own Claktories.',
        DmKeyChange.keyWaitsForLimit => _limitWords(
          context,
          who: _name,
          many: false,
          e2ee: widget.e2ee,
        ),
      };
      _signInFirst = stopped == DmKeyChange.signInAgain;
    });
  }

  /// A new key wants a recent sign-in (row 630): signed in again, it
  /// carries on where it stopped.
  var _signInFirst = false;

  Future<void> _signInThenGo() async {
    if (!await _signInAgain(
          context,
          'For your security, sign in again to change your security key.',
        ) ||
        !mounted) {
      return;
    }
    setState(() => _signInFirst = false);
    await _go();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return FormFrame(
      title:
          'Lock out ${widget.lost.name.isEmpty ? 'the unnamed device' : proseCodes(_name)}?',
      body:
          '${_lostSince.isEmpty ? '' : '$_lostSince.\n\n'}'
          '$_name is removed and signed out of your Claktory account, and '
          "you get a new security key, so $_nameIn can't read new messages. "
          'Your friends see your security key change.\n\n'
          "Also change your email's password, since your email can sign in "
          "new devices. And remove it in Settings → Devices, so it can't use "
          'your Claktory.',
      fields: [
        if (_others.isNotEmpty) ...[
          Text(
            'Devices to keep',
            style: type.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
          const SizedBox(height: Space.xs),
          for (final device in _others)
            Semantics(
              checked: _keep.contains(device.leafId),
              label: device.name.isEmpty ? 'A device' : device.name,
              excludeSemantics: true,
              child: Pressable(
                onTap: _busy || _started != null
                    ? null
                    : () => setState(() {
                        if (!_keep.remove(device.leafId)) {
                          _keep.add(device.leafId);
                        }
                      }),
                builder: (context, hovered) => Padding(
                  padding: const EdgeInsets.symmetric(vertical: Space.xs),
                  child: Row(
                    children: [
                      CheckMark(on: _keep.contains(device.leafId)),
                      const SizedBox(width: Space.s),
                      Expanded(
                        child: _DeviceName(
                          name: device.name,
                          since: deviceSince(context, device, widget.devices),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          const SizedBox(height: Space.xs),
          Text(
            "Untick any you don't recognize, and they're locked out too.",
            style: type.caption.copyWith(color: palette.textFaint),
          ),
        ],
      ],
      error: _error,
      submitLabel: _signInFirst
          ? 'Sign in again…'
          : _error == null
          ? 'Lock out'
          : 'Try again',
      onSubmit: _busy
          ? null
          : _signInFirst
          ? _signInThenGo
          : _go,
      onCancel: widget.close,
    );
  }
}

/// The digits two devices compare for a pasted code, in four groups.
class _Digits extends StatelessWidget {
  const _Digits(this.digits);

  final String digits;

  @override
  Widget build(BuildContext context) => Text(
    digits,
    style: TypeScale.of(context).emphasis.copyWith(
      color: Palette.of(context).text,
      fontFeatures: const [FontFeature.tabularFigures()],
      letterSpacing: 0.5,
    ),
  );
}

/// [dayLabel] mid-line: "today", "yesterday", or the date as it is
/// ("Since today", Trish).
String _sinceDay(String label) => switch (label) {
  'Today' => 'today',
  'Yesterday' => 'yesterday',
  _ => label,
};

/// Since when [device] has been one of the person's: "Since Mon 16 Nov",
/// with the time too when another of [devices] has its name, so two Macs
/// both called "Mac Studio" read apart (Zucchero's row 198); empty when it
/// isn't known.
String deviceSince(
  BuildContext context,
  DmOwnDevice device,
  List<DmOwnDevice> devices,
) {
  if (device.sinceMs <= 0) return '';
  final day = _sinceDay(
    dayLabel(
      DateTime.fromMillisecondsSinceEpoch(device.sinceMs),
      DateTime.now(),
    ),
  );
  final twin = devices.any(
    (other) => other.leafId != device.leafId && other.name == device.name,
  );
  if (!twin) return 'Since $day';
  final time = clockTime(
    device.sinceMs,
    h24: MediaQuery.maybeAlwaysUse24HourFormatOf(context) ?? true,
  );
  return 'Since $day at $time';
}

/// What a Lock out says while its new key waits for the worker's next day
/// ([DmKeyChange.keyWaitsForLimit], Fugo 43752): plainly not finished, and
/// when it carries on by itself.
String _limitWords(
  BuildContext context, {
  required String who,
  required bool many,
  required E2ee e2ee,
}) {
  final it = many ? 'them' : 'it';
  final at = e2ee.keyChangeAtMs;
  final when = at == null ? 'later' : _atWords(context, at);
  return e2ee.keyHeldByLockedOut
      ? '$who got your current security key before you locked $it out. '
            'Claktory changes it again $when to finish locking $it out.'
      : "$who can't read new messages, but your security key hasn't changed "
            'yet: it can change only 3 times a day. Claktory changes it $when.';
}

/// "at 01:00", or "tomorrow at 01:00", on this device's clock.
String _atWords(BuildContext context, int ms) {
  final time = clockTime(
    ms,
    h24: MediaQuery.maybeAlwaysUse24HourFormatOf(context) ?? true,
  );
  final at = DateTime.fromMillisecondsSinceEpoch(ms);
  final now = DateTime.now();
  final today =
      at.year == now.year && at.month == now.month && at.day == now.day;
  return today ? 'at $time' : 'tomorrow at $time';
}

/// A device's name, whole (wrapped, never cut short), and since when it's
/// been the person's under it.
class _DeviceName extends StatelessWidget {
  const _DeviceName({
    required this.name,
    required this.since,
    this.signedOut = false,
    this.notSetUp = false,
  });

  final String name;
  final String since;

  /// Listed, but its code ran out before it set up (row 564).
  final bool notSetUp;

  /// It signed out of the person's Claktory account (row 671): still on
  /// their list until they remove it.
  final bool signedOut;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Text.rich(
          TextSpan(
            children: [
              TextSpan(
                text: name.isEmpty ? 'A device' : proseCodes(name),
                style: TextStyle(color: palette.text),
              ),
              if (signedOut)
                TextSpan(
                  text: '  Signed out',
                  style: type.caption.copyWith(color: palette.textMuted),
                ),
              if (notSetUp)
                TextSpan(
                  text: '  Not set up',
                  style: type.caption.copyWith(color: palette.danger),
                ),
            ],
          ),
          style: type.body,
        ),
        if (since.isNotEmpty)
          Text(since, style: type.caption.copyWith(color: palette.textFaint)),
      ],
    );
  }
}
