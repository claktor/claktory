import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:url_launcher/url_launcher.dart';

/// The install page stays put when a release's APK filename changes. It is
/// usable even when the version manifest is unavailable (including lockout).
final androidAppInstallPage = Uri.https('claktory.com', '/android');
final androidAppUpdateManifest = Uri.https(
  'claktory.com',
  '/android/latest.json',
);

const _maxVersionCode = 2100000000;
const _productionBuild = kReleaseMode && !bool.fromEnvironment('FACTORY_DEV');

/// Validated public release metadata, not an installed or downloaded APK.
/// Opening [page] does not certify a download's bytes or install anything.
@immutable
class AndroidAppUpdate {
  const AndroidAppUpdate._({
    required this.version,
    required this.versionCode,
    required this.minServer,
    required this.page,
    required this.apk,
    required this.size,
    required this.sha256,
    required this.certSha256,
  });

  final String version;
  final int versionCode;
  final String minServer;
  final Uri page;
  final Uri apk;
  final int size;
  final String sha256;
  final String certSha256;

  /// The app's compatibility line must match as well as its verified floor.
  /// A higher version number alone does not promise cross-line compatibility.
  bool supportsBackend(String backendVersion) {
    final backend = _version(backendVersion);
    final app = _version(version)!;
    final floor = _version(minServer)!;
    return backend != null &&
        backend[0] == app[0] &&
        backend[1] == app[1] &&
        _compare(backend, floor) >= 0;
  }
}

List<BigInt>? _version(String value) {
  final match = RegExp(r'^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$')
      .firstMatch(value);
  return match == null
      ? null
      : [for (var i = 1; i <= 3; i++) BigInt.parse(match.group(i)!)];
}

int _compare(List<BigInt> a, List<BigInt> b) {
  for (var i = 0; i < 3; i++) {
    final compared = a[i].compareTo(b[i]);
    if (compared != 0) return compared;
  }
  return 0;
}

bool _safeClaktoryUrl(Uri uri) =>
    uri.scheme == 'https' &&
    uri.host == 'claktory.com' &&
    uri.port == 443 &&
    uri.userInfo.isEmpty &&
    !uri.hasFragment;

bool _installPage(Uri uri) =>
    _safeClaktoryUrl(uri) && uri.path == '/android' && !uri.hasQuery;

/// An absent, malformed or incomplete manifest is unknown, never an offer.
/// Additional fields are deliberately ignored. All download receipt fields
/// are retained, but this adapter opens the install page, not the APK itself.
AndroidAppUpdate? parseAndroidAppUpdate(String? text) {
  if (text == null) return null;
  try {
    final value = jsonDecode(text);
    if (value is! Map<String, dynamic>) return null;
    final version = value['version'];
    final versionCode = value['version_code'];
    final minServer = value['min_server'];
    final pageText = value['page'];
    final apkText = value['apk'];
    final size = value['size'];
    final sha256 = value['sha256'];
    final certificate = value['cert_sha256'];
    if (version is! String ||
        _version(version) == null ||
        minServer is! String ||
        _version(minServer) == null ||
        versionCode is! int ||
        versionCode <= 0 ||
        versionCode > _maxVersionCode ||
        pageText is! String ||
        apkText is! String ||
        size is! int ||
        size <= 0 ||
        value['package'] != 'com.claktory.app' ||
        sha256 is! String ||
        certificate is! String) {
      return null;
    }
    final hex = RegExp(r'^[0-9a-fA-F]{64}$');
    if (!hex.hasMatch(sha256) || !hex.hasMatch(certificate)) return null;
    final page = Uri.tryParse(pageText);
    final apk = Uri.tryParse(apkText);
    if (page == null ||
        apk == null ||
        !_installPage(page) ||
        !_safeClaktoryUrl(apk) ||
        !apk.path.endsWith('.apk')) {
      return null;
    }
    return AndroidAppUpdate._(
      version: version,
      versionCode: versionCode,
      minServer: minServer,
      page: page,
      apk: apk,
      size: size,
      sha256: sha256.toLowerCase(),
      certSha256: certificate.toLowerCase(),
    );
  } on FormatException {
    return null;
  }
}

