// Sharing files from the device (#418, Matt): "my phone already can
// preview things, so there's no reason why it shouldn't be able to share
// it to somebody with AirDrop." A file is on the machine Claktory runs on,
// so the app first fetches the whole original, in pieces sized to the link
// (Backend.readFile's paced reads), with how much has come and Cancel, into
// a folder of its own in the device's temporary folder; then the system's
// share sheet sends it (AirDrop, Messages, Save to Files…; Android's
// chooser; the Mac's share menu), and the folder goes when it closes.
// Folders left by a share cut short go at the next launch
// ([sweepSharedFiles]), so nothing stays on the device.
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'claktory_identity.dart' show claktoryName;
import 'controls.dart';
import 'forms.dart' show FormFrame;
import 'mobile/haptics.dart';
import 'identity.dart' show touchFirst;
import 'mobile/presentation.dart' show SheetAction, showActions;
import 'mobile/share.dart' show globalRectOf;
import 'scope.dart';
import 'viewers/loading_progress.dart';

export 'mobile/share.dart' show globalRectOf;

const _device = MethodChannel('factory/device');

/// How much one read asks for: it comes in pieces sized to the link, but a
/// read given up on (the connection gone over a minute) keeps none of it,
/// so Try again carries on from the last whole one (Abbacchio's row 1024).
const _piece = 1024 * 1024;

/// Where shares are fetched to, a folder for each. On Android the cache
/// folder, which the host's file provider serves (Dart's temporary folder
/// there is the code cache, which it can't, and an update wipes).
Directory get _shares =>
    debugSharesFolder ??
    Directory(
      defaultTargetPlatform == TargetPlatform.android
          ? '${Directory.systemTemp.parent.path}/cache/claktory-share'
          : '${Directory.systemTemp.path}/claktory-share',
    );

/// The folder of shares, as on this device.
@visibleForTesting
Directory get sharesFolder => _shares;

/// A test's own folder of shares in place of the device's, so tests run at
/// once never sweep each other's.
@visibleForTesting
Directory? debugSharesFolder;

/// How long a share that went somewhere is kept: the app it went to may
/// read it well after the sheet answers, for as long as its screen lives
/// (a mail draft, Messages, AirDrop; Risotto), and Copy leaves only where
/// it is on the clipboard, read when it's pasted (Abbacchio's row 1023).
const _keep = Duration(hours: 24);

/// In a share's folder once it went somewhere: kept [_keep] from then,
/// where one without it was cut short (closed, cancelled, the app quit).
const _sentMark = '.sent';

/// Characters that would make a name read as another in the share sheet:
/// controls, and invisible or reordering formatting (Fugo: an agent's file
/// named from web text, `invoice\u202Efdp.exe`).
final _hidden = RegExp(
  '[\\x00-\\x1F\\x7F-\\x9F\\u200B-\\u200F\\u202A-\\u202E\\u2066-\\u2069'
  '\\uFEFF]',
);

/// The name a fetched file has on the device: [path]'s last part without
/// [_hidden], at most 255 bytes with its extension kept, `file` when
/// nothing's left of it (or it's `.` or `..`).
String shareName(String path) {
  final name = path.split('/').last.replaceAll(_hidden, '');
  if (name.isEmpty || name == '.' || name == '..') return 'file';
  if (utf8.encode(name).length <= 255) return name;
  final dot = name.lastIndexOf('.');
  final extension = dot > 0 && utf8.encode(name.substring(dot)).length <= 32
      ? name.substring(dot)
      : '';
  final stem = name.substring(0, name.length - extension.length).runes.toList();
  final room = 255 - utf8.encode(extension).length;
  while (utf8.encode(String.fromCharCodes(stem)).length > room) {
    stem.removeLast();
  }
  return String.fromCharCodes(stem) + extension;
}

/// Whether this device has a share sheet for files: the iPhone, the iPad,
/// Android and the Mac (`factory/device`'s `shareFiles`).
bool get canShareFiles =>
    !kIsWeb &&
    (defaultTargetPlatform == TargetPlatform.iOS ||
        defaultTargetPlatform == TargetPlatform.android ||
        defaultTargetPlatform == TargetPlatform.macOS);

/// The label of the action that shares one file, or [count] of them.
String shareLabel([int count = 1]) =>
    count == 1 ? 'Share…' : 'Share $count files…';

/// What's left of shares goes, at launch: those cut short (the app quit
/// while one was fetched or shown) and those sent over a day ago.
Future<void> sweepSharedFiles() => _sweep(cutShort: Duration.zero);

/// Shares sent more than [_keep] ago go, and those cut short more than
/// [cutShort] ago.
Future<void> _sweep({required Duration cutShort}) async {
  if (kIsWeb) return;
  try {
    final shares = _shares;
    if (!await shares.exists()) return;
    final now = DateTime.now();
    await for (final share in shares.list(followLinks: false)) {
      final mark = File('${share.path}/$_sentMark');
      final sent = share is Directory && await mark.exists();
      final since = (await (sent ? mark : share).stat()).modified;
      if (now.difference(since) < (sent ? _keep : cutShort)) continue;
      await share.delete(recursive: true);
    }
  } on FileSystemException {
    // Nothing there, or another launch got to it first.
  }
}

