// Choosing a model (Squalo's U3, #343): with OpenRouter signed in there are
// hundreds, some named alike across providers ("Claude Opus 5" from
// Anthropic and from OpenRouter). The field names the provider beside the
// model, and opens a list grouped by provider, with a search.
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart' show RequestFailure;
import 'controls.dart';
import 'forms.dart' show FormFrame;
import 'identity.dart';
import 'mobile/metrics.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart' show Unselectable;

/// The provider part of a model's id ("anthropic" of
/// "anthropic/claude-opus-5"); empty for an id without one.
String modelProviderId(String modelId) {
  final slash = modelId.indexOf('/');
  return slash < 0 ? '' : modelId.substring(0, slash);
}

/// The provider's name as Settings → Providers says it ("OpenRouter"), or
/// its id when this Claktory doesn't list it.
String modelProviderName(Workspace workspace, String modelId) {
  final id = modelProviderId(modelId);
  return workspace.providers
          .where((provider) => provider.id == id)
          .firstOrNull
          ?.name ??
      id;
}

/// A model's display name, falling back to its id.
String _name(Model model) => model.name.isEmpty ? model.id : model.name;

/// A model field: what's chosen, with its provider after it, and a click
/// (or Enter, Space or Down) opens [ModelChooser]. [extra] are choices
/// before the models, such as the Claktory default or "No model
/// available" for one that can't run; [value] is one of them or a model's
/// id.
class ModelSelect extends StatefulWidget {
  const ModelSelect({
    super.key,
    required this.workspace,
    required this.value,
    required this.onChanged,
    this.extra = const [],
    this.title = 'Model',
  });

  final Workspace workspace;
  final String value;
  final ValueChanged<String> onChanged;
  final List<SelectOption<String>> extra;

  /// Over the list of models.
  final String title;

  @override
  State<ModelSelect> createState() => _ModelSelectState();
}

