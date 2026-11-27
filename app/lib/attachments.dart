import 'dart:async';
import 'dart:io';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart' show HitTestResult, RenderMetaData;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'identity.dart';
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
  /// (MainFlutterWindow.swift) and Windows' (app/windows/runner/
  /// drop_target.cpp). Elsewhere (an iPhone, an iPad, Android at any width)
  /// nothing here touches the window's channel, and no overlay shows.
  static bool get windowDrops =>
      defaultTargetPlatform == TargetPlatform.macOS ||
      defaultTargetPlatform == TargetPlatform.windows;

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
        case 'filesDropped':
          // Dropped: {paths, x, y}; pasted: the paths alone.
          final arguments = call.arguments;
          final paths = arguments is Map ? arguments['paths'] : arguments;
          dragged(0);
          deliver([
            for (final path in paths as List<Object?>)
              if (path is String) path,
          ], at: _point(arguments));
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
  /// the one under it there, nothing covering it (a sheet, a dialog).
  static CardDropTarget? cardAt(Offset point) {
    if (_cards.isEmpty) return null;
    final binding = WidgetsBinding.instance;
    final view = binding.platformDispatcher.implicitView;
    if (view == null) return null;
    final result = HitTestResult();
    binding.hitTestInView(result, point, view.viewId);
    for (final entry in result.path) {
      if (entry.target case RenderMetaData(metaData: final CardDropTarget card)
          when _cards.contains(card)) {
        return card;
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

  /// Whether the window takes files dropped on it now.
  static Future<void> _accept(bool on) async {
    if (!windowDrops) return;
    _listen();
    try {
      await _channel.invokeMethod<void>('takeFiles', on);
    } on Exception {
      // No native window (a test without one).
    }
  }
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

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final progress = this.progress;
    // On a phone, a finger's size, its × too (research mobile.md §4).
    final phone = isPhoneLayout(context);
    final type = TypeScale.of(context);
    final chip = Container(
      height: phone ? Touch.target : 28,
      constraints: const BoxConstraints(maxWidth: maxWidth),
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
                Flexible(
                  child: Text(
                    shownName(name),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: (phone ? type.secondary : TextStyles.label).copyWith(
                      color: palette.text,
                    ),
                  ),
                ),
                const SizedBox(width: Space.xs + 2),
                Text(
                  progress == null
                      ? formatBytes(Int64(size))
                      : '${(progress * 100).floor()}%',
                  style: type.caption.copyWith(
                    color: palette.textFaint,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
                if (onRemove != null)
                  Pressable(
                    onTap: onRemove,
                    builder: (context, hovered) => Semantics(
                      // On a phone its own element, or the chip's others
                      // merge into it.
                      container: phone,
                      button: true,
                      label: 'Remove $name',
                      onTap: phone ? onRemove : null,
                      child: SizedBox.square(
                        dimension: phone ? Touch.target : 24,
                        child: Center(
                          child: ExtensionGlyph(
                            icon: 'x',
                            name: 'Remove',
                            color: hovered ? palette.text : palette.textFaint,
                            size: phone ? 16 : 12,
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
          height: phone ? Touch.target : 28,
          constraints: const BoxConstraints(maxWidth: AttachmentChip.maxWidth),
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
