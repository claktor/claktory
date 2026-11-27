import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'mobile/input.dart' show InputKind;
import 'login.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';
import 'tell_rivet.dart' show ErrorLine;
import 'usage_wait.dart' show UsageAutoContinueSetting;

/// Settings → Providers: a card for each provider signed in or being
/// signed in to, then [ProviderPicker] to add another. With none, the
/// picker is the page's point: why, then the list itself.
class ProviderSettings extends StatefulWidget {
  const ProviderSettings({super.key, required this.workspace});

  final Workspace workspace;

  @override
  State<ProviderSettings> createState() => _ProviderSettingsState();
}

/// Where an agent's work goes, said before a provider is chosen (App Store
/// 5.1.2(i): data shared with a third-party AI is disclosed, and choosing
/// is the permission): on Connect a model ([adding] false) and above Add a
/// provider… in Settings → Providers.
String aiProviderConsent({required bool adding}) =>
    'To work, your agents send your messages and the files and pages they '
    'use to the AI provider you choose, such as Anthropic or OpenAI. '
    '${adding ? 'By adding one, you allow this.' : 'By choosing one below, you allow this.'}';

/// [aiProviderConsent] as it reads on screen: caption, muted.
class AiProviderConsent extends StatelessWidget {
  const AiProviderConsent({super.key, required this.adding});

  final bool adding;

  @override
  Widget build(BuildContext context) => Text(
    aiProviderConsent(adding: adding),
    style: TypeScale.of(context).caption
        .copyWith(color: Palette.of(context).textMuted),
  );
}

/// Whether [provider] has been signed in to or added: it has accounts, a
/// credential, or a sign-in under way ([flow]).
bool providerAdded(Provider provider, LoginFlow? flow) =>
    provider.accounts.isNotEmpty ||
    provider.signedInWith.isNotEmpty ||
    flow?.providerId == provider.id;

class _ProviderSettingsState extends State<ProviderSettings> {
  String? _failure;

  @override
  void initState() {
    super.initState();
    // What it shows read again where stale: model lists, keys' credit.
    fireAndLog(AppScope.of(context).providersShown());
  }

  Future<void> _start(Provider provider, LoginMethod method) async {
    setState(() => _failure = null);
    try {
      await AppScope.scope(context).login.start(
        provider.id,
        method.id,
        // As the card's own Add buttons do, keeping those signed in.
        addAccount: provider.accounts.isNotEmpty,
      );
    } on RequestFailure catch (error) {
      if (mounted) setState(() => _failure = error.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final workspace = widget.workspace;
    final flow = workspace.hasLogin() ? workspace.login : null;
    final added = [
      for (final provider in workspace.providers)
        if (providerAdded(provider, flow)) provider,
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final provider in added) ...[
          // By provider: a card's own state (a Refresh models failure)
          // stays with its provider as others sort in above it
          // (Abbacchio's row 1707).
          ProviderCard(
            key: ValueKey('provider-card:${provider.id}'),
            provider: provider,
            flow: flow?.providerId == provider.id ? flow : null,
          ),
          const SizedBox(height: Space.s),
        ],
        // Once a provider is signed in.
        if (added.isNotEmpty) ...[
          const SizedBox(height: Space.s),
          UsageAutoContinueSetting(on: workspace.usageAutoContinue),
          const SizedBox(height: Space.l),
        ],
        if (added.isEmpty) ...[
          Text(
            'Sign in to a provider so your agents can work.',
            style: TypeScale.of(context).body
                .copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.m),
        ] else
          const SizedBox(height: Space.xs),
        const AiProviderConsent(adding: true),
        const SizedBox(height: Space.m),
        ProviderPicker(
          providers: workspace.providers,
          onPick: _start,
          inline: added.isEmpty,
          placeholder: added.isEmpty ? 'Search providers' : 'Add a provider…',
        ),
        if (_failure != null)
          Padding(
            padding: const EdgeInsets.only(top: Space.s),
            child: ErrorLine(_failure!, about: 'In Settings:'),
          ),
      ],
    );
  }
}

/// Every way to sign in that [providers] offer, in their order: one per
/// provider and method ("Anthropic (Claude Pro/Max)", "Anthropic API key").
List<ProviderChoice> providerChoices(List<Provider> providers) => [
  for (final provider in providers)
    for (final method in provider.methods) (provider: provider, method: method),
];

