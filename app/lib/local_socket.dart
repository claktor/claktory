// How the Mac app reaches its own Claktory:
// a WebSocket over the Unix socket `<home>/app.sock`, owner-only, whose
// backend checks this app's code signature, rather than the loopback port
// or a pairing code. Nothing on this machine but Claktory's own app gets
// in there, so the app sends no token.
import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart' show kReleaseMode;
import 'package:flutter/services.dart';

/// The longest path a Unix socket's address takes on macOS (sun_path's 104
/// bytes, less its closing zero).
const _socketPathMost = 103;

/// Whether this app shows the dev Claktory by itself: Claktory Dev
/// (`FACTORY_DEV`), and any build but a release one, so a stray run or a
/// test never knocks on the person's Claktory.
const showsDevClaktory = bool.fromEnvironment('FACTORY_DEV') || !kReleaseMode;

/// Claktory Dev's data folder ([showsDevClaktory]): the environment's
/// `FACTORY_HOME` when set, else `~/.factory-dev`.
String localHome({Map<String, String>? environment}) {
  final env = environment ?? Platform.environment;
  final set = env['FACTORY_HOME'];
  return set != null && set.isNotEmpty
      ? set
      : '${env['HOME'] ?? ''}/.factory-dev';
}

/// The person's home folder (or, for a long path, their temporary folder)
/// couldn't be read from this Mac, so Claktory's socket isn't looked for
/// anywhere else.
class LocalHomeUnknown implements Exception {
  const LocalHomeUnknown();

  @override
  String toString() => "LocalHomeUnknown: the user database didn't say";
}

/// Where the Claktory this app shows by itself listens: Claktory Dev's at
/// [localHome]; Claktory's always in `.factory` in the person's home folder
/// as this Mac's user database has it (getpwuid_r, on the native side),
/// never by the environment, since another program could set `HOME` or
/// `FACTORY_HOME` (`launchctl setenv`) to a home of its own with a genuine
/// backend on it (Fugo). Throws [LocalHomeUnknown] when the database says
/// nothing.
/// A path too long for a socket's address goes in this user's own
/// temporary folder, as the native side reads it (confstr, as factoryd
/// does), never `TMPDIR`.
Future<String> localClaktorySocket({
  bool dev = showsDevClaktory,
  Future<String?> Function()? userHome,
  Future<String?> Function()? userTemp,
}) async {
  final home = await localClaktoryHome(dev: dev, userHome: userHome);
  if (dev) return localSocketPath(home);
  final path = '$home/app.sock';
  if (utf8.encode(path).length <= _socketPathMost) return path;
  final temp = await _ask('userTemp', userTemp);
  if (temp == null || !temp.startsWith('/')) throw const LocalHomeUnknown();
  return localSocketPath(home, temp: temp);
}

/// The data folder of the Claktory this app shows by itself, found as
/// [localClaktorySocket] finds its socket: Claktory Dev's [localHome], or
/// `.factory` in the person's home folder as the user database has it.
Future<String> localClaktoryHome({
  bool dev = showsDevClaktory,
  Future<String?> Function()? userHome,
}) async {
  if (dev) return localHome();
  final home = await _ask('userHome', userHome);
  if (home == null || !home.startsWith('/')) throw const LocalHomeUnknown();
  return '$home/.factory';
}

Future<String?> _ask(String method, Future<String?> Function()? given) async {
  try {
    return await (given ?? () => _device.invokeMethod<String>(method))();
  } on MissingPluginException {
    return null;
  }
}

/// Where [home]'s backend listens for this app: `<home>/app.sock`, or, when
/// that's longer than a socket's address can be, `claktory-<the first 16
/// hex digits of the home path's SHA-256>/app.sock` in the user's own
/// temporary folder ([temp], `$TMPDIR`: confstr's DARWIN_USER_TEMP_DIR for
/// an app outside the sandbox, as this one is).
String localSocketPath(String home, {String? temp}) {
  final path = '$home/app.sock';
  if (utf8.encode(path).length <= _socketPathMost) return path;
  final hash = sha256.convert(utf8.encode(home)).toString().substring(0, 16);
  var folder = temp ?? Platform.environment['TMPDIR'] ?? '';
  if (folder.isEmpty) folder = Directory.systemTemp.path;
  if (folder.endsWith('/')) folder = folder.substring(0, folder.length - 1);
  return '$folder/claktory-$hash/app.sock';
}

