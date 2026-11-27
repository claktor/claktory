// Files with what the person writes on a card: a written
// answer, Approve's or Dismiss's comment, a reply or Resolve on its talk,
// and a step's Done or Can't do this. They're added as the message box's
// are (the paperclip, a phone's picker, and on a Mac dropped or pasted on
// the window while the card's field has the keys), wait under the field as
// chips, go up to the card's agent as its answer is sent (its progress on
// each chip), and are named in the answer (AnswerAsk.attachments). The
// files waiting are the app's attachment drafts', by the card, in memory
// only, as the message box's are; what was sent shows on the card from the
// backend (the talk's turns, the answer, a step's comment).
import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'attachments.dart';
import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'mobile/platform_policy.dart' show PhonePlatformPolicy;
import 'protocol/factory/v1/factory.pb.dart' show Attachment;
import 'scope.dart';
import 'transcript.dart' show SentFiles;

/// The key a card's files wait under in the attachment drafts.
String cardFilesKey(String agentId, String askId) => 'ask/$agentId/$askId';

/// A file of a card's on its way to the card's agent.
class _Upload {
  _Upload(this.id);

  /// Made up here, so an upload cut off is picked up where it stopped.
  final String id;

  /// How many bytes the backend has.
  var sent = 0;

  /// Where the backend put it, once it's all there.
  Attachment? done;
}

/// The cards' files going up to their agents, by card and file: kept until
/// the answer they went with is sent, so an answer sent again (refused,
/// or cut off) doesn't send them twice. One per backend, in memory.
class CardUploads extends ChangeNotifier {
  CardUploads._();

  static final _each = Expando<CardUploads>('card uploads');

  static CardUploads of(Backend backend) => _each[backend] ??= CardUploads._();

  final _uploads = <String, _Upload>{};
  final _random = Random.secure();

  /// Files sent from here this session, by where the backend put them, as
  /// the outbox has them ([Outbox.localCopyOf]).
  final _localCopies = <String, String>{};

  /// Where the file the backend keeps at [path] is on this device, when a
  /// card's answer took it from here this session.
  String? localCopyOf(String path) => _localCopies[path];

  /// The most sent in one chunk, as the outbox does.
  static const chunk = 1024 * 1024;

  String _key(String card, LocalFile file) => '$card\n${file.path}';

  /// How far [file] of [card]'s got, 0 to 1; null before it starts.
  double? progress(String card, LocalFile file) {
    final upload = _uploads[_key(card, file)];
    if (upload == null) return null;
    return file.size == 0 ? 1 : upload.sent / file.size;
  }

  /// Sends [files], [card]'s, to [agentId] (each from where it stopped):
  /// what to name in the answer, in order.
  Future<List<Attachment>> send(
    Backend backend,
    String agentId,
    String card,
    List<LocalFile> files,
  ) async {
    // Photos chosen a moment ago, still being copied for the app ("Adding
    // photo…"): the answer waits for them, and they go with it.
    if (await backend.attachments.settled(card)) {
      files = backend.attachments.of(card);
    }
    final sent = <Attachment>[];
    for (final file in files) {
      final upload = _uploads.putIfAbsent(
        _key(card, file),
        () => _Upload(
          List.generate(
            16,
            (_) => _random.nextInt(16).toRadixString(16),
          ).join(),
        ),
      );
      final done = upload.done;
      if (done != null) {
        sent.add(done);
        continue;
      }
      upload.sent = await backend.startUpload(
        upload.id,
        agentId,
        name: file.name,
        size: file.size,
        mime: mimeOf(file.name),
      );
      notifyListeners();
      final source = await File(file.path).open();
      try {
        while (upload.sent < file.size) {
          await source.setPosition(upload.sent);
          final data = await source.read(min(chunk, file.size - upload.sent));
          if (data.isEmpty) {
            throw FileSystemException('It got shorter', file.path);
          }
          upload.sent = await backend.uploadChunk(upload.id, upload.sent, data);
          notifyListeners();
        }
      } finally {
        await source.close();
      }
      upload.done = await backend.finishUpload(upload.id);
      _localCopies[upload.done!.path] = file.path;
      notifyListeners();
      sent.add(upload.done!);
    }
    return sent;
  }

  /// [file] was taken off [card] before its answer went: what the backend
  /// has of it is dropped.
  void drop(Backend backend, String card, LocalFile file) {
    final upload = _uploads.remove(_key(card, file));
    if (upload == null) return;
    notifyListeners();
    if (upload.done == null && upload.sent > 0) {
      unawaited(backend.cancelUpload(upload.id).catchError((Object _) {}));
    }
  }

