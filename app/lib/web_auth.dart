import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

/// The browser step of connecting a service, on the device that clicked
/// Connect (docs: factory-research/one-click-secrets.md, section 3). The
/// service sends the browser back with a code; the app catches that address
/// and hands it to the backend, over the same connection as everything
/// else, so a backend on another machine needs no address of its own.
///
/// Two ways to catch it:
/// - [LoopbackReceiver]: on the Mac, an address on this computer only
///   (`http://127.0.0.1:<port>/…`) that listens for that one visit, while the
///   service's page opens in the default browser.
/// - [webAuth]: the system's sign-in sheet (ASWebAuthenticationSession),
///   which ends when the service sends back to the app's own scheme. The
///   only way on iPhone.

const _channel = MethodChannel('factory/device');

/// Opens [url] in the system's sign-in sheet and answers the address the
/// service sent back to on [scheme], or null when the person closed it.
/// Throws [WebAuthFailure] when the sheet couldn't open or failed.
Future<Uri?> webAuth(Uri url, String scheme) async {
  try {
    final back = await _channel.invokeMethod<String>('webAuth', {
      'url': url.toString(),
      'scheme': scheme,
    });
    return back == null ? null : Uri.parse(back);
  } on PlatformException catch (error) {
    throw WebAuthFailure(error.message ?? "Couldn't open the sign-in page.");
  } on MissingPluginException {
    throw const WebAuthFailure("This device can't open the sign-in page.");
  }
}

/// Closes the sign-in sheet [webAuth] opened; it answers null.
Future<void> cancelWebAuth() async {
  try {
    await _channel.invokeMethod<void>('cancelWebAuth');
  } on MissingPluginException {
    // Nothing open.
  }
}

/// Why the browser step couldn't finish, in words for the person.
class WebAuthFailure implements Exception {
  const WebAuthFailure(this.message);

  final String message;

  @override
  String toString() => message;
}

/// Where a service's page sends the browser back to, listening for it.
abstract interface class ConnectReceiver {
  /// The address to give the service.
  Uri get address;

  /// The address the browser came back to, or null once closed first.
  Future<Uri?> get back;

  Future<void> close();
}

/// An address on this computer that a service's page sends the browser back
/// to: listens on 127.0.0.1 at a port the system picks, takes the first
/// visit to [path] and answers it with a page saying to go back to
/// Claktory, then stops listening. Other visits (a favicon, another path)
/// get 404 and change nothing.
class LoopbackReceiver implements ConnectReceiver {
  LoopbackReceiver._(this._server, this.path) : _port = _server.port {
    _server.listen(_visit, onError: (_) {});
  }

  /// Starts listening. [path] is the part after the port, starting "/";
  /// [port] 0 lets the system pick one. Throws [SocketException] when the
  /// port is taken.
  static Future<LoopbackReceiver> start({
    String path = '/callback',
    int port = 0,
  }) async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, port);
    return LoopbackReceiver._(server, path);
  }

  final HttpServer _server;
  final int _port;
  final String path;
  final _back = Completer<Uri?>();

  @override
  Uri get address =>
      Uri(scheme: 'http', host: '127.0.0.1', port: _port, path: path);

  @override
  Future<Uri?> get back => _back.future;

  Future<void> _visit(HttpRequest request) async {
    final response = request.response;
    if (request.method != 'GET' ||
        request.uri.path != path ||
        _back.isCompleted) {
      response.statusCode = HttpStatus.notFound;
      await response.close();
      return;
    }
    // Only this computer reaches 127.0.0.1, so the address is whole as asked.
    final back = address.replace(query: request.uri.query);
    response.headers
      ..contentType = ContentType.html
      ..set('Cache-Control', 'no-store')
      ..set('Referrer-Policy', 'no-referrer');
    response.write(_page);
    await response.close();
    _back.complete(back);
    await _server.close();
  }

  /// Stops listening; [back] answers null if nothing came.
  @override
  Future<void> close() async {
    if (!_back.isCompleted) _back.complete(null);
    await _server.close(force: true);
  }

  static const _page = '''<!doctype html>
<html><head><meta charset="utf-8"><title>Claktory</title>
<meta name="viewport" content="width=device-width">
<style>
body{font:15px -apple-system,system-ui,sans-serif;display:flex;align-items:center;justify-content:center;height:90vh;margin:0;color:#1d1d1f;background:#fbfbfd}
@media(prefers-color-scheme:dark){body{color:#f5f5f7;background:#1c1c1e}}
</style></head>
<body><p>You can close this tab and go back to Claktory.</p></body></html>''';
}