class _ModelSelectState extends State<ModelSelect> {
  late final _focus = FocusNode(onKeyEvent: _onKey);
  var _hovered = false;

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) return KeyEventResult.ignored;
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.enter ||
        key == LogicalKeyboardKey.space ||
        key == LogicalKeyboardKey.arrowDown) {
      _open();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  Future<void> _open() async {
    String? chosen;
    await showModal(
      context,
      (context, close) => ModelChooser(
        workspace: widget.workspace,
        value: widget.value,
        extra: widget.extra,
        title: widget.title,
        onChoose: (id) {
          chosen = id;
          close();
        },
        onCancel: close,
      ),
    );
    if (chosen case final id? when id != widget.value) widget.onChanged(id);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final phone = touchFirst(context);
    final model = widget.workspace.models
        .where((model) => model.id == widget.value)
        .firstOrNull;
    final extra = widget.extra
        .where((option) => option.value == widget.value)
        .firstOrNull;
    final label = model != null
        ? _name(model)
        : extra?.label ?? 'No model available';
    final provider = model == null
        ? ''
        : modelProviderName(widget.workspace, model.id);
    return Unselectable(
      child: Semantics(
        button: true,
        label: provider.isEmpty
            ? '${widget.title}: $label'
            : '${widget.title}: $label, $provider',
        onTap: _open,
        excludeSemantics: true,
        child: Focus(
          focusNode: _focus,
          child: ListenableBuilder(
            listenable: _focus,
            builder: (context, _) => MouseRegion(
              cursor: SystemMouseCursors.click,
              onEnter: (_) => setState(() => _hovered = true),
              onExit: (_) => setState(() => _hovered = false),
              child: GestureDetector(
                onTap: _open,
                child: Container(
                  height: phone ? null : 30,
                  constraints: phone
                      ? BoxConstraints(minHeight: Metrics.of(context).target)
                      : null,
                  padding: EdgeInsets.symmetric(
                    horizontal: phone ? Space.m : Space.s + 2,
                    vertical: phone ? Space.s : 0,
                  ),
                  decoration: BoxDecoration(
                    color: _hovered ? palette.hover : palette.canvas,
                    borderRadius: BorderRadius.circular(Radii.control),
                    border: Border.all(
                      color: _focus.hasFocus
                          ? palette.signal
                          : palette.hairline,
                    ),
                  ),
                  child: Row(
                    children: [
                      Expanded(
                        child: Text.rich(
                          TextSpan(
                            children: [
                              TextSpan(text: label),
                              if (provider.isNotEmpty)
                                TextSpan(
                                  text: '  $provider',
                                  style: TextStyle(color: palette.textFaint),
                                ),
                            ],
                          ),
                          maxLines: phone ? null : 1,
                          overflow: phone ? null : TextOverflow.ellipsis,
                          style: type.body.copyWith(color: palette.text),
                        ),
                      ),
                      const SizedBox(width: Space.s),
                      Chevron(color: palette.textMuted, down: true),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Every model that can run, grouped by provider in Settings → Providers'
/// order, after [extra]; a search narrows them by name, id or provider
/// (every word). A click chooses one; Enter in the search, or Choose,
/// takes the first that matches. A model id the list doesn't have
/// ("openrouter/google/gemini-4-neon", or "gemini-4-neon" under each
/// provider signed in to) is offered under the list, "Use … from
/// OpenRouter": the provider is asked whether this sign-in has it before
/// it's chosen, and its refusal shows (friend bug #14).
class ModelChooser extends StatefulWidget {
  const ModelChooser({
    super.key,
    required this.workspace,
    required this.value,
    required this.onChoose,
    required this.onCancel,
    this.extra = const [],
    this.title = 'Model',
    this.embedded = false,
  });

  final Workspace workspace;
  final String value;
  final List<SelectOption<String>> extra;
  final String title;
  final ValueChanged<String> onChoose;
  final VoidCallback onCancel;

  /// Inside a popover of its own (an iPad's model popover): the search and
  /// the list only, scrolling, the chosen one checked, with no title and no
  /// Cancel or Choose, as a choice applies at once there.
  final bool embedded;

  @override
  State<ModelChooser> createState() => _ModelChooserState();
}

class _ModelChooserState extends State<ModelChooser> {
  final _search = TextEditingController();

  /// The model id being checked with its provider, until it answers.
  String? _checking;

  /// Why the provider refused the last one checked.
  String? _error;

  @override
  void initState() {
    super.initState();
    _search.addListener(
      () => setState(() {
        if (_checking == null) _error = null;
      }),
    );
  }

  /// What a model's id may hold, as the backend checks it.
  static final _idChars = RegExp(r'^[A-Za-z0-9._\-:/]+$');

  /// Model ids the search names that the list doesn't have, each with its
  /// provider's name: the search itself when it starts with a provider's
  /// id, else the search under each provider signed in to.
  List<({String id, String provider})> _typed() {
    final query = _search.text.trim();
    if (query.isEmpty || !_idChars.hasMatch(query)) return const [];
    final workspace = widget.workspace;
    bool listed(String id) => workspace.models.any((model) => model.id == id);
    final slash = query.indexOf('/');
    if (slash > 0) {
      final named = workspace.providers
          .where((provider) => provider.id == query.substring(0, slash))
          .firstOrNull;
      if (named != null) {
        if (slash == query.length - 1 || listed(query)) return const [];
        return [(id: query, provider: named.name)];
      }
    }
    return [
      for (final provider in workspace.providers)
        if ((provider.accounts.isNotEmpty ||
                provider.signedInWith.isNotEmpty) &&
            !listed('${provider.id}/$query'))
          (id: '${provider.id}/$query', provider: provider.name),
    ];
  }

  /// Asks [id]'s provider whether this sign-in has it, and chooses it once
  /// it's in the list.
  Future<void> _use(String id) async {
    if (_checking != null) return;
    setState(() {
      _checking = id;
      _error = null;
    });
    try {
      await AppScope.of(context).addModel(id);
      if (mounted) widget.onChoose(id);
    } on RequestFailure catch (error) {
      if (mounted) setState(() => _error = error.message);
    } finally {
      if (mounted) setState(() => _checking = null);
    }
  }

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  /// The models matching the search, by provider in Settings' order (then
  /// any it doesn't list), each provider's in the catalog's order.
  List<(String, List<Model>)> _groups() {
    final words = _search.text.toLowerCase().split(RegExp(r'\s+'))
      ..removeWhere((word) => word.isEmpty);
    final workspace = widget.workspace;
    final byProvider = <String, List<Model>>{};
    for (final model in workspace.models) {
      final provider = modelProviderName(workspace, model.id);
      final haystack = '${_name(model)} ${model.id} $provider'.toLowerCase();
      if (!words.every(haystack.contains)) continue;
      byProvider.putIfAbsent(modelProviderId(model.id), () => []).add(model);
    }
    final order = [
      for (final provider in workspace.providers)
        if (byProvider.containsKey(provider.id)) provider.id,
      for (final id in byProvider.keys)
        if (!workspace.providers.any((provider) => provider.id == id)) id,
    ];
    return [
      for (final id in order)
        (
          workspace.providers
                  .where((provider) => provider.id == id)
                  .firstOrNull
                  ?.name ??
              id,
          byProvider[id]!,
        ),
    ];
  }

  /// What Enter or Choose does: take the first choice the search leaves,
  /// or, when no model in the list matches, use the first id it names.
  VoidCallback? _first() {
    if (_checking != null) return null;
    if (_search.text.trim().isEmpty && widget.extra.isNotEmpty) {
      final value = widget.extra.first.value;
      return () => widget.onChoose(value);
    }
    final groups = _groups();
    if (groups.isNotEmpty) {
      final id = groups.first.$2.first.id;
      return () => widget.onChoose(id);
    }
    final typed = _typed();
    if (typed.isEmpty) return null;
    return () => _use(typed.first.id);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final query = _search.text.trim();
    final extra = query.isEmpty ? widget.extra : const <SelectOption<String>>[];
    final groups = _groups();
    final first = _first();
    // Offered when nothing in the list matches, or the search is a whole
    // model id ("openrouter/…"), after the list and out of Enter's way
    // while the list has a match.
    final typed = groups.isEmpty || query.contains('/')
        ? _typed()
        : const <({String id, String provider})>[];
    final search = <Widget>[
      TextField(
        controller: _search,
        // In a popover on touch, no keyboard until the search is tapped.
        autofocus: !isPhoneLayout(context) && !widget.embedded,
        placeholder: 'Search models or providers',
        semanticsLabel: 'Search models',
        // What matches the words as they are now, even before the
        // list has redrawn for them.
        onSubmitted: (_) => _first()?.call(),
      ),
      const SizedBox(height: Space.s),
    ];
    final rows = <Widget>[
      for (final option in extra)
        _ModelRow(
          label: option.label,
          chosen: option.value == widget.value,
          onTap: () => widget.onChoose(option.value),
        ),
      if (groups.isEmpty && typed.isEmpty && query.isNotEmpty)
        Text(
          'No model matches “$query”.',
          style: type.body.copyWith(color: palette.textMuted),
        ),
      for (final (provider, models) in groups) ...[
        Padding(
          padding: const EdgeInsets.only(
            top: Space.s,
            left: Space.s,
            bottom: 2,
          ),
          child: Text(
            provider,
            style: type.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
        for (final model in models)
          _ModelRow(
            label: _name(model),
            // What it can do beyond writing, as the provider says it.
            note: model.imageOutput ? modelMakesImages : '',
            chosen: model.id == widget.value,
            check: widget.embedded,
            onTap: () => widget.onChoose(model.id),
          ),
      ],
      if (typed.isNotEmpty) ...[
        Padding(
          padding: const EdgeInsets.only(
            top: Space.s,
            left: Space.s,
            bottom: 2,
          ),
          child: Text(
            'Not in the list',
            style: type.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
        for (final (:id, :provider) in typed)
          _ModelRow(
            label: _checking == id
                ? 'Checking with $provider…'
                : 'Use “${id.substring(id.indexOf('/') + 1)}” from '
                      '$provider',
            chosen: false,
            onTap: () => _use(id),
          ),
      ],
    ];
    if (widget.embedded) {
      return Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // A search only for a long list (Lucy): two models need none.
          if (widget.workspace.models.length + widget.extra.length >
              embeddedSearchPast)
            ...search,
          Flexible(
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: rows,
              ),
            ),
          ),
          if (_error case final error?)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: Text(
                error,
                style: type.body.copyWith(color: palette.danger),
              ),
            ),
        ],
      );
    }
    return FormFrame(
      title: widget.title,
      fields: [...search, ...rows],
      error: _error,
      submitLabel: _checking == null ? 'Choose' : 'Checking…',
      onSubmit: first,
      onCancel: widget.onCancel,
    );
  }
}

/// An embedded list ([ModelChooser.embedded]) longer than this has a
/// search.
const embeddedSearchPast = 8;

/// What the model list says after a model that can make images (#403,
/// Model.image_output).
const modelMakesImages = 'Makes images';

/// One choice: its name, then [note] faint, the chosen one tinted.
class _ModelRow extends StatelessWidget {
  const _ModelRow({
    required this.label,
    required this.chosen,
    required this.onTap,
    this.note = '',
    this.check = false,
  });

  final String label;
  final String note;
  final bool chosen;

  /// The chosen one has a check at its end too.
  final bool check;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Unselectable(
      child: Semantics(
        button: true,
        selected: chosen,
        child: Pressable(
          onTap: onTap,
          builder: (context, hovered) => Container(
            constraints: BoxConstraints(
              // A finger's row on touch: a phone, an iPad's popover.
              minHeight: touchFirst(context) ? Metrics.of(context).target : 30,
            ),
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            alignment: Alignment.centerLeft,
            decoration: BoxDecoration(
              color: chosen
                  ? palette.selected
                  : hovered
                  ? palette.hover
                  : null,
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Row(
              children: [
                Expanded(
                  child: Text.rich(
                    TextSpan(
                      children: [
                        TextSpan(text: label),
                        if (note.isNotEmpty)
                          TextSpan(
                            text: '  $note',
                            style: TextStyle(color: palette.textFaint),
                          ),
                      ],
                    ),
                    style: TypeScale.of(context).body
                        .copyWith(color: palette.text),
                  ),
                ),
                if (check && chosen) ...[
                  const SizedBox(width: Space.s),
                  ExtensionGlyph(
                    icon: 'check',
                    name: 'Chosen',
                    color: palette.text,
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}
