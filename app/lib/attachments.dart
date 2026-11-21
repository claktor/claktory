import 'dart:async';
import 'dart:io';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'identity.dart';
import 'mobile/metrics.dart';
import 'resources.dart' show formatBytes;

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

  List<LocalFile> of(String agentId) => _byAgent[agentId] ?? const [];

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

/// Files dragged onto the window from the Finder: the window takes them
/// only while a message box can (see [accept]), and hands them over here.
class FileDrop {
  FileDrop._();

  static const _channel = MethodChannel('factory/window');

  /// Whether this platform's window takes files dropped on it: only the
  /// Mac's does (MainFlutterWindow.swift). Elsewhere (an iPhone, an iPad,
  /// Android at any width) nothing here touches the window's channel, and
  /// no overlay shows.
  static bool get windowDrops => defaultTargetPlatform == TargetPlatform.macOS;

  /// Files are being dragged over the window.
  static final over = ValueNotifier<bool>(false);

  /// How many files are being dragged over the window; 0 when none are.
  static final count = ValueNotifier<int>(0);

  /// The native window says files came over it ([files] of them) or left.
  @visibleForTesting
  static void dragged(int files) {
    count.value = files;
    over.value = files > 0;
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
          // How many, or (from an older window) whether any.
          final files = call.arguments;
          dragged(files is int ? files : (files == true ? 1 : 0));
        case 'filesDropped':
          dragged(0);
          _dropped.add([
            for (final path in call.arguments as List<Object?>)
              if (path is String) path,
          ]);
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
  static void claim(Object owner, {required bool takesFiles}) {
    _owner = owner;
    FileDrop.takesFiles.value = takesFiles;
    unawaited(_accept(takesFiles));
  }

  /// [owner] is gone; if it was the one showing, no box is.
  static void release(Object owner) {
    if (!identical(_owner, owner)) return;
    _owner = null;
    takesFiles.value = null;
    unawaited(_accept(false));
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

/// On iPhone: the system's menu of where to attach from (Choose from
/// Photos, Choose from Files, and Paste image when the clipboard holds one), then its
/// picker. The chosen files' paths, copied for the app; empty when
/// cancelled.
Future<List<String>> pickAttachments() =>
    _paths(const MethodChannel('factory/device'), 'pickAttachments');

Future<List<String>> _paths(MethodChannel channel, String method) async {
  try {
    final paths = await channel.invokeMethod<List<Object?>>(method);
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
                    name,
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