/// The address a WebSocket asks for over the socket: the backend ignores
/// its path.
const localSocketUrl = 'ws://factory/';

/// Checks who listens at the socket, connected but before a word goes to
/// it: null to go on, else why not, for the log.
typedef LocalPeerCheck = Future<String?> Function(Socket socket);

/// What listens at the socket isn't this Mac's own Claktory backend
/// ([checkLocalPeer]), so nothing was said to it.
class LocalPeerRefused implements Exception {
  const LocalPeerRefused(this.why);

  /// Why, in a few words for the log.
  final String why;

  @override
  String toString() => 'LocalPeerRefused: $why';
}

/// SOL_LOCAL's LOCAL_PEERCRED (an xucred) and LOCAL_PEERTOKEN (an
/// audit_token_t), macOS's sys/un.h.
const _solLocal = 0;
const _localPeerCred = 0x001;
const _localPeerToken = 0x006;

const _device = MethodChannel('factory/device');

/// The Mac app's check of the backend at the other end of [socket]:
/// another program of the person's that took the socket's place could
/// otherwise read what they type in the app. The peer's credentials and
/// audit token, read off the connection, go to the native side
/// (BackendPeer.swift): it must run as this user, and for Claktory be
/// Claktory's own backend, signed with this app's own Developer ID team;
/// Claktory Dev's backend is ad hoc, so only its user counts there.
Future<String?> checkLocalPeer(Socket socket) async {
  final Uint8List credentials;
  final Uint8List token;
  try {
    credentials = socket.getRawOption(
      RawSocketOption(_solLocal, _localPeerCred, Uint8List(76)),
    );
    token = socket.getRawOption(
      RawSocketOption(_solLocal, _localPeerToken, Uint8List(32)),
    );
  } on SocketException catch (error) {
    return "its credentials can't be read (${error.osError?.message})";
  }
  try {
    return await _device.invokeMethod<String>('checkBackendPeer', {
      'credentials': credentials,
      'token': token,
      'dev': showsDevClaktory,
    });
  } on MissingPluginException {
    return "this app can't check it";
  } on PlatformException catch (error) {
    return "it couldn't be checked (${error.message})";
  }
}

/// An HTTP client whose every connection is the Unix socket at [path], for
/// [WebSocket.connect]'s customClient, each checked with [check] before
/// anything is sent on it; one that fails it is closed, and the connection
/// fails with [LocalPeerRefused].
/// Always a real client, whatever [HttpOverrides] are in force (a widget
/// test's mock can't reach a socket).
HttpClient localSocketClient(
  String path, {
  LocalPeerCheck check = checkLocalPeer,
}) {
  final address = InternetAddress(path, type: InternetAddressType.unix);
  return HttpOverrides.runWithHttpOverrides(HttpClient.new, _RealHttp())
    ..connectionFactory = (uri, proxyHost, proxyPort) async {
      final task = await Socket.startConnect(address, 0);
      return ConnectionTask.fromSocket(
        task.socket.then((socket) async {
          final why = await check(socket);
          if (why == null) return socket;
          socket.destroy();
          throw LocalPeerRefused(why);
        }),
        task.cancel,
      );
    };
}

class _RealHttp extends HttpOverrides {}

/// Opens a WebSocket to this Mac's own Claktory over its socket, once
/// [check] says it's Claktory's own backend.
Future<WebSocket> connectLocalSocket({
  String? path,
  LocalPeerCheck check = checkLocalPeer,
}) async => WebSocket.connect(
  localSocketUrl,
  customClient: localSocketClient(
    path ?? await localClaktorySocket(),
    check: check,
  ),
);
