import 'dart:async';
import 'dart:io';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart' show HitTestResult, RenderMetaData;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'identity.dart';
import 'card_files.dart' show CardDropAreaState;
import 'mobile/device.dart' show deviceCapabilities;
import 'mobile/device_permissions.dart' show CameraPermission;
import 'mobile/metrics.dart';
import 'mobile/permission.dart' show PermissionAccess, askPermission;
import 'resources.dart' show formatBytes;
import 'scope.dart' show AppScope;
import 'visible_codes.dart' show shownName;

/// A picture on this device at [path], as a conversation shows it sent:
/// decoded at its frame's width (240 points) and no bigger.
ImageProvider sentPicture(String path, double ratio) =>
    ResizeImage.resizeIfNeeded(
      (240 * ratio).round(),
      null,
      FileImage(File(path)),
    );

/// Starts decoding the picture at [path] as a conversation will show it
/// once sent, while it waits in the message box: sent, it's in its bubble
/// from the first frame, rather than joining it a moment later and moving
/// what's under it (Zucchero's row 675: the conversation lost the bottom).
void warmSentPicture(BuildContext context, String path) {
  // Once decoded, it's taken from the image cache at once.
  final picture = sentPicture(path, MediaQuery.devicePixelRatioOf(context));
  unawaited(precacheImage(picture, context, onError: (_, _) {}));
}

/// A file on this device, to send with a message.
class LocalFile {
  const LocalFile({required this.path, required this.name, required this.size});

  /// Where it is on this device.
  final String path;
  final String name;

  /// In bytes.
  final int size;

  /// Whether it shows as a picture.
  bool get isImage => isImageName(name);
}

/// Whether a file by [name] is a picture the app can show.
bool isImageName(String name) {
  final dot = name.lastIndexOf('.');
  if (dot < 0) return false;
  return const {
    'png',
    'jpg',
    'jpeg',
    'gif',
    'webp',
    'bmp',
    'heic',
  }.contains(name.substring(dot + 1).toLowerCase());
}

/// The MIME type a file by [name] most likely has; empty when unsure, and
/// the backend makes it out.
String mimeOf(String name) {
  final dot = name.lastIndexOf('.');
  if (dot < 0) return '';
  return const {
        'png': 'image/png',
        'jpg': 'image/jpeg',
        'jpeg': 'image/jpeg',
        'gif': 'image/gif',
        'webp': 'image/webp',
        'heic': 'image/heic',
        'pdf': 'application/pdf',
        'txt': 'text/plain',
        'md': 'text/markdown',
        'csv': 'text/csv',
        'json': 'application/json',
        'html': 'text/html',
        'zip': 'application/zip',
        'm4a': 'audio/mp4',
      }[name.substring(dot + 1).toLowerCase()] ??
      '';
}

/// The files waiting in each agent's message box, by agent id, until the
/// message goes with them. Only on this device, and only in memory: they're
/// this device's files, and the box forgets them when the app quits.
class AttachmentDrafts extends ChangeNotifier {
  final _byAgent = <String, List<LocalFile>>{};
  final _adding = <String, List<String>>{};

  List<LocalFile> of(String agentId) => _byAgent[agentId] ?? const [];

  /// What's on its way into [key]'s box from the picker, one kind each
  /// ('photo', 'video', 'file'): chosen, still being copied for the app
  /// (Matt: a photo shows at once, not seconds after the picker closes).
  List<String> adding(String key) => _adding[key] ?? const [];

  /// [kinds] were chosen for [key]'s box and are on their way: they show
  /// as "Adding photo…" until [doneAdding].
  void startAdding(String key, List<String> kinds) {
    if (kinds.isEmpty) return;
    _adding.putIfAbsent(key, () => []).addAll(kinds);
    notifyListeners();
  }

  /// [kinds], from [startAdding], arrived (added with [add] just before)
  /// or couldn't be copied.
  void doneAdding(String key, List<String> kinds) {
    final list = _adding[key];
    if (list == null || kinds.isEmpty) return;
    for (final kind in kinds) {
      list.remove(kind);
    }
    if (list.isEmpty) _adding.remove(key);
    notifyListeners();
  }