/// Fetches [paths] from the Claktory and opens the share sheet for them,
/// pointing at [from] (the button or row, in global coordinates) on an
/// iPad and the Mac. What was fetched is deleted when the person cancels
/// or closes the sheet; sent somewhere, it's kept a day for the app it
/// went to ([_keep]). True once they sent them somewhere.
Future<bool> shareFiles(
  BuildContext context,
  List<String> paths, {
  Rect? from,
}) async {
  if (paths.isEmpty || !canShareFiles) return false;
  // A device that's never relaunched doesn't keep old shares (Fugo); one
  // cut short an hour ago isn't still being made.
  await _sweep(cutShort: const Duration(hours: 1));
  if (!context.mounted) return false;
  final folder = Directory(
    '${_shares.path}/${DateTime.now().microsecondsSinceEpoch}-'
    '${math.Random().nextInt(1 << 30)}',
  );
  List<_Fetched>? fetched;
  var sent = false;
  final runs = <Future<void>>[];
  try {
    await showModal(
      context,
      (context, close) => _Fetching(
        paths: paths,
        folder: folder,
        runs: runs,
        onDone: (files) {
          fetched = files;
          close();
        },
        onCancel: close,
      ),
    );
    final files = fetched;
    if (files == null) return false;
    sent =
        await _device.invokeMethod<bool>('shareFiles', {
          'paths': [for (final file in files) file.file.path],
          'mimes': [for (final file in files) file.mime],
          if (from != null)
            'rect': [from.left, from.top, from.width, from.height],
        }) ??
        false;
    return sent;
  } on MissingPluginException {
    return false;
  } on PlatformException {
    return false;
  } finally {
    // Sent: kept for the app it went to, which may still read it. Else
    // nothing stays, and again once a fetch Cancel cut short has stopped,
    // as it may write a moment longer.
    if (sent) {
      await _markSent(folder);
    } else {
      unawaited(_delete(folder));
      unawaited(
        Future.wait(runs).then((_) => _delete(folder), onError: (_) {}),
      );
    }
  }
}

/// Shares [paths] from [context]'s widget, the sheet pointing at it.
Future<bool> shareFrom(BuildContext context, List<String> paths) =>
    shareFiles(context, paths, from: globalRectOf(context));

Future<void> _markSent(Directory folder) async {
  try {
    await File('${folder.path}/$_sentMark').create();
  } on FileSystemException {
    // Unmarked, it goes at the next launch.
  }
}

Future<void> _delete(Directory folder) async {
  try {
    if (await folder.exists()) await folder.delete(recursive: true);
  } on FileSystemException {
    // Swept at the next launch.
  }
}

/// A file's row or card on a touch device (Files, a message's attachment,
/// an agent's show): touch and hold it for Share…, and Share all, when
/// [all] has more (a message's files, sent together). Elsewhere [child] as
/// it is: a Mac shares from the viewer's header and the conversation's
/// right click.
class ShareHold extends StatelessWidget {
  const ShareHold({
    super.key,
    required this.path,
    this.all = const [],
    required this.child,
  });

  final String path;

  /// The files it was sent with, itself among them.
  final List<String> all;
  final Widget child;

  Future<void> _menu(BuildContext context) async {
    unawaited(Haptics.impact());
    final chosen = await showActions<List<String>>(
      context,
      actions: [
        SheetAction(shareLabel(), [path]),
        if (all.length > 1) SheetAction(shareLabel(all.length), all),
      ],
    );
    if (chosen == null || !context.mounted) return;
    await shareFrom(context, chosen);
  }

  @override
  Widget build(BuildContext context) {
    if (!canShareFiles || !touchFirst(context) || path.isEmpty) return child;
    return GestureDetector(onLongPress: () => _menu(context), child: child);
  }
}

typedef _Fetched = ({File file, String mime});

/// Fetching the files to share, one after another: each one's name, how
/// much of how much has come, with a bar; Cancel; a read that failed says
/// why, and Try again carries on from where it stopped.
class _Fetching extends StatefulWidget {
  const _Fetching({
    required this.paths,
    required this.folder,
    required this.runs,
    required this.onDone,
    required this.onCancel,
  });

  final List<String> paths;
  final Directory folder;

  /// Each fetch started, to know when the last has stopped.
  final List<Future<void>> runs;
  final ValueChanged<List<_Fetched>> onDone;
  final VoidCallback onCancel;

  @override
  State<_Fetching> createState() => _FetchingState();
}

class _FetchingState extends State<_Fetching> {
  late Backend _backend;
  var _started = false;
  final _done = <_Fetched>[];

  /// The file coming now: which, how much of it is written, its size and
  /// version (what a read from the middle asks for).
  var _index = 0;
  var _received = 0;

  /// How much of it is in the file: where a read carries on from. What
  /// [_received] shows runs ahead of it while a piece comes.
  var _written = 0;
  var _size = 0;
  var _sha256 = '';
  var _mime = '';
  String? _error;
  var _cancelled = false;

