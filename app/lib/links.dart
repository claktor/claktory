// Links that open Claktory: `claktory://add?code=<pairing code>` adds the
// Claktory the code is for (after asking), so an agent setting one up on a
// server can hand this Mac its code with `open "claktory://add?code=…"`.
// The Mac's app takes them (AppDelegate.swift); Claktory Dev's scheme is
// claktory-dev, and debug builds' claktory-debug, so claktory:// always
// opens the real Claktory.
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'connections.dart';
import 'controls.dart' show showModal;
import 'devices.dart' show thisDeviceKind;
import 'forms.dart' show FormFrame;
import 'scope.dart';

/// The schemes this app's builds open links with.
const linkSchemes = {'claktory', 'claktory-dev', 'claktory-debug'};

/// What a link asks for: to add the Claktory its code is for ([target]),
/// or nothing this app can do ([target] null).
class AddLink {
  const AddLink(this.target);

  final RemoteFactory? target;
}

/// What [link] asks for; null when it isn't a link to add a Claktory.
AddLink? parseLink(Uri link) {
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

  /// Takes links from the app (macOS): those that came before, then each
  /// as it comes.
  Future<void> listen() async {
    if (defaultTargetPlatform != TargetPlatform.macOS) return;
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

  /// [link] opened the app.
  void receive(String link) {
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
    final target = asked.target;
    final connections = AppScope.scope(context).connections;
    if (target == null) {
      showModal(
        context,
        (context, close) => FormFrame(
          title: "Can't use this link",
          body:
              'This link has no Claktory code in it. Paste the code in '
              'Connect to another Claktory instead.',
          fields: const [],
          error: null,
          submitLabel: 'OK',
          onSubmit: close,
          onCancel: close,
        ),
      );
      return;
    }
    // Added already: it shows.
    if (connections.saved.contains(target)) {
      AppScope.switchFactory(context, target);
      return;
    }
    final name = target.label;
    showModal(
      context,
      (dialog, close) => FormFrame(
        title: 'Connect to $name?',
        body:
            'A link asks to connect this $thisDeviceKind to the Claktory '
            '“$name”. Connect only if you or one of your agents opened it. '
            "You'll still need to allow this $thisDeviceKind on $name.",
        fields: const [],
        error: null,
        submitLabel: 'Connect',
        onSubmit: () {
          close();
          if (mounted) AppScope.switchFactory(context, target);
        },
        onCancel: close,
      ),
    );
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
