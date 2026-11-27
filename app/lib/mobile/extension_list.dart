import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../controls.dart';
import '../extension_about.dart' show hasExtensionView;
import '../extensions.dart' show enabledExtensions;
import '../identity.dart';
import '../layout.dart';
import '../panes.dart' show pageOfKind, paneGlyph, paneTitle;
import '../pinned_agents.dart' show pinLabel;
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import 'metrics.dart';

/// What a phone's ⋯ beside an agent's name lists (research mobile.md §4):
/// the files beside its conversation, then its extensions with something
/// to show (its checklist, its tasks…), then, behind "Show N more", those
/// that only say what they do (Web, Viewer…), so the ones worth opening
/// aren't lost in a list of fourteen. One row each, a finger's height and
/// growing with the text: choosing one opens its panel, or pushes the
/// file.
class PhoneExtensionList extends StatefulWidget {
  const PhoneExtensionList({
    super.key,
    required this.agent,
    required this.chosen,
    required this.viewed,
    this.onSearch,
    this.onVersions,
    this.onPin,
    this.onSettings,
  });

  final Agent agent;

  /// Opens the agent's other versions of its conversation:
  /// a row after Search while it has any.
  final VoidCallback? onVersions;

  /// Opens the conversation's find: Search heads the list when it can.
  final VoidCallback? onSearch;

  /// An extension was chosen: its panel opens, so the list closes.
  final VoidCallback chosen;

  /// Runs after this sheet closes, on the phone's stable owner context.
  final ValueChanged<Agent>? onPin;
  final ValueChanged<Agent>? onSettings;

  /// A file beside the conversation was chosen: its pane's id.
  final ValueChanged<String> viewed;

  @override
  State<PhoneExtensionList> createState() => _PhoneExtensionListState();
}

class _PhoneExtensionListState extends State<PhoneExtensionList> {
  /// The ones with nothing of their own to show are in view.
  var _more = false;

  @override
  Widget build(BuildContext context) {
    // Live while it's open: a file an agent shows meanwhile comes in on top.
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => _list(
        context,
        store.workspace.agents.firstWhere(
          (agent) => agent.id == widget.agent.id,
          orElse: () => widget.agent,
        ),
      ),
    );
  }

  Widget _list(BuildContext context, Agent agent) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final workspace = backend.workspace.workspace;
    final extensions = enabledExtensions(workspace, agent);
    final pin = widget.onPin == null ? null : pinLabel(workspace, agent);
    // Secrets shows what the agent was given, though the backend sends no
    // view for it.
    bool shows(Extension extension) =>
        hasExtensionView(extension) || extension.id == 'secrets';
    final panels = [
      for (final extension in extensions)
        if (shows(extension)) extension,
    ];
    final others = [
      for (final extension in extensions)
        if (!shows(extension)) extension,
    ];
    final viewers = newestFirst([
      for (final pane in leaves(layoutOf(agent)))
        // Settings and other pages a Mac shows beside the conversation are
        // the phone's own screens here, and so are a project's Files.
        if (pane.leaf.kind != chatKind &&
            pane.leaf.kind != filesKind &&
            pageOfKind(pane.leaf.kind) == null)
          pane,
    ]);
    void open(Extension extension) {
      widget.chosen();
      fireAndLog(backend.openPanel(agent.id, extension.id));
    }

    Widget extensionRow(Extension extension) => _Row(
      glyph: ExtensionGlyph(
        icon: extension.icon,
        name: extension.name,
        color: palette.textMuted,
      ),
      name: extension.name,
      onTap: () => open(extension),
    );
    return SingleChildScrollView(
      padding: const EdgeInsets.only(bottom: Space.s),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (widget.onSearch case final search?)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: _Row(
                glyph: ExtensionGlyph(
                  icon: 'search',
                  name: 'Search',
                  color: palette.textMuted,
                ),
                name: 'Search',
                onTap: search,
              ),
            ),
          if (widget.onVersions case final versions?)
            _Row(
              glyph: ExtensionGlyph(
                icon: 'history',
                name: 'Versions',
                color: palette.textMuted,
              ),
              name: 'Versions (${agent.branches})…',
              label: 'Other versions, ${agent.branches}',
              onTap: versions,
            ),
          if (viewers.isNotEmpty) ...[
            const _Heading('Files'),
            for (final pane in viewers)
              _Row(
                glyph: paneGlyph(agent, pane.leaf, palette.textMuted),
                name: paneTitle(
                  pane.leaf,
                  AppScope.of(context).workspace.workspace,
                ),
                onTap: () => widget.viewed(pane.id),
              ),
          ],
          _Heading(
            extensions.isEmpty
                ? '${agent.name} has no extensions turned on.'
                : "${agent.name}'s extensions",
            // The sheet has no title of its own: this names it, even
            // under Files.
            namesRoute: true,
          ),
          for (final extension in panels) extensionRow(extension),
          if (others.isNotEmpty)
            if (_more || panels.isEmpty)
              for (final extension in others) extensionRow(extension)
            else
              _Row(
                glyph: Chevron(color: palette.textFaint, down: true),
                name: 'Show ${others.length} more',
                muted: true,
                onTap: () => setState(() => _more = true),
              ),
          if (pin != null || widget.onSettings != null) ...[
            Container(
              height: 1,
              margin: const EdgeInsets.symmetric(
                horizontal: Touch.gutter,
                vertical: Space.s,
              ),
              color: palette.hairline,
            ),
            if (pin != null)
              _Row(
                glyph: ExtensionGlyph(
                  icon: pin == 'Pin' ? 'pin' : 'pin-off',
                  name: pin,
                  color: palette.textMuted,
                ),
                name: pin,
                label: pin,
                onTap: () => widget.onPin!(agent),
              ),
            if (widget.onSettings case final settings?)
              _Row(
                glyph: CogGlyph(color: palette.textMuted),
                name: 'Agent settings…',
                label: 'Agent settings',
                onTap: () => settings(agent),
              ),
          ],
        ],
      ),
    );
  }
}

