import 'dart:async';
import 'dart:convert' show Utf8Decoder;
import 'dart:math' as math;
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../explorer.dart' show openFile;
import '../identity.dart';
import '../markdown.dart' show MarkdownFiles;
import '../layout.dart' show viewerLeaf;
import '../mobile/metrics.dart';
import '../mobile/navigation.dart' show PhoneRoute, PhoneRoutes;
import '../mobile/viewer_chrome.dart' show ViewerScreen;
import '../pane_chrome.dart' show PaneChrome, PhonePane;
import '../mobile/presentation.dart' show SheetAction, showActions;
import '../paced.dart' show FilePreviews, lostWhileReading;
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import '../share_files.dart'
    show
        canDownloadFiles,
        canShareFiles,
        downloadFiles,
        downloadLabel,
        shareFrom;
import '../visible_codes.dart' show shownName;
import 'registry.dart';

/// The pictures a Markdown file shows in place, by extension: what
/// every app's decoder reads. Anything else it points to is a link, never
/// read until the person opens it.
const markdownPictureTypes = {'png', 'jpg', 'jpeg', 'gif', 'webp'};

/// The videos a Markdown file shows in place, as a card that opens them.
const markdownVideoTypes = {'mp4', 'mov', 'm4v'};

/// The most a `data:` picture may hold, decoded; a bigger one shows as its
/// words.
const markdownDataLimit = 5 * 1024 * 1024;

/// The pictures and videos of the Markdown file at [file]: each
/// `![alt](src)` with a path to one (beside the file, or absolute) shows
/// it, read as its viewer reads it; one to another file is a link that
/// opens it. Remote ones are never fetched: [Markdown] makes them links.
class FileMarkdownMedia implements MarkdownFiles {
  const FileMarkdownMedia(this.file, {this.reloads});

  /// The Markdown file's path.
  final String file;

  /// Ticks when the Markdown file is read again (Reload, or it changed):
  /// its pictures and videos are checked again then too.
  final Listenable? reloads;

  @override
  Widget? embed(String src, String alt) {
    if (src.trimLeft().toLowerCase().startsWith('data:')) {
      return dataPicture(src) == null
          ? null
          : MarkdownDataPicture(key: ValueKey(src), src: src, alt: alt);
    }
    final path = markdownPath(file, src);
    if (path == null) return null;
    return switch (markdownMediaKind(path)) {
      MarkdownMediaKind.picture => MarkdownPicture(
        key: ValueKey(path),
        path: path,
        alt: alt,
        reloads: reloads,
      ),
      MarkdownMediaKind.video => MarkdownVideo(
        key: ValueKey(path),
        path: path,
        alt: alt,
        reloads: reloads,
      ),
      null => null,
    };
  }

  @override
  VoidCallback? opener(BuildContext context, String src) {
    final path = markdownPath(file, src);
    if (path == null) return null;
    return () => openFromMarkdown(context, path);
  }
}

/// Opens [path], which a Markdown file shows or links to: in its own pane
/// beside the document on a Mac or iPad; on a phone, as a page over the
/// document, so Back comes back to it where it was,
/// rather than in the agent's layout in the document's place.
void openFromMarkdown(BuildContext context, String path) {
  final routes = PhoneRoutes.of(context);
  if (routes == null) return openFile(context, path);
  unawaited(
    routes.push<void>(
      PhoneRoute(
        id: 'markdown-file:$path',
        edgeToEdge: deviceIdiom(context) != Idiom.pad,
        builder: (context) => PaneChrome(
          phone: PhonePane(title: _nameOf(path), onBack: routes.pop),
          child: Builder(
            builder: (context) => viewerBody(context, viewerLeaf(path).leaf),
          ),
        ),
      ),
    ),
  );
}

enum MarkdownMediaKind { picture, video }

/// Whether [path] is a picture or a video a Markdown file shows in place,
/// by its extension; null for anything else.
MarkdownMediaKind? markdownMediaKind(String path) {
  final name = path.split(RegExp(r'[/\\]')).last;
  final dot = name.lastIndexOf('.');
  if (dot < 0) return null;
  final extension = name.substring(dot + 1).toLowerCase();
  if (markdownPictureTypes.contains(extension)) {
    return MarkdownMediaKind.picture;
  }
  if (markdownVideoTypes.contains(extension)) return MarkdownMediaKind.video;
  return null;
}

