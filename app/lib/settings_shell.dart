// The pieces Settings is made of at every width (Lucy's Settings spec §4):
// the search field, the Claktory's heading, a section's row in the index or
// the list, a search result, and the anchors a link or a result brings into
// view and glows. settings_view.dart puts them together.
import 'package:flutter/gestures.dart' show TapGestureRecognizer;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'claktory_identity.dart' show claktoryColor, claktoryNameInSettings;
import 'controls.dart';
import 'forms.dart' show FittedHeading;
import 'highlights.dart' show Glow;
import 'identity.dart';
import 'mobile/drawer.dart' show largeText;
import 'mobile/input.dart' show InputKind;
import 'mobile/metrics.dart';
import 'mobile/platform_policy.dart' show PhonePlatformPolicy;
import 'scope.dart';
import 'settings_search.dart';

/// Settings' search, always on screen at the top of the index or the list
/// (§4.3): "Search", with Clear once something's typed.
class SettingsSearchField extends StatelessWidget {
  const SettingsSearchField({
    super.key,
    required this.controller,
    required this.focusNode,
    this.onSubmitted,
  });

  final TextEditingController controller;
  final FocusNode focusNode;

  /// Return: the first result.
  final VoidCallback? onSubmitted;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final touch = touchFirst(context);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: palette.hover,
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Row(
        children: [
          Expanded(
            child: TextFieldBox(
              focusNode: focusNode,
              child: ConstrainedBox(
                // iOS's search field's height on touch, growing with the
                // text; a Mac's control height.
                constraints: BoxConstraints(
                  minHeight: touch
                      ? PhonePlatformPolicy.least(context, Touch.denseLine)
                      : 28,
                ),
                child: Padding(
                  padding: const EdgeInsetsDirectional.only(start: Space.s),
                  child: Row(
                    children: [
                      ExcludeSemantics(
                        child: ExtensionGlyph(
                          icon: 'search',
                          name: '',
                          color: palette.textFaint,
                          size: touch ? 16 : 14,
                        ),
                      ),
                      Expanded(
                        child: TextField(
                          controller: controller,
                          focusNode: focusNode,
                          placeholder: 'Search',
                          semanticsLabel: 'Search Settings',
                          bordered: false,
                          kind: InputKind.text,
                          textInputAction: TextInputAction.search,
                          autocorrect: false,
                          onSubmitted: (_) => onSubmitted?.call(),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
          ListenableBuilder(
            listenable: controller,
            builder: (context, _) => controller.text.isEmpty
                ? const SizedBox.shrink()
                : GlyphButton(
                    label: 'Clear search',
                    active: false,
                    onTap: () {
                      controller.clear();
                      focusNode.requestFocus();
                    },
                    glyph: (color) =>
                        ExtensionGlyph(icon: 'x', name: '', color: color),
                  ),
          ),
        ],
      ),
    );
  }
}

/// The Claktory being shown, over its sections (§3): its 8px color dot and
/// its name, in caption weight 600 `textFaint`. Switching Claktory in the
/// rail changes it. [dense] for the index; the list's sits in its gutter.
class SettingsClaktoryHeading extends StatelessWidget {
  const SettingsClaktoryHeading({super.key});

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) => SettingsClaktoryLine(
        name: claktoryNameInSettings(backend),
        color: claktoryColor(backend),
        style: TypeScale.of(context).caption
            .copyWith(color: palette.textFaint, fontWeight: FontWeight.w600),
      ),
    );
  }
}

/// A Claktory's dot and name in [style]: the index's heading, and a
/// narrow section page's first line when more than one Claktory is saved
/// (§4.2).
class SettingsClaktoryLine extends StatelessWidget {
  const SettingsClaktoryLine({
    super.key,
    required this.name,
    required this.color,
    required this.style,
  });

  final String name;
  final Color? color;
  final TextStyle style;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      header: true,
      child: Row(
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(
              color: color ?? palette.textFaint,
              shape: BoxShape.circle,
            ),
          ),
          const SizedBox(width: Space.s),
          Expanded(
            child: Text(
              name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: style,
            ),
          ),
        ],
      ),
    );
  }
}

