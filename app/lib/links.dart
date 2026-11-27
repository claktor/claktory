// Links that open Claktory: `claktory://add?code=<pairing code>` adds the
// Claktory the code is for (after asking); and a pairing code itself (`factory://?t=…&e=…`, or the same as
// `claktory://`), which the iPhone's
// Camera opens from a Mac's QR code. The Mac's app takes them
// (AppDelegate.swift), the iPhone's (SceneDelegate) and Android's
// (MainActivity) too, and https://claktory.com/pair#<the code's query>,
// the page a QR code may open, as a Universal Link or App Link; Claktory
// Dev's scheme is claktory-dev, and debug
// builds' claktory-debug, so claktory:// always opens the real Claktory.
// Nothing connects without the person's Connect in [confirmPairing], the
// one confirmation every way in uses (a link, the scanner, a paste).
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'connections.dart';
import 'controls.dart' show showModal;
import 'devices.dart' show thisDeviceKind;
import 'forms.dart' show FormFrame;
import 'identity.dart';
import 'ipad/pair_step.dart' show pairScreenKey;
import 'mobile/device.dart' show deviceCapabilities;
import 'mobile/drawer.dart' show PhoneDrawerHost;
import 'mobile/local_network.dart' show connectNewClaktory;
import 'mobile/permission.dart' show permissionHost;
import 'rail.dart' show PhoneClaktories;
import 'scope.dart';

/// The schemes this app's builds open links with.
const linkSchemes = {'claktory', 'claktory-dev', 'claktory-debug'};

/// The longest link taken: anything longer is refused before it's parsed.
const linkMaxLength = 4096;

/// What a link asks for: to add the Claktory its code is for ([target]),
/// or nothing this app can do ([target] null).
class AddLink {
  const AddLink(this.target);

  final RemoteFactory? target;
}

/// What [link] asks for; null when it isn't a link to add a Claktory.
AddLink? parseLink(Uri link) {
  // A pairing code itself, as a Mac's QR code holds it: factory://, or
  // claktory:// in a code's shapes (no host, through the tunnel; or a
  // host and port), never claktory://add or another word.
  final shaped = link.host.isEmpty || link.hasPort;
  if (link.scheme == 'factory' || (link.scheme == 'claktory' && shaped)) {
    return AddLink(parsePairingCode(link.toString()));
  }
  // The pairing page's link, as a Universal Link or App Link opens the app
  // with it (#352 P6): only the code after its `#` is read.
  if (link.scheme == 'https') {
    final code = pairLinkCode(link.toString());
    return code == null ? null : AddLink(parsePairingCode(code));
  }
  if (!linkSchemes.contains(link.scheme) || link.host != 'add') return null;
  // The code is URL-encoded in `code`; one pasted in as it is runs on into
  // the link's own query, so everything after `code=` is tried too.
  // A broken escape (%E0%A4, say) throws as the query is decoded: a
  // garbled link carries no code, rather than an error.
  String code;
  try {
    code = link.queryParameters['code'] ?? '';
  } on FormatException {
    code = '';
  }
  final raw = link.query;
  final at = raw.indexOf('code=');
  final whole = at < 0 ? '' : raw.substring(at + 'code='.length);
  String decoded(String text) {
    try {
      return Uri.decodeComponent(text);
    } on ArgumentError {
      return text;
    } on FormatException {
      return text;
    }
  }

  return AddLink(parsePairingCode(code) ?? parsePairingCode(decoded(whole)));
}

/// The links the app was opened with, waiting for the window to take
/// them.
class AppLinks extends ChangeNotifier {
  final _waiting = <Uri>[];

  static const _channel = MethodChannel('factory/links');

  /// Takes links from the app (a Mac, an iPhone, Android): those that
  /// came before, then each as it comes.
  Future<void> listen() async {
    if (!_takesLinks.contains(defaultTargetPlatform)) return;
    _channel.setMethodCallHandler((call) async {
      if (call.method == 'opened' && call.arguments is String) {
        receive(call.arguments as String);
      }
    });
    try {
      final waiting = await _channel.invokeListMethod<String>('listen');
      for (final link in waiting ?? const <String>[]) {
        receive(link);
      }
    } on PlatformException {
      // An app without the channel: no links.
    } on MissingPluginException {
      // The same, in tests.
    }
  }

