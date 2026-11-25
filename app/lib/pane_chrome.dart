import 'dart:async';

import 'package:flutter/widgets.dart';

import 'controls.dart' show ExtensionGlyph, GlyphButton;
import 'identity.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'mobile/page_find.dart' show phonePageSearch;
import 'mobile/presentation.dart';
import 'mobile/viewer_chrome.dart';
import 'share_files.dart';

/// Where a pane's header is, filled by what the pane shows: its details
/// and controls. Above each pane ([Pane] in panes.dart). On a phone, where
/// a viewer is a page of its own, [phone] says what its header holds
/// besides what the viewer gives.
class PaneChrome extends InheritedWidget {
  const PaneChrome({
    super.key,
    this.header,
    this.phone,
    this.reloads,
    required super.child,
  });

  /// The pane's header with [details] and [controls], and [pinned] before
  /// Expand, which a narrow pane never clips as it clips [controls] from
  /// their end; null when there's no header.
  final Widget Function(
    String details,
    List<Widget> controls, {
    List<Widget> pinned,
  })?
  header;

  /// On a phone, the viewer's page: its title, Back, and the pane's own
  /// actions; [PaneScaffold] builds the header from it.
  final PhonePane? phone;

  /// Fires when an agent shows the pane's file again: the viewer reads it
  /// again.
  final Listenable? reloads;

  static PaneChrome? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<PaneChrome>();

  @override
  bool updateShouldNotify(PaneChrome old) =>
      old.header != header || old.phone != phone || old.reloads != reloads;
}

/// A viewer shown as a phone's page (research mobile.md §4, "Comments and
/// annotations"): a header of Back, the file's name over its details, the
/// comments' count and More, which holds the viewer's own actions and,
/// for a pane of an agent's layout, Close everywhere.
class PhonePane {
  const PhonePane({required this.title, required this.onBack, this.onRemove});

  /// The file's name.
  final String title;

  /// Back: off this phone's screen only. The pane stays in the agent's
  /// layout, for its other devices.
  final VoidCallback onBack;

  /// Takes the pane out of the agent's layout, on every device: in More,
  /// asked first.
  final VoidCallback? onRemove;
}

/// One of a viewer's actions in a phone's More: "Fit", "Plain text",
/// "Reload". [active] marks one that's on, with a check.
class PaneAction {
  const PaneAction(this.label, this.onTap, {this.active = false});

  final String label;
  final VoidCallback onTap;
  final bool active;
}

/// A pane's content: its header (from [PaneChrome]), then [body]. On a
/// phone the header is a [PhoneHeader] instead, with [comments] and More
/// ([actions]), and [toolbar] sits under the body, in the thumb's reach.
class PaneScaffold extends StatelessWidget {
  const PaneScaffold({
    super.key,
    required this.body,
    this.details = '',
    this.controls = const [],
    this.comments,
    this.actions = const [],
    this.toolbar,
    this.share,
    this.phoneKind = PhoneViewerKind.document,
    this.phoneOverlay,
  });

  /// On a phone: a picture or video fills the screen, its chrome over it;
  /// a document keeps its header above it ([PhoneViewerScaffold]).
  final PhoneViewerKind phoneKind;

  /// On a phone, over a picture or video's foot, above [toolbar]: a
  /// video's controls.
  final Widget? phoneOverlay;

  /// The file it shows, which Share… fetches and sends with the system's
  /// share sheet (#418): in More on a phone, in the header elsewhere.
  /// Null where there's nothing to share.
  final String? share;

  final Widget body;
  final String details;
  final List<Widget> controls;

  /// On a phone: the comments' button in the header (how many, opening
  /// them).
  final Widget? comments;

  /// On a phone: what More holds, in place of [controls].
  final List<PaneAction> actions;

  /// On a phone: a bar under the body (Comment, a video's controls).
  final Widget? toolbar;