  /// Once nothing is on its way into [key]'s box any more: true when it
  /// waited for something (what came is in [of] by then).
  Future<bool> settled(String key) async {
    if (adding(key).isEmpty) return false;
    final done = Completer<void>();
    void check() {
      if (adding(key).isEmpty && !done.isCompleted) done.complete();
    }

    addListener(check);
    try {
      await done.future;
    } finally {
      removeListener(check);
    }
    return true;
  }

  /// Adds the files at [paths] to [agentId]'s box, each once; folders and
  /// what can't be read are left out. Answers how many were left out.
  Future<int> add(String agentId, Iterable<String> paths) async {
    final list = _byAgent.putIfAbsent(agentId, () => []);
    var left = 0;
    for (final path in paths) {
      if (list.any((attachment) => attachment.path == path)) continue;
      final FileStat stat;
      try {
        stat = await FileStat.stat(path);
      } on FileSystemException {
        left++;
        continue;
      }
      if (stat.type != FileSystemEntityType.file) {
        left++;
        continue;
      }
      list.add(
        LocalFile(
          path: path,
          name: path.split(Platform.pathSeparator).last,
          size: stat.size,
        ),
      );
    }
    notifyListeners();
    return left;
  }

  void remove(String agentId, LocalFile attachment) {
    if (_byAgent[agentId]?.remove(attachment) ?? false) notifyListeners();
  }

  /// Empties [agentId]'s box, answering what was in it.
  List<LocalFile> take(String agentId) {
    final taken = _byAgent.remove(agentId) ?? const [];
    if (taken.isNotEmpty) notifyListeners();
    return taken;
  }
}

/// A card that takes files dropped on it, wherever its field's keys are
/// ([FileDrop.addCard]): what the drop overlay says they'd go to, and
/// where they go.
abstract interface class CardDropTarget {
  /// "your answer to June".
  String get dropLabel;

  void onFiles(List<String> paths);
}

/// Files dragged onto the window from the Finder: the window takes them
/// only while a message box can (see [accept]), and hands them over here.
class FileDrop {
  FileDrop._();

  static const _channel = MethodChannel('factory/window');

  /// Whether this platform's window takes files dropped on it: the Mac's
  /// (MainFlutterWindow.swift), Windows' (app/windows/runner/
  /// drop_target.cpp) and an iPad's (app/ios/Runner/Drop.swift:
  /// from Files, Photos and other apps). Elsewhere (an iPhone, Android at
  /// any width) nothing here touches the window's channel, and no overlay
  /// shows.
  static bool get windowDrops =>
      defaultTargetPlatform == TargetPlatform.macOS ||
      defaultTargetPlatform == TargetPlatform.windows ||
      iPadDrops;

  /// An iPad's window: its drops come staged ([staged]), and its overlay
  /// shows at every width, the phone's layout's too.
  static bool get iPadDrops =>
      defaultTargetPlatform == TargetPlatform.iOS &&
      deviceCapabilities.idiom == Idiom.pad;

  /// Files are being dragged over the window.
  static final over = ValueNotifier<bool>(false);

  /// How many files are being dragged over the window; 0 when none are.
  static final count = ValueNotifier<int>(0);

  /// Where files dragged over the window are, in the window's points from
  /// its top left; null when none are, or the window doesn't say.
  static final at = ValueNotifier<Offset?>(null);

  /// The native window says files came over it ([files] of them), at
  /// [point] when it says, or left.
  @visibleForTesting
  static void dragged(int files, {Offset? point}) {
    at.value = files > 0 ? point : null;
    cardOver.value = files > 0 && point != null
        ? cardAt(point)?.dropLabel
        : null;
    count.value = files;
    over.value = files > 0;
  }

  /// What the card under files dragged over the window would add them to
  /// ([CardDropTarget.dropLabel]); null when no card is under them.
  static final cardOver = ValueNotifier<String?>(null);