/// Whether a newer installed build is available without making the currently
/// shown Claktory incompatible. Rebuilds compare by the actual Android code,
/// not a code guessed from the app's displayed version. Unknown stays quiet.
AndroidAppUpdate? androidAppUpdate({
  required AndroidAppUpdate? latest,
  required int? installedVersionCode,
  required String backendVersion,
}) =>
    latest != null &&
        installedVersionCode != null &&
        installedVersionCode > 0 &&
        installedVersionCode <= _maxVersionCode &&
        latest.versionCode > installedVersionCode &&
        latest.supportsBackend(backendVersion)
    ? latest
    : null;

/// The native package's real version code (including a release's rebuild).
/// Older hosts and an unavailable PackageManager are both unknown.
Future<int?> installedAndroidAppVersionCode({
  MethodChannel channel = const MethodChannel('factory/device'),
}) async {
  try {
    final answer = await channel.invokeMethod<Object?>('appVersionCode');
    return answer is int && answer > 0 && answer <= _maxVersionCode
        ? answer
        : null;
  } on MissingPluginException {
    return null;
  } on PlatformException {
    return null;
  }
}

/// Fetches only the public metadata endpoint, with a bounded response and no
/// redirects. The optional client factory makes the HTTP contract testable.
Future<String?> loadAndroidAppUpdateManifest({
  HttpClient Function()? client,
}) async {
  const limit = 64 * 1024;
  const timeout = Duration(seconds: 10);
  final http = (client ?? HttpClient.new)()..connectionTimeout = timeout;
  try {
    final request = await http
        .getUrl(androidAppUpdateManifest)
        .timeout(timeout);
    request.followRedirects = false;
    request.headers.set(HttpHeaders.acceptHeader, 'application/json');
    final response = await request.close().timeout(timeout);
    if (response.statusCode != HttpStatus.ok ||
        response.contentLength > limit ||
        response.headers.contentType?.mimeType != 'application/json') {
      return null;
    }
    Future<String?> readBody() async {
      final bytes = <int>[];
      await for (final chunk in response) {
        if (bytes.length + chunk.length > limit) return null;
        bytes.addAll(chunk);
      }
      return utf8.decode(bytes);
    }

    return await readBody().timeout(timeout);
  } on Exception {
    return null;
  } finally {
    http.close(force: true);
  }
}

/// A screen-owned, transient check, refreshed at launch/return by its host.
/// No polling, persistent cache, backend update action or APK installer.
/// A failed refresh clears an offer; it never invents a release or permission.
class AndroidAppUpdates extends ChangeNotifier {
  AndroidAppUpdates({
    Future<String?> Function()? loadManifest,
    Future<int?> Function()? readVersionCode,
    bool? enabled,
  }) : _load = loadManifest ?? loadAndroidAppUpdateManifest,
       _read = readVersionCode ?? installedAndroidAppVersionCode,
       enabled =
           enabled ??
           (_productionBuild &&
               defaultTargetPlatform == TargetPlatform.android);

  final bool enabled;
  final Future<String?> Function() _load;
  final Future<int?> Function() _read;
  AndroidAppUpdate? _latest;
  int? _installedVersionCode;
  bool _disposed = false;
  int _generation = 0;

  AndroidAppUpdate? get latest => _latest;
  int? get installedVersionCode => _installedVersionCode;

  Future<void> refresh() async {
    if (!enabled || _disposed) return;
    final generation = ++_generation;
    Future<T?> unknownOnFailure<T>(Future<T?> Function() read) async {
      try {
        return await read();
      } on Exception {
        return null;
      }
    }

    final code = unknownOnFailure(_read);
    final manifest = await unknownOnFailure(_load);
    final installed = await code;
    if (_disposed || generation != _generation) return;
    _latest = parseAndroidAppUpdate(manifest);
    _installedVersionCode = installed;
    notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _generation++;
    super.dispose();
  }
}

/// Opens only the human install page in the browser. A lockout can use the
/// stable page without knowing the latest release. No direct APK download.
Future<bool> openAndroidAppUpdatePage({
  Uri? page,
  Future<bool> Function(Uri)? open,
}) async {
  final target = page ?? androidAppInstallPage;
  if (!_installPage(target)) return false;
  try {
    return await (open ??
        (uri) => launchUrl(uri, mode: LaunchMode.externalApplication))(target);
  } on Exception {
    return false;
  }
}
