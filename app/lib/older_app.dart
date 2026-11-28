// An update this Mac's Claktory couldn't start, from a version whose
// backend has no socket for this app: its supervisor put the older backend
// back, but this newer app, already in place, can only connect on the
// socket. So after the socket has stayed missing a while, the app reads
// what the supervisor wrote (`<home>/update-failed.json`) and offers the
// older version's own app, which the native side checks is Claktory's,
// genuinely signed, before it swaps it in.
import 'dart:convert';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart' show Backend;
import 'claktory_problem.dart';
import 'local_socket.dart' show localClaktoryHome, showsDevClaktory;

/// The first version whose backend listens on this app's socket: an update
/// to one that failed left a backend this app can't reach.
const _firstWithSocket = (0, 1, 28);

/// How long the socket stays missing before it counts as gone, rather than
/// its backend restarting.
const olderAppAfter = Duration(seconds: 30);

/// A release's version, `v0.1.27`; with [dev], also Claktory Dev's
/// `v0.1.27-dev.20261127-2244.09`.
final class AppVersion implements Comparable<AppVersion> {
  const AppVersion._(this.text, this.major, this.minor, this.patch, this.dev);

  static final _release = RegExp(r'^v(\d{1,6})\.(\d{1,6})\.(\d{1,6})$');
  static final _devRelease = RegExp(
    r'^v(\d{1,6})\.(\d{1,6})\.(\d{1,6})-dev\.(\d{8}-\d{4}\.\d{2})$',
  );

  /// Null for anything else: a path, a date stamp, "dev".
  static AppVersion? parse(String text, {required bool dev}) {
    final match =
        _release.firstMatch(text) ??
        (dev ? _devRelease.firstMatch(text) : null);
    if (match == null) return null;
    int part(int group) => int.parse(match.group(group)!);
    return AppVersion._(
      text,
      part(1),
      part(2),
      part(3),
      match.groupCount > 3 ? match.group(4) : null,
    );
  }

  final String text;
  final int major;
  final int minor;
  final int patch;

  /// A dev release's stamp, which sorts as its time.
  final String? dev;

  @override
  int compareTo(AppVersion other) {
    for (final (a, b) in [
      (major, other.major),
      (minor, other.minor),
      (patch, other.patch),
    ]) {
      if (a != b) return a.compareTo(b);
    }
    return (dev ?? '').compareTo(other.dev ?? '');
  }

  bool operator <(AppVersion other) => compareTo(other) < 0;

  bool atLeast((int, int, int) version) {
    final (major, minor, patch) = version;
    return compareTo(AppVersion._('', major, minor, patch, null)) >= 0;
  }

  @override
  String toString() => text;
}

/// What the page offers: [failed] didn't start, so this Mac went back to
/// [backTo], whose app can use it.
typedef OlderApp = ({AppVersion failed, AppVersion backTo});

/// The offer `update-failed.json` ([json]) makes to this app, at version
/// [appVersion]; null unless it names an update that failed, to a version
/// no newer than this app's (and, for a release, one with the socket), and
/// a version to go back to older than both, each a version exactly.
OlderApp? olderAppOffer(
  String json, {
  required String appVersion,
  required bool dev,
}) {
  final Object? read;
  try {
    read = jsonDecode(json);
  } on FormatException {
    return null;
  }
  if (read is! Map<String, Object?>) return null;
  final (version, backTo) = (read['version'], read['back_to']);
  if (version is! String || backTo is! String) return null;
  final app = AppVersion.parse(appVersion, dev: dev);
  final failed = AppVersion.parse(version, dev: dev);
  final older = AppVersion.parse(backTo, dev: dev);
  if (app == null || failed == null || older == null) return null;
  if (app < failed || !(older < failed)) return null;
  if (!dev && !failed.atLeast(_firstWithSocket)) return null;
  return (failed: failed, backTo: older);
}

/// Watches this Mac's socket through failed tries to connect to it, and
/// once it has been missing for [after], reads the offer.
class OlderAppWatch {
  OlderAppWatch({
    required this.appVersion,
    this.dev = showsDevClaktory,
    this.after = olderAppAfter,
    Future<String> Function()? home,
  }) : _home = home ?? (() => localClaktoryHome(dev: dev));

  final String appVersion;
  final bool dev;
  final Duration after;
  final Future<String> Function() _home;

  DateTime? _missingSince;

  /// After a try at the socket at [socket] failed: the offer, once it has
  /// been missing long enough; null otherwise, or when there's none.
  Future<OlderApp?> failed(String socket, {DateTime? now}) async {
    if (FileSystemEntity.typeSync(socket, followLinks: false) !=
        FileSystemEntityType.notFound) {
      _missingSince = null;
      return null;
    }
    now ??= DateTime.now();
    final since = _missingSince ??= now;
    if (now.difference(since) < after) return null;
    try {
      final home = await _home();
      final json = await File('$home/update-failed.json').readAsString();
      return olderAppOffer(json, appVersion: appVersion, dev: dev);
    } on Exception {
      return null;
    }
  }

  /// Connected on the socket: it's there.
  void connected() => _missingSince = null;
}

const _device = MethodChannel('factory/device');

/// "Claktory v0.1.28 didn't start": this Mac went back to [OlderApp.backTo],
/// whose app is the one press here; nothing changes until it's pressed.
class OlderAppPage extends StatefulWidget {
  const OlderAppPage({super.key, required this.offer});

  final OlderApp offer;

  @override
  State<OlderAppPage> createState() => _OlderAppPageState();
}

class _OlderAppPageState extends State<OlderAppPage> {
  var _opening = false;
  String? _error;

  Future<void> _use() async {
    setState(() {
      _opening = true;
      _error = null;
    });
    String? error;
    try {
      final home = await localClaktoryHome();
      // On success the app quits, and the older one opens.
      await _device.invokeMethod<void>('useOlderApp', {
        'home': home,
        'version': widget.offer.backTo.text,
      });
    } on MissingPluginException {
      error = "this app can't swap itself";
    } on PlatformException catch (failure) {
      error = failure.message ?? failure.code;
    } on Exception catch (failure) {
      error = '$failure';
    }
    if (!mounted || error == null) return;
    setState(() {
      _opening = false;
      _error = error;
    });
  }

  @override
  Widget build(BuildContext context) {
    final (:failed, :backTo) = widget.offer;
    final error = _error;
    return ClaktoryProblemPage(
      title: "Claktory $failed didn't start",
      body: [
        ProblemText(
          'This Mac went back to Claktory $backTo, and your agents keep '
          "working there. This app can't connect to it, so use the $backTo "
          'app until the next update.',
        ),
        if (error != null)
          ProblemText(
            "Couldn't open Claktory $backTo: $error. Install Claktory again "
            'from claktory.com.',
          ),
      ],
      primary: ProblemAction(
        _opening ? 'Opening…' : 'Open Claktory $backTo',
        _opening ? null : _use,
      ),
    );
  }
}

/// [backend]'s offer, while this Mac's own Claktory is shown.
OlderApp? olderAppOf(Backend backend) =>
    backend.target == null ? backend.olderApp.value : null;