  /// The point a native call's arguments give ({x, y}), if they do.
  static Offset? _point(Object? arguments) {
    if (arguments is! Map) return null;
    final x = arguments['x'], y = arguments['y'];
    return x is num && y is num ? Offset(x.toDouble(), y.toDouble()) : null;
  }

  static final _dropped = StreamController<List<String>>.broadcast();

  /// The paths of files dropped on the window.
  static Stream<List<String>> get dropped {
    _listen();
    return _dropped.stream;
  }

  static var _listening = false;

  static void _listen() {
    if (_listening || !windowDrops) return;
    _listening = true;
    _channel.setMethodCallHandler((call) async {
      switch (call.method) {
        case 'filesOver':
          // How many and where ({count, x, y}), or (from an older window)
          // how many, or whether any.
          final files = call.arguments;
          dragged(switch (files) {
            {'count': final int count} => count,
            final int count => count,
            true => 1,
            _ => 0,
          }, point: _point(files));
        case 'textDropped':
          // An iPad's: {text, x, y}.
          final arguments = call.arguments;
          if (arguments case {'text': final String text} when text.isNotEmpty) {
            _texts.add(text);
          }
        case 'filesDropping':
          // An iPad's drop, still being copied for the app: {kinds,
          // ticket, x, y}; its paths follow in filesDropped's ticket.
          final arguments = call.arguments;
          if (arguments is! Map) return;
          dragged(0);
          stage(
            [
              for (final kind in arguments['kinds'] as List<Object?>? ?? [])
                if (kind is String) kind,
            ],
            arguments['ticket'] as int? ?? -1,
            at: _point(arguments),
          );
        case 'filesDropped':
          // Dropped: {paths, x, y}, with an iPad's ticket when it was
          // staged; pasted: the paths alone.
          final arguments = call.arguments;
          final paths = arguments is Map ? arguments['paths'] : arguments;
          dragged(0);
          final list = [
            for (final path in paths as List<Object?>)
              if (path is String) path,
          ];
          if (arguments case {'ticket': final int ticket}) {
            finish(ticket, list, at: _point(arguments));
          } else {
            deliver(list, at: _point(arguments));
          }
      }
    });
  }

  /// The message box the window's drops go to, if one shows ([claim]).
  static Object? _owner;

  /// Whether the message box showing takes files; null when none shows.
  /// The drop overlay says what dropping would do from it.
  static final takesFiles = ValueNotifier<bool?>(null);

  /// [owner], a message box, is the one showing: files dropped on the
  /// window go to it if it [takesFiles]. A new box claims before the one
  /// it replaces lets go, so the last to claim keeps it.
  static void claim(Object owner, {required bool takesFiles, String? key}) {
    _owner = owner;
    claimedKey = key;
    FileDrop.takesFiles.value = takesFiles;
    _sync();
  }

  /// The key of the target ([MessageTarget.key]: an agent's id) of the box
  /// that claimed the window last, so the overlay names whom files go to:
  /// the box in focus mode's Context, not the agent selected under it
  /// (Abbacchio's row 1607).
  static String? claimedKey;

  /// Whether [owner] is the box drops go to now: the last to [claim].
  static bool owns(Object owner) => identical(_owner, owner);

  /// [owner] is gone; if it was the one showing, no box is.
  static void release(Object owner) {
    if (!identical(_owner, owner)) return;
    _owner = null;
    claimedKey = null;
    takesFiles.value = null;
    _sync();
  }

  /// Tells the window whether it takes files now: while a message box
  /// does, a card's field has the keys, or a card that takes them shows.
  static void _sync() => unawaited(
    _accept(
      (takesFiles.value ?? false) || _diverted != null || _cards.isNotEmpty,
    ),
  );

  /// Files dropped or pasted on the window: dropped [at] a card that takes
  /// files, to it, whether its field has the keys or not (#511: dragging
  /// from the Finder, the window isn't the key one, so nothing in it has
  /// the keys); else to a card's field that has the keys ([divert]); else
  /// to the message boxes.
  @visibleForTesting
  static void deliver(List<String> paths, {Offset? at}) {
    if ((at == null ? null : cardAt(at)) case final card?) {
      card.onFiles(paths);
    } else if (_diverted case final diverted?) {
      diverted.onFiles(paths);
    } else {
      _dropped.add(paths);
    }
  }