  /// [card]'s answer was sent, its files with it.
  void sent(String card) {
    _uploads.removeWhere((key, _) => key.startsWith('$card\n'));
    notifyListeners();
  }
}

/// Why a card's answer didn't go: a file to go with it couldn't be read.
String cardFilesFailure(FileSystemException error) =>
    "Couldn't read ${error.path?.split(Platform.pathSeparator).last ?? 'a file'}. "
    'It may have been moved or deleted.';

/// What a picker that failed says, as the message box does.
const cardPickerFailed = "Couldn't open your photos or files. Try again.";
const cardPickerMissing = "You can't attach files from this phone yet.";

/// Adds the files at [paths] to [card]: answers what to say about any
/// that couldn't be added (folders, what can't be read), or null.
Future<String?> addCardFiles(
  Backend backend,
  String card,
  List<String> paths,
) async {
  if (paths.isEmpty) return null;
  final left = await backend.attachments.add(card, paths);
  if (left == 0) return null;
  return left == paths.length && left == 1
      ? "Can't attach this: it's a folder or can't be read."
      : "Some items weren't attached: folders, and files that can't be read.";
}

/// The system's picker for files (and photos, on a phone or an iPad), as
/// the message box's paperclip has it: their paths, empty when cancelled.
/// Throws [MissingPluginException] where the app has no picker.
/// On an iPad its menu points at [from], the paperclip;
/// Take Photo comes first where there's a camera ([context] explains it).
/// With [onChosen], what was chosen is said as soon as the picker closes,
/// while it's copied for the app ([pickOnDevice]).
Future<List<String>> pickCardFiles(
  BuildContext context, {
  Rect? from,
  OnChosen? onChosen,
}) async {
  if (!PhonePlatformPolicy.current.touch) return pickFiles();
  return pickOnDevice(context, from: from, camera: true, onChosen: onChosen);
}

/// The paperclip beside a card's field: the picker, the files chosen added
/// to [card]. [onNote] says what didn't go in.
class CardAttachButton extends StatelessWidget {
  const CardAttachButton({
    super.key,
    required this.card,
    required this.enabled,
    required this.onNote,
  });

  final String card;
  final bool enabled;
  final void Function(String? note) onNote;

  Future<void> _pick(BuildContext context) async {
    final backend = AppScope.of(context);
    final drafts = backend.attachments;
    // What was chosen shows under the field as soon as the picker closes,
    // "Adding photo…" while it's copied for the app.
    var chosen = const <String>[];
    final List<String> paths;
    try {
      paths = await pickCardFiles(
        context,
        from: pickerAnchor(context),
        onChosen: (kinds) => drafts.startAdding(card, chosen = kinds),
      );
    } on MissingPluginException {
      drafts.doneAdding(card, chosen);
      onNote(cardPickerMissing);
      return;
    } on Exception {
      drafts.doneAdding(card, chosen);
      onNote(cardPickerFailed);
      return;
    }
    final String? note;
    try {
      note = await addCardFiles(backend, card, paths);
    } finally {
      drafts.doneAdding(card, chosen);
    }
    onNote(note ?? notAdded(chosen, chosen.length - paths.length));
  }

  @override
  Widget build(BuildContext context) => GlyphButton(
    label: 'Attach files',
    active: false,
    // While its answer is being sent, nothing.
    onTap: enabled ? () => unawaited(_pick(context)) : () {},
    glyph: (color) =>
        ExtensionGlyph(icon: 'paperclip', name: 'Attach files', color: color),
  );
}

/// The files waiting to go with what's written on [card]: a chip each,
/// with × while nothing is being sent, and how far each got as it goes.
/// Nothing when none waits.
class CardFileChips extends StatelessWidget {
  const CardFileChips({
    super.key,
    required this.card,
    required this.busy,
    this.note,
  });

  final String card;

  /// Under the files while there are any: who may get them beyond the
  /// agent (a pipe's other side), so it's clear where they go.
  final Widget? note;

  /// The answer is being sent: the files stay as they are.
  final bool busy;

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final drafts = backend.attachments;
    final uploads = CardUploads.of(backend);
    return ListenableBuilder(
      listenable: Listenable.merge([drafts, uploads]),
      builder: (context, _) {
        final files = drafts.of(card);
        final adding = drafts.adding(card);
        if (files.isEmpty && adding.isEmpty) return const SizedBox.shrink();
        return Padding(
          padding: const EdgeInsets.only(top: Space.xs),
          child: Wrap(
            spacing: Space.xs,
            runSpacing: Space.xs,
            children: [
              for (final file in files)
                AttachmentChip(
                  key: ValueKey(file.path),
                  name: file.name,
                  size: file.size,
                  progress: busy ? uploads.progress(card, file) : null,
                  onRemove: busy
                      ? null
                      : () {
                          drafts.remove(card, file);
                          uploads.drop(backend, card, file);
                        },
                ),
              for (final kind in adding) AddingChip(kind: kind),
              if (note case final note?)
                SizedBox(width: double.infinity, child: note),
            ],
          ),
        );
      },
    );
  }
}