/// One way to sign in: a provider and one of its methods.
typedef ProviderChoice = ({Provider provider, LoginMethod method});

/// Whether [choice] matches what's typed: every word of [query] is in its
/// provider's name or id, or its method's label, ignoring case.
bool choiceMatches(ProviderChoice choice, String query) {
  final haystack =
      '${choice.provider.name} ${choice.provider.id} ${choice.method.label}'
          .toLowerCase();
  return query
      .toLowerCase()
      .split(RegExp(r'\s+'))
      .where((word) => word.isNotEmpty)
      .every(haystack.contains);
}

/// A searchable list of every provider's ways to sign in: a field to type
/// in, which filters the list, and the list under it. Arrow keys move,
/// Enter picks, Esc closes the list (then clears what's typed). Picking
/// calls [onPick]; the caller starts the sign-in. As a dropdown (the
/// default) the list opens under the field while it has focus; [inline]
/// shows it always, below the field, for a page or a step that's about
/// choosing one.
class ProviderPicker extends StatefulWidget {
  const ProviderPicker({
    super.key,
    required this.providers,
    required this.onPick,
    this.inline = false,
    this.autofocus = false,
    this.placeholder = 'Add a provider…',
  });

  /// Workspace.providers: every provider the backend offers, signed in or
  /// not.
  final List<Provider> providers;
  final void Function(Provider provider, LoginMethod method) onPick;
  final bool inline;
  final bool autofocus;
  final String placeholder;

  /// A row's height, and how many show before the list scrolls.
  static const rowHeight = 32.0;
  static const visibleRows = 8;

  @override
  State<ProviderPicker> createState() => _ProviderPickerState();
}

class _ProviderPickerState extends State<ProviderPicker> {
  final _query = TextEditingController();
  late final _focus = FocusNode(debugLabel: 'provider picker');
  final _menu = OverlayPortalController();
  final _link = LayerLink();
  final _tapRegion = Object();
  final _scroll = ScrollController();
  var _highlighted = 0;

  @override
  void initState() {
    super.initState();
    _query.addListener(_typed);
    _focus.addListener(_focusChanged);
  }

  @override
  void dispose() {
    _query.dispose();
    _focus.dispose();
    _scroll.dispose();
    super.dispose();
  }

  List<ProviderChoice> get _matches => [
    for (final choice in providerChoices(widget.providers))
      if (choiceMatches(choice, _query.text)) choice,
  ];

  void _typed() {
    setState(() => _highlighted = 0);
    if (_scroll.hasClients) _scroll.jumpTo(0);
    if (_focus.hasFocus) _open();
  }

  void _focusChanged() {
    if (_focus.hasFocus) {
      _open();
    }
  }

  void _open() {
    if (!widget.inline && !_menu.isShowing) _menu.show();
  }

  void _close() {
    if (_menu.isShowing) _menu.hide();
  }

  void _pick(ProviderChoice choice) {
    _focus.unfocus();
    _close();
    // Cleared without opening again: focus leaves only after this frame.
    _query.removeListener(_typed);
    _query.clear();
    _query.addListener(_typed);
    setState(() => _highlighted = 0);
    widget.onPick(choice.provider, choice.method);
  }