  static final _staged = StreamController<StagedDrop>.broadcast();

  /// An iPad's drops for the message box, as soon as they're dropped:
  /// what's on its way, and its paths once copied for the app.
  static Stream<StagedDrop> get staged {
    _listen();
    return _staged.stream;
  }

  /// The staged drops whose paths haven't come yet, by ticket.
  static final _waiting = <int, ({Completer<List<String>> done, Timer give})>{};

  /// How long a staged drop's "Adding…" waits for its paths before it
  /// says they weren't added: copying a long video from another app can
  /// take a while, but never this long.
  static const stagedWait = Duration(minutes: 2);

  /// An iPad's drop of [kinds] (one each: 'photo', 'video', 'file') [at]
  /// a point, still being copied: it goes where [deliver] would send it,
  /// decided now, as the person let go; the box shows "Adding photo…"
  /// until [finish] brings the paths, or [stagedWait] passes.
  @visibleForTesting
  static void stage(List<String> kinds, int ticket, {Offset? at}) {
    final done = Completer<List<String>>();
    _waiting.remove(ticket)?.give.cancel();
    _waiting[ticket] = (
      done: done,
      give: Timer(stagedWait, () {
        if (_waiting[ticket]?.done != done) return;
        _waiting.remove(ticket);
        done.complete(const []);
      }),
    );
    void then(void Function(List<String>) onFiles) => unawaited(
      done.future.then((paths) {
        if (paths.isNotEmpty) onFiles(paths);
      }),
    );
    if ((at == null ? null : cardAt(at)) case final card?) {
      then(card.onFiles);
    } else if (_diverted case final diverted?) {
      then(diverted.onFiles);
    } else {
      _staged.add(StagedDrop(kinds, done.future));
    }
  }

  /// The paths of the drop staged as [ticket]; one that waited too long,
  /// or was never staged, goes where a drop [at] its point goes now.
  @visibleForTesting
  static void finish(int ticket, List<String> paths, {Offset? at}) {
    final waiting = _waiting.remove(ticket);
    if (waiting == null) return deliver(paths, at: at);
    waiting.give.cancel();
    waiting.done.complete(paths);
  }

  /// The cards showing that take files dropped on them ([addCard]).
  static final _cards = <CardDropTarget>{};

  /// [card] shows and takes files dropped on it.
  static void addCard(CardDropTarget card) {
    if (_cards.add(card)) _sync();
  }

  /// [card] no longer takes them.
  static void removeCard(CardDropTarget card) {
    if (_cards.remove(card)) _sync();
  }

  /// The card that takes files dropped at [point], in the window's points:
  /// the one under it there, its field or anywhere on its request
  /// ([CardDropArea]), nothing covering it (a sheet, a dialog).
  static CardDropTarget? cardAt(Offset point) {
    if (_cards.isEmpty) return null;
    final binding = WidgetsBinding.instance;
    final view = binding.platformDispatcher.implicitView;
    if (view == null) return null;
    final result = HitTestResult();
    binding.hitTestInView(result, point, view.viewId);
    for (final entry in result.path) {
      if (entry.target case RenderMetaData(:final metaData)) {
        final card = switch (metaData) {
          final CardDropTarget card => card,
          final CardDropAreaState area => area.target,
          _ => null,
        };
        if (card != null && _cards.contains(card)) return card;
      }
    }
    return null;
  }

  /// A card's field that has the keys (#511): what's dropped or pasted
  /// goes to it rather than to the message box, and the overlay says so.
  static ({Object owner, void Function(List<String>) onFiles})? _diverted;

  /// What dropping would add to while a card's field has the keys ("your
  /// answer to June"); null when it goes to the message box.
  static final divertedTo = ValueNotifier<String?>(null);

  /// [owner], a card's field, has the keys: files dropped or pasted on the
  /// window go to [onFiles] until it lets go ([undivert]); [label] says to
  /// what, on the overlay.
  static void divert(
    Object owner,
    void Function(List<String> paths) onFiles, {
    required String label,
  }) {
    _diverted = (owner: owner, onFiles: onFiles);
    divertedTo.value = label;
    _sync();
  }

