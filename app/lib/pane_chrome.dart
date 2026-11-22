import 'dart:async';

import 'package:flutter/widgets.dart';

import 'controls.dart' show ExtensionGlyph;
import 'identity.dart';
import 'mobile/header.dart';
import 'mobile/metrics.dart';
import 'mobile/page_find.dart' show phonePageSearch;
import 'mobile/presentation.dart';

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

  /// The pane's header with [details] and [controls]; null when there's no
  /// header.
  final Widget Function(String details, List<Widget> controls)? header;

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
  });

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
    if (chrome?.phone case final phone?) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          PhonePaneHeader(
            pane: phone,
            details: details,
            comments: comments,
            actions: actions,
          ),
          Expanded(child: body),
          ?toolbar,
        ],
      );
    }
    final header = chrome?.header;
    if (header == null) return body;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        header(details, controls),
        Expanded(child: body),
      ],
    );
  }
}

/// A viewer's header on a phone: Back, the file's name over its details,
/// then the comments and More.
class PhonePaneHeader extends StatelessWidget {
  const PhonePaneHeader({
    super.key,
    required this.pane,
    this.details = '',
    this.comments,
    this.actions = const [],
  });

  final PhonePane pane;
  final String details;
  final Widget? comments;
  final List<PaneAction> actions;

  Future<void> _more(BuildContext context) async {
    final onRemove = pane.onRemove;
    // Find in what it shows, first (a phone's page bar, mobile/page_find).
    final search = phonePageSearch(context);
    final chosen = await showActions<int>(
      context,
      actions: [
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
        pane.onRemove != null ||
        phonePageSearch(context) != null;
    return PhoneHeader(
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