/// The file [src] names in the Markdown file at [file]: beside it (its
/// folder, then `..` and the rest), absolute, or from the home folder
/// ("~/…", which the backend reads as such). Null for an address on the
/// web or of any other kind, and for one that climbs past the root.
///
/// A file on Windows (`C:\…`, or a share's `\\server\share\…`) names its
/// pictures as Windows does, exactly as the backend's
/// `annotations::embeds::markdown_path`: `\` and `/` both separate, a
/// drive (`D:\…`, `D:/…`, never a one-letter scheme) is absolute, `\…` is
/// on the file's own drive, `~` comes back as `~/` and the rest with `\`.
/// A share is only ever the file's own: reading another computer's sends
/// it this user's sign-in (NTLM), so a file naming one, `\\?\…` or
/// `\\.\…` names nothing.
String? markdownPath(String file, String src) {
  final windows = _windowsRoot(file) != null;
  var target = src.trim();
  if (target.isEmpty || target.startsWith('#')) return null;
  final scheme = RegExp(r'^([a-zA-Z][a-zA-Z0-9+.-]*):').firstMatch(target);
  if (scheme != null && !(windows && scheme.group(1)!.length == 1)) {
    if (scheme.group(1)!.toLowerCase() != 'file') return null;
    final uri = Uri.tryParse(target);
    if (uri == null || uri.path.isEmpty) return null;
    target = uri.path;
    // file:///C:/x.png is C:/x.png.
    if (windows &&
        target.startsWith('/') &&
        _windowsRoot(target.substring(1)) != null) {
      target = target.substring(1);
    }
  } else {
    // Its anchor or query is no part of the file's name.
    final cut = target.indexOf(RegExp('[?#]'));
    if (cut >= 0) target = target.substring(0, cut);
  }
  try {
    target = Uri.decodeComponent(target);
  } on ArgumentError {
    // A stray "%": the name as written.
  }
  if (target.isEmpty) return null;
  if (windows) return _windowsPath(file, target);
  final String joined;
  if (target.startsWith('/') || target == '~' || target.startsWith('~/')) {
    joined = target;
  } else {
    final slash = file.lastIndexOf('/');
    joined = '${slash < 0 ? '' : file.substring(0, slash)}/$target';
  }
  final home = joined.startsWith('~');
  final parts = <String>[];
  for (final part in joined.split('/').skip(home ? 1 : 0)) {
    if (part.isEmpty || part == '.') continue;
    if (part == '..') {
      if (parts.isEmpty) return null;
      parts.removeLast();
      continue;
    }
    parts.add(part);
  }
  if (parts.isEmpty) return null;
  return '${home ? '~' : ''}/${parts.join('/')}';
}

/// A Windows path's root and the rest: `C:` for `C:\…` or `C:/…`, and
/// `\\server\share` for a share (either separator); null otherwise.
(String, String)? _windowsRoot(String path) {
  if (RegExp(r'^[a-zA-Z]:[\\/]').hasMatch(path)) {
    return (path.substring(0, 2), path.substring(2));
  }
  final String rest;
  if (path.startsWith(r'\\') || path.startsWith('//')) {
    rest = path.substring(2);
  } else {
    return null;
  }
  final server = rest.indexOf(RegExp(r'[\\/]'));
  if (server <= 0) return null;
  // `\\?\…` and `\\.\…` aren't shares.
  final name = rest.substring(0, server);
  if (name == '?' || name == '.') return null;
  final share = rest.substring(server + 1);
  var end = share.indexOf(RegExp(r'[\\/]'));
  if (end < 0) end = share.length;
  if (end == 0) return null;
  final root = 2 + server + 1 + end;
  return (path.substring(0, root), path.substring(root));
}