  /// [owner] let go of the keys: files go to the message box again, if
  /// one takes them.
  static void undivert(Object owner) {
    if (!identical(_diverted?.owner, owner)) return;
    _diverted = null;
    divertedTo.value = null;
    _sync();
  }

  /// Whether the window takes files dropped on it now; on an iPad, also
  /// whether it takes text ([texts]): while a message box shows.
  static Future<void> _accept(bool on) async {
    if (!windowDrops) return;
    _listen();
    try {
      await _channel.invokeMethod<void>('takeFiles', on);
      if (iPadDrops) {
        await _channel.invokeMethod<void>('takeText', _owner != null);
      }
    } on Exception {
      // No native window (a test without one).
    }
  }

  static final _texts = StreamController<String>.broadcast();

  /// Text dragged onto an iPad's window from another app (a selection, a
  /// link), for the message box showing: Flutter's own fields take no
  /// drops there.
  static Stream<String> get texts {
    _listen();
    return _texts.stream;
  }
}

/// An iPad's drop on the message box, as the person let go ([FileDrop.staged]):
/// one kind for each file on its way ('photo', 'video', 'file'), and their
/// paths once copied for the app (fewer when some couldn't be; none when
/// none could, or they took too long).
class StagedDrop {
  const StagedDrop(this.kinds, this.paths);

  final List<String> kinds;
  final Future<List<String>> paths;
}

/// On a Mac: asks the person for files to send in the system's open
/// panel. Their paths; empty when cancelled.
Future<List<String>> pickFiles() =>
    _paths(const MethodChannel('factory/window'), 'pickFiles');

/// What a picker said is on its way once it closed, before its files are
/// copied for the app: one kind each ('photo', 'video', 'file').
typedef OnChosen = void Function(List<String> kinds);

/// What's said when [missing] of the [kinds] chosen couldn't be copied for
/// the app; null when none is missing.
String? notAdded(List<String> kinds, int missing) {
  if (missing <= 0) return null;
  final kind = kinds.toSet().length == 1 ? kinds.first : 'item';
  return missing == 1 && kinds.length == 1
      ? "Couldn't add this $kind. Try again."
      : missing == 1
      ? "One $kind wasn't added. Try again."
      : "$missing ${kind}s weren't added. Try again.";
}

/// On iPhone: the system's menu of where to attach from (Choose from
/// Photos, Choose from Files, and Paste image when the clipboard holds one), then its
/// picker. The chosen files' paths, copied for the app; empty when
/// cancelled. On an iPad the menu points at [from], the control tapped
/// ([pickerAnchor]); without it, it shows in the middle.
Future<List<String>> pickAttachments({Rect? from}) => _paths(
  const MethodChannel('factory/device'),
  'pickAttachments',
  pickerArguments(from),
);

/// Where [context]'s widget is in the window, for an iPad's picker menu to
/// point at; null when it isn't laid out.
Rect? pickerAnchor(BuildContext? context) {
  final box = context?.findRenderObject();
  if (box is! RenderBox || !box.attached || !box.hasSize) return null;
  return box.localToGlobal(Offset.zero) & box.size;
}

/// pickAttachments' arguments: the rect its iPad menu points at, if any
/// (Zucchero row 630), and whether it offers Take Photo ([camera]).
Map<String, Object>? pickerArguments(Rect? from, {bool camera = false}) =>
    from == null && !camera
    ? null
    : {
        if (from != null)
          'rect': [from.left, from.top, from.width, from.height],
        if (camera) 'camera': true,
      };

