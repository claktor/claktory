// A sealed file (the Seal extension): an agent recorded its SHA-256 at a
// moment, so the person can tell when it changed since. A viewer showing
// one says so: a quiet "Sealed" in its details while it still matches, a
// line under its header once it changed, with Open sealed version to see
// what was sealed (a page of its own on a phone, in the pane's place
// elsewhere, with Show current version to come back).
import 'package:flutter/widgets.dart';

import '../controls.dart' show Button, ExtensionGlyph, showModal;
import '../forms.dart' show FormFrame;
import '../identity.dart';
import '../message_time.dart' show clockTime, dayLine;
import '../mobile/metrics.dart' show TypeScale;
import '../mobile/presentation.dart' show SheetAction, showActions;
import '../protocol/factory/v1/factory.pb.dart';
import '../selection.dart' show Unselectable;

/// The seal [seals] is checked against, its newest; null when it has none.
FileSeal? latestSeal(FileSeals? seals) =>
    seals == null || seals.seals.isEmpty ? null : seals.seals.first;

/// Whether [seals] says the file changed since its latest seal.
bool changedSinceSealed(FileSeals? seals) =>
    latestSeal(seals) != null &&
    seals!.state == FileSealState.FILE_SEAL_STATE_CHANGED;

/// Whether [seals] says the file is still what its latest seal names.
bool stillSealed(FileSeals? seals) =>
    latestSeal(seals) != null &&
    seals!.state == FileSealState.FILE_SEAL_STATE_MATCHES;

/// When [seal] was made, to follow "sealed it": "today at 14:02",
/// "yesterday at 9:15 AM", "on Monday at 14:02", "on Mon 17 Nov at 14:02".
String sealedWhen(FileSeal seal, DateTime now, {bool h24 = true}) {
  final ms = seal.sealedMs.toInt();
  final day = dayLine(ms, now);
  final on = switch (day) {
    'Today' => 'today',
    'Yesterday' => 'yesterday',
    _ => 'on $day',
  };
  return '$on at ${clockTime(ms, h24: h24)}';
}

/// What the line under a sealed file's header says.
enum SealNoticeKind {
  /// The file changed since [SealNotice.seal]: Open sealed version.
  changed,

  /// What shows is the version [SealNotice.seal] kept.
  version,
}

/// The line under a viewer's header about a file's seal: who sealed it and
/// when, the seal's note under it, and its one action at the end, wrapping
/// under the words where they don't leave room.
class SealNotice extends StatelessWidget {
  const SealNotice({
    super.key,
    required this.kind,
    required this.seal,
    this.onAction,
    this.onDelete,
    this.error,
  });

  final SealNoticeKind kind;
  final FileSeal seal;

  /// Open sealed version, or Show current version; none where there's
  /// nothing to open (a version too big to keep) or Back already does it.
  final VoidCallback? onAction;

  /// Delete kept copy…, beside Open sealed version: the person's alone.
  final VoidCallback? onDelete;

  /// Why the kept copy couldn't be deleted.
  final String? error;

  @override
  Widget build(BuildContext context) {
    final who = seal.agentName.isEmpty ? 'An agent' : seal.agentName;
    final when = sealedWhen(
      seal,
      DateTime.now(),
      h24: MediaQuery.alwaysUse24HourFormatOf(context),
    );
    final words = switch (kind) {
      SealNoticeKind.changed => 'Changed since $who sealed it $when.',
      SealNoticeKind.version => 'The version $who sealed $when.',
    };
    final action = switch (kind) {
      SealNoticeKind.changed => 'Open sealed version',
      SealNoticeKind.version => 'Show current version',
    };
    final note = seal.note.trim();
    // Under a page's header it grows with the text only to twice its
    // size, as the device banners do, so at the largest sizes it never
    // takes the screen from what it's about.
    return MediaQuery.withClampedTextScaling(
      maxScaleFactor: 2,
      child: _line(context, words, action, note),
    );
  }