  void _move(int by) {
    final count = _matches.length;
    if (count == 0) return;
    setState(() => _highlighted = (_highlighted + by + count) % count);
    // Keep it in view.
    if (!_scroll.hasClients) return;
    final top = _highlighted * ProviderPicker.rowHeight;
    final view = _scroll.position.viewportDimension;
    final at = _scroll.offset;
    if (top < at) {
      _scroll.jumpTo(top);
    } else if (top + ProviderPicker.rowHeight > at + view) {
      _scroll.jumpTo(top + ProviderPicker.rowHeight - view);
    }
  }

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.arrowDown) {
      if (!widget.inline && !_menu.isShowing) {
        _open();
      } else {
        _move(1);
      }
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.arrowUp) {
      _move(-1);
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.enter ||
        key == LogicalKeyboardKey.numpadEnter) {
      final matches = _matches;
      if (matches.isNotEmpty && (widget.inline || _menu.isShowing)) {
        _pick(matches[_highlighted.clamp(0, matches.length - 1)]);
      }
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.escape) {
      if (_menu.isShowing) {
        _close();
      } else if (_query.text.isNotEmpty) {
        _query.clear();
      } else {
        _focus.unfocus();
      }
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final field = Focus(
      canRequestFocus: false,
      skipTraversal: true,
      onKeyEvent: _key,
      child: Stack(
        alignment: Alignment.centerLeft,
        children: [
          TextField(
            controller: _query,
            focusNode: _focus,
            placeholder: widget.placeholder,
            kind: InputKind.text,
            textInputAction: TextInputAction.search,
            autocorrect: false,
            autofocus: widget.autofocus,
          ),
          // The field's own padding holds the text clear of nothing: the
          // glyph sits in it at the right.
          Positioned(
            right: Space.s + 2,
            child: IgnorePointer(
              child: Chevron(color: palette.textMuted, down: true),
            ),
          ),
        ],
      ),
    );
    if (widget.inline) {
      return Unselectable(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            field,
            const SizedBox(height: Space.xs),
            Container(
              padding: const EdgeInsets.all(Space.xs),
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(Radii.panel),
                border: Border.all(color: palette.hairline),
              ),
              child: ListenableBuilder(
                listenable: _query,
                builder: (context, _) => _list(palette),
              ),
            ),
          ],
        ),
      );
    }
    return Unselectable(
      child: TapRegion(
        groupId: _tapRegion,
        onTapOutside: (_) => _close(),
        child: CompositedTransformTarget(
          link: _link,
          child: OverlayPortal(
            controller: _menu,
            overlayChildBuilder: _dropdown,
            child: field,
          ),
        ),
      ),
    );
  }

  /// The list as a floating layer under the field, as wide as it.
  Widget _dropdown(BuildContext context) {
    final palette = Palette.of(context);
    final box = this.context.findRenderObject();
    final width = box is RenderBox && box.hasSize ? box.size.width : null;
    return CompositedTransformFollower(
      link: _link,
      targetAnchor: Alignment.bottomLeft,
      offset: const Offset(0, Space.xs),
      child: Align(
        alignment: Alignment.topLeft,
        child: TapRegion(
          groupId: _tapRegion,
          child: Container(
            width: width,
            constraints: const BoxConstraints(minWidth: 240),
            padding: const EdgeInsets.all(Space.xs),
            decoration: BoxDecoration(
              color: palette.sidebar,
              borderRadius: BorderRadius.circular(Radii.panel),
              border: Border.all(color: palette.hairline),
              boxShadow: floatingShadow,
            ),
            child: ListenableBuilder(
              listenable: _query,
              builder: (context, _) => _list(palette),
            ),
          ),
        ),
      ),
    );
  }

  Widget _list(Palette palette) {
    final matches = _matches;
    if (matches.isEmpty) {
      return SizedBox(
        height: ProviderPicker.rowHeight,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          child: Align(
            alignment: Alignment.centerLeft,
            child: Text(
              widget.providers.isEmpty
                  ? 'No providers to add.'
                  : 'No provider matches “${_query.text.trim()}”.',
              style: TypeScale.of(context).body
                  .copyWith(color: palette.textFaint),
            ),
          ),
        ),
      );
    }
    final rows = matches.length < ProviderPicker.visibleRows
        ? matches.length
        : ProviderPicker.visibleRows;
    return SizedBox(
      height: rows * ProviderPicker.rowHeight,
      child: ListView.builder(
        controller: _scroll,
        padding: EdgeInsets.zero,
        itemExtent: ProviderPicker.rowHeight,
        itemCount: matches.length,
        itemBuilder: (context, index) => _row(palette, matches[index], index),
      ),
    );
  }

  Widget _row(Palette palette, ProviderChoice choice, int index) {
    final (:provider, :method) = choice;
    final signedIn =
        provider.accounts.isNotEmpty || provider.signedInWith.isNotEmpty;
    // The provider's name when the method's label doesn't say it.
    final aside =
        method.label.toLowerCase().contains(provider.name.toLowerCase())
        ? ''
        : provider.name;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _highlighted = index),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => _pick(choice),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          decoration: BoxDecoration(
            color: index == _highlighted
                ? palette.selected
                : const Color(0x00000000),
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  method.label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TypeScale.of(context).body
                      .copyWith(color: palette.text),
                ),
              ),
              if (aside.isNotEmpty || signedIn) ...[
                const SizedBox(width: Space.s),
                Text(
                  [
                    aside,
                    if (signedIn) 'Signed in',
                  ].where((part) => part.isNotEmpty).join(' · '),
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textFaint),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