/// A phone's or tablet's picker (`factory/device`'s pickAttachments) for
/// the message box or a card, whose menu offers Take Photo first where the
/// device has a camera (Matt, Bruno 46436). Chosen, the menu closes and
/// answers `take_photo`; the camera is explained first on the permission
/// sheet, then takePhoto opens it full screen. The chosen or taken files'
/// paths; empty when cancelled. Its channel's failures are thrown as they
/// come (MissingPluginException where the app has no picker), for the
/// caller to say.
///
/// With [onChosen], a photo pick that takes a moment to copy what was
/// chosen says what's on its way as soon as the picker closes ("Adding
/// photo…"), and the paths follow once they're copied (Matt: a photo took
/// seconds to show).
Future<List<String>> pickOnDevice(
  BuildContext context, {
  Rect? from,
  bool camera = false,
  OnChosen? onChosen,
}) async {
  const channel = MethodChannel('factory/device');
  final arguments = <String, Object>{
    ...?pickerArguments(from, camera: camera),
    if (onChosen != null) 'staged': true,
  };
  final Object? answer;
  try {
    answer = await channel.invokeMethod<Object?>(
      'pickAttachments',
      arguments.isEmpty ? null : arguments,
    );
  } on PlatformException catch (error) {
    if (error.code != 'take_photo') rethrow;
    if (!context.mounted) return const [];
    return _takePhoto(context, from);
  }
  if (answer is Map) {
    final pending = answer['pending'];
    onChosen?.call([
      for (final kind in pending is List ? pending : const <Object?>[])
        if (kind is String) kind,
    ]);
    return _strings(
      await channel.invokeMethod<Object?>('pickedAttachments', {
        'ticket': answer['ticket'],
      }),
    );
  }
  return _strings(answer);
}

/// Take Photo: the camera's permission asked on the sheet that says why
/// (Before; Declined with Open Settings), then the camera.
Future<List<String>> _takePhoto(BuildContext context, Rect? from) async {
  const channel = MethodChannel('factory/device');
  final permission = CameraPermission(
    AppScope.scope(context).camera,
    photo: true,
  );
  final access = await askPermission(context, permission);
  if (access != PermissionAccess.granted || !context.mounted) {
    return const [];
  }
  try {
    return _strings(
      await channel.invokeListMethod<Object?>(
        'takePhoto',
        pickerArguments(from),
      ),
    );
  } on PlatformException catch (error) {
    if (error.code != 'camera_denied') rethrow;
    // Turned off between the check and the camera: what's off, and how
    // to turn it on.
    if (context.mounted) await askPermission(context, permission);
    return const [];
  }
}

List<String> _strings(Object? paths) => [
  for (final path in paths is List ? paths : const <Object?>[])
    if (path is String) path,
];

Future<List<String>> _paths(
  MethodChannel channel,
  String method, [
  Object? arguments,
]) async {
  try {
    final paths = await channel.invokeMethod<List<Object?>>(method, arguments);
    return [
      for (final path in paths ?? const <Object?>[])
        if (path is String) path,
    ];
  } on Exception {
    return const [];
  }
}

/// A file in the message box, or with a message: its glyph, its name and
/// its size, and × to take it out of the box. While it uploads, the size
/// gives way to how far it got, and a bar fills along the bottom.
class AttachmentChip extends StatelessWidget {
  const AttachmentChip({
    super.key,
    required this.name,
    required this.size,
    this.progress,
    this.onRemove,
    this.onOpen,
  });

  final String name;
  final int size;

  /// 0 to 1 while it uploads.
  final double? progress;

  /// Takes it out of the message box; none once it's sent.
  final VoidCallback? onRemove;

  /// Opens it; none when it can't be opened here.
  final VoidCallback? onOpen;

  /// The widest a chip gets; its name is cut short to fit.
  static const maxWidth = 240.0;