class _Heading extends StatelessWidget {
  const _Heading(this.text, {this.namesRoute = false});

  final String text;

  /// Whether VoiceOver and TalkBack announce it as the sheet's name.
  final bool namesRoute;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(
      Touch.gutter,
      Space.m,
      Touch.gutter,
      Space.xs,
    ),
    child: Semantics(
      header: true,
      namesRoute: namesRoute,
      child: Text(
        text,
        style: PhoneType.secondary.copyWith(
          color: Palette.of(context).textMuted,
          fontWeight: FontWeight.w500,
        ),
      ),
    ),
  );
}

/// [panes] newest first: the file an agent most recently
/// showed, or the person most recently opened, on top, the one most
/// likely wanted. Equal times (all 0, from before the backend kept them)
/// keep their layout order.
List<ViewNode> newestFirst(List<ViewNode> panes) {
  final ordered = panes.indexed.toList()
    ..sort((a, b) {
      final newer = b.$2.leaf.openedMs.compareTo(a.$2.leaf.openedMs);
      return newer != 0 ? newer : a.$1.compareTo(b.$1);
    });
  return [for (final (_, pane) in ordered) pane];
}

/// A row as Settings' are on a phone: its glyph, its name, at least 52pt
/// tall and growing with the text.
class _Row extends StatelessWidget {
  const _Row({
    required this.glyph,
    required this.name,
    required this.onTap,
    this.muted = false,
    this.label,
  });

  final Widget glyph;
  final String name;
  final VoidCallback onTap;

  /// What VoiceOver reads, when it isn't [name].
  final String? label;

  /// A row that shows more rather than opening something.
  final bool muted;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      container: true,
      button: true,
      label: label,
      excludeSemantics: label != null,
      child: Pressable(
        onTap: onTap,
        builder: (context, pressed) => Container(
          constraints: const BoxConstraints(minHeight: Touch.row),
          color: pressed ? palette.hover : const Color(0x00000000),
          padding: const EdgeInsets.symmetric(
            horizontal: Touch.gutter,
            vertical: Space.s,
          ),
          child: Row(
            children: [
              SizedBox(
                width: 36,
                child: Align(
                  alignment: AlignmentDirectional.centerStart,
                  child: glyph,
                ),
              ),
              Expanded(
                child: Text(
                  name,
                  style: PhoneType.body.copyWith(
                    color: muted ? palette.textMuted : palette.text,
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
