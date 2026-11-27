// This PC's own Claktory, as the Windows app reaches it (windows.md section
// 3, authority B; Giorno 48383, Fugo 48393): on its loopback port, with the
// app-token its backend writes at every start into <home>\app-token, sent in
// Hello.token. The token goes only to a backend running as this same Windows
// account: before Hello, the app checks who holds the server end of its own
// connection (Rust's loopback_peer_same_user), and sends nothing to anyone
// else, since another account on the PC could take the port first.
import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

import 'tunnel/api/windows.dart' as rust;
import 'tunnel_client.dart' show RustTunnels;

/// Whether this PC's Claktory answers on the app's own connection: the
/// server end of 127.0.0.1:[serverPort] ↔ 127.0.0.1:[clientPort] belongs to
/// a process of this same Windows account. Anything it can't tell is false.
typedef SameUserCheck = Future<bool> Function(int serverPort, int clientPort);

/// What the Windows app needs to reach its own Claktory, each part
/// replaceable in tests.
class WindowsLocal {
  const WindowsLocal({
    this.readToken = readAppToken,
    this.sameUser = loopbackSameUser,
  });

  /// The app-token of the Claktory the app talks to (the dev one's when
  /// [dev]), or null when it hasn't written one.
  final Future<String?> Function({required bool dev}) readToken;

  final SameUserCheck sameUser;
}

/// The app's Rust library answers (app/rust/src/api/windows.rs): exactly
/// one connection there, its owner's SID this account's. It throws on
/// anything short of a sure answer, and off Windows always; either is no.
Future<bool> loopbackSameUser(int serverPort, int clientPort) async {
  try {
    await RustTunnels.load();
    return await rust.loopbackPeerSameUser(
      serverPort: serverPort,
      clientPort: clientPort,
    );
  } on Object {
    return false;
  }
}

/// Where this PC's Claktory keeps its app-token: in its home folder,
/// %LOCALAPPDATA%\Claktory\factory (factory-dev for the dev Claktory, which
/// [dev] builds talk to), or FACTORY_HOME when that's set, as the backend
/// finds its own.
String appTokenPath(Map<String, String> environment, {required bool dev}) {
  final home = environment['FACTORY_HOME'] ?? '';
  if (home.isNotEmpty) return '$home\\app-token';
  final local = environment['LOCALAPPDATA'] ?? '';
  return '$local\\Claktory\\${dev ? 'factory-dev' : 'factory'}\\app-token';
}

const _device = MethodChannel('factory/device');

/// The app-token, read by the runner (app/windows/runner/device_channel.cpp:
/// shared for delete, so the backend can replace it meanwhile, and checked
/// as 64 hex). Null when there's none, or it can't be read: either way the
/// app can't reach its Claktory yet, and tries again.
Future<String?> readAppToken({required bool dev}) async {
  try {
    return await _device.invokeMethod<String>(
      'appToken',
      appTokenPath(Platform.environment, dev: dev),
    );
  } on PlatformException {
    return null;
  } on MissingPluginException {
    return null;
  }
}

/// A WebSocket to [endpoint] (`ws://127.0.0.1:<port>`), with the port its
/// own end has on this PC: what the server end's owner is looked up by.
Future<(WebSocket, int)> connectLoopback(String endpoint) async {
  Socket? opened;
  final client = HttpClient()
    ..connectionFactory = (uri, proxyHost, proxyPort) async {
      final task = await Socket.startConnect(uri.host, uri.port);
      unawaited(
        task.socket.then((socket) => opened = socket, onError: (Object _) {}),
      );
      return task;
    };
  final socket = await WebSocket.connect(endpoint, customClient: client);
  final own = opened;
  if (own == null) {
    await socket.close();
    throw const SocketException("Couldn't tell this connection's own port");
  }
  return (socket, own.port);
}