/// While a field in [child] has the keys on a Mac, files dropped on the
/// window or pasted (Cmd+V with files or a picture copied) go to [card]
/// rather than the message box ([FileDrop.divert]); files dropped on
/// [child] itself go to it whether it has the keys or not (dragged from
/// the Finder, the window isn't the key one); [onNote] says what didn't
/// go in.
class CardFileDrop extends StatefulWidget {
  const CardFileDrop({
    super.key,
    required this.card,
    required this.label,
    required this.enabled,
    required this.onNote,
    required this.child,
  });

  final String card;

  /// What the window's drop overlay says they'd go to ("your answer to
  /// June").
  final String label;
  final bool enabled;
  final void Function(String? note) onNote;
  final Widget child;

  @override
  State<CardFileDrop> createState() => _CardFileDropState();
}

class _CardFileDropState extends State<CardFileDrop> implements CardDropTarget {
  Backend? _backend;
  var _focused = false;

  @override
  String get dropLabel => widget.label;

  @override
  void onFiles(List<String> paths) => unawaited(_dropped(paths));

  @override
  void initState() {
    super.initState();
    _sync();
  }

  /// The whole request this field is in, which takes files too.
  CardDropAreaState? _area;

  /// Whether nothing is pushed over this request (a phone layout's
  /// Settings, say): only then does it take files.
  var _onTop = true;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _backend = AppScope.of(context);
    final onTop = ModalRoute.isCurrentOf(context) ?? true;
    if (onTop != _onTop) {
      _onTop = onTop;
      _sync();
    }
    final area = context.findAncestorStateOfType<CardDropAreaState>();
    if (area != _area) {
      _area?._leave(this);
      _area = area?.._target = this;
    }
  }

  @override
  void didUpdateWidget(CardFileDrop old) {
    super.didUpdateWidget(old);
    _sync();
  }

  void _focusChanged(bool focused) {
    _focused = focused;
    _sync();
  }

  void _sync() {
    if (!FileDrop.windowDrops) return;
    final takes = widget.enabled && _onTop;
    if (takes) {
      FileDrop.addCard(this);
    } else {
      FileDrop.removeCard(this);
    }
    if (_focused && takes) {
      FileDrop.divert(this, _dropped, label: widget.label);
    } else {
      FileDrop.undivert(this);
    }
  }

  Future<void> _dropped(List<String> paths) async {
    final backend = _backend;
    if (backend == null || !widget.enabled) return;
    final note = await addCardFiles(backend, widget.card, paths);
    if (mounted) widget.onNote(note);
  }

  @override
  void dispose() {
    _area?._leave(this);
    FileDrop.undivert(this);
    FileDrop.removeCard(this);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => MetaData(
    // Found under a drop's point ([FileDrop.cardAt]), padding and all.
    metaData: this,
    behavior: HitTestBehavior.translucent,
    child: Focus(
      canRequestFocus: false,
      skipTraversal: true,
      includeSemantics: false,
      onFocusChange: _focusChanged,
      child: widget.child,
    ),
  );
}

/// A whole request that takes files: dropped anywhere on [child], not
/// only on its field, they go to the [CardFileDrop] inside it.
class CardDropArea extends StatefulWidget {
  const CardDropArea({super.key, required this.child});

  final Widget child;

  @override
  State<CardDropArea> createState() => CardDropAreaState();
}

class CardDropAreaState extends State<CardDropArea> {
  CardDropTarget? _target;

  /// What files dropped on the request go to, if it takes them now.
  CardDropTarget? get target => _target;

  void _leave(CardDropTarget target) {
    if (identical(_target, target)) _target = null;
  }

  @override
  Widget build(BuildContext context) => MetaData(
    metaData: this,
    behavior: HitTestBehavior.translucent,
    child: widget.child,
  );
}

/// The files the person sent with a turn of a card (its talk, its answer,
/// a step's comment), from the backend: each opens in a viewer, as a
/// message's do.
class CardSentFiles extends StatelessWidget {
  const CardSentFiles(this.files, {super.key});

  final List<Attachment> files;

  @override
  Widget build(BuildContext context) {
    if (files.isEmpty) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(top: Space.xs),
      child: Align(
        alignment: AlignmentDirectional.centerStart,
        child: SentFiles(files),
      ),
    );
  }
}