/// A section's row: in the wide index (glyph and title, selected in
/// `selected`, 28px on a Mac and 44pt on touch), or in the narrow list
/// ([list]: what it's set to at the right, and a chevron to its page).
/// Focusable, with the signal ring; Enter opens it.
class SettingsSectionRow extends StatefulWidget {
  const SettingsSectionRow({
    super.key,
    required this.title,
    required this.glyph,
    required this.onTap,
    this.summary = '',
    this.selected = false,
    this.list = false,
    this.focusNode,
  });

  final String title;
  final Widget Function(Color color) glyph;
  final VoidCallback onTap;
  final String summary;
  final bool selected;
  final bool list;
  final FocusNode? focusNode;

  @override
  State<SettingsSectionRow> createState() => _SettingsSectionRowState();
}

class _SettingsSectionRowState extends State<SettingsSectionRow> {
  var _hovered = false;
  var _focused = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final touch = touchFirst(context);
    final phone = isPhoneLayout(context);
    final large = widget.list && largeText(context);
    final bodyStyle = phone ? PhoneType.body : TypeScale.of(context).body;
    // Never broken inside a word ("Notification / s" at the largest text
    // on an iPhone SE, Zucchero's row 510): drawn smaller to fit.
    final title = FittedHeading(
      widget.title,
      style: bodyStyle.copyWith(color: palette.text),
    );
    final summary = !widget.list || widget.summary.isEmpty
        ? null
        : Text(
            widget.summary,
            maxLines: large ? 2 : 1,
            overflow: TextOverflow.ellipsis,
            textAlign: large ? TextAlign.start : TextAlign.end,
            style: (large && phone ? PhoneType.secondary : bodyStyle).copyWith(
              color: palette.textMuted,
            ),
          );
    final double height = touch
        ? PhonePlatformPolicy.least(
            context,
            widget.list && phone ? Touch.row : Touch.target,
          )
        : widget.list
        ? 32
        : 28;
    return Semantics(
      container: true,
      button: true,
      selected: widget.selected,
      child: FocusableActionDetector(
        focusNode: widget.focusNode,
        mouseCursor: SystemMouseCursors.click,
        onShowHoverHighlight: (value) => setState(() => _hovered = value),
        onShowFocusHighlight: (value) => setState(() => _focused = value),
        actions: {
          ActivateIntent: CallbackAction<ActivateIntent>(
            onInvoke: (_) {
              widget.onTap();
              return null;
            },
          ),
        },
        child: Pressable(
          onTap: widget.onTap,
          builder: (context, pressed) => Container(
            constraints: BoxConstraints(minHeight: height),
            padding: EdgeInsets.symmetric(
              horizontal: widget.list && phone ? Touch.gutter : Space.s,
              vertical: touch ? Space.xs : 0,
            ),
            decoration: BoxDecoration(
              color: widget.selected
                  ? palette.selected
                  : pressed || _hovered
                  ? palette.hover
                  : const Color(0x00000000),
              borderRadius: widget.list && phone
                  ? null
                  : BorderRadius.circular(Radii.control),
              border: widget.list && phone
                  ? null
                  : Border.all(
                      color: _focused
                          ? palette.signal
                          : const Color(0x00000000),
                    ),
            ),
            child: Row(
              children: [
                SizedBox(
                  width: widget.list && phone ? 36 : 24,
                  child: Align(
                    alignment: AlignmentDirectional.centerStart,
                    child: widget.glyph(
                      widget.selected ? palette.text : palette.textMuted,
                    ),
                  ),
                ),
                if (large)
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [title, ?summary],
                    ),
                  )
                else if (widget.list) ...[
                  title,
                  const SizedBox(width: Space.m),
                  Expanded(child: summary ?? const SizedBox.shrink()),
                ] else
                  Expanded(child: title),
                if (widget.list) ...[
                  const SizedBox(width: Space.s),
                  Chevron(color: palette.textFaint, down: false),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A search result (§4.3): the setting's label in body, its section in
/// caption `textMuted` under it; an answer alone, in `textMuted`, leading
/// nowhere.
class SettingsResultRow extends StatefulWidget {
  const SettingsResultRow({
    super.key,
    required this.result,
    required this.onTap,
    this.first = false,
  });

  final SettingsResult result;
  final VoidCallback onTap;

  /// Return in the search field opens it.
  final bool first;

  @override
  State<SettingsResultRow> createState() => _SettingsResultRowState();
}

class _SettingsResultRowState extends State<SettingsResultRow> {
  var _hovered = false;
  var _focused = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    final body = phone ? PhoneType.body : TypeScale.of(context).body;
    final caption = phone ? PhoneType.secondary : TypeScale.of(context).caption;
    final result = widget.result;
    final padding = EdgeInsets.symmetric(
      horizontal: phone ? Touch.gutter : Space.s,
      vertical: Space.xs + 2,
    );
    if (result.isAnswer) {
      return Padding(
        padding: padding,
        child: Text(
          result.label,
          style: body.copyWith(color: palette.textMuted),
        ),
      );
    }
    final showSection = result.label != result.sectionTitle;
    return Semantics(
      container: true,
      button: true,
      child: FocusableActionDetector(
        mouseCursor: SystemMouseCursors.click,
        onShowHoverHighlight: (value) => setState(() => _hovered = value),
        onShowFocusHighlight: (value) => setState(() => _focused = value),
        actions: {
          ActivateIntent: CallbackAction<ActivateIntent>(
            onInvoke: (_) {
              widget.onTap();
              return null;
            },
          ),
        },
        child: Pressable(
          onTap: widget.onTap,
          builder: (context, pressed) => Container(
            constraints: BoxConstraints(
              minHeight: touchFirst(context)
                  ? PhonePlatformPolicy.least(context, Touch.target)
                  : 28,
            ),
            padding: padding,
            decoration: BoxDecoration(
              color: pressed || _hovered
                  ? palette.hover
                  : const Color(0x00000000),
              borderRadius: phone ? null : BorderRadius.circular(Radii.control),
              border: phone
                  ? null
                  : Border.all(
                      color: _focused
                          ? palette.signal
                          : const Color(0x00000000),
                    ),
            ),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(result.label, style: body.copyWith(color: palette.text)),
                if (showSection)
                  Text(
                    result.sectionTitle,
                    style: caption.copyWith(color: palette.textMuted),
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// What search says when it finds nothing: No results for “wallpaper”.
class SettingsNoResults extends StatelessWidget {
  const SettingsNoResults({super.key, required this.query});

  final String query;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    return Padding(
      padding: EdgeInsets.symmetric(
        horizontal: phone ? Touch.gutter : Space.s,
        vertical: Space.s,
      ),
      child: Text(
        'No results for “${query.trim()}”',
        style: (phone ? PhoneType.body : TypeScale.of(context).body).copyWith(
          color: palette.textMuted,
        ),
      ),
    );
  }
}

/// Where a section's [SettingAnchor]s are kept while it's shown, so a link
/// or a search result can bring one into view ([reveal]).
class SettingAnchors extends StatefulWidget {
  const SettingAnchors({super.key, required this.child});

  final Widget child;

  /// The nearest above [context].
  static SettingAnchorsState? of(BuildContext context) =>
      context.findAncestorStateOfType<SettingAnchorsState>();

  /// Reveals the setting [id] under [anchors] once it's there: a section
  /// just opened is laid out frames later, and may move to another body
  /// meanwhile (wide to narrow as a pane opens), so it tries each frame
  /// for a few.
  static void revealWhenReady(
    GlobalKey<SettingAnchorsState> anchors,
    String id, [
    int frames = 12,
  ]) {
    if (id.isEmpty) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (anchors.currentState?.reveal(id) ?? false) return;
      if (frames > 1) revealWhenReady(anchors, id, frames - 1);
    });
    WidgetsBinding.instance.ensureVisualUpdate();
  }

  @override
  State<SettingAnchors> createState() => SettingAnchorsState();
}

class SettingAnchorsState extends State<SettingAnchors> {
  final _anchors = <String, BuildContext>{};

  /// Brings the setting [id] into view once it's laid out, its top near
  /// the top, and glows it in signal as a Highlight (§4.3). Whether it's
  /// there.
  bool reveal(String id) {
    final anchor = _anchors[id];
    if (anchor == null || !anchor.mounted) return false;
    Scrollable.ensureVisible(
      anchor,
      alignment: 0.1,
      duration: Motion.base,
      curve: Curves.easeOut,
    );
    AppScope.of(context).highlights
        .glow(settingGlowKey(id), [Palette.of(context).signal]);
    return true;
  }

  /// [reveal] after the next frame, when what was just opened is laid out.
  void revealSoon(String id) {
    if (id.isEmpty) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) reveal(id);
    });
    WidgetsBinding.instance.ensureVisualUpdate();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

/// The highlight key a setting glows under.
String settingGlowKey(String id) => 'setting:$id';

/// A setting a link or a search result can go to by [id] ("zoom",
/// "move"): brought into view and glowed.
class SettingAnchor extends StatefulWidget {
  const SettingAnchor({super.key, required this.id, required this.child});

  final String id;
  final Widget child;

  @override
  State<SettingAnchor> createState() => _SettingAnchorState();
}

class _SettingAnchorState extends State<SettingAnchor> {
  SettingAnchorsState? _host;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _host = SettingAnchors.of(context);
    _host?._anchors[widget.id] = context;
  }

  @override
  void didUpdateWidget(SettingAnchor oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.id == widget.id) return;
    _forget(oldWidget.id);
    _host?._anchors[widget.id] = context;
  }

  @override
  void dispose() {
    _forget(widget.id);
    super.dispose();
  }

  void _forget(String id) {
    final anchors = _host?._anchors;
    if (anchors != null && anchors[id] == context) anchors.remove(id);
  }

  @override
  Widget build(BuildContext context) => Glow(
    store: AppScope.of(context).highlights,
    matches: (key) => key == settingGlowKey(widget.id),
    child: widget.child,
  );
}

/// A sentence that names a place in Settings, with the place as a link to
/// it (Lucy's Settings spec §5): [before], then [label] ("Settings →
/// General"), then [after], in [style]. The link opens [section], with
/// [setting] in it glowing; [onOpen] runs first (a dialog closing, say).
class SettingsLink extends StatefulWidget {
  const SettingsLink({
    super.key,
    this.before = '',
    required this.label,
    this.after = '',
    required this.section,
    this.setting = '',
    required this.style,
    this.onOpen,
  });

  final String before;
  final String label;
  final String after;
  final String section;
  final String setting;
  final TextStyle style;
  final VoidCallback? onOpen;

  @override
  State<SettingsLink> createState() => _SettingsLinkState();
}

class _SettingsLinkState extends State<SettingsLink> {
  late final _tap = TapGestureRecognizer()..onTap = _open;
  var _hovered = false;

  void _open() {
    widget.onOpen?.call();
    AppScope.openSettingsAt(context, widget.section, setting: widget.setting);
  }

  @override
  void dispose() {
    _tap.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    // Underlined at rest, as the app's other quiet links, so it reads as
    // one where there's no pointer to find it.
    final color = _hovered ? palette.text : palette.textMuted;
    return Text.rich(
      TextSpan(
        style: widget.style,
        children: [
          if (widget.before.isNotEmpty) TextSpan(text: widget.before),
          TextSpan(
            text: widget.label,
            recognizer: _tap,
            mouseCursor: SystemMouseCursors.click,
            onEnter: (_) => setState(() => _hovered = true),
            onExit: (_) => setState(() => _hovered = false),
            style: TextStyle(
              color: color,
              fontWeight: FontWeight.w500,
              decoration: TextDecoration.underline,
              decorationColor: color,
            ),
          ),
          if (widget.after.isNotEmpty) TextSpan(text: widget.after),
        ],
      ),
    );
  }
}