  static const _takesLinks = {
    TargetPlatform.macOS,
    TargetPlatform.iOS,
    TargetPlatform.android,
  };

  /// [link] opened the app. One too long is refused unread.
  void receive(String link) {
    if (link.length > linkMaxLength) return;
    final uri = Uri.tryParse(link.trim());
    if (uri == null) return;
    _waiting.add(uri);
    notifyListeners();
  }

  /// The oldest link not yet taken, taking it.
  Uri? take() => _waiting.isEmpty ? null : _waiting.removeAt(0);
}

/// The app's links.
final appLinks = AppLinks();

/// The first 16 characters of what identifies [target]'s Claktory (its
/// tunnel's endpoint id, else its certificate's fingerprint), in fours:
/// "a1b2 c3d4 e5f6 7890".
String pairingFingerprint(RemoteFactory target) {
  final id = target.endpointId.isNotEmpty
      ? target.endpointId
      : target.fingerprint;
  final short = id.length > 16 ? id.substring(0, 16) : id;
  return [
    for (var i = 0; i < short.length; i += 4)
      short.substring(i, i + 4 > short.length ? short.length : i + 4),
  ].join(' ');
}

/// Whether [target] is one of the Claktories on the person's account, by
/// the endpoint the app would dial; null when that can't be known (signed
/// out, or the list unreadable).
Future<bool?> Function(BuildContext context, RemoteFactory target)?
onAccountTest;

Future<bool?> _onAccount(BuildContext context, RemoteFactory target) async {
  final test = onAccountTest;
  if (test != null) return test(context, target);
  final account = AppScope.scope(context).connections.account;
  if (!account.signedIn || target.endpointId.isEmpty) return null;
  try {
    final listed = await account.claktories();
    return listed.any(
      (claktory) => claktory.endpointId == target.endpointId.toLowerCase(),
    );
  } catch (_) {
    return null;
  }
}

/// Asks before this device connects to [target], however its code came
/// (a link, a QR code, a paste): true only for the person's Connect. It
/// leads with the code's fingerprint; the name is the other side's own
/// words, so it shows only quoted, never as the title; and it warns when
/// the Claktory isn't on the person's account (Fugo's rules), when that's
/// known: signed out or unreadable, it only cautions.
Future<bool> confirmPairing(BuildContext context, RemoteFactory target) async {
  final onAccount = await pairingOnAccount(context, target);
  if (!context.mounted) return false;
  var connect = false;
  // On an iPad a dialog at every width, which a change between Split View
  // and full screen keeps (Sale's row 198).
  final iPad =
      defaultTargetPlatform == TargetPlatform.iOS &&
      deviceCapabilities.idiom == Idiom.pad;
  await showModal(dialog: iPad, context, (dialog, close) {
    return FormFrame(
      title: pairingCheckTitle,
      fields: [PairingCheck(target: target, onAccount: onAccount)],
      error: null,
      submitLabel: 'Connect',
      onSubmit: () {
        connect = true;
        close();
      },
      onCancel: close,
    );
  });
  return connect;
}

/// Whether [target] is on the person's account, for [PairingCheck]: null
/// when that can't be known.
Future<bool?> pairingOnAccount(BuildContext context, RemoteFactory target) =>
    _onAccount(context, target);

/// What asks before a code connects: [confirmPairing]'s, and Add a
/// Claktory's once a code is pasted or scanned (devices-signin.md §3).
const pairingCheckTitle = 'Connect to this Claktory?';

/// What's checked before a code connects: the code's fingerprint, the
/// other side's name only quoted, and whether it's on the person's account
/// as far as that's known ([onAccount], Fugo's rules).
class PairingCheck extends StatelessWidget {
  const PairingCheck({
    super.key,
    required this.target,
    required this.onAccount,
  });