  /// What the progress follows once a read was given up on: no waiting.
  static final _given = ValueNotifier(true);

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (!_started || !identical(backend, _backend)) {
      if (_started) _backend.connected.removeListener(_connectedChanged);
      backend.connected.addListener(_connectedChanged);
      _wasConnected = backend.connected.value;
    }
    _backend = backend;
    if (_started) return;
    _started = true;
    _start();
  }

  @override
  void dispose() {
    _cancelled = true;
    _backend.connected.removeListener(_connectedChanged);
    super.dispose();
  }

  var _wasConnected = true;

  /// Back after a read was given up on (a drop of over a minute): it
  /// carries on by itself from where it got, rather than waiting for Try
  /// again under words saying the connection hasn't come back (Sale's
  /// row 91).
  void _connectedChanged() {
    final now = _backend.connected.value;
    final back = now && !_wasConnected;
    _wasConnected = now;
    if (back && _error != null && !_cancelled && mounted) _start();
  }

  void _start() {
    final run = _fetch();
    widget.runs.add(run);
    unawaited(run);
  }

  String _name(String path) => shareName(path);

  File get _file =>
      File('${widget.folder.path}/$_index/${_name(widget.paths[_index])}');

  Future<void> _fetch() async {
    if (_error != null) setState(() => _error = null);
    try {
      while (_index < widget.paths.length) {
        await _fetchOne();
        if (_cancelled || !mounted) return;
        _done.add((file: _file, mime: _mime));
        setState(() {
          _index++;
          _received = _written = _size = 0;
          _sha256 = _mime = '';
        });
      }
      widget.onDone(_done);
    } on RequestFailure catch (failure) {
      if (mounted && !_cancelled) {
        setState(() {
          // Offline when it started, or when Try again was pressed: the
          // Claktory by its name, not the request's generic words
          // (Abbacchio's row 1065). It carries on once it's back.
          _error = failure.message == 'Not connected to the backend.'
              ? 'Not connected to ${claktoryName(_backend)}.'
              : failure.message;
          _received = _written;
        });
      }
    } on FileSystemException catch (error) {
      if (mounted && !_cancelled) {
        setState(() {
          _error = "Couldn't save it on this device: ${error.message}";
          _received = _written;
        });
      }
    }
  }

  /// The file at [_index], from [_written] on: a piece at a time, each
  /// in pieces sized to the link.
  Future<void> _fetchOne() async {
    final path = widget.paths[_index];
    final file = _file;
    await file.parent.create(recursive: true);
    final sink = file.openWrite(
      mode: _written == 0 ? FileMode.write : FileMode.append,
    );
    var changed = false;
    try {
      while (_size == 0 || _written < _size) {
        if (_cancelled || !mounted) return;
        final at = _written;
        final piece = await _backend.readFile(
          path,
          offset: at,
          maxBytes: _piece,
          expectedSha256: _sha256,
          onProgress: (through, size) {
            if (!mounted || _cancelled) return;
            setState(() {
              _received = math.max(_received, math.min(through, size));
              _size = size;
            });
          },
        );
        if (_cancelled || !mounted) return;
        if (piece.error == 'changed') {
          // Changed while it came: from the start, the new version.
          changed = true;
          break;
        }
        if (piece.error.isNotEmpty) throw RequestFailure(piece.error);
        if (piece.offset.toInt() != at ||
            (piece.data.isEmpty && piece.size > 0)) {
          throw const RequestFailure(
            "To share files, update the Claktory you're connected to.",
          );
        }
        sink.add(piece.data);
        _written = at + piece.data.length;
        setState(() {
          _received = _written;
          _size = piece.size.toInt();
          if (_sha256.isEmpty) _sha256 = piece.sha256;
          if (_mime.isEmpty) _mime = piece.mime;
        });
        if (piece.data.isEmpty) break;
      }
    } finally {
      await sink.close();
    }
    if (changed && mounted && !_cancelled) {
      setState(() => _received = _written = _size = 0);
      _sha256 = _mime = '';
      await _fetchOne();
    }
  }

  void _cancel() {
    _cancelled = true;
    widget.onCancel();
  }

  @override
  Widget build(BuildContext context) {
    final paths = widget.paths;
    final index = math.min(_index, paths.length - 1);
    final many = paths.length > 1;
    return FormFrame(
      title: many
          ? 'Share ${paths.length} files'
          : 'Share ${_name(paths.single)}',
      body: many ? 'File ${index + 1} of ${paths.length}' : null,
      fields: [
        // Failed before anything came (offline from the start): only the
        // error, not an empty bar under it (Sale's row 104).
        if (_error == null || _size > 0)
          LoadingProgress(
            name: many ? _name(paths[index]) : null,
            label: 'Loading…',
            received: _received,
            size: _size,
            // Given up on, it says so and nothing else (Abbacchio's row
            // 1024: "carries on from here" beside it).
            connected: _error == null ? _backend.connected : _given,
          ),
      ],
      error: _error,
      submitLabel: 'Try again',
      onSubmit: _error == null ? null : _start,
      onCancel: _cancel,
    );
  }
}