  /// The text factor from which a chip's size goes under its name.
  static const stackFromTextFactor = 1.35;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final progress = this.progress;
    final phone = isPhoneLayout(context);
    // A finger's target for the chip and its ×, wherever a finger is
    // what touches it.
    final finger = touchFirst(context);
    final type = TypeScale.of(context);
    // From large text sizes the size goes under the name, so neither is
    // cut to nothing and the × stays put.
    final stacked =
        MediaQuery.textScalerOf(context).scale(17) / 17 >= stackFromTextFactor;
    final nameText = Text(
      shownName(name),
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: (phone ? type.secondary : TextStyles.label).copyWith(
        color: palette.text,
      ),
    );
    final sizeText = Text(
      progress == null
          ? formatBytes(Int64(size))
          : '${(progress * 100).floor()}%',
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: type.caption.copyWith(
        color: palette.textFaint,
        fontFeatures: const [FontFeature.tabularFigures()],
      ),
    );
    final chip = Container(
      constraints: BoxConstraints(
        minHeight: finger ? Touch.target : 28,
        maxWidth: maxWidth,
      ),
      decoration: BoxDecoration(
        color: palette.canvas,
        borderRadius: BorderRadius.circular(Radii.control),
        border: Border.all(color: palette.hairline),
      ),
      clipBehavior: Clip.antiAlias,
      child: Stack(
        children: [
          Padding(
            padding: EdgeInsets.only(
              left: Space.s,
              right: onRemove == null ? Space.s : 2,
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                SizedBox.square(
                  dimension: 14,
                  child: isImageName(name)
                      ? ExtensionGlyph(
                          icon: 'image',
                          name: name,
                          color: palette.textMuted,
                          size: 14,
                        )
                      : FileGlyph(color: palette.textMuted),
                ),
                const SizedBox(width: Space.xs + 2),
                if (stacked)
                  Flexible(
                    child: Padding(
                      padding: const EdgeInsets.symmetric(vertical: Space.xs),
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [nameText, sizeText],
                      ),
                    ),
                  )
                else ...[
                  Flexible(child: nameText),
                  const SizedBox(width: Space.xs + 2),
                  sizeText,
                ],
                if (onRemove != null)
                  Pressable(
                    onTap: onRemove,
                    builder: (context, hovered) => Semantics(
                      // Under a finger its own element, or the chip's
                      // others merge into it.
                      container: finger,
                      button: true,
                      label: 'Remove $name',
                      onTap: finger ? onRemove : null,
                      child: SizedBox.square(
                        dimension: finger ? Touch.target : 24,
                        child: Center(
                          child: ExtensionGlyph(
                            icon: 'x',
                            name: 'Remove',
                            color: hovered ? palette.text : palette.textFaint,
                            size: finger ? 16 : 12,
                          ),
                        ),
                      ),
                    ),
                  ),
              ],
            ),
          ),
          if (progress != null)
            Positioned(
              left: 0,
              right: 0,
              bottom: 0,
              child: Align(
                alignment: Alignment.centerLeft,
                child: FractionallySizedBox(
                  widthFactor: progress.clamp(0, 1),
                  child: Container(height: 2, color: palette.signal),
                ),
              ),
            ),
        ],
      ),
    );
    final onOpen = this.onOpen;
    if (onOpen == null) return chip;
    return Pressable(onTap: onOpen, builder: (context, hovered) => chip);
  }
}

/// A file chosen in the picker, on its way into the box while it's copied
/// for the app: "Adding photo…" after a faint live dot, in an
/// [AttachmentChip]'s shape, the real chip taking its place once it's
/// here.
class AddingChip extends StatelessWidget {
  const AddingChip({super.key, required this.kind});

  /// 'photo', 'video' or 'file'.
  final String kind;

  /// What it says: "Adding photo…".
  static String label(String kind) =>
      'Adding ${switch (kind) {
        'photo' || 'video' => kind,
        _ => 'file',
      }}…';

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    final type = TypeScale.of(context);
    return Semantics(
      container: true,
      liveRegion: true,
      label: label(kind),
      child: ExcludeSemantics(
        child: Container(
          constraints: BoxConstraints(
            minHeight: touchFirst(context) ? Touch.target : 28,
            maxWidth: AttachmentChip.maxWidth,
          ),
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          decoration: BoxDecoration(
            color: palette.canvas,
            borderRadius: BorderRadius.circular(Radii.control),
            border: Border.all(color: palette.hairline),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              SizedBox.square(
                dimension: 14,
                child: Center(child: LiveDot(color: palette.textFaint)),
              ),
              const SizedBox(width: Space.xs + 2),
              Flexible(
                child: Text(
                  label(kind),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: (phone ? type.secondary : TextStyles.label).copyWith(
                    color: palette.textMuted,
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