  final RemoteFactory target;
  final bool? onAccount;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text.rich(
          TextSpan(
            children: [
              // What the code's four groups are for, with no noun of
              // their own: "code" is only what's pasted or scanned (Trish
              // 41557). Its name only in the line under it, quoted, its
              // own words (Fugo), so not twice.
              TextSpan(
                text: 'Check that it matches what it showed there: ',
                style: TextStyle(color: palette.textMuted),
              ),
              TextSpan(
                text: pairingFingerprint(target),
                style: TextStyle(
                  fontFamily: TextStyles.mono,
                  color: palette.text,
                ),
              ),
            ],
          ),
          style: TextStyles.body,
        ),
        const SizedBox(height: Space.xs),
        Text(
          [
            if (target.name.isNotEmpty) 'It calls itself “${target.name}”.',
            // One on this device's account lets it in with no Allow
            // (Abbacchio's row 1667).
            if (onAccount != true)
              "You'll still need to allow this $thisDeviceKind on it.",
          ].join(' '),
          style: TextStyles.body.copyWith(color: palette.textMuted),
        ),
        // Off the account only once its list was read and says so (#529,
        // Squalo's MC3): signed out, or the list unreadable, nothing is
        // known of whose it is, so there's no claim, only the caution.
        if (onAccount == false) ...[
          const SizedBox(height: Space.m),
          Text(
            "This Claktory isn't on your Claktory account. Connect only "
            "if you're setting it up yourself, right now.",
            style: TextStyles.body.copyWith(color: palette.danger),
          ),
        ] else if (onAccount == null) ...[
          const SizedBox(height: Space.m),
          Text(
            "Connect only if you're setting it up yourself, right now.",
            style: TextStyles.body.copyWith(color: palette.textMuted),
          ),
        ],
      ],
    );
  }
}

/// Takes what [links] brings, in the window: asks before adding a
/// Claktory a link names, and shows one already added. A page can open a
/// link without the person, so nothing connects without their click (and
/// the Claktory still has to allow this device).
class LinkOpener extends StatefulWidget {
  const LinkOpener({super.key, required this.child, this.links});

  final Widget child;

  /// Where links come from: the app's ([appLinks]) unless a test says.
  final AppLinks? links;

  @override
  State<LinkOpener> createState() => _LinkOpenerState();
}

class _LinkOpenerState extends State<LinkOpener> {
  AppLinks get _links => widget.links ?? appLinks;

  @override
  void initState() {
    super.initState();
    _links.addListener(_next);
    // One that came before this window.
    WidgetsBinding.instance.addPostFrameCallback((_) => _next());
  }

  @override
  void dispose() {
    _links.removeListener(_next);
    super.dispose();
  }

  void _next() {
    if (!mounted) return;
    final link = _links.take();
    if (link == null) return;
    final asked = parseLink(link);
    if (asked == null) return;
    // A link takes the person out of the phone's menu, as a deep link
    // dismisses what's open; what it asks in a sheet would otherwise open
    // behind the menu.
    PhoneDrawerHost.closeAll();
    final target = asked.target;
    final connections = AppScope.scope(context).connections;
    if (target == null) {
      showModal(
        context,
        (context, close) => FormFrame(
          title: "Can't use this link",
          // The screen it's pasted on, by its name on this device.
          body:
              'This link has no Claktory code in it. Paste the code in '
              '${isPhoneLayout(context) ? 'Connect to your Claktory' : 'Connect to another Claktory'} '
              'instead.',
          fields: const [],
          error: null,
          submitLabel: 'OK',
          onSubmit: close,
          onCancel: close,
        ),
      );
      return;
    }
    // Added already: it shows; on a phone, in place of its list of
    // Claktories or Add a Claktory, where a link is taken too (Zucchero's
    // row 685).
    if (connections.saved.contains(target)) {
      AppScope.switchFactory(context, target);
      PhoneClaktories.showConversation();
      return;
    }
    unawaited(() async {
      if (!await confirmPairing(context, target)) return;
      // Crossing between the narrow and wide layouts while Connect to this
      // Claktory? waits builds a new window, and this opener with it: Connect
      // goes on from the pairing screen, which crosses whole, or else the
      // screen showing now (Sale's row 202).
      final from = mounted
          ? context
          : pairScreenKey.currentContext ?? permissionHost;
      if (from != null && from.mounted) {
        // The first on an iPhone or iPad: why Claktory asks for the local
        // network, before iOS does (mobile/local_network.dart).
        await connectNewClaktory(from, target);
        if (connections.saved.contains(target)) {
          PhoneClaktories.showConversation();
        }
      }
    }());
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