/// [target] in the Markdown file at [file] on Windows: see [markdownPath].
String? _windowsPath(String file, String target) {
  final (fileRoot, fileRest) = _windowsRoot(file)!;
  final String root;
  final bool home;
  final String rest;
  if (_windowsRoot(target) case (final named, final after)) {
    // Reading another computer's share sends it this user's sign-in
    // (NTLM): only the shown file's own share, never one a file names.
    final share = named.startsWith(RegExp(r'[\\/]'));
    if (share && !_sameRoot(named, fileRoot)) return null;
    // The file's own share, named as the file names it.
    root = share ? fileRoot : named;
    home = false;
    rest = after;
  } else if (target.startsWith(r'\\') ||
      target.startsWith('//') ||
      target.startsWith(r'\/') ||
      target.startsWith(r'/\')) {
    // `\\?\…`, `\\.\…` or a share with no name: not a picture's place.
    return null;
  } else if (target == '~' ||
      target.startsWith('~/') ||
      target.startsWith(r'~\')) {
    root = '';
    home = true;
    rest = target.substring(1);
  } else if (target.startsWith(RegExp(r'[\\/]'))) {
    root = fileRoot;
    home = false;
    rest = target;
  } else {
    final slash = fileRest.lastIndexOf(RegExp(r'[\\/]'));
    root = fileRoot;
    home = false;
    rest = '${slash < 0 ? '' : fileRest.substring(0, slash)}\\$target';
  }
  final parts = <String>[];
  for (final part in rest.split(RegExp(r'[\\/]'))) {
    if (part.isEmpty || part == '.') continue;
    if (part == '..') {
      if (parts.isEmpty) return null;
      parts.removeLast();
      continue;
    }
    parts.add(part);
  }
  if (parts.isEmpty) return null;
  // `~/` as the backend's watcher expands it, then Windows' own `\`.
  return home ? '~/${parts.join(r'\')}' : '$root\\${parts.join(r'\')}';
}

/// Whether two Windows roots (`C:`, `\\server\share`) are the same, as
/// Windows compares them: case aside, `\` and `/` alike.
bool _sameRoot(String a, String b) {
  String plain(String root) => root.replaceAll('/', r'\').toLowerCase();
  return plain(a) == plain(b);
}

/// The picture a `data:` address holds: its bytes, when it's a picture
/// every app reads (PNG, JPEG, GIF, WebP; never SVG, which could load
/// more) of at most [markdownDataLimit]; null otherwise.
Uint8List? dataPicture(String src) {
  final text = src.trim();
  // Base64 is 4 characters for 3 bytes: anything longer is too big,
  // known before decoding it.
  if (text.length > markdownDataLimit * 4 ~/ 3 + 256) return null;
  final UriData data;
  try {
    data = UriData.parse(text);
  } on FormatException {
    return null;
  }
  const types = {'image/png', 'image/jpeg', 'image/gif', 'image/webp'};
  if (!types.contains(data.mimeType.toLowerCase())) return null;
  final Uint8List bytes;
  try {
    bytes = data.contentAsBytes();
  } on FormatException {
    return null;
  }
  if (bytes.length > markdownDataLimit || pictureType(bytes) == null) {
    return null;
  }
  return bytes;
}

/// What picture [bytes] begin as ("png", "jpeg", "gif", "webp"), by their
/// first bytes; null when they're none of them, so they're never kept on
/// the device as one.
String? pictureType(List<int> bytes) {
  bool at(int offset, List<int> signature) {
    if (bytes.length < offset + signature.length) return false;
    for (var i = 0; i < signature.length; i++) {
      if (bytes[offset + i] != signature[i]) return false;
    }
    return true;
  }

  if (at(0, const [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])) {
    return 'png';
  }
  if (at(0, const [0xFF, 0xD8, 0xFF])) return 'jpeg';
  if (at(0, 'GIF87a'.codeUnits) || at(0, 'GIF89a'.codeUnits)) return 'gif';
  if (at(0, 'RIFF'.codeUnits) && at(8, 'WEBP'.codeUnits)) return 'webp';
  return null;
}

/// Whether [bytes] begin as an MP4 or QuickTime movie.
bool looksLikeVideo(List<int> bytes) {
  if (bytes.length < 12) return false;
  final box = const Utf8Decoder(allowMalformed: true)
      .convert(bytes.sublist(4, 8));
  if (box == 'ftyp') {
    final brand = const Utf8Decoder(allowMalformed: true)
        .convert(bytes.sublist(8, 12));
    // HEIF pictures are boxes of the same kind.
    return !const {'heic', 'heix', 'mif1', 'msf1', 'avif'}.contains(brand);
  }
  return const {'moov', 'mdat', 'wide', 'free', 'skip'}.contains(box);
}

/// The file's name as the person reads it.
String _nameOf(String path) {
  final windows =
      _windowsRoot(path) != null ||
      (path.startsWith('~/') && path.contains(r'\'));
  return shownName(
    path
            .split(windows ? RegExp(r'[\\/]') : RegExp('/'))
            .where((part) => part.isNotEmpty)
            .lastOrNull ??
        path,
  );
}

/// A picture in a Markdown file, read as the image viewer reads it (from
/// this device's copy when it's still the file, as a preview made for a
/// phone's screen, again when it changes), at its own size but never
/// wider than the text. A click opens it in its own pane; a right click,
/// or a touch and hold, offers Open, Download (on a Mac) and Share….
class MarkdownPicture extends StatefulWidget {
  const MarkdownPicture({
    super.key,
    required this.path,
    required this.alt,
    this.reloads,
  });

  final String path;
  final String alt;

  /// Ticks when it should be read again (its Markdown file was).
  final Listenable? reloads;

  @override
  State<MarkdownPicture> createState() => _MarkdownPictureState();
}

class _MarkdownPictureState extends State<MarkdownPicture>
    with FileLoading<MarkdownPicture> {
  @override
  String get path => widget.path;

  /// A GIF comes whole, so it moves; other pictures may come as a copy
  /// made for this screen, as the image viewer's do.
  @override
  bool get previews => !path.toLowerCase().endsWith('.gif');

  @override
  bool keeps(FileContent read) => pictureType(read.data) != null;

  @override
  void initState() {
    super.initState();
    widget.reloads?.addListener(_again);
  }

  @override
  void didUpdateWidget(MarkdownPicture old) {
    super.didUpdateWidget(old);
    if (old.reloads != widget.reloads) {
      old.reloads?.removeListener(_again);
      widget.reloads?.addListener(_again);
    }
  }

  @override
  void dispose() {
    widget.reloads?.removeListener(_again);
    super.dispose();
  }

  void _again() => unawaited(reload());

  /// The version shown ([FileContent.sha256]), so the same one read again
  /// isn't decoded again.
  var _shown = '';

  Uint8List? _bytes;

  /// Its size where it shows, in its own pixels: the original's, also
  /// while a preview of it shows.
  Size? _size;

  /// How wide the bytes read are, in pixels (a preview's are fewer).
  var _pixels = 0.0;

  /// It was read but isn't a picture this app can show.
  var _undecodable = false;

  @override
  void loaded(FileContent file) {
    if (file.error.isNotEmpty) return;
    if (file.truncated || pictureType(file.data) == null) {
      setState(() => _undecodable = true);
      return;
    }
    final version = '${file.sha256}:${file.isPreview}';
    if (file.sha256.isNotEmpty && version == _shown && _bytes != null) return;
    _shown = version;
    unawaited(_measure(file, Uint8List.fromList(file.data)));
  }

  Future<void> _measure(FileContent file, Uint8List bytes) async {
    try {
      final buffer = await ui.ImmutableBuffer.fromUint8List(bytes);
      final descriptor = await ui.ImageDescriptor.encoded(buffer);
      final size =
          file.isPreview &&
              file.originalPixelWidth > 0 &&
              file.originalPixelHeight > 0
          ? Size(
              file.originalPixelWidth.toDouble(),
              file.originalPixelHeight.toDouble(),
            )
          : Size(descriptor.width.toDouble(), descriptor.height.toDouble());
      final pixels = descriptor.width.toDouble();
      descriptor.dispose();
      buffer.dispose();
      if (!mounted) return;
      setState(() {
        _undecodable = size.isEmpty;
        _bytes = bytes;
        _size = size;
        _pixels = pixels;
      });
    } on Exception {
      if (mounted) setState(() => _undecodable = true);
    }
  }

  @override
  Widget build(BuildContext context) {
    final file = this.file;
    final failure = this.failure;
    final waitsForConnection =
        failure != null &&
        failure != lostWhileReading &&
        lostWithConnection(failure);
    if (_undecodable ||
        (failure != null && !waitsForConnection) ||
        (file != null && file.error.isNotEmpty)) {
      return MarkdownMediaProblem(alt: widget.alt, name: _nameOf(path));
    }
    final bytes = _bytes;
    final size = _size;
    if (bytes == null || size == null) {
      return _Loading(name: _nameOf(path));
    }
    return _MediaPress(
      path: path,
      label: widget.alt.isEmpty ? _nameOf(path) : widget.alt,
      child: _Sized(
        size: size,
        builder: (width, height) => Image.memory(
          bytes,
          width: width,
          height: height,
          cacheWidth: _decodeWidth(context, width, _pixels),
          fit: BoxFit.fill,
          gaplessPlayback: true,
          filterQuality: FilterQuality.medium,
          errorBuilder: (context, error, stack) =>
              MarkdownMediaProblem(alt: widget.alt, name: _nameOf(path)),
        ),
      ),
    );
  }
}

/// A picture written into the Markdown itself (a `data:` address, at most
/// [markdownDataLimit]): shown as [MarkdownPicture] shows one, with no
/// file to open.
class MarkdownDataPicture extends StatefulWidget {
  const MarkdownDataPicture({super.key, required this.src, required this.alt});

  final String src;
  final String alt;

  @override
  State<MarkdownDataPicture> createState() => _MarkdownDataPictureState();
}

class _MarkdownDataPictureState extends State<MarkdownDataPicture> {
  Uint8List? _bytes;
  Size? _size;
  var _undecodable = false;

  @override
  void initState() {
    super.initState();
    final bytes = dataPicture(widget.src);
    if (bytes == null) {
      _undecodable = true;
    } else {
      unawaited(_measure(bytes));
    }
  }

  Future<void> _measure(Uint8List bytes) async {
    try {
      final buffer = await ui.ImmutableBuffer.fromUint8List(bytes);
      final descriptor = await ui.ImageDescriptor.encoded(buffer);
      final size = Size(
        descriptor.width.toDouble(),
        descriptor.height.toDouble(),
      );
      descriptor.dispose();
      buffer.dispose();
      if (!mounted) return;
      setState(() {
        _undecodable = size.isEmpty;
        _bytes = bytes;
        _size = size;
      });
    } on Exception {
      if (mounted) setState(() => _undecodable = true);
    }
  }

  @override
  Widget build(BuildContext context) {
    final bytes = _bytes;
    final size = _size;
    if (_undecodable) {
      return MarkdownMediaProblem(alt: widget.alt, name: 'this picture');
    }
    if (bytes == null || size == null) return const SizedBox.shrink();
    return Semantics(
      image: true,
      label: widget.alt,
      child: _Sized(
        size: size,
        builder: (width, height) => Image.memory(
          bytes,
          width: width,
          height: height,
          cacheWidth: _decodeWidth(context, width, size.width),
          fit: BoxFit.fill,
          gaplessPlayback: true,
          filterQuality: FilterQuality.medium,
          errorBuilder: (context, error, stack) =>
              MarkdownMediaProblem(alt: widget.alt, name: 'this picture'),
        ),
      ),
    );
  }
}

/// A video in a Markdown file: a card with Play and its name, which opens
/// it in the video viewer to play. Only its first bytes are read, to know
/// it's there and is a video; the video itself comes when it's opened.
class MarkdownVideo extends StatefulWidget {
  const MarkdownVideo({
    super.key,
    required this.path,
    required this.alt,
    this.reloads,
  });

  final String path;
  final String alt;

  /// Ticks when it should be checked again (its Markdown file was read).
  final Listenable? reloads;

  @override
  State<MarkdownVideo> createState() => _MarkdownVideoState();
}

class _MarkdownVideoState extends State<MarkdownVideo> {
  /// Null while its first bytes are on their way; whether they're a video.
  bool? _found;

  @override
  void initState() {
    super.initState();
    widget.reloads?.addListener(_again);
    unawaited(_check());
  }

  @override
  void didUpdateWidget(MarkdownVideo old) {
    super.didUpdateWidget(old);
    if (old.reloads != widget.reloads) {
      old.reloads?.removeListener(_again);
      widget.reloads?.addListener(_again);
    }
  }

  @override
  void dispose() {
    widget.reloads?.removeListener(_again);
    super.dispose();
  }

  void _again() => unawaited(_check());

  Future<void> _check() async {
    try {
      final read = await AppScope.of(context)
          .readFile(widget.path, maxBytes: 64);
      if (!mounted) return;
      setState(() => _found = read.error.isEmpty && looksLikeVideo(read.data));
    } on Exception {
      if (mounted) setState(() => _found = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final name = _nameOf(widget.path);
    final found = _found;
    if (found == false) {
      return MarkdownMediaProblem(alt: widget.alt, name: name);
    }
    if (found == null) return _Loading(name: name);
    final label = widget.alt.isEmpty ? name : widget.alt;
    return _MediaPress(
      path: widget.path,
      label: 'Play $label',
      child: _Sized(
        size: const Size(480, 270),
        builder: (width, height) => Container(
          width: width,
          height: height,
          decoration: BoxDecoration(
            color: palette.sidebar,
            borderRadius: BorderRadius.circular(Radii.panel),
            border: Border.all(color: palette.hairline),
          ),
          padding: const EdgeInsets.all(Space.m),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Container(
                width: 56,
                height: 56,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: palette.canvas,
                  border: Border.all(color: palette.hairline),
                ),
                alignment: Alignment.center,
                child: ExtensionGlyph(
                  icon: 'play',
                  name: 'Play',
                  color: palette.text,
                  size: 24,
                ),
              ),
              const SizedBox(height: Space.s),
              Text(
                label,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
                style: type.caption.copyWith(color: palette.textMuted),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A picture or video that can't show: its words, if it has any, and a
/// quiet "Can't show" and its name under them.
class MarkdownMediaProblem extends StatelessWidget {
  const MarkdownMediaProblem({
    super.key,
    required this.alt,
    required this.name,
  });

  final String alt;
  final String name;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        if (alt.trim().isNotEmpty)
          Text(alt, style: type.body.copyWith(color: palette.text)),
        Text(
          "Can't show $name",
          style: type.caption.copyWith(color: palette.textFaint),
        ),
      ],
    );
  }
}

/// While a picture's on its way: its name, faint.
class _Loading extends StatelessWidget {
  const _Loading({required this.name});

  final String name;

  @override
  Widget build(BuildContext context) => Text(
    'Loading $name…',
    style: TypeScale.of(context).caption
        .copyWith(color: Palette.of(context).textFaint),
  );
}

/// [size] (a picture's own pixels) at most as wide as the text, in
/// proportion, at the start of the line.
class _Sized extends StatelessWidget {
  const _Sized({required this.size, required this.builder});

  final Size size;
  final Widget Function(double width, double height) builder;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final room = constraints.maxWidth;
      var width = room.isFinite ? math.min(size.width, room) : size.width;
      var height = width * size.height / size.width;
      // On a phone, never taller than the screen shows under the page's
      // header: a tall screenshot, or any picture on a turned phone, is
      // seen whole, and opens bigger when tapped.
      if (ViewerScreen.wholeOf(context)) {
        final screen = MediaQuery.sizeOf(context).height;
        final safe = MediaQuery.paddingOf(context);
        final most = math.max(
          120.0,
          screen - safe.top - safe.bottom - Touch.target - 2 * Space.l,
        );
        if (height > most) {
          width = width * most / height;
          height = most;
        }
      }
      return Align(
        alignment: AlignmentDirectional.centerStart,
        child: builder(width, height),
      );
    },
  );
}

/// How wide to decode a picture shown [width] wide: no wider than this
/// screen shows it, so a big one doesn't take its whole size in memory;
/// null when it shows at its own size or more.
int? _decodeWidth(BuildContext context, double width, double pixels) {
  final shown = (width * MediaQuery.devicePixelRatioOf(context)).ceil();
  return shown < pixels ? shown : null;
}

/// A picture or video that opens in its own pane when clicked or tapped,
/// and offers Open, Download (on a Mac) and Share… on a right click or a
/// touch and hold.
class _MediaPress extends StatelessWidget {
  const _MediaPress({
    required this.path,
    required this.label,
    required this.child,
  });

  final String path;
  final String label;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    void open() => openFromMarkdown(context, path);
    Future<void> menu() async {
      final chosen = await showActions<String>(
        context,
        title: _nameOf(path),
        actions: [
          const SheetAction('Open', 'open'),
          // On a Mac, into Downloads.
          if (canDownloadFiles) const SheetAction(downloadLabel, 'download'),
          if (canShareFiles) const SheetAction('Share…', 'share'),
        ],
      );
      if (!context.mounted) return;
      switch (chosen) {
        case 'open':
          open();
        case 'download':
          unawaited(downloadFiles(context, [path]));
        case 'share':
          unawaited(shareFrom(context, [path]));
      }
    }

    return Semantics(
      image: true,
      button: true,
      label: label,
      child: GestureDetector(
        onLongPress: () => unawaited(menu()),
        child: Pressable(
          onTap: open,
          onSecondaryTap: () => unawaited(menu()),
          builder: (context, hovered) => child,
        ),
      ),
    );
  }
}