  Widget _line(BuildContext context, String words, String action, String note) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final onAction = this.onAction;
    final onDelete = this.onDelete;
    final error = this.error;
    return Container(
      key: ValueKey('seal-notice-${kind.name}'),
      padding: const EdgeInsets.symmetric(
        horizontal: Space.l,
        vertical: Space.s,
      ),
      // On whatever the header's on: it reads as part of it.
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      child: Wrap(
        crossAxisAlignment: WrapCrossAlignment.center,
        alignment: WrapAlignment.spaceBetween,
        spacing: Space.m,
        runSpacing: Space.s,
        children: [
          Row(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Padding(
                // Level with the first line's middle.
                padding: EdgeInsets.only(
                  top: (type.caption.fontSize! * 1.4 - 14) / 2,
                  right: Space.s,
                ),
                child: ExcludeSemantics(
                  child: ExtensionGlyph(
                    icon: 'stamp',
                    name: 'Seal',
                    color: kind == SealNoticeKind.changed
                        ? palette.text
                        : palette.textMuted,
                    size: 14,
                  ),
                ),
              ),
              Flexible(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      words,
                      style: type.caption.copyWith(
                        color: palette.text,
                        fontWeight: kind == SealNoticeKind.changed
                            ? FontWeight.w600
                            : FontWeight.w400,
                      ),
                    ),
                    if (note.isNotEmpty)
                      Text(
                        note,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: type.caption.copyWith(color: palette.textMuted),
                      ),
                    if (error != null)
                      Text(
                        error,
                        style: type.caption.copyWith(color: palette.danger),
                      ),
                  ],
                ),
              ),
            ],
          ),
          if (onAction != null || onDelete != null)
            Unselectable(
              child: Wrap(
                spacing: Space.s,
                runSpacing: Space.s,
                children: [
                  if (onAction != null)
                    Button(label: action, onPressed: onAction),
                  if (onDelete != null)
                    Button(label: deleteKeptCopyLabel, onPressed: onDelete),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

const deleteKeptCopyLabel = 'Delete kept copy…';

/// Asks before the copy kept of [seal]'s version goes: it goes for every
/// seal of the same content, on any file. True to delete it.
Future<bool> confirmDeleteKeptCopy(BuildContext context) async {
  const title = 'Delete the kept copy of this version?';
  const body =
      "The seal stays, but this version won't open any more, here or for "
      'any other seal of the same content.';
  if (isPhoneLayout(context)) {
    return await showActions<bool>(
          context,
          title: title,
          message: body,
          actions: const [
            SheetAction('Delete kept copy', true, destructive: true),
          ],
        ) ??
        false;
  }
  var yes = false;
  await showModal(
    context,
    (context, close) => FormFrame(
      title: title,
      body: body,
      fields: const [],
      error: null,
      submitLabel: 'Delete kept copy',
      onSubmit: () {
        yes = true;
        close();
      },
      onCancel: close,
    ),
  );
  return yes;
}

/// Where a viewer in a pane shows a sealed version of its file in its own
/// place ([SealedVersionHost]).
class SealedVersions extends InheritedWidget {
  const SealedVersions({
    super.key,
    required this.shown,
    required this.show,
    required super.child,
  });

  /// The seal whose version shows; null for the file as it is.
  final FileSeal? shown;

  /// Shows [seal]'s version, or the file as it is (null).
  final ValueChanged<FileSeal?> show;

  static SealedVersions? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<SealedVersions>();

  @override
  bool updateShouldNotify(SealedVersions old) =>
      old.shown != shown || old.show != show;
}

/// Holds which version of a pane's file shows: the file as it is, or a
/// sealed version of it, built by [builder] (null: as it is).
class SealedVersionHost extends StatefulWidget {
  const SealedVersionHost({super.key, required this.builder});

  final Widget Function(BuildContext context, FileSeal? sealed) builder;

  @override
  State<SealedVersionHost> createState() => _SealedVersionHostState();
}

class _SealedVersionHostState extends State<SealedVersionHost> {
  FileSeal? _shown;

  void _show(FileSeal? seal) => setState(() => _shown = seal);

  @override
  Widget build(BuildContext context) => SealedVersions(
    shown: _shown,
    show: _show,
    child: Builder(builder: (context) => widget.builder(context, _shown)),
  );
}