  @override
  Widget build(BuildContext context) {
    final chrome = PaneChrome.of(context);
    final share = canShareFiles ? this.share : null;
    if (chrome?.phone case final phone?) {
      return PhoneViewerScaffold(
        kind: phoneKind,
        onClose: phone.onBack,
        header: PhonePaneHeader(
          pane: phone,
          details: details,
          comments: comments,
          actions: actions,
          share: share == null
              ? null
              : () => unawaited(shareFiles(context, [share])),
          // Over a picture or video, its bar draws its own fill.
          background: phoneKind == PhoneViewerKind.document
              ? null
              : const Color(0x00000000),
        ),
        overlay: phoneOverlay,
        toolbar: toolbar,
        body: body,
      );
    }
    final header = chrome?.header;
    if (header == null) return body;
    final shareButton = share == null
        ? null
        : Builder(
            // The share menu points at the button (an iPad's popover).
            builder: (context) => GlyphButton(
              label: shareLabel(),
              active: false,
              onTap: () => shareFrom(context, [share]),
              glyph: (color) =>
                  ExtensionGlyph(icon: 'share', name: 'Share', color: color),
            ),
          );
    // On an iPad it's how a file is saved or sent on (Bruno 27653): never
    // clipped, as the viewer's controls are from their end on a narrow
    // pane beside the conversation. On a Mac, last of them.
    final touch = touchFirst(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        header(
          details,
          [...controls, if (!touch) ?shareButton],
          pinned: [if (touch) ?shareButton],
        ),
        Expanded(child: body),
      ],
    );
  }
}

/// A viewer's header on a phone: Back, the file's name over its details,
/// then the comments (or Share, for a file that has none) and More, whose
/// sheet starts with Share.
class PhonePaneHeader extends StatelessWidget {
  const PhonePaneHeader({
    super.key,
    required this.pane,
    this.details = '',
    this.comments,
    this.actions = const [],
    this.share,
    this.background,
  });

  final PhonePane pane;
  final String details;
  final Widget? comments;
  final List<PaneAction> actions;

  /// Its fill, the sidebar's unless given ([PhoneHeader.background]).
  final Color? background;

  /// Shares the file shown (#418): Save Video, Save to Files, AirDrop…
  final VoidCallback? share;

  static Widget _shareGlyph(Color color, double size) =>
      ExtensionGlyph(icon: 'share', name: 'Share', color: color, size: size);

  Future<void> _more(BuildContext context) async {
    final onRemove = pane.onRemove;
    // Find in what it shows, first (a phone's page bar, mobile/page_find).
    final search = phonePageSearch(context);
    final chosen = await showActions<int>(
      context,
      actions: [
        // First: what Matt looked for to save a video (Bruno 27653).
        if (share != null)
          SheetAction(
            shareLabel(),
            -3,
            glyph: (color) => _shareGlyph(color, 16),
          ),
        if (search != null)
          SheetAction(
            'Search',
            -2,
            glyph: (color) => ExtensionGlyph(
              icon: 'search',
              name: 'Search',
              color: color,
              size: 16,
            ),
          ),
        for (final (index, action) in actions.indexed)
          SheetAction(
            action.label,
            index,
            glyph: action.active
                ? (color) => ExtensionGlyph(
                    icon: 'check',
                    name: 'On',
                    color: color,
                    size: 16,
                  )
                : null,
          ),
        if (onRemove != null)
          const SheetAction('Close everywhere', -1, destructive: true),
      ],
    );
    if (chosen == null || !context.mounted) return;
    if (chosen == -2) return search?.call();
    if (chosen == -3) return share?.call();
    if (chosen >= 0) {
      actions[chosen].onTap();
      return;
    }
    // On every device: asked first.
    final sure = await showActions<bool>(
      context,
      message:
          'It closes beside the conversation on every device. '
          'The file itself stays.',
      actions: const [SheetAction('Close everywhere', true, destructive: true)],
    );
    if (sure == true) onRemove?.call();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final more =
        actions.isNotEmpty ||
        this.share != null ||
        pane.onRemove != null ||
        phonePageSearch(context) != null;
    final share = this.share;
    return PhoneHeader(
      background: background,
      leading: HeaderButton.back(onTap: pane.onBack),
      title: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            pane.title,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: type.emphasis.copyWith(color: palette.text),
          ),
          if (details.isNotEmpty)
            Text(
              details,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: type.caption.copyWith(color: palette.textMuted),
            ),
        ],
      ),
      trailing: [
        ?comments,
        // The header holds two at its end: Share beside More where the
        // comments don't take the place; always first in More.
        if (comments == null && share != null)
          HeaderButton(
            label: shareLabel(),
            onTap: share,
            glyph: (color) => _shareGlyph(color, 20),
          ),
        if (more)
          HeaderButton.more(
            label: 'More',
            onTap: () => unawaited(_more(context)),
          ),
      ],
    );
  }
}

/// Settings, the Marketplace or What's new shown in a pane beside an
/// agent's conversation, whose header names and closes it: the page leaves
/// out its own bar's title and Done, and doesn't take the keyboard's focus
/// from the conversation. As a page of its own (a phone, or no agent to
/// show it beside), there's none.
class InPane extends InheritedWidget {
  const InPane({super.key, required super.child});

  static bool of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<InPane>() != null;

  @override
  bool updateShouldNotify(InPane old) => false;
}
